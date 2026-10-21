//! The unit events that release the interaction latch (`0x4931c0`), off the field edges the apply
//! pass emits, as the reference's mirror handlers see them.
//!
//! A unit whose health falls from above 0 to 0 or below, compared signed
//! ([`FieldChanged::unit_died`]), runs `CGUnit_C::OnDied` (`0x605860`, from the health watcher
//! `0x6046f0` at `0x6047a3`). For a player it calls `0x5e99b0` first, which for the active player
//! releases the latch with the goodbye (`0x5e9a51`, `dl = 0`); then, for any unit, it releases the
//! latch if it names the dying unit, without a goodbye (`0x605950`, `dl = 1`). A change of the
//! active player's shapeshift form (`UNIT_FIELD_BYTES_1` byte 2, watcher `0x5de010`) into a form
//! that is not a stance (`0x60e9f0`) and lacks flag `0x8` releases it with the goodbye
//! (`0x5de07f`, `dl = 0`). The release closes the window through `0x493310`; the goodbye is
//! [`super::LatchVocal`], which a dead player does not hear (`0x605f30`).

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::ShapeshiftForm;
use benilla_protocol::field::FIELD_UNIT_BYTES_1;
use benilla_protocol::messages::ObjectType;

use super::latch::{release_cell, release_latch};
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

/// The death of a unit and the active player's form change release the latch. The edges are read
/// after the apply pass, and the releases run in order at the next sync point, before the `"npc"`
/// feed.
pub(super) fn release_on_field_edges(
    mut edges: MessageReader<FieldChanged>,
    me: Query<Entity, With<SelfPlayer>>,
    spells: Option<Res<Spells>>,
    mut commands: Commands,
) {
    let me = me.single().ok();
    for e in edges.read() {
        if e.unit_died() {
            // `0x605860` → `0x5e99b0` for the active player, then its own guid for any unit.
            if e.kind == ObjectType::Player && Some(e.entity) == me {
                commands.queue(|world: &mut World| release_latch(world, true));
            }
            let guid = e.guid;
            commands.queue(move |world: &mut World| release_cell(world, guid, false));
        } else if e.unit_field(FIELD_UNIT_BYTES_1)
            && Some(e.entity) == me
            && form_of(e.old) != form_of(e.new)
            && spells
                .as_ref()
                .is_some_and(|s| form_bars_interaction(&s.forms, form_of(e.new)))
        {
            commands.queue(|world: &mut World| release_latch(world, true));
        }
    }
}
