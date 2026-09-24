//! The auction-house bindings: the app pushes a snapshot of three lists, rows already resolved and
//! in display order, and the Lua calls queue intents the app drains. Getters are keyed by `"list"`
//! (Browse), `"bidder"` (Bids) or `"owner"` (Auctions), with `index` 1-based in the 50-row batch.
//!
//! The wire carries no sort: each list keeps an 8-deep stack of `(key, reversed)`, where clicking
//! the primary column toggles it and another column is promoted with the direction it remembers.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{self, CursorPayload};
use super::Model;

/// The three list types, in the order the app pushes them.
pub const LIST: usize = 0;
pub const BIDDER: usize = 1;
pub const OWNER: usize = 2;

/// The eight sort keys the reference's column headers pass, in the API's order.
pub const SORT_KEYS: [&str; 8] = [
    "level", "quality", "bid", "duration", "buyout", "status", "name", "seller",
];

/// A list type's snapshot index, for the other modules (the tooltip's `SetAuctionItem`).
pub(super) fn list_index_of(kind: &str) -> Option<usize> {
    list_index(kind)
}

fn list_index(kind: &str) -> Option<usize> {
    match kind {
        "list" => Some(LIST),
        "bidder" => Some(BIDDER),
        "owner" => Some(OWNER),
        _ => None,
    }
}

/// One auction row, resolved by the app from a wire record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionItemRow {
    /// The wire auction id a bid or a cancel addresses; never shown.
    pub auction_id: u32,
    pub item_id: u32,
    /// The wire's `randomPropertyId`, 0 when unrolled; [`Self::name`] already carries its suffix,
    /// and the reference's `SetAuctionItem` writes it into the tooltip's `+0x424`.
    pub random_property_id: u32,
    /// `None` while the item template query is in flight.
    pub name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    pub quality: Option<u32>,
    /// `RequiredLevel`, printed red above the player's level.
    pub level: u32,
    /// The seller's opening price, shown as the current bid while [`Self::bid_amount`] is 0.
    pub min_bid: u32,
    /// The minimum step above the current bid; 0 before any bid.
    pub min_increment: u32,
    /// `0` = no buyout.
    pub buyout_price: u32,
    /// The current high bid; 0 before any bid.
    pub bid_amount: u32,
    /// Whether the player holds the high bid: a flag, not a name.
    pub high_bidder: bool,
    /// The seller's name; `None` while the name query is in flight.
    pub owner: Option<String>,
    /// The time-left bucket, `1..=4` (Short, Medium, Long, Very Long); 0 when not known yet.
    pub time_left: u32,
    /// The full item link: auction rows carry entry, enchant, random property and suffix.
    pub link: Option<String>,
}

/// One of the three lists: the batch the server sent, the pre-cap match count and its sort stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionListState {
    /// The current batch in display order, at most 50 (the server's page cap).
    pub rows: Vec<AuctionItemRow>,
    /// `totalAuctions`: the match count before the 50-row cap.
    pub total: u32,
    /// The most-recently-clicked sort stack, primary first: `(key, reversed)`.
    pub sort: Vec<(String, bool)>,
}

impl AuctionListState {
    /// This list's remembered direction for `key`, or `false` for a key it has never sorted by.
    fn reversed(&self, key: &str) -> bool {
        self.sort
            .iter()
            .find(|(k, _)| k == key)
            .is_some_and(|(_, rev)| *rev)
    }
}

/// One class of the Browse tab's category tree, from the player's `ItemClass.dbc`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionCategory {
    /// The wire's `mainCategory`: an item class id, not the menu position.
    pub class_id: u32,
    pub name: String,
    /// The `0x807060` table's second column. False (Consumable, Trade Goods, Reagent,
    /// Miscellaneous) empties `GetAuctionItemSubClasses` and `GetAuctionInvTypes`, but
    /// `QueryAuctionItems` never reads it.
    pub has_subclass_filter: bool,
    /// The class's `ItemSubClass.dbc` rows with `DisplayFlags & 2` clear, in file order: what both
    /// the menu and the query's index scan walk.
    pub subclasses: Vec<AuctionSubCategory>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionSubCategory {
    /// The wire's `subCategory`.
    pub sub_id: u32,
    pub name: String,
    /// `ItemSubClass.Flags & 0x200` (`0x4cfb63`): whether the subclass offers the 14
    /// inventory-slot rows.
    pub has_inv_types: bool,
}

/// The open auction house's snapshot, pushed whole by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionState {
    /// Browse / Bids / Auctions, indexed by [`LIST`], [`BIDDER`], [`OWNER`].
    pub lists: [AuctionListState; 3],
    /// `GetAuctionHouseDepositRate()`: the house's `AuctionHouse.dbc` deposit percentage.
    pub deposit_percent: u32,
}

/// A drained `QueryAuctionItems`, the `CMSG_AUCTION_LIST_ITEMS` filters: each is already a wire id,
/// mapped from the Lua's menu position as `0x4ce980` does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionQuery {
    pub name: String,
    /// `0` = no filter, for both.
    pub min_level: u32,
    pub max_level: u32,
    /// `None` = no filter (the wire's `0xFFFFFFFF`), for all three.
    pub inv_type: Option<u32>,
    pub class: Option<u32>,
    pub sub_class: Option<u32>,
    /// A minimum quality, not an equality; `None` = no filter (the dropdown's All, -1).
    pub quality: Option<u32>,
    /// 0-based; the app multiplies by 50 for the wire's item offset.
    pub page: u32,
    pub usable_only: bool,
}

/// A drained `StartAuction`.
#[derive(Clone, Debug, PartialEq)]
pub struct AuctionStartRequest {
    pub min_bid: u32,
    /// `0` = no buyout.
    pub buyout: u32,
    /// Minutes: 120, 480 or 1440, the only values the reference sends.
    pub duration: u32,
}

/// A drained `PlaceAuctionBid`.
#[derive(Clone, Debug, PartialEq)]
pub struct AuctionBid {
    /// [`LIST`] or [`BIDDER`]: the Auctions tab has no bid button.
    pub list: usize,
    /// 1-based row within the current batch; the app maps it to the wire auction id.
    pub index: u32,
    pub amount: u32,
}

impl super::UiScript {
    /// Push the Browse tab's class tree. Login-scoped: the stock `AuctionFrameBrowse_OnLoad` reads
    /// the classes when the addon loads, before any auctioneer session.
    pub fn set_auction_item_classes(&mut self, classes: Vec<AuctionCategory>) {
        self.model_mut().auction_item_classes = classes;
    }

    pub fn set_auction(&mut self, state: Option<AuctionState>) {
        let mut model = self.model_mut();
        // Only opening or closing the session drops the selection: it is an auction id, so it
        // survives a new page or a re-sort and stops resolving when the auction leaves the page.
        if model.auction.is_none() || state.is_none() {
            model.auction_selected = [0; 3];
        }
        model.auction = state;
    }

    /// Push `CanSendAuctionQuery()`'s answer, the app's query throttle.
    pub fn set_auction_can_query(&mut self, can: bool) {
        self.model_mut().auction_can_query = can;
    }

    /// Drain the Browse search the Lua queued, if any.
    pub fn take_auction_query(&mut self) -> Option<AuctionQuery> {
        self.model_mut().auction_query.take()
    }

    /// Drain the owner-list page request (`GetOwnerAuctionItems`), 0-based.
    pub fn take_auction_owner_query(&mut self) -> Option<u32> {
        self.model_mut().auction_owner_query.take()
    }

    /// Drain the bidder-list page request (`GetBidderAuctionItems`), 0-based.
    pub fn take_auction_bidder_query(&mut self) -> Option<u32> {
        self.model_mut().auction_bidder_query.take()
    }

    /// Drain the bids placed since the last drain.
    pub fn take_auction_bids(&mut self) -> Vec<AuctionBid> {
        std::mem::take(&mut self.model_mut().auction_bids)
    }

    /// Drain the 1-based owner-row indices `CancelAuction` was called on.
    pub fn take_auction_cancels(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().auction_cancels)
    }

    /// Drain a pending `StartAuction`.
    pub fn take_auction_start(&mut self) -> Option<AuctionStartRequest> {
        self.model_mut().auction_start.take()
    }

    /// Drain the header clicks: `(list index, sort key)`, in click order. The app owns the stacks.
    pub fn take_auction_sorts(&mut self) -> Vec<(usize, String)> {
        std::mem::take(&mut self.model_mut().auction_sorts)
    }

    /// Whether `CloseAuctionHouse` was called since the last drain (and clear the flag).
    pub fn take_auction_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().auction_close)
    }

    /// The sell slot's `(bag, slot)`, resolved by the app to the item guid on `StartAuction`.
    pub fn auction_sell_item(&mut self) -> Option<(i64, u32)> {
        self.model_mut()
            .auction_sell_item
            .as_ref()
            .map(|it| (it.bag, it.slot))
    }

    /// Empty the sell slot, once the auction is sent and on session close.
    pub fn clear_auction_sell_item(&mut self) {
        self.model_mut().auction_sell_item = None;
    }
}

fn row_at(model: &Model, kind: &str, index: usize) -> Option<AuctionItemRow> {
    let i = list_index(kind)?;
    model
        .auction
        .as_ref()
        .and_then(|a| index.checked_sub(1).and_then(|n| a.lists[i].rows.get(n)))
        .cloned()
}

/// The sell slot's `(name, texture, count, quality, canUse, stackValue)`, `stackValue` being the
/// vendor price times the stack: the base of the suggested opening price and the deposit.
fn sell_item_info(model: &Model) -> Option<(String, Option<String>, u32, i64, bool, u32)> {
    let it = model.auction_sell_item.as_ref()?;
    // The real stack size, not the split-carry field: the deposit is per stack.
    let count = cursor::held_count(model, it);
    let template = model.item_templates.get(&it.item_id);
    let name = cursor::item_link_name(it.link.as_deref());
    let quality = it.quality.map_or(-1, i64::from);
    let sell_price = template.map_or(0, |v| v.sell_price);
    let can_use = super::item_stats::item_usable_by_id(model, it.item_id);
    Some((
        name,
        it.texture.clone(),
        count,
        quality,
        can_use,
        sell_price.saturating_mul(count),
    ))
}

/// The reference client's deposit, `floor(rate × stackValue / 100) × floor(minutes / 120)`: the
/// inner floor zeroes a cheap stack the server still charges a few copper for.
fn deposit_for(rate: u32, stack_value: u32, minutes: u32) -> u32 {
    let per_unit = u64::from(rate).saturating_mul(u64::from(stack_value)) / 100;
    let units = u64::from(minutes) / 120;
    u32::try_from(per_unit.saturating_mul(units)).unwrap_or(u32::MAX)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumAuctionItems(type) → numBatchAuctions (at most 50), totalAuctions (before the cap).
    g.set(
        "GetNumAuctionItems",
        lua.create_function(|lua, kind: String| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some((batch, total)) = list_index(&kind).and_then(|i| {
                model
                    .auction
                    .as_ref()
                    .map(|a| (a.lists[i].rows.len() as i64, i64::from(a.lists[i].total)))
            }) else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Integer(0),
                    Value::Integer(0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(batch),
                Value::Integer(total.max(batch)),
            ]))
        })?,
    )?;

    // GetAuctionItemInfo(type, index) → twelve values; `highBidder` is a flag, not the seller.
    g.set(
        "GetAuctionItemInfo",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                row_at(&model, &kind, index)
            };
            let Some(r) = row else {
                // A miss (unknown type, out of range, not cached) still answers twelve values, with
                // `count = 1` and `quality = -1` (one shared exit, `0x4cf1ec`), read unguarded.
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,         // name
                    Value::Nil,         // texture
                    Value::Integer(1),  // count
                    Value::Integer(-1), // quality
                    Value::Nil,         // canUse
                    Value::Integer(0),  // level
                    Value::Integer(0),  // minBid
                    Value::Integer(0),  // minIncrement
                    Value::Integer(0),  // buyoutPrice
                    Value::Integer(0),  // bidAmount
                    Value::Nil,         // highBidder
                    Value::Nil,         // owner
                ]));
            };
            // `canUse` is per call, not pushed: it follows the player's level, class and spellbook.
            let can_use = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                super::item_stats::item_usable_by_id(&model, r.item_id)
            };
            let opt_str = |s: &Option<String>| -> mlua::Result<Value> {
                Ok(match s {
                    Some(v) => Value::String(lua.create_string(v)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(&r.name)?,
                opt_str(&r.texture)?,
                Value::Integer(i64::from(r.count)),
                match r.quality {
                    Some(q) => Value::Integer(i64::from(q)),
                    None => Value::Nil,
                },
                flag(can_use),
                Value::Integer(i64::from(r.level)),
                Value::Integer(i64::from(r.min_bid)),
                Value::Integer(i64::from(r.min_increment)),
                Value::Integer(i64::from(r.buyout_price)),
                Value::Integer(i64::from(r.bid_amount)),
                flag(r.high_bidder),
                opt_str(&r.owner)?,
            ]))
        })?,
    )?;

    // GetAuctionItemTimeLeft(type, index) → the 1..4 bucket, 0 when unknown. The reference has no
    // live countdown: it moves only when a new list lands.
    g.set(
        "GetAuctionItemTimeLeft",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(row_at(&model, &kind, index).map_or(0, |r| i64::from(r.time_left)))
        })?,
    )?;

    g.set(
        "GetAuctionItemLink",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                row_at(&model, &kind, index).and_then(|r| r.link)
            };
            // A miss returns no values at all, not nil (`0x4cf45b`).
            Ok(match link {
                Some(l) => MultiValue::from_vec(vec![Value::String(lua.create_string(&l)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetSelectedAuctionItem(type) → the selected row, 0 for none. Stored as the auction id and
    // resolved on the way out (`0x4cfda0`/`0x4cfec0`), so it follows its auction through a re-sort.
    g.set(
        "GetSelectedAuctionItem",
        lua.create_function(|lua, kind: String| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(i) = list_index(&kind) else {
                return Ok(0i64);
            };
            let id = model.auction_selected[i];
            if id == 0 {
                return Ok(0i64);
            }
            Ok(model.auction.as_ref().map_or(0, |a| {
                a.lists[i]
                    .rows
                    .iter()
                    .position(|r| r.auction_id == id)
                    .map_or(0, |p| p as i64 + 1)
            }))
        })?,
    )?;

    // SetSelectedAuctionItem(type, index): remembers the auction at that row; out of range clears.
    g.set(
        "SetSelectedAuctionItem",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let Some(i) = list_index(&kind) else {
                return Ok(());
            };
            let id = model
                .auction
                .as_ref()
                .and_then(|a| index.checked_sub(1).and_then(|n| a.lists[i].rows.get(n)))
                .map_or(0, |r| r.auction_id);
            model.auction_selected[i] = id;
            Ok(())
        })?,
    )?;

    // SortAuctionItems(type, key): queue the header click for the app, which pushes the reordered
    // rows back. The reference raises this `Usage:` only for an argument neither string nor number
    // (`0x6f3510`) and matches both strings case-insensitively (`0x64a4c0`), an unknown one sorting
    // nothing (`0x4cfd90`); here an unknown or differently cased type or column raises.
    g.set(
        "SortAuctionItems",
        lua.create_function(|lua, (kind, key): (String, String)| {
            let Some(i) = list_index(&kind) else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: SortAuctionItems(\"type\", \"sort\")".into(),
                ));
            };
            if !SORT_KEYS.contains(&key.as_str()) {
                return Err(mlua::Error::RuntimeError(
                    "Usage: SortAuctionItems(\"type\", \"sort\")".into(),
                ));
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.auction_sorts.push((i, key));
            Ok(())
        })?,
    )?;

    // IsAuctionSortReversed(type, key) → 1/nil, for any key the stack remembers.
    g.set(
        "IsAuctionSortReversed",
        lua.create_function(|lua, (kind, key): (String, String)| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let rev = list_index(&kind)
                .zip(model.auction.as_ref())
                .is_some_and(|(i, a)| a.lists[i].reversed(&key));
            Ok(flag(rev))
        })?,
    )?;

    // CanSendAuctionQuery() → 1/nil; a throttled query is dropped silently.
    g.set(
        "CanSendAuctionQuery",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.auction_can_query))
        })?,
    )?;

    // GetAuctionItemClasses() → the ten auctionable class names in the reference's menu order, off
    // the player's ItemClass.dbc; pushed at login, no session needed.
    g.set(
        "GetAuctionItemClasses",
        lua.create_function(|lua, ()| {
            let names: Vec<String> = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .auction_item_classes
                    .iter()
                    .map(|c| c.name.clone())
                    .collect()
            };
            let mut out = Vec::with_capacity(names.len());
            for n in &names {
                out.push(Value::String(lua.create_string(n)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetAuctionItemSubClasses(classIndex) → the subclass names; an out-of-range index or a class
    // whose `0x807060` flag is 0 returns nothing, without an error (`0x4cfa00`, `0x4cfa0e`).
    g.set(
        "GetAuctionItemSubClasses",
        lua.create_function(|lua, class_index: usize| {
            let names: Vec<String> = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                class_index
                    .checked_sub(1)
                    .and_then(|i| model.auction_item_classes.get(i))
                    .filter(|c| c.has_subclass_filter)
                    .map_or_else(Vec::new, |c| {
                        c.subclasses.iter().map(|s| s.name.clone()).collect()
                    })
            };
            let mut out = Vec::with_capacity(names.len());
            for n in &names {
                out.push(Value::String(lua.create_string(n)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetAuctionInvTypes(classIndex, subclassIndex) → the fixed 14 inventory-slot GlobalString
    // keys, or nothing (`0x4cfab0`): nothing without a class (or with its `0x807060` flag 0); a
    // found subclass needs `Flags & 0x200`, but an index the scan runs past skips that gate (the
    // exhaust edge at `0x4cfb61` lands past the test at `0x4cfb63`).
    g.set(
        "GetAuctionInvTypes",
        lua.create_function(|lua, (class_index, sub_index): (usize, usize)| {
            let offers = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                class_index
                    .checked_sub(1)
                    .and_then(|i| model.auction_item_classes.get(i))
                    .filter(|c| c.has_subclass_filter)
                    .is_some_and(|c| {
                        sub_index
                            .checked_sub(1)
                            .and_then(|i| c.subclasses.get(i))
                            .is_none_or(|s| s.has_inv_types)
                    })
            };
            if !offers {
                return Ok(MultiValue::new());
            }
            let mut out = Vec::with_capacity(AUCTION_INV_TYPES.len());
            for (_, key) in AUCTION_INV_TYPES {
                out.push(Value::String(lua.create_string(key)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // QueryAuctionItems(name, minLevel, maxLevel, invTypeIndex, classIndex, subclassIndex, page,
    // isUsable, qualityIndex): nine arguments, no `exactMatch` in 1.12. Each filter is a 1-based
    // menu position mapped to the wire id as `0x4ce980` does: invType through the `0x8070b0` rows,
    // class through `0x807060`, subclass as the class's Nth non-excluded row (a scan that never
    // reads the class's filter flag). Anything unresolved is the wire's "any".
    g.set(
        "QueryAuctionItems",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let num = |i: usize| -> Option<u32> {
                match a.get(i) {
                    Some(Value::Integer(n)) => u32::try_from(*n).ok(),
                    Some(Value::Number(n)) => (*n >= 0.0).then_some(*n as u32),
                    // The min/max level boxes are edit boxes: their text arrives as a string.
                    Some(Value::String(s)) => s.to_str().ok()?.trim().parse::<u32>().ok(),
                    _ => None,
                }
            };
            let name = match a.first() {
                Some(Value::String(s)) => s.to_str().map(|s| s.to_string()).unwrap_or_default(),
                _ => String::new(),
            };
            let truthy = |i: usize| {
                !matches!(
                    a.get(i),
                    None | Some(Value::Nil) | Some(Value::Boolean(false))
                )
            };
            // qualityIndex arrives as the dropdown's value: -1 for All, else the quality itself.
            let quality = match a.get(8) {
                Some(Value::Integer(n)) if *n >= 0 => u32::try_from(*n).ok(),
                Some(Value::Number(n)) if *n >= 0.0 => Some(*n as u32),
                _ => None,
            };
            let position = |i: usize| num(i).and_then(|n| (n as usize).checked_sub(1));
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let inv_type = position(3)
                .and_then(|i| AUCTION_INV_TYPES.get(i))
                .map(|&(id, _)| id);
            let category = position(4).and_then(|i| model.auction_item_classes.get(i));
            let class = category.map(|c| c.class_id);
            let sub_class = category.and_then(|c| {
                position(5)
                    .and_then(|i| c.subclasses.get(i))
                    .map(|s| s.sub_id)
            });
            model.auction_query = Some(AuctionQuery {
                name,
                min_level: num(1).unwrap_or(0),
                max_level: num(2).unwrap_or(0),
                inv_type,
                class,
                sub_class,
                quality,
                page: num(6).unwrap_or(0),
                usable_only: truthy(7),
            });
            Ok(())
        })?,
    )?;

    // GetOwnerAuctionItems([page]) / GetBidderAuctionItems([page]): unthrottled, page 0 by default.
    g.set(
        "GetOwnerAuctionItems",
        lua.create_function(|lua, page: Option<u32>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.auction_owner_query = Some(page.unwrap_or(0));
            Ok(())
        })?,
    )?;
    g.set(
        "GetBidderAuctionItems",
        lua.create_function(|lua, page: Option<u32>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.auction_bidder_query = Some(page.unwrap_or(0));
            Ok(())
        })?,
    )?;

    // PlaceAuctionBid(type, index, bidAmount); a buyout is a bid of exactly the buyout price.
    g.set(
        "PlaceAuctionBid",
        lua.create_function(|lua, (kind, index, amount): (String, u32, u32)| {
            let Some(list) = list_index(&kind) else {
                return Ok(());
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.auction_bids.push(AuctionBid {
                list,
                index,
                amount,
            });
            Ok(())
        })?,
    )?;

    // StartAuction(minBid, buyoutPrice, runTime), runTime in minutes.
    g.set(
        "StartAuction",
        lua.create_function(|lua, (min_bid, buyout, duration): (u32, u32, u32)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.auction_start = Some(AuctionStartRequest {
                min_bid,
                buyout,
                duration,
            });
            Ok(())
        })?,
    )?;

    // CancelAuction(index), always an "owner" row.
    g.set(
        "CancelAuction",
        lua.create_function(|lua, index: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.auction_cancels.push(index);
            Ok(())
        })?,
    )?;

    g.set(
        "GetAuctionHouseDepositRate",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model
                .auction
                .as_ref()
                .map_or(0, |a| i64::from(a.deposit_percent)))
        })?,
    )?;

    // CalculateAuctionDeposit(runTime) → copper, by the client's arithmetic (`deposit_for`).
    g.set(
        "CalculateAuctionDeposit",
        lua.create_function(|lua, minutes: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let rate = model.auction.as_ref().map_or(0, |a| a.deposit_percent);
            let stack_value = sell_item_info(&model).map_or(0, |(_, _, _, _, _, v)| v);
            Ok(i64::from(deposit_for(rate, stack_value, minutes)))
        })?,
    )?;

    // GetAuctionSellItemInfo() → name, texture, count, quality, canUse, price: six values always.
    // An empty slot answers count 1 and quality -1: the stock Lua reads `count > 1` unguarded.
    g.set(
        "GetAuctionSellItemInfo",
        lua.create_function(|lua, ()| {
            let info = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                sell_item_info(&model)
            };
            let Some((name, texture, count, quality, can_use, price)) = info else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(1),
                    Value::Integer(-1),
                    Value::Nil,
                    Value::Integer(0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                if name.is_empty() {
                    Value::Nil
                } else {
                    Value::String(lua.create_string(&name)?)
                },
                match &texture {
                    Some(t) => Value::String(lua.create_string(t)?),
                    None => Value::Nil,
                },
                Value::Integer(i64::from(count)),
                Value::Integer(quality),
                flag(can_use),
                Value::Integer(i64::from(price)),
            ]))
        })?,
    )?;

    // ClickAuctionSellItemButton(): attach the cursor's item, or pick the attached one back up;
    // every cursor write queues both CURSOR_UPDATE and ITEM_LOCK_CHANGED.
    g.set(
        "ClickAuctionSellItemButton",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            click_auction_sell_item(&mut model);
            Ok(())
        })?,
    )?;

    // CloseAuctionHouse(): the client-side close; nothing goes on the wire.
    g.set(
        "CloseAuctionHouse",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.auction_close = true;
            Ok(())
        })?,
    )?;

    Ok(())
}

/// The `0x8070b0` table's fourteen `{u32 invTypeId; char name[0x20]}` rows, in its order: the id
/// goes on the wire, the name is a GlobalString key.
const AUCTION_INV_TYPES: [(u32, &str); 14] = [
    (1, "INVTYPE_HEAD"),
    (2, "INVTYPE_NECK"),
    (3, "INVTYPE_SHOULDER"),
    (4, "INVTYPE_BODY"),
    (5, "INVTYPE_CHEST"),
    (6, "INVTYPE_WAIST"),
    (7, "INVTYPE_LEGS"),
    (8, "INVTYPE_FEET"),
    (9, "INVTYPE_WRIST"),
    (10, "INVTYPE_HAND"),
    (11, "INVTYPE_FINGER"),
    (12, "INVTYPE_TRINKET"),
    (16, "INVTYPE_CLOAK"),
    (23, "INVTYPE_HOLDABLE"),
];

/// Attach the cursor's item to the sell slot, or with an empty cursor pick the attached one back
/// up; any other payload stays on the cursor.
fn click_auction_sell_item(model: &mut Model) {
    match model.cursor.take() {
        Some(CursorPayload::Item(item)) => {
            let (bag, slot) = (item.bag, item.slot);
            model.auction_sell_item = Some(item);
            cursor::queue_cursor_update(model);
            cursor::queue_lock_changed(model, bag, slot);
        }
        None => {
            if let Some(item) = model.auction_sell_item.take() {
                let (bag, slot) = (item.bag, item.slot);
                model.cursor = Some(CursorPayload::Item(item));
                cursor::queue_cursor_update(model);
                cursor::queue_lock_changed(model, bag, slot);
            }
        }
        Some(other) => {
            model.cursor = Some(other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// A session holding one browse page of the given auction ids.
    fn page(ids: &[u32]) -> AuctionState {
        let rows: Vec<AuctionItemRow> = ids
            .iter()
            .map(|&auction_id| AuctionItemRow {
                auction_id,
                item_id: 1000 + auction_id,
                count: 1,
                ..Default::default()
            })
            .collect();
        let mut state = AuctionState::default();
        state.lists[LIST] = AuctionListState {
            total: rows.len() as u32,
            rows,
            sort: Vec::new(),
        };
        state
    }

    #[test]
    fn the_selection_follows_its_auction_through_a_resort() {
        let mut s = UiScript::new().unwrap();
        s.set_auction(Some(page(&[11, 22])));

        s.eval::<()>(r#"SetSelectedAuctionItem("list", 2)"#)
            .unwrap();
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            2
        );

        // The same two auctions, re-sorted into the opposite order.
        s.set_auction(Some(page(&[22, 11])));
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            1,
            "auction 22 moved to row 1 and the selection went with it"
        );

        // It leaves the page.
        s.set_auction(Some(page(&[33, 44])));
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            0,
            "gone means nothing selected, never its neighbour"
        );

        // Closing the session drops it outright.
        s.set_auction(Some(page(&[33])));
        s.eval::<()>(r#"SetSelectedAuctionItem("list", 1)"#)
            .unwrap();
        s.set_auction(None);
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            0
        );
    }

    #[test]
    fn the_sell_slot_reads_a_whole_stacks_real_size() {
        use crate::script::container::{ContainerSlot, ContainerState};
        use crate::script::cursor::CursorItem;

        let mut s = UiScript::new().unwrap();
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1u32,
            ContainerSlot {
                count: 20,
                item_id: 2589,
                ..Default::default()
            },
        );
        s.set_container(
            0,
            Some(ContainerState {
                name: None,
                num_slots: 16,
                slots,
            }),
        );
        // Picked up whole: `count: None` means the whole stack, not one.
        {
            let mut model = s.model_mut();
            model.auction_sell_item = Some(CursorItem {
                bag: 0,
                slot: 1,
                item_id: 2589,
                texture: None,
                link: None,
                count: None,
                quality: None,
                equip_slots: Vec::new(),
                bar_placeable: false,
            });
        }
        let count: i64 = s
            .eval(r#"local _, _, c = GetAuctionSellItemInfo() return c"#)
            .unwrap();
        assert_eq!(count, 20, "the whole stack, not one of it");
    }

    #[test]
    fn a_missing_row_still_answers_twelve_values() {
        let s = UiScript::new().unwrap();
        let n = s.arity(r#"GetAuctionItemInfo("list", 99)"#).unwrap();
        assert_eq!(n, 12, "twelve, even with no session open at all");
        let (count, quality): (i64, i64) = s
            .eval(r#"local _, _, c, q = GetAuctionItemInfo("list", 99) return c, q"#)
            .unwrap();
        assert_eq!((count, quality), (1, -1));

        // The link answers no values on a miss, not a nil.
        let n = s.arity(r#"GetAuctionItemLink("list", 99)"#).unwrap();
        assert_eq!(n, 0, "zero values, not one nil");
    }

    /// A 9-copper stack at 5% over 24 h deposits 0 here, where vmangos charges 5.
    #[test]
    fn the_deposit_is_the_clients_arithmetic_truncation_and_all() {
        assert_eq!(deposit_for(5, 9, 1440), 0);
        // Once the inner floor clears, the duration scales it 1 / 4 / 12.
        assert_eq!(deposit_for(5, 10_000, 120), 500);
        assert_eq!(deposit_for(5, 10_000, 480), 2_000);
        assert_eq!(deposit_for(5, 10_000, 1440), 6_000);
        // Blackwater's 25% on the same stack.
        assert_eq!(deposit_for(25, 10_000, 120), 2_500);
        // Under the two-hour unit it floors to 0; the reference never sends such a duration.
        assert_eq!(deposit_for(5, 10_000, 60), 0);
    }

    /// Weapon and Armor, then Consumable (flag 0: no subclass rows, but the query scans them).
    fn tree() -> Vec<AuctionCategory> {
        let sub = |sub_id: u32, name: &str, has_inv_types: bool| AuctionSubCategory {
            sub_id,
            name: name.into(),
            has_inv_types,
        };
        vec![
            AuctionCategory {
                class_id: 2,
                name: "Weapon".into(),
                has_subclass_filter: true,
                subclasses: vec![sub(0, "Axe", false), sub(1, "Axe", false)],
            },
            AuctionCategory {
                class_id: 4,
                name: "Armor".into(),
                has_subclass_filter: true,
                subclasses: vec![sub(1, "Cloth", true), sub(6, "Shield", false)],
            },
            AuctionCategory {
                class_id: 0,
                name: "Consumable".into(),
                has_subclass_filter: false,
                subclasses: vec![sub(0, "Consumable", false)],
            },
        ]
    }

    #[test]
    fn query_sends_ids_not_menu_positions() {
        let mut s = UiScript::new().unwrap();
        s.set_auction_item_classes(tree());

        // Weapon / its first subclass / Back (the 13th inventory row).
        s.run(r#"QueryAuctionItems("", "", "", 13, 1, 1, 0, nil, -1)"#)
            .unwrap();
        let q = s.take_auction_query().expect("queued");
        assert_eq!(q.inv_type, Some(16), "INVTYPE_CLOAK is inventory type 16");
        assert_eq!(q.class, Some(2), "Weapon is item class 2, not position 1");
        assert_eq!(q.sub_class, Some(0), "the first weapon subclass is id 0");

        // Armor / its second subclass skips the id gap: Shield is 6, not 2.
        s.run(r#"QueryAuctionItems("", "", "", 14, 2, 2, 0, nil, -1)"#)
            .unwrap();
        let q = s.take_auction_query().expect("queued");
        assert_eq!(
            (q.inv_type, q.class, q.sub_class),
            (Some(23), Some(4), Some(6))
        );

        // The scan ignores the filter flag: Consumable offers no subclass rows, yet resolves one.
        s.run(r#"QueryAuctionItems("", "", "", 0, 3, 1, 0, nil, -1)"#)
            .unwrap();
        let q = s.take_auction_query().expect("queued");
        assert_eq!((q.inv_type, q.class, q.sub_class), (None, Some(0), Some(0)));

        // Out of range, or no class to scan under: the wire's "any".
        s.run(r#"QueryAuctionItems("", "", "", 15, 11, 1, 0, nil, -1)"#)
            .unwrap();
        let q = s.take_auction_query().expect("queued");
        assert_eq!((q.inv_type, q.class, q.sub_class), (None, None, None));
        s.run(r#"QueryAuctionItems("", "", "", nil, 1, 3, 0, nil, -1)"#)
            .unwrap();
        let q = s.take_auction_query().expect("queued");
        assert_eq!((q.inv_type, q.class, q.sub_class), (None, Some(2), None));
    }

    /// The gates of `0x4cf9c0` (subclasses) and `0x4cfab0` (inventory types).
    #[test]
    fn subclass_and_inv_type_menus_follow_the_reference_gates() {
        let mut s = UiScript::new().unwrap();
        s.set_auction_item_classes(tree());
        assert_eq!(s.arity("GetAuctionItemSubClasses(1)").unwrap(), 2);
        assert_eq!(
            s.arity("GetAuctionItemSubClasses(3)").unwrap(),
            0,
            "Consumable's flag is 0: no subclass rows"
        );
        assert_eq!(s.arity("GetAuctionInvTypes(2, 1)").unwrap(), 14, "Cloth");
        assert_eq!(s.arity("GetAuctionInvTypes(2, 2)").unwrap(), 0, "Shield");
        assert_eq!(s.arity("GetAuctionInvTypes(3, 1)").unwrap(), 0, "flag 0");
        assert_eq!(s.arity("GetAuctionInvTypes(11, 1)").unwrap(), 0, "no class");
        assert_eq!(
            s.arity("GetAuctionInvTypes(1, 9)").unwrap(),
            14,
            "an exhausted scan skips the gate"
        );
    }

    #[test]
    fn reversed_answers_for_any_remembered_key() {
        let list = AuctionListState {
            sort: vec![
                ("bid".into(), false),
                ("quality".into(), true),
                ("level".into(), false),
            ],
            ..Default::default()
        };
        assert!(!list.reversed("bid"), "the primary, not reversed");
        assert!(list.reversed("quality"), "remembered, and reversed");
        assert!(!list.reversed("level"));
        assert!(!list.reversed("seller"), "never sorted by: not reversed");
    }
}
