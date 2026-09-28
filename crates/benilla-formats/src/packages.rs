//! `Package.dbc`: the send tab's gift packages, 12 columns of `0x30` bytes (the loader's checks,
//! `0x54b4ae` and `0x54b4e6`). The reference keeps the rows in file order (`[0xc0da28]`, count
//! `[0xc0da2c]`) and indexes them by position: `GetPackageInfo` reads the icon at `+4`, the cost at
//! `+8` and the localized name at `+0xc`, `SelectPackage` stores the id at `+0`. 1.12 ships one
//! row, "Test Package".

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const PACKAGE: &str = "DBFilesClient\\Package.dbc";

/// The column count the loader requires.
const PACKAGE_FIELDS: usize = 12;

/// One `Package.dbc` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageRow {
    pub id: u32,
    /// The icon's basename, which the reference prefixes with `Interface\Icons\`.
    pub icon: String,
    /// Copper, read as a signed dword (`0x4ae50d` `fild`).
    pub cost: i32,
    /// The enUS name.
    pub name: String,
}

/// Load `Package.dbc`'s rows, in file order.
pub fn load_packages(chain: &mut Chain) -> Result<Vec<PackageRow>> {
    let bytes = chain.read_file(PACKAGE).context("reading Package.dbc")?;
    let mut schema = Schema::new("Package");
    for i in 0..PACKAGE_FIELDS {
        let ty = if i == 1 || i == 3 {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        schema.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    let set = parse(&bytes, schema, "Package.dbc")?;
    Ok(set
        .records()
        .iter()
        .filter_map(|r| {
            Some(PackageRow {
                id: u32_at(r, 0)?,
                icon: str_at(&set, r, 1).unwrap_or_default(),
                cost: u32_at(r, 2)? as i32,
                name: str_at(&set, r, 3).unwrap_or_default(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_one_shipped_package_is_the_test_package() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_packages(&mut chain).expect("load Package.dbc");
        assert_eq!(
            rows,
            [PackageRow {
                id: 2,
                icon: "INV_BOX_04".into(),
                cost: 10,
                name: "Test Package".into(),
            }]
        );
    }
}
