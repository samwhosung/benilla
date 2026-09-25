//! What the M2 and WMO loaders share: the per-batch submesh and the WoW-to-Bevy bakes of its mesh,
//! skeleton, attachments and animation clips. Materials are built at the spawn site.

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

/// A billboarded batch's card in Bevy space; its mesh is centred on `pivot` (model-local).
#[derive(Clone)]
pub struct BillboardInfo {
    pub pivot: Vec3,
    /// The billboard bone, the joint the card rides on an animated host.
    pub bone: u16,
    pub kind: BillboardKind,
    /// The bone's global-sequence scale loop (a glow card's pulse), folded into the card's scale.
    pub scale_anim: Option<BoneScaleAnim>,
    /// The bone's per-sequence translation loops `(anim id, loop)`, keys in Bevy axes: the quest
    /// marker bobs low on anim 0, and higher on 190 while its unit shows an overhead name.
    pub seq_translations: Vec<(u16, BoneScaleAnim)>,
}

/// One render batch of an M2 or a WMO group: decoded geometry, texture and draw state, but no
/// `Mesh` asset, since a labeled mesh sub-asset uploads the whole model in one frame. The app
/// builds meshes paced, through [`submesh_to_static_mesh`] and [`submesh_to_skinned_mesh`].
#[derive(Clone)]
pub struct ModelSubmesh {
    /// The decoded geometry in WoW model space, the model's one resident CPU copy of its vertices.
    pub geometry: std::sync::Arc<RenderSubmesh>,
    /// The embedded albedo texture; `None` for a creature skin slot, filled at spawn.
    pub texture: Option<Handle<Image>>,
    /// The creature skin variation this batch draws from (`0/1/2` for `Monster1/2/3`).
    pub skin_slot: Option<u8>,
    /// The geoset id, `group * 100 + variant`, by which the character compositor picks parts.
    pub geoset_id: u16,
    /// The character texture slot the spawn site fills from the player's appearance.
    pub char_slot: Option<CharSkinSlot>,
    pub blend: ModelBlend,
    pub two_sided: bool,
    /// A WMO interior group's batch, lit by its baked MOCV with the sun off.
    pub interior: bool,
    /// Unlit, drawn fullbright: M2 render flag `UNLIT` (0x01), or WMO `UNLIT` on an exterior group.
    pub emissive: bool,
    /// The icon slot (M2 texture type 14), filled from `Model:ReplaceIconTexture`'s path.
    pub icon_slot: bool,
    /// The WMO MOMT SIDN night-glow colour, ramped by the night fraction.
    pub sidn: Option<[u8; 3]>,
    /// WMO MOMT WINDOW: an interior batch lit by the Direct/Ambient midpoint.
    pub window: bool,
    /// Blends additively (M2 glow cards and coronae).
    pub additive: bool,
    /// Sphere-mapped: `wow_model.wgsl` derives the UV from the view-space reflection.
    pub env_map: bool,
    /// M2 render flag 0x10; unset, a transparent batch writes depth, as in the reference.
    pub no_depth_write: bool,
    /// M2 render flag 0x08: no depth test.
    pub no_depth_test: bool,
    /// The batch's fog colour policy; `Scene` for WMO.
    pub fog_policy: benilla_formats::FogPolicy,
    /// Set when the batch rides an M2 billboard bone; its mesh is built centred on the pivot.
    pub billboard: Option<BillboardInfo>,
    /// The animated material alpha, colour alpha times transparency weight (`0x707680`).
    pub alpha_anim: Option<std::sync::Arc<benilla_formats::AlphaAnim>>,
    /// The texture transform's translation loop (sampled at `0x714260`, applied at `0x70b740`).
    /// The `Arc` is also the material-dedup identity, shared by every instance.
    pub uv_anim: Option<std::sync::Arc<benilla_formats::UvAnim>>,
    /// The UV loop per file sequence slot, `Some` only where slots disagree; also a dedup identity.
    pub uv_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    /// The texture-transform rotation loop per file sequence slot, read only by the UI model tiles.
    pub uv_rot_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>>>,
    /// The scale loop per file sequence slot, likewise.
    pub uv_scale_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    /// The time-varying M2Color tint; when `Some`, the material carries it, not the vertex colour.
    pub rgb_anim: Option<std::sync::Arc<benilla_formats::RgbAnim>>,
    /// The tint loop per file sequence slot, on [`Self::uv_seq`]'s rule.
    pub rgb_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 3]>>>,
    /// The WMO batch's MOBA section, which picks an interior batch's lighting (INT, TRANS, EXT).
    pub wmo_batch: Option<benilla_formats::WmoBatchClass>,
    /// A flat ground quad (WoW model space), which the ground-fx lane redraws as a projected decal.
    pub ground_quad: Option<benilla_formats::GroundQuad>,
}

/// Bake a [`RenderSubmesh`] to a Bevy-space [`Mesh`]: authored normals, else flat ones.
fn build_submesh_mesh(sub: &RenderSubmesh, usages: RenderAssetUsages) -> Mesh {
    // A billboard batch is centred on its bone pivot, so the spawn site turns it in place.
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
        // Deviation: a card authored facing away from the viewer is lit on the face it presents,
        // because the reference shades it inverted and camera-dependent. Flip its normals, never
        // its winding, which would show the single-sided ones the reference culls at every angle.
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

/// The skinned mesh's per-vertex M2 bone indices, at shader location 10. Not Bevy's
/// `ATTRIBUTE_JOINT_INDEX`, whose presence in a layout forces Bevy's own skinning: this id makes
/// `WowModelExt::specialize` compile the `WOW_RIG_SKIN` path, which reads our own palette.
pub const ATTRIBUTE_WOW_JOINT_INDEX: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new(
        "Wow_JointIndex",
        988_540_917,
        bevy::render::render_resource::VertexFormat::Uint16x4,
    );
/// The skinned mesh's per-vertex joint weights, normalized, at shader location 11.
pub const ATTRIBUTE_WOW_JOINT_WEIGHT: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new(
        "Wow_JointWeight",
        988_540_918,
        bevy::render::render_resource::VertexFormat::Float32x4,
    );

/// A merged blob's per-vertex fade sphere (world centre, fade radius), the same on every vertex
/// of one placement, from which `wow_model.wgsl` fades each placement. Shader location 12.
pub const ATTRIBUTE_WOW_FADE_SPHERE: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new(
        "Wow_FadeSphere",
        988_540_919,
        bevy::render::render_resource::VertexFormat::Float32x4,
    );

/// A merged interior-prop blob's per-vertex SH-probe slot, in place of the `MeshTag` payload.
/// Shader location 13.
pub const ATTRIBUTE_WOW_MERGED_SLOT: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new(
        "Wow_MergedSlot",
        988_540_920,
        bevy::render::render_resource::VertexFormat::Uint32,
    );

/// The static mesh, render-world only. Pair it with an explicit `Aabb`: a render-world mesh races
/// Bevy's `calculate_bounds`, and the exterior cull draws a mesh that has no bound.
pub fn submesh_to_static_mesh(sub: &RenderSubmesh) -> Mesh {
    build_submesh_mesh(sub, RenderAssetUsages::RENDER_WORLD)
}

/// The skinned mesh: the static one plus the joint attributes. It keeps its main-world copy,
/// which the mouseover picker (`target::hover`) skins on the CPU.
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

/// One rest-skeleton joint in Bevy space: its parent (`-1` for the root) and local translation.
#[derive(Clone, Copy)]
pub struct ModelJoint {
    pub parent: i16,
    pub local_translation: Vec3,
    /// The bone's billboard arm (bone flags 0x08/0x10/0x20/0x40). The reference replaces the
    /// bone's rotation in the palette, so its children inherit the facing, and arms every flagged
    /// bone: the animate kernel `0x714260` reads no vertex data, so a welded seam is no gate. The
    /// seam holds because the replacement is built about the bone's own pivot.
    pub billboard: Option<benilla_formats::BillboardKind>,
    /// Bone flags 0x1/0x2/0x4: how the parent matrix is rebuilt from the model's root frame,
    /// applied before the billboard arm.
    pub parent_arm: Option<benilla_formats::ParentArm>,
}

/// A model's rest skeleton in Bevy space, shared by every instance.
#[derive(Clone, Default)]
pub struct ModelSkeleton {
    pub joints: Vec<ModelJoint>,
    /// The KeyBoneID 4 (SpineLow) bone, which counter-twists toward the aim while the body's yaw
    /// is off it, as in a strafe (`0x711f10(4, …)` in `0x607ed0`).
    pub spine_bone: Option<u16>,
    /// The KeyBoneID 6 (Head) bone, which takes the rest of that twist (`0x711f10(6, …)`).
    pub head_bone: Option<u16>,
}

/// Bake a [`Skeleton`] to Bevy space, with its inverse bind poses. A vanilla M2 has no
/// inverse-bind array and an identity rest pose, and a bone's pivot is its bind position, so the
/// inverse bind pose is `translate(−pivot)` and a joint's rest translation `pivot − parent pivot`:
/// at rest every joint matrix is the entity transform.
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
    // A bone carries KeyBoneID `k` iff `keyBoneLookup[k]` points at it.
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

/// Each bone's Bevy-space pivot, the one source for the joints and the attachment offsets.
pub(crate) fn skeleton_pivots(skel: &Skeleton) -> Vec<Vec3> {
    skel.bones.iter().map(|b| wow_to_bevy(b.pivot)).collect()
}

/// One M2 attachment point as an offset from its bone's pivot, Bevy space; about zero on
/// character models, whose attach bones sit on the point.
#[derive(Clone, Copy)]
pub struct ModelAttachment {
    pub id: u16,
    pub bone: u16,
    pub offset: Vec3,
}

/// The right and left arm subtree roots: each hand attachment's bone (1 right, 2 left) walked up
/// to its shoulder key-bone (2 or 3).
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

/// The upper-body subtree root, the reference's `CGUnit+0xd5c` from the probe `0x60ce70`:
/// key-bone 4 (SpineLow), else 6 (Head). SpineLow's subtree leaves out the legs, which hang off
/// the pelvis. `None` means no split, and a one-shot plays full-body.
pub(crate) fn upper_subtree_root(skel: &Skeleton) -> Option<usize> {
    let bone_with = |k: i16| skel.bones.iter().position(|b| b.key_bone == k);
    bone_with(4).or_else(|| bone_with(6))
}

/// The finger key-bones per hand `(right, left)`, 8-12 and 13-17: the reference's `CloseHand`
/// targets, posed with `HandsClosed` (AnimationData 15) while that hand holds a weapon
/// (`0x479660`/`0x60b590`). Empty for a model without them.
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

/// Whether bone `i` is `root` or one of its descendants.
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

/// Bake the [`M2Attachment`] table to bone-local offsets, dropping a record with a bad bone.
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

/// One animation-event position, baked like [`ModelAttachment`]. The reference's by-4CC queries
/// (`0x7130e0`/`0x7131b0`) take the first match: `$CSL`/`$CSR`/`$CST` hands, `$BWR` ranged release.
#[derive(Clone, Copy)]
pub struct ModelMarker {
    /// The identifier 4CC, stored forward (`*b"$CSL"`).
    pub ident: [u8; 4],
    pub bone: u16,
    pub offset: Vec3,
}

/// Bake the event markers like [`build_attachments`], in file order: queries take the first match.
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

/// A bone's stable [`AnimationTargetId`], shared by the clips' curves and the joint entities.
pub fn bone_target_id(bone: u16) -> AnimationTargetId {
    AnimationTargetId::from_name(&Name::new(format!("benilla_bone_{bone}")))
}

/// The WoW-to-Bevy basis change as a quaternion `r`; a rotation key converts as `r·q·r⁻¹`.
fn wow_to_bevy_quat() -> Quat {
    Quat::from_mat3(&Mat3::from_cols(
        wow_to_bevy([1.0, 0.0, 0.0]),
        wow_to_bevy([0.0, 1.0, 0.0]),
        wow_to_bevy([0.0, 0.0, 1.0]),
    ))
}

/// A keyframe curve; one key becomes a flat curve over `[0, duration]`, as Bevy needs two.
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

/// Build one sequence's [`AnimationClip`] and its [`PoseClip`] in one walk. The M2 translation
/// track is a delta on the rest offset, a rotation converts as `r·q·r⁻¹`, and scale permutes axes.
/// A sequence that poses no bone still gets a clip of its own duration, since the clip is the
/// instance's sequence clock; the flag says whether any channel made a curve for a rig to pose.
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
        // Bevy takes a clip's duration from its curves; with none, the clock would never leave 0.
        clip.set_duration(anim.duration.max(1e-3));
    }
    (clip, pose, any)
}

/// The weapon-grip overlay: one constant rotation per finger bone, played masked to one hand.
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

/// The [`BillboardInfo`] of a submesh that rides a billboard bone, in Bevy space.
pub(crate) fn billboard_info(sub: &RenderSubmesh) -> Option<BillboardInfo> {
    let bb = sub.billboard.as_ref()?;
    Some(BillboardInfo {
        pivot: wow_to_bevy(bb.pivot),
        bone: bb.bone,
        kind: bb.kind,
        scale_anim: bb.scale_anim.clone(),
        // Translation keys are vectors and take Bevy axes; the scale keys stay per-axis factors.
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

/// A global-sequence bone channel in Bevy space, a free-running loop; times in seconds.
#[derive(Clone)]
pub struct GlobalSeqChannel<T> {
    pub period: f32,
    pub keys: Vec<(f32, T)>,
}

impl<T: Copy> GlobalSeqChannel<T> {
    /// The keys around `t` mod the period, clamped at the end keys, which sit on the loop's ends.
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

/// A bone's global-sequence channels; only the driven ones override the playing animation.
#[derive(Clone)]
pub struct GlobalBone {
    pub bone: u16,
    pub translation: Option<GlobalSeqChannel<Vec3>>,
    pub rotation: Option<GlobalSeqChannel<Quat>>,
    pub scale: Option<GlobalSeqChannel<Vec3>>,
}

/// Bake the global-sequence channels with [`build_animation_clip`]'s transforms, ms to seconds.
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

/// One mesh from many placed static parts, transforms baked in (normals take only the rotation,
/// as placements scale uniformly), each vertex carrying its part's fade sphere and, with `slots`,
/// its SH-probe slot; missing normals and colours pad (up, white). Returns the mesh, its local
/// bounds and its world centre, where the caller must place the entity: Bevy sorts transparent
/// draws by entity origin, and a mesh left at the world origin draws first and depth-hides the
/// transparent draws behind it. The fade spheres stay world-space.
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
    // Recentre on the union-AABB centre; with no vertices (`mn > mx`) the centre is the origin.
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

    /// One sequence record with the given bones, the rest neutral.
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

    /// A clip with no curves still carries its sequence's duration, or its clock never advances.
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
        // The flag gates the rig: a clock-only clip must never spawn one.
        let moving = benilla_formats::BoneKeys {
            bone: 0,
            translation: vec![(0.0, [0.0, 0.0, 0.0]), (0.5, [0.0, 0.0, 1.0])],
            rotation: Vec::new(),
            scale: Vec::new(),
        };
        let (_, _, poses_bones) = build_animation_clip(&sequence(1.0, vec![moving]), &skeleton);
        assert!(poses_bones, "a keyed track ⇒ a real pose");
    }

    /// The eye-blink shape from a real eyelid track: open, a fast ramp shut, open, repeating.
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

    /// Pins the conjugation and the `[x, y, z, w]` key order.
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

    #[test]
    fn identity_rotation_conjugates_to_identity() {
        let r = wow_to_bevy_quat();
        let id = r * Quat::IDENTITY * r.inverse();
        assert!(id.dot(Quat::IDENTITY).abs() > 0.9999, "got {id:?}");
    }

    /// The shipped pauldron `LShoulder_Plate_PVPAlliance_A_01.m2` welds two spherical billboard
    /// bones to the body with 50/50 seam rings: the card split refuses to lift them, and the
    /// palette faces them anyway, since `0x714260` reads no vertex data.
    #[test]
    fn a_welded_billboard_bone_still_reaches_the_palette_with_its_arm() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let path = "Item\\ObjectComponents\\Shoulder\\LShoulder_Plate_PVPAlliance_A_01.m2";
        let bytes = chain.read_file(path).expect("read the pauldron");
        let raw = benilla_formats::parse_m2_skeleton(&bytes).expect("parse its skeleton");
        // Two authored arms, so the test cannot pass on a build that stopped parsing bone flags.
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

    /// `RidingHorse.m2`'s rider seat (attachment 0) rides bone 30, flags 0x6: the mount's root
    /// basis replaces the seat's, so the gallop's spine swing reaches the rider as translation
    /// only (the copy-root leg `0x714a18`).
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
        // The spine bones above it carry no arm; they supply the swing.
        assert!(
            built.joints[23].parent_arm.is_none() && built.joints[25].parent_arm.is_none(),
            "the swinging spine bones are ordinary"
        );
    }
}
