//! A model's playable animations, and how a requested animation id resolves to one of them.

use benilla_formats::AnimDataCatalog;
pub use benilla_formats::PlayableAnim;
use bevy::animation::graph::{AnimationGraph, AnimationNodeIndex};
use bevy::prelude::*;

/// One event keyframe on a clip, with where as well as when: the reference's event kernel
/// (`0x719370`) fires at `placement · boneMatrix[bone] · position`, and many records sit far from
/// the model origin. [`Self::offset`] serves a posed joint, [`Self::point`] a bone chain that
/// never moves; both are exact.
#[derive(Debug, Clone, Copy)]
pub struct ClipEvent {
    /// Seconds from the clip start.
    pub time: f32,
    /// The forward-stored 4CC (`*b"$SND"`).
    pub ident: [u8; 4],
    /// The payload: a SoundEntries id for `$SND`/`$DSL`/`$DSO`, else 0.
    pub data: u32,
    /// The bone this record rides.
    pub bone: u16,
    /// The point relative to [`Self::bone`]'s pivot, Bevy space, for the joint's live global.
    pub offset: Vec3,
    /// The point in model space, Bevy axes, for the model's world frame when the chain is still.
    pub point: Vec3,
}

/// One sequence in a model's [`ModelAnimations`] graph.
#[derive(Clone)]
pub struct AnimClip {
    pub anim_id: u16,
    /// The file slot the per-sequence bakes key on, not the index in [`ModelAnimations::clips`].
    pub seq_index: usize,
    pub node: AnimationNodeIndex,
    pub looping: bool,
    /// Length in seconds; a unit streamed in dead seeks its one-shot to the end pose.
    pub duration: f32,
    /// The authored gait speed (yd/s) the playback rate divides the live speed by; 0 if none.
    /// Signed: a backwards gait is authored negative and the reference's guard is `divisor > 0`
    /// (`0x5fe2f0`), so it plays at 1×. Never take `abs()`.
    pub move_speed: f32,
    /// The cross-fade into this clip, in seconds.
    pub blend_time: f32,
    /// The sequence's bounds-sphere centre, Bevy model space: the reference's mouse-pick broad
    /// phase, ahead of the posed per-triangle test (`0x7089c0`).
    pub bounds_center: Vec3,
    /// The bounds-sphere radius in model-local yards; 0 when unauthored.
    pub bounds_radius: f32,
    /// The sequence's box, Bevy model space, which sizes the blob shadow: clamped to ±5 per axis,
    /// scaled, yaw-rotated, then axis-aligned (`0x711a20`, corners at `0x6d7920`); zero when
    /// unauthored.
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// The sequence's event keyframes, by time on the clip clock.
    pub events: std::sync::Arc<[ClipEvent]>,
    /// Right- and left-arm masked nodes for the sheath family (89/90), so a draw or stow plays
    /// on one arm over the gait.
    pub arm_nodes: Option<(AnimationNodeIndex, AnimationNodeIndex)>,
    /// The upper-body masked node, so a swing or emote plays over the legs' own animation.
    pub upper_node: Option<AnimationNodeIndex>,
    /// This variation's weight in the reference's per-play roll (`M2Sequence+0x14`, `0x71248a`).
    pub frequency: u16,
    /// `(minReplay, maxReplay)` (`M2Sequence+0x18/+0x1c`): each arm rolls a play count `R` from
    /// it, so a one-shot runs `R` times. A loop uses it too: a placed doodad re-rolls its
    /// variation when the window expires, every loop for `(0, 0)`.
    pub replay: (u32, u32),
    /// Whether a bone track made a curve; a clip that poses none is only a clock and gets no rig.
    pub poses_bones: bool,
}

/// A model's animations: one `AnimationGraph` shared by its instances, each with its own player.
#[derive(Clone, Component)]
pub struct ModelAnimations {
    pub graph: Handle<AnimationGraph>,
    pub clips: Vec<AnimClip>,
    /// The `HandsClosed` grip nodes `(right, left)`, overlaid while that hand holds a weapon
    /// (`0x479660`/`0x60b590`); an empty hand or a forearm shield stays open.
    pub hand_close: [Option<AnimationNodeIndex>; 2],
    /// The header's PlayableAnimationLookup (`+0x2c/+0x30`): row `i` substitutes for requested id
    /// `i`. Empty without a table, and [`Self::resolve`] then returns the id itself.
    pub playable_animation_lookup: Vec<PlayableAnim>,
    /// The header's AnimationLookup (`+0x24/+0x28`): each id's first slot, `0xffff` for none.
    pub animation_lookup: Vec<u16>,
    /// The global-sequence bone channels (an eyelid's blink), free-running over the sequence pose.
    pub global_bones: Vec<super::GlobalBone>,
    /// Index into [`Self::clips`] of the loader-idle seed, the sequence the reference arms at load
    /// (`0x71019b`): id 0 through the playable lookup, not the first file slot, which on
    /// `DuelingFlag.m2` is Spawn. `None` when that idle is the rest pose and needs no rig.
    pub first_seq: Option<usize>,
    /// The pose evaluator's bake, mirroring [`Self::graph`].
    pub pose: std::sync::Arc<super::PoseSource>,
}

/// A requested animation id resolved to what this model plays (`0x711bf0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedAnim {
    /// The id to look up with [`ModelAnimations::find`].
    pub id: u16,
    /// PATH 1's high-16 direction code, else 0. Carried, not applied: the reference's arm negates
    /// the play rate for code 1 and zeroes it for 2 and 3 (`0x7126d2`-`0x712712`).
    pub dir_flags: u16,
}

impl ModelAnimations {
    /// The id's head variation; a one-shot that alternates uses [`Self::pick_variation`].
    pub fn find(&self, anim_id: u16) -> Option<&AnimClip> {
        self.clips.iter().find(|c| c.anim_id == anim_id)
    }

    /// Whether the model authors `anim_id` (`0x711960`), which the GameObject arm's remap branches
    /// on (`0x5f3930`). Not `find(..).is_some()`: `clips` lacks the zero-length sequences the parse
    /// skips, which the reference still counts.
    pub fn owns(&self, anim_id: u16) -> bool {
        self.animation_lookup
            .get(anim_id as usize)
            .is_some_and(|&slot| slot != 0xffff)
    }

    /// The clip a free or effect model runs: `preferred` when the model has it, else the
    /// reference's load bootstrap (`0x710153`-`0x71019b`), id 0 `Stand` at variation 0, else the
    /// first record's id (`0x710181`). Unlike [`Self::first_seq`], no bind-pose gate: an effect's
    /// emitters read the posed joints.
    pub fn preferred_clip(&self, preferred: Option<u16>) -> Option<&AnimClip> {
        preferred
            .and_then(|id| self.find(id))
            .or_else(|| self.find(0))
            .or_else(|| self.clips.first())
    }

    /// The file slot the loader-idle seed arms, [`Self::first_seq`]'s selection without its
    /// bind-pose gate: the reference arms it on every instance (`0x70ebd0`), so a consumer keyed on
    /// the playing sequence needs it, rig or not. `None` only for a model with no clip.
    pub fn idle_seq(&self) -> Option<usize> {
        self.idle_clip().map(|c| c.seq_index)
    }

    /// The loader-idle seed's clip, which a spawn site arms to start the instance's sequence clock.
    pub fn idle_clip(&self) -> Option<&AnimClip> {
        let idle_id = self
            .playable_animation_lookup
            .first()
            .map_or(0, |p| p.resolved_id);
        self.clips
            .iter()
            .find(|c| c.anim_id == idle_id)
            .or_else(|| self.clips.first())
    }

    /// Pick a variation of `anim_id` for a `rand()` roll, the reference's weighted walk
    /// (`0x7121a0`, variation −1). Exhaustion keeps the head: only the win path (`0x7124e9`)
    /// overwrites the slot set at `0x71247a`. Variations are the sequences sharing `anim_id` in
    /// file order, as retail files lay out the `variationNext` chain.
    pub fn pick_variation(&self, anim_id: u16, roll: u16) -> Option<&AnimClip> {
        let mut roll = i32::from(roll);
        let mut head = None;
        for c in self.clips.iter().filter(|c| c.anim_id == anim_id) {
            head.get_or_insert(c);
            if roll < i32::from(c.frequency) {
                return Some(c);
            }
            roll -= i32::from(c.frequency);
        }
        head
    }

    /// Resolve a requested id as the reference does before every sequence lookup (`0x711bf0`):
    /// - PATH 1, inside the playable lookup: the model's own baked substitute.
    /// - PATH 2, past it (ids 203-207 on retail data): the `AnimationData.dbc` fallback walk.
    /// - No table: the id itself.
    pub fn resolve(&self, requested: u16, catalog: &AnimDataCatalog) -> ResolvedAnim {
        if let Some(row) = self.playable_animation_lookup.get(requested as usize) {
            return ResolvedAnim {
                id: row.resolved_id,
                dir_flags: row.dir_flags,
            };
        }
        if self.playable_animation_lookup.is_empty() {
            return ResolvedAnim {
                id: requested,
                dir_flags: 0,
            };
        }
        self.resolve_path2(requested, catalog)
    }

    /// PATH 2 (`0x711c1f`): follow `catalog.fallback` until the model has a clip for the id, with
    /// the reference's 208-row cycle guard. Exhaustion gives Stand (0), where the reference takes
    /// Stand only if the model authors it, else 147 if authored, else the first sequence's id
    /// (`0x711c22`-`0x711c4e`); every id benilla requests resolves in PATH 1.
    fn resolve_path2(&self, requested: u16, catalog: &AnimDataCatalog) -> ResolvedAnim {
        const STAND: ResolvedAnim = ResolvedAnim {
            id: 0,
            dir_flags: 0,
        };
        let mut visited = [false; 208];
        let mut id = requested;
        loop {
            if self.find(id).is_some() {
                return ResolvedAnim { id, dir_flags: 0 };
            }
            match visited.get_mut(id as usize) {
                Some(seen) if !*seen => *seen = true,
                _ => return STAND, // revisited (cycle) or id ≥ 208 (NULL/max-id guard)
            }
            match catalog.fallback(id) {
                Some(next) => id = next,
                None => return STAND, // unknown row / self-fallback / already-Stand → exhaustion
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ModelAnimations::resolve ──────────────────────────────────────────────

    use bevy::animation::graph::AnimationNodeIndex;

    fn test_clip(anim_id: u16) -> AnimClip {
        AnimClip {
            anim_id,
            seq_index: 0,
            node: AnimationNodeIndex::new(0),
            looping: true,
            duration: 1.0,
            move_speed: 0.0,
            blend_time: 0.0,
            bounds_center: Vec3::ZERO,
            bounds_radius: 0.0,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            events: Vec::new().into(),
            arm_nodes: None,
            upper_node: None,
            frequency: 0,
            replay: (0, 0),
            poses_bones: true,
        }
    }

    fn test_anims(has_clips: &[u16], table: Vec<PlayableAnim>) -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: has_clips.iter().map(|&id| test_clip(id)).collect(),
            playable_animation_lookup: table,
            animation_lookup: Vec::new(),
            hand_close: [None, None],
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }

    fn playable(resolved_id: u16, dir_flags: u16) -> PlayableAnim {
        PlayableAnim {
            resolved_id,
            dir_flags,
        }
    }

    fn empty_catalog() -> AnimDataCatalog {
        AnimDataCatalog::from_rows([])
    }

    /// A chicken without Attack2H bakes `[18] = 16`.
    #[test]
    fn path1_reads_the_baked_table_entry() {
        let mut table = vec![playable(0, 0); 19];
        table[18] = playable(16, 0);
        let anims = test_anims(&[0, 16], table);
        let resolved = anims.resolve(18, &empty_catalog());
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 16,
                dir_flags: 0
            }
        );
    }

    /// HumanMale's `playableAnimationLookup[6]` is `0x00030001`: id 1, direction code 3.
    #[test]
    fn path1_carries_dir_flags_through_unapplied() {
        let mut table = vec![playable(0, 0); 7];
        table[6] = playable(1, 3);
        let anims = test_anims(&[0, 1], table);
        let resolved = anims.resolve(6, &empty_catalog());
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 1,
                dir_flags: 3
            }
        );
    }

    #[test]
    fn path2_walks_the_dbc_fallback_past_the_table() {
        let cat = AnimDataCatalog::from_rows([
            (
                5,
                benilla_formats::AnimEntry {
                    weapon_flags: 0,
                    fallback: 4,
                },
            ),
            (
                4,
                benilla_formats::AnimEntry {
                    weapon_flags: 0,
                    fallback: 3,
                },
            ),
        ]);
        let anims = test_anims(&[0, 3], vec![playable(0, 0); 3]); // table covers only ids 0..2
        let resolved = anims.resolve(5, &cat);
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 3,
                dir_flags: 0
            }
        );
    }

    #[test]
    fn path2_cycle_guard_terminates_at_stand() {
        let cat = AnimDataCatalog::from_rows([
            (
                5,
                benilla_formats::AnimEntry {
                    weapon_flags: 0,
                    fallback: 6,
                },
            ),
            (
                6,
                benilla_formats::AnimEntry {
                    weapon_flags: 0,
                    fallback: 5,
                },
            ),
        ]);
        let anims = test_anims(&[], vec![playable(0, 0); 3]); // model has neither 5 nor 6
        let resolved = anims.resolve(5, &cat);
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 0,
                dir_flags: 0
            }
        );
    }

    /// `AnimDataCatalog::fallback` gives `None` for an unknown row, a self-fallback or Stand.
    #[test]
    fn path2_self_or_null_fallback_exhausts_to_stand() {
        let anims = test_anims(&[], vec![playable(0, 0); 3]);
        let resolved = anims.resolve(99, &empty_catalog()); // id 99 has no row at all
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 0,
                dir_flags: 0
            }
        );
    }

    /// An id past the 208 rows cannot be marked visited, so it exhausts at once.
    #[test]
    fn path2_out_of_dbc_range_id_exhausts_to_stand() {
        let anims = test_anims(&[], vec![playable(0, 0); 3]);
        let resolved = anims.resolve(250, &empty_catalog());
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 0,
                dir_flags: 0
            }
        );
    }

    #[test]
    fn no_table_degrades_to_identity() {
        let anims = test_anims(&[0], Vec::new());
        let resolved = anims.resolve(42, &empty_catalog());
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 42,
                dir_flags: 0
            }
        );
    }

    // ── preferred_clip: the model-load bootstrap (`0x710153`–`0x71019b`) ─────────────────────

    /// `LightningShield_State`'s shape: slot 0 is its Decay (159).
    #[test]
    fn preferred_clip_arms_stand_over_the_file_order_first_slot() {
        let anims = test_anims(&[159, 0, 158], Vec::new());
        assert_eq!(anims.preferred_clip(None).map(|c| c.anim_id), Some(0));
    }

    /// The guarded fallback (`0x710181`), as on Cripple's effect: no Stand, slot 0 Hold (158).
    #[test]
    fn preferred_clip_falls_back_to_the_first_record_without_a_stand() {
        let anims = test_anims(&[158, 159], Vec::new());
        assert_eq!(anims.preferred_clip(None).map(|c| c.anim_id), Some(158));
    }

    #[test]
    fn preferred_clip_honours_a_named_id_then_falls_to_stand() {
        let anims = test_anims(&[159, 0, 158], Vec::new());
        assert_eq!(
            anims.preferred_clip(Some(158)).map(|c| c.anim_id),
            Some(158)
        );
        assert_eq!(anims.preferred_clip(Some(42)).map(|c| c.anim_id), Some(0));
    }

    // ── idle_seq: the loader-idle file slot, apart from the rig gate ─────────────────────────

    /// `test_anims` with each clip's `seq_index` set to its file slot.
    fn slotted(ids: &[u16], table: Vec<PlayableAnim>) -> ModelAnimations {
        let mut a = test_anims(ids, table);
        for (i, c) in a.clips.iter_mut().enumerate() {
            c.seq_index = i;
        }
        a
    }

    /// The Spawn/Stand/Despawn GameObject shape (`DuelingFlag`, `ArenaFlag`, battlefield banners).
    #[test]
    fn idle_seq_is_the_stand_slot_not_file_slot_zero() {
        let anims = slotted(&[145, 0, 157], Vec::new());
        assert_eq!(anims.idle_seq(), Some(1));
    }

    #[test]
    fn idle_seq_answers_even_when_the_content_gate_skipped_the_arm() {
        let anims = slotted(&[145, 0, 157], Vec::new());
        assert_eq!(anims.first_seq, None, "the render gate declined to arm");
        assert_eq!(
            anims.idle_seq(),
            Some(1),
            "the sequence identity is still known"
        );
    }

    /// `Cripple_State_Base` remaps requested id 0 to 158 (Hold).
    #[test]
    fn idle_seq_resolves_id_zero_through_the_playable_lookup() {
        let anims = slotted(&[159, 158], vec![playable(158, 0)]);
        assert_eq!(anims.idle_seq(), Some(1));
    }

    #[test]
    fn idle_seq_falls_back_to_the_first_record_then_to_none() {
        assert_eq!(slotted(&[158, 159], Vec::new()).idle_seq(), Some(0));
        assert_eq!(slotted(&[], Vec::new()).idle_seq(), None);
    }

    /// A roll past every weight gives the head variation, not the last.
    #[test]
    fn pick_variation_walks_the_weighted_chain() {
        let mut anims = test_anims(&[16, 16, 5], Vec::new());
        anims.clips[0].frequency = 0x6000; // Attack1H variation 0, the common arc
        anims.clips[1].frequency = 0x1fff; // variation 1, the off-side swing
        let pick = |anims: &ModelAnimations, id, roll| {
            let c = anims.pick_variation(id, roll).unwrap();
            anims.clips.iter().position(|x| std::ptr::eq(x, c)).unwrap()
        };
        assert_eq!(pick(&anims, 16, 0x0000), 0);
        assert_eq!(pick(&anims, 16, 0x5fff), 0); // last roll inside variation 0's weight
        assert_eq!(pick(&anims, 16, 0x6000), 1); // first roll past it
        assert_eq!(pick(&anims, 16, 0x7ffe), 1);
        // A roll past the weights keeps the head (the reference's exhaust path, `0x7124de`).
        assert_eq!(pick(&anims, 16, 0x7fff), 0);
        assert_eq!(pick(&anims, 5, 0x7fff), 2); // lone variation wins by exhaustion→head (weight 0)
        assert!(anims.pick_variation(99, 0).is_none());
    }
}
