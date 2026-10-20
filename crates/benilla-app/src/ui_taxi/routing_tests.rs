//! The taxi map builder: the node list, the route search, the fares and the route lines.

use benilla_formats::{TaxiPath, TaxiPathNode};
use benilla_protocol::messages::TaxiMask;
use benilla_ui::script::{TaxiUiState, UiScript};

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

/// `0x4dc3d0`'s rounding: `1 - discount` stays f64 and only the product is stored as f32, so a
/// near-tie becomes an exact `.5` and rounds to even where the f64 shortcut rounds down.
#[test]
fn the_shown_fare_stores_only_the_product_as_f32() {
    let (d05, d10) = (f64::from(0.05f32), f64::from(0.1f32));
    let d15 = d10 + d05;
    // 0.1: 15 × 0.89999999850988388 = 13.4999999776, stored 13.5, to even 14; the f64
    // shortcut gives 13.
    assert_eq!(shown_fare(15, d10), 14);
    // 0.15: 30 × 0.84999999776482582 = 25.4999999329, stored 25.5, to even 26; and 110
    // (Stormwind to Sentinel Hill) = 93.4999997541, stored 93.5, to even 94.
    assert_eq!(shown_fare(30, d15), 26);
    assert_eq!(shown_fare(110, d15), 94);
    // 0.15 at 786437 stores 668471.4375, so 668471; repair's f32 multiplier gives 668472.
    assert_eq!(shown_fare(786_437, d15), 668_471);
    // 0.15 at 2097157 stores 1782583.5, to even 1782584; the discount narrowed to f32 first
    // (0.15000000596) would store 1782583.375, so 1782583.
    assert_eq!(shown_fare(2_097_157, d15), 1_782_584);
    // A plain case: 0.2 at 110 is 88, and no discount is the price.
    assert_eq!(shown_fare(110, d15 + d05), 88);
    assert_eq!(shown_fare(110, 0.0), 110);
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
/// node known, every Eastern Kingdoms row inside the rect is listed, Programmer Isle (3) outside
/// it is not, and the Horde stops Stormwind never reaches are `NONE`.
#[test]
fn build_nodes_classifies_a_real_known_hop() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
    let open = TaxiOpen {
        flightmaster: 0x42,
        nearest_node: 2,
        known: mask_of(&[2, 4]),
    };
    let (map_id, ui, resolved) = build_nodes(&open, &cat, Some(0.0)).expect("map builds");
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

    // Every node known.
    let open = TaxiOpen {
        flightmaster: 0x42,
        nearest_node: 2,
        known: TaxiMask([u32::MAX; 8]),
    };
    let (_, ui, _) = build_nodes(&open, &cat, Some(0.0)).expect("map builds");
    let inside_rect = cat
        .nodes
        .rows()
        .filter(|n| n.map_id == 0 && n.id != 3)
        .count();
    assert_eq!(ui.len(), inside_rect);
    assert!(!ui.iter().any(|n| n.name == "Programmer Isle"));
    for name in ["Grom'gol, Stranglethorn", "Kargath, Badlands"] {
        assert_eq!(ui[index_of(&ui, name)].node_type, TaxiNodeType::Unrouted);
    }
    assert!(
        ui.iter().all(|n| n.node_type != TaxiNodeType::Distant),
        "the DISTANT classification is a dead branch — never produced"
    );
}

// --- A synthetic map: nodes on continent 1, hops flown over chosen points ---

/// A `TaxiNodes` row on continent 1 at world `(x, y)`, its two mount ids at `+0x38` and `+0x3c`.
fn row(id: u32, x: f32, y: f32, mounts: [u32; 2]) -> TaxiNode {
    TaxiNode {
        id,
        map_id: 1,
        pos: [x, y, 0.0],
        name: format!("node {id}"),
        mount_horde: mounts[0],
        mount_alliance: mounts[1],
    }
}

/// A continent-1 rect from `min` to `max`, `(x, y)` each.
fn rect(min: (f32, f32), max: (f32, f32)) -> WorldMapContinent {
    WorldMapContinent {
        map_id: 1,
        left_boundary: 0,
        right_boundary: 0,
        top_boundary: 0,
        bottom_boundary: 0,
        offset_x: 0.0,
        offset_y: 0.0,
        scale: 1.0,
        taxi_min: min,
        taxi_max: max,
    }
}

/// One `TaxiPath` row `from → to` at `cost`, id `from * 100 + to`, flown over `points` `(x, y)`.
type Hop<'a> = (u32, u32, u32, &'a [(f32, f32)]);

/// The catalogs of `rows` and `hops` on `rect`.
fn synthetic(rows: Vec<TaxiNode>, hops: &[Hop], cont: WorldMapContinent) -> TaxiCatalogs {
    let paths = hops.iter().map(|&(from, to, cost, _)| TaxiPath {
        id: from * 100 + to,
        from,
        to,
        cost,
    });
    let mut id = 0;
    let points = hops.iter().flat_map(|&(from, to, _, pts)| {
        pts.iter()
            .enumerate()
            .map(move |(i, &(x, y))| (from * 100 + to, i, x, y))
    });
    let points = points.map(|(path_id, i, x, y)| {
        id += 1;
        TaxiPathNode {
            id,
            path_id,
            node_index: i as u32,
            map_id: 1,
            pos: [x, y, 0.0],
            flags: 0,
            delay: 0,
        }
    });
    TaxiCatalogs::from_parts(
        TaxiNodes::from_rows(rows),
        TaxiPaths::from_rows(paths),
        &TaxiPathNodes::from_rows(points.collect::<Vec<_>>()),
        WorldMapContinentCatalog::from_rows([cont]),
    )
}

/// The map open at `current` knowing `known`.
fn open_at(current: u32, known: &[u32]) -> TaxiOpen {
    TaxiOpen {
        flightmaster: 0x42,
        nearest_node: current,
        known: mask_of(known),
    }
}

/// The built list pushed into a VM, as the feed pushes it.
fn vm_of(ui: Vec<TaxiUiNode>) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_taxi(Some(TaxiUiState {
        art: String::new(),
        nodes: ui,
    }));
    s
}

/// The list index of the node named `name`.
fn index_of(ui: &[TaxiUiNode], name: &str) -> usize {
    ui.iter()
        .position(|n| n.name == name)
        .unwrap_or_else(|| panic!("{name} listed"))
}

/// `0x4dbbd0` over the shipped path from Thunder Bluff to Gadgetzan (`TaxiPath` 309): x's
/// difference stored as an f32, y's, z's, the squares and the root at 53 bits, the sum stored as
/// an f32 at each point: 8692.9052734375 exactly.
#[test]
fn a_shipped_path_s_length_sums_its_points_as_the_x87_does() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
    assert_eq!(cat.paths.between(22, 40).map(|p| p.id), Some(309));
    assert_eq!(cat.length_back(22, 40), 8_692.905);
}

/// A path of one point is 0 long; one with no points, and a pair with only the forward row, weigh
/// the no-points length 99999997952 (`0x4dbc5d`).
#[test]
fn a_path_without_a_run_weighs_the_no_points_length() {
    let cat = synthetic(
        vec![
            row(1, 0.0, 0.0, [0, 0]),
            row(2, 100.0, 0.0, [0, 0]),
            row(3, 0.0, 100.0, [0, 0]),
        ],
        &[
            (1, 2, 1, &[(0.0, 0.0)]),
            (2, 1, 1, &[]),
            (1, 3, 1, &[(0.0, 0.0), (0.0, 100.0)]),
        ],
        rect((-1000.0, -1000.0), (1000.0, 1000.0)),
    );
    assert_eq!(cat.length_back(1, 2), 0.0);
    assert_eq!(cat.length_back(2, 1), 99_999_997_952.0);
    assert_eq!(cat.length_back(3, 1), 99_999_997_952.0, "no 3 → 1 row");
    assert_eq!(cat.length_back(1, 3), 100.0);
}

/// A hop `a → b` is weighed by the points of the path from `b` back to `a` (`0x4dbe01` passes the
/// reached slot first, `0x4dbbf2` reads `to * 256 + from`), summed point to point (`0x4dbc68` to
/// `0x4dbccd`), and the shorter total wins. Here 1 → 2 → 3 is short forward and by straight line,
/// but 3 → 2 runs a 1624-long detour; 1 → 4 → 3 sums 283 + 300 from 4 → 1 and 3 → 4.
#[test]
fn a_hop_weighs_the_points_of_the_path_back() {
    let cat = synthetic(
        vec![
            row(1, 0.0, 0.0, [0, 0]),
            row(2, 100.0, 0.0, [0, 0]),
            row(3, 100.0, 200.0, [0, 0]),
            row(4, -200.0, 200.0, [0, 0]),
        ],
        &[
            (1, 2, 10, &[(0.0, 0.0), (100.0, 0.0)]),
            (2, 1, 10, &[(100.0, 0.0), (0.0, 0.0)]),
            (2, 3, 20, &[(100.0, 0.0), (100.0, 200.0)]),
            (3, 2, 20, &[(100.0, 200.0), (900.0, 200.0), (100.0, 0.0)]),
            (1, 4, 40, &[(0.0, 0.0), (-200.0, 200.0)]),
            (4, 1, 40, &[(-200.0, 200.0), (0.0, 0.0)]),
            (
                4,
                3,
                80,
                &[
                    (-200.0, 200.0),
                    (-200.0, 900.0),
                    (100.0, 900.0),
                    (100.0, 200.0),
                ],
            ),
            (3, 4, 80, &[(100.0, 200.0), (-200.0, 200.0)]),
        ],
        rect((-1000.0, -1000.0), (1000.0, 1000.0)),
    );
    let (_, ui, resolved) =
        build_nodes(&open_at(1, &[1, 2, 3, 4]), &cat, Some(0.0)).expect("map builds");
    let three = index_of(&ui, "node 3");
    assert_eq!(resolved[three].chain, vec![1, 4, 3]);
    assert_eq!(
        resolved[three].cost, 120,
        "the fare rides along the chosen chain"
    );
    assert_eq!(
        ui[three].cost, 120,
        "no direct path: the shown fare is the chain's"
    );
    assert_eq!(ui[three].routes.len(), 2, "one line per hop");
}

/// Relaxation is depth first in slot order and keeps a route unless a later one is strictly
/// shorter (`0x4dbe2b`): of two equal 300-long chains the first found stays, though it has more
/// hops. Slot 2 is relaxed first, so 1 → 2 → 3 → 5 reaches 5 before 1 → 4 → 5.
#[test]
fn an_equal_route_found_later_does_not_replace_the_first() {
    let line = |a: f32, b: f32| [(a, 0.0), (b, 0.0)];
    let (l12, l21, l23, l32, l35, l53) = (
        line(0.0, 100.0),
        line(100.0, 0.0),
        line(100.0, 200.0),
        line(200.0, 100.0),
        line(200.0, 300.0),
        line(300.0, 200.0),
    );
    let (l14, l41, l45, l54) = (
        line(0.0, 150.0),
        line(150.0, 0.0),
        line(150.0, 300.0),
        line(300.0, 150.0),
    );
    let cat = synthetic(
        vec![
            row(1, 0.0, 0.0, [0, 0]),
            row(2, 100.0, 0.0, [0, 0]),
            row(3, 200.0, 0.0, [0, 0]),
            row(4, 150.0, 0.0, [0, 0]),
            row(5, 300.0, 0.0, [0, 0]),
        ],
        &[
            (1, 2, 1, &l12),
            (2, 1, 1, &l21),
            (2, 3, 1, &l23),
            (3, 2, 1, &l32),
            (3, 5, 1, &l35),
            (5, 3, 1, &l53),
            (1, 4, 5, &l14),
            (4, 1, 5, &l41),
            (4, 5, 5, &l45),
            (5, 4, 5, &l54),
        ],
        rect((-1000.0, -1000.0), (1000.0, 1000.0)),
    );
    let (_, ui, resolved) =
        build_nodes(&open_at(1, &[1, 2, 3, 4, 5]), &cat, Some(0.0)).expect("map builds");
    let five = index_of(&ui, "node 5");
    assert_eq!(resolved[five].chain, vec![1, 2, 3, 5]);
    assert_eq!(ui[five].cost, 3);
}

/// A destination with a direct path shows that path's price (`0x4dc44f` to `0x4dc488`), draws the
/// one segment from the current node (`0x4dc643`) and counts one route (`0x4dc170`), though the
/// search routes it 1 → 2 → 3, whose 241 beats the direct path's 1900-long return.
#[test]
fn a_direct_destination_keeps_its_own_price_and_line() {
    let cat = synthetic(
        vec![
            row(1, 0.0, 0.0, [0, 0]),
            row(2, 0.0, 100.0, [0, 0]),
            row(3, 100.0, 0.0, [0, 0]),
        ],
        &[
            (1, 3, 50, &[(0.0, 0.0), (100.0, 0.0)]),
            (
                3,
                1,
                50,
                &[(100.0, 0.0), (100.0, 900.0), (0.0, 900.0), (0.0, 0.0)],
            ),
            (1, 2, 10, &[(0.0, 0.0), (0.0, 100.0)]),
            (2, 1, 10, &[(0.0, 100.0), (0.0, 0.0)]),
            (2, 3, 10, &[(0.0, 100.0), (100.0, 0.0)]),
            (3, 2, 10, &[(100.0, 0.0), (0.0, 100.0)]),
        ],
        rect((-1000.0, -1000.0), (1000.0, 1000.0)),
    );
    let (_, ui, resolved) =
        build_nodes(&open_at(1, &[1, 2, 3]), &cat, Some(0.0)).expect("map builds");
    let three = index_of(&ui, "node 3");
    assert_eq!(
        resolved[three].chain,
        vec![1, 2, 3],
        "the search's own route"
    );
    assert_eq!(ui[three].cost, 50, "the direct path's price");
    let one = index_of(&ui, "node 1");
    let [[sx, sy, dx, dy]] = ui[three].routes[..] else {
        panic!("one segment: {:?}", ui[three].routes);
    };
    assert_eq!(((sx, sy), (dx, dy)), (ui[one].pos, ui[three].pos));
    let s = vm_of(ui);
    assert_eq!(
        s.eval::<i64>(&format!("return GetNumRoutes({})", three + 1))
            .unwrap(),
        1
    );
}

/// The route lines project with each axis over its own span (`0x4dc890`); the node positions
/// divide across (`0x4db978`, `0x4db995`). On a rect 100 wide in x and 50 in y they differ.
#[test]
fn route_lines_project_over_each_axis_own_span() {
    let cat = synthetic(
        vec![row(1, 10.0, 20.0, [0, 0]), row(2, 30.0, 40.0, [0, 0])],
        &[
            (1, 2, 10, &[(10.0, 20.0), (30.0, 40.0)]),
            (2, 1, 10, &[(30.0, 40.0), (10.0, 20.0)]),
        ],
        rect((0.0, 0.0), (100.0, 50.0)),
    );
    let (_, ui, _) = build_nodes(&open_at(1, &[1, 2]), &cat, Some(0.0)).expect("map builds");
    let two = index_of(&ui, "node 2");
    // u = (50 - y) / 50, v = (x - 0) / 100.
    assert_eq!(ui[two].routes, vec![[0.6, 0.1, 0.2, 0.3]]);
    // The node itself: u = (50 - 40) / 100, v = 30 / 50.
    assert_eq!(ui[two].pos, (0.1, 0.6));
}

/// The faction filter (`0x4dbd59` to `0x4dbdeb`) on the node being reached, by the group mask of
/// the `FactionTemplate` row whose id is the race (`0x4dbd2d` to `0x4dbd56`): group 3 skips a node
/// naming 2224 or 3574 and neither 3837 nor 541, group 5 the reverse, in either mount field; any
/// other group, a race with no row and no player skip nothing. A skipped node gets no route
/// (`NONE`), a node behind it only another road, and a direct one keeps its count of 1, its
/// direct price, no line and its direct activate.
#[test]
fn the_race_group_skips_the_other_side_s_nodes() {
    use benilla_formats::FactionTemplate;
    let straight = |a: (f32, f32), b: (f32, f32)| [a, b];
    let at = |id: u32| match id {
        1 => (0.0, 0.0),
        10 => (500.0, 500.0),
        11 => (-500.0, 500.0),
        n => (n as f32 * 10.0, 0.0),
    };
    let mounts: [(u32, [u32; 2]); 9] = [
        (1, [2224, 541]), // the current node, both sides
        (2, [2224, 0]),
        (3, [3574, 0]),
        (4, [0, 541]),
        (5, [0, 3837]),
        (6, [2224, 3837]),
        (7, [0, 0]),
        (8, [541, 0]),
        (9, [0, 3574]),
    ];
    let mut rows: Vec<TaxiNode> = mounts
        .iter()
        .map(|&(id, m)| row(id, at(id).0, at(id).1, m))
        .collect();
    rows.push(row(10, 500.0, 500.0, [0, 0])); // only behind 2
    rows.push(row(11, -500.0, 500.0, [0, 0])); // behind 2, or longer behind 7
    let legs: Vec<(u32, u32, u32)> = (2..=9)
        .map(|n| (1, n, 10 * n))
        .chain([(2, 10, 7), (2, 11, 7), (7, 11, 9)])
        .collect();
    let mut owned = Vec::new();
    for &(a, b, cost) in &legs {
        owned.push((a, b, cost, straight(at(a), at(b))));
        owned.push((b, a, cost, straight(at(b), at(a))));
    }
    let hops: Vec<Hop> = owned
        .iter()
        .map(|(a, b, c, p)| (*a, *b, *c, &p[..]))
        .collect();
    let cat = synthetic(rows, &hops, rect((-1000.0, -1000.0), (1000.0, 1000.0)));
    let template = |group_mask| FactionTemplate {
        group_mask,
        ..Default::default()
    };
    let factions = FactionCatalog::from_rows(
        HashMap::from([(1, template(3)), (2, template(5)), (7, template(0))]),
        HashMap::new(),
    );
    let open = open_at(1, &(1..=11).collect::<Vec<_>>());
    let types = |player: Option<(u8, &FactionCatalog)>| -> Vec<String> {
        let (_, ui, _) = build_nodes_for(&open, &cat, Some(0.0), player).expect("map builds");
        let s = vm_of(ui);
        (1..=11)
            .map(|i| {
                s.eval::<String>(&format!("return TaxiNodeGetType({i})"))
                    .unwrap()
            })
            .collect()
    };
    let r = "REACHABLE";
    let n = "NONE";
    let c = "CURRENT";
    // Slots in id order, 1 to 11.
    assert_eq!(
        types(Some((1, &factions))),
        [c, n, n, r, r, r, r, r, n, n, r],
        "group 3"
    );
    assert_eq!(
        types(Some((2, &factions))),
        [c, r, r, n, n, r, r, n, r, r, r],
        "group 5"
    );
    let all = [c, r, r, r, r, r, r, r, r, r, r];
    assert_eq!(types(Some((7, &factions))), all, "group 0");
    assert_eq!(types(Some((8, &factions))), all, "a race with no row");
    assert_eq!(types(None), all, "no player");

    // Group 3: 11 goes the long road behind 7; 2, never routed, keeps its direct path's count and
    // price, with no line and no route.
    let (_, ui, resolved) =
        build_nodes_for(&open, &cat, Some(0.0), Some((1, &factions))).expect("map builds");
    assert_eq!(resolved[10].chain, vec![1, 7, 11]);
    assert_eq!(resolved[1].node, 2);
    assert!(resolved[1].chain.is_empty());
    assert!(ui[1].routes.is_empty());
    assert_eq!(ui[1].cost, 20);
    let s = vm_of(ui);
    assert_eq!(s.eval::<i64>("return GetNumRoutes(2)").unwrap(), 1);
    assert_eq!(s.eval::<f64>("return TaxiGetSrcX(2, 1)").unwrap(), 0.0);
}

/// A known node the search never reaches stays in the list typed `NONE` (`0x4db8a0` to
/// `0x4dba17`, `0x4dc512`), with its name and position; its fare is the start sum 999999
/// (`0x4db9d5`, `0x4dc495`) and its route count 999998 (`0x4dc18b`), and nothing is drawn or
/// sent. At Menethil Harbor knowing Stormwind but not Ironforge: no path joins the two.
#[test]
fn a_known_node_with_no_route_stays_listed_as_none() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
    let (_, ui, resolved) = build_nodes(&open_at(7, &[2, 7]), &cat, Some(0.0)).expect("map");
    assert_eq!(resolved[0].node, 2);
    assert!(resolved[0].chain.is_empty(), "no route");
    let pos = ui[0].pos;
    let s = vm_of(ui);
    let text = |lua: &str| s.eval::<String>(lua).unwrap();
    let number = |lua: &str| s.eval::<f64>(lua).unwrap();
    assert_eq!(number("return NumTaxiNodes()"), 2.0);
    assert_eq!(text("return TaxiNodeName(1)"), "Stormwind, Elwynn");
    assert_eq!(text("return TaxiNodeGetType(1)"), "NONE");
    assert_eq!(text("return TaxiNodeGetType(2)"), "CURRENT");
    assert_eq!(number("return TaxiNodeCost(1)"), 999_999.0);
    assert_eq!(number("return GetNumRoutes(1)"), 999_998.0);
    assert_eq!(number("return GetNumRoutes(2)"), 0.0);
    assert_eq!(number("return TaxiGetSrcX(1, 1)"), 0.0);
    let (x, y) = s.eval::<(f32, f32)>("return TaxiNodePosition(1)").unwrap();
    assert_eq!((x, y), pos);
}

/// The issue's replays over the shipped tables, each faction's flight points known: 1.12.1's
/// routes and fares, where straight lines fly Stormwind to Southshore through Ironforge alone for
/// 380, Nethergarde through Morgan's Vigil and Thorium Point for 2490, and the Sepulcher through
/// Undercity for 740.
#[test]
fn shipped_routes_follow_the_flight_paths_length() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
    let alliance = [2, 4, 5, 6, 7, 8, 12, 14, 16, 19, 43, 45, 66, 67, 71, 74];
    let horde = [10, 11, 13, 17, 18, 20, 21, 56, 68, 70, 75, 76];
    for (known, from, to, want, fare) in [
        (&alliance[..], 2, 14, vec![2, 6, 7, 14], 710),
        (&alliance[..], 45, 6, vec![45, 2, 6], 880),
        (&horde[..], 10, 21, vec![10, 13, 17, 21], 1490),
    ] {
        let (_, ui, resolved) = build_nodes(&open_at(from, known), &cat, Some(0.0)).expect("map");
        let i = resolved.iter().position(|r| r.node == to).expect("listed");
        assert_eq!(resolved[i].chain, want, "{from} to {to}");
        assert_eq!(
            (resolved[i].cost, ui[i].cost),
            (fare, fare),
            "{from} to {to}"
        );
    }
}

/// The map under #204's screenshot: a Horde character at Gadgetzan knowing the eight flight
/// points it shows. Sun Rock Retreat routes through Freewind Post and the Crossroads for 1070
/// (straight lines took Thunder Bluff for 940), and every segment of every listed route runs
/// between shown nodes: no route ends where no node is drawn.
#[test]
fn the_gadgetzan_map_draws_lines_only_between_its_shown_nodes() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
    let factions = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
    let open = open_at(40, &[22, 23, 25, 29, 30, 40, 55, 58]);
    // An orc, race 2.
    let (_, ui, resolved) =
        build_nodes_for(&open, &cat, Some(0.0), Some((2, &factions))).expect("map builds");
    assert_eq!(ui.len(), 8);
    let sun_rock = index_of(&ui, "Sun Rock Retreat, Stonetalon Mountains");
    assert_eq!(resolved[sun_rock].chain, vec![40, 30, 25, 29]);
    assert_eq!(ui[sun_rock].cost, 1070);

    let current = ui
        .iter()
        .find(|n| n.node_type == TaxiNodeType::Current)
        .expect("current");
    let shown: Vec<(f32, f32)> = ui
        .iter()
        .filter(|n| matches!(n.node_type, TaxiNodeType::Current | TaxiNodeType::Reachable))
        .map(|n| n.pos)
        .collect();
    let near = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6;
    for n in ui.iter().filter(|n| n.node_type == TaxiNodeType::Reachable) {
        let mut at = current.pos;
        for &[sx, sy, dx, dy] in &n.routes {
            assert!(
                near((sx, sy), at),
                "{}: a segment starts off the route",
                n.name
            );
            assert!(
                shown.iter().any(|&p| near(p, (dx, dy))),
                "{}: a segment ends on no shown node",
                n.name
            );
            at = (dx, dy);
        }
        assert!(near(at, n.pos), "{}: the line ends at its node", n.name);
    }
}

/// The filter over the shipped tables, every node known. An orc at the Crossroads gets no route
/// to Theramore or Astranaar; a troll (race 8, no template row) and a gnome (race 7, group 0)
/// do. A human at Ratchet gets none to the Crossroads, a direct neighbor at 110.
#[test]
fn shipped_maps_skip_the_other_faction_s_flight_points() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
    let factions = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
    let all = TaxiOpen {
        flightmaster: 0x42,
        nearest_node: 25,
        known: TaxiMask([u32::MAX; 8]),
    };
    let type_of = |ui: &[TaxiUiNode], name: &str| -> String {
        let i = index_of(ui, name);
        vm_of(ui.to_vec())
            .eval::<String>(&format!("return TaxiNodeGetType({})", i + 1))
            .unwrap()
    };
    for (race, want) in [(2, "NONE"), (8, "REACHABLE"), (7, "REACHABLE")] {
        let (_, ui, resolved) =
            build_nodes_for(&all, &cat, Some(0.0), Some((race, &factions))).expect("map");
        for name in ["Theramore, Dustwallow Marsh", "Astranaar, Ashenvale"] {
            assert_eq!(type_of(&ui, name), want, "race {race}: {name}");
        }
        assert_eq!(type_of(&ui, "Ratchet, The Barrens"), "REACHABLE");
        if race == 2 {
            assert!(
                resolved.iter().all(|r| !r.chain.contains(&32)),
                "no route passes Theramore"
            );
        }
    }
    let at_ratchet = TaxiOpen {
        nearest_node: 80,
        ..all
    };
    let (_, ui, resolved) =
        build_nodes_for(&at_ratchet, &cat, Some(0.0), Some((1, &factions))).expect("map");
    let crossroads = index_of(&ui, "Crossroads, The Barrens");
    assert_eq!(type_of(&ui, "Crossroads, The Barrens"), "NONE");
    assert_eq!(ui[crossroads].cost, 110);
    assert!(resolved[crossroads].chain.is_empty());
}
