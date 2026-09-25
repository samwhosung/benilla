//! The bank-arc live probe (`WOW_PROBE_BANK=1`) — decision 0604's end-to-end instrument: log in,
//! GM-hop to a PURE banker, drive the whole six-opcode bank wire
//! (`CMSG_BANKER_ACTIVATE`→`SMSG_SHOW_BANK`, deposit/withdraw via `CMSG_AUTOBANK_ITEM`/
//! `CMSG_AUTOSTORE_BANK_ITEM`, `CMSG_BUY_BANK_SLOT` and its refusal), and print a
//! `PROBE_BANK: <step> PASS/FAIL/SKIP <detail>` line per step plus a final
//! `PROBE_BANK: DONE pass=<n> fail=<m>` summary. Modeled closely on [`super::probe_mail`] (same
//! phase-machine shape, trace style, self-terminating exit) — but unlike mail, the bank steps ride
//! [`ClientCommand`]/the descriptor directly (the vault is already streamed, the
//! window is first-party, no Lua click surface to drive); the live Lua VM is only touched for the
//! bonus refusal step's `UI_ERROR_MESSAGE` observation, the mail probe's own idiom.
//!
//! ## The banker (live-DB verified against the local vmangos, `characters`/
//! `mangos` DBs)
//!
//! Soleil Stonemantle, creature entry 5099, spawn guid 12629, map 0 (Ironforge, The Vault), pos
//! `(-4895.64, -1004.66, 504.024)`. Her `creature_template.npc_flags` is **256** exactly — bit 8
//! only (`UNIT_NPC_FLAG_BANKER`), no gossip bits — so she is a *pure* banker: the direct
//! `CMSG_BANKER_ACTIVATE` route applies with no gossip pre-empt (decision 0604's interact-routing
//! note).
//!
//! ## The funding note — `.modify money` is NOT within a probe account's reach (verified, corrects
//! the task brief)
//!
//! vmangos `Chat.cpp`'s `modifyCommandTable` pins `.modify money` at `SEC_BASIC_ADMIN` (4,
//! `Common.h` `AccountTypes`) — one level ABOVE `SEC_GAMEMASTER` (3), the level every probe account
//! is provisioned at (docs/METHOD.md). So `.modify money` is refused server-side for a probe login,
//! exactly the same floor [`super::probe_taxi`]'s module doc already found for the taxi fare. This
//! probe never sends it: step (e) reads the character's live `money` field instead (DB-verified
//! this session: `Probeone` carries 100000 copper, `bank_bag_slots` 0 — comfortably funds the
//! first purchase-ladder rung, 1000 copper) and SKIPs the purchase gracefully if the live balance
//! ever falls short of the next rung's price, rather than sending a command known to be refused.
//!
//! ## The run recipe
//!
//! ```text
//! WOW_DATA=WoW/Data WOW_USER=probe1 WOW_PASS=pprobe1 WOW_CHAR=Probeone \
//!     WOW_PROBE_BANK=1 cargo run -q -p benilla
//! ```
//! (the checkout's probe identity — `.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR; the `probe`
//! skill).
//! Non-combat; GM mode is left exactly as found. An outer `timeout` + grep on `PROBE_BANK:` is the
//! whole harness; the probe self-exits ([`super::probes::ProbeExitPlugin`]'s pattern) once DONE.

use bevy::prelude::*;

use benilla_protocol::messages::{BAG_PLAYER_INVENTORY, SLOT_PACK_FIRST};
use benilla_protocol::EntityKind;
use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_bank::{BankOpen, BankPrices};

/// Soleil Stonemantle's spawn (vmangos `creature` guid 12629, entry 5099, map 0) — live-DB
/// verified this session (module doc).
const BANKER_AT: [f32; 3] = [-4895.64, -1004.66, 504.024];
/// Her creature template entry — the streamed-unit identity check.
const BANKER_ENTRY: u32 = 5099;
/// `UNIT_NPC_FLAG_BANKER` (bit 8) — the fallback identity check (either suffices).
const NPC_FLAG_BANKER: u32 = 0x100;
/// The server's `CheckBanker`/`GetNPCIfCanInteractWith` range is a few yards; scan generously wide
/// so a slightly-off `.go` landing still finds her (the mail probe's `MAILBOX_SCAN_RANGE` idiom).
const SCAN_RANGE: f32 = 12.0;
/// The probe's deposit/withdraw fixture: Linen Cloth (cheap, stackable, no durability/equip
/// complications).
const ITEM_ENTRY: u32 = 2589;
/// The first player-array bank slot (`PLAYER_FIELD_BANK_SLOT_1`'s wire index — `SLOT_PACK_FIRST`
/// (23) + the backpack's 16 slots, decision 0604's addressing note: bank slots are wire 39-62).
const SLOT_BANK_FIRST: u8 = SLOT_PACK_FIRST + 16;
/// The client-side purchase ladder (`BankBagSlotPrices.dbc`) — the fallback used
/// only if [`BankPrices`] failed to load; index = slots already purchased (0-based).
const PRICE_LADDER: [u32; 6] = [1_000, 10_000, 100_000, 250_000, 500_000, 1_000_000];
/// Slack added on top of the exact shortfall when the buy-slot step funds itself — the purse can
/// move under it (a repair, a vendor sale) between the read and the purchase.
const FUND_MARGIN_COPPER: u32 = 10_000;
/// The purchasable-slot ceiling (`GetNumBankSlots()` reports full at 6).
const MAX_BANK_BAGS: u8 = 6;
/// How far past the banker's service range the refusal step teleports (yd, WoW space) — well
/// past the handful of yards `GetNPCIfCanInteractWith` checks.
const REFUSAL_OFFSET_X: f32 = 150.0;

const SETTLE_SECS: f64 = 3.0;
const SCAN_TIMEOUT_SECS: f64 = 15.0;
const ACTIVATE_TIMEOUT_SECS: f64 = 10.0;
const ITEM_TIMEOUT_SECS: f64 = 10.0;
const ACTION_TIMEOUT_SECS: f64 = 8.0;
/// The bonus refusal step's event wait — generous but bounded; a miss SKIPs, never FAILs (task
/// brief: "if flaky, SKIP with a note rather than FAIL").
const REFUSAL_GRACE_SECS: f64 = 6.0;

pub(crate) struct ProbeBankPlugin;

impl Plugin for ProbeBankPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BankProbe>()
            .add_systems(Update, bank_probe);
    }
}

/// The probe's phase machine + the identities discovered along the way (the mail probe's shape:
/// `Copy` phase, resource-level bulk state so an arm can freely mutate `probe` without fighting
/// the borrow checker over `probe.phase`).
#[derive(Resource, Default)]
struct BankProbe {
    phase: Phase,
    /// The banker's guid, once streamed in.
    banker: Option<u64>,
    /// The item round-tripped through deposit/withdraw (same guid both ways — an item's object
    /// identity survives a bag/slot move server-side).
    item_guid: Option<u64>,
    /// The bank slot index (0-based) the fixture item landed in after deposit.
    bank_idx: Option<u8>,
    /// Step (e)'s baseline purchased-slot count `B`, latched once so the verdict can compare
    /// against it after the buy.
    baseline_purchased: u8,
    baseline_money: u32,
    /// The next rung's price, resolved once (DBC if loaded, else [`PRICE_LADDER`]).
    next_cost: u32,
    passes: u32,
    fails: u32,
    /// Latched once [`Phase::Done`] has fired its exit (never re-fire on a later frame).
    exited: bool,
}

/// Every field is `Copy` — the phase is snapshotted out of the resource each tick (the mail
/// probe's `let phase = probe.phase;` idiom), freeing the match arms to mutate `probe` freely.
#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// `.go` issued; settling before the world streams the banker in (step 1).
    Settling {
        sent_at: f64,
    },
    /// `BankerActivate` sent; waiting for `BankOpen::is_open()` (step 2).
    Activating {
        sent_at: f64,
    },
    /// Ensuring a backpack item exists to deposit — `.additem` if the bags are empty (step 3
    /// prep).
    EnsureItem {
        since: f64,
        sent: bool,
    },
    /// `AutoBankItem` already sent; waiting for the fixture guid to land in a bank slot (step 3).
    Deposit {
        since: f64,
    },
    /// `AutoStoreBankItem` already sent; waiting for the fixture guid back in the pack (step 4).
    Withdraw {
        since: f64,
    },
    /// `BuyBankSlot` sent (funds permitting); waiting for the purchased-count descriptor delta
    /// (step 5).
    BuySlot {
        since: f64,
        sent: bool,
        /// Whether this step has already granted itself the fare with `.modify money`. One shot:
        /// if the purse is still short after a grant, that is a real defect, not a permission wall.
        funded: bool,
    },
    /// Step 6 (bonus): teleport out of range, then `BuyBankSlot` the now-stale guid, expecting
    /// `SMSG_BUY_BANK_SLOT_RESULT` NOTBANKER. `teleported`/`bought` gate the two sends in order;
    /// `since` resets at each send so the following wait is measured from it, not from entry.
    Refusal {
        since: f64,
        teleported: bool,
        bought: bool,
        events_baseline: i64,
    },
    Done,
}

/// Read the Lua-side `ProbeBankEvents` log length (the `UI_ERROR_MESSAGE` hook, the mail probe's
/// idiom) — `0` on any eval hiccup (treated as "nothing observed yet", never a panic).
fn events_len(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return table.getn(ProbeBankEvents or {})")
        .unwrap_or(0)
}

/// Read the newest `ProbeBankEvents` entry (the just-fired `UI_ERROR_MESSAGE` text).
fn last_event(script: &UiScript) -> String {
    script
        .eval::<String>("return ProbeBankEvents[table.getn(ProbeBankEvents)] or \"\"")
        .unwrap_or_default()
}

/// The first backpack slot holding a nonzero item guid, if any.
fn find_pack_item(store: &ObjectStore) -> Option<(u8, u64)> {
    (0..16u8).find_map(|i| {
        store
            .0
            .player_pack_slot(i)
            .filter(|&g| g != 0)
            .map(|g| (i, g))
    })
}

/// The bank slot index currently holding `guid`, if any.
fn find_bank_slot(store: &ObjectStore, guid: u64) -> Option<u8> {
    (0..24u8).find(|&i| store.0.player_bank_slot(i) == Some(guid))
}

fn bank_probe(
    time: ProbeClock,
    mut probe: ResMut<BankProbe>,
    open: Res<BankOpen>,
    prices: Option<Res<BankPrices>>,
    script: Option<NonSendMut<UiScript>>,
    self_player: Query<(), With<SelfPlayer>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    if self_player.is_empty() {
        return; // not in-world yet
    }
    let Some(script) = script else {
        return; // no UI VM this build (headless net-only) — nothing this probe can drive
    };
    let now = time.elapsed_secs_f64();
    let phase = probe.phase;
    let Some(store) = self_store.iter().next() else {
        return;
    };

    match phase {
        Phase::Wait => {
            // The UI_ERROR_MESSAGE hook (step 6's observation channel — the mail probe's exact
            // pattern), installed up front so it's live well before the refusal step needs it.
            if let Err(e) = script.run(
                r#"
                if not ProbeBankHooked then
                    ProbeBankHooked = true
                    ProbeBankEvents = {}
                    local f = CreateFrame("Frame")
                    f:RegisterEvent("UI_ERROR_MESSAGE")
                    f:SetScript("OnEvent", function()
                        table.insert(ProbeBankEvents, arg1 or "")
                    end)
                end
                "#,
            ) {
                error!("PROBE_BANK: installing the UI_ERROR_MESSAGE hook: {e}");
            }
            let [x, y, z] = BANKER_AT;
            info!("PROBE_BANK: hopping to Soleil Stonemantle at ({x}, {y}, {z})");
            let _ = net.0.send(ClientCommand::Chat {
                kind: ChatKind::Say,
                target: None,
                text: format!(".go xyz {x} {y} {z} 0"),
            });
            probe.phase = Phase::Settling { sent_at: now };
        }
        Phase::Settling { sent_at } => {
            if now - sent_at < SETTLE_SECS {
                return;
            }
            let me = player.pos;
            let banker = units.iter().find(|(_, net_e, u_store, tf)| {
                net_e.kind == EntityKind::Unit
                    && (u_store.0.object_entry() == Some(BANKER_ENTRY)
                        || u_store.0.unit_npc_flags() & NPC_FLAG_BANKER != 0)
                    && tf.translation.distance(me) < SCAN_RANGE
            });
            if let Some((guid, ..)) = banker {
                info!(
                    "PROBE_BANK: PASS (1 teleport) — banker {:#x} streamed in range",
                    guid.0
                );
                probe.passes += 1;
                probe.banker = Some(guid.0);
                let _ = net.0.send(ClientCommand::BankerActivate { guid: guid.0 });
                info!(
                    "PROBE_BANK: (2 activate) BankerActivate({:#x}) sent",
                    guid.0
                );
                probe.phase = Phase::Activating { sent_at: now };
            } else if now - sent_at > SCAN_TIMEOUT_SECS {
                error!(
                    "PROBE_BANK: FAIL (1 teleport) — no entry {BANKER_ENTRY}/banker-flagged unit \
                     streamed in within {SCAN_TIMEOUT_SECS}s of the hop"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Activating { sent_at } => {
            if open.is_open() {
                info!("PROBE_BANK: PASS (2 activate) — BankOpen is_open (SMSG_SHOW_BANK landed)");
                probe.passes += 1;
                probe.phase = Phase::EnsureItem {
                    since: now,
                    sent: false,
                };
            } else if now - sent_at > ACTIVATE_TIMEOUT_SECS {
                error!(
                    "PROBE_BANK: FAIL (2 activate) — no SMSG_SHOW_BANK within \
                     {ACTIVATE_TIMEOUT_SECS}s"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::EnsureItem { since, sent } => {
            if let Some((idx, guid)) = find_pack_item(store) {
                info!(
                    "PROBE_BANK: (3 deposit) fixture item {guid:#x} at pack slot {idx} — \
                     depositing"
                );
                probe.item_guid = Some(guid);
                let wire_slot = SLOT_PACK_FIRST + idx;
                let _ = net.0.send(ClientCommand::AutoBankItem {
                    bag: BAG_PLAYER_INVENTORY,
                    slot: wire_slot,
                });
                probe.phase = Phase::Deposit { since: now };
                return;
            }
            if !sent {
                info!("PROBE_BANK: (3 deposit) bags empty — `.additem {ITEM_ENTRY} 1`");
                let _ = net.0.send(ClientCommand::Chat {
                    kind: ChatKind::Say,
                    target: None,
                    text: format!(".additem {ITEM_ENTRY} 1"),
                });
                probe.phase = Phase::EnsureItem {
                    since: now,
                    sent: true,
                };
            } else if now - since > ITEM_TIMEOUT_SECS {
                error!(
                    "PROBE_BANK: FAIL (3 deposit) — no pack item within {ITEM_TIMEOUT_SECS}s of \
                     `.additem`"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Deposit { since } => {
            let Some(item_guid) = probe.item_guid else {
                probe.phase = Phase::Done;
                return;
            };
            if let Some(bank_idx) = find_bank_slot(store, item_guid) {
                info!(
                    "PROBE_BANK: PASS (3 deposit) — {item_guid:#x} landed in bank slot \
                     {bank_idx} (the descriptor delta)"
                );
                probe.passes += 1;
                probe.bank_idx = Some(bank_idx);
                let wire_slot = SLOT_BANK_FIRST + bank_idx;
                let _ = net.0.send(ClientCommand::AutoStoreBankItem {
                    bag: BAG_PLAYER_INVENTORY,
                    slot: wire_slot,
                });
                info!("PROBE_BANK: (4 withdraw) AutoStoreBankItem(bank slot {bank_idx}) sent");
                probe.phase = Phase::Withdraw { since: now };
            } else if now - since > ACTION_TIMEOUT_SECS {
                error!(
                    "PROBE_BANK: FAIL (3 deposit) — {item_guid:#x} never landed in a bank slot \
                     within {ACTION_TIMEOUT_SECS}s"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Withdraw { since } => {
            let (Some(item_guid), Some(bank_idx)) = (probe.item_guid, probe.bank_idx) else {
                probe.phase = Phase::Done;
                return;
            };
            let bank_empty = store.0.player_bank_slot(bank_idx) != Some(item_guid);
            let back_in_pack = find_pack_item(store).is_some_and(|(_, g)| g == item_guid);
            if bank_empty && back_in_pack {
                info!(
                    "PROBE_BANK: PASS (4 withdraw) — {item_guid:#x} back in the pack, bank slot \
                     {bank_idx} empty"
                );
                probe.passes += 1;
                probe.phase = Phase::BuySlot {
                    since: now,
                    sent: false,
                    funded: false,
                };
            } else if now - since > ACTION_TIMEOUT_SECS {
                error!(
                    "PROBE_BANK: FAIL (4 withdraw) — {item_guid:#x} never round-tripped back to \
                     the pack within {ACTION_TIMEOUT_SECS}s (bank_empty={bank_empty} \
                     back_in_pack={back_in_pack})"
                );
                probe.fails += 1;
                probe.phase = Phase::BuySlot {
                    since: now,
                    sent: false,
                    funded: false,
                };
            }
        }
        Phase::BuySlot {
            since,
            sent,
            funded,
        } => {
            let Some(banker) = probe.banker else {
                probe.phase = Phase::Done;
                return;
            };
            if !sent {
                let purchased = store.0.player_bank_bag_slots_purchased().unwrap_or(0);
                let money = store.0.player_money().unwrap_or(0);
                probe.baseline_purchased = purchased;
                probe.baseline_money = money;
                if purchased >= MAX_BANK_BAGS {
                    warn!(
                        "PROBE_BANK: SKIP (5 buy_slot) — vault already full ({purchased}/\
                         {MAX_BANK_BAGS} bags purchased from an earlier run)"
                    );
                    probe.phase = Phase::Refusal {
                        since: now,
                        teleported: false,
                        bought: false,
                        events_baseline: events_len(&script),
                    };
                    return;
                }
                let cost = prices
                    .as_ref()
                    .and_then(|p| p.0.next_slot_price(purchased))
                    .or_else(|| PRICE_LADDER.get(purchased as usize).copied())
                    .unwrap_or(0);
                probe.next_cost = cost;
                info!(
                    "PROBE_BANK: (5 buy_slot) baseline purchased={purchased} money={money}c next \
                     rung costs {cost}c"
                );
                if money < cost && !funded {
                    // Fund the rung and come back next tick. `.modify money <n>` ADDS `n` copper
                    // (vmangos `HandleModifyMoneyCommand`: the arg is `addmoney`, not a set) and
                    // needs SEC_BASIC_ADMIN(4) — which probe accounts have had since they were
                    // actually raised to 6. This step used to SKIP here, because the
                    // accounts were gmlevel 3 and the grant would have been refused; the whole
                    // buy-slot leg was therefore unverified whenever the purse ran dry.
                    let grant = cost - money + FUND_MARGIN_COPPER;
                    info!("PROBE_BANK: (5 buy_slot) purse {money}c < {cost}c — granting {grant}c with `.modify money`");
                    let _ = net.0.send(ClientCommand::Chat {
                        kind: ChatKind::Say,
                        text: format!(".modify money {grant}"),
                        target: None,
                    });
                    probe.phase = Phase::BuySlot {
                        since: now,
                        sent: false,
                        funded: true,
                    };
                    return;
                }
                if money < cost {
                    error!(
                        "PROBE_BANK: FAIL (5 buy_slot) — purse is still {money}c against a \
                         {cost}c rung after `.modify money` — the grant did not land (check the \
                         `net: server says` lines)"
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Refusal {
                        since: now,
                        teleported: false,
                        bought: false,
                        events_baseline: events_len(&script),
                    };
                    return;
                }
                let _ = net.0.send(ClientCommand::BuyBankSlot { guid: banker });
                info!("PROBE_BANK: (5 buy_slot) BuyBankSlot({banker:#x}) sent");
                probe.phase = Phase::BuySlot {
                    since: now,
                    sent: true,
                    funded,
                };
                return;
            }
            let purchased = store.0.player_bank_bag_slots_purchased().unwrap_or(0);
            let money = store.0.player_money().unwrap_or(0);
            let want_purchased = probe.baseline_purchased + 1;
            let want_money = probe.baseline_money.saturating_sub(probe.next_cost);
            if purchased == want_purchased && money == want_money {
                info!(
                    "PROBE_BANK: PASS (5 buy_slot) — purchased {} -> {purchased}, money {} -> \
                     {money} (cost {}c debited)",
                    probe.baseline_purchased, probe.baseline_money, probe.next_cost
                );
                probe.passes += 1;
                probe.phase = Phase::Refusal {
                    since: now,
                    teleported: false,
                    bought: false,
                    events_baseline: events_len(&script),
                };
            } else if now - since > ACTION_TIMEOUT_SECS {
                error!(
                    "PROBE_BANK: FAIL (5 buy_slot) — no descriptor delta within \
                     {ACTION_TIMEOUT_SECS}s (purchased {} -> {purchased} wanted \
                     {want_purchased}; money {} -> {money} wanted {want_money})",
                    probe.baseline_purchased, probe.baseline_money
                );
                probe.fails += 1;
                probe.phase = Phase::Refusal {
                    since: now,
                    teleported: false,
                    bought: false,
                    events_baseline: events_len(&script),
                };
            }
        }
        Phase::Refusal {
            since,
            teleported,
            bought,
            events_baseline,
        } => {
            let Some(banker) = probe.banker else {
                probe.phase = Phase::Done;
                return;
            };
            if !teleported {
                let [bx, by, bz] = BANKER_AT;
                let x = bx + REFUSAL_OFFSET_X;
                info!(
                    "PROBE_BANK: (6 refusal, bonus) hopping {REFUSAL_OFFSET_X}yd out of range to \
                     ({x}, {by}, {bz})"
                );
                let _ = net.0.send(ClientCommand::Chat {
                    kind: ChatKind::Say,
                    target: None,
                    text: format!(".go xyz {x} {by} {bz} 0"),
                });
                probe.phase = Phase::Refusal {
                    since: now,
                    teleported: true,
                    bought: false,
                    events_baseline,
                };
                return;
            }
            if !bought {
                // A settle window before firing the stale-guid buy — mirrors step 1's
                // SETTLE_SECS, long enough for the `.go` + the range-guard close to land.
                if now - since < SETTLE_SECS {
                    return;
                }
                let _ = net.0.send(ClientCommand::BuyBankSlot { guid: banker });
                info!(
                    "PROBE_BANK: (6 refusal) BuyBankSlot({banker:#x}) sent out of range — \
                     expecting NOTBANKER"
                );
                probe.phase = Phase::Refusal {
                    since: now,
                    teleported: true,
                    bought: true,
                    events_baseline,
                };
                return;
            }
            let seen = events_len(&script);
            if seen > events_baseline {
                let text = last_event(&script);
                if text.to_lowercase().contains("banker") {
                    info!("PROBE_BANK: PASS (6 refusal) — surfaced error: {text:?}");
                    probe.passes += 1;
                } else {
                    warn!(
                        "PROBE_BANK: SKIP (6 refusal) — surfaced text {text:?} didn't match \
                         \"banker\" (flaky observation, not a wire failure)"
                    );
                }
                probe.phase = Phase::Done;
            } else if now - since > REFUSAL_GRACE_SECS {
                warn!(
                    "PROBE_BANK: SKIP (6 refusal) — no UI_ERROR_MESSAGE observed within \
                     {REFUSAL_GRACE_SECS}s of the send (flaky observation, not a wire failure)"
                );
                probe.phase = Phase::Done;
            }
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            info!(
                "PROBE_BANK: DONE pass={} fail={}",
                probe.passes, probe.fails
            );
            // The probe self-exit pattern (`ProbeExitPlugin::fire_probe_exit`): a polite AppExit
            // plus a hard backstop thread, so a net/winit teardown hang can't leave a zombie
            // client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_BANK: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
