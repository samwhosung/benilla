//! The cast sends: each `CMSG_CAST_SPELL` target shape, and the three cancels (cast, channel,
//! aura), all addressed by spell id.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_CAST_SPELL`: `None` is a self or implicit cast, `Some` a unit target; answered by
    /// `SMSG_CAST_RESULT`.
    pub fn cast_spell(&mut self, spell_id: u32, target: Option<u64>) -> Result<()> {
        self.send(
            opcode::CMSG_CAST_SPELL,
            &messages::cast_spell(spell_id, target),
        )
    }

    /// `CMSG_CANCEL_AURA`: by spell id, not slot; the server refuses passives, no-cancel spells and
    /// debuffs. No reply: the slot clears by a `UNIT_FIELD_AURA` delta.
    pub fn cancel_aura(&mut self, spell_id: u32) -> Result<()> {
        self.send(opcode::CMSG_CANCEL_AURA, &messages::cancel_aura(spell_id))
    }

    /// `CMSG_CAST_SPELL` at a GameObject: the open-lock cast on a chest, vein or herb. The server
    /// checks the skill, and a chest answers with its loot.
    pub fn cast_spell_gameobject(&mut self, spell_id: u32, go_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_CAST_SPELL,
            &messages::cast_spell_gameobject(spell_id, go_guid),
        )
    }

    /// `CMSG_CAST_SPELL` at a corpse: a corpse bit and the corpse's packed guid, a resurrection on
    /// a released player. The server resurrects the corpse's owner (`Spell.cpp:3055-3064`).
    pub fn cast_spell_corpse(
        &mut self,
        spell_id: u32,
        target: messages::CorpseTarget,
        corpse_guid: u64,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_CAST_SPELL,
            &messages::cast_spell_corpse(spell_id, target, corpse_guid),
        )
    }

    /// `CMSG_CAST_SPELL` with `TARGET_FLAG_ITEM` and the item's packed guid: an enchant on the item
    /// picked in the craft frame.
    pub fn cast_spell_item(&mut self, spell_id: u32, item_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_CAST_SPELL,
            &messages::cast_spell_item(spell_id, item_guid),
        )
    }

    /// `CMSG_CAST_SPELL` with `TARGET_FLAG_DEST_LOCATION`: a ground-targeted spell at a world
    /// point; the server checks range and line of sight (`Spell::CheckCast`).
    pub fn cast_spell_at_dest(&mut self, spell_id: u32, dest: [f32; 3]) -> Result<()> {
        self.send(
            opcode::CMSG_CAST_SPELL,
            &messages::cast_spell_at_dest(spell_id, dest),
        )
    }

    /// `CMSG_CAST_SPELL` with `TARGET_FLAG_SOURCE_LOCATION`, for a `Targets & 0x20` spell; the
    /// server centres the area effect on `src`, a world point.
    pub fn cast_spell_at_source(&mut self, spell_id: u32, src: [f32; 3]) -> Result<()> {
        self.send(
            opcode::CMSG_CAST_SPELL,
            &messages::cast_spell_at_source(spell_id, src),
        )
    }

    /// `CMSG_CANCEL_CAST`, one `u32` spell id: sent by the wand auto-repeat handoff (reference:
    /// `0x6095b8`) and when movement or Esc cancels a cast.
    pub fn cancel_cast(&mut self, spell_id: u32) -> Result<()> {
        self.send(opcode::CMSG_CANCEL_CAST, &spell_id.to_le_bytes())
    }

    /// `CMSG_CANCEL_CHANNELLING`: the 1.12 client writes the spell id, which vmangos ignores,
    /// interrupting unconditionally.
    pub fn cancel_channelling(&mut self, spell_id: u32) -> Result<()> {
        self.send(opcode::CMSG_CANCEL_CHANNELLING, &spell_id.to_le_bytes())
    }
}
