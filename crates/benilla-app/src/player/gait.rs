//! The rendered **body heading** + the animation's view of the flags — the client's display-facing
//! pose (the `0x607ed0` tail; supersedes 0051's
//! ease-toward-velocity), beside its concern like `mover`/`swim`/`arc`. [`super::control`] calls
//! [`drive_body_heading`] once per controlled frame, after this frame's move flags are final.

use crate::creature_anim::{ease_strafe_yaw, move_flags, strafe_body_offset, wrap_pi};

use super::{Player, STATIONARY_CHASE_RATE};

/// Advance `model_yaw` (the rendered body heading) one frame and return the **animation's** view of
/// the move flags (the wire keeps the real key flags — observers integrate the facing from them):
///
///  · **strafing** — the root turns to `face_yaw ± 90°` (pure) / `± 45°` (diagonal, mirrored
///    when backpedaling), eased a quarter of the remaining gap per frame **in aim-relative
///    offset space** (a left↔right flip is an exact 180° tie in absolute yaw — offset space
///    always swings it around the front); the SpineLow/Head counter-twist walks the upper
///    body back so the head keeps looking at the aim.
///  · **moving fwd/back or airborne** — snap to the aim (the client's `flags & 0x2003` snap;
///    a backpedal keeps facing forward and plays WalkBackwards).
///  · **standing** — the FROZEN chase (`0x6081bf`): while the aim is being
///    steered (A/D turn keys or mouse-look) the catch-up is off and only the 90° ceiling
///    applies — the camera/aim and the head-twist lead while the body holds, then the body
///    follows the turn lagging exactly 90°; once the steering stops it sweeps back onto the
///    aim at 8× the turn rate (~63 ms for 90°). The lag mechanism is the freeze, not a slow
///    rate — the client stamps its chase clock every non-steering frame.
///  · **swimming** ignores the ground display-facing pose (no strafe body-offset, no standing
///    chase): the body SNAPs to the aim (the client's facing snap list — dead or swimming) and
///    the swim *pitch* tilts it in `control`'s transform write (the render law `0x60a110`).
///
/// The turn-in-place foot-shuffle rides the BODY's actual rotation (the client's chase-step bits
/// `0x800/0x1000`, cleared each frame after the anim layer reads them) — NOT the turn keys: while
/// the frozen chase holds the body under a leading head, the feet hold too, and a stationary
/// MOUSE turn shuffles once the body steps (no key flag involved). A deck turning under the rider
/// carries `model_yaw` rigidly at the ride block, so it never registers as a body step here —
/// the shuffle (and its keyframed step sounds) only ever sees real turns.
// The eighth parameter arrived with the possessed mover's own turn rate (1278) and clippy's
// default bound is seven. These are one frame of mover state, each read from a different one of
// the caller's queries — bundling them into a struct would name nothing the call site does not
// already say, and the gate is `-D warnings`, so the honest move is to say why, here.
pub(super) fn drive_body_heading(
    player: &mut Player,
    move_flags_now: u32,
    dt: f32,
    swimming: bool,
    moving: bool,
    airborne: bool,
    steering: bool,
    // The mover's own turn rate (rad/s) — the release sweep is a multiple of it, so a possessed
    // creature's body settles onto its aim at the creature's pace.
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

    /// The release sweep is a multiple of the **mover's** turn rate, not of a constant. It is the
    /// one place the rate reaches the *rendered* body rather than the aim, so a possessed creature
    /// with a slow turn rate would otherwise snap its body around at a human's pace while its
    /// facing crawled — the head and the body disagreeing about what they are attached to.
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
        // Below the 90° ceiling, the whole step is the sweep — so it scales linearly with the rate.
        let slow = sweep(std::f32::consts::PI / 4.0);
        let fast = sweep(std::f32::consts::PI);
        assert!(slow > 0.0 && fast > slow);
        assert!(
            (fast / slow - 4.0).abs() < 1e-3,
            "a 4× turn rate sweeps 4× as far in a frame: {slow} vs {fast}"
        );
        // Steering freezes the chase whatever the rate — the byte rule, not a slow sweep.
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
