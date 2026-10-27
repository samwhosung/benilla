//! The latch released by a shapeshift into a form that cannot interact, and the windows a lost
//! service flag closes. A death's releases are its handler's (`crate::target`'s `died` tests).

use bevy::prelude::*;

use benilla_formats::ShapeshiftForm;
use benilla_protocol::field::FIELD_UNIT_BYTES_1;
use benilla_protocol::messages::ObjectType;
use benilla_protocol::SessionEvent;

use super::latch_tests::{latch, open, vocals, BANKER, VENDOR};
use super::release::form_bars_interaction;
use super::{LatchVocal, NpcWindow};
use crate::net::{FieldChanged, SelfGuid};
use crate::ui_action::Spells;
use crate::ui_script::sweep_test::seen_in;

const ME: u64 = 0x0000_0000_0000_0001;
/// Another player.
const OTHER: u64 = 0x0000_0000_0000_0002;

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

/// The whole client headless, a VM spying on the windows' closes, the active player and
/// `SpellShapeshiftForm` rows: 1 and 5 plain forms, 7 a stance (flag 0x1), 8 a form that may still
/// interact (flag 0x8).
fn app() -> (App, Entity) {
    seat(crate::game_plugins::schedule_tests::headless_client_unrun())
}

/// [`app`]'s seat on `app`.
fn seat(mut app: App) -> (App, Entity) {
    let (tx, rx) = crossbeam_channel::unbounded();
    // Kept alive for the run: a dropped receiver fails the sends.
    std::mem::forget(rx);
    let mut spells = Spells::empty_for_tests();
    spells.forms.insert(1, form(0));
    spells.forms.insert(5, form(0x2));
    spells.forms.insert(7, form(0x1 | 0x2));
    spells.forms.insert(8, form(0x8));
    app.insert_resource(crate::net::NetCommands(tx))
        .insert_resource(spells)
        .insert_resource(SelfGuid(Some(ME)))
        .insert_non_send_resource(crate::ui_script::sweep_test::spied_vm(&[
            "BANKFRAME_CLOSED",
            "MERCHANT_CLOSED",
            "GOSSIP_CLOSED",
            "QUEST_FINISHED",
            "TAXIMAP_CLOSED",
            "TRAINER_CLOSED",
        ]));
    let me = app.world_mut().spawn_empty().id();
    (app, me)
}

/// The update packet's end on the edges written, which runs their callbacks, then the frame's
/// lanes.
fn packet_end(app: &mut App) {
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, vec![SessionEvent::ObjectUpdateEnd]);
    crate::lanes::fire_in_order(world);
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

fn shift(app: &mut App, entity: Entity, guid: u64, old: u8, new: u8) {
    app.world_mut().write_message(edge(
        entity,
        guid,
        ObjectType::Player,
        FIELD_UNIT_BYTES_1,
        u32::from(old) << 16 | 0x0102,
        u32::from(new) << 16 | 0x0102,
    ));
    packet_end(app);
}

/// `0x5de07f`: a form without flag 0x8 closes the window with the goodbye; none, a stance, a form
/// that may interact, an unknown form, another unit's change and a move of another byte do not.
#[test]
fn a_shapeshift_into_a_form_that_cannot_interact_closes_the_window() {
    let (mut app, me) = app();
    let other = app.world_mut().spawn_empty().id();
    open(&mut app, NpcWindow::Bank, BANKER);
    vocals(&mut app);

    shift(&mut app, me, ME, 1, 0);
    shift(&mut app, me, ME, 0, 7);
    shift(&mut app, me, ME, 0, 8);
    shift(&mut app, me, ME, 0, 99);
    shift(&mut app, other, OTHER, 0, 1);
    // The form byte unchanged, the byte beside it moved.
    app.world_mut().write_message(edge(
        me,
        ME,
        ObjectType::Player,
        FIELD_UNIT_BYTES_1,
        1 << 16,
        1 << 16 | 0x0100,
    ));
    packet_end(&mut app);
    assert_eq!(latch(&app), Some(BANKER));
    assert!(seen_in(&app).is_empty());

    shift(&mut app, me, ME, 0, 1);
    assert_eq!(seen_in(&app), ["BANKFRAME_CLOSED"]);
    assert_eq!(latch(&app), None);
    assert_eq!(vocals(&mut app), [LatchVocal::Goodbye(BANKER)]);

    // A form on a plain, non-stance row with the interact flag clear, from another form.
    open(&mut app, NpcWindow::Merchant, VENDOR);
    shift(&mut app, me, ME, 1, 5);
    assert_eq!(latch(&app), None);
}

/// `0x5de010` releases the latch after its own signals (`0x5de011`-`0x5de01b`, then `0x5de07f`),
/// inside the values packet that moved the form byte: the bank's close fires behind
/// `UPDATE_BONUS_ACTIONBAR`, whose notice the notify pass wrote after the edge, and ahead of the
/// next packet's change into a stance, which signals again and closes nothing. The frame runs
/// between the dispatch and the drain, as the client's does.
#[test]
fn a_form_change_closes_the_window_after_its_own_signals() {
    let (mut app, _) = seat(crate::game_plugins::schedule_tests::headless_client());
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ))
    .insert_resource(State::new(crate::client_state::ClientState::InWorld));
    let world = app.world_mut();
    world.insert_non_send_resource(crate::ui_script::sweep_test::spied_vm(&[
        "UPDATE_BONUS_ACTIONBAR",
        "BANKFRAME_CLOSED",
    ]));
    let me = world
        .spawn((
            crate::net::Guid(ME),
            crate::net::SelfPlayer,
            crate::net::ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(&[(2, 0x19), (FIELD_UNIT_BYTES_1, 0)])
                    .into_created(ObjectType::Player),
            ),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    open(&mut app, NpcWindow::Bank, BANKER);
    // A frame that settles the open and the VM's first feeds.
    app.world_mut().run_schedule(Update);
    let settled = seen_in(&app).len();
    let form = |form: u32| SessionEvent::ObjectValues {
        guid: ME,
        fields: benilla_protocol::ObjectFields::from_pairs(&[(FIELD_UNIT_BYTES_1, form << 16)]),
    };
    crate::net::handlers::dispatch(app.world_mut(), crate::net::wire([form(1), form(7)]));
    app.world_mut().run_schedule(Update);
    assert_eq!(
        seen_in(&app)[settled..],
        [
            "UPDATE_BONUS_ACTIONBAR",
            "BANKFRAME_CLOSED",
            "UPDATE_BONUS_ACTIONBAR"
        ]
    );
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
            packet_end(app);
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
    packet_end(&mut app);
    assert!(seen_in(&app).is_empty());
    assert_eq!(latch(&app), Some(VENDOR));
}
