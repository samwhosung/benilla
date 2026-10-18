//! Frame pacing: the frame's time step follows the presentation cadence, not the CPU loop. Under
//! `PresentMode::Fifo` frames are shown on refresh boundaries, but `Time<Real>` is stamped where
//! the main thread waits on the render thread, so two frames shown 16.67 ms apart can measure
//! 26.4 and 8.0 ms, and every bone of a standing model jitters at once. When recent deltas look
//! vsync'd, each is snapped to a whole number of refresh periods and the remainder carried, so
//! elapsed time still matches the wall clock; otherwise (uncapped, a real stall) it passes through.
//!
//! The paced step is [`FrameStep`], the one the interface sees: 1.12.1 hands every `OnUpdate` the
//! loop's frame time unclamped (`0x420f70` → `0x442330` → `0x442350` → `0x764330` → `0x76b2c0`).
//! `Time<Virtual>` takes the same step under its 250 ms `max_delta`. `Time<Real>` stays unpaced
//! because probes read it and a stall must stay visible there. `WOW_NO_FRAME_PACE=1` turns pacing
//! off, and a manual `TimeUpdateStrategy` is never paced.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::time::{Real, TimeSystems, TimeUpdateStrategy, Virtual};

/// Recent raw deltas the refresh-period median is taken over, about a third of a second at 60 Hz.
const WINDOW: usize = 20;
/// A sample counts as "on cadence" when it is within this fraction of the median.
const TIGHT: f64 = 0.15;
/// Pacing engages only when at least this share of the window is on cadence.
const TIGHT_SHARE: f64 = 0.6;
/// The largest remainder, in periods, still worked off in whole refreshes; beyond it the frame is
/// a stall and passes through raw.
const CARRY_MAX: f64 = 1.5;
/// The largest multiple snapped to; a longer frame is a stall, not a mis-split.
const MAX_MULT: f64 = 4.0;

#[derive(Resource)]
pub(crate) struct FramePacer {
    /// Recent raw deltas (seconds), for the refresh-period median.
    hist: VecDeque<f64>,
    /// Time measured but not yet handed out (negative: handed out ahead of the wall clock), within
    /// [`CARRY_MAX`] periods. Every step is `raw + old carry − new carry`, so the paced clock is
    /// always the wall clock less this.
    carry: f64,
    /// What `Time<Virtual>::elapsed` is rewritten to; `None` until the first paced frame adopts
    /// Bevy's own, so the clock never jumps.
    total: Option<Duration>,
    /// `WOW_NO_FRAME_PACE=1`.
    off: bool,
}

impl Default for FramePacer {
    fn default() -> Self {
        Self {
            hist: VecDeque::with_capacity(WINDOW),
            carry: 0.0,
            total: None,
            off: std::env::var("WOW_NO_FRAME_PACE").as_deref() == Ok("1"),
        }
    }
}

impl FramePacer {
    /// The refresh period this stream looks like, or `None` when it does not look vsync'd.
    fn cadence(&self) -> Option<f64> {
        if self.hist.len() < WINDOW {
            return None;
        }
        let mut v: Vec<f64> = self.hist.iter().copied().collect();
        v.sort_by(f64::total_cmp);
        let period = v[v.len() / 2];
        if period <= 0.0 {
            return None;
        }
        let tight = v
            .iter()
            .filter(|d| (*d - period).abs() < TIGHT * period)
            .count();
        (tight as f64 >= TIGHT_SHARE * v.len() as f64).then_some(period)
    }

    /// Snaps `raw` to the presentation cadence, carrying the remainder; a stream that is not vsync'd
    /// and a stall pass through, with any time still owed added. One call is
    /// one presented frame, so the multiple is 1 until `raw` plus the carry reaches 1.5 periods,
    /// then a floor: rounding would pace the 26.4 ms long half of a mis-split pair (1.58 periods)
    /// as two frames. No time is created or lost: a carry the step cannot absorb waits.
    fn pace(&mut self, raw: f64) -> f64 {
        if self.hist.len() == WINDOW {
            self.hist.pop_front();
        }
        self.hist.push_back(raw);
        let want = raw + self.carry;
        let Some(period) = self.cadence() else {
            return self.pass(raw);
        };
        if want < 0.0 {
            // Ahead of the wall clock by more than this frame is worth: the frame passes raw and
            // the lead waits for a longer one.
            return raw;
        }
        let mult = if want >= 1.5 * period {
            (want / period).floor().min(MAX_MULT)
        } else {
            1.0
        };
        let snapped = mult * period;
        let carry = want - snapped;
        if carry.abs() > CARRY_MAX * period {
            // A real stall, not a mis-split: report it as it happened.
            return self.pass(raw);
        }
        self.carry = carry;
        snapped
    }

    /// An unpaced frame: `raw` plus any time still owed; a lead (negative carry) is kept, so the
    /// step never falls below what the frame measured.
    fn pass(&mut self, raw: f64) -> f64 {
        if self.carry > 0.0 {
            let owed = std::mem::take(&mut self.carry);
            raw + owed
        } else {
            raw
        }
    }

    /// Drops the carry, for a frame whose clock is not paced (pacing off, a manual clock).
    fn reset(&mut self) {
        self.carry = 0.0;
    }
}

/// The frame's time step as the display showed it: what the interface's `OnUpdate` receives as
/// `elapsed` and its `GetTime` advances by. 1.12.1 measures one frame time per loop tick, the
/// millisecond clock's difference times 0.001 with no clamp (`0x420f70`), sums it for the frame
/// pass (`0x442330`) and hands it to every frame's update (`0x442350` → `0x764330` → `0x76b2c0`),
/// so a stall passes whole. Here that is the pacer's step, unclamped, or `Time<Real>`'s with
/// pacing off or a manual clock. `at` and `elapsed` advance by exactly `delta` each frame.
#[derive(Resource, Clone, Copy, Debug)]
pub struct FrameStep {
    /// This frame's step.
    pub delta: Duration,
    /// The wall-clock instant the paced clock stands at: `Time<Real>::last_update` less the time
    /// the pacer still carries.
    pub at: Instant,
    /// The paced clock since `Time<Real>`'s first update: its `elapsed` less the carry.
    pub elapsed: Duration,
}

impl Default for FrameStep {
    fn default() -> Self {
        Self {
            delta: Duration::ZERO,
            at: Instant::now(),
            elapsed: Duration::ZERO,
        }
    }
}

impl FrameStep {
    /// The step for a frame whose `Time<Real>` reads (`last_update`, `elapsed`), with `step`
    /// handed out and `carry` seconds still held by the pacer.
    fn new(last_update: Instant, elapsed: Duration, step: f64, carry: f64) -> Self {
        let held = Duration::from_secs_f64(carry.abs());
        let (at, elapsed) = if carry >= 0.0 {
            (
                last_update.checked_sub(held).unwrap_or(last_update),
                elapsed.saturating_sub(held),
            )
        } else {
            (last_update + held, elapsed + held)
        };
        Self {
            delta: Duration::from_secs_f64(step.max(0.0)),
            at,
            elapsed,
        }
    }

    /// [`Self::elapsed`] in seconds, the base a `Time<Real>` stamp maps onto `GetTime` from.
    pub fn elapsed_secs_f64(&self) -> f64 {
        self.elapsed.as_secs_f64()
    }
}

/// Paces this frame's step into [`FrameStep`] and rewrites `Time<Virtual>` and the generic `Time`
/// to take it, under `Virtual`'s `max_delta`; runs right after Bevy's clock tick. Bevy has no delta
/// setter, so the clock is rebuilt with its context and wrap period, and two `advance_by` calls
/// place `elapsed` and `delta`. Paused or scaled virtual time keeps Bevy's own step, since
/// `Virtual`'s `effective_speed` is private; the interface's step is paced regardless.
pub(crate) fn pace_frame_time(
    real: Res<Time<Real>>,
    mut virt: ResMut<Time<Virtual>>,
    mut generic: ResMut<Time>,
    mut pacer: ResMut<FramePacer>,
    mut step: ResMut<FrameStep>,
    strategy: Option<Res<TimeUpdateStrategy>>,
) {
    let raw = real.delta().as_secs_f64();
    let last_update = real.last_update().unwrap_or_else(Instant::now);
    // A manual clock (the capture harness, deterministic tests) must reproduce frame for frame.
    let manual = !matches!(
        strategy.as_deref(),
        None | Some(TimeUpdateStrategy::Automatic)
    );
    if pacer.off || manual {
        pacer.reset();
        pacer.total = None;
        *step = FrameStep::new(last_update, real.elapsed(), raw, 0.0);
        return;
    }
    let paced = pacer.pace(raw);
    *step = FrameStep::new(last_update, real.elapsed(), paced, pacer.carry);
    if virt.is_paused() || virt.relative_speed_f64() != 1.0 {
        pacer.total = None;
        return;
    }
    let paced = step.delta.min(virt.max_delta());
    let total = pacer.total.unwrap_or(virt.elapsed()) + paced;
    pacer.total = Some(total);

    let ctx = *virt.context();
    let wrap = virt.wrap_period();
    let mut t = Time::<Virtual>::default();
    *t.context_mut() = ctx;
    t.set_wrap_period(wrap);
    t.advance_by(total.saturating_sub(paced));
    t.advance_by(paced);
    *virt = t;
    *generic = virt.as_generic();
}

/// `WOW_FIXED_DT`'s pending arm (see [`plugin`]).
#[derive(Resource)]
struct FixedDt {
    dt: Duration,
    at: std::time::Instant,
    armed: bool,
}

/// Installs the manual strategy once the wall clock passes the arm time, and logs it.
fn arm_fixed_dt(mut cfg: ResMut<FixedDt>, mut commands: Commands) {
    if cfg.armed || std::time::Instant::now() < cfg.at {
        return;
    }
    cfg.armed = true;
    commands.insert_resource(TimeUpdateStrategy::ManualDuration(cfg.dt));
    info!(
        "frame_pace: WOW_FIXED_DT armed — animation clock pinned to {:.4} ms/frame",
        cfg.dt.as_secs_f64() * 1000.0
    );
}

pub fn plugin(app: &mut App) {
    // `WOW_FIXED_DT=<ms>[,<arm_after_wall_seconds>]` (default 40 s) pins the animation step, so a
    // per-frame screenshot burst, whose PNG saves stall every frame, measures the render's
    // evenness and not the clock's; [`pace_frame_time`] stands down under it. The arm delay is
    // required: a manual strategy also drives `Time<Real>`, which probe triggers read, and during
    // an unthrottled load it would race far ahead of the wall before the character streams in.
    if let Some((ms, after)) = std::env::var("WOW_FIXED_DT").ok().and_then(|v| {
        let (ms, after) = match v.split_once(',') {
            Some((a, b)) => (a, b.trim().parse::<f64>().ok()?),
            None => (v.as_str(), 40.0),
        };
        Some((ms.trim().parse::<f64>().ok().filter(|m| *m > 0.0)?, after))
    }) {
        app.insert_resource(FixedDt {
            dt: Duration::from_secs_f64(ms / 1000.0),
            at: std::time::Instant::now() + Duration::from_secs_f64(after.max(0.0)),
            armed: false,
        })
        .add_systems(First, arm_fixed_dt.before(TimeSystems));
    }
    app.init_resource::<FramePacer>()
        .init_resource::<FrameStep>()
        .add_systems(First, pace_frame_time.after(TimeSystems));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pacer() -> FramePacer {
        FramePacer {
            off: false,
            ..Default::default()
        }
    }

    #[test]
    fn a_mis_split_pair_is_restored_to_the_cadence_the_display_showed() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        for _ in 0..WINDOW {
            p.pace(period);
        }
        // A measured split of two on-cadence frames.
        let a = p.pace(0.0264);
        let b = p.pace(0.0080);
        assert!(
            (a - period).abs() < 1.0e-6,
            "long half snapped to one period: {a}"
        );
        assert!(
            (b - period).abs() < 1.0e-6,
            "short half snapped to one period: {b}"
        );
        assert!(
            ((a + b) - (0.0264 + 0.0080)).abs() <= period,
            "no time invented or lost beyond one period of carry"
        );
    }

    #[test]
    fn snapping_redistributes_time_it_never_creates_or_destroys_it() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        let mut raw_total = 0.0;
        let mut paced_total = 0.0;
        // Long/short jitter around the cadence, the measured shape.
        for i in 0..1000 {
            let raw = match i % 4 {
                0 => period * 1.55,
                1 => period * 0.45,
                2 => period * 0.80,
                _ => period * 1.20,
            };
            raw_total += raw;
            paced_total += p.pace(raw);
        }
        assert!(
            (raw_total - paced_total).abs() < period,
            "drift {:.6}s over 1000 frames must stay under one period",
            raw_total - paced_total
        );
    }

    #[test]
    fn a_dropped_frame_is_paced_as_two_periods_not_one() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        for _ in 0..WINDOW {
            p.pace(period);
        }
        let d = p.pace(2.0 * period);
        assert!(
            (d - 2.0 * period).abs() < 1.0e-6,
            "a dropped frame keeps both refreshes: {d}"
        );
    }

    #[test]
    fn a_genuine_stall_passes_through_unpaced() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        for _ in 0..WINDOW {
            p.pace(period);
        }
        let stall = 1.82; // a measured streaming stall
        assert_eq!(p.pace(stall), stall, "a stall is reported as it happened");
    }

    const P: f64 = 1.0 / 60.0;

    /// A pacer that has seen a steady 60 Hz window.
    fn warm() -> FramePacer {
        let mut p = pacer();
        for _ in 0..WINDOW {
            p.pace(P);
        }
        p
    }

    /// A 60 Hz stream whose every third pair is mis-split at 26.4 ms: `i % 6` is 4 or 5 for the
    /// split halves, `short` the second's raw delta.
    fn split_stream(i: usize, short: f64) -> f64 {
        match i % 6 {
            4 => 0.0264,
            5 => short,
            _ => P,
        }
    }

    #[test]
    fn mis_split_pairs_on_a_60_hz_cadence_pace_as_even_steps_that_keep_the_wall_clock() {
        let mut p = warm();
        let (mut raw_sum, mut paced_sum) = (0.0, 0.0);
        for i in 0..600 {
            // The pair's halves sum to two refreshes, as the display showed them.
            let raw = split_stream(i, 2.0 * P - 0.0264);
            raw_sum += raw;
            let d = p.pace(raw);
            paced_sum += d;
            assert!(
                (d - P).abs() < 1.0e-9,
                "frame {i}: a step of {d} s, not one period"
            );
        }
        assert!(
            (raw_sum - paced_sum).abs() < 1.0e-9,
            "the paced clock ends on the wall clock: drift {} s",
            raw_sum - paced_sum
        );
    }

    #[test]
    fn the_paced_clock_is_the_wall_clock_less_the_carry() {
        // The measured 26.4 / 8.0 ms split, whose halves add up to more than two refreshes.
        let mut p = warm();
        let (mut raw_sum, mut paced_sum) = (0.0, 0.0);
        for i in 0..600 {
            let raw = split_stream(i, 0.0080);
            raw_sum += raw;
            let d = p.pace(raw);
            paced_sum += d;
            let periods = d / P;
            assert!(
                periods >= 1.0 - 1.0e-9 && (periods - periods.round()).abs() < 1.0e-9,
                "frame {i}: a step of {d} s is not whole refreshes"
            );
            assert!(
                (raw_sum - paced_sum - p.carry).abs() < 1.0e-9,
                "frame {i}: time created or lost"
            );
            assert!(
                p.carry.abs() <= CARRY_MAX * P,
                "frame {i}: carry {}",
                p.carry
            );
        }
    }

    #[test]
    fn a_stall_passes_through_with_the_time_still_owed() {
        let mut p = warm();
        // The long half of a split leaves most of a period owed.
        assert!((p.pace(0.0264) - P).abs() < 1.0e-9);
        let owed = p.carry;
        assert!(owed > 0.0, "the long half carries {owed}");
        let stall = 1.82;
        assert_eq!(
            p.pace(stall),
            stall + owed,
            "a stall passes whole, the debt settled"
        );
        assert_eq!(p.carry, 0.0);
    }

    /// One frame through [`pace_frame_time`] at the wall instant `t`, after Bevy's own update.
    fn frame(world: &mut World, t: Instant) -> (FrameStep, Duration) {
        use bevy::ecs::system::RunSystemOnce;
        let real = {
            let mut real = world.resource_mut::<Time<Real>>();
            real.update_with_instant(t);
            *real
        };
        let mut virt = *world.resource::<Time<Virtual>>();
        let mut generic = *world.resource::<Time>();
        bevy::time::update_virtual_time(&mut generic, &mut virt, &real);
        world.insert_resource(virt);
        world.insert_resource(generic);
        world.run_system_once(pace_frame_time).unwrap();
        (
            *world.resource::<FrameStep>(),
            world.resource::<Time<Virtual>>().delta(),
        )
    }

    fn clock_world(t0: Instant) -> World {
        let mut world = World::new();
        world.insert_resource(Time::<Real>::new(t0));
        world.init_resource::<Time<Virtual>>();
        world.init_resource::<Time>();
        world.insert_resource(pacer());
        world.init_resource::<FrameStep>();
        world
    }

    #[test]
    fn the_frame_step_moves_its_instant_and_clock_by_its_own_delta_and_keeps_a_stall_unclamped() {
        let t0 = Instant::now();
        let mut world = clock_world(t0);
        let mut t = t0;
        let mut prev = frame(&mut world, t).0;
        let secs = Duration::from_secs_f64;
        for i in 0..WINDOW + 60 {
            t += secs(if i < WINDOW {
                P
            } else {
                split_stream(i, 2.0 * P - 0.0264)
            });
            let (step, virt) = frame(&mut world, t);
            let close = |a: Duration, b: Duration| a.abs_diff(b) < Duration::from_micros(1);
            assert!(
                close(step.at - prev.at, step.delta),
                "frame {i}: the instant steps by delta"
            );
            assert!(
                close(step.elapsed - prev.elapsed, step.delta),
                "frame {i}: the clock steps by delta"
            );
            assert!(
                off(t, step.at).as_secs_f64() <= CARRY_MAX * P,
                "frame {i}: the paced instant strays from the wall"
            );
            if i >= WINDOW {
                assert!(
                    close(step.delta, secs(P)),
                    "frame {i}: an uneven step {:?}",
                    step.delta
                );
            }
            assert_eq!(
                virt, step.delta,
                "frame {i}: the virtual clock takes the same step"
            );
            prev = step;
        }
        t += secs(1.82);
        let (step, virt) = frame(&mut world, t);
        assert!(
            step.delta >= secs(1.82),
            "a stall reaches the interface whole: {:?}",
            step.delta
        );
        let max = world.resource::<Time<Virtual>>().max_delta();
        assert_eq!(virt, max, "only the virtual clock clamps");
        assert!(
            off(t, step.at) < Duration::from_micros(1),
            "and the clock is back on the wall"
        );
    }

    /// How far apart two instants are, either way round.
    fn off(a: Instant, b: Instant) -> Duration {
        if a >= b {
            a - b
        } else {
            b - a
        }
    }

    #[test]
    fn a_manual_clock_steps_the_interface_raw() {
        let t0 = Instant::now();
        let mut world = clock_world(t0);
        world.insert_resource(TimeUpdateStrategy::ManualDuration(secs_ms(10)));
        let mut t = t0;
        frame(&mut world, t);
        for i in 0..WINDOW + 10 {
            let raw = if i % 2 == 0 { secs_ms(26) } else { secs_ms(8) };
            t += raw;
            let (step, _) = frame(&mut world, t);
            assert_eq!(step.delta, raw, "frame {i}: a manual clock is never paced");
            assert_eq!(step.at, t);
        }
    }

    fn secs_ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn an_uncapped_stream_is_left_alone() {
        let mut p = pacer();
        // Frame times of a `PresentMode::AutoNoVsync` run.
        let raws = [0.004, 0.011, 0.0031, 0.019, 0.0072, 0.0155, 0.0028, 0.0093];
        for i in 0..WINDOW * 2 {
            let raw = raws[i % raws.len()];
            assert_eq!(p.pace(raw), raw, "no cadence ⇒ no snapping");
        }
    }
}
