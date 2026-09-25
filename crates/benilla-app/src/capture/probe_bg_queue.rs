//! The battleground-queue live probe (`WOW_PROBE_BGQUEUE=1`): the fixture for a login while
//! queued. It levels the body past the bracket floor, hops to the Stormwind Warsong Gulch
//! battlemaster, greets him (`CMSG_BATTLEMASTER_HELLO`, `SMSG_BATTLEFIELD_LIST`), joins through
//! the listed guid (`CMSG_BATTLEMASTER_JOIN`) and logs `PROBE bgqueue:` with the slot it got.
//!
//! The body stays queued: vmangos only marks a logged-out entry offline
//! (`BattleGroundMgr.cpp:1768`) and sends its status again at login
//! (`BattleGroundMgr.cpp:1728`), so the next plain login is the measurement. Dequeue it with
//! `WOW_PROBE_LUA='AcceptBattlefieldPort(1,0)'` on a plain login.
//!
//! The bodyless `CMSG_BATTLEFIELD_JOIN` cannot queue: vmangos passes an empty battlemaster guid
//! (`BattleGroundHandler.cpp:78`), which the NPC interaction check never resolves.
//!
//! Non-combat; the switches are `docs/CONTRIBUTING.md`, "Running it unattended".

use bevy::prelude::*;

use super::probes::ProbeClock;
use crate::net::{ClientCommand, Guid, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::target::cursor_mode::npc_flags;
use crate::ui_battlefield::Battlefield;
use crate::ui_dialog_verbs::BattlefieldQueue;

/// Elfarran's spawn (vmangos `creature` guid 54614, entry 14981, map 0), Stormwind's Warsong
/// Gulch battlemaster; he is then found in the streamed world by his npc flag.
const BATTLEMASTER_AT: [f32; 3] = [-8454.62, 318.85, 120.97];
/// Warsong Gulch's Map.dbc row, which `GetBattleGroundTypeIdByMapId` maps to template 2.
const WSG_MAP: u32 = 489;
/// The level the body is raised to: `battleground_template` opens WSG at 10, 20 or 21 by content
/// patch, and 25 clears all three.
const QUEUE_LEVEL: u32 = 25;
/// The hop's search radius, not a gate: the server applies its own interaction range.
const NEAR_YD: f32 = 15.0;

pub(crate) struct ProbeBgQueuePlugin;

impl Plugin for ProbeBgQueuePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BgQueueProbe>()
            .add_systems(Update, bg_queue_probe);
    }
}

#[derive(Resource, Default)]
struct BgQueueProbe {
    phase: Phase,
}

/// `Wait`, `Hopped` (levelled, hop sent), `Greeted` (hello sent), `Joined` (join sent), `Done`.
#[derive(Default, PartialEq)]
enum Phase {
    #[default]
    Wait,
    Hopped {
        sent_at: f64,
    },
    Greeted {
        master: u64,
        sent_at: f64,
    },
    Joined {
        sent_at: f64,
    },
    Done,
}

fn bg_queue_probe(
    time: ProbeClock,
    mut probe: ResMut<BgQueueProbe>,
    me: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    battlefield: Res<Battlefield>,
    queue: Res<BattlefieldQueue>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<crate::net::NetCommands>,
) {
    let Ok(store) = me.single() else {
        return;
    };
    let now = time.elapsed_secs_f64();
    match probe.phase {
        Phase::Wait => {
            // Revive first: `CanInteractWithNPC` refuses a dead player (vmangos `Player.cpp:2529`)
            // with a log line that reads like a wrong guid; `.revive` on the living is a no-op.
            let _ = net.0.send(ClientCommand::Chat {
                kind: crate::net::ChatKind::Say,
                target: None,
                text: ".revive".to_string(),
            });
            // A body under the bracket floor is refused at the hello, with no list to join from.
            let level = store.0.unit_level().unwrap_or(0);
            if level < QUEUE_LEVEL {
                info!("PROBE bgqueue: level {level} — raising to {QUEUE_LEVEL} for the bracket");
                let _ = net.0.send(ClientCommand::Chat {
                    kind: crate::net::ChatKind::Say,
                    target: None,
                    text: format!(".levelup {}", QUEUE_LEVEL - level),
                });
            }
            let [x, y, z] = BATTLEMASTER_AT;
            info!("PROBE bgqueue: hopping to the Warsong Gulch battlemaster at ({x}, {y}, {z})");
            let _ = net.0.send(ClientCommand::Chat {
                kind: crate::net::ChatKind::Say,
                target: None,
                text: format!(".go xyz {x} {y} {z} 0"),
            });
            probe.phase = Phase::Hopped { sent_at: now };
        }
        Phase::Hopped { sent_at } => {
            if now - sent_at < 3.0 {
                return; // let the battlemaster stream in
            }
            let here = player.pos;
            let master = units.iter().find(|(_, net_e, store, tf)| {
                net_e.kind == benilla_protocol::EntityKind::Unit
                    && store.0.unit_npc_flags() & npc_flags::BATTLEMASTER != 0
                    && tf.translation.distance(here) < NEAR_YD
            });
            if let Some((guid, ..)) = master {
                info!(
                    "PROBE bgqueue: battlemaster {:#x} in range — greeting",
                    guid.0
                );
                let _ = net.0.send(ClientCommand::BattlemasterHello { npc: guid.0 });
                probe.phase = Phase::Greeted {
                    master: guid.0,
                    sent_at: now,
                };
            } else if now - sent_at > 15.0 {
                error!("PROBE bgqueue: FAILURE — no battlemaster streamed in within 15 s");
                probe.phase = Phase::Done;
            }
        }
        Phase::Greeted { master, sent_at } => {
            // Wait for the list: the join quotes its guid, and it proves the level gate cleared.
            if battlefield.battlemaster() == Some(master) {
                info!("PROBE bgqueue: list landed — queueing for map {WSG_MAP}");
                let _ = net.0.send(ClientCommand::BattlemasterJoin {
                    battlemaster: master,
                    map_id: WSG_MAP,
                    instance_id: 0,
                    as_group: false,
                });
                probe.phase = Phase::Joined { sent_at: now };
            } else if now - sent_at > 8.0 {
                error!(
                    "PROBE bgqueue: FAILURE — no SMSG_BATTLEFIELD_LIST 8 s after the hello \
                     (a level under the bracket floor is refused here, silently)"
                );
                probe.phase = Phase::Done;
            }
        }
        Phase::Joined { sent_at } => {
            let queued = queue
                .slots()
                .iter()
                .enumerate()
                .find_map(|(i, s)| s.as_ref().map(|(status, _)| (i, status)));
            if let Some((slot, status)) = queued {
                info!(
                    "PROBE bgqueue: QUEUED — slot {slot}, map {}, status {}, instance {} \
                     (the body stays queued; the next plain login is the measurement)",
                    status.map_id, status.status, status.instance_id,
                );
                probe.phase = Phase::Done;
            } else if now - sent_at > 15.0 {
                error!("PROBE bgqueue: FAILURE — no queue slot 15 s after CMSG_BATTLEMASTER_JOIN");
                probe.phase = Phase::Done;
            }
        }
        Phase::Done => {}
    }
}
