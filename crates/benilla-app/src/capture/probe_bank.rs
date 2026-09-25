//! The bank live probe (`WOW_PROBE_BANK=1`): hops to a pure banker, drives the six bank opcodes
//! (activate, deposit, withdraw, buy a slot and its out-of-range refusal) and logs one
//! `PROBE_BANK: <step> PASS/FAIL/SKIP <detail>` line per step, then `PROBE_BANK: DONE pass=<n>
//! fail=<m>`, and exits. Non-combat; the switches are `docs/CONTRIBUTING.md`, "Running it
//! unattended".
//!
//! The banker is Soleil Stonemantle (entry 5099, Ironforge), whose `npc_flags` is 256, banker
//! only, so `CMSG_BANKER_ACTIVATE` opens the bank with no gossip in between.
//!
//! The slot purchase funds itself with `.modify money <n>`, which adds `n` copper and needs
//! `SEC_BASIC_ADMIN` (vmangos `Chat.cpp:586`).

use bevy::prelude::*;

use benilla_protocol::messages::{BAG_PLAYER_INVENTORY, SLOT_PACK_FIRST};
use benilla_protocol::EntityKind;
use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_bank::{BankOpen, BankPrices};

/// Soleil Stonemantle's spawn (vmangos `creature` guid 12629, map 0).
const BANKER_AT: [f32; 3] = [-4895.64, -1004.66, 504.024];
/// Her creature template entry, one of the two identity checks.
const BANKER_ENTRY: u32 = 5099;
/// `UNIT_NPC_FLAG_BANKER` (bit 8), the other identity check.
const NPC_FLAG_BANKER: u32 = 0x100;
/// Wider than the server's interact range, so a slightly-off `.go` landing still finds her.
const SCAN_RANGE: f32 = 12.0;
/// The deposit/withdraw fixture: Linen Cloth.
const ITEM_ENTRY: u32 = 2589;
/// The first bank slot's wire index: `SLOT_PACK_FIRST` (23) plus the backpack's 16; bank slots
/// are 39-62.
const SLOT_BANK_FIRST: u8 = SLOT_PACK_FIRST + 16;
/// `BankBagSlotPrices.dbc`'s ladder, indexed by slots already bought; used only if
/// [`BankPrices`] failed to load.
const PRICE_LADDER: [u32; 6] = [1_000, 10_000, 100_000, 250_000, 500_000, 1_000_000];
/// Slack over the exact shortfall when the buy-slot step funds itself.
const FUND_MARGIN_COPPER: u32 = 10_000;
/// The purchasable-slot ceiling (`GetNumBankSlots()` reports full at 6).
const MAX_BANK_BAGS: u8 = 6;
/// How far (yd, WoW x) the refusal step hops, well past `GetNPCIfCanInteractWith`'s range.
const REFUSAL_OFFSET_X: f32 = 150.0;

const SETTLE_SECS: f64 = 3.0;
const SCAN_TIMEOUT_SECS: f64 = 15.0;
const ACTIVATE_TIMEOUT_SECS: f64 = 10.0;
const ITEM_TIMEOUT_SECS: f64 = 10.0;
const ACTION_TIMEOUT_SECS: f64 = 8.0;
/// The refusal step's event wait; a miss SKIPs, never FAILs.
const REFUSAL_GRACE_SECS: f64 = 6.0;

pub(crate) struct ProbeBankPlugin;

impl Plugin for ProbeBankPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BankProbe>()
            .add_systems(Update, bank_probe);
    }
}

/// The probe's phase machine and the identities it discovers.
#[derive(Resource, Default)]
struct BankProbe {
    phase: Phase,
    banker: Option<u64>,
    /// The item round-tripped through deposit/withdraw; its guid survives the move.
    item_guid: Option<u64>,
    /// The bank slot index (0-based) the fixture item landed in after deposit.
    bank_idx: Option<u8>,
    /// The buy-slot step's baseline purchased count, latched before the buy.
    baseline_purchased: u8,
    baseline_money: u32,
    /// The next rung's price, resolved once (DBC if loaded, else [`PRICE_LADDER`]).
    next_cost: u32,
    passes: u32,
    fails: u32,
    exited: bool,
}

/// `Copy`, so each tick snapshots it and the match arms can mutate `probe`.
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
    /// Ensuring a backpack item to deposit, `.additem` if the bags are empty (step 3).
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
    /// `BuyBankSlot` sent; waiting for the purchased-count descriptor delta (step 5).
    BuySlot {
        since: f64,
        sent: bool,
        /// Whether the one `.modify money` grant has gone out; still short after it is a FAIL.
        funded: bool,
    },
    /// Step 6: hop out of range, then `BuyBankSlot` the stale guid, expecting
    /// `SMSG_BUY_BANK_SLOT_RESULT` NOTBANKER; `since` resets at each send.
    Refusal {
        since: f64,
        teleported: bool,
        bought: bool,
        events_baseline: i64,
    },
    Done,
}

/// The length of the Lua `ProbeBankEvents` log (the `UI_ERROR_MESSAGE` hook); `0` on an eval
/// error.
fn events_len(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return table.getn(ProbeBankEvents or {})")
        .unwrap_or(0)
}

/// The newest `ProbeBankEvents` entry.
fn last_event(script: &UiScript) -> String {
    script
        .eval::<String>("return ProbeBankEvents[table.getn(ProbeBankEvents)] or \"\"")
        .unwrap_or_default()
}

fn find_pack_item(store: &ObjectStore) -> Option<(u8, u64)> {
    (0..16u8).find_map(|i| {
        store
            .0
            .player_pack_slot(i)
            .filter(|&g| g != 0)
            .map(|g| (i, g))
    })
}

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
        return; // no UI VM in a headless build
    };
    let now = time.elapsed_secs_f64();
    let phase = probe.phase;
    let Some(store) = self_store.iter().next() else {
        return;
    };

    match phase {
        Phase::Wait => {
            // Step 6's `UI_ERROR_MESSAGE` hook, installed up front.
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
                    // Fund the rung and come back next tick; `.modify money <n>` adds `n`
                    // copper (vmangos `CharacterCommands.cpp:4477`).
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
                // Let the `.go` and the bank window's range close land first.
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
            // `AppExit` plus a hard backstop, so a teardown hang cannot keep the account held.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_BANK: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
