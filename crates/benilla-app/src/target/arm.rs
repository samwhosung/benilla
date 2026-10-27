//! The world click as the mouse commands make it: `CameraOrSelectOrMoveStart` and
//! `TurnOrActionStart` arm it and pick the world under the press (`ArmClick 0x514810` →
//! `PickObjectUnderCursor 0x481f00` → `PickAtCursor 0x481190`), and the last primary's release
//! dispatches it (`ReleaseCommands 0x514b70` → `DispatchWorldClick 0x481f60`), all inside the
//! command's call. The VM keeps the command bits and hands the steps on in call order
//! ([`benilla_ui::script::ClickCall`]); [`world_steps`] applies them at the record whose binding
//! made them, before the next record, so a UI click and a world click in one frame resolve in the
//! order they came.
//!
//! Deviation: the click's legs, which are engine systems ([`WorldClickLegs`]), run as the record's
//! walk returns, not inside the call. Lua after the command in the same binding body, or a hook
//! around it, reads the world as the click found it, where the reference's reads it as the click
//! left it.

use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

use benilla_ui::script::{ClickCall, ClickMode};
use benilla_world::interact::{WorldClick, WorldRightClick};

use super::PressPick;
use crate::player::PressGesture;
use crate::ui_script::{InputRecord, Record};

/// The click a mouse command armed: its mode (`CInputControl+0x18`) and the press's gesture, the
/// press time (`+0x14`) and the travel since (`+0x08`, `+0x0c`), which the release's drag test
/// reads (`0x514ae0`). The look session adds the travel.
#[derive(Resource, Default)]
pub(crate) struct ClickArm {
    armed: Option<(ClickMode, PressGesture)>,
}

impl ClickArm {
    /// Mode 0 stored: nothing dispatches at the release.
    pub(crate) fn disarm(&mut self) {
        self.armed = None;
    }

    /// One motion's camera travel, while a click is armed (`0x514400`).
    pub(crate) fn add_travel(&mut self, yaw: f32, pitch: f32) {
        if let Some((_, gesture)) = &mut self.armed {
            gesture.add_travel(yaw, pitch);
        }
    }
}

/// The world click's legs, the dispatcher's in its order (`0x481f60`): the payload, the trade
/// leg, the selects, the cursor's commits, the right-click's action, the binds of the units the
/// selects parked, the interacts, the pet's feed and the casts the dispatch queued. Run after the
/// record that clicked, and once more where the frame's records end for what no record made (a
/// crate's `Interact`, a `DropItemOnUnit`, a select a packet parked).
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct WorldClickLegs;

/// The press's pick at `at`, in window coordinates ([`super::hover::PointPick`]).
fn pick_at(In(at): In<Option<Vec2>>, mut pick: super::hover::PointPick) -> PressPick {
    pick.at(at)
}

/// Run a one-shot of the click on `world`, the unit edits it queued applied after.
fn run<M>(world: &mut World, system: impl IntoSystem<(), (), M> + 'static) {
    if let Err(err) = crate::script_calls::run_cached(world, system) {
        debug!("world click: a step did not run: {err}");
    }
}

/// Trace the press's pick at `at` into [`PressPick`], against the targeting word as it stands
/// now: a right press's cancel may have ended it, so its readers are published first.
fn latch(world: &mut World, at: Option<Vec2>) {
    run(world, crate::spell::targeting::publish_picks_self);
    run(world, crate::spell::targeting::publish_corpse_pick);
    run(world, crate::spell::targeting::publish_unit_pick);
    run(world, crate::spell::targeting::publish_go_pick);
    // `WOW_PICK_COST=1` logs each press pick's cost.
    static METER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let t0 = METER
        .get_or_init(|| std::env::var("WOW_PICK_COST").as_deref() == Ok("1"))
        .then(std::time::Instant::now);
    let picked = world.run_system_cached_with(pick_at, at);
    if let Some(t0) = t0 {
        info!("pick-cost: press pick {} us", t0.elapsed().as_micros());
    }
    match picked {
        Ok(pick) => *world.resource_mut::<PressPick>() = pick,
        Err(err) => debug!("world click: the press pick did not run: {err}"),
    }
}

/// Apply the click steps `calls` made at the cursor `at`: an arm traces the pick and starts the
/// gesture, a disarm drops it, and the release dispatches the armed click when it is no drag.
/// Whether a click is due for the legs.
fn apply(world: &mut World, calls: Vec<ClickCall>, at: Option<Vec2>) -> bool {
    let mut due = false;
    for call in calls {
        let now = world.resource::<Time>().elapsed_secs();
        // The world enter behind the cover releases everything (`0x5144c0`); a gesture in flight
        // is dropped, so the release clicks nothing in the world just left.
        let covering = world
            .get_resource::<crate::loading_screen::LoadingScreen>()
            .is_some_and(|c| c.covering());
        match call {
            ClickCall::Arm(mode) => {
                // A left press the payload's drop consumed, or the inspector's, selects nothing.
                let refused = mode == ClickMode::Select
                    && (world
                        .get_resource::<crate::ui_script::PlayerUiClickConsumed>()
                        .is_some_and(|c| c.0)
                        || world
                            .get_resource::<crate::ui_script::InspectMode>()
                            .is_some_and(|i| i.enabled));
                if covering || refused {
                    world.resource_mut::<ClickArm>().disarm();
                    continue;
                }
                latch(world, at);
                world.resource_mut::<ClickArm>().armed = Some((mode, PressGesture::new(now)));
            }
            ClickCall::Disarm => world.resource_mut::<ClickArm>().disarm(),
            ClickCall::Release => {
                let armed = world.resource_mut::<ClickArm>().armed.take();
                let Some((mode, mut gesture)) = armed else {
                    continue;
                };
                // The frame's motion, which reaches the look session only after the records: the
                // reference's motion messages reach `0x514400` ahead of the release's test.
                let (yaw, pitch) = release_frame_travel(world);
                gesture.add_travel(yaw, pitch);
                if covering || !gesture.is_click(now) {
                    continue;
                }
                // `0x514d8b` dispatches action 1 for mode 1, `0x514d77` action 4 for mode 2.
                match mode {
                    ClickMode::Select => {
                        world.write_message(WorldClick);
                    }
                    ClickMode::Action => {
                        world.write_message(WorldRightClick);
                    }
                }
                due = true;
            }
        }
    }
    due
}

/// The camera travel the frame's mouse motion charges a click, as the look session charges it: none
/// while the window lacks the focus, whose motion is the pointer travelling in another app.
fn release_frame_travel(world: &mut World) -> (f32, f32) {
    let focused = world
        .query_filtered::<&Window, With<bevy::window::PrimaryWindow>>()
        .single(world)
        .is_ok_and(|w| w.focused);
    let (Some(motion), Some(look)) = (
        world.get_resource::<bevy::input::mouse::AccumulatedMouseMotion>(),
        world.get_resource::<crate::player::LookConfig>(),
    ) else {
        return (0.0, 0.0);
    };
    if !focused {
        return (0.0, 0.0);
    }
    look.click_travel(motion.delta)
}

/// Whether the world frame took a press: one no frame took, or any while a look session holds the
/// cursor (in freelook every button goes to the world frame, `0x492b50`).
fn world_press(world: &World, over_ui: bool) -> bool {
    let in_world = world
        .get_resource::<State<crate::client_state::ClientState>>()
        .is_some_and(|s| *s.get() == crate::client_state::ClientState::InWorld);
    let looking = world
        .get_resource::<crate::player::CameraControl>()
        .is_some_and(|rig| rig.is_looking());
    in_world && (looking || !over_ui)
}

/// The world frame's right press (`CGWorldFrame::OnMouseDown 0x483c40`), when the world frame took
/// it: its hook `0x492c20` at `0x483c62`, ahead of the binding (`0x483c93`), so the binding and
/// the pick its arm traces read what the hook left. The hook's legs in its order: a pending target
/// cancelled (`0x492c26`-`0x492c3f`), a pending gift wrap's cursor cleared (`0x492c44`,
/// `0x492c54`), a Repair base mode back to Point (`0x492c59`-`0x492c72`). It tests no payload and
/// consumes nothing (`0x492c77`), so the binding runs. Then the targeting word as the hook left it
/// goes to the VM, for the binding's `SpellIsTargeting`.
pub(crate) fn right_press(world: &mut World, over_ui: bool) {
    if !world_press(world, over_ui) {
        return;
    }
    run(
        world,
        crate::spell::targeting::cancel_targeting_on_right_press,
    );
    run(world, crate::ui_items::clear_wrap_on_right_press);
    run(world, crate::ui_merchant::end_repair_mode_on_right_press);
    run(world, crate::spell::targeting::feed_targeting_to_vm);
}

/// The world's steps of one input record, as its walk returns and before the next record: the
/// click steps its binding's mouse commands made and the click's legs when the record clicked the
/// world, and a press on a nameplate's pick. A right press's hook ran ahead of the binding
/// ([`right_press`]).
pub(crate) fn world_steps(world: &mut World, record: &InputRecord) {
    let press = match record.record {
        Record::Button {
            button,
            state: bevy::input::ButtonState::Pressed,
        } => Some(button),
        _ => None,
    };
    let calls = crate::ui_script::take_click_calls(world);
    let armed = calls.iter().any(|c| matches!(c, ClickCall::Arm(_)));
    if press.is_some() {
        // The plate the walk left hovered, as the record's press found it.
        run(world, crate::vplates::seed_plate_hover);
    }
    if apply(world, calls, record.at) {
        world.run_schedule(WorldClickLegs);
    }
    // A press a nameplate took: its click acts on the plate's unit (`0x7cb910` → `0x4949f0`), which
    // the pick names, classified as the press found it.
    let plate = world
        .get_resource::<crate::vplates::PlateHover>()
        .is_some_and(|h| h.0.is_some());
    if matches!(press, Some(MouseButton::Left | MouseButton::Right)) && plate && !armed {
        latch(world, record.at);
    }
}

/// The frame's walk begins: the plate clicks the last frame's plates completed act first, on the
/// pick their press latched, ahead of any press of this frame's.
pub(crate) fn plate_clicks_first(world: &mut World) {
    if world
        .get_resource::<crate::vplates::PlateClicks>()
        .is_some_and(|c| !c.left.is_empty() || !c.right.is_empty())
    {
        world.run_schedule(WorldClickLegs);
    }
}

/// Where the frame's records end: the click steps made since the last record's (a packet
/// handler's `MouselookStart`, a stuck-latch sweep's release), then the legs once more for what no
/// record made.
pub(crate) fn run_world_clicks(world: &mut World) {
    let calls = crate::ui_script::take_click_calls(world);
    apply(world, calls, None);
    world.run_schedule(WorldClickLegs);
}

#[cfg(test)]
#[path = "arm_tests.rs"]
mod tests;
