//! The trade's packet handlers: `SMSG_TRADE_STATUS` drives the open, accept and close state and
//! `SMSG_TRADE_STATUS_EXTENDED` replaces one side's offer, both into [`TradeSession`], and each
//! queues what it signals on the trade lane ([`TradeLane`]) inside itself.

use benilla_protocol::messages::{TradeStatus, TradeStatusExtended};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{StatusReads, TradeEvent, TradeLane, TradeSession};
use crate::net::NetHandlerApp;
use crate::ui_action::{FillArg, UiError};

/// Register the trade's handlers: one per kind, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.init_resource::<TradeLane>()
        .net_handler(K::TradeStatus, on_trade_status)
        .net_handler(K::TradeStatusExtended, on_trade_status_extended)
        .net_handler(K::Disconnected, on_session_end);
}

/// `0x5d47c0`: the request's ladder inside the status (case 1), every other status's arm, and the
/// window's open or close with the world in hand after the handler.
fn on_trade_status(
    In(ev): In<SessionEvent>,
    mut trade: ResMut<TradeSession>,
    mut lane: ResMut<TradeLane>,
    reads: StatusReads,
    mut world_commands: Commands,
) {
    let SessionEvent::TradeStatus { status } = ev else {
        return;
    };
    trade.settle_world_reset();
    bevy::log::info!(target: "trade", "SMSG_TRADE_STATUS {status:?}");
    if let TradeStatus::BeginTrade { partner } = status {
        super::answer_request(partner, &mut trade, &mut lane, &reads);
        return;
    }
    // The open leg's coins are tested against the coinage as the handler finds it (`0x5d4570`).
    let purse = trade.opens_with_held().then(|| reads.purse());
    let Some(window) = trade_status(status, &mut trade, &mut lane, &|who| reads.unit_name(who))
    else {
        return;
    };
    world_commands.queue(move |world: &mut World| match window {
        Window::Open => super::open_window(world, purse),
        Window::Close(close) => super::server_close(world, close),
    });
}

/// What a status arm does to the window, run with the world in hand after the handler's tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Window {
    /// `OPEN_WINDOW` (`0x4bf7f8` → `0x4bf4e0(partner)`).
    Open,
    /// `CANCELED`, `COMPLETE` or `CLOSE_WINDOW`.
    Close(Close),
}

/// A closing arm: only `CANCELED` signals `TRADE_REQUEST_CANCEL`, only `COMPLETE` closes with
/// `done` 1 (`0x4bf862`); `CLOSE_WINDOW` prints its line before the close core (`0x4bf88e`), the
/// other two theirs after it (`0x4bf847`, `0x4bf871`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Close {
    pub(super) canceled: bool,
    pub(super) done: bool,
    pub(super) first: Option<UiError>,
    pub(super) after: Option<UiError>,
}

fn on_trade_status_extended(
    In(ev): In<SessionEvent>,
    mut trade: ResMut<TradeSession>,
    mut lane: ResMut<TradeLane>,
) {
    if let SessionEvent::TradeStatusExtended { state } = ev {
        trade.settle_world_reset();
        trade_status_extended(&state, &mut trade, &mut lane);
    }
}

/// An open trade dies with the socket, after `net::session::on_disconnected`'s own teardown.
fn on_session_end(In(_): In<SessionEvent>, mut trade: ResMut<TradeSession>) {
    trade.clear_session();
}

/// `SMSG_TRADE_STATUS` but `BEGIN_TRADE`, arm for arm the reference's 23-case dispatcher
/// `0x4bf720` (called from the handler `0x5d47c0`), each arm's signals and line queued on `lane`
/// in its order; only `CANCELED`, `COMPLETE` and `CLOSE_WINDOW` close the window, and the
/// handler's tail then clears the network cells ([`TradeSession::clear_pending`]). `name_of` is
/// `GetUnitName` on a streamed player, `None` for anyone else (`0x468460`, `0x609210`).
fn trade_status(
    status: TradeStatus,
    trade: &mut TradeSession,
    lane: &mut TradeLane,
    name_of: &dyn Fn(u64) -> Option<FillArg>,
) -> Option<Window> {
    let line = |lane: &mut TradeLane, key| lane.push(TradeEvent::Line(UiError::key(key)));
    // The four ending statuses zero both money offers before the dispatcher (`0x5d4884`,
    // `0x5d48eb`); [`super::server_close`] zeroes the VM's copy.
    if matches!(
        status,
        TradeStatus::Canceled
            | TradeStatus::Complete
            | TradeStatus::CloseWindow { .. }
            | TradeStatus::Unknown13
    ) {
        trade.zero_gold();
    }
    match status {
        TradeStatus::BeginTrade { .. } => {}
        TradeStatus::OpenWindow => return Some(Window::Open),
        // Case 4 (`0x4bf80b`) sets the partner's accept flag and case 9 (`0x4bf821`) clears it;
        // neither touches ours, closes anything or is in the tail's clear set.
        TradeStatus::Accept => trade.partner_accepted(lane),
        TradeStatus::Rejected => trade.partner_unaccepted(lane),
        // Case 7 (`0x4bf81a`) drops both accepts, ours through `0x4bf230(0)`, which signals and
        // then sends `CMSG_UNACCEPT_TRADE` when the flag was up, then the partner's. vmangos then
        // passes a `BACK_TO_TRADE` on to the partner (`TradeHandler.cpp:539`).
        TradeStatus::BackToTrade => trade.back_to_trade(lane),
        // The three that close the window through the core ([`super::server_close`]), which sends
        // `CMSG_CANCEL_TRADE` as the reference's does (`0x4bf527`); vmangos ignores it once the
        // trade is gone (`Player.cpp:11739`). The tail's clear runs here, the core's reset later.
        TradeStatus::Canceled | TradeStatus::Complete | TradeStatus::CloseWindow { .. } => {
            trade.clear_pending();
            let after = |key| Some(UiError::key(key));
            return Some(Window::Close(match status {
                TradeStatus::Canceled => Close {
                    canceled: true,
                    done: false,
                    first: None,
                    after: after("ERR_TRADE_CANCELLED"),
                },
                TradeStatus::Complete => Close {
                    canceled: false,
                    done: true,
                    first: None,
                    after: after("ERR_TRADE_COMPLETE"),
                },
                TradeStatus::CloseWindow { result, side, .. } => Close {
                    canceled: false,
                    done: false,
                    first: game_error(result, side != 0),
                    after: None,
                },
                _ => unreachable!("the three closing statuses"),
            }));
        }
        // The line, then the placement bounced, not the window (case 22, `0x4bf9ec` →
        // `0x4bfbd0`).
        TradeStatus::OnlyConjured { slot } => {
            line(lane, "ERR_TRADE_WRONG_REALM");
            trade.bounce_own_offer(slot, lane);
        }
        // Cases 0, 5 and 14 name the partner only under the initiate latch and while the cell
        // names a streamed player (`0x4bf8dc`-`0x4bf964`), then the tail's clear.
        TradeStatus::Busy | TradeStatus::Busy2 | TradeStatus::IgnoreYou => {
            let key = match status {
                TradeStatus::IgnoreYou => "ERR_IGNORING_YOU_S",
                _ => "ERR_PLAYER_BUSY_S",
            };
            if let Some(name) = trade.line_names().and_then(name_of) {
                lane.push(TradeEvent::Line(UiError::args(key, vec![name])));
            }
            trade.clear_pending();
        }
        // The initiate refusals: the line, then the tail's clear.
        TradeStatus::NoTarget
        | TradeStatus::TargetTooFar
        | TradeStatus::WrongFaction
        | TradeStatus::YouStunned
        | TradeStatus::TargetStunned
        | TradeStatus::YouDead
        | TradeStatus::TargetDead
        | TradeStatus::YouLogout
        | TradeStatus::TargetLogout
        | TradeStatus::TrialAccount => {
            if let Some(key) = plain_line(status) {
                line(lane, key);
            }
            trade.clear_pending();
        }
        // Code 13: the handler's clear and `PLAYER_MONEY` (`0x5d4884`-`0x5d48a8`), with no arm in
        // the dispatcher (`0x4bfa02`); the window stays.
        TradeStatus::Unknown13 => {
            lane.push(TradeEvent::Purse);
            trade.clear_pending();
        }
        // Past `0x16`: the bare epilogue `0x4bfa02`, as for case 13; vmangos never sends one.
        TradeStatus::Unknown(_) => {}
    }
    None
}

/// `CLOSE_WINDOW`'s line, `GetGameError` (`0x4bfd70`) on the status's inventory result and its
/// byte, which the handler passes as zero or not (`0x5d4916`): bag full (4) and too many (0x11)
/// name the player or the partner by the byte, in opposite senses (`0x4bfda4`, `0x4bfd97`), a
/// missing reagent (0x1c) prints only for byte 0 (`0x4bfd84`, 0x1d1 printing nothing), and every
/// other result goes through the inventory table (`0x4bfd7f` → `0x622630`). The display reads one
/// argument word past the id, the status handler's tail flag, 1 for this status (`0x5d4905`),
/// which only result 1's level line prints.
fn game_error(result: u32, side: bool) -> Option<UiError> {
    let key = match (result, side) {
        (4, false) => "ERR_TRADE_TARGET_BAG_FULL",
        (4, true) => "ERR_TRADE_BAG_FULL",
        (0x11, false) => "ERR_TRADE_MAX_COUNT_EXCEEDED",
        (0x11, true) => "ERR_TRADE_TARGET_MAX_COUNT_EXCEEDED",
        (0x1c, false) => "ERR_SPELL_FAILED_REAGENTS_GENERIC",
        (0x1c, true) => return None,
        (result, _) => crate::ui_items::inventory_result_key(result)?,
    };
    Some(UiError::args(key, vec![FillArg::D(1)]))
}

/// The `GlobalStrings` key a status arm with no argument raises through `DisplayError`, or `None`.
/// The catalog row's `+0x04`, not the key, decides chat, yellow or red: the two successful
/// outcomes are the yellow ones, and `ERR_TRADE_WRONG_REALM`'s text is the conjured-items
/// sentence.
fn plain_line(status: TradeStatus) -> Option<&'static str> {
    Some(match status {
        TradeStatus::Canceled => "ERR_TRADE_CANCELLED",
        TradeStatus::NoTarget => "ERR_GENERIC_NO_TARGET",
        TradeStatus::Complete => "ERR_TRADE_COMPLETE",
        TradeStatus::TargetTooFar => "ERR_TRADE_TOO_FAR",
        TradeStatus::WrongFaction => "ERR_PLAYER_WRONG_FACTION",
        TradeStatus::YouStunned => "ERR_GENERIC_STUNNED",
        TradeStatus::TargetStunned => "ERR_TARGET_STUNNED",
        TradeStatus::YouDead => "ERR_PLAYER_DEAD",
        TradeStatus::TargetDead => "ERR_TRADE_TARGET_DEAD",
        TradeStatus::YouLogout => "ERR_LOGGING_OUT",
        TradeStatus::TargetLogout => "ERR_TARGET_LOGGING_OUT",
        TradeStatus::TrialAccount => "ERR_RESTRICTED_ACCOUNT",
        TradeStatus::OnlyConjured { .. } => "ERR_TRADE_WRONG_REALM",
        _ => return None,
    })
}

/// `SMSG_TRADE_STATUS_EXTENDED`: replace one side's offer and queue what it signals.
fn trade_status_extended(
    ext: &TradeStatusExtended,
    trade: &mut TradeSession,
    lane: &mut TradeLane,
) {
    bevy::log::info!(
        target: "trade",
        "SMSG_TRADE_STATUS_EXTENDED their_window={} gold={} items={}",
        ext.their_window,
        ext.gold,
        ext.slots.iter().filter(|s| s.is_some()).count(),
    );
    trade.set_offer(ext, lane);
}

#[cfg(test)]
mod tests {
    use benilla_ui::messages::{kind_of, MsgKind};

    use super::*;
    use crate::net::NetCommands;
    use crate::ui_session::LatchApp;

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
                side: 0,
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

    /// `name_of` for a world where `0x7` is a streamed player named Grubbis.
    fn grubbis(who: u64) -> Option<FillArg> {
        (who == 0x7).then(|| FillArg::S("Grubbis".into()))
    }

    /// A window up and both sides accepted, so a wrong close, accept reset or cell clear shows.
    fn open_trade() -> TradeSession {
        let mut trade = TradeSession::default();
        trade.initiate(0x7, false);
        trade.open_window();
        trade.accept();
        trade.partner_accepted(&mut TradeLane::default());
        trade
    }

    /// The lines `lane` holds, by key, taking every item.
    fn lines(lane: &mut TradeLane) -> Vec<UiError> {
        lane.take()
            .into_iter()
            .filter_map(|e| match e {
                TradeEvent::Line(line) => Some(line),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_table_routes_the_status_and_the_session_end_to_the_trade() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<TradeSession>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        app.add_latched_window::<TradeSession>();
        register(&mut app);
        app.world_mut().resource_mut::<TradeSession>().begin(0x7);

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::TradeStatus {
                status: TradeStatus::OpenWindow,
            }],
        );
        assert!(app.world().resource::<TradeSession>().is_open());
        assert_eq!(
            app.world_mut().resource_mut::<TradeLane>().take(),
            [TradeEvent::Show {
                partner: 0x7,
                purse: None
            }],
            "the open signals on the lane, inside its handler"
        );

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );
        assert!(!app.world().resource::<TradeSession>().is_open());
    }

    #[test]
    fn sixteen_arms_print_and_the_seven_silent_ones_are_pinned() {
        let silent: Vec<u32> = every_status()
            .into_iter()
            .filter(|s| {
                let mut trade = open_trade();
                let mut lane = TradeLane::default();
                trade_status(*s, &mut trade, &mut lane, &grubbis);
                let printed = !lines(&mut lane).is_empty()
                    || matches!(
                        trade_status_window(*s),
                        Some(Window::Close(
                            Close { first: Some(_), .. } | Close { after: Some(_), .. }
                        ))
                    );
                !printed
            })
            .map(TradeStatus::code)
            .collect();
        assert_eq!(
            silent,
            vec![1, 2, 4, 7, 9, 12, 13],
            "silent: BEGIN_TRADE (the ladder's line is its own), OPEN_WINDOW, TRADE_ACCEPT, \
             BACK_TO_TRADE, REJECTED, CLOSE_WINDOW on result 0 (id 0x1d1) and UNKNOWN_13"
        );
    }

    /// The window `status` leaves, on a fresh open trade.
    fn trade_status_window(status: TradeStatus) -> Option<Window> {
        trade_status(
            status,
            &mut open_trade(),
            &mut TradeLane::default(),
            &grubbis,
        )
    }

    #[test]
    fn the_surfaces_are_the_catalogs_and_not_the_key_names() {
        for status in every_status() {
            let mut lane = TradeLane::default();
            let window = trade_status(status, &mut open_trade(), &mut lane, &grubbis);
            let mut keys: Vec<&str> = lines(&mut lane).into_iter().map(|l| l.key).collect();
            if let Some(Window::Close(close)) = window {
                keys.extend(close.first.iter().chain(&close.after).map(|l| l.key));
            }
            for key in keys {
                assert!(
                    benilla_ui::messages::by_key(key).is_some(),
                    "{status:?} names {key}, which is not a catalog row"
                );
            }
        }
        assert_eq!(kind_of("ERR_PLAYER_BUSY_S"), MsgKind::Chat);
        assert_eq!(kind_of("ERR_IGNORING_YOU_S"), MsgKind::Chat);
        assert_eq!(kind_of("ERR_TRADE_CANCELLED"), MsgKind::Info);
        assert_eq!(kind_of("ERR_TRADE_COMPLETE"), MsgKind::Info);
        assert_eq!(kind_of("ERR_TRADE_TARGET_DEAD"), MsgKind::Error);
    }

    /// `0x4bf4e0` has four call sites in the dispatcher: one opens the window, three close it,
    /// and only `CANCELED`'s signals `TRADE_REQUEST_CANCEL` first (`0x4bf832`); the cancel's and
    /// the complete's lines follow their closes (`0x4bf847`, `0x4bf871`).
    #[test]
    fn only_canceled_complete_and_close_window_tear_the_window_down() {
        let windows: Vec<(u32, Window)> = every_status()
            .into_iter()
            .filter_map(|status| trade_status_window(status).map(|w| (status.code(), w)))
            .collect();
        assert_eq!(
            windows,
            vec![
                (2, Window::Open),
                (
                    3,
                    Window::Close(Close {
                        canceled: true,
                        done: false,
                        first: None,
                        after: Some(UiError::key("ERR_TRADE_CANCELLED")),
                    }),
                ),
                (
                    8,
                    Window::Close(Close {
                        canceled: false,
                        done: true,
                        first: None,
                        after: Some(UiError::key("ERR_TRADE_COMPLETE")),
                    }),
                ),
                (
                    12,
                    Window::Close(Close {
                        canceled: false,
                        done: false,
                        first: None,
                        after: None,
                    }),
                ),
            ]
        );
    }

    /// `CLOSE_WINDOW`'s line, `GetGameError` (`0x4bfd70`) on its result and byte: the three
    /// results it maps itself, by the byte, and the rest through the inventory table
    /// (`0x622630`), past 66 its default and 0 nothing.
    #[test]
    fn close_window_prints_the_line_its_result_and_byte_pick() {
        let line = |result, side| {
            let status = TradeStatus::CloseWindow {
                result,
                side,
                item_limit_category: 0,
            };
            match trade_status_window(status) {
                Some(Window::Close(close)) => {
                    assert_eq!(close.after, None, "the line comes before the close");
                    close.first.map(|l| l.key)
                }
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(line(4, 0), Some("ERR_TRADE_TARGET_BAG_FULL"));
        assert_eq!(line(4, 1), Some("ERR_TRADE_BAG_FULL"));
        assert_eq!(
            line(4, 9),
            Some("ERR_TRADE_BAG_FULL"),
            "the byte is read as nonzero"
        );
        assert_eq!(line(0x11, 0), Some("ERR_TRADE_MAX_COUNT_EXCEEDED"));
        assert_eq!(line(0x11, 1), Some("ERR_TRADE_TARGET_MAX_COUNT_EXCEEDED"));
        assert_eq!(line(0x1c, 0), Some("ERR_SPELL_FAILED_REAGENTS_GENERIC"));
        assert_eq!(line(0x1c, 1), None);
        assert_eq!(line(0x32, 0), Some("ERR_INV_FULL"));
        assert_eq!(line(67, 1), Some("ERR_BAG_FULL"));
        assert_eq!(line(0, 0), None);
    }

    /// The tail's clear set minus the three closing codes.
    #[test]
    fn the_refusals_clear_the_latch_and_leave_the_window_standing() {
        for status in every_status() {
            let code = status.code();
            if !matches!(code, 0 | 5 | 6 | 10 | 11 | 13 | 14..=21) {
                continue;
            }
            let mut trade = open_trade();
            trade_status(status, &mut trade, &mut TradeLane::default(), &grubbis);
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

    #[test]
    fn rejected_only_drops_the_partners_accept() {
        let mut trade = open_trade();
        let mut lane = TradeLane::default();
        trade_status(TradeStatus::Rejected, &mut trade, &mut lane, &grubbis);
        assert!(trade.is_open());
        assert!(trade.we_accepted(), "ours is untouched");
        assert!(!trade.partner_has_accepted(), "theirs is dropped");
        assert_eq!(
            lane.take(),
            [TradeEvent::Accept(true, false)],
            "and it says nothing"
        );
        assert!(trade.line_names().is_some(), "9 is not in the clear set");
    }

    /// `BACK_TO_TRADE` (`0x4bf81c`, `0x4bf823`): `0x4bf230(0)` signals our drop and then sends the
    /// un-accept, and `0x4bf280(0)` signals the partner's own drop after it; each only when its
    /// flag was up, so the signal can come twice, once or not at all.
    #[test]
    fn back_to_trade_signals_each_drop_and_unaccepts_after_ours() {
        let mut trade = open_trade(); // we accepted, and so has the partner
        let mut lane = TradeLane::default();
        trade_status(TradeStatus::BackToTrade, &mut trade, &mut lane, &grubbis);
        assert_eq!(
            lane.take(),
            [
                TradeEvent::Accept(false, true),
                TradeEvent::Unaccept,
                TradeEvent::Accept(false, false)
            ]
        );
        assert!(!trade.we_accepted());

        // A second bounce with neither flag up does nothing.
        trade_status(TradeStatus::BackToTrade, &mut trade, &mut lane, &grubbis);
        assert!(lane.take().is_empty());
    }

    /// `0xff` names the gold; an out-of-range slot (200) is dropped. The line comes first, then
    /// `0x4bfbd0`'s signal (`0x4bf9ec`, `0x4bf9fd`).
    #[test]
    fn only_conjured_bounces_one_slot_and_leaves_the_window_up() {
        // Wire slot 1 is UI slot 2, the one `place_own_item` fills below.
        for (slot, expect_gold, expect_slot2, signal) in [
            (1u8, 700u32, false, Some(TradeEvent::Invalidate(1))),
            (0xff, 0, true, Some(TradeEvent::PlayerMoney)),
            (200, 700, true, None),
        ] {
            let mut trade = open_trade();
            let mut lane = TradeLane::default();
            trade.place_own_item(2, item());
            trade.set_own_gold(700);
            trade_status(
                TradeStatus::OnlyConjured { slot },
                &mut trade,
                &mut lane,
                &grubbis,
            );
            assert!(trade.is_open(), "slot {slot}: the window stays up");
            let (gold, filled) = trade.own_offer_for_test();
            assert_eq!(gold, expect_gold, "slot {slot}");
            assert_eq!(filled[1], expect_slot2, "slot {slot}");
            let mut want = vec![TradeEvent::Line(UiError::key("ERR_TRADE_WRONG_REALM"))];
            want.extend(signal);
            assert_eq!(lane.take(), want, "slot {slot}");
        }
    }

    /// Cases 0, 5 and 14 print only under the initiate latch `[0xc4bec8]` and for a streamed
    /// player, so the refuser stays silent when vmangos echoes `IGNORE_YOU` to both sides
    /// (`TradeHandler.cpp:46`), and a partner who left names no one.
    #[test]
    fn the_naming_lines_are_gated_on_having_initiated_and_a_streamed_partner() {
        for status in [
            TradeStatus::Busy,
            TradeStatus::Busy2,
            TradeStatus::IgnoreYou,
        ] {
            let key = match status {
                TradeStatus::IgnoreYou => "ERR_IGNORING_YOU_S",
                _ => "ERR_PLAYER_BUSY_S",
            };
            // We asked: the line names our target, at once.
            let mut trade = TradeSession::default();
            trade.initiate(0x7, false);
            let mut lane = TradeLane::default();
            trade_status(status, &mut trade, &mut lane, &grubbis);
            assert_eq!(
                lines(&mut lane),
                [UiError::s(key, "Grubbis")],
                "{status:?}: the asker is told, and told about its target"
            );

            // A target that is no streamed player: nothing.
            let mut trade = TradeSession::default();
            trade.initiate(0x8, false);
            trade_status(status, &mut trade, &mut lane, &grubbis);
            assert!(lines(&mut lane).is_empty(), "{status:?}: no live object");

            // We did not ask: the refuser's own echo is silent.
            let mut trade = TradeSession::default();
            trade.begin(0x7); // accepted an incoming request: no latch
            trade_status(status, &mut trade, &mut lane, &grubbis);
            assert!(
                lines(&mut lane).is_empty(),
                "{status:?}: the side that did not initiate hears nothing"
            );
        }
    }

    #[test]
    fn the_plain_refusals_are_raised_in_the_handler() {
        let mut trade = TradeSession::default();
        trade.initiate(0x7, false);
        let mut lane = TradeLane::default();
        trade_status(TradeStatus::TargetDead, &mut trade, &mut lane, &grubbis);
        assert_eq!(lines(&mut lane), [UiError::key("ERR_TRADE_TARGET_DEAD")]);
    }

    /// Code 13 signals `PLAYER_MONEY` over both offers zeroed (`0x5d48a8`), and nothing else.
    #[test]
    fn code_13_zeroes_the_offers_and_signals_the_purse() {
        let mut trade = open_trade();
        trade.set_own_gold(700);
        let mut lane = TradeLane::default();
        trade_status(TradeStatus::Unknown13, &mut trade, &mut lane, &grubbis);
        assert_eq!(trade.own_offer_for_test().0, 0);
        assert_eq!(lane.take(), [TradeEvent::Purse]);
        assert!(trade.is_open());
    }
}
