//! The merchant window's app side: [`MerchantOpen`] holds the `SMSG_LIST_INVENTORY` rows, which
//! the getters read with the buy-back rows and the repair total through the host as they run
//! ([`live`]), the items' repair costs among them; the handlers signal on the merchant lane
//! ([`lane`]), and [`MerchantSteps`] applies the verbs, the close among them, inside
//! their calls. A bag click sells through [`crate::ui_items`].

use benilla_protocol::messages::{buy_result, sell_result, VendorItem};
use bevy::prelude::*;

use benilla_ui::script::{ItemStatsHead, MerchantItem, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ObjectStore, Objects};
use crate::query_cache::Ask;
use crate::ui_items::{item_link, wire_pos};
use crate::ui_script::{SweepApp, SweepLeg};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

/// A vendor row's unlimited `current_count` (vmangos `ItemHandler.cpp:795`), `-1` to Lua.
const STOCK_UNLIMITED: u32 = 0xFFFF_FFFF;

/// `UNIT_NPC_FLAG_REPAIR`, the bit `CanMerchantRepair` tests (`0x4fadb0`).
const NPC_FLAG_REPAIR: u32 = 0x4000;

/// The first absolute buyback slot (`BUYBACK_SLOT_START`, slots 69-80).
const BUYBACK_SLOT_FIRST: u32 = 69;

/// `DurabilityCosts.dbc` and `DurabilityQuality.dbc`, the client's repair prices; absent, every
/// cost shows 0.
#[derive(Resource)]
pub(crate) struct RepairTables(pub(crate) benilla_formats::DurabilityTables);

/// The open vendor, the cell `[0xbddfa0]` a close clears, and the list cache `0xc4c4a0` of the
/// last `SMSG_LIST_INVENTORY` with its guid `[0xc4d498]`, which every list writes (`0x5db9c7`) and
/// only the world reset zeroes (`0x5e2515`): a close keeps them.
#[derive(Resource, Default)]
pub(crate) struct MerchantOpen {
    pub(crate) vendor: Option<u64>,
    /// The list cache's guid, which `SMSG_BUY_ITEM` matches (`0x5dcf20`).
    pub(crate) listed: Option<u64>,
    /// In wire order, which is the 1-based display order.
    pub(crate) items: Vec<VendorItem>,
    /// The row records `GetMerchantItemInfo` asked for and is waiting on (`0xbddfb0`): each miss
    /// counts one (`0x4faf23`), each answer one down (`0x4fb0f9`), and a list starts it at 0
    /// (`0x4fad88`).
    pub(crate) pending: u32,
    /// The cascade's reset, owed from its item's push ([`MerchantOpen::settle_world_reset`]).
    pub(crate) world_reset: crate::ui_world_enter::OwedReset,
}

impl MerchantOpen {
    pub(crate) fn open(&mut self, vendor: u64, items: Vec<VendorItem>) {
        self.settle_world_reset();
        self.vendor = Some(vendor);
        self.listed = Some(vendor);
        self.items = items;
        self.pending = 0;
    }

    #[cfg(test)]
    pub(crate) fn is_open(&self) -> bool {
        self.vendor.is_some()
    }

    /// The row at a 1-based index: `CMSG_BUY_ITEM` buys by its entry, not by its vendor slot.
    pub(crate) fn row_at(&self, index_1based: u32) -> Option<&VendorItem> {
        index_1based
            .checked_sub(1)
            .and_then(|i| self.items.get(i as usize))
    }

    /// `SMSG_BUY_ITEM`'s new count for the row at 1-based vendor `slot`, answering its item, or
    /// `None` with no such row; the bought item itself arrives as an object create.
    pub(crate) fn update_stock(&mut self, slot: u32, new_count: u32) -> Option<u32> {
        let it = self.items.iter_mut().find(|it| it.slot == slot)?;
        it.current_count = new_count;
        Some(it.entry)
    }

    /// Zero every row of `entry`, as the reference's `ITEM_ALREADY_SOLD` arm writes 0 (`0x5dcdbf`)
    /// into each matching row of its 128-row cache (`0x5dcdcd`); how many it zeroed.
    pub(crate) fn sold_out(&mut self, entry: u32) -> usize {
        let mut rows = 0;
        for it in self.items.iter_mut().filter(|it| it.entry == entry) {
            it.current_count = 0;
            rows += 1;
        }
        rows
    }

    /// A client-side close (`0x4fadf0`): the cell alone; the list cache stays.
    pub(crate) fn clear(&mut self) {
        self.vendor = None;
    }

    /// The disconnect clear, the world reset's zeroed cache guid (`0x5e2515`) with it.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
        self.listed = None;
        self.items.clear();
        self.world_reset = Default::default();
    }
}

mod calls;
pub(crate) mod lane;
pub(crate) mod live;
mod net;
pub(crate) use calls::MerchantSteps;
pub(crate) use lane::{MerchantEvent, MerchantLane, MerchantRecordAnswers};
pub(crate) use live::MerchantLive;

pub(crate) struct UiMerchantPlugin;

impl Plugin for UiMerchantPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        lane::add_merchant_lane(app);
        app.init_resource::<MerchantOpen>()
            .add_latched_window::<MerchantOpen>()
            // `0x490ada` → `0x4facc0` → `0x4fadf0`, the core itself.
            .add_sweep_leg(SweepLeg::Merchant, close_core);
    }
}

/// The one `BuyResult` code the reference shows nothing for: `0x5dcde7`'s jump table sends 6 to
/// the handler's `ret`. vmangos never sends it.
const BUY_SILENT: u8 = 6;

/// The message a `SMSG_BUY_FAILED` code shows, from the reference's switch at `0x5dcdd8`; its
/// default arm (`0x5dce81`) shows `ERR_ITEM_NOT_FOUND` for 0, 3, 9, 10 and 13 up.
fn buy_error_key(reason: u8) -> Option<&'static str> {
    Some(match reason {
        // One arm (`0x5dcdee`) for both; only code 1 also zeroes the row.
        buy_result::ITEM_ALREADY_SOLD | buy_result::ITEM_SOLD_OUT => "ERR_VENDOR_SOLD_OUT", // 0x23
        buy_result::NOT_ENOUGH_MONEY => "ERR_NOT_ENOUGH_MONEY", // 0x25, speaks line 0x28
        buy_result::SELLER_DONT_LIKE_YOU => "ERR_VENDOR_HATES_YOU", // 0x22
        buy_result::DISTANCE_TOO_FAR => "ERR_VENDOR_TOO_FAR",   // 0x24
        BUY_SILENT => return None,
        buy_result::CANT_CARRY_MORE => "ERR_ITEM_MAX_COUNT", // 0x12, speaks line 0x1e
        buy_result::RANK_REQUIRE => "ERR_CANT_EQUIP_RANK",   // 0x05
        buy_result::REPUTATION_REQUIRE => "ERR_CANT_EQUIP_REPUTATION", // 0x06
        _ => "ERR_ITEM_NOT_FOUND",                           // 0x17, the switch's default
    })
}

/// The message a `SMSG_SELL_ITEM` error code shows, from the reference's switch at `0x5dd22c`.
/// Its default is silence: 0 returns before the switch (`0x5dd21e`), and 5 and 7 up skip the
/// `DisplayError`.
fn sell_error_key(reason: u8) -> Option<&'static str> {
    Some(match reason {
        sell_result::CANT_FIND_ITEM => "ERR_ITEM_NOT_FOUND", // 0x17
        sell_result::CANT_SELL_ITEM => "ERR_VENDOR_NOT_INTERESTED", // 0x21
        sell_result::CANT_FIND_VENDOR => "ERR_VENDOR_HATES_YOU", // 0x22
        sell_result::YOU_DONT_OWN_THAT_ITEM => "ERR_NOT_OWNER", // 0x1b
        sell_result::ONLY_EMPTY_BAG => "ERR_DESTROY_NONEMPTY_BAG", // 0x0b
        _ => return None,
    })
}

/// One wire row as a Lua row: the icon from the wire `display_id` at once, the name and stats once
/// the item template answers. Read as `ask` says: `GetMerchantItemInfo` looks it up with a
/// callback (`0x4faf1a`, callback `0x4fada0`).
fn resolve_item(
    item: &VendorItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    ask: Ask<'_>,
) -> MerchantItem {
    let template = items.template(item.entry, 0, ask);
    let name = template.map(|t| t.name.clone());
    let stats = template.map(|t| ItemStatsHead {
        quality: t.quality,
        inventory_type: t.inventory_type,
        class: t.class,
        subclass: t.subclass,
        dmg_min: t.dmg_min,
        dmg_max: t.dmg_max,
        dmg_type: t.dmg_type,
        delay_ms: t.delay_ms,
        armor: t.armor,
        block: t.block,
        sell_price: t.sell_price,
    });
    let texture = icons
        .and_then(|i| i.catalog.get(item.display_id))
        .and_then(|d| d.icon.clone());
    let num_available = if item.current_count == STOCK_UNLIMITED {
        -1
    } else {
        item.current_count as i32
    };
    // `GetMerchantItemLink`'s link, built as the reference's `0x52adb0` builds it; a vendor row
    // has no enchant or random property.
    let link = template.map(|t| item_link(item.entry, &t.name, t.quality));
    MerchantItem {
        name,
        texture,
        price: item.price,
        quantity: item.buy_count,
        num_available,
        item_id: item.entry,
        stats,
        link,
        max_stack: template.map(|t| t.stackable),
    }
}

/// The occupied buyback slots (0-11) in the reference's display order, oldest first: `0x4fafd0`
/// takes the slots with a guid and a price and sorts them by timestamp. Entry `i` is
/// `GetBuybackItemInfo(i + 1)`.
fn buyback_order(store: &benilla_protocol::ObjectFields) -> Vec<u8> {
    let mut v: Vec<(u8, u32)> = (0..12u8)
        .filter_map(|i| {
            let guid = store.player_buyback_slot(i).unwrap_or(0);
            let price = store.player_buyback_price(i).unwrap_or(0);
            (guid != 0 && price != 0).then(|| (i, store.player_buyback_timestamp(i).unwrap_or(0)))
        })
        .collect();
    v.sort_by_key(|&(_, ts)| ts);
    v.into_iter().map(|(i, _)| i).collect()
}

/// One buyback slot as a Lua row: the item from its object, which stays streamed while it sits in
/// the slot, and the price from the player's buyback price field. Read as `ask` says:
/// `GetBuybackItemInfo` looks it up with a callback and guid 0 (`0x4fb3f0`-`0x4fb40c`).
fn resolve_buyback(
    idx: u8,
    store: &benilla_protocol::ObjectFields,
    objects: &(impl crate::ui_items::ItemObjects + ?Sized),
    items: &Items,
    (icons, rolls): (Option<&ItemDisplays>, crate::items::RollCatalogs<'_>),
    ask: Ask<'_>,
) -> MerchantItem {
    let guid = store.player_buyback_slot(idx).unwrap_or(0);
    let obj = objects.object(guid);
    let entry = obj.and_then(|o| o.object_entry()).unwrap_or(0);
    let quantity = obj.and_then(|o| o.item_stack_count()).unwrap_or(1).max(1);
    let template = items.template(entry, 0, ask);
    // The name is the object's full name, `CGItem_C::GetFullName` (`0x4fb42a` → `0x5d8bc0`): the
    // record's with its random-property suffix, none on a wrapped item.
    let property = obj.map_or(0, |o| crate::ui_items::object_link_numbers(o, template).1);
    let name = template.map(|t| rolls.name(&t.name, property));
    let stats = template.map(|t| ItemStatsHead {
        quality: t.quality,
        inventory_type: t.inventory_type,
        class: t.class,
        subclass: t.subclass,
        dmg_min: t.dmg_min,
        dmg_max: t.dmg_max,
        dmg_type: t.dmg_type,
        delay_ms: t.delay_ms,
        armor: t.armor,
        block: t.block,
        sell_price: t.sell_price,
    });
    let display_id = template.map(|t| t.display_info_id).unwrap_or(0);
    let texture = icons
        .and_then(|i| i.catalog.get(display_id))
        .and_then(|d| d.icon.clone());
    MerchantItem {
        name,
        texture,
        price: store.player_buyback_price(idx).unwrap_or(0),
        quantity,
        num_available: 0,
        item_id: entry,
        stats,
        // No link: 1.12 has no `GetBuybackItemLink`, and the buyback click is a bare
        // `BuybackItem(this:GetID())` (`MerchantFrame.lua:358-361`).
        link: None,
        // No max stack: `GetMerchantItemMaxStack` indexes the merchant list, and a buyback row
        // is bought whole.
        max_stack: None,
    }
}

/// One item's repair cost, by the reference's `0x4faf30` arithmetic
/// ([`benilla_formats::DurabilityTables`]), after the price `discount` (`0x4faf8a`). The template
/// is only read (`0x5da368`); a wrapped item costs 0 (`ITEM_FIELD_FLAGS & 0x8`, `0x5da387`).
fn item_repair_cost(
    guid: u64,
    objects: &(impl crate::ui_items::ItemObjects + ?Sized),
    items: &Items,
    tables: &RepairTables,
    discount: f64,
) -> u32 {
    let Some(obj) = objects.object(guid) else {
        return 0;
    };
    if obj.item_flags().unwrap_or(0) & benilla_protocol::messages::ITEM_DYNFLAG_WRAPPED != 0 {
        return 0;
    }
    let cur = obj.item_durability().unwrap_or(0);
    let max = obj.item_max_durability().unwrap_or(0);
    let entry = obj.object_entry().unwrap_or(0);
    if max == 0 || cur >= max || entry == 0 {
        return 0;
    }
    let points = max - cur;
    let Some(t) = items.template(entry, guid, Ask::Read) else {
        return 0;
    };
    let (level, quality, class, subclass) = (t.item_level, t.quality, t.class, t.subclass);
    tables
        .0
        .repair_cost(points, level, quality, class, subclass, discount)
}

/// `GetRepairAllCost`'s walk (`0x4fbd60`): the equipped slots 0 to 18, the backpack's 23 to 38
/// and each item inside the four equipped bags (19 to 22), never the bags themselves, the bank,
/// the keyring or the buy-back slots. An item counts when its flags lack the wrap bit
/// (`0x4fbdf1`) and its maximum durability is above 0 (`0x4fbdfd`); the total sums
/// `0x4faf30`'s costs, and the flag is any counted item below its maximum (`0x4fbe32`).
fn repair_all(
    store: &benilla_protocol::ObjectFields,
    objects: &(impl crate::ui_items::ItemObjects + ?Sized),
    items: &Items,
    tables: Option<&RepairTables>,
    discount: f64,
) -> (u32, bool) {
    let worn = (0..19u8).map(|i| store.player_inv_slot(i).unwrap_or(0));
    let pack = (0..16u8).map(|i| store.player_pack_slot(i).unwrap_or(0));
    let bagged = (19..23u8)
        .filter_map(|i| objects.object(store.player_inv_slot(i).unwrap_or(0)))
        .flat_map(|bag| {
            let n = bag.container_num_slots().unwrap_or(0).min(36) as u8;
            (0..n).map(move |j| bag.container_slot(j).unwrap_or(0))
        });
    let (mut total, mut damaged) = (0u64, false);
    for guid in worn.chain(pack).chain(bagged).filter(|&g| g != 0) {
        let Some(obj) = objects.object(guid) else {
            continue;
        };
        let wrapped =
            obj.item_flags().unwrap_or(0) & benilla_protocol::messages::ITEM_DYNFLAG_WRAPPED != 0;
        let max = obj.item_max_durability().unwrap_or(0);
        if wrapped || max == 0 {
            continue;
        }
        damaged |= obj.item_durability().unwrap_or(0) < max;
        if let Some(t) = tables {
            total += u64::from(item_repair_cost(guid, objects, items, t, discount));
        }
    }
    (total.min(u64::from(u32::MAX)) as u32, damaged)
}

/// The open vendor's guid and descriptor, `None` with none open or its unit not streamed
/// (`0x4fadb0` resolves the cell's guid).
pub(super) fn vendor_unit<'a>(
    open: &MerchantOpen,
    objects: &'a Objects,
) -> Option<(u64, &'a ObjectStore)> {
    let guid = open.vendor?;
    objects.store(guid).map(|store| (guid, store))
}

/// The open vendor's price discount for the player whose descriptor is `player` (`0x612b80`), 0
/// with no vendor or no player unit.
pub(super) fn open_vendor_discount(
    vendor: Option<(u64, &ObjectStore)>,
    relations: &crate::target::RelationWorld,
    player: &ObjectStore,
) -> f64 {
    let (Some((guid, vendor)), Some(me)) = (vendor, relations.me()) else {
        return 0.0;
    };
    crate::target::vendor_price_discount(
        &relations.get(),
        &relations.unit_of(guid, vendor),
        &me,
        player,
    )
}

/// Whether the open vendor repairs (`0x4fadb0`): `UNIT_NPC_FLAGS` bit 14 of its unit, `false`
/// with none open or its unit not streamed.
pub(super) fn vendor_repairs(vendor: Option<(u64, &ObjectStore)>) -> bool {
    vendor.is_some_and(|(_, store)| store.0.unit_npc_flags() & NPC_FLAG_REPAIR != 0)
}

/// What the purse test before a repair's send reads: the items' costs, the open vendor for its
/// price discount, and the message path a refusal takes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RepairPurse<'w, 's> {
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
    tables: Option<Res<'w, RepairTables>>,
    relations: crate::target::RelationWorld<'w, 's>,
    sink: crate::ui_action::MessageSink<'w>,
}

impl RepairPurse<'_, '_> {
    /// The purse test of the four item-click repair arms (`0x4f9cc8`–`0x4f9ccf`) and of
    /// `RepairAllItems` (`0x4fc08e`): the `0x4faf30` costs of `guids`, summed in a u32, above
    /// `PLAYER_FIELD_COINAGE` (unsigned, so an equal cost passes) raise `ERR_NOT_ENOUGH_MONEY`
    /// (`0x496720(0x25)`, speech line 0x28) and answer `true`, and the caller sends nothing.
    pub(crate) fn refuses(
        &mut self,
        script: &mut UiScript,
        open: &MerchantOpen,
        player: &ObjectStore,
        guids: impl IntoIterator<Item = u64>,
    ) -> bool {
        // `0x4faf30` keeps the full price unless it resolves the vendor's unit.
        let discount =
            open_vendor_discount(vendor_unit(open, &self.objects), &self.relations, player);
        let cost = guids.into_iter().filter(|&g| g != 0).fold(0u32, |sum, g| {
            let item = self.tables.as_deref().map_or(0, |t| {
                item_repair_cost(g, &self.objects, &self.items, t, discount)
            });
            sum.wrapping_add(item)
        });
        let money = player.0.player_money().unwrap_or(0);
        if cost <= money {
            return false;
        }
        debug!("ui_merchant: repair of {cost} copper refused with {money} in the purse");
        if let Some(line) = crate::ui_action::keyed_line(script, "ERR_NOT_ENOUGH_MONEY") {
            crate::ui_action::show_messages(script, &mut self.sink, "ui_merchant", [line]);
        }
        true
    }
}

/// The vendor's cell `[0xbddfa0]`; `CloseMerchant` (`0x4fb120`) is the core itself, run inside
/// its call ([`crate::script_calls::ScriptCallHost`]).
impl LatchedWindow for MerchantOpen {
    const WINDOW: NpcWindow = NpcWindow::Merchant;

    fn cell(&self) -> Option<u64> {
        self.vendor
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }
}

impl MerchantOpen {
    /// The world-enter cascade's merchant reset (`0x4909b0` → `0x4fac50`), which signals nothing:
    /// the vendor and its rows zeroed (`0x4fac61`-`0x4fac7a`), the list cache's guid kept. The
    /// buy-back watch it registers (`0x4fac8d`) is the field watchers' standing one, and the list
    /// it rebuilds (`0x4fafd0`) is read off the player's fields at each call. Owed from the
    /// cascade's push, it runs at the cascade's place or ahead of a later packet's write
    /// ([`crate::ui_world_enter::OwedReset`]).
    pub(crate) fn settle_world_reset(&mut self) {
        if self.world_reset.take() {
            self.vendor = None;
            self.items.clear();
        }
    }
}

/// The merchant reset in the cascade's place, unless a later packet ran it first: the engine's
/// vendor and rows, then the VM's vendor.
pub(crate) fn reset_on_world_enter(
    mut vm: crate::script_calls::VmWith<Option<ResMut<'static, MerchantOpen>>>,
) {
    if let Some(mut open) = vm.own() {
        open.settle_world_reset();
    }
    vm.run(|script| {
        if let Some(script) = script {
            script.set_merchant(None);
        }
    });
}

/// The close core `0x4fadf0`, every close's: with a vendor open, the latch release (`0x4fae04`),
/// the cell cleared, `MERCHANT_CLOSED` (`0x4fae2c`), then the tail ([`close_tail`]).
/// The leave-world sweep's leg (`0x490ada` → `0x4facc0`) calls it as it is.
pub(crate) fn close_core(world: &mut World) {
    let Some(vendor) = world.resource::<MerchantOpen>().vendor else {
        return;
    };
    lane::behind_lane(world, |world| {
        crate::ui_session::release(world, vendor);
        world.resource_mut::<MerchantOpen>().clear();
        crate::ui_session::fire(world, |script| {
            script.fire_event("MERCHANT_CLOSED", vec![]);
            close_tail(script);
        });
    });
}

/// `SMSG_LIST_INVENTORY` opens the window on the vendor's rows (`0x4facf0`): the old rows' record
/// callbacks cancelled while the window holds rows (`0x4fad01`-`0x4fad1b`), which a close zeroes
/// (`0x4fae22`), so a list after a close leaves the waits standing; the Vendors tutorial, on every
/// list (`0x4fad32`); then the latch, registered
/// with no test of its own cell (`0x4fad40`), so a list from the open vendor closes it and opens
/// it again, the cell (`0x4fad4b`), the rows, the wait count at 0 (`0x4fad88`) and
/// `MERCHANT_SHOW` (`0x4fad92`).
pub(crate) fn open_merchant(world: &mut World, vendor: u64, items: Vec<VendorItem>) {
    debug!("net: vendor {vendor:#x} listed {} items", items.len());
    let held = world
        .get_resource::<MerchantOpen>()
        .is_some_and(|open| open.vendor.is_some() && !open.items.is_empty());
    if let (true, Some(cache)) = (held, world.get_resource::<Items>()) {
        cache.cancel_waits(crate::items::RecordCallback::Merchant);
    }
    if let Some(tutorials) = world.get_resource::<crate::tutorial::Tutorials>() {
        tutorials.trigger_now(crate::tutorial::id::VENDORS);
    }
    crate::ui_session::register(world, vendor);
    world.resource_mut::<MerchantOpen>().open(vendor, items);
    if let Some(mut lane) = world.get_resource_mut::<MerchantLane>() {
        lane.push(MerchantEvent::Show);
    }
}

/// The close core's tail (`0x4fadf0`, every close's), after `MERCHANT_CLOSED`: a vendor row held
/// on the cursor, mode 5 with its entry stored (`0x4fae31`, `[0xb4b41c]` through `0x495170`), is
/// cleared (`0x4fae4a` → `ClearCursor(1, 1)`), then a Repair base mode returns to Point
/// (`0x4fae4f`-`0x4fae68`).
pub(crate) fn close_tail(script: &mut UiScript) {
    if matches!(
        script.held_cursor().payload(),
        Some(benilla_ui::script::CursorPayload::Merchant(_))
    ) {
        script.clear_cursor_payload();
    }
    script.end_repair_mode();
}

/// The world frame's right-press hook's last leg (`0x492c20`, `0x492c59`-`0x492c72`): a Repair
/// base mode returns to Point (`0x492c68`). It tests no payload and consumes nothing
/// ([`crate::target::right_press_hook`]).
pub(crate) fn end_repair_mode_on_right_press(mut hosted_vm: crate::script_calls::HostedVm) {
    hosted_vm.run(|script| {
        if let Some(script) = script {
            script.end_repair_mode();
        }
    })
}

/// A level-20 sword 40 points down at a repair vendor flagged `UNIT_FLAG_PVP`, for the purse tests
/// here and in [`crate::ui_items`].
#[cfg(test)]
pub(crate) mod purse_fixture {
    use super::*;
    use crate::net::{Guid, NetCommands, SelfPlayer};
    use benilla_protocol::field::{
        FIELD_PLAYER_FIELD_COINAGE, FIELD_UNIT_FLAGS, FIELD_UNIT_NPC_FLAGS,
    };
    use benilla_protocol::ObjectFields;

    pub(crate) const VENDOR: u64 = 0xF130_0000_0000_0043;
    pub(crate) const SWORD: u64 = 0x4000_0000_0000_0707;
    pub(crate) const SWORD_ENTRY: u32 = 2488;
    /// Absolute descriptor indices.
    const OBJECT_ENTRY: u16 = 3;
    const ITEM_DURABILITY: u16 = 46;
    const ITEM_MAXDURABILITY: u16 = 47;
    const PLAYER_BYTES_3: u16 = 195;
    /// The string `ERR_NOT_ENOUGH_MONEY` holds in 1.12's `GlobalStrings.lua`.
    pub(crate) const NOT_ENOUGH_MONEY: &str = "You don't have enough money.";

    /// One level-20 row at 1 copper a point for a sword (weapon subclass 7), and quality 1's
    /// multiplier (position `2·1 + 1`) at 1.
    fn tables() -> benilla_formats::DurabilityTables {
        let mut row = vec![0u32; 29];
        row[7] = 1;
        benilla_formats::DurabilityTables::from_rows([(20, row)], [1.0; 4])
    }

    /// The sword's repair in full, and after the two honor steps (`0x612bcf`, `0x612bd7`) an honor
    /// rank byte of 8 earns at this vendor; a test at the discounted price discriminates the
    /// discount only while the two differ.
    pub(crate) fn sword_prices() -> (u32, u32) {
        let t = tables();
        let steps = f64::from(0.05f32) + f64::from(0.05f32);
        let full = t.repair_cost(40, 20, 1, 2, 7, 0.0);
        let discounted = t.repair_cost(40, 20, 1, 2, 7, steps);
        assert!(discounted < full, "{discounted} of {full}");
        (full, discounted)
    }

    /// The merchant open on the vendor, whose unit is streamed, the tables, and the sword's
    /// template and object.
    pub(crate) fn seat(world: &mut World) {
        world.insert_resource(RepairTables(tables()));
        world
            .resource_mut::<MerchantOpen>()
            .open(VENDOR, Vec::new());
        let vendor = world
            .spawn((
                Guid(VENDOR),
                ObjectStore(ObjectFields::from_pairs(&[
                    (FIELD_UNIT_NPC_FLAGS, NPC_FLAG_REPAIR),
                    (FIELD_UNIT_FLAGS, 0x1000),
                ])),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(VENDOR, vendor);
        let mut sword = crate::items::test_template("Sword");
        (sword.class, sword.subclass, sword.quality, sword.item_level) = (2, 7, 1, 20);
        world
            .resource_mut::<Items>()
            .insert_template(SWORD_ENTRY, Some(sword));
        crate::items::test_spawn_item(
            world,
            SWORD,
            ObjectFields::from_pairs(&[
                (OBJECT_ENTRY, SWORD_ENTRY),
                (ITEM_DURABILITY, 35),
                (ITEM_MAXDURABILITY, 75),
            ]),
            false,
        );
    }

    /// The player's fields: the sword in the guid pair at `slot_field`, `money` copper, and the
    /// honor rank byte at 8.
    pub(crate) fn player(slot_field: u16, money: u32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (slot_field, SWORD as u32),
            (slot_field + 1, (SWORD >> 32) as u32),
            (FIELD_PLAYER_FIELD_COINAGE, money),
            (PLAYER_BYTES_3, 8 << 24),
        ]))
    }

    /// [`seat`] and the player with the sword at `slot_field`; returns the player's entity too.
    pub(crate) fn repair_app(slot_field: u16) -> (App, Entity) {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<MerchantOpen>()
            .init_resource::<crate::net::Reputations>()
            .insert_resource(NetCommands(tx));
        seat(app.world_mut());
        let me = app
            .world_mut()
            .spawn((SelfPlayer, Guid(0x5e1f), player(slot_field, 0)))
            .id();
        (app, me)
    }

    /// `chunk`'s answer with a host lent whose one family is the merchant's ([`live`]): the
    /// tooltips' costs read the world at the call.
    pub(crate) fn live_eval<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
        struct Host<'a, 'w, 's>(&'a mut live::MerchantLive<'w, 's>);
        impl benilla_ui::script::CallHost for Host<'_, '_, '_> {
            fn apply(
                &mut self,
                _: &mut UiScript,
                _: benilla_ui::script::ScriptCall,
            ) -> Option<benilla_ui::script::CallTail> {
                None
            }
            fn read_live(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
                crate::script_calls::live::LiveFamily::answer(self.0, ask)
            }
        }
        let world = app.world_mut();
        let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
        let mut state = bevy::ecs::system::SystemState::<live::MerchantLive>::new(world);
        let mut family = state.get_mut(world);
        let r = script.hosted(&mut Host(&mut family), |s| {
            s.eval::<T>(chunk).expect("runs")
        });
        world.insert_non_send_resource(script);
        r
    }

    /// A VM with the sword at live id 16, backpack slot 1 and bank slot 1, and a tooltip `TT`.
    pub(crate) fn sword_vm() -> UiScript {
        let mut script = UiScript::new().unwrap();
        crate::ui_script::test_ui::load_tooltip_template(&script);
        let mut inv: benilla_ui::script::InventorySlots = Default::default();
        inv[16] = Some(benilla_ui::script::InvSlotView {
            item_id: SWORD_ENTRY,
            ..Default::default()
        });
        script.set_inventory_slots(inv);
        let mut bag = benilla_ui::script::ContainerState {
            num_slots: 16,
            ..Default::default()
        };
        bag.slots.insert(
            1,
            benilla_ui::script::ContainerSlot {
                item_id: SWORD_ENTRY,
                count: 1,
                ..Default::default()
            },
        );
        script.set_container(0, Some(bag.clone()));
        script.set_container(crate::ui_items::BANK_CONTAINER, Some(bag));
        script
            .run(
                r#"CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate"):SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")"#,
            )
            .unwrap();
        script
    }

    /// The message's string, and a recorder of the `UI_ERROR_MESSAGE` lines [`shown`] reads.
    pub(crate) fn record_errors(script: &UiScript) {
        script
            .run(&format!(
                r#"
                ERR_NOT_ENOUGH_MONEY = "{NOT_ENOUGH_MONEY}"
                BENILLA_TEST_LINES = {{}}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UI_ERROR_MESSAGE")
                f:SetScript("OnEvent", function() table.insert(BENILLA_TEST_LINES, arg1) end)
                "#
            ))
            .unwrap();
    }

    /// The `UI_ERROR_MESSAGE` lines since [`record_errors`].
    pub(crate) fn shown(script: &UiScript) -> Vec<String> {
        script
            .eval::<Vec<String>>("return BENILLA_TEST_LINES")
            .unwrap()
    }

    /// The message rows queued for their sound.
    pub(crate) fn sounded(world: &World) -> Vec<&'static str> {
        world
            .resource::<crate::sound::MessageSounds>()
            .queued()
            .iter()
            .map(|r| r.key)
            .collect()
    }
}

#[cfg(test)]
mod call_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{ClientCommand, Guid, NetCommands, SelfPlayer};
    use benilla_ui::script::MerchantState;

    /// `GetRepairAllCost` sums each item's cost after the open merchant's discount (`0x4faf8a`):
    /// at Honored with a PvP-flagged vendor and honor rank byte 8, 0.2 comes off each item before
    /// its rounding, and the undiscounted total is not what the VM reads.
    #[test]
    fn the_repair_all_total_takes_the_merchants_discount() {
        use benilla_protocol::field::{
            FIELD_PLAYER_INV_SLOT_HEAD, FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS,
            FIELD_UNIT_NPC_FLAGS,
        };
        use benilla_protocol::ObjectFields;
        use bevy::ecs::system::RunSystemOnce;

        /// Absolute descriptor indices.
        const OBJECT_ENTRY: u16 = 3;
        const ITEM_DURABILITY: u16 = 46;
        const ITEM_MAXDURABILITY: u16 = 47;
        const BYTES_0: u16 = 36;
        const PLAYER_BYTES_3: u16 = 195;
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        const SWORD: u64 = 0x4000_0000_0000_0007;
        const SWORD_ENTRY: u32 = 2488;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let tables = benilla_formats::load_durability_tables(&mut chain).expect("durability");
        let (factions, template, reps) = crate::target::stormwind_fixture(&mut chain, 9000);
        // A level 20 green sword (class 2, subclass 7) 40 points down.
        let disc = f64::from(0.1f32) + f64::from(0.05f32) + f64::from(0.05f32);
        let want = tables.repair_cost(40, 20, 2, 2, 7, disc);
        let full = tables.repair_cost(40, 20, 2, 2, 7, 0.0);
        assert!(
            want > 0 && want < full,
            "the case discriminates: {want} of {full}"
        );

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx))
            .insert_resource(RepairTables(tables))
            .insert_resource(factions)
            .insert_resource(reps);
        let mut open = MerchantOpen::default();
        open.open(VENDOR, Vec::new());
        app.insert_resource(open);
        let mut sword = crate::items::test_template("Sword");
        (sword.class, sword.subclass, sword.quality, sword.item_level) = (2, 7, 2, 20);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(SWORD_ENTRY, Some(sword));
        crate::items::test_spawn_item(
            app.world_mut(),
            SWORD,
            ObjectFields::from_pairs(&[
                (OBJECT_ENTRY, SWORD_ENTRY),
                (ITEM_DURABILITY, 35),
                (ITEM_MAXDURABILITY, 75),
            ]),
            false,
        );
        // A human warrior player, the sword in the main hand (slot 15); the current honor rank
        // byte 8. The vendor's standing leg needs the player's type, flag `0x8` and template
        // (`0x606576`-`0x6065b6`).
        let main_hand = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
        app.world_mut().spawn((
            SelfPlayer,
            Guid(0x10),
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x19), // OBJECT_FIELD_TYPE
                (FIELD_UNIT_FACTIONTEMPLATE, 1),
                (FIELD_UNIT_FLAGS, 0x8),
                (BYTES_0, crate::target::HUMAN_WARRIOR),
                (PLAYER_BYTES_3, 8 << 24),
                (main_hand, SWORD as u32),
                (main_hand + 1, (SWORD >> 32) as u32),
            ])),
        ));
        let vendor = app
            .world_mut()
            .spawn((
                Guid(VENDOR),
                ObjectStore(ObjectFields::from_pairs(&[
                    (FIELD_UNIT_NPC_FLAGS, NPC_FLAG_REPAIR),
                    (FIELD_UNIT_FACTIONTEMPLATE, template),
                    (FIELD_UNIT_FLAGS, 0x1000),
                ])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(VENDOR, vendor);
        let got = app
            .world_mut()
            .run_system_once(|live: live::MerchantLive| live.repair_all())
            .unwrap();
        assert_eq!(
            got,
            Some(Some((want, true))),
            "the undiscounted total is {full}"
        );
    }

    /// A carried item's own cost (`0x5da330`) rides beside the discounted one, by its guid: the
    /// merchant price takes off the cost before the discount (`0x52e44a`).
    #[test]
    fn a_carried_items_own_cost_rides_beside_the_discounted_one() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        use bevy::ecs::system::RunSystemOnce;
        use purse_fixture::*;

        let (full, discounted) = sword_prices();
        let (mut app, _) = repair_app(FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23);
        let costs = app
            .world_mut()
            .run_system_once(|live: live::MerchantLive| {
                (live.repair_cost(0, 1), live.own_repair_cost(SWORD))
            })
            .unwrap();
        assert_eq!(costs, (Some(discounted), Some(full)));
    }

    /// `RepairAllItems()` through the feed and the host, at [`purse_fixture`]'s vendor carrying
    /// `npc_flags`, with the sword at `slot_field` and `money` copper: the packets, the
    /// `UI_ERROR_MESSAGE` lines and the message rows queued for their sound.
    fn repair_all(
        npc_flags: u32,
        slot_field: u16,
        money: u32,
    ) -> (Vec<ClientCommand>, Vec<String>, Vec<&'static str>) {
        use benilla_protocol::field::FIELD_UNIT_NPC_FLAGS;
        use benilla_protocol::ObjectFields;
        use bevy::ecs::system::RunSystemOnce;
        use purse_fixture::*;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<MerchantOpen>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .insert_resource(NetCommands(tx));
        seat(app.world_mut());
        let mut units = app.world_mut().query::<(&Guid, &mut ObjectStore)>();
        for (guid, mut store) in units.iter_mut(app.world_mut()) {
            if guid.0 == VENDOR {
                let flags = ObjectFields::from_pairs(&[(FIELD_UNIT_NPC_FLAGS, npc_flags)]);
                store.0.merge(flags);
            }
        }
        app.world_mut()
            .spawn((SelfPlayer, Guid(0x5e1f), player(slot_field, money)));
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
        record_errors(&script);
        // The VM's copy of the vendor's repair flag, which `RepairAllItems` gates on with no host
        // to ask.
        script.set_merchant(Some(MerchantState {
            can_repair: npc_flags & NPC_FLAG_REPAIR != 0,
            ..Default::default()
        }));
        app.insert_non_send_resource(script);
        let mut calls = {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.run("RepairAllItems()").unwrap();
            script.take_merchant_calls()
        };
        app.world_mut()
            .run_system_once(
                move |mut steps: MerchantSteps, mut script: NonSendMut<UiScript>| {
                    for call in std::mem::take(&mut calls) {
                        steps.apply(&mut script, call);
                    }
                },
            )
            .unwrap();
        let sent = rx.try_iter().collect();
        let lines = shown(app.world().non_send_resource::<UiScript>());
        (sent, lines, sounded(app.world()))
    }

    /// At a vendor that does not repair, `RepairAllItems` sends nothing and says nothing: it
    /// returns (`0x4fbff0`) unless `0x4fadb0` finds the repair flag on the open merchant, before
    /// its purse test. The same worn sword at a repairer goes out.
    #[test]
    fn repair_all_does_nothing_at_a_vendor_that_does_not_repair() {
        use crate::target::cursor_mode::npc_flags::VENDOR as NPC_FLAG_VENDOR;
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        use purse_fixture::*;

        let (_, discounted) = sword_prices();
        let main_hand = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
        for money in [discounted, discounted - 1] {
            let (sent, lines, rows) = repair_all(NPC_FLAG_VENDOR, main_hand, money);
            assert!(sent.is_empty(), "{money} copper: no repair goes out");
            assert!(
                lines.is_empty() && rows.is_empty(),
                "{money} copper: no purse refusal"
            );
        }
        let (sent, ..) = repair_all(NPC_FLAG_VENDOR | NPC_FLAG_REPAIR, main_hand, discounted);
        assert!(matches!(
            sent.as_slice(),
            [ClientCommand::RepairItem {
                vendor: VENDOR,
                item_guid: 0
            }]
        ));
    }

    /// `RepairAllItems` prices only the equipped slots for its purse test (`0x4fc026`–`0x4fc063`,
    /// `0x4fc08e`): the sword worn and over the purse is refused with `ERR_NOT_ENOUGH_MONEY` and
    /// no send; the same sword in the backpack, which `GetRepairAllCost` would count, leaves a worn
    /// total of 0 and the repair-all goes out.
    #[test]
    fn repair_all_tests_the_purse_against_the_worn_gear_alone() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        use purse_fixture::*;

        let (_, discounted) = sword_prices();

        // The main hand (slot 15).
        let main_hand = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
        let (sent, lines, rows) = repair_all(NPC_FLAG_REPAIR, main_hand, discounted - 1);
        assert!(sent.is_empty(), "worn, one copper short: nothing goes out");
        assert_eq!(lines, [NOT_ENOUGH_MONEY]);
        assert_eq!(
            rows,
            ["ERR_NOT_ENOUGH_MONEY"],
            "the row that speaks line 0x28"
        );

        // Backpack slot 1, inventory slot 23.
        let backpack_1 = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
        let (sent, lines, rows) = repair_all(NPC_FLAG_REPAIR, backpack_1, discounted - 1);
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::RepairItem {
                    vendor: VENDOR,
                    item_guid: 0
                }]
            ),
            "the bag's {discounted} is not in the purse test"
        );
        assert!(lines.is_empty() && rows.is_empty());
    }

    /// An item's cost is discounted at the vendor and full once it closes (`0x4faf30`).
    #[test]
    fn each_items_cost_takes_the_open_vendors_discount() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        use purse_fixture::*;

        let (full, discounted) = sword_prices();
        let priced = |slot_field: u16, read: &str| {
            let (mut app, _) = repair_app(slot_field);
            app.insert_non_send_resource(sword_vm());
            let read_cost = |app: &mut App| live_eval::<i64>(app, read);
            let at_vendor = read_cost(&mut app);
            app.world_mut().resource_mut::<MerchantOpen>().clear();
            (at_vendor, read_cost(&mut app))
        };
        let want = (i64::from(discounted), i64::from(full));

        // Main hand, inventory slot 15.
        let worn = priced(
            FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15,
            r#"local _, _, c = TT:SetInventoryItem("player", 16) return c"#,
        );
        assert_eq!(worn, want, "worn: at the vendor, then with none open");
        // Backpack slot 1, inventory slot 23.
        let carried = priced(
            FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23,
            "local _, c = TT:SetBagItem(0, 1) return c",
        );
        assert_eq!(
            carried, want,
            "in the backpack: at the vendor, then with none open"
        );
    }

    /// A wrapped item costs nothing to repair: `0x5da330` answers 0 when `ITEM_FIELD_FLAGS` has
    /// 0x8 (`0x5da387`), ahead of the durability arithmetic, so the displayed cost `0x4faf30` is 0.
    #[test]
    fn a_wrapped_items_repair_costs_nothing() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        use purse_fixture::*;
        /// `ITEM_FIELD_FLAGS`, and its wrapped bit.
        const ITEM_FLAGS: u16 = 21;
        const WRAPPED: u32 = 0x8;
        let (mut app, _) = repair_app(FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15);
        app.insert_non_send_resource(sword_vm());
        let cost = |app: &mut App| {
            live_eval::<i64>(
                app,
                r#"local _, _, c = TT:SetInventoryItem("player", 16) return c"#,
            )
        };
        assert!(cost(&mut app) > 0, "the damaged sword costs");
        let sword = app.world().resource::<crate::net::GuidIndex>().0[&SWORD];
        app.world_mut()
            .get_mut::<ObjectStore>(sword)
            .unwrap()
            .0
            .merge(benilla_protocol::ObjectFields::from_pairs(&[(
                ITEM_FLAGS, WRAPPED,
            )]));
        assert_eq!(cost(&mut app), 0, "wrapped, it costs nothing");
    }

    /// Each cost reads the world at the call: the tables, the durability, the slot, the vendor's
    /// discount and the record, each written alone with no push between; a bank item is priced
    /// too.
    #[test]
    fn each_cost_reads_the_world_at_the_call() {
        use benilla_protocol::field::{FIELD_PLAYER_INV_SLOT_HEAD, FIELD_UNIT_FLAGS};
        use benilla_protocol::ObjectFields;
        use purse_fixture::*;

        /// `ITEM_FIELD_DURABILITY`, absolute.
        const ITEM_DURABILITY: u16 = 46;
        let main_hand = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
        let backpack_1 = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
        // `PLAYER_FIELD_BANK_SLOT_1`, inventory slot 39.
        let bank_1 = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 39;

        let (mut app, me) = repair_app(main_hand);
        let tables = app.world_mut().remove_resource::<RepairTables>().unwrap();
        let mut script = sword_vm();
        script.set_merchant(Some(MerchantState {
            can_repair: true,
            ..Default::default()
        }));
        // A banker open too: the vault's slots read empty to the tooltip without one
        // (`0x6228a0`).
        script.set_bank(Some(benilla_ui::script::BankState::default()));
        app.insert_non_send_resource(script);

        let entity =
            |app: &App, guid: u64| app.world().resource::<crate::net::GuidIndex>().0[&guid];
        let write = |app: &mut App, e: Entity, pairs: &[(u16, u32)]| {
            let mut store = app.world_mut().get_mut::<ObjectStore>(e).unwrap();
            store.0.merge(ObjectFields::from_pairs(pairs));
        };
        // (worn, backpack, bank) at the call.
        let frame = |app: &mut App| {
            live_eval::<(i64, i64, i64)>(
                app,
                r#"local _, _, worn = TT:SetInventoryItem("player", 16)
                   local _, carried = TT:SetBagItem(0, 1)
                   local _, banked = TT:SetBagItem(-1, 1)
                   return worn, carried, banked"#,
            )
        };
        let vendor = app
            .world_mut()
            .query::<(Entity, &Guid)>()
            .iter(app.world())
            .find(|(_, g)| g.0 == VENDOR)
            .unwrap()
            .0;
        let hi = (SWORD >> 32) as u32;
        let moved =
            |from: u16, to: u16| [(from, 0), (from + 1, 0), (to, SWORD as u32), (to + 1, hi)];

        assert_eq!(frame(&mut app), (0, 0, 0), "no tables, no prices");
        // 1 copper a point, less 0.1: 40 points cost 36.
        app.insert_resource(tables);
        assert_eq!(frame(&mut app), (36, 0, 0), "the tables arrive");
        let sword = entity(&app, SWORD);
        write(&mut app, sword, &[(ITEM_DURABILITY, 55)]);
        assert_eq!(frame(&mut app), (18, 0, 0), "a repair's write: 20 points");
        write(&mut app, me, &moved(main_hand, bank_1));
        assert_eq!(frame(&mut app), (0, 0, 18), "banked: priced");
        write(&mut app, me, &moved(bank_1, backpack_1));
        assert_eq!(frame(&mut app), (0, 18, 0), "the sword in the backpack");
        write(&mut app, vendor, &[(FIELD_UNIT_FLAGS, 0)]);
        assert_eq!(
            frame(&mut app),
            (0, 20, 0),
            "no PvP flag, no honor discount"
        );
        app.world_mut().resource_mut::<MerchantOpen>().clear();
        assert_eq!(frame(&mut app), (0, 20, 0), "no vendor: the full price");
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(SWORD_ENTRY, None);
        assert_eq!(
            frame(&mut app),
            (0, 0, 0),
            "an unknown template prices nothing"
        );
    }

    fn row(entry: u32, slot: u32, count: u32) -> VendorItem {
        VendorItem {
            slot,
            entry,
            display_id: 100 + entry,
            current_count: count,
            price: 500,
            max_durability: 0,
            buy_count: 1,
        }
    }

    /// The ids are asserted, not just the keys: each is its arm's `push <id>; call 0x496720`.
    #[test]
    fn the_refusal_tables_are_the_references_own() {
        let id = |key: &str| {
            benilla_ui::messages::by_key(key)
                .unwrap_or_else(|| panic!("{key} is not a catalog row"))
                .id
        };
        // `SMSG_BUY_FAILED`, the switch at `0x5dcdd8`: 1 and 7 share `0x23`, 6 is silent, and
        // codes outside the table fall to `0x17`.
        for (code, want) in [
            (0u8, Some(0x17u16)),
            (1, Some(0x23)),
            (2, Some(0x25)),
            (3, Some(0x17)),
            (4, Some(0x22)),
            (5, Some(0x24)),
            (6, None),
            (7, Some(0x23)),
            (8, Some(0x12)),
            (9, Some(0x17)),
            (10, Some(0x17)),
            (11, Some(0x05)),
            (12, Some(0x06)),
            (13, Some(0x17)),
            (255, Some(0x17)),
        ] {
            assert_eq!(buy_error_key(code).map(id), want, "buy code {code}");
        }
        // `SMSG_SELL_ITEM`, the switch at `0x5dd22c`, where the default is silence.
        for (code, want) in [
            (0u8, None),
            (1, Some(0x17u16)),
            (2, Some(0x21)),
            (3, Some(0x22)),
            (4, Some(0x1b)),
            (5, None),
            (6, Some(0x0b)),
            (7, None),
            (255, None),
        ] {
            assert_eq!(sell_error_key(code).map(id), want, "sell code {code}");
        }
    }

    /// Only these two vendor refusals speak.
    #[test]
    fn the_purse_refusals_carry_their_voice_lines() {
        let tag = |key: &str| benilla_ui::messages::by_key(key).expect("row").type_tag;
        assert_eq!(tag("ERR_NOT_ENOUGH_MONEY"), 0x28);
        assert_eq!(tag("ERR_ITEM_MAX_COUNT"), 0x1e);
        for key in [
            "ERR_VENDOR_SOLD_OUT",
            "ERR_VENDOR_HATES_YOU",
            "ERR_VENDOR_TOO_FAR",
            "ERR_ITEM_NOT_FOUND",
            "ERR_VENDOR_NOT_INTERESTED",
            "ERR_NOT_OWNER",
            "ERR_DESTROY_NONEMPTY_BAG",
            "ERR_CANT_EQUIP_RANK",
            "ERR_CANT_EQUIP_REPUTATION",
        ] {
            assert_eq!(tag(key), 0x44, "{key}");
        }
    }

    /// The reference's cache write (`0x5dcdbf`), keyed by entry as vmangos fills the field.
    #[test]
    fn a_sold_out_refusal_zeroes_only_that_rows_count() {
        let mut open = MerchantOpen::default();
        open.open(7, vec![row(11, 1, 3), row(22, 2, 5), row(11, 3, 2)]);
        open.sold_out(11);
        assert_eq!(open.items[0].current_count, 0);
        assert_eq!(
            open.items[1].current_count, 5,
            "a different entry is untouched"
        );
        assert_eq!(
            open.items[2].current_count, 0,
            "the reference does not stop at the first match"
        );
        open.sold_out(999);
        assert_eq!(
            open.items[1].current_count, 5,
            "an entry we do not stock is a no-op"
        );
    }

    #[test]
    fn row_at_maps_the_one_based_row() {
        let mut open = MerchantOpen::default();
        assert!(!open.is_open());
        open.open(0x42, vec![row(159, 1, STOCK_UNLIMITED), row(4540, 2, 5)]);
        assert!(open.is_open());
        let entry = |row| open.row_at(row).map(|r| r.entry);
        assert_eq!(entry(1), Some(159));
        assert_eq!(entry(2), Some(4540));
        assert_eq!(entry(3), None); // out of range
        assert_eq!(entry(0), None); // 0 has no row (1-based)
    }

    #[test]
    fn resolve_maps_unlimited_stock_to_minus_one() {
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        // Unlimited stock is `numAvailable` -1; a finite count passes through.
        let unlimited = resolve_item(
            &row(159, 1, STOCK_UNLIMITED),
            &items,
            None,
            Ask::New(&commands),
        );
        assert_eq!(unlimited.num_available, -1);
        assert_eq!(unlimited.item_id, 159);
        let finite = resolve_item(&row(4540, 2, 5), &items, None, Ask::New(&commands));
        assert_eq!(finite.num_available, 5);
        // No template answer yet: no name or stats, the rest present.
        assert!(finite.name.is_none());
        assert!(finite.stats.is_none());
        assert_eq!(finite.price, 500);
    }

    #[test]
    fn resolve_fills_the_tooltip_stats_from_the_template() {
        let mut items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        items.insert_template(
            2129,
            Some(benilla_protocol::messages::ItemInfo {
                class: 4,
                subclass: 6,
                name: "Chipped Buckler".into(),
                display_info_id: 18730,
                quality: 1,
                flags: 0,
                buy_price: 0,
                sell_price: 0,
                inventory_type: 14,
                allowable_class: -1,
                allowable_race: -1,
                item_level: 0,
                required_level: 0,
                required_skill: 0,
                required_skill_rank: 0,
                required_spell: 0,
                required_honor_rank: 0,
                required_city_rank: 0,
                required_rep_faction: 0,
                required_rep_rank: 0,
                max_count: 0,
                stackable: 1,
                container_slots: 0,
                stats: Vec::new(),
                damages: Vec::new(),
                dmg_min: 0.0,
                dmg_max: 0.0,
                dmg_type: 0,
                armor: 85,
                resistances: [0; 6],
                delay_ms: 0,
                ammo_type: 0,
                ranged_mod_range: 0.0,
                spells: Vec::new(),
                spell_charges_0: 0,
                use_spell: None,
                bonding: 0,
                description: String::new(),
                page_text: 0,
                language_id: 0,
                page_material: 0,
                start_quest: 0,
                lock_id: 0,
                material: 0,
                sheath: 4,
                random_property: 0,
                block: 1,
                item_set: 0,
                max_durability: 0,
                area: 0,
                map: 0,
                bag_family: 0,
            }),
        );
        let resolved = resolve_item(&row(2129, 1, 3), &items, None, Ask::New(&commands));
        let stats = resolved.stats.expect("template answered → stats present");
        assert_eq!(
            (
                stats.quality,
                stats.inventory_type,
                stats.class,
                stats.subclass
            ),
            (1, 14, 4, 6)
        );
        assert_eq!((stats.armor, stats.block), (85, 1));
    }

    #[test]
    fn update_stock_moves_the_matching_row() {
        let mut open = MerchantOpen::default();
        open.open(0x42, vec![row(159, 1, STOCK_UNLIMITED), row(4540, 2, 5)]);
        assert_eq!(open.update_stock(2, 4), Some(4540));
        assert_eq!(open.items[1].current_count, 4);
        assert_eq!(open.update_stock(99, 0), None); // no such slot: a no-op
        assert_eq!(open.items[0].current_count, STOCK_UNLIMITED);
    }

    #[test]
    fn clear_closes_the_window() {
        let mut open = MerchantOpen::default();
        open.open(0x42, vec![row(159, 1, 3)]);
        open.clear();
        assert!(!open.is_open());
        assert_eq!(open.listed, Some(0x42), "the list cache outlives the close");
        open.clear_session();
        assert_eq!(open.listed, None);
        assert!(open.items.is_empty());
    }
}
