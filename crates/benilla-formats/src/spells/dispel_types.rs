//! `SpellDispelType.dbc`: the dispel class name the aura tooltip (`0x52f880`) shows and
//! `UnitAura`/`UnitDebuff` return as `debuffType`, which FrameXML's `DebuffTypeColor` keys the
//! debuff border on. A row is named only when column 10 (`+0x28`) is nonzero (`0x52f906`): on 5875
//! that is ids 1-4, so Stealth and Invisibility carry names that never print. Column 11 repeats
//! the name on those four rows; the tooltip does not read it.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

/// The named dispel classes by `Spell.dbc` `Dispel` id; a row the gate withholds is absent.
#[derive(Default)]
pub struct SpellDispelTypes {
    names: HashMap<u32, String>,
}

impl SpellDispelTypes {
    /// The class name; `None` for no dispel class (0) or one the gate withholds.
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

/// Load `SpellDispelType.dbc` off the patch chain, keeping only the rows the gate names.
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

    #[test]
    fn real_dispel_types_name_only_what_the_gate_allows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let types = load_spell_dispel_types(&mut chain).expect("load SpellDispelType");
        assert_eq!(types.name(1), Some("Magic"));
        assert_eq!(types.name(2), Some("Curse"));
        assert_eq!(types.name(3), Some("Disease"));
        assert_eq!(types.name(4), Some("Poison"));
        // Named in the file, withheld by the gate.
        assert_eq!(types.name(5), None, "Stealth");
        assert_eq!(types.name(6), None, "Invisibility");
        assert_eq!(types.name(9), None, "Frenzy");
        assert_eq!(types.name(0), None, "no dispel class");
        assert_eq!(types.len(), 4, "exactly the four the gate allows");
    }
}
