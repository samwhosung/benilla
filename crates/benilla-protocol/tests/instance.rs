//! The instance lockout messages in the reference's read order (handlers registered at
//! `0x498680`-`0x4986cf` and `0x4e7e60`; vmangos `Server/Packets/Misc.cpp`).

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{
    self, opcode, InstanceResetFailure, RaidInstanceMessage, RaidInstanceWarning,
};
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// `CMSG_RESET_INSTANCES` is empty in the reference (`0x48a6b0`) and in vmangos's handler.
#[test]
fn reset_instances_body_is_empty() {
    assert_eq!(messages::reset_instances(), Vec::<u8>::new());
}

/// `SMSG_RAID_INSTANCE_MESSAGE`: `u32` type, map id and seconds until reset, in that order; the
/// reference reads them at `0x49e1cd`/`0x49e1d8`/`0x49e1e3` and looks up `Map.dbc` by the second.
#[test]
fn raid_instance_message_wire() {
    // type 4 (WELCOME), map 409 (Molten Core), 3 d 2 h 0 m = 266400 s.
    let body = hx(concat!("04000000", "99010000", "a0100400"));
    let p = messages::parse_server(opcode::SMSG_RAID_INSTANCE_MESSAGE, &body).unwrap();
    match &p {
        ServerPacket::RaidInstanceMessage { message } => {
            assert_eq!(
                *message,
                RaidInstanceMessage {
                    message_type: 4,
                    map: 409,
                    reset: 266_400,
                }
            );
        }
        other => panic!("expected RaidInstanceMessage, got {}", other.name()),
    }
    match decode(p).as_slice() {
        [SessionEvent::RaidInstanceMessage { message }] => {
            assert_eq!(message.map, 409);
            assert_eq!(message.reset, 266_400);
        }
        other => panic!("raid instance message decode: {other:?}"),
    }

    // A body one field short is a parse failure, not a zero-filled message.
    assert!(
        messages::parse_server(opcode::SMSG_RAID_INSTANCE_MESSAGE, &hx("0400000099010000"))
            .is_err()
    );
}

/// The seconds are a signed word (`0x49e1e9`, divided by `imul` at `0x49e259`): 2^31 reads
/// negative.
#[test]
fn a_reset_word_past_the_sign_bit_reads_negative() {
    let body = hx(concat!("01000000", "99010000", "00000080"));
    match messages::parse_server(opcode::SMSG_RAID_INSTANCE_MESSAGE, &body).unwrap() {
        ServerPacket::RaidInstanceMessage { message } => assert_eq!(message.reset, i32::MIN),
        other => panic!("expected RaidInstanceMessage, got {}", other.name()),
    }
}

/// The four warning types; the reference's jump table (`0x49e246`) prints nothing for 0 or 5 and
/// up, including 5, which later clients call `RAID_INSTANCE_EXPIRED`.
#[test]
fn raid_instance_warning_types() {
    use RaidInstanceWarning as W;
    assert_eq!(W::from_wire(1), Some(W::Hours));
    assert_eq!(W::from_wire(2), Some(W::Minutes));
    assert_eq!(W::from_wire(3), Some(W::MinutesSoon));
    assert_eq!(W::from_wire(4), Some(W::Welcome));
    assert_eq!(W::from_wire(0), None);
    assert_eq!(
        W::from_wire(5),
        None,
        "RAID_INSTANCE_EXPIRED prints nothing"
    );
    assert_eq!(W::from_wire(u32::MAX), None);

    assert_eq!(W::Hours.token(), "RAID_INSTANCE_WARNING_HOURS");
    assert_eq!(W::Minutes.token(), "RAID_INSTANCE_WARNING_MIN");
    assert_eq!(W::MinutesSoon.token(), "RAID_INSTANCE_WARNING_MIN_SOON");
    assert_eq!(W::Welcome.token(), "RAID_INSTANCE_WELCOME");
}

/// `SMSG_INSTANCE_RESET`: one `u32` map id (`0x49e481`).
#[test]
fn instance_reset_wire() {
    let p = messages::parse_server(opcode::SMSG_INSTANCE_RESET, &hx("24000000")).unwrap();
    match &p {
        ServerPacket::InstanceReset { map } => assert_eq!(*map, 36), // Deadmines
        other => panic!("expected InstanceReset, got {}", other.name()),
    }
    match decode(p).as_slice() {
        [SessionEvent::InstanceReset { map: 36 }] => {}
        other => panic!("instance reset decode: {other:?}"),
    }
    assert!(messages::parse_server(opcode::SMSG_INSTANCE_RESET, &hx("2400")).is_err());
}

/// `SMSG_INSTANCE_RESET_FAILED`: `u32` reason first, then `u32` map id (`0x49e54d`/`0x49e558`).
#[test]
fn instance_reset_failed_wire() {
    let body = hx(concat!("01000000", "24000000")); // OFFLINE, Deadmines
    let p = messages::parse_server(opcode::SMSG_INSTANCE_RESET_FAILED, &body).unwrap();
    match &p {
        ServerPacket::InstanceResetFailed { failure } => {
            assert_eq!(failure.reason, 1);
            assert_eq!(failure.map, 36);
        }
        other => panic!("expected InstanceResetFailed, got {}", other.name()),
    }
    match decode(p).as_slice() {
        [SessionEvent::InstanceResetFailed { failure }] => {
            assert_eq!(
                InstanceResetFailure::from_wire(failure.reason),
                Some(InstanceResetFailure::Offline)
            );
        }
        other => panic!("instance reset failed decode: {other:?}"),
    }
}

/// The three refusal reasons; vmangos names 3 and up `INSTANCERESET_FAIL_SILENTLY`. Deviation:
/// for those we print nothing, because the reference prints an uninitialized buffer.
#[test]
fn instance_reset_failure_reasons() {
    use InstanceResetFailure as F;
    assert_eq!(F::from_wire(0), Some(F::General));
    assert_eq!(F::from_wire(1), Some(F::Offline));
    assert_eq!(F::from_wire(2), Some(F::Zoning));
    assert_eq!(F::from_wire(3), None, "INSTANCERESET_FAIL_SILENTLY");
    assert_eq!(F::from_wire(99), None);

    assert_eq!(F::General.token(), "INSTANCE_RESET_FAILED");
    assert_eq!(F::Offline.token(), "INSTANCE_RESET_FAILED_OFFLINE");
    assert_eq!(F::Zoning.token(), "INSTANCE_RESET_FAILED_ZONING");
}

/// Three bare `u32` bodies: the save-created flag (`0x4e7e6c`), the last-instance map
/// (`0x49e676`) and the ownership flag (`0x49e6c6`), a bool: the reference tests it against zero.
#[test]
fn save_created_last_instance_and_ownership_wire() {
    let p = messages::parse_server(opcode::SMSG_INSTANCE_SAVE_CREATED, &hx("00000000")).unwrap();
    match decode(p).as_slice() {
        [SessionEvent::InstanceSaveCreated { flag: 0 }] => {}
        other => panic!("save created decode: {other:?}"),
    }

    let p = messages::parse_server(opcode::SMSG_UPDATE_LAST_INSTANCE, &hx("99010000")).unwrap();
    match decode(p).as_slice() {
        [SessionEvent::UpdateLastInstance { map: 409 }] => {}
        other => panic!("last instance decode: {other:?}"),
    }

    for (body, owns) in [("00000000", false), ("01000000", true), ("07000000", true)] {
        let p = messages::parse_server(opcode::SMSG_UPDATE_INSTANCE_OWNERSHIP, &hx(body)).unwrap();
        match decode(p).as_slice() {
            [SessionEvent::UpdateInstanceOwnership { owns: got }] => assert_eq!(*got, owns),
            other => panic!("ownership decode: {other:?}"),
        }
    }
}
