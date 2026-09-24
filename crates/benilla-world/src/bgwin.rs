//! Background instrumented runs: probe/capture/regression windows that open *behind* the
//! director's work — and stay reachable once they're there.
//!
//! Agent-driven runs (captures, live probes, the rig, FPS journals) open a real window for
//! seconds to minutes on the same desktop the director is working on. Left alone, each launch
//! stole their focus twice over:
//!
//! - **Window-side** — the primary window opened focused (`makeKeyAndOrderFront`), swallowing
//!   whatever they were typing (on 2026-07-19 a login-shot run typed their keystrokes into the
//!   account box). `Window::focused = false` fixed that for capture/login-shot runs only; every
//!   live probe still opened key.
//! - **App-side (macOS)** — winit's AppKit backend unconditionally calls
//!   `activateIgnoringOtherApps` at `applicationDidFinishLaunching` and sets the Regular
//!   activation policy, so even an unfocused run activated the app: the menu bar switched and
//!   the window ordered in **front** of every other app's. bevy_winit 0.18 exposes no
//!   `EventLoopBuilderExtMacOS` hook to turn either off, so it can't be prevented — only undone,
//!   immediately after launch.
//!
//! Stealing the screen is a **launch-time event**, and only an event's worth of correction is
//! owed. 0703 paid for it with two *permanent* states, and the bill came due: with
//! `WindowLevel::AlwaysOnBottom` (`kCGNormalWindowLevel - 1`) the window could never be raised
//! above a normal one however hard you clicked it, and the Accessory activation policy struck it
//! from Cmd-Tab and the Dock. Every instrumented window was unreachable for its whole life, not
//! just polite at birth. Decision 0709 keeps both mechanisms — they are the right ones, and
//! Accessory in particular is doing more work than it looks (see [`macos::hand_back_activation`])
//! — but scopes them to the launch instead of the run:
//!
//! [`background_run`] detects an instrumented run from the env that drives it (no recipe or
//! script changes anywhere). `main` opens the window unfocused and below the normal level, so it
//! cannot flash over the director's work on the way up. [`BgWinPlugin`] then holds the launch:
//! Accessory policy, activation handed straight back to whatever they were using, and our window
//! ordered to the **back of the stack** — on screen, so surface rendering, readback and
//! screenshots are untouched, but behind their work. Once the launch goes quiet (half a second
//! with no activation landing on us; hard-capped at three seconds) it lets go in sequence —
//! window level back to Normal, policy back to Regular, one last order-to-back — and latches
//! shut. What's left is an ordinary window that Cmd-Tabs, clicks and raises like any other, for
//! as long as the run lasts.

use bevy::prelude::*;

/// Env prefixes that mark a run as agent-driven — the capture harness, shots/smokes, and the
/// live-probe fleet. Deliberately only *run-driving* switches: a plain modifier the director
/// might set on an attended run (`WOW_GM`, `WOW_FARCLIP`, traces…) must NOT push their own
/// window to the bottom of the stack. A new probe env usually starts with `WOW_PROBE`/
/// `WOW_CAPTURE` and is covered for free; one that doesn't shows up as "the new probe window
/// opened focused" and gets its prefix added here.
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
    // The one that names the condition directly: `WOW_UNATTENDED` *is* "the chair is empty"
    // (1769), and docs/METHOD.md tells every ad-hoc probe to set it. It was missing here for as long
    // as this list has existed, so a probe launched exactly the way the method prescribes — but
    // under none of the named prefixes — opened focused and borderless-fullscreen over the
    // director's work. Reported 2026-09-01 while probing B346. This is the case the list's own
    // doc predicts ("one that doesn't shows up as the new probe window opened focused"); it is
    // also the strongest signal available, since nothing else asserts nobody is there.
    "WOW_UNATTENDED",
    "WOW_WORLDVIEW_",
];

/// Is this an instrumented background run? `WOW_BG=1` forces yes on any run, `WOW_BG=0` forces
/// no (the escape hatch when the director wants to *watch* a probe live); otherwise, auto-detect
/// from the run-driving env ([`BG_ENV_PREFIXES`]).
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

/// Background-run envs whose run produces **no image** — nothing reads the framebuffer, so the
/// window is pure plumbing. Prefix-matched, and the rule in [`no_pixel_run`] is **all-of**: a run
/// counts as no-pixel only when *every* background env it sets is in this list, so a
/// `WOW_PROBE_CHAT=… WOW_LIVE_SHOT=…` pairing (drive there, then shoot) keeps its full-size window.
///
/// Erring here is cheap in one direction only: a missing entry just means a big window, which is
/// the old behaviour. A *wrong* entry shrinks a window someone is photographing — so a new
/// `WOW_PROBE…`-named env that captures pixels has to be excluded by name here, since the bare
/// `WOW_PROBE` prefix would otherwise sweep it in.
const NO_PIXEL_ENV_PREFIXES: &[&str] = &[
    // Paired with its entry above, and it has to be BOTH or neither: `cine.sh` and
    // `summon-live.sh` all set `WOW_UNATTENDED` beside a `WOW_PROBE*`/`WOW_RIG` that IS no-pixel,
    // so listing it only as a background env would make it the one non-no-pixel member of every
    // such run and the all-of rule would grow all three back to the full 1600x900 that decision
    // 1148 shrank. A run whose ONLY background env is this one reads no pixels by construction —
    // anything that photographs the frame names itself (`WOW_CAPTURE`, `WOW_LIVE_SHOT`,
    // `WOW_*_SHOT`), and none of those are here, so every such pairing keeps its full size.
    "WOW_UNATTENDED",
    "WOW_FEED_GATE_CHECK",
    "WOW_FEED_GATE_TRACE",
    "WOW_FPS_",
    "WOW_LIVE_FPS",
    "WOW_PARTICLE_CENSUS",
    "WOW_PROBE",
    "WOW_RIG",
    "WOW_SCHED_CENSUS",
    // The engine boot check reads no pixels — it reads the ERROR LOG. Its sibling
    // `WOW_WORLDVIEW_SHOT` is deliberately absent: that one photographs the frame, and the
    // all-of rule below keeps a CHECK+SHOT pairing at full size.
    "WOW_WORLDVIEW_CHECK",
];

/// Does this run read no pixels — i.e. may its window be small and parked out of the way?
///
/// The point is the director's screen. A probe window is asserted `AlwaysOnTop` for its whole life
/// ([`crate::capture::ProbeFocusPlugin`], decision 0906 — an occluded macOS window throttles to
/// ~1 fps and a wall-clock probe schedule then fires the wrong steps), which silently defeats
/// everything this module does to keep instrumented runs behind the director's work: the level is
/// re-asserted every frame, over the `AlwaysOnBottom` birth cage AND over the `Normal` this module
/// hands back at release. That assertion is correct and stays. What was wrong is pairing it with
/// the **full 1600×900 default**, so every agent probe planted a screen-filling window over
/// whatever they were doing — six of them in one session, which is how it got reported.
/// Un-occludable and unobtrusive were never in conflict: a small window in a
/// corner is both.
///
/// `WOW_BG=1` (forced background, no run-driving env) is deliberately NOT no-pixel — that is the
/// hand-driven case, and shrinking a window the director asked for is the opposite of the point.
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

/// On a [`background_run`], undo winit's launch-time app activation and window ordering (macOS;
/// no-op elsewhere) for the length of the launch window, then get out of the way. The
/// `focused: false` half lives where the window is built, in `main`.
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

    /// How quiet the launch has to go before [`hold_background`] calls it settled and lets go:
    /// this long with the app continuously inactive. winit's `activateIgnoringOtherApps` is a
    /// WindowServer round-trip that can be *granted* well after `PreStartup`'s
    /// [`hand_back_activation`] already ran — and a granted activation re-raises our window — so
    /// the hold can't be a fixed count; every activation it sees restarts the clock.
    const SETTLE: Duration = Duration::from_millis(500);

    /// Hard ceiling on the hold, however unsettled things look. Past this the window is the
    /// director's, and an unrecognised activation source is a bug to find, not to fight forever.
    const CEILING: Duration = Duration::from_secs(3);

    /// How long to keep watching after the promotion to Regular. Raising the activation policy
    /// is the one step that could itself pull the app forward, so the release is not trusted
    /// blind — it is checked for a moment, then latched shut.
    const TAIL: Duration = Duration::from_millis(300);

    /// Where the launch correction has got to. The whole plugin is this one-way sequence.
    #[derive(Default)]
    pub(super) enum Phase {
        /// Undoing winit's activation and raise until the launch goes quiet.
        #[default]
        Holding,
        /// Quiet reached: the window level has been retargeted at Normal and bevy applies the
        /// `setLevel` this frame — which re-orders us to the front of the level we just joined.
        Promoting,
        /// Regular policy restored (Dock icon, Cmd-Tab entry). Watching for a moment in case that
        /// promotion pulled the app forward with it.
        Tail(Duration),
        /// Hands off, permanently.
        Done,
    }

    /// Give activation back to the app the director was using. The `NonSendMarker` param pins
    /// the system to the main thread (AppKit's requirement).
    ///
    /// Demoting to the Accessory policy for the launch is load-bearing, and not merely to hide
    /// the Dock icon: `deactivate()` on its own does **not** hold a Regular-policy app in the
    /// background on macOS 26 — measured, the app took frontmost within 160 ms of launch and kept
    /// it for the whole run, which is the 2026-07-19 keystroke-stealing bug exactly. Accessory is
    /// what actually keeps us out of the foreground. Its cost — no Cmd-Tab entry — is why it is
    /// wrong to *leave* on (that is what stranded the window), so it is scoped to the launch and
    /// promoted back to Regular the moment things settle.
    pub(super) fn hand_back_activation(_main_thread: NonSendMarker) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let _ = hand_back(&app);
    }

    /// The launch window: keep handing activation back and keep our window at the back of the
    /// stack until the launch settles, then let go for good. A no-op thereafter — from that point
    /// the window is ordinary, and raising it is between the director and the WindowServer.
    ///
    /// Deliberately *not* keyed on window focus, which reads like the obvious "the director
    /// reached for it" signal and isn't: winit's `window_activation_hack`
    /// (`app_state.rs`, `applicationDidFinishLaunching`) calls `makeKeyAndOrderFront` on every
    /// visible window regardless of the `focused: false` we asked for, so the launch itself
    /// delivers a focus gain on ~frame 2 — and an activation granted later delivers another, as
    /// the key window resigns and re-takes key across the app deactivate/activate. Focus cannot
    /// tell us apart from the WindowServer, so the hold is timed against quiet instead, and kept
    /// short enough that it can't meaningfully fight a director who clicks during it.
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
                        // An activation landed (winit's, granted late). Restart the settle clock:
                        // whatever raised us once can raise us again while the round-trip is
                        // still in flight.
                        *last_active = now;
                        debug!("bgwin: took activation back {now:?} in");
                    }
                    order_back(&app);
                    return;
                }
                // Quiet. Start letting go — the window level first, retargeted through the
                // component so bevy's own cache stays honest about where the window is.
                let Ok(mut window) = windows.single_mut() else {
                    return;
                };
                window.window_level = WindowLevel::Normal;
                *phase = Phase::Promoting;
            }
            Phase::Promoting => {
                // bevy applied the `setLevel` at the end of last frame, which put us at the front
                // of the normal level; drop straight back to the bottom of it before anything is
                // presented on top of the director's work. Then hand the Dock icon and the
                // Cmd-Tab entry back.
                order_back(&app);
                app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
                *phase = Phase::Tail(now);
            }
            Phase::Tail(since) => {
                if hand_back(&app) {
                    // The promotion pulled us forward after all — put it back.
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

    /// Give activation back to whoever had it before we launched. Reports whether there was any
    /// to give back — i.e. whether an activation had landed on us since the last look.
    fn hand_back(app: &NSApplication) -> bool {
        // SAFETY: main thread — every caller takes `NonSendMarker` and checks `MainThreadMarker`.
        unsafe {
            if !app.isActive() {
                return false;
            }
            app.deactivate();
            true
        }
    }

    /// Order our windows to the back of the normal window level: on screen (surface rendering,
    /// readback and screenshots untouched) but behind the director's work.
    ///
    /// Plain stacking, not a window level — that is the whole point. `AlwaysOnBottom` pins a
    /// window below every normal one *forever*; a window ordered to the back is merely last in
    /// line, and comes forward the instant anything raises it.
    ///
    /// Unconditional for the length of the hold, and deliberately not gated on "the app is
    /// active": winit raises the window when it creates it and again in its
    /// `window_activation_hack`, both of which can land while we are already inactive, so a gate
    /// on activation would leave that raise standing and the window parked on top for the whole
    /// run. Repeating it while already at the back is an idempotent no-op.
    fn order_back(app: &NSApplication) {
        // SAFETY: main thread — every caller takes `NonSendMarker` and checks `MainThreadMarker`.
        unsafe {
            for window in app.windows().iter() {
                window.orderBack(None);
            }
        }
    }
}
