//! The trade's packet handlers: `SMSG_TRADE_STATUS` drives the open, accept and close state and
//! `SMSG_TRADE_STATUS_EXTENDED` replaces one side's offer, both into [`TradeSession`]. The Lua
//! events they cause are fired by [`super::feed_trade`], which owns the VM.

use benilla_protocol::messages::{TradeStatus, TradeStatusExtended};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{NamedLine, TradeSession};
use crate::net::NetHandlerApp;
use crate::ui_action::{UiError, UiErrorKeys};

/// Register the trade's handlers: one per kind, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TradeStatus, on_trade_status)
        .net_handler(K::TradeStatusExtended, on_trade_status_extended)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_trade_status(
    In(ev): In<SessionEvent>,
    mut trade: ResMut<TradeSession>,
    errors: Res<UiErrorKeys>,
    mut world_commands: Commands,
) {
    if let SessionEvent::TradeStatus { status } = ev {
        if let Some(window) = trade_status(status, &mut trade, &errors) {
            world_commands.queue(move |world: &mut World| match window {
                Window::Open => super::open_window(world),
                Window::Close { canceled, done } => super::server_close(world, canceled, done),
                // Status 13's `PLAYER_MONEY` (`0x5d48a8`), the only thing it signals, after both
                // offers are zeroed (`0x5d489c`, `0x5d48a2`).
                Window::Money => crate::ui_session::fire(world, |script| {
                    script.zero_trade_gold();
                    script.fire_event("PLAYER_MONEY", vec![]);
                }),
            });
        }
    }
}

/// What a status arm does to the window, run with the world in hand after the arm's line and the
/// handler's tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Window {
    /// `OPEN_WINDOW` (`0x4bf7f8` → `0x4bf4e0(partner)`).
    Open,
    /// `CANCELED`, `COMPLETE` or `CLOSE_WINDOW`; only `CANCELED` signals `TRADE_REQUEST_CANCEL`,
    /// and only `COMPLETE` closes with `done` 1 (`0x4bf862`).
    Close { canceled: bool, done: bool },
    /// Code 13: the handler's clear and `PLAYER_MONEY` (`0x5d4884`-`0x5d48a8`), with no arm in
    /// the dispatcher (`0x4bfa02`); the window stays.
    Money,
}

fn on_trade_status_extended(In(ev): In<SessionEvent>, mut trade: ResMut<TradeSession>) {
    if let SessionEvent::TradeStatusExtended { state } = ev {
        trade_status_extended(&state, &mut trade);
    }
}

/// An open trade dies with the socket, after `net::session::on_disconnected`'s own teardown.
fn on_session_end(In(_): In<SessionEvent>, mut trade: ResMut<TradeSession>) {
    trade.clear_session();
}

/// `SMSG_TRADE_STATUS`, arm for arm the reference's 23-case dispatcher `0x4bf720` (called from the
/// handler `0x5d47c0`): an arm may print ([`status_message`]), only `CANCELED`, `COMPLETE` and
/// `CLOSE_WINDOW` close the window, and the handler's tail then clears the network cells
/// ([`TradeSession::clear_pending`]). `BEGIN_TRADE` only records the request for
/// [`super::answer_trade_request`].
fn trade_status(
    status: TradeStatus,
    trade: &mut TradeSession,
    errors: &UiErrorKeys,
) -> Option<Window> {
    bevy::log::info!(target: "trade", "SMSG_TRADE_STATUS {status:?}");

    // The line first: a naming arm reads the guid cells before the tail clears them, the
    // reference's order (`0x5d4923` dispatches, `0x5d4931` clears). A `%s` line waits for a name.
    match status_message(status) {
        Some(Line::Plain(key)) => errors.push(UiError::key(key)),
        Some(Line::Naming(key)) => {
            if let Some(who) = trade.line_names() {
                trade.owe_named_line(NamedLine {
                    key,
                    who,
                    // The arm reads the live `CGUnit`; the feed applies that guard.
                    needs_live_object: true,
                });
            }
        }
        None => {}
    }

    // The four ending statuses zero both money offers before the dispatcher (`0x5d4884`,
    // `0x5d48eb`); [`super::server_close`] zeroes the VM's.
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
        TradeStatus::BeginTrade { partner } => trade.request(partner),
        TradeStatus::OpenWindow => return Some(Window::Open),
        // Case 4 (`0x4bf80b`) sets the partner's accept flag and case 9 (`0x4bf821`) clears it;
        // neither touches ours, closes anything or is in the tail's clear set.
        TradeStatus::Accept => trade.partner_accepted(),
        TradeStatus::Rejected => trade.partner_unaccepted(),
        // Case 7 (`0x4bf81a`) drops both accepts, ours through `0x4bf230(0)`, which signals and
        // then sends `CMSG_UNACCEPT_TRADE` when the flag was up, then the partner's; the feed
        // replays the steps. vmangos then passes a `BACK_TO_TRADE` on to the partner
        // (`TradeHandler.cpp:539`).
        TradeStatus::BackToTrade => trade.back_to_trade(),
        // The three that close the window through the core ([`super::server_close`]), which sends
        // `CMSG_CANCEL_TRADE` as the reference's does (`0x4bf527`); vmangos ignores it once the
        // trade is gone (`Player.cpp:11739`). The tail's clear runs here, the core's reset later.
        TradeStatus::Canceled => {
            trade.clear_pending();
            return Some(Window::Close {
                canceled: true,
                done: false,
            });
        }
        TradeStatus::Complete => {
            trade.clear_pending();
            return Some(Window::Close {
                canceled: false,
                done: true,
            });
        }
        TradeStatus::CloseWindow { .. } => {
            trade.clear_pending();
            return Some(Window::Close {
                canceled: false,
                done: false,
            });
        }
        // The placement is bounced, not the window (case 22, `0x4bf9ec` → `0x4bfbd0`).
        TradeStatus::OnlyConjured { slot } => trade.bounce_own_offer(slot),
        // The initiate refusals: the tail's clear alone, each line already queued above.
        TradeStatus::Busy
        | TradeStatus::Busy2
        | TradeStatus::NoTarget
        | TradeStatus::TargetTooFar
        | TradeStatus::WrongFaction
        | TradeStatus::IgnoreYou
        | TradeStatus::YouStunned
        | TradeStatus::TargetStunned
        | TradeStatus::YouDead
        | TradeStatus::TargetDead
        | TradeStatus::YouLogout
        | TradeStatus::TargetLogout
        | TradeStatus::TrialAccount => trade.clear_pending(),
        TradeStatus::Unknown13 => {
            trade.clear_pending();
            return Some(Window::Money);
        }
        // Past `0x16`: the bare epilogue `0x4bfa02`, as for case 13; vmangos never sends one.
        TradeStatus::Unknown(_) => {}
    }
    None
}

/// A status arm's line: a plain key, or one whose `%s` waits for a player's name.
enum Line {
    Plain(&'static str),
    Naming(&'static str),
}

/// The `GlobalStrings` key each status arm raises through `DisplayError`, or `None`. The catalog
/// row's `+0x04`, not the key, decides chat, yellow or red: the two successful outcomes are the
/// yellow ones, and `ERR_TRADE_WRONG_REALM`'s text is the conjured-items sentence.
///
/// Not built: `CLOSE_WINDOW` (12) prints an id `0x4bfd70` computes from `result` (61 ids, the rest
/// through `0x622630`, `0x1d1` printing nothing); vmangos never sends 12, so it closes silently.
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
        // Silent, plus `BEGIN_TRADE` (the ladder's line) and `CLOSE_WINDOW` (not built).
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

/// `SMSG_TRADE_STATUS_EXTENDED`: replace one side's offer; the feed repaints on the change.
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
    use crate::net::{ClientCommand, NetCommands};
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

    fn sink() -> (
        UiErrorKeys,
        NetCommands,
        crossbeam_channel::Receiver<ClientCommand>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (UiErrorKeys::default(), NetCommands(tx), rx)
    }

    /// A window up and both sides accepted, so a wrong close, accept reset or cell clear shows.
    fn open_trade() -> TradeSession {
        let mut trade = TradeSession::default();
        trade.initiate(0x7, false);
        trade.open_window();
        trade.accept();
        trade.partner_accepted();
        trade.take_named_lines_for_test();
        trade
    }

    #[test]
    fn the_table_routes_the_status_and_the_session_end_to_the_trade() {
        let (_, commands, _rx) = sink();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<TradeSession>()
            .init_resource::<UiErrorKeys>()
            .insert_resource(commands);
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
        assert_eq!(kind_of("ERR_PLAYER_BUSY_S"), MsgKind::Chat);
        assert_eq!(kind_of("ERR_IGNORING_YOU_S"), MsgKind::Chat);
        assert_eq!(kind_of("ERR_TRADE_CANCELLED"), MsgKind::Info);
        assert_eq!(kind_of("ERR_TRADE_COMPLETE"), MsgKind::Info);
        assert_eq!(kind_of("ERR_TRADE_TARGET_DEAD"), MsgKind::Error);
    }

    /// `0x4bf4e0` has four call sites in the dispatcher: one opens the window, three close it,
    /// and only `CANCELED`'s signals `TRADE_REQUEST_CANCEL` first (`0x4bf832`).
    #[test]
    fn only_canceled_complete_and_close_window_tear_the_window_down() {
        let windows: Vec<(u32, Window)> = every_status()
            .into_iter()
            .filter_map(|status| {
                let (errors, _commands, _rx) = sink();
                let mut trade = open_trade();
                trade_status(status, &mut trade, &errors).map(|w| (status.code(), w))
            })
            .collect();
        assert_eq!(
            windows,
            vec![
                (2, Window::Open),
                (
                    3,
                    Window::Close {
                        canceled: true,
                        done: false,
                    },
                ),
                (
                    8,
                    Window::Close {
                        canceled: false,
                        done: true,
                    },
                ),
                (
                    12,
                    Window::Close {
                        canceled: false,
                        done: false,
                    },
                ),
                (13, Window::Money),
            ]
        );
    }

    /// The tail's clear set minus the three closing codes.
    #[test]
    fn the_refusals_clear_the_latch_and_leave_the_window_standing() {
        for status in every_status() {
            let code = status.code();
            if !matches!(code, 0 | 5 | 6 | 10 | 11 | 13 | 14..=21) {
                continue;
            }
            let (errors, _commands, _rx) = sink();
            let mut trade = open_trade();
            trade_status(status, &mut trade, &errors);
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
        let (errors, _commands, _rx) = sink();
        let mut trade = open_trade();
        trade_status(TradeStatus::Rejected, &mut trade, &errors);
        assert!(trade.is_open());
        assert!(trade.we_accepted(), "ours is untouched");
        assert!(!trade.partner_has_accepted(), "theirs is dropped");
        assert!(errors.is_empty(), "and it says nothing");
        assert!(trade.line_names().is_some(), "9 is not in the clear set");
    }

    /// `BACK_TO_TRADE` (`0x4bf81c`, `0x4bf823`): `0x4bf230(0)` signals our drop and then sends the
    /// un-accept, and `0x4bf280(0)` signals the partner's own drop after it; each only when its
    /// flag was up, so the signal can come twice, once or not at all.
    #[test]
    fn back_to_trade_signals_each_drop_and_unaccepts_after_ours() {
        use super::super::AcceptStep::{SendUnaccept, Signal};
        let (errors, _commands, _rx) = sink();
        let mut trade = open_trade(); // we accepted
        trade.partner_accepted();
        assert_eq!(trade.take_accept_steps(), [Signal(true, true)]);
        trade_status(TradeStatus::BackToTrade, &mut trade, &errors);
        assert_eq!(
            trade.take_accept_steps(),
            [Signal(false, true), SendUnaccept, Signal(false, false)]
        );
        assert!(!trade.we_accepted());
        assert!(errors.is_empty(), "the bounce is silent");

        // A second bounce with neither flag up does nothing.
        trade_status(TradeStatus::BackToTrade, &mut trade, &errors);
        assert!(trade.take_accept_steps().is_empty());
    }

    /// `0xff` names the gold; an out-of-range slot (200) is dropped.
    #[test]
    fn only_conjured_bounces_one_slot_and_leaves_the_window_up() {
        // Wire slot 1 is UI slot 2, the one `place_own_item` fills below.
        for (slot, expect_gold, expect_slot2) in
            [(1u8, 700u32, false), (0xff, 0, true), (200, 700, true)]
        {
            let (errors, _commands, _rx) = sink();
            let mut trade = open_trade();
            trade.place_own_item(2, item());
            trade.set_own_gold(700);
            trade_status(TradeStatus::OnlyConjured { slot }, &mut trade, &errors);
            assert!(trade.is_open(), "slot {slot}: the window stays up");
            let (gold, filled) = trade.own_offer_for_test();
            assert_eq!(gold, expect_gold, "slot {slot}");
            assert_eq!(filled[1], expect_slot2, "slot {slot}");
            assert_eq!(
                errors.snapshot(),
                vec![UiError::key("ERR_TRADE_WRONG_REALM")]
            );
        }
    }

    /// Cases 0, 5 and 14 print only under the initiate latch `[0xc4bec8]`, so the refuser stays
    /// silent when vmangos echoes `IGNORE_YOU` to both sides (`TradeHandler.cpp:46`).
    #[test]
    fn the_naming_lines_are_gated_on_having_initiated() {
        for status in [
            TradeStatus::Busy,
            TradeStatus::Busy2,
            TradeStatus::IgnoreYou,
        ] {
            // We asked: the line is owed, naming our target.
            let (errors, _commands, _rx) = sink();
            let mut trade = TradeSession::default();
            trade.initiate(0x7, false);
            trade.take_named_lines_for_test();
            trade_status(status, &mut trade, &errors);
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
            assert!(errors.is_empty(), "a %s line waits for the name cache");

            // We did not ask: the refuser's own echo is silent.
            let (errors, _commands, _rx) = sink();
            let mut trade = TradeSession::default();
            trade.request(0x7);
            trade.begin(0x7); // accepted an incoming request: no latch
            trade_status(status, &mut trade, &errors);
            assert!(
                trade.take_named_lines_for_test().is_empty() && errors.is_empty(),
                "{status:?}: the side that did not initiate hears nothing"
            );
        }
    }

    #[test]
    fn the_plain_refusals_are_raised_immediately() {
        let (errors, _commands, _rx) = sink();
        let mut trade = TradeSession::default();
        trade.initiate(0x7, false);
        trade.take_named_lines_for_test();
        trade_status(TradeStatus::TargetDead, &mut trade, &errors);
        assert_eq!(
            errors.snapshot(),
            vec![UiError::key("ERR_TRADE_TARGET_DEAD")]
        );
    }
}
