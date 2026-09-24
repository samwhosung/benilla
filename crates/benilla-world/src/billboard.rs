//! M2 billboard cards: submeshes riding a billboard bone (glow cards, chains, the quest markers),
//! turned to the camera every frame as the 1.12 client does.
//!
//! The reference computes the bone palette in view space and replaces a billboard bone's rows with
//! the camera basis. Spherical (`0x08`) takes the whole basis, bone X toward the viewer, Y
//! screen-right, Z screen-up (the rows `{(0,0,−1),(1,0,0),(0,1,0)}` at `0x714463`); lock-X/Y/Z
//! (`0x10`/`0x20`/`0x40`) keeps its authored axis and rebuilds the other two from the camera. It
//! is one shared orientation for every billboard, never a per-pivot aim or a facet normal.
//!
//! A card's mesh is centred at its bone pivot in model-local Bevy axes (WoW X→−Z, Y→−X, Z→+Y), so
//! the entity sits at the pivot with the rebuilt basis as its rotation.

use benilla_assets::BillboardInfo;
use benilla_formats::{BillboardKind, BoneScaleAnim};
use bevy::math::{Affine3A, Mat3A, Vec3A};
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::view::WorldCamera;

/// A spawned billboard card: world pivot, per-axis scale, bone-flag arm and scale pulse. It needs
/// a [`MeshTag`] as a world root: a per-model alpha (the self-model fade) reaches it through its
/// own tag, and the default `MeshTag(0)` is the shader's untagged, opaque sentinel.
#[derive(Component)]
#[require(Transform, Visibility, MeshTag)]
pub struct BillboardCard {
    world_pivot: Vec3,
    /// Per-axis scale in the card's own frame, applied before the camera basis (`T·R_cam·S`) as
    /// the joint palette applies it. Not a scalar: the Lightwell's shaft
    /// (`World\Goober\G_HolyLightWell.m2` bone 0) holds `(8.808, 8.808, 37.560)`.
    scale: Vec3,
    kind: BillboardKind,
    /// The bone's global-sequence scale loop (a lamppost glow's breathe), times [`Self::scale`].
    scale_anim: Option<BoneScaleAnim>,
    /// The arm time, negated so a wrapping add subtracts: [`Self::seq_translation`] samples at
    /// `elapsed − arm_ms`, re-armed per play like the reference's sequence tracks.
    arm_neg_ms: u32,
    /// The global-sequence clock's attach anchor (ms), stamped on the first placement pass and
    /// never re-armed: the reference snapshots the scene clock once per instance (`CM2Model+0x68`)
    /// and samples at `sceneNow − attach`, so cards attached on different frames breathe apart.
    gseq_attach_ms: Option<u32>,
    /// The bone's armed first-sequence translation loop (a quest marker's bob, Bevy axes), added
    /// at the pivot and turned by [`Self::placement_rot`]. Only the marker spawn site arms one.
    seq_translation: Option<BoneScaleAnim>,
    /// The placement's rotation, so the bob offset (model-local) points where the instance points.
    placement_rot: Quat,
    /// The entity this card follows, re-seated from it every frame and despawned with it (a unit
    /// or GameObject anchor, a held-item root); `None` for a fixed placement.
    follow: Option<Entity>,
    /// The pivot in the model's local Bevy frame (re-applied each frame when `follow` is set).
    local_pivot: Vec3,
}

impl BillboardCard {
    /// A card for a submesh's [`BillboardInfo`] at a fixed instance `placement`.
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

    /// A card following `owner` (a creature, GameObject, held item, missile or spell effect).
    pub fn following(info: &BillboardInfo, owner: Entity) -> Self {
        let mut card = Self::new(info, Transform::IDENTITY);
        card.follow = Some(owner);
        card
    }

    /// A card on an animated host's billboard joint (a swinging lamp, a mount's lights). The joint
    /// already bakes the pivot and the bone's global-sequence scale
    /// ([`crate::rig_anim::GlobalSeqDrive`]), so the card samples none: that would twinkle twice.
    pub fn following_joint(info: &BillboardInfo, joint: Entity) -> Self {
        let mut card = Self::following(info, joint);
        card.local_pivot = Vec3::ZERO;
        card.scale_anim = None;
        card
    }

    /// A card with no mesh: a billboard bone's replaced palette matrix as a live transform, for an
    /// item's particle emitter to ride (an item spawns no rig); the reference folds the emitter's
    /// position through this matrix (`0x7190a9`-`0x71910c`). `pivot` is model-local Bevy axes.
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

    /// Arms the first-sequence translation loop (a quest marker's bob) at `arm_ms`, so it opens on
    /// its first key, as the reference arms it when the quest status arrives.
    pub fn with_seq_translation(mut self, anim: Option<BoneScaleAnim>, arm_ms: u32) -> Self {
        self.arm_seq_translation(anim, arm_ms);
        self
    }

    /// Re-arms the loop on a live card: the marker swaps between its low (anim 0) and raised
    /// (anim 190) bob when the unit's overhead name toggles.
    pub fn arm_seq_translation(&mut self, anim: Option<BoneScaleAnim>, arm_ms: u32) {
        if anim.is_some() {
            self.arm_neg_ms = arm_ms.wrapping_neg();
        }
        self.seq_translation = anim;
    }

    /// The entity this card follows. A card is a world root, so a walk over its model's tree (the
    /// self fade, the shade push) recognises it through this, and the visibility authority
    /// (`model_render::visibility`) draws it only while this entity is visible.
    pub fn follows(&self) -> Option<Entity> {
        self.follow
    }

    /// Re-seats the card from a fresh `placement` (a quest marker over a moving unit).
    pub fn re_place(&mut self, placement: Transform, local_pivot: Vec3) {
        self.world_pivot = placement.transform_point(local_pivot);
        self.scale = placement.scale;
        self.placement_rot = placement.rotation;
    }
}

/// A bone's rewritten parent matrix, the `flags & 0x7` arm (`0x71496d`-`0x714d0c`), taken before
/// its own TRS and the `& 0x78` billboard switch, which still runs. `root` is the model's frame
/// `[model+0xfc]` (composed at `0x714389`; a rider's seat anchor), `pivot` the bone's bind local
/// translation. The tail `T' = pivotWorld − piv·newBasis` keeps where the animated parent carried
/// the bone and drops only its orientation, so a mount carries its rider without rocking them.
/// The reference's row K is our column K, both the image of basis vector K.
pub(crate) fn parent_arm_matrix(
    arm: benilla_formats::ParentArm,
    parent: Affine3A,
    root: Affine3A,
    pivot: Vec3,
) -> Affine3A {
    use benilla_formats::ParentBasis;
    // `0x714bdb`'s unit guard: a degenerate axis is left as it is.
    const UNIT_EPS: f32 = 1.0 / (1 << 22) as f32;
    // `0x80c5c8`, the ratio leg's own constant, not the unit eps above.
    const RATIO_EPS: f32 = 1e-5;
    let (p, r) = (parent.matrix3, root.matrix3);
    let per_axis = |f: &dyn Fn(usize) -> Vec3A| Mat3A::from_cols(f(0), f(1), f(2));
    let matrix3 = match arm.basis {
        ParentBasis::Keep => p,
        // `flags & 6 == 2`, ignore parent scale: unit-length axes, directions kept.
        ParentBasis::UnitNormalize => per_axis(&|k| {
            let len = p.col(k).length();
            if len > UNIT_EPS {
                p.col(k) / len
            } else {
                p.col(k)
            }
        }),
        // `flags & 6 == 4`, ignore parent rotation: the root's direction at the parent's length.
        ParentBasis::RootDirection => per_axis(&|k| {
            let rl2 = r.col(k).length_squared();
            let ratio = if rl2 <= RATIO_EPS {
                1.0
            } else {
                p.col(k).length() / rl2.sqrt()
            };
            r.col(k) * ratio
        }),
        // `flags & 6 == 6`, ignore parent rotation and scale: the root basis outright.
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

/// The rebuilt orientation for a billboard of `kind`, for cards and joints alike; `kept_rot` is
/// the pre-billboard rotation a lock arm keeps its axis from. `bx/by/bz` are the bone's WoW axes
/// after the replacement, and Bevy local X/Y/Z maps onto `−by`/`bz`/`−bx`.
pub fn billboard_basis(
    kind: BillboardKind,
    kept_rot: Quat,
    fwd: Vec3,
    right: Vec3,
    up: Vec3,
) -> Quat {
    let (bx, by, bz) = match kind {
        // Spherical (`0x08`): X toward the viewer, Y screen-right, Z screen-up.
        BillboardKind::Spherical => (-fwd, right, up),
        // Lock-Z (`0x40`, the `?` marker): keep the authored Z, rebuild the in-plane pair.
        // `Y = Fwd × Z`, as the other order mirrors the model, and it agrees with the spherical
        // arm at a level camera. Looking along Z degenerates the cross: Y holds screen-right.
        BillboardKind::LockZ => {
            let bz = (kept_rot * Vec3::Y).normalize_or(Vec3::Y);
            let by = fwd.cross(bz).try_normalize().unwrap_or(right);
            let bx = by.cross(bz);
            (bx, by, bz)
        }
        // Lock-X/Y: lock-Z's structure per kept axis, the cyclically previous axis taking
        // `Fwd × kept` and the third completing the right-handed WoW triple. Their in-plane sign
        // follows lock-Z's by analogy, not from the reference.
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

/// A rigged host's billboard and parent-arm bones, beside its `AnimationPlayer`. The reference
/// billboards the bone palette, where children multiply onto the replaced matrix (`0x7151ba`), so
/// geometry skinned to a billboard bone's child (the frost-armor sheets) inherits the facing.
#[derive(Component)]
pub struct BillboardJointRig {
    /// The host root, the model's own frame `[model+0xfc]` that the `flags & 0x7` arm builds from.
    root: Entity,
    joints: Vec<Entity>,
    parents: Vec<i16>,
    kinds: Vec<Option<BillboardKind>>,
    /// Bone flags `0x1/0x2/0x4` per joint: the HandArrow/Bullet helpers, every mount's rider seat.
    arms: Vec<Option<benilla_formats::ParentArm>>,
    /// Each bone's bind local translation, the pivot the arm preserves; the animated local is not
    /// interchangeable, as the reference rotates it by the new basis.
    binds: Vec<Vec3>,
}

impl BillboardJointRig {
    /// The host root: the collapsed-rig world pass leaves a nested rig's interior to it.
    pub(crate) fn root(&self) -> Entity {
        self.root
    }

    /// For a rig under host `root`; `None` when no bone billboards or has a `flags & 0x7` arm.
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

/// The palette half of the billboard law: each billboard joint's world rotation becomes the camera
/// basis, keeping scale and translation (the reference's tail `0x715868`), and its descendants
/// re-compose on it, so skinned geometry and riding emitters inherit the facing. Every palette
/// consumer must read after this pass: avian's physics sync re-propagates from locals in the
/// fixed loop, so an Update read sees the un-billboarded pose. The walk never enters a nested rig,
/// not even its root (a spell effect on an attach bone), so no rig overwrites another's frames.
/// M2 bones are parent-sorted; a malformed child whose parent follows it keeps its propagated pose.
pub fn billboard_joint_palette(
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    hosts: Query<&BillboardJointRig>,
    // A parked host (frozen off-frustum) is not re-faced but stays in the do-not-enter set.
    parked: Query<Has<crate::rig_anim::AnimParked>>,
    mut joints: Query<(&Transform, &mut GlobalTransform), Without<WorldCamera>>,
    children: Query<&Children>,
) {
    let Ok(cam_tf) = cam.single() else {
        return;
    };
    let (fwd, right, up) = (*cam_tf.forward(), *cam_tf.right(), *cam_tf.up());
    // Every rig root: the walk's do-not-enter set.
    let rig_roots: bevy::platform::collections::HashSet<Entity> =
        hosts.iter().map(|r| r.root).collect();
    for rig in hosts
        .iter()
        .filter(|r| !parked.get(r.root).unwrap_or(false))
    {
        // The model's own frame `[model+0xfc]`, which the `flags & 0x7` arm builds from.
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
                continue; // untouched: the propagated pose stands
            }
            // An armed bone under an untouched parent reads the parent's propagated frame.
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
                // `flags & 0x7` rewrites the parent; the billboard switch below still runs.
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
        // Rigid children under a rewritten joint (held items, the nocked arrow) were propagated
        // before the rewrite: re-compose them, skipping joints, which the loop above owns.
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

/// The billboard placement pass: PostUpdate, after transform propagation and before visibility,
/// so placement reads this frame's pose. Card re-seaters (the quest markers) order before it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BillboardPlace;

/// Faces each card to the camera about its pivot and applies its scale pulse, first re-seating a
/// following card from its owner, or despawning it with the owner. Cards are world roots, so the
/// direct `GlobalTransform` write is exact. A `Visibility` write here would be lost: this set is
/// not ordered before visibility propagation, and [`crate::model_render::visibility`] owns it.
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
    // A teleport frame moves the seat's local before propagation, so either change counts.
    let cam_moved = cam_tf.is_changed() || cam_local.as_ref().is_some_and(|l| l.is_changed());
    let (fwd, right, up) = (*cam_tf.forward(), *cam_tf.right(), *cam_tf.up());
    let elapsed_ms = time.elapsed().as_millis() as u32;
    for (entity, mut card, mut tf, mut global, tag) in &mut cards {
        // A track-less card is a pure function of the camera and its owner, so it is skipped
        // when neither moved, unless it has never been placed (no attach stamp yet).
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
                    // The `card` trace (`WOW_MOVE_TRACE`, ~2 Hz): each following card's place,
                    // scale and alpha, the three causes of an invisible card. `enabled_for`, so
                    // `WOW_MOVE_TRACE_TAGS` drops this busiest tag cheaply.
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
        // The gseq attach anchor, stamped on the first placement pass (`CM2Model+0x68`).
        let attach_ms = *card.gseq_attach_ms.get_or_insert(elapsed_ms);
        let card = &*card;
        let rotation = billboard_basis(card.kind, card.placement_rot, fwd, right, up);
        // The global-sequence scale pulse on the instance's anchored cursor, `sceneNow − attach`.
        let pulse = card.scale_anim.as_ref().map_or(Vec3::ONE, |a| {
            Vec3::from_array(a.sample(elapsed_ms.wrapping_sub(attach_ms)))
        });
        // The armed bob on the arm cursor `elapsed − arm_ms`, turned and sized by the placement.
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
        // Only a real change is written, so change-detection consumers go quiet on a still frame.
        if *tf != placed {
            *tf = placed;
        }
        // Propagation already ran this frame: the direct global write is what renders.
        let placed_global = GlobalTransform::from(placed);
        if *global != placed_global {
            *global = placed_global;
        }
    }
}

/// Registers the [`BillboardPlace`] pass; the model spawn sites spawn cards, in Update.
pub struct BillboardPlugin;

impl Plugin for BillboardPlugin {
    fn build(&self, app: &mut App) {
        // The set holds the ordering, so the particle and ribbon sims other plugins add get it too.
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
                // The collapsed-rig world pass, between the joint rewrite and the card facing:
                // cards on a unit's billboard-bone anchor read the replaced frame.
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

    /// The R14 pauldron's spikes run along WoW −Z (`seamswing`), Bevy −Y: a spherical billboard
    /// points them screen-down from every camera and discards the pre-billboard rotation
    /// (`0x7152f8`), so the wearer's shoulder yaw never reaches them.
    #[test]
    fn a_spike_along_its_bone_axis_points_screen_down_from_every_angle() {
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
        app.world_mut().entity_mut(owner).despawn();
        app.update();
        assert!(
            app.world().get_entity(card).is_err(),
            "card despawns with its owner"
        );
    }

    /// The joint's `GlobalSeqDrive` carries the twinkle; only a rigless card samples its own.
    #[test]
    fn joint_lane_takes_the_twinkle_from_the_joint_alone() {
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
            kind: BillboardKind::Spherical,
            scale_anim: Some(BoneScaleAnim {
                duration_ms: 1000,
                interp: false,
                keys: vec![(0, [2.0, 2.0, 2.0])],
            }),
            seq_translations: vec![],
        };
        // The joint as the rig composes it: twinkle ×2 × parent flare ×3 = 6; sampling the
        // track again would render ×12.
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

    /// `World\Goober\G_HolyLightWell.m2` bone 0 is a lock-Z card whose sequence scale track
    /// (`m2anim`, WoW axes) holds `(8.808, 8.808, 37.560)` through its Stand loop; it reaches the
    /// card only through the joint, and stands the 0.12 yd card 4.51 yd tall.
    #[test]
    fn a_card_keeps_its_bones_non_uniform_scale() {
        // WoW axes permuted to Bevy as the clip builder does: Bevy Y is WoW Z, the long axis.
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
            scale_anim: None, // a sequence track: the joint is its only carrier
            seq_translations: vec![],
        };
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

    /// The R14 PVP shoulder (`LShoulder_Mail_PVPAlliance_C_01`): billboard bone 1's pivot and its
    /// sparkle emitter, raw WoW axes. The emitter rides `pivot + camBasis·(position − pivot)`, so
    /// the sparkle sits 0.24 yd behind the pivot along the view axis from any camera.
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

        // Camera 1: the Bevy default, looking down −Z, so away from the viewer is −Z.
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

        // Camera 2: a quarter turn. The offset follows the camera; a rest pose would not move.
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

    /// A card tagged `ExteriorScene` keeps the cull's `Hidden`, and is still placed.
    #[test]
    fn the_exterior_cull_owns_a_tagged_cards_visibility() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, face_billboards);
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 10.0)),
        ));
        // A visible owner, whose verdict must not reach a card the cull has hidden.
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
                // The cull has already hidden it: no window admits this model.
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

    /// A lock-Z joint takes the camera basis, keeping its pivot and scale, and its child joint
    /// re-composes on it; the host's yaw does not reach the result.
    #[test]
    fn palette_pass_faces_joints_and_recomposes_children() {
        let mut app = App::new();
        app.add_systems(Update, billboard_joint_palette);
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from(Transform::from_translation(Vec3::new(0.0, 0.0, 10.0))),
        ));
        let mut spawn_host = |yaw: f32| {
            let host_rot = Quat::from_rotation_y(yaw);
            let j0_global = GlobalTransform::from(Transform {
                translation: Vec3::new(5.0, 1.0, 0.0),
                rotation: host_rot,
                scale: Vec3::splat(2.0),
            });
            let j0 = app.world_mut().spawn((Transform::IDENTITY, j0_global)).id();
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
        assert!((r0 * Vec3::Y).dot(Vec3::Y) > 0.999, "kept axis upright");
        assert!((r0 * -Vec3::Z).dot(Vec3::Z) > 0.999, "faces the camera");
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
        // The child re-composed on the replaced parent: one parent-scaled unit above the pivot,
        // composed scale 2·0.5 = 1.
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

    /// Each `flags & 0x6` leg (`0x71496d`-`0x714d0c`) is asserted on what defines it, axis lengths
    /// for scale and axis directions for rotation, so a leg doing another's job fails; then the
    /// pivot-preserving tail.
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

        let keep = parent_arm_matrix(arm(ParentBasis::Keep), parent, root, pivot);
        assert!(keep.matrix3.abs_diff_eq(parent.matrix3, 1e-5));

        let unit = parent_arm_matrix(arm(ParentBasis::UnitNormalize), parent, root, pivot);
        for k in 0..3 {
            assert!((axis_len(unit, k) - 1.0).abs() < 1e-5, "axis {k} unit");
            assert!(
                axis_dir(unit, k).dot(axis_dir(parent, k)) > 0.9999,
                "axis {k} keeps the parent's direction"
            );
        }

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

        let full = parent_arm_matrix(arm(ParentBasis::RootBasis), parent, root, pivot);
        assert!(full.matrix3.abs_diff_eq(root.matrix3, 1e-5));

        // The tail: every leg lands the bind translation where the animated parent carried it.
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

    /// Bone flag 0x04 (the HandArrow/Bullet helpers): the pivot rides the parent, the rotation
    /// resets to the host root's, and a rigid child (the nocked arrow) re-composes onto it.
    #[test]
    fn ignore_parent_rotation_joint_keeps_the_model_frame() {
        let mut app = App::new();
        app.add_systems(Update, billboard_joint_palette);
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from(Transform::from_translation(Vec3::new(0.0, 0.0, 10.0))),
        ));
        // The host root, yawed 90°: the frame every flag-0x04 joint lands on.
        let host_rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let host = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                GlobalTransform::from(Transform::from_rotation(host_rot)),
            ))
            .id();
        // Joint 0: the animated hand, with a roll about X the arrow must not inherit.
        let hand_rot = host_rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let j0_global = GlobalTransform::from(Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: hand_rot,
            scale: Vec3::ONE,
        });
        let j0 = app.world_mut().spawn((Transform::IDENTITY, j0_global)).id();
        // Joint 1: the flag-0x04 attach helper, one local unit up the hand's frame.
        let j1_local = Transform::from_translation(Vec3::Y);
        let j1 = app
            .world_mut()
            .spawn((j1_local, j0_global.mul_transform(j1_local)))
            .id();
        // The rigid arrow under the helper, propagated before the pass with the twisted frame.
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

    /// A rig nested under another's rewritten joint (a spell impact on an attach helper) keeps its
    /// root's propagated frame and its own billboard, in either spawn order.
    #[test]
    fn nested_rig_interior_is_owned_by_its_own_pass() {
        for nested_first in [false, true] {
            let mut app = App::new();
            app.add_systems(Update, billboard_joint_palette);
            app.world_mut().spawn((
                crate::view::WorldCamera,
                GlobalTransform::from(Transform::from_translation(Vec3::new(0.0, 0.0, 10.0))),
            ));
            // The outer host: yawed 90°, one flag-0x04 attach helper whose propagated global
            // carries an animated twist the reset erases.
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
            // The nested effect: its root one local X under the helper, its one joint a spherical
            // billboard whose propagated global still carries the host twist.
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
            let (_, rj, _) = app
                .world()
                .entity(fj0)
                .get::<GlobalTransform>()
                .unwrap()
                .to_scale_rotation_translation();
            // The spherical basis at this camera: Bevy-local −Z (WoW X) toward the viewer, +Y
            // (WoW Z) screen-up, the camera-born plane at `0x71547c`.
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

    /// Armed at t=0 and sampled at the loop's midpoint, the card sits at the middle key's offset.
    #[test]
    fn armed_seq_translation_bobs_the_card() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, face_billboards);
        // A camera straight ahead: the facing rotation is identity, isolating the bob.
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
