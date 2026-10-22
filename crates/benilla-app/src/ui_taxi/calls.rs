//! The taxi verbs' engine half, which the call host applies inside each call
//! ([`crate::script_calls::ScriptCallHost`]): `TakeTaxiNode` (`0x4dc8d0`) refuses or activates with
//! the flight master's cell as it stands, and `CloseTaxiMap` (`0x4dcce0`) runs the close core
//! `0x4dc0f0`, before the verb returns.

use benilla_ui::script::{CallTail, TaxiCall, UiScript};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{TaxiCatalogs, TaxiRouteCache, TaxiState};
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_session::{InteractLatch, LatchedWindow};

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
    /// Read only: the release itself waits for the drain ([`super::release_closed`]).
    latch: Option<Res<'w, InteractLatch>>,
    ahead: AheadOfTaxi<'w>,
}

/// The cells `0x493310` tests ahead of the taxi map's (`0x493341`-`0x4933b6`), read only: trade,
/// item text and merchant. Gossip and quest are left out: each writes a guid into its cell only
/// just after its own register (`0x4e1fe9` → `0x4e1ff0`, `0x500d74` → `0x500d86`), which closes a
/// latched map first, so neither holds the flight master's guid while the map is latched. The
/// item text writes its cell at the read (`0x4e3364`) and registers only when the text arrives
/// (`0x4e349a`).
#[derive(SystemParam)]
pub(crate) struct AheadOfTaxi<'w> {
    trade: Option<Res<'w, crate::ui_trade::TradeSession>>,
    item_text: Option<Res<'w, crate::ui_item_text::ItemTextOpen>>,
    merchant: Option<Res<'w, crate::ui_merchant::MerchantOpen>>,
}

impl AheadOfTaxi<'_> {
    /// Whether a window `0x493310` closes before the taxi map's holds `guid`.
    fn hold(&self, guid: u64) -> bool {
        fn holds<T: LatchedWindow>(w: &Option<Res<T>>, guid: u64) -> bool {
            w.as_deref().and_then(T::cell) == Some(guid)
        }
        holds(&self.trade, guid) || holds(&self.item_text, guid) || holds(&self.merchant, guid)
    }
}

impl TaxiSteps<'_, '_> {
    /// Apply one taxi call; the close's VM half and its events are the tail's.
    pub(crate) fn apply(&mut self, call: TaxiCall) -> Option<CallTail> {
        let (Some(state), Some(cache)) = (self.state.as_deref_mut(), self.cache.as_deref_mut())
        else {
            debug!("ui_taxi: no taxi map in this world; {call:?} dropped");
            return None;
        };
        match call {
            TaxiCall::Take(slot) => {
                take(
                    state,
                    cache,
                    self.catalogs.as_deref(),
                    &self.self_q,
                    &self.net,
                    &self.errors,
                    slot,
                );
                None
            }
            TaxiCall::Close => close(state, cache, self.latch.as_deref(), &self.ahead),
        }
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

/// The close core `0x4dc0f0` inside `CloseTaxiMap`'s call: with a map open, the cell cleared
/// (`0x4dc113`) and each slot's route freed (`0x4dc130`-`0x4dc145`), then `TAXIMAP_CLOSED`
/// (`0x4dc15f`). While the latch names the flight master its release (`0x4dc105`) runs
/// `0x493310`, which closes the first window in its order holding that guid: the taxi map itself
/// when none ahead of it does, re-entering the core, so the event fires twice. The slots stay,
/// their fares 0 with no flight master ([`UiScript::close_taxi`]). The latch's own release, the
/// goodbye, its clear and a window ahead's close, is the drain's ([`super::release_closed`]), as
/// it needs the latch (Part of #2202).
fn close(
    state: &mut TaxiState,
    cache: &mut TaxiRouteCache,
    latch: Option<&InteractLatch>,
    ahead: &AheadOfTaxi,
) -> Option<CallTail> {
    let flightmaster = state.open.as_ref()?.flightmaster;
    let reenters =
        latch.and_then(InteractLatch::guid) == Some(flightmaster) && !ahead.hold(flightmaster);
    let fires = if reenters { 2 } else { 1 };
    state.clear();
    state.released = Some(flightmaster);
    for slot in &mut cache.slots {
        slot.chain.clear();
    }
    debug!("ui_taxi: client-side close (no packet)");
    Some(Box::new(move |script: &mut UiScript| {
        script.close_taxi();
        script.note_taxi_closed();
        for _ in 0..fires {
            script.fire_event("TAXIMAP_CLOSED", Vec::new());
        }
    }))
}
