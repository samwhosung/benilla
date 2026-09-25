//! The pet system: [`PetBar`], the pet action bar's state, and the app side of
//! `benilla_ui::script::pet`. The usual three commands, four spells and three reactions are server
//! data (vmangos `CharmInfo::InitPetActionBar`), not a layout: a possessed or charmed unit fills
//! the same ten words differently, so nothing here assumes them.

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

/// The pet action bar's state. A zero `spells.pet_guid` means no bar: the teardown packet carries
/// only that guid.
#[derive(Resource, Default)]
pub(crate) struct PetBar {
    /// The last `SMSG_PET_SPELLS`, with `SMSG_PET_MODE` and local presses folded in.
    pub(crate) spells: PetSpells,
    /// The pet's own cooldowns: the reference keeps a `SPELLHISTORY` per unit. Seeded from
    /// `SMSG_PET_SPELLS`, topped up by `SMSG_SPELL_COOLDOWN` for the pet's guid.
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
}

impl PetBar {
    /// `PetHasActionBar()`: a nonzero cached guid, with no alive or control check (`0x4bdc20`).
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
                // Feeds ride the unit feed, before the VM ticks; drains run after the input pass so
                // a click is sent that frame. The old-target clear precedes the bar feed so Attack
                // goes dark the frame the selection moves.
                pet_stop_on_old_target_clear
                    .in_set(UnitFeed)
                    .before(feed_pet_bar),
                // After the pet snapshot: these feeds' events reach Lua at once, and their
                // handlers read `HasPetUI()`, which `crate::ui_pet_stats` pushes.
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

    /// The pet's entity under [`Self::store`]'s contract, for the pet paper doll's model.
    pub(crate) fn entity(&self, pet_guid: u64) -> Option<Entity> {
        let e = *self.index.0.get(&pet_guid)?;
        self.stores.contains(e).then_some(e)
    }
}
