//! The openable-item live probe (`WOW_PROBE_CLAM=1`) — **the clam instrument**: right-click a
//! LOOTABLE item in the bags and does a loot window actually open?
//!
//! The director, 2026-08-22: *"Clams don't open anymore. I right click them and they turn gray
//! correctly but no loot window opens ever."* Both halves of that sentence are readings this probe
//! takes separately, because they come from two different pre-send writes in the same emitter
//! (`0x5edc80`):
//!
//! - the **grey lock** ([`PendingItemOps`], `0x4953e0` at `0x5edcd9`) — the half that kept working;
//! - the **loot latch** ([`LootLatch`], `0x5edcc0`) — the half decision 1477 left unmodelled, on
//!   the reading that an item arm changes no pose. It changes no pose and it is still load-bearing:
//!   vmangos answers `CMSG_OPEN_ITEM` with `SendLoot(item guid, LOOT_CORPSE)`, so
//!   `SMSG_LOOT_RESPONSE` comes back **type 1** on the item's own guid, and 1477's admission gate
//!   *refuses* a type-1 answer against a cold latch — bounces a `CMSG_LOOT_RELEASE` and opens
//!   nothing. Grey clam, no window, forever.
//!
//! **The window is a number here, not a picture.** The probe drives the click through the live UI
//! VM's own `UseContainerItem(0, slot)` — the same binding the bag button calls, so the whole
//! dispatcher (`ui_items::drain::drain_container_uses`) runs for real — and then reads
//! `LootFrame:IsShown()` plus the app-side session guid. Four readings, in order, which is what
//! makes it a regression test rather than a one-sided assertion:
//!
//! 1. **BEFORE** — the clam sitting in the bag, unclicked: no loot window. The control; a probe
//!    that only ever sees a window cannot tell a fix from a window that was already up.
//! 2. **OPEN** — after the click: a window, whose session guid is the **clam's own item guid**
//!    (not a corpse's, not zero), with the grey lock on its slot.
//! 3. **CLOSED** — `CloseLoot()` through the live VM: the window goes and the latch clears.
//! 4. **REOPEN** — the same clam clicked again: it opens a second time. The adjacent state that
//!    catches a latch which arms once and never re-arms (the classic "works the first time" shape).
//!
//! ## The run recipe
//!
//! ```text
//! WOW_NOSOUND=1 WOW_USER=probe0 WOW_PASS=pprobe0 WOW_CHAR=Probezero \
//!     WOW_PROBE_CLAM=1 cargo run -q -p benilla
//! ```
//! (the checkout's probe identity — `.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR; the `probe`
//! skill). `WOW_PROBE_CLAM=<entry>` aims it at a different openable template;
//! the default is a Small Barnacled Clam (7973 — `Flags` LOOTABLE with `LockID = 0`, so it is
//! openable the moment it exists: no key, no lockpicking). The probe `.additem`s its own copy and
//! subtracts it again on the way out, so it leaves the character as it found it.
//!
//! Auto-loot is forced **off** for the run: with it on, the clam's single row is taken at the open
//! edge and the last-row auto-release closes the window within a frame or two, which would read as
//! "no window" for the wrong reason.
//!
//! Grep `PROBE_CLAM:` for the verdict; the probe self-exits when it lands.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::items::Items;
use crate::net::{ChatKind, ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::pending_item_ops::PendingItemOps;
use crate::ui_loot::{LootConfig, LootLatch, LootState};

/// The probe's default template: "Small Barnacled Clam" (entry 7973) — the director's own case, and
/// the `benilla-world --open-item` wire probe's. Its `item_loot_template` row (a Zesty Clam Meat)
/// is what the window must show.
const CLAM_ENTRY: u32 = 7973;

/// How long to wait for `.additem` to land the item and its template in the bags.
const STOCK_TIMEOUT_SECS: f64 = 20.0;
/// How long a click gets to produce a window before the run is called a failure.
const OPEN_TIMEOUT_SECS: f64 = 10.0;
/// How long `CloseLoot()` gets to take the window down.
const CLOSE_TIMEOUT_SECS: f64 = 10.0;
/// How long the `.additem -1` gets to take the probe's own copy back out of the bags.
const CLEANUP_TIMEOUT_SECS: f64 = 8.0;
/// How long the REOPEN round waits after the window goes down before clicking again.
///
/// **Not padding — a real race the reference shares.** `CloseLoot()` sends `CMSG_LOOT_RELEASE` and
/// clears the latch locally at the *send* (`0x48f2c9` in `CloseInteraction 0x48f200`); the server's
/// `SMSG_LOOT_RELEASE_RESPONSE` clears it again, **guid-matched** (`0x5ec0d4`). Re-open the *same*
/// object inside that round trip and the response's clear lands on the latch the re-open just
/// armed, so its type-1 answer is refused — guid-matching cannot separate them when both guids are
/// the same object. Both clear sites are the reference's own, so this is faithful rather than ours
/// to fix; a hand re-click never gets near the ~40 ms window, and a probe clicking on the next
/// frame does, every few runs. One second is a human's reaction time, generously.
const REOPEN_SETTLE_SECS: f64 = 1.0;
/// Frames the BEFORE control samples — long enough to outlast a transient, short enough to be free.
const CONTROL_FRAMES: usize = 60;

pub(crate) struct ProbeClamPlugin;

impl Plugin for ProbeClamPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClamProbe>()
            .add_systems(Update, clam_probe);
    }
}

/// Which template the probe opens: `WOW_PROBE_CLAM=<entry>`, else [`CLAM_ENTRY`]. Anything
/// unparseable falls back to the default rather than failing the run — the common value is `1`.
fn target_entry() -> u32 {
    std::env::var("WOW_PROBE_CLAM")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|e| *e != 0 && *e != 1)
        .unwrap_or(CLAM_ENTRY)
}

#[derive(Resource, Default)]
struct ClamProbe {
    phase: Phase,
    /// The clam's item guid and its 1-based Lua backpack slot, once the stock scan finds it.
    clam: Option<u64>,
    slot: u32,
    /// Frames sampled with the clam untouched — every one must show no window.
    control: Vec<bool>,
    /// What each open round read: the session guid the window came up on, and whether the clicked
    /// slot was grey at the same moment. Reported separately because the director's report split
    /// exactly there — grey yes, window no.
    opened: Vec<Option<u64>>,
    greyed: Vec<bool>,
    /// Whether the latch was clear after each `CloseLoot()`.
    cleared: Vec<bool>,
    fails: u32,
    exited: bool,
}

/// The open rounds the probe runs: the first click, and the re-click that catches an arm which
/// fires once and never again.
const ROUNDS: u8 = 2;

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// `.additem` issued; waiting for the item AND its template to reach the bags.
    Stocking {
        sent_at: f64,
    },
    /// Sampling with the clam untouched — the control window.
    Before,
    /// `UseContainerItem` queued; waiting for a loot window. `round` counts from 0.
    WaitOpen {
        since: f64,
        round: u8,
    },
    /// `CloseLoot()` called; waiting for the window to actually go.
    WaitClosed {
        since: f64,
        round: u8,
    },
    /// The window is down; letting the release round trip finish before the re-click. See
    /// [`REOPEN_SETTLE_SECS`].
    Settling {
        since: f64,
        round: u8,
    },
    /// `.additem <entry> -1` sent; waiting for the copy to actually leave the bags before exiting.
    Cleanup {
        since: f64,
    },
    Done,
}

/// Send the probe's own copy back and enter [`Phase::Cleanup`] — the one way out of the round loop,
/// so no exit path can skip the cleanup.
fn finish(net: &NetCommands, entry: u32, now: f64) -> Phase {
    let _ = net.0.send(ClientCommand::Chat {
        kind: ChatKind::Say,
        target: None,
        text: format!(".additem {entry} -1"),
    });
    Phase::Cleanup { since: now }
}

/// Whether the loot window is up, asked of the **live UI** rather than of our own state: the window
/// is what the director looked for, and `LootFrame` shows only once the feed has fired
/// `LOOT_OPENED` off the wire. Falls back to the app-side session when the frame can't be read.
fn window_open(script: &UiScript, loot: &LootState) -> bool {
    script
        .eval::<bool>("return LootFrame:IsShown() and 1 or nil")
        .unwrap_or_else(|_| loot.source().is_some())
}

/// Find the probe's own copy of `entry` in the backpack: its item guid and 1-based Lua slot. The
/// template has to have landed too — the click dispatcher's open arm is a **template** LOOTABLE
/// test, so a click made before the answer arrives falls through to a plain use and proves nothing.
fn find_in_backpack(
    store: &ObjectStore,
    entry: u32,
    objects: &Objects,
    items: &Items,
    net: &NetCommands,
) -> Option<(u64, u32)> {
    let (i, guid) = (0..16u8)
        .filter_map(|i| {
            store
                .0
                .player_pack_slot(i)
                .filter(|g| *g != 0)
                .map(|g| (i, g))
        })
        .find(|(_, guid)| objects.object(*guid).and_then(|f| f.object_entry()) == Some(entry))?;
    // `template` asks the server once on a miss and answers `None` until the reply lands, which is
    // exactly the "not stocked yet" state this returns.
    items
        .template(entry, guid, net)
        .filter(|t| t.opens_loot())
        .map(|_| (guid, u32::from(i) + 1))
}

fn clam_probe(
    time: ProbeClock,
    mut probe: ResMut<ClamProbe>,
    script: Option<NonSendMut<UiScript>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    mut cfg: ResMut<LootConfig>,
    loot: Res<LootState>,
    latch: Res<LootLatch>,
    pending: Res<PendingItemOps>,
    net: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(store) = self_store.single() else {
        return; // not in-world yet
    };
    let Some(mut script) = script else {
        return; // no UI VM this build — nothing this probe can drive
    };
    let now = time.elapsed_secs_f64();
    let entry = target_entry();

    match probe.phase {
        Phase::Wait => {
            // The window must survive to be looked at: with auto-loot on, the single row is taken
            // at the open edge and the last-row auto-release shuts it again immediately.
            cfg.auto_loot = false;
            info!("PROBE_CLAM: stocking one openable item (entry {entry}) — auto-loot forced off");
            let _ = net.0.send(ClientCommand::Chat {
                kind: ChatKind::Say,
                target: None,
                text: format!(".additem {entry}"),
            });
            probe.phase = Phase::Stocking { sent_at: now };
        }
        Phase::Stocking { sent_at } => {
            if let Some((guid, slot)) = find_in_backpack(store, entry, &objects, &items, &net) {
                info!(
                    "PROBE_CLAM: item {entry} is guid {guid:#x} in backpack slot {slot} — \
                     sampling {CONTROL_FRAMES} control frames with it UNCLICKED"
                );
                probe.clam = Some(guid);
                probe.slot = slot;
                probe.phase = Phase::Before;
            } else if now - sent_at > STOCK_TIMEOUT_SECS {
                error!(
                    "PROBE_CLAM: FAIL — no LOOTABLE item {entry} with a landed template in the \
                     backpack within {STOCK_TIMEOUT_SECS}s. `.additem` needs a GM account, and it \
                     refuses silently when the backpack is full. This is NOT a passing run"
                );
                probe.fails += 1;
                probe.phase = Phase::Done; // nothing was stocked — nothing to clean up
            }
        }
        Phase::Before => {
            probe.control.push(window_open(&script, &loot));
            if probe.control.len() < CONTROL_FRAMES {
                return;
            }
            click(&mut script, &mut probe, 0);
            probe.phase = Phase::WaitOpen {
                since: now,
                round: 0,
            };
        }
        Phase::WaitOpen { since, round } => {
            if window_open(&script, &loot) {
                let slot = probe.slot;
                let source = loot.source();
                let grey = pending.contains(0, slot);
                info!(
                    "PROBE_CLAM: round {round} — window up on {:?} (latch {:?}, slot grey {grey})",
                    source.map(|g| format!("{g:#x}")),
                    latch.0.map(|g| format!("{g:#x}")),
                );
                probe.opened.push(source);
                probe.greyed.push(grey);
                let _ = script.eval::<()>("CloseLoot()");
                probe.phase = Phase::WaitClosed { since: now, round };
            } else if now - since > OPEN_TIMEOUT_SECS {
                let grey = pending.contains(0, probe.slot);
                error!(
                    "PROBE_CLAM: round {round} FAIL — no loot window within {OPEN_TIMEOUT_SECS}s \
                     of the click (latch {:?}, slot grey {grey}). A grey slot with no window is \
                     the refused type-1 response: the CMSG_OPEN_ITEM arm never latched the item \
                     guid",
                    latch.0.map(|g| format!("{g:#x}")),
                );
                probe.fails += 1;
                probe.opened.push(None);
                probe.greyed.push(grey);
                probe.phase = finish(&net, entry, now);
            }
        }
        Phase::Settling { since, round } => {
            if now - since >= REOPEN_SETTLE_SECS {
                info!("PROBE_CLAM: re-clicking the same item (round {round})");
                click(&mut script, &mut probe, round);
                probe.phase = Phase::WaitOpen { since: now, round };
            }
        }
        Phase::WaitClosed { since, round } => {
            if !window_open(&script, &loot) {
                probe.cleared.push(latch.0.is_none());
                let next = round + 1;
                if next < ROUNDS {
                    probe.phase = Phase::Settling {
                        since: now,
                        round: next,
                    };
                } else {
                    probe.phase = finish(&net, entry, now);
                }
            } else if now - since > CLOSE_TIMEOUT_SECS {
                error!(
                    "PROBE_CLAM: FAIL — the loot window never closed within {CLOSE_TIMEOUT_SECS}s"
                );
                probe.fails += 1;
                probe.phase = finish(&net, entry, now);
            }
        }
        Phase::Cleanup { since } => {
            // Put the character back as we found it, and **watch it land**: `AppExit` tears the net
            // thread down within a frame or two, so a fire-and-forget `.additem -1` written on the
            // way out never reaches the wire (the first run of this probe left its clam behind
            // exactly that way). Leave when the copy is gone, or say plainly that it isn't.
            let gone = probe
                .clam
                .is_some_and(|guid| (0..16u8).all(|i| store.0.player_pack_slot(i) != Some(guid)));
            if gone {
                probe.phase = Phase::Done;
            } else if now - since > CLEANUP_TIMEOUT_SECS {
                warn!(
                    "PROBE_CLAM: the probe's own copy of {entry} is still in the bags after \
                     {CLEANUP_TIMEOUT_SECS}s — remove it by hand (`.additem {entry} -1`)"
                );
                probe.phase = Phase::Done;
            }
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            report(&probe, entry);
            // The probe self-exit pattern (`ProbeExitPlugin::fire_probe_exit`): a polite AppExit
            // plus a hard backstop thread, so a net/winit teardown hang can't leave a zombie
            // client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_CLAM: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}

/// Right-click the clam the way the bag button does — through the live VM's own
/// `UseContainerItem`, so `ui_items::drain::drain_container_uses` runs its real dispatcher (and its
/// real open arm) rather than the probe re-deciding which packet an openable item takes.
fn click(script: &mut UiScript, probe: &mut ClamProbe, round: u8) {
    let slot = probe.slot;
    info!("PROBE_CLAM: round {round} — UseContainerItem(0, {slot}) through the live VM");
    if let Err(e) = script.run(&format!("UseContainerItem(0, {slot})")) {
        error!("PROBE_CLAM: FAIL — the click chunk did not run: {e}");
        probe.fails += 1;
    }
}

/// The verdict, one line per reading, then the DONE tally.
fn report(probe: &ClamProbe, entry: u32) {
    let mut fails = probe.fails;
    let clam = probe.clam.unwrap_or_default();

    let stray = probe.control.iter().filter(|open| **open).count();
    if stray == 0 && !probe.control.is_empty() {
        info!(
            "PROBE_CLAM: BEFORE PASS — no loot window on any of {} control frames",
            probe.control.len()
        );
    } else {
        error!(
            "PROBE_CLAM: BEFORE FAIL — a loot window was already up on {stray}/{} control frames; \
             the open reading below means nothing",
            probe.control.len()
        );
        fails += 1;
    }

    for round in 0..usize::from(ROUNDS) {
        let label = if round == 0 { "OPEN" } else { "REOPEN" };
        match probe.opened.get(round) {
            Some(Some(source)) if *source == clam => info!(
                "PROBE_CLAM: {label:<6} PASS — the window's session is the item's own guid \
                 {source:#x} (slot grey {})",
                probe.greyed.get(round).copied().unwrap_or(false)
            ),
            Some(Some(source)) => {
                error!(
                    "PROBE_CLAM: {label:<6} FAIL — a window opened, but on {source:#x}, not the \
                     clam {clam:#x}"
                );
                fails += 1;
            }
            _ => {
                error!("PROBE_CLAM: {label:<6} FAIL — no window on this click");
                fails += 1;
            }
        }
        match probe.cleared.get(round) {
            Some(true) => {
                info!("PROBE_CLAM: CLOSE{round}  PASS — the latch cleared with the window")
            }
            Some(false) => {
                error!(
                    "PROBE_CLAM: CLOSE{round}  FAIL — the window went but the latch is still armed"
                );
                fails += 1;
            }
            None => {}
        }
    }

    info!("PROBE_CLAM: DONE entry={entry} item={clam:#x} fail={fails}");
}
