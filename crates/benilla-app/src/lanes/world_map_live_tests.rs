//! The world map's getters read the state their listener's packet left, at the call, where the
//! reference reads it: the setter `0x4a67a0` reads the active player's explored bytes and the
//! landmark sources as it builds (`0x4a6bf4`, `0x4a6819`), and `GetCorpseMapPosition` reads the
//! corpse cache (`0x4a86df` → `0x4a7960` → `0x492090`). Each packet's handler runs whole before the
//! next (`0x537ae5`), so a listener in one packet of a frame reads that packet's state, not the
//! frame's last.

use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::{UiScript, WorldMapContinentView, WorldMapOverlayView, WorldMapZoneView};
use benilla_world::map_proj::ZoneRect;
use bevy::prelude::*;
use crossbeam_channel::Sender;

use super::unit_view_tests::{created, values, wire_packets};

const ME: u64 = 0x7;
/// `PLAYER_FIELD_BYTES`, whose byte 1 is the combo count (`0x5ddff0`): the packet whose
/// `PLAYER_COMBO_POINTS` listener reads the map.
const PLAYER_FIELD_BYTES: u16 = 188 + 0x1028 / 4;
/// `PLAYER_FLAGS`, and its ghost bit (`[player+0xe68]+8`, bit 4).
const PLAYER_FLAGS: u16 = 188 + 0x8 / 4;
const GHOST: u32 = 0x10;
/// `PLAYER_EXPLORED_ZONES_1`, the first dword of the explored bytes (`188 + 0xe6c / 4`).
const EXPLORED_1: u16 = 188 + 0xe6c / 4;
/// Elwynn Forest, the one zone, and Northshire Valley inside it, explored by bit 125 (word 3, bit
/// 29), which reveals the zone's overlay and gates its landmark.
const ELWYNN: u32 = 12;
const NORTHSHIRE: u32 = 9;
const NORTHSHIRE_BIT: u32 = 125;
/// The zone's rect, 1000 yards a side from the origin.
const RECT: ZoneRect = ZoneRect {
    left: 1000.0,
    right: 0.0,
    top: 1000.0,
    bottom: 0.0,
};

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// What the combo listener heard, emptied.
    fn seen(&mut self) -> Vec<String> {
        crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
            let s = s.expect("a VM");
            let seen = s.eval::<Vec<String>>("return Seen").expect("the spy");
            s.run("Seen = {}").expect("the reset");
            seen
        })
    }
}

/// The client with a map of one continent and one zone, Elwynn, whose overlay and whose one
/// `AreaPOI` row, a zone-level landmark in Northshire, wait on bit 125; and a VM whose
/// `PLAYER_COMBO_POINTS` listener selects Elwynn and notes the overlays, the landmarks and the
/// corpse's position it reads.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, _sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let mut script = UiScript::new().expect("a VM");
    script.set_world_map_catalog(vec![WorldMapContinentView {
        name: "Eastern Kingdoms".into(),
        map_file: "Azeroth".into(),
        zones: vec![WorldMapZoneView {
            name: "Elwynn Forest".into(),
            area_id: ELWYNN,
            map_file: "Elwynn".into(),
            loc_rect: (RECT.left, RECT.right, RECT.top, RECT.bottom),
            overlays: vec![WorldMapOverlayView {
                texture: "Interface\\WorldMap\\Elwynn\\NORTHSHIREVALLEY".into(),
                width: 200,
                height: 176,
                explore_bits: vec![NORTHSHIRE_BIT],
                ..Default::default()
            }],
        }],
        ..Default::default()
    }]);
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_COMBO_POINTS")
            f:SetScript("OnEvent", function()
                SetMapZoom(1, 1)
                local x, y = GetCorpseMapPosition()
                table.insert(Seen, string.format("%d/%d %.3f,%.3f",
                    GetNumMapOverlays(), GetNumMapLandmarks(), x, y))
            end)
            "#,
        )
        .expect("the spy");
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    app.insert_resource(crate::ui_world_map::WorldMapUiData::one_zone_for_test(
        0, ELWYNN, RECT,
    ));
    app.insert_resource(benilla_world::world_map::CurrentMap(0));
    let area = |explore_flag, exploration_level| benilla_formats::AreaTableRow {
        map_id: 0,
        zone_id: 0,
        explore_flag,
        flags: 0,
        faction_group_mask: 0,
        exploration_level,
        name: String::new(),
    };
    app.insert_resource(crate::area::AreaTableRes(
        benilla_formats::AreaTableCatalog::from_rows(vec![
            (ELWYNN, area(0, -1)),
            (NORTHSHIRE, area(NORTHSHIRE_BIT, 0)),
        ]),
    ));
    app.insert_resource(crate::area_poi::AreaPoiRes(
        benilla_formats::AreaPoiCatalog::from_rows(vec![(
            1,
            benilla_formats::AreaPoi {
                importance: 0,
                icon: 6,
                faction_id: 0,
                pos: [500.0, 500.0, 0.0],
                continent_id: 0,
                // Zone level.
                flags: 0x04,
                area_id: NORTHSHIRE,
                name: "Northshire Abbey".into(),
                description: String::new(),
                world_state_id: 0,
            },
        )]),
    ));
    let mut c = Client { app, inbound };
    // A frame with no player: the map's projection goes in, and whatever reads the bytes reads
    // none.
    c.frame(vec![]);
    c
}

/// A packet that sets our combo count to `n`, whose listener reads the map.
fn combo(n: u32) -> Vec<SessionEvent> {
    vec![values(ME, &[(PLAYER_FIELD_BYTES, n << 8)])]
}

/// `MSG_CORPSE_QUERY`'s found answer, our corpse at `(x, y)` on map 0.
fn corpse_at(x: f32, y: f32) -> Vec<SessionEvent> {
    vec![SessionEvent::CorpseQuery {
        found: true,
        display_map: 0,
        position: [x, y, 0.0],
        corpse_map: 0,
    }]
}

/// The not-found answer, which resets the cache.
fn no_corpse() -> Vec<SessionEvent> {
    vec![SessionEvent::CorpseQuery {
        found: false,
        display_map: -1,
        position: [0.0; 3],
        corpse_map: 0,
    }]
}

/// Our first create carries Northshire explored, which no watcher signals (a first create runs no
/// notify pass); a later packet's listener selects the map, whose setter reads the bytes at its
/// run (`0x4a6bf4`): the overlay and the landmark they gate show, though a packet after it clears
/// the bit.
#[test]
fn a_selection_reads_the_explored_bytes_its_packet_left() {
    let mut c = client();
    c.frame(vec![
        vec![created(
            ME,
            EntityKind::Player,
            &[(EXPLORED_1 + 3, 1 << (NORTHSHIRE_BIT % 32))],
        )],
        combo(2),
        vec![values(ME, &[(EXPLORED_1 + 3, 0)])],
    ]);
    assert_eq!(c.seen(), ["1/1 0.000,0.000"]);
}

/// The world-state init's setter run (`0x48fa0d` → `0x4a6cdf`) reads the explored bytes as it
/// builds (`0x4a6bf4`): after our create, whose bits no watcher signals, the init's
/// `WORLD_MAP_UPDATE` listener on the map already shown reads the overlay and the landmark the
/// bits reveal.
#[test]
fn a_world_state_rebuild_reads_the_bytes_its_packet_left() {
    let mut c = client();
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM")
            .run(
                r#"
                SetMapZoom(1, 1)
                local f = CreateFrame("Frame")
                f:RegisterEvent("WORLD_MAP_UPDATE")
                f:SetScript("OnEvent", function()
                    table.insert(Seen, "update " .. GetNumMapOverlays() .. "/" .. GetNumMapLandmarks())
                end)
                "#,
            )
            .expect("the listener")
    });
    c.seen();
    c.frame(vec![
        vec![created(
            ME,
            EntityKind::Player,
            &[(EXPLORED_1 + 3, 1 << (NORTHSHIRE_BIT % 32))],
        )],
        vec![SessionEvent::WorldStates {
            scope: Some((0, ELWYNN)),
            states: Vec::new(),
        }],
    ]);
    assert_eq!(
        c.seen().first().map(String::as_str),
        Some("update 1/1"),
        "the init's run reads the bytes"
    );
}

/// Where `(x, y)` lands on Elwynn, the zone the listener selects.
fn on_elwynn(x: f32, y: f32) -> String {
    let (u, v) = crate::ui_world_map::project_on_displayed(
        &crate::ui_world_map::WorldMapUiData::one_zone_for_test(0, ELWYNN, RECT),
        (1, 1, None),
        0,
        x,
        y,
    )
    .expect("on the zone");
    format!("0/0 {u:.3},{v:.3}")
}

/// The corpse's position is read at the call: a listener between two answers of a frame reads
/// the first's, projected onto the zone it selected, where the frame before held another. We are
/// a ghost, the one state the answer is stored for (`0x48f766`-`0x48f779`).
#[test]
fn the_corpse_position_reads_its_packets_answer() {
    let mut c = client();
    c.frame(vec![
        vec![created(ME, EntityKind::Player, &[(PLAYER_FLAGS, GHOST)])],
        corpse_at(250.0, 750.0),
    ]);
    c.frame(vec![corpse_at(600.0, 300.0), combo(2), no_corpse()]);
    assert_eq!(c.seen(), [on_elwynn(600.0, 300.0)]);
    c.frame(vec![combo(0)]);
    assert_eq!(
        c.seen(),
        ["0/0 0.000,0.000"],
        "the reset cache reads nothing"
    );
}

/// A found answer that reaches a living player resets the cache, as any answer but a ghost's
/// does (`0x48f766`-`0x48f779` → `0x48f7e3`): the corpse the cache held goes, and a listener
/// after the answer reads 0, 0.
#[test]
fn a_found_answer_for_the_living_clears_the_corpse() {
    let mut c = client();
    c.frame(vec![vec![created(ME, EntityKind::Player, &[])]]);
    c.app
        .world_mut()
        .resource_mut::<crate::death::DeathNet>()
        .corpse = Some(crate::death::CorpsePoint {
        display_map: 0,
        position: [250.0, 750.0, 0.0],
        corpse_map: 0,
    });
    c.frame(vec![combo(2)]);
    assert_eq!(
        c.seen(),
        [on_elwynn(250.0, 750.0)],
        "control: the cache shows"
    );
    c.frame(vec![corpse_at(600.0, 300.0), combo(0)]);
    assert_eq!(c.seen(), ["0/0 0.000,0.000"]);
}
