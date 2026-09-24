//! `ItemRandomProperties.dbc`, the random-suffix table: the "of the Monkey" roll an item carries
//! and the enchants it grants. The name formatter `0x5d8b00` joins name and suffix through
//! `ITEM_SUFFIX_TEMPLATE` (`"%s %s"`), taking its plain-name exit (`0x5d8ba5`) for an id that is
//! `0`, negative, past the table or suffix-less and its suffix exit (`0x5d8b84`) otherwise. The
//! tooltip (`0x52b7bf`-`0x52b7fb`) copies the row's five enchant ids (`row+0x8..+0x18`) into its
//! enchant slots 2..6 for `0x52c991` to print, so a linked item shows its stat lines, not
//! `<Random enchantment>`. An item object carries its own enchants (`ITEM_FIELD_ENCHANTMENT`
//! 2..6) and reads the table only for the name, off `ITEM_FIELD_RANDOM_PROPERTIES_ID`. The
//! localized suffix is columns 7..14, read at `0x1c + 4*locale`.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

const ITEM_RANDOM_PROPERTIES: &str = "DBFilesClient\\ItemRandomProperties.dbc";

/// The enchant slots a row grants, the item's enchant slots 2..6 (0 permanent, 1 temporary).
pub const RANDOM_PROPERTY_SLOTS: usize = 5;

/// The item-enchant slot the first random-property enchant lands in: `0x52b7e0`-`0x52b7fb`
/// copies the row's five dwords to the tooltip session's `+0x3d8..+0x3e8`, slots 2..6.
pub const RANDOM_PROPERTY_FIRST_SLOT: u8 = 2;

/// One `ItemRandomProperties` row: the suffix the name takes, and the enchants the roll grants.
#[derive(Clone, Debug, Default)]
pub struct RandomProperty {
    /// The enUS suffix (`"of the Bear"`), never empty: a suffix-less row is the formatter's
    /// plain-name exit and is not loaded.
    pub suffix: String,
    /// The five `SpellItemEnchantment` ids for enchant slots 2..6, `0` where the row grants none.
    pub enchants: [u32; RANDOM_PROPERTY_SLOTS],
}

/// `ItemRandomProperties.dbc` keyed by id, for the name suffix and the tooltip's enchants.
pub struct RandomPropertyCatalog {
    rows: HashMap<u32, RandomProperty>,
}

impl RandomPropertyCatalog {
    /// The row for a random-property id, taken signed because `0x5d8b00` gates it so: `0`,
    /// negative and unknown ids name none.
    pub fn get(&self, id: i32) -> Option<&RandomProperty> {
        (id > 0).then(|| self.rows.get(&(id as u32)))?
    }

    /// The display name as `0x5d8b00` builds it: `ITEM_SUFFIX_TEMPLATE` (`"%s %s"`) for a
    /// resolved suffix, else the plain name.
    pub fn suffixed_name(&self, name: &str, id: i32) -> String {
        match self.get(id) {
            Some(row) => format!("{name} {}", row.suffix),
            None => name.to_string(),
        }
    }

    /// Every row as `(id, row)`, for the whole-table push to the engine.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &RandomProperty)> + '_ {
        self.rows.iter().map(|(&id, row)| (id, row))
    }

    /// A catalog from explicit rows, for tests and fixtures.
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

/// Load `ItemRandomProperties.dbc` off the patch chain, dropping a row with no enUS suffix, the
/// formatter's plain-name exit.
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
        // Field 7, `Suffix0`: the enUS string at `0x1c`, the one the name formatter reads.
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
        assert_eq!(
            cat.len(),
            2012,
            "every shipped row carries a locale-0 suffix"
        );
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
