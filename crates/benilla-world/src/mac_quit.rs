//! macOS `Cmd+Q`, re-pointed at the window close so quitting saves the session. winit's default
//! menu wires Quit to AppKit's `terminate:`, which tears the app down with no `app.update()`, so no
//! `AppExit` is read and the `Last` shutdown systems never run; bevy_winit 0.18 exposes no way to
//! turn that menu off. Re-pointed at `performClose:`, Quit becomes the window close, which Bevy
//! carries through `AppExit` to the shutdown systems. A no-op off macOS.

use bevy::prelude::*;

/// Re-points the Quit menu item at `Startup`, which runs inside the event loop, after winit built
/// its menu in `applicationDidFinishLaunching`.
pub struct MacQuitPlugin;

impl Plugin for MacQuitPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, route_quit_through_window_close);
    }
}

#[cfg(not(target_os = "macos"))]
fn route_quit_through_window_close() {}

/// The `NonSendMarker` pins the system to the main thread, as AppKit requires.
#[cfg(target_os = "macos")]
fn route_quit_through_window_close(_main_thread: bevy::ecs::system::NonSendMarker) {
    use objc2::sel;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::MainThreadMarker;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: main-thread AppKit reads of the app's own menu tree and property writes on the items
    // found there, each call with its documented signature.
    let rerouted = unsafe {
        let Some(main_menu) = app.mainMenu() else {
            // No default menu (a bundled build's own, or `default_menu` off): nothing to re-point.
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
                entry.setTarget(None); // nil: down the responder chain to the key window
                entry.setAction(Some(sel!(performClose:)));
                rerouted += 1;
            }
        }
        rerouted
    };
    // Warn when nothing was found: a winit change that moves the item would otherwise silently
    // bring back the quit that skips the shutdown systems.
    if rerouted == 0 {
        warn!(
            "mac_quit: no Quit item bound to terminate: — Cmd+Q may bypass the shutdown tail \
             and lose this session's settings"
        );
    } else {
        info!("mac_quit: Cmd+Q routed through the window close ({rerouted} item(s))");
    }
}
