//! Our own movement onto the wire: a `MSG_MOVE_*` per flag transition, the jump and the landing,
//! a heartbeat while moving and a `SET_FACING` each frame the facing changes. The wire must mirror
//! the avatar's actual motion: vmangos relays each packet verbatim and observers extrapolate from
//! its flags, so a flag left set is a phantom walk on every other screen.

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::{JumpInfo, TransportPose};
use crossbeam_channel::Sender;

use crate::creature_anim::move_flags;
use crate::net::{ClientCommand, MoveKind};

use super::Player;

/// The `MSG_MOVE_HEARTBEAT` period (s) while moving: the reference arms its send deadline
/// `[mgr+0x130]` to now + 500 ms (`0x615b80`).
const HEARTBEAT_INTERVAL: f32 = 0.5;
/// The flag bits we send, each with its tail where it has one (`FALLING` the jump quad, `SWIMMING`
/// the pitch, `ON_TRANSPORT` the deck pose). `FALLING_FAR` changes no opcode; it rides the arc's
/// packets as the reference's live flags do.
const OUTBOUND_FLAG_MASK: u32 = move_flags::FORWARD
    | move_flags::BACKWARD
    | move_flags::STRAFE_LEFT
    | move_flags::STRAFE_RIGHT
    | move_flags::TURN_LEFT
    | move_flags::TURN_RIGHT
    | move_flags::WALK_MODE
    | move_flags::FALLING
    | move_flags::FALLING_FAR
    | move_flags::SWIMMING
    // The granted modes echo back: the reference builds its packets from the flags word the
    // server's merges land in. vmangos stores what we report, so a dropped `LEVITATING` returns
    // cleared on the next server-authored move and ends GM flight.
    | move_flags::LEVITATING
    // vmangos re-adds `ROOT` to a rooted mover's report (`MovementHandler.cpp:1064-1065`) and kicks
    // a root-apply ack without it (`MovementHandler.cpp:715-722`).
    | move_flags::ROOT
    | move_flags::WATER_WALKING
    | move_flags::SAFE_FALL
    | move_flags::HOVER
    | move_flags::ON_TRANSPORT;

/// The bits under which the position changes every frame and the move stream carries it; the
/// position reconcile runs only with none set. Turning is absent: a turn in place moves nothing.
const IN_MOTION: u32 = move_flags::ANY_MOVE
    | move_flags::FALLING
    | move_flags::FALLING_FAR
    | move_flags::SWIMMING
    | move_flags::ON_TRANSPORT;

/// This frame's input to the `CMSG_MOVE_TIME_SKIPPED` report ([`stream_skipped_time`]).
pub(super) struct SkipClock {
    /// The frame's length in seconds.
    pub(super) dt: f32,
    /// The mover held instead of stepping ([`super::Player::settling`]): nothing integrated `dt`.
    pub(super) held: bool,
    /// The active mover, a possessed unit or our own body; `None` until the server names one.
    pub(super) mover: Option<u64>,
}

/// This frame's airborne edges, as the send rules read them.
pub(super) struct ArcEdges {
    /// A take-off launched this frame.
    pub(super) jumped: bool,
    /// The take-off came from the server, a knockback or the jump a hover grant owes
    /// ([`super::Player::hover_launch`]), so it sends no `MSG_MOVE_JUMP`: that packet's one
    /// emission site is the move-command drain's jump arm (`0x615ed1`); the hover handler
    /// `0x61a620` sends nothing and the knockback arm sends its ack (`0xf0`).
    pub(super) wire_launch: bool,
    /// The standstill air nudge fired ([`super::mover::step`]): the one mid-air press that moves
    /// us, so the one that breaks the airborne silence.
    pub(super) air_nudged: bool,
    /// The arc ended this frame: `MSG_MOVE_FALL_LAND`.
    pub(super) landed: bool,
    /// Ms since take-off, the caller's snapshot: the landing frame has already cleared
    /// `airborne_since`, and vmangos deals fall damage only from a landing `fallTime` of 1229 ms
    /// (`Player.cpp:20954`).
    pub(super) fall_time: u32,
}

/// Streams this frame's movement as the reference does: a `MSG_MOVE_*` per axis transition (its
/// broadcaster `0x61a820` picks the opcode from the flag delta, `0x619f00`), a `SET_FACING` each
/// frame the facing changes off the turn axis, and a heartbeat every 500 ms while moving. Airborne,
/// the forward, back and strafe transitions go silent and their live bits ride the packets that do
/// go out. Sends are fire-and-forget; updates `player`'s last-sent flags, facing and heartbeat.
pub(super) fn stream_self_movement(
    sender: &Sender<ClientCommand>,
    player: &mut Player,
    move_flags_now: u32,
    swim_pitch: f32,
    arc: ArcEdges,
    now: f32,
    speed_acks: &[crate::net::SpeedChangeMessage],
    knock_ack: Option<super::state::PendingKnockback>,
    transport: Option<TransportPose>,
    skip: SkipClock,
) {
    let ArcEdges {
        jumped,
        wire_launch,
        air_nudged,
        landed,
        fall_time,
    } = arc;
    stream_skipped_time(sender, player, skip);
    let wow_pos = bevy_to_wow(player.pos);
    // `face_yaw` is unbounded; the reference sends [0, 2π), and vmangos drops a packet with
    // `|o| > 4π` unrelayed (`VerifyMovementInfo`, `GridDefines.h:203`), a stop included.
    let facing = player.face_yaw.rem_euclid(std::f32::consts::TAU);
    let wire_flags = move_flags_now & OUTBOUND_FLAG_MASK;
    // The jump tail, on every airborne packet: the take-off vertical speed and the horizontal
    // velocity frozen at take-off, in world XY.
    let wire_jump = (wire_flags & move_flags::FALLING != 0).then(|| {
        let v = bevy_to_wow(player.horiz_vel); // WoW velocity [vx, vy, 0] (the transform is linear)
        let xy = v[0].hypot(v[1]);
        let (cos_angle, sin_angle) = if xy > 1.0e-4 {
            (v[0] / xy, v[1] / xy)
        } else {
            (facing.cos(), facing.sin()) // a standing jump has no direction: use the facing
        };
        JumpInfo {
            // Down-positive on the wire: the 1.12 client sends -7.955547 (`0xc0fe93d8`) rising.
            zspeed: -player.jump_zspeed,
            cos_angle,
            sin_angle,
            xy_speed: xy,
        }
    });
    // A forced speed change is acked within the server's 4 s window with this frame's live
    // payload, which vmangos relocates us to and runs its anticheat position tests on.
    for ack in speed_acks {
        let _ = sender.send(ClientCommand::ForceSpeedAck {
            kind: ack.kind,
            guid: ack.guid,
            counter: ack.counter,
            speed: ack.speed,
            flags: wire_flags,
            pos: wow_pos,
            orientation: facing,
            pitch: swim_pitch,
            fall_time,
            jump: wire_jump,
            transport,
        });
        // The ack relocates us server-side, so it is a position report too.
        player.last_pos = wow_pos;
    }

    // The knockback ack goes out on the take-off frame with the take-off pose, as the reference
    // applies the launch and then sends (`0x61624d` call `0x6179c0`, then `0x616261` push `0xf0`).
    // `launch` is echoed bit for bit, as the reference copies its live fields: vmangos matches the
    // four floats within 0.01 (`Unit.cpp:7097-7100`), and a re-derived quad has no direction at
    // `xy_speed == 0`.
    if let Some(k) = knock_ack {
        let _ = sender.send(ClientCommand::KnockBackAck {
            guid: k.guid,
            counter: k.counter,
            launch: k.launch,
            flags: wire_flags,
            pos: wow_pos,
            orientation: facing,
            transport,
        });
        // A position report too, like a speed ack.
        player.last_pos = wow_pos;
    }
    let prev = player.move_flags;
    let added = wire_flags & !prev;
    let removed = prev & !wire_flags;
    let mut sent = false;
    macro_rules! send_move {
        ($kind:expr) => {{
            if *crate::net::CAST_TRACE {
                bevy::log::info!(
                    "cast-trace: SEND move {:?} flags={:#x} pos=[{:.3},{:.3},{:.3}] o={:.3}",
                    $kind,
                    wire_flags,
                    wow_pos[0],
                    wow_pos[1],
                    wow_pos[2],
                    facing
                );
            }
            super::move_trace::sent($kind, wire_flags, facing, wow_pos);
            let _ = sender.send(ClientCommand::Move {
                kind: $kind,
                flags: wire_flags,
                pos: wow_pos,
                orientation: facing,
                // Written only while SWIMMING is set.
                pitch: swim_pitch,
                fall_time,
                jump: wire_jump,
                // Written only with ON_TRANSPORT: the rider's deck-local pose.
                transport,
            });
            sent = true;
            // vmangos relocates the mover to every one of these (`HandleMoverRelocation`).
            player.last_pos = wow_pos;
        }};
    }
    const FB: u32 = move_flags::FORWARD | move_flags::BACKWARD;
    const STRAFE: u32 = move_flags::STRAFE_LEFT | move_flags::STRAFE_RIGHT;
    const TURN: u32 = move_flags::TURN_LEFT | move_flags::TURN_RIGHT;
    // Airborne, the forward, back and strafe transitions are silent: while FALLING the reference's
    // `StartMove` (`0x7c6ae0`) defers a press into an inert latch (`0x20000`/`0x40000`), so the
    // broadcaster `0x61a820` sees no flag delta. It defers nothing with nothing moving, so the
    // standstill air nudge does send, carrying the re-seeded jump tail.
    let falling = wire_flags & move_flags::FALLING != 0;
    // The arc is JUMP at a keyboard take-off and FALL_LAND at the end, with no trailing Stop, as a
    // mid-air release only updated the flags. A jumpless fall opens with nothing: the broadcaster
    // sends only on a locomotion-nibble change (`0x61a99d` test al,0xf), and vmangos would read
    // the JUMPING bit as movement and stand a seated player up (`Unit.cpp:10398-10399`). A
    // server-driven take-off sends no JUMP: vmangos flags a JUMP faster than run speed as a cheat,
    // knockbacks included (`MovementAnticheat.cpp:650`).
    if jumped && !wire_launch {
        send_move!(MoveKind::Jump);
    } else if landed {
        send_move!(MoveKind::FallLand);
    }
    // `START_SWIM` (0xca) and `STOP_SWIM` (0xcb) go out the frame `SWIMMING` flips, enqueued by
    // the swim decision `0x6030c0`; airborne and swimming never overlap, so the arc cannot race.
    if added & move_flags::SWIMMING != 0 {
        send_move!(MoveKind::StartSwim);
    } else if removed & move_flags::SWIMMING != 0 {
        send_move!(MoveKind::StopSwim);
    }
    // `SET_WALK_MODE` (0xc3) and `SET_RUN_MODE` (0xc2) go out the frame the bit flips, airborne or
    // standing still: the reference's `ToggleRun` enqueues its own event, since the broadcaster
    // gates on the locomotion nibble (`0x61a99d` test al,0xf) and would drop it.
    if added & move_flags::WALK_MODE != 0 {
        send_move!(MoveKind::SetWalkMode);
    } else if removed & move_flags::WALK_MODE != 0 {
        send_move!(MoveKind::SetRunMode);
    }
    // The stop arms stay silent while falling even on the nudge frame: a mid-air release is always
    // deferred, and this keeps one on the other axis from slipping out with the nudge.
    if !falling || air_nudged {
        if added & move_flags::FORWARD != 0 {
            send_move!(MoveKind::StartForward);
        } else if added & move_flags::BACKWARD != 0 {
            send_move!(MoveKind::StartBackward);
        } else if !falling && removed & FB != 0 && wire_flags & FB == 0 {
            send_move!(MoveKind::Stop);
        }
        if added & move_flags::STRAFE_LEFT != 0 {
            send_move!(MoveKind::StartStrafeLeft);
        } else if added & move_flags::STRAFE_RIGHT != 0 {
            send_move!(MoveKind::StartStrafeRight);
        } else if !falling && removed & STRAFE != 0 && wire_flags & STRAFE == 0 {
            send_move!(MoveKind::StopStrafe);
        }
    }
    // The turn axis sends airborne too: turning works mid-air.
    if added & move_flags::TURN_LEFT != 0 {
        send_move!(MoveKind::StartTurnLeft);
    } else if added & move_flags::TURN_RIGHT != 0 {
        send_move!(MoveKind::StartTurnRight);
    } else if removed & TURN != 0 && wire_flags & TURN == 0 {
        send_move!(MoveKind::StopTurn);
    }
    // One `SET_FACING` per frame the facing changed, moving or not, with no rate limit: the
    // reference drains each facing set into a send (`0x615fc6` push `0xda`). None while a turn bit
    // is set, which already rotates the mover for observers, and not gated on `sent`: the facing
    // report and the broadcaster are independent emitters.
    if wire_flags & TURN == 0 && facing != player.last_facing {
        send_move!(MoveKind::SetFacing);
    }
    // Boarding or leaving a deck has no opcode of its own, so a heartbeat carries the flip that
    // frame if nothing else went out.
    if !sent && (added | removed) & move_flags::ON_TRANSPORT != 0 {
        send_move!(MoveKind::Heartbeat);
    }
    // The heartbeat runs while any sent flag is set; the reference's (`0x616620`) runs only while
    // `flags & 0x200f`, the directions and FALLING (`0x616725`). Every send re-arms its deadline
    // (`0x600a30` → `0x615b80`), hence the stamp on any send below.
    if !sent && wire_flags != 0 && now - player.last_heartbeat >= HEARTBEAT_INTERVAL {
        send_move!(MoveKind::Heartbeat);
    }
    // At rest, a position change goes out the frame it happens: the resolver settles a resting
    // body by fractions of a millimetre after the packet that reported it, and vmangos compares
    // positions exactly (`Player.cpp:6092`), so a stale copy arrives with the next packet as
    // movement and interrupts the cast in flight (`Unit.cpp:10387`).
    if !sent && wire_flags & IN_MOTION == 0 && wow_pos != player.last_pos {
        send_move!(MoveKind::Heartbeat);
    }
    if sent {
        player.last_heartbeat = now;
    }
    // The facing compare is against the previous frame's facing, sent or not, so a keyboard turn
    // leaves no catch-up `SET_FACING` behind.
    player.last_facing = facing;
    player.move_flags = wire_flags;
}

/// `CMSG_MOVE_TIME_SKIPPED`: the milliseconds the mover advanced through without integrating. The
/// reference's common senders are its no-geometry pair (`0x634154`, `0x635742`), time with no
/// world to resolve against, which is our settle hold; its long-frame site (`0x616642`) has no
/// counterpart, as we integrate a long frame whole. Deviation: a hold is summed and sent once on
/// release, without the send's `[mgr+0x130] += lag` heartbeat push, because ours lasts seconds
/// where the reference's lasts a substep.
fn stream_skipped_time(sender: &Sender<ClientCommand>, player: &mut Player, skip: SkipClock) {
    if skip.held {
        player.skipped_ms += skip.dt * 1000.0;
        return;
    }
    // The release edge sends the whole hold in whole ms; the residue is dropped.
    let lag_ms = player.skipped_ms as u32;
    player.skipped_ms = 0.0;
    if lag_ms == 0 {
        return;
    }
    let Some(guid) = skip.mover else {
        return;
    };
    // The reference's builder `0x600be0` sends only for the active mover (`0x600bec`/`0x600bfb`
    // against `[0xc4da98]`/`[0xc4da9c]`); we name only the one we drive.
    super::move_trace::skipped_time(guid, lag_ms);
    let _ = sender.send(ClientCommand::MoveTimeSkipped { guid, lag_ms });
}

/// Sends one flag-less `MSG_MOVE_STOP` when the controller stops driving the avatar (free-fly),
/// as nothing else would clear the flags observers extrapolate from; a no-op once stopped.
pub(super) fn park_mover(sender: &Sender<ClientCommand>, player: &mut Player) {
    if player.move_flags == 0 {
        return;
    }
    let facing = player.face_yaw.rem_euclid(std::f32::consts::TAU);
    let pos = bevy_to_wow(player.pos);
    player.last_pos = pos; // the park is a position report too
    super::move_trace::sent(MoveKind::Stop, 0, facing, pos);
    let _ = sender.send(ClientCommand::Move {
        kind: MoveKind::Stop,
        flags: 0,
        pos,
        orientation: facing,
        pitch: 0.0, // flags cleared → not swimming → no pitch tail written
        fall_time: 0,
        jump: None,
        transport: None, // flags cleared → no transport tail written
    });
    player.move_flags = 0;
    player.last_facing = facing;
}

/// The rider's deck-local pose for the `ON_TRANSPORT` tail, or `None` off a deck. `bevy_to_wow` is
/// a pure rotation, so it converts the local offset too; the local facing is `face_yaw - boat_yaw`.
pub(super) fn wire_transport(player: &Player) -> Option<TransportPose> {
    player.ride.as_ref().map(|r| {
        let local = bevy_to_wow(r.local_pos);
        TransportPose {
            guid: r.guid,
            pos: benilla_protocol::wire::Vector3d {
                x: local[0],
                y: local[1],
                z: local[2],
            },
            orientation: (player.face_yaw - r.boat_yaw).rem_euclid(std::f32::consts::TAU),
        }
    })
}

/// Acks forced speed changes on a frame the controller does not drive (a server spline, a fear, a
/// mover hand-off), as the reference's per-mover drain acks whoever drives (`0x616142`,
/// `0x61812d`). Unacked, vmangos enforces the change after 4 s, counts the miss for anticheat and
/// holds the graveyard repop (`Player.cpp:1332`). The payload is our last reported flags minus the
/// airborne pair, as no arc runs on these frames.
pub(super) fn ack_speeds_undriven(
    sender: &Sender<ClientCommand>,
    player: &Player,
    acks: &[crate::net::SpeedChangeMessage],
) {
    let transport = wire_transport(player);
    let mut flags =
        player.move_flags & OUTBOUND_FLAG_MASK & !(move_flags::FALLING | move_flags::FALLING_FAR);
    if transport.is_none() {
        flags &= !move_flags::ON_TRANSPORT; // flag and tail travel together
    }
    for ack in acks {
        let _ = sender.send(ClientCommand::ForceSpeedAck {
            kind: ack.kind,
            guid: ack.guid,
            counter: ack.counter,
            speed: ack.speed,
            flags,
            pos: bevy_to_wow(player.pos),
            orientation: player.face_yaw.rem_euclid(std::f32::consts::TAU),
            pitch: 0.0,
            fall_time: 0,
            jump: None,
            transport: transport.filter(|_| flags & move_flags::ON_TRANSPORT != 0),
        });
    }
}

#[cfg(test)]
mod tests {
    fn no_skip() -> SkipClock {
        SkipClock {
            dt: 0.0,
            held: false,
            mover: None,
        }
    }

    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn a_settle_hold_reports_its_skipped_milliseconds_once_on_release() {
        const MOVER: u64 = 0x0000_0000_0000_002a;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player::default();
        let held = |dt: f32| SkipClock {
            dt,
            held: true,
            mover: Some(MOVER),
        };
        let released = || SkipClock {
            dt: 0.016,
            held: false,
            mover: Some(MOVER),
        };
        // Four held frames, 0.5 s in all.
        for _ in 0..3 {
            stream_skipped_time(&tx, &mut player, held(0.1));
        }
        stream_skipped_time(&tx, &mut player, held(0.2));
        assert!(
            rx.try_recv().is_err(),
            "a hold in progress sends nothing — the reference's own captures show one packet per \
             skip, not one per frame"
        );
        stream_skipped_time(&tx, &mut player, released());
        match rx.try_recv().expect("the release edge reports") {
            ClientCommand::MoveTimeSkipped { guid, lag_ms } => {
                assert_eq!(guid, MOVER, "the packet names the mover we were driving");
                assert_eq!(lag_ms, 500, "0.1+0.1+0.1+0.2 s of un-integrated simulation");
            }
            other => panic!("expected MoveTimeSkipped, got {other:?}"),
        }
        stream_skipped_time(&tx, &mut player, released());
        assert!(
            rx.try_recv().is_err(),
            "the report is an edge; a running mover skips nothing and says nothing"
        );
    }

    #[test]
    fn the_walk_toggle_sends_its_own_opcode_with_no_movement_at_all() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player::default();
        let arc = || ArcEdges {
            jumped: false,
            wire_launch: false,
            air_nudged: false,
            landed: false,
            fall_time: 0,
        };
        // Entering walk: MSG_MOVE_SET_WALK_MODE (0xc3).
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::WALK_MODE,
            0.0,
            arc(),
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, flags, .. } =
            rx.try_recv().expect("a standing walk toggle still sends")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::SetWalkMode);
        assert_eq!(kind.opcode(), 0x00C3);
        assert_eq!(flags & move_flags::WALK_MODE, move_flags::WALK_MODE);

        // Leaving it: MSG_MOVE_SET_RUN_MODE (0xc2).
        stream_self_movement(
            &tx,
            &mut player,
            0,
            0.0,
            arc(),
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, flags, .. } =
            rx.try_recv().expect("the run half sends too")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::SetRunMode);
        assert_eq!(kind.opcode(), 0x00C2);
        assert_eq!(flags & move_flags::WALK_MODE, 0);

        stream_self_movement(
            &tx,
            &mut player,
            move_flags::WALK_MODE,
            0.0,
            arc(),
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
        while let Ok(ClientCommand::Move { kind, .. }) = rx.try_recv() {
            assert_eq!(kind, MoveKind::SetWalkMode, "the re-entry edge only");
        }
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::WALK_MODE,
            0.1,
            arc(),
            0.1,
            &[],
            None,
            None,
            no_skip(),
        );
        assert!(
            rx.try_recv().is_err(),
            "a walk already announced is silent until it flips back"
        );
    }

    #[test]
    fn a_walk_toggle_taken_mid_air_is_not_swallowed_by_the_airborne_silence() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD | move_flags::FALLING,
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD | move_flags::FALLING | move_flags::WALK_MODE,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 400,
            },
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
        let kinds: Vec<_> = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::Move { kind, .. } => kind,
                _ => panic!("expected Move commands"),
            })
            .collect();
        assert!(
            kinds.contains(&MoveKind::SetWalkMode),
            "the walk edge rides out mid-arc: {kinds:?}"
        );
    }

    #[test]
    fn wire_orientation_is_normalized_into_0_2pi() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            face_yaw: 100.0, // ~15.9 turns, far past vmangos's 4π bound
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );

        let ClientCommand::Move { orientation, .. } = rx
            .try_recv()
            .expect("a StartForward is sent on first FORWARD")
        else {
            panic!("expected a Move command");
        };
        assert!(
            (0.0..TAU).contains(&orientation),
            "orientation must be normalized into [0, 2π), got {orientation}"
        );
        assert!(
            (orientation - 100.0_f32.rem_euclid(TAU)).abs() < 1e-4,
            "the wrap preserves the angle (100 mod 2π): got {orientation}"
        );
    }

    #[test]
    fn fall_land_reports_the_accumulated_fall_time() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player::default(); // airborne_since already cleared, as on a landing frame
        stream_self_movement(
            &tx,
            &mut player,
            0,
            // grounded: no FALLING on the land packet itself
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: true,
                fall_time: 1700,
            },
            // 1.7 s of fall, past vmangos's 1229 ms fall-damage gate
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );

        let ClientCommand::Move {
            kind, fall_time, ..
        } = rx.try_recv().expect("a FALL_LAND is sent on landing")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::FallLand);
        assert_eq!(
            fall_time, 1700,
            "the FALL_LAND carries the accumulated fall time, not a cleared clock"
        );
    }

    #[test]
    fn airborne_direction_release_is_silent_and_the_landing_sends_only_fall_land() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD | move_flags::FALLING, // last sent: the JUMP's flags
            ..Default::default()
        };
        // Mid-air, W released.
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 300,
            },
            0.2,
            &[],
            None,
            None,
            no_skip(),
        );
        assert!(rx.try_recv().is_err(), "a mid-air release sends nothing");
        assert_eq!(
            player.move_flags,
            move_flags::FALLING,
            "the flag state still updated silently"
        );
        // The landing frame, no keys held.
        stream_self_movement(
            &tx,
            &mut player,
            0,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: true,
                fall_time: 800,
            },
            0.8,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, flags, .. } = rx.try_recv().expect("the landing packet")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::FallLand);
        assert_eq!(flags, 0, "the FALL_LAND carries the live (released) flags");
        assert!(
            rx.try_recv().is_err(),
            "no trailing Stop after the FALL_LAND"
        );
    }

    #[test]
    fn a_jumpless_fall_opens_with_nothing_and_heartbeats_on_the_deadline() {
        let (tx, rx) = crossbeam_channel::unbounded();
        // Grounded last frame, airborne now with no jump: a walk-off.
        let mut player = Player::default();
        let step_off = |now: f32, player: &mut Player| {
            stream_self_movement(
                &tx,
                player,
                move_flags::FALLING,
                0.0,
                ArcEdges {
                    jumped: false,
                    wire_launch: false,
                    air_nudged: false,
                    landed: false,
                    fall_time: (now * 1000.0) as u32,
                },
                now,
                &[],
                None,
                None,
                no_skip(),
            );
        };

        step_off(0.1, &mut player);
        assert!(
            rx.try_recv().is_err(),
            "the arc's first frame puts NOTHING on the wire — this packet is B79's un-seat"
        );
        assert_eq!(
            player.move_flags,
            move_flags::FALLING,
            "the flag state still updated silently, to ride the next packet that does go out"
        );

        step_off(0.4, &mut player);
        assert!(rx.try_recv().is_err(), "inside the deadline, still silent");

        step_off(0.7, &mut player);
        let ClientCommand::Move { kind, flags, .. } = rx
            .try_recv()
            .expect("past the deadline the fall heartbeats")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::Heartbeat);
        assert_ne!(
            flags & move_flags::FALLING,
            0,
            "and it carries the arc, so observers learn of a long fall in progress"
        );
    }

    #[test]
    fn the_standstill_air_nudge_is_the_one_press_that_breaks_the_airborne_silence() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FALLING, // the standing jump: airborne, no direction
            // Post-nudge: the mover re-seeded it this frame (Bevy −Z = WoW +X, so |xy| = 2.5).
            horiz_vel: bevy::prelude::Vec3::new(0.0, 0.0, -2.5),
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD | move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: true,
                landed: false,
                fall_time: 300,
            },
            0.3,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move {
            kind, flags, jump, ..
        } = rx.try_recv().expect("the nudge broadcasts its transition")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::StartForward);
        assert_ne!(flags & move_flags::FALLING, 0, "it rides the arc");
        let tail = jump.expect("an airborne packet carries the ballistic tail");
        assert!(
            (tail.xy_speed - 2.5).abs() < 1.0e-3,
            "the tail carries the RE-SEEDED horizontal — the whole point: an observer whose \
             arc says xy_speed = 0 learns the mover started moving, got {}",
            tail.xy_speed
        );
        assert!(rx.try_recv().is_err(), "one packet, not a burst");

        // A press mid-arc with momentum is deferred, so silent.
        let mut moving = Player {
            move_flags: move_flags::FORWARD | move_flags::FALLING,
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut moving,
            move_flags::FORWARD | move_flags::STRAFE_LEFT | move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 300,
            },
            0.3,
            &[],
            None,
            None,
            no_skip(),
        );
        assert!(
            rx.try_recv().is_err(),
            "a deferred mid-air press sends nothing"
        );
    }

    #[test]
    fn airborne_turn_transitions_and_facing_still_stream() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD | move_flags::FALLING,
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD | move_flags::TURN_LEFT | move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 200,
            },
            0.2,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, flags, .. } =
            rx.try_recv().expect("a mid-air turn broadcasts")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::StartTurnLeft);
        assert_ne!(flags & move_flags::FALLING, 0, "the packet rides the arc");
        assert!(rx.try_recv().is_err(), "the turn axis carries the facing");
        // A mouse turn as the turn key releases, past the heartbeat deadline: the heartbeat is
        // `!sent`-gated, so only the StopTurn and the SET_FACING go out.
        player.face_yaw = 1.0;
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD | move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 900,
            },
            1.5,
            &[],
            None,
            None,
            no_skip(),
        );
        let kinds: Vec<_> = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::Move { kind, .. } => kind,
                _ => panic!("expected Move commands"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![MoveKind::StopTurn, MoveKind::SetFacing],
            "the turn closes, then the facing reports — the frame already sent, so no heartbeat"
        );
    }

    #[test]
    fn facing_streams_while_running() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD, // already running: no transition this frame
            ..Default::default()
        };
        player.face_yaw = 0.7;
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.05,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, flags, .. } = rx
            .try_recv()
            .expect("a mouse-turn while running reports its facing")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::SetFacing);
        assert_eq!(
            flags,
            move_flags::FORWARD,
            "the facing report carries the live direction flags"
        );
        assert!(
            rx.try_recv().is_err(),
            "exactly one packet per changed frame"
        );
        // The same facing, inside the heartbeat deadline.
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.06,
            &[],
            None,
            None,
            no_skip(),
        );
        assert!(rx.try_recv().is_err(), "an unchanged facing is silent");
    }

    #[test]
    fn the_turn_axis_carries_its_own_facing() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD | move_flags::TURN_RIGHT,
            ..Default::default()
        };
        for (i, yaw) in [0.3_f32, 0.6, 0.9].into_iter().enumerate() {
            player.face_yaw = yaw;
            stream_self_movement(
                &tx,
                &mut player,
                move_flags::FORWARD | move_flags::TURN_RIGHT,
                0.0,
                ArcEdges {
                    jumped: false,
                    wire_launch: false,
                    air_nudged: false,
                    landed: false,
                    fall_time: 0,
                },
                0.05 * (i as f32 + 1.0),
                &[],
                None,
                None,
                no_skip(),
            );
            assert!(
                rx.try_recv().is_err(),
                "the turn axis is silent (frame {i})"
            );
        }
        // Release the turn key, the facing unchanged since the last frame.
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.2,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, .. } = rx.try_recv().expect("the STOP_TURN") else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::StopTurn);
        assert!(
            rx.try_recv().is_err(),
            "no catch-up SET_FACING after a suppressed turn"
        );
    }

    #[test]
    fn a_transition_does_not_swallow_the_facing_report() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD,
            ..Default::default()
        };
        player.face_yaw = 1.2;
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD | move_flags::STRAFE_RIGHT,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.05,
            &[],
            None,
            None,
            no_skip(),
        );
        let kinds: Vec<_> = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::Move { kind, .. } => kind,
                _ => panic!("expected Move commands"),
            })
            .collect();
        assert_eq!(kinds, vec![MoveKind::StartStrafeRight, MoveKind::SetFacing]);
    }

    /// One idle frame at `pos`.
    fn idle_frame(tx: &Sender<ClientCommand>, player: &mut Player, pos: bevy::prelude::Vec3) {
        player.pos = pos;
        stream_self_movement(
            tx,
            player,
            player.move_flags,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
    }

    #[test]
    fn a_resting_body_that_drifts_reports_it_once() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player::default();
        idle_frame(&tx, &mut player, bevy::prelude::Vec3::new(1.0, 2.0, 3.0));
        let ClientCommand::Move {
            kind, flags, pos, ..
        } = rx.try_recv().expect("the drift is reported")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::Heartbeat);
        assert_eq!(flags, 0, "at rest — the reconcile invents no motion");
        assert_eq!(pos, bevy_to_wow(bevy::prelude::Vec3::new(1.0, 2.0, 3.0)));
        idle_frame(&tx, &mut player, bevy::prelude::Vec3::new(1.0, 2.0, 3.0));
        assert!(
            rx.try_recv().is_err(),
            "an unchanged resting position is silent — one packet per settle, not per frame"
        );
        // One ULP still counts: vmangos's compare has no epsilon.
        idle_frame(
            &tx,
            &mut player,
            bevy::prelude::Vec3::new(1.0, f32::from_bits(2.0f32.to_bits() - 1), 3.0),
        );
        assert!(
            rx.try_recv().is_ok(),
            "a one-ULP settle is exactly what the server's exact compare would read as movement"
        );
    }

    #[test]
    fn the_reconcile_never_fires_while_the_body_is_in_motion() {
        let (tx, rx) = crossbeam_channel::unbounded();
        for flags in [move_flags::FORWARD, move_flags::FALLING] {
            let mut player = Player {
                move_flags: flags, // already streaming this state: no transition this frame
                ..Default::default()
            };
            idle_frame(&tx, &mut player, bevy::prelude::Vec3::new(9.0, 9.0, 9.0));
            assert!(
                rx.try_recv().is_err(),
                "moving ({flags:#x}): the position rides the movement stream, not a reconcile"
            );
        }
    }

    #[test]
    fn park_mover_flushes_a_stop_and_clears_stale_flags() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD,
            face_yaw: -3.0, // negative: must still go out normalized
            ..Default::default()
        };
        park_mover(&tx, &mut player);

        let ClientCommand::Move {
            flags, orientation, ..
        } = rx
            .try_recv()
            .expect("a Stop is flushed when flags were stale")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(flags, 0, "the parked Stop clears the move-flags");
        assert!(
            (0.0..TAU).contains(&orientation),
            "the parked facing is normalized into [0, 2π), got {orientation}"
        );
        assert_eq!(player.move_flags, 0, "bookkeeping is zeroed after parking");
    }

    #[test]
    fn a_speed_change_is_acked_while_not_driving() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let ack = crate::net::SpeedChangeMessage {
            guid: 7,
            kind: benilla_protocol::SpeedKind::Run,
            counter: 3,
            speed: 3.5,
        };

        // A spline ride reporting FORWARD, off any deck, with a stale ON_TRANSPORT bit.
        let player = Player {
            move_flags: move_flags::FORWARD | move_flags::FALLING | move_flags::ON_TRANSPORT,
            face_yaw: -1.0,
            ..Default::default()
        };
        ack_speeds_undriven(&tx, &player, &[ack]);
        let Ok(ClientCommand::ForceSpeedAck {
            counter,
            speed,
            flags,
            orientation,
            transport,
            jump,
            ..
        }) = rx.try_recv()
        else {
            panic!("the change is acked");
        };
        assert_eq!((counter, speed), (3, 3.5), "the ack echoes the change");
        assert_eq!(
            flags,
            move_flags::FORWARD,
            "FORWARD kept; FALLING and a tailless ON_TRANSPORT dropped"
        );
        assert!(transport.is_none() && jump.is_none());
        assert!((0.0..TAU).contains(&orientation));
        assert!(rx.try_recv().is_err(), "one ack per change");

        // On a deck.
        let player = Player {
            move_flags: move_flags::ON_TRANSPORT,
            ride: Some(super::super::state::PlayerRide {
                entity: bevy::ecs::entity::Entity::PLACEHOLDER,
                guid: 0x1F,
                local_pos: bevy::math::Vec3::ZERO,
                boat_yaw: 0.0,
            }),
            ..Default::default()
        };
        ack_speeds_undriven(&tx, &player, &[ack]);
        let Ok(ClientCommand::ForceSpeedAck {
            flags, transport, ..
        }) = rx.try_recv()
        else {
            panic!("the change is acked");
        };
        assert_eq!(flags, move_flags::ON_TRANSPORT);
        assert_eq!(transport.map(|t| t.guid), Some(0x1F));
    }

    #[test]
    fn a_knockback_acks_and_sends_no_jump() {
        let (tx, rx) = crossbeam_channel::unbounded();
        // Mid-launch, the arc seeded from the quad below: Bevy −Z is WoW +X (north), so a 25 yd/s
        // push north is `horiz_vel.z = −25`, and `jump_zspeed` is up-positive.
        let launch = benilla_protocol::JumpInfo {
            zspeed: -12.0,
            cos_angle: 1.0,
            sin_angle: 0.0,
            xy_speed: 25.0,
        };
        let mut player = Player {
            horiz_vel: bevy::prelude::Vec3::new(0.0, 0.0, -25.0),
            jump_zspeed: 12.0,
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: true, // the arc opened; only `wire_launch` tells a knockback apart
                wire_launch: true,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.0,
            &[],
            Some(super::super::state::PendingKnockback {
                guid: 0x1234,
                counter: 7,
                launch,
            }),
            None,
            no_skip(),
        );

        let ClientCommand::KnockBackAck {
            guid,
            counter,
            launch: acked,
            flags,
            ..
        } = rx.try_recv().expect("the launch owes an ack")
        else {
            panic!("expected the knockback ack first");
        };
        assert_eq!((guid, counter), (0x1234, 7), "the counter must be echoed");
        assert_eq!(
            acked, launch,
            "the four floats go back bit-for-bit — vmangos matches them within 0.01 before it will \
             relay the knockback to anyone"
        );
        assert_ne!(
            flags & move_flags::FALLING,
            0,
            "the ack's MovementInfo must carry JUMPING, or the jump tail it needs is never written"
        );

        for extra in rx.try_iter() {
            if let ClientCommand::Move { kind, .. } = extra {
                assert_ne!(
                    kind,
                    MoveKind::Jump,
                    "a knockback must not put MSG_MOVE_JUMP on the wire — instant \
                     CHEAT_TYPE_OVERSPEED_JUMP, and the reference's jump arm is a different drain arm"
                );
            }
        }
    }

    #[test]
    fn an_ordinary_jump_still_sends_its_jump() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            jump_zspeed: 7.955547,
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: true,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, jump, .. } =
            rx.try_recv().expect("a jump take-off is announced")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::Jump);
        let tail = jump.expect("the JUMP carries the ballistic tail");
        assert!(
            (tail.zspeed + 7.955547).abs() < 1.0e-4,
            "the wire zspeed is down-positive: a rising jump reports negative, got {}",
            tail.zspeed
        );
    }

    #[test]
    fn park_mover_is_a_noop_once_already_stopped() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player::default(); // move_flags == 0
        park_mover(&tx, &mut player);
        assert!(
            rx.try_recv().is_err(),
            "no Stop is sent when we were already reported stopped"
        );
    }
}
