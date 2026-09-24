//! The auction-house bindings — the Era-shaped auctioneer surface, the same two-way
//! seam as [`super::merchant`]/[`super::mail`]: the app pushes an **auction snapshot** (three lists
//! of rows, already resolved from the wire to name/icon/quality/owner and already in display order)
//! and the Lua `QueryAuctionItems`/`PlaceAuctionBid`/`StartAuction`/… calls queue outbound
//! **intents** the app drains. The engine holds no auction knowledge: a row is a header, the sort
//! is a pushed ordering, and the category tree is a pushed table.
//!
//! ## Three lists, one API
//!
//! Every getter is keyed by a list type string — `"list"` (Browse), `"bidder"` (Bids) and
//! `"owner"` (Auctions) — and `index` is **1-based within the current 50-row batch**, not within
//! the whole result set. `GetNumAuctionItems` returns the pair `numBatchAuctions, totalAuctions`;
//! when `total > batch` the window is paging, and the page turners re-query rather than scroll.
//!
//! ## The two null shapes that are load-bearing
//!
//! `GetAuctionSellItemInfo()` **always returns six values, never zero** — an empty sell slot pushes
//! `nil, nil, 1, -1, nil, 0`, with a hard `count = 1` and `quality = -1` rather than nils. The
//! reference Lua reads that count unguarded (`if count > 1`), so a nil there throws the moment the
//! player pulls the item back out of the slot. `GetAuctionItemInfo` answers a lone `nil` for an
//! out-of-range index, which is the shape its callers test.
//!
//! ## Sorting is ours, and it is not a three-state cycle
//!
//! The wire carries no sort field, so the ordering is entirely client-side over the ≤50 rows the
//! server returned. Each list owns an **8-deep most-recently-clicked key stack** of
//! `(key, reversed)` pairs: clicking the column that is already primary toggles its direction;
//! clicking any other promotes it to primary **keeping the direction it remembers**. That is why
//! `IsAuctionSortReversed` can answer "reversed" for a column that is not currently sorting
//! anything. [`super::UiScript::take_auction_sorts`] hands the click to the app, which owns the
//! stacks and pushes both the reordered rows and the stack back. The selection rides through all
//! of it untouched, because it is stored as an auction **id** and only resolved to a row position
//! when something asks (`0x4cfda0`/`0x4cfec0`).
//!
//! ## The deposit is computed here, and it is the *client's* arithmetic
//!
//! `CalculateAuctionDeposit(minutes)` reproduces the real client's formula —
//! `floor(rate × stackValue / 100) × floor(minutes / 120)` — including the intermediate truncation
//! that makes it disagree with what the server actually charges on cheap items. Decision 1511 §7
//! records why the label follows the client rather than the server: it is a client artifact, and
//! benilla renders what the director's client renders.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{self, CursorPayload};
use super::Model;

/// The three list types, in the order the app pushes them. The wire calls them nothing at all —
/// this is purely the Lua API's own keying.
pub const LIST: usize = 0;
pub const BIDDER: usize = 1;
pub const OWNER: usize = 2;

/// The eight sort keys the reference's headers pass. Order here is the API's, not the comparator's
/// (decision 1511's INTERIM: the comparator's own mode order is pinned to an in-flight
/// investigation).
pub const SORT_KEYS: [&str; 8] = [
    "level", "quality", "bid", "duration", "buyout", "status", "name", "seller",
];

/// `"list"` / `"bidder"` / `"owner"` → the index the snapshot stores them at, for the other
/// modules that key off the same three names (the tooltip's `SetAuctionItem`).
pub(super) fn list_index_of(kind: &str) -> Option<usize> {
    list_index(kind)
}

/// `"list"` / `"bidder"` / `"owner"` → the index the snapshot stores them at.
fn list_index(kind: &str) -> Option<usize> {
    match kind {
        "list" => Some(LIST),
        "bidder" => Some(BIDDER),
        "owner" => Some(OWNER),
        _ => None,
    }
}

/// One auction row, resolved by the app from a wire record. Plain data — its
/// 1-based order in the window is its position in [`AuctionListState::rows`], which is already the
/// sorted display order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionItemRow {
    /// The wire auction id — what a bid or a cancel addresses. Never shown; the app maps a clicked
    /// row index back through this.
    pub auction_id: u32,
    /// The item's template entry — the tooltip store's key and the link's identity.
    pub item_id: u32,
    /// The listed item's **random-suffix roll** (the wire's `randomPropertyId`) — what the row
    /// hover resolves its enchant lines from, and the id whose suffix [`Self::name`] already
    /// carries. `0` = unrolled. The reference's `SetAuctionItem` writes exactly this into the
    /// tooltip's `+0x424`.
    pub random_property_id: u32,
    /// `None` while the item template answer is in flight (the row shows a placeholder and fills
    /// in when it lands, the merchant/mail pattern).
    pub name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    pub quality: Option<u32>,
    /// `RequiredLevel` — printed red when it exceeds the player's own level.
    pub level: u32,
    /// The seller's opening price. This is what the row shows as the current bid while
    /// [`Self::bid_amount`] is 0.
    pub min_bid: u32,
    /// The minimum step above the current bid — `0` while nobody has bid.
    pub min_increment: u32,
    /// `0` = no buyout.
    pub buyout_price: u32,
    /// The current high bid; `0` = no bids yet.
    pub bid_amount: u32,
    /// Whether *the player* holds the high bid — a flag, not a name. The Browse row's "Your bid:"
    /// caption and both action-button gates read it.
    pub high_bidder: bool,
    /// The seller's name; `None` while the name query is in flight.
    pub owner: Option<String>,
    /// The time-left bucket, `1..=4` (Short/Medium/Long/Very Long); `0` = not known yet.
    pub time_left: u32,
    /// The full item link, built app-side from the row's entry + enchant + random-property +
    /// suffix (auction rows carry all four on the wire, so the link is the complete one).
    pub link: Option<String>,
}

/// One of the three lists: the batch the server sent, the pre-cap match count, and the sort stack
/// that ordered it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionListState {
    /// The current batch, already in display order (≤50 — the server's own page cap).
    pub rows: Vec<AuctionItemRow>,
    /// `totalAuctions` — how many matched *before* the 50-row cap, so the pager can say
    /// "( %d total )". Equal to `rows.len()` when everything fits on one page.
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

/// One row of the Browse tab's category tree, pushed by the app from the player's own
/// `ItemClass.dbc` / `ItemSubClass.dbc` (the set and order of classes is a
/// structural fact; every string here comes off the player's install).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionCategory {
    /// The wire's `mainCategory` — an item class id, **not** the menu position.
    pub class_id: u32,
    pub name: String,
    /// The `0x807060` table's second column: whether the class offers subclass rows at all.
    /// `false` (Consumable, Trade Goods, Reagent, Miscellaneous) makes `GetAuctionItemSubClasses`
    /// and `GetAuctionInvTypes` answer nothing — but `QueryAuctionItems` never reads it, so
    /// [`Self::subclasses`] is still the class's full non-excluded list either way.
    pub has_subclass_filter: bool,
    /// The class's `ItemSubClass.dbc` rows whose `DisplayFlags & 2` is clear, in file order —
    /// the list both the menu and the query's index scan walk.
    pub subclasses: Vec<AuctionSubCategory>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionSubCategory {
    /// The wire's `subCategory`.
    pub sub_id: u32,
    pub name: String,
    /// Whether selecting this subclass offers the 14 inventory-slot rows beneath it
    /// (`GetAuctionInvTypes`) — the row's `ItemSubClass.Flags & 0x200` (`0x4cfb63`), read
    /// app-side off the player's own DBC.
    pub has_inv_types: bool,
}

/// The open auction house's snapshot. Pushed whole by the app; `None` means no auctioneer session
/// is open (the window is closed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionState {
    /// Browse / Bids / Auctions, indexed by [`LIST`], [`BIDDER`], [`OWNER`].
    pub lists: [AuctionListState; 3],
    /// `GetAuctionHouseDepositRate()` — the percentage this house charges, read from
    /// `AuctionHouse.dbc` at the `houseId` the hello reply carried.
    pub deposit_percent: u32,
}

/// A drained `QueryAuctionItems` — the Browse search, exactly as the reference builds it: every
/// filter is already the wire's item-template id (the binding maps the Lua's menu positions, as
/// `0x4ce980` does). The app copies this into `CMSG_AUCTION_LIST_ITEMS` (decision 1511 P1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionQuery {
    pub name: String,
    /// `0` = no filter, for both.
    pub min_level: u32,
    pub max_level: u32,
    /// `None` = no filter (the wire's `0xFFFFFFFF` sentinel). Ids, never menu positions: an
    /// `InventoryType`, an item class, a subclass of that class.
    pub inv_type: Option<u32>,
    pub class: Option<u32>,
    pub sub_class: Option<u32>,
    /// A **minimum** quality, not an equality. `None` = no filter (the dropdown's `All`, −1).
    pub quality: Option<u32>,
    /// The page, 0-based. The app multiplies by 50 for the wire's item offset.
    pub page: u32,
    pub usable_only: bool,
}

/// A drained `StartAuction` (decision 1511 P2).
#[derive(Clone, Debug, PartialEq)]
pub struct AuctionStartRequest {
    pub min_bid: u32,
    /// `0` = no buyout.
    pub buyout: u32,
    /// Minutes — only 120, 480 or 1440 are legal, and the reference refuses to send anything else.
    pub duration: u32,
}

/// A drained `PlaceAuctionBid`.
#[derive(Clone, Debug, PartialEq)]
pub struct AuctionBid {
    /// The list the index addresses ([`LIST`] or [`BIDDER`] — the Auctions tab has no bid button).
    pub list: usize,
    /// 1-based row within the current batch; the app maps it to the wire auction id.
    pub index: u32,
    pub amount: u32,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open auction house's snapshot. Replacing a list's rows
    /// **clears that list's selection**, which is why this is one call and not two: the reference's
    /// selection is C-side state that cannot survive the batch it indexes into, and a stale index
    /// would silently address a different auction.
    /// Push the Browse tab's class tree — `GetAuctionItemClasses` and its two siblings read it.
    /// Login-scoped, not the session's: the stock `AuctionFrameBrowse_OnLoad` reads the classes at
    /// the addon's LOAD, before any auctioneer, and in the client they are `ItemClass.dbc` names
    /// with no session behind them (1971).
    pub fn set_auction_item_classes(&mut self, classes: Vec<AuctionCategory>) {
        self.model_mut().auction_item_classes = classes;
    }

    pub fn set_auction(&mut self, state: Option<AuctionState>) {
        let mut model = self.model_mut();
        // Only opening or closing the session drops the selection. A new PAGE does not, and
        // neither does a re-sort: the selection is an auction **id**, so it survives the row
        // moving and simply stops resolving if the auction leaves the page (`0x4cfda0`/`0x4cfec0`).
        if model.auction.is_none() || state.is_none() {
            model.auction_selected = [0; 3];
        }
        model.auction = state;
    }

    /// Push `CanSendAuctionQuery()`'s answer — the app's throttle, read by the Search button's
    /// every-frame `OnUpdate`. Separate from the snapshot precisely because it changes every frame
    /// while throttled and would otherwise churn the snapshot's diff.
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

    /// The item currently in the sell slot — the app resolves its `(bag, slot)` to the wire item
    /// guid when `StartAuction` fires (the lazy resolve mail's send uses).
    pub fn auction_sell_item(&mut self) -> Option<(i64, u32)> {
        self.model_mut()
            .auction_sell_item
            .as_ref()
            .map(|it| (it.bag, it.slot))
    }

    /// Empty the sell slot — the app calls this once the auction is away, and on session close.
    pub fn clear_auction_sell_item(&mut self) {
        self.model_mut().auction_sell_item = None;
    }
}

/// Fetch a cloned row for a list type + 1-based index, or `None` (unknown type / out of range /
/// no session open).
fn row_at(model: &Model, kind: &str, index: usize) -> Option<AuctionItemRow> {
    let i = list_index(kind)?;
    model
        .auction
        .as_ref()
        .and_then(|a| index.checked_sub(1).and_then(|n| a.lists[i].rows.get(n)))
        .cloned()
}

/// The sell slot's `(name, texture, count, quality, canUse, stackValue)`, or `None` when empty.
/// `stackValue` is the item's vendor sell price **times the stack**, which is what the reference's
/// suggested opening price and the deposit are both computed from.
fn sell_item_info(model: &Model) -> Option<(String, Option<String>, u32, i64, bool, u32)> {
    let it = model.auction_sell_item.as_ref()?;
    // The TRUE stack size, not the split-carry field — the deposit is per stack.
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

/// The real client's deposit arithmetic, reproduced including its intermediate truncation:
/// `floor(rate × stackValue / 100) × floor(minutes / 120)`. Both floors matter
/// — the inner one is what makes a cheap stack deposit zero here while the server still charges a
/// few copper, and the outer is why 120/480/1440 minutes scale as 1/4/12 rather than continuously.
fn deposit_for(rate: u32, stack_value: u32, minutes: u32) -> u32 {
    let per_unit = u64::from(rate).saturating_mul(u64::from(stack_value)) / 100;
    let units = u64::from(minutes) / 120;
    u32::try_from(per_unit.saturating_mul(units)).unwrap_or(u32::MAX)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumAuctionItems(type) → numBatchAuctions, totalAuctions. The pair is the whole paging
    // model: batch is what this packet carried (≤50), total is what matched before the cap.
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

    // GetAuctionItemInfo(type, index) → the 12-tuple. `highBidder` is a FLAG, not a name — the
    // seller's name is the last return, and it is what the Browse "Seller" column shows.
    g.set(
        "GetAuctionItemInfo",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                row_at(&model, &kind, index)
            };
            let Some(r) = row else {
                // The null tail is TWELVE values, not a lone nil — an unknown type string, an
                // out-of-range index and an item-cache miss all share it, with a hard `count = 1`
                // and `quality = -1` (one shared exit at `0x4cf1ec`). Callers
                // destructure all twelve unguarded, so a short return throws.
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
            // `canUse` is computed HERE rather than pushed with the row: it depends on the
            // player's own level, class and spellbook, which move independently of the auction
            // snapshot. A pushed flag would go stale the moment the player dinged mid-browse.
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

    // GetAuctionItemTimeLeft(type, index) → 1..4, the bucket the row's text and tooltip key off.
    // 0 for a row we have no answer for yet; the reference has no live countdown (its ticking
    // version is commented out), so this only moves when a fresh list lands.
    g.set(
        "GetAuctionItemTimeLeft",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(row_at(&model, &kind, index).map_or(0, |r| i64::from(r.time_left)))
        })?,
    )?;

    // GetAuctionItemLink(type, index) → the full item link. Auction rows carry enchant, random
    // property and suffix on the wire, so the app builds the complete link rather than the
    // zeroed-tail one the bag path settles for.
    g.set(
        "GetAuctionItemLink",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                row_at(&model, &kind, index).and_then(|r| r.link)
            };
            // A miss returns NO values at all — not nil (`0x4cf45b`). `DressUpItemLink(nil)`
            // and `nil` reaching a chat insert behave differently from an empty argument list.
            Ok(match link {
                Some(l) => MultiValue::from_vec(vec![Value::String(lua.create_string(&l)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetSelectedAuctionItem(type) → the 1-based selected row, or 0 for none.
    //
    // Stored as the **auction id**, resolved to a row position on the way out
    // (`0x4cfda0`/`0x4cfec0`).
    // That indirection is the whole reason a re-sort cannot silently move the selection onto a
    // different auction: the id follows the row wherever the comparator puts it, and an auction
    // that has left the page simply stops resolving instead of pointing at its neighbour.
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

    // SetSelectedAuctionItem(type, index) — takes a row position and remembers the auction it
    // names. An out-of-range index clears the selection rather than remembering a phantom.
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

    // SortAuctionItems(type, key) — queue the header click; the app owns the stacks and pushes the
    // reordered rows back. The reference raises on a bad type, and that Usage string is recorded,
    // so it is reproduced here rather than swallowed.
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

    // IsAuctionSortReversed(type, key) → 1/nil. Answers for ANY key the stack remembers, not just
    // the primary one — which is the whole reason a non-primary column's arrow can point down.
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

    // CanSendAuctionQuery() → 1/nil. Polled every frame by the Search button's OnUpdate, because
    // a throttled query is dropped silently and there is no failure event to react to.
    g.set(
        "CanSendAuctionQuery",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.auction_can_query))
        })?,
    )?;

    // GetAuctionItemClasses() → the class names, in the reference's own menu order. The ORDER and
    // the set are structural (ten auctionable classes); every string is the player's own
    // ItemClass.dbc row, so none of Blizzard's text ships with us. Static —
    // pushed at login, read by the stock addon at its load (1971), no session required.
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

    // GetAuctionItemSubClasses(classIndex) → the subclass names under a 1-based menu class. An
    // out-of-range index, or a class whose `0x807060` flag is 0, returns nothing — the
    // reference's own bound-with-no-error (`0x4cfa00`, `0x4cfa0e`).
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

    // GetAuctionInvTypes(classIndex, subclassIndex) → the 14 inventory-slot GlobalString KEYS, or
    // nothing. The pair only decides WHETHER the list is offered — the list itself is fixed.
    // `0x4cfab0`: no class (index out of range, or its `0x807060` flag 0) → nothing; a FOUND
    // subclass offers the list only with `ItemSubClass.Flags & 0x200` (pushed as
    // `has_inv_types`); and a subclass index the scan runs off the end of skips that gate and
    // offers all fourteen (the exhaust edge at `0x4cfb61` lands past the gate at `0x4cfb63`).
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
    // isUsable, qualityIndex) — nine arguments (TBC's tenth, exactMatch, does not exist on 5875).
    // The level boxes hand us strings. Every filter argument is a 1-based MENU POSITION, and —
    // exactly as the reference's `0x4ce980` does — it is turned into the wire's item-template id
    // here, before anything is queued: invType through the `0x8070b0` rows, class through the
    // `0x807060` table, subclass as the Nth non-excluded subclass row of that class (a scan that
    // never reads the class's filter flag). Anything that does not resolve is the wire's "any".
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
            // A 1-based position into a table, `None` for a missing, zero or negative argument.
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

    // GetOwnerAuctionItems([page]) / GetBidderAuctionItems([page]) — the other two tabs' fetches.
    // Neither is throttled the way Browse is; both default to page 0.
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

    // PlaceAuctionBid(type, index, bidAmount). A buyout is not a distinct verb — it is a bid of
    // exactly the buyout price, which is why the confirmation popup calls straight into this.
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

    // StartAuction(minBid, buyoutPrice, runTime) — runTime in minutes.
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

    // CancelAuction(index) — always an "owner" row.
    g.set(
        "CancelAuction",
        lua.create_function(|lua, index: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.auction_cancels.push(index);
            Ok(())
        })?,
    )?;

    // GetAuctionHouseDepositRate() → the percentage, off AuctionHouse.dbc at this session's house.
    // The shipped reference addon never calls it; third-party addons may, so we ship it.
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

    // CalculateAuctionDeposit(runTime) → copper. See `deposit_for` — this is the client's own
    // arithmetic, truncation included, not the server's charge.
    g.set(
        "CalculateAuctionDeposit",
        lua.create_function(|lua, minutes: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let rate = model.auction.as_ref().map_or(0, |a| a.deposit_percent);
            let stack_value = sell_item_info(&model).map_or(0, |(_, _, _, _, _, v)| v);
            Ok(i64::from(deposit_for(rate, stack_value, minutes)))
        })?,
    )?;

    // GetAuctionSellItemInfo() → name, texture, count, quality, canUse, price. SIX values ALWAYS —
    // the empty slot pushes a hard count of 1 and a quality of -1, never nils, because the
    // reference Lua reads that count unguarded the moment the slot empties.
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

    // ClickAuctionSellItemButton() — attach the cursor's item, or take back the attached one. The
    // mail send slot's exact shape (decision 0216's rails): every cursor write is followed by both
    // queues, which is the law that keeps CURSOR_UPDATE and ITEM_LOCK_CHANGED honest.
    g.set(
        "ClickAuctionSellItemButton",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            click_auction_sell_item(&mut model);
            Ok(())
        })?,
    )?;

    // CloseAuctionHouse() — the client-side close. Vanilla sends nothing; the session simply ends.
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

/// The fourteen inventory-slot rows the reference offers — the `0x8070b0` table's
/// `{u32 invTypeId; char name[0x20]}` rows, in its own order. The name is a GlobalString **key**
/// the Lua resolves through the player's own strings, so no English ships here; the id is what
/// `QueryAuctionItems` puts on the wire for the row's 1-based position.
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

/// Attach the cursor's held item to the sell slot, or — with an empty cursor — pick the attached
/// one back up. A spell or action payload is refused and put back untouched, exactly as the mail
/// send slot refuses it.
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

    /// The selection is an auction id, not a row position — so a re-sort moves the row under it
    /// and the selection follows, rather than coming to mean whatever took that slot.
    ///
    /// This is the case an index would get silently wrong: pick row 2, re-sort, and an
    /// index-based selection is now pointing at a *different auction* that the Bid button would
    /// happily bid on.
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

        // It leaves the page entirely (outbid, bought, or filtered out by a new search).
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

    /// A WHOLE-STACK pickup carries no count of its own, so the sell slot has to read the real
    /// stack size back off the source slot. Reading the cursor field directly answers 1 for a
    /// stack of twenty — and since the deposit is charged per stack, the pane would quote a
    /// twentieth of what the server then takes.
    #[test]
    fn the_sell_slot_reads_a_whole_stacks_real_size() {
        use crate::script::container::{ContainerSlot, ContainerState};
        use crate::script::cursor::CursorItem;

        let mut s = UiScript::new().unwrap();
        // Twenty of item 2589 sitting in backpack slot 1.
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
        // Picked up whole — `count: None` is the "whole stack" signal, not "one".
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

    /// The null tail is twelve values with a hard `count = 1` and `quality = -1`. The reference
    /// Lua destructures all twelve unguarded, so a short return throws rather than showing an
    /// empty row.
    #[test]
    fn a_missing_row_still_answers_twelve_values() {
        let s = UiScript::new().unwrap();
        let n = s.arity(r#"GetAuctionItemInfo("list", 99)"#).unwrap();
        assert_eq!(n, 12, "twelve, even with no session open at all");
        let (count, quality): (i64, i64) = s
            .eval(r#"local _, _, c, q = GetAuctionItemInfo("list", 99) return c, q"#)
            .unwrap();
        assert_eq!((count, quality), (1, -1));

        // The link, by contrast, answers with NO values on a miss — not a nil.
        let n = s.arity(r#"GetAuctionItemLink("list", 99)"#).unwrap();
        assert_eq!(n, 0, "zero values, not one nil");
    }

    /// The client's own deposit arithmetic, including the intermediate truncation that makes it
    /// disagree with the server. The cheap-stack case is the one that matters:
    /// a 9-copper vendor value at 5% over 24h floors to 0 here while vmangos still charges 5, and
    /// that divergence is deliberate — the label is a client artifact.
    #[test]
    fn the_deposit_is_the_clients_arithmetic_truncation_and_all() {
        // 5% of 9c = 0 after the inner floor, so no duration can lift it off zero.
        assert_eq!(deposit_for(5, 9, 1440), 0);
        // Once the inner floor clears, the duration scales it 1 / 4 / 12.
        assert_eq!(deposit_for(5, 10_000, 120), 500);
        assert_eq!(deposit_for(5, 10_000, 480), 2_000);
        assert_eq!(deposit_for(5, 10_000, 1440), 6_000);
        // Blackwater's 25% on the same stack.
        assert_eq!(deposit_for(25, 10_000, 120), 2_500);
        // A duration under the two-hour unit floors the whole thing away — the reference refuses
        // to send such a duration at all, so this is a shape check, not a reachable state.
        assert_eq!(deposit_for(5, 10_000, 60), 0);
    }

    /// The browse tree as the reference's `0x807060` table shapes it: Weapon (filterable) first,
    /// then Armor, then Consumable (flag 0 — no subclass rows offered, but its subclasses still
    /// exist for the query's own scan).
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

    /// `QueryAuctionItems` takes menu POSITIONS and puts item-template IDS on the wire — the
    /// reference does the mapping itself (`0x4ce980`: invType through `0x8070b0`, class through
    /// `0x807060`, subclass as the Nth non-excluded `ItemSubClass.dbc` row of that class). Sending
    /// the position instead asked vmangos for class 1 (Container) when the player picked Weapon.
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

        // Armor / its SECOND subclass skips the id gap: Shield is 6, not 2.
        s.run(r#"QueryAuctionItems("", "", "", 14, 2, 2, 0, nil, -1)"#)
            .unwrap();
        let q = s.take_auction_query().expect("queued");
        assert_eq!(
            (q.inv_type, q.class, q.sub_class),
            (Some(23), Some(4), Some(6))
        );

        // The query's scan ignores the class's filter flag: Consumable offers no subclass rows,
        // yet an index still resolves against its (non-excluded) subclasses.
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

    /// `0x4cf9c0` / `0x4cfab0` return nothing for a class whose `0x807060` flag is 0, and
    /// `0x4cfab0` gates a FOUND subclass on `ItemSubClass.Flags & 0x200` but pushes all fourteen
    /// when the scan runs off the end (the exhaust edge at `0x4cfb61` lands past the gate at
    /// `0x4cfb63`).
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

    /// The sort stack answers for any key it remembers, not just the primary — the reason a
    /// non-primary column's arrow can still point down.
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
