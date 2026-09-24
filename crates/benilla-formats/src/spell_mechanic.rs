//! SpellMechanic.dbc — the vocabulary that fills `SPELL_FAILED_PREVENTED_BY_MECHANIC`'s `%s`,
//! turning "Can't do that while %s" into "Can't do that while stunned".
//!
//! It is the crowd-control ladder's other half: the exemption scan reports the
//! blocking aura's mechanic as an **id**, and this names it. Without the table the arm's refusal
//! displayed its template with the specifier unfilled, which is the loose end 1941 recorded.
//!
//! The reference reads the same store (`0xc0d7c4`) from `0x6e2190`, the `0x8d` argument arm.
//!
//! Layout byte-checked on the raw 5875 file (a struct-unpack dump: **27 records × 10 fields,
//! record size 40**, string block 246) — the same shape as `SpellFocusObject.dbc`: `ID@0` · the
//! 8-locale `Name` block (enUS first ⇒ **Name = column 1**) · its flags word (9). Anchor rows:
//! 5 "fleeing" · 7 "rooted" · 12 "stunned" · 17 "polymorphed".

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const SPELL_MECHANIC: &str = "DBFilesClient\\SpellMechanic.dbc";
const SPELL_MECHANIC_FIELDS: usize = 10;
const COL_NAME_ENUS: usize = 1;

/// `SpellMechanic.Id → Name` — the "Can't do that while stunned" vocabulary.
///
/// The names are **lower-case and adjectival** in the shipped data ("stunned", "asleep",
/// "polymorphed"), which is what makes them read as the tail of that sentence rather than as a
/// title. Nothing here capitalises them; the reference does not either.
pub struct SpellMechanicCatalog {
    names: HashMap<u32, String>,
}

impl SpellMechanicCatalog {
    /// The display name for a mechanic id, or `None` for 0/unknown — which is exactly the case
    /// the refusal treats as "no mechanic named", falling back to the arm's own reason.
    pub fn name(&self, mechanic_id: u32) -> Option<&str> {
        self.names.get(&mechanic_id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("SpellMechanic");
    for i in 0..SPELL_MECHANIC_FIELDS {
        let ty = if i == COL_NAME_ENUS {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load SpellMechanic.dbc from the patch chain into a [`SpellMechanicCatalog`].
pub fn load_spell_mechanic_catalog(chain: &mut Chain) -> Result<SpellMechanicCatalog> {
    let bytes = chain
        .read_file(SPELL_MECHANIC)
        .with_context(|| format!("reading {SPELL_MECHANIC}"))?;
    let rs = parse(&bytes, schema(), "SpellMechanic.dbc")?;
    let mut names = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(name) = str_at(&rs, r, COL_NAME_ENUS) {
            names.insert(id, name);
        }
    }
    Ok(SpellMechanicCatalog { names })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows the crowd-control ladder actually names, on the real build-5875 file. A column
    /// slip lands on another locale or the flags word and fails loudly. Skips without client data.
    ///
    /// The four ids here are the ones decision 1941's arms can report, cross-checked against the
    /// `Spell.dbc` mechanic columns pinned in `spell_catalog`: Fear's `Mechanic` is 5, Frost Nova's
    /// `EffectMechanic[1]` is 7, Polymorph's is 17.
    #[test]
    fn real_spell_mechanic_names_the_crowd_control_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_spell_mechanic_catalog(&mut chain).expect("load SpellMechanic.dbc");

        assert_eq!(cat.name(5), Some("fleeing"));
        assert_eq!(cat.name(7), Some("rooted"));
        assert_eq!(cat.name(11), Some("ensnared"));
        assert_eq!(cat.name(12), Some("stunned"));
        assert_eq!(cat.name(17), Some("polymorphed"));
        assert_eq!(cat.name(0), None, "0 = no mechanic, and no line to fill");
        assert_eq!(cat.name(999), None);
        assert_eq!(cat.len(), 27, "the 5875 file's full row count");
    }
}
