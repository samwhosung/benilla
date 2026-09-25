//! The sea-crossing live probe (`WOW_PROBE=crossing`): once in-world, waits for a cross-continent
//! transport docked on our map, drops onto its deck with `.go xyz`, and logs a `PROBE crossing:`
//! line at each edge: aboard, map flip (the riding `NEW_WORLD` branch), still riding, docked on the
//! far continent. Non-combat. It needs the checkout's own probe account, since a shared one is
//! kicked mid-ride; the switches are `docs/CONTRIBUTING.md`, "Running it unattended".

use bevy::prelude::*;

use super::probes::ProbeClock;
use crate::net::{ClientCommand, Guid, SelfPlayer};
use crate::player::Player;
use crate::transport::{Transport, TransportAnchor};
use benilla_world::world_map::CurrentMap;

pub(crate) struct ProbeCrossingPlugin;

impl Plugin for ProbeCrossingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CrossingProbe>()
            .add_systems(Update, crossing_probe);
    }
}

/// `Wait`, `Boarding` (drop sent), `Aboard` (ride attached), `Crossed` (map flipped, still
/// riding), `Done`. A missed boarding retries at the next dock; a ride lost after the flip FAILs.
#[derive(Resource, Default)]
struct CrossingProbe {
    phase: Phase,
    /// Latches the one dock hop, so a body that still sees no ferry does not hop every frame.
    bootstrapped: bool,
}

#[derive(Default, PartialEq)]
enum Phase {
    #[default]
    Wait,
    Boarding {
        boat: u64,
        sent_at: f64,
    },
    Aboard {
        boat: u64,
        start_map: u32,
    },
    Crossed {
        boat: u64,
        to_map: u32,
    },
    Done,
}

/// Yards above the boat's sampled origin the drop aims. A sea ferry's path nodes sit at the
/// waterline (path 241 is all `z = 0`) with the deck ~16 yd above, while zeppelin nodes sit at deck
/// height; landing anywhere on the boat counts, so overshooting is harmless.
const DROP_HEIGHT: f32 = 25.0;
/// Seconds after the drop before the boarding counts as missed (6 s settle, fall, attach, slack).
const BOARD_DEADLINE: f64 = 15.0;
/// The Booty Bay pier beside the Ratchet ferry's berth (WoW coords), where the probe goes when no
/// cross-continent transport is in range.
const BOOTSTRAP_DOCK: [f32; 4] = [-14297.2, 531.0, 8.8, 0.0];
/// `WOW_PROBE_DOCK=x,y,z[,map]`: go to this dock first, to measure a named ferry's seam (one
/// path crosses mid-cycle, another at the cycle wrap). `map` defaults to 0 and is always sent:
/// `.go xyz` without one stays on the current map (vmangos `TeleportCommands.cpp:871`).
fn dock_override() -> Option<[f32; 4]> {
    let raw = std::env::var("WOW_PROBE_DOCK").ok()?;
    let mut it = raw.split(',').map(|p| p.trim().parse::<f32>());
    match (it.next(), it.next(), it.next(), it.next(), it.next()) {
        (Some(Ok(x)), Some(Ok(y)), Some(Ok(z)), None, _) => Some([x, y, z, 0.0]),
        (Some(Ok(x)), Some(Ok(y)), Some(Ok(z)), Some(Ok(m)), None) => Some([x, y, z, m]),
        _ => {
            warn!("WOW_PROBE_DOCK={raw:?} is not `x,y,z` or `x,y,z,map` — ignored");
            None
        }
    }
}
/// How near [`dock_override`]'s dock the body must stand before boarding anything.
const DOCK_ARRIVED_YD: f32 = 200.0;
/// How near a docked ferry must be to drop onto it: a long pier, short of the next dock.
const BOARDABLE_YD: f32 = 400.0;
/// Seconds to wait for the login's object stream to show a ferry before going to
/// [`BOOTSTRAP_DOCK`].
const BOOTSTRAP_AFTER: f64 = 20.0;

fn crossing_probe(
    time: ProbeClock,
    mut probe: ResMut<CrossingProbe>,
    self_player: Query<(), With<SelfPlayer>>,
    player: Res<Player>,
    current_map: Option<Res<CurrentMap>>,
    transports: Query<(&Guid, &Transport, &TransportAnchor)>,
    net: Res<crate::net::NetCommands>,
) {
    if self_player.is_empty() {
        return;
    }
    let Some(map) = current_map.as_deref().map(|m| m.0) else {
        return;
    };
    let now = time.elapsed_secs_f64();
    match probe.phase {
        Phase::Wait => {
            // With no sea-crossing transport in range, go to a dock once.
            let forced = dock_override();
            let adrift = !transports
                .iter()
                .any(|(_, t, _)| t.touches_map(0) && t.touches_map(1));
            // A named dock hops at once, and nothing is boarded until the body stands there: the
            // `.go` is a round trip, and the login spot's own ferry is still in range meanwhile.
            if let Some(dock) = forced {
                let here = benilla_assets::coords::bevy_to_wow(player.pos);
                let (dx, dy) = (here[0] - dock[0], here[1] - dock[1]);
                if (dx * dx + dy * dy).sqrt() > DOCK_ARRIVED_YD {
                    if probe.bootstrapped {
                        return; // hop sent, still in flight
                    }
                } else {
                    probe.bootstrapped = true; // arrived; fall through to the scan
                }
            }
            let hop_now = forced.is_some() || (adrift && now > BOOTSTRAP_AFTER);
            if hop_now && !probe.bootstrapped {
                probe.bootstrapped = true;
                let [x, y, z, m] = forced.unwrap_or(BOOTSTRAP_DOCK);
                info!(
                    "PROBE crossing: going to the dock ({x:.1}, {y:.1}, {z:.1}) on map {m} — {}",
                    if forced.is_some() {
                        "WOW_PROBE_DOCK"
                    } else {
                        "no cross-continent transport in range"
                    }
                );
                // Always send the map id: `.go xyz` without one stays on the current map.
                let _ = net.0.send(ClientCommand::Chat {
                    kind: crate::net::ChatKind::Say,
                    target: None,
                    text: format!(".go xyz {x} {y} {z} {}", m as i32),
                });
                return;
            }
            if adrift {
                return;
            }
            // A transport touching maps 0 and 1, docked on our map: drop onto its deck.
            // `sample.pos` is in WoW coords, as `.go xyz` takes.
            for (guid, transport, anchor) in &transports {
                if !(transport.touches_map(0) && transport.touches_map(1)) {
                    continue;
                }
                let sample = transport.sample_at(anchor, map);
                if sample.map != map || sample.moving {
                    continue;
                }
                // Only a nearby ferry: after a cross-continent `.go` the departed surroundings'
                // objects stay in the entity list until the server removes them.
                let here = benilla_assets::coords::bevy_to_wow(player.pos);
                let (dx, dy) = (here[0] - sample.pos[0], here[1] - sample.pos[1]);
                if (dx * dx + dy * dy).sqrt() > BOARDABLE_YD {
                    continue;
                }
                let [x, y, z] = sample.pos;
                info!(
                    "PROBE crossing: boat {:#x} docked on map {map} at ({x:.1}, {y:.1}, {z:.1}) \
                     — dropping onto its deck",
                    guid.0
                );
                let _ = net.0.send(ClientCommand::Chat {
                    kind: crate::net::ChatKind::Say,
                    target: None,
                    text: format!(".go xyz {x} {y} {} ", z + DROP_HEIGHT),
                });
                probe.phase = Phase::Boarding {
                    boat: guid.0,
                    sent_at: now,
                };
                break;
            }
        }
        Phase::Boarding { boat, sent_at } => {
            if player.riding() == Some(boat) {
                info!("PROBE crossing: aboard {boat:#x} on map {map} — riding to the seam");
                probe.phase = Phase::Aboard {
                    boat,
                    start_map: map,
                };
            } else if now - sent_at > BOARD_DEADLINE {
                info!("PROBE crossing: boarding missed the window — waiting for the next dock");
                probe.phase = Phase::Wait;
            }
        }
        Phase::Aboard { boat, start_map } => {
            if map != start_map {
                if player.riding() == Some(boat) {
                    info!("PROBE crossing: map flipped {start_map} → {map} STILL ABOARD {boat:#x}");
                    probe.phase = Phase::Crossed { boat, to_map: map };
                } else {
                    error!(
                        "PROBE crossing: FAILURE — map flipped {start_map} → {map} but the ride \
                         did not survive the seam"
                    );
                    probe.phase = Phase::Done;
                }
            } else if player.riding() != Some(boat) {
                info!("PROBE crossing: lost the deck before the seam — re-boarding");
                probe.phase = Phase::Wait;
            }
        }
        Phase::Crossed { boat, to_map } => {
            if player.riding() != Some(boat) {
                error!("PROBE crossing: FAILURE — detached after the flip, before the far dock");
                probe.phase = Phase::Done;
            } else if let Some((_, transport, anchor)) =
                transports.iter().find(|(g, ..)| g.0 == boat)
            {
                let sample = transport.sample_at(anchor, map);
                if sample.map == to_map && !sample.moving {
                    info!(
                        "PROBE crossing: SUCCESS — docked on map {to_map} still aboard {boat:#x}"
                    );
                    probe.phase = Phase::Done;
                }
            }
        }
        Phase::Done => {}
    }
}
