//! The latch released by a shapeshift into a form that cannot interact, and the windows a lost
//! service flag closes. A death's releases are its handler's (`crate::target`'s `died` tests).

use bevy::prelude::*;

use benilla_formats::ShapeshiftForm;
use benilla_protocol::field::FIELD_UNIT_BYTES_1;
use benilla_protocol::messages::ObjectType;

use super::latch_tests::{harness, latch, open, vocals, BANKER, VENDOR};
use super::release::{form_bars_interaction, release_on_field_edges};
use super::{LatchVocal, NpcWindow};
use crate::net::{FieldChanged, SelfPlayer};
use crate::ui_action::Spells;
use crate::ui_script::sweep_test::seen_in;

const ME: u64 = 0x0000_0000_0000_0001;

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
        at: crate::lanes::stamp(),
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

/// The `UNIT_NPC_FLAGS` watcher (`0x604a20` → `0x60b420`) closes the window whose NPC lost the
/// flag for it, testing each bit against its window's cell (`0x60b45c`-`0x60b552`): gossip `0x1`
/// (`0x4e1fa0`), quest `0x2` (`0x501130`), taxi `0x8` (`0x4dc0f0`), trainer `0x10` (`0x4d7440`)
/// and bank `0x100` (`0x4f84d0`). Each core is entered directly while its window holds the
/// latch, so it fires its close twice, quest once. A flag kept or gained, another unit's loss and
/// a merchant's lost vendor bit close nothing.
#[test]
fn an_npc_that_loses_a_service_flag_closes_its_window() {
    use crate::target::cursor_mode::npc_flags as f;
    use benilla_protocol::field::FIELD_UNIT_NPC_FLAGS;
    const KEEP: u32 = f::VENDOR | f::INNKEEPER;
    for (window, bit, event, times) in [
        (NpcWindow::Gossip, f::GOSSIP, "GOSSIP_CLOSED", 2),
        (NpcWindow::Quest, f::QUESTGIVER, "QUEST_FINISHED", 1),
        (NpcWindow::Taxi, f::FLIGHTMASTER, "TAXIMAP_CLOSED", 2),
        (NpcWindow::Trainer, f::TRAINER, "TRAINER_CLOSED", 2),
        (NpcWindow::Bank, f::BANKER, "BANKFRAME_CLOSED", 2),
    ] {
        let (mut app, _) = app();
        let npc = app.world_mut().spawn_empty().id();
        let other = app.world_mut().spawn_empty().id();
        open(&mut app, window, VENDOR);
        let flags = |app: &mut App, entity, guid, old, new| {
            app.world_mut().write_message(edge(
                entity,
                guid,
                ObjectType::Unit,
                FIELD_UNIT_NPC_FLAGS,
                old,
                new,
            ));
            app.update();
        };
        flags(&mut app, other, BANKER, bit | KEEP, KEEP);
        flags(&mut app, npc, VENDOR, KEEP, bit | KEEP);
        flags(&mut app, npc, VENDOR, bit | KEEP, bit);
        assert!(seen_in(&app).is_empty(), "{window:?}: {:?}", seen_in(&app));
        assert_eq!(latch(&app), Some(VENDOR), "{window:?}");

        flags(&mut app, npc, VENDOR, bit | KEEP, KEEP);
        assert_eq!(seen_in(&app), vec![event; times], "{window:?}");
        assert_eq!(latch(&app), None, "{window:?}");
    }

    // The watcher has no vendor arm.
    let (mut app, _) = app();
    let npc = app.world_mut().spawn_empty().id();
    open(&mut app, NpcWindow::Merchant, VENDOR);
    app.world_mut().write_message(edge(
        npc,
        VENDOR,
        ObjectType::Unit,
        FIELD_UNIT_NPC_FLAGS,
        u32::MAX,
        0,
    ));
    app.update();
    assert!(seen_in(&app).is_empty());
    assert_eq!(latch(&app), Some(VENDOR));
}
