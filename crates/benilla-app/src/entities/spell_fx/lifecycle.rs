//! The effect-model **animation lifecycle**: `Stand` → `Hold` → `Decay`, the state machine a
//! spell-visual `CEffect`'s attached model runs for as long as it lives. Split from [`super`] along
//! the concern seam: that file owns the *instances* (the model cache, the attach cascade, the reap,
//! the world plants); this one owns *what they play*.
//!
//! ## The mechanism
//!
//! A `CEffect`'s model is armed **by `AnimationData.dbc` id, never by file order**. The model-load
//! bootstrap (`0x710153`–`0x71019b`) arms **id 0 `Stand`, variation 0** — the *birth*
//! ([`benilla_assets::ModelAnimations::preferred_clip`] is that resolve). When the armed sequence's
//! authored span elapses, the CM2 Advance `0x719370` latches once (`block+0xc0`) and enqueues the
//! model's registered completion callback; the scene tick drains it at `0x707595` at the **end of
//! the same tick**, passing **the id that just completed** as its first argument. `Which` callback
//! is registered is chosen by `PlaySpellVisualKit 0x60edf0` **per stage** — that table is
//! [`FxStage`], and this module is its three arms:
//!
//! - **[`FxStage::OneShot`]** (stages 0/1, `0x5fbf50`) — destroy at the first completion. Modelled
//!   by the instance's own span clock in [`super::attach_spell_fx`], so nothing here watches it.
//! - **[`FxStage::State`]** (stage 2, `0x5ff170`) — iff the model authors **`Hold` (158)**, arm it
//!   and keep it running for the effect's whole life; if it does not, do **nothing at all** (no
//!   destroy, no re-loop — the model simply stays parked on its birth sequence). The reference's
//!   "keep running" is a literal re-arm every span by `0x5ff1d0`, gated on a deadline at
//!   `node+0x58` that is **nonzero only at stage 3** — so for an aura state or a channel it never
//!   fires and Hold repeats forever. **This is Ice Barrier's pulse.**
//! - **[`FxStage::Relive`]** (stages 3/4, `0x60ed00`) — re-arm **the id that just completed**,
//!   forever, with no `Hold` lookup, no deadline and no destroy. A precast whose birth sequence
//!   clamps therefore still repeats it.
//!
//! Reap ([`FxDecay`]) is the same shape from the other end (`0x614150` → `0x6141c0`): if the model
//! authors **`Decay` (159)**, arm it and the instance **keeps rendering for that sequence's
//! authored span** before it despawns; otherwise it goes immediately. `0x6203e0` — the destructor —
//! plays nothing itself (its `0x4` bit is owner-attachment teardown, not a decay-out).
//!
//! ## What this replaces, and how wrong it was
//!
//! Every effect instance used to arm **one** clip for its whole life — the model's *file-order-first*
//! sequence, repeated only if that sequence's own loop flag was clear. For `Spells\IceShield_State`
//! (Ice Barrier's state kit) that is the 0.70 s clamping birth, so the shield grew in and then held
//! its last frame for the aura's entire duration. `benilla-extract fxlifescan` sizes the class:
//! **163 of 9691 models author a `Hold`/`Decay` leg** (158 of them under `Spells\`), **116 of which
//! froze** exactly this way — Mana Shield, Lightning Shield, Divine Shield, Frost Nova, the Fire and
//! Frost Wards, Immolate, Net, the healing auras.
//!
//! ## Named divergences
//!
//! - The reference re-arms `Hold` with variation `-1` (a `_rand`-weighted walk of the alias chain),
//!   so a model authoring several `Hold` variations could pick a different one each pass. We run one
//!   repeating play instead. Corpus: of the 692 `Spells\` models, 507 have a single sequence, 119
//!   two, 65 three and **one** has four or more — so a multi-variation `Hold` is at most that single
//!   model, and it is not worth a per-pass re-roll's complexity until one shows.
//! - A **ribbon**'s per-sequence visibility is still decided once at spawn ([`benilla_world::ribbons`],
//!   which spawns per fixed-sequence entity). A trail authored dark in `Stand` and lit in `Hold`
//!   would stay dark. Left as a residual: the ribbon lane's own spawn shape has to change for it,
//!   and no reported effect turns on it.

use benilla_assets::ModelAnimations;
use bevy::animation::{graph::AnimationNodeIndex, RepeatAnimation};
use bevy::prelude::*;

use crate::creature_anim::FxStage;

/// `AnimationData.dbc` **158 `Hold`** — the sustained pulse leg (`0x9e` at `0x5ff188`/`0x5ff1bb`).
pub(crate) const ANIM_HOLD: u16 = 158;
/// `AnimationData.dbc` **159 `Decay`** — the fade-out leg (`0x9f` at `0x5ff233`/`0x6141c0`).
pub(crate) const ANIM_DECAY: u16 = 159;

/// The completion callback one effect-model instance carries — the ECS twin of the `model+0x70`
/// registration `0x711bb0` writes, and of the callback *swap* `0x5ff170` performs when it hands the
/// birth over to `Hold`.
///
/// Present on every kit-effect instance root that armed a rig, whatever its stage: the reap
/// ([`arm_decay`]) needs a handle on the player of an instance whose lifecycle is otherwise
/// finished. A missile, an item glow or the `fxview` fixture arms none — they are not `CEffect`s
/// (the missile is the separate `CMissile` TU) and keep the plain single-clip arm.
#[derive(Component)]
pub(crate) enum FxAnimLife {
    /// Stage 2's birth, waiting to hand over — `0x5ff170` is the only callback that watches for a
    /// completion and then does something *other* than re-arm or destroy.
    Birth(AnimationNodeIndex),
    /// Nothing left to advance: the birth handed over to `Hold` (whose "re-arm every span" is the
    /// repeating play), or this stage never had a handover at all — `0x5fbf50`'s destroy is the
    /// instance's own span clock, and `0x60ed00`'s re-arm-forever is the repeat armed below. The
    /// node is still carried so a reap can stop it before arming `Decay`.
    Settled(AnimationNodeIndex),
}

impl FxAnimLife {
    /// Arm `clip` on `player` for `stage` and return the watcher that finishes the job.
    ///
    /// The repeat policy is the stage's, not the sequence flag's alone: a [`FxStage::Relive`]
    /// instance re-arms unconditionally (`0x60ed00` consults no flag, so a clamping precast still
    /// repeats), while the other stages play the sequence exactly as the M2 sampler would —
    /// wrapping on a bit0-CLEAR sequence (`0x71462a`'s modulo), holding the last frame on a
    /// bit0-SET one (`0x7145db`'s clamp).
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

    /// The graph node currently armed.
    fn armed(&self) -> AnimationNodeIndex {
        match self {
            Self::Birth(n) | Self::Settled(n) => *n,
        }
    }
}

/// Marker: this instance has been reaped and should play its **`Decay`** out (`0x614150`'s
/// `0x6141c0`). Written by [`super::resolve_spell_fx`], which also sets the instance's expiry to
/// the decay span; consumed once by [`advance_fx_anim`], which arms the clip through [`arm_decay`].
#[derive(Component)]
pub(crate) struct FxDecay;

/// Run the completion callbacks of every live effect instance — the birth → `Hold` handover, and
/// the reap's `Decay` arm.
///
/// The reference dispatches these at the END of the tick that advanced the models (`0x707595` in
/// the scene tick `0x7074b0`); Bevy advances `AnimationPlayer`s in `PreUpdate`, so an `Update`
/// system reading [`bevy::animation::ActiveAnimation::completions`] fires in the same frame the
/// span elapsed. Completion is read as `completions() >= 1` rather than `is_finished()` precisely
/// because a wrapping birth never "finishes" — the reference's latch fires at the first span end
/// either way (`0x7194bc`, LOOP-flag-independent).
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
            // The lifecycle is over: the instance's expiry clock owns the despawn from here, and
            // nothing may re-arm over the decay.
            commands.entity(root).try_remove::<FxAnimLife>();
            continue;
        }
        let FxAnimLife::Birth(armed) = *life else {
            continue; // settled — no callback of this stage's changes anything on completion
        };
        // `0x719370`'s fire-once latch: one notification per authored span.
        if !player
            .animation(armed)
            .is_some_and(|a| a.completions() >= 1)
        {
            continue;
        }
        // `0x5ff170`: the birth is over. Iff the model authors Hold, arm it and keep it running —
        // `0x5ff1d0` then re-arms it every span while the deadline `node+0x58` is unset, and that
        // deadline is nonzero at stage 3 alone. Otherwise do nothing whatsoever: the model is left
        // parked on its birth, which is both the reference's behaviour and what we already did.
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

/// The lifecycle's instrument (`WOW_MOVE_TRACE=<path>`, tag `fx`) — one line per leg change, beside
/// the `kit spawn` / `kit expire` lines the instance lane already writes.
///
/// This is how "the Ice Barrier shield is frozen" is *closed by measurement* rather than by eye
/// (docs/METHOD.md §4/§5): with the lifecycle dead the trace shows `kit spawn` and nothing after; with it
/// live the same cast prints `fx leg hold e=… anim=158` one birth-span (0.70 s) later, and the aura
/// drop prints `fx leg decay` followed by the `kit expire` a further 1.10 s on. A duration question
/// is answered from timestamps, never from watching.
pub(super) fn trace_leg(leg: &str, root: Entity, anim_id: u16) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    benilla_assets::trace::line("fx", &format!("leg {leg} e={root} anim={anim_id}"));
}

/// The reap's decay-out (`0x614150`, gates at `0x614187`–`0x6141a1`): arm `Decay` if the model
/// authors it. When it does not, nothing is armed and the caller's immediate expiry stands — the
/// reference's `je 0x6141d6` straight to the destructor.
fn arm_decay(player: &mut AnimationPlayer, armed: AnimationNodeIndex, anims: &ModelAnimations) {
    let Some(decay) = anims.find(ANIM_DECAY) else {
        return;
    };
    player.stop(armed);
    // Never repeated, whatever the sequence flags say: the instance is destroyed at this
    // sequence's completion, and 61 of the corpus's 62 Decay sequences clamp anyway.
    player
        .play(decay.node)
        .set_repeat(RepeatAnimation::Never)
        .replay();
}

/// The authored span of a model's `Decay` sequence — how long a reaped instance keeps rendering
/// before it despawns (`0x6141f0`: the node is *not* torn down synchronously). `None` when the
/// model authors no `Decay`, which is the reference's immediate-destroy gate.
pub(crate) fn decay_span(anims: Option<&ModelAnimations>) -> Option<f32> {
    anims?.find(ANIM_DECAY).map(|c| c.duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::AnimClip;
    use bevy::animation::RepeatAnimation;

    /// One clip of `anim_id`, with the M2 loop flag `looping`, on graph node `node`.
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

    /// A stage-2 instance opens on its birth and keeps a watcher: `0x5ff170` is the one callback
    /// that has a handover to make. `IceShield_State`'s birth clamps, so it is armed unrepeated —
    /// the freeze the whole fix is about is *correct* right up until the completion fires.
    #[test]
    fn state_arms_the_birth_and_watches_for_the_handover() {
        let mut player = AnimationPlayer::default();
        let life = FxAnimLife::arm(&mut player, &clip(0, 0, false), FxStage::State);
        assert!(matches!(life, FxAnimLife::Birth(n) if n == AnimationNodeIndex::new(0)));
        assert_eq!(repeat(&player, 0), RepeatAnimation::Never);
    }

    /// Stages 3/4 (`0x60ed00`) re-arm the completed id **unguarded** — the repeat does not consult
    /// the sequence's own clamp bit, so a precast whose `Stand` clamps still repeats where a
    /// loop-flag-only arm would freeze it. And there is nothing left to watch.
    #[test]
    fn relive_repeats_even_a_clamping_sequence() {
        let mut player = AnimationPlayer::default();
        let life = FxAnimLife::arm(&mut player, &clip(0, 0, false), FxStage::Relive);
        assert!(matches!(life, FxAnimLife::Settled(_)));
        assert_eq!(repeat(&player, 0), RepeatAnimation::Forever);
    }

    /// A stage-0/1 one-shot plays its sequence exactly as the M2 sampler would — the clamp bit
    /// decides — and self-terminates on the instance's span clock, not here.
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

    /// The reap's decay-out and its gate (`0x6141a1`): a model authoring `Decay` reports the span
    /// the instance must keep rendering for; one that does not reports `None`, which is the
    /// reference's straight-to-destructor branch.
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
