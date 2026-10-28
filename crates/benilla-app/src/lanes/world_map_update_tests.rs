//! `WORLD_MAP_UPDATE` from the explored bytes' watcher. The active player's `PLAYER_EXPLORED_ZONES`
//! carries a mirror watcher (`0x4a6430` arms `0x4a6460` on section 4, `+0xe6c`, 0x100 bytes), whose
//! run in the update packet's notify pass (`0x465330`) calls the map setter (`0x4a6477` →
//! `0x4a67a0`): the overlays follow the bytes it reads, and it signals (`0x4a6ce4`), whole before a
//! later packet's events and after an earlier one's (`0x5384d0`).

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
