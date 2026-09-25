//! This frame's move-flag word, the reference's `CMovement+0x40`, rebuilt from state every frame
//! so the animation selector, the outbound `MSG_MOVE_*` stream and the local gates (the sit
//! refusal, the cast self-cancel) read one answer. The wire word keeps the direction bits live in
//! mid-air, as a 1.12.1 capture shows the reference's do; the pose word freezes them at takeoff.

use crate::creature_anim::move_flags;

use super::{input, state, Player};

/// The two flag words this frame, plus the two facts the wire lifecycle needs from the arc.
pub(super) struct FrameFlags {
    /// What goes on the wire (and into the local gates): direction bits stay live mid-air.
    pub wire: u32,
    /// What the animation sees: direction bits frozen at take-off.
    pub pose: u32,
    /// The arc landed this frame: the `MSG_MOVE_FALL_LAND` edge.
    pub landed: bool,
    /// Milliseconds since the arc began, taken before the landing clears it; vmangos gates fall
    /// damage on it.
    pub fall_time: u32,
}

/// Builds this frame's two flag words. `swim` is `Some((fwd, side))` while swimming, the netted
/// amounts that drove the mover.
pub(super) fn this_frame(
    player: &mut Player,
    axes: &input::MoveAxes,
    swim: Option<(f32, f32)>,
    airborne: bool,
    jumped: bool,
    held: bool,
    air_nudged: bool,
    // The reference's input predicates `0x514560` and `0x5145b0` (`state::may_translate`,
    // `state::may_turn`); death takes both down.
    may_translate: bool,
    may_turn: bool,
    now: f32,
    launch_y: f32,
) -> FrameFlags {
    // Taken before the arc bookkeeping clears `airborne_since` on the landing frame, so FALL_LAND
    // carries the whole fall: vmangos `Player::HandleFall` charges no damage under 1229 ms.
    let wire_fall_time = if jumped {
        // A jump starts a fresh arc at zero, even a same-frame relaunch whose `airborne_since`
        // is still the last arc's.
        0
    } else {
        player
            .airborne_since
            .map_or(0, |t0| ((now - t0) * 1000.0).max(0.0) as u32)
    };
    // Every granted mover mode rides every packet, root included: the reference's builder reads
    // the `[cmov+0x40]` the server's merge wrote them into, and a mode dropped here is cleared by
    // the server's next echo.
    let mut move_flags_now = player.modes.wire_flags();
    // The swim branch never lands: leaving the water resumes the ground mover from rest.
    let landed;
    if let Some((swim_fwd, swim_side)) = swim {
        // SWIMMING (its pitch tail rides with it) and the direction bits of the netted swim
        // amounts, which the swim gait selector cascades on (`0x5fd100`: turn 41, strafe 43/44,
        // back 45, forward 42, idle 41). Space sets nothing: its swim role is the breach
        // (`0x7c6230`). No FALLING, and the arc is cleared.
        move_flags_now |= move_flags::SWIMMING;
        if swim_fwd < 0.0 {
            move_flags_now |= move_flags::BACKWARD;
        } else if swim_fwd > 0.0 {
            move_flags_now |= move_flags::FORWARD;
        }
        if swim_side < 0.0 {
            move_flags_now |= move_flags::STRAFE_LEFT;
        } else if swim_side > 0.0 {
            move_flags_now |= move_flags::STRAFE_RIGHT;
        }
        player.airborne_since = None;
        player.fall_far = false;
        landed = false;
    } else {
        // Off the net axis: a pair that nets to zero streams no direction bit, the emitter's STOP.
        match axes.fwd.signum() {
            1 => move_flags_now |= move_flags::FORWARD,
            -1 => move_flags_now |= move_flags::BACKWARD,
            _ => {}
        }
        // Off the netted strafe axis: the server relays no packet carrying both strafe bits.
        match axes.side.signum() {
            -1 => move_flags_now |= move_flags::STRAFE_LEFT,
            1 => move_flags_now |= move_flags::STRAFE_RIGHT,
            _ => {}
        }
        if !axes.mouselook {
            if axes.turn_left {
                move_flags_now |= move_flags::TURN_LEFT;
            }
            if axes.turn_right {
                move_flags_now |= move_flags::TURN_RIGHT;
            }
        }
        // The arc's edges; FALLING rides the wire too, so observers replay the arc.
        let arc = player.advance_airborne_arc(airborne, jumped, now, launch_y);
        landed = arc.landed;
        if airborne {
            move_flags_now |= move_flags::FALLING;
            // In mid-air the wire's direction bits track the keys, as the reference's do (in a
            // 1.12.1 capture a strafe pressed mid-air lands as `(Forward, StrafeLeft)`); only the
            // velocity basis freezes (`0x7c5a20` skips it while FALLING), and observers pick the
            // landing anim off the touchdown flags (`0x602c60`). The pose keeps the takeoff
            // directions, re-seeded by a new arc and by the standstill air nudge.
            if arc.new_arc || air_nudged {
                player.airborne_dirs = move_flags_now & move_flags::ANY_MOVE;
            }
            // FALLINGFAR rides the live flags: the Fall(40) pose, the landing-anim gate, the wire.
            if player.fall_far {
                move_flags_now |= move_flags::FALLING_FAR;
            }
        }
        // The settle hold freezes the body with gravity off, so it reports no locomotion, on the
        // wire or in the pose.
        if held {
            move_flags_now = 0;
        }
    }
    // `MOVEFLAG_WALK_MODE` (`0x100`) is a mode, not a motion, so it goes on after the settle's
    // wipe and survives the root and stun suppressions: the reference's input allow-list
    // (`0x615c71`, table `0x618054`) blocks translation under a root and permits run/walk.
    if player.walking {
        move_flags_now |= move_flags::WALK_MODE;
    }
    // A knockback streams FORWARD all arc: the reference's apply `0x6179c0` clears bit 1
    // (`0x617a0f`) and sets bit 0 (`0x617a18 or edx,0x8001`) at launch, and the send mask
    // (`0x618909 and edx,0x75a07dff`) keeps it; the same bit makes the arc unsteerable.
    if player.knock_arc {
        move_flags_now = (move_flags_now & !move_flags::BACKWARD) | move_flags::FORWARD;
    }
    // The translate predicate down drops the direction bits and the turn predicate the turn bits,
    // whichever branch built the word; death drops both.
    move_flags_now = state::incapacitated_flags(move_flags_now, !may_translate, !may_turn);
    // ON_TRANSPORT, whose local-pose tail is built at the send, from the post-attach state so
    // the two agree on the frame we board or step off.
    if player.ride.is_some() && !held {
        move_flags_now |= move_flags::ON_TRANSPORT;
    }

    // In the air the pose keeps the takeoff directions: the reference's anim layer plays the
    // takeoff flags until FALLINGFAR latches (`0x602c40`) or the unit lands.
    let pose_flags = if airborne {
        (move_flags_now & !move_flags::ANY_MOVE) | player.airborne_dirs
    } else {
        move_flags_now
    };

    FrameFlags {
        wire: move_flags_now,
        pose: pose_flags,
        landed,
        fall_time: wire_fall_time,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::input::MoveAxes;

    fn still() -> MoveAxes {
        MoveAxes {
            fwd: 0,
            side: 0,
            mouselook: false,
            turning: false,
            translating: false,
            autorun_armed: false,
            strafe_left: false,
            strafe_right: false,
            turn_left: false,
            turn_right: false,
        }
    }

    #[test]
    fn the_walk_gait_outlives_the_settle_the_root_and_the_stun() {
        let case = |held: bool, rooted: bool, stunned: bool| {
            let mut player = Player {
                walking: true,
                ..Default::default()
            };
            player.modes.rooted = rooted;
            let mut axes = still();
            axes.fwd = 1;
            axes.translating = true;
            this_frame(
                &mut player,
                &axes,
                None,
                false,
                false,
                held,
                false,
                // Alive, so each predicate is its own term.
                !rooted,
                !stunned,
                1.0,
                0.0,
            )
        };
        for (name, held, rooted, stunned) in [
            ("moving", false, false, false),
            ("settling", true, false, false),
            ("rooted", false, true, false),
            ("stunned", false, false, true),
        ] {
            let f = case(held, rooted, stunned);
            assert_eq!(
                f.wire & move_flags::WALK_MODE,
                move_flags::WALK_MODE,
                "{name}: the gait rides the wire ({:#x})",
                f.wire
            );
            assert_eq!(
                f.pose & move_flags::WALK_MODE,
                move_flags::WALK_MODE,
                "{name}: …and the animation sees it too ({:#x})",
                f.pose
            );
        }
        // Control: the settle does wipe the direction bits.
        assert_eq!(
            case(true, false, false).wire & move_flags::ANY_MOVE,
            0,
            "the settle still clears the direction bits"
        );
        // A runner never carries the bit.
        let mut runner = Player::default();
        let mut axes = still();
        axes.fwd = 1;
        axes.translating = true;
        let f = this_frame(
            &mut runner,
            &axes,
            None,
            false,
            false,
            false,
            false,
            true,
            true,
            1.0,
            0.0,
        );
        assert_eq!(f.wire & move_flags::WALK_MODE, 0);
    }

    #[test]
    fn a_knockback_arc_plants_forward_on_the_wire() {
        let mut player = Player {
            knock_arc: true,
            ..Default::default()
        };
        // Airborne with no keys held: nothing else sets a direction bit.
        let f = this_frame(
            &mut player,
            &still(),
            None,
            true,
            false,
            false,
            false,
            true,
            true,
            1.0,
            0.0,
        );
        assert!(
            f.wire & move_flags::FORWARD != 0,
            "the launch's planted FORWARD rides the wire: {:#x}",
            f.wire
        );
        assert!(
            f.wire & move_flags::BACKWARD == 0,
            "and it clears BACKWARD, as the apply does: {:#x}",
            f.wire
        );
        assert!(f.wire & move_flags::FALLING != 0, "still an airborne arc");

        // Without the knockback, the same frame streams no direction.
        let mut plain = Player::default();
        let f = this_frame(
            &mut plain,
            &still(),
            None,
            true,
            false,
            false,
            false,
            true,
            true,
            1.0,
            0.0,
        );
        assert!(
            f.wire & move_flags::ANY_MOVE == 0,
            "an ordinary fall with no keys held streams no direction: {:#x}",
            f.wire
        );
    }
}
