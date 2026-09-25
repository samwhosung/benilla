//! The **frame-stall injector** (`WOW_STALL="<ms>[,<every_s>[,<after_s>[,<frames>]]]"`) — the
//! instrument that makes "I tabbed away and came back to X" reproducible.
//!
//! A probe window cannot be tabbed away from: every scripted live probe asserts `AlwaysOnTop`
//! ([`super::ProbeFocusPlugin`]) precisely so the OS cannot throttle it. So the one
//! class of bug that only appears *because* the window went to the background — the client running
//! for seconds at ~1 fps and then resuming — had no reproduction at all. This is that reproduction:
//! it blocks the main loop for `<ms>` on each of `<frames>` consecutive frames, every `<every_s>`
//! seconds, starting at `<after_s>`. Nothing else.
//!
//! **What backgrounding actually does, and why this shape is the faithful one.** Two independent
//! mechanisms, and only the second one bites:
//!
//! - *winit* — we never set [`bevy::winit::WinitSettings`], so we get its `Default`, which is
//!   `WinitSettings::game()`: `focused_mode: Continuous`, `unfocused_mode:
//!   reactive_low_power(1/60 s)` (`bevy_winit-0.18.1/src/winit_config.rs:19-22`). An unfocused
//!   window therefore still targets **60 Hz**; losing focus on its own costs nothing.
//! - *macOS* — a window that is fully **covered** is throttled at the drawable: every
//!   `CAMetalLayer.nextDrawable` blocks about a second, so the run continues at ~1 fps for as long
//!   as it is covered (the director's correction in 0906 — "any covering
//!   window, not just the lock screen"; 1355 measured the signature as a `p99 ≈ 1020 ms`
//!   metronome). A ten-second glance at a terminal is therefore ~10 consecutive one-second frames,
//!   which is why `<frames>` exists: a *single* long frame is a hitch, and a hitch is not what
//!   backgrounding does.
//!
//! So the knob that reproduces a ten-second tab-away is `WOW_STALL="1000,20,60,10"`.
//!
//! **Not to be confused with `WOW_STALL_INJECT`** (`crate::perf::stall`),
//! whose name is one suffix away. That one fires **a single** sleep at a single instant, and it
//! exists to prove the stuck-main-thread *watchdog* fires — it is a test affordance for an
//! instrument, not a reproduction of a gameplay state. This one repeats, blocks `<frames>` in a
//! row, and carries the frame-delta/occlusion monitor, which is the whole difference: 1663 was
//! reproducible only with **consecutive** long frames (one alone loses the deck for a frame and
//! recovers; two drop the rider through it). The two should probably become one knob — flagged,
//! not done here.
//!
//! **`WOW_STALL="0"` injects nothing** and leaves only the monitor running — the "what is this
//! run's frame pacing actually doing" reader, which is how the premise above gets checked on a real
//! run instead of assumed. Every run prints, per window:
//!
//! ```text
//! STALL_WATCH t=35.0 frames=299 p50=16.7 p99=17.9 max=33.4 focused=1 occluded=0
//! STALL      t=35.0 sleeping 1000 ms (1/10)
//! STALL_HIT  t=36.0 injected=1000 wall_dt=1016.4 real_dt=16.7 virt_dt=16.7 clamped=0
//! ```
//!
//! **Three clocks on that line, and they are three different clocks — that is the point.**
//!
//! - `wall_dt` is [`Instant`] to [`Instant`] across the main-schedule pass: the true frame time,
//!   and the clock `transport::Transport::cycle_ms` runs on (`anchor.at.elapsed()`).
//! - `real_dt` is `Time<Real>`'s delta, which under pipelined rendering is **not** the wall clock:
//!   `time_system` stamps it from an `Instant` the *render world* sends over a `bounded(2)` channel
//!   (`bevy_time-0.18.1/src/lib.rs:152-171`, `bevy_render-0.18.1/src/renderer/mod.rs:113`), so it
//!   arrives a frame or two late — bevy's own source carries a `TODO: Figure out how to handle
//!   this when using pipelined rendering` right above it.
//! - `virt_dt` is what every consumer of `Res<Time>` integrates: `real_dt` after the frame pacer,
//!   clamped to `Time<Virtual>`'s stock 250 ms `max_delta`. `clamped=1` marks the frames where the
//!   clamp bit.
//!
//! A `focused=`/`occluded=` transition prints its own line the moment it lands.

use core::time::Duration;
use std::time::Instant;

use bevy::prelude::*;
use bevy::time::Virtual;
use bevy::window::WindowOccluded;

use super::ProbeClock;

/// Default seconds between stall bursts, and default seconds of run before the first one — long
/// enough that login, world entry and the first streaming burst are never what is being measured.
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
        // `Last`, so the block lands at the frame boundary: the sleep is over by the time the
        // next frame's `First` stamps the clocks, and it therefore shows up as that frame's
        // delta — which is the shape an unscheduled/throttled frame has.
        .add_systems(Last, drive_stall);
    }
}

/// [`StallPlugin`] state: the knob, the schedule, and the current window's samples.
#[derive(Resource)]
struct Stall {
    /// Milliseconds to block for, `0` = monitor only.
    ms: u64,
    /// Seconds between bursts.
    every: f32,
    /// Consecutive frames blocked per burst — a backgrounded window is ~1 fps for as long as it
    /// is covered, so one long frame is a hitch and `burst` long frames are a tab-away.
    burst: u32,
    /// Wall-clock second of the next burst.
    next_stall: f32,
    /// Frames still to block in the burst in progress.
    left: u32,
    /// Wall-clock second of the next `STALL_WATCH` summary.
    next_watch: f32,
    /// Set on the frame that slept; read (and reported) on the frame that paid for it.
    pending: Option<u64>,
    /// End of the previous main-schedule pass — the true wall-clock frame delta, which under
    /// pipelined rendering `Time<Real>` is not (see the module header).
    last: Option<Instant>,
    /// This window's true frame deltas, milliseconds.
    samples: Vec<f64>,
    /// Last seen window focus, `None` until the first read — so the first frame prints the state
    /// rather than a transition into it.
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

/// Report the frame that just ran, then decide whether to block the next one.
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

    // Window state, printed on transition — the premise check: a run that was occluded (or never
    // focused) was measuring the OS, not us.
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
