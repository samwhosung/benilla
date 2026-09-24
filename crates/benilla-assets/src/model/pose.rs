//! The pose source — the direct M2 pose evaluator's baked data.
//!
//! Bevy's `animate_targets` prices every bone at graph-walk + hash-lookup + boxed-curve rates
//! (~1.9 µs/bone — 0711's second lane). These types hold the *same* Bevy-space keyframes as the
//! `AnimationClip`s — emitted by the same bake walk, so they cannot drift — in the flat shape a
//! direct sampler wants: per clip, bone-sorted channel tracks; per graph node, the (clip, mask)
//! pair; per bone, its mask-group bits. The runtime evaluator (`benilla::creature_anim::pose`)
//! folds these with Bevy's own `Animatable::interpolate`, reproducing `animate_targets`' output
//! exactly (goldens there assert it bone-for-bone).

use bevy::animation::animatable::Animatable;
use bevy::animation::graph::AnimationNodeIndex;
use bevy::prelude::*;

/// One channel's keyframes: times ascending and strictly deduplicated — the exact normalization
/// `UnevenCore::new` applies to the twin `AnimatableKeyframeCurve` (stable sort by time, dedup
/// keeping the FIRST sample at each time), so [`Self::sample`] and `sample_clamped` agree
/// key-for-key. Empty ⇒ the channel is absent (the clip never keys it — Bevy leaves the joint
/// component untouched, and so does the evaluator).
#[derive(Clone)]
pub struct PoseTrack<T> {
    times: Vec<f32>,
    values: Vec<T>,
}

// Manual: the derive would bound `T: Default`, which `Quat` channels don't need — an empty track
// has no values of `T` at all.
impl<T> Default for PoseTrack<T> {
    fn default() -> Self {
        Self {
            times: Vec::new(),
            values: Vec::new(),
        }
    }
}

impl<T: Animatable + Clone> PoseTrack<T> {
    /// Bake a channel from the clip walk's key list, mirroring `keyframe_curve`'s presence rules
    /// exactly: no keys ⇒ absent; one key ⇒ a constant (Bevy builds a flat 2-key curve — same
    /// sampled value everywhere); ≥2 keys ⇒ `UnevenCore`'s sort + dedup, and absent if fewer than
    /// 2 distinct times remain (Bevy's constructor errors there and the channel is dropped).
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

    /// Sample at `t`, clamped into the key span — `uneven_interp`'s law verbatim (binary search;
    /// exact hit returns that key; before/after the span clamps to the end keys; between keys
    /// interpolates with `Animatable::interpolate` — `Vec3::lerp` / `Quat::slerp`, the same calls
    /// the Bevy curve makes). `None` iff the channel is absent.
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

/// One bone's channels in one clip — only bones with at least one present channel appear.
#[derive(Clone)]
pub struct PoseBone {
    pub bone: u16,
    pub translation: PoseTrack<Vec3>,
    pub rotation: PoseTrack<Quat>,
    pub scale: PoseTrack<Vec3>,
}

/// One clip's keyed bones, sorted ascending by bone index (the evaluator's merge walk relies on
/// it). The same channels, values, and presence as the twin `AnimationClip`.
#[derive(Clone, Default)]
pub struct PoseClip {
    pub bones: Vec<PoseBone>,
}

impl PoseClip {
    /// Push a bone's channels, keeping [`Self::bones`] sorted (the bake walks file bone order,
    /// which is ascending for every retail M2 — the insertion sort is a no-op there and a
    /// correctness backstop elsewhere). Bones with no present channel are skipped.
    pub fn push(&mut self, bone: PoseBone) {
        if bone.translation.is_empty() && bone.rotation.is_empty() && bone.scale.is_empty() {
            return;
        }
        let at = self.bones.partition_point(|b| b.bone <= bone.bone);
        self.bones.insert(at, bone);
    }
}

/// One graph node the evaluator understands: which [`PoseClip`] it plays and its mask bits (the
/// `add_clip_with_mask` argument; a node whose mask bit intersects a bone's mask groups skips
/// that bone — Bevy's `computed_masks` law with a flat graph, where ancestors contribute 0).
/// Node weights are all 1.0 by construction (`add_clip(_, 1.0, root)`), so none is stored.
#[derive(Clone, Copy)]
pub struct PoseNode {
    pub clip: u32,
    pub mask: u64,
}

/// A model's baked pose source: everything the runtime evaluator needs, filled by
/// the same code that builds the `AnimationGraph` so the two cannot drift.
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
    /// Record `node` as playing `clip` under `mask` — called right after the corresponding
    /// `graph.add_clip*` so the table mirrors the graph by construction.
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

    /// The channel golden: [`PoseTrack::sample`] must equal
    /// `AnimatableKeyframeCurve::sample_clamped` — the exact curve the twin `AnimationClip`
    /// carries — over a dense time sweep spanning before-first, between-keys, exact-hit, and
    /// past-last, for both Vec3 (lerp) and Quat (slerp) channels.
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

    /// The presence rules mirror `keyframe_curve`: a single key is a constant everywhere (Bevy's
    /// flat 2-key curve), duplicate-time keys dedup keeping the FIRST (`UnevenCore::new`), and a
    /// list that dedups below 2 keys is absent (Bevy's constructor errors → the channel drops).
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

        // Two keys at one time: Bevy's constructor would fail — the channel must be absent.
        assert!(PoseTrack::new(&[(0.5, Vec3::X), (0.5, Vec3::Y)])
            .sample(0.5)
            .is_none());
    }

    /// `PoseClip::push` keeps bones ascending and drops all-absent bones.
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
