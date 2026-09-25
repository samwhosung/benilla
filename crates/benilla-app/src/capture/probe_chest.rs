//! The chest live probe (`WOW_PROBE_CHEST=1`) — **B84's instrument**: does the player kneel at an
//! open chest?
//!
//! The symptom: no looting animation when opening a chest. At the same Mirror Lake Orchard chest,
//! benilla and the reference side by side show the reference
//! gnome kneeling over the open loot window, ours standing bolt upright. Decision 0515 had shipped
//! the kneel keyed to the `CMSG_LOOT` send alone and recorded a chest's silence as *correct*; 1471
//! is the correction (the real client also arms the latch in `OnLootResponse 0x5eb900`, and a chest
//! never sends `CMSG_LOOT` at all).
//!
//! **A kneel is a number here, not a picture** (`docs/METHOD.md` step 4: timing and pose questions are
//! settled with instruments). The probe reads the self unit's own
//! [`AnimDriver::active_anim`][crate::creature_anim::AnimDriver::active_anim] — the id the base
//! selector actually landed on — so the verdict is `50` (`Loot`) or it is not, with no screenshot
//! and no eye in the loop. Three readings, in order, which is what makes it a *regression* test and
//! not a one-sided assertion:
//!
//! 1. **BEFORE** — parked at the chest, window shut: the base anim must be `0` (`Stand`). This is
//!    the control; a probe that only ever sees `50` cannot tell a fix from a stuck kneel.
//! 2. **OPEN** — the chest used on the click's own route, the loot window up: must be `50`.
//! 3. **AFTER** — `CloseLoot()` through the live VM: back to `0`.
//!
//! Nothing here re-decides what packet a chest takes. The probe resolves the object through
//! [`crate::target::click::resolve_go_action`] — the same lock chain the mouse runs —
//! and sends whichever arm it names, so a chest whose `Lock.dbc` row changes shape still gets the
//! packet the real click would send. (Every world chest carries a lock: `Lock.dbc` 43 is one
//! `LockType 13 "Open Kneeling"` slot, 57 is `5 "Open"` + `6 "Treasure (DND)"`, all `Skill 0`, so
//! the "Opening" spell every character knows satisfies them and the live arm is `OpenLock`.)
//!
//! The DONE line reports the two mechanism halves separately — `latch-while-open` (predicate A: a
//! loot session is open) and `kneel-while-open` (predicate B: this *kind* of target is knelt at,
//! decision 1477). They are not the same question, and the whole of B84's correction lives in the
//! gap: a fishing bobber arms the latch exactly as a chest does and must still read `kneel=false`.
//!
//! ## The run recipe
//!
//! ```text
//! WOW_NOSOUND=1 WOW_USER=probe0 WOW_PASS=pprobe0 WOW_CHAR=Probezero \
//!     WOW_PROBE_CHEST=1 cargo run -q -p benilla
//! ```
//! (the checkout's probe identity — `.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR; the `probe`
//! skill). `WOW_PROBE_CHEST=<x>,<y>,<z>[,<map>]` aims it at a different
//! object; the default is a live `Battered Chest` spawn (`gameobject.guid` 26756, template 2843) in
//! the Mirror Lake stretch of Elwynn, the same corner of the world the report came from. A chest is
//! a respawning spawn point: if someone emptied that one it will be missing for its respawn timer,
//! which the probe reports as a scan failure rather than as a passing run.
//!
//! Grep `PROBE_CHEST:` for the verdict; the probe self-exits when it lands.

use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::creature_anim::AnimDriver;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::target::click::{resolve_go_action, GoAction};
use crate::target::lock::GoLockInputs;
use crate::ui_loot::{LootKneel, LootLatch, LootState};

/// The probe's default object: the `Worn Wooden Chest` at `gameobject` 3998644 (template 1765,
/// `Lock.dbc` 43 → `LockType 13 "Open Kneeling"`, loot 1570) below Northshire in Elwynn.
///
/// **Not** the reporter's own Mirror Lake `Battered Chest`, deliberately, and the reason is the
/// trap this probe fell into first: every ordinary world chest — that one included — is a
/// **spawn-pool** member (`pool_gameobject`, one of pool 20004's many points spawned at a time), so
/// a fixed coordinate finds nothing most of the time and the probe reads "no chest" where the real
/// answer is "not this point today". 3998644 is unpooled with a 25 s respawn, so it is there every
/// run and it comes back between runs. It is the same GameObject type, the same lock family and the
/// same `SMSG_LOOT_RESPONSE`-only session as the reported object.
const CHEST_AT: [f32; 4] = [-9474.37, 111.25, 57.03, 0.0];
/// `GAMEOBJECT_TYPE_CHEST` — the strategy type a chest (and a herb/mining node) carries.
const GO_TYPE_CHEST: i32 = 3;
/// `AnimationData.dbc` ids the verdict is written in: the kneel we want, and the Stand we must
/// return to. Named rather than inlined so the report reads as animations, not magic numbers.
const ANIM_LOOT: u16 = 50;
const ANIM_STAND: u16 = 0;

/// Scan radius around the landing spot, in yards.
const SCAN_RANGE: f32 = 25.0;
/// Frames each window samples. The base anim is a settled per-frame state, so this is about
/// outlasting a transient, not about statistics.
const SAMPLE_FRAMES: usize = 60;
const SETTLE_SECS: f64 = 5.0;
const SCAN_TIMEOUT_SECS: f64 = 25.0;
const LOOT_TIMEOUT_SECS: f64 = 15.0;
const CLOSE_TIMEOUT_SECS: f64 = 10.0;

/// The streamed-object columns the scan and the lock resolve both read — a `type` alias so the
/// one `Query` stays inside clippy's complexity bar.
type NearbyObject = (
    &'static Guid,
    &'static NetEntity,
    &'static ObjectStore,
    &'static Transform,
    Option<&'static crate::go_anim::GoAnim>,
);

pub(crate) struct ProbeChestPlugin;

impl Plugin for ProbeChestPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChestProbe>()
            .add_systems(Update, chest_probe);
    }
}

#[derive(Resource, Default)]
struct ChestProbe {
    phase: Phase,
    /// The chest's guid, once the scan finds it.
    chest: Option<u64>,
    /// The base anim ids seen in each window, in frame order.
    before: Vec<Option<u16>>,
    open: Vec<Option<u16>>,
    after: Vec<Option<u16>>,
    /// The two mechanism readings taken when the OPEN window finished — reported beside the pose
    /// so a failure says *which* half broke. They are deliberately separate: predicate A (a loot
    /// session is open — the latch) and predicate B (this KIND of target is knelt at — decision
    /// 1477). A fishing bobber is `latch=Some(...) kneel=false`, and that is correct.
    latch_open: Option<u64>,
    kneel_open: bool,
    fails: u32,
    exited: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// `.go` issued; letting the world stream the chest in.
    Settling {
        sent_at: f64,
    },
    /// Sampling with the chest SHUT — the control window.
    Before,
    /// The use packet is out; waiting for the server to open a loot window.
    WaitLoot {
        since: f64,
    },
    /// Sampling with the loot window OPEN — the reported condition.
    Open,
    /// `CloseLoot()` called; waiting for the window to actually go.
    WaitClosed {
        since: f64,
    },
    /// Sampling after the close — the "and it stands back up" half.
    After,
    Done,
}

/// Where the probe is aimed: `WOW_PROBE_CHEST=<x>,<y>,<z>[,<map>]`, else [`CHEST_AT`]. Anything
/// unparseable falls back to the default rather than failing the run — the common value is `1`.
fn target() -> [f32; 4] {
    let Ok(raw) = std::env::var("WOW_PROBE_CHEST") else {
        return CHEST_AT;
    };
    let parts: Vec<f32> = raw
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    match parts.len() {
        3 => [parts[0], parts[1], parts[2], CHEST_AT[3]],
        4 => [parts[0], parts[1], parts[2], parts[3]],
        _ => CHEST_AT,
    }
}

/// Whether the loot window is up, asked of the **live UI** rather than of our own state: the
/// window is what the reporter saw, and `LootFrame` shows only once the feed has fired
/// `LOOT_OPENED` off the wire. Falls back to the app-side session when the frame can't be read.
fn window_open(script: &UiScript, loot: &LootState) -> bool {
    script
        .eval::<bool>("return LootFrame:IsShown() and 1 or nil")
        .unwrap_or_else(|_| loot.source().is_some())
}

/// One window's verdict line: what the base slot held, how steadily, and whether it is the id the
/// reference shows. `expect` is the id every frame of the window must carry.
fn report(label: &str, expect: u16, seen: &[Option<u16>]) -> u32 {
    let held = seen.iter().filter(|a| **a == Some(expect)).count();
    // What it held instead, most-common first — the useful half of a failure.
    let mut other: Vec<String> = Vec::new();
    for a in seen.iter().filter(|a| **a != Some(expect)) {
        let name = a.map_or_else(|| "none".to_string(), |id| id.to_string());
        if !other.contains(&name) {
            other.push(name);
        }
    }
    if held == seen.len() && !seen.is_empty() {
        info!(
            "PROBE_CHEST: {label:<6} PASS — base anim {expect} on all {}/{} frames",
            held,
            seen.len()
        );
        0
    } else {
        error!(
            "PROBE_CHEST: {label:<6} FAIL — base anim {expect} on only {}/{} frames (also saw: {})",
            held,
            seen.len(),
            if other.is_empty() {
                "nothing".to_string()
            } else {
                other.join(", ")
            }
        );
        1
    }
}

fn chest_probe(
    time: ProbeClock,
    mut probe: ResMut<ChestProbe>,
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&AnimDriver, With<SelfPlayer>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    objects: Query<NearbyObject, Without<SelfPlayer>>,
    mut go_inputs: GoLockInputs,
    actions: Res<crate::ui_action::PlayerActions>,
    loot: Res<LootState>,
    latch: Res<LootLatch>,
    kneel: Res<LootKneel>,
    net: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(driver) = self_q.single() else {
        return; // not in-world yet
    };
    let Some(script) = script else {
        return; // no UI VM this build — nothing this probe can drive
    };
    let now = time.elapsed_secs_f64();
    let anim = driver.active_anim();

    match probe.phase {
        Phase::Wait => {
            let [x, y, z, map] = target();
            info!(
                "PROBE_CHEST: heading to the chest ({x} {y} {z} map {map}) — B84's object, \
                 GameObject type {GO_TYPE_CHEST}"
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
            let me = player.pos;
            let chest = objects.iter().find(|(_, net_e, store, tf, _)| {
                net_e.kind == EntityKind::GameObject
                    && store.0.gameobject_type_id() == GO_TYPE_CHEST
                    && tf.translation.distance(me) < SCAN_RANGE
            });
            if let Some((guid, ..)) = chest {
                info!(
                    "PROBE_CHEST: chest {:#x} in range — sampling {SAMPLE_FRAMES} frames with it SHUT",
                    guid.0
                );
                probe.chest = Some(guid.0);
                probe.phase = Phase::Before;
            } else if now - sent_at > SCAN_TIMEOUT_SECS {
                error!(
                    "PROBE_CHEST: FAIL — no type-{GO_TYPE_CHEST} GameObject within {SCAN_RANGE} yd \
                     in {SCAN_TIMEOUT_SECS}s. A chest is a respawning spawn point: if it was looted \
                     recently it is simply not there yet. This is NOT a passing run"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Before => {
            probe.before.push(anim);
            if probe.before.len() < SAMPLE_FRAMES {
                return;
            }
            let Some(guid) = probe.chest else {
                probe.phase = Phase::Done;
                return;
            };
            // The click's own route: ask the shared lock chain what the right-click
            // would send, then send exactly that.
            let go = objects
                .iter()
                .find(|(g, ..)| g.0 == guid)
                .map(|(_, _, store, _, anim)| (store, crate::go_anim::go_state(anim, store)));
            let me_store = self_store.single().ok();
            match resolve_go_action(guid, &mut go_inputs, &actions.spells, go, me_store, &net) {
                GoAction::Use => {
                    info!("PROBE_CHEST: lockless — CMSG_GAMEOBJ_USE on {guid:#x}");
                    let _ = net.0.send(ClientCommand::GameObjUse { guid });
                }
                GoAction::OpenLock(spell_id) => {
                    info!("PROBE_CHEST: opener cast {spell_id} at {guid:#x} (the live arm)");
                    let _ = net.0.send(ClientCommand::CastSpellGameObject {
                        spell_id,
                        go_guid: guid,
                    });
                }
                GoAction::OpenByKey { .. } | GoAction::Refuse(_) => {
                    error!(
                        "PROBE_CHEST: FAIL — the lock chain refuses this object (key/unmet). Aim \
                         the probe at a chest this character can actually open"
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                    return;
                }
            }
            probe.phase = Phase::WaitLoot { since: now };
        }
        Phase::WaitLoot { since } => {
            if window_open(&script, &loot) {
                info!(
                    "PROBE_CHEST: loot window up on {:#x} — sampling {SAMPLE_FRAMES} frames OPEN",
                    loot.source().unwrap_or_default()
                );
                probe.phase = Phase::Open;
            } else if now - since > LOOT_TIMEOUT_SECS {
                error!(
                    "PROBE_CHEST: FAIL — no loot window within {LOOT_TIMEOUT_SECS}s of the use; \
                     the anim question can't be asked at all"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Open => {
            probe.open.push(anim);
            if probe.open.len() >= SAMPLE_FRAMES {
                probe.latch_open = latch.0;
                probe.kneel_open = kneel.0;
                info!("PROBE_CHEST: closing the window through the live VM's own CloseLoot()");
                let _ = script.eval::<()>("CloseLoot()");
                probe.phase = Phase::WaitClosed { since: now };
            }
        }
        Phase::WaitClosed { since } => {
            if !window_open(&script, &loot) {
                info!("PROBE_CHEST: window closed — sampling {SAMPLE_FRAMES} frames AFTER");
                probe.phase = Phase::After;
            } else if now - since > CLOSE_TIMEOUT_SECS {
                error!(
                    "PROBE_CHEST: FAIL — the loot window never closed within {CLOSE_TIMEOUT_SECS}s"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::After => {
            probe.after.push(anim);
            if probe.after.len() >= SAMPLE_FRAMES {
                probe.phase = Phase::Done;
            }
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            let mut fails = probe.fails;
            fails += report("BEFORE", ANIM_STAND, &probe.before);
            fails += report("OPEN", ANIM_LOOT, &probe.open);
            fails += report("AFTER", ANIM_STAND, &probe.after);
            info!(
                "PROBE_CHEST: DONE chest={:#x} latch-while-open={:?} kneel-while-open={} fail={fails}",
                probe.chest.unwrap_or_default(),
                probe.latch_open.map(|g| format!("{g:#x}")),
                probe.kneel_open,
            );
            // The probe self-exit pattern (`ProbeExitPlugin::fire_probe_exit`): a polite AppExit
            // plus a hard backstop thread, so a net/winit teardown hang can't leave a zombie
            // client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_CHEST: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
