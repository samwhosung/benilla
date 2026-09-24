//! `MO_TRANSPORT` timetable builder + cycle sampler (decision 0438 phase 0) — pure math, no Bevy,
//! **WoW coordinates throughout** (the Bevy transform is the consumer's job).
//!
//! This transcribes the structural skeleton of vmangos's own transport-path builder and mover:
//! - `TransportMgr::GenerateWaypoints` (vmangos `src/game/Transports/
//!   TransportMgr.cpp:105-334`) — the keyframe list, the map-change/teleport skip dance, the
//!   per-leg Catmull-Rom splines, the `DistSinceStop`/`DistUntilStop` modular walks, the
//!   four-regime `TimeTo` trapezoid, and the `ArriveTime`/`DepartureTime` accumulation.
//! - `ShipTransport::Update` + `CalculateSegmentPos` (`.../Transport.cpp:283-380`) — the
//!   stateless "where on the path is `progress % period`" query, [`TransportTimetable::sample`].
//! - The spline machinery (`.../Movement/spline/spline.cpp`): `s_catmullRomCoeffs` (:61-65) is
//!   transcribed as closed-form blending polynomials in [`catmull_rom_weights`] /
//!   [`catmull_rom_deriv_weights`] (verified by hand-expanding the `Vector4 * Matrix4` row-vector
//!   product — standard Catmull-Rom basis functions, tau = 0.5); `SegLengthCatmullRom` (:159-177,
//!   `STEPS_PER_SEGMENT` chord samples) is [`Leg::seg_length`]; `InitCatmullRom`'s non-cyclic
//!   virtual endpoints (:238-268 — the **first** virtual point is `lerp(c0, c1, -1)`, i.e.
//!   `2·c0 − c1`; the **last** is a plain **duplicate** of the final real control, not
//!   extrapolated) are [`Leg::control`].
//!
//! **The whole-path orientation spline collapses to a closed form.** `GenerateWaypoints` builds a
//! second, separate Catmull-Rom spline over the *entire* raw path (with hand-computed virtual
//! endpoints, `TransportMgr.cpp:120-127`) purely to read `InitialOrientation` off
//! `evaluate_derivative(i, t=0)` for every kept node. Expanding `s_catmullRomCoeffs`' derivative
//! weights at `t=0` gives `[-0.5, 0, 0.5, 0]` — only the two *interior* control points survive (the
//! virtual endpoints' weight is always zero at `t=0`, and are in fact never even reached within the
//! nodes this loop visits). So `InitialOrientation(i) = atan2(path[i+1].y − path[i−1].y,
//! path[i+1].x − path[i−1].x) + π`, using the **raw, unfiltered** node array — no spline object
//! needed. See `orientation_at` below.
//!
//! **Timing-mode calibration is the point of this module.** vmangos's own computed periods do
//! **not** match the real client's — the server DB-overrides them (`transports` table,
//! `TransportMgr.cpp:63-79`, "load period override from db since our algorithm is not perfect").
//! The client's per-span arc-length time is transcribed bit-exact from the reference client's
//! `0x5f9120` — closed-form constant-acceleration kinematics, ×1000, round-half-away-from-zero,
//! truncate. [`TimeMode`] (private —
//! not public API) selects between vmangos's own f32-seconds accumulation (`Mode::Vmangos`,
//! transcribing `TransportMgr.cpp:297-325` exactly) and a client-closed-form per-span accumulation
//! (`Mode::ClientForms`); the arc-length chord-sampling density (`STEPS_PER_SEGMENT` — vmangos uses
//! 3, its own comment notes "client's value is 20") is a second calibration knob. The `tests`
//! module builds all nine live transports under every (mode × steps) combination against the
//! server's actually-effective (DB-pinned) periods and reports a table — see decision 0438 phase 0.
//!
//! **The cycle length itself is past calibration:** [`TransportTimetable::build`] pins its period
//! to the real client's own bookkeeping, transcribed in [`crate::transport_period`] and gold-gated
//! bit-exact against all nine server-sniff values. The sample
//! table between the pins stays vmangos-mode (internally consistent windows + easing); porting the
//! client's true per-leg tick evaluation is the recorded follow-on.

use std::f32::consts::PI;

use crate::taxi::TaxiPathNode;

/// A transport's position/heading at one instant of its cycle — WoW coordinates, ready for the
/// Bevy consumer's own coordinate transform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportSample {
    /// The map this `pos` is expressed in (can differ from the timetable's "home" map — a
    /// transport crosses continents mid-cycle).
    pub map: u32,
    /// World position `(x, y, z)`.
    pub pos: [f32; 3],
    /// Facing, radians, normalized to `[0, 2π)` — `atan2(dir.y, dir.x) + π`
    /// (`Transport.cpp:349`).
    pub heading: f32,
    /// `false` while parked at a station stop (or the zero-width instant at a non-stop
    /// keyframe); `true` while under way.
    pub moving: bool,
}

/// A single kept keyframe — one surviving `TaxiPathNode` after the map-change/teleport skip dance
/// (`TransportMgr.cpp:128-152`), with every quantity [`TransportTimetable::sample`] needs from
/// `GenerateWaypoints`. `IsStopFrame`/`Teleport` themselves aren't stored: `sample` recovers both
/// from the derived timestamps/distances alone (a non-stop frame's `[arrive, depart)` window is
/// always zero-width; a teleport frame's `next_dist_from_prev` is always `0` — see their doc
/// comments below) rather than carrying redundant state.
#[derive(Debug, Clone)]
struct Frame {
    map_id: u32,
    pos: [f32; 3],
    initial_orientation: f32,
    dist_since_stop: f32,
    dist_until_stop: f32,
    /// The distance of the segment from this frame *to the next* — `0` for a teleport frame (no
    /// continuous travel across a jump) and for the path's final frame.
    next_dist_from_prev: f32,
    time_from: f32,
    time_to: f32,
    departure_time: u32,
    next_arrive_time: u32,
    /// Which [`Leg`] (and which 0-based control-point position within it) this frame's outbound
    /// spline segment is evaluated on.
    leg: usize,
    local_index: usize,
}

/// A raw kept node, pre-derived-quantities — the map-change dance's output, source data for
/// [`Frame`] and both timing modes.
#[derive(Debug, Clone, Copy)]
struct RawFrame {
    map_id: u32,
    pos: [f32; 3],
    is_stop: bool,
    delay_secs: f32,
    initial_orientation: f32,
    teleport: bool,
}

/// One spline leg: a maximal run of keyframes between teleport boundaries (a leg's last member is
/// always a teleport frame, or the path's final frame). Non-cyclic Catmull-Rom
/// (`InitCatmullRom`'s non-cyclic branch) over `controls`, with virtual endpoints computed
/// on-demand by [`Leg::control`] rather than stored.
#[derive(Debug, Clone)]
struct Leg {
    controls: Vec<[f32; 3]>,
}

impl Leg {
    /// The control point at raw (possibly out-of-range) index `i`, extending with vmangos's
    /// non-cyclic virtual endpoints: `i < 0` → `2·c0 − c1` (extrapolated); `i >= len` → a
    /// **duplicate** of the last real control (not extrapolated) — `spline.cpp:262-265`.
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

    /// Position on segment `k` (0-based, connecting `controls[k]` to `controls[k+1]`) at `t ∈
    /// [0, 1]` — `SplineBase::EvaluateCatmullRom`.
    fn evaluate_percent(&self, k: usize, t: f32) -> [f32; 3] {
        catmull_rom_eval(
            self.control(k as isize - 1),
            self.control(k as isize),
            self.control(k as isize + 1),
            self.control(k as isize + 2),
            t,
        )
    }

    /// Derivative on segment `k` at `t` — `SplineBase::EvaluateDerivativeCatmullRom`.
    fn evaluate_derivative(&self, k: usize, t: f32) -> [f32; 3] {
        catmull_rom_derivative(
            self.control(k as isize - 1),
            self.control(k as isize),
            self.control(k as isize + 1),
            self.control(k as isize + 2),
            t,
        )
    }

    /// Arc length of segment `k`, approximated by `steps` evenly-spaced chords —
    /// `SplineBase::SegLengthCatmullRom` (`spline.cpp:159-177`), `length_type` is `double` there
    /// (`Movement::Spline<double>`), matched here.
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

/// The vmangos `s_catmullRomCoeffs` matrix (`spline.cpp:61-65`), expanded by hand as the `t³ t² t
/// 1` row-vector times the matrix (`C_Evaluate`'s `Vector4 weights(tvec * matr)`) into the four
/// classic Catmull-Rom (τ = 0.5) blending polynomials — verified by direct expansion, not assumed.
fn catmull_rom_weights(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    ]
}

/// `d/dt` of [`catmull_rom_weights`] — `C_Evaluate_Derivative`'s `3t² 2t 1 0` row vector times the
/// same matrix.
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

/// Wrap to `[0, 2π)` — `Geometry::NormalizeOrientation`.
fn normalize_orientation(o: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let wrapped = o % tau;
    if wrapped < 0.0 {
        wrapped + tau
    } else {
        wrapped
    }
}

/// The timing-accumulation variant — see the module doc's "Timing-mode calibration". Not public
/// API: [`TransportTimetable::build`] picks one default; the calibration test in this module's
/// `#[cfg(test)]` sweeps all of them, including `Vmangos` — hence the `cfg_attr` below: outside
/// `cargo test`, `build`'s hardcoded default is the only caller, so plain `cargo build` never
/// constructs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeMode {
    /// vmangos's own `TransportMgr.cpp:297-325`: a running `f32` seconds accumulator, truncated
    /// to ms once per keyframe. **The sample-table default** — windows and easing from one
    /// accumulation (see [`TransportTimetable::build`]).
    Vmangos,
    /// The real client's constant-acceleration closed form (client fn `0x5f9120`),
    /// applied once per stop-to-stop span and rounded with `round_ftol`, accumulated as integer
    /// ms. Interior timestamps are distance-linear — a period-calibration device (the golden
    /// sweep), NOT a sampling table; constructed only by the calibration tests.
    #[cfg_attr(not(test), allow(dead_code))]
    ClientForms,
}

/// `·1000.0` then round-half-away-from-zero then truncate — the client's own `__ftol` (`0x40a2b0`)
/// rounding idiom (`fcom 0.0;test ah,0x41` → `±0.5`), transcribed exactly.
fn round_ftol(t: f64) -> i32 {
    let scaled = t * 1000.0;
    let adj = if scaled > 0.0 {
        scaled + 0.5
    } else {
        scaled - 0.5
    };
    adj.trunc() as i32
}

/// The client's "both ramps present" closed form for a stop-to-stop span of distance `d_span`
/// (the reference client's `0x5f9120`, `!first` branch: `if 2B < D { (D −
/// 2B)/v + 2A } else { 2·√(D/a) }`, `A = v/a` stored `f32`, `B = ½·v·A` kept live `f64` for the
/// branch compare but stored `f32` for the arithmetic — the exact f32/f64 mixing is bit-exact),
/// rounded to ms via [`round_ftol`].
fn span_time_ms(d_span: f32, speed: f32, accel: f32) -> u32 {
    if accel <= 0.0 {
        return 0;
    }
    let d = f64::from(d_span);
    let v = f64::from(speed);
    let a_param = f64::from(accel);
    let a: f32 = (v / a_param) as f32; // A, stored f32 (the client's own precision loss)
    let b_live = 0.5 * v * f64::from(a); // B kept live (f64) — the branch compare uses this
    let b: f32 = b_live as f32; // B stored f32 — the arithmetic reads this
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

/// Mode V: vmangos's own accumulation, transcribed exactly (`TransportMgr.cpp:297-325`) —
/// including the "add the previous frame's `TimeTo`, then subtract this frame's own `TimeTo` back
/// off when it isn't a stop" pattern (the running-estimate-plus-correction trick that gives
/// continuous-looking per-frame timestamps from a per-frame "time to the far stop" quantity).
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

/// Mode C: per-stop-to-stop-span time from the client's closed form ([`span_time_ms`]),
/// accumulated as integer ms; delays enter as exact `delay·1000`. A span's *boundary* timestamps
/// (at its two stop frames) are exact by construction — that's what the golden gate needs.
/// Interior (non-stop) frame timestamps are apportioned by cumulative distance within the span, a
/// phase-0 simplification: only the period (a boundary quantity) is gated this round; the
/// client's true per-point interior scheme is phase-1 work (see the module doc).
///
/// **The path's own start/end fragments (frame 0 to the first stop; the last stop to the final
/// frame) are not self-contained spans** — frame 0 and the final frame are both mid-cruise, not
/// at rest (the path is cyclic: the true journey between the *last* stop and the *first* stop
/// runs last_stop → final frame → [the period wrap, zero distance] → frame 0 → first_stop, one
/// continuous two-ramp span). Treating the two fragments as independent fresh spans (each
/// wrongly assumed to start/end at rest) double-charges roughly one full `accel_time` across the
/// period — the ~30 s-off-of-30 s-`accel_time` bug this comment is pinned against. `leading_dist`
/// / `trailing_dist` / `d_wrap` below reconstruct that one true wrap span; both fragments are
/// timed off its single [`span_time_ms`] total, at their own distance fraction of it.
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
        // Degenerate (no station stop anywhere on the path) — not expected for a real transport;
        // a defensive fallback treating the whole cyclic path as one wrap span, no delays.
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

    // Leading fragment (frame 0 is clock-zero by definition; arrive[0] stays its `vec![0; n]`
    // default): every frame up to and including first_stop, at the wrap span's rate.
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

    // Interior stop-to-stop spans, in order — each is a genuine, self-contained two-ramp span.
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

    // Trailing fragment (last_stop to the final frame) — the wrap span's tail, same rate.
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

/// A `MO_TRANSPORT`'s full cyclic timetable, built once per `taxiPathId` — the timekeeping half of
/// decision 0438 phase 0. See the module doc for provenance.
#[derive(Debug, Clone)]
pub struct TransportTimetable {
    /// The full cycle length, ms — `progress % period_ms` is the transport's position in its
    /// loop (decision 0438 §3).
    pub period_ms: u32,
    frames: Vec<Frame>,
    legs: Vec<Leg>,
    move_speed: f32,
    accel_rate: f32,
    accel_time: f32,
    accel_dist: f32,
}

impl TransportTimetable {
    /// Build a transport's timetable from its `TaxiPathNode` rows (sorted by `node_index`) and
    /// its `gameobject_template` `data1`/`data2` (`moveSpeed`/`accelRate`, as `f32`). `None` if
    /// the path is too short to have an interior (fewer than 3 nodes — the first and last are
    /// always teleport-arrival cells with no travel of their own).
    pub fn build(nodes: &[TaxiPathNode], move_speed: f32, accel_rate: f32) -> Option<Self> {
        // Vmangos-mode accumulation for the SAMPLE table: its u32 windows and the float
        // trapezoid easing derive from one accumulation, so `sample()` is internally consistent.
        // ClientForms apportions interior timestamps linearly by distance (a period-calibration
        // device, per its doc) — feeding those windows to the trapezoid easing made the boat
        // stick at segment ends and leap when the window flipped (the director's "ship lagged
        // into Booty Bay", measured at up to 15,000 yd/s spikes by the speed-profile probe).
        // Arc sampling stays at the client's 20 chord steps (`spline.h:61`).
        let mut tt =
            Self::build_with_variant(nodes, move_speed, accel_rate, TimeMode::Vmangos, 20)?;
        // The cycle LENGTH self-pins to the real client's own bookkeeping (`transport_period`,
        // gold-gated bit-exact against all nine server-sniff periods). The wire anchor is a raw
        // server-uptime-scale clock, so `% period` amplifies
        // any Δms by the whole cycle count; vmangos pins its DB periods to sniffs of the same
        // client computation, so this IS the server's period — no consumer-side table needed.
        if let Some(period) =
            crate::transport_period::client_period_ms(nodes, move_speed, accel_rate)
        {
            tt.override_period(period);
        }
        Some(tt)
    }

    /// **The cycle instant this path first stands on `map_id`, searching forward from
    /// `from_cycle_ms` and wrapping** — the client-side answer to "the server says this transport
    /// has just crossed onto that continent; where in my own timetable is that?"
    ///
    /// A client computes a transport's position from its own path clock, anchored whenever the
    /// object's CREATE block last arrived, so that clock is free to drift from the server's about
    /// *when* a crossing happens (measured live at 32–144 ms on the 1.12 fleet). The server does
    /// re-anchor it on arrival — `Map::SendInitSelf` sends the ridden transport's create first,
    /// which is why `SendInitTransports` then skips it — but that lands a round trip after the
    /// worldport ack, and a consumer that must place a rider *in the worldport frame* has nothing
    /// to place them through until it does. This is that answer: the instant our own timetable
    /// puts the transport on the map the server just named.
    ///
    /// Returns the frame's **arrival** instant (the start of its window), so the sample lands at
    /// the head of the destination leg rather than mid-travel. `None` if no keyframe lies on
    /// `map_id` at all — the caller has misidentified the transport.
    pub fn first_cycle_on_map(&self, from_cycle_ms: u32, map_id: u32) -> Option<u32> {
        let n = self.frames.len();
        if n == 0 {
            return None;
        }
        // The frame `from_cycle_ms` currently sits in — the scan starts there so a clock already
        // on the destination leg answers "right here" instead of a whole cycle away.
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

    /// Whether any of the cycle's keyframes lies on `map_id` — i.e. this transport exists on
    /// that map for part of its loop. The cross-map worldport (decision 0455) keeps such a
    /// transport alive through the map switch (its clock is one continuous domain over the
    /// whole loop; the per-frame tick already resolves which legs render on which map).
    pub fn touches_map(&self, map_id: u32) -> bool {
        self.frames.iter().any(|f| f.map_id == map_id)
    }

    /// The calibration entry point (not public API — see the module doc). Mirrors
    /// `TransportMgr::GenerateWaypoints` phase for phase; each phase is commented with its
    /// vmangos source line range.
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

        // Phase A: the map-change/teleport skip dance (TransportMgr.cpp:128-152). First/last raw
        // nodes are teleport-arrival cells, never visited (loop bound `1..n_raw-1`).
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
                // The whole-path orientation spline collapses to a central difference at t=0
                // (module doc) — using the RAW, unfiltered neighbors, regardless of whether they
                // themselves survive the skip dance.
                let (prev, nxt) = (nodes[i - 1].pos, nodes[i + 1].pos);
                let initial_orientation =
                    normalize_orientation((nxt[1] - prev[1]).atan2(nxt[0] - prev[0]) + PI);
                raws.push(RawFrame {
                    map_id: node.map_id,
                    pos: node.pos,
                    // The CLIENT tests the stop flag as a bitmask (`test byte[row+0x1c], 0x2`
                    // @0x5f4e37); vmangos's `IsStopFrame` uses exact
                    // `== 2`. Identical on the live data (flags ∈ {0, 2}) — we follow the client.
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
        // Last to first is always "teleport", even for closed paths (GenerateWaypoints, after
        // the main loop).
        raws.last_mut().unwrap().teleport = true;
        let n = raws.len();

        // Phase B: legs — a new leg starts right after every teleport=true frame (a teleport
        // frame is always the last member of its own leg, matching the vmangos `extra`-point
        // dance without needing to replicate its raw-pointer bookkeeping — see the module doc).
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

        // Phase D: DistFromPrev / NextDistFromPrev (TransportMgr.cpp:192-234) — arc length of the
        // segment ending at (resp. starting at) each frame, `0` for a leg's first member (resp.
        // teleport frames and the path's last frame).
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

        // firstStop / lastStop (TransportMgr.cpp:213-222).
        let mut first_stop: Option<usize> = None;
        let mut last_stop: Option<usize> = None;
        for (j, r) in raws.iter().enumerate() {
            if r.is_stop {
                first_stop.get_or_insert(j);
                last_stop = Some(j);
            }
        }
        let (first_stop, last_stop) = (first_stop.unwrap_or(0), last_stop.unwrap_or(0));

        // Phase E: DistSinceStop / DistUntilStop, the two modular walks (TransportMgr.cpp:237-256).
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

        // Phase F: per-frame TimeTo, the four-regime trapezoid (TransportMgr.cpp:260-284) — pure
        // kinematics, shared by both timing modes (only the Arrive/Departure accumulation, Phase
        // H, is mode-dependent — see the module doc).
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

        // Phase G: TimeFrom (TransportMgr.cpp:287-295).
        let mut time_from = vec![0.0f32; n];
        let mut segment_time = 0.0f32;
        for i in 0..n {
            let j = (i + last_stop) % n;
            if raws[j].is_stop || j == last_stop {
                segment_time = time_to[j];
            }
            time_from[j] = segment_time - time_to[j];
        }

        // Phase H: Arrive/Departure + period — the calibrated variant. `_arrive_time` (per-frame
        // arrival) isn't retained on [`Frame`]: `sample` only ever needs `departure_time` (a
        // window's start) and `next_arrive_time` (its end) — see the `Frame` doc.
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

    /// Pin the cycle length to the exact period — **the server's own move**
    /// (`TransportMgr.cpp:63-79`: vmangos overrides its computed `pathTime` with the DB-sniffed
    /// per-build client value, stretching the final keyframe's departure), mirrored here.
    /// [`Self::build`] applies it with the client-transcribed period ([`crate::transport_period`],
    /// bit-exact against all nine server-sniff values): the
    /// wire anchor is a *raw* server-uptime-scale clock, so a Δms period error is amplified by
    /// the whole `floor(progress/period)` cycle count at every `% period` — days of uptime turn
    /// a 20 ms mismatch into minutes of phase error.
    ///
    /// A longer pin parks the transport at its final keyframe for the extra slice (exactly
    /// vmangos's stretch); a shorter one truncates the final approach at wrap ([`Self::sample`]
    /// clamps into the last window). Both deltas are tens of ms — invisible either way.
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

    /// The transport's position/heading at `cycle_ms` (`progress % period_ms`) —
    /// `ShipTransport::Update` + `CalculateSegmentPos` (`Transport.cpp:283-380`), done
    /// statelessly (no persistent "current frame" — a fresh search every call).
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

        // The first frame whose window `[ArriveTime, NextArriveTime)` contains cycle_ms — the
        // union of its stop window `[Arrive, Depart)` and its outbound travel window `[Depart,
        // NextArrive)`.
        let idx = self
            .frames
            .iter()
            .position(|f| cycle_ms < f.next_arrive_time)
            .unwrap_or(n - 1);
        let frame = &self.frames[idx];

        if cycle_ms < frame.departure_time || frame.next_dist_from_prev <= 0.0 {
            // Waiting at a stop, the zero-width instant at a non-stop keyframe, or (defensively)
            // a teleport's zero-width travel window — teleports never have a real outbound
            // window (see the `next_dist_from_prev` doc on `Frame`).
            return TransportSample {
                map: frame.map_id,
                pos: frame.pos,
                heading: frame.initial_orientation,
                moving: false,
            };
        }

        // Moving toward the next frame — CalculateSegmentPos (Transport.cpp:353-380): distance
        // from the *nearer* stop, under the same four-regime trapezoid as the build-time TimeTo.
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
