//! The probe **run's own shell** — everything about the process and its window rather than
//! about what is being measured: the un-occludable, parked window ([`ProbeFocusPlugin`]), the
//! bounded lifetime ([`ProbeExitPlugin`]), and the mid-run resize ([`ProbeResizePlugin`]).
//! Every scripted probe rides these, whatever instrument it is running.

use bevy::prelude::*;

use super::ProbeClock;

/// Keep a probe run's window **un-occludable, and out of the director's way** — the one defence
/// against macOS's ~1 fps throttle for a fully covered window (docs/METHOD.md's
/// `caffeinate` note), at the smallest footprint that still buys it.
///
/// The on-top half used to live inside the FPS probe alone, which reads as "a frame-rate concern".
/// It is not: **every** scripted probe schedule is wall-clock ([`ProbeClock`]), so a throttled run
/// doesn't just measure slowly, it *executes the wrong script* — one session's mounted-jump run
/// fired `W@16` and `Space@19` in the SAME frame at ~1 fps, i.e. it jumped from a standstill
/// instead of mid-run, and the leg had to be re-read to notice. Any probe env arms
/// it, so a key/chat/Lua probe defends itself exactly like the FPS one.
///
/// **The parking half is the other side of that bill**. Asserting `AlwaysOnTop`
/// every frame silently defeats [`benilla_world::bgwin`]'s whole design — it overrides the
/// `AlwaysOnBottom` birth cage *and* the `Normal` handed back at release — so an instrumented run
/// sits on top for its entire life however politely it was launched. At the full-size default that
/// is a screen-filling window over whatever the director is doing, and a session that fires six
/// probes fires six of them (which is how it got reported). The answer is not to drop the
/// assertion — the throttle is real — but to shrink what is being asserted:
/// [`benilla_world::bgwin::no_pixel_run`] sizes such a run at 640×360 and this parks it in the top-right
/// corner. A run that photographs pixels is excluded from both and keeps the full window.
///
/// Write-gated on both counts: re-marking `Window` every frame would re-apply its whole state
/// through winit, and the park is one-shot, latched once a monitor is actually readable (the
/// monitor entities do not exist on frame 1).
pub(crate) struct ProbeFocusPlugin;

impl Plugin for ProbeFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, keep_probe_window_on_top);
    }
}

/// Where a no-pixel probe window goes, and whether it is pinned on top — `WOW_PROBE_PARK`.
///
/// This is a **dial, not a preference**, and it exists because the thing it trades against is
/// recorded as unproven. `AlwaysOnTop` costs the director screen for every probe a session fires;
/// what it buys is immunity from the macOS occlusion throttle, whose evidence is decision 0713's
/// *"one suspect survives, unproven"* (stall magnitudes of 1026–1053 ms sitting on
/// `CAMetalLayer.nextDrawable`'s 1 s timeout) promoted to fact by 0777 and built on by 0906. The
/// probe line already stamps `occluded_frames=`, so each setting is measurable against the others
/// on the same pin — which is the only way to retire the on-top assertion honestly.
#[derive(Clone, Copy, PartialEq)]
enum Park {
    /// Top-right corner, `AlwaysOnTop`. Today's behaviour.
    Corner,
    /// Mostly past the right edge — a [`PARK_EDGE_SLIVER`]-wide strip left on screen, at the
    /// NORMAL level. macOS marks a window `NSWindowOcclusionStateVisible` if *any* part of it is
    /// visible, so a sliver should be enough to dodge the throttle at ~1% of the screen cost.
    Edge,
    /// Wholly past the right edge, NORMAL level. The best case if AppKit allows it — AppKit
    /// constrains ordinary titled windows back onto a screen (`constrainFrameRect:toScreen:`), so
    /// this may simply not stick; that is the assumption the dial exists to test.
    Off,
}

/// How much of an [`Park::Edge`] window stays on screen (logical px).
const PARK_EDGE_SLIVER: f32 = 32.0;

/// Gap (logical px) a [`Park::Corner`] window keeps from the screen's top and right edges — clear
/// of the menu bar, which a window at `y = 0` would sit under.
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
    // Only `Corner` needs the pin: the other two get out of the way by *position*, which is the
    // point of measuring them — an edge/offscreen window that never reports `occluded_frames` has
    // retired the assertion on evidence instead of preference.
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
    // `WindowPosition::At` is in PHYSICAL pixels while the window's own resolution is logical, so
    // the width has to be scaled before it is subtracted.
    let Some(m) = monitors.iter().next() else {
        return; // no monitor entity yet — try again next frame
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

/// The probe self-termination as its own plugin, registered whenever `WOW_PROBE_EXIT_AT` is set
/// — it used to ride inside [`super::ProbeLuaPlugin`], so a chat/key-only probe's exit knob silently
/// did nothing (the 0441 flourish probe hung past its window on exactly that).
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

/// The probe run's clean self-termination (`WOW_PROBE_EXIT_AT=<secs>`, off when unset): exit the
/// app after N wall seconds, so a scripted live probe (`WOW_PROBE_LUA`/`WOW_PROBE_CHAT`) is one
/// foreground command with a bounded lifetime — no external kill, no orphaned window (0437's
/// probe rounds prompted it; generic to every future live probe).
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
        // The hard backstop rides its own OS thread: the polite AppExit stops the Update
        // schedule, so an in-schedule backstop can never fire — exactly the hang it existed
        // for (a winit/net-thread teardown hang leaves a zombie client holding the account;
        // the 0451 probe reproduced it). A probe run has nothing to lose.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(5));
            warn!("probe-exit: still alive 5s after AppExit — hard exit");
            std::process::exit(0);
        });
    }
}

/// The window-resize probe (`WOW_PROBE_RESIZE="<secs>:<W>x<H>"`, logical units): resize the
/// primary window mid-run — the headless stand-in for a mac fullscreen toggle or a window drag,
/// so resize-reactive layout (the glue screens' rescale rebuild) is verifiable in one scripted
/// run: open, resize at `t`, shoot after (`WOW_LOGIN_SHOT_OUT` fires at 8 s).
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

/// [`ProbeResizePlugin`] state: the fire time, the target logical size, and the once-latch.
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
