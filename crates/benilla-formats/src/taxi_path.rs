//! `TaxiPath.dbc`: the direct hops between flight-master nodes and their fares, laid out as vmangos
//! `TaxiPathEntry` (`DBCStructure.h:688-694`); a multi-hop trip chains several. The waypoints are
//! `TaxiPathNode.dbc`'s.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const TAXI_PATH: &str = "DBFilesClient\\TaxiPath.dbc";

/// One direct hop between two flight-master nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaxiPath {
    pub id: u32,
    pub from: u32,
    pub to: u32,
    /// The undiscounted fare in copper; vmangos applies the reputation discount when it charges.
    pub cost: u32,
}

/// `TaxiPath.dbc` rows by id, plus a `(from, to)` lookup.
pub struct TaxiPaths {
    rows: HashMap<u32, TaxiPath>,
    by_pair: HashMap<(u32, u32), TaxiPath>,
}

impl TaxiPaths {
    /// A catalog of these hops in file order, a later row replacing an earlier one with its id. A
    /// pair keeps its first row: the reference's pair table walks the rows from the last, each
    /// overwriting its pair's cell (`0x4db387`-`0x4db3e2`).
    pub fn from_rows(hops: impl IntoIterator<Item = TaxiPath>) -> Self {
        let mut rows = HashMap::new();
        let mut by_pair = HashMap::new();
        for hop in hops {
            rows.insert(hop.id, hop);
            by_pair.entry((hop.from, hop.to)).or_insert(hop);
        }
        Self { rows, by_pair }
    }

    pub fn get(&self, id: u32) -> Option<&TaxiPath> {
        self.rows.get(&id)
    }

    /// The direct hop from `from` to `to`; the table is directed, one row per direction.
    pub fn between(&self, from: u32, to: u32) -> Option<&TaxiPath> {
        self.by_pair.get(&(from, to))
    }

    /// Every direct hop leaving `node`, in no order.
    pub fn paths_from(&self, node: u32) -> impl Iterator<Item = &TaxiPath> {
        self.rows.values().filter(move |p| p.from == node)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("TaxiPath");
    for name in ["ID", "FromTaxiNode", "ToTaxiNode", "Cost"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Read `TaxiPath.dbc` off the patch chain into a [`TaxiPaths`] catalog.
pub fn load_taxi_paths(chain: &mut Chain) -> Result<TaxiPaths> {
    let bytes = chain.read_file(TAXI_PATH).context("reading TaxiPath.dbc")?;
    let rs = parse(&bytes, schema(), "TaxiPath")?;
    let hops = rs.records().iter().filter_map(|r| {
        Some(TaxiPath {
            id: u32_at(r, 0)?,
            from: u32_at(r, 1)?,
            to: u32_at(r, 2)?,
            cost: u32_at(r, 3).unwrap_or(0),
        })
    });
    Ok(TaxiPaths::from_rows(hops))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path table keeps the first row of a pair: `0x4db387` walks the rows from the last, each
    /// overwriting the pair's cell (`0x4db3e2`).
    #[test]
    fn a_duplicated_pair_keeps_its_first_row() {
        let hop = |id, cost| TaxiPath {
            id,
            from: 1,
            to: 2,
            cost,
        };
        let cat = TaxiPaths::from_rows([hop(7, 10), hop(9, 20)]);
        assert_eq!(cat.between(1, 2).map(|p| (p.id, p.cost)), Some((7, 10)));
    }

    /// Hop 6 runs Stormwind (node 2) to Sentinel Hill (node 4) for 110 copper.
    #[test]
    fn real_taxi_path_layout_sanity() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_taxi_paths(&mut chain).expect("load TaxiPath");
        assert_eq!(cat.len(), 287, "1.12 ships 287 taxi path hops");

        let hop = cat.between(2, 4).expect("a direct 2 -> 4 hop exists");
        assert_eq!(hop.id, 6);
        assert_eq!(hop.cost, 110);
        assert!(hop.cost > 0, "a real hop always carries a nonzero fare");

        assert!(cat.paths_from(2).any(|p| p.to == 4 && p.cost == 110));
    }
}
