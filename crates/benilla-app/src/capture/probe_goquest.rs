//! The GameObject-questgiver live probe (`WOW_PROBE_GOQUEST=1`) — the instrument for *"wanted
//! posters and quest objects are never status-queried"*, and for the answer that question turned
//! out to have (decision 1872).
//!
//! A quest-giving **GameObject** — a wanted poster, a half-eaten body, a suspicious barrel — is a
//! questgiver on the wire exactly as a creature is, and the reference client asks about it: the
//! sweep callback tests typemask bit 5 and sends `CMSG_QUESTGIVER_STATUS_QUERY` for any GameObject
//! carrying `GAMEOBJECT_FLAGS` bit 2 whose reaction toward us is `> 1`. benilla asked about no
//! GameObject at all, which is a real wire gap.
//!
//! But **the answer is refused, and nothing is ever drawn**: the reference's handler `0x5dc9f0`
//! resolves the GUID with typemask 8, a GameObject's `0x21` misses bit 3, and the packet dies at
//! `0x5dca2f`. And the real 1.12 service never sent one anyway — zero GameObject GUIDs across the
//! 1292 in the genuine sniff corpus. **vmangos does send one** (`TYPEMASK_CREATURE_OR_GAMEOBJECT`,
//! `QuestHandler.cpp:41`), so on our server the refusal is load-bearing rather than theoretical:
//! without it a wanted poster would wear a gold `!` the reference client leaves bare.
//!
//! That makes this probe's job the three-way distinction a unit test cannot make at the real
//! object: **the query went out**, **the answer came back**, and **we refused it** — where "never
//! asked" and "asked and correctly refused" both read as an empty status map. The refusal readout
//! ([`QuestGiver::refused_for`]) is what separates them, and an arriving status can only exist if
//! our query did. It is keyed by guid on purpose: Goldshire has several quest objects in view and
//! the server answers all of them in one drain, so a single last-writer slot reads as whichever
//! object archetype order happened to put last (it read as a regression exactly once, on a run
//! where nothing about the behaviour had changed).
//!
//! Four readings, in order, and the last two are the controls that make it a regression test:
//!
//! 1. **LOW** — at level [`LOW_LEVEL`], below the quest's own `MinLevel`: the server must answer
//!    `1` (`UNAVAILABLE`, the grey `!` a *creature* would wear) and we must refuse it.
//! 2. **HIGH** — after a ding past `MinLevel`: the answer must change to `5` (`AVAILABLE`). A probe
//!    that only ever saw one value could not tell a live query from a single stale packet.
//! 3. **NO STATUS** — `QuestGiver::status()` for the poster stays `None` throughout, sampled every
//!    frame rather than at the windows. The marker layer and the minimap dot both draw from exactly
//!    that map, so an empty entry *is* "no `!`, no dot".
//! 4. **THE UNIT CONTROL** — a creature questgiver in the same scene still gets and keeps its own
//!    status. This is the half that must not change, and it is what would catch a refusal written
//!    one bit too wide.
//!
//! ## The run recipe
//!
//! ```text
//! WOW_NOSOUND=1 WOW_USER=probe4 WOW_PASS=pprobe4 WOW_CHAR=Probefour \
//!     WOW_PROBE_GOQUEST=1 cargo run -q -p benilla
//! ```
//! (the checkout's probe identity — `.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR; the `probe`
//! skill). `WOW_PROBE_GOQUEST=<x>,<y>,<z>[,<map>]` aims it elsewhere; the
//! default is the **Goldshire wanted poster** — `gameobject.guid` 26843, template 68
//! `Wanted Poster`, `type` 2 `GAMEOBJECT_TYPE_QUESTGIVER`, `flags` 4 `INTERACT_COND`, offering
//! quest 176 `Wanted: "Hogger"` (`MinLevel` 5, `QuestLevel` 11). It is a single **unpooled** spawn
//! with a 900 s respawn, so it is there every run — unlike the chest probe's first target, which
//! was a spawn-pool member and read "missing" most of the time (decision 1471's trap). Goldshire
//! also supplies the unit control (Marshal Dughan and the innkeeper are both questgiver-flagged),
//! which is why both halves can be read at one spot.
//!
//! Each window forces its own sweep, because that is the *only* way a GameObject GUID reaches the
//! wire: it sets the level (the reference's own `UNIT_FIELD_LEVEL` watch, and what makes the answer
//! change) and then bumps the packet epoch, so the window sweeps even if the character already
//! happened to be standing at the level the window wants. The starting level is restored on exit.
//!
//! **One trap the scan line will show you:** every probe character is permanently in GM mode
//! (`characters.extra_flags` bit `0x1`), and vmangos ORs `target->IsGameMaster()` into a
//! GameObject's `IsActivateToQuest` (`Object.cpp`) — so `GAMEOBJECT_DYN_FLAGS` reads `0x1` on
//! every quest object regardless of whether the character could actually take the quest. The
//! *dialog status* is computed without any GM term, so the windows below are unaffected; but do
//! not read the dyn-flag as "this quest is available to me", and do not gate a query on it.
//!
//! Grep `PROBE_GOQUEST:` for the verdict; the probe self-exits when it lands.

use bevy::prelude::*;

use benilla_protocol::EntityKind;

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_quest::QuestGiver;

/// The probe's default object: the `Wanted Poster` outside the Goldshire inn (`gameobject` 26843,
/// template 68, quest 176). `[x, y, z, map]`.
const POSTER_AT: [f32; 4] = [-9668.23, 683.39, 36.33, 0.0];
/// `GAMEOBJECT_TYPE_QUESTGIVER` — the strategy type a wanted poster carries.
const GO_TYPE_QUESTGIVER: i32 = 2;
/// `GAMEOBJECT_FLAGS` bit 2 — the reference's whole GameObject query gate (`0x5eb0f2 shr eax,0x2`).
const GO_FLAG_INTERACT_COND: u32 = 0x4;
/// `UNIT_NPC_FLAGS` questgiver bit — how the unit control names itself.
const NPC_FLAG_QUESTGIVER: u32 = 0x2;
/// The two levels the two windows are read at, chosen against quest 176's own `MinLevel` 5 and
/// `QuestLevel` 11: below it the server answers `UNAVAILABLE`, above it `AVAILABLE` (and not
/// `CHAT`, which would need `level > QuestLevel + Quests.LowLevelHideDiff`).
const LOW_LEVEL: u32 = 2;
const HIGH_LEVEL: u32 = 8;
/// `DialogStatus` ids the verdict is written in (the keys of the status map `0x80c454`).
const STATUS_UNAVAILABLE: u32 = 1;
const STATUS_AVAILABLE: u32 = 5;

const SETTLE_SECS: f64 = 5.0;
const SCAN_TIMEOUT_SECS: f64 = 25.0;
/// How long a window waits for the refused answer. The query goes out on the sweep the window
/// itself forces; nothing arriving in this long means it was never sent.
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
    /// The unit control's guid — a questgiver-flagged creature in the same scene.
    control: Option<u64>,
    /// The level the character was at when the probe started — restored before it exits.
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

/// Where the probe is aimed: `WOW_PROBE_GOQUEST=<x>,<y>,<z>[,<map>]`, else [`POSTER_AT`]. Anything
/// unparseable falls back to the default rather than failing the run — the common value is `1`.
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

/// `.levelup <delta>` on the probe's own character. Nothing is ever selected by this probe, so the
/// command's creature branch (`GetSelectedCreature`) can't fire.
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
    // Sampled every frame, not just at the windows: a status that flashed on for one frame and was
    // pruned would still have been a rendered `!`, and the claim is that none is ever stored.
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
                // Force the sweep this window depends on. A GameObject GUID reaches the wire ONLY
                // from a sweep (`0x5eb159`, `0x5eb456`), and the level change above is one — but it
                // is a no-op when the character already stood at this level, so the epoch bump
                // makes the window's sweep unconditional.
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
            // The HIGH window must see a *different* answer than LOW, or one stale refusal would
            // satisfy both.
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
            // The probe self-exit pattern (`ProbeExitPlugin::fire_probe_exit`): a polite AppExit
            // plus a hard backstop thread, so a net/winit teardown hang can't leave a zombie
            // client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_GOQUEST: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
