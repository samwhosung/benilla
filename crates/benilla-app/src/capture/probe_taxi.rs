//! The taxi-flight live probe (`WOW_PROBE=taxi`) — decision 0484's end-to-end instrument, inert
//! without the env: once in-world, GM-hop to the Stormwind flight master (Dungar Longdrink),
//! open the taxi menu on the real wire (`CMSG_TAXIQUERYAVAILABLENODES` → `SMSG_SHOWTAXINODES`),
//! activate the short verified hop Stormwind → Sentinel Hill (nodes 2 → 4, TaxiPath 6), and ride
//! the server's flying spline to arrival. Every phase edge prints a `PROBE taxi:` line and the
//! landing prints a SUCCESS/FAILURE verdict with the two machine checks of the 0484 gate:
//! arrival distance to the destination node's DBC position, and measured flight duration vs the
//! DBC prediction `Σ path-segment length ÷ 32 yd/s` (`PLAYER_FLIGHT_SPEED`) — timing measured,
//! never eyeballed. An outer `timeout`d run + grep is the whole harness.
//! Non-combat. Pair with the checkout's probe identity (`.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR — the `probe` skill). Uses `.taxicheat on` so
//! the fresh probe character can fly to an unvisited node (and so the SHOWTAXINODES mask
//! exercises the full-network branch); the 110-copper fare is DB-seeded (see the Wait phase —
//! `.modify money` outranks the probe account).

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::{load_taxi_nodes, load_taxi_path_nodes};
use bevy::prelude::*;

use super::probes::ProbeClock;
use crate::net::{ClientCommand, Guid, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_taxi::TaxiState;
use benilla_assets::{LockRecover, WorldAssets};

/// The flight under test: Stormwind (node 2) → Sentinel Hill (node 4), TaxiPath id 6 — the pair
/// byte-verified against the real 5875 tables by the phase-1 catalog tests.
const SRC_NODE: u32 = 2;
const DEST_NODE: u32 = 4;
const TAXI_PATH: u32 = 6;
/// Dungar Longdrink's spawn (vmangos `creature` guid 79658, entry 352, map 0) — the `.go xyz`
/// target; the flight master is then scanned from the streamed world by its npc flag, never by a
/// hardcoded guid.
const FLIGHTMASTER_AT: [f32; 3] = [-8835.8, 490.1, 109.7];
/// `UNIT_NPC_FLAG_FLIGHTMASTER` (bit 3) — the same bit the cursor classifier keys on.
const NPC_FLAG_FLIGHTMASTER: u32 = 0x8;
/// vmangos `PLAYER_FLIGHT_SPEED` (yd/s, hardcoded server-side) — the duration prediction's
/// divisor.
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
                // After the loading cover's input swallow as well as winit's input pass — see
                // `probes::act::ProbeKeyPlugin` for why a synthetic press orders past it.
                hold_w_post_land
                    .after(bevy::input::InputSystems)
                    .after(crate::loading_screen::CoverInput),
            );
    }
}

/// The DBC-derived expectations, loaded once off the patch chain: the destination node's world
/// position (the arrival assert) and the path's total length (the duration prediction).
#[derive(Default)]
struct Expectations {
    dest_pos: Option<[f32; 3]>,
    path_len: Option<f32>,
}

/// The probe's phase machine. `Wait` → (GM hop sent) `Hopped` → (flight master streamed in,
/// query sent) `Queried` → (map opened, activate sent) `Activated` → (self-spline riding)
/// `Flying` → (ride ended) verdict → `Done`.
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
        /// Latched true the first in-flight frame the anim pair reads right — rider base
        /// Mount(91), mount child base Fly(135) (`0x5fd19c` + the 0441 mount pin).
        gait_ok: bool,
        /// The largest |flying pitch| (radians) seen on the SELF transform mid-flight — the
        /// `sample_splines` tangent-climb attitude. The route climbs
        /// Westfall's hills, so a working tilt shows ≳0.1 rad; ~0 means it never rendered.
        max_pitch: f32,
        /// The largest |flying BANK| (radians) mid-flight — the 0516 look-ahead lean. The route
        /// turns repeatedly, so a working bank shows ≳0.05 rad; ~0 means no lean rendered.
        max_bank: f32,
    },
    /// Post-landing diagnosis (the director's "we float a char height and can't move for ~5 s"):
    /// W is held synthetically ([`hold_w_post_land`]) from the moment the verdict prints; each
    /// second this logs the pose — height over the ground (a conform-style down-ray), distance
    /// from the landing point, ride/mount state — and stamps when movement actually began.
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

// One Bevy system's full input set (the crossing-probe shape) + its self query tuple.
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
            // The fare is seeded offline in `characters.money` (100000 copper; a flight spends
            // ~110) — re-seed via `UPDATE characters SET money=100000 WHERE name LIKE 'Probe%'
            // AND online=0;` if a NOT_ENOUGH_MONEY reply ever shows up. The seed used to be the
            // *only* option: `.modify money` is SEC_BASIC_ADMIN(4) and the probe accounts were
            // gmlevel 3 (two runs bounced on NOT_ENOUGH_MONEY before that was traced). Since they
            // were actually raised to 6 this probe could grant its own fare in-band the way
            // `probe_bank` now does — left alone because the offline seed works and is one fewer
            // command inside the flight window.
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
            // Post-teleport settle, then scan the streamed world for a flight-master-flagged
            // unit in interaction range — the guid comes from the wire, never hardcoded.
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
                // First contact LEARNS, never opens (vmangos SendLearnNewTaxiNode) — the second
                // query opens the menu.
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
                // The anim pair the flight must show (`0x5fd19c` + the 0441 mount pin):
                // rider base Mount(91), mount child base Fly(135). Latched — the first frames
                // legitimately lag (mount attach, first selection).
                let rider = drivers.get(self_entity).ok().map(|d| d.playing().0);
                let mount = mount_child
                    .and_then(|mc| drivers.get(mc.0).ok())
                    .map(|d| d.playing().0);
                let pair_ok = rider == Some(Some(91)) && mount == Some(Some(135));
                // The flying attitude: `sample_splines` composes
                // `Ry(f)·Rx(pitch)·Rz(bank)`, so the YXZ euler reads back (yaw, pitch, bank).
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
            // The ride ended (server_ride snapped to the endpoint and sent CMSG_MOVE_SPLINE_DONE
            // — its own log line). Verdict time.
            let flew_for = now - started_at;
            let dist = probe.expect.dest_pos.map(|p| {
                let (dx, dy, dz) = (wow[0] - p[0], wow[1] - p[1], wow[2] - p[2]);
                (dx * dx + dy * dy + dz * dz).sqrt()
            });
            let predicted = probe.expect.path_len.map(|l| l / FLIGHT_SPEED);
            let dist_ok = dist.is_some_and(|d| d < 20.0);
            // The server's duration also stretches with the mount-up delays and our
            // spline-arrival timing; a ±15% band catches a wrong curve/speed without flaking.
            let time_ok =
                predicted.is_some_and(|p| (flew_for - f64::from(p)).abs() < f64::from(p) * 0.15);
            // The route climbs Westfall's hills AND turns repeatedly: a working flying attitude
            // shows well over 0.05 rad (~3°) on each axis somewhere along it; ~0 means that
            // axis never rendered.
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
                // Conform-style ground probe: from 2 yd above the feet, 50 yd down.
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

/// Hold W from the landing verdict on (PreUpdate, after winit's input processing so the synthetic
/// press is visible to the controller the same frame — the [`super::probes::ProbeKeyPlugin`]
/// pattern): the post-land phase measures when the avatar actually starts moving.
fn hold_w_post_land(probe: Res<TaxiProbe>, mut keys: ResMut<ButtonInput<KeyCode>>) {
    if matches!(probe.phase, Phase::PostLand { .. }) {
        keys.press(KeyCode::KeyW);
    } else if keys.pressed(KeyCode::KeyW) && matches!(probe.phase, Phase::Done) {
        keys.release(KeyCode::KeyW);
    }
}
