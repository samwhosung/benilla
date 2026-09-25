//! The frame-cost meters the pill reads: a rolling window each of wall frame time and process
//! CPU per frame.
//!
//! **The law: while synced, wall frame time measures the display's present grant, not our
//! cost.** Only the CPU series measures work — it is the pill's headline. `wall` exists for the
//! dim fps anchor and the hitch log below, nothing else.
//!
//! This file used to carry a per-series spike latch (0610's burst detector) and the
//! median baselines it tested against; both left with their last surface — the pill's arrow
//! (1455, after 1454 removed the expanded panel). The standing stall record is the
//! `frame hitch` warn below plus the self-sampler ([`super::stall`]); burst-shaped analysis
//! belongs to the instruments (the journal, the probes, Tracy).

use std::collections::VecDeque;

use bevy::prelude::*;
use bevy::time::Real;

use super::clock::{main_thread_cpu_secs, process_cpu_secs};

/// Recent frames kept for the pill's windowed means (~5 s at 60 fps, ~2.5 s at 120).
pub(super) const SAMPLE_WINDOW: usize = 300;

/// Frame duration above which a frame is logged as a hitch (a stall the player feels). Far above
/// any present interval so it only fires on real stalls (load bursts), not normal jitter.
const HITCH_LOG_MS: f32 = 250.0;

/// A rolling window of one cost series, in milliseconds, capped at its own length.
#[derive(Clone)]
pub(super) struct Series {
    samples: VecDeque<f32>,
    cap: usize,
}

impl Series {
    fn new(cap: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(cap),
            cap,
        }
    }

    fn push(&mut self, ms: f32) {
        if self.samples.len() == self.cap {
            self.samples.pop_front();
        }
        self.samples.push_back(ms);
    }

    pub(super) fn len(&self) -> usize {
        self.samples.len()
    }

    /// Windowed mean. `None` on an empty window, so a caller can tell "no data" from "zero cost".
    pub(super) fn mean(&self) -> Option<f32> {
        (!self.samples.is_empty())
            .then(|| self.samples.iter().sum::<f32>() / self.samples.len() as f32)
    }
}

/// The per-frame cost meters.
///
/// `Clone` is for the HUD's 4 Hz snapshot (`PerfHud::maybe_refresh`) — a few hundred floats,
/// four times a second.
#[derive(Resource, Clone)]
pub(super) struct FrameStats {
    /// Wall frame interval. The grant while synced; our real cost only when uncapped.
    pub(super) wall: Series,
    /// Process CPU per frame, user+system across every thread (`getrusage`) — the campaign's
    /// currency and the meter the rail cannot fool.
    pub(super) cpu: Series,
    prev_cpu_secs: Option<f64>,
    /// The MAIN thread's CPU per frame — the part of the frame the player can feel: the wall
    /// the schedule runs on. Sampled pinned (`sample_frame_time`'s `NonSendMarker`).
    pub(super) main: Series,
    prev_main_secs: Option<f64>,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self {
            wall: Series::new(SAMPLE_WINDOW),
            cpu: Series::new(SAMPLE_WINDOW),
            main: Series::new(SAMPLE_WINDOW),
            prev_cpu_secs: None,
            prev_main_secs: None,
        }
    }
}

impl FrameStats {
    /// Windowed mean frames per second. A mean, and labelled as one — it is the number that
    /// cannot see cost, which is why the pill draws it dim and small, never as the
    /// headline.
    pub(super) fn fps(&self) -> f32 {
        match self.wall.mean() {
            Some(mean) if mean > 0.0 => 1000.0 / mean,
            _ => 0.0,
        }
    }

    /// Drive the meters from a sibling module's test, on the same path [`sample_frame_time`]
    /// takes minus the clocks. Each frame is `(wall_ms, cpu_ms)`; returns the clock it left off
    /// at. `hud`'s snapshot test needs a realistically-populated `FrameStats`.
    #[cfg(test)]
    pub(super) fn feed_frames(&mut self, frames: &[(f32, f32)], start_t: f32, dt: f32) -> f32 {
        let mut t = start_t;
        for &(wall, cpu) in frames {
            self.wall.push(wall);
            self.cpu.push(cpu);
            // The fixture's main-thread share: a fixed fraction of the sum, so a test can tell
            // the two apart without a second column.
            self.main.push(cpu * 0.5);
            t += dt;
        }
        t
    }
}

/// Sample both meters for this frame.
pub(super) fn sample_frame_time(
    // Pinned to the main thread: `main_thread_cpu_secs` reads the CALLING thread's clock.
    _pin: bevy::ecs::system::NonSendMarker,
    time: Res<Time<Real>>,
    mut stats: ResMut<FrameStats>,
) {
    let wall_ms = time.delta_secs() * 1000.0;
    stats.wall.push(wall_ms);

    let cpu = process_cpu_secs();
    if let (Some(prev), Some(n)) = (stats.prev_cpu_secs, cpu) {
        stats.cpu.push(((n - prev) * 1000.0) as f32);
    }
    stats.prev_cpu_secs = cpu;
    let main = main_thread_cpu_secs();
    if let (Some(prev), Some(n)) = (stats.prev_main_secs, main) {
        stats.main.push(((n - prev) * 1000.0) as f32);
    }
    stats.prev_main_secs = main;

    // Log hard hitches so a load freeze is attributable from the log alone (one big stall vs many
    // medium ones, and roughly when). The first frame's delta is the startup gap, not a hitch.
    if wall_ms > HITCH_LOG_MS && stats.wall.len() > 1 {
        warn!("frame hitch: {wall_ms:.0} ms (main thread blocked this long)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window is capped and the means are windowed: 300 quiet frames after 100 loud ones
    /// must read quiet, and fps must be the reciprocal of the windowed wall mean.
    #[test]
    fn the_windows_roll_and_the_means_are_windowed() {
        let mut s = FrameStats::default();
        let loud: Vec<_> = (0..100).map(|_| (33.2, 30.0)).collect();
        let calm: Vec<_> = (0..SAMPLE_WINDOW).map(|_| (16.6, 8.0)).collect();
        let mut t = s.feed_frames(&loud, 0.0, 1.0 / 30.0);
        t = s.feed_frames(&calm, t, 1.0 / 60.0);
        let _ = t;

        assert_eq!(s.wall.len(), SAMPLE_WINDOW, "the window is capped");
        let cpu = s.cpu.mean().expect("cpu has samples");
        assert!(
            (cpu - 8.0).abs() < 0.01,
            "the loud prefix must have rolled out, got {cpu}"
        );
        assert!(
            (s.fps() - 1000.0 / 16.6).abs() < 0.5,
            "fps is the windowed wall mean's reciprocal, got {}",
            s.fps()
        );
    }

    /// An empty cpu window is "no data", not "zero cost" — the pill prints `--` for it.
    #[test]
    fn no_data_is_not_zero_cost() {
        let s = FrameStats::default();
        assert!(s.cpu.mean().is_none());
        assert_eq!(s.fps(), 0.0);
    }
}
