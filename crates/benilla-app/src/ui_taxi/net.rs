//! The taxi map's packet handlers (decision 0484 phase 1; in the net handler table since 2318,
//! moved out of the drain's npc arm file) — the [`TaxiState`] session the taxi feed ([`super`])
//! reads, and the flight masters' overhead status.

use benilla_protocol::messages::TaxiMask;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::TaxiState;
use crate::net::{GuidIndex, NetHandlerApp};

/// Register the taxi handlers — called from [`super::UiTaxiPlugin`]. One per kind, plus the
/// session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TaxiNodesShown, on_nodes_shown)
        .net_handler(K::TaxiNodeStatus, on_node_status)
        .net_handler(K::ActivateTaxiReply, on_activate_reply)
        .net_handler(K::NewTaxiPath, on_new_path)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_nodes_shown(In(ev): In<SessionEvent>, mut taxi: ResMut<TaxiState>) {
    if let SessionEvent::TaxiNodesShown {
        flightmaster,
        nearest_node,
        known_mask,
    } = ev
    {
        taxi_nodes_shown(flightmaster, nearest_node, known_mask, &mut taxi);
    }
}

fn on_node_status(In(ev): In<SessionEvent>, mut commands: Commands, index: Res<GuidIndex>) {
    if let SessionEvent::TaxiNodeStatus { guid, known } = ev {
        taxi_node_status(guid, known, &mut commands, &index);
    }
}

fn on_activate_reply(In(ev): In<SessionEvent>, mut taxi: ResMut<TaxiState>) {
    if let SessionEvent::ActivateTaxiReply { code } = ev {
        taxi_activate_reply(code, &mut taxi);
    }
}

fn on_new_path(In(ev): In<SessionEvent>, mut taxi: ResMut<TaxiState>) {
    if let SessionEvent::NewTaxiPath = ev {
        taxi_new_path(&mut taxi);
    }
}

/// An open taxi map dies with the socket. A listener on the session end
/// (a second handler on the kind, after the bridge's own teardown).
fn on_session_end(In(_): In<SessionEvent>, mut taxi: ResMut<TaxiState>) {
    taxi.clear_session();
}

/// The taxi map (`SMSG_SHOWTAXINODES`): fill the [`TaxiState`] phase 2's feed will read — the
/// trainer list's shape.
fn taxi_nodes_shown(
    flightmaster: u64,
    nearest_node: u32,
    known_mask: TaxiMask,
    taxi: &mut TaxiState,
) {
    debug!("net: taxi map on {flightmaster:#x} — nearest node {nearest_node}");
    taxi.open(flightmaster, nearest_node, known_mask);
}

/// A taxi node's known status (`SMSG_TAXINODE_STATUS`) — answers `CMSG_TAXINODE_STATUS_QUERY`
/// ([`super`]'s per-flight-master ask), and also rides the first-visit learn pair
/// alongside `SMSG_NEW_TAXI_PATH` ([`taxi_new_path`] carries that signal into `discovered`; the
/// paired `known = true` here is what clears the icon live on a learn). Upserts
/// [`super::FlightMasterStatus`] on the flight master: `known = false` shows the green
/// `TalkToMeGreen` overhead icon — the client's `0x5ecdd0` handler → `0x607480` marker swap
/// (resource table `0xc4d9d8` index 4). The client also gates the reply on the
/// unit's NPC_FLAGS bit 3 — moot here: only flight masters are ever queried or answered.
fn taxi_node_status(guid: u64, known: bool, commands: &mut Commands, index: &GuidIndex) {
    debug!("net: taxi node status — {guid:#x} known={known}");
    if let Some(&e) = index.0.get(&guid) {
        commands
            .entity(e)
            .insert(super::FlightMasterStatus { known });
    }
}

/// The activate verdict (`SMSG_ACTIVATETAXIREPLY`) — staged for phase 2's window to surface as
/// the red error line and clear (the trainer/merchant error-line pattern, folded into
/// `TaxiState.reply` since there's no other consumer yet).
fn taxi_activate_reply(code: u32, taxi: &mut TaxiState) {
    debug!("net: activate taxi reply — code {code}");
    taxi.reply = Some(code);
}

/// A first-visit "learn" landed (`SMSG_NEW_TAXI_PATH`, empty body) — vmangos's only send site
/// (`SendLearnNewTaxiNode`) always pairs this with a `SMSG_TAXINODE_STATUS(known=true)` for the
/// same flight master, so the empty body alone is the discovery signal; phase 2's presentation
/// reads the flag.
fn taxi_new_path(taxi: &mut TaxiState) {
    debug!("net: taxi — first-visit node learned (SMSG_NEW_TAXI_PATH)");
    taxi.discovered = true;
}
