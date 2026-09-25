//! The frame-cost meters the pill reads: a rolling window each of wall frame time and process
//! CPU per frame.
//! While synced, wall frame time measures the display's present grant, not our cost, so the CPU
//! series is the pill's headline and `wall` only feeds the dim fps and the hitch log.

use std::collections::VecDeque;

use bevy::prelude::*;
use bevy::time::Real;

use super::clock::{main_thread_cpu_secs, process_cpu_secs};

/// Recent frames kept for the pill's windowed means (~5 s at 60 fps, ~2.5 s at 120).
pub(super) const SAMPLE_WINDOW: usize = 300;

/// Frame duration above which a frame is logged as a hitch, far above any present interval.
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

    /// Windowed mean; `None` on an empty window tells "no data" from "zero cost".
    pub(super) fn mean(&self) -> Option<f32> {
        (!self.samples.is_empty())
            .then(|| self.samples.iter().sum::<f32>() / self.samples.len() as f32)
    }
}

/// The per-frame cost meters; `Clone` is for the HUD's 4 Hz snapshot.
#[derive(Resource, Clone)]
pub(super) struct FrameStats {
    /// Wall frame interval: the present grant while synced, our cost only when uncapped.
    pub(super) wall: Series,
    /// Process CPU per frame, user+system across every thread (`getrusage`).
    pub(super) cpu: Series,
    prev_cpu_secs: Option<f64>,
    /// The main thread's CPU per frame, sampled pinned to that thread.
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
    /// Windowed mean frames per second; it cannot see cost, so the pill draws it dim.
    pub(super) fn fps(&self) -> f32 {
        match self.wall.mean() {
            Some(mean) if mean > 0.0 => 1000.0 / mean,
            _ => 0.0,
        }
    }

    /// Feed `(wall_ms, cpu_ms)` frames without the clocks, for tests; returns the clock it left
    /// off at.
    #[cfg(test)]
    pub(super) fn feed_frames(&mut self, frames: &[(f32, f32)], start_t: f32, dt: f32) -> f32 {
        let mut t = start_t;
        for &(wall, cpu) in frames {
            self.wall.push(wall);
            self.cpu.push(cpu);
            // A fixed main-thread share, so a test can tell the two series apart.
            self.main.push(cpu * 0.5);
            t += dt;
        }
        t
    }
}

/// Sample both meters for this frame.
pub(super) fn sample_frame_time(
    // Pinned to the main thread: `main_thread_cpu_secs` reads the calling thread's clock.
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

    // The first frame's delta is the startup gap, not a hitch.
    if wall_ms > HITCH_LOG_MS && stats.wall.len() > 1 {
        warn!("frame hitch: {wall_ms:.0} ms (main thread blocked this long)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn no_data_is_not_zero_cost() {
        let s = FrameStats::default();
        assert!(s.cpu.mean().is_none());
        assert_eq!(s.fps(), 0.0);
    }
}
