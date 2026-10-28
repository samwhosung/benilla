//! The stable window's app side: [`StableOpen`] holds the `MSG_LIST_STABLED_PETS` rows, which the
//! server sends unprompted for the gossip stable option; the getters read them through the host as
//! they run ([`live`]), the handlers and the creature callbacks signal on the stable lane
//! ([`lane`]), and the stable verbs, the close among them, act inside their calls
//! ([`StableSteps`]).
//!
//! A row's family, and with it the icon and diet, is not on the wire: it comes from the creature
//! record, which a getter that misses it asks with the window's callback
//! ([`StableCreatureWaits`]), whose landing signals the window to read it again.
//! Slot 0 is read from the wire like every row, because a dismissed pet still has one; stock
//! `PetStable.lua:123-138` prefers the live pet and falls back to it.

use benilla_protocol::messages::StabledPet;
use bevy::prelude::*;

use benilla_ui::script::{StableCall, StableIntent, StablePetSlot, StableState, NUM_STABLE_SLOTS};

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::Ask;
use crate::ui_pet_stats::{PetFamilyTables, PetStatTables};
use crate::ui_script::{SweepApp, SweepLeg};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

#[cfg(test)]
mod call_tests;
pub(crate) mod lane;
pub(crate) mod live;
mod net;
pub(crate) use lane::{StableEvent, StableLane};
pub(crate) use live::StableLive;

pub(crate) struct UiStablePlugin;

impl Plugin for UiStablePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        lane::add_stable_lane(app);
        app.init_resource::<StableOpen>()
            .init_resource::<StableCreatureWaits>()
            .add_latched_window::<StableOpen>()
            // `0x490b2a` → `0x4caae0` → `0x4cae10`, the core.
            .add_sweep_leg(SweepLeg::Stable, close_core);
    }
}

/// The stable's module state: its master's cell and the last list's rows as the wire gave them.
/// A close clears only the cell (`0x4cae29`), and the world entry's reset the rest (`0x4caab0`).
#[derive(Resource, Default)]
pub(crate) struct StableOpen {
    pub(crate) npc: Option<u64>,
    /// Slots bought (0..=2), not slots occupied.
    pub(crate) num_stable_slots: u8,
    /// The wire rows, each with its client slot index.
    pub(crate) pets: Vec<StabledPet>,
    /// One step per list, which the VM's copy carries ([`benilla_ui::script::StableQuery`]).
    pub(crate) list: u64,
    /// The cascade's reset, owed from its item's push ([`StableOpen::settle_world_reset`]).
    pub(crate) world_reset: crate::ui_world_enter::OwedReset,
}

impl StableOpen {
    pub(crate) fn open(&mut self, npc: u64, num_stable_slots: u8, pets: Vec<StabledPet>) {
        self.settle_world_reset();
        self.npc = Some(npc);
        self.num_stable_slots = num_stable_slots;
        self.pets = pets;
        self.list = self.list.wrapping_add(1);
    }

    /// The world-enter cascade's reset (`0x490a0a` → `0x4caab0`): no master, no slot bought, no
    /// pet; the session's end runs it too.
    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.num_stable_slots = 0;
        self.pets.clear();
        self.world_reset = Default::default();
    }

    /// The cascade's reset, owed from its push, at its place or ahead of a later stable packet
    /// ([`crate::ui_world_enter::OwedReset`]); the close keeps the count (`0x4cae10`), so a getter
    /// before it reads what the close left.
    pub(crate) fn settle_world_reset(&mut self) {
        if self.world_reset.take() {
            self.clear();
        }
    }

    /// The close core's clear of the cell (`0x4cae29`); the rows and the count stay.
    pub(crate) fn close_cell(&mut self) {
        self.npc = None;
    }
}

/// The stable master's cell `[0xb72240]`; `ClosePetStables` (`0x4cae40`) is the core itself, run
/// inside its call ([`crate::script_calls::ScriptCallHost`]). The
/// server also checks range on every verb (`CheckStableMaster`), failing with code 6, which shows
/// nothing.
impl LatchedWindow for StableOpen {
    const WINDOW: NpcWindow = NpcWindow::Stable;

    fn cell(&self) -> Option<u64> {
        self.npc
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }
}

/// The close core `0x4cae10`: with a stable open, the latch release (`0x4cae24`), the cell
/// cleared, then `PET_STABLE_CLOSED` (`0x4cae3a`), held behind what the lane still holds
/// ([`lane::behind_lane`]).
pub(crate) fn close_core(world: &mut World) {
    if world.resource::<StableOpen>().npc.is_none() {
        return;
    }
    lane::behind_lane(world, |world| {
        let Some(npc) = world.resource::<StableOpen>().npc else {
            return;
        };
        crate::ui_session::release(world, npc);
        world.resource_mut::<StableOpen>().close_cell();
        crate::ui_session::fire(world, |script| {
            script.fire_event("PET_STABLE_CLOSED", vec![]);
        });
    });
}

/// The stable's reset in the cascade's place (`0x490a0a` → `0x4caab0`), unless a later stable
/// packet ran it first.
pub(crate) fn reset_on_world_enter(open: Option<ResMut<StableOpen>>) {
    if let Some(mut open) = open {
        open.settle_world_reset();
    }
}

/// `MSG_LIST_STABLED_PETS`'s open, `0x4cadc0`: the open stable master's own list re-lists in place
/// (`0x4cadd6`); any other registers on the latch (`0x4cade2`), then the cell (`0x4caded`), the
/// selection's clear (`0x4cadf8`) and `PET_STABLE_SHOW` (`0x4cac9b`), on the lane, every list.
pub(crate) fn open_stable(
    world: &mut World,
    npc: u64,
    num_stable_slots: u8,
    pets: Vec<StabledPet>,
) {
    if world.resource::<StableOpen>().npc != Some(npc) {
        crate::ui_session::register(world, npc);
    }
    world
        .resource_mut::<StableOpen>()
        .open(npc, num_stable_slots, pets);
    if let Some(mut lane) = world.get_resource_mut::<StableLane>() {
        lane.push(StableEvent::Show);
    }
}

/// One wire row as a Lua slot; icon, family and diet stay `None` until the creature record lands.
/// Read as `ask` says: the getters read it ([`Ask::Read`]) and ask a miss with their callback
/// inside their calls ([`StableCreatureWaits`]).
fn resolve_pet(
    wire: &StabledPet,
    names: &NameCache,
    families: Option<&PetFamilyTables>,
    stats: Option<&PetStatTables>,
    ask: Ask<'_>,
) -> StablePetSlot {
    // Guid 0 asks for the template alone: a stabled pet has no spawn.
    let _ = names.resolve_creature(wire.creature_entry, 0, ask);
    let family_id = names
        .creature_record(wire.creature_entry)
        .map(|r| r.pet_family)
        .unwrap_or(0);
    let family_row = families.and_then(|t| t.families.get(family_id));
    StablePetSlot {
        pet_number: wire.pet_number,
        entry: wire.creature_entry,
        // The template's display, which `SetPetStablePaperdoll` shows (`0x4cba0e`).
        display_id: names
            .creature_record(wire.creature_entry)
            .map(|r| r.display_id),
        // The family's `CreatureFamily.dbc` icon; `SetItemButtonTexture` draws `None` as empty.
        icon: families
            .and_then(|t| t.families.icon(family_id))
            .map(str::to_string),
        // The given name from the wire, not the template's.
        name: wire.name.clone(),
        level: wire.level,
        family: family_row.map(|f| f.name.clone()),
        // The loyalty level's `PetLoyalty.dbc` name, as `GetPetLoyalty` gives the live pet's.
        loyalty: stats
            .and_then(|t| t.loyalty.name(wire.loyalty))
            .map(str::to_string),
        // The food mask as diet names, as `GetPetFoodTypes` gives them; empty for a zero mask.
        diet: family_row
            .map(|f| {
                families
                    .map(|t| {
                        t.foods
                            .for_mask(f.pet_food_mask)
                            .into_iter()
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default(),
    }
}

/// The stable as the getters read it, a closed one's rows and count included: the getters test
/// no cell.
fn snapshot(
    open: &StableOpen,
    names: &NameCache,
    families: Option<&PetFamilyTables>,
    stats: Option<&PetStatTables>,
    next_slot_cost: u32,
    has_live_pet: bool,
    ask: Ask<'_>,
) -> Option<StableState> {
    let mut slots: [Option<StablePetSlot>; NUM_STABLE_SLOTS] = Default::default();
    // The summoned pet (wire slot byte 1, bit 1 clear) takes row 0; the stabled ones (bit 1 set)
    // pack behind it in list order, whatever their slot byte says, so with only the second slot
    // occupied (the server does not renumber) the pet shows in box 1 and `UnstablePet(1)` takes it
    // (`0x4cab10`-`0x4cac95`, the array `0xb720c8`; `0x4cb285`-`0x4cb298`, `0x4cafa5`-`0x4cafb1`).
    let mut next = 1;
    for wire in &open.pets {
        let seat = if wire.is_summoned() {
            0
        } else {
            next += 1;
            next - 1
        };
        match slots.get_mut(seat) {
            Some(seat) => *seat = Some(resolve_pet(wire, names, families, stats, ask)),
            None => debug!(
                "ui_stable: pet {} arrived past the {NUM_STABLE_SLOTS} rows the window has, dropped",
                wire.pet_number
            ),
        }
    }
    Some(StableState {
        num_stable_slots: u32::from(open.num_stable_slots),
        next_slot_cost,
        slots,
        has_live_pet,
    })
}

/// What the stable verbs read and send through: the engine half of the stable calls
/// ([`crate::script_calls::ScriptCallHost`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct StableSteps<'w, 's> {
    /// The stored master; absent in a world with no stable window, which stores none.
    open: Option<ResMut<'w, StableOpen>>,
    commands: Res<'w, NetCommands>,
    self_q: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
}

impl StableSteps<'_, '_> {
    /// The stable master's cell `[0xb72240]`.
    pub(crate) fn master(&self) -> Option<u64> {
        self.open.as_deref().and_then(|o| o.npc)
    }

    /// The close core's clear of the cell (`0x4cae29`).
    pub(crate) fn clear_master(&mut self) {
        if let Some(open) = self.open.as_deref_mut() {
            open.close_cell();
        }
    }

    /// One stable verb inside its call, sent with the stored master's guid `[0xb72240]`.
    /// `StablePet` and `BuyStableSlot` send only while one is stored (`0x4cae56`, `0x4cb0ae`); a
    /// drop and `UnstablePet` send without testing it, 0 when none (`0x4cb4e9`, `0x4cb57a`,
    /// `0x4cb623`, `0x4cb693`, `0x4cb001`).
    pub(crate) fn apply(&mut self, call: StableCall) {
        let master = self.open.as_ref().and_then(|o| o.npc);
        let (npc, intent) = match call {
            StableCall::Request(intent) => {
                let Some(npc) = master else {
                    debug!("ui_stable: {intent:?} with no stable master stored, not sent");
                    return;
                };
                (npc, intent)
            }
            StableCall::Drop(intent) => (master.unwrap_or(0), intent),
            // `UnstablePet`'s own gate: the active player held (`0x4caf6d`), with neither a charm
            // nor a summon (`0x4caf73`-`0x4caf88`).
            StableCall::Unstable(pet_number) => {
                let free = self
                    .self_q
                    .iter()
                    .next()
                    .is_some_and(|me| me.0.unit_pet_guid().is_none());
                if !free {
                    debug!("ui_stable: unstable refused, a charm or a summon is out");
                    return;
                }
                (master.unwrap_or(0), StableIntent::Unstable(pet_number))
            }
            // The close core needs the latch, which the host lends beside this.
            StableCall::Close => return,
        };
        debug!("ui_stable: {intent:?} (stable master {npc:#x})");
        let command = match intent {
            StableIntent::Stable => ClientCommand::StablePet { npc },
            StableIntent::Unstable(pet_number) => ClientCommand::UnstablePet { npc, pet_number },
            StableIntent::Swap(pet_number) => ClientCommand::StableSwapPet { npc, pet_number },
            StableIntent::BuySlot => ClientCommand::BuyStableSlot { npc },
        };
        let _ = self.commands.0.send(command);
    }
}

/// The window's creature callbacks the getters' lookups kept, one per call that missed (flag 0,
/// `0x4cb2b5`): `0x4cb3b0`, or with `paperdoll` `0x4cba50`, by the entry they wait on. The cache
/// runs them when the record's answer lands, a refusal too, and they outlive the window
/// (`0x556e20`).
#[derive(Resource, Default)]
pub(crate) struct StableCreatureWaits(Vec<(u32, bool)>);

/// A stable getter's creature lookup with its callback, inside its call
/// ([`benilla_ui::script::RecordAsk::StableCreature`]): the creature cache asks a miss, owned by
/// the active player (`0x468550`), and keeps the callback; a held record keeps nothing.
pub(crate) fn ask_creature(
    names: &NameCache,
    commands: &NetCommands,
    waits: &mut StableCreatureWaits,
    owner: u64,
    entry: u32,
    paperdoll: bool,
) {
    if entry == 0 || names.creature_record(entry).is_some() {
        return;
    }
    let _ = names.resolve_creature(entry, owner, Ask::Event(commands));
    waits.0.push((entry, paperdoll));
}

impl StableCreatureWaits {
    /// The record of `entry` answered, `found` or refused: each callback waiting on it runs and
    /// goes, in the order kept, and a found record's signals `PET_STABLE_UPDATE` (`0x4cb3bf`) or
    /// `PET_STABLE_UPDATE_PAPERDOLL` (`0x4cba5f`), on the lane; a refusal's signal nothing.
    fn answered(&mut self, entry: u32, found: bool, lane: &mut StableLane) {
        self.0.retain(|&(waits_on, paperdoll)| {
            if waits_on != entry {
                return true;
            }
            if found {
                lane.push(if paperdoll {
                    StableEvent::Paperdoll
                } else {
                    StableEvent::Update
                });
            }
            false
        });
    }
}

/// `StableSlotPrices.dbc`, the purchase row's prices; absent, the row quotes 0.
#[derive(Resource)]
pub(crate) struct StableSlotPrices(pub(crate) benilla_formats::StableSlotPrices);

#[cfg(test)]
mod tests {
    use super::*;

    /// The range guard cannot close it: after the drop there is no self player to measure from.
    #[test]
    fn the_session_end_closes_the_stable() {
        let mut app = App::new();
        app.init_resource::<StableOpen>()
            .init_resource::<StableLane>()
            .init_resource::<StableCreatureWaits>()
            .init_resource::<crate::names::NameCache>();
        net::register(&mut app);
        {
            let mut open = app.world_mut().resource_mut::<StableOpen>();
            open.npc = Some(0xF130_0000_0000_0042);
            open.num_stable_slots = 2;
        }

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        let open = app.world().resource::<StableOpen>();
        assert_eq!((open.npc, open.num_stable_slots), (None, 0));
    }
}
