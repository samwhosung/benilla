//! The auction-house messages (vmangos `Server/Packets/AuctionHouse.cpp`): hello, the three list
//! pages, sell, bid, cancel, the command result and three notifications; 5875 has no
//! pending-sales opcode. Every guid is a plain `u64`, and the auctioneer guid heads every CMSG
//! because the server re-checks the 5 yd interact distance on each one.

use std::io;

use crate::wire::{capacity_hint, read_i32_le, read_u32_le, read_u64_le};

/// `SMSG_AUCTION_COMMAND_RESULT`'s `action` (`AuctionAction`, `AuctionHouseMgr.h:53-58`).
pub mod auction_action {
    /// Answers [`super::auction_sell_item`].
    pub const STARTED: u32 = 0;
    /// Answers [`super::auction_remove_item`].
    pub const REMOVED: u32 = 1;
    /// Answers [`super::auction_place_bid`], buyouts too (inferred, never flagged).
    pub const BID_PLACED: u32 = 2;
}

/// `SMSG_AUCTION_COMMAND_RESULT`'s `error` (`AuctionError`, `AuctionHouseMgr.h:40-51`). vmangos
/// never sends 6, 8, 9, 11 or 12; the reference shows any unknown code as a generic failure.
pub mod auction_error {
    pub const OK: u32 = 0;
    /// Carries an `EQUIP_ERR_*` tail ([`super::AuctionCommandTail::Inventory`]).
    pub const INVENTORY: u32 = 1;
    /// `ERR_AUCTION_DATABASE_ERROR`, also vmangos's catch-all (a bad `etime` lands here).
    pub const DATABASE: u32 = 2;
    pub const NOT_ENOUGH_MONEY: u32 = 3;
    pub const ITEM_NOT_FOUND: u32 = 4;
    /// Outbid in flight; carries the [`super::AuctionCommandTail::HigherBid`] tail.
    pub const HIGHER_BID: u32 = 5;
    /// The bid did not clear the minimum increment (5% of the current bid, floored, min 1 copper).
    pub const BID_INCREMENT: u32 = 7;
    pub const BID_OWN: u32 = 10;
    pub const RESTRICTED_ACCOUNT: u32 = 13;
}

/// The no-filter values of [`auction_list_items`] (`AuctionHouseObject::BuildListAuctionItems`,
/// which takes a whole-table fast path when all are unset).
pub mod auction_filter {
    /// For `slot_id`, `main_category`, `sub_category` and `quality`.
    pub const ANY: u32 = 0xFFFF_FFFF;
    /// For `level_min`/`level_max`, which gate `RequiredLevel`; `level_max` needs `level_min`.
    pub const ANY_LEVEL: u8 = 0;
    /// For `usable`; nonzero drops what `CanUseItem` refuses and recipes already known.
    pub const ANY_USABILITY: u8 = 0;
}

/// The three `etime` values of `CMSG_AUCTION_SELL_ITEM`, in minutes: 1, 4 and 12 times the 2 h
/// `MIN_AUCTION_TIME` (`AuctionHouseHandler.cpp:281-296`); the multiple also scales the deposit.
pub mod auction_duration {
    pub const SHORT_MINUTES: u32 = 120;
    pub const MEDIUM_MINUTES: u32 = 480;
    pub const LONG_MINUTES: u32 = 1440;
}

/// Records per list page (every server list builder stops at 50); `list_from` steps by it.
pub const AUCTION_PAGE_SIZE: u32 = 50;

/// A list record's fixed width: `7×u32, u64, 4×u32, u64, u32` (`AuctionEntry::BuildAuctionInfo`,
/// `AuctionHouseMgr.cpp:811-842`); it bounds the read of a body shorter than its `count`.
pub const AUCTION_RECORD_BYTES: usize = 64;

/// One row of any of the three list results; the item name is not on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuctionListEntry {
    pub auction_id: u32,
    pub item_entry: u32,
    /// The `PERM_ENCHANTMENT_SLOT` enchant, the only enchant a 1.12 auction row carries.
    pub perm_enchant: u32,
    /// Signed: negative is a random suffix id, positive a random property id.
    pub random_property_id: i32,
    pub suffix_factor: u32,
    /// The stack size.
    pub count: u32,
    /// Signed: negative means that many charges, then the item is destroyed (`GetSpellCharges`).
    pub spell_charges: i32,
    pub owner_guid: u64,
    /// The seller's opening price, fixed for the auction's life: what the first bid must meet.
    pub start_bid: u32,
    /// The next bid's step over [`Self::current_bid`] (5%, floored, min 1); 0 before any bid.
    pub min_increment: u32,
    /// 0 for none; a bid of at least a nonzero buyout is a buyout.
    pub buyout: u32,
    /// Milliseconds left, unclamped: an expired auction not yet swept wraps to near 2^32, which a
    /// consumer must read as expired.
    pub time_left_ms: u32,
    /// `0` = nobody has bid.
    pub bidder_guid: u64,
    /// `0` = no bid yet.
    pub current_bid: u32,
}

/// The tail after `SMSG_AUCTION_COMMAND_RESULT`'s three dwords, chosen by `error`, then by
/// `action` only on success (`AuctionHouseHandler.cpp:70-96`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuctionCommandTail {
    /// Every other pair, or a tail too short for its fields: the three dwords are all there is.
    Empty,
    /// A placed bid: the step the next bid must clear, from the bid just accepted.
    BidPlaced { new_min_outbid: u32 },
    /// [`auction_error::INVENTORY`]: an `InventoryResult` (`EQUIP_ERR_*`) code.
    Inventory { result: u32 },
    /// [`auction_error::HIGHER_BID`]: who outbid us, at what bid, and the next step to clear.
    HigherBid {
        new_bidder_guid: u64,
        new_bid: u32,
        new_min_outbid: u32,
    },
}

/// `SMSG_AUCTION_BIDDER_NOTIFICATION`, won or outbid (`Server/Packets/AuctionHouse.cpp:78-87`);
/// it leads with the house id and has the guid third, unlike the owner notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuctionBidderNotification {
    /// `AuctionHouse.dbc` row (1..7), as in the hello reply.
    pub house_id: u32,
    pub auction_id: u32,
    pub bidder_guid: u64,
    /// 0 means won (`won ? 0 : auction->bid`); otherwise the bid that beat us.
    pub bid_or_zero: u32,
    /// The step the next bid must clear (`GetAuctionOutBid()`).
    pub out_bid: u32,
    pub item_entry: u32,
    /// Signed, as on [`AuctionListEntry::random_property_id`]; 0 when the item is gone.
    pub random_property_id: i32,
}

/// `SMSG_AUCTION_OWNER_NOTIFICATION`, a sale or a new bid, to the seller
/// (`Server/Packets/AuctionHouse.cpp:89-97`): no house id, and the guid fourth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuctionOwnerNotification {
    pub auction_id: u32,
    pub bid: u32,
    /// The step the next bid must clear (`GetAuctionOutBid()`).
    pub out_bid: u32,
    /// Who bid; zero on a sale, since vmangos fills it only `if (!sold)`.
    pub bidder_guid: u64,
    pub item_entry: u32,
    /// Signed, as on [`AuctionListEntry::random_property_id`]; 0 when the item is gone.
    pub random_property_id: i32,
}

/// `MSG_AUCTION_HELLO` (`Server/Packets/AuctionHouse.cpp:3-6`): the auctioneer guid. The same
/// opcode comes back with the house id ([`read_auction_hello`]), and that reply opens the window.
pub fn auction_hello(auctioneer: u64) -> Vec<u8> {
    auctioneer.to_le_bytes().to_vec()
}

/// `CMSG_AUCTION_SELL_ITEM`: an `etime_minutes` outside [`auction_duration`] gets `DATABASE`; a
/// zero `bid` or `etime`, or an unaffordable deposit, gets no answer (`HandleAuctionSellItem`).
pub fn auction_sell_item(
    auctioneer: u64,
    item_guid: u64,
    bid: u32,
    buyout: u32,
    etime_minutes: u32,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 8 + 4 + 4 + 4);
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&item_guid.to_le_bytes());
    body.extend_from_slice(&bid.to_le_bytes());
    body.extend_from_slice(&buyout.to_le_bytes());
    body.extend_from_slice(&etime_minutes.to_le_bytes());
    body
}

/// `CMSG_AUCTION_REMOVE_ITEM` (`Server/Packets/AuctionHouse.cpp:36-40`), the seller's cancel. A
/// seller who cannot pay the 5% cut on an auction with a bid gets no answer at all.
pub fn auction_remove_item(auctioneer: u64, auction_id: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4);
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&auction_id.to_le_bytes());
    body
}

/// `CMSG_AUCTION_LIST_ITEMS` (`Server/Packets/AuctionHouse.cpp:51-63`): ten fields, no sort
/// bytes (5875 sorts on the client). `quality` is a minimum; the name is a case-insensitive
/// substring match on the localized name plus its random-property suffix.
pub fn auction_list_items(
    auctioneer: u64,
    list_from: u32,
    searched_name: &str,
    level_min: u8,
    level_max: u8,
    slot_id: u32,
    main_category: u32,
    sub_category: u32,
    quality: u32,
    usable: u8,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4 + searched_name.len() + 1 + 1 + 1 + 4 + 4 + 4 + 4 + 1);
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&list_from.to_le_bytes());
    body.extend_from_slice(searched_name.as_bytes());
    body.push(0);
    body.push(level_min);
    body.push(level_max);
    body.extend_from_slice(&slot_id.to_le_bytes());
    body.extend_from_slice(&main_category.to_le_bytes());
    body.extend_from_slice(&sub_category.to_le_bytes());
    body.extend_from_slice(&quality.to_le_bytes());
    body.push(usable);
    body
}

/// `CMSG_AUCTION_LIST_OWNER_ITEMS`, the Auctions tab (`Server/Packets/AuctionHouse.cpp:23-27`);
/// the server matches the account, then filters to this character.
pub fn auction_list_owner_items(auctioneer: u64, list_from: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4);
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&list_from.to_le_bytes());
    body
}

/// `CMSG_AUCTION_PLACE_BID` (`Server/Packets/AuctionHouse.cpp:29-34`), also the buyout: a `price`
/// equal to the buyout is one. An unaffordable bid gets no answer at all.
pub fn auction_place_bid(auctioneer: u64, auction_id: u32, price: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4 + 4);
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&auction_id.to_le_bytes());
    body.extend_from_slice(&price.to_le_bytes());
    body
}

/// `CMSG_AUCTION_LIST_BIDDER_ITEMS`, the Bid tab (`Server/Packets/AuctionHouse.cpp:8-21`). The
/// ids are a refresh set, not a filter: the server lists them first, then every auction we lead,
/// in one page, so an id in both appears twice. The reference sends those it was outbid on.
pub fn auction_list_bidder_items(auctioneer: u64, list_from: u32, auction_ids: &[u32]) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4 + 4 + 4 * auction_ids.len());
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&list_from.to_le_bytes());
    body.extend_from_slice(&(auction_ids.len() as u32).to_le_bytes());
    for id in auction_ids {
        body.extend_from_slice(&id.to_le_bytes());
    }
    body
}

/// The `MSG_AUCTION_HELLO` reply (`Server/Packets/AuctionHouse.cpp:72-76`): the guid and the
/// `AuctionHouse.dbc` house id (1..7), whose row sets the deposit and cut rates.
pub(super) fn read_auction_hello(r: &mut &[u8]) -> io::Result<(u64, u32)> {
    Ok((read_u64_le(r)?, read_u32_le(r)?))
}

fn read_auction_list_entry(r: &mut &[u8]) -> io::Result<AuctionListEntry> {
    Ok(AuctionListEntry {
        auction_id: read_u32_le(r)?,
        item_entry: read_u32_le(r)?,
        perm_enchant: read_u32_le(r)?,
        random_property_id: read_i32_le(r)?,
        suffix_factor: read_u32_le(r)?,
        count: read_u32_le(r)?,
        spell_charges: read_i32_le(r)?,
        owner_guid: read_u64_le(r)?,
        start_bid: read_u32_le(r)?,
        min_increment: read_u32_le(r)?,
        buyout: read_u32_le(r)?,
        time_left_ms: read_u32_le(r)?,
        bidder_guid: read_u64_le(r)?,
        current_bid: read_u32_le(r)?,
    })
}

/// The body the three list results share (`AuctionHouseHandler.cpp:651-701`): `u32 count`, the
/// records, then `u32 totalCount`, the match count before the page cap, at the very end.
pub(super) fn read_auction_list_result(r: &mut &[u8]) -> io::Result<(Vec<AuctionListEntry>, u32)> {
    let count = read_u32_le(r)?;
    let mut auctions = Vec::with_capacity(capacity_hint(count, r.len() / AUCTION_RECORD_BYTES));
    for _ in 0..count {
        // `count` is an upper bound: vmangos's no-filter path (`AuctionHouseMgr.cpp:716-735`)
        // counts an auction whose item is gone but writes no bytes for it.
        if r.len() < AUCTION_RECORD_BYTES {
            break;
        }
        auctions.push(read_auction_list_entry(r)?);
    }
    let total_count = if r.len() >= 4 {
        read_u32_le(r)?
    } else {
        auctions.len() as u32
    };
    Ok((auctions, total_count))
}

/// `SMSG_AUCTION_COMMAND_RESULT` (`AuctionHouseHandler.cpp:70-96`): `u32` auction id, action,
/// error, then the [`AuctionCommandTail`]; the auction id is 0 on most failures.
pub(super) fn read_auction_command_result(
    r: &mut &[u8],
) -> io::Result<(u32, u32, u32, AuctionCommandTail)> {
    let auction_id = read_u32_le(r)?;
    let action = read_u32_le(r)?;
    let error = read_u32_le(r)?;
    // The tail reads short: `0x418e30` and its kin write nothing past the end and the handler
    // goes on, so a tail shorter than its fields is no tail and the packet stands. A higher bid
    // missing a field would patch from unwritten stack slots, which nothing here can know.
    let tail = match error {
        auction_error::OK if action == auction_action::BID_PLACED && r.len() >= 4 => {
            AuctionCommandTail::BidPlaced {
                new_min_outbid: read_u32_le(r)?,
            }
        }
        auction_error::INVENTORY if r.len() >= 4 => AuctionCommandTail::Inventory {
            result: read_u32_le(r)?,
        },
        auction_error::HIGHER_BID if r.len() >= 16 => AuctionCommandTail::HigherBid {
            new_bidder_guid: read_u64_le(r)?,
            new_bid: read_u32_le(r)?,
            new_min_outbid: read_u32_le(r)?,
        },
        _ => AuctionCommandTail::Empty,
    };
    Ok((auction_id, action, error, tail))
}

pub(super) fn read_auction_bidder_notification(
    r: &mut &[u8],
) -> io::Result<AuctionBidderNotification> {
    Ok(AuctionBidderNotification {
        house_id: read_u32_le(r)?,
        auction_id: read_u32_le(r)?,
        bidder_guid: read_u64_le(r)?,
        bid_or_zero: read_u32_le(r)?,
        out_bid: read_u32_le(r)?,
        item_entry: read_u32_le(r)?,
        random_property_id: read_i32_le(r)?,
    })
}

pub(super) fn read_auction_owner_notification(
    r: &mut &[u8],
) -> io::Result<AuctionOwnerNotification> {
    Ok(AuctionOwnerNotification {
        auction_id: read_u32_le(r)?,
        bid: read_u32_le(r)?,
        out_bid: read_u32_le(r)?,
        bidder_guid: read_u64_le(r)?,
        item_entry: read_u32_le(r)?,
        random_property_id: read_i32_le(r)?,
    })
}

/// `SMSG_AUCTION_REMOVED_NOTIFICATION` (`Server/Packets/AuctionHouse.cpp:65-70`), to a bidder
/// whose auction was cancelled: `(auction_id, item_entry, random_property_id)`.
pub(super) fn read_auction_removed_notification(r: &mut &[u8]) -> io::Result<(u32, u32, i32)> {
    Ok((read_u32_le(r)?, read_u32_le(r)?, read_i32_le(r)?))
}
