//! The cast pipeline on the wire: the `CMSG_CAST_SPELL` shapes, the server's verdict, the
//! start/launch pair every observer sees, interrupt and pushback notices, the channel timer and the
//! aura messages. Aura state itself is descriptor data (`UNIT_FIELD_AURA`), not a packet.

use std::io::{self, Read};

use crate::wire::{
    capacity_hint, read_cstring, read_i32_le, read_packed_guid, read_u16_le, read_u32_le,
    read_u64_le, read_u8, Vector3d,
};

/// `SMSG_CAST_RESULT`'s verdict: status 0 (`OKAY`) ends the packet; status 2 (`FAIL`) adds a `u8`
/// reason and up to two argument words (vmangos `CastResult::AppendBodyTo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastOutcome {
    Ok,
    Failed {
        reason: u8,
        /// The `%s` of the client's message (`0x6e1d8e`): a `SpellFocusObject.dbc` id for
        /// `REQUIRES_SPELL_FOCUS` (0x5e), an `AreaTable.dbc` id for `REQUIRES_AREA` (0x5d), an
        /// `EquippedItemClass` for 0x19-0x1b, a permanent-cooldown flag for `NOT_READY` (0x3c).
        arg: Option<u32>,
    },
}

/// Read `SMSG_CAST_RESULT` (client `0x6e7330`). Each argument word is read only while bytes
/// remain, never keyed on the reason, because vmangos omits a zero argument. `None` stands for
/// the client's absent value, -1 (`0x6e736e`). The second word, sent only for 0x19-0x1b, is
/// dropped: the client fills that message from the spell's own `EquippedItem*` columns.
pub(super) fn read_cast_result(r: &mut &[u8]) -> io::Result<(u32, CastOutcome)> {
    let spell_id = read_u32_le(r)?;
    let status = read_u8(r)?;
    let outcome = if status == 2 {
        let reason = read_u8(r)?;
        let arg = (r.len() >= 4).then(|| read_u32_le(r)).transpose()?;
        let _arg2 = (r.len() >= 4).then(|| read_u32_le(r)).transpose()?;
        CastOutcome::Failed { reason, arg }
    } else {
        CastOutcome::Ok
    };
    Ok((spell_id, outcome))
}

// --- Start and go --------------------------------------------------------------------------------
//
// vmangos `SpellCastTargetFlags` (`SpellDefines.h:96-113`).
const TARGET_FLAG_UNIT: u16 = 0x0002;
const TARGET_FLAG_ITEM: u16 = 0x0010;
const TARGET_FLAG_SOURCE_LOCATION: u16 = 0x0020;
const TARGET_FLAG_DEST_LOCATION: u16 = 0x0040;
const TARGET_FLAG_CORPSE_ENEMY: u16 = 0x0200;
const TARGET_FLAG_GAMEOBJECT: u16 = 0x0800;
const TARGET_FLAG_TRADE_ITEM: u16 = 0x1000;
const TARGET_FLAG_STRING: u16 = 0x2000;
const TARGET_FLAG_CORPSE_ALLY: u16 = 0x8000;

/// A decoded `SpellCastTargets` in the server's write order (`SpellCastTargetsInfo.cpp:180-234`),
/// not its read order: any mix of UNIT, GAMEOBJECT and CORPSE_* bits yields one packed guid.
/// Item, corpse, source and string targets are read and dropped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpellCastTargets {
    pub mask: u16,
    pub unit_target: Option<u64>,
    /// The `TARGET_FLAG_GAMEOBJECT` target, such as the chest an open-lock cast opens.
    pub go_target: Option<u64>,
    pub dest: Option<Vector3d>,
}

fn read_spell_cast_targets(r: &mut impl Read) -> io::Result<SpellCastTargets> {
    let mask = read_u16_le(r)?;
    let mut unit_target = None;
    let mut go_target = None;
    if mask
        & (TARGET_FLAG_UNIT
            | TARGET_FLAG_GAMEOBJECT
            | TARGET_FLAG_CORPSE_ENEMY
            | TARGET_FLAG_CORPSE_ALLY)
        != 0
    {
        // One packed guid; the server's priority is UNIT, then GAMEOBJECT, then CORPSE_*.
        let guid = read_packed_guid(r)?;
        if mask & TARGET_FLAG_UNIT != 0 {
            unit_target = Some(guid);
        } else if mask & TARGET_FLAG_GAMEOBJECT != 0 {
            go_target = Some(guid);
        }
    }
    if mask & (TARGET_FLAG_ITEM | TARGET_FLAG_TRADE_ITEM) != 0 {
        let _item_guid = read_packed_guid(r)?;
    }
    if mask & TARGET_FLAG_SOURCE_LOCATION != 0 {
        let _src = Vector3d::read(r)?;
    }
    let dest = if mask & TARGET_FLAG_DEST_LOCATION != 0 {
        Some(Vector3d::read(r)?)
    } else {
        None
    };
    if mask & TARGET_FLAG_STRING != 0 {
        let _string_target = read_cstring(r)?;
    }
    Ok(SpellCastTargets {
        mask,
        unit_target,
        go_target,
        dest,
    })
}

/// vmangos `Spell.h:53`: set on START and GO for every ranged spell; gates the ammo block.
const CAST_FLAG_AMMO: u16 = 0x0020;

/// Read the ammo block (`Spell.cpp:4540-4606`): `u32 displayId`, `u32 inventoryType` (dropped).
fn read_ammo(r: &mut impl Read) -> io::Result<u32> {
    let display_id = read_u32_le(r)?;
    let _inventory_type = read_u32_le(r)?;
    Ok(display_id)
}

/// `SMSG_SPELL_START` (vmangos `Spell.cpp:4468-4503`): a non-triggered cast began, instants
/// included (`cast_time_ms == 0`). `item_or_caster` is the cast item's guid if any, else the
/// caster's; `caster` is 0 for a GameObject caster. `cast_flags` is always 0x2, plus
/// `CAST_FLAG_AMMO` for a ranged spell.
#[derive(Debug, Clone, PartialEq)]
pub struct SpellStart {
    pub item_or_caster: u64,
    pub caster: u64,
    pub spell_id: u32,
    pub cast_flags: u16,
    pub cast_time_ms: u32,
    pub targets: SpellCastTargets,
    pub ammo_display_id: Option<u32>,
}

pub(super) fn read_spell_start(r: &mut impl Read) -> io::Result<SpellStart> {
    let item_or_caster = read_packed_guid(r)?;
    let caster = read_packed_guid(r)?;
    let spell_id = read_u32_le(r)?;
    let cast_flags = read_u16_le(r)?;
    let cast_time_ms = read_u32_le(r)?;
    let targets = read_spell_cast_targets(r)?;
    let ammo_display_id = if cast_flags & CAST_FLAG_AMMO != 0 {
        Some(read_ammo(r)?)
    } else {
        None
    };
    Ok(SpellStart {
        item_or_caster,
        caster,
        spell_id,
        cast_flags,
        cast_time_ms,
        targets,
        ammo_display_id,
    })
}

/// vmangos `SpellDefines.h:173`: the one `SpellMissInfo` followed by a byte, the reflected
/// spell's outcome against its new target.
const SPELL_MISS_REFLECT: u8 = 11;

/// `SMSG_SPELL_GO` (vmangos `Spell.cpp:4505-4538, 4608-4659`): the cast launched. Guids and spell
/// as in [`SpellStart`]; `cast_flags` is always 0x100, plus `CAST_FLAG_AMMO` for a ranged spell.
/// Sent at launch: the server times the impact from `Spell.dbc` Speed, so no missile travel data
/// rides it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpellGo {
    pub item_or_caster: u64,
    pub caster: u64,
    pub spell_id: u32,
    pub cast_flags: u16,
    pub hits: Vec<u64>,
    /// `(guid, SpellMissInfo)`; a reflect's trailing outcome byte is dropped.
    pub misses: Vec<(u64, u8)>,
    pub targets: SpellCastTargets,
    pub ammo_display_id: Option<u32>,
}

pub(super) fn read_spell_go(r: &mut impl Read) -> io::Result<SpellGo> {
    let item_or_caster = read_packed_guid(r)?;
    let caster = read_packed_guid(r)?;
    let spell_id = read_u32_le(r)?;
    let cast_flags = read_u16_le(r)?;

    // Both counts are `u8`s the server backfills (`Spell.cpp:4657-4658`); no tighter bound.
    let hit_count = read_u8(r)?;
    let mut hits = Vec::with_capacity(capacity_hint(hit_count, usize::from(u8::MAX)));
    for _ in 0..hit_count {
        hits.push(read_u64_le(r)?); // raw guid, never packed (Spell.cpp:4627,4635)
    }
    let miss_count = read_u8(r)?;
    let mut misses = Vec::with_capacity(capacity_hint(miss_count, usize::from(u8::MAX)));
    for _ in 0..miss_count {
        let guid = read_u64_le(r)?;
        let reason = read_u8(r)?;
        if reason == SPELL_MISS_REFLECT {
            let _reflect_result = read_u8(r)?;
        }
        misses.push((guid, reason));
    }

    let targets = read_spell_cast_targets(r)?;
    let ammo_display_id = if cast_flags & CAST_FLAG_AMMO != 0 {
        Some(read_ammo(r)?)
    } else {
        None
    };
    Ok(SpellGo {
        item_or_caster,
        caster,
        spell_id,
        cast_flags,
        hits,
        misses,
        targets,
        ammo_display_id,
    })
}

/// Read `SMSG_SPELL_FAILED_OTHER` (vmangos `Spell.cpp:4780-4789`): an observer's cast-cancel
/// notice, raw `u64` caster then spell id. Our own failure rides `SMSG_CAST_RESULT`; vmangos never
/// sends `SMSG_SPELL_FAILURE`.
pub(super) fn read_spell_failed_other(r: &mut impl Read) -> io::Result<(u64, u32)> {
    let caster = read_u64_le(r)?;
    let spell_id = read_u32_le(r)?;
    Ok((caster, spell_id))
}

/// Read `SMSG_SPELL_DELAYED` (vmangos `Spell.cpp:7472`): raw `u64` caster, `u32` pushback ms. Sent
/// to the caster when damage pushes back a cast; the server extends its own cast timer by as much.
pub(super) fn read_spell_delayed(r: &mut impl Read) -> io::Result<(u64, u32)> {
    let caster = read_u64_le(r)?;
    let delay_ms = read_u32_le(r)?;
    Ok((caster, delay_ms))
}

/// Read `MSG_CHANNEL_START` (vmangos `Spell.cpp:4963-4966`): `(spell_id, duration_ms)`, sent only
/// to the caster, so no guid.
pub(super) fn read_channel_start(r: &mut impl Read) -> io::Result<(u32, u32)> {
    let spell_id = read_u32_le(r)?;
    let duration_ms = read_u32_le(r)?;
    Ok((spell_id, duration_ms))
}

/// Read `MSG_CHANNEL_UPDATE` (vmangos `Player.cpp:21141-21146`): ms left, caster only; 0 ends the
/// channel, whether it finished or was interrupted.
pub(super) fn read_channel_update(r: &mut impl Read) -> io::Result<u32> {
    read_u32_le(r)
}

/// Read `SMSG_SET_FLAT_SPELL_MODIFIER` / `SMSG_SET_PCT_SPELL_MODIFIER`: a `u8` `SpellFamilyFlags`
/// bit index (0..63), a `u8` SpellModOp (0..28), then the `i32` sum of every matching modifier.
/// The bit comes first: the client (`0x6e9950`) stores at `bit * 29 + op`, and vmangos
/// (`Player.cpp:17815`) sends one packet per set bit. The client range-checks neither byte.
pub(super) fn read_set_spell_modifier(r: &mut impl Read) -> io::Result<(u8, u8, i32)> {
    let mask_bit = read_u8(r)?;
    let op = read_u8(r)?;
    let value = read_i32_le(r)?;
    Ok((mask_bit, op, value))
}

/// Read `SMSG_UPDATE_AURA_DURATION` (vmangos `SpellAuras.cpp:7511-7523`): an aura slot and its ms
/// left. Sent only to the aura's target and never for a permanent aura, so an occupied slot with
/// no duration lasts until cancelled.
pub(super) fn read_update_aura_duration(r: &mut impl Read) -> io::Result<(u8, u32)> {
    let slot = read_u8(r)?;
    let remaining_ms = read_u32_le(r)?;
    Ok((slot, remaining_ms))
}

/// Read `SMSG_PLAY_SPELL_VISUAL` (vmangos `Packets/Spell.cpp:54-58`): raw `u64` unit, `u32` kit
/// id. The client (`0x6e98d0`) checks the kit against `SpellVisualKit.dbc` and plays it at stage 0.
pub(super) fn read_play_spell_visual(r: &mut impl Read) -> io::Result<(u64, u32)> {
    let unit = read_u64_le(r)?;
    let kit_id = read_u32_le(r)?;
    Ok((unit, kit_id))
}

/// Body of `CMSG_CAST_SPELL` for a self or unit target. `None` sends mask 0 (`TARGET_FLAG_SELF`)
/// and the server applies the spell's implicit targeting; `Some` sends `TARGET_FLAG_UNIT` and the
/// packed guid.
pub fn cast_spell(spell_id: u32, target: Option<u64>) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&spell_id.to_le_bytes());
    match target {
        None => body.extend_from_slice(&0u16.to_le_bytes()),
        Some(guid) => {
            body.extend_from_slice(&2u16.to_le_bytes());
            crate::wire::write_packed_guid(guid, &mut body).expect("vec write");
        }
    }
    body
}

/// Body of `CMSG_CAST_SPELL` at a GameObject: the open-lock cast on a locked chest, vein or herb
/// (instead of `CMSG_GAMEOBJ_USE`), or a targeting-cursor commit. The mask is `GAMEOBJECT` alone:
/// the 1.12 client never puts `TARGET_FLAG_LOCKED` (0x4000) on the wire; `BindTarget` (`0x6e5b40`)
/// consumes it from the targeting word, and no write to the wire mask (`0xceac5c`) sets it.
pub fn cast_spell_gameobject(spell_id: u32, go_guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&spell_id.to_le_bytes());
    body.extend_from_slice(&TARGET_FLAG_GAMEOBJECT.to_le_bytes());
    crate::wire::write_packed_guid(go_guid, &mut body).expect("vec write");
    body
}

/// Body of `CMSG_CAST_SPELL` at an item, such as an enchant or poison: `TARGET_FLAG_ITEM` and the
/// item's packed guid (`SpellCastTargetsInfo.cpp:159-160`).
pub fn cast_spell_item(spell_id: u32, item_guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&spell_id.to_le_bytes());
    body.extend_from_slice(&TARGET_FLAG_ITEM.to_le_bytes());
    crate::wire::write_packed_guid(item_guid, &mut body).expect("vec write");
    body
}

/// Body of `CMSG_CAST_SPELL` at a ground point, for a `Targets & 0x40` spell such as Blizzard:
/// `DEST_LOCATION` and three `f32` WoW world coords (`SpellCastTargetsInfo.cpp:169-174`; client
/// `BindLocation` `0x6e60f0`).
pub fn cast_spell_at_dest(spell_id: u32, dest: [f32; 3]) -> Vec<u8> {
    let mut body = Vec::with_capacity(18);
    body.extend_from_slice(&spell_id.to_le_bytes());
    body.extend_from_slice(&TARGET_FLAG_DEST_LOCATION.to_le_bytes());
    for c in dest {
        body.extend_from_slice(&c.to_le_bytes());
    }
    body
}

/// Body of `CMSG_CAST_SPELL` at a source point, for a `Targets & 0x20` spell: `SOURCE_LOCATION`
/// and three `f32` WoW world coords (client `0x6e60f0`, `SpellCastTargetsInfo.cpp:161-167`). The
/// server centres the AoE there (`TARGET_ENUM_UNITS_ENEMY_AOE_AT_SRC_LOC`, `Spell.cpp:2265`).
pub fn cast_spell_at_source(spell_id: u32, src: [f32; 3]) -> Vec<u8> {
    let mut body = Vec::with_capacity(18);
    body.extend_from_slice(&spell_id.to_le_bytes());
    body.extend_from_slice(&TARGET_FLAG_SOURCE_LOCATION.to_le_bytes());
    for c in src {
        body.extend_from_slice(&c.to_le_bytes());
    }
    body
}

/// Body of `CMSG_CANCEL_AURA`: by spell id, not slot. `HandleCancelAuraOpcode`
/// (`SpellHandler.cpp:333-405`) refuses passives, debuffs and `SPELL_ATTR_NO_AURA_CANCEL` spells;
/// the client gates on the aura's `AURA_FLAG_CANCELABLE` bit.
pub fn cancel_aura(spell_id: u32) -> Vec<u8> {
    spell_id.to_le_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_spell_gameobject_body_golden() {
        // spell 1, mask 0x0800 (no LOCKED 0x4000), packed guid 0x1234 (mask 0x03, bytes 34 12).
        assert_eq!(
            cast_spell_gameobject(1, 0x1234),
            [0x01, 0x00, 0x00, 0x00, 0x00, 0x08, 0x03, 0x34, 0x12],
            "CMSG_CAST_SPELL (GameObject/OPEN_LOCK) body"
        );
        // The self-cast shape stays distinct: mask 0, no guid.
        assert_eq!(cast_spell(1, None), [0x01, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn cast_spell_at_dest_body_golden() {
        // Spell 10 is Blizzard.
        assert_eq!(
            cast_spell_at_dest(10, [1.0, -2.5, 3.0]),
            [
                0x0A, 0x00, 0x00, 0x00, // spell id
                0x40, 0x00, // TARGET_FLAG_DEST_LOCATION
                0x00, 0x00, 0x80, 0x3F, // x = 1.0
                0x00, 0x00, 0x20, 0xC0, // y = -2.5
                0x00, 0x00, 0x40, 0x40, // z = 3.0
            ],
            "CMSG_CAST_SPELL (ground dest) body"
        );
    }

    #[test]
    fn cast_spell_at_source_body_golden() {
        // Spell 265 is Area Death (TEST), Martin Fury's on-use.
        assert_eq!(
            cast_spell_at_source(265, [1.0, -2.5, 3.0]),
            [
                0x09, 0x01, 0x00, 0x00, // spell id 265
                0x20, 0x00, // TARGET_FLAG_SOURCE_LOCATION
                0x00, 0x00, 0x80, 0x3F, // x = 1.0
                0x00, 0x00, 0x20, 0xC0, // y = -2.5
                0x00, 0x00, 0x40, 0x40, // z = 3.0
            ],
            "CMSG_CAST_SPELL (ground source) body"
        );
        let (src, dest) = (
            cast_spell_at_source(265, [1.0, -2.5, 3.0]),
            cast_spell_at_dest(265, [1.0, -2.5, 3.0]),
        );
        assert_eq!(src.len(), dest.len());
        assert_eq!(
            src.iter().zip(&dest).filter(|(a, b)| a != b).count(),
            1,
            "source and dest bodies differ only in the mask"
        );
    }

    #[test]
    fn aura_bodies_golden() {
        // CMSG_CANCEL_AURA: 1126 (Mark of the Wild) = 0x0000_0466.
        assert_eq!(cancel_aura(1126), [0x66, 0x04, 0x00, 0x00]);

        // SMSG_UPDATE_AURA_DURATION: u8 slot, then u32 ms LE. Slot 3, 12_000 ms = 0x0000_2EE0.
        let body = [0x03, 0xE0, 0x2E, 0x00, 0x00];
        let mut r = &body[..];
        assert_eq!(read_update_aura_duration(&mut r).unwrap(), (3, 12_000));
        assert!(
            r.is_empty(),
            "the body is exactly 5 bytes — slot is a byte, not a dword"
        );
    }

    #[test]
    fn set_spell_modifier_body_golden() {
        // mask_bit 35 (0x23), op 14 (0x0e, SPELLMOD_COST), value -30 (0xFFFF_FFE2 LE).
        let body = [0x23, 0x0E, 0xE2, 0xFF, 0xFF, 0xFF];
        let mut r = &body[..];
        assert_eq!(read_set_spell_modifier(&mut r).unwrap(), (35, 14, -30));
        assert!(
            r.is_empty(),
            "the body is exactly 6 bytes — two bytes and a dword, never three dwords"
        );

        // 35 is a legal mask bit (0..=63) but not a legal op (0..=28), so a swap would show.
        let (mask_bit, op, _) = read_set_spell_modifier(&mut &body[..]).unwrap();
        assert!(
            mask_bit >= 29,
            "field 1 would be an out-of-range op if swapped"
        );
        assert!(op < 29, "field 2 is a legal op as read");

        // Both bytes are unsigned, the value signed; the consumer must refuse bit 255.
        assert_eq!(
            read_set_spell_modifier(&mut &[0xFF, 0xFF, 0x01, 0x00, 0x00, 0x00][..]).unwrap(),
            (255, 255, 1)
        );
    }
}

/// `SMSG_SPELL_UPDATE_CHAIN_TARGETS`: a beam's hop list (vmangos `Spell.cpp:4970-4997`, client
/// `0x6e9820`). vmangos sends it only for channeled spells; the 1.12 client also fills the same
/// hop array from `SMSG_SPELL_GO`'s hits (`0x6e800d`), which is how Chain Lightning draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellChainTargets {
    pub caster: u64,
    pub spell_id: u32,
    /// In wire order. The client (`0x605780`) skips any entry equal to the caster; that filter is
    /// the consumer's.
    pub targets: Vec<u64>,
}

/// Read `SMSG_SPELL_UPDATE_CHAIN_TARGETS`; every guid in it is a raw `u64`, never packed.
pub(super) fn read_spell_chain_targets(r: &mut impl Read) -> io::Result<SpellChainTargets> {
    let caster = read_u64_le(r)?;
    let spell_id = read_u32_le(r)?;
    let count = read_u32_le(r)?;
    // The `u32` count is unbounded, so nothing is pre-allocated from it.
    let mut targets = Vec::new();
    for _ in 0..count {
        targets.push(read_u64_le(r)?);
    }
    Ok(SpellChainTargets {
        caster,
        spell_id,
        targets,
    })
}
