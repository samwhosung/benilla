//! The reputation verbs through the whole client's host: `FactionToggleAtWar` (`0x4d6950` →
//! `0x4d5fd0`), `SetFactionInactive` and `SetFactionActive` (`0x4d69b0`, `0x4d6a00` → `0x4d60f0`)
//! and `SetWatchedFactionIndex` (`0x4d6b60` → `0x4d6240`) send inside their calls, so a chunk's
//! packets leave in call order with every other verb's.

use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;
use crossbeam_channel::Receiver;

use super::*;
use crate::net::{Guid, GuidIndex};
use crate::script_calls::with_hosted_vm;
use benilla_formats::faction_flags as flag;

const ME: u64 = 0x10;
/// `UNIT_FIELD_FLAGS` and its in-combat bit 19 (`0x4d6038`).
const UNIT_FLAGS: u16 = 46;
const IN_COMBAT: u32 = 0x8_0000;
/// `PLAYER_FIELD_WATCHED_FACTION_INDEX`.
const WATCHED: u16 = 1261;

fn entry(faction_id: u32, rep_list_id: u32, parent_id: u32, name: &str) -> FactionEntry {
    FactionEntry {
        faction_id,
        rep_list_id,
        parent_id,
        name: name.into(),
        description: String::new(),
        standing: 4000,
        standing_id: 5,
        bar_min: 3000,
        bar_max: 9000,
        visible: true,
        is_header: false,
        at_war: false,
        can_toggle_at_war: true,
        inactive: false,
    }
}

fn header(faction_id: u32, rep_list_id: u32, name: &str) -> FactionEntry {
    FactionEntry {
        visible: false,
        is_header: true,
        ..entry(faction_id, rep_list_id, 0, name)
    }
}

/// Rows: 1 Alliance, 2 Ironforge, 3 Stormwind, 4 Steamwheedle Cartel, 5 Booty Bay, 6 Other,
/// 7 Argent Dawn, 8 Bloodsail Buccaneers.
fn state() -> ReputationState {
    ReputationState {
        entries: vec![
            entry(72, 19, 469, "Stormwind"),
            entry(529, 13, 0, "Argent Dawn"),
            header(469, 11, "Alliance"),
            entry(21, 1, 169, "Booty Bay"),
            entry(47, 20, 469, "Ironforge"),
            header(169, 10, "Steamwheedle Cartel"),
            entry(87, 0, 0, "Bloodsail Buccaneers"),
        ],
        watched: None,
    }
}

/// The whole client, headless, its VM seated with [`state`], the player's descriptor holding
/// `unit_flags` and watching nothing, and Stormwind's slot 19 holding `stormwind` flags.
fn client(unit_flags: u32, stormwind: u8, list: ReputationState) -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let me = world
        .spawn((
            SelfPlayer,
            Guid(ME),
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x19),
                (UNIT_FLAGS, unit_flags),
                (WATCHED, u32::MAX),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
    world.insert_resource(crate::net::SelfGuid(Some(ME)));
    let mut slots = vec![(flag::VISIBLE, 0); 21];
    slots[19].0 = stormwind;
    world.resource_mut::<Reputations>().set_slots(&slots);
    let mut script = UiScript::new().expect("a VM");
    script.set_reputation(list);
    world.insert_non_send_resource(script);
    (app, rx)
}

fn run(app: &mut App, lua: &str) {
    with_hosted_vm(app.world_mut(), |vm| vm.expect("a VM").run(lua)).expect("the chunk runs");
}

fn eval<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, lua: &str) -> T {
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<T>(lua)
        .expect("the chunk evaluates")
}

/// The packets a chunk sent, the name queries left out.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| !c.contains("Query"))
        .collect()
}

fn roll(min: u32, max: u32) -> String {
    format!("{:?}", ClientCommand::RandomRoll { min, max })
}

/// Each verb sends inside its call: the at-war request (`0x4d60be`), the inactive request
/// (`0x4d6176`) and the watched faction (`0x4d62b4`), between the rolls around them.
#[test]
fn the_reputation_verbs_send_inside_their_calls() {
    let (mut app, rx) = client(0, flag::VISIBLE, state());
    let _ = sent(&rx);
    // Stormwind moves under the folded Inactive header, so Booty Bay is row 4 after it.
    run(
        &mut app,
        "FactionToggleAtWar(3) RandomRoll(1, 2) SetFactionInactive(3) RandomRoll(3, 4) \
         SetWatchedFactionIndex(4) RandomRoll(5, 6)",
    );
    assert_eq!(
        sent(&rx),
        [
            format!(
                "{:?}",
                ClientCommand::SetFactionAtWar {
                    rep_list_id: 19,
                    at_war: true
                }
            ),
            roll(1, 2),
            format!(
                "{:?}",
                ClientCommand::SetFactionInactive {
                    rep_list_id: 19,
                    inactive: true
                }
            ),
            roll(3, 4),
            format!("{:?}", ClientCommand::SetWatchedFaction { rep_list_id: 1 }),
            roll(5, 6),
        ]
    );
}

/// In combat the at-war request shows `ERR_NOT_IN_COMBAT` (`0x4d6040`) inside the call and sends
/// nothing.
#[test]
fn the_at_war_request_refuses_in_combat_inside_the_call() {
    let (mut app, rx) = client(IN_COMBAT, flag::VISIBLE, state());
    let _ = sent(&rx);
    run(
        &mut app,
        r#"ERR_NOT_IN_COMBAT = "You can't do that while in combat"
        local f = CreateFrame("Frame") f:RegisterEvent("UI_ERROR_MESSAGE")
        f:SetScript("OnEvent", function() Refused = arg1 end)
        FactionToggleAtWar(3) InCall = Refused RandomRoll(1, 2)"#,
    );
    assert_eq!(sent(&rx), [roll(1, 2)]);
    assert_eq!(
        eval::<String>(&mut app, "return InCall"),
        "You can't do that while in combat"
    );
    assert!(eval::<benilla_ui::mlua::Value>(
        &mut app,
        "local _,_,_,_,_,_,w = GetFactionInfo(3) return w"
    )
    .is_nil());
}

/// A peace-forced faction still sends the war it was asked for, but its bit is cleared, not set
/// (`0x4d6065`-`0x4d606f`): the stock pane disables the box, an addon can still call the verb.
#[test]
fn a_peace_forced_war_request_sends_and_leaves_the_bit_clear() {
    let mut list = state();
    list.entries[0].can_toggle_at_war = false;
    let (mut app, rx) = client(0, flag::VISIBLE | flag::PEACE_FORCED, list);
    let _ = sent(&rx);
    run(
        &mut app,
        "FactionToggleAtWar(3) local _,_,_,_,_,_,w = GetFactionInfo(3) War = w",
    );
    assert_eq!(
        sent(&rx),
        [format!(
            "{:?}",
            ClientCommand::SetFactionAtWar {
                rep_list_id: 19,
                at_war: true
            }
        )]
    );
    assert!(eval::<benilla_ui::mlua::Value>(&mut app, "return War").is_nil());
}

/// The two requests write the slot's flag byte at once (`0x4d6074`, `0x4d612c`), so the next push
/// of the list keeps what they set.
#[test]
fn the_requests_write_the_slots_flags() {
    let (mut app, _rx) = client(0, flag::VISIBLE, state());
    run(&mut app, "FactionToggleAtWar(3) SetFactionInactive(3)");
    use bevy::ecs::system::RunSystemOnce;
    app.world_mut().run_system_once(feed_reputation).unwrap();
    let flags = app.world().resource::<Reputations>().flags(19);
    assert_eq!(
        flags,
        flag::VISIBLE | flag::AT_WAR | flag::INACTIVE,
        "{flags:#x}"
    );
}

/// `FactionToggleAtWar` writes the slot's at-war bit inside the call (`0x4d6074`), and the
/// player's reaction toward a unit of that faction reads the bit live (`0x60640c` through
/// `0x4d61b0`): `UnitReaction` and `UnitCanAttack` on the next line answer war, with no event
/// between (`0x4d5fd0` signals nothing).
#[test]
fn the_reaction_after_a_toggle_reads_the_bit_the_toggle_wrote() {
    use crate::target::relations::{RelUnit, RelationSnapshot};
    use benilla_formats::{FactionCatalog, FactionTemplate};
    use benilla_ui::script::UnitState;
    use std::collections::HashMap;
    /// A guard of Stormwind (`Faction.dbc` 72, slot 19) and the player's own template.
    const GUARD: u64 = 0xF130_0000_0000_0044;
    let catalog = FactionCatalog::from_rows(
        HashMap::from([
            (
                1,
                FactionTemplate {
                    faction: 1,
                    ..Default::default()
                },
            ),
            (
                11,
                FactionTemplate {
                    faction: 72,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::new(),
    )
    .with_reputation(72, 19, 1, 4000);
    let factions = crate::target::Factions::from_catalog(catalog);
    let (mut app, _rx) = client(0, flag::VISIBLE, state());
    app.insert_resource(factions.clone());
    let me = RelUnit {
        guid: ME,
        is_player: true,
        flags: 0x8,
        template: Some(1),
        race: 1,
        class: 1,
        ..Default::default()
    };
    let guard = RelUnit {
        guid: GUARD,
        template: Some(11),
        ..Default::default()
    };
    let snapshot = RelationSnapshot {
        units: HashMap::from([(ME, me), (GUARD, guard)]),
        factions: Some(factions),
        reputations: app.world().resource::<Reputations>().clone(),
        me: Some(ME),
        roster: Default::default(),
    };
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_unit_relations(Some(std::rc::Rc::new(snapshot)));
        for (token, guid) in [("player", ME), ("target", GUARD)] {
            script.set_unit(
                token,
                Some(UnitState {
                    exists: true,
                    guid,
                    ..Default::default()
                }),
            );
        }
    }
    let ask = r#"R, A = UnitReaction("player", "target"), UnitCanAttack("player", "target")"#;
    run(&mut app, ask);
    assert_eq!(
        eval::<(i64, Option<i64>)>(&mut app, "return R, A"),
        (5, None)
    );
    run(&mut app, &format!("FactionToggleAtWar(3) {ask}"));
    assert_eq!(
        eval::<(i64, Option<i64>)>(&mut app, "return R, A"),
        (2, Some(1)),
        "hostile and attackable on the line after the toggle"
    );
    run(&mut app, &format!("FactionToggleAtWar(3) {ask}"));
    assert_eq!(
        eval::<(i64, Option<i64>)>(&mut app, "return R, A"),
        (5, None),
        "and peace on the line after the next"
    );
}

/// The verbs write the store through `&mut` (`0x4d5fc0`), so a toggle a late handler makes marks
/// it changed in its own frame: a feed gated on the relation inputs re-reads at its next run,
/// whatever its order against the handler, with no fold between.
#[test]
fn a_toggle_from_a_late_handler_wakes_the_feeds_gated_on_the_store() {
    let (mut app, _rx) = client(0, flag::VISIBLE, state());
    let reader = app
        .world_mut()
        .register_system(|w: crate::target::RelationWorld| w.inputs_changed());
    let read = |app: &mut App| app.world_mut().run_system(reader).unwrap();
    read(&mut app);
    assert!(!read(&mut app), "nothing moved");
    run(&mut app, "FactionToggleAtWar(3)");
    assert!(read(&mut app), "the toggle's write");
    assert!(!read(&mut app), "and once");
}
