//! The collapsed rig's two passes over the [`RigPose`] arrays. The model pass
//! ([`compose_rig_models`]) folds locals into model space, the `flags & 0x7` arm included, and
//! re-seats the anchors before propagation; the world pass ([`finalize_rig_worlds`]) composes
//! rig-relative with the billboard camera basis, writes the palette rows and re-seats replaced
//! subtrees. The arm runs in both passes, each on its own chain, so it is not applied twice; it
//! must run in the model pass, since the anchors are seated from it.

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use crate::billboard::{billboard_basis, parent_arm_matrix, BillboardJointRig};
use crate::rig_palette::{RigPalettes, RigSkin};
use crate::view::WorldCamera;

use super::{AnimParked, RigFrame, RigPose};

/// The pose post-pass window: the locals writers after the evaluator, before the model compose.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PosePost;

/// Fold each pose-dirty rig into model space and re-seat its anchors, leaving `pose_dirty` for
/// the world pass. A parked rig is skipped; its wake unparks it before this runs.
fn compose_rig_models(
    mut rigs: Query<&mut RigPose, Without<AnimParked>>,
    mut anchors: Query<&mut Transform>,
) {
    for rig in &mut rigs {
        if !rig.pose_dirty {
            continue;
        }
        let rig = rig.into_inner();
        rig.compose();
        for &(bone, anchor) in &rig.anchors {
            let Some(m) = rig.model.get(bone as usize) else {
                continue;
            };
            let Ok(mut t) = anchors.get_mut(anchor) else {
                continue;
            };
            let (scale, rotation, translation) = m.to_scale_rotation_translation();
            *t = Transform {
                translation,
                rotation,
                scale,
            };
        }
    }
}

/// Translate a composed rig-relative frame back into world space.
fn shift(g: GlobalTransform, origin: Vec3) -> GlobalTransform {
    let mut a = g.affine();
    a.translation += bevy::math::Vec3A::from(origin);
    GlobalTransform::from(a)
}

/// One rig's chain and its replaced bones from `root_g`, the root frame `[model+0xfc]` (for a
/// rider, its seat anchor, `0x714389`), passed translation-zeroed; mirrors
/// `billboard_joint_palette`.
fn rig_worlds(
    rig: &RigPose,
    root_g: GlobalTransform,
    cam: Option<(Vec3, Vec3, Vec3)>,
) -> (Vec<GlobalTransform>, Vec<bool>) {
    let n = rig.locals.len();
    let mut worlds = Vec::with_capacity(n);
    let mut touched = vec![false; n];
    for i in 0..n {
        let parent = usize::try_from(rig.parents[i]).ok().filter(|&p| p < i);
        let parent_world = match parent {
            Some(p) => worlds[p],
            None => root_g,
        };
        // The arm first (`0x714961`): it rewrites the billboard's input.
        let mut g = match rig.arms[i] {
            Some(arm) => {
                touched[i] = true;
                GlobalTransform::from(parent_arm_matrix(
                    arm,
                    parent_world.affine(),
                    root_g.affine(),
                    rig.binds[i],
                ))
            }
            None => parent_world,
        }
        .mul_transform(rig.locals[i]);
        if let (Some(kind), Some((fwd, right, up))) = (rig.kinds[i], cam) {
            let (scale, rot, translation) = g.to_scale_rotation_translation();
            g = GlobalTransform::from(Transform {
                translation,
                rotation: billboard_basis(kind, rot, fwd, right, up),
                scale,
            });
            touched[i] = true;
        } else if !touched[i] {
            touched[i] = parent.is_some_and(|p| touched[p]);
        }
        worlds.push(g);
    }
    (worlds, touched)
}

/// Seed a rig's rows from its composed pose when the doodad lane claims its slot mid-frame, as
/// the skinned mesh swaps in that same frame.
pub(crate) fn seed_rig_rows(
    rig: &RigPose,
    root_g: GlobalTransform,
    skin: &RigSkin,
    ibp: &[Mat4],
    palettes: &mut RigPalettes,
) {
    let origin = crate::rig_palette::rebase_origin(root_g.translation());
    let root_rel = crate::rig_palette::rebase_global(root_g, origin);
    let (worlds, _) = rig_worlds(rig, root_rel, None);
    palettes.write_rig_worlds(skin, &worlds, ibp, origin);
}

/// The world pass: palette rows, anchor re-seats and the seat-frame cascade.
#[allow(clippy::type_complexity)]
pub fn finalize_rig_worlds(
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    // A slot-less doodad rig still needs `pose_dirty` cleared and its anchors re-seated; a
    // `RigRider`'s rows are the rider lane's.
    mut rigs: Query<(
        Entity,
        &mut RigPose,
        Option<&RigSkin>,
        Has<AnimParked>,
        Has<crate::rig_rider::RigRider>,
    )>,
    // The `Changed` filter conflicts with the mutable query; the refresh set is collected first.
    mut worlds_params: ParamSet<(
        Query<(), Changed<GlobalTransform>>,
        Query<(&Transform, &mut GlobalTransform), Without<WorldCamera>>,
    )>,
    children: Query<&Children>,
    hosts: Query<&BillboardJointRig>,
    frames: Query<&RigFrame>,
    ibps: Res<Assets<SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<RigPalettes>,
) {
    let cam_basis = cam
        .single()
        .ok()
        .map(|t| (*t.forward(), *t.right(), *t.up()));
    // Refresh a rig that is pose-dirty, whose model frame moved or that billboards, never a parked
    // one: its rows go stale on purpose, and the wake re-raises `pose_dirty` before this runs.
    let refresh: Vec<Entity> = {
        let roots_changed = worlds_params.p0();
        rigs.iter()
            .filter(|(_, rig, _, parked, _)| {
                !parked
                    && (rig.pose_dirty
                        || roots_changed.contains(rig.joints_root)
                        || rig.has_billboard)
            })
            .map(|(holder, ..)| holder)
            .collect()
    };
    if refresh.is_empty() {
        return;
    }
    let cost_t0 = crate::rig_palette::rig_cost_enabled().then(std::time::Instant::now);
    let mut globals = worlds_params.p1();
    // As in the entity pass, the re-walk does not enter a nested rig with its own billboard output.
    let fx_roots: bevy::platform::collections::HashSet<Entity> =
        hosts.iter().map(|r| r.root()).collect();
    let mut cascade: Vec<Entity> = Vec::new();
    let mut finalize =
        |rig: &mut RigPose,
         skin: Option<&RigSkin>,
         globals: &mut Query<(&Transform, &mut GlobalTransform), Without<WorldCamera>>,
         cascade: &mut Vec<Entity>,
         root_moved: bool| {
            // A slot-less rig without special bones is done; the caller clears `pose_dirty`.
            if skin.is_none() && !rig.has_special && !root_moved {
                return;
            }
            let Ok(root_g) = globals.get(rig.joints_root).map(|(_, g)| *g) else {
                return;
            };
            // Rig-relative, which translation-equivariant steps allow: at ~9.5 k yards an f32 ULP
            // (~1 mm) per matmul, fresh each frame, shimmers. `WOW_NO_RIG_REBASE=1` turns it off.
            let origin = crate::rig_palette::rebase_origin(root_g.translation());
            let root_rel = crate::rig_palette::rebase_global(root_g, origin);
            let (worlds, touched) = rig_worlds(rig, root_rel, cam_basis);
            if let Some(skin) = skin {
                if let Some(ibp) = ibps.get(&skin.ibp) {
                    palettes.write_rig_worlds(skin, &worlds, ibp, origin);
                }
            }
            if !rig.has_special && !root_moved {
                return;
            }
            // Re-seat the anchors in replaced subtrees, or every anchor when a rider's seat moved
            // after propagation, and re-compose their rigid children.
            let mut stack: Vec<(Entity, GlobalTransform)> = Vec::new();
            for &(bone, anchor) in &rig.anchors {
                let b = bone as usize;
                if !root_moved && !touched.get(b).copied().unwrap_or(false) {
                    continue;
                }
                // Anchors are world-space scene entities; only the palette stays rig-relative.
                let Some(world) = worlds.get(b).map(|&w| shift(w, origin)) else {
                    continue;
                };
                if let Ok((_, mut g)) = globals.get_mut(anchor) {
                    *g = world;
                }
                if let Ok(cs) = children.get(anchor) {
                    stack.extend(
                        cs.iter()
                            .filter(|c| !fx_roots.contains(c))
                            .map(|c| (c, world)),
                    );
                }
            }
            while let Some((e, parent_g)) = stack.pop() {
                if let Ok(rf) = frames.get(e) {
                    cascade.push(rf.0);
                }
                let Ok((local, mut global)) = globals.get_mut(e) else {
                    continue;
                };
                let g = parent_g.mul_transform(*local);
                *global = g;
                if let Ok(cs) = children.get(e) {
                    stack.extend(cs.iter().filter(|c| !fx_roots.contains(c)).map(|c| (c, g)));
                }
            }
        };
    for &holder in &refresh {
        let Ok((_, rig, skin, _, rider)) = rigs.get_mut(holder) else {
            continue;
        };
        let rig = rig.into_inner();
        // A rider's rows are the rider lane's: no skin, so only the anchors.
        let skin = skin.filter(|_| !rider);
        finalize(rig, skin, &mut globals, &mut cascade, false);
        rig.pose_dirty = false;
    }
    // Re-finalize each rig whose seat a patch walk moved; one level deep by construction.
    if !cascade.is_empty() {
        for (holder, rig, skin, _, rider) in &mut rigs {
            if !cascade.contains(&holder) {
                continue;
            }
            let rig = rig.into_inner();
            let mut ignore = Vec::new();
            let skin = skin.filter(|_| !rider);
            finalize(rig, skin, &mut globals, &mut ignore, true);
            rig.pose_dirty = false;
        }
    }
    if let Some(t0) = cost_t0 {
        eprintln!(
            "[rig-finalize] refreshed={} ms={:.3}",
            refresh.len(),
            t0.elapsed().as_secs_f32() * 1000.0
        );
    }
}

/// Register the model pass; [`crate::billboard::BillboardPlugin`] chains the world pass.
pub fn plugin(app: &mut App) {
    app.configure_sets(
        PostUpdate,
        PosePost
            .after(bevy::app::AnimationSystems)
            .before(bevy::transform::TransformSystems::Propagate),
    )
    .add_systems(
        PostUpdate,
        compose_rig_models
            .after(PosePost)
            .before(bevy::transform::TransformSystems::Propagate),
    );
}

#[cfg(test)]
mod tests {
    use benilla_assets::{ModelJoint, ModelSkeleton};
    use benilla_formats::BillboardKind;

    use super::*;

    fn skeleton(joints: Vec<ModelJoint>) -> ModelSkeleton {
        ModelSkeleton {
            joints,
            spine_bone: None,
            head_bone: None,
        }
    }

    fn joint(parent: i16, t: Vec3) -> ModelJoint {
        ModelJoint {
            parent,
            local_translation: t,
            billboard: None,
            parent_arm: None,
        }
    }

    /// `RidingHorse`'s seat, in the model pass: under a swinging spine, the `flags = 0x6` bone
    /// keeps the model's orientation and rides the spine's position (`0x714caf`–`0x714d09`).
    #[test]
    fn a_flags_0x6_seat_bone_discards_the_gallop_before_the_anchors_are_seated() {
        let sk = skeleton(vec![
            joint(-1, Vec3::ZERO),
            joint(0, Vec3::Y), // the spine, which swings
            ModelJoint {
                parent: 1,
                local_translation: Vec3::new(0.0, 0.5, 0.25),
                billboard: None,
                parent_arm: Some(benilla_formats::ParentArm {
                    ignore_translate: false,
                    basis: benilla_formats::ParentBasis::RootBasis,
                }),
            },
        ]);
        let mut rig = RigPose::new(Entity::PLACEHOLDER, &sk);
        for step in 0..16 {
            let swing = (step as f32 - 7.5) * 0.05; // ±21°, the horse's measured stride
            rig.locals[1].rotation = Quat::from_rotation_x(swing);
            rig.compose();
            let (_, seat_rot, seat_pos) = rig.model[2].to_scale_rotation_translation();
            assert!(
                seat_rot.angle_between(Quat::IDENTITY) < 1e-4,
                "the seat keeps the model's orientation at spine swing {swing}: got {seat_rot:?}"
            );
            // It still rides the spine: the pivot is where the animated parent put it.
            let want = rig.model[1].transform_point3(Vec3::new(0.0, 0.5, 0.25));
            assert!(
                (seat_pos - want).length() < 1e-4,
                "the seat travels with the gallop at swing {swing}: {seat_pos} vs {want}"
            );
        }
        // The sweep must actually have moved the seat, or the assertions above are vacuous.
        rig.locals[1].rotation = Quat::from_rotation_x(0.4);
        rig.compose();
        let far = rig.model[2].translation;
        rig.locals[1].rotation = Quat::from_rotation_x(-0.4);
        rig.compose();
        assert!(
            (Vec3::from(far) - Vec3::from(rig.model[2].translation)).length() > 0.1,
            "the spine sweep really does carry the seat"
        );
    }

    #[test]
    fn compose_matches_entity_propagation() {
        let sk = skeleton(vec![
            joint(-1, Vec3::new(1.0, 2.0, 3.0)),
            joint(0, Vec3::Y),
            joint(1, Vec3::X),
        ]);
        let mut rig = RigPose::new(Entity::PLACEHOLDER, &sk);
        rig.locals[0].rotation = Quat::from_rotation_y(0.7);
        rig.locals[1].scale = Vec3::new(2.0, 1.0, 0.5);
        rig.locals[1].rotation = Quat::from_rotation_x(-0.3);
        rig.compose();
        // Compared as matrices: decomposing two identical affines can NaN an `angle_between`.
        let mut g = GlobalTransform::IDENTITY;
        for i in 0..3 {
            g = g.mul_transform(rig.locals[i]);
            let oracle = Mat4::from(g.affine());
            let ours = Mat4::from(rig.model[i]);
            assert!(
                oracle.abs_diff_eq(ours, 1e-5),
                "bone {i}: {oracle} vs {ours}"
            );
        }
    }

    #[test]
    fn world_pass_matches_the_entity_billboard_law() {
        let sk = skeleton(vec![
            ModelJoint {
                parent: -1,
                local_translation: Vec3::new(5.0, 1.0, 0.0),
                billboard: Some(BillboardKind::LockZ),
                parent_arm: None,
            },
            joint(0, Vec3::Y),
            ModelJoint {
                parent: 1,
                local_translation: Vec3::Y,
                billboard: None,
                parent_arm: Some(benilla_formats::ParentArm {
                    ignore_translate: false,
                    basis: benilla_formats::ParentBasis::RootDirection,
                }),
            },
        ]);
        let mut rig = RigPose::new(Entity::PLACEHOLDER, &sk);
        rig.locals[0].scale = Vec3::splat(2.0);
        rig.locals[1].scale = Vec3::splat(0.5);
        rig.locals[1].rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        rig.compose();
        let root_rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        // The identity camera frame: looking down −Z, up Y, right X.
        let cam = (-Vec3::Z, Vec3::X, Vec3::Y);
        let (worlds, touched) = rig_worlds(
            &rig,
            GlobalTransform::from(Transform::from_rotation(root_rot)),
            Some(cam),
        );
        assert_eq!(touched, vec![true; 3], "the whole chain is replaced");
        // Bone 0: pivot at the root-rotated authored spot, scale kept, upright, facing the viewer.
        let (s0, r0, t0) = worlds[0].to_scale_rotation_translation();
        assert!((t0 - root_rot * Vec3::new(5.0, 1.0, 0.0)).length() < 1e-4);
        assert!((s0 - Vec3::splat(2.0)).length() < 1e-5, "scale preserved");
        assert!((r0 * Vec3::Y).dot(Vec3::Y) > 0.999, "kept axis upright");
        assert!((r0 * -Vec3::Z).dot(Vec3::Z) > 0.999, "faces the camera");
        // Bone 1 chains onto the replaced parent: one parent-scaled unit up the new Y.
        let (s1, _, t1) = worlds[1].to_scale_rotation_translation();
        assert!((t1 - (t0 + r0 * (Vec3::Y * 2.0))).length() < 1e-4, "{t1}");
        assert!((s1 - Vec3::ONE).length() < 1e-5, "2 × 0.5 scale chain");
        // Bone 2 (`flags & 0x4`): pivot from the animated parent, basis from the model root.
        let (_, r2, t2) = worlds[2].to_scale_rotation_translation();
        let expect_t = worlds[1].transform_point(Vec3::Y);
        assert!((t2 - expect_t).length() < 1e-4, "pivot rides the parent");
        assert!(
            r2.angle_between(root_rot) < 1e-3,
            "basis comes from the model root, not the parent bone"
        );
    }

    #[test]
    fn plain_rigs_touch_nothing() {
        let sk = skeleton(vec![joint(-1, Vec3::X), joint(0, Vec3::Y)]);
        let mut rig = RigPose::new(Entity::PLACEHOLDER, &sk);
        rig.locals[0].rotation = Quat::from_rotation_z(0.4);
        rig.compose();
        let root = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 7.0));
        let (worlds, touched) = rig_worlds(&rig, root, None);
        assert_eq!(touched, vec![false; 2]);
        for (i, w) in worlds.iter().enumerate() {
            let expect = GlobalTransform::from(root.affine() * rig.model[i]);
            assert!(
                (w.translation() - expect.translation()).length() < 1e-5,
                "bone {i}"
            );
        }
    }

    /// One idling chain composed at Goldshire's coordinates and rig-relative, each scored by how
    /// its error against an f64 oracle changes per frame: at ~9.5 k yards an f32 ULP is ~0.98 mm,
    /// re-rounded every frame; rig-relative it is ~0.1 µm.
    #[test]
    fn the_rebased_chain_does_not_re_round_at_world_scale_every_frame() {
        use bevy::math::{DAffine3, DVec3};

        // Goldshire (`.go xyz -9464 62 56 0`), not integers, as 9464.0 is exact in f32.
        const ROOT: Vec3 = Vec3::new(-9464.31, 62.17, 56.91);
        const AMP: [f32; 6] = [0.010, 0.012, 0.008, 0.015, 0.020, 0.014]; // a breathing idle

        let sk = skeleton(vec![
            joint(-1, Vec3::ZERO),
            joint(0, Vec3::new(0.0, 1.02, 0.0)),
            joint(1, Vec3::new(0.0, 0.34, 0.0)),
            joint(2, Vec3::new(0.21, 0.26, 0.0)),
            joint(3, Vec3::new(0.0, -0.31, 0.0)),
            joint(4, Vec3::new(0.0, -0.29, 0.0)),
        ]);
        let mut rig = RigPose::new(Entity::PLACEHOLDER, &sk);
        let tip = sk.joints.len() - 1;
        // A facing and two-axis swings, or the motion can miss the large world axis.
        let root_yaw = Quat::from_rotation_y(0.9);

        // The same chain in f64, from an identity root: the truth both routes are scored against.
        let oracle = |rig: &RigPose| -> DVec3 {
            let mut w: Vec<DAffine3> = Vec::with_capacity(rig.locals.len());
            for i in 0..rig.locals.len() {
                let l = DAffine3::from_scale_rotation_translation(
                    rig.locals[i].scale.as_dvec3(),
                    rig.locals[i].rotation.as_dquat(),
                    rig.locals[i].translation.as_dvec3(),
                );
                w.push(
                    match usize::try_from(rig.parents[i]).ok().filter(|&p| p < i) {
                        Some(p) => w[p] * l,
                        None => l,
                    },
                );
            }
            w[tip].translation
        };

        let root_abs =
            GlobalTransform::from(Transform::from_translation(ROOT).with_rotation(root_yaw));
        let root_rel = GlobalTransform::from(Transform::from_rotation(root_yaw));
        let (mut abs_err, mut rel_err): (Vec<DVec3>, Vec<DVec3>) = (Vec::new(), Vec::new());
        let mut truths: Vec<DVec3> = Vec::new();
        for step in 0..180 {
            let phase = std::f32::consts::TAU * step as f32 / 90.0; // a 1.5 s idle loop at 60 fps
            for (i, amp) in AMP.iter().enumerate() {
                let a = phase + i as f32 * 0.7;
                rig.locals[i].rotation =
                    Quat::from_rotation_x(amp * a.sin()) * Quat::from_rotation_z(amp * a.cos());
            }
            let truth = root_yaw.as_dquat() * oracle(&rig);
            truths.push(truth);
            let abs = rig_worlds(&rig, root_abs, None).0[tip].translation();
            let rel = rig_worlds(&rig, root_rel, None).0[tip].translation();
            abs_err.push(abs.as_dvec3() - (truth + ROOT.as_dvec3()));
            rel_err.push(rel.as_dvec3() - truth);
        }
        let jitter = |e: &[DVec3]| e.windows(2).map(|w| w[1].distance(w[0])).sum::<f64>() / 179.0;
        let (abs_j, rel_j) = (jitter(&abs_err), jitter(&rel_err));
        // The tip's real per-frame motion, the yardstick for the noise.
        let signal = jitter(&truths);

        // Measured with `--nocapture` (yards): abs_j 7.4e-4, rel_j 1.0e-7, signal 1.6e-3.
        eprintln!(
            "abs_err mean={:.3e} rel_err mean={:.3e} abs_j={abs_j:.3e} rel_j={rel_j:.3e} \
             signal={signal:.3e} noise/signal abs={:.2} rel={:.5}",
            abs_err.iter().map(|e| e.length()).sum::<f64>() / 180.0,
            rel_err.iter().map(|e| e.length()).sum::<f64>() / 180.0,
            abs_j / signal,
            rel_j / signal,
        );
        // The idle must move the tip, or every number above measures nothing.
        assert!(
            signal > 1e-4,
            "the idle really does move the tip: {signal:.3e}"
        );
        assert!(
            abs_j > 3e-4,
            "the absolute route spends a world-scale ULP per frame — got {abs_j:.3e} yd. If this \
             ever fails, the rig-relative rebase has lost its reason here (or the rig above has \
             drifted into missing the large world axis again) — re-derive before deleting."
        );
        assert!(
            abs_j / signal > 0.1,
            "…and it is a large fraction of the real motion, which is why it READS as shake: \
             {:.2}",
            abs_j / signal
        );
        assert!(
            rel_j < 1e-6,
            "the rebased chain holds sub-micron — got {rel_j:.3e} yd"
        );
        assert!(
            abs_j / rel_j > 100.0,
            "the rebase is worth ≥2 orders of magnitude: {abs_j:.3e} vs {rel_j:.3e} yd/frame"
        );
    }
}
