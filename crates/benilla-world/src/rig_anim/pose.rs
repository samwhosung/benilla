//! The direct M2 pose evaluator: turn each rig's `AnimationPlayer` state into
//! bone `Transform`s ourselves, off the baked [`PoseSource`], instead of routing per-bone entities
//! through Bevy's `animate_targets` (0711's second lane — ~1.9 µs *per bone* of graph-walk +
//! hash-lookup + boxed-curve machinery to produce, for most bones, a constant).
//!
//! The player stays the playback state machine — the driver's arms, cross-fades, weights, seeks,
//! and completions are untouched — and this system reproduces `animate_targets`' *output* exactly
//! (the goldens below run both side by side and assert bone-for-bone equality). The law, read from
//! `bevy_animation-0.18.1` and pinned in 0712: per (bone, property), active nodes contribute in
//! **ascending node-index order**, folded by `total += w; acc = interpolate(acc, v, w/total)`
//! (`Vec3::lerp` / `Quat::slerp` — the same `Animatable` calls, so the math is bit-identical); a
//! node contributes iff its weight ≠ 0 and `bone_masks[bone] & node_mask == 0` and its clip keys
//! the channel; sampling clamps into the key span; a property no active clip keys is left alone
//! (there is no rest-pose reset — the joint keeps what it has). Paused animations still evaluate
//! at their frozen seek, exactly as Bevy does.
//!
//! Parking is the [`AnimParked`] marker: a parked rig is skipped here — clocks,
//! driver, and events all keep running — which preserves 0448's two observables (absolute-clock
//! snap on wake, off-screen combat audibility) with no `AnimatedBy` repoint machinery.

use benilla_assets::{ModelAnimations, ModelSkeleton, PoseSource};
use bevy::animation::animatable::Animatable;
use bevy::app::AnimationSystems;
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use super::AnimParked;
use crate::vis_chain::VisChainOnly;

/// The collapsed rig's pose buffer, on the entity carrying the `AnimationPlayer`:
/// one local `Transform` per bone in skeleton order — no joint entities at all (decision 0712's
/// `PosedRig` handle grew into this when the ~59 k bone entities collapsed). Every pose writer —
/// the evaluator below, the body twist, the global-sequence channels — writes `locals` and raises
/// [`Self::pose_dirty`]; `compose` then folds the parent-sorted chain into per-bone model-space
/// affines, and the world finalize (`creature_anim::compose`) turns those into palette rows and
/// consumer-anchor frames.
#[derive(Component)]
pub struct RigPose {
    /// The rig's model-space frame: the holder itself, its conform node, or a mounted rider's
    /// seat anchor. Its `GlobalTransform` is `world_from_model`; the palette recomputes when it
    /// moves.
    pub joints_root: Entity,
    /// Per-bone local TRS, skeleton order — what the per-bone joint `Transform`s used to hold.
    pub locals: Vec<Transform>,
    /// Per-bone **animated** model-space affine (`parent_model × local`), the product transform
    /// propagation used to compute through the joint entities. Rebuilt by [`Self::compose`] for a
    /// dirty rig, WITH the `flags & 0x7` arm folded in — it is model-space-computable and the
    /// anchors are seated from these. Only the camera-dependent billboard replacement waits for
    /// the world pass.
    pub model: Vec<Affine3A>,
    pub parents: Vec<i16>,
    /// The bone's billboard arm (`0x08/0x10/0x20/0x40`), for the world pass's camera replacement.
    pub(crate) kinds: Vec<Option<benilla_formats::BillboardKind>>,
    /// Bone flags `0x1/0x2/0x4`: how the bone's effective PARENT matrix is rebuilt from the model
    /// root before it composes (`crate::billboard::parent_arm_matrix`).
    pub(crate) arms: Vec<Option<benilla_formats::ParentArm>>,
    /// Each bone's BIND local translation — the pivot the arm preserves. `locals` carry the
    /// ANIMATED translation, which the byte law rotates by the *new* basis; not interchangeable.
    pub(crate) binds: Vec<Vec3>,
    /// Any bone billboards — the world pass re-faces this rig whenever it isn't parked.
    pub(crate) has_billboard: bool,
    /// Any bone billboards or resets — the world pass's override walk runs at all.
    pub(crate) has_special: bool,
    /// `(bone, anchor entity)` — the consumer anchors (attachments, markers, emitter/ribbon/
    /// light/billboard-card bones), children of [`Self::joints_root`]. The compose pass re-seats
    /// their local `Transform`s so ordinary propagation carries every consumer subtree — held
    /// items, nested effect rigs, the mount seat — exactly as the joint hierarchy did.
    pub anchors: Vec<(u16, Entity)>,
    /// A pose writer touched `locals` since the last world pass. Starts `true` so a rig that
    /// never animates (bind pose) still gets its one palette write.
    pub pose_dirty: bool,
}

impl RigPose {
    /// Build the rig's buffer at bind pose (composed), `pose_dirty` armed for the first palette
    /// write. Anchors are registered by the spawner as it creates them.
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

    /// Forward-fold `locals` into `model` — the exact affine product transform propagation
    /// computed through the joint entities (`parent_global.mul_transform(local)`, model-rooted).
    /// M2 bones are parent-sorted (the format guarantees parent < child); a malformed child whose
    /// parent follows it composes from the model root, matching the old hierarchy's `unwrap_or(root)`.
    ///
    /// The **`flags & 0x7` arm runs here**, in model space, with the model root as the identity —
    /// which is what it *is*, relative to this rig. That is not a shortcut around the world-space
    /// form the world pass uses: a model frame is a similarity (uniform scale), so `root_world ×
    /// arm(P_model, I)` and `arm(root_world × P_model, root_world)` agree axis for axis, including
    /// the ratio leg's per-axis magnitudes (the common factor cancels).
    ///
    /// Doing it here rather than only in the world pass is load-bearing, not a tidy-up. `model` is
    /// what the pre-propagation pass re-seats consumer anchors from, so an arm applied only
    /// afterwards leaves ordinary propagation to carry every attached subtree on the UN-armed
    /// frame — and the world pass's patch walk deliberately does not enter a nested rig. That is
    /// how a mounted rider's pauldron ended up riding the horse's gallop while the shoulder it
    /// hangs off did not.
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

    /// This rig is rendered by an **off-world camera** (a portrait booth), so the world pass must
    /// not camera-face its billboard bones: [`finalize_rig_worlds`](super::finalize_rig_worlds)
    /// takes its basis from the `WorldCamera`, which is not the camera drawing this rig. Dropping
    /// the kinds leaves those bones at their composed pose — exactly what the entity-joint booth
    /// lane produced (its joints never carried a `BillboardJointRig`), while the booth's own card
    /// facer counter-rotates each card's anchor to the booth camera. The `flags & 0x7` parent
    /// arms stay: that law is camera-free and runs in [`Self::compose`] regardless.
    pub fn without_camera_billboards(mut self) -> Self {
        self.kinds.iter_mut().for_each(|k| *k = None);
        self.has_billboard = false;
        self.has_special = self.arms.iter().any(Option::is_some);
        self
    }

    /// The anchor entity standing in for `bone`, spawned on first demand: a
    /// consumer that needs an *entity* on a bone — a held item's parent, an emitter's owner
    /// frame, a quest marker's seat — resolves it here, and only bones something actually
    /// consumes ever get one. At the LBRS pin, 96 % of the eagerly-spawned population hosted
    /// nothing.
    ///
    /// The spawn seats from `model` — the same matrices the compose pass re-seats every anchor
    /// from — so an anchor created mid-frame (a weapon equipped in combat) stands at the current
    /// composed pose, never the rest pose. `rig` is the [`RigPose`] holder (`self` cannot know
    /// its own entity); the anchor parents under [`Self::joints_root`] and dies with it.
    ///
    /// `None` = the bone is outside this skeleton (an out-of-range authored reference — the
    /// consumer misses, as it always has).
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
        // Visibility so an attached subtree (a held item under a hand anchor) inherits the
        // owner's hide — chain only: an anchor renders nothing, and at the Goldshire pin the
        // anchor population was the single largest never-rendering block in the per-camera
        // visibility sweep (4.8k rows).
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

    /// The world-space point `offset` under `bone` at the current composed pose — what a
    /// consumer that only ever *reads* a position (the overhead anchor, a missile launch point,
    /// the bowstring's nock) gets instead of an anchor entity, so those reads spawn nothing.
    /// `root_global` is [`Self::joints_root`]'s `GlobalTransform` — exactly the
    /// frame an anchor's own global would compose through.
    ///
    /// One deliberate difference from reading a spawned anchor: `model` carries the composed
    /// pose with the `flags & 0x7` arm folded in but **not** the world pass's camera-billboard
    /// replacement. No attachment point or event marker sits on a camera-faced bone (those bones
    /// carry cards, which stay entity-anchored via [`Self::anchor_for`]), so nothing observable
    /// rides on the difference.
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

/// On every consumer anchor: which rig and bone it stands in for. The body twist's model-frame
/// walk splices through it (a mounted rider's chain crosses the mount's seat bone), and the world
/// pass uses it to cascade a re-seated root into dependent rigs.
#[derive(Component)]
pub struct RigAnchor {
    /// The rig holder (the entity carrying [`RigPose`]).
    pub rig: Entity,
    pub bone: u16,
}

/// On an entity serving as some rig's `joints_root` while living inside ANOTHER rig's anchor
/// subtree (the mounted rider's seat anchor under the mount's attachment-0 anchor): the world
/// pass, having re-seated that subtree, re-finalizes the dependent rig too — its
/// `world_from_model` moved after propagation ran.
#[derive(Component)]
pub struct RigFrame(pub Entity);

/// One active animation's evaluation inputs, resolved against the rig's [`PoseSource`].
struct Active {
    /// `AnimationNodeIndex::index()` — the sort key that reproduces Bevy's blend order.
    node: usize,
    clip: usize,
    mask: u64,
    weight: f32,
    seek: f32,
    /// The merge walk's position in the clip's bone list.
    cursor: usize,
}

/// Evaluate every live rig's pose. Runs where `animate_targets` runs — inside [`AnimationSystems`]
/// after `advance_animations` ticked the seek clocks — so the pose post-passes (body twist,
/// global-sequence writes) and the model compose see it at the same point in the frame.
fn evaluate_rig_poses(
    mut rigs: Query<(&AnimationPlayer, &ModelAnimations, &mut RigPose), Without<AnimParked>>,
) {
    // Parallel over rigs: a rig reads its shared clips and writes only its own
    // pose, and a raid stands ~400 live rigs / ~12k bones — 0.4 ms of one thread's frame when
    // walked serially, and this system sits on the main thread's critical path.
    rigs.par_iter_mut().for_each(|(player, anims, mut rig)| {
        let src = &anims.pose;
        // This rig's contributing animations, ascending by node index (Bevy's effective blend
        // order — sorted children folded through a LIFO stack, see the module doc). One scratch
        // per worker thread, reused across rigs: a `Vec` per rig per frame was an allocation
        // and a free for every live rig on every frame (decision 1979's floor).
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
                    // The steady state: one looping gait. A single contribution commits at full
                    // value whatever its weight (Bevy's register initializes unnormalized).
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

/// The multi-animation path (a cross-fade, a masked overlay, the grip): merge-walk the
/// bone-sorted clips and fold each (bone, property)'s contributions in node order.
fn blend_rig(src: &PoseSource, active: &mut [Active], rig: &mut RigPose) {
    // Walk the next keyed bone across all clips until every cursor is spent.
    while let Some(bone) = active
        .iter()
        .filter_map(|a| src.clips[a.clip].bones.get(a.cursor).map(|b| b.bone))
        .min()
    {
        let bone_mask = src.bone_masks.get(bone as usize).copied().unwrap_or(0);
        // Fold each property over the clips keying this bone, in node order (`active` is sorted).
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
                continue; // masked out for this node — no contribution, cursor still advances
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

/// One property's running fold — Bevy's blend register verbatim: the first contribution lands at
/// full value (carrying its weight), each later one at `w / total`.
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

/// Register the evaluator in `animate_targets`' window: after the seek clocks tick, before the
/// pose post-passes (which order `.after(AnimationSystems)`) and transform propagation.
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

    /// One test clip's authored channels: per bone, optional key lists for T/R/S.
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

    /// Build the graph + clips + pose source from specs, exactly as `m2.rs` does: the Bevy clip
    /// through `AnimatableCurve`/`AnimatableKeyframeCurve`, the pose twin through
    /// `PoseTrack`/`PoseClip`, node table beside each graph add.
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

    /// A `ModelAnimations` carrying only what the evaluator reads (graph + pose).
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

    /// Spawn the two rigs: the Bevy-path oracle (per-joint targets, `animate_targets` evaluates
    /// it) and the evaluator rig (a joint-less [`RigPose`] buffer). Returns `(oracle_root,
    /// oracle_joints, ours_root)`.
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

    /// Run `f` identically on both players, then step and assert every bone's local is
    /// **exactly** equal between the oracle's joint `Transform`s and the evaluator's `locals`
    /// array — same inputs, same `Animatable` calls, bit-identical outputs.
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

    /// Run the same player mutation on each rig — identical inputs is the whole point of the
    /// twin-rig golden.
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

    /// The steady state (one looping clip): channels present and absent, exact equality mid-loop —
    /// including a bone the clip never keys (both paths must leave it at its spawn value).
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
                // bone 2 never keyed — the untouched-property law.
            ],
            mask: 0,
        };
        let (graph, nodes, pose) = build(&mut app, &[spec], vec![0, 0, 0]);
        let (oracle, oj, ours) = twin_rigs(&mut app, 3, &graph, &pose);
        // One warm-up frame: Bevy's ThreadedAnimationGraphs builds off the graph asset's Added
        // event, so `animate_targets` bails on the spawn frame (our evaluator doesn't need it and
        // evaluates immediately — strictly earlier, not different). Compare from frame 2 on.
        app.update();
        drive(&mut app, &[oracle, ours], |p, _| {
            p.play(nodes[0]).repeat();
        });
        for _ in 0..5 {
            step(&mut app, 0.05);
            assert_twins_equal(&mut app, &oj, ours);
        }
    }

    /// A cross-fade (`AnimationTransitions::play` mid-flight): main at weight 1 plus the fading
    /// node's decaying weight — the normalized fold must match Bevy's frame for frame, and the
    /// past-the-end clamp holds once the one-shot finishes.
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
        // Cross-fade into the run over 0.25 s; sample mid-fade several times.
        drive(&mut app, &[oracle, ours], |p, tr| {
            tr.play(p, nodes[1], Duration::from_secs_f32(0.25)).repeat();
        });
        for _ in 0..6 {
            step(&mut app, 0.06);
            assert_twins_equal(&mut app, &oj, ours);
        }
    }

    /// The masked upper-body overlay at weight 8 over a full-body base (the driver's one-shot
    /// route), plus a weight-0 node that must contribute nothing: bones in the mask group blend
    /// 8:1, bones outside it take the base alone.
    #[test]
    fn masked_overlay_matches_animate_targets() {
        let mut app = app();
        let base = ClipSpec {
            bones: vec![
                (
                    0, // "legs" — in mask group 2, the overlay must not touch it
                    BoneSpec {
                        t: vec![(0.0, Vec3::ZERO), (0.5, Vec3::X)],
                        r: vec![(0.0, Quat::IDENTITY), (0.5, Quat::from_rotation_y(1.0))],
                        s: vec![],
                    },
                ),
                (
                    1, // "torso" — both drive it, the 8:1 blend
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
        // bone 0 is in mask group 2 (the "outside the upper subtree" group).
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
        // And the masked bone really is base-only: it moved off spawn (the base wrote it), yet
        // never took the overlay's parked 9.0 constant.
        let t0 = app.world().entity(ours).get::<RigPose>().unwrap().locals[0];
        assert_ne!(
            t0.translation,
            Vec3::splat(9.0),
            "mask kept the overlay off bone 0"
        );
    }

    /// A parked rig is skipped (the 0448 gate's new mechanism): bones freeze while the clock
    /// advances; unparking resumes sampling at the absolute clock.
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
        // The oracle twin runs unparked throughout — the absolute-clock witness.
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

    /// The DOODAD lane's drive pattern (decision 1360's golden, built AHEAD of the collapse):
    /// the three player mutations `doodad_anim` performs — the FirstSeq arm
    /// (`play(node).repeat()`), the variation re-roll's snap (`stop_all` + replay,
    /// `reroll_doodad_variation`), and the draw gate's hide/resume (`stop_all`, then re-arm +
    /// `seek_to` the absolute clock, `gate_doodad_anim`) — each run identically on the oracle
    /// and the evaluator rig and asserted bit-equal, wraps and the untouched-property law
    /// included. Green here means the joint-entity collapse only has to move consumers; the
    /// drive semantics are already proven shared.
    #[test]
    fn doodad_drive_pattern_matches_animate_targets() {
        let mut app = app();
        // Clip A keys bones 0+1; clip B keys only bone 1 — so the re-roll snap leaves bone 0 on
        // clip A's last-applied value on BOTH paths (the no-rest-reset law across a re-arm).
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
        step(&mut app, 0.6); // past 0.8 — the repeat wrap
        assert_twins_equal(&mut app, &oj, ours);

        // 2 · the re-roll snap: stop everything, hard-play the new variation.
        drive(&mut app, &rigs, |p, _| {
            p.stop_all();
            p.play(nodes[1]).repeat();
        });
        step(&mut app, 0.2);
        assert_twins_equal(&mut app, &oj, ours); // bone 0 untouched-by-B on both paths

        // 3 · the draw gate's hide: stop (not pause — a paused animation is still sampled;
        // stopped, both paths leave every target exactly where it stands).
        drive(&mut app, &rigs, |p, _| {
            p.stop_all();
        });
        step(&mut app, 0.15);
        assert_twins_equal(&mut app, &oj, ours);

        // 4 · the resume: re-arm + seek to the shared-clock cursor, the gate's exact calls.
        drive(&mut app, &rigs, |p, _| {
            p.stop_all();
            p.play(nodes[0]).repeat().seek_to(0.37);
        });
        step(&mut app, 0.1);
        assert_twins_equal(&mut app, &oj, ours);
    }
}
