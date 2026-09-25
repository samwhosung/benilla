//! Game object messages: `CMSG_GAMEOBJ_USE`, the one verb for every usable object, which the
//! server answers through loot, gossip or state; the template query; and one-shot animations.

use std::io;

use crate::wire::{read_cstring, read_i32_le, read_u32_le, read_u64_le};

/// `CMSG_GAMEOBJ_USE` body: the object's full guid (`Handlers/SpellHandler.cpp`). A chest's loot
/// opens through this, never `CMSG_LOOT`, which rejects a game object guid.
pub fn gameobj_use(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `CMSG_GAMEOBJECT_QUERY` body: the template entry, then a full guid (vmangos `QueryGameObject`).
pub fn gameobject_query(entry: u32, guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&entry.to_le_bytes());
    body.extend_from_slice(&guid.to_le_bytes());
    body
}

/// A game object template from `SMSG_GAMEOBJECT_QUERY_RESPONSE`; `data` is the raw
/// `GameObjectData` union, whose slots mean different things per `type_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct GameObjectQueryInfo {
    pub type_id: u32,
    pub display_id: u32,
    pub name: String,
    pub data: [i32; 24],
}

/// Read `SMSG_GAMEOBJECT_QUERY_RESPONSE` (vmangos `HandleGameObjectQueryOpcode`): no trailing
/// `size` float in 1.12.1, and a miss is the lone `u32` `entry | 0x8000_0000`.
pub(super) fn read_gameobject_query_response(
    r: &mut &[u8],
) -> io::Result<(u32, Option<GameObjectQueryInfo>)> {
    let entry = read_u32_le(r)?;
    if entry & 0x8000_0000 != 0 {
        return Ok((entry & 0x7FFF_FFFF, None));
    }
    let type_id = read_u32_le(r)?;
    let display_id = read_u32_le(r)?;
    let name = read_cstring(r)?;
    for _ in 0..3 {
        let _ = read_cstring(r)?; // name2..name4, always empty in 5875
    }
    let _icon = read_cstring(r)?; // name5, the icon key
    let mut data = [0i32; 24];
    for slot in &mut data {
        *slot = read_i32_le(r)?;
    }
    Ok((
        entry,
        Some(GameObjectQueryInfo {
            type_id,
            display_id,
            name,
            data,
        }),
    ))
}

/// Read `SMSG_GAMEOBJECT_CUSTOM_ANIM`: the guid, then `animId`, which the 1.12 client plays as
/// custom substate `8 + animId` (AnimationData 153 to 156), ignoring `animId >= 4`.
pub(super) fn read_gameobject_custom_anim(r: &mut &[u8]) -> io::Result<(u64, u32)> {
    let guid = read_u64_le(r)?;
    let anim_id = read_u32_le(r)?;
    Ok((guid, anim_id))
}

/// Read `SMSG_GAMEOBJECT_DESPAWN_ANIM`, only the guid (`Objects/Object.cpp:2308`); the 1.12
/// client plays substate 12, AnimationData 157 Despawn.
pub(super) fn read_gameobject_despawn_anim(r: &mut &[u8]) -> io::Result<u64> {
    read_u64_le(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{opcode, parse_server, ServerPacket};

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn cmsg_gameobj_use_body_golden() {
        // SpellHandler.cpp reads a single ObjectGuid.
        assert_eq!(
            gameobj_use(0x1234_5678_9abc_def0),
            hx("f0debc9a78563412"),
            "CMSG_GAMEOBJ_USE body"
        );
    }

    #[test]
    fn cmsg_gameobject_query_body_golden() {
        // As vmangos `QueryGameObject::ReadFromWorldPacket` reads it.
        assert_eq!(
            gameobject_query(1731, 0x1234_5678_9abc_def0),
            hx("c3060000f0debc9a78563412"),
            "CMSG_GAMEOBJECT_QUERY body"
        );
    }

    #[test]
    fn gameobject_query_response_hit_decodes() {
        let mut body = 1731u32.to_le_bytes().to_vec(); // entry
        body.extend_from_slice(&3u32.to_le_bytes()); // type = GAMEOBJECT_TYPE_CHEST
        body.extend_from_slice(&123u32.to_le_bytes()); // displayId
        body.extend_from_slice(b"Chest\0"); // name
        body.extend_from_slice(&[0, 0, 0]); // name2..name4, empty C-strings
        body.extend_from_slice(b"Icon\0"); // icon (name5), dropped
        let mut data = [0i32; 24];
        data[0] = 57; // a stand-in lockId
        for v in data {
            body.extend_from_slice(&v.to_le_bytes());
        }

        match parse_server(opcode::SMSG_GAMEOBJECT_QUERY_RESPONSE, &body).unwrap() {
            ServerPacket::GameObjectQueryResponse { entry, info } => {
                assert_eq!(entry, 1731);
                let info = info.expect("hit");
                assert_eq!(info.type_id, 3);
                assert_eq!(info.display_id, 123);
                assert_eq!(info.name, "Chest");
                assert_eq!(info.data[0], 57);
                assert_eq!(info.data[1], 0);
            }
            other => panic!("expected GameObjectQueryResponse, got {}", other.name()),
        }
    }

    #[test]
    fn gameobject_despawn_anim_decodes() {
        let body = hx("f0debc9a78563412");
        match parse_server(opcode::SMSG_GAMEOBJECT_DESPAWN_ANIM, &body).unwrap() {
            ServerPacket::GameObjectDespawnAnim { guid } => {
                assert_eq!(guid, 0x1234_5678_9abc_def0);
            }
            other => panic!("expected GameObjectDespawnAnim, got {}", other.name()),
        }
    }

    #[test]
    fn gameobject_custom_anim_decodes() {
        // A fishing bobber's bite is animId 0.
        let body = hx("f0debc9a7856341200000000");
        match parse_server(opcode::SMSG_GAMEOBJECT_CUSTOM_ANIM, &body).unwrap() {
            ServerPacket::GameObjectCustomAnim { guid, anim_id } => {
                assert_eq!(guid, 0x1234_5678_9abc_def0);
                assert_eq!(anim_id, 0);
            }
            other => panic!("expected GameObjectCustomAnim, got {}", other.name()),
        }
    }

    #[test]
    fn fish_verdicts_decode_from_empty_bodies() {
        // Both fishing verdicts are size-0 sends (vmangos `GameObject::Update`/`Use`).
        assert!(matches!(
            parse_server(opcode::SMSG_FISH_NOT_HOOKED, &[]).unwrap(),
            ServerPacket::FishNotHooked
        ));
        assert!(matches!(
            parse_server(opcode::SMSG_FISH_ESCAPED, &[]).unwrap(),
            ServerPacket::FishEscaped
        ));
    }

    #[test]
    fn gameobject_query_response_miss_decodes() {
        let body = (1731u32 | 0x8000_0000).to_le_bytes();
        match parse_server(opcode::SMSG_GAMEOBJECT_QUERY_RESPONSE, &body).unwrap() {
            ServerPacket::GameObjectQueryResponse { entry, info } => {
                assert_eq!(entry, 1731);
                assert!(info.is_none());
            }
            other => panic!("expected GameObjectQueryResponse, got {}", other.name()),
        }
    }
}
