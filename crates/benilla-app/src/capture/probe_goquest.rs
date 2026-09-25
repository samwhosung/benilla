//! The GameObject-questgiver live probe (`WOW_PROBE_GOQUEST=1`). The reference asks about a
//! quest-giving GameObject as it does a creature: the sweep sends `CMSG_QUESTGIVER_STATUS_QUERY`
//! for any GameObject with `GAMEOBJECT_FLAGS` bit 2 whose reaction toward us is `> 1`. It then
//! refuses the answer: its handler `0x5dc9f0` resolves the guid with typemask 8, a GameObject's
//! `0x21` misses bit 3, and the packet dies at `0x5dca2f`. Real 1.12 captures carry no GameObject
//! status at all, but vmangos answers one (`TYPEMASK_CREATURE_OR_GAMEOBJECT`,
//! `QuestHandler.cpp:40`), so without the refusal a wanted poster would wear a `!`. The probe tells
//! "asked and refused" from "never asked" through [`QuestGiver::refused_for`], keyed by guid since
//! one drain answers several quest objects.
//!
//! Four readings:
//!
//! 1. LOW: at level [`LOW_LEVEL`], below the quest's `MinLevel`, the server answers 1
//!    (`UNAVAILABLE`) and we refuse it.
//! 2. HIGH: past `MinLevel` the answer changes to 5 (`AVAILABLE`), so it is a live query.
//! 3. NO STATUS: `QuestGiver::status()` for the poster stays `None` every frame; the marker layer
//!    and the minimap dot both draw from it.
//! 4. UNIT CONTROL: a creature questgiver in the same scene still gets and keeps its status.
//!
//! `WOW_PROBE_GOQUEST=<x>,<y>,<z>[,<map>]` aims it elsewhere; the default is the Goldshire wanted
//! poster (`gameobject` 26843, template 68, type 2, flags 4, quest 176 with `MinLevel` 5 and
//! `QuestLevel` 11), a single unpooled spawn, with questgiver creatures nearby for the control.
//! Each window sets the level and bumps the re-ask epoch to force a sweep, the only way a
//! GameObject guid reaches the wire; the starting level is restored on exit. It logs
//! `PROBE_GOQUEST:` lines and exits; the switches are `docs/CONTRIBUTING.md`, "Running it
//! unattended".
//!
//! vmangos ORs `IsGameMaster()` into a GameObject's `IsActivateToQuest` (`Object.cpp:652`), so in
//! GM mode `GAMEOBJECT_DYN_FLAGS` reads `0x1` on every quest object; the dialog status has no GM
//! term, so the readings are unaffected.

use bevy::prelude::*;

use benilla_protocol::EntityKind;

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_quest::QuestGiver;

/// The probe's default object: the `Wanted Poster` outside the Goldshire inn (`gameobject` 26843,
/// template 68, quest 176). `[x, y, z, map]`.
const POSTER_AT: [f32; 4] = [-9668.23, 683.39, 36.33, 0.0];
/// `GAMEOBJECT_TYPE_QUESTGIVER`, the type a wanted poster carries.
const GO_TYPE_QUESTGIVER: i32 = 2;
/// `GAMEOBJECT_FLAGS` bit 2, the reference's GameObject query gate (`0x5eb0f2`).
const GO_FLAG_INTERACT_COND: u32 = 0x4;
/// `UNIT_NPC_FLAGS` questgiver bit, how the unit control is found.
const NPC_FLAG_QUESTGIVER: u32 = 0x2;
/// The two windows' levels, against quest 176's `MinLevel` 5 and `QuestLevel` 11: below it the
/// server answers `UNAVAILABLE`, above it `AVAILABLE` (not `CHAT`, which needs
/// `level > QuestLevel + Quests.LowLevelHideDiff`).
const LOW_LEVEL: u32 = 2;
const HIGH_LEVEL: u32 = 8;
/// `DialogStatus` ids the verdict is written in (the keys of the status map `0x80c454`).
const STATUS_UNAVAILABLE: u32 = 1;
const STATUS_AVAILABLE: u32 = 5;

const SETTLE_SECS: f64 = 5.0;
const SCAN_TIMEOUT_SECS: f64 = 25.0;
/// How long a window waits for the refused answer; nothing arriving means it was never sent.
const STATUS_TIMEOUT_SECS: f64 = 15.0;
/// How long a `.levelup` is given to come back down the wire as a `UNIT_FIELD_LEVEL` change.
const LEVEL_TIMEOUT_SECS: f64 = 15.0;
/// Scan radius around the landing spot, in yards.
const SCAN_RANGE: f32 = 40.0;

pub(crate) struct ProbeGoQuestPlugin;

impl Plugin for ProbeGoQuestPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GoQuestProbe>()
            .add_systems(Update, goquest_probe);
    }
}

#[derive(Resource, Default)]
struct GoQuestProbe {
    phase: Phase,
    /// The poster's guid, once the scan finds it.
    poster: Option<u64>,
    /// The unit control's guid, a questgiver-flagged creature in the same scene.
    control: Option<u64>,
    /// The level the character was at when the probe started, restored before it exits.
    start_level: Option<u32>,
    /// The refused status read in each window.
    low: Option<u32>,
    high: Option<u32>,
    /// Whether the poster ever acquired a stored status (it must not) and the control's own stored
    /// status (it must have one).
    poster_ever_stored: bool,
    control_stored: Option<u32>,
    fails: u32,
    exited: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// `.go` issued; letting the world stream the poster in.
    Settling {
        sent_at: f64,
    },
    /// Levelling into a window's level, then sweeping.
    Level {
        to: u32,
        since: f64,
    },
    /// Waiting for the refused answer at the window's level.
    Read {
        level: u32,
        since: f64,
    },
    /// Levelling back to where the character started.
    Restore {
        since: f64,
    },
    Done,
}

/// Where the probe is aimed: `WOW_PROBE_GOQUEST=<x>,<y>,<z>[,<map>]`, else [`POSTER_AT`] (also
/// for `1`).
fn target() -> [f32; 4] {
    let Ok(raw) = std::env::var("WOW_PROBE_GOQUEST") else {
        return POSTER_AT;
    };
    let parts: Vec<f32> = raw
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    match parts.len() {
        3 => [parts[0], parts[1], parts[2], POSTER_AT[3]],
        4 => [parts[0], parts[1], parts[2], parts[3]],
        _ => POSTER_AT,
    }
}

/// `.levelup <delta>` on the probe's own character; nothing is selected, so the command's
/// creature branch (`GetSelectedCreature`) cannot fire.
fn level_to(net: &NetCommands, from: u32, to: u32) {
    let delta = i64::from(to) - i64::from(from);
    if delta == 0 {
        return;
    }
    let _ = net.0.send(ClientCommand::Chat {
        kind: ChatKind::Say,
        target: None,
        text: format!(".levelup {delta}"),
    });
}

/// One window's verdict line.
fn report(label: &str, expect: u32, seen: Option<u32>) -> u32 {
    match seen {
        Some(s) if s == expect => {
            info!("PROBE_GOQUEST: {label:<4} PASS — the server answered {s}, and we refused it");
            0
        }
        Some(s) => {
            error!("PROBE_GOQUEST: {label:<4} FAIL — refused status {s}, expected {expect}");
            1
        }
        None => {
            error!(
                "PROBE_GOQUEST: {label:<4} FAIL — no status ever arrived for the object (expected \
                 {expect}). vmangos answers every status query it can resolve, so nothing arriving \
                 means the query was never SENT — the sweep's GameObject leg is not firing"
            );
            1
        }
    }
}

fn goquest_probe(
    time: ProbeClock,
    mut probe: ResMut<GoQuestProbe>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    objects: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    mut quest: ResMut<QuestGiver>,
    net: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(me) = self_store.single() else {
        return; // not in-world yet
    };
    let Some(level) = me.0.unit_level() else {
        return; // our own descriptor hasn't landed
    };
    let now = time.elapsed_secs_f64();
    // Sampled every frame: a status stored for one frame is still a rendered `!`.
    if probe.poster.is_some_and(|p| quest.status(p).is_some()) {
        probe.poster_ever_stored = true;
    }
    if let Some(s) = probe.control.and_then(|c| quest.status(c)) {
        probe.control_stored = Some(s);
    }

    match probe.phase {
        Phase::Wait => {
            let [x, y, z, map] = target();
            probe.start_level = Some(level);
            info!(
                "PROBE_GOQUEST: heading to the quest object ({x} {y} {z} map {map}) — \
                 GameObject type {GO_TYPE_QUESTGIVER}, starting level {level}"
            );
            let _ = net.0.send(ClientCommand::Chat {
                kind: ChatKind::Say,
                target: None,
                text: format!(".go xyz {x} {y} {z} {map}"),
            });
            probe.phase = Phase::Settling { sent_at: now };
        }
        Phase::Settling { sent_at } => {
            if now - sent_at < SETTLE_SECS {
                return;
            }
            let here = player.pos;
            let in_range = |tf: &Transform| tf.translation.distance(here) < SCAN_RANGE;
            let poster = objects.iter().find(|(_, net_e, store, tf)| {
                net_e.kind == EntityKind::GameObject
                    && store.0.gameobject_type_id() == GO_TYPE_QUESTGIVER
                    && in_range(tf)
            });
            let control = objects.iter().find(|(_, net_e, store, tf)| {
                net_e.kind == EntityKind::Unit
                    && store.0.unit_npc_flags() & NPC_FLAG_QUESTGIVER != 0
                    && in_range(tf)
            });
            let Some((guid, _, store, _)) = poster else {
                if now - sent_at > SCAN_TIMEOUT_SECS {
                    error!(
                        "PROBE_GOQUEST: FAIL — no type-{GO_TYPE_QUESTGIVER} GameObject within \
                         {SCAN_RANGE} yd in {SCAN_TIMEOUT_SECS}s. This is NOT a passing run"
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                }
                return;
            };
            let flags = store.0.gameobject_flags();
            info!(
                "PROBE_GOQUEST: quest object {:#x} in range — GAMEOBJECT_FLAGS {flags:#x} \
                 (INTERACT_COND {}), GAMEOBJECT_DYN_FLAGS {:#x}",
                guid.0,
                flags & GO_FLAG_INTERACT_COND != 0,
                store.0.gameobject_dynamic_flags(),
            );
            if flags & GO_FLAG_INTERACT_COND == 0 {
                error!(
                    "PROBE_GOQUEST: FAIL — this object does not carry GAMEOBJECT_FLAGS bit 2, so \
                     the reference would never query it either. Aim the probe at a quest object"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
                return;
            }
            probe.poster = Some(guid.0);
            match control {
                Some((g, ..)) => {
                    info!(
                        "PROBE_GOQUEST: unit control — questgiver creature {:#x}",
                        g.0
                    );
                    probe.control = Some(g.0);
                }
                None => warn!(
                    "PROBE_GOQUEST: no questgiver-flagged creature within {SCAN_RANGE} yd — the \
                     unit control cannot be read at this spot"
                ),
            }
            probe.phase = Phase::Level {
                to: LOW_LEVEL,
                since: now,
            };
            level_to(&net, level, LOW_LEVEL);
        }
        Phase::Level { to, since } => {
            if level == to {
                // A GameObject guid reaches the wire only from a sweep (`0x5eb159`, `0x5eb456`);
                // the epoch bump forces one even if the level did not change.
                quest.bump_reask();
                info!("PROBE_GOQUEST: at level {to}, swept — reading the window");
                probe.phase = Phase::Read {
                    level: to,
                    since: now,
                };
            } else if now - since > LEVEL_TIMEOUT_SECS {
                error!("PROBE_GOQUEST: FAIL — still level {level} after a `.levelup` to {to}");
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Read { level: at, since } => {
            let count = quest.refused_count();
            let mine = probe.poster.and_then(|p| quest.refused_for(p));
            // HIGH must see a different answer than LOW, or one stale refusal would satisfy both.
            let settled = if at == LOW_LEVEL {
                mine
            } else {
                mine.filter(|s| Some(*s) != probe.low)
            };
            if settled.is_none() && now - since <= STATUS_TIMEOUT_SECS {
                return;
            }
            if at == LOW_LEVEL {
                probe.low = settled;
                info!(
                    "PROBE_GOQUEST: LOW refused {settled:?} (refusals so far: {count}) — dinging \
                     to level {HIGH_LEVEL}"
                );
                probe.phase = Phase::Level {
                    to: HIGH_LEVEL,
                    since: now,
                };
                level_to(&net, level, HIGH_LEVEL);
            } else {
                probe.high = settled;
                let back = probe.start_level.unwrap_or(level);
                info!(
                    "PROBE_GOQUEST: HIGH refused {settled:?} (refusals so far: {count}) — \
                     restoring level {back}"
                );
                level_to(&net, level, back);
                probe.phase = Phase::Restore { since: now };
            }
        }
        Phase::Restore { since } => {
            let back = probe.start_level.unwrap_or(level);
            if level == back {
                probe.phase = Phase::Done;
            } else if now - since > LEVEL_TIMEOUT_SECS {
                warn!("PROBE_GOQUEST: left the character at level {level}, not {back}");
                probe.phase = Phase::Done;
            }
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            let mut fails = probe.fails;
            fails += report("LOW", STATUS_UNAVAILABLE, probe.low);
            fails += report("HIGH", STATUS_AVAILABLE, probe.high);
            if probe.poster_ever_stored {
                error!(
                    "PROBE_GOQUEST: STORE FAIL — a status was stored for the GameObject. The \
                     marker layer and the minimap dot both read that map, so this is a `!` the \
                     reference client does not draw"
                );
                fails += 1;
            } else {
                info!(
                    "PROBE_GOQUEST: STORE PASS — no status ever stored for the GameObject, so no \
                     marker and no minimap dot"
                );
            }
            match (probe.control, probe.control_stored) {
                (Some(g), Some(s)) => info!(
                    "PROBE_GOQUEST: CTRL PASS — the questgiver creature {g:#x} still has its own \
                     status ({s})"
                ),
                (Some(g), None) => {
                    error!(
                        "PROBE_GOQUEST: CTRL FAIL — the questgiver creature {g:#x} never got a \
                         status. The refusal is one bit too wide and units lost theirs"
                    );
                    fails += 1;
                }
                (None, _) => warn!("PROBE_GOQUEST: CTRL SKIPPED — no creature questgiver in range"),
            }
            info!(
                "PROBE_GOQUEST: DONE object={:#x} low={:?} high={:?} refusals={} fail={fails}",
                probe.poster.unwrap_or_default(),
                probe.low,
                probe.high,
                quest.refused_count(),
            );
            // `ProbeExitPlugin::fire_probe_exit`'s pattern: `AppExit` plus a hard backstop, so a
            // teardown hang cannot leave a client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_GOQUEST: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
