//! The stable-master bindings (decision 1676) — the hunter stable window's Lua surface, the same
//! two-way seam as [`super::bank`] and [`super::trainer`]: the app pushes a **stable snapshot**
//! ([`UiScript::set_stable`] — the wire's pet rows already resolved to icon/family/loyalty/diet
//! strings), and the Lua's click/purchase/close calls queue outbound **intents** the app drains.
//!
//! ## Three slots, and the current pet is one of them
//!
//! The window shows **`0..=2`**: slot `0` is the pet at the player's side, slots `1` and `2` are the
//! two stable slots a hunter buys (5875 ships exactly two — `StableSlotPrices.dbc` has two rows,
//! vmangos's `MAX_PET_STABLES` is 2, and the reference's `NUM_PET_STABLE_SLOTS` is 2). The wire is
//! 1-based over these; [`benilla_protocol::messages::StabledPet::slot`] already rebased it, so
//! everything here speaks the reference UI's own indices.
//!
//! **Slot 0 can be occupied while the player has no pet out.** A hunter whose pet is dismissed (or
//! merely too far away to be summoned) still gets a slot-0 row from the server, read off the
//! character-pet cache — which is exactly why the reference falls back to `GetStablePetInfo(0)`
//! when `UnitExists("pet")` is false (`PetStable.lua:131-146`) instead of showing an empty slot.
//!
//! ## What the snapshot resolves, and why the app does it
//!
//! The wire names a `creature_template` entry and a loyalty *level*; the window wants an icon, a
//! localized family word, a loyalty *name* and a diet list. Every one of those is a catalog join
//! the app already owns for the live pet ([`super::pet`], decisions 1005/1062), so the app does the
//! join once and pushes strings — this module never sees a DBC. The one join with no live-pet twin
//! is the icon of a pet that is *not* summoned: it comes from the creature query's display id,
//! which is why that field stopped being discarded (decision 1676).
//!
//! ## The drag rides the cursor at mode 10
//!
//! `PickupStablePet` puts the pet on the **global cursor** under payload mode 10
//! ([`super::cursor::CursorPayload::StablePet`]), carrying the stable index the grab recorded.
//!
//! This corrects what benilla shipped first. The original build made the drag frame-local, reading
//! payload mode 10 as a class/talent-ability id rather than the stabled-pet grab. `0x495020` **is**
//! the stabled-pet grab, and `[0xb4d900] = 10` is written at exactly one site image-wide, inside
//! it. Decision 1677.
//!
//! ## Two return conventions that are the API, not details
//!
//! `ClickStablePet` returns **exactly one value, always**, and which one is keyed *solely* on
//! whether the cursor was holding a pet — never on whether a packet went out:
//!
//! - **a plain click** (no payload) is a pure select and pushes the number `1.0` on all three of
//!   its legs, so it is **always truthy** and the reference always repaints;
//! - **a drop** pushes **nil** on every leg, including the ones that send a packet, so the
//!   reference never repaints from the drop — the repaint comes from the server's next list.
//!
//! Getting this backwards is invisible in a test that only checks the packets and very visible in
//! play: the window would repaint off a stale snapshot on every drag and skip the repaint on every
//! click.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// Window slots: the current pet (`0`) plus the two stable slots — the reference's
/// `NUM_PET_STABLE_SLOTS = 2` counted inclusively from zero (`PetStable.lua:1`).
pub const NUM_STABLE_SLOTS: usize = 3;

/// One row of the stable window, with every wire field already resolved to what the Lua renders.
/// `None` in [`StableState::slots`] is an empty slot — which the window draws differently from an
/// *unbought* one (that distinction is [`StableState::num_stable_slots`]'s).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StablePetSlot {
    /// The pet's own id — what [`StableIntent::Unstable`]/[`StableIntent::Swap`] name on the wire.
    /// Never its slot: the two disagree for any hunter whose stable is not in id order.
    pub pet_number: u32,
    /// `Interface\Icons\…` for the pet's family, or `None` while the creature query is in flight.
    /// The reference passes this straight to `SetItemButtonTexture`, which takes an empty texture
    /// for a missing icon — so `None` renders the empty-slot art, not a white square (decision
    /// 1046's sweep).
    pub icon: Option<String>,
    /// The name the hunter gave the pet, not the creature template's.
    pub name: String,
    pub level: u32,
    /// The localized `CreatureFamily.dbc` word ("Wolf", "Cat"). `None` when the creature query has
    /// not landed — the reference concatenates it into the level line unguarded, so the binding
    /// substitutes an empty string rather than handing Lua a nil to concatenate.
    pub family: Option<String>,
    /// The localized `PetLoyalty.dbc` name for the wire's loyalty level.
    pub loyalty: Option<String>,
    /// The localized pet-food names this pet's family eats — `GetStablePetFoodTypes`'s returns,
    /// which the reference joins with `BuildListString` into the diet tooltip.
    pub diet: Vec<String>,
}

/// The open stable window's snapshot. Pushed whole while a stable session is open; `None` = no
/// stable open (the window is closed).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StableState {
    /// Stable slots **purchased**, `0..=2` (the wire's `numStableSlots`). Not a count of occupied
    /// ones: it is what enables slot buttons `1..=num` and greys the rest, and what prices the next
    /// purchase.
    pub num_stable_slots: u32,
    /// The next slot's price in copper (`StableSlotPrices.dbc` row `num_stable_slots + 1`), or `0`
    /// past the table — a state in which the reference has already hidden the purchase row.
    pub next_slot_cost: u32,
    /// The three window slots; index `0` is the current pet.
    pub slots: [Option<StablePetSlot>; NUM_STABLE_SLOTS],
    /// The player has a **live** pet out — the client's own `[0xb714a0]|[0xb714a4]` guid test, the
    /// gate that forks a stabled pet's drop between swap and unstable.
    ///
    /// **Deliberately not `slots[0].is_some()`.** A dismissed pet, or one left out of range, still
    /// has a slot-0 row from the server's character-pet cache while the live guid is zero — so the
    /// two disagree exactly there, and the client follows the guid.
    pub has_live_pet: bool,
}

impl StableState {
    /// How many of the three slots hold a pet — `GetNumStablePets()`.
    fn num_pets(&self) -> u32 {
        self.slots.iter().filter(|s| s.is_some()).count() as u32
    }
}

/// An outbound stable verb the Lua asked for, drained by the app (which addresses it to the open
/// session's NPC — the guid is the app's, never Lua's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StableIntent {
    /// `CMSG_STABLE_PET` — put the current pet away. **Carries no destination**: the server takes
    /// the first free bought slot itself, so there is no "stable into slot 2" to express.
    Stable,
    /// `CMSG_UNSTABLE_PET` — summon this pet number, valid only with no current pet.
    Unstable(u32),
    /// `CMSG_STABLE_SWAP_PET` — trade the current pet for this pet number, in one step.
    Swap(u32),
    /// `CMSG_BUY_STABLE_SLOT` — buy the next slot; the *which* is implicit, as with the bank's.
    BuySlot,
}

/// The window's own transient state — selection and the frame-local drag (module doc: the cursor is
/// not involved). Lives beside the snapshot rather than inside it because the app *replaces* the
/// snapshot on every list packet, and a repaint must not drop what the player has selected.
#[derive(Debug)]
pub(crate) struct StableModel {
    pub(crate) state: Option<StableState>,
    /// The selection, held **as the client holds it** (`[0xb72250]`): `-1` = the summoned pet,
    /// `0` = nothing, otherwise a **petNumber**.
    ///
    /// A petNumber rather than a slot index because that is what the binary stores, and it is what
    /// makes a *stale* selection degrade correctly: `GetSelectedStablePet` searches the array for
    /// the number and answers `-1` when the pet is no longer there (fall-through `0x4cb84c`).
    pub(crate) selected: i32,
    pub(crate) intents: Vec<StableIntent>,
    pub(crate) close: bool,
}

/// The zero state is the client's `0` — "nothing selected" — not `-1`, which is its encoding for
/// *the summoned pet*. The Lua-facing `-1` that `PetStable.lua:44` tests is
/// [`super::UiScript::stable_selection`]'s translation of this, not this field.
impl Default for StableModel {
    fn default() -> Self {
        Self {
            state: None,
            // `0` is "nothing selected" in the client's own encoding — NOT `-1`, which means the
            // summoned pet. The window's zero state is the reset `0x4caad3` writes.
            selected: 0,
            intents: Vec::new(),
            close: false,
        }
    }
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open stable's snapshot.
    ///
    /// **Every list message clears the selection**, and that is the client's own behaviour
    /// (`0x4cadf8` writes `[0xb72250] = 0` on each one), not a simplification. The first build kept
    /// it across a refresh on the reasoning that benilla re-lists after every action and a reset
    /// would "fight the player" — but the reference re-lists on exactly the same
    /// successes and clears every time. It does not fight anything, because `PetStable_Update`
    /// immediately re-picks: the current pet if there is one, else the first occupied slot
    /// (`PetStable.lua:44-59`). Keeping a selection across a list is what would be wrong — it can
    /// name a pet the new list no longer contains.
    pub fn set_stable(&mut self, state: Option<StableState>) {
        let mut model = self.model_mut();
        model.stable.selected = 0;
        // The held payload is NOT dropped here: closing the window clears only the stable-master
        // guid in the reference (`0x4cae10`), and a picked-up pet lives on the shared cursor, whose
        // own ClearCursor arms own it.
        model.stable.state = state;
    }

    /// Drain the queued stable verbs (module doc: the app addresses them to the open NPC).
    pub fn take_stable_intents(&mut self) -> Vec<StableIntent> {
        std::mem::take(&mut self.model_mut().stable.intents)
    }

    /// Whether `ClosePetStables()` was called since the last drain (and clear the flag). No packet
    /// exists for it — the app just clears its local session, the bank/merchant pattern.
    pub fn take_stable_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().stable.close)
    }

    /// `GetSelectedStablePet()`'s answer — **`0` = the summoned pet, `1..=2` a stable slot, `-1`
    /// nothing** — translated out of the petNumber the model holds, exactly as `0x4cb800` does
    /// (find the number in the array, `inc eax`; a number no longer present falls through to `-1`).
    pub fn stable_selection(&mut self) -> i32 {
        let model = self.model_mut();
        selected_slot(&model.stable)
    }
}

/// The petNumber→slot translation behind `GetSelectedStablePet` (`0x4cb810`).
fn selected_slot(m: &StableModel) -> i32 {
    match m.selected {
        -1 => 0,
        0 => -1,
        pet_number => m
            .state
            .as_ref()
            .and_then(|s| {
                s.slots
                    .iter()
                    .enumerate()
                    .skip(1)
                    .find(|(_, slot)| {
                        slot.as_ref()
                            .is_some_and(|p| i64::from(p.pet_number) == i64::from(pet_number))
                    })
                    .map(|(i, _)| i as i32)
            })
            // A selection whose pet is gone after a refresh degrades to "nothing", never to a
            // wrong slot — the loop's own fall-through at `0x4cb84c`.
            .unwrap_or(-1),
    }
}

/// Read a slot argument into an index into [`StableState::slots`]. Out-of-range answers `None`, so
/// every binding below degrades to the reference's empty-slot behaviour rather than panicking on a
/// stray addon call.
fn slot_index(i: i64) -> Option<usize> {
    usize::try_from(i).ok().filter(|&i| i < NUM_STABLE_SLOTS)
}

/// Commit a drag from slot `from` onto slot `to` — the **one** place the stable's move law lives,
/// now read off the binary (`ClickStablePet 0x4cb420` regime B; decision 1677).
///
/// The first build inferred this from the server's constraint set and got the shape right and the
/// **three edges wrong**. Each of them is a real case:
///
/// 1. **The summoned pet onto an OCCUPIED stable slot sends `SWAP`, not `STABLE`.** "Drag out ⇒
///    stable it" only holds when the target slot is empty. The inferred version sent `STABLE` for
///    both, which vmangos then refuses whenever both bought slots are full — the pet stays put and
///    nothing says why.
/// 2. **A drop onto an UNPURCHASED slot sends nothing**, and is still a completed drop (cursor
///    cleared, falsy return).
/// 3. **A stabled pet onto the summoned slot forks on the LIVE PET GUID**, not on whether slot 0
///    has a row. A dismissed pet has a row and no guid, and the client sends `UNSTABLE` there —
///    the inferred version read the row and sent `SWAP`.
///
/// Stable→stable remains a no-op: 5875 has no opcode for it, which the first build did get right.
fn drag_verb(from: u8, to: u8, state: &StableState) -> Option<StableIntent> {
    if from == to {
        return None;
    }
    let occupied = |i: u8| state.slots.get(usize::from(i)).and_then(|s| s.as_ref());
    match (from, to) {
        // The summoned pet is held.
        (0, _) => match occupied(to) {
            // …onto an occupied stable slot: trade places.
            Some(target) => Some(StableIntent::Swap(target.pet_number)),
            // …onto an empty slot the player owns: stable it (the server picks the destination).
            None if u32::from(to) <= state.num_stable_slots => Some(StableIntent::Stable),
            // …onto a slot they have not bought: nothing.
            None => None,
        },
        // A stabled pet is held, dropped on the summoned slot.
        (_, 0) => occupied(from).map(|held| {
            if state.has_live_pet {
                StableIntent::Swap(held.pet_number)
            } else {
                StableIntent::Unstable(held.pet_number)
            }
        }),
        // Stable slot → stable slot: no opcode exists in 5875.
        _ => None,
    }
}

/// Register the stable globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetStablePetInfo(i) → icon, name, level, family, loyalty — **5 values on every reachable
    // exit** (`0x4cb280`: `eax = 5` at both returns). A miss pushes `nil, nil, 0.0, nil, nil`, NOT
    // nothing: the reference both tests the call for truthiness and destructures all five, and a
    // bare nil would make `level` nil where the client answers 0.
    g.set(
        "GetStablePetInfo",
        lua.create_function(|lua, i: i64| {
            let pet = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                slot_index(i).and_then(|i| model.stable.state.as_ref()?.slots[i].clone())
            };
            let Some(pet) = pet else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Number(0.0),
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                match &pet.icon {
                    Some(t) => Value::String(lua.create_string(t)?),
                    None => Value::Nil,
                },
                Value::String(lua.create_string(&pet.name)?),
                Value::Number(f64::from(pet.level)),
                match &pet.family {
                    Some(f) => Value::String(lua.create_string(f)?),
                    None => Value::Nil,
                },
                match &pet.loyalty {
                    Some(l) => Value::String(lua.create_string(l)?),
                    None => Value::Nil,
                },
            ]))
        })?,
    )?;

    // GetStablePetFoodTypes(i) → the localized diet names, one return each (the reference feeds the
    // lot to BuildListString). Nothing at all for an empty slot or a pet whose family has no diet —
    // the reference guards with `if ( GetStablePetFoodTypes(i) )` before formatting the tooltip.
    g.set(
        "GetStablePetFoodTypes",
        lua.create_function(|lua, i: i64| {
            let diet = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                slot_index(i)
                    .and_then(|i| {
                        Some(model.stable.state.as_ref()?.slots[i].as_ref()?.diet.clone())
                    })
                    .unwrap_or_default()
            };
            let mut out = Vec::with_capacity(diet.len());
            for d in &diet {
                out.push(Value::String(lua.create_string(d)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetNumStableSlots() → slots PURCHASED (0..=2). The reference both enables buttons `i <= n`
    // and hides the purchase row at `n == NUM_PET_STABLE_SLOTS` off this one number.
    g.set(
        "GetNumStableSlots",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(
                model
                    .stable
                    .state
                    .as_ref()
                    .map_or(0, |s| s.num_stable_slots),
            ))
        })?,
    )?;

    // GetNumStablePets() → how many of the three slots hold a pet.
    g.set(
        "GetNumStablePets",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(
                model.stable.state.as_ref().map_or(0, StableState::num_pets),
            ))
        })?,
    )?;

    // GetNextStableSlotCost() → the next slot's price in copper (the app read it from
    // StableSlotPrices.dbc). 0 with no stable open, and 0 past the table — where the reference has
    // already hidden the row that would show it.
    g.set(
        "GetNextStableSlotCost",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(
                model.stable.state.as_ref().map_or(0, |s| s.next_slot_cost),
            ))
        })?,
    )?;

    // GetSelectedStablePet() → the selected slot, or -1. The sentinel is the API: the reference
    // tests `selectedPet == -1` to decide whether to pick a slot for the player.
    g.set(
        "GetSelectedStablePet",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // NOT the raw field: the model holds the client's petNumber encoding, and this binding
            // is its translation to slot indices (`selected_slot`).
            Ok(i64::from(selected_slot(&model.stable)))
        })?,
    )?;

    // ClickStablePet(i) → **exactly one value, always**, and which one is keyed SOLELY on whether
    // the cursor held a pet — never on whether a packet went out (`0x4cb420`).
    //
    //   plain click  -> pure select, no packet, pushes 1.0 on ALL THREE legs  => always TRUTHY
    //   drop         -> clears the cursor, pushes nil on EVERY leg            => always FALSY
    //
    // So the reference repaints on every click and on NO drop; a drop's repaint arrives with the
    // server's next list. The first build had this inverted (true on a committed drag, false on a
    // no-op) — invisible to a packet-only test, and in play a window that repaints off a stale
    // snapshot on every drag and skips the repaint on every click.
    g.set(
        "ClickStablePet",
        lua.create_function(|lua, i: i64| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let Some(to) = slot_index(i) else {
                // Out of range still takes the select leg's "nothing" arm below.
                if !matches!(
                    model.cursor,
                    Some(super::cursor::CursorPayload::StablePet(_))
                ) {
                    model.stable.selected = 0;
                    return Ok(Value::Number(1.0));
                }
                super::cursor::clear_cursor(&mut model);
                return Ok(Value::Nil);
            };
            let to = to as u8;

            // Regime B — a drop.
            if let Some(super::cursor::CursorPayload::StablePet(held)) = model.cursor.clone() {
                let from = held.slot;
                let intent = model
                    .stable
                    .state
                    .as_ref()
                    .and_then(|state| drag_verb(from, to, state));
                super::cursor::clear_cursor(&mut model);
                if let Some(intent) = intent {
                    model.stable.intents.push(intent);
                }
                return Ok(Value::Nil);
            }

            // Regime A — a plain click: pure select, always truthy.
            let pet_number = model
                .stable
                .state
                .as_ref()
                .and_then(|s| s.slots[usize::from(to)].as_ref())
                .map(|p| p.pet_number);
            model.stable.selected = match (to, pet_number) {
                // Slot 0 selects the summoned pet by its own sentinel, not by a petNumber.
                (0, _) => -1,
                (_, Some(n)) => n as i32,
                // An empty (or unowned) slot selects NOTHING, rather than the slot itself.
                (_, None) => 0,
            };
            Ok(Value::Number(1.0))
        })?,
    )?;

    // PickupStablePet(i) — the mode-10 grab. **0 Lua values, always.**
    //
    // The client's gate is the family ICON path, not occupancy (`0x495010`: resolve the record,
    // require a non-empty `CreatureFamily` icon, then set the payload). Ours is the resolved icon,
    // which is the same test one step later: no icon, no grab.
    g.set(
        "PickupStablePet",
        lua.create_function(|lua, i: i64| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let texture = slot_index(i)
                .and_then(|i| model.stable.state.as_ref()?.slots[i].as_ref())
                .and_then(|p| p.icon.clone());
            if let Some(texture) = texture {
                model.cursor = Some(super::cursor::CursorPayload::StablePet(
                    super::cursor::CursorStablePet {
                        slot: i as u8,
                        texture,
                    },
                ));
                super::cursor::queue_cursor_update(&mut model);
            }
            Ok(())
        })?,
    )?;

    // StablePet() — 0 args, and it makes no Lua call at all in the reference: its only gate is a
    // stable master being open, which for us is the snapshot's existence. Not called by the shipped
    // FrameXML (the drag path sends this verb); it exists because the binding does.
    g.set(
        "StablePet",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.stable.state.is_some() {
                model.stable.intents.push(StableIntent::Stable);
            }
            Ok(())
        })?,
    )?;

    // UnstablePet(i) — carries a gate the drag path does NOT: it requires no charmed unit and **no
    // pet out** (`0x468550`), and bails SILENTLY otherwise, with no packet and no error. Its
    // unsigned bound also rejects index 0 outright, where the drag path treats 0 as the summoned
    // pet.
    g.set(
        "UnstablePet",
        lua.create_function(|lua, i: i64| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let pet_number = model
                .stable
                .state
                .as_ref()
                .filter(|s| !s.has_live_pet)
                .and_then(|s| {
                    slot_index(i)
                        .filter(|&i| i != 0)
                        .and_then(|i| s.slots[i].as_ref())
                })
                .map(|p| p.pet_number);
            if let Some(pet_number) = pet_number {
                model
                    .stable
                    .intents
                    .push(StableIntent::Unstable(pet_number));
            }
            Ok(())
        })?,
    )?;

    // SetPetStablePaperdoll(model) — inert here, deliberately, and this is the same divergence
    // PetPaperDollFrame.xml records for `PetModelFrame:SetUnit("pet")`: benilla's model panes are
    // app-side booths that follow the selection every frame, so there is no VM-side unit to point.
    // The binding still exists because the reference calls it (four sites) and an addon may.
    g.set(
        "SetPetStablePaperdoll",
        lua.create_function(|_, _model: Value| Ok(()))?,
    )?;

    // BuyStableSlot() — 0 args, with the client's own **silent** local gates in order: a stable
    // master open · the hard cap `slots != 2` (`0x4cb0c4`) · a price row exists · **affordability**
    // (`0x4cb122`). None of them shows a message; the reference disables the button instead.
    // Applying them here rather than trusting the button matters because an addon can call the
    // binding directly, and the server's refusal for the cap is the same indistinguishable
    // ERR_STABLE as everything else.
    g.set(
        "BuyStableSlot",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // Affordability reads the VM's own purse — the same `model.money` the reference's
            // `GetMoney()` returns and its own button-state ladder compares (`PetStable.lua:203`),
            // rather than a second copy of the number in the stable snapshot.
            let money = model.money;
            let allowed = model.stable.state.as_ref().is_some_and(|s| {
                s.num_stable_slots as usize != NUM_STABLE_SLOTS - 1
                    && s.next_slot_cost != 0
                    && u64::from(s.next_slot_cost) <= money
            });
            if allowed {
                model.stable.intents.push(StableIntent::BuySlot);
            }
            Ok(())
        })?,
    )?;

    // ClosePetStables() — client-side close, no packet exists (vmangos has no close opcode): flag
    // the app to clear its session.
    g.set(
        "ClosePetStables",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.stable.close = true;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn pet(number: u32, name: &str, level: u32) -> StablePetSlot {
        StablePetSlot {
            pet_number: number,
            icon: Some("Interface\\Icons\\Ability_Hunter_Pet_Wolf".into()),
            name: name.into(),
            level,
            family: Some("Wolf".into()),
            loyalty: Some("(Loyalty Level 6) Best Friend".into()),
            diet: vec!["Meat".into(), "Fish".into()],
        }
    }

    fn state(slots: [Option<StablePetSlot>; NUM_STABLE_SLOTS]) -> StableState {
        StableState {
            num_stable_slots: 1,
            next_slot_cost: 50_000,
            slots,
            has_live_pet: true,
        }
    }

    /// A hunter with a pet out and one stabled, one slot bought.
    fn open(s: &mut UiScript) {
        s.set_stable(Some(state([
            Some(pet(7, "Rex", 41)),
            Some(pet(8, "Bruiser", 38)),
            None,
        ])));
    }

    /// The read surface, through the reference's own destructuring.
    #[test]
    fn the_read_surface_answers_the_reference_calls() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumStableSlots()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetNumStablePets()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetNextStableSlotCost()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetSelectedStablePet()").unwrap(), -1);

        open(&mut s);
        assert_eq!(s.eval::<i64>("return GetNumStableSlots()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetNumStablePets()").unwrap(), 2);
        assert_eq!(
            s.eval::<i64>("return GetNextStableSlotCost()").unwrap(),
            50_000
        );

        // `PetStable.lua:76` — the exact five-value destructuring the window renders from.
        assert_eq!(
            s.eval::<(String, String, i64, String, String)>(
                "local i, n, l, f, loy = GetStablePetInfo(1) return i, n, l, f, loy"
            )
            .unwrap(),
            (
                "Interface\\Icons\\Ability_Hunter_Pet_Wolf".into(),
                "Bruiser".into(),
                38,
                "Wolf".into(),
                "(Loyalty Level 6) Best Friend".into()
            )
        );

        assert_eq!(
            s.eval::<(String, String)>("return GetStablePetFoodTypes(1)")
                .unwrap(),
            ("Meat".into(), "Fish".into())
        );
        assert!(s
            .eval::<bool>("return GetStablePetFoodTypes(2) == nil")
            .unwrap());
    }

    /// **`GetStablePetInfo` returns five values on EVERY exit** (`eax = 5` at both returns), with a
    /// miss pushing `nil, nil, 0, nil, nil`. The reference both tests the call for truthiness and
    /// destructures all five, so a bare nil would leave `level` nil where the client answers 0 —
    /// and would silently change the arity an addon sees.
    #[test]
    fn a_missing_pet_still_answers_five_values() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        for call in ["GetStablePetInfo(2)", "GetStablePetInfo(9)"] {
            assert_eq!(s.arity(call).unwrap(), 5, "{call} arity");
            // Still falsy on the reference's own `if ( GetStablePetInfo(i) )` test.
            assert!(s.eval::<bool>(&format!("return not {call}")).unwrap());
            assert_eq!(
                s.eval::<i64>(&format!("local _, _, l = {call} return l"))
                    .unwrap(),
                0,
                "{call} level is 0, not nil"
            );
        }
    }

    /// **A plain click always returns truthy**, on all three of its legs — including a re-click of
    /// the same slot and a click on an empty one. The reference repaints on the return, so a
    /// falsy answer anywhere here is a window that stops repainting on click.
    #[test]
    fn every_plain_click_returns_truthy() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        for call in [
            "ClickStablePet(1)",
            "ClickStablePet(1)",
            "ClickStablePet(0)",
            "ClickStablePet(2)",
        ] {
            assert!(
                s.eval::<bool>(&format!("return {call} and true or false"))
                    .unwrap(),
                "{call}"
            );
        }
        assert!(
            s.take_stable_intents().is_empty(),
            "selecting sends nothing"
        );
    }

    /// The selection's three encodings, read back through `GetSelectedStablePet`'s translation:
    /// slot 0 answers 0, an occupied stable slot answers its index, and an **empty** slot selects
    /// nothing (`-1`) rather than itself.
    #[test]
    fn the_selection_translates_back_to_slot_indices() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.eval::<()>("ClickStablePet(0)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedStablePet()").unwrap(), 0);
        s.eval::<()>("ClickStablePet(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedStablePet()").unwrap(), 1);
        s.eval::<()>("ClickStablePet(2)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetSelectedStablePet()").unwrap(),
            -1,
            "an empty slot selects nothing, not itself"
        );
    }

    /// A selection is held as a **petNumber**, so a refresh that no longer contains that pet
    /// degrades to "nothing" rather than to a wrong slot — and every list clears it anyway.
    #[test]
    fn a_list_clears_the_selection_and_a_stale_pet_degrades() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.eval::<()>("ClickStablePet(1)").unwrap();
        assert_eq!(s.stable_selection(), 1);

        // Every list message clears it (`0x4cadf8`) — the reference re-picks in PetStable_Update.
        open(&mut s);
        assert_eq!(s.stable_selection(), -1, "a list clears the selection");
    }

    /// **A drop always returns nil**, on every leg — the ones that send a packet included. The
    /// reference therefore never repaints from a drop; the repaint arrives with the server's next
    /// list. This is the convention the first build had inverted.
    #[test]
    fn every_drop_returns_falsy_even_when_it_sends() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        // A drop that DOES send.
        assert!(s
            .eval::<bool>("PickupStablePet(1) return ClickStablePet(0) == nil")
            .unwrap());
        assert_eq!(s.take_stable_intents(), vec![StableIntent::Swap(8)]);

        // A drop that sends nothing is equally falsy.
        assert!(s
            .eval::<bool>("PickupStablePet(1) return ClickStablePet(1) == nil")
            .unwrap());
        assert!(s.take_stable_intents().is_empty());
    }

    /// **The summoned pet onto an OCCUPIED stable slot is a SWAP, not a STABLE** — the first of the
    /// three edges the inferred law got wrong. Sending STABLE there is refused by the server
    /// whenever both bought slots are full, and nothing says why.
    #[test]
    fn dropping_onto_an_occupied_slot_swaps() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.eval::<()>("PickupStablePet(0) ClickStablePet(1)")
            .unwrap();
        assert_eq!(s.take_stable_intents(), vec![StableIntent::Swap(8)]);
    }

    /// …and onto an EMPTY slot the player owns it is a STABLE, whose destination the wire cannot
    /// carry; onto one they have **not bought**, nothing at all — but the cursor still clears.
    #[test]
    fn an_unpurchased_slot_takes_the_drop_and_sends_nothing() {
        let mut s = UiScript::new().unwrap();
        // Slot 1 bought and empty, slot 2 unbought.
        s.set_stable(Some(state([Some(pet(7, "Rex", 41)), None, None])));
        s.eval::<()>("PickupStablePet(0) ClickStablePet(1)")
            .unwrap();
        assert_eq!(s.take_stable_intents(), vec![StableIntent::Stable]);

        s.eval::<()>("PickupStablePet(0) ClickStablePet(2)")
            .unwrap();
        assert!(s.take_stable_intents().is_empty(), "slot 2 is not bought");
        // The drop completed: the cursor is empty, so the next click SELECTS.
        assert!(s.eval::<bool>("return ClickStablePet(1) and true").unwrap());
        assert!(s.take_stable_intents().is_empty());
    }

    /// **The stabled→summoned fork reads the LIVE PET GUID, not slot 0's row** — the third edge.
    /// A dismissed pet still has a row from the server's character-pet cache while the guid is
    /// zero, and the client sends UNSTABLE there.
    #[test]
    fn the_fork_follows_the_live_pet_not_the_row() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.eval::<()>("PickupStablePet(1) ClickStablePet(0)")
            .unwrap();
        assert_eq!(s.take_stable_intents(), vec![StableIntent::Swap(8)]);

        // The same list, but the pet is dismissed: the row survives, the guid does not.
        let mut dismissed = state([Some(pet(7, "Rex", 41)), Some(pet(8, "Bruiser", 38)), None]);
        dismissed.has_live_pet = false;
        s.set_stable(Some(dismissed));
        s.eval::<()>("PickupStablePet(1) ClickStablePet(0)")
            .unwrap();
        assert_eq!(
            s.take_stable_intents(),
            vec![StableIntent::Unstable(8)],
            "a row without a live guid unstables"
        );
    }

    /// Stable→stable has no opcode in 5875, and an empty slot carries nothing to grab.
    #[test]
    fn stable_to_stable_is_a_no_op_and_empty_slots_do_not_grab() {
        let mut s = UiScript::new().unwrap();
        s.set_stable(Some(StableState {
            num_stable_slots: 2,
            slots: [None, Some(pet(8, "Bruiser", 38)), None],
            ..state([None, None, None])
        }));
        s.eval::<()>("PickupStablePet(1) ClickStablePet(2)")
            .unwrap();
        assert!(s.take_stable_intents().is_empty());

        // An empty slot never arms a grab, so the following click is a select.
        s.eval::<()>("PickupStablePet(2)").unwrap();
        assert!(s.eval::<bool>("return ClickStablePet(0) and true").unwrap());
        assert!(s.take_stable_intents().is_empty());
    }

    /// `BuyStableSlot`'s gates are the client's own, and all silent: the hard cap, and
    /// affordability.
    #[test]
    fn buy_stable_slot_gates_locally_and_silently() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000_000);
        open(&mut s);
        s.eval::<()>("BuyStableSlot()").unwrap();
        assert_eq!(s.take_stable_intents(), vec![StableIntent::BuySlot]);

        // Too poor.
        s.set_money(10);
        s.set_stable(Some(state([None, None, None])));
        s.eval::<()>("BuyStableSlot()").unwrap();
        assert!(s.take_stable_intents().is_empty(), "unaffordable is silent");

        // Both slots owned — the hard cap (and money restored, so only the cap can refuse).
        s.set_money(1_000_000);
        let mut full = state([None, None, None]);
        full.num_stable_slots = 2;
        s.set_stable(Some(full));
        s.eval::<()>("BuyStableSlot()").unwrap();
        assert!(s.take_stable_intents().is_empty(), "the cap is silent");
    }

    /// `UnstablePet` carries a gate the drag path does not: no pet out, else silence. And its
    /// unsigned bound rejects index 0, where the drag path reads 0 as the summoned pet.
    #[test]
    fn the_unstable_binding_gates_where_the_drag_path_does_not() {
        let mut s = UiScript::new().unwrap();
        open(&mut s); // has_live_pet = true
        s.eval::<()>("UnstablePet(1)").unwrap();
        assert!(s.take_stable_intents().is_empty(), "a pet is out — silent");

        let mut dismissed = state([None, Some(pet(8, "Bruiser", 38)), None]);
        dismissed.has_live_pet = false;
        s.set_stable(Some(dismissed));
        s.eval::<()>("UnstablePet(0)").unwrap();
        assert!(s.take_stable_intents().is_empty(), "index 0 is rejected");
        s.eval::<()>("UnstablePet(1)").unwrap();
        assert_eq!(s.take_stable_intents(), vec![StableIntent::Unstable(8)]);
    }

    /// The close intent, and that closing does NOT drop a held pet — the reference's close clears
    /// only the stable-master guid.
    #[test]
    fn close_drains_and_leaves_the_cursor_alone() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        assert!(!s.take_stable_close());
        s.eval::<()>("ClosePetStables()").unwrap();
        assert!(s.take_stable_close());
        assert!(!s.take_stable_close(), "drain clears");
    }

    /// `SetPetStablePaperdoll` exists and is harmless — the reference calls it at four sites.
    #[test]
    fn the_paperdoll_setter_is_callable() {
        let s = UiScript::new().unwrap();
        s.eval::<()>("SetPetStablePaperdoll(nil)").unwrap();
    }
}
