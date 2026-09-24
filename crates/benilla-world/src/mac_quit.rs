//! **macOS `Cmd+Q`, turned into an exit the client can see**.
//!
//! On every other exit the client gets a frame to write the player's session down — the shutdown
//! systems run in `Last`, after the `AppExit` (`benilla_app::shutdown`). `Cmd+Q` gets none.
//!
//! The gesture is winit's own default menu bar: `menu.rs` builds an app menu whose *Quit* item is
//! wired straight to AppKit's `terminate:` (`applicationDidFinishLaunching`, and only when
//! `default_menu` is on — bevy_winit 0.18 leaves it on and exposes no
//! `EventLoopBuilderExtMacOS` hook to turn it off). `terminate:` runs
//! `applicationWillTerminate:`, which closes every window and calls winit's `internal_exit()` →
//! `LoopExiting` → `bevy_winit`'s `exiting()`, which clears the world. **No `app.update()` happens
//! anywhere in that sequence**, so there is no schedule for a shutdown system to run in and no
//! `AppExit` for one to read. Measured on the real client, in the world: `Cmd+Q` wrote nothing —
//! not the flat saved variables, not one addon's `saved/*.lua`, not `AddOns.txt`, not the camera
//! pose, not `config.toml`.
//!
//! The fix is the one winit itself points at: *"The menubar initialization should be before the
//! `NewEvents` event, to allow overriding of the default menu even if it's created."* We re-point
//! that one item at **`performClose:`**, so `Cmd+Q` becomes the window close — which Bevy already
//! models end to end (`WindowCloseRequested` → `close_when_requested` → `exit_on_all_closed` →
//! `AppExit` in `PostUpdate` → our `Last` shutdown tail → a clean exit). For a single-window game
//! *quit* and *close the window* are the same act, so nothing is lost by making them the same
//! code path; what is gained is that the gesture a macOS player reaches for first stops eating
//! their settings.
//!
//! `setTarget(nil)` sends it down the responder chain, so the key window handles it — the same
//! route the red close button takes. Everything here is a no-op off macOS.

use bevy::prelude::*;

/// Re-points macOS's *Quit* menu item at the window close, at `Startup` — which runs inside the
/// event loop, so winit's menu already exists (it is built in `applicationDidFinishLaunching`,
/// before the first `NewEvents`).
pub struct MacQuitPlugin;

impl Plugin for MacQuitPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, route_quit_through_window_close);
    }
}

#[cfg(not(target_os = "macos"))]
fn route_quit_through_window_close() {}

/// The `NonSendMarker` pins the system to the main thread, which is AppKit's requirement (the
/// same param `bgwin`'s AppKit systems carry).
#[cfg(target_os = "macos")]
fn route_quit_through_window_close(_main_thread: bevy::ecs::system::NonSendMarker) {
    use objc2::sel;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::MainThreadMarker;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: main-thread AppKit reads of the app's own menu tree, and one property write on an
    // item we found there. Every call is the documented signature.
    let rerouted = unsafe {
        let Some(main_menu) = app.mainMenu() else {
            // No default menu (a bundled build that supplies its own, or `default_menu` off).
            // Nothing to correct, and nothing bound to `terminate:` to lose settings through.
            return;
        };
        let mut rerouted = 0usize;
        for item in main_menu.itemArray().iter() {
            let Some(submenu) = item.submenu() else {
                continue;
            };
            for entry in submenu.itemArray().iter() {
                if entry.action() != Some(sel!(terminate:)) {
                    continue;
                }
                entry.setTarget(None); // down the responder chain → the key window
                entry.setAction(Some(sel!(performClose:)));
                rerouted += 1;
            }
        }
        rerouted
    };
    // Loud when it finds nothing: a winit change that moves or renames the item would otherwise
    // silently restore the settings-eating gesture, and that failure is invisible from inside the
    // client.
    if rerouted == 0 {
        warn!(
            "mac_quit: no Quit item bound to terminate: — Cmd+Q may bypass the shutdown tail \
             and lose this session's settings (1528)"
        );
    } else {
        info!("mac_quit: Cmd+Q routed through the window close ({rerouted} item(s))");
    }
}
