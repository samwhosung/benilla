//! The chest live probe (`WOW_PROBE_CHEST=1`): does the player kneel at an open chest? The
//! reference arms the loot latch in `OnLootResponse` (`0x5eb900`) as well as on the `CMSG_LOOT`
//! send, and a chest never sends `CMSG_LOOT`.
//!
//! It reads the self unit's
//! [`AnimDriver::active_anim`][crate::creature_anim::AnimDriver::active_anim] in three windows:
//!
//! 1. before, window shut: `0` (`Stand`), the control;
//! 2. open, the chest used through the click's own route: `50` (`Loot`);
//! 3. after `CloseLoot()`: back to `0`.
//!
//! The use goes through [`crate::target::click::resolve_go_action`], the mouse's lock chain, and
//! sends whichever arm it names; world chests carry a `Skill 0` lock the "Opening" spell meets, so
//! the live arm is `OpenLock`. The DONE line reports the latch (a loot session is open) and the
//! kneel (this kind of target is knelt at) apart: a fishing bobber latches but must not kneel.
//!
//! `WOW_PROBE_CHEST=<x>,<y>,<z>[,<map>]` aims it elsewhere; grep `PROBE_CHEST:` for the verdict.
//! The switches are `docs/CONTRIBUTING.md`, "Running it unattended".

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

/// The default object: the `Worn Wooden Chest` at `gameobject` 3998644 (template 1765,
/// `Lock.dbc` 43, `LockType 13 "Open Kneeling"`) in Elwynn. Ordinary world chests are spawn-pool
/// members, absent from a fixed point most of the time; this one is unpooled with a 25 s respawn.
const CHEST_AT: [f32; 4] = [-9474.37, 111.25, 57.03, 0.0];
/// `GAMEOBJECT_TYPE_CHEST`, also a herb or mining node.
const GO_TYPE_CHEST: i32 = 3;
/// `AnimationData.dbc` ids: the kneel, and the stand to return to.
const ANIM_LOOT: u16 = 50;
const ANIM_STAND: u16 = 0;

/// Scan radius around the landing spot, in yards.
const SCAN_RANGE: f32 = 25.0;
/// Frames each window samples, enough to outlast a transient.
const SAMPLE_FRAMES: usize = 60;
const SETTLE_SECS: f64 = 5.0;
const SCAN_TIMEOUT_SECS: f64 = 25.0;
const LOOT_TIMEOUT_SECS: f64 = 15.0;
const CLOSE_TIMEOUT_SECS: f64 = 10.0;

/// The streamed-object columns the scan and the lock resolve read.
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
    chest: Option<u64>,
    /// The base anim ids seen in each window, in frame order.
    before: Vec<Option<u16>>,
    open: Vec<Option<u16>>,
    after: Vec<Option<u16>>,
    /// The latch (a loot session is open) and the kneel (this kind of target is knelt at), read
    /// when the open window ends; a fishing bobber is `latch=Some(..) kneel=false`.
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
    /// Sampling with the chest shut, the control window.
    Before,
    /// The use packet is out; waiting for the server to open a loot window.
    WaitLoot {
        since: f64,
    },
    /// Sampling with the loot window open.
    Open,
    /// `CloseLoot()` called; waiting for the window to go.
    WaitClosed {
        since: f64,
    },
    /// Sampling after the close.
    After,
    Done,
}

/// `WOW_PROBE_CHEST=<x>,<y>,<z>[,<map>]`, else [`CHEST_AT`] (the common value is `1`).
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

/// Whether `LootFrame` is shown (after `LOOT_OPENED`), else the app-side loot session.
fn window_open(script: &UiScript, loot: &LootState) -> bool {
    script
        .eval::<bool>("return LootFrame:IsShown() and 1 or nil")
        .unwrap_or_else(|_| loot.source().is_some())
}

/// One window's verdict line; `expect` is the id every frame of the window must carry.
fn report(label: &str, expect: u16, seen: &[Option<u16>]) -> u32 {
    let held = seen.iter().filter(|a| **a == Some(expect)).count();
    // What it held instead, in first-seen order.
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
        return; // no UI VM in this build
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
            // Send exactly what the right-click's lock chain would send.
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
            // `AppExit` plus a hard backstop, so a teardown hang cannot keep the account held.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_CHEST: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
