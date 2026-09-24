//! `SpellMechanic.dbc`: the `%s` of `SPELL_FAILED_PREVENTED_BY_MECHANIC` ("Can't do that while
//! %s"), naming the blocking aura's mechanic. The reference reads this table (`0xc0d7c4`) in
//! `0x6e2190`, its `0x8d` argument arm.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const SPELL_MECHANIC: &str = "DBFilesClient\\SpellMechanic.dbc";
const SPELL_MECHANIC_FIELDS: usize = 10;
const COL_NAME_ENUS: usize = 1;

/// Mechanic id → name, lower-case in the data ("stunned") and left so, as the reference does.
pub struct SpellMechanicCatalog {
    names: HashMap<u32, String>,
}

impl SpellMechanicCatalog {
    /// The name for a mechanic id; `None` for 0, where the refusal falls back to its own reason.
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

    /// Fear's `Mechanic` is 5, Frost Nova's `EffectMechanic[1]` 7, Polymorph's 17.
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
