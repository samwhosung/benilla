//! The merchant getters' reads at the call, the call host's half of
//! [`benilla_ui::script::MerchantHeadQuery`] and its kin: the window's cell and rows
//! (`0xbddfa0`, `0xbdd118`), the player's buy-back fields and the open vendor's unit as the
//! handlers left them, each row resolved off the item cache as the reference's getter reads it at
//! the call (`0x4fb150`, `0x4fb310`, `0x4fadb0`, `0x4fbd60`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    BuybackRowQuery, MerchantHead, MerchantHeadQuery, MerchantItem, MerchantRowQuery,
    RepairAllCostQuery,
};

use super::{
    buyback_order, open_vendor_discount, repair_all, resolve_buyback, resolve_item, vendor_repairs,
    vendor_unit, MerchantOpen, RepairTables,
};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::Objects;
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
    relations: crate::target::RelationWorld<'w, 's>,
}

impl MerchantLive<'_, '_> {
    /// The cell, the row count the close zeroes (`0x4fae22`), the buy-back count with a vendor
    /// (`0x4fc166`) and the repair flag (`0x4fadb0`).
    fn head(&self) -> Option<MerchantHead> {
        let open = self.open.as_deref()?;
        if open.vendor.is_none() {
            return Some(MerchantHead::default());
        }
        let buyback = self
            .objects
            .player_fields()
            .map_or(0, |p| buyback_order(p).len());
        Some(MerchantHead {
            open: true,
            items: open.items.len(),
            buyback,
            can_repair: vendor_repairs(vendor_unit(open, &self.objects)),
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
        let Some(player) = self.objects.player_fields() else {
            return Some(None);
        };
        Some(buyback_order(player).get(index).map(|&slot| {
            resolve_buyback(
                slot,
                player,
                &self.objects,
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
        let vendor = vendor_unit(open, &self.objects);
        let player = self.objects.player().and_then(|e| self.objects.store_of(e));
        let (true, Some(player)) = (vendor_repairs(vendor), player) else {
            return Some(None);
        };
        let discount = open_vendor_discount(vendor, &self.relations, player);
        Some(Some(repair_all(
            &player.0,
            &self.objects,
            items,
            self.tables.as_deref(),
            discount,
        )))
    }
}

impl crate::script_calls::live::LiveFamily for MerchantLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &MerchantHeadQuery| self.head())
            || ask.answer(|q: &MerchantRowQuery| self.row(q.0))
            || ask.answer(|q: &BuybackRowQuery| self.buyback(q.0))
            || ask.answer(|_: &RepairAllCostQuery| self.repair_all())
    }
}
