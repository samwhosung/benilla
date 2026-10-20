//! `TaxiPathNode.dbc`, the ordered waypoints of every flight path and every `MO_TRANSPORT` (boat,
//! zeppelin), keyed by `TaxiPath.dbc` id; the layout is vmangos's `TaxiPathNodeEntry`
//! (`DBCStructure.h:696-707`). A path can cross maps (302, Orgrimmar to Undercity, spans 1 and 0),
//! and the transport timetable (`crate::transports`) turns a map change into a teleport keyframe.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const TAXI_PATH_NODE: &str = "DBFilesClient\\TaxiPathNode.dbc";

/// One `TaxiPathNode.dbc` row, a waypoint on a taxi or transport path.
#[derive(Clone, Copy, Debug)]
pub struct TaxiPathNode {
    /// The row's own id, meaningless outside this table.
    pub id: u32,
    /// The `TaxiPath.dbc` id this node belongs to.
    pub path_id: u32,
    /// 0-based position in the path, the order the catalog stores each path in.
    pub node_index: u32,
    /// The map this node's `pos` is expressed in (`Map.dbc` id).
    pub map_id: u32,
    /// World position `(x, y, z)` on `map_id`.
    pub pos: [f32; 3],
    /// Raw `actionFlag`: `& 2` marks a station stop (the reference tests the bit at `0x5f4e37`,
    /// vmangos `== 2`, the same on the shipped data) and `& 1` a teleport (`TransportMgr.cpp:134`).
    pub flags: u32,
    /// Stop delay in whole seconds, set on stops only.
    pub delay: u32,
}

/// `TaxiPathNode.dbc` rows grouped by `PathID`, each path's `Vec` sorted by `NodeIndex`.
pub struct TaxiPathNodes {
    paths: HashMap<u32, Vec<TaxiPathNode>>,
}

impl TaxiPathNodes {
    /// These waypoints grouped by `PathID`, each path in `NodeIndex` order.
    pub fn from_rows(rows: impl IntoIterator<Item = TaxiPathNode>) -> Self {
        let mut paths: HashMap<u32, Vec<TaxiPathNode>> = HashMap::new();
        for node in rows {
            paths.entry(node.path_id).or_default().push(node);
        }
        for nodes in paths.values_mut() {
            nodes.sort_by_key(|n| n.node_index);
        }
        Self { paths }
    }

    /// A path's nodes in `NodeIndex` order.
    pub fn path(&self, path_id: u32) -> Option<&[TaxiPathNode]> {
        self.paths.get(&path_id).map(Vec::as_slice)
    }

    /// Every path, in no particular order.
    pub fn paths(&self) -> impl Iterator<Item = (u32, &[TaxiPathNode])> {
        self.paths.iter().map(|(&id, nodes)| (id, nodes.as_slice()))
    }

    /// The number of paths, not nodes.
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

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

/// Read `TaxiPathNode.dbc` off the patch chain into a [`TaxiPathNodes`].
pub fn load_taxi_path_nodes(chain: &mut Chain) -> Result<TaxiPathNodes> {
    let bytes = chain
        .read_file(TAXI_PATH_NODE)
        .context("reading TaxiPathNode.dbc")?;
    let rs = parse(&bytes, schema(), "TaxiPathNode")?;
    let rows = rs.records().iter().filter_map(|r| {
        let (x, y, z) = (f32_at(r, 4)?, f32_at(r, 5)?, f32_at(r, 6)?);
        Some(TaxiPathNode {
            id: u32_at(r, 0)?,
            path_id: u32_at(r, 1)?,
            node_index: u32_at(r, 2).unwrap_or(0),
            map_id: u32_at(r, 3).unwrap_or(0),
            pos: [x, y, z],
            flags: u32_at(r, 7).unwrap_or(0),
            delay: u32_at(r, 8).unwrap_or(0),
        })
    });
    Ok(TaxiPathNodes::from_rows(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference reads a path's points as one run of consecutive rows in file order
    /// (`0x4db580` starts a run, `0x4db58c` grows it); the shipped file keeps each path's rows
    /// together and in `NodeIndex` order, the order [`TaxiPathNodes::path`] gives.
    #[test]
    fn shipped_paths_are_single_runs_in_node_index_order() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain.read_file(TAXI_PATH_NODE).expect("read TaxiPathNode");
        let rs = parse(&bytes, schema(), "TaxiPathNode").expect("parse TaxiPathNode");
        let mut runs = std::collections::HashSet::new();
        let mut prev: Option<(u32, u32)> = None;
        for r in rs.records() {
            let (path, index) = (u32_at(r, 1).unwrap(), u32_at(r, 2).unwrap());
            match prev {
                Some((p, i)) if p == path => assert_eq!(index, i + 1, "path {path}"),
                _ => assert!(runs.insert(path), "path {path} starts a second run"),
            }
            prev = Some((path, index));
        }
    }

    /// The 5875 layout on path 302 (Orgrimmar to Undercity, two 60 s stops) and path 285.
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
