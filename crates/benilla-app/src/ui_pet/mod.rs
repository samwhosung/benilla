//! The pet system: [`PetBar`], the pet action bar's state, and the app side of
//! `benilla_ui::script::pet`. The usual three commands, four spells and three reactions are server
//! data (vmangos `CharmInfo::InitPetActionBar`), not a layout: a possessed or charmed unit fills
//! the same ten words differently, so nothing here assumes them.

use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::PetSpells;

use crate::net::{GuidIndex, ObjectStore};
use crate::spell::Cooldowns;
use crate::ui_action::CooldownEvents;
use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;

mod bar;
mod drain;
mod menu;
mod net;
mod unit;

use bar::{feed_pet_bar, fire_pet_cooldown_events};
use drain::drain_pet_actions;
// A pet bar press, applied in call order by `crate::script_calls`.
pub(crate) use drain::pet_stop_on_old_target_clear;
pub(crate) use drain::PetPress;
use menu::{drain_pet_menu, feed_pet_menu};
use unit::feed_pet_unit;

#[cfg(test)]
mod flush_tests;
#[cfg(test)]
mod press_tests;
#[cfg(test)]
mod tests;

/// The pet action bar's state. A zero `spells.pet_guid` means no bar: the teardown packet carries
/// only that guid.
#[derive(Resource, Default)]
pub(crate) struct PetBar {
    /// The last `SMSG_PET_SPELLS`, with `SMSG_PET_MODE` and local presses folded in.
    pub(crate) spells: PetSpells,
    /// The pet's own cooldowns, the list at `0xcecb04`. Seeded from `SMSG_PET_SPELLS`, topped up
    /// by `SMSG_SPELL_COOLDOWN` for the pet's guid, by the pet's own `SMSG_SPELL_GO` and by a
    /// press's GCD. Each mutation but the seed fires the flush, off [`Cooldowns::generation`]
    /// (`bar::fire_pet_cooldown_events`).
    pub(crate) cooldowns: Cooldowns,
    /// The client's `[0xb714b0]`, a local latch: all of `IsPetAttackActive`, and the only thing
    /// that lights Attack (`0x4bdf16`-`0x4bdf22`). Raised only for a possessed unit (`0x4bd420`),
    /// so never on a hunter's bar; lowered by `PetStopAttack` (`0x4bd650`), a new pet (`0x4bc8ce`)
    /// and the old-target clear (`0x493a18`). `PET_ATTACK_*` read the pet's flag `0x800` instead.
    pub(crate) attacking: bool,
    /// `PET_BAR_UPDATE` signals, counted. Both state writes signal unconditionally (`0x4bc940`,
    /// `0x4bc960`), and `OnClick`'s `SetChecked(0)` needs that repaint to relight a press on the
    /// current mode, so the count is in the feed's dedup key. Wrapping: only a change is read.
    pub(crate) bar_signals: u32,
    /// `[0xb714a8]`: the charm or possess expiry, the packet's duration past its arrival; `None`
    /// when the duration is 0, as for a hunter's or warlock's own pet.
    pub(crate) expires: Option<Instant>,
}

impl PetBar {
    /// `PetHasActionBar()`: a nonzero cached guid, with no alive or control check (`0x4bdc20`).
    pub(crate) fn has_bar(&self) -> bool {
        self.spells.pet_guid != 0
    }

    /// No pet: every field back to its default, the cooldown list emptied with its counters kept
    /// ([`Cooldowns::clear_silent`]), for the teardown packet and the session end.
    pub(crate) fn clear(&mut self) {
        let mut cooldowns = std::mem::take(&mut self.cooldowns);
        cooldowns.clear_silent();
        *self = Self {
            cooldowns,
            ..Default::default()
        };
    }
}

pub(crate) struct UiPetPlugin;

impl Plugin for UiPetPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<PetBar>().add_systems(
            Update,
            (
                // Feeds ride the unit feed, before the VM ticks; drains run after the input pass so
                // a click is sent that frame (a press is a script call, `PetPress`). The old-target
                // clear precedes the bar feed so Attack
                // goes dark the frame the selection moves.
                pet_stop_on_old_target_clear
                    .in_set(UnitFeed)
                    .before(feed_pet_bar),
                // After the pet snapshot: these feeds' events reach Lua at once, and their
                // handlers read `HasPetUI()`, which `crate::ui_pet_stats` pushes. The bar pushes
                // its cooldown triples before the cooldown events and the pet's flush fires after
                // them, so the buttons it wakes read the new list.
                feed_pet_bar
                    .in_set(UnitFeed)
                    .after(crate::ui_pet_stats::PetSnapshot)
                    .before(CooldownEvents),
                fire_pet_cooldown_events
                    .in_set(UnitFeed)
                    .after(CooldownEvents),
                feed_pet_unit
                    .in_set(UnitFeed)
                    .after(crate::ui_pet_stats::PetSnapshot),
                feed_pet_menu.in_set(UnitFeed),
                // After this frame's presses, as the one drain before it ran them ahead of the
                // toggles, stops and writes.
                drain_pet_actions
                    .after(UiInput)
                    .after(crate::script_calls::apply_script_calls),
                drain_pet_menu.after(UiInput),
            ),
        );
    }
}

/// The unit behind [`PetBar`]'s cached guid, plus our own guid for the ownership tests the
/// reference makes before trusting the pet's fields (`0x5ff780`, `0x612e33`).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetUnit<'w, 's> {
    index: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    self_guid: Res<'w, crate::net::SelfGuid>,
    /// The per-field edges, for `fire_transitions`' watch-bridge arms.
    pub(super) edges: MessageReader<'w, 's, crate::net::FieldChanged>,
}

impl PetUnit<'_, '_> {
    /// The pet's descriptor, `None` while its object is not streamed: the reference's "no pet".
    pub(crate) fn store(&self, pet_guid: u64) -> Option<&ObjectStore> {
        let e = *self.index.0.get(&pet_guid)?;
        self.stores.get(e).ok()
    }

    /// The active player's descriptor, `None` while its object is not streamed: the lookup the
    /// pet bar's spell arm makes on the client's player guid (`0x4bd31a`, typemask `0x10`).
    pub(crate) fn player_store(&self) -> Option<&ObjectStore> {
        self.store(self.self_guid.0?)
    }

    /// The pet's entity under [`Self::store`]'s contract, for the pet paper doll's model.
    pub(crate) fn entity(&self, pet_guid: u64) -> Option<Entity> {
        let e = *self.index.0.get(&pet_guid)?;
        self.stores.contains(e).then_some(e)
    }
}
