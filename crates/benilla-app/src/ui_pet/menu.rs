//! The pet frame's right-click menu. `UnitPopup.lua:402-416` shows the paper doll and Abandon when
//! `PetCanBeAbandoned()`, Rename when `PetCanBeRenamed()` too, and Dismiss otherwise. `PetAbandon`
//! (`0x4be4c0`, `0x4bd740`) sends `CMSG_PET_ABANDON`; `PetDismiss` (`0x4be4d0`) sends no packet
//! itself but hands the Dismiss command word to the bar's dispatcher (`0x4bd1d0`), so it leaves as
//! `CMSG_PET_ACTION`. vmangos unsummons a summon on either, so sending the wrong one goes unseen.

use bevy::prelude::*;

use benilla_protocol::messages::{PET_ACT_COMMAND, PET_COMMAND_DISMISS};
use benilla_ui::script::UiScript;

use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::{UiError, UiErrorKeys};

use super::{PetBar, PetUnit};

/// `UNIT_FLAG_PET_RENAME`, `0x4be5c4`'s mask: set on a tamed pet not yet renamed. No client code
/// writes it; the server clears it after a rename (the sender re-checks it at `0x4bd8b5`).
const UNIT_FLAG_PET_RENAME: u32 = 0x0000_0010;
/// `UNIT_FLAG_PET_ABANDON`, `0x4be544`'s mask: set on a hunter pet, clear on a summon.
const UNIT_FLAG_PET_ABANDON: u32 = 0x0000_0020;

/// The rename sender's silent truncation, `0x64a7f0(dst, 0x50, "%s", name)` at `0x4bd840`: 79
/// plus the NUL. The popup's 12-letter cap is FrameXML's, a layer above.
const PET_NAME_MAX: usize = 79;

/// `(PetCanBeAbandoned, PetCanBeRenamed)` (`0x4be500`, `0x4be580`), read off `UNIT_FIELD_FLAGS`
/// (dword 46), not the TBC's `UNIT_FIELD_BYTES_2`, which this server leaves zero.
pub(super) fn menu_predicates(pet_flags: u32) -> (bool, bool) {
    (
        pet_flags & UNIT_FLAG_PET_ABANDON != 0,
        pet_flags & UNIT_FLAG_PET_RENAME != 0,
    )
}

/// Push the menu's predicates. Their owner test is `UNIT_FIELD_SUMMONEDBY` alone, with no
/// `CHARMEDBY` leg (`0x4be500`), so a charmed unit gets every row off, and `UnitPopup_ShowMenu`
/// opens no menu holding only Cancel.
pub(super) fn feed_pet_menu(script: Option<NonSendMut<UiScript>>, bar: Res<PetBar>, pet: PetUnit) {
    let Some(mut script) = script else {
        return;
    };
    let flags = (bar.spells.pet_guid != 0)
        .then(|| pet.store(bar.spells.pet_guid))
        .flatten()
        .filter(|store| store.0.unit_summoned_by() == pet.self_guid.0)
        .map_or(0, |store| store.0.unit_flags());
    let (can_be_abandoned, can_be_renamed) = menu_predicates(flags);
    script.set_pet_menu(can_be_abandoned, can_be_renamed);
}

/// Send the menu's verbs under [`PetBar`]'s pet guid. Nothing is applied locally: the server
/// answers by removing the pet or moving its name timestamp.
pub(super) fn drain_pet_menu(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    commands: Res<NetCommands>,
    mut ui_errors: ResMut<UiErrorKeys>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (abandons, dismisses) = script.take_pet_gives_up();
    let renames = script.take_pet_renames();
    if abandons == 0 && dismisses == 0 && renames.is_empty() {
        return;
    }
    let pet_guid = bar.spells.pet_guid;
    if pet_guid == 0 {
        debug!("ui_pet: dropping a queued menu verb — there is no pet");
        return;
    }

    // Counted, not deduped: the reference sends one per press.
    for _ in 0..abandons {
        debug!("ui_pet: abandoning pet {pet_guid:#x}");
        let _ = commands.0.send(ClientCommand::PetAbandon { pet_guid });
    }
    // `0x4bd6e0` stages `0x07000003` at target 0.
    let dismiss_word = PET_COMMAND_DISMISS | (u32::from(PET_ACT_COMMAND) << 24);
    for _ in 0..dismisses {
        debug!("ui_pet: dismissing pet {pet_guid:#x} ({dismiss_word:#010x})");
        let _ = commands.0.send(ClientCommand::PetAction {
            pet_guid,
            packed: dismiss_word,
            target_guid: 0,
        });
    }
    for name in renames {
        // `0x4bd840`: an empty name raises an error and sends nothing; the rest is truncated.
        if name.is_empty() {
            ui_errors.0.push(UiError::key("ERR_NULL_PETNAME"));
            continue;
        }
        let name = clamp_pet_name(&name);
        debug!("ui_pet: renaming pet {pet_guid:#x} to {name:?}");
        let _ = commands.0.send(ClientCommand::PetRename { pet_guid, name });
    }
}

/// [`PET_NAME_MAX`]'s truncation. Deviation: it counts characters where the reference counts
/// bytes, because a byte cut can split a UTF-8 character.
fn clamp_pet_name(name: &str) -> String {
    name.chars().take(PET_NAME_MAX).collect()
}
