//! Our own player's arrival (`OnActiveArrival 0x5dea50`), the last step of its `PostInit`
//! (`0x5debe0`, at `0x5dec7b`), which the update packet's notify pass runs as our create's hook,
//! ahead of the later blocks' notifies: its writes are made here, inside the packet, so the next
//! packet's handlers find our player tagged, in control and its mover claimed. Its signals and the
//! world-enter cascade's resets run as one item on the arrival lane, stamped here, in the
//! reference's order ([`crate::ui_world_enter`]). The restore is the arrival's alone, never the
//! cascade's, which a `/reload` runs too.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::Scene;
use crate::net::ClientCommand;

/// What the arrival writes past the streamed world; each absent in a harness without it.
#[derive(SystemParam)]
pub(super) struct Arrival<'w> {
    actions: Option<ResMut<'w, crate::ui_action::PlayerActions>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    reactive: Option<ResMut<'w, crate::ui_action::ReactiveSpells>>,
    lane: Option<ResMut<'w, crate::ui_world_enter::ArrivalLane>>,
    stable: Option<ResMut<'w, crate::ui_stable::StableOpen>>,
}

/// The arrival's writes, in its order, and its item on the arrival lane, at the create's place.
pub(super) fn arrive(sc: &mut Scene, arrival: &mut Arrival, guid: u64) {
    // `ResetSpellBook 0x4b23a0` empties the reactive list (`0x5dea86`), and each initial spell's
    // `LearnSpell` runs the registrar, whose leg takes it in again in wire order
    // (`0x5dea96`-`0x5deab9`).
    if let (Some(actions), Some(spells), Some(reactive)) = (
        arrival.actions.as_deref(),
        arrival.spells.as_deref(),
        arrival.reactive.as_mut(),
    ) {
        reactive.0.reset();
        for &spell_id in &actions.initial {
            reactive.0.learned(spell_id, &spells.catalog);
        }
    }
    // `0x5deb49`: the manager's `+0xc8` gates the restore, the claim and the cascade; its
    // constructor (`0x4650e7`) is its only store, so it is clear at every create.
    let control_gained = crate::player::regain_control_on_arrival(
        sc.mover_lock.as_deref_mut(),
        sc.own_control_lost.as_deref_mut(),
    );
    // `0x4958a0` (`0x5deb5b`) claims our own mover through `SetActiveMover` (`0x6006e0`), which
    // sends `CMSG_SET_ACTIVE_MOVER` only when the mover changes ([`crate::net::MoverClaim`]).
    // `0x5fa600(1)` claims it first, at `0x5fa696`, when the camera's `+0x88` already names us
    // (`0x5fa63c`-`0x5fa65e`), ahead of its signal at `0x5fa6b1`; this claim is then a no-op. The
    // world entry and the world load that purges the streamed world for every login, reconnect
    // and cross-map worldport zero the mover first, so each of those creates sends; a same-map
    // teleport creates nothing. The server has seated the player by then, so nothing sent here is
    // dropped.
    let claimed = match sc.mover.as_deref_mut() {
        Some(mover) => mover.claim(guid, &sc.net),
        None => sc
            .net
            .0
            .send(ClientCommand::SetActiveMover { guid })
            .is_ok(),
    };
    if claimed && benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "mvr",
            &format!("SET_ACTIVE_MOVER guid={guid:#x} (self create)"),
        );
    }
    // The arrival's signals and its world-enter cascade (`0x5dea86`-`0x5debcc`, `0x5deb60` →
    // `0x4908c0`), with the flag's rise among them (`0x5deb56`); the form-list edits of the
    // packets after this one are kept for its replay to apply after its own.
    if let Some(lane) = arrival.lane.as_mut() {
        lane.push(crate::ui_world_enter::Entry::Arrival { control_gained });
        if let Some(actions) = arrival.actions.as_mut() {
            actions.form_edits = Some(Vec::new());
        }
    }
    // The cascade's silent window resets (`0x4909ab`, `0x4909b0`, `0x4909b5`, `0x490a0a`), owed
    // from here ([`crate::ui_world_enter::OwedReset`]).
    sc.trainer.owe_world_reset();
    sc.merchant.owe_world_reset();
    if let Some(trade) = sc.trade.as_mut() {
        trade.world_reset.owe();
    }
    if let Some(stable) = arrival.stable.as_mut() {
        stable.world_reset.owe();
    }
}

#[cfg(test)]
mod tests;
