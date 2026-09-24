//! The transport speed-profile probe (decision 0438's reconciliation instrument, director report
//! 2026-07-17: "ship lagged a bit on the way into Booty Bay").
//!
//! Samples the Ratchet–Booty Bay boat's timetable at 60 Hz across its whole cycle and measures the
//! finite-difference speed. Root-caused the report same day: ClientForms-mode u32 windows
//! (distance-linear interior timestamps) fed the trapezoid easing — seconds of disagreement, so
//! the boat pinned at segment ends and leapt on window flips (65 spikes, max 15,009 yd/s, one
//! 59 s crawl). `build()` now uses the internally-consistent Vmangos accumulation for the sample
//! table; this test stays as the smooth-motion regression gate. Anomaly classes:
//! - **speed spikes ≫ cruise (30 yd/s)** at segment boundaries → inter-segment position
//!   discontinuities (the integer arrive/depart times disagreeing with the float easing) — reads
//!   as stutter;
//! - **smooth but modulated speed** (swings well above/below cruise inside segments, worst on
//!   curves) → the Catmull-Rom *parameter* fraction standing in for an *arc-length* fraction —
//!   reads as the boat surging/dragging through bends;
//! - **neither** → the lag wasn't the timetable (frame hitches from Booty Bay's asset streaming
//!   are the next suspect).
//!
//! Run: `cargo test -p benilla-formats --test transport_speed_profile -- --nocapture`

use benilla_formats::{load_taxi_path_nodes, open_chain, TransportTimetable};

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

#[test]
fn ratchet_booty_bay_speed_profile() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");
    let nodes = load_taxi_path_nodes(&mut chain).expect("taxi nodes");
    let path = nodes.path(241).expect("path 241 (Ratchet–Booty Bay)");
    let tt = TransportTimetable::build(path, 30.0, 1.0).expect("timetable");
    // The build self-pins its period to the client-transcribed bookkeeping (bit-exact); this
    // cross-checks the pin landed for the path under measurement.
    assert_eq!(tt.period_ms, 350_818, "path 241's self-pinned period");

    let step_ms = 16u32; // ~60 Hz
    let dt = step_ms as f32 * 0.001;
    let mut prev = tt.sample(0);
    let mut moving_samples = 0u32;
    let mut speed_sum = 0.0f64;
    let mut max_speed = 0.0f32;
    let mut max_speed_at = 0u32;
    let mut spikes = Vec::new(); // (cycle_ms, speed) where speed > 1.5× cruise
    let mut slow_underway = Vec::new(); // (cycle_ms, speed) where moving but < 0.5× cruise
    let mut t = step_ms;
    while t < tt.period_ms {
        let cur = tt.sample(t);
        if cur.moving && prev.moving && cur.map == prev.map {
            let v = dist(cur.pos, prev.pos) / dt;
            moving_samples += 1;
            speed_sum += f64::from(v);
            if v > max_speed {
                max_speed = v;
                max_speed_at = t;
            }
            if v > 45.0 {
                spikes.push((t, v));
            } else if v < 15.0 {
                slow_underway.push((t, v));
            }
        }
        prev = cur;
        t += step_ms;
    }

    let mean = speed_sum / f64::from(moving_samples.max(1));
    eprintln!(
        "path 241: period {} ms, {} moving samples",
        tt.period_ms, moving_samples
    );
    eprintln!("mean speed {mean:.2} yd/s (cruise 30), max {max_speed:.2} at cycle {max_speed_at}");
    eprintln!(
        "spikes >45 yd/s: {} samples{}",
        spikes.len(),
        if spikes.is_empty() {
            String::new()
        } else {
            format!(" — first 10: {:?}", &spikes[..spikes.len().min(10)])
        }
    );
    // Cluster the slow samples into runs so the report reads as "windows", not thousands of rows.
    let mut slow_runs: Vec<(u32, u32, f32)> = Vec::new(); // (start, end, min_speed)
    for &(t, v) in &slow_underway {
        match slow_runs.last_mut() {
            Some((_, end, min_v)) if t - *end <= 2 * step_ms => {
                *end = t;
                *min_v = min_v.min(v);
            }
            _ => slow_runs.push((t, t, v)),
        }
    }
    eprintln!(
        "slow-underway (<15 yd/s while moving): {} samples in {} runs",
        slow_underway.len(),
        slow_runs.len()
    );
    for (start, end, min_v) in slow_runs.iter().take(20) {
        eprintln!(
            "  cycle {start}..{end} ms ({} s long), min {min_v:.2} yd/s",
            (end - start) / 1000
        );
    }

    // The structural assertion: a window/easing inconsistency reads as a position jump traversed
    // in one 16 ms step — hundreds to thousands of yd/s (the pre-fix table measured 15,009).
    // Healthy motion peaks ~51 yd/s (the Catmull-Rom parameter-vs-arc-length artifact through one
    // sharp bend, ~1.5 s per cycle — a known, bounded residual pending the per-point-table
    // verdict).
    assert!(
        max_speed < 100.0,
        "position discontinuity: {max_speed:.1} yd/s at cycle {max_speed_at} ms"
    );
}
