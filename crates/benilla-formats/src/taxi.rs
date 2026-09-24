//! `TaxiPathNode.dbc` — a taxi/transport path's ordered waypoints (map + world position, a
//! station-stop flag, and a stop delay). Every flight path AND every `MO_TRANSPORT`
//! (boat/zeppelin) is one `TaxiPath.dbc` id whose nodes live here, keyed by
//! `TaxiPath.dbc`'s id (this table's `PathID` column) — `TaxiPath.dbc` itself only carries the
//! `(fromNode, toNode, cost)` triple for the flight-master UI and is not adapted separately yet.
//!
//! **9 fields (verified this session, raw-parsed against build 5875):**
//! `ID(0), PathID(1), NodeIndex(2), MapID(3), LocX(4), LocY(5), LocZ(6), Flags(7), Delay(8)` —
//! matching vmangos's own `TaxiPathNodeEntry` column comments (`DBCStructure.h:696-707`,
//! `m_ID/m_PathID/m_NodeIndex/m_ContinentID/m_LocX/m_LocY/m_LocZ/m_flags/m_delay`) field-for-field.
//! A path's nodes are **not** confined to one continent: path 302 (Orgrimmar–Undercity) has 36
//! nodes spanning `MapID` `{1, 0}` (Kalimdor, Azeroth) — the map-change itself is where the
//! transport timetable builder (`crate::transports`) inserts a teleport keyframe, never a spline
//! segment. `Flags == 2` marks a station stop (vmangos `KeyFrame::IsStopFrame`, exact equality,
//! not a bitmask test) and pairs with a nonzero `Delay` (seconds) at that node: path 302 stops
//! twice, 60 s each; path 285 (27 nodes) stops at node indexes 4 and 20.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const TAXI_PATH_NODE: &str = "DBFilesClient\\TaxiPathNode.dbc";

/// One `TaxiPathNode.dbc` row — a single waypoint on a taxi/transport path.
#[derive(Clone, Copy, Debug)]
pub struct TaxiPathNode {
    /// The row's own id (`ID`) — a global waypoint id, not meaningful outside this table.
    pub id: u32,
    /// `TaxiPath.dbc` id this node belongs to (the catalog's key).
    pub path_id: u32,
    /// Ordinal position within the path, `0`-based — the catalog sorts each path's nodes by
    /// this field, so the stored `Vec` order already matches path traversal order.
    pub node_index: u32,
    /// The map this node's `pos` is expressed in (`Map.dbc` id).
    pub map_id: u32,
    /// World position `(x, y, z)` on `map_id`.
    pub pos: [f32; 3],
    /// Raw `actionFlag`. Bit `1` (`flags & 2`) marks a station stop — the client tests it as a
    /// bitmask (`0x5f4e37`; vmangos's `IsStopFrame` uses `== 2`, identical on the live data);
    /// bit `0` (`flags & 1`) marks a map-change/teleport trigger in the
    /// transport timetable builder.
    pub flags: u32,
    /// Stop delay in whole seconds — only meaningful when `flags == 2`; `0` otherwise.
    pub delay: u32,
}

/// `TaxiPathNode.dbc` rows grouped by `PathID`, each path's `Vec` sorted by `NodeIndex`.
pub struct TaxiPathNodes {
    paths: HashMap<u32, Vec<TaxiPathNode>>,
}

impl TaxiPathNodes {
    /// A path's nodes, in `NodeIndex` order, or `None` if the path id is unknown.
    pub fn path(&self, path_id: u32) -> Option<&[TaxiPathNode]> {
        self.paths.get(&path_id).map(Vec::as_slice)
    }

    /// Every path, in no particular order.
    pub fn paths(&self) -> impl Iterator<Item = (u32, &[TaxiPathNode])> {
        self.paths.iter().map(|(&id, nodes)| (id, nodes.as_slice()))
    }

    /// Number of distinct paths (not the total node count).
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// 9 fields per the module doc.
fn schema() -> Schema {
    let mut s = Schema::new("TaxiPathNode");
    for name in ["ID", "PathID", "NodeIndex", "MapID"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for name in ["LocX", "LocY", "LocZ"] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    for name in ["Flags", "Delay"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Read `TaxiPathNode.dbc` off the patch chain into a [`TaxiPathNodes`], each path's `Vec`
/// sorted by `NodeIndex`.
pub fn load_taxi_path_nodes(chain: &mut Chain) -> Result<TaxiPathNodes> {
    let bytes = chain
        .read_file(TAXI_PATH_NODE)
        .context("reading TaxiPathNode.dbc")?;
    let rs = parse(&bytes, schema(), "TaxiPathNode")?;
    let mut paths: HashMap<u32, Vec<TaxiPathNode>> = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let Some(path_id) = u32_at(r, 1) else {
            continue;
        };
        let (Some(x), Some(y), Some(z)) = (f32_at(r, 4), f32_at(r, 5), f32_at(r, 6)) else {
            continue;
        };
        let node = TaxiPathNode {
            id,
            path_id,
            node_index: u32_at(r, 2).unwrap_or(0),
            map_id: u32_at(r, 3).unwrap_or(0),
            pos: [x, y, z],
            flags: u32_at(r, 7).unwrap_or(0),
            delay: u32_at(r, 8).unwrap_or(0),
        };
        paths.entry(path_id).or_default().push(node);
    }
    for nodes in paths.values_mut() {
        nodes.sort_by_key(|n| n.node_index);
    }
    Ok(TaxiPathNodes { paths })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 5875 table proves the layout and the two pinned paths from the module doc: path
    /// 302 (Orgrimmar–Undercity, 36 nodes spanning maps `{0,1}`, two `Flags==2` stops at 60 s
    /// each) and path 285 (27 nodes, stops at indexes 4 and 20). Skips without client data.
    #[test]
    fn real_taxi_path_node_layout_sanity() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");
        assert!(
            cat.len() > 100,
            "many taxi/transport paths load: {}",
            cat.len()
        );

        let path302 = cat.path(302).expect("path 302 exists");
        assert_eq!(path302.len(), 36, "path 302 has 36 nodes");
        // sorted by node_index
        for w in path302.windows(2) {
            assert!(
                w[0].node_index < w[1].node_index,
                "nodes sorted by node_index: {:?} then {:?}",
                w[0].node_index,
                w[1].node_index
            );
        }
        let maps: std::collections::BTreeSet<u32> = path302.iter().map(|n| n.map_id).collect();
        assert_eq!(
            maps,
            std::collections::BTreeSet::from([0, 1]),
            "path 302 spans maps {{0,1}}"
        );
        let mut flag_histogram: HashMap<u32, u32> = HashMap::new();
        for n in path302 {
            *flag_histogram.entry(n.flags).or_insert(0) += 1;
        }
        assert_eq!(flag_histogram.get(&0).copied(), Some(34));
        assert_eq!(flag_histogram.get(&2).copied(), Some(2));
        let flag2_delays: Vec<u32> = path302
            .iter()
            .filter(|n| n.flags == 2)
            .map(|n| n.delay)
            .collect();
        assert_eq!(flag2_delays, vec![60, 60], "both flag-2 nodes delay 60s");

        let path285 = cat.path(285).expect("path 285 exists");
        assert_eq!(path285.len(), 27, "path 285 has 27 nodes");
        let stop_indexes: Vec<u32> = path285
            .iter()
            .filter(|n| n.flags == 2)
            .map(|n| n.node_index)
            .collect();
        assert_eq!(stop_indexes, vec![4, 20], "path 285 stops at indexes 4, 20");

        let path292 = cat.path(292).expect("path 292 exists");
        assert!(!path292.is_empty(), "path 292 is non-empty");
    }
}
