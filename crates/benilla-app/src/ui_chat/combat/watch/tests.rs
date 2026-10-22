//! The aura lines' words and gates (`0x62b480`).

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use benilla_protocol::field::FIELD_UNIT_AURA;
use benilla_protocol::messages::ObjectType;

use crate::net::{FieldChanged, GuidIndex, ObjectStore, Reputations, SelfGuid};
use crate::ui_chat::combat::{CombatLogRanges, LineWord};
use crate::ui_chat::ChatLog;

const ME: u64 = 1;
const MOB: u64 = 2;
const FORTITUDE: u32 = 1243;
const CORRUPTION: u32 = 172;
/// `Attributes & 0x40`, a passive spell that a slot can still hold.
const PASSIVE: u32 = 20_000;

/// You and a creature, alive or dead, the three spells known, the log empty.
fn world(health: u32) -> App {
    let display = |id, name: &str, attributes| {
        (
            id,
            benilla_formats::SpellDisplay {
                id,
                name: name.into(),
                attributes,
                ..Default::default()
            },
        )
    };
    let mut app = App::new();
    app.add_message::<FieldChanged>()
        .init_resource::<GuidIndex>()
        .init_resource::<crate::ui_party::GroupState>()
        .init_resource::<Reputations>()
        .init_resource::<CombatLogRanges>()
        .init_resource::<ChatLog>()
        .insert_resource(SelfGuid(Some(ME)))
        .insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    display(FORTITUDE, "Power Word: Fortitude", 0),
                    display(CORRUPTION, "Corruption", 0),
                    display(PASSIVE, "Passive", 0x40),
                ]
                .into_iter()
                .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        });
    for (guid, kind) in [(ME, 0x19), (MOB, 0x09)] {
        let e = app
            .world_mut()
            .spawn((
                Transform::default(),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (2, kind),
                    (22, health),
                ])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(guid, e);
    }
    app
}

/// One aura slot of `guid` moving from `old` to `new`: each line's family stem and word.
fn slot(health: u32, guid: u64, slot: u16, old: u32, new: u32) -> Vec<(String, Option<LineWord>)> {
    let mut app = world(health);
    let entity = app.world().resource::<GuidIndex>().0[&guid];
    app.world_mut().write_message(FieldChanged {
        at: crate::lanes::stamp(),
        entity,
        guid,
        kind: if guid == ME {
            ObjectType::Player
        } else {
            ObjectType::Unit
        },
        index: FIELD_UNIT_AURA + slot,
        old,
        new,
    });
    app.world_mut().run_system_once(super::aura_lines).unwrap();
    app.world()
        .resource::<ChatLog>()
        .pending_combat()
        .into_iter()
        .map(|l| (l.family.key(l.variant), l.word.clone()))
        .collect()
}

/// `0x62b480` raises `AURA_START`, `AURA_START_HARMFUL`, `AURA_END` or `AURA_END_HARMFUL` with the
/// spell's name after its line on you (`0x62b74f`-`0x62b7ac`); harmful is slots 32 to 47
/// (`0x61238e`).
#[test]
fn an_aura_on_you_raises_its_start_or_end_word() {
    let word = |m, s| Some(LineWord::with(m, s));
    assert_eq!(
        slot(100, ME, 0, 0, FORTITUDE),
        [(
            "AURAADDEDSELFHELPFUL".into(),
            word("AURA_START", "Power Word: Fortitude")
        )]
    );
    assert_eq!(
        slot(100, ME, 0x20, 0, CORRUPTION),
        [(
            "AURAADDEDSELFHARMFUL".into(),
            word("AURA_START_HARMFUL", "Corruption")
        )]
    );
    assert_eq!(
        slot(100, ME, 0, FORTITUDE, 0),
        [(
            "AURAREMOVEDSELF".into(),
            word("AURA_END", "Power Word: Fortitude")
        )]
    );
    assert_eq!(
        slot(100, ME, 0x2f, CORRUPTION, 0),
        [(
            "AURAREMOVEDSELF".into(),
            word("AURA_END_HARMFUL", "Corruption")
        )]
    );
    assert_eq!(
        slot(100, MOB, 0, 0, FORTITUDE),
        [("AURAADDEDOTHERHELPFUL".into(), None)]
    );
}

/// `0x62b480` writes nothing for a passive spell (`Attributes & 0x40`, `0x62b4fb`), and nothing for
/// an aura leaving a unit with no health (`0x62b503`-`0x62b515`): no line and no word.
#[test]
fn a_passive_aura_or_one_leaving_the_dead_writes_nothing() {
    assert!(slot(100, ME, 0, 0, PASSIVE).is_empty());
    assert!(slot(100, ME, 0, PASSIVE, 0).is_empty());
    assert!(slot(0, ME, 0, FORTITUDE, 0).is_empty(), "dead");
    assert_eq!(
        slot(0, ME, 0, 0, FORTITUDE).len(),
        1,
        "an arrival on the dead still writes"
    );
}

/// An aura line takes its field edge's place among what the packets queued: the reference's
/// callback writes it inside the update's handler (`0x465330`), so a later packet's item fires
/// behind it though this system runs after both were handled.
#[test]
fn an_aura_line_fires_at_its_field_edges_place() {
    let mut app = world(100);
    let entity = app.world().resource::<GuidIndex>().0[&ME];
    app.world_mut().write_message(FieldChanged {
        at: crate::lanes::stamp(),
        entity,
        guid: ME,
        kind: ObjectType::Player,
        index: FIELD_UNIT_AURA,
        old: 0,
        new: FORTITUDE,
    });
    app.world().resource::<ChatLog>().push_combat_edge(true);
    app.world_mut().run_system_once(super::aura_lines).unwrap();
    assert_eq!(
        app.world().resource::<ChatLog>().pending_kinds(),
        ["combat", "PLAYER_ENTER_COMBAT"]
    );
}
