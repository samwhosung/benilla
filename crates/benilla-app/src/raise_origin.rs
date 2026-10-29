//! The origins outside a script call, a lane's firing and the tick that may raise an event into
//! the VM ([`benilla_ui::script::origin`]), and the arming of that check around each frame's
//! schedules. A raise anywhere else fails in a debug build, naming its caller.

use benilla_ui::script::origin::Allow;
use bevy::app::MainScheduleOrder;
use bevy::ecs::schedule::{ExecutorKind, ScheduleLabel};
use bevy::prelude::*;

/// Each origin, by a suffix of its function's path, with the reference fact that puts its raise
/// there, or the guard row that names it as remaining work: a reason that opens with a list's
/// name ([`ROW_LISTS`]) cites that list's row for the same function, which a test holds to it.
pub(crate) const RAISES_AT_THEIR_OWN_PLACE: &[Allow] = &[
    // Their reference places, outside every packet and call.
    Allow {
        frame: "net::handlers::dispatch",
        why: "a packet's handler that signals in place, inside its packet, as the reference's \
              handlers run whole (`0x537ae5`) and signal inside themselves (`0x703e50`)",
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
    Allow {
        frame: "ui_social::feed::feed_social",
        why: "a new VM's seed, which no packet makes: the list events the handlers raised before \
              it was up (`0x5ae867`, `0x5ae92d`)",
    },
    // Rows of the guard's lists.
    Allow {
        frame: "ui_script::glue_mgr::drive_glue_mgr",
        why: "AT_THEIR_OWN_PLACE: the login and character screens' answers",
    },
    Allow {
        frame: "bindings::sync_dispatch",
        why: "NOT_YET_ON_A_LANE: `::account_data::AccountData`, whose binding sets' read owes the \
              `UPDATE_BINDINGS` the copy into the live table signals (`0x4b649d`)",
    },
    Allow {
        frame: "ui_action::feed::feed_actions",
        why: "NOT_YET_ON_A_LANE: `::ui_action::ActionSlots`, which it resolves and signals \
              `ACTIONBAR_SLOT_CHANGED` off",
    },
    Allow {
        frame: "ui_action::state::fire_action_events",
        why: "NOT_YET_ON_A_LANE: the usable cache's refresh, one pass a frame",
    },
    Allow {
        frame: "capture::fixtures::seed_ui_fixture",
        why: "QUEUED_FOR_THE_TICK: a capture's fixture",
    },
    Allow {
        frame: "capture::probe_service::service_probe",
        why: "QUEUED_FOR_THE_TICK: a probe's scripted sale",
    },
    Allow {
        frame: "target::click::talk",
        why: "QUEUED_FOR_THE_TICK: the vendor arm's sale",
    },
    Allow {
        frame: "ui_world_map::feed_landmarks",
        why: "QUEUED_FOR_THE_TICK: the landmark list's rebuild",
    },
    Allow {
        frame: "ui_world_map::feed_world_map",
        why: "QUEUED_FOR_THE_TICK: the first world enter's selection",
    },
    Allow {
        frame: "ui_world_map::install_world_loc_projector",
        why: "QUEUED_FOR_THE_TICK: the projector's install",
    },
    Allow {
        frame: "ui_world_map::seed_world_map_catalog",
        why: "QUEUED_FOR_THE_TICK: the catalog and the instance maps' push",
    },
    // Remaining work no guard row names yet.
    Allow {
        frame: "ui_char::feed_char",
        why: "remaining work: `set_inventory_slots` signals `UPDATE_INVENTORY_ALERTS` off the \
              frame's doll, where the reference recomputes and signals (`0x4c7ee0`, `0x4c8085`) in \
              the item field watchers (`0x5d952a`, `0x5d97aa`, `0x5d9920`, `0x5d9b2f`, \
              `0x5dee7a`), inside the update packet",
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
    benilla_ui::script::origin::arm_until_disarmed(RAISES_AT_THEIR_OWN_PLACE);
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
