//! SpellFocusObject.dbc — the table naming the world object a spell must be cast NEAR (an Anvil, a
//! Forge, a Cooking Fire): the crafting book's "Requires: …" line resolves a recipe's
//! `Spell.dbc RequiresSpellFocus` id here. The proximity *check* is the server's
//! (`Spell::CheckCast`'s focus search) — the client only names the requirement.
//!
//! Layout byte-checked on the raw 5875 file this session (a struct-unpack dump: 138 records × 10
//! fields, record size 40): `ID@0` · the 8-locale `Name` block (enUS first ⇒ **Name = column 1**) ·
//! its flags word (9). Anchor rows: 1 "Anvil" · 2 "Loom" · 3 "Forge" · 4 "Cooking Fire".

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const SPELL_FOCUS_OBJECT: &str = "DBFilesClient\\SpellFocusObject.dbc";
const SPELL_FOCUS_FIELDS: usize = 10;
const COL_NAME_ENUS: usize = 1;

/// `SpellFocusObject.Id → Name` — the "Requires: Anvil" vocabulary.
pub struct SpellFocusCatalog {
    names: HashMap<u32, String>,
}

impl SpellFocusCatalog {
    /// The display name for a `RequiresSpellFocus` id, or `None` for 0/unknown (no requirement
    /// line at all).
    pub fn name(&self, focus_id: u32) -> Option<&str> {
        self.names.get(&focus_id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("SpellFocusObject");
    for i in 0..SPELL_FOCUS_FIELDS {
        let ty = if i == COL_NAME_ENUS {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load SpellFocusObject.dbc from the patch chain into a [`SpellFocusCatalog`].
pub fn load_spell_focus_catalog(chain: &mut Chain) -> Result<SpellFocusCatalog> {
    let bytes = chain
        .read_file(SPELL_FOCUS_OBJECT)
        .with_context(|| format!("reading {SPELL_FOCUS_OBJECT}"))?;
    let rs = parse(&bytes, schema(), "SpellFocusObject.dbc")?;
    let mut names = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(name) = str_at(&rs, r, COL_NAME_ENUS) {
            names.insert(id, name);
        }
    }
    Ok(SpellFocusCatalog { names })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The profession-focus rows on the real build-5875 file, byte-anchored (module doc's dump):
    /// a column slip lands on another locale/flags column and fails loudly. Skips without client
    /// data.
    #[test]
    fn real_spell_focus_names_the_profession_objects() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_spell_focus_catalog(&mut chain).expect("load SpellFocusObject.dbc");

        assert_eq!(cat.name(1), Some("Anvil"));
        assert_eq!(cat.name(3), Some("Forge"));
        assert_eq!(cat.name(4), Some("Cooking Fire"));
        assert_eq!(cat.name(0), None, "0 = no requirement");
        assert_eq!(cat.len(), 138, "the 5875 file's full row count");
    }
}
