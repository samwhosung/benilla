//! The bar's click intents onto the wire, and the state a press latches locally: vmangos answers
//! no command or reaction press (`PetHandler.cpp:81-99`) and no autocast toggle
//! (`PetHandler.cpp:203-300`), and sends `SMSG_PET_MODE` only from `Pet::SetEnabled`
//! (`Pet.cpp:2362-2377`), so the client applies them itself until the next `SMSG_PET_SPELLS`.

use bevy::prelude::*;

use benilla_protocol::messages::{
    PetActionEntry, PET_COMMAND_ATTACK, PET_COMMAND_FOLLOW, PET_COMMAND_STAY,
};
use benilla_ui::script::UiScript;

use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::target::Selection;
use crate::ui_action::Spells;

use super::bar::active_aura_press;
use super::{PetBar, PetUnit};

/// `UNIT_FLAG_POSSESSED`, `UNIT_FIELD_FLAGS` bit 24 (vmangos `UnitDefines.h:569`), which the
/// reference reads as `[[pet+0x110]+0xA3] & 1`.
pub(super) const UNIT_FLAG_POSSESSED: u32 = 0x0100_0000;

/// Send the bar's intents under [`PetBar`]'s pet guid, never one from the VM, so an intent queued
/// as the pet leaves dies here. `CMSG_PET_ACTION` echoes the slot's word, which the server
/// dispatches on its type, with our selection as the target; `HandlePetAction` drops a target
/// the spell does not want.
pub(super) fn drain_pet_actions(
    script: Option<NonSendMut<UiScript>>,
    mut bar: ResMut<PetBar>,
    mut selection: ResMut<Selection>,
    commands: Res<NetCommands>,
    pet: PetUnit,
    spells: Option<Res<Spells>>,
    mut ui_errors: ResMut<crate::ui_action::UiErrorKeys>,
    scan: crate::target::TargetScan,
    mut seam: crate::creature_anim::AttackSeam,
) {
    let Some(mut script) = script else {
        return;
    };
    let pressed = script.take_pet_actions();
    let orders = script.take_pet_orders();
    let toggles = script.take_pet_autocast_toggles();
    let stops = script.take_pet_stop_attacks();
    let writes = script.take_pet_set_actions();
    if pressed.is_empty()
        && orders.is_empty()
        && toggles.is_empty()
        && stops == 0
        && writes.is_empty()
    {
        return;
    }
    let pet_guid = bar.spells.pet_guid;
    if pet_guid == 0 {
        debug!("ui_pet: dropping queued pet intents — the bar is gone");
        return;
    }
    let pet_store = pet.store(pet_guid);
    let possessing = possessing(pet_store, pet.self_guid.0);

    // A slot press and a scripted order (`PetAttack` and the like) are one press: both reach the
    // reference's one dispatcher (`0x4bd1d0`). The slot number only labels logs.
    let presses: Vec<(u32, PetActionEntry)> = pressed
        .into_iter()
        .filter_map(|slot| slot_entry(&bar, slot).map(|e| (slot, e)))
        .chain(
            orders
                .into_iter()
                .map(|packed| (0, PetActionEntry::from(packed))),
        )
        .collect();
    for (slot, entry) in presses {
        // A press on a spell the pet is running cancels its aura and sends no `CMSG_PET_ACTION`
        // (`0x4bd240`-`0x4bd2ad`); nothing latches, the icon follows the pet's aura field.
        let display = entry
            .is_spell()
            .then(|| spells.as_ref().and_then(|s| s.catalog.get(entry.action())))
            .flatten();
        if let Some(spell_id) = active_aura_press(entry, pet_store, display) {
            debug!("ui_pet: slot {slot} cancels its own aura (spell {spell_id}) — no PetAction");
            let _ = commands
                .0
                .send(ClientCommand::PetCancelAura { pet_guid, spell_id });
            continue;
        }
        // Only Attack runs `0x612df0`, the attack validator with the pet as actor, whose target
        // pick can move the selection; every other press sends the selection as is (`0x4bd212`).
        let mut target_guid = selection.guid.unwrap_or(0);
        let refused = if is_attack_order(entry) {
            crate::ui_action::attack_actor_refusal(pet_store, pet.self_guid.0, &mut ui_errors)
                || match crate::target::attack_order_target(
                    &scan,
                    &mut selection,
                    &mut seam,
                    &mut ui_errors,
                ) {
                    Some(guid) => {
                        target_guid = guid;
                        false
                    }
                    None => true,
                }
        } else {
            false
        };
        if !commit_press(&mut bar, entry, refused, possessing) {
            debug!("ui_pet: slot {slot} refused by the attack validator — no packet");
            continue;
        }
        debug!(
            "ui_pet: press slot {slot} (action {} kind {:#04x}) at {target_guid:#x}",
            entry.action(),
            entry.kind()
        );
        let _ = commands.0.send(ClientCommand::PetAction {
            pet_guid,
            packed: entry.packed,
            target_guid,
        });
    }
    // The repaint is `latch_press`'s alone, not every click's: a refused `TogglePetAutocast`
    // (`0x4bcbf7`: any right-click on a token) signals nothing, so after `OnClick`'s
    // `SetChecked(0)` that button stays unlit until the next repaint, as in the reference.
    for slot in toggles {
        let Some(flipped) = toggle_slot_autocast(&mut bar, slot) else {
            continue;
        };
        debug!(
            "ui_pet: autocast {} for spell {} (slot {slot})",
            flipped.autocast_on(),
            flipped.action()
        );
        let _ = commands.0.send(ClientCommand::PetSetAction {
            pet_guid,
            // Lua slots are 1-based, the wire's 0-based; `slot_entry` already refused 0.
            entries: vec![(slot - 1, flipped.packed)],
        });
    }
    for _ in 0..stops {
        if stop_pet_attack(&mut bar, &commands) {
            debug!("ui_pet: stop attack");
        }
    }
    // The drag's `(0-based position, word)` pairs: mirrored here and sent as one batch, since the
    // server tells one pair from two by body size.
    for entries in writes {
        for &(position, packed) in &entries {
            if let Some(e) = bar.spells.bar.get_mut(position as usize) {
                *e = PetActionEntry::from(packed);
            }
        }
        debug!("ui_pet: bar write {entries:?}");
        let _ = commands
            .0
            .send(ClientCommand::PetSetAction { pet_guid, entries });
    }
}

/// `TogglePetAutocast`'s local half (`0x4bcbb0`): flip bit 30 in place and return the whole word
/// to send (`0x4bcbff`/`0x4bcc17`), or `None` for a slot out of range or without bit 31
/// (`0x4bcbf1`). `0x4bcc19` then calls `0x4bd190`, which copies a spell slot's word over the last
/// spellbook entry equal to it under `& 0x3FFFFFFF`.
pub(super) fn toggle_slot_autocast(bar: &mut PetBar, slot: u32) -> Option<PetActionEntry> {
    let entry = slot_entry(bar, slot).filter(|e| e.autocast_allowed())?;
    let flipped = entry.with_autocast(!entry.autocast_on());
    *slot_entry_mut(bar, slot)? = flipped;
    if flipped.kind() == 1 {
        let key = flipped.packed & 0x3FFF_FFFF;
        if let Some(book) = bar
            .spells
            .spells
            .iter_mut()
            .rev()
            .find(|w| w.packed & 0x3FFF_FFFF == key)
        {
            *book = flipped;
        }
    }
    Some(flipped)
}

/// `PetStopAttack`'s core (`0x4bd650`): send `CMSG_PET_STOP_ATTACK` and lower the latch, or do
/// nothing when it is already down (`0x4bd65e`). Returns whether it sent.
pub(super) fn stop_pet_attack(bar: &mut PetBar, commands: &NetCommands) -> bool {
    let pet_guid = bar.spells.pet_guid;
    if !bar.attacking || pet_guid == 0 {
        return false;
    }
    let _ = commands.0.send(ClientCommand::PetStopAttack { pet_guid });
    bar.attacking = false;
    true
}

/// `0x493910`'s entry gate: the old-target clear runs only when a selection existed (`0x493937`)
/// and is being replaced or dropped (`0x493949`/`0x493951`).
pub(super) fn old_target_cleared(previous: Option<u64>, now: Option<u64>) -> bool {
    previous.is_some() && previous != now
}

/// The old-target clear's `PetStopAttack` (`0x493910`, at `0x493a18` when `0x5ee5a0` finds a
/// possessed unit at `0x493a0f`). It sits before the notify branch, so every selection writer runs
/// it; a re-select runs no clear (`0x493540`).
pub(super) fn pet_stop_on_old_target_clear(
    selection: Res<Selection>,
    mut previous: Local<Option<u64>>,
    mut bar: ResMut<PetBar>,
    commands: Res<NetCommands>,
) {
    let now = selection.guid;
    if *previous == now {
        return;
    }
    let cleared = old_target_cleared(*previous, now);
    *previous = now;
    if cleared && stop_pet_attack(&mut bar, &commands) {
        debug!("ui_pet: the old-target clear called the pet off (0x493a18)");
    }
}

/// `0x5ee5a0`: the unit the player possesses, which must be the bar's unit for an Attack press to
/// latch (`0x4bd420`, `0x4bd42e`). It needs `UNIT_FLAG_POSSESSED` (`0x5ee626`) and charmed-by,
/// else created-by, us (`0x5ee62f`); vmangos sets that flag only with possession (Mind Control,
/// Eyes of the Beast), never for an ordinary pet. The reference also tests the active mover
/// (`0x5ee5bc`, `0x5ee5e9`), not modelled here: vmangos sets the mover with the flag.
pub(super) fn possessing(store: Option<&ObjectStore>, self_guid: Option<u64>) -> bool {
    store.is_some_and(|s| {
        s.0.unit_flags() & UNIT_FLAG_POSSESSED != 0
            && s.0.unit_owner(benilla_protocol::OwnerFallback::CreatedBy) == self_guid
    })
}

/// The Attack order, the one press validated before it sends: the type-7 arm branches only on
/// `action <= 1` and `action == 2`, so every other command sends as it is.
pub(super) fn is_attack_order(entry: PetActionEntry) -> bool {
    entry.kind() == benilla_protocol::messages::PET_ACT_COMMAND
        && entry.action() == PET_COMMAND_ATTACK
}

/// Apply a press and answer whether it is sent. A veto from the validator (`0x4bd40d`, the pet as
/// actor) jumps past the send (`0x4bd414 je 0x4bd4c6`): no packet, no latch, no repaint. Failing
/// the possession compare after it (`0x4bd420`) costs only the latch (`0x4bd427` to the send).
pub(super) fn commit_press(
    bar: &mut PetBar,
    entry: PetActionEntry,
    refused: bool,
    possessing: bool,
) -> bool {
    if refused {
        return false;
    }
    latch_press(bar, entry, possessing);
    true
}

/// Latch a press's state, the half the server never confirms, with the client's masks. A
/// reaction writes byte 0 (`0x4bc94c`). Stay and Follow, the only commands to reach the write
/// (`action <= 1` at `0x4bd3b1`), set bits 8-15 and keep byte 0 and bit 27, which only the server
/// writes (`0x4bc96f`). Attack raises a possessed unit's attack latch instead (`0x4bd42e`).
pub(super) fn latch_press(bar: &mut PetBar, entry: PetActionEntry, possessing: bool) {
    let action = entry.action();
    match entry.kind() {
        benilla_protocol::messages::PET_ACT_COMMAND
            if action == PET_COMMAND_STAY || action == PET_COMMAND_FOLLOW =>
        {
            // `0x4bc960` signals `PET_BAR_UPDATE` even when the command is unchanged.
            bar.spells.state = (bar.spells.state & 0x0800_00FF) | (action << 8);
            bar.bar_signals = bar.bar_signals.wrapping_add(1);
        }
        benilla_protocol::messages::PET_ACT_COMMAND
            if action == PET_COMMAND_ATTACK && possessing =>
        {
            // `0x4bd42e` signals too; an ordinary pet never gets here.
            bar.attacking = true;
            bar.bar_signals = bar.bar_signals.wrapping_add(1);
        }
        benilla_protocol::messages::PET_ACT_REACTION => {
            // `0x4bc940`: the same, on byte 0.
            bar.spells.state = (bar.spells.state & 0xFFFF_FF00) | action;
            bar.bar_signals = bar.bar_signals.wrapping_add(1);
        }
        // Dismiss, spells and unknown types reach the send (`0x4bd444`) without a signal.
        _ => {}
    }
}

/// The word held for a 1-based Lua slot.
pub(super) fn slot_entry(bar: &PetBar, slot: u32) -> Option<PetActionEntry> {
    let index = usize::try_from(slot.checked_sub(1)?).ok()?;
    bar.spells.bar.get(index).copied()
}

pub(super) fn slot_entry_mut(bar: &mut PetBar, slot: u32) -> Option<&mut PetActionEntry> {
    let index = usize::try_from(slot.checked_sub(1)?).ok()?;
    bar.spells.bar.get_mut(index)
}
