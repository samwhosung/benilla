//! **The application-exit edge** — the one place that says *where* "the client is going down" may
//! be observed, because getting that wrong is silent and costs the player everything they changed.
//!
//! ## The frame the app decides to quit is the last frame there is — but it is not always the only one
//!
//! Bevy's runner checks `App::should_exit()` **after** `app.update()` returns and, if an `AppExit`
//! is there, leaves the event loop (`bevy_winit::state`, `run_app_update`'s tail). So a system that
//! wants to run "on the way out" gets its chance in the same `Main` pass in which the exit was
//! announced — and only if it runs *after* the announcement.
//!
//! What it does **not** get is a guarantee that the pass is the last one. On macOS
//! `event_loop.exit()` is a request, not a return: events already queued still dispatch, and one
//! more `about_to_wait` → `run_app_update` can land before the loop unwinds. `exit_on_all_closed`
//! has no latch of its own — it writes an `AppExit` on **every** frame its window query is empty —
//! so that extra pass announces the exit a second time and every shutdown system reading
//! `MessageReader<AppExit>` runs twice. Observed on the real client, in `scripts/smoke.sh`, roughly
//! one run in three: two `"No windows are open, exiting"` lines 50 ms apart, the second on the far
//! side of winit's own `"Closing window 0v0"`, with a complete duplicate save burst under each.
//!
//! Rewriting the same files twice is harmless; **firing `PLAYER_LEAVING_WORLD`/`PLAYER_LOGOUT`
//! into every addon's Lua twice is not**. So the set runs on the *edge*: [`the_exit_frame`] opens
//! on the first frame of a run of announcements and stays shut for the rest of it.
//!
//! Announcements do not all arrive early. The one a **player** produces is the latest of all:
//! clicking the window's close button posts `WindowCloseRequested`, `close_when_requested`
//! (`Update`) despawns the window, and `exit_on_all_closed` (**`PostUpdate`**) writes the
//! `AppExit` (`bevy_window`'s own placement, 0.18.1). Every `Update` system has already run by
//! then. So *"read `MessageReader<AppExit>` in `Update`"* — which is what three of our five
//! shutdown systems did — cannot see the exit a player actually causes, and no later frame will
//! give it a second look.
//!
//! **Measured, not reasoned**: the real client, in the world, closed by its own window button
//! wrote *nothing* — no `saved-variables.lua`, no per-addon `saved/*.lua`, no `addons/*.txt`, no
//! camera pose, no `config.toml`. Every addon's settings and every UI toggle the session changed
//! were lost, and the next login came up on whatever the last *graceful* exit had written. That is
//! the director's *"addon settings don't save between logins"* in one line.
//!
//! ## The rule
//!
//! **Register it here, never by hand.** [`on_app_exit`] puts the system in `Last`, which runs after
//! `PostUpdate` in the same `Main` pass and therefore after every announcement Bevy can make. Two
//! systems already lived there and had quietly been right the whole time
//! (`hover_log::report_on_exit`, `perf::stall::beat` — whose own comment says *"no later frame will
//! run to unlatch anything"*); this makes that the rule instead of a coincidence.
//!
//! ## What this does NOT fix, said out loud
//!
//! An exit the process is never told about in ECS terms still loses the session:
//!
//! - **macOS `Cmd+Q`** goes `terminate:` → `applicationWillTerminate:` → winit's `LoopExiting`,
//!   with no `app.update()` anywhere in it — so there is no schedule to run in. Handled a layer
//!   down instead, by [`benilla_world::mac_quit`], which re-points that menu item at
//!   `performClose:` so the gesture becomes the window close above.
//! - **A crash, a `SIGKILL`, a wedged teardown.** Nothing survives those, and the reference client
//!   loses settings to them too (decision 1128 kept its no-autosave shape deliberately). If we ever
//!   want to beat that, it is an autosave design and its own decision — not a fourth place to read
//!   `AppExit`.

use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;

/// Every system that persists state as the client goes down. A set rather than bare `Last`
/// registrations so the ordering question ("does the CVar flush see the VM the UI teardown just
/// replaced?") has somewhere to be answered when it comes up.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct OnAppExit;

/// Marks the one-time `configure_sets` below as done — [`on_app_exit`] is called once per shutdown
/// system, and the set's edge condition must be attached exactly once however many of them there are.
#[derive(Resource)]
struct ExitSetLatched;

/// **The rising edge of "the app is exiting"** — true on the first frame of a run of `AppExit`
/// announcements, false for every frame the run continues.
///
/// An *edge*, not a once-per-process latch, because that is what the tail is for and because the
/// two are not the same thing. `exit_on_all_closed` re-announces on every frame its window query is
/// empty, so the extra `Main` pass macOS can pump after `event_loop.exit()` produces a second,
/// immediately-consecutive announcement — suppressed here. A *later* announcement with a quiet
/// frame in between is a different shutdown, and is served: that is how a harness drives more than
/// one exit through one `App`, which `cvars`' persistence test does.
///
/// A run condition on the set rather than a latch inside each shutdown system, because the systems
/// are registered from eight different modules and "has this edge already been served" is a
/// property of the edge, not of any one saver.
fn the_exit_frame(mut exits: MessageReader<AppExit>, mut announced: Local<bool>) -> bool {
    let now = exits.read().next().is_some();
    let rising = now && !*announced;
    *announced = now;
    rising
}

/// Register a system that must run on the frame the app decides to exit.
///
/// The schedule is not the caller's choice — that is the whole point of the function existing (see
/// the module header). Call it with a system that reads `MessageReader<AppExit>` and does its work
/// when the read is non-empty; the set's own edge condition guarantees it is asked once per exit.
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

    /// How many frames the probe ran on. A resource rather than a `Local` so the test can read it
    /// back — and a count rather than a flag, because "ran twice" is one of the things under test.
    #[derive(Resource, Default)]
    struct Saw(u32);

    fn probe(mut exits: MessageReader<AppExit>, mut saw: ResMut<Saw>) {
        if exits.read().next().is_some() {
            saw.0 += 1;
        }
    }

    /// `bevy_window`'s close takes **two** frames — `close_when_requested` marks the window
    /// `ClosingWindow` on the frame the request arrives and despawns it on the next, and only then
    /// does `exit_on_all_closed` find no windows. Both are driven here so the test measures the
    /// real sequence rather than a shortened one.
    fn close_and_exit(app: &mut App) {
        app.update();
        app.update();
    }

    /// An app with `bevy_window`'s real close→exit systems and one window, plus a probe registered
    /// by `place`. No winit: `close_when_requested` and `exit_on_all_closed` are plain systems and
    /// they are the two that decide when the `AppExit` lands.
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

    /// **The director's bug, reproduced.** A player closes the window; a shutdown system in
    /// `Update` never learns the client is exiting, because the `AppExit` is not written until
    /// `PostUpdate` — and the runner leaves the loop before another frame starts.
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

    /// …and the fix, at the same edge: [`on_app_exit`] places it in `Last`, after `PostUpdate`,
    /// so the same close is observed on the frame it happens.
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

    /// The other four roots announce from `Update` (a `/quit` from Lua, the logout-complete edge,
    /// the capture harness). `Last` is after those too, so one placement covers every announcement
    /// rather than winning a scheduler lottery — which is what the old `Update` placement was doing
    /// on the paths where it happened to work.
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

    /// **The exit can be announced twice, and the tail must still run once** (1537).
    ///
    /// `exit_on_all_closed` has no latch: with the window gone it writes an `AppExit` on every
    /// frame it runs. macOS pumps one more `Main` pass after `event_loop.exit()` often enough to
    /// fail `scripts/smoke.sh` about one run in three — and a second pass means a second
    /// `PLAYER_LOGOUT` into every addon's Lua, not merely a second identical file write. Driven
    /// here by running past the exit frame, which is exactly what that extra pass is.
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

    /// …and an edge is not a once-per-process latch. A harness that drives an exit, carries on, and
    /// drives another gets both — which is how `cvars`' persistence test writes `config.toml` twice
    /// through one `App`, and the reason [`the_exit_frame`] detects an edge rather than spending.
    #[test]
    fn a_later_exit_after_a_quiet_frame_is_a_new_edge() {
        let mut app = App::new();
        app.init_resource::<Saw>();
        on_app_exit(&mut app, probe.into_configs());

        app.world_mut().write_message(AppExit::Success);
        app.update();
        app.update(); // quiet — nothing announced, so the edge re-arms
        app.world_mut().write_message(AppExit::Success);
        app.update();

        assert_eq!(app.world().resource::<Saw>().0, 2);
    }
}
