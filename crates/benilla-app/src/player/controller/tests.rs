//! [`super::control`] in the whole client, headless, frame by frame: the press the cast cancel
//! reads and the autorun latch it is read against.

use benilla_ui::script::{BindingInput, FiredInput, HeldInput};
use bevy::ecs::system::{RunSystemOnce, SystemId};
use bevy::window::{CursorOptions, PrimaryWindow};

use super::*;
use crate::bindings::BindingsState;
use crate::net::{GuidIndex, SelfGuid, SelfPlayer};
use crate::spell::LocalMoveStart;

use BindingInput::{Fire, Start};

const ME: u64 = 0xAA;
const CREATURE: u64 = 0xBB;

/// The body the client is attached to.
#[derive(Clone, Copy, PartialEq)]
enum Body {
    /// Our own character.
    Own,
    /// A creature we possess, streamed in and seized.
    Possessed,
    /// A claimed mover not yet streamed in: the reseat stays unresolved.
    Unstreamed,
}

/// The whole client with us in the world, a window and the startup's camera, and the body placed
/// and steered each frame by [`super::super::embody::maintain_embodiment`], which the schedule
/// runs ahead of the controller.
struct Client {
    app: App,
    embody: SystemId,
    drive: SystemId,
}

impl Client {
    /// `set` applies to [`Player`] before the first placement.
    fn new(body: Body, set: impl FnOnce(&mut Player)) -> Self {
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
        let me = world.spawn((Transform::default(), SelfPlayer)).id();
        world.insert_resource(SelfGuid(Some(ME)));
        world.get_resource_or_init::<GuidIndex>().0.insert(ME, me);
        if body == Body::Possessed {
            let creature = world.spawn(Transform::default()).id();
            world
                .resource_mut::<GuidIndex>()
                .0
                .insert(CREATURE, creature);
        }
        {
            let mut player = world.resource_mut::<Player>();
            player.active = true;
            if body != Body::Own {
                player.foreign_mover = Some(CREATURE);
            }
            set(&mut player);
        }
        let embody = world.register_system(super::super::embody::maintain_embodiment);
        let drive = world.register_system(control);
        world.run_system(embody).expect("the embodiment runs");
        // `wire_in` seizes a streamed body (`0x6006e0`); an unstreamed claim stays a reseat.
        if body != Body::Unstreamed {
            world.resource_mut::<Player>().reseat = false;
        }
        Self { app, embody, drive }
    }

    /// One frame, its binding calls after `earlier`'s: the press it left, as whether the player
    /// was its mover.
    fn frame(&mut self, earlier: Vec<BindingInput>, now: Vec<BindingInput>) -> Option<bool> {
        let world = self.app.world_mut();
        world.run_system(self.embody).expect("the embodiment runs");
        *world.resource_mut::<BindingsState>() = BindingsState::after_calls(earlier, now);
        world.run_system(self.drive).expect("the controller runs");
        // Taken, as `local_self_cancel` takes it.
        let mut press = world.resource_mut::<LocalMoveStart>();
        std::mem::take(&mut press.pressed).then_some(press.mover_is_player)
    }

    fn player(&mut self) -> Mut<'_, Player> {
        self.app.world_mut().resource_mut::<Player>()
    }
}

/// One frame's press on a fresh client.
fn press(body: Body, set: impl FnOnce(&mut Player), now: Vec<BindingInput>) -> Option<bool> {
    Client::new(body, set).frame(vec![], now)
}

fn forward() -> Vec<BindingInput> {
    vec![Start(HeldInput::MoveForward)]
}

fn toggle() -> Vec<BindingInput> {
    vec![Fire(FiredInput::ToggleAutoRun)]
}

/// A root empties the streamed word of direction bits, but the press cancels: nothing between
/// `0x5150a7` and the cancel `0x51512a` tests root or stun.
#[test]
fn a_press_while_rooted_cancels_the_cast() {
    let rooted = |p: &mut Player| p.modes.rooted = true;
    assert_eq!(press(Body::Own, rooted, forward()), Some(true));
}

/// The cancel runs at the press whoever drives the body: only the taint gate `0x494a50` stands
/// between the press `0x5150a7` and the cancel `0x51512a`. The channel's cancel compares the
/// player with the active mover `0xc4da98` (`0x51517b`-`0x515191`), which lost control zeroes
/// (`0x5fa600`) and a server ride keeps.
#[test]
fn a_press_while_the_body_is_not_ours_to_drive_cancels_the_cast() {
    assert_eq!(
        press(Body::Own, |p| p.control_lost = true, forward()),
        Some(false),
        "control lost: the cast, not the channel"
    );
    assert_eq!(
        press(Body::Own, |p| p.server_riding = true, forward()),
        Some(true),
        "a server ride"
    );
}

/// Steering a possessed unit cancels our cast (`0x5150f2` tests no mover) and marks the press as
/// not the player's, which spares the channel (`0x515180`).
#[test]
fn a_press_on_a_possessed_unit_cancels_the_cast_and_spares_the_channel() {
    assert_eq!(press(Body::Possessed, |_| {}, forward()), Some(false));
}

/// `Script::Jump` resolves the active mover and leaves without one (`0x513be0`-`0x513c02`): lost
/// control zeroes it (`0x5fa600`) and an unstreamed claim resolves nothing. The move keys'
/// handler needs none.
#[test]
fn jump_cancels_only_with_a_mover() {
    let jump = || vec![Fire(FiredInput::Jump)];
    assert_eq!(press(Body::Own, |_| {}, jump()), Some(true), "our body");
    assert_eq!(
        press(Body::Possessed, |_| {}, jump()),
        Some(false),
        "a possessed body"
    );
    assert_eq!(
        press(Body::Own, |p| p.control_lost = true, jump()),
        None,
        "control lost"
    );
    assert_eq!(press(Body::Unstreamed, |_| {}, jump()), None, "no mover");
    assert_eq!(
        press(Body::Unstreamed, |_| {}, forward()),
        Some(false),
        "a key with no mover"
    );
}

/// `ToggleAutoRun` reads the held bit and presses or releases it whatever the control state
/// (`0x513de7`, the press storing it at `0x514907`, the release at `0x514b70`). With control lost
/// the tick finds no unit and leaves at `0x51466e`, so the bit survives: the presses alternate,
/// the on press cancelling and the off press not, and `SetActiveMover` runs the tick on the
/// returned mover (`0x60084d`), so the latch runs.
#[test]
fn the_latch_toggles_while_control_is_lost_and_runs_on_its_return() {
    let mut client = Client::new(Body::Own, |p| p.control_lost = true);
    assert_eq!(client.frame(vec![], toggle()), Some(false), "on");
    assert_eq!(client.frame(vec![], toggle()), None, "off");
    assert_eq!(client.frame(vec![], toggle()), Some(false), "on again");
    client.player().control_lost = false;
    assert_eq!(client.frame(vec![], vec![]), None);
    let player = client.player();
    assert!(player.autorun, "the latch outlives the loss of control");
    assert_ne!(
        player.move_flags() & crate::creature_anim::move_flags::FORWARD,
        0,
        "and runs"
    );
}

/// An unresolved reseat resolves no unit either (`0x51466e`): the latch toggles as it does with
/// control lost.
#[test]
fn the_latch_toggles_while_the_claimed_mover_is_unstreamed() {
    let mut client = Client::new(Body::Unstreamed, |_| {});
    assert_eq!(client.frame(vec![], toggle()), Some(false), "on");
    assert_eq!(client.frame(vec![], toggle()), None, "off");
    assert!(!client.player().autorun);
}

/// A server ride keeps the mover, but its spline fails `0x5144e0`, so the tick (run on the
/// mover's monster move, `0x602053`, and after every press, `0x5151a0`) clears the bit on its
/// may-not-translate leg (`0x514748`): every press there is an on press, and the ride ends with
/// autorun off.
#[test]
fn a_server_ride_clears_the_latch_so_every_toggle_cancels() {
    let mut client = Client::new(Body::Own, |p| {
        p.autorun = true;
        p.server_riding = true;
    });
    assert_eq!(client.frame(vec![], vec![]), None);
    assert!(!client.player().autorun, "the ride's tick clears autorun");
    for n in 1..=3 {
        assert_eq!(client.frame(vec![], toggle()), Some(true), "press {n}");
    }
    assert!(!client.player().autorun);
}
