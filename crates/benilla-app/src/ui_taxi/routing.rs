//! The taxi map's data: the DBC catalogs, the reference's map projection and route search, and
//! the node list they build.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use benilla_formats::{
    load_taxi_nodes, load_taxi_path_nodes, load_taxi_paths, load_world_map_continent_catalog,
    FactionCatalog, TaxiNodes, TaxiPathNodes, TaxiPaths, WorldMapContinent,
    WorldMapContinentCatalog,
};
use benilla_protocol::messages::{taxi_reply, TaxiMask};
use bevy::prelude::*;

use benilla_ui::script::{TaxiNodeType, TaxiUiNode};

use benilla_assets::{LockRecover, WorldAssets};

use super::TaxiOpen;

/// `TaxiNodes.dbc`, `TaxiPath.dbc` (the direct hops and their fares) and `WorldMapContinent.dbc`
/// (each continent's taxi-map rect, its `taxi_min`/`taxi_max`).
#[derive(Resource)]
pub(crate) struct TaxiCatalogs {
    nodes: TaxiNodes,
    pub(super) paths: TaxiPaths,
    continents: WorldMapContinentCatalog,
}

/// Load [`TaxiCatalogs`] once, when the patch chain is open; nothing reads them before a flight
/// master opens the map.
pub(super) fn load_taxi_catalogs(
    mut done: Local<bool>,
    world_assets: Option<Res<WorldAssets>>,
    mut commands: Commands,
) {
    if *done {
        return;
    }
    let Some(assets) = world_assets else {
        return;
    };
    *done = true;
    let mut chain = assets.chain.lock_recover();
    let loaded = TaxiCatalogs::load(&mut chain);
    drop(chain);
    match loaded {
        Ok(cat) => {
            info!(
                "ui_taxi: catalogs loaded — {} nodes, {} paths, {} continents",
                cat.nodes.len(),
                cat.paths.len(),
                cat.continents.len()
            );
            commands.insert_resource(cat);
        }
        Err(e) => error!("ui_taxi: DBC catalogs failed to load, taxi map disabled: {e:#}"),
    }
}

impl TaxiCatalogs {
    /// The three tables off the patch chain.
    pub(super) fn load(chain: &mut benilla_formats::Chain) -> anyhow::Result<Self> {
        Ok(Self::from_parts(
            load_taxi_nodes(chain)?,
            load_taxi_paths(chain)?,
            &load_taxi_path_nodes(chain)?,
            load_world_map_continent_catalog(chain)?,
        ))
    }

    /// The catalogs from their tables, `TaxiPathNode.dbc`'s points read once.
    fn from_parts(
        nodes: TaxiNodes,
        paths: TaxiPaths,
        _points: &TaxiPathNodes,
        continents: WorldMapContinentCatalog,
    ) -> Self {
        Self {
            nodes,
            paths,
            continents,
        }
    }
}

/// A node's world `(x, y)` on the taxi map's 0..1 space, as the reference computes it
/// (`0x4db958`), with the rect from `WorldMapContinent.dbc` fields 9-12 of the node's continent:
///
/// ```text
/// u = (Ymax − worldY) / (Xmax − Xmin)    ; world Y (west+) → horizontal, inverted
/// v = (worldX − Xmin) / (Ymax − Ymin)    ; world X (north+) → vertical, from BOTTOMLEFT
/// ```
///
/// The denominators are cross-axis; the reference's route projector `0x4dc890` uses same-axis
/// ones, and the two agree because every shipped continent's rect is square.
pub(crate) fn project(cont: &WorldMapContinent, world_x: f32, world_y: f32) -> (f32, f32) {
    let (min_x, min_y) = cont.taxi_min;
    let (max_x, max_y) = cont.taxi_max;
    let x = (max_y - world_y) / (max_x - min_x);
    let y = (world_x - min_x) / (max_y - min_y);
    (x, y)
}

/// One outgoing edge, `(to, fare)`.
type Edge = (u32, u32);

/// The route from `from` to `to` through known nodes, minimizing summed distance with the fare
/// carried along, as the reference's relaxation (`0x4dbce0`) does; ties go to fewer hops. `dist`
/// is the per-edge metric: `build_nodes` passes the straight 3-D distance between the two nodes,
/// where the reference's `0x4dbbd0` sums the length of the path's own points. `from` counts as
/// known: the server opens the map only on a visited node. The chain runs `from` to `to`.
fn shortest_route(
    known: &TaxiMask,
    edges: impl Fn(u32) -> Vec<Edge>,
    dist: impl Fn(u32, u32) -> f32,
    from: u32,
    to: u32,
) -> Option<(Vec<u32>, u32)> {
    if from == to {
        return Some((vec![from], 0));
    }
    // Dijkstra keyed on (distance, hops): a non-negative `f32`'s bits order like its value, and
    // `Reverse` makes the max-heap a min-heap.
    let mut best: HashMap<u32, (u32, u32)> = HashMap::new(); // node → (dist_bits, hops)
    let mut fares: HashMap<u32, u32> = HashMap::new();
    let mut prev: HashMap<u32, u32> = HashMap::new();
    let mut heap = BinaryHeap::new();
    best.insert(from, (0, 0));
    fares.insert(from, 0);
    heap.push(Reverse((0u32, 0u32, from)));

    while let Some(Reverse((dist_bits, hops, node))) = heap.pop() {
        if node == to {
            break; // the first pop of `to` is optimal
        }
        if best.get(&node) != Some(&(dist_bits, hops)) {
            continue; // a stale entry: a better path already won
        }
        for (next, edge_fare) in edges(node) {
            if !known.is_known(next) {
                continue; // the search never steps through an undiscovered node
            }
            let leg = dist(node, next).max(0.0);
            let candidate = ((f32::from_bits(dist_bits) + leg).to_bits(), hops + 1);
            if best.get(&next).is_none_or(|&b| candidate < b) {
                best.insert(next, candidate);
                fares.insert(next, fares[&node] + edge_fare);
                prev.insert(next, node);
                heap.push(Reverse((candidate.0, candidate.1, next)));
            }
        }
    }

    best.get(&to)?;
    let total_fare = *fares.get(&to)?;
    let mut chain = vec![to];
    let mut cur = to;
    while cur != from {
        cur = *prev.get(&cur)?;
        chain.push(cur);
    }
    chain.reverse();
    Some((chain, total_fare))
}

/// `TaxiNodeCost`'s fare (`0x4dc3d0`): the price times `1 - discount`, both in f64 with nothing
/// stored (`fsubr`, `fmul st,st(1)`), the product stored as an f32 and rounded half to even
/// (`fstp dword`, `fistp dword`).
fn shown_fare(price: u32, discount: f64) -> u32 {
    let product = (f64::from(price) * (1.0 - discount)) as f32;
    f64::from(product).round_ties_even() as u32
}

/// A listed node's chain from the nearest node (the node alone for `Current`) and its fare, for
/// `drain_taxi`.
pub(super) struct ResolvedTaxiNode {
    pub(super) chain: Vec<u32>,
    pub(super) cost: u32,
}

/// The pushed node list's routes, index for index, so `TakeTaxiNode(i)` finds its route.
#[derive(Resource, Default)]
pub(super) struct TaxiRouteCache(pub(super) Vec<ResolvedTaxiNode>);

/// The listed nodes: every known node on the current node's continent, which the reference caches
/// off the packet (`DAT_00bb4a80+4`), by id. The current node is `Current`, a routable one
/// `Reachable` with its fare and route segments, and an unroutable one is left out, where the
/// reference keeps it typed `NONE`, which stock `TaxiFrame.lua` hides; its `DISTANT` type is
/// never produced. Returns the continent's map id, the Lua nodes and their routes, in one order.
///
/// A Lua node's fare takes the flight master's price `discount` ([`shown_fare`]), and is 0 with
/// none, when the flight master or the player does not resolve (`0x4dc3fa`, `0x4dc423`); the
/// route's fare stays the raw sum, which `CMSG_ACTIVATETAXIEXPRESS` carries (`0x4dc96f`).
pub(super) fn build_nodes(
    open: &TaxiOpen,
    cat: &TaxiCatalogs,
    discount: Option<f64>,
) -> Option<(u32, Vec<TaxiUiNode>, Vec<ResolvedTaxiNode>)> {
    build_nodes_for(open, cat, discount, None)
}

/// [`build_nodes`] for a player, its race and the faction tables.
pub(super) fn build_nodes_for(
    open: &TaxiOpen,
    cat: &TaxiCatalogs,
    discount: Option<f64>,
    _player: Option<(u8, &FactionCatalog)>,
) -> Option<(u32, Vec<TaxiUiNode>, Vec<ResolvedTaxiNode>)> {
    let map_id = cat.nodes.get(open.nearest_node)?.map_id;
    let cont = cat.continents.get(map_id)?;
    let dist = |a: u32, b: u32| -> f32 {
        match (cat.nodes.get(a), cat.nodes.get(b)) {
            (Some(a), Some(b)) => {
                let (dx, dy, dz) = (
                    a.pos[0] - b.pos[0],
                    a.pos[1] - b.pos[1],
                    a.pos[2] - b.pos[2],
                );
                (dx * dx + dy * dy + dz * dz).sqrt()
            }
            _ => f32::MAX / 4.0, // an edge into a node the catalog lacks never wins
        }
    };
    let mut rows: Vec<_> = cat
        .nodes
        .rows()
        .filter(|n| n.map_id == map_id && open.known.is_known(n.id))
        .collect();
    rows.sort_by_key(|n| n.id);

    let mut ui = Vec::new();
    let mut resolved = Vec::new();
    for n in rows {
        let pos = project(cont, n.pos[0], n.pos[1]);
        if n.id == open.nearest_node {
            ui.push(TaxiUiNode {
                name: n.name.clone(),
                node_type: TaxiNodeType::Current,
                pos,
                cost: 0,
                routes: Vec::new(),
            });
            resolved.push(ResolvedTaxiNode {
                chain: vec![n.id],
                cost: 0,
            });
            continue;
        }
        let Some((chain, cost)) = shortest_route(
            &open.known,
            |from| cat.paths.paths_from(from).map(|p| (p.to, p.cost)).collect(),
            dist,
            open.nearest_node,
            n.id,
        ) else {
            continue; // unroutable: left out
        };
        // Each hop's segment for `GetNumRoutes` and `TaxiGetSrcX`..`TaxiGetDestY`, through this
        // continent's rect even for a node off it.
        let routes = chain
            .windows(2)
            .filter_map(|w| {
                let a = cat.nodes.get(w[0])?;
                let b = cat.nodes.get(w[1])?;
                let (ax, ay) = project(cont, a.pos[0], a.pos[1]);
                let (bx, by) = project(cont, b.pos[0], b.pos[1]);
                Some([ax, ay, bx, by])
            })
            .collect();
        ui.push(TaxiUiNode {
            name: n.name.clone(),
            node_type: TaxiNodeType::Reachable,
            pos,
            cost: discount.map_or(0, |d| shown_fare(cost, d)),
            routes,
        });
        resolved.push(ResolvedTaxiNode { chain, cost });
    }
    Some((map_id, ui, resolved))
}

/// The message an `SMSG_ACTIVATETAXIREPLY` code shows: the reference's handler (`0x5ed1e0`)
/// closes the map on 0, shows `[0x85fedc + 4*code]` through `DisplayError` (`0x496720`) for
/// 1-12, and does nothing from 13 up. The message row picks the surface, the yellow info line for
/// seven of the twelve, and the sound: `ERR_TAXINOTENOUGHMONEY` speaks line `0x36`.
pub(super) fn taxi_error_key(code: u32) -> Option<&'static str> {
    Some(match code {
        taxi_reply::OK => return None,
        taxi_reply::UNSPECIFIED_SERVER_ERROR => "ERR_TAXIUNSPECIFIEDSERVERERROR", // 0xac
        taxi_reply::NO_SUCH_PATH => "ERR_TAXINOSUCHPATH",                         // 0xab
        taxi_reply::NOT_ENOUGH_MONEY => "ERR_TAXINOTENOUGHMONEY",                 // 0xad
        taxi_reply::TOO_FAR => "ERR_TAXITOOFARAWAY",                              // 0xae
        taxi_reply::NO_VENDOR_NEARBY => "ERR_TAXINOVENDORNEARBY",                 // 0xaf
        taxi_reply::NOT_VISITED => "ERR_TAXINOTVISITED",                          // 0xb0
        taxi_reply::BUSY => "ERR_TAXIPLAYERBUSY",                                 // 0xb1
        taxi_reply::ALREADY_MOUNTED => "ERR_TAXIPLAYERALREADYMOUNTED",            // 0xb2
        taxi_reply::SHAPESHIFTED => "ERR_TAXIPLAYERSHAPESHIFTED",                 // 0xb3
        taxi_reply::PLAYER_MOVING => "ERR_TAXIPLAYERMOVING",                      // 0xb4
        taxi_reply::SAME_NODE => "ERR_TAXISAMENODE",                              // 0xaa
        taxi_reply::NOT_STANDING => "ERR_TAXINOTSTANDING",                        // 0xb6
        _ => return None,
    })
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
