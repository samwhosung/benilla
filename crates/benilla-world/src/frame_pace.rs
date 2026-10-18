//! Frame pacing: the frame's time step follows the presentation cadence, not the CPU loop. Under
//! `PresentMode::Fifo` frames are shown on refresh boundaries, but `Time<Real>` is stamped where
//! the main thread waits on the render thread, so two frames shown 16.67 ms apart can measure
//! 26.4 and 8.0 ms, and every bone of a standing model jitters at once. When recent deltas look
//! vsync'd, each is snapped to a whole number of refresh periods and the remainder carried, so
//! elapsed time still matches the wall clock; otherwise (uncapped, a real stall) it passes through.
//! The refresh period is the display's, from the monitor under the window, so a frame really shown
//! two refreshes after the last keeps both; the deltas' median stands in only while it is unknown.
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
use bevy::window::{Monitor, PrimaryMonitor, PrimaryWindow};

/// Recent raw deltas the cadence tests read, about a third of a second at 60 Hz.
const WINDOW: usize = 20;
/// A sample counts as "on cadence" when it is within this fraction of a period of its mark.
const TIGHT: f64 = 0.15;
/// Pacing engages only when at least this share of the window is on cadence.
const TIGHT_SHARE: f64 = 0.6;
/// A frame counts one more refresh from this far past a whole number of them, with the display's
/// period known: a mis-split's long half (26.4 ms, 1.58 periods) stays one, and a frame shown two
/// refreshes later still counts two when measured up to a quarter period short. A measured median
/// carries its own error, so without the display's period the multiple is a plain floor.
const SNAP_BIAS: f64 = 0.25;
/// The largest remainder, in periods, still worked off in whole refreshes; beyond it the frame is
/// a stall and passes through raw.
const CARRY_MAX: f64 = 1.5;
/// The largest multiple snapped to; a longer frame is a stall, not a mis-split.
const MAX_MULT: f64 = 4.0;
/// The share of the carry each frame hands out besides its snapped step, so a median that sits a
/// little off the true refresh, or a lead left by a refresh-rate drop, drains instead of parking.
const BLEED: f64 = 0.05;

#[derive(Resource)]
pub(crate) struct FramePacer {
    /// Recent raw deltas (seconds), for the cadence tests.
    hist: VecDeque<f64>,
    /// The display's refresh period (seconds), from [`track_display`]; `None` while unknown.
    grid: Option<f64>,
    /// Time measured but not yet handed out (negative: handed out ahead of the wall clock), about
    /// [`CARRY_MAX`] periods at most and drained by [`BLEED`]. Every step is
    /// `raw + old carry − new carry`, so the paced clock is always the wall clock less this.
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
            grid: None,
            carry: 0.0,
            total: None,
            off: std::env::var("WOW_NO_FRAME_PACE").as_deref() == Ok("1"),
        }
    }
}

impl FramePacer {
    /// Takes the display's refresh period; a new one (the window moved to another display, or
    /// the rate is first known) starts the cadence history over, since its deltas were shown on
    /// the old refresh.
    fn set_grid(&mut self, grid: Option<f64>) {
        if self.grid != grid {
            self.grid = grid;
            self.hist.clear();
        }
    }

    /// The period this stream is shown at, or `None` when it does not look vsync'd.
    ///
    /// With the display's refresh known, the unit is the whole number of refreshes nearest the
    /// window's 40th percentile (a 60 fps stream on a 120 Hz display is shown every second
    /// refresh), and the stream is vsync'd when most deltas sit on its multiples, or, for a run of
    /// mis-splits, when most adjacent pairs add up to two of it. A frame of one refresh and one of
    /// two alternating keep the unit one refresh, since the percentile sits below their median.
    /// An uncapped or variable-refresh stream, shown whenever a frame is ready and the display's
    /// rate only its ceiling, sits on neither and passes through. Unknown, the unit is the median
    /// of the deltas when most sit on it.
    fn unit(&self) -> Option<f64> {
        if self.hist.len() < WINDOW {
            return None;
        }
        let v: Vec<f64> = self.hist.iter().copied().collect();
        let Some(grid) = self.grid else {
            return on_cadence(&v);
        };
        let mut sorted = v.clone();
        sorted.sort_by(f64::total_cmp);
        let unit = grid * (sorted[v.len() * 2 / 5] / grid).round().max(1.0);
        let near = |d: f64, target: f64| (d - target).abs() < TIGHT * unit;
        let on_grid = v
            .iter()
            .filter(|&&d| {
                let k = (d / unit).round();
                k >= 1.0 && near(d, k * unit)
            })
            .count();
        let split_pairs = v
            .windows(2)
            .filter(|w| near(w[0] + w[1], 2.0 * unit))
            .count();
        let share = |n: usize, of: usize| n as f64 >= TIGHT_SHARE * of as f64;
        (share(on_grid, v.len()) || share(split_pairs, v.len() - 1)).then_some(unit)
    }

    /// Snaps `raw` to the presentation cadence, carrying the remainder; a stream that is not
    /// vsync'd passes through with any time still owed added, and a stall with the whole carry.
    /// One call is one presented frame, so it is at least one refresh: a frame shorter than the
    /// unit (a 120 fps stretch of a stream the window shows at 60) snaps on the display's own
    /// refresh. No time is created or lost, and every branch drains the carry by [`BLEED`].
    fn pace(&mut self, raw: f64) -> f64 {
        if self.hist.len() == WINDOW {
            self.hist.pop_front();
        }
        self.hist.push_back(raw);
        let want = raw + self.carry;
        let Some(unit) = self.unit() else {
            return self.pass(raw, want);
        };
        if want < 0.0 {
            // Ahead of the wall clock by more than this frame is worth: the frame passes raw and
            // the lead drains.
            return self.settle(want, raw);
        }
        let bias = if self.grid.is_some() { SNAP_BIAS } else { 0.0 };
        let mult = (want / unit + bias).floor().min(MAX_MULT);
        let snapped = if mult >= 1.0 {
            mult * unit
        } else {
            let refresh = self.grid.unwrap_or(unit);
            (want / refresh + bias).floor().max(1.0) * refresh
        };
        if (want - snapped).abs() > CARRY_MAX * unit {
            // A real stall, not a mis-split: report it as it happened, the carry settled with it.
            self.carry = 0.0;
            return want;
        }
        self.settle(want, snapped)
    }

    /// An unpaced frame: `raw` plus any time still owed; a lead (negative carry) drains instead,
    /// so the step never falls far below what the frame measured.
    fn pass(&mut self, raw: f64, want: f64) -> f64 {
        if self.carry > 0.0 {
            self.carry = 0.0;
            want
        } else {
            self.settle(want, raw)
        }
    }

    /// Hands out `ideal` plus [`BLEED`] of the carry it leaves, never below zero; `want` is the
    /// frame's raw delta plus the old carry.
    fn settle(&mut self, want: f64, ideal: f64) -> f64 {
        let carry = (want - ideal) * (1.0 - BLEED);
        let step = want - carry;
        if step < 0.0 {
            self.carry = want;
            return 0.0;
        }
        self.carry = carry;
        step
    }

    /// Drops the carry, for a frame whose clock is not paced (pacing off, a manual clock).
    fn reset(&mut self) {
        self.carry = 0.0;
    }
}

/// The refresh period of the display under the primary window: the monitor holding the window's
/// centre, else the primary monitor; `None` when neither reports a rate.
fn display_period(window: &Window, monitors: &[(&Monitor, bool)]) -> Option<f64> {
    let centre = match window.position {
        bevy::window::WindowPosition::At(p) => Some(
            p + IVec2::new(
                window.physical_width() as i32,
                window.physical_height() as i32,
            ) / 2,
        ),
        _ => None,
    };
    let under = centre.and_then(|c| {
        monitors.iter().find(|(m, _)| {
            let min = m.physical_position;
            let max = min + IVec2::new(m.physical_width as i32, m.physical_height as i32);
            c.x >= min.x && c.x < max.x && c.y >= min.y && c.y < max.y
        })
    });
    let monitor = under.or_else(|| monitors.iter().find(|(_, primary)| *primary))?;
    let mhz = monitor.0.refresh_rate_millihertz.filter(|&r| r > 0)?;
    Some(1000.0 / f64::from(mhz))
}

/// Follows the primary window to its display's refresh period, logging each change.
fn track_display(
    windows: Query<&Window, With<PrimaryWindow>>,
    monitors: Query<(&Monitor, Has<PrimaryMonitor>)>,
    mut pacer: ResMut<FramePacer>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let monitors: Vec<(&Monitor, bool)> = monitors.iter().collect();
    let grid = display_period(window, &monitors);
    if grid != pacer.grid {
        match grid {
            Some(p) => info!("frame_pace: display refresh {:.3} Hz", 1.0 / p),
            None => info!("frame_pace: display refresh unknown; pacing on the measured cadence"),
        }
        pacer.set_grid(grid);
    }
}

/// The median of `v` when at least [`TIGHT_SHARE`] of it sits within [`TIGHT`] of it.
fn on_cadence(v: &[f64]) -> Option<f64> {
    let mut v = v.to_vec();
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

    /// The paced clock before this frame's step, as (instant, seconds): where `GetTime` stands for
    /// a system that runs ahead of the UI's tick, which then adds [`Self::delta`].
    pub fn before_step(&self) -> (Instant, f64) {
        (
            self.at.checked_sub(self.delta).unwrap_or(self.at),
            self.elapsed.saturating_sub(self.delta).as_secs_f64(),
        )
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
        .add_systems(
            First,
            (track_display, pace_frame_time).chain().after(TimeSystems),
        );
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

    /// How far a paced step may sit off whole refreshes: [`BLEED`] of a mis-split's carry, where
    /// the raw halves sit 9.7 ms off.
    const EVEN: f64 = 1.0e-3;

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
            (a - period).abs() < EVEN,
            "long half snapped to one period: {a}"
        );
        assert!(
            (b - period).abs() < EVEN,
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
                (d - P).abs() < EVEN,
                "frame {i}: a step of {d} s, not one period"
            );
        }
        assert!(
            (raw_sum - paced_sum - p.carry).abs() < 1.0e-9 && p.carry.abs() < 0.011,
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
                periods >= 1.0 - EVEN / P && (periods - periods.round()).abs() < EVEN / P,
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

    /// Runs `raws` through a pacer, checking on every frame that no time is created or lost and
    /// that the carry stays within `bound`; returns the steps.
    fn replay(p: &mut FramePacer, raws: &[f64], bound: f64) -> Vec<f64> {
        let (mut raw_sum, mut paced_sum) = (0.0, 0.0);
        raws.iter()
            .enumerate()
            .map(|(i, &raw)| {
                let d = p.pace(raw);
                raw_sum += raw;
                paced_sum += d;
                assert!(
                    (raw_sum - paced_sum - p.carry).abs() < 1.0e-9,
                    "frame {i}: time created or lost"
                );
                assert!(p.carry.abs() <= bound, "frame {i}: carry {} s", p.carry);
                d
            })
            .collect()
    }

    /// A pacer that knows the display refreshes every `period` seconds.
    fn on_display(period: f64) -> FramePacer {
        let mut p = pacer();
        p.set_grid(Some(period));
        p
    }

    /// A small deterministic jitter, within ±`ms` milliseconds.
    fn jitter(i: usize, ms: f64) -> f64 {
        let x = ((i as u64).wrapping_mul(2_654_435_761) % 2001) as f64 / 1000.0 - 1.0;
        x * ms / 1000.0
    }

    /// 22 s of `Time<Real>` deltas from a 60 Hz run on Metal, in milliseconds. Its median sits off
    /// the true refresh, which a carry that never drains turns into a lead parked near -14.6 ms,
    /// from which a short-first split leaks raw. Paced on the display's 60 Hz, every step is even;
    /// on the median alone, time is still exact and the carry bounded.
    #[test]
    fn a_measured_stream_keeps_its_carry_drained_and_its_steps_even() {
        let raws: Vec<f64> = include_str!("../tests/data/frame_pace_stream.txt")
            .lines()
            .map(|l| l.trim().parse::<f64>().unwrap() / 1000.0)
            .collect();
        assert_eq!(raws.len(), 1319);
        let steps = replay(&mut on_display(P), &raws, 0.011);
        for (i, d) in steps.iter().enumerate().skip(WINDOW) {
            assert!((d - P).abs() < EVEN, "frame {i}: a step of {d} s");
        }
        replay(&mut pacer(), &raws, 0.011);
    }

    /// A frame shown one refresh after the last, then one shown two after, alternating (a
    /// GPU-bound 40 fps on a 60 Hz display): the steps are those intervals, not their 25 ms mean.
    #[test]
    fn a_real_one_and_two_refresh_alternation_steps_one_and_two_refreshes() {
        let truth: Vec<f64> = (0..400)
            .map(|i| if i % 2 == 0 { P } else { 2.0 * P })
            .collect();
        for ms in [0.0, 1.0] {
            let raws: Vec<f64> = truth
                .iter()
                .enumerate()
                .map(|(i, t)| t + jitter(i, ms))
                .collect();
            let steps = replay(&mut on_display(P), &raws, CARRY_MAX * P);
            for (i, (d, t)) in steps.iter().zip(&truth).enumerate().skip(WINDOW) {
                assert!(
                    (d - t).abs() < EVEN,
                    "±{ms} ms, frame {i}: a step of {d} s for a frame shown {t} s after the last"
                );
            }
        }
    }

    /// Five mis-split pairs in a row put half the window off the refresh; each pair still adds up
    /// to two refreshes, so the run stays paced, on a 60 Hz display and at 60 fps on a 120 Hz one.
    #[test]
    fn a_burst_of_mis_splits_stays_paced() {
        let mut raws = vec![P; 40];
        for _ in 0..5 {
            raws.extend([0.0264, 2.0 * P - 0.0264]);
        }
        raws.extend([P; 40]);
        for display in [P, P / 2.0] {
            let steps = replay(&mut on_display(display), &raws, CARRY_MAX * P);
            for (i, d) in steps.iter().enumerate().skip(WINDOW) {
                assert!(
                    (d - P).abs() < EVEN,
                    "{display} s display, frame {i}: a step of {d} s"
                );
            }
        }
    }

    /// Every frame mis-split on a 60 Hz display: halves that add up to two refreshes step one
    /// each; the measured 26.4 / 8.0 ms, which add up to more, step whole refreshes.
    #[test]
    fn an_every_frame_mis_split_on_a_known_display_steps_whole_refreshes() {
        let paired: Vec<f64> = (0..400)
            .map(|i| if i % 2 == 0 { 0.0264 } else { 2.0 * P - 0.0264 })
            .collect();
        let steps = replay(&mut on_display(P), &paired, CARRY_MAX * P);
        for (i, d) in steps.iter().enumerate().skip(WINDOW) {
            assert!((d - P).abs() < EVEN, "frame {i}: a step of {d} s");
        }
        let measured: Vec<f64> = (0..400)
            .map(|i| if i % 2 == 0 { 0.0264 } else { 0.0080 })
            .collect();
        let steps = replay(&mut on_display(P), &measured, CARRY_MAX * P);
        for (i, d) in steps.iter().enumerate().skip(WINDOW) {
            let periods = d / P;
            assert!(
                periods.round() >= 1.0 && (periods - periods.round()).abs() < EVEN / P,
                "frame {i}: a step of {d} s is not whole refreshes"
            );
        }
    }

    /// The window moves from a 60 Hz display to a 120 Hz one: once a window of frames shown on the
    /// new refresh is in, it paces on it, a mis-split at the new rate included, and no lead is
    /// left behind.
    #[test]
    fn a_move_to_a_faster_display_paces_on_its_refresh() {
        let fast = P / 2.0;
        let mut p = on_display(P);
        replay(&mut p, &[P; 200], CARRY_MAX * P);
        p.set_grid(Some(fast));
        let raws: Vec<f64> = (0..600)
            .map(|i| match i % 6 {
                4 => 0.0125,
                5 => 2.0 * fast - 0.0125,
                _ => fast,
            })
            .collect();
        let steps = replay(&mut p, &raws, CARRY_MAX * fast);
        for (i, d) in steps.iter().enumerate().skip(WINDOW) {
            assert!(
                (d - fast).abs() < EVEN / 2.0,
                "frame {i}: a step of {d} s at 120 Hz"
            );
        }
    }

    /// With the display's rate unknown, a 60 to 120 Hz rise snaps the first fast frames on the old
    /// median and leaves a lead; it drains, and a mis-split at the new rate is paced again.
    #[test]
    fn a_refresh_rate_rise_drains_its_lead_and_paces_the_new_cadence() {
        let fast = P / 2.0;
        let mut raws = vec![P; 200];
        raws.extend((0..600).map(|i| match i % 6 {
            4 => 0.0125,
            5 => 2.0 * fast - 0.0125,
            _ => fast,
        }));
        let mut p = pacer();
        let steps = replay(&mut p, &raws, CARRY_MAX * P);
        for (i, d) in steps.iter().enumerate().skip(400) {
            assert!(
                (d - fast).abs() < EVEN / 2.0,
                "frame {i}: a step of {d} s at 120 Hz"
            );
        }
        assert!(p.carry.abs() < EVEN, "the lead drained: {}", p.carry);
    }

    /// Uncapped frames, and a variable-refresh stream shown whenever a frame is ready under a
    /// 120 Hz ceiling, sit on no refresh grid and pass through raw.
    #[test]
    fn an_uncapped_or_variable_refresh_stream_passes_raw_on_a_known_display() {
        let uncapped = [0.004, 0.011, 0.0031, 0.019, 0.0072, 0.0155, 0.0028, 0.0093];
        for display in [P, P / 2.0] {
            let mut p = on_display(display);
            for i in 0..WINDOW * 4 {
                let raw = uncapped[i % uncapped.len()];
                assert_eq!(p.pace(raw), raw, "{display} s display, frame {i}");
            }
        }
        let mut p = on_display(P / 2.0);
        for i in 0..400 {
            let raw = 0.012 + jitter(i, 1.5);
            assert_eq!(p.pace(raw), raw, "variable refresh, frame {i}");
        }
    }

    fn monitor(x: i32, hz: u32) -> Monitor {
        Monitor {
            name: None,
            physical_height: 1080,
            physical_width: 1920,
            physical_position: IVec2::new(x, 0),
            refresh_rate_millihertz: Some(hz * 1000),
            scale_factor: 1.0,
            video_modes: Vec::new(),
        }
    }

    /// The period follows the window's centre from one display to the next, and falls back to the
    /// primary display while the window has no position.
    #[test]
    fn the_display_period_follows_the_window_to_its_monitor() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(pacer());
        world.spawn((monitor(0, 60), PrimaryMonitor));
        world.spawn(monitor(1920, 120));
        let window = world.spawn((Window::default(), PrimaryWindow)).id();
        let period = |world: &mut World| {
            world.run_system_once(track_display).unwrap();
            world.resource::<FramePacer>().grid
        };
        assert_eq!(
            period(&mut world),
            Some(1.0 / 60.0),
            "no position: the primary"
        );
        world.resource_mut::<FramePacer>().hist.push_back(P);
        world.get_mut::<Window>(window).unwrap().position =
            bevy::window::WindowPosition::At(IVec2::new(2400, 200));
        assert_eq!(
            period(&mut world),
            Some(1.0 / 120.0),
            "moved onto the 120 Hz display"
        );
        assert!(
            world.resource::<FramePacer>().hist.is_empty(),
            "the old display's deltas are dropped"
        );
    }

    #[test]
    fn a_stall_passes_through_with_the_time_still_owed() {
        let mut p = warm();
        // The long half of a split leaves most of a period owed.
        assert!((p.pace(0.0264) - P).abs() < EVEN);
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
                    (step.delta.as_secs_f64() - P).abs() < EVEN,
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
            (step.delta.as_secs_f64() - 1.82).abs() <= CARRY_MAX * P,
            "a stall reaches the interface whole, less only the carry: {:?}",
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
