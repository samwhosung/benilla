//! Paced model render-form building — the model-lane twin of the tile furnisher (
//! closing 0832's named residual).
//!
//! The M2/WMO loaders ship **geometry, no meshes** (`ModelSubmesh::geometry`): a loader-built
//! labeled sub-asset lands the instant its decode completes, and the render world ingests the
//! whole model — a city root's thousands of group batches, a fresh row's hundreds of doodads —
//! in ONE frame. Measured at the Stormwind line (pre): 2000–3200 mesh assets per crossing,
//! 44–119 ms wall on the worst frame, on every crossing including re-entries. No budget
//! downstream of a load can spread work the loader has already packaged, so the build lives
//! here: consumers `require()` the forms they need, and [`furnish_model_forms`] builds a bounded
//! amount per frame while live — nearest requester first — uncapped behind the loading cover.
//!
//! One [`Entry`] per loaded model asset holds the built handles; every instance of the model
//! shares them (the build runs once per asset, not per placement). The **static** form is
//! `RENDER_WORLD`-only with its `Aabb` computed at build time (the exterior cull fails open on a
//! missing bound — 0832's rule); the **skinned** twin keeps main-world data because the mouseover
//! picker CPU-skins its vertices (`target::hover`), and it is built only for lanes that rig —
//! which retires the loader's old always-built, mostly-unused twin (the 0019 deferral).

use std::collections::HashMap;
use std::time::Instant;

use benilla_assets::{
    submesh_to_skinned_mesh, submesh_to_static_mesh, M2Model, ModelSubmesh, WmoModel,
};
use bevy::asset::AssetId;
use bevy::camera::primitives::{Aabb, MeshAabb};
use bevy::prelude::*;

/// Request bit: the static (unskinned) render form.
pub(crate) const WANT_STATIC: u8 = 1;
/// Request bit: the skinned twin (rigged lanes only).
pub(crate) const WANT_SKINNED: u8 = 2;

/// Per-frame build budget while live, in submeshes. The companion vertex budget below is the
/// real governor for big WMO group batches; this cap bounds the per-mesh fixed cost (asset add +
/// extract + prepare) the Stormwind leg measured dominating at ~3000 small doodad meshes per
/// crossing. At 96/frame a whole city crossing's ~3000 meshes land in ~0.5 s of frames — inside
/// the two-tile fog margin the 5×5 window gives a first contact.
const MESH_BUDGET: usize = 96;
/// Per-frame build budget while live, in vertices — the governor for the handful of huge group
/// batches (tens of thousands of vertices each). A single submesh may exceed it alone; it then
/// lands alone in its frame (a submesh is the atomic unit — at least one always builds, so
/// progress is guaranteed).
const VERT_BUDGET: usize = 16_384;

/// A loaded model asset the forms cache keys by — the two model kinds share one cache because
/// every consumer lane (placements, entities, markers, booths) handles both.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ModelKey {
    M2(AssetId<M2Model>),
    Wmo(AssetId<WmoModel>),
}

impl From<&Handle<M2Model>> for ModelKey {
    fn from(h: &Handle<M2Model>) -> Self {
        Self::M2(h.id())
    }
}
impl From<&Handle<WmoModel>> for ModelKey {
    fn from(h: &Handle<WmoModel>) -> Self {
        Self::Wmo(h.id())
    }
}

/// One model's built render forms + its build cursors (`stat.len()` / `skin.len()` — resumed
/// across frames, like a tile's cell cursor).
#[derive(Default)]
struct Entry {
    /// The static form per submesh, with the `Aabb` computed at build (`None` = degenerate
    /// geometry): consumers insert it explicitly — `RENDER_WORLD` meshes race Bevy's
    /// `calculate_bounds`, and the exterior cull fails open on a missing bound.
    stat: Vec<(Handle<Mesh>, Option<Aabb>)>,
    /// The skinned twin per submesh (rigged lanes only; main-world data kept for the picker).
    skin: Vec<Handle<Mesh>>,
    /// Kinds ever requested (`WANT_*` bits). Sticky: demand for a form doesn't vanish because
    /// one requester despawned mid-build — the entry frees with the asset.
    want: u8,
    /// Kinds fully built.
    done: u8,
    /// This frame's most urgent requester (lower = sooner), reset after each furnish pass so a
    /// vanished requester stops hoisting its model.
    priority: i32,
}

/// The per-asset render-form cache. Entries are created by [`Self::require`], built by
/// [`furnish_model_forms`], and dropped when the model asset itself leaves the store (the mesh
/// handles drop with them; instances that already spawned keep theirs via `Mesh3d`).
#[derive(Resource, Default)]
pub struct ModelForms {
    entries: HashMap<ModelKey, Entry>,
}

/// A borrowed view of one model's built forms for the assembler, index-parallel with the model's
/// submeshes: the static handle + build-time `Aabb` per batch, plus the skinned twins when the
/// lane rigs.
#[derive(Clone, Copy)]
pub struct FormSlices<'a> {
    pub stat: &'a [(Handle<Mesh>, Option<Aabb>)],
    pub skin: Option<&'a [Handle<Mesh>]>,
}

impl ModelForms {
    /// Record that a consumer needs `kinds` of this model's forms, at `priority` (lower =
    /// sooner; pass the requester's tile/chunk distance). Returns `true` when every requested
    /// kind is already built — the consumer's gate, polled the same way it polls the model
    /// asset itself.
    pub(crate) fn require(&mut self, key: ModelKey, kinds: u8, priority: i32) -> bool {
        let e = self.entries.entry(key).or_default();
        e.want |= kinds;
        e.priority = e.priority.min(priority);
        e.done & kinds == kinds
    }

    /// The built static forms (handle + build-time `Aabb` per submesh), or `None` until
    /// [`Self::require`]`(…, WANT_STATIC, …)` has returned `true`.
    pub(crate) fn static_meshes(&self, key: ModelKey) -> Option<&[(Handle<Mesh>, Option<Aabb>)]> {
        let e = self.entries.get(&key)?;
        (e.done & WANT_STATIC != 0).then_some(e.stat.as_slice())
    }

    /// The built skinned twins, or `None` until built.
    pub(crate) fn skinned_meshes(&self, key: ModelKey) -> Option<&[Handle<Mesh>]> {
        let e = self.entries.get(&key)?;
        (e.done & WANT_SKINNED != 0).then_some(e.skin.as_slice())
    }

    /// Request **both** forms of a model and report whether they are built — the gate every lane
    /// whose instances can rig polls each frame (creatures, players, held items, spell effects,
    /// animated GameObjects, the booths). Takes the model handle rather than a key: the key is
    /// the cache's business, not the caller's.
    pub fn require_rigged(&mut self, model: impl Into<ModelKey>, priority: i32) -> bool {
        self.require(model.into(), WANT_STATIC | WANT_SKINNED, priority)
    }

    /// Request the **static** form only — the lanes whose instances never skin: WMO displays,
    /// billboard cards, the particle-model quad source.
    pub fn require_static(&mut self, model: impl Into<ModelKey>, priority: i32) -> bool {
        self.require(model.into(), WANT_STATIC, priority)
    }

    /// One model's built forms, index-parallel with its submeshes. Empty slices until the
    /// matching `require_*` has returned `true` — a caller that spawns anyway gets default (dead)
    /// handles rather than a panic, which is the same contract the assembler already documents.
    pub fn slices(&self, model: impl Into<ModelKey>) -> FormSlices<'_> {
        let key = model.into();
        FormSlices {
            stat: self.static_meshes(key).unwrap_or(&[]),
            skin: self.skinned_meshes(key),
        }
    }

    /// [`Self::ensure_now`] for **both** forms — the booth/glue/marker lanes' rigged models.
    pub fn ensure_now_rigged(
        &mut self,
        model: impl Into<ModelKey>,
        submeshes: &[benilla_assets::ModelSubmesh],
        meshes: &mut Assets<Mesh>,
    ) {
        self.ensure_now(model.into(), WANT_STATIC | WANT_SKINNED, submeshes, meshes);
    }

    /// [`Self::ensure_now`] for the **static** form only.
    pub(crate) fn ensure_now_static(
        &mut self,
        model: impl Into<ModelKey>,
        submeshes: &[benilla_assets::ModelSubmesh],
        meshes: &mut Assets<Mesh>,
    ) {
        self.ensure_now(model.into(), WANT_STATIC, submeshes, meshes);
    }

    /// Build every requested-and-missing form of one model NOW, uncapped — the booth/glue/marker
    /// lanes: one small model at a time, usually behind a screen whose job is to absorb exactly
    /// this. The streaming lanes must go through [`Self::require`] + the paced furnisher instead.
    pub(crate) fn ensure_now(
        &mut self,
        key: ModelKey,
        kinds: u8,
        submeshes: &[ModelSubmesh],
        meshes: &mut Assets<Mesh>,
    ) {
        let e = self.entries.entry(key).or_default();
        e.want |= kinds;
        build_entry(e, kinds, submeshes, meshes, usize::MAX, &mut 0);
    }

    /// Drop one model's entry (its asset left the store).
    fn forget(&mut self, key: ModelKey) {
        self.entries.remove(&key);
    }

    /// Drop everything — leaving the world (`release_world`), where the per-asset `Unused`
    /// events can no longer reach the world-live-gated furnisher.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Advance one entry's requested builds, bounded by `budget` submeshes and [`VERT_BUDGET`]
/// vertices (via `verts`, shared across entries within a frame). Returns how many submeshes were
/// built. Marks a kind done when its cursor has walked every submesh.
fn build_entry(
    e: &mut Entry,
    kinds: u8,
    submeshes: &[ModelSubmesh],
    meshes: &mut Assets<Mesh>,
    budget: usize,
    verts: &mut usize,
) -> usize {
    let mut built = 0usize;
    if kinds & WANT_STATIC != 0 && e.done & WANT_STATIC == 0 {
        while e.stat.len() < submeshes.len() {
            if built >= budget || *verts >= VERT_BUDGET {
                return built;
            }
            let geo = &submeshes[e.stat.len()].geometry;
            let mesh = submesh_to_static_mesh(geo);
            let aabb = mesh.compute_aabb();
            *verts += geo.positions.len();
            e.stat.push((meshes.add(mesh), aabb));
            built += 1;
        }
        e.done |= WANT_STATIC;
    }
    if kinds & WANT_SKINNED != 0 && e.done & WANT_SKINNED == 0 {
        while e.skin.len() < submeshes.len() {
            if built >= budget || *verts >= VERT_BUDGET {
                return built;
            }
            let geo = &submeshes[e.skin.len()].geometry;
            *verts += geo.positions.len();
            e.skin.push(meshes.add(submesh_to_skinned_mesh(geo)));
            built += 1;
        }
        e.done |= WANT_SKINNED;
    }
    built
}

/// Build requested model forms, [`MESH_BUDGET`]/[`VERT_BUDGET`] per frame while live — most
/// urgent requester first — uncapped behind the loading cover (entry/teleport/world-stale), which
/// exists to absorb exactly that burst. At least one submesh always builds, so progress is
/// guaranteed. Frees an entry when its model asset leaves the store.
pub(crate) fn furnish_model_forms(
    mut forms: ResMut<ModelForms>,
    m2s: Res<Assets<M2Model>>,
    wmos: Res<Assets<WmoModel>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut activity: ResMut<crate::terrain_stream::StreamActivity>,
    focus: Res<crate::terrain_stream::ViewFocus>,
    mut m2_events: MessageReader<AssetEvent<M2Model>>,
    mut wmo_events: MessageReader<AssetEvent<WmoModel>>,
) {
    let t0 = Instant::now();
    // The free path: a model asset leaving the store (or replaced by a reload) drops its forms —
    // the mesh handles' last cache-side strong refs go with it, so the GPU copies free once the
    // last spawned instance is gone too. `Unused` fires for untracked (`RENDER_WORLD`-extracted)
    // and tracked assets alike, exactly once (0832's counter rule).
    for ev in m2_events.read() {
        if let AssetEvent::Unused { id }
        | AssetEvent::Removed { id }
        | AssetEvent::Modified { id } = ev
        {
            forms.forget(ModelKey::M2(*id));
        }
    }
    for ev in wmo_events.read() {
        if let AssetEvent::Unused { id }
        | AssetEvent::Removed { id }
        | AssetEvent::Modified { id } = ev
        {
            forms.forget(ModelKey::Wmo(*id));
        }
    }

    let cap = if focus.paced { MESH_BUDGET } else { usize::MAX };

    // The frame's work list: entries with a requested kind still unbuilt, most urgent first.
    let mut pending: Vec<(i32, ModelKey)> = forms
        .entries
        .iter()
        .filter(|(_, e)| e.done & e.want != e.want)
        .map(|(&k, e)| (e.priority, k))
        .collect();
    if pending.is_empty() {
        return;
    }
    pending.sort_unstable_by_key(|(p, _)| *p);

    let mut built = 0usize;
    let mut verts = 0usize;
    for (_, key) in pending {
        if built >= cap || verts >= VERT_BUDGET {
            break;
        }
        // The asset can lag its request (still decoding) or predecease it (dropped mid-build);
        // both just skip — the entry builds when the asset lands, or frees on its event above.
        let submeshes: &[ModelSubmesh] = match key {
            ModelKey::M2(id) => match m2s.get(id) {
                Some(m) => &m.submeshes,
                None => continue,
            },
            ModelKey::Wmo(id) => match wmos.get(id) {
                Some(m) => &m.submeshes,
                None => continue,
            },
        };
        let Some(e) = forms.entries.get_mut(&key) else {
            continue;
        };
        let want = e.want;
        built += build_entry(e, want, submeshes, &mut meshes, cap - built, &mut verts);
        e.priority = i32::MAX; // re-asserted by next frame's require() calls
    }
    activity.model_meshes_built += built as u32;
    activity.mfurnish_ms += t0.elapsed().as_secs_f32() * 1000.0;
}

#[cfg(test)]
mod tests {
    /// The cap's arithmetic, kept honest in one place (the furnish.rs test's model twin): the
    /// Stormwind crossing's measured ~3200 mesh burst must land in well under a second of frames
    /// at 60 Hz — inside the fog margin a 5×5-window first contact gives.
    #[test]
    fn a_city_crossing_furnishes_in_under_a_second() {
        let crossing_meshes: usize = 3200;
        let frames = crossing_meshes.div_ceil(super::MESH_BUDGET);
        assert!(
            frames <= 40,
            "a city crossing's models must land within ~2/3 s at 60 Hz, got {frames} frames"
        );
    }
}
