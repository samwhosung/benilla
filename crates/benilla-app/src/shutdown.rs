//! The application-exit edge: where "the client is going down" may be observed.
//!
//! Bevy's runner leaves the loop after the `app.update()` in which an `AppExit` appears. A window
//! close writes it in `PostUpdate` (`exit_on_all_closed`), after every `Update` system, so a saver
//! in `Update` never sees the exit a player causes. [`on_app_exit`] puts savers in `Last`, after
//! every announcement.
//!
//! On macOS one more `Main` pass can run after `event_loop.exit()`, and `exit_on_all_closed`
//! announces again on every frame with no window, so the set runs only on the rising edge
//! ([`the_exit_frame`]): firing `PLAYER_LEAVING_WORLD` and `PLAYER_LOGOUT` twice would reach every
//! addon twice.
//!
//! Not covered: macOS `Cmd+Q` runs no `app.update()` and is re-pointed at `performClose:` by
//! [`benilla_world::mac_quit`]; a crash or `SIGKILL` loses the session, as in the reference, which
//! has no autosave.

use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;

/// Every system that persists state as the client goes down.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct OnAppExit;

/// The set's edge condition is attached once, however many savers register.
#[derive(Resource)]
struct ExitSetLatched;

/// True on the first frame of a run of `AppExit` announcements. An edge, not a once-per-process
/// latch: an exit after a quiet frame is a new shutdown, as a test harness drives.
fn the_exit_frame(mut exits: MessageReader<AppExit>, mut announced: Local<bool>) -> bool {
    let now = exits.read().next().is_some();
    let rising = now && !*announced;
    *announced = now;
    rising
}

/// Registers a system to run once on the frame the app decides to exit; it reads
/// `MessageReader<AppExit>` and works when the read is non-empty.
pub(crate) fn on_app_exit(app: &mut App, systems: ScheduleConfigs<ScheduleSystem>) -> &mut App {
    if !app.world().contains_resource::<ExitSetLatched>() {
        app.insert_resource(ExitSetLatched);
        app.configure_sets(Last, OnAppExit.run_if(the_exit_frame));
    }
    app.add_systems(Last, systems.in_set(OnAppExit))
}

/// Alt+F4 closes the primary window on Windows and Linux.
///
/// Deviation: the reference leaves it to the OS. Its key arm (`0x42d261`-`0x42d276`) hands Alt+F4
/// to `DefWindowProc`, whose `SC_CLOSE` becomes the kind-5 event (`0x42d287`, as `WM_CLOSE` posts at
/// `0x42d292`) and runs the quit at once (`0x423a21` -> `0x423ca0` -> `0x41f5d0` -> `0x422030`),
/// with no logout timer, at the login screen and in game alike. A tiling window manager (i3,
/// sway, Hyprland) binds nothing to it, so benilla reads the chord itself and asks for the same
/// close, which the shutdown tail above already serves. Not on macOS: `Cmd+Q` is
/// [`benilla_world::mac_quit`].
pub(crate) struct AltF4Plugin;

impl Plugin for AltF4Plugin {
    fn build(&self, app: &mut App) {
        if cfg!(target_os = "macos") {
            return;
        }
        // Before the loading cover swallows the input plane, which would eat the key.
        app.add_systems(
            PreUpdate,
            close_on_alt_f4
                .after(bevy::input::InputSystems)
                .before(crate::loading_screen::CoverInput),
        );
    }
}

/// F4 pressed with Alt held asks the primary window to close, as a click on its close box does.
fn close_on_alt_f4(
    mut keyboard: MessageReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<Entity, With<bevy::window::PrimaryWindow>>,
    mut close: MessageWriter<bevy::window::WindowCloseRequested>,
) {
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let pressed = keyboard.read().any(|ev| {
        ev.key_code == KeyCode::F4 && ev.state == bevy::input::ButtonState::Pressed && !ev.repeat
    });
    if pressed && alt {
        if let Ok(window) = windows.single() {
            close.write(bevy::window::WindowCloseRequested { window });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::{PrimaryWindow, Window, WindowCloseRequested, WindowPlugin};

    /// How many frames the probe ran on.
    #[derive(Resource, Default)]
    struct Saw(u32);

    fn probe(mut exits: MessageReader<AppExit>, mut saw: ResMut<Saw>) {
        if exits.read().next().is_some() {
            saw.0 += 1;
        }
    }

    /// `bevy_window`'s close takes two frames: mark `ClosingWindow`, then despawn.
    fn close_and_exit(app: &mut App) {
        app.update();
        app.update();
    }

    /// `bevy_window`'s real close and exit systems, one window, and a probe placed by `place`.
    fn app_with_probe(place: fn(&mut App)) -> App {
        let mut app = App::new();
        app.add_plugins(WindowPlugin {
            primary_window: None, // spawned below, so the test owns the entity id
            ..default()
        })
        .init_resource::<Saw>();
        place(&mut app);
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        app.world_mut()
            .write_message(WindowCloseRequested { window });
        app
    }

    #[test]
    fn an_update_system_never_sees_the_exit_a_window_close_produces() {
        let mut app = app_with_probe(|app| {
            app.add_systems(Update, probe);
        });
        close_and_exit(&mut app);

        assert!(
            app.should_exit().is_some(),
            "the close did produce an AppExit — bevy_window's exit_on_all_closed ran"
        );
        assert!(
            app.world().resource::<Saw>().0 == 0,
            "…and the Update system did not see it. This is the bug: every file the session \
             would have written on the way out is simply never written"
        );
    }

    #[test]
    fn on_app_exit_sees_the_exit_a_window_close_produces() {
        let mut app = app_with_probe(|app| {
            on_app_exit(app, probe.into_configs());
        });
        close_and_exit(&mut app);

        assert!(app.should_exit().is_some(), "the close produced an AppExit");
        assert!(
            app.world().resource::<Saw>().0 == 1,
            "and the shutdown system ran on that frame — the frame the exit was announced"
        );
    }

    /// A `/quit`, the logout edge and the capture harness announce from `Update`.
    #[test]
    fn on_app_exit_also_sees_an_exit_announced_from_update() {
        let mut app = App::new();
        app.init_resource::<Saw>();
        app.add_systems(Update, |mut exit: MessageWriter<AppExit>| {
            exit.write(AppExit::Success);
        });
        on_app_exit(&mut app, probe.into_configs());
        app.update();

        assert_eq!(app.world().resource::<Saw>().0, 1);
    }

    /// Running past the exit frame is the extra pass macOS can pump.
    #[test]
    fn a_second_announcement_does_not_run_the_tail_again() {
        let mut app = app_with_probe(|app| {
            on_app_exit(app, probe.into_configs());
        });
        close_and_exit(&mut app);
        assert_eq!(
            app.world().resource::<Saw>().0,
            1,
            "the exit frame ran the tail"
        );

        app.update(); // the pass macOS can pump after `event_loop.exit()`
        app.update();
        assert!(
            app.should_exit().is_some(),
            "…and `exit_on_all_closed` really did announce again — the control for this test"
        );
        assert_eq!(
            app.world().resource::<Saw>().0,
            1,
            "the tail runs on the edge; a second run would re-fire \
             PLAYER_LEAVING_WORLD/PLAYER_LOGOUT into every addon"
        );
    }

    #[test]
    fn a_later_exit_after_a_quiet_frame_is_a_new_edge() {
        let mut app = App::new();
        app.init_resource::<Saw>();
        on_app_exit(&mut app, probe.into_configs());

        app.world_mut().write_message(AppExit::Success);
        app.update();
        app.update(); // quiet: the edge re-arms
        app.world_mut().write_message(AppExit::Success);
        app.update();

        assert_eq!(app.world().resource::<Saw>().0, 2);
    }

    /// Alt+F4 writes the close request for the primary window; F4 alone and Alt with another key
    /// do not.
    #[test]
    fn alt_f4_asks_the_primary_window_to_close_and_plain_f4_does_not() {
        use bevy::input::keyboard::{Key, KeyboardInput};
        use bevy::input::ButtonState;
        let run = |alt: bool, key: KeyCode| {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .add_plugins(bevy::input::InputPlugin)
                .add_message::<WindowCloseRequested>()
                .add_systems(PreUpdate, close_on_alt_f4.after(bevy::input::InputSystems));
            let window = app
                .world_mut()
                .spawn((Window::default(), PrimaryWindow))
                .id();
            if alt {
                app.world_mut().write_message(KeyboardInput {
                    key_code: KeyCode::AltLeft,
                    logical_key: Key::Alt,
                    state: ButtonState::Pressed,
                    text: None,
                    repeat: false,
                    window,
                });
            }
            app.world_mut().write_message(KeyboardInput {
                key_code: key,
                logical_key: Key::F4,
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window,
            });
            app.update();
            let sent: Vec<Entity> = app
                .world_mut()
                .resource_mut::<Messages<WindowCloseRequested>>()
                .drain()
                .map(|c| c.window)
                .collect();
            (sent, window)
        };
        let (sent, window) = run(true, KeyCode::F4);
        assert_eq!(sent, [window], "Alt+F4 closes the primary window");
        assert!(
            run(false, KeyCode::F4).0.is_empty(),
            "plain F4 does nothing"
        );
        assert!(run(true, KeyCode::F5).0.is_empty(), "Alt+F5 does nothing");
    }
}
