//! Spell-book and cooldown messages, all inbound. A normal cast's cooldown is tracked by the
//! client; the server sends cooldowns only for school lockouts, pets, item procs and GM resets.

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_u16_le, read_u32_le, read_u64_le, read_u8};

/// One active cooldown from `SMSG_INITIAL_SPELLS` (vmangos `SendInitialSpells`). A permanent one,
/// re-armed by the server, is `spell_cd_ms == 1` with the category word's top bit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellCooldown {
    pub spell_id: u16,
    pub item_id: u16,
    pub category: u16,
    pub spell_cd_ms: u32,
    pub category_cd_ms: u32,
}

/// Read `SMSG_INITIAL_SPELLS` (vmangos `Player::SendInitialSpells`): `u8 0`, a `u16` count of
/// `{u16 spellId, u16 0}`, then a `u16` count of [`SpellCooldown`]s.
pub(super) fn read_initial_spells(r: &mut impl Read) -> io::Result<(Vec<u16>, Vec<SpellCooldown>)> {
    let _ = read_u8(r)?;
    let n = read_u16_le(r)?;
    // No protocol bound on either list; a 1.12 spellbook holds a few hundred spells.
    let mut spells = Vec::with_capacity(capacity_hint(n, 1024));
    for _ in 0..n {
        spells.push(read_u16_le(r)?);
        let _ = read_u16_le(r)?;
    }
    let m = read_u16_le(r)?;
    let mut cooldowns = Vec::with_capacity(capacity_hint(m, 1024));
    for _ in 0..m {
        cooldowns.push(SpellCooldown {
            spell_id: read_u16_le(r)?,
            item_id: read_u16_le(r)?,
            category: read_u16_le(r)?,
            spell_cd_ms: read_u32_le(r)?,
            category_cd_ms: read_u32_le(r)?,
        });
    }
    Ok((spells, cooldowns))
}

/// Read `SMSG_LEARNED_SPELL` (vmangos `Server/Packets/Spell.cpp:175-179`): `u16 spellId`, then
/// an action-bar slot the client does not use. The one message that adds to the book after login.
pub(super) fn read_learned_spell(r: &mut impl Read) -> io::Result<u16> {
    let spell_id = read_u16_le(r)?;
    let _action_bar_slot = read_u16_le(r)?;
    Ok(spell_id)
}

/// Read `SMSG_REMOVED_SPELL` (vmangos `Server/Packets/Spell.cpp:181`): a bare `u16 spellId`, no
/// action-bar slot.
pub(super) fn read_removed_spell(r: &mut impl Read) -> io::Result<u16> {
    read_u16_le(r)
}

/// Read `SMSG_SUPERCEDED_SPELL` (vmangos `Server/Packets/Spell.cpp:169-173`): `(oldSpellId,
/// newSpellId)`; a rank-up replaces the old spell in both the book and the action bar.
pub(super) fn read_superceded_spell(r: &mut impl Read) -> io::Result<(u16, u16)> {
    Ok((read_u16_le(r)?, read_u16_le(r)?))
}

/// Read `SMSG_SPELL_COOLDOWN` (vmangos `Server/Packets/Spell.cpp:142-150`, client `0x6e9460`): a
/// `u64` caster, then `(spell, ms)` pairs to the end, with no flags byte in 1.12. `ms == 0` means
/// the spell's own `Spell.dbc` recovery times. Sent for school lockouts and pet cooldowns.
pub(super) fn read_spell_cooldown(r: &mut &[u8]) -> io::Result<(u64, Vec<(u32, u32)>)> {
    let caster = read_u64_le(r)?;
    let mut cooldowns = Vec::new();
    while !r.is_empty() {
        let spell_id = read_u32_le(r)?;
        let cooldown_ms = read_u32_le(r)?;
        cooldowns.push((spell_id, cooldown_ms));
    }
    Ok((caster, cooldowns))
}

/// Read `SMSG_ITEM_COOLDOWN` (vmangos `Server/Packets/Item.cpp:229-233`): `(item_guid, spell_id)`,
/// sent when an item with an on-use spell is equipped (`Player::ApplyEquipCooldown`,
/// `Player.cpp:19358-19384`). The client (`0x6e95d0`) gives the item a hardcoded 30 s cooldown; no
/// duration is on the wire.
pub(super) fn read_item_cooldown(r: &mut impl Read) -> io::Result<(u64, u32)> {
    Ok((read_u64_le(r)?, read_u32_le(r)?))
}

/// Read `SMSG_COOLDOWN_EVENT` or `SMSG_CLEAR_COOLDOWN` (vmangos
/// `Server/Packets/Spell.cpp:152-167`, client `0x6e9670`): the `u32` spell id first, then the
/// `u64` caster. EVENT starts the timers of an on-hold (`SPELL_ATTR_COOLDOWN_ON_EVENT`) cooldown;
/// CLEAR removes the cooldown.
pub(super) fn read_cooldown_event(r: &mut impl Read) -> io::Result<(u32, u64)> {
    Ok((read_u32_le(r)?, read_u64_le(r)?))
}

/// Read `SMSG_COOLDOWN_CHEAT`, the GM `.cooldown` reset. The client (`0x6e9730`) clears every
/// cooldown of the self or pet the guid names.
pub(super) fn read_cooldown_cheat(r: &mut impl Read) -> io::Result<u64> {
    read_u64_le(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_book_deltas_have_three_different_bodies() {
        // SMSG_LEARNED_SPELL: u16 spell + u16 slot (dropped).
        let body: Vec<u8> = [1752u16.to_le_bytes(), 0u16.to_le_bytes()].concat();
        assert_eq!(read_learned_spell(&mut &body[..]).unwrap(), 1752);

        // SMSG_REMOVED_SPELL: the bare u16, and nothing after it.
        let body = 1752u16.to_le_bytes().to_vec();
        let mut r = &body[..];
        assert_eq!(read_removed_spell(&mut r).unwrap(), 1752);
        assert!(r.is_empty(), "the removal body is two bytes, full stop");

        // SMSG_SUPERCEDED_SPELL: the pair.
        let body: Vec<u8> = [1752u16.to_le_bytes(), 1757u16.to_le_bytes()].concat();
        assert_eq!(read_superceded_spell(&mut &body[..]).unwrap(), (1752, 1757));
    }

    #[test]
    fn cooldown_bodies_golden() {
        // SMSG_SPELL_COOLDOWN: guid, then (spell, ms) pairs, no flags byte; {133, 0} = Spell.dbc.
        let body: Vec<u8> = [
            0x10u64.to_le_bytes().to_vec(),
            133u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
            5384u32.to_le_bytes().to_vec(),
            30_000u32.to_le_bytes().to_vec(),
        ]
        .concat();
        let mut r = &body[..];
        assert_eq!(
            read_spell_cooldown(&mut r).unwrap(),
            (0x10, vec![(133, 0), (5384, 30_000)])
        );
        assert!(r.is_empty());

        // SMSG_ITEM_COOLDOWN: item guid + spell id, nothing else.
        let body: Vec<u8> = [
            0x40u64.to_le_bytes().to_vec(),
            439u32.to_le_bytes().to_vec(),
        ]
        .concat();
        let mut r = &body[..];
        assert_eq!(read_item_cooldown(&mut r).unwrap(), (0x40, 439));

        // SMSG_COOLDOWN_EVENT / SMSG_CLEAR_COOLDOWN: spell id first, then the guid.
        let body: Vec<u8> = [
            1784u32.to_le_bytes().to_vec(),
            0x22u64.to_le_bytes().to_vec(),
        ]
        .concat();
        let mut r = &body[..];
        assert_eq!(read_cooldown_event(&mut r).unwrap(), (1784, 0x22));

        // SMSG_COOLDOWN_CHEAT: the lone raw u64.
        let body = 0x33u64.to_le_bytes();
        let mut r = &body[..];
        assert_eq!(read_cooldown_cheat(&mut r).unwrap(), 0x33);
    }
}
