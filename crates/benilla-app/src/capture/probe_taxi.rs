//! The taxi-flight live probe (`WOW_PROBE=taxi`): hops to Stormwind's flight master, opens the
//! menu (`CMSG_TAXIQUERYAVAILABLENODES`, `SMSG_SHOWTAXINODES`), flies Stormwind to Sentinel Hill
//! and logs a `PROBE taxi:` line at each edge. The landing verdict checks the distance to the
//! destination node's DBC position, the flight time against the path length over 32 yd/s, the
//! in-flight anims and the flying pitch and bank; then it holds `W` and logs when movement starts.
//!
//! Non-combat. `.taxicheat on` lets a fresh character fly to an unvisited node; the ~110-copper
//! fare comes from the character's own purse. The switches are `docs/CONTRIBUTING.md`, "Running
//! it unattended".

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::{load_taxi_nodes, load_taxi_path_nodes};
use bevy::prelude::*;

use super::probes::ProbeClock;
use crate::net::{ClientCommand, Guid, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_taxi::TaxiState;
use benilla_assets::{LockRecover, WorldAssets};

/// The flight under test: Stormwind (node 2) to Sentinel Hill (node 4), TaxiPath 6.
const SRC_NODE: u32 = 2;
const DEST_NODE: u32 = 4;
const TAXI_PATH: u32 = 6;
/// Dungar Longdrink's spawn (vmangos `creature` guid 79658, entry 352, map 0); the flight master
/// is then found in the streamed world by his npc flag.
const FLIGHTMASTER_AT: [f32; 3] = [-8835.8, 490.1, 109.7];
/// `UNIT_NPC_FLAG_FLIGHTMASTER` (bit 3).
const NPC_FLAG_FLIGHTMASTER: u32 = 0x8;
/// vmangos `PLAYER_FLIGHT_SPEED` in yd/s (`WaypointMovementGenerator.cpp:390`).
const FLIGHT_SPEED: f32 = 32.0;

pub(crate) struct ProbeTaxiPlugin;

impl Plugin for ProbeTaxiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TaxiProbe>()
            .add_systems(
                Startup,
                load_expectations.after(benilla_assets::AssetSet::Open),
            )
            .add_systems(Update, taxi_probe)
            .add_systems(
                PreUpdate,
                // After winit's input pass and the loading cover's input swallow.
                hold_w_post_land
                    .after(bevy::input::InputSystems)
                    .after(crate::loading_screen::CoverInput),
            );
    }
}

/// From the DBCs: the destination node's position and the path's total length.
#[derive(Default)]
struct Expectations {
    dest_pos: Option<[f32; 3]>,
    path_len: Option<f32>,
}

/// `Wait`, `Hopped` (hop sent), `Queried` (query sent), `Activated` (activate sent), `Flying`
/// (riding the spline), `PostLand`, `Done`.
#[derive(Resource, Default)]
struct TaxiProbe {
    expect: Expectations,
    phase: Phase,
}

#[derive(Default, PartialEq)]
enum Phase {
    #[default]
    Wait,
    Hopped {
        sent_at: f64,
    },
    Queried {
        flightmaster: u64,
        sent_at: f64,
        retried: bool,
    },
    Activated {
        sent_at: f64,
    },
    Flying {
        started_at: f64,
        last_report: f64,
        /// Latched once the rider plays Mount (91) and the mount child Fly (135) (`0x5fd19c`).
        gait_ok: bool,
        /// The largest flying pitch magnitude (radians) on the self transform.
        max_pitch: f32,
        /// The largest flying bank magnitude (radians), the look-ahead lean.
        max_bank: f32,
    },
    /// After landing, with `W` held ([`hold_w_post_land`]): logs height over ground, distance
    /// moved and ride state each second, and when movement began.
    PostLand {
        landed_at: f64,
        landed_pos: Vec3,
        last_log: f64,
        first_move: Option<f64>,
    },
    Done,
}

fn load_expectations(mut probe: ResMut<TaxiProbe>, world_assets: Option<Res<WorldAssets>>) {
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match load_taxi_nodes(&mut chain) {
        Ok(nodes) => probe.expect.dest_pos = nodes.get(DEST_NODE).map(|n| n.pos),
        Err(e) => warn!("PROBE taxi: TaxiNodes.dbc unavailable ({e:#}) — arrival check off"),
    }
    match load_taxi_path_nodes(&mut chain) {
        Ok(paths) => {
            probe.expect.path_len = paths.path(TAXI_PATH).map(|nodes| {
                nodes
                    .windows(2)
                    .map(|w| {
                        let (a, b) = (w[0].pos, w[1].pos);
                        let (dx, dy, dz) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
                        (dx * dx + dy * dy + dz * dz).sqrt()
                    })
                    .sum()
            });
        }
        Err(e) => warn!("PROBE taxi: TaxiPathNode.dbc unavailable ({e:#}) — duration check off"),
    }
}

#[allow(clippy::type_complexity)]
fn taxi_probe(
    time: ProbeClock,
    mut probe: ResMut<TaxiProbe>,
    self_player: Query<
        (
            Entity,
            &Guid,
            &ObjectStore,
            &Transform,
            Option<&crate::entities::mount::MountChild>,
        ),
        With<SelfPlayer>,
    >,
    player: Res<Player>,
    taxi: Res<TaxiState>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    drivers: Query<&crate::creature_anim::AnimDriver>,
    net: Res<crate::net::NetCommands>,
    spatial: avian3d::prelude::SpatialQuery,
) {
    let Ok((self_entity, self_guid, self_store, self_tf, mount_child)) = self_player.single()
    else {
        return;
    };
    let now = time.elapsed_secs_f64();
    match probe.phase {
        Phase::Wait => {
            let [x, y, z] = FLIGHTMASTER_AT;
            info!("PROBE taxi: hopping to the Stormwind flight master at ({x}, {y}, {z})");
            // The fare comes from the character's purse; a NOT_ENOUGH_MONEY reply means it is
            // empty (`.modify money` refills it).
            let _ = net
                .0
                .send(ClientCommand::SetSelection { guid: self_guid.0 });
            for text in [
                ".taxicheat on".to_string(),
                format!(".go xyz {x} {y} {z} 0"),
            ] {
                let _ = net.0.send(ClientCommand::Chat {
                    kind: crate::net::ChatKind::Say,
                    target: None,
                    text,
                });
            }
            probe.phase = Phase::Hopped { sent_at: now };
        }
        Phase::Hopped { sent_at } => {
            // Settle, then find a flight-master-flagged unit in range.
            if now - sent_at < 3.0 {
                return;
            }
            let me = player.pos;
            let fm = units.iter().find(|(_, net_e, store, tf)| {
                net_e.kind == benilla_protocol::EntityKind::Unit
                    && store.0.unit_npc_flags() & NPC_FLAG_FLIGHTMASTER != 0
                    && tf.translation.distance(me) < 15.0
            });
            if let Some((guid, ..)) = fm {
                info!(
                    "PROBE taxi: flight master {:#x} in range — querying nodes",
                    guid.0
                );
                let _ = net.0.send(ClientCommand::TaxiQueryNodes { guid: guid.0 });
                probe.phase = Phase::Queried {
                    flightmaster: guid.0,
                    sent_at: now,
                    retried: false,
                };
            } else if now - sent_at > 15.0 {
                error!("PROBE taxi: FAILURE — no flight master streamed in within 15 s");
                probe.phase = Phase::Done;
            }
        }
        Phase::Queried {
            flightmaster,
            sent_at,
            retried,
        } => {
            if let Some(open) = &taxi.open {
                let known = (1..=256).filter(|&id| open.known.is_known(id)).count();
                info!(
                    "PROBE taxi: map open — flight master {:#x}, nearest node {}, {known} known \
                     nodes",
                    open.flightmaster, open.nearest_node
                );
                if open.nearest_node != SRC_NODE {
                    warn!(
                        "PROBE taxi: nearest node {} != expected {SRC_NODE} (flying anyway)",
                        open.nearest_node
                    );
                }
                info!("PROBE taxi: activating {SRC_NODE} -> {DEST_NODE} (TaxiPath {TAXI_PATH})");
                let _ = net.0.send(ClientCommand::ActivateTaxi {
                    guid: flightmaster,
                    source_node: open.nearest_node,
                    dest_node: DEST_NODE,
                });
                probe.phase = Phase::Activated { sent_at: now };
            } else if now - sent_at > 2.0 && !retried {
                // A first visit learns the node and opens nothing (vmangos
                // `TaxiHandler.cpp:75`); the second query opens the menu.
                info!("PROBE taxi: no menu yet (first-visit learn?) — querying again");
                let _ = net
                    .0
                    .send(ClientCommand::TaxiQueryNodes { guid: flightmaster });
                probe.phase = Phase::Queried {
                    flightmaster,
                    sent_at: now,
                    retried: true,
                };
            } else if now - sent_at > 5.0 {
                error!("PROBE taxi: FAILURE — SMSG_SHOWTAXINODES never arrived");
                probe.phase = Phase::Done;
            }
        }
        Phase::Activated { sent_at } => {
            if let Some(code) = taxi.reply {
                if code == benilla_protocol::messages::taxi_reply::OK {
                    info!("PROBE taxi: ACTIVATETAXIREPLY OK");
                } else {
                    error!("PROBE taxi: FAILURE — ACTIVATETAXIREPLY code {code}");
                    probe.phase = Phase::Done;
                    return;
                }
            }
            if player.server_riding() {
                let mount = self_store.0.unit_mount_display_id();
                info!(
                    "PROBE taxi: AIRBORNE — server spline riding, mount display id {mount} \
                     (nonzero = the taxi mount landed on the wire)"
                );
                probe.phase = Phase::Flying {
                    started_at: now,
                    last_report: now,
                    gait_ok: false,
                    max_pitch: 0.0,
                    max_bank: 0.0,
                };
            } else if now - sent_at > 8.0 {
                error!(
                    "PROBE taxi: FAILURE — activate sent but no self-spline arrived (reply: {:?})",
                    taxi.reply
                );
                probe.phase = Phase::Done;
            }
        }
        Phase::Flying {
            started_at,
            last_report,
            gait_ok,
            max_pitch,
            max_bank,
        } => {
            let wow = bevy_to_wow(player.pos);
            if player.server_riding() {
                // Rider Mount (91), mount child Fly (135) (`0x5fd19c`); latched, as the first
                // frames lag the mount attach.
                let rider = drivers.get(self_entity).ok().map(|d| d.playing().0);
                let mount = mount_child
                    .and_then(|mc| drivers.get(mc.0).ok())
                    .map(|d| d.playing().0);
                let pair_ok = rider == Some(Some(91)) && mount == Some(Some(135));
                // `sample_splines` composes `Ry(f) * Rx(pitch) * Rz(bank)`, so YXZ reads it back.
                let (_, pitch, bank) = self_tf.rotation.to_euler(EulerRot::YXZ);
                let max_pitch = max_pitch.max(pitch.abs());
                let max_bank = max_bank.max(bank.abs());
                if now - last_report >= 5.0 {
                    info!(
                        "PROBE taxi: in flight {:.0}s — at ({:.1}, {:.1}, {:.1}); anims rider \
                         {rider:?} mount {mount:?}; pitch {:.2} rad (max {max_pitch:.2}); bank \
                         {:.2} rad (max {max_bank:.2})",
                        now - started_at,
                        wow[0],
                        wow[1],
                        wow[2],
                        pitch,
                        bank,
                    );
                    probe.phase = Phase::Flying {
                        started_at,
                        last_report: now,
                        gait_ok: gait_ok || pair_ok,
                        max_pitch,
                        max_bank,
                    };
                } else {
                    probe.phase = Phase::Flying {
                        started_at,
                        last_report,
                        gait_ok: gait_ok || pair_ok,
                        max_pitch,
                        max_bank,
                    };
                }
                return;
            }
            // The ride ended: `server_ride` snapped to the endpoint and sent
            // `CMSG_MOVE_SPLINE_DONE`.
            let flew_for = now - started_at;
            let dist = probe.expect.dest_pos.map(|p| {
                let (dx, dy, dz) = (wow[0] - p[0], wow[1] - p[1], wow[2] - p[2]);
                (dx * dx + dy * dy + dz * dz).sqrt()
            });
            let predicted = probe.expect.path_len.map(|l| l / FLIGHT_SPEED);
            let dist_ok = dist.is_some_and(|d| d < 20.0);
            // A 15% band: the server's duration also holds the mount-up delays.
            let time_ok =
                predicted.is_some_and(|p| (flew_for - f64::from(p)).abs() < f64::from(p) * 0.15);
            // The route climbs and turns, so each axis passes 0.05 rad somewhere along it.
            let pitch_ok = max_pitch > 0.05;
            let bank_ok = max_bank > 0.05;
            let verdict = if dist_ok && time_ok && gait_ok && pitch_ok && bank_ok {
                "SUCCESS"
            } else {
                "FAILURE"
            };
            let level_ok = |ok: bool| if ok { "ok" } else { "MISMATCH" };
            info!(
                "PROBE taxi: {verdict} — landed at ({:.1}, {:.1}, {:.1}); dist to node {DEST_NODE} \
                 {} ({}); flight {flew_for:.1}s vs predicted {} ({}); in-flight anims 91/135 ({}); \
                 max pitch {max_pitch:.2} rad ({}); max bank {max_bank:.2} rad ({})",
                wow[0],
                wow[1],
                wow[2],
                dist.map_or("n/a".into(), |d| format!("{d:.1} yd")),
                level_ok(dist_ok),
                predicted.map_or("n/a".into(), |p| format!("{p:.1}s = len/32")),
                level_ok(time_ok),
                level_ok(gait_ok),
                level_ok(pitch_ok),
                level_ok(bank_ok),
            );
            probe.phase = Phase::PostLand {
                landed_at: now,
                landed_pos: player.pos,
                last_log: 0.0,
                first_move: None,
            };
        }
        Phase::PostLand {
            landed_at,
            landed_pos,
            last_log,
            first_move,
        } => {
            let since = now - landed_at;
            let moved = (player.pos - landed_pos).with_y(0.0).length();
            let first_move = match first_move {
                None if moved > 0.5 => {
                    info!("PROBE taxi: post-land — movement began after {since:.1}s (W held from landing)");
                    Some(since)
                }
                fm => fm,
            };
            let mut last_log = last_log;
            if since - last_log >= 1.0 {
                last_log = since;
                let wow = bevy_to_wow(player.pos);
                // Ground probe: from 2 yd above the feet, 50 yd down.
                let origin = player.pos + Vec3::Y * 2.0;
                let ground = spatial
                    .cast_ray_predicate(
                        origin,
                        Dir3::NEG_Y,
                        52.0,
                        true,
                        &benilla_world::collision::WorldCollision::body_filter(),
                        &|_| true,
                    )
                    .map(|hit| origin.y - hit.distance);
                let height = ground.map(|g| player.pos.y - g);
                info!(
                    "PROBE taxi: post-land {since:.1}s — at ({:.1}, {:.1}, {:.1}); height over ground {}; moved {moved:.1} yd; riding={} mount={}",
                    wow[0],
                    wow[1],
                    wow[2],
                    height.map_or("n/a".into(), |h| format!("{h:.2} yd")),
                    player.server_riding(),
                    self_store.0.unit_mount_display_id(),
                );
            }
            if since > 8.0 {
                info!(
                    "PROBE taxi: post-land verdict — movement began {} after landing",
                    match first_move {
                        Some(t) => format!("{t:.1}s"),
                        None => "NEVER (still locked at 8s)".into(),
                    }
                );
                probe.phase = Phase::Done;
            } else {
                probe.phase = Phase::PostLand {
                    landed_at,
                    landed_pos,
                    last_log,
                    first_move,
                };
            }
        }
        Phase::Done => {}
    }
}

/// Holds `W` from the landing verdict on, in `PreUpdate` after winit's input pass so the
/// controller sees it the same frame.
fn hold_w_post_land(probe: Res<TaxiProbe>, mut keys: ResMut<ButtonInput<KeyCode>>) {
    if matches!(probe.phase, Phase::PostLand { .. }) {
        keys.press(KeyCode::KeyW);
    } else if keys.pressed(KeyCode::KeyW) && matches!(probe.phase, Phase::Done) {
        keys.release(KeyCode::KeyW);
    }
}
