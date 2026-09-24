//! The **inside-a-battleground** live probe (`WOW_PROBE_BG=wsg|ab|av`) — the instrument that turns
//! "does a battleground work?" from a question nobody can answer into a census anyone can read.
//! Inert without the env.
//!
//! ## Why this exists
//!
//! Decisions 1963 / 1972 / 1974 / 1980 built the whole battleground **interface** — the queue, the
//! list window, the scoreboard, the always-up readout, the map blips, forty-odd Lua verbs — and
//! every one of them off the reference's own FrameXML. What none of them could build is the thing
//! on the far side of the port button: **nothing in this tree has ever entered map 489, 529 or
//! 30.** `ProbeBgQueuePlugin` stops at the queue by design (it is 2232's login-burst fixture);
//! the terrain, the world states, the objects, the spirit guides and the match's own end have
//! never been observed at all. So the gaps are not *known* to be gaps — they are unmeasured, and
//! an unmeasured subsystem is where a "should work by construction" quietly isn't.
//!
//! This probe is the measurement. It walks the real player road — level, greet a battlemaster on
//! the wire, queue, take the port through the **stock Lua verb** the reference's own dialog calls
//! — and then, from inside, prints a structured census every [`CENSUS_EVERY`] until
//! [`census_samples`] are in. Every later battleground round is judged against these lines.
//!
//! ## The one server-side lever, and why it is needed — and why it is turned back off
//!
//! A battleground does not start until `min_players_per_team` bodies are queued on each side —
//! four for Warsong Gulch on this server's patch, twenty for Alterac Valley
//! (`mangos.battleground_template`). One probe can never reach that. vmangos has the lever built
//! in: **`.debug bg`** toggles `BattleGroundMgr::m_testing`, and `BattleGroundQueue::CheckNormalMatch`
//! then starts a battleground as soon as *either* side has one player
//! (`BattleGroundMgr.cpp:557/623/756`). It is `SEC_ADMINISTRATOR` (6), exactly what a probe account
//! holds, and it is **in-memory only** — `m_testing` is initialised `false` in the manager's
//! constructor, so a server restart clears it whatever happens here.
//!
//! `.debug bg` is a **toggle, not a set**, and it announces itself to the whole realm
//! (`LANG_DEBUG_BG_ON`/`_OFF`). Both facts are reported on the probe's own lines so a run that
//! started from the wrong state reads as such rather than as a broken queue.
//!
//! **The lever is tracked by PARITY, not by state** ([`BgProbe::toggles_sent`]), and put back by
//! [`leave_testing_as_found`] — which every road to [`Phase::Done`] goes through, failures
//! included. "What is it now?" can only be answered by a world-text reply that may never arrive;
//! "how far from where I found it?" needs no reply at all. It is restored **as soon as the probe
//! is inside the instance**, because that is the moment the lever has finished its job: it is
//! needed for `CheckNormalMatch` to *form* the match, not to run one.
//!
//! Leaving it on also makes a solo battleground unendable: vmangos decrements its premature-finish
//! countdown inside an `else if (!sBattleGroundMgr.isTesting())` branch (`BattleGround.cpp:337`),
//! so while testing is on the countdown is armed and then **frozen**. Decision 2290 said the
//! opposite, from reading `GetPrematureFinishTime()` without the branch that consumes it; 2296
//! corrects it, having watched a run sit in Warsong Gulch for 492 s with `GetBattlefieldWinner()`
//! nil the whole way.
//!
//! ## What the census reports, and why each column is there
//!
//! | column | the question it answers |
//! |---|---|
//! | `map` / `area` | did the worldport land, and does the terrain know where we are? |
//! | `pos` | is the body at the battleground's own start location, or at the origin? |
//! | `terrain` | [`WorldLoadProgress`] — did map 489's ADTs stream, or is the ground missing? |
//! | `ents` | units / players / gameobjects / dynamic / corpses actually mirrored into the ECS |
//! | `gos` | the gameobject **entries** in range — the flags, the doors, the banners |
//! | `states` | the raw world-state table and its scope: the score, the flag carriers, the timer |
//! | `alwaysup` | `GetNumWorldStateUI()` — what the reference's readout would actually draw |
//! | `status` | `GetBattlefieldStatus(1..3)`, straight out of the VM |
//! | `score` | `GetNumBattlefieldScores()` / `GetBattlefieldWinner()` |
//! | `guide` | the nearest `SPIRITGUIDE`-flagged unit — the graveyard's resurrect wave |
//! | `dropped` | every opcode the codec threw on the floor, by name |
//! | `events` | the Lua event tap's window — or an explicit "the tap is broken", never silence |
//! | `mapinfo` | (at Report) `GetMapInfo()` and the overlay/landmark counts — whether the battle
//!   map has anything to **draw**, which `IsShown()` cannot tell you |
//!
//! There is deliberately **no `lua` column**; see the note at the end of [`census`] for why the one
//! that used to be here could not measure what it claimed.
//!
//! ## What this probe cannot see
//!
//! It calls [`crate::ui_chat::idle::LastInput::stamp_present`] every frame, which suppresses the
//! whole idle band — auto-sit, auto-AFK and camp alike (`ui_chat::idle::idle_action`). That is
//! deliberate and necessary (see the note at the top of [`bg_probe`]), but it means the one
//! long-running instrument that otherwise sits still for minutes can never exercise the idle
//! handler. A regression there has to be caught somewhere else.
//!
//! ## The run recipe
//!
//! ```sh
//! cd <slot> && WOW_USER=probe7 WOW_PASS=pprobe7 WOW_CHAR=Probeseven \
//!   WOW_UNATTENDED=1 WOW_NOSOUND=1 WOW_GM=off WOW_PROBE_BG=wsg \
//!   timeout 600 cargo run -p benilla 2>&1 | grep -E 'PROBE bg:'
//! ```
//!
//! Raise the `timeout` with [`census_samples`]: a default run needs ~5 min including login, and a
//! run sized to reach a match's ending needs ~10.
//!
//! `WOW_GM=off` is not optional here: GM mode re-templates the body's faction to 35, and a
//! battleground is the one place where every reaction, every objective and the server's own team
//! assignment reads off it (0649, 0679).
//!
//! Non-combat — the probe never attacks and never stands anywhere contested; with one player in
//! the instance there is nobody to fight. Pair with the checkout's probe identity (`.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR — the `probe` skill).

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

/// One battleground's fixtures: what to queue for, and which battlemaster to greet.
///
/// All three battlemasters stand within ~35 yd of each other in Stormwind's PvP alcove, so one
/// `.go` reaches any of them — which is why the probe finds its NPC by **creature entry** off the
/// guid rather than by "the nearest battlemaster", a predicate that would pick whichever of the
/// three streamed in first.
struct Arena {
    /// `WOW_PROBE_BG`'s value.
    key: &'static str,
    /// The Map.dbc row — what `CMSG_BATTLEMASTER_JOIN` carries and what the worldport must land on.
    map: u32,
    /// The battlemaster's `creature_template.entry` (vmangos `battlemaster_entry`).
    npc_entry: u32,
    /// That NPC's display name, for the probe's own lines.
    npc_name: &'static str,
    /// Its spawn — the `.go xyz` target (vmangos `creature`, map 0, Stormwind).
    at: [f32; 3],
    /// The objective this battleground's **flag leg** goes for: the gameobject entry to click and
    /// where it stands on the battleground map (vmangos `gameobject`). `None` for a battleground
    /// whose objective is not a single clickable flag.
    flag: Option<(u32, [f32; 3])>,
}

/// The three 1.12 battlegrounds. Levels are not per-arena: [`QUEUE_LEVEL`] clears every bracket
/// floor on every content patch, so the probe never has to know which patch the server runs.
const ARENAS: [Arena; 3] = [
    Arena {
        key: "wsg",
        map: 489,
        npc_entry: 14981,
        npc_name: "Elfarran",
        at: [-8454.62, 318.85, 120.97],
        // The **Warsong Flag** at the Horde base — the one an Alliance body carries. Our own
        // Silverwing Flag (179830) is the capture point, not the pickup.
        flag: Some((179831, [916.02, 1434.40, 345.41])),
    },
    Arena {
        key: "ab",
        map: 529,
        npc_entry: 15008,
        npc_name: "Lady Hoteshem",
        at: [-8420.48, 328.71, 120.89],
        // Arathi Basin's objective is five capturable banners, not a carried flag.
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

/// The level the probe body is raised to.
///
/// 60 rather than the 25 `probe_bg_queue` uses, because Alterac Valley's floor is **51** on every
/// patch that ships it and the bracket is checked at the HELLO — a body under it is refused there,
/// silently, so the list never arrives and the failure reads like a broken client.
const QUEUE_LEVEL: u32 = 60;

/// How many times to drop and re-take the queue before giving up (see the `Joined` arm).
const MAX_REJOINS: u32 = 3;

/// The `.go`'s settle radius before the battlemaster scan. Not a gate — the server applies its own
/// interaction check; this only keeps the probe from greeting a battlemaster it has not reached.
const NEAR_YD: f32 = 20.0;

/// `PLAYER_FLAGS_GM` — the bit that says the body carries GM mode's faction-35 re-template
/// (`crate::probe_shield` names the same constant).
const PLAYER_FLAGS_GM: u32 = 0x0000_0008;

/// How long the exit waits for the battleground leave to land before going anyway. The teleport
/// out is a server round trip plus a worldport; five seconds is far more than a local server needs
/// and is a backstop, not a budget.
const LEAVE_GRACE: f64 = 5.0;

/// How long to wait for `probe_shield`'s `.gm off` to land before refusing the run.
///
/// **Sized by that module's own sequencing, not by guesswork** — five seconds was measured as too
/// tight and refused a run that had `WOW_GM=off` set and was about to comply. `probe_shield` waits
/// up to `NAME_WAIT_SECS` (3 s) for the body's name to resolve, then spaces its commands
/// `STEP_SECS` (0.8 s) apart with the god line *before* the `.gm off` (the shield must be up first,
/// 0679), and the flag only clears once the server's field update comes back. Twenty seconds
/// clears all of that with room, and still fails fast against a run that is never going to comply.
const GM_DROP_WAIT: f64 = 20.0;

/// How long to keep listening for `.debug bg`'s world-text reply before giving up and saying so.
///
/// The reply crosses the network, the chat pipeline and the Lua event dispatch before it reaches
/// the tap. The old code took **one** destructive look 1.5 s after the send, which is both too
/// narrow to be reliable and, being a single sample, unable to tell "not yet" from "never".
const TOGGLE_REPLY_WINDOW: f64 = 6.0;

/// How often a census line is printed once inside.
const CENSUS_EVERY: f64 = 12.0;

/// How many census samples to take before reporting and leaving.
///
/// **Sized by the preparation phase, not by taste.** A battleground does not begin when you land
/// in it: vmangos holds every arrival behind closed doors for `BG_START_DELAY_2M` = **120 s**
/// (`BattleGround.cpp:248`, the four `BG_STARTING_EVENT_*` steps at 2 min / 1 min / 30 s / go),
/// and `.debug bg` does not shorten it. A window that ended before then would census nothing but
/// the pen and report it as the whole battleground.
///
/// Twelve × 12 s = **144 s**: the full prep, the doors dropping at ~116 s, and two samples past
/// them, which leaves room for the flag and graveyard legs inside the same run. Reaching the
/// *ending* is a different window — see [`census_samples`].
const CENSUS_SAMPLES_DEFAULT: u32 = 12;

/// The census sample count, overridable with `WOW_PROBE_BG_SAMPLES`.
///
/// The default covers the preparation phase and the doors. **The reason it is a dial** is the end
/// of a match: with one body inside, vmangos starts its `BattleGround.PrematureFinishTimer` (5 min,
/// because `CreateNewBattleGround` reads `min_players_per_team` from the template and not from the
/// testing override, so a solo match is permanently under-populated) and then ends the battleground
/// **with no winner** — `WINNER_NONE` = 2, not a team (`BattleGroundDefines.h:204`).
///
/// **Sizing it is arithmetic, not taste.** The countdown is armed when the battleground reaches
/// `STATUS_IN_PROGRESS` — the doors, ≈T+120 s — and runs 300 s from there, so the ending lands at
/// ≈T+420 s ≈ **35 samples**. `WOW_PROBE_BG_SAMPLES=36` reaches it; the 30 this doc used to
/// recommend stops 60 s short and reports "the match never ended", which reads as a defect rather
/// than as a window that was too small.
///
/// **Alterac Valley can never be ended this way at all**: the whole premature-finish block is
/// guarded by `GetTypeID() != BATTLEGROUND_AV` (`BattleGround.cpp:318`).
///
/// A value below 1 is meaningless and is clamped to 1 — the first sample is also where the testing
/// lever is put back, and a run that never censuses never restores it.
fn census_samples() -> u32 {
    std::env::var("WOW_PROBE_BG_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CENSUS_SAMPLES_DEFAULT)
        .max(1)
}

/// How often the ghost leg samples, and for how long. The battleground resurrect wave is a 30 s
/// server cycle (`BattleGround::Update`'s `m_lastResurrectTime`), so eleven 4 s samples cover a
/// full wave and a little of the next — enough to see the clock arm, tick down, and fire.
const GHOST_SAMPLE_EVERY: f64 = 4.0;
/// See [`GHOST_SAMPLE_EVERY`].
const GHOST_SAMPLES: u32 = 11;

/// How many 3 s samples the flag leg takes after the use.
const FLAG_SAMPLES: u32 = 5;

/// `SPIRITGUIDE` — `UNIT_NPC_FLAGS` bit 6, the flag the reference's area-spirit-healer acquire
/// scan `0x4924c0` keys on.
const NPC_FLAG_SPIRITGUIDE: u32 = 1 << 6;

/// The **event tap** — a Lua frame the probe installs on entry that records every battleground
/// event the reference's own interface listens for, with its `arg1`.
///
/// This exists because the first run's finding could not be read off the log at all. vmangos sends
/// three `CHAT_MSG_BG_SYSTEM_NEUTRAL` lines during the countdown (`BattleGround.cpp:383/392/400`,
/// the one-minute / half-minute / has-begun `m_startMessageIds`) and a `SMSG_PLAY_SOUND`
/// (`PlaySoundToAll(SOUND_BG_START)`), and **nothing appeared** — but an absent log line is not an
/// absent event, because a chat kind that reaches the VM logs nothing on the way. Silence in a log
/// is not evidence; a tap on the event itself is.
///
/// It is written in the reference's own Lua dialect (`this`/`event`/`arg1` globals in `OnEvent`,
/// no `ipairs` over a literal) so it exercises the same road a 1.12 addon would.
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

/// Drain the tap: everything it recorded since the last census, then empty it.
///
/// **It reports the tap's own liveness first**, and that is not decoration. `BenillaBgLog` is set
/// on `EVENT_TAP`'s first line, so a tap that raised *after* that line — or a tap installed into a
/// VM that has since been thrown away and minted fresh — leaves a table that drains cleanly to `""`
/// forever. An empty string would then be indistinguishable from "the battleground fired nothing",
/// which is the one answer this instrument must never give by accident. The sentinel makes the
/// difference readable, and [`bg_probe`] re-installs on seeing it.
const TAP_GONE: &str = "<tap-gone>";

/// See [`TAP_GONE`]: both globals are checked, because the frame is what actually receives events
/// and the table is what holds them — either one missing means the readings are not trustworthy.
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
    /// The Lua event tap is in and believed live. **Not a latch** — the drain re-checks the tap's
    /// own global every sample and clears this if the VM went away underneath it, because the
    /// interface VM is minted fresh on every world entry (`ui_script::lifecycle::mint_entry_vm`)
    /// and a tap installed into the boot VM is silently gone after the port. A dead tap that still
    /// reads `tap_installed` prints `EVENTS  (none)` forever, which is the exact inversion this
    /// instrument exists to avoid.
    tap_installed: bool,
    /// The tap raised on install and cannot be trusted. Kept apart from `tap_installed` so an
    /// `EVENTS` line can say *"the tap is broken"* rather than *"the battleground fired nothing"*.
    tap_broken: bool,
    /// **How many `.debug bg` toggles this run has sent — parity, not state.**
    ///
    /// `.debug bg` is a toggle, so "what is it now?" is the wrong question and the one the old code
    /// asked: it can only be answered by reading a world-text reply that may never arrive. "How far
    /// from where I found it?" is always answerable, needs no reply, and is what a restore actually
    /// needs. An odd count means this run left the lever flipped and owes the realm one more send;
    /// an even count means it is already as found — including the normal road, where the run turns
    /// testing on to queue and off again once inside.
    toggles_sent: u32,
    /// What the tap last said the lever reads: `Some(true)` = on ("…for debugging"), `Some(false)` =
    /// off ("…normal playercount"), `None` = no reply seen yet. Consumed once so a second pass
    /// cannot re-decide on a stale marker.
    testing_seen: Option<bool>,
    /// The lever was **read back** as on before the join, rather than merely assumed. The `Joined`
    /// failure text says which, because "the flag was confirmed ON, so look elsewhere" sends the
    /// reader past the actual suspect when nothing was ever confirmed.
    testing_confirmed: bool,
    /// How many times the queue has been dropped and re-taken (see the `Joined` arm).
    rejoins: u32,
    /// The `AppExit` has been written — `Done` is re-entered every frame until the app actually
    /// stops, and a second exit write would be noise.
    exited: bool,
    /// When [`Phase::Done`] was first seen, so the exit can wait for the leave to land.
    done_at: Option<f64>,
    /// When the `Wait` arm first saw a body, so the GM-mode gate can time out rather than hang.
    waiting_since: Option<f64>,
    /// Events drained before the first census (during the toggle read-back and the queue road).
    /// Carried rather than discarded: the old code threw them away and then attributed them to the
    /// first 12 s census window, which is a reading about the wrong interval.
    pending_events: String,
}

/// `Wait` → (levelled, hop sent) `Hopped` → (`.debug bg` sent) `Toggled` → (reply read, testing
/// confirmed on, battlemaster greeted) `Greeted` →
/// (join sent) `Joined` → (the port taken) `Ported` → (map 489 reached) `Inside` →
/// (`.die` sent) `Dying` → (released, ghost at the graveyard) `Ghost` → `Done`.
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
    /// Looking for the battlemaster, **after** the testing lever is settled. Split out of `Toggled`
    /// so the reply read happens once: the old code re-ran its destructive drain every frame of the
    /// 15 s scan, which threw away every tap event that arrived during it and re-printed a
    /// "TESTING unread" warning over the "TESTING on" line it had already printed.
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

/// Which battleground this run is for — `WOW_PROBE_BG`'s value, defaulting to Warsong Gulch (the
/// smallest, fastest and the only one whose objectives fit in one census window).
fn arena() -> &'static Arena {
    let want = std::env::var("WOW_PROBE_BG").unwrap_or_default();
    ARENAS
        .iter()
        .find(|a| a.key.eq_ignore_ascii_case(&want))
        .unwrap_or(&ARENAS[0])
}

/// A GM dot-command on the real chat wire; every reply is echoed by `net` as
/// `server says — …`, so a command that did nothing is visible as such.
fn gm(net: &NetCommands, text: impl Into<String>) {
    let _ = net.0.send(ClientCommand::Chat {
        kind: ChatKind::Say,
        target: None,
        text: text.into(),
    });
}

/// Drain the event tap, distinguishing its three outcomes rather than flattening them to `""`.
///
/// Returns the events, or a bracketed reason. A `TAP_GONE` sentinel also **clears
/// `tap_installed`**, so the next frame re-installs into whatever VM is live now — which is the
/// self-heal for the one-frame world-entry window where the tap can land in a boot VM that is
/// about to be discarded (`ui_script::lifecycle`).
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

/// Put the realm's `.debug bg` lever back if **this run** is the one that moved it.
///
/// Every road to [`Phase::Done`] goes through this, and that is the whole point. `.debug bg` is a
/// realm-global, in-memory toggle (`BattleGroundMgr::ToggleTesting`) that announces itself to every
/// player in the world, and it is a **toggle, not a set** — so a run that ends without restoring it
/// hands the next run the opposite of what it expects. That is not hypothetical: it is how runs 2,
/// 4 and 5 of this probe failed (2290 §2). The old code restored it at exactly one place, an
/// equality test on the census counter, which every failure path and every
/// `WOW_PROBE_BG_SAMPLES < DOORS_SAMPLE` run skipped silently.
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

/// Send `.debug bg` and count it. **Every** send goes through here — the parity in
/// [`leave_testing_as_found`] is only as good as the counting.
fn toggle_testing(probe: &mut BgProbe, net: &NetCommands) {
    probe.toggles_sent += 1;
    gm(net, ".debug bg");
}

/// End the run, restoring anything this run changed on the server first.
fn finish(probe: &mut BgProbe, net: &NetCommands) {
    leave_testing_as_found(probe, net);
    probe.phase = Phase::Done;
}

/// A world state, distinguishing **absent** from **zero**.
///
/// [`WorldStates::get`] is reference-faithful: a key the server never sent reads `0`, because that
/// is what the real client's table does. That is right for the client and wrong for an instrument —
/// `ws2338=0` reads as a well-formed "the flag is at its base" when it actually means "no
/// `SMSG_INIT_WORLD_STATES` ever carried this key". vmangos writes only 1 or 2 into the flag rows
/// (`BattleGroundWS.cpp:509-515`, all eight call sites), so a `0` there is *always* a missing
/// reading — and the person reading the log is exactly the person who does not know that yet.
fn ws(states: &WorldStates, key: u32) -> String {
    states
        .pairs()
        .find(|(k, _)| *k == key)
        .map_or_else(|| "absent".to_string(), |(_, v)| v.to_string())
}

// One Bevy system's full input set — the guard-poi probe's shape, plus the census reads.
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

    // **This probe stands in for a player who is AT the keyboard.** benilla implements the
    // reference's idle handler faithfully (`ui_chat::idle`: auto-sit then auto-AFK at 300 000 ms
    // of no input), and vmangos removes an AFK player from a battleground outright
    // (`Player::ToggleAFK` → `LeaveBattleground`). Two correct behaviours, one on each side, and
    // together they eject an unattended probe five minutes in — which is exactly what the first
    // long run hit: at t=300 s the census read Stormwind, not Warsong Gulch, and the chat said
    // "You are now AFK". A battleground match cannot be watched to its end without this.
    idle.stamp_present(std::time::Duration::from_secs_f64(now));

    // **The tap goes in as soon as there is a VM to put it in, and not before.** The probe's own
    // first frame is a world-entry frame: the in-game interface has not materialized yet, so an
    // install at Setup gets `no VM` and every later `EVENTS` line reads `(none)` — which looks
    // exactly like "the battleground fired no events" and is the most misleading answer this
    // instrument could give. Retried every frame until it lands; one bool test once it has.
    if !probe.tap_installed {
        if let Some(script) = script.as_deref_mut() {
            match script.eval::<()>(EVENT_TAP) {
                Ok(()) => {
                    probe.tap_installed = true;
                    info!("PROBE bg: TAP installed");
                }
                Err(e) => {
                    probe.tap_installed = true; // a raise will not fix itself; say so once
                    probe.tap_broken = true; // …and every later EVENTS line says so too
                    error!("PROBE bg: TAP FAILURE — {e}");
                }
            }
        }
    }

    match probe.phase {
        Phase::Wait => {
            // **GM mode makes every objective in this probe fail silently, so refuse to start in
            // it.** vmangos's `Player::CanUseBattleGroundObject` opens with
            // `if (IsGameMaster()) return false;` (`Player.cpp:20576`), which means the flag click,
            // the Arathi Basin banners and Alterac Valley's towers are all rejected server-side
            // with no reply — while the queue, the port, the census, the world states and the
            // scoreboard all keep working perfectly. That is the worst possible failure shape: a
            // run that looks 90% right and is wrong exactly where the objectives are.
            //
            // MEASURED, by getting it wrong: a run without `WOW_GM=off` reached the Warsong Flag at
            // 1.8 yd, sent `CMSG_GAMEOBJ_USE`, and reported `ws2339=1` with no banner aura and no
            // `CHAT_MSG_BG_SYSTEM_ALLIANCE` — indistinguishable, from the probe's own lines, from a
            // client that cannot pick up flags. `probe_shield` drops GM mode when `WOW_GM=off` is
            // set (0679), so this WAITS for the flag to clear rather than refusing outright: the
            // shield arms and drops GM a few hundred ms into the world, after this arm first runs.
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
            // Revive first, unconditionally: `CanInteractWithNPC` refuses a dead player outright,
            // and the refusal surfaces as an anticheat line about an "invalid creature" — which
            // reads like a wrong guid, not like a corpse (`probe_bg_queue`'s note).
            gm(&net, ".revive");
            // **Wait for the real level rather than assuming one.** `unit_level()` is `None` until
            // `UNIT_FIELD_LEVEL` is in the store, and this arm fires on the first frame the
            // `SelfPlayer` entity is queryable. Reading a missing field as `0` sends
            // `.levelup 60` to a body that may already be 60 — which makes it **120**, above every
            // bracket ceiling (`battleground_template.max_lvl = 60`), and the run then dies at
            // `Greeted` blaming a level *under* the floor. A missing reading is not a zero.
            let Some(level) = store.0.unit_level() else {
                return;
            };
            if level < QUEUE_LEVEL {
                gm(&net, format!(".levelup {}", QUEUE_LEVEL - level));
            }
            // **Deserter first.** Leaving a battleground any way but "the match ended" earns
            // spell **26013** (vmangos `Player.cpp:2178/18816`, `Battleground.CastDeserter`
            // defaults true), and `HandleBattlemasterJoinOpcode` then refuses the join outright
            // (`BG_GROUPJOIN_DESERTERS`) — so the *second* run of this probe never queues at all,
            // and reads as "the queue is broken" rather than "the last run left a debuff". Found
            // the hard way; cleared unconditionally, because a body without it does not care.
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
            // **The lever goes here, not in Setup, and its reply is read before anything queues.**
            // `.debug bg` is a TOGGLE (`BattleGroundMgr::ToggleTesting`), so a blind send is not
            // idempotent and a run that died before its own undo flips the next one the wrong way.
            // Correcting it *after* joining does not work either, and that is the part worth
            // writing down: vmangos evaluates a queue only when something SCHEDULES an update
            // (`BattleGroundMgr::Update` drains `m_queueUpdateScheduler`, which a join or a leave
            // fills — `BattleGroundMgr.cpp:1005-1028`). Flipping the flag under a queue entry that
            // is already sitting there schedules nothing, so the entry waits forever. Hence: get
            // the flag right first, then join once.
            toggle_testing(&mut probe, &net);
            probe.phase = Phase::Toggled {
                sent_at: now,
                corrected: false,
            };
        }
        Phase::Toggled { sent_at, corrected } => {
            // **Drain every frame and keep what comes back**, rather than taking one destructive
            // look 1.5 s in. The reply is a world text that crosses the network, reaches the chat
            // pipeline and then the tap; a single window wide enough to be reliable is wider than
            // one worth waiting on every run, and a single window narrow enough to be quick misses
            // the reply often enough to matter — a missed reply is how this probe misdiagnoses
            // itself. Anything drained here that is *not* the reply is kept for the first census
            // instead of being thrown on the floor.
            if let Some(s) = script.as_deref_mut() {
                let seen = drain_events(&mut probe, s);
                // The LAST marker in the chunk wins, because both can appear in one drain when the
                // correcting send's reply arrives alongside the first. `contains` cannot express
                // that and was the reason a corrected run could read its own first reply again.
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
                    // Unread is a THIRD outcome, and it is carried forward rather than rounded to
                    // "on": the `Joined` failure text used to assert the flag "was confirmed ON
                    // before the join" even on this path, sending the reader to look at Deserter
                    // and brackets when the lever was the actual suspect.
                    warn!(
                        "PROBE bg: TESTING unread — no `.debug bg` reply reached the tap in \
                         {TOGGLE_REPLY_WINDOW:.0}s; continuing, but if the queue never pops, this \
                         is the first thing to doubt"
                    );
                    probe.phase = Phase::Scanning { since: now };
                }
                // Still inside the window: stay in `Toggled` and look again next frame.
                None => {}
            }
        }
        Phase::Scanning { since: sent_at } => {
            let here = player.pos;
            // By ENTRY, not by "the nearest battlemaster": all three stand in one alcove.
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
            // Waiting for the LIST, not for a clock: its guid is the one the join must quote, and
            // its arrival is the proof the level gate was cleared.
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
            // Status 2 is "your battleground is ready, confirm within the deadline". The probe
            // takes it through the STOCK VERB, not through a hand-built packet: the whole point
            // is that the reference's own dialog road works.
            // **Matched on the MAP as well as the status**, because a slot is not ours just because
            // it is ready. A killed run can leave this character queued for another battleground,
            // and vmangos re-sends that `SMSG_BATTLEFIELD_STATUS` at login — so a status-2 slot for
            // Arathi Basin would be taken here, port the body into map 529, and the `Ported` arm
            // would then time out complaining that the map "is still 529, not 489". The cause would
            // read as a broken worldport; it is a stale queue entry the probe confirmed on purpose.
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
                // **Leave the queue and join again.** vmangos evaluates a queue only when
                // something SCHEDULES an update, and a queue entry that was scheduled once and
                // did not match just sits there — no timer re-examines it
                // (`BattleGroundMgr::Update` drains `m_queueUpdateScheduler` and nothing refills
                // it on its own, `BattleGroundMgr.cpp:1005`). Whatever left the server's
                // bookkeeping unable to match this entry, a fresh one gets a fresh evaluation.
                // Both halves are real client verbs: `AcceptBattlefieldPort(slot, 0)` is the
                // queue's own decline, which no other probe or test here has ever walked.
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
            // Drained HERE, not inside `census`, so the tap's self-heal can reach `probe` — and so
            // everything the tap caught on the queue road is attributed to the window it actually
            // happened in rather than being folded into the first census.
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

            // **Once inside, the lever has done its job — put it back immediately.**
            //
            // It was an equality test on `DOORS_SAMPLE` (11), and that was wrong twice over. Any
            // `WOW_PROBE_BG_SAMPLES` below 11 — a documented, user-facing dial — skipped it
            // entirely and left the realm-global flag flipped for the next run, which is the exact
            // breakage 2290 §2 records as having cost runs 2, 4 and 5. And there was no reason to
            // wait: the flag is needed only for `CheckNormalMatch` to *form* the match, which has
            // already happened by the time we are standing in it. vmangos arms the premature
            // countdown when the battleground reaches `STATUS_IN_PROGRESS` (the doors) and
            // decrements it inside `else if (!sBattleGroundMgr.isTesting())` (`BattleGround.cpp:337`),
            // so restoring it now simply lets that clock run from the doors rather than from
            // sample 11 — twelve seconds *earlier* to an ending, not later.
            if samples == 1 {
                info!(
                    "PROBE bg: TESTING — inside the instance; the lever has done its job and the \
                     premature-finish countdown is frozen while it is on"
                );
                leave_testing_as_found(&mut probe, &net);
            }

            if samples >= census_samples() {
                // **The graveyard leg.** `.die` is the one thing that clears the probe's god
                // shield by design (0677), so a probe CAN die on purpose; a battleground death is
                // also the only place the area-spirit-healer arc (2291) is reachable at all, and
                // a mechanism that has never been watched fire is not a mechanism anyone should
                // claim. Non-combat: nothing kills us, we ask to be dead.
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
                return; // let the decline land and the slot clear
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
                return; // the hop crosses the map; let the far base stream in
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
                    // What the click's own GameObject ladder would decide, reported rather than
                    // assumed: a FLAGSTAND carries no lock, so `resolve_go_action` takes its
                    // `Use` arm and the packet is `CMSG_GAMEOBJ_USE` — the one sent here.
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
            // **The two readings that say the flag is on the body.** World state 2339 is the
            // ALLIANCE readout row's icon selector, and it is only ever 1 or 2 — 2 meaning the
            // flag that row is about is being carried (vmangos `BattleGroundWS.cpp:512` and
            // `FillInitialWorldStates` 713-722). It is **not** the four-value `m_flagState` enum,
            // which never reaches the client at all; that distinction is why a census reading
            // `2338=1 2339=1` for a whole match is correct rather than a stuck value. The aura is
            // the other half: what the reference actually draws on the body.
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
            // The whole point of this leg, in four readings: are we a ghost, is a spirit guide in
            // range, did the engine adopt it (`GetAreaSpiritHealerTime` is non-zero only once
            // `SMSG_AREA_SPIRIT_HEALER_TIME` answered the query the adopt sent), and did the
            // stock dialog's event fire.
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
                // Take the wave if one is offered — the verb that was permanently silent before
                // 2291, and the only way back to the world from a battleground graveyard.
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

/// **Reaching `Done` ends the process.** This probe was the one live probe here without it
/// (`probe_auction`'s tail and `ProbeExitPlugin::fire_probe_exit` are the established shape), so
/// the client simply idled at `Done` until whatever external `timeout` wrapped the run killed it:
/// a four-minute Warsong Gulch census cost a fifteen-minute wall clock, and a sequence of runs
/// spent most of its time watching a finished probe stand still.
///
/// It is its own system rather than an arm of [`bg_probe`] only because that system is already at
/// Bevy's parameter ceiling — there is nothing conditional about the split.
///
/// **It waits for the leave to land before it goes**, and that wait is the whole reason this is not
/// a one-liner. [`report`] ends by teleporting the body out to map 0, and `AppExit` stops the
/// Update schedule — so exiting on the next frame can cut the connection before the worldport
/// completes and strand the body **inside a live battleground**, which is precisely the poisoned
/// state this probe warns the next run about. So: leave first, confirmed by the map actually
/// changing, then exit; with [`LEAVE_GRACE`] as the backstop for a leave that never lands, because
/// hanging forever would be worse than logging out somewhere awkward.
///
/// The polite `AppExit` plus a hard backstop on its own OS thread is the pattern, not belt and
/// braces: `AppExit` stops the Update schedule, so an in-schedule backstop could never fire, and a
/// net/winit teardown hang would otherwise leave a zombie client holding the probe account.
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

/// One census sample — eleven greppable `PROBE bg:` lines describing everything the client can see
/// from inside the battleground. Deliberately verbose: this is the first look anyone has had, and
/// a column nobody needed is cheaper than a round trip for one that was left out.
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
        // `map=none` rather than `map=0`: 0 is Eastern Kingdoms, a real answer, and printing it
        // for "there is no `CurrentMap` resource" is the same missing-reads-as-a-value mistake
        // the world-state column had.
        "PROBE bg: CENSUS t={t:.0}s map={} area={:?} pos=({:.0},{:.0},{:.0}) terrain={terrain}",
        map.map_or_else(|| "none".to_string(), |m| m.0.to_string()),
        area.and_then(|a| a.0),
        p.x,
        p.y,
        p.z,
    );

    // Entities, by kind — "is there a world here at all?"
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
                    // The NAME, not just the entry: "179918" is a number to look up in a database
                    // and `Doodad_PortcullisActive01` is the battleground's gate. The cache is the
                    // ask-once `GAMEOBJECT_QUERY` store (0239) the client already fills for every
                    // GO that streams in, so this costs nothing and answers for free.
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

    // The world-state table — the battleground's whole score, as raw dwords.
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

    // What the reference's own readout would draw, straight out of the VM.
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

    // **The wire-coverage tally** — every opcode the codec threw on the floor, by name. This is
    // the column the first run did not have and most needed: a battleground that sends something
    // benilla has no arm for is invisible in every other reading here, and reads as "nothing
    // happened" rather than as a gap. `DroppedOpcodes` is never cleared, so the numbers are
    // cumulative over the whole session — a row that grows between samples is the finding.
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

    // The event tap's window (see `EVENT_TAP`): what the VM was actually told since the last
    // sample. An empty window while the server is running the start countdown is a finding —
    // **provided the tap is alive**, which is why a broken tap says so instead of printing
    // `(none)`. "The battleground fired nothing" and "nobody was listening" are opposite
    // conclusions and used to produce identical lines.
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

    // **No `LUA` column, deliberately.** It used to print `LUA clean` off `take_errors()`, which
    // reads as "the interface raised nothing inside a battleground for the last 12 s". It cannot
    // mean that: `take_errors` is a DRAIN, and two systems already drain it every frame
    // (`ui_script::input` and `ui_script::extract::extract`), so this call could only ever see
    // whatever our own `eval`s in this same frame had just raised. A column that reports a 12 s
    // window while measuring a fraction of one frame is worse than no column, because it reads as
    // evidence. Engine-side Lua raises appear as `WARN ui_script:` lines — grep for those.
    let _ = (arena, script);
}

/// The run's last act: drive the two battleground windows the census cannot reach by watching
/// (they are opened by a keybind, never by an event), then put the server's testing toggle back
/// and leave the battleground the way a player does.
fn report(arena: &Arena, net: &NetCommands, queue: &BattlefieldQueue, script: &mut UiScript) {
    // The scoreboard and the battlefield minimap: both are LoadOnDemand-shaped roads nothing in
    // this tree has ever walked. A raise here is the finding.
    // **Read both windows BEFORE toggling them.** The scoreboard shows itself when a match ends —
    // `WorldStateScoreFrame_Update` does `if (GetBattlefieldWinner()) then ShowUIPanel(...)`,
    // driven by `UPDATE_BATTLEFIELD_SCORE` — so after a real ending the toggle below *hides* it,
    // and reading only the post-toggle state reports `IsShown=nil` for the run where the feature
    // worked. That is the wrong way round, and it cost a round of inference to notice.
    //
    // Through `getglobal`, because `BattlefieldMinimap` is LoadOnDemand: indexing it before the
    // toggle that loads it is a nil-index raise, and "not loaded yet" is a reading, not an error.
    // **Does the battle map have anything to draw?** `IsShown()` alone cannot say: the stock
    // `BattlefieldMinimap_Update` bails on its fourth line when `GetMapInfo()` is nil, so a window
    // that is up and completely empty reports exactly the same `IsShown=1` as one full of terrain.
    // These four readings are the difference, and they are the ones the minimap round never took.
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

    // `LeaveBattlefield` is gated client-side on the scoreboard's "ended" byte (1972), so while a
    // match is live the verb is correctly silent — which means the honest way out is the same one
    // a player uses when they give up on a battleground: the GM teleport home. The queue slot the
    // server still holds is cleared by the leave it sends on the map change.
    info!(
        "PROBE bg: LEAVING map {} (active_map={:?})",
        arena.map,
        queue.active_map()
    );
    let _ = script.eval::<()>("LeaveBattlefield()");
    // **NOT `.recall`.** vmangos saves the recall point on *every* `.go`
    // (`HandleGoHelper` → `SaveRecallPosition()`, `TeleportCommands.cpp:755`), so the flag leg's
    // `.go xyz … 489` overwrote it with a position **inside the battleground**. `.recall` then
    // resolves to the same map and `Player::TeleportTo` takes its near-teleport arm — the probe
    // printed `LEAVING map 489` and went nowhere, leaving the body logged out inside a live
    // instance and the queue slot uncleared. That is the poisoned state the `Joined` failure text
    // warns the *next* run about, manufactured by this one. An explicit hop to the battlemaster
    // alcove on map 0 is a real map change, which is what clears the slot.
    let [x, y, z] = arena.at;
    gm(net, format!(".go xyz {x} {y} {z} 0"));
    info!("PROBE bg: DONE");
}
