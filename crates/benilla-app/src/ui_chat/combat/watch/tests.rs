//! The aura lines' words, gates and order (`0x604d00`, `0x62b480`, `0x612450`, `0x62b800`).

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use benilla_protocol::field::{FIELD_UNIT_AURA, FIELD_UNIT_AURAAPPLICATIONS, FIELD_UNIT_AURAFLAGS};
use benilla_protocol::messages::ObjectType;

use crate::net::{AuraFieldsChanged, FieldChanged, GuidIndex, ObjectStore, Reputations, SelfGuid};
use crate::ui_chat::combat::{CombatLogRanges, LineWord};
use crate::ui_chat::ChatLog;

const ME: u64 = 1;
const MOB: u64 = 2;
const FORTITUDE: u32 = 1243;
const INTELLECT: u32 = 1459;
const MARK: u32 = 1126;
const CORRUPTION: u32 = 172;
/// `Attributes & 0x40`, a passive spell that a slot can still hold.
const PASSIVE: u32 = 20_000;
/// Charges and no `StackAmount`, as Lightning Shield's record has.
const SHIELD: u32 = 324;
/// `StackAmount` 5, as Sunder Armor's record has.
const SUNDER: u32 = 7386;

/// A slot's `UNIT_FIELD_AURAFLAGS` nibble for an aura vmangos shows: an effect bit of 0xe and
/// `AFLAG_POSITIVE`.
const PRESENT: u32 = 0x9;

/// You and a creature, alive or dead, the spells known, the log empty.
fn world(health: u32) -> App {
    let display = |id, name: &str, attributes, stack_amount| {
        (
            id,
            benilla_formats::SpellDisplay {
                id,
                name: name.into(),
                attributes,
                stack_amount,
                ..Default::default()
            },
        )
    };
    let mut app = App::new();
    app.add_message::<FieldChanged>()
        .add_message::<AuraFieldsChanged>()
        .init_resource::<GuidIndex>()
        .init_resource::<crate::ui_party::GroupState>()
        .init_resource::<Reputations>()
        .init_resource::<CombatLogRanges>()
        .init_resource::<ChatLog>()
        .insert_resource(SelfGuid(Some(ME)))
        .insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    display(FORTITUDE, "Power Word: Fortitude", 0, 0),
                    display(INTELLECT, "Arcane Intellect", 0, 0),
                    display(MARK, "Mark of the Wild", 0, 0),
                    display(CORRUPTION, "Corruption", 0, 0),
                    display(PASSIVE, "Passive", 0x40, 0),
                    display(SHIELD, "Lightning Shield", 0, 0),
                    display(SUNDER, "Sunder Armor", 0, 5),
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

fn kind_of(guid: u64) -> ObjectType {
    if guid == ME {
        ObjectType::Player
    } else {
        ObjectType::Unit
    }
}

/// One aura slot's move in a block: `(slot, old spell, old nibble, new spell, new nibble)`.
type Move = (u16, u32, u32, u32, u32);

/// One block's notify on `guid`, as the drain writes it: each moved dword's edge and the aura
/// watcher's one call, at the place of the block's first edge in the range.
fn notify(app: &mut App, guid: u64, moves: &[Move]) {
    let entity = app.world().resource::<GuidIndex>().0[&guid];
    let health = app
        .world()
        .get::<ObjectStore>(entity)
        .and_then(|s| s.0.unit_health())
        .unwrap_or(0);
    let (mut old, mut new) = ([0u32; 54], [0u32; 54]);
    for &(slot, old_spell, old_flags, new_spell, new_flags) in moves {
        let (s, word, shift) = (
            usize::from(slot),
            48 + usize::from(slot / 8),
            (slot % 8) * 4,
        );
        old[s] = old_spell;
        new[s] = new_spell;
        old[word] |= old_flags << shift;
        new[word] |= new_flags << shift;
    }
    let mut first = None;
    for (i, (&was, &now)) in old.iter().zip(&new).enumerate() {
        if was == now {
            continue;
        }
        let index = if i < 48 {
            FIELD_UNIT_AURA + i as u16
        } else {
            FIELD_UNIT_AURAFLAGS + (i - 48) as u16
        };
        let at = crate::lanes::stamp();
        first.get_or_insert(at);
        app.world_mut().write_message(FieldChanged {
            at,
            entity,
            guid,
            kind: kind_of(guid),
            index,
            old: was,
            new: now,
        });
    }
    app.world_mut().write_message(AuraFieldsChanged {
        at: first.expect("a block that moves the range"),
        entity,
        guid,
        old,
        new,
        health,
    });
}

/// The parked lines' family stems and spells, in order.
fn lines(app: &App) -> Vec<(String, String)> {
    app.world()
        .resource::<ChatLog>()
        .pending_combat()
        .into_iter()
        .map(|l| (l.family.key(l.variant), l.fills.spell.clone()))
        .collect()
}

/// One block of `moves` on you: the lines' stems and spells.
fn on_me(moves: &[Move]) -> Vec<(String, String)> {
    let mut app = world(100);
    notify(&mut app, ME, moves);
    app.world_mut().run_system_once(super::aura_lines).unwrap();
    lines(&app)
}

/// One aura slot of `guid` moving from `old` to `new`, present while it holds a spell: each
/// line's family stem and word.
fn slot(health: u32, guid: u64, slot: u16, old: u32, new: u32) -> Vec<(String, Option<LineWord>)> {
    let mut app = world(health);
    let nibble = |spell| if spell == 0 { 0 } else { PRESENT };
    notify(
        &mut app,
        guid,
        &[(slot, old, nibble(old), new, nibble(new))],
    );
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

/// `0x604d00` walks the 48 slots twice: every slot whose old aura is gone or replaced is removed
/// first (`0x604d40`-`0x604dd8`, `0x612320`), then every slot with a new aura is added
/// (`0x604df5`-`0x604e85`, `0x6123f0`), so a replace and a loss beside it word both losses ahead
/// of the arrival.
#[test]
fn one_update_writes_every_loss_before_any_gain() {
    assert_eq!(
        on_me(&[
            (0, FORTITUDE, PRESENT, INTELLECT, PRESENT),
            (1, MARK, PRESENT, 0, 0),
        ]),
        [
            ("AURAREMOVEDSELF".into(), "Power Word: Fortitude".into()),
            ("AURAREMOVEDSELF".into(), "Mark of the Wild".into()),
            ("AURAADDEDSELFHELPFUL".into(), "Arcane Intellect".into()),
        ]
    );
}

/// A slot holds an aura when its spell is nonzero and its `AURAFLAGS` nibble has a bit of 0xe
/// (`0x604d79`, `0x604d98`, `0x604e2e`, `0x604e4d`): the nibble alone arriving or leaving words the
/// line, and a spell under a nibble without those bits words none.
#[test]
fn a_slot_holds_an_aura_only_under_an_effect_bit() {
    assert_eq!(
        on_me(&[(0, FORTITUDE, 0, FORTITUDE, PRESENT)]),
        [(
            "AURAADDEDSELFHELPFUL".into(),
            "Power Word: Fortitude".into()
        )],
        "the nibble arriving under a standing spell"
    );
    assert_eq!(
        on_me(&[(0, FORTITUDE, PRESENT, FORTITUDE, 0)]),
        [("AURAREMOVEDSELF".into(), "Power Word: Fortitude".into())],
        "the nibble leaving"
    );
    assert!(
        on_me(&[(0, 0, 0, FORTITUDE, 0x1)]).is_empty(),
        "no effect bit"
    );
    assert!(
        on_me(&[(0, FORTITUDE, 0x1, 0, 0)]).is_empty(),
        "nothing held to lose"
    );
}

/// An aura line takes its watcher's place among what the packets queued: the reference's
/// callback writes it inside the update's handler (`0x465330`), so a later packet's item fires
/// behind it though this system runs after both were handled.
#[test]
fn an_aura_line_fires_at_its_field_edges_place() {
    let mut app = world(100);
    notify(&mut app, ME, &[(0, 0, 0, FORTITUDE, PRESENT)]);
    app.world().resource::<ChatLog>().push_combat_edge(true);
    app.world_mut().run_system_once(super::aura_lines).unwrap();
    assert_eq!(
        app.world().resource::<ChatLog>().pending_kinds(),
        ["combat", "PLAYER_ENTER_COMBAT"]
    );
}

/// A creature's slot 0 holding `spell` and its `UNIT_FIELD_AURAAPPLICATIONS` byte moving from
/// `before` to `now`, in a block that also brings `spell` into the slot from `arrived_from` when
/// that is given: each line's stem, spell and count.
fn stacks(
    spell: u32,
    before: u32,
    now: u32,
    arrived_from: Option<u32>,
) -> Vec<(String, String, i64)> {
    let mut app = world(100);
    let entity = app.world().resource::<GuidIndex>().0[&MOB];
    app.world_mut().get_mut::<ObjectStore>(entity).unwrap().0 =
        benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x09),
            (22, 100),
            (FIELD_UNIT_AURA, spell),
            (FIELD_UNIT_AURAFLAGS, PRESENT),
            (FIELD_UNIT_AURAAPPLICATIONS, now),
        ]);
    if let Some(old) = arrived_from {
        notify(&mut app, MOB, &[(0, old, 0, spell, PRESENT)]);
    }
    app.world_mut().write_message(FieldChanged {
        at: crate::lanes::stamp(),
        entity,
        guid: MOB,
        kind: ObjectType::Unit,
        index: FIELD_UNIT_AURAAPPLICATIONS,
        old: before,
        new: now,
    });
    app.world_mut().run_system_once(super::aura_lines).unwrap();
    app.world()
        .resource::<ChatLog>()
        .pending_combat()
        .into_iter()
        .map(|l| {
            (
                l.family.key(l.variant),
                l.fills.spell.clone(),
                l.fills.amount,
            )
        })
        .collect()
}

/// `0x62b800` writes `AURAAPPLICATIONADDED…` only for a spell whose `StackAmount`
/// (`[spell+0x9c]`) is above 1 (`0x62b87c`): a charged aura refilled with a higher byte, as
/// vmangos writes `charges × stacks − 1` there, prints nothing.
#[test]
fn a_stack_line_needs_a_spell_that_stacks() {
    assert!(stacks(SHIELD, 0, 2, None).is_empty(), "charges, no stacks");
    assert_eq!(
        stacks(SUNDER, 1, 2, None),
        [(
            "AURAAPPLICATIONADDEDOTHERHELPFUL".into(),
            "Sunder Armor".into(),
            3
        )]
    );
    assert!(stacks(SUNDER, 2, 1, None).is_empty(), "a fall");
}

/// The applications watcher compares the byte alone (`0x612450`: `cmp`, `jbe`), and `0x62b800`
/// reads the slot's spell as the descriptor holds it then (`0x62b822`): a rise in the block that
/// brought the spell into its slot words the stack, behind the arrival.
#[test]
fn a_stack_rise_words_its_line_though_the_slot_changed_spell() {
    assert_eq!(
        stacks(SUNDER, 0, 1, Some(0)),
        [
            ("AURAADDEDOTHERHELPFUL".into(), "Sunder Armor".into(), 0),
            (
                "AURAAPPLICATIONADDEDOTHERHELPFUL".into(),
                "Sunder Armor".into(),
                2
            ),
        ]
    );
}

/// `0x62b800` names the spell the slot holds when the byte rises: a later block in the frame that
/// empties the slot does not take the stack's line away.
#[test]
fn a_stack_line_names_the_spell_its_slot_held_at_the_rise() {
    let mut app = world(100);
    let entity = app.world().resource::<GuidIndex>().0[&MOB];
    // The descriptor after both blocks: the slot emptied, the byte left as it rose.
    app.world_mut().get_mut::<ObjectStore>(entity).unwrap().0 =
        benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x09),
            (22, 100),
            (FIELD_UNIT_AURAAPPLICATIONS, 1),
        ]);
    app.world_mut().write_message(FieldChanged {
        at: crate::lanes::stamp(),
        entity,
        guid: MOB,
        kind: ObjectType::Unit,
        index: FIELD_UNIT_AURAAPPLICATIONS,
        old: 0,
        new: 1,
    });
    notify(&mut app, MOB, &[(0, SUNDER, PRESENT, 0, 0)]);
    app.world_mut().run_system_once(super::aura_lines).unwrap();
    assert_eq!(
        lines(&app),
        [
            (
                "AURAAPPLICATIONADDEDOTHERHELPFUL".into(),
                "Sunder Armor".into()
            ),
            ("AURAREMOVEDOTHER".into(), "Sunder Armor".into()),
        ]
    );
}
