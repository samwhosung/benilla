//! The auction-house bindings: the app pushes a snapshot of three lists, rows resolved and in
//! wire order, and the verbs that send hand their call to the host, which applies it inside the
//! call ([`AuctionCall`]). Getters are keyed by `"list"` (Browse), `"bidder"` (Bids) or `"owner"`
//! (Auctions), with `index` 1-based in the 50-row batch.
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
    /// The seller's guid (`rec+0x20`), whose name `GetAuctionItemInfo` asks the name cache for
    /// with the list's callback when it is not cached (`0x4cf1c8`); 0 asks nothing (`0x4cf1b2`).
    pub owner_guid: u64,
    /// The bidder the eleventh value names: on an owner row the player owns with a bid
    /// (`0x4cf131`-`0x4cf152`), the bid's guid, asked like the seller's (`0x4cf167`); else 0.
    pub named_bidder: u64,
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

/// What a bid result or notice writes into a held row: the bid (`+0x40`), the step (`+0x2c`), the
/// deadline (`+0x34`) and the bidder (`+0x38`), with what the getters read off the bidder, as the
/// app resolves a row's ([`UiScript::patch_auction_row`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionRowBid {
    pub bid_amount: u32,
    pub min_increment: u32,
    pub deadline: u32,
    pub bidder_rank: u8,
    pub high_bidder: AuctionHighBidder,
    pub named_bidder: u64,
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
    /// (`0xb72644`, `0xb72648`, `0xb7264c`), so a removal or a bid patch signals nothing
    /// (`0x4ce035`, `0x4cc5c3`, `0x4cd090`).
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
}

impl Default for AuctionState {
    /// Three empty lists, each with its world-entry stack, and no deposit rate.
    fn default() -> Self {
        Self {
            lists: AuctionSortStack::seeded_all().map(|sort| AuctionListState {
                sort,
                ..Default::default()
            }),
            deposit_percent: 0,
        }
    }
}

/// `QueryAuctionItems`' arguments, the `CMSG_AUCTION_LIST_ITEMS` filters: each is already a wire
/// id, mapped from the Lua's menu position as `0x4ce980` does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionQuery {
    pub name: String,
    /// `0` = no filter, for both: the argument's `_ftol` low byte, 0 for a non-number
    /// (`0x4ce9fd`-`0x4cea3b`).
    pub min_level: u8,
    pub max_level: u8,
    /// `None` = no filter (the wire's `0xFFFFFFFF`), for all three.
    pub inv_type: Option<u32>,
    pub class: Option<u32>,
    pub sub_class: Option<u32>,
    /// A minimum quality, not an equality; `None` = no filter (the dropdown's All, -1).
    pub quality: Option<u32>,
    /// The wire's item offset: the page argument's `_ftol` times 50 in 32 bits, 0 for a
    /// non-number (`0x4ceb28`-`0x4ceb44`).
    pub list_from: u32,
    pub usable_only: bool,
}

/// `StartAuction`'s send (`0x4ce8c5`-`0x4ce925`): the sell slot's item as the attach stored it.
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

/// The run times `StartAuction` and the deposit accept, in minutes (`0x807028`, `0x80e4b0`).
const RUN_TIMES: [u32; 3] = [120, 480, 1440];

/// `PlaceAuctionBid` past its row (`0x4cf610`): the bid as passed and the fields of the record its
/// checks read, as the call found them.
#[derive(Clone, Debug, PartialEq)]
pub struct AuctionBid {
    /// [`LIST`], [`BIDDER`] or [`OWNER`]: the verb takes any of the three (`0x4cf6ae`-`0x4cf70f`).
    pub list: usize,
    /// `rec+0x04`, the id the packet carries (`0x4cf8f6`).
    pub auction_id: u32,
    /// The bid, `_ftol` of argument 3 (`0x4cf696`).
    pub amount: u32,
    /// `rec+0x08`, the entry the unique-item check looks up (`0x4cf86f`).
    pub item_id: u32,
    /// `rec+0x28`, the start bid.
    pub min_bid: u32,
    /// `rec+0x2c`, the minimum increment.
    pub increment: u32,
    /// `rec+0x30`, 0 for none.
    pub buyout: u32,
    /// `rec+0x40`, 0 before any bid.
    pub current: u32,
    /// The player holds the bid: `rec+0x38` is its guid (`0x4cf7fb`-`0x4cf810`).
    pub player_leads: bool,
}

/// What `PlaceAuctionBid`'s checks make of a bid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BidVerdict {
    /// Send `CMSG_AUCTION_PLACE_BID` with this bid (`0x4cf915`), the buyout when it was lowered.
    Send(u32),
    /// Show this line and send nothing.
    Refuse(&'static str),
    /// Send nothing and say nothing: a bid past 2,000,000,000 and the next step (`0x4cf7f0`).
    Drop,
}

impl AuctionBid {
    /// The checks in the reference's order (`0x4cf762`-`0x4cf8a6`), every compare unsigned but the
    /// unique count's. `money` is the player's coinage, 0 when it is not the active player
    /// (`0x4cf834`); `unique` gives the template's `MaxCount` (`+0x5c`) and the carried count
    /// (`0x622130` with scope `0x4f`), asked only past the money check and only for a cached record
    /// (`0x4cf879`, no callback).
    pub fn verdict(&self, money: u32, unique: impl FnOnce() -> Option<(i32, i32)>) -> BidVerdict {
        // A bid above a nonzero buyout becomes the buyout (`0x4cf762`-`0x4cf773`).
        let bid = if self.buyout > 0 && self.amount > self.buyout {
            self.buyout
        } else {
            self.amount
        };
        // Under the start bid, whatever the current bid (`0x4cf775`).
        if bid < self.min_bid {
            return BidVerdict::Refuse("ERR_AUCTION_MIN_BID");
        }
        // Not above the current bid (`0x4cf795`).
        if bid <= self.current {
            return BidVerdict::Refuse("ERR_AUCTION_HIGHER_BID");
        }
        // With a bid, under it plus the increment, unless it reaches a nonzero buyout
        // (`0x4cf7b1`-`0x4cf7c4`).
        let step = self.current.wrapping_add(self.increment);
        if self.current != 0 && bid < step && (self.buyout == 0 || bid < self.buyout) {
            return BidVerdict::Refuse("ERR_AUCTION_BID_INCREMENT");
        }
        // Past 2,000,000,000 and the step, dropped (`0x4cf7e1`-`0x4cf7f0`).
        if bid > 2_000_000_000 && bid > step {
            return BidVerdict::Drop;
        }
        // The money covers the bid, or only the rise when the player leads (`0x4cf812`-`0x4cf84a`).
        let needed = if self.player_leads {
            bid.wrapping_sub(self.current)
        } else {
            bid
        };
        if money < needed {
            return BidVerdict::Refuse("ERR_NOT_ENOUGH_MONEY");
        }
        // A unique item carried at its limit is error 0x12 (`0x4cf887`-`0x4cf8a8`).
        if unique().is_some_and(|(max, carried)| max > 0 && carried >= max) {
            return BidVerdict::Refuse("ERR_ITEM_MAX_COUNT");
        }
        BidVerdict::Send(bid)
    }
}

/// `CancelAuction`'s fee on an auction with a bid (`0x4d0338`-`0x4d0341`): `fild` of the bid times
/// the `f32` 0.05 at `0x807024`, truncated by `_ftol`, and compared unsigned (`0x4d0346`).
pub fn cancel_fee(bid: u32) -> u32 {
    binding_abi::ftol(f64::from(bid) * f64::from(f32::from_bits(0x3d4c_cccd))) as u32
}

/// An auction verb past its binding's checks, which the host applies inside the call: its gates
/// on the auctioneer and the session's flags, its refusals and its packet, before the verb
/// returns.
#[derive(Clone, Debug, PartialEq)]
pub enum AuctionCall {
    /// `QueryAuctionItems` (`0x4ce980`), its arguments read: with an auctioneer and the throttle
    /// open (`0x4ce99c`, `0x4ce9c0`), `CMSG_AUCTION_LIST_ITEMS` and the throttle armed.
    Query(AuctionQuery),
    /// `GetOwnerAuctionItems(page)` (`0x4cec80`): the list offset, page × 50 (`0x4cecbf`), which
    /// the sender `0x4cd680` sends while the owner flag is armed.
    OwnerList { offset: u32 },
    /// `GetBidderAuctionItems(page)` (`0x4cecd0`), the offset `0x4ced0f`, through `0x4cd720`.
    BidderList { offset: u32 },
    /// `PlaceAuctionBid` past its row: with a player and an auctioneer (`0x4cf735`-`0x4cf75c`),
    /// [`AuctionBid::verdict`].
    Bid(AuctionBid),
    /// `CancelAuction(index)` past its row: with an auctioneer (`0x4d02a5`), the fee check on a
    /// bid (`0x4d02dc`) and `CMSG_AUCTION_REMOVE_ITEM`.
    Cancel { auction_id: u32, bid: u32 },
    /// `StartAuction` past its slot: with an auctioneer (`0x4ce7da`), the damaged item's refusal
    /// or the send and the sell slot's clear (`0x4ce925`, `0x4ce92a`).
    Start(Result<AuctionStartRequest, &'static str>),
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
                    merge_list(list, held);
                }
                Some(state)
            }
            (None, state) => Some(state),
        };
    }

    /// The close's half the VM holds (`0x4cd613`-`0x4cd61f`): the house goes, and its deposit rate
    /// with it; the lists, their stacks and the selection stand, and the getters still answer them.
    pub fn close_auction_session(&mut self) {
        if let Some(a) = self.model_mut().auction.as_mut() {
            a.deposit_percent = 0;
        }
    }

    /// The world-enter cascade's reset of the selection (`0x4cc2d5`), the one writer of the cells
    /// besides `SetSelectedAuctionItem`.
    pub fn clear_auction_selections(&mut self) {
        self.model_mut().auction_selected = [0; 3];
    }

    /// One list's result as its handler writes it (`0x4cc880`), merged as [`Self::set_auction`]
    /// merges each list; with no lists held yet, the others take their empty state and every list
    /// the app's stack `sorts`.
    pub fn set_auction_list(
        &mut self,
        list: usize,
        mut state: AuctionListState,
        sorts: [AuctionSortStack; 3],
    ) {
        let mut model = self.model_mut();
        match model.auction.as_mut() {
            Some(held) => {
                let old = std::mem::take(&mut held.lists[list]);
                merge_list(&mut state, old);
                held.lists[list] = state;
            }
            None => {
                let mut fresh = AuctionState::default();
                for (l, sort) in fresh.lists.iter_mut().zip(sorts) {
                    l.sort = sort;
                }
                fresh.lists[list] = state;
                model.auction = Some(fresh);
            }
        }
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

    /// The auction calls a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_auction_calls(&mut self) -> Vec<AuctionCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Auction(call) => Some(call.clone()),
            _ => None,
        })
    }

    /// The guids `GetAuctionItemInfo` read and missed in the name cache since the last take, each
    /// with the list whose callback it registers, in read order, for the app to ask
    /// (`0x4cf167`, `0x4cf1c8`).
    pub fn take_auction_name_asks(&mut self) -> Vec<(u64, usize)> {
        std::mem::take(&mut self.model_mut().auction_name_asks)
    }

    /// `0x4cdfe0` as a result or notice runs it (`0x4cc658`, `0x4cd173`, `0x4cd468`,
    /// `0x4cd550`): the auction leaves each list that holds it on the page `pages` names, browse,
    /// owner and bidder in that order, each signalled as its copy finishes ([`remove_from`]).
    pub fn remove_auction_from(&mut self, auction_id: u32, pages: [Option<u32>; 3]) {
        for list in [LIST, OWNER, BIDDER] {
            if let Some(page) = pages[list] {
                super::tick::verb(&self.lua, |model| {
                    remove_from(model, list, Some(page), auction_id)
                });
            }
        }
    }

    /// A bid result or notice's write into `list`'s row for the auction on `page`, then the
    /// list's update unless it waits on an item record: browse `0x4cc5cb`, `0x4cc6fe` and
    /// `0x4cd13e`, owner `0x4cd2fc`, bidder `0x4cc764` and `0x4cd099`, each after its pending
    /// count's test. The update fires whether or not the row changed.
    pub fn patch_auction_row(
        &mut self,
        list: usize,
        page: u32,
        auction_id: u32,
        bid: AuctionRowBid,
    ) {
        super::tick::verb(&self.lua, |model| {
            patch_row(model, list, page, auction_id, bid)
        });
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

/// A pushed list against the one the VM held: the VM's stack kept, and on the same page the VM's
/// order and the rows a time-left read removed.
fn merge_list(list: &mut AuctionListState, held: AuctionListState) {
    list.sort = held.sort;
    list.removed = Vec::new();
    if list.page == held.page {
        list.removed = held.removed;
        let removed = &list.removed;
        list.rows.retain(|r| !removed.contains(&r.auction_id));
        list.rows = in_held_order(&held.rows, std::mem::take(&mut list.rows));
    }
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

/// `GetTime`'s seconds as the reference's millisecond tick (`0x42c010`), the dword it wraps at: the
/// clock the auction records' deadlines and the bid notices' 90 s floor are on.
pub fn tick_ms(now: f64) -> u32 {
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
        super::tick::verb(lua, |model| remove_from(model, list, None, auction_id));
    }
}

/// One list's copy of `0x4cdfe0`: the auction's row goes, the rows after it moving up and the
/// total kept, and a list that held it signals unless its sort tail is owed (`0x4ce035`,
/// `0x4ce095`, `0x4ce0f5`). The VM keeps the id, so a push of the same page leaves it out. With a
/// `page`, a list holding another page is left alone.
fn remove_from(model: &mut Model, list: usize, page: Option<u32>, auction_id: u32) {
    let Some(l) = model.auction.as_mut().map(|a| &mut a.lists[list]) else {
        return;
    };
    if page.is_some_and(|p| p != l.page) {
        return;
    }
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

/// One list's copy of a bid patch ([`UiScript::patch_auction_row`]): the row's bid, step,
/// deadline and bidder written, then the list's update unless its pending count is not 0.
fn patch_row(model: &mut Model, list: usize, page: u32, auction_id: u32, bid: AuctionRowBid) {
    let Some(l) = model.auction.as_mut().map(|a| &mut a.lists[list]) else {
        return;
    };
    if l.page != page {
        return;
    }
    let Some(r) = l.rows.iter_mut().find(|r| r.auction_id == auction_id) else {
        return;
    };
    r.bid_amount = bid.bid_amount;
    r.min_increment = bid.min_increment;
    r.deadline = bid.deadline;
    r.bidder_rank = bid.bidder_rank;
    r.high_bidder = bid.high_bidder;
    r.named_bidder = bid.named_bidder;
    if l.pending {
        return;
    }
    match list {
        LIST => model.raise("AUCTION_ITEM_LIST_UPDATE", Vec::new()),
        OWNER => model.raise("AUCTION_OWNED_LIST_UPDATE", Vec::new()),
        _ => model.raise("AUCTION_BIDDER_LIST_UPDATE", Vec::new()),
    }
}

/// Hand `call` to the host, which applies it before this returns ([`super::calls::submit`]).
fn submit(lua: &Lua, call: AuctionCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Auction(call))
}

/// A level bound of `QueryAuctionItems`: a number's `_ftol` low byte (`0x4cea17`, `0x4cea3b`),
/// anything else 0.
fn level_byte(lua: &Lua, v: Option<Value>) -> u8 {
    v.and_then(|v| binding_abi::is_number(lua, v))
        .map_or(0, |n| binding_abi::ftol(n) as u8)
}

/// A list verb's offset: a number's `_ftol` times 50 in 32 bits (`0x4ceb44`, `0x4cecbf`,
/// `0x4ced0f`), anything else 0 (`0x4ceb2f`, `0x4cecaa`, `0x4cecfa`); the verbs never raise.
fn list_offset(lua: &Lua, page: Option<Value>) -> u32 {
    match page.and_then(|v| binding_abi::is_number(lua, v)) {
        Some(n) => binding_abi::ftol(n).wrapping_mul(50) as u32,
        None => 0,
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
    // The price is a 32-bit `imul` in both readers (`0x4ce295`, `0x4ce703`).
    Some((
        name,
        it.texture.clone(),
        count,
        quality,
        can_use,
        sell_price.wrapping_mul(count),
    ))
}

/// The reference client's deposit, `0x644bc0(rate, stackValue, minutes)`: 0 for a run time off the
/// table at `0x80e4b0` (`0x644bd0`-`0x644be1`), else `rate × stackValue` in 32 bits (`0x644be9`)
/// over 100, times `minutes / 120` (`0x644bf3`-`0x644c17`). The inner floor zeroes a cheap stack
/// the server still charges a few copper for.
fn deposit_for(rate: u32, stack_value: u32, minutes: u32) -> u32 {
    if !RUN_TIMES.contains(&minutes) {
        return 0;
    }
    // At most `u32::MAX / 100 × 12`, so the x87 product `_ftol` truncates is exact.
    rate.wrapping_mul(stack_value) / 100 * (minutes / 120)
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
            // A name the cache misses is asked with the list's callback, the bidder's on an owned
            // row first (`0x4cf167`), then the seller's (`0x4cf1c8`); nothing else asks a name.
            if let Some(list) = list_index(&kind) {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let bidder_missed = r.named_bidder != 0 && r.high_bidder == AuctionHighBidder::Nil;
                let seller_missed = r.owner_guid != 0 && r.owner.is_none();
                for (missed, guid) in [
                    (bidder_missed, r.named_bidder),
                    (seller_missed, r.owner_guid),
                ] {
                    if missed && !model.auction_name_asks.contains(&(guid, list)) {
                        model.auction_name_asks.push((guid, list));
                    }
                }
            }
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

    // GetSelectedAuctionItem(type) (`0x4cfec0`) → the selected row, 0 for none or gone. Stored as
    // the auction id and resolved on the way out (`0x4cff21`-`0x4cff35`), so it follows its
    // auction through a re-sort. A type that is none of the three answers 1 (`0x4cffc5` →
    // `0x4d0013`, `push 0x3ff00000`).
    g.set(
        "GetSelectedAuctionItem",
        lua.create_function(|lua, kind: Value| {
            // `is-number-or-string` else the usage line (`0x4cfecc`, `0x8495e4` at `0x4cfed5`).
            let kind =
                binding_abi::string_arg(lua, kind, r#"Usage: GetSelectedAuctionItem("type")"#)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(i) = list_index(&kind) else {
                return Ok(1i64);
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
            // A menu position: a number's `_ftol` less one, compared unsigned, so 0 and a
            // negative fall out of range (`0x4cea62`-`0x4cea6b`).
            let position = |i: usize| {
                a.get(i)
                    .cloned()
                    .and_then(|v| binding_abi::is_number(lua, v))
                    .map(|n| binding_abi::ftol(n).wrapping_sub(1) as u32 as usize)
            };
            let query = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
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
                AuctionQuery {
                    name,
                    min_level: level_byte(lua, a.get(1).cloned()),
                    max_level: level_byte(lua, a.get(2).cloned()),
                    inv_type,
                    class,
                    sub_class,
                    quality,
                    list_from: list_offset(lua, a.get(6).cloned()),
                    usable_only: truthy(7),
                }
            };
            submit(lua, AuctionCall::Query(query))
        })?,
    )?;

    // GetOwnerAuctionItems([page]) (`0x4cec80`) / GetBidderAuctionItems([page]) (`0x4cecd0`):
    // unthrottled, and sent only while the list's one-shot flag is armed, which the host tests
    // with the auctioneer (`0x4cec8e`-`0x4cec97`, `0x4cecde`-`0x4cece7`).
    g.set(
        "GetOwnerAuctionItems",
        lua.create_function(|lua, page: Option<Value>| {
            submit(
                lua,
                AuctionCall::OwnerList {
                    offset: list_offset(lua, page),
                },
            )
        })?,
    )?;
    g.set(
        "GetBidderAuctionItems",
        lua.create_function(|lua, page: Option<Value>| {
            submit(
                lua,
                AuctionCall::BidderList {
                    offset: list_offset(lua, page),
                },
            )
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
            // The record at the row as the call finds it; no row, no bid (`0x4cf735`).
            let bid = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let Some(r) = index
                    .checked_sub(1)
                    .and_then(|n| model.auction.as_ref()?.lists[list].rows.get(n as usize))
                else {
                    return Ok(());
                };
                AuctionBid {
                    list,
                    auction_id: r.auction_id,
                    amount,
                    item_id: r.item_id,
                    min_bid: r.min_bid,
                    increment: r.min_increment,
                    buyout: r.buyout_price,
                    current: r.bid_amount,
                    player_leads: r.bidder_rank == 2,
                }
            };
            submit(lua, AuctionCall::Bid(bid))
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
            let call = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                // An empty slot, its stored guid zero, ends it before the send (`0x4ce7e4`); the
                // host tests the auctioneer ahead of it (`0x4ce7da`).
                let guid = model.auction_sell_item.as_ref().map_or(0, |it| it.guid);
                if guid == 0 {
                    return Ok(());
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
                    Err("ERR_AUCTION_REPAIR_ITEM")
                } else {
                    // A buyout under the minimum bid is raised to it (`0x4ce87b`-`0x4ce883`).
                    let buyout = if buyout != 0 && buyout < min_bid {
                        min_bid
                    } else {
                        buyout
                    };
                    // A run time off the table returns silently (`0x4ce8a0`-`0x4ce8b8`).
                    if !RUN_TIMES.contains(&duration) {
                        return Ok(());
                    }
                    Ok(AuctionStartRequest {
                        item_guid: guid,
                        min_bid,
                        buyout,
                        duration,
                    })
                }
            };
            submit(lua, AuctionCall::Start(call))
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
            // The owner record at the row; out of range sends nothing (`0x4d02c5`).
            let cancel = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                index
                    .checked_sub(1)
                    .and_then(|n| model.auction.as_ref()?.lists[OWNER].rows.get(n as usize))
                    .map(|r| AuctionCall::Cancel {
                        auction_id: r.auction_id,
                        bid: r.bid_amount,
                    })
            };
            match cancel {
                Some(call) => submit(lua, call),
                None => Ok(()),
            }
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

    // CalculateAuctionDeposit(runTime) (`0x4ce1c0`) → copper, by the client's arithmetic
    // ([`deposit_for`]), 0 for a run time other than 120, 480 or 1440.
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
    /// The owner and bidder queries (`0x4cec80`, `0x4cecd0`) read `trunc(n)` as the page, times 50
    /// in 32 bits (`0x4cecbf`, `0x4ced0f`), and take anything else for page 0; they never raise,
    /// and hand their offset to the host.
    #[test]
    fn owner_and_bidder_queries_default_to_page_zero_and_never_raise() {
        use super::AuctionCall;
        let mut s = crate::script::UiScript::new().unwrap();
        s.keep_calls();
        for (call, offset) in [
            ("()", 0),
            ("(2)", 100),
            ("(2.9)", 100),
            ("('x')", 0),
            ("({})", 0),
            ("(nil)", 0),
            ("(-1)", (-50i32) as u32),
        ] {
            s.run(&format!(
                "GetOwnerAuctionItems{call} GetBidderAuctionItems{call}"
            ))
            .unwrap_or_else(|e| panic!("{call} raised: {e}"));
            assert_eq!(
                s.take_auction_calls(),
                [
                    AuctionCall::OwnerList { offset },
                    AuctionCall::BidderList { offset }
                ],
                "{call}"
            );
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
        let mut state = AuctionState::default();
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
        s.keep_calls();
        s.set_auction_item_classes(tree());
        let query = |s: &mut UiScript| match s.take_auction_calls().pop() {
            Some(AuctionCall::Query(q)) => q,
            other => panic!("one query handed to the host, got {other:?}"),
        };

        // Weapon / its first subclass / Back (the 13th inventory row).
        s.run(r#"QueryAuctionItems("", "", "", 13, 1, 1, 0, nil, -1)"#)
            .unwrap();
        let q = query(&mut s);
        assert_eq!(q.inv_type, Some(16), "INVTYPE_CLOAK is inventory type 16");
        assert_eq!(q.class, Some(2), "Weapon is item class 2, not position 1");
        assert_eq!(q.sub_class, Some(0), "the first weapon subclass is id 0");

        // Armor / its second subclass skips the id gap: Shield is 6, not 2.
        s.run(r#"QueryAuctionItems("", "", "", 14, 2, 2, 0, nil, -1)"#)
            .unwrap();
        let q = query(&mut s);
        assert_eq!(
            (q.inv_type, q.class, q.sub_class),
            (Some(23), Some(4), Some(6))
        );

        // The scan ignores the filter flag: Consumable offers no subclass rows, yet resolves one.
        s.run(r#"QueryAuctionItems("", "", "", 0, 3, 1, 0, nil, -1)"#)
            .unwrap();
        let q = query(&mut s);
        assert_eq!((q.inv_type, q.class, q.sub_class), (None, Some(0), Some(0)));

        // Out of range, or no class to scan under: the wire's "any".
        s.run(r#"QueryAuctionItems("", "", "", 15, 11, 1, 0, nil, -1)"#)
            .unwrap();
        let q = query(&mut s);
        assert_eq!((q.inv_type, q.class, q.sub_class), (None, None, None));
        s.run(r#"QueryAuctionItems("", "", "", nil, 1, 3, 0, nil, -1)"#)
            .unwrap();
        let q = query(&mut s);
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

    /// `StartAuction` (`0x4ce770`) hands the host its send, the guid the attach stored
    /// (`0x4ce8f3`) wherever the item has since gone; the host tests the auctioneer, sends and
    /// empties the slot. A run time off the table hands nothing and keeps the slot (`0x4ce8a0`); a
    /// buyout under the minimum bid is raised to it (`0x4ce883`); an empty slot hands nothing
    /// (`0x4ce7e4`).
    #[test]
    fn start_auction_hands_its_send_to_the_host() {
        use crate::script::cursor::two_item_bag;
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run("StartAuction(100, 0, 120)").unwrap();
        assert!(s.take_auction_calls().is_empty(), "an empty slot");
        let mut bag = two_item_bag();
        for (slot, item) in bag.slots.iter_mut() {
            item.guid = 0x4000 + u64::from(*slot);
        }
        s.set_container(0, Some(bag));
        s.run("PickupContainerItem(0, 1) ClickAuctionSellItemButton()")
            .unwrap();
        // The attach stored the guid (`[0xb72608]`); the item moving on leaves it.
        s.set_container(0, Some(two_item_bag()));
        s.run("StartAuction(100, 50, 100)").unwrap();
        assert!(
            s.take_auction_calls().is_empty(),
            "a run time off the table"
        );
        assert_eq!(s.auction_sell_item(), Some((0, 1)));

        s.run("StartAuction(100, 50, 480)").unwrap();
        assert_eq!(
            s.take_auction_calls(),
            [AuctionCall::Start(Ok(AuctionStartRequest {
                item_guid: 0x4001,
                min_bid: 100,
                buyout: 100,
                duration: 480,
            }))]
        );
        let err = s.run("StartAuction(100, 0)").unwrap_err().to_string();
        assert!(err.contains("Usage: StartAuction(minBid, buyoutPrice, runTime)"));
    }

    /// A damaged item, not wrapped, hands the host the refusal `ERR_AUCTION_REPAIR_ITEM` (id 375,
    /// `0x4ce80f`-`0x4ce83f`) in place of the send, and keeps its slot; a wrapped one is not
    /// checked.
    #[test]
    fn start_auction_refuses_a_damaged_item() {
        use crate::script::cursor::two_item_bag;
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        assert_eq!(
            s.take_auction_calls(),
            [AuctionCall::Start(Err("ERR_AUCTION_REPAIR_ITEM"))]
        );
        assert_eq!(s.auction_sell_item(), Some((0, 1)), "the slot keeps it");

        s.set_container(0, Some(bag(Some((10, 40)), 0x8)));
        s.run("StartAuction(100, 0, 120)").unwrap();
        assert!(
            matches!(s.take_auction_calls()[..], [AuctionCall::Start(Ok(_))]),
            "a wrapped item is not checked"
        );
    }

    /// `GetSelectedAuctionItem` (`0x4cfec0`) answers 1 for a type that is none of the three
    /// (`0x4cffc5` → `0x4d0013`, the double 1.0), and raises its usage line for a non-string
    /// (`0x4cfecc`, `0x8495e4`); a number is a type's text.
    #[test]
    fn an_unknown_selection_type_answers_one() {
        let mut s = UiScript::new().unwrap();
        s.set_auction(page(&[11]));
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("foo")"#)
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>("return GetSelectedAuctionItem(5)").unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("LIST")"#)
                .unwrap(),
            0,
            "a known type with no selection"
        );
        for call in ["GetSelectedAuctionItem()", "GetSelectedAuctionItem({})"] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(
                err.contains(r#"Usage: GetSelectedAuctionItem("type")"#),
                "{call}: {err}"
            );
        }
    }

    /// `0x644bc0` answers 0 unless the run time is one of the three at `0x80e4b0`
    /// (`0x644bd0`-`0x644be1`), and multiplies the rate in 32 bits (`0x644be9`).
    #[test]
    fn the_deposit_is_zero_off_the_three_run_times() {
        for minutes in [0, 60, 119, 121, 240, 479, 960, 1439, 1441, 2880] {
            assert_eq!(deposit_for(5, 10_000, minutes), 0, "{minutes} minutes");
        }
        assert_eq!(deposit_for(5, 10_000, 480), 2_000);
        // 25 × 200,000,000 wraps to 705,032,704 (`imul ecx,edx`), over 100.
        assert_eq!(deposit_for(25, 200_000_000, 120), 7_050_327);
    }

    /// `CalculateAuctionDeposit(240)` reads 0 where it read twice the two-hour deposit.
    #[test]
    fn calculate_auction_deposit_answers_zero_for_four_hours() {
        use crate::script::container::{ContainerSlot, ContainerState};
        use crate::script::cursor::CursorItem;
        use crate::script::ItemTemplateView;
        let mut s = UiScript::new().unwrap();
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1u32,
            ContainerSlot {
                count: 1,
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
        s.set_item_template(
            2589,
            ItemTemplateView {
                sell_price: 10_000,
                ..Default::default()
            },
        );
        s.set_auction(AuctionState {
            deposit_percent: 5,
            ..Default::default()
        });
        s.model_mut().auction_sell_item = Some(CursorItem {
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
        let deposit = |s: &UiScript, minutes: u32| {
            s.eval::<i64>(&format!("return CalculateAuctionDeposit({minutes})"))
                .unwrap()
        };
        assert_eq!(deposit(&s, 120), 500);
        assert_eq!(deposit(&s, 240), 0);
        assert_eq!(deposit(&s, 1440), 6_000);
    }

    /// The stack's value is a 32-bit `imul` of the vendor price and the count, in the deposit and
    /// in `GetAuctionSellItemInfo` (`0x4ce295`, `0x4ce703`): `0x80000001 × 2` is 2.
    #[test]
    fn the_stack_value_is_a_32_bit_product() {
        use crate::script::container::{ContainerSlot, ContainerState};
        use crate::script::cursor::CursorItem;
        use crate::script::ItemTemplateView;
        let mut s = UiScript::new().unwrap();
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1u32,
            ContainerSlot {
                count: 2,
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
        s.set_item_template(
            2589,
            ItemTemplateView {
                sell_price: 0x8000_0001,
                ..Default::default()
            },
        );
        s.set_auction(AuctionState {
            deposit_percent: 100,
            ..Default::default()
        });
        s.model_mut().auction_sell_item = Some(CursorItem {
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
        assert_eq!(
            s.eval::<i64>("local _, _, _, _, _, price = GetAuctionSellItemInfo() return price")
                .unwrap(),
            2
        );
        assert_eq!(
            s.eval::<i64>("return CalculateAuctionDeposit(120)")
                .unwrap(),
            2
        );
    }

    /// A bid on a row: 100c to start, 500c bid with a 25c step, a 2000c buyout, from someone else.
    fn bid(amount: u32) -> AuctionBid {
        AuctionBid {
            list: LIST,
            auction_id: 7,
            amount,
            item_id: 2589,
            min_bid: 100,
            increment: 25,
            buyout: 2000,
            current: 500,
            player_leads: false,
        }
    }

    /// `PlaceAuctionBid`'s checks in their order (`0x4cf762`-`0x4cf8a8`): the buyout clamp, the
    /// start bid, the current bid, the step unless the buyout is reached, the silent cap, the
    /// money (only the rise when the player leads) and the unique item.
    #[test]
    fn a_bid_is_checked_in_the_references_order() {
        use BidVerdict::{Drop, Refuse, Send};
        let rich = u32::MAX;
        let none = || None;
        assert_eq!(
            bid(5000).verdict(rich, none),
            Send(2000),
            "lowered to the buyout"
        );
        let mut b = bid(2500);
        b.buyout = 0;
        assert_eq!(b.verdict(rich, none), Send(2500), "no buyout, no clamp");
        let mut b = bid(99);
        b.current = 0;
        assert_eq!(b.verdict(rich, none), Refuse("ERR_AUCTION_MIN_BID"));
        assert_eq!(
            bid(99).verdict(rich, none),
            Refuse("ERR_AUCTION_MIN_BID"),
            "the start bid is tested before the current one"
        );
        assert_eq!(
            bid(500).verdict(rich, none),
            Refuse("ERR_AUCTION_HIGHER_BID")
        );
        assert_eq!(
            bid(524).verdict(rich, none),
            Refuse("ERR_AUCTION_BID_INCREMENT")
        );
        assert_eq!(bid(525).verdict(rich, none), Send(525));
        // A buyout under the step: reaching it passes.
        let mut b = bid(510);
        b.buyout = 510;
        assert_eq!(b.verdict(rich, none), Send(510));
        let mut b = bid(100);
        b.current = 0;
        b.increment = 0;
        assert_eq!(
            b.verdict(rich, none),
            Send(100),
            "no bid yet: the step is not asked"
        );
        // Past 2,000,000,000 and the step: nothing at all.
        let mut b = bid(2_000_000_001);
        b.buyout = 0;
        assert_eq!(b.verdict(rich, none), Drop);
        b.current = 2_000_000_000;
        b.increment = 1;
        assert_eq!(
            b.verdict(rich, none),
            Send(2_000_000_001),
            "not past the step"
        );
        assert_eq!(bid(525).verdict(524, none), Refuse("ERR_NOT_ENOUGH_MONEY"));
        let mut b = bid(525);
        b.player_leads = true;
        assert_eq!(b.verdict(25, none), Send(525), "the rise alone");
        assert_eq!(b.verdict(24, none), Refuse("ERR_NOT_ENOUGH_MONEY"));
        assert_eq!(
            bid(525).verdict(rich, || Some((1, 1))),
            Refuse("ERR_ITEM_MAX_COUNT")
        );
        assert_eq!(bid(525).verdict(rich, || Some((2, 1))), Send(525));
        assert_eq!(
            bid(525).verdict(rich, || Some((0, 9))),
            Send(525),
            "no limit"
        );
        let mut asked = false;
        let _ = bid(525).verdict(0, || {
            asked = true;
            None
        });
        assert!(!asked, "the unique check comes after the money's");
    }

    /// `CancelAuction`'s fee (`0x4d0338`-`0x4d0341`): 5% of the bid, truncated, with the `f32`
    /// 0.05 a hair above it.
    #[test]
    fn the_cancel_fee_is_five_percent_truncated() {
        assert_eq!(cancel_fee(0), 0);
        assert_eq!(cancel_fee(19), 0);
        assert_eq!(cancel_fee(20), 1);
        assert_eq!(cancel_fee(39), 1);
        assert_eq!(cancel_fee(1_000_000), 50_000);
        assert_eq!(cancel_fee(u32::MAX), 214_748_367);
    }

    /// `PlaceAuctionBid` and `CancelAuction` hand the host the record at the row as the call
    /// finds it (`0x4cf6b7`-`0x4cf716`, `0x4d02c5`-`0x4d02cb`); no row, no call.
    #[test]
    fn the_bid_and_cancel_verbs_hand_the_host_their_row() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut state = page(&[7]);
        let r = &mut state.lists[LIST].rows[0];
        (r.min_bid, r.min_increment, r.buyout_price, r.bid_amount) = (100, 25, 2000, 500);
        r.bidder_rank = 2;
        state.lists[OWNER] = state.lists[LIST].clone();
        s.set_auction(state);
        s.run(r#"PlaceAuctionBid("list", 1, 600.9) PlaceAuctionBid("list", 2, 600)"#)
            .unwrap();
        s.run("CancelAuction(1) CancelAuction(2) CancelAuction(0)")
            .unwrap();
        assert_eq!(
            s.take_auction_calls(),
            [
                AuctionCall::Bid(AuctionBid {
                    list: LIST,
                    auction_id: 7,
                    amount: 600,
                    item_id: 1007,
                    min_bid: 100,
                    increment: 25,
                    buyout: 2000,
                    current: 500,
                    player_leads: true,
                }),
                AuctionCall::Cancel {
                    auction_id: 7,
                    bid: 500
                },
            ]
        );
    }

    /// `GetAuctionItemInfo` asks the name cache for an owned row's bidder (`0x4cf167`) and then the
    /// seller (`0x4cf1c8`) when either misses, with the list's callback; a cached name, a row
    /// with no bid to name, or a row whose item is not cached (`0x4cefa0`) asks nothing.
    #[test]
    fn the_row_read_asks_the_names_it_misses() {
        let mut s = UiScript::new().unwrap();
        let mut state = page(&[1, 2, 3]);
        state.lists[OWNER] = state.lists[LIST].clone();
        let rows = &mut state.lists[OWNER].rows;
        // Owned with a bid: neither name known.
        rows[0].named_bidder = 0x99;
        rows[0].owner_guid = 0x77;
        // Named already.
        rows[1].owner_guid = 0x55;
        rows[1].owner = Some("Seller".into());
        rows[1].named_bidder = 0x98;
        rows[1].high_bidder = AuctionHighBidder::Name("Bidder".into());
        // Uncached item: the miss shape, before any name.
        rows[2].owner_guid = 0x56;
        rows[2].quality = None;
        s.set_auction(state);
        s.run(
            r#"for i = 1, 3 do GetAuctionItemInfo("owner", i) end GetAuctionItemInfo("owner", 1)"#,
        )
        .unwrap();
        assert_eq!(s.take_auction_name_asks(), [(0x99, OWNER), (0x77, OWNER)]);
        s.run(r#"GetAuctionItemInfo("owner", 1)"#).unwrap();
        assert_eq!(
            s.take_auction_name_asks(),
            [(0x99, OWNER), (0x77, OWNER)],
            "every read asks"
        );
        assert!(s.take_auction_name_asks().is_empty());
    }
}
