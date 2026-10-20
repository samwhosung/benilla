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

/// The book holds the three, the bar nothing; the player alive, its aura state `aura_state`.
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
        .insert_resource(NetCommands(tx));
    seat(&mut app);
    app.insert_non_send_resource(listener());
    app.add_systems(Update, super::state::feed_action_state);
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

/// Set the player's aura state and run the feed once: what the VM heard.
fn step(app: &mut App, aura_state: u32) -> Vec<String> {
    let mut q = app
        .world_mut()
        .query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
    let mut store = q.single_mut(app.world_mut()).unwrap();
    store.0 = ObjectFields::from_pairs(&[(22, 100), (23, 500), (AURA_STATE, aura_state)]);
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
/// game's start (`0x48f59f` → `0x4b21c0`) and the active player's arrival (`0x5dea86`), so the new
/// VM hears no `SPELL_ACTIVE` for a spell already usable; a new arrival empties the list.
#[test]
fn a_reload_keeps_the_reactive_list_and_an_arrival_empties_it() {
    let mut app = app();
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"]
    );
    // The seated `/reload`: the leave-world sweep, then a new VM.
    assert!(crate::ui_script::run_leave_world_sweep(app.world_mut()));
    app.insert_non_send_resource(listener());
    assert!(step(&mut app, 0x1).is_empty(), "the list kept its answer");

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
/// entry and its answer outlive the spell leaving the book, and relearning a rank no higher moves
/// nothing (`0x4b2838`).
#[test]
fn the_reactive_list_keeps_a_spell_the_book_lost() {
    let mut app = app();
    assert_eq!(
        step(&mut app, 0x1),
        ["SPELL_ACTIVE Revenge", "SPELL_UPDATE_USABLE"]
    );
    fn spells(app: &mut App) -> Mut<'_, PlayerActions> {
        app.world_mut().resource_mut::<PlayerActions>()
    }
    spells(&mut app).spells.retain(|id| *id == FIREBALL);
    assert!(step(&mut app, 0x1).is_empty(), "both ranks unlearned");
    spells(&mut app).spells.insert(REVENGE);
    assert!(
        step(&mut app, 0x1).is_empty(),
        "rank 1 relearned: the entry kept its answer, no second word"
    );
    assert_eq!(
        step(&mut app, 0),
        ["SPELL_UPDATE_USABLE"],
        "the kept entry still checks"
    );
}
