//! Server packets with small fixed bodies, parsed and decoded against hand-built bytes.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::{MoveMode, ServerPacket};
use common::hx;

#[test]
fn simple_server_bodies_parse() {
    match messages::parse_server(messages::opcode::SMSG_AUTH_CHALLENGE, &hx("efbeadde")).unwrap() {
        ServerPacket::AuthChallenge { server_seed } => assert_eq!(server_seed, 0xDEAD_BEEF),
        _ => panic!("auth challenge"),
    }
    // SMSG_NEW_WORLD: u32 map, f32 x/y/z, f32 orientation.
    let nw = hx("010000000000803f0000004000004040000000bf");
    match messages::parse_server(messages::opcode::SMSG_NEW_WORLD, &nw).unwrap() {
        ServerPacket::NewWorld {
            map,
            position,
            orientation,
        } => {
            assert_eq!(map, 1);
            assert_eq!((position.x, position.y, position.z), (1.0, 2.0, 3.0));
            assert_eq!(orientation, -0.5);
        }
        _ => panic!("new world"),
    }
    // SMSG_LOGIN_SETTIMESPEED: packed date (LSB up: minute:6, hour:5, weekday:3, day:6, month:4,
    // year:5) then f32 timescale. The day serial counts 31-day months: year·372 + month·31 + day.
    let datetime: u32 = (26 << 24) | (6 << 20) | (17 << 14) | (14 << 6) | 30;
    let mut ts = datetime.to_le_bytes().to_vec();
    ts.extend_from_slice(&0.0166_6667f32.to_le_bytes());
    match messages::parse_server(messages::opcode::SMSG_LOGIN_SETTIMESPEED, &ts).unwrap() {
        ServerPacket::TimeSpeed {
            hours,
            minutes,
            day_serial,
            timescale,
        } => {
            assert_eq!((hours, minutes), (14, 30));
            assert_eq!(day_serial, 26 * 372 + 6 * 31 + 17);
            assert_eq!(timescale, 0.0166_6667);
        }
        _ => panic!("settimespeed"),
    }
}

/// The response is one u32 of unix-epoch seconds (vmangos `Handlers/QueryHandler.cpp:418-423`).
#[test]
fn query_time_asks_empty_and_decodes_the_server_wall_clock() {
    assert!(
        messages::query_time().is_empty(),
        "CMSG_QUERY_TIME is a NullClientPacket"
    );

    // 2026-08-13T12:00:00Z = 1_786_622_400 = 0x6a7db1c0: four distinct bytes.
    assert_eq!(u32::from_le_bytes([0xc0, 0xb1, 0x7d, 0x6a]), 1_786_622_400);
    let p = messages::parse_server(messages::opcode::SMSG_QUERY_TIME_RESPONSE, &hx("c0b17d6a"))
        .unwrap();
    assert!(matches!(
        p,
        ServerPacket::QueryTimeResponse {
            unix_time: 1_786_622_400
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::ServerUnixTime {
            unix_time: 1_786_622_400
        }]
    ));

    assert!(
        messages::parse_server(messages::opcode::SMSG_QUERY_TIME_RESPONSE, &hx("c0b17d")).is_err()
    );
}

/// `SMSG_TRIGGER_CINEMATIC` is one u32 `CinematicSequences.dbc` id; `SMSG_CHAR_DELETE` one byte,
/// which the handshake consumes, so it decodes to no event.
#[test]
fn cinematic_and_char_delete_parse_and_decode() {
    // 41 = the dwarf intro.
    let body = 41u32.to_le_bytes();
    let p = messages::parse_server(messages::opcode::SMSG_TRIGGER_CINEMATIC, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::TriggerCinematic { cinematic_id: 41 }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::CinematicTriggered { cinematic_id: 41 }]
    ));

    // 0x39 = CHAR_DELETE_SUCCESS.
    let p = messages::parse_server(messages::opcode::SMSG_CHAR_DELETE, &[0x39]).unwrap();
    assert!(matches!(p, ServerPacket::CharDelete { result: 0x39 }));
    assert!(decode(p).is_empty());
}

#[test]
fn server_sound_trio_parse_and_decode() {
    let body = 8595u32.to_le_bytes();
    let p = messages::parse_server(messages::opcode::SMSG_PLAY_SOUND, &body).unwrap();
    assert!(matches!(p, ServerPacket::PlaySound { sound_id: 8595 }));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PlaySound { sound_id: 8595 }]
    ));

    let body = 2523u32.to_le_bytes();
    let p = messages::parse_server(messages::opcode::SMSG_PLAY_MUSIC, &body).unwrap();
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PlayMusic { music_id: 2523 }]
    ));

    // 3355: the fishing-bobber splash (vmangos `GameObject.cpp:373`).
    let mut body = 3355u32.to_le_bytes().to_vec();
    body.extend_from_slice(&0xF110_0000_0000_002Au64.to_le_bytes());
    let p = messages::parse_server(messages::opcode::SMSG_PLAY_OBJECT_SOUND, &body).unwrap();
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PlayObjectSound {
            sound_id: 3355,
            guid: 0xF110_0000_0000_002A,
        }]
    ));
}

/// Body per vmangos `Weather::SendWeatherForPlayersInZone`; 8534 is the RainMedium sound loop.
#[test]
fn weather_parses_and_decodes() {
    let mut body = 1u32.to_le_bytes().to_vec(); // WEATHER_TYPE_RAIN
    body.extend_from_slice(&0.7f32.to_le_bytes());
    body.extend_from_slice(&8534u32.to_le_bytes());
    body.push(0);
    let p = messages::parse_server(messages::opcode::SMSG_WEATHER, &body).unwrap();
    match decode(p).as_slice() {
        [SessionEvent::Weather {
            weather_type,
            grade,
            sound_id,
            instant,
        }] => {
            assert_eq!((*weather_type, *sound_id, *instant), (1, 8534, false));
            assert!((grade - 0.7).abs() < 1e-6);
        }
        other => panic!("weather decode: {} events", other.len()),
    }
}

/// u8 itemClass + u32 subclass bitmask, per vmangos `Skill.cpp` `AppendBodyTo`.
#[test]
fn set_proficiency_parses_and_decodes() {
    assert_eq!(messages::opcode::SMSG_SET_PROFICIENCY, 295);
    // Weapons (class 2), mask 0x2408F: what a fresh warrior gets.
    let body = hx("028f400200");
    let p = messages::parse_server(messages::opcode::SMSG_SET_PROFICIENCY, &body).unwrap();
    match p {
        ServerPacket::SetProficiency {
            item_class,
            subclass_mask,
        } => {
            assert_eq!(item_class, 2);
            assert_eq!(subclass_mask, 0x0002_408f);
        }
        _ => panic!("set proficiency"),
    }
    let p = messages::parse_server(messages::opcode::SMSG_SET_PROFICIENCY, &body).unwrap();
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::Proficiency {
            item_class: 2,
            subclass_mask: 0x0002_408f
        }]
    ));
}

/// u32 count + count x (u32 reputationListId, i32 standing), per vmangos `SetFactionStanding`.
#[test]
fn set_faction_standing_parses_and_decodes() {
    assert_eq!(messages::opcode::SMSG_SET_FACTION_STANDING, 292);
    // Two slots: list 46 -> 3000, list 89 -> -6000 (standing is signed).
    let body = hx("020000002e000000b80b00005900000090e8ffff");
    let p = messages::parse_server(messages::opcode::SMSG_SET_FACTION_STANDING, &body).unwrap();
    match p {
        ServerPacket::SetFactionStanding { ref standings } => {
            assert_eq!(standings[..], [(46, 3000), (89, -6000)]);
        }
        _ => panic!("set faction standing"),
    }
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::ReputationDelta { ref standings }]
            if standings[..] == [(46, 3000), (89, -6000)]
    ));
}

/// An opcode with no parse arm decodes to `PacketDropped`, not silence, so the app can tally it.
#[test]
fn unknown_opcode_decodes_to_packet_dropped() {
    // Sentinel: SMSG_SET_REST_START (0x021E) has no parse arm; swap in another if it gains one.
    let packet = messages::parse_server(0x021E, &hx("0102030405")).unwrap();
    assert!(matches!(packet, ServerPacket::Other { opcode: 0x021E }));
    match decode(packet).as_slice() {
        [SessionEvent::PacketDropped {
            opcode: 0x021E,
            unparseable: false,
        }] => {}
        other => panic!("expected one PacketDropped event, got {other:?}"),
    }
    assert_eq!(messages::opcode_name(0x021E), Some("SMSG_SET_REST_START"));
    assert_eq!(
        messages::opcode_name(messages::opcode::SMSG_UPDATE_OBJECT),
        Some("SMSG_UPDATE_OBJECT")
    );
    assert_eq!(messages::opcode_name(0xFFFF), None);
}

#[test]
fn death_arc_family_parses_and_decodes() {
    use benilla_protocol::messages::opcode;

    // Found: a dungeon corpse reports its entrance's map and coords, so the two maps differ.
    let mut body = vec![1u8];
    body.extend_from_slice(&0i32.to_le_bytes());
    body.extend_from_slice(&(-11209.6f32).to_le_bytes());
    body.extend_from_slice(&1666.54f32.to_le_bytes());
    body.extend_from_slice(&25.0f32.to_le_bytes());
    body.extend_from_slice(&36u32.to_le_bytes());
    let pkt = messages::parse_server(opcode::MSG_CORPSE_QUERY, &body).unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::CorpseQuery {
            found,
            display_map,
            position,
            corpse_map,
        }] => {
            assert!(found);
            assert_eq!(*display_map, 0);
            assert_eq!(*corpse_map, 36);
            assert_eq!(position[2], 25.0);
        }
        other => panic!("corpse query decode: {other:?}"),
    }
    // Not found, also sent unprompted when the corpse turns to bones: a lone u8 0.
    let pkt = messages::parse_server(opcode::MSG_CORPSE_QUERY, &[0u8]).unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::CorpseQuery { found: false, .. }] => {}
        other => panic!("corpse query not-found decode: {other:?}"),
    }

    let pkt = messages::parse_server(opcode::SMSG_CORPSE_RECLAIM_DELAY, &30_000u32.to_le_bytes())
        .unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::CorpseReclaimDelay { delay_ms: 30_000 }] => {}
        other => panic!("reclaim delay decode: {other:?}"),
    }

    // SMSG_RESURRECT_REQUEST, player caster: guid, u32 len 1, empty cstring, sickness 0, timer 1.
    let mut body = 0x2Au64.to_le_bytes().to_vec();
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&[0, 0, 1]);
    let pkt = messages::parse_server(opcode::SMSG_RESURRECT_REQUEST, &body).unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::ResurrectRequest {
            caster: 0x2A,
            name,
            sickness: false,
            has_timer: true,
        }] => assert!(name.is_empty(), "player caster ⇒ empty wire name"),
        other => panic!("resurrect request decode: {other:?}"),
    }

    let pkt = messages::parse_server(
        opcode::SMSG_SPIRIT_HEALER_CONFIRM,
        &0xF130_0000_0000_2AB3u64.to_le_bytes(),
    )
    .unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::SpiritHealerConfirm {
            npc: 0xF130_0000_0000_2AB3,
        }] => {}
        other => panic!("spirit healer confirm decode: {other:?}"),
    }

    let body = hx("012a07000000"); // packed(0x2A) + counter 7
    for (op, mode, apply) in [
        (opcode::SMSG_FORCE_MOVE_ROOT, MoveMode::Root, true),
        (opcode::SMSG_FORCE_MOVE_UNROOT, MoveMode::Root, false),
        (opcode::SMSG_MOVE_WATER_WALK, MoveMode::WaterWalk, true),
        (opcode::SMSG_MOVE_LAND_WALK, MoveMode::WaterWalk, false),
        (opcode::SMSG_MOVE_FEATHER_FALL, MoveMode::FeatherFall, true),
        (opcode::SMSG_MOVE_NORMAL_FALL, MoveMode::FeatherFall, false),
        (opcode::SMSG_MOVE_SET_HOVER, MoveMode::Hover, true),
        (opcode::SMSG_MOVE_UNSET_HOVER, MoveMode::Hover, false),
    ] {
        let pkt = messages::parse_server(op, &body).unwrap();
        match decode(pkt).as_slice() {
            [SessionEvent::MoveMode {
                guid: 0x2A,
                counter: 7,
                mode: got_mode,
                apply: got_apply,
            }] if *got_mode == mode && *got_apply == apply => {}
            other => panic!("move mode decode for opcode {op:#06x}: {other:?}"),
        }
    }

    // The movement-flag bits (vmangos `Objects/MovementInfo.h:28-62`).
    assert_eq!(MoveMode::Root.flag(), 0x0000_1000);
    assert_eq!(MoveMode::WaterWalk.flag(), 0x1000_0000);
    assert_eq!(MoveMode::FeatherFall.flag(), 0x2000_0000);
    assert_eq!(MoveMode::Hover.flag(), 0x4000_0000);

    // Root alone acks per direction, with no apply dword (vmangos `HandleMoveRootAck`).
    assert_eq!(
        MoveMode::Root.ack_opcode(true),
        opcode::CMSG_FORCE_MOVE_ROOT_ACK
    );
    assert_eq!(
        MoveMode::Root.ack_opcode(false),
        opcode::CMSG_FORCE_MOVE_UNROOT_ACK
    );
    assert!(!MoveMode::Root.ack_carries_apply());
    for mode in [MoveMode::WaterWalk, MoveMode::FeatherFall, MoveMode::Hover] {
        assert_eq!(
            mode.ack_opcode(true),
            mode.ack_opcode(false),
            "{mode:?}: one ack opcode both ways"
        );
        assert!(mode.ack_carries_apply(), "{mode:?}: ack carries apply");
    }
}

/// Root's ack lacks the trailing u32 apply the other modes carry (vmangos `Movement.cpp:38-59`).
#[test]
fn move_flag_ack_bodies_differ_by_the_apply_tail() {
    use benilla_protocol::messages::{move_flag_ack, MovementInfo};
    use benilla_protocol::wire::Vector3d;

    let info = MovementInfo {
        flags: 0,
        timestamp: 0x1122_3344,
        position: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
        transport: None,
        pitch: 0.0,
        fall_time: 0,
        jump: None,
    };
    let root = move_flag_ack(0x2A, 7, &info, None);
    let walk = move_flag_ack(0x2A, 7, &info, Some(true));
    assert_eq!(root.len() + 4, walk.len());
    assert_eq!(&walk[..root.len()], root.as_slice());
    assert_eq!(&walk[root.len()..], 1u32.to_le_bytes());
    // The ack's guid is full, not packed.
    assert_eq!(&root[..8], &0x2Au64.to_le_bytes());
    assert_eq!(&root[8..12], &7u32.to_le_bytes());
}

/// `SMSG_DURABILITY_DAMAGE_DEATH` (0x2BD): empty body; the cue for the 10% durability-loss line.
#[test]
fn durability_damage_death_is_an_empty_body_cue() {
    let pkt = messages::parse_server(messages::opcode::SMSG_DURABILITY_DAMAGE_DEATH, &[]).unwrap();
    assert!(matches!(pkt, ServerPacket::DurabilityDamageDeath));
    assert!(matches!(
        decode(pkt).as_slice(),
        [SessionEvent::DurabilityDamageDeath]
    ));
}

/// u32 mapId, then u32 transportEntry + u32 oldMapId when the player rides a transport through
/// (vmangos `Misc.cpp:493-501`).
#[test]
fn transfer_pending_both_shapes_and_abort_parse_and_decode() {
    let plain =
        messages::parse_server(messages::opcode::SMSG_TRANSFER_PENDING, &hx("01000000")).unwrap();
    assert!(matches!(
        plain,
        ServerPacket::TransferPending {
            map: 1,
            transport: None
        }
    ));
    assert!(matches!(
        decode(plain).as_slice(),
        [SessionEvent::TransferPending {
            map_id: 1,
            transport_entry: None
        }]
    ));
    // Riding: 176310 is the Menethil boat, arriving in EK (0) from Kalimdor (1).
    let mut riding = 0u32.to_le_bytes().to_vec();
    riding.extend_from_slice(&176310u32.to_le_bytes());
    riding.extend_from_slice(&1u32.to_le_bytes());
    let riding = messages::parse_server(messages::opcode::SMSG_TRANSFER_PENDING, &riding).unwrap();
    assert!(matches!(
        riding,
        ServerPacket::TransferPending {
            map: 0,
            transport: Some((176310, 1))
        }
    ));
    assert!(matches!(
        decode(riding).as_slice(),
        [SessionEvent::TransferPending {
            map_id: 0,
            transport_entry: Some(176310)
        }]
    ));
    let abort = messages::parse_server(messages::opcode::SMSG_TRANSFER_ABORTED, &[2]).unwrap();
    assert!(matches!(abort, ServerPacket::TransferAborted { reason: 2 }));
    assert!(matches!(
        decode(abort).as_slice(),
        [SessionEvent::TransferAborted { reason: 2 }]
    ));
}

/// Packed mover guid then u8 allowMove (vmangos `Server/Packets/Misc.cpp:677-682`).
#[test]
fn client_control_update_reads_a_packed_mover_and_the_allow_byte() {
    // Grant: creature 0xF130000C1A00A2B4, allowMove 1. Mask 0xdb marks its non-zero LE bytes
    // (b4 a2 00 1a 0c 00 30 f1 → indices 0,1,3,4,6,7), which follow in order.
    let grant = hx("dbb4a21a0c30f101");
    match messages::parse_server(messages::opcode::SMSG_CLIENT_CONTROL_UPDATE, &grant).unwrap() {
        ServerPacket::ClientControlUpdate { mover, allow_move } => {
            assert_eq!(mover, 0xF130_000C_1A00_A2B4);
            assert!(allow_move);
        }
        other => panic!("client control update, got {}", other.name()),
    }
    // Revoke: our own guid 0x45, allowMove 0; vmangos never roots a mind-controlled player.
    let revoke = hx("014500");
    let revoke = messages::parse_server(messages::opcode::SMSG_CLIENT_CONTROL_UPDATE, &revoke)
        .expect("revoke parses");
    assert!(matches!(
        revoke,
        ServerPacket::ClientControlUpdate {
            mover: 0x45,
            allow_move: false
        }
    ));
    assert!(matches!(
        decode(revoke).as_slice(),
        [SessionEvent::ClientControl {
            mover: 0x45,
            allow_move: false
        }]
    ));
    // A zero guid packs to a lone zero mask byte, so the very next byte is allowMove.
    let empty = messages::parse_server(messages::opcode::SMSG_CLIENT_CONTROL_UPDATE, &hx("0001"))
        .expect("empty guid parses");
    assert!(matches!(
        empty,
        ServerPacket::ClientControlUpdate {
            mover: 0,
            allow_move: true
        }
    ));
}

#[test]
fn pet_feedback_family_parses_and_decodes() {
    use benilla_protocol::messages::opcode;

    // 9 = PETTAME_TOOHIGHLEVEL.
    let p = messages::parse_server(opcode::SMSG_PET_TAME_FAILURE, &hx("09")).unwrap();
    assert!(matches!(p, ServerPacket::PetTameFailure { reason: 9 }));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PetTameFailure { reason: 9 }]
    ));
    // Reason to GlobalStrings key: the reference bounds `reason - 1` at 0xa (`0x6e6a20`).
    assert_eq!(messages::pet_tame_failure_key(1), "PETTAME_INVALIDCREATURE");
    assert_eq!(
        messages::pet_tame_failure_key(5),
        "PETTAME_ANOTHERSUMMONACTIVE"
    );
    assert_eq!(messages::pet_tame_failure_key(11), "PETTAME_NOTDEAD");
    // Out of range on either side, vmangos's 12th value included, falls to the default arm.
    assert_eq!(messages::pet_tame_failure_key(0), "PETTAME_UNKNOWNERROR");
    assert_eq!(messages::pet_tame_failure_key(12), "PETTAME_UNKNOWNERROR");
    assert_eq!(messages::pet_tame_failure_key(200), "PETTAME_UNKNOWNERROR");

    // The reference reads no body here (vmangos writes none), so stray bytes change nothing.
    for body in ["", "deadbeef"] {
        let p = messages::parse_server(opcode::SMSG_PET_NAME_INVALID, &hx(body)).unwrap();
        assert!(matches!(p, ServerPacket::PetNameInvalid), "body {body:?}");
        assert!(matches!(decode(p)[..], [SessionEvent::PetNameInvalid]));

        let p = messages::parse_server(opcode::SMSG_PET_BROKEN, &hx(body)).unwrap();
        assert!(matches!(p, ServerPacket::PetBroken), "body {body:?}");
        assert!(matches!(decode(p)[..], [SessionEvent::PetBroken]));
    }

    // SMSG_PET_ACTION_SOUND: u64 guid then the u32 talk selector.
    let body = hx("2a0000000000401001000000");
    let p = messages::parse_server(opcode::SMSG_PET_ACTION_SOUND, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::PetActionSound {
            pet_guid: 0x1040_0000_0000_002A,
            talk: messages::PET_TALK_ATTACK,
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PetActionSound {
            pet_guid: 0x1040_0000_0000_002A,
            talk: 1
        }]
    ));

    // SMSG_PET_DISMISS_SOUND: u32 CreatureModelData id then raw WoW x/y/z (the human start).
    // The reference handler's +1.0 on z belongs to playback, not to the decode.
    let mut body = hx("d6020000"); // model 726
    for f in [-8949.95_f32, -132.493, 83.5312] {
        body.extend_from_slice(&f.to_le_bytes());
    }
    let p = messages::parse_server(opcode::SMSG_PET_DISMISS_SOUND, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::PetDismissSound {
            model_id: 726,
            position: benilla_protocol::wire::Vector3d {
                x: -8949.95,
                y: -132.493,
                z: 83.5312,
            },
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PetDismissSound {
            model_id: 726,
            position: [-8949.95, -132.493, 83.5312],
        }]
    ));
}
