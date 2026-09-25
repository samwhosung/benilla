//! The guard-directions live probe (`WOW_PROBE=guardpoi`) for [`crate::poi_marker`]: hops to a
//! Stormwind City Guard, opens his gossip (`CMSG_GOSSIP_HELLO`, `SMSG_GOSSIP_MESSAGE`), clicks the
//! "Weapons Trainer" option and checks the `SMSG_GOSSIP_POI` marker field by field against
//! `points_of_interest` row 808, then logs which minimap draw it gets from here.
//!
//! Non-combat; grep `PROBE guardpoi:`. The switches are `docs/CONTRIBUTING.md`, "Running it
//! unattended".

use bevy::prelude::*;

use super::probes::ProbeClock;
use crate::net::{ClientCommand, Guid, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::poi_marker::PoiMarker;
use crate::ui_gossip::GossipState;

/// A Stormwind City Guard's spawn (vmangos `creature` guid 79664, entry 68, map 0); the guard is
/// then found in the streamed world by his gossip flag.
const GUARD_AT: [f32; 3] = [-8854.14, 541.299, 105.984];
/// `UNIT_NPC_FLAG_GOSSIP` (bit 0).
const NPC_FLAG_GOSSIP: u32 = 0x1;
/// The option clicked, matched by label: gossip menu 435's "Weapons Trainer", `action_poi_id` 808.
const OPTION_LABEL: &str = "Weapons Trainer";
/// `points_of_interest` row 808.
const EXPECT_NAME: &str = "Woo Ping";
const EXPECT_POS: [f32; 2] = [-8796.2, 613.098];
const EXPECT_ICON: u32 = 6; // ICON_POI_REDFLAG
const EXPECT_FLAGS: u32 = 99; // bits 0 (a candidate) and 1 (draw the in-range icon)
/// The in/out-of-range split of the minimap's landmark pass (`0x811730`).
const BLIP_EDGE_RATIO: f32 = 0.8;

pub(crate) struct ProbeGuardPoiPlugin;

impl Plugin for ProbeGuardPoiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GuardPoiProbe>()
            .add_systems(Update, guard_poi_probe);
    }
}

#[derive(Resource, Default)]
struct GuardPoiProbe {
    phase: Phase,
}

/// `Wait`, `Hopped` (hop sent), `Greeted` (hello sent), `Asked` (option clicked), `Done`.
#[derive(Default, PartialEq)]
enum Phase {
    #[default]
    Wait,
    Hopped {
        sent_at: f64,
    },
    Greeted {
        guard: u64,
        sent_at: f64,
    },
    Asked {
        sent_at: f64,
    },
    Done,
}

fn guard_poi_probe(
    time: ProbeClock,
    mut probe: ResMut<GuardPoiProbe>,
    self_player: Query<(), With<SelfPlayer>>,
    player: Res<Player>,
    gossip: Res<GossipState>,
    marker: Res<PoiMarker>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<crate::net::NetCommands>,
) {
    if self_player.single().is_err() {
        return;
    }
    let now = time.elapsed_secs_f64();
    match probe.phase {
        Phase::Wait => {
            let [x, y, z] = GUARD_AT;
            info!("PROBE guardpoi: hopping to a Stormwind City Guard at ({x}, {y}, {z})");
            let _ = net.0.send(ClientCommand::Chat {
                kind: crate::net::ChatKind::Say,
                target: None,
                text: format!(".go xyz {x} {y} {z} 0"),
            });
            probe.phase = Phase::Hopped { sent_at: now };
        }
        Phase::Hopped { sent_at } => {
            if now - sent_at < 3.0 {
                return; // let the guard stream in
            }
            let me = player.pos;
            let guard = units.iter().find(|(_, net_e, store, tf)| {
                net_e.kind == benilla_protocol::EntityKind::Unit
                    && store.0.unit_npc_flags() & NPC_FLAG_GOSSIP != 0
                    && tf.translation.distance(me) < 15.0
            });
            if let Some((guid, ..)) = guard {
                info!(
                    "PROBE guardpoi: guard {:#x} in range — asking for directions",
                    guid.0
                );
                let _ = net.0.send(ClientCommand::GossipHello { guid: guid.0 });
                probe.phase = Phase::Greeted {
                    guard: guid.0,
                    sent_at: now,
                };
            } else if now - sent_at > 15.0 {
                error!("PROBE guardpoi: FAILURE — no gossip NPC streamed in within 15 s");
                probe.phase = Phase::Done;
            }
        }
        Phase::Greeted { guard, sent_at } => {
            if gossip.npc == Some(guard) && !gossip.options.is_empty() {
                let labels: Vec<&str> = gossip.options.iter().map(|o| o.message.as_str()).collect();
                info!(
                    "PROBE guardpoi: menu open — {} options: {labels:?}",
                    labels.len()
                );
                let Some(option) = gossip
                    .options
                    .iter()
                    .find(|o| o.message == OPTION_LABEL)
                    .map(|o| o.index)
                else {
                    error!("PROBE guardpoi: FAILURE — no \"{OPTION_LABEL}\" option on this menu");
                    probe.phase = Phase::Done;
                    return;
                };
                info!("PROBE guardpoi: clicking \"{OPTION_LABEL}\" (option {option})");
                let _ = net.0.send(ClientCommand::GossipSelectOption {
                    guid: guard,
                    option,
                });
                probe.phase = Phase::Asked { sent_at: now };
            } else if now - sent_at > 8.0 {
                error!("PROBE guardpoi: FAILURE — SMSG_GOSSIP_MESSAGE never arrived");
                probe.phase = Phase::Done;
            }
        }
        Phase::Asked { sent_at } => {
            let Some(poi) = &marker.poi else {
                if now - sent_at > 8.0 {
                    error!(
                        "PROBE guardpoi: FAILURE — no marker 8 s after the click (no \
                            SMSG_GOSSIP_POI, or it was cleared on arrival)"
                    );
                    probe.phase = Phase::Done;
                }
                return;
            };
            // Every field against the server's row, so a mis-ordered parse fails.
            let mut wrong: Vec<String> = Vec::new();
            if poi.name != EXPECT_NAME {
                wrong.push(format!("name {:?} != {EXPECT_NAME:?}", poi.name));
            }
            let off = ((poi.pos[0] - EXPECT_POS[0]).powi(2) + (poi.pos[1] - EXPECT_POS[1]).powi(2))
                .sqrt();
            if off > 0.5 {
                wrong.push(format!(
                    "pos ({:.2}, {:.2}) is {off:.2} yd off ({}, {})",
                    poi.pos[0], poi.pos[1], EXPECT_POS[0], EXPECT_POS[1]
                ));
            }
            if poi.icon != EXPECT_ICON {
                wrong.push(format!("icon {} != {EXPECT_ICON}", poi.icon));
            }
            if poi.flags != EXPECT_FLAGS {
                wrong.push(format!("flags {} != {EXPECT_FLAGS}", poi.flags));
            }

            // The minimap's landmark pass from here, at the default zoom's 133.3-yd view radius.
            let w = benilla_assets::coords::bevy_to_wow(player.pos);
            let d = ((poi.pos[0] - w[0]).powi(2) + (poi.pos[1] - w[1]).powi(2)).sqrt();
            let radius = 133.3;
            let draw = match d / radius <= BLIP_EDGE_RATIO {
                true if poi.flags & 0x2 != 0 => {
                    format!("POIIcons cell {} at its true spot", poi.icon)
                }
                true => "nothing (in range, no Flags&2)".to_string(),
                false => "the gold guide arrow on the rim, pointing at it (1519)".to_string(),
            };
            info!(
                "PROBE guardpoi: marker \"{}\" at ({:.1}, {:.1}) icon {} flags {} — {d:.1} yd away \
                 (ratio {:.2} of the {radius:.0}-yd view) → minimap draws {draw}",
                poi.name,
                poi.pos[0],
                poi.pos[1],
                poi.icon,
                poi.flags,
                d / radius,
            );
            if wrong.is_empty() {
                info!(
                    "PROBE guardpoi: SUCCESS — the guard's directions match points_of_interest 808"
                );
            } else {
                error!("PROBE guardpoi: FAILURE — {}", wrong.join("; "));
            }
            probe.phase = Phase::Done;
        }
        Phase::Done => {}
    }
}
