//! The stable master's packet handlers: they fill [`StableOpen`] and [`StableErrors`].

use benilla_protocol::messages::StabledPet;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{StableErrors, StableOpen};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};

/// Register the stable handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::ListStabledPets, on_list)
        .net_handler(K::StableResult, on_result)
        .net_handler(K::Disconnected, on_session_end);
}

/// The stable closes with the connection; the range guard cannot close it, as it has no self
/// player to measure from after the drop.
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

/// `MSG_LIST_STABLED_PETS`, sent unprompted for the gossip stable option, which opens the window,
/// and in answer to a refresh.
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
    // Seed the pet-name cache from the rows, the pair `SMSG_PET_NAME_QUERY_RESPONSE` would carry:
    // stock `PetStable.lua:163` hands `UnitName("pet")` to a tooltip whose `SetText` raises on nil
    // (`0x531b90`), and unlike the reference's `petnamecache.wdb` this cache starts empty.
    for pet in &pets {
        if !pet.name.is_empty() {
            names.insert_pet(pet.pet_number, pet.name.clone());
        }
    }
    stable_open.open(npc, num_stable_slots, pets);
}

/// `SMSG_STABLE_RESULT`, handled as the reference's jump table (`0x4cadac`, `0x4cad98`) does: 1
/// shows `ERR_NOT_ENOUGH_MONEY` (`DisplayError(0x25)`), 8 and 9 re-request the list, as no success
/// carries one, 10 counts a bought slot and re-requests, and 0, 2-7 and 12 up show nothing. 11
/// fires `PET_STABLE_UPDATE` in the reference and nothing here: vmangos never sends it.
fn stable_result(
    result: u8,
    stable_open: &mut StableOpen,
    errors: &mut StableErrors,
    net_commands: &NetCommands,
) {
    use benilla_protocol::messages::stable_result as code;
    // Before the guid test, as `0x4cacf3` precedes `0x4cad05`: the count moves even with no
    // stable open.
    if result == code::SUCCESS_BUY_SLOT {
        stable_open.num_stable_slots = stable_open.num_stable_slots.saturating_add(1);
    }
    match result {
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
        // The reference shows nothing for these.
        _ => debug!("net: stable result {result} — no client-visible effect"),
    }
}
