//! Paced model render-form building. The M2 and WMO loaders ship geometry, not meshes: a loader's
//! meshes would reach the render world a whole model in one frame. Consumers `require()` the
//! forms they need, and [`furnish_model_forms`] builds a bounded amount per frame while live,
//! nearest requester first, uncapped behind the loading cover. One [`Entry`] per model asset
//! serves every instance.

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

/// Per-frame build budget while live, in submeshes: it bounds the per-mesh fixed cost, so a city
/// crossing's ~3000 small meshes land in about 0.5 s, inside the 5×5 window's two-tile fog margin.
const MESH_BUDGET: usize = 96;
/// Per-frame build budget while live, in vertices, for the huge WMO group batches; a submesh over
/// it builds alone in its frame.
const VERT_BUDGET: usize = 16_384;

/// A loaded model asset, M2 or WMO, as the forms cache keys it.
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

/// One model's built forms; `stat.len()` and `skin.len()` are the build cursors across frames.
#[derive(Default)]
struct Entry {
    /// The static form per submesh with its build-time `Aabb` (`None`: degenerate), which
    /// consumers insert themselves: `calculate_bounds` races a `RENDER_WORLD` mesh, and the
    /// exterior cull fails open without a bound.
    stat: Vec<(Handle<Mesh>, Option<Aabb>)>,
    /// The skinned twin per submesh, for rigged lanes only; it keeps main-world data because the
    /// mouseover picker skins it on the CPU (`target::hover`).
    skin: Vec<Handle<Mesh>>,
    /// Kinds ever requested (`WANT_*` bits), sticky until the asset frees.
    want: u8,
    /// Kinds fully built.
    done: u8,
    /// This frame's most urgent requester (lower is sooner), reset after each furnish pass.
    priority: i32,
}

/// The per-asset render-form cache; an entry drops when its model asset leaves the store.
#[derive(Resource, Default)]
pub struct ModelForms {
    entries: HashMap<ModelKey, Entry>,
}

/// One model's built forms for the assembler, index-parallel with its submeshes.
#[derive(Clone, Copy)]
pub struct FormSlices<'a> {
    pub stat: &'a [(Handle<Mesh>, Option<Aabb>)],
    pub skin: Option<&'a [Handle<Mesh>]>,
}

impl ModelForms {
    /// Record that a consumer needs `kinds` at `priority` (the requester's tile distance, lower is
    /// sooner); `true` once every requested kind is built.
    pub(crate) fn require(&mut self, key: ModelKey, kinds: u8, priority: i32) -> bool {
        let e = self.entries.entry(key).or_default();
        e.want |= kinds;
        e.priority = e.priority.min(priority);
        e.done & kinds == kinds
    }

    /// The built static forms, or `None` until built.
    pub(crate) fn static_meshes(&self, key: ModelKey) -> Option<&[(Handle<Mesh>, Option<Aabb>)]> {
        let e = self.entries.get(&key)?;
        (e.done & WANT_STATIC != 0).then_some(e.stat.as_slice())
    }

    /// The built skinned twins, or `None` until built.
    pub(crate) fn skinned_meshes(&self, key: ModelKey) -> Option<&[Handle<Mesh>]> {
        let e = self.entries.get(&key)?;
        (e.done & WANT_SKINNED != 0).then_some(e.skin.as_slice())
    }

    /// Request both forms, `true` once built: the gate every lane whose instances can rig polls.
    pub fn require_rigged(&mut self, model: impl Into<ModelKey>, priority: i32) -> bool {
        self.require(model.into(), WANT_STATIC | WANT_SKINNED, priority)
    }

    /// Request the static form only, for lanes whose instances never skin.
    pub fn require_static(&mut self, model: impl Into<ModelKey>, priority: i32) -> bool {
        self.require(model.into(), WANT_STATIC, priority)
    }

    /// One model's built forms, empty until the matching `require_*` returns `true`.
    pub fn slices(&self, model: impl Into<ModelKey>) -> FormSlices<'_> {
        let key = model.into();
        FormSlices {
            stat: self.static_meshes(key).unwrap_or(&[]),
            skin: self.skinned_meshes(key),
        }
    }

    /// [`Self::ensure_now`] for both forms.
    pub fn ensure_now_rigged(
        &mut self,
        model: impl Into<ModelKey>,
        submeshes: &[benilla_assets::ModelSubmesh],
        meshes: &mut Assets<Mesh>,
    ) {
        self.ensure_now(model.into(), WANT_STATIC | WANT_SKINNED, submeshes, meshes);
    }

    /// [`Self::ensure_now`] for the static form only.
    pub(crate) fn ensure_now_static(
        &mut self,
        model: impl Into<ModelKey>,
        submeshes: &[benilla_assets::ModelSubmesh],
        meshes: &mut Assets<Mesh>,
    ) {
        self.ensure_now(model.into(), WANT_STATIC, submeshes, meshes);
    }

    /// Build a model's missing forms now, uncapped, for one-off lanes (booths, glue, markers);
    /// streaming lanes go through [`Self::require`] and the paced furnisher.
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

    /// Drop everything on leaving the world, where the gated furnisher no longer reads `Unused`.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Advance one entry's builds within `budget` submeshes and [`VERT_BUDGET`]; returns the count.
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

/// Build requested forms most urgent first, [`MESH_BUDGET`] and [`VERT_BUDGET`] per frame while
/// live, uncapped behind the loading cover; at least one submesh always builds.
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
    // An asset leaving the store or reloading drops its forms; `Unused` fires once for any asset.
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
        // An asset still decoding or already dropped skips: it builds on landing or frees above.
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
    /// The measured Stormwind crossing burst, ~3200 meshes, lands inside the fog margin at 60 Hz.
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
