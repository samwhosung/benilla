//! Samples the Ratchet-Booty Bay boat's timetable at 60 Hz over its whole cycle and measures the
//! finite-difference speed against its 30 yd/s cruise. A spike at a segment boundary is a position
//! discontinuity; a smooth swing through a bend is the spline parameter standing in for arc length.
//! `--nocapture` prints the profile.

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
    // The build pins its period to the reference's own bookkeeping, bit-exact.
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
    // Cluster the slow samples into runs.
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

    // A discontinuity crosses in one 16 ms step at hundreds of yd/s. Smooth motion peaks near
    // 51 yd/s through one sharp bend: the table between stops is vmangos-mode, the spline parameter
    // standing in for arc length, and the reference's per-leg evaluation is not ported.
    assert!(
        max_speed < 100.0,
        "position discontinuity: {max_speed:.1} yd/s at cycle {max_speed_at} ms"
    );
}
