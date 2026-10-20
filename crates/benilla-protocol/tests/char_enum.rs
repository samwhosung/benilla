//! `SMSG_CHAR_ENUM` in vmangos `Player::BuildEnumData` order (`Player.cpp:1629`), two entries per
//! body so any misalignment shows, plus the logout and refused-login replies.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::ServerPacket;

/// One enum entry; the fixed fields hold distinct values so a misaligned parse cannot pass.
fn enum_entry(
    guid: u64,
    name: &str,
    race: u8,
    class: u8,
    gender: u8,
    level: u8,
    map: u32,
    pos: [f32; 3],
    pet_display: u32,
) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&guid.to_le_bytes());
    b.extend_from_slice(name.as_bytes());
    b.push(0);
    b.extend_from_slice(&[race, class, gender]);
    b.extend_from_slice(&[3, 4, 5, 6, 7]); // skin/face/hairStyle/hairColor/facialHair
    b.push(level);
    b.extend_from_slice(&12u32.to_le_bytes()); // zone
    b.extend_from_slice(&map.to_le_bytes());
    for c in pos {
        b.extend_from_slice(&c.to_le_bytes());
    }
    b.extend_from_slice(&0u32.to_le_bytes()); // guild id
    b.extend_from_slice(&0x0200_0000u32.to_le_bytes()); // character flags (DECLINED, a junk value)
    b.push(1); // first login
    b.extend_from_slice(&pet_display.to_le_bytes()); // pet display id
    b.extend_from_slice(&7u32.to_le_bytes()); // pet level
    b.extend_from_slice(&1u32.to_le_bytes()); // pet family
    for slot in 0..19u32 {
        b.extend_from_slice(&(1000 + slot).to_le_bytes()); // equipment display id
        b.push(slot as u8); // inventory type
    }
    b.extend_from_slice(&2000u32.to_le_bytes()); // first bag display id
    b.push(18); // first bag inventory type
    b
}

#[test]
fn char_enum_roster_parses_and_decodes() {
    let mut body = vec![2u8]; // count
    body.extend(enum_entry(
        0x0000_0000_0000_0007,
        "Two",
        1, // Human
        1, // Warrior
        0,
        1,
        0,
        [-8949.95, -132.493, 83.5312],
        99, // a pet, which the select screen stands beside this character
    ));
    body.extend(enum_entry(
        0x0000_0000_0000_0009,
        "Twomage",
        1, // Human
        8, // Mage
        1,
        60,
        1,
        [1.5, -2.5, 3.5],
        0, // a mage: the server sends the triple zeroed, and no pet renders
    ));

    let p = messages::parse_server(messages::opcode::SMSG_CHAR_ENUM, &body).unwrap();
    let ServerPacket::CharEnum { ref characters } = p else {
        panic!("char enum")
    };
    assert_eq!(characters.len(), 2);
    let c = &characters[0];
    assert_eq!(
        (c.guid, c.name.as_str(), c.race, c.class, c.gender),
        (7, "Two", 1, 1, 0)
    );
    assert_eq!(
        (c.skin, c.face, c.hair_style, c.hair_color, c.facial_hair),
        (3, 4, 5, 6, 7)
    );
    assert_eq!((c.zone, c.flags), (12, 0x0200_0000));
    // The pet triple sits before the equipment array: a misread slides every display id below.
    assert_eq!(
        (c.pet_display_id, c.pet_level, c.pet_family),
        (99, 7, 1),
        "the hunter/warlock pet the select screen renders"
    );
    assert_eq!((c.level, c.map), (1, 0));
    assert_eq!((c.position.x, c.position.y), (-8949.95, -132.493));
    for (slot, item) in c.equipment.iter().enumerate() {
        assert_eq!(
            (item.display_id, item.inventory_type),
            (1000 + slot as u32, slot as u8)
        );
    }
    let c = &characters[1];
    assert_eq!(
        (c.guid, c.name.as_str(), c.race, c.class, c.gender),
        (9, "Twomage", 1, 8, 1)
    );
    assert_eq!((c.level, c.map, c.position.z), (60, 1, 3.5));
    assert_eq!(c.equipment[18].display_id, 1018);
    // The server zeroes the triple for all but a hunter's or warlock's pet, so display id 0 is the
    // whole no-pet gate.
    assert_eq!(c.pet_display_id, 0);

    // The raw decode has no realm; the IO thread's own emit carries the auth realm entry.
    match &decode(p)[..] {
        [SessionEvent::CharacterList {
            characters, realm, ..
        }] => {
            assert_eq!(characters.len(), 2);
            assert!(realm.is_none());
        }
        other => panic!("expected CharacterList, got {other:?}"),
    }
}

/// The character list parse stores level 1 for a record read with level 0 and sets the record's
/// new-character byte `+0x10a` (`0x5b44e8` `test al,al`, `0x5b44f2` the store of 1, `0x5b44f9` the
/// flag); any other level is kept and clears the byte (`0x5b4502`). The first-login wire byte
/// (`+0x109`, 1 in every entry here) has no part in it.
#[test]
fn a_level_zero_record_is_stored_as_level_one_and_flagged_new() {
    let mut body = vec![3u8];
    for (guid, level) in [(1, 0), (2, 1), (3, 60)] {
        body.extend(enum_entry(
            guid,
            "Lv",
            1,
            1,
            0,
            level,
            0,
            [1.0, 2.0, 3.0],
            0,
        ));
    }
    let p = messages::parse_server(messages::opcode::SMSG_CHAR_ENUM, &body).unwrap();
    let ServerPacket::CharEnum { characters } = p else {
        panic!("char enum")
    };
    let read: Vec<_> = characters
        .iter()
        .map(|c| (c.guid, c.level, c.new_character))
        .collect();
    assert_eq!(read, [(1, 1, true), (2, 1, false), (3, 60, false)]);
}

#[test]
fn logout_complete_decodes_to_logged_out() {
    let p = messages::parse_server(messages::opcode::SMSG_LOGOUT_COMPLETE, &[]).unwrap();
    assert!(matches!(decode(p)[..], [SessionEvent::LoggedOut]));
}

/// `SMSG_CHARACTER_LOGIN_FAILED` (opcode 65): one raw 1-based reason byte the glue layer maps;
/// vmangos sends only `0x01`, and `0x08` is past the reference's six-entry switch.
#[test]
fn a_refused_character_login_parses_and_decodes() {
    for result in [0x01u8, 0x08] {
        let p = messages::parse_server(
            messages::opcode::SMSG_CHARACTER_LOGIN_FAILED,
            std::slice::from_ref(&result),
        )
        .unwrap();
        assert!(matches!(
            p,
            ServerPacket::CharacterLoginFailed { result: got } if got == result
        ));
        assert!(matches!(
            decode(p)[..],
            [SessionEvent::CharacterLoginFailed { result: got }] if got == result
        ));
    }
}
