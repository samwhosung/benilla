//! Flight-master wire: the taxi send verbs, the node map and its mask, status, reply and path ack.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, taxi_reply, TaxiMask};
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn taxi_send_bodies_golden() {
    // Both queries are one full 8-byte guid (vmangos `Packets/Taxi.cpp`).
    assert_eq!(
        messages::taxi_node_status_query(0x1234_5678_9abc_def0),
        hx("f0debc9a78563412"),
        "CMSG_TAXINODE_STATUS_QUERY body"
    );
    assert_eq!(
        messages::taxi_query_available_nodes(0x1234_5678_9abc_def0),
        hx("f0debc9a78563412"),
        "CMSG_TAXIQUERYAVAILABLENODES body"
    );

    // CMSG_ACTIVATETAXI: guid, from node, to node; Stormwind (2) -> Sentinel Hill (4).
    assert_eq!(
        messages::activate_taxi(0x1234_5678_9abc_def0, 2, 4),
        hx(concat!("f0debc9a78563412", "02000000", "04000000")),
        "CMSG_ACTIVATETAXI body"
    );

    // CMSG_ACTIVATETAXIEXPRESS: guid, u32 total cost, u32 count, count x u32 node.
    assert_eq!(
        messages::activate_taxi_express(0x1234_5678_9abc_def0, 110, &[2, 3, 4]),
        hx(concat!(
            "f0debc9a78563412",
            "6e000000",
            "03000000",
            "02000000",
            "03000000",
            "04000000"
        )),
        "CMSG_ACTIVATETAXIEXPRESS body"
    );
    assert_eq!(
        messages::activate_taxi_express(0x1234_5678_9abc_def0, 0, &[]),
        hx(concat!("f0debc9a78563412", "00000000", "00000000")),
        "CMSG_ACTIVATETAXIEXPRESS body, empty node list"
    );
}

#[test]
fn taxi_mask_bit_law() {
    // Node ids are 1-based: field (id-1)/32, bit (id-1)%32 (vmangos `PlayerTaxi.h`).
    let mask = TaxiMask([0x0000_000A, 0x0000_0001, 0, 0, 0, 0, 0, 0]);
    assert!(mask.is_known(2));
    assert!(mask.is_known(4));
    assert!(mask.is_known(33));
    assert!(!mask.is_known(3), "bit not set for node 3");
    assert!(!mask.is_known(65), "field 2 is all-zero");
    assert!(!mask.is_known(0), "node id 0 is never valid");
}

#[test]
fn show_taxi_nodes_wire() {
    // SMSG_SHOWTAXINODES (vmangos `TaxiHandler.cpp:82-96`): window always 1, guid not packed.
    let mut body = 1u32.to_le_bytes().to_vec(); // window constant
    body.extend_from_slice(&0xCCu64.to_le_bytes()); // flightmaster guid
    body.extend_from_slice(&2u32.to_le_bytes()); // nearest node
    let mask_words = [0x0000_0002u32, 0x0000_0001, 0, 0, 0, 0, 0, 0];
    for w in mask_words {
        body.extend_from_slice(&w.to_le_bytes());
    }
    assert_eq!(body.len(), 4 + 8 + 4 + 8 * 4, "48-byte fixed body");

    match messages::parse_server(messages::opcode::SMSG_SHOWTAXINODES, &body).unwrap() {
        ServerPacket::ShowTaxiNodes {
            window,
            flightmaster,
            nearest_node,
            known,
        } => {
            assert_eq!((window, flightmaster, nearest_node), (1, 0xCC, 2));
            assert_eq!(known, TaxiMask(mask_words));
            assert!(known.is_known(2));
            assert!(known.is_known(33));
            assert!(!known.is_known(3));
        }
        other => panic!("show taxi nodes, got {}", other.name()),
    }

    let packet = messages::parse_server(messages::opcode::SMSG_SHOWTAXINODES, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::TaxiNodesShown {
            flightmaster,
            nearest_node,
            known_mask,
        } => {
            assert_eq!((flightmaster, nearest_node), (0xCC, 2));
            assert_eq!(known_mask, TaxiMask(mask_words));
        }
        other => panic!("taxi nodes shown event, got {other:?}"),
    }

    // Mode 0 skips the flight master and the node and reads the mask alone (`0x5ece9a`), for the
    // console's list (`0x5ecf37`); vmangos never sends it.
    let mut listed = 0u32.to_le_bytes().to_vec();
    for w in mask_words {
        listed.extend_from_slice(&w.to_le_bytes());
    }
    let packet = messages::parse_server(messages::opcode::SMSG_SHOWTAXINODES, &listed).unwrap();
    match decode(packet).as_slice() {
        [SessionEvent::TaxiNodesListed { known_mask }] => {
            assert_eq!(*known_mask, TaxiMask(mask_words))
        }
        other => panic!("taxi nodes listed event, got {other:?}"),
    }
}

#[test]
fn taxi_node_status_wire() {
    // SMSG_TAXINODE_STATUS (vmangos `TaxiNodeStatus::AppendBodyTo`): raw u64 guid + u8 known.
    let mut known_body = 0xCCu64.to_le_bytes().to_vec();
    known_body.push(1);
    match messages::parse_server(messages::opcode::SMSG_TAXINODE_STATUS, &known_body).unwrap() {
        ServerPacket::TaxiNodeStatus { guid, known } => assert_eq!((guid, known), (0xCC, true)),
        other => panic!("taxi node status, got {}", other.name()),
    }
    let packet =
        messages::parse_server(messages::opcode::SMSG_TAXINODE_STATUS, &known_body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::TaxiNodeStatus { guid, known } => assert_eq!((guid, known), (0xCC, true)),
        other => panic!("taxi node status event, got {other:?}"),
    }

    let mut unknown_body = 0xDDu64.to_le_bytes().to_vec();
    unknown_body.push(0);
    match messages::parse_server(messages::opcode::SMSG_TAXINODE_STATUS, &unknown_body).unwrap() {
        ServerPacket::TaxiNodeStatus { guid, known } => assert_eq!((guid, known), (0xDD, false)),
        other => panic!("taxi node status (unknown), got {}", other.name()),
    }
}

#[test]
fn activate_taxi_reply_and_new_taxi_path_wire() {
    // SMSG_ACTIVATETAXIREPLY (vmangos `ActivateTaxiReply::AppendBodyTo`): one u32 TaxiError code.
    let body = taxi_reply::NOT_ENOUGH_MONEY.to_le_bytes().to_vec();
    match messages::parse_server(messages::opcode::SMSG_ACTIVATETAXIREPLY, &body).unwrap() {
        ServerPacket::ActivateTaxiReply { code } => {
            assert_eq!(code, taxi_reply::NOT_ENOUGH_MONEY);
        }
        other => panic!("activate taxi reply, got {}", other.name()),
    }
    let packet = messages::parse_server(messages::opcode::SMSG_ACTIVATETAXIREPLY, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::ActivateTaxiReply { code } => {
            assert_eq!(code, taxi_reply::NOT_ENOUGH_MONEY);
        }
        other => panic!("activate taxi reply event, got {other:?}"),
    }

    // vmangos `TaxiError` (`Objects/Player.h:404-416`); `ActivateTaxiPathTo` only ever sends 0-4
    // and 7-9 (`Player.cpp:18008-18252`).
    assert_eq!(
        [
            taxi_reply::OK,
            taxi_reply::UNSPECIFIED_SERVER_ERROR,
            taxi_reply::NO_SUCH_PATH,
            taxi_reply::NOT_ENOUGH_MONEY,
            taxi_reply::TOO_FAR,
            taxi_reply::NO_VENDOR_NEARBY,
            taxi_reply::NOT_VISITED,
            taxi_reply::BUSY,
            taxi_reply::ALREADY_MOUNTED,
            taxi_reply::SHAPESHIFTED,
            taxi_reply::PLAYER_MOVING,
            taxi_reply::SAME_NODE,
            taxi_reply::NOT_STANDING,
        ],
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    );

    match messages::parse_server(messages::opcode::SMSG_NEW_TAXI_PATH, &[]).unwrap() {
        ServerPacket::NewTaxiPath => {}
        other => panic!("new taxi path, got {}", other.name()),
    }
    let packet = messages::parse_server(messages::opcode::SMSG_NEW_TAXI_PATH, &[]).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::NewTaxiPath => {}
        other => panic!("new taxi path event, got {other:?}"),
    }
}
