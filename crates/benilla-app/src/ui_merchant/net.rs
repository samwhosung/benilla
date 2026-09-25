//! The vendor window's packet handlers (decision 0081 phase 4; in the net handler table since
//! 2318, moved out of the drain's npc arm file) — each fills the [`MerchantOpen`] session or the
//! [`MerchantErrors`] line queue the merchant feed ([`super`]) reads.

use benilla_protocol::messages::VendorItem;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{MerchantErrors, MerchantOpen, MerchantRefusal};
use crate::net::NetHandlerApp;

/// Register the vendor handlers — called from [`super::UiMerchantPlugin`]. One per kind, plus the
/// session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::VendorInventory, on_inventory)
        .net_handler(K::VendorBuyResult, on_buy_result)
        .net_handler(K::VendorBuyFailed, on_buy_failed)
        .net_handler(K::VendorSellFailed, on_sell_failed)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_inventory(In(ev): In<SessionEvent>, mut merchant: ResMut<MerchantOpen>) {
    if let SessionEvent::VendorInventory { vendor, items } = ev {
        vendor_inventory(vendor, items, &mut merchant);
    }
}

fn on_buy_result(In(ev): In<SessionEvent>, mut merchant: ResMut<MerchantOpen>) {
    if let SessionEvent::VendorBuyResult {
        vendor,
        slot,
        new_count,
        ..
    } = ev
    {
        vendor_buy_result(vendor, slot, new_count, &mut merchant);
    }
}

fn on_buy_failed(
    In(ev): In<SessionEvent>,
    mut merchant: ResMut<MerchantOpen>,
    mut errors: ResMut<MerchantErrors>,
) {
    if let SessionEvent::VendorBuyFailed {
        vendor,
        item_entry,
        reason,
    } = ev
    {
        vendor_buy_failed(vendor, item_entry, reason, &mut merchant, &mut errors);
    }
}

fn on_sell_failed(In(ev): In<SessionEvent>, mut errors: ResMut<MerchantErrors>) {
    if let SessionEvent::VendorSellFailed { reason, .. } = ev {
        vendor_sell_failed(reason, &mut errors);
    }
}

/// An open vendor window dies with the socket. A listener on the session end
/// (a second handler on the kind, after the bridge's own teardown).
fn on_session_end(In(_): In<SessionEvent>, mut merchant: ResMut<MerchantOpen>) {
    merchant.clear_session();
}

/// A vendor's stock (`SMSG_LIST_INVENTORY`): fill the [`MerchantOpen`] the merchant feed
/// ([`super`]) reads. A successful buy updates the stock display via
/// [`vendor_buy_result`] (the item itself lands via item-create); a successful sell is silent
/// (never a packet here — only the error path is).
fn vendor_inventory(vendor: u64, items: Vec<VendorItem>, merchant: &mut MerchantOpen) {
    debug!("net: vendor {vendor:#x} listed {} items", items.len());
    merchant.open(vendor, items);
}

/// A purchase updated the vendor's stock (`SMSG_BUY_ITEM`). Only touch stock for the open
/// vendor (a late answer for a closed window is stale).
fn vendor_buy_result(vendor: u64, slot: u32, new_count: u32, merchant: &mut MerchantOpen) {
    if merchant.vendor == Some(vendor) {
        merchant.update_stock(slot, new_count);
    }
}

/// A purchase was refused (`SMSG_BUY_FAILED`) — the merchant window's error line, and for the
/// out-of-stock code the refusing row's own count.
///
/// **`ITEM_ALREADY_SOLD` zeroes the row** (`0x5dcda7`..`0x5dcdd6`): the reference
/// walks its 128-row vendor cache, writes 0 into the matching row's count word and repaints —
/// gated, as every other stale-answer path here is, on the packet naming the vendor still open.
///
/// **NAMED DIVERGENCE in the key.** The reference matches the row's `+0x00`, the vendor *slot* —
/// the same word `SMSG_BUY_ITEM` keys its stock update by. vmangos puts the item *entry* in that
/// field (`Player::SendBuyError`, `Player.cpp:11637`: `packet->itemEntry = item`), so matching by
/// slot would find nothing on the server we actually talk to; matching by entry is the same row.
/// vmangos raises this code from exactly the condition the zeroing models —
/// `GetVendorItemCurrentCount(crItem) < totalCount` under `crItem->maxcount != 0`
/// (`Player.cpp:18508`).
fn vendor_buy_failed(
    vendor: u64,
    item_entry: u32,
    reason: u8,
    merchant: &mut MerchantOpen,
    errors: &mut MerchantErrors,
) {
    debug!("net: buy failed (entry {item_entry}, reason {reason})");
    if reason == benilla_protocol::messages::buy_result::ITEM_ALREADY_SOLD
        && merchant.vendor == Some(vendor)
    {
        merchant.sold_out(item_entry);
    }
    errors.0.push(MerchantRefusal::Buy(reason));
}

/// A sell was refused (`SMSG_SELL_ITEM`'s error path) — the merchant window's error line.
fn vendor_sell_failed(reason: u8, errors: &mut MerchantErrors) {
    debug!("net: sell failed (reason {reason})");
    errors.0.push(MerchantRefusal::Sell(reason));
}
