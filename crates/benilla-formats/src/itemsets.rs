//! `ItemSet.dbc`: the item sets behind the item tooltip's set block (builder `0x52b650`), laid out
//! as vmangos `ItemSetEntry` (`DBCStructure.h`): 17 item ids, then 8 spells and each one's
//! required equipped count.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const ITEM_SET: &str = "DBFilesClient\\ItemSet.dbc";

/// One set. `bonuses` is `(required equipped count, spell id)` in stored slot order; the tooltip
/// sorts it by count at print time, as the client's qsort `0x52e5c0` does.
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
    /// The set for an item template's `itemset` id.
    pub fn set(&self, id: u32) -> Option<&ItemSetInfo> {
        self.sets.get(&id)
    }

    pub fn len(&self) -> usize {
        self.sets.len()
    }

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
