//! **The straddle split** — a translucent model that crosses its water plane draws on BOTH sides
//! of the water pass, each copy cut at the waterline.
//!
//! **The reference.** The collector (`0x707680`) dots each model's bound-box centre (instance
//! matrix × `(min+max)/2`) against the plane `{0,0,1,−surfaceZ}` from the model's own liquid hit,
//! once per instance. With `M2UseClipPlanes` granted — the cvar's default `"1"`, caps-gated at
//! `0x7066a6` — it keeps TWO side booleans, `A = d ≥ −r` (the above list) and `B = d ≤ r` (the
//! below list), `r` the world-scaled bound radius. A model inside the `±r` band lands on BOTH
//! lists, its z-fill depth primes ride the same booleans (`0x707ffe`/`0x708048`), and the mesh
//! draw arm brackets each list's copy with a hardware clip plane at the waterline (`0x70baf0`
//! @`0x70c094`–`0x70c103`). The frame interleave draws the eye's far list before the water and the
//! near list after it — so the half under the surface is painted over by the water, and the
//! half above it paints over the water.
//!
//! **What benilla had.** The mesh lane shipped the no-clip-planes fallback only — one list
//! per model, a bare sign test at the placement origin — and named the band as a gap. Its worst
//! case was a unit fading in while standing in water (the director's report, 2026-09-11): the
//! appear ramp puts every part on its blend twin, which is exactly when the lane classifies it;
//! feet under the surface sent the WHOLE body to the far list; and because the fade twin and the
//! depth prime both write depth, the part above the water primed its depth before the water drew.
//! A body-shaped hole in the surface, with the head blended over the unwatered lake bed behind it
//! — strongest at the ramp's start, where the body is least opaque.
//!
//! **The shape here — three pieces, one verdict:**
//! - [`band_instances`] — per model instance (a root carrying a [`RigSkin`] slot and a
//!   [`WorldUnit`] bound), the reference's two booleans at the transformed bound centre. A
//!   straddler's plane goes into the per-slot [`WaterClips`] table — a region of the shared light
//!   buffer on the same slot index as the body tint — and [`ModelWaterBand`] on the root is
//!   the edge the classifier fans down from.
//! - `model_render::classify_water_side` reads the SAME table word by the batch's `MeshTag` slot:
//!   a straddling batch stays on its NEAR identity and is marked [`StraddlesWater`], and
//!   [`sync_straddle_twins`] hangs a child on it carrying the FAR twin — the second list.
//! - `wow_model.wgsl`'s `WOW_WATER_CLIP` block (every transparent-pass pipeline) is the clip plane:
//!   a fragment of a straddling instance survives only on its own copy's side — the near copy
//!   keeps the eye's half, the far twin (`FAR_SIDE_MARKER`) the other. [`keeps`] is the law's Rust
//!   twin.
//!
//! The CPU twin set and the GPU clip read one word, so they cannot disagree: a batch is doubled
//! exactly when its fragments are being halved.
//!
//! **Scope: slot-bearing instances.** The clip is per INSTANCE, and the one per-instance channel
//! the fragment stage has is the rig slot — so the split covers every skinned wire body (units,
//! players, rigged GameObjects), the parts carrying its slot (boneless geosets, billboard cards),
//! and every model CHAINED to it — worn gear and hung spell kits, which own slots of their own
//! since 1609 and take the body's word through their `ParentModel` link. Slot-0
//! content — map doodads, unskinned models — keeps 0919's one-sided fallback; its only
//! translucent episodes are the distance-fade ring's small props.

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

/// One slot's clip word: `[plane height, near side]` — the waterline's world height (Bevy Y,
/// which is WoW Z) and the side the instance's NEAR copy keeps: `+1` above (a dry eye's near list
/// is the above one), `−1` below (a submerged eye's, `0x4836d6`). A side of `0` is "not
/// straddling" — every slot-0 instance, every slot nothing wrote, and every zeroed studio buffer.
pub type ClipWord = [f32; 2];

/// The "no clip" word. Zero is identity, so a zeroed region is inert by construction: the
/// portrait booths' studio buffers never receive this region's writes and keep it that way.
pub const NO_CLIP: ClipWord = [0.0, 0.0];

/// Bytes per slot in the clip region: two `f32` — `wow_model.wgsl`'s `array<vec2<f32>, 2048>`.
const SLOT_BYTES: u64 = 8;

/// Byte offset of the clip region inside a `wow_light`-layout buffer: after the mat-anim table,
/// before the palette rows (which stay last — the shader's one runtime-sized array).
pub(crate) fn region_offset() -> u64 {
    crate::mat_anim_table::region_offset() + crate::mat_anim_table::region_bytes()
}

/// Bytes this region adds to every `wow_light`-layout buffer (16 KB at 2048 slots).
pub(crate) fn region_bytes() -> u64 {
    MAX_RIG_SLOTS as u64 * SLOT_BYTES
}

/// The live per-slot clip table, indexed by the instance's `MeshTag` rig slot. `Arc`-shared so
/// the render-world extract is a pointer bump, and generation-stamped so a world where nothing
/// straddles uploads nothing at all.
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
    /// Set slot `slot`'s word. Slot 0 is the world's no-rig sentinel and is never written:
    /// everything unskinned shares it.
    pub(crate) fn set(&mut self, slot: u16, word: ClipWord) {
        let i = slot as usize;
        if i == 0 || i >= MAX_RIG_SLOTS || self.slots[i] == word {
            return;
        }
        Arc::make_mut(&mut self.slots)[i] = word;
        self.generation += 1;
    }

    /// Back to "not straddling" — called from `RigSkin`'s free hook as well, so a recycled slot
    /// can never hand a dead unit's waterline to the next unit that allocates it.
    pub(crate) fn clear(&mut self, slot: u16) {
        self.set(slot, NO_CLIP);
    }

    /// This slot's word (`NO_CLIP` for an out-of-range slot) — what a chained model inherits
    /// from its wearer.
    pub(crate) fn word(&self, slot: u16) -> ClipWord {
        self.slots.get(slot as usize).copied().unwrap_or(NO_CLIP)
    }

    /// Whether `slot`'s instance straddles its water plane — the classifier's read of the very
    /// word the fragment clips by.
    pub(crate) fn straddles(&self, slot: u16) -> bool {
        self.slots.get(slot as usize).is_some_and(|w| w[1] != 0.0)
    }
}

/// On a model-instance root: whether its model straddles its water plane this frame. Written by
/// [`band_instances`] (change-gated) and read as a change EDGE — `classify_water_side`
/// re-classifies the holder's subtree when it flips, the same fan-down the room claim takes.
/// Absent reads "no": it is inserted the first time an instance straddles.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelWaterBand {
    pub straddles: bool,
}

/// On a transparent batch of a straddling instance: the classifier's verdict that this batch
/// draws on both lists. Sparse; [`sync_straddle_twins`] keeps a far twin alive exactly while it
/// is present.
#[derive(Component)]
pub(crate) struct StraddlesWater;

/// On a straddling batch: its live far-side twin. [`sync_straddle_twins`] is the only writer.
#[derive(Component)]
pub(crate) struct StraddleTwin(Entity);

/// On a twin: the batch it doubles for — the mirror's key, and the marker that keeps a twin out
/// of the classifier's walk (a twin is the classification's output, never its input).
#[derive(Component)]
pub(crate) struct StraddleTwinOf(Entity);

/// The reference's two side booleans for a model whose bound centre sits `d` yd over its water
/// plane, with slack `r`: `(above, below) = (d ≥ −r, d ≤ r)` (`0x7079a2`–`0x707a17`). Both ties
/// are inclusive — `d == −r` keeps A (the emitter lane's `is_above`), `d == r` keeps B (`jp`
/// @`0x7079e2`) — and a NaN keeps neither, which reads as "not straddling".
pub(crate) fn side_booleans(d: f32, r: f32) -> (bool, bool) {
    (d >= -r, d <= r)
}

/// How far past the reference's `±r` band a straddler must travel before it stops straddling.
///
/// **Not a fidelity deviation.** A split draw is pixel-identical to the one-list draw for a model
/// wholly on one side of its plane — one copy's half is simply empty — so where the verdict flips
/// only decides what the draw COSTS, never what it shows. The reference recomputes its booleans
/// every frame with no hysteresis because its flip is free (list membership); ours spawns or
/// despawns a twin and swaps the batch's handle. Without this, a murloc bobbing 0.3 yd at the
/// band's edge flipped nearly every frame (the 2188 live leg: 17 transitions on one body, three
/// inside 40 ms).
const STICKY_YD: f32 = 1.0;

/// The straddle verdict with the sticky exit: an instance enters on the reference's band and
/// leaves only once its centre is [`STICKY_YD`] past it. `was` = it straddled last time it was
/// banded.
pub(crate) fn straddles(d: f32, r: f32, was: bool) -> bool {
    let slack = if was { r + STICKY_YD } else { r };
    side_booleans(d, slack) == (true, true)
}

/// The clip plane's law — the Rust twin of `wow_model.wgsl`'s `WOW_WATER_CLIP` block: does a
/// fragment at world height `y` survive on this copy of a batch? `far_copy` = the copy's material
/// carries `FAR_SIDE_MARKER` (it is the twin on the eye's far list). A `0` side keeps everything,
/// and a fragment exactly on the plane survives on both copies.
pub fn keeps(word: ClipWord, y: f32, far_copy: bool) -> bool {
    if word[1] == 0.0 {
        return true;
    }
    let side = if far_copy { -word[1] } else { word[1] };
    side * (y - word[0]) >= 0.0
}

/// The waterline a CHAINED model clips at: its nearest body's word, walked up the
/// [`ParentModel`] chain from `start` (the model's parent). `link(k)` answers, for one link, its
/// own palette slot, its parent, and whether it is a body; a broken chain, a body with no slot
/// and a chain past [`MAX_MODEL_CHAIN`] all read "not straddling". Generic over the key so the
/// walk is testable without an ECS.
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

/// The roots [`band_instances`] re-bands on an ordinary frame: an instance whose matrix, bound,
/// slot or room claim moved. The eye crossing the surface and the surface population changing
/// re-band everything.
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

/// A model chained to a body — worn gear and the spell kits hung on it — with its own slot.
type ChainedRoot<'a> = (
    Entity,
    &'a RigSkin,
    &'a ParentModel,
    Option<Mut<'a, ModelWaterBand>>,
);

/// A chained model whose slot or link is new this frame — it has no word yet, whatever its
/// wearer's did.
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

/// **Band every model instance against its water plane** — the reference's two side booleans
/// per instance, written where both halves of the split read them: the per-slot [`WaterClips`]
/// word (the fragment's clip plane, the classifier's doubling verdict) and [`ModelWaterBand`]
/// on the root (the classifier's re-classify edge).
///
/// **Every model chained to a body inherits the body's word**. Worn gear does not
/// share its wearer's slot: since 1609 every ordinary item rides a rider slot of its own (0841's
/// welded items a joint rig), so a helm, a pair of shoulders or a weapon at the waterline was
/// never banded and kept the one-list fallback — the hole in the water the director still saw
/// when the surface crossed a unit's gear. The body's band is the instance's verdict and its
/// plane is the one water plane under it, and the clip is exact on any copy whatever side it
/// lands, so the gear takes the word verbatim rather than a band of its own.
///
/// Update, after the submersion verdict and before `classify_water_side` — the auto-inserted
/// sync point between them lands a first-time `ModelWaterBand` before the classifier looks, so
/// the clip and the twin set flip on the same frame.
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
    // The chained pass: every chained model when any body's word moved this frame (the gate is
    // the table's own generation — a dry world, and a wet one standing still, skip it whole),
    // otherwise only the chains that are new.
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

/// One chained model's word — its wearer's, onto its own slot — and its band edge.
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

/// Publish an instance's verdict as its [`ModelWaterBand`] edge: change-gated on a live
/// component, inserted the first time it straddles.
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

/// One instance's band — the shared body of [`band_instances`]' full and reactive paths.
fn band_one(
    interleave: &WaterInterleave,
    clips: &mut WaterClips,
    commands: &mut Commands,
    near_side: f32,
    (entity, gt, unit, rig, band): BandRoot<'_>,
) {
    // The reference's point and slack: the bound centre through the instance matrix (`0x70848d`),
    // the radius through its row-0 scale (`0x708478`). The game hands the armed idle's authored
    // CAaBox rather than the header sphere, so the slack is that box's circumscribed sphere —
    // never smaller than the model, and an over-wide band costs only a copy whose half the clip
    // discards whole.
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

/// One straddle candidate as [`sync_straddle_twins`] sees it: the batch, what its twin copies
/// (tag, mesh, handle, cull box and marker, layers), its verdict, and the twin it already has.
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

/// Keep every straddling batch's far twin in sync with its verdict — spawn the twin child when
/// the classifier marks the batch [`StraddlesWater`], despawn it when the mark goes — and mirror
/// the batch's tag (the fade alpha moves every ramp frame; the slot rides it), mesh (a gear
/// redress swaps it in place) and handle (composed down one water rung) onto a live twin.
///
/// PostUpdate, after the depth-prime sync: a zfill twin is a batch like any other here, so its
/// own far twin mirrors the tag the zfill mirror has just written.
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
                // The classifier builds the far twin before it marks the batch; a miss means the
                // batch's handle moved since, and next frame's classification rebuilds it.
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
                // The twin culls exactly as its batch does: a skinned part's cull belongs to the
                // body root's election, and its own box is the picker's (`attach::dress`).
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
            continue; // batch going away this frame — the child despawns with it
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

/// The split's instrument (`WOW_MOVE_TRACE=<path>`, tag `fx` — beside the classifier's
/// `straddle`/`far-side` lines): one line per twin edge.
fn trace(what: &str, part: Entity) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    benilla_assets::trace::line("fx", &format!("straddle-twin {what} part={part}"));
}

/// Render world (`PrepareResources`): write the whole 16 KB region when anything changed — the
/// shared buffer only. The portrait booths and the glue scene keep their zeroed region: nothing in
/// a studio stands in world water.
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

    /// The band is the reference's: a centre within `r` of the plane on EITHER side straddles,
    /// both ties included; beyond it the model sits wholly on one list.
    #[test]
    fn the_band_is_plus_minus_r_inclusive() {
        // A wading unit: centre 0.4 yd over the surface, 1.5 yd of slack — both lists.
        assert_eq!(side_booleans(0.4, 1.5), (true, true));
        // Waist-deep the other way: centre under the surface, still inside the band.
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

    /// Entry is the reference's band; exit is sticky by `STICKY_YD`, so a body bobbing at the
    /// edge holds its verdict instead of re-spawning its twins every frame.
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

    /// The clip plane: for a dry eye the near copy keeps the part above the water and the far
    /// twin the part below; a submerged eye swaps them; the plane itself survives on both copies,
    /// so the two halves meet without a seam.
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

    /// Gear and kits take their BODY's word however deep the chain — a glow on a weapon on a unit
    /// — and a chain that never reaches a body, or a body with no slot, clips nothing.
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

    /// Every slot-0 draw in the world shares the word: writing it would clip terrain, WMOs and
    /// doodads at one unit's waterline, so the setter refuses it — and a dry world never uploads.
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

    /// The shader declares `array<vec2<f32>, 2048>` between two `vec4` arrays: the region must be
    /// one 8-byte word per addressable slot and end on the 16-byte boundary the palette rows need.
    #[test]
    fn the_region_is_one_vec2_per_slot_on_a_vec4_boundary() {
        assert_eq!(region_bytes(), (MAX_RIG_SLOTS * 8) as u64);
        assert_eq!(region_offset() % 16, 0);
        assert_eq!((region_offset() + region_bytes()) % 16, 0);
    }
}
