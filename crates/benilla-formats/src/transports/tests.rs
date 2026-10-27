//! Transport timetable tests against the nine live transports' server periods; they skip without
//! client data.

use super::*;
use crate::taxi::load_taxi_path_nodes;

struct Golden {
    entry: u32,
    path_id: u32,
    period_ms: u32,
    move_speed: u32,
    accel_rate: u32,
}

/// The nine live transports' periods from vmangos's `transports` table (the build ≤ 5875 row) and
/// their `gameobject_template` `data1`/`data2`.
const GOLDENS: &[Golden] = &[
    Golden {
        entry: 20808,
        path_id: 241,
        period_ms: 350818,
        move_speed: 30,
        accel_rate: 1,
    },
    Golden {
        entry: 164871,
        path_id: 302,
        period_ms: 356284,
        move_speed: 30,
        accel_rate: 1,
    },
    Golden {
        entry: 175080,
        path_id: 285,
        period_ms: 303463,
        move_speed: 30,
        accel_rate: 1,
    },
    Golden {
        entry: 176231,
        path_id: 292,
        period_ms: 329313,
        move_speed: 30,
        accel_rate: 1,
    },
    Golden {
        entry: 176244,
        path_id: 293,
        period_ms: 316251,
        move_speed: 30,
        accel_rate: 1,
    },
    Golden {
        entry: 176310,
        path_id: 295,
        period_ms: 295579,
        move_speed: 30,
        accel_rate: 1,
    },
    Golden {
        entry: 176495,
        path_id: 301,
        period_ms: 333044,
        move_speed: 30,
        accel_rate: 1,
    },
    Golden {
        entry: 177233,
        path_id: 303,
        period_ms: 317040,
        move_speed: 30,
        accel_rate: 1,
    },
    Golden {
        entry: 181056,
        path_id: 436,
        period_ms: 1_208_014,
        move_speed: 1,
        accel_rate: 1,
    },
];

/// The period must reproduce every golden exactly: the wire anchor is a server-uptime clock, whose
/// `mod period` multiplies any error by the cycle count.
#[test]
fn client_period_bit_exact() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");

    let mut failures = Vec::new();
    for g in GOLDENS {
        let nodes = cat
            .path(g.path_id)
            .unwrap_or_else(|| panic!("path {} exists in TaxiPathNode.dbc", g.path_id));
        let computed = TransportTimetable::build(nodes, g.move_speed, g.accel_rate)
            .map_or(0, |tt| tt.period_ms);
        let delta = i64::from(computed) - i64::from(g.period_ms);
        eprintln!(
            "path {:>4}: client_period={computed:>8}  golden={:>8}  delta={delta:+}",
            g.path_id, g.period_ms
        );
        if delta != 0 {
            failures.push((g.entry, g.path_id, computed, g.period_ms));
        }
    }
    assert!(
        failures.is_empty(),
        "client period diverges from the server-sniff goldens: {failures:?}"
    );
}

/// Also proves that at least one live transport crosses continents.
#[test]
fn touches_map_matches_the_paths_map_set() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");

    let mut any_cross_map = false;
    for g in GOLDENS {
        let nodes = cat
            .path(g.path_id)
            .unwrap_or_else(|| panic!("path {} exists in TaxiPathNode.dbc", g.path_id));
        let tt = TransportTimetable::build(nodes, g.move_speed, g.accel_rate)
            .unwrap_or_else(|| panic!("path {} builds", g.path_id));
        let maps: std::collections::HashSet<u32> = nodes.iter().map(|n| n.map_id).collect();
        any_cross_map |= maps.len() > 1;
        for &m in &maps {
            assert!(tt.touches_map(m), "path {} should touch map {m}", g.path_id);
        }
        assert!(
            !tt.touches_map(9999),
            "path {} touches a map no node lives on",
            g.path_id
        );
    }
    assert!(
        any_cross_map,
        "no golden path crosses continents — the worldport spare predicate has nothing to spare"
    );
}

/// The angle from `b` to `a`, wrapped to `(-π, π]`.
fn angle_between(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    if d > std::f32::consts::PI {
        d - std::f32::consts::TAU
    } else {
        d
    }
}

/// `0x5f50a0` faces the ship against its curve's tangent (`0x5f5251`-`0x5f52ae`), so over a whole
/// period, on every transport path, the facing turned half a circle is the direction of travel:
/// the chord of the positions about 1.5 yd either side at cruise. A flight path can part from it,
/// where a curve's derivative leaves its chord by more than 60° and `0x454580` takes the chord.
#[test]
fn a_ship_faces_its_direction_of_travel_on_every_transport_path() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");
    const STEP_MS: u32 = 250;
    let mut checked = 0usize;
    let mut worst: Vec<(u32, u32, f32)> = Vec::new();
    for g in GOLDENS {
        let nodes = cat.path(g.path_id).expect("the path exists");
        let tt = TransportTimetable::build(nodes, g.move_speed, g.accel_rate).expect("it builds");
        let speed = g.move_speed as f32;
        let half_window_ms = (1500.0 / speed) as u32;
        let window_s = 2.0 * half_window_ms as f32 * 0.001;
        let mut path_worst = (0u32, 0.0f32);
        let mut t = half_window_ms;
        while t + half_window_ms < tt.period_ms {
            let (before, here, after) = (
                tt.sample(t - half_window_ms),
                tt.sample(t),
                tt.sample(t + half_window_ms),
            );
            let chord = [after.pos[0] - before.pos[0], after.pos[1] - before.pos[1]];
            let moved = chord[0].hypot(chord[1]);
            // Under way at a third of cruise or more, on one map, with no teleport in the window.
            if before.map == after.map
                && moved >= speed * window_s / 3.0
                && moved <= speed * window_s * 1.5
            {
                let travel = chord[1].atan2(chord[0]);
                let off = angle_between(here.heading + std::f32::consts::PI, travel).abs();
                checked += 1;
                if off > path_worst.1 {
                    path_worst = (t, off);
                }
            }
            t += STEP_MS;
        }
        if path_worst.1 > 0.01 {
            worst.push((g.path_id, path_worst.0, path_worst.1));
        }
    }
    assert!(checked > 5_000, "only {checked} samples under way");
    assert!(
        worst.is_empty(),
        "paths facing off their travel by more than 0.01 rad, (path, cycle ms, rad): {worst:?}"
    );
}

/// Path 241 (Ratchet to Booty Bay) at instants of its period as the reference's build
/// (`0x5f4cc0`) and tick (`0x5f50a0`) place it, from benilla-re's `ship_path`, which matches the
/// client bit for bit: its legs open at their second nodes (1 and 24) at 0 and 193437 ms, and it
/// stands at Ratchet from 66858 ms and at Booty Bay from 233731 ms for 60 s, then speeds up
/// (0xa2), cruises (0xa3) and slows (0xa4) on `0x5f8dc0`.
const PATH_241: &[(u32, u32, [f32; 3], f32, u32)] = &[
    (0, 1, [-1969.408, -4932.962, 0.0], -2.616_787_7, 0xa3),
    (30_000, 1, [-1382.4594, -4289.3115, 0.0], -2.026_320_7, 0xa3),
    (66_858, 1, [-1005.6135, -3841.6477, 0.0], 2.623_475_6, 0),
    (100_000, 1, [-1005.6135, -3841.6477, 0.0], 2.623_475_6, 0),
    (126_858, 1, [-1005.6135, -3841.6477, 0.0], 2.623_475_6, 0xa2),
    (140_000, 1, [-940.99854, -3897.3918, 0.0], 2.050_847_3, 0xa2),
    (180_000, 1, [-1353.893, -4842.0107, 0.0], 0.936_019_8, 0xa3),
    (193_437, 0, [-14366.629, 1302.033, 0.0], 1.565_639, 0xa3),
    (210_000, 0, [-14365.361, 822.61865, 0.0], 1.668_638_8, 0xa4),
    (233_731, 0, [-14277.745, 582.86615, 0.0], 2.819_079_6, 0),
    (300_000, 0, [-14260.513, 578.3817, 0.0], 2.944_230_6, 0xa2),
    (340_000, 0, [-14590.503, 854.23706, 0.0], 0.055_232_66, 0xa3),
];

#[test]
fn path_241_follows_the_reference_bit_for_bit() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");
    let nodes = cat.path(241).expect("path 241");
    let tt = TransportTimetable::build(nodes, 30, 1).expect("it builds");
    assert_eq!(nodes[1].pos, PATH_241[0].2, "leg 1 opens at node 1");
    assert_eq!(nodes[24].pos, PATH_241[7].2, "leg 2 opens at node 24");
    for &(t, map, pos, heading, anim) in PATH_241 {
        let s = tt.sample(t);
        assert_eq!(
            (
                s.map,
                s.pos.map(f32::to_bits),
                s.heading.to_bits(),
                s.anim.id()
            ),
            (map, pos.map(f32::to_bits), heading.to_bits(), anim),
            "at {t} ms: {:?} facing {}, the reference {pos:?} facing {heading}",
            s.pos,
            s.heading
        );
    }
}

/// A point's nearest place on the segments `near` of a polyline: the distance to it, how far
/// along the line it lies and the segment it lies on.
fn onto_polyline(
    p: [f32; 2],
    line: &[[f32; 2]],
    near: std::ops::Range<usize>,
) -> (f32, f32, usize) {
    let (mut best, mut along, mut seg, mut walked) = (f32::INFINITY, 0.0, 0, 0.0);
    for (i, w) in line.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let len = len2.sqrt();
        let u = if len2 > 0.0 {
            (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let off = (p[0] - a[0] - u * d[0]).hypot(p[1] - a[1] - u * d[1]);
        if near.contains(&i) && off < best {
            (best, along, seg) = (off, walked + u * len, i);
        }
        walked += len;
    }
    (best, along, seg)
}

/// A regression guard, not a report's reproduction: path 241 (Ratchet to Booty Bay) sampled
/// every 50 ms over its whole period, as `0x5f4cc0` builds it from every node
/// (`0x5f4dda`-`0x5f4de9`) and `0x5f50a0` places it. On each leg the ship only ever moves the way
/// it faces, never back along the line its nodes draw (its run north from node 29 to 32 and back
/// to 35 is the route's own), never more than a curve's bow off that line and never off its map.
#[test]
fn path_241_runs_forward_along_its_route_over_the_whole_period() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");
    let nodes = cat.path(241).expect("path 241");
    let tt = TransportTimetable::build(nodes, 30, 1).expect("it builds");
    // The legs as the builder opens them: at a map change and after a flag 0x1 node.
    let mut legs: Vec<(u32, Vec<[f32; 2]>)> = Vec::new();
    let mut after_flag1 = false;
    for n in nodes {
        if legs.last().is_none_or(|l| l.0 != n.map_id) || after_flag1 {
            legs.push((n.map_id, Vec::new()));
        }
        legs.last_mut().unwrap().1.push([n.pos[0], n.pos[1]]);
        after_flag1 = n.flags & 1 != 0;
    }
    assert_eq!(legs.len(), 2, "one leg per continent");
    // A map's edge: 64 tiles of 533⅓ yd about its centre.
    const EDGE: f32 = 32.0 * 1600.0 / 3.0;
    const STEP_MS: u32 = 50;
    let (mut worst_off, mut worst_back, mut worst_turn) = (0.0f32, 0.0f32, 1.0f32);
    let mut prev: Option<(TransportSample, f32, usize)> = None;
    let mut t = 0;
    while t < tt.period_ms {
        let s = tt.sample(t);
        assert!(
            s.pos[0].abs() < EDGE && s.pos[1].abs() < EDGE,
            "at {t} ms the ship is off its map at {:?}",
            s.pos
        );
        let (_, line) = legs
            .iter()
            .find(|l| l.0 == s.map)
            .expect("a leg on its map");
        // Tracked from the segment it was last on, so a route that passes near itself does not
        // hand the ship to its other pass.
        let same = prev.filter(|(p, ..)| p.map == s.map);
        let near = same.map_or(0..line.len(), |(.., seg)| seg.saturating_sub(2)..seg + 3);
        let (off, along, seg) = onto_polyline([s.pos[0], s.pos[1]], line, near);
        worst_off = worst_off.max(off);
        if let Some((p, p_along, _)) = same {
            worst_back = worst_back.max(p_along - along);
            let d = [s.pos[0] - p.pos[0], s.pos[1] - p.pos[1]];
            let moved = d[0].hypot(d[1]);
            if moved > 1e-3 {
                // The ship faces against its tangent, so it travels towards its heading + π.
                let h = p.heading + std::f32::consts::PI;
                worst_turn = worst_turn.min((d[0] * h.cos() + d[1] * h.sin()) / moved);
            }
        }
        prev = Some((s, along, seg));
        t += STEP_MS;
    }
    eprintln!(
        "off the route {worst_off} yd, back along it {worst_back} yd, travel·facing {worst_turn}"
    );
    // The curve bows off its chords through a bend, up to 18 yd on this path.
    assert!(
        worst_off < 25.0,
        "the ship strays {worst_off} yd off its route"
    );
    assert!(
        worst_back < 0.1,
        "the ship goes {worst_back} yd back along its route"
    );
    assert!(
        worst_turn > 0.9,
        "the ship moves against its facing: cos {worst_turn}"
    );
}
