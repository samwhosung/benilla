//! Golden tests for the auction house wire: every client body byte-exact, and hand-built server
//! bodies through `parse_server` and `decode`.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{
    self, auction_action, auction_duration, auction_error, auction_filter,
    AuctionBidderNotification, AuctionCommandTail, AuctionListEntry, AuctionOwnerNotification,
    AUCTION_RECORD_BYTES,
};
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

const AUCTIONEER: u64 = 0x00F1_3000_0000_0055;

#[test]
fn auction_send_bodies_golden() {
    // MSG_AUCTION_HELLO: the auctioneer's full guid and nothing else.
    assert_eq!(
        messages::auction_hello(AUCTIONEER),
        hx("550000000030f100"),
        "MSG_AUCTION_HELLO body"
    );

    assert_eq!(
        messages::auction_sell_item(
            AUCTIONEER,
            0x0000_0000_0100_00AB,
            10_000,
            50_000,
            auction_duration::MEDIUM_MINUTES,
        ),
        hx(concat!(
            "550000000030f100", // auctioneer
            "ab00000100000000", // item guid
            "10270000",         // bid 10000
            "50c30000",         // buyout 50000
            "e0010000",         // etime 480 minutes
        )),
        "CMSG_AUCTION_SELL_ITEM body"
    );

    // CMSG_AUCTION_REMOVE_ITEM: u64 auctioneer, u32 auctionId.
    assert_eq!(
        messages::auction_remove_item(AUCTIONEER, 4242),
        hx("550000000030f10092100000"),
        "CMSG_AUCTION_REMOVE_ITEM body"
    );

    // CMSG_AUCTION_PLACE_BID: u64 auctioneer, u32 auctionId, u32 price.
    assert_eq!(
        messages::auction_place_bid(AUCTIONEER, 4242, 12_345),
        hx("550000000030f1009210000039300000"),
        "CMSG_AUCTION_PLACE_BID body"
    );

    // CMSG_AUCTION_LIST_OWNER_ITEMS: u64 auctioneer, u32 listfrom.
    assert_eq!(
        messages::auction_list_owner_items(AUCTIONEER, 50),
        hx("550000000030f10032000000"),
        "CMSG_AUCTION_LIST_OWNER_ITEMS body"
    );
}

#[test]
fn auction_list_items_body_golden() {
    // Ten fields and nothing after them: 1.12 has no sort column, sort count or padding.
    assert_eq!(
        messages::auction_list_items(AUCTIONEER, 50, "Copper", 10, 20, 1, 2, 3, 4, 1),
        hx(concat!(
            "550000000030f100", // auctioneer
            "32000000",         // listfrom 50
            "436f7070657200",   // "Copper\0"
            "0a",               // levelmin 10
            "14",               // levelmax 20
            "01000000",         // slotId
            "02000000",         // mainCategory
            "03000000",         // subCategory
            "04000000",         // quality, a minimum, not an equality
            "01",               // usable
        )),
        "CMSG_AUCTION_LIST_ITEMS body (filters set)"
    );

    // Every filter at its sentinel: the body that takes vmangos's no-filter fast path.
    let body = messages::auction_list_items(
        AUCTIONEER,
        0,
        "",
        auction_filter::ANY_LEVEL,
        auction_filter::ANY_LEVEL,
        auction_filter::ANY,
        auction_filter::ANY,
        auction_filter::ANY,
        auction_filter::ANY,
        auction_filter::ANY_USABILITY,
    );
    assert_eq!(
        body,
        hx(concat!(
            "550000000030f100", // auctioneer
            "00000000",         // listfrom 0
            "00",               // "": the lone NUL is the whole name field
            "00",               // levelmin 0
            "00",               // levelmax 0
            "ffffffff",         // slotId    ANY
            "ffffffff",         // mainCategory ANY
            "ffffffff",         // subCategory  ANY
            "ffffffff",         // quality      ANY
            "00",               // usable 0
        )),
        "CMSG_AUCTION_LIST_ITEMS body (empty name, all sentinels)"
    );
    // 8 + 4 + 1 + 1 + 1 + 4 * 4 + 1: the smallest browse body.
    assert_eq!(body.len(), 32, "no trailing sort bytes ride this opcode");
}

#[test]
fn auction_list_bidder_items_body_golden() {
    // u64 auctioneer, u32 listfrom, u32 id count: the count is present even with no ids.
    assert_eq!(
        messages::auction_list_bidder_items(AUCTIONEER, 0, &[]),
        hx("550000000030f1000000000000000000"),
        "CMSG_AUCTION_LIST_BIDDER_ITEMS body (no ids)"
    );

    assert_eq!(
        messages::auction_list_bidder_items(AUCTIONEER, 50, &[7, 4242, 0xDEAD_BEEF]),
        hx(concat!(
            "550000000030f100", // auctioneer
            "32000000",         // listfrom 50
            "03000000",         // 3 ids follow
            "07000000",         // id 7
            "92100000",         // id 4242
            "efbeadde",         // id 0xDEADBEEF
        )),
        "CMSG_AUCTION_LIST_BIDDER_ITEMS body (3 ids)"
    );
}

#[test]
fn auction_hello_reply_wire() {
    // The reply rides the request's opcode: u64 auctioneer, u32 houseId.
    let mut body = AUCTIONEER.to_le_bytes().to_vec();
    body.extend_from_slice(&6u32.to_le_bytes()); // houseId 6 (Orgrimmar), AuctionHouse.dbc 1..7
    match messages::parse_server(messages::opcode::MSG_AUCTION_HELLO, &body).unwrap() {
        ServerPacket::AuctionHello {
            auctioneer,
            house_id,
        } => assert_eq!((auctioneer, house_id), (AUCTIONEER, 6)),
        other => panic!("auction hello, got {}", other.name()),
    }
    let packet = messages::parse_server(messages::opcode::MSG_AUCTION_HELLO, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::AuctionHello {
            auctioneer,
            house_id,
        } => assert_eq!((auctioneer, house_id), (AUCTIONEER, 6)),
        other => panic!("auction hello event, got {other:?}"),
    }
}

/// Build one `SMSG_AUCTION_COMMAND_RESULT` head: `u32 auctionId, u32 action, u32 error`.
fn command_head(auction_id: u32, action: u32, error: u32) -> Vec<u8> {
    let mut body = auction_id.to_le_bytes().to_vec();
    body.extend_from_slice(&action.to_le_bytes());
    body.extend_from_slice(&error.to_le_bytes());
    body
}

fn parse_command(body: &[u8]) -> (u32, u32, u32, AuctionCommandTail) {
    match messages::parse_server(messages::opcode::SMSG_AUCTION_COMMAND_RESULT, body).unwrap() {
        ServerPacket::AuctionCommandResult {
            auction_id,
            action,
            error,
            tail,
        } => (auction_id, action, error, tail),
        other => panic!("auction command result, got {}", other.name()),
    }
}

#[test]
fn auction_command_result_bare_wire() {
    // OK carries a tail only with BID_PLACED, so a successful listing or cancel is bare.
    let body = command_head(4242, auction_action::STARTED, auction_error::OK);
    assert_eq!(body.len(), 12);
    assert_eq!(
        parse_command(&body),
        (
            4242,
            auction_action::STARTED,
            auction_error::OK,
            AuctionCommandTail::Empty
        )
    );

    assert_eq!(
        parse_command(&command_head(
            4242,
            auction_action::REMOVED,
            auction_error::OK
        ))
        .3,
        AuctionCommandTail::Empty
    );

    // `auction_id` is 0 when the server had no auction to name.
    assert_eq!(
        parse_command(&command_head(
            0,
            auction_action::BID_PLACED,
            auction_error::NOT_ENOUGH_MONEY
        )),
        (
            0,
            auction_action::BID_PLACED,
            auction_error::NOT_ENOUGH_MONEY,
            AuctionCommandTail::Empty
        )
    );

    // Codes 6, 8, 9, 11 and 12 have no vmangos name; the 1.12 client shows a generic failure.
    assert_eq!(
        parse_command(&command_head(0, auction_action::BID_PLACED, 9)).3,
        AuctionCommandTail::Empty
    );

    let packet = messages::parse_server(
        messages::opcode::SMSG_AUCTION_COMMAND_RESULT,
        &command_head(4242, auction_action::STARTED, auction_error::OK),
    )
    .unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::AuctionCommandResult {
            auction_id,
            action,
            error,
            tail,
        } => assert_eq!(
            (auction_id, action, error, tail),
            (
                4242,
                auction_action::STARTED,
                auction_error::OK,
                AuctionCommandTail::Empty
            )
        ),
        other => panic!("auction command result event, got {other:?}"),
    }
}

/// A tail shorter than its field reads short and the packet stands: `0x418e30` writes nothing
/// past the end, so result 1 keeps its seed and a placed or higher bid carries nothing, as a
/// higher bid missing its last dword does.
#[test]
fn auction_command_result_short_tail_reads_short() {
    for (action, error, lengths) in [
        (auction_action::STARTED, auction_error::INVENTORY, 1..4),
        (auction_action::BID_PLACED, auction_error::OK, 1..4),
        (auction_action::BID_PLACED, auction_error::HIGHER_BID, 1..16),
    ] {
        for short in lengths {
            let mut body = command_head(4242, action, error);
            body.extend(std::iter::repeat_n(0xAB, short));
            assert_eq!(
                parse_command(&body),
                (4242, action, error, AuctionCommandTail::Empty),
                "error {error}, {short} bytes"
            );
        }
    }
}

#[test]
fn auction_command_result_bid_placed_tail_wire() {
    let mut body = command_head(4242, auction_action::BID_PLACED, auction_error::OK);
    body.extend_from_slice(&617u32.to_le_bytes());
    assert_eq!(
        parse_command(&body),
        (
            4242,
            auction_action::BID_PLACED,
            auction_error::OK,
            AuctionCommandTail::BidPlaced {
                new_min_outbid: 617
            }
        )
    );
}

#[test]
fn auction_command_result_inventory_tail_wire() {
    // The INVENTORY tail is keyed on the error alone, whatever the action.
    let mut body = command_head(0, auction_action::STARTED, auction_error::INVENTORY);
    body.extend_from_slice(&2u32.to_le_bytes()); // EQUIP_ERR_* code
    assert_eq!(
        parse_command(&body),
        (
            0,
            auction_action::STARTED,
            auction_error::INVENTORY,
            AuctionCommandTail::Inventory { result: 2 }
        )
    );
}

#[test]
fn auction_command_result_higher_bid_tail_wire() {
    let mut body = command_head(4242, auction_action::BID_PLACED, auction_error::HIGHER_BID);
    body.extend_from_slice(&0x0000_0000_0000_07D1u64.to_le_bytes()); // new bidder
    body.extend_from_slice(&25_000u32.to_le_bytes()); // new bid
    body.extend_from_slice(&1_250u32.to_le_bytes()); // new min outbid
    assert_eq!(body.len(), 12 + 16);
    assert_eq!(
        parse_command(&body),
        (
            4242,
            auction_action::BID_PLACED,
            auction_error::HIGHER_BID,
            AuctionCommandTail::HigherBid {
                new_bidder_guid: 0x07D1,
                new_bid: 25_000,
                new_min_outbid: 1_250,
            }
        )
    );
}

/// Append one 64-byte list-result record in `AuctionEntry::BuildAuctionInfo`'s order.
fn push_record(body: &mut Vec<u8>, e: &AuctionListEntry) {
    let before = body.len();
    body.extend_from_slice(&e.auction_id.to_le_bytes());
    body.extend_from_slice(&e.item_entry.to_le_bytes());
    body.extend_from_slice(&e.perm_enchant.to_le_bytes());
    body.extend_from_slice(&e.random_property_id.to_le_bytes());
    body.extend_from_slice(&e.suffix_factor.to_le_bytes());
    body.extend_from_slice(&e.count.to_le_bytes());
    body.extend_from_slice(&e.spell_charges.to_le_bytes());
    body.extend_from_slice(&e.owner_guid.to_le_bytes());
    body.extend_from_slice(&e.start_bid.to_le_bytes());
    body.extend_from_slice(&e.min_increment.to_le_bytes());
    body.extend_from_slice(&e.buyout.to_le_bytes());
    body.extend_from_slice(&e.time_left_ms.to_le_bytes());
    body.extend_from_slice(&e.bidder_guid.to_le_bytes());
    body.extend_from_slice(&e.current_bid.to_le_bytes());
    // The record width is the reader's short-buffer bound.
    assert_eq!(body.len() - before, AUCTION_RECORD_BYTES);
    assert_eq!(AUCTION_RECORD_BYTES, 64);
}

/// A no-bids row: `min_increment`/`bidder_guid`/`current_bid` all zero, a buyout set.
fn unbid_row(auction_id: u32) -> AuctionListEntry {
    AuctionListEntry {
        auction_id,
        item_entry: 2589, // Linen Cloth
        perm_enchant: 0,
        random_property_id: 0,
        suffix_factor: 0,
        count: 20,
        spell_charges: 0,
        owner_guid: 0x0000_0000_0000_0101,
        start_bid: 5_000,
        min_increment: 0,
        buyout: 20_000,
        time_left_ms: 7_200_000,
        bidder_guid: 0,
        current_bid: 0,
    }
}

fn parse_list(opcode: u16, body: &[u8]) -> (Vec<AuctionListEntry>, u32) {
    match messages::parse_server(opcode, body).unwrap() {
        ServerPacket::AuctionListResult {
            auctions,
            total_count,
        }
        | ServerPacket::AuctionOwnerListResult {
            auctions,
            total_count,
        }
        | ServerPacket::AuctionBidderListResult {
            auctions,
            total_count,
        } => (auctions, total_count),
        other => panic!("auction list result, got {}", other.name()),
    }
}

#[test]
fn auction_list_result_empty_wire() {
    // Count 0, then a totalCount that is still nonzero when a page past the end is asked.
    let mut body = 0u32.to_le_bytes().to_vec();
    body.extend_from_slice(&37u32.to_le_bytes());
    assert_eq!(body.len(), 8);

    for opcode in [
        messages::opcode::SMSG_AUCTION_LIST_RESULT,
        messages::opcode::SMSG_AUCTION_OWNER_LIST_RESULT,
        messages::opcode::SMSG_AUCTION_BIDDER_LIST_RESULT,
    ] {
        let (auctions, total_count) = parse_list(opcode, &body);
        assert!(auctions.is_empty());
        assert_eq!(total_count, 37);
    }
}

#[test]
fn auction_list_result_records_wire() {
    let rows = [
        unbid_row(1),
        AuctionListEntry {
            auction_id: 2,
            item_entry: 12_640, // Lionheart Helm
            perm_enchant: 2_504,
            random_property_id: 0,
            suffix_factor: 0,
            count: 1,
            spell_charges: 0,
            owner_guid: 0x0000_0000_0000_0202,
            start_bid: 1_000_000,
            min_increment: 60_000,
            buyout: 0, // no buyout
            time_left_ms: 86_400_000,
            bidder_guid: 0x0000_0000_0000_0303,
            current_bid: 1_200_000,
        },
        unbid_row(3),
    ];
    let mut body = (rows.len() as u32).to_le_bytes().to_vec();
    for row in &rows {
        push_record(&mut body, row);
    }
    body.extend_from_slice(&129u32.to_le_bytes()); // totalCount, well past the 50-row page cap
    assert_eq!(body.len(), 4 + 3 * AUCTION_RECORD_BYTES + 4);

    let (auctions, total_count) = parse_list(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body);
    assert_eq!(auctions, rows);
    assert_eq!(total_count, 129);

    // `min_increment` is 0 until a bid, `buyout` 0 means none, `start_bid` is not the current bid.
    assert_eq!(auctions[0].min_increment, 0);
    assert_eq!(auctions[0].current_bid, 0);
    assert_eq!(auctions[1].buyout, 0);
    assert_ne!(auctions[1].start_bid, auctions[1].current_bid);

    let packet = messages::parse_server(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::AuctionListResult {
            auctions,
            total_count,
        } => {
            assert_eq!(auctions.len(), 3);
            assert_eq!(total_count, 129);
        }
        other => panic!("auction list result event, got {other:?}"),
    }

    let packet =
        messages::parse_server(messages::opcode::SMSG_AUCTION_OWNER_LIST_RESULT, &body).unwrap();
    assert!(matches!(
        decode(packet).pop().unwrap(),
        SessionEvent::AuctionOwnerListResult { .. }
    ));
    let packet =
        messages::parse_server(messages::opcode::SMSG_AUCTION_BIDDER_LIST_RESULT, &body).unwrap();
    assert!(matches!(
        decode(packet).pop().unwrap(),
        SessionEvent::AuctionBidderListResult { .. }
    ));
}

#[test]
fn auction_list_result_total_count_rides_after_the_records() {
    // Neither the leading count nor any record field is 9999, so only the trailing total can be.
    let row = AuctionListEntry {
        auction_id: 11,
        item_entry: 12,
        perm_enchant: 13,
        random_property_id: 14,
        suffix_factor: 15,
        count: 16,
        spell_charges: 17,
        owner_guid: 18,
        start_bid: 19,
        min_increment: 20,
        buyout: 21,
        time_left_ms: 22,
        bidder_guid: 23,
        current_bid: 24,
    };
    let mut body = 1u32.to_le_bytes().to_vec();
    push_record(&mut body, &row);
    body.extend_from_slice(&9_999u32.to_le_bytes());

    let (auctions, total_count) = parse_list(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body);
    assert_eq!(auctions, [row], "the record reads intact");
    assert_eq!(
        total_count, 9_999,
        "totalCount comes from AFTER the records, not from the head or a record field"
    );
}

#[test]
fn auction_list_record_round_trips_negative_signed_fields() {
    // Both are int32: a negative property id is a suffix; negative charges destroy when spent.
    let row = AuctionListEntry {
        auction_id: 77,
        item_entry: 7_078,
        perm_enchant: 0,
        random_property_id: -19, // a suffix id
        suffix_factor: 143,
        count: 1,
        spell_charges: -5, // 5 charges, then destroy
        owner_guid: 0x0000_0000_0000_0404,
        start_bid: 1,
        min_increment: 0,
        buyout: 0,
        time_left_ms: 1_000,
        bidder_guid: 0,
        current_bid: 0,
    };
    let mut body = 1u32.to_le_bytes().to_vec();
    push_record(&mut body, &row);
    body.extend_from_slice(&1u32.to_le_bytes());

    let (auctions, _) = parse_list(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body);
    assert_eq!(auctions[0].random_property_id, -19);
    assert_eq!(auctions[0].spell_charges, -5);
    assert_eq!(auctions[0].suffix_factor, 143);

    // An expired but unswept auction's time left wraps instead of clamping; it passes through.
    let mut expired = unbid_row(78);
    expired.time_left_ms = u32::MAX - 500; // (expire - now) * 1000 on a negative difference
    let mut body = 1u32.to_le_bytes().to_vec();
    push_record(&mut body, &expired);
    body.extend_from_slice(&1u32.to_le_bytes());
    let (auctions, _) = parse_list(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body);
    assert_eq!(auctions[0].time_left_ms, u32::MAX - 500);
}

#[test]
fn auction_list_result_survives_a_count_larger_than_the_records() {
    // vmangos's browse fast path counts a stale auction whose item is gone but writes no bytes.
    let rows = [unbid_row(1), unbid_row(2)];
    let mut body = 3u32.to_le_bytes().to_vec(); // the server's inflated count
    for row in &rows {
        push_record(&mut body, row);
    }
    body.extend_from_slice(&2u32.to_le_bytes()); // totalCount still rides at the end

    let (auctions, total_count) = parse_list(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body);
    assert_eq!(auctions, rows, "the records that DID arrive come back");
    assert_eq!(total_count, 2);

    // With the trailing total missing too, the total falls back to the records read.
    let mut body = 3u32.to_le_bytes().to_vec();
    for row in &rows {
        push_record(&mut body, row);
    }
    let (auctions, total_count) = parse_list(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body);
    assert_eq!(auctions, rows);
    assert_eq!(total_count, 2, "falls back to the records actually read");

    // A nonsense count yields the one record the buffer holds.
    let mut body = u32::MAX.to_le_bytes().to_vec();
    push_record(&mut body, &rows[0]);
    let (auctions, _) = parse_list(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body);
    assert_eq!(auctions, [rows[0]]);
}

#[test]
fn auction_bidder_notification_wire() {
    // House id first and the guid third, unlike the owner notification.
    let mut body = 6u32.to_le_bytes().to_vec(); // houseId
    body.extend_from_slice(&4242u32.to_le_bytes()); // auctionId
    body.extend_from_slice(&0x0000_0000_0000_0505u64.to_le_bytes()); // bidderGuid
    body.extend_from_slice(&15_000u32.to_le_bytes()); // bidOrZero, nonzero: outbid
    body.extend_from_slice(&750u32.to_le_bytes()); // outBid
    body.extend_from_slice(&12_640u32.to_le_bytes()); // itemEntry
    body.extend_from_slice(&(-19i32).to_le_bytes()); // randomPropertyId, signed

    let expected = AuctionBidderNotification {
        house_id: 6,
        auction_id: 4242,
        bidder_guid: 0x0505,
        bid_or_zero: 15_000,
        out_bid: 750,
        item_entry: 12_640,
        random_property_id: -19,
    };
    match messages::parse_server(messages::opcode::SMSG_AUCTION_BIDDER_NOTIFICATION, &body).unwrap()
    {
        ServerPacket::AuctionBidderNotification(n) => assert_eq!(n, expected),
        other => panic!("auction bidder notification, got {}", other.name()),
    }

    // bidOrZero 0 means won, not no bid.
    let mut won = body.clone();
    won[16..20].copy_from_slice(&0u32.to_le_bytes());
    match messages::parse_server(messages::opcode::SMSG_AUCTION_BIDDER_NOTIFICATION, &won).unwrap()
    {
        ServerPacket::AuctionBidderNotification(n) => {
            assert_eq!(n.bid_or_zero, 0, "0 = WON");
            assert_eq!(n.auction_id, 4242, "the rest of the row is unmoved");
            assert_eq!(n.out_bid, 750);
        }
        other => panic!("auction bidder notification (won), got {}", other.name()),
    }

    // The owner reader takes 28 of these 32 bytes and misplaces every field.
    match messages::parse_server(messages::opcode::SMSG_AUCTION_OWNER_NOTIFICATION, &body).unwrap()
    {
        ServerPacket::AuctionOwnerNotification(n) => {
            assert_eq!(
                n.auction_id, 6,
                "the owner reader eats houseId as auctionId"
            );
            assert_ne!(n.bidder_guid, expected.bidder_guid, "the guid slides");
            assert_ne!(n.item_entry, expected.item_entry);
        }
        other => panic!("auction owner notification, got {}", other.name()),
    }

    let packet =
        messages::parse_server(messages::opcode::SMSG_AUCTION_BIDDER_NOTIFICATION, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::AuctionBidderNotification(n) => assert_eq!(n, expected),
        other => panic!("auction bidder notification event, got {other:?}"),
    }
}

#[test]
fn auction_owner_notification_wire() {
    // No house id and the guid fourth, unlike the bidder notification.
    let mut body = 4242u32.to_le_bytes().to_vec(); // auctionId
    body.extend_from_slice(&15_000u32.to_le_bytes()); // bid
    body.extend_from_slice(&750u32.to_le_bytes()); // outBid
    body.extend_from_slice(&0x0000_0000_0000_0505u64.to_le_bytes()); // bidderGuid
    body.extend_from_slice(&12_640u32.to_le_bytes()); // itemEntry
    body.extend_from_slice(&(-19i32).to_le_bytes()); // randomPropertyId, signed
    assert_eq!(body.len(), 28, "four bytes shorter than the bidder notice");

    let expected = AuctionOwnerNotification {
        auction_id: 4242,
        bid: 15_000,
        out_bid: 750,
        bidder_guid: 0x0505,
        item_entry: 12_640,
        random_property_id: -19,
    };
    match messages::parse_server(messages::opcode::SMSG_AUCTION_OWNER_NOTIFICATION, &body).unwrap()
    {
        ServerPacket::AuctionOwnerNotification(n) => assert_eq!(n, expected),
        other => panic!("auction owner notification, got {}", other.name()),
    }

    // An all-zero bidder guid means sold, not a missing bidder.
    let mut sold = body.clone();
    sold[12..20].copy_from_slice(&0u64.to_le_bytes());
    match messages::parse_server(messages::opcode::SMSG_AUCTION_OWNER_NOTIFICATION, &sold).unwrap()
    {
        ServerPacket::AuctionOwnerNotification(n) => {
            assert_eq!(n.bidder_guid, 0, "0 = sold");
            assert_eq!(n.item_entry, 12_640, "the rest of the row is unmoved");
        }
        other => panic!("auction owner notification (sold), got {}", other.name()),
    }

    let packet =
        messages::parse_server(messages::opcode::SMSG_AUCTION_OWNER_NOTIFICATION, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::AuctionOwnerNotification(n) => assert_eq!(n, expected),
        other => panic!("auction owner notification event, got {other:?}"),
    }
}

#[test]
fn auction_removed_notification_wire() {
    // SMSG_AUCTION_REMOVED_NOTIFICATION: u32 auctionId, u32 itemEntry, i32 randomPropertyId.
    let mut body = 4242u32.to_le_bytes().to_vec();
    body.extend_from_slice(&2589u32.to_le_bytes());
    body.extend_from_slice(&(-7i32).to_le_bytes());
    match messages::parse_server(messages::opcode::SMSG_AUCTION_REMOVED_NOTIFICATION, &body)
        .unwrap()
    {
        ServerPacket::AuctionRemovedNotification {
            auction_id,
            item_entry,
            random_property_id,
        } => assert_eq!(
            (auction_id, item_entry, random_property_id),
            (4242, 2589, -7)
        ),
        other => panic!("auction removed notification, got {}", other.name()),
    }

    let packet =
        messages::parse_server(messages::opcode::SMSG_AUCTION_REMOVED_NOTIFICATION, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::AuctionRemovedNotification {
            auction_id,
            item_entry,
            random_property_id,
        } => assert_eq!(
            (auction_id, item_entry, random_property_id),
            (4242, 2589, -7)
        ),
        other => panic!("auction removed notification event, got {other:?}"),
    }
}
