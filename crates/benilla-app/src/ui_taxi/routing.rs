//! The taxi domain logic split out of [`super`] purely for size (the ui_taxi module doc): the
//! static DBC catalogs, the byte-verified map projection and geo-distance route search (decision
//! 0496 — the 0484 fold-back), and the node list they build together. Pure/testable — no Bevy
//! system runs here except the catalog loader, which only reaches out for the patch chain.

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

/// The static DBC catalogs phase 2's node list/route computation reads: `TaxiNodes.dbc` (name +
/// world position + map, decision 0484 phase 1), `TaxiPath.dbc` (the direct-hop fare graph, phase
/// 1), and `WorldMapContinent.dbc` (the taxi-map projection rect per continent — decision 0203,
/// its `taxi_min`/`taxi_max` fields, byte-verified as the projection rect). Loaded once, the
/// [`crate::ui_world_map`] "gated on `WorldAssets`" idiom rather than Startup-ordered (the patch
/// chain opens asynchronously).
#[derive(Resource)]
pub(crate) struct TaxiCatalogs {
    nodes: TaxiNodes,
    /// `pub(super)`: the drain's direct-edge discriminator reads it.
    pub(super) paths: TaxiPaths,
    continents: WorldMapContinentCatalog,
}

/// Load [`TaxiCatalogs`] once the patch chain exists. Never re-runs past the first success/failure
/// (`Local<bool>`), and that is right *here* where it was wrong for the map catalog (2240): this
/// inserts a Bevy resource and pushes nothing into the VM, so no Lua caller can be early for it —
/// nothing asks until a flight master opens the window.
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

/// Project a node's world `(x, y)` onto the taxi map's normalized 0..1 space (the
/// FPU trace at `0x4db958`): rect = `WorldMapContinent.dbc` fields 9–12
/// (Xmin/Ymin/Xmax/Ymax), matched by continentId, and
///
/// ```text
/// u = (Ymax − worldY) / (Xmax − Xmin)    ; world Y (west+) → horizontal, inverted
/// v = (worldX − Xmin) / (Ymax − Ymin)    ; world X (north+) → vertical (BOTTOMLEFT-origin seam)
/// ```
///
/// The denominators are **cross-axis** — u divides by the X-span, v by the Y-span — exactly as
/// the binary computes them. This equals the naive own-extent form only because every shipped
/// continent's taxi rect is *square* in world units (a data invariant, not code — the ref's
/// route-segment projector `0x4dc890` uses the naive form and coincides for the same reason).
/// The axis mapping was additionally confirmed on real geography with negative controls
/// ([`tests::real_taxi_projection_matches_geography`]).
pub(crate) fn project(cont: &WorldMapContinent, world_x: f32, world_y: f32) -> (f32, f32) {
    let (min_x, min_y) = cont.taxi_min;
    let (max_x, max_y) = cont.taxi_max;
    let x = (max_y - world_y) / (max_x - min_x);
    let y = (world_x - min_x) / (max_y - min_y);
    (x, y)
}

/// One directed graph edge for [`shortest_route`]: `(to, fare)`. Decoupled from the `TaxiPath` DBC
/// row type so the route search is unit-testable on a synthetic graph — `TaxiPaths` (decision 0484
/// phase 1) has no public in-memory constructor, only the DBC loader.
type Edge = (u32, u32);

/// Shortest route from `from` to `to` over a directed graph, expansion restricted to nodes `known`
/// marks discovered — **the byte-verified metric** (decision 0496 folds back 0484, superseding
/// INTERIM I2's fare-Dijkstra): the client's route relaxation (`0x4dbce0`, metric
/// `0x4dbbd0`) minimizes **summed geographic distance**, carrying the money fare and the hop
/// count *alongside* the optimization, not in it. `edges(node)` returns `node`'s outgoing
/// `(to, fare)` pairs; `dist(a, b)` is the geographic metric between two node ids (production:
/// euclidean over `TaxiNodes.dbc` world positions; whether the ref's is 2-D or 3-D is unpinned —
/// node altitude differences are negligible against route lengths, so the choice is invisible on
/// real data). Distance ties break toward fewer hops — a determinism guard, not a byte law (real
/// float distances never tie). Returns the full node chain (`from` first, `to` last) and its
/// summed fare; `None` if `to` is unreachable through only-known nodes. `from` itself is trusted
/// known (SHOWTAXINODES never opens on an unvisited node — "first contact learns, never opens");
/// only the nodes an edge steps INTO are gated.
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
    // Dijkstra over (distance, hops), the fare carried per node. Distances are non-negative
    // f32s, whose IEEE bit patterns order identically to their values — `to_bits` makes the
    // heap key `Ord` without a float-wrapper type. `Reverse` flips the max-heap into a min-heap.
    let mut best: HashMap<u32, (u32, u32)> = HashMap::new(); // node → (dist_bits, hops)
    let mut fares: HashMap<u32, u32> = HashMap::new();
    let mut prev: HashMap<u32, u32> = HashMap::new();
    let mut heap = BinaryHeap::new();
    best.insert(from, (0, 0));
    fares.insert(from, 0);
    heap.push(Reverse((0u32, 0u32, from)));

    while let Some(Reverse((dist_bits, hops, node))) = heap.pop() {
        if node == to {
            break; // Dijkstra: the first pop of `to` is optimal for the (distance, hops) key.
        }
        if best.get(&node) != Some(&(dist_bits, hops)) {
            continue; // a stale heap entry — a better path to `node` already won
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

/// One visible node's route, resolved app-side for `drain_taxi` — the full node-id chain from
/// `nearest_node` (first) to this node (last), and its total fare. Empty for the `Current` node
/// itself (a single-node chain) and for a `Distant` (unreachable) node (empty); `drain_taxi`
/// no-ops on either. Kept out of the engine-facing [`TaxiUiNode`] — the Lua side never needs raw
/// node ids, only positions/costs/route-line segments.
pub(super) struct ResolvedTaxiNode {
    pub(super) chain: Vec<u32>,
    pub(super) cost: u32,
}

/// `feed_taxi`'s app-private mirror of the pushed `TaxiUiState`'s node list, index-aligned 1:1
/// (same iteration, same order) so `drain_taxi` can map a `TakeTaxiNode` 1-based index back to a
/// real route without the engine ever carrying one.
#[derive(Resource, Default)]
pub(super) struct TaxiRouteCache(pub(super) Vec<ResolvedTaxiNode>);

/// Build the visible node list (decision 0484 phase 2, corrected by the 0496 fold-back): the
/// continent is the **current node's own `TaxiNodes.dbc` continentId** — the ref caches it off
/// the SHOWTAXINODES packet's nearest node (`DAT_00bb4a80+4`), never a live player-map lookup —
/// and the list is every known node on it, sorted by id for a deterministic display order. The
/// flight master's own node types `Current`; every other known node routes from it over
/// [`shortest_route`] (the geo-distance metric) — `Reachable` with its fare/route-hop segments if
/// a path exists, **absent otherwise**: the ref's `DISTANT` classification is a dead branch
/// (byte-verified, 0496 §TU-3 — 1.12 never shows a yellow icon), so an unroutable node simply
/// doesn't render, exactly like an unknown one. Positions project through [`project`]. Returns
/// the continent's map id (the art index) + the paired engine snapshot nodes + the app-private
/// [`ResolvedTaxiNode`] cache, same order; `None` when the nearest node or its continent row is
/// missing from the catalogs (no map to draw).
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
            continue; // unroutable = invisible (the dead DISTANT branch — 0496 §TU-3)
        };
        // Each hop's segment in the same normalized space (`GetNumRoutes`/`TaxiGetSrc/DestX/Y`).
        // An intermediate node the chain passes through that isn't on THIS continent (a cross-map
        // transport hop — rare) still projects through the SAME rect as everything else on this
        // map: its segment may draw off the visible art, a cosmetic gap only — no in-flight route
        // overlay is in scope (decision 0484's "What this does NOT claim").
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

/// The **message id** an `SMSG_ACTIVATETAXIREPLY` refusal displays, as its GlobalStrings key.
///
/// The reference's parser `FUN_005ed1e0` indexes a 13-entry table at `[0x85fedc + 4*code]` and
/// hands the result straight to `CGGameUI::DisplayError 0x496720`: `code == 0` closes the map,
/// `1..=12` toast, `code >= 13` does nothing at all. So this returns a key rather than a string,
/// and the catalog row behind it decides the text, the **surface** and the sound — which matters
/// here more than anywhere: **seven of the twelve are `kind = 1`, the YELLOW info line**, not the
/// red one they all used to take, and `ERR_TAXINOTENOUGHMONEY` carries error-speech line `0x36`.
///
/// `OK` and anything past the table return `None` — the reference's own `code >= 13` no-op, in
/// place of the "Taxi activation failed (n)." literal that used to stand here and that the
/// reference never shows.
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

    /// The byte-verified route metric on a synthetic graph: the search minimizes
    /// summed GEOGRAPHIC distance — a geographically shorter 2-hop detour beats a longer direct
    /// hop even though its FARE is higher — and the fare is carried along the chosen chain, not
    /// optimized. An exact-tie breaks toward fewer hops (the determinism guard), and a node with
    /// no known-restricted path — disconnected, or reachable only through an undiscovered node —
    /// has no route at all (the caller drops it: the dead DISTANT branch).
    #[test]
    fn shortest_route_minimizes_geo_distance_and_carries_fare() {
        // Positions on a line: 1 at 0, 2 at 40, 4 at 100 — but the DIRECT 1→4 edge detours
        // geographically (dist 150), while 1→2→4 sums 40+60 = 100. Fares invert: direct 10,
        // detour 5+20 = 25 — the geo metric must pick the detour and REPORT the pricier fare.
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

        // Node 5 has no edge from the known graph at all — unreachable.
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

        // A real edge to node 3 exists, but 3 is NOT in the known mask — restricted out.
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

    /// The projection's cross-axis denominators (`u ÷ X-span, v ÷ Y-span`) on a
    /// deliberately NON-square rect, where the byte formula and the naive own-extent form
    /// diverge: X-span 100, Y-span 50, point at the rect's Y-max/X-mid.
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
        // worldX = 50 (mid X), worldY = 0 (Ymin): u = (50-0)/100 = 0.5 (÷ X-span!), v = (50-0)/50
        // = 1.0 (÷ Y-span!). The naive form would give u = 1.0, v = 0.5.
        let (u, v) = project(&cont, 50.0, 0.0);
        assert!((u - 0.5).abs() < 1e-6, "u divides by the X-span (got {u})");
        assert!((v - 1.0).abs() < 1e-6, "v divides by the Y-span (got {v})");
    }

    /// I1 verified against real 5875 data: every known-map node (except id 3, "Programmer Isle" —
    /// a debug row confirmed disconnected from `TaxiPath.dbc`, never known by a real player)
    /// projects into [0.02, 0.98] on both axes for maps 0/1, and the relative geography holds:
    /// Ironforge (6) sits ABOVE Stormwind (2) — Dun Morogh is north of Elwynn — and Menethil
    /// Harbor (7) sits WEST of Lakeshire (5) — Wetlands is west of Redridge. Skips without client
    /// data (the `taxi_nodes.rs`/`taxi_path.rs` test style).
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
                    continue; // Programmer Isle — see the doc comment above
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

    /// [`build_nodes`] end-to-end on the real, byte-verified Stormwind(2)->Sentinel Hill(4) hop
    /// (`TaxiPath` id 6, cost 110 copper — pinned by `taxi_path.rs`'s own test): the continent
    /// comes from the NEAREST node's own row (the packet-cached continentId, map 0 here),
    /// Stormwind classifies `Current`, Sentinel Hill `Reachable` with the exact fare and a
    /// one-hop route segment. And the dead-DISTANT law: with every node marked
    /// known, the cross-faction EK nodes (no `TaxiPath` route from Stormwind exists at all) are
    /// simply ABSENT from the list — known-but-unroutable never renders. Skips without client
    /// data.
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

        // Dead DISTANT: mark EVERY node known — the EK list must still omit at least one node
        // (the Horde-only stops, e.g. Grom'gol/Kargath, have no TaxiPath route from Stormwind),
        // and must contain no Distant entries at all.
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
