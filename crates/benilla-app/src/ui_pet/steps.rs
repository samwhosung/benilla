//! The engine half of the pet verbs the host applies inside their calls ([`PetSteps`]): the
//! autocast toggles, the drag's bar writes, the stop, the abandon and the rename. vmangos answers
//! no toggle (`PetHandler.cpp:203-300`), so each applies its own state here and sends.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::messages::PetActionEntry;
use benilla_ui::script::{PetCall, UiScript};

use crate::net::{ClientCommand, NetCommands};

use super::drain::{stop_pet_attack, toggle_slot_autocast};
use super::menu::UNIT_FLAG_PET_RENAME;
use super::{PetAttackLatch, PetBar, PetUnit};

/// The rename sender's silent truncation, `0x64a7f0(dst, 0x50, "%s", name)` at `0x4bd8ee`: 79
/// plus the NUL. The popup's 12-letter cap is FrameXML's, a layer above.
const PET_NAME_MAX: usize = 79;

/// What the pet verbs read and write: the bar, the pet's descriptor, the open stable's master and
/// the red line.
#[derive(SystemParam)]
pub(crate) struct PetSteps<'w, 's> {
    bar: ResMut<'w, PetBar>,
    latch: ResMut<'w, PetAttackLatch>,
    commands: Res<'w, NetCommands>,
    pet: PetUnit<'w, 's>,
    /// Absent in a world with no stable window.
    stable: Option<Res<'w, crate::ui_stable::StableOpen>>,
    sink: crate::ui_action::MessageSink<'w>,
}

impl PetSteps<'_, '_> {
    /// One pet verb inside its call, under [`PetBar`]'s pet guid, never one from the VM.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: PetCall) {
        let pet_guid = self.bar.spells.pet_guid;
        match call {
            // `0x4bcbb0` asks the usability predicate first (`0x4bcbcf`), then bit 31 (`0x4bcbf1`).
            PetCall::ToggleAutocast(slot) => {
                if !self.pet.actions_usable(&self.bar) {
                    debug!("ui_pet: autocast toggle on slot {slot} refused, not usable");
                    return;
                }
                let Some(flipped) = toggle_slot_autocast(&mut self.bar, slot) else {
                    return;
                };
                debug!(
                    "ui_pet: autocast {} for spell {} (slot {slot})",
                    flipped.autocast_on(),
                    flipped.action()
                );
                // Lua slots are 1-based, the wire's 0-based; the toggle refused 0.
                self.send(ClientCommand::PetSetAction {
                    pet_guid,
                    entries: vec![(slot - 1, flipped.packed)],
                });
            }
            // The drag's write, gated where the VM applied it (`0x4bc9d0`), mirrored here and sent
            // as one batch.
            PetCall::SetActions(entries) => {
                for &(position, packed) in &entries {
                    if let Some(e) = self.bar.spells.bar.get_mut(position as usize) {
                        *e = PetActionEntry::from(packed);
                    }
                }
                debug!("ui_pet: bar write {entries:?}");
                self.send(ClientCommand::PetSetAction { pet_guid, entries });
            }
            // `0x4bd650`: with the latch up, the send, the latch down and `PET_BAR_UPDATE`
            // (`0x4bd662`-`0x4bd6b4`); with it down, nothing.
            PetCall::StopAttack => {
                if stop_pet_attack(&mut self.latch, &self.commands) {
                    debug!("ui_pet: stop attack");
                    script.fire_event("PET_BAR_UPDATE", vec![]);
                }
            }
            // `0x4bd740` past its gate: `CMSG_PET_ABANDON` with the pet's guid (`0x4bd765`), then,
            // with a stable master held, the stable list asked again (`0x4bd7a4`-`0x4bd7b2`).
            PetCall::Abandon => {
                debug!("ui_pet: abandoning pet {pet_guid:#x}");
                self.send(ClientCommand::PetAbandon { pet_guid });
                if let Some(npc) = self.stable.as_ref().and_then(|s| s.npc) {
                    self.send(ClientCommand::ListStabledPets { npc });
                }
            }
            PetCall::Rename(name) => self.rename(script, pet_guid, name),
            // `0x4bccb0` asks the usability predicate first (`0x4bccce`), then the word's bit 31
            // (`0x4bccf5`).
            PetCall::SpellAutocast(spell_id) => {
                if !self.pet.actions_usable(&self.bar) {
                    debug!("ui_pet: autocast {spell_id} refused, the pet's actions are not usable");
                    return;
                }
                let Some(on) = crate::ui_pet_book::flip_autocast(&mut self.bar.spells, spell_id)
                else {
                    return;
                };
                debug!("ui_pet: book autocast {spell_id} -> {on}");
                self.send(ClientCommand::PetSpellAutocast {
                    pet_guid,
                    spell_id,
                    enabled: on,
                });
            }
        }
    }

    /// `0x4bd840`: each gate raises its line and sends nothing: no pet object (`0x4bd872`,
    /// `ERR_NO_PET`), a pet not summoned by us (`0x4bd89f`, `ERR_NOTYOURPET`), one that cannot be
    /// renamed (`0x4bd8b5`, `ERR_PET_NOT_RENAMEABLE`), and no name or an empty one (`0x4bd8d2`,
    /// `ERR_NULL_PETNAME`). Then the name, cut to 79 bytes (`0x4bd8ee`), goes out (`0x4bd8fe`).
    fn rename(&mut self, script: &mut UiScript, pet_guid: u64, name: Option<String>) {
        let refusal = match self.pet.store(pet_guid) {
            None => Some("ERR_NO_PET"),
            Some(store) if store.0.unit_summoned_by() != self.pet.self_guid.0 => {
                Some("ERR_NOTYOURPET")
            }
            Some(store) if store.0.unit_flags() & UNIT_FLAG_PET_RENAME == 0 => {
                Some("ERR_PET_NOT_RENAMEABLE")
            }
            Some(_) if name.as_deref().is_none_or(str::is_empty) => Some("ERR_NULL_PETNAME"),
            Some(_) => None,
        };
        if let Some(key) = refusal {
            debug!("ui_pet: rename refused ({key})");
            if let Some(line) = crate::ui_action::keyed_line(script, key) {
                crate::ui_action::show_now(script, &mut self.sink, "ui_pet", line);
            }
            return;
        }
        let name = clamp_pet_name(name.as_deref().unwrap_or_default());
        debug!("ui_pet: renaming pet {pet_guid:#x} to {name:?}");
        self.send(ClientCommand::PetRename { pet_guid, name });
    }

    /// Whether the attack latch is up (`[0xb714b0]`).
    pub(crate) fn attack_latched(&self) -> bool {
        self.latch.up()
    }

    fn send(&self, command: ClientCommand) {
        let _ = self.commands.0.send(command);
    }
}

/// [`PET_NAME_MAX`]'s truncation. Deviation: it counts characters where the reference counts
/// bytes, because a byte cut can split a UTF-8 character.
fn clamp_pet_name(name: &str) -> String {
    name.chars().take(PET_NAME_MAX).collect()
}
