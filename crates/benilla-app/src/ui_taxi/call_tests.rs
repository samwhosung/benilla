//! The taxi verbs through the whole client's host: `CloseTaxiMap` (`0x4dcce0` → `0x4dc0f0`) and
//! `TakeTaxiNode` (`0x4dc8d0`) act inside their calls, so a chunk's next line reads the closed map
//! and its packets leave in call order with every other verb's.

use benilla_protocol::field::{FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS};
use benilla_protocol::ObjectFields;
use bevy::ecs::system::RunSystemOnce;
use crossbeam_channel::Receiver;

use super::*;
use crate::net::ClientCommand;
use crate::script_calls::with_hosted_vm;

const ME: u64 = 0x10;
const MASTER: u64 = 0xF130_0000_0000_0099;

/// The whole client, headless, with the map open on [`MASTER`] at Stormwind (2) knowing Sentinel
/// Hill (4), Ironforge (6) and Thelsamar (8), the master latched, and every map event and shown
/// line recorded in `HEARD`.
fn client(data: &std::path::Path) -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let mut chain = benilla_formats::open_chain(data).expect("open chain");
    app.insert_resource(TaxiCatalogs::load(&mut chain).expect("taxi DBCs"));
    let world = app.world_mut();
    let me = world
        .spawn((
            SelfPlayer,
            crate::net::Guid(ME),
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x19),
                (FIELD_UNIT_FACTIONTEMPLATE, 1),
                (FIELD_UNIT_FLAGS, 0x8),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
    world.insert_resource(crate::net::SelfGuid(Some(ME)));
    let master = world
        .spawn((
            crate::net::Guid(MASTER),
            ObjectStore(ObjectFields::from_pairs(&[(FIELD_UNIT_FLAGS, 0x1000)])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(MASTER, master);
    world
        .resource_mut::<TaxiState>()
        .open(MASTER, 2, TaxiMask([0b1010_1010, 0, 0, 0, 0, 0, 0, 0]));
    world
        .resource_mut::<crate::ui_session::InteractLatch>()
        .latch_for_test(MASTER);
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            ERR_TAXINOSUCHPATH = "taxi no such path"
            HEARD = ""
            local f = CreateFrame("Frame")
            f:RegisterEvent("TAXIMAP_CLOSED")
            f:RegisterEvent("UI_INFO_MESSAGE")
            f:SetScript("OnEvent", function()
                HEARD = HEARD .. event .. "(" .. tostring(arg1) .. ") "
            end)
            "#,
        )
        .expect("the recorder installs");
    world.insert_non_send_resource(script);
    world.run_system_once(feed_taxi).expect("the feed runs");
    (app, rx)
}

fn eval<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, lua: &str) -> T {
    with_hosted_vm(app.world_mut(), |vm| vm.expect("a VM").eval::<T>(lua)).expect("the chunk runs")
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

/// The packets a chunk sent, the name queries left out.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| !c.starts_with("NameQuery") && !c.starts_with("CreatureQuery"))
        .collect()
}

/// `CloseTaxiMap()` closes inside the call (`0x4dc0f0`): the latch's release re-enters the core,
/// so `TAXIMAP_CLOSED` fires twice before the next line (`0x4dc105`, `0x4dc15f`), and every fare
/// reads 0 there (`0x4dc423`). The chunk's `TakeTaxiNode` then reads the zeroed cell
/// (`0x4dc113`): a direct path activates with guid 0 (`0x4dc9ac`), and a slot only a route
/// reaches finds its route freed (`0x4dc145`) and shows `0xab`, a yellow line, inside the call
/// (`0x4dc999`), ahead of the roll the chunk sends after it.
#[test]
fn close_taxi_map_closes_inside_the_call() {
    let data = benilla_formats::wow_data_or_skip!();
    let (mut app, rx) = client(&data);
    let sentinel = slot_of(&mut app, "Sentinel Hill, Westfall");
    let thelsamar = slot_of(&mut app, "Thelsamar, Loch Modan");
    let _ = sent(&rx);
    let (closed, cost, heard): (String, i64, String) = eval(
        &mut app,
        &format!(
            "HEARD = '' CloseTaxiMap() local closed, cost = HEARD, TaxiNodeCost({sentinel}) \
             HEARD = '' TakeTaxiNode({sentinel}) TakeTaxiNode({thelsamar}) RandomRoll(1, 2) \
             return closed, cost, HEARD"
        ),
    );
    assert_eq!(closed, "TAXIMAP_CLOSED(nil) TAXIMAP_CLOSED(nil) ");
    assert_eq!(cost, 0);
    assert_eq!(heard, "UI_INFO_MESSAGE(taxi no such path) ");
    assert_eq!(
        sent(&rx),
        [
            "ActivateTaxi { guid: 0, source_node: 2, dest_node: 4 }",
            "RandomRoll { min: 1, max: 2 }",
        ]
    );
    assert!(app.world().resource::<TaxiState>().open.is_none());
    // The latch released inside the call (`0x4dc105`).
    assert_eq!(
        app.world()
            .resource::<crate::ui_session::InteractLatch>()
            .guid(),
        None
    );
}

/// `TakeTaxiNode` on the open map sends inside the call, with the master's guid: Sentinel Hill's
/// direct path (`0x4dc9be`), then Thelsamar's route as an express with its raw fare sum
/// (`0x4dc98e`), in call order with the roll between them.
#[test]
fn take_taxi_node_sends_inside_the_call() {
    let data = benilla_formats::wow_data_or_skip!();
    let (mut app, rx) = client(&data);
    let sentinel = slot_of(&mut app, "Sentinel Hill, Westfall");
    let thelsamar = slot_of(&mut app, "Thelsamar, Loch Modan");
    let _ = sent(&rx);
    eval::<()>(
        &mut app,
        &format!("TakeTaxiNode({sentinel}) RandomRoll(1, 2) TakeTaxiNode({thelsamar})"),
    );
    let sent = sent(&rx);
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(sent[0].starts_with("ActivateTaxi { guid: 17379390962022744217, source_node: 2"));
    assert_eq!(sent[1], "RandomRoll { min: 1, max: 2 }");
    assert!(
        sent[2].starts_with("ActivateTaxiExpress { guid: 17379390962022744217, total_cost: 160"),
        "{sent:?}"
    );
}

/// The open's Travel tutorial follows its `TAXIMAP_OPENED` (`0x4dba96`, `0x4dbaa2`), raised where
/// the event is, so a handler that closes the map inside its call cannot skip it; the close and
/// the feeds after it raise no other.
#[test]
fn the_open_raises_the_travel_tutorial_with_its_event() {
    use crate::tutorial::{id, TutorialEvent};
    let data = benilla_formats::wow_data_or_skip!();
    let (mut app, _rx) = client(&data);
    let travel = |app: &App, cursor: &mut bevy::ecs::message::MessageCursor<TutorialEvent>| {
        cursor
            .read(app.world().resource::<Messages<TutorialEvent>>())
            .filter(|e| **e == TutorialEvent::trigger(id::TRAVEL))
            .count()
    };
    let mut cursor = app
        .world()
        .resource::<Messages<TutorialEvent>>()
        .get_cursor();
    assert_eq!(travel(&app, &mut cursor), 1, "the open raised it");
    eval::<()>(&mut app, "CloseTaxiMap()");
    app.world_mut()
        .run_system_once(feed_taxi)
        .expect("the feed runs");
    assert_eq!(travel(&app, &mut cursor), 0);
}
