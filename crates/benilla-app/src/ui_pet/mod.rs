//! The pet system — the app side of `benilla_ui::script::pet`'s seam, and the
//! home of [`PetBar`], the server-authoritative state `net/apply/pet.rs` writes.
//!
//! **One concern per file** (the split owed by decision 1003, restated by 1005, done in 1066). What
//! decides the cut is *which clock a thing moves on*, which is also why these never wanted to be
//! one file:
//!
//! | module | what it owns | moves on |
//! |---|---|---|
//! | [`bar`] | the action bar's **feed** — ten packed words rendered as ten `PetActionView`s | `SMSG_PET_SPELLS` |
//! | [`unit`] | the **`"pet"` unit token** and the pet frame's events | the pet's descriptor |
//! | [`drain`] | the bar's click **intents** → the wire, and the state they latch locally | a click |
//! | [`menu`] | the **right-click menu**: two predicates, three verbs | a menu pick |
//!
//! [`PetBar`], [`PetUnit`] and [`UiPetPlugin`] stay here, because every module reads them.
//!
//! **The slot law** (decode and constants in `benilla_protocol::messages::pet`, VERIFIED against
//! the client itself in decision 0988): each slot is one packed word whose type — bits 24–29,
//! **masked** — decides its whole reading:
//!
//! - `PET_ACT_COMMAND 7` — `action` is a `CommandStates`: Stay 0 / Follow 1 / Attack 2 / Dismiss 3.
//!   A token: `GetPetActionInfo` returns the NAMES of two globals rather than values (the
//!   reference's own `PetActionBarFrame.lua:98-104` fork), which `PetActionBar.xml` resolves —
//!   `PET_ACTION_*` out of the shipped `GlobalStrings.lua`, `PET_*_TEXTURE` out of the constants
//!   that file declares and ours quotes.
//! - `PET_ACT_REACTION 6` — `action` is a `ReactStates`: Passive 0 / Defensive 1 / Aggressive 2.
//!   Also a token.
//! - types **1–5** — `action` is a spell id, and the autocast pair rides in bits 31/30 rather than
//!   in the type. That split is why the server's `ACT_PASSIVE`/`ACT_DISABLED`/`ACT_ENABLED` are
//!   one branch here and not three.
//!
//! The two halves of `GetPetActionInfo`'s signature never overlap: `isActive` is a **token**
//! concept and `autoCast*` a **spell** one, each nil on the other's branch.
//!
//! The **3 commands / 4 spells / 3 reactions** arrangement everyone recognises is therefore
//! *server data*, not a layout: it is what `CharmInfo::InitPetActionBar` happens to write, and a
//! possessed minion or a charmed creature arrives with a different mix through the same ten words.
//! Nothing in this folder or in `PetActionBar.xml` assumes it.

use bevy::prelude::*;

use benilla_protocol::messages::PetSpells;

use crate::net::{GuidIndex, ObjectStore};
use crate::spell::Cooldowns;
use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;

mod bar;
mod drain;
mod menu;
mod net;
mod unit;

use bar::feed_pet_bar;
use drain::{drain_pet_actions, pet_stop_on_old_target_clear};
use menu::{drain_pet_menu, feed_pet_menu};
use unit::feed_pet_unit;

#[cfg(test)]
mod tests;

/// The pet action bar's mirror of the server's state — written only by `net/apply/pet.rs`, read
/// only by [`feed_pet_bar`] and [`drain_pet_actions`].
///
/// `spells.pet_guid == 0` is "there is no pet bar", and it is the single test: the teardown packet
/// carries exactly that and nothing else.
///
/// The session end resets it the same way (`net::on_session_end`): a dropped socket sends no teardown.
#[derive(Resource, Default)]
pub(crate) struct PetBar {
    /// The last `SMSG_PET_SPELLS` in full, with `SMSG_PET_MODE`'s state edits folded in.
    pub(crate) spells: PetSpells,
    /// The **pet's own** cooldown store — a second [`Cooldowns`] instance, not the player's. The
    /// two must not share: a hunter and their pet both know Growl-shaped spells on independent
    /// timers, and the reference keeps a `SPELLHISTORY` list per unit for exactly this reason.
    /// Seeded from the `SMSG_PET_SPELLS` tail and topped up by `SMSG_SPELL_COOLDOWN` addressed to
    /// the pet's guid.
    pub(crate) cooldowns: Cooldowns,
    /// **"The possessed unit is attacking"** — the client's own `[0xb714b0]`, a purely local latch
    /// with no field behind it anywhere, and **the possess bar's, not the pet bar's**.
    ///
    /// It is `IsPetAttackActive` entire, and `GetPetActionInfo`'s COMMAND branch ORs it into
    /// `isActive` for action 2 (`0x4bdf16`–`0x4bdf22`), so it is also the only thing that can ever
    /// light an Attack button. What gates the one write that raises it is the whole story
    /// ([`possessing`]): `0x4bd420` demands `0x5ee5a0(player) == the bar's unit`, and `0x5ee5a0`
    /// answers the **possessed** unit or nothing. A hunter's pet is not possessed, so on an
    /// ordinary pet bar this latch **can never rise** — which is exactly the director's report:
    /// Attack is a button you press, never a mode that stays lit.
    ///
    /// The three ways down are transcribed anyway, because under possession they are live: the lit
    /// button's own second press (Lua `PetStopAttack` → `0x4bd650`), a new pet (`0x4bc8ce`), and
    /// the **old-target clear `0x493910` at `0x493a18`** ([`pet_stop_on_old_target_clear`]), which
    /// every selection writer runs.
    ///
    /// benilla once read this off the pet's streamed `UNIT_FIELD_TARGET`. That is a
    /// different question with a different answer: a defensive pet that retaliates on its own has
    /// a target the player never ordered, and the reference does not light the Attack button for
    /// it either.
    ///
    /// **It is not the source of `PET_ATTACK_START`/`PET_ATTACK_STOP`**, however much it reads like
    /// one — decision 0990 made exactly that inference and it was wrong. Those ride the pet's
    /// server-owned `UNIT_FIELD_FLAGS` bit `0x800` instead ([`feed_pet_unit`]). This latch answers
    /// "did the player order the unit they are *driving* to attack"; the flag answers "is the pet
    /// fighting", and it is the flag that moves for a hunter.
    pub(crate) attacking: bool,
    /// **`SignalEvent(0x161 PET_BAR_UPDATE)`, counted** — the half of a press that is not the state
    /// write and matters just as much.
    ///
    /// Both state writers end on an **unconditional** signal, with no compare of the old value
    /// against the new one:
    ///
    /// ```text
    /// 0x4bc940  [0xb71468] = state & 0xFFFFFF00 | action ; ecx = 0x161 ; jmp SignalEvent  (reaction)
    /// 0x4bc960  [0xb71468] = state & 0x080000FF | a << 8 ; ecx = 0x161 ; jmp SignalEvent  (command)
    /// ```
    ///
    /// That is not redundancy, it is load-bearing, and missing it is visible on the first click:
    /// the CheckButton widget flips itself before `OnClick` runs and the reference's first line is
    /// `this:SetChecked(0)`, so **every** press starts by taking the light off. What puts it back
    /// is `PetActionBar_Update` running on this signal and re-deriving `isActive`. Press the mode
    /// the pet is *already* in and nothing about the state changes — but the signal still fires and
    /// the light still comes back.
    ///
    /// benilla's feed dedups on what it last pushed ([`PetBarMemory`]), which is right for a
    /// per-frame feed and wrong here: a no-change press pushed nothing, fired nothing, and left the
    /// button visually un-toggled with no way back until something else moved. So the counter is
    /// folded into the dedup key — a bump is a forced repaint, which is exactly what the signal
    /// buys the reference. Wrapping, because only its *changes* are ever read.
    pub(crate) bar_signals: u32,
}

impl PetBar {
    /// `PetHasActionBar()` — is there a bar at all. The client's own gate is exactly this, a
    /// nonzero cached pet guid: no alive check, no control check (`0x4bdc20`).
    pub(crate) fn has_bar(&self) -> bool {
        self.spells.pet_guid != 0
    }
}

pub(crate) struct UiPetPlugin;

impl Plugin for UiPetPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<PetBar>().add_systems(
            Update,
            (
                // Every feed rides with the unit feed (before the VM ticks, like the stance bar's);
                // each drain runs after the input pass so a click goes out the same frame.
                // The old-target clear runs BEFORE the feed so the Attack button goes out in the
                // same frame the selection moved, not one behind it.
                pet_stop_on_old_target_clear
                    .in_set(UnitFeed)
                    .before(feed_pet_bar),
                // …and both event-firing feeds run AFTER the pet snapshot. Their
                // events reach Lua synchronously, and the handlers read `HasPetUI()` — which is
                // `crate::ui_pet_stats`'s push, not ours. Unordered, a cold summon fired both
                // edges against last frame's answer and the Pet tab never came up.
                feed_pet_bar
                    .in_set(UnitFeed)
                    .after(crate::ui_pet_stats::PetSnapshot),
                feed_pet_unit
                    .in_set(UnitFeed)
                    .after(crate::ui_pet_stats::PetSnapshot),
                feed_pet_menu.in_set(UnitFeed),
                drain_pet_actions.after(UiInput),
                drain_pet_menu.after(UiInput),
            ),
        );
    }
}

/// **The unit behind [`PetBar`]'s cached guid, plus our own identity** — the one lookup both pet
/// systems need and neither owns.
///
/// It exists because the client's two newest pet answers are both *about the pet's own
/// descriptor* rather than about the bar: the `PET_ATTACK_*` edge reads the pet's
/// `UNIT_FIELD_FLAGS`, and the ATTACK order's validator reads the pet's whole eligibility. Both
/// also need the active player's guid, because both test **ownership** before they trust what they
/// read (`0x5ff780` and `0x612e33`).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetUnit<'w, 's> {
    index: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    self_guid: Res<'w, crate::net::SelfGuid>,
    /// The per-field edges, for `fire_transitions`' watch-bridge arms.
    pub(super) edges: MessageReader<'w, 's, crate::net::FieldChanged>,
}

impl PetUnit<'_, '_> {
    /// The pet's streamed descriptor, or `None` while the bar names a guid whose object has not
    /// arrived (or has left) — the case the reference treats as "no pet", never as "a bad pet".
    pub(crate) fn store(&self, pet_guid: u64) -> Option<&ObjectStore> {
        let e = *self.index.0.get(&pet_guid)?;
        self.stores.get(e).ok()
    }

    /// The pet's ECS **entity**, under [`Self::store`]'s exact contract — `None` while the named
    /// guid's object has not arrived (or has left). The pet paper doll's body booth needs the
    /// entity itself, not its fields (`crate::ui_pet_doll`); gating it on the same
    /// store presence keeps "there is a pet" one answer rather than two that can disagree.
    pub(crate) fn entity(&self, pet_guid: u64) -> Option<Entity> {
        let e = *self.index.0.get(&pet_guid)?;
        self.stores.contains(e).then_some(e)
    }
}
