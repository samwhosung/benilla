//! Background instrumented runs: a probe or capture window opens behind the user's work without
//! taking focus, then becomes an ordinary window.
//!
//! winit's AppKit backend activates the app at launch and bevy_winit 0.18 has no hook to stop it,
//! so `main` opens the window unfocused below the normal level and [`BgWinPlugin`] undoes the
//! launch, then restores the Normal level and Regular policy: left on, `AlwaysOnBottom` keeps the
//! window under every other and Accessory hides it from Cmd-Tab and the Dock.

use bevy::prelude::*;

/// Env prefixes that mark an instrumented run: only run-driving switches, never a modifier someone
/// sets on an attended run (`WOW_GM`, `WOW_FARCLIP`). A probe env none covers opens focused.
const BG_ENV_PREFIXES: &[&str] = &[
    "WOW_AUDIT_",
    "WOW_CAPTURE",
    "WOW_CHARCREATE_SHOT",
    "WOW_CHARSELECT_SHOT",
    "WOW_CREATE_TEST",
    "WOW_DEPTH",
    "WOW_FEED_GATE_CHECK",
    "WOW_FEED_GATE_TRACE",
    "WOW_FPS_",
    "WOW_GLUE_ROUNDTRIP",
    "WOW_LIVE_",
    "WOW_LOGIN_SHOT",
    "WOW_LOGIN_SMOKE",
    "WOW_LOGOUT_SMOKE",
    "WOW_MM_BLIP_PROBE",
    "WOW_MM_PROBE",
    "WOW_NODE_PROBE",
    "WOW_PARTICLE_CENSUS",
    "WOW_PHASE",
    "WOW_PICK",
    "WOW_PORTRAIT_TEST",
    "WOW_PROBE",
    "WOW_RIG",
    "WOW_SCHED_CENSUS",
    // Set by every unattended run (docs/METHOD.md): the one env that says nobody is at the desk.
    "WOW_UNATTENDED",
    "WOW_WORLDVIEW_",
];

/// Whether this is an instrumented background run: `WOW_BG=0` forces no (to watch a probe live),
/// any other `WOW_BG` forces yes, and otherwise any [`BG_ENV_PREFIXES`] env counts.
pub fn background_run() -> bool {
    match std::env::var("WOW_BG").as_deref() {
        Ok("0") => return false,
        Ok(_) => return true,
        Err(_) => {}
    }
    std::env::vars_os().any(|(name, _)| {
        name.to_str()
            .is_some_and(|name| BG_ENV_PREFIXES.iter().any(|p| name.starts_with(p)))
    })
}

/// Background envs whose run reads no pixels, so its window may be small; [`no_pixel_run`] needs
/// every background env a run sets to be here. A wrong entry shrinks a window being photographed,
/// and the bare `WOW_PROBE` prefix also matches any new `WOW_PROBE…` env that reads pixels.
const NO_PIXEL_ENV_PREFIXES: &[&str] = &[
    // In both lists: unattended runs pair it with a no-pixel `WOW_PROBE*` or `WOW_RIG`, which the
    // all-of rule would otherwise grow back to full size. A run that photographs names itself.
    "WOW_UNATTENDED",
    "WOW_FEED_GATE_CHECK",
    "WOW_FEED_GATE_TRACE",
    "WOW_FPS_",
    "WOW_LIVE_FPS",
    "WOW_PARTICLE_CENSUS",
    "WOW_PROBE",
    "WOW_RIG",
    "WOW_SCHED_CENSUS",
    // The engine boot check reads its log; `WOW_WORLDVIEW_SHOT` photographs and stays off.
    "WOW_WORLDVIEW_CHECK",
];

/// Whether this run reads no pixels, so its window may be small and parked. The app pins a probe
/// window `AlwaysOnTop` (an occluded macOS window throttles to about 1 fps), overriding this
/// module's ordering. `WOW_BG=1` with no run-driving env is not a no-pixel run.
pub fn no_pixel_run() -> bool {
    if !background_run() {
        return false;
    }
    let mut saw_one = false;
    for (name, _) in std::env::vars_os() {
        let Some(name) = name.to_str() else { continue };
        if !BG_ENV_PREFIXES.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        if !NO_PIXEL_ENV_PREFIXES.iter().any(|p| name.starts_with(p)) {
            return false;
        }
        saw_one = true;
    }
    saw_one
}

/// On a [`background_run`], undoes winit's launch-time activation and raise (macOS only).
pub struct BgWinPlugin;

impl Plugin for BgWinPlugin {
    fn build(&self, app: &mut App) {
        if !background_run() {
            return;
        }
        info!(
            "bgwin: instrumented run — window opens unfocused behind your work, then behaves \
             normally (Cmd-Tab/click to raise it; WOW_BG=0 for a normal focused window)"
        );
        #[cfg(target_os = "macos")]
        app.add_systems(PreStartup, macos::hand_back_activation)
            .add_systems(Update, macos::hold_background);
        #[cfg(not(target_os = "macos"))]
        let _ = app;
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use bevy::ecs::system::NonSendMarker;
    use bevy::prelude::*;
    use bevy::time::Real;
    use bevy::window::{PrimaryWindow, WindowLevel};
    use core::time::Duration;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;

    /// How long the app must stay inactive before [`hold_background`] lets go; winit's activation
    /// is a WindowServer round-trip that can land late, so each one seen restarts this clock.
    const SETTLE: Duration = Duration::from_millis(500);

    /// Hard ceiling on the hold, however unsettled the launch looks.
    const CEILING: Duration = Duration::from_secs(3);

    /// How long to watch after the promotion to Regular, which can itself activate the app.
    const TAIL: Duration = Duration::from_millis(300);

    /// Where the launch correction has got to, a one-way sequence.
    #[derive(Default)]
    pub(super) enum Phase {
        /// Undoing winit's activation and raise until the launch goes quiet.
        #[default]
        Holding,
        /// Quiet: the level is retargeted at Normal, and bevy's `setLevel` this frame puts the
        /// window at the front of that level.
        Promoting,
        /// Regular policy restored (Dock icon, Cmd-Tab); watching in case that activated the app.
        Tail(Duration),
        /// Hands off, permanently.
        Done,
    }

    /// Demotes the app to the Accessory policy and hands activation back, on the main thread
    /// (`NonSendMarker`, AppKit's requirement). Accessory is what holds the app back: on macOS 26
    /// `deactivate()` alone lets a Regular app take frontmost within 160 ms and keep it.
    pub(super) fn hand_back_activation(_main_thread: NonSendMarker) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let _ = hand_back(&app);
    }

    /// Hands activation back and orders the window back until the launch settles, then lets go.
    /// Timed against quiet, not window focus: winit's `window_activation_hack` makes every visible
    /// window key at launch whatever `focused` says, so focus gains come from the WindowServer too.
    pub(super) fn hold_background(
        _main_thread: NonSendMarker,
        time: Res<Time<Real>>,
        mut windows: Query<&mut Window, With<PrimaryWindow>>,
        mut span: Local<Option<(Duration, Duration)>>,
        mut phase: Local<Phase>,
    ) {
        if matches!(*phase, Phase::Done) {
            return;
        }
        let now = time.elapsed();
        let (started, last_active) = span.get_or_insert((now, now));
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);

        match *phase {
            Phase::Holding => {
                if now.saturating_sub(*last_active) < SETTLE
                    && now.saturating_sub(*started) < CEILING
                {
                    if hand_back(&app) {
                        // A late-granted activation landed: restart the settle clock.
                        *last_active = now;
                        debug!("bgwin: took activation back {now:?} in");
                    }
                    order_back(&app);
                    return;
                }
                // Quiet: release the level through the component, so bevy's cache stays true.
                let Ok(mut window) = windows.single_mut() else {
                    return;
                };
                window.window_level = WindowLevel::Normal;
                *phase = Phase::Promoting;
            }
            Phase::Promoting => {
                // bevy's `setLevel` last frame put us at the front of the normal level: order
                // back before restoring the Dock icon and Cmd-Tab entry.
                order_back(&app);
                app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
                *phase = Phase::Tail(now);
            }
            Phase::Tail(since) => {
                if hand_back(&app) {
                    debug!("bgwin: policy promotion activated the app; handed it back");
                }
                order_back(&app);
                if now.saturating_sub(since) >= TAIL {
                    *phase = Phase::Done;
                    info!(
                        "bgwin: launch settled — ordinary window now (Cmd-Tab or click to raise it)"
                    );
                }
            }
            Phase::Done => {}
        }
    }

    /// Deactivates the app if it is active; `true` when an activation had landed on us.
    fn hand_back(app: &NSApplication) -> bool {
        // SAFETY: main thread; every caller takes `NonSendMarker` and checks `MainThreadMarker`.
        unsafe {
            if !app.isActive() {
                return false;
            }
            app.deactivate();
            true
        }
    }

    /// Orders our windows to the back of the normal level, still on screen, where anything can
    /// raise them. Not gated on the app being active: winit raises the window at creation and in
    /// its activation hack, and either can land while we are inactive.
    fn order_back(app: &NSApplication) {
        // SAFETY: main thread; every caller takes `NonSendMarker` and checks `MainThreadMarker`.
        unsafe {
            for window in app.windows().iter() {
                window.orderBack(None);
            }
        }
    }
}
