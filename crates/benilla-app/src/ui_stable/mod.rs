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
use crate::query_cache::Ask;
use crate::ui_pet_stats::{PetFamilyTables, PetStatTables};
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{CoreCloses, LatchApp, LatchedWindow, NpcWindow};

mod net;

pub(crate) struct UiStablePlugin;

impl Plugin for UiStablePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<StableOpen>()
            .init_resource::<StableErrors>()
            .add_latched_window::<StableOpen>()
            // `0x490b2a` → `0x4caae0` → `0x4cae10`, the core.
            .add_sweep_leg(SweepLeg::Stable, close_core)
            .add_systems(
                Update,
                (
                    feed_stable.in_set(UiFeed),
                    drain_stable.in_set(crate::ui_script::UiDrain),
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

/// The stable master's cell `[0xb72240]`; `ClosePetStables` (`0x4cae40`) is the core itself. The
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

    fn take_close_verb(script: &mut UiScript) -> bool {
        script.take_stable_close()
    }
}

/// The close core `0x4cae10`: with a stable open, the latch release (`0x4cae24`), the cell
/// cleared, then `PET_STABLE_CLOSED` (`0x4cae3a`).
pub(crate) fn close_core(world: &mut World) {
    let Some(npc) = world.resource::<StableOpen>().npc else {
        return;
    };
    crate::ui_session::release(world, npc);
    world.resource_mut::<StableOpen>().clear();
    crate::ui_session::note_closed(world, NpcWindow::Stable);
    crate::ui_session::fire(world, |script| {
        script.set_stable(None);
        script.fire_event("PET_STABLE_CLOSED", vec![]);
    });
}

/// `MSG_LIST_STABLED_PETS`'s open, `0x4cadc0`: the open stable master's own list re-lists in place
/// (`0x4cadd6`); any other registers on the latch (`0x4cade2`), then the cell (`0x4caded`).
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
}

/// One wire row as a Lua slot; icon, family and diet stay `None` until the creature query answers.
/// Read as `ask` says: `GetStablePetInfo` looks the template up with a callback (`0x4cb2ce`).
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

fn snapshot(
    open: &StableOpen,
    names: &NameCache,
    families: Option<&PetFamilyTables>,
    stats: Option<&PetStatTables>,
    next_slot_cost: u32,
    has_live_pet: bool,
    ask: Ask<'_>,
) -> Option<StableState> {
    open.npc?;
    let mut slots: [Option<StablePetSlot>; NUM_STABLE_SLOTS] = Default::default();
    for wire in &open.pets {
        // Seat rows by their slot, not position: a petless hunter's list has no slot-0 row.
        match slots.get_mut(usize::from(wire.slot)) {
            Some(seat) => *seat = Some(resolve_pet(wire, names, families, stats, ask)),
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

/// Push the stable into the VM and fire its events on a change; the close is the core's
/// ([`close_core`]).
fn feed_stable(
    mut vm: crate::script_calls::VmWith<Res<'static, crate::ui_pet::PetBar>>,
    // Mutable only to take the fresh-list latch.
    mut open: ResMut<StableOpen>,
    pet: crate::ui_pet::PetUnit,
    families: Option<Res<PetFamilyTables>>,
    stats: Option<Res<PetStatTables>>,
    prices: Option<Res<StableSlotPrices>>,
    commands: Res<NetCommands>,
    names: Res<NameCache>,
    mut errors: ResMut<StableErrors>,
    mut last: crate::ui_script::WindowMemo<Option<StableState>>,
    mut closes: CoreCloses,
    mut sink: crate::ui_action::MessageSink,
    ticks: bevy::ecs::system::SystemChangeTick,
) {
    // A refusal's line shows inside `SMSG_STABLE_RESULT`'s handler (`0x4cacb0`), so its handlers
    // read the stable the packets before it left: one a later packet's write to the stable follows
    // shows ahead of the frame's push, the rest after it and its events.
    let (listed, now) = (open.last_changed(), ticks.this_run());
    let ahead = errors
        .0
        .iter()
        .take_while(|(_, at)| listed.is_newer_than(*at, now))
        .count();
    let behind = errors.0.split_off(ahead);
    if !errors.0.is_empty() {
        vm.run(|script| show_refusals(script, &mut errors.0, &mut sink));
    }
    vm.feed(|script, bar, fires| {
        let Some(script) = script else {
            return;
        };
        let last = last.get(script);
        if closes.since_last(script, NpcWindow::Stable) {
            *last = None;
        }

        // The next slot's price: `StableSlotPrices.dbc` row `purchased + 1`, 0 past the table.
        let next_slot_cost = prices
            .as_deref()
            .and_then(|p| p.0.next_slot_price(open.num_stable_slots))
            .unwrap_or(0);

        // The reference's live-pet test, `[0xb714a0]|[0xb714a4]`; a dismissed pet fails it.
        let has_live_pet = bar.spells.pet_guid != 0;
        // The summoned pet's cache record while its object is held, `[unit+0xb30]` (`0x60b131`):
        // the template's display, which `0x505a70` reads at `[rec+0x2c]`. Apart from the snapshot,
        // so it fires nothing.
        let live_pet_display = has_live_pet
            .then(|| pet.store(bar.spells.pet_guid))
            .flatten()
            .and_then(|s| s.0.object_entry())
            .and_then(|entry| names.creature_record(entry))
            .map(|r| r.display_id);
        script.set_stable_live_pet_display(live_pet_display);
        // Every list fires `PET_STABLE_SHOW`, on which the stock window calls its getters.
        let fresh_list = std::mem::take(&mut open.fresh_list);
        let fresh = snapshot(
            &open,
            &names,
            families.as_deref(),
            stats.as_deref(),
            next_slot_cost,
            has_live_pet,
            Ask::at(fresh_list || last.is_none(), &commands),
        );
        if fresh == *last && !fresh_list {
            return;
        }
        // The push before the posts: the handlers run when they fire, after the turn, and read it.
        script.set_stable(fresh.clone());
        if fresh_list && fresh.is_some() {
            // Every list fires `PET_STABLE_SHOW` (`0x4cac9b`); on the visible window it only
            // repaints.
            fires.post("PET_STABLE_SHOW", vec![]);
        } else if last.is_some() && fresh.is_some() {
            // A change with no new list is a creature-query answer, the reference's callback edge.
            // A close is the core's, which told the VM, and a disconnect's clear tells it nothing;
            // a first snapshot always comes with the list latch.
            fires.post("PET_STABLE_UPDATE", vec![]);
            fires.post("PET_STABLE_UPDATE_PAPERDOLL", vec![]);
        }
        *last = fresh;
    });
    errors.0.extend(behind);
    if !errors.0.is_empty() {
        vm.run(|script| show_refusals(script, &mut errors.0, &mut sink));
    }
}

/// Show `refusals`, in a run with the host lent, each as its message row says, speech included;
/// with no VM up they wait.
fn show_refusals(
    script: Option<&mut UiScript>,
    refusals: &mut Vec<(&'static str, bevy::ecs::change_detection::Tick)>,
    sink: &mut crate::ui_action::MessageSink,
) {
    let Some(script) = script else {
        return;
    };
    let lines: Vec<_> = refusals
        .drain(..)
        .filter_map(|(key, _)| {
            let text = script.lua().globals().get::<String>(key).ok()?;
            (!text.is_empty()).then(|| crate::ui_action::Shown::keyed(key, text))
        })
        .collect();
    crate::ui_action::show_messages(script, sink, "ui_stable", lines);
}

/// Drain the Lua intents into the four stable verbs, each aimed at the open session's NPC; a close
/// runs the core, as there is no close opcode.
fn drain_stable(
    mut hosted_vm: crate::script_calls::HostedVm,
    open: Res<StableOpen>,
    commands: Res<NetCommands>,
    mut world_commands: Commands,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
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
                StableIntent::Unstable(pet_number) => {
                    ClientCommand::UnstablePet { npc, pet_number }
                }
                StableIntent::Swap(pet_number) => ClientCommand::StableSwapPet { npc, pet_number },
                StableIntent::BuySlot => ClientCommand::BuyStableSlot { npc },
            };
            let _ = commands.0.send(command);
        }
        if script.take_stable_close() {
            debug!("ui_stable: client-side close (no packet)");
            crate::ui_session::queue(&mut world_commands, close_core);
        }
    })
}

/// Stable refusals as message keys: only `ERR_NOT_ENOUGH_MONEY`, `DisplayError(0x25)` in the
/// `SMSG_STABLE_RESULT` handler `0x4cacb0`, whose row also speaks (line `0x28`). Each carries the
/// change tick of the handler call that queued it, its place on the wire: each call is its own
/// system run, so a later packet's writes carry newer ticks.
#[derive(Resource, Default)]
pub(crate) struct StableErrors(pub(crate) Vec<(&'static str, bevy::ecs::change_detection::Tick)>);

/// `StableSlotPrices.dbc`, the purchase row's prices; absent, the row quotes 0.
#[derive(Resource)]
pub(crate) struct StableSlotPrices(pub(crate) benilla_formats::StableSlotPrices);

#[cfg(test)]
mod tests {
    use super::*;

    /// Every list fires `PET_STABLE_SHOW`, on which the stock window calls `GetStablePetInfo`,
    /// which looks the template up with a callback (`0x4cb2ce`); the frames between read. A
    /// refused template is asked by no frame, and again by the next list.
    #[test]
    fn a_refused_stabled_pet_is_asked_at_the_next_list_never_by_a_frame() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<StableOpen>()
            .init_resource::<StableErrors>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_systems(Update, feed_stable);
        app.insert_non_send_resource(UiScript::new().unwrap());
        let list = |app: &mut App| {
            app.world_mut().resource_mut::<StableOpen>().open(
                0xF130_0000_4500_0001,
                1,
                vec![StabledPet {
                    pet_number: 3,
                    creature_entry: 1_126,
                    level: 10,
                    name: "Boar".into(),
                    loyalty: 1,
                    slot: 1,
                }],
            );
        };
        list(&mut app);
        app.update();
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        app.world_mut()
            .resource_mut::<crate::names::NameCache>()
            .insert_creature(1_126, None);
        for _ in 0..600 {
            app.update();
        }
        assert!(crate::query_cache::queries(&rx).is_empty());
        list(&mut app);
        app.update();
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
    }

    /// A stable open on its master with no slot bought, pushed, whose VM notes the slot count
    /// at each `UI_ERROR_MESSAGE`; the stable's handlers in the table.
    fn refusal_app() -> App {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<StableOpen>()
            .init_resource::<StableErrors>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_systems(Update, feed_stable);
        net::register(&mut app);
        let script = UiScript::new().unwrap();
        script
            .run(
                "ERR_NOT_ENOUGH_MONEY = 'Poor.' SEEN = '' local f = CreateFrame('Frame') \
                 f:RegisterEvent('UI_ERROR_MESSAGE') \
                 f:SetScript('OnEvent', function() SEEN = SEEN .. GetNumStableSlots() end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.world_mut()
            .resource_mut::<StableOpen>()
            .open(MASTER, 0, Vec::new());
        app.update();
        app
    }

    const MASTER: u64 = 0xF130_0000_4500_0001;

    /// The refusal and a list showing two slots bought, the list first when `list_first`, as one
    /// frame's packets; then the count the refusal's handler read and the count after the frame.
    fn refusal_then(list_first: bool) -> (String, i64) {
        use benilla_protocol::messages::stable_result;
        use benilla_protocol::SessionEvent;
        let mut app = refusal_app();
        let refusal = SessionEvent::StableResult {
            result: stable_result::ERR_MONEY,
        };
        let list = SessionEvent::ListStabledPets {
            npc: MASTER,
            num_stable_slots: 2,
            pets: Vec::new(),
        };
        let events = if list_first {
            vec![list, refusal]
        } else {
            vec![refusal, list]
        };
        crate::net::handlers::dispatch(app.world_mut(), events);
        app.update();
        let s = app.world().non_send_resource::<UiScript>();
        (
            s.eval::<String>("return SEEN").unwrap(),
            s.eval::<i64>("return GetNumStableSlots()").unwrap(),
        )
    }

    /// The refusal's line shows in `SMSG_STABLE_RESULT`'s handler (`0x4cacb0`,
    /// `DisplayError(0x25)`), so its handlers read the stable as the packets before it left it: a
    /// list after it is not there yet, and a list before it is.
    #[test]
    fn a_refusals_handlers_read_the_stable_the_packets_before_it_left() {
        assert_eq!(refusal_then(false), ("0".into(), 2), "the list came after");
        assert_eq!(refusal_then(true), ("2".into(), 2), "the list came first");
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
