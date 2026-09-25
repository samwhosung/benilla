//! The rendered body heading and the animation's view of the flags, the reference's display facing
//! (the `0x607ed0` tail). [`super::control`] calls [`drive_body_heading`] once per controlled
//! frame, after the move flags are final.

use crate::creature_anim::{ease_strafe_yaw, move_flags, strafe_body_offset, wrap_pi};

use super::{Player, STATIONARY_CHASE_RATE};

/// Advances `model_yaw` one frame and returns the animation's view of the move flags (the wire
/// keeps the real key flags). Strafing eases the body toward `face_yaw ± 90°` (`± 45°` diagonal,
/// mirrored backpedaling) by a quarter of the gap a frame in aim-relative offset space, so a
/// left-right flip swings round the front. Moving forward or back or airborne (`flags & 0x2003`),
/// or swimming, snaps it to the aim; standing runs the frozen chase (`0x6081bf`), holding the 90°
/// ceiling while the aim is steered, then sweeping onto it at 8× the turn rate. The swim pitch is
/// applied at the transform write (`0x60a110`). The turn-in-place shuffle keys on the body's own
/// rotation (chase-step bits `0x800`/`0x1000`), not the turn keys, so a deck's turn never shuffles.
pub(super) fn drive_body_heading(
    player: &mut Player,
    move_flags_now: u32,
    dt: f32,
    swimming: bool,
    moving: bool,
    airborne: bool,
    steering: bool,
    // The mover's own turn rate in rad/s: a possessed creature settles at its own pace.
    turn_rate: f32,
) -> u32 {
    let strafe_offset = if swimming {
        0.0
    } else {
        strafe_body_offset(move_flags_now)
    };
    let mut body_turn_step = 0.0_f32;
    if swimming {
        player.model_yaw = player.face_yaw;
    } else if strafe_offset != 0.0 {
        player.model_yaw = ease_strafe_yaw(player.model_yaw, player.face_yaw, strafe_offset, dt);
    } else if moving || airborne {
        player.model_yaw = player.face_yaw;
    } else {
        let delta = wrap_pi(player.face_yaw - player.model_yaw);
        let mut step = (delta.abs() - std::f32::consts::FRAC_PI_2).max(0.0); // the ceiling
        if !steering {
            step += dt * turn_rate * STATIONARY_CHASE_RATE; // the release sweep
        }
        body_turn_step = step.min(delta.abs()).copysign(delta);
        player.model_yaw = wrap_pi(player.model_yaw + body_turn_step);
    }

    let mut anim_flags = move_flags_now;
    if !swimming && !moving && !airborne {
        anim_flags &= !(move_flags::TURN_LEFT | move_flags::TURN_RIGHT);
        if body_turn_step > 1e-5 {
            anim_flags |= move_flags::TURN_LEFT; // +yaw = turning left
        } else if body_turn_step < -1e-5 {
            anim_flags |= move_flags::TURN_RIGHT;
        }
    }
    anim_flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standing_body_sweeps_back_at_the_movers_own_rate() {
        let sweep = |turn_rate: f32| {
            let mut player = Player {
                face_yaw: 1.0,
                model_yaw: 0.0,
                ..Default::default()
            };
            drive_body_heading(&mut player, 0, 0.01, false, false, false, false, turn_rate);
            player.model_yaw
        };
        // Below the 90° ceiling the whole step is the sweep, so it scales with the rate.
        let slow = sweep(std::f32::consts::PI / 4.0);
        let fast = sweep(std::f32::consts::PI);
        assert!(slow > 0.0 && fast > slow);
        assert!(
            (fast / slow - 4.0).abs() < 1e-3,
            "a 4× turn rate sweeps 4× as far in a frame: {slow} vs {fast}"
        );
        // Steering freezes the chase whatever the rate.
        let mut player = Player {
            face_yaw: 1.0,
            model_yaw: 0.0,
            ..Default::default()
        };
        drive_body_heading(
            &mut player,
            0,
            0.01,
            false,
            false,
            false,
            true,
            std::f32::consts::PI,
        );
        assert_eq!(
            player.model_yaw, 0.0,
            "under the 90° ceiling a steering frame moves the body not at all"
        );
    }
}
