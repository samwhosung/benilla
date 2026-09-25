//! The taxi map's packet handlers: they fill [`TaxiState`] and the flight masters' overhead
//! status.

use benilla_protocol::messages::TaxiMask;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::TaxiState;
use crate::net::{GuidIndex, NetHandlerApp};

/// Register the taxi handlers and the session-end listener.
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

/// The map and its staged replies end with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut taxi: ResMut<TaxiState>) {
    taxi.clear_session();
}

/// `SMSG_SHOWTAXINODES` opens the map.
fn taxi_nodes_shown(
    flightmaster: u64,
    nearest_node: u32,
    known_mask: TaxiMask,
    taxi: &mut TaxiState,
) {
    debug!("net: taxi map on {flightmaster:#x} — nearest node {nearest_node}");
    taxi.open(flightmaster, nearest_node, known_mask);
}

/// `SMSG_TAXINODE_STATUS`, the answer to `CMSG_TAXINODE_STATUS_QUERY` and the second half of a
/// first-visit learn: upsert [`super::FlightMasterStatus`], so the green icon follows `known`.
/// The reference's handler (`0x5ecdd0`) also requires the flight-master NPC flag (bit 3); only
/// flight masters are queried or answered, so it is not tested here.
fn taxi_node_status(guid: u64, known: bool, commands: &mut Commands, index: &GuidIndex) {
    debug!("net: taxi node status — {guid:#x} known={known}");
    if let Some(&e) = index.0.get(&guid) {
        commands
            .entity(e)
            .insert(super::FlightMasterStatus { known });
    }
}

/// `SMSG_ACTIVATETAXIREPLY`, staged for the feed.
fn taxi_activate_reply(code: u32, taxi: &mut TaxiState) {
    debug!("net: activate taxi reply — code {code}");
    taxi.reply = Some(code);
}

/// `SMSG_NEW_TAXI_PATH`, an empty body: vmangos sends it only on a first visit, beside a `known`
/// node status for the flight master (`SendLearnNewTaxiNode`, `TaxiHandler.cpp:117`).
fn taxi_new_path(taxi: &mut TaxiState) {
    debug!("net: taxi — first-visit node learned (SMSG_NEW_TAXI_PATH)");
    taxi.discovered = true;
}
