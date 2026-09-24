//! The nine-golden calibration + gold gates for the transport timetable (decision 0438 phase
//! 0): the mode × chord-steps period report against the server's effective (DB-pinned) periods,
//! and the bit-exactness gate on the client-transcribed period (`crate::transport_period`) that
//! lets [`TransportTimetable::build`] self-pin. Needs the vanilla client data (skips otherwise).

use super::*;
use crate::taxi::load_taxi_path_nodes;

struct Golden {
    entry: u32,
    path_id: u32,
    period_ms: u32,
    move_speed: f32,
    accel_rate: f32,
    /// A real build sniff (vs. a `build 0` fallback row) — the task's distinguished subset.
    sniffed: bool,
}

/// The nine live transports' server-effective (DB-pinned) periods — vmangos `transports`
/// table, build-≤5875 row selected, and `gameobject_template` `data1`/`data2`, both read this
/// session (decision 0438's data-model survey). All moveSpeed=30/accelRate=1 except Naxxramas
/// (181056), moveSpeed=1/accelRate=1.
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

/// Builds every one of the nine live transports' timetables under every (mode × arc-length
/// chord-sampling) combination and reports computed-vs-golden period for each. This is the
/// calibration report the phase-0 gate needs, not a pass/fail bit: the goldens pin **which**
/// variant is the real client's algorithm (decision 0438 phase 0), so only loose sanity is
/// asserted here — a period > 0, and the *best* variant per path within 5% of golden.
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

/// The bit-exactness gate for the client-transcribed period (`transport_period`): all nine
/// live transports must reproduce their server-sniff golden **exactly** — this is what lets
/// [`TransportTimetable::build`] self-pin its cycle length instead of consulting a hardcoded
/// server table (decision 0438 §3's exactness requirement).
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

/// [`TransportTimetable::touches_map`] answers exactly the raw path's map set — the cross-map
/// worldport's spare predicate rests on it — and the premise itself holds: at
/// least one of the nine live transports really does cross continents mid-cycle.
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
        "no golden path crosses continents — the 0455 spare predicate has nothing to spare"
    );
}

/// [`TransportTimetable::first_cycle_on_map`] — the seam's re-anchor target.
///
/// Two properties, on every cross-continent path in the fleet, because the fleet's seams are not
/// alike: one path changes map mid-cycle, another only at the cycle wrap (path 241's map-1 legs are
/// frames 0..22 and its map-0 legs are the tail, so "cross to Kalimdor" *is* the wrap), and a
/// re-anchor that only handled the interior case would leave exactly the reported ferry broken.
///
/// 1. From anywhere in the cycle, the answer is an instant that really does sample on the asked-for
///    map — that is the whole contract, and it is what the rider's world pose is composed through.
/// 2. Asking from an instant already on that map returns an instant on it too (the caller's
///    already-there short-circuit rests on the map agreeing, not on the number).
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
        // Probe from 64 instants spread across the whole cycle — every leg, both continents, and
        // the frames either side of the wrap.
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
        // A map the path never visits has no answer at all.
        assert_eq!(tt.first_cycle_on_map(0, 9999), None);
    }
    assert!(
        crossed > 0,
        "no golden path crosses continents — this test proves nothing"
    );
}
