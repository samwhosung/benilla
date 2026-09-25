//! The effect-model animation lifecycle, `Stand` → `Hold` → `Decay`, of a spell-visual `CEffect`.
//!
//! The model-load bootstrap (`0x710153`–`0x71019b`) arms `AnimationData.dbc` id 0 `Stand`, not the
//! file-order-first sequence. At each span's end the scene tick runs the model's completion
//! callback with the completed id (`0x719370`, `0x707595`), picked per [`FxStage`] by
//! `PlaySpellVisualKit 0x60edf0`: stages 0/1 destroy (`0x5fbf50`), which the span clock in
//! [`super::attach_spell_fx`] models; stage 2 hands over to `Hold` if the model authors it, else
//! stays on the birth (`0x5ff170`); stages 3/4 re-arm the completed id forever (`0x60ed00`). The
//! reap plays `Decay` out (`0x614150`); the destructor `0x6203e0` plays nothing.
//!
//! Deviation: `Hold` is one repeating play where the reference re-arms it each pass with variation
//! -1, a weighted random pick, because at most one `Spells\` model authors several variations.

use benilla_assets::ModelAnimations;
use bevy::animation::{graph::AnimationNodeIndex, RepeatAnimation};
use bevy::prelude::*;

use crate::creature_anim::FxStage;

/// `AnimationData.dbc` 158 `Hold`, the sustained pulse leg (`0x9e` at `0x5ff188`/`0x5ff1bb`).
pub(crate) const ANIM_HOLD: u16 = 158;
/// `AnimationData.dbc` 159 `Decay`, the fade-out leg (`0x9f` at `0x5ff233`/`0x6141c0`).
pub(crate) const ANIM_DECAY: u16 = 159;

/// The completion callback an effect instance carries (the `model+0x70` registration `0x711bb0`
/// writes). On every kit-effect root that armed a rig, since the reap needs its player; a missile,
/// an item glow or the `fxview` fixture is not a `CEffect` and carries none.
#[derive(Component)]
pub(crate) enum FxAnimLife {
    /// Stage 2's birth, waiting for `0x5ff170`'s handover to `Hold`.
    Birth(AnimationNodeIndex),
    /// Nothing left to advance; the node is kept so a reap can stop it before arming `Decay`.
    Settled(AnimationNodeIndex),
}

impl FxAnimLife {
    /// Arm `clip` for `stage`. [`FxStage::Relive`] repeats whatever the sequence flag says
    /// (`0x60ed00` reads none); the other stages wrap a bit-0-clear sequence (`0x71462a`) and clamp
    /// a bit-0-set one (`0x7145db`), as the M2 sampler does.
    pub(super) fn arm(
        player: &mut AnimationPlayer,
        clip: &benilla_assets::AnimClip,
        stage: FxStage,
    ) -> Self {
        let play = player.play(clip.node);
        if clip.looping || stage == FxStage::Relive {
            play.repeat();
        }
        match stage {
            FxStage::State => Self::Birth(clip.node),
            FxStage::OneShot | FxStage::Relive => Self::Settled(clip.node),
        }
    }

    fn armed(&self) -> AnimationNodeIndex {
        match self {
            Self::Birth(n) | Self::Settled(n) => *n,
        }
    }
}

/// Marks a reaped instance that plays its `Decay` out (`0x6141c0`); its expiry is already set to
/// the decay span.
#[derive(Component)]
pub(crate) struct FxDecay;

/// Run each instance's completion callback, the `Hold` handover or the reap's `Decay`. The
/// reference drains them at the end of the scene tick (`0x707595` in `0x7074b0`); run in `Update`,
/// after `PreUpdate` advances the players, this fires in the frame the span ends. Completion is
/// `completions() >= 1`, not `is_finished()`: a wrapping birth never finishes, and the reference
/// latches at the first span end either way (`0x7194bc`).
pub(crate) fn advance_fx_anim(
    mut commands: Commands,
    mut instances: Query<(
        Entity,
        &mut FxAnimLife,
        &mut AnimationPlayer,
        &ModelAnimations,
        Has<FxDecay>,
    )>,
) {
    for (root, mut life, mut player, anims, decaying) in &mut instances {
        if decaying {
            arm_decay(&mut player, life.armed(), anims);
            // The expiry clock owns the despawn now, and nothing may re-arm over the decay.
            commands.entity(root).try_remove::<FxAnimLife>();
            continue;
        }
        let FxAnimLife::Birth(armed) = *life else {
            continue; // settled: nothing changes on completion
        };
        // `0x719370`'s fire-once latch: one notification per authored span.
        if !player
            .animation(armed)
            .is_some_and(|a| a.completions() >= 1)
        {
            continue;
        }
        // `0x5ff170`: arm `Hold` if the model authors it and keep it running (`0x5ff1d0`'s deadline
        // `node+0x58` is set only at stage 3); otherwise stay parked on the birth.
        match anims.find(ANIM_HOLD) {
            Some(hold) => {
                player.stop(armed);
                player.play(hold.node).set_repeat(RepeatAnimation::Forever);
                *life = FxAnimLife::Settled(hold.node);
                trace_leg("hold", root, ANIM_HOLD);
            }
            None => {
                *life = FxAnimLife::Settled(armed);
                trace_leg("park", root, 0);
            }
        }
    }
}

/// The lifecycle's trace line (`WOW_MOVE_TRACE=<path>`, tag `fx`), one per leg change, beside the
/// instance lane's `kit spawn` and `kit expire`.
pub(super) fn trace_leg(leg: &str, root: Entity, anim_id: u16) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    benilla_assets::trace::line("fx", &format!("leg {leg} e={root} anim={anim_id}"));
}

/// The reap's decay-out (`0x614150`, gates `0x614187`–`0x6141a1`): arm `Decay` if the model authors
/// it; otherwise the caller's immediate expiry stands (`0x6141d6`, straight to the destructor).
fn arm_decay(player: &mut AnimationPlayer, armed: AnimationNodeIndex, anims: &ModelAnimations) {
    let Some(decay) = anims.find(ANIM_DECAY) else {
        return;
    };
    player.stop(armed);
    // Never repeated: the instance is destroyed at this sequence's completion.
    player
        .play(decay.node)
        .set_repeat(RepeatAnimation::Never)
        .replay();
}

/// How long a reaped instance keeps rendering (`0x6141f0`): its model's `Decay` span, or `None`
/// when it authors none and is destroyed at once.
pub(crate) fn decay_span(anims: Option<&ModelAnimations>) -> Option<f32> {
    anims?.find(ANIM_DECAY).map(|c| c.duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::AnimClip;
    use bevy::animation::RepeatAnimation;

    /// One clip of `anim_id` with the M2 loop flag `looping`, on graph node `node`.
    fn clip(anim_id: u16, node: usize, looping: bool) -> AnimClip {
        AnimClip {
            anim_id,
            seq_index: node,
            node: AnimationNodeIndex::new(node),
            looping,
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

    fn repeat(player: &AnimationPlayer, node: usize) -> RepeatAnimation {
        player
            .animation(AnimationNodeIndex::new(node))
            .expect("armed")
            .repeat_mode()
    }

    /// `IceShield_State`'s birth clamps: armed unrepeated, watching for `0x5ff170`'s handover.
    #[test]
    fn state_arms_the_birth_and_watches_for_the_handover() {
        let mut player = AnimationPlayer::default();
        let life = FxAnimLife::arm(&mut player, &clip(0, 0, false), FxStage::State);
        assert!(matches!(life, FxAnimLife::Birth(n) if n == AnimationNodeIndex::new(0)));
        assert_eq!(repeat(&player, 0), RepeatAnimation::Never);
    }

    /// `0x60ed00` re-arms without reading the clamp bit, so a clamping precast repeats.
    #[test]
    fn relive_repeats_even_a_clamping_sequence() {
        let mut player = AnimationPlayer::default();
        let life = FxAnimLife::arm(&mut player, &clip(0, 0, false), FxStage::Relive);
        assert!(matches!(life, FxAnimLife::Settled(_)));
        assert_eq!(repeat(&player, 0), RepeatAnimation::Forever);
    }

    /// The clamp bit decides; a one-shot ends on the instance's span clock, not here.
    #[test]
    fn oneshot_follows_the_sequence_flag_only() {
        let mut clamped = AnimationPlayer::default();
        let life = FxAnimLife::arm(&mut clamped, &clip(0, 0, false), FxStage::OneShot);
        assert!(matches!(life, FxAnimLife::Settled(_)));
        assert_eq!(repeat(&clamped, 0), RepeatAnimation::Never);

        let mut wrapping = AnimationPlayer::default();
        FxAnimLife::arm(&mut wrapping, &clip(0, 0, true), FxStage::OneShot);
        assert_eq!(repeat(&wrapping, 0), RepeatAnimation::Forever);
    }

    /// The reap's gate (`0x6141a1`): no `Decay` is `None`, the straight-to-destructor branch.
    #[test]
    fn decay_span_is_the_gate_and_the_lifetime() {
        let with = test_anims(&[clip(0, 0, false), clip(ANIM_HOLD, 1, true), {
            let mut c = clip(ANIM_DECAY, 2, false);
            c.duration = 1.1;
            c
        }]);
        assert_eq!(decay_span(Some(&with)), Some(1.1));

        let without = test_anims(&[clip(0, 0, false), clip(ANIM_HOLD, 1, true)]);
        assert_eq!(decay_span(Some(&without)), None);
        assert_eq!(decay_span(None), None);
    }

    fn test_anims(clips: &[AnimClip]) -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: clips.to_vec(),
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }
}
