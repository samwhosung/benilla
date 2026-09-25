//! Pet messages: the pet action bar's state packet and the client verbs that drive it. The bar's
//! contents are the server's; its state (lit command, lit reaction, autocast) is the client's,
//! applied on the press, because the server never answers these verbs.

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_u16_le, read_u32_le, read_u64_le, read_u8, Vector3d};

/// Pet bar slots: vmangos `MAX_UNIT_ACTION_BAR_INDEX`, FrameXML's `NUM_PET_ACTION_SLOTS`.
pub const PET_ACTION_SLOTS: usize = 10;

/// vmangos `ActiveStates` (`UnitDefines.h:724-732`): the server's top byte of a packed word; the
/// client decodes the masked [`PetActionEntry::kind`] instead. Passive: shown, never clickable.
pub const PET_ACT_PASSIVE: u8 = 0x01;
/// A castable spell with autocast off: `0x01 | ` [`PET_AUTOCAST_ALLOWED`]`>>24`.
pub const PET_ACT_DISABLED: u8 = 0x81;
/// A castable spell with autocast on: `0x81 | ` [`PET_AUTOCAST_ON`]`>>24`.
pub const PET_ACT_ENABLED: u8 = 0xC1;
/// A command token: the action is a [`PET_COMMAND_STAY`]-family value, not a spell id.
pub const PET_ACT_COMMAND: u8 = 0x07;
/// A reaction token: the action is a [`PET_REACT_PASSIVE`]-family value, not a spell id.
pub const PET_ACT_REACTION: u8 = 0x06;

/// First spell-branch slot type. The client's type is the top byte `& 0x3F` (`0x4bdccd`), which
/// folds all three spell `ActiveStates` onto 1; types 6 and 7 are reaction and command.
pub const PET_TYPE_SPELL_FIRST: u8 = 1;
/// The last type the client's jump table routes to the spell branch.
pub const PET_TYPE_SPELL_LAST: u8 = 5;

/// Bit 31 of a packed word: the slot may autocast (`0x4bdd65`), given a `Spell.dbc` record.
pub const PET_AUTOCAST_ALLOWED: u32 = 0x8000_0000;
/// Bit 30: autocast is running (`0x4bdda4`); `TogglePetAutocast` flips it before the send.
pub const PET_AUTOCAST_ON: u32 = 0x4000_0000;

/// vmangos `CommandStates` (`UnitDefines.h:755-761`): a [`PET_ACT_COMMAND`] slot's action.
pub const PET_COMMAND_STAY: u32 = 0;
pub const PET_COMMAND_FOLLOW: u32 = 1;
pub const PET_COMMAND_ATTACK: u32 = 2;
pub const PET_COMMAND_DISMISS: u32 = 3;

/// vmangos `ReactStates` (`UnitDefines.h:734-739`): a [`PET_ACT_REACTION`] slot's action.
pub const PET_REACT_PASSIVE: u32 = 0;
pub const PET_REACT_DEFENSIVE: u32 = 1;
pub const PET_REACT_AGGRESSIVE: u32 = 2;

/// Bit 27 of [`PetSpells::state`], vmangos's fourth byte `0x8` (`Player.cpp:17536`): the bar is
/// disabled, its lit reaction reads Passive and it is unusable, but it still draws (`0x4bd08d`).
pub const PET_STATE_BAR_DISABLED: u32 = 0x0800_0000;

/// The pet's `UNIT_FIELD_FLAGS` bits that make its bar unusable: stunned, confused, fleeing
/// (`0x4bd075`). `UNIT_FLAG_POSSESSED` is not among them: a possessed unit's bar must work.
pub const PET_UNUSABLE_UNIT_FLAGS: u32 = 0x0004_0000 | 0x0040_0000 | 0x0080_0000;

/// Permanent-cooldown marker OR-ed into a pet cooldown's category duration (vmangos
/// `Unit.cpp:11270-11271`); strip it before using the number as a duration.
pub const PET_COOLDOWN_PERMANENT: u32 = 0x0800_0000;

/// One packed pet action word, as the client decodes it (`0x4bdcc7`/`0x4bdccd`/`0x4bdce3`): the
/// action in bits 0-15, the type in 24-29, autocast allowed in 31 and on in 30; bits 16-23 are
/// never read. The raw word is kept because the client echoes it verbatim in [`pet_action`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PetActionEntry {
    pub packed: u32,
}

impl PetActionEntry {
    /// Bits 0-15: the spell id, command state or react state.
    pub fn action(self) -> u32 {
        self.packed & 0xFFFF
    }
    /// The client's slot type, bits 24-29.
    pub fn kind(self) -> u8 {
        ((self.packed >> 24) & 0x3F) as u8
    }
    /// Client types 1-5 take the spell branch. Deviation: a type outside 1-7 reads as nothing
    /// rather than the reference's default arm, which under-pushes its returns.
    pub fn is_spell(self) -> bool {
        (PET_TYPE_SPELL_FIRST..=PET_TYPE_SPELL_LAST).contains(&self.kind())
    }
    /// Empty is a zero word (`0x4bdcbd`). vmangos's unused slots, `0x8100_0000`, take the spell
    /// branch instead and come back nil, since spell 0 has no `Spell.dbc` record.
    pub fn is_empty(self) -> bool {
        self.packed == 0
    }
    /// Bit 31; the client also requires a `Spell.dbc` record, which the catalog holder checks.
    pub fn autocast_allowed(self) -> bool {
        self.packed & PET_AUTOCAST_ALLOWED != 0
    }
    /// Bit 30: autocast is running.
    pub fn autocast_on(self) -> bool {
        self.packed & PET_AUTOCAST_ON != 0
    }
    /// Bit 30 set or cleared: `TogglePetAutocast`'s in-place flip (`0x4bcbff`), before the send.
    pub fn with_autocast(self, on: bool) -> Self {
        Self {
            packed: if on {
                self.packed | PET_AUTOCAST_ON
            } else {
                self.packed & !PET_AUTOCAST_ON
            },
        }
    }
}

impl From<u32> for PetActionEntry {
    fn from(packed: u32) -> Self {
        Self { packed }
    }
}

/// One decoded entry of `SMSG_PET_SPELLS`' trailing cooldown block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PetSpellCooldown {
    pub spell_id: u32,
    pub category: u16,
    pub spell_cd_ms: u32,
    /// The category remainder, possibly carrying [`PET_COOLDOWN_PERMANENT`] in its top bits.
    pub category_cd_ms: u32,
}

/// `SMSG_PET_SPELLS`: the pet action bar's entire state in one body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PetSpells {
    /// The pet, possessed minion or charmed creature; zero means the bar is gone.
    pub pet_guid: u64,
    /// The possess or charm aura's remaining ms; 0 for a real pet.
    pub duration_ms: u32,
    /// The mode word, one dword as the client keeps it (`[0xb71468]`): react state in byte 0,
    /// command state in byte 1, [`PET_STATE_BAR_DISABLED`] at bit 27. Do not split it into bytes.
    pub state: u32,
    /// The ten bar slots, in bar order.
    pub bar: [PetActionEntry; PET_ACTION_SLOTS],
    /// The pet spellbook, packed like the bar; only a permanent pet has one (`Player.cpp:17547`).
    pub spells: Vec<PetActionEntry>,
    pub cooldowns: Vec<PetSpellCooldown>,
}

impl PetSpells {
    /// Byte 0 of [`Self::state`], a [`PET_REACT_PASSIVE`]-family value.
    pub fn react_state(&self) -> u32 {
        self.state & 0xFF
    }
    /// `state >> 8`, unmasked like the client's compare (`0x4bdf0f`): with bit 27 set it matches
    /// no command state, so every command button goes dark. Masking it would lose that.
    pub fn command_state(&self) -> u32 {
        self.state >> 8
    }
    /// Bit 27: the server has disabled the bar.
    pub fn bar_disabled(&self) -> bool {
        self.state & PET_STATE_BAR_DISABLED != 0
    }
}

/// Read `SMSG_PET_SPELLS` (vmangos `Player::PetSpellInitialize`, `Player.cpp:17519-17561`):
/// `u64 petGuid, u32 duration, u32 state, u32 bar[10], u8 count, u32 spells[count]`, then the
/// cooldown block. A lone guid is the teardown; any longer body cut short is an error.
pub(super) fn read_pet_spells(r: &mut &[u8]) -> io::Result<PetSpells> {
    let pet_guid = read_u64_le(r)?;
    if r.is_empty() {
        // The teardown; vmangos always writes a zero guid here.
        return Ok(PetSpells {
            pet_guid,
            ..Default::default()
        });
    }
    let duration_ms = read_u32_le(r)?;
    let state = read_u32_le(r)?;

    let mut bar = [PetActionEntry::default(); PET_ACTION_SLOTS];
    for slot in &mut bar {
        *slot = read_u32_le(r)?.into();
    }

    let spell_count = read_u8(r)?;
    // A `u8` count with no tighter server bound (`Player.cpp:17544`).
    let mut spells = Vec::with_capacity(capacity_hint(spell_count, usize::from(u8::MAX)));
    for _ in 0..spell_count {
        spells.push(read_u32_le(r)?.into());
    }

    let cooldowns = read_cooldown_block(r)?;

    Ok(PetSpells {
        pet_guid,
        duration_ms,
        state,
        bar,
        spells,
        cooldowns,
    })
}

/// The trailing cooldown block. The client reads `u8 count` then 12-byte entries
/// `{u16 spell, u16 category, u32, u32}` (`0x4bda58`); vmangos writes `u16 count` then 14-byte
/// entries with a `u32` spell (`Unit.cpp:11245-11278`). Deviation: both are read, told apart
/// exactly by the tail length (`12n` vs `1 + 14n`), since the 1.12 client mis-parses vmangos's.
fn read_cooldown_block(r: &mut &[u8]) -> io::Result<Vec<PetSpellCooldown>> {
    let count = usize::from(read_u8(r)?);
    // At `count == 0` vmangos still leaves its `u16` count's high byte; it is consumed so the
    // decode-length check sees no tail.
    let vmangos = r.len() == 1 + 14 * count;
    if vmangos {
        let _high_byte = read_u8(r)?;
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    // `count` came off a `u8`; no tighter bound exists on either producer.
    let mut cooldowns = Vec::with_capacity(capacity_hint(count, usize::from(u8::MAX)));
    for _ in 0..count {
        let spell_id = if vmangos {
            read_u32_le(r)?
        } else {
            u32::from(read_u16_le(r)?)
        };
        cooldowns.push(PetSpellCooldown {
            spell_id,
            category: read_u16_le(r)?,
            spell_cd_ms: read_u32_le(r)?,
            category_cd_ms: read_u32_le(r)?,
        });
    }
    Ok(cooldowns)
}

/// `SMSG_PET_MODE`: the state word without the bar, for a command or reaction change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PetMode {
    pub pet_guid: u64,
    /// The same dword as [`PetSpells::state`]; the client stores both through `0x4bc930`.
    pub state: u32,
}

/// Read `SMSG_PET_MODE`: `u64 petGuid, u32 state` (`0x4bdb10`; vmangos `Packets/Pet.cpp:104-111`).
pub(super) fn read_pet_mode(r: &mut impl Read) -> io::Result<PetMode> {
    Ok(PetMode {
        pet_guid: read_u64_le(r)?,
        state: read_u32_le(r)?,
    })
}

/// Read `SMSG_PET_ACTION_FEEDBACK` (vmangos `Packets/Pet.cpp:90-93`): one reason byte.
pub(super) fn read_pet_action_feedback(r: &mut impl Read) -> io::Result<u8> {
    read_u8(r)
}

/// Read `SMSG_PET_TAME_FAILURE` (vmangos `Packets/Pet.cpp:122-125`): one reason byte.
pub(super) fn read_pet_tame_failure(r: &mut impl Read) -> io::Result<u8> {
    read_u8(r)
}

/// The `GlobalStrings.lua` key filling `ERR_TAME_FAILED`'s `%s`: the reference's table
/// (`0x6e6ac0`) covers `1..=11`, and anything else, vmangos's 12 included, is the default arm.
pub fn pet_tame_failure_key(reason: u8) -> &'static str {
    match reason {
        1 => "PETTAME_INVALIDCREATURE",
        2 => "PETTAME_TOOMANY",
        3 => "PETTAME_CREATUREALREADYOWNED",
        4 => "PETTAME_NOTTAMEABLE",
        5 => "PETTAME_ANOTHERSUMMONACTIVE",
        6 => "PETTAME_UNITSCANTTAME",
        7 => "PETTAME_NOPETAVAILABLE",
        8 => "PETTAME_INTERNALERROR",
        9 => "PETTAME_TOOHIGHLEVEL",
        10 => "PETTAME_DEAD",
        11 => "PETTAME_NOTDEAD",
        _ => "PETTAME_UNKNOWNERROR",
    }
}

/// Sound selector for an accepted order (vmangos `PET_TALK_SPECIAL_SPELL`, `PetHandler.cpp:523`).
pub const PET_TALK_ORDER: u32 = 0;
/// Sound selector for an attack order (vmangos `PET_TALK_ATTACK`, `Unit.cpp:8961`).
pub const PET_TALK_ATTACK: u32 = 1;

/// Read `SMSG_PET_ACTION_SOUND`: `u64 petGuid, u32 selector` (`0x6040ca`, vmangos
/// `Packets/Pet.cpp:96-100`). The selector is a `PET_TALK_*` value, not a `SoundEntries` id: the
/// sound comes off the pet's `CreatureSoundData` row.
pub(super) fn read_pet_action_sound(r: &mut impl Read) -> io::Result<(u64, u32)> {
    Ok((read_u64_le(r)?, read_u32_le(r)?))
}

/// Read `SMSG_PET_DISMISS_SOUND`: `u32 creatureModelDataId, f32 x, y, z` (reader `0x604140`;
/// vmangos never sends it). The handler's `+1.0` on `z` is a play detail, not applied here.
pub(super) fn read_pet_dismiss_sound(r: &mut impl Read) -> io::Result<(u32, Vector3d)> {
    Ok((read_u32_le(r)?, Vector3d::read(r)?))
}

/// Read `SMSG_PET_CAST_FAILED` (vmangos `Packets/Pet.cpp:127-132`): `u32` spell, `u8` status,
/// `u8` reason, in `SMSG_CAST_RESULT`'s vocabulary (2 is a failure) but with no argument words.
pub(super) fn read_pet_cast_failed(r: &mut impl Read) -> io::Result<(u32, super::CastOutcome)> {
    let spell_id = read_u32_le(r)?;
    let status = read_u8(r)?;
    let outcome = if status == 2 {
        super::CastOutcome::Failed {
            reason: read_u8(r)?,
            arg: None,
        }
    } else {
        super::CastOutcome::Ok
    };
    Ok((spell_id, outcome))
}

/// Body of `CMSG_PET_ACTION` (opcode 373, vmangos `Packets/Pet.cpp:9-14`): `u64` pet guid, the
/// slot's `u32` word echoed verbatim, `u64` target guid (0 when the action needs none).
pub fn pet_action(pet_guid: u64, packed: u32, target_guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(20);
    body.extend_from_slice(&pet_guid.to_le_bytes());
    body.extend_from_slice(&packed.to_le_bytes());
    body.extend_from_slice(&target_guid.to_le_bytes());
    body
}

/// Body of `CMSG_PET_STOP_ATTACK` (opcode 746, vmangos `Packets/Pet.cpp:33-36`): the pet guid,
/// sent by the Attack button's second press (`PetActionBarFrame.lua:258-262`).
pub fn pet_stop_attack(pet_guid: u64) -> Vec<u8> {
    pet_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_PET_CANCEL_AURA` (opcode 619, sender `0x4bd25f`): `u64` pet guid, `u32` spell.
/// The server drops it unless the guid is our pet or charm and answers a dead pet with
/// `FEEDBACK_PET_DEAD` (vmangos `SpellHandler.cpp:407-432`).
pub fn pet_cancel_aura(pet_guid: u64, spell_id: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&pet_guid.to_le_bytes());
    body.extend_from_slice(&spell_id.to_le_bytes());
    body
}

/// Body of `CMSG_PET_SPELL_AUTOCAST` (opcode 755, sender `0x4bcd5f`): `u64` guid, `u32` spell,
/// `u8` state. The pet spellbook's autocast verb, with no reply; the bar's is [`pet_set_action`].
pub fn pet_spell_autocast(pet_guid: u64, spell_id: u32, enabled: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(13);
    body.extend_from_slice(&pet_guid.to_le_bytes());
    body.extend_from_slice(&spell_id.to_le_bytes());
    body.push(u8::from(enabled));
    body
}

/// Body of `CMSG_PET_SET_ACTION` (opcode 372): `u64` pet guid, then one or two
/// `(u32 position, u32 packed)` pairs, counted by body size alone (24 bytes is two, vmangos
/// `Packets/Pet.cpp:52-62`), so one pair must not be padded. It carries the bar's autocast toggle
/// (one pair, bit 30 flipped, `0x4bcc1e`) and its drag (`0x4bc9a0`), whose optional first pair
/// relocates the target slot's occupant; vmangos requires it when a command or reaction moves.
pub fn pet_set_action(pet_guid: u64, entries: &[(u32, u32)]) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 8 * entries.len());
    body.extend_from_slice(&pet_guid.to_le_bytes());
    for &(position, packed) in entries {
        body.extend_from_slice(&position.to_le_bytes());
        body.extend_from_slice(&packed.to_le_bytes());
    }
    body
}

/// Body of `CMSG_PET_ABANDON` (opcode 374, vmangos `Packets/Pet.cpp:16-19`): the pet guid. The
/// menu's Abandon and Dismiss both send it; the server deletes a hunter pet and unsummons others.
pub fn pet_abandon(pet_guid: u64) -> Vec<u8> {
    pet_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_PET_RENAME` (opcode 375, vmangos `Packets/Pet.cpp:21-25`): the pet guid, then
/// the name NUL-terminated. The 12-letter cap is the popup's; the server refuses a bad name with
/// `SMSG_PET_NAME_INVALID` rather than truncating.
pub fn pet_rename(pet_guid: u64, name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(9 + name.len());
    body.extend_from_slice(&pet_guid.to_le_bytes());
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hunter pet's `SMSG_PET_SPELLS` as vmangos builds it, with one spell and one cooldown.
    fn pet_spells_body() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0xF140_0000_0000_002Au64.to_le_bytes()); // pet guid
        b.extend_from_slice(&0u32.to_le_bytes()); // duration (0 = a real pet)
        b.push(PET_REACT_DEFENSIVE as u8);
        b.push(PET_COMMAND_FOLLOW as u8);
        b.push(0);
        b.push(0); // enabled
                   // The bar is CharmInfo::InitPetActionBar's default, Claw 3010 autocasting.
        for packed in [
            PET_COMMAND_ATTACK | (u32::from(PET_ACT_COMMAND) << 24),
            PET_COMMAND_FOLLOW | (u32::from(PET_ACT_COMMAND) << 24),
            PET_COMMAND_STAY | (u32::from(PET_ACT_COMMAND) << 24),
            3010 | (u32::from(PET_ACT_ENABLED) << 24),
            u32::from(PET_ACT_DISABLED) << 24, // empty
            u32::from(PET_ACT_DISABLED) << 24, // empty
            u32::from(PET_ACT_DISABLED) << 24, // empty
            PET_REACT_AGGRESSIVE | (u32::from(PET_ACT_REACTION) << 24),
            PET_REACT_DEFENSIVE | (u32::from(PET_ACT_REACTION) << 24),
            PET_REACT_PASSIVE | (u32::from(PET_ACT_REACTION) << 24),
        ] {
            b.extend_from_slice(&packed.to_le_bytes());
        }
        b.push(1); // spell count
        b.extend_from_slice(&(3010u32 | (u32::from(PET_ACT_ENABLED) << 24)).to_le_bytes());
        // The cooldown block in vmangos's layout: u16 count, u32 spell id (14-byte entries).
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&3010u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes()); // category
        b.extend_from_slice(&4_500u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b
    }

    /// The same packet with the cooldown block in the client's layout: u8 count, 12-byte entries.
    fn pet_spells_body_client_cooldowns() -> Vec<u8> {
        let mut b = pet_spells_body();
        b.truncate(b.len() - 16); // drop vmangos's u16 count + its one 14-byte entry
        b.push(1); // u8 count
        b.extend_from_slice(&3010u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes()); // category
        b.extend_from_slice(&4_500u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b
    }

    #[test]
    fn pet_spells_golden() {
        let body = pet_spells_body();
        let mut r = &body[..];
        let p = read_pet_spells(&mut r).unwrap();
        assert!(r.is_empty(), "the body is fully consumed");

        assert_eq!(p.pet_guid, 0xF140_0000_0000_002A);
        assert_eq!(p.duration_ms, 0);
        assert_eq!(p.react_state(), PET_REACT_DEFENSIVE);
        assert_eq!(p.command_state(), PET_COMMAND_FOLLOW);
        assert!(!p.bar_disabled());

        assert_eq!(p.bar[0].kind(), PET_ACT_COMMAND);
        assert_eq!(p.bar[0].action(), PET_COMMAND_ATTACK);
        assert!(!p.bar[0].is_spell());
        assert_eq!(p.bar[9].kind(), PET_ACT_REACTION);
        assert_eq!(p.bar[9].action(), PET_REACT_PASSIVE);

        // The server sent 0xC1; the client's type is 1, with autocast in bits 31/30.
        assert_eq!(p.bar[3].kind(), PET_TYPE_SPELL_FIRST);
        assert_eq!(p.bar[3].action(), 3010);
        assert!(p.bar[3].is_spell() && !p.bar[3].is_empty());
        assert!(p.bar[3].autocast_allowed() && p.bar[3].autocast_on());

        // vmangos's unused slot (0, ACT_DISABLED) is not empty; it takes the spell branch.
        assert!(!p.bar[4].is_empty());
        assert!(p.bar[4].is_spell() && p.bar[4].action() == 0);
        assert!(
            PetActionEntry::default().is_empty(),
            "zero IS the empty word"
        );

        assert_eq!(p.spells.len(), 1);
        assert_eq!(p.spells[0].action(), 3010);
        assert_eq!(
            p.cooldowns,
            vec![PetSpellCooldown {
                spell_id: 3010,
                category: 0,
                spell_cd_ms: 4_500,
                category_cd_ms: 0,
            }]
        );
    }

    #[test]
    fn both_cooldown_layouts_decode_identically() {
        let from_vmangos = read_pet_spells(&mut &pet_spells_body()[..]).unwrap();
        let body = pet_spells_body_client_cooldowns();
        let mut r = &body[..];
        let from_client = read_pet_spells(&mut r).unwrap();
        assert!(r.is_empty(), "the client-layout body is fully consumed");
        assert_eq!(from_client.cooldowns, from_vmangos.cooldowns);
        assert_eq!(from_client, from_vmangos);
    }

    /// No cooldowns: the client's form is one byte, vmangos's two, and both are fully consumed.
    #[test]
    fn an_empty_cooldown_block_reads_either_way() {
        let mut client = &[0u8][..];
        assert!(read_cooldown_block(&mut client).unwrap().is_empty());
        assert!(client.is_empty());
        let mut vmangos = &[0u8, 0][..];
        assert!(read_cooldown_block(&mut vmangos).unwrap().is_empty());
        assert!(
            vmangos.is_empty(),
            "vmangos's count high byte is consumed, not left as a tail"
        );
    }

    /// A zero-guid body is the only signal that the bar is gone, so it must not be an error.
    #[test]
    fn the_guid_only_body_is_the_teardown() {
        let body = 0u64.to_le_bytes();
        let mut r = &body[..];
        let p = read_pet_spells(&mut r).unwrap();
        assert_eq!(p, PetSpells::default());
        assert_eq!(p.pet_guid, 0);
    }

    /// Cut after the first bar slot: an error, not a half-read bar.
    #[test]
    fn a_truncated_body_errors() {
        let body = pet_spells_body();
        let mut r = &body[..20];
        assert!(read_pet_spells(&mut r).is_err());
    }

    #[test]
    fn pet_mode_and_the_client_verbs_golden() {
        // SMSG_PET_MODE: vmangos's fourth state byte 0x8 is the client's bit 27.
        let mut body = 0x2Au64.to_le_bytes().to_vec();
        body.extend_from_slice(&[2, 0, 0, 0x8]);
        let mut r = &body[..];
        let mode = read_pet_mode(&mut r).unwrap();
        assert_eq!(mode.pet_guid, 0x2A);
        assert_eq!(mode.state, 0x0800_0002);
        assert_eq!(mode.state & PET_STATE_BAR_DISABLED, PET_STATE_BAR_DISABLED);

        let packed = 3010 | (u32::from(PET_ACT_ENABLED) << 24);
        assert_eq!(
            pet_action(0x2A, packed, 0x99),
            [
                0x2Au64.to_le_bytes().to_vec(),
                packed.to_le_bytes().to_vec(),
                0x99u64.to_le_bytes().to_vec(),
            ]
            .concat()
        );

        assert_eq!(pet_stop_attack(0x2A), 0x2Au64.to_le_bytes());

        assert_eq!(
            pet_cancel_aura(0x2A, 2645),
            [
                0x2Au64.to_le_bytes().to_vec(),
                2645u32.to_le_bytes().to_vec()
            ]
            .concat()
        );
        assert_eq!(pet_cancel_aura(0x2A, 2645).len(), 12);

        // CMSG_PET_SET_ACTION: the server counts entries by body size, so 16 bytes or exactly 24.
        assert_eq!(pet_set_action(0x2A, &[(3, packed)]).len(), 16);
        assert_eq!(pet_set_action(0x2A, &[(3, packed), (4, 0)]).len(), 24);

        assert_eq!(pet_abandon(0x2A), 0x2Au64.to_le_bytes());

        // CMSG_PET_RENAME: guid then a NUL-terminated name, the terminator being its only framing.
        assert_eq!(
            pet_rename(0x2A, "Bruce"),
            [&0x2Au64.to_le_bytes()[..], b"Bruce\0"].concat()
        );
        assert_eq!(
            pet_rename(0x2A, "").len(),
            9,
            "an empty name is still framed"
        );
    }

    #[test]
    fn the_autocast_flip_moves_only_bit_30() {
        let on = PetActionEntry::from(3010 | (u32::from(PET_ACT_ENABLED) << 24));
        assert!(on.autocast_allowed() && on.autocast_on());

        let off = on.with_autocast(false);
        assert_eq!(off.packed, 3010 | (u32::from(PET_ACT_DISABLED) << 24));
        assert_eq!(off.action(), 3010);
        assert_eq!(off.kind(), PET_TYPE_SPELL_FIRST, "the TYPE never moves");
        assert!(off.autocast_allowed() && !off.autocast_on());
        assert_eq!(off.with_autocast(true), on);

        // Bit 31 is the server's to grant; flipping bit 30 on a passive spell does not forge it.
        let passive = PetActionEntry::from(3010 | (u32::from(PET_ACT_PASSIVE) << 24));
        assert!(!passive.with_autocast(true).autocast_allowed());
    }

    #[test]
    fn the_type_mask_collapses_the_spell_states() {
        for act in [PET_ACT_PASSIVE, PET_ACT_DISABLED, PET_ACT_ENABLED] {
            let e = PetActionEntry::from(3010 | (u32::from(act) << 24));
            assert_eq!(e.kind(), PET_TYPE_SPELL_FIRST, "{act:#04x} masks to 1");
            assert!(e.is_spell());
        }
        assert_eq!(
            PetActionEntry::from(u32::from(PET_ACT_COMMAND) << 24).kind(),
            PET_ACT_COMMAND,
            "the token types are below the mask, so they survive it unchanged"
        );
        assert_eq!(
            PetActionEntry::from(u32::from(PET_ACT_REACTION) << 24).kind(),
            PET_ACT_REACTION
        );

        // A type outside 1-7 is neither a spell nor a token.
        let odd = PetActionEntry::from(3010 | (0x33u32 << 24));
        assert!(!odd.is_spell());
        assert!(odd.kind() != PET_ACT_COMMAND && odd.kind() != PET_ACT_REACTION);
    }
}

/// `SMSG_PET_UNLEARN_CONFIRM` (`0x5e4a26`): the trainer's guid and the unlearn cost in copper,
/// which the reference latches before firing `CONFIRM_PET_UNLEARN(cost)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetUnlearnConfirm {
    pub trainer: u64,
    pub cost: u32,
}

/// Parse `SMSG_PET_UNLEARN_CONFIRM`: `u64 trainerGuid`, `u32 costCopper`.
pub(super) fn read_pet_unlearn_confirm(
    r: &mut impl std::io::Read,
) -> std::io::Result<PetUnlearnConfirm> {
    Ok(PetUnlearnConfirm {
        trainer: crate::wire::read_u64_le(r)?,
        cost: crate::wire::read_u32_le(r)?,
    })
}

/// Body of `CMSG_PET_UNLEARN` (the confirm arm of `0x5dfba0`): the latched trainer guid.
pub fn pet_unlearn(trainer: u64) -> Vec<u8> {
    trainer.to_le_bytes().to_vec()
}
