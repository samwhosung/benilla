//! `SMSG_UPDATE_OBJECT` and `SMSG_DESTROY_OBJECT` wire tests (`src/messages/update_object.rs`).

mod common;

use benilla_protocol::events::{decode, EntityKind, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::wire::write_packed_guid;
use common::hx;

#[test]
fn update_object_fixture_decodes() {
    // A unit create (0xAA), a gameobject create (0xBB, a HAS_POSITION block at (100, 200, 300), as
    // vmangos sends one) and an out-of-range block [0x10, 0x20].
    let body = hx("03000000000301aa03200000000000000000cdd70bc6357e04c3f90fa74200000040000000000000803f0000e040000090400000000000000000db0f4940051700000000000000000000000000000008000000aa00000000000000090000000000c03f320000000301bb05400000c84200004843000096430000803f0117810700bb000000000000002100000000000040630000000000c84200004843000096430000803f040200000001100120");
    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let events = decode(packet);

    let unit = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::ObjectCreate {
                guid: 0xAA,
                kind,
                display_id,
                position,
                scale,
                speeds,
                ..
            } => Some((*kind, *display_id, *position, *scale, *speeds)),
            _ => None,
        })
        .expect("unit create");
    assert_eq!(unit.0, EntityKind::Unit);
    assert_eq!(unit.1, Some(50));
    assert_eq!(unit.2, [-8949.95, -132.493, 83.5312]);
    assert_eq!(unit.3, 1.5);
    assert_eq!(unit.4.map(|s| s.walk), Some(1.0)); // speeds[0] = walk, from the LIVING block

    let go = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::ObjectCreate {
                guid: 0xBB,
                kind,
                display_id,
                position,
                scale,
                speeds,
                ..
            } => Some((*kind, *display_id, *position, *scale, *speeds)),
            _ => None,
        })
        .expect("gameobject create");
    assert_eq!(go.0, EntityKind::GameObject);
    assert_eq!(go.1, Some(99));
    assert_eq!(go.2, [100.0, 200.0, 300.0]);
    assert_eq!(go.3, 2.0);
    assert_eq!(go.4, None); // GameObject HAS_POSITION block carries no speeds

    let removed = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::ObjectsRemoved(g) => Some(g.clone()),
            _ => None,
        })
        .expect("out-of-range");
    assert_eq!(removed, vec![0x10, 0x20]);
}

#[test]
fn destroy_object_decodes_to_object_destroyed() {
    // SMSG_DESTROY_OBJECT: a raw u64 guid (vmangos `DestroyObject::AppendBodyTo`).
    let body = hx("efbeadde01000000");
    let packet = messages::parse_server(messages::opcode::SMSG_DESTROY_OBJECT, &body).unwrap();
    assert_eq!(packet.name(), "SMSG_DESTROY_OBJECT");
    let destroyed = decode(packet)
        .into_iter()
        .find_map(|e| match e {
            SessionEvent::ObjectDestroyed(guid) => Some(guid),
            _ => None,
        })
        .expect("destroy");
    assert_eq!(destroyed, 0x1_DEAD_BEEF);
}

#[test]
fn values_update_decodes_unit_state() {
    // Values update for 0xAA: fields 22 HEALTH 100, 28 MAXHEALTH 120, 34 LEVEL 5 and 36 BYTES_0
    // (race 1, class 1, gender 0), in 2 mask blocks with values in ascending field order.
    let body = hx("01000000000001aa02000040101400000064000000780000000500000001010001");
    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let fields = decode(packet)
        .into_iter()
        .find_map(|e| match e {
            SessionEvent::ObjectValues { guid: 0xAA, fields } => Some(fields),
            _ => None,
        })
        .expect("object values");
    assert_eq!(
        (
            fields.unit_health(),
            fields.unit_max_health(),
            fields.unit_level(),
            fields.unit_race(),
            fields.unit_class(),
            fields.unit_gender(),
        ),
        (Some(100), Some(120), Some(5), Some(1), Some(1), Some(0))
    );
    assert_eq!(fields.player_skin(), None);
    assert_eq!(fields.player_facial_hair(), None);
}

#[test]
fn values_update_decodes_player_customization() {
    // Player 0xBB: PLAYER_BYTES (193) = skin, face, hairStyle, hairColor in b0..b3; PLAYER_BYTES_2
    // (194) b0 = facialHair. Both sit in mask block 6 (bits 1, 2), after six empty blocks.
    let body = hx(
        "01000000000001bb07000000000000000000000000000000000000000000000000060000001011121314000000",
    );
    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let fields = decode(packet)
        .into_iter()
        .find_map(|e| match e {
            SessionEvent::ObjectValues { guid: 0xBB, fields } => Some(fields),
            _ => None,
        })
        .expect("player object values");
    assert_eq!(
        (
            fields.player_skin(),
            fields.player_face(),
            fields.player_hair_style(),
            fields.player_hair_color(),
            fields.player_facial_hair(),
        ),
        (Some(0x10), Some(0x11), Some(0x12), Some(0x13), Some(0x14))
    );
}

#[test]
fn values_update_decodes_unit_power() {
    // Fields 23 POWER1 = 40, 29 MAXPOWER1 = 60, 36 BYTES_0 with power type 0 (mana) in b3 (vmangos
    // `UpdateFields_1_12_1.h`).
    let body = hx("01000000000001aa020000802010000000280000003c00000001010000");
    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let fields = decode(packet)
        .into_iter()
        .find_map(|e| match e {
            SessionEvent::ObjectValues { guid: 0xAA, fields } => Some(fields),
            _ => None,
        })
        .expect("object values");
    assert_eq!(fields.unit_power_type(), 0);
    assert_eq!(fields.unit_power(0), Some(40));
    assert_eq!(fields.unit_max_power(0), Some(60));
    assert_eq!(fields.unit_power(1), None);
    assert_eq!(fields.unit_max_power(5), None);
}

#[test]
fn values_update_decodes_unit_virtual_items_and_sheath() {
    // Fields 37 (virtual item display 0), 40/41 (its info pair, sheath 3 in 41's b0) and 164
    // (UNIT_FIELD_BYTES_2, b0 = sheath state 1, melee drawn).
    let mut body = hx("01000000000001cc06"); // count 1, no transport, VALUES, guid 0xCC, 6 blocks
    let mut masks = [0u32; 6];
    masks[1] = (1 << 5) | (1 << 8) | (1 << 9);
    masks[5] = 1 << 4;
    for m in masks {
        body.extend_from_slice(&m.to_le_bytes());
    }
    let info_dword0 = 2u32 | (7 << 8) | (1 << 16) | (21 << 24); // class/subclass/material/invType
    for v in [1234u32, info_dword0, 3u32, 1u32] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let fields = decode(packet)
        .into_iter()
        .find_map(|e| match e {
            SessionEvent::ObjectValues { guid: 0xCC, fields } => Some(fields),
            _ => None,
        })
        .expect("object values");

    assert_eq!(fields.unit_virtual_item_display(0), Some(1234));
    assert_eq!(fields.unit_virtual_item_display(1), None, "slot 1 unsent");
    assert_eq!(
        fields.unit_virtual_item_display(3),
        None,
        "slot out of range"
    );
    assert_eq!(fields.unit_virtual_item_info(0), Some((2, 7, 1, 21)));
    assert_eq!(fields.unit_virtual_item_info(3), None, "slot out of range");
    assert_eq!(fields.unit_virtual_item_sheath(0), Some(3));
    assert_eq!(
        fields.unit_virtual_item_sheath(3),
        None,
        "slot out of range"
    );
    assert_eq!(fields.unit_sheath_state(), Some(1));
}

#[test]
fn values_update_decodes_inventory_slots() {
    // INV_SLOT 16, the offhand (fields 518/519) = 0xAB; PACK_SLOT 0 (532/533) = 0xCD. The bases
    // (INV_SLOT_HEAD 486, PACK 532) follow vmangos's enum arithmetic, not its stale hex comments.
    let mut body = hx("010000000000010111"); // count 1, no transport, VALUES, guid 1, 17 blocks
    for block in 0..17u32 {
        let mask: u32 = if block == 16 { 0x0030_00C0 } else { 0 };
        body.extend_from_slice(&mask.to_le_bytes());
    }
    for v in [0xABu32, 0, 0xCD, 0] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let fields = decode(packet)
        .into_iter()
        .find_map(|e| match e {
            SessionEvent::ObjectValues { guid: 1, fields } => Some(fields),
            _ => None,
        })
        .expect("values event");
    assert_eq!(fields.player_inv_slot(16), Some(0xAB));
    assert_eq!(fields.player_pack_slot(0), Some(0xCD));
    assert_eq!(fields.player_pack_slot(3), None, "unsent slot reads None");
    assert_eq!(fields.player_bank_slot(0), None);
    assert_eq!(fields.container_slot(36), None, "out of range");
}

#[test]
fn item_and_container_creates_decode() {
    // Item and bag creates as vmangos streams them at login (movement block 0x10 + u32 1). Bases
    // follow vmangos's enum arithmetic, not its stale hex comments: NUM_SLOTS 48, SLOT_1 50.
    let mut body = hx("0200000000");
    // Item guid 0x42, entry 4660 (field 3), stack count 5 (field 14): 1 mask block, bits 3+14.
    body.extend_from_slice(&hx("0201420110010000000108400000"));
    body.extend_from_slice(&4660u32.to_le_bytes());
    body.extend_from_slice(&5u32.to_le_bytes());
    // Container guid 0x43, entry 828 (field 3), num-slots 6 (field 48), slot-0 guid 0x42
    // (fields 50/51): 2 mask blocks, block0 bit 3, block1 bits 16/18/19.
    body.extend_from_slice(&hx("020143021001000000020800000000000d00"));
    for v in [828u32, 6, 0x42, 0] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let events = decode(packet);
    assert_eq!(events.len(), 3, "both creates decode, nothing spatial");
    assert!(matches!(events[2], SessionEvent::ObjectUpdateEnd));
    match &events[0] {
        SessionEvent::ItemCreate {
            guid: 0x42,
            container: false,
            fields,
        } => {
            assert_eq!(fields.object_entry(), Some(4660));
            assert_eq!(fields.item_stack_count(), Some(5));
        }
        other => panic!("item create, got {other:?}"),
    }
    match &events[1] {
        SessionEvent::ItemCreate {
            guid: 0x43,
            container: true,
            fields,
        } => {
            assert_eq!(fields.object_entry(), Some(828));
            assert_eq!(fields.container_num_slots(), Some(6));
            assert_eq!(fields.container_slot(0), Some(0x42));
            assert_eq!(fields.container_slot(1), None, "unsent slot reads None");
        }
        other => panic!("container create, got {other:?}"),
    }
}

/// A boat's create: `HAS_POSITION` (0x40) + `TRANSPORT` (0x02), whose u32 tail is pathProgress ms
/// (vmangos `Object.cpp:590-605`); vmangos sends no `GAMEOBJECT_POS_*`, only the movement pose.
#[test]
fn gameobject_create_surfaces_transport_progress_and_position_fallback() {
    // HIGH_MO_TRANSPORT (0x1FC0) guids hold the entry in the full low 32 bits; 176495 is the
    // Grom'Gol-Undercity zeppelin.
    let guid: u64 = 176_495 | (0x1FC0u64 << 48);

    let mut body = 1u32.to_le_bytes().to_vec(); // amount_of_objects
    body.push(0); // has_transport (unused by the parser)
    body.push(2); // update_type: CREATE_OBJECT
    write_packed_guid(guid, &mut body).unwrap();
    body.push(5); // TypeId::GameObject

    body.push(0x40 | 0x02);
    body.extend_from_slice(&5000.0f32.to_le_bytes()); // stationary pos.x
    body.extend_from_slice(&6000.0f32.to_le_bytes()); // stationary pos.y
    body.extend_from_slice(&15.0f32.to_le_bytes()); // stationary pos.z
    body.extend_from_slice(&2.0f32.to_le_bytes()); // orientation
    body.extend_from_slice(&123_456u32.to_le_bytes()); // TRANSPORT tail: pathProgress ms

    // Mask: 1 block, GAMEOBJECT_DISPLAYID (field 8) only.
    body.push(1);
    body.extend_from_slice(&(1u32 << 8).to_le_bytes());
    body.extend_from_slice(&3015u32.to_le_bytes()); // transportship.wmo's displayId

    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let events = decode(packet);
    match events.as_slice() {
        [SessionEvent::ObjectCreate {
            guid: g,
            kind,
            display_id,
            position,
            orientation,
            speeds,
            transport_progress,
            transport,
            ..
        }, SessionEvent::ObjectUpdateEnd] => {
            assert_eq!(*g, guid);
            assert_eq!(*kind, EntityKind::GameObject);
            assert_eq!(*display_id, Some(3015));
            assert_eq!(
                *position,
                [5000.0, 6000.0, 15.0],
                "falls back to the movement block's HAS_POSITION pose — GAMEOBJECT_POS_* is absent"
            );
            assert_eq!(*orientation, 2.0);
            assert_eq!(
                *speeds, None,
                "a GameObject's HAS_POSITION block carries no speeds"
            );
            assert_eq!(
                *transport_progress,
                Some(123_456),
                "UPDATE_FLAG_TRANSPORT's u32 tail is the pathProgress ms anchor"
            );
            assert_eq!(
                *transport, None,
                "a HAS_POSITION block (not LIVING) carries no ON_TRANSPORT rider tail"
            );
        }
        other => panic!("expected one ObjectCreate, got {other:?}"),
    }
}

/// `MOVEFLAG_ON_TRANSPORT` (0x0200_0000) adds a rider tail to the `LIVING` block: u64 transport
/// guid, then local x, y, z and orientation.
#[test]
fn unit_living_block_surfaces_on_transport_rider_pose() {
    // 176244 is the Moonspray, the Auberdine-Rut'theran boat.
    let transport_guid: u64 = 176_244 | (0x1FC0u64 << 48);

    let mut body = 1u32.to_le_bytes().to_vec();
    body.push(0); // has_transport
    body.push(2); // update_type: CREATE_OBJECT
    write_packed_guid(0xAA, &mut body).unwrap();
    body.push(3); // TypeId::Unit

    body.push(0x20); // movement block: LIVING only
    let flags: u32 = 0x0200_0000; // MOVEFLAG_ON_TRANSPORT (1.12 bit 25, vmangos MovementInfo.h)
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // timestamp
    body.extend_from_slice(&100.0f32.to_le_bytes()); // living pos.x (world, not local)
    body.extend_from_slice(&200.0f32.to_le_bytes()); // living pos.y
    body.extend_from_slice(&5.0f32.to_le_bytes()); // living pos.z
    body.extend_from_slice(&1.0f32.to_le_bytes()); // living orientation
    body.extend_from_slice(&transport_guid.to_le_bytes()); // ON_TRANSPORT tail: full u64 guid
    body.extend_from_slice(&2.0f32.to_le_bytes()); // local x
    body.extend_from_slice(&3.0f32.to_le_bytes()); // local y
    body.extend_from_slice(&0.5f32.to_le_bytes()); // local z
    body.extend_from_slice(&0.25f32.to_le_bytes()); // local o
    body.extend_from_slice(&0.0f32.to_le_bytes()); // fall_time (no swim or jump tails)
    for v in [2.5f32, 7.0, 4.5, 4.722_222_3, 2.5, std::f32::consts::PI] {
        body.extend_from_slice(&v.to_le_bytes()); // the 6 speeds, right after the tail
    }

    body.push(0); // mask: 0 blocks (nothing to interpret beyond the pose)

    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let events = decode(packet);
    match events.as_slice() {
        [SessionEvent::ObjectCreate {
            guid: 0xAA,
            kind: EntityKind::Unit,
            position,
            orientation,
            speeds,
            transport_progress,
            transport,
            ..
        }, SessionEvent::ObjectUpdateEnd] => {
            assert_eq!(*position, [100.0, 200.0, 5.0], "the unit's own world pose");
            assert_eq!(*orientation, 1.0);
            assert_eq!(
                transport,
                &Some(benilla_protocol::messages::TransportPose {
                    guid: transport_guid,
                    pos: benilla_protocol::wire::Vector3d {
                        x: 2.0,
                        y: 3.0,
                        z: 0.5,
                    },
                    orientation: 0.25,
                }),
                "the ON_TRANSPORT tail surfaces as the rider's local pose"
            );
            assert_eq!(
                transport_progress, &None,
                "no UPDATE_FLAG_TRANSPORT on a rider create"
            );
            assert_eq!(
                speeds.map(|s| s.walk),
                Some(2.5),
                "the 6 speeds still parse right after the transport tail"
            );
        }
        other => panic!("expected one ObjectCreate, got {other:?}"),
    }
}

/// A swimming create's flags word and swim pitch both reach [`SessionEvent::ObjectCreate::mover`];
/// the reference re-authors the mover's live flags from this block (the `0x75a07dff` merge).
#[test]
fn unit_living_block_surfaces_the_swim_it_is_already_in() {
    let mut body = 1u32.to_le_bytes().to_vec();
    body.push(0); // has_transport
    body.push(2); // update_type: CREATE_OBJECT
    write_packed_guid(0xAA, &mut body).unwrap();
    body.push(4); // TypeId::Player

    body.push(0x20); // movement block: LIVING only
    let flags: u32 = 0x20_0000 | 0x1; // MOVEFLAG_SWIMMING | MOVEFLAG_FORWARD
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // timestamp
    body.extend_from_slice(&(-812.5f32).to_le_bytes()); // living pos.x
    body.extend_from_slice(&(-566.0f32).to_le_bytes()); // living pos.y
    body.extend_from_slice(&(-3.25f32).to_le_bytes()); // living pos.z (under the surface)
    body.extend_from_slice(&2.0f32.to_le_bytes()); // living orientation
    body.extend_from_slice(&(-0.4f32).to_le_bytes()); // the SWIMMING tail: pitch, nose down
    body.extend_from_slice(&0.0f32.to_le_bytes()); // fall_time
    for v in [2.5f32, 7.0, 4.5, 4.722_222_3, 2.5, std::f32::consts::PI] {
        body.extend_from_slice(&v.to_le_bytes()); // the 6 speeds, right after the tail
    }

    body.push(0); // mask: 0 blocks

    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    match decode(packet).as_slice() {
        [SessionEvent::ObjectCreate {
            guid: 0xAA,
            kind: EntityKind::Player,
            orientation,
            mover,
            speeds,
            ..
        }, SessionEvent::ObjectUpdateEnd] => {
            assert_eq!(*orientation, 2.0);
            assert_eq!(
                mover,
                &Some(benilla_protocol::MoverState { flags, pitch: -0.4 }),
                "the live flags word and its swim-pitch tail both reach the app"
            );
            assert_eq!(
                speeds.map(|s| s.walk),
                Some(2.5),
                "the 6 speeds still parse right after the swim-pitch tail"
            );
        }
        other => panic!("expected one ObjectCreate, got {other:?}"),
    }
}

/// Without SWIMMING there is no pitch tail; the mover still surfaces, with a level 0.0 pitch.
#[test]
fn a_dry_living_block_has_no_pitch_tail_and_reads_level() {
    let mut body = 1u32.to_le_bytes().to_vec();
    body.push(0);
    body.push(2);
    write_packed_guid(0xAB, &mut body).unwrap();
    body.push(3); // TypeId::Unit

    body.push(0x20); // LIVING
    let flags: u32 = 0x1; // FORWARD, walking on dry land
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // timestamp
    for v in [10.0f32, 20.0, 30.0, 0.5] {
        body.extend_from_slice(&v.to_le_bytes()); // pos + orientation
    }
    body.extend_from_slice(&0.0f32.to_le_bytes()); // fall_time, no pitch tail before it
    for v in [2.5f32, 7.0, 4.5, 4.722_222_3, 2.5, std::f32::consts::PI] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    body.push(0);

    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    match decode(packet).as_slice() {
        [SessionEvent::ObjectCreate { mover, speeds, .. }, SessionEvent::ObjectUpdateEnd] => {
            assert_eq!(
                mover,
                &Some(benilla_protocol::MoverState { flags, pitch: 0.0 }),
                "no tail on the wire reads as a level pitch, not a misparse"
            );
            assert_eq!(
                speeds.map(|s| s.walk),
                Some(2.5),
                "the speeds land right after fall_time — the tail really was absent"
            );
        }
        other => panic!("expected one ObjectCreate, got {other:?}"),
    }
}

/// `MOVEFLAG_SPLINE_ENABLED` (0x0040_0000) adds a spline tail (vmangos `packet_builder.cpp:152`).
/// Its nodes are the internal control array `[phantom, p₀…pₙ, tail]`, since vmangos builds even a
/// linear spline through `InitCatmullRom` (`spline.cpp:52`); the decoded path drops the two ends.
#[test]
fn unit_living_block_surfaces_the_walk_it_is_already_on() {
    let path = [
        [10.0f32, 20.0, 30.0],
        [20.0, 20.0, 30.0],
        [20.0, 40.0, 30.0],
    ];
    let nodes = [
        [0.0f32, 20.0, 30.0], // phantom: 2·p₀ − p₁
        path[0],
        path[1],
        path[2],
        path[2], // tail: the destination again
    ];

    let mut body = 1u32.to_le_bytes().to_vec();
    body.push(0); // has_transport
    body.push(2); // update_type: CREATE_OBJECT
    write_packed_guid(0xAA, &mut body).unwrap();
    body.push(3); // TypeId::Unit

    body.push(0x20); // movement block: LIVING only
    let flags: u32 = 0x0040_0000; // MOVEFLAG_SPLINE_ENABLED (vmangos MovementInfo.h:53)
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // timestamp
    for v in [14.0f32, 20.0, 30.0, 0.0] {
        body.extend_from_slice(&v.to_le_bytes()); // stored pose, synced to the spline every 400 ms
    }
    body.extend_from_slice(&0.0f32.to_le_bytes()); // fall_time
    for v in [2.5f32, 7.0, 4.5, 4.722_222_3, 2.5, std::f32::consts::PI] {
        body.extend_from_slice(&v.to_le_bytes()); // the 6 speeds
    }
    // The spline tail: flags, time_passed, duration, id, node count, the nodes, final destination.
    body.extend_from_slice(&0u32.to_le_bytes()); // spline flags: ground walk, no dictated facing
    body.extend_from_slice(&3_000u32.to_le_bytes()); // time_passed: a quarter of the ride is done
    body.extend_from_slice(&12_000u32.to_le_bytes()); // duration
    body.extend_from_slice(&42u32.to_le_bytes()); // spline id
    body.extend_from_slice(&(nodes.len() as u32).to_le_bytes());
    for n in nodes {
        for f in n {
            body.extend_from_slice(&f.to_le_bytes());
        }
    }
    for f in path[2] {
        body.extend_from_slice(&f.to_le_bytes()); // FinalDestination
    }

    // A descriptor field behind the spline: UNIT_FIELD_DISPLAYID (131) = block 4, bit 3.
    body.push(5); // 5 mask blocks
    for m in [0u32, 0, 0, 0, 1 << 3] {
        body.extend_from_slice(&m.to_le_bytes());
    }
    body.extend_from_slice(&1234u32.to_le_bytes());

    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let events = decode(packet);
    match events.as_slice() {
        [SessionEvent::ObjectCreate {
            guid: 0xAA,
            kind: EntityKind::Unit,
            display_id,
            position,
            spline,
            ..
        }, SessionEvent::ObjectUpdateEnd] => {
            let s = spline.as_ref().expect("the create block's live spline");
            assert_eq!(
                s.path, path,
                "the two virtual control points are the server's, not waypoints"
            );
            assert_eq!(s.time_passed_ms, 3_000);
            assert_eq!(s.duration_ms, 12_000);
            assert_eq!(s.id, 42);
            assert!(!s.flying, "no Flying bit ⇒ a ground walk");
            assert!(!s.cyclic);
            assert_eq!(
                *position,
                [14.0, 20.0, 30.0],
                "the block's own pose is the server's last 400 ms sync, kept as the spawn pose"
            );
            assert_eq!(
                *display_id,
                Some(1234),
                "the descriptor mask behind the spline still parses — the block was sized right"
            );
        }
        other => panic!("expected one ObjectCreate, got {other:?}"),
    }
}

/// The GameObject constructor copies the create block's position over `GAMEOBJECT_POS_X`..`FACING`
/// (`0x5f7046`..`0x5f706b`), so a create whose descriptor and block disagree is placed by the block.
#[test]
fn gameobject_create_is_placed_by_the_block_over_its_position_fields() {
    let mut body = 1u32.to_le_bytes().to_vec();
    body.push(0); // has_transport
    body.push(2); // CREATE_OBJECT
    write_packed_guid(0xBB, &mut body).unwrap();
    body.push(5); // TypeId::GameObject
    body.push(0x40); // HAS_POSITION
    for v in [5000.0f32, 6000.0, 15.0, 2.0] {
        body.extend_from_slice(&v.to_le_bytes()); // the block's pose
    }
    // Fields 8 (GAMEOBJECT_DISPLAYID) and 15..=18 (GAMEOBJECT_POS_X, _Y, _Z, FACING) in block 0.
    body.push(1);
    body.extend_from_slice(&((1u32 << 8) | (0xF << 15)).to_le_bytes());
    body.extend_from_slice(&3015u32.to_le_bytes());
    for v in [100.0f32, 200.0, 300.0, 1.0] {
        body.extend_from_slice(&v.to_le_bytes()); // the descriptor's pose
    }

    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    match decode(packet).as_slice() {
        [SessionEvent::ObjectCreate {
            position,
            orientation,
            display_id,
            ..
        }, SessionEvent::ObjectUpdateEnd] => {
            assert_eq!(*display_id, Some(3015), "control: the fields were sent");
            assert_eq!(*position, [5000.0, 6000.0, 15.0], "the block wins");
            assert_eq!(*orientation, 2.0);
        }
        other => panic!("expected one ObjectCreate, got {other:?}"),
    }
}

/// The create spline's final-facing bits are tested one by one, each field read in this order when
/// its bit is set: point (`0x10000`, 12 bytes, `0x47f0bf`), guid (`0x20000`, 8, `0x47f0e8`), angle
/// (`0x40000`, 4, `0x47f0f9`). The id, timing and path behind them land only if every set bit took
/// its bytes. Flags `0x50000` is the golden: the point (1, 2, 3), then the angle 1.5.
#[test]
fn create_spline_reads_each_final_facing_field_its_bit_names() {
    // A unit create with a LIVING block on a spline: head and move flags, pose, speeds, then
    // `spline flags`, the facing fields, timing (3000, 12000, id 42, 5 nodes), the 5 nodes and the
    // final destination, and a descriptor behind it (UNIT_FIELD_DISPLAYID 1234).
    fn create(spline_flags: &str, facing: &str) -> Vec<u8> {
        hx(&[
            concat!("01000000", "00", "02", "01aa", "03", "20"),
            concat!("00004000", "00000000"),
            concat!("000060410000a0410000f04100000000", "00000000"),
            "000020400000e04000009040721c974000002040db0f4940",
            spline_flags,
            facing,
            "b80b0000e02e00002a00000005000000",
            concat!("000000000000a0410000f041", "000020410000a0410000f041"),
            concat!("0000a0410000a0410000f041", "0000a041000020420000f041"),
            concat!("0000a041000020420000f041", "0000a041000020420000f041"),
            "050000000000000000000000000000000008000000d2040000",
        ]
        .concat())
    }
    let point = "0000803f0000004000004040";
    let guid = "1122334455667788";
    let angle = "0000c03f";
    for (name, flags, facing) in [
        ("point and angle", "00000500", [point, angle].concat()),
        ("point and guid", "00000300", [point, guid].concat()),
        ("guid and angle", "00000600", [guid, angle].concat()),
        ("all three", "00000700", [point, guid, angle].concat()),
        ("the point alone", "00000100", point.to_string()),
        ("the guid alone", "00000200", guid.to_string()),
        ("the angle alone", "00000400", angle.to_string()),
        ("none", "00000000", String::new()),
    ] {
        let packet = messages::parse_server(
            messages::opcode::SMSG_UPDATE_OBJECT,
            &create(flags, &facing),
        )
        .unwrap();
        match decode(packet).as_slice() {
            [SessionEvent::ObjectCreate {
                guid: 0xAA,
                display_id,
                spline,
                ..
            }, SessionEvent::ObjectUpdateEnd] => {
                let s = spline.as_ref().expect("the spline");
                assert_eq!(s.id, 42, "{name}: the id is read where the facing ends");
                assert_eq!(s.time_passed_ms, 3_000, "{name}");
                assert_eq!(s.duration_ms, 12_000, "{name}");
                assert_eq!(s.path.len(), 3, "{name}");
                assert_eq!(
                    *display_id,
                    Some(1234),
                    "{name}: the fields behind it parse"
                );
            }
            other => panic!("{name}: expected one ObjectCreate, got {other:?}"),
        }
    }
}

#[test]
fn unit_living_block_without_the_spline_flag_carries_no_walk() {
    let mut body = 1u32.to_le_bytes().to_vec();
    body.push(0); // has_transport
    body.push(2); // CREATE_OBJECT
    write_packed_guid(0xAB, &mut body).unwrap();
    body.push(3); // TypeId::Unit
    body.push(0x20); // LIVING
    body.extend_from_slice(&0u32.to_le_bytes()); // no move flags
    body.extend_from_slice(&0u32.to_le_bytes()); // timestamp
    for v in [1.0f32, 2.0, 3.0, 0.5] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    body.extend_from_slice(&0.0f32.to_le_bytes()); // fall_time
    for v in [2.5f32, 7.0, 4.5, 4.722_222_3, 2.5, std::f32::consts::PI] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    body.push(0); // no descriptor fields

    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    match decode(packet).as_slice() {
        [SessionEvent::ObjectCreate { spline, .. }, SessionEvent::ObjectUpdateEnd] => {
            assert!(spline.is_none())
        }
        other => panic!("expected one ObjectCreate, got {other:?}"),
    }
}

/// `PLAYER_VISIBLE_ITEM_<n>_0`, the public entry other players' gear renders from, sits at field
/// 258 (`PLAYER_VISIBLE_ITEM_1_CREATOR`, a 2-field guid) + 2 + 12·slot: 260, 272, 428, 476 here.
#[test]
fn values_update_decodes_player_visible_item_entries() {
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&1u32.to_le_bytes()); // count 1
    body.push(0); // no transport
    body.push(0); // UPDATETYPE_VALUES
    body.extend_from_slice(&hx("01dd")); // packed guid 0xDD
    body.push(15); // 15 mask blocks
    let mut masks = [0u32; 15];
    masks[8] = (1 << 4) | (1 << 16);
    masks[13] = 1 << 12;
    masks[14] = 1 << 28;
    for m in masks {
        body.extend_from_slice(&m.to_le_bytes());
    }
    for v in [7365u32, 2196u32, 15406u32, 2504u32] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    let fields = decode(packet)
        .into_iter()
        .find_map(|e| match e {
            SessionEvent::ObjectValues { guid: 0xDD, fields } => Some(fields),
            _ => None,
        })
        .expect("object values");

    assert_eq!(fields.player_visible_item_entry(0), Some(7365), "HEAD");
    assert_eq!(fields.player_visible_item_entry(1), Some(2196), "NECK");
    assert_eq!(fields.player_visible_item_entry(14), Some(15406), "BACK");
    assert_eq!(fields.player_visible_item_entry(18), Some(2504), "TABARD");
    // An unsent slot and an out-of-range one both read None, never item 0.
    assert_eq!(fields.player_visible_item_entry(2), None, "slot 2 unsent");
    assert_eq!(
        fields.player_visible_item_entry(19),
        None,
        "slot out of range (19 slots, 0..=18)"
    );
}

/// Every block reaches the app, an empty values block and a near-objects block included (the
/// reference's handler acts on both: `0x46720f`, `0x467230`), and the packet's end closes them.
#[test]
fn an_update_packet_decodes_every_block_then_its_end() {
    // Two blocks: values for 0xAA with no mask words, near objects [0x10].
    let mut body = hx("0200000000");
    body.extend_from_slice(&hx("0001aa00"));
    body.extend_from_slice(&hx("05010000000110"));
    let packet = messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap();
    match decode(packet).as_slice() {
        [SessionEvent::ObjectValues { guid: 0xAA, fields }, SessionEvent::ObjectsNear(near), SessionEvent::ObjectUpdateEnd] =>
        {
            assert!(fields.is_empty());
            assert_eq!(near, &[0x10]);
        }
        other => panic!("{other:?}"),
    }
}
