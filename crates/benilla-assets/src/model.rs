//! Shared building blocks for the M2 and WMO model loaders: the per-batch submesh type and the
//! WoW→Bevy mesh bake. Both formats decode to a list of these (geometry + texture + blend/sidedness),
//! with materials deferred to the spawn site (see the loader module docs) so the assets stay
//! app-independent.

use benilla_formats::{
    BillboardKind, BoneScaleAnim, CharSkinSlot, M2Attachment, ModelAnimation, ModelBlend,
    RenderSubmesh, Skeleton,
};
use bevy::animation::animatable::Animatable;
use bevy::animation::animation_curves::{AnimatableCurve, AnimatableKeyframeCurve};
use bevy::animation::{animated_field, AnimationClip, AnimationTargetId};
use bevy::asset::RenderAssetUsages;
use bevy::math::Mat3;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

use crate::coords::wow_to_bevy;

mod anims;
mod pose;
pub use anims::{AnimClip, ClipEvent, ModelAnimations};
pub use pose::{PoseBone, PoseClip, PoseNode, PoseSource, PoseTrack};

/// A billboarded submesh's render data (Bevy space): the bone pivot the card rotates about (model-local
/// — compose with the instance transform) and the billboard kind. The mesh is built **centred at
/// `pivot`** so the spawn site can rotate it about the pivot to face the camera each frame.
#[derive(Clone)]
pub struct BillboardInfo {
    pub pivot: Vec3,
    /// The billboard bone's index — the joint the card rides on an animated host (swinging lamp,
    /// mount lights). On a rest-pose host the pivot alone places the card.
    pub bone: u16,
    pub kind: BillboardKind,
    /// The billboard bone's global-sequence scale animation (the looping glow-card pulse), if any —
    /// sampled per-frame at the spawn site and folded into the card scale. `None` for a static card.
    pub scale_anim: Option<BoneScaleAnim>,
    /// The bone's per-sequence translation loops `(anim id, loop)`, keys baked to **Bevy axes** —
    /// a model-local offset the card adds at its pivot when the spawn site arms one. Anim 0 is the
    /// load arm's default (the questgiver `?` marker's low bob); anim 190 the marker's raised bob,
    /// armed while the unit shows an overhead name (only the marker arms these today; see
    /// `benilla_formats::Billboard::seq_translations`).
    pub seq_translations: Vec<(u16, BoneScaleAnim)>,
}

/// One render batch of a loaded model (an M2 batch or a WMO group batch): the decoded geometry,
/// the batch's albedo texture (WorldArt), and its blend/sidedness. Carries data + metadata only —
/// **no `Mesh` assets** (decision 0834, the model-lane twin of 0832's terrain rule): a labeled
/// mesh sub-asset lands the instant the decode completes and the render world ingests the whole
/// model in ONE frame, which is the city first-contact spike. The app builds each batch's render
/// form paced (`benilla`'s `model_forms`) via [`submesh_to_static_mesh`] /
/// [`submesh_to_skinned_mesh`]; the material stays a spawn-site concern as before.
#[derive(Clone)]
pub struct ModelSubmesh {
    /// The batch's decoded geometry (model space, WoW axes) — everything the mesh builders below
    /// need, shared across every instance of the model. `Arc` keeps the submesh clone cheap and is
    /// the model's single resident CPU copy of the vertex data (the static render form is
    /// GPU-only; see [`submesh_to_static_mesh`]).
    pub geometry: std::sync::Arc<RenderSubmesh>,
    /// Embedded albedo texture, if any. `None` for creature skin slots (filled at spawn from the
    /// display's skin variation — see [`Self::skin_slot`]).
    pub texture: Option<Handle<Image>>,
    /// The creature skin variation this batch draws from (`Some(0/1/2)` ⇒ `Monster1/2/3`) when its
    /// [`Self::texture`] is `None` — so the entity spawn site can fill it from `CreatureDisplayInfo`.
    /// `None` for batches with their own embedded texture (all doodads/WMOs; most M2 batches).
    pub skin_slot: Option<u8>,
    /// The batch's `skinSectionId` (geoset / mesh-part ID, see
    /// [`benilla_formats::RenderSubmesh::geoset_id`]) — `group*100 + variant`. The character compositor
    /// selects which geosets a player shows by this; `0` for a single-geoset model (creatures/doodads).
    pub geoset_id: u16,
    /// The character runtime texture slot this batch carries (see
    /// [`benilla_formats::RenderSubmesh::char_slot`]) — the spawn site fills its texture per-player from
    /// the appearance (decisions 0041 / 0044 / 0045). `None` for everything with its own texture.
    pub char_slot: Option<CharSkinSlot>,
    pub blend: ModelBlend,
    pub two_sided: bool,
    /// This batch is a WMO **interior** group (see [`benilla_formats::RenderSubmesh::interior`]) — lit
    /// by its baked MOCV with the directional sun off. `false` for M2 doodads and exterior WMO groups.
    pub interior: bool,
    /// This batch is unlit (see [`benilla_formats::RenderSubmesh::emissive`]) — rendered fullbright:
    /// M2 `UNLIT (0x01)` glass/glow, or WMO `UNLIT` on an exterior-group batch. `false` otherwise.
    pub emissive: bool,
    /// This batch's texture is the **icon slot** (M2 texture type 14, see
    /// [`benilla_formats::RenderSubmesh::icon_slot`]): the spawn site fills it from
    /// `Model:ReplaceIconTexture`'s path. `false` for every other batch and all of WMO.
    pub icon_slot: bool,
    /// The WMO MOMT **SIDN** night-glow colour (see [`benilla_formats::RenderSubmesh::sidn`]) — the
    /// authored emissive RGB the shader ramps by the night fraction on lit lanes. `None` for M2.
    pub sidn: Option<[u8; 3]>,
    /// WMO MOMT **WINDOW** (see [`benilla_formats::RenderSubmesh::window`]) — an interior-group
    /// batch takes the brighter Direct/Ambient-midpoint light. `false` for M2.
    pub window: bool,
    /// This batch blends **additively** (M2 glow cards / coronae — see
    /// [`benilla_formats::RenderSubmesh::additive`]): the spawn site builds it with `AlphaMode::Add` so
    /// its warm colour is added on top of the scene, not mixed with the background. `false` otherwise.
    pub additive: bool,
    /// This batch's texture coordinates are **generated, not authored** (see
    /// [`benilla_formats::RenderSubmesh::env_map`]): a sphere-map environment coordinate off the
    /// view-space reflection vector. The spawn site marks the material so `wow_model.wgsl` derives
    /// the UV instead of reading the (meaningless) vertex one. `false` for WMO batches and for the
    /// M2 batches that name a real UV channel.
    pub env_map: bool,
    /// M2 render flag **0x10 — disable depth write** (see [`benilla_formats::RenderSubmesh::no_depth_write`]):
    /// `specialize` keeps depth-write ON for transparent batches *unless* this is set, matching the real
    /// client (so a model's transparent cards occlude each other instead of bleeding through). `false` for WMO.
    pub no_depth_write: bool,
    /// M2 render flag **0x08 — disable depth test** (drawn over everything). `false` for WMO.
    pub no_depth_test: bool,
    /// The batch's fog COLOUR policy (see [`benilla_formats::RenderSubmesh::fog_policy`]) — `Scene`
    /// for WMO and every non-M2 batch.
    pub fog_policy: benilla_formats::FogPolicy,
    /// Set when this batch rides an M2 billboard bone (glow cards, chains): the spawn site faces it to
    /// the camera each frame. The [`Self::mesh`] is built centred at the pivot so it rotates in place.
    pub billboard: Option<BillboardInfo>,
    /// The batch's **animated material alpha** (decision 0130 phase 2, combined in `0x707680`):
    /// time-varying colour-alpha/transparency-weight loops (or a dimming constant) the doodad spawn
    /// site samples per instance into the render-alpha channel. `None` for the overwhelming majority
    /// (both factors static-1 or statically culled). `Arc` keeps the submesh clone cheap.
    pub alpha_anim: Option<std::sync::Arc<benilla_formats::AlphaAnim>>,
    /// The batch's **UV-animation** loop (decision 0130 phase 3; sampled in `0x714260`, applied at
    /// `0x70b740`): the texture transform's translation track — sampled per frame into the batch
    /// material's UV offset (flowing waterfalls, scrolling energy). `None` for the ~98.6% of models
    /// with no texture transform and all WMO batches. The `Arc` doubles as the material-dedup
    /// identity: every instance of a loaded model shares this allocation.
    pub uv_anim: Option<std::sync::Arc<benilla_formats::UvAnim>>,
    /// The batch's UV loop **per file sequence slot**, `Some` only where the slots disagree — the
    /// batches for which [`Self::uv_anim`]'s single loop is structurally unable to be right,
    /// because which loop applies depends on the sequence the *instance* is playing (decision
    /// 1408). Like [`Self::uv_anim`], the `Arc` doubles as a material-dedup identity.
    pub uv_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    /// The batch's texture-transform **rotation** loop per file sequence slot (decision 2019)
    /// — the UI model tiles sample it off the pane's play head into the material's affine row;
    /// no world lane reads it (no placed doodad authors one). `None` for the rest.
    pub uv_rot_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>>>,
    /// The scaling twin of [`Self::uv_rot_seq`].
    pub uv_scale_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    /// The batch's **animated RGB tint** (the M2Color colour track, time-varying only — a spell
    /// effect's white-hot flash cooling to red). When `Some`, the static vertex tint was skipped
    /// at parse (`benilla-formats`): the render side seeds the material's tint at the first key —
    /// pixel-identical to the old static bake — and the material-animating lanes tick it per
    /// frame. `None` for constant/keyless tints (the vertex bake).
    pub rgb_anim: Option<std::sync::Arc<benilla_formats::RgbAnim>>,
    /// The tint twin of [`Self::uv_seq`], on the same rule.
    pub rgb_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 3]>>>,
    /// The batch's MOBA section for WMO group batches ([`benilla_formats::WmoBatchClass`] — an
    /// interior group's per-batch lighting law: INT = unlit `tex × MOCV`, TRANS = the MOCV-alpha
    /// lit↔bake lerp, EXT = the exterior day/night law). `None` for every M2 batch.
    pub wmo_batch: Option<benilla_formats::WmoBatchClass>,
    /// The batch's flat **ground-plane quad** shape ([`benilla_formats::GroundQuad`], detected at
    /// load — corners stay in WoW model space; consumers map through `coords::wow_to_bevy`). The
    /// ground-fx decal lane (`benilla::ground_fx`) re-renders such parts of a base-anchored spell
    /// effect as projected surface decals. `None` for ordinary geometry and all WMO batches.
    pub ground_quad: Option<benilla_formats::GroundQuad>,
}

/// Turn a [`RenderSubmesh`] (model space) into a Bevy [`Mesh`] baked to Bevy space — decision 0002,
/// applied here at the render boundary. Prefers the authored normals (soft, outward — how WoW lights
/// foliage); recomputes flat ones only when absent. WMO per-vertex MOCV colour is folded in when
/// present (M2 has none). The WoW→Bevy map is a pure rotation, so it applies to normals too.
///
/// `usages` is the caller's lane split (decision 0834): the **static** form is `RENDER_WORLD`-only
/// (nothing reads it main-side; the render world takes the buffers at extract, no resident CPU
/// copy), while the **skinned** twin keeps the default `MAIN_WORLD | RENDER_WORLD` because the
/// mouseover picker rays its vertices on the main world (`target::hover::ray_posed_mesh`).
fn build_submesh_mesh(sub: &RenderSubmesh, usages: RenderAssetUsages) -> Mesh {
    // A billboard batch is built centred at its bone pivot, so the spawn site can rotate it about that
    // point to face the camera (the entity transform then re-places it at the pivot in the world).
    let center = sub
        .billboard
        .as_ref()
        .map_or(Vec3::ZERO, |b| wow_to_bevy(b.pivot));
    let positions: Vec<[f32; 3]> = sub
        .positions
        .iter()
        .map(|p| (wow_to_bevy(*p) - center).to_array())
        .collect();
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, usages);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, sub.uvs.clone());
    mesh.insert_indices(Indices::U32(sub.indices.clone()));
    if sub.vertex_colors.len() == sub.positions.len() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, sub.vertex_colors.clone());
    }
    if sub.normals.len() == sub.positions.len() {
        // A billboard CARD is lit on the face it PRESENTS (decision 0788): a card authored
        // back-to-front against the law's `+X`-at-the-viewer gets its plane's normals turned round
        // here, so its shading stops swinging with the camera (the shape, the reference's own
        // behaviour and the 279-batch population are on
        // [`RenderSubmesh::billboard_card_faces_away`]).
        //
        // **Normals only — never the winding.** The 177 away-facing SINGLE-sided cards are
        // authored placeholder geometry the reference backface-culls from every angle
        // (`bbfacescan`), and our `cull_mode` reproduces that; re-winding them would reveal it.
        let flip = sub.billboard_card_faces_away();
        let normals: Vec<[f32; 3]> = sub
            .normals
            .iter()
            .map(|n| {
                let b = wow_to_bevy(*n);
                if flip { -b } else { b }.to_array()
            })
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    } else {
        mesh.compute_normals();
    }
    mesh
}

/// The skinned twin's per-vertex joint indices (`Uint16x4` — M2 bone indices, used directly as
/// palette indices). **Deliberately NOT `Mesh::ATTRIBUTE_JOINT_INDEX`** (decision 0720): Bevy's
/// `SKINNED` pipeline path triggers on the standard attributes being in the mesh LAYOUT — while
/// its draw-time skin bind group resolves per ENTITY registration — so a mesh carrying them can
/// only ever render through Bevy's skin lane. Our own attribute ids keep Bevy's `is_skinned()`
/// false everywhere; `WowModelExt::specialize` sees them in the layout and compiles the
/// `WOW_RIG_SKIN` path instead, which skins from the owned palette region of the shared light
/// buffer (`rig_palette`). Shader locations 10/11 (Bevy's builtin set ends at 7).
pub const ATTRIBUTE_WOW_JOINT_INDEX: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new(
        "Wow_JointIndex",
        988_540_917,
        bevy::render::render_resource::VertexFormat::Uint16x4,
    );
/// The skinned twin's per-vertex joint weights (`Float32x4`, normalised) — see
/// [`ATTRIBUTE_WOW_JOINT_INDEX`].
pub const ATTRIBUTE_WOW_JOINT_WEIGHT: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new(
        "Wow_JointWeight",
        988_540_918,
        bevy::render::render_resource::VertexFormat::Float32x4,
    );

/// A merged fader blob's per-vertex fade sphere (`center.xyz` world-space, `w` = fade radius —
/// decision 1418): every vertex of one baked placement carries the placement's sphere, and
/// `wow_model.wgsl` computes the faithful `doodad_fade_alpha` from it per vertex (the erode
/// lane), replacing the per-entity MeshTag/twin/Hidden fade channels a merged draw cannot
/// express. Own attribute id for the same reason as the joints above; shader location 12.
pub const ATTRIBUTE_WOW_FADE_SPHERE: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new(
        "Wow_FadeSphere",
        988_540_919,
        bevy::render::render_resource::VertexFormat::Float32x4,
    );

/// A merged interior-prop blob's per-vertex SH-probe slot (decision 1418 lane 3): the slot
/// index every vertex of one baked placement carries, replacing the per-entity `MeshTag`
/// payload (`wow_model.wgsl`'s `WOW_MERGED_SLOT` reads it instead of the tag bits). Shader
/// location 13.
pub const ATTRIBUTE_WOW_MERGED_SLOT: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new(
        "Wow_MergedSlot",
        988_540_920,
        bevy::render::render_resource::VertexFormat::Uint32,
    );

/// The app-facing **static** mesh build (decision 0834): geometry only, `RENDER_WORLD`-only
/// usages — the render world takes the vertex buffers at extract and the main world keeps no
/// copy. Consumers must pair it with an explicit `Aabb` computed at build time (the exterior
/// cull fails OPEN on a missing bound, and `RENDER_WORLD` races Bevy's `calculate_bounds` —
/// the same rule 0832 set for terrain cells).
pub fn submesh_to_static_mesh(sub: &RenderSubmesh) -> Mesh {
    build_submesh_mesh(sub, RenderAssetUsages::RENDER_WORLD)
}

/// Build the **skinned** twin of [`submesh_to_static_mesh`]: the same baked geometry plus the
/// per-vertex [`ATTRIBUTE_WOW_JOINT_INDEX`] + [`ATTRIBUTE_WOW_JOINT_WEIGHT`]. These two
/// attributes are the entire trigger for the owned-palette skinning path (decisions 0019/0720).
/// When the submesh carries no skin (WMO / a boneless batch) this is identical to the static
/// mesh — harmless, but the rigged paths are the only consumers regardless. Keeps the default
/// `MAIN_WORLD | RENDER_WORLD` usages: the mouseover picker skins these vertices on the CPU
/// (`target::hover`), so the main-world copy is read, not waste.
pub fn submesh_to_skinned_mesh(sub: &RenderSubmesh) -> Mesh {
    let mut mesh = build_submesh_mesh(sub, RenderAssetUsages::default());
    if sub.joints.len() == sub.positions.len() && !sub.joints.is_empty() {
        mesh.insert_attribute(
            ATTRIBUTE_WOW_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(sub.joints.clone()),
        );
        mesh.insert_attribute(
            ATTRIBUTE_WOW_JOINT_WEIGHT,
            VertexAttributeValues::Float32x4(sub.weights.clone()),
        );
    }
    mesh
}

/// One joint of a model's rest skeleton, baked to Bevy space (decision 0019): its parent joint index
/// (`-1` = root → parented to the entity) and its **local** rest translation. The skinned creature
/// path spawns one entity per joint carrying this translation; animation later drives each joint's TRS.
#[derive(Clone, Copy)]
pub struct ModelJoint {
    pub parent: i16,
    pub local_translation: Vec3,
    /// The bone's billboard arm, if authored (M2 bone flags `0x08/0x10/0x20/0x40`). The byte law
    /// replaces the billboarded bone's rotation IN THE PALETTE, so children — and any geometry
    /// skinned to them — inherit the camera facing (the frost-armor sheets skin to the scale-in
    /// CHILD of a lock-Z bone). Rigged hosts feed this to the billboard joint pass; the per-batch
    /// card split only covers geometry skinned to the billboard bone itself.
    ///
    /// Authored on every bone that carries the flag, welded seam or not: the animate kernel
    /// `0x714260` reads no vertex, weight or triangle data at all, so there is no topology gate on
    /// the arm (decision
    /// 0945 superseding 0935). A welded seam stays welded because the replacement is built about
    /// the bone's own pivot, which is what makes the blended seam ring move by millimetres.
    pub billboard: Option<benilla_formats::BillboardKind>,
    /// Bone flags `0x1/0x2/0x4` (see [`benilla_formats::SkeletonBone::parent_arm`]): how the
    /// bone's effective PARENT matrix is rebuilt from the model's own root frame before it
    /// composes. Applied by the same palette passes as `billboard`, and *before* them.
    pub parent_arm: Option<benilla_formats::ParentArm>,
}

/// A model's rest skeleton in Bevy space (decision 0019). Built once per M2 asset; the creature path
/// spawns a joint-entity hierarchy per instance from `joints` and shares the matching inverse bind
/// poses ([`build_skeleton`]'s second return) across instances.
#[derive(Clone, Default)]
pub struct ModelSkeleton {
    pub joints: Vec<ModelJoint>,
    /// The bone whose `KeyBoneID == 4` (SpineLow) — the client's display-facing counter-twist spine
    /// channel (`0x711f10(4, …)` in the `0x607ed0` body-facing tail): while a unit's rendered root
    /// yaw is offset from its aim (a strafe), this subtree counter-rotates back toward the aim.
    /// `None` when the model lacks the key-bone (beasts, props).
    pub spine_bone: Option<u16>,
    /// The bone whose `KeyBoneID == 6` (Head) — the twist's head channel (`0x711f10(6, …)`), taking
    /// the gap the spine channel leaves so the head lands back on the aim. `None` when absent.
    pub head_bone: Option<u16>,
}

/// Bake a raw [`Skeleton`] into the Bevy-space [`ModelSkeleton`] + its inverse-bind-pose matrices.
///
/// Rig math per the reference (vanilla M2 has no inverse-bind array, rest pose is identity TRS,
/// the **pivot encodes bind position**) composed with Bevy's joint formula
/// (`joint_matrix = joint_global · inverse_bindpose`, which *becomes* `world_from_local`):
/// - **inverse bind pose** `i = translate(−pivot_i)` (model→bone-local at bind),
/// - **joint rest-local translation** = `pivot_i − pivot_parent` (pure translations, so they telescope
///   to `pivot_i` up the chain). At rest every joint matrix collapses to the entity transform, so the
///   creature renders exactly where the static mesh did, undeformed — even with a scaled entity
///   transform (the two pivot translations cancel before scale acts).
///
/// Pivots map WoW→Bevy via [`wow_to_bevy`] (a pure rotation), so the whole skeleton lives in mesh space.
pub(crate) fn build_skeleton(skel: &Skeleton) -> (ModelSkeleton, Vec<Mat4>) {
    let pivots = skeleton_pivots(skel);
    let joints = skel
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let parent_pivot = usize::try_from(b.parent)
                .ok()
                .and_then(|p| pivots.get(p).copied())
                .unwrap_or(Vec3::ZERO);
            ModelJoint {
                parent: b.parent,
                local_translation: pivots[i] - parent_pivot,
                billboard: b.billboard,
                parent_arm: b.parent_arm,
            }
        })
        .collect();
    let inverse_bindposes = pivots.iter().map(|p| Mat4::from_translation(-*p)).collect();
    // A bone carries KeyBoneID `k` iff `keyBoneLookup[k]` points at it (same rule as
    // [`upper_subtree_root`]); 4 = SpineLow, 6 = Head — the display-facing twist channels.
    let key_bone = |k: i16| {
        skel.bones
            .iter()
            .position(|b| b.key_bone == k)
            .map(|i| i as u16)
    };
    (
        ModelSkeleton {
            joints,
            spine_bone: key_bone(4),
            head_bone: key_bone(6),
        },
        inverse_bindposes,
    )
}

/// Each bone's Bevy-space bind-pose pivot (`wow_to_bevy(bone.pivot)`), in file order — the one
/// source [`build_skeleton`] (inverse bind poses) and [`build_attachments`] (attach-point offsets)
/// both derive from, so the two never compute a divergent pivot.
pub(crate) fn skeleton_pivots(skel: &Skeleton) -> Vec<Vec3> {
    skel.bones.iter().map(|b| wow_to_bevy(b.pivot)).collect()
}

/// One M2 attachment point (decision 0072 — held items), baked to a **Bevy-space bone-local
/// offset**: a child spawned under the bone's joint entity at `Transform::from_translation(offset)`
/// sits exactly at the attach point at bind pose and rides the bone's animation thereafter.
/// `offset` is `≈ Vec3::ZERO` on character models (the attach bones are leaves sitting exactly at
/// the attach point — VERIFIED, direct dump of `HumanMale.m2`, `decisions/0072`).
#[derive(Clone, Copy)]
pub struct ModelAttachment {
    pub id: u16,
    pub bone: u16,
    pub offset: Vec3,
}

/// The right/left **arm subtree roots** for the per-arm animation masks: each hand attachment's
/// (id 1 = right, id 2 = left) bone walked up to its shoulder-keybone ancestor (keybones 2/3 —
/// either; the walk just finds "the shoulder above this hand", so no left/right convention is
/// assumed). `None` when the model lacks either hand attachment or a shoulder ancestor (beasts,
/// props — they never play per-slot one-shots).
pub(crate) fn arm_subtree_roots(
    skel: &Skeleton,
    attachments: &[ModelAttachment],
) -> Option<(usize, usize)> {
    let shoulder_of = |hand_attach_id: u16| -> Option<usize> {
        let mut bone = attachments
            .iter()
            .find(|a| a.id == hand_attach_id)
            .map(|a| a.bone as usize)?;
        loop {
            let b = skel.bones.get(bone)?;
            if matches!(b.key_bone, 2 | 3) {
                return Some(bone);
            }
            bone = usize::try_from(b.parent).ok()?;
        }
    };
    Some((shoulder_of(1)?, shoulder_of(2)?))
}

/// The model's **upper-body split key-bone** subtree root — the client's `CGUnit+0xd5c`, selected by
/// the capability probe `0x60ce70` (`keyBoneLookup[4]` preferred, `[6]` fallback, else the −1
/// sentinel = no split). On HumanMale `keyBoneLookup[4]` is bone 20 (SpineLow: chest/shoulders/arms/
/// hands/head — a subtree that provably **excludes** the legs, which hang off the pelvis sibling
/// bone 21), so a clip masked to this subtree moves the upper body only. A bone carries KeyBoneID
/// `k` iff `keyBoneLookup[k]` points at it, so the SpineLow bone is the one whose `key_bone == 4`
/// (else `== 6`). `None` when the model has neither (the −1 sentinel) — the one-shot route then
/// falls back to full-body.
pub(crate) fn upper_subtree_root(skel: &Skeleton) -> Option<usize> {
    let bone_with = |k: i16| skel.bones.iter().position(|b| b.key_bone == k);
    bone_with(4).or_else(|| bone_with(6))
}

/// The model's **finger key-bone subtree roots** per hand `(right, left)` — the client's `CloseHand`
/// targets. WoW rigs each hand's fingers under **key-bones 8–12 (mainhand/right)** and **13–17 (offhand/
/// left)**; the grip pose (`HandsClosed`, AnimationData 15) is armed on exactly these when a weapon is
/// held in that hand (`0x479660`/`0x60b590`). Only the *rigged* ones exist (HumanMale carries 11/12
/// and 16/17; the rest are the −1 sentinel), so each entry is the set of bones actually carrying a
/// finger key-bone in that hand's range — **empty** for a model with no finger key-bones
/// (beasts/props), which never grip.
pub(crate) fn finger_subtree_roots(skel: &Skeleton) -> [Vec<usize>; 2] {
    let roots = |lo: i16, hi: i16| -> Vec<usize> {
        skel.bones
            .iter()
            .enumerate()
            .filter(|(_, b)| (lo..=hi).contains(&b.key_bone))
            .map(|(i, _)| i)
            .collect()
    };
    [roots(8, 12), roots(13, 17)]
}

/// Whether bone `i` is `root` or one of its descendants (a parent-chain walk).
pub(crate) fn in_subtree(skel: &Skeleton, i: usize, root: usize) -> bool {
    let mut cur = i;
    loop {
        if cur == root {
            return true;
        }
        match skel
            .bones
            .get(cur)
            .and_then(|b| usize::try_from(b.parent).ok())
        {
            Some(p) => cur = p,
            None => return false,
        }
    }
}

/// Bake the raw [`M2Attachment`] table into [`ModelAttachment`]s: `offset = wow_to_bevy(position) −
/// pivot_bevy(bone)`, the bone-local offset from the joint's bind position. Records whose `bone`
/// indexes past `pivots` are dropped (defence in depth — `benilla-m2` already range-checks against
/// its own bone array; this guards against the skeleton/attachment parses ever disagreeing).
pub(crate) fn build_attachments(
    attachments: &[M2Attachment],
    pivots: &[Vec3],
) -> Vec<ModelAttachment> {
    attachments
        .iter()
        .filter_map(|a| {
            let pivot = pivots.get(a.bone as usize)?;
            Some(ModelAttachment {
                id: a.id,
                bone: a.bone,
                offset: wow_to_bevy(a.position) - *pivot,
            })
        })
        .collect()
}

/// One M2 animation-event **positional marker**, baked exactly like [`ModelAttachment`] (same
/// bone-local Bevy-space offset convention — a consumer transforms it by the bone joint's live
/// global). The client's by-4CC position queries (`0x7130e0`/`0x7131b0`) read this table first
/// match: the missile launch points `$CSL`/`$CSR`/`$CST` (casting hand) and `$BWR` (ranged
/// release) are the consumers here.
#[derive(Clone, Copy)]
pub struct ModelMarker {
    /// The identifier 4CC, stored forward (`*b"$CSL"`).
    pub ident: [u8; 4],
    pub bone: u16,
    pub offset: Vec3,
}

/// Bake the raw [`benilla_formats::EventMarker`] table into [`ModelMarker`]s — the same
/// `offset = wow_to_bevy(position) − pivot_bevy(bone)` bake and the same defence-in-depth bone
/// range check as [`build_attachments`]. File order is preserved (queries take the first ident
/// match, the client's scan order).
pub(crate) fn build_markers(
    markers: &[benilla_formats::EventMarker],
    pivots: &[Vec3],
) -> Vec<ModelMarker> {
    markers
        .iter()
        .filter_map(|m| {
            let pivot = pivots.get(m.bone as usize)?;
            Some(ModelMarker {
                ident: m.ident,
                bone: m.bone,
                offset: wow_to_bevy(m.position) - *pivot,
            })
        })
        .collect()
}

/// A stable [`AnimationTargetId`] for bone index `bone` (decision 0019). The idle clip's curves and the
/// joint entities' target ids both derive from the bone index (via a synthetic name), so they match
/// without a real `Name` hierarchy. Scoped to one creature's joints + its shared clip.
pub fn bone_target_id(bone: u16) -> AnimationTargetId {
    AnimationTargetId::from_name(&Name::new(format!("benilla_bone_{bone}")))
}

/// The WoW→Bevy basis change as a quaternion. DR 0002's map is a proper rotation (det +1); a bone
/// rotation track is a WoW-space quat, and a rotation transforms under a basis change by **conjugation**
/// `r·q·r⁻¹`, so this `r` is what [`build_idle_clip`] conjugates each rotation keyframe by. Derived from
/// `wow_to_bevy` itself (its images of the basis vectors are `R`'s columns) so it can't drift from it.
fn wow_to_bevy_quat() -> Quat {
    Quat::from_mat3(&Mat3::from_cols(
        wow_to_bevy([1.0, 0.0, 0.0]),
        wow_to_bevy([0.0, 1.0, 0.0]),
        wow_to_bevy([0.0, 0.0, 1.0]),
    ))
}

/// Build a keyframe curve, handling Bevy's ≥2-sample requirement: a single key (a constant channel)
/// becomes a flat curve spanning `[0, duration]`. `None` for no keys (the channel stays at rest).
fn keyframe_curve<T: Animatable + Clone>(
    duration: f32,
    keys: Vec<(f32, T)>,
) -> Option<AnimatableKeyframeCurve<T>> {
    match keys.len() {
        0 => None,
        1 => AnimatableKeyframeCurve::new([
            (0.0, keys[0].1.clone()),
            (duration.max(1e-3), keys[0].1.clone()),
        ])
        .ok(),
        _ => AnimatableKeyframeCurve::new(keys).ok(),
    }
}

/// Build one sequence's [`AnimationClip`] — **and its [`PoseClip`] twin** (decision 0712, one walk
/// so the two cannot drift) — from parsed raw-WoW keyframes (decision 0019), transforming each
/// channel into the joints' Bevy-space `Transform`:
/// - **translation** = the bone's rest local (`pivot − pivot_parent`) **plus** `wow_to_bevy(track)` —
///   the M2 translation track is a delta on the pivot offset;
/// - **rotation** = the WoW quat conjugated into Bevy space (`r·q·r⁻¹`);
/// - **scale** = the WoW scale with axes permuted to Bevy's (magnitudes; near-always uniform anyway).
///
/// Always returns a clip, and a flag for whether any channel produced a **curve**. A sequence
/// whose bones all hold bind pose still gets one — an empty clip carrying the sequence's own
/// duration — because the clip is not only a bone pose: it is this model's carrier for the
/// instance's **sequence clock** (which slot is playing, and how far into it), which the emitter
/// rate/params tracks, the material-alpha loops and the GameObject state arm all resolve through.
/// Dropping the track-less ones left 807 corpus models — the ones whose animation is authored
/// entirely in the *emitter* tracks — with no clock at all, frozen on slot 0 at t=0 for ever
/// (decision 0941). The flag is what the doodad content gate reads instead (`poses_bones`): a clip
/// with no curves can only ever render the bind-pose mesh, so it must not spawn a rig.
pub(crate) fn build_animation_clip(
    anim: &ModelAnimation,
    skeleton: &ModelSkeleton,
) -> (AnimationClip, PoseClip, bool) {
    let r = wow_to_bevy_quat();
    let mut clip = AnimationClip::default();
    let mut pose = PoseClip::default();
    let mut any = false;
    for bk in &anim.bones {
        let target = bone_target_id(bk.bone);
        let rest = skeleton
            .joints
            .get(bk.bone as usize)
            .map_or(Vec3::ZERO, |j| j.local_translation);
        let trans: Vec<_> = bk
            .translation
            .iter()
            .map(|(t, v)| (*t, rest + wow_to_bevy(*v)))
            .collect();
        let pose_trans = PoseTrack::new(&trans);
        if let Some(c) = keyframe_curve(anim.duration, trans) {
            clip.add_curve_to_target(
                target,
                AnimatableCurve::new(animated_field!(Transform::translation), c),
            );
            any = true;
        }
        let rot: Vec<_> = bk
            .rotation
            .iter()
            .map(|(t, q)| {
                (
                    *t,
                    r * Quat::from_xyzw(q[0], q[1], q[2], q[3]) * r.inverse(),
                )
            })
            .collect();
        let pose_rot = PoseTrack::new(&rot);
        if let Some(c) = keyframe_curve(anim.duration, rot) {
            clip.add_curve_to_target(
                target,
                AnimatableCurve::new(animated_field!(Transform::rotation), c),
            );
            any = true;
        }
        let scale: Vec<_> = bk
            .scale
            .iter()
            .map(|(t, s)| (*t, Vec3::new(s[1], s[2], s[0])))
            .collect();
        let pose_scale = PoseTrack::new(&scale);
        if let Some(c) = keyframe_curve(anim.duration, scale) {
            clip.add_curve_to_target(
                target,
                AnimatableCurve::new(animated_field!(Transform::scale), c),
            );
            any = true;
        }
        pose.push(PoseBone {
            bone: bk.bone,
            translation: pose_trans,
            rotation: pose_rot,
            scale: pose_scale,
        });
    }
    if !any {
        // Bevy derives a clip's duration from its curves; with none, the sequence's own band
        // length is the clock's period — without it the player would wrap on a 0-second loop
        // (a modulo by zero) and the clock would never leave t=0, which is the whole defect.
        clip.set_duration(anim.duration.max(1e-3));
    }
    (clip, pose, any)
}

/// Build the weapon-grip **overlay clip** from clamped finger poses ([`benilla_formats::hand_grip_finger_poses`]):
/// a single constant rotation key per finger key-bone, conjugated into Bevy space with the same `r·q·r⁻¹`
/// as [`build_animation_clip`]. Played masked to one hand's finger subtrees, it holds those fingers curled
/// over the gait (rotation only — the fingers don't translate). Empty `poses` → an empty clip (the caller
/// builds no grip node).
pub(crate) fn build_grip_clip(poses: &[(u16, [f32; 4])]) -> (AnimationClip, PoseClip) {
    let r = wow_to_bevy_quat();
    let mut clip = AnimationClip::default();
    let mut pose = PoseClip::default();
    for &(bone, q) in poses {
        let rot = r * Quat::from_xyzw(q[0], q[1], q[2], q[3]) * r.inverse();
        if let Some(c) = keyframe_curve(0.033, vec![(0.0, rot)]) {
            clip.add_curve_to_target(
                bone_target_id(bone),
                AnimatableCurve::new(animated_field!(Transform::rotation), c),
            );
            pose.push(PoseBone {
                bone,
                translation: PoseTrack::default(),
                rotation: PoseTrack::new(&[(0.0, rot)]),
                scale: PoseTrack::default(),
            });
        }
    }
    (clip, pose)
}

/// The [`BillboardInfo`] for a submesh that rides a billboard bone (Bevy space): the pivot to rotate
/// about. `None` for ordinary geometry. Pairs with [`build_submesh_mesh`], which centres the mesh.
///
/// It used to also derive the card's resting plane normal from its first triangle. Nothing has read
/// that since the camera-basis law replaced the facet-normal card (0788's family), so it is deleted
/// rather than left as a field that reads as load-bearing.
pub(crate) fn billboard_info(sub: &RenderSubmesh) -> Option<BillboardInfo> {
    let bb = sub.billboard.as_ref()?;
    Some(BillboardInfo {
        pivot: wow_to_bevy(bb.pivot),
        bone: bb.bone,
        kind: bb.kind,
        scale_anim: bb.scale_anim.clone(),
        // Translation values are model-space offsets — bake each key to Bevy axes here (scale keys
        // above stay raw: per-axis factors, not vectors).
        seq_translations: bb
            .seq_translations
            .iter()
            .map(|(id, a)| {
                let mut a = a.clone();
                for (_, v) in &mut a.keys {
                    *v = wow_to_bevy(*v).to_array();
                }
                (*id, a)
            })
            .collect(),
    })
}

/// A global-sequence bone channel baked to Bevy space: the free-clock loop the runtime samples at
/// `(model_time mod period)` and writes onto the joint. `period` and key times are **seconds**; values
/// are already in the joints' Bevy `Transform` frame. See [`GlobalBone`].
#[derive(Clone)]
pub struct GlobalSeqChannel<T> {
    pub period: f32,
    pub keys: Vec<(f32, T)>,
}

impl<T: Copy> GlobalSeqChannel<T> {
    /// The two keys bracketing `t` (wrapped into `[0, period]`) and the fraction between them. Clamps to
    /// the end keys (WoW authors the first/last key at the loop endpoints, so there is no wrap gap).
    fn bracket(&self, t: f32) -> (T, T, f32) {
        let period = self.period.max(1e-3);
        let t = t.rem_euclid(period);
        let keys = &self.keys;
        if t <= keys[0].0 {
            return (keys[0].1, keys[0].1, 0.0);
        }
        for w in keys.windows(2) {
            if t <= w[1].0 {
                let span = (w[1].0 - w[0].0).max(1e-6);
                return (w[0].1, w[1].1, (t - w[0].0) / span);
            }
        }
        let last = keys[keys.len() - 1].1;
        (last, last, 0.0)
    }
}

impl GlobalSeqChannel<Vec3> {
    pub fn sample(&self, t: f32) -> Vec3 {
        let (a, b, f) = self.bracket(t);
        a.lerp(b, f)
    }
}

impl GlobalSeqChannel<Quat> {
    pub fn sample(&self, t: f32) -> Quat {
        let (a, b, f) = self.bracket(t);
        a.slerp(b, f)
    }
}

/// A bone's global-sequence channels baked to Bevy space (see [`GlobalSeqChannel`]) — any of
/// translation / rotation / scale, each on its own free clock. The runtime writes only the driven
/// components onto the bone's joint entity, leaving the rest to the playing animation.
#[derive(Clone)]
pub struct GlobalBone {
    pub bone: u16,
    pub translation: Option<GlobalSeqChannel<Vec3>>,
    pub rotation: Option<GlobalSeqChannel<Quat>>,
    pub scale: Option<GlobalSeqChannel<Vec3>>,
}

/// Bake the raw [`benilla_formats::GlobalSeqBone`] channels into Bevy-space [`GlobalBone`]s, applying
/// the same per-channel transforms as [`build_animation_clip`]: translation = the bone's rest-local
/// (`pivot − pivot_parent`) plus `wow_to_bevy(track)`; rotation conjugated `r·q·r⁻¹`; scale axis-permuted
/// to Bevy's. Times/periods convert ms → seconds.
pub(crate) fn build_global_bones(
    gseq: &[benilla_formats::GlobalSeqBone],
    skeleton: &ModelSkeleton,
) -> Vec<GlobalBone> {
    let r = wow_to_bevy_quat();
    gseq.iter()
        .map(|g| {
            let rest = skeleton
                .joints
                .get(g.bone as usize)
                .map_or(Vec3::ZERO, |j| j.local_translation);
            let ms = |t: u32| t as f32 / 1000.0;
            GlobalBone {
                bone: g.bone,
                translation: g.translation.as_ref().map(|c| GlobalSeqChannel {
                    period: ms(c.period_ms),
                    keys: c
                        .keys
                        .iter()
                        .map(|(t, v)| (ms(*t), rest + wow_to_bevy(*v)))
                        .collect(),
                }),
                rotation: g.rotation.as_ref().map(|c| GlobalSeqChannel {
                    period: ms(c.period_ms),
                    keys: c
                        .keys
                        .iter()
                        .map(|(t, q)| {
                            (
                                ms(*t),
                                r * Quat::from_xyzw(q[0], q[1], q[2], q[3]) * r.inverse(),
                            )
                        })
                        .collect(),
                }),
                scale: g.scale.as_ref().map(|c| GlobalSeqChannel {
                    period: ms(c.period_ms),
                    keys: c
                        .keys
                        .iter()
                        .map(|(t, s)| (ms(*t), Vec3::new(s[1], s[2], s[0])))
                        .collect(),
                }),
            }
        })
        .collect()
}

/// Build ONE mesh from many placed static submeshes — the transforms baked into the vertices
/// (the production blob build, 1418; the retired `WOW_MEGA_STATIC` bracket's unfaded twin died
/// with it): same attribute recipe as [`submesh_to_static_mesh`], concatenated, with each part's
/// placement transform applied to positions and its rotation to normals (placements scale
/// uniformly, so directions survive), plus the per-vertex [`ATTRIBUTE_WOW_FADE_SPHERE`] (one
/// sphere per PART, replicated onto each of its vertices). `slots` (index-parallel with `parts`
/// when `Some`) additionally bakes each part's SH-probe slot as [`ATTRIBUTE_WOW_MERGED_SLOT`] —
/// the interior-prop lane. Parts lacking vertex colours pad white when any part has them — one
/// attribute set must cover the whole concatenation.
///
/// **The mesh is recentred on its union-AABB centre** and that world centre is returned last:
/// the caller places the entity AT the centre (`Transform::from_translation`), and the returned
/// min/max are mesh-LOCAL for its `Aabb`. A world-baked mesh on `Transform::IDENTITY` sorts the
/// entity at the WORLD ORIGIN in Bevy's transparent phase — ~9 400 yd out, drawn first among
/// ALL transparent content with the twin's depth-write ON, so a feathering blob's translucent
/// pixels depth-killed every transparent entity drawn after it (a per-entity fader behind a
/// 20 %-alpha merged fence vanished outright, popping in only when the sightline cleared the
/// segment — decision 1422). Centre-sort restores the same location-sort semantics every
/// per-entity draw has (whose sort key is its placement origin). The fade spheres stay
/// WORLD-space — the shader compares them against the camera, never against mesh-local
/// positions — so the recentring does not touch them.
pub fn merged_static_mesh_faded(
    parts: &[(std::sync::Arc<RenderSubmesh>, Transform)],
    spheres: &[Vec4],
    slots: Option<&[u32]>,
) -> (Mesh, Vec3, Vec3, Vec3) {
    merged_static_mesh_impl(parts, Some((spheres, slots)))
}

fn merged_static_mesh_impl(
    parts: &[(std::sync::Arc<RenderSubmesh>, Transform)],
    extras: Option<(&[Vec4], Option<&[u32]>)>,
) -> (Mesh, Vec3, Vec3, Vec3) {
    let any_colors = parts
        .iter()
        .any(|(s, _)| s.vertex_colors.len() == s.positions.len());
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut fade: Vec<[f32; 4]> = Vec::new();
    let mut slot_verts: Vec<u32> = Vec::new();
    let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (i, (sub, transform)) in parts.iter().enumerate() {
        let base = u32::try_from(positions.len()).expect("merged mesh under u32 vertices");
        for p in &sub.positions {
            let w = transform.transform_point(wow_to_bevy(*p));
            mn = mn.min(w);
            mx = mx.max(w);
            positions.push(w.to_array());
        }
        if sub.normals.len() == sub.positions.len() {
            let flip = sub.billboard_card_faces_away();
            for n in &sub.normals {
                let b = transform.rotation * wow_to_bevy(*n);
                normals.push(if flip { -b } else { b }.normalize_or_zero().to_array());
            }
        } else {
            // The lone compute_normals fallback path never merges (its models are rare and the
            // bracket tolerates flat shading there): pad up, face normals are close enough for a
            // measurement build.
            normals.extend(std::iter::repeat_n([0.0, 1.0, 0.0], sub.positions.len()));
        }
        uvs.extend(sub.uvs.iter().copied());
        if any_colors {
            if sub.vertex_colors.len() == sub.positions.len() {
                colors.extend(sub.vertex_colors.iter().copied());
            } else {
                colors.extend(std::iter::repeat_n(
                    [1.0, 1.0, 1.0, 1.0],
                    sub.positions.len(),
                ));
            }
        }
        if let Some((spheres, slots)) = extras {
            fade.extend(std::iter::repeat_n(
                spheres[i].to_array(),
                sub.positions.len(),
            ));
            if let Some(slots) = slots {
                slot_verts.extend(std::iter::repeat_n(slots[i], sub.positions.len()));
            }
        }
        indices.extend(sub.indices.iter().map(|i| base + i));
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    if any_colors {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    }
    if let Some((_, slots)) = extras {
        mesh.insert_attribute(ATTRIBUTE_WOW_FADE_SPHERE, fade);
        if slots.is_some() {
            mesh.insert_attribute(
                ATTRIBUTE_WOW_MERGED_SLOT,
                bevy::mesh::VertexAttributeValues::Uint32(slot_verts),
            );
        }
    }
    mesh.insert_indices(Indices::U32(indices));
    // Recentre on the union-AABB centre (see `merged_static_mesh`'s doc): positions become
    // mesh-local, min/max follow, the centre goes back to the caller for the entity Transform.
    // Guard the empty accumulation (no parts) — a MAX/MIN sentinel centre would explode.
    // (`mn > mx` iff the vertex loop never ran; `positions` has been moved into the mesh.)
    let center = if mn.x > mx.x {
        Vec3::ZERO
    } else {
        (mn + mx) * 0.5
    };
    if center != Vec3::ZERO {
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(pos)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
        else {
            unreachable!("positions were just inserted as Float32x3");
        };
        for p in pos.iter_mut() {
            p[0] -= center.x;
            p[1] -= center.y;
            p[2] -= center.z;
        }
    }
    (mesh, mn - center, mx - center, center)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One sequence record, `bones` supplied by the caller — everything else at a neutral value.
    fn sequence(duration: f32, bones: Vec<benilla_formats::BoneKeys>) -> ModelAnimation {
        ModelAnimation {
            anim_id: 0,
            seq_index: 0,
            start_ms: 0,
            end_ms: (duration * 1000.0) as u32,
            duration,
            looping: true,
            move_speed: 0.0,
            blend_time: 0.0,
            bounds_center: [0.0; 3],
            bounds_radius: 0.0,
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
            frequency: 0,
            min_replay: 0,
            max_replay: 0,
            bones,
            events: Vec::new(),
        }
    }

    /// A sequence whose bones hold bind pose still becomes a clip — an EMPTY one carrying the
    /// sequence's own duration — because the clip is this model's carrier for the instance's
    /// **sequence clock** (decision 0941). Dropping it left 807 corpus models whose animation is
    /// authored purely in their emitter tracks with no clock at all: no player, so no slot and no
    /// time, so every per-sequence track read file slot 0 at t = 0 for ever. The duration is the
    /// load-bearing half — Bevy derives a clip's period from its curves, and a 0-second period
    /// would wrap the player's `seek_time` by a modulo-zero and never leave t = 0.
    #[test]
    fn a_boneless_sequence_still_yields_a_clock_clip() {
        let skeleton = ModelSkeleton {
            joints: Vec::new(),
            spine_bone: None,
            head_bone: None,
        };
        let (clip, _pose, poses_bones) =
            build_animation_clip(&sequence(1.333, Vec::new()), &skeleton);
        assert!(!poses_bones, "no bone track ⇒ nothing to skin to");
        assert!(
            (clip.duration() - 1.333).abs() < 1e-4,
            "the clock's period is the sequence's own band, not 0: {}",
            clip.duration()
        );
        // The flag is what the doodad content gate reads: a clock-only clip must never spawn a rig.
        let moving = benilla_formats::BoneKeys {
            bone: 0,
            translation: vec![(0.0, [0.0, 0.0, 0.0]), (0.5, [0.0, 0.0, 1.0])],
            rotation: Vec::new(),
            scale: Vec::new(),
        };
        let (_, _, poses_bones) = build_animation_clip(&sequence(1.0, vec![moving]), &skeleton);
        assert!(poses_bones, "a keyed track ⇒ a real pose");
    }

    /// A [`GlobalSeqChannel`] samples with linear interpolation, clamps at the endpoint keys, and wraps
    /// on its period — the eye-blink shape: `0` (open) held, a fast ramp to `1` (shut), back to `0`, and
    /// the whole thing repeating every `period`. Models the real eyelid keys (ms → seconds).
    #[test]
    fn global_seq_channel_samples_and_wraps() {
        let ch = GlobalSeqChannel {
            period: 6.633,
            keys: vec![
                (0.0, Vec3::ZERO),
                (0.033, Vec3::ONE),
                (0.100, Vec3::ONE),
                (0.133, Vec3::ZERO),
            ],
        };
        assert!(
            ch.sample(0.0).abs_diff_eq(Vec3::ZERO, 1e-4),
            "loop start = open"
        );
        assert!(
            ch.sample(0.06).abs_diff_eq(Vec3::ONE, 1e-4),
            "shut during the blink"
        );
        assert!(
            ch.sample(3.0).abs_diff_eq(Vec3::ZERO, 1e-4),
            "held open for the rest of the loop"
        );
        assert!(
            ch.sample(0.0165).abs_diff_eq(Vec3::splat(0.5), 1e-2),
            "linear ramp"
        );
        assert!(
            ch.sample(6.633 + 0.06).abs_diff_eq(Vec3::ONE, 1e-4),
            "wraps modulo period"
        );
    }

    /// The WoW→Bevy rotation quat must reproduce `wow_to_bevy` on vectors — it's the same rotation, so
    /// `r·v` equals the coordinate map. (Guards against `wow_to_bevy_quat` drifting from `wow_to_bevy`.)
    #[test]
    fn rotation_quat_matches_the_coordinate_map() {
        let r = wow_to_bevy_quat();
        for axis in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            let by_quat = r * Vec3::from_array(axis);
            let by_map = wow_to_bevy(axis);
            assert!(
                by_quat.abs_diff_eq(by_map, 1e-5),
                "r·{axis:?} = {by_quat:?} should equal wow_to_bevy = {by_map:?}"
            );
        }
    }

    /// A bone rotation about WoW **up** (+Z) must conjugate to a rotation about Bevy **up** (+Y) by the
    /// same angle — the load-bearing check for the `r·q·r⁻¹` conjugation + the `[x,y,z,w]` quat order.
    /// (A wrong conjugation/sign/order is exactly the "plausible but twisted creature" failure.)
    #[test]
    fn wow_up_yaw_conjugates_to_bevy_up_yaw() {
        let r = wow_to_bevy_quat();
        let theta = std::f32::consts::FRAC_PI_3; // 60°, an asymmetric angle
        let (s, c) = (theta / 2.0).sin_cos();
        // A WoW quat for a yaw about +Z: (x,y,z,w) = (0,0,sin,cos).
        let q_wow = Quat::from_xyzw(0.0, 0.0, s, c);
        let q_bevy = r * q_wow * r.inverse();
        let expected = Quat::from_axis_angle(Vec3::Y, theta);
        // Quats equal up to sign: |dot| ≈ 1.
        assert!(
            q_bevy.dot(expected).abs() > 0.9999,
            "WoW +Z yaw should map to Bevy +Y yaw: got {q_bevy:?}, want {expected:?}"
        );
    }

    /// Conjugating the identity rotation is the identity (a sanity floor for the conjugation).
    #[test]
    fn identity_rotation_conjugates_to_identity() {
        let r = wow_to_bevy_quat();
        let id = r * Quat::IDENTITY * r.inverse();
        assert!(id.dot(Quat::IDENTITY).abs() > 0.9999, "got {id:?}");
    }

    /// **A welded billboard bone still reaches the palette with its arm** (decision 0945,
    /// superseding 0935's gate) — on the real shipped asset, the director's own pauldron.
    ///
    /// `LShoulder_Plate_PVPAlliance_A_01.m2` authors 3 bones: a plain root and two spherical
    /// billboard spikes whose 8-vertex 50/50 seam rings stitch them to the body. 0935 dropped
    /// their arm on exactly that welding. The animate kernel `0x714260` reads no vertex, weight or
    /// triangle data at all — over its whole extent there is a single `movzx byte ptr`
    /// (`0x7152ea`), the arm-selector map — so no such gate exists in the reference. The seam
    /// survives because the replacement is built about the bone's own pivot, not because the arm
    /// is skipped.
    ///
    /// The predicate itself is still live and still correct where it belongs: the per-batch **card
    /// split** refuses to lift a welded bone's geometry into its own draw (0839). This test pins
    /// that the two lanes now disagree *on purpose* — welded for the card split, faced by the
    /// palette. Skips when the client isn't installed (the repo ships no assets).
    #[test]
    fn a_welded_billboard_bone_still_reaches_the_palette_with_its_arm() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let path = "Item\\ObjectComponents\\Shoulder\\LShoulder_Plate_PVPAlliance_A_01.m2";
        let bytes = chain.read_file(path).expect("read the pauldron");
        let raw = benilla_formats::parse_m2_skeleton(&bytes).expect("parse its skeleton");
        // The asset really does author two spherical arms — otherwise this test would pass on a
        // build that had simply stopped parsing bone flags.
        assert_eq!(
            raw.bones.iter().filter(|b| b.billboard.is_some()).count(),
            2,
            "the M2 authors two billboard bones"
        );
        assert_eq!(
            benilla_formats::non_separable_billboard_bones(&bytes),
            vec![1, 2],
            "both are welded to the body — the card split still refuses to lift them"
        );
        let (built, _) = build_skeleton(&raw);
        assert_eq!(
            built
                .joints
                .iter()
                .filter(|j| j.billboard == Some(benilla_formats::BillboardKind::Spherical))
                .count(),
            2,
            "…and the palette faces them anyway: welding is not a gate on the arm"
        );
    }

    /// **The mount seat's parent arm survives the bake** — the load-bearing case, on the asset
    /// that decides it.
    ///
    /// `RidingHorse.m2` attachment 0 (the rider seat) rides bone 30, `flags = 0x6` = ignore parent
    /// rotation + scale. That is why a galloping horse never rocks its rider: the seat's basis is
    /// replaced by the mount's own root basis before the rider's frame is built from it, so the
    /// spine's ~21° stride swing is discarded at the saddle while the seat still *translates* with
    /// it (the copy-root leg `0x714a18`). Every vanilla player mount is `0x4`, `0x6`, or has a
    /// seat whose ancestors don't rotate.
    #[test]
    fn the_riding_horse_seat_bone_carries_the_root_basis_arm() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Creature\\RidingHorse\\RidingHorse.m2")
            .expect("read the horse");
        let raw = benilla_formats::parse_m2_skeleton(&bytes).expect("parse its skeleton");
        let seat = benilla_formats::parse_m2_attachments(&bytes)
            .expect("parse attachments")
            .into_iter()
            .find(|a| a.id == 0)
            .expect("the horse authors a rider seat");
        assert_eq!(seat.bone, 30, "the seat rides bone 30");
        let (built, _) = build_skeleton(&raw);
        assert_eq!(
            built.joints[usize::from(seat.bone)].parent_arm,
            Some(benilla_formats::ParentArm {
                ignore_translate: false,
                basis: benilla_formats::ParentBasis::RootBasis,
            }),
            "the seat takes the mount's own root basis — the gallop reaches it as translation only"
        );
        // The spine bones it hangs off carry no arm: they are what supplies the discarded swing.
        assert!(
            built.joints[23].parent_arm.is_none() && built.joints[25].parent_arm.is_none(),
            "the swinging spine bones are ordinary"
        );
    }
}
