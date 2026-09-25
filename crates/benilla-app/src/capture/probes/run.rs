//! The probe run's shell: the un-occludable, parked window ([`ProbeFocusPlugin`]), the bounded
//! lifetime ([`ProbeExitPlugin`]) and the mid-run resize ([`ProbeResizePlugin`]).

use bevy::prelude::*;

use super::ProbeClock;

/// Keeps a probe window un-occludable: macOS throttles a fully covered window to ~1 fps, which
/// on the wall clock ([`ProbeClock`]) collapses scheduled steps into one frame. A no-pixel run
/// ([`benilla_world::bgwin::no_pixel_run`], 640×360) is also parked once, per [`Park`]; a run that
/// photographs pixels keeps the full window. Pinning overrides [`benilla_world::bgwin`]'s levels.
/// Writes only on change: marking `Window` changed re-applies its whole state through winit.
pub(crate) struct ProbeFocusPlugin;

impl Plugin for ProbeFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, keep_probe_window_on_top);
    }
}

/// `WOW_PROBE_PARK`: where a no-pixel probe window goes and whether it is pinned on top; compare
/// settings by the probe line's `occluded_frames=`.
#[derive(Clone, Copy, PartialEq)]
enum Park {
    /// Top-right corner, `AlwaysOnTop`; the default.
    Corner,
    /// A [`PARK_EDGE_SLIVER`]-wide strip on screen at the normal level: macOS counts a window
    /// visible if any part of it is.
    Edge,
    /// Wholly past the right edge, normal level; AppKit may constrain it back
    /// (`constrainFrameRect:toScreen:`).
    Off,
}

/// How much of an [`Park::Edge`] window stays on screen (logical px).
const PARK_EDGE_SLIVER: f32 = 32.0;

/// Gap (logical px) a [`Park::Corner`] window keeps from the top and right edges, clear of the
/// menu bar.
const PROBE_WINDOW_MARGIN: f32 = 36.0;

fn park_mode() -> Park {
    match std::env::var("WOW_PROBE_PARK").as_deref() {
        Ok("edge") => Park::Edge,
        Ok("off") => Park::Off,
        _ => Park::Corner,
    }
}

fn keep_probe_window_on_top(
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    monitors: Query<&bevy::window::Monitor>,
    mut parked: Local<bool>,
) {
    let Ok(mut w) = windows.single_mut() else {
        return;
    };
    let no_pixel = benilla_world::bgwin::no_pixel_run();
    let mode = park_mode();
    // A pixel run is always pinned; a no-pixel run only in `Corner`, the others dodge by position.
    let want_top = !no_pixel || mode == Park::Corner;
    let level = if want_top {
        bevy::window::WindowLevel::AlwaysOnTop
    } else {
        bevy::window::WindowLevel::Normal
    };
    if w.window_level != level {
        w.window_level = level;
    }
    if *parked || !no_pixel {
        return;
    }
    // `WindowPosition::At` is physical pixels, the resolution logical: scale the width.
    let Some(m) = monitors.iter().next() else {
        return; // no monitor entity yet (frame 1)
    };
    let scale = m.scale_factor as f32;
    let width = (w.resolution.width() * scale) as i32;
    let screen = m.physical_width as i32;
    let margin = (PROBE_WINDOW_MARGIN * scale) as i32;
    let pos = match mode {
        Park::Corner => IVec2::new((screen - width - margin).max(0), margin),
        Park::Edge => IVec2::new(screen - (PARK_EDGE_SLIVER * scale) as i32, margin),
        Park::Off => IVec2::new(screen + margin, margin),
    };
    w.position = bevy::window::WindowPosition::At(pos);
    *parked = true;
    info!(
        "probe window: {}×{} logical, parked at {pos:?} physical ({})",
        w.resolution.width(),
        w.resolution.height(),
        match mode {
            Park::Corner => "corner, AlwaysOnTop",
            Park::Edge => "edge sliver, normal level",
            Park::Off => "offscreen, normal level",
        }
    );
}

/// The probe self-termination, registered whenever `WOW_PROBE_EXIT_AT` is set.
pub(crate) struct ProbeExitPlugin;

impl Plugin for ProbeExitPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ProbeExit {
            at: std::env::var("WOW_PROBE_EXIT_AT")
                .ok()
                .and_then(|v| v.parse().ok()),
            fired: false,
        })
        .add_systems(Update, fire_probe_exit);
    }
}

/// `WOW_PROBE_EXIT_AT=<secs>`: exits the app after that many wall seconds; off when unset.
#[derive(Resource)]
struct ProbeExit {
    at: Option<f32>,
    fired: bool,
}

fn fire_probe_exit(
    mut probe: ResMut<ProbeExit>,
    time: ProbeClock,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(at) = probe.at else { return };
    if !probe.fired && time.elapsed_secs() >= at {
        info!("probe-exit: {at}s elapsed — exiting");
        probe.fired = true;
        exit.write(AppExit::Success);
        // The hard backstop runs on its own thread, since `AppExit` stops `Update`: a teardown
        // hang would otherwise leave a client holding the account.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(5));
            warn!("probe-exit: still alive 5s after AppExit — hard exit");
            std::process::exit(0);
        });
    }
}

/// `WOW_PROBE_RESIZE="<secs>:<W>x<H>"`: resizes the primary window to that logical size mid-run,
/// standing in for a fullscreen toggle or a window drag.
pub(crate) struct ProbeResizePlugin;

impl Plugin for ProbeResizePlugin {
    fn build(&self, app: &mut App) {
        let spec = std::env::var("WOW_PROBE_RESIZE").unwrap_or_default();
        let parsed = spec.split_once(':').and_then(|(t, wh)| {
            let (w, h) = wh.split_once('x')?;
            Some((t.parse().ok()?, w.parse().ok()?, h.parse().ok()?))
        });
        match parsed {
            Some((at, w, h)) => {
                app.insert_resource(ProbeResize {
                    at,
                    size: Vec2::new(w, h),
                    fired: false,
                })
                .add_systems(Update, fire_probe_resize);
            }
            None => warn!("WOW_PROBE_RESIZE: expected \"<secs>:<W>x<H>\", got {spec:?}"),
        }
    }
}

/// [`ProbeResizePlugin`] state; `size` is logical.
#[derive(Resource)]
struct ProbeResize {
    at: f32,
    size: Vec2,
    fired: bool,
}

fn fire_probe_resize(
    mut probe: ResMut<ProbeResize>,
    time: ProbeClock,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
) {
    if probe.fired || time.elapsed_secs() < probe.at {
        return;
    }
    probe.fired = true;
    if let Ok(mut w) = windows.single_mut() {
        w.resolution.set(probe.size.x, probe.size.y);
        info!(
            "probe-resize: window -> {}x{} logical",
            probe.size.x, probe.size.y
        );
    }
}
