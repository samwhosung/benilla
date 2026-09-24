//! The collapsed rig's two composition passes (decision 0724) — what transform propagation and
//! the billboard joint pass used to do through ~59 k bone entities, done on the [`RigPose`]
//! arrays instead.
//!
//! **Model pass** ([`compose_rig_models`], pre-propagation): a pose-dirty rig forward-folds its
//! `locals` into model-space affines — **including the `flags & 0x7` parent-matrix arm**, which
//! needs nothing but model space (`RigPose::compose`) — and re-seats its consumer **anchors**
//! (children of the rig's `joints_root`) from them, so ordinary propagation carries every consumer
//! subtree (held items, nested effect rigs, the mount seat) at this frame's pose, exactly as the
//! joint hierarchy did. Runs after [`PosePost`] (body twist, global sequences — the writers that
//! follow the evaluator).
//!
//! **World pass** ([`finalize_rig_worlds`], post-propagation, inside
//! [`crate::billboard::BillboardPlace`]): per rig needing it — pose-dirty, `joints_root` moved,
//! or camera-faced — compose the chain `world_from_model × local…` **with the root's translation
//! zeroed** (decision 0974: the whole chain runs rig-relative, because composing it at the map's
//! ~9.5 k-yard coordinates spent an f32 ULP — ~1 mm — per matmul, freshly every frame), apply the
//! byte-law bone replacements (`billboard_joint_palette`'s math verbatim: the arm again, in its
//! world-space form, then billboard kinds take the camera basis, and descendants chain onto the
//! replaced frames), write the palette rows (`frame × inverse_bindpose`, decision 0720) beside the
//! rig's world origin (which the vertex stage adds back camera-relative), and re-seat the
//! anchors sitting on replaced subtrees — including the same rigid-child re-walk and the same
//! nested-rig do-not-enter rule as the entity pass. A re-seated subtree that carries another
//! rig's model frame (the mounted rider's seat anchor, [`RigFrame`]) cascades: that rig
//! re-finalizes in the same pass, so its palette never lags its seat.
//!
//! The arm appearing in BOTH passes is not a double application — each pass builds its own chain
//! once, in its own space, and the two agree (see `RigPose::compose`). What it is NOT allowed to
//! be is world-pass-only: the anchors are seated from the model pass, so an arm that ran only
//! afterwards left every attached subtree riding the un-armed frame (decision 0945).
//!
//! A parked, stationary rig runs neither pass and uploads zero bytes (decision 0448's park
//! observables carry over unchanged).

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use crate::billboard::{billboard_basis, parent_arm_matrix, BillboardJointRig};
use crate::rig_palette::{RigPalettes, RigSkin};
use crate::view::WorldCamera;

use super::{AnimParked, RigFrame, RigPose};

/// The pose post-pass window: every writer of [`RigPose`] locals that runs after the evaluator
/// (the body twist, the global-sequence channels) is a member; the model compose runs after the
/// whole set. Configured after [`bevy::app::AnimationSystems`], before transform propagation.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PosePost;

/// Pre-propagation: fold each pose-dirty rig's locals into model-space affines and re-seat its
/// anchors' local `Transform`s, so this frame's propagation places every consumer subtree at
/// this frame's pose. `pose_dirty` stays raised — the world pass consumes and clears it.
///
/// A PARKED rig skips even this (decision 1365): the doodad lane's hosts are born pose-dirty and
/// spend most of their residency parked with the flag still raised (the world pass, which clears
/// it, also skips parked rigs), so an unfiltered pass re-composed every parked host's bind pose
/// every frame. Nothing observable is lost — a parked rig's anchors were seated by its last
/// composed frame (or by `RigPose::new`/`anchor_for` at bind pose), its consumers are out of the
/// draw set by the same gate that parked it, and the wake drops the marker before this pass runs,
/// so the first unparked frame composes the current pose before anything reads it.
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

/// Translate a composed rig-relative frame back into world space (decision 0974).
fn shift(g: GlobalTransform, origin: Vec3) -> GlobalTransform {
    let mut a = g.affine();
    a.translation += bevy::math::Vec3A::from(origin);
    GlobalTransform::from(a)
}

/// One rig's chain + which bones sit in a replaced subtree, composed **in the root's frame**: the
/// caller passes `root_g` with its translation zeroed, so every frame out is rig-relative
/// (decision 0974) and the whole chain runs at rig-sized magnitudes. `root_g` is otherwise the
/// rig's `joints_root` propagated world — the model's own root frame `[model+0xfc]`, which is both
/// what the `flags & 0x7` arm rebuilds a parent matrix out of and, for a mounted rider, its seat
/// anchor (`0x714389`). `cam` is the camera basis (`None` = no camera, so only the arm applies).
/// The math mirrors `billboard_joint_palette` operation for operation — rewrite the parent matrix,
/// compose the local, then face — so the collapsed lane is bit-compatible with the entity lane.
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
        // `flags & 0x7` first (`0x714961`): it changes the INPUT the billboard law is applied
        // to, and the billboard switch below still runs.
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

/// Seed one collapsed rig's palette rows from its CURRENT composed pose — the doodad lane's
/// lazy-promote edge (decision 1365, the 0863 wake): the slot is claimed mid-frame and the
/// skinned mesh swaps in the same frame, so the rows must be written NOW or the first skinned
/// frame renders zeroed (origin-collapsed) rows. No camera basis — a billboard bone seeds at its
/// composed pose and the world pass re-faces it this same frame (the promoted host is unparked
/// and `has_billboard` puts it in the refresh set).
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

/// Post-propagation (inside `BillboardPlace`, chained after the entity lane's
/// `billboard_joint_palette` and before `face_billboards`): finalize every rig that needs it —
/// palette rows + replaced-subtree anchor re-seats, with the seat-frame cascade.
#[allow(clippy::type_complexity)] // billboard_joint_palette's shape
pub fn finalize_rig_worlds(
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    // `Option<&RigSkin>`, not `&RigSkin` (decision 1365): the doodad lane's rigs hold their
    // palette slot lazily (allocated at first wake, reaped under pressure), and a slot-less rig
    // still needs this pass for two things — clearing `pose_dirty` (or the model pass re-composes
    // it every frame for ever) and, when it authors billboard/arm bones, the camera-dependent
    // anchor re-seat its cards and emitters ride. The palette write alone is skipped.
    // `Has<RigRider>`: a rider owns its slot's rows from the OTHER end (decision 1609 — the
    // placement is composed in the HOST's rig frame, never from this absolute one), and decision
    // 2281 gave that lane a posed arm, so an animated ranged prop arrives here with both a pose
    // and a skin and must still not have its rows written twice from two different frames.
    // Everything else this pass does for it — clearing `pose_dirty`, re-seating the anchors its
    // emitters ride — is exactly what it needs.
    mut rigs: Query<(
        Entity,
        &mut RigPose,
        Option<&RigSkin>,
        Has<AnimParked>,
        Has<crate::rig_rider::RigRider>,
    )>,
    // B0001: the `Changed` filter reads `GlobalTransform` ticks, which conflicts with the
    // mutable frame query — a `ParamSet` sequences them (the refresh set is collected first).
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
    // Which rigs refresh this frame: pose-dirty, model frame moved, or camera-faced — and never
    // a parked one (decision 0739). The LOD gate ruled a parked rig un-viewable, so neither an
    // idle's pose_dirty, a patrol's root motion, nor a billboard face can matter until the wake
    // — which drops the marker before the same frame's pose evaluation, re-raising `pose_dirty`
    // before this pass runs, so a woken rig's rows are current before anything draws them. (The
    // world-space rows of a moving parked rig go stale on purpose — nothing reads them; at the
    // LBRS pin the parked-but-patrolling re-writes were ~260 of the ~620 per-frame refreshes.)
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
    // The `WOW_RIG_COST` compose meter (decision 0736): how many rigs refresh and what the
    // whole finalize pass costs, beside the palette's copy/write and upload lines.
    let cost_t0 = crate::rig_palette::rig_cost_enabled().then(std::time::Instant::now);
    let mut globals = worlds_params.p1();
    // The rigid-child re-walk's do-not-enter set, exactly the entity pass's: a nested rig with
    // its own billboard output owns its interior (and its root keeps the propagated frame).
    let fx_roots: bevy::platform::collections::HashSet<Entity> =
        hosts.iter().map(|r| r.root()).collect();
    // Model frames re-seated by a patch walk → the rigs riding them re-finalize below.
    let mut cascade: Vec<Entity> = Vec::new();
    let mut finalize =
        |rig: &mut RigPose,
         skin: Option<&RigSkin>,
         globals: &mut Query<(&Transform, &mut GlobalTransform), Without<WorldCamera>>,
         cascade: &mut Vec<Entity>,
         root_moved: bool| {
            // A slot-less rig with no special bones has nothing left for this pass: no rows to
            // write, and its anchors were placed by the model pass + ordinary propagation. The
            // caller still clears `pose_dirty` — which is the point of arriving here at all.
            if skin.is_none() && !rig.has_special && !root_moved {
                return;
            }
            let Ok(root_g) = globals.get(rig.joints_root).map(|(_, g)| *g) else {
                return;
            };
            // **The chain is composed RIG-RELATIVE** (decision 0974): same root basis, translation
            // zeroed. Every operation below — the `flags & 0x7` arm, the local compose, the
            // billboard replacement — is translation-equivariant (the arm is 3×3 work plus a
            // pivot-preserving translation; the billboard rewrites rotation only), so this yields
            // exactly the world chain shifted by `−origin`, at rig-sized magnitudes. That is the
            // fix: composing at Elwynn's ~9.5 k yards spent an f32 ULP (~1 mm) per matmul, freshly
            // every frame because the pose is recomposed every frame, and that is the shimmer.
            // (`WOW_NO_RIG_REBASE=1` makes this the map origin — the A/B lever for what the
            // rebase is worth; see `rig_palette::rebase_origin`.)
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
            // Anchors in replaced subtrees: re-seat their globals and re-compose their rigid
            // children from the replaced frames — the entity pass's child walk, anchor-rooted.
            //
            // `root_moved` widens that to EVERY anchor. The touched-only rule assumes propagation
            // already placed an untouched anchor correctly, and that assumption dies the moment
            // this rig's own model frame is re-seated after propagation ran: a mounted rider's
            // seat is patched here, so every one of its anchors is standing on the pre-patch
            // frame. Its palette rows are rebuilt from `root_g` either way, which is why the BODY
            // looked right while its attached items rode the stale seat — the mounted pauldron
            // swinging on the gallop while the shoulder under it did not (decision 0945).
            let mut stack: Vec<(Entity, GlobalTransform)> = Vec::new();
            for &(bone, anchor) in &rig.anchors {
                let b = bone as usize;
                if !root_moved && !touched.get(b).copied().unwrap_or(false) {
                    continue;
                }
                // Anchors are ordinary scene-graph entities: their `GlobalTransform` is world
                // space and every consumer (propagation, colliders, the effect lane, item
                // placement) reads it as such — so the rig-relative frame goes back to absolute
                // here. Only the PALETTE stays relative; that is the whole seam (decision 0974).
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
        // A rider's rows are the rider lane's (see the query's note): hand `finalize` no skin so
        // it does the anchors and nothing else.
        let skin = skin.filter(|_| !rider);
        finalize(rig, skin, &mut globals, &mut cascade, false);
        rig.pose_dirty = false;
    }
    // The cascade: a patch walk above moved some rig's model frame after it (or before it) ran —
    // re-finalize against the fresh seat. One level deep by construction (a seat anchor's
    // subtree holds no further seat anchors).
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

/// Register the model pass; the world pass is chained by [`crate::billboard::BillboardPlugin`]
/// between the entity lane's joint pass and the card facing, where its readers sit.
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

    /// **The mount seat, in the pass that decides it** — `compose`, pre-propagation, which is what
    /// consumer anchors (the rider's seat, and every item hanging off the rider) are re-seated
    /// from. The shape is `RidingHorse`'s: a spine bone that swings with the gallop, and the seat
    /// bone under it carrying `flags = 0x6`.
    ///
    /// The seat must come out with the MODEL's orientation whatever the spine does, while still
    /// being carried to the position the spine put it at — that is the byte law's pivot-preserving
    /// tail (`0x714caf`–`0x714d09`), and it is the whole reason a galloping horse bobs its rider
    /// without rocking them. Asserted across a swept spine angle, because a single
    /// sample cannot tell a discarded rotation from a lucky one.
    ///
    /// Pinning it HERE and not only in the world pass is the point of the regression: the arm was
    /// applied post-propagation first, the anchors were seated from the un-armed matrix, and the
    /// mounted pauldron swung 53° over the stride while the shoulder it hung off stood still.
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
            // …and it still RIDES the spine: the pivot is where the animated parent put it.
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

    /// The model compose is the exact affine product entity propagation computed: a three-bone
    /// chain with rotation + non-uniform scale folds to the same matrices as chained
    /// `GlobalTransform::mul_transform`.
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
        // The oracle: GlobalTransform chaining from an identity root, exactly what propagation
        // did through the joint entities. Compared as matrices — decomposing two identical
        // affines can NaN an `angle_between` on a dot marginally past 1.
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

    /// The world pass reproduces `billboard_joint_palette`'s replacements: a lock-Z billboard
    /// bone takes the camera basis (pivot/scale kept), its child chains onto the replaced frame,
    /// and a `flags & 0x4` helper takes the model root's basis while its pivot still rides the
    /// animated parent — the entity pass's three verified behaviours, computed from the arrays.
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
        // Bone 0: pivot at the root-rotated authored spot, scale kept, lock-Z basis — upright
        // kept axis, local −Z toward the viewer (the billboard.rs test's assertions).
        let (s0, r0, t0) = worlds[0].to_scale_rotation_translation();
        assert!((t0 - root_rot * Vec3::new(5.0, 1.0, 0.0)).length() < 1e-4);
        assert!((s0 - Vec3::splat(2.0)).length() < 1e-5, "scale preserved");
        assert!((r0 * Vec3::Y).dot(Vec3::Y) > 0.999, "kept axis upright");
        assert!((r0 * -Vec3::Z).dot(Vec3::Z) > 0.999, "faces the camera");
        // Bone 1 chains onto the REPLACED parent: one parent-scaled unit up the new Y.
        let (s1, _, t1) = worlds[1].to_scale_rotation_translation();
        assert!((t1 - (t0 + r0 * (Vec3::Y * 2.0))).length() < 1e-4, "{t1}");
        assert!((s1 - Vec3::ONE).length() < 1e-5, "2 × 0.5 scale chain");
        // Bone 2 (flags & 0x4): pivot carried by the animated parent's frame, basis taken from
        // the model root — the parent here is unit-scaled, so the ratio leg reproduces it exactly.
        let (_, r2, t2) = worlds[2].to_scale_rotation_translation();
        let expect_t = worlds[1].transform_point(Vec3::Y);
        assert!((t2 - expect_t).length() < 1e-4, "pivot rides the parent");
        assert!(
            r2.angle_between(root_rot) < 1e-3,
            "basis comes from the model root, not the parent bone"
        );
    }

    /// An unreplaced chain has no touched bones and composes root × model verbatim — the common
    /// rig costs the palette rows and nothing else.
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

    /// **The jitter pin** (decision 0974) — the measurement the rebase exists for.
    ///
    /// Compose the same idling six-bone chain frame by frame two ways — rooted at Goldshire's real
    /// world coordinates, and rooted at the rig's own origin with the position carried alongside —
    /// and score each against an f64 oracle of the identical chain. What matters is not the error
    /// but its **frame-to-frame variation**: a constant offset is invisible, a per-frame one is
    /// shimmer. The absolute route re-rounds to the f32 grid at ~9.5 k yards (1 ULP ≈ 0.98 mm)
    /// every frame *because the pose changes every frame* — that is the long-standing "characters
    /// look jittery up close" the director reported, and terrain is exempt only because it doesn't
    /// move. The rebased chain runs at rig-sized magnitudes where an ULP is ~0.1 µm.
    ///
    /// Pinned as a ratio and an absolute ceiling: someone re-rooting this chain in world space
    /// again — the natural-looking simplification — puts the shimmer straight back.
    #[test]
    fn the_rebased_chain_does_not_re_round_at_world_scale_every_frame() {
        use bevy::math::{DAffine3, DVec3};

        // Goldshire. `.go xyz -9464 62 56 0` — near the worst |coord| the 1.12 maps reach, and
        // exactly where the report came from. Deliberately not integers: 9464.0 is exact in f32.
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
        // A FACING, and bones that swing on two axes. Without both, a chain whose motion happens to
        // miss the large world axis measures almost nothing — an all-about-X rig at this root left
        // the ~9.5 k x-coordinate static and scored 40× better than it has any right to. A real
        // skeleton's limbs move in all three world axes; the pin has to too.
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
        // The real per-frame motion of the tip — what the noise above has to be read against.
        let signal = jitter(&truths);

        // `--nocapture` prints the measurement this pin was written from (yards):
        //   abs_err 5.2e-4  rel_err 9.6e-8 | abs_j 7.4e-4  rel_j 1.0e-7 | signal 1.6e-3
        // i.e. 0.74 mm of FRESH noise per frame against 1.65 mm of real motion — 45 % of a slow
        // idle's apparent movement was rounding — and 1 Å after the rebase.
        eprintln!(
            "abs_err mean={:.3e} rel_err mean={:.3e} abs_j={abs_j:.3e} rel_j={rel_j:.3e} \
             signal={signal:.3e} noise/signal abs={:.2} rel={:.5}",
            abs_err.iter().map(|e| e.length()).sum::<f64>() / 180.0,
            rel_err.iter().map(|e| e.length()).sum::<f64>() / 180.0,
            abs_j / signal,
            rel_j / signal,
        );
        // The sweep has to actually move the tip, or every number above is measuring nothing.
        assert!(
            signal > 1e-4,
            "the idle really does move the tip: {signal:.3e}"
        );
        assert!(
            abs_j > 3e-4,
            "the absolute route spends a world-scale ULP per frame — got {abs_j:.3e} yd. If this \
             ever fails, decision 0974's premise has stopped holding here (or the rig above has \
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
