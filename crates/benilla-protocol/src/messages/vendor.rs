//! Vendor messages: the inventory list, buy and sell (opcodes 414-421), buyback and repair.

use std::io;

use crate::wire::{capacity_hint, read_u32_le, read_u64_le, read_u8};

/// One vendor row (`ItemHandler.cpp:741-810`): `slot` is the 1-based list position, not what a buy
/// sends; `current_count` `0xFFFF_FFFF` is unlimited; `price` is already reputation-discounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorItem {
    pub slot: u32,
    pub entry: u32,
    /// `item_template.display_id` → icon via `ItemDisplayInfo.dbc`.
    pub display_id: u32,
    pub current_count: u32,
    /// Buy price in copper.
    pub price: u32,
    pub max_durability: u32,
    /// Stack size delivered per purchase (`item_template.buy_count`).
    pub buy_count: u32,
}

/// `BuyResult` (vmangos `ItemDefines.h:120-141`): the `u8` reason on `SMSG_BUY_FAILED`.
pub mod buy_result {
    pub const CANT_FIND_ITEM: u8 = 0;
    pub const ITEM_ALREADY_SOLD: u8 = 1;
    pub const NOT_ENOUGH_MONEY: u8 = 2;
    pub const SELLER_DONT_LIKE_YOU: u8 = 4;
    pub const DISTANCE_TOO_FAR: u8 = 5;
    pub const ITEM_SOLD_OUT: u8 = 7;
    pub const CANT_CARRY_MORE: u8 = 8;
    pub const RANK_REQUIRE: u8 = 11;
    pub const REPUTATION_REQUIRE: u8 = 12;
}

/// `SellResult` (vmangos `ItemDefines.h:120-141`): the `u8` reason on `SMSG_SELL_ITEM`.
pub mod sell_result {
    pub const CANT_FIND_ITEM: u8 = 1;
    pub const CANT_SELL_ITEM: u8 = 2;
    pub const CANT_FIND_VENDOR: u8 = 3;
    pub const YOU_DONT_OWN_THAT_ITEM: u8 = 4;
    pub const UNK: u8 = 5;
    pub const ONLY_EMPTY_BAG: u8 = 6;
}

/// Body of `CMSG_LIST_INVENTORY`: the vendor guid; the dead get no list (`ItemHandler.cpp:693`).
pub fn list_inventory(vendor_guid: u64) -> Vec<u8> {
    vendor_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_BUY_ITEM` (vmangos `Item.cpp:104-110`): `u64 vendorGuid`, `u32` template entry
/// (not the row's `muid`), `u8 count` (stacks), `u8 unk1` (0); it fills the first free slot.
pub fn buy_item(vendor_guid: u64, entry: u32, count: u8) -> Vec<u8> {
    let mut body = Vec::with_capacity(14);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.extend_from_slice(&entry.to_le_bytes());
    body.push(count);
    body.push(0); // unk1
    body
}

/// Body of `CMSG_BUY_ITEM_IN_SLOT` (vmangos `Item.cpp:113-120`): `u64 vendorGuid`, `u32` template
/// entry, `u64 bagGuid` (ours for the backpack), `u8 bagSlot`, `u8 count`; the reference sends it
/// with `count = 1` for a vendor row dropped into a slot (`0x5e1f30`).
pub fn buy_item_in_slot(
    vendor_guid: u64,
    entry: u32,
    bag_guid: u64,
    bag_slot: u8,
    count: u8,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(22);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.extend_from_slice(&entry.to_le_bytes());
    body.extend_from_slice(&bag_guid.to_le_bytes());
    body.push(bag_slot);
    body.push(count);
    body
}

/// Body of `CMSG_SELL_ITEM` (vmangos `Item.cpp:87-92`): `u64 vendorGuid, u64 itemGuid, u8 count`,
/// where 0 sells the whole stack.
pub fn sell_item(vendor_guid: u64, item_guid: u64, count: u8) -> Vec<u8> {
    let mut body = Vec::with_capacity(17);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.extend_from_slice(&item_guid.to_le_bytes());
    body.push(count);
    body
}

/// Body of `CMSG_BUYBACK_ITEM` (`0x4fb950`): `u64 vendorGuid, u32 slot`, the absolute buyback
/// slot 69-80 (`BUYBACK_SLOT_START` + index), not re-based.
pub fn buyback_item(vendor_guid: u64, slot: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.extend_from_slice(&slot.to_le_bytes());
    body
}

/// Body of `CMSG_REPAIR_ITEM`: `u64 vendorGuid, u64 itemGuid`; item guid 0 repairs everything.
pub fn repair_item(vendor_guid: u64, item_guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.extend_from_slice(&item_guid.to_le_bytes());
    body
}

/// Read `SMSG_LIST_INVENTORY` (vmangos `ItemHandler.cpp:741-810`): `u64 vendorGuid, u8 count`
/// (at most 128), then `count` rows. An empty list is followed by one `u8` 0
/// (`ItemHandler.cpp:728-733`, `:806-809`), read here so no tail is left over.
pub(super) fn read_list_inventory(r: &mut &[u8]) -> io::Result<(u64, Vec<VendorItem>)> {
    let vendor_guid = read_u64_le(r)?;
    let count = read_u8(r)?;
    if count == 0 {
        let _no_inventory = read_u8(r)?;
        return Ok((vendor_guid, Vec::new()));
    }
    // vmangos `MAX_VENDOR_ITEMS` 128 (`Objects/CreatureDefines.h:617`).
    let mut items = Vec::with_capacity(capacity_hint(count, 128));
    for _ in 0..count {
        items.push(VendorItem {
            slot: read_u32_le(r)?,
            entry: read_u32_le(r)?,
            display_id: read_u32_le(r)?,
            current_count: read_u32_le(r)?,
            price: read_u32_le(r)?,
            max_durability: read_u32_le(r)?,
            buy_count: read_u32_le(r)?,
        });
    }
    Ok((vendor_guid, items))
}

/// Read `SMSG_BUY_ITEM` (vmangos `Item.cpp:190-196`): `u64 vendorGuid, u32 vendorSlot` (1-based),
/// `u32 newCount` (`0xFFFF_FFFF` unlimited), `u32 purchaseCount`; the item arrives separately.
pub(super) fn read_buy_item(r: &mut &[u8]) -> io::Result<(u64, u32, u32, u32)> {
    Ok((
        read_u64_le(r)?,
        read_u32_le(r)?,
        read_u32_le(r)?,
        read_u32_le(r)?,
    ))
}

/// Read `SMSG_SELL_ITEM` (vmangos `Item.cpp:183-188`): `u64 vendorGuid, u64 itemGuid, u8 reason`
/// (a [`sell_result`] code). Only a failed sell sends it; a sale shows only in `UPDATE_OBJECT`.
pub(super) fn read_sell_item(r: &mut &[u8]) -> io::Result<(u64, u64, u8)> {
    Ok((read_u64_le(r)?, read_u64_le(r)?, read_u8(r)?))
}

/// Read `SMSG_BUY_FAILED` (vmangos `Item.h:277`): `u64 vendorGuid, u32 itemEntry, u8 reason`
/// (a [`buy_result`] code).
pub(super) fn read_buy_failed(r: &mut &[u8]) -> io::Result<(u64, u32, u8)> {
    Ok((read_u64_le(r)?, read_u32_le(r)?, read_u8(r)?))
}
