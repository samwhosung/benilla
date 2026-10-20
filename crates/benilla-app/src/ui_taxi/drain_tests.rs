//! The map through its feed, its drain and its close, over the shipped tables: the race the route
//! search filters by, `TakeTaxiNode`'s refusals and sends (`0x4dc8d0`), and the close that keeps
//! the slots (`0x4dc0f0`).

use std::path::Path;

use benilla_protocol::field::{
    FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS, FIELD_UNIT_MOUNTDISPLAYID,
};
use benilla_protocol::ObjectFields;
use bevy::ecs::system::RunSystemOnce;
use crossbeam_channel::Receiver;

use super::*;

const MASTER: u64 = 0xF130_0000_0000_0099;
/// `UNIT_FIELD_BYTES_0`'s absolute index; its low byte is the race (`[+0x110]+0x78`, `0x4dbd2d`).
const BYTES_0: u16 = 36;

/// The map open on [`MASTER`] at `current` knowing `known`, for a player of `race` on mount
/// display `mount`, with the three taxi refusals' strings set and every shown line recorded.
fn taxi_app(
    data: &Path,
    current: u32,
    known: &[u32],
    race: u32,
    mount: u32,
) -> (App, Receiver<ClientCommand>) {
    let mut chain = benilla_formats::open_chain(data).expect("open chain");
    let catalogs = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
    let (factions, template, reps) = crate::target::stormwind_fixture(&mut chain, 0);
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.init_resource::<TaxiState>()
        .init_resource::<TaxiRouteCache>()
        .init_resource::<GuidIndex>()
        .init_resource::<NameCache>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .insert_resource(NetCommands(tx))
        .insert_resource(catalogs)
        .insert_resource(factions)
        .insert_resource(reps)
        .insert_non_send_resource(UiScript::new().unwrap());
    app.world_mut().spawn((
        SelfPlayer,
        crate::net::Guid(0x10),
        ObjectStore(ObjectFields::from_pairs(&[
            (2, 0x19), // OBJECT_FIELD_TYPE
            (FIELD_UNIT_FACTIONTEMPLATE, 1),
            (FIELD_UNIT_FLAGS, 0x8),
            (BYTES_0, race | 1 << 8),
            (FIELD_UNIT_MOUNTDISPLAYID, mount),
        ])),
    ));
    let master = app
        .world_mut()
        .spawn((
            crate::net::Guid(MASTER),
            ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_UNIT_FACTIONTEMPLATE, template),
                (FIELD_UNIT_FLAGS, 0x1000),
            ])),
        ))
        .id();
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .insert(MASTER, master);
    let mut mask = TaxiMask::default();
    for &id in known {
        mask.0[((id - 1) / 32) as usize] |= 1 << ((id - 1) % 32);
    }
    app.world_mut()
        .resource_mut::<TaxiState>()
        .open(MASTER, current, mask);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            ERR_TAXISAMENODE = "taxi same node"
            ERR_TAXINOSUCHPATH = "taxi no such path"
            ERR_TAXIPLAYERALREADYMOUNTED = "taxi mounted"
            BENILLA_TEST_LINES = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UI_ERROR_MESSAGE")
            f:RegisterEvent("UI_INFO_MESSAGE")
            f:SetScript("OnEvent", function() table.insert(BENILLA_TEST_LINES, arg1) end)
            "#,
        )
        .unwrap();
    (app, rx)
}

fn eval<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval(chunk)
        .unwrap()
}

/// The 1-based slot of the node named `name`.
fn slot_of(app: &mut App, name: &str) -> i64 {
    eval(
        app,
        &format!(
            "for i = 1, NumTaxiNodes() do if TaxiNodeName(i) == {name:?} then return i end end"
        ),
    )
}

/// `TakeTaxiNode(slot)` and the drain: the activates sent and the lines shown since the last.
fn take(
    app: &mut App,
    rx: &Receiver<ClientCommand>,
    slot: i64,
) -> (Vec<ClientCommand>, Vec<String>) {
    eval::<()>(
        app,
        &format!("BENILLA_TEST_LINES = {{}} TakeTaxiNode({slot})"),
    );
    app.world_mut().run_system_once(drain_taxi).unwrap();
    let sent = rx
        .try_iter()
        .filter(|c| {
            matches!(
                c,
                ClientCommand::ActivateTaxi { .. } | ClientCommand::ActivateTaxiExpress { .. }
            )
        })
        .collect();
    (sent, eval(app, "return BENILLA_TEST_LINES"))
}

/// The feed filters the route search by the player's race (`0x4dbd2d`-`0x4dbd56`): a human at
/// Ratchet gets no route to the Horde-only Crossroads, though a path joins the two. The take tests
/// the path, not the route (`0x4dc952`), so it still activates.
#[test]
fn the_feed_filters_the_route_search_by_the_player_s_race() {
    let data = benilla_formats::wow_data_or_skip!();
    let (mut app, rx) = taxi_app(&data, 80, &[25, 80], 1, 0);
    app.world_mut().run_system_once(feed_taxi).unwrap();
    let crossroads = slot_of(&mut app, "Crossroads, The Barrens");
    assert_eq!(
        eval::<String>(&mut app, &format!("return TaxiNodeGetType({crossroads})")),
        "NONE"
    );
    let (sent, shown) = take(&mut app, &rx, crossroads);
    assert!(
        matches!(
            sent.as_slice(),
            [ClientCommand::ActivateTaxi {
                guid: MASTER,
                source_node: 80,
                dest_node: 25
            }]
        ),
        "{sent:?}"
    );
    assert!(shown.is_empty());
}

/// `TakeTaxiNode` refuses a mounted player first, with `0xb2` (`0x4dc909`-`0x4dc918`), then the
/// current node, with `0xaa` (`0x4dc939`-`0x4dc942`); each sends nothing. Unmounted, the direct
/// Sentinel Hill activates.
#[test]
fn take_taxi_node_refuses_a_mounted_player_then_the_current_node() {
    let data = benilla_formats::wow_data_or_skip!();
    let (mut app, rx) = taxi_app(&data, 2, &[2, 4], 1, 2402);
    app.world_mut().run_system_once(feed_taxi).unwrap();
    let (stormwind, sentinel) = (
        slot_of(&mut app, "Stormwind, Elwynn"),
        slot_of(&mut app, "Sentinel Hill, Westfall"),
    );
    for slot in [sentinel, stormwind] {
        let (sent, shown) = take(&mut app, &rx, slot);
        assert!(sent.is_empty(), "mounted, slot {slot}: {sent:?}");
        assert_eq!(shown, ["taxi mounted"], "slot {slot}");
    }

    let player = app
        .world_mut()
        .query_filtered::<Entity, With<SelfPlayer>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .entity_mut(player)
        .insert(ObjectStore(ObjectFields::from_pairs(&[
            (2, 0x19),
            (FIELD_UNIT_FACTIONTEMPLATE, 1),
            (FIELD_UNIT_FLAGS, 0x8),
            (BYTES_0, 1 | 1 << 8),
        ])));
    let (sent, shown) = take(&mut app, &rx, stormwind);
    assert!(sent.is_empty(), "the current node: {sent:?}");
    assert_eq!(shown, ["taxi same node"]);
    let (sent, shown) = take(&mut app, &rx, sentinel);
    assert!(
        matches!(
            sent.as_slice(),
            [ClientCommand::ActivateTaxi {
                guid: MASTER,
                source_node: 2,
                dest_node: 4
            }]
        ),
        "{sent:?}"
    );
    assert!(shown.is_empty());
}

/// A slot with no direct path and no route shows `0xab`, `ERR_TAXINOSUCHPATH` (`0x4dc999`), and
/// sends nothing: Stormwind from Menethil Harbor without Ironforge.
#[test]
fn take_taxi_node_on_a_slot_with_no_route_shows_no_such_path() {
    let data = benilla_formats::wow_data_or_skip!();
    let (mut app, rx) = taxi_app(&data, 7, &[2, 7], 1, 0);
    app.world_mut().run_system_once(feed_taxi).unwrap();
    let stormwind = slot_of(&mut app, "Stormwind, Elwynn");
    let (sent, shown) = take(&mut app, &rx, stormwind);
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(shown, ["taxi no such path"]);
}

/// The close (`0x4dc0f0`) clears the flight master's cell and frees each slot's route
/// (`0x4dc130`-`0x4dc145`), but keeps the slot count (`0xbb4aa8`) and the lines: the closed map
/// still lists its nodes, their types, routes and lines, and its fares read 0 with no flight
/// master (`0x4dc423`). A take then activates a direct path with a zero GUID (`0x4dc9ac`) and finds
/// no route for any other (`0x4dc965`, `0x4dc999`).
#[test]
fn the_closed_map_keeps_its_slots() {
    let data = benilla_formats::wow_data_or_skip!();
    let (mut app, rx) = taxi_app(&data, 2, &[2, 4, 6, 8], 1, 0);
    app.world_mut().run_system_once(feed_taxi).unwrap();
    let (sentinel, thelsamar) = (
        slot_of(&mut app, "Sentinel Hill, Westfall"),
        slot_of(&mut app, "Thelsamar, Loch Modan"),
    );
    let line = format!("return TaxiGetSrcX({thelsamar}, 2), TaxiGetDestY({thelsamar}, 2)");
    let open_line: (f64, f64) = eval(&mut app, &line);
    assert!(eval::<i64>(&mut app, &format!("return TaxiNodeCost({sentinel})")) > 0);

    close_core(app.world_mut());
    app.world_mut().run_system_once(feed_taxi).unwrap();
    assert_eq!(eval::<i64>(&mut app, "return NumTaxiNodes()"), 4);
    assert_eq!(
        eval::<String>(&mut app, &format!("return TaxiNodeName({thelsamar})")),
        "Thelsamar, Loch Modan"
    );
    assert_eq!(
        eval::<String>(&mut app, &format!("return TaxiNodeGetType({thelsamar})")),
        "REACHABLE"
    );
    assert_eq!(
        eval::<i64>(&mut app, &format!("return GetNumRoutes({thelsamar})")),
        2
    );
    assert_eq!(eval::<(f64, f64)>(&mut app, &line), open_line);
    for slot in 1..=4 {
        assert_eq!(
            eval::<i64>(&mut app, &format!("return TaxiNodeCost({slot})")),
            0,
            "slot {slot}"
        );
    }

    let (sent, shown) = take(&mut app, &rx, sentinel);
    assert!(
        matches!(
            sent.as_slice(),
            [ClientCommand::ActivateTaxi {
                guid: 0,
                source_node: 2,
                dest_node: 4
            }]
        ),
        "{sent:?}"
    );
    assert!(shown.is_empty());
    let (sent, shown) = take(&mut app, &rx, thelsamar);
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(shown, ["taxi no such path"]);
}

/// The leave-world leg `0x4db700` frees the slots before the close (`0x4db630`, the count to 0 at
/// `0x4db6bf`): nothing is listed, and a take finds no slot (`0x4dc8df`).
#[test]
fn the_leave_world_leg_frees_the_slots() {
    let data = benilla_formats::wow_data_or_skip!();
    let (mut app, rx) = taxi_app(&data, 2, &[2, 4], 1, 0);
    app.world_mut().run_system_once(feed_taxi).unwrap();
    let sentinel = slot_of(&mut app, "Sentinel Hill, Westfall");
    leave_world(app.world_mut());
    assert_eq!(eval::<i64>(&mut app, "return NumTaxiNodes()"), 0);
    let (sent, shown) = take(&mut app, &rx, sentinel);
    assert!(sent.is_empty(), "{sent:?}");
    assert!(shown.is_empty(), "{shown:?}");
}

/// An open resets the slots before it builds (`0x4db748`): a map from another flight master
/// that builds none (`0x4dba40`, `0x4dbab0`) lists nothing of the last one's, and a take finds no
/// slot.
#[test]
fn an_open_drops_the_last_map_s_slots() {
    let data = benilla_formats::wow_data_or_skip!();
    let (mut app, rx) = taxi_app(&data, 2, &[2, 4], 1, 0);
    app.world_mut().run_system_once(feed_taxi).unwrap();
    let sentinel = slot_of(&mut app, "Sentinel Hill, Westfall");
    open_map(app.world_mut(), MASTER + 1, 2, TaxiMask::default());
    app.world_mut().run_system_once(feed_taxi).unwrap();
    assert_eq!(eval::<i64>(&mut app, "return NumTaxiNodes()"), 0);
    let (sent, shown) = take(&mut app, &rx, sentinel);
    assert!(sent.is_empty(), "{sent:?}");
    assert!(shown.is_empty(), "{shown:?}");
}
