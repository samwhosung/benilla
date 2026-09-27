//! `Cfg_Categories.dbc`, the realm list's category tabs. The reference builds its category set
//! once per process (`0x46e300`, from `CGlueMgr::Initialize`) out of every row whose column 1
//! equals the client Region ([`crate::client_region`]), in file order: column 0 is the id a
//! realm's wire category byte is matched against, and the row's localized name is the tab's
//! label (`GetRealmCategories`, `0x46f238`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const CFG_CATEGORIES: &str = "DBFilesClient\\Cfg_Categories.dbc";

/// One realm category of the client's Region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmCategory {
    /// Column 0, compared for equality with a realm's category byte (`0x46e60b`).
    pub id: u32,
    /// The enUS `Name_Lang` slot, the column the reference reads on an enUS or enGB client.
    pub name: String,
}

/// `ID`, `Region`, eight locale names and their flags: the 11 columns × 0x2c bytes the loader
/// (`0x5411b0`) gates on.
fn cfg_categories_schema() -> Schema {
    let mut s = Schema::new("Cfg_Categories");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Region", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s
}

/// The categories of `region`, in file order (`0x46e3b1`: a row joins when its column 1 equals
/// the Region). A Region no row names yields none.
pub fn load_realm_categories(chain: &mut Chain, region: u32) -> Result<Vec<RealmCategory>> {
    let bytes = chain
        .read_file(CFG_CATEGORIES)
        .with_context(|| format!("reading {CFG_CATEGORIES}"))?;
    let rs = parse(&bytes, cfg_categories_schema(), "Cfg_Categories")?;
    Ok(rs
        .records()
        .iter()
        .filter(|r| u32_at(r, 1) == Some(region))
        .filter_map(|r| {
            Some(RealmCategory {
                id: u32_at(r, 0)?,
                // An empty name is pushed as the empty string; only a missing row reads "UNKNOWN".
                name: str_at(&rs, r, 2).unwrap_or_default(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(region: u32) -> Option<Vec<(u32, String)>> {
        let data = crate::wow_data_or_skip!(None);
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cats = load_realm_categories(&mut chain, region).expect("Cfg_Categories.dbc");
        Some(cats.into_iter().map(|c| (c.id, c.name)).collect())
    }

    /// Region 1 is the US client's: "United States" and "Oceanic", ids 1 and 5, in file order.
    #[test]
    fn the_us_region_is_united_states_then_oceanic() {
        let Some(us) = rows(1) else { return };
        assert_eq!(
            us,
            [(1, "United States".to_string()), (5, "Oceanic".to_string())]
        );
    }

    /// Region 3 is Europe's four language tabs; the ids are not contiguous, so a category is
    /// matched by id, never by position.
    #[test]
    fn the_european_region_is_four_tabs_with_a_gap_in_the_ids() {
        let Some(eu) = rows(3) else { return };
        let ids: Vec<u32> = eu.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [1, 2, 3, 5]);
        assert_eq!(eu[0].1, "English");
        assert_eq!(eu[3].1, "Spanish");
    }

    /// No row carries Region 0, the value a client without a `Wow.ini` Region reads.
    #[test]
    fn region_zero_has_no_categories() {
        let Some(none) = rows(0) else { return };
        assert!(none.is_empty(), "{none:?}");
    }
}
