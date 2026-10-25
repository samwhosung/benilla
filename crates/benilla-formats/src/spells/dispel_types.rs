//! `SpellDispelType.dbc`: the dispel class. A row is named only when column 10 (`+0x28`) is
//! nonzero (`0x52f906`, `0x519a82`): on 5875 that is ids 1-4, so Stealth and Invisibility carry
//! names that never print. Two strings name a class. Column 11 (`+0x2c`), English on every locale,
//! is the `dispelType` `UnitDebuff` returns (`0x519a89`), which FrameXML's `DebuffTypeColor` keys
//! the debuff border on in every locale's FrameXML. The `Name` block (columns 1-8) at the client's
//! locale slot is the word the aura tooltip prints (`0x52f880`): `Magie` on a German install.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{loc_str, parse, str_at, u32_at};

/// The named dispel classes by `Spell.dbc` `Dispel` id; a row the gate withholds is absent.
#[derive(Default)]
pub struct SpellDispelTypes {
    names: HashMap<u32, String>,
    labels: HashMap<u32, String>,
}

impl SpellDispelTypes {
    /// Fixture constructor for tests, each class's key its label too; the live path is
    /// [`load_spell_dispel_types`].
    pub fn from_names(names: HashMap<u32, String>) -> Self {
        Self {
            labels: names.clone(),
            names,
        }
    }

    /// The class's key, column 11: `Magic` on every locale, as `UnitDebuff` returns it; `None` for
    /// no dispel class (0) or one the gate withholds.
    pub fn name(&self, dispel: u32) -> Option<&str> {
        self.names.get(&dispel).map(String::as_str)
    }

    /// The class's word at the client's locale slot, which the aura tooltip prints; `None` as for
    /// [`Self::name`].
    pub fn label(&self, dispel: u32) -> Option<&str> {
        self.labels.get(&dispel).map(String::as_str)
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
    schema.add_field(SchemaField::new("InternalName", FieldType::String));
    let set = parse(&bytes, schema, "SpellDispelType.dbc")?;
    let locale = chain.locale();
    let (mut names, mut labels) = (HashMap::new(), HashMap::new());
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        // The gate first: a 0 here withholds the name however good the string is (Stealth, id 5).
        if u32_at(r, 10).unwrap_or(0) == 0 {
            continue;
        }
        if let Some(name) = str_at(&set, r, 11).filter(|n| !n.is_empty()) {
            names.insert(id, name);
        }
        if let Some(label) = loc_str(&set, r, 1, locale) {
            labels.insert(id, label);
        }
    }
    Ok(SpellDispelTypes { names, labels })
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

    /// The key is column 11 on every install, the label the locale slot's word: one and the same
    /// on an English install, `Magie` against `Magic` on a German one, where a localized key would
    /// miss `DebuffTypeColor` and break `TargetFrame.lua`'s debuff border.
    #[test]
    fn the_key_is_english_on_every_locale_and_the_label_is_the_locales() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let types = load_spell_dispel_types(&mut chain).expect("load SpellDispelType");
        for (id, key) in [(1, "Magic"), (2, "Curse"), (3, "Disease"), (4, "Poison")] {
            assert_eq!(types.name(id), Some(key));
            let label = types.label(id).expect("a gated row has a label");
            eprintln!("dispel {id}: key {key}, label {label}");
            if chain.locale().index() == 0 {
                assert_eq!(label, key, "enUS reads the key's own word");
            }
        }
        if chain.locale().name() == "deDE" {
            assert_eq!(types.label(1), Some("Magie"));
            assert_eq!(types.label(4), Some("Gift"));
        }
        assert_eq!(types.label(5), None, "the gate withholds the label too");
    }
}
