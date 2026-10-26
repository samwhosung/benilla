//! The merchant getters' reads at the call, the call host's half of
//! [`benilla_ui::script::MerchantHeadQuery`] and its kin: the window's cell and rows
//! (`0xbddfa0`, `0xbdd118`), the player's buy-back fields, items and the open vendor's unit as of
//! the drain's place, each row resolved off the item cache as the reference's getter reads it at
//! the call (`0x4fb150`, `0x4fb310`, `0x4fadb0`, `0x4fbd60`), and each item's repair cost the
//! tooltips read at theirs (`0x4faf30`, `0x5da330`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    BuybackRowQuery, ItemRepairCostQuery, MerchantHead, MerchantHeadQuery, MerchantItem,
    MerchantRowQuery, RepairAllCostQuery, RepairCostQuery,
};

use super::{
    buyback_order, item_repair_cost, open_vendor_discount, repair_all, resolve_buyback,
    resolve_item, vendor_repairs, MerchantOpen, RepairTables,
};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ObjectStore, Objects};
use crate::query_cache::Ask;

/// What the merchant getters read as they run; the window, the item cache and the tables each
/// absent in a harness without them, where the getters read the VM's copy or nothing.
#[derive(SystemParam)]
pub(crate) struct MerchantLive<'w, 's> {
    open: Option<Res<'w, MerchantOpen>>,
    items: Option<Res<'w, Items>>,
    icons: Option<Res<'w, ItemDisplays>>,
    tables: Option<Res<'w, RepairTables>>,
    rolls: crate::items::Rolls<'w>,
    objects: Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    relations: crate::target::RelationWorld<'w, 's>,
}

impl MerchantLive<'_, '_> {
    /// The active player's store as of the drain's place.
    fn player(&self) -> Option<&ObjectStore> {
        self.objects.player().and_then(|e| self.stores.get(e).ok())
    }

    /// The open vendor's unit as of the drain's place.
    fn vendor(&self, open: &MerchantOpen) -> Option<(u64, &ObjectStore)> {
        let guid = open.vendor?;
        self.stores
            .by_guid(guid, self.objects.store(guid))
            .map(|store| (guid, store))
    }

    /// The cell, the row count the close zeroes (`0x4fae22`), the buy-back count with a vendor
    /// (`0x4fc166`) and the repair flag (`0x4fadb0`).
    fn head(&self) -> Option<MerchantHead> {
        let open = self.open.as_deref()?;
        if open.vendor.is_none() {
            return Some(MerchantHead::default());
        }
        let buyback = self.player().map_or(0, |p| buyback_order(&p.0).len());
        Some(MerchantHead {
            open: true,
            items: open.items.len(),
            buyback,
            can_repair: vendor_repairs(self.vendor(open)),
        })
    }

    /// The open window's row `index`, its record only read: the getter's own lookup is its call's
    /// ask.
    fn row(&self, index: usize) -> Option<Option<MerchantItem>> {
        let (open, items) = (self.open.as_deref()?, self.items.as_deref()?);
        Some(
            open.vendor
                .and(open.items.get(index))
                .map(|it| resolve_item(it, items, self.icons.as_deref(), Ask::Read)),
        )
    }

    /// Buy-back row `index` of the player's fields, sorted as `0x4fafd0` keeps them; no vendor is
    /// read (`0x4fb310`-`0x4fb3d7`).
    fn buyback(&self, index: usize) -> Option<Option<MerchantItem>> {
        let items = self.items.as_deref()?;
        let Some(player) = self.player() else {
            return Some(None);
        };
        let objects = self.stores.item_objects(&self.objects);
        Some(buyback_order(&player.0).get(index).map(|&slot| {
            resolve_buyback(
                slot,
                &player.0,
                &objects,
                items,
                (self.icons.as_deref(), self.rolls.get()),
                Ask::Read,
            )
        }))
    }

    /// `GetRepairAllCost`'s pair at a repairer with a player (`0x4fbd6d`), the costs after the
    /// open vendor's discount (`0x4faf8a`).
    pub(super) fn repair_all(&self) -> Option<Option<(u32, bool)>> {
        let (open, items) = (self.open.as_deref()?, self.items.as_deref()?);
        let vendor = self.vendor(open);
        let player = self.player();
        let (true, Some(player)) = (vendor_repairs(vendor), player) else {
            return Some(None);
        };
        let discount = open_vendor_discount(vendor, &self.relations, player);
        Some(Some(repair_all(
            &player.0,
            &self.stores.item_objects(&self.objects),
            items,
            self.tables.as_deref(),
            discount,
        )))
    }

    /// The item at the player's `bag` slot `slot` (1-based; the doll's live id with
    /// [`benilla_ui::script::EQUIPMENT_BAG`]) priced as `0x4faf30` prices it at the call: the
    /// object's own cost (`0x5da330`) after the open vendor's discount, the full price with none
    /// open (`0x4faf8a`); 0 for no object, no player or no tables.
    pub(super) fn repair_cost(&self, bag: i64, slot: u32) -> Option<u32> {
        let items = self.items.as_deref()?;
        let Some(player) = self.player() else {
            return Some(0);
        };
        let (Some(slot0), Some(tables)) = (
            slot.checked_sub(1).and_then(|s| u8::try_from(s).ok()),
            self.tables.as_deref(),
        ) else {
            return Some(0);
        };
        let objects = self.stores.item_objects(&self.objects);
        let Some(guid) = crate::ui_items::slot_guid(&player.0, bag, slot0, &objects) else {
            return Some(0);
        };
        let discount = self.open.as_deref().map_or(0.0, |open| {
            open_vendor_discount(self.vendor(open), &self.relations, player)
        });
        Some(item_repair_cost(guid, &objects, items, tables, discount))
    }

    /// The item object `guid`'s own cost before any discount (`0x5da330`), as of the drain's
    /// place.
    pub(super) fn own_repair_cost(&self, guid: u64) -> Option<u32> {
        let items = self.items.as_deref()?;
        let objects = self.stores.item_objects(&self.objects);
        Some(self.tables.as_deref().map_or(0, |tables| {
            item_repair_cost(guid, &objects, items, tables, 0.0)
        }))
    }
}

impl crate::script_calls::live::LiveFamily for MerchantLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &MerchantHeadQuery| self.head())
            || ask.answer(|q: &MerchantRowQuery| self.row(q.0))
            || ask.answer(|q: &BuybackRowQuery| self.buyback(q.0))
            || ask.answer(|_: &RepairAllCostQuery| self.repair_all())
            || ask.answer(|q: &RepairCostQuery| self.repair_cost(q.bag, q.slot))
            || ask.answer(|q: &ItemRepairCostQuery| self.own_repair_cost(q.0))
    }
}
