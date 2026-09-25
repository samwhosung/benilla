//! The movement wire: the `[packed guid][MovementInfo]` relay of other units' moves, teleport
//! acks, speed changes, knockbacks, compressed batches and the movement-mode opcodes.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, MovementInfo, RelayVerb, TransportPose};
use benilla_protocol::wire::{write_packed_guid, Vector3d};
use benilla_protocol::ServerPacket;
use common::hx;

#[test]
fn movement_relay_decodes_to_unit_move() {
    // Packed guid 0xAA, then the 28-byte FORWARD MovementInfo of `client_bodies_golden`; the
    // server relays another player's move under the mover's own opcode.
    let body = hx("01aa0100000004030201cdd70bc6357e04c3f90fa7420000a03f00000000");
    match messages::parse_server(messages::opcode::MSG_MOVE_START_FORWARD, &body).unwrap() {
        ServerPacket::PlayerMove {
            guid,
            opcode,
            flags,
            position,
            orientation,
            jump,
            ..
        } => {
            assert_eq!(guid, 0xAA);
            assert_eq!(opcode, messages::opcode::MSG_MOVE_START_FORWARD);
            assert_eq!(flags, 0x1);
            assert_eq!(
                (position.x, position.y, position.z),
                (-8949.95, -132.493, 83.5312)
            );
            assert_eq!(orientation, 1.25);
            assert_eq!(jump, None, "a non-jumping relay carries no jump tail");
        }
        p => panic!("expected PlayerMove, got {}", p.name()),
    }
    // Every relay opcode shares the body shape.
    for op in [
        messages::opcode::MSG_MOVE_HEARTBEAT,
        messages::opcode::MSG_MOVE_SET_FACING,
        messages::opcode::MSG_MOVE_STOP,
    ] {
        let events = decode(messages::parse_server(op, &body).unwrap());
        assert!(
            matches!(
                events.as_slice(),
                [SessionEvent::UnitMove {
                    guid: 0xAA,
                    orientation,
                    flags: 0x1,
                    ..
                }] if *orientation == 1.25
            ),
            "opcode {op:#06x} should decode to one UnitMove, got {events:?}"
        );
    }
}

#[test]
fn jump_relay_round_trips_the_ballistic_tail() {
    // JUMPING (0x2000) adds the launch tail after the `u32` fall_time (ms): zspeed, cosAngle,
    // sinAngle, xyspeed, cos before sin (vmangos `MovementInfo::Read`).
    let mi = MovementInfo {
        flags: 0x2000 | 0x1,
        timestamp: 0x0102_0304,
        position: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
        transport: None, // not on a transport: no transport tail
        pitch: 0.0,      // not swimming: no pitch tail
        fall_time: 250,
        jump: Some(messages::JumpInfo {
            zspeed: 7.955_547,
            cos_angle: 0.25,
            sin_angle: 0.75,
            xy_speed: 7.0,
        }),
    };
    let mut body = hx("01aa"); // [packed guid 0xAA][MovementInfo], the relay shape.
    body.extend_from_slice(&messages::movement(&mi));
    match messages::parse_server(messages::opcode::MSG_MOVE_JUMP, &body).unwrap() {
        ServerPacket::PlayerMove {
            flags,
            fall_time,
            jump,
            ..
        } => {
            assert_eq!(flags, 0x2001);
            assert_eq!(fall_time, 250, "fall_time is a u32 ms count, not an f32");
            let j = jump.expect("a JUMPING relay carries the ballistic tail");
            assert_eq!(j.zspeed, 7.955_547);
            assert_eq!(
                j.cos_angle, 0.25,
                "cosAngle is the 2nd jump float (before sin)"
            );
            assert_eq!(j.sin_angle, 0.75, "sinAngle is the 3rd jump float");
            assert_eq!(j.xy_speed, 7.0);
        }
        p => panic!("expected PlayerMove, got {}", p.name()),
    }
}

/// A rider with ON_TRANSPORT, SWIMMING and JUMPING set: the transport pose, pitch, fall_time and
/// jump tail share one packet, so a wrong transport-tail length misaligns everything after it.
#[test]
fn movement_relay_surfaces_transport_pose_and_keeps_the_rest_aligned() {
    // An elevator guid (HIGH_TRANSPORT 0xF120): entry 900, low guid 4242.
    let transport_guid: u64 = 4242 | (900u64 << 24) | (0xF120u64 << 48);

    let mut body = Vec::new();
    write_packed_guid(0xAA, &mut body).unwrap(); // mover guid
    let flags: u32 = 0x0200_0000 | 0x20_0000 | 0x2000 | 0x1; // transport, swim, jump, forward
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&0x1122_3344u32.to_le_bytes()); // timestamp
    body.extend_from_slice(&10.0f32.to_le_bytes()); // position.x
    body.extend_from_slice(&20.0f32.to_le_bytes()); // position.y
    body.extend_from_slice(&30.0f32.to_le_bytes()); // position.z
    body.extend_from_slice(&0.75f32.to_le_bytes()); // orientation
    body.extend_from_slice(&transport_guid.to_le_bytes()); // ON_TRANSPORT tail: full u64 guid
    body.extend_from_slice(&1.0f32.to_le_bytes()); // local x
    body.extend_from_slice(&2.0f32.to_le_bytes()); // local y
    body.extend_from_slice(&3.0f32.to_le_bytes()); // local z
    body.extend_from_slice(&0.5f32.to_le_bytes()); // local o
    body.extend_from_slice(&(-0.3f32).to_le_bytes()); // SWIMMING tail: pitch
    body.extend_from_slice(&999u32.to_le_bytes()); // fall_time (u32, right after pitch)
    body.extend_from_slice(&7.9f32.to_le_bytes()); // JUMPING tail: zspeed
    body.extend_from_slice(&0.6f32.to_le_bytes()); // cos_angle
    body.extend_from_slice(&0.8f32.to_le_bytes()); // sin_angle
    body.extend_from_slice(&5.0f32.to_le_bytes()); // xy_speed

    let want_transport = TransportPose {
        guid: transport_guid,
        pos: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
    };

    match messages::parse_server(messages::opcode::MSG_MOVE_HEARTBEAT, &body).unwrap() {
        ServerPacket::PlayerMove {
            orientation,
            transport,
            fall_time,
            jump,
            ..
        } => {
            assert_eq!(
                orientation, 0.75,
                "position/orientation land before the tail, unaffected"
            );
            assert_eq!(
                transport,
                Some(want_transport),
                "the ON_TRANSPORT tail surfaces as the rider's local pose"
            );
            assert_eq!(
                fall_time, 999,
                "fall_time still lands right after the transport + swim-pitch tails"
            );
            let j =
                jump.expect("the jump tail still parses after the transport/pitch/fall_time run");
            assert_eq!(j.zspeed, 7.9);
            assert_eq!(j.cos_angle, 0.6);
            assert_eq!(j.sin_angle, 0.8);
            assert_eq!(j.xy_speed, 5.0);
        }
        p => panic!("expected PlayerMove, got {}", p.name()),
    }

    let events =
        decode(messages::parse_server(messages::opcode::MSG_MOVE_HEARTBEAT, &body).unwrap());
    match events.as_slice() {
        [SessionEvent::UnitMove {
            transport,
            fall_time,
            jump,
            ..
        }] => {
            assert_eq!(transport, &Some(want_transport));
            assert_eq!(*fall_time, 999);
            assert!(jump.is_some());
        }
        other => panic!("expected one UnitMove, got {other:?}"),
    }
}

#[test]
fn teleport_ack_parses_pose() {
    // MSG_MOVE_TELEPORT_ACK from the server: [packed guid][u32 counter][MovementInfo].
    let body = hx("01aa070000000100000004030201cdd70bc6357e04c3f90fa7420000a03f00000000");
    match messages::parse_server(messages::opcode::MSG_MOVE_TELEPORT_ACK, &body).unwrap() {
        ServerPacket::Teleport {
            guid,
            counter,
            position,
            orientation,
        } => {
            assert_eq!(guid, 0xAA);
            assert_eq!(counter, 7);
            assert_eq!(
                (position.x, position.y, position.z),
                (-8949.95, -132.493, 83.5312)
            );
            assert_eq!(orientation, 1.25);
        }
        p => panic!("expected Teleport, got {}", p.name()),
    }
}

/// `SMSG_FORCE_*_SPEED_CHANGE` is `[packed guid][u32 counter][f32 speed]` (vmangos
/// `SendSpeedChangeToController`); the ack is `[u64 guid][u32 counter][MovementInfo][f32 speed]`
/// with the guid unpacked (`MoveSpeedAck::ReadFromWorldPacket`).
#[test]
fn force_speed_change_parses_and_ack_body_golden() {
    use benilla_protocol::messages::SpeedKind;

    // SMSG_FORCE_RUN_SPEED_CHANGE: packed guid 8 (mask 0x01, one byte), counter 7, speed 14.0.
    let body = hx("01080700000000006041");
    let packet =
        messages::parse_server(messages::opcode::SMSG_FORCE_RUN_SPEED_CHANGE, &body).unwrap();
    match &packet {
        ServerPacket::ForceSpeedChange {
            guid: 8,
            kind: SpeedKind::Run,
            counter: 7,
            speed,
        } => assert_eq!(*speed, 14.0),
        _ => panic!("expected ForceSpeedChange (run, guid 8, counter 7)"),
    }
    match decode(packet).as_slice() {
        [SessionEvent::ForceSpeedChange {
            guid: 8,
            kind: SpeedKind::Run,
            counter: 7,
            speed,
        }] => assert_eq!(*speed, 14.0),
        other => panic!("expected one ForceSpeedChange event, got {other:?}"),
    }
    // Each kind has its own opcode; walk, swim-back and turn rate sit at 730-735, not 226-231.
    for (op, kind) in [
        (
            messages::opcode::SMSG_FORCE_WALK_SPEED_CHANGE,
            SpeedKind::Walk,
        ),
        (
            messages::opcode::SMSG_FORCE_SWIM_SPEED_CHANGE,
            SpeedKind::Swim,
        ),
        (
            messages::opcode::SMSG_FORCE_TURN_RATE_CHANGE,
            SpeedKind::TurnRate,
        ),
    ] {
        match messages::parse_server(op, &body).unwrap() {
            ServerPacket::ForceSpeedChange { kind: k, .. } => assert_eq!(k, kind),
            _ => panic!("expected ForceSpeedChange for opcode {op:#06x}"),
        }
    }

    let info = MovementInfo {
        flags: 0,
        timestamp: 12345,
        position: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
        transport: None,
        pitch: 0.0,
        fall_time: 42,
        jump: None,
    };
    assert_eq!(
        messages::force_speed_ack(8, 7, &info, 14.0),
        hx("08000000000000000700000000000000393000000000803f00000040000040400000003f2a00000000006041"),
        "CMSG_FORCE_*_SPEED_CHANGE_ACK body"
    );
    assert_eq!(SpeedKind::Run.ack_opcode(), 0x00E3);
    assert_eq!(SpeedKind::Walk.ack_opcode(), 0x02DB);
    assert_eq!(SpeedKind::TurnRate.ack_opcode(), 0x02DF);
}

/// The knockback and its ack: `SMSG_MOVE_KNOCK_BACK` orders the launch vcos, vsin, speedXY,
/// speedZ; the ack's jump tail orders it zspeed, cos, sin, xyspeed, and vmangos rejects a
/// mismatched ack. speedZ is down-positive: the reference stores it as-is (`0x7c61f0`) where a
/// jump stores -7.955547, so -12 throws the body up.
#[test]
fn knock_back_parses_and_ack_body_golden() {
    // Packed guid 8, counter 7, then vcos 0.6, vsin 0.8, speedXY 25, speedZ -12.
    let body = hx("0108070000009a99193fcdcc4c3f0000c841000040c1");
    let packet = messages::parse_server(messages::opcode::SMSG_MOVE_KNOCK_BACK, &body).unwrap();
    let launch = match &packet {
        ServerPacket::KnockBack {
            guid: 8,
            counter: 7,
            launch,
        } => *launch,
        other => panic!(
            "expected KnockBack (guid 8, counter 7), got {}",
            other.name()
        ),
    };
    assert_eq!(launch.cos_angle, 0.6, "first float on the wire is vcos");
    assert_eq!(launch.sin_angle, 0.8, "second is vsin");
    assert_eq!(launch.xy_speed, 25.0, "third is the horizontal speed");
    assert_eq!(
        launch.zspeed, -12.0,
        "fourth is speedZ, down-positive — negative throws the body up"
    );
    match decode(packet).as_slice() {
        [SessionEvent::KnockBack {
            guid: 8,
            counter: 7,
            launch: got,
        }] => assert_eq!(*got, launch),
        other => panic!("expected one KnockBack event, got {other:?}"),
    }

    // The ack: the full guid (packed only inbound), the counter, and a MovementInfo with
    // MOVEFLAG_JUMPING (0x2000), without which no jump tail is written for the server to match.
    let info = MovementInfo {
        flags: 0x2000,
        timestamp: 12345,
        position: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
        transport: None,
        pitch: 0.0,
        fall_time: 0,
        jump: Some(launch),
    };
    assert_eq!(
        messages::knock_back_ack(8, 7, &info),
        hx("08000000000000000700000000200000393000000000803f00000040000040400000003f00000000000040c19a99193fcdcc4c3f0000c841"),
        "CMSG_MOVE_KNOCK_BACK_ACK body"
    );
    assert_eq!(messages::opcode::SMSG_MOVE_KNOCK_BACK, 0x00EF);
    assert_eq!(messages::opcode::CMSG_MOVE_KNOCK_BACK_ACK, 0x00F0);
}

/// `MSG_MOVE_KNOCK_BACK` relays another unit's knockback: `[packed guid][MovementInfo]` and the
/// launch again. The MovementInfo is built from the victim's ack, whose jump tail already holds
/// the launch, so the appended quad goes unread.
#[test]
fn an_observed_knockback_relays_the_arc() {
    // packed guid 0xAA, a JUMPING MovementInfo whose tail is (zspeed -12, cos 0.6, sin 0.8, xy 25),
    // then the same quad again in the SMSG's own direction-first order.
    let body = hx("01aa00200000393000000000803f00000040000040400000003f00000000000040c19a99193fcdcc4c3f0000c8419a99193fcdcc4c3f0000c841000040c1");
    let packet = messages::parse_server(messages::opcode::MSG_MOVE_KNOCK_BACK, &body).unwrap();
    match &packet {
        ServerPacket::PlayerMove {
            guid: 0xAA,
            flags: 0x2000,
            jump: Some(j),
            ..
        } => {
            assert_eq!(j.zspeed, -12.0, "the relayed arc rises, down-positive");
            assert_eq!((j.cos_angle, j.sin_angle, j.xy_speed), (0.6, 0.8, 25.0));
        }
        other => panic!("expected a relayed PlayerMove, got {}", other.name()),
    }
    match decode(packet).as_slice() {
        [SessionEvent::UnitMove { guid: 0xAA, .. }] => {}
        other => panic!("expected one UnitMove event, got {other:?}"),
    }
    assert_eq!(messages::opcode::MSG_MOVE_KNOCK_BACK, 0x00F1);
}

/// An observed unit's speed changes, with no counter or ack: `SMSG_SPLINE_SET_*_SPEED` is
/// `[packed guid][f32 speed]` (vmangos `SendSpeedChangeToAll`), `MSG_MOVE_SET_*_SPEED` is
/// `[packed guid][MovementInfo][f32 speed]` (`SendSpeedChangeToObservers`), a pose and a speed.
#[test]
fn observer_speed_legs_parse_golden() {
    use benilla_protocol::messages::SpeedKind;

    // SMSG_SPLINE_SET_RUN_SPEED: packed guid 0xAA (mask 0x01, one byte), speed 14.0.
    let body = hx("01aa00006041");
    let packet =
        messages::parse_server(messages::opcode::SMSG_SPLINE_SET_RUN_SPEED, &body).unwrap();
    match &packet {
        ServerPacket::SplineSpeedChange {
            guid: 0xAA,
            kind: SpeedKind::Run,
            speed,
        } => assert_eq!(*speed, 14.0),
        p => panic!(
            "expected SplineSpeedChange (run, guid 0xAA), got {}",
            p.name()
        ),
    }
    match decode(packet).as_slice() {
        [SessionEvent::SpeedChanged {
            guid: 0xAA,
            kind: SpeedKind::Run,
            speed,
        }] => assert_eq!(*speed, 14.0),
        other => panic!("expected one SpeedChanged event, got {other:?}"),
    }
    // Walk (769) and swim-back (770) sit in a different order from the FORCE family's.
    for (op, kind) in [
        (
            messages::opcode::SMSG_SPLINE_SET_WALK_SPEED,
            SpeedKind::Walk,
        ),
        (
            messages::opcode::SMSG_SPLINE_SET_SWIM_BACK_SPEED,
            SpeedKind::SwimBack,
        ),
    ] {
        match messages::parse_server(op, &body).unwrap() {
            ServerPacket::SplineSpeedChange { kind: k, .. } => assert_eq!(k, kind),
            p => panic!("expected SplineSpeedChange for {op:#06x}, got {}", p.name()),
        }
    }

    // MSG_MOVE_SET_RUN_SPEED: packed guid 0xAA, the shared FORWARD MovementInfo, speed 14.0.
    let body = hx("01aa0100000004030201cdd70bc6357e04c3f90fa7420000a03f0000000000006041");
    let packet = messages::parse_server(messages::opcode::MSG_MOVE_SET_RUN_SPEED, &body).unwrap();
    match &packet {
        ServerPacket::MoveSetSpeed {
            guid: 0xAA,
            kind: SpeedKind::Run,
            flags: 0x1,
            orientation,
            speed,
            ..
        } => {
            assert_eq!(*orientation, 1.25);
            assert_eq!(*speed, 14.0);
        }
        p => panic!("expected MoveSetSpeed (run, guid 0xAA), got {}", p.name()),
    }
    match decode(packet).as_slice() {
        [SessionEvent::UnitMove {
            guid: 0xAA,
            flags: 0x1,
            position,
            ..
        }, SessionEvent::SpeedChanged {
            guid: 0xAA,
            kind: SpeedKind::Run,
            speed,
        }] => {
            assert_eq!(position, &[-8949.95, -132.493, 83.5312]);
            assert_eq!(*speed, 14.0);
        }
        other => panic!("expected UnitMove + SpeedChanged, got {other:?}"),
    }

    // SMSG_MOUNTRESULT / SMSG_DISMOUNTRESULT: one u32 code (OK = 10 / 3).
    match decode(
        messages::parse_server(messages::opcode::SMSG_MOUNTRESULT, &hx("0a000000")).unwrap(),
    )
    .as_slice()
    {
        [SessionEvent::MountResult {
            mount: true,
            code: 10,
        }] => {}
        other => panic!("expected MountResult(mount, 10), got {other:?}"),
    }
    match decode(
        messages::parse_server(messages::opcode::SMSG_DISMOUNTRESULT, &hx("03000000")).unwrap(),
    )
    .as_slice()
    {
        [SessionEvent::MountResult {
            mount: false,
            code: 3,
        }] => {}
        other => panic!("expected MountResult(dismount, 3), got {other:?}"),
    }

    // SMSG_MOUNTSPECIAL_ANIM: one unpacked `u64` guid (vmangos `HandleMountSpecialAnimOpcode`).
    match decode(
        messages::parse_server(
            messages::opcode::SMSG_MOUNTSPECIAL_ANIM,
            &hx("0400000000000040"),
        )
        .unwrap(),
    )
    .as_slice()
    {
        [SessionEvent::MountSpecial {
            guid: 0x4000_0000_0000_0004,
        }] => {}
        other => panic!("expected MountSpecial(player 4), got {other:?}"),
    }
}

/// `SMSG_COMPRESSED_MOVES`, which vmangos switches a session to after 300 movement packets in ten
/// seconds: `[u32 size][zlib(records)]`, each record `[u8 size][u16 opcode][body]` with the size
/// counting the opcode (vmangos `MovementData::BuildPacket`).
#[test]
fn compressed_moves_unwraps_to_the_same_events_as_loose_relays() {
    let body = hx("42000000780153d8cac0b88a91818181859989f1ec75ee63a6752c877ff22f77626058600f146650b805540062602a38005600002f2610d9");
    let events =
        decode(messages::parse_server(messages::opcode::SMSG_COMPRESSED_MOVES, &body).unwrap());

    // Both records, in wire order: the remote-replay queue rebuilds a mover's arc from that order.
    match events.as_slice() {
        [SessionEvent::UnitMove {
            guid: 0xAA,
            flags: 0x1,
            ..
        }, SessionEvent::UnitMove {
            guid: 0xAA,
            flags: 0x0,
            ..
        }] => {}
        other => panic!(
            "expected the moving then the idle relay for mover 0xAA, in that order, got {other:?}"
        ),
    }

    // The batch's two records, relayed loose under their own opcodes.
    let loose: Vec<SessionEvent> = [
        (
            messages::opcode::MSG_MOVE_START_FORWARD,
            "01aa0100000004030201cdd70bc6357e04c3f90fa7420000a03f00000000",
        ),
        (
            messages::opcode::MSG_MOVE_SET_FACING,
            "01aa0000000004030201cdd70bc6357e04c3f90fa7420000c03f00000000",
        ),
    ]
    .into_iter()
    .flat_map(|(opcode, body)| decode(messages::parse_server(opcode, &hx(body)).unwrap()))
    .collect();
    assert_eq!(format!("{events:?}"), format!("{loose:?}"));
}

/// A misframed batch is a parse error, never a silent drop of the moves it carries.
#[test]
fn a_misframed_batch_errors_rather_than_dropping_moves() {
    // A HEARTBEAT record claiming a 40-byte body with only 4 bytes left in the stream.
    let truncated = hx("070000007801d37ac700020006c10119");
    assert!(messages::parse_server(messages::opcode::SMSG_COMPRESSED_MOVES, &truncated).is_err());

    // A record whose size (1) cannot hold its opcode: misframed, and it must not underflow.
    let undersized = hx("030000007801637cc7000001e200f0");
    assert!(messages::parse_server(messages::opcode::SMSG_COMPRESSED_MOVES, &undersized).is_err());
}

/// The twelve `SMSG_SPLINE_MOVE_*` opcodes (reference dispatch `0x304..=0x31A`, root at `0x31A`):
/// the body is a bare packed guid with no counter, and `SET_RUN_MODE` clears
/// `MOVEFLAG_WALK_MODE`, because the reference (`0x617e80`) passes its bool to `SetRunMode`.
#[test]
fn spline_move_mode_family_parses_golden() {
    use benilla_protocol::messages::SplineMode;

    let body = hx("01aa");
    assert_eq!(body.len(), 2, "the whole body is one packed guid");

    let expected = [
        (
            messages::opcode::SMSG_SPLINE_MOVE_ROOT,
            SplineMode::Root,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_UNROOT,
            SplineMode::Root,
            false,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_WATER_WALK,
            SplineMode::WaterWalk,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_LAND_WALK,
            SplineMode::WaterWalk,
            false,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_FEATHER_FALL,
            SplineMode::FeatherFall,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_NORMAL_FALL,
            SplineMode::FeatherFall,
            false,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_SET_HOVER,
            SplineMode::Hover,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_UNSET_HOVER,
            SplineMode::Hover,
            false,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_START_SWIM,
            SplineMode::Swimming,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_STOP_SWIM,
            SplineMode::Swimming,
            false,
        ),
        // The inversion: the WALK opcode sets the bit, the RUN opcode clears it.
        (
            messages::opcode::SMSG_SPLINE_MOVE_SET_WALK_MODE,
            SplineMode::WalkMode,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_SET_RUN_MODE,
            SplineMode::WalkMode,
            false,
        ),
    ];

    for (op, mode, apply) in expected {
        let packet = messages::parse_server(op, &body)
            .unwrap_or_else(|e| panic!("{op:#06x} must parse from a bare packed guid: {e}"));
        match &packet {
            ServerPacket::SplineMoveMode {
                guid: 0xAA,
                mode: m,
                apply: a,
            } => assert_eq!(
                (*m, *a),
                (mode, apply),
                "{} decoded to the wrong mode/direction",
                packet.name()
            ),
            p => panic!("expected SplineMoveMode for {op:#06x}, got {}", p.name()),
        }
        match decode(packet).as_slice() {
            [SessionEvent::SplineMoveMode {
                guid: 0xAA,
                mode: m,
                apply: a,
            }] => assert_eq!((*m, *a), (mode, apply)),
            other => panic!("expected one SplineMoveMode event for {op:#06x}, got {other:?}"),
        }
    }

    assert_eq!(SplineMode::Root.flag(), messages::MoveMode::Root.flag());
    assert_eq!(
        SplineMode::WaterWalk.flag(),
        messages::MoveMode::WaterWalk.flag()
    );
    assert_eq!(
        SplineMode::FeatherFall.flag(),
        messages::MoveMode::FeatherFall.flag()
    );
    assert_eq!(SplineMode::Hover.flag(), messages::MoveMode::Hover.flag());
}

/// The movement-mode opcodes relayed to observers (root, hover, feather fall, water walk,
/// teleport) use the plain relay shape, no counter. Only root has a separate off opcode: vmangos
/// updates the others' flag before broadcasting (`MovementHandler.cpp:620-631`, `:736-737`), so
/// the flags word carries the state. Only `MSG_MOVE_TELEPORT` is tagged as a teleport.
#[test]
fn observer_move_mode_family_parses_golden() {
    // Packed guid 0xAA and a 28-byte MovementInfo with no tails: time 12345, pos (1, 2, 3),
    // orientation 0.5, fall_time 0; 30 bytes, four fewer than `MSG_MOVE_TELEPORT_ACK`'s.
    let info = |flags: u32| {
        let mut b = hx("01aa");
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&hx("393000000000803f00000040000040400000003f00000000"));
        b
    };

    const ROOT: u32 = 0x0000_1000;
    const WATER_WALK: u32 = 0x1000_0000;
    const FEATHER_FALL: u32 = 0x2000_0000;
    const HOVER: u32 = 0x4000_0000;

    // (opcode, the flags word the server writes into it, the opcode's verb)
    let expected = [
        (messages::opcode::MSG_MOVE_ROOT, ROOT, RelayVerb::Root(true)),
        (messages::opcode::MSG_MOVE_UNROOT, 0, RelayVerb::Root(false)),
        (
            messages::opcode::MSG_MOVE_WATER_WALK,
            WATER_WALK,
            RelayVerb::Pose,
        ),
        // The same opcode, the other direction: the bit is absent and the verb is still `Pose`.
        (messages::opcode::MSG_MOVE_WATER_WALK, 0, RelayVerb::Pose),
        (
            messages::opcode::MSG_MOVE_FEATHER_FALL,
            FEATHER_FALL,
            RelayVerb::Pose,
        ),
        (messages::opcode::MSG_MOVE_HOVER, HOVER, RelayVerb::Pose),
        // Levitate grants all three at once: one word, three bits.
        (
            messages::opcode::MSG_MOVE_HOVER,
            HOVER | FEATHER_FALL | WATER_WALK,
            RelayVerb::Pose,
        ),
        (messages::opcode::MSG_MOVE_TELEPORT, 0, RelayVerb::Teleport),
    ];

    for (op, flags, expected_verb) in expected {
        let body = info(flags);
        assert_eq!(body.len(), 30, "the relay shape, with no counter dword");
        let packet = messages::parse_server(op, &body)
            .unwrap_or_else(|e| panic!("{op:#06x} must parse as an ordinary relay: {e}"));
        match &packet {
            ServerPacket::PlayerMove {
                guid: 0xAA,
                opcode,
                flags: f,
                position,
                ..
            } => {
                assert_eq!(*opcode, op);
                assert_eq!(*f, flags, "the whole flags word survives for {op:#06x}");
                assert_eq!(
                    (position.x, position.y, position.z),
                    (1.0, 2.0, 3.0),
                    "the pose starts right after the guid for {op:#06x}"
                );
            }
            other => panic!(
                "expected a relayed PlayerMove for {op:#06x}, got {}",
                other.name()
            ),
        }
        match decode(packet).as_slice() {
            [SessionEvent::UnitMove {
                guid: 0xAA,
                flags: f,
                verb,
                ..
            }] => {
                assert_eq!(*f, flags);
                assert_eq!(
                    *verb, expected_verb,
                    "the opcode's verb, and only for the three that have one ({op:#06x})"
                );
            }
            other => panic!("expected one UnitMove event for {op:#06x}, got {other:?}"),
        }
    }

    // Opcode numbers (vmangos `Opcodes_1_12_1.h`).
    assert_eq!(messages::opcode::MSG_MOVE_TELEPORT, 197);
    assert_eq!(messages::opcode::MSG_MOVE_TELEPORT_ACK, 199);
    assert_eq!(messages::opcode::MSG_MOVE_ROOT, 236);
    assert_eq!(messages::opcode::MSG_MOVE_UNROOT, 237);
    assert_eq!(messages::opcode::MSG_MOVE_HOVER, 247);
    assert_eq!(messages::opcode::MSG_MOVE_FEATHER_FALL, 688);
    assert_eq!(messages::opcode::MSG_MOVE_WATER_WALK, 689);
}
