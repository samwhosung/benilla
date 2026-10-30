//! [`super::control`] in the whole client, headless, frame by frame, after the frame's mouse input
//! ([`super::sample_look`]) and its command presses ([`super::super::press`]) as the schedule runs
//! them: the press the cast cancel reads, the autorun latch it is read against, and where in the
//! frame a press's messages go out.

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
    sample: SystemId,
    commands: SystemId,
    cancel: SystemId,
    press: SystemId,
    drive: SystemId,
    /// What the frames sent, in order.
    sent: crossbeam_channel::Receiver<ClientCommand>,
    /// The position each move [`Client::traced`] read off it carried, in order.
    poses: Vec<(crate::net::MoveKind, [f32; 3])>,
}

impl Client {
    /// `set` applies to [`Player`] before the first placement.
    fn new(body: Body, set: impl FnOnce(&mut Player)) -> Self {
        Self::with_lock(body, set, |_| {})
    }

    /// [`Self::new`], with `lock` applied to the mover's lock too.
    fn with_lock(
        body: Body,
        set: impl FnOnce(&mut Player),
        lock: impl FnOnce(&mut super::super::MoverLock),
    ) -> Self {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let world = app.world_mut();
        // The startup's player, rig and camera, with no client data, the empty world's liquids and
        // rooms, and a session to send on.
        world
            .run_system_once(super::super::setup::setup_player)
            .expect("the player sets up");
        benilla_world::world_point::init_world_point_resources(world);
        let (tx, sent) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
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
            set(&mut player);
            let mut mover = world.resource_mut::<super::super::MoverLock>();
            if body != Body::Own {
                mover.foreign_mover = Some(CREATURE);
            }
            lock(&mut mover);
        }
        let embody = world.register_system(super::super::embody::maintain_embodiment);
        let sample = world.register_system(sample_look);
        let commands = world.register_system(super::super::press::press_commands);
        let cancel = world.register_system(crate::spell::local_self_cancel);
        let press = world.register_system(super::super::press::press_tick);
        let drive = world.register_system(control);
        world.run_system(embody).expect("the embodiment runs");
        // `wire_in` seizes a streamed body (`0x6006e0`); an unstreamed claim stays a reseat.
        if body != Body::Unstreamed {
            world.resource_mut::<super::super::MoverLock>().reseat = false;
        }
        Self {
            app,
            embody,
            sample,
            commands,
            cancel,
            press,
            drive,
            sent,
            poses: Vec::new(),
        }
    }

    /// One frame, its binding calls after `earlier`'s: the press it left, as whether the player
    /// was its mover. The mouse's input runs with the bindings, ahead of the frame time's
    /// embodiment and controller.
    fn frame(&mut self, earlier: Vec<BindingInput>, now: Vec<BindingInput>) -> Option<bool> {
        let world = self.app.world_mut();
        *world.resource_mut::<BindingsState>() = BindingsState::after_calls(earlier, now);
        world
            .run_system(self.sample)
            .expect("the mouse's input runs");
        world
            .run_system(self.commands)
            .expect("the press's commands run");
        world.run_system(self.press).expect("the press runs");
        world.run_system(self.embody).expect("the embodiment runs");
        world.run_system(self.drive).expect("the controller runs");
        // Taken, as `local_self_cancel` takes it.
        let mut press = world.resource_mut::<LocalMoveStart>();
        std::mem::take(&mut press.pressed).then_some(press.mover_is_player)
    }

    fn player(&mut self) -> Mut<'_, Player> {
        self.app.world_mut().resource_mut::<Player>()
    }

    fn lock(&mut self) -> Mut<'_, super::super::MoverLock> {
        self.app
            .world_mut()
            .resource_mut::<super::super::MoverLock>()
    }
}

/// One frame's press on a fresh client.
fn press(body: Body, set: impl FnOnce(&mut Player), now: Vec<BindingInput>) -> Option<bool> {
    Client::new(body, set).frame(vec![], now)
}

/// [`press`] with control of the body lost.
fn press_lost(body: Body, now: Vec<BindingInput>) -> Option<bool> {
    Client::with_lock(body, |_| {}, |l| l.control_lost = true).frame(vec![], now)
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
        press_lost(Body::Own, forward()),
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
    assert_eq!(press_lost(Body::Own, jump()), None, "control lost");
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
    let mut client = Client::with_lock(Body::Own, |_| {}, |l| l.control_lost = true);
    assert_eq!(client.frame(vec![], toggle()), Some(false), "on");
    assert_eq!(client.frame(vec![], toggle()), None, "off");
    assert_eq!(client.frame(vec![], toggle()), Some(false), "on again");
    client.lock().control_lost = false;
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

/// The water-walk and hover acks go out through `0x60e0a0` (`0x6163c0` push 0x2d0, `0x616316` and
/// `0x616364` push 0xf6, into `0x6163cf`), whose table sends neither at once, so each ends the
/// mover's precast (`0x60e15d`); the root, unroot and feather-fall acks (0xe9, 0xeb, 0x2cf) are
/// exempt (`0x60e0f1`-`0x60e137`).
#[test]
fn a_water_walk_or_hover_ack_ends_the_precast_and_a_root_or_feather_fall_ack_does_not() {
    use benilla_protocol::MoveMode;
    for (mode, apply, ends) in [
        (MoveMode::WaterWalk, true, true),
        (MoveMode::WaterWalk, false, true),
        (MoveMode::Hover, true, true),
        (MoveMode::Hover, false, true),
        (MoveMode::Root, true, false),
        (MoveMode::Root, false, false),
        (MoveMode::FeatherFall, true, false),
    ] {
        const CORPSE: u64 = 0xF130_0000_0000_0042;
        let mut client = Client::new(Body::Own, |_| {});
        let world = client.app.world_mut();
        let me = world.resource::<GuidIndex>().0[&ME];
        world.entity_mut(me).insert(crate::creature_anim::Precast {
            spell_id: 133,
            busy: true,
        });
        world.resource_mut::<crate::ui_loot::LootState>().open(
            CORPSE,
            benilla_protocol::messages::loot_type::CORPSE,
            5,
            vec![],
        );
        world.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(CORPSE);
        world.write_message(crate::net::MoveModeMessage {
            guid: ME,
            counter: 1,
            mode,
            apply,
        });
        client.frame(vec![], vec![]);
        let world = client.app.world_mut();
        world.flush();
        assert_eq!(
            world.get::<crate::creature_anim::Precast>(me).is_none(),
            ends,
            "{mode:?} {}",
            if apply { "on" } else { "off" }
        );
        // The end's tail releases our loot (`0x60d10b`), precast or not, as the ack's end applies:
        // the move-start close while the body falls through this empty world, the full one at
        // rest, either of which takes a corpse's window.
        let closed = world
            .resource::<crate::ui_loot::LootState>()
            .source()
            .is_none();
        assert_eq!(closed, ends, "{mode:?}: the loot release");
        let released = client
            .sent
            .try_iter()
            .any(|c| matches!(c, ClientCommand::LootRelease { guid } if guid == CORPSE));
        assert_eq!(released, ends, "{mode:?}: released as it closes");
    }
}

/// The pitch keys' bits pitch the body wherever they survive (`0x7c53b9` tests `0xc0` in any
/// mode, `0x7c4f80`), so a pitch held out of the water keeps pitching on land and rides the word;
/// a root's `and 0xffe07f00` (`0x7c734f`) clears them.
#[test]
fn the_pitch_keys_pitch_a_body_out_of_the_water_until_a_root() {
    use crate::creature_anim::move_flags;
    use benilla_protocol::MoveMode;
    let mut client = Client::new(Body::Own, |p| p.pitch_keys = Some(true));
    let step = |client: &mut Client| {
        client
            .app
            .world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(100));
        client.frame(vec![], vec![]);
    };
    step(&mut client);
    let pitched = client.player().mover_pitch;
    assert!(pitched > 0.0, "the key pitches on land: {pitched}");
    assert_eq!(client.player().move_flags & 0xc0, move_flags::PITCH_UP);
    client
        .app
        .world_mut()
        .write_message(crate::net::MoveModeMessage {
            guid: ME,
            counter: 1,
            mode: MoveMode::Root,
            apply: true,
        });
    step(&mut client);
    assert_eq!(client.player().pitch_keys, None, "the root clears them");
    let rooted = client.player().mover_pitch;
    step(&mut client);
    assert_eq!(client.player().mover_pitch, rooted);
    assert_eq!(client.player().move_flags & 0xc0, 0);
}

/// The press that completes both mouse bits hands the camera's pitch to the body with no motion
/// (`0x514924`-`0x514966` → `0x5103e0`), which clears the pitch keys (`0x7c6f70`).
#[test]
fn the_both_button_press_hands_the_pitch_off() {
    use benilla_ui::script::HeldInput;
    let mut client = Client::new(Body::Own, |p| {
        p.pitch_keys = Some(true);
        p.mover_pitch = 0.5;
    });
    client.frame(
        vec![],
        vec![
            Start(HeldInput::TurnOrAction),
            Start(HeldInput::CameraOrSelectOrMove),
        ],
    );
    let player = client.player();
    assert_eq!(player.pitch_keys, None);
    assert_ne!(player.mover_pitch, 0.5, "the aim took it");
}

/// A `MouselookStart` a packet's handler makes after the frame's look session, completing both
/// mouse bits over a left drag, hands the camera's facing and pitch to the body in the frame's
/// controller, as the reference's press handler does inside the call (`0x514924`-`0x514966` →
/// `0x5103e0`).
#[test]
fn a_late_mouselook_start_over_a_left_drag_hands_the_facing_and_pitch_off() {
    let mut client = Client::new(Body::Own, |p| {
        p.pitch_keys = Some(true);
        p.mover_pitch = 0.5;
        p.face_yaw = 0.0;
    });
    let (sample, commands, press, embody, drive) = (
        client.sample,
        client.commands,
        client.press,
        client.embody,
        client.drive,
    );
    let world = client.app.world_mut();
    *world.resource_mut::<BindingsState>() = BindingsState::after_calls(vec![], vec![]);
    world.run_system(sample).expect("the mouse's input runs");
    world
        .run_system(commands)
        .expect("the press's commands run");
    world.run_system(press).expect("the press runs");
    world
        .query_filtered::<&mut FlyCam, With<Camera>>()
        .single_mut(world)
        .expect("the camera")
        .yaw = 1.0;
    world.resource_scope(|world, mut binds: Mut<BindingsState>| {
        use benilla_ui::script::LookCall;
        binds.apply_unhosted(Start(HeldInput::CameraOrSelectOrMove));
        let mut rig = world.resource_mut::<CameraControl>();
        rig.world_mouse
            .late_call_for_test(LookCall::Mouselook(true), &mut binds);
    });
    world.run_system(embody).expect("the embodiment runs");
    world.run_system(drive).expect("the controller runs");
    let player = client.player();
    assert_eq!(player.face_yaw, 1.0, "the body took the camera's facing");
    assert_eq!(player.pitch_keys, None, "and its pitch");
}

/// The filter [`super::self_seat`] reads a deck's pose through.
type Decks = (
    Without<crate::net::Embodied>,
    Without<FlyCam>,
    Without<Camera>,
);

/// A create on a transport seats us only on a game object, the type mask 0x20 the lookup asks
/// (`0x468460`), whose flags carry the transport bit 0x8 (`0x618b30` → `0x630a80`); on any other
/// the login pose stands.
#[test]
fn a_login_seats_us_only_on_a_game_object_flagged_as_a_transport() {
    use crate::transport::TransportRider;
    const BOAT: u64 = 0xF120_0000_0000_0010;
    let seat = |mask: u32, flags: u32| {
        let mut world = World::new();
        let boat = world
            .spawn((
                Transform::from_xyz(1.0, 2.0, 3.0),
                // `OBJECT_FIELD_TYPE`, field 2, and `GAMEOBJECT_FLAGS`, field 9.
                crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (2, mask),
                    (9, flags),
                ])),
            ))
            .id();
        world.spawn((
            crate::net::Embodied,
            TransportRider {
                transport_guid: BOAT,
                local_pos: [4.0, 5.0, 6.0],
                local_orientation: 0.5,
            },
        ));
        world.insert_resource(GuidIndex([(BOAT, boat)].into_iter().collect()));
        world
            .run_system_once(
                |riders: Query<&TransportRider, With<crate::net::Embodied>>,
                 index: Res<GuidIndex>,
                 decks: Query<&Transform, Decks>,
                 stores: Query<&crate::net::ObjectStore>| {
                    super::self_seat(&(riders, index, decks, stores))
                        .map(|s| (s.entity, s.guid, s.local_pos, s.local_orientation))
                },
            )
            .unwrap()
            .map(|(_, guid, pos, o)| (guid, pos, o))
    };
    assert_eq!(seat(0x21, 0x8 | 0x20), Some((BOAT, [4.0, 5.0, 6.0], 0.5)));
    assert_eq!(seat(0x21, 0x20), None, "no transport bit");
    assert_eq!(seat(0x09, 0x8), None, "no game object");
}

/// Where in the frame a command went out: in the input records, ahead of the packets, or in the
/// frame time after them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum When {
    Input,
    FrameTime,
}

/// One sent command, as these tests read it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sent {
    Move(crate::net::MoveKind, u32),
    Ack(benilla_protocol::MoveMode),
    Stand(u32),
    CancelCast,
}

/// A frame's length, which the 500 ms heartbeat counts in.
const FRAME_MS: u64 = 16;

impl Client {
    /// [`Client::new`] standing at rest on a flat floor, with nothing sent yet: the terrain gate
    /// off, as in an app with no streamer, so the mover resolves against the floor.
    fn standing(body: Body) -> Self {
        use avian3d::prelude::{Collider, RigidBody};
        let mut client = Self::new(body, |p| p.pos = Vec3::new(0.0, 0.5, 0.0));
        let world = client.app.world_mut();
        let corner = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        world.spawn((
            RigidBody::Static,
            Collider::trimesh(
                vec![
                    corner(-50.0, -50.0),
                    corner(50.0, -50.0),
                    corner(50.0, 50.0),
                    corner(-50.0, 50.0),
                ],
                vec![[0u32, 2, 1], [0, 3, 2]],
            ),
            Transform::default(),
        ));
        world.remove_resource::<benilla_world::terrain_stream::TerrainResidency>();
        world.run_schedule(avian3d::schedule::PhysicsSchedule);
        for _ in 0..60 {
            client.traced(vec![], vec![], |_| {});
        }
        let player = client.player();
        assert!(
            player.airborne_since.is_none() && player.move_flags == 0,
            "the body rests on the floor"
        );
        client.poses.clear();
        client
    }

    /// One frame of [`FRAME_MS`]: its binding calls after `earlier`'s, then `packets` writing what
    /// the frame's net drain hands the frame time, then the frame time. What it sent, and where.
    fn traced(
        &mut self,
        earlier: Vec<BindingInput>,
        now: Vec<BindingInput>,
        packets: impl FnOnce(&mut World),
    ) -> Vec<(When, Sent)> {
        let mut out = Vec::new();
        let world = self.app.world_mut();
        world
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(FRAME_MS));
        *world.resource_mut::<BindingsState>() = BindingsState::after_calls(earlier, now);
        world
            .run_system(self.sample)
            .expect("the mouse's input runs");
        world
            .run_system(self.commands)
            .expect("the press's commands run");
        world.run_system(self.cancel).expect("the cast cancel runs");
        world.run_system(self.press).expect("the press runs");
        self.drain(When::Input, &mut out);
        let world = self.app.world_mut();
        packets(world);
        world.run_system(self.embody).expect("the embodiment runs");
        world.run_system(self.drive).expect("the controller runs");
        self.drain(When::FrameTime, &mut out);
        out
    }

    fn drain(&mut self, when: When, out: &mut Vec<(When, Sent)>) {
        for c in self.sent.try_iter() {
            match c {
                ClientCommand::Move {
                    kind, flags, pos, ..
                } => {
                    self.poses.push((kind, pos));
                    out.push((when, Sent::Move(kind, flags)));
                }
                ClientCommand::MoveModeAck { mode, .. } => out.push((when, Sent::Ack(mode))),
                ClientCommand::StandStateChange { state } => out.push((when, Sent::Stand(state))),
                ClientCommand::CancelCast { .. } => out.push((when, Sent::CancelCast)),
                _ => {}
            }
        }
    }
}

fn forward_held() -> Vec<BindingInput> {
    vec![Start(HeldInput::MoveForward)]
}

fn forward_released() -> Vec<BindingInput> {
    vec![BindingInput::Stop(HeldInput::MoveForward)]
}

/// The root a packet of the frame applies.
fn root(world: &mut World) {
    world.write_message(crate::net::MoveModeMessage {
        guid: ME,
        counter: 1,
        mode: benilla_protocol::MoveMode::Root,
        apply: true,
    });
}

/// `SetCommands` (`0x515090`) runs the tick at its tail (`0x5151a0` → `0x514640`) inside the
/// press, so its root test (`0x514560`) reads the mover before the frame's packets: the start goes
/// out with the position the press found, and the root that lands after it stops the move.
#[test]
fn a_forward_press_in_a_frame_that_roots_us_starts_before_the_root_and_then_stops() {
    use crate::creature_anim::move_flags::{FORWARD, ROOT};
    use crate::net::MoveKind::{StartForward, Stop};
    let mut client = Client::standing(Body::Own);
    let rest = benilla_assets::coords::bevy_to_wow(client.player().pos);
    assert_eq!(
        client.traced(vec![], forward(), root),
        vec![
            (When::Input, Sent::Move(StartForward, FORWARD)),
            (When::FrameTime, Sent::Move(Stop, 0)),
            (When::FrameTime, Sent::Ack(benilla_protocol::MoveMode::Root)),
        ]
    );
    assert_eq!(
        client.poses.first(),
        Some(&(StartForward, rest)),
        "the start carries the pose the press found, before the frame's movement"
    );
    let player = client.player();
    assert_eq!(player.move_flags & FORWARD, 0, "the root stopped the move");
    assert_eq!(player.move_flags & ROOT, ROOT, "and the word carries it");
}

/// A press in a frame with no packets sends what it always has, the start, the heartbeat 500 ms
/// after it (`0x615b80`) and the stop, on the same frames.
#[test]
fn a_press_with_no_packets_sends_the_same_messages_on_the_same_frames() {
    use crate::creature_anim::move_flags::FORWARD;
    use crate::net::MoveKind::{Heartbeat, StartForward, Stop};
    let mut client = Client::standing(Body::Own);
    let mut log = Vec::new();
    log.extend(
        client
            .traced(vec![], forward(), |_| {})
            .into_iter()
            .map(|(_, s)| (0, s)),
    );
    let started = client.app.world().resource::<Time>().elapsed_secs();
    assert_eq!(
        client.player().last_heartbeat,
        started,
        "the start re-arms the deadline"
    );
    for n in 1..40 {
        log.extend(
            client
                .traced(forward_held(), vec![], |_| {})
                .into_iter()
                .map(|(_, s)| (n, s)),
        );
    }
    log.extend(
        client
            .traced(forward_held(), forward_released(), |_| {})
            .into_iter()
            .map(|(_, s)| (40, s)),
    );
    // The first frame at least 500 ms after the start: 32 frames of 16 ms.
    assert_eq!(
        log,
        vec![
            (0, Sent::Move(StartForward, FORWARD)),
            (32, Sent::Move(Heartbeat, FORWARD)),
            (40, Sent::Move(Stop, 0)),
        ]
    );
}

/// A release mid-move is a `SetCommands` too: its tick stops the move inside the release, ahead of
/// the frame's packets and its frame time.
#[test]
fn a_release_mid_move_stops_inside_the_release() {
    use crate::net::MoveKind::Stop;
    let mut client = Client::standing(Body::Own);
    client.traced(vec![], forward(), |_| {});
    for _ in 0..5 {
        client.traced(forward_held(), vec![], |_| {});
    }
    assert_eq!(
        client.traced(forward_held(), forward_released(), |_| {}),
        vec![(When::Input, Sent::Move(Stop, 0))]
    );
}

/// `ToggleAutoRun` presses and releases 0x1000 through `SetCommands` (`0x513e14`), and the two
/// mouse commands press 0x1 and 0x2 through it (`0x51414c`, `0x5141bc`), which the forward axis
/// nets as forward (`0x514da0`): each starts and stops inside its call.
#[test]
fn autorun_and_the_both_button_run_start_and_stop_inside_their_calls() {
    use crate::creature_anim::move_flags::FORWARD;
    use crate::net::MoveKind::{StartForward, Stop};
    let mut client = Client::standing(Body::Own);
    assert_eq!(
        client.traced(vec![], toggle(), |_| {}),
        vec![(When::Input, Sent::Move(StartForward, FORWARD))],
        "autorun on"
    );
    assert_eq!(
        client.traced(vec![], toggle(), |_| {}),
        vec![(When::Input, Sent::Move(Stop, 0))],
        "autorun off"
    );
    let both = || {
        vec![
            Start(HeldInput::TurnOrAction),
            Start(HeldInput::CameraOrSelectOrMove),
        ]
    };
    assert_eq!(
        client.traced(vec![], both(), |_| {}),
        vec![(When::Input, Sent::Move(StartForward, FORWARD))],
        "both mouse commands"
    );
    assert_eq!(
        client.traced(
            both(),
            vec![BindingInput::Stop(HeldInput::CameraOrSelectOrMove)],
            |_| {}
        ),
        vec![(When::Input, Sent::Move(Stop, 0))],
        "one let go"
    );
}

/// Start a `/follow` and hold its forward, as `follow::steer_follow` leaves it each frame.
fn follow(client: &mut Client) {
    client
        .app
        .world_mut()
        .resource_mut::<super::super::follow::FollowState>()
        .start(0xCC, "Leader".into());
    client.player().auto_forward = true;
}

fn following(client: &mut Client) -> bool {
    client
        .app
        .world()
        .resource::<super::super::follow::FollowState>()
        .guid
        .is_some()
}

/// Far sight on, its object gone: the render's update (`0x483054` → `0x5ee290`) finds the field
/// naming no held object, puts the camera back on the player and returns with the bit still set
/// (`0x5ee324`-`0x5ee360`), so the input tick's precondition still refuses (`0x514540`): a forward
/// press starts nothing, though the camera is home.
#[test]
fn a_lost_far_sight_object_keeps_the_input_torn_down() {
    use crate::player::{FarSight, ViewSubject};
    use benilla_protocol::field::FIELD_UNIT_HEALTH;
    /// `PLAYER_FARSIGHT`, the guid's low word.
    const FARSIGHT: u16 = 712;
    const EYE: u64 = 0xF130_0000_0000_0043;
    let mut client = Client::standing(Body::Own);
    let world = client.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::net::ObjectStore(
        benilla_protocol::ObjectFields::from_pairs(&[
            (FIELD_UNIT_HEALTH, 100),
            (FARSIGHT, EYE as u32),
            (FARSIGHT + 1, (EYE >> 32) as u32),
        ]),
    ));
    let eye = world.spawn(Transform::from_xyz(30.0, 0.0, 0.0)).id();
    world.resource_mut::<GuidIndex>().0.insert(EYE, eye);
    let index = world.resource::<GuidIndex>();
    world.resource::<FarSight>().start(
        Some(EYE),
        |g| index.0.contains_key(&g),
        world.resource::<NetCommands>(),
    );
    world.resource_mut::<GuidIndex>().0.remove(&EYE);
    world
        .run_system_once(super::super::view_subject::publish_view_subject)
        .expect("the render's update runs");
    let far_sight = world.resource::<FarSight>();
    assert!(far_sight.on() && far_sight.camera().is_none());
    assert!(
        world.resource::<ViewSubject>().remote.is_none(),
        "the camera is home"
    );
    let sent = client.traced(vec![], forward(), |_| {});
    assert!(
        !sent
            .iter()
            .any(|(_, s)| matches!(s, Sent::Move(crate::net::MoveKind::StartForward, _))),
        "{sent:?}"
    );
}

/// A stun that lands before its root: the stun's tick finds the mover free to translate, and the
/// root's ack runs the tick again (`0x60e33d` → `0x60e347`), which now takes the teardown leg
/// (`0x5146d6`): the follow ends and autorun clears at the ack.
#[test]
fn a_root_ack_after_a_stun_ends_the_follow_at_the_ack() {
    use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH};
    let mut client = Client::new(Body::Own, |_| {});
    let world = client.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::net::ObjectStore(
        benilla_protocol::ObjectFields::from_pairs(&[
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_FLAGS, crate::player::UNIT_FLAG_STUNNED),
        ]),
    ));
    follow(&mut client);
    client.frame(vec![], vec![]);
    assert!(
        following(&mut client),
        "a stun alone leaves the mover free to translate"
    );
    client.player().autorun = true;
    root(client.app.world_mut());
    client.frame(vec![], vec![]);
    assert!(!following(&mut client), "the root's ack ends it");
    assert!(!client.player().autorun, "and releases autorun at once");
}

/// `ToggleAutoRun`'s on press starts the move through `0x60dc60`, whose guard `0x60e990` ends a
/// follow; its off press stops through `0x60dc90`, which has no guard, so the follow goes on.
#[test]
fn autoruns_on_press_ends_a_follow_and_its_off_press_does_not() {
    let mut client = Client::new(Body::Own, |_| {});
    let press_button4 = |client: &mut Client| {
        let world = client.app.world_mut();
        let mut buttons = world.resource_mut::<ButtonInput<MouseButton>>();
        buttons.release_all();
        buttons.clear();
        buttons.press(MouseButton::Forward);
    };
    follow(&mut client);
    press_button4(&mut client);
    client.frame(vec![], toggle());
    assert!(client.player().autorun);
    assert!(!following(&mut client), "the on press ends it");
    follow(&mut client);
    press_button4(&mut client);
    client.frame(vec![], toggle());
    assert!(!client.player().autorun);
    assert!(following(&mut client), "the off press does not");
}

/// A press while following: the start emitters' guard `0x60e990` ends the follow first, so the
/// tick's axis nets without its forward. S reverses at once, with no stop between.
#[test]
fn a_back_press_while_following_reverses_with_no_stop() {
    use crate::creature_anim::move_flags::{BACKWARD, FORWARD};
    use crate::net::MoveKind::{StartBackward, StartForward};
    let mut client = Client::standing(Body::Own);
    follow(&mut client);
    assert_eq!(
        client.traced(vec![], vec![], |_| {}),
        vec![(When::FrameTime, Sent::Move(StartForward, FORWARD))]
    );
    assert_eq!(
        client.traced(vec![], vec![Start(HeldInput::MoveBackward)], |_| {}),
        vec![(When::Input, Sent::Move(StartBackward, BACKWARD))]
    );
    assert!(!following(&mut client));
}

/// A strafe while following stops the follow's forward and then starts the strafe, the forward
/// emitter ahead of the strafe emitter (`0x514da0`, `0x514e80`).
/// Both carry the press's word, as every message of one stream does.
#[test]
fn a_strafe_press_while_following_stops_then_strafes() {
    use crate::creature_anim::move_flags::{FORWARD, STRAFE_LEFT};
    use crate::net::MoveKind::{StartForward, StartStrafeLeft, Stop};
    let mut client = Client::standing(Body::Own);
    follow(&mut client);
    assert_eq!(
        client.traced(vec![], vec![], |_| {}),
        vec![(When::FrameTime, Sent::Move(StartForward, FORWARD))]
    );
    assert_eq!(
        client.traced(vec![], vec![Start(HeldInput::StrafeLeft)], |_| {}),
        vec![
            (When::Input, Sent::Move(Stop, STRAFE_LEFT)),
            (When::Input, Sent::Move(StartStrafeLeft, STRAFE_LEFT)),
        ]
    );
}

/// A `ToggleMouseMove` during a right drag leaves freelook (`0x50f910` → `0x50f940`) and keeps bit
/// 0x1 held, so a turn key strafes (`0x514e98`-`0x514eb1`) and starts no turn (`0x514f6d`): the
/// split reads the command bit, not the look.
#[test]
fn a_turn_key_strafes_under_bit_one_after_a_toggle_left_the_drag() {
    use crate::creature_anim::move_flags::STRAFE_LEFT;
    use crate::net::MoveKind::StartStrafeLeft;
    let mut client = Client::standing(Body::Own);
    let right = || vec![Start(HeldInput::TurnOrAction)];
    client.traced(vec![], right(), |_| {});
    assert!(client.app.world().resource::<CameraControl>().is_looking());
    // The toggle as a call with no host lent hands it to the session.
    client
        .app
        .world_mut()
        .resource_scope(|world, mut rig: Mut<CameraControl>| {
            let mut binds = world.resource_mut::<BindingsState>();
            rig.world_mouse
                .late_call_for_test(benilla_ui::script::LookCall::ToggleMouseMove, &mut binds);
        });
    let sent = client.traced(right(), vec![Start(HeldInput::TurnLeft)], |_| {});
    assert!(
        !client.app.world().resource::<CameraControl>().is_looking(),
        "the toggle left freelook"
    );
    assert_eq!(
        sent,
        vec![(When::Input, Sent::Move(StartStrafeLeft, STRAFE_LEFT))]
    );
}

/// A follow ends at the mouse's hand-off to the body, whose facing write reaches the move guard
/// (`0x5103e0` → `0x5152e0` → `0x60de30` → `0x60e990(1, 1)`), behind bit 0x1 (`0x5145fa`): a
/// right press that has not moved the mouse hands nothing off and keeps it, and the first motion
/// ends it.
#[test]
fn a_right_drag_ends_a_follow_at_its_first_motion() {
    use bevy::input::mouse::AccumulatedMouseMotion;
    let mut client = Client::standing(Body::Own);
    let leader = client.app.world_mut().spawn(Transform::default()).id();
    client
        .app
        .world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .insert(0xCC, leader);
    follow(&mut client);
    let steer = |client: &mut Client| {
        client
            .app
            .world_mut()
            .run_system_once(super::super::follow::steer_follow)
            .expect("the follow steers");
    };
    let right = || vec![Start(HeldInput::TurnOrAction)];
    client.traced(vec![], right(), |_| {});
    steer(&mut client);
    assert!(following(&mut client), "a still press hands nothing off");
    client
        .app
        .world_mut()
        .resource_mut::<AccumulatedMouseMotion>()
        .delta = Vec2::new(10.0, 0.0);
    client.traced(right(), vec![], |_| {});
    steer(&mut client);
    assert!(!following(&mut client), "the motion's hand-off ended it");
}

/// The pitch latch a call left yields to the frame's own writes of it: the mouse hand-off clears
/// the latch (`0x515378`) in a frame whose word did not change, where the press's tick, which takes
/// the calls' latch over only past its changed-word gate, never runs, so the next call reads the
/// cleared latch, not the call's.
#[test]
fn the_mouse_hand_off_supersedes_the_pitch_latch_a_call_left() {
    use super::super::press::CallPitch;
    use bevy::input::mouse::AccumulatedMouseMotion;
    let mut client = Client::standing(Body::Own);
    let right = || vec![Start(HeldInput::TurnOrAction)];
    client.traced(vec![], right(), |_| {});
    let world = client.app.world_mut();
    world.resource_mut::<CallPitch>().set_for_test(true);
    world.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::new(10.0, 0.0);
    client.traced(right(), vec![], |_| {});
    assert_eq!(
        client.app.world().resource::<CallPitch>().called(),
        None,
        "the hand-off's clear is the latch the calls read"
    );
}

/// A move key while seated: the forward emitter stands the body (`0x514e0c` → `0x5ed430`, sent at
/// once) before it queues the start (`0x514e6e`).
#[test]
fn a_move_key_while_seated_stands_before_it_starts() {
    use crate::creature_anim::move_flags::FORWARD;
    use crate::net::MoveKind::StartForward;
    let mut client = Client::standing(Body::Own);
    *client
        .app
        .world_mut()
        .resource_mut::<super::super::StandPending>() = super::super::StandPending(Some(1));
    assert_eq!(
        client.traced(vec![], forward(), |_| {}),
        vec![
            (When::Input, Sent::Stand(0)),
            (When::Input, Sent::Move(StartForward, FORWARD)),
        ]
    );
}

/// The tick runs in the air: its translate predicate tests root alone (`0x514560` tests `0x1200`,
/// not the fall bit `0x2000`), so a key pressed while falling runs its emitter's start, whose
/// `0x60e990` closes the loot at the press (`0x60dc6a`) and ends a follow, before the frame time.
#[test]
fn a_move_key_in_the_air_closes_the_loot_and_ends_a_follow_at_the_press() {
    let mut client = Client::new(Body::Own, |_| {});
    client.frame(vec![], vec![]);
    assert!(
        client.player().move_flags & crate::creature_anim::move_flags::FALLING != 0,
        "falling through the empty world"
    );
    follow(&mut client);
    const CORPSE: u64 = 0xF130_0000_0000_0042;
    let world = client.app.world_mut();
    world.resource_mut::<crate::ui_loot::LootState>().open(
        CORPSE,
        benilla_protocol::messages::loot_type::CORPSE,
        5,
        vec![],
    );
    world.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(CORPSE);
    let _ = client.sent.try_iter().count();
    let world = client.app.world_mut();
    *world.resource_mut::<BindingsState>() = BindingsState::after_calls(vec![], forward());
    for system in [client.sample, client.commands, client.cancel, client.press] {
        world.run_system(system).expect("the press runs");
    }
    assert_eq!(
        world.resource::<crate::ui_loot::LootState>().source(),
        None,
        "the loot closes at the press"
    );
    assert!(
        client
            .sent
            .try_iter()
            .any(|c| matches!(c, ClientCommand::LootRelease { guid } if guid == CORPSE)),
        "released there"
    );
    assert!(!following(&mut client), "and the follow ends there");
}

/// The press cancels a cast that movement interrupts at once (`0x51512a` → `0x6e4940`, sent at
/// `0x6e49ab`), before its tick queues the start: the cancel goes out first, once.
#[test]
fn a_press_mid_cast_sends_the_cancel_before_the_start() {
    use crate::creature_anim::move_flags::FORWARD;
    use crate::net::MoveKind::StartForward;
    const FIREBALL: u32 = 133;
    let mut client = Client::standing(Body::Own);
    let world = client.app.world_mut();
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
            FIREBALL,
            benilla_formats::SpellDisplay {
                interrupt_flags: 0xf,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    let mut pending = world.resource_mut::<crate::spell::PendingCast>();
    pending.fill_record();
    pending.fill_record_spell(FIREBALL);
    pending.push(FIREBALL, None, std::time::Instant::now());
    assert_eq!(
        client.traced(vec![], forward(), |_| {}),
        vec![
            (When::Input, Sent::CancelCast),
            (When::Input, Sent::Move(StartForward, FORWARD)),
        ]
    );
}

/// A pitch key's press while swimming runs the tick's pitch arm inside the press, after the turn
/// arm (`0x514777` → `0x515010`), so its start goes out ahead of a root the frame's packets apply
/// and of the frame time's swim decision, which here leaves the water.
#[test]
fn a_pitch_press_while_swimming_starts_before_the_frames_packets() {
    use crate::creature_anim::move_flags::{PITCH_UP, SWIMMING};
    use crate::net::MoveKind::{StartPitchUp, Stop};
    let mut client = Client::standing(Body::Own);
    {
        let mut player = client.player();
        player.swimming = true;
        player.move_flags = SWIMMING;
    }
    assert_eq!(
        client.traced(vec![], vec![Start(HeldInput::PitchUp)], root),
        vec![
            (When::Input, Sent::Move(StartPitchUp, SWIMMING | PITCH_UP)),
            (When::FrameTime, Sent::Move(Stop, 0)),
            (When::FrameTime, Sent::Ack(benilla_protocol::MoveMode::Root)),
        ]
    );
    assert!(client.player().pitch_latch, "the latch outlives the root");
}

/// A jump taken standing keeps its one air nudge until a direction key opens it: the opener runs
/// only while the direction nibble (`+0x40 & 0xf`) is clear (`0x7c6afc`), and an arc with no key
/// down leaves the nibble clear, so a forward press mid-arc steers the jump.
#[test]
fn a_standing_jump_keeps_its_air_nudge_for_a_key_pressed_mid_arc() {
    let mut client = Client::standing(Body::Own);
    client.traced(vec![], vec![Fire(FiredInput::Jump)], |_| {});
    for _ in 0..5 {
        client.traced(vec![], vec![], |_| {});
    }
    let player = client.player();
    assert!(player.airborne_since.is_some(), "the body is mid-arc");
    assert!(!player.arc_dirs_set, "no key opened the nudge");
    client.traced(vec![], forward(), |_| {});
    let speed = client.player().horiz_vel.length();
    assert!(
        (speed - crate::player::AIR_NUDGE_SPEED).abs() < 1e-4,
        "the forward press nudges the arc: {speed}"
    );
}
