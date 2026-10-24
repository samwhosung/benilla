use super::*;
use crate::taxi::load_taxi_path_nodes;

fn node(id: u32, path_id: u32, node_index: u32, map_id: u32, flags: u32) -> TaxiPathNode {
    TaxiPathNode {
        id,
        path_id,
        node_index,
        map_id,
        pos: [id as f32, 0.0, 0.0],
        flags,
        delay: 0,
    }
}

fn ids(table: &TaxiLegTable, path: u32) -> Vec<Option<(u32, u32)>> {
    table.legs[&path]
        .iter()
        .map(|p| p.map(|(a, b)| (a.id, b.id)))
        .collect()
}

/// `0x408000`: a stop is a run's last and, the first time, its first; a map change and a flag 1
/// node close the run without being read as stops; the path change closes a run with a stop in
/// the path before it, and a run still open at the store's end is dropped.
#[test]
fn the_leg_table_closes_runs_as_the_reference_does() {
    let nodes = TaxiPathNodes::from_rows([
        node(1, 10, 0, 0, 0),
        node(2, 10, 1, 0, 2),
        node(3, 10, 2, 0, 2),
        // A new continent closes (2, 3); its own stop flag is not read.
        node(4, 10, 3, 1, 2),
        node(5, 10, 4, 1, 2),
        // Flag 1 closes (5, 5), and its stop flag is not read either.
        node(6, 10, 5, 1, 3),
        node(7, 10, 6, 1, 0),
        // A run with no stop: the path change appends nothing for it.
        node(8, 11, 0, 0, 0),
        node(9, 11, 1, 0, 1),
        // Path 12's run is still open at the end: dropped.
        node(10, 12, 0, 0, 2),
    ]);
    let table = TaxiLegTable::build(&nodes);
    assert_eq!(ids(&table, 10), [Some((2, 3)), Some((5, 5))]);
    // Flag 1 appends with no test of the first stop: a pair with no first stop.
    assert_eq!(ids(&table, 11), [None]);
    assert!(!table.legs.contains_key(&12));
}

/// The search of `0x4072c0` takes the first leg whose first stop is on the map left, and ends at
/// a leg with no stop.
#[test]
fn a_transfer_shows_the_leg_whose_first_stop_is_on_the_map_left() {
    let nodes = TaxiPathNodes::from_rows([
        node(1, 10, 0, 1, 2),
        node(2, 10, 1, 0, 0),
        node(3, 10, 2, 0, 2),
        node(4, 10, 3, 0, 1),
        node(5, 10, 4, 2, 1),
        node(6, 11, 0, 0, 0),
    ]);
    let table = TaxiLegTable::build(&nodes);
    assert_eq!(ids(&table, 10), [Some((1, 1)), Some((3, 3)), None]);
    assert_eq!(table.leg_from(10, 1), Some(0));
    assert_eq!(table.leg_from(10, 0), Some(1));
    assert_eq!(
        table.leg_from(10, 2),
        None,
        "the walk ends at the leg with no stop"
    );
    assert_eq!(table.leg_from(11, 0), None);
}

/// `0x4075a0`: the markers at A (the leg's last stop) and B (the next leg's first stop), the
/// ribbon a ship quad at every step from A to B, `max(trunc(L · 66.66667), 1)` steps.
#[test]
fn the_route_runs_from_the_legs_end_stop_to_the_next_legs_first() {
    let nodes = TaxiPathNodes::from_rows([
        node(1, 10, 0, 0, 2),
        node(2, 10, 1, 1, 0),
        node(3, 10, 2, 1, 2),
        node(4, 11, 0, 0, 0),
    ]);
    let table = TaxiLegTable::build(&nodes);
    let splines = LoadingScreenTaxiSplines::from_rows(vec![LoadingScreenTaxiSpline {
        path: 10,
        leg: 0,
        x: [0.3, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
        y: [0.0, 0.4, 0.6, 0.0, 0.0, 0.0, 0.0, 0.0],
    }]);
    // Node `id` projects to (id / 10, 0.2): A = (0.1, 0.2), B = (0.3, 0.2).
    let project = |_map: u32, x: f32, _y: f32| Some([x / 10.0, 0.2]);
    let route = table.route(10, 0, &splines, project).expect("a route");
    let centre = |q: &RouteQuad| {
        [
            (q[0].pos[0] + q[3].pos[0]) * 0.5,
            (q[0].pos[1] + q[3].pos[1]) * 0.5,
        ]
    };
    let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;
    assert!(close(centre(&route.markers[0]), [0.1, 0.2]));
    assert!(close(centre(&route.markers[1]), [0.3, 0.2]));
    // A takes the right quarter of the sheet's top half, B the left.
    assert_eq!(route.markers[0][0].uv, [0.5, 0.5]);
    assert_eq!(route.markers[1][3].uv, [0.5, 0.0]);
    let first = route.ribbon.first().unwrap();
    let last = route.ribbon.last().unwrap();
    assert!(close(centre(first), [0.1, 0.2]), "the ribbon starts at A");
    assert!(close(centre(last), [0.3, 0.2]), "and ends at B");
    // All three waypoints count: each has an x or a y above 0.
    let length = Curve::new(
        [
            [0.1, 0.2],
            [0.1, 0.2],
            [0.3, 0.0],
            [0.0, 0.4],
            [0.5, 0.6],
            [0.3, 0.2],
            [0.3, 0.2],
        ]
        .map(|[x, y]| [x, y, 0.0])
        .to_vec(),
    )
    .length;
    let steps = (f64::from(length) * f64::from(STEPS_PER_UNIT)).trunc() as usize;
    assert!(steps > 1);
    assert_eq!(route.ribbon.len(), steps + 1);
    // Leg 1 has no row.
    assert!(table.route(10, 1, &splines, project).is_none());
    // No WorldMapContinent row, no route.
    assert!(table.route(10, 0, &splines, |_, _, _| None).is_none());
}

/// `0x406fb0`: quad `k` draws while `k / (n − 1)` is not above the fraction.
#[test]
fn the_ribbon_reveals_with_the_load() {
    let quad = [RouteVertex {
        pos: [0.0; 2],
        uv: [0.0; 2],
    }; 4];
    let route = Route {
        markers: [quad; 2],
        ribbon: vec![quad; 5],
    };
    assert_eq!(route.revealed(0.0), 1);
    assert_eq!(route.revealed(0.25), 2);
    assert_eq!(route.revealed(0.49), 2);
    assert_eq!(route.revealed(0.5), 3);
    assert_eq!(route.revealed(1.0), 5);
    assert_eq!(route.revealed(f32::NAN), 5);
}

/// Path 241's legs on the install: Ratchet's stop on Kalimdor, then Booty Bay's on Azeroth, and
/// both of its `LoadingScreenTaxiSplines` rows.
#[test]
fn path_241_shows_ratchet_to_booty_bay() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let nodes = load_taxi_path_nodes(&mut chain).expect("TaxiPathNode");
    let splines = load_loading_screen_taxi_splines(&mut chain).expect("LoadingScreenTaxiSplines");
    let table = TaxiLegTable::build(&nodes);
    assert_eq!(ids(&table, 241), [Some((5047, 5047)), Some((5065, 5065))]);
    assert_eq!(table.leg_from(241, 1), Some(0), "leaving Kalimdor");
    assert_eq!(table.leg_from(241, 0), Some(1), "leaving Azeroth");
    for leg in [0, 1] {
        assert!(
            table
                .route(241, leg, &splines, |_, x, y| Some([x, y]))
                .is_some(),
            "leg {leg} has a route"
        );
    }
}
