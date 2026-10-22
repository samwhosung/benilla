//! The reputation pane's wire: its three client verbs, in vmangos's read order, and the one push
//! without a standing, `SMSG_SET_FACTION_VISIBLE`.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, opcode};
use benilla_protocol::ServerPacket;
use common::hx;

/// `CMSG_SET_FACTION_ATWAR` (293), the pane's crossed-swords box: `u32` repListId, `u8` flag
/// (vmangos `SetFactionAtWar::ReadFromWorldPacket`).
#[test]
fn set_faction_at_war_body_is_slot_then_flag() {
    assert_eq!(opcode::CMSG_SET_FACTION_ATWAR, 293);
    // Slot 21 (Darnassus), at war.
    assert_eq!(messages::set_faction_at_war(21, true), hx("1500000001"));
    // The flag is a whole byte: off is 0, not a cleared bit.
    assert_eq!(messages::set_faction_at_war(21, false), hx("1500000000"));
    // Slot 0 is a real faction (the Bloodsail Buccaneers), an ordinary request here.
    assert_eq!(messages::set_faction_at_war(0, true), hx("0000000001"));
}

/// `CMSG_SET_FACTION_INACTIVE` (791), the "move to inactive" box: `u32` repListId, `u8` inactive
/// (vmangos `SetFactionInactive::ReadFromWorldPacket`).
#[test]
fn set_faction_inactive_body_is_slot_then_flag() {
    assert_eq!(opcode::CMSG_SET_FACTION_INACTIVE, 791);
    assert_eq!(messages::set_faction_inactive(54, true), hx("3600000001"));
    assert_eq!(messages::set_faction_inactive(54, false), hx("3600000000"));
}

/// `CMSG_SET_WATCHED_FACTION` (792): one signed `i32` slot, -1 for none. vmangos writes it into
/// `PLAYER_FIELD_WATCHED_FACTION_INDEX` (`HandleSetWatchedFactionOpcode`) and slot 0 is a real
/// faction, so the binding maps FrameXML's `SetWatchedFactionIndex(0)` to `WATCHED_FACTION_NONE`.
#[test]
fn set_watched_faction_body_is_a_signed_slot_and_none_is_minus_one() {
    assert_eq!(opcode::CMSG_SET_WATCHED_FACTION, 792);
    assert_eq!(messages::WATCHED_FACTION_NONE, -1);
    assert_eq!(messages::set_watched_faction(11), hx("0b000000"));
    assert_eq!(
        messages::set_watched_faction(messages::WATCHED_FACTION_NONE),
        hx("ffffffff")
    );
    assert_ne!(
        messages::set_watched_faction(0),
        messages::set_watched_faction(messages::WATCHED_FACTION_NONE)
    );
}

/// `SMSG_SET_FACTION_VISIBLE` (291): one `u32` reputation-list slot (vmangos
/// `ReputationMgr::SendVisible`), pushed the first time the player meets a faction.
#[test]
fn set_faction_visible_parses_and_decodes() {
    assert_eq!(opcode::SMSG_SET_FACTION_VISIBLE, 291);
    let p = messages::parse_server(opcode::SMSG_SET_FACTION_VISIBLE, &hx("0d000000")).unwrap();
    assert!(matches!(p, ServerPacket::SetFactionVisible { list_id: 13 }));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::ReputationVisible { list_id: 13 }]
    ));
}

/// `SMSG_SET_FACTION_ATWAR` (787): a `u32` slot and a `u8` flag byte (`0x4d56be`, `0x4d56c9`;
/// vmangos `FactionAtWarChange::AppendBodyTo`), whose bit `0x02` the handler copies.
#[test]
fn set_faction_at_war_parses_and_decodes() {
    assert_eq!(opcode::SMSG_SET_FACTION_ATWAR, 787);
    let p = messages::parse_server(opcode::SMSG_SET_FACTION_ATWAR, &hx("1500000002")).unwrap();
    assert!(matches!(
        p,
        ServerPacket::SetFactionAtWar {
            list_id: 21,
            flags: 2
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::ReputationAtWar {
            list_id: 21,
            flags: 2
        }]
    ));
    // vmangos clears with 0.
    let p = messages::parse_server(opcode::SMSG_SET_FACTION_ATWAR, &hx("1500000000")).unwrap();
    assert!(matches!(
        p,
        ServerPacket::SetFactionAtWar {
            list_id: 21,
            flags: 0
        }
    ));
}
