//! The trainer getters as a listener reads them, and the trainer's events at their packets. The
//! getters read a service's required abilities against the book at the call (`0x4d97c8` →
//! `0x60c740`), its icon's and description's products off the item cache (`0x4d9096`,
//! `0x4d9d07`); the list signals `TRAINER_SHOW` inside its handler (`0x4d7b73`), and each caller of
//! the state pass its `TRAINER_UPDATE` inside its own: the coinage watch (`0x5ddf41`), a learned
//! spell (`0x5e9fb7`) and a product record a getter missed (`0x4d914a`).

use benilla_formats::{SpellCatalog, SpellDisplay, SPELL_EFFECT_LEARN_SPELL};
use benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE;
use benilla_protocol::messages::{trainer_spell_state, TrainerSpell};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use std::collections::HashMap;

use super::unit_view_tests::{created, values};
use crate::ui_trainer::{TrainerEvent, TrainerLane, TrainerOpen};

const ME: u64 = 0x10;
const TRAINER: u64 = 0xF130_0000_4500_0001;
/// The service: a tradeskill wrapper teaching [`TAUGHT`], which creates [`PRODUCT`], and needing
/// [`REQUIRED`] known.
const WRAPPER: u32 = 100;
const TAUGHT: u32 = 200;
const REQUIRED: u32 = 300;
const PRODUCT: u32 = 777;

fn catalog() -> SpellCatalog {
    let wrapper = SpellDisplay {
        name: "Copper Shortsword".into(),
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [TAUGHT, 0, 0],
        ..Default::default()
    };
    let taught = SpellDisplay {
        name: "Copper Shortsword".into(),
        effect_item_type: [PRODUCT, 0, 0],
        ..Default::default()
    };
    let required = SpellDisplay {
        name: "Smelt Copper".into(),
        ..Default::default()
    };
    SpellCatalog::from_displays(HashMap::from([
        (WRAPPER, wrapper),
        (TAUGHT, taught),
        (REQUIRED, required),
    ]))
}

/// The spy: each trainer event in `Seen`, and in `Rows` with the rows the VM holds as it fires.
const SPY: &str = r#"
    Seen = {} Rows = {}
    local f = CreateFrame("Frame")
    f:RegisterEvent("TRAINER_SHOW")
    f:RegisterEvent("TRAINER_UPDATE")
    f:RegisterEvent("TRAINER_CLOSED")
    f:SetScript("OnEvent", function()
        if event ~= "TRAINER_CLOSED" then table.insert(Seen, event) end
        table.insert(Rows, event .. ":" .. GetNumTrainerServices())
    end)
"#;

/// The client with us in the world and a tradeskill trainer asked for, and a VM whose own copy
/// holds what the lane last pushed: its spy notes each trainer event.
fn client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.insert_resource(crate::ui_action::Spells {
        catalog: catalog(),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.insert_resource(crate::ui_spellbook::SkillLines {
        catalog: benilla_formats::SkillLineCatalog::from_spell_lines([]),
    });
    let world = app.world_mut();
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    world.resource_mut::<TrainerOpen>().asked = TRAINER;
    let script = UiScript::new().expect("a VM");
    script.run(SPY).unwrap();
    world.insert_non_send_resource(script);
    crate::net::handlers::dispatch(
        world,
        vec![
            created(ME, EntityKind::Player, &[(FIELD_PLAYER_FIELD_COINAGE, 100)]),
            SessionEvent::ObjectUpdateEnd,
        ],
    );
    world.flush();
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::net::SelfPlayer);
    app
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

/// `packets` through their handlers, each item they put on the trainer lane stamped inside them,
/// none heard before the drain; then the drain, and what the spy heard.
fn handled(app: &mut App, packets: Vec<SessionEvent>) -> (Vec<TrainerEvent>, Vec<String>) {
    let before = crate::lanes::stamp();
    crate::net::handlers::dispatch(app.world_mut(), packets);
    app.world_mut().flush();
    let after = crate::lanes::stamp();
    let stamped = app.world().resource::<TrainerLane>().stamped();
    assert!(
        stamped.iter().all(|(at, _)| before < *at && *at < after),
        "stamped inside the handler"
    );
    let heard: Vec<String> = eval(app, false, "return Seen");
    assert!(heard.is_empty(), "on the lane: {heard:?}");
    crate::lanes::fire_in_order(app.world_mut());
    let heard = eval(app, false, "local s = Seen Seen = {} return s");
    (stamped.into_iter().map(|(_, e)| e).collect(), heard)
}

fn list() -> SessionEvent {
    SessionEvent::TrainerList {
        trainer: TRAINER,
        trainer_type: 2,
        services: vec![TrainerSpell {
            spell: WRAPPER,
            state: trainer_spell_state::GREEN,
            cost: 10,
            can_learn_primary_prof: false,
            is_primary_prof_first_rank: false,
            req_level: 1,
            req_skill: 0,
            req_skill_value: 0,
            req_spells: [REQUIRED, 0, 0],
        }],
        greeting: String::new(),
    }
}

/// With the host lent `GetTrainerServiceAbilityReq` reads the book as it stands, where the VM's
/// copy holds the book as the list's push left it.
#[test]
fn the_ability_getter_reads_the_book_through_the_host() {
    let mut app = client();
    assert_eq!(
        handled(&mut app, vec![list()]),
        (vec![TrainerEvent::Show], vec!["TRAINER_SHOW".into()])
    );
    // Row 1 is the group's header.
    let read = "local name, met = GetTrainerServiceAbilityReq(2, 1) return name, met";
    let unmet = (Some("Smelt Copper".to_string()), None);
    assert_eq!(
        eval::<(Option<String>, Option<i64>)>(&mut app, false, read),
        unmet
    );
    assert_eq!(
        eval::<(Option<String>, Option<i64>)>(&mut app, true, read),
        unmet
    );
    app.world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .insert(REQUIRED);
    assert_eq!(
        eval::<(Option<String>, Option<i64>)>(&mut app, false, read),
        unmet,
        "the copy"
    );
    assert_eq!(
        eval::<(Option<String>, Option<i64>)>(&mut app, true, read),
        (Some("Smelt Copper".to_string()), Some(1)),
        "live"
    );
}

/// The list's `TRAINER_SHOW` and the passes' `TRAINER_UPDATE`s sit on the trainer lane at their
/// handler's place, and fire there in the drain: the coinage's, a learned spell's, and a product
/// record's whose lookup the icon getter's call missed, which keeps one callback per call.
#[test]
fn the_list_and_the_passes_fire_on_the_lane_at_their_handlers() {
    let mut app = client();
    let show = (vec![TrainerEvent::Show], vec!["TRAINER_SHOW".to_string()]);
    assert_eq!(handled(&mut app, vec![list()]), show);
    let update = |n: usize| {
        (
            vec![TrainerEvent::Update; n],
            vec!["TRAINER_UPDATE".to_string(); n],
        )
    };
    let money = values(ME, &[(FIELD_PLAYER_FIELD_COINAGE, 200)]);
    let money = vec![money, SessionEvent::ObjectUpdateEnd];
    assert_eq!(handled(&mut app, money), update(1));
    let learned = SessionEvent::SpellLearned { spell_id: REQUIRED };
    assert_eq!(handled(&mut app, vec![learned]), update(1));
    // With our player, the old rank's unlearn runs none (`0x5e6392`), the new rank's learn one
    // (`0x5e63a4`) and the handler its own (`0x5e64f1`).
    let superceded = SessionEvent::SpellSuperceded {
        old_spell_id: REQUIRED,
        new_spell_id: REQUIRED + 1,
    };
    assert_eq!(handled(&mut app, vec![superceded]), update(2));
    eval::<()>(
        &mut app,
        true,
        "GetTrainerServiceIcon(2) GetTrainerServiceIcon(2)",
    );
    let record = SessionEvent::ItemTemplate {
        entry: PRODUCT,
        info: Some(Box::new(crate::items::test_template("Copper Shortsword"))),
    };
    assert_eq!(handled(&mut app, vec![record]), update(2));
}

/// The rows the VM holds at each trainer event since the last call.
fn rows(app: &mut App) -> Vec<String> {
    eval(app, false, "local r = Rows Rows = {} return r")
}

/// A list or a pass whose window a close ends in the same frame still signals, at its packet's
/// place, over the rows; the close's two `TRAINER_CLOSED`s (`0x4d7468`, and the release's
/// re-entry, `0x493434`) follow it, and the rows clear after them (`0x4d7481`, `0x4d7486`).
#[test]
fn a_close_behind_a_list_or_a_pass_fires_after_it() {
    let mut app = client();
    crate::net::handlers::dispatch(app.world_mut(), vec![list()]);
    app.world_mut().flush();
    crate::ui_trainer::close_core(app.world_mut());
    crate::lanes::fire_in_order(app.world_mut());
    assert_eq!(
        rows(&mut app),
        ["TRAINER_SHOW:2", "TRAINER_CLOSED:2", "TRAINER_CLOSED:2"]
    );
    assert_eq!(
        eval::<i64>(&mut app, false, "return GetNumTrainerServices()"),
        0
    );

    crate::net::handlers::dispatch(app.world_mut(), vec![list()]);
    app.world_mut().flush();
    crate::lanes::fire_in_order(app.world_mut());
    let _ = rows(&mut app);
    let money = values(ME, &[(FIELD_PLAYER_FIELD_COINAGE, 300)]);
    crate::net::handlers::dispatch(app.world_mut(), vec![money, SessionEvent::ObjectUpdateEnd]);
    crate::ui_trainer::close_core(app.world_mut());
    crate::lanes::fire_in_order(app.world_mut());
    assert_eq!(
        rows(&mut app),
        ["TRAINER_UPDATE:2", "TRAINER_CLOSED:2", "TRAINER_CLOSED:2"]
    );
    assert_eq!(
        eval::<i64>(&mut app, false, "return GetNumTrainerServices()"),
        0
    );
}

/// A `/reload`'s VM answers from the rows the engine holds (`0xb737b0`, `0xb73a10`), with no
/// `TRAINER_SHOW` of its own.
#[test]
fn a_reloaded_vm_reads_the_held_rows_without_a_show() {
    let mut app = client();
    handled(&mut app, vec![list()]);
    let mut fresh = UiScript::new().expect("a VM");
    fresh.run(SPY).unwrap();
    crate::ui_trainer::seed_reloaded_vm(app.world_mut(), &mut fresh);
    app.world_mut().insert_non_send_resource(fresh);
    assert!(rows(&mut app).is_empty(), "no event");
    let read = "local name = GetTrainerServiceInfo(2) return GetNumTrainerServices(), name";
    assert_eq!(
        eval::<(i64, Option<String>)>(&mut app, true, read),
        (2, Some("Copper Shortsword".into()))
    );
}
