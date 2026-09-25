//! `WOW_STALL="<ms>[,<every_s>[,<after_s>[,<frames>]]]"`: blocks the main loop for `<ms>` on each
//! of `<frames>` consecutive frames, every `<every_s>` seconds from `<after_s>`, reproducing a
//! backgrounded window, which a kept-visible probe window ([`super::ProbeFocusPlugin`]) never is.
//!
//! Losing focus alone costs nothing (bevy's default `WinitSettings::game()` keeps an unfocused
//! window at 60 Hz); a fully covered macOS window blocks ~1 s in `CAMetalLayer.nextDrawable` every
//! frame. A ten-second tab-away is therefore `WOW_STALL="1000,20,60,10"`, and `WOW_STALL="0"` is
//! the monitor alone. Distinct from `WOW_STALL_INJECT` (`crate::perf::stall`), one watchdog sleep.
//!
//! `STALL_HIT` prints three clocks: `wall_dt`, [`Instant`] across the main-schedule pass (the
//! clock `transport::Transport::cycle_ms` runs on); `real_dt`, `Time<Real>`, which under pipelined
//! rendering is stamped from an `Instant` the render world sends and lags a frame or two;
//! `virt_dt`, after the frame pacer and the 250 ms `max_delta` clamp (`clamped=1`).

use core::time::Duration;
use std::time::Instant;

use bevy::prelude::*;
use bevy::time::Virtual;
use bevy::window::WindowOccluded;

use super::ProbeClock;

/// Default seconds between bursts and before the first, clear of login and world entry.
const DEFAULT_EVERY_SECS: f32 = 20.0;
const DEFAULT_AFTER_SECS: f32 = 30.0;
/// How often the monitor prints its `STALL_WATCH` summary, seconds.
const WATCH_SECS: f32 = 5.0;

pub(crate) struct StallPlugin;

impl Plugin for StallPlugin {
    fn build(&self, app: &mut App) {
        let raw = std::env::var("WOW_STALL").unwrap_or_default();
        let mut parts = raw
            .split(',')
            .map(|s| s.trim().parse::<f32>().unwrap_or(-1.0));
        let ms = parts.next().filter(|v| *v >= 0.0).unwrap_or(0.0) as u64;
        let every = parts
            .next()
            .filter(|v| *v > 0.0)
            .unwrap_or(DEFAULT_EVERY_SECS);
        let after = parts
            .next()
            .filter(|v| *v >= 0.0)
            .unwrap_or(DEFAULT_AFTER_SECS);
        let burst = parts.next().filter(|v| *v >= 1.0).unwrap_or(1.0) as u32;
        info!(
            "stall: WOW_STALL armed — {ms} ms x{burst} consecutive frames every {every} s from \
             t={after} s ({})",
            if ms == 0 {
                "monitor only, nothing injected"
            } else {
                "injecting"
            },
        );
        app.insert_resource(Stall {
            ms,
            every,
            burst,
            next_stall: after,
            left: 0,
            next_watch: WATCH_SECS,
            pending: None,
            last: None,
            samples: Vec::new(),
            focused: None,
            occluded: false,
        })
        // `Last`: the sleep ends before the next `First` stamps the clocks, so it is that
        // frame's delta, as a throttled frame's is.
        .add_systems(Last, drive_stall);
    }
}

/// [`StallPlugin`] state.
#[derive(Resource)]
struct Stall {
    /// Milliseconds to block for; `0` is monitor only.
    ms: u64,
    /// Seconds between bursts.
    every: f32,
    /// Consecutive frames blocked per burst: one long frame is a hitch, several a tab-away.
    burst: u32,
    /// Wall-clock second of the next burst.
    next_stall: f32,
    /// Frames still to block in the burst in progress.
    left: u32,
    /// Wall-clock second of the next `STALL_WATCH` summary.
    next_watch: f32,
    /// Set on the frame that slept; reported on the frame that paid for it.
    pending: Option<u64>,
    /// End of the previous main-schedule pass, for the wall-clock frame delta.
    last: Option<Instant>,
    /// This window's true frame deltas, milliseconds.
    samples: Vec<f64>,
    /// Last seen window focus; `None` until the first read, which prints the state.
    focused: Option<bool>,
    /// Last seen occlusion, from `WindowOccluded`.
    occluded: bool,
}

impl Stall {
    /// The `p`-th percentile of a sorted, non-empty sample window.
    fn pct(sorted: &[f64], p: f64) -> f64 {
        sorted[((sorted.len() - 1) as f64 * p).round() as usize]
    }
}

/// Reports the frame that just ran, then decides whether to block the next one.
fn drive_stall(
    real: ProbeClock,
    virt: Res<Time<Virtual>>,
    mut stall: ResMut<Stall>,
    mut occlusions: MessageReader<WindowOccluded>,
    windows: Query<&Window>,
) {
    let now = real.elapsed_secs();
    let at = Instant::now();
    let wall_ms = stall
        .last
        .map_or(0.0, |p| at.duration_since(p).as_secs_f64() * 1000.0);
    stall.last = Some(at);
    stall.samples.push(wall_ms);

    // Window state, printed on transition: an occluded run measures the OS throttle.
    for o in occlusions.read() {
        if o.occluded != stall.occluded {
            stall.occluded = o.occluded;
            println!("STALL_WINDOW t={now:.2} occluded={}", u8::from(o.occluded));
        }
    }
    let focused = windows.iter().any(|w| w.focused);
    if stall.focused != Some(focused) {
        stall.focused = Some(focused);
        println!("STALL_WINDOW t={now:.2} focused={}", u8::from(focused));
    }

    // The frame that paid for the last injection.
    if let Some(injected) = stall.pending.take() {
        let real_ms = real.delta_secs_f64() * 1000.0;
        let virt_ms = virt.delta_secs_f64() * 1000.0;
        println!(
            "STALL_HIT  t={now:.2} injected={injected} wall_dt={wall_ms:.1} real_dt={real_ms:.1} \
             virt_dt={virt_ms:.1} clamped={}",
            u8::from(real_ms - virt_ms > 1.0),
        );
    }

    if now >= stall.next_watch {
        stall.next_watch = now + WATCH_SECS;
        let mut sorted = std::mem::take(&mut stall.samples);
        sorted.sort_by(f64::total_cmp);
        if let Some(&max) = sorted.last() {
            println!(
                "STALL_WATCH t={now:.2} frames={} p50={:.1} p99={:.1} max={max:.1} focused={} \
                 occluded={}",
                sorted.len(),
                Stall::pct(&sorted, 0.50),
                Stall::pct(&sorted, 0.99),
                u8::from(focused),
                u8::from(stall.occluded),
            );
        }
    }

    if stall.ms == 0 {
        return;
    }
    if stall.left == 0 && now >= stall.next_stall {
        stall.left = stall.burst;
        stall.next_stall = now + stall.every;
    }
    if stall.left > 0 {
        let n = stall.burst - stall.left + 1;
        println!(
            "STALL      t={now:.2} sleeping {} ms ({n}/{})",
            stall.ms, stall.burst
        );
        stall.left -= 1;
        std::thread::sleep(Duration::from_millis(stall.ms));
        stall.pending = Some(stall.ms);
    }
}
