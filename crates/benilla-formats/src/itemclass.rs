//! `ItemClass.dbc`: an item class's name, `GetItemInfo`'s fifth return (`itemType`). The binding
//! (`0x48e070`) bounds the class by the row count (`0xc0dc28`), indexes the rows (`0xc0dc24`) and
//! pushes the name unfiltered, `(OBSOLETE)` suffixes included, or "" when there is no row
//! (`0x48e236`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const ITEM_CLASS: &str = "DBFilesClient\\ItemClass.dbc";

/// ItemClass.dbc keyed by the `class` an item template carries.
pub struct ItemClassCatalog {
    names: HashMap<u32, String>,
}

impl ItemClassCatalog {
    /// The class's name; `None` where the reference pushes the empty string.
    pub fn name(&self, id: u32) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

fn item_class_schema() -> Schema {
    let mut s = Schema::new("ItemClass");
    s.add_field(SchemaField::new("ClassID", FieldType::UInt32));
    s.add_field(SchemaField::new("SubClassMapID", FieldType::UInt32));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s
}

/// Load ItemClass.dbc from the patch chain.
pub fn load_item_classes(chain: &mut Chain) -> Result<ItemClassCatalog> {
    let bytes = chain
        .read_file(ITEM_CLASS)
        .with_context(|| format!("reading {ITEM_CLASS}"))?;
    let rs = parse(&bytes, item_class_schema(), "ItemClass")?;
    let mut names = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(name)) = (u32_at(r, 0), str_at(&rs, r, 3).filter(|n| !n.is_empty()))
        else {
            continue;
        };
        names.insert(id, name);
    }
    Ok(ItemClassCatalog { names })
}

#[cfg(test)]
mod tests {
    use super::load_item_classes;

    /// Bagnon (`Bagnon_Core/localization.lua:94-95`) expects "Container" and "Quiver" here.
    #[test]
    fn the_shipped_classes_are_named_as_getiteminfo_returns_them() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_classes(&mut chain).expect("ItemClass.dbc");

        assert_eq!(cat.name(0), Some("Consumable"));
        assert_eq!(cat.name(1), Some("Container"), "Bagnon's own expectation");
        assert_eq!(cat.name(2), Some("Weapon"));
        assert_eq!(cat.name(4), Some("Armor"));
        assert_eq!(cat.name(7), Some("Trade Goods"));
        assert_eq!(cat.name(11), Some("Quiver"), "Bagnon's own expectation");
        assert_eq!(cat.name(15), Some("Miscellaneous"));
        // The reference pushes the row string unfiltered.
        assert_eq!(cat.name(3), Some("Jewelry(OBSOLETE)"));
        assert_eq!(cat.name(10), Some("Money(OBSOLETE)"));
        assert_eq!(cat.name(16), None);
        assert_eq!(cat.len(), 16, "the whole shipped table");
    }
}
