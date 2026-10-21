//! `ChrClasses.dbc`, the per-class table, narrowed to the four columns the reference reads off
//! its class-indexed record table (`0xc0def4`, max id `0xc0def8`, loader `0x542360`).
//!
//! Field 2, the damage bonus stat: `GetDamageBonusStat` (`0x48b520`) reads it at `rec + 8` for the
//! active player's class byte and answers it plus one, a 1-based `UnitStat` index. Strength (0)
//! for every row but the Hunter's and the Rogue's Agility (1).
//!
//! Field 4, the pet name token: the second return of `HasPetSpells` (`0x4b4410`), `"PET"` on every
//! row but the Warlock's `"DEMON"`. FrameXML resolves it with `getglobal("PET_TYPE_"..token)`
//! (`SpellBookFrame.lua:173`), so the token is a key, never display text, and is not localized.
//!
//! Field 5, the class name: the `Name_Lang` block (`+0x14`, eight locale columns), which
//! `GetCharacterInfo` (`0x4732a0`) returns as the class (`0x473368`).
//!
//! Field 14, the class file string (`+0x38`, `"WARRIOR"`), which the create screen's
//! `GetClassesForRace` (`0x471594`) and `GetSelectedClass` (`0x47171d`) return after the name.
//!
//! Field 15, the class spell family: a talent modifier applies to a spell only when its
//! `SpellFamilyName` (`Spell.dbc` column 160) equals the local player's (`GetSpellModifiers`,
//! `0x6e6b30`), cached in `[0xcecaac]` by its one non-zeroing writer, `0x6e6ca0` (called from
//! `0x5debcc`). The shipped values are vmangos `SpellFamilyNames`.
//!
//! Field 16, the relic-slot flag, read at the row's `+0x40` by `UnitHasRelicSlot` (`0x519ebb`),
//! the slot check (`0x5ea7c8`) and the sheathe toggle and setter (`0x5eb639`, `0x611d41`): no
//! class id is compared, the table is the data. It is set for Paladin, Shaman and Druid, and
//! `IsValidForSlot` (`0x5da1d0`), which the slot check calls, enforces it: INVSLOT 17 takes a relic
//! (`InventoryType` 28) for those three and a ranged weapon for everyone else.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{loc_str, parse, str_at, u32_at};

const CHR_CLASSES: &str = "DBFilesClient\\ChrClasses.dbc";

/// The patch copy's column count, which `benilla-dbc` enforces as the reference loader does
/// (`0x54240e`, `0x542446`): the base archive's 16-column copy lacks the relic flag and is refused,
/// never read one column short.
const CHR_CLASSES_FIELDS: usize = 17;

/// Field 2, read at `rec + 8` by `GetDamageBonusStat` (`0x48b569`).
const DAMAGE_BONUS_STAT_FIELD: usize = 0x8 / 4;

/// Field 5, the first of the eight `Name_Lang` columns (`rec + 0x14`, `0x473368`).
const NAME_FIELD: usize = 0x14 / 4;

/// Field 4, read at `rec + 0x10` by `HasPetSpells` (`0x4b447c`).
const PET_NAME_TOKEN_FIELD: usize = 0x10 / 4;

/// Field 16, read at `row + 0x40` by `UnitHasRelicSlot` (`0x519ebb`).
const RELIC_SLOT_FIELD: usize = 0x40 / 4;

/// Field 14, read at `+0x38` by `GetClassesForRace` (`0x471594`).
const FILE_FIELD: usize = 0x38 / 4;

/// Field 15, read at `+0x3c` by `0x6e6ca0`.
const SPELL_FAMILY_FIELD: usize = 0x3c / 4;

/// The literal the reference pushes when the player does not resolve (`0x846a40`). Every class but
/// the Warlock carries it too, so an unloaded table degrades invisibly.
pub const PET_NAME_TOKEN_FALLBACK: &str = "PET";

#[derive(Debug, Clone)]
struct ChrClass {
    damage_bonus_stat: u32,
    pet_name_token: Option<String>,
    name: Option<String>,
    file: Option<String>,
    has_relic_slot: bool,
    spell_family: u32,
}

/// `ChrClasses.dbc`'s read columns by class id.
#[derive(Debug, Default, Clone)]
pub struct ChrClasses(HashMap<u32, ChrClass>);

impl ChrClasses {
    /// A table of `(id, name, file)` rows, the other columns at their zero values.
    pub fn from_rows<'a>(rows: impl IntoIterator<Item = (u32, &'a str, &'a str)>) -> Self {
        Self(
            rows.into_iter()
                .map(|(id, name, file)| {
                    let row = ChrClass {
                        damage_bonus_stat: 0,
                        pet_name_token: None,
                        name: Some(name.to_string()).filter(|n| !n.is_empty()),
                        file: Some(file.to_string()).filter(|f| !f.is_empty()),
                        has_relic_slot: false,
                        spell_family: 0,
                    };
                    (id, row)
                })
                .collect(),
        )
    }

    /// `HasPetSpells`' second return: always a string, never nil, and the literal
    /// [`PET_NAME_TOKEN_FALLBACK`] for a class with no row (`0x4b44a6`).
    pub fn pet_name_token(&self, class: u32) -> &str {
        self.0
            .get(&class)
            .and_then(|c| c.pet_name_token.as_deref())
            .unwrap_or(PET_NAME_TOKEN_FALLBACK)
    }

    /// The class's `Name_Lang` name, `GetCharacterInfo`'s third return; `None` for a class with no
    /// row or no name, which that binding answers with the empty string (`0x47336e`).
    pub fn name(&self, class: u32) -> Option<&str> {
        self.0.get(&class)?.name.as_deref()
    }

    /// Every row's id and name, in no order, the empty string for a row with none, as the `/who`
    /// class tag searches them (`0x5aef0d`).
    pub fn names_by_id(&self) -> impl Iterator<Item = (u32, &str)> {
        self.0
            .iter()
            .map(|(id, c)| (*id, c.name.as_deref().unwrap_or_default()))
    }

    /// The class's file string (`"WARRIOR"`), the create screen's second value per class; `None`
    /// for a class with no row or an empty string.
    pub fn file(&self, class: u32) -> Option<&str> {
        self.0.get(&class)?.file.as_deref()
    }

    /// Whether the table has a row for `class`.
    pub fn has_row(&self, class: u32) -> bool {
        self.0.contains_key(&class)
    }

    /// The class's damage bonus stat, 0-based (Strength 0, Agility 1); `None` for a class with
    /// no row, where `GetDamageBonusStat` answers 0 (`0x48b58a`).
    pub fn damage_bonus_stat(&self, class: u32) -> Option<u32> {
        self.0.get(&class).map(|c| c.damage_bonus_stat)
    }

    /// Whether the class's INVSLOT 17 is a relic slot, `UnitHasRelicSlot` whole. A class with no
    /// row answers false, as the reference's bound check against `0xc0def8` falls to its nil leg.
    pub fn has_relic_slot(&self, class: u32) -> bool {
        self.0.get(&class).is_some_and(|c| c.has_relic_slot)
    }

    /// Whether a unit of `class` may hold the ranged sheathe state: a row whose relic flag is
    /// clear. The sheathe toggle (`0x5eb616`-`0x5eb640`) and setter (`0x611d12`-`0x611d44`) read
    /// the flag as [`Self::has_relic_slot`] does, but a class with no row fails their test too.
    pub fn has_ranged_sheath(&self, class: u32) -> bool {
        self.0.get(&class).is_some_and(|c| !c.has_relic_slot)
    }

    /// Sets `class`'s relic flag, for a table built by [`Self::from_rows`]; a class with no row
    /// is left without one.
    pub fn with_relic_slot(mut self, class: u32) -> Self {
        if let Some(c) = self.0.get_mut(&class) {
            c.has_relic_slot = true;
        }
        self
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
        // The token, the file string and the eight name columns are strings; the rest stay opaque dwords.
        let ty = if i == PET_NAME_TOKEN_FIELD
            || i == FILE_FIELD
            || (NAME_FIELD..NAME_FIELD + 8).contains(&i)
        {
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
    let locale = chain.locale();
    let mut by_id = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        by_id.insert(
            id,
            ChrClass {
                damage_bonus_stat: u32_at(r, DAMAGE_BONUS_STAT_FIELD).unwrap_or(0),
                pet_name_token: str_at(&rs, r, PET_NAME_TOKEN_FIELD),
                name: loc_str(&rs, r, NAME_FIELD, locale),
                file: str_at(&rs, r, FILE_FIELD),
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

    /// The sheathe class test (`0x5eb616`-`0x5eb640`, `0x611d12`-`0x611d44`): a relic class has
    /// no ranged sheathe state, and neither has a class with no row, where `UnitHasRelicSlot`
    /// answers nil.
    #[test]
    fn a_relic_class_and_a_class_without_a_row_have_no_ranged_sheath() {
        let t = ChrClasses::from_rows([(1, "Warrior", "WARRIOR"), (2, "Paladin", "PALADIN")])
            .with_relic_slot(2);
        assert!(t.has_ranged_sheath(1), "a Warrior draws a bow");
        assert!(
            !t.has_ranged_sheath(2),
            "a Paladin's libram is no ranged weapon"
        );
        assert!(!t.has_ranged_sheath(6), "no row");
        assert!(
            !t.has_relic_slot(6),
            "while UnitHasRelicSlot says no relic slot there"
        );
    }

    /// Field 14 is the class file string the create screen keys its icons by
    /// (`CLASS_ICON_TCOORDS`), one column past the name block's flags.
    #[test]
    fn the_file_string_is_field_14() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        for (class, file) in [
            (1, "WARRIOR"),
            (2, "PALADIN"),
            (3, "HUNTER"),
            (4, "ROGUE"),
            (5, "PRIEST"),
            (7, "SHAMAN"),
            (8, "MAGE"),
            (9, "WARLOCK"),
            (11, "DRUID"),
        ] {
            assert_eq!(t.file(class), Some(file), "class {class}");
        }
        assert_eq!(t.file(6), None);
        assert!(t.has_row(1) && !t.has_row(6));
    }

    /// Hunters and Rogues scale their damage off Agility, everyone else off Strength.
    #[test]
    fn hunters_and_rogues_take_their_damage_bonus_from_agility() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        for (class, who) in [(3, "Hunter"), (4, "Rogue")] {
            assert_eq!(t.damage_bonus_stat(class), Some(1), "{who}");
        }
        for (class, who) in [
            (1, "Warrior"),
            (2, "Paladin"),
            (5, "Priest"),
            (7, "Shaman"),
            (8, "Mage"),
            (9, "Warlock"),
            (11, "Druid"),
        ] {
            assert_eq!(t.damage_bonus_stat(class), Some(0), "{who}");
        }
        assert_eq!(t.damage_bonus_stat(6), None);
    }

    /// `GetCharacterInfo`'s class is the `Name_Lang` block at field 5, one column per locale; a
    /// class with no row has no name (6 and 10 in 1.12).
    #[test]
    fn class_names_are_the_name_lang_column() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        for (class, name) in [
            (1, "Warrior"),
            (2, "Paladin"),
            (3, "Hunter"),
            (4, "Rogue"),
            (5, "Priest"),
            (7, "Shaman"),
            (8, "Mage"),
            (9, "Warlock"),
            (11, "Druid"),
        ] {
            assert_eq!(t.name(class), Some(name), "class {class}");
        }
        assert_eq!((t.name(6), t.name(10), t.name(0)), (None, None, None));
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
