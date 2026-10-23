//! The taxi verbs' engine half, which the call host applies inside each call
//! ([`crate::script_calls::ScriptCallHost`]): `TakeTaxiNode` (`0x4dc8d0`) refuses or activates with
//! the flight master's cell as it stands before the verb returns; `CloseTaxiMap` (`0x4dcce0`)
//! runs the close core `0x4dc0f0` through the host, which lends the latch beside this.

use benilla_ui::script::{TaxiCall, UiScript};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{TaxiCatalogs, TaxiRouteCache, TaxiState};
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_session::LatchedWindow;

/// The map, its slots and what the verbs send through.
#[derive(SystemParam)]
pub(crate) struct TaxiSteps<'w, 's> {
    /// Absent in a world with no taxi map, which drops the calls.
    state: Option<ResMut<'w, TaxiState>>,
    cache: Option<ResMut<'w, TaxiRouteCache>>,
    catalogs: Option<Res<'w, TaxiCatalogs>>,
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    net: Res<'w, NetCommands>,
    errors: Res<'w, UiErrorKeys>,
}

impl TaxiSteps<'_, '_> {
    /// Apply one taxi call; the close is the host's ([`TaxiCall::Close`]).
    pub(crate) fn apply(&mut self, call: TaxiCall) {
        let (Some(state), Some(cache)) = (self.state.as_deref_mut(), self.cache.as_deref_mut())
        else {
            debug!("ui_taxi: no taxi map in this world; {call:?} dropped");
            return;
        };
        if let TaxiCall::Take(slot) = call {
            take(
                state,
                cache,
                self.catalogs.as_deref(),
                &self.self_q,
                &self.net,
                &self.errors,
                slot,
            );
        }
    }

    /// The flight master's cell `[0xbb4ab8]`.
    pub(crate) fn flightmaster(&self) -> Option<u64> {
        self.state.as_deref().and_then(TaxiState::cell)
    }

    /// The close core past its release (`0x4dc0f0`): the cell cleared (`0x4dc113`) and each
    /// slot's route freed (`0x4dc130`-`0x4dc145`). The slots stay, their fares 0 with no flight
    /// master ([`UiScript::close_taxi`]).
    pub(crate) fn clear_map(&mut self, script: &mut UiScript) {
        if let Some(state) = self.state.as_deref_mut() {
            state.clear();
        }
        if let Some(cache) = self.cache.as_deref_mut() {
            for slot in &mut cache.slots {
                slot.chain.clear();
            }
        }
        script.close_taxi();
    }
}

/// `TakeTaxiNode(slot)` (`0x4dc8d0`) takes a slot under the count (`0x4dc8dc`), then in order: a
/// mounted player is refused with `0xb2`, `ERR_TAXIPLAYERALREADYMOUNTED` (`0x4dc909`-`0x4dc918`),
/// and the current node with `0xaa`, `ERR_TAXISAMENODE` (`0x4dc939`-`0x4dc942`); a `TaxiPath`
/// from the current node to the slot's sends `CMSG_ACTIVATETAXI`, routed or not (`0x4dc952`,
/// `0x4dc9be`); else the slot's route sends `CMSG_ACTIVATETAXIEXPRESS` with its raw fare sum
/// (`0x4dc965`-`0x4dc98e`); and a slot with neither shows `0xab`, `ERR_TAXINOSUCHPATH`
/// (`0x4dc999`). Each send reads the flight master's cell, 0 once the map closed (`0x4dc113`).
fn take(
    state: &TaxiState,
    cache: &TaxiRouteCache,
    catalogs: Option<&TaxiCatalogs>,
    self_q: &Query<&ObjectStore, With<SelfPlayer>>,
    net: &NetCommands,
    errors: &UiErrorKeys,
    slot: i32,
) {
    // The Lua number less 1, compared unsigned with the count.
    let Some(resolved) = usize::try_from(slot)
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| cache.slots.get(i))
    else {
        return;
    };
    let guid = state.open.as_ref().map_or(0, |open| open.flightmaster);
    // `UNIT_FIELD_MOUNTDISPLAYID` read signed (`[+0x110]+0x1fc`, `jle` at `0x4dc911`).
    let mounted = self_q
        .iter()
        .next()
        .is_some_and(|s| s.0.unit_mount_display_id() as i32 > 0);
    let (src, dest) = (cache.current, resolved.node);
    let direct = catalogs.is_some_and(|c| c.paths.between(src, dest).is_some());
    let refusal = if mounted {
        Some("ERR_TAXIPLAYERALREADYMOUNTED")
    } else if dest == src {
        Some("ERR_TAXISAMENODE")
    } else if !direct && resolved.chain.is_empty() {
        Some("ERR_TAXINOSUCHPATH")
    } else {
        None
    };
    if let Some(key) = refusal {
        debug!("ui_taxi: TakeTaxiNode({slot}) refused — {key}");
        errors.push(UiError::key(key));
        return;
    }
    if direct {
        debug!(
            "ui_taxi: activate {src} -> {dest} (direct path, {} copper)",
            resolved.cost
        );
        let _ = net.0.send(ClientCommand::ActivateTaxi {
            guid,
            source_node: src,
            dest_node: dest,
        });
    } else {
        debug!(
            "ui_taxi: activate express {:?} ({} copper)",
            resolved.chain, resolved.cost
        );
        let _ = net.0.send(ClientCommand::ActivateTaxiExpress {
            guid,
            total_cost: resolved.cost,
            nodes: resolved.chain.clone(),
        });
    }
}
