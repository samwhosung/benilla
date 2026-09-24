//! `MO_TRANSPORT` timetable builder and cycle sampler: pure math in WoW coordinates, transcribing
//! vmangos's path builder `TransportMgr::GenerateWaypoints` (`TransportMgr.cpp:105-334`), its mover
//! `ShipTransport::Update`/`CalculateSegmentPos` (`Transport.cpp:283-380`) and its Catmull-Rom
//! splines (`Movement/spline/spline.cpp`).
//!
//! The period is the reference's own ([`crate::transport_period`]); vmangos's computed periods do
//! not match it, which is why the server overrides them from its DB (`TransportMgr.cpp:63-79`).
//! Between stops the sample follows vmangos's accumulation; the reference's per-leg tick
//! evaluation is not ported.

use std::f32::consts::PI;

use crate::taxi::TaxiPathNode;

/// A transport's position and heading at one instant of its cycle, in WoW coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportSample {
    /// The map of `pos`, which changes as a transport crosses continents.
    pub map: u32,
    pub pos: [f32; 3],
    /// Radians in `[0, 2π)`, `atan2(dir.y, dir.x) + π` (`Transport.cpp:349`).
    pub heading: f32,
    /// False while parked at a stop or on a keyframe's zero-width instant.
    pub moving: bool,
}

/// A keyframe that survives the map-change skip (`TransportMgr.cpp:128-152`), with what `sample`
/// needs. Stop and teleport are implicit: a non-stop window is zero-width, and a teleport's
/// `next_dist_from_prev` is 0.
#[derive(Debug, Clone)]
struct Frame {
    map_id: u32,
    pos: [f32; 3],
    initial_orientation: f32,
    dist_since_stop: f32,
    dist_until_stop: f32,
    /// Arc length to the next frame; 0 for a teleport frame and the last frame.
    next_dist_from_prev: f32,
    time_from: f32,
    time_to: f32,
    departure_time: u32,
    next_arrive_time: u32,
    /// The [`Leg`] and control index of this frame's outbound segment.
    leg: usize,
    local_index: usize,
}

/// A kept node before the derived quantities.
#[derive(Debug, Clone, Copy)]
struct RawFrame {
    map_id: u32,
    pos: [f32; 3],
    is_stop: bool,
    delay_secs: f32,
    initial_orientation: f32,
    teleport: bool,
}

/// A run of keyframes ending at a teleport or the path's end: a non-cyclic Catmull-Rom spline
/// (`InitCatmullRom`).
#[derive(Debug, Clone)]
struct Leg {
    controls: Vec<[f32; 3]>,
}

impl Leg {
    /// Control `i` with vmangos's virtual endpoints (`spline.cpp:262-265`): before the start
    /// `2·c0 − c1`, past the end a duplicate of the last control, not extrapolated.
    fn control(&self, i: isize) -> [f32; 3] {
        let n = self.controls.len();
        if i < 0 {
            let c0 = self.controls[0];
            let c1 = if n > 1 { self.controls[1] } else { c0 };
            lerp(c0, c1, -1.0)
        } else if (i as usize) >= n {
            self.controls[n - 1]
        } else {
            self.controls[i as usize]
        }
    }

    /// Position on segment `k` at `t` in `[0, 1]` (`SplineBase::EvaluateCatmullRom`).
    fn evaluate_percent(&self, k: usize, t: f32) -> [f32; 3] {
        catmull_rom_eval(
            self.control(k as isize - 1),
            self.control(k as isize),
            self.control(k as isize + 1),
            self.control(k as isize + 2),
            t,
        )
    }

    /// Derivative on segment `k` at `t` (`SplineBase::EvaluateDerivativeCatmullRom`).
    fn evaluate_derivative(&self, k: usize, t: f32) -> [f32; 3] {
        catmull_rom_derivative(
            self.control(k as isize - 1),
            self.control(k as isize),
            self.control(k as isize + 1),
            self.control(k as isize + 2),
            t,
        )
    }

    /// Arc length of segment `k` over `steps` chords, summed in `f64` as
    /// `SplineBase::SegLengthCatmullRom` does (`spline.cpp:159-177`).
    fn seg_length(&self, k: usize, steps: u32) -> f64 {
        let mut cur = self.control(k as isize); // t=0 evaluates to exactly controls[k]
        let mut total = 0.0f64;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let next = self.evaluate_percent(k, t);
            total += dist64(cur, next);
            cur = next;
        }
        total
    }
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn dist64(a: [f32; 3], b: [f32; 3]) -> f64 {
    let (dx, dy, dz) = (
        f64::from(a[0] - b[0]),
        f64::from(a[1] - b[1]),
        f64::from(a[2] - b[2]),
    );
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// vmangos's `s_catmullRomCoeffs` (`spline.cpp:61-65`) expanded as `t³ t² t 1` times the matrix:
/// the Catmull-Rom blending polynomials, τ = 0.5.
fn catmull_rom_weights(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    ]
}

/// `d/dt` of [`catmull_rom_weights`]: `C_Evaluate_Derivative`'s `3t² 2t 1 0` times the matrix.
fn catmull_rom_deriv_weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    [
        -1.5 * t2 + 2.0 * t - 0.5,
        4.5 * t2 - 5.0 * t,
        -4.5 * t2 + 4.0 * t + 0.5,
        1.5 * t2 - t,
    ]
}

fn blend(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3], w: [f32; 4]) -> [f32; 3] {
    [
        p0[0] * w[0] + p1[0] * w[1] + p2[0] * w[2] + p3[0] * w[3],
        p0[1] * w[0] + p1[1] * w[1] + p2[1] * w[2] + p3[1] * w[3],
        p0[2] * w[0] + p1[2] * w[1] + p2[2] * w[2] + p3[2] * w[3],
    ]
}

fn catmull_rom_eval(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3], t: f32) -> [f32; 3] {
    blend(p0, p1, p2, p3, catmull_rom_weights(t))
}

fn catmull_rom_derivative(
    p0: [f32; 3],
    p1: [f32; 3],
    p2: [f32; 3],
    p3: [f32; 3],
    t: f32,
) -> [f32; 3] {
    blend(p0, p1, p2, p3, catmull_rom_deriv_weights(t))
}

/// Wrap to `[0, 2π)`, as `Geometry::NormalizeOrientation`.
fn normalize_orientation(o: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let wrapped = o % tau;
    if wrapped < 0.0 {
        wrapped + tau
    } else {
        wrapped
    }
}

/// How keyframe times accumulate: `build` uses `Vmangos`, the calibration tests sweep both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeMode {
    /// vmangos's `f32` seconds, truncated to ms per keyframe (`TransportMgr.cpp:297-325`).
    Vmangos,
    /// The reference's closed form (`0x5f9120`) per stop-to-stop span, in integer ms. Interior
    /// times are distance-linear, so it only calibrates periods; only the tests build it.
    #[cfg_attr(not(test), allow(dead_code))]
    ClientForms,
}

/// Seconds to ms, `±0.5` then truncated, as the reference's `__ftol` idiom (`0x40a2b0`).
fn round_ftol(t: f64) -> i32 {
    let scaled = t * 1000.0;
    let adj = if scaled > 0.0 {
        scaled + 0.5
    } else {
        scaled - 0.5
    };
    adj.trunc() as i32
}

/// A stop-to-stop span's time in ms, the reference's `0x5f9120` `!first` branch:
/// `if 2B < D { (D − 2B)/v + 2A } else { 2·√(D/a) }`, with `A = v/a` stored `f32` and `B = ½·v·A`
/// compared in `f64` but stored `f32`; the mixed precision is the reference's.
fn span_time_ms(d_span: f32, speed: f32, accel: f32) -> u32 {
    if accel <= 0.0 {
        return 0;
    }
    let d = f64::from(d_span);
    let v = f64::from(speed);
    let a_param = f64::from(accel);
    let a: f32 = (v / a_param) as f32; // A, stored f32 as the reference does
    let b_live = 0.5 * v * f64::from(a); // B in f64, for the branch compare
    let b: f32 = b_live as f32; // B stored f32, for the arithmetic
    let t: f64 = if b_live < 0.5 * d {
        (d - 2.0 * f64::from(b)) / v + 2.0 * f64::from(a)
    } else {
        2.0 * (d / a_param).sqrt()
    };
    round_ftol(t).max(0) as u32
}

fn delay_ms(secs: f32) -> u32 {
    (secs * 1000.0).round().max(0.0) as u32
}

/// vmangos's accumulation (`TransportMgr.cpp:297-325`): add the previous frame's `TimeTo`, and at
/// a non-stop frame subtract its own.
fn accumulate_vmangos(raws: &[RawFrame], time_to: &[f32]) -> (Vec<u32>, Vec<u32>, Vec<u32>, u32) {
    let n = raws.len();
    let mut arrive = vec![0u32; n];
    let mut depart = vec![0u32; n];
    let mut next_arrive = vec![0u32; n];

    let mut cur_path_time = 0.0f32;
    if raws[0].is_stop {
        cur_path_time = raws[0].delay_secs;
        depart[0] = (cur_path_time * 1000.0) as u32;
    }
    for i in 1..n {
        cur_path_time += time_to[i - 1];
        if raws[i].is_stop {
            arrive[i] = (cur_path_time * 1000.0) as u32;
            next_arrive[i - 1] = arrive[i];
            cur_path_time += raws[i].delay_secs;
            depart[i] = (cur_path_time * 1000.0) as u32;
        } else {
            cur_path_time -= time_to[i];
            arrive[i] = (cur_path_time * 1000.0) as u32;
            next_arrive[i - 1] = arrive[i];
            depart[i] = arrive[i];
        }
    }
    next_arrive[n - 1] = depart[n - 1];
    let period = depart[n - 1];
    (arrive, depart, next_arrive, period)
}

/// Integer-ms times from [`span_time_ms`] per stop-to-stop span, exact at the stops; interior
/// frames are apportioned by distance, not by the reference's per-point scheme. The fragments
/// before the first stop and after the last are one span across the wrap, not two from rest.
fn accumulate_client_forms(
    raws: &[RawFrame],
    dist_from_prev: &[f32],
    speed: f32,
    accel: f32,
) -> (Vec<u32>, Vec<u32>, Vec<u32>, u32) {
    let n = raws.len();
    let mut arrive = vec![0u32; n];
    let mut depart = vec![0u32; n];
    let mut next_arrive = vec![0u32; n];

    let stops: Vec<usize> = raws
        .iter()
        .enumerate()
        .filter(|(_, r)| r.is_stop)
        .map(|(i, _)| i)
        .collect();

    if stops.is_empty() {
        // No stop at all: the whole cyclic path is one wrap span.
        let d_total: f32 = dist_from_prev[1..].iter().sum();
        let t_total = i64::from(span_time_ms(d_total, speed, accel));
        let mut cum = 0.0f32;
        for k in 1..n {
            cum += dist_from_prev[k];
            let frac = if d_total > 0.0 {
                f64::from(cum / d_total)
            } else {
                1.0
            };
            arrive[k] = (frac * t_total as f64).round().max(0.0) as u32;
            depart[k] = arrive[k];
        }
        if n > 1 {
            next_arrive[..n - 1].copy_from_slice(&arrive[1..n]);
        }
        if n > 0 {
            next_arrive[n - 1] = depart[n - 1];
        }
        let period = depart[n.saturating_sub(1)];
        return (arrive, depart, next_arrive, period);
    }

    let first_stop = stops[0];
    let last_stop = *stops.last().unwrap();

    let leading_dist: f32 = dist_from_prev[1..=first_stop].iter().sum();
    let trailing_dist: f32 = dist_from_prev[last_stop + 1..].iter().sum();
    let d_wrap = leading_dist + trailing_dist;
    let t_wrap = i64::from(span_time_ms(d_wrap, speed, accel));

    // The leading fragment, frame 0 at clock zero, at the wrap span's rate.
    let mut cum = 0.0f32;
    for k in 1..=first_stop {
        cum += dist_from_prev[k];
        let frac = if d_wrap > 0.0 {
            f64::from(cum / d_wrap)
        } else {
            1.0
        };
        arrive[k] = (frac * t_wrap as f64).round().max(0.0) as u32;
    }
    depart[first_stop] = arrive[first_stop] + delay_ms(raws[first_stop].delay_secs);

    // Interior spans, each two-ramp and self-contained.
    for w in stops.windows(2) {
        let (b_prev, b_next) = (w[0], w[1]);
        let depart_prev = i64::from(depart[b_prev]);
        let d_span: f32 = dist_from_prev[b_prev + 1..=b_next].iter().sum();
        let span_ms = i64::from(span_time_ms(d_span, speed, accel));
        let mut cum = 0.0f32;
        for k in (b_prev + 1)..=b_next {
            cum += dist_from_prev[k];
            let frac = if d_span > 0.0 {
                f64::from(cum / d_span)
            } else {
                1.0
            };
            let arrive_k = (depart_prev + (frac * span_ms as f64).round() as i64).max(0) as u32;
            arrive[k] = arrive_k;
            depart[k] = arrive_k + delay_ms(raws[k].delay_secs);
        }
    }

    // The trailing fragment, the wrap span's tail.
    let depart_last = i64::from(depart[last_stop]);
    let mut cum = 0.0f32;
    for k in (last_stop + 1)..n {
        cum += dist_from_prev[k];
        let frac = if d_wrap > 0.0 {
            f64::from(cum / d_wrap)
        } else {
            1.0
        };
        let arrive_k = (depart_last + (frac * t_wrap as f64).round() as i64).max(0) as u32;
        arrive[k] = arrive_k;
        depart[k] = arrive_k; // no stop follows last_stop by construction
    }

    if n > 1 {
        next_arrive[..n - 1].copy_from_slice(&arrive[1..n]);
    }
    next_arrive[n - 1] = depart[n - 1];
    let period = depart[n - 1];
    (arrive, depart, next_arrive, period)
}

/// A `MO_TRANSPORT`'s cyclic timetable, built once per `taxiPathId`.
#[derive(Debug, Clone)]
pub struct TransportTimetable {
    /// Cycle length in ms; `progress % period_ms` is the position in the loop.
    pub period_ms: u32,
    frames: Vec<Frame>,
    legs: Vec<Leg>,
    move_speed: f32,
    accel_rate: f32,
    accel_time: f32,
    accel_dist: f32,
}

impl TransportTimetable {
    /// Build from `TaxiPathNode` rows sorted by `node_index` and the `gameobject_template`
    /// `data1`/`data2` (`moveSpeed`, `accelRate`). The first and last nodes are never travelled,
    /// so fewer than 3 is `None`.
    pub fn build(nodes: &[TaxiPathNode], move_speed: f32, accel_rate: f32) -> Option<Self> {
        // Vmangos mode, so windows and easing come from one accumulation: ClientForms windows
        // under the trapezoid easing make a ship stick at segment ends and then leap. 20 chord
        // steps, the client's (`spline.h:61`).
        let mut tt =
            Self::build_with_variant(nodes, move_speed, accel_rate, TimeMode::Vmangos, 20)?;
        // The reference's period (`transport_period`), which vmangos's DB periods are sniffs of;
        // `% period` on the server-uptime anchor multiplies any error by the cycle count.
        if let Some(period) =
            crate::transport_period::client_period_ms(nodes, move_speed, accel_rate)
        {
            tt.override_period(period);
        }
        Some(tt)
    }

    /// The arrival instant of the first frame on `map_id`, scanning from the frame `from_cycle_ms`
    /// is in and wrapping. A worldport places a rider with it until the server re-anchors the
    /// transport's clock, a round trip after the ack (`Map::SendInitSelf`).
    pub fn first_cycle_on_map(&self, from_cycle_ms: u32, map_id: u32) -> Option<u32> {
        let n = self.frames.len();
        if n == 0 {
            return None;
        }
        // Start at the current frame: a clock already on the map answers here.
        let start = self
            .frames
            .iter()
            .position(|f| from_cycle_ms < f.next_arrive_time)
            .unwrap_or(n - 1);
        (0..n)
            .map(|k| (start + k) % n)
            .find(|&i| self.frames[i].map_id == map_id)
            .map(|i| match i {
                0 => 0,
                _ => self.frames[i - 1].next_arrive_time,
            })
    }

    /// Whether any keyframe is on `map_id`; a worldport keeps such a transport across the switch.
    pub fn touches_map(&self, map_id: u32) -> bool {
        self.frames.iter().any(|f| f.map_id == map_id)
    }

    /// `TransportMgr::GenerateWaypoints` phase by phase, with a chosen time mode and chord count.
    fn build_with_variant(
        nodes: &[TaxiPathNode],
        move_speed: f32,
        accel_rate: f32,
        mode: TimeMode,
        arc_steps: u32,
    ) -> Option<Self> {
        let n_raw = nodes.len();
        if n_raw < 3 || accel_rate <= 0.0 {
            return None;
        }

        // Phase A: the map-change skip (`TransportMgr.cpp:128-152`), end nodes never visited.
        let mut raws: Vec<RawFrame> = Vec::new();
        let mut map_change = false;
        for i in 1..n_raw - 1 {
            if map_change {
                map_change = false;
                continue;
            }
            let (node, next) = (&nodes[i], &nodes[i + 1]);
            if node.flags & 1 != 0 || node.map_id != next.map_id {
                if let Some(last) = raws.last_mut() {
                    last.teleport = true;
                }
                map_change = true;
            } else {
                // vmangos's whole-path spline derivative at t = 0 is this central difference of
                // the raw neighbours, kept or not (`TransportMgr.cpp:120-127`).
                let (prev, nxt) = (nodes[i - 1].pos, nodes[i + 1].pos);
                let initial_orientation =
                    normalize_orientation((nxt[1] - prev[1]).atan2(nxt[0] - prev[0]) + PI);
                raws.push(RawFrame {
                    map_id: node.map_id,
                    pos: node.pos,
                    // The reference tests the stop bit as a mask (`0x5f4e37`), vmangos's
                    // `IsStopFrame` as `== 2`; the data only holds 0 and 2.
                    is_stop: node.flags & 2 != 0,
                    delay_secs: node.delay as f32,
                    initial_orientation,
                    teleport: false,
                });
            }
        }
        if raws.is_empty() {
            return None;
        }
        // The last frame always teleports, even on a closed path (`GenerateWaypoints`).
        raws.last_mut().unwrap().teleport = true;
        let n = raws.len();

        // Phase B: a new leg starts after every teleport frame.
        let mut leg_of = vec![0usize; n];
        let mut local_index_of = vec![0usize; n];
        let mut leg_controls: Vec<Vec<[f32; 3]>> = vec![Vec::new()];
        {
            let mut leg = 0usize;
            for j in 0..n {
                if j > 0 && raws[j - 1].teleport {
                    leg += 1;
                    leg_controls.push(Vec::new());
                }
                leg_of[j] = leg;
                local_index_of[j] = leg_controls[leg].len();
                leg_controls[leg].push(raws[j].pos);
            }
        }
        let legs: Vec<Leg> = leg_controls
            .into_iter()
            .map(|controls| Leg { controls })
            .collect();

        // Phase D: `DistFromPrev`, `NextDistFromPrev` (`TransportMgr.cpp:192-234`).
        let mut dist_from_prev = vec![0.0f32; n];
        for j in 0..n {
            if local_index_of[j] > 0 {
                dist_from_prev[j] =
                    legs[leg_of[j]].seg_length(local_index_of[j] - 1, arc_steps) as f32;
            }
        }
        let mut next_dist_from_prev = vec![0.0f32; n];
        for j in 0..n {
            next_dist_from_prev[j] = if raws[j].teleport || j == n - 1 {
                0.0
            } else {
                dist_from_prev[j + 1]
            };
        }

        // `firstStop`, `lastStop` (`TransportMgr.cpp:213-222`).
        let mut first_stop: Option<usize> = None;
        let mut last_stop: Option<usize> = None;
        for (j, r) in raws.iter().enumerate() {
            if r.is_stop {
                first_stop.get_or_insert(j);
                last_stop = Some(j);
            }
        }
        let (first_stop, last_stop) = (first_stop.unwrap_or(0), last_stop.unwrap_or(0));

        // Phase E: `DistSinceStop`, `DistUntilStop` (`TransportMgr.cpp:237-256`).
        let mut dist_since_stop = vec![0.0f32; n];
        let mut tmp = 0.0f32;
        for i in 0..n {
            let j = (i + last_stop) % n;
            tmp = if raws[j].is_stop || j == last_stop {
                0.0
            } else {
                tmp + dist_from_prev[j]
            };
            dist_since_stop[j] = tmp;
        }
        let mut dist_until_stop = vec![0.0f32; n];
        let mut tmp = 0.0f32;
        for i in (0..n).rev() {
            let j = (i + first_stop) % n;
            tmp += dist_from_prev[(j + 1) % n];
            dist_until_stop[j] = tmp;
            if raws[j].is_stop || j == first_stop {
                tmp = 0.0;
            }
        }

        // Phase F: `TimeTo`, the four-regime trapezoid (`TransportMgr.cpp:260-284`).
        let accel_dist = 0.5 * move_speed * move_speed / accel_rate;
        let accel_time = move_speed / accel_rate;
        let mut time_to = vec![0.0f32; n];
        for j in 0..n {
            let (since, until) = (dist_since_stop[j], dist_until_stop[j]);
            let total = since + until;
            time_to[j] = if total < 2.0 * accel_dist {
                if since < until {
                    let segment_time = 2.0 * ((until + since) / accel_rate).sqrt();
                    segment_time - (2.0 * since / accel_rate).sqrt()
                } else {
                    (2.0 * until / accel_rate).sqrt()
                }
            } else if since < accel_dist {
                let segment_time = (until + since) / move_speed + (move_speed / accel_rate);
                segment_time - (2.0 * since / accel_rate).sqrt()
            } else if until < accel_dist {
                (2.0 * until / accel_rate).sqrt()
            } else {
                (until / move_speed) + (0.5 * move_speed / accel_rate)
            };
        }

        // Phase G: `TimeFrom` (`TransportMgr.cpp:287-295`).
        let mut time_from = vec![0.0f32; n];
        let mut segment_time = 0.0f32;
        for i in 0..n {
            let j = (i + last_stop) % n;
            if raws[j].is_stop || j == last_stop {
                segment_time = time_to[j];
            }
            time_from[j] = segment_time - time_to[j];
        }

        // Phase H: arrivals, departures and the period, by mode; `sample` needs no arrival.
        let (_arrive_time, departure_time, next_arrive_time, period_ms) = match mode {
            TimeMode::Vmangos => accumulate_vmangos(&raws, &time_to),
            TimeMode::ClientForms => {
                accumulate_client_forms(&raws, &dist_from_prev, move_speed, accel_rate)
            }
        };

        let frames: Vec<Frame> = (0..n)
            .map(|j| Frame {
                map_id: raws[j].map_id,
                pos: raws[j].pos,
                initial_orientation: raws[j].initial_orientation,
                dist_since_stop: dist_since_stop[j],
                dist_until_stop: dist_until_stop[j],
                next_dist_from_prev: next_dist_from_prev[j],
                time_from: time_from[j],
                time_to: time_to[j],
                departure_time: departure_time[j],
                next_arrive_time: next_arrive_time[j],
                leg: leg_of[j],
                local_index: local_index_of[j],
            })
            .collect();

        Some(TransportTimetable {
            period_ms,
            frames,
            legs,
            move_speed,
            accel_rate,
            accel_time,
            accel_dist,
        })
    }

    /// Pin the period as vmangos pins its DB value (`TransportMgr.cpp:63-79`): a longer one parks
    /// the transport at its last keyframe, a shorter one cuts the final approach at the wrap.
    fn override_period(&mut self, period_ms: u32) {
        if period_ms == 0 || period_ms == self.period_ms {
            return;
        }
        if period_ms > self.period_ms {
            if let Some(last) = self.frames.last_mut() {
                last.departure_time = period_ms;
                last.next_arrive_time = period_ms;
            }
        }
        self.period_ms = period_ms;
    }

    /// Position and heading at `cycle_ms` (`progress % period_ms`), as `ShipTransport::Update` and
    /// `CalculateSegmentPos` compute them (`Transport.cpp:283-380`), with no state between calls.
    pub fn sample(&self, cycle_ms: u32) -> TransportSample {
        let n = self.frames.len();
        if n == 0 {
            return TransportSample {
                map: 0,
                pos: [0.0; 3],
                heading: 0.0,
                moving: false,
            };
        }
        let cycle_ms = cycle_ms.min(self.period_ms.saturating_sub(1));

        // The frame whose `[ArriveTime, NextArriveTime)`, stop then travel, holds `cycle_ms`.
        let idx = self
            .frames
            .iter()
            .position(|f| cycle_ms < f.next_arrive_time)
            .unwrap_or(n - 1);
        let frame = &self.frames[idx];

        if cycle_ms < frame.departure_time || frame.next_dist_from_prev <= 0.0 {
            // At a stop, on a keyframe's instant, or on a teleport's zero-length travel.
            return TransportSample {
                map: frame.map_id,
                pos: frame.pos,
                heading: frame.initial_orientation,
                moving: false,
            };
        }

        // Moving: distance from the nearer stop under the same trapezoid as `TimeTo`
        // (`Transport.cpp:353-380`).
        let now = cycle_ms as f32 * 0.001;
        let since_departure = now - frame.departure_time as f32 * 0.001;
        let time_since_stop = frame.time_from + since_departure;
        let time_until_stop = frame.time_to - since_departure;
        let dist = if time_since_stop < time_until_stop {
            let d = if time_since_stop < self.accel_time {
                0.5 * self.accel_rate * time_since_stop * time_since_stop
            } else {
                self.accel_dist + (time_since_stop - self.accel_time) * self.move_speed
            };
            d - frame.dist_since_stop
        } else {
            let d = if time_until_stop < self.accel_time {
                0.5 * self.accel_rate * time_until_stop * time_until_stop
            } else {
                self.accel_dist + (time_until_stop - self.accel_time) * self.move_speed
            };
            frame.dist_until_stop - d
        };
        let t = (dist / frame.next_dist_from_prev).clamp(0.0, 1.0);

        let leg = &self.legs[frame.leg];
        let pos = leg.evaluate_percent(frame.local_index, t);
        let dir = leg.evaluate_derivative(frame.local_index, t);
        let heading = normalize_orientation(dir[1].atan2(dir[0]) + PI);

        TransportSample {
            map: frame.map_id,
            pos,
            heading,
            moving: true,
        }
    }
}

#[cfg(test)]
mod tests;
