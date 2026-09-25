//! The auction-arc live probe (`WOW_PROBE_AUCTION=1`) — decision 1511's end-to-end instrument:
//! GM-hop to a real Stormwind auctioneer, greet it on the wire, and drive browse / throttle /
//! sell / owner-list / cancel through the **live Lua VM** exactly as a click would, printing a
//! `PROBE_AUCTION:` trace line with a PASS/FAIL/SKIP verdict per step and a final
//! `PROBE_AUCTION: DONE pass=<n> fail=<m>` summary. Modeled closely on [`super::probe_mail`]
//! (same phase-machine shape, same trace style, same self-terminating exit).
//!
//! ## The one assertion no unit test can make
//!
//! Step (2). **The window is opened by the server, not by the click** ([`crate::ui_auction`]):
//! `MSG_AUCTION_HELLO` goes out and nothing happens; the *reply* — the same opcode coming back
//! with the auctioneer guid and an `AuctionHouse.dbc` house id — opens the session and fires
//! `AUCTION_HOUSE_SHOW`. Every other piece of this arc is downstream of that reply arriving, so it
//! is the step this probe exists for. The house id is checked to be in `1..=7` because it keys the
//! deposit rate the sell pane quotes, and a zero would silently quote a free listing.
//!
//! ## The auctioneer (live-DB verified against the local vmangos, `mangos`)
//!
//! Auctioneer Fitch, creature entry 8719, spawn guid 12696, map 0 (Stormwind, Trade District),
//! pos `(-8821.53, 659.886, 97.4645)`. Her `creature_template.npc_flags` is **4096** exactly —
//! bit 12 only (`UNIT_NPC_FLAG_AUCTIONEER`), no gossip bit — so the right-click route sends
//! `AuctionHello` directly with no gossip pre-empt (`target::click::interact_command`'s own note
//! on the service ladder), which is precisely the command this probe sends.
//!
//! ## What the server does that shapes the phases (VERIFIED, vmangos `AuctionHouseHandler.cpp`)
//!
//! - **One AH list request at a time.** `HandleAuctionListItems`/`ListOwnerItems`/`ListBidderItems`
//!   all open with `if (ReceivedAHListRequest()) return;` — a second query while one is in flight
//!   is dropped *silently*. Every list phase here therefore waits for its answer before asking
//!   again, and the owner phase re-asks on a timer rather than spamming.
//! - **`etime` is minutes on the wire**, converted server-side (`etime * MINUTE`) and switched
//!   against 1/4/12 × `MIN_AUCTION_TIME` (2 h). 120 is the shortest legal listing.
//! - **The deposit** is `uint32(SellPrice × count × (etime / MIN_AUCTION_TIME) × depositPercent /
//!   100)` (`AuctionHouseMgr::GetAuctionDeposit`), with `Auction.Deposit.Min = 0` and
//!   `Rate.Auction.Deposit = 1` in this deploy's `mangosd.conf`. At **120 minutes the unit count
//!   is 1**, which is exactly where the client's own arithmetic (`CalculateAuctionDeposit`, whose
//!   intermediate truncation disagrees with the server on longer listings) and
//!   the server's agree to the copper. That is why step (5) lists for 120 minutes: it makes "money
//!   fell by exactly the quoted deposit" a real assertion instead of a flaky one.
//! - **A cancel with no bidder is free** and returns the item **by mail**, not to the bag
//!   (`HandleAuctionRemoveItem`) — so a clean run leaves the auction house as it found it and one
//!   letter in the probe's own mailbox. Vanilla's behaviour, not litter we could avoid.
//! - `GM.AllowTrades = 1` in this deploy, so the gmlevel-6 probe accounts are not refused with
//!   `AUCTION_ERR_RESTRICTED_ACCOUNT` (the guard at the top of `HandleAuctionSellItem`).
//!
//! ## The row-identity rule this probe obeys
//!
//! [`super::probe_mail`]'s header records the defect worth not repeating: a probe that re-finds
//! its row **by predicate** each tick reads its own success as absence, because a successful
//! action is exactly what stops the row matching. So the auction created in step (5) is
//! remembered by the **auction id the server's own `STARTED` result carried**, once, and every
//! later step tracks that id — never "the row whose item is Linen Cloth".
//!
//! ## The run recipe
//!
//! ```text
//! WOW_PROBE_AUCTION=1 WOW_NOSOUND=1 WOW_USER=probe5 WOW_PASS=pprobe5 WOW_CHAR=Probefive \
//!     cargo run -p benilla
//! ```
//! (the checkout's probe identity — `.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR; the `probe`
//! skill).
//! Non-combat. An outer grep on `PROBE_AUCTION:` is the whole harness; the probe self-exits (the
//! [`super::probes::ProbeExitPlugin`] pattern) once DONE.

use bevy::prelude::*;

use benilla_protocol::messages::{auction_action, auction_duration, auction_error};
use benilla_protocol::EntityKind;
use benilla_ui::script::{UiScript, LIST, OWNER};

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_auction::AuctionOpen;

/// Auctioneer Fitch's spawn (vmangos `creature` guid 12696, entry 8719, map 0) — the `.go xyz`
/// target; the auctioneer itself is then scanned out of the streamed world by its npc flag, never
/// by a hardcoded guid (the taxi/bank probes' idiom).
const AUCTIONEER_AT: [f32; 3] = [-8821.53, 659.886, 97.4645];
/// Her creature template entry — reported, not required (the flag below is the identity test).
const AUCTIONEER_ENTRY: u32 = 8719;
/// `UNIT_NPC_FLAG_AUCTIONEER` (bit 12) — the same bit the cursor classifier and the click router
/// key on (`target::cursor_mode::npc_flags::AUCTIONEER`).
const NPC_FLAG_AUCTIONEER: u32 = 0x1000;
/// How wide the streamed world is searched for an auctioneer. Generous, so a slightly-off `.go`
/// landing still finds one — but the NEAREST match is the one greeted, never the first the ECS
/// happens to yield.
///
/// **That distinction cost this probe its first run.** Stormwind's Trade District holds several
/// auctioneers a few yards apart; a plain `find` picked one 7.0 yd away and the greeting came back
/// as *nothing at all*, because the server's `CanInteractWithNPC` ends in
/// `IsWithinDistInMap(this, INTERACTION_DISTANCE)` and refuses silently — the exact failure mode
/// step (2) exists to detect, arrived at by our own aim rather than by a defect.
const SCAN_RANGE: f32 = 12.0;
/// vmangos `INTERACTION_DISTANCE` (`ObjectDefines.h:24`) — 5.0 yd between centres, plus each
/// side's bounding radius. Anything past it is refused with no packet, so the probe would rather
/// say so than send a greeting it knows will be ignored.
const INTERACT_MAX_YD: f32 = 5.0;

/// The fixture listed in step (5): Linen Cloth — cheap, stackable, no durability/bind/equip
/// complications, and a `sell_price` of 13 c (live-DB verified), so a stack of five has a
/// nonzero deposit. A ZERO deposit would make the money assertion vacuous.
const ITEM_ENTRY: u32 = 2589;
const ITEM_COUNT: u32 = 5;
/// The listing length. 120 minutes is the one duration where the client's deposit arithmetic and
/// the server's agree exactly (module doc).
const DURATION_MINUTES: u32 = auction_duration::SHORT_MINUTES;

const SETTLE_SECS: f64 = 3.0;
const SCAN_TIMEOUT_SECS: f64 = 15.0;
const HELLO_TIMEOUT_SECS: f64 = 10.0;
const LIST_TIMEOUT_SECS: f64 = 15.0;
const ACTION_TIMEOUT_SECS: f64 = 10.0;
const ITEM_TIMEOUT_SECS: f64 = 10.0;
/// How long the refusal is watched before it is believed — the throttle drops the query with no
/// event at all, so the only honest reading is "nothing went out, and nothing came back, for this
/// long" (`AuctionWireLog`'s own reason for existing).
const REFUSAL_WATCH_SECS: f64 = 2.0;
/// The client throttle is 5 s; give the recovery a generous but bounded window.
const RECOVER_TIMEOUT_SECS: f64 = 12.0;
/// The owner list is re-asked on this cadence (the server drops a second in-flight list request
/// silently, so a re-ask has to be spaced, not spammed).
const OWNER_REASK_SECS: f64 = 3.0;
const OWNER_TIMEOUT_SECS: f64 = 20.0;
/// How long a UI EVENT is given to trail the state change it answers. Nothing orders this probe
/// against `feed_auction`, which is what fires them, so an event can legitimately land a frame
/// after the wire state it reports — but not ninety of them.
const EVENT_GRACE_SECS: f64 = 1.5;
/// How long the sell slot is given to empty itself after the listing is away, before (5c) calls
/// it. Generous: it is a one-line clear, not a round trip.
const SELL_SLOT_GRACE_SECS: f64 = 3.0;

pub(crate) struct ProbeAuctionPlugin;

impl Plugin for ProbeAuctionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuctionProbe>()
            .add_systems(Update, auction_probe);
    }
}

/// The probe's phase machine + the identities it discovers along the way (kept resource-level, not
/// per-variant, since several later phases re-resolve the same auction by its stable id — the
/// module doc's row-identity rule).
#[derive(Resource, Default)]
struct AuctionProbe {
    phase: Phase,
    /// The auctioneer's guid, once streamed in.
    auctioneer: Option<u64>,
    /// **The** handle: the auction id the server's own `STARTED` result carried. Everything after
    /// step (5) tracks this, never a predicate over the rows.
    auction_id: Option<u32>,
    /// The vendor value of the stack in the sell slot — what both deposits are computed from.
    stack_value: i64,
    /// `CalculateAuctionDeposit(120)` — the client's own quote, which step (5) then holds the
    /// server's charge against.
    deposit: i64,
    min_bid: i64,
    buyout: i64,
    /// The purse immediately before `StartAuction` went in.
    baseline_money: u32,
    /// `AUCTION_OWNED_LIST_UPDATE`'s count immediately before `StartAuction` went in.
    ///
    /// Taken THERE and not when step (6) starts, so that an owner re-query queued by the STARTED
    /// result cannot fire the event before the baseline is read — a baseline taken after it would
    /// wait for a second event that a repeated identical page will never produce. (Since 2308 that
    /// re-query only happens for a list we already hold, and step 6 is what first asks for this
    /// one, so the ordering is belt and braces rather than load-bearing.)
    owned_event_baseline: i64,
    /// How many red `UI_ERROR_MESSAGE` lines the run had already raised when the auction window
    /// opened — the login environment's own, not the arc's.
    ///
    /// Step (6d) asserts the arc raised **no** red line, and it used to read the whole tally: on a
    /// `probeN` account that is never zero, because the preflight's `GM mode is ON` and
    /// `GOD mode is ON` banners are red lines too, and they land before the greeting. The
    /// assertion was therefore FAILING on every clean run, which is worse than not making it.
    red_baseline: i64,
    passes: u32,
    fails: u32,
    /// Latched once [`Phase::Done`] has fired its exit (never re-fire on a later frame).
    exited: bool,
}

/// Every field is `Copy` — the phase is snapshotted out of the resource each tick
/// ([`auction_probe`]'s `let phase = probe.phase;`), which frees the match arms to mutate `probe`
/// without fighting the borrow checker over `probe.phase`; an arm that wants to "keep waiting"
/// simply never writes it.
#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// `.go` + `.additem` issued; settling before the world streams the auctioneer in (step 1).
    Settling {
        sent_at: f64,
    },
    /// `AuctionHello` sent; waiting for the **reply** to open the session (step 2).
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
    /// The second query, which the client throttle must refuse outright (step 4a).
    Refuse {
        since: f64,
        sent: bool,
        sent_baseline: u32,
        results_baseline: u32,
    },
    /// Waiting for `CanSendAuctionQuery()` to come back, then proving it with a real third query
    /// that actually goes out (step 4b).
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
    /// Picked up and dropped in the sell slot; waiting for the slot to read back a priced item
    /// (step 5 prep).
    Attach {
        since: f64,
    },
    /// `StartAuction` away; waiting for `SMSG_AUCTION_COMMAND_RESULT` (step 5).
    Sell {
        since: f64,
    },
    /// The money assertion (which trails the verdict by however long the descriptor takes) and
    /// the sell-slot one. Each is LATCHED, because they land at different moments and neither may
    /// re-print while the other is still waiting.
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

/// One Lua-side event counter (`ProbeAuctionEvents`), or `0` on any eval hiccup — treated as
/// "nothing observed yet", never a panic.
fn event_count(script: &UiScript, event: &str) -> i64 {
    script
        .eval::<i64>(&format!(
            "return (ProbeAuctionEvents or {{}})['{event}'] or 0"
        ))
        .unwrap_or(0)
}

/// The newest `UI_ERROR_MESSAGE` text the hook logged (the arc's failure channel — a refused
/// auction command surfaces here and nowhere else).
fn last_error(script: &UiScript) -> String {
    script
        .eval::<String>("return ProbeAuctionErrors[table.getn(ProbeAuctionErrors)] or \"\"")
        .unwrap_or_default()
}

/// (6d) Where the STARTED verdict was SAID — the live half of the director's 2026-08-22 report that
/// an auction outcome arrived as a red centre-screen line.
///
/// Two halves, and the second is the one that would have caught the bug: the "Auction created." line
/// has to be in the **chat** log (catalog row `0x178`, kind 0 → `CHAT_MSG_SYSTEM`), and a clean run
/// has to have raised **no red line at all**. Asserting only the first would still pass if we
/// printed it in both places.
///
/// Compared against `getglobal("ERR_AUCTION_STARTED")` rather than against English: the string is
/// the player's own, and this file must not carry a copy of it.
/// Red `UI_ERROR_MESSAGE` lines raised so far, as the hook's own array counts them.
fn red_count(script: &UiScript) -> i64 {
    script
        .eval::<i64>("return table.getn(ProbeAuctionErrors)")
        .unwrap_or(-1)
}

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
            // The reds are listed from the baseline on, for the same reason `reds` counts from
            // there: the login environment's own banners are not this arc's output.
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

/// (6c) The owner row's money frame, read out of the live VM — the live half of the director's
/// 2026-08-22 report that the price columns were dropping their zeros.
///
/// The law under test is `MoneyTypeInfo["AUCTION"]`'s `showSmallerCoins`: it collapses only the
/// **leading** zero denominations, so a 1-silver minimum bid reads `1s 0c` and not a lone silver
/// coin. Derived from `probe.min_bid` rather than hardcoded, because the listing price is computed
/// from the item's own vendor value and a fixed expectation would be a lie the day the item changes.
///
/// `IsShown`, not `IsVisible`: the coin buttons' own shown-ness is the subject, not whatever the
/// window is doing around them.
///
/// The pane is the *front* tab by the time this runs, and that is load-bearing rather than
/// incidental: the row is painted by `AuctionFrameAuctions_Update`, which cannot complete before
/// the tab's `OnShow` has given it a `page`. This check read the player's purse
/// out of the row's money frame for as long as the probe asked for the owned list without opening
/// the tab — the frame was still on its `MoneyTypeInfo["PLAYER"]` default because nothing had ever
/// repainted it.
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

/// The 1-based **display** index our auction currently sits at in the owner list, or `None` if it
/// is not on the page. Read off the app-side page rather than by predicate: `CancelAuction(index)`
/// is mapped back through the same sorted view the feed pushed, and with no header ever clicked
/// that view is the wire order (`ui_auction::sort`: an empty stack leaves the order alone).
fn owner_index_of(auction: &AuctionOpen, auction_id: u32) -> Option<u32> {
    auction.lists[OWNER]
        .entries
        .iter()
        .position(|e| e.auction_id == auction_id)
        .map(|i| i as u32 + 1)
}

/// Step (5)'s entry — the next phase every step-4 exit, pass or fail, funnels into.
fn ensure_item(now: f64) -> Phase {
    Phase::EnsureItem {
        since: now,
        sent: false,
    }
}

/// Step (7)'s entry. Reached from step (6) either way: a listing we created is cancelled even
/// when the assertion about it failed, because leaving it is the one thing this probe must not do.
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
        return; // no UI VM this build (headless net-only) — nothing this probe can drive
    };
    let now = time.elapsed_secs_f64();
    // A cheap `Copy` snapshot (see [`Phase`]'s doc).
    let phase = probe.phase;

    match phase {
        Phase::Wait => {
            // The observation channel: one hidden frame counting every event this arc fires, plus
            // a log of the `UI_ERROR_MESSAGE` texts (the mail probe's `ProbeMailEvents` idiom,
            // widened to a per-event tally because this arc fires six different events and the
            // question is always "did THIS one fire").
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
            // The teleport (the `ProbeChatPlugin`/`probe_taxi` idiom: GM dot-commands ride as
            // plain Say lines). The sell fixture is NOT stocked here: step (5) looks in the bags
            // first and only `.additem`s when there is nothing to reuse, because a cancelled
            // auction returns its stack by MAIL and an unconditional grant would mint five more
            // linen into the world on every run.
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
            // The NEAREST auctioneer, not the first one the ECS yields — see [`SCAN_RANGE`].
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
                // The exact command a right-click on a pure auctioneer builds
                // (`target::click::interact_command`, CursorKind::Buy + bit 12). It opens
                // NOTHING on its own — the reply does.
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
                    // Everything red before this instant is the login environment (the GM/GOD
                    // banners); the arc's own tally starts here.
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
                // The Browse pane's own call, with every filter at its default: empty name, empty
                // level boxes (the pane hands strings), no class/subclass/invtype, page 0, not
                // usable-only, quality ALL (-1).
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
            // The bag scan the bag UI itself would do — encoded bag*100+slot, -1 = not there
            // (the mail probe's own `GetContainerItemLink` idiom).
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
                // Pick it up and drop it in the sell slot — the two calls the sell button's own
                // OnClick chain makes (`AuctionsItemButton_OnClick` → ClickAuctionSellItemButton).
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
            // Six values ALWAYS; the first is nil for an empty slot. `price` is the stack's
            // vendor value, which is what both deposits are computed from — so wait for it to be
            // nonzero rather than quoting a deposit off an item template still in flight.
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
                // The sell pane's own suggested opening price (`AuctionSellItemButton_OnEvent`):
                // half again the stack's vendor value, floored at one silver.
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
            // The slot itself: with the item gone from the bag, the pane must not still be
            // showing it.
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
                // The WIRE side lands a frame before the feed pushes the snapshot, so reading Lua
                // the moment the entry appears gets the twelve-value null tail — zeros and an
                // empty name — for a row that genuinely exists. Wait for the VM to agree rather
                // than printing that as if it were the row's content: a PASS whose own message
                // shows empty data is the "probe that lies" this file's header warns about, and it
                // is the reason the Lua read is part of the assertion below and not decoration.
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
                // The item's NAME is allowed to still be empty — it fills in from the ask-once
                // template cache and the window shows a placeholder until it does. The numbers
                // come straight off the wire and must agree on both sides.
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
            // Re-ask on a cadence: the server drops a second in-flight list request silently.
            //
            // **The FIRST ask is the tab click, not the raw binding**. Every
            // caller of `GetOwnerAuctionItems` in the stock files lives inside the Auctions pane —
            // its `OnShow` and its two page turners — and the `OnShow` is also the one place that
            // assigns `AuctionFrameAuctions.page`. Calling the binding cold puts the window in a
            // state no click can reach, and the reply's `AUCTION_OWNED_LIST_UPDATE` then repaints
            // a tab whose `page` is still nil: this probe raised
            // `Blizzard_AuctionUI.lua:837: attempt to perform arithmetic on field 'page'` twice a
            // run doing exactly that, at WARN, where nothing was looking. Clicking the tab is both
            // the faithful drive ("exactly as a click would", this file's header) and what makes
            // step (6c) a real assertion instead of a permanently red one.
            if last_ask == 0.0 || now - last_ask > OWNER_REASK_SECS {
                let ask = if last_ask == 0.0 {
                    // The tab's own OnShow issues the query, once per window session
                    // (`AuctionFrame.gotAuctions`), and sets the page the repaint needs.
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
            // ...and it has to actually leave the page the window is showing.
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
            // The re-ask cadence, plus the write-back that keeps `removed` from re-printing its
            // verdict every tick.
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
            // The probe self-exit pattern (`ProbeExitPlugin::fire_probe_exit`): a polite AppExit
            // plus a hard backstop thread, so a net/winit teardown hang can't leave a zombie
            // client holding the probe account.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_AUCTION: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
