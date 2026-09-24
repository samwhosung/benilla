//! `ItemRandomProperties.dbc` — the **random-suffix** table: the "… of the Monkey" roll an item
//! carries, and the enchants that roll actually grants.
//!
//! The client reads this table from two places, and both are one row lookup:
//!
//! - **the NAME.** `0x5d8b00(dst, size, itemEntry, randomPropertyId)` — the one item-name
//!   formatter — joins the item-cache name with this row's locale-indexed suffix string through
//!   `ITEM_SUFFIX_TEMPLATE` (`"%s %s"`), so a rolled green reads "Chipped Claw of the Bear". Its
//!   two exits ARE the gate: a `randomPropertyId` that is `0`, negative, past the store's max id,
//!   or resolves to a null/empty suffix takes the plain-name exit (`0x5d8ba5`); anything else
//!   takes the suffix exit (`0x5d8b84`).
//! - **the ENCHANTS.** The item tooltip's own suffix mechanism `0x52b7bf–0x52b7fb` resolves the
//!   same row from the tooltip's `+0x424` randomPropertyId and copies **five dwords from
//!   `row+0x8..+0x18`** into the tooltip's session enchant slots 2..6 — which the enchant family
//!   (`0x52c991`) then prints, in white, exactly as if the item object had carried them. That is
//!   why a looted or linked
//!   random-property item shows its real stat lines and *not* the `<Random enchantment>`
//!   placeholder: the placeholder is the no-roll-known arm, and a known roll fills the slots.
//!
//! An item OBJECT never needs this table for its enchants — the server writes the rolled ids into
//! its own `ITEM_FIELD_ENCHANTMENT` slots 2..6 — but it still needs it for the NAME, off
//! `ITEM_FIELD_RANDOM_PROPERTIES_ID`.
//!
//! ## Layout — VERIFIED against build 5875 (the shipped table dumped whole)
//!
//! | table | records | fields | record size |
//! |---|---|---|---|
//! | `ItemRandomProperties` | 2012 | 16 | 64 |
//!
//! Columns: `0` id · `1` the internal (unlocalized) name · `2`-`6` the **five
//! `SpellItemEnchantment` ids** (byte offsets `0x8..0x18` — the exact five dwords the tooltip
//! copies) · `7`-`14` the localized suffix `Name_Lang[8]` (byte offset `0x1c`, and `0x1c + 4*locale`
//! is the string the name formatter reads) · `15` the name mask.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

const ITEM_RANDOM_PROPERTIES: &str = "DBFilesClient\\ItemRandomProperties.dbc";

/// The five enchant slots a random-property row grants — and, one-for-one, the item's enchant
/// slots **2..6** (slot 0 is the permanent enchant, slot 1 the temporary one; the suffix never
/// touches either).
pub const RANDOM_PROPERTY_SLOTS: usize = 5;

/// The item-enchant slot the first random-property enchant lands in (`ITEM_FIELD_ENCHANTMENT`
/// slot 2 — the session block's `+0x3d8` is slot 2, and `0x52b7e0–0x52b7fb` copies the row's five
/// dwords into `+0x3d8..+0x3e8`).
pub const RANDOM_PROPERTY_FIRST_SLOT: u8 = 2;

/// One `ItemRandomProperties` row: the suffix the name takes, and the enchants the roll grants.
#[derive(Clone, Debug, Default)]
pub struct RandomProperty {
    /// The locale-0 (enUS) suffix — `"of the Bear"`. Never empty: a row whose suffix string is
    /// empty is the name formatter's plain-name exit, so it never reaches this map.
    pub suffix: String,
    /// The row's five `SpellItemEnchantment` ids, in slot order (item enchant slots 2..6). `0`
    /// where the row grants nothing — most rows carry one or two.
    pub enchants: [u32; RANDOM_PROPERTY_SLOTS],
}

/// `ItemRandomProperties.dbc`, keyed by id — the one load both consumers read (the name suffix and
/// the tooltip's slots 2..6). One table, one loader: two would be how a schema drifts.
pub struct RandomPropertyCatalog {
    rows: HashMap<u32, RandomProperty>,
}

impl RandomPropertyCatalog {
    /// The row for a random-property id, or `None` when the id names none.
    ///
    /// Takes the id **signed**, because that is how the client gates it: `0x5d8b00` rejects `0`,
    /// negative, and `> maxId` before it ever reads the row. A row that resolves but carries an
    /// empty suffix is equally nothing (it never entered the map).
    pub fn get(&self, id: i32) -> Option<&RandomProperty> {
        (id > 0).then(|| self.rows.get(&(id as u32)))?
    }

    /// The suffixed display name — the client's `0x5d8b00`, whose whole law is its two exits:
    /// `ITEM_SUFFIX_TEMPLATE` (`"%s %s"`) joined when the id resolves to a real suffix, the plain
    /// name otherwise.
    pub fn suffixed_name(&self, name: &str, id: i32) -> String {
        match self.get(id) {
            Some(row) => format!("{name} {}", row.suffix),
            None => name.to_string(),
        }
    }

    /// Every row, `(id, row)` — the whole-table resolve the app pushes to the engine reads it.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &RandomProperty)> + '_ {
        self.rows.iter().map(|(&id, row)| (id, row))
    }

    /// Build from explicit rows — tests and synthetic fixtures.
    pub fn from_rows(rows: HashMap<u32, RandomProperty>) -> Self {
        RandomPropertyCatalog { rows }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(crate) fn item_random_properties_schema() -> Schema {
    let mut s = Schema::new("ItemRandomProperties");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Name", FieldType::String));
    for i in 0..RANDOM_PROPERTY_SLOTS {
        s.add_field(SchemaField::new(
            format!("Enchantment{i}"),
            FieldType::UInt32,
        ));
    }
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Suffix{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("SuffixFlags", FieldType::UInt32));
    s
}

/// Load `ItemRandomProperties.dbc` off the patch chain.
///
/// A row with no locale-0 suffix is dropped at load — it is the name formatter's plain-name exit
/// and grants a suffix to nothing, so keeping it would only invite a caller to print an empty
/// `"Name "`. (The shipped table has none, and the drop is the client's own `cmp`, not a guess.)
pub fn load_random_property_catalog(chain: &mut Chain) -> Result<RandomPropertyCatalog> {
    let bytes = chain
        .read_file(ITEM_RANDOM_PROPERTIES)
        .with_context(|| format!("reading {ITEM_RANDOM_PROPERTIES}"))?;
    let rs = parse(
        &bytes,
        item_random_properties_schema(),
        "ItemRandomProperties",
    )?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        // Field 7 = `Suffix0`, the enUS slot of the localized block — byte offset 0x1c, which is
        // `0x1c + 4*locale` at locale 0, the string the name formatter reads.
        let Some(suffix) = str_at(&rs, r, 7) else {
            continue;
        };
        let mut enchants = [0u32; RANDOM_PROPERTY_SLOTS];
        for (slot, e) in enchants.iter_mut().enumerate() {
            *e = u32_at(r, 2 + slot).unwrap_or(0);
        }
        rows.insert(id, RandomProperty { suffix, enchants });
    }
    Ok(RandomPropertyCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_table_loads_with_its_suffixes_and_enchants() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_random_property_catalog(&mut chain).expect("load");
        // 2012 rows in the shipped 1.12.1 table (dumped whole: 16 fields, 64-byte records).
        assert_eq!(
            cat.len(),
            2012,
            "every shipped row carries a locale-0 suffix"
        );
        // Row 5 — the table's first: "of Intellect", one enchant (79).
        let row = cat.get(5).expect("row 5");
        assert_eq!(row.suffix, "of Intellect");
        assert_eq!(row.enchants, [79, 0, 0, 0, 0]);
        // The name join is the client's `ITEM_SUFFIX_TEMPLATE` "%s %s".
        assert_eq!(
            cat.suffixed_name("Chipped Claw", 5),
            "Chipped Claw of Intellect"
        );
        // The formatter's plain-name exit: 0, negative, and an id past the table.
        for id in [0, -1, 999_999] {
            assert_eq!(cat.suffixed_name("Chipped Claw", id), "Chipped Claw");
        }
    }
}
