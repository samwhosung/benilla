//! `ChrClasses.dbc`, the per-class table, narrowed to the three columns the reference reads off
//! its class-indexed record table (`0xc0def4`, max id `0xc0def8`, loader `0x542360`).
//!
//! Field 4, the pet name token: the second return of `HasPetSpells` (`0x4b4410`), `"PET"` on every
//! row but the Warlock's `"DEMON"`. FrameXML resolves it with `getglobal("PET_TYPE_"..token)`
//! (`SpellBookFrame.lua:173`), so the token is a key, never display text, and is not localized.
//!
//! Field 15, the class spell family: a talent modifier applies to a spell only when its
//! `SpellFamilyName` (`Spell.dbc` column 160) equals the local player's (`GetSpellModifiers`,
//! `0x6e6b30`), cached in `[0xcecaac]` by its one non-zeroing writer, `0x6e6ca0` (called from
//! `0x5debcc`). The shipped values are vmangos `SpellFamilyNames`.
//!
//! Field 16, the relic-slot flag, read by `UnitHasRelicSlot` (`0x519e50`) and nothing else: no
//! class id is compared, the table is the data. It is set for Paladin, Shaman and Druid, and
//! `IsValidForSlot` (`0x5da1d0`) enforces it: INVSLOT 17 takes a relic (`InventoryType` 28) for
//! those three and a ranged weapon for everyone else.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const CHR_CLASSES: &str = "DBFilesClient\\ChrClasses.dbc";

/// The patch copy's column count, which `benilla-dbc` enforces as the reference loader does
/// (`0x54240e`, `0x542446`): the base archive's 16-column copy lacks the relic flag and is refused,
/// never read one column short.
const CHR_CLASSES_FIELDS: usize = 17;

/// Field 4, read at `rec + 0x10` by `HasPetSpells` (`0x4b447c`).
const PET_NAME_TOKEN_FIELD: usize = 0x10 / 4;

/// Field 16, read at `row + 0x40` by `UnitHasRelicSlot` (`0x519ebb`).
const RELIC_SLOT_FIELD: usize = 0x40 / 4;

/// Field 15, read at `+0x3c` by `0x6e6ca0`.
const SPELL_FAMILY_FIELD: usize = 0x3c / 4;

/// The literal the reference pushes when the player does not resolve (`0x846a40`). Every class but
/// the Warlock carries it too, so an unloaded table degrades invisibly.
pub const PET_NAME_TOKEN_FALLBACK: &str = "PET";

#[derive(Debug, Clone)]
struct ChrClass {
    pet_name_token: Option<String>,
    has_relic_slot: bool,
    spell_family: u32,
}

/// `ChrClasses.dbc`'s read columns by class id.
#[derive(Debug, Default, Clone)]
pub struct ChrClasses(HashMap<u32, ChrClass>);

impl ChrClasses {
    /// `HasPetSpells`' second return: always a string, never nil, and the literal
    /// [`PET_NAME_TOKEN_FALLBACK`] for a class with no row (`0x4b44a6`).
    pub fn pet_name_token(&self, class: u32) -> &str {
        self.0
            .get(&class)
            .and_then(|c| c.pet_name_token.as_deref())
            .unwrap_or(PET_NAME_TOKEN_FALLBACK)
    }

    /// Whether the class's INVSLOT 17 is a relic slot, `UnitHasRelicSlot` whole. A class with no
    /// row answers false, as the reference's bound check against `0xc0def8` falls to its nil leg.
    pub fn has_relic_slot(&self, class: u32) -> bool {
        self.0.get(&class).is_some_and(|c| c.has_relic_slot)
    }

    /// The class spell family, which the reference caches in `[0xcecaac]` and matches against a
    /// spell's `SpellFamilyName` before a talent modifier applies. A class with no row answers 0,
    /// the global's value from world-enter (`0x6e7150`) until the player resolves, which the
    /// gate's `SpellFamilyName != 0` conjunct makes match nothing.
    pub fn spell_family(&self, class: u32) -> u32 {
        self.0.get(&class).map_or(0, |c| c.spell_family)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("ChrClasses");
    for i in 0..CHR_CLASSES_FIELDS {
        // Only the token is a string; the rest, the class name block from field 5 included, stay
        // opaque dwords.
        let ty = if i == PET_NAME_TOKEN_FIELD {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load `ChrClasses.dbc`'s read columns from the patch chain.
pub fn load_chr_classes(chain: &mut Chain) -> Result<ChrClasses> {
    let bytes = chain
        .read_file(CHR_CLASSES)
        .with_context(|| format!("reading {CHR_CLASSES}"))?;
    let rs = parse(&bytes, schema(), "ChrClasses.dbc")?;
    let mut by_id = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        by_id.insert(
            id,
            ChrClass {
                pet_name_token: str_at(&rs, r, PET_NAME_TOKEN_FIELD),
                has_relic_slot: u32_at(r, RELIC_SLOT_FIELD).is_some_and(|v| v != 0),
                spell_family: u32_at(r, SPELL_FAMILY_FIELD).unwrap_or(0),
            },
        );
    }
    Ok(ChrClasses(by_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> Option<crate::chain::Chain> {
        let data = crate::wow_data_or_skip!(None);
        Some(crate::open_chain(&data).expect("open chain"))
    }

    /// Field 5 holds the class name, so a one-column slip would blank the spellbook tab silently.
    #[test]
    fn warlocks_pet_is_a_demon_and_everyone_elses_is_a_pet() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        assert!(!t.is_empty());
        assert_eq!(t.pet_name_token(9), "DEMON", "Warlock");
        for (class, who) in [(1, "Warrior"), (3, "Hunter"), (11, "Druid")] {
            assert_eq!(t.pet_name_token(class), "PET", "{who}");
        }
        // 6 and 10 have no row in 1.12; the reference's out-of-range arm answers the literal.
        assert_eq!(t.pet_name_token(6), PET_NAME_TOKEN_FALLBACK);
        assert_eq!(t.pet_name_token(0), PET_NAME_TOKEN_FALLBACK);
    }

    #[test]
    fn libram_totem_and_idol_are_the_three_relic_classes() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        for (class, who) in [(2, "Paladin"), (7, "Shaman"), (11, "Druid")] {
            assert!(t.has_relic_slot(class), "{who} carries a relic");
        }
        for (class, who) in [
            (1, "Warrior"),
            (3, "Hunter"),
            (4, "Rogue"),
            (5, "Priest"),
            (8, "Mage"),
            (9, "Warlock"),
        ] {
            assert!(!t.has_relic_slot(class), "{who} wields a ranged weapon");
        }
        // No row: the reference's nil leg.
        assert!(!t.has_relic_slot(6));
        assert!(!t.has_relic_slot(0));
    }

    /// The whole column pins field 15: nine distinct values matching vmangos `SpellFamilyNames`
    /// are a shape no neighbour has (field 14 is the class's file name, field 16 the relic flag).
    #[test]
    fn every_class_row_carries_its_vmangos_spell_family() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        for (class, family, who) in [
            (1u32, 4u32, "Warrior"),
            (2, 10, "Paladin"),
            (3, 9, "Hunter"),
            (4, 8, "Rogue"),
            (5, 6, "Priest"),
            (7, 11, "Shaman"),
            (8, 3, "Mage"),
            (9, 5, "Warlock"),
            (11, 7, "Druid"),
        ] {
            assert_eq!(t.spell_family(class), family, "{who}");
        }
        // No row: the reference's zeroed global, which the gate's first conjunct refuses.
        assert_eq!(t.spell_family(6), 0);
        assert_eq!(t.spell_family(0), 0);
    }
}
