//! The origins outside a script call, a lane's firing and the tick that may raise an event into
//! the VM ([`benilla_ui::script::origin`]), and the arming of that check around each frame's
//! schedules. In a debug build a raise anywhere else names its caller: the app's tests and a run
//! under `BENILLA_RAISE_STRICT=1` panic, and a live run warns once per site and goes on.

use benilla_ui::script::origin::{Allow, Failure};
use bevy::app::MainScheduleOrder;
use bevy::ecs::schedule::{ExecutorKind, ScheduleLabel};
use bevy::prelude::*;

/// Each origin, by a suffix of its function's path, with the reference fact that puts its raise
/// there, or the guard row that names it as remaining work: a reason that opens with a list's
/// name ([`ROW_LISTS`]) cites that list's row for the same function, which a test holds to it.
pub(crate) const RAISES_AT_THEIR_OWN_PLACE: &[Allow] = &[
    // Their reference places, outside every packet and call.
    Allow {
        frame: "net::handlers::call",
        why: "a packet's handler that signals in place, inside its packet, as the reference's \
              handlers run whole (`0x537ae5`) and signal inside themselves (`0x703e50`): one \
              handler's run, `NetHandlers::run`'s, not the dispatch's flushes, packet ends and \
              span starts around it",
    },
    Allow {
        frame: "minimap::feed_minimap_inside",
        why: "the indoor byte's one writer is the minimap's update (`0x6d93a0`), which signals \
              `MINIMAP_UPDATE_ZOOM` on its change (`0x6d95b8`) at frame time",
    },
    Allow {
        frame: "ui_script::sweep::sweep_on_worldport",
        why: "a worldport's teardown is a timer (`0x401bc0`, posted at `0x401ba5`), and the tick \
              runs its timers ahead of the input and the packets (`0x420d0c`); its sweep signals \
              `PLAYER_LEAVING_WORLD` (`0x490b4d`)",
    },
    Allow {
        frame: "ui_logout::leave_world",
        why: "`SetInWorld(0)` (`0x5aae70`), the teardown timer's first call (`0x401bd4`), \
              signals `LOGOUT_CANCEL` in place (`0x5aae97`)",
    },
    Allow {
        frame: "ui_session::latch::walk",
        why: "the interaction watchdog (`0x493230`), every frame, closes the latched window \
              through `0x493310` (`0x493305`), whose close core signals; ours applies from the \
              watchdog's commands",
    },
    Allow {
        frame: "ui_script::lifecycle::install_glue_vm",
        why: "the glue's build (`0x46a7b0`) signals `FRAMES_LOADED` (`0x46aa34`) and the screen \
              (`0x46b848`) itself, outside any packet",
    },
    Allow {
        frame: "ui_script::lifecycle::load_ingame_ui_seated",
        why: "the interface's load signals its own events, `VARIABLES_LOADED` among them \
              (`0x49156a`), outside any packet",
    },
    Allow {
        frame: "ui_script::lifecycle::close_ui_session",
        why: "the interface's shutdown (`0x490bd0`) signals `PLAYER_LEAVING_WORLD` (`0x490b4d`) \
              and `PLAYER_LOGOUT` (`0x490c2a`) itself, at a logout's, a disconnect's or a \
              reload's root",
    },
    // Rows of the guard's lists.
    Allow {
        frame: "ui_script::glue_mgr::drive_glue_mgr",
        why: "AT_THEIR_OWN_PLACE: the login and character screens' answers",
    },
    Allow {
        frame: "capture::fixtures::seed_ui_fixture",
        why: "QUEUED_FOR_THE_TICK: a capture's fixture",
    },
    Allow {
        frame: "capture::fixtures::open_character_fixture",
        why: "a capture's fixture, dev tooling outside any session: the character window's open \
              stands in for a login's items' creates, whose set-up signals \
              `UNIT_INVENTORY_CHANGED` in place (`0x5d8606`)",
    },
    Allow {
        frame: "ui_world_map::feed_world_map",
        why: "QUEUED_FOR_THE_TICK: the first world enter's selection",
    },
    Allow {
        frame: "ui_world_map::install_world_loc_projector",
        why: "QUEUED_FOR_THE_TICK: the projector's install",
    },
];

/// The guard's lists an allow entry's reason may cite as its row.
#[cfg(test)]
pub(crate) const ROW_LISTS: &[&str] = &[
    "AT_THEIR_OWN_PLACE",
    "NOT_YET_ON_A_LANE",
    "QUEUED_FOR_THE_TICK",
];

/// Ahead of [`First`]: arms the check for the frame.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct ArmRaiseOrigin;

/// After [`Last`]: disarms it.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct DisarmRaiseOrigin;

/// Arm the check from the head of each frame's schedules to their tail; once per app.
pub(crate) fn arm_each_frame(app: &mut App) {
    if app.world().get_resource::<MainScheduleOrder>().is_none()
        || app.get_schedule(ArmRaiseOrigin).is_some()
    {
        return;
    }
    for (label, run) in [
        (ArmRaiseOrigin.intern(), arm as fn(&mut World)),
        (DisarmRaiseOrigin.intern(), disarm),
    ] {
        let mut schedule = Schedule::new(label);
        // Run on the thread running the frame, which holds the VM.
        schedule.set_executor_kind(ExecutorKind::SingleThreaded);
        schedule.add_systems(run);
        app.add_schedule(schedule);
    }
    let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
    order.insert_before(First, ArmRaiseOrigin);
    order.insert_after(Last, DisarmRaiseOrigin);
}

fn arm(_: &mut World) {
    benilla_ui::script::origin::arm_until_disarmed(RAISES_AT_THEIR_OWN_PLACE, failure());
}

/// A panic under the app's tests or `BENILLA_RAISE_STRICT=1`; else a warning per site, so a
/// developer's debug client goes on.
fn failure() -> Failure {
    static STRICT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let strict = *STRICT.get_or_init(|| {
        std::env::var_os("BENILLA_RAISE_STRICT").is_some_and(|v| !v.is_empty() && v != "0")
    });
    if cfg!(test) || strict {
        Failure::Panic
    } else {
        Failure::Report(report)
    }
}

/// A live run's report of a raise from an unlisted origin: a warning, and a line on the trace
/// (`WOW_MOVE_TRACE=<path>`, tag `raise`).
fn report(line: &str) {
    warn!("raise origin: {line}");
    if benilla_assets::trace::enabled_for("raise") {
        benilla_assets::trace::line("raise", line);
    }
}

fn disarm(_: &mut World) {
    benilla_ui::script::origin::disarm();
}

#[cfg(test)]
mod tests {
    use benilla_ui::script::origin::{within, Origin};
    use benilla_ui::script::UiScript;
    use bevy::prelude::*;

    fn app(system: fn(&mut World)) -> App {
        let mut app = App::new();
        super::arm_each_frame(&mut app);
        app.insert_non_send_resource(UiScript::new().expect("a VM"))
            .add_systems(Update, system);
        app
    }

    /// The app's tests panic on an unlisted raise.
    #[test]
    fn the_apps_tests_fail_hard() {
        assert!(matches!(super::failure(), super::Failure::Panic));
    }

    fn raises_at_frame_time(world: &mut World) {
        let mut ui = world.non_send_resource_mut::<UiScript>();
        ui.fire_event("PLAYER_MONEY", Vec::new());
    }

    fn raises_in_a_lane(world: &mut World) {
        let mut ui = world.non_send_resource_mut::<UiScript>();
        within(Origin::Lane, || ui.fire_event("PLAYER_MONEY", Vec::new()));
    }

    /// A frame-time system that raises outside every script call, lane firing and tick fails, its
    /// own function named.
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "the check compiles out")]
    fn a_raise_from_a_frame_time_system_fails_naming_it() {
        let mut app = app(raises_at_frame_time);
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.update()))
            .expect_err("the raise passed");
        let message = failed.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(
            message.contains("`PLAYER_MONEY`")
                && message.contains("raise_origin::tests::raises_at_frame_time"),
            "{message}"
        );
    }

    /// The same raise inside a lane's firing passes, and so does one between frames, unarmed.
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "the check compiles out")]
    fn a_raise_inside_an_origin_or_between_frames_passes() {
        let mut app = app(raises_in_a_lane);
        app.update();
        app.update();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .fire_event("PLAYER_MONEY", Vec::new());
    }
}
