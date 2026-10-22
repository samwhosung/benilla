//! The taxi map's packet handlers: they fill [`TaxiState`] and the flight masters' overhead
//! status.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::TaxiState;
use crate::net::{GuidIndex, NetHandlerApp};

/// Register the taxi handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TaxiNodesShown, on_nodes_shown)
        .net_handler(K::TaxiNodesListed, on_nodes_listed)
        .net_handler(K::TaxiNodeStatus, on_node_status)
        .net_handler(K::ActivateTaxiReply, on_activate_reply)
        .net_handler(K::NewTaxiPath, on_new_path)
        .net_handler(K::Disconnected, on_session_end);
}

/// `SMSG_SHOWTAXINODES` (`0x5ece60`): the mask's known nodes counted over all 256 bits
/// (`0x5eced7`-`0x5ecf2e`); under two (`0x5ecf39`) it shows `0xb5`, `ERR_TAXINOPATHS`
/// (`0x5ecf3f`), closes the gossip menu (`0x4e1fa0` with a zero guid, `0x5ecf55`) and opens no
/// map; else the map opens.
fn on_nodes_shown(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::TaxiNodesShown {
        flightmaster,
        nearest_node,
        known_mask,
    } = ev
    {
        let known: u32 = known_mask.0.iter().map(|w| w.count_ones()).sum();
        debug!("net: taxi map on {flightmaster:#x} — nearest node {nearest_node}, {known} known");
        commands.queue(move |world: &mut World| {
            if known >= 2 {
                super::open_map(world, flightmaster, nearest_node, known_mask);
                return;
            }
            // `DisplayError` shows the line inside the handler, before the close's event.
            let _ = crate::script_calls::run_cached(world, show_no_paths);
            if world.contains_resource::<crate::ui_gossip::GossipState>() {
                crate::ui_gossip::close_core(world);
            }
        });
    }
}

/// `SMSG_SHOWTAXINODES` in mode 0 (`0x5ecf7f`): no map, only a console line per known node. The
/// console prints to the log ([`crate::console`]).
fn on_nodes_listed(In(ev): In<SessionEvent>, catalogs: Option<Res<super::TaxiCatalogs>>) {
    if let SessionEvent::TaxiNodesListed { known_mask } = ev {
        let name = |id| catalogs.as_deref().and_then(|c| c.node_name(id));
        for line in listed_nodes(&known_mask, name) {
            info!("console: {line}");
        }
    }
}

/// Mode 0's lines: for each known node from 1 to 256 (`0x5ecf95`-`0x5ed008`) that has a
/// `TaxiNodes.dbc` row (`0x5ecfb3`-`0x5ecfc9`), `[%02d]: %s` (`0x8606b0`) of its id and name,
/// through `ConsolePrintf` (`0x5ecfde`).
fn listed_nodes<'a>(
    mask: &benilla_protocol::messages::TaxiMask,
    name: impl Fn(u32) -> Option<&'a str>,
) -> Vec<String> {
    (1..=256)
        .filter(|&id| mask.is_known(id))
        .filter_map(|id| Some(format!("[{id:02}]: {}", name(id)?)))
        .collect()
}

/// `0xb5`, `ERR_TAXINOPATHS`, through `DisplayError` (`0x5ecf44` → `0x496720`), which shows it now.
fn show_no_paths(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut sink: crate::ui_action::MessageSink,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        if let Some(line) = crate::ui_action::keyed_line(script, "ERR_TAXINOPATHS") {
            crate::ui_action::show_now(script, &mut sink, "ui_taxi", line);
        }
    });
}

fn on_node_status(In(ev): In<SessionEvent>, mut commands: Commands, index: Res<GuidIndex>) {
    if let SessionEvent::TaxiNodeStatus { guid, known } = ev {
        taxi_node_status(guid, known, &mut commands, &index);
    }
}

fn on_activate_reply(
    In(ev): In<SessionEvent>,
    mut taxi: ResMut<TaxiState>,
    mut commands: Commands,
) {
    if let SessionEvent::ActivateTaxiReply { code } = ev {
        taxi_activate_reply(code, &mut taxi);
        // `OK` closes the map through the core (`0x5ed1ed` → `0x5ed203`).
        if code == benilla_protocol::messages::taxi_reply::OK {
            crate::ui_session::queue(&mut commands, super::close_core);
        }
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

/// `SMSG_ACTIVATETAXIREPLY`, staged for the feed, which shows a refusal.
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

#[cfg(test)]
mod tests {
    use benilla_protocol::messages::TaxiMask;

    use super::*;

    /// Mode 0 lists each known node with a row as `[%02d]: %s` (`0x8606b0`), in id order, the
    /// mask's bit `n` naming node `n + 1` (`0x5ecf87`); a node with no row prints nothing.
    #[test]
    fn mode_zero_lists_the_known_nodes_with_rows() {
        let mask = TaxiMask([0b1010_0010, 0, 0, 0, 0, 0, 0, 0x8000_0000]);
        let name = |id| match id {
            2 => Some("Stormwind, Elwynn"),
            8 => Some("Thelsamar, Loch Modan"),
            256 => Some("Last"),
            _ => None,
        };
        assert_eq!(
            listed_nodes(&mask, name),
            [
                "[02]: Stormwind, Elwynn",
                "[08]: Thelsamar, Loch Modan",
                "[256]: Last"
            ]
        );
    }
}
