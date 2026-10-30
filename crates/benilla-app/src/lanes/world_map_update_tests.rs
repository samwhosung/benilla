//! `WORLD_MAP_UPDATE` from the explored bytes' watcher and the gossip marker. The active player's
//! `PLAYER_EXPLORED_ZONES` carries a mirror watcher (`0x4a6430` arms `0x4a6460` on section 4,
//! `+0xe6c`, 0x100 bytes), whose run in the update packet's notify pass (`0x465330`) calls the map
//! setter (`0x4a6477` → `0x4a67a0`): the overlays follow the bytes it reads, and it signals
//! (`0x4a6ce4`), whole before a later packet's events and after an earlier one's (`0x5384d0`).
//! The marker's set in `SMSG_GOSSIP_POI`'s handler (`0x6dac93`) and its clears in the minimap's
//! update (`0x6d9411`, `0x6d9a4c`) call the same setter where they run.

use benilla_protocol::messages::ObjectType;
use benilla_protocol::{ObjectFields, SessionEvent};
use benilla_ui::script::{UiScript, WorldMapContinentView, WorldMapOverlayView, WorldMapZoneView};
use bevy::prelude::*;

const ME: u64 = 0x7;
/// `PLAYER_FIELD_BYTES`, whose byte 1 is the combo count (`0x5ddff0`), the neighbour each golden
/// interleaves with.
const PLAYER_FIELD_BYTES: u16 = 188 + 0x1028 / 4;
/// `PLAYER_EXPLORED_ZONES_1`, the first dword of the watched range (`188 + 0xe6c / 4`).
const EXPLORED_1: u16 = 188 + 0xe6c / 4;
/// The dword just below the range, which no watcher covers.
const BELOW_EXPLORED: u16 = EXPLORED_1 - 1;

fn values(pairs: &[(u16, u32)]) -> Vec<SessionEvent> {
    vec![
        SessionEvent::ObjectValues {
            guid: ME,
            fields: ObjectFields::from_pairs(pairs),
        },
        SessionEvent::ObjectUpdateEnd,
    ]
}

fn combo(n: u32) -> Vec<SessionEvent> {
    values(&[(PLAYER_FIELD_BYTES, n << 8)])
}

/// Northshire Valley's `AreaBit` 125 as an update: word 3, bit 29.
fn northshire() -> Vec<SessionEvent> {
    values(&[(EXPLORED_1 + 3, 1 << 29)])
}

/// Bit 126 beside it, which the fixture's zone carries no overlay for.
fn northshire_and_other() -> Vec<SessionEvent> {
    values(&[(EXPLORED_1 + 3, 1 << 29 | 1 << 30)])
}

/// The client in the world as [`ME`], with a map of one zone that shows an overlay once bit 125 is
/// explored, selected, and a VM whose spy records `WORLD_MAP_UPDATE` with the overlays a handler
/// counts through the getter, and the combo event.
fn client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    // In the world with the interface up, so the dispatch fires each packet's lanes.
    app.insert_resource(bevy::prelude::State::new(
        crate::client_state::ClientState::InWorld,
    ));
    let world = app.world_mut();
    let me = world
        .spawn((
            crate::net::Guid(ME),
            crate::net::SelfPlayer,
            crate::net::ObjectStore(ObjectFields::from_pairs(&[]).into_created(ObjectType::Player)),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut script = UiScript::new().unwrap();
    script.set_world_map_catalog(vec![WorldMapContinentView {
        name: "Eastern Kingdoms".into(),
        map_file: "Azeroth".into(),
        zones: vec![WorldMapZoneView {
            name: "Elwynn Forest".into(),
            area_id: 12,
            map_file: "Elwynn".into(),
            overlays: vec![WorldMapOverlayView {
                texture: "Interface\\WorldMap\\Elwynn\\NORTHSHIREVALLEY".into(),
                width: 200,
                height: 176,
                explore_bits: vec![125],
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    }]);
    script.tick(0.01);
    script
        .run(
            r#"
            SetMapZoom(1, 1)
            Heard = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("WORLD_MAP_UPDATE")
            f:RegisterEvent("PLAYER_COMBO_POINTS")
            f:SetScript("OnEvent", function()
                if event == "WORLD_MAP_UPDATE" then
                    table.insert(Heard, event .. ":" .. GetNumMapOverlays())
                else
                    table.insert(Heard, event)
                end
            end)
            "#,
        )
        .unwrap();
    world.insert_non_send_resource(script);
    app
}

/// One frame: the packets through the handler table, then the lanes' drain; what the spy heard.
fn frame(app: &mut App, packets: impl IntoIterator<Item = Vec<SessionEvent>>) -> Vec<String> {
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, packets.into_iter().flatten().collect());
    crate::lanes::fire_in_order(world);
    let s = world.non_send_resource_mut::<UiScript>();
    let out = s.eval::<Vec<String>>("return Heard").unwrap();
    s.run("Heard = {}").unwrap();
    out
}

/// The watcher signals inside the update that moved the bytes: between the neighbouring packets'
/// events, in either order.
#[test]
fn an_explored_update_signals_the_map_between_its_neighbours() {
    let mut app = client();
    assert_eq!(
        frame(&mut app, [combo(2), northshire(), combo(0)]),
        [
            "PLAYER_COMBO_POINTS",
            "WORLD_MAP_UPDATE:1",
            "PLAYER_COMBO_POINTS"
        ]
    );
    assert_eq!(
        frame(&mut app, [northshire_and_other(), combo(2)]),
        ["WORLD_MAP_UPDATE:1", "PLAYER_COMBO_POINTS"],
        "a second update is a second signal, ahead of the packet after it"
    );
}

/// A handler reads the bits its packet wrote through the map's getters: no overlay before, the
/// revealed zone's after, none again when a later update clears the bit.
#[test]
fn a_listener_reads_the_newly_explored_zone_through_the_getters() {
    let mut app = client();
    assert_eq!(
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<i64>("return GetNumMapOverlays()")
            .unwrap(),
        0,
        "control: nothing explored yet"
    );
    assert_eq!(
        frame(&mut app, [northshire()]),
        ["WORLD_MAP_UPDATE:1"],
        "the handler counted the overlay the packet revealed"
    );
    assert_eq!(
        frame(&mut app, [values(&[(EXPLORED_1 + 3, 0)])]),
        ["WORLD_MAP_UPDATE:0"]
    );
}

/// An update that moves no byte of the range fires nothing: one beside it, and one that writes the
/// bytes the player already has.
#[test]
fn an_update_that_moves_no_explored_byte_fires_nothing() {
    let mut app = client();
    assert_eq!(
        frame(&mut app, [northshire()]),
        ["WORLD_MAP_UPDATE:1"],
        "control: the watcher is armed"
    );
    assert_eq!(
        frame(
            &mut app,
            [
                values(&[(BELOW_EXPLORED, 5)]),
                northshire(),
                values(&[(EXPLORED_1 + 3, 1 << 29), (BELOW_EXPLORED, 6)]),
            ]
        ),
        Vec::<String>::new()
    );
}

/// The guard's directions at `pos`, as `SMSG_GOSSIP_POI` carries them.
fn directions(pos: [f32; 2]) -> Vec<SessionEvent> {
    vec![SessionEvent::GossipPoi(
        benilla_protocol::messages::GossipPoi {
            flags: 99,
            pos,
            icon: 6,
            data: 0,
            name: "Stormwind Warrior Trainer".into(),
        },
    )]
}

/// Where the marker drops, far from where we stand.
const MARKER: [f32; 2] = [50.0, 50.0];

/// [`client`] with the map data the landmark sources read, a projection that puts a world point
/// at its own UV, and a second spy that counts the landmarks on `WORLD_MAP_UPDATE`.
fn landmark_client() -> App {
    let mut app = client();
    let world = app.world_mut();
    world.insert_resource(benilla_world::world_map::CurrentMap(0));
    world.init_resource::<crate::player::Player>();
    let start = std::time::Instant::now();
    let mut clock = Time::<bevy::time::Real>::new(start);
    clock.update_with_instant(start);
    world.insert_resource(clock);
    world.insert_resource(ClockStart(start));
    world.insert_resource(crate::area::AreaTableRes(
        benilla_formats::AreaTableCatalog::from_rows(Vec::new()),
    ));
    let mut script = world.non_send_resource_mut::<UiScript>();
    script.set_world_loc_projector(Box::new(|_, _, x, y| Some((x, y))));
    script.tick(0.01);
    script
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("WORLD_MAP_UPDATE")
            f:SetScript("OnEvent", function()
                table.insert(Heard, "landmarks:" .. GetNumMapLandmarks())
            end)
            Heard = {}
            "#,
        )
        .unwrap();
    app
}

/// The hand-driven clock's start.
#[derive(Resource)]
struct ClockStart(std::time::Instant);

/// The real clock `secs` past its start.
fn clock_at(app: &mut App, secs: u64) {
    let start = app.world().resource::<ClockStart>().0;
    app.world_mut()
        .resource_mut::<Time<bevy::time::Real>>()
        .update_with_instant(start + std::time::Duration::from_secs(secs));
}

fn marker_held(app: &App) -> bool {
    app.world()
        .resource::<crate::poi_marker::PoiMarker>()
        .poi
        .is_some()
}

/// [`frame`] with the minimap's update between the packets and the drain, as the frame runs it:
/// the marker's clears (`crate::poi_marker::expire_marker`).
fn frame_with_minimap(
    app: &mut App,
    packets: impl IntoIterator<Item = Vec<SessionEvent>>,
) -> Vec<String> {
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, packets.into_iter().flatten().collect());
    world
        .run_system_cached(crate::poi_marker::expire_marker)
        .unwrap();
    crate::lanes::fire_in_order(world);
    let s = world.non_send_resource_mut::<UiScript>();
    let out = s.eval::<Vec<String>>("return Heard").unwrap();
    s.run("Heard = {}").unwrap();
    out
}

/// The marker's set re-selects the shown map inside its handler (`0x4e28aa` → `0x6dac10` →
/// `0x6dac93`): `WORLD_MAP_UPDATE` falls between the neighbouring packets' events, its listeners
/// reading the marker in the landmark list, and a second set signals again.
#[test]
fn the_gossip_marker_signals_the_map_between_its_neighbours() {
    let mut app = landmark_client();
    assert_eq!(
        frame_with_minimap(&mut app, [combo(2), directions(MARKER), combo(0)]),
        [
            "PLAYER_COMBO_POINTS",
            "WORLD_MAP_UPDATE:0",
            "landmarks:1",
            "PLAYER_COMBO_POINTS"
        ]
    );
    assert_eq!(
        frame_with_minimap(&mut app, [directions(MARKER), combo(2)]),
        ["WORLD_MAP_UPDATE:0", "landmarks:1", "PLAYER_COMBO_POINTS"],
        "the same directions again are a second set, which signals whether or not the list moved"
    );
}

/// Arriving within 10 yards clears the marker in the minimap's update (`0x6d9a08`-`0x6d9a4c`),
/// whose re-select signals in that frame, after its packets' events, its listeners reading the
/// list without the marker; the next frame's packets come after it.
#[test]
fn arriving_at_the_marker_signals_the_map_in_its_frame() {
    let mut app = landmark_client();
    frame_with_minimap(&mut app, [directions(MARKER)]);
    app.world_mut().resource_mut::<crate::player::Player>().pos =
        benilla_assets::coords::wow_to_bevy([MARKER[0] + 3.0, MARKER[1], 0.0]);
    assert_eq!(
        frame_with_minimap(&mut app, [combo(2)]),
        ["PLAYER_COMBO_POINTS", "WORLD_MAP_UPDATE:0", "landmarks:0"]
    );
    assert_eq!(
        frame_with_minimap(&mut app, [combo(0)]),
        ["PLAYER_COMBO_POINTS"],
        "the clear signalled once"
    );
}

/// The deadline, 480 seconds past the set (`0x6dac7e`), clears the marker in the minimap's update
/// and re-selects the map (`0x6d93d0`-`0x6d9411`): one signal in that frame, after its packets'
/// events, the player never having moved.
#[test]
fn the_deadline_signals_the_map_in_its_frame() {
    let mut app = landmark_client();
    frame_with_minimap(&mut app, [directions(MARKER)]);
    clock_at(&mut app, 479);
    assert_eq!(
        frame_with_minimap(&mut app, [combo(2)]),
        ["PLAYER_COMBO_POINTS"],
        "control: a second short of the deadline"
    );
    clock_at(&mut app, 480);
    assert_eq!(
        frame_with_minimap(&mut app, [combo(0)]),
        ["PLAYER_COMBO_POINTS", "WORLD_MAP_UPDATE:0", "landmarks:0"]
    );
    assert!(!marker_held(&app));
    assert_eq!(
        frame_with_minimap(&mut app, [combo(2)]),
        ["PLAYER_COMBO_POINTS"],
        "the deadline fired once"
    );
}

/// A `/reload` runs the world-enter cascade (`0x490168` → `0x4908c0`), whose minimap leg
/// (`0x49096f` → `0x4ed9d0` → `0x6d8e10`) zeroes the marker's record (`0x6d8fb0`) with no
/// signal and leaves its deadline: at the deadline the update still clears and re-selects the
/// map, one late `WORLD_MAP_UPDATE`.
#[test]
fn a_reload_drops_the_marker_and_its_deadline_still_signals() {
    let mut app = landmark_client();
    frame_with_minimap(&mut app, [directions(MARKER)]);
    assert!(marker_held(&app), "control: the marker is set");
    app.world_mut()
        .resource_mut::<crate::ui_world_enter::ArrivalLane>()
        .push(crate::ui_world_enter::Entry::Reload);
    let heard = frame_with_minimap(&mut app, []);
    assert!(
        !heard.iter().any(|e| e.starts_with("WORLD_MAP_UPDATE")),
        "the cascade's clear signals nothing: {heard:?}"
    );
    assert!(!marker_held(&app), "the cascade zeroed the record");
    assert_eq!(
        frame_with_minimap(&mut app, [combo(2)]),
        ["PLAYER_COMBO_POINTS"]
    );
    clock_at(&mut app, 480);
    assert_eq!(
        frame_with_minimap(&mut app, [combo(0)]),
        ["PLAYER_COMBO_POINTS", "WORLD_MAP_UPDATE:0", "landmarks:0"],
        "the kept deadline re-selects the map"
    );
}

/// Arrival is tested only in an update whose player position or map moved since the cached one
/// (`0x6d9419`-`0x6d9459`, `0x6d95c4` → `0x6d9a58`): a marker dropped within 10 yards of a player
/// standing still stays, and the first step clears it.
#[test]
fn arrival_waits_for_an_update_that_moved() {
    let mut app = landmark_client();
    let here = [MARKER[0] + 3.0, MARKER[1]];
    app.world_mut().resource_mut::<crate::player::Player>().pos =
        benilla_assets::coords::wow_to_bevy([here[0], here[1], 0.0]);
    frame_with_minimap(&mut app, [combo(2)]);
    assert_eq!(
        frame_with_minimap(&mut app, [directions(MARKER)]),
        ["WORLD_MAP_UPDATE:0", "landmarks:1"],
        "set beside a player standing still: no arrival"
    );
    assert_eq!(
        frame_with_minimap(&mut app, [combo(0)]),
        ["PLAYER_COMBO_POINTS"]
    );
    assert!(marker_held(&app));
    app.world_mut().resource_mut::<crate::player::Player>().pos =
        benilla_assets::coords::wow_to_bevy([here[0] - 1.0, here[1], 0.0]);
    assert_eq!(
        frame_with_minimap(&mut app, [combo(2)]),
        ["PLAYER_COMBO_POINTS", "WORLD_MAP_UPDATE:0", "landmarks:0"],
        "the step is an arrival"
    );
}
