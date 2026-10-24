//! A ship leg's clock: the time of each span between stops (`0x5f9120`) and the distance a time
//! into a span covers (`0x5f8dc0`), both a constant acceleration `a` up to the cruise `v`. The x87
//! runs at 53 bits: each operation is an `f64`, each store an `f32`.

/// The speed curve's phase, the animation the ship plays (`0x5f8dc0`); a ship at a stop stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShipAnim {
    /// 0, at a stop.
    Stand,
    /// 0xa2, speeding up out of a stop.
    Accelerate,
    /// 0xa3, at speed.
    Cruise,
    /// 0xa4, slowing into a stop.
    Decelerate,
}

impl ShipAnim {
    /// The `AnimationData` id the reference plays (`0x5f49b0`).
    pub fn id(self) -> u32 {
        match self {
            Self::Stand => 0,
            Self::Accelerate => 0xa2,
            Self::Cruise => 0xa3,
            Self::Decelerate => 0xa4,
        }
    }
}

/// `__ftol` (`0x40a2b0`) as a caller reads `eax`: truncate to 64 bits, `i64::MIN` for NaN or out of
/// range, and keep the low dword.
fn ftol(v: f64) -> i32 {
    const TWO_63: f64 = (1u64 << 63) as f64;
    let t = v.trunc();
    let q = if (-TWO_63..TWO_63).contains(&t) {
        t as i64
    } else {
        i64::MIN
    };
    q as i32
}

/// Seconds to ms: `t·1000`, then `+ 0.5` above zero and `− 0.5` otherwise, NaN included, then
/// `__ftol`.
fn round_ms(t: f64) -> u32 {
    let ms = t * 1000.0;
    ftol(if ms > 0.0 { ms + 0.5 } else { ms - 0.5 }) as u32
}

/// `0x5f9120`'s ramp: `1/a` unrounded, `ta = (1/a)·v` stored, and `da = (v·0.5)·ta` kept unrounded
/// for the compares and stored for the formulas (`fst` at `0x5f91d8`, `0x5f92f0`).
struct Ramp {
    recip: f64,
    ta: f64,
    da_live: f64,
    da: f64,
}

impl Ramp {
    fn new(v: f32, a: f32) -> Self {
        let recip = 1.0 / f64::from(a);
        let ta = f64::from((recip * f64::from(v)) as f32);
        let da_live = (f64::from(v) * 0.5) * ta;
        Self {
            recip,
            ta,
            da_live,
            da: f64::from(da_live as f32),
        }
    }

    /// A span with a stop at one end only: `(seg − da)/v + ta` past the ramp, else
    /// `√(seg·(1/a)·2)`; the compare's `jp` sends NaN to the root.
    fn one_ended(&self, seg: f64, v: f32) -> f64 {
        if self.da_live < seg {
            (seg - self.da) / f64::from(v) + self.ta
        } else {
            (seg * self.recip + seg * self.recip).sqrt()
        }
    }
}

/// `0x5f91af`-`0x5f924f`: the ms from the stop at distance `last` (stored) to the stop at `at`
/// (`0x453300`'s unrounded sum). The leg's first span starts at speed and slows into the stop; a
/// later one leaves a stop and enters one: `(seg − 2·da)/v + 2·ta` when `da < seg/2`, else
/// `2·√(seg·(1/a))`.
pub(super) fn span_to_stop_ms(at: f64, last: f32, v: f32, a: f32, first: bool) -> u32 {
    let seg = at - f64::from(last);
    let ramp = Ramp::new(v, a);
    let t = if first {
        ramp.one_ended(seg, v)
    } else if ramp.da_live < seg * 0.5 {
        (seg - (ramp.da + ramp.da)) / f64::from(v) + (ramp.ta + ramp.ta)
    } else {
        let r = (seg * ramp.recip).sqrt();
        r + r
    };
    round_ms(t)
}

/// `0x5f92b8`-`0x5f9335`: the ms from the last stop at `last` to the leg's end at `length`, both
/// stored; a leg with no stop cruises it at `seg / v`.
pub(super) fn span_to_end_ms(length: f32, last: f32, v: f32, a: f32, no_stop: bool) -> u32 {
    let seg = f64::from(length) - f64::from(last);
    let t = if no_stop {
        seg / f64::from(v)
    } else {
        Ramp::new(v, a).one_ended(seg, v)
    };
    round_ms(t)
}

/// `0x5f8dc0(t, T, accelerates, decelerates)`: the distance `t` seconds into a span of `T` seconds
/// covers, unrounded, and its phase. `ta = v/a`; `r = min(T, ta)`, or `min(T/2, ta)` with both
/// ramps. Every `t ≤ x` test sends NaN to the lower form (`test ah,0x41; jne`).
pub(super) fn speed_curve(
    v: f32,
    a: f32,
    t: f32,
    span: f32,
    accelerates: bool,
    decelerates: bool,
) -> (f64, ShipAnim) {
    let (v, a, t, span) = (f64::from(v), f64::from(a), f64::from(t), f64::from(span));
    match (accelerates, decelerates) {
        (false, false) => (t * v, ShipAnim::Cruise),
        (false, true) => {
            let r = cut(v / a, span);
            let cruise = span - r;
            if not_above(t, cruise) {
                return (t * v, ShipAnim::Cruise);
            }
            let ra = r * a;
            let d = t - cruise;
            (
                cruise * v + ((ra + ra) - d * a) * 0.5 * d,
                ShipAnim::Decelerate,
            )
        }
        (true, false) => {
            let ta = v / a;
            if not_above(t, cut(ta, span)) {
                return (t * a * 0.5 * t, ShipAnim::Accelerate);
            }
            (v * 0.5 * ta + (t - ta) * v, ShipAnim::Cruise)
        }
        (true, true) => {
            // 0x5f8ddf, 0x5f8dff: ta and T − r are stored.
            let ta = f64::from((v / a) as f32);
            let half = span * 0.5;
            // `test ah,5; jnp` keeps T/2 only when it is below ta; equal and NaN take ta.
            let r = if half < ta { half } else { ta };
            let slow_from = f64::from((span - r) as f32);
            if not_above(t, slow_from) {
                if not_above(t, r) {
                    return (t * a * 0.5 * t, ShipAnim::Accelerate);
                }
                return ((t - ta) * v + v * 0.5 * ta, ShipAnim::Cruise);
            }
            let ra = r * a;
            let d = t - slow_from;
            let slowing = ((ra + ra) - d * a) * 0.5 * d;
            let cruising = (span - (r + r)) * v;
            ((slowing + cruising) + ra * 0.5 * r, ShipAnim::Decelerate)
        }
    }
}

/// `x ≤ y` or unordered: the lower form of a `t` test (`test ah,0x41; jne`).
fn not_above(x: f64, y: f64) -> bool {
    x <= y || x.is_nan() || y.is_nan()
}

/// The ramp cut to the span the way the one-ended forms do it (`fcom; jnp`): only a span below
/// the ramp, ordered, replaces it.
fn cut(ramp: f64, span: f64) -> f64 {
    if span < ramp {
        span
    } else {
        ramp
    }
}
