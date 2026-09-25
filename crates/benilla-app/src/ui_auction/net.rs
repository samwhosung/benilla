//! The auction house's packet handlers: every `SMSG_AUCTION_*` becomes state on [`AuctionOpen`],
//! which [`super::feed_auction`] turns into the window's events; nothing here touches the VM.

use benilla_protocol::messages::{
    auction_action, auction_error, AuctionBidderNotification, AuctionCommandTail, AuctionListEntry,
    AuctionOwnerNotification,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{BIDDER, LIST, OWNER};
use bevy::prelude::*;

use super::{AuctionMessage, AuctionOpen};
use crate::net::NetHandlerApp;

/// One handler per `SMSG_AUCTION_*` kind, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::AuctionHello, on_hello)
        .net_handler(K::AuctionCommandResult, on_command_result)
        .net_handler(K::AuctionListResult, on_list_result)
        .net_handler(K::AuctionOwnerListResult, on_owner_list_result)
        .net_handler(K::AuctionBidderListResult, on_bidder_list_result)
        .net_handler(K::AuctionBidderNotification, on_bidder_notification)
        .net_handler(K::AuctionOwnerNotification, on_owner_notification)
        .net_handler(K::AuctionRemovedNotification, on_removed_notification)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_hello(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionHello {
        auctioneer,
        house_id,
    } = ev
    {
        auction_hello(auctioneer, house_id, &mut auction);
    }
}

fn on_command_result(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionCommandResult {
        auction_id,
        action,
        error,
        tail,
    } = ev
    {
        auction_command_result(auction_id, action, error, &tail, &mut auction);
    }
}

fn on_list_result(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionListResult {
        auctions,
        total_count,
    } = ev
    {
        auction_list_result(auctions, total_count, &mut auction);
    }
}

fn on_owner_list_result(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionOwnerListResult {
        auctions,
        total_count,
    } = ev
    {
        auction_owner_list_result(auctions, total_count, &mut auction);
    }
}

fn on_bidder_list_result(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionBidderListResult {
        auctions,
        total_count,
    } = ev
    {
        auction_bidder_list_result(auctions, total_count, &mut auction);
    }
}

fn on_bidder_notification(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionBidderNotification(n) = ev {
        auction_bidder_notification(&n, &mut auction);
    }
}

fn on_owner_notification(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionOwnerNotification(n) = ev {
        auction_owner_notification(&n, &mut auction);
    }
}

fn on_removed_notification(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionRemovedNotification { item_entry, .. } = ev {
        auction_removed_notification(item_entry, &mut auction);
    }
}

/// The window dies with the socket: the server re-validates the auctioneer on every command, so
/// a window kept across a reconnect would fail silently.
fn on_session_end(In(_): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    auction.clear_session();
}

/// The `MSG_AUCTION_HELLO` reply opens the window, as in the reference's hello handler `0x4cc420`.
fn auction_hello(auctioneer: u64, house_id: u32, auction: &mut AuctionOpen) {
    auction.open(auctioneer, house_id);
}

/// Any of the three list results, which share a record and differ only in their tab.
fn auction_list(
    which: usize,
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    // Counted before the drop below, so a dropped result still shows as arrived.
    auction.wire.list_results[which] += 1;
    // A page in flight when the window closed is dropped: the reference has nowhere to put it.
    if auction.auctioneer.is_none() {
        return;
    }
    auction.set_list(which, auctions, total_count);
}

fn auction_list_result(
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    auction_list(LIST, auctions, total_count, auction);
}

fn auction_owner_list_result(
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    auction_list(OWNER, auctions, total_count, auction);
}

/// The bidder page, deduped by auction id: the server lists the refreshed ids, then every auction
/// we lead, so one auction can appear twice.
fn auction_bidder_list_result(
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    let mut seen = std::collections::HashSet::with_capacity(auctions.len());
    let deduped: Vec<AuctionListEntry> = auctions
        .into_iter()
        .filter(|e| seen.insert(e.auction_id))
        .collect();
    auction_list(BIDDER, deduped, total_count, auction);
}

/// `SMSG_AUCTION_COMMAND_RESULT`, the verdict on a sell, a cancel or a bid. `auction_id` is 0 on
/// most failures, and some refusals send nothing at all, so nothing waits on this.
fn auction_command_result(
    auction_id: u32,
    action: u32,
    error: u32,
    tail: &AuctionCommandTail,
    auction: &mut AuctionOpen,
) {
    let _ = tail;
    // For the live probe: a success otherwise leaves no trace but a re-query.
    auction.wire.last_command = Some((auction_id, action, error));
    if error == auction_error::OK {
        // A success re-asks page 0 and patches nothing. The reference re-asks at the saved page
        // (`[0xb72650]`, `[0xb72654]`) on `STARTED` (`0x4cc4e0`) and `BID_PLACED` (`0x4cc528`,
        // then patches the browse row), deletes the row locally on `REMOVED` (`0x4cdfe0`), and
        // prints each success in chat with no fill (`0x4cc4be`).
        match action {
            auction_action::STARTED => {
                auction.sell_slot_taken();
                auction.refresh_owner();
                auction
                    .messages
                    .push(AuctionMessage::chat("ERR_AUCTION_STARTED"));
            }
            auction_action::REMOVED => {
                auction.refresh_owner();
                auction
                    .messages
                    .push(AuctionMessage::chat("ERR_AUCTION_REMOVED"));
            }
            auction_action::BID_PLACED => {
                auction.refresh_bidder();
                auction
                    .messages
                    .push(AuctionMessage::chat("ERR_AUCTION_BID_PLACED"));
            }
            _ => {}
        }
        return;
    }
    // `HIGHER_BID` prints nothing: its arm (`0x4cc672`) patches the row, and the player's line is
    // `ERR_AUCTION_OUTBID_S` from the bidder notification.
    if error == auction_error::HIGHER_BID {
        auction.refresh_bidder();
        return;
    }
    if let Some(key) = command_error_key(error) {
        auction.messages.push(AuctionMessage::error(key));
    }
}

/// The failed command's GlobalStrings key, per the reference's dispatch (`0x4cc460`), whose
/// default arm takes 2, 6, 8, 9, 11, 12 and anything past 13. Codes 1, 4 and 13 fall to that
/// default here; the reference formats 1 through the inventory results (`0x622630`) and raises
/// `ERR_ITEM_NOT_FOUND` (`0x17`) for 4 and `ERR_RESTRICTED_ACCOUNT` (`0x1be`) for 13.
fn command_error_key(error: u32) -> Option<&'static str> {
    Some(match error {
        auction_error::NOT_ENOUGH_MONEY => "ERR_NOT_ENOUGH_MONEY", // `0x25`, the shared id
        auction_error::BID_INCREMENT => "ERR_AUCTION_BID_INCREMENT", // `0x174`
        auction_error::BID_OWN => "ERR_AUCTION_BID_OWN",           // `0x173`
        // `0x172`, the server's catch-all and the reference's default arm.
        _ => "ERR_AUCTION_DATABASE_ERROR",
    })
}

/// `SMSG_AUCTION_BIDDER_NOTIFICATION`: a zero `bid_or_zero` means we won, else we were outbid.
fn auction_bidder_notification(notice: &AuctionBidderNotification, auction: &mut AuctionOpen) {
    let won = notice.bid_or_zero == 0;
    auction.messages.push(AuctionMessage::chat_item(
        if won {
            "ERR_AUCTION_WON_S"
        } else {
            "ERR_AUCTION_OUTBID_S"
        },
        notice.item_entry,
    ));
    auction.refresh_bidder();
}

/// `SMSG_AUCTION_OWNER_NOTIFICATION`: one of ours took a bid, sold or expired.
fn auction_owner_notification(notice: &AuctionOwnerNotification, auction: &mut AuctionOpen) {
    // A bidder guid means a new bid, which the reference does not announce (no display call in
    // `[0x4cd25f, 0x4cd3bd)`); a zero guid is a close, sold if the bid is non-zero (`0x4cd1f0`).
    if notice.bidder_guid == 0 {
        auction.messages.push(AuctionMessage::chat_item(
            if notice.bid != 0 {
                "ERR_AUCTION_SOLD_S"
            } else {
                "ERR_AUCTION_EXPIRED_S"
            },
            notice.item_entry,
        ));
    }
    auction.refresh_owner();
}

/// `SMSG_AUCTION_REMOVED_NOTIFICATION`: the seller cancelled an auction we bid on; the reference
/// always prints `ERR_AUCTION_REMOVED_S` (`0x4cd480`).
fn auction_removed_notification(item_entry: u32, auction: &mut AuctionOpen) {
    auction.messages.push(AuctionMessage::chat_item(
        "ERR_AUCTION_REMOVED_S",
        item_entry,
    ));
    auction.refresh_bidder();
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::messages::MsgKind;

    #[test]
    fn the_table_routes_the_hello_and_the_session_end_to_the_house() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AuctionOpen>();
        register(&mut app);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::AuctionHello {
                auctioneer: 0x10,
                house_id: 1,
            }],
        );
        assert_eq!(app.world().resource::<AuctionOpen>().auctioneer, Some(0x10));
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );
        assert_eq!(app.world().resource::<AuctionOpen>().auctioneer, None);
    }

    fn keys(auction: &AuctionOpen) -> Vec<(&'static str, MsgKind, Option<u32>)> {
        auction
            .messages
            .iter()
            .map(|m| (m.key, benilla_ui::messages::kind_of(m.key), m.item))
            .collect()
    }

    fn owner(bidder_guid: u64, bid: u32) -> AuctionOwnerNotification {
        AuctionOwnerNotification {
            auction_id: 7,
            bid,
            out_bid: 5,
            bidder_guid,
            item_entry: 2589,
            random_property_id: 0,
        }
    }

    fn bidder(bid_or_zero: u32) -> AuctionBidderNotification {
        AuctionBidderNotification {
            house_id: 1,
            auction_id: 7,
            bidder_guid: 0x6C,
            bid_or_zero,
            out_bid: 5,
            item_entry: 2589,
            random_property_id: 0,
        }
    }

    #[test]
    fn a_bid_on_your_auction_updates_the_row_and_says_nothing() {
        let mut a = AuctionOpen::default();
        auction_owner_notification(&owner(0x6C, 500), &mut a);
        assert!(
            a.messages.is_empty(),
            "a live bid raises no message; `[0x4cd25f, 0x4cd3bd)` holds no display call"
        );

        // Sold: guid zeroed, and a price paid.
        let mut a = AuctionOpen::default();
        auction_owner_notification(&owner(0, 10_000), &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_SOLD_S", MsgKind::Chat, Some(2589))]
        );

        // Expired: guid zeroed, and nobody paid anything.
        let mut a = AuctionOpen::default();
        auction_owner_notification(&owner(0, 0), &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_EXPIRED_S", MsgKind::Chat, Some(2589))],
            "a zero bid on a closed auction is an expiry, not a sale"
        );
    }

    #[test]
    fn a_zero_bid_field_means_you_won() {
        let mut a = AuctionOpen::default();
        auction_bidder_notification(&bidder(0), &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_WON_S", MsgKind::Chat, Some(2589))]
        );

        let mut a = AuctionOpen::default();
        auction_bidder_notification(&bidder(9_000), &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_OUTBID_S", MsgKind::Chat, Some(2589))]
        );
    }

    #[test]
    fn the_outcomes_are_chat_and_the_refusals_are_the_error_frame() {
        let tail = AuctionCommandTail::Empty;
        for (action, key) in [
            (auction_action::STARTED, "ERR_AUCTION_STARTED"),
            (auction_action::REMOVED, "ERR_AUCTION_REMOVED"),
            (auction_action::BID_PLACED, "ERR_AUCTION_BID_PLACED"),
        ] {
            let mut a = AuctionOpen::default();
            auction_command_result(7, action, auction_error::OK, &tail, &mut a);
            assert_eq!(
                keys(&a),
                vec![(key, MsgKind::Chat, None)],
                "a successful {action} says so in chat, with no item fill"
            );
        }

        let mut a = AuctionOpen::default();
        auction_removed_notification(2589, &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_REMOVED_S", MsgKind::Chat, Some(2589))]
        );

        // A refusal, on the red frame.
        let mut a = AuctionOpen::default();
        auction_command_result(0, auction_action::BID_PLACED, 10, &tail, &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_BID_OWN", MsgKind::Error, None)]
        );

        // `HIGHER_BID` patches the row; the outbid line comes from the bidder notification.
        let mut a = AuctionOpen::default();
        auction_command_result(7, auction_action::BID_PLACED, 5, &tail, &mut a);
        assert!(
            a.messages.is_empty(),
            "code 5 patches the row, it does not talk"
        );
    }

    #[test]
    fn closing_the_window_does_not_swallow_a_pending_notice() {
        let mut a = AuctionOpen::default();
        a.open(0x42, 1);
        auction_owner_notification(&owner(0, 10_000), &mut a);
        a.clear();
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_SOLD_S", MsgKind::Chat, Some(2589))]
        );
    }
}
