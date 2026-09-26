//! The merchant window's app side: [`MerchantOpen`] holds the `SMSG_LIST_INVENTORY` rows,
//! [`feed_merchant`] pushes them with the buyback and repair rows, the purse and the refusals,
//! [`feed_repair_all_cost`] the repair-all total, and [`drain_merchant`] sends the Lua intents. A
//! bag click sells through [`crate::ui_items`].

use benilla_protocol::messages::{buy_result, sell_result, VendorItem};
use benilla_world::interact::WorldRightPress;
use bevy::prelude::*;

use benilla_ui::script::{ItemStatsHead, MerchantItem, MerchantState, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_items::{item_link, slot_guid, wire_pos};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, npc_switched, NpcSession};
use benilla_protocol::messages::BAG_PLAYER_INVENTORY;

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

/// The open vendor and its `SMSG_LIST_INVENTORY` rows, until a close or a disconnect.
#[derive(Resource, Default)]
pub(crate) struct MerchantOpen {
    pub(crate) vendor: Option<u64>,
    /// In wire order, which is the 1-based display order.
    pub(crate) items: Vec<VendorItem>,
}

impl MerchantOpen {
    pub(crate) fn open(&mut self, vendor: u64, items: Vec<VendorItem>) {
        self.vendor = Some(vendor);
        self.items = items;
    }

    pub(crate) fn is_open(&self) -> bool {
        self.vendor.is_some()
    }

    /// The item entry at a 1-based row: `CMSG_BUY_ITEM` buys by entry, not by vendor slot.
    pub(crate) fn entry_at(&self, index_1based: u32) -> Option<u32> {
        index_1based
            .checked_sub(1)
            .and_then(|i| self.items.get(i as usize))
            .map(|it| it.entry)
    }

    /// `SMSG_BUY_ITEM`'s new count for the row at 1-based vendor `slot`; the bought item itself
    /// arrives as an object create.
    pub(crate) fn update_stock(&mut self, slot: u32, new_count: u32) {
        if let Some(it) = self.items.iter_mut().find(|it| it.slot == slot) {
            it.current_count = new_count;
        }
    }

    /// Zero every row of `entry`, as the reference's `ITEM_ALREADY_SOLD` arm writes 0 (`0x5dcdbf`)
    /// into each matching row of its 128-row cache (`0x5dcdcd`).
    pub(crate) fn sold_out(&mut self, entry: u32) {
        for it in self.items.iter_mut().filter(|it| it.entry == entry) {
            it.current_count = 0;
        }
    }

    /// A client-side close; a re-open re-lists.
    pub(crate) fn clear(&mut self) {
        self.vendor = None;
        self.items.clear();
    }

    /// The disconnect clear.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

/// Buy and sell refusals waiting for the feed's message line.
#[derive(Resource, Default)]
pub(crate) struct MerchantErrors(pub Vec<MerchantRefusal>);

/// A refused buy or sell and its wire reason code.
pub(crate) enum MerchantRefusal {
    Buy(u8),
    Sell(u8),
}

mod net;

pub(crate) struct UiMerchantPlugin;

impl Plugin for UiMerchantPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<MerchantOpen>()
            .init_resource::<MerchantErrors>()
            .add_systems(
                Update,
                (
                    // Range-close first so the clear fires `MERCHANT_CLOSED` the same frame. The
                    // feed follows `UnitFeed`: `MERCHANT_SHOW`'s paint reads the item and
                    // player-requirement stores for `GetMerchantItemInfo`'s `isUsable`.
                    close_npc_session_out_of_range::<MerchantOpen>.before(feed_merchant),
                    feed_merchant.after(crate::ui_unit::UnitFeed).in_set(UiFeed),
                    // Before `feed_char`: the `UNIT_INVENTORY_CHANGED` a repair fires refreshes the
                    // Repair All button from this total (`PaperDollFrame.lua:717-724`).
                    feed_repair_all_cost
                        .in_set(UiFeed)
                        .before(crate::ui_char::feed_char),
                    drain_merchant.after(UiInput),
                ),
            );
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
/// the item template answers.
fn resolve_item(
    item: &VendorItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> MerchantItem {
    let template = items.template(item.entry, 0, commands);
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
        max_stack: template.map(|t| t.stackable.max(1)),
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
/// the slot, and the price from the player's buyback price field.
fn resolve_buyback(
    idx: u8,
    store: &benilla_protocol::ObjectFields,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> MerchantItem {
    let guid = store.player_buyback_slot(idx).unwrap_or(0);
    let obj = objects.object(guid);
    let entry = obj.and_then(|o| o.object_entry()).unwrap_or(0);
    let quantity = obj.and_then(|o| o.item_stack_count()).unwrap_or(1).max(1);
    let template = items.template(entry, guid, commands);
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
/// ([`benilla_formats::DurabilityTables`]), after the price `discount` (`0x4faf8a`).
fn item_repair_cost(
    guid: u64,
    objects: &Objects,
    items: &Items,
    tables: &RepairTables,
    commands: &NetCommands,
    discount: f64,
) -> u32 {
    let Some(obj) = objects.object(guid) else {
        return 0;
    };
    let cur = obj.item_durability().unwrap_or(0);
    let max = obj.item_max_durability().unwrap_or(0);
    let entry = obj.object_entry().unwrap_or(0);
    if max == 0 || cur >= max || entry == 0 {
        return 0;
    }
    let points = max - cur;
    let Some(t) = items.template(entry, guid, commands) else {
        return 0;
    };
    let (level, quality, class, subclass) = (t.item_level, t.quality, t.class, t.subclass);
    tables
        .0
        .repair_cost(points, level, quality, class, subclass, discount)
}

/// The repair-all total over the reference's three sweeps (`0x4fbd60`): equipped slots 0-18, the
/// backpack and the four bags' contents; never the bank, keyring or buyback. It sums the
/// discounted, rounded per-item costs (`0x4fbe0b`); the total itself is never discounted.
fn repair_all_cost(
    store: &benilla_protocol::ObjectFields,
    objects: &Objects,
    items: &Items,
    tables: &RepairTables,
    commands: &NetCommands,
    discount: f64,
) -> u32 {
    let mut total: u64 = 0;
    let mut add = |guid: u64, items: &Items| {
        if guid != 0 {
            total += u64::from(item_repair_cost(
                guid, objects, items, tables, commands, discount,
            ));
        }
    };
    for i in 0..19u8 {
        add(store.player_inv_slot(i).unwrap_or(0), items);
    }
    for i in 0..16u8 {
        add(store.player_pack_slot(i).unwrap_or(0), items);
    }
    for bag in 19..23u8 {
        let bag_guid = store.player_inv_slot(bag).unwrap_or(0);
        if bag_guid == 0 {
            continue;
        }
        let slots: Vec<u64> = objects
            .object(bag_guid)
            .map(|b| {
                let n = b.container_num_slots().unwrap_or(0).min(36) as u8;
                (0..n).filter_map(|i| b.container_slot(i)).collect()
            })
            .unwrap_or_default();
        for guid in slots {
            add(guid, items);
        }
    }
    total.min(u64::from(u32::MAX)) as u32
}

/// The open vendor's unit, `None` with none open or its unit not streamed.
fn vendor_store<'a>(
    open: &MerchantOpen,
    units: &'a Query<(&Guid, &ObjectStore), Without<SelfPlayer>>,
) -> Option<&'a ObjectStore> {
    open.vendor
        .and_then(|g| units.iter().find(|(guid, _)| guid.0 == g))
        .map(|(_, store)| store)
}

/// The open vendor's `UNIT_NPC_FLAGS`, 0 with none open or its unit not streamed.
fn vendor_npc_flags(
    open: &MerchantOpen,
    units: &Query<(&Guid, &ObjectStore), Without<SelfPlayer>>,
) -> u32 {
    vendor_store(open, units).map_or(0, |store| store.0.unit_npc_flags())
}

/// Push `GetRepairAllCost`'s total, which the reference sweeps at the call (`0x4fbd60`): every
/// frame a repairing vendor is open, 0 otherwise.
fn feed_repair_all_cost(
    script: Option<NonSendMut<UiScript>>,
    open: Res<MerchantOpen>,
    objects: Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    units: Query<(&Guid, &ObjectStore), Without<SelfPlayer>>,
    reactions: crate::target::ReactionInputs,
    tables: Option<Res<RepairTables>>,
    mut last: Local<crate::ui_script::VmMemo<Option<u32>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let vendor =
        vendor_store(&open, &units).filter(|store| store.0.unit_npc_flags() & NPC_FLAG_REPAIR != 0);
    let total = match (vendor, self_q.iter().next(), tables.as_deref()) {
        (Some(vendor), Some(store), Some(t)) => {
            // The open merchant's discount for the player (`0x4faf8a`).
            let discount = crate::target::vendor_price_discount(
                reactions.factions.as_deref(),
                &reactions.reputations,
                vendor,
                store,
            );
            repair_all_cost(&store.0, &objects, &items, t, &commands, discount)
        }
        _ => 0,
    };
    if *last != Some(total) {
        script.set_repair_all_cost(total);
        *last = Some(total);
    }
}

fn snapshot(
    open: &MerchantOpen,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    player: Option<&benilla_protocol::ObjectFields>,
    vendor_npc_flags: u32,
) -> Option<MerchantState> {
    open.vendor?;
    let buyback = player
        .map(|store| {
            buyback_order(store)
                .into_iter()
                .map(|idx| resolve_buyback(idx, store, objects, items, icons, commands))
                .collect()
        })
        .unwrap_or_default();
    Some(MerchantState {
        items: open
            .items
            .iter()
            .map(|it| resolve_item(it, items, icons, commands))
            .collect(),
        buyback,
        can_repair: vendor_npc_flags & NPC_FLAG_REPAIR != 0,
    })
}

/// Push the merchant into the VM and fire its events on a change; show the refusals and push the
/// purse.
fn feed_merchant(
    script: Option<NonSendMut<UiScript>>,
    open: Res<MerchantOpen>,
    objects: Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    units: Query<(&Guid, &ObjectStore), Without<SelfPlayer>>,
    names: Res<NameCache>,
    mut errors: ResMut<MerchantErrors>,
    mut last: Local<crate::ui_script::VmMemo<Option<MerchantState>>>,
    mut last_money: Local<crate::ui_script::VmMemo<Option<u64>>>,
    mut last_name: Local<crate::ui_script::VmMemo<Option<String>>>,
    mut last_vendor: Local<crate::ui_script::VmMemo<Option<u64>>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_money = last_money.get(&script);
    let last_name = last_name.get(&script);
    let last_vendor = last_vendor.get(&script);
    // Refusals show, and speak, as their message rows say; a silent code has no key.
    let refusals: Vec<_> = errors
        .0
        .drain(..)
        .filter_map(|refusal| {
            let key = match refusal {
                MerchantRefusal::Buy(r) => buy_error_key(r)?,
                MerchantRefusal::Sell(r) => sell_error_key(r)?,
            };
            crate::ui_action::keyed_line(&script, key)
        })
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_merchant", refusals);
    // The purse (`PLAYER_FIELD_COINAGE`): on a change, push it and fire `PLAYER_MONEY`, which the
    // money frames repaint on.
    if let Some(store) = self_q.iter().next() {
        if let Some(copper) = store.0.player_money() {
            let copper = u64::from(copper);
            if *last_money != Some(copper) {
                script.set_money(copper);
                script.fire_event("PLAYER_MONEY", vec![]);
                *last_money = Some(copper);
            }
        }
    }

    // The vendor's service bits gate the repair pair; the player's descriptor carries buyback.
    let player = self_q.iter().next().map(|s| &s.0);
    let fresh = snapshot(
        &open,
        &objects,
        &items,
        icons.as_deref(),
        &commands,
        player,
        vendor_npc_flags(&open, &units),
    );
    // The vendor's name rides `MERCHANT_SHOW` and `MERCHANT_UPDATE` as arg1, and its landing alone
    // re-fires `MERCHANT_UPDATE`; the reference fires both bare (`0x4fad92`, `0x4facaa`), and
    // stock `MerchantFrame.lua:67` titles with `UnitName("NPC")`.
    let vendor_name = open
        .vendor
        .and_then(|g| names.resolve(g, &commands).map(str::to_string));
    let name_changed = *last_name != vendor_name;
    // A different vendor while open is a close then an open: `ShowUIPanel` returns early on a
    // visible frame, so only a hide replays the open sound.
    let switched = npc_switched(*last_vendor, open.vendor);
    if fresh == *last && !name_changed && !switched {
        return;
    }
    script.set_merchant(fresh.clone());
    let name_arg = || vec![ScriptValue::Str(vendor_name.clone().unwrap_or_default())];
    if switched {
        // `MERCHANT_CLOSED` queues a close intent through OnHide; consume it so the drain keeps
        // the new vendor.
        script.fire_event("MERCHANT_CLOSED", vec![]);
        script.fire_event("MERCHANT_SHOW", name_arg());
        let _ = script.take_merchant_close();
    } else {
        match (&*last, &fresh) {
            (None, Some(_)) => script.fire_event("MERCHANT_SHOW", name_arg()),
            // A change while open: an item or the name landed, or the stock moved.
            (Some(_), Some(_)) => script.fire_event("MERCHANT_UPDATE", name_arg()),
            (Some(_), None) => script.fire_event("MERCHANT_CLOSED", vec![]),
            (None, None) => {}
        }
    }
    *last = fresh;
    *last_name = vendor_name;
    *last_vendor = open.vendor;
}

/// The range guard closes the window, as `CloseMerchant` does, when the vendor is out of range or
/// gone.
impl NpcSession for MerchantOpen {
    fn npc(&self) -> Option<u64> {
        self.vendor
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Drain the Lua intents: a buy sends `CMSG_BUY_ITEM` by item entry, a buyback
/// `CMSG_BUYBACK_ITEM` with the absolute slot (69-80), a repair-all `CMSG_REPAIR_ITEM` with guid
/// 0, and a close clears locally, as there is no close packet.
fn drain_merchant(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<MerchantOpen>,
    commands: Res<NetCommands>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    objects: Objects,
) {
    let Some(mut script) = script else {
        return;
    };
    let self_pair = self_q.iter().next();
    let self_store = self_pair.map(|(store, _)| store);
    for index in script.take_merchant_buybacks() {
        let Some(vendor) = open.vendor else { continue };
        let Some(store) = self_store else {
            continue;
        };
        let order = buyback_order(&store.0);
        match usize::try_from(index)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| order.get(i))
        {
            Some(&idx) => {
                let slot = BUYBACK_SLOT_FIRST + u32::from(idx);
                debug!("ui_merchant: buyback list index {index} → absolute slot {slot}");
                let _ = commands.0.send(ClientCommand::BuybackItem { vendor, slot });
            }
            None => debug!("ui_merchant: BuybackItem({index}) out of range — ignored"),
        }
    }
    if script.take_repair_all() {
        if let Some(vendor) = open.vendor {
            debug!("ui_merchant: repair all");
            let _ = commands.0.send(ClientCommand::RepairItem {
                vendor,
                item_guid: 0,
            });
        }
    }
    for (index, quantity) in script.take_merchant_buys() {
        let Some(vendor) = open.vendor else { continue };
        match open.entry_at(index) {
            Some(entry) => {
                // Stack count is a u8 on the wire (`CMSG_BUY_ITEM`); clamp the Lua quantity.
                let count = quantity.clamp(1, u32::from(u8::MAX)) as u8;
                debug!("ui_merchant: buy row {index} (entry {entry} ×{count})");
                let _ = commands.0.send(ClientCommand::BuyItem {
                    vendor,
                    entry,
                    count,
                });
            }
            None => debug!("ui_merchant: BuyMerchantItem({index}) out of range — ignored"),
        }
    }
    // `PickupMerchantItem`'s sell arm, a bag item dropped on the window, sold by its item guid so
    // an emptied slot is a no-op. It runs before the close below, as `0x4fb760`'s sell arm runs
    // before its vendor check (`0x4fb7f7`), so a drop on a closing window still sells.
    for (bag, slot) in script.take_merchant_cursor_sells() {
        let Some(vendor) = open.vendor else { continue };
        let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
        match self_store.and_then(|s| slot_guid(&s.0, bag, slot0, &objects)) {
            Some(item_guid) => {
                debug!("ui_merchant: cursor sell bag {bag} slot {slot} (item {item_guid:#x})");
                let _ = commands.0.send(ClientCommand::SellItem {
                    vendor,
                    item_guid,
                    count: 0,
                });
            }
            None => debug!("ui_merchant: cursor sell on an empty slot ({bag}, {slot}) — ignored"),
        }
    }
    // A held vendor row dropped into a slot: `CMSG_BUY_ITEM_IN_SLOT` with count 1, the
    // reference's fixed count on all three drop paths.
    for (bag, slot, entry) in script.take_merchant_slot_buys() {
        let Some(vendor) = open.vendor else { continue };
        let (Some(store), Some((bag_index, bag_slot))) = (self_store, wire_pos(bag, slot)) else {
            debug!("ui_merchant: slot buy to an unaddressable slot ({bag}, {slot}) — ignored");
            continue;
        };
        // The wire wants the container's guid: the player's for a slot in the player's own
        // array (`BAG_PLAYER_INVENTORY`), the bag's for a real bag.
        let bag_guid = if bag_index == BAG_PLAYER_INVENTORY {
            self_pair.map(|(_, g)| g.0)
        } else {
            store.0.player_inv_slot(bag_index).filter(|g| *g != 0)
        };
        match bag_guid {
            Some(bag_guid) => {
                debug!(
                    "ui_merchant: slot buy entry {entry} → bag {bag_guid:#x} slot {bag_slot} \
                     (lua {bag}, {slot})"
                );
                let _ = commands.0.send(ClientCommand::BuyItemInSlot {
                    vendor,
                    entry,
                    bag_guid,
                    bag_slot,
                    count: 1,
                });
            }
            None => debug!("ui_merchant: slot buy into an absent bag {bag} — ignored"),
        }
    }
    if script.take_merchant_close() {
        debug!("ui_merchant: client-side close (no packet)");
        open.clear();
    }
}

/// A right mouse-down in the world ends repair mode: the WorldFrame hook `0x492c20` resets the
/// Repair base mode to Point (`0x492c68`) and consumes nothing. A held payload pre-empts the hook
/// (`0x492b50`), and a press over a UI frame never reaches it.
pub(crate) fn end_repair_mode_on_right_press(
    mut presses: MessageReader<WorldRightPress>,
    payload_held: Res<crate::ui_script::CursorPayloadHeld>,
    script: Option<NonSendMut<UiScript>>,
) {
    if presses.read().last().is_none() || payload_held.0 {
        return;
    }
    if let Some(mut script) = script {
        script.end_repair_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repair's `UNIT_INVENTORY_CHANGED` reads this frame's total: the stock handler refreshes
    /// the Repair All button from `GetRepairAllCost` (`PaperDollFrame.lua:717-724`).
    #[test]
    fn the_repair_all_total_is_swept_before_the_inventory_event() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        assert!(crate::test_support::runs_before(
            &mut app,
            feed_repair_all_cost,
            crate::ui_char::feed_char
        ));
    }

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
        // The sword in the main hand (slot 15); the current honor rank byte 8.
        let main_hand = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (BYTES_0, crate::target::HUMAN_WARRIOR),
                (PLAYER_BYTES_3, 8 << 24),
                (main_hand, SWORD as u32),
                (main_hand + 1, (SWORD >> 32) as u32),
            ])),
        ));
        app.world_mut().spawn((
            Guid(VENDOR),
            ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_UNIT_NPC_FLAGS, NPC_FLAG_REPAIR),
                (FIELD_UNIT_FACTIONTEMPLATE, template),
                (FIELD_UNIT_FLAGS, 0x1000),
            ])),
        ));
        let mut script = UiScript::new().unwrap();
        script.set_merchant(Some(MerchantState {
            can_repair: true,
            ..Default::default()
        }));
        app.insert_non_send_resource(script);

        app.world_mut()
            .run_system_once(feed_repair_all_cost)
            .unwrap();
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let got = script.eval::<i64>("return (GetRepairAllCost())").unwrap();
        assert_eq!(got, i64::from(want), "the undiscounted total is {full}");
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
    fn entry_at_maps_the_one_based_row() {
        let mut open = MerchantOpen::default();
        assert!(!open.is_open());
        open.open(0x42, vec![row(159, 1, STOCK_UNLIMITED), row(4540, 2, 5)]);
        assert!(open.is_open());
        assert_eq!(open.entry_at(1), Some(159));
        assert_eq!(open.entry_at(2), Some(4540));
        assert_eq!(open.entry_at(3), None); // out of range
        assert_eq!(open.entry_at(0), None); // 0 has no row (1-based)
    }

    #[test]
    fn resolve_maps_unlimited_stock_to_minus_one() {
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        // Unlimited stock is `numAvailable` -1; a finite count passes through.
        let unlimited = resolve_item(&row(159, 1, STOCK_UNLIMITED), &items, None, &commands);
        assert_eq!(unlimited.num_available, -1);
        assert_eq!(unlimited.item_id, 159);
        let finite = resolve_item(&row(4540, 2, 5), &items, None, &commands);
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
        let resolved = resolve_item(&row(2129, 1, 3), &items, None, &commands);
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
        open.update_stock(2, 4);
        assert_eq!(open.items[1].current_count, 4);
        open.update_stock(99, 0); // no such slot: a no-op
        assert_eq!(open.items[0].current_count, STOCK_UNLIMITED);
    }

    #[test]
    fn clear_closes_the_window() {
        let mut open = MerchantOpen::default();
        open.open(0x42, vec![row(159, 1, 3)]);
        open.clear();
        assert!(!open.is_open());
        assert!(open.items.is_empty());
    }
}
