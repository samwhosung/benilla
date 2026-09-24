//! The per-hand weapon-grip finger overlay ([`drive_hand_grip`]) — split out of [`super`] as its
//! own concern; re-exported by [`super`] so `driver::drive_hand_grip` keeps working unchanged.

use benilla_assets::ModelAnimations;
use bevy::prelude::*;

use super::super::{HandGrip, HAND_GRIP_WEIGHT};

/// Hold each hand's `HandsClosed` finger pose while that hand grips a weapon, release it when it doesn't
/// — the persistent weapon-grip overlay (`CloseHand 0x479660`: the client arms the finger
/// key-bones with `HandsClosed` purely by a weapon occupying that hand's attach point). Played *over* the
/// gait on the finger-mask nodes, so the arm keeps its animation while only the fingers curl; on release
/// the fingers fall back to the base gait's flat Stand rest. Idempotent — plays only on the open→closed
/// edge and stops on closed→open, so it rides gait changes untouched (like the sheath/one-shot overlays).
/// Runs after [`super::drive_animations`], re-asserting over whatever the base machine set this frame.
pub(in super::super) fn drive_hand_grip(
    mut units: Query<(&HandGrip, &ModelAnimations, &mut AnimationPlayer)>,
) {
    for (grip, anims, mut player) in &mut units {
        for (hand, want) in [(0usize, grip.right), (1, grip.left)] {
            let Some(node) = anims.hand_close[hand] else {
                continue; // this model can't grip with this hand (no finger key-bones / no HandsClosed)
            };
            match (want, player.animation(node).is_some()) {
                (true, false) => {
                    let active = player.play(node);
                    active.repeat(); // a single-key clamp pose — hold it, don't let it "finish"
                    active.set_weight(HAND_GRIP_WEIGHT);
                }
                (false, true) => {
                    player.stop(node);
                }
                _ => {}
            }
        }
    }
}
