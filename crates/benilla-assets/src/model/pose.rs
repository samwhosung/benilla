//! The direct M2 pose evaluator's baked data: the `AnimationClip`s' keyframes from the same bake
//! walk, flattened per clip, graph node and bone. The evaluator (`benilla_world::rig_anim::pose`)
//! reproduces Bevy's `animate_targets` output exactly, so these must match the clips key for key.

use bevy::animation::animatable::Animatable;
use bevy::animation::graph::AnimationNodeIndex;
use bevy::prelude::*;

/// One channel's keyframes, normalized as `UnevenCore::new` does (stable sort by time, the first
/// sample at a time kept), so [`Self::sample`] matches the clip's curve. Empty: the clip does not
/// key the channel, and the joint keeps its value.
#[derive(Clone)]
pub struct PoseTrack<T> {
    times: Vec<f32>,
    values: Vec<T>,
}

// Manual: the derive would add a needless `T: Default` bound.
impl<T> Default for PoseTrack<T> {
    fn default() -> Self {
        Self {
            times: Vec::new(),
            values: Vec::new(),
        }
    }
}

impl<T: Animatable + Clone> PoseTrack<T> {
    /// Mirrors `keyframe_curve`: one key is a constant, and keys that dedup to fewer than two
    /// times are absent, since Bevy's constructor refuses them.
    pub fn new(keys: &[(f32, T)]) -> Self {
        match keys.len() {
            0 => Self::default(),
            1 => Self {
                times: vec![keys[0].0],
                values: vec![keys[0].1.clone()],
            },
            _ => {
                let mut sorted: Vec<(f32, T)> = keys.to_vec();
                sorted.sort_by(|(t0, _), (t1, _)| t0.total_cmp(t1));
                sorted.dedup_by_key(|(t, _)| *t);
                if sorted.len() < 2 {
                    return Self::default();
                }
                let (times, values) = sorted.into_iter().unzip();
                Self { times, values }
            }
        }
    }

    /// Sample at `t`, clamped into the key span, as Bevy's `uneven_interp` does.
    #[inline]
    pub fn sample(&self, t: f32) -> Option<T> {
        let times = &self.times;
        match times.len() {
            0 => None,
            1 => Some(self.values[0].clone()),
            _ => Some(
                match times.binary_search_by(|pt| pt.partial_cmp(&t).unwrap()) {
                    Ok(i) => self.values[i].clone(),
                    Err(0) => self.values[0].clone(),
                    Err(i) if i >= times.len() => self.values[times.len() - 1].clone(),
                    Err(i) => {
                        let s = (t - times[i - 1]) / (times[i] - times[i - 1]);
                        T::interpolate(&self.values[i - 1], &self.values[i], s)
                    }
                },
            ),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }
}

/// One bone's channels in one clip; a bone with no channel is left out.
#[derive(Clone)]
pub struct PoseBone {
    pub bone: u16,
    pub translation: PoseTrack<Vec3>,
    pub rotation: PoseTrack<Quat>,
    pub scale: PoseTrack<Vec3>,
}

/// One clip's keyed bones, ascending by bone index, which the evaluator's merge walk needs.
#[derive(Clone, Default)]
pub struct PoseClip {
    pub bones: Vec<PoseBone>,
}

impl PoseClip {
    /// Insert a bone in bone order, skipping one with no channel.
    pub fn push(&mut self, bone: PoseBone) {
        if bone.translation.is_empty() && bone.rotation.is_empty() && bone.scale.is_empty() {
            return;
        }
        let at = self.bones.partition_point(|b| b.bone <= bone.bone);
        self.bones.insert(at, bone);
    }
}

/// One graph node: the [`PoseClip`] it plays and its mask bits, which skip a bone whose mask
/// groups they intersect. Every node's weight is 1.0, so none is stored.
#[derive(Clone, Copy)]
pub struct PoseNode {
    pub clip: u32,
    pub mask: u64,
}

/// A model's baked pose source, filled beside the `AnimationGraph` it mirrors.
#[derive(Clone, Default)]
pub struct PoseSource {
    /// The pose twin of each built clip (sequence clips in build order, then the grip clip).
    pub clips: Vec<PoseClip>,
    /// Dense by `AnimationNodeIndex::index()`; `None` for the root/unused slots.
    pub nodes: Vec<Option<PoseNode>>,
    /// Per bone: the mask-group bits it belongs to (the `add_target_to_mask_group` calls).
    pub bone_masks: Vec<u64>,
}

impl PoseSource {
    /// Record `node` as playing `clip` under `mask`; call it beside each `graph.add_clip*`.
    pub fn set_node(&mut self, node: AnimationNodeIndex, clip: u32, mask: u64) {
        let i = node.index();
        if self.nodes.len() <= i {
            self.nodes.resize(i + 1, None);
        }
        self.nodes[i] = Some(PoseNode { clip, mask });
    }

    /// The [`PoseNode`] for a graph node, if the evaluator knows it.
    #[inline]
    pub fn node(&self, node: AnimationNodeIndex) -> Option<PoseNode> {
        self.nodes.get(node.index()).copied().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::animation::animation_curves::AnimatableKeyframeCurve;
    use bevy::math::curve::Curve;

    #[test]
    fn track_matches_the_bevy_keyframe_curve() {
        let vkeys = [
            (0.1, Vec3::new(1.0, 2.0, 3.0)),
            (0.35, Vec3::new(-2.0, 0.5, 4.0)),
            (0.4, Vec3::new(0.0, 1.0, 0.0)),
            (1.2, Vec3::new(5.0, -1.0, 2.0)),
        ];
        let track = PoseTrack::new(&vkeys);
        let curve = AnimatableKeyframeCurve::new(vkeys).unwrap();
        for i in 0..=1400 {
            let t = -0.1 + i as f32 * 0.001;
            assert_eq!(
                track.sample(t).unwrap(),
                curve.sample_clamped(t),
                "Vec3 diverges at t={t}"
            );
        }

        let qkeys = [
            (0.0, Quat::from_rotation_y(0.3)),
            (0.25, Quat::from_rotation_x(1.4)),
            // A >90° step, exercising slerp's shortest-path sign handling.
            (0.6, Quat::from_axis_angle(Vec3::new(0.6, 0.8, 0.0), 2.9)),
        ];
        let track = PoseTrack::new(&qkeys);
        let curve = AnimatableKeyframeCurve::new(qkeys).unwrap();
        for i in 0..=800 {
            let t = -0.05 + i as f32 * 0.001;
            assert_eq!(
                track.sample(t).unwrap(),
                curve.sample_clamped(t),
                "Quat diverges at t={t}"
            );
        }
    }

    #[test]
    fn presence_and_dedup_mirror_the_curve_constructor() {
        assert!(PoseTrack::<Vec3>::new(&[]).sample(0.5).is_none());

        let single = PoseTrack::new(&[(0.7, Vec3::X)]);
        for t in [-1.0, 0.0, 0.7, 3.0] {
            assert_eq!(single.sample(t), Some(Vec3::X));
        }

        let dup = PoseTrack::new(&[
            (0.0, Vec3::X),
            (0.5, Vec3::Y),
            (0.5, Vec3::Z),
            (1.0, Vec3::X),
        ]);
        let curve = AnimatableKeyframeCurve::new([
            (0.0, Vec3::X),
            (0.5, Vec3::Y),
            (0.5, Vec3::Z),
            (1.0, Vec3::X),
        ])
        .unwrap();
        for i in 0..=1000 {
            let t = i as f32 * 0.001;
            assert_eq!(dup.sample(t).unwrap(), curve.sample_clamped(t), "t={t}");
        }

        // Two keys at one time: Bevy's constructor fails, so the channel is absent.
        assert!(PoseTrack::new(&[(0.5, Vec3::X), (0.5, Vec3::Y)])
            .sample(0.5)
            .is_none());
    }

    #[test]
    fn clip_bones_stay_sorted() {
        let bone = |i: u16| PoseBone {
            bone: i,
            translation: PoseTrack::new(&[(0.0, Vec3::X)]),
            rotation: PoseTrack::default(),
            scale: PoseTrack::default(),
        };
        let mut clip = PoseClip::default();
        for i in [3u16, 1, 2, 7, 0] {
            clip.push(bone(i));
        }
        clip.push(PoseBone {
            bone: 5,
            translation: PoseTrack::default(),
            rotation: PoseTrack::default(),
            scale: PoseTrack::default(),
        });
        let order: Vec<u16> = clip.bones.iter().map(|b| b.bone).collect();
        assert_eq!(
            order,
            vec![0, 1, 2, 3, 7],
            "sorted; the empty bone 5 dropped"
        );
    }
}
