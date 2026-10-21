//! The auction house's packet handlers: every `SMSG_AUCTION_*` becomes state on [`AuctionOpen`],
//! which [`super::feed_auction`] turns into the window's events, and the requests a result sends
//! go out here; nothing here touches the VM.

use benilla_protocol::messages::{
    auction_action, auction_error, AuctionBidderNotification, AuctionCommandTail, AuctionListEntry,
    AuctionOwnerNotification,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{BIDDER, LIST, OWNER};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{AuctionMessage, AuctionOpen, RowPatch};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};

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

/// What a handler reads besides the house: the socket a result's requests go out on, the VM
/// clock's tick the bid notices' deadline floor counts from (`0x42c010`), and the player's guid a
/// placed bid writes into its row (`0x468550`).
#[derive(SystemParam)]
pub(super) struct HouseWire<'w> {
    commands: Option<Res<'w, NetCommands>>,
    clock: Option<Res<'w, crate::ui_script::UiClock>>,
    me: Option<Res<'w, crate::net::SelfGuid>>,
}

impl HouseWire<'_> {
    fn view(&self) -> Wire<'_> {
        Wire {
            commands: self.commands.as_deref(),
            now: self
                .clock
                .as_ref()
                .map_or(0, |c| benilla_ui::script::auction_tick_ms(c.ui_now)),
            me: self.me.as_ref().and_then(|m| m.0).unwrap_or(0),
        }
    }
}

/// [`HouseWire`] as the handlers read it.
#[derive(Clone, Copy)]
pub(super) struct Wire<'a> {
    pub(super) commands: Option<&'a NetCommands>,
    /// The tick now, on `GetTime`'s clock.
    pub(super) now: u32,
    pub(super) me: u64,
}

impl Wire<'_> {
    fn send(&self, command: ClientCommand) {
        if let Some(commands) = self.commands {
            let _ = commands.0.send(command);
        }
    }
}

/// The `MSG_AUCTION_HELLO` reply opens the window, as in the reference's hello handler `0x4cc420`.
fn on_hello(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::AuctionHello {
        auctioneer,
        house_id,
    } = ev
    {
        commands.queue(move |world: &mut World| {
            super::open_auction(world, auctioneer, house_id);
        });
    }
}

fn on_command_result(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>, wire: HouseWire) {
    if let SessionEvent::AuctionCommandResult {
        auction_id,
        action,
        error,
        tail,
    } = ev
    {
        auction_command_result(auction_id, action, error, &tail, &mut auction, wire.view());
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

fn on_bidder_notification(
    In(ev): In<SessionEvent>,
    mut auction: ResMut<AuctionOpen>,
    wire: HouseWire,
) {
    if let SessionEvent::AuctionBidderNotification(n) = ev {
        auction_bidder_notification(&n, &mut auction, wire.view());
    }
}

fn on_owner_notification(
    In(ev): In<SessionEvent>,
    mut auction: ResMut<AuctionOpen>,
    wire: HouseWire,
) {
    if let SessionEvent::AuctionOwnerNotification(n) = ev {
        auction_owner_notification(&n, &mut auction, wire.view());
    }
}

fn on_removed_notification(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionRemovedNotification {
        auction_id,
        item_entry,
        ..
    } = ev
    {
        auction_removed_notification(auction_id, item_entry, &mut auction);
    }
}

/// The window dies with the socket: the server re-validates the auctioneer on every command, so
/// a window kept across a reconnect would fail silently.
fn on_session_end(In(_): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    auction.clear_session();
}

/// Any of the three list results, which share a record and differ only in their tab. A result is
/// written whatever the session, as its handler reads no auctioneer (`0x4cc7f0`, `0x4cca30`,
/// `0x4ccc80`): one in flight at the close lands in the lists the close left standing.
fn auction_list(
    which: usize,
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    auction.wire.list_results[which] += 1;
    auction.set_list(which, auctions, total_count);
}

fn auction_list_result(
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    auction_list(LIST, auctions, total_count, auction);
}

/// The owner page; a count other than the total arms the owner request (`0x4ccc2d`-`0x4ccc35`).
fn auction_owner_list_result(
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    if auctions.len() as u32 != total_count {
        auction.owner_armed = true;
    }
    auction_list(OWNER, auctions, total_count, auction);
}

/// The bidder page, deduped by auction id: the server lists the refreshed ids, then every auction
/// we lead, so one auction can appear twice. The wire's count other than its total arms the
/// bidder request (`0x4cce7d`-`0x4cce85`).
fn auction_bidder_list_result(
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    if auctions.len() as u32 != total_count {
        auction.bidder_armed = true;
    }
    let mut seen = std::collections::HashSet::with_capacity(auctions.len());
    let deduped: Vec<AuctionListEntry> = auctions
        .into_iter()
        .filter(|e| seen.insert(e.auction_id))
        .collect();
    auction_list(BIDDER, deduped, total_count, auction);
}

/// The inventory result the reference reads when the fourth dword is short: `0x4cc472` seeds it
/// with 40 and a short read leaves it (`0x418e30`).
const SHORT_INVENTORY_RESULT: u32 = 0x28;

/// `SMSG_AUCTION_COMMAND_RESULT` (`0x4cc460`), the verdict on a sell, a cancel or a bid, through
/// the jump table `0x4cc7b4`. `auction_id` is 0 on most failures.
fn auction_command_result(
    auction_id: u32,
    action: u32,
    error: u32,
    tail: &AuctionCommandTail,
    auction: &mut AuctionOpen,
    wire: Wire,
) {
    // For the live probe: a success otherwise leaves no trace but its request.
    auction.wire.last_command = Some((auction_id, action, error));
    match error {
        auction_error::OK => match action {
            // Started (`0x4cc4c7`): its line, the owner request armed and sent again at its
            // saved offset (`0x4cc4da`, `0x4cc4e0`).
            auction_action::STARTED => {
                auction
                    .messages
                    .push(AuctionMessage::chat("ERR_AUCTION_STARTED"));
                auction.owner_armed = true;
                if let Some(commands) = wire.commands {
                    let offset = auction.owner_offset;
                    auction.send_owner_list(commands, offset);
                }
            }
            // Cancelled (`0x4cc648`): its line and the auction removed (`0x4cc658`).
            auction_action::REMOVED => {
                auction
                    .messages
                    .push(AuctionMessage::chat("ERR_AUCTION_REMOVED"));
                auction.remove_auction(auction_id);
            }
            auction_action::BID_PLACED => {
                auction
                    .messages
                    .push(AuctionMessage::chat("ERR_AUCTION_BID_PLACED"));
                bid_placed(auction_id, tail, auction, wire);
            }
            // Any other action shows nothing (`0x4cc642`).
            _ => {}
        },
        // `HIGHER_BID` (`0x4cc672`): with the window open the browse and bidder rows take the
        // packet's bidder, bid and increment, and every exit shows the line (`0x4cc76e`,
        // `0x4cc728`).
        auction_error::HIGHER_BID => {}
        _ => {
            let inventory_result = match tail {
                AuctionCommandTail::Inventory { result } => *result,
                _ => SHORT_INVENTORY_RESULT,
            };
            if let Some(key) = command_error_key(error, inventory_result) {
                auction.messages.push(AuctionMessage::error(key));
            }
        }
    }
}

/// A placed bid (`0x4cc4f3`): its outbid pair removed (`0x4cc517`), the bidder request armed and
/// sent again at its saved offset (`0x4cc522`, `0x4cc528`); with the window open the browse row
/// takes the bid last sent, the packet's increment, the player as bidder and a deadline at least
/// 90 s out (`0x4cc564`-`0x4cc5c0`); then, open or not, the wave (`0x4cc5dd`-`0x4cc60c`).
fn bid_placed(auction_id: u32, tail: &AuctionCommandTail, auction: &mut AuctionOpen, wire: Wire) {
    auction.outbid_remove(auction_id);
    auction.bidder_armed = true;
    if let Some(commands) = wire.commands {
        let offset = auction.bidder_offset;
        auction.send_bidder_list(commands, offset);
    }
    let _ = (tail, EMOTE_WAVE, wire.me, wire.now);
}

/// `EMOTE_ONESHOT_WAVE`, the placed bid's emote (`0x4cc5fc push 3`).
const EMOTE_WAVE: u32 = 3;

/// A failed command's GlobalStrings key, the message the jump table `0x4cc7b4` pushes to
/// `0x496720`: 1 maps the inventory result through `0x622630`, 3 `0x25`, 4 `0x17`, 7 `0x174`, 10
/// `0x173`, 13 `0x1be`, and 2, 6, 8, 9, 11, 12 and past 13 `0x172` (`0x4cc799`). `None` where the
/// display shows nothing.
fn command_error_key(error: u32, inventory_result: u32) -> Option<&'static str> {
    Some(match error {
        auction_error::INVENTORY => {
            let _ = inventory_result;
            "ERR_AUCTION_DATABASE_ERROR"
        }
        auction_error::NOT_ENOUGH_MONEY => "ERR_NOT_ENOUGH_MONEY", // `0x25`
        auction_error::BID_INCREMENT => "ERR_AUCTION_BID_INCREMENT", // `0x174`
        auction_error::BID_OWN => "ERR_AUCTION_BID_OWN",           // `0x173`
        _ => "ERR_AUCTION_DATABASE_ERROR",                         // `0x172`
    })
}

/// `SMSG_AUCTION_BIDDER_NOTIFICATION` (`0x4cced0`): a zero `bid_or_zero` means we won, else we
/// were outbid.
fn auction_bidder_notification(
    notice: &AuctionBidderNotification,
    auction: &mut AuctionOpen,
    wire: Wire,
) {
    let won = notice.bid_or_zero == 0;
    auction.messages.push(AuctionMessage::chat_item(
        if won {
            "ERR_AUCTION_WON_S"
        } else {
            "ERR_AUCTION_OUTBID_S"
        },
        notice.item_entry,
    ));
    // Won: the auction removed from the lists (`0x4cd173`).
    if won {
        auction.remove_auction(notice.auction_id);
        return;
    }
    // Outbid: the pair kept whatever the house (`0x4ccff3`); in the open house the bidder and
    // browse rows take the bid, the increment, the new bidder and the 90 s floor
    // (`0x4cd033`-`0x4cd132`). A closed window's house is 0, which no notice names.
    auction.outbid_append(notice.house_id, notice.auction_id);
    let _ = wire;
}

/// `SMSG_AUCTION_OWNER_NOTIFICATION` (`0x4cd1f0`): one of ours took a bid, sold or expired.
fn auction_owner_notification(
    notice: &AuctionOwnerNotification,
    auction: &mut AuctionOpen,
    wire: Wire,
) {
    // A bidder guid means a new bid, which the reference does not announce (no display call in
    // `[0x4cd25f, 0x4cd3bd)`): with the window open (`0x4cd26b`) the owner and browse rows take
    // it, with the 90 s floor (`0x4cd296`-`0x4cd3a8`).
    if notice.bidder_guid != 0 {
        let _ = wire;
        return;
    }
    // A zero guid closes it, sold with a bid (`0x4cd43e`), and removes it (`0x4cd468`).
    auction.messages.push(AuctionMessage::chat_item(
        if notice.bid != 0 {
            "ERR_AUCTION_SOLD_S"
        } else {
            "ERR_AUCTION_EXPIRED_S"
        },
        notice.item_entry,
    ));
    auction.remove_auction(notice.auction_id);
}

/// `SMSG_AUCTION_REMOVED_NOTIFICATION` (`0x4cd480`): the seller cancelled an auction we bid on;
/// it always prints `ERR_AUCTION_REMOVED_S` (`0x4cd540`) and removes the auction (`0x4cd550`).
fn auction_removed_notification(auction_id: u32, item_entry: u32, auction: &mut AuctionOpen) {
    auction.messages.push(AuctionMessage::chat_item(
        "ERR_AUCTION_REMOVED_S",
        item_entry,
    ));
    auction.remove_auction(auction_id);
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

    const ME: u64 = 0x77;
    const NOW: u32 = 1_000_000;
    /// Commands as their debug text, `ClientCommand` having no `PartialEq`.
    fn text(commands: impl IntoIterator<Item = ClientCommand>) -> Vec<String> {
        commands.into_iter().map(|c| format!("{c:?}")).collect()
    }
    const AUCTIONEER: u64 = 0x42;

    /// The socket and the tick a handler reads, the sends caught.
    struct Rig {
        commands: NetCommands,
        rx: crossbeam_channel::Receiver<ClientCommand>,
    }

    impl Rig {
        fn new() -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            Self {
                commands: NetCommands(tx),
                rx,
            }
        }

        fn wire(&self) -> Wire<'_> {
            Wire {
                commands: Some(&self.commands),
                now: NOW,
                me: ME,
            }
        }

        fn sent(&self) -> Vec<ClientCommand> {
            self.rx.try_iter().collect()
        }
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

    /// A row of auction `auction_id`: 100c to start, no bid, an hour left.
    fn entry(auction_id: u32) -> AuctionListEntry {
        AuctionListEntry {
            auction_id,
            item_entry: 2589,
            perm_enchant: 0,
            random_property_id: 0,
            suffix_factor: 0,
            count: 1,
            spell_charges: 0,
            owner_guid: 0x55,
            start_bid: 100,
            min_increment: 0,
            buyout: 0,
            time_left_ms: 3_600_000,
            bidder_guid: 0,
            current_bid: 0,
        }
    }

    /// A house open at house 1 holding auctions 7 and 8 on all three lists, each read at `NOW`.
    fn house() -> AuctionOpen {
        let mut a = AuctionOpen::default();
        a.open(AUCTIONEER, 1);
        for list in [LIST, OWNER, BIDDER] {
            a.set_list(list, vec![entry(7), entry(8)], 2);
            a.lists[list].arrived_ms = Some(NOW);
        }
        a
    }

    fn row(a: &AuctionOpen, list: usize, auction_id: u32) -> Option<&AuctionListEntry> {
        a.lists[list]
            .entries
            .iter()
            .find(|e| e.auction_id == auction_id)
    }

    #[test]
    fn a_bid_on_your_auction_updates_the_row_and_says_nothing() {
        let rig = Rig::new();
        let mut a = AuctionOpen::default();
        auction_owner_notification(&owner(0x6C, 500), &mut a, rig.wire());
        assert!(
            a.messages.is_empty(),
            "a live bid raises no message; `[0x4cd25f, 0x4cd3bd)` holds no display call"
        );

        // Sold: guid zeroed, and a price paid.
        let mut a = AuctionOpen::default();
        auction_owner_notification(&owner(0, 10_000), &mut a, rig.wire());
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_SOLD_S", MsgKind::Chat, Some(2589))]
        );

        // Expired: guid zeroed, and nobody paid anything.
        let mut a = AuctionOpen::default();
        auction_owner_notification(&owner(0, 0), &mut a, rig.wire());
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_EXPIRED_S", MsgKind::Chat, Some(2589))],
            "a zero bid on a closed auction is an expiry, not a sale"
        );
    }

    /// A bid on one of ours (`0x4cd25f`-`0x4cd3a8`): with the window open (`0x4cd26b`) the owner
    /// and browse rows take the notice's bid, increment and bidder and the 90 s floor; the bidder
    /// row and a closed window's rows stay.
    #[test]
    fn a_bid_on_your_auction_patches_the_owner_and_browse_rows() {
        let rig = Rig::new();
        let mut a = house();
        a.lists[OWNER].entries[0].time_left_ms = 1_000;
        auction_owner_notification(&owner(0x6C, 500), &mut a, rig.wire());
        for list in [OWNER, LIST] {
            let e = row(&a, list, 7).unwrap();
            assert_eq!(
                (e.current_bid, e.min_increment, e.bidder_guid),
                (500, 5, 0x6C)
            );
        }
        assert_eq!(row(&a, OWNER, 7).unwrap().time_left_ms, 90_000);
        assert_eq!(row(&a, LIST, 7).unwrap().time_left_ms, 3_600_000);
        assert_eq!(row(&a, BIDDER, 7).unwrap().current_bid, 0);

        let mut a = house();
        a.clear();
        auction_owner_notification(&owner(0x6C, 500), &mut a, rig.wire());
        assert_eq!(row(&a, LIST, 7).unwrap().current_bid, 0);
        assert!(rig.sent().is_empty());
    }

    #[test]
    fn a_zero_bid_field_means_you_won() {
        let rig = Rig::new();
        let mut a = AuctionOpen::default();
        auction_bidder_notification(&bidder(0), &mut a, rig.wire());
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_WON_S", MsgKind::Chat, Some(2589))]
        );

        let mut a = AuctionOpen::default();
        auction_bidder_notification(&bidder(9_000), &mut a, rig.wire());
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_OUTBID_S", MsgKind::Chat, Some(2589))]
        );
    }

    #[test]
    fn the_outcomes_are_chat_and_the_refusals_are_the_error_frame() {
        let rig = Rig::new();
        let tail = AuctionCommandTail::Empty;
        for (action, key) in [
            (auction_action::STARTED, "ERR_AUCTION_STARTED"),
            (auction_action::REMOVED, "ERR_AUCTION_REMOVED"),
            (auction_action::BID_PLACED, "ERR_AUCTION_BID_PLACED"),
        ] {
            let mut a = AuctionOpen::default();
            auction_command_result(7, action, auction_error::OK, &tail, &mut a, rig.wire());
            assert_eq!(
                keys(&a),
                vec![(key, MsgKind::Chat, None)],
                "a successful {action} says so in chat, with no item fill"
            );
        }

        let mut a = AuctionOpen::default();
        auction_removed_notification(7, 2589, &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_REMOVED_S", MsgKind::Chat, Some(2589))]
        );

        // A refusal, on the red frame.
        let mut a = AuctionOpen::default();
        auction_command_result(0, auction_action::BID_PLACED, 10, &tail, &mut a, rig.wire());
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_BID_OWN", MsgKind::Error, None)]
        );
    }

    /// Error 5 (`0x4cc672`) always ends at `push 0x175`: with the window open it first patches the
    /// browse and bidder rows from the packet, and with the window closed (`0x4cc69f`) or the
    /// auction on no list (`0x4cc728`) it says the same.
    #[test]
    fn a_higher_bid_patches_the_rows_and_always_says_so() {
        let rig = Rig::new();
        let tail = AuctionCommandTail::HigherBid {
            new_bidder_guid: 0x6C,
            new_bid: 900,
            new_min_outbid: 45,
        };
        let higher = [("ERR_AUCTION_HIGHER_BID", MsgKind::Error, None)];
        let mut a = house();
        auction_command_result(7, auction_action::BID_PLACED, 5, &tail, &mut a, rig.wire());
        assert_eq!(keys(&a), higher);
        for list in [LIST, BIDDER] {
            let e = row(&a, list, 7).unwrap();
            assert_eq!(
                (
                    e.current_bid,
                    e.min_increment,
                    e.bidder_guid,
                    e.time_left_ms
                ),
                (900, 45, 0x6C, 3_600_000),
                "list {list}: the deadline stays"
            );
        }
        assert_eq!(
            row(&a, OWNER, 7).unwrap().current_bid,
            0,
            "not the owner row"
        );

        let mut a = house();
        auction_command_result(9, auction_action::BID_PLACED, 5, &tail, &mut a, rig.wire());
        assert_eq!(keys(&a), higher, "no row holds it");

        let mut a = house();
        a.clear();
        auction_command_result(7, auction_action::BID_PLACED, 5, &tail, &mut a, rig.wire());
        assert_eq!(keys(&a), higher, "the window closed");
        assert_eq!(
            row(&a, LIST, 7).unwrap().current_bid,
            0,
            "and nothing patched"
        );
        assert!(rig.sent().is_empty(), "a refusal sends nothing");
    }

    /// The jump table `0x4cc7b4`: 4 is `0x17` (`0x4cc78e`), 13 `0x1be` (`0x4cc792`), and 2, 6, 8,
    /// 9, 11, 12 and past 13 the default `0x172` (`0x4cc799`).
    #[test]
    fn each_refusal_shows_its_tables_line() {
        let rig = Rig::new();
        let shown = |error: u32, tail: AuctionCommandTail| {
            let mut a = AuctionOpen::default();
            auction_command_result(
                0,
                auction_action::BID_PLACED,
                error,
                &tail,
                &mut a,
                rig.wire(),
            );
            a.messages.iter().map(|m| m.key).collect::<Vec<_>>()
        };
        let e = || AuctionCommandTail::Empty;
        assert_eq!(shown(3, e()), ["ERR_NOT_ENOUGH_MONEY"]);
        assert_eq!(shown(4, e()), ["ERR_ITEM_NOT_FOUND"]);
        assert_eq!(shown(7, e()), ["ERR_AUCTION_BID_INCREMENT"]);
        assert_eq!(shown(13, e()), ["ERR_RESTRICTED_ACCOUNT"]);
        for error in [2, 6, 8, 9, 11, 12, 14, 999] {
            assert_eq!(shown(error, e()), ["ERR_AUCTION_DATABASE_ERROR"], "{error}");
        }
    }

    /// Error 1 maps its fourth dword through `0x622630` (`0x4cc783`): 23 is `ERR_ITEM_NOT_FOUND`,
    /// 0 is the id the display refuses, and past 66, the whole dword tested, `ERR_BAG_FULL`. A
    /// short packet leaves the dword at its seed, 40 (`0x4cc472`).
    #[test]
    fn an_inventory_refusal_shows_the_inventory_results_line() {
        let rig = Rig::new();
        let shown = |tail: AuctionCommandTail| {
            let mut a = AuctionOpen::default();
            auction_command_result(0, auction_action::STARTED, 1, &tail, &mut a, rig.wire());
            a.messages.iter().map(|m| m.key).collect::<Vec<_>>()
        };
        let inv = |result| AuctionCommandTail::Inventory { result };
        assert_eq!(shown(inv(23)), ["ERR_ITEM_NOT_FOUND"]);
        assert!(
            shown(inv(0)).is_empty(),
            "0x1d1 is past the display's table"
        );
        assert_eq!(shown(inv(67)), ["ERR_BAG_FULL"]);
        assert_eq!(shown(inv(256 + 23)), ["ERR_BAG_FULL"], "not narrowed to 23");
        assert_eq!(shown(inv(29)), ["ERR_NOT_ENOUGH_MONEY"]);
        assert_eq!(shown(AuctionCommandTail::Empty), ["ERR_INTERNAL_BAG_ERROR"]);
    }

    /// A placed bid (`0x4cc4f3`): its outbid pair goes (`0x4cc517`), the bidder request is armed
    /// and sent at its saved offset (`0x4cc522`, `0x4cc528`), the browse row takes the bid last
    /// sent, the packet's increment, the player and a deadline at least 90 s out
    /// (`0x4cc564`-`0x4cc5c0`), and `CMSG_EMOTE` 3 goes out (`0x4cc5dd`-`0x4cc60c`).
    #[test]
    fn a_placed_bid_patches_the_browse_row_and_waves() {
        let rig = Rig::new();
        let mut a = house();
        a.lists[LIST].entries[0].time_left_ms = 30_000;
        a.outbid_append(1, 7);
        a.outbid_append(1, 9);
        a.bidder_armed = false;
        a.bidder_offset = 50;
        a.last_bid = 650;
        let tail = AuctionCommandTail::BidPlaced { new_min_outbid: 33 };
        auction_command_result(7, auction_action::BID_PLACED, 0, &tail, &mut a, rig.wire());
        let e = row(&a, LIST, 7).unwrap();
        assert_eq!(
            (
                e.current_bid,
                e.min_increment,
                e.bidder_guid,
                e.time_left_ms
            ),
            (650, 33, ME, 90_000)
        );
        assert_eq!(
            row(&a, BIDDER, 7).unwrap().current_bid,
            0,
            "the browse row alone"
        );
        assert!(
            a.outbid.is_empty(),
            "auction 7's pair by the bid (`0x4cc517`), house 1's first left by the request \
             (`0x4cd808`)"
        );
        assert!(!a.bidder_armed, "the request went out");
        assert_eq!(
            text(rig.sent()),
            text([
                ClientCommand::AuctionListBidderItems {
                    auctioneer: AUCTIONEER,
                    list_from: 50,
                    auction_ids: vec![9],
                },
                ClientCommand::Emote { emote: 3 },
            ])
        );

        // A deadline past the floor stays; with the window closed nothing is patched, and the
        // request and the wave still go.
        let mut a = house();
        a.last_bid = 650;
        auction_command_result(7, auction_action::BID_PLACED, 0, &tail, &mut a, rig.wire());
        assert_eq!(row(&a, LIST, 7).unwrap().time_left_ms, 3_600_000);
        let _ = rig.sent();
        a.clear();
        a.last_bid = 700;
        auction_command_result(8, auction_action::BID_PLACED, 0, &tail, &mut a, rig.wire());
        assert_eq!(row(&a, LIST, 8).unwrap().current_bid, 0);
        assert_eq!(
            text(rig.sent()),
            text([
                ClientCommand::AuctionListBidderItems {
                    auctioneer: 0,
                    list_from: 0,
                    auction_ids: vec![],
                },
                ClientCommand::Emote { emote: 3 },
            ]),
            "the sender tests no auctioneer (`0x4cd726`)"
        );
    }

    /// A started auction re-arms the owner request and sends it at its saved offset
    /// (`0x4cc4da`, `0x4cc4e0`).
    #[test]
    fn a_started_auction_asks_the_owner_list_again() {
        let rig = Rig::new();
        let mut a = house();
        a.owner_armed = false;
        a.owner_offset = 100;
        let tail = AuctionCommandTail::Empty;
        auction_command_result(7, auction_action::STARTED, 0, &tail, &mut a, rig.wire());
        assert_eq!(
            text(rig.sent()),
            text([ClientCommand::AuctionListOwnerItems {
                auctioneer: AUCTIONEER,
                list_from: 100,
            }])
        );
        assert!(!a.owner_armed);
    }

    /// The cancel reply, a win, a sale or expiry and the removed notice each remove the auction
    /// from every list that holds it (`0x4cc658`, `0x4cd173`, `0x4cd468`, `0x4cd550`), with its
    /// page per list for the feed, and send nothing; a closed window's lists change too.
    #[test]
    fn the_closing_notices_remove_the_auction_and_ask_nothing() {
        let rig = Rig::new();
        let remove = |f: &dyn Fn(&mut AuctionOpen)| {
            let mut a = house();
            a.clear();
            a.owner_armed = true;
            a.bidder_armed = true;
            let pages = a.lists.each_ref().map(|l| Some(l.page));
            f(&mut a);
            for list in [LIST, OWNER, BIDDER] {
                assert!(row(&a, list, 7).is_none(), "list {list}");
                assert!(row(&a, list, 8).is_some());
                assert_eq!(a.lists[list].total, 2, "the total stays");
            }
            assert_eq!(a.removals, [(7, pages)]);
        };
        let e = AuctionCommandTail::Empty;
        remove(&|a| auction_command_result(7, auction_action::REMOVED, 0, &e, a, rig.wire()));
        remove(&|a| auction_bidder_notification(&bidder(0), a, rig.wire()));
        remove(&|a| auction_owner_notification(&owner(0, 10_000), a, rig.wire()));
        remove(&|a| auction_owner_notification(&owner(0, 0), a, rig.wire()));
        remove(&|a| auction_removed_notification(7, 2589, a));
        assert!(rig.sent().is_empty(), "no list request");
    }

    /// Outbid (`0x4ccfed`-`0x4cd132`): the pair kept whatever the house (`0x4ccff3`); in the open
    /// house the bidder and browse rows take the notice's bid, increment and bidder and the 90 s
    /// floor; another house patches nothing.
    #[test]
    fn an_outbid_notice_keeps_the_pair_and_patches_the_open_houses_rows() {
        let rig = Rig::new();
        let mut a = house();
        a.lists[BIDDER].entries[0].time_left_ms = 1_000;
        auction_bidder_notification(&bidder(9_000), &mut a, rig.wire());
        assert_eq!(a.outbid, [(1, 7)]);
        for list in [BIDDER, LIST] {
            let e = row(&a, list, 7).unwrap();
            assert_eq!(
                (e.current_bid, e.min_increment, e.bidder_guid),
                (9_000, 5, 0x6C)
            );
        }
        assert_eq!(row(&a, BIDDER, 7).unwrap().time_left_ms, 90_000);
        assert_eq!(row(&a, OWNER, 7).unwrap().current_bid, 0);

        let mut a = house();
        let mut other = bidder(9_000);
        other.house_id = 6;
        auction_bidder_notification(&other, &mut a, rig.wire());
        assert_eq!(a.outbid, [(6, 7)]);
        assert_eq!(row(&a, LIST, 7).unwrap().current_bid, 0);
        assert!(rig.sent().is_empty());
    }

    /// The bidder request (`0x4cd720`) sends the open house's outbid ids, then removes the first
    /// pair of that house alone: `0x4cdf90`'s shift reuses the scan's index and ends it.
    #[test]
    fn the_bidder_request_sends_the_houses_ids_and_drops_one_pair() {
        let rig = Rig::new();
        let mut a = house();
        for pair in [(1, 7), (2, 8), (1, 9), (1, 10)] {
            a.outbid_append(pair.0, pair.1);
        }
        a.send_bidder_list(&rig.commands, 0);
        assert_eq!(
            text(rig.sent()),
            text([ClientCommand::AuctionListBidderItems {
                auctioneer: AUCTIONEER,
                list_from: 0,
                auction_ids: vec![7, 9, 10],
            }])
        );
        assert_eq!(a.outbid, [(2, 8), (1, 9), (1, 10)]);
        a.send_bidder_list(&rig.commands, 0);
        assert!(rig.sent().is_empty(), "the flag is spent");
    }

    /// `0x4cde50`: on a full list the room doubles and the pair is written past the count it does
    /// not raise (`0x4cdf02`-`0x4cdf1c`), so the ninth is lost and the tenth lands.
    #[test]
    fn a_full_outbid_list_grows_and_loses_the_pair_that_filled_it() {
        let mut a = AuctionOpen::default();
        for auction_id in 1..=10 {
            a.outbid_append(1, auction_id);
        }
        let ids: Vec<u32> = a.outbid.iter().map(|&(_, id)| id).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7, 8, 10]);
    }

    /// An owner or bidder result whose count is not its total arms its request (`0x4ccc35`,
    /// `0x4cce85`), the bidder's before its dedupe; the browse result has no such test.
    #[test]
    fn a_partial_owner_or_bidder_page_arms_its_request() {
        let mut a = AuctionOpen::default();
        auction_owner_list_result(vec![entry(1)], 1, &mut a);
        auction_bidder_list_result(vec![entry(1), entry(1)], 2, &mut a);
        assert!(!a.owner_armed && !a.bidder_armed, "whole pages");
        auction_owner_list_result(vec![entry(1)], 60, &mut a);
        auction_bidder_list_result(vec![entry(1)], 60, &mut a);
        assert!(a.owner_armed && a.bidder_armed);
    }

    #[test]
    fn closing_the_window_does_not_swallow_a_pending_notice() {
        let rig = Rig::new();
        let mut a = AuctionOpen::default();
        a.open(0x42, 1);
        auction_owner_notification(&owner(0, 10_000), &mut a, rig.wire());
        a.clear();
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_SOLD_S", MsgKind::Chat, Some(2589))]
        );
    }
}
