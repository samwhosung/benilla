//! The inside-a-battleground live probe (`WOW_PROBE_BG=wsg|ab|av`): level, greet a battlemaster,
//! queue, take the port through the stock `AcceptBattlefieldPort`, print a census every
//! [`CENSUS_EVERY`] from inside, then take the flag and die for the graveyard leg. Inert without
//! the env.
//!
//! One body can never fill a battleground (`battleground_template.min_players_per_team`), so the
//! probe sends `.debug bg`, which toggles `BattleGroundMgr::m_testing` and lets a match form with
//! one player on either side (`BattleGroundMgr.cpp:557/623/756`). It is a realm-wide, in-memory
//! toggle announced to every player, so the probe counts its sends ([`BgProbe::toggles_sent`]) and
//! [`leave_testing_as_found`] restores the parity on every road to [`Phase::Done`]. It goes back
//! off once inside: while testing is on, the premature-finish countdown never decrements
//! (`BattleGround.cpp:337`).
//!
//! A census reports the map, area and position, terrain streaming, entity counts by kind, the
//! gameobject entries, the raw world-state table, `GetNumWorldStateUI()`,
//! `GetBattlefieldStatus(1..3)`, the scoreboard, the nearest spirit guide, the dropped opcodes and
//! the Lua event tap's window.
//!
//! The probe stamps input every frame, so it never exercises the idle handler.
//!
//! Run it with `WOW_GM=off` (GM mode re-templates the body's faction to 35, and a battleground
//! reads every reaction and the team assignment off it) under a `timeout` of about 5 min, or 10
//! for a run sized by `WOW_PROBE_BG_SAMPLES` to reach a match's end; the rest is
//! `docs/CONTRIBUTING.md`, "Running it unattended". Non-combat: the probe never attacks.

use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use benilla_ui::script::UiScript;
use benilla_world::terrain_stream::{CurrentArea, WorldLoadProgress};
use benilla_world::world_map::CurrentMap;

use super::probes::ProbeClock;
use crate::net::{
    ChatKind, ClientCommand, DroppedOpcodes, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer,
};
use crate::player::Player;
use crate::target::cursor_mode::npc_flags;
use crate::ui_battlefield::Battlefield;
use crate::ui_dialog_verbs::BattlefieldQueue;
use crate::world_state::WorldStates;

/// One battleground's fixtures. The three battlemasters stand within ~35 yd of each other in
/// Stormwind, so the probe finds its NPC by creature entry, not by nearness.
struct Arena {
    /// `WOW_PROBE_BG`'s value.
    key: &'static str,
    /// The Map.dbc row the join carries and the worldport lands on.
    map: u32,
    /// The battlemaster's `creature_template.entry`.
    npc_entry: u32,
    npc_name: &'static str,
    /// The battlemaster's spawn on map 0, the `.go xyz` target.
    at: [f32; 3],
    /// The flag leg's gameobject entry and its position on the battleground map; `None` where the
    /// objective is not one clickable flag.
    flag: Option<(u32, [f32; 3])>,
}

/// The three 1.12 battlegrounds; [`QUEUE_LEVEL`] clears every bracket floor on every patch.
const ARENAS: [Arena; 3] = [
    Arena {
        key: "wsg",
        map: 489,
        npc_entry: 14981,
        npc_name: "Elfarran",
        at: [-8454.62, 318.85, 120.97],
        // The Warsong Flag at the Horde base, the one an Alliance body carries; the Silverwing
        // Flag (179830) is the capture point.
        flag: Some((179831, [916.02, 1434.40, 345.41])),
    },
    Arena {
        key: "ab",
        map: 529,
        npc_entry: 15008,
        npc_name: "Lady Hoteshem",
        at: [-8420.48, 328.71, 120.89],
        // Arathi Basin's objectives are five banners, not a carried flag.
        flag: None,
    },
    Arena {
        key: "av",
        map: 30,
        npc_entry: 7410,
        npc_name: "Thelman Slatefist",
        at: [-8424.55, 342.81, 120.89],
        // Alterac Valley's are towers, graveyards and captains.
        flag: None,
    },
];

/// The probe body's level: Alterac Valley's floor is 51, and a body under the bracket is refused
/// at the hello with no reply.
const QUEUE_LEVEL: u32 = 60;

/// How many times to drop and re-take the queue before giving up.
const MAX_REJOINS: u32 = 3;

/// The settle radius before greeting; the server applies its own interaction range.
const NEAR_YD: f32 = 20.0;

/// `PLAYER_FLAGS_GM`: the body carries GM mode's faction-35 re-template.
const PLAYER_FLAGS_GM: u32 = 0x0000_0008;

/// How long the exit waits for the leave's worldport before going anyway.
const LEAVE_GRACE: f64 = 5.0;

/// How long to wait for `probe_shield`'s `.gm off` to land: it waits up to 3 s for the body's
/// name, sends its commands 0.8 s apart with `.gm off` after the god line, and the flag clears
/// only on the server's field update.
const GM_DROP_WAIT: f64 = 20.0;

/// How long to listen for `.debug bg`'s world-text reply, which crosses the network, the chat
/// pipeline and the Lua event dispatch before it reaches the tap.
const TOGGLE_REPLY_WINDOW: f64 = 6.0;

/// How often a census line is printed once inside.
const CENSUS_EVERY: f64 = 12.0;

/// 12 × 12 s = 144 s: vmangos holds arrivals behind closed doors for `BG_START_DELAY_2M` (120 s,
/// `BattleGround.cpp:248`), which `.debug bg` does not shorten, plus two samples past the doors.
const CENSUS_SAMPLES_DEFAULT: u32 = 12;

/// The census sample count, `WOW_PROBE_BG_SAMPLES`, at least 1 (the first sample restores the
/// testing lever). A solo match stays under the template's `min_players_per_team`, so vmangos ends
/// it with no winner (`WINNER_NONE` = 2, `BattleGroundDefines.h:204`) 300 s after the doors,
/// about T+420 s: 36 samples reach it. Alterac Valley never ends this way (`BattleGround.cpp:318`).
fn census_samples() -> u32 {
    std::env::var("WOW_PROBE_BG_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CENSUS_SAMPLES_DEFAULT)
        .max(1)
}

/// The ghost leg's sample period: the resurrect wave is a 30 s server cycle
/// (`BattleGround::Update`'s `m_lastResurrectTime`), so eleven 4 s samples cover one full wave.
const GHOST_SAMPLE_EVERY: f64 = 4.0;
/// See [`GHOST_SAMPLE_EVERY`].
const GHOST_SAMPLES: u32 = 11;

/// How many 3 s samples the flag leg takes after the use.
const FLAG_SAMPLES: u32 = 5;

/// `SPIRITGUIDE`, `UNIT_NPC_FLAGS` bit 6: the flag the reference's area-spirit-healer scan
/// `0x4924c0` keys on.
const NPC_FLAG_SPIRITGUIDE: u32 = 1 << 6;

/// A Lua frame that records every battleground event the stock interface listens for, with its
/// `arg1`: a chat kind that reaches the VM logs nothing, so only a tap shows the countdown's
/// `CHAT_MSG_BG_SYSTEM_NEUTRAL` lines (`BattleGround.cpp:383-418`). Written in 1.12 dialect
/// (`this`/`event`/`arg1` globals, no `ipairs` over a literal).
const EVENT_TAP: &str = r#"
BenillaBgLog = {};
BenillaBgTap = CreateFrame("Frame");
BenillaBgTap:SetScript("OnEvent", function()
    table.insert(BenillaBgLog, event .. "(" .. tostring(arg1) .. ")");
end);
BenillaBgTap:RegisterEvent("CHAT_MSG_BG_SYSTEM_NEUTRAL");
BenillaBgTap:RegisterEvent("CHAT_MSG_BG_SYSTEM_ALLIANCE");
BenillaBgTap:RegisterEvent("CHAT_MSG_BG_SYSTEM_HORDE");
BenillaBgTap:RegisterEvent("CHAT_MSG_SYSTEM");
BenillaBgTap:RegisterEvent("CHAT_MSG_MONSTER_YELL");
BenillaBgTap:RegisterEvent("UPDATE_WORLD_STATES");
BenillaBgTap:RegisterEvent("UPDATE_BATTLEFIELD_STATUS");
BenillaBgTap:RegisterEvent("UPDATE_BATTLEFIELD_SCORE");
BenillaBgTap:RegisterEvent("AREA_SPIRIT_HEALER_IN_RANGE");
BenillaBgTap:RegisterEvent("AREA_SPIRIT_HEALER_OUT_OF_RANGE");
BenillaBgTap:RegisterEvent("PLAYER_DEAD");
BenillaBgTap:RegisterEvent("PLAYER_UNGHOST");
BenillaBgTap:RegisterEvent("PLAYER_ALIVE");
BenillaBgTap:RegisterEvent("ZONE_CHANGED_NEW_AREA");
"#;

/// What the drain returns when the tap's globals are gone (the VM was minted fresh), so a dead tap
/// never reads as "no events"; [`bg_probe`] re-installs on seeing it.
const TAP_GONE: &str = "<tap-gone>";

/// Drain the tap since the last census and empty it; both globals are checked, the frame that
/// receives and the table that holds.
const EVENT_DRAIN: &str = r#"
local out = "";
if not BenillaBgLog or not BenillaBgTap then return "<tap-gone>" end
for i = 1, table.getn(BenillaBgLog) do out = out .. "  " .. BenillaBgLog[i]; end
BenillaBgLog = {};
return out;
"#;

pub(crate) struct ProbeBgPlugin;

impl Plugin for ProbeBgPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BgProbe>()
            .add_systems(Update, (bg_probe, bg_probe_exit).chain());
    }
}

#[derive(Resource, Default)]
struct BgProbe {
    phase: Phase,
    /// The Lua event tap is in and believed live; cleared when a drain finds it gone, because the
    /// interface VM is minted fresh on every world entry (`ui_script::lifecycle::mint_entry_vm`).
    tap_installed: bool,
    /// The tap raised on install, so an `EVENTS` line says the tap is broken, not that nothing
    /// fired.
    tap_broken: bool,
    /// How many `.debug bg` toggles this run has sent: an odd count owes the realm one more send.
    /// Parity needs no reply; the lever's state can only be read from a reply that may never come.
    toggles_sent: u32,
    /// What the tap last said the lever reads: on ("…for debugging"), off ("…normal
    /// playercount"), or no reply yet; consumed once.
    testing_seen: Option<bool>,
    /// The lever was read back as on before the join, not assumed; the `Joined` failure says which.
    testing_confirmed: bool,
    rejoins: u32,
    /// The `AppExit` has been written; `Done` is re-entered every frame until the app stops.
    exited: bool,
    /// When [`Phase::Done`] was first seen, so the exit can wait for the leave to land.
    done_at: Option<f64>,
    /// When the `Wait` arm first saw a body, so the GM-mode gate can time out.
    waiting_since: Option<f64>,
    /// Events drained before the first census, reported with it rather than dropped.
    pending_events: String,
}

/// `Wait` → `Hopped` → `Toggled` → `Scanning` → `Greeted` → `Joined` (↔ `Rejoining`) → `Ported` →
/// `Inside` → `FlagHop` → `FlagUsed` (a flag arena only) → `Dying` → `Ghost` → `Done`.
#[derive(Default, PartialEq)]
enum Phase {
    #[default]
    Wait,
    Hopped {
        sent_at: f64,
    },
    Toggled {
        sent_at: f64,
        corrected: bool,
    },
    /// Looking for the battlemaster once the testing lever is settled.
    Scanning {
        since: f64,
    },
    Greeted {
        master: u64,
        sent_at: f64,
    },
    Joined {
        sent_at: f64,
    },
    Rejoining {
        at: f64,
    },
    Ported {
        sent_at: f64,
    },
    Inside {
        entered_at: f64,
        next_census: f64,
        samples: u32,
    },
    FlagHop {
        at: f64,
    },
    FlagUsed {
        at: f64,
        samples: u32,
    },
    Dying {
        sent_at: f64,
    },
    Ghost {
        released_at: f64,
        next_sample: f64,
        samples: u32,
    },
    Done,
}

/// The battleground `WOW_PROBE_BG` names, Warsong Gulch by default.
fn arena() -> &'static Arena {
    let want = std::env::var("WOW_PROBE_BG").unwrap_or_default();
    ARENAS
        .iter()
        .find(|a| a.key.eq_ignore_ascii_case(&want))
        .unwrap_or(&ARENAS[0])
}

/// A GM dot-command on the chat wire; `net` logs every reply.
fn gm(net: &NetCommands, text: impl Into<String>) {
    let _ = net.0.send(ClientCommand::Chat {
        kind: ChatKind::Say,
        target: None,
        text: text.into(),
    });
}

/// Drain the event tap: the events, or a bracketed reason. [`TAP_GONE`] clears `tap_installed` so
/// the next frame re-installs into the live VM.
fn drain_events(probe: &mut BgProbe, script: &mut UiScript) -> String {
    match script.eval::<String>(EVENT_DRAIN) {
        Ok(s) if s.trim() == TAP_GONE => {
            probe.tap_installed = false;
            format!("  {TAP_GONE} — the VM was replaced under the tap; re-installing")
        }
        Ok(s) => s,
        Err(e) => format!("  <drain raised: {e}>"),
    }
}

/// Put the realm's `.debug bg` lever back if this run moved it. Every road to [`Phase::Done`] goes
/// through here: the command is a realm-wide toggle (`BattleGroundMgr::ToggleTesting`), not a set.
fn leave_testing_as_found(probe: &mut BgProbe, net: &NetCommands) {
    if probe.toggles_sent % 2 == 1 {
        info!(
            "PROBE bg: TESTING — {} toggle(s) sent this run; one more puts the realm's \
             `.debug bg` lever back as found",
            probe.toggles_sent
        );
        toggle_testing(probe, net);
    } else {
        info!(
            "PROBE bg: TESTING — {} toggle(s) sent this run; the lever is already as found",
            probe.toggles_sent
        );
    }
}

/// Send `.debug bg` and count it; every send must go through here for the restore's parity.
fn toggle_testing(probe: &mut BgProbe, net: &NetCommands) {
    probe.toggles_sent += 1;
    gm(net, ".debug bg");
}

/// End the run, restoring anything this run changed on the server first.
fn finish(probe: &mut BgProbe, net: &NetCommands) {
    leave_testing_as_found(probe, net);
    probe.phase = Phase::Done;
}

/// A world state, or `absent`: [`WorldStates::get`] reads a never-sent key as `0`, as the
/// reference's table does, and vmangos writes only 1 or 2 into the flag rows
/// (`BattleGroundWS.cpp:509-515`).
fn ws(states: &WorldStates, key: u32) -> String {
    states
        .pairs()
        .find(|(k, _)| *k == key)
        .map_or_else(|| "absent".to_string(), |(_, v)| v.to_string())
}

#[allow(clippy::too_many_arguments)]
fn bg_probe(
    time: ProbeClock,
    mut probe: ResMut<BgProbe>,
    me: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    battlefield: Res<Battlefield>,
    queue: Res<BattlefieldQueue>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    map: Option<Res<CurrentMap>>,
    area: Option<Res<CurrentArea>>,
    load: Option<Res<WorldLoadProgress>>,
    states: Res<WorldStates>,
    dropped: Res<DroppedOpcodes>,
    go_templates: Res<crate::go_templates::GameObjectTemplates>,
    mut idle: ResMut<crate::ui_chat::idle::LastInput>,
    mut script: Option<NonSendMut<UiScript>>,
) {
    let Ok(store) = me.single() else {
        return;
    };
    let arena = arena();
    let now = time.elapsed_secs_f64();

    // The probe stands in for a player at the keyboard: auto-AFK comes at 300 s of no input
    // (`ui_chat::idle`), and vmangos removes an AFK player from a battleground
    // (`Player.cpp:1741-1742`).
    idle.stamp_present(std::time::Duration::from_secs_f64(now));

    // The tap goes in on the first frame with a VM; the world-entry frame has none yet.
    if !probe.tap_installed {
        if let Some(script) = script.as_deref_mut() {
            match script.eval::<()>(EVENT_TAP) {
                Ok(()) => {
                    probe.tap_installed = true;
                    info!("PROBE bg: TAP installed");
                }
                Err(e) => {
                    probe.tap_installed = true; // a raise will not fix itself
                    probe.tap_broken = true;
                    error!("PROBE bg: TAP FAILURE — {e}");
                }
            }
        }
    }

    match probe.phase {
        Phase::Wait => {
            // No start in GM mode: `CanUseBattleGroundObject` refuses a GM every objective with no
            // reply (`Player.cpp:20576`) while the queue and census still work. Under
            // `WOW_GM=off`, `probe_shield` drops GM mode a moment into the world, so this waits.
            if store.0.player_flags() & PLAYER_FLAGS_GM != 0 {
                let since = *probe.waiting_since.get_or_insert(now);
                if now - since < GM_DROP_WAIT {
                    return;
                }
                error!(
                    "PROBE bg: FAILURE — GM mode is still on after {GM_DROP_WAIT:.0}s. A \
                     battleground is the one place it cannot be left on: \
                     `CanUseBattleGroundObject` refuses a GM outright (vmangos \
                     `Player.cpp:20576`), so every objective fails silently while the queue, the \
                     port, the census and the scoreboard all keep working. Re-run with \
                     `WOW_GM=off` — and if it WAS set, look for `probe-shield:` lines: the drop \
                     is sequenced behind the shield and a refused command shows up there."
                );
                finish(&mut probe, &net);
                return;
            }
            // Revive first: `CanInteractWithNPC` refuses a dead player, logged as an "invalid
            // creature".
            gm(&net, ".revive");
            // Wait for the real level: a missing one read as 0 would level a 60 to 120, above
            // every bracket ceiling (`battleground_template.max_lvl = 60`).
            let Some(level) = store.0.unit_level() else {
                return;
            };
            if level < QUEUE_LEVEL {
                gm(&net, format!(".levelup {}", QUEUE_LEVEL - level));
            }
            // Clear Deserter (26013): leaving before the match ends casts it
            // (`Player.cpp:2178/18816`), and the join handler then refuses the queue
            // (`BG_GROUPJOIN_DESERTERS`, `BattleGroundHandler.cpp:176`).
            gm(&net, ".unaura 26013");
            let [x, y, z] = arena.at;
            info!(
                "PROBE bg: SETUP arena={} map={} level>={QUEUE_LEVEL} master={}({})",
                arena.key, arena.map, arena.npc_name, arena.npc_entry,
            );
            gm(&net, format!(".go xyz {x} {y} {z} 0"));
            probe.phase = Phase::Hopped { sent_at: now };
        }
        Phase::Hopped { sent_at } => {
            if now - sent_at < 3.0 {
                return; // post-teleport settle: let the battlemasters stream in
            }
            // The lever is settled before the join: vmangos evaluates a queue only when a join or
            // a leave schedules it (`BattleGroundMgr.cpp:1005-1028`), so flipping it under a
            // waiting entry does nothing.
            toggle_testing(&mut probe, &net);
            probe.phase = Phase::Toggled {
                sent_at: now,
                corrected: false,
            };
        }
        Phase::Toggled { sent_at, corrected } => {
            // Drain every frame until the reply shows; the rest is kept for the first census.
            if let Some(s) = script.as_deref_mut() {
                let seen = drain_events(&mut probe, s);
                // The last marker wins: a correcting send's reply can share a drain with the first.
                let on = seen.rfind("debugging");
                let off = seen.rfind("normal playercount");
                match (on, off) {
                    (Some(a), Some(b)) => probe.testing_seen = Some(a > b),
                    (Some(_), None) => probe.testing_seen = Some(true),
                    (None, Some(_)) => probe.testing_seen = Some(false),
                    (None, None) => {}
                }
                probe.pending_events.push_str(&seen);
            }
            match probe.testing_seen.take() {
                Some(true) => {
                    info!(
                        "PROBE bg: TESTING on (1v0) — confirmed from the tap after {:.1}s",
                        now - sent_at
                    );
                    probe.testing_confirmed = true;
                    probe.phase = Phase::Scanning { since: now };
                }
                Some(false) if corrected => {
                    error!(
                        "PROBE bg: FAILURE — `.debug bg` reads OFF after two sends; the account \
                         may be below SEC_ADMINISTRATOR (6) for it"
                    );
                    finish(&mut probe, &net);
                }
                Some(false) => {
                    info!("PROBE bg: TESTING was on; that send turned it OFF — sending once more");
                    toggle_testing(&mut probe, &net);
                    probe.phase = Phase::Toggled {
                        sent_at: now,
                        corrected: true,
                    };
                }
                None if now - sent_at > TOGGLE_REPLY_WINDOW => {
                    // Unread stays unread: `testing_confirmed` is left false for the failure text.
                    warn!(
                        "PROBE bg: TESTING unread — no `.debug bg` reply reached the tap in \
                         {TOGGLE_REPLY_WINDOW:.0}s; continuing, but if the queue never pops, this \
                         is the first thing to doubt"
                    );
                    probe.phase = Phase::Scanning { since: now };
                }
                None => {}
            }
        }
        Phase::Scanning { since: sent_at } => {
            let here = player.pos;
            // By entry, not nearness: all three stand in one alcove.
            let master = units.iter().find(|(guid, kind, store, tf)| {
                kind.kind == benilla_protocol::EntityKind::Unit
                    && store.0.unit_npc_flags() & npc_flags::BATTLEMASTER != 0
                    && benilla_protocol::guid::entry(guid.0) == Some(arena.npc_entry)
                    && tf.translation.distance(here) < NEAR_YD
            });
            if let Some((guid, ..)) = master {
                info!(
                    "PROBE bg: GREETING {} ({:#x}) on the wire",
                    arena.npc_name, guid.0
                );
                let _ = net.0.send(ClientCommand::BattlemasterHello { npc: guid.0 });
                probe.phase = Phase::Greeted {
                    master: guid.0,
                    sent_at: now,
                };
            } else if now - sent_at > 15.0 {
                error!(
                    "PROBE bg: FAILURE — {} (entry {}) never streamed within {NEAR_YD:.0} yd in 15 s",
                    arena.npc_name, arena.npc_entry
                );
                finish(&mut probe, &net);
            }
        }
        Phase::Greeted { master, sent_at } => {
            // The list's guid is the one the join quotes; its arrival proves the level gate passed.
            if battlefield.battlemaster() == Some(master) {
                info!("PROBE bg: LISTED — queueing for map {}", arena.map);
                let _ = net.0.send(ClientCommand::BattlemasterJoin {
                    battlemaster: master,
                    map_id: arena.map,
                    instance_id: 0,
                    as_group: false,
                });
                probe.phase = Phase::Joined { sent_at: now };
            } else if now - sent_at > 8.0 {
                error!(
                    "PROBE bg: FAILURE — no SMSG_BATTLEFIELD_LIST 8 s after the hello \
                     (a level under the bracket floor is refused here, silently)"
                );
                finish(&mut probe, &net);
            }
        }
        Phase::Joined { sent_at } => {
            // Status 2 is "ready, confirm", taken through the stock verb. Matched on the map too:
            // vmangos re-sends a stale queue's status at login.
            let ready = queue.slots().iter().position(|s| {
                s.as_ref()
                    .is_some_and(|(st, _)| st.status == 2 && st.map_id == arena.map)
            });
            if let Some(index) = ready {
                let slot = index + 1; // the verbs are 1-based
                info!("PROBE bg: CONFIRM — slot {slot} is ready; AcceptBattlefieldPort({slot}, 1)");
                if let Some(script) = script {
                    if let Err(e) = script.eval::<()>(&format!("AcceptBattlefieldPort({slot}, 1)"))
                    {
                        error!("PROBE bg: FAILURE — AcceptBattlefieldPort raised: {e}");
                        finish(&mut probe, &net);
                        return;
                    }
                } else {
                    error!("PROBE bg: FAILURE — no VM to take the port through");
                    finish(&mut probe, &net);
                    return;
                }
                probe.phase = Phase::Ported { sent_at: now };
            } else if now - sent_at > 12.0 && probe.rejoins < MAX_REJOINS {
                // Decline and re-join: an entry that did not match is never re-examined until a
                // join or a leave schedules its queue (`BattleGroundMgr.cpp:1005`).
                probe.rejoins += 1;
                let slot = queue
                    .slots()
                    .iter()
                    .position(|s| s.is_some())
                    .map_or(1, |i| i + 1);
                info!(
                    "PROBE bg: REJOIN {} — nothing popped in 12 s; \
                     AcceptBattlefieldPort({slot}, 0) then queueing again",
                    probe.rejoins
                );
                if let Some(script) = script.as_deref_mut() {
                    let _ = script.eval::<()>(&format!("AcceptBattlefieldPort({slot}, 0)"));
                }
                probe.phase = Phase::Rejoining { at: now };
            } else if now - sent_at > 30.0 {
                let slots: Vec<String> = queue
                    .slots()
                    .iter()
                    .map(|s| {
                        s.as_ref().map_or_else(
                            || "-".into(),
                            |(st, _)| format!("{}:{}", st.map_id, st.status),
                        )
                    })
                    .collect();
                error!(
                    "PROBE bg: FAILURE — no slot reached status 2 in 30 s (slots: {}). {} \
                     Also worth checking: Deserter (26013) on the body, a bracket the server \
                     refused, or a battleground this body is still registered in from a killed run.",
                    slots.join(" "),
                    if probe.testing_confirmed {
                        "The testing flag was READ BACK as on before the join, so the cause is \
                         probably not that."
                    } else {
                        "The testing flag was NEVER READ BACK (no `.debug bg` reply reached the \
                         tap), so it is the first suspect: a previous run that died before its \
                         own restore leaves the lever inverted, and a queue entry is only ever \
                         evaluated when a join or a leave schedules it."
                    },
                );
                finish(&mut probe, &net);
            }
        }
        Phase::Ported { sent_at } => {
            let here = map.as_ref().map_or(0, |m| m.0);
            if here == arena.map {
                info!(
                    "PROBE bg: ENTERED map {} after {:.1}s — census every {CENSUS_EVERY:.0}s × {}",
                    arena.map,
                    now - sent_at,
                    census_samples(),
                );
                probe.phase = Phase::Inside {
                    entered_at: now,
                    next_census: now + CENSUS_EVERY,
                    samples: 0,
                };
            } else if now - sent_at > 30.0 {
                error!(
                    "PROBE bg: FAILURE — 30 s after the port the map is still {here}, not {}",
                    arena.map
                );
                finish(&mut probe, &net);
            }
        }
        Phase::Inside {
            entered_at,
            next_census,
            samples,
        } => {
            if now < next_census {
                return;
            }
            let Some(mut script) = script else {
                error!("PROBE bg: FAILURE — the VM went away inside the battleground");
                finish(&mut probe, &net);
                return;
            };
            // Drained here, not in `census`, so the tap's self-heal can reach `probe`.
            let mut events = std::mem::take(&mut probe.pending_events);
            events.push_str(&drain_events(&mut probe, &mut script));
            let tap_broken = probe.tap_broken;
            census(
                now - entered_at,
                arena,
                &player,
                &units,
                map.as_deref(),
                area.as_deref(),
                load.as_deref(),
                &states,
                &dropped,
                &go_templates,
                &queue,
                &mut script,
                &events,
                tap_broken,
            );
            let samples = samples + 1;

            // Inside, the lever has formed the match; off again so the premature-finish countdown
            // runs (it only decrements while testing is off, `BattleGround.cpp:337`).
            if samples == 1 {
                info!(
                    "PROBE bg: TESTING — inside the instance; the lever has done its job and the \
                     premature-finish countdown is frozen while it is on"
                );
                leave_testing_as_found(&mut probe, &net);
            }

            if samples >= census_samples() {
                // The flag leg, then the graveyard: `.die` clears the probe's god shield, and a
                // battleground death is the only way to reach the area spirit healer.
                match arena.flag {
                    Some((entry, [x, y, z])) => {
                        info!("PROBE bg: FLAG — hopping to {entry} at ({x}, {y}, {z}) to take it");
                        gm(&net, format!(".go xyz {x} {y} {z} {}", arena.map));
                        probe.phase = Phase::FlagHop { at: now };
                    }
                    None => {
                        info!("PROBE bg: FLAG — none for this battleground; on to the graveyard");
                        gm(&net, ".die");
                        probe.phase = Phase::Dying { sent_at: now };
                    }
                }
            } else {
                probe.phase = Phase::Inside {
                    entered_at,
                    next_census: now + CENSUS_EVERY,
                    samples,
                };
            }
        }
        Phase::Rejoining { at } => {
            if now - at < 2.0 {
                return; // the decline lands and the slot clears
            }
            let Some(master) = battlefield.battlemaster() else {
                error!("PROBE bg: FAILURE — the battlemaster list went away before the rejoin");
                finish(&mut probe, &net);
                return;
            };
            let _ = net.0.send(ClientCommand::BattlemasterJoin {
                battlemaster: master,
                map_id: arena.map,
                instance_id: 0,
                as_group: false,
            });
            probe.phase = Phase::Joined { sent_at: now };
        }
        Phase::FlagHop { at } => {
            if now - at < 4.0 {
                return; // the hop crosses the map; the far base streams in
            }
            let Some((entry, _)) = arena.flag else {
                finish(&mut probe, &net);
                return;
            };
            let found = units.iter().find(|(guid, kind, _, tf)| {
                kind.kind == benilla_protocol::EntityKind::GameObject
                    && benilla_protocol::guid::entry(guid.0) == Some(entry)
                    && tf.translation.distance(player.pos) < 15.0
            });
            match found {
                Some((guid, _, _, tf)) => {
                    // A flag stand has no lock, so a click's `resolve_go_action` sends
                    // `CMSG_GAMEOBJ_USE`, as here; the lock is logged, not assumed.
                    let lock = go_templates.get(guid.0).map_or(u32::MAX, |t| t.lock_id);
                    info!(
                        "PROBE bg: FLAG {entry} streamed at {:.1} yd, lock_id={lock} — CMSG_GAMEOBJ_USE",
                        tf.translation.distance(player.pos)
                    );
                    let _ = net.0.send(ClientCommand::GameObjUse { guid: guid.0 });
                    probe.phase = Phase::FlagUsed {
                        at: now,
                        samples: 0,
                    };
                }
                None if now - at > 20.0 => {
                    error!("PROBE bg: FLAG FAILURE — {entry} never streamed within 15 yd in 20 s");
                    gm(&net, ".die");
                    probe.phase = Phase::Dying { sent_at: now };
                }
                None => {}
            }
        }
        Phase::FlagUsed { at, samples } => {
            if now - at < f64::from(samples + 1) * 3.0 {
                return;
            }
            let Some(mut script) = script else {
                error!("PROBE bg: FAILURE — the VM went away during the flag leg");
                finish(&mut probe, &net);
                return;
            };
            // World state 2339 is the Alliance row's icon selector: 1, or 2 while the flag that
            // row is about is carried (`BattleGroundWS.cpp:512`, `:714-722`), never the four-value
            // `m_flagState`, so `2339=1` held all match is correct, not stuck. The buffs are the
            // other half.
            let auras = script
                .eval::<String>(
                    r#"
                    local out = "";
                    for i = 1, 16 do
                        local t = UnitBuff("player", i);
                        if t then out = out .. " " .. t; end
                    end
                    return out;
                    "#,
                )
                .unwrap_or_else(|e| format!("<raised: {e}>"));
            let events = drain_events(&mut probe, &mut script);
            info!(
                "PROBE bg: FLAG t={:.0}s ws2338={} ws2339={} captures={}/{} buffs=[{}]{}",
                now - at,
                ws(&states, 2338),
                ws(&states, 2339),
                ws(&states, 1581),
                ws(&states, 1582),
                auras.trim(),
                if events.trim().is_empty() {
                    String::new()
                } else {
                    format!(" events={events}")
                },
            );
            let samples = samples + 1;
            if samples >= FLAG_SAMPLES {
                info!("PROBE bg: FLAG done — on to the graveyard");
                gm(&net, ".die");
                probe.phase = Phase::Dying { sent_at: now };
            } else {
                probe.phase = Phase::FlagUsed { at, samples };
            }
        }
        Phase::Dying { sent_at } => {
            if store.0.unit_is_dead() {
                info!("PROBE bg: DEAD — releasing (CMSG_REPOP_REQUEST)");
                let _ = net.0.send(ClientCommand::RepopRequest);
                probe.phase = Phase::Ghost {
                    released_at: now,
                    next_sample: now + GHOST_SAMPLE_EVERY,
                    samples: 0,
                };
            } else if now - sent_at > 15.0 {
                error!(
                    "PROBE bg: FAILURE — still alive 15 s after `.die` (is the shield re-armed?)"
                );
                finish(&mut probe, &net);
            }
        }
        Phase::Ghost {
            released_at,
            next_sample,
            samples,
        } => {
            if now < next_sample {
                return;
            }
            let Some(mut script) = script else {
                finish(&mut probe, &net);
                return;
            };
            // Ghost, nearest spirit guide, the wave clock (non-zero only once
            // `SMSG_AREA_SPIRIT_HEALER_TIME` answers) and the stock popup.
            let ghost = store.0.player_is_ghost();
            let nearest = units
                .iter()
                .filter(|(_, k, st, _)| {
                    k.kind == benilla_protocol::EntityKind::Unit
                        && st.0.unit_npc_flags() & NPC_FLAG_SPIRITGUIDE != 0
                })
                .map(|(g, _, _, tf)| (g.0, tf.translation.distance(player.pos)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let wave = script
                .eval::<String>(
                    "return tostring(GetAreaSpiritHealerTime()) .. \" popup=\" .. \
                     tostring(StaticPopup_Visible and StaticPopup_Visible(\"AREA_SPIRIT_HEAL\"))",
                )
                .unwrap_or_else(|e| format!("<raised: {e}>"));
            let events = drain_events(&mut probe, &mut script);
            info!(
                "PROBE bg: GHOST t={:.0}s ghost={ghost} pos=({:.0},{:.0},{:.0}) area={:?} guide={} wave={wave}{}",
                now - released_at,
                player.pos.x,
                player.pos.y,
                player.pos.z,
                area.as_deref().and_then(|a| a.0),
                nearest.map_or_else(|| "none".to_string(), |(g, d)| format!("{g:#x}@{d:.1}yd")),
                if events.trim().is_empty() {
                    String::new()
                } else {
                    format!(" events={events}")
                },
            );
            let samples = samples + 1;
            if samples >= GHOST_SAMPLES {
                // Take the wave if one is offered.
                match script.eval::<()>("AcceptAreaSpiritHeal()") {
                    Ok(()) => info!("PROBE bg: GHOST AcceptAreaSpiritHeal() sent"),
                    Err(e) => error!("PROBE bg: GHOST AcceptAreaSpiritHeal() raised: {e}"),
                }
                report(arena, &net, &queue, &mut script);
                finish(&mut probe, &net);
            } else {
                probe.phase = Phase::Ghost {
                    released_at,
                    next_sample: now + GHOST_SAMPLE_EVERY,
                    samples,
                };
            }
        }
        Phase::Done => {}
    }
}

/// Ends the process once [`Phase::Done`] is reached and the body has left the battleground map
/// (or [`LEAVE_GRACE`] passed): exiting before the worldport completes leaves the body inside a
/// live battleground. A separate system because [`bg_probe`] is at Bevy's parameter ceiling; the
/// hard-exit thread fires even though `AppExit` stops the Update schedule.
fn bg_probe_exit(
    time: ProbeClock,
    mut probe: ResMut<BgProbe>,
    map: Option<Res<CurrentMap>>,
    mut exit: MessageWriter<AppExit>,
) {
    if probe.phase != Phase::Done || probe.exited {
        return;
    }
    let now = time.elapsed_secs_f64();
    let since = *probe.done_at.get_or_insert(now);
    let left = map.as_ref().is_none_or(|m| m.0 != arena().map);
    if !left && now - since < LEAVE_GRACE {
        return;
    }
    if !left {
        warn!(
            "PROBE bg: still on map {} {LEAVE_GRACE:.0}s after the leave — exiting anyway, but \
             this body may still be registered in the battleground",
            arena().map
        );
    }
    probe.exited = true;
    info!("PROBE bg: exiting");
    exit.write(AppExit::Success);
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(5));
        warn!("PROBE bg: still alive 5s after AppExit — hard exit");
        std::process::exit(0);
    });
}

/// One census sample: greppable `PROBE bg:` lines for everything the client sees from inside.
#[allow(clippy::too_many_arguments)]
fn census(
    t: f64,
    arena: &Arena,
    player: &Player,
    units: &Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    map: Option<&CurrentMap>,
    area: Option<&CurrentArea>,
    load: Option<&WorldLoadProgress>,
    states: &WorldStates,
    dropped: &DroppedOpcodes,
    go_templates: &crate::go_templates::GameObjectTemplates,
    queue: &BattlefieldQueue,
    script: &mut UiScript,
    events: &str,
    tap_broken: bool,
) {
    use benilla_protocol::EntityKind as K;

    let p = player.pos;
    let terrain = load.map_or_else(
        || "no-progress".to_string(),
        |l| {
            format!(
                "{}/{} focus={} scene={} colliders={}",
                l.ready, l.total, l.focus_resident, l.scene_ready, l.colliders_pending
            )
        },
    );
    info!(
        // `map=none`, not `map=0`: 0 is Eastern Kingdoms.
        "PROBE bg: CENSUS t={t:.0}s map={} area={:?} pos=({:.0},{:.0},{:.0}) terrain={terrain}",
        map.map_or_else(|| "none".to_string(), |m| m.0.to_string()),
        area.and_then(|a| a.0),
        p.x,
        p.y,
        p.z,
    );

    // Entities, by kind.
    let (mut u, mut pl, mut go, mut dy, mut co) = (0, 0, 0, 0, 0);
    let mut go_entries: Vec<(u32, Option<String>)> = Vec::new();
    let mut nearest_guide: Option<(u64, f32)> = None;
    for (guid, kind, store, tf) in units.iter() {
        match kind.kind {
            K::Unit => {
                u += 1;
                if store.0.unit_npc_flags() & NPC_FLAG_SPIRITGUIDE != 0 {
                    let d = tf.translation.distance(p);
                    if nearest_guide.is_none_or(|(_, best)| d < best) {
                        nearest_guide = Some((guid.0, d));
                    }
                }
            }
            K::Player => pl += 1,
            K::GameObject => {
                go += 1;
                if let Some(e) = benilla_protocol::guid::entry(guid.0) {
                    // With its name from the `GAMEOBJECT_QUERY` cache, already filled per stream.
                    go_entries.push((e, go_templates.get(guid.0).map(|t| t.name.clone())));
                }
            }
            K::DynamicObject => dy += 1,
            K::Corpse => co += 1,
            K::Other => {}
        }
    }
    go_entries.sort_unstable();
    let mut counted: Vec<String> = Vec::new();
    let mut i = 0;
    while i < go_entries.len() {
        let (e, ref name) = go_entries[i];
        let n = go_entries[i..].iter().take_while(|(x, _)| *x == e).count();
        let label = name
            .as_deref()
            .map_or_else(|| e.to_string(), |n| format!("{e}:{n}"));
        counted.push(if n > 1 {
            format!("{label}×{n}")
        } else {
            label
        });
        i += n;
    }
    info!("PROBE bg: ENTS units={u} players={pl} gos={go} dyn={dy} corpses={co}");
    info!(
        "PROBE bg: GOS [{}]",
        if counted.is_empty() {
            "none".to_string()
        } else {
            counted.join(" ")
        }
    );
    info!(
        "PROBE bg: GUIDE {}",
        nearest_guide.map_or_else(
            || "none in range".to_string(),
            |(g, d)| format!("{g:#x} at {d:.1} yd")
        )
    );

    // The world-state table, the battleground's whole score, as raw dwords.
    let mut pairs: Vec<(u32, i32)> = states.pairs().collect();
    pairs.sort_unstable_by_key(|(k, _)| *k);
    info!(
        "PROBE bg: STATES scope={:?} n={} [{}]",
        states.scope(),
        pairs.len(),
        pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ")
    );

    // What the stock readout would draw, out of the VM.
    let alwaysup = script
        .eval::<String>(
            r#"
            local n = GetNumWorldStateUI()
            local out = tostring(n)
            for i = 1, n do
                local ui, state, hidden, text, icon = GetWorldStateUIInfo(i)
                out = out .. " | " .. tostring(state) .. " '" .. tostring(text) .. "' icon=" .. tostring(icon)
            end
            return out
            "#,
        )
        .unwrap_or_else(|e| format!("<raised: {e}>"));
    info!("PROBE bg: ALWAYSUP {alwaysup}");

    let status = script
        .eval::<String>(
            r#"
            local out = ""
            for i = 1, 3 do
                local s, name, id, lo, hi = GetBattlefieldStatus(i)
                out = out .. i .. "=" .. tostring(s) .. "/" .. tostring(name) .. "/" .. tostring(id) .. " "
            end
            return out .. "runtime=" .. tostring(GetBattlefieldInstanceRunTime())
            "#,
        )
        .unwrap_or_else(|e| format!("<raised: {e}>"));
    info!(
        "PROBE bg: STATUS {status} active_map={:?}",
        queue.active_map()
    );

    let score = script
        .eval::<String>(
            r#"
            RequestBattlefieldScoreData()
            return tostring(GetNumBattlefieldScores()) .. " winner=" .. tostring(GetBattlefieldWinner())
            "#,
        )
        .unwrap_or_else(|e| format!("<raised: {e}>"));
    info!("PROBE bg: SCORE n={score}");

    // Every opcode the codec dropped, by name; `DroppedOpcodes` is never cleared, so the counts
    // are cumulative over the session.
    let mut drops: Vec<(u16, u64, u64)> = dropped
        .0
        .iter()
        .map(|(&op, t)| (op, t.unknown, t.unparseable))
        .collect();
    drops.sort_unstable_by_key(|(op, _, _)| *op);
    info!(
        "PROBE bg: DROPPED [{}]",
        if drops.is_empty() {
            "none".to_string()
        } else {
            drops
                .iter()
                .map(|(op, unk, bad)| {
                    let name = benilla_protocol::messages::opcode_name(*op)
                        .map_or_else(|| format!("{op:#06x}"), str::to_string);
                    format!("{name}:unknown={unk},unparseable={bad}")
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    );

    // The event tap's window since the last sample; a broken tap says so instead of `(none)`.
    info!(
        "PROBE bg: EVENTS{}",
        if tap_broken {
            "  <tap broken — see TAP FAILURE above; this is NOT 'no events'>".to_string()
        } else if events.trim().is_empty() {
            "  (none)".to_string()
        } else {
            events.to_string()
        }
    );

    // No Lua-error column: `take_errors` is drained every frame by `ui_script::input` and
    // `ui_script::extract`, so Lua raises show as `WARN ui_script:` lines instead.
    let _ = (arena, script);
}

/// The run's last act: read the battle map, toggle the scoreboard and the battlefield minimap
/// (opened by a keybind, never by an event), then leave the battleground.
fn report(arena: &Arena, net: &NetCommands, queue: &BattlefieldQueue, script: &mut UiScript) {
    // Whether the battle map has anything to draw: `BattlefieldMinimap_Update` returns early when
    // `GetMapInfo()` is nil (`Blizzard_BattlefieldMinimap.lua:84`), which `IsShown()` cannot see.
    let mapinfo = script
        .eval::<String>(
            r#"
            local f, sx, sy, ox, oy = GetMapInfo();
            return "file=" .. tostring(f)
                .. " continent=" .. tostring(GetCurrentMapContinent())
                .. " zone=" .. tostring(GetCurrentMapZone())
                .. " overlays=" .. tostring(GetNumMapOverlays())
                .. " landmarks=" .. tostring(GetNumMapLandmarks());
            "#,
        )
        .unwrap_or_else(|e| format!("<raised: {e}>"));
    info!("PROBE bg: MAPINFO {mapinfo}");

    // Read both before toggling: `WorldStateScoreFrame_Update` shows the scoreboard once there is
    // a winner (`WorldStateFrame.lua:192-194`), so the toggle would hide it. Through `getglobal`
    // because `BattlefieldMinimap` is load-on-demand.
    for name in ["WorldStateScoreFrame", "BattlefieldMinimap"] {
        let shown = script
            .eval::<String>(&format!(
                "local f = getglobal(\"{name}\")                  if not f then return \"not-loaded\" end return tostring(f:IsShown())"
            ))
            .unwrap_or_else(|e| format!("<raised: {e}>"));
        info!("PROBE bg: BEFORE-TOGGLE {name}:IsShown={shown}");
    }
    for (what, chunk) in [
        ("SCOREFRAME", "ToggleWorldStateScoreFrame()"),
        ("BFMINIMAP", "ToggleBattlefieldMinimap()"),
    ] {
        match script.eval::<()>(chunk) {
            Ok(()) => {
                let shown = script
                    .eval::<String>(&format!(
                        "return tostring({}:IsShown())",
                        if what == "SCOREFRAME" {
                            "WorldStateScoreFrame"
                        } else {
                            "BattlefieldMinimap"
                        }
                    ))
                    .unwrap_or_else(|e| format!("<raised: {e}>"));
                info!("PROBE bg: {what} {chunk} ok, IsShown={shown}");
            }
            Err(e) => error!("PROBE bg: {what} FAILURE — {chunk} raised: {e}"),
        }
    }
    for e in script.take_errors() {
        error!("PROBE bg: LUA ERROR {e}");
    }

    // `LeaveBattlefield` sends nothing until the scoreboard's "ended" byte (`0x4abe66`), so a live
    // match is left by a GM teleport; the server clears the slot on the map change.
    info!(
        "PROBE bg: LEAVING map {} (active_map={:?})",
        arena.map,
        queue.active_map()
    );
    let _ = script.eval::<()>("LeaveBattlefield()");
    // Not `.recall`: every `.go` saves the recall point (`TeleportCommands.cpp:755`), so after
    // the flag leg it lies inside the battleground. A hop to map 0 is a real map change.
    let [x, y, z] = arena.at;
    gm(net, format!(".go xyz {x} {y} {z} 0"));
    info!("PROBE bg: DONE");
}
