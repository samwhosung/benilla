//! Deferred material realization: a `WowModelMaterial` becomes an asset the first frame something
//! bound to it is visible, not when a spawner builds it, since every live material costs per-frame
//! CPU in wgpu's usage trackers whether drawn or not. The builder reserves a handle and parks the
//! value here; [`realize_bound`] inserts it once a bound entity is view-visible, and a handle every
//! holder dropped (`AssetEvent::Unused`) takes its parked value with it. A reader that clones a
//! material before binding it calls [`realize`] first.
//!
//! The deadline is the store's event publish, not extraction: `Assets::insert` only queues an
//! `AssetEvent::Added`, which `Assets::asset_events` publishes in `PostUpdate`, and an asset
//! realized after that is not drawn that frame (a material swap blinks). So the sweep runs at the
//! top of `PostUpdate`, before `AssetEventSystems` and before `VisibilityPropagate` resets the
//! bits, and reads last frame's visibility; a fresh binding draws on its second frame.
//!
//! The parked table is a process-wide mutex beside the store; main world only.

use std::collections::HashMap;
use std::sync::Mutex;

use bevy::asset::{Asset, AssetId};
use bevy::camera::visibility::ViewVisibility;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;

/// Built values of one asset type whose handles are reserved but not yet in the store.
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

    /// Makes the asset behind `id` exist now; `false` only for a foreign or fully dropped handle.
    pub fn realize(&mut self, store: &mut Assets<A>, id: AssetId<A>) -> bool {
        if store.contains(id) {
            return true;
        }
        let Some(asset) = self.0.remove(&id) else {
            return false;
        };
        // `Err`: the index was re-minted since parking and the `Unused` purge has not run yet.
        store.insert(id, asset).is_ok()
    }

    /// Insert every parked value.
    pub fn realize_all(&mut self, store: &mut Assets<A>) {
        for (id, asset) in self.0.drain() {
            let _ = store.insert(id, asset);
        }
    }

    /// Drops the value of a handle the store reported unused.
    pub fn purge(&mut self, id: AssetId<A>) {
        self.0.remove(&id);
    }

    /// The value behind `id`, realized or parked: a build-time stamp writes here, since
    /// `Assets::get_mut` reads a parked material as absent.
    pub fn value_mut<'a>(
        &'a mut self,
        store: &'a mut Assets<A>,
        id: AssetId<A>,
    ) -> Option<&'a mut A> {
        // Store first: once realized it is the truth, and a parked value for it would be stale.
        if store.contains(id) {
            return store.get_mut(id);
        }
        self.0.get_mut(&id)
    }

    /// Read-only [`Self::value_mut`], for a reader that must not mark the asset `Modified` (a
    /// bind-group rebuild on the non-bindless path).
    pub fn value<'a>(&'a self, store: &'a Assets<A>, id: AssetId<A>) -> Option<&'a A> {
        store.get(id).or_else(|| self.0.get(&id))
    }

    /// The deferral-aware `Assets::contains`: a registry that evicts on the store alone drops every
    /// parked material.
    pub fn holds(&self, store: &Assets<A>, id: AssetId<A>) -> bool {
        store.contains(id) || self.0.contains_key(&id)
    }

    /// Realizes every parked value a visible binding names; a binding with no visibility (`None`:
    /// a booth part before its camera) counts as visible.
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

/// The builder's `Assets::add`: reserves a handle and parks the value until a binding draws.
pub fn defer(
    materials: &Assets<WowModelMaterial>,
    material: WowModelMaterial,
) -> Handle<WowModelMaterial> {
    with_pending(|p| p.defer(materials, material))
}

/// Makes the asset behind `id` exist now ([`Parked::realize`]).
pub fn realize(materials: &mut Assets<WowModelMaterial>, id: AssetId<WowModelMaterial>) -> bool {
    with_pending(|p| p.realize(materials, id))
}

/// Inserts every parked value, for a lane that reads materials back right after building them.
pub fn realize_all(materials: &mut Assets<WowModelMaterial>) {
    with_pending(|p| p.realize_all(materials));
}

/// Applies `f` to a built material wherever it lives ([`Parked::value_mut`]), for a build-time
/// stamp that must not force it into the store; a closure, as the table is behind a mutex.
pub fn with_material_mut<R>(
    materials: &mut Assets<WowModelMaterial>,
    id: AssetId<WowModelMaterial>,
    f: impl FnOnce(&mut WowModelMaterial) -> R,
) -> Option<R> {
    with_pending(|p| p.value_mut(materials, id).map(f))
}

/// Reads a built material wherever it lives, without dirtying it ([`Parked::value`]).
pub fn with_material<R>(
    materials: &Assets<WowModelMaterial>,
    id: AssetId<WowModelMaterial>,
    f: impl FnOnce(&WowModelMaterial) -> R,
) -> Option<R> {
    with_pending(|p| p.value(materials, id).map(f))
}

/// The liveness test a registry keyed by material id evicts on ([`Parked::holds`]).
pub fn holds(materials: &Assets<WowModelMaterial>, id: AssetId<WowModelMaterial>) -> bool {
    // The store answers the common, realized case without taking the lock.
    materials.contains(id) || with_pending(|p| p.holds(materials, id))
}

/// How many values are parked (the census figure beside `mats=`).
pub fn pending_len() -> usize {
    with_pending(|p| p.len())
}

/// Top of `PostUpdate`, before `AssetEventSystems` and `VisibilityPropagate`: realizes the
/// material of every bound entity visible last frame and purges every handle reported unused.
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

    /// A parked value is a live material: writable in place and `holds`-alive before any draw.
    #[test]
    fn a_parked_value_is_writable_and_counts_as_held() {
        let mut store = Assets::<Stub>::default();
        let mut parked = Parked::default();
        let deferred = parked.defer(&store, Stub(1));
        assert!(!store.contains(deferred.id()));
        assert!(store.get_mut(deferred.id()).is_none());
        assert!(parked.holds(&store, deferred.id()));
        assert_eq!(parked.value(&store, deferred.id()).map(|s| s.0), Some(1));
        parked
            .value_mut(&mut store, deferred.id())
            .expect("the parked value is addressable")
            .0 = 7;
        // The stamp survives realization: the value goes into the store as written.
        assert!(parked.realize(&mut store, deferred.id()));
        assert_eq!(store.get(&deferred).map(|s| s.0), Some(7));
        assert_eq!(parked.value(&store, deferred.id()).map(|s| s.0), Some(7));
        parked
            .value_mut(&mut store, deferred.id())
            .expect("realized values stay addressable")
            .0 = 9;
        assert_eq!(store.get(&deferred).map(|s| s.0), Some(9));
        let stray = store.reserve_handle();
        assert!(!parked.holds(&store, stray.id()));
        assert!(parked.value(&store, stray.id()).is_none());
        assert!(parked.value_mut(&mut store, stray.id()).is_none());
    }

    /// The deadline is the publish, not the insert. The reader stands in for extraction, which
    /// runs right after `Last` and reads the same message buffer.
    #[test]
    fn the_sweep_publishes_its_added_in_the_frame_it_realizes() {
        use bevy::asset::{AssetEvent, AssetEventSystems, AssetPlugin};
        use bevy::camera::visibility::VisibilitySystems;

        #[derive(Resource)]
        struct Table(Parked<Stub>);
        #[derive(Resource)]
        struct Reserved(Handle<Stub>);
        /// Frames on which the extraction stand-in saw the `Added`.
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

        /// The frames extraction sees the `Added` on, with the sweep where `place` puts it.
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

        // Realized in `Last`: published by the next frame's `PostUpdate`.
        assert_eq!(
            frames_added_was_visible(|app| {
                app.add_systems(Last, realize_reserved.before(extraction_stand_in));
            }),
            vec![2],
            "a `Last` realize is announced a frame late — the blink"
        );

        // Realized at the top of `PostUpdate`: published the same frame.
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
