//! The pet sends. No reply answers a bar send: the 1.12 client applies the press to its own state
//! (`0xb71468`, `PET_BAR_UPDATE`) before sending, and the bar changes only on a fresh
//! `SMSG_PET_SPELLS`. The server drops a send naming a pet we do not control (`PetHandler.cpp`).

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_PET_ACTION`: press a bar slot. The server dispatches on the type byte of the echoed
    /// word (command, reaction or spell); `target_guid` is 0 for none.
    pub fn pet_action(&mut self, pet_guid: u64, packed: u32, target_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_PET_ACTION,
            &messages::pet_action(pet_guid, packed, target_guid),
        )
    }

    /// `CMSG_PET_CAST_SPELL`: a cast through the ladder whose caster is the pet or a possessed
    /// unit, not the active player (`0x6e5808`); answered by `SMSG_PET_CAST_FAILED` on a refusal.
    pub fn pet_cast_spell(
        &mut self,
        caster_guid: u64,
        spell_id: u32,
        target: messages::UseItemTarget,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_PET_CAST_SPELL,
            &messages::pet_cast_spell(caster_guid, spell_id, target),
        )
    }

    /// `CMSG_PET_STOP_ATTACK`: the Attack button's second press.
    pub fn pet_stop_attack(&mut self, pet_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_PET_STOP_ATTACK,
            &messages::pet_stop_attack(pet_guid),
        )
    }

    /// `CMSG_PET_CANCEL_AURA`: a bar click on a spell already on the pet sends only this, not
    /// `CMSG_PET_ACTION` (reference: `0x4bd25f`, predicate `0x4bcea0`). Not pre-applied: the aura
    /// leaves by a `UNIT_FIELD_AURA` delta.
    pub fn pet_cancel_aura(&mut self, pet_guid: u64, spell_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_PET_CANCEL_AURA,
            &messages::pet_cancel_aura(pet_guid, spell_id),
        )
    }

    /// `CMSG_PET_SET_ACTION`: one entry toggles autocast (the slot's word with bit 30 flipped); a
    /// drag sends one or two, and the server applies a two-entry swap atomically.
    pub fn pet_set_action(&mut self, pet_guid: u64, entries: &[(u32, u32)]) -> Result<()> {
        self.send(
            opcode::CMSG_PET_SET_ACTION,
            &messages::pet_set_action(pet_guid, entries),
        )
    }

    /// `CMSG_PET_SPELL_AUTOCAST`: `ToggleSpellAutocast` from the pet spellbook, by spell id. The
    /// bar's autocast click is `pet_set_action` instead.
    pub fn pet_spell_autocast(
        &mut self,
        pet_guid: u64,
        spell_id: u32,
        enabled: bool,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_PET_SPELL_AUTOCAST,
            &messages::pet_spell_autocast(pet_guid, spell_id, enabled),
        )
    }

    /// `CMSG_PET_ABANDON`: both Abandon and Dismiss. Not pre-applied: the pet leaving answers it,
    /// with `SMSG_PET_SPELLS` carrying a zero guid.
    pub fn pet_abandon(&mut self, pet_guid: u64) -> Result<()> {
        self.send(opcode::CMSG_PET_ABANDON, &messages::pet_abandon(pet_guid))
    }

    /// `CMSG_PET_RENAME`: not pre-applied, as the server may refuse the name
    /// (`ObjectMgr::CheckPetName`). Success bumps `UNIT_FIELD_PET_NAME_TIMESTAMP`.
    pub fn pet_rename(&mut self, pet_guid: u64, name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_PET_RENAME,
            &messages::pet_rename(pet_guid, name),
        )
    }

    /// `CMSG_PET_UNLEARN`: `ConfirmPetUnlearn()`, carrying the latched trainer guid.
    pub fn pet_unlearn(&mut self, trainer: u64) -> Result<()> {
        self.send(opcode::CMSG_PET_UNLEARN, &messages::pet_unlearn(trainer))
    }
}
