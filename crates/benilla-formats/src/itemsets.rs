//! ItemSet.dbc — the item-set catalog behind the tooltip's SET block (the item tooltip builder
//! `0x52b650`: set name "(owned/total)" gold, per-member lines
//! pale-cream/gray, threshold bonuses green/gray via the `$`-token engine).
//!
//! Record layout per vmangos `ItemSetEntry` (`DBCStructure.h`, the 1.12 branch): id@0, the 8+1
//! localized name block @1..9, itemId[17]@10..26 (0-padded), setSpellID[8]@27..34,
//! setThreshold[8]@35..42 (each spell's required equipped count), requiredSkill@43,
//! requiredSkillRank@44 — 45 fields.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const ITEM_SET: &str = "DBFilesClient\\ItemSet.dbc";

/// One set: display name, the member item ids (nonzero of the 17 slots), the threshold bonuses
/// (`(required equipped count, spell id)`, nonzero spells only, in the DBC's stored slot order
/// — the tooltip sorts threshold-ascending at print time, like the client's qsort `0x52e5c0`),
/// and the set-level skill requirement.
#[derive(Debug, Clone)]
pub struct ItemSetInfo {
    pub name: String,
    pub items: Vec<u32>,
    pub bonuses: Vec<(u32, u32)>,
    pub required_skill: u32,
    pub required_skill_rank: u32,
}

/// ItemSet.dbc loaded into an id → row map.
pub struct ItemSetCatalog {
    sets: HashMap<u32, ItemSetInfo>,
}

impl ItemSetCatalog {
    /// The set row for an item template's `itemset` id, or `None` for an id the DBC lacks.
    pub fn set(&self, id: u32) -> Option<&ItemSetInfo> {
        self.sets.get(&id)
    }

    /// Number of rows (for logging/diagnostics).
    pub fn len(&self) -> usize {
        self.sets.len()
    }

    /// Whether no rows loaded.
    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }
}

fn item_set_schema() -> Schema {
    let mut s = Schema::new("ItemSet");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    for i in 0..17 {
        s.add_field(SchemaField::new(format!("Item{i}"), FieldType::UInt32));
    }
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Spell{i}"), FieldType::UInt32));
    }
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Threshold{i}"), FieldType::UInt32));
    }
    s.add_field(SchemaField::new("RequiredSkill", FieldType::UInt32));
    s.add_field(SchemaField::new("RequiredSkillRank", FieldType::UInt32));
    s
}

/// Load ItemSet.dbc from the patch chain into an [`ItemSetCatalog`].
pub fn load_item_sets(chain: &mut Chain) -> Result<ItemSetCatalog> {
    let bytes = chain
        .read_file(ITEM_SET)
        .with_context(|| format!("reading {ITEM_SET}"))?;
    let rs = parse(&bytes, item_set_schema(), "ItemSet")?;
    let mut sets = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(name)) = (u32_at(r, 0), str_at(&rs, r, 1)) else {
            continue;
        };
        let at = |i| u32_at(r, i).unwrap_or(0);
        sets.insert(
            id,
            ItemSetInfo {
                name,
                items: (10..27).map(at).filter(|&i| i != 0).collect(),
                bonuses: (0..8)
                    .filter_map(|i| {
                        let spell = at(27 + i);
                        (spell != 0).then(|| (at(35 + i), spell))
                    })
                    .collect(),
                required_skill: at(43),
                required_skill_rank: at(44),
            },
        );
    }
    Ok(ItemSetCatalog { sets })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Data-gated on the real 5875 DBC (172 rows): Vestments of the Devout (182, the priest
    /// dungeon set) carries 8 members + its bonus ladder; Defias Leather (161) carries 5.
    /// Bonuses load in the DBC's stored slot order (The Gladiator stores 3,2,5,4 — the tooltip
    /// sorts at print time). Every skill-gated set carries a nonzero rank (the builder's
    /// rank-0 format leg is data-empty). Skips without client data.
    #[test]
    fn item_sets_load_from_the_chain() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_sets(&mut chain).expect("ItemSet.dbc loads");
        assert!(!cat.is_empty());
        let devout = cat.set(182).expect("Vestments of the Devout row");
        assert_eq!(devout.name, "Vestments of the Devout");
        assert_eq!(devout.items.len(), 8);
        assert!(!devout.bonuses.is_empty());
        assert!(devout.bonuses.iter().all(|&(n, s)| n >= 2 && s != 0));
        assert_eq!(cat.set(161).expect("Defias Leather row").items.len(), 5);
        assert!(
            cat.sets
                .values()
                .all(|s| s.required_skill == 0 || s.required_skill_rank > 0),
            "a skill-gated set with rank 0 would need the builder's rank-less format leg"
        );
    }
}
