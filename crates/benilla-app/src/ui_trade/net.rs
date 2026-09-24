//! The player trade's packet handlers (decision 0592 P1; in the net handler table since 2306) —
//! the status packet drives the open/accept/close state machine, the extended snapshot replaces
//! one side's item/gold, both into the [`TradeSession`] the feed reads. The `UiScript` events
//! these ultimately drive are fired by [`super::feed_trade`] (the feed owns the VM), so the
//! handlers only mutate the session + send the auto-reply.

use benilla_protocol::messages::{TradeStatus, TradeStatusExtended};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{NamedLine, TradeSession};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::ui_action::{UiError, UiErrorKeys};

/// Register the trade's handlers — called from [`super::UiTradePlugin`]. One per kind, plus the
/// session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TradeStatus, on_trade_status)
        .net_handler(K::TradeStatusExtended, on_trade_status_extended)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_trade_status(
    In(ev): In<SessionEvent>,
    mut trade: ResMut<TradeSession>,
    mut errors: ResMut<UiErrorKeys>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::TradeStatus { status } = ev {
        trade_status(status, &mut trade, &mut errors, &commands);
    }
}

fn on_trade_status_extended(In(ev): In<SessionEvent>, mut trade: ResMut<TradeSession>) {
    if let SessionEvent::TradeStatusExtended { state } = ev {
        trade_status_extended(&state, &mut trade);
    }
}

/// An open trade dies with the socket (decision 0592) — the reconnect starts with no trade. A
/// listener on the session end (a second handler on `Disconnected`, after the bridge's own
/// teardown, `net::session::on_disconnected`).
fn on_session_end(In(_): In<SessionEvent>, mut trade: ResMut<TradeSession>) {
    trade.clear_session();
}

/// `SessionEvent::TradeStatus` (`SMSG_TRADE_STATUS`) — benilla's face of the reference's own
/// 23-case dispatcher `CGTradeInfo 0x4bf720` (`ecx` = the status code; jump table `0x4bfa08`;
/// sole caller `0x5d4923`, inside the `SMSG_TRADE_STATUS` handler `0x5d47c0`). Every arm below is
/// that function's, walked.
///
/// **Three different things a status can do, and they are not the same set.**
///
/// 1. **Print.** Eighteen of the twenty-three arms raise a `CGGameUI::DisplayError` message —
///    [`status_message`] carries sixteen of them, `BEGIN_TRADE`'s belongs to the ladder that
///    answers it, and `CLOSE_WINDOW`'s is the one deliberate gap (the table's own doc says why).
///    The arc raised four before this, which is the report it answers.
/// 2. **Close the window** (`0x4bf4e0(0, 0)` → `TRADE_CLOSED`). **Three** arms do: `CANCELED`,
///    `COMPLETE`, `CLOSE_WINDOW`. Two more touch the window without closing it (`OPEN_WINDOW`
///    opens it, `ONLY_CONJURED` bounces one slot out of our offer).
/// 3. **Clear the network cells** — the outbound-initiate latch and the pending guid — which is
///    the *handler's* common tail (`0x5d490a`), not the arm's, and runs on every code except
///    `{1, 2, 4, 7, 9, 22}`. [`TradeSession::clear_pending`].
///
/// benilla had (2) and (3) fused: seventeen statuses called a full window close. On vmangos that
/// is unobservable — every code in (3) that is not in (2) arrives as an *initiate* refusal, with
/// no window open — but it made the code claim `TARGET_STUNNED` tears down a trade window, and it
/// got `REJECTED` and `UNKNOWN_13` wrong in opposite directions (decision 1764 wrote both down as
/// unsettled; their arms, `0x4bf821` and `0x4bfa02`, settle them).
///
/// `BEGIN_TRADE` records the incoming request **without answering it** (decision 1764 — the reply
/// is a ladder of eight gates, so [`super::answer_trade_request`] owns it, and it is the
/// only arm of the reference's dispatcher that speaks to the wire).
fn trade_status(
    status: TradeStatus,
    trade: &mut TradeSession,
    errors: &mut UiErrorKeys,
    commands: &NetCommands,
) {
    bevy::log::info!(target: "trade", "SMSG_TRADE_STATUS {status:?}");

    // ── The message, first, because the arms that name a player must read the guid cells BEFORE
    //    the tail clears them — which is exactly the reference's order (`0x5d4923` calls the
    //    dispatcher, `0x5d4931` clears). A `%s` line is parked for the feed to name; the rest go
    //    straight out, since nothing about them can block. ──────────────────────────────────────
    match status_message(status) {
        Some(Line::Plain(key)) => errors.0.push(UiError::key(key)),
        Some(Line::Naming(key)) => {
            if let Some(who) = trade.line_names() {
                trade.owe_named_line(NamedLine {
                    key,
                    who,
                    // These arms read the name off the live `CGUnit` and print nothing when the
                    // object is gone — the reference's second guard, which the feed applies where
                    // it resolves the name.
                    needs_live_object: true,
                });
            }
        }
        None => {}
    }

    match status {
        TradeStatus::BeginTrade { partner } => trade.request(partner),
        TradeStatus::OpenWindow => trade.open_window(),
        // Case 4 (`0x4bf80b`) sets the PARTNER's accept flag; case 9 (`0x4bf821`) clears it.
        // Neither touches ours, neither closes anything, and neither is in the tail's clear set.
        TradeStatus::Accept => trade.partner_accepted(),
        TradeStatus::Rejected => trade.partner_unaccepted(),
        // Case 7 (`0x4bf81a`) resets BOTH accept flags — and the "mine" half is `0x4bf230(0)`,
        // which is idempotent and, when it really had to change the flag, **also sends
        // `CMSG_UNACCEPT_TRADE`**. So the reference answers its own bounced accept on the wire,
        // and vmangos's `HandleUnacceptTradeOpcode` → `SetAccepted(false, crosssend)` passes a
        // `BACK_TO_TRADE` on to the partner. benilla dropped the glow locally and said nothing.
        TradeStatus::BackToTrade => {
            if trade.we_accepted() {
                let _ = commands.0.send(ClientCommand::UnacceptTrade);
            }
            trade.back_to_trade();
        }
        // The three that really do tear the window down. `CANCELED` also signals
        // `TRADE_REQUEST_CANCEL` (`0x4bf832`) — the ONE Lua event this whole dispatcher fires
        // (there is no other `SignalEvent` in `[0x4bf720, 0x4bfa08)`) — and it signals it BEFORE
        // the close, which is why the flag survives [`TradeSession::close_window`].
        //
        // Each of the three also sends `CMSG_CANCEL_TRADE` from inside `0x4bf4e0`. benilla does
        // not send it here and does not need to: the window is the stock `TradeFrame.xml`, whose
        // `OnHide` calls `CloseTrade()` — so the reference sends that packet TWICE on a
        // server-driven close (once from the arm, once from the frame) and benilla sends the
        // second one already, through the same stock file. vmangos's `HandleCancelTradeOpcode`
        // no-ops once `m_trade` is gone, so the first is a redundancy, not a behaviour.
        TradeStatus::Canceled => {
            trade.signal_request_cancel();
            trade.close_window();
        }
        TradeStatus::Complete | TradeStatus::CloseWindow { .. } => trade.close_window(),
        // The placement is bounced, not the window (case 22, `0x4bf9ec` → `0x4bfbd0`).
        TradeStatus::OnlyConjured { slot } => trade.bounce_own_offer(slot),
        // Everything else: the handler's tail clear, and nothing else. These are the initiate
        // refusals — each has already queued its line above.
        TradeStatus::Busy
        | TradeStatus::Busy2
        | TradeStatus::NoTarget
        | TradeStatus::TargetTooFar
        | TradeStatus::WrongFaction
        | TradeStatus::Unknown13
        | TradeStatus::IgnoreYou
        | TradeStatus::YouStunned
        | TradeStatus::TargetStunned
        | TradeStatus::YouDead
        | TradeStatus::TargetDead
        | TradeStatus::YouLogout
        | TradeStatus::TargetLogout
        | TradeStatus::TrialAccount => trade.clear_pending(),
        // A code outside `0..=0x16` takes the dispatcher's own `ja` to the bare epilogue
        // `0x4bfa02` — the same target case 13 has. vmangos never sends one.
        TradeStatus::Unknown(_) => {}
    }
}

/// A message a status arm raises: either a plain key, or one whose `%s` names a player and so has
/// to wait for [`crate::names::NameCache`].
enum Line {
    Plain(&'static str),
    Naming(&'static str),
}

/// **What each status arm prints** — the `GlobalStrings` key, or `None`.
///
/// Five arms genuinely say nothing (`OPEN_WINDOW`, `TRADE_ACCEPT`, `BACK_TO_TRADE`, `REJECTED`,
/// `UNKNOWN_13`); the other eighteen print. Two of those eighteen answer `None` *here* for reasons
/// of their own: `BEGIN_TRADE`'s line is the ladder's
/// ([`super::answer_trade_request`]'s leg 8), and `CLOSE_WINDOW`'s is the gap at the
/// bottom of this doc.
///
/// **Eleven of the ids are outside the trade block** `0xb9..=0xbf` — which is why the arc shipped
/// four of them and read as done: that block's own emit-site census answers "which arms print a
/// *trade-keyed* message", and that is a different question from "which arms print". The full walk
/// covers every arm of `0x4bf720`; the tell was in the reference's own interface, where
/// `ERR_TARGET_STUNNED`'s `GlobalStrings.lua` comment reads `-- Trade failure`.
///
/// | # | status | id | key | catalog kind |
/// |---|---|---|---|---|
/// | 0 | `BUSY` | `0x3e` | `ERR_PLAYER_BUSY_S` | `0` chat, **names the player** |
/// | 5 | `BUSY_2` | `0x3e` | `ERR_PLAYER_BUSY_S` | `0` chat, names |
/// | 3 | `CANCELED` | `0xbe` | `ERR_TRADE_CANCELLED` | `1` yellow |
/// | 6 | `NO_TARGET` | `0xb8` | `ERR_GENERIC_NO_TARGET` | `2` red |
/// | 8 | `COMPLETE` | `0xbf` | `ERR_TRADE_COMPLETE` | `1` yellow |
/// | 10 | `TARGET_TOO_FAR` | `0xbd` | `ERR_TRADE_TOO_FAR` | `2` red |
/// | 11 | `WRONG_FACTION` | `0xff` | `ERR_PLAYER_WRONG_FACTION` | `2` red |
/// | 14 | `IGNORE_YOU` | `0x13d` | `ERR_IGNORING_YOU_S` | `0` chat, names |
/// | 15 | `YOU_STUNNED` | `0x191` | `ERR_GENERIC_STUNNED` | `2` red |
/// | 16 | `TARGET_STUNNED` | `0x192` | `ERR_TARGET_STUNNED` | `2` red |
/// | 17 | `YOU_DEAD` | `0x7e` | `ERR_PLAYER_DEAD` | `2` red |
/// | 18 | `TARGET_DEAD` | `0xbc` | `ERR_TRADE_TARGET_DEAD` | `2` red |
/// | 19 | `YOU_LOGOUT` | `0x19d` | `ERR_LOGGING_OUT` | `2` red |
/// | 20 | `TARGET_LOGOUT` | `0x19e` | `ERR_TARGET_LOGGING_OUT` | `2` red |
/// | 21 | `TRIAL_ACCOUNT` | `0x1be` | `ERR_RESTRICTED_ACCOUNT` | `2` red |
/// | 22 | `ONLY_CONJURED` | `0x1ca` | `ERR_TRADE_WRONG_REALM` | `2` red |
///
/// Case 1's own line (`ERR_TRADE_BLOCKED_S`, leg 8 only) is
/// [`super::answer_trade_request`]'s, and case 12's is the gap below.
///
/// **Nothing here decides a surface.** Three of these are chat lines and thirteen are toasts,
/// split between yellow and red, and the split does not follow the English: the two *successful*
/// outcomes are the yellow ones. `ui_action::feed_actions` reads each row's `+0x04`.
/// `ERR_TRADE_WRONG_REALM`'s key is actively misleading — its text is the ONLY_CONJURED sentence.
///
/// **The one arm not built: `CLOSE_WINDOW` (12) prints a *computed* id**, not a fixed one —
/// `0x4bfd70` maps the packet's `result` word (our [`TradeStatus::CloseWindow::result`]) over a
/// **61-id** range, tail-jumping `0x622630` for everything it does not special-case, with `0x1d1`
/// as a suppression sentinel. vmangos never sends status 12 (`SharedDefines.h` defines it;
/// `TradeHandler.cpp` sends it nowhere), so there is no observable to build against — and a
/// self/other polarity inconsistency inside that mapper wants a live capture to settle.
/// Transcribing 61 rows nothing can reach, around a known-doubtful axis, is how a table
/// gets written wrong and believed; the window still closes on 12, it just says nothing.
fn status_message(status: TradeStatus) -> Option<Line> {
    Some(match status {
        TradeStatus::Busy | TradeStatus::Busy2 => Line::Naming("ERR_PLAYER_BUSY_S"),
        TradeStatus::IgnoreYou => Line::Naming("ERR_IGNORING_YOU_S"),
        TradeStatus::Canceled => Line::Plain("ERR_TRADE_CANCELLED"),
        TradeStatus::NoTarget => Line::Plain("ERR_GENERIC_NO_TARGET"),
        TradeStatus::Complete => Line::Plain("ERR_TRADE_COMPLETE"),
        TradeStatus::TargetTooFar => Line::Plain("ERR_TRADE_TOO_FAR"),
        TradeStatus::WrongFaction => Line::Plain("ERR_PLAYER_WRONG_FACTION"),
        TradeStatus::YouStunned => Line::Plain("ERR_GENERIC_STUNNED"),
        TradeStatus::TargetStunned => Line::Plain("ERR_TARGET_STUNNED"),
        TradeStatus::YouDead => Line::Plain("ERR_PLAYER_DEAD"),
        TradeStatus::TargetDead => Line::Plain("ERR_TRADE_TARGET_DEAD"),
        TradeStatus::YouLogout => Line::Plain("ERR_LOGGING_OUT"),
        TradeStatus::TargetLogout => Line::Plain("ERR_TARGET_LOGGING_OUT"),
        TradeStatus::TrialAccount => Line::Plain("ERR_RESTRICTED_ACCOUNT"),
        TradeStatus::OnlyConjured { .. } => Line::Plain("ERR_TRADE_WRONG_REALM"),
        // Silent: `OPEN_WINDOW`, `TRADE_ACCEPT`, `BACK_TO_TRADE`, `REJECTED`, `UNKNOWN_13` — and
        // `BEGIN_TRADE`, whose one line belongs to the ladder that answers it. `CLOSE_WINDOW`'s
        // computed id is the gap named above.
        TradeStatus::BeginTrade { .. }
        | TradeStatus::OpenWindow
        | TradeStatus::Accept
        | TradeStatus::BackToTrade
        | TradeStatus::Rejected
        | TradeStatus::CloseWindow { .. }
        | TradeStatus::Unknown13
        | TradeStatus::Unknown(_) => return None,
    })
}

/// `SessionEvent::TradeStatusExtended` (`SMSG_TRADE_STATUS_EXTENDED`) — replace one side's item/gold
/// snapshot (decision 0592 P1); the feed repaints (`TRADE_UPDATE`) on the change.
fn trade_status_extended(ext: &TradeStatusExtended, trade: &mut TradeSession) {
    bevy::log::info!(
        target: "trade",
        "SMSG_TRADE_STATUS_EXTENDED their_window={} gold={} items={}",
        ext.their_window,
        ext.gold,
        ext.slots.iter().filter(|s| s.is_some()).count(),
    );
    trade.set_offer(ext);
}

#[cfg(test)]
mod tests {
    use benilla_ui::messages::{kind_of, MsgKind};

    use super::*;

    /// Every status code, as the wire delivers it — the tail-carrying three with a payload.
    fn every_status() -> Vec<TradeStatus> {
        vec![
            TradeStatus::Busy,
            TradeStatus::BeginTrade { partner: 0x7 },
            TradeStatus::OpenWindow,
            TradeStatus::Canceled,
            TradeStatus::Accept,
            TradeStatus::Busy2,
            TradeStatus::NoTarget,
            TradeStatus::BackToTrade,
            TradeStatus::Complete,
            TradeStatus::Rejected,
            TradeStatus::TargetTooFar,
            TradeStatus::WrongFaction,
            TradeStatus::CloseWindow {
                result: 0,
                item_limit_category: 0,
            },
            TradeStatus::Unknown13,
            TradeStatus::IgnoreYou,
            TradeStatus::YouStunned,
            TradeStatus::TargetStunned,
            TradeStatus::YouDead,
            TradeStatus::TargetDead,
            TradeStatus::YouLogout,
            TradeStatus::TargetLogout,
            TradeStatus::TrialAccount,
            TradeStatus::OnlyConjured { slot: 2 },
        ]
    }

    /// A filled trade slot — only the fields the offer model reads matter here.
    fn item() -> benilla_protocol::messages::TradeItem {
        benilla_protocol::messages::TradeItem {
            entry: 1234,
            display_id: 1,
            count: 1,
            wrapped: false,
            gift_creator: 0,
            perm_enchant: 0,
            creator: 0,
            charges: 0,
            suffix_factor: 0,
            random_prop_id: 0,
            lock_id: 0,
            max_durability: 0,
            durability: 0,
        }
    }

    fn sink() -> (
        UiErrorKeys,
        NetCommands,
        crossbeam_channel::Receiver<ClientCommand>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (UiErrorKeys::default(), NetCommands(tx), rx)
    }

    /// A session mid-trade with a window up, both sides accepted — so a wrong close, a wrong
    /// accept reset or a wrong cell clear all have something to damage.
    fn open_trade() -> TradeSession {
        let mut trade = TradeSession::default();
        trade.initiate(0x7);
        trade.open_window();
        trade.accept();
        trade.partner_accepted();
        trade.take_named_lines_for_test();
        trade
    }

    /// End to end through the real registration: a status opens the window, and the session
    /// end — the bridge's own teardown beside this listener — takes the trade with it.
    #[test]
    fn the_table_routes_the_status_and_the_session_end_to_the_trade() {
        let (_, commands, _rx) = sink();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<TradeSession>()
            .init_resource::<UiErrorKeys>()
            .insert_resource(commands);
        register(&mut app);

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::TradeStatus {
                status: TradeStatus::OpenWindow,
            }],
        );
        assert!(app.world().resource::<TradeSession>().is_open());

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );
        assert!(!app.world().resource::<TradeSession>().is_open());
    }

    /// **Eighteen of the twenty-three arms print; this table carries sixteen.** The arc raised
    /// four, which is the report.
    ///
    /// Five arms are genuinely silent — the window opening, the two accept-flag pokes, the accept
    /// bounce, and `UNKNOWN_13`'s bare epilogue. Two more answer `None` here on purpose:
    /// `BEGIN_TRADE`'s line belongs to the ladder that answers it, and `CLOSE_WINDOW`'s is a
    /// computed 61-id mapper nothing on this server can reach. Pinning the whole set in one list
    /// is what stops a future edit quietly re-silencing one.
    #[test]
    fn sixteen_arms_print_here_and_the_seven_silent_ones_are_pinned() {
        let silent: Vec<u32> = every_status()
            .into_iter()
            .filter(|s| status_message(*s).is_none())
            .map(TradeStatus::code)
            .collect();
        assert_eq!(
            silent,
            vec![1, 2, 4, 7, 9, 12, 13],
            "silent: BEGIN_TRADE (the ladder's line is its own), OPEN_WINDOW, TRADE_ACCEPT, \
             BACK_TO_TRADE, REJECTED, CLOSE_WINDOW (a computed id we deliberately do not build) \
             and UNKNOWN_13"
        );
    }

    /// **Every key names a real catalog row, and the surface comes from the row.** The split is
    /// not guessable and not ours: the two *successful* outcomes are the yellow ones, two of the
    /// refusals are chat lines, and the rest are red.
    #[test]
    fn the_surfaces_are_the_catalogs_and_not_the_key_names() {
        for status in every_status() {
            let Some(line) = status_message(status) else {
                continue;
            };
            let key = match line {
                Line::Plain(k) | Line::Naming(k) => k,
            };
            assert!(
                benilla_ui::messages::by_key(key).is_some(),
                "{status:?} names {key}, which is not a catalog row"
            );
        }
        // The three that are NOT red toasts — the half a re-implementation gets wrong.
        assert_eq!(kind_of("ERR_PLAYER_BUSY_S"), MsgKind::Chat);
        assert_eq!(kind_of("ERR_IGNORING_YOU_S"), MsgKind::Chat);
        assert_eq!(kind_of("ERR_TRADE_CANCELLED"), MsgKind::Info);
        assert_eq!(kind_of("ERR_TRADE_COMPLETE"), MsgKind::Info);
        // …against a control from the same table.
        assert_eq!(kind_of("ERR_TRADE_TARGET_DEAD"), MsgKind::Error);
    }

    /// **Only three arms close the window** (`0x4bf4e0` has four call sites inside the dispatcher:
    /// one to open, three to close). benilla closed on seventeen — it was writing the *handler's*
    /// network-cell clear as a window teardown.
    #[test]
    fn only_canceled_complete_and_close_window_tear_the_window_down() {
        let closes: Vec<u32> = every_status()
            .into_iter()
            .filter(|status| {
                let (mut errors, commands, _rx) = sink();
                let mut trade = open_trade();
                trade_status(*status, &mut trade, &mut errors, &commands);
                !trade.is_open()
            })
            .map(TradeStatus::code)
            .collect();
        assert_eq!(closes, vec![3, 8, 12]);
    }

    /// The fourteen that clear the network cells and **leave the window alone** — the tail's set
    /// minus the three that also close. Each drops the outbound-initiate latch (which is what
    /// gates the `%s` lines) without touching the offer or the accepts.
    #[test]
    fn the_refusals_clear_the_latch_and_leave_the_window_standing() {
        for status in every_status() {
            let code = status.code();
            if !matches!(code, 0 | 5 | 6 | 10 | 11 | 13 | 14..=21) {
                continue;
            }
            let (mut errors, commands, _rx) = sink();
            let mut trade = open_trade();
            trade_status(status, &mut trade, &mut errors, &commands);
            assert!(trade.is_open(), "{status:?} must not close the window");
            assert!(
                trade.line_names().is_none(),
                "{status:?} must drop the initiate latch"
            );
            assert!(
                trade.we_accepted(),
                "{status:?} must not touch the accept glow"
            );
        }
    }

    /// `REJECTED` drops **the partner's** accept and nothing else — no close, no line, no cell
    /// clear. benilla closed the whole session; 1764 wrote the disagreement down rather than
    /// guessing, and the 23-arm walk settles it. vmangos never sends 9, so this is faithfulness
    /// with no observable, which is exactly when it is cheapest to be right.
    #[test]
    fn rejected_only_drops_the_partners_accept() {
        let (mut errors, commands, _rx) = sink();
        let mut trade = open_trade();
        trade_status(TradeStatus::Rejected, &mut trade, &mut errors, &commands);
        assert!(trade.is_open());
        assert!(trade.we_accepted(), "ours is untouched");
        assert!(!trade.partner_has_accepted(), "theirs is dropped");
        assert!(errors.0.is_empty(), "and it says nothing");
        assert!(trade.line_names().is_some(), "9 is not in the clear set");
    }

    /// **A bounced accept answers on the wire.** `0x4bf230(0)` is idempotent and, when it really
    /// changes the flag, tail-jumps the `CMSG_UNACCEPT_TRADE` sender — so the reference tells the
    /// server it withdrew, and vmangos passes a `BACK_TO_TRADE` on to the partner. benilla dropped
    /// the glow locally and sent nothing.
    #[test]
    fn back_to_trade_unaccepts_on_the_wire_but_only_if_we_had_accepted() {
        let (mut errors, commands, rx) = sink();
        let mut trade = open_trade(); // we accepted
        trade_status(TradeStatus::BackToTrade, &mut trade, &mut errors, &commands);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::UnacceptTrade)));
        assert!(!trade.we_accepted());
        assert!(errors.0.is_empty(), "the bounce is silent");

        // Idempotent: a second bounce with no accept up sends nothing.
        trade_status(TradeStatus::BackToTrade, &mut trade, &mut errors, &commands);
        assert!(rx.try_recv().is_err());
    }

    /// `ONLY_CONJURED` bounces the offending **slot**, not the window — and `0xff` names the money
    /// instead. Out-of-range is dropped: the reference indexes its 7-slot mirror with the wire
    /// byte unchecked, and reproducing that is not reproducing a behaviour.
    #[test]
    fn only_conjured_bounces_one_slot_and_leaves_the_window_up() {
        // Wire slot 1 is UI slot 2 — the one `place_own_item` fills below.
        for (slot, expect_gold, expect_slot2) in
            [(1u8, 700u32, false), (0xff, 0, true), (200, 700, true)]
        {
            let (mut errors, commands, _rx) = sink();
            let mut trade = open_trade();
            trade.place_own_item(2, item());
            trade.set_own_gold(700);
            trade_status(
                TradeStatus::OnlyConjured { slot },
                &mut trade,
                &mut errors,
                &commands,
            );
            assert!(trade.is_open(), "slot {slot}: the window stays up");
            let (gold, filled) = trade.own_offer_for_test();
            assert_eq!(gold, expect_gold, "slot {slot}");
            assert_eq!(filled[1], expect_slot2, "slot {slot}");
            assert_eq!(errors.0, vec![UiError::key("ERR_TRADE_WRONG_REALM")]);
        }
    }

    /// **A `%s` line prints only for the side that asked.** Cases 0/5/14 read the network
    /// handler's guid cell and are guarded on the outbound-initiate latch `[0xc4bec8]`, which
    /// exactly one instruction image-wide sets — inside `InitiateTrade`.
    ///
    /// That guard is load-bearing rather than decorative: vmangos's `HandleIgnoreTradeOpcode`
    /// calls `TradeCancel(sendback = true, IGNORE_YOU)`, so the client that just *refused* a trade
    /// receives `IGNORE_YOU` back at itself with the initiator's guid still in the cell. The latch
    /// is what keeps that silent. A client without it would print "Grubbis is ignoring you." to
    /// the person who did the ignoring — inventing a quirk, not reproducing one.
    #[test]
    fn the_naming_lines_are_gated_on_having_initiated() {
        for status in [
            TradeStatus::Busy,
            TradeStatus::Busy2,
            TradeStatus::IgnoreYou,
        ] {
            // We asked: the line is owed, naming our target.
            let (mut errors, commands, _rx) = sink();
            let mut trade = TradeSession::default();
            trade.initiate(0x7);
            trade.take_named_lines_for_test();
            trade_status(status, &mut trade, &mut errors, &commands);
            assert_eq!(
                trade.take_named_lines_for_test(),
                vec![NamedLine {
                    key: match status {
                        TradeStatus::IgnoreYou => "ERR_IGNORING_YOU_S",
                        _ => "ERR_PLAYER_BUSY_S",
                    },
                    who: 0x7,
                    needs_live_object: true,
                }],
                "{status:?}: the asker is told, and told about its target"
            );
            assert!(errors.0.is_empty(), "a %s line waits for the name cache");

            // We did not ask — the refuser's own echo. Silent.
            let (mut errors, commands, _rx) = sink();
            let mut trade = TradeSession::default();
            trade.request(0x7);
            trade.begin(0x7); // accepted an INCOMING request: the latch was never set
            trade_status(status, &mut trade, &mut errors, &commands);
            assert!(
                trade.take_named_lines_for_test().is_empty() && errors.0.is_empty(),
                "{status:?}: the side that did not initiate hears nothing"
            );
        }
    }

    /// The plain lines need no name and go straight out, in the same drain the packet arrived in.
    #[test]
    fn the_plain_refusals_are_raised_immediately() {
        let (mut errors, commands, _rx) = sink();
        let mut trade = TradeSession::default();
        trade.initiate(0x7);
        trade.take_named_lines_for_test();
        trade_status(TradeStatus::TargetDead, &mut trade, &mut errors, &commands);
        assert_eq!(errors.0, vec![UiError::key("ERR_TRADE_TARGET_DEAD")]);
    }
}
