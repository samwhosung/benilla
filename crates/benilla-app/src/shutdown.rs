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
             would have written on the way out is simply never written (1528)"
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
             PLAYER_LEAVING_WORLD/PLAYER_LOGOUT into every addon (1537)"
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
}
