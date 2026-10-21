//! The auction-house bindings: the app pushes a snapshot of three lists, rows resolved and in
//! wire order, and the Lua calls queue intents the app drains. Getters are keyed by `"list"`
//! (Browse), `"bidder"` (Bids) or `"owner"` (Auctions), with `index` 1-based in the 50-row batch.
//!
//! The order is the VM's: a list is sorted where the reference sorts it, by its key stack
//! ([`AuctionSortStack`]), in `SortAuctionItems` and in the tail the app runs once a result's
//! items are all known ([`super::UiScript::sort_auction_list`]), and holds still in between.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{self, flag};
use super::cursor;
use super::Model;
use crate::auction_sort::{floor_level, sort_column, AuctionSortStack};

/// The three list types, in the order the app pushes them.
pub const LIST: usize = 0;
pub const BIDDER: usize = 1;
pub const OWNER: usize = 2;

/// The eight sort columns by number, as `SortAuctionItems` names them (`0x4cfc4b`-`0x4cfd1d`)
/// and the comparator `0x4cd940` numbers its modes: level 0, quality 1, bid 2, duration 3,
/// buyout 4, status 5, name 6, seller 7.
pub const SORT_KEYS: [&str; 8] = [
    "level", "quality", "bid", "duration", "buyout", "status", "name", "seller",
];

/// A list type's snapshot index, for the other modules (the tooltip's `SetAuctionItem`).
pub(super) fn list_index_of(kind: &str) -> Option<usize> {
    list_index(kind)
}

/// `"owner"`, `"bidder"` or `"list"`, matched without case in that order (`SStrCmpI`,
/// `0x4ced6b`, `0x4cedb0`, `0x4cedf4`), by every function that takes a list type.
fn list_index(kind: &str) -> Option<usize> {
    [("owner", OWNER), ("bidder", BIDDER), ("list", LIST)]
        .into_iter()
        .find(|(name, _)| crate::crt::str_cmp_i(name, kind) == 0)
        .map(|(_, i)| i)
}

/// `SortAuctionItems`' and `IsAuctionSortReversed`' two arguments: `lua_isstring` on each, a number
/// read as its text (`0x6f3510`, `0x6f3690`), else the usage line.
fn type_and_sort(
    lua: &Lua,
    kind: Value,
    sort: Value,
    usage: &'static str,
) -> mlua::Result<(String, String)> {
    Ok((
        binding_abi::string_arg(lua, kind, usage)?,
        binding_abi::string_arg(lua, sort, usage)?,
    ))
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
    /// The wire's spell charges, which `SetAuctionItem` writes into the tooltip's `+0x428`
    /// (`0x535a4a`).
    pub charges: i32,
    /// `None` while the item template query is in flight.
    pub name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    /// `None` while the item template query is in flight.
    pub quality: Option<u32>,
    /// The template's `RequiredLevel` (`+0x3c`), 0 for none; `GetAuctionItemInfo` and the level
    /// sort read it floored at 1.
    pub level: u32,
    /// The seller's opening price, shown as the current bid while [`Self::bid_amount`] is 0.
    pub min_bid: u32,
    /// The minimum step above the current bid; 0 before any bid.
    pub min_increment: u32,
    /// `0` = no buyout.
    pub buyout_price: u32,
    /// The current high bid; 0 before any bid.
    pub bid_amount: u32,
    /// `GetAuctionItemInfo`'s eleventh value, which the app picks per list and row.
    pub high_bidder: AuctionHighBidder,
    /// The seller's name; `None` while the name query is in flight.
    pub owner: Option<String>,
    /// The auction's deadline in milliseconds on `GetTime`'s clock ([`UiScript::auction_tick_ms`]):
    /// the tick its list was read at plus the wire's milliseconds left, wrapping (`0x4cc865`,
    /// `0x4cc966`-`0x4cc975`). `GetAuctionItemTimeLeft` reads it against the clock at the call.
    ///
    /// [`UiScript::auction_tick_ms`]: super::UiScript::auction_tick_ms
    pub deadline: u32,
    /// The high bidder's rank (`0x4cdb10`-`0x4cdb6a`): 2 the player, 1 another bidder, 0 no bid.
    pub bidder_rank: u8,
    /// The item template's own name (`[rec+8]`), without the random suffix [`Self::name`]
    /// carries; `None` while the template query is in flight.
    pub base_name: Option<String>,
    /// The full item link: auction rows carry entry, enchant, random property and suffix.
    pub link: Option<String>,
}

/// `GetAuctionItemInfo`'s `highBidder` (`0x4cf12a`–`0x4cf1a0`): on an `"owner"` row the player
/// owns, the bidder's name from the name cache; on any other row, `1` when the player holds the
/// bid. Nil otherwise, which the stock Auctions tab prints as `NO_BIDS`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum AuctionHighBidder {
    /// No bid, another player's bid on a row the player does not own, or a name not arrived.
    #[default]
    Nil,
    /// The player holds the bid on a row it does not own: `1`.
    Player,
    /// The bidder on the player's own auction, by name.
    Name(String),
}

/// One of the three lists: the batch the server sent, the pre-cap match count and its sort stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionListState {
    /// The current batch, at most 50 (the server's page cap): in wire order as the app pushes it,
    /// in display order once the VM holds it.
    pub rows: Vec<AuctionItemRow>,
    /// `totalAuctions`: the match count before the 50-row cap.
    pub total: u32,
    /// The list's key stack. The VM takes the app's copy at its first push and keeps its own
    /// after it, which the header clicks promote in place.
    pub sort: AuctionSortStack,
    /// Which result the rows came from, counted by the app across every list: a new one is laid
    /// in wire order, as the result handler writes its records (`0x4cc880`), and one the VM holds
    /// keeps the VM's order.
    pub page: u32,
    /// The list's sort tail is still owed: its pending-lookups counter is not back at 0
    /// (`0xb72644`, `0xb72648`, `0xb7264c`), so a removal from it signals nothing (`0x4ce035`).
    pub pending: bool,
    /// The auctions a time-left read removed from this page ([`remove_auction`]), which a push of
    /// the same page leaves out. The VM's own: a push's value is not read.
    pub removed: Vec<u32>,
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

/// The auction house's snapshot, pushed whole by the app: the three lists, which outlive the
/// session as the reference's arrays do, and the session itself.
#[derive(Clone, Debug, PartialEq)]
pub struct AuctionState {
    /// Browse / Bids / Auctions, indexed by [`LIST`], [`BIDDER`], [`OWNER`].
    pub lists: [AuctionListState; 3],
    /// `GetAuctionHouseDepositRate()`: the house's `AuctionHouse.dbc` deposit percentage.
    pub deposit_percent: u32,
    /// An auctioneer session is open (`[0xb725f8]` nonzero): `StartAuction` sends only then
    /// (`0x4ce7da`).
    pub open: bool,
}

impl Default for AuctionState {
    /// Three empty lists, each with its world-entry stack, and no session.
    fn default() -> Self {
        Self {
            lists: AuctionSortStack::seeded_all().map(|sort| AuctionListState {
                sort,
                ..Default::default()
            }),
            deposit_percent: 0,
            open: false,
        }
    }
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

/// A drained `StartAuction`: the sell slot's item as the attach stored it, which the verb then
/// emptied.
#[derive(Clone, Debug, PartialEq)]
pub struct AuctionStartRequest {
    /// The item's guid as the attach found it (`[0xb72608]`, sent at `0x4ce8f3`).
    pub item_guid: u64,
    pub min_bid: u32,
    /// `0` = no buyout; else at least the minimum bid (`0x4ce87b`-`0x4ce883`).
    pub buyout: u32,
    /// Minutes: 120, 480 or 1440, the only values the verb sends (`0x4ce8a0`).
    pub duration: u32,
}

/// The run times `StartAuction` accepts, in minutes (`0x807028`).
const RUN_TIMES: [u32; 3] = [120, 480, 1440];

/// A drained `PlaceAuctionBid`.
#[derive(Clone, Debug, PartialEq)]
pub struct AuctionBid {
    /// [`LIST`] or [`BIDDER`]: the Auctions tab has no bid button.
    pub list: usize,
    /// The auction at the bid's row when the call was made, as the verb reads it (`0x4cf610`).
    pub auction_id: u32,
    pub amount: u32,
}

impl super::UiScript {
    /// Push the Browse tab's class tree. Login-scoped: the stock `AuctionFrameBrowse_OnLoad` reads
    /// the classes when the addon loads, before any auctioneer session.
    pub fn set_auction_item_classes(&mut self, classes: Vec<AuctionCategory>) {
        self.model_mut().auction_item_classes = classes;
    }

    /// Push the lists and the session. The VM's first push takes the app's key stacks and every
    /// later one keeps the VM's; a list pushed with the page it holds keeps the VM's order and
    /// leaves out what a time-left read removed. The selection is an auction id no push writes
    /// (`0x849058`): it stops resolving when its auction leaves the list.
    pub fn set_auction(&mut self, state: AuctionState) {
        let mut model = self.model_mut();
        model.auction = match (model.auction.take(), state) {
            (Some(held), mut state) => {
                for (list, held) in state.lists.iter_mut().zip(held.lists) {
                    list.sort = held.sort;
                    list.removed = Vec::new();
                    if list.page == held.page {
                        list.removed = held.removed;
                        let removed = &list.removed;
                        list.rows.retain(|r| !removed.contains(&r.auction_id));
                        list.rows = in_held_order(&held.rows, std::mem::take(&mut list.rows));
                    }
                }
                Some(state)
            }
            (None, state) => Some(state),
        };
    }

    /// The close's half the VM holds (`0x4cd613`-`0x4cd61f`): the session and its house go; the
    /// lists, their stacks and the selection stand, and the getters still answer them.
    pub fn close_auction_session(&mut self) {
        if let Some(a) = self.model_mut().auction.as_mut() {
            a.open = false;
            a.deposit_percent = 0;
        }
    }

    /// The world-enter cascade's reset of the selection (`0x4cc2d5`), the one writer of the cells
    /// besides `SetSelectedAuctionItem`.
    pub fn clear_auction_selections(&mut self) {
        self.model_mut().auction_selected = [0; 3];
    }

    /// A list's sort tail, run by the app when the last of a result's items is known (`0x4cc9f1`,
    /// `0x4cca21`): the list sorted by its stack and its update signalled.
    pub fn sort_auction_list(&mut self, list: usize) {
        super::tick::verb(&self.lua, |model| sort_tail(model, list));
    }

    /// The 1-based row the auction `auction_id` shows at in `list`, for an instrument that clicks
    /// it.
    pub fn auction_display_index(&self, list: usize, auction_id: u32) -> Option<u32> {
        let model = self.model_ref();
        let rows = &model.auction.as_ref()?.lists.get(list)?.rows;
        rows.iter()
            .position(|r| r.auction_id == auction_id)
            .map(|i| i as u32 + 1)
    }

    /// The millisecond tick a list result is read at (`0x42c010` at `0x4cc865`), on `GetTime`'s
    /// clock: the one `GetAuctionItemTimeLeft` reads at its call (`0x4cf548`), so each row's
    /// deadline is this plus the wire's milliseconds left.
    pub fn auction_tick_ms(&self) -> u32 {
        tick_ms(self.model_ref().now)
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

    /// Drain the auctions `CancelAuction` was called on, each read at its row when called.
    pub fn take_auction_cancels(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().auction_cancels)
    }

    /// Drain the `StartAuction`s sent since the last drain, for `CMSG_AUCTION_SELL_ITEM`.
    pub fn take_auction_starts(&mut self) -> Vec<AuctionStartRequest> {
        std::mem::take(&mut self.model_mut().auction_starts)
    }

    /// Drain the header clicks: `(list index, sort key)`, in click order, which the app replays
    /// onto its own copy of the stacks, the one that outlives the session and the VM.
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

    /// Empty the sell slot at the close and the world-enter reset: the sell-slot clear
    /// (`0x4cd640`), as [`clear_sell_slot`] runs it.
    pub fn clear_auction_sell_item(&mut self) {
        super::tick::verb(&self.lua, clear_sell_slot);
    }
}

/// The sell-slot clear (`0x4cd640`): it lifts the attached item's lock with its
/// `ITEM_LOCK_CHANGED` (`0x495420` at `0x4cd653`), empties the slot and then signals
/// `NEW_AUCTION_UPDATE` (`0x4cd678`); an empty slot signals nothing (`0x4cd64f`).
pub(crate) fn clear_sell_slot(model: &mut Model) {
    if let Some(item) = model.auction_sell_item.take() {
        super::cursor::unlock_item(model, item.guid);
        model.raise("NEW_AUCTION_UPDATE", Vec::new());
    }
}

/// The error `ClickAuctionSellItemButton` refuses a held item with (`0x4ce360`-`0x4ce4f1`), read
/// off the item and its record as the reference reads its object; none when the item is gone, as
/// an object the cursor's guid no longer finds goes straight to the attach (`0x4ce35a`). In order:
/// a bound item (`0x5da2c0`) is `ERR_AUCTION_QUEST_ITEM` for quest bonding 4 or 5 (`[rec+0x194]`)
/// and `ERR_AUCTION_BOUND_ITEM` otherwise, a missing record included (`0x4ce38d`); record flags
/// `0x2` or `0x10000` (`0x4ce3e2`, `0x4ce3ec`), `ITEM_FIELD_DURATION` (`0x4ce411`), an area
/// (`0x5da500`) or a map (`0x5da570`) are `ERR_AUCTION_CONJURED_ITEM`; a container with an item in
/// a seat is `ERR_AUCTION_BAG` (`0x4ce45e`-`0x4ce48f`); `ITEM_FIELD_FLAGS & 8` is
/// `ERR_AUCTION_WRAPPED_ITEM` (`0x4ce4b1`); record flags `& 4` are `ERR_AUCTION_LOOT_ITEM`
/// (`0x4ce4d6`). The record is the item cache's, read with no callback (`0x55ba30`); without one
/// its flags read 0 (`0x5da2b3`).
fn sell_refusal(model: &Model, item: &cursor::CursorItem) -> Option<&'static str> {
    let (bound, flags, duration, seats) = if item.bag == cursor::EQUIPMENT_BAG {
        let s = model
            .inv_slot("player", item.slot as usize)
            .filter(|s| s.guid == item.guid)?;
        (s.already_bound, s.flags, s.field_duration, s.occupied_seats)
    } else {
        let s = model
            .containers
            .get(&item.bag)?
            .slots
            .get(&item.slot)
            .filter(|s| s.guid == item.guid)?;
        (s.already_bound, s.flags, s.field_duration, s.occupied_seats)
    };
    let record = model.item_templates.get(&item.item_id);
    /// `BIND_QUEST_ITEM` and `BIND_QUEST_ITEM1`.
    const QUEST_BONDING: [u32; 2] = [4, 5];
    if bound {
        return Some(
            if record.is_some_and(|r| QUEST_BONDING.contains(&r.bonding)) {
                "ERR_AUCTION_QUEST_ITEM"
            } else {
                "ERR_AUCTION_BOUND_ITEM"
            },
        );
    }
    let record_flags = record.map_or(0, |r| r.flags);
    if record_flags & (0x2 | 0x10000) != 0
        || duration != 0
        || record.is_some_and(|r| r.area != 0 || r.map != 0)
    {
        return Some("ERR_AUCTION_CONJURED_ITEM");
    }
    if seats.is_some_and(|n| n > 0) {
        return Some("ERR_AUCTION_BAG");
    }
    if flags & 0x8 != 0 {
        return Some("ERR_AUCTION_WRAPPED_ITEM");
    }
    (record_flags & 0x4 != 0).then_some("ERR_AUCTION_LOOT_ITEM")
}

/// `pushed`, a page the VM already holds, in the order of `held`, its rows matched by auction id;
/// a page whose ids differ is taken as pushed.
fn in_held_order(held: &[AuctionItemRow], pushed: Vec<AuctionItemRow>) -> Vec<AuctionItemRow> {
    if held.len() != pushed.len() {
        return pushed;
    }
    let mut used = vec![false; pushed.len()];
    let mut order = Vec::with_capacity(held.len());
    for h in held {
        match (0..pushed.len()).find(|&k| !used[k] && pushed[k].auction_id == h.auction_id) {
            Some(k) => {
                used[k] = true;
                order.push(k);
            }
            None => return pushed,
        }
    }
    let mut rows: Vec<Option<AuctionItemRow>> = pushed.into_iter().map(Some).collect();
    order.into_iter().filter_map(|k| rows[k].take()).collect()
}

/// The sort tails `0x4cd8b0`, `0x4cdcd0` and `0x4cddc0`: `qsort` by the list's stack, then the
/// list's event with no test (`mov ecx,0x1a8`-`0x1aa`; `jmp 0x703e50`), a session or none.
fn sort_tail(model: &mut Model, list: usize) {
    if let Some(l) = model.auction.as_mut().and_then(|a| a.lists.get_mut(list)) {
        let stack = l.sort;
        stack.sort(&mut l.rows);
    }
    match list {
        LIST => model.raise("AUCTION_ITEM_LIST_UPDATE", Vec::new()),
        BIDDER => model.raise("AUCTION_BIDDER_LIST_UPDATE", Vec::new()),
        _ => model.raise("AUCTION_OWNED_LIST_UPDATE", Vec::new()),
    }
}

/// `GetTime`'s seconds as the reference's millisecond tick (`0x42c010`), the dword it wraps at.
fn tick_ms(now: f64) -> u32 {
    (now * 1000.0) as u64 as u32
}

/// The time-left bands' upper bounds in milliseconds left (`0x8072a8`): band 1 under 30
/// minutes, 2 under 2 hours, 3 under 8 hours, 4 past them (`0x4cf5a1`).
const TIME_LEFT_BOUNDS: [u32; 3] = [1_800_000, 7_200_000, 28_800_000];

/// How far past its deadline a row still answers 1 and stays: further, the read removes it
/// (`0x4cf569`).
const TIME_LEFT_GRACE_MS: u32 = 10_000;

/// `GetAuctionItemTimeLeft`'s answer for a row with `deadline` at the tick `now` (`0x4cf548`-
/// `0x4cf5a1`): 1 at or past it, the sign read off the wrapping difference (`0x4cf556`), else the
/// band of the time left; and whether it is far enough past to be removed (`0x4cf569`).
fn time_left_band(deadline: u32, now: u32) -> (i64, bool) {
    let past = now.wrapping_sub(deadline);
    if (past as i32) >= 0 {
        return (1, past > TIME_LEFT_GRACE_MS);
    }
    let left = deadline.wrapping_sub(now);
    let band = TIME_LEFT_BOUNDS
        .iter()
        .position(|&bound| left < bound)
        .map_or(4, |i| i as i64 + 1);
    (band, false)
}

/// `0x4cdfe0`: the auction leaves browse, owner and bidder in that order (`0x4cdfec`, `0x4ce04b`,
/// `0x4ce0ab`), each list signalled as its own copy finishes (`0x4ce03e`, `0x4ce09e`,
/// `0x4ce101`), so each segment is its own verb and the browse list's handlers still see the row
/// on the owner list.
fn remove_auction(lua: &Lua, auction_id: u32) {
    for list in [LIST, OWNER, BIDDER] {
        super::tick::verb(lua, |model| remove_from(model, list, auction_id));
    }
}

/// One list's copy of `0x4cdfe0`: the auction's row goes, the rows after it moving up and the
/// total kept, and a list that held it signals unless its sort tail is owed (`0x4ce035`,
/// `0x4ce095`, `0x4ce0f5`). The VM keeps the id, so a push of the same page leaves it out.
fn remove_from(model: &mut Model, list: usize, auction_id: u32) {
    let Some(l) = model.auction.as_mut().map(|a| &mut a.lists[list]) else {
        return;
    };
    let Some(i) = l.rows.iter().position(|r| r.auction_id == auction_id) else {
        return;
    };
    l.rows.remove(i);
    l.removed.push(auction_id);
    if l.pending {
        return;
    }
    match list {
        LIST => model.raise("AUCTION_ITEM_LIST_UPDATE", Vec::new()),
        OWNER => model.raise("AUCTION_OWNED_LIST_UPDATE", Vec::new()),
        _ => model.raise("AUCTION_BIDDER_LIST_UPDATE", Vec::new()),
    }
}

/// The auction id at `list`'s 1-based row `index`, as the getters read it.
fn row_id(model: &Model, list: usize, index: usize) -> Option<u32> {
    model
        .auction
        .as_ref()
        .and_then(|a| index.checked_sub(1).and_then(|n| a.lists[list].rows.get(n)))
        .map(|r| r.auction_id)
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

    // GetAuctionItemInfo(type, index) → twelve values; `highBidder` is a name, `1` or nil.
    g.set(
        "GetAuctionItemInfo",
        lua.create_function(|lua, (kind, index): (String, Value)| {
            let index = crate::script::binding_abi::index_arg(
                lua,
                index,
                "Usage: GetAuctionItemInfo(\"type\", index)",
            )?;
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                row_at(&model, &kind, index)
            };
            // A miss (unknown type, out of range, or an item template not cached: `quality` is
            // `None` until it lands) answers twelve values, `count = 1` and `quality = -1`, through
            // the one exit `0x4cf1ec`; the item-cache lookup `0x55ba30` returning 0 reaches it at
            // `0x4cefa0`.
            let Some(r) = row.filter(|r| r.quality.is_some()) else {
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
                // The required level floored at 1 (`0x4cf091`-`0x4cf0b1`).
                Value::Integer(i64::from(floor_level(&r))),
                Value::Integer(i64::from(r.min_bid)),
                Value::Integer(i64::from(r.min_increment)),
                Value::Integer(i64::from(r.buyout_price)),
                Value::Integer(i64::from(r.bid_amount)),
                match &r.high_bidder {
                    AuctionHighBidder::Nil => Value::Nil,
                    AuctionHighBidder::Player => flag(true),
                    AuctionHighBidder::Name(n) => Value::String(lua.create_string(n)?),
                },
                opt_str(&r.owner)?,
            ]))
        })?,
    )?;

    // GetAuctionItemTimeLeft(type, index) (`0x4cf470`) → the band 1 to 4 of the row's time left,
    // read off its deadline against the clock at the call ([`time_left_band`]); 0 for a type or
    // row that misses (`0x4cf5e1`). A row far enough past is removed after its 1 is pushed
    // (`0x4cf574` → `0x4cdfe0`), its lists signalled inside the call.
    g.set(
        "GetAuctionItemTimeLeft",
        lua.create_function(|lua, (kind, index): (Value, Value)| {
            // `is-number-or-string` and `is-number` else `Usage: GetAuctionItemTimeLeft("type",
            // index)` (`0x8494a0`, `0x4cf5f8`).
            let kind = binding_abi::string_arg(
                lua,
                kind,
                r#"Usage: GetAuctionItemTimeLeft("type", index)"#,
            )?;
            let index = binding_abi::index_arg(
                lua,
                index,
                r#"Usage: GetAuctionItemTimeLeft("type", index)"#,
            )?;
            let (band, remove) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let Some(row) = row_at(&model, &kind, index) else {
                    return Ok(0);
                };
                let (band, remove) = time_left_band(row.deadline, tick_ms(model.now));
                (band, remove.then_some(row.auction_id))
            };
            if let Some(auction_id) = remove {
                remove_auction(lua, auction_id);
            }
            Ok(band)
        })?,
    )?;

    g.set(
        "GetAuctionItemLink",
        lua.create_function(|lua, (kind, index): (String, Value)| {
            let index = crate::script::binding_abi::index_arg(
                lua,
                index,
                "Usage: GetAuctionItemLink(\"type\", index)",
            )?;
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
        lua.create_function(|lua, (kind, index): (Value, Value)| {
            // `is-number-or-string` and `is-number` else `Usage: SetSelectedAuctionItem("type",
            // index)` (`0x8495b4`, `0x4cfeab`).
            let kind = binding_abi::string_arg(
                lua,
                kind,
                r#"Usage: SetSelectedAuctionItem("type", index)"#,
            )?;
            let index = binding_abi::index_arg(
                lua,
                index,
                r#"Usage: SetSelectedAuctionItem("type", index)"#,
            )?;
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

    // SortAuctionItems(type, sort) (`0x4cfc00`): the sort name is matched first, then the type;
    // either unknown sorts nothing (`0x4cfd90`). Otherwise the list's driver promotes the column
    // and runs the sort tail, which sorts and signals before the call returns (`0x4cd850`,
    // `0x4cdc70`, `0x4cdd60`); the click is queued for the app's copy of the stack.
    g.set(
        "SortAuctionItems",
        lua.create_function(|lua, (kind, sort): (Value, Value)| {
            let (kind, sort) = type_and_sort(
                lua,
                kind,
                sort,
                r#"Usage: SortAuctionItems("type", "sort")"#,
            )?;
            let Some(column) = sort_column(&sort) else {
                return Ok(());
            };
            let Some(list) = list_index(&kind) else {
                return Ok(());
            };
            super::tick::verb(lua, |model| {
                model
                    .auction_sorts
                    .push((list, SORT_KEYS[usize::from(column)].to_string()));
                if let Some(a) = model.auction.as_mut() {
                    a.lists[list].sort.promote(column);
                }
                sort_tail(model, list);
            });
            Ok(())
        })?,
    )?;

    // IsAuctionSortReversed(type, sort) → 1/nil (`0x4d0030`): an unknown sort name returns no
    // value (`0x4d024e`), an unknown type nil (`0x4d0230`).
    g.set(
        "IsAuctionSortReversed",
        lua.create_function(|lua, (kind, sort): (Value, Value)| {
            let (kind, sort) = type_and_sort(
                lua,
                kind,
                sort,
                r#"Usage: IsAuctionSortReversed("type", "sort")"#,
            )?;
            let Some(column) = sort_column(&sort) else {
                return Ok(MultiValue::new());
            };
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let rev = list_index(&kind)
                .zip(model.auction.as_ref())
                .and_then(|(i, a)| a.lists[i].sort.reversed(column))
                .unwrap_or(false);
            Ok(MultiValue::from_vec(vec![flag(rev)]))
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
        lua.create_function(|lua, class_index: Value| {
            // `is-number` else `Usage: GetAuctionItemSubClasses(index)` (`0x8494fc`, `0x4cf9d8`).
            let class_index =
                binding_abi::index_arg(lua, class_index, "Usage: GetAuctionItemSubClasses(index)")?;
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
        lua.create_function(|lua, (class_index, sub_index): (Value, Value)| {
            // `is-number` on args 1 and 2 else
            // `Usage: GetAuctionInvTypes(classIndex, subClassIndex)` (`0x849524`, `0x4cfb99`).
            let class_index = binding_abi::index_arg(
                lua,
                class_index,
                "Usage: GetAuctionInvTypes(classIndex, subClassIndex)",
            )?;
            let sub_index = binding_abi::index_arg(
                lua,
                sub_index,
                "Usage: GetAuctionInvTypes(classIndex, subClassIndex)",
            )?;
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
        lua.create_function(|lua, page: Option<Value>| {
            // A page that is not a number is page 0 (`0x4cecaa`, `0x4cecfa`): the query never
            // raises, and `trunc(n)` is the signed page (`0x40a2b0`).
            let page = crate::script::binding_abi::coerced_number(lua, page);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.auction_owner_query = Some(crate::script::binding_abi::ftol(page) as u32);
            Ok(())
        })?,
    )?;
    g.set(
        "GetBidderAuctionItems",
        lua.create_function(|lua, page: Option<Value>| {
            // A page that is not a number is page 0 (`0x4cecaa`, `0x4cecfa`): the query never
            // raises, and `trunc(n)` is the signed page (`0x40a2b0`).
            let page = crate::script::binding_abi::coerced_number(lua, page);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.auction_bidder_query = Some(crate::script::binding_abi::ftol(page) as u32);
            Ok(())
        })?,
    )?;

    // PlaceAuctionBid(type, index, bidAmount); a buyout is a bid of exactly the buyout price.
    g.set(
        "PlaceAuctionBid",
        lua.create_function(|lua, (kind, index, amount): (Value, Value, Value)| {
            // `is-number` on args 2 and 3, `is-number-or-string` on arg 1 else
            // `Usage: PlaceAuctionBid("type", index, bid)` (`0x8494d0`, `0x4cf94d`).
            let kind = binding_abi::string_arg(
                lua,
                kind,
                r#"Usage: PlaceAuctionBid("type", index, bid)"#,
            )?;
            let index = binding_abi::index_arg32(
                lua,
                index,
                r#"Usage: PlaceAuctionBid("type", index, bid)"#,
            )?;
            let amount = binding_abi::index_arg32(
                lua,
                amount,
                r#"Usage: PlaceAuctionBid("type", index, bid)"#,
            )?;
            let Some(list) = list_index(&kind) else {
                return Ok(());
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // The record at the row as the call finds it; no row, no bid (`0x4cf610`).
            let Some(auction_id) = row_id(&model, list, index as usize) else {
                return Ok(());
            };
            model.auction_bids.push(AuctionBid {
                list,
                auction_id,
                amount,
            });
            Ok(())
        })?,
    )?;

    // StartAuction(minBid, buyoutPrice, runTime), runTime in minutes.
    g.set(
        "StartAuction",
        lua.create_function(|lua, (bid, buyout, run): (Value, Value, Value)| {
            const USAGE: &str = "Usage: StartAuction(minBid, buyoutPrice, runTime)";
            let min_bid = super::binding_abi::number_arg(lua, bid, USAGE)? as u32;
            let buyout = super::binding_abi::number_arg(lua, buyout, USAGE)? as u32;
            let duration = super::binding_abi::number_arg(lua, run, USAGE)? as u32;
            super::tick::verb(lua, |model| {
                // No auctioneer or an empty slot, its stored guid zero, ends it before the send
                // (`0x4ce7da`, `0x4ce7e4`).
                let guid = model.auction_sell_item.as_ref().map_or(0, |it| it.guid);
                if !model.auction.as_ref().is_some_and(|a| a.open) || guid == 0 {
                    return;
                }
                // A damaged item, not wrapped (`flags & 8`), with a max durability whose current
                // falls short, refuses with `ERR_AUCTION_REPAIR_ITEM` (`0x4ce80f`-`0x4ce83f`). An
                // item not found skips the check (`0x4ce802`).
                let damaged = model
                    .containers
                    .values()
                    .flat_map(|c| c.slots.values())
                    .find(|s| s.guid == guid)
                    .is_some_and(|s| {
                        s.flags & 0x8 == 0
                            && s.durability.is_some_and(|(cur, max)| max > 0 && cur < max)
                    });
                if damaged {
                    model.push_ui_error("ERR_AUCTION_REPAIR_ITEM");
                    return;
                }
                // A buyout under the minimum bid is raised to it (`0x4ce87b`-`0x4ce883`).
                let buyout = if buyout != 0 && buyout < min_bid {
                    min_bid
                } else {
                    buyout
                };
                // A run time off the table returns silently (`0x4ce8a0`-`0x4ce8b8`).
                if !RUN_TIMES.contains(&duration) {
                    return;
                }
                model.auction_starts.push(AuctionStartRequest {
                    item_guid: guid,
                    min_bid,
                    buyout,
                    duration,
                });
                // The send (`0x4ce925`) is followed by the slot clear whatever the server
                // answers (`0x4ce92a` → `0x4cd640`).
                clear_sell_slot(model);
            });
            Ok(())
        })?,
    )?;

    // CancelAuction(index), always an "owner" row.
    g.set(
        "CancelAuction",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: SetSelectedAuctionItem(index)` (`0x84963c`, `0x4d0279`), the
            // client's own text naming `SetSelectedAuctionItem`.
            let index =
                binding_abi::index_arg32(lua, index, "Usage: SetSelectedAuctionItem(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // The owner record at the row; out of range sends nothing (`0x4d02c5`).
            if let Some(auction_id) = row_id(&model, OWNER, index as usize) {
                model.auction_cancels.push(auction_id);
            }
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
        lua.create_function(|lua, minutes: Value| {
            // `is-number` else `Usage: CalculateAuctionDeposit(runTime)` (`0x8493b0`,
            // `0x4ce1d9`); the run time is `_ftol`'s 32 bits (`0x4ce28d`).
            let minutes =
                binding_abi::index_arg32(lua, minutes, "Usage: CalculateAuctionDeposit(runTime)")?;
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

    // ClickAuctionSellItemButton() (`0x4ce310`): a held item the reference refuses shows its
    // error and changes nothing ([`sell_refusal`]); otherwise attach the cursor's item, putting an
    // earlier attachment back on the cursor ([`cursor::click_attach_slot`]), and a changed slot
    // signals `NEW_AUCTION_UPDATE` right after that `SetCursorItem` (`0x4ce56c`, `0x4ce576`).
    g.set(
        "ClickAuctionSellItemButton",
        lua.create_function(|lua, ()| {
            let refusal = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                match &model.cursor {
                    Some(cursor::CursorPayload::Item(item)) => sell_refusal(&model, item),
                    _ => None,
                }
            };
            if let Some(key) = refusal {
                super::tick::verb(lua, |model| model.push_ui_error(key));
                return Ok(());
            }
            if cursor::click_attach_slot(lua, |model| &mut model.auction_sell_item) {
                super::tick::verb(lua, |model| {
                    model.raise("NEW_AUCTION_UPDATE", Vec::new());
                });
            }
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

#[cfg(test)]
mod tests {
    /// The owner and bidder queries (`0x4cec80`, `0x4cecd0`) read `trunc(n)` as the page and take
    /// anything else for page 0; they never raise.
    #[test]
    fn owner_and_bidder_queries_default_to_page_zero_and_never_raise() {
        let mut s = crate::script::UiScript::new().unwrap();
        for (call, page) in [
            ("()", 0),
            ("(2)", 2),
            ("(2.9)", 2),
            ("('x')", 0),
            ("({})", 0),
            ("(nil)", 0),
            ("(-1)", u32::MAX),
        ] {
            s.run(&format!(
                "GetOwnerAuctionItems{call} GetBidderAuctionItems{call}"
            ))
            .unwrap_or_else(|e| panic!("{call} raised: {e}"));
            assert_eq!(s.take_auction_owner_query(), Some(page), "owner {call}");
            assert_eq!(s.take_auction_bidder_query(), Some(page), "bidder {call}");
        }
    }

    use super::*;
    use crate::script::UiScript;

    /// A session holding one browse page of the given auction ids.
    fn page(ids: &[u32]) -> AuctionState {
        let rows: Vec<AuctionItemRow> = ids
            .iter()
            .map(|&auction_id| AuctionItemRow {
                auction_id,
                item_id: 1000 + auction_id,
                // A cached template: a `None` quality is a row still in the item cache's miss.
                name: Some(format!("Item {auction_id}")),
                base_name: Some(format!("Item {auction_id}")),
                quality: Some(1),
                count: 1,
                ..Default::default()
            })
            .collect();
        let mut state = AuctionState {
            open: true,
            ..Default::default()
        };
        state.lists[LIST] = AuctionListState {
            total: rows.len() as u32,
            rows,
            ..Default::default()
        };
        state
    }

    /// Every list type and sort name matches without case (`SStrCmpI`, `0x64a4c0`), an unknown
    /// one sorts nothing (`0x4cfd90`), and only an argument that is neither string nor number
    /// raises (`0x4cfd82`). `IsAuctionSortReversed` answers nothing for an unknown sort name
    /// (`0x4d024e`) and nil for an unknown type (`0x4d0230`).
    #[test]
    fn list_types_and_sort_names_match_without_case() {
        let mut s = UiScript::new().unwrap();
        s.set_auction(page(&[7]));
        assert_eq!(
            s.eval::<i64>(r#"return (GetNumAuctionItems("LIST"))"#)
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<String>(r#"return (GetAuctionItemInfo("List", 1))"#)
                .unwrap(),
            "Item 7"
        );
        s.run(r#"SortAuctionItems("LIST", "Name")"#).unwrap();
        assert_eq!(s.take_auction_sorts(), [(LIST, "name".to_string())]);
        s.run(r#"SortAuctionItems("list", "nonsense") SortAuctionItems("nowhere", "name")"#)
            .unwrap();
        s.run("SortAuctionItems(1, 2)").unwrap();
        assert!(
            s.take_auction_sorts().is_empty(),
            "an unknown one sorts nothing"
        );
        let err = s
            .run(r#"SortAuctionItems({}, "name")"#)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(r#"Usage: SortAuctionItems("type", "sort")"#),
            "{err}"
        );
        assert_eq!(
            s.arity(r#"IsAuctionSortReversed("list", "nonsense")"#)
                .unwrap(),
            0
        );
        assert_eq!(
            s.arity(r#"IsAuctionSortReversed("nowhere", "bid")"#)
                .unwrap(),
            1
        );
    }

    /// `GetAuctionItemInfo`'s sixth value is the required level floored at 1
    /// (`0x4cf091`-`0x4cf0b1`, `max(template+0x3c, 1)`).
    #[test]
    fn the_level_is_floored_at_one() {
        let mut s = UiScript::new().unwrap();
        let mut state = page(&[7, 8]);
        state.lists[LIST].rows[0].level = 0;
        state.lists[LIST].rows[1].level = 12;
        s.set_auction(state);
        let level = |s: &UiScript, i: usize| {
            s.eval::<i64>(&format!(
                r#"local _, _, _, _, _, l = GetAuctionItemInfo("list", {i}) return l"#
            ))
            .unwrap()
        };
        assert_eq!(level(&s, 1), 1, "no requirement reads 1");
        assert_eq!(level(&s, 2), 12);
    }

    /// `SortAuctionItems` sorts and signals before it returns (`0x4cd850` → `0x4cd8b0`: `qsort`,
    /// then `jmp 0x703e50`), so an addon that flags its own call, as Auctioneer's
    /// `PerformingSortAuctionItems` does, sees the update inside it with the rows in order, and
    /// reads the flipped flag on the next line.
    #[test]
    fn a_sort_signals_its_list_inside_the_call_with_the_rows_in_order() {
        let mut s = UiScript::new().unwrap();
        let mut state = page(&[1, 2]);
        state.lists[LIST].rows[0].name = Some("Cheap".into());
        state.lists[LIST].rows[0].buyout_price = 100;
        state.lists[LIST].rows[1].name = Some("Dear".into());
        state.lists[LIST].rows[1].buyout_price = 900;
        s.set_auction(state);
        s.run(
            r#"SEEN = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("AUCTION_ITEM_LIST_UPDATE")
               f:SetScript("OnEvent", function()
                   table.insert(SEEN, tostring(INSIDE) .. ":" .. (GetAuctionItemInfo("list", 1)))
               end)
               INSIDE = true
               SortAuctionItems("list", "buyout")
               INSIDE = false
               REVERSED = IsAuctionSortReversed("list", "buyout")"#,
        )
        .unwrap();
        assert_eq!(
            s.eval::<Vec<String>>("return SEEN").unwrap(),
            ["true:Dear"],
            "the highest buyout first, signalled inside the call"
        );
        assert!(s.eval::<bool>("return REVERSED == nil").unwrap());
        s.run(
            r#"SEEN = {}
               SortAuctionItems("list", "buyout")
               REVERSED = IsAuctionSortReversed("list", "buyout")"#,
        )
        .unwrap();
        assert_eq!(
            s.eval::<Vec<String>>("return SEEN").unwrap(),
            ["false:Cheap"]
        );
        assert!(
            s.eval::<bool>("return REVERSED == 1").unwrap(),
            "the second click flips the primary, and the next line reads it"
        );
    }

    /// `GetAuctionItemTimeLeft` reads the row's deadline against the clock at the call
    /// (`0x4cf548`): 1 to 3 under 30 minutes, 2 hours and 8 hours left (`0x8072a8`), else 4
    /// (`0x4cf5a1`); 1 at or past the deadline (`0x4cf558`). More than 10 s past (`0x4cf569`) the
    /// call also removes the auction (`0x4cdfe0`) from browse, owner and bidder in that order, each
    /// list that held it signalled inside the call unless its sort tail is owed (`0x4ce035`), the
    /// totals kept; a push of the same page keeps it out. A type or row that misses answers 0
    /// (`0x4cf5e1`).
    #[test]
    fn the_time_left_reads_the_deadline_at_the_call() {
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.set_now(1000.0);
        let now: u32 = 1_000_000;
        let list = |rows: &[(u32, u32)], pending: bool| {
            let rows: Vec<AuctionItemRow> = rows
                .iter()
                .map(|&(auction_id, deadline)| AuctionItemRow {
                    auction_id,
                    item_id: 1000 + auction_id,
                    name: Some(format!("Item {auction_id}")),
                    quality: Some(1),
                    count: 1,
                    deadline,
                    ..Default::default()
                })
                .collect();
            AuctionListState {
                total: rows.len() as u32 + 2,
                rows,
                pending,
                ..Default::default()
            }
        };
        let gone = now - 10_001;
        let mut state = AuctionState::default();
        state.lists[LIST] = list(
            &[
                (1, now + 1_799_999),
                (2, now + 1_800_000),
                (3, now + 7_200_000),
                (4, now + 28_800_000),
                (5, now),
                (6, now - 10_000),
                (7, gone),
            ],
            false,
        );
        state.lists[OWNER] = list(&[(7, gone)], false);
        state.lists[BIDDER] = list(&[(7, gone)], true);
        s.set_auction(state.clone());
        heard::listen(
            &s,
            &[
                "AUCTION_ITEM_LIST_UPDATE",
                "AUCTION_OWNED_LIST_UPDATE",
                "AUCTION_BIDDER_LIST_UPDATE",
            ],
            Some("(GetNumAuctionItems('list')) .. '/' .. (GetNumAuctionItems('owner'))"),
        );
        let left = |s: &UiScript, kind: &str, i: i64| {
            s.eval::<i64>(&format!("return GetAuctionItemTimeLeft('{kind}', {i})"))
                .unwrap()
        };
        let bands: Vec<i64> = (1..=6).map(|i| left(&s, "list", i)).collect();
        assert_eq!(bands, [1, 2, 3, 4, 1, 1]);
        assert!(heard::take(&s).is_empty(), "10 s past is kept");
        assert_eq!(left(&s, "list", 7), 1);
        assert_eq!(
            heard::take(&s),
            [
                "AUCTION_ITEM_LIST_UPDATE 6/1",
                "AUCTION_OWNED_LIST_UPDATE 6/0"
            ],
            "the bidder list owes its tail"
        );
        let counts = "local a, b = GetNumAuctionItems('list') local c, d = \
                      GetNumAuctionItems('bidder') return a, b, c, d";
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(counts).unwrap(),
            (6, 9, 0, 3)
        );
        s.set_auction(state);
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(counts).unwrap(),
            (6, 9, 0, 3)
        );
        assert_eq!(left(&s, "list", 7), 0, "a row past the list");
        assert_eq!(left(&s, "list", 0), 0);
        assert_eq!(left(&s, "auction", 1), 0);
        assert_eq!(left(&s, "LIST", 6), 1);
    }

    /// The band reads the wrapping difference's sign (`0x4cf556`): an unclamped wire time gone
    /// negative (`AuctionHouseMgr.cpp:838`) is past, not eternal, and a deadline across the
    /// dword's wrap still counts down.
    #[test]
    fn the_time_left_band_reads_the_wrapping_difference() {
        let now: u32 = 5_000_000;
        assert_eq!(
            time_left_band(now.wrapping_add(u32::MAX - 1000), now),
            (1, false)
        );
        assert_eq!(
            time_left_band(now.wrapping_add(u32::MAX - 20_000), now),
            (1, true)
        );
        assert_eq!(time_left_band(100, u32::MAX - 5), (1, false), "106 ms left");
        assert_eq!(time_left_band(now + 20 * 3_600_000, now), (4, false));
    }

    #[test]
    fn the_selection_follows_its_auction_through_a_resort() {
        let mut s = UiScript::new().unwrap();
        s.set_auction(page(&[11, 22]));

        s.eval::<()>(r#"SetSelectedAuctionItem("list", 2)"#)
            .unwrap();
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            2
        );

        // The same two auctions, sorted into the opposite order: name, reversed.
        s.run(r#"SortAuctionItems("list", "name") SortAuctionItems("list", "name")"#)
            .unwrap();
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            1,
            "auction 22 moved to row 1 and the selection went with it"
        );

        // It leaves the page.
        s.set_auction(page(&[33, 44]));
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            0,
            "gone means nothing selected, never its neighbour"
        );

        // The close keeps it (`0x4cd5f0`); the world-enter cascade's reset drops it.
        s.set_auction(page(&[33]));
        s.eval::<()>(r#"SetSelectedAuctionItem("list", 1)"#)
            .unwrap();
        s.close_auction_session();
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            1
        );
        s.clear_auction_selections();
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
                item_class: None,
                guid: 0x108,
                bag: 0,
                slot: 1,
                item_id: 2589,
                texture: None,
                link: None,
                count: None,
                quality: None,
                equip_slots: Vec::new(),
                bar_placeable: false,
                consumable: false,
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

    /// A row whose item template has not landed answers the client's miss shape, not a half-filled
    /// row: `0x55ba30` returns 0, `je 0x4cefa0` -> `0x4cf1ec`, nil,nil,1,-1,nil,0,0,0,0,0,nil,nil.
    #[test]
    fn an_uncached_row_answers_the_miss_shape() {
        let mut s = UiScript::new().unwrap();
        let mut state = page(&[7]);
        // The template query is still in flight: no name, icon or quality, a count and prices.
        state.lists[LIST].rows[0] = AuctionItemRow {
            auction_id: 7,
            item_id: 1007,
            count: 20,
            min_bid: 100,
            buyout_price: 200,
            high_bidder: AuctionHighBidder::Player,
            owner: Some("Seller".into()),
            ..Default::default()
        };
        s.set_auction(state);
        let shape: String = s
            .eval(
                r#"local t = {GetAuctionItemInfo("list", 1)}
                   local out = {}
                   for i = 1, 12 do out[i] = tostring(t[i]) end
                   return table.concat(out, ",")"#,
            )
            .unwrap();
        assert_eq!(shape, "nil,nil,1,-1,nil,0,0,0,0,0,nil,nil");
        assert_eq!(s.arity(r#"GetAuctionItemInfo("list", 1)"#).unwrap(), 12);

        // The stock lines index the colour table with the quality, unguarded.
        let r: f64 = s
            .eval(
                r#"ITEM_QUALITY_COLORS = { [-1] = { r = 0.5 }, [1] = { r = 1 } }
                   local name, texture, count, quality = GetAuctionItemInfo("list", 1)
                   local color = ITEM_QUALITY_COLORS[quality]
                   return color.r"#,
            )
            .expect("the colour lookup must not throw on a miss row");
        assert_eq!(r, 0.5);
    }

    /// The same row once its template lands answers its real values.
    #[test]
    fn a_cached_row_answers_its_quality() {
        let mut s = UiScript::new().unwrap();
        s.set_auction(page(&[7]));
        let (name, quality): (String, i64) = s
            .eval(r#"local n, _, _, q = GetAuctionItemInfo("list", 1) return n, q"#)
            .unwrap();
        assert_eq!((name.as_str(), quality), ("Item 7", 1));
    }

    /// The eleventh value is the name, `1` or nil as the app picked it (`0x4cf12a`–`0x4cf1a0`).
    #[test]
    fn high_bidder_answers_a_name_a_one_or_nil() {
        let mut s = UiScript::new().unwrap();
        let mut state = page(&[1, 2, 3]);
        state.lists[LIST].rows[0].high_bidder = AuctionHighBidder::Name("Bidder".into());
        state.lists[LIST].rows[1].high_bidder = AuctionHighBidder::Player;
        s.set_auction(state);
        let eleventh = |i: usize| {
            s.eval::<Value>(&format!(
                r#"local _,_,_,_,_,_,_,_,_,_,h = GetAuctionItemInfo("list", {i}) return h"#
            ))
            .unwrap()
        };
        assert_eq!(
            eleventh(1)
                .as_string()
                .map(|v| v.to_str().unwrap().to_string()),
            Some("Bidder".into())
        );
        assert_eq!(eleventh(2), Value::Integer(1));
        assert_eq!(eleventh(3), Value::Nil);
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

    /// The attach clears with `SetCursorItem(0, …)` (`0x4ce56a`): no unlock and no
    /// `ITEM_LOCK_CHANGED`, so the item stays locked in its bag and cannot be picked up again,
    /// until the sell-slot clear lifts the lock with its event (`0x4cd653`).
    #[test]
    fn an_item_in_the_sell_slot_keeps_its_lock_until_it_leaves() {
        use crate::script::cursor::{bag_lock_probe, two_item_bag};
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(two_item_bag()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        heard::listen(
            &s,
            &["ITEM_LOCK_CHANGED", "CURSOR_UPDATE", "NEW_AUCTION_UPDATE"],
            Some(&bag_lock_probe(1)),
        );
        s.run("ClickAuctionSellItemButton()").unwrap();
        assert_eq!(
            heard::take(&s),
            ["CURSOR_UPDATE 1", "NEW_AUCTION_UPDATE 1"],
            "the attach signals in place, after its SetCursorItem (0x4ce576)"
        );
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(
            s.cursor_payload().is_none(),
            "a locked item is not picked up"
        );
        s.clear_auction_sell_item();
        assert_eq!(
            heard::take(&s),
            ["ITEM_LOCK_CHANGED nil", "NEW_AUCTION_UPDATE nil"]
        );
        s.clear_auction_sell_item();
        assert!(
            heard::take(&s).is_empty(),
            "an empty slot's clear signals nothing"
        );
    }

    /// An occupied sell slot puts the old item back on the cursor through `SetCursorItem(0, …)`
    /// (`0x4ce4f2`-`0x4ce56c`), both items staying locked; with nothing held and nothing attached
    /// the click is `ClearCursor(1,1)` (`0x4ce523`).
    #[test]
    fn an_attach_onto_an_occupied_sell_slot_puts_the_old_item_on_the_cursor() {
        use crate::script::cursor::{bag_lock_probe, install_kind_probe, two_item_bag};
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(two_item_bag()));
        install_kind_probe(&s);
        s.run("PickupContainerItem(0, 1) ClickAuctionSellItemButton() PickupContainerItem(0, 2)")
            .unwrap();
        heard::listen(
            &s,
            &["CURSOR_UPDATE", "ITEM_LOCK_CHANGED", "NEW_AUCTION_UPDATE"],
            Some("CursorKind()"),
        );
        s.run("ClickAuctionSellItemButton()").unwrap();
        assert_eq!(
            heard::take(&s),
            [
                "CURSOR_UPDATE none",
                "CURSOR_UPDATE item",
                "NEW_AUCTION_UPDATE item"
            ]
        );
        assert_eq!(s.cursor_item().map(|i| i.slot), Some(1));
        assert_eq!(s.auction_sell_item(), Some((0, 2)));
        for slot in [1, 2] {
            let probe = bag_lock_probe(slot);
            assert!(s.eval::<bool>(&format!("return {probe} == 1")).unwrap());
        }
        s.run("ClearCursor() ClickAuctionSellItemButton() ClearCursor()")
            .unwrap();
        assert_eq!(s.auction_sell_item(), None);
        heard::take(&s);
        s.run("ClickAuctionSellItemButton()").unwrap();
        assert_eq!(heard::take(&s), ["CURSOR_UPDATE none"]);
    }

    /// `ClickAuctionSellItemButton`'s refusals (`0x4ce360`-`0x4ce4f1`), in the reference's order:
    /// each shows its error (`0x496720`) and returns, the cursor kept, the slot holding what it
    /// held and nothing signalled. A bound item (`0x5da2c0`) is `ERR_AUCTION_QUEST_ITEM` for quest
    /// bonding 4 or 5 and `ERR_AUCTION_BOUND_ITEM` otherwise, a missing record included; then
    /// `ERR_AUCTION_CONJURED_ITEM` for record flags `0x2` or `0x10000`, a nonzero
    /// `ITEM_FIELD_DURATION`, an area or a map; then `ERR_AUCTION_BAG` for a container with an
    /// item in a seat, `ERR_AUCTION_WRAPPED_ITEM` for `ITEM_FIELD_FLAGS & 8` and
    /// `ERR_AUCTION_LOOT_ITEM` for record flags `& 4`. An item that passes them all attaches, one
    /// with no record among them, whose flags read 0 (`0x5da2b3`).
    #[test]
    fn the_sell_slot_refuses_in_the_references_order() {
        use crate::script::container::{ContainerSlot, ContainerState};
        use crate::script::tick::heard;
        use crate::script::ItemTemplateView;
        let mut s = UiScript::new().unwrap();
        // `(bound, ITEM_FIELD_FLAGS, ITEM_FIELD_DURATION, filled seats, record as (flags,
        // bonding, area, map))` per backpack slot, and what the click answers.
        type Record = Option<(u32, u32, u32, u32)>;
        type Case = (bool, u32, u32, Option<u32>, Record, Option<&'static str>);
        #[rustfmt::skip]
        let cases: [Case; 14] = [
            (true, 0, 0, None, Some((0, 4, 0, 0)), Some("ERR_AUCTION_QUEST_ITEM")),
            (true, 0, 0, None, Some((0x2, 5, 12, 0)), Some("ERR_AUCTION_QUEST_ITEM")),
            (true, 0, 0, None, Some((0, 1, 0, 0)), Some("ERR_AUCTION_BOUND_ITEM")),
            (true, 0, 0, None, None, Some("ERR_AUCTION_BOUND_ITEM")),
            (false, 0, 0, None, Some((0x2, 0, 0, 0)), Some("ERR_AUCTION_CONJURED_ITEM")),
            (false, 0, 0, None, Some((0x10000, 0, 0, 0)), Some("ERR_AUCTION_CONJURED_ITEM")),
            (false, 0, 60_000, Some(2), Some((0, 0, 0, 0)), Some("ERR_AUCTION_CONJURED_ITEM")),
            (false, 0, 0, None, Some((0, 0, 12, 0)), Some("ERR_AUCTION_CONJURED_ITEM")),
            (false, 0, 0, None, Some((0, 0, 0, 33)), Some("ERR_AUCTION_CONJURED_ITEM")),
            (false, 0x8, 0, Some(2), Some((0x4, 0, 0, 0)), Some("ERR_AUCTION_BAG")),
            (false, 0x8, 0, None, Some((0x4, 0, 0, 0)), Some("ERR_AUCTION_WRAPPED_ITEM")),
            (false, 0, 0, None, Some((0x4, 0, 0, 0)), Some("ERR_AUCTION_LOOT_ITEM")),
            (false, 0, 0, Some(0), Some((0, 0, 0, 0)), None),
            (false, 0, 0, None, None, None),
        ];
        let mut bag = ContainerState {
            num_slots: 16,
            ..Default::default()
        };
        for (i, &(bound, flags, duration, seats, record, _)) in cases.iter().enumerate() {
            let slot = i as u32 + 1;
            bag.slots.insert(
                slot,
                ContainerSlot {
                    item_id: 1000 + slot,
                    guid: 0x500 + u64::from(slot),
                    count: 1,
                    quality: record.map(|_| 1),
                    already_bound: bound,
                    flags,
                    field_duration: duration,
                    occupied_seats: seats,
                    ..Default::default()
                },
            );
            if let Some((flags, bonding, area, map)) = record {
                s.set_item_template(
                    1000 + slot,
                    ItemTemplateView {
                        flags,
                        bonding,
                        area,
                        map,
                        ..Default::default()
                    },
                );
            }
        }
        s.set_container(0, Some(bag));
        // A worn trinket with a lifetime, picked up off the doll.
        let mut doll: crate::script::InventorySlots = Default::default();
        doll[14] = Some(crate::script::InvSlotView {
            item_id: 7000,
            guid: 0x714,
            field_duration: 3_600_000,
            ..Default::default()
        });
        s.set_inventory_slots(doll);
        heard::listen(
            &s,
            &["CURSOR_UPDATE", "NEW_AUCTION_UPDATE", "ITEM_LOCK_CHANGED"],
            None,
        );
        let click = |s: &mut UiScript, pick: &str| {
            s.run(&format!("ClearCursor() {pick}")).unwrap();
            heard::take(s);
            s.run("ClickAuctionSellItemButton()").unwrap();
            (s.take_ui_errors(), heard::take(s))
        };
        // The last slot attaches first, so each refusal shows the slot left as it was.
        let (errors, heard) = click(&mut s, "PickupContainerItem(0, 14)");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(heard, ["CURSOR_UPDATE", "NEW_AUCTION_UPDATE"]);
        let mut held = 14;
        for (i, &(.., refusal)) in cases.iter().enumerate() {
            let slot = i as u32 + 1;
            let pick = format!("PickupContainerItem(0, {slot})");
            let (errors, heard) = click(&mut s, &pick);
            match refusal {
                Some(key) => {
                    assert_eq!((errors, heard), (vec![key], vec![]), "slot {slot}");
                    assert_eq!(
                        s.cursor_item().map(|c| c.slot),
                        Some(slot),
                        "slot {slot}: the cursor keeps it"
                    );
                }
                None => {
                    assert!(errors.is_empty(), "slot {slot}: {errors:?}");
                    held = slot;
                }
            }
            assert_eq!(s.auction_sell_item(), Some((0, held)), "slot {slot}");
        }
        let (errors, heard) = click(&mut s, "PickupInventoryItem(14)");
        assert_eq!(
            (errors, heard),
            (vec!["ERR_AUCTION_CONJURED_ITEM"], vec![]),
            "a worn item answers to its own ITEM_FIELD_DURATION"
        );
        assert_eq!(s.auction_sell_item(), Some((0, held)));
    }

    /// `StartAuction` (`0x4ce770`) sends and then runs the sell-slot clear in the verb
    /// (`0x4ce92a` → `0x4cd640`): the unlock and `NEW_AUCTION_UPDATE` fire before it returns,
    /// whatever the server answers, so a second call finds the slot empty and sends nothing
    /// (`0x4ce7e4`). A run time off the table sends nothing and keeps the slot (`0x4ce8a0`); a
    /// buyout under the minimum bid is raised to it (`0x4ce883`).
    #[test]
    fn start_auction_sends_and_empties_the_slot_in_the_verb() {
        use crate::script::cursor::{bag_lock_probe, two_item_bag};
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        let mut bag = two_item_bag();
        for (slot, item) in bag.slots.iter_mut() {
            item.guid = 0x4000 + u64::from(*slot);
        }
        s.set_container(0, Some(bag));
        s.run("PickupContainerItem(0, 1) ClickAuctionSellItemButton()")
            .unwrap();
        // The attach stored the guid (`[0xb72608]`); the item moving on leaves it.
        s.set_container(0, Some(two_item_bag()));
        s.run("StartAuction(100, 0, 120)").unwrap();
        assert!(s.take_auction_starts().is_empty(), "no auctioneer");

        s.set_auction(AuctionState {
            open: true,
            ..Default::default()
        });
        heard::listen(
            &s,
            &["ITEM_LOCK_CHANGED", "NEW_AUCTION_UPDATE"],
            Some(&format!(
                "tostring({}) .. ',' .. tostring((GetAuctionSellItemInfo()))",
                bag_lock_probe(1)
            )),
        );
        s.run("StartAuction(100, 50, 100)").unwrap();
        assert!(
            s.take_auction_starts().is_empty(),
            "a run time off the table"
        );
        assert_eq!(s.auction_sell_item(), Some((0, 1)));

        s.run("StartAuction(100, 50, 480) StartAuction(100, 0, 480)")
            .unwrap();
        assert_eq!(
            heard::take(&s),
            ["ITEM_LOCK_CHANGED nil,nil", "NEW_AUCTION_UPDATE nil,nil"]
        );
        assert_eq!(
            s.take_auction_starts(),
            [AuctionStartRequest {
                item_guid: 0x4001,
                min_bid: 100,
                buyout: 100,
                duration: 480,
            }],
            "one send; the second call finds the slot empty"
        );
        let err = s.run("StartAuction(100, 0)").unwrap_err().to_string();
        assert!(err.contains("Usage: StartAuction(minBid, buyoutPrice, runTime)"));
    }

    /// A damaged item, not wrapped, refuses before the send with `ERR_AUCTION_REPAIR_ITEM` (id
    /// 375, `0x4ce80f`-`0x4ce83f`) and keeps its slot; a wrapped one, or one at full durability,
    /// goes.
    #[test]
    fn start_auction_refuses_a_damaged_item() {
        use crate::script::cursor::two_item_bag;
        let mut s = UiScript::new().unwrap();
        s.set_auction(AuctionState {
            open: true,
            ..Default::default()
        });
        let bag = |durability, flags| {
            let mut bag = two_item_bag();
            for (slot, item) in bag.slots.iter_mut() {
                item.guid = 0x4000 + u64::from(*slot);
                item.durability = durability;
                item.flags = flags;
            }
            bag
        };
        s.set_container(0, Some(bag(Some((10, 40)), 0)));
        s.run("PickupContainerItem(0, 1) ClickAuctionSellItemButton() StartAuction(100, 0, 120)")
            .unwrap();
        assert!(s.take_auction_starts().is_empty());
        assert_eq!(s.take_ui_errors(), vec!["ERR_AUCTION_REPAIR_ITEM"]);
        assert_eq!(s.auction_sell_item(), Some((0, 1)), "the slot keeps it");

        s.set_container(0, Some(bag(Some((10, 40)), 0x8)));
        s.run("StartAuction(100, 0, 120)").unwrap();
        assert_eq!(
            s.take_auction_starts().len(),
            1,
            "a wrapped item is not checked"
        );
        assert!(s.take_ui_errors().is_empty());
    }
}
