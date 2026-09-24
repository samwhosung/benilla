//! The reference's `MO_TRANSPORT` cycle period (`0x5f4cc0` with its arc-time solver `0x5f9120`),
//! transcribed step for step: it reproduces all nine transport paths' sniffed periods bit-exact,
//! and must, as the wire anchor is a server-uptime clock whose `% period` multiplies any error by
//! the cycle count. vmangos's periods are sniffs of this computation.
//!
//! A leg ends where the map changes or the previous row has `Flags & 1`; its first and last points
//! are Catmull-Rom guards the travel never reaches. A span runs stop to stop, a leg's ends counting
//! as mid-cruise, and each span rounds to ms on its own before the integer sum, which a float sum
//! over the path would miss. The period adds every stop's `Delay × 1000`.

use crate::taxi::TaxiPathNode;

/// The reference's basis `0xb05e10` (set at `0x453fa0`): uniform Catmull-Rom, tension 0.5, row
/// `i` the Horner coefficients, highest degree first, that weight `P[seg+i]`.
const CR_BASIS: [[f32; 4]; 4] = [
    [-0.5, 1.0, -0.5, 0.0],
    [1.5, -2.5, 0.0, 1.0],
    [-1.5, 2.0, 0.5, 0.0],
    [0.5, -0.5, 0.0, 0.0],
];

/// `0x453620`: the basis cubic in Horner form, computed in f64 and returned unrounded.
fn basis_weight(coeff: &[f32; 4], t: f32) -> f64 {
    let t = f64::from(t);
    let mut w = f64::from(coeff[0]);
    for &c in &coeff[1..] {
        w = w * t + f64::from(c);
    }
    w
}

/// `0x453580`: `Σ wᵢ·Pᵢ` over 4 control points, narrowing asymmetrically: the x product stays f64
/// into its add, the y and z products narrow to f32 first, and every accumulator narrows to f32
/// each step.
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

/// `0x453760`: 20 sub-chords at an f32 parameter stepped by 0.05, each `sqrt((dz²+dy²)+dx²)` in
/// f64, the sum narrowing to f32 every step.
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

/// `0x453300`: point `idx`'s distance from the leg's start, the f64 sum of the first `idx - 1`
/// f32 segment lengths (segment 0 spans `P[1]` → `P[2]`).
fn knot_sum(seg_len: &[f32], idx: usize) -> f64 {
    let mut acc = 0.0f64;
    for &k in seg_len.iter().take(idx.saturating_sub(1)) {
        acc += f64::from(k);
    }
    acc
}

/// Seconds to ms, rounded half away from zero: the reference's `__ftol` idiom (`0x40a2b0`).
fn round_ftol(t: f64) -> i32 {
    let scaled = t * 1000.0;
    let adj = if scaled > 0.0 {
        scaled + 0.5
    } else {
        scaled - 0.5
    };
    adj.trunc() as i32
}

/// `0x5f9120`: a span's constant-acceleration time over `d = p - l`, `A` and `B` stored as f32
/// but compared on the live f64 `B`. A leg's `first` span has one ramp, as its start is
/// mid-cruise; later spans have two.
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

/// `0x5f9120` block 2, a leg's last span: pure cruise `d/v` when the leg had no stop, else one
/// ramp out of the last stop.
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
    // Segments exist only past 3 points (`0x4532e0`); a shorter leg solves every span over d = 0.
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
    let mut l = 0.0f32; // the span start, an f32 slot (`[ebp+0x10]`)
    let mut processed = 0usize;
    for &(pt_idx, delay_ms) in stops {
        delays += delay_ms;
        // A stop on the leg's last point or guard has no span (`0x5f9120`'s `count - 1 <= idx`
        // break), but its delay counts.
        if pt_idx + 1 >= points.len() {
            break;
        }
        let p = knot_sum(&seg_len, pt_idx);
        duration += arc_time_ms(p, l, speed, accel, processed == 0);
        l = p as f32; // the next span's start narrows to f32 (`fst dword`)
        processed += 1;
    }
    duration += arc_time_ms_final(total, l, speed, accel, processed == 0);
    (duration, delays)
}

/// One transport path's period in ms, `0x5f4cc0`'s `handler+0x3c`; the `transports` calibration
/// test pins it to the nine sniffed values.
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
        // The stop check precedes the append, so a stop on a leg's first row is ignored
        // (`0x5f4cc0`'s `local_1c != 0` gate); `Flags & 2` is a bitmask test (`0x5f4e37`).
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
