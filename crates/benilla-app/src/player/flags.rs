//! **The move-flag word this frame carries** — the client's `CMovement+0x40`, rebuilt from state
//! every frame and then read by three different consumers, which is the whole reason it is built
//! in one place: the animation selector, the outbound `MSG_MOVE_*` stream, and the local gates
//! (the sit refusal, the cast self-cancel) must never disagree about what the avatar is doing
//! (decision 0056 — the wire mirrors the avatar's actual motion).
//!
//! Two words come out, and the difference between them is verified, not cosmetic: the **wire**
//! word keeps the direction bits live mid-air (the 1.12.1 sniff proves the real client's do), while
//! the **pose** word freezes them at take-off, because the anim layer plays the step-off gait off
//! the takeoff-frozen flags until FALLINGFAR latches or the unit lands.
//!
//! The airborne arc's own bookkeeping — the snapshot, the FALLINGFAR latch, the landing edge —
//! belongs to [`Player::advance_airborne_arc`]; this module only reads its verdict and the fall
//! clock that rides the landing packet.

use crate::creature_anim::move_flags;

use super::{input, state, Player};

/// The two flag words this frame, plus the two facts the wire lifecycle needs from the arc.
pub(super) struct FrameFlags {
    /// What goes on the wire (and into the local gates): direction bits stay live mid-air.
    pub wire: u32,
    /// What the animation sees: direction bits frozen at take-off.
    pub pose: u32,
    /// The airborne arc ended this frame — the `MSG_MOVE_FALL_LAND` edge.
    pub landed: bool,
    /// Milliseconds since the arc began, snapshotted *before* the landing clears it — vmangos
    /// gates fall damage on this field.
    pub fall_time: u32,
}

/// Build this frame's two flag words. `swim` is `Some((fwd, side))` exactly while swimming — the
/// netted swim amounts that actually drove the mover, so a rooted or key-cancelled swimmer cannot
/// stream a phantom direction.
pub(super) fn this_frame(
    player: &mut Player,
    axes: &input::MoveAxes,
    swim: Option<(f32, f32)>,
    airborne: bool,
    jumped: bool,
    held: bool,
    air_nudged: bool,
    // This frame's two movement-input predicates — what the reference's `0x514560` and `0x5145b0`
    // answered ([`state::may_translate`], [`state::may_turn`]). Both go down on death (1753).
    may_translate: bool,
    may_turn: bool,
    now: f32,
    launch_y: f32,
) -> FrameFlags {
    // The wire fall clock (ms since the airborne arc began), snapshotted HERE — before the arc
    // bookkeeping below clears `airborne_since` on the landing frame — so the MSG_MOVE_FALL_LAND
    // reports the *accumulated* fall time. vmangos `Player::HandleFall` gates fall damage on the
    // land packet's fallTime ≥ 1229 ms (the free-fall time of the 14.57-yd damage threshold); a
    // clock zeroed by the landing silently disables fall damage. The takeoff frame still sends 0
    // (`airborne_since` is not yet set at this point in that frame).
    let wire_fall_time = if jumped {
        // A jump launch starts a fresh arc — its fall clock is zero. This also covers a
        // same-frame land+relaunch, where `airborne_since` still holds the *previous* arc's
        // start; without this the bounce's JUMP would carry a stale (accumulated) fall time,
        // and a long spam-jump chain could spuriously cross the server's fall-damage gate.
        0
    } else {
        player
            .airborne_since
            .map_or(0, |t0| ((now - t0) * 1000.0).max(0.0) as u32)
    };
    // The CMovement move-flags this frame's input implies. The same bitset drives our avatar's
    // animation *and* the movement stream we send the server, so the two can
    // never disagree. Direction bits mirror the client's MOVEMENTFLAGS; FALLING marks the airborne
    // arc (animation-only — it is masked off before going on the wire, see the send block).
    // **Every granted mover mode rides every packet**, in or out of the water — the reference's
    // builder reads the one `[cmov+0x40]` the server's merge wrote them into, so it echoes back
    // whatever was granted for free (decisions 0726, 0866). Ours has to put them back
    // explicitly, because this word is rebuilt from state each frame; drop one and the server
    // forgets the mode, then the next server-authored move echoes a mode-less word back and
    // clears it under us. Root rides too — moving bits are what must not accompany it, and
    // rooted input can't produce any (the controller zeroes `dir`, jumps refused).
    let mut move_flags_now = player.modes.wire_flags();
    // `landed` gates the wire's jump/fall lifecycle; the swim branch never sets
    // them (leaving the water resumes the ground mover from rest, no airborne report).
    let landed;
    if let Some((swim_fwd, swim_side)) = swim {
        // Swimming: `MOVEFLAG_SWIMMING` (the swim-pitch tail rides with it) plus the travel-direction
        // bits the swim gait selector cascades on (`0x5fd100`: turn→41, strafe→43/44, back→45,
        // fwd→42, idle→41). The bits mirror the NET swim amounts that actually drive the mover —
        // one source, so a rooted or key-cancelled swimmer can't stream a phantom direction
        // (decision 0056). Space sets nothing here — its whole swim role is the jump-exit,
        // which runs the breach arm (`0x7c6230`). No FALLING, no airborne bookkeeping: the
        // arc state is cleared so leaving the water starts a clean walk/fall from rest.
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
        // Straight off the net axis, so a netted-to-zero press pair streams NO direction bit
        // (the emitter's genuine STOP) rather than a phantom FORWARD we aren't actually moving
        // in — decision 0056's law that the flags mirror the avatar's motion.
        match axes.fwd.signum() {
            1 => move_flags_now |= move_flags::FORWARD,
            -1 => move_flags_now |= move_flags::BACKWARD,
            _ => {}
        }
        // Straight off the netted strafe axis, so a cancelled press pair streams NO strafe bit —
        // the two are mutually exclusive on the wire, and both-set is silently dropped by the
        // server (decision 0622).
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
        // Airborne (a jump or a step-off a ledge) — the caller's hoisted value. The arc's
        // snapshot / far-latch / landing edges live in [`Player::advance_airborne_arc`] (a
        // fresh jump is always a NEW arc, even a same-frame land+relaunch — see there). FALLING
        // also rides the wire (decision 0053), so observers replay it.
        let arc = player.advance_airborne_arc(airborne, jumped, now, launch_y);
        landed = arc.landed;
        if airborne {
            move_flags_now |= move_flags::FALLING;
            // Mid-air the direction flags stay LIVE — the real client's `CMovement+0x40` keeps
            // tracking the keys while airborne, and the wire proves it (VERIFIED, vanilla-sniffs
            // `dwarf_rogue_dun_morogh`: a strafe pressed mid-air rides the landing FALL_LAND as
            // `(Forward, StrafeLeft)`; an S→W swap mid-air lands as `(Forward)`). What's frozen
            // at takeoff is the *velocity basis* (the mover's momentum — `0x7c5a20` skips the
            // basis recompute while FALLING), never the reported state; the landing-anim pick
            // (`jump_land_pick`, the ref's `0x602c60`) keys on the flags *at touchdown*, so a
            // frozen wire strands observers on stale flags and they play a locomotion anim
            // instead of the landing. The ANIM path keeps the takeoff-frozen dirs (`pose_flags`
            // below — the RE'd step-off gait freeze); a new arc (re)seeds them, and the
            // standstill air nudge is the one mid-arc input that really moves us.
            if arc.new_arc || air_nudged {
                player.airborne_dirs = move_flags_now & move_flags::ANY_MOVE;
            }
            // FALLINGFAR (latched by `advance_airborne_arc` above — the exclusive distance/timer
            // legs, decision 0179) rides the live flags: the mid-air Fall(40) pose, the
            // landing-anim gate, and the wire (heartbeats carry it; the axis differ ignores it).
            if player.fall_far {
                move_flags_now |= move_flags::FALLING_FAR;
            }
        }
        // While `held` (post-teleport/login settle) the avatar is frozen in place with gravity off,
        // so it has no locomotion to report — clear the flags so we never stream a phantom walk/turn
        // the server would extrapolate onto observers while we sit on the settle. The frozen position
        // was already reported by the teleport Stop; a facing change still streams a harmless
        // SET_FACING. The same bitset drives the local animation (0052), so this also keeps the
        // held avatar idle rather than moonwalking in place. (Decision 0056 — the wire mirrors the
        // avatar's actual motion.)
        if held {
            move_flags_now = 0;
        }
    }
    // **The walk gait rides every packet** — `MOVEFLAG_WALK_MODE` `0x100`, latched by the
    // `TOGGLERUN` keybind ([`super::walk`], decision 1752). Deliberately OUTSIDE the branches
    // above, and after the settle's `move_flags_now = 0`: this is a MODE, not a motion. The
    // settle's wipe exists so a frozen avatar reports no locomotion, and clearing the walk bit
    // with it would tell the server we went back to running — a spurious SET_RUN_MODE, then a
    // SET_WALK_MODE the frame the settle releases, on every teleport. Nothing below strips it
    // either: `incapacitated_flags` touches only the direction and turn bits, which is the
    // reference's own answer too — its input allow-list (`0x615c71` → the table at `0x618054`)
    // blocks the translation commands while rooted and **explicitly permits run/walk**.
    if player.walking {
        move_flags_now |= move_flags::WALK_MODE;
    }
    // **A knockback arc streams FORWARD for its whole length** (decision 1740). The reference's
    // apply plants it as part of the launch (`0x6179c0`'s `0x617a18 or edx,0x8001` — set bit 0,
    // clear bit 1) and nothing clears it until the arc ends, and the send mask
    // (`0x618909 and edx,0x75a07dff`) keeps bit 0, so observers see it. It is also the mechanism
    // that makes the arc unsteerable, so the bit and the freeze are the same fact, set in one
    // place. Ours never sent it at all: a knocked-back body streamed a bare FALLING and observers
    // replayed it as a plain drop.
    if player.knock_arc {
        move_flags_now = (move_flags_now & !move_flags::BACKWARD) | move_flags::FORWARD;
    }
    // The two incapacitate suppressions — the translate predicate down drops the direction bits,
    // the turn predicate down drops the turn bits — applied to the whole word in one place,
    // whichever branch built it, and with the reference's byte trail in
    // [`state::incapacitated_flags`] (decision 0880). Death drops both, through the precondition
    // the two predicates share (decision 1753), so a corpse streams a bare word.
    move_flags_now = state::incapacitated_flags(move_flags_now, !may_translate, !may_turn);
    // Riding a transport: the ON_TRANSPORT bit rides every packet with its local-pose tail
    // (built at the send). Set from the POST-attach state so flag and tail agree the
    // very frame we board or step off (decision 0438 phase 2).
    if player.ride.is_some() && !held {
        move_flags_now |= move_flags::ON_TRANSPORT;
    }

    // The animation/body-pose view of the flags: airborne it keeps the TAKEOFF-FROZEN direction
    // bits — the reference's anim layer plays the step-off gait off the takeoff-frozen
    // flags/speed until FALLINGFAR latches (`0x602c40`) or the unit lands,
    // and a mid-air Q press must not twist the body or animate a strafe. The *wire* flags above
    // stay live (the sniff-verified send law); only the pose reads the freeze.
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

    /// **Walk mode is a MODE, so it survives everything that clears locomotion** (decision 1752).
    /// Three states that each wipe or narrow the word, and the bit must be in both the wire and
    /// the pose word out of every one of them:
    ///
    /// * the post-teleport/login **settle** (`held`), whose `move_flags_now = 0` exists so a
    ///   frozen avatar reports no walking — clearing the gait with it would tell the server we
    ///   went back to running, then re-announce a frame later: a SET_RUN_MODE/SET_WALK_MODE pair
    ///   on every single teleport;
    /// * **rooted**, which drops the direction bits — and the reference agrees the gait stays: its
    ///   input allow-list (`0x615c71` → `0x618054`) blocks the translation commands while rooted
    ///   and **explicitly permits** run/walk;
    /// * **stunned**, which drops the turn bits.
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
                // The two predicates, as this case's root and stun leave them — the body is alive,
                // so each is simply its own term (decision 1753).
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
        // The settle really is wiping the locomotion — the control that says the assertion above
        // is about a mode surviving a wipe, not about there being no wipe.
        assert_eq!(
            case(true, false, false).wire & move_flags::ANY_MOVE,
            0,
            "the settle still clears the direction bits"
        );
        // …and a runner never carries the bit.
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

    /// **A knockback arc streams FORWARD for its whole length** (decision 1740). The reference's
    /// apply plants the bit as part of the launch (`0x6179c0`'s `0x617a18 or edx,0x8001` — set bit
    /// 0, clear bit 1) and only the arc's end clears it; the send mask
    /// (`0x618909 and edx,0x75a07dff`) keeps bit 0, so observers get it on every packet of the
    /// flight. We sent a bare FALLING instead, and observers replayed a knockback as a plain drop.
    ///
    /// It is the same bit that makes the arc unsteerable, which is why the two are one fact set in
    /// one place — see [`super::mover`]'s air-control gate.
    #[test]
    fn a_knockback_arc_plants_forward_on_the_wire() {
        let mut player = Player {
            knock_arc: true,
            ..Default::default()
        };
        // Airborne, no keys held at all — nothing else could produce a direction bit.
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

        // The same frame without the knockback provenance streams no direction at all.
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
