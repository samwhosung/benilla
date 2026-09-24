//! `SpellDispelType.dbc` — the name of a spell's dispel class, and the flag that decides whether
//! the class is *named at all*.
//!
//! Two consumers read exactly this table: the **aura tooltip's right column** ("Magic" on Ice
//! Armor — the aura builder `0x52f880`), and the
//! `debuffType` return of `UnitAura`/`UnitDebuff`, which FrameXML's `DebuffTypeColor` keys the
//! debuff border tint on. Both take the name from this row; neither hard-codes it.
//!
//! **The gate is `[+0x28]`, not the id.** A row is named only when its `+0x28` field is nonzero —
//! read at `0x52f906`, and confirmed here on the shipped file: the 11 records are `{0 "",
//! 1 Magic, 2 Curse, 3 Disease, 4 Poison, 5 Stealth, 6 Invisibility, 7 All(M+C+D+P), 8 "Special -
//! npc only", 9 Frenzy, 10 ZG Trinkets}` and `[+0x28]` reads **1 for ids 1–4 and 0 for every
//! other row**. So Stealth and Invisibility have names in the file yet print nothing — the flag,
//! not the string, is what withholds them.
//!
//! Record layout (11 records × 12 fields × 4 B = 0x30 stride, read off the extracted file):
//! `ID@0`, `Name_Lang@1..8` (enUS at 1; every other locale is empty on 5875), `NameFlags@9`
//! (`0x007F007E`, `0x003F007E` on the last two rows), the gate at `@10` (= `+0x28`), and `@11` —
//! a second string column holding the *same* string as the name, populated on exactly the same
//! four rows. Whatever `@11` is for, it is not what the tooltip reads; `@10` is.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

/// The named dispel classes, by `Spell.dbc` `Dispel` id — a row whose `[+0x28]` gate is 0 is
/// **absent**, so a lookup miss is exactly "this class is not named" (see the module doc).
#[derive(Default)]
pub struct SpellDispelTypes {
    names: HashMap<u32, String>,
}

impl SpellDispelTypes {
    /// The dispel class's name ("Magic"/"Curse"/"Disease"/"Poison" on 5875), or `None` when the
    /// spell has no dispel class (`dispel == 0`) or its class is one the gate withholds.
    pub fn name(&self, dispel: u32) -> Option<&str> {
        self.names.get(&dispel).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// Load `SpellDispelType.dbc` off the patch chain, keeping only the rows the `[+0x28]` gate names.
pub fn load_spell_dispel_types(chain: &mut Chain) -> Result<SpellDispelTypes> {
    let bytes = chain
        .read_file("DBFilesClient\\SpellDispelType.dbc")
        .context("reading SpellDispelType.dbc")?;
    let mut schema = Schema::new("SpellDispelType");
    schema.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..8 {
        schema.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    schema.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    schema.add_field(SchemaField::new("Named", FieldType::UInt32));
    schema.add_field(SchemaField::new("Unknown11", FieldType::String));
    let set = parse(&bytes, schema, "SpellDispelType.dbc")?;
    let mut names = HashMap::new();
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        // The gate first: a 0 here withholds the name however good the string is (Stealth, id 5).
        if u32_at(r, 10).unwrap_or(0) == 0 {
            continue;
        }
        if let Some(name) = str_at(&set, r, 1).filter(|n| !n.is_empty()) {
            names.insert(id, name);
        }
    }
    Ok(SpellDispelTypes { names })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole shipped table, and the gate that is the point of it: ids 1–4 name themselves,
    /// and Stealth/Invisibility/Frenzy — which DO carry strings in the file — do not. Skips
    /// without client data.
    #[test]
    fn real_dispel_types_name_only_what_the_gate_allows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let types = load_spell_dispel_types(&mut chain).expect("load SpellDispelType");
        assert_eq!(types.name(1), Some("Magic"));
        assert_eq!(types.name(2), Some("Curse"));
        assert_eq!(types.name(3), Some("Disease"));
        assert_eq!(types.name(4), Some("Poison"));
        // Named in the file, withheld by the gate — the whole reason we read `[+0x28]`.
        assert_eq!(types.name(5), None, "Stealth");
        assert_eq!(types.name(6), None, "Invisibility");
        assert_eq!(types.name(9), None, "Frenzy");
        assert_eq!(types.name(0), None, "no dispel class");
        assert_eq!(types.len(), 4, "exactly the four the gate allows");
    }
}
