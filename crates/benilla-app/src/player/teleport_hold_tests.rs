//! The long same-map teleport's frozen frame, driven through the real path: the wire's teleport
//! into the controller's apply, the stream's residency into the settle release, the hold's
//! decision, and the loading screen beside it.

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::camera::FlyCam;
use super::wire_in::apply_server_moves;
use super::{OwnControlLost, Player, SETTLE_TIMEOUT};
use crate::loading_screen::hold::PresentHold;
use crate::loading_screen::LoadingScreen;
use crate::net::{
    ClientControlMessage, KnockBackMessage, MoveModeMessage, NetCommands, SelfMoveMessage,
    SpeedChangeMessage, TeleportMessage, WorldportMessage,
};
use benilla_assets::coords::wow_to_bevy;
use benilla_world::terrain_stream::WorldLoadProgress;
use benilla_world::world_map::CurrentMap;

const GM_ISLAND: [f32; 3] = [16_222.0, 16_252.0, 12.5];
const SOUTH_SEAS: [f32; 3] = [-11_880.0, -4_700.0, 2.0];

fn tile_of(wow: [f32; 3]) -> (i32, i32) {
    let (tx, ty) = benilla_formats::world_to_tile(wow[0], wow[1]);
    (tx as i32, ty as i32)
}

/// The world's report about the tile at `wow`: in, or still out.
fn progress(wow: [f32; 3], resident: bool) -> WorldLoadProgress {
    WorldLoadProgress {
        ready: if resident { 9 } else { 0 },
        total: 9,
        focus_resident: resident,
        scene_ready: resident,
        focus_tile: Some(tile_of(wow)),
        ..Default::default()
    }
}

/// The controller's server-move apply, as `control` runs it.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn apply(
    time: Res<Time>,
    mut commands: Commands,
    mut player: ResMut<Player>,
    net: Res<NetCommands>,
    mut readers: (
        MessageReader<TeleportMessage>,
        MessageReader<WorldportMessage>,
        MessageReader<SpeedChangeMessage>,
        MessageReader<MoveModeMessage>,
        MessageReader<KnockBackMessage>,
        MessageReader<SelfMoveMessage>,
        MessageReader<ClientControlMessage>,
    ),
    transports: super::TransportQuery,
) {
    let mut cam = FlyCam {
        yaw: 0.0,
        pitch: 0.0,
        speed: 0.0,
    };
    apply_server_moves(
        &time,
        &mut commands,
        &mut player,
        &mut crate::player::MoverLock::default(),
        &mut cam,
        &net,
        &mut readers.0,
        &mut readers.1,
        &mut readers.2,
        &mut readers.3,
        &mut readers.4,
        &mut readers.5,
        &mut readers.6,
        &mut crate::net::MoverClaim::default(),
        Some(1),
        &transports,
        None,
        None,
    );
}

/// The controller and the settle release, ahead of the cover's drive.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
struct Feed;

/// In world on GM Island's resident ground, the clock stepping 100 ms a frame.
fn on_gm_island() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )))
        .insert_state(crate::client_state::ClientState::InWorld)
        .add_message::<TeleportMessage>()
        .add_message::<WorldportMessage>()
        .add_message::<SpeedChangeMessage>()
        .add_message::<MoveModeMessage>()
        .add_message::<KnockBackMessage>()
        .add_message::<SelfMoveMessage>()
        .add_message::<ClientControlMessage>()
        .add_message::<crate::net::EnteredWorldMessage>()
        .add_message::<crate::net::LoggedOutMessage>()
        .add_message::<crate::net::DisconnectedMessage>()
        .add_message::<crate::net::CharacterLoginFailedMessage>()
        .add_message::<crate::transport::TransportLegLeft>()
        .insert_resource(NetCommands(crossbeam_channel::unbounded().0))
        .init_resource::<OwnControlLost>()
        .init_resource::<LoadingScreen>()
        .init_resource::<PresentHold>()
        .init_resource::<crate::pipe_warm::WarmPass>()
        .init_resource::<Assets<Image>>()
        .insert_resource(CurrentMap(1))
        .insert_resource(progress(GM_ISLAND, true))
        .insert_resource(Player {
            active: true,
            pos: wow_to_bevy(GM_ISLAND),
            ..Default::default()
        })
        .add_systems(
            Update,
            (apply, super::world_focus::release_post_snap_hold)
                .chain()
                .in_set(Feed),
        )
        .add_systems(PostUpdate, crate::loading_screen::hold::decide_present_hold);
    crate::loading_screen::add_drive(&mut app, Feed);
    app.update();
    app
}

fn ack_teleport(app: &mut App, to: [f32; 3]) {
    app.world_mut().write_message(TeleportMessage {
        guid: 1,
        counter: 0,
        position: to,
        orientation: 0.0,
    });
}

fn self_teleport(app: &mut App, to: [f32; 3]) {
    app.world_mut().write_message(SelfMoveMessage {
        position: to,
        orientation: 0.0,
        flags: 0,
        pitch: 0.0,
        fall_time: 0,
        jump: None,
        transport: None,
        teleport: true,
    });
}

/// The stream's report for this frame, as the stream stage writes it.
fn stream(app: &mut App, wow: [f32; 3], resident: bool) {
    app.insert_resource(progress(wow, resident));
}

fn held(app: &App) -> bool {
    app.world().resource::<PresentHold>().held()
}

fn covering(app: &App) -> bool {
    app.world().resource::<LoadingScreen>().covering()
}

/// GM Island to the South Seas, both on Kalimdor (`0x630bd0` → `0x694920`): no frame is
/// presented from the snap until the destination is resident, and no screen comes up.
#[test]
fn a_long_teleport_presents_no_frame_until_resident_and_raises_no_screen() {
    let mut app = on_gm_island();
    assert!(
        !held(&app),
        "standing on resident ground, every frame presents"
    );
    ack_teleport(&mut app, SOUTH_SEAS);
    stream(&mut app, SOUTH_SEAS, false);
    for frame in 0..30 {
        app.update();
        assert!(held(&app), "frame {frame} after the snap is presented");
        assert!(!covering(&app), "and the hold is no loading screen");
    }
    stream(&mut app, SOUTH_SEAS, true);
    app.update();
    assert!(
        !held(&app),
        "the frame of residency presents the destination"
    );
    assert!(!covering(&app));
}

/// vmangos sends `MSG_MOVE_TELEPORT` twice, both to the destination (`MovementHandler.cpp:252`,
/// `:255`), and an ack hop can land under a hold: a second, short teleport keeps the hold, which
/// only its release ends.
#[test]
fn a_second_teleport_under_the_hold_keeps_it_until_resident() {
    for second in [
        SOUTH_SEAS,
        [SOUTH_SEAS[0] + 10.0, SOUTH_SEAS[1], SOUTH_SEAS[2]],
    ] {
        for by_ack in [true, false] {
            let mut app = on_gm_island();
            let send = if by_ack { ack_teleport } else { self_teleport };
            send(&mut app, SOUTH_SEAS);
            stream(&mut app, SOUTH_SEAS, false);
            app.update();
            send(&mut app, second);
            for _ in 0..5 {
                app.update();
                assert!(held(&app), "a second copy ({by_ack}) let the frame through");
            }
            stream(&mut app, second, true);
            app.update();
            assert!(!held(&app), "residency releases it");
        }
    }
}

/// A transfer ends the hold, so its loading screen reaches the window.
#[test]
fn a_worldport_under_the_hold_ends_it_and_shows_its_screen() {
    let mut app = on_gm_island();
    ack_teleport(&mut app, SOUTH_SEAS);
    stream(&mut app, SOUTH_SEAS, false);
    app.update();
    assert!(held(&app));
    app.world_mut().write_message(WorldportMessage {
        map_id: 0,
        position: [-8_949.95, -132.493, 83.5312],
        orientation: 0.0,
        needs_ack: true,
        transport_entry: None,
    });
    app.update();
    assert!(covering(&app), "the transfer's screen is up");
    assert!(!held(&app), "and the window shows it");
}

/// The reference's wait-all returns once every read ends, failed or not (`0x443d30`), so the hold
/// is bounded: the stall budget from the snap, which nothing extends.
#[test]
fn the_hold_ends_at_its_stall_budget_from_the_snap() {
    let mut app = on_gm_island();
    ack_teleport(&mut app, SOUTH_SEAS);
    // The destination never comes in, and its tile never matches: the stale world keeps pushing
    // the settle deadline.
    stream(&mut app, GM_ISLAND, false);
    let frames = |secs: f32| (secs * 10.0).round() as usize;
    for _ in 0..frames(SETTLE_TIMEOUT - 0.5) {
        app.update();
    }
    assert!(held(&app), "inside the budget the frame holds");
    for _ in 0..frames(1.0) {
        app.update();
    }
    assert!(!held(&app), "past it the frame is shown");
}
