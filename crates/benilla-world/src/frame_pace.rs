//! **Frame pacing** — the clock we animate on follows the *presentation* cadence, not the CPU loop.
//!
//! Under `PresentMode::Fifo` (our default — `video::present_mode`) a frame is displayed only on a
//! refresh boundary, so the interval the eye receives is always an exact multiple of the monitor's
//! period. The interval we *measure* is not: Bevy stamps `Time<Real>` once per main-schedule pass,
//! and with pipelined rendering the main thread's wait for the render thread is where vsync
//! back-pressure lands. That wait moves around, so two frames shown 16.67 ms apart can be measured
//! as 26.4 ms and 8.0 ms. Their sum is right; the split is not.
//!
//! Advancing an animation by that split samples the pose at instants the display never shows: the
//! model lurches ahead by the long delta, then barely moves on the short one. It is a rigid,
//! whole-skeleton motion — every bone at once — which is exactly how it was reported ("a lot of
//! jitter on the whole model", standing still, hands off the mouse), and why it is invisible until
//! you are zoomed close enough for a fraction of a millimetre to be a pixel.
//!
//! **Measured at the Elwynn guard pin** (`.go xyz -9481.53 76.10 56.57 0`, 3200×1800, camera
//! parked, no input), 1805 frames of steady state:
//!
//! - 77 frames (4.3%) had a delta more than 3 ms off the 16.663 ms median.
//! - **27 of them were long/short PAIRS summing to an exact multiple of the refresh** — (26.4, 8.0),
//!   (25.3, 8.0), (20.2, 13.2), (25.9, 7.1) — i.e. both frames were presented on cadence and only
//!   the timestamps were split wrong. Just 3 were genuine dropped frames.
//! - 91% of 10-frame windows summed to within 2 ms of 166.7 ms: presentation stayed locked.
//! - The median bone's Δ tracked `dt` proportionally (`Δ/dt` constant to three digits), so the pose
//!   is *correct for the time it is handed*. The time is what is wrong.
//! - Whole-skeleton events (up to 91 of 117 bones past a quarter-pixel in one frame) landed on
//!   those frames **95%** of the time: `P(twitch | bad delta) = 0.26` against `0.0006` otherwise.
//!
//! **What this does.** When the recent deltas look like a vsync cadence, the raw delta is snapped to
//! the nearest whole multiple of the estimated refresh period and the remainder is carried into the
//! next frame, so the long-run elapsed time still matches the wall clock exactly — no time is
//! invented and none is lost. When the cadence does not look like vsync (uncapped, a genuine hitch,
//! a load spike) the raw delta passes through untouched: pacing must never paper over a real stall,
//! only correct a mis-split one.
//!
//! **Why `Time<Virtual>` and not `Time<Real>`.** `Real` is the wall clock, and the probe harness is
//! required to read it (`capture::probes::ProbeClock`) precisely so a hitching run
//! cannot silently under-run a knob. Pacing it would hide the defect from the instrument that found
//! it. `Virtual` is Bevy's own *adjustable* game clock and is what `advance_animations`, the
//! particle/ribbon sims and `Time<Fixed>` all derive from — one correction there reaches every
//! animating consumer, which is the point: this is not a rig defect, it is a clock defect.
//!
//! `WOW_NO_FRAME_PACE=1` turns it off — the A/B lever, same shape as `WOW_NO_RIG_REBASE`. A
//! hand-driven clock (`TimeUpdateStrategy::Manual*` — the capture harness, the deterministic
//! tests) is never paced: those runs exist to reproduce frame-for-frame.

use std::collections::VecDeque;
use std::time::Duration;

use bevy::prelude::*;
use bevy::time::{Real, TimeSystems, TimeUpdateStrategy, Virtual};

/// How many recent raw deltas the refresh estimate is taken over — about a third of a second at
/// 60 Hz, long enough for a median to be stable and short enough to follow a refresh-rate change.
const WINDOW: usize = 20;
/// A sample counts as "on cadence" when it is within this fraction of the median.
const TIGHT: f64 = 0.15;
/// Engage only when at least this fraction of the window is on cadence — below it we are not
/// looking at a vsync'd stream and snapping would be a lie.
const TIGHT_SHARE: f64 = 0.6;
/// How far the unspent remainder may run before we stop trying to account for the frame in whole
/// refreshes and report it as it happened. This is what keeps a genuine stall visible.
const CARRY_MAX: f64 = 1.5;
/// The largest multiple worth snapping to: past four dropped frames it is a stall, not a mis-split.
const MAX_MULT: f64 = 4.0;

/// The pacer's running state: the cadence estimate, the unspent remainder, and the paced total.
#[derive(Resource)]
pub(crate) struct FramePacer {
    /// Recent raw deltas (seconds), for the refresh-period median.
    hist: VecDeque<f64>,
    /// Time measured but not yet handed out, so the long-run sum is exact. Bounded by one period.
    carry: f64,
    /// The paced elapsed total — what `Time<Virtual>::elapsed` is rewritten to. `None` until the
    /// first paced frame adopts whatever Bevy had, so the clock never jumps at start-up.
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

    /// Snap `raw` to the presentation cadence, carrying the remainder. Returns `raw` unchanged
    /// whenever snapping would not be honest.
    ///
    /// **One call to this is one presented frame, so the answer is one refresh period** unless
    /// frames were genuinely dropped — and that asymmetry is the whole rule. Rounding `want/period`
    /// to nearest instead gets the case this exists for exactly backwards: the measured long half
    /// of a mis-split pair reads 26.4 ms = 1.58 periods, rounds *up* to 2, and the frame that was
    /// really shown for one refresh is paced as two. So the multiple is taken as a FLOOR, and only
    /// once `want` has actually reached 1.5 periods — meaning the remainder says we are a whole
    /// frame behind — is anything above 1 considered.
    ///
    /// The remainder is carried, not discarded, so this only ever re-distributes time: over any
    /// run the paced total tracks the wall clock to within one period. When the remainder cannot be
    /// worked off in whole refreshes ([`CARRY_MAX`]) the frame is not a mis-split at all — it is a
    /// stall — and the raw delta passes through so the hitch stays visible.
    fn pace(&mut self, raw: f64) -> f64 {
        if self.hist.len() == WINDOW {
            self.hist.pop_front();
        }
        self.hist.push_back(raw);
        let Some(period) = self.cadence() else {
            self.carry = 0.0;
            return raw;
        };
        // Spend what the wall clock gave us plus whatever the last frame did not use.
        let want = raw + self.carry;
        if want < 0.0 {
            // We are ahead of the wall clock by more than this frame is worth: stop inventing.
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
            // Not a mis-split — a real stall. Report it as it happened.
            self.carry = 0.0;
            return raw;
        }
        self.carry = carry;
        snapped
    }
}

/// Rewrite `Time<Virtual>` (and the generic `Time` derived from it) so this frame's delta is the
/// paced one. Runs straight after Bevy's own clock tick, before anything reads a delta.
///
/// Bevy exposes no delta setter, so the clock is rebuilt: its context and wrap period are carried
/// over, then two `advance_by` calls place `elapsed` at the paced total with `delta` as the paced
/// step. Both are O(1) — `advance_by` is plain field arithmetic.
///
/// **Paused or time-scaled virtual time passes straight through**, untouched. `Virtual`'s cached
/// `effective_speed` is private, so a rebuilt clock cannot honour a scale correctly, and pacing a
/// clock that is deliberately not tracking wall time is meaningless anyway.
pub(crate) fn pace_virtual_time(
    real: Res<Time<Real>>,
    mut virt: ResMut<Time<Virtual>>,
    mut generic: ResMut<Time>,
    mut pacer: ResMut<FramePacer>,
    strategy: Option<Res<TimeUpdateStrategy>>,
) {
    // A hand-driven clock is never a presentation cadence. The capture harness and the
    // deterministic tests set `ManualInstant`/`ManualDuration` precisely so a run reproduces
    // frame-for-frame; snapping their steps to a median would make the output depend on the
    // machine, which is the one thing those captures exist to rule out. (With a *constant* manual
    // delta the snap happens to be identity anyway — this guard is so that stays true by
    // construction rather than by luck.)
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
    // First paced frame adopts Bevy's own elapsed, so nothing jumps when this engages.
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

/// `WOW_FIXED_DT`'s pending arm — see [`plugin`].
#[derive(Resource)]
struct FixedDt {
    dt: Duration,
    at: std::time::Instant,
    armed: bool,
}

/// Install the manual strategy once the wall clock passes the arm time, and say so in the log —
/// a run whose clock silently failed to pin would read exactly like a run that measured something.
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
    // **`WOW_FIXED_DT=<ms>` — the deterministic clock**, and it exists for one question that no
    // other instrument here can answer: *is the RENDERED motion even?*
    //
    // A per-frame screenshot burst is the only way to measure what the eye actually receives, and
    // saving a 3200x1800 PNG stalls the frame to tens of milliseconds — so the burst perturbs the
    // very timing it is trying to measure, and its frames sample the animation on a grid as ragged
    // as the capture. Pinning the advance makes the geometry side perfectly even BY CONSTRUCTION,
    // so any unevenness left in the captured pixels belongs to the render, not the clock. That is
    // the whole discrimination: a body whose bones all move sub-pixel per frame can be flawless in
    // world space and still step on screen, and only this pairing separates the two.
    //
    // It composes with the pacer rather than fighting it: `Time<Real>` drives `Time<Virtual>`, and
    // [`pace_virtual_time`] stands down the moment a manual strategy is set.
    // Spelled `<ms>[,<arm_after_wall_seconds>]`, default 40 s. **The delay is not a convenience,
    // it is required**: a manual strategy overrides `Time<Real>`, which is the clock every probe
    // trigger reads, and during load the app is not vsync-limited — it ran ~385 updates/second, so
    // a pinned 16.6667 ms clock reached "90 seconds" in 14 s of wall time and the run exited before
    // the character had finished streaming in. Arming on a real wall-clock `Instant` keeps the
    // pinning for the part of the run that is being measured and leaves login/teleport on real time.
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

    /// The whole point: a long/short PAIR that sums to two refresh periods comes back out as two
    /// equal periods — which is what the display actually showed — and nothing is invented.
    #[test]
    fn a_mis_split_pair_is_restored_to_the_cadence_the_display_showed() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        for _ in 0..WINDOW {
            p.pace(period);
        }
        // The measured split of two on-cadence frames: 26.4 ms then 8.0 ms (the real reading).
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
        // And the pair still spends exactly what the wall clock measured, to within the carry.
        assert!(
            ((a + b) - (0.0264 + 0.0080)).abs() <= period,
            "no time invented or lost beyond one period of carry"
        );
    }

    /// Long-run elapsed must track the wall clock exactly — the carry exists so that snapping is a
    /// re-distribution, never a drift. A thousand jittered frames may not gain or lose a frame.
    #[test]
    fn snapping_redistributes_time_it_never_creates_or_destroys_it() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        let mut raw_total = 0.0;
        let mut paced_total = 0.0;
        // A deterministic long/short jitter around the cadence, the shape the measurement showed.
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

    /// A genuinely dropped frame IS two periods — pacing must not flatten a real 30 Hz stretch
    /// into 60 Hz, or the clock would run slow for as long as the drop lasts.
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

    /// A real stall is NOT smoothed away: pacing corrects a mis-split, it does not hide a hitch.
    #[test]
    fn a_genuine_stall_passes_through_unpaced() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        for _ in 0..WINDOW {
            p.pace(period);
        }
        let stall = 1.82; // the 1820 ms streaming stall the same run caught
        assert_eq!(p.pace(stall), stall, "a stall is reported as it happened");
    }

    /// An uncapped (non-vsync) stream never engages: with no cadence there is nothing to snap to.
    #[test]
    fn an_uncapped_stream_is_left_alone() {
        let mut p = pacer();
        // Wildly varying frame times — a `PresentMode::AutoNoVsync` run.
        let raws = [0.004, 0.011, 0.0031, 0.019, 0.0072, 0.0155, 0.0028, 0.0093];
        for i in 0..WINDOW * 2 {
            let raw = raws[i % raws.len()];
            assert_eq!(p.pace(raw), raw, "no cadence ⇒ no snapping");
        }
    }
}
