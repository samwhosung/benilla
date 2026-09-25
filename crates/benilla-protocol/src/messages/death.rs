//! Death and resurrection: the corpse query, corpse reclaim, spirit healers, resurrect offers.

use std::io;

use crate::wire::{read_cstring, read_f32_le, read_i32_le, read_u32_le, read_u64_le, read_u8};

/// `MSG_CORPSE_QUERY`'s answer (`QueryHandler.cpp:258-304`), a lone `u8(0)` when not found; the
/// request is the same opcode, empty. The server also sends an unprompted not-found when a looted
/// corpse turns to bones (`Map.cpp:3624-3629`), which means "drop the marker".
#[derive(Debug, Clone, PartialEq)]
pub struct CorpseLocation {
    pub found: bool,
    /// The map to walk toward, rewritten to the entrance's for a corpse inside a dungeon.
    pub display_map: i32,
    /// Raw WoW coordinates of the corpse, or of the dungeon entrance.
    pub position: [f32; 3],
    /// The corpse's real map id, never adjusted.
    pub corpse_map: u32,
}

pub(super) fn read_corpse_query_response(r: &mut &[u8]) -> io::Result<CorpseLocation> {
    let found = read_u8(r)? != 0;
    if !found {
        return Ok(CorpseLocation {
            found: false,
            display_map: 0,
            position: [0.0; 3],
            corpse_map: 0,
        });
    }
    let display_map = read_i32_le(r)?;
    let position = [read_f32_le(r)?, read_f32_le(r)?, read_f32_le(r)?];
    let corpse_map = read_u32_le(r)?;
    Ok(CorpseLocation {
        found: true,
        display_map,
        position,
        corpse_map,
    })
}

/// Read `SMSG_CORPSE_RECLAIM_DELAY`: the ms until the corpse can be reclaimed
/// (`Misc.cpp:653-656`), sent at release and at login while dead, never at death itself. It is
/// 30 s, or 60 or 120 s after repeated deaths (`Player.cpp:106`).
pub(super) fn read_corpse_reclaim_delay(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// `SMSG_RESURRECT_REQUEST`, a resurrection offer (`Spell.cpp:5024-5044`). `sickness` warns the
/// accept brings resurrection sickness, `has_timer` keeps the reclaim-delay gate, and the stock
/// popup picks its variant by the two. `name` is empty for a player caster, named by guid instead.
#[derive(Debug, Clone, PartialEq)]
pub struct ResurrectRequestBody {
    pub caster: u64,
    /// Empty for a player caster (resolve by guid); the creature's name for an NPC caster.
    pub name: String,
    pub sickness: bool,
    pub has_timer: bool,
}

/// Read `SMSG_RESURRECT_REQUEST`; the name's `u32` length (strlen + 1) is skipped for its NUL.
pub(super) fn read_resurrect_request(r: &mut &[u8]) -> io::Result<ResurrectRequestBody> {
    let caster = read_u64_le(r)?;
    let _name_len = read_u32_le(r)?;
    let name = read_cstring(r)?;
    let sickness = read_u8(r)? != 0;
    let has_timer = read_u8(r)? != 0;
    Ok(ResurrectRequestBody {
        caster,
        name,
        sickness,
        has_timer,
    })
}

/// Read `SMSG_SPIRIT_HEALER_CONFIRM`: the spirit healer's full guid, sent from its gossip option
/// through spell 17251 (`SpellEffects.cpp:818-831`); accepting sends [`spirit_healer_activate`].
pub(super) fn read_spirit_healer_confirm(r: &mut &[u8]) -> io::Result<u64> {
    read_u64_le(r)
}

/// `CMSG_RECLAIM_CORPSE` body: the corpse's full guid, as the 1.12 client sends, though the server
/// ignores it. It requires a ghost, the delay elapsed and 39 yd range (`MiscHandler.cpp:573-603`).
pub fn reclaim_corpse(corpse_guid: u64) -> Vec<u8> {
    corpse_guid.to_le_bytes().to_vec()
}

/// `CMSG_SPIRIT_HEALER_ACTIVATE` body: the healer's full guid (`Npc.cpp:40-43`), within 5 yd. The
/// server resurrects at 50% with 25% durability loss and, from level 11, sickness
/// (`NPCHandler.cpp:430-477`).
pub fn spirit_healer_activate(npc: u64) -> Vec<u8> {
    npc.to_le_bytes().to_vec()
}

/// `CMSG_RESURRECT_RESPONSE` body: the offerer's guid and `u8 accept` (`Misc.cpp:132-136`); an
/// accept moves us to the caster and resurrects us (`Player.cpp:20188-20244`).
pub fn resurrect_response(caster: u64, accept: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.extend_from_slice(&caster.to_le_bytes());
    body.push(u8::from(accept));
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built as `HandleCorpseQueryOpcode` builds it (`QueryHandler.cpp:296-303`).
    #[test]
    fn corpse_query_found_golden() {
        let mut body = vec![1u8];
        body.extend_from_slice(&0i32.to_le_bytes());
        body.extend_from_slice(&(-8949.95f32).to_le_bytes());
        body.extend_from_slice(&(-132.49f32).to_le_bytes());
        body.extend_from_slice(&83.53f32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        let mut r = body.as_slice();
        let loc = read_corpse_query_response(&mut r).unwrap();
        assert!(r.is_empty(), "the whole body is consumed");
        assert_eq!(
            loc,
            CorpseLocation {
                found: true,
                display_map: 0,
                position: [-8949.95, -132.49, 83.53],
                corpse_map: 0,
            }
        );
    }

    /// The lone `u8(0)` of `QueryHandler.cpp:262-267`.
    #[test]
    fn corpse_query_not_found_golden() {
        let body = [0u8];
        let mut r = body.as_slice();
        let loc = read_corpse_query_response(&mut r).unwrap();
        assert!(r.is_empty());
        assert!(!loc.found);
        assert_eq!(loc.corpse_map, 0);
    }

    /// A dungeon corpse's entrance map and real map (`QueryHandler.cpp:276-292`).
    #[test]
    fn corpse_query_dungeon_entrance_split() {
        let mut body = vec![1u8];
        body.extend_from_slice(&0i32.to_le_bytes()); // entrance map: EK
        body.extend_from_slice(&(-11209.6f32).to_le_bytes()); // Deadmines entrance-ish
        body.extend_from_slice(&1666.54f32.to_le_bytes());
        body.extend_from_slice(&25.0f32.to_le_bytes());
        body.extend_from_slice(&36u32.to_le_bytes()); // the corpse's real map: Deadmines
        let mut r = body.as_slice();
        let loc = read_corpse_query_response(&mut r).unwrap();
        assert_eq!(loc.display_map, 0);
        assert_eq!(loc.corpse_map, 36);
    }

    /// One `u32` of ms (`Misc.cpp:653-656`); 30 s is the base delay.
    #[test]
    fn corpse_reclaim_delay_golden() {
        let body = 30_000u32.to_le_bytes();
        let mut r = body.as_slice();
        assert_eq!(read_corpse_reclaim_delay(&mut r).unwrap(), 30_000);
        assert!(r.is_empty());
    }

    /// Built as `Spell::SendResurrectRequest` builds a player's offer (`Spell.cpp:5024-5044`).
    #[test]
    fn resurrect_request_player_caster_golden() {
        let mut body = Vec::new();
        body.extend_from_slice(&0x0000_0001_0000_002Au64.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes()); // strlen("") + 1
        body.push(0); // the empty cstring
        body.push(0); // sickness = false
        body.push(1); // hasResTimer = true
        let mut r = body.as_slice();
        let req = read_resurrect_request(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(
            req,
            ResurrectRequestBody {
                caster: 0x0000_0001_0000_002A,
                name: String::new(),
                sickness: false,
                has_timer: true,
            }
        );
    }

    /// A spirit healer's offer as `Spell::SendResurrectRequest` builds it: named, with sickness.
    #[test]
    fn resurrect_request_npc_caster_golden() {
        let name = b"Spirit Healer";
        let mut body = Vec::new();
        body.extend_from_slice(&0xF130_0FBE_0000_2AB3u64.to_le_bytes());
        body.extend_from_slice(&((name.len() + 1) as u32).to_le_bytes());
        body.extend_from_slice(name);
        body.push(0);
        body.push(1); // sickness = true
        body.push(1); // hasResTimer = true
        let mut r = body.as_slice();
        let req = read_resurrect_request(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(
            req,
            ResurrectRequestBody {
                caster: 0xF130_0FBE_0000_2AB3,
                name: "Spirit Healer".into(),
                sickness: true,
                has_timer: true,
            }
        );
    }

    /// `SMSG_SPIRIT_HEALER_CONFIRM` is one full guid (`SpellEffects.cpp:825-829`).
    #[test]
    fn spirit_healer_confirm_golden() {
        let body = 0xF130_0FBE_0000_2AB3u64.to_le_bytes();
        let mut r = body.as_slice();
        assert_eq!(
            read_spirit_healer_confirm(&mut r).unwrap(),
            0xF130_0FBE_0000_2AB3
        );
        assert!(r.is_empty());
    }

    #[test]
    fn client_bodies_are_full_guids() {
        assert_eq!(
            reclaim_corpse(0xF500_0000_0000_0001),
            0xF500_0000_0000_0001u64.to_le_bytes().to_vec()
        );
        assert_eq!(
            spirit_healer_activate(0xF130_0FBE_0000_2AB3),
            0xF130_0FBE_0000_2AB3u64.to_le_bytes().to_vec()
        );
        let mut expect = 0x2Au64.to_le_bytes().to_vec();
        expect.push(1);
        assert_eq!(resurrect_response(0x2A, true), expect);
        let mut expect = 0x2Au64.to_le_bytes().to_vec();
        expect.push(0);
        assert_eq!(resurrect_response(0x2A, false), expect);
        assert_eq!(
            area_spirit_healer(0xF130_0033_3C00_0010),
            0xF130_0033_3C00_0010u64.to_le_bytes().to_vec()
        );
    }
}

/// `SMSG_AREA_SPIRIT_HEALER_TIME`: a battleground spirit healer's guid and the ms until its next
/// resurrection wave (client `0x48fa29`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AreaSpiritHealerTime {
    pub healer: u64,
    pub ms: u32,
}

pub(super) fn read_area_spirit_healer_time(
    r: &mut impl std::io::Read,
) -> std::io::Result<AreaSpiritHealerTime> {
    Ok(AreaSpiritHealerTime {
        healer: crate::wire::read_u64_le(r)?,
        ms: crate::wire::read_u32_le(r)?,
    })
}

/// `CMSG_AREA_SPIRIT_HEALER_QUERY` or `_QUEUE` body: the healer's guid; the client queries when it
/// adopts a healer and queues on accept.
pub fn area_spirit_healer(healer: u64) -> Vec<u8> {
    healer.to_le_bytes().to_vec()
}
