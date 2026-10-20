//! `SPELL_ACTIVE`, the reactive list's word (`0x4b31c0`), off the usable feed.

use bevy::prelude::*;

use benilla_formats::SpellDisplay;
use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;

use super::{PlayerActions, Spells};
use crate::net::{NetCommands, ObjectStore, SelfPlayer};

/// Two ranks of a reactive spell (`AttributesEx4 & 0x200`) that needs the defense aura state,
/// and a plain spell.
const REVENGE: u32 = 6572;
const REVENGE_2: u32 = 6574;
const FIREBALL: u32 = 133;

/// `UNIT_FIELD_AURASTATE`, whose bit 0 is the defense state Revenge needs.
const AURA_STATE: u16 = 125;

/// The player's guid.
const ME: u64 = 0x10;

/// The book holds the three, the bar nothing; the player alive, its aura state 0, the feed's
/// first frame run.
fn app() -> App {
    let revenge = |id, rank: &str| {
        (
            id,
            SpellDisplay {
                id,
                name: "Revenge".into(),
                rank: Some(rank.into()),
                attributes_ex4: 0x200,
                caster_aura_state: 1,
                ..Default::default()
            },
        )
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = PlayerActions::default();
    actions.spells.extend([REVENGE, REVENGE_2, FIREBALL]);
    app.insert_resource(actions)
        .insert_resource(crate::ui_macro::MacroBoundSpells::default())
        .insert_resource(Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    revenge(REVENGE, "Rank 1"),
                    revenge(REVENGE_2, "Rank 2"),
                    (
                        FIREBALL,
                        SpellDisplay {
                            id: FIREBALL,
                            name: "Fireball".into(),
                            ..Default::default()
                        },
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            ..Spells::empty_for_tests()
        })
        .init_resource::<crate::player::Player>()
        .init_resource::<crate::spell::Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<crate::spell::AutoRepeatActive>()
        .init_resource::<crate::creature_anim::UnitEdits>()
        .add_message::<crate::player::StandStateRequest>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::SpellTargeting>()
        .init_resource::<crate::spell::HeldForPick>()
        .init_resource::<crate::target::Selection>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::Reputations>()
        .init_resource::<crate::items::Items>()
        // The leave-world sweep's latch and count, as the app holds them.
        .init_resource::<crate::ui_script::LeavingWorldArmed>()
        .init_resource::<crate::ui_script::SweptWorlds>()
        .insert_resource(NetCommands(tx))
        // The usable refresh's callers and the cast targeting the walk reads.
        .add_message::<crate::net::FieldChanged>()
        .add_message::<crate::item_watch::ItemNotice>()
        .add_message::<crate::net::WorldEnterCascadeMessage>()
        .insert_resource(crate::net::SelfGuid(Some(ME)))
        .init_resource::<crate::spell::AutoSelfCast>()
        .init_resource::<super::state::ActionEvents>();
    seat(&mut app);
    app.insert_non_send_resource(listener());
    app.add_systems(
        Update,
        (
            super::state::feed_action_state,
            super::state::fire_action_events,
        )
            .chain(),
    );
    // The VM's first frame, which meets the state as it stands and hears no refresh.
    app.update();
    app
}

/// Create the active player, as its arrival does, its world latch armed.
fn seat(app: &mut App) {
    app.world_mut().spawn((
        SelfPlayer,
        Transform::default(),
        ObjectStore(ObjectFields::from_pairs(&[(22, 100), (23, 500)])),
    ));
    app.world_mut()
        .resource_mut::<crate::ui_script::LeavingWorldArmed>()
        .arm();
}

/// A VM that records the reactive word and the spells' usable event.
fn listener() -> UiScript {
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"
            HEARD = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("COMBAT_TEXT_UPDATE")
            f:RegisterEvent("SPELL_UPDATE_USABLE")
            f:SetScript("OnEvent", function()
                if event == "COMBAT_TEXT_UPDATE" then
                    table.insert(HEARD, arg1 .. " " .. tostring(arg2))
                else
                    table.insert(HEARD, event)
                end
            end)
        "#,
        )
        .unwrap();
    script
}

/// Set the player's aura state, its watch's edge (`0x6048b6`, a caller of the usable refresh) on a
/// change, and run the feed once: what the VM heard.
fn step(app: &mut App, aura_state: u32) -> Vec<String> {
    let mut q = app
        .world_mut()
        .query_filtered::<(Entity, &mut ObjectStore), With<SelfPlayer>>();
    let (entity, mut store) = q.single_mut(app.world_mut()).unwrap();
    let old = store.0.unit_aura_state();
    store.0 = ObjectFields::from_pairs(&[(22, 100), (23, 500), (AURA_STATE, aura_state)]);
    if old != aura_state {
        app.world_mut().write_message(crate::net::FieldChanged {
            entity,
            guid: ME,
            kind: benilla_protocol::messages::ObjectType::Player,
            index: AURA_STATE,
            old,
            new: aura_state,
        });
    }
    app.update();
    let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
    s.resolve();
    s.eval::<Vec<String>>("local h = HEARD; HEARD = {}; return h")
        .unwrap()
}

/// `0x4b31c0` re-checks each reactive spell the book holds, one per name (`0x4b2720`-`0x4b2844`),
/// whether or not a bar carries it, and raises `SPELL_ACTIVE` with its name on each turn to usable
/// (`0x4b320b`-`0x4b3226`), ahead of `SPELL_UPDATE_USABLE` (`0x4b3246`); a plain spell never does.
#[test]
fn a_reactive_spell_turning_usable_raises_spell_active() {
    let mut app = app();
    assert!(
        !step(&mut app, 0)
            .iter()
            .any(|e| e.starts_with("SPELL_ACTIVE")),
        "not usable yet"
    );
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"],
        "one word for the two ranks, before the usable event"
    );
    assert!(step(&mut app, 0x1).is_empty(), "still usable: nothing");
    assert_eq!(
        step(&mut app, 0),
        ["SPELL_UPDATE_USABLE"],
        "unusable again: the event, no word"
    );
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"]
    );
}

/// A `/reload` (`0x495664` → `0x490bd0`, `0x48fbf0`) reaches neither of `0x4b23a0`'s callers, the
/// game's start (`0x48f59f` → `0x4b21c0`) and the active player's arrival (`0x5dea86`), so the
/// entry and its answer outlive the VM: the new VM's first refresh raises no `SPELL_ACTIVE` for a
/// spell already usable, and the entry still words its next turn. A new arrival empties the list.
#[test]
fn a_reload_keeps_the_reactive_list_and_an_arrival_empties_it() {
    let mut app = app();
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"]
    );
    // The seated `/reload`: the leave-world sweep, then a new VM and its first frame.
    assert!(crate::ui_script::run_leave_world_sweep(app.world_mut()));
    app.insert_non_send_resource(listener());
    assert!(step(&mut app, 0x1).is_empty(), "the new VM's first frame");
    select(&mut app, 0x20);
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_UPDATE_USABLE"],
        "a refresh: the list kept its answer, no second word"
    );
    assert_eq!(step(&mut app, 0), ["SPELL_UPDATE_USABLE"]);
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"],
        "the kept entry words its next turn"
    );

    let mut me = app.world_mut().query_filtered::<Entity, With<SelfPlayer>>();
    let me = me.single(app.world()).unwrap();
    app.world_mut().despawn(me);
    seat(&mut app);
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"],
        "a new arrival starts the list over"
    );
}

/// The list's only writers are its clear, its add (`0x4b27b3`-`0x4b27c7`), its rank move
/// (`0x4b2844`) and the refresh (`0x4b3231`); unlearning (`0x4b2c50`) never touches it, so an
/// entry and its answer outlive the spell leaving the book, and the entry still checks.
#[test]
fn the_reactive_list_keeps_a_spell_the_book_lost() {
    let mut app = app();
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"]
    );
    book(&mut app).spells.retain(|id| *id == FIREBALL);
    assert_eq!(
        step(&mut app, 0),
        ["SPELL_UPDATE_USABLE"],
        "both ranks unlearned: the kept entry still checks"
    );
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"],
        "and words its turn"
    );
}

/// Relearning a rank no higher than the entry's moves nothing (`0x4b2838`): the entry the
/// unlearning left keeps its answer, so the next refresh raises no second word.
#[test]
fn relearning_a_lower_rank_keeps_the_entrys_answer() {
    let mut app = app();
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"]
    );
    book(&mut app).spells.retain(|id| *id == FIREBALL);
    assert!(step(&mut app, 0x1).is_empty(), "both ranks unlearned");
    book(&mut app).spells.insert(REVENGE);
    select(&mut app, 0x20);
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_UPDATE_USABLE"],
        "rank 1 relearned: a refresh, the entry kept its answer, no second word"
    );
}

/// The book.
fn book(app: &mut App) -> Mut<'_, PlayerActions> {
    app.world_mut().resource_mut::<PlayerActions>()
}

/// Select `guid`, a caller of the usable refresh (`0x493892`).
fn select(app: &mut App, guid: u64) {
    app.insert_resource(crate::target::Selection {
        target: None,
        guid: Some(guid),
        last: None,
    });
}

/// The re-check is `0x4b31c0`'s own loop (`0x4b31d7`-`0x4b323d`), so it runs inside each of the
/// refresh's callers and nowhere else: a reactive spell that turns usable with no caller raises
/// nothing until one runs, here the target's select (`0x493892`), and then raises its
/// `SPELL_ACTIVE` ahead of the refresh's `SPELL_UPDATE_USABLE`.
#[test]
fn the_reactive_re_check_runs_inside_each_usable_refresh_alone() {
    let mut app = app();
    let mut q = app
        .world_mut()
        .query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
    q.single_mut(app.world_mut()).unwrap().0 =
        ObjectFields::from_pairs(&[(22, 100), (23, 500), (AURA_STATE, 0x1)]);
    assert!(step(&mut app, 0x1).is_empty(), "no caller: no re-check");
    select(&mut app, 0x20);
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"],
        "the select's refresh re-checks the list"
    );
}

/// `0x4b31c0` raises the bar's `ACTIONBAR_UPDATE_USABLE` (`0x4e5c53`), then each reactive word
/// (`0x4b3226`), then `SPELL_UPDATE_USABLE` (`0x4b3246`), all after its pushes: with Revenge on
/// the bar, its turn to usable raises the three in that order.
#[test]
fn the_bars_usable_event_comes_before_the_reactive_word() {
    let mut app = app();
    {
        let mut actions = app.world_mut().resource_mut::<PlayerActions>();
        actions.buttons.insert(
            0,
            benilla_protocol::messages::ActionButton {
                slot: 0,
                action: REVENGE,
                kind: benilla_protocol::messages::ACTION_KIND_SPELL,
            },
        );
        actions.dirty = true;
    }
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"local f = CreateFrame("Frame")
            f:RegisterEvent("ACTIONBAR_UPDATE_USABLE")
            f:SetScript("OnEvent", function() table.insert(HEARD, event) end)"#,
        )
        .unwrap();
    step(&mut app, 0);
    assert_eq!(
        step(&mut app, 0x1),
        [
            "ACTIONBAR_UPDATE_USABLE",
            "SPELL_ACTIVE Revenge",
            "SPELL_UPDATE_USABLE"
        ]
    );
}
