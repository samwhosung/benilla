//! [`super::control`] run once in the whole client, headless: the press the cast cancel reads.

use benilla_ui::script::{BindingInput, FiredInput, HeldInput};
use bevy::ecs::system::RunSystemOnce;
use bevy::window::{CursorOptions, PrimaryWindow};

use super::*;
use crate::bindings::BindingsState;
use crate::spell::LocalMoveStart;

use BindingInput::{Fire, Start};

/// One controller frame: us in the world with a window and the startup's camera, our body unless
/// `bodiless`, `set` applied to [`Player`], and this frame's binding calls after `earlier`'s. The
/// press the frame left, and whether the player was its mover.
fn frame(
    earlier: Vec<BindingInput>,
    now: Vec<BindingInput>,
    bodiless: bool,
    set: impl FnOnce(&mut Player),
) -> (bool, bool) {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let world = app.world_mut();
    // The startup's player, rig and camera, with no client data, the empty world's liquids and
    // rooms, and a session to send on.
    world
        .run_system_once(super::super::setup::setup_player)
        .expect("the player sets up");
    benilla_world::world_point::init_world_point_resources(world);
    world.insert_resource(NetCommands(crossbeam_channel::unbounded().0));
    world.spawn((Window::default(), CursorOptions::default(), PrimaryWindow));
    if !bodiless {
        world.spawn((Transform::default(), crate::net::Embodied));
    }
    *world.resource_mut::<BindingsState>() = BindingsState::after_calls(earlier, now);
    {
        let mut player = world.resource_mut::<Player>();
        player.active = true;
        set(&mut player);
    }
    world.run_system_once(control).expect("the controller runs");
    let press = world.resource::<LocalMoveStart>();
    (press.pressed, press.mover_is_player)
}

fn forward() -> Vec<BindingInput> {
    vec![Start(HeldInput::MoveForward)]
}

/// A root empties the streamed word of direction bits, but the press cancels: nothing between
/// `0x5150a7` and the cancel `0x51512a` tests root or stun.
#[test]
fn a_press_while_rooted_cancels_the_cast() {
    let rooted = |p: &mut Player| p.modes.rooted = true;
    assert_eq!(frame(vec![], forward(), false, rooted), (true, true));
}

/// The cancel runs at the press whoever drives the body: only the taint gate `0x494a50` stands
/// between the press `0x5150a7` and the cancel `0x51512a`, so a feared or charmed body and a
/// server ride cancel too.
#[test]
fn a_press_while_the_body_is_not_ours_to_drive_cancels_the_cast() {
    assert_eq!(
        frame(vec![], forward(), false, |p| p.control_lost = true),
        (true, true),
        "control lost"
    );
    assert_eq!(
        frame(vec![], forward(), false, |p| p.server_riding = true),
        (true, true),
        "a server ride"
    );
}

/// Steering a possessed unit cancels our cast (`0x5150f2` tests no mover) and marks the press as
/// not the player's, which spares the channel (`0x515180`).
#[test]
fn a_press_on_a_possessed_unit_cancels_the_cast_and_spares_the_channel() {
    let possessing = |p: &mut Player| p.foreign_mover = Some(0xBB);
    assert_eq!(frame(vec![], forward(), false, possessing), (true, false));
}

/// `Script::Jump` leaves before its cancel when there is no mover (`0x513c02`); the move keys'
/// handler needs none.
#[test]
fn jump_cancels_only_with_a_mover() {
    let jump = || vec![Fire(FiredInput::Jump)];
    assert!(frame(vec![], jump(), false, |_| {}).0, "with our body");
    assert!(!frame(vec![], jump(), true, |_| {}).0, "with no mover");
    assert!(
        frame(vec![], forward(), true, |_| {}).0,
        "a key with no mover"
    );
}
