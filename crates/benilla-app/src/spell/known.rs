//! What a unit other than the active player knows of the spells and may learn, the reference's
//! `CGUnit_C` helpers `KnowsSpell 0x60c740`, `FindSkillLineAbility 0x60c7c0`,
//! `GetSpellRequiredLevel 0x60c840` and `KnowsHigherRank 0x60c8d0` on the pet: the craft window's
//! rows ask them, and so do the cast validator's pet learn rungs (`0x6097b1`-`0x6097d9`).

use bevy::prelude::*;

use crate::net::Objects;

/// A unit as `0x468460` finds it by guid with type mask 8: its guid, and its creature family's two
/// skill lines when its creature record names a family row.
pub(crate) struct PetUnit {
    pub(crate) guid: u64,
    lines: Option<[u32; 2]>,
    /// `UNIT_FIELD_BYTES_0`'s race and class bytes (`+0x78`, `+0x79`).
    race: u8,
    class: u8,
}

/// What a pet's knowledge reads: the pet bar's guid (`0xb714a0`) and its spellbook (`0xb6f098`),
/// the creature records and the family table.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetReads<'w> {
    pub(crate) bar: Option<Res<'w, crate::ui_pet::PetBar>>,
    names: Option<Res<'w, crate::names::NameCache>>,
    families: Option<Res<'w, crate::ui_pet_stats::PetFamilyTables>>,
}

/// The pet's knowledge, resolved once per use.
pub(crate) struct PetView<'a> {
    pub(crate) bar_guid: u64,
    /// The pet book's spells, the ids `KnowsSpell` scans for the pet bar's unit (`0x60c7a0`):
    /// the bar packet's book words that pass the book's add gate (`0x4b2f90`).
    book: Vec<u32>,
    reads: &'a PetReads<'a>,
    skill_lines: &'a benilla_formats::SkillLineCatalog,
    spells: &'a benilla_formats::SpellCatalog,
}

impl<'a> PetView<'a> {
    /// The view over the pet bar, `None` without one.
    pub(crate) fn of(
        reads: &'a PetReads<'a>,
        skill_lines: &'a benilla_formats::SkillLineCatalog,
        spells: &'a benilla_formats::SpellCatalog,
    ) -> Option<Self> {
        let bar = reads.bar.as_deref()?;
        Some(Self {
            bar_guid: bar.spells.pet_guid,
            book: bar
                .spells
                .spells
                .iter()
                .map(|e| e.action())
                .filter(|&s| spells.get(s).is_some_and(|d| d.in_pet_book()))
                .collect(),
            reads,
            skill_lines,
            spells,
        })
    }

    /// The unit behind `guid`, when it is a streamed unit.
    pub(crate) fn unit(&self, objects: &Objects, guid: u64) -> Option<PetUnit> {
        let fields = objects.object(guid).filter(|_| guid != 0)?;
        if !matches!(
            fields.object_type(),
            Some(benilla_protocol::messages::ObjectType::Unit)
                | Some(benilla_protocol::messages::ObjectType::Player)
        ) {
            return None;
        }
        let lines = fields
            .object_entry()
            .and_then(|e| self.reads.names.as_deref()?.creature_record(e))
            .and_then(|r| self.reads.families.as_deref()?.families.get(r.pet_family))
            .map(|f| f.skill_lines);
        Some(PetUnit {
            guid,
            lines,
            race: fields.unit_race().unwrap_or(0),
            class: fields.unit_class().unwrap_or(0),
        })
    }

    /// `KnowsSpell` (`0x60c740`) for a unit that is not the active player: only the pet bar's
    /// unit knows anything (`0x60c77a`-`0x60c790`), the spells of its book.
    pub(crate) fn knows(&self, unit: &PetUnit, spell: u32) -> bool {
        unit.guid == self.bar_guid && self.book.contains(&spell)
    }

    /// `FindSkillLineAbility` (`0x60c7c0`) for a creature: the first row on its family's lines, in
    /// their order (`0x60c7f3`-`0x60c80a`), each a file-order scan (`0x6de0d0`). A unit with no
    /// family answers none here; the reference indexes a unit with no creature record by race
    /// and class (`0x60c813`), which no pet bar holds.
    pub(crate) fn ability(
        &self,
        unit: &PetUnit,
        spell: u32,
    ) -> Option<(u32, benilla_formats::LineAbility)> {
        unit.lines?.into_iter().find_map(|line| {
            self.skill_lines
                .line_ability(line, spell)
                .map(|row| (line, *row))
        })
    }

    /// `GetSpellRequiredLevel` (`0x60c840`): `None` without a spell record or an ability row,
    /// else the spell's `spellLevel` (`+0x74`) raised to the `SkillRaceClassInfo` level
    /// (`+0x14`) of the row's line for the unit's race and class, when one admits them
    /// (`0x60c891`-`0x60c8a4`). No pet line has such a row.
    pub(crate) fn required_level(&self, unit: &PetUnit, spell: u32) -> Option<i32> {
        let d = self.spells.get(spell)?;
        let (line, _) = self.ability(unit, spell)?;
        let level = d.spell_level as i32;
        Some(
            match self.skill_lines.race_class(line, unit.race, unit.class) {
                Some(rc) => level.max(rc.min_level as i32),
                None => level,
            },
        )
    }

    /// `KnowsHigherRank` (`0x60c8d0`): a later rank down the unit's ability rows' supersede chain
    /// (`+0x20`) that the unit knows.
    pub(crate) fn knows_higher_rank(&self, unit: &PetUnit, spell: u32) -> bool {
        let mut row = self.ability(unit, spell);
        for _ in 0..16 {
            let Some((_, r)) = row.filter(|(_, r)| r.superseded_by > 0) else {
                return false;
            };
            if self.knows(unit, r.superseded_by) {
                return true;
            }
            row = self.ability(unit, r.superseded_by);
        }
        false
    }
}
