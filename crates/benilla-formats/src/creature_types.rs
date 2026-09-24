//! `CreatureType.dbc`: each creature type's flags word (column 10). The TAB-target scorer
//! (`0x494200`) looks a unit's type up in its cached table (`[0xc0de2c]`) and rejects it when
//! `row[+0x28] & 1`. A unit's type comes from `SMSG_CREATURE_QUERY_RESPONSE`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const CREATURE_TYPE: &str = "DBFilesClient\\CreatureType.dbc";

/// Creature type id → the row's flags word.
#[derive(Debug, Default, Clone)]
pub struct CreatureTypeFlags(HashMap<u32, u32>);

impl CreatureTypeFlags {
    /// Whether TAB targeting skips this type (`flags & 1`): in 1.12 data only Critter (8), not
    /// Totem (11). An unknown type is targetable, as the client skips an out-of-range index.
    pub fn no_tab_target(&self, creature_type: u32) -> bool {
        self.0.get(&creature_type).is_some_and(|f| f & 1 != 0)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn creature_type_schema() -> Schema {
    let mut s = Schema::new("CreatureType");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    s
}

/// Load CreatureType.dbc from the patch chain.
pub fn load_creature_type_flags(chain: &mut Chain) -> Result<CreatureTypeFlags> {
    let bytes = chain
        .read_file(CREATURE_TYPE)
        .with_context(|| format!("reading {CREATURE_TYPE}"))?;
    let rs = parse(&bytes, creature_type_schema(), "CreatureType")?;
    let mut flags = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let Some(id) = u32_at(r, 0) {
            flags.insert(id, u32_at(r, 10).unwrap_or(0));
        }
    }
    Ok(CreatureTypeFlags(flags))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_flags_mark_only_critter() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let flags = load_creature_type_flags(&mut chain).expect("load CreatureType.dbc");
        assert_eq!(flags.len(), 11, "1.12 ships 11 creature types");
        assert!(flags.no_tab_target(8), "Critter (8) must be un-TAB-able");
        for id in [1, 7, 11] {
            assert!(!flags.no_tab_target(id), "type {id} must be targetable");
        }
        assert!(!flags.no_tab_target(999), "unknown type is targetable");
    }
}
