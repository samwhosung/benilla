//! The camera's smoothed-scalar channel: one template the reference instantiates for pitch
//! (armer/inner `0x512830`/`0x512980`), pitch bias (`0x512a50`/`0x512ba0`), ground tilt
//! (`0x512490`/`0x5125e0`) and pivot height (`0x5126b0`/`0x512790`), stepped in one block of
//! `0x50f160`.
//!
//! The armer rewraps the live value into `[target − π, target + π]`, refuses a request when the
//! channel is already arming the same `{target, delay, factor}` or is within 0.001 of the target,
//! and otherwise sets `duration = |target − live| / rate · factor`. The step eases by the cosine
//! smoothstep `0x5b7bb0`, `a + (b − a)·(1 − cos πs)/2` with `s = elapsed / duration`, and lands on
//! the target at `s ≥ 1`. The duration is seconds, not a rate, and the rate is in the live value's
//! units: the angular channels convert their deg/s CVar with `π/180`, the height's is yd/s.
//!
//! Deviation: a delay holds the channel at `from`, because the reference's `0x512830` backs the
//! start time up by the delay, so `s` runs negative and the even cosine jumps the value away and
//! back, an unclamped divide rather than a behaviour. No default reaches it: every
//! `cameraTerrainTilt*` row's `Delay` is 0, and the pitch and bias arms pass 0.

/// The armer's "already there, already arming this" epsilon, 0.001 at `[0x801360]`, shared by all
/// four channels and the `0x5107f0`/`0x5106f0` displacement predicates.
pub(super) const CHANNEL_EPS: f32 = 0.001;

/// One `arm` request: the armer's four arguments plus the ground channel's duration clamp.
#[derive(Clone, Copy, Debug)]
pub(super) struct Arm {
    /// In the live value's units.
    pub(super) target: f32,
    /// Seconds held before the tween starts (the module's deviation).
    pub(super) delay: f32,
    pub(super) factor: f32,
    /// Live-value units per second: `cvar.to_radians()` for the angular channels, yd/s for height.
    pub(super) rate: f32,
    /// The ground channel's bound: after the arm, `0x50dd29` clamps `[cam+0x1ac]` between
    /// `Factor × cameraTerrainTiltTimeMin` and `Factor × cameraTerrainTiltTimeMax`.
    pub(super) duration: Option<(f32, f32)>,
}

impl Arm {
    /// No delay, factor 1 and no bound, as the pitch (`0x512830`) and bias (`0x512a50`) sites arm.
    pub(super) fn at(target: f32, rate: f32) -> Self {
        Self {
            target,
            delay: 0.0,
            factor: 1.0,
            rate,
            duration: None,
        }
    }
}

/// A tween in flight: the reference's `{start, target, startMs, duration}` plus its
/// `{delay, factor}` memo; `Some` is the armed bit.
#[derive(Clone, Copy, Debug)]
struct Flight {
    /// Seconds since the arm; `delay` is the hold in front of the tween.
    elapsed: f32,
    delay: f32,
    duration: f32,
    /// The armed `(delay, factor)` (`+0x1ec`/`+0x1e8`), so a repeated request is not a restart.
    memo: (f32, f32),
}

/// One smoothed camera scalar. `Default` is linear, for the pivot height in yards;
/// [`SmoothChannel::angular`] is the other three.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SmoothChannel {
    live: f32,
    from: f32,
    to: f32,
    flight: Option<Flight>,
    /// Whether the armer's `2π` shortest-path rewrap applies: radians only, never yards.
    wrap: bool,
}

/// What [`SmoothChannel::arm`] did: the reference's three-way return (`0x512a50` returns 1 for
/// already arming, 0 for already there).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Armed {
    Started,
    /// Already arming this exact request; nothing changed.
    Already,
    /// Already within [`CHANNEL_EPS`] of the target; nothing is in flight afterwards.
    AtRest,
}

impl SmoothChannel {
    /// A radians channel (pitch, bias, ground tilt), which the armer rewraps (`0x512abe`).
    pub(super) fn angular() -> Self {
        Self {
            wrap: true,
            ..Self::default()
        }
    }

    pub(super) fn live(&self) -> f32 {
        self.live
    }

    /// The reference's armed bit in `[cam+0x90]`.
    pub(super) fn in_flight(&self) -> bool {
        self.flight.is_some()
    }

    /// Sets the channel to `v` with nothing in flight, the reference's hard snap: the water band
    /// crossing writes the pitch's live `[cam+0xf4]` and target `[cam+0x1e0]` directly while
    /// `[cam+0x90] & 1` (`0x50ed13`), and a camera's first height arm snaps (`0x5127d4`).
    pub(super) fn snap(&mut self, v: f32) {
        self.live = v;
        self.from = v;
        self.to = v;
        self.flight = None;
    }

    /// The armer, in the reference's order.
    pub(super) fn arm(&mut self, arm: &Arm) -> Armed {
        // The rewrap is stored back, not only compared (`0x512abe`).
        if self.wrap {
            let two_pi = std::f32::consts::TAU;
            while self.live - arm.target > std::f32::consts::PI {
                self.live -= two_pi;
            }
            while arm.target - self.live > std::f32::consts::PI {
                self.live += two_pi;
            }
        }
        if let Some(f) = self.flight {
            if (self.to - arm.target).abs() < CHANNEL_EPS
                && (f.memo.0 - arm.delay).abs() < CHANNEL_EPS
                && (f.memo.1 - arm.factor).abs() < CHANNEL_EPS
            {
                return Armed::Already;
            }
        }
        let gap = (arm.target - self.live).abs();
        if gap < CHANNEL_EPS {
            // Park the target and disarm, so arming every frame equals arming on change.
            self.to = arm.target;
            self.flight = None;
            return Armed::AtRest;
        }
        let mut duration = gap / arm.rate.max(f32::EPSILON) * arm.factor;
        if let Some((lo, hi)) = arm.duration {
            duration = duration.clamp(lo, hi);
        }
        self.from = self.live;
        self.to = arm.target;
        self.flight = Some(Flight {
            elapsed: 0.0,
            delay: arm.delay,
            duration: duration.max(f32::EPSILON),
            memo: (arm.delay, arm.factor),
        });
        Armed::Started
    }

    /// Steps the tween and returns the live value (`0x50f160`'s block).
    pub(super) fn advance(&mut self, dt: f32) -> f32 {
        if let Some(f) = self.flight.as_mut() {
            f.elapsed += dt;
            let t = f.elapsed - f.delay;
            if t >= 0.0 {
                let s = t / f.duration;
                if s >= 1.0 {
                    self.live = self.to;
                    self.flight = None;
                } else {
                    let e = (1.0 - (std::f32::consts::PI * s).cos()) * 0.5;
                    self.live = self.from + (self.to - self.from) * e;
                }
            }
        }
        self.live
    }

    /// `(live, target)`, the columns of a `WOW_CAM_DUMP` line.
    pub(super) fn probe(&self) -> (f32, f32) {
        (self.live, self.to)
    }
}

/// Walks `f` from `from` to `to` in `step`s and asserts no step moves the output more than
/// `max_jump`: point tests inside each regime cannot see a cliff between regimes.
#[cfg(test)]
pub(super) fn assert_bounded_step(
    (from, to): (f32, f32),
    step: f32,
    max_jump: f32,
    mut f: impl FnMut(f32) -> f32,
) {
    let mut previous: Option<(f32, f32)> = None;
    let steps = ((to - from) / step).ceil() as i32;
    for i in 0..=steps {
        let x = (from + step * i as f32).min(to);
        let y = f(x);
        if let Some((px, py)) = previous {
            assert!(
                (y - py).abs() <= max_jump,
                "a step from {px} to {x} moved the output {py} -> {y} \
                 ({:+}), past the bound {max_jump}",
                y - py
            );
        }
        previous = Some((x, y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Step at 60 Hz for `secs` and return every live value it passed through.
    fn run(c: &mut SmoothChannel, secs: f32) -> Vec<f32> {
        let dt = 1.0 / 60.0;
        (0..(secs / dt).round() as usize)
            .map(|_| c.advance(dt))
            .collect()
    }

    #[test]
    fn the_duration_is_the_gap_over_the_rate_times_the_factor() {
        for (gap, rate, factor) in [
            (10.0_f32, 45.0_f32, 1.0_f32),
            (30.0, 90.0, 2.0),
            (1.0, 7.5, 1.0),
        ] {
            let mut c = SmoothChannel::default();
            let expected = gap / rate * factor;
            assert_eq!(
                c.arm(&Arm {
                    target: gap,
                    delay: 0.0,
                    factor,
                    rate,
                    duration: None
                }),
                Armed::Started
            );
            let frames = run(&mut c, expected * 2.0);
            let arrived = frames
                .iter()
                .position(|v| (v - gap).abs() < CHANNEL_EPS)
                .expect("arrives");
            assert!(
                (arrived as f32 / 60.0 - expected).abs() < 0.05,
                "|Δ|/rate·factor = {expected:.3}s, took {:.3}s",
                arrived as f32 / 60.0
            );
            assert!(!c.in_flight(), "and disarms on arrival");
        }
    }

    #[test]
    fn a_per_frame_re_arm_neither_restarts_nor_stretches_the_tween() {
        let mut once = SmoothChannel::default();
        let mut every = SmoothChannel::default();
        // Rate 0.5 over a gap of 1.0 = a 2 s tween, so the second below is spent mid-flight.
        let arm = Arm::at(1.0, 0.5);
        once.arm(&arm);
        every.arm(&arm);
        let dt = 1.0 / 60.0;
        for _ in 0..60 {
            assert_eq!(every.arm(&arm), Armed::Already);
            assert_eq!(every.advance(dt), once.advance(dt));
        }
        run(&mut once, 1.5);
        run(&mut every, 1.5);
        assert_eq!(every.arm(&arm), Armed::AtRest);
        assert!(!every.in_flight());
    }

    #[test]
    fn the_arm_rewraps_the_live_value_to_the_short_side() {
        let mut c = SmoothChannel::angular();
        c.snap(3.1);
        let target = -3.1_f32;
        c.arm(&Arm::at(target, 1.0));
        // The gap the tween covers is the short one (≈0.083), not 6.2.
        let dur = (target - (3.1 - std::f32::consts::TAU)).abs();
        let frames = run(&mut c, 1.0);
        assert!(
            (frames[frames.len() - 1] - target).abs() < CHANNEL_EPS,
            "arrives at the target"
        );
        assert!(dur < 0.1, "the short way is {dur}");
        // Nothing in between left the short arc `[3.1 − 2π, −3.1]`.
        let wrapped = 3.1 - std::f32::consts::TAU;
        assert!(frames
            .iter()
            .all(|v| *v >= wrapped - CHANNEL_EPS && *v <= target + CHANNEL_EPS));
        // A linear channel with the same numbers does not wrap: it travels the long way.
        let mut linear = SmoothChannel::default();
        linear.snap(3.1);
        linear.arm(&Arm::at(target, 1.0));
        assert!(run(&mut linear, 0.5).iter().any(|v| *v > 0.0));
    }

    /// The module's deviation, pinned against the reference's negative-`s` jump.
    #[test]
    fn a_delay_holds_the_channel_before_the_tween_rather_than_jumping_it() {
        let mut c = SmoothChannel::default();
        c.arm(&Arm {
            target: 1.0,
            delay: 0.5,
            factor: 1.0,
            rate: 1.0,
            duration: None,
        });
        let frames = run(&mut c, 0.45);
        assert!(frames.iter().all(|v| *v == 0.0), "held for the delay");
        let frames = run(&mut c, 1.1);
        assert!((frames[frames.len() - 1] - 1.0).abs() < CHANNEL_EPS);
    }

    #[test]
    fn a_duration_bound_clamps_the_tween_not_the_gap() {
        let mut c = SmoothChannel::default();
        c.arm(&Arm {
            target: 100.0,
            delay: 0.0,
            factor: 1.0,
            rate: 7.5,
            duration: Some((0.1, 0.5)),
        });
        let frames = run(&mut c, 0.6);
        let arrived = frames
            .iter()
            .position(|v| (v - 100.0).abs() < CHANNEL_EPS)
            .expect("arrives");
        assert!(
            (arrived as f32 / 60.0 - 0.5).abs() < 0.05,
            "|Δ|/rate would be 13.3 s; the bound caps it at 0.5"
        );
    }
}
