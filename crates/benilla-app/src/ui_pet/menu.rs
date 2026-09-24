//! The pet **right-click menu** — its two predicates and its three verbs (decision 1066).
//!
//! `PetFrame_OnClick`'s else arm opens `PetFrameDropDown` on `UnitPopupMenus["PET"]`, whose four
//! rows are decided by `PetCanBeAbandoned()` alone (`UnitPopup.lua:402-416`): paperdoll and abandon
//! show when it is true, dismiss when it is false, rename when it and `PetCanBeRenamed()` both are.
//! So one predicate makes the same menu read *Abandon* on a hunter's pet and *Dismiss* on a
//! warlock's demon — and the row a player sees is the whole of the difference the client draws
//! between "give this pet up forever" and "send this summon away".
//!
//! **The predicates** `0x4be500` and `0x4be580` are byte-identical but for three dwords:
//! resolve `[0xb714a0]`, check the pet's `UNIT_FIELD_SUMMONEDBY` against the local player, then test
//! **one byte of `UNIT_FIELD_FLAGS`** — `[pet+0x110]+0xa0`, dword 46 — with mask `0x20` for abandon
//! (`0x4be544`) and `0x10` for rename (`0x4be5c4`). Each returns exactly one value on every path: a
//! double `1.0`, or nil.
//!
//! **The `UNIT_FIELD_BYTES_2` byte-2 layout everybody remembers is REFUTED here** — that is the
//! TBC+ home of these bits, its record is dword 164, and neither predicate touches it. This is the
//! shape decision 0988 warns about, and it was nearly built wrong: benilla's own
//! queue had recorded the TBC guess, off a field that is always zero on this server.
//!
//! **The two give-up verbs are NOT one opcode** (the seam's two counts, kept apart on principle,
//! are what made the fold a two-line change):
//!
//! - `PetAbandon 0x4be4c0` → `0x4bd740` sends **`CMSG_PET_ABANDON` 0x176** `{u64 petGuid}`.
//! - `PetDismiss 0x4be4d0` → `0x4bd6e0` **opens no packet at all**. It stages the packed slot word
//!   `0x07000003` — command type 7, action 3, `PET_COMMAND_DISMISS` — and hands it to `0x4bd1d0`,
//!   the *same dispatcher every pet-bar click uses*, which sends **`CMSG_PET_ACTION` 0x175**
//!   `{u64 petGuid, u32 0x07000003, u64 0}`. Dismiss is a bar press with no button.
//!
//! Both would have worked against vmangos — `HandlePetAbandon` unsummons a non-hunter pet, and so
//! does `COMMAND_DISMISS` — which is exactly why the wrong one would never have been noticed.

use bevy::prelude::*;

use benilla_protocol::messages::{PET_ACT_COMMAND, PET_COMMAND_DISMISS};
use benilla_ui::script::UiScript;

use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::{UiError, UiErrorKeys};

use super::{PetBar, PetUnit};

/// `UNIT_FIELD_FLAGS` bit 4 — `UNIT_FLAG_PET_RENAME`, `0x4be5c4`'s mask. Set on a tamed hunter pet
/// that has never been renamed.
///
/// **One-shot, and server-side only.** Of the field-view reads of `[+0x110]+0xa0`, exactly three
/// test these two masks — the two predicates and the rename sender's own re-check (`0x4bd8b5`) —
/// and **no writer exists anywhere in `.text`**. Nothing client-side clears this; the row
/// disappears because the server re-streams the field and `UnitPopup` re-evaluates when the menu is
/// next built. There is no event to listen for and none is needed.
const UNIT_FLAG_PET_RENAME: u32 = 0x0000_0010;
/// `UNIT_FIELD_FLAGS` bit 5 — `UNIT_FLAG_PET_ABANDON`, `0x4be544`'s mask. Set on a hunter pet and
/// clear on a summon, which is what forks the whole menu.
const UNIT_FLAG_PET_ABANDON: u32 = 0x0000_0020;

/// The client's own silent cap on a pet name — `0x64a7f0(dst, 0x50, "%s", name)` in the rename
/// sender `0x4bd840`, i.e. 79 characters plus the terminator.
///
/// It is **not** the 12 everyone knows: that is `RENAME_PET`'s `maxLetters` in FrameXML, a
/// different layer with a different job. This one is the last thing between the Lua and the wire,
/// it truncates rather than refusing, and it exists because the binding is reachable from any
/// script, not only from the dialog.
const PET_NAME_MAX: usize = 79;

/// `(PetCanBeAbandoned, PetCanBeRenamed)` from a pet's `UNIT_FIELD_FLAGS` — the whole fork, in one
/// place, so the one mistake that matters here has somewhere to be tested: swapping the two masks
/// would offer a hunter *Dismiss* and a warlock *Abandon*, which is quest-breaking in one direction
/// and pet-losing in the other.
pub(super) fn menu_predicates(pet_flags: u32) -> (bool, bool) {
    (
        pet_flags & UNIT_FLAG_PET_ABANDON != 0,
        pet_flags & UNIT_FLAG_PET_RENAME != 0,
    )
}

/// Push the menu's two predicates off the pet's descriptor.
///
/// **The ownership test is the predicates' own**, and it is `UNIT_FIELD_SUMMONEDBY` **alone** —
/// `0x4be500` compares `[pet+0x110]+0x18/0x1c` against the local player before either bit is read,
/// with no `CHARMEDBY` leg. That distinction is the third one in this folder (`0x5ff580` falls back
/// from charmed to summoned, `0x5ee5a0` from charmed to created, this reads summoned flat), and it
/// is exactly the case it decides: a **charmed** unit has the bar and the cached guid, and is not
/// somebody's summon — so its menu is empty, which is right, because it is not yours to abandon.
///
/// No pet, an unstreamed pet, or a pet that is not our summon pushes `(false, false)` — every row
/// off. That is the honest answer and also the useful one: `UnitPopup_ShowMenu` counts the shown
/// rows and **refuses to open a menu with nothing but CANCEL in it**, so an unresolved pet takes no
/// menu rather than an empty one.
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

/// Drain the menu's three verbs onto the wire.
///
/// Each carries [`PetBar`]'s cached pet guid rather than anything the VM supplied — the drain's own
/// rule, and here it also means a verb queued in the frame the pet left dies rather than naming a
/// unit that is gone. Nothing is applied locally: unlike the bar's presses, all three ask the
/// *server* for something it may refuse, and it answers by removing the pet or by bumping its name
/// timestamp.
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

    // Counted rather than deduped: a double click on the confirm is two presses, and the reference
    // sends two.
    for _ in 0..abandons {
        debug!("ui_pet: abandoning pet {pet_guid:#x}");
        let _ = commands.0.send(ClientCommand::PetAbandon { pet_guid });
    }
    // Dismiss is a bar press with no button (the header): the word `0x07000003` down the ordinary
    // `CMSG_PET_ACTION` path, target 0. Built from the constants rather than written as the literal
    // so it stays the same word `PET_COMMAND_DISMISS` means everywhere else in this folder.
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
        // The sender's own two steps, in its order (`0x4bd840`): an empty name is refused to the
        // error line and sends nothing, then the rest is truncated silently. Neither is the
        // dialog's 12-letter cap — this is the layer under it.
        if name.is_empty() {
            ui_errors.0.push(UiError::key("ERR_NULL_PETNAME"));
            continue;
        }
        let name = clamp_pet_name(&name);
        debug!("ui_pet: renaming pet {pet_guid:#x} to {name:?}");
        let _ = commands.0.send(ClientCommand::PetRename { pet_guid, name });
    }
}

/// [`PET_NAME_MAX`]'s truncation, on **character** boundaries.
///
/// The reference counts bytes into a fixed buffer; we cannot, because a mid-character cut would not
/// be a shorter name but an invalid string. Every name a 1.12 client can produce is well inside the
/// cap either way — the clamp is here so a scripted `PetRename` cannot hand the wire something the
/// server will read differently than we meant.
fn clamp_pet_name(name: &str) -> String {
    name.chars().take(PET_NAME_MAX).collect()
}
