//! The app-side **stable feed** (decision 1676) — `PetStableFrame`'s data, the inward half of the
//! stable seam around [`benilla_ui::script`]'s `stable` module, and the twin of
//! [`crate::ui_trainer`]'s trainer feed.
//!
//! The net bridge fills [`StableOpen`] from the wire (`MSG_LIST_STABLED_PETS`, which the server
//! sends unprompted when the gossip stable option is chosen — the trainer's and the vendor's
//! arrangement exactly). Each frame [`feed_stable`] resolves each wire [`StabledPet`] into a
//! Lua-facing [`StablePetSlot`], pushes the snapshot, and fires `PET_STABLE_SHOW` on open /
//! `PET_STABLE_UPDATE` on a content change / `PET_STABLE_CLOSED` on clear. [`drain_stable`] pulls
//! the intents back out into the four wire verbs.
//!
//! ## The resolve, and the one round trip it needs
//!
//! The wire names a `creature_template` **entry** and a loyalty **level**; the window wants an
//! icon, a family word, a loyalty name and a diet. Three of those come from tables already loaded
//! for the live pet ([`PetFamilyTables`], [`PetStatTables`] — decisions 1062/1005); the fourth,
//! the family *id*, is not on the wire at all and must come from the creature query
//! ([`NameCache::resolve_creature`], ask-once).
//!
//! **So a stabled pet's row fills in two steps**, and that is not a defect to paper over: the name
//! and level render immediately off the wire, and the icon/family/diet appear when the template
//! answer lands a frame or two later. The feed diffs, so the arrival re-pushes and fires
//! `PET_STABLE_UPDATE` on its own. The reference has the same two-step (its own template cache
//! misses the first time too); what it does not have is our *empty* first step, because
//! `creaturecache.wdb` persists across sessions and ours does not yet.
//!
//! ## Why the current pet's row is read from the wire, not from the live pet
//!
//! Slot 0 can be occupied while `UnitExists("pet")` is false — a dismissed pet, or one left too far
//! away to be summoned, still gets a row from the server's character-pet cache. The reference
//! prefers the live unit and falls back to `GetStablePetInfo(0)` (`PetStable.lua:120-146`), so both
//! sources must be right; this feed fills the wire half for every row uniformly and lets the Lua
//! choose, rather than special-casing slot 0 into a live-unit read that would be blank for exactly
//! the hunter who needs the window most.

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
                    // Range-close before the feed so the clear becomes PET_STABLE_CLOSED the same
                    // frame; push before the input pass so an open/close is on screen the same frame;
                    // drain after it (the trainer/merchant/gossip ordering).
                    close_npc_session_out_of_range::<StableOpen>.before(feed_stable),
                    feed_stable.in_set(UiFeed),
                    drain_stable.after(UiInput),
                    // AFTER the VM ticks, unlike every other booth feed: this one reads the
                    // *selection*, which a click on a slot writes during `UiInput` — and
                    // `PetStable_Update`'s own re-pick (`PetStable.lua:44-59`) runs inside the
                    // event dispatch `feed_stable` performs. Reading it before the tick would show
                    // the previous pet for a frame after every click.
                    feed_stable_booth
                        .after(UiInput)
                        // Nothing samples the booth while no stable is open, and the feed's
                        // first line is a name scan over the whole frame arena (1979's floor).
                        .run_if(
                            |open: Res<StableOpen>, booth: Res<crate::portrait::StableBooth>| {
                                // …and one more run after the close: the feed is also what
                                // EMPTIES the booth (unit/display_id → None), and a gate that
                                // shut on the close frame left the pet standing in a closed
                                // window's pane for good (review of 2026-09-04).
                                open.npc.is_some()
                                    || booth.unit.is_some()
                                    || booth.display_id.is_some()
                            },
                        ),
                ),
            );
    }
}

/// The open stable, filled by the net bridge ([`crate::net`]) and read by [`feed_stable`]. Holds
/// the stable master's guid and the pet rows exactly as the wire delivered them. Cleared on a
/// client-side close and on disconnect.
#[derive(Resource, Default)]
pub(crate) struct StableOpen {
    /// The stable master whose window is open; `None` = no stable open.
    pub(crate) npc: Option<u64>,
    /// Stable slots **purchased** (0..=2) — the wire's `numStableSlots`, not a count of occupied
    /// ones.
    pub(crate) num_stable_slots: u8,
    /// The wire rows, each already carrying its rebased client slot index.
    pub(crate) pets: Vec<StabledPet>,
    /// A list packet has landed and the feed has not yet turned it into `PET_STABLE_SHOW`.
    ///
    /// The distinction this latch draws is the client's: `PET_STABLE_SHOW` fires **unconditionally
    /// at the tail of every `MSG_LIST_STABLED_PETS` handler** (`0x4cac9b`) — every list, not just
    /// the first — while `PET_STABLE_UPDATE`/`_PADERDOLL` fire from the **creature-cache
    /// callbacks** (`0x4cb3bf`, `0x4cba5f`), i.e. exactly when a late template answer fills a row
    /// in. Our two-step resolve has the same two edges, so the events land on the same causes.
    pub(crate) fresh_list: bool,
}

impl StableOpen {
    /// Open (or replace) the window with a stable master's freshly-listed pets.
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

/// The stable window is an NPC session: the standardized range guard ([`crate::ui_session`])
/// client-side-closes it when the player walks out of the stable master's service range or the NPC
/// despawns. This matters more here than for most windows — the server re-checks the interaction on
/// **every** stable verb (`CheckStableMaster`), so a window left open at a distance would fail each
/// button with an indistinguishable generic error instead of simply not being there.
impl NpcSession for StableOpen {
    fn npc(&self) -> Option<u64> {
        self.npc
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Resolve one wire row into the Lua-facing slot. The template answer drives icon/family/diet and
/// arrives asynchronously (module doc), so each of those is independently `None`-able rather than
/// gating the whole row: a pet whose query is in flight still shows its name and level.
fn resolve_pet(
    wire: &StabledPet,
    names: &NameCache,
    families: Option<&PetFamilyTables>,
    stats: Option<&PetStatTables>,
    commands: &NetCommands,
) -> StablePetSlot {
    // Ask-once for the template; `guid: 0` is the template-only convention (this pet has no spawn
    // to name — it is asleep in a stable, which is the whole point).
    let _ = names.resolve_creature(wire.creature_entry, 0, commands);
    let family_id = names
        .creature_record(wire.creature_entry)
        .map(|r| r.pet_family)
        .unwrap_or(0);
    let family_row = families.and_then(|t| t.families.get(family_id));
    StablePetSlot {
        pet_number: wire.pet_number,
        // The family's own `CreatureFamily.dbc` icon column — a pet has no item behind it, so this
        // is the only icon there is. `None` until the query lands; the reference's
        // `SetItemButtonTexture` renders the empty-slot art for it, which is right (decision 1046:
        // a path resolving to nothing would draw WHITE).
        icon: families
            .and_then(|t| t.families.icon(family_id))
            .map(str::to_string),
        // The pet's given name, straight off the wire — never the template's.
        name: wire.name.clone(),
        level: wire.level,
        family: family_row.map(|f| f.name.clone()),
        // The wire carries a loyalty LEVEL; the window shows the `PetLoyalty.dbc` name for it, the
        // same table `GetPetLoyalty` reads for the live pet (decision 1005).
        loyalty: stats
            .and_then(|t| t.loyalty.name(wire.loyalty))
            .map(str::to_string),
        // The family row's food mask expanded to localized diet names — `GetPetFoodTypes`'s law
        // (decision 1062). Legitimately empty for a zero mask; the reference guards on that.
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

/// Build the Lua-facing snapshot from [`StableOpen`] — `None` when no stable is open.
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
        // Seat each row by its OWN slot, never by position: the current-pet row is absent for a
        // petless hunter, so `pets[0]` is not slot 0 (decision 1676's decode note). A slot byte
        // past the window's three is dropped rather than panicking — the server never sends one,
        // and a malformed list must not take the window down.
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

/// Push the current stable into the VM and fire the show/update/close events on a transition (or a
/// content change). Diffed against a `Local` memory, exactly like the trainer/merchant feeds.
fn feed_stable(
    script: Option<NonSendMut<UiScript>>,
    // ResMut only to consume the fresh-list latch; the feed never authors stable content.
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
    // Refusals go to the surface — and the voice — their message record names (1815).
    let lines: Vec<_> = errors
        .0
        .drain(..)
        .filter_map(|key| {
            let text = script.lua().globals().get::<String>(key).ok()?;
            (!text.is_empty()).then(|| crate::ui_action::Shown::keyed(key, text))
        })
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_stable", lines);

    // The next slot's price is the client's own job (the wire never sends it) — `StableSlotPrices`
    // row `purchased + 1`, the bank's arrangement. `0` past the table, a state in which the
    // reference has already hidden the row that would show it.
    let next_slot_cost = prices
        .as_deref()
        .and_then(|p| p.0.next_slot_price(open.num_stable_slots))
        .unwrap_or(0);

    // The client's own `[0xb714a0]|[0xb714a4]` test — a LIVE pet guid, which a dismissed pet does
    // not have even though the server still sends its slot-0 row.
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
    // PUSH before firing: `fire_event` dispatches the Lua handlers synchronously, so the snapshot
    // must already be in the VM when `PetStable_Update` reads it back (decision 1073's rule).
    script.set_stable(fresh.clone());
    if switched {
        // A different stable master while the window is open is a real close+open — the reference's
        // ShowUIPanel early-returns when visible, so the open sound only re-plays after a hide
        // (decision 0096). Consume the close intent the CLOSED→OnHide→ClosePetStables round queues,
        // so the drain does not clear the stable we just re-opened to.
        script.fire_event("PET_STABLE_CLOSED", vec![]);
        script.fire_event("PET_STABLE_SHOW", vec![]);
        let _ = script.take_stable_close();
    } else if fresh_list && fresh.is_some() {
        // **Every list fires SHOW**, not just the first (`0x4cac9b` is unconditional). The window
        // is already visible for a refresh, and the reference's own `ShowUIPanel` early-return is
        // what makes that idempotent rather than a re-open.
        script.fire_event("PET_STABLE_SHOW", vec![]);
    } else {
        match (&*last, &fresh) {
            // A content change with no new list is a late creature-query answer filling a row in —
            // the client's cache-callback edge, and the only thing those callbacks fire.
            (Some(_), Some(_)) => {
                script.fire_event("PET_STABLE_UPDATE", vec![]);
                script.fire_event("PET_STABLE_UPDATE_PAPERDOLL", vec![]);
            }
            (Some(_), None) => script.fire_event("PET_STABLE_CLOSED", vec![]),
            // A first snapshot with no list latch cannot happen (the latch is set by the packet
            // that creates it); a None→None is nothing at all.
            (None, _) => {}
        }
    }
    *last = fresh;
    *last_npc = open.npc;
}

/// Drain the Lua intents into the four wire verbs, and the close into a local clear (no packet
/// exists — vmangos has no close opcode).
///
/// Every verb is addressed to the **open session's** NPC: the guid is the app's, never Lua's, so an
/// addon cannot aim a stable verb at an arbitrary unit.
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

/// Point the stable window's model pane at the pet `GetSelectedStablePet()` names — the app half of
/// the reference's `SetPetStablePaperdoll 0x4cb870`.
///
/// **The reference's own fork, and it is the whole of this system.** On `[0xb72250] == -1` — the
/// summoned pet — it resolves the live pet by GUID and takes `[unit+0xb30]`'s model; the resolve
/// failing (`0x4cb9bc`) falls through to the *same* path everything else takes: map the selection
/// back to a record, and go through the **creature cache** on that record's creature entry, with a
/// callback that fires `PET_STABLE_UPDATE_PAPERDOLL` when a late answer lands.
///
/// So the pane has two sources and one of them is not a unit at all:
///
/// - A **live** pet gets [`StableBooth::unit`], and the pane is the pet paper doll's case exactly
///   (decision 1057) — the booth mirrors the world body.
/// - Everything else gets [`StableBooth::display_id`], off the creature query's
///   [`crate::names::CreatureRecord::display_id`]. That covers every stabled pet *and* slot 0 while
///   the pet is dismissed or too far to summon: the server still sends its row, and the reference
///   still draws it.
///
/// A selection with no answer yet is `None`/`None`, which empties the pane; the query's arrival
/// re-points it on a later frame, which is what the reference's cache callback is for.
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
    // Written every frame, selection or none — a stale yaw would snap the model the moment one is
    // picked (the pet paper doll's arrangement).
    booth.yaw = script.model_pane_facing("PetStableModel");

    // `GetSelectedStablePet()`'s own answer: `0` = the summoned pet, `1..=2` a bought stable slot,
    // `-1` nothing. Reading the binding's translation rather than the raw petNumber is deliberate —
    // it is what already degrades a *stale* selection (a pet the latest list no longer holds) to
    // "nothing" instead of to a wrong slot.
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

/// The selection → `(live unit, display id)` fork, pure so the law is testable without a VM, a
/// wire or a world (module doc on [`feed_stable_booth`] for the reference's shape).
///
/// **The live arm consults no row at all** — `0x4cb870`'s `[0xb72250] == -1` branch reads the live
/// pet's GUID out of `[0xb714a0]` and takes its model; the stable array is not in that path. So a
/// summoned pet is drawn from its world body whether or not the list happened to carry its row.
///
/// **The display arm is resolved regardless**, because that is the reference's own order: the live
/// resolve failing (a dismissed pet, an object that has not streamed) falls straight through to the
/// creature cache, and having the answer already in hand means the pane does not blank for the
/// frames a pet spends being dismissed. `0` is the template's "no display" and reads as no answer —
/// the reference's cache miss draws nothing either.
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

/// Stable refusals staged for the feed, as **message-catalog keys** — only ever
/// `ERR_NOT_ENOUGH_MONEY`, the single code the client speaks for (decision 1677), reached through
/// `DisplayError(0x25)` (the `SMSG_STABLE_RESULT` handler `0x4cacb0`). A key rather than the
/// resolved text because that row carries error-speech line `0x28` too — the character says it
/// aloud (decision 1815).
#[derive(Resource, Default)]
pub(crate) struct StableErrors(pub(crate) Vec<&'static str>);

/// `StableSlotPrices.dbc`, loaded once at startup — the purchase row's price oracle. A missing
/// table quotes `0`, which is the same shape as "past the ladder": degraded, never wrong.
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

    /// The whole fork, arm by arm — this is what a blank pane looks like when it is wrong, and no
    /// build gate can see it.
    #[test]
    fn the_selection_forks_between_a_live_body_and_a_bare_display() {
        let pets = [row(0, 1), row(1, 2)];
        let live = Entity::from_raw_u32(7).expect("a test entity id");

        // Nothing selected: neither source, so the pane empties.
        assert_eq!(
            stable_subject(-1, Some(live), &pets, display_of),
            (None, None)
        );

        // Slot 0 with the pet out: the world body, and the display resolved beside it so a dismiss
        // does not blank the pane.
        assert_eq!(
            stable_subject(0, Some(live), &pets, display_of),
            (Some(live), Some(4449))
        );

        // Slot 0 with the pet dismissed (a row, no live guid): the display alone — the case that
        // has no world entity anywhere and the whole reason the stand-in exists.
        assert_eq!(
            stable_subject(0, None, &pets, display_of),
            (None, Some(4449))
        );

        // A stabled slot NEVER takes the live body, even while a pet is out: the reference's live
        // arm is gated on the selection being the summoned pet, not on one existing.
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

    /// A row whose creature query has not answered yet, and one whose template ships display `0`,
    /// both read as "no answer" — the reference's cache miss, which draws nothing rather than
    /// drawing wrong.
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

    /// **The stable window dies with the session.** The walk-away guard does not cover it: it
    /// needs a self player to measure from, and between the socket dying and the next world
    /// entry there is none — so a stable open at the drop rode into the next login's fresh VM.
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
