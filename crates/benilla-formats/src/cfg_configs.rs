//! `Cfg_Configs.dbc`, the realm types. A realm's wire type (its realm-list record's first dword)
//! is joined against `RealmType` by a linear scan that takes the first match, in the realm list
//! (`GetRealmInfo`, `0x46efda`), the character screen's `GetServerName` (`0x46d280`) and at world
//! entry (`0x4015b1`, the latch `GetZonePVPInfo` reads).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const CFG_CONFIGS: &str = "DBFilesClient\\Cfg_Configs.dbc";

/// One realm type's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealmConfig {
    /// Column 1, the key a realm's wire type is matched against.
    pub realm_type: u32,
    /// Column 2, `PlayerKillingAllowed` (`+0x08`): `GetRealmInfo`'s `pvp`.
    pub pvp: bool,
    /// Column 3, `RoleplayingRealm` (`+0x0c`): `GetRealmInfo`'s `rp`.
    pub rp: bool,
}

/// The realm types, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RealmConfigs(Vec<RealmConfig>);

impl RealmConfigs {
    /// A table over rows given directly.
    pub fn from_rows(rows: Vec<RealmConfig>) -> Self {
        Self(rows)
    }

    /// The first row whose `RealmType` is `realm_type`, as every reader's scan stops there.
    pub fn get(&self, realm_type: u32) -> Option<&RealmConfig> {
        self.0.iter().find(|c| c.realm_type == realm_type)
    }

    /// The rows, in file order.
    pub fn rows(&self) -> &[RealmConfig] {
        &self.0
    }
}

/// `ID`, `RealmType`, `PlayerKillingAllowed`, `RoleplayingRealm`: the 4 columns × 0x10 bytes the
/// loader (`0x5413f0`) gates on.
fn cfg_configs_schema() -> Schema {
    let mut s = Schema::new("Cfg_Configs");
    for name in [
        "ID",
        "RealmType",
        "PlayerKillingAllowed",
        "RoleplayingRealm",
    ] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Every row of `Cfg_Configs.dbc`.
pub fn load_realm_configs(chain: &mut Chain) -> Result<RealmConfigs> {
    let bytes = chain
        .read_file(CFG_CONFIGS)
        .with_context(|| format!("reading {CFG_CONFIGS}"))?;
    let rs = parse(&bytes, cfg_configs_schema(), "Cfg_Configs")?;
    Ok(RealmConfigs(
        rs.records()
            .iter()
            .filter_map(|r| {
                Some(RealmConfig {
                    realm_type: u32_at(r, 1)?,
                    pvp: u32_at(r, 2)? != 0,
                    rp: u32_at(r, 3)? != 0,
                })
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped table: 11 rows, types 0 to 10 in order; PvP on 1, 3, 5, 8 and 10, RP on 6, 7
    /// and 8.
    #[test]
    fn the_shipped_table_is_eleven_types_with_pvp_on_1_3_5_8_10() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let configs = load_realm_configs(&mut chain).expect("Cfg_Configs.dbc");
        let rows: Vec<(u32, bool, bool)> = configs
            .rows()
            .iter()
            .map(|c| (c.realm_type, c.pvp, c.rp))
            .collect();
        assert_eq!(
            rows,
            [
                (0, false, false),
                (1, true, false),
                (2, false, false),
                (3, true, false),
                (4, false, false),
                (5, true, false),
                (6, false, true),
                (7, false, true),
                (8, true, true),
                (9, false, false),
                (10, true, false),
            ]
        );
        assert!(configs.get(11).is_none(), "no row past type 10");
    }

    /// The scan stops at the first match.
    #[test]
    fn a_duplicated_type_reads_its_first_row() {
        let row = |pvp| RealmConfig {
            realm_type: 4,
            pvp,
            rp: false,
        };
        let configs = RealmConfigs::from_rows(vec![row(true), row(false)]);
        assert!(configs.get(4).expect("row").pvp);
    }
}
