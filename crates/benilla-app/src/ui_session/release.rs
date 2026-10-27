//! The unit events that release the interaction latch (`0x4931c0`), off the field edges the update
//! packet's notify pass writes, run inside the packet as the reference's watchers run there
//! ([`crate::field_callbacks`]), their signals parked at their places.
//!
//! A change of the active player's shapeshift form (`UNIT_FIELD_BYTES_1` byte 2, watcher
//! `0x5de010`) into a form that is not a stance (`0x60e9f0`) and lacks flag `0x8` releases it with
//! the goodbye (`0x5de07f`, `dl = 0`). The release closes the window through `0x493310`; the
//! goodbye is [`super::LatchVocal`], which a dead player does not hear (`0x605f30`). A death's
//! releases (`0x5e9a51`, `0x605950`) run inside its handler, in its order (`crate::target`'s
//! `died`).
//!
//! A unit's `UNIT_NPC_FLAGS` edge runs `0x60b420` (from the watcher `0x604a20`), which closes the
//! window a lost service flag served ([`close_lost_services`]).

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::ShapeshiftForm;
use benilla_protocol::field::{FIELD_UNIT_BYTES_1, FIELD_UNIT_NPC_FLAGS};

use super::latch::release_latch;
use crate::field_callbacks::Parked;
use crate::field_watch::{FieldNotice, NoticeLane};
use crate::net::{FieldChanged, SelfGuid};
use crate::ui_action::Spells;

/// `UNIT_FIELD_BYTES_1` byte 2, the shapeshift form (`[+0x110]+0x212`).
fn form_of(bytes_1: u32) -> u8 {
    (bytes_1 >> 16) as u8
}

/// Whether `form` bars the player from NPCs: a form other than none whose row is not a stance
/// (`IsShapeShifted`, `0x60e9f0`) and lacks the interact flag (`0x5f017f`, `0x5de072`).
/// `InteractWithNpc` (`0x5f0130`) refuses a talk with it, and taking it closes the open window
/// (`0x5de010`). A form with no row reads nothing here, where the reference faults.
pub(crate) fn form_bars_interaction(forms: &HashMap<u32, ShapeshiftForm>, form: u8) -> bool {
    form != 0
        && forms
            .get(&u32::from(form))
            .is_some_and(|row| !row.is_stance() && !row.allows_interaction())
}

/// `0x60b420(old)`: each service flag the unit lost (`0x60b44e`, set before and clear now) closes
/// the window whose cell holds the unit, in its order (`0x60b45c`-`0x60b552`): gossip `0x1`
/// (`0x4e1fa0`), quest `0x2` (`0x501130` with its report clear), taxi `0x8` (`0x4dc0f0`), trainer
/// `0x10` (`0x4d7440`) and bank `0x100` (`0x4f84d0`), each core entered as it is with a zero guid.
fn close_lost_services(world: &mut World, guid: u64, old: u32, new: u32) {
    use crate::target::cursor_mode::npc_flags as f;
    let lost = (old ^ new) & !new;
    let holds = |world: &World, cell: fn(&World) -> Option<u64>| cell(world) == Some(guid);
    if lost & f::GOSSIP != 0
        && holds(world, |w| {
            w.get_resource::<crate::ui_gossip::GossipState>()?.npc
        })
    {
        crate::ui_gossip::close_core(world);
    }
    if lost & f::QUESTGIVER != 0
        && holds(world, |w| {
            w.get_resource::<crate::ui_quest::QuestGiver>()?.npc
        })
    {
        crate::ui_quest::close_core(world, false);
    }
    if lost & f::FLIGHTMASTER != 0
        && holds(world, |w| {
            Some(
                w.get_resource::<crate::ui_taxi::TaxiState>()?
                    .open
                    .as_ref()?
                    .flightmaster,
            )
        })
    {
        crate::ui_taxi::close_core(world);
    }
    if lost & f::TRAINER != 0
        && holds(world, |w| {
            w.get_resource::<crate::ui_trainer::TrainerOpen>()?.trainer
        })
    {
        crate::ui_trainer::close_core(world);
    }
    if lost & f::BANKER != 0
        && holds(world, |w| {
            w.get_resource::<crate::ui_bank::BankOpen>()?.banker
        })
    {
        crate::ui_bank::close_core(world);
    }
}

/// The callbacks of one field edge: the active player's form change releases the latch, and a
/// unit's lost service flags close their windows.
pub(crate) fn on_field_edge(world: &mut World, e: FieldChanged) {
    if e.unit_field(FIELD_UNIT_BYTES_1) {
        form_changed(world, e);
    } else if e.unit_field(FIELD_UNIT_NPC_FLAGS) && (e.old ^ e.new) & !e.new != 0 {
        // `0x604a20` → `0x60b420(old)` at the edge's own place, a type watcher's.
        let (guid, old, new) = (e.guid, e.old, e.new);
        crate::lanes::firing(e.at, || {
            Parked::new(world).leg(world, |world| close_lost_services(world, guid, old, new));
        });
    }
}

/// `OnShapeshiftFormChanged` (`0x5de010`), our own watch on our form byte: past its usable
/// refresh and its state signals (`0x5de011`-`0x5de01b`, the block's [`FieldNotice::Shapeshift`],
/// which the notify pass wrote after the edge), a form that bars interaction releases the latch
/// with the goodbye (`0x5de07f`, `dl = 0`).
fn form_changed(world: &mut World, e: FieldChanged) {
    if Some(e.guid) != world.get_resource::<SelfGuid>().and_then(|s| s.0)
        || form_of(e.old) == form_of(e.new)
        || !world
            .get_resource::<Spells>()
            .is_some_and(|s| form_bars_interaction(&s.forms, form_of(e.new)))
    {
        return;
    }
    let place = world
        .get_resource::<NoticeLane>()
        .and_then(|lane| lane.first_after(e.at, |n| *n == FieldNotice::Shapeshift))
        .unwrap_or(e.at);
    crate::lanes::firing(place, || {
        Parked::new(world).leg(world, |world| release_latch(world, true));
    });
}
