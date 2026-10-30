//! The stable getters as a listener reads them, and the stable's events at their packets. The
//! getters read the module's records and counts at the call, a closed window's included
//! (`0x4cb280`, `0x4cae29`); a record whose creature the cache lacks is asked with the window's
//! callback, which signals `PET_STABLE_UPDATE` when it lands (`0x4cb2ce`, `0x4cb3bf`); the list
//! signals `PET_STABLE_SHOW` and the money refusal its line inside their handlers (`0x4cac9b`,
//! `0x4cacde`).

use benilla_protocol::messages::{stable_result, StabledPet};
use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::ClientCommand;
use crate::ui_stable::{StableLane, StableOpen};

const ME: u64 = 0x10;
const MASTER: u64 = 0xF130_0000_4500_0001;
const BOAR: u32 = 1_126;

/// The client with us known, and a VM whose own copy holds no stable: its spy notes each stable
/// event and message line.
fn client() -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    // In the world with the interface up, so the dispatch fires each packet's lanes.
    app.insert_resource(bevy::prelude::State::new(
        crate::client_state::ClientState::InWorld,
    ));
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.insert_resource(crate::net::SelfGuid(Some(ME)));
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            ERR_NOT_ENOUGH_MONEY = "poor"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"PET_STABLE_SHOW", "PET_STABLE_UPDATE",
                    "PET_STABLE_UPDATE_PAPERDOLL", "PET_STABLE_CLOSED", "UI_ERROR_MESSAGE"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(arg1))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    (app, rx)
}

/// `chunk`'s answer through the lent host when `live`, else off the VM's copy alone.
fn eval<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, live: bool, chunk: &str) -> T {
    if live {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM").eval::<T>(chunk).expect("runs")
        })
    } else {
        crate::script_calls::eval_in_vm(app.world(), chunk)
    }
}

fn boar() -> StabledPet {
    StabledPet {
        pet_number: 3,
        creature_entry: BOAR,
        level: 10,
        name: "Boar".into(),
        loyalty: 1,
        slot: 1,
    }
}

/// `packets` through their handlers: each event the stable lane takes fires inside the dispatch, at
/// its packet's place, as a handler signals inside itself (`0x703e50`) ahead of the next packet's
/// (`0x537ae5`), so the drain finds nothing; what the spy heard.
fn handled(app: &mut App, packets: Vec<SessionEvent>) -> Vec<String> {
    crate::net::handlers::dispatch(app.world_mut(), packets);
    app.world_mut().flush();
    assert!(
        app.world().resource::<StableLane>().stamped().is_empty(),
        "fired inside the dispatch"
    );
    eval(app, false, "local s = Seen Seen = {} return s")
}

/// With the host lent the getters read the records as the world holds them, where the VM's copy
/// holds them as the list's signal left them, and a close leaves the records and the count,
/// clearing only the cell (`0x4cae29`).
#[test]
fn the_getters_read_the_stable_through_the_host() {
    let (mut app, _rx) = client();
    let list = SessionEvent::ListStabledPets {
        npc: MASTER,
        num_stable_slots: 2,
        pets: vec![boar()],
    };
    handled(&mut app, vec![list]);
    app.world_mut().resource_mut::<StableOpen>().pets[0].name = "Tusker".into();
    let read = "local _, name = GetStablePetInfo(1) \
                return GetNumStableSlots(), GetNumStablePets(), name";
    assert_eq!(
        eval::<(i64, i64, Option<String>)>(&mut app, false, read),
        (2, 1, Some("Boar".into())),
        "the copy"
    );
    assert_eq!(
        eval::<(i64, i64, Option<String>)>(&mut app, true, read),
        (2, 1, Some("Tusker".into())),
        "live"
    );
    crate::ui_stable::close_core(app.world_mut());
    assert_eq!(
        eval::<(i64, i64, Option<String>)>(&mut app, true, read),
        (2, 1, Some("Tusker".into())),
        "the close keeps the records"
    );
}

/// The list's `PET_STABLE_SHOW`, the money refusal's line and code 11's `PET_STABLE_UPDATE` fire
/// inside their packet's dispatch.
#[test]
fn the_list_and_the_results_fire_inside_their_dispatch() {
    let (mut app, _rx) = client();
    let list = SessionEvent::ListStabledPets {
        npc: MASTER,
        num_stable_slots: 1,
        pets: vec![boar()],
    };
    assert_eq!(handled(&mut app, vec![list]), ["PET_STABLE_SHOW:nil"]);
    let refused = SessionEvent::StableResult {
        result: stable_result::ERR_MONEY,
    };
    assert_eq!(handled(&mut app, vec![refused]), ["UI_ERROR_MESSAGE:poor"]);
    assert_eq!(
        handled(&mut app, vec![SessionEvent::StableResult { result: 11 }]),
        ["PET_STABLE_UPDATE:nil"]
    );
}

/// The creature answer's record a getter missed, as the server sends it.
fn answer(found: bool) -> SessionEvent {
    SessionEvent::CreatureName {
        entry: BOAR,
        name: found.then(|| "Boar".into()),
        subname: None,
        creature_type: Some(1),
        pet_family: 5,
        rank: 0,
        type_flags: 0,
        pet_spell_list: 0,
        display_id: 607,
        civilian: false,
        racial_leader: false,
    }
}

/// Each `GetStablePetInfo` call that misses the record keeps the callback `0x4cb3b0` (flag 0),
/// the cache asking once, owned by the active player; the answer runs each one, which signals
/// `PET_STABLE_UPDATE` inside its handler (`0x4cb3bf`), and a held record keeps none.
#[test]
fn a_missed_record_signals_the_update_once_per_call_when_it_lands() {
    let (mut app, rx) = client();
    let list = SessionEvent::ListStabledPets {
        npc: MASTER,
        num_stable_slots: 1,
        pets: vec![boar()],
    };
    handled(&mut app, vec![list]);
    let _ = rx.try_iter().count();
    eval::<()>(&mut app, true, "GetStablePetInfo(1) GetStablePetInfo(1)");
    let queries: Vec<_> = rx
        .try_iter()
        .filter(|c| matches!(c, ClientCommand::CreatureQuery { .. }))
        .collect();
    assert_eq!(
        format!("{queries:?}"),
        format!("[CreatureQuery {{ entry: {BOAR}, guid: {ME} }}]")
    );
    assert_eq!(
        handled(&mut app, vec![answer(true)]),
        ["PET_STABLE_UPDATE:nil", "PET_STABLE_UPDATE:nil"]
    );
    eval::<()>(&mut app, true, "GetStablePetInfo(1)");
    assert_eq!(
        handled(&mut app, vec![answer(true)]),
        Vec::<String>::new(),
        "a held record keeps no callback"
    );
}

/// A refusal's line shows in `SMSG_STABLE_RESULT`'s handler (`0x4cacb0` → `0x496720`), so its
/// listener reads the stable the packets before it left: a list after it in the frame is not there
/// yet, and a list before it is.
#[test]
fn a_refusals_listener_reads_the_stable_the_packets_before_it_left() {
    for list_first in [false, true] {
        let (mut app, _rx) = client();
        let list = |slots: u8| SessionEvent::ListStabledPets {
            npc: MASTER,
            num_stable_slots: slots,
            pets: vec![boar()],
        };
        handled(&mut app, vec![list(0)]);
        eval::<()>(
            &mut app,
            false,
            "local g = CreateFrame('Frame') g:RegisterEvent('UI_ERROR_MESSAGE') \
             g:SetScript('OnEvent', function() SLOTS = GetNumStableSlots() end)",
        );
        let refused = SessionEvent::StableResult {
            result: stable_result::ERR_MONEY,
        };
        let packets = if list_first {
            vec![list(2), refused]
        } else {
            vec![refused, list(2)]
        };
        crate::net::handlers::dispatch(app.world_mut(), packets);
        app.world_mut().flush();
        crate::lanes::fire_in_order(app.world_mut());
        let read = eval::<i64>(&mut app, false, "return SLOTS");
        assert_eq!(
            read,
            if list_first { 2 } else { 0 },
            "list first {list_first}"
        );
        assert_eq!(eval::<i64>(&mut app, true, "return GetNumStableSlots()"), 2);
    }
}
