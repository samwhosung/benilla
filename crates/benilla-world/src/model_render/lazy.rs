//! **Deferred material realization** — a `WowModelMaterial` becomes an *asset* the first frame
//! something drawable is bound to it, not the moment a spawner builds it.
//!
//! Why: every material asset costs the GPU lane real per-frame CPU whether or not it is ever
//! drawn. Bevy's non-bindless `AsBindGroup` gives each material its own bind group and one
//! buffer per `#[uniform]` binding (two for this material), and wgpu's pass encoder allocates
//! and clears usage trackers sized to the device's *live* buffer and texture counts for every
//! render pass it encodes — a per-pass term that scales with residency, not with what is drawn.
//! Measured on the crowd rig (1929's follow-on, the `wgpu_bufs=` census): 9.9k materials,
//! 20.7k buffers and 10.2k bind groups alive at the Stormwind auction house with 36 entity
//! batches drawn, because the batch builder ([`super::batch::M2BatchMaterials`]) hands every
//! spawner its whole variant set — steady, the two interior lanes, their blend twins, the
//! depth-prime twin — and the variants a batch never switches to sat in the store forever.
//!
//! The mechanism: the builder reserves a handle (`Assets::reserve_handle`) and parks the built
//! value here, keyed by the reserved id. [`realize_bound`] walks the bound handles and inserts
//! the asset for every one whose entity is view-visible this frame. A handle every holder has
//! dropped arrives as the store's own `AssetEvent::Unused` (it fires for a reserved id too) and
//! the parked value goes with it. Nothing about the handles changes: a part still stores and
//! swaps `Handle<WowModelMaterial>`s, and every reader that clones a material *before* binding
//! it — the portrait booth's relight twins, a spell kit's per-instance copy, the pipeline
//! warmer — calls [`realize`] first, which is a no-op for an asset that already exists.
//!
//! **The deadline is the store's event PUBLISH, not extraction.** `Assets::insert` does not
//! announce anything: it pushes an `AssetEvent::Added` onto the store's private queue, and
//! `Assets::<A>::asset_events` — scheduled by bevy_asset in **`PostUpdate`** — is what writes
//! that queue into the message stream `extract_render_asset` reads. A sweep placed after it
//! (this module's original `Last`) therefore realizes an asset the render world is not told
//! about until the NEXT frame's publish, and `queue_material_meshes` skips every entity whose
//! material is missing from `RenderAssets` — one frame of the object not drawn. That is
//! invisible for a first bind (nothing was on screen to lose) and plainly visible for a
//! **swap onto an entity that is already drawing**: the appear fade's blend→cutout latch, where
//! a freshly logged-in NPC blinked out for a frame at the instant it finished fading in, and
//! the doodad distance fade's latch and the far-side water twin behind it. Pinned by
//! [`tests::the_sweep_publishes_its_added_in_the_frame_it_realizes`].
//!
//! **So the sweep is the top of `PostUpdate`, and the visibility it reads is last frame's** —
//! not a preference, the only placement bevy's own graph leaves. `AssetEventSystems` is ordered
//! *before* `CalculateBounds`, which is before `CheckVisibility` (bevy_camera's
//! `VisibilityPlugin`), so "after this frame's verdict" and "before the publish" is a cycle; and
//! `reset_view_visibility` (in `VisibilityPropagate`, also before `CheckVisibility`) clears every
//! current bit, so a sweep between the two would read `false` for the whole world. Before both,
//! `ViewVisibility::get` is still the previous frame's answer — which is exactly the right
//! question here: the entity this exists to keep drawing is the one that WAS drawing and had its
//! material swapped under it in `Update`.
//!
//! A binding whose entity was not visible last frame — a fresh spawn, an unparked part, the
//! depth-prime twin `zfill::sync_zfill_twins` spawns in `PostUpdate` — realizes on the next
//! sweep and draws on its second frame. That is what the `Last` placement did too (a
//! just-spawned entity's material was realized on its spawn frame and published on the next),
//! so nothing pays more than before, and everything that swaps pays a frame less.
//!
//! **The table is a process global, deliberately.** It is a side of `Assets<WowModelMaterial>`
//! — "reserved, value parked" — that the asset store has no slot for, and the builder that
//! needs it ([`super::model_material`]) is a free function with a two-dozen-argument signature
//! reached from seven lanes through three caches (the engine's, the streamer's, the warmer's
//! `Local`). Threading one more resource through all of them for what is a property of the
//! store, not of any cache, is the wrong shape; one mutex, locked once per system run, is the
//! right one. Main-world only, like the store it shadows.

use std::collections::HashMap;
use std::sync::Mutex;

use bevy::asset::{Asset, AssetId};
use bevy::camera::visibility::ViewVisibility;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;

/// The parked values of one asset type: built, handle reserved, not yet in the store. Generic
/// so the table's law is testable on a stub asset (a real `WowModelMaterial` carries a GPU
/// buffer and cannot exist without a device).
pub struct Parked<A: Asset>(HashMap<AssetId<A>, A>);

impl<A: Asset> Default for Parked<A> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

impl<A: Asset> Parked<A> {
    /// Reserve a handle for `asset` and park the value.
    pub fn defer(&mut self, store: &Assets<A>, asset: A) -> Handle<A> {
        let handle = store.reserve_handle();
        self.0.insert(handle.id(), asset);
        handle
    }

    /// Make the asset behind `id` exist now, if its value is parked. Returns whether the store
    /// holds it afterwards — `false` only for an id this table never saw and the store does not
    /// have (a foreign handle, or one already dropped everywhere).
    pub fn realize(&mut self, store: &mut Assets<A>, id: AssetId<A>) -> bool {
        if store.contains(id) {
            return true;
        }
        let Some(asset) = self.0.remove(&id) else {
            return false;
        };
        // `Err` = the reserved index was dropped and re-minted with a new generation since this
        // value was parked; the `Unused` purge simply has not run yet. Nothing to insert.
        store.insert(id, asset).is_ok()
    }

    /// Insert every parked value.
    pub fn realize_all(&mut self, store: &mut Assets<A>) {
        for (id, asset) in self.0.drain() {
            let _ = store.insert(id, asset);
        }
    }

    /// The store reported `id` unused: whoever held the handle is gone, and so is the value.
    pub fn purge(&mut self, id: AssetId<A>) {
        self.0.remove(&id);
    }

    /// The value behind `id` **wherever it currently lives** — the store when the asset exists,
    /// the parked half when it does not yet. `None` only when neither holds it (a foreign handle,
    /// or one dropped everywhere).
    ///
    /// This is the address a build-time stamp needs. Deferral split "the material a spawner just
    /// built" into two homes, and a lane that reaches for the store alone reads a *live feature*
    /// as an absent asset: the mat-anim registration wrote its table slot with
    /// `Assets::get_mut`, got `None` for every deferred material, and silently froze every
    /// UV-scroll and animated-tint batch in the world. Realizing the asset to
    /// stamp it would work and would also throw away exactly the residency deferral buys; the
    /// stamp belongs in the value, not in the store it happens to be sitting outside of.
    pub fn value_mut<'a>(
        &'a mut self,
        store: &'a mut Assets<A>,
        id: AssetId<A>,
    ) -> Option<&'a mut A> {
        // Store first: once realized it is the truth, and a parked value for a live id could
        // only be a stale leftover.
        if store.contains(id) {
            return store.get_mut(id);
        }
        self.0.get_mut(&id)
    }

    /// [`Self::value_mut`]'s read-only twin — for a reader that must not dirty the asset
    /// (`Assets::get_mut` marks it `Modified`, which is a bind-group rebuild on the Metal
    /// non-bindless path: the exact cost 1381 removed from this lane).
    pub fn value<'a>(&'a self, store: &'a Assets<A>, id: AssetId<A>) -> Option<&'a A> {
        store.get(id).or_else(|| self.0.get(&id))
    }

    /// Either half holds a value for `id` — "this material is alive", the deferral-aware form of
    /// `Assets::contains`. A registry that evicts on the store alone drops every entry whose
    /// material is merely parked.
    pub fn holds(&self, store: &Assets<A>, id: AssetId<A>) -> bool {
        store.contains(id) || self.0.contains_key(&id)
    }

    /// Realize every parked value some *visible* binding names. `bound` yields each binding
    /// with its view-visibility (`None` = not on the visibility lane at all — a booth part
    /// before its camera, a test world — which counts as visible: bound is enough).
    pub fn realize_visible(
        &mut self,
        store: &mut Assets<A>,
        bound: impl IntoIterator<Item = (AssetId<A>, Option<bool>)>,
    ) {
        if self.0.is_empty() {
            return;
        }
        for (id, visible) in bound {
            if visible == Some(false) || store.contains(id) {
                continue;
            }
            if let Some(asset) = self.0.remove(&id) {
                let _ = store.insert(id, asset);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

static PENDING: Mutex<Option<Parked<WowModelMaterial>>> = Mutex::new(None);

fn with_pending<R>(f: impl FnOnce(&mut Parked<WowModelMaterial>) -> R) -> R {
    let mut guard = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Parked::default))
}

/// Reserve a handle for `material` and park the value until something bound to the handle is
/// drawn. The builder's replacement for `Assets::add`.
pub fn defer(
    materials: &Assets<WowModelMaterial>,
    material: WowModelMaterial,
) -> Handle<WowModelMaterial> {
    with_pending(|p| p.defer(materials, material))
}

/// Make the asset behind `id` exist now — see [`Parked::realize`].
pub fn realize(materials: &mut Assets<WowModelMaterial>, id: AssetId<WowModelMaterial>) -> bool {
    with_pending(|p| p.realize(materials, id))
}

/// Insert every parked value. For a lane that reads materials back right after building them
/// (the pipeline warmer, which inspects each variant it built to derive its far-side twins).
pub fn realize_all(materials: &mut Assets<WowModelMaterial>) {
    with_pending(|p| p.realize_all(materials));
}

/// Apply `f` to a built material **wherever it lives** — see [`Parked::value_mut`]. `None` when
/// neither the store nor the parked half holds `id`.
///
/// The write-side counterpart of [`realize`], and the right call for a **one-shot build-time
/// stamp**: a lane that must leave a mark on a material it just built, without forcing the
/// material into the store (which is what deferral exists to avoid) and without caring whether
/// something has drawn it yet. The closure form is not a style choice — the parked table lives
/// behind a mutex, so a `&mut` into it cannot leave the lock.
pub fn with_material_mut<R>(
    materials: &mut Assets<WowModelMaterial>,
    id: AssetId<WowModelMaterial>,
    f: impl FnOnce(&mut WowModelMaterial) -> R,
) -> Option<R> {
    with_pending(|p| p.value_mut(materials, id).map(f))
}

/// Read a built material **wherever it lives** — [`with_material_mut`]'s non-dirtying twin
/// ([`Parked::value`]). What a readout wants: a probe that reached for the store alone would
/// report a parked material's fields as absent and reproduce the very confusion it exists to
/// resolve.
pub fn with_material<R>(
    materials: &Assets<WowModelMaterial>,
    id: AssetId<WowModelMaterial>,
    f: impl FnOnce(&WowModelMaterial) -> R,
) -> Option<R> {
    with_pending(|p| p.value(materials, id).map(f))
}

/// Either half holds a value for `id` — see [`Parked::holds`]. The liveness test a registry
/// keyed by material asset id must evict on; `Assets::contains` alone reads a parked material as
/// dead.
pub fn holds(materials: &Assets<WowModelMaterial>, id: AssetId<WowModelMaterial>) -> bool {
    // The store answers without the lock, and a REALIZED material is the common case for
    // everything a per-frame lane asks about (only a parked one can be undrawn), so the mutex is
    // reached for exactly the entries the store cannot settle.
    materials.contains(id) || with_pending(|p| p.holds(materials, id))
}

/// How many values are parked — the census figure beside `mats=`.
pub fn pending_len() -> usize {
    with_pending(|p| p.len())
}

/// The top of `PostUpdate`: realize the material of every bound entity that was view-visible on
/// the last frame's verdict, and drop the parked value of every handle the store reports unused.
/// Before `AssetEventSystems` (so the `Added` this insert queues is published in time for this
/// frame's extraction) and before `VisibilityPropagate` (whose `reset_view_visibility` clears the
/// bits this reads) — see the module doc's deadline paragraph for why those are the only bounds.
pub(super) fn realize_bound(
    bound: Query<(&MeshMaterial3d<WowModelMaterial>, Option<&ViewVisibility>)>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
    mut unused: MessageReader<AssetEvent<WowModelMaterial>>,
) {
    with_pending(|p| {
        for event in unused.read() {
            if let AssetEvent::Unused { id } = event {
                p.purge(*id);
            }
        }
        p.realize_visible(
            &mut materials,
            bound.iter().map(|(m, v)| (m.0.id(), v.map(|v| v.get()))),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Asset, TypePath)]
    struct Stub(u8);

    #[test]
    fn deferred_asset_is_absent_until_realized() {
        let mut store = Assets::<Stub>::default();
        let mut parked = Parked::default();
        let handle = parked.defer(&store, Stub(1));
        assert!(!store.contains(handle.id()));
        assert_eq!(parked.len(), 1);
        assert!(parked.realize(&mut store, handle.id()));
        assert_eq!(store.get(&handle).map(|s| s.0), Some(1));
        assert!(parked.is_empty());
        // Realizing again is a no-op that still reports the asset present.
        assert!(parked.realize(&mut store, handle.id()));
        // An id nobody parked and the store lacks: false, nothing inserted.
        let stray = store.reserve_handle();
        assert!(!parked.realize(&mut store, stray.id()));
    }

    /// **The law decision 2038 was written from**: a value that is only PARKED is still a live
    /// material — writable in place, and `holds`-alive — so a lane that stamps a material it just
    /// built lands its mark whether or not anything has drawn it yet, and a registry keyed by
    /// asset id does not evict it as dead. Reaching for the store alone is what silently froze
    /// every UV-scroll and animated-tint batch in the world.
    #[test]
    fn a_parked_value_is_writable_and_counts_as_held() {
        let mut store = Assets::<Stub>::default();
        let mut parked = Parked::default();
        let deferred = parked.defer(&store, Stub(1));
        // The store does not have it — the read that used to answer "no such material".
        assert!(!store.contains(deferred.id()));
        assert!(store.get_mut(deferred.id()).is_none());
        // …and both deferral-aware reads do.
        assert!(parked.holds(&store, deferred.id()));
        assert_eq!(parked.value(&store, deferred.id()).map(|s| s.0), Some(1));
        parked
            .value_mut(&mut store, deferred.id())
            .expect("the parked value is addressable")
            .0 = 7;
        // The stamp survives realization: the value goes into the store as written.
        assert!(parked.realize(&mut store, deferred.id()));
        assert_eq!(store.get(&deferred).map(|s| s.0), Some(7));
        // Realized, the store is the half that answers — and still the same value.
        assert_eq!(parked.value(&store, deferred.id()).map(|s| s.0), Some(7));
        parked
            .value_mut(&mut store, deferred.id())
            .expect("realized values stay addressable")
            .0 = 9;
        assert_eq!(store.get(&deferred).map(|s| s.0), Some(9));
        // An id neither half holds is the only `None` — and the only not-held.
        let stray = store.reserve_handle();
        assert!(!parked.holds(&store, stray.id()));
        assert!(parked.value(&store, stray.id()).is_none());
        assert!(parked.value_mut(&mut store, stray.id()).is_none());
    }

    /// **The deadline is the publish, not the insert** — the law the `Last` placement broke.
    /// `Assets::insert` only queues its `AssetEvent::Added`; `Assets::<A>::asset_events`
    /// (bevy_asset, `PostUpdate`) is what writes it into the message stream the render world
    /// extracts from. A sweep that realizes AFTER that system announces nothing this frame, so
    /// `extract_render_asset` never extracts the material and `queue_material_meshes` skips every
    /// entity bound to it — one frame not drawn, seen as a blink the instant an appear fade
    /// latched from its blend twin onto its steady material.
    ///
    /// The reader stands in for extraction: `ExtractSchedule` runs immediately after `Last` and
    /// reads the same message buffer, so "visible to a `Last` reader" is exactly "visible to
    /// extraction".
    #[test]
    fn the_sweep_publishes_its_added_in_the_frame_it_realizes() {
        use bevy::asset::{AssetEvent, AssetEventSystems, AssetPlugin};
        use bevy::camera::visibility::VisibilitySystems;

        #[derive(Resource)]
        struct Table(Parked<Stub>);
        #[derive(Resource)]
        struct Reserved(Handle<Stub>);
        /// Frames on which a reader standing where extraction stands saw the `Added`.
        #[derive(Resource, Default)]
        struct Seen(Vec<u32>);
        #[derive(Resource, Default)]
        struct Frame(u32);

        fn realize_reserved(
            mut table: ResMut<Table>,
            mut store: ResMut<Assets<Stub>>,
            reserved: Res<Reserved>,
        ) {
            table.0.realize(&mut store, reserved.0.id());
        }

        fn extraction_stand_in(
            mut events: MessageReader<AssetEvent<Stub>>,
            mut seen: ResMut<Seen>,
            frame: Res<Frame>,
        ) {
            if events.read().any(|e| matches!(e, AssetEvent::Added { .. })) {
                seen.0.push(frame.0);
            }
        }

        fn tick(mut frame: ResMut<Frame>) {
            frame.0 += 1;
        }

        /// An app whose realize sweep sits where `place` puts it; returns the frames on which
        /// extraction could see the `Added`.
        fn frames_added_was_visible(place: impl FnOnce(&mut App)) -> Vec<u32> {
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, AssetPlugin::default()))
                .init_asset::<Stub>()
                .init_resource::<Seen>()
                .init_resource::<Frame>()
                .add_systems(First, tick);
            let mut table = Parked::default();
            let handle = {
                let store = app.world().resource::<Assets<Stub>>();
                table.defer(store, Stub(1))
            };
            app.insert_resource(Table(table))
                .insert_resource(Reserved(handle));
            place(&mut app);
            // Ordered last in `Last`, i.e. the final read before `ExtractSchedule`.
            app.add_systems(Last, extraction_stand_in);
            app.update();
            app.update();
            app.world().resource::<Seen>().0.clone()
        }

        // The shipped placement: realized in `Last`, published by the NEXT frame's PostUpdate.
        assert_eq!(
            frames_added_was_visible(|app| {
                app.add_systems(Last, realize_reserved.before(extraction_stand_in));
            }),
            vec![2],
            "a `Last` realize is announced a frame late — the blink"
        );

        // Where it lives now: the top of `PostUpdate`, ahead of the publish.
        assert_eq!(
            frames_added_was_visible(|app| {
                app.add_systems(
                    PostUpdate,
                    realize_reserved
                        .before(AssetEventSystems)
                        .before(VisibilitySystems::VisibilityPropagate),
                );
            }),
            vec![1],
            "the sweep's own frame must carry the `Added`"
        );
    }

    #[test]
    fn bound_and_visible_realizes_in_the_walk_hidden_does_not() {
        let mut store = Assets::<Stub>::default();
        let mut parked = Parked::default();
        let shown = parked.defer(&store, Stub(1));
        let hidden = parked.defer(&store, Stub(2));
        let unlaned = parked.defer(&store, Stub(3));
        parked.realize_visible(
            &mut store,
            [
                (shown.id(), Some(true)),
                (hidden.id(), Some(false)),
                (unlaned.id(), None),
            ],
        );
        assert!(store.contains(shown.id()), "visible ⇒ realized");
        assert!(!store.contains(hidden.id()), "hidden ⇒ still parked");
        assert!(
            store.contains(unlaned.id()),
            "no visibility lane ⇒ bound is enough"
        );
        assert_eq!(parked.len(), 1);
        parked.purge(hidden.id());
        assert!(
            parked.is_empty(),
            "the store's Unused drops the parked value"
        );
    }
}
