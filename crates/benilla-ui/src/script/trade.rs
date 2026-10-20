//! The trade bindings: the app pushes both sides of the trade ([`UiScript::set_trade`]), and the
//! trade verbs queue intents it drains. Slots are 1-based, 1..=7, the seventh the non-traded
//! enchant slot; an empty or out-of-range slot answers nil. The partner rides the `"npc"` unit,
//! which the app points at them. The getters test no open state: both offers stay as the last
//! trade left them until the next open clears them ([`UiScript::open_trade_window`]).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{self, flag};
use super::cursor::{self, CursorPayload};
use super::Model;

/// Slots per side, six traded and the non-traded enchant slot: vmangos `TRADE_SLOT_COUNT`
/// (`TradeData.h`), kept equal to the protocol crate's, which this crate does not depend on.
pub const TRADE_SLOTS: usize = 7;

/// One filled trade slot, resolved by the app; an empty slot is `None`, never a zeroed `Some`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSlotItem {
    /// The item entry, the key for `isUsable` and the tooltip.
    pub item_id: u32,
    /// `None` while the item template is in flight.
    pub name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    /// `None` while the template is in flight. `GetTradePlayerItemInfo` and
    /// `GetTradeTargetItemInfo` report it only when `inventory_type` is nonzero (`0x4c059c`,
    /// `0x4c0241`), else -1.
    pub quality: Option<u32>,
    /// The template's InventoryType; 0 while the template is in flight and for an item that cannot
    /// be equipped.
    pub inventory_type: u32,
    /// The wire's `lock_id`: the trade target's tooltip draws LOCKED off it (`0x534563`,
    /// `0x52bdd2`). 0 for none; the player's side ignores it.
    pub lock_id: u32,
    /// The wire's wrapped word: the partner's block `+0x42c` (`0x534559`), which the tooltip's
    /// wrapped gates read (`0x52c991`, `0x52cc5b`). The player's side reads its object's
    /// `ITEM_FIELD_FLAGS` instead, so this only mirrors them there.
    pub wrapped: bool,
    /// The resolved name of the giver of a wrapped gift (`ITEM_FIELD_GIFTCREATOR` on the player's
    /// side, the block's `+0x418` on the partner's, `0x534574`); `None` while unresolved or when
    /// the guid is 0, which prints no `ITEM_WRAPPED_BY` line.
    pub gift_creator: Option<String>,
    /// The giver's guid, 0 for none: with no [`Self::gift_creator`] yet, the name the tooltip
    /// waits for.
    pub gift_creator_guid: u64,
    /// The resolved name of the item's creator (`ITEM_FIELD_CREATOR` on the player's side, the
    /// block's `+0x410` on the partner's, `0x53453e`), the "Made by" line; `None` while unresolved
    /// or when the guid is 0.
    pub creator: Option<String>,
    /// The creator's guid, 0 for none: with no [`Self::creator`] yet, the name the tooltip waits
    /// for.
    pub creator_guid: u64,
    /// Slot 7 only: the name of the spell being cast on the item, which stock shows in green
    /// (`TradeFrame.lua:62`) and the slot's tooltip proposes (`0x534397`, `0x53459a`); `None` on
    /// every other slot and while nothing is cast.
    pub enchantment: Option<String>,
    /// The partner's side only: the wire's charges and `(current, max)` durability, the block's
    /// `+0x428` (`0x534595`) and `+0x438`/`+0x434` (`0x5345c0`, `0x5345b4`).
    pub charges: i32,
    pub durability: (u32, u32),
    /// The escaped item link the link verbs answer; `None` while the template is in flight, since
    /// it embeds the name and the quality.
    pub link: Option<String>,
}

/// One side's offer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSideState {
    /// Index 0 is trade slot 1, index 6 the enchant slot.
    pub slots: [Option<TradeSlotItem>; TRADE_SLOTS],
    /// In copper.
    pub gold: u32,
}

/// The open trade, pushed whole by the app. The accept highlight is not here: it rides the
/// `TRADE_ACCEPT_UPDATE` event (`TradeFrame.lua:35`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeState {
    /// Our offer.
    pub player: TradeSideState,
    /// The partner's offer, the wire's `their_window` snapshot.
    pub target: TradeSideState,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the trade's two offers.
    pub fn set_trade(&mut self, state: Option<TradeState>) {
        self.model_mut().trade = state;
    }

    /// The open of the window (`0x4bf4e0` with a partner): the partner cell is set (`0x4bf549`) and
    /// both offers and our item table clear (`0x4bf55d`-`0x4bf5ef`), so the getters answer nil
    /// until the statuses fill them. Only an open clears them, and the world entry's reset
    /// (`0x4909b5` → `0x4bf0e0`, `0x4bf174`-`0x4bf20e`), which clears the tables and the cell too.
    pub fn open_trade_window(&mut self) {
        let mut model = self.model_mut();
        model.trade = None;
        model.trade_offered = Default::default();
        model.trade_window = true;
    }

    /// Both money offers zero (`0xc4be70`, `0xc4be74`), as the cancel, complete, close-window and
    /// code 13 statuses do before their dispatch (`0x5d4884`-`0x5d48a2`, `0x5d48eb`-`0x5d48ff`).
    pub fn zero_trade_gold(&mut self) {
        if let Some(trade) = self.model_mut().trade.as_mut() {
            trade.player.gold = 0;
            trade.target.gold = 0;
        }
    }

    /// The close core's cell clear (`0x4bf516`): the window is shut. Both offers and our item
    /// table stay (`0x4bfcf0` only unlocks), so the getters still answer the last offer, in
    /// `TRADE_CLOSED`'s handlers too (`0x4bf522`).
    pub fn close_trade_window(&mut self) {
        self.model_mut().trade_window = false;
    }

    /// Our column as the VM holds it, `ClickTradeButton`'s writes included: what the slots'
    /// `TRADE_PLAYER_ITEM_CHANGED` handlers last read. `None` before any trade has opened.
    pub fn trade_player_slots(&self) -> Option<[Option<TradeSlotItem>; TRADE_SLOTS]> {
        self.model_ref()
            .trade
            .as_ref()
            .map(|t| t.player.slots.clone())
    }

    /// `UnlockTradeItems` (`0x4bfcf0`), which the `CANCELED`, `COMPLETE` and `CLOSE_WINDOW` status
    /// arms run before the window closes (`0x4bf837`, `0x4bf859`, `0x4bf87e`): every item our
    /// slots held unlocks, one `ITEM_LOCK_CHANGED` each (`0x4bfd03`), fired now. The table keeps
    /// its guids (`0x4bfd08`-`0x4bfd11` only walk it). A local `CloseTrade` (`0x4bfdc0`) never
    /// runs it; the server's `CANCELED` does.
    pub fn unlock_trade_items(&mut self) {
        let offered: Vec<u64> = self
            .model_ref()
            .trade_offered
            .iter()
            .flatten()
            .map(|item| item.guid)
            .collect();
        for guid in offered {
            self.unlock_item(guid);
        }
    }

    /// `InvalidateTradeSlot` (`0x4bfbd0`) for our 0-based slot, from the `ONLY_CONJURED` arm
    /// (`0x4bf9fd`) and an own-side `SMSG_TRADE_STATUS_EXTENDED` that empties it (`0x5d4ad3`,
    /// `0x5d4b33`): the item it held unlocks (`0x4bfc0a`), the slot empties, and
    /// `TRADE_PLAYER_ITEM_CHANGED(slot)` fires (`0x4bfc3f`), now.
    pub fn invalidate_trade_slot(&mut self, slot: u8) {
        let cell = usize::from(slot);
        if cell >= TRADE_SLOTS {
            return;
        }
        let item = self.model_mut().trade_offered[cell].take();
        if let Some(item) = item {
            self.unlock_item(item.guid);
        }
        self.fire_event(
            "TRADE_PLAYER_ITEM_CHANGED",
            vec![super::ScriptValue::Int(i64::from(slot) + 1)],
        );
    }

    /// The scans `0x4bfb10` and `0x4bfb60`: the first of our seven slots whose stored item is
    /// `guid`, with the bag position it was placed from, `(slot, bag, bag_slot)`, 0-based slot;
    /// none for a zero guid (`0x4bfb1e`). The table outlives a close, as the reference's does
    /// (`0xb716d0`): it asks nothing of the window.
    pub fn trade_slot_holding(&self, guid: u64) -> Option<(u8, i64, u32)> {
        if guid == 0 {
            return None;
        }
        let model = self.model_ref();
        model
            .trade_offered
            .iter()
            .enumerate()
            .find_map(|(i, cell)| {
                cell.as_ref()
                    .filter(|item| item.guid == guid)
                    .map(|item| (i as u8, item.bag, item.slot))
            })
    }

    /// `0x4bfa70` with a zero item, which `0x4bfb10` runs once the slot's item is gone: the slot
    /// empties, with no unlock (the item and its lock went together), and
    /// `TRADE_PLAYER_ITEM_CHANGED(slot)` fires (`0x4bfaef`), now. The app has sent
    /// `CMSG_CLEAR_TRADE_ITEM`.
    pub fn empty_trade_slot(&mut self, slot: u8) {
        let cell = usize::from(slot);
        if cell >= TRADE_SLOTS {
            return;
        }
        {
            let mut model = self.model_mut();
            model.trade_offered[cell] = None;
            if let Some(trade) = model.trade.as_mut() {
                trade.player.slots[cell] = None;
            }
        }
        self.fire_event(
            "TRADE_PLAYER_ITEM_CHANGED",
            vec![super::ScriptValue::Int(i64::from(slot) + 1)],
        );
    }

    /// `0x4bfa70` placing the same item again, which `0x4bfb60` runs when the slot's item
    /// restacks or takes an enchantment: the slot shows `row`, the item as the getter reads it
    /// live (`0x4c05d1` reads the object), and `TRADE_PLAYER_ITEM_CHANGED(slot)` fires
    /// (`0x4bfaef`), now. The app has sent `CMSG_SET_TRADE_ITEM`.
    pub fn replace_trade_slot(&mut self, slot: u8, row: TradeSlotItem) {
        let cell = usize::from(slot);
        if cell >= TRADE_SLOTS {
            return;
        }
        if let Some(trade) = self.model_mut().trade.as_mut() {
            trade.player.slots[cell] = Some(row);
        }
        self.fire_event(
            "TRADE_PLAYER_ITEM_CHANGED",
            vec![super::ScriptValue::Int(i64::from(slot) + 1)],
        );
    }

    /// A placement the app could not send (its bag position has no wire form): the slot gives
    /// the item back, unlocked, as the reference's failed send never writes it (`0x4bfab1`), and
    /// the slot's handler repaints with `TRADE_PLAYER_ITEM_CHANGED`.
    pub fn return_unsent_trade_item(&mut self, id: u32) {
        let Some(i) = id
            .checked_sub(1)
            .map(|i| i as usize)
            .filter(|&i| i < TRADE_SLOTS)
        else {
            return;
        };
        super::tick::verb(&self.lua, |model| {
            let Some(item) = model.trade_offered[i].take() else {
                return;
            };
            if let Some(trade) = model.trade.as_mut() {
                trade.player.slots[i] = None;
            }
            super::cursor::unlock_item(model, item.guid);
            model.raise(
                "TRADE_PLAYER_ITEM_CHANGED",
                vec![super::ScriptValue::Int(i64::from(id))],
            );
        });
    }

    /// The entries the trade getters asked the item cache for since the last drain, one per call
    /// that found a filled slot with no template: the host registers the callback `0x4c0340` on
    /// each that is still unanswered.
    pub fn take_trade_record_asks(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().trade_record_asks)
    }

    /// Drain the tokens `InitiateTrade` queued; the app sends `CMSG_INITIATE_TRADE` for each.
    pub fn take_trade_initiates(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().trade_initiates)
    }

    /// Whether `AcceptTrade` was called since the last drain: `CMSG_ACCEPT_TRADE`.
    pub fn take_trade_accept(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_accept)
    }

    /// Whether `CancelTradeAccept` was called since the last drain: `CMSG_UNACCEPT_TRADE`.
    pub fn take_trade_unaccept(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_unaccept)
    }

    /// Whether `CloseTrade`, the window's OnHide verb, was called since the last drain; the app
    /// sends `CMSG_CANCEL_TRADE` and ends its session.
    pub fn take_trade_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_close)
    }

    /// Whether `BeginTrade()` was called since the last drain: an empty `CMSG_BEGIN_TRADE`.
    pub fn take_trade_begin(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_begin)
    }

    /// Whether `CancelTrade()` was called since the last drain: the bare `CMSG_CANCEL_TRADE`.
    pub fn take_trade_cancel(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_cancel)
    }

    /// The copper `SetTradeMoney` last offered since the last drain: `CMSG_SET_TRADE_GOLD`.
    pub fn take_trade_money(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().trade_set_money)
    }

    /// The `(trade_id, bag, slot)` placements `ClickTradeButton` queued, for `CMSG_SET_TRADE_ITEM`:
    /// `trade_id` 1..=7, `bag` and `slot` in the cursor's space.
    pub fn take_trade_set_items(&mut self) -> Vec<(u32, i64, u32)> {
        std::mem::take(&mut self.model_mut().trade_set_items)
    }

    /// The 1-based slots `ClickTradeButton` queued to clear, for `CMSG_CLEAR_TRADE_ITEM`.
    pub fn take_trade_clear_items(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().trade_clear_items)
    }
}

impl Model {
    /// `0x4bfd20`: the window is open (the partner cell, tested first at `0x4bfd23`-`0x4bfd2f`)
    /// and one of our slots holds `guid`. The equip refusal (`0x5e1897`) and the stackable
    /// arrival's unlock (`0x622229`) ask it.
    pub(crate) fn trade_holds(&self, guid: u64) -> bool {
        self.trade_window
            && self
                .trade_offered
                .iter()
                .flatten()
                .any(|item| item.guid == guid)
    }
}

/// What a getter's item-cache lookup leaves behind when the slot is filled and its template has
/// not answered: a request carrying the callback `0x4c0340` (`0x4c017f`, `0x4c0582`), one per
/// call. The host registers it and signals `TRADE_UPDATE` when the record lands.
fn ask_record(model: &mut Model, slot: Option<&TradeSlotItem>) {
    if let Some(slot) = slot.filter(|s| s.name.is_none()) {
        model.trade_record_asks.push(slot.item_id);
    }
}

/// The slot at a 1-based index on the side `pick` selects.
fn slot_at(
    model: &Model,
    index: usize,
    pick: impl Fn(&TradeState) -> &TradeSideState,
) -> Option<TradeSlotItem> {
    model
        .trade
        .as_ref()
        .map(pick)
        .and_then(|side| index.checked_sub(1).and_then(|n| side.slots.get(n)))
        .cloned()
        .flatten()
}

fn gold(model: &Model, pick: impl Fn(&TradeState) -> &TradeSideState) -> u32 {
    model.trade.as_ref().map(pick).map_or(0, |side| side.gold)
}

/// `ClickTradeButton(id)` on our slot (`TradeFrame.xml:135`, `0x4bfdf0`). Coins on the cursor go
/// into the offer, the index unread (`0x4bfe34`). Otherwise the held item, or none, is compared
/// with the slot's (`0x4c0017`): the same, nothing on both sides included, is `ClearCursor(1,1)`
/// (`0x4c0026`), which drops any other payload. Else the slot takes the held item, a
/// `CMSG_SET_TRADE_ITEM`, or empties, a `CMSG_CLEAR_TRADE_ITEM` (`0x4bfa70`), and
/// `SetCursorItem` puts the slot's old item back on the cursor, whole (count 0, `0x4c004e`),
/// lifting no lock when the send went (`0x4c004a`-`0x4c0064`), so a placed item stays locked in
/// its bag. `0x4bfa70` stores the slot and signals `TRADE_PLAYER_ITEM_CHANGED` with it
/// (`0x4bfaef`) before that `SetCursorItem`, so the handler reads the slot as written. The VM
/// fills our column itself, since vmangos echoes a placement only to the partner
/// (`TradeData.cpp:81`).
fn click_trade_button(lua: &Lua, id: i32) {
    let money = super::tick::verb(lua, |model| {
        if !matches!(model.cursor, Some(CursorPayload::Money(_))) {
            return false;
        }
        cursor::money::add_trade_money(model);
        true
    });
    if money {
        return;
    }
    // A held item a slot below 7 refuses shows its error and nothing else happens: no send, no
    // signal, the cursor kept. The id less one is compared signed (`0x4bfe7f`, `0x4bfea1`), so a
    // zero or negative id is checked too.
    if i64::from(id) - 1 < 6 {
        let refusal = {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            match &model.cursor {
                Some(CursorPayload::Item(item)) => trade_refusal(&model, item),
                _ => None,
            }
        };
        if let Some(key) = refusal {
            super::tick::verb(lua, |model| model.push_ui_error(key));
            return;
        }
    }
    // A slot outside the seven reads as empty (`0x4bffc5`-`0x4c0001`).
    let index = usize::try_from(i64::from(id) - 1)
        .ok()
        .filter(|&i| i < TRADE_SLOTS);
    let (held, old) = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let held = match &model.cursor {
            Some(CursorPayload::Item(item)) => Some(item.clone()),
            _ => None,
        };
        (held, index.and_then(|i| model.trade_offered[i].clone()))
    };
    let key = |item: &Option<cursor::CursorItem>| item.as_ref().map(|i| (i.bag, i.slot));
    if key(&held) == key(&old) {
        super::tick::verb(lua, cursor::clear_cursor);
        return;
    }
    // `0x4bfa70`: a slot outside the seven sends nothing; the set send fails only on a missing
    // player or item (`0x5d43af`, `0x5d43da`), which a held item never is here. The table keeps
    // the item whole: what comes back is the stack.
    let sent = super::tick::verb(lua, |model| {
        let Some(i) = index else {
            return false;
        };
        let placed = held.as_ref().map(|item| trade_slot_item(model, item));
        match &held {
            Some(item) => model
                .trade_set_items
                .push((i as u32 + 1, item.bag, item.slot)),
            None => model.trade_clear_items.push(i as u32 + 1),
        }
        model.trade_offered[i] = held.map(|item| cursor::CursorItem {
            count: None,
            ..item
        });
        write_player_slot(model, i, placed);
        true
    });
    cursor::set_cursor(lua, !sent, |_| old.map(CursorPayload::Item));
}

/// The error `ClickTradeButton` refuses a held item for slots 1 to 6 with (`0x4bfeb5`-`0x4bffae`),
/// read off the item's live record as the reference reads its object; none when the record is
/// gone, as an item the cursor's guid no longer finds is let through (`0x4bfead`). In order: a
/// bound item (`0x5da2c0`) is `ERR_TRADE_QUEST_ITEM` for quest bonding 4 or 5 and
/// `ERR_TRADE_BOUND_ITEM` otherwise, a missing record included (`0x4bfee7`); a container with an
/// item in any seat is `ERR_TRADE_BAG` (`0x4bff50`-`0x4bff62`); an empty one worn in a bag slot,
/// player slots 19-22, is `ERR_TRADE_EQUIPPED_BAG` (`0x6225d0`, `0x4bffa4`-`0x4bffae`).
fn trade_refusal(model: &Model, item: &cursor::CursorItem) -> Option<&'static str> {
    let (bound, bonding, seats) = if item.bag == cursor::EQUIPMENT_BAG {
        let s = model
            .inv_slot("player", item.slot as usize)
            .filter(|s| s.guid == item.guid)?;
        (s.already_bound, s.bonding, s.occupied_seats)
    } else {
        let s = model
            .containers
            .get(&item.bag)?
            .slots
            .get(&item.slot)
            .filter(|s| s.guid == item.guid)?;
        (s.already_bound, s.bonding, s.occupied_seats)
    };
    /// `BIND_QUEST_ITEM` and `BIND_QUEST_ITEM1`.
    const QUEST_BONDING: [u32; 2] = [4, 5];
    if bound {
        return Some(if QUEST_BONDING.contains(&bonding) {
            "ERR_TRADE_QUEST_ITEM"
        } else {
            "ERR_TRADE_BOUND_ITEM"
        });
    }
    let seats = seats?;
    if seats > 0 {
        return Some("ERR_TRADE_BAG");
    }
    // The paper doll's bag slots, 1-based 20 to 23.
    (item.bag == cursor::EQUIPMENT_BAG && (20..=23).contains(&item.slot))
        .then_some("ERR_TRADE_EQUIPPED_BAG")
}

/// `0x4bfa70`'s local half: our 0-based slot shows `item` and signals
/// `TRADE_PLAYER_ITEM_CHANGED` with its 1-based index (`0x4bfaef`). The reference writes only
/// past a successful send (`0x4bfab1`), which needs an open trade.
fn write_player_slot(model: &mut Model, i: usize, item: Option<TradeSlotItem>) {
    let Some(trade) = model.trade.as_mut() else {
        return;
    };
    trade.player.slots[i] = item;
    model.raise(
        "TRADE_PLAYER_ITEM_CHANGED",
        vec![super::ScriptValue::Int(i as i64 + 1)],
    );
}

/// What our slot shows for a held item: the bag record the app fed, else what the cursor carries.
fn trade_slot_item(model: &Model, item: &cursor::CursorItem) -> TradeSlotItem {
    let name = |link: Option<&str>| link.map(|l| cursor::item_link_name(Some(l)));
    model
        .containers
        .get(&item.bag)
        .and_then(|c| c.slots.get(&item.slot))
        .filter(|s| s.item_id == item.item_id)
        .map(|s| TradeSlotItem {
            item_id: s.item_id,
            name: name(s.link.as_deref()),
            texture: s.texture.clone(),
            count: s.count.max(1),
            quality: s.quality,
            inventory_type: s.inventory_type,
            lock_id: 0,
            wrapped: s.flags & 0x8 != 0,
            gift_creator: s.gift_creator.clone(),
            gift_creator_guid: s.gift_creator_guid,
            creator: s.creator.clone(),
            creator_guid: s.creator_guid,
            enchantment: None,
            charges: 0,
            durability: (0, 0),
            link: s.link.clone(),
        })
        .unwrap_or_else(|| TradeSlotItem {
            item_id: item.item_id,
            name: name(item.link.as_deref()),
            texture: item.texture.clone(),
            count: cursor::held_count(model, item),
            quality: item.quality,
            link: item.link.clone(),
            ..Default::default()
        })
}

/// Register the trade globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetTradePlayerItemInfo(id): name, texture, numItems, quality, enchantment; an empty slot is
    // nil, nil, 0, 0, nil (`0x4c0613`), and a filled one's texture is a string. The fourth is the
    // quality, -1 unless the template is cached
    // and equippable (`0x4c059c`), never a usable flag; `TradeFrame.lua:55` names it `isUsable` and
    // never reads it.
    g.set(
        "GetTradePlayerItemInfo",
        lua.create_function(|lua, id: Value| {
            // `is-number` else `Usage: GetTradePlayerItemInfo(index)` (`0x847c44`, `0x4c047e`).
            let id = binding_abi::index_arg(lua, id, "Usage: GetTradePlayerItemInfo(index)")?;
            let slot = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let slot = slot_at(&model, id, |t| &t.player);
                ask_record(&mut model, slot.as_ref());
                slot
            };
            let Some(s) = slot else {
                return Ok(super::binding_abi::tuple(&[
                    None,
                    None,
                    Some(0),
                    Some(0),
                    None,
                ]));
            };
            let opt_str = |v: &Option<String>| -> mlua::Result<Value> {
                Ok(match v {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(&s.name)?,
                // The question mark while the template is unknown (`0x4c0529`).
                Value::String(
                    lua.create_string(
                        s.texture
                            .as_deref()
                            .unwrap_or(super::item_stats::MISSING_ITEM_ICON),
                    )?,
                ),
                Value::Integer(i64::from(s.count.max(1))),
                Value::Integer(super::item_stats::gated_quality(
                    s.quality,
                    s.inventory_type,
                )),
                opt_str(&s.enchantment)?,
            ]))
        })?,
    )?;

    // The link verbs, for a slot's ctrl and shift clicks (`TradeFrame.xml:129`, `:95`).
    // Each `is-number` else its own usage line (`0x847c6c` at `0x4c0661`, `0x847c1c` at
    // `0x4c0391`).
    for (name, side, usage) in [
        (
            "GetTradePlayerItemLink",
            true,
            "Usage: GetTradePlayerItemLink(index)",
        ),
        (
            "GetTradeTargetItemLink",
            false,
            "Usage: GetTradeTargetItemLink(index)",
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, id: Value| {
                let id = binding_abi::index_arg(lua, id, usage)?;
                let link = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    let slot = if side {
                        slot_at(&model, id, |t| &t.player)
                    } else {
                        slot_at(&model, id, |t| &t.target)
                    };
                    slot.and_then(|s| s.link.clone())
                };
                match link {
                    Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
    }

    // GetTradeTargetItemInfo(id): the six values `TradeFrame.lua:84` reads; an empty slot is
    // nil, nil, 0, 0, nil, nil (`0x4c02f6`). The fourth is the quality through the same gate as
    // the player's (`0x4c0241`).
    g.set(
        "GetTradeTargetItemInfo",
        lua.create_function(|lua, id: Value| {
            // `is-number` else `Usage: GetTradeTargetItemInfo(index)` (`0x847bf4`, `0x4c011e`).
            let id = binding_abi::index_arg(lua, id, "Usage: GetTradeTargetItemInfo(index)")?;
            let (slot, usable) = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let slot = slot_at(&model, id, |t| &t.target);
                ask_record(&mut model, slot.as_ref());
                let usable = slot
                    .as_ref()
                    .is_none_or(|s| super::item_stats::item_usable_by_id(&model, s.item_id));
                (slot, usable)
            };
            // A template the cache lacks takes the miss too (`0x4c019b`, `0x4c02f6`).
            let Some(s) = slot.filter(|s| s.name.is_some()) else {
                return Ok(super::binding_abi::tuple(&[
                    None,
                    None,
                    Some(0),
                    Some(0),
                    None,
                    None,
                ]));
            };
            let opt_str = |v: &Option<String>| -> mlua::Result<Value> {
                Ok(match v {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(&s.name)?,
                Value::String(
                    lua.create_string(
                        s.texture
                            .as_deref()
                            .unwrap_or(super::item_stats::MISSING_ITEM_ICON),
                    )?,
                ),
                Value::Integer(i64::from(s.count.max(1))),
                Value::Integer(super::item_stats::gated_quality(
                    s.quality,
                    s.inventory_type,
                )),
                flag(usable),
                opt_str(&s.enchantment)?,
            ]))
        })?,
    )?;

    g.set(
        "GetPlayerTradeMoney",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // Gated on the partner cell (`0x4c0706`-`0x4c0714`); the target's is not (`0x4c0759`).
            Ok(if model.trade_window {
                i64::from(gold(&model, |t| &t.player))
            } else {
                0
            })
        })?,
    )?;
    g.set(
        "GetTargetTradeMoney",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(gold(&model, |t| &t.target)))
        })?,
    )?;

    // InitiateTrade(unit): the unit menu's TRADE row.
    g.set(
        "InitiateTrade",
        lua.create_function(|lua, unit: String| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_initiates
                .push(unit);
            Ok(())
        })?,
    )?;

    g.set(
        "AcceptTrade",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_accept = true;
            Ok(())
        })?,
    )?;
    g.set(
        "CancelTradeAccept",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_unaccept = true;
            Ok(())
        })?,
    )?;
    g.set(
        "CloseTrade",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_close = true;
            Ok(())
        })?,
    )?;
    // BeginTrade() and CancelTrade() (`0x48aa60`, `0x48aa70`): no arguments, no returns, no gate,
    // and an empty `0x117` or `0x11C`. They are the `TRADE` popup's Yes and No
    // (`StaticPopup.lua:517-522`), which never shows in 1.12 as nothing signals `TRADE_REQUEST`.
    g.set(
        "BeginTrade",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_begin = true;
            Ok(())
        })?,
    )?;
    g.set(
        "CancelTrade",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_cancel = true;
            Ok(())
        })?,
    )?;

    // SetTradeMoney(copper): the money input's callback (`TradeFrame.lua:173`).
    g.set(
        "SetTradeMoney",
        lua.create_function(|lua, copper: Value| {
            // `0x4c0820`: a non-number raises; the low dword is the whole offer, sent only when the
            // purse covers it, and a refusal is silent.
            let n = crate::script::binding_abi::number_arg(
                lua,
                copper,
                "Usage: SetTradeMoney(amount)",
            )? as u32;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if u64::from(n) > model.money {
                return Ok(());
            }
            model.trade_set_money.push(n);
            Ok(())
        })?,
    )?;

    g.set(
        "ClickTradeButton",
        // A non-number raises the usage before anything else (`0x4bfe12`, `0x847bac`); the
        // number is truncated by `_ftol` (`0x40a2b0`).
        lua.create_function(|lua, v: Value| {
            let id = super::binding_abi::number_arg(lua, v, "Usage: ClickTradeButton(index)")?;
            click_trade_button(lua, id);
            Ok(())
        })?,
    )?;

    // ClickTargetTradeButton(id) (`TradeFrame.xml:101`): the partner's column takes nothing.
    g.set(
        "ClickTargetTradeButton",
        lua.create_function(|lua, _id: Value| {
            // `is-number` else `Usage: ClickTargetTradeButton(index)` (`0x847bcc`, `0x4c0091`).
            let _id = binding_abi::index_arg32(lua, _id, "Usage: ClickTargetTradeButton(index)")?;
            // Coins on the cursor still go into our offer, the index unread (`0x4c00a3`).
            super::tick::verb(lua, cursor::money::add_trade_money);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn item(item_id: u32, count: u32, quality: u32) -> TradeSlotItem {
        TradeSlotItem {
            item_id,
            name: Some("Linen Cloth".into()),
            texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
            count,
            quality: Some(quality),
            inventory_type: 0,
            lock_id: 0,
            wrapped: false,
            gift_creator: None,
            gift_creator_guid: 0,
            creator: None,
            creator_guid: 0,
            enchantment: None,
            charges: 0,
            durability: (0, 0),
            link: Some(format!(
                "|cffffffff|Hitem:{item_id}:0:0:0|h[Linen Cloth]|h|r"
            )),
        }
    }

    fn state() -> TradeState {
        let mut player = TradeSideState {
            gold: 12_345,
            ..Default::default()
        };
        player.slots[0] = Some(item(2589, 5, 1));
        let mut target = TradeSideState {
            gold: 500,
            ..Default::default()
        };
        target.slots[0] = Some(item(4306, 1, 2));
        target.slots[6] = Some(item(6217, 1, 1)); // the enchant slot carries an item
        TradeState { player, target }
    }

    #[test]
    fn the_trade_slots_answer_their_item_links() {
        let mut s = UiScript::new().unwrap();
        // No trade open: both sides answer nil rather than raising.
        assert!(s
            .eval::<bool>("return GetTradePlayerItemLink(1) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradeTargetItemLink(1) == nil")
            .unwrap());

        let mut st = state();
        // A slot whose template is in flight has no link.
        st.target.slots[1] = Some(TradeSlotItem {
            item_id: 4306,
            name: None,
            texture: None,
            count: 1,
            quality: None,
            inventory_type: 0,
            lock_id: 0,
            wrapped: false,
            gift_creator: None,
            gift_creator_guid: 0,
            creator: None,
            creator_guid: 0,
            enchantment: None,
            charges: 0,
            durability: (0, 0),
            link: None,
        });
        s.set_trade(Some(st));

        assert_eq!(
            s.eval::<String>("return GetTradePlayerItemLink(1)")
                .unwrap(),
            "|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r"
        );
        assert_eq!(
            s.eval::<String>("return GetTradeTargetItemLink(1)")
                .unwrap(),
            "|cffffffff|Hitem:4306:0:0:0|h[Linen Cloth]|h|r"
        );
        // The in-flight slot, an empty one, and one past the seven.
        assert!(s
            .eval::<bool>("return GetTradeTargetItemLink(2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradePlayerItemLink(7) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradePlayerItemLink(9) == nil")
            .unwrap());
    }

    #[test]
    fn player_and_target_item_info_read_the_reference_tuples() {
        let mut s = UiScript::new().unwrap();
        // No trade open → every slot is nil, money 0.
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(1) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(), 0);

        s.set_trade(Some(state()));

        // Linen cloth has InventoryType 0, so the fourth value is the -1 sentinel.
        let (name, tex, count, quality): (String, String, i64, i64) = s
            .eval(
                "local n,t,c,q,e = GetTradePlayerItemInfo(1)\n\
                 return n,t,c,q",
            )
            .unwrap();
        assert_eq!((name.as_str(), count, quality), ("Linen Cloth", 5, -1));
        assert_eq!(tex, "Interface\\Icons\\INV_Fabric_Linen_01");
        assert!(s
            .eval::<bool>("local n,t,c,q,e = GetTradePlayerItemInfo(1)\nreturn e == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(2) == nil")
            .unwrap());

        // The target side adds the enchant and the usable flag; the fourth is the quality, -1 for
        // linen cloth (InventoryType 0).
        let (name, _tex, count, quality, usable): (String, String, i64, i64, i64) =
            s.eval("return GetTradeTargetItemInfo(1)").unwrap();
        assert_eq!(
            (name.as_str(), count, quality, usable),
            ("Linen Cloth", 1, -1, 1)
        );
        // The enchant slot (7) is filled on the target side.
        assert!(s
            .eval::<bool>("return GetTradeTargetItemInfo(7) ~= nil")
            .unwrap());
    }

    /// The fourth value is the quality, through the item-info gate (`0x4c059c`..`0x4c05c1`): the
    /// template's quality for a cached, equippable item, else the number -1; five values on every
    /// slot, the fourth never nil and never a usable flag.
    #[test]
    fn player_item_quality_is_gated_on_the_inventory_type() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        let mut weapon = item(1234, 1, 4);
        weapon.inventory_type = 13;
        st.player.slots[1] = Some(weapon);
        // Equippable, but its template is still in flight.
        let mut pending = item(1235, 1, 0);
        pending.quality = None;
        pending.inventory_type = 0;
        st.player.slots[2] = Some(pending);
        s.set_trade(Some(st));
        for (slot, want) in [(1, "5:number:-1"), (2, "5:number:4"), (3, "5:number:-1")] {
            assert_eq!(
                s.eval::<String>(&format!(
                    "local c = function(...) return arg.n .. ':' .. type(arg[4]) .. ':' .. \
                     tostring(arg[4]) end return c(GetTradePlayerItemInfo({slot}))"
                ))
                .unwrap(),
                want,
                "slot {slot}"
            );
        }
    }

    /// The target side's quality goes through the same gate, and an empty slot pushes the six-value
    /// miss tuple (`0x4c02f6`): nil, nil, 0, 0, nil, nil.
    #[test]
    fn target_item_info_gates_its_quality_and_pushes_six_values_on_a_miss() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        let mut weapon = item(1234, 1, 3);
        weapon.inventory_type = 13;
        st.target.slots[1] = Some(weapon);
        s.set_trade(Some(st));
        let shape = |s: &UiScript, slot: u32| -> String {
            s.eval(&format!(
                "local c = function(...) local o = {{}} for i = 1, arg.n do o[i] = tostring(arg[i]) end \
                 return table.concat(o, ',') end return c(GetTradeTargetItemInfo({slot}))"
            ))
            .unwrap()
        };
        assert_eq!(
            shape(&s, 2),
            "Linen Cloth,Interface\\Icons\\INV_Fabric_Linen_01,1,3,1,nil"
        );
        assert_eq!(shape(&s, 3), "nil,nil,0,0,nil,nil");
        // A row whose template is still in flight takes the miss too (`0x4c019b`, `0x4c02f6`).
        let mut st = state();
        st.target.slots[3] = Some(TradeSlotItem {
            item_id: 4306,
            count: 1,
            ..Default::default()
        });
        s.set_trade(Some(st));
        assert_eq!(shape(&s, 4), "nil,nil,0,0,nil,nil");
        assert_eq!(shape(&s, 0), "nil,nil,0,0,nil,nil");
        assert_eq!(shape(&s, 9), "nil,nil,0,0,nil,nil");
    }

    /// The texture is a string for a filled slot: `0x5d8af0` falls back to the question mark while
    /// the template is unknown.
    #[test]
    fn player_item_texture_falls_back_to_the_question_mark() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        let mut pending = item(1235, 1, 0);
        pending.texture = None;
        st.player.slots[1] = Some(pending);
        s.set_trade(Some(st));
        for (slot, want) in [
            (1, "Interface\\Icons\\INV_Fabric_Linen_01"),
            (2, "Interface\\Icons\\INV_Misc_QuestionMark"),
        ] {
            assert_eq!(
                s.eval::<String>(&format!(
                    "local n, t = GetTradePlayerItemInfo({slot}) return type(t) .. ':' .. t"
                ))
                .unwrap(),
                format!("string:{want}"),
                "slot {slot}"
            );
        }
    }

    #[test]
    fn money_reads_the_pushed_state() {
        let mut s = UiScript::new().unwrap();
        s.set_trade(Some(state()));
        assert_eq!(
            s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(),
            0,
            "our getter answers 0 with the partner cell clear"
        );
        s.open_trade_window();
        s.set_trade(Some(state()));
        assert_eq!(
            s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(),
            12_345
        );
        assert_eq!(s.eval::<i64>("return GetTargetTradeMoney()").unwrap(), 500);
    }

    #[test]
    fn clearing_the_trade_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_trade(Some(state()));
        s.set_trade(None);
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(1) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetTargetTradeMoney()").unwrap(), 0);
    }

    /// The item getters test no open state (`0x4c015a`, `0x4c04b3`): a close leaves both offers,
    /// the next open clears them (`0x4bf55d`-`0x4bf5ef`), and nothing else does. The money getters
    /// are the VM half only; the app's `both_offers_answer_inside_trade_closed…` drives the real
    /// close and the zeroing statuses.
    #[test]
    fn a_close_keeps_both_offers_and_the_next_open_clears_them() {
        let mut s = UiScript::new().unwrap();
        s.open_trade_window();
        s.set_trade(Some(state()));
        s.close_trade_window();
        for read in [
            "return GetTradePlayerItemInfo(1)",
            "return GetTradeTargetItemInfo(1)",
            "return GetTradeTargetItemInfo(7)",
        ] {
            assert_eq!(
                s.eval::<Option<String>>(read).unwrap().as_deref(),
                Some("Linen Cloth"),
                "{read} after the close"
            );
        }
        // A local close leaves the offers, but our money getter tests the partner cell
        // (`0x4c0706`) and the target's does not (`0x4c0759`).
        assert_eq!(s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetTargetTradeMoney()").unwrap(), 500);
        // The statuses that end a trade zero both before the close (`0x5d4884`).
        s.zero_trade_gold();
        assert_eq!(s.eval::<i64>("return GetTargetTradeMoney()").unwrap(), 0);

        s.open_trade_window();
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(1) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradeTargetItemInfo(1) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(), 0);
    }

    /// `0x4bfcf0` only unlocks: the table keeps its guids, so the scans of `0x4bfb10` and
    /// `0x4bfb60` still find the slot, and the open clears it.
    #[test]
    fn unlocking_the_offered_items_leaves_the_table() {
        let mut s = UiScript::new().unwrap();
        s.open_trade_window();
        s.set_trade(Some(TradeState::default()));
        let mut bag = crate::script::ContainerState::default();
        bag.slots.insert(
            1,
            crate::script::ContainerSlot {
                item_id: 2589,
                guid: 0x2589,
                count: 1,
                ..Default::default()
            },
        );
        s.set_container(0, Some(bag));
        s.run("PickupContainerItem(0, 1) ClickTradeButton(1)")
            .unwrap();
        assert!(s.item_locked(0x2589));
        s.unlock_trade_items();
        assert!(!s.item_locked(0x2589), "unlocked");
        assert_eq!(s.trade_slot_holding(0x2589), Some((0, 0, 1)), "still held");
        s.close_trade_window();
        assert_eq!(
            s.trade_slot_holding(0x2589),
            Some((0, 0, 1)),
            "past the close"
        );
        s.open_trade_window();
        assert_eq!(s.trade_slot_holding(0x2589), None, "the open clears it");
    }

    /// `0x4c017f` and `0x4c0582` ask the item cache with the callback that signals `TRADE_UPDATE`
    /// when a getter finds a filled slot whose template has not answered: one ask per call, none
    /// for an answered slot (a hit never calls it), an empty one or a link verb (no callback,
    /// `0x4c03e5`).
    #[test]
    fn a_getter_that_finds_an_unanswered_template_asks_the_cache_once_per_call() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        st.target.slots[0] = Some(TradeSlotItem {
            name: None,
            ..item(4306, 1, 2)
        });
        st.player.slots[1] = Some(TradeSlotItem {
            name: None,
            ..item(2592, 1, 1)
        });
        s.set_trade(Some(st));
        s.run(
            "GetTradeTargetItemInfo(1) GetTradeTargetItemInfo(1) GetTradePlayerItemInfo(2) \
             GetTradePlayerItemInfo(1) GetTradeTargetItemInfo(7) GetTradeTargetItemInfo(2) \
             GetTradeTargetItemLink(1) GetTradePlayerItemLink(2)",
        )
        .unwrap();
        assert_eq!(
            s.take_trade_record_asks(),
            [4306, 4306, 2592],
            "each unanswered call, in call order; the named slots, the empty one and the links ask nothing"
        );
        assert!(s.take_trade_record_asks().is_empty(), "drained");
    }

    #[test]
    fn intents_queue_and_drain() {
        let mut s = UiScript::new().unwrap();
        s.run("InitiateTrade('target')").unwrap();
        s.run("InitiateTrade('party2')").unwrap();
        assert_eq!(s.take_trade_initiates(), vec!["target", "party2"]);
        assert!(s.take_trade_initiates().is_empty(), "drained");

        s.run("AcceptTrade()").unwrap();
        assert!(s.take_trade_accept());
        assert!(!s.take_trade_accept(), "drained");

        s.run("CancelTradeAccept()").unwrap();
        assert!(s.take_trade_unaccept());

        s.run("CloseTrade()").unwrap();
        assert!(s.take_trade_close());
        assert!(!s.take_trade_close(), "drained");

        // An offer is sent only when the purse covers it.
        s.set_money(20_000);
        s.run("SetTradeMoney(1 * 10000 + 23 * 100 + 45)").unwrap();
        assert_eq!(s.take_trade_money(), [12_345]);
        assert!(s.take_trade_money().is_empty(), "drained");
        // The low dword of -5 is 0xFFFFFFFB, past any purse: refused silently.
        s.run("SetTradeMoney(-5)").unwrap();
        assert!(s.take_trade_money().is_empty());
        s.run("SetTradeMoney(30000)").unwrap();
        assert!(
            s.take_trade_money().is_empty(),
            "more than the purse: silent"
        );
        assert!(s.run("SetTradeMoney(nil)").is_err(), "a non-number raises");
    }

    /// `0x4bfa70` writes our slot and signals `TRADE_PLAYER_ITEM_CHANGED` with it (`0x4bfaef`)
    /// before the cursor's `SetCursorItem` (`0x4c0064`); an empty cursor on a filled slot writes
    /// the clear the same way, and `SetCursorItem`'s clear and put of the old item follow it.
    #[test]
    fn a_trade_slot_click_signals_its_slot_before_the_cursor() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(crate::script::cursor::two_item_bag()));
        s.set_trade(Some(TradeState::default()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        crate::script::tick::heard::listen(
            &s,
            &["TRADE_PLAYER_ITEM_CHANGED", "CURSOR_UPDATE"],
            Some("tostring(arg1) .. ' ' .. tostring((GetTradePlayerItemInfo(2)))"),
        );
        s.run("ClickTradeButton(2)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            [
                "TRADE_PLAYER_ITEM_CHANGED 2 Tough Jerky",
                "CURSOR_UPDATE nil Tough Jerky"
            ]
        );
        assert_eq!(s.take_trade_set_items(), vec![(2, 0, 1)]);

        s.run("ClickTradeButton(2)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            [
                "TRADE_PLAYER_ITEM_CHANGED 2 nil",
                "CURSOR_UPDATE nil nil",
                "CURSOR_UPDATE nil nil"
            ]
        );
        assert_eq!(s.take_trade_clear_items(), vec![2]);
    }

    /// `ClickTradeButton` (`0x4bfdf0`): a held item goes in and the slot's old item comes back on
    /// the cursor (`0x4c0036`-`0x4c0064`), both items locked while in the slot; an empty hand
    /// takes the old item back and queues the slot's clear; the same item on both sides, nothing
    /// included, is `ClearCursor(1,1)` (`0x4c0026`), which drops a spell. The close unlocks.
    #[test]
    fn click_trade_button_swaps_the_old_item_onto_the_cursor() {
        use crate::script::cursor::{bag_lock_probe, two_item_bag, CursorPayload, CursorSpell};
        use crate::script::tick::heard;

        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(two_item_bag()));
        s.set_trade(Some(TradeState::default()));
        let locked = |s: &UiScript, slot: u32| {
            s.eval::<bool>(&format!("return {} == 1", bag_lock_probe(slot)))
                .unwrap()
        };

        s.run("PickupContainerItem(0, 1) ClickTradeButton(2)")
            .unwrap();
        assert_eq!(s.take_trade_set_items(), vec![(2, 0, 1)]);
        assert!(s.cursor_payload().is_none());
        assert!(locked(&s, 1), "an offered item stays locked");

        heard::listen(
            &s,
            &["CURSOR_UPDATE", "ITEM_LOCK_CHANGED"],
            Some("CursorHasItem()"),
        );
        s.run("PickupContainerItem(0, 2)").unwrap();
        heard::take(&s);
        s.run("ClickTradeButton(2)").unwrap();
        // `SetCursorItem(0, old)`: its `ClearCursor(0,1)` lifts no lock, then the set.
        assert_eq!(heard::take(&s), ["CURSOR_UPDATE nil", "CURSOR_UPDATE 1"]);
        assert_eq!(s.take_trade_set_items(), vec![(2, 0, 2)]);
        assert_eq!(
            s.cursor_item().map(|i| i.slot),
            Some(1),
            "the old item is back"
        );
        assert!(locked(&s, 1) && locked(&s, 2));

        s.run("ClearCursor() ClickTradeButton(2)").unwrap();
        assert_eq!(s.take_trade_clear_items(), vec![2]);
        assert_eq!(s.cursor_item().map(|i| i.slot), Some(2));
        s.run("ClearCursor()").unwrap();
        heard::take(&s);

        // Nothing held and nothing offered: a plain clear, here of a spell.
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            spell_id: 133,
            texture: None,
        }));
        s.run("ClickTradeButton(4)").unwrap();
        assert!(s.cursor_payload().is_none());
        assert_eq!(heard::take(&s), ["CURSOR_UPDATE nil"]);
        assert!(s.take_trade_clear_items().is_empty());

        // The partner's column queues nothing and leaves the cursor alone.
        s.run("PickupContainerItem(0, 1) ClickTargetTradeButton(1)")
            .unwrap();
        assert!(s.take_trade_set_items().is_empty());
        assert!(s.eval::<bool>("return CursorHasItem()").unwrap());

        // Closing the window unlocks nothing (`0x4bfdc0`); `UnlockTradeItems` does, one event
        // each (`0x4bfd03`), and an invalidated slot unlocks with its own event (`0x4bfbd0`).
        s.run("ClickTradeButton(1)").unwrap();
        heard::take(&s);
        s.set_trade(None);
        assert!(locked(&s, 1));
        s.unlock_trade_items();
        assert_eq!(heard::take(&s), ["ITEM_LOCK_CHANGED nil"]);
        assert!(!locked(&s, 1));

        heard::listen(
            &s,
            &["ITEM_LOCK_CHANGED", "TRADE_PLAYER_ITEM_CHANGED"],
            None,
        );
        s.run("PickupContainerItem(0, 2) ClickTradeButton(3)")
            .unwrap();
        heard::take(&s);
        s.invalidate_trade_slot(2);
        assert_eq!(
            heard::take(&s),
            ["ITEM_LOCK_CHANGED", "TRADE_PLAYER_ITEM_CHANGED"]
        );
        assert!(!locked(&s, 2));
    }

    /// The put-back passes count 0 (`0x4c004e`), so a split offered comes back as the whole stack.
    /// The set send tests no trade (`0x4bfa70`, `0x5d4380`), so it goes out with none open.
    #[test]
    fn an_offered_split_comes_back_whole() {
        let mut s = UiScript::new().unwrap();
        let mut bag = crate::script::cursor::two_item_bag();
        bag.slots.get_mut(&1).unwrap().count = 5;
        s.set_container(0, Some(bag));
        s.run("SplitContainerItem(0, 1, 2) ClickTradeButton(1)")
            .unwrap();
        assert_eq!(s.take_trade_set_items(), vec![(1, 0, 1)]);
        s.run("ClickTradeButton(1)").unwrap();
        let back = s.cursor_item().expect("the item is back");
        assert_eq!((back.slot, back.count), (1, None));
    }

    /// `ClickTradeButton`'s client refusals for slots 1 to 6 (`0x4bfeb5`-`0x4bffae`), in the
    /// reference's order: the error queued, nothing sent, no signal, the cursor kept. Slot 7 takes
    /// a bound item, and an item that passes is placed as before.
    #[test]
    fn click_trade_button_refuses_bound_quest_and_bag_items_for_the_traded_slots() {
        use crate::script::container::{ContainerSlot, ContainerState};
        let mut s = UiScript::new().unwrap();
        s.set_trade(Some(TradeState::default()));
        let slot = |guid: u64, bound: bool, bonding: u32, seats: Option<u32>| ContainerSlot {
            item_id: 1000 + guid as u32,
            guid,
            count: 1,
            already_bound: bound,
            bonding,
            occupied_seats: seats,
            ..Default::default()
        };
        let mut bag = ContainerState {
            num_slots: 16,
            ..Default::default()
        };
        bag.slots.insert(1, slot(1, true, 1, None));
        bag.slots.insert(2, slot(2, true, 4, None));
        bag.slots.insert(3, slot(3, true, 5, Some(3)));
        bag.slots.insert(4, slot(4, false, 0, Some(2)));
        bag.slots.insert(5, slot(5, false, 0, Some(0)));
        bag.slots.insert(6, slot(6, false, 2, None));
        s.set_container(0, Some(bag));
        // An empty bag worn in `Bag1Slot` (doll id 21).
        let mut doll: crate::script::InventorySlots = Default::default();
        doll[21] = Some(crate::script::InvSlotView {
            item_id: 4496,
            guid: 0x21,
            occupied_seats: Some(0),
            equip_slots: vec![20, 21, 22, 23],
            ..Default::default()
        });
        s.set_inventory_slots(doll);
        s.run(
            r#"SEEN = 0
               local f = CreateFrame("Frame")
               f:RegisterEvent("TRADE_PLAYER_ITEM_CHANGED")
               f:SetScript("OnEvent", function() SEEN = SEEN + 1 end)"#,
        )
        .unwrap();
        let click = |s: &mut UiScript, pick: &str, id: i64| -> (Vec<&'static str>, i64, bool) {
            s.run(&format!("ClearCursor() {pick} ClickTradeButton({id})"))
                .unwrap();
            let sent = !s.take_trade_set_items().is_empty();
            let seen: i64 = s.eval("local n = SEEN SEEN = 0 return n").unwrap();
            let held = s.cursor_item().is_some();
            assert_eq!(held, !sent, "{pick}: a refusal keeps the cursor");
            (s.take_ui_errors(), seen, sent)
        };
        for (pick, key) in [
            ("PickupContainerItem(0, 1)", "ERR_TRADE_BOUND_ITEM"),
            ("PickupContainerItem(0, 2)", "ERR_TRADE_QUEST_ITEM"),
            // Bound comes first, its contents unasked.
            ("PickupContainerItem(0, 3)", "ERR_TRADE_QUEST_ITEM"),
            ("PickupContainerItem(0, 4)", "ERR_TRADE_BAG"),
            ("PickupInventoryItem(21)", "ERR_TRADE_EQUIPPED_BAG"),
        ] {
            assert_eq!(click(&mut s, pick, 1), (vec![key], 0, false), "{pick}");
        }
        // An empty bag in the backpack and an unbound item pass, as does a bound item on slot 7.
        assert_eq!(
            click(&mut s, "PickupContainerItem(0, 5)", 2),
            (vec![], 1, true)
        );
        assert_eq!(
            click(&mut s, "PickupContainerItem(0, 6)", 3),
            (vec![], 1, true)
        );
        assert_eq!(
            click(&mut s, "PickupContainerItem(0, 1)", 7),
            (vec![], 1, true)
        );
        // The id less one is compared signed: a zero or negative id refuses too, and an unbound
        // item on it reaches the slot compare, which reads it as no slot and sends nothing.
        for id in [0, -3] {
            assert_eq!(
                click(&mut s, "PickupContainerItem(0, 2)", id),
                (vec!["ERR_TRADE_QUEST_ITEM"], 0, false),
                "id {id}"
            );
        }
        s.run("ClearCursor() PickupContainerItem(0, 6) ClickTradeButton(-3)")
            .unwrap();
        assert!(s.take_ui_errors().is_empty());
        assert!(s.take_trade_set_items().is_empty());
        // A non-number raises the binding's usage.
        let err = s.run("ClickTradeButton('x')").unwrap_err().to_string();
        assert!(err.contains("Usage: ClickTradeButton(index)"), "{err}");
    }
}
