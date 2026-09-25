//! The stable window's app side: [`StableOpen`] holds the `MSG_LIST_STABLED_PETS` rows, which the
//! server sends unprompted for the gossip stable option, [`feed_stable`] pushes them and
//! [`drain_stable`] sends the four stable verbs.
//!
//! A row's family, and with it the icon and diet, is not on the wire: it comes from the creature
//! query, so those fill in a frame or two after the name and level, as on a reference cache miss.
//! Slot 0 is read from the wire like every row, because a dismissed pet still has one; stock
//! `PetStable.lua:123-138` prefers the live pet and falls back to it.

use benilla_protocol::messages::StabledPet;
use bevy::prelude::*;

use benilla_ui::script::{StableIntent, StablePetSlot, StableState, UiScript, NUM_STABLE_SLOTS};

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_pet_stats::{PetFamilyTables, PetStatTables};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, npc_switched, NpcSession};

mod net;

pub(crate) struct UiStablePlugin;

impl Plugin for UiStablePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<StableOpen>()
            .init_resource::<StableErrors>()
            .add_systems(
                Update,
                (
                    // Range-close first so the clear fires `PET_STABLE_CLOSED` the same frame.
                    close_npc_session_out_of_range::<StableOpen>.before(feed_stable),
                    feed_stable.in_set(UiFeed),
                    drain_stable.after(UiInput),
                    // After the VM ticks: the selection is written by a click during `UiInput`
                    // and by `PetStable_Update`'s re-pick (`PetStable.lua:48-63`) inside the
                    // feed's events, so an earlier read shows the previous pet for a frame.
                    feed_stable_booth
                        .after(UiInput)
                        // Gated: the feed starts with a name lookup over every UI frame.
                        .run_if(
                            |open: Res<StableOpen>, booth: Res<crate::portrait::StableBooth>| {
                                // One more run after the close, which empties the booth.
                                open.npc.is_some()
                                    || booth.unit.is_some()
                                    || booth.display_id.is_some()
                            },
                        ),
                ),
            );
    }
}

/// The open stable: its master and the list's rows as the wire gave them, until a close or a
/// disconnect.
#[derive(Resource, Default)]
pub(crate) struct StableOpen {
    pub(crate) npc: Option<u64>,
    /// Slots bought (0..=2), not slots occupied.
    pub(crate) num_stable_slots: u8,
    /// The wire rows, each with its client slot index.
    pub(crate) pets: Vec<StabledPet>,
    /// A list landed and `PET_STABLE_SHOW` is owed: the reference fires it after every list
    /// (`0x4cac9b`), and `PET_STABLE_UPDATE`/`_PAPERDOLL` only from its creature-cache callbacks
    /// (`0x4cb3bf`, `0x4cba5f`).
    pub(crate) fresh_list: bool,
}

impl StableOpen {
    pub(crate) fn open(&mut self, npc: u64, num_stable_slots: u8, pets: Vec<StabledPet>) {
        self.npc = Some(npc);
        self.num_stable_slots = num_stable_slots;
        self.pets = pets;
        self.fresh_list = true;
    }

    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.num_stable_slots = 0;
        self.pets.clear();
        self.fresh_list = false;
    }
}

/// The range guard closes the stable when its master is out of range or gone; the server checks
/// range on every verb (`CheckStableMaster`), failing with code 6, which shows nothing.
impl NpcSession for StableOpen {
    fn npc(&self) -> Option<u64> {
        self.npc
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// One wire row as a Lua slot; icon, family and diet stay `None` until the creature query answers.
fn resolve_pet(
    wire: &StabledPet,
    names: &NameCache,
    families: Option<&PetFamilyTables>,
    stats: Option<&PetStatTables>,
    commands: &NetCommands,
) -> StablePetSlot {
    // Guid 0 asks for the template alone: a stabled pet has no spawn.
    let _ = names.resolve_creature(wire.creature_entry, 0, commands);
    let family_id = names
        .creature_record(wire.creature_entry)
        .map(|r| r.pet_family)
        .unwrap_or(0);
    let family_row = families.and_then(|t| t.families.get(family_id));
    StablePetSlot {
        pet_number: wire.pet_number,
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

fn snapshot(
    open: &StableOpen,
    names: &NameCache,
    families: Option<&PetFamilyTables>,
    stats: Option<&PetStatTables>,
    next_slot_cost: u32,
    has_live_pet: bool,
    commands: &NetCommands,
) -> Option<StableState> {
    open.npc?;
    let mut slots: [Option<StablePetSlot>; NUM_STABLE_SLOTS] = Default::default();
    for wire in &open.pets {
        // Seat rows by their slot, not position: a petless hunter's list has no slot-0 row.
        match slots.get_mut(usize::from(wire.slot)) {
            Some(seat) => *seat = Some(resolve_pet(wire, names, families, stats, commands)),
            None => debug!(
                "ui_stable: pet {} arrived in slot {} — past the {NUM_STABLE_SLOTS} the window has, dropped",
                wire.pet_number, wire.slot
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

/// Push the stable into the VM and fire its events on a change.
fn feed_stable(
    script: Option<NonSendMut<UiScript>>,
    // Mutable only to take the fresh-list latch.
    mut open: ResMut<StableOpen>,
    bar: Res<crate::ui_pet::PetBar>,
    families: Option<Res<PetFamilyTables>>,
    stats: Option<Res<PetStatTables>>,
    prices: Option<Res<StableSlotPrices>>,
    commands: Res<NetCommands>,
    names: Res<NameCache>,
    mut errors: ResMut<StableErrors>,
    mut last: Local<crate::ui_script::VmMemo<Option<StableState>>>,
    mut last_npc: Local<crate::ui_script::VmMemo<Option<u64>>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_npc = last_npc.get(&script);
    // Refusals show, and speak, as their message rows say.
    let lines: Vec<_> = errors
        .0
        .drain(..)
        .filter_map(|key| {
            let text = script.lua().globals().get::<String>(key).ok()?;
            (!text.is_empty()).then(|| crate::ui_action::Shown::keyed(key, text))
        })
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_stable", lines);

    // The next slot's price: `StableSlotPrices.dbc` row `purchased + 1`, 0 past the table.
    let next_slot_cost = prices
        .as_deref()
        .and_then(|p| p.0.next_slot_price(open.num_stable_slots))
        .unwrap_or(0);

    // The reference's live-pet test, `[0xb714a0]|[0xb714a4]`; a dismissed pet fails it.
    let has_live_pet = bar.spells.pet_guid != 0;
    let fresh = snapshot(
        &open,
        &names,
        families.as_deref(),
        stats.as_deref(),
        next_slot_cost,
        has_live_pet,
        &commands,
    );
    let switched = npc_switched(*last_npc, open.npc);
    let fresh_list = std::mem::take(&mut open.fresh_list);
    if fresh == *last && !switched && !fresh_list {
        return;
    }
    // Push before firing: `fire_event` runs the Lua handlers synchronously.
    script.set_stable(fresh.clone());
    if switched {
        // A new stable master while open is a close then an open; consume the close intent
        // OnHide queues so the drain keeps the new stable.
        script.fire_event("PET_STABLE_CLOSED", vec![]);
        script.fire_event("PET_STABLE_SHOW", vec![]);
        let _ = script.take_stable_close();
    } else if fresh_list && fresh.is_some() {
        // Every list fires `PET_STABLE_SHOW` (`0x4cac9b`); on the visible window it only repaints.
        script.fire_event("PET_STABLE_SHOW", vec![]);
    } else {
        match (&*last, &fresh) {
            // A change with no new list is a creature-query answer, the reference's callback edge.
            (Some(_), Some(_)) => {
                script.fire_event("PET_STABLE_UPDATE", vec![]);
                script.fire_event("PET_STABLE_UPDATE_PAPERDOLL", vec![]);
            }
            (Some(_), None) => script.fire_event("PET_STABLE_CLOSED", vec![]),
            // A first snapshot always comes with the list latch.
            (None, _) => {}
        }
    }
    *last = fresh;
    *last_npc = open.npc;
}

/// Drain the Lua intents into the four stable verbs, each aimed at the open session's NPC; a close
/// clears locally, as there is no close opcode.
fn drain_stable(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<StableOpen>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for intent in script.take_stable_intents() {
        let Some(npc) = open.npc else {
            debug!("ui_stable: {intent:?} with no open stable — ignored");
            continue;
        };
        debug!("ui_stable: {intent:?} (stable master {npc:#x})");
        let command = match intent {
            StableIntent::Stable => ClientCommand::StablePet { npc },
            StableIntent::Unstable(pet_number) => ClientCommand::UnstablePet { npc, pet_number },
            StableIntent::Swap(pet_number) => ClientCommand::StableSwapPet { npc, pet_number },
            StableIntent::BuySlot => ClientCommand::BuyStableSlot { npc },
        };
        let _ = commands.0.send(command);
    }
    if script.take_stable_close() {
        debug!("ui_stable: client-side close (no packet)");
        open.clear();
    }
}

/// Point the model pane at the pet `GetSelectedStablePet()` names, as the reference's
/// `SetPetStablePaperdoll` (`0x4cb870`) does: the summoned pet (`[0xb72250] == -1`) shows its
/// live body (`[unit+0xb30]`), and anything else, or a failed live resolve (`0x4cb9bc`), shows the
/// row's creature display, whose late cache answer re-points the pane.
fn feed_stable_booth(
    script: Option<NonSendMut<UiScript>>,
    open: Res<StableOpen>,
    bar: Res<crate::ui_pet::PetBar>,
    pet: crate::ui_pet::PetUnit,
    names: Res<NameCache>,
    mut booth: ResMut<crate::portrait::StableBooth>,
) {
    let Some(mut script) = script else {
        return;
    };
    // Every frame, so a stale yaw cannot snap the model when a pet is picked.
    booth.yaw = script.model_pane_facing("PetStableModel");

    // `GetSelectedStablePet()`: 0 the summoned pet, 1..=2 a stable slot, -1 nothing, including a
    // pet the latest list no longer holds.
    let slot = script.stable_selection();

    let live_pet = bar.spells.pet_guid;
    let (unit, display_id) = stable_subject(
        slot,
        (live_pet != 0).then(|| pet.entity(live_pet)).flatten(),
        &open.pets,
        |entry| names.creature_record(entry).map(|r| r.display_id),
    );
    booth.unit = unit;
    booth.display_id = display_id;
}

/// The selection's `(live unit, display id)`. The live arm reads the pet guid at `[0xb714a0]` and
/// no row; the display is resolved too, as the reference falls through to it when the live
/// resolve fails. Display 0 reads as no answer.
fn stable_subject(
    slot: i32,
    live_pet: Option<Entity>,
    pets: &[StabledPet],
    display_of: impl Fn(u32) -> Option<u32>,
) -> (Option<Entity>, Option<u32>) {
    let unit = if slot == 0 { live_pet } else { None };
    let display = u8::try_from(slot)
        .ok()
        .and_then(|slot| pets.iter().find(|p| p.slot == slot))
        .and_then(|row| display_of(row.creature_entry))
        .filter(|&d| d != 0);
    (unit, display)
}

/// Stable refusals as message keys: only `ERR_NOT_ENOUGH_MONEY`, `DisplayError(0x25)` in the
/// `SMSG_STABLE_RESULT` handler `0x4cacb0`, whose row also speaks (line `0x28`).
#[derive(Resource, Default)]
pub(crate) struct StableErrors(pub(crate) Vec<&'static str>);

/// `StableSlotPrices.dbc`, the purchase row's prices; absent, the row quotes 0.
#[derive(Resource)]
pub(crate) struct StableSlotPrices(pub(crate) benilla_formats::StableSlotPrices);

#[cfg(test)]
mod tests {
    use super::*;

    fn row(slot: u8, entry: u32) -> StabledPet {
        StabledPet {
            pet_number: u32::from(slot) + 100,
            creature_entry: entry,
            level: 30,
            name: format!("pet{slot}"),
            loyalty: 6,
            slot,
        }
    }

    /// One display per entry; entry `9` is the template that ships none.
    fn display_of(entry: u32) -> Option<u32> {
        match entry {
            1 => Some(4449),
            2 => Some(822),
            9 => Some(0),
            _ => None,
        }
    }

    #[test]
    fn the_selection_forks_between_a_live_body_and_a_bare_display() {
        let pets = [row(0, 1), row(1, 2)];
        let live = Entity::from_raw_u32(7).expect("a test entity id");

        // Nothing selected: neither source, so the pane empties.
        assert_eq!(
            stable_subject(-1, Some(live), &pets, display_of),
            (None, None)
        );

        // Slot 0 with the pet out: the body, and the display beside it so a dismiss keeps the pane.
        assert_eq!(
            stable_subject(0, Some(live), &pets, display_of),
            (Some(live), Some(4449))
        );

        // Slot 0 with the pet dismissed: the display alone.
        assert_eq!(
            stable_subject(0, None, &pets, display_of),
            (None, Some(4449))
        );

        // A stabled slot never takes the live body: the live arm is gated on the selection.
        assert_eq!(
            stable_subject(1, Some(live), &pets, display_of),
            (None, Some(822))
        );

        // A slot with no row (never bought, or empty) and a slot past the window: nothing.
        assert_eq!(
            stable_subject(2, Some(live), &pets, display_of),
            (None, None)
        );
    }

    #[test]
    fn an_unanswered_or_display_less_template_empties_the_pane() {
        assert_eq!(
            stable_subject(1, None, &[row(1, 404)], display_of),
            (None, None),
            "the query has not landed yet"
        );
        assert_eq!(
            stable_subject(1, None, &[row(1, 9)], display_of),
            (None, None),
            "the template ships no display"
        );
    }

    /// The range guard cannot close it: after the drop there is no self player to measure from.
    #[test]
    fn the_session_end_closes_the_stable() {
        let mut app = App::new();
        app.init_resource::<StableOpen>()
            .init_resource::<StableErrors>()
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
