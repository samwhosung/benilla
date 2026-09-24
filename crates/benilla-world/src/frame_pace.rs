//! Frame pacing: `Time<Virtual>` advances on the presentation cadence, not the CPU loop. Under
//! `PresentMode::Fifo` frames are shown on refresh boundaries, but `Time<Real>` is stamped where
//! the main thread waits on the render thread, so two frames shown 16.67 ms apart can measure
//! 26.4 and 8.0 ms, and every bone of a standing model jitters at once. When recent deltas look
//! vsync'd, each is snapped to a whole number of refresh periods and the remainder carried, so
//! elapsed time still matches the wall clock; otherwise (uncapped, a real stall) it passes through.
//!
//! `Time<Real>` stays unpaced because probes read it and a stall must stay visible there.
//! `WOW_NO_FRAME_PACE=1` turns pacing off, and a manual `TimeUpdateStrategy` is never paced.

use std::collections::VecDeque;
use std::time::Duration;

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
    /// Time measured but not yet handed out, within [`CARRY_MAX`] periods.
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

    /// Snaps `raw` to the presentation cadence, carrying the remainder, or returns it unchanged
    /// for a stream that is not vsync'd and for a stall. One call is one presented frame, so the
    /// multiple is 1 until `raw` plus the carry reaches 1.5 periods, then a floor: rounding would
    /// pace the 26.4 ms long half of a mis-split pair (1.58 periods) as two frames.
    fn pace(&mut self, raw: f64) -> f64 {
        if self.hist.len() == WINDOW {
            self.hist.pop_front();
        }
        self.hist.push_back(raw);
        let Some(period) = self.cadence() else {
            self.carry = 0.0;
            return raw;
        };
        let want = raw + self.carry;
        if want < 0.0 {
            // Ahead of the wall clock by more than this frame is worth: stop inventing time.
            self.carry = 0.0;
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
            self.carry = 0.0;
            return raw;
        }
        self.carry = carry;
        snapped
    }
}

/// Rewrites `Time<Virtual>` and the generic `Time` so this frame's delta is the paced one; runs
/// right after Bevy's clock tick. Bevy has no delta setter, so the clock is rebuilt with its
/// context and wrap period, and two `advance_by` calls place `elapsed` and `delta`. Paused or
/// scaled virtual time passes through, since `Virtual`'s `effective_speed` is private.
pub(crate) fn pace_virtual_time(
    real: Res<Time<Real>>,
    mut virt: ResMut<Time<Virtual>>,
    mut generic: ResMut<Time>,
    mut pacer: ResMut<FramePacer>,
    strategy: Option<Res<TimeUpdateStrategy>>,
) {
    // A manual clock (the capture harness, deterministic tests) must reproduce frame for frame.
    let manual = matches!(
        strategy.as_deref(),
        Some(TimeUpdateStrategy::ManualInstant(_) | TimeUpdateStrategy::ManualDuration(_))
    );
    if pacer.off || manual || virt.is_paused() || virt.relative_speed_f64() != 1.0 {
        pacer.total = None;
        pacer.carry = 0.0;
        return;
    }
    let paced =
        Duration::from_secs_f64(pacer.pace(real.delta().as_secs_f64())).min(virt.max_delta());
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
    // evenness and not the clock's; [`pace_virtual_time`] stands down under it. The arm delay is
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
        .add_systems(First, pace_virtual_time.after(TimeSystems));
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
