//! The straddle split: a translucent model crossing its water plane draws on both sides of the
//! water pass, each copy clipped at the waterline.
//!
//! The reference (`0x707680`, `M2UseClipPlanes` at its default 1, caps-gated at `0x7066a6`) dots
//! each instance's bound-box centre with its liquid plane `{0, 0, 1, −surfaceZ}` and keeps two
//! booleans, above `d ≥ −r` and below `d ≤ r`, `r` the world-scaled bound radius. Inside the band
//! a model and its depth primes (`0x707ffe`/`0x708048`) go on both lists, each copy under a clip
//! plane at the waterline (`0x70baf0`, `0x70c094`–`0x70c103`): the eye's far list draws before the
//! water, the near list after.
//!
//! The plane goes into the instance's rig-slot [`WaterClips`] word, which both the twin set
//! (`model_render::classify_water_side`, [`sync_straddle_twins`]) and the shader's
//! `WOW_WATER_CLIP` read, so they cannot disagree. Chained models (worn gear, spell kits) take
//! their body's word. Slot-0 content (map doodads, unskinned models) keeps one list per model by a
//! sign test at its origin; its only translucent moments are the distance-fade ring's small props.

use std::sync::Arc;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::renderer::RenderQueue;
use bevy::render::{Render, RenderApp, RenderSystems};

use benilla_assets::materials::WowModelMaterial;

use crate::mesh_tag::MAX_RIG_SLOTS;
use crate::model_fade::{ParentModel, MAX_MODEL_CHAIN};
use crate::model_render::FarSideTwins;
use crate::particles::WaterInterleave;
use crate::rig_palette::RigSkin;
use crate::world_unit::WorldUnit;

/// One slot's clip word, `[plane height, near side]`: the waterline's Bevy Y and the side the near
/// copy keeps, `+1` above for a dry eye, `−1` below for a submerged one (`0x4836d6`), 0 when not
/// straddling.
pub type ClipWord = [f32; 2];

/// The no-clip word; zero, so a zeroed region (a studio buffer's) is inert.
pub const NO_CLIP: ClipWord = [0.0, 0.0];

/// Bytes per slot: two `f32`, `wow_model.wgsl`'s `array<vec2<f32>, 2048>`.
const SLOT_BYTES: u64 = 8;

/// The clip region's offset in a `wow_light` buffer: after the mat-anim table, before the palette
/// rows, which stay last as the shader's one runtime-sized array.
pub(crate) fn region_offset() -> u64 {
    crate::mat_anim_table::region_offset() + crate::mat_anim_table::region_bytes()
}

/// Bytes this region adds to every `wow_light`-layout buffer (16 KB at 2048 slots).
pub(crate) fn region_bytes() -> u64 {
    MAX_RIG_SLOTS as u64 * SLOT_BYTES
}

/// The clip table by `MeshTag` rig slot; generation-stamped, so a dry world uploads nothing.
#[derive(Resource, Clone, ExtractResource)]
pub struct WaterClips {
    slots: Arc<Vec<ClipWord>>,
    generation: u64,
}

impl Default for WaterClips {
    fn default() -> Self {
        Self {
            slots: Arc::new(vec![NO_CLIP; MAX_RIG_SLOTS]),
            generation: 0,
        }
    }
}

impl WaterClips {
    /// Set a slot's word; slot 0, shared by everything unskinned, is never written.
    pub(crate) fn set(&mut self, slot: u16, word: ClipWord) {
        let i = slot as usize;
        if i == 0 || i >= MAX_RIG_SLOTS || self.slots[i] == word {
            return;
        }
        Arc::make_mut(&mut self.slots)[i] = word;
        self.generation += 1;
    }

    /// Back to not straddling; `RigSkin`'s free hook calls it, so a recycled slot starts dry.
    pub(crate) fn clear(&mut self, slot: u16) {
        self.set(slot, NO_CLIP);
    }

    /// A slot's word, [`NO_CLIP`] out of range.
    pub(crate) fn word(&self, slot: u16) -> ClipWord {
        self.slots.get(slot as usize).copied().unwrap_or(NO_CLIP)
    }

    /// Whether a slot's instance straddles, read from the word the fragment clips by.
    pub(crate) fn straddles(&self, slot: u16) -> bool {
        self.slots.get(slot as usize).is_some_and(|w| w[1] != 0.0)
    }
}

/// On an instance root: whether it straddles, a change edge that re-classifies its subtree.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelWaterBand {
    pub straddles: bool,
}

/// On a transparent batch of a straddling instance: it draws on both lists, with a far twin.
#[derive(Component)]
pub(crate) struct StraddlesWater;

/// On a straddling batch: its live far-side twin. [`sync_straddle_twins`] is the only writer.
#[derive(Component)]
pub(crate) struct StraddleTwin(Entity);

/// On a twin: the batch it doubles, which also keeps it out of the classifier's walk.
#[derive(Component)]
pub(crate) struct StraddleTwinOf(Entity);

/// The reference's side booleans for a bound centre `d` yd over its plane with slack `r`,
/// `(d ≥ −r, d ≤ r)` (`0x7079a2`–`0x707a17`), both ties inclusive (`0x7079e2`); NaN keeps neither.
pub(crate) fn side_booleans(d: f32, r: f32) -> (bool, bool) {
    (d >= -r, d <= r)
}

/// Exit hysteresis (yd) past the reference's `±r` band. Not a deviation: a split draw looks the
/// same as one list for a model wholly on one side, so where it flips changes cost, not pixels.
const STICKY_YD: f32 = 1.0;

/// The verdict: enter on the reference's band, leave [`STICKY_YD`] past it; `was` is the last one.
pub(crate) fn straddles(d: f32, r: f32, was: bool) -> bool {
    let slack = if was { r + STICKY_YD } else { r };
    side_booleans(d, slack) == (true, true)
}

/// The Rust twin of `wow_model.wgsl`'s `WOW_WATER_CLIP`: whether a fragment at height `y` survives
/// on this copy (`far_copy`: the `FAR_SIDE_MARKER` twin); the plane itself survives on both.
pub fn keeps(word: ClipWord, y: f32, far_copy: bool) -> bool {
    if word[1] == 0.0 {
        return true;
    }
    let side = if far_copy { -word[1] } else { word[1] };
    side * (y - word[0]) >= 0.0
}

/// A chained model's word, its nearest body's up the [`ParentModel`] chain; a broken chain, a
/// slotless body or one past [`MAX_MODEL_CHAIN`] reads not straddling.
fn wearer_word<K: Copy>(
    start: K,
    clips: &WaterClips,
    link: impl Fn(K) -> Option<(Option<u16>, Option<K>, bool)>,
) -> ClipWord {
    let mut at = start;
    for _ in 0..MAX_MODEL_CHAIN {
        let Some((slot, up, body)) = link(at) else {
            break;
        };
        if body {
            return slot.map_or(NO_CLIP, |s| clips.word(s));
        }
        match up {
            Some(p) => at = p,
            None => break,
        }
    }
    NO_CLIP
}

/// The roots re-banded on an ordinary frame: those whose matrix, bound, slot or room claim moved.
type BandDirty = (
    With<RigSkin>,
    With<WorldUnit>,
    Or<(
        Changed<GlobalTransform>,
        Changed<WorldUnit>,
        Changed<RigSkin>,
        Changed<crate::wmo_portal::UnitWmoRoom>,
    )>,
);

/// A model chained to a body (worn gear, a hung spell kit), with its own slot.
type ChainedRoot<'a> = (
    Entity,
    &'a RigSkin,
    &'a ParentModel,
    Option<Mut<'a, ModelWaterBand>>,
);

/// A chained model whose slot or link is new this frame, so it has no word yet.
type ChainedFresh = (
    With<ParentModel>,
    Without<WorldUnit>,
    Or<(Changed<RigSkin>, Changed<ParentModel>)>,
);

/// One model-instance root as [`band_instances`] sees it.
type BandRoot<'a> = (
    Entity,
    &'a GlobalTransform,
    &'a WorldUnit,
    &'a RigSkin,
    Option<Mut<'a, ModelWaterBand>>,
);

/// Band every instance against its water plane into its [`WaterClips`] word and
/// [`ModelWaterBand`]; a chained model, on a slot of its own, takes its body's word verbatim. The
/// sync point before `classify_water_side` lands a new band before it looks.
pub(crate) fn band_instances(
    interleave: WaterInterleave,
    mut clips: ResMut<WaterClips>,
    mut commands: Commands,
    mut roots: Query<(
        Entity,
        &GlobalTransform,
        &WorldUnit,
        &RigSkin,
        Option<&mut ModelWaterBand>,
    )>,
    dirty: Query<Entity, BandDirty>,
    mut chained: Query<
        (Entity, &RigSkin, &ParentModel, Option<&mut ModelWaterBand>),
        Without<WorldUnit>,
    >,
    fresh: Query<Entity, ChainedFresh>,
    links: Query<(Option<&RigSkin>, Option<&ParentModel>, Has<WorldUnit>)>,
    mut eye_was_submerged: Local<Option<bool>>,
) {
    let before = clips.generation;
    let eye = interleave.eye_submerged();
    let full = *eye_was_submerged != Some(eye) || interleave.surfaces_changed();
    *eye_was_submerged = Some(eye);
    // A dry eye's near list is the above one; a submerged eye's, the below one (`0x4836d6`).
    let near_side = if eye { -1.0 } else { 1.0 };
    if full {
        for root in &mut roots {
            band_one(&interleave, &mut clips, &mut commands, near_side, root);
        }
    } else {
        for e in &dirty {
            if let Ok(root) = roots.get_mut(e) {
                band_one(&interleave, &mut clips, &mut commands, near_side, root);
            }
        }
    }
    // The chained pass: every chain when a body's word moved (the generation did), else new ones.
    let link = |e: Entity| {
        links
            .get(e)
            .ok()
            .map(|(skin, up, body)| (skin.map(|s| s.slot), up.map(|p| p.0), body))
    };
    if clips.generation != before {
        for item in &mut chained {
            chain_one(&mut clips, &mut commands, &link, item);
        }
    } else {
        for e in &fresh {
            if let Ok(item) = chained.get_mut(e) {
                chain_one(&mut clips, &mut commands, &link, item);
            }
        }
    }
}

/// Copy the wearer's word onto a chained model's slot and publish its band edge.
fn chain_one(
    clips: &mut WaterClips,
    commands: &mut Commands,
    link: &impl Fn(Entity) -> Option<(Option<u16>, Option<Entity>, bool)>,
    (entity, rig, parent, band): ChainedRoot<'_>,
) {
    let word = wearer_word(parent.0, clips, link);
    clips.set(rig.slot, word);
    band_edge(commands, entity, band, word[1] != 0.0);
}

/// Publish the verdict as [`ModelWaterBand`], change-gated, inserted when it first straddles.
fn band_edge(
    commands: &mut Commands,
    entity: Entity,
    band: Option<Mut<ModelWaterBand>>,
    straddles: bool,
) {
    match band {
        Some(mut b) => {
            b.set_if_neq(ModelWaterBand { straddles });
        }
        None if straddles => {
            commands.entity(entity).insert(ModelWaterBand { straddles });
        }
        None => {}
    }
}

/// One instance's band, for both of [`band_instances`]' paths.
fn band_one(
    interleave: &WaterInterleave,
    clips: &mut WaterClips,
    commands: &mut Commands,
    near_side: f32,
    (entity, gt, unit, rig, band): BandRoot<'_>,
) {
    // The reference's point and slack: the bound centre through the instance matrix (`0x70848d`),
    // the radius by its row-0 scale (`0x708478`). The bound here is the armed idle's CAaBox, so the
    // slack is its circumscribed sphere; a wider band costs only a copy the clip empties.
    let was = band.as_ref().is_some_and(|b| b.straddles);
    let word = unit.bound.and_then(|b| {
        let centre = gt.transform_point(Vec3::from(b.center));
        let r = gt.affine().matrix3.x_axis.length() * Vec3::from(b.half_extents).length();
        let d = crate::particles::water_height(interleave, Some(entity), centre)?;
        straddles(d, r, was).then_some([centre.y - d, near_side])
    });
    clips.set(rig.slot, word.unwrap_or(NO_CLIP));
    band_edge(commands, entity, band, word.is_some());
}

/// A straddle candidate: the batch, what its twin copies, its verdict and its twin.
type StraddleParts<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static MeshTag,
        &'static Mesh3d,
        &'static MeshMaterial3d<WowModelMaterial>,
        Has<StraddlesWater>,
        Option<&'static StraddleTwin>,
        Option<&'static Aabb>,
        Has<NoFrustumCulling>,
        Option<&'static RenderLayers>,
    ),
    (
        Without<StraddleTwinOf>,
        Or<(With<StraddlesWater>, With<StraddleTwin>)>,
    ),
>;

/// Keep a far twin under each [`StraddlesWater`] batch, mirroring its tag, mesh and handle; after
/// the depth-prime sync, so a zfill twin's own twin mirrors its fresh tag.
pub(crate) fn sync_straddle_twins(
    mut commands: Commands,
    far: Res<FarSideTwins>,
    parts: StraddleParts,
    mut twins: Query<(
        &StraddleTwinOf,
        &mut MeshTag,
        &mut Mesh3d,
        &mut MeshMaterial3d<WowModelMaterial>,
    )>,
) {
    for (part, tag, mesh, mat, straddling, twin, aabb, no_cull, layers) in &parts {
        match twin {
            None if straddling => {
                // A miss: the handle moved since the twin was built; next frame rebuilds it.
                let Some(far_h) = far.far_of(&mat.0) else {
                    continue;
                };
                let mut t = commands.spawn((
                    Mesh3d(mesh.0.clone()),
                    MeshMaterial3d(far_h.clone()),
                    MeshTag(tag.0),
                    Transform::IDENTITY,
                    StraddleTwinOf(part),
                    ChildOf(part),
                ));
                // The twin culls exactly as its batch does.
                if let Some(aabb) = aabb {
                    t.insert(*aabb);
                }
                if no_cull {
                    t.insert(NoFrustumCulling);
                }
                if let Some(layers) = layers {
                    t.insert(layers.clone());
                }
                let t = t.id();
                commands.entity(part).insert(StraddleTwin(t));
                trace("arm", part);
            }
            Some(t) if !straddling => {
                commands.entity(t.0).try_despawn();
                commands.entity(part).remove::<StraddleTwin>();
                trace("release", part);
            }
            _ => {}
        }
    }
    for (of, mut tag, mut mesh, mut mat) in &mut twins {
        let Ok((_, ptag, pmesh, pmat, ..)) = parts.get(of.0) else {
            continue; // the batch is going away, and the child with it
        };
        if tag.0 != ptag.0 {
            tag.0 = ptag.0;
        }
        if mesh.0 != pmesh.0 {
            mesh.0 = pmesh.0.clone();
        }
        if let Some(far_h) = far.far_of(&pmat.0) {
            if mat.0 != *far_h {
                mat.0 = far_h.clone();
            }
        }
    }
}

/// `WOW_MOVE_TRACE=<path>`, tag `fx`: one line per twin armed or released.
fn trace(what: &str, part: Entity) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    benilla_assets::trace::line("fx", &format!("straddle-twin {what} part={part}"));
}

/// Render world: write the whole region to the shared buffer on a change; studio buffers stay zero.
fn upload_water_clips(
    queue: Res<RenderQueue>,
    shared: Option<Res<crate::lighting::SharedLightBuffer>>,
    clips: Option<Res<WaterClips>>,
    mut last: Local<Option<u64>>,
) {
    let (Some(shared), Some(clips)) = (shared, clips) else {
        return;
    };
    if *last == Some(clips.generation) {
        return;
    }
    *last = Some(clips.generation);
    queue.write_buffer(
        &shared.0,
        region_offset(),
        bytemuck::cast_slice(clips.slots.as_slice()),
    );
}

/// The straddle split's registration.
pub fn plugin(app: &mut App) {
    app.init_resource::<WaterClips>()
        .add_plugins(ExtractResourcePlugin::<WaterClips>::default())
        .add_systems(
            Update,
            band_instances
                .after(crate::liquid::SubmersionVerdict)
                .before(crate::model_render::classify_water_side),
        )
        .add_systems(
            PostUpdate,
            sync_straddle_twins.after(crate::zfill::sync_zfill_twins),
        );
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(
            Render,
            upload_water_clips.in_set(RenderSystems::PrepareResources),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_band_is_plus_minus_r_inclusive() {
        assert_eq!(side_booleans(0.4, 1.5), (true, true));
        assert_eq!(side_booleans(-1.2, 1.5), (true, true));
        assert_eq!(side_booleans(-1.5, 1.5), (true, true), "d == −r keeps A");
        assert_eq!(side_booleans(1.5, 1.5), (true, true), "d == r keeps B");
        assert_eq!(side_booleans(1.6, 1.5), (true, false), "wholly above");
        assert_eq!(side_booleans(-1.6, 1.5), (false, true), "wholly below");
        // No slack is the fallback's sign test, and only the plane itself straddles it.
        assert_eq!(side_booleans(0.0, 0.0), (true, true));
        assert_eq!(side_booleans(-0.1, 0.0), (false, true));
        assert_eq!(
            side_booleans(f32::NAN, 1.0),
            (false, false),
            "NaN never splits"
        );
    }

    #[test]
    fn the_exit_is_sticky_and_the_entry_is_not() {
        let r = 1.2;
        assert!(!straddles(-1.5, r, false), "outside the band: no entry");
        assert!(straddles(-1.2, r, false), "the band's own edge enters");
        assert!(
            straddles(-1.5, r, true),
            "a straddler bobbing past the edge holds"
        );
        assert!(straddles(1.5, r, true), "…on either side");
        assert!(
            straddles(-2.2, r, true),
            "the sticky edge itself still holds"
        );
        assert!(!straddles(-2.3, r, true), "past the sticky edge it leaves");
        assert!(!straddles(f32::NAN, r, true), "NaN never splits");
    }

    #[test]
    fn each_copy_keeps_its_own_half() {
        let dry = [10.0, 1.0];
        assert!(keeps(dry, 11.0, false), "the head, on the near copy");
        assert!(!keeps(dry, 11.0, true), "…and not again on the far twin");
        assert!(keeps(dry, 9.0, true), "the legs, on the far twin");
        assert!(
            !keeps(dry, 9.0, false),
            "…and not over the water on the near copy"
        );
        assert!(
            keeps(dry, 10.0, false) && keeps(dry, 10.0, true),
            "the waterline itself"
        );
        let submerged = [10.0, -1.0];
        assert!(
            keeps(submerged, 9.0, false),
            "under water, the near half is the lower one"
        );
        assert!(keeps(submerged, 11.0, true));
        assert!(!keeps(submerged, 11.0, false));
        // Not straddling: nothing is clipped on either copy.
        for y in [-100.0, 0.0, 100.0] {
            assert!(keeps(NO_CLIP, y, false) && keeps(NO_CLIP, y, true));
        }
    }

    #[test]
    fn a_chained_model_inherits_its_bodys_waterline() {
        let mut clips = WaterClips::default();
        clips.set(5, [57.0, 1.0]);
        // key → (own slot, parent, is a body)
        let chain = |k: u32| match k {
            1 => Some((Some(9), Some(2), false)), // weapon glow → weapon
            2 => Some((Some(8), Some(3), false)), // weapon (rider slot 8) → unit
            3 => Some((Some(5), None, true)),     // the unit, straddling on slot 5
            4 => Some((None, None, true)),        // a body with no rig slot
            6 => Some((Some(7), Some(6), false)), // a self-loop: bounded, never a body
            _ => None,
        };
        assert_eq!(
            wearer_word(2, &clips, chain),
            [57.0, 1.0],
            "the weapon's parent"
        );
        assert_eq!(wearer_word(3, &clips, chain), [57.0, 1.0]);
        assert_eq!(wearer_word(1, &clips, chain), [57.0, 1.0], "two links up");
        assert_eq!(wearer_word(4, &clips, chain), NO_CLIP, "a slotless body");
        assert_eq!(
            wearer_word(6, &clips, chain),
            NO_CLIP,
            "a cycle ends bounded"
        );
        assert_eq!(wearer_word(99, &clips, chain), NO_CLIP, "a broken chain");
    }

    #[test]
    fn slot_zero_is_never_written_and_only_real_changes_upload() {
        let mut c = WaterClips::default();
        c.set(0, [1.0, 1.0]);
        assert!(!c.straddles(0));
        assert_eq!(c.generation, 0);
        c.set(7, [12.5, 1.0]);
        assert!(c.straddles(7));
        assert_eq!(c.generation, 1);
        c.set(7, [12.5, 1.0]);
        assert_eq!(c.generation, 1, "same word, no upload");
        c.clear(7);
        assert!(!c.straddles(7));
        assert_eq!(c.generation, 2);
        c.set(u16::MAX, [1.0, 1.0]);
        assert_eq!(c.generation, 2, "an out-of-range slot is dropped");
    }

    /// The shader's `array<vec2<f32>, 2048>` sits between two `vec4` arrays: it ends on 16 bytes.
    #[test]
    fn the_region_is_one_vec2_per_slot_on_a_vec4_boundary() {
        assert_eq!(region_bytes(), (MAX_RIG_SLOTS * 8) as u64);
        assert_eq!(region_offset() % 16, 0);
        assert_eq!((region_offset() + region_bytes()) % 16, 0);
    }
}
