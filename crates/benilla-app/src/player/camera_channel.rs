//! **The camera's smoothed-scalar channel** — the one template the reference instantiates four
//! times, in one place instead of four.
//!
//! The finding this module exists to honour: the
//! camera's pitch, pitch-bias and ground-tilt channels (`0x512830`/`0x512980`,
//! `0x512a50`/`0x512ba0`, `0x512490`/`0x5125e0`) are **one compiler-emitted template** at three
//! sets of field offsets, and `0x5126b0`/`0x512790` — the pivot **height** channel this client
//! already had ([`super::camera::PivotGlide`]) — is a fourth. Their per-frame step is one block of
//! one function (`0x50f160`).
//!
//! | role | live | start | target | start ms | duration | armed bit | rate CVar |
//! |---|---|---|---|---|---|---|---|
//! | pitch | `+0xf4` | `+0x1e4` | `+0x1e0` | `+0x1d8` | `+0x1dc` | `0x2000000` | `cameraPitchSmoothSpeed` `45.0` |
//! | bias | `+0x104` | `+0x1fc` | `+0x1f8` | `+0x1f0` | `+0x1f4` | `0x8000000` | `cameraTargetSmoothSpeed` `90.0` |
//! | ground | `+0x108` | `+0x1b4` | `+0x1b0` | `+0x1a8` | `+0x1ac` | `0x10000000` | `cameraGroundSmoothSpeed` `7.5` |
//! | height | `+0xfc` | `+0x1cc` | `+0x1c8` | `+0x1c0` | `+0x1c4` | `0x20000000` | `cameraHeightSmoothSpeed` `1.2` |
//!
//! The armer's contract, byte-derived and identical at all four: shortest-path-rewrap the live
//! value into `[target − π, target + π]`; **refuse** the request if the channel is already arming
//! the same `{target, delay, factor}` within `0.001`, or if the live value is already within
//! `0.001` of the target; otherwise compute `duration = |target − live| / rate · factor` and hand
//! the tween to the inner. The step is `s = elapsed / duration`, `s ≥ 1` → `live = target`, else
//! the cosine smoothstep `0x5b7bb0` — `a + (b − a)·(1 − cos πs)/2`.
//!
//! **Two things a re-implementation gets wrong.** `+0x1dc`/`+0x1f4`/`+0x1ac`/`+0x1c4` are
//! **durations in seconds**, not rates — the consumer *divides* elapsed seconds by them; and the
//! rate is in the **live value's own units**, so the three angular channels convert their deg/s
//! CVar with `π/180` while the height channel's `1.2` is already yd/s. [`Arm::rate`] is therefore
//! stated in live-units/s and the conversion belongs to the caller, where the unit is known.
//!
//! **The delay is a HOLD here, and the reference's is not** — a named divergence. `0x512830` backs
//! the start time up by `delay` (`now − (int)(p2 · −1000)`), so `0x50f160`'s `s` goes *negative*
//! for that long; and because the profile is `cos(π·s)` — an even function — a negative `s`
//! evaluates the ease as though `|s|`, which makes the live value jump away from `from` at the
//! instant of arming and walk back. That is an unclamped divide, not an intent, and it is
//! unreachable at every default this client reads: `Delay` is `0.0` on all thirty rows of the
//! `cameraTerrainTilt<Style><State>` table, and both the bias and pitch arms pass `0` literally.
//! benilla holds at `from` until the delay elapses — [`super::camera::FollowRig`] already does
//! exactly that for the one family whose delay is genuinely nonzero (`Track`/`Fear` under Smart,
//! `0.4`), so this is the house reading, not a new one.

/// The channel's "already there / already arming this" epsilon — VERIFIED `0.001` (`[0x801360]`),
/// shared by all four instantiations and by the `0x5107f0`/`0x5106f0` displacement predicates.
pub(super) const CHANNEL_EPS: f32 = 0.001;

/// What one `arm` call asks for — the armer's four arguments plus the one caller-side clamp.
#[derive(Clone, Copy, Debug)]
pub(super) struct Arm {
    /// Where the channel should end up, in the live value's units.
    pub(super) target: f32,
    /// Dead time before the tween starts, seconds (the module doc's named divergence).
    pub(super) delay: f32,
    /// Stretches the duration: `duration = |Δ| / rate · factor`. `1.0` is "at the rate".
    pub(super) factor: f32,
    /// The channel's rate **in the live value's units per second** — `cvar.to_radians()` for the
    /// three angular channels, the raw yd/s for the height one.
    pub(super) rate: f32,
    /// The duration bound `(min, max)`, when the caller has one. Only the ground channel does:
    /// `0x50dd29` clamps `[cam+0x1ac]` to `[Factor × cameraTerrainTiltTimeMin, Factor × …TimeMax]`
    /// *after* the arm, which is why it is the armer's business and not the tween's.
    pub(super) duration: Option<(f32, f32)>,
}

impl Arm {
    /// An arm with no delay, no stretch and no duration bound — the shape both the pitch
    /// (`0x512830(target, 0, 1.0f, now)`) and bias (`0x512a50(cam, 0, 0, 1.0f, now)`) sites pass.
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

/// A tween in flight — the reference's `{start, target, startMs, duration}` quadruple plus the
/// `{p2, p3}` staleness memo, with the armed bit expressed as `Option<Flight>` rather than a bit.
#[derive(Clone, Copy, Debug)]
struct Flight {
    /// Seconds since the arm, and the dead time in front of the tween.
    elapsed: f32,
    delay: f32,
    /// The tween's own length, seconds — never a rate.
    duration: f32,
    /// What it was armed with (`+0x1ec`/`+0x1e8`), so a repeat asking for the same move is a
    /// no-op instead of restarting it from its own midpoint.
    memo: (f32, f32),
}

/// One smoothed scalar channel of the camera — see the module doc.
///
/// `Default` is the **linear** channel, because the one instantiation whose live value is not an
/// angle is the pivot height (yards) — see [`SmoothChannel::angular`].
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SmoothChannel {
    live: f32,
    from: f32,
    to: f32,
    flight: Option<Flight>,
    /// Does the armer's shortest-path rewrap apply? It is a **`2π` rewrap**, so it is meaningful
    /// only on the three channels whose live value is an angle in radians; running it on the
    /// height channel would fold yards around a circle.
    wrap: bool,
}

/// What an [`SmoothChannel::arm`] did — the reference's own three-way return, kept because the
/// callers branch on it (`0x512a50` returns 1 for "already arming this", 0 for "already there").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Armed {
    /// A tween was started.
    Started,
    /// The channel is already arming this exact request — nothing changed.
    Already,
    /// The live value is already within [`CHANNEL_EPS`] of the target — nothing changed, and
    /// nothing is in flight afterwards either.
    AtRest,
}

impl SmoothChannel {
    /// An **angular** channel — pitch, pitch-bias or ground tilt, whose live value is radians and
    /// whose armer therefore rewraps it into `[target − π, target + π]` (`0x512abe`).
    pub(super) fn angular() -> Self {
        Self {
            wrap: true,
            ..Self::default()
        }
    }

    /// The value the camera uses this frame.
    pub(super) fn live(&self) -> f32 {
        self.live
    }

    /// Is a tween in flight? (The reference's armed bit of `[cam+0x90]`.)
    pub(super) fn in_flight(&self) -> bool {
        self.flight.is_some()
    }

    /// Establish the channel at `v` with nothing in flight — the reference's **hard snap**, which
    /// is a real leg of two of the four: the water band-crossing writes `[cam+0xf4]` *and*
    /// `[cam+0x1e0]` directly when `[cam+0x90] & 1` (`0x50ed13`), and the height channel's first
    /// arm of a camera's life snaps (`0x5127d4`).
    pub(super) fn snap(&mut self, v: f32) {
        self.live = v;
        self.from = v;
        self.to = v;
        self.flight = None;
    }

    /// The armer (`0x512830`/`0x512a50`/`0x512490`/`0x5126b0`), in its own order.
    pub(super) fn arm(&mut self, arm: &Arm) -> Armed {
        // The shortest-path rewrap: the live value is brought into `[target − π, target + π]`
        // first, so a channel that has accumulated past a half-turn takes the short way round
        // (`0x512abe fst` — it is stored back, not merely used for the compare).
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
            // Already there: park the target and disarm. This is the steady-state path, and it is
            // what makes arming every frame identical to arming on change.
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

    /// Step whatever is in flight and return the live value — `0x50f160`'s block, shared by all
    /// four channels.
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
                    // `0x5b7bb0` — the cosine smoothstep, eased at both ends.
                    let e = (1.0 - (std::f32::consts::PI * s).cos()) * 0.5;
                    self.live = self.from + (self.to - self.from) * e;
                }
            }
        }
        self.live
    }

    /// `(live, target)` — what a `WOW_CAM_DUMP` line prints. A channel question is a *timing*
    /// question, and these two columns are how a trace answers it (method: timing is measured).
    pub(super) fn probe(&self) -> (f32, f32) {
        (self.live, self.to)
    }
}

/// **Sweep a regime switch and bound its step** — the check decision 2165 exists because this tree
/// did not have one.
///
/// A mechanism that switches between *regimes* — liquid bands, a slope staircase, a state ladder,
/// an eligibility gate — is not verified by point assertions inside each regime. Those are
/// structurally blind to a cliff *between* them, which is how a pivot corridor that moved the
/// camera's framing point 1.04 yd in one frame shipped behind five green tests, each correct.
/// Walk the parameter across every boundary instead and bound the step.
///
/// Where a jump is intended, `max_jump` is where its size gets **written down** — which is the
/// second reason to reach for this rather than eyeball a sweep: the bound is the claim.
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

    /// The template's duration law is `|Δ| / rate · factor`, and it is a **duration**, not a rate:
    /// the tween takes exactly that long and the value arrives only at its end.
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

    /// Arming every frame with a steady target is exactly arming once — the property the whole
    /// per-frame-arm design rests on. A memo that only compared the target would still restart the
    /// tween whenever the *factor* changed, so both are in the compare.
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
        // …and once it is there, the re-arm reports rest rather than starting a zero move.
        run(&mut once, 1.5);
        run(&mut every, 1.5);
        assert_eq!(every.arm(&arm), Armed::AtRest);
        assert!(!every.in_flight());
    }

    /// The rewrap takes the short way round: a channel sitting just under `+π` asked for a target
    /// just over `−π` travels the small gap, not the long one.
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
        // …and a LINEAR channel of the same numbers does not wrap: it travels the long way,
        // because yards are not radians.
        let mut linear = SmoothChannel::default();
        linear.snap(3.1);
        linear.arm(&Arm::at(target, 1.0));
        assert!(run(&mut linear, 0.5).iter().any(|v| *v > 0.0));
    }

    /// The delay holds at the start value and then runs the full tween — the module doc's named
    /// divergence, pinned so it cannot silently become the reference's negative-`s` jump.
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

    /// The ground channel's own clamp (`0x50dd29`): the duration, not the rate, is bounded.
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
