//! The taxi map's data: the DBC catalogs, the reference's map projection and route search, and
//! the node list they build.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use benilla_formats::{
    load_taxi_nodes, load_taxi_paths, load_world_map_continent_catalog, TaxiNodes, TaxiPaths,
    WorldMapContinent, WorldMapContinentCatalog,
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
    let loaded = load_taxi_nodes(&mut chain).and_then(|nodes| {
        let paths = load_taxi_paths(&mut chain)?;
        let continents = load_world_map_continent_catalog(&mut chain)?;
        Ok((nodes, paths, continents))
    });
    drop(chain);
    match loaded {
        Ok((nodes, paths, continents)) => {
            info!(
                "ui_taxi: catalogs loaded — {} nodes, {} paths, {} continents",
                nodes.len(),
                paths.len(),
                continents.len()
            );
            commands.insert_resource(TaxiCatalogs {
                nodes,
                paths,
                continents,
            });
        }
        Err(e) => error!("ui_taxi: DBC catalogs failed to load, taxi map disabled: {e:#}"),
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
pub(super) fn build_nodes(
    open: &TaxiOpen,
    cat: &TaxiCatalogs,
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
            cost,
            routes,
        });
        resolved.push(ResolvedTaxiNode { chain, cost });
    }
    Some((map_id, ui, resolved))
}

/// The message an `SMSG_ACTIVATETAXIREPLY` code shows: the reference's handler (`FUN_005ed1e0`)
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
mod tests {
    use super::*;

    fn mask_of(ids: &[u32]) -> TaxiMask {
        let mut mask = TaxiMask::default();
        for &id in ids {
            let word = ((id - 1) / 32) as usize;
            let bit = (id - 1) % 32;
            mask.0[word] |= 1 << bit;
        }
        mask
    }

    #[test]
    fn shortest_route_minimizes_geo_distance_and_carries_fare() {
        // Nodes 1, 2 and 4 at 0, 40 and 100 on a line. The direct 1→4 edge is 150 long for fare
        // 10; 1→2→4 is 40 + 60 = 100 long for fare 5 + 20 = 25, and wins.
        let positions: HashMap<u32, f32> = HashMap::from([(1, 0.0), (2, 40.0), (4, 100.0)]);
        let dists: HashMap<(u32, u32), f32> = HashMap::from([((1, 4), 150.0)]);
        let dist = |a: u32, b: u32| {
            dists
                .get(&(a, b))
                .copied()
                .unwrap_or_else(|| (positions[&a] - positions[&b]).abs())
        };
        let mut edges: HashMap<u32, Vec<Edge>> = HashMap::new();
        edges.insert(1, vec![(4, 10), (2, 5)]);
        edges.insert(2, vec![(4, 20)]);
        let known = mask_of(&[1, 2, 4]);
        let (chain, fare) = shortest_route(
            &known,
            |n| edges.get(&n).cloned().unwrap_or_default(),
            dist,
            1,
            4,
        )
        .expect("reachable");
        assert_eq!(
            chain,
            vec![1, 2, 4],
            "the geographically shorter detour wins regardless of fare"
        );
        assert_eq!(
            fare, 25,
            "the fare is the chosen chain's sum, not a minimum"
        );

        // An exact distance tie (direct 100 vs 40+60) breaks toward fewer hops.
        let (chain, fare) = shortest_route(
            &known,
            |n| edges.get(&n).cloned().unwrap_or_default(),
            |a, b| (positions[&a] - positions[&b]).abs(),
            1,
            4,
        )
        .expect("reachable");
        assert_eq!(chain, vec![1, 4], "an exact-tie breaks toward fewer hops");
        assert_eq!(fare, 10);

        // Node 5 has no edge from the known graph at all.
        assert!(
            shortest_route(
                &known,
                |n| edges.get(&n).cloned().unwrap_or_default(),
                dist,
                1,
                5
            )
            .is_none(),
            "a truly disconnected node has no route"
        );

        // An edge to node 3 exists, but 3 is not known.
        let mut gated: HashMap<u32, Vec<Edge>> = HashMap::new();
        gated.insert(1, vec![(3, 5)]);
        let known_without_3 = mask_of(&[1]);
        assert!(
            shortest_route(
                &known_without_3,
                |n| gated.get(&n).cloned().unwrap_or_default(),
                |_, _| 1.0,
                1,
                3
            )
            .is_none(),
            "an edge into an undiscovered node is not a usable route"
        );
    }

    /// A non-square rect, where cross-axis and same-axis denominators differ.
    #[test]
    fn projection_uses_cross_axis_denominators() {
        let cont = WorldMapContinent {
            map_id: 0,
            left_boundary: 0,
            right_boundary: 0,
            top_boundary: 0,
            bottom_boundary: 0,
            offset_x: 0.0,
            offset_y: 0.0,
            scale: 1.0,
            taxi_min: (0.0, 0.0),    // (Xmin, Ymin)
            taxi_max: (100.0, 50.0), // (Xmax, Ymax)
        };
        // worldX 50, worldY 0: u = (50 - 0) / 100 = 0.5 and v = (50 - 0) / 50 = 1.0; same-axis
        // denominators would give the reverse.
        let (u, v) = project(&cont, 50.0, 0.0);
        assert!((u - 0.5).abs() < 1e-6, "u divides by the X-span (got {u})");
        assert!((v - 1.0).abs() < 1e-6, "v divides by the Y-span (got {v})");
    }

    /// Every node on maps 0 and 1 but id 3 (Programmer Isle, a debug row with no `TaxiPath` edge)
    /// projects inside the map, Ironforge above Stormwind and Menethil Harbor west of Lakeshire.
    #[test]
    fn real_taxi_projection_matches_geography() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let nodes = load_taxi_nodes(&mut chain).expect("load TaxiNodes");
        let continents = load_world_map_continent_catalog(&mut chain).expect("load WMC");

        for map_id in [0u32, 1u32] {
            let cont = continents.get(map_id).expect("continent row");
            for n in nodes.rows().filter(|n| n.map_id == map_id) {
                if n.id == 3 {
                    continue; // Programmer Isle
                }
                let (x, y) = project(cont, n.pos[0], n.pos[1]);
                assert!(
                    (0.02..=0.98).contains(&x) && (0.02..=0.98).contains(&y),
                    "{} (id {}, map {map_id}) projects out of bounds: ({x}, {y})",
                    n.name,
                    n.id
                );
            }
        }

        let ek = continents.get(0).expect("Eastern Kingdoms");
        let stormwind = nodes.get(2).expect("Stormwind");
        let ironforge = nodes.get(6).expect("Ironforge");
        let menethil = nodes.get(7).expect("Menethil Harbor");
        let lakeshire = nodes.get(5).expect("Lakeshire");
        assert_eq!(stormwind.name, "Stormwind, Elwynn");
        assert_eq!(ironforge.name, "Ironforge, Dun Morogh");
        assert_eq!(menethil.name, "Menethil Harbor, Wetlands");
        assert_eq!(lakeshire.name, "Lakeshire, Redridge");

        let (_, sw_y) = project(ek, stormwind.pos[0], stormwind.pos[1]);
        let (_, if_y) = project(ek, ironforge.pos[0], ironforge.pos[1]);
        assert!(
            if_y > sw_y,
            "Ironforge ({if_y}) should project above Stormwind ({sw_y}) — Dun Morogh is north"
        );

        let (men_x, _) = project(ek, menethil.pos[0], menethil.pos[1]);
        let (lake_x, _) = project(ek, lakeshire.pos[0], lakeshire.pos[1]);
        assert!(
            men_x < lake_x,
            "Menethil Harbor ({men_x}) should project west of Lakeshire ({lake_x}) — Wetlands is west"
        );
    }

    /// The real Stormwind (2) to Sentinel Hill (4) hop, `TaxiPath` id 6 at 110 copper; with every
    /// node known, the ones Stormwind cannot route to are left out.
    #[test]
    fn build_nodes_classifies_a_real_known_hop() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = TaxiCatalogs {
            nodes: load_taxi_nodes(&mut chain).expect("load TaxiNodes"),
            paths: load_taxi_paths(&mut chain).expect("load TaxiPath"),
            continents: load_world_map_continent_catalog(&mut chain).expect("load WMC"),
        };
        let open = TaxiOpen {
            flightmaster: 0x42,
            nearest_node: 2,
            known: mask_of(&[2, 4]),
        };
        let (map_id, ui, resolved) = build_nodes(&open, &cat).expect("map builds");
        assert_eq!(map_id, 0, "the continent is the nearest node's own map");
        assert_eq!(ui.len(), 2, "only the two known EK nodes are visible");

        let sw_idx = ui
            .iter()
            .position(|n| n.name == "Stormwind, Elwynn")
            .expect("Stormwind visible");
        assert_eq!(ui[sw_idx].node_type, TaxiNodeType::Current);
        assert_eq!(resolved[sw_idx].chain, vec![2]);

        let sh_idx = ui
            .iter()
            .position(|n| n.name == "Sentinel Hill, Westfall")
            .expect("Sentinel Hill visible");
        assert_eq!(ui[sh_idx].node_type, TaxiNodeType::Reachable);
        assert_eq!(ui[sh_idx].cost, 110);
        assert_eq!(ui[sh_idx].routes.len(), 1);
        assert_eq!(resolved[sh_idx].chain, vec![2, 4]);
        assert_eq!(resolved[sh_idx].cost, 110);

        // Every node known: the Horde-only stops (Grom'gol, Kargath) have no route from Stormwind.
        let all_known = TaxiMask([u32::MAX; 8]);
        let open = TaxiOpen {
            flightmaster: 0x42,
            nearest_node: 2,
            known: all_known,
        };
        let (_, ui, _) = build_nodes(&open, &cat).expect("map builds");
        let ek_known_total = cat.nodes.rows().filter(|n| n.map_id == 0).count();
        assert!(
            ui.len() < ek_known_total,
            "some all-known EK nodes are unroutable from Stormwind and must be dropped \
             ({} shown of {ek_known_total})",
            ui.len()
        );
        assert!(
            ui.iter().all(|n| n.node_type != TaxiNodeType::Distant),
            "the DISTANT classification is a dead branch — never produced"
        );
    }
}
