//! Transport timetable tests against the nine live transports' server periods; they skip without
//! client data.

use super::*;
use crate::taxi::load_taxi_path_nodes;

struct Golden {
    entry: u32,
    path_id: u32,
    period_ms: u32,
    move_speed: f32,
    accel_rate: f32,
    /// A sniffed row, not a `build 0` fallback.
    sniffed: bool,
}

/// The nine live transports' periods from vmangos's `transports` table (the build ≤ 5875 row) and
/// their `gameobject_template` `data1`/`data2`.
const GOLDENS: &[Golden] = &[
    Golden {
        entry: 20808,
        path_id: 241,
        period_ms: 350818,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: true,
    },
    Golden {
        entry: 164871,
        path_id: 302,
        period_ms: 356284,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: true,
    },
    Golden {
        entry: 175080,
        path_id: 285,
        period_ms: 303463,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: false,
    },
    Golden {
        entry: 176231,
        path_id: 292,
        period_ms: 329313,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: true,
    },
    Golden {
        entry: 176244,
        path_id: 293,
        period_ms: 316251,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: false,
    },
    Golden {
        entry: 176310,
        path_id: 295,
        period_ms: 295579,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: true,
    },
    Golden {
        entry: 176495,
        path_id: 301,
        period_ms: 333044,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: false,
    },
    Golden {
        entry: 177233,
        path_id: 303,
        period_ms: 317040,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: true,
    },
    Golden {
        entry: 181056,
        path_id: 436,
        period_ms: 1_208_014,
        move_speed: 1.0,
        accel_rate: 1.0,
        sniffed: true,
    },
];

const MODES: &[(&str, TimeMode)] = &[("V", TimeMode::Vmangos), ("C", TimeMode::ClientForms)];
const STEPS_VARIANTS: &[u32] = &[3, 10, 20, 100];

/// A report of every mode and chord count against the goldens; it only asserts that each path's
/// best variant is within 5%.
#[test]
fn nine_period_calibration_report() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");

    let variant_count = MODES.len() * STEPS_VARIANTS.len();
    let mut all_nine_exact = vec![true; variant_count];
    let mut all_sniffed_exact = vec![true; variant_count];

    eprintln!(
        "{:>8} {:>5} {:>5} {:>6} {:>12} {:>12} {:>10} {:>9}",
        "entry", "path", "mode", "steps", "computed_ms", "golden_ms", "delta_ms", "pct"
    );

    for g in GOLDENS {
        let nodes = cat
            .path(g.path_id)
            .unwrap_or_else(|| panic!("path {} exists in TaxiPathNode.dbc", g.path_id));

        let mut best_pct = f64::MAX;
        let mut best_desc = String::new();
        let mut variant_idx = 0;
        for &(mode_name, mode) in MODES {
            for &steps in STEPS_VARIANTS {
                let computed = TransportTimetable::build_with_variant(
                    nodes,
                    g.move_speed,
                    g.accel_rate,
                    mode,
                    steps,
                )
                .map(|tt| tt.period_ms)
                .unwrap_or(0);
                let delta = i64::from(computed) - i64::from(g.period_ms);
                let pct = delta as f64 / f64::from(g.period_ms) * 100.0;
                eprintln!(
                    "{:>8} {:>5} {:>5} {:>6} {:>12} {:>12} {:>10} {:>8.4}%",
                    g.entry, g.path_id, mode_name, steps, computed, g.period_ms, delta, pct
                );

                if computed != g.period_ms {
                    all_nine_exact[variant_idx] = false;
                    if g.sniffed {
                        all_sniffed_exact[variant_idx] = false;
                    }
                }
                if pct.abs() < best_pct {
                    best_pct = pct.abs();
                    best_desc = format!("{mode_name} steps={steps} computed={computed}");
                }
                variant_idx += 1;
            }
        }

        assert!(
            best_pct < 5.0,
            "path {} ({}): best variant ({best_desc}) not within 5% of golden {}",
            g.path_id,
            g.entry,
            g.period_ms
        );
    }

    let mut variant_idx = 0;
    let mut any_all_nine = false;
    let mut any_all_sniffed = false;
    for &(mode_name, _) in MODES {
        for &steps in STEPS_VARIANTS {
            if all_nine_exact[variant_idx] {
                any_all_nine = true;
                eprintln!(
                    "*** variant mode={mode_name} steps={steps} matches ALL NINE golden periods EXACTLY ***"
                );
            }
            if all_sniffed_exact[variant_idx] {
                any_all_sniffed = true;
                eprintln!(
                    "*** variant mode={mode_name} steps={steps} matches ALL SIX SNIFFED golden periods EXACTLY ***"
                );
            }
            variant_idx += 1;
        }
    }
    if !any_all_nine && !any_all_sniffed {
        eprintln!(
            "(no variant matched any golden subset exactly this round — see the table above)"
        );
    }
}

/// `transport_period` must reproduce every golden exactly, since `build` pins its period to it.
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
        let computed = crate::transport_period::client_period_ms(nodes, g.move_speed, g.accel_rate)
            .unwrap_or(0);
        let delta = i64::from(computed) - i64::from(g.period_ms);
        eprintln!(
            "path {:>4}: client_period={computed:>8}  golden={:>8}  delta={delta:+}",
            g.path_id, g.period_ms
        );
        if delta != 0 {
            failures.push((g.path_id, computed, g.period_ms));
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

/// From any instant, the answer samples on the asked-for map, on every cross-continent path:
/// path 241 reaches map 1 only across the cycle wrap.
#[test]
fn first_cycle_on_map_lands_on_that_map() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");

    let mut crossed = 0usize;
    for g in GOLDENS {
        let nodes = cat
            .path(g.path_id)
            .unwrap_or_else(|| panic!("path {} exists in TaxiPathNode.dbc", g.path_id));
        let tt = TransportTimetable::build(nodes, g.move_speed, g.accel_rate)
            .unwrap_or_else(|| panic!("path {} builds", g.path_id));
        let maps: std::collections::HashSet<u32> = nodes.iter().map(|n| n.map_id).collect();
        if maps.len() < 2 {
            continue;
        }
        crossed += 1;
        // 64 instants across the cycle, both sides of the wrap included.
        for k in 0..64u32 {
            let from = (u64::from(tt.period_ms) * u64::from(k) / 64) as u32;
            for &m in &maps {
                let at = tt.first_cycle_on_map(from, m).unwrap_or_else(|| {
                    panic!(
                        "path {} touches map {m} but found no cycle on it",
                        g.path_id
                    )
                });
                assert!(
                    at < tt.period_ms,
                    "path {} map {m}: {at} ms is outside the {} ms cycle",
                    g.path_id,
                    tt.period_ms
                );
                assert_eq!(
                    tt.sample(at).map,
                    m,
                    "path {} from {from} ms: first_cycle_on_map({m}) = {at} ms samples map {}",
                    g.path_id,
                    tt.sample(at).map
                );
            }
        }
        // A map the path never visits.
        assert_eq!(tt.first_cycle_on_map(0, 9999), None);
    }
    assert!(
        crossed > 0,
        "no golden path crosses continents — this test proves nothing"
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
/// stands at Ratchet from 66858 ms and at Booty Bay from 233731 ms for 60 s.
const PATH_241: &[(u32, u32, [f32; 3], f32)] = &[
    (0, 1, [-1969.408, -4932.962, 0.0], -2.616_787_7),
    (30_000, 1, [-1382.4594, -4289.3115, 0.0], -2.026_320_7),
    (66_858, 1, [-1005.6135, -3841.6477, 0.0], 2.623_475_6),
    (100_000, 1, [-1005.6135, -3841.6477, 0.0], 2.623_475_6),
    (126_858, 1, [-1005.6135, -3841.6477, 0.0], 2.623_475_6),
    (140_000, 1, [-940.99854, -3897.3918, 0.0], 2.050_847_3),
    (180_000, 1, [-1353.893, -4842.0107, 0.0], 0.936_019_8),
    (193_437, 0, [-14366.629, 1302.033, 0.0], 1.565_639),
    (210_000, 0, [-14365.361, 822.61865, 0.0], 1.668_638_8),
    (233_731, 0, [-14277.745, 582.86615, 0.0], 2.819_079_6),
    (300_000, 0, [-14260.513, 578.3817, 0.0], 2.944_230_6),
    (340_000, 0, [-14590.503, 854.23706, 0.0], 0.055_232_66),
];

#[test]
fn path_241_follows_the_reference_bit_for_bit() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");
    let nodes = cat.path(241).expect("path 241");
    let tt = TransportTimetable::build(nodes, 30.0, 1.0).expect("it builds");
    assert_eq!(nodes[1].pos, PATH_241[0].2, "leg 1 opens at node 1");
    assert_eq!(nodes[24].pos, PATH_241[7].2, "leg 2 opens at node 24");
    for &(t, map, pos, heading) in PATH_241 {
        let s = tt.sample(t);
        assert_eq!(
            (s.map, s.pos.map(f32::to_bits), s.heading.to_bits()),
            (map, pos.map(f32::to_bits), heading.to_bits()),
            "at {t} ms: {:?} facing {}, the reference {pos:?} facing {heading}",
            s.pos,
            s.heading
        );
    }
}
