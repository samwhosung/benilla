//! The per-hand weapon-grip finger overlay.

use benilla_assets::ModelAnimations;
use bevy::prelude::*;

use super::super::{HandGrip, HAND_GRIP_WEIGHT};

/// Curls a hand's fingers (`HandsClosed`, on the finger-mask nodes over the gait) while a weapon
/// occupies its attach point, as `CloseHand 0x479660` does. Runs after [`super::drive_animations`].
pub(in super::super) fn drive_hand_grip(
    mut units: Query<(&HandGrip, &ModelAnimations, &mut AnimationPlayer)>,
) {
    for (grip, anims, mut player) in &mut units {
        for (hand, want) in [(0usize, grip.right), (1, grip.left)] {
            let Some(node) = anims.hand_close[hand] else {
                continue; // no finger key-bones or no HandsClosed
            };
            match (want, player.animation(node).is_some()) {
                (true, false) => {
                    let active = player.play(node);
                    active.repeat(); // a single-key pose: repeat so it never finishes
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
