//! The stable master's packet handlers (in the net handler table since
//! 2318, moved out of the drain's npc arm file) — the [`StableOpen`] session and the
//! [`StableErrors`] line queue the stable feed ([`super`]) reads.

use benilla_protocol::messages::StabledPet;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{StableErrors, StableOpen};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};

/// Register the stable handlers — called from [`super::UiStablePlugin`].
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::ListStabledPets, on_list)
        .net_handler(K::StableResult, on_result)
        .net_handler(K::Disconnected, on_session_end);
}

/// The stable window dies with the socket — a listener on the session end (a second handler on the
/// kind, after the bridge's own teardown). The walk-away guard cannot stand in: it measures from a
/// self player, and there is none between the drop and the next world entry.
fn on_session_end(In(_): In<SessionEvent>, mut stable_open: ResMut<StableOpen>) {
    stable_open.clear();
}

fn on_list(
    In(ev): In<SessionEvent>,
    mut stable_open: ResMut<StableOpen>,
    mut names: ResMut<NameCache>,
) {
    if let SessionEvent::ListStabledPets {
        npc,
        num_stable_slots,
        pets,
    } = ev
    {
        list_stabled_pets(npc, num_stable_slots, pets, &mut stable_open, &mut names);
    }
}

fn on_result(
    In(ev): In<SessionEvent>,
    mut stable_open: ResMut<StableOpen>,
    mut errors: ResMut<StableErrors>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::StableResult { result } = ev {
        stable_result(result, &mut stable_open, &mut errors, &commands);
    }
}

/// A stable master's pet list (`MSG_LIST_STABLED_PETS`): fill the [`StableOpen`] the stable feed
/// ([`super`]) reads. Arrives unprompted off the gossip stable option — that is how the
/// window opens — and again in answer to our own refresh send.
fn list_stabled_pets(
    npc: u64,
    num_stable_slots: u8,
    pets: Vec<StabledPet>,
    stable_open: &mut StableOpen,
    names: &mut NameCache,
) {
    debug!(
        "net: stable master {npc:#x} listed {} pets ({num_stable_slots} slots bought)",
        pets.len()
    );
    // **Seed the pet-name cache from the list**. Every row carries the pet's own
    // number and the name its owner gave it — the exact `(pet_number, name)` pair
    // `SMSG_PET_NAME_QUERY_RESPONSE` would answer with — so the pet a player unstables has a
    // resolvable `UnitName("pet")` the moment it is summoned, instead of after a round trip.
    //
    // The window this closes is a real one the director hit: `PetStable_Update` sets the current
    // pet's button tooltip to a bare `UnitName("pet")` (`PetStable.lua:161`, transcribed
    // unguarded because the reference is unguarded), and hands it to `GameTooltip:SetText`, whose
    // byte-pinned signature REQUIRES a string (`0x531b90`) and raises otherwise. Nil name ⇒ Lua
    // error dialog, once per unstable.
    //
    // Why the reference does not trip over its own unguarded line: its pet-name cache is
    // `petnamecache.wdb`, which **persists across sessions**, so a pet you have owned before is
    // already warm before the packet arrives. benilla has no such file, so a window the reference
    // has practically closed is wide open for us. Seeding is not a workaround for the missing
    // cache — it is using the answer the server has already sent us in this very packet.
    for pet in &pets {
        if !pet.name.is_empty() {
            names.insert_pet(pet.pet_number, pet.name.clone());
        }
    }
    stable_open.open(npc, num_stable_slots, pets);
}

/// The answer to a stable verb (`SMSG_STABLE_RESULT`) — one byte, and the client's whole response
/// to it is a five-way jump table (the raw remap/jump bytes at `0x4cadac`/`0x4cad98`; decision
/// 1677):
///
/// | code | what the client does |
/// |---|---|
/// | 1 | `DisplayError(0x25)` = **`ERR_NOT_ENOUGH_MONEY`** — the only code that says anything |
/// | 2–7 | **absolutely nothing** — vmangos's catch-all `STABLE_ERR_STABLE = 6` included |
/// | 8, 9 | re-request the list |
/// | 10 | **`inc` the local purchased-slot count**, then re-request |
/// | 11 | fire `PET_STABLE_UPDATE` (no vmangos counterpart — nothing in 1.12 sends it) |
/// | 0, ≥12 | nothing |
///
/// The re-request is not a benilla convenience: no success carries an updated list, so without it
/// the window would go on showing the pre-action arrangement.
///
/// **Code 10's local increment happens BEFORE the guid test**, so a buy-slot success arriving with
/// no stable master open still bumps the count without refreshing. That ordering is reproduced
/// deliberately — it is what makes the purchase row correct if the window is reopened, and the
/// alternative would be to invent a tidier client than the one being reimplemented.
fn stable_result(
    result: u8,
    stable_open: &mut StableOpen,
    errors: &mut StableErrors,
    net_commands: &NetCommands,
) {
    use benilla_protocol::messages::stable_result as code;
    // Ahead of the guid test, exactly as `0x4cacf3` sits ahead of `0x4cad05`.
    if result == code::SUCCESS_BUY_SLOT {
        stable_open.num_stable_slots = stable_open.num_stable_slots.saturating_add(1);
    }
    match result {
        // The one code that speaks. Its text is the reference's own `GlobalStrings` value for
        // `ERR_NOT_ENOUGH_MONEY`, reached through `DisplayError` row 0x25.
        code::ERR_MONEY => {
            debug!("net: stable purchase refused — not enough money");
            errors.0.push("ERR_NOT_ENOUGH_MONEY");
        }
        code::SUCCESS_STABLE | code::SUCCESS_UNSTABLE | code::SUCCESS_BUY_SLOT => {
            let Some(npc) = stable_open.npc else {
                debug!("net: stable success {result} with no open stable — no re-list");
                return;
            };
            debug!("net: stable action succeeded (code {result}) — re-listing");
            let _ = net_commands.0.send(ClientCommand::ListStabledPets { npc });
        }
        // Codes 2–7 (the generic ERR_STABLE among them), 0 and ≥12: the client shows NOTHING.
        // Not an omission — the catch-all is one code for six causes, and the reference declines to
        // guess which.
        _ => debug!("net: stable result {result} — no client-visible effect"),
    }
}
