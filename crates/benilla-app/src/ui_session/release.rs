//! The unit events that release the interaction latch (`0x4931c0`), off the field edges the apply
//! pass emits, as the reference's mirror handlers see them.
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
use crate::net::{FieldChanged, SelfPlayer};
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

/// The active player's form change releases the latch, and a unit's lost service flags close
/// their windows. The edges are read after the apply pass, and the releases run in order at the
/// next sync point, before the `"npc"` feed.
pub(super) fn release_on_field_edges(
    mut edges: MessageReader<FieldChanged>,
    me: Query<Entity, With<SelfPlayer>>,
    spells: Option<Res<Spells>>,
    mut commands: Commands,
) {
    let me = me.single().ok();
    for e in edges.read() {
        if e.unit_field(FIELD_UNIT_BYTES_1)
            && Some(e.entity) == me
            && form_of(e.old) != form_of(e.new)
            && spells
                .as_ref()
                .is_some_and(|s| form_bars_interaction(&s.forms, form_of(e.new)))
        {
            commands.queue(|world: &mut World| release_latch(world, true));
        } else if e.unit_field(FIELD_UNIT_NPC_FLAGS) {
            let (guid, old, new) = (e.guid, e.old, e.new);
            commands.queue(move |world: &mut World| close_lost_services(world, guid, old, new));
        }
    }
}
