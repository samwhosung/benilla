//! The M2 pose evaluator: poses each rig from its `AnimationPlayer` and the baked [`PoseSource`],
//! matching `bevy_animation` 0.18.1's `animate_targets` bit for bit. Per bone and property, the
//! contributing nodes (weight not 0, mask clear, channel keyed) fold in ascending node index by
//! `acc = interpolate(acc, v, w / total)`, and an unkeyed property keeps its value.

use benilla_assets::{ModelAnimations, ModelSkeleton, PoseSource};
use bevy::animation::animatable::Animatable;
use bevy::app::AnimationSystems;
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use super::AnimParked;
use crate::vis_chain::VisChainOnly;

/// A rig's pose buffer, one local per bone in skeleton order, on the `AnimationPlayer`'s entity;
/// writers raise [`Self::pose_dirty`], and `compose` folds the locals into model space.
#[derive(Component)]
pub struct RigPose {
    /// The model frame: the holder, its conform node or a mounted rider's seat anchor.
    pub joints_root: Entity,
    /// Per-bone local TRS, in skeleton order.
    pub locals: Vec<Transform>,
    /// Per-bone model-space affines, `flags & 0x7` arm included; the anchors are seated from these.
    pub model: Vec<Affine3A>,
    pub parents: Vec<i16>,
    /// The bone's billboard flag (`0x08/0x10/0x20/0x40`), faced to the camera by the world pass.
    pub(crate) kinds: Vec<Option<benilla_formats::BillboardKind>>,
    /// Bone flags `0x1/0x2/0x4`: how the parent matrix is rebuilt from the model root.
    pub(crate) arms: Vec<Option<benilla_formats::ParentArm>>,
    /// Bind-pose local translations, the pivots the arm preserves; `locals` hold the animated ones.
    pub(crate) binds: Vec<Vec3>,
    pub(crate) has_billboard: bool,
    /// A bone billboards or carries a parent arm, so the world pass's override walk runs.
    pub(crate) has_special: bool,
    /// Consumer anchors by bone, children of [`Self::joints_root`], re-seated by the compose pass.
    pub anchors: Vec<(u16, Entity)>,
    /// `locals` changed since the last world pass; starts raised so the bind pose is written.
    pub pose_dirty: bool,
}

impl RigPose {
    /// The rig at bind pose, composed; anchors come from [`Self::anchor_for`].
    pub fn new(joints_root: Entity, skeleton: &ModelSkeleton) -> Self {
        let locals: Vec<Transform> = skeleton
            .joints
            .iter()
            .map(|j| Transform::from_translation(j.local_translation))
            .collect();
        let mut rig = Self {
            joints_root,
            model: vec![Affine3A::IDENTITY; locals.len()],
            locals,
            parents: skeleton.joints.iter().map(|j| j.parent).collect(),
            kinds: skeleton.joints.iter().map(|j| j.billboard).collect(),
            arms: skeleton.joints.iter().map(|j| j.parent_arm).collect(),
            binds: skeleton
                .joints
                .iter()
                .map(|j| j.local_translation)
                .collect(),
            has_billboard: skeleton.joints.iter().any(|j| j.billboard.is_some()),
            has_special: skeleton
                .joints
                .iter()
                .any(|j| j.billboard.is_some() || j.parent_arm.is_some()),
            anchors: Vec::new(),
            pose_dirty: true,
        };
        rig.compose();
        rig
    }

    /// Fold `locals` into `model`, the `flags & 0x7` arm included since the anchors are seated
    /// from it; a model frame is a similarity, so this agrees with the world pass's arm. M2 bones
    /// are parent-sorted; a malformed child whose parent follows it composes from the root.
    pub(crate) fn compose(&mut self) {
        for i in 0..self.locals.len() {
            let local = self.locals[i].compute_affine();
            let parent = match usize::try_from(self.parents[i]).ok().filter(|&p| p < i) {
                Some(p) => self.model[p],
                None => Affine3A::IDENTITY,
            };
            self.model[i] = match self.arms[i] {
                Some(arm) => crate::billboard::parent_arm_matrix(
                    arm,
                    parent,
                    Affine3A::IDENTITY,
                    self.binds[i],
                ),
                None => parent,
            } * local;
        }
    }

    /// For a rig drawn by an off-world camera (a portrait booth): drops the billboard kinds, which
    /// the world pass would face to the `WorldCamera`; the camera-free parent arms stay.
    pub fn without_camera_billboards(mut self) -> Self {
        self.kinds.iter_mut().for_each(|k| *k = None);
        self.has_billboard = false;
        self.has_special = self.arms.iter().any(Option::is_some);
        self
    }

    /// The anchor entity for `bone`, spawned on first demand at the current composed pose under
    /// [`Self::joints_root`]; `rig` is the [`RigPose`] holder.
    pub fn anchor_for(
        &mut self,
        commands: &mut Commands,
        rig: Entity,
        bone: u16,
    ) -> Option<Entity> {
        if let Some(&(_, anchor)) = self.anchors.iter().find(|&&(b, _)| b == bone) {
            return Some(anchor);
        }
        let m = self.model.get(bone as usize)?;
        let (scale, rotation, translation) = m.to_scale_rotation_translation();
        // Chain-only visibility: the anchor draws nothing, but its subtree inherits the hide.
        let anchor = commands
            .spawn((
                Transform {
                    translation,
                    rotation,
                    scale,
                },
                Visibility::default(),
                RigAnchor { rig, bone },
            ))
            .vis_chain_only()
            .id();
        commands.entity(self.joints_root).add_child(anchor);
        self.anchors.push((bone, anchor));
        Some(anchor)
    }

    /// The world point `offset` under `bone` at the composed pose, without an anchor entity; it
    /// skips the camera billboard, which no attachment point or event marker sits on.
    pub fn posed_point(
        &self,
        root_global: &GlobalTransform,
        bone: u16,
        offset: Vec3,
    ) -> Option<Vec3> {
        let m = self.model.get(bone as usize)?;
        Some(
            root_global
                .affine()
                .transform_point3(m.transform_point3(offset)),
        )
    }
}

/// On every consumer anchor: the rig and bone it stands for. The body twist's model-frame walk
/// splices through it (a mounted rider's chain crosses the mount's seat bone).
#[derive(Component)]
pub struct RigAnchor {
    /// The rig holder, the entity carrying [`RigPose`].
    pub rig: Entity,
    pub bone: u16,
}

/// On a rig's `joints_root` inside another rig's anchor subtree (a rider's seat anchor under the
/// mount's attachment-0 anchor): the world pass re-finalizes the rig after re-seating the subtree.
#[derive(Component)]
pub struct RigFrame(pub Entity);

/// One active animation's evaluation inputs, resolved against the rig's [`PoseSource`].
struct Active {
    /// `AnimationNodeIndex::index()`, the sort key that reproduces Bevy's blend order.
    node: usize,
    clip: usize,
    mask: u64,
    weight: f32,
    seek: f32,
    /// The merge walk's position in the clip's bone list.
    cursor: usize,
}

/// Pose every live rig where `animate_targets` runs, after `advance_animations`.
fn evaluate_rig_poses(
    mut rigs: Query<(&AnimationPlayer, &ModelAnimations, &mut RigPose), Without<AnimParked>>,
) {
    // Parallel: a rig reads shared clips and writes only its own pose.
    rigs.par_iter_mut().for_each(|(player, anims, mut rig)| {
        let src = &anims.pose;
        // The contributing animations in node order, in one scratch per worker thread.
        thread_local! {
            static ACTIVE: std::cell::RefCell<Vec<Active>> =
                const { std::cell::RefCell::new(Vec::new()) };
        }
        ACTIVE.with(|cell| {
            let mut active = cell.borrow_mut();
            active.clear();
            for (&node, anim) in player.playing_animations() {
                // Bevy's exact gate: a weight of literal 0.0 skips the node (paused does not).
                if anim.weight() == 0.0 {
                    continue;
                }
                let Some(pn) = src.node(node) else { continue };
                if src.clips.get(pn.clip as usize).is_none() {
                    continue;
                }
                active.push(Active {
                    node: node.index(),
                    clip: pn.clip as usize,
                    mask: pn.mask,
                    weight: anim.weight(),
                    seek: anim.seek_time(),
                    cursor: 0,
                });
            }
            active.sort_unstable_by_key(|a| a.node);
            match active.as_mut_slice() {
                [] => {}
                [one] => {
                    // One contribution commits at full value whatever its weight.
                    rig.pose_dirty = true;
                    for pb in &src.clips[one.clip].bones {
                        if src.bone_masks.get(pb.bone as usize).copied().unwrap_or(0) & one.mask
                            != 0
                        {
                            continue;
                        }
                        let Some(tf) = rig.locals.get_mut(pb.bone as usize) else {
                            continue;
                        };
                        if let Some(v) = pb.translation.sample(one.seek) {
                            tf.translation = v;
                        }
                        if let Some(v) = pb.rotation.sample(one.seek) {
                            tf.rotation = v;
                        }
                        if let Some(v) = pb.scale.sample(one.seek) {
                            tf.scale = v;
                        }
                    }
                }
                many => {
                    rig.pose_dirty = true;
                    blend_rig(src, many, &mut rig);
                }
            }
        });
    });
}

/// Several animations: merge-walk the bone-sorted clips, folding each property in node order.
fn blend_rig(src: &PoseSource, active: &mut [Active], rig: &mut RigPose) {
    while let Some(bone) = active
        .iter()
        .filter_map(|a| src.clips[a.clip].bones.get(a.cursor).map(|b| b.bone))
        .min()
    {
        let bone_mask = src.bone_masks.get(bone as usize).copied().unwrap_or(0);
        let mut translation: Option<(Vec3, f32)> = None;
        let mut rotation: Option<(Quat, f32)> = None;
        let mut scale: Option<(Vec3, f32)> = None;
        for a in active.iter_mut() {
            let Some(pb) = src.clips[a.clip]
                .bones
                .get(a.cursor)
                .filter(|b| b.bone == bone)
            else {
                continue;
            };
            a.cursor += 1;
            if bone_mask & a.mask != 0 {
                continue; // masked out: no contribution, but the cursor advanced
            }
            fold(&mut translation, pb.translation.sample(a.seek), a.weight);
            fold(&mut rotation, pb.rotation.sample(a.seek), a.weight);
            fold(&mut scale, pb.scale.sample(a.seek), a.weight);
        }
        if translation.is_none() && rotation.is_none() && scale.is_none() {
            continue;
        }
        let Some(tf) = rig.locals.get_mut(bone as usize) else {
            continue;
        };
        if let Some((v, _)) = translation {
            tf.translation = v;
        }
        if let Some((v, _)) = rotation {
            tf.rotation = v;
        }
        if let Some((v, _)) = scale {
            tf.scale = v;
        }
    }
}

/// Bevy's blend register: the first contribution lands whole, each later one at `w / total`.
#[inline]
fn fold<T: Animatable>(register: &mut Option<(T, f32)>, sample: Option<T>, weight: f32) {
    let Some(v) = sample else { return };
    *register = Some(match register.take() {
        None => (v, weight),
        Some((acc, total)) => {
            let total = total + weight;
            (T::interpolate(&acc, &v, weight / total), total)
        }
    });
}

/// Register the evaluator in `animate_targets`' window, before the pose post-passes.
pub fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        evaluate_rig_poses
            .in_set(AnimationSystems)
            .after(bevy::animation::advance_animations)
            .before(TransformSystems::Propagate),
    );
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use benilla_assets::{bone_target_id, PoseBone, PoseClip, PoseTrack};
    use bevy::animation::animation_curves::{AnimatableCurve, AnimatableKeyframeCurve};
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
    use bevy::animation::transition::AnimationTransitions;
    use bevy::animation::{animated_field, AnimatedBy, AnimationClip};

    use super::*;

    #[derive(Clone, Default)]
    struct ClipSpec {
        bones: Vec<(u16, BoneSpec)>,
        /// The `add_clip_with_mask` mask (0 = unmasked).
        mask: u64,
    }
    #[derive(Clone, Default)]
    struct BoneSpec {
        t: Vec<(f32, Vec3)>,
        r: Vec<(f32, Quat)>,
        s: Vec<(f32, Vec3)>,
    }

    /// Build the graph, clips and pose source from specs, as `m2.rs` does.
    fn build(
        app: &mut App,
        specs: &[ClipSpec],
        bone_masks: Vec<u64>,
    ) -> (Handle<AnimationGraph>, Vec<AnimationNodeIndex>, PoseSource) {
        let mut graph = AnimationGraph::new();
        let root = graph.root;
        let mut pose = PoseSource {
            bone_masks,
            ..Default::default()
        };
        // Mirror the graph's mask groups from the baked bone bits (group g ⇔ bit g).
        for (i, &bits) in pose.bone_masks.iter().enumerate() {
            for g in 0..8 {
                if bits & (1 << g) != 0 {
                    graph.add_target_to_mask_group(bone_target_id(i as u16), g);
                }
            }
        }
        let mut nodes = Vec::new();
        for spec in specs {
            let mut clip = AnimationClip::default();
            let mut pclip = PoseClip::default();
            for (bone, bs) in &spec.bones {
                let target = bone_target_id(*bone);
                if bs.t.len() >= 2 {
                    clip.add_curve_to_target(
                        target,
                        AnimatableCurve::new(
                            animated_field!(Transform::translation),
                            AnimatableKeyframeCurve::new(bs.t.iter().copied()).unwrap(),
                        ),
                    );
                }
                if bs.r.len() >= 2 {
                    clip.add_curve_to_target(
                        target,
                        AnimatableCurve::new(
                            animated_field!(Transform::rotation),
                            AnimatableKeyframeCurve::new(bs.r.iter().copied()).unwrap(),
                        ),
                    );
                }
                if bs.s.len() >= 2 {
                    clip.add_curve_to_target(
                        target,
                        AnimatableCurve::new(
                            animated_field!(Transform::scale),
                            AnimatableKeyframeCurve::new(bs.s.iter().copied()).unwrap(),
                        ),
                    );
                }
                pclip.push(PoseBone {
                    bone: *bone,
                    translation: if bs.t.len() >= 2 {
                        PoseTrack::new(&bs.t)
                    } else {
                        PoseTrack::default()
                    },
                    rotation: if bs.r.len() >= 2 {
                        PoseTrack::new(&bs.r)
                    } else {
                        PoseTrack::default()
                    },
                    scale: if bs.s.len() >= 2 {
                        PoseTrack::new(&bs.s)
                    } else {
                        PoseTrack::default()
                    },
                });
            }
            let handle = app
                .world_mut()
                .resource_mut::<Assets<AnimationClip>>()
                .add(clip);
            let clip_idx = pose.clips.len() as u32;
            pose.clips.push(pclip);
            let node = if spec.mask == 0 {
                graph.add_clip(handle, 1.0, root)
            } else {
                graph.add_clip_with_mask(handle, spec.mask, 1.0, root)
            };
            pose.set_node(node, clip_idx, spec.mask);
            nodes.push(node);
        }
        let graph = app
            .world_mut()
            .resource_mut::<Assets<AnimationGraph>>()
            .add(graph);
        (graph, nodes, pose)
    }

    fn model_anims(graph: Handle<AnimationGraph>, pose: PoseSource) -> ModelAnimations {
        ModelAnimations {
            graph,
            clips: Vec::new(),
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            hand_close: [None, None],
            global_bones: Vec::new(),
            first_seq: None,
            pose: std::sync::Arc::new(pose),
        }
    }

    /// The Bevy oracle rig and the evaluator's joint-less rig: `(oracle, oracle_joints, ours)`.
    fn twin_rigs(
        app: &mut App,
        nbones: u16,
        graph: &Handle<AnimationGraph>,
        pose: &PoseSource,
    ) -> (Entity, Vec<Entity>, Entity) {
        let spawn_root = |app: &mut App| {
            app.world_mut()
                .spawn((
                    Transform::default(),
                    AnimationPlayer::default(),
                    AnimationTransitions::new(),
                    AnimationGraphHandle(graph.clone()),
                ))
                .id()
        };
        let oracle = spawn_root(app);
        let oracle_joints: Vec<Entity> = (0..nbones)
            .map(|i| {
                app.world_mut()
                    .spawn((
                        Transform::default(),
                        ChildOf(oracle),
                        bone_target_id(i),
                        AnimatedBy(oracle),
                    ))
                    .id()
            })
            .collect();
        let ours = spawn_root(app);
        let skeleton = ModelSkeleton {
            joints: (0..nbones)
                .map(|_| benilla_assets::ModelJoint {
                    parent: -1,
                    local_translation: Vec3::ZERO,
                    billboard: None,
                    parent_arm: None,
                })
                .collect(),
            spine_bone: None,
            head_bone: None,
        };
        app.world_mut().entity_mut(ours).insert((
            model_anims(graph.clone(), pose.clone()),
            RigPose::new(ours, &skeleton),
        ));
        (oracle, oracle_joints, ours)
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::animation::AnimationPlugin,
        ));
        plugin(&mut app);
        app
    }

    /// Every bone's local is bit-equal between the oracle's joints and the evaluator's `locals`.
    #[track_caller]
    fn assert_twins_equal(app: &mut App, oracle_joints: &[Entity], ours: Entity) {
        let rig = app.world().entity(ours).get::<RigPose>().unwrap();
        for (i, &a) in oracle_joints.iter().enumerate() {
            let ta = *app.world().entity(a).get::<Transform>().unwrap();
            assert_eq!(ta, rig.locals[i], "bone {i} diverged (oracle vs evaluator)");
        }
    }

    fn step(app: &mut App, secs: f32) {
        std::thread::sleep(Duration::from_secs_f32(secs));
        app.update();
    }

    fn drive(
        app: &mut App,
        rigs: &[Entity],
        f: impl Fn(&mut AnimationPlayer, &mut AnimationTransitions),
    ) {
        for &rig in rigs {
            let mut e = app.world_mut().entity_mut(rig);
            // Take transitions out to appease the borrow checker, run, put back.
            let mut tr = e.take::<AnimationTransitions>().unwrap();
            {
                let player = e.get_mut::<AnimationPlayer>().unwrap();
                f(player.into_inner(), &mut tr);
            }
            e.insert(tr);
        }
    }

    /// One looping clip, with a bone it never keys: both paths leave that bone at its spawn value.
    #[test]
    fn single_clip_matches_animate_targets() {
        let mut app = app();
        let spec = ClipSpec {
            bones: vec![
                (
                    0,
                    BoneSpec {
                        t: vec![(0.0, Vec3::ZERO), (0.4, Vec3::X), (0.8, Vec3::Y)],
                        r: vec![(0.0, Quat::IDENTITY), (0.8, Quat::from_rotation_y(2.0))],
                        s: vec![],
                    },
                ),
                (
                    1,
                    BoneSpec {
                        t: vec![],
                        r: vec![
                            (0.1, Quat::from_rotation_x(0.4)),
                            (0.5, Quat::from_rotation_z(-1.2)),
                        ],
                        s: vec![(0.0, Vec3::ONE), (0.8, Vec3::splat(2.0))],
                    },
                ),
            ],
            mask: 0,
        };
        let (graph, nodes, pose) = build(&mut app, &[spec], vec![0, 0, 0]);
        let (oracle, oj, ours) = twin_rigs(&mut app, 3, &graph, &pose);
        // One warm-up frame: Bevy's `ThreadedAnimationGraphs` builds on the graph asset's `Added`
        // event, so `animate_targets` skips the spawn frame.
        app.update();
        drive(&mut app, &[oracle, ours], |p, _| {
            p.play(nodes[0]).repeat();
        });
        for _ in 0..5 {
            step(&mut app, 0.05);
            assert_twins_equal(&mut app, &oj, ours);
        }
    }

    #[test]
    fn crossfade_matches_animate_targets() {
        let mut app = app();
        let walk = ClipSpec {
            bones: vec![(
                0,
                BoneSpec {
                    t: vec![(0.0, Vec3::ZERO), (0.6, Vec3::X * 3.0)],
                    r: vec![(0.0, Quat::IDENTITY), (0.6, Quat::from_rotation_z(1.0))],
                    s: vec![],
                },
            )],
            mask: 0,
        };
        let run = ClipSpec {
            bones: vec![(
                0,
                BoneSpec {
                    t: vec![(0.0, Vec3::Y), (0.3, Vec3::NEG_Y)],
                    r: vec![
                        (0.0, Quat::from_rotation_x(0.5)),
                        (0.3, Quat::from_rotation_x(-0.5)),
                    ],
                    s: vec![],
                },
            )],
            mask: 0,
        };
        let (graph, nodes, pose) = build(&mut app, &[walk, run], vec![0]);
        let (oracle, oj, ours) = twin_rigs(&mut app, 1, &graph, &pose);
        app.update(); // warm-up: see single_clip_matches_animate_targets
        drive(&mut app, &[oracle, ours], |p, tr| {
            tr.play(p, nodes[0], Duration::ZERO).repeat();
        });
        step(&mut app, 0.1);
        drive(&mut app, &[oracle, ours], |p, tr| {
            tr.play(p, nodes[1], Duration::from_secs_f32(0.25)).repeat();
        });
        for _ in 0..6 {
            step(&mut app, 0.06);
            assert_twins_equal(&mut app, &oj, ours);
        }
    }

    #[test]
    fn masked_overlay_matches_animate_targets() {
        let mut app = app();
        let base = ClipSpec {
            bones: vec![
                (
                    0, // "legs": in mask group 2, so the overlay must not touch it
                    BoneSpec {
                        t: vec![(0.0, Vec3::ZERO), (0.5, Vec3::X)],
                        r: vec![(0.0, Quat::IDENTITY), (0.5, Quat::from_rotation_y(1.0))],
                        s: vec![],
                    },
                ),
                (
                    1, // "torso": both drive it, the 8:1 blend
                    BoneSpec {
                        t: vec![(0.0, Vec3::ZERO), (0.5, Vec3::Z)],
                        r: vec![(0.0, Quat::IDENTITY), (0.5, Quat::from_rotation_z(0.7))],
                        s: vec![],
                    },
                ),
            ],
            mask: 0,
        };
        let overlay = ClipSpec {
            bones: vec![
                (
                    0,
                    BoneSpec {
                        t: vec![(0.0, Vec3::splat(9.0)), (0.4, Vec3::splat(9.0))],
                        r: vec![],
                        s: vec![],
                    },
                ),
                (
                    1,
                    BoneSpec {
                        t: vec![(0.0, Vec3::NEG_Z), (0.4, Vec3::NEG_X)],
                        r: vec![
                            (0.0, Quat::from_rotation_x(1.2)),
                            (0.4, Quat::from_rotation_x(0.2)),
                        ],
                        s: vec![],
                    },
                ),
            ],
            mask: 1 << 2,
        };
        let idle = ClipSpec {
            bones: vec![(
                1,
                BoneSpec {
                    t: vec![(0.0, Vec3::splat(50.0)), (0.4, Vec3::splat(50.0))],
                    r: vec![],
                    s: vec![],
                },
            )],
            mask: 0,
        };
        let (graph, nodes, pose) = build(&mut app, &[base, overlay, idle], vec![1 << 2, 0]);
        let (oracle, oj, ours) = twin_rigs(&mut app, 2, &graph, &pose);
        app.update(); // warm-up: see single_clip_matches_animate_targets
        drive(&mut app, &[oracle, ours], |p, _| {
            p.play(nodes[0]).repeat();
            p.play(nodes[1]).repeat().set_weight(8.0);
            p.play(nodes[2]).repeat().set_weight(0.0); // must be skipped entirely
        });
        for _ in 0..4 {
            step(&mut app, 0.07);
            assert_twins_equal(&mut app, &oj, ours);
        }
        // The masked bone is base-only: it never took the overlay's 9.0 constant.
        let t0 = app.world().entity(ours).get::<RigPose>().unwrap().locals[0];
        assert_ne!(
            t0.translation,
            Vec3::splat(9.0),
            "mask kept the overlay off bone 0"
        );
    }

    /// Parked bones freeze while the clock advances; unparking resumes at the absolute clock.
    #[test]
    fn parked_rig_freezes_bones_only() {
        let mut app = app();
        let spec = ClipSpec {
            bones: vec![(
                0,
                BoneSpec {
                    t: vec![(0.0, Vec3::ZERO), (0.4, Vec3::X)],
                    r: vec![],
                    s: vec![],
                },
            )],
            mask: 0,
        };
        let (graph, nodes, pose) = build(&mut app, &[spec], vec![0]);
        let (oracle, oj, ours) = twin_rigs(&mut app, 1, &graph, &pose);
        // The oracle runs unparked throughout: the absolute-clock witness.
        drive(&mut app, &[oracle, ours], |p, _| {
            p.play(nodes[0]).repeat();
        });
        step(&mut app, 0.05);
        app.world_mut().entity_mut(ours).insert(AnimParked);
        let bone0 =
            |app: &App| app.world().entity(ours).get::<RigPose>().unwrap().locals[0].translation;
        let frozen = bone0(&app);
        step(&mut app, 0.1);
        assert_eq!(bone0(&app), frozen, "parked bones hold");
        app.world_mut().entity_mut(ours).remove::<AnimParked>();
        step(&mut app, 0.05);
        assert_twins_equal(&mut app, &oj, ours); // woke to the absolute-clock pose
    }

    /// The doodad lane's three player mutations: the first-sequence arm, the variation re-roll's
    /// snap (`stop_all`, replay) and the draw gate's hide and resume (`stop_all`, re-arm, seek).
    #[test]
    fn doodad_drive_pattern_matches_animate_targets() {
        let mut app = app();
        // Clip A keys bones 0 and 1, clip B only bone 1: the re-roll leaves bone 0 on A's last
        // value on both paths.
        let spec_a = ClipSpec {
            bones: vec![
                (
                    0,
                    BoneSpec {
                        t: vec![(0.0, Vec3::ZERO), (0.4, Vec3::X), (0.8, Vec3::Y)],
                        r: vec![(0.0, Quat::IDENTITY), (0.8, Quat::from_rotation_y(1.1))],
                        s: vec![],
                    },
                ),
                (
                    1,
                    BoneSpec {
                        t: vec![(0.1, Vec3::Z), (0.7, Vec3::splat(0.3))],
                        r: vec![],
                        s: vec![(0.0, Vec3::ONE), (0.8, Vec3::splat(1.5))],
                    },
                ),
            ],
            mask: 0,
        };
        let spec_b = ClipSpec {
            bones: vec![(
                1,
                BoneSpec {
                    t: vec![(0.0, Vec3::NEG_Y), (0.5, Vec3::NEG_X)],
                    r: vec![(0.0, Quat::from_rotation_z(0.3)), (0.5, Quat::IDENTITY)],
                    s: vec![],
                },
            )],
            mask: 0,
        };
        let (graph, nodes, pose) = build(&mut app, &[spec_a, spec_b], vec![0, 0]);
        let (oracle, oj, ours) = twin_rigs(&mut app, 2, &graph, &pose);
        let rigs = [oracle, ours];
        app.update(); // ThreadedAnimationGraphs warm-up (see single_clip's note)

        // 1 · the FirstSeq arm, mid-loop and across the loop wrap.
        drive(&mut app, &rigs, |p, _| {
            p.play(nodes[0]).repeat();
        });
        step(&mut app, 0.3);
        assert_twins_equal(&mut app, &oj, ours);
        step(&mut app, 0.6); // past 0.8: the repeat wrap
        assert_twins_equal(&mut app, &oj, ours);

        // 2 · the re-roll snap: stop everything, hard-play the new variation.
        drive(&mut app, &rigs, |p, _| {
            p.stop_all();
            p.play(nodes[1]).repeat();
        });
        step(&mut app, 0.2);
        assert_twins_equal(&mut app, &oj, ours); // bone 0 untouched-by-B on both paths

        // 3 · the draw gate's hide: stop, not pause, since a paused animation is still sampled.
        drive(&mut app, &rigs, |p, _| {
            p.stop_all();
        });
        step(&mut app, 0.15);
        assert_twins_equal(&mut app, &oj, ours);

        // 4 · the resume: re-arm and seek to the shared-clock cursor, the gate's exact calls.
        drive(&mut app, &rigs, |p, _| {
            p.stop_all();
            p.play(nodes[0]).repeat().seek_to(0.37);
        });
        step(&mut app, 0.1);
        assert_twins_equal(&mut app, &oj, ours);
    }
}
