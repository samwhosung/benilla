//! The latch released by the death of the active player, a shapeshift into a form that cannot
//! interact, and the death of the latched unit.

use bevy::prelude::*;

use benilla_formats::ShapeshiftForm;
use benilla_protocol::field::{FIELD_UNIT_BYTES_1, FIELD_UNIT_HEALTH};
use benilla_protocol::messages::ObjectType;

use super::latch_tests::{harness, latch, open, vocals, BANKER, VENDOR};
use super::release::{form_bars_interaction, release_on_field_edges};
use super::{LatchVocal, NpcWindow};
use crate::net::{FieldChanged, SelfPlayer};
use crate::ui_action::Spells;
use crate::ui_script::sweep_test::seen_in;

const ME: u64 = 0x0000_0000_0000_0001;
const OTHER: u64 = 0x0000_0000_0000_0009;

/// A form row with `flags1` set as given; nothing else of it is read here.
fn form(flags: u32) -> ShapeshiftForm {
    ShapeshiftForm {
        bonus_bar: 0,
        name: String::new(),
        flags,
        creature_type: 0,
        attack_icon: None,
    }
}

/// The latch harness with the release system, the active player and `SpellShapeshiftForm` rows: 1
/// and 5 plain forms, 7 a stance (flag 0x1), 8 a form that may still interact (flag 0x8).
fn app() -> (App, Entity) {
    let mut app = harness();
    let mut spells = Spells::empty_for_tests();
    spells.forms.insert(1, form(0));
    spells.forms.insert(5, form(0x2));
    spells.forms.insert(7, form(0x1 | 0x2));
    spells.forms.insert(8, form(0x8));
    app.insert_resource(spells)
        .add_message::<FieldChanged>()
        .add_systems(Update, release_on_field_edges);
    let me = app.world_mut().spawn(SelfPlayer).id();
    (app, me)
}

fn edge(
    entity: Entity,
    guid: u64,
    kind: ObjectType,
    index: u16,
    old: u32,
    new: u32,
) -> FieldChanged {
    FieldChanged {
        entity,
        guid,
        kind,
        index,
        old,
        new,
    }
}

fn shift(app: &mut App, entity: Entity, old: u8, new: u8) {
    app.world_mut().write_message(edge(
        entity,
        ME,
        ObjectType::Player,
        FIELD_UNIT_BYTES_1,
        u32::from(old) << 16 | 0x0102,
        u32::from(new) << 16 | 0x0102,
    ));
    app.update();
}

/// `0x5e9a51`: the active player dying releases the latch with the goodbye (the player who is
/// dead hears none, `0x605f30`), closing the window; another player dying does not.
#[test]
fn the_active_players_death_closes_the_window() {
    let (mut app, me) = app();
    let other = app.world_mut().spawn_empty().id();
    open(&mut app, NpcWindow::Merchant, VENDOR);
    vocals(&mut app);

    // Health falls to 0 on another player, and rises on the active one: nothing.
    app.world_mut().write_message(edge(
        other,
        OTHER,
        ObjectType::Player,
        FIELD_UNIT_HEALTH,
        50,
        0,
    ));
    app.world_mut()
        .write_message(edge(me, ME, ObjectType::Player, FIELD_UNIT_HEALTH, 0, 50));
    app.world_mut()
        .write_message(edge(me, ME, ObjectType::Player, FIELD_UNIT_HEALTH, 50, 20));
    app.update();
    assert_eq!(latch(&app), Some(VENDOR));
    assert!(seen_in(&app).is_empty());

    app.world_mut()
        .write_message(edge(me, ME, ObjectType::Player, FIELD_UNIT_HEALTH, 20, 0));
    app.update();
    assert_eq!(seen_in(&app), ["MERCHANT_CLOSED"]);
    assert_eq!(latch(&app), None);
    assert_eq!(vocals(&mut app), [LatchVocal::Goodbye(VENDOR)]);
}

/// `0x5de07f`: a form without flag 0x8 closes the window with the goodbye; none, a stance, a form
/// that may interact, an unknown form, another unit's change and a move of another byte do not.
#[test]
fn a_shapeshift_into_a_form_that_cannot_interact_closes_the_window() {
    let (mut app, me) = app();
    let other = app.world_mut().spawn_empty().id();
    open(&mut app, NpcWindow::Bank, BANKER);
    vocals(&mut app);

    shift(&mut app, me, 1, 0);
    shift(&mut app, me, 0, 7);
    shift(&mut app, me, 0, 8);
    shift(&mut app, me, 0, 99);
    shift(&mut app, other, 0, 1);
    // The form byte unchanged, the byte beside it moved.
    app.world_mut().write_message(edge(
        me,
        ME,
        ObjectType::Player,
        FIELD_UNIT_BYTES_1,
        1 << 16,
        1 << 16 | 0x0100,
    ));
    app.update();
    assert_eq!(latch(&app), Some(BANKER));
    assert!(seen_in(&app).is_empty());

    shift(&mut app, me, 0, 1);
    assert_eq!(seen_in(&app), ["BANKFRAME_CLOSED"]);
    assert_eq!(latch(&app), None);
    assert_eq!(vocals(&mut app), [LatchVocal::Goodbye(BANKER)]);

    // A form on a plain, non-stance row with the interact flag clear, from another form.
    open(&mut app, NpcWindow::Merchant, VENDOR);
    shift(&mut app, me, 1, 5);
    assert_eq!(latch(&app), None);
}

/// `0x605950`: the latched unit dying closes its window with no goodbye (`dl = 1`); another
/// unit's death leaves it.
#[test]
fn the_latched_units_death_closes_its_window_without_a_goodbye() {
    let (mut app, _) = app();
    let vendor = app.world_mut().spawn_empty().id();
    let bystander = app.world_mut().spawn_empty().id();
    open(&mut app, NpcWindow::Merchant, VENDOR);
    vocals(&mut app);

    app.world_mut().write_message(edge(
        bystander,
        OTHER,
        ObjectType::Unit,
        FIELD_UNIT_HEALTH,
        50,
        0,
    ));
    app.update();
    assert_eq!(latch(&app), Some(VENDOR));

    app.world_mut().write_message(edge(
        vendor,
        VENDOR,
        ObjectType::Unit,
        FIELD_UNIT_HEALTH,
        50,
        0,
    ));
    app.update();
    assert_eq!(seen_in(&app), ["MERCHANT_CLOSED"]);
    assert_eq!(latch(&app), None);
    assert_eq!(vocals(&mut app), [], "dl = 1: the unit says no goodbye");
}

/// A trade partner is a player: its death releases the latch through `0x605950` as any latched
/// unit's does.
#[test]
fn a_latched_players_death_closes_the_window_too() {
    let (mut app, _) = app();
    let partner = app.world_mut().spawn_empty().id();
    open(&mut app, NpcWindow::Trade, OTHER);
    vocals(&mut app);
    app.world_mut().write_message(edge(
        partner,
        OTHER,
        ObjectType::Player,
        FIELD_UNIT_HEALTH,
        50,
        0,
    ));
    app.update();
    assert_eq!(seen_in(&app), ["TRADE_CLOSED"]);
    assert_eq!(latch(&app), None);
    assert_eq!(vocals(&mut app), []);
}

/// The shipped `SpellShapeshiftForm.dbc` through the gate: every druid form that is not a stance
/// and Ghost Wolf close the window; the warrior stances, Stealth, Moonkin (all stances) and
/// Shadowform (stance, and flag 0x8) do not.
#[test]
fn the_shipped_forms_split_into_those_that_close_the_window_and_those_that_do_not() {
    let Some(spells) = crate::ui_action::real_spells() else {
        return;
    };
    let closes = |id: u8| form_bars_interaction(&spells.forms, id);
    for (id, name) in [
        (1, "Cat"),
        (2, "Tree"),
        (3, "Travel"),
        (4, "Aquatic"),
        (5, "Bear"),
        (8, "Dire Bear"),
        (16, "Ghost Wolf"),
    ] {
        assert!(closes(id), "{name} form {id}");
    }
    for (id, name) in [
        (0, "no form"),
        (17, "Battle Stance"),
        (18, "Defensive Stance"),
        (19, "Berserker Stance"),
        (28, "Shadowform"),
        (30, "Stealth"),
        (31, "Moonkin"),
    ] {
        assert!(!closes(id), "{name} form {id}");
    }
}

/// The health watcher compares signed (`0x604774 jg`, `0x604778 jle`): a rise out of a negative
/// health to 0 is no death, and a fall to a negative one is.
#[test]
fn the_death_edge_reads_health_signed() {
    let (mut app, me) = app();
    open(&mut app, NpcWindow::Merchant, VENDOR);
    vocals(&mut app);
    app.world_mut().write_message(edge(
        me,
        ME,
        ObjectType::Player,
        FIELD_UNIT_HEALTH,
        0x8000_0000,
        0,
    ));
    app.update();
    assert_eq!(latch(&app), Some(VENDOR), "the old health was not above 0");

    app.world_mut().write_message(edge(
        me,
        ME,
        ObjectType::Player,
        FIELD_UNIT_HEALTH,
        20,
        u32::MAX,
    ));
    app.update();
    assert_eq!(latch(&app), None, "a fall to -1");
}
