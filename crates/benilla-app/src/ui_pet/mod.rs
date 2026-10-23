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
use crate::ui_unit::UnitFeed;

mod bar;
pub(crate) use bar::{active_aura_press, drop_bar, PetBarFeed};
mod drain;
mod menu;
mod net;
mod steps;
mod unit;
mod usable;
// The predicate `0x4bcf70` over what the caller already holds, for the action bar's pet legs.
pub(crate) use usable::actions_usable as pet_actions_usable;

use bar::{feed_pet_bar, fire_pet_cooldown_events};
// A pet bar press and the pet's other verbs, applied in call order by `crate::script_calls`.
pub(crate) use drain::call_pet_off;
pub(crate) use drain::{cast_pet_press, PetPress, PetStep};
use menu::feed_pet_menu;
pub(crate) use steps::PetSteps;
pub(crate) use unit::{feed_pet_unit, pet_state, PetSources};

#[cfg(test)]
mod call_tests;
#[cfg(test)]
mod flush_tests;
#[cfg(test)]
mod press_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod usable_tests;

/// The bar's feed as a system a test outside the pet module runs pass by pass, its memory kept
/// between them.
#[cfg(test)]
pub(crate) fn pet_bar_feed(world: &mut World) -> bevy::ecs::system::SystemId {
    world.init_resource::<bar::PetBarFeed>();
    world.register_system(feed_pet_bar)
}

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
    /// `PET_BAR_UPDATE` signals, counted. Both state writes signal unconditionally (`0x4bc940`,
    /// `0x4bc960`), and `OnClick`'s `SetChecked(0)` needs that repaint to relight a press on the
    /// current mode, so the count is in the feed's dedup key. Wrapping: only a change is read.
    pub(crate) bar_signals: u32,
    /// `[0xb714a8]`: the charm or possess expiry, the packet's duration past its arrival; `None`
    /// when the duration is 0, as for a hunter's or warlock's own pet.
    pub(crate) expires: Option<Instant>,
}

/// The client's `[0xb714b0]`, a local latch: all of `IsPetAttackActive`, and the only thing that
/// lights Attack (`0x4bdf16`-`0x4bdf22`). Raised only for a possessed unit (`0x4bd420`), so never
/// on a hunter's bar; lowered by `PetStopAttack` (`0x4bd650`), a new pet (`0x4bc8ce`) and the
/// selection's release (`0x493a18`). `PET_ATTACK_*` read the pet's flag `0x800` instead. Up, it
/// holds the bar's guid it was raised under (`[0xb714a0]`), the guid the stop is sent for, which
/// a new pet's lowering keeps the bar's; apart from [`PetBar`], so the selection's release
/// reaches it beside every holder of the bar.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PetAttackLatch(pub(crate) Option<u64>);

impl PetAttackLatch {
    /// Whether the latch is up.
    pub(crate) fn up(&self) -> bool {
        self.0.is_some()
    }
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

/// `CGUnit_C::IsPlayerOrHunterPet` (`0x6116e0`) on `unit`, held under `guid`: a player
/// (`0x6116ec`), or a unit with a pet number (`0x611704`) that our charm, else our summon, names
/// (`0x61172d`-`0x61174a`) while our class is Hunter (`0x611757`). `player` is our own object,
/// `None` while it is not held (`0x611725`). The unit must resolve as a unit first (typemask 8).
pub(crate) fn is_player_or_hunter_pet(
    unit: &crate::net::ObjectStore,
    guid: u64,
    player: Option<&crate::net::ObjectStore>,
) -> bool {
    /// `TYPEMASK_UNIT` and `TYPEMASK_PLAYER`.
    const UNIT: u32 = 0x8;
    const PLAYER: u32 = 0x10;
    /// `UNIT_FIELD_BYTES_0` byte 1, Hunter.
    const CLASS_HUNTER: u8 = 3;
    let mask = unit.0.object_type_mask().unwrap_or(0);
    if mask & UNIT == 0 {
        return false;
    }
    if mask & PLAYER != 0 {
        return true;
    }
    if unit.0.unit_pet_number() == 0 {
        return false;
    }
    let Some(player) = player else {
        return false;
    };
    player.0.unit_pet_guid() == Some(guid) && player.0.unit_class() == Some(CLASS_HUNTER)
}

pub(crate) struct UiPetPlugin;

impl Plugin for UiPetPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<PetBar>()
            .init_resource::<PetAttackLatch>()
            .init_resource::<bar::PetBarFeed>()
            // `SMSG_PET_SPELLS` writes its `UNIT_PET` there; the unit plugin fires the lane.
            .init_resource::<crate::field_watch::NoticeLane>()
            .add_systems(
                Update,
                (
                    // Feeds ride the unit feed, before the VM ticks; every verb is a script call
                    // (`PetPress`, `PetSteps`).
                    // The GCDs the attempt's sends owe the pet's list, in before the bar reads it.
                    drain::arm_owed_pet_gcds
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
                        .in_set(crate::ui_unit::field_events::FieldEventPushes)
                        .after(crate::ui_pet_stats::PetSnapshot),
                    feed_pet_menu.in_set(UnitFeed),
                ),
            );
    }
}

/// The unit behind [`PetBar`]'s cached guid, plus our own guid for the ownership tests the
/// reference makes before trusting the pet's fields (`0x5ff780`, `0x612e33`), and the usability
/// predicate every pet-bar verb the reference guards asks of them ([`PetUnit::usable_pet`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetUnit<'w, 's> {
    index: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    self_guid: Res<'w, crate::net::SelfGuid>,
}

impl PetUnit<'_, '_> {
    /// The pet's descriptor, `None` while its object is not streamed: the reference's "no pet".
    pub(crate) fn store(&self, pet_guid: u64) -> Option<&ObjectStore> {
        let e = *self.index.0.get(&pet_guid)?;
        self.stores.get(e).ok()
    }

    /// The active player's descriptor, `None` while its object is not streamed: the lookup the
    /// usability predicate makes on the client's player guid (`0x4bcf89`, typemask `0x10`).
    pub(crate) fn player_store(&self) -> Option<&ObjectStore> {
        self.store(self.self_guid.0?)
    }
}
