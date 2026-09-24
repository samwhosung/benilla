//! M2 billboard cards — submeshes that ride a billboard bone (glow cards, chains, the questgiver
//! markers). The real 1.12 client re-orients the bone to the camera every frame; benilla otherwise
//! renders M2 geometry in its static bind pose, single-sided.
//!
//! **The re-orientation law is byte-pinned**: the M2 bone palette is computed in **VIEW space**,
//! and a billboard bone's matrix rows are
//! REPLACED with the camera basis — spherical (`0x08`) takes the whole fixed basis (bone X toward
//! the viewer, Y screen-right, Z screen-up: the identity rows `{(0,0,−1),(1,0,0),(0,1,0)}` at
//! `0x714463`); the lock arms (`0x10`/`0x20`/`0x40` = keep X/Y/Z) keep their authored axis and
//! rebuild the other two from the camera (`0x40` lock-Z — the `?` marker's `0x240` — keeps model
//! up, rebuild the in-plane pair). Crucially this is the **view-matrix basis, one shared
//! orientation for every billboard** — NOT a per-pivot aim, and NOT the geometry's facet normal
//! (the old card aimed its first-triangle normal at the camera: arbitrary for 3-D geometry like
//! the 353-vert `?`, which is exactly why its proportions read wrong). The lock-Z in-plane sign
//! is `Y = Fwd × Z` — the 0168 handedness residual, settled by the director's A/B (the recorded
//! `Z × Fwd` order turned the model 180°: a mirrored `?`); it makes lock-Z agree with the
//! spherical arm's toward-the-viewer X at a level camera.
//!
//! The submesh mesh is built **centred at its bone pivot** (`benilla_assets::build_submesh_mesh`)
//! in the model-local Bevy frame — where the WoW bone axes land as X→−Z, Y→−X, Z→+Y (coords.rs) —
//! so we place the entity at the pivot's world position and write the rebuilt basis as its
//! rotation each frame; the geometry itself is never touched.

use benilla_assets::BillboardInfo;
use benilla_formats::{BillboardKind, BoneScaleAnim};
use bevy::math::{Affine3A, Mat3A, Vec3A};
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::view::WorldCamera;

/// A spawned billboard card: where its pivot sits in the world, its per-axis scale, how
/// it tracks the camera (the bone-flag arm), and its optional global-sequence scale pulse. The
/// per-frame system rewrites the entity transform from these — including `Visibility` (the
/// hidden-owner mirror), so a card requires it rather than trusting every spawn site's `Mesh3d`
/// to bring it along.
///
/// It requires a [`MeshTag`] for the same reason: a card is a world ROOT, so every per-model alpha
/// that reaches an ordinary submesh by descending the model's tree has to reach a card through its
/// own tag instead (`player::apply_self_model_fade` — the zoom-to-first-person feather). Only the
/// alpha-animated spawn sites used to bring one, which left the channel *incidentally* present;
/// the default `MeshTag(0)` is the shader's untagged-⇒-opaque sentinel, so requiring it changes
/// nothing about how a card draws.
#[derive(Component)]
#[require(Transform, Visibility, MeshTag)]
pub struct BillboardCard {
    world_pivot: Vec3,
    /// The card's scale in its OWN (pre-billboard) frame — the joint's/placement's per-axis scale,
    /// applied before the camera basis, exactly as [`billboard_joint_palette`] applies it to a
    /// billboard JOINT (`Transform { rotation: camera_basis, scale, .. }` = `T·R_cam·S`). It is a
    /// `Vec3` and not a scalar because real content animates a billboard bone **non-uniformly**:
    /// the Lightwell's shaft (`World\Goober\G_HolyLightWell.m2`, bone 0, lock-Z) holds
    /// `(8.808, 8.808, 37.560)` through its whole Stand loop, a 0.12 yd card stretched into a
    /// 4.5 yd column of light. Collapsing that to `scale.x` and splatting it — which every lane
    /// did until 2026-08-25 — rendered the shaft 1.06 yd tall instead: a squat, blown-out card
    /// sitting on the well's own bowl and drowning it (bug B169, "Lightwell renders very buggy").
    scale: Vec3,
    kind: BillboardKind,
    /// The billboard bone's looping scale animation (the lamppost glow "breathe"), sampled each frame
    /// and multiplied into [`Self::scale`]. `None` for a static card (no global-sequence scale track).
    scale_anim: Option<BoneScaleAnim>,
    /// The armed-sequence cursor offset (ms, negated so a wrapping ADD subtracts): sampling the
    /// [`Self::seq_translation`] loop runs on `elapsed − arm_ms` — the reference's per-play
    /// phase for SEQUENCE tracks (`cursor = clock − startOffset`, re-baked at every arm). `0`
    /// until [`Self::arm_seq_translation`] arms a loop.
    arm_neg_ms: u32,
    /// The gseq [`Self::scale_anim`]'s ATTACH anchor (ms): `None` until the first placement pass
    /// stamps it — the reference snapshots the scene clock once per model instance at attach
    /// (`CM2Model+0x68`; decision 0856), so a row of lampposts streamed
    /// in on different frames breathes at per-instance phases, while same-frame spawns share
    /// one. Distinct from [`Self::arm_neg_ms`]: the gseq anchor is stamped once per instance,
    /// the sequence cursor re-arms per play. (An earlier position-hash de-sync here emulated
    /// the per-instance spread with an invented mechanism; 0855 briefly removed phase entirely —
    /// both superseded by the byte law.)
    gseq_attach_ms: Option<u32>,
    /// The bone's armed first-sequence **translation** loop (the questgiver `?` marker's bob, keys in
    /// Bevy axes) — sampled each frame on the same clock/phase and added at the pivot, rotated by
    /// [`Self::placement_rot`]. `None` (every doodad card today) = the static pivot; only the marker
    /// spawn site arms it via [`Self::with_seq_translation`] — the doodad half of that ride belongs
    /// to the 0130 phase-4 bone-follow work.
    seq_translation: Option<BoneScaleAnim>,
    /// The placement's rotation, so the bob offset (model-local) points where the instance points.
    placement_rot: Quat,
    /// The entity this card FOLLOWS (a unit/GameObject anchor or held-item root): the facing system
    /// re-seats the card from its live `GlobalTransform` every frame and despawns the card when it
    /// goes — the ONE mechanism for every non-doodad spawn path (braziers, held torches, missiles),
    /// so a glow card can never again render at the model origin because a spawn site forgot the
    /// pivot (the recurring "glow on the ground" family — decision 0153). `None` = fixed placement
    /// (terrain doodads, whose transform never moves).
    follow: Option<Entity>,
    /// The pivot in the model's local Bevy frame (re-applied each frame when `follow` is set).
    local_pivot: Vec3,
}

impl BillboardCard {
    /// Build a card from a submesh's [`BillboardInfo`] and its instance `placement`. The pivot is placed
    /// in the world; the card's orientation ignores the placement rotation (a billboard faces the camera
    /// regardless of how the prop is turned).
    pub fn new(info: &BillboardInfo, placement: Transform) -> Self {
        let world_pivot = placement.transform_point(info.pivot);
        Self {
            world_pivot,
            scale: placement.scale,
            kind: info.kind,
            scale_anim: info.scale_anim.clone(),
            arm_neg_ms: 0,
            gseq_attach_ms: None,
            seq_translation: None,
            placement_rot: placement.rotation,
            follow: None,
            local_pivot: info.pivot,
        }
    }

    /// Build a card that FOLLOWS `owner` — the entity-path form (creatures, GameObjects, held
    /// items, missiles, spell effects): world pivot/scale/rotation are re-derived from the owner's
    /// live `GlobalTransform` every frame, and the card despawns when the owner goes.
    pub fn following(info: &BillboardInfo, owner: Entity) -> Self {
        let mut card = Self::new(info, Transform::IDENTITY);
        card.follow = Some(owner);
        card
    }

    /// Build a card riding a live JOINT — an animated host's billboard bone (the swinging lamp,
    /// the mount's lights). The joint's frame already bakes the bone pivot (the 0130 rig identity
    /// `joint = root · M_bone · T(pivot)`), so the card's local pivot is the joint origin.
    ///
    /// The joint also already carries the bone's global-sequence scale — every joint/anchor lane
    /// runs a [`crate::rig_anim::GlobalSeqDrive`] over the same bone list, and `re_place`
    /// reads the composed result back as [`Self::scale`] — so the card must NOT sample its own
    /// copy of the track. Keeping it multiplied the twinkle in twice, at two different clocks
    /// (the drive's spawn clock × the card's position-hash phase): squared peaks at random
    /// alignment — Arcane Intellect's sometimes-2.5-yd lens flare (decision 0851). The sampler
    /// stays on the rigless lanes ([`Self::new`]/[`Self::following`]), where the card is the only
    /// thing animating.
    pub fn following_joint(info: &BillboardInfo, joint: Entity) -> Self {
        let mut card = Self::following(info, joint);
        card.local_pivot = Vec3::ZERO;
        card.scale_anim = None;
        card
    }

    /// A card with **no geometry** — a pure billboard *frame*: an entity whose live transform is a
    /// billboard bone's replaced palette matrix (pivot in the world, camera basis as the rotation),
    /// for the other consumers of that matrix to ride. Today's one caller is the equipped-item
    /// emitter lane (`entities::equipment::spawn`): an item model spawns no rig, so nothing else
    /// would apply the replacement to a particle emitter hanging under a billboard bone — and the
    /// reference folds the emitter's record position through exactly this matrix
    /// (`0x7190a9`–`0x71910c`).
    ///
    /// It is the same mechanism as a card and deliberately not a second one (decision 0153's rule):
    /// same basis, same pivot law, same follow/despawn contract, one system. A card without a
    /// `Mesh3d` simply draws nothing while its transform is maintained.
    ///
    /// `pivot` is model-local **Bevy** axes (the frame `owner`'s children live in).
    pub fn frame_following(kind: BillboardKind, pivot: Vec3, owner: Entity) -> Self {
        Self {
            world_pivot: Vec3::ZERO, // re-seated from the owner before the first facing write
            scale: Vec3::ONE,
            kind,
            scale_anim: None,
            arm_neg_ms: 0,
            gseq_attach_ms: None,
            seq_translation: None,
            placement_rot: Quat::IDENTITY,
            follow: Some(owner),
            local_pivot: pivot,
        }
    }

    /// Arm the card's first-sequence translation loop (the questgiver `?` bob) with the client's
    /// arm-time cursor: sampling runs on `elapsed − arm_ms` (the loop starts at its first key the
    /// moment the marker attaches, like the real arm at status receive).
    pub fn with_seq_translation(mut self, anim: Option<BoneScaleAnim>, arm_ms: u32) -> Self {
        self.arm_seq_translation(anim, arm_ms);
        self
    }

    /// Re-arm the translation loop on a LIVE card — the marker swapping between its low (anim 0)
    /// and raised (anim 190) bob when the unit's overhead name toggles: fresh cursor, same law as
    /// [`Self::with_seq_translation`].
    pub fn arm_seq_translation(&mut self, anim: Option<BoneScaleAnim>, arm_ms: u32) {
        if anim.is_some() {
            self.arm_neg_ms = arm_ms.wrapping_neg();
        }
        self.seq_translation = anim;
    }

    /// The entity this card follows, if any — the anchor/joint that decides both where it sits and
    /// which model it BELONGS to. A card is a world root, so a system that walks a model's tree
    /// (the self-avatar fade; the light node's shade push in `entity_shade`) can only recognise the
    /// model's own cards by testing this against the entities it walked. `None` = a fixed terrain
    /// doodad's card, whose shade rides its material selector instead.
    ///
    /// It is also the entity whose `InheritedVisibility` decides whether an entity-lane card draws
    /// at all — a world root inherits nothing, so its model's hide has to be *read*, by the one
    /// `Visibility` authority (`model_render::visibility`, decision 1409).
    pub fn follows(&self) -> Option<Entity> {
        self.follow
    }

    /// Re-seat a card that FOLLOWS something (the questgiver `!`/`?` markers over a unit that can
    /// move) — recompute the world pivot/scale/rotation from a fresh `placement`, keeping the card's
    /// orientation kind, rest normal, and animation phase. Doodad cards never need this (their
    /// placement is fixed at spawn).
    pub fn re_place(&mut self, placement: Transform, local_pivot: Vec3) {
        self.world_pivot = placement.transform_point(local_pivot);
        self.scale = placement.scale;
        self.placement_rot = placement.rotation;
    }
}

/// A bone's rewritten **effective parent matrix** — the `flags & 0x7` arm the reference takes at
/// `0x71496d`–`0x714d0c`, before the billboard selector and before the bone's own TRS
/// composes onto it. This is not an
/// escape hatch from the billboard: it changes the input the billboard law is applied to, and the
/// `&0x78` switch runs afterwards exactly as before.
///
/// - `parent` — the bone's ANIMATED parent world matrix (`palette[parent_bone]` before the rewrite).
/// - `root` — the model's own root frame, `world_from_model` (`[model+0xfc]`): for a rigged host
///   that is its `joints_root`, and for a mounted rider that is its seat anchor, which is exactly
///   what the reference composes at `0x714389`.
/// - `pivot` — the bone's BIND local translation (`pivot_i − pivot_parent`). Our joint chain is
///   pivot-relative, so this plays the role of the byte law's model-space `piv` in the
///   pivot-preserving tail `T' = pivotWorld − piv·newBasis`: the bone keeps the POSITION its
///   animated parent carried it to and loses only the orientation. That is the whole reason a
///   galloping mount still carries its rider up and down while never rocking them.
///
/// The reference is row-major with row vectors, so its "row K" is our column K — both name the
/// image of model basis vector K, and the WoW→Bevy bake is a signed permutation of those axes, so
/// the per-axis legs below pair the same axes the bytes do.
pub(crate) fn parent_arm_matrix(
    arm: benilla_formats::ParentArm,
    parent: Affine3A,
    root: Affine3A,
    pivot: Vec3,
) -> Affine3A {
    use benilla_formats::ParentBasis;
    // `0x714bdb`'s unit guard: a degenerate axis is left alone rather than exploding.
    const UNIT_EPS: f32 = 1.0 / (1 << 22) as f32;
    // `0x80c5c8`, the ratio leg's own constant — deliberately NOT the unit eps above.
    const RATIO_EPS: f32 = 1e-5;
    let (p, r) = (parent.matrix3, root.matrix3);
    let per_axis = |f: &dyn Fn(usize) -> Vec3A| Mat3A::from_cols(f(0), f(1), f(2));
    let matrix3 = match arm.basis {
        ParentBasis::Keep => p,
        // `flags & 6 == 2` — ignore parent scale: unit-length axes, directions kept.
        ParentBasis::UnitNormalize => per_axis(&|k| {
            let len = p.col(k).length();
            if len > UNIT_EPS {
                p.col(k) / len
            } else {
                p.col(k)
            }
        }),
        // `flags & 6 == 4` — ignore parent rotation: the ROOT's direction at the PARENT's
        // magnitude, per axis.
        ParentBasis::RootDirection => per_axis(&|k| {
            let rl2 = r.col(k).length_squared();
            let ratio = if rl2 <= RATIO_EPS {
                1.0
            } else {
                p.col(k).length() / rl2.sqrt()
            };
            r.col(k) * ratio
        }),
        // `flags & 6 == 6` — ignore parent rotation AND scale: the root basis outright.
        ParentBasis::RootBasis => r,
    };
    Affine3A {
        matrix3,
        // `flags & 1` (`0x714c92`) takes the root's own origin; otherwise the pivot-preserving
        // tail at `0x714caf`.
        translation: if arm.ignore_translate {
            root.translation
        } else {
            parent.transform_point3a(pivot.into()) - matrix3 * Vec3A::from(pivot)
        },
    }
}

/// The rebuilt orientation for a billboard of `kind` — the byte law (module doc), one function for
/// both consumers: the CARD path (`kept_rot` = the placement/owner rotation) and the JOINT palette
/// pass below (`kept_rot` = the joint's fully-composed pre-billboard world rotation). `bx/by/bz`
/// are the bone's WoW-frame X/Y/Z axes as world directions after the replacement; the returned quat
/// maps the mesh's model-local Bevy frame onto them (WoW axes sit in that frame as X→−Z, Y→−X, Z→+Y
/// — coords.rs — so local X→−by, Y→bz, Z→−bx).
pub fn billboard_basis(
    kind: BillboardKind,
    kept_rot: Quat,
    fwd: Vec3,
    right: Vec3,
    up: Vec3,
) -> Quat {
    let (bx, by, bz) = match kind {
        // Spherical (`0x08`): the whole fixed basis — X toward the viewer, Y screen-right,
        // Z screen-up (the view-space identity rows).
        BillboardKind::Spherical => (-fwd, right, up),
        // Lock-Z (`0x40` — the `?` marker, the frost-armor sheets): keep the authored bone Z
        // (model up, pointed by `kept_rot`), rebuild the in-plane pair from the camera. The
        // in-plane sign is `Y = Fwd × Z` — the 0168 residual, settled by the director's A/B
        // (the other order showed the model's back: a mirrored `?`); this order also agrees
        // with the spherical arm at a level camera (X toward the viewer, Y screen-right), the
        // coherence the flipped version lacked. A camera looking straight along the kept axis
        // degenerates the cross — hold screen-right then.
        BillboardKind::LockZ => {
            let bz = (kept_rot * Vec3::Y).normalize_or(Vec3::Y);
            let by = fwd.cross(bz).try_normalize().unwrap_or(right);
            let bx = by.cross(bz);
            (bx, by, bz)
        }
        // Lock-X/-Y: the same verified structure generalized per kept axis — the cyclically
        // PREVIOUS axis takes `Fwd × kept` (that assignment is what reproduces the settled
        // lock-Z arm), the third completes the right-handed WoW triple. No shipped content has
        // A/B'd these two arms yet; if a chain/rope ever reads mirrored, the sign here is the
        // one knob (0168's pattern).
        BillboardKind::LockX => {
            let bx = (kept_rot * -Vec3::Z).normalize_or(-fwd);
            let bz = fwd.cross(bx).try_normalize().unwrap_or(up);
            let by = bz.cross(bx);
            (bx, by, bz)
        }
        BillboardKind::LockY => {
            let by = (kept_rot * -Vec3::X).normalize_or(right);
            let bx = fwd.cross(by).try_normalize().unwrap_or(-fwd);
            let bz = bx.cross(by);
            (bx, by, bz)
        }
    };
    Quat::from_mat3(&Mat3::from_cols(-by, bz, -bx))
}

/// A rigged host whose skeleton authors billboard bones (component beside the rig's
/// `AnimationPlayer`): the joint entities in bone order, each bone's parent, and which joints
/// billboard. [`billboard_joint_palette`] rewrites those joints' propagated world rotations to
/// the camera basis every frame — the byte law operates on the BONE PALETTE, where children
/// multiply onto the replaced parent matrix (`0x7151ba`), so geometry
/// skinned to a billboard bone's CHILDREN inherits the facing. The per-batch card split can
/// never catch that case: the frost-armor sheets skin every vertex to the scale-in CHILD of the
/// lock-Z bone, which is exactly why they rendered glued to the character.
#[derive(Component)]
pub struct BillboardJointRig {
    /// The host root entity — the model's own root frame `[model+0xfc]`, which the `flags & 0x7`
    /// arm rebuilds a bone's parent matrix out of.
    root: Entity,
    joints: Vec<Entity>,
    parents: Vec<i16>,
    kinds: Vec<Option<BillboardKind>>,
    /// Bone flags `0x1/0x2/0x4` per joint ([`parent_arm_matrix`]) — the HandArrow/Bullet attach
    /// helpers (the nocked arrow lies flat along the facing instead of twisting with the draw
    /// hand) and every vanilla mount's rider seat.
    arms: Vec<Option<benilla_formats::ParentArm>>,
    /// Each bone's BIND local translation — the pivot the arm preserves. `locals` carry the
    /// ANIMATED translation, which the byte law rotates by the *new* basis, so the two are not
    /// interchangeable here.
    binds: Vec<Vec3>,
}

impl BillboardJointRig {
    /// The host root — the collapsed-rig world pass's do-not-enter set reads it (a nested rig
    /// with its own billboard output owns its interior, whichever lane the outer rig is on).
    pub(crate) fn root(&self) -> Entity {
        self.root
    }

    /// Build for a spawned rig — `None` when the skeleton authors neither a billboard bone nor a
    /// `flags & 0x7` bone (the common case: ordinary rigs cost nothing). `root` is the host entity
    /// the joints hang under (the model-space frame).
    pub fn new(
        skeleton: &benilla_assets::ModelSkeleton,
        joints: &[Entity],
        root: Entity,
    ) -> Option<Self> {
        if skeleton
            .joints
            .iter()
            .all(|j| j.billboard.is_none() && j.parent_arm.is_none())
        {
            return None;
        }
        Some(Self {
            root,
            joints: joints.to_vec(),
            parents: skeleton.joints.iter().map(|j| j.parent).collect(),
            kinds: skeleton.joints.iter().map(|j| j.billboard).collect(),
            arms: skeleton.joints.iter().map(|j| j.parent_arm).collect(),
            binds: skeleton
                .joints
                .iter()
                .map(|j| j.local_translation)
                .collect(),
        })
    }
}

/// The palette half of the billboard law: for each rigged host, replace every billboard joint's
/// world rotation with the camera basis (scale and pivot translation preserved — the reference's
/// default tail `0x715868`, which in our rig identity is simply "keep the joint's global
/// scale/translation"), then re-compose every descendant joint from its local TRS so skinned
/// geometry — and emitters/ribbons riding those joints — inherit the facing. Runs after propagation
/// and writes `GlobalTransform` directly (the same exactness argument as [`face_billboards`], which
/// must run after this so following-joint cards read the replaced frames). **Every palette consumer
/// must read AFTER this system, same frame**: avian's physics sync re-propagates the hierarchy from
/// locals inside the fixed loop, so an Update-time read gets the UN-billboarded pose — the Demon
/// Skin flames followed the character's yaw instead of the camera until the particle/ribbon sims
/// moved behind this pass. Bone order is parent-sorted in every real M2 (the format guarantees
/// parent < child); a malformed child whose parent follows it just keeps its propagated pose.
///
/// A rigged model can hang under ANOTHER rig's joint — a spell-effect instance on a unit's
/// attach-helper bone, a rigged held item in a hand. One ownership law keeps the passes from
/// fighting over those frames: the child-recompose walk **never enters a nested rig's subtree**
/// — not even its root, whose propagated global (the live ANIMATED attach-bone frame) is what
/// its emitters' attach frame must read. Without it, the boar's flag-0x04 attach helper
/// re-composed the Eviscerate impact model's frames from raw locals, erasing its camera-born
/// billboard basis or its animated attach rotation depending on per-launch query order — the
/// burst rendered as a body-locked pillar on some launches and correctly on others. With no rig
/// ever writing into another rig's subtree, the passes are order-independent again.
pub fn billboard_joint_palette(
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    hosts: Query<&BillboardJointRig>,
    // A parked unit's pose is frozen off-frustum (decision 0448) — camera-facing its glow joints
    // would re-dirty the subtree for a rig no one sees. A parked host still sits in the
    // do-not-enter set (its propagated frames are real); it just isn't re-faced.
    parked: Query<Has<crate::rig_anim::AnimParked>>,
    mut joints: Query<(&Transform, &mut GlobalTransform), Without<WorldCamera>>,
    children: Query<&Children>,
) {
    let Ok(cam_tf) = cam.single() else {
        return;
    };
    let (fwd, right, up) = (*cam_tf.forward(), *cam_tf.right(), *cam_tf.up());
    // Every rig root — the walk's do-not-enter set.
    let rig_roots: bevy::platform::collections::HashSet<Entity> =
        hosts.iter().map(|r| r.root).collect();
    for rig in hosts
        .iter()
        .filter(|r| !parked.get(r.root).unwrap_or(false))
    {
        // The model's own root frame `[model+0xfc]` — what the `flags & 0x7` arm rebuilds a
        // parent matrix out of. Read before the joint loop (the root is never a joint).
        let root_affine = joints
            .get(rig.root)
            .map(|(_, g)| g.affine())
            .unwrap_or_default();
        let n = rig.joints.len();
        let mut replaced: Vec<Option<GlobalTransform>> = vec![None; n];
        for i in 0..n {
            let pidx = usize::try_from(rig.parents[i]).ok().filter(|&p| p < i);
            let parent_new = pidx.and_then(|p| replaced[p]);
            let arm = rig.arms[i];
            if parent_new.is_none() && rig.kinds[i].is_none() && arm.is_none() {
                continue; // untouched subtree — the propagated pose stands
            }
            // The arm needs the PARENT's matrix in hand; propagation only left us the child's, so
            // an armed bone whose parent was untouched reads the parent's propagated frame here.
            let parent_world = if arm.is_some() {
                parent_new.or_else(|| {
                    let e = pidx.map_or(rig.root, |p| rig.joints[p]);
                    joints.get(e).ok().map(|(_, g)| *g)
                })
            } else {
                parent_new
            };
            let Ok((local, mut global)) = joints.get_mut(rig.joints[i]) else {
                continue;
            };
            let mut g = match (arm, parent_world) {
                // `flags & 0x7`: rewrite the parent matrix, then compose this bone's own TRS onto
                // it — and fall through to the billboard switch, which still runs.
                (Some(a), Some(pw)) => GlobalTransform::from(parent_arm_matrix(
                    a,
                    pw.affine(),
                    root_affine,
                    rig.binds[i],
                ))
                .mul_transform(*local),
                (_, Some(pw)) => pw.mul_transform(*local),
                (_, None) => *global,
            };
            if let Some(kind) = rig.kinds[i] {
                let (scale, rot, translation) = g.to_scale_rotation_translation();
                g = GlobalTransform::from(Transform {
                    translation,
                    rotation: billboard_basis(kind, rot, fwd, right, up),
                    scale,
                });
            }
            replaced[i] = Some(g);
            *global = g;
        }
        // Rigid children hanging under a rewritten joint (a held item, the nocked arrow) got
        // their globals from ordinary propagation — BEFORE this rewrite. Re-compose those
        // subtrees from the replaced frames; sibling JOINTS are excluded (the replaced-chain
        // above owns them). Skinned geometry never needs this — it reads the joint frames.
        let joint_set: bevy::platform::collections::HashSet<Entity> =
            rig.joints.iter().copied().collect();
        let mut stack: Vec<(Entity, GlobalTransform)> = Vec::new();
        for (&joint, g) in rig
            .joints
            .iter()
            .zip(&replaced)
            .filter_map(|(j, r)| r.map(|g| (j, g)))
        {
            if let Ok(cs) = children.get(joint) {
                stack.extend(
                    cs.iter()
                        .filter(|c| !joint_set.contains(c) && !rig_roots.contains(c))
                        .map(|c| (c, g)),
                );
            }
        }
        while let Some((e, parent_g)) = stack.pop() {
            let Ok((local, mut global)) = joints.get_mut(e) else {
                continue;
            };
            let g = parent_g.mul_transform(*local);
            *global = g;
            if let Ok(cs) = children.get(e) {
                stack.extend(cs.iter().filter(|c| !rig_roots.contains(c)).map(|c| (c, g)));
            }
        }
    }
}

/// The billboard placement pass (PostUpdate, after `TransformSystems::Propagate`, before
/// visibility) — [`billboard_joint_palette`] then [`face_billboards`] run here, and upstream
/// card re-seaters (the quest markers) order `.before` it. Placement reads the SAME-frame
/// propagated pose: running in Update read last-frame joint/owner globals, so a card over a
/// moving unit trailed a frame behind and snapped forward on stop (the nameplate lag's sibling).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BillboardPlace;

/// Per-frame: face each billboard card to the camera (around its pivot) and apply its scale pulse.
/// A FOLLOWING card (entity-path) is first re-seated from its owner's live global transform — and
/// despawned when the owner is gone (streamed out, unequipped, died). Runs in [`BillboardPlace`]
/// (post-propagation), so it writes `GlobalTransform` directly alongside `Transform` — cards
/// write ABSOLUTE world transforms and live at the root/identity, so the direct write is exact.
///
/// It does NOT own a card's `Visibility` — [`crate::model_render::visibility`] does, for every
/// card as for every other `ModelPart` (decision 0025). This system used to mirror a hidden owner
/// here as well, and that write was **dead**: [`BillboardPlace`] is only ordered
/// `before(CheckVisibility)`, not before `VisibilityPropagate`, so the `Hidden` it wrote landed
/// after the propagation that would have consumed it and was overwritten in the next frame's
/// `Update` by the authority — for a year, a GameObject's glow card went on burning over its own
/// culled model (decision 1409).
#[allow(clippy::type_complexity)] // the card tuple, commented inline
pub(crate) fn face_billboards(
    mut commands: Commands,
    time: Res<Time>,
    cam: Query<(Ref<GlobalTransform>, Option<Ref<Transform>>), With<WorldCamera>>,
    owners: Query<Ref<GlobalTransform>, (Without<WorldCamera>, Without<BillboardCard>)>,
    mut cards: Query<
        (
            Entity,
            &mut BillboardCard,
            &mut Transform,
            &mut GlobalTransform,
            // Read for the trace probe below (the tag's low bits are the card's render alpha).
            Option<&bevy::mesh::MeshTag>,
        ),
        Without<WorldCamera>,
    >,
) {
    let Ok((cam_tf, cam_local)) = cam.single() else {
        return;
    };
    // The camera basis — the VIEW-MATRIX axes the byte law substitutes (one shared orientation
    // for every billboard; never a per-pivot aim).
    // The propagated frame OR the seat's own write: a teleport frame moves the local before
    // propagation runs (see doodad_anim's gate), and the old read carried a pre-snap placement
    // across the snap.
    let cam_moved = cam_tf.is_changed() || cam_local.as_ref().is_some_and(|l| l.is_changed());
    let (fwd, right, up) = (*cam_tf.forward(), *cam_tf.right(), *cam_tf.up());
    let elapsed_ms = time.elapsed().as_millis() as u32;
    for (entity, mut card, mut tf, mut global, tag) in &mut cards {
        // A card with no track is a pure function of the camera basis and its owner's frame:
        // when neither moved this frame, the placement below would come out bit-identical
        // (the write guards at the end already knew that; the recompute did not — 1979's
        // floor, every resident card re-placed on a still frame).
        // …and a card that has never been placed (the first-pass stamp is what says so): it
        // spawned unrotated at scale one, and a still frame must not leave it that way
        // (review of 2026-09-04 — a tile bursting in under a parked camera).
        let tracked = card.scale_anim.is_some()
            || card.seq_translation.is_some()
            || card.gseq_attach_ms.is_none();
        if let Some(owner) = card.follow {
            match owners.get(owner) {
                Ok(gt) => {
                    if !tracked && !cam_moved && !gt.is_changed() {
                        continue;
                    }
                    let pivot = card.local_pivot;
                    card.re_place(gt.compute_transform(), pivot);
                    // The card-provenance trace (`WOW_MOVE_TRACE`, ~2 Hz): where each following
                    // card sits, how big it renders, and at what alpha. An invisible spell-fx
                    // card has exactly three possible causes — placement, scale, alpha — and a
                    // screenshot cannot tell them apart; this line does (it split Arcane
                    // Intellect's "missing" stars into scale ✓ / place ✓ / alpha ×0.3 — the
                    // faithful stealth-aura compose, not a defect).
                    // `enabled_for`, not `enabled`: this is by far the busiest tag in the file
                    // (thousands of lines a second in a populated scene, each an unbuffered write
                    // under the shared mutex), so it is the one most worth dropping cheaply when the
                    // run is asking a movement question — `WOW_MOVE_TRACE_TAGS`, decision 0880.
                    if benilla_assets::trace::enabled_for("card") && elapsed_ms % 512 < 20 {
                        benilla_assets::trace::line(
                            "card",
                            &format!(
                                "card={entity} owner={owner} scale={:.3?} a={:.2} pivot={:.2?}",
                                card.scale,
                                tag.map_or(-1.0, |t| crate::mesh_tag::alpha_of(t.0)),
                                card.world_pivot
                            ),
                        );
                    }
                }
                Err(_) => {
                    commands.entity(entity).despawn();
                    continue;
                }
            }
        } else if !tracked && !cam_moved {
            continue;
        }
        // The gseq attach anchor: stamped on the card's first placement pass — the reference's
        // once-per-instance scene-clock snapshot (decision 0856).
        let attach_ms = *card.gseq_attach_ms.get_or_insert(elapsed_ms);
        let card = &*card;
        let rotation = billboard_basis(card.kind, card.placement_rot, fwd, right, up);
        // The bone's global-sequence scale pulse (the lamppost glow "breathe"), on the instance's
        // anchored cursor (`sceneNow − attach`): instances stamped on different frames pulse at
        // per-instance phases, same-frame spawns in phase — the reference's anchor law.
        // `Vec3::ONE` (no-op) when the card has no scale track.
        let pulse = card.scale_anim.as_ref().map_or(Vec3::ONE, |a| {
            Vec3::from_array(a.sample(elapsed_ms.wrapping_sub(attach_ms)))
        });
        // The armed first-sequence translation loop (the questgiver `?` bob): a model-local offset
        // at the pivot, pointed by the placement rotation and sized by its scale — on the ARM
        // cursor (`elapsed − arm_ms`), the sequence-track half of the clock law.
        let bob = card.seq_translation.as_ref().map_or(Vec3::ZERO, |a| {
            card.placement_rot
                * (Vec3::from_array(a.sample(elapsed_ms.wrapping_add(card.arm_neg_ms)))
                    * card.scale)
        });
        let placed = Transform {
            translation: card.world_pivot + bob,
            rotation,
            scale: card.scale * pulse,
        };
        // A parked camera over a still, track-less card recomputes bit-identical values, so the
        // write only lands on real movement: unconditional, it marked every card
        // `Changed<Transform>`+`Changed<GlobalTransform>` every frame and the downstream
        // change-detection consumers (`classify_water_side`'s moved sweep, the shade dirty walk)
        // never went quiet.
        if *tf != placed {
            *tf = placed;
        }
        // Propagation already ran this frame — the direct global write is what renders.
        let placed_global = GlobalTransform::from(placed);
        if *global != placed_global {
            *global = placed_global;
        }
    }
}

/// Registers the billboard placement pass ([`BillboardPlace`], PostUpdate post-propagation).
/// Cards are spawned by the model spawn sites (in Update — mesh churn stays there).
pub struct BillboardPlugin;

impl Plugin for BillboardPlugin {
    fn build(&self, app: &mut App) {
        // The set carries the schedule constraints so every member — including the
        // particle/ribbon sims other plugins add — lands post-propagation, pre-visibility.
        app.configure_sets(
            PostUpdate,
            BillboardPlace
                .after(bevy::transform::TransformSystems::Propagate)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        )
        .add_systems(
            PostUpdate,
            (
                billboard_joint_palette,
                // The collapsed-rig world pass (decision 0724): palette rows + replaced-subtree
                // anchor re-seats, between the entity lane's joint rewrite and the card facing
                // (cards following a unit's billboard-bone anchor read the replaced frame).
                crate::rig_anim::finalize_rig_worlds,
                face_billboards,
            )
                .chain()
                .in_set(BillboardPlace),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::BillboardInfo;

    /// **Geometry lying along a bone's own axis does not sweep** — the fact decision 0847 got wrong
    /// and 0853 restored, pinned here in our own basis rather than trusted to algebra.
    ///
    /// The R14 pauldron's spikes run along WoW **−Z** (`seamswing`: mean |z| 0.284/0.291 against
    /// ≤0.06 on x and y, worst vertex 12° off axis), which is Bevy local **−Y**. A spherical
    /// billboard maps WoW Z to the camera's up, so that direction must come out as **−up — screen
    /// DOWN — from every camera orientation**, never tracing an arc. 0847 read the spikes' 0.29 yd
    /// length as a 0.29 yd arc through the plate and withdrew a correct change on it; if that arc
    /// were real, this test is where it would show up as a direction that moves with the camera.
    ///
    /// The `kept_rot` argument is swept too: the spherical arm discards the pre-billboard rotation
    /// outright (`0x7152f8`), so the wearer's shoulder yaw must not reach
    /// the result — which is *also* why the spike stops following the shoulder, the real visible
    /// difference the arm makes.
    #[test]
    fn a_spike_along_its_bone_axis_points_screen_down_from_every_angle() {
        // Bevy local −Y is the pauldron spike's run axis (WoW −Z through coords.rs' X→−Z, Y→−X,
        // Z→+Y).
        let spike_local = Vec3::NEG_Y;
        for (yaw, pitch) in [
            (0.0, 0.0),
            (0.7, 0.0),
            (2.4, 0.0),
            (-1.9, 0.0),
            (std::f32::consts::PI, 0.0),
            (0.0, 0.5),
            (1.2, -0.6),
            (-2.8, 0.9),
        ] {
            let cam =
                Transform::from_rotation(Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch));
            let (fwd, right, up) = (*cam.forward(), *cam.right(), *cam.up());
            for kept in [
                Quat::IDENTITY,
                Quat::from_rotation_y(1.1),
                Quat::from_rotation_x(-0.8),
                Quat::from_rotation_z(2.2),
            ] {
                let got =
                    billboard_basis(BillboardKind::Spherical, kept, fwd, right, up) * spike_local;
                assert!(
                    got.distance(-up) < 1e-5,
                    "spike must point screen-down, not sweep: cam yaw {yaw} pitch {pitch}, \
                     kept {kept:?} → {got:?}, expected {:?}",
                    -up
                );
            }
        }
    }

    /// A FOLLOWING card (decision 0153 — the entity-path glow cards) re-seats from its owner's
    /// live global transform each frame and despawns with it: the brazier glow burns at the bowl
    /// (owner translation + authored pivot), never the model origin — and dies when the owner
    /// streams out / unequips.
    #[test]
    fn following_card_rides_its_owner_and_dies_with_it() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, face_billboards);
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 10.0)),
        ));
        let owner = app
            .world_mut()
            .spawn(GlobalTransform::from_translation(Vec3::new(5.0, 0.0, 0.0)))
            .id();
        let info = BillboardInfo {
            bone: 0,
            pivot: Vec3::new(0.0, 1.7, 0.0), // the brazier-bowl height, model-local Bevy frame
            kind: BillboardKind::Spherical,
            scale_anim: None,
            seq_translations: vec![],
        };
        let card = app
            .world_mut()
            .spawn((BillboardCard::following(&info, owner), Transform::IDENTITY))
            .id();
        app.update();
        let tf = app.world().entity(card).get::<Transform>().unwrap();
        assert_eq!(
            tf.translation,
            Vec3::new(5.0, 1.7, 0.0),
            "owner translation + authored pivot — not the model origin"
        );
        // The hidden-owner half of this test used to live here, asserting a `Visibility` write
        // this system made and the real schedule then threw away — a green test pinning a dead
        // write (decision 1409). The law is unchanged and now lives with the one authority that
        // can enforce it: `model_render::visibility::a_card_follows_its_owners_verdict`.
        app.world_mut().entity_mut(owner).despawn();
        app.update();
        assert!(
            app.world().get_entity(card).is_err(),
            "card despawns with its owner"
        );
    }

    /// A JOINT-lane card takes the global-sequence twinkle from the joint ALONE (decision 0851):
    /// every joint/anchor lane runs a `GlobalSeqDrive` that writes the bone's scale track onto
    /// the joint, and `re_place` reads the composed result back as the card's scale — a card that
    /// also sampled its own copy multiplied the twinkle in twice, at two clocks offset by the
    /// position-hash phase (Arcane Intellect's sometimes-2.5-yd lens flare). The rigless lanes
    /// keep the sampler: there the card is the only thing animating.
    #[test]
    fn joint_lane_takes_the_twinkle_from_the_joint_alone() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, face_billboards);
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 10.0)),
        ));
        // The twinkle track, a flat ×2 — visible wherever it is applied.
        let info = BillboardInfo {
            bone: 0,
            pivot: Vec3::ZERO,
            kind: BillboardKind::Spherical,
            scale_anim: Some(BoneScaleAnim {
                duration_ms: 1000,
                interp: false,
                keys: vec![(0, [2.0, 2.0, 2.0])],
            }),
            seq_translations: vec![],
        };
        // The joint arrives already composed by the rig: drive-written twinkle ×2 × parent
        // flare ×3 = 6. Sampling the track again on top would render ×12.
        let joint = app
            .world_mut()
            .spawn(GlobalTransform::from(Transform::from_scale(Vec3::splat(
                6.0,
            ))))
            .id();
        let joint_card = app
            .world_mut()
            .spawn((
                BillboardCard::following_joint(&info, joint),
                Transform::IDENTITY,
            ))
            .id();
        // The rigless lane: a plain anchor at scale 1 — the card's own sampler is the only
        // twinkle writer.
        let anchor = app.world_mut().spawn(GlobalTransform::IDENTITY).id();
        let rigless_card = app
            .world_mut()
            .spawn((BillboardCard::following(&info, anchor), Transform::IDENTITY))
            .id();
        app.update();
        let scale = |e: Entity| app.world().entity(e).get::<Transform>().unwrap().scale;
        assert_eq!(
            scale(joint_card),
            Vec3::splat(6.0),
            "joint lane: the composed joint scale, once — no self-sample on top"
        );
        assert_eq!(
            scale(rigless_card),
            Vec3::splat(2.0),
            "rigless lane: the card's own sampler still runs"
        );
    }

    /// **A card keeps its bone's NON-UNIFORM scale** — bug B169, the Lightwell (2026-08-25).
    ///
    /// `World\Goober\G_HolyLightWell.m2` bone 0 is a lock-Z billboard carrying the well's light
    /// shaft (batch 0, `LIGHTWELLRAY.BLP`, 12 verts all weight-1 on that bone, so the 0028 split
    /// makes it a card). Its scale is a **sequence** track, not a global-sequence one, so nothing
    /// in `BillboardInfo::scale_anim` carries it: the value reaches the card only through the
    /// joint, via `re_place`. Authored (WoW axes, `m2anim`): the Spawn clamp ramps
    /// `(1,1,1) → (8.808, 8.808, 37.560)` in 0.5 s and the 23.3 s Stand loop holds it there,
    /// pulsing Z between 35.170 and 37.560 — a 0.18 × 0.12 yd card stretched into a
    /// **1.59 × 4.51 yd** column of light standing out of the bowl.
    ///
    /// `re_place` used to keep `placement.scale.x` and splat it, so the shaft rendered
    /// 1.59 × **1.06** yd: a squat additive card sitting exactly on the 1.09 yd bowl at the same
    /// height, blowing it out. Every test above this one used a uniform `Vec3::splat`, which is
    /// why the collapse survived — the numbers here are the asset's own.
    #[test]
    fn a_card_keeps_its_bones_non_uniform_scale() {
        // The authored WoW-axis scale, permuted to Bevy axes exactly as the clip builder does
        // (`benilla_assets`: `Vec3::new(s[1], s[2], s[0])` — Bevy Y is WoW Z, the shaft's long axis).
        const WOW: [f32; 3] = [8.808, 8.808, 37.560];
        let bevy_scale = Vec3::new(WOW[1], WOW[2], WOW[0]);
        // The card's authored span (`m2batch`): 0.18 yd across, 0.12 yd tall, in the YZ plane.
        const CARD_H_YD: f32 = 0.12;

        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, face_billboards);
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 10.0)),
        ));
        let info = BillboardInfo {
            bone: 0,
            pivot: Vec3::ZERO,
            kind: BillboardKind::LockZ,
            scale_anim: None, // a SEQUENCE track — the joint is the only carrier
            seq_translations: vec![],
        };
        // The joint as the rig composes it: the billboard palette preserves the full scale, so
        // this is what `re_place` reads back.
        let joint = app
            .world_mut()
            .spawn(GlobalTransform::from(Transform::from_scale(bevy_scale)))
            .id();
        let card = app
            .world_mut()
            .spawn((
                BillboardCard::following_joint(&info, joint),
                Transform::IDENTITY,
            ))
            .id();
        app.update();
        let tf = *app.world().entity(card).get::<Transform>().unwrap();
        assert_eq!(
            tf.scale, bevy_scale,
            "the joint's per-axis scale reaches the card whole — not `splat(scale.x)`"
        );
        let shaft_yd = tf.scale.y * CARD_H_YD;
        assert!(
            (shaft_yd - 4.507).abs() < 0.01,
            "the shaft stands 4.51 yd out of the well, not the 1.06 yd the x-collapse rendered \
             (got {shaft_yd:.3} yd)"
        );
    }

    /// **The billboard FRAME an equipped item's emitter rides** (decision 0813), with the real
    /// numbers of nazriel's B118 item (`LShoulder_Mail_PVPAlliance_C_01`: billboard bone 1 pivot
    /// `(-0.012, 0.162, -0.060)`, sparkle emitter position `(-0.252, 0.178, -0.046)`, both raw WoW
    /// model space — pinned in `benilla_formats`' `real_pvp_shoulder_emitters_ride_a_billboard_bone`).
    ///
    /// The law: the emitter's live origin is `pivot + camBasis·(position − pivot)`, so the sparkle
    /// sits a **fixed 0.24 yd along the VIEW axis** from the pauldron's billboard pivot — behind it
    /// (the chain offset's WoW +X is toward the viewer and this offset is negative), which is what
    /// puts most of the 0.7 yd quad behind the pauldron's own depth. A rest-pose placement instead
    /// nails it to a fixed model-space point, so what the pad occludes changes with every camera
    /// move — the reported "way too strong and off position". Both halves are asserted: the offset's
    /// magnitude/direction at one camera, and that it FOLLOWS the camera to the next.
    /// A card that spawns under a parked camera must still be placed on its first pass: the
    /// still-frame skip (1979) has no view of "never placed", and a tile bursting in under a
    /// parked camera left every glow card unrotated at scale one until the mouse moved
    /// (review 2026-09-04).
    #[test]
    fn a_card_born_on_a_still_frame_is_placed() {
        use benilla_assets::coords::wow_to_bevy;
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, face_billboards);
        app.world_mut()
            .spawn((crate::view::WorldCamera, GlobalTransform::IDENTITY));
        // Two quiet frames: the camera's change tick ages past the still-frame window.
        app.update();
        app.update();
        let root = app
            .world_mut()
            .spawn(GlobalTransform::from_translation(Vec3::new(3.0, 1.5, 0.0)))
            .id();
        app.update();
        let pivot = wow_to_bevy([-0.012, 0.162, -0.060]);
        let frame = app
            .world_mut()
            .spawn((
                BillboardCard::frame_following(BillboardKind::Spherical, pivot, root),
                Transform::IDENTITY,
            ))
            .id();
        app.update();
        let tf = *app.world().entity(frame).get::<Transform>().unwrap();
        let pivot_world = Vec3::new(3.0, 1.5, 0.0) + pivot;
        assert!(
            (tf.translation - pivot_world).length() < 1e-5,
            "a card born on a still frame sits at its pivot, not at the origin: {:?}",
            tf.translation
        );
    }

    #[test]
    fn an_item_emitters_billboard_frame_puts_it_behind_the_pivot() {
        use benilla_assets::coords::wow_to_bevy;
        const PIVOT: [f32; 3] = [-0.012, 0.162, -0.060];
        const EMITTER: [f32; 3] = [-0.252, 0.178, -0.046];
        // What `spawn_emitter`'s pivot rebase stores: the chain offset, raw WoW axes.
        let local = wow_to_bevy([
            EMITTER[0] - PIVOT[0],
            EMITTER[1] - PIVOT[1],
            EMITTER[2] - PIVOT[2],
        ]);

        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, face_billboards);
        let cam = app
            .world_mut()
            .spawn((crate::view::WorldCamera, GlobalTransform::IDENTITY))
            .id();
        // The item root — the shoulder attach point, wherever the wearer stands.
        let root = app
            .world_mut()
            .spawn(GlobalTransform::from_translation(Vec3::new(3.0, 1.5, 0.0)))
            .id();
        let frame = app
            .world_mut()
            .spawn((
                BillboardCard::frame_following(BillboardKind::Spherical, wow_to_bevy(PIVOT), root),
                Transform::IDENTITY,
            ))
            .id();

        // Camera 1: the Bevy default (at the origin, looking down −Z) — so "away from the viewer"
        // is −Z.
        app.update();
        let tf = *app.world().entity(frame).get::<Transform>().unwrap();
        let pivot_world = Vec3::new(3.0, 1.5, 0.0) + wow_to_bevy(PIVOT);
        assert!(
            (tf.translation - pivot_world).length() < 1e-5,
            "the frame sits AT the billboard pivot — the rotation swap keeps it fixed"
        );
        let sparkle = tf.transform_point(local) - pivot_world;
        assert!(
            (sparkle.z + 0.240).abs() < 2e-3,
            "0.24 yd along the view axis, away from the viewer: {sparkle:?}"
        );
        assert!(
            sparkle.truncate().length() < 0.025,
            "…and all but ~2 cm of the offset is in that one axis: {sparkle:?}"
        );

        // Camera 2: a quarter turn — the offset must follow the camera, not the model. This is the
        // whole difference from the rest pose, which would return the same vector both times.
        app.world_mut()
            .entity_mut(cam)
            .insert(GlobalTransform::from(Transform::from_rotation(
                Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            )));
        app.update();
        let turned = app
            .world()
            .entity(frame)
            .get::<Transform>()
            .unwrap()
            .transform_point(local)
            - pivot_world;
        assert!(
            (turned.x + 0.240).abs() < 2e-3 && turned.z.abs() < 0.025,
            "the same 0.24 yd, now along the new view axis: {turned:?}"
        );
    }

    /// A card the **exterior-scene cull** owns must not have its `Visibility` written here
    /// (decision 0784). One component, one authority (0025): a world-placement card is tagged
    /// with the rest of its model, both systems run in the same unordered post-propagation
    /// window, and this mirror silently undid the cull's `Hidden` — a lamp glow drawing through
    /// a sealed room's wall while every other submesh of the same lamp was correctly gone.
    ///
    /// The card must still be *placed* (its transform is this system's job either way), and an
    /// untagged card must still mirror — that half is the test above.
    #[test]
    fn the_exterior_cull_owns_a_tagged_cards_visibility() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, face_billboards);
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 10.0)),
        ));
        // A VISIBLE owner — the mirror's "show it" arm, which is the one that did the damage.
        let owner = app
            .world_mut()
            .spawn((
                GlobalTransform::from_translation(Vec3::new(5.0, 0.0, 0.0)),
                InheritedVisibility::VISIBLE,
            ))
            .id();
        let info = BillboardInfo {
            bone: 0,
            pivot: Vec3::new(0.0, 1.7, 0.0),
            kind: BillboardKind::Spherical,
            scale_anim: None,
            seq_translations: vec![],
        };
        let card = app
            .world_mut()
            .spawn((
                BillboardCard::following(&info, owner),
                Transform::IDENTITY,
                // …and the cull has already hidden it: no window admits this model.
                Visibility::Hidden,
                crate::exterior_cull::ExteriorScene,
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().entity(card).get::<Visibility>().unwrap(),
            Visibility::Hidden,
            "the cull's verdict must survive the owner mirror"
        );
        assert_eq!(
            app.world()
                .entity(card)
                .get::<Transform>()
                .unwrap()
                .translation,
            Vec3::new(5.0, 1.7, 0.0),
            "standing down from the visibility write must not stop the card being PLACED"
        );
    }

    /// The palette pass: a lock-Z billboard JOINT gets its propagated world rotation replaced by
    /// the camera basis (translation/scale kept — the pivot stays put, the grow-in scale
    /// survives), and its CHILD joint is re-composed from the replaced parent — so geometry
    /// skinned to the child inherits the facing (the frost-armor case). The host's own yaw must
    /// not leak into the result: two hosts facing opposite ways produce the SAME billboarded
    /// orientation for an upright lock-Z bone.
    #[test]
    fn palette_pass_faces_joints_and_recomposes_children() {
        let mut app = App::new();
        app.add_systems(Update, billboard_joint_palette);
        app.world_mut().spawn((
            crate::view::WorldCamera,
            // Looking along −Z from +Z, world-up Y — the identity camera frame.
            GlobalTransform::from(Transform::from_translation(Vec3::new(0.0, 0.0, 10.0))),
        ));
        let mut spawn_host = |yaw: f32| {
            let host_rot = Quat::from_rotation_y(yaw);
            // Joint 0: lock-Z billboard at the host's frame, world pivot (5, 1, 0), scale 2.
            let j0_global = GlobalTransform::from(Transform {
                translation: Vec3::new(5.0, 1.0, 0.0),
                rotation: host_rot,
                scale: Vec3::splat(2.0),
            });
            let j0 = app.world_mut().spawn((Transform::IDENTITY, j0_global)).id();
            // Joint 1: the scale-in child, one unit up its parent's Y, half scale.
            let j1_local = Transform::from_translation(Vec3::Y).with_scale(Vec3::splat(0.5));
            let j1 = app
                .world_mut()
                .spawn((j1_local, j0_global.mul_transform(j1_local)))
                .id();
            let skeleton = benilla_assets::ModelSkeleton {
                joints: vec![
                    benilla_assets::ModelJoint {
                        parent: -1,
                        local_translation: Vec3::ZERO,
                        billboard: Some(BillboardKind::LockZ),
                        parent_arm: None,
                    },
                    benilla_assets::ModelJoint {
                        parent: 0,
                        local_translation: Vec3::Y,
                        billboard: None,
                        parent_arm: None,
                    },
                ],
                spine_bone: None,
                head_bone: None,
            };
            let host = app
                .world_mut()
                .spawn((Transform::IDENTITY, GlobalTransform::IDENTITY))
                .id();
            let rig =
                BillboardJointRig::new(&skeleton, &[j0, j1], host).expect("has a billboard bone");
            app.world_mut().spawn(rig);
            (j0, j1)
        };
        let (a0, a1) = spawn_host(0.0);
        let (b0, _) = spawn_host(std::f32::consts::PI); // faces the other way
        app.update();
        let g0 = *app.world().entity(a0).get::<GlobalTransform>().unwrap();
        let (s0, r0, t0) = g0.to_scale_rotation_translation();
        assert_eq!(t0, Vec3::new(5.0, 1.0, 0.0), "the pivot stays put");
        assert!((s0 - Vec3::splat(2.0)).length() < 1e-5, "scale preserved");
        // Lock-Z at this camera: kept axis = world up; the replaced frame is exactly the
        // camera-agreeing basis — local +Y stays up, local −Z faces the viewer.
        assert!((r0 * Vec3::Y).dot(Vec3::Y) > 0.999, "kept axis upright");
        assert!((r0 * -Vec3::Z).dot(Vec3::Z) > 0.999, "faces the camera");
        // The opposite-facing host lands on the SAME orientation — char yaw does not leak.
        let (_, rb, _) = app
            .world()
            .entity(b0)
            .get::<GlobalTransform>()
            .unwrap()
            .to_scale_rotation_translation();
        assert!(
            rb.angle_between(r0) < 1e-4,
            "host yaw must not change the facing"
        );
        // The child re-composed onto the replaced parent: parent's new Y is world Y, so the
        // child sits one PARENT-scaled unit above the pivot, with composed scale 2·0.5 = 1.
        let (s1, _, t1) = app
            .world()
            .entity(a1)
            .get::<GlobalTransform>()
            .unwrap()
            .to_scale_rotation_translation();
        assert!(
            (t1 - Vec3::new(5.0, 3.0, 0.0)).length() < 1e-4,
            "child rides the new frame"
        );
        assert!(
            (s1 - Vec3::ONE).length() < 1e-5,
            "the grow-in scale chain survives"
        );
    }

    /// The four `flags & 0x6` legs of [`parent_arm_matrix`], each against its byte definition
    /// (`0x71496d`–`0x714d0c`), plus the pivot-preserving tail that is the whole
    /// reason a galloping mount carries its rider without rocking them.
    ///
    /// The parent here is rotated 90° about X and scaled non-uniformly; the root is a plain 90°
    /// yaw at scale 3. Each leg is asserted on the quantity it is defined by — axis LENGTHS for
    /// the scale legs, axis DIRECTIONS for the rotation legs — so a leg that accidentally did the
    /// other one's job cannot pass.
    #[test]
    fn the_parent_arm_legs_match_their_byte_definitions() {
        use benilla_formats::{ParentArm, ParentBasis};
        let pivot = Vec3::new(0.0, 2.0, 0.0);
        let parent = Affine3A::from_scale_rotation_translation(
            Vec3::new(1.0, 2.0, 4.0),
            Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            Vec3::new(7.0, 0.0, 0.0),
        );
        let root = Affine3A::from_scale_rotation_translation(
            Vec3::splat(3.0),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            Vec3::new(0.0, 5.0, 0.0),
        );
        let arm = |basis| ParentArm {
            ignore_translate: false,
            basis,
        };
        let axis_len = |m: Affine3A, k: usize| m.matrix3.col(k).length();
        let axis_dir = |m: Affine3A, k: usize| Vec3::from(m.matrix3.col(k).normalize());

        // `flags & 6 == 0` — the basis is the parent's, untouched.
        let keep = parent_arm_matrix(arm(ParentBasis::Keep), parent, root, pivot);
        assert!(keep.matrix3.abs_diff_eq(parent.matrix3, 1e-5));

        // `flags & 6 == 2` — unit axes, parent's directions.
        let unit = parent_arm_matrix(arm(ParentBasis::UnitNormalize), parent, root, pivot);
        for k in 0..3 {
            assert!((axis_len(unit, k) - 1.0).abs() < 1e-5, "axis {k} unit");
            assert!(
                axis_dir(unit, k).dot(axis_dir(parent, k)) > 0.9999,
                "axis {k} keeps the parent's direction"
            );
        }

        // `flags & 6 == 4` — root's direction, parent's magnitude, per axis.
        let ratio = parent_arm_matrix(arm(ParentBasis::RootDirection), parent, root, pivot);
        for k in 0..3 {
            assert!(
                (axis_len(ratio, k) - axis_len(parent, k)).abs() < 1e-4,
                "axis {k} keeps the parent's magnitude"
            );
            assert!(
                axis_dir(ratio, k).dot(axis_dir(root, k)) > 0.9999,
                "axis {k} takes the root's direction"
            );
        }

        // `flags & 6 == 6` — the root basis outright, scale included.
        let full = parent_arm_matrix(arm(ParentBasis::RootBasis), parent, root, pivot);
        assert!(full.matrix3.abs_diff_eq(root.matrix3, 1e-5));

        // The pivot-preserving tail: whichever leg ran, composing the bone's own bind translation
        // onto the rewritten matrix lands it exactly where the ANIMATED parent carried it.
        let want = parent.transform_point3a(pivot.into());
        for m in [keep, unit, ratio, full] {
            let landed = m.transform_point3a(pivot.into());
            assert!((landed - want).length() < 1e-4, "pivot preserved: {landed}");
        }

        // `flags & 1` instead places the bone at the model root's own origin.
        let moved = parent_arm_matrix(
            ParentArm {
                ignore_translate: true,
                basis: ParentBasis::RootBasis,
            },
            parent,
            root,
            pivot,
        );
        assert!((Vec3::from(moved.translation) - Vec3::new(0.0, 5.0, 0.0)).length() < 1e-5);
    }

    /// The ignore-parent-rotation joint (bone flag 0x04 — the HandArrow/Bullet attach helpers):
    /// its pivot rides the parent's full matrix, its
    /// ROTATION resets to the host root's frame — and a rigid child (the nocked arrow) hanging
    /// under it re-composes onto the replaced frame instead of keeping the twisted propagated one.
    #[test]
    fn ignore_parent_rotation_joint_keeps_the_model_frame() {
        let mut app = App::new();
        app.add_systems(Update, billboard_joint_palette);
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from(Transform::from_translation(Vec3::new(0.0, 0.0, 10.0))),
        ));
        // The host root: yawed 90° — the model frame every flag-0x04 joint must land on.
        let host_rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let host = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                GlobalTransform::from(Transform::from_rotation(host_rot)),
            ))
            .id();
        // Joint 0: the animated hand — twisted a further 90° about X (the draw-hand roll the
        // arrow must NOT inherit), pivot at (1, 2, 3).
        let hand_rot = host_rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let j0_global = GlobalTransform::from(Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: hand_rot,
            scale: Vec3::ONE,
        });
        let j0 = app.world_mut().spawn((Transform::IDENTITY, j0_global)).id();
        // Joint 1: the flag-0x04 attach helper, one local unit up the HAND's frame.
        let j1_local = Transform::from_translation(Vec3::Y);
        let j1 = app
            .world_mut()
            .spawn((j1_local, j0_global.mul_transform(j1_local)))
            .id();
        // The rigid arrow child under the helper, at a local offset — propagated PRE-pass with
        // the twisted frame (what the bug rendered).
        let arrow_local = Transform::from_translation(Vec3::X);
        let arrow = app
            .world_mut()
            .spawn((
                arrow_local,
                j0_global.mul_transform(j1_local).mul_transform(arrow_local),
            ))
            .id();
        app.world_mut().entity_mut(j1).add_child(arrow);
        let skeleton = benilla_assets::ModelSkeleton {
            joints: vec![
                benilla_assets::ModelJoint {
                    parent: -1,
                    local_translation: Vec3::ZERO,
                    billboard: None,
                    parent_arm: None,
                },
                benilla_assets::ModelJoint {
                    parent: 0,
                    local_translation: Vec3::Y,
                    billboard: None,
                    parent_arm: Some(benilla_formats::ParentArm {
                        ignore_translate: false,
                        basis: benilla_formats::ParentBasis::RootDirection,
                    }),
                },
            ],
            spine_bone: None,
            head_bone: None,
        };
        let rig = BillboardJointRig::new(&skeleton, &[j0, j1], host)
            .expect("has an ignore-parent-rotation bone");
        app.world_mut().spawn(rig);
        app.update();

        // The helper joint: pivot carried by the HAND's frame (hand rot · Y above the hand),
        // rotation snapped back to the HOST's.
        let (_, r1, t1) = app
            .world()
            .entity(j1)
            .get::<GlobalTransform>()
            .unwrap()
            .to_scale_rotation_translation();
        let expected_pivot = Vec3::new(1.0, 2.0, 3.0) + hand_rot * Vec3::Y;
        assert!(
            (t1 - expected_pivot).length() < 1e-5,
            "the pivot rides the parent's full matrix"
        );
        assert!(
            r1.angle_between(host_rot) < 1e-3,
            "the rotation resets to the model root's frame"
        );
        // The arrow child re-composed onto the replaced frame: host-frame X off the pivot.
        let (_, ra, ta) = app
            .world()
            .entity(arrow)
            .get::<GlobalTransform>()
            .unwrap()
            .to_scale_rotation_translation();
        assert!(
            (ta - (expected_pivot + host_rot * Vec3::X)).length() < 1e-5,
            "the rigid child rides the replaced frame"
        );
        assert!(
            ra.angle_between(host_rot) < 1e-3,
            "the child inherits the flat model-space orientation"
        );
    }

    /// A rigged model nested under another rig's rewritten joint (the Eviscerate impact instance
    /// on the boar's flag-0x04 attach helper): the outer rig's child walk must not enter the
    /// nested rig's subtree AT ALL — the root keeps its propagated global (the live animated
    /// attach-bone frame its emitters' attach rotation reads), and the interior belongs to the
    /// nested rig's own pass. Both spawn orders must land on the identical result; pre-fix,
    /// whichever rig iterated last won, so the effect's camera-born billboard frame (and its
    /// animated attach frame) survived or died per launch.
    #[test]
    fn nested_rig_interior_is_owned_by_its_own_pass() {
        for nested_first in [false, true] {
            let mut app = App::new();
            app.add_systems(Update, billboard_joint_palette);
            app.world_mut().spawn((
                crate::view::WorldCamera,
                GlobalTransform::from(Transform::from_translation(Vec3::new(0.0, 0.0, 10.0))),
            ));
            // The outer host (a boar): yawed 90°, one flag-0x04 attach-helper joint whose
            // propagated global carries an animated twist the reset must erase.
            let host_rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
            let host = app
                .world_mut()
                .spawn((
                    Transform::IDENTITY,
                    GlobalTransform::from(Transform::from_rotation(host_rot)),
                ))
                .id();
            let j0_global = GlobalTransform::from(Transform {
                translation: Vec3::new(1.0, 2.0, 3.0),
                rotation: host_rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                scale: Vec3::ONE,
            });
            let j0 = app.world_mut().spawn((Transform::IDENTITY, j0_global)).id();
            let outer_skeleton = benilla_assets::ModelSkeleton {
                joints: vec![benilla_assets::ModelJoint {
                    parent: -1,
                    local_translation: Vec3::ZERO,
                    billboard: None,
                    parent_arm: Some(benilla_formats::ParentArm {
                        ignore_translate: false,
                        basis: benilla_formats::ParentBasis::RootDirection,
                    }),
                }],
                spine_bone: None,
                head_bone: None,
            };
            // The nested effect instance: its root hangs one local X under the helper, and its
            // single joint is a lock-Z billboard whose propagated global still carries the
            // (wrong) host twist — its own pass must replace it, and keep it replaced.
            let fx_local = Transform::from_translation(Vec3::X);
            let fx_root = app
                .world_mut()
                .spawn((fx_local, j0_global.mul_transform(fx_local)))
                .id();
            app.world_mut().entity_mut(j0).add_child(fx_root);
            let fj0 = app
                .world_mut()
                .spawn((Transform::IDENTITY, j0_global.mul_transform(fx_local)))
                .id();
            app.world_mut().entity_mut(fx_root).add_child(fj0);
            let nested_skeleton = benilla_assets::ModelSkeleton {
                joints: vec![benilla_assets::ModelJoint {
                    parent: -1,
                    local_translation: Vec3::ZERO,
                    billboard: Some(BillboardKind::Spherical),
                    parent_arm: None,
                }],
                spine_bone: None,
                head_bone: None,
            };
            let outer_rig = BillboardJointRig::new(&outer_skeleton, &[j0], host).unwrap();
            let nested_rig = BillboardJointRig::new(&nested_skeleton, &[fj0], fx_root).unwrap();
            if nested_first {
                app.world_mut().spawn(nested_rig);
                app.world_mut().spawn(outer_rig);
            } else {
                app.world_mut().spawn(outer_rig);
                app.world_mut().spawn(nested_rig);
            }
            app.update();

            // The nested root keeps its PROPAGATED global — the animated attach-bone frame
            // (with the hand twist): the walk never entered the nested subtree.
            let expected = j0_global.mul_transform(fx_local);
            let (_, rr, rt) = app
                .world()
                .entity(fx_root)
                .get::<GlobalTransform>()
                .unwrap()
                .to_scale_rotation_translation();
            let (_, er, et) = expected.to_scale_rotation_translation();
            assert!(
                (rt - et).length() < 1e-5,
                "nested root keeps its propagated seat (nested_first={nested_first})"
            );
            assert!(
                rr.angle_between(er) < 1e-3,
                "nested root keeps the animated attach rotation (nested_first={nested_first})"
            );
            // The nested rig's own billboard frame SURVIVES, in either spawn order: the outer
            // walk stopped at the nested root instead of re-composing fj0 from its raw local.
            let (_, rj, _) = app
                .world()
                .entity(fj0)
                .get::<GlobalTransform>()
                .unwrap()
                .to_scale_rotation_translation();
            // The spherical basis at this camera, through the WoW→Bevy axis fold
            // (`billboard_basis`'s `from_cols(-by, bz, -bx)`): Bevy-local −Z toward the viewer
            // (WoW X), Bevy-local +Y screen-up (WoW Z) — the (π,0) ray ring's camera-born plane
            // (`0x71547c`).
            assert!(
                (rj * -Vec3::Z).dot(Vec3::Z) > 0.999,
                "the nested billboard faces the camera (nested_first={nested_first})"
            );
            assert!(
                (rj * Vec3::Y).dot(Vec3::Y) > 0.999,
                "the nested billboard's screen-up axis holds (nested_first={nested_first})"
            );
        }
    }

    /// An armed first-sequence translation loop (the questgiver `?` bob) moves the card off its
    /// pivot by the sampled offset, on the arm-time cursor: armed at t=0, sampled at the loop's
    /// midpoint, the card sits at the middle key's offset. No `Time` plugin — the clock is set by
    /// hand so the sample point is exact.
    #[test]
    fn armed_seq_translation_bobs_the_card() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, face_billboards);
        // A camera straight ahead of the card's rest normal: the facing rotation is identity, so
        // the transform isolates the bob.
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 10.0)),
        ));
        let bob = BoneScaleAnim {
            duration_ms: 1000,
            interp: true,
            keys: vec![(0, [0.0; 3]), (500, [0.0, 1.0, 0.0]), (1000, [0.0; 3])],
        };
        let info = BillboardInfo {
            bone: 0,
            pivot: Vec3::ZERO,
            kind: BillboardKind::LockZ,
            scale_anim: None,
            seq_translations: vec![], // doodad default: `new` never arms one
        };
        let card = app
            .world_mut()
            .spawn((
                BillboardCard::new(&info, Transform::IDENTITY).with_seq_translation(Some(bob), 0),
                Transform::IDENTITY,
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(500));
        app.update();
        let tf = app.world().entity(card).get::<Transform>().unwrap();
        assert_eq!(
            tf.translation,
            Vec3::new(0.0, 1.0, 0.0),
            "the middle key's offset, at the pivot"
        );
    }
}
