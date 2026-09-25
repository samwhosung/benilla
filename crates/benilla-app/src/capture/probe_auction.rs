//! The auction live probe (`WOW_PROBE_AUCTION=1`): hops to a Stormwind auctioneer, greets it on
//! the wire and drives browse, throttle, sell, owner list and cancel through the live Lua VM as a
//! click would, logging one `PROBE_AUCTION:` PASS/FAIL/SKIP line per step, then
//! `PROBE_AUCTION: DONE pass=<n> fail=<m>`, and exits. Non-combat; the switches are
//! `docs/CONTRIBUTING.md`, "Running it unattended".
//!
//! The window opens on the server's `MSG_AUCTION_HELLO` reply, not on the click, and the reply's
//! house id (checked in `1..=7`) keys the deposit rate the sell pane quotes.
//!
//! Server behaviour the phases follow (vmangos `AuctionHouseHandler.cpp`):
//! - One list request at a time: a second while one is in flight is dropped silently (`:710`).
//! - `etime` is minutes, one of 1, 4 or 12 times the 2 h `MIN_AUCTION_TIME` (`:287-291`).
//! - The deposit is `SellPrice × count × (etime / MIN_AUCTION_TIME) × depositPercent / 100`
//!   (`AuctionHouseMgr.cpp:98`); at 120 minutes the client's `CalculateAuctionDeposit` agrees
//!   with it to the copper.
//! - A cancel with no bidder is free and returns the item by mail.
//! - A GM account is refused with `RESTRICTED_ACCOUNT` unless `GM.AllowTrades` is on (`:249`).
//!
//! The auction is tracked by the id the server's `STARTED` result carried, never re-found by a
//! predicate over the rows, since a successful action is what stops a row matching.

use bevy::prelude::*;

use benilla_protocol::messages::{auction_action, auction_duration, auction_error};
use benilla_protocol::EntityKind;
use benilla_ui::script::{UiScript, LIST, OWNER};

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_auction::AuctionOpen;

/// Auctioneer Fitch's spawn (vmangos `creature` guid 12696, map 0), the `.go xyz` target; the
/// unit itself is found by its npc flag.
const AUCTIONEER_AT: [f32; 3] = [-8821.53, 659.886, 97.4645];
/// Her creature template entry, reported only; her `npc_flags` is 4096, auctioneer only, so the
/// right-click route sends `AuctionHello` with no gossip in between.
const AUCTIONEER_ENTRY: u32 = 8719;
/// `UNIT_NPC_FLAG_AUCTIONEER` (bit 12).
const NPC_FLAG_AUCTIONEER: u32 = 0x1000;
/// Wider than the interact range, so a slightly-off `.go` landing still finds one; the nearest is
/// greeted, as several auctioneers stand a few yards apart.
const SCAN_RANGE: f32 = 12.0;
/// vmangos `INTERACTION_DISTANCE` (`ObjectDefines.h:24`); past it a greeting gets no reply.
const INTERACT_MAX_YD: f32 = 5.0;

/// The step (5) fixture: Linen Cloth, whose 13 c sell price gives a stack of five a nonzero
/// deposit.
const ITEM_ENTRY: u32 = 2589;
const ITEM_COUNT: u32 = 5;
/// 120 minutes, where the client's deposit quote and the server's charge agree exactly.
const DURATION_MINUTES: u32 = auction_duration::SHORT_MINUTES;

const SETTLE_SECS: f64 = 3.0;
const SCAN_TIMEOUT_SECS: f64 = 15.0;
const HELLO_TIMEOUT_SECS: f64 = 10.0;
const LIST_TIMEOUT_SECS: f64 = 15.0;
const ACTION_TIMEOUT_SECS: f64 = 10.0;
const ITEM_TIMEOUT_SECS: f64 = 10.0;
/// How long nothing must go out or come back before the throttle's silent refusal is believed.
const REFUSAL_WATCH_SECS: f64 = 2.0;
/// The client throttle is 5 s.
const RECOVER_TIMEOUT_SECS: f64 = 12.0;
/// The owner list re-ask cadence, spaced because the server drops an in-flight second request.
const OWNER_REASK_SECS: f64 = 3.0;
const OWNER_TIMEOUT_SECS: f64 = 20.0;
/// How long a UI event may trail its wire state: `feed_auction` fires it, unordered with this
/// probe.
const EVENT_GRACE_SECS: f64 = 1.5;
/// How long the sell slot has to empty after the listing is away.
const SELL_SLOT_GRACE_SECS: f64 = 3.0;

pub(crate) struct ProbeAuctionPlugin;

impl Plugin for ProbeAuctionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuctionProbe>()
            .add_systems(Update, auction_probe);
    }
}

/// The probe's phase machine and the identities it discovers along the way.
#[derive(Resource, Default)]
struct AuctionProbe {
    phase: Phase,
    /// The auctioneer's guid, once streamed in.
    auctioneer: Option<u64>,
    /// The auction id the server's `STARTED` result carried; every later step tracks it.
    auction_id: Option<u32>,
    /// The sell-slot stack's vendor value, which both deposits are computed from.
    stack_value: i64,
    /// The client's `CalculateAuctionDeposit` quote, held against the server's charge.
    deposit: i64,
    min_bid: i64,
    buyout: i64,
    /// The purse just before `StartAuction`.
    baseline_money: u32,
    /// `AUCTION_OWNED_LIST_UPDATE`'s count just before `StartAuction`, so an owner re-query the
    /// `STARTED` result queues cannot fire the event before the baseline is read.
    owned_event_baseline: i64,
    /// Red `UI_ERROR_MESSAGE` lines raised before the window opened: on a probe account the GM
    /// and GOD mode banners, not the arc's.
    red_baseline: i64,
    passes: u32,
    fails: u32,
    /// Latched once [`Phase::Done`] has fired its exit.
    exited: bool,
}

/// `Copy`, so each tick snapshots it and the arms can mutate `probe`; an arm that keeps waiting
/// never writes it.
#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// `.go` sent; settling while the world streams the auctioneer in (step 1).
    Settling {
        sent_at: f64,
    },
    /// `AuctionHello` sent; waiting for the reply to open the session (step 2).
    Greet {
        sent_at: f64,
        show_baseline: i64,
    },
    /// `QueryAuctionItems` issued; waiting for the list result to land (step 3).
    Browse {
        since: f64,
        sent: bool,
        results_baseline: u32,
        event_baseline: i64,
    },
    /// The list result is in; waiting for the window's own `AUCTION_ITEM_LIST_UPDATE` (step 3b).
    BrowseEvent {
        since: f64,
        event_baseline: i64,
    },
    /// A second query, which the client throttle must refuse (step 4a).
    Refuse {
        since: f64,
        sent: bool,
        sent_baseline: u32,
        results_baseline: u32,
    },
    /// Waiting for `CanSendAuctionQuery()` to return true, then a third query that must go out
    /// (step 4b).
    Recover {
        since: f64,
        sent: bool,
        sent_baseline: u32,
    },
    /// Finding (or `.additem`-ing) the fixture in the bags (step 5 prep).
    EnsureItem {
        since: f64,
        sent: bool,
    },
    /// Dropped in the sell slot; waiting for it to read back a priced item (step 5 prep).
    Attach {
        since: f64,
    },
    /// `StartAuction` away; waiting for `SMSG_AUCTION_COMMAND_RESULT` (step 5).
    Sell {
        since: f64,
    },
    /// The money and sell-slot assertions, each latched since they land at different moments.
    SellMoney {
        since: f64,
        money_done: bool,
        slot_done: bool,
    },
    /// `GetOwnerAuctionItems()`; waiting for our own auction id to appear in the `"owner"` list
    /// (step 6).
    OwnerList {
        since: f64,
        last_ask: f64,
        event_baseline: i64,
    },
    /// Our row is on the owner page; waiting for `AUCTION_OWNED_LIST_UPDATE` (step 6b).
    OwnerEvent {
        since: f64,
        event_baseline: i64,
    },
    /// `CancelAuction`; waiting for `REMOVED` and for the row to leave the list (step 7).
    Cancel {
        since: f64,
        sent: bool,
        removed: bool,
        last_ask: f64,
    },
    Done,
}

/// One Lua-side event counter (`ProbeAuctionEvents`), or 0 on an eval error.
fn event_count(script: &UiScript, event: &str) -> i64 {
    script
        .eval::<i64>(&format!(
            "return (ProbeAuctionEvents or {{}})['{event}'] or 0"
        ))
        .unwrap_or(0)
}

/// The newest `UI_ERROR_MESSAGE` text the hook logged, where a refused auction command surfaces.
fn last_error(script: &UiScript) -> String {
    script
        .eval::<String>("return ProbeAuctionErrors[table.getn(ProbeAuctionErrors)] or \"\"")
        .unwrap_or_default()
}

/// Red `UI_ERROR_MESSAGE` lines raised so far, as the hook's own array counts them.
fn red_count(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return table.getn(ProbeAuctionErrors)")
        .unwrap_or(-1)
}

/// (6d) The `STARTED` verdict prints as a chat line (catalog row `0x178`, kind 0,
/// `CHAT_MSG_SYSTEM`) and never as a red line; matched against the player's own
/// `ERR_AUCTION_STARTED`.
fn started_chat_check(script: &UiScript, probe: &mut AuctionProbe) {
    let said = script
        .eval::<i64>(
            "local want = getglobal(\"ERR_AUCTION_STARTED\") \
             if not want or want == \"\" then return -1 end \
             for i = 1, table.getn(ProbeAuctionChat) do \
                 if ProbeAuctionChat[i] == want then return 1 end \
             end \
             return 0",
        )
        .unwrap_or(-2);
    let reds = red_count(script) - probe.red_baseline;
    match (said, reds) {
        (1, 0) => {
            info!(
                "PROBE_AUCTION: PASS (6d chat) — the STARTED verdict landed as a CHAT_MSG_SYSTEM \
                 line and the run raised no red UI_ERROR_MESSAGE at all"
            );
            probe.passes += 1;
        }
        (-1, _) => {
            error!(
                "PROBE_AUCTION: FAIL (6d chat) — ERR_AUCTION_STARTED is empty in the player's own \
                 GlobalStrings, so the assertion cannot be made (chain not loaded?)"
            );
            probe.fails += 1;
        }
        (said, reds) => {
            // Listed from the baseline on: the login banners are not this arc's output.
            let lines = script
                .eval::<String>(&format!(
                    "local t = {{}} \
                     for i = 1, table.getn(ProbeAuctionChat) do table.insert(t, ProbeAuctionChat[i]) end \
                     for i = {} + 1, table.getn(ProbeAuctionErrors) do table.insert(t, \"RED:\" .. ProbeAuctionErrors[i]) end \
                     return table.concat(t, \" | \")",
                    probe.red_baseline.max(0)
                ))
                .unwrap_or_default();
            error!(
                "PROBE_AUCTION: FAIL (6d chat) — wanted the STARTED line in chat and zero red \
                 lines; got said={said} reds={reds}. Everything the run said: {lines:?}"
            );
            probe.fails += 1;
        }
    }
}

/// (6c) The owner row's money frame: `MoneyTypeInfo["AUCTION"]`'s `showSmallerCoins` hides only
/// leading zero denominations, so a 1-silver bid reads `1s 0c` (`MoneyFrame.lua:223-253`). The
/// Auctions tab must be in front: its `OnShow` sets the `page` that `AuctionFrameAuctions_Update`
/// paints the row with.
fn owner_money_check(script: &UiScript, probe: &mut AuctionProbe) {
    let (gold, silver, copper) = (
        probe.min_bid / 10_000,
        (probe.min_bid % 10_000) / 100,
        probe.min_bid % 100,
    );
    let want_gold = gold > 0;
    let want_silver = want_gold || silver > 0;
    let read = script.eval::<(bool, String, bool, String, bool, String)>(
        "local m = \"AuctionsButton1MoneyFrame\" \
         local g, s, c = getglobal(m .. \"GoldButton\"), getglobal(m .. \"SilverButton\"), getglobal(m .. \"CopperButton\") \
         return g:IsShown(), tostring(g:GetText()), s:IsShown(), tostring(s:GetText()), c:IsShown(), tostring(c:GetText())",
    );
    let Ok((gold_on, gold_text, silver_on, silver_text, copper_on, copper_text)) = read else {
        error!(
            "PROBE_AUCTION: FAIL (6c owner-money) — AuctionsButton1MoneyFrame's coin buttons did \
             not read back at all; the row is on SmallMoneyFrameTemplate, so this means the \
             template did not resolve"
        );
        probe.fails += 1;
        return;
    };
    let ok = gold_on == want_gold
        && silver_on == want_silver
        && copper_on
        && (!want_gold || gold_text == gold.to_string())
        && (!want_silver || silver_text == silver.to_string())
        && copper_text == copper.to_string();
    if ok {
        info!(
            "PROBE_AUCTION: PASS (6c owner-money) — the {}c minimum bid paints {gold_on}/{silver_on}/true \
             gold/silver/copper reading {gold_text:?}/{silver_text:?}/{copper_text:?}: the leading \
             zeros collapsed, the trailing ones stayed",
            probe.min_bid
        );
        probe.passes += 1;
    } else {
        error!(
            "PROBE_AUCTION: FAIL (6c owner-money) — {}c should paint gold={want_gold} \
             silver={want_silver} copper=true with {gold}/{silver}/{copper}; got \
             gold={gold_on}{gold_text:?} silver={silver_on}{silver_text:?} copper={copper_on}{copper_text:?}",
            probe.min_bid
        );
        probe.fails += 1;
    }
}

/// The 1-based display index of our auction in the owner list. `CancelAuction(index)` maps back
/// through the same sorted view, which is wire order while no header is clicked.
fn owner_index_of(auction: &AuctionOpen, auction_id: u32) -> Option<u32> {
    auction.lists[OWNER]
        .entries
        .iter()
        .position(|e| e.auction_id == auction_id)
        .map(|i| i as u32 + 1)
}

/// Step (5)'s entry, where every step-4 exit lands.
fn ensure_item(now: f64) -> Phase {
    Phase::EnsureItem {
        since: now,
        sent: false,
    }
}

/// Step (7)'s entry, reached from step (6) pass or fail so a created listing is never left.
fn cancel_at(now: f64) -> Phase {
    Phase::Cancel {
        since: now,
        sent: false,
        removed: false,
        last_ask: 0.0,
    }
}

/// A `(action, error)` verdict, decoded for the trace.
fn action_name(action: u32) -> &'static str {
    match action {
        auction_action::STARTED => "STARTED",
        auction_action::REMOVED => "REMOVED",
        auction_action::BID_PLACED => "BID_PLACED",
        _ => "UNKNOWN",
    }
}

fn error_name(error: u32) -> &'static str {
    match error {
        auction_error::OK => "OK",
        auction_error::INVENTORY => "INVENTORY",
        auction_error::DATABASE => "DATABASE",
        auction_error::NOT_ENOUGH_MONEY => "NOT_ENOUGH_MONEY",
        auction_error::ITEM_NOT_FOUND => "ITEM_NOT_FOUND",
        auction_error::HIGHER_BID => "HIGHER_BID",
        auction_error::BID_INCREMENT => "BID_INCREMENT",
        auction_error::BID_OWN => "BID_OWN",
        auction_error::RESTRICTED_ACCOUNT => "RESTRICTED_ACCOUNT",
        _ => "unnamed",
    }
}

#[allow(clippy::too_many_lines)]
fn auction_probe(
    time: ProbeClock,
    mut probe: ResMut<AuctionProbe>,
    auction: Res<AuctionOpen>,
    script: Option<NonSendMut<UiScript>>,
    self_player: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(store) = self_player.single() else {
        return; // not in-world yet
    };
    let Some(script) = script else {
        return; // no UI VM in a headless build
    };
    let now = time.elapsed_secs_f64();
    let phase = probe.phase;

    match phase {
        Phase::Wait => {
            // One hidden frame counting each event this arc fires, logging the red and system
            // chat texts.
            if let Err(e) = script.run(
                r#"
                if not ProbeAuctionHooked then
                    ProbeAuctionHooked = true
                    ProbeAuctionEvents = {}
                    ProbeAuctionErrors = {}
                    ProbeAuctionChat = {}
                    local f = CreateFrame("Frame")
                    f:RegisterEvent("AUCTION_HOUSE_SHOW")
                    f:RegisterEvent("AUCTION_HOUSE_CLOSED")
                    f:RegisterEvent("AUCTION_ITEM_LIST_UPDATE")
                    f:RegisterEvent("AUCTION_OWNED_LIST_UPDATE")
                    f:RegisterEvent("AUCTION_BIDDER_LIST_UPDATE")
                    f:RegisterEvent("NEW_AUCTION_UPDATE")
                    f:RegisterEvent("UI_ERROR_MESSAGE")
                    f:RegisterEvent("CHAT_MSG_SYSTEM")
                    f:SetScript("OnEvent", function()
                        ProbeAuctionEvents[event] = (ProbeAuctionEvents[event] or 0) + 1
                        if event == "UI_ERROR_MESSAGE" then
                            table.insert(ProbeAuctionErrors, arg1 or "")
                        elseif event == "CHAT_MSG_SYSTEM" then
                            table.insert(ProbeAuctionChat, arg1 or "")
                        end
                    end)
                end
                "#,
            ) {
                error!("PROBE_AUCTION: FAIL (0 setup) — installing the event hook: {e}");
                probe.fails += 1;
                probe.phase = Phase::Done;
                return;
            }
            let [x, y, z] = AUCTIONEER_AT;
            info!("PROBE_AUCTION: hopping to Auctioneer Fitch (entry {AUCTIONEER_ENTRY}) at ({x}, {y}, {z})");
            // GM commands ride as Say lines. The fixture is not granted here: step (5) reuses a
            // stack from the bags first, since each cancel returns its stack by mail.
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
            // The nearest auctioneer, not the first the ECS yields.
            let nearest = units
                .iter()
                .filter(|(_, net_e, store, tf)| {
                    net_e.kind == EntityKind::Unit
                        && store.0.unit_npc_flags() & NPC_FLAG_AUCTIONEER != 0
                        && tf.translation.distance(me) < SCAN_RANGE
                })
                .map(|(guid, _, store, tf)| {
                    (
                        guid.0,
                        store.0.object_entry().unwrap_or(0),
                        store.0.unit_npc_flags(),
                        tf.translation.distance(me),
                    )
                })
                .min_by(|a, b| a.3.total_cmp(&b.3));
            if let Some((guid, entry, flags, dist)) = nearest.filter(|n| n.3 <= INTERACT_MAX_YD) {
                info!(
                    "PROBE_AUCTION: PASS (1 reach) — auctioneer {guid:#x} (entry {entry}) \
                     streamed in {dist:.1} yd away, npc_flags {flags:#x} — inside the server's \
                     {INTERACT_MAX_YD} yd interaction distance"
                );
                probe.passes += 1;
                probe.auctioneer = Some(guid);
                // What a right-click on a pure auctioneer sends; the reply opens the window.
                let _ = net.0.send(ClientCommand::AuctionHello { auctioneer: guid });
                info!("PROBE_AUCTION: (2 greet) MSG_AUCTION_HELLO sent — the window opens on the REPLY, not on this");
                probe.phase = Phase::Greet {
                    sent_at: now,
                    show_baseline: event_count(&script, "AUCTION_HOUSE_SHOW"),
                };
            } else if now - sent_at > SCAN_TIMEOUT_SECS {
                match nearest {
                    Some((guid, entry, _, dist)) => error!(
                        "PROBE_AUCTION: FAIL (1 reach) — the nearest auctioneer {guid:#x} (entry \
                         {entry}) is {dist:.1} yd away, past the server's {INTERACT_MAX_YD} yd \
                         interaction distance, so a greeting would be refused with no packet. The \
                         `.go` landed short of the spawn this probe aims at"
                    ),
                    None => error!(
                        "PROBE_AUCTION: FAIL (1 reach) — no auctioneer-flagged unit within \
                         {SCAN_RANGE} yd {SCAN_TIMEOUT_SECS}s after the `.go` (did the teleport \
                         land? check the preflight banner and the `net: server says` lines)"
                    ),
                }
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Greet {
            sent_at,
            show_baseline,
        } => {
            let opened = auction.auctioneer.is_some();
            let show_fired = event_count(&script, "AUCTION_HOUSE_SHOW") > show_baseline;
            let next = |auction: &AuctionOpen, script: &UiScript| Phase::Browse {
                since: now,
                sent: false,
                results_baseline: auction.wire.list_results[LIST],
                event_baseline: event_count(script, "AUCTION_ITEM_LIST_UPDATE"),
            };
            if opened && show_fired {
                let house = auction.house_id;
                let right = auction.auctioneer == probe.auctioneer;
                let house_ok = (1..=7).contains(&house);
                if right && house_ok {
                    info!(
                        "PROBE_AUCTION: PASS (2 greet) — the hello REPLY opened the session: \
                         auctioneer {:#x}, house_id {house} (in 1..=7), AUCTION_HOUSE_SHOW fired",
                        auction.auctioneer.unwrap_or(0)
                    );
                    probe.passes += 1;
                    // The arc's red-line tally starts here.
                    probe.red_baseline = red_count(&script);
                } else {
                    error!(
                        "PROBE_AUCTION: FAIL (2 greet) — the session opened, but on {:?} (we \
                         greeted {:?}) with house_id {house} (wanted 1..=7)",
                        auction.auctioneer, probe.auctioneer
                    );
                    probe.fails += 1;
                }
                probe.phase = next(&auction, &script);
            } else if now - sent_at > HELLO_TIMEOUT_SECS {
                if opened {
                    error!(
                        "PROBE_AUCTION: FAIL (2 greet) — the reply opened the session \
                         (auctioneer {:?}, house_id {}) but AUCTION_HOUSE_SHOW never fired within \
                         {HELLO_TIMEOUT_SECS}s, so no window would have appeared",
                        auction.auctioneer, auction.house_id
                    );
                    probe.fails += 1;
                    probe.phase = next(&auction, &script);
                } else {
                    error!(
                        "PROBE_AUCTION: FAIL (2 greet) — no MSG_AUCTION_HELLO reply within \
                         {HELLO_TIMEOUT_SECS}s, so the window never opened and everything \
                         downstream is unreachable. The server answers nothing at all when the \
                         player is out of `GetNPCIfCanInteractWith` range or the unit lacks bit 12"
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                }
            }
        }
        Phase::Browse {
            since,
            sent,
            results_baseline,
            event_baseline,
        } => {
            if !sent {
                // The Browse pane's call with every filter at its default: empty name and level
                // strings, no class/subclass/invtype, page 0, not usable-only, quality all (-1).
                if let Err(e) =
                    script.run(r#"QueryAuctionItems("", "", "", nil, nil, nil, 0, nil, -1)"#)
                {
                    error!("PROBE_AUCTION: FAIL (3 browse) — QueryAuctionItems errored: {e}");
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                    return;
                }
                info!("PROBE_AUCTION: (3 browse) QueryAuctionItems(default filter, page 0) queued");
                probe.phase = Phase::Browse {
                    since: now,
                    sent: true,
                    results_baseline,
                    event_baseline,
                };
                return;
            }
            if auction.wire.list_results[LIST] > results_baseline {
                let (batch, total) = script
                    .eval::<(i64, i64)>(r#"return GetNumAuctionItems("list")"#)
                    .unwrap_or((-1, -1));
                info!(
                    "PROBE_AUCTION: PASS (3 browse) — SMSG_AUCTION_LIST_RESULT landed; \
                     GetNumAuctionItems(\"list\") = rows={batch} total={total} (an empty auction \
                     house is a legitimate result — the assertion is the round trip)"
                );
                probe.passes += 1;
                probe.phase = Phase::BrowseEvent {
                    since: now,
                    event_baseline,
                };
            } else if now - since > LIST_TIMEOUT_SECS {
                error!(
                    "PROBE_AUCTION: FAIL (3 browse) — no list result within {LIST_TIMEOUT_SECS}s \
                     (browse queries actually sent: {})",
                    auction.wire.browse_sent
                );
                probe.fails += 1;
                probe.phase = ensure_item(now);
            }
        }
        Phase::BrowseEvent {
            since,
            event_baseline,
        } => {
            if event_count(&script, "AUCTION_ITEM_LIST_UPDATE") > event_baseline {
                info!("PROBE_AUCTION: PASS (3b list-event) — AUCTION_ITEM_LIST_UPDATE fired for the result");
                probe.passes += 1;
            } else if now - since > EVENT_GRACE_SECS {
                error!(
                    "PROBE_AUCTION: FAIL (3b list-event) — the result landed but \
                     AUCTION_ITEM_LIST_UPDATE never fired within {EVENT_GRACE_SECS}s: \
                     `feed_auction` fires the list events only when the pushed snapshot DIFFERS, \
                     so a result that changes nothing (an empty house, a repeated search) is \
                     silent — and the Browse pane only clears `isSearching` on that event"
                );
                probe.fails += 1;
            } else {
                return;
            }
            probe.phase = Phase::Refuse {
                since: now,
                sent: false,
                sent_baseline: auction.wire.browse_sent,
                results_baseline: auction.wire.list_results[LIST],
            };
        }
        Phase::Refuse {
            since,
            sent,
            sent_baseline,
            results_baseline,
        } => {
            if !sent {
                let can = script
                    .eval::<bool>("return CanSendAuctionQuery() and true or false")
                    .unwrap_or(true);
                if can {
                    warn!(
                        "PROBE_AUCTION: SKIP (4 throttle) — CanSendAuctionQuery() was already \
                         true when the first result landed, so the 5 s gate had expired before \
                         the round trip finished; there is no refusal to observe"
                    );
                    probe.phase = ensure_item(now);
                    return;
                }
                info!("PROBE_AUCTION: (4 throttle) CanSendAuctionQuery() is false — issuing a second query that must be refused");
                if let Err(e) =
                    script.run(r#"QueryAuctionItems("", "", "", nil, nil, nil, 0, nil, -1)"#)
                {
                    error!("PROBE_AUCTION: FAIL (4 throttle) — the second QueryAuctionItems errored: {e}");
                    probe.fails += 1;
                    probe.phase = ensure_item(now);
                    return;
                }
                probe.phase = Phase::Refuse {
                    since: now,
                    sent: true,
                    sent_baseline,
                    results_baseline,
                };
                return;
            }
            if auction.wire.browse_sent > sent_baseline {
                error!(
                    "PROBE_AUCTION: FAIL (4 throttle) — the second query went OUT anyway \
                     (browse_sent {sent_baseline} -> {}); the 5 s gate did not hold",
                    auction.wire.browse_sent
                );
                probe.fails += 1;
                probe.phase = Phase::Recover {
                    since: now,
                    sent: false,
                    sent_baseline: auction.wire.browse_sent,
                };
                return;
            }
            if auction.wire.list_results[LIST] > results_baseline {
                error!(
                    "PROBE_AUCTION: FAIL (4 throttle) — a second list result came back \
                     (list_results {results_baseline} -> {}) though nothing was supposed to go out",
                    auction.wire.list_results[LIST]
                );
                probe.fails += 1;
                probe.phase = Phase::Recover {
                    since: now,
                    sent: false,
                    sent_baseline,
                };
                return;
            }
            if now - since > REFUSAL_WATCH_SECS {
                info!(
                    "PROBE_AUCTION: PASS (4a throttle-refuses) — the second query was dropped \
                     silently: browse_sent still {sent_baseline} and list_results still \
                     {results_baseline} after {REFUSAL_WATCH_SECS}s"
                );
                probe.passes += 1;
                probe.phase = Phase::Recover {
                    since: now,
                    sent: false,
                    sent_baseline,
                };
            }
        }
        Phase::Recover {
            since,
            sent,
            sent_baseline,
        } => {
            if !sent {
                let can = script
                    .eval::<bool>("return CanSendAuctionQuery() and true or false")
                    .unwrap_or(false);
                if can {
                    info!("PROBE_AUCTION: (4b throttle-recovers) CanSendAuctionQuery() came back — issuing a third query, which must go out");
                    if let Err(e) =
                        script.run(r#"QueryAuctionItems("", "", "", nil, nil, nil, 0, nil, -1)"#)
                    {
                        error!("PROBE_AUCTION: FAIL (4b throttle-recovers) — the third QueryAuctionItems errored: {e}");
                        probe.fails += 1;
                        probe.phase = ensure_item(now);
                        return;
                    }
                    probe.phase = Phase::Recover {
                        since: now,
                        sent: true,
                        sent_baseline,
                    };
                } else if now - since > RECOVER_TIMEOUT_SECS {
                    error!(
                        "PROBE_AUCTION: FAIL (4b throttle-recovers) — CanSendAuctionQuery() still \
                         false {RECOVER_TIMEOUT_SECS}s after the refusal; the gate never cleared"
                    );
                    probe.fails += 1;
                    probe.phase = ensure_item(now);
                }
                return;
            }
            if auction.wire.browse_sent > sent_baseline {
                info!(
                    "PROBE_AUCTION: PASS (4b throttle-recovers) — the third query went out \
                     (browse_sent {sent_baseline} -> {})",
                    auction.wire.browse_sent
                );
                probe.passes += 1;
                probe.phase = ensure_item(now);
            } else if now - since > ACTION_TIMEOUT_SECS {
                error!(
                    "PROBE_AUCTION: FAIL (4b throttle-recovers) — CanSendAuctionQuery() was true \
                     but the query never reached the wire within {ACTION_TIMEOUT_SECS}s"
                );
                probe.fails += 1;
                probe.phase = ensure_item(now);
            }
        }
        Phase::EnsureItem { since, sent } => {
            // A bag scan by item link, returning bag*100+slot or -1.
            let found = script
                .eval::<i64>(&format!(
                    "for bag=0,4 do local n=GetContainerNumSlots(bag) or 0 \
                     for slot=1,n do local link=GetContainerItemLink(bag,slot) \
                     if link and string.find(link,'item:{ITEM_ENTRY}:',1,true) then \
                     return bag*100+slot end end end return -1"
                ))
                .unwrap_or(-1);
            if found >= 0 {
                let (bag, lslot) = (found / 100, found % 100);
                // The two calls the sell button's `AuctionsItemButton_OnClick` chain makes.
                if let Err(e) = script.run(&format!(
                    "PickupContainerItem({bag}, {lslot}) ClickAuctionSellItemButton()"
                )) {
                    error!("PROBE_AUCTION: FAIL (5 sell) — attaching bag {bag} slot {lslot} errored: {e}");
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                    return;
                }
                info!("PROBE_AUCTION: (5 sell) fixture {ITEM_ENTRY} at bag {bag} slot {lslot} — picked up and dropped in the sell slot");
                probe.phase = Phase::Attach { since: now };
                return;
            }
            if !sent {
                info!("PROBE_AUCTION: (5 sell) nothing to reuse in the bags — `.additem {ITEM_ENTRY} {ITEM_COUNT}`");
                let _ = net.0.send(ClientCommand::Chat {
                    kind: ChatKind::Say,
                    target: None,
                    text: format!(".additem {ITEM_ENTRY} {ITEM_COUNT}"),
                });
                probe.phase = Phase::EnsureItem {
                    since: now,
                    sent: true,
                };
            } else if now - since > ITEM_TIMEOUT_SECS {
                error!(
                    "PROBE_AUCTION: FAIL (5 sell) — item {ITEM_ENTRY} never appeared in the bags \
                     within {ITEM_TIMEOUT_SECS}s of `.additem` (bags full? check the `net: server \
                     says` lines for the GM command's own answer)"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Attach { since } => {
            // Six values, the first nil for an empty slot. Waits for a nonzero `price` (the
            // stack's vendor value) so the deposit is not quoted off a template still in flight.
            let (name, count, price) = script
                .eval::<(String, i64, i64)>(
                    "local n, _, c, _, _, p = GetAuctionSellItemInfo() return n or \"\", c or 0, p or 0",
                )
                .unwrap_or_default();
            if !name.is_empty() && price > 0 {
                let deposit = script
                    .eval::<i64>(&format!(
                        "return CalculateAuctionDeposit({DURATION_MINUTES})"
                    ))
                    .unwrap_or(-1);
                // The sell pane's suggested opening price: `max(100, floor(price * 1.5))`
                // (`AuctionSellItemButton_OnEvent`).
                let min_bid = (price * 3 / 2).max(100);
                let buyout = min_bid * 2;
                let money = store.0.player_money().unwrap_or(0);
                info!(
                    "PROBE_AUCTION: (5 sell) slot reads {name:?} x{count}, stack value {price}c; \
                     CalculateAuctionDeposit({DURATION_MINUTES}) = {deposit}c at \
                     GetAuctionHouseDepositRate()={}%; listing for min_bid {min_bid}c buyout \
                     {buyout}c; purse {money}c",
                    script
                        .eval::<i64>("return GetAuctionHouseDepositRate()")
                        .unwrap_or(-1),
                );
                probe.stack_value = price;
                probe.deposit = deposit;
                probe.min_bid = min_bid;
                probe.buyout = buyout;
                probe.baseline_money = money;
                probe.owned_event_baseline = event_count(&script, "AUCTION_OWNED_LIST_UPDATE");
                if let Err(e) = script.run(&format!(
                    "StartAuction({min_bid}, {buyout}, {DURATION_MINUTES})"
                )) {
                    error!("PROBE_AUCTION: FAIL (5 sell) — StartAuction errored: {e}");
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                    return;
                }
                probe.phase = Phase::Sell { since: now };
            } else if now - since > ACTION_TIMEOUT_SECS {
                error!(
                    "PROBE_AUCTION: FAIL (5 sell) — the sell slot never read back a priced item \
                     within {ACTION_TIMEOUT_SECS}s (name={name:?} count={count} price={price}); \
                     ClickAuctionSellItemButton took nothing, or the item template never landed"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Sell { since } => match auction.wire.last_command {
            Some((id, action, error)) if action == auction_action::STARTED => {
                if error == auction_error::OK {
                    info!(
                        "PROBE_AUCTION: PASS (5 sell) — SMSG_AUCTION_COMMAND_RESULT \
                             action=STARTED error=OK, auction id {id}"
                    );
                    probe.passes += 1;
                    probe.auction_id = Some(id);
                    probe.phase = Phase::SellMoney {
                        since: now,
                        money_done: false,
                        slot_done: false,
                    };
                } else {
                    error!(
                        "PROBE_AUCTION: FAIL (5 sell) — SMSG_AUCTION_COMMAND_RESULT \
                             action=STARTED error={} ({error}); surfaced line: {:?}",
                        error_name(error),
                        last_error(&script)
                    );
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                }
            }
            Some((id, action, error)) => {
                error!(
                    "PROBE_AUCTION: FAIL (5 sell) — the only command result seen answers \
                         action={} ({action}) error={} ({error}), auction id {id} — not our \
                         STARTED",
                    action_name(action),
                    error_name(error)
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
            None if now - since > ACTION_TIMEOUT_SECS => {
                error!(
                    "PROBE_AUCTION: FAIL (5 sell) — no SMSG_AUCTION_COMMAND_RESULT at all \
                         within {ACTION_TIMEOUT_SECS}s of StartAuction: either the packet never \
                         went out (the drain resolves the sell slot to a wire item guid at send \
                         time and drops a zero silently) or bid/etime were zero"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
            None => {}
        },
        Phase::SellMoney {
            since,
            money_done,
            slot_done,
        } => {
            let mut money_done = money_done;
            let mut slot_done = slot_done;
            if !money_done {
                let money = store.0.player_money().unwrap_or(0);
                let spent = i64::from(probe.baseline_money) - i64::from(money);
                let deposit = probe.deposit;
                if spent == deposit && deposit > 0 {
                    info!(
                        "PROBE_AUCTION: PASS (5b deposit) — purse {} -> {money} c, down exactly \
                         the {deposit} c the client quoted",
                        probe.baseline_money
                    );
                    probe.passes += 1;
                    money_done = true;
                } else if now - since > ACTION_TIMEOUT_SECS {
                    if deposit == 0 {
                        warn!(
                            "PROBE_AUCTION: SKIP (5b deposit) — the client quoted a ZERO deposit \
                             for this stack (value {}c), so there is nothing to observe in the \
                             purse (which moved by {spent}c)",
                            probe.stack_value
                        );
                    } else {
                        error!(
                            "PROBE_AUCTION: FAIL (5b deposit) — purse {} -> {money} c is a \
                             {spent} c charge against the {deposit} c the client quoted over a \
                             stack worth {}c. At {DURATION_MINUTES} minutes the client's \
                             arithmetic and vmangos's agree exactly, so a gap here is a real \
                             disagreement about the rate or the stack value",
                            probe.baseline_money, probe.stack_value
                        );
                        probe.fails += 1;
                    }
                    money_done = true;
                }
            }
            // With the item gone from the bag, the pane must not still show it.
            if !slot_done {
                let occupied = script
                    .eval::<bool>("return GetAuctionSellItemInfo() ~= nil")
                    .unwrap_or(false);
                if !occupied {
                    info!("PROBE_AUCTION: PASS (5c sell-slot) — the sell slot emptied once the auction was away");
                    probe.passes += 1;
                    slot_done = true;
                } else if now - since > SELL_SLOT_GRACE_SECS {
                    error!(
                        "PROBE_AUCTION: FAIL (5c sell-slot) — the auction is away and the item \
                         has left the bag, but GetAuctionSellItemInfo() still reads one. \
                         `clear_auction_sell_item` is documented as called \"once the auction is \
                         away, and on session close\" and in fact only ever runs on CLOSE \
                         (`feed_auction`'s `closed` arm is its one caller), so the pane shows a \
                         phantom stack until the window is shut"
                    );
                    probe.fails += 1;
                    slot_done = true;
                }
            }
            probe.phase = if money_done && slot_done {
                Phase::OwnerList {
                    since: now,
                    last_ask: 0.0,
                    event_baseline: probe.owned_event_baseline,
                }
            } else {
                Phase::SellMoney {
                    since,
                    money_done,
                    slot_done,
                }
            };
        }
        Phase::OwnerList {
            since,
            last_ask,
            event_baseline,
        } => {
            let Some(auction_id) = probe.auction_id else {
                probe.phase = Phase::Done;
                return;
            };
            if let Some(index) = owner_index_of(&auction, auction_id) {
                let entry = auction.lists[OWNER].entries[(index - 1) as usize];
                let (name, min_bid, buyout, count) = script
                    .eval::<(String, i64, i64, i64)>(&format!(
                        "local n, _, c, _, _, _, b, _, o = GetAuctionItemInfo(\"owner\", {index}) \
                         return n or \"\", b or -1, o or -1, c or -1"
                    ))
                    .unwrap_or_default();
                // The wire entry lands a frame before the feed pushes the snapshot, so Lua reads
                // an empty row until the VM agrees; wait for it.
                let lua_ready = min_bid == probe.min_bid && buyout == probe.buyout;
                if !lua_ready {
                    if now - since > OWNER_TIMEOUT_SECS {
                        error!(
                            "PROBE_AUCTION: FAIL (6 owner) — auction {auction_id} is on the wire \
                             at owner row {index} but the pushed snapshot never caught up within \
                             {OWNER_TIMEOUT_SECS}s: GetAuctionItemInfo reads {name:?} min_bid \
                             {min_bid} buyout {buyout} (wanted {} / {})",
                            probe.min_bid, probe.buyout,
                        );
                        probe.fails += 1;
                        probe.phase = Phase::Done;
                    }
                    return;
                }
                // The name may still be empty until the template cache fills it; the numbers
                // come off the wire and must agree.
                let right = entry.item_entry == ITEM_ENTRY
                    && i64::from(entry.start_bid) == probe.min_bid
                    && i64::from(entry.buyout) == probe.buyout;
                if right {
                    info!(
                        "PROBE_AUCTION: PASS (6 owner) — auction {auction_id} is owner row \
                         {index} of {}: {name:?} x{count}, min bid {min_bid}c, buyout {buyout}c",
                        auction.lists[OWNER].entries.len()
                    );
                    probe.passes += 1;
                } else {
                    error!(
                        "PROBE_AUCTION: FAIL (6 owner) — auction {auction_id} is listed but wrong: \
                         wire entry item={} (wanted {ITEM_ENTRY}) start_bid={} (wanted {}) \
                         buyout={} (wanted {}); Lua row reads {name:?} min_bid {min_bid} buyout \
                         {buyout}",
                        entry.item_entry,
                        entry.start_bid,
                        probe.min_bid,
                        entry.buyout,
                        probe.buyout,
                    );
                    probe.fails += 1;
                }
                owner_money_check(&script, &mut probe);
                started_chat_check(&script, &mut probe);
                probe.phase = Phase::OwnerEvent {
                    since: now,
                    event_baseline,
                };
                return;
            }
            if now - since > OWNER_TIMEOUT_SECS {
                error!(
                    "PROBE_AUCTION: FAIL (6 owner) — auction {auction_id} never appeared in the \
                     \"owner\" list within {OWNER_TIMEOUT_SECS}s ({} owner result(s) landed, {} \
                     row(s) held)",
                    auction.wire.list_results[OWNER],
                    auction.lists[OWNER].entries.len()
                );
                probe.fails += 1;
                probe.phase = cancel_at(now);
                return;
            }
            // Re-ask on a cadence. The first ask is the tab click: every stock caller of
            // `GetOwnerAuctionItems` is in the Auctions pane, and its `OnShow` sets the
            // `AuctionFrameAuctions.page` the repaint needs (`Blizzard_AuctionUI.lua:813-837`).
            if last_ask == 0.0 || now - last_ask > OWNER_REASK_SECS {
                let ask = if last_ask == 0.0 {
                    // The tab's OnShow queries once per window session (`gotAuctions`).
                    "AuctionFrameTab_OnClick(3)"
                } else {
                    "GetOwnerAuctionItems()"
                };
                crate::ui_script::run_or_warn(&script, ask);
                probe.phase = Phase::OwnerList {
                    since,
                    last_ask: now,
                    event_baseline,
                };
            }
        }
        Phase::OwnerEvent {
            since,
            event_baseline,
        } => {
            if event_count(&script, "AUCTION_OWNED_LIST_UPDATE") > event_baseline {
                info!("PROBE_AUCTION: PASS (6b owned-event) — AUCTION_OWNED_LIST_UPDATE fired for the new listing");
                probe.passes += 1;
            } else if now - since > EVENT_GRACE_SECS {
                error!(
                    "PROBE_AUCTION: FAIL (6b owned-event) — our listing is on the owner page but \
                     AUCTION_OWNED_LIST_UPDATE never fired within {EVENT_GRACE_SECS}s, so the \
                     Auctions tab would never repaint"
                );
                probe.fails += 1;
            } else {
                return;
            }
            probe.phase = cancel_at(now);
        }
        Phase::Cancel {
            since,
            sent,
            removed,
            last_ask,
        } => {
            let Some(auction_id) = probe.auction_id else {
                probe.phase = Phase::Done;
                return;
            };
            if !sent {
                let Some(index) = owner_index_of(&auction, auction_id) else {
                    if now - since > ACTION_TIMEOUT_SECS {
                        error!(
                            "PROBE_AUCTION: FAIL (7 cancel) — auction {auction_id} is not on the \
                             owner page, so there is no row to cancel; it is LEFT LISTED in the \
                             auction house"
                        );
                        probe.fails += 1;
                        probe.phase = Phase::Done;
                    }
                    return;
                };
                if let Err(e) = script.run(&format!("CancelAuction({index})")) {
                    error!("PROBE_AUCTION: FAIL (7 cancel) — CancelAuction({index}) errored: {e}");
                    probe.fails += 1;
                    probe.phase = Phase::Done;
                    return;
                }
                info!("PROBE_AUCTION: (7 cancel) CancelAuction(owner row {index}) sent for auction {auction_id}");
                probe.phase = Phase::Cancel {
                    since: now,
                    sent: true,
                    removed: false,
                    last_ask: 0.0,
                };
                return;
            }
            let mut removed = removed;
            if !removed {
                match auction.wire.last_command {
                    Some((id, action, error)) if action == auction_action::REMOVED => {
                        if error == auction_error::OK {
                            info!(
                                "PROBE_AUCTION: PASS (7 cancel) — SMSG_AUCTION_COMMAND_RESULT \
                                 action=REMOVED error=OK (auction id {id}); the stack comes back \
                                 by mail, which is vanilla's own behaviour"
                            );
                            probe.passes += 1;
                            removed = true;
                        } else {
                            error!(
                                "PROBE_AUCTION: FAIL (7 cancel) — action=REMOVED error={} \
                                 ({error}); surfaced line: {:?}. The auction is LEFT LISTED",
                                error_name(error),
                                last_error(&script)
                            );
                            probe.fails += 1;
                            probe.phase = Phase::Done;
                            return;
                        }
                    }
                    _ if now - since > ACTION_TIMEOUT_SECS => {
                        error!(
                            "PROBE_AUCTION: FAIL (7 cancel) — no REMOVED result within \
                             {ACTION_TIMEOUT_SECS}s (last result seen: {:?}). The auction is LEFT \
                             LISTED",
                            auction.wire.last_command
                        );
                        probe.fails += 1;
                        probe.phase = Phase::Done;
                        return;
                    }
                    _ => return,
                }
            }
            // The row must also leave the page the window shows.
            if owner_index_of(&auction, auction_id).is_none() {
                info!("PROBE_AUCTION: PASS (7b cancel-list) — auction {auction_id} is gone from the \"owner\" list");
                probe.passes += 1;
                probe.phase = Phase::Done;
                return;
            }
            if now - since > OWNER_TIMEOUT_SECS {
                error!(
                    "PROBE_AUCTION: FAIL (7b cancel-list) — the server said REMOVED but auction \
                     {auction_id} is still on the owner page {OWNER_TIMEOUT_SECS}s later"
                );
                probe.fails += 1;
                probe.phase = Phase::Done;
                return;
            }
            // Re-ask on the cadence; writing `removed` back keeps its verdict from re-printing.
            let ask = last_ask == 0.0 || now - last_ask > OWNER_REASK_SECS;
            if ask {
                crate::ui_script::run_or_warn(&script, "GetOwnerAuctionItems()");
            }
            probe.phase = Phase::Cancel {
                since,
                sent: true,
                removed,
                last_ask: if ask { now } else { last_ask },
            };
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            info!(
                "PROBE_AUCTION: DONE pass={} fail={}",
                probe.passes, probe.fails
            );
            // AppExit plus a hard-exit backstop, so a teardown hang cannot keep the account held.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_AUCTION: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
