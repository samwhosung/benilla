//! The real client's own MO_TRANSPORT cycle-period bookkeeping (`WoW.exe` `0x5f4cc0` + its
//! arc-time solver `0x5f9120`), transcribed step-for-step: this recipe reproduces **all nine**
//! live transport paths' server-sniff periods bit-exact. The wire anchor is a raw
//! server-uptime-scale clock, so the `% period` amplifies any Δms by the whole cycle count — the
//! period must be *exact*, not close (decision 0438 §3). vmangos pins its DB periods to sniffs of
//! THIS computation, so matching the client is matching the server.
//!
//! Layout facts this transcription rests on:
//! - Legs split when the row's map changes OR the previous row has `Flags & 1`; **every** row of
//!   the path lands in a leg — the leg's first and last points are Catmull-Rom guard points the
//!   travel never lands on (curve eval samples segment `seg` over `P[seg..seg+4]`, interpolating
//!   `P[seg+1] → P[seg+2]` (`0x453580`); `n_seg = count − 3`, built only when `count > 3`
//!   (`0x4532e0`).
//! - Per-segment arc length = 20 sub-chords of the cubic eval, **f32-narrowing accumulate**, the
//!   sample parameter itself an f32 stepped by 0.05 (`0x453760`); cumulative distance = f64 sum
//!   of the f32 segment lengths (`0x453300`); the cached leg total narrows to f32 (`0x4532e0`'s
//!   `fstp dword`).
//! - Span times: a span runs stop→stop *including leg start/end as mid-cruise boundaries* — the
//!   first span charges one ramp (the decel into the first stop), interior spans two (the full
//!   trapezoid), the final span one (the accel out); a leg with no stops is pure cruise `d/v`.
//!   Each span's time is rounded to ms **individually** (`round_ftol`) and accumulated as
//!   integer — the per-span rounding is exactly what a whole-path float accumulation misses.
//! - `period = Σ leg durations + Σ stop delays (Delay × 1000, integer)`; stops are counted once,
//!   and a stop flag on a leg's *first* row is ignored (the point list is still empty when the
//!   stop check runs — `0x5f4cc0`'s `local_1c != 0` gate).

use crate::taxi::TaxiPathNode;

/// The client's position-basis matrix `0xb05e10` — the uniform Catmull-Rom (tension 0.5) basis,
/// rows as Horner coefficients highest-degree-first (row `i` weights control point `P[seg+i]`).
/// Decoded from the static-init immediates at `0x453fa0`.
const CR_BASIS: [[f32; 4]; 4] = [
    [-0.5, 1.0, -0.5, 0.0],
    [1.5, -2.5, 0.0, 1.0],
    [-1.5, 2.0, 0.5, 0.0],
    [0.5, -0.5, 0.0, 0.0],
];

/// `0x453620` `BasisWeight`: the cubic basis polynomial in Horner form, f64-internal, returned
/// unrounded to the evaluator.
fn basis_weight(coeff: &[f32; 4], t: f32) -> f64 {
    let t = f64::from(t);
    let mut w = f64::from(coeff[0]);
    for &c in &coeff[1..] {
        w = w * t + f64::from(c);
    }
    w
}

/// `0x453580` cubic point evaluator: `out = Σ wᵢ·Pᵢ` over 4 consecutive control points, with the
/// **asymmetric** per-component narrowing the bytes show — the x product stays f64 into its add,
/// the y/z products narrow to f32 first (`fstp dword` temp), and every component's accumulator
/// re-narrows to f32 each step.
fn eval_point_cubic(cps: &[[f32; 3]], t: f32) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (i, p) in cps.iter().take(4).enumerate() {
        let w = basis_weight(&CR_BASIS[i], t);
        out[0] = (w * f64::from(p[0]) + f64::from(out[0])) as f32;
        let ty = (w * f64::from(p[1])) as f32;
        let tz = (w * f64::from(p[2])) as f32;
        out[1] = (f64::from(ty) + f64::from(out[1])) as f32;
        out[2] = (f64::from(tz) + f64::from(out[2])) as f32;
    }
    out
}

/// `0x453760` arc-length integrator: 20 sub-chords at an f32 parameter stepped by 0.05, chord
/// length `sqrt((dz²+dy²)+dx²)` f64-internal, the accumulator narrowing to f32 every step.
fn seg_arc_length(cps: &[[f32; 3]]) -> f32 {
    let mut prev = eval_point_cubic(cps, 0.0);
    let mut acc = 0.0f32;
    let mut t = f32::from_bits(0x3d4c_cccd); // 0.05
    for _ in 0..20 {
        let cur = eval_point_cubic(cps, t);
        let dx = f64::from(cur[0]) - f64::from(prev[0]);
        let dy = f64::from(cur[1]) - f64::from(prev[1]);
        let dz = f64::from(cur[2]) - f64::from(prev[2]);
        let len = ((dz * dz + dy * dy) + dx * dx).sqrt();
        acc = (len + f64::from(acc)) as f32;
        t = (f64::from(t) + 0.05f64) as f32;
        prev = cur;
    }
    acc
}

/// `0x453300` knot-sum: the cumulative distance of point index `idx` from the leg's travel start
/// = the pure-f64 sum of the first `idx − 1` per-segment f32 lengths (the guard-point shift makes
/// this exactly point `idx`'s distance — segment 0 spans `P[1] → P[2]`).
fn knot_sum(seg_len: &[f32], idx: usize) -> f64 {
    let mut acc = 0.0f64;
    for &k in seg_len.iter().take(idx.saturating_sub(1)) {
        acc += f64::from(k);
    }
    acc
}

/// `·1000` then round-half-away-from-zero then truncate — the client's `__ftol` rounding idiom
/// (`0x40a2b0`).
fn round_ftol(t: f64) -> i32 {
    let scaled = t * 1000.0;
    let adj = if scaled > 0.0 {
        scaled + 0.5
    } else {
        scaled - 0.5
    };
    adj.trunc() as i32
}

/// `0x5f9120` per-span arc time: the constant-acceleration solve over the span's distance
/// `d = p − l`, with the client's own f32/f64 mixing (`A` and `B` stored f32, the discriminant
/// compare on the *live* f64 `B`).
/// `first` = the leg's first span (one ramp: leg start is mid-cruise); otherwise two ramps.
fn arc_time_ms(p: f64, l: f32, speed: f32, accel: f32, first: bool) -> i32 {
    let d = p - f64::from(l);
    let a = (f64::from(speed) / f64::from(accel)) as f32;
    let b_live = 0.5 * f64::from(speed) * f64::from(a);
    let b = b_live as f32;
    let t: f64 = if first {
        if b_live < d {
            (d - f64::from(b)) / f64::from(speed) + f64::from(a)
        } else {
            (2.0 * d / f64::from(accel)).sqrt()
        }
    } else if b_live < 0.5 * d {
        (d - 2.0 * f64::from(b)) / f64::from(speed) + 2.0 * f64::from(a)
    } else {
        2.0 * (d / f64::from(accel)).sqrt()
    };
    round_ftol(t)
}

/// `0x5f9120` block 2, the leg's final span: with no stop
/// processed the whole leg is pure cruise `d/v` (no ramps — both ends are mid-cruise); after a
/// stop it's the one-ramp form (the accel out of the last stop).
fn arc_time_ms_final(p: f32, l: f32, speed: f32, accel: f32, first: bool) -> i32 {
    let d = f64::from(p) - f64::from(l);
    let t: f64 = if first {
        d / f64::from(speed)
    } else {
        let a = (f64::from(speed) / f64::from(accel)) as f32;
        let b_live = 0.5 * f64::from(speed) * f64::from(a);
        if b_live < d {
            (d - f64::from(b_live as f32)) / f64::from(speed) + f64::from(a)
        } else {
            (2.0 * d / f64::from(accel)).sqrt()
        }
    };
    round_ftol(t)
}

/// One closed leg's contribution: `(span-time sum, Σ its stop delays)`.
fn close_leg(points: &[[f32; 3]], stops: &[(usize, i32)], speed: f32, accel: f32) -> (i32, i32) {
    // BuildArcLen only runs for count > 3 (`0x4532e0`'s gate); a shorter leg keeps a zero
    // seg-table and zero cached total, so every span solves over d = 0.
    let (seg_len, total) = if points.len() > 3 {
        let n_seg = points.len() - 3;
        let seg_len: Vec<f32> = (0..n_seg).map(|s| seg_arc_length(&points[s..])).collect();
        let total = seg_len.iter().map(|&l| f64::from(l)).sum::<f64>() as f32; // fstp dword
        (seg_len, total)
    } else {
        (Vec::new(), 0.0f32)
    };

    let mut duration = 0i32;
    let mut delays = 0i32;
    let mut l = 0.0f32; // the running span-start distance ([ebp+0x10], an f32 slot)
    let mut processed = 0usize;
    for &(pt_idx, delay_ms) in stops {
        delays += delay_ms;
        // A stop on the leg's last point (or the guard) has no span of its own (`0x5f9120`'s
        // `count − 1 <= idx` break) — its delay still counts in the period.
        if pt_idx + 1 >= points.len() {
            break;
        }
        let p = knot_sum(&seg_len, pt_idx);
        duration += arc_time_ms(p, l, speed, accel, processed == 0);
        l = p as f32; // the solver's `fst dword` — the next span's start narrows to f32
        processed += 1;
    }
    duration += arc_time_ms_final(total, l, speed, accel, processed == 0);
    (duration, delays)
}

/// The full client period for one transport path, ms — `0x5f4cc0`'s `handler+0x3c`, bit-exact
/// (gold-gated against the nine server-sniff values by `transports.rs`' calibration test).
/// `None` for an empty path.
pub(crate) fn client_period_ms(nodes: &[TaxiPathNode], speed: f32, accel: f32) -> Option<u32> {
    if nodes.is_empty() || accel <= 0.0 {
        return None;
    }
    let mut period = 0i64;
    let mut points: Vec<[f32; 3]> = Vec::new();
    let mut stops: Vec<(usize, i32)> = Vec::new();
    let mut leg_map = nodes[0].map_id;
    let mut prev_teleport = false;
    for node in nodes {
        if node.map_id != leg_map || prev_teleport {
            let (dur, delays) = close_leg(&points, &stops, speed, accel);
            period += i64::from(dur) + i64::from(delays);
            points.clear();
            stops.clear();
            leg_map = node.map_id;
        }
        // The stop check runs before the point append: a stop flag on the leg's first row is
        // ignored (`0x5f4cc0`, the `local_1c != 0` gate). The client tests `Flags & 2` as a
        // bitmask (`0x5f4e37`) and multiplies the delay as an integer.
        if node.flags & 2 != 0 && !points.is_empty() {
            stops.push((points.len(), node.delay as i32 * 1000));
        }
        points.push(node.pos);
        prev_teleport = node.flags & 1 != 0;
    }
    if !points.is_empty() {
        let (dur, delays) = close_leg(&points, &stops, speed, accel);
        period += i64::from(dur) + i64::from(delays);
    }
    u32::try_from(period).ok()
}
