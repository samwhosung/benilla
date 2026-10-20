//! The taxi map's data: the DBC catalogs, and the reference's map builder (`0x4db710`), route
//! search (`0x4dc010`, `0x4dbce0`), fares (`0x4dc3d0`) and route lines (`0x4dc5c0`) over them.

use std::cmp::Ordering;
use std::collections::HashMap;

use benilla_formats::{
    load_taxi_nodes, load_taxi_path_nodes, load_taxi_paths, load_world_map_continent_catalog,
    FactionCatalog, TaxiNode, TaxiNodes, TaxiPathNode, TaxiPathNodes, TaxiPaths, WorldMapContinent,
    WorldMapContinentCatalog,
};
use benilla_protocol::messages::taxi_reply;
use bevy::prelude::*;

use benilla_ui::script::{TaxiNodeType, TaxiUiNode};

use benilla_assets::{LockRecover, WorldAssets};

use super::TaxiOpen;

/// `TaxiNodes.dbc`, `TaxiPath.dbc` (the direct hops and their fares), each hop's length over its
/// `TaxiPathNode.dbc` points, and `WorldMapContinent.dbc` (each continent's taxi-map rect, its
/// `taxi_min`/`taxi_max`).
#[derive(Resource)]
pub(crate) struct TaxiCatalogs {
    nodes: TaxiNodes,
    pub(super) paths: TaxiPaths,
    /// Each path's length over its points ([`path_length`]), by `TaxiPath` id.
    lengths: HashMap<u32, f32>,
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

/// The fare sum and node count of a slot no route reached (`0x4db9d5`, `0x4db9db`).
const UNROUTED: u32 = 999_999;

/// The length of a path with no points (`0x4dbc5d`, the float at `0x807668`).
const NO_POINTS: f32 = 99_999_997_952.0;

impl TaxiCatalogs {
    /// The four tables off the patch chain.
    pub(super) fn load(chain: &mut benilla_formats::Chain) -> anyhow::Result<Self> {
        Ok(Self::from_parts(
            load_taxi_nodes(chain)?,
            load_taxi_paths(chain)?,
            &load_taxi_path_nodes(chain)?,
            load_world_map_continent_catalog(chain)?,
        ))
    }

    /// The catalogs from their tables, `TaxiPathNode.dbc`'s points summed once.
    fn from_parts(
        nodes: TaxiNodes,
        paths: TaxiPaths,
        points: &TaxiPathNodes,
        continents: WorldMapContinentCatalog,
    ) -> Self {
        Self {
            nodes,
            paths,
            lengths: points
                .paths()
                .map(|(id, points)| (id, path_length(points)))
                .collect(),
            continents,
        }
    }

    /// `0x4dbbd0(to, from)`: the length of the path from `to` back to `from` (`0x4dbbf2` reads
    /// `to * 256 + from`), [`NO_POINTS`] when it has no points. The reference reads that row with
    /// no null test (`0x4dbc02`) and faults on a pair with only the forward row; here such a pair
    /// weighs as a path with no points.
    fn length_back(&self, to: u32, from: u32) -> f32 {
        self.paths
            .between(to, from)
            .and_then(|path| self.lengths.get(&path.id))
            .copied()
            .unwrap_or(NO_POINTS)
    }
}

/// A path's length (`0x4dbc68`-`0x4dbccd`): the straight 3-D distance between consecutive points,
/// added to an f32 sum. The x87 runs at 53 bits, so f64 is its arithmetic: x's difference is
/// stored as an f32 (`0x4dbc97`), y's and z's are not, and the squares add z, x, then y. The
/// reference walks the points in file order; the shipped file keeps each path's points
/// consecutive and in `NodeIndex` order, the order [`TaxiPathNodes`] gives.
fn path_length(points: &[TaxiPathNode]) -> f32 {
    let mut sum = 0.0f32;
    for pair in points.windows(2) {
        let (from, to) = (pair[0].pos, pair[1].pos);
        let dx = f64::from(to[0] - from[0]);
        let dy = f64::from(to[1]) - f64::from(from[1]);
        let dz = f64::from(to[2]) - f64::from(from[2]);
        let step = (dz * dz + dx * dx + dy * dy).sqrt();
        sum = (step + f64::from(sum)) as f32;
    }
    sum
}

/// A node's map position (`0x4db958`-`0x4db995`) on the taxi map's 0..1 space, with the rect from
/// `WorldMapContinent.dbc` fields 9-12 of the current node's continent, in f64 and stored as f32:
///
/// ```text
/// u = (Ymax − worldY) / (Xmax − Xmin)    ; world Y (west+) → horizontal, inverted
/// v = (worldX − Xmin) / (Ymax − Ymin)    ; world X (north+) → vertical, from BOTTOMLEFT
/// ```
///
/// The denominators are cross-axis; the route lines' [`project_line`] uses same-axis ones.
pub(crate) fn project(cont: &WorldMapContinent, world_x: f32, world_y: f32) -> (f32, f32) {
    let (min_x, min_y) = cont.taxi_min;
    let (max_x, max_y) = cont.taxi_max;
    let u = (f64::from(max_y) - f64::from(world_y)) / (f64::from(max_x) - f64::from(min_x));
    let v = (f64::from(world_x) - f64::from(min_x)) / (f64::from(max_y) - f64::from(min_y));
    (u as f32, v as f32)
}

/// A route line's end (`0x4dc890`): [`project`]'s numerators, each over its own axis's span.
fn project_line(cont: &WorldMapContinent, world_x: f32, world_y: f32) -> (f32, f32) {
    let (min_x, min_y) = cont.taxi_min;
    let (max_x, max_y) = cont.taxi_max;
    let u = (f64::from(max_y) - f64::from(world_y)) / (f64::from(max_y) - f64::from(min_y));
    let v = (f64::from(world_x) - f64::from(min_x)) / (f64::from(max_x) - f64::from(min_x));
    (u as f32, v as f32)
}

/// The route line from node `a` to node `b`, `[src_u, src_v, dest_u, dest_v]`.
fn line(cont: &WorldMapContinent, a: &TaxiNode, b: &TaxiNode) -> [f32; 4] {
    let (au, av) = project_line(cont, a.pos[0], a.pos[1]);
    let (bu, bv) = project_line(cont, b.pos[0], b.pos[1]);
    [au, av, bu, bv]
}

/// A slot's route once the route flag `+0x0d` is set: the chain from the current node (`+0x14`,
/// `+0x18`), its fare sum (`+0x10`) and its length (`+0x1c`).
#[derive(Clone)]
struct Route {
    chain: Vec<u32>,
    fare: u32,
    length: f32,
}

/// The route search over the slots: each slot's direct paths to other slots, in slot order, as
/// `(slot, price, length back)`, and whether the faction filter skips each ([`skips`]).
struct Search<'a> {
    ids: &'a [u32],
    hops: Vec<Vec<(usize, u32, f32)>>,
    skipped: Vec<bool>,
}

impl Search<'_> {
    /// `0x4dbce0(to, from, price)`: nothing for a slot the faction filter skips (`0x4dbffc`), else
    /// `from`'s route plus the hop, kept when `to` has no route yet (`0x4dbe17`) or the total is
    /// strictly shorter (`0x4dbe2b`), then every slot `to` has a path to relaxed in slot order,
    /// depth first. The total is the f32 sum of the hop's length back and `from`'s (`0x4dbe06`,
    /// `0x4dbe14`); fares and hop counts never decide.
    fn relax(&self, routes: &mut [Option<Route>], to: usize, from: usize, price: u32, back: f32) {
        if self.skipped[to] {
            return;
        }
        let Some(prev) = &routes[from] else {
            return; // never: the seed and every relaxation start from a routed slot
        };
        let length = (f64::from(back) + f64::from(prev.length)) as f32;
        if let Some(held) = &routes[to] {
            if length.partial_cmp(&held.length) != Some(Ordering::Less) {
                return;
            }
        }
        let mut chain = Vec::with_capacity(prev.chain.len() + 1);
        chain.extend_from_slice(&prev.chain);
        chain.push(self.ids[to]);
        let fare = prev.fare.wrapping_add(price);
        routes[to] = Some(Route {
            chain,
            fare,
            length,
        });
        for &(next, price, back) in &self.hops[to] {
            self.relax(routes, next, to, price, back);
        }
    }
}

/// The seed `0x4dc010`: the current node's slot, slot 0 when none matches (`0x4dc039`), takes the
/// route `[current]` at fare and length 0, and every slot the current node has a path to is
/// relaxed from it in slot order. A slot left `None` was never reached.
fn search(cat: &TaxiCatalogs, ids: &[u32], current: u32, skipped: Vec<bool>) -> Vec<Option<Route>> {
    let mut routes = vec![None; ids.len()];
    if ids.is_empty() {
        return routes;
    }
    let hops_from = |from: u32, from_slot: usize| -> Vec<(usize, u32, f32)> {
        ids.iter()
            .enumerate()
            .filter_map(|(slot, &to)| {
                let price = cat.paths.between(from, to)?.cost;
                Some((slot, price, cat.length_back(to, ids[from_slot])))
            })
            .collect()
    };
    let search = Search {
        ids,
        hops: (0..ids.len())
            .map(|slot| hops_from(ids[slot], slot))
            .collect(),
        skipped,
    };
    let seed = ids.iter().position(|&id| id == current).unwrap_or(0);
    routes[seed] = Some(Route {
        chain: vec![current],
        fare: 0,
        length: 0.0,
    });
    for (slot, price, back) in hops_from(current, seed) {
        search.relax(&mut routes, slot, seed, price, back);
    }
    routes
}

/// The faction filter (`0x4dbd59`-`0x4dbdeb`) on a node's two mount ids (`+0x38`, `+0x3c`), for
/// the player's group: group 3 skips a node naming a Horde mount (`0x8b0`, `0xdf6`) and no
/// Alliance one (`0xefd`, `0x21d`), group 5 the reverse, and any other group, or none, skips
/// nothing.
fn skips(group: Option<u32>, node: &TaxiNode) -> bool {
    const HORDE: [u32; 2] = [2224, 3574];
    const ALLIANCE: [u32; 2] = [3837, 541];
    let names =
        |side: [u32; 2]| side.contains(&node.mount_horde) || side.contains(&node.mount_alliance);
    match group {
        Some(3) => names(HORDE) && !names(ALLIANCE),
        Some(5) => names(ALLIANCE) && !names(HORDE),
        _ => false,
    }
}

/// `TaxiNodeCost`'s fare (`0x4dc3d0`): the price times `1 - discount`, both in f64 with nothing
/// stored (`fsubr`, `fmul st,st(1)`), the product stored as an f32 and rounded half to even
/// (`fstp dword`, `fistp dword`).
fn shown_fare(price: u32, discount: f64) -> u32 {
    let product = (f64::from(price) * (1.0 - discount)) as f32;
    f64::from(product).round_ties_even() as u32
}

/// A listed node as `TakeTaxiNode` (`0x4dc8d0`) reads its slot: the node, and its route from the
/// current node (`+0x14`, `+0x18`), empty when never routed, with the route's raw fare sum
/// (`+0x10`).
pub(super) struct ResolvedTaxiNode {
    pub(super) node: u32,
    pub(super) chain: Vec<u32>,
    pub(super) cost: u32,
}

/// The pushed node list's slots, index for index, so `TakeTaxiNode(i)` finds its own, and the
/// current node (`[0xbb4a80]`).
#[derive(Resource, Default)]
pub(super) struct TaxiRouteCache {
    pub(super) current: u32,
    pub(super) slots: Vec<ResolvedTaxiNode>,
}

/// The listed nodes, as the builder `0x4db710` makes its slots: every known id from 1 to 256
/// whose `TaxiNodes` row lies on the current node's continent and inside its taxi rect, ends
/// included, in id order (`0x4db8a0`-`0x4dba17`); no slot, no row for the current node or its
/// continent, or a rect of zero span builds no map. The route search ([`search`]) runs over the
/// slots alone. The current node is `Current`, a routed one `Reachable` and one the search never
/// reached `Unrouted`, `NONE`, which stock `TaxiFrame.lua` hides; `DISTANT` is never produced.
/// Returns the continent's map id, the Lua nodes and their routes, in one order.
///
/// A node with a direct path from the current node shows that path's price, draws its one
/// segment once routed and counts one route (`0x4dc44f`, `0x4dc643`, `0x4dc185`), whatever the
/// search routed; any other shows its route's fare sum and one segment per hop, or 999999 and
/// none unrouted. A Lua node's fare takes the flight master's price `discount`
/// ([`shown_fare`]), and is 0 with none, when the flight master or the player does not resolve
/// (`0x4dc3fa`, `0x4dc423`); the route's fare stays the raw sum, which
/// `CMSG_ACTIVATETAXIEXPRESS` carries (`0x4dc96f`).
///
/// For the active player, its race and the faction tables, the search skips the other side's
/// nodes by the group mask (`+0xc`) of the `FactionTemplate` row whose id is the race
/// (`0x4dbd2d`-`0x4dbd56`), not the template `ChrRaces` gives the race, so races 7 and 8 skip
/// nothing. No player or no such row skips nothing (`0x4dbd21`, `0x4dbd50`).
pub(super) fn build_nodes_for(
    open: &TaxiOpen,
    cat: &TaxiCatalogs,
    discount: Option<f64>,
    player: Option<(u8, &FactionCatalog)>,
) -> Option<(u32, Vec<TaxiUiNode>, Vec<ResolvedTaxiNode>)> {
    let current = cat.nodes.get(open.nearest_node)?;
    let cont = cat.continents.get(current.map_id)?;
    let ((min_x, min_y), (max_x, max_y)) = (cont.taxi_min, cont.taxi_max);
    if max_x - min_x == 0.0 || max_y - min_y == 0.0 {
        return None; // `0x4db853`, `0x4db870`
    }
    // The x87 compares pass a NaN coordinate (`0x4db906`-`0x4db94d`).
    let inside = |n: &TaxiNode| {
        let [x, y, _] = n.pos;
        !(x > max_x || x < min_x || y > max_y || y < min_y)
    };
    let slots: Vec<&TaxiNode> = (1..=256)
        .filter(|&id| open.known.is_known(id))
        .filter_map(|id| cat.nodes.get(id))
        .filter(|n| n.map_id == current.map_id && inside(n))
        .collect();
    if slots.is_empty() {
        return None; // `0x4dba40`
    }
    let ids: Vec<u32> = slots.iter().map(|n| n.id).collect();
    let group = player
        .and_then(|(race, factions)| factions.template(u32::from(race)))
        .map(|template| template.group_mask);
    let skipped = slots.iter().map(|n| skips(group, n)).collect();
    let routes = search(cat, &ids, current.id, skipped);

    let mut ui = Vec::with_capacity(slots.len());
    let mut resolved = Vec::with_capacity(slots.len());
    for (n, route) in slots.into_iter().zip(routes) {
        // The no-path byte `+0x0c` (`0x4db99f`).
        let direct = cat.paths.between(current.id, n.id);
        // `0x4dc504` tests the current node before the route flag (`0x4dc51a`).
        let node_type = if n.id == current.id {
            TaxiNodeType::Current
        } else if route.is_some() {
            TaxiNodeType::Reachable
        } else {
            TaxiNodeType::Unrouted
        };
        // The fare sum and node count an unrouted slot keeps from the builder.
        let (fare, count) = route
            .as_ref()
            .map_or((UNROUTED, UNROUTED), |r| (r.fare, r.chain.len() as u32));
        let (price, num_routes) = match direct {
            Some(path) => (path.cost, 1),
            None => (fare, count.wrapping_sub(1)),
        };
        // `0x4dc5c0`: nothing unrouted, the direct segment, else one per hop.
        let routes = match (&route, direct) {
            (None, _) => Vec::new(),
            (Some(_), Some(_)) => vec![line(cont, current, n)],
            (Some(route), None) => route
                .chain
                .windows(2)
                .filter_map(|hop| Some(line(cont, cat.nodes.get(hop[0])?, cat.nodes.get(hop[1])?)))
                .collect(),
        };
        ui.push(TaxiUiNode {
            name: n.name.clone(),
            node_type,
            pos: project(cont, n.pos[0], n.pos[1]),
            cost: discount.map_or(0, |d| shown_fare(price, d)),
            routes,
            num_routes,
        });
        resolved.push(ResolvedTaxiNode {
            node: n.id,
            chain: route.map_or_else(Vec::new, |route| route.chain),
            cost: fare,
        });
    }
    Some((current.map_id, ui, resolved))
}

/// [`build_nodes_for`] with no player, which skips nothing.
#[cfg(test)]
pub(super) fn build_nodes(
    open: &TaxiOpen,
    cat: &TaxiCatalogs,
    discount: Option<f64>,
) -> Option<(u32, Vec<TaxiUiNode>, Vec<ResolvedTaxiNode>)> {
    build_nodes_for(open, cat, discount, None)
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
