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
    by_pair: HashMap<(u32, u32), u32>,
}

impl TaxiPaths {
    pub fn get(&self, id: u32) -> Option<&TaxiPath> {
        self.rows.get(&id)
    }

    /// The direct hop from `from` to `to`; the table is directed, one row per direction.
    pub fn between(&self, from: u32, to: u32) -> Option<&TaxiPath> {
        self.by_pair
            .get(&(from, to))
            .and_then(|id| self.rows.get(id))
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
    let mut rows = HashMap::with_capacity(rs.records().len());
    let mut by_pair = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let Some(from) = u32_at(r, 1) else { continue };
        let Some(to) = u32_at(r, 2) else { continue };
        let cost = u32_at(r, 3).unwrap_or(0);
        rows.insert(id, TaxiPath { id, from, to, cost });
        by_pair.insert((from, to), id);
    }
    Ok(TaxiPaths { rows, by_pair })
}

#[cfg(test)]
mod tests {
    use super::*;

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
