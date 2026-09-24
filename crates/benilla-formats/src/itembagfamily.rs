//! ItemBagFamily.dbc — the container-family names, i.e. what a specialised bag *accepts*:
//! "Arrows", "Bullets", "Soul Shards", "Herbs", "Enchanting Supplies", "Engineering Supplies",
//! "Keys".
//!
//! One consumer, and it is the reason this table exists here at all: the inventory error line's
//! **reason 16** substitution (`ERR_WRONG_BAG_TYPE_SUBCLASS` = *"Only %s can be placed in
//! that."*). The helper is `0x5ede00(player, bagSlot)`: it resolves the named bag as an item,
//! maps it through `0x5da050`
//! into the `[0xc0dc38]` row table (bound `[0xc0dc3c]`), reads the localized name at
//! `[row + 4*[0xc0e080] + 4]` — i.e. field 1 at the active locale — and calls
//! `DisplayError(0x118, thatName)` itself. **Despite the errorId's `_SUBCLASS` name this is not
//! ItemSubClass.dbc**: the row is keyed by the bag's `BagFamily`, which is what makes the line
//! read "Only Arrows can be placed in that." for a quiver rather than naming the bag's own type.
//!
//! Record layout (8 rows in the shipped 5875 file, verified by loading it): `ID@0`,
//! `Name_Lang@1..8`, `NameFlags@9`. **Ids are not contiguous** — 0,1,2,3,6,7,8,9, with 4 and 5
//! absent — so this is an id-keyed lookup, never an index. Row 0's name is the literal string
//! `"NONE"`; see [`ItemBagFamilyCatalog::name`].

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
    /// What family `id` is *called* — the `%s` of "Only %s can be placed in that."
    ///
    /// **Family 0 yields `None`**, not the row's literal `"NONE"`. 0 means "an ordinary item /
    /// ordinary bag", so a resolved 0 would render "Only NONE can be placed in that." — the
    /// caller wants the generic line there. Unreachable on this wire in practice (the server
    /// only sends reason 16 for a *specialised* container mismatch), so this is a guard on a
    /// nonsense string rather than a modelled branch.
    pub fn name(&self, id: u32) -> Option<&str> {
        (id != 0)
            .then(|| self.names.get(&id).map(String::as_str))
            .flatten()
    }

    /// Row count, for the load log.
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

    /// The shipped 5875 table, read as data rather than assumed: the ids our
    /// `ItemInfo::bag_family` doc already names must each carry the family's *plural* spelling,
    /// which is what makes "Only Arrows can be placed in that." read correctly. Also pins the
    /// non-contiguity (4 and 5 absent) that makes this an id lookup, and family 0's guard.
    /// Skips without client data.
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
        // 4 and 5 genuinely do not ship — the gap is why this is keyed, not indexed.
        assert_eq!(cat.name(4), None);
        assert_eq!(cat.name(5), None);
        // Row 0 exists and is literally "NONE"; the accessor refuses it (see `name`).
        assert_eq!(cat.name(0), None, "family 0 must not render as \"NONE\"");
        assert_eq!(cat.len(), 8, "the whole shipped table");
    }
}
