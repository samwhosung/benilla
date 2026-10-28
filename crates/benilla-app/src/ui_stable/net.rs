//! The stable master's packet handlers: they fill [`StableOpen`] and signal on the stable lane
//! ([`StableLane`]) inside themselves, the creature answers' callbacks included.

use benilla_protocol::messages::StabledPet;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{StableCreatureWaits, StableEvent, StableLane, StableOpen};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};

/// `SMSG_STABLE_RESULT`'s code 11, which signals `PET_STABLE_UPDATE` (`0x4cad83`); vmangos sends
/// none.
const RESULT_UPDATE: u8 = 0x0B;

/// Register the stable handlers, the creature callbacks' second handler on the creature answer,
/// and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::ListStabledPets, on_list)
        .net_handler(K::StableResult, on_result)
        .net_handler(K::CreatureName, on_creature_answer)
        .net_handler(K::Disconnected, on_session_end);
}

/// The creature cache's answer runs the window's callbacks waiting on its entry, a refusal with
/// status 0 (`0x556f05`, `0x556f91`); [`StableCreatureWaits::answered`].
fn on_creature_answer(
    In(ev): In<SessionEvent>,
    mut waits: ResMut<StableCreatureWaits>,
    mut lane: ResMut<StableLane>,
) {
    if let SessionEvent::CreatureName { entry, name, .. } = ev {
        waits.answered(entry, name.is_some(), &mut lane);
    }
}

/// The stable closes with the connection; the range guard cannot close it, as it has no self
/// player to measure from after the drop.
fn on_session_end(In(_): In<SessionEvent>, mut stable_open: ResMut<StableOpen>) {
    stable_open.clear();
}

fn on_list(In(ev): In<SessionEvent>, mut names: ResMut<NameCache>, mut commands: Commands) {
    if let SessionEvent::ListStabledPets {
        npc,
        num_stable_slots,
        pets,
    } = ev
    {
        list_stabled_pets(npc, &pets, &mut names, num_stable_slots);
        commands.queue(move |world: &mut World| {
            super::open_stable(world, npc, num_stable_slots, pets);
        });
    }
}

fn on_result(
    In(ev): In<SessionEvent>,
    mut stable_open: ResMut<StableOpen>,
    mut lane: ResMut<StableLane>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::StableResult { result } = ev {
        stable_result(result, &mut stable_open, &mut lane, &commands);
    }
}

/// `MSG_LIST_STABLED_PETS`, sent unprompted for the gossip stable option, which opens the window,
/// and in answer to a refresh: the pet names it carries, before the open.
fn list_stabled_pets(npc: u64, pets: &[StabledPet], names: &mut NameCache, num_stable_slots: u8) {
    debug!(
        "net: stable master {npc:#x} listed {} pets ({num_stable_slots} slots bought)",
        pets.len()
    );
    // Seed the pet-name cache from the rows, the pair `SMSG_PET_NAME_QUERY_RESPONSE` would carry:
    // stock `PetStable.lua:163` hands `UnitName("pet")` to a tooltip whose `SetText` raises on nil
    // (`0x531b90`), and the pet-name store starts every world session empty (`0x5557f9`).
    for pet in pets {
        if !pet.name.is_empty() {
            names.insert_pet(pet.pet_number, pet.name.clone());
        }
    }
}

/// `SMSG_STABLE_RESULT`, handled as the reference's jump table (`0x4cadac`, `0x4cad98`) does: 1
/// shows `ERR_NOT_ENOUGH_MONEY` (`DisplayError(0x25)`), 8 and 9 re-request the list, as no success
/// carries one, 10 counts a bought slot and re-requests, 11 signals `PET_STABLE_UPDATE`
/// (`0x4cad83`), and 0, 2-7 and 12 up show nothing. The line and the signal go on the lane.
fn stable_result(
    result: u8,
    stable_open: &mut StableOpen,
    lane: &mut StableLane,
    net_commands: &NetCommands,
) {
    use benilla_protocol::messages::stable_result as code;
    stable_open.settle_world_reset();
    // Before the guid test, as `0x4cacf3` precedes `0x4cad05`: the count moves even with no
    // stable open.
    if result == code::SUCCESS_BUY_SLOT {
        stable_open.num_stable_slots = stable_open.num_stable_slots.saturating_add(1);
    }
    match result {
        code::ERR_MONEY => {
            debug!("net: stable purchase refused — not enough money");
            lane.push(StableEvent::Line("ERR_NOT_ENOUGH_MONEY"));
        }
        code::SUCCESS_STABLE | code::SUCCESS_UNSTABLE | code::SUCCESS_BUY_SLOT => {
            let Some(npc) = stable_open.npc else {
                debug!("net: stable success {result} with no open stable — no re-list");
                return;
            };
            debug!("net: stable action succeeded (code {result}) — re-listing");
            let _ = net_commands.0.send(ClientCommand::ListStabledPets { npc });
        }
        RESULT_UPDATE => lane.push(StableEvent::Update),
        // The reference shows nothing for these.
        _ => debug!("net: stable result {result} — no client-visible effect"),
    }
}
