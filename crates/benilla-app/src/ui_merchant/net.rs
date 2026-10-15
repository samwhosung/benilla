//! The vendor window's packet handlers: they fill [`MerchantOpen`] and [`MerchantErrors`].

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{MerchantErrors, MerchantOpen, MerchantRefusal};
use crate::net::NetHandlerApp;

/// Register the vendor handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::VendorInventory, on_inventory)
        .net_handler(K::VendorBuyResult, on_buy_result)
        .net_handler(K::VendorBuyFailed, on_buy_failed)
        .net_handler(K::VendorSellFailed, on_sell_failed)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_inventory(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::VendorInventory { vendor, items } = ev {
        commands.queue(move |world: &mut World| super::open_merchant(world, vendor, items));
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

fn on_sell_failed(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<MerchantErrors>,
    mut releases: ResMut<crate::ui_items::locks::ItemReleases>,
) {
    if let SessionEvent::VendorSellFailed {
        item_guid, reason, ..
    } = ev
    {
        vendor_sell_failed(reason, item_guid, &mut errors, &mut releases);
    }
}

/// The vendor window closes with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut merchant: ResMut<MerchantOpen>) {
    merchant.clear_session();
}

/// `SMSG_BUY_ITEM`'s new stock count, applied only to the open vendor.
fn vendor_buy_result(vendor: u64, slot: u32, new_count: u32, merchant: &mut MerchantOpen) {
    if merchant.vendor == Some(vendor) {
        merchant.update_stock(slot, new_count);
    }
}

/// `SMSG_BUY_FAILED`: queue the message line, and for `ITEM_ALREADY_SOLD` zero the refused row of
/// the open vendor, as the reference does (`0x5dcda7`..`0x5dcdd6`). vmangos sends that code only
/// for a limited row short of stock (`Player.cpp:18541`).
///
/// Deviation: the row is matched by item entry, where the reference matches its vendor slot word
/// (`+0x00`), because vmangos writes the entry in that field (`Player.cpp:11718`).
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

/// `SMSG_SELL_ITEM` (`0x5dd1f0`): a zero result is a success and returns at once (`0x5dd221`);
/// any other queues the message line and unlocks the item the sale locked (`0x5dd26b`), a zero
/// guid excepted (`0x5dd267`). A success's item leaves its slot, which unlocks it.
fn vendor_sell_failed(
    reason: u8,
    item_guid: u64,
    errors: &mut MerchantErrors,
    releases: &mut crate::ui_items::locks::ItemReleases,
) {
    if reason == 0 {
        return;
    }
    debug!("net: sell failed (reason {reason}, item {item_guid:#x})");
    errors.0.push(MerchantRefusal::Sell(reason));
    releases.unlock(item_guid);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_items::locks::{ItemReleases, Release};

    /// A refused sale unlocks the item it names (`0x5dd26b`); a zero result is a success, which
    /// unlocks nothing and shows nothing (`0x5dd221`).
    #[test]
    fn a_refused_sale_unlocks_its_item_and_a_success_does_nothing() {
        const ITEM: u64 = 0x4000_0000_0000_0042;
        let mut errors = MerchantErrors::default();
        let mut releases = ItemReleases::default();
        vendor_sell_failed(0, ITEM, &mut errors, &mut releases);
        assert!(errors.0.is_empty() && releases.0.is_empty());
        vendor_sell_failed(1, ITEM, &mut errors, &mut releases);
        assert_eq!(errors.0.len(), 1);
        assert_eq!(releases.0, [Release::Item(ITEM)]);
    }
}
