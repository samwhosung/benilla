//! The auction window's app side: [`net`] writes the wire into [`AuctionOpen`] and does in each
//! handler what the reference's handler shows and signals, [`feed_auction`] pushes the pages into
//! the VM and fires their events, the call host applies the verbs inside their calls
//! ([`AuctionVerbs`]), and [`drain_auction`] follows the header clicks and the close verb.
//!
//! The hello reply opens the window, not the click (`0x4cd570` fires `AUCTION_HOUSE_SHOW` inside
//! the hello handler). Browse, Bids and Auctions are independent lists, as in the reference's
//! three arrays (`0x4cc0f0`), each one 50-row page with its own match count, sort stack and update
//! event. The lists outlive the session: the hello empties them, the close leaves them, and the
//! world-enter cascade empties them again (`0x4cc280`). The window is on the interaction latch: walking out of the 5.56 yd service range closes
//! it with no packet (`[0xb72410]` holds 30.864, that radius squared), and so does any other NPC
//! window's open.
//!
//! The VM sorts the lists ([`AuctionSortStack`]); the app pushes each page in wire order, runs a
//! page's sort tail once its items are known, and keeps the stacks across sessions and VMs.

use bevy::prelude::*;

use benilla_protocol::messages::AuctionListEntry;
use benilla_ui::auction_sort::{sort_column, AuctionSortStack};
use benilla_ui::script::{
    AuctionCategory, AuctionHighBidder, AuctionItemRow, AuctionListState, AuctionRowBid,
    AuctionState, AuctionSubCategory, UiScript, BIDDER, LIST, OWNER,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, SelfPlayer};
use crate::query_cache::Ask;
use crate::script_calls::VmWith;
use crate::ui_action::{show_messages, ui_error_text, MessageSink, Shown, UiError};
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{CoreCloses, LatchApp, LatchedWindow, NpcWindow};

mod net;
mod verbs;

pub(crate) use verbs::AuctionVerbs;

#[cfg(test)]
mod sort_tests;
#[cfg(test)]
mod verb_tests;

/// The browse rate limit: `0x4ce980` arms `tick + 0x1388` once sent; a refused query fires nothing.
pub(super) const QUERY_THROTTLE_SECS: f64 = 5.0;

/// `ItemSubClass.dbc` `DisplayFlags` bit 1: left out of the auction category filter (`0x4cf9c0`).
const SUBCLASS_HIDDEN_FROM_AUCTIONS: u32 = 0x2;

/// The reference's `0x807060` rows `{itemClassId, hasSubclassFilter}` in menu order; a class with
/// the flag clear offers no subclass or slot rows. The names come from `ItemClass.dbc`.
const AUCTION_CLASSES: [(u32, bool); 10] = [
    (2, true),   // Weapon
    (4, true),   // Armor
    (1, true),   // Container
    (0, false),  // Consumable
    (7, false),  // Trade Goods
    (6, true),   // Projectile
    (11, true),  // Quiver
    (9, true),   // Recipe
    (5, false),  // Reagent
    (15, false), // Miscellaneous
];

/// `ItemSubClass.dbc` `Flags` bit `0x200`: the subclass offers the inventory-slot rows beneath it
/// (`GetAuctionInvTypes`, `0x4cfb63`). Set on Armor 0 to 4 only, not Shield, Libram, Idol or Totem.
const SUBCLASS_OFFERS_INV_TYPES: u32 = 0x200;

/// The `AuctionHouse.dbc` catalog; without it the sell pane shows no deposit.
#[derive(Resource)]
pub(crate) struct AuctionHouses(pub(crate) benilla_formats::AuctionHouseCatalog);

/// One of the three lists.
#[derive(Debug, Default, Clone)]
pub(crate) struct AuctionListSlot {
    /// The current page in wire order.
    pub(crate) entries: Vec<AuctionListEntry>,
    /// `totalCount`, the match count before the page cap; the pager reads it.
    pub(crate) total: u32,
    /// Which result `entries` came from, from [`AuctionOpen::next_page`].
    page: u32,
    /// The millisecond tick the result was read at (`0x4cc865`), each row's deadline base, on
    /// `GetTime`'s clock as `GetAuctionItemTimeLeft` reads it: the feed stamps it the frame the
    /// result lands, before any Lua reads the page.
    arrived_ms: Option<u32>,
    /// The page's sort tail is owed until every one of these item entries is answered: the
    /// reference's pending-lookups counter is not yet back at zero (`0xb72644`, `0xb72648`,
    /// `0xb7264c`), so the list holds its event. The hello leaves it (`0x4cd570` writes no
    /// counter); the world-enter reset zeroes it (`0x4cc2c3`-`0x4cc2cd`).
    awaiting: Option<Vec<u32>>,
}

/// The three lists' key stacks as the app keeps them: seeded at world entry (`0x48f5f9` →
/// `0x4cc0f0`), promoted as the VM reports each header click, and taken by a VM at its first
/// push. Nothing else writes them (`0x4cc238`-`0x4cc274`, the three drivers), so they outlast the
/// window, the auctioneer and a `/reload`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SortStacks(pub(crate) [AuctionSortStack; 3]);

impl Default for SortStacks {
    fn default() -> Self {
        Self(AuctionSortStack::seeded_all())
    }
}

/// What the live probe (`crate::capture::ProbeAuctionPlugin`) reads to tell an empty answer from
/// none; the client never reads it, and [`AuctionOpen::clear`] does not reset it.
#[derive(Default, Clone)]
pub(crate) struct AuctionWireLog {
    /// List results landed, per list, in [`LIST`]/[`BIDDER`]/[`OWNER`] order.
    pub(crate) list_results: [u32; 3],
    /// Browse queries that went out; a throttled one does not count.
    pub(crate) browse_sent: u32,
    /// The latest `SMSG_AUCTION_COMMAND_RESULT` as `(auction_id, action, error)`.
    pub(crate) last_command: Option<(u32, u32, u32)>,
}

/// The auctioneer session, closed by its close core and on disconnect, and the three lists, which
/// outlive it.
#[derive(Resource, Default, Clone)]
pub(crate) struct AuctionOpen {
    pub(crate) auctioneer: Option<u64>,
    /// The `AuctionHouse.dbc` row (1..=7), which keys the deposit rate.
    pub(crate) house_id: u32,
    pub(crate) lists: [AuctionListSlot; 3],
    /// Fire `AUCTION_HOUSE_SHOW` next feed: a hello reply landed.
    show_requested: bool,
    /// Per list, a result landed this frame: the feed's lookups of its items stand for the
    /// result handler's, which pass a callback (`0x4cc9bf`, `0x4ccbff`, `0x4cce4f`).
    list_result_landed: [bool; 3],
    /// The `Time::elapsed_secs_f64` before which a browse query is refused (`0xb72638`): the
    /// hello and the world-enter reset clear it, the close leaves it.
    pub(super) query_gate: Option<f64>,
    /// What the handler now running did that the VM sees, in its order, which the handler applies
    /// before it returns ([`HouseEffect`]).
    pub(crate) effects: Vec<HouseEffect>,
    /// The `_S` lines a handler could not fill, their item not cached, which [`feed_auction`]
    /// shows as each record lands; [`Self::clear`] keeps them, since a sale notice can land after
    /// a close.
    pub(crate) messages: Vec<AuctionMessage>,
    /// How many of `messages`, from the front, a feed has read: the rest came since, each with its
    /// notification's lookup with a callback (`0x4ccfa2`, `0x4cd41f`, `0x4cd51a`).
    messages_asked: usize,
    /// The one-shot request flags, owner (`0xb7263c`) and bidder (`0xb72640`): the hello arms both
    /// (`0x4cd5a8`, `0x4cd5ae`), as do an owner or bidder result whose count is not its total
    /// (`0x4ccc35`, `0x4cce85`) and a successful start or bid (`0x4cc4da`, `0x4cc522`); the close
    /// (`0x4cd624`, `0x4cd629`), the world-enter reset (`0x4cc2b9`, `0x4cc2be`) and the send
    /// (`0x4cd6e9`, `0x4cd814`) clear them.
    pub(crate) owner_armed: bool,
    pub(crate) bidder_armed: bool,
    /// The offsets the owner and bidder requests last went out at (`0xb72650`, `0xb72654`), which a
    /// successful start or bid asks again; the hello zeroes them (`0x4cd5d2`, `0x4cd5d7`), the
    /// reset leaves them.
    owner_offset: u32,
    bidder_offset: u32,
    /// The outbid list, `{house, auction}` pairs (`0xb72594`, count `0xb72634`): a bidder notice
    /// that we were outbid appends (`0x4cde50`), a placed bid removes its auction (`0x4cdf30`), and
    /// the bidder request sends the open house's ids (`0x4cd7a4`-`0x4cd7e8`) and removes a pair of
    /// that house (`0x4cdf90`), and each world entry's start-up empties it (`0x4cc1d0`).
    pub(crate) outbid: Vec<(u32, u32)>,
    /// How often a full append has doubled the list's room (`0xb72590`), eight pairs at each
    /// world entry's start-up (`0x4cc207`).
    outbid_doublings: u32,
    /// The amount the last `PlaceAuctionBid` sent (`0xb72658`, stored at `0x4cf91e`), which a
    /// placed bid's result writes into its browse row (`0x4cc56b`).
    pub(crate) last_bid: u32,
    /// The count [`AuctionListSlot::page`] is drawn from; never reset, so no two results share one.
    next_page: u32,
    /// The key stacks; neither [`Self::open`] nor [`Self::clear`] touches them.
    pub(crate) sorts: SortStacks,
    pub(crate) wire: AuctionWireLog,
}

/// How far past the tick a bid notice pushes a deadline at least (`0x15f90`, `0x4cc5a2`).
const DEADLINE_FLOOR_MS: u32 = 90_000;

/// The outbid list's room at start-up (`0x4cc213`), doubled by each full append.
const OUTBID_ROOM: usize = 8;

/// What a bid result or notice writes into a cached row: the bid (`+0x40`), the increment
/// (`+0x2c`) when the packet carries it, the high bidder (`+0x38`), and, with `floor_at` the tick
/// now, the deadline (`+0x34`) raised to at least 90 s after it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowPatch {
    pub(crate) bid: u32,
    pub(crate) increment: Option<u32>,
    pub(crate) bidder: u64,
    pub(crate) floor_at: Option<u32>,
}

/// What a handler did that the VM sees, in the handler's order: the reference's handlers show their
/// lines and signal each list as they go, so the VM's half of each step is done before the next.
#[derive(Debug, Clone)]
pub(crate) enum HouseEffect {
    /// A line shown (`0x496720`).
    Line(AuctionMessage),
    /// `list`'s row for the auction took a bid ([`AuctionOpen::patch_row`]).
    Edit { list: usize, auction_id: u32 },
    /// The auction left the lists, each on the page it held it on ([`AuctionOpen::remove_auction`]).
    Remove {
        auction_id: u32,
        pages: [Option<u32>; 3],
    },
    /// `list`'s new page, pushed with its tail when every item is known ([`AuctionOpen::land_page`]).
    Page(usize),
    /// A packet the handler sends at this point: a list request (`0x4cc4e0`, `0x4cc528`) or the
    /// placed bid's wave after its row's update (`0x4cc5dd`-`0x4cc60c`).
    Send(ClientCommand),
}

/// A message an auction handler shows (`0x496720`): a refusal on the red `UIErrorsFrame`, or one of
/// the eight outcomes (`0x178`-`0x17f`, kind 0) in chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuctionMessage {
    pub(crate) key: &'static str,
    /// The entry whose name fills the `%s`. The reference fills a plain name, never a link
    /// (`0x5d8b00`), with its random suffix; this carries no roll, so the name is unsuffixed.
    pub(crate) item: Option<u32>,
}

impl AuctionMessage {
    /// The line as the VM's strings word it, its `%s` the item's name `fill`.
    pub(crate) fn shown(&self, script: &UiScript, fill: Option<String>) -> Option<Shown> {
        let get = |key: &str| script.lua().globals().get::<String>(key).ok();
        let err = match fill {
            Some(s) => UiError::s(self.key, s),
            None => UiError::key(self.key),
        };
        ui_error_text(&err, &get).map(|text| Shown::keyed(self.key, text))
    }

    pub(crate) fn chat(key: &'static str) -> Self {
        Self { key, item: None }
    }

    /// A line naming one item: the five `_S` outcomes.
    pub(crate) fn chat_item(key: &'static str, item: u32) -> Self {
        Self {
            key,
            item: Some(item),
        }
    }

    /// A refusal, which takes no fill: the command result pushes the id alone (`0x4cc79e`).
    pub(crate) fn error(key: &'static str) -> Self {
        Self { key, item: None }
    }
}

impl AuctionOpen {
    /// The hello reply (`0x4cd570`): the auctioneer and house stored, the browse gate zeroed
    /// (`0x4cd5a3`), both request flags armed (`0x4cd5a8`, `0x4cd5ae`), every list emptied
    /// (`0x4cd5b4`-`0x4cd5cd`) and the saved offsets zeroed (`0x4cd5d2`, `0x4cd5d7`), from the
    /// auctioneer already open as from any other; the show re-fires on every greeting. A list's
    /// owed tail, the sort stacks, the selection and the outbid list stay.
    pub(crate) fn open(&mut self, auctioneer: u64, house_id: u32) {
        self.auctioneer = Some(auctioneer);
        self.house_id = house_id;
        self.empty_lists();
        self.query_gate = None;
        self.show_requested = true;
        self.owner_armed = true;
        self.bidder_armed = true;
        self.owner_offset = 0;
        self.bidder_offset = 0;
    }

    /// Every list's count and total zeroed, as the hello and the world-enter reset write them.
    /// Each emptied list is a page of its own, which the VM takes as it comes.
    fn empty_lists(&mut self) {
        for slot in &mut self.lists {
            self.next_page = self.next_page.wrapping_add(1);
            slot.entries.clear();
            slot.total = 0;
            slot.page = self.next_page;
        }
    }

    /// Replace one list's page; that list's event is owed even for an identical page.
    pub(crate) fn set_list(&mut self, which: usize, entries: Vec<AuctionListEntry>, total: u32) {
        self.list_result_landed[which] = true;
        self.next_page = self.next_page.wrapping_add(1);
        let slot = &mut self.lists[which];
        slot.entries = entries;
        slot.total = total.max(slot.entries.len() as u32);
        slot.page = self.next_page;
        slot.arrived_ms = None;
        slot.awaiting = Some(slot.entries.iter().map(|e| e.item_entry).collect());
    }

    /// A line the handler shows, in its order ([`HouseEffect::Line`]).
    pub(crate) fn line(&mut self, msg: AuctionMessage) {
        self.effects.push(HouseEffect::Line(msg));
    }

    /// `0x4cdfe0`: the auction leaves each list that holds it, the later rows moving up and the
    /// total kept; the handler removes it from the VM's lists that hold the same page, each
    /// signalled there ([`UiScript::remove_auction_from`]). It checks no session, so a closed
    /// window's lists change too.
    pub(crate) fn remove_auction(&mut self, auction_id: u32) {
        let mut pages = [None; 3];
        for (slot, page) in self.lists.iter_mut().zip(&mut pages) {
            if let Some(i) = slot.entries.iter().position(|e| e.auction_id == auction_id) {
                slot.entries.remove(i);
                *page = Some(slot.page);
            }
        }
        if pages.iter().any(Option::is_some) {
            self.effects.push(HouseEffect::Remove { auction_id, pages });
        }
    }

    /// Write a bid notice into `list`'s row for `auction_id`, if the list holds it, and owe the
    /// VM's row the same with the list's update ([`HouseEffect::Edit`]); a list that does not
    /// hold it signals nothing.
    pub(crate) fn patch_row(&mut self, list: usize, auction_id: u32, patch: &RowPatch) {
        let slot = &mut self.lists[list];
        let Some(e) = slot.entries.iter_mut().find(|e| e.auction_id == auction_id) else {
            return;
        };
        self.effects.push(HouseEffect::Edit { list, auction_id });
        e.current_bid = patch.bid;
        if let Some(increment) = patch.increment {
            e.min_increment = increment;
        }
        e.bidder_guid = patch.bidder;
        if let Some(now) = patch.floor_at {
            // `+0x34` raised to now plus 90 s when at or under it (`cmp edi,eax; jbe`), on the
            // result's tick, which this frame's feed stamps when it has not yet.
            let arrived = slot.arrived_ms.unwrap_or(now);
            let floor = now.wrapping_add(DEADLINE_FLOOR_MS);
            if arrived.wrapping_add(e.time_left_ms) <= floor {
                e.time_left_ms = floor.wrapping_sub(arrived);
            }
        }
    }

    /// The page `list` holds and what its row for `auction_id` shows of its bid, as the feed
    /// resolves it ([`row_bid`]); `None` for a row the list lacks or a page no feed has read yet.
    pub(crate) fn row_bid(
        &self,
        list: usize,
        auction_id: u32,
        self_guid: Option<u64>,
        names: &NameCache,
    ) -> Option<(u32, AuctionRowBid)> {
        let slot = &self.lists[list];
        let arrived = slot.arrived_ms?;
        let entry = slot.entries.iter().find(|e| e.auction_id == auction_id)?;
        Some((slot.page, row_bid(list, entry, arrived, self_guid, names)))
    }

    /// `0x4cde50`: the pair appended; on a full list the room doubles and the pair is written past
    /// the count without raising it (`0x4cde9a`-`0x4cdf1c`), so it is lost.
    pub(crate) fn outbid_append(&mut self, house: u32, auction_id: u32) {
        let room = OUTBID_ROOM << self.outbid_doublings;
        if self.outbid.len() >= room {
            self.outbid_doublings += 1;
            return;
        }
        self.outbid.push((house, auction_id));
    }

    /// `0x4cdf30`: the first pair of `auction_id` removed, whatever its house.
    fn outbid_remove(&mut self, auction_id: u32) {
        if let Some(i) = self.outbid.iter().position(|&(_, a)| a == auction_id) {
            self.outbid.remove(i);
        }
    }

    /// `0x4cdf90`: the first pair of `house` removed. Its loop shifts the later pairs down with the
    /// scan's own index and then steps it past the count (`0x4cdfb6`-`0x4cdfdb`), so it ends after
    /// the first match.
    fn outbid_remove_house(&mut self, house: u32) {
        if let Some(i) = self.outbid.iter().position(|&(h, _)| h == house) {
            self.outbid.remove(i);
        }
    }

    /// The owner list's sender `0x4cd680`: while the flag is armed, `CMSG_AUCTION_LIST_OWNER_ITEMS`
    /// at `offset` with the auctioneer cell as it stands, then the flag cleared and the offset
    /// saved (`0x4cd6e9`, `0x4cd6ef`); the caller sends it. It tests no auctioneer.
    pub(crate) fn owner_list(&mut self, offset: u32) -> Option<ClientCommand> {
        if !self.owner_armed {
            return None;
        }
        self.owner_armed = false;
        self.owner_offset = offset;
        Some(ClientCommand::AuctionListOwnerItems {
            auctioneer: self.auctioneer.unwrap_or(0),
            list_from: offset,
        })
    }

    /// The bidder list's sender `0x4cd720`: while the flag is armed, `CMSG_AUCTION_LIST_BIDDER_ITEMS`
    /// at `offset` with the outbid ids of the open house (`0x4cd7b6`), then a pair of that house
    /// removed (`0x4cd808`), the flag cleared and the offset saved (`0x4cd814`, `0x4cd81a`); the
    /// caller sends it.
    pub(crate) fn bidder_list(&mut self, offset: u32) -> Option<ClientCommand> {
        if !self.bidder_armed {
            return None;
        }
        let house = self.house_id;
        let auction_ids = self
            .outbid
            .iter()
            .filter(|&&(h, _)| h == house)
            .map(|&(_, a)| a)
            .collect();
        let request = ClientCommand::AuctionListBidderItems {
            auctioneer: self.auctioneer.unwrap_or(0),
            list_from: offset,
            auction_ids,
        };
        self.outbid_remove_house(house);
        self.bidder_armed = false;
        self.bidder_offset = offset;
        Some(request)
    }

    /// The close (`0x4cd5f0`), client-side, sending nothing: the auctioneer, house and request
    /// flags zeroed (`0x4cd613`-`0x4cd629`). The lists, their owed tails and the browse gate stand,
    /// as the reference leaves its counts and its throttle.
    pub(crate) fn clear(&mut self) {
        self.auctioneer = None;
        self.house_id = 0;
        self.show_requested = false;
        self.owner_armed = false;
        self.bidder_armed = false;
        self.list_result_landed = [false; 3];
    }

    /// The world-enter cascade's reset (`0x4cc280`): the session closed with its request flags
    /// (`0x4cc2b9`, `0x4cc2be`), every list emptied with its owed tail (`0x4cc296`-`0x4cc2cd`) and
    /// the browse gate zeroed (`0x4cc2b4`); the sort stacks, the saved offsets and the outbid list
    /// stay.
    pub(crate) fn reset_world_state(&mut self) {
        self.clear();
        self.empty_lists();
        for slot in &mut self.lists {
            slot.awaiting = None;
        }
        self.query_gate = None;
    }

    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

/// The auctioneer's cell `[0xb725f8]`; `CloseAuctionHouse` (`0x4ce160`) is the core itself.
impl LatchedWindow for AuctionOpen {
    const WINDOW: NpcWindow = NpcWindow::Auction;

    fn cell(&self) -> Option<u64> {
        self.auctioneer
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }

    fn take_close_verb(script: &mut UiScript) -> bool {
        script.take_auction_close()
    }
}

/// The hello reply (`0x4cc420` → `0x4cd570`): the latch first, with no test of the cell
/// (`0x4cd57f`), so a hello from the open auctioneer closes the window and opens it again; then
/// the cell (`0x4cd58c`) and the emptied lists ([`AuctionOpen::open`]).
pub(crate) fn open_auction(world: &mut World, auctioneer: u64, house_id: u32) {
    crate::ui_session::register(world, auctioneer);
    world
        .resource_mut::<AuctionOpen>()
        .open(auctioneer, house_id);
}

/// The close core `0x4cd5f0`: with an auctioneer open, `AUCTION_HOUSE_CLOSED` first (`0x4cd602`),
/// then the latch release on the cell as it stands after the fire (`0x4cd60e`), the auctioneer
/// cleared (`0x4cd613`-`0x4cd629`) with the lists left standing, and the sell slot emptied with
/// its unlock (`0x4cd62e` → `0x4cd640`). The fire comes before the clear, so `OnHide`'s `CloseAuctionHouse()` re-enters
/// with the cell set.
pub(crate) fn close_core(world: &mut World) {
    if world.resource::<AuctionOpen>().auctioneer.is_none() {
        return;
    }
    crate::ui_session::fire(world, |script| {
        script.fire_event("AUCTION_HOUSE_CLOSED", vec![]);
    });
    if let Some(auctioneer) = world.resource::<AuctionOpen>().auctioneer {
        crate::ui_session::release(world, auctioneer);
    }
    if world.resource::<AuctionOpen>().auctioneer.is_some() {
        world.resource_mut::<AuctionOpen>().clear();
        crate::ui_session::note_closed(world, NpcWindow::Auction);
        crate::ui_session::with_script(world, UiScript::close_auction_session);
    }
    crate::ui_session::fire(world, |script| script.clear_auction_sell_item());
}

pub(crate) struct UiAuctionPlugin;

impl Plugin for UiAuctionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<AuctionOpen>()
            .add_message::<crate::net::WorldEnterCascadeMessage>()
            .add_latched_window::<AuctionOpen>()
            // `0x490b25` → `0x4cc2f0` → `0x4cd5f0`, the core.
            .add_sweep_leg(SweepLeg::Auction, close_core)
            .add_systems(
                Update,
                (
                    // The feed before the input pass, and after `UnitFeed` so a row's tooltip reads
                    // a landed template; the drain after the input pass so a header click or a
                    // close lands the same frame.
                    // Only with the interface up: a sale notice landing in the login burst would
                    // otherwise be shown on the glue VM and lost.
                    feed_auction
                        .after(crate::ui_unit::UnitFeed)
                        .in_set(UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    drain_auction.in_set(crate::ui_script::UiDrain),
                ),
            )
            .add_systems(
                Update,
                reset_auction_on_world_enter
                    .in_set(UiFeed)
                    .before(feed_auction),
            )
            .add_systems(
                OnEnter(crate::client_state::ClientState::InWorld),
                start_up_at_world_entry,
            );
    }
}

/// The world-enter cascade's auction reset (`0x490a05` → `0x4cc280`), which a login, a cross-map
/// worldport and a `/reload` all run: the sell slot emptied (`0x4cd640`), the session and the
/// lists cleared ([`AuctionOpen::reset_world_state`]) and the selection with them, signalling
/// nothing else. The emptied lists reach the VM with the feed that follows.
fn reset_auction_on_world_enter(
    mut vm: VmWith<ResMut<'static, AuctionOpen>>,
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
) {
    if cascades.read().count() == 0 {
        return;
    }
    vm.own().reset_world_state();
    vm.run(|script| {
        if let Some(script) = script {
            script.clear_auction_sell_item();
            script.clear_auction_selections();
        }
    })
}

/// The world entry's start-up (`0x48f5f9` → `0x4cc0f0`), from `ClientInitializeGame`
/// (`0x401570`), which runs once per entry from the glue screens and not on a worldport: the
/// outbid list emptied (`0x4cc1d0`) with its room back at eight (`0x4cc207`), and the three
/// stacks seeded (`0x4cc236`-`0x4cc274`).
fn start_up_at_world_entry(mut auction: ResMut<AuctionOpen>) {
    auction.outbid.clear();
    auction.outbid_doublings = 0;
    auction.sorts = SortStacks::default();
}

/// The status sort's rank of a row's high bidder (`0x4cdb10`-`0x4cdb6a`): 2 when it is the player
/// (`0x468550`, 0 with no player), 1 for any other bidder, 0 for no bid.
fn bidder_rank(bidder_guid: u64, self_guid: Option<u64>) -> u8 {
    if bidder_guid == self_guid.unwrap_or(0) {
        2
    } else if bidder_guid != 0 {
        1
    } else {
        0
    }
}

/// `GetAuctionItemInfo`'s `highBidder` for a row of `list` (`0x4cf12a`–`0x4cf1a0`). On an
/// `"owner"` row the player owns (`0x4cf131`–`0x4cf142` compare its owner guid with the player's),
/// the bidder guid (vmangos `AuctionHouseMgr.cpp:840`) goes through the name cache: nil with no
/// bid (`0x4cf150`) or until the asked name lands (`0x4cf16e`), whose arrival re-fires the owned
/// list's update (`0x4cf2b0`) for an owned list that read it. Any other row: `1` if the player holds the
/// bid, else nil (`0x4cf17b`–`0x4cf1a0`).
fn high_bidder(
    list: usize,
    entry: &AuctionListEntry,
    self_guid: Option<u64>,
    names: &NameCache,
    ask: Ask<'_>,
) -> AuctionHighBidder {
    if is_owned(list, entry, self_guid) {
        if entry.bidder_guid == 0 {
            return AuctionHighBidder::Nil;
        }
        return names
            .resolve_player(entry.bidder_guid, ask)
            .map_or(AuctionHighBidder::Nil, |n| {
                AuctionHighBidder::Name(n.to_string())
            });
    }
    if self_guid.is_some_and(|g| g == entry.bidder_guid) {
        AuctionHighBidder::Player
    } else {
        AuctionHighBidder::Nil
    }
}

/// An `"owner"` row the player owns (`0x4cf131`-`0x4cf142`), whose eleventh value is a name.
fn is_owned(list: usize, entry: &AuctionListEntry, self_guid: Option<u64>) -> bool {
    list == OWNER && self_guid.is_some_and(|g| g == entry.owner_guid)
}

/// An owned row with a bid (`0x4cf150`): the bidder whose name `GetAuctionItemInfo` reads.
fn owned_with_bid(list: usize, entry: &AuctionListEntry, self_guid: Option<u64>) -> bool {
    is_owned(list, entry, self_guid) && entry.bidder_guid != 0
}

/// One wire entry of `list` as a display row, through the ask-once template and name caches; a
/// `None` fills in when its answer lands. The link carries the row's enchant, property and suffix.
/// The item is read as `ask` says, the list's packet looking each up with a callback (`0x4cc9bf`,
/// `0x4ccbff`, `0x4cce4f`); the names only from the cache.
fn resolve_row(
    list: usize,
    entry: &AuctionListEntry,
    arrived_ms: u32,
    self_guid: Option<u64>,
    items: &Items,
    icons: Option<&ItemDisplays>,
    names: &NameCache,
    ask: Ask<'_>,
    rolls: crate::items::RollCatalogs,
) -> AuctionItemRow {
    let template = items.template(entry.item_entry, 0, ask);
    let bid = row_bid(list, entry, arrived_ms, self_guid, names);
    let roll = entry.random_property_id as u32;
    let name = template.map(|t| rolls.name(&t.name, roll));
    let quality = template.map(|t| t.quality);
    let level = template.map_or(0, |t| t.required_level);
    let texture = crate::ui_items::item_icon(icons, template.map_or(0, |t| t.display_info_id));
    let link = name.as_ref().map(|n| {
        crate::ui_items::item_link_full(
            entry.item_entry,
            entry.perm_enchant,
            roll,
            entry.suffix_factor,
            n,
            quality.unwrap_or(0),
        )
    });

    AuctionItemRow {
        auction_id: entry.auction_id,
        item_id: entry.item_entry,
        random_property_id: roll,
        charges: entry.spell_charges,
        count: entry.count,
        name,
        texture,
        quality,
        level,
        min_bid: entry.start_bid,
        min_increment: bid.min_increment,
        buyout_price: entry.buyout,
        bid_amount: bid.bid_amount,
        deadline: bid.deadline,
        bidder_rank: bid.bidder_rank,
        base_name: template.map(|t| t.name.clone()),
        high_bidder: bid.high_bidder,
        // The names are read, never asked: `GetAuctionItemInfo` asks what it misses
        // ([`UiScript::take_auction_name_asks`]), and the seller sort reads the cache alone
        // (`0x4cdbda`).
        owner: names
            .resolve_player(entry.owner_guid, Ask::Read)
            .map(str::to_string),
        link,
        owner_guid: entry.owner_guid,
        named_bidder: bid.named_bidder,
    }
}

/// What a list's rows read beside the house: the player, the item and name caches, the icons and
/// the random suffixes.
pub(crate) struct ListReads<'a> {
    pub(crate) self_guid: Option<u64>,
    pub(crate) items: &'a Items,
    pub(crate) icons: Option<&'a ItemDisplays>,
    pub(crate) names: &'a NameCache,
    pub(crate) rolls: crate::items::RollCatalogs<'a>,
}

impl AuctionOpen {
    /// `list` as the VM shows it: each row resolved ([`resolve_row`]), each item read as `ask`
    /// says, and the page owed its tail while an item is out.
    pub(crate) fn list_state(
        &self,
        list: usize,
        tick: u32,
        reads: &ListReads,
        ask: Ask<'_>,
    ) -> AuctionListState {
        let slot = &self.lists[list];
        let rows: Vec<AuctionItemRow> = slot
            .entries
            .iter()
            .map(|e| {
                resolve_row(
                    list,
                    e,
                    slot.arrived_ms.unwrap_or(tick),
                    reads.self_guid,
                    reads.items,
                    reads.icons,
                    reads.names,
                    ask,
                    reads.rolls,
                )
            })
            .collect();
        AuctionListState {
            total: slot.total.max(rows.len() as u32),
            rows,
            sort: self.sorts.0[list],
            page: slot.page,
            pending: slot.awaiting.is_some(),
            ..Default::default()
        }
    }

    /// A list result's own step in its handler (`0x4cc7f0`, `0x4cca30`, `0x4ccc80`): the page
    /// stamped with the tick, and its tail run now when every item is already known, as the
    /// handler's own count reaching 0 runs it (`0x4cc9e8`-`0x4cc9f1`); else it waits for the
    /// last record (`0x4cca10`). Whether the tail ran.
    pub(crate) fn land_page(&mut self, list: usize, tick: u32, items: &Items) -> bool {
        let slot = &mut self.lists[list];
        slot.arrived_ms.get_or_insert(tick);
        let known = slot
            .awaiting
            .as_ref()
            .is_some_and(|items_of| items_of.iter().all(|&e| items.template_answered(e)));
        if known {
            slot.awaiting = None;
        }
        known
    }
}

/// What a row of `list` shows of its bid, from the wire entry as the last result or patch left it:
/// what a bid patch rewrites in the VM's row, so the handler's write and the feed's push agree.
fn row_bid(
    list: usize,
    entry: &AuctionListEntry,
    arrived_ms: u32,
    self_guid: Option<u64>,
    names: &NameCache,
) -> AuctionRowBid {
    AuctionRowBid {
        bid_amount: entry.current_bid,
        min_increment: entry.min_increment,
        // The wire's milliseconds are unclamped (`AuctionHouseMgr.cpp:838`): one gone negative
        // wraps the deadline into the past, as the reference's add does.
        deadline: arrived_ms.wrapping_add(entry.time_left_ms),
        bidder_rank: bidder_rank(entry.bidder_guid, self_guid),
        high_bidder: high_bidder(list, entry, self_guid, names, Ask::Read),
        named_bidder: if owned_with_bid(list, entry, self_guid) {
            entry.bidder_guid
        } else {
            0
        },
    }
}

/// The Browse tab's category tree from the player's own DBCs; the addon-corpus survey reuses it.
pub(crate) fn categories(
    classes: Option<&crate::ui_items::ItemClasses>,
    subclasses: Option<&crate::ui_items::ItemSubClasses>,
) -> Vec<AuctionCategory> {
    let (Some(classes), Some(subclasses)) = (classes, subclasses) else {
        return Vec::new();
    };
    AUCTION_CLASSES
        .iter()
        .filter_map(|&(class_id, has_subclass_filter)| {
            let name = classes.0.name(class_id)?.to_string();
            let subs = subclasses
                .0
                .subclasses_of(class_id)
                .into_iter()
                .filter(|&sub| {
                    subclasses.0.display_flags(class_id, sub) & SUBCLASS_HIDDEN_FROM_AUCTIONS == 0
                })
                .filter_map(|sub| {
                    Some(AuctionSubCategory {
                        sub_id: sub,
                        name: subclasses.0.name(class_id, sub)?.to_string(),
                        has_inv_types: subclasses.0.flags(class_id, sub)
                            & SUBCLASS_OFFERS_INV_TYPES
                            != 0,
                    })
                })
                .collect();
            Some(AuctionCategory {
                class_id,
                name,
                has_subclass_filter,
                subclasses: subs,
            })
        })
        .collect()
}

/// The DBC catalogs as one system param, since [`feed_auction`] is at Bevy's 16-param limit: the
/// class tables for the category tree and the random-suffix pair for a rolled listing.
type AuctionCatalogs<'w> = (
    Option<Res<'w, crate::ui_items::ItemClasses>>,
    Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    Option<Res<'w, crate::items::RandomProperties>>,
    Option<Res<'w, crate::items::Enchants>>,
);

/// Push the auction house into the VM and fire its events on a change, diffed against `VmMemo`s:
/// an engine turn, since the house is the call host's too, whose events' handlers run as it ends.
fn feed_auction(
    mut vm: VmWith<ResMut<'static, AuctionOpen>>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    time: Res<Time>,
    catalogs: AuctionCatalogs,
    houses: Option<Res<AuctionHouses>>,
    mut sink: MessageSink,
    self_q: Query<&crate::net::Guid, With<SelfPlayer>>,
    mut last: crate::ui_script::WindowMemo<Option<AuctionState>>,
    mut last_open: crate::ui_script::WindowMemo<Option<u64>>,
    mut last_can_query: Local<crate::ui_script::VmMemo<bool>>,
    mut last_classes: Local<crate::ui_script::VmMemo<Vec<AuctionCategory>>>,
    mut name_waits: Local<crate::ui_script::VmMemo<Vec<(u64, usize)>>>,
    mut closes: CoreCloses,
) {
    vm.turn(|script, mut auction| {
        let Some(script) = script else {
            return;
        };
        let last = last.get(script);
        let last_open = last_open.get(script);
        if closes.since_last(script, NpcWindow::Auction) {
            *last = None;
            *last_open = None;
        }
        let last_can_query = last_can_query.get(script);

        // The class tree needs no session: the stock `AuctionFrameBrowse_OnLoad` reads
        // `GetAuctionItemClasses()` when the addon loads (`Blizzard_AuctionUI.lua:174`).
        let classes_now = categories(catalogs.0.as_deref(), catalogs.1.as_deref());
        let last_classes = last_classes.get(script);
        if *last_classes != classes_now {
            *last_classes = classes_now.clone();
            script.set_auction_item_classes(classes_now);
        }

        // A `_S` line whose item was not cached when its handler ran, shown once the record lands,
        // as the reference defers it through `0x4cd190`; the `items.template` call below asks for
        // it.
        {
            let mut deferred = Vec::new();
            let mut lines = Vec::new();
            let asked = std::mem::take(&mut auction.messages_asked);
            for (i, msg) in std::mem::take(&mut auction.messages)
                .into_iter()
                .enumerate()
            {
                let fill = match msg.item {
                    None => None,
                    Some(entry) => match items.template(entry, 0, Ask::at(i >= asked, &commands)) {
                        Some(t) => Some(t.name.clone()),
                        None => {
                            deferred.push(msg);
                            continue;
                        }
                    },
                };
                lines.extend(msg.shown(script, fill));
            }
            auction.messages_asked = deferred.len();
            auction.messages = deferred;
            show_messages(script, &mut sink, "ui_auction", lines);
        }

        // The names `GetAuctionItemInfo` missed, asked with the reading list's callback
        // (`0x4cf167`, `0x4cf1c8`), kept once per name and list (`0x55f080`'s dedupe flag 1) until
        // the name lands.
        let name_waits = name_waits.get(script);
        for (guid, list) in script.take_auction_name_asks() {
            let _ = names.resolve_player(guid, Ask::Event(&commands));
            if !name_waits.contains(&(guid, list)) {
                name_waits.push((guid, list));
            }
        }

        let self_guid = self_q.iter().next().map(|g| g.0);
        let rolls = crate::items::RollCatalogs {
            props: catalogs.2.as_deref(),
            enchants: catalogs.3.as_deref(),
        };
        // A list's result, and the window's show, are the frames the stock tabs call their getters
        // on.
        let shown = last_open.is_none() || auction.show_requested;
        // A landed result's tick, on the clock `GetAuctionItemTimeLeft` reads (`0x4cc865`).
        let tick = script.auction_tick_ms();
        for slot in &mut auction.lists {
            slot.arrived_ms.get_or_insert(tick);
        }
        let lists = [LIST, BIDDER, OWNER].map(|i| {
            let ask = Ask::at(shown || auction.list_result_landed[i], &commands);
            let reads = ListReads {
                self_guid,
                items: &items,
                icons: icons.as_deref(),
                names: &names,
                rolls,
            };
            auction.list_state(i, tick, &reads, ask)
        });
        let mut fresh = AuctionState {
            lists,
            deposit_percent: houses
                .as_deref()
                .and_then(|h| h.0.deposit_percent(auction.house_id))
                .unwrap_or(0),
        };

        // A close is the core's, which told the VM; a disconnect's clear tells it nothing.
        let opened = last_open.is_none() && auction.auctioneer.is_some();
        let show_requested = std::mem::take(&mut auction.show_requested);
        auction.list_result_landed = [false; 3];
        // Per list, the sort tail once every item its page waits on is answered, a record or a
        // refusal, as the pending counter's last decrement runs it (`0x4cc9e8`, `0x4cca10`),
        // whatever the session.
        let tails: [bool; 3] = std::array::from_fn(|i| {
            auction.lists[i]
                .awaiting
                .as_ref()
                .is_some_and(|items_of| items_of.iter().all(|&e| items.template_answered(e)))
        });
        for (slot, tail) in auction.lists.iter_mut().zip(tails) {
            if tail {
                slot.awaiting = None;
            }
        }
        // A tail run now leaves its list with nothing owed.
        for (list, slot) in fresh.lists.iter_mut().zip(&auction.lists) {
            list.pending = slot.awaiting.is_some();
        }
        if last.as_ref() != Some(&fresh) {
            script.set_auction(fresh.clone());
        }
        if opened || (auction.auctioneer.is_some() && show_requested) {
            script.fire_event("AUCTION_HOUSE_SHOW", vec![]);
        }
        for list in [LIST, BIDDER, OWNER] {
            if tails[list] {
                script.sort_auction_list(list);
            }
        }
        // Each landed name's callbacks, in the order they were kept: the reading list's update,
        // with no pending test and sorting nothing (`0x4cf290`, `0x4cf2b0`, `0x4cf2d0`).
        let landed: Vec<usize> = {
            let mut landed = Vec::new();
            name_waits.retain(|&(guid, list)| {
                let named = names.resolve_player(guid, Ask::Read).is_some();
                if named {
                    landed.push(list);
                }
                !named
            });
            landed
        };
        for list in landed {
            // Literal names: `reference_ui`'s producer test finds fire sites by literal.
            match list {
                LIST => script.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]),
                BIDDER => script.fire_event("AUCTION_BIDDER_LIST_UPDATE", vec![]),
                _ => script.fire_event("AUCTION_OWNED_LIST_UPDATE", vec![]),
            }
        }
        *last = Some(fresh);
        *last_open = auction.auctioneer;

        // The browse gate, pushed when it changes; the Search button polls it every frame.
        let now = time.elapsed_secs_f64();
        let can_query = auction.query_gate.is_none_or(|gate| now >= gate);
        if can_query != *last_can_query {
            script.set_auction_can_query(can_query);
            *last_can_query = can_query;
        }
    })
}

/// The header clicks into the app's copy of the stacks, and the close verb into its core; every
/// other verb applies inside its call ([`AuctionVerbs`]).
fn drain_auction(mut vm: VmWith<ResMut<'static, AuctionOpen>>, mut world_commands: Commands) {
    vm.turn(|script, mut auction| {
        let Some(script) = script else {
            return;
        };
        // The VM sorted in the verb; the app's copy of the stacks follows each click, with a
        // session or without, as the drivers run with none (`0x4cd850`). A sort sends nothing.
        for (which, key) in script.take_auction_sorts() {
            if let (Some(stack), Some(column)) = (auction.sorts.0.get_mut(which), sort_column(&key))
            {
                stack.promote(column);
            }
        }
        // With no session a stray close finds no cell (`0x4cd5fb`).
        if script.take_auction_close() && auction.auctioneer.is_some() {
            crate::ui_session::queue(&mut world_commands, close_core);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference's gates: the `0x807060` class flag, and `Flags & 0x200` (`0x4cfb63`), which
    /// Shield lacks though it is Armor.
    #[test]
    fn the_tree_carries_the_reference_gates() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).unwrap();
        let classes =
            crate::ui_items::ItemClasses(benilla_formats::load_item_classes(&mut chain).unwrap());
        let subclasses = crate::ui_items::ItemSubClasses(
            benilla_formats::load_item_sub_classes(&mut chain).unwrap(),
        );
        let tree = categories(Some(&classes), Some(&subclasses));
        let ids: Vec<(u32, bool)> = tree
            .iter()
            .map(|c| (c.class_id, c.has_subclass_filter))
            .collect();
        assert_eq!(ids, AUCTION_CLASSES.to_vec());

        let weapon = &tree[0];
        assert_eq!(weapon.subclasses[0].sub_id, 0, "Axe leads, in file order");
        assert!(
            !weapon
                .subclasses
                .iter()
                .any(|s| [9, 11, 12, 17].contains(&s.sub_id)),
            "the excluded weapon rows are not offered"
        );
        assert!(weapon.subclasses.iter().all(|s| !s.has_inv_types));

        let armor = &tree[1];
        let offering: Vec<u32> = armor
            .subclasses
            .iter()
            .filter(|s| s.has_inv_types)
            .map(|s| s.sub_id)
            .collect();
        assert_eq!(
            offering,
            vec![0, 1, 2, 3, 4],
            "not Shield, Libram, Idol, Totem"
        );

        // Consumable's flag is 0, yet its subclass list stays for the query's own scan.
        assert!(!tree[3].has_subclass_filter);
        assert!(!tree[3].subclasses.is_empty());
    }

    /// Every result runs its sort tail, which signals with no test (`0x4cd8b0`): the Browse pane
    /// leaves "Searching" only on it (`Blizzard_AuctionUI.lua:187`), even for an identical page.
    #[test]
    fn an_identical_list_result_still_owes_its_events() {
        let mut open = AuctionOpen::default();
        open.open(0x1234, 1);

        open.set_list(LIST, Vec::new(), 0);
        assert!(open.lists[LIST].awaiting.is_some(), "the first empty page");

        // The feed runs the tail and clears it.
        open.lists[LIST].awaiting = None;

        open.set_list(LIST, Vec::new(), 0);
        assert!(
            open.lists[LIST].awaiting.is_some(),
            "an identical page owes the event too — this is the whole bug"
        );
    }

    /// Only that list's tail: the Auctions tab's update needs the `page` its `OnShow` sets
    /// (`Blizzard_AuctionUI.lua:817`).
    #[test]
    fn a_result_owes_only_its_own_lists_event() {
        let mut open = AuctionOpen::default();
        open.open(0x1234, 1);
        let owed = |open: &AuctionOpen| open.lists.each_ref().map(|l| l.awaiting.is_some());

        open.set_list(LIST, Vec::new(), 0);
        assert_eq!(
            owed(&open),
            [true, false, false],
            "a browse result is not news about the player's bids or listings"
        );

        open.lists[LIST].awaiting = None;
        open.set_list(OWNER, Vec::new(), 0);
        assert_eq!(owed(&open), [false, false, true]);

        open.lists[OWNER].awaiting = None;
        open.set_list(BIDDER, Vec::new(), 0);
        assert_eq!(owed(&open), [false, true, false]);
    }

    const ME: u64 = 0x77;
    const BIDDER_GUID: u64 = 0x99;

    /// A wire row owned by `owner` with `bidder` on it (0 for no bid).
    fn entry(owner: u64, bidder: u64) -> AuctionListEntry {
        AuctionListEntry {
            auction_id: 1,
            item_entry: 2589,
            perm_enchant: 0,
            random_property_id: 0,
            suffix_factor: 0,
            count: 1,
            spell_charges: 0,
            owner_guid: owner,
            start_bid: 1000,
            min_increment: if bidder == 0 { 0 } else { 125 },
            buyout: 0,
            time_left_ms: 60_000,
            bidder_guid: bidder,
            current_bid: if bidder == 0 { 0 } else { 2500 },
        }
    }

    fn name_queries(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<u64> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::NameQuery { guid } => Some(guid),
                _ => None,
            })
            .collect()
    }

    /// The player's own auction names its bidder from the cache (`0x4cf144`–`0x4cf174`).
    #[test]
    fn an_owned_row_names_its_bidder() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut names = NameCache::default();
        names.insert_player(BIDDER_GUID, "Bidder".into(), None);
        assert_eq!(
            high_bidder(
                OWNER,
                &entry(ME, BIDDER_GUID),
                Some(ME),
                &names,
                Ask::New(&commands)
            ),
            AuctionHighBidder::Name("Bidder".into())
        );
        assert!(name_queries(&rx).is_empty(), "a cached name asks nothing");
    }

    /// An unknown bidder is nil and asked for; no bid is nil and asks nothing (`0x4cf150`).
    #[test]
    fn an_owned_row_is_nil_until_the_name_lands_and_with_no_bid() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut names = NameCache::default();
        assert_eq!(
            high_bidder(
                OWNER,
                &entry(ME, BIDDER_GUID),
                Some(ME),
                &names,
                Ask::New(&commands)
            ),
            AuctionHighBidder::Nil
        );
        assert_eq!(
            name_queries(&rx),
            vec![BIDDER_GUID],
            "the name goes out once"
        );

        assert_eq!(
            high_bidder(OWNER, &entry(ME, 0), Some(ME), &names, Ask::New(&commands)),
            AuctionHighBidder::Nil
        );
        assert!(name_queries(&rx).is_empty(), "no bidder, no query");

        names.insert_player(BIDDER_GUID, "Bidder".into(), None);
        assert_eq!(
            high_bidder(
                OWNER,
                &entry(ME, BIDDER_GUID),
                Some(ME),
                &names,
                Ask::New(&commands)
            ),
            AuctionHighBidder::Name("Bidder".into())
        );
    }

    /// Browse, Bids and an owner row the player does not own answer `1` or nil, never a name,
    /// even with the bidder's name cached (`0x4cf17b`–`0x4cf1a0`).
    #[test]
    fn other_rows_answer_one_or_nil() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut names = NameCache::default();
        names.insert_player(BIDDER_GUID, "Bidder".into(), None);
        const SELLER: u64 = 0x55;
        for list in [LIST, BIDDER] {
            assert_eq!(
                high_bidder(
                    list,
                    &entry(SELLER, ME),
                    Some(ME),
                    &names,
                    Ask::New(&commands)
                ),
                AuctionHighBidder::Player
            );
            assert_eq!(
                high_bidder(
                    list,
                    &entry(SELLER, BIDDER_GUID),
                    Some(ME),
                    &names,
                    Ask::New(&commands)
                ),
                AuctionHighBidder::Nil
            );
            assert_eq!(
                high_bidder(
                    list,
                    &entry(SELLER, 0),
                    Some(ME),
                    &names,
                    Ask::New(&commands)
                ),
                AuctionHighBidder::Nil
            );
            // The player's own auction, seen on a browse page, is not named either.
            assert_eq!(
                high_bidder(
                    list,
                    &entry(ME, BIDDER_GUID),
                    Some(ME),
                    &names,
                    Ask::New(&commands)
                ),
                AuctionHighBidder::Nil
            );
        }
        // The owner list gates on the row's owner, not the list alone (`0x4cf131`).
        assert_eq!(
            high_bidder(
                OWNER,
                &entry(SELLER, ME),
                Some(ME),
                &names,
                Ask::New(&commands)
            ),
            AuctionHighBidder::Player
        );
        assert_eq!(
            high_bidder(
                OWNER,
                &entry(SELLER, BIDDER_GUID),
                Some(ME),
                &names,
                Ask::New(&commands)
            ),
            AuctionHighBidder::Nil
        );
        assert!(name_queries(&rx).is_empty());
    }

    /// A bidder's name landing after the owned list re-fires `AUCTION_OWNED_LIST_UPDATE` and the
    /// row then answers it (the `0x4cf2b0` callback fires event `0x1a9`).
    #[test]
    fn a_landed_bidder_name_refires_the_owned_list() {
        let mut app = App::new();
        app.init_resource::<AuctionOpen>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<Time>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(ME, "Seller".into(), None);
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("AUCTION_OWNED_LIST_UPDATE")
                f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.world_mut().spawn((SelfPlayer, crate::net::Guid(ME)));
        // The row's item template is cached: an uncached row answers the miss shape, whose
        // eleventh value is nil whatever the bidder.
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
        {
            let mut open = app.world_mut().resource_mut::<AuctionOpen>();
            open.open(0x1234, 1);
            open.set_list(OWNER, vec![entry(ME, BIDDER_GUID)], 1);
        }

        // One registered system, so its `VmMemo` locals persist from frame to frame.
        let feed = app.world_mut().register_system(feed_auction);
        let step = |app: &mut App| -> (Vec<String>, Option<String>) {
            app.world_mut().run_system(feed).unwrap();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            let seen = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            let bidder = s
                .eval::<Option<String>>(
                    r#"local _,_,_,_,_,_,_,_,_,_,h = GetAuctionItemInfo("owner", 1) return h"#,
                )
                .unwrap();
            (seen, bidder)
        };

        // The window opens with the page, which asks no name; the read finds the bidder unknown,
        // so nil, and asks for it (`0x4cf167`), which the next feed sends.
        let (_, bidder) = step(&mut app);
        assert_eq!(bidder, None);
        assert!(name_queries(&rx).is_empty(), "the page asks no name");

        let (seen, bidder) = step(&mut app);
        assert!(seen.is_empty(), "nothing new, nothing fired: {seen:?}");
        assert_eq!(bidder, None);
        assert_eq!(name_queries(&rx), vec![BIDDER_GUID], "the read asked it");

        app.world_mut().resource_mut::<NameCache>().insert_player(
            BIDDER_GUID,
            "Bidder".into(),
            None,
        );
        let (seen, bidder) = step(&mut app);
        assert_eq!(bidder.as_deref(), Some("Bidder"));
        assert_eq!(seen, vec!["AUCTION_OWNED_LIST_UPDATE".to_string()]);
    }

    /// A list's packet asks no name (`0x4cca30` asks the item cache alone); `GetAuctionItemInfo`
    /// asks the bidder it misses with the list's callback (`0x4cf167`), a refused one again at
    /// each read, and a frame that reads nothing asks nothing.
    #[test]
    fn a_bidders_name_is_asked_by_each_read_and_by_no_frame() {
        let mut app = App::new();
        app.init_resource::<AuctionOpen>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<Time>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_systems(Update, feed_auction);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(ME, "Seller".into(), None);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.world_mut().spawn((SelfPlayer, crate::net::Guid(ME)));
        {
            let mut open = app.world_mut().resource_mut::<AuctionOpen>();
            open.open(0x1234, 1);
            open.set_list(OWNER, vec![entry(ME, BIDDER_GUID)], 1);
        }
        let read = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run(r#"GetAuctionItemInfo("owner", 1)"#)
                .unwrap();
            app.update();
        };
        app.update();
        assert!(name_queries(&rx).is_empty(), "the page asks no name");
        read(&mut app);
        assert_eq!(name_queries(&rx), vec![BIDDER_GUID]);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(BIDDER_GUID, String::new(), None);
        for _ in 0..600 {
            app.update();
        }
        assert!(name_queries(&rx).is_empty(), "no frame asks the refusal");
        read(&mut app);
        assert_eq!(name_queries(&rx), vec![BIDDER_GUID], "the next read does");
    }

    /// The feed and the sweep's auction leg, the feed's other inputs at rest.
    fn sweep_auction_app() -> App {
        use crate::ui_script::SweepApp;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<AuctionOpen>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<Time>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .insert_resource(NetCommands(tx))
            .add_latched_window::<AuctionOpen>()
            // `0x490b25` → `0x4cc2f0` → `0x4cd5f0`, the core.
            .add_sweep_leg(SweepLeg::Auction, close_core)
            .add_systems(Update, feed_auction);
        app
    }

    const EVENTS: [&str; 2] = ["AUCTION_HOUSE_SHOW", "AUCTION_HOUSE_CLOSED"];
    const AUCTIONEER: u64 = 0xF130_0000_0000_0A0C;

    /// A `/reload` at an open auction house: the auction leg (`0x490b25` → `0x4cd5f0`) runs the
    /// core while the auctioneer holds the latch, so the dying VM hears `AUCTION_HOUSE_CLOSED`
    /// twice and the auctioneer is cleared; the new VM shows nothing.
    #[test]
    fn a_reload_closes_the_auction_house_and_the_new_vm_shows_nothing() {
        use crate::ui_script::sweep_test::{reload_in_place, seen, seen_in, spied_vm};
        let mut app = sweep_auction_app();
        app.insert_non_send_resource(spied_vm(&EVENTS));
        open_auction(app.world_mut(), AUCTIONEER, 1);
        app.update();
        assert_eq!(seen_in(&app), ["AUCTION_HOUSE_SHOW"]);

        let dying = reload_in_place(&mut app, &EVENTS);
        assert_eq!(
            seen(&dying),
            [
                "AUCTION_HOUSE_SHOW",
                "AUCTION_HOUSE_CLOSED",
                "AUCTION_HOUSE_CLOSED"
            ]
        );
        assert_eq!(app.world().resource::<AuctionOpen>().auctioneer, None);

        app.update();
        assert!(
            seen_in(&app).is_empty(),
            "the new VM has no auction house to show"
        );
    }

    /// A worldport's surviving VM hears the leg's `AUCTION_HOUSE_CLOSED` twice, and nothing after.
    #[test]
    fn a_worldport_closes_the_auction_house_once() {
        use crate::ui_script::sweep_test::{seen_in, spied_vm, sweep_in_place};
        let mut app = sweep_auction_app();
        app.insert_non_send_resource(spied_vm(&EVENTS));
        open_auction(app.world_mut(), AUCTIONEER, 1);
        app.update();
        sweep_in_place(&mut app);
        app.update();
        app.update();
        assert_eq!(
            seen_in(&app),
            [
                "AUCTION_HOUSE_SHOW",
                "AUCTION_HOUSE_CLOSED",
                "AUCTION_HOUSE_CLOSED"
            ]
        );
    }
}
