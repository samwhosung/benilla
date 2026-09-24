//! `ItemBagFamily.dbc`: what a specialised bag accepts ("Arrows", "Soul Shards", "Keys", …), the
//! `%s` of inventory error reason 16, `ERR_WRONG_BAG_TYPE_SUBCLASS`. Despite that name the
//! reference (`0x5ede00`) keys the row by the bag's `BagFamily`, not its subclass, through
//! `0x5da050` into `[0xc0dc38]`. Ids skip 4 and 5, so this is keyed, never indexed.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const ITEM_BAG_FAMILY: &str = "DBFilesClient\\ItemBagFamily.dbc";

/// ItemBagFamily.dbc keyed by the `BagFamily` id an item template carries.
pub struct ItemBagFamilyCatalog {
    names: HashMap<u32, String>,
}

impl ItemBagFamilyCatalog {
    /// The family's name; `None` for family 0, an ordinary bag, whose row reads "NONE". The
    /// server sends reason 16 only for a specialised bag.
    pub fn name(&self, id: u32) -> Option<&str> {
        (id != 0)
            .then(|| self.names.get(&id).map(String::as_str))
            .flatten()
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

fn item_bag_family_schema() -> Schema {
    let mut s = Schema::new("ItemBagFamily");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s
}

/// Load ItemBagFamily.dbc from the patch chain.
pub fn load_item_bag_families(chain: &mut Chain) -> Result<ItemBagFamilyCatalog> {
    let bytes = chain
        .read_file(ITEM_BAG_FAMILY)
        .with_context(|| format!("reading {ITEM_BAG_FAMILY}"))?;
    let rs = parse(&bytes, item_bag_family_schema(), "ItemBagFamily")?;
    let mut names = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(name)) = (u32_at(r, 0), str_at(&rs, r, 1).filter(|n| !n.is_empty()))
        else {
            continue;
        };
        names.insert(id, name);
    }
    Ok(ItemBagFamilyCatalog { names })
}

#[cfg(test)]
mod tests {
    use super::load_item_bag_families;

    #[test]
    fn the_shipped_families_are_named_as_the_error_line_needs() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_bag_families(&mut chain).expect("ItemBagFamily.dbc");

        assert_eq!(cat.name(1), Some("Arrows"), "quiver");
        assert_eq!(cat.name(2), Some("Bullets"), "ammo pouch");
        assert_eq!(cat.name(3), Some("Soul Shards"), "soul bag");
        assert_eq!(cat.name(6), Some("Herbs"));
        assert_eq!(cat.name(7), Some("Enchanting Supplies"));
        assert_eq!(cat.name(8), Some("Engineering Supplies"));
        assert_eq!(cat.name(9), Some("Keys"), "the keyring family");
        // 4 and 5 do not ship.
        assert_eq!(cat.name(4), None);
        assert_eq!(cat.name(5), None);
        assert_eq!(cat.name(0), None, "family 0 must not render as \"NONE\"");
        assert_eq!(cat.len(), 8, "the whole shipped table");
    }
}
