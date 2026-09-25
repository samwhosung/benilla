//! **The main thread's CPU, inside the app's frame** — `First` to `Last`, per frame, so the
//! probe can subtract it from the thread's whole-frame CPU (`thr=[main:…]`) and name what is
//! left: the event loop, AppKit's redraw path, the pipelined-render handshake. The vsync tax
//! is +1.2 ms of main-thread user time per frame that no system grew and
//! no sampler placed; whether it is spent inside the schedules or around them is this split.
//!
//! Two pinned systems, one `clock_gettime` each — nothing a frame can feel.

use bevy::prelude::*;

use super::clock::main_thread_cpu_secs;

#[derive(Resource, Default)]
pub(crate) struct MainThreadSplit {
    at_first: Option<f64>,
    /// Sum of per-frame main-thread CPU inside `First..Last`, seconds, and the frames summed.
    pub inside_secs: f64,
    pub frames: u32,
}

impl MainThreadSplit {
    /// Restart the accumulation (the probe does this at its window's first frame).
    pub fn restart(&mut self) {
        self.inside_secs = 0.0;
        self.frames = 0;
    }

    /// Mean main-thread CPU inside the app's frame, ms per frame, over the accumulation.
    pub fn inside_ms(&self) -> Option<f64> {
        (self.frames > 0).then(|| self.inside_secs * 1000.0 / f64::from(self.frames))
    }
}

fn stamp_first(_pin: bevy::ecs::system::NonSendMarker, mut split: ResMut<MainThreadSplit>) {
    split.at_first = main_thread_cpu_secs();
}

fn stamp_last(_pin: bevy::ecs::system::NonSendMarker, mut split: ResMut<MainThreadSplit>) {
    if let (Some(t0), Some(t1)) = (split.at_first, main_thread_cpu_secs()) {
        split.inside_secs += t1 - t0;
        split.frames += 1;
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<MainThreadSplit>()
        .add_systems(First, stamp_first)
        .add_systems(Last, stamp_last);
}
