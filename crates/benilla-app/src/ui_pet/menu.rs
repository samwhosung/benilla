//! The pet frame's right-click menu's predicates. `UnitPopup.lua:402-416` shows the paper doll and
//! Abandon when `PetCanBeAbandoned()`, Rename when `PetCanBeRenamed()` too, and Dismiss otherwise.
//! `PetAbandon` (`0x4be4c0`, `0x4bd740`) sends `CMSG_PET_ABANDON` inside its call
//! ([`super::PetSteps`]); `PetDismiss` (`0x4be4d0`) sends no packet itself but hands the Dismiss
//! command word to the bar's dispatcher (`0x4bd1d0`), so it leaves as `CMSG_PET_ACTION`, and only
//! past the dispatcher's usability predicate (`0x4bd1f2`). vmangos unsummons a summon on either,
//! so sending the wrong one goes unseen.

use bevy::prelude::*;

use super::{PetBar, PetUnit};

/// `UNIT_FLAG_PET_RENAME`, `0x4be5c4`'s mask: set on a tamed pet not yet renamed. No client code
/// writes it; the server clears it after a rename (the sender re-checks it at `0x4bd8b5`).
pub(super) const UNIT_FLAG_PET_RENAME: u32 = 0x0000_0010;
/// `UNIT_FLAG_PET_ABANDON`, `0x4be544`'s mask: set on a hunter pet, clear on a summon.
const UNIT_FLAG_PET_ABANDON: u32 = 0x0000_0020;

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
pub(super) fn feed_pet_menu(
    mut vm: crate::script_calls::VmWith<Res<'static, PetBar>>,
    pet: PetUnit,
) {
    vm.turn(|script, bar| {
        let Some(script) = script else {
            return;
        };
        let flags = (bar.spells.pet_guid != 0)
            .then(|| pet.store(bar.spells.pet_guid))
            .flatten()
            .filter(|store| store.0.unit_summoned_by() == pet.self_guid.0)
            .map_or(0, |store| store.0.unit_flags());
        let (can_be_abandoned, can_be_renamed) = menu_predicates(flags);
        script.set_pet_menu(can_be_abandoned, can_be_renamed);
    })
}
