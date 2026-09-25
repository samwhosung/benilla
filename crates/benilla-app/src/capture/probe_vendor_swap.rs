//! The vendor-swap live probe (`WOW_PROBE_VENDOR_SWAP=1`): does a second vendor, opened over the
//! first's window, swap the whole window (stock `MerchantFrame.lua:67-68`, the `UnitName("NPC")`
//! title and the `"NPC"` portrait) on the dispatch that raises `MERCHANT_SHOW`, and does closing
//! the window clear the `"npc"` token? It stands in the Goldshire inn between Brog Hamfist (151)
//! and Barkeep Dobbins (465), 5.45 yd apart, both within service reach.
//!
//! Legs, one `PROBE_VENDOR_SWAP: <leg> PASS/FAIL/SKIP` line each:
//! 1. open: `CMSG_LIST_INVENTORY` to Brog; at `MERCHANT_SHOW`, `UnitName("npc")` and the title
//!    both read his name.
//! 2. portrait: the round region is sampled under the `"npc"` key (`portrait::BoothPanes`) and the
//!    booth publishes a live bake for it (`PortraitImages`).
//! 3. swap: with Brog's window open, `CMSG_LIST_INVENTORY` to Dobbins; at that `MERCHANT_SHOW`,
//!    both read Dobbins' name.
//! 4. title: a second later the window still shows his name.
//! 5. clear: `HideUIPanel(MerchantFrame)` (stock OnHide calls `CloseMerchant`); once the session
//!    is gone `UnitExists("npc")` is nil.
//!
//! The event readings are taken inside the event by a hook frame registered after the stock one,
//! so a later `MERCHANT_UPDATE` repaint cannot mask what the open left. Non-combat, nothing bought
//! or sold. The switches are `docs/CONTRIBUTING.md`, "Running it unattended".

use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::names::NameCache;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::portrait::{BoothPanes, PortraitImages, PortraitSource};
use crate::ui_merchant::MerchantOpen;

/// Brog Hamfist `<General Supplies>`: `creature_template.entry = 151`, `npc_flags = 4`.
const BROG_ENTRY: u32 = 151;
/// Barkeep Dobbins: `entry = 465`, `npc_flags = 4`.
const DOBBINS_ENTRY: u32 = 465;
/// Midway between the two spawns (Brog -9465.29, 9.63; Dobbins -9459.98, 8.41; both z 57.15),
/// ~2.8 yd from each, inside the 5.5556 yd service reach.
const STAND_AT: [f32; 3] = [-9462.6, 9.0, 57.15];
const MAP: u32 = 0;

const SETTLE_SECS: f64 = 3.0;
/// How long to wait for both vendors and their names before a SKIP.
const SCAN_TIMEOUT_SECS: f64 = 20.0;
const WINDOW_TIMEOUT_SECS: f64 = 15.0;
const PORTRAIT_TIMEOUT_SECS: f64 = 10.0;
/// How long after the swap to read the title the window is left showing.
const SETTLED_SECS: f64 = 1.0;
const SCAN_RANGE_SQ: f32 = crate::target::SERVICE_RANGE_SQ;

/// The hook frame: one line per merchant event, read inside the dispatch. Registered after the
/// stock `MerchantFrame`, so `MerchantNameText` shows what the stock `OnEvent` left there.
const HOOK: &str = r#"
    if not ProbeVendorSwapHooked then
        ProbeVendorSwapHooked = true
        ProbeVendorSwapLog = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("MERCHANT_SHOW")
        f:RegisterEvent("MERCHANT_UPDATE")
        f:RegisterEvent("MERCHANT_CLOSED")
        f:SetScript("OnEvent", function()
            table.insert(ProbeVendorSwapLog,
                event .. "|" .. tostring(UnitName("npc")) .. "|" .. tostring(MerchantNameText:GetText()))
        end)
    end
"#;

pub(crate) struct ProbeVendorSwapPlugin;

impl Plugin for ProbeVendorSwapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VendorSwapProbe>()
            .add_systems(Update, vendor_swap_probe);
    }
}

#[derive(Resource, Default)]
struct VendorSwapProbe {
    phase: Phase,
    passes: u32,
    fails: u32,
    skips: u32,
    brog: Option<u64>,
    dobbins: Option<u64>,
    exited: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Start,
    /// `.go` sent; waiting for both vendors to stream in reach, with their names cached.
    Settle {
        sent_at: f64,
    },
    /// Brog's `CMSG_LIST_INVENTORY` sent; waiting for his window and its `MERCHANT_SHOW`.
    Open {
        since: f64,
    },
    /// Brog's window is open; waiting for the round portrait to be sampled and baked.
    Portrait {
        since: f64,
    },
    /// Dobbins' `CMSG_LIST_INVENTORY` sent over Brog's open window; waiting for the swap.
    Swap {
        since: f64,
    },
    /// The swap happened; reading the title the window settles on.
    Settled {
        since: f64,
    },
    /// `HideUIPanel(MerchantFrame)` run; waiting for the session to clear, then one more poll.
    Clear {
        since: f64,
        closed_at: Option<f64>,
    },
    Done,
}

/// The first `MERCHANT_SHOW` line in the hook's log, split into `(UnitName("npc"), title)`.
fn first_show(script: &UiScript) -> Option<(String, String)> {
    let log: Vec<String> = script.eval("return ProbeVendorSwapLog").unwrap_or_default();
    log.iter().find_map(|line| {
        let mut parts = line.splitn(3, '|');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("MERCHANT_SHOW"), Some(npc), Some(title)) => {
                Some((npc.to_string(), title.to_string()))
            }
            _ => None,
        }
    })
}

fn clear_log(script: &UiScript) {
    if let Err(e) = script.run("ProbeVendorSwapLog = {}") {
        error!("PROBE_VENDOR_SWAP: clearing the hook log: {e}");
    }
}

fn vendor_swap_probe(
    time: ProbeClock,
    mut probe: ResMut<VendorSwapProbe>,
    merchant: Res<MerchantOpen>,
    script: Option<NonSendMut<UiScript>>,
    names: Res<NameCache>,
    panes: Res<BoothPanes>,
    images: Res<PortraitImages>,
    self_player: Query<(), With<SelfPlayer>>,
    player: Res<Player>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    if self_player.is_empty() {
        return; // not in-world yet
    }
    let now = time.elapsed_secs_f64();
    let Some(script) = script else {
        return;
    };

    match probe.phase {
        Phase::Start => {
            if let Err(e) = script.run(HOOK) {
                error!("PROBE_VENDOR_SWAP: installing the merchant hook: {e}");
                probe.phase = Phase::Done;
                return;
            }
            let [x, y, z] = STAND_AT;
            info!("PROBE_VENDOR_SWAP: hopping to the Goldshire inn ({x}, {y}, {z}) map {MAP}");
            let _ = net.0.send(ClientCommand::Chat {
                kind: ChatKind::Say,
                target: None,
                text: format!(".go xyz {x} {y} {z} {MAP}"),
            });
            probe.phase = Phase::Settle { sent_at: now };
        }
        Phase::Settle { sent_at } => {
            if now - sent_at < SETTLE_SECS {
                return;
            }
            let me = player.pos;
            let find = |entry: u32| {
                units
                    .iter()
                    .find(|(_, net_e, store, tf)| {
                        net_e.kind == EntityKind::Unit
                            && store.0.object_entry() == Some(entry)
                            && tf.translation.distance_squared(me) <= SCAN_RANGE_SQ
                    })
                    .map(|(guid, ..)| guid.0)
            };
            let (brog, dobbins) = (find(BROG_ENTRY), find(DOBBINS_ENTRY));
            // Both names must be cached before the legs read them back out of the VM.
            let named = |g: Option<u64>| g.is_some_and(|g| names.resolve(g, &net).is_some());
            let (brog_named, dobbins_named) = (named(brog), named(dobbins));
            if brog_named && dobbins_named {
                probe.brog = brog;
                probe.dobbins = dobbins;
                let brog = brog.expect("named");
                info!(
                    "PROBE_VENDOR_SWAP: Brog {brog:#x} and Dobbins {:#x} both in reach — \
                     CMSG_LIST_INVENTORY to Brog",
                    dobbins.expect("named")
                );
                clear_log(&script);
                let _ = net.0.send(ClientCommand::ListInventory { guid: brog });
                probe.phase = Phase::Open { since: now };
            } else if now - sent_at > SCAN_TIMEOUT_SECS {
                warn!(
                    "PROBE_VENDOR_SWAP: SKIP — vendors in reach: Brog {brog:?}, Dobbins \
                     {dobbins:?} (names cached: {}/{}). Environmental (the `.go` refused, or the \
                     inn never streamed).",
                    brog_named, dobbins_named
                );
                probe.skips += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Open { since } => {
            let brog = probe.brog.expect("set at Settle");
            let expect = names.peek(brog).unwrap_or("").to_string();
            match (merchant.vendor == Some(brog), first_show(&script)) {
                (true, Some((npc, title))) => {
                    if npc == expect && title == expect {
                        info!(
                            "PROBE_VENDOR_SWAP: open PASS — at MERCHANT_SHOW UnitName(\"npc\") \
                             = {npc:?}, title = {title:?}"
                        );
                        probe.passes += 1;
                    } else {
                        error!(
                            "PROBE_VENDOR_SWAP: open FAIL — at MERCHANT_SHOW UnitName(\"npc\") \
                             = {npc:?}, title = {title:?}; expected {expect:?} for both"
                        );
                        probe.fails += 1;
                    }
                    probe.phase = Phase::Portrait { since: now };
                }
                _ if now - since > WINDOW_TIMEOUT_SECS => {
                    error!(
                        "PROBE_VENDOR_SWAP: open FAIL — no merchant window + MERCHANT_SHOW on \
                         Brog within {WINDOW_TIMEOUT_SECS}s (vendor = {:?})",
                        merchant.vendor
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                }
                _ => {}
            }
        }
        Phase::Portrait { since } => {
            let sampled = panes.0.contains_key("npc");
            let baked = matches!(images.0.get("npc"), Some(PortraitSource::Live(_)));
            if sampled && baked {
                info!(
                    "PROBE_VENDOR_SWAP: portrait PASS — the round region samples the \"npc\" \
                     slot and the booth publishes a live bake for it"
                );
                probe.passes += 1;
            } else if now - since > PORTRAIT_TIMEOUT_SECS {
                let mut keys: Vec<&String> = panes.0.keys().collect();
                keys.sort();
                error!(
                    "PROBE_VENDOR_SWAP: portrait FAIL — sampled under \"npc\": {sampled}, live \
                     bake: {baked}; pane keys published this frame: {keys:?}"
                );
                probe.fails += 1;
            } else {
                return;
            }
            let dobbins = probe.dobbins.expect("set at Settle");
            info!("PROBE_VENDOR_SWAP: CMSG_LIST_INVENTORY to Dobbins over Brog's open window");
            clear_log(&script);
            let _ = net.0.send(ClientCommand::ListInventory { guid: dobbins });
            probe.phase = Phase::Swap { since: now };
        }
        Phase::Swap { since } => {
            let dobbins = probe.dobbins.expect("set at Settle");
            let expect = names.peek(dobbins).unwrap_or("").to_string();
            match (merchant.vendor == Some(dobbins), first_show(&script)) {
                (true, Some((npc, title))) => {
                    if npc == expect && title == expect {
                        info!(
                            "PROBE_VENDOR_SWAP: swap PASS — at MERCHANT_SHOW UnitName(\"npc\") \
                             = {npc:?}, title = {title:?}"
                        );
                        probe.passes += 1;
                    } else {
                        error!(
                            "PROBE_VENDOR_SWAP: swap FAIL — at MERCHANT_SHOW UnitName(\"npc\") \
                             = {npc:?}, title = {title:?}; expected {expect:?} for both"
                        );
                        probe.fails += 1;
                    }
                    probe.phase = Phase::Settled { since: now };
                }
                _ if now - since > WINDOW_TIMEOUT_SECS => {
                    error!(
                        "PROBE_VENDOR_SWAP: swap FAIL — no window swap + MERCHANT_SHOW on \
                         Dobbins within {WINDOW_TIMEOUT_SECS}s (vendor = {:?})",
                        merchant.vendor
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                }
                _ => {}
            }
        }
        Phase::Settled { since } => {
            if now - since < SETTLED_SECS {
                return;
            }
            let dobbins = probe.dobbins.expect("set at Settle");
            let expect = names.peek(dobbins).unwrap_or("").to_string();
            let title: String = script
                .eval("return tostring(MerchantNameText:GetText())")
                .unwrap_or_default();
            if title == expect {
                info!("PROBE_VENDOR_SWAP: title PASS — the window settled on {title:?}");
                probe.passes += 1;
            } else {
                error!(
                    "PROBE_VENDOR_SWAP: title FAIL — the window settled on {title:?}, expected \
                     {expect:?}"
                );
                probe.fails += 1;
            }
            if let Err(e) = script.run("HideUIPanel(MerchantFrame)") {
                error!("PROBE_VENDOR_SWAP: HideUIPanel(MerchantFrame): {e}");
            }
            probe.phase = Phase::Clear {
                since: now,
                closed_at: None,
            };
        }
        Phase::Clear { since, closed_at } => {
            match closed_at {
                None if merchant.vendor.is_none() => {
                    probe.phase = Phase::Clear {
                        since,
                        closed_at: Some(now),
                    };
                }
                None if now - since > WINDOW_TIMEOUT_SECS => {
                    error!(
                        "PROBE_VENDOR_SWAP: clear FAIL — the session never cleared after \
                         HideUIPanel (vendor = {:?})",
                        merchant.vendor
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                }
                None => {}
                // The unit feed drops the token the frame after the session clears.
                Some(at) if now - at >= 0.25 => {
                    // `and 1 or 0` turns `UnitExists`'s `1` or nil into a number.
                    let exists: i64 = script
                        .eval(r#"return UnitExists("npc") and 1 or 0"#)
                        .unwrap_or(1);
                    if exists == 1 {
                        error!(
                            "PROBE_VENDOR_SWAP: clear FAIL — UnitExists(\"npc\") is still true \
                             after the window closed"
                        );
                        probe.fails += 1;
                    } else {
                        info!("PROBE_VENDOR_SWAP: clear PASS — UnitExists(\"npc\") is nil");
                        probe.passes += 1;
                    }
                    probe.phase = Phase::Done;
                }
                Some(_) => {}
            }
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            info!(
                "PROBE_VENDOR_SWAP: DONE pass={} fail={} skip={}",
                probe.passes, probe.fails, probe.skips
            );
            // `AppExit` plus a hard backstop, so a teardown hang cannot keep the account held.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_VENDOR_SWAP: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
