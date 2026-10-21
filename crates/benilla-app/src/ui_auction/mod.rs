//! The auction window's app side: [`net`] fills [`AuctionOpen`] from the wire, [`feed_auction`]
//! pushes it into the VM and fires its events, and [`drain_auction`] sends the Lua intents.
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

use benilla_protocol::messages::{auction_filter, AuctionListEntry};
use benilla_ui::auction_sort::{sort_column, AuctionSortStack};
use benilla_ui::script::{
    AuctionCategory, AuctionHighBidder, AuctionItemRow, AuctionListState, AuctionState,
    AuctionSubCategory, UiScript, BIDDER, LIST, OWNER,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_action::{show_messages, ui_error_text, MessageSink, Shown, UiError};
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{CoreCloses, LatchApp, LatchedWindow, NpcWindow};

mod net;

#[cfg(test)]
mod sort_tests;

/// The browse rate limit: `0x4ce980` arms `tick + 0x1388` once sent; a refused query fires nothing.
const QUERY_THROTTLE_SECS: f64 = 5.0;

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
#[derive(Debug, Default)]
pub(crate) struct AuctionListSlot {
    /// The current page in wire order.
    pub(crate) entries: Vec<AuctionListEntry>,
    /// `totalCount`, the match count before the page cap; the pager reads it.
    pub(crate) total: u32,
    /// A result for this list landed, an empty page included; the refreshes gate on it.
    received: bool,
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
#[derive(Default)]
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
#[derive(Resource, Default)]
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
    query_gate: Option<f64>,
    /// Messages queued for [`feed_auction`]; [`Self::clear`] keeps them, since a sale notice can
    /// land after a close.
    pub(crate) messages: Vec<AuctionMessage>,
    /// How many of `messages`, from the front, a feed has read: the rest came since, each with its
    /// notification's lookup with a callback (`0x4ccfa2`, `0x4cd41f`, `0x4cd51a`).
    messages_asked: usize,
    /// A list we hold went stale (a sale, an outbid, a cancel): the drain re-asks its page 0.
    pending_owner_refresh: bool,
    pending_bidder_refresh: bool,
    /// The count [`AuctionListSlot::page`] is drawn from; never reset, so no two results share one.
    next_page: u32,
    /// The key stacks; neither [`Self::open`] nor [`Self::clear`] touches them.
    pub(crate) sorts: SortStacks,
    pub(crate) wire: AuctionWireLog,
}

/// One of the reference's twenty auction message rows: the twelve refusals (`0x16c`-`0x177`) are
/// kind 2, on the red `UIErrorsFrame`; the eight outcomes (`0x178`-`0x17f`) are kind 0, in chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuctionMessage {
    pub(crate) key: &'static str,
    /// The entry whose name fills the `%s`. The reference fills a plain name, never a link
    /// (`0x5d8b00`), with its random suffix; this carries no roll, so the name is unsuffixed.
    pub(crate) item: Option<u32>,
}

impl AuctionMessage {
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

    /// A refusal; none of the twelve takes a fill.
    pub(crate) fn error(key: &'static str) -> Self {
        Self { key, item: None }
    }
}

impl AuctionOpen {
    /// The hello reply (`0x4cd570`): the auctioneer and house stored, the browse gate zeroed
    /// (`0x4cd5a3`) and every list emptied (`0x4cd5b4`-`0x4cd5cd`), from the auctioneer already
    /// open as from any other; the show re-fires on every greeting. A list's owed tail, the sort
    /// stacks and the selection stay.
    pub(crate) fn open(&mut self, auctioneer: u64, house_id: u32) {
        self.auctioneer = Some(auctioneer);
        self.house_id = house_id;
        self.empty_lists();
        self.query_gate = None;
        self.show_requested = true;
        self.pending_owner_refresh = false;
        self.pending_bidder_refresh = false;
    }

    /// Every list's count and total zeroed, as the hello and the world-enter reset write them.
    /// Each emptied list is a page of its own, which the VM takes as it comes.
    fn empty_lists(&mut self) {
        for slot in &mut self.lists {
            self.next_page = self.next_page.wrapping_add(1);
            slot.entries.clear();
            slot.total = 0;
            slot.received = false;
            slot.page = self.next_page;
        }
    }

    /// Replace one list's page; that list's event is owed even for an identical page.
    pub(crate) fn set_list(&mut self, which: usize, entries: Vec<AuctionListEntry>, total: u32) {
        self.list_result_landed[which] = true;
        self.next_page = self.next_page.wrapping_add(1);
        let slot = &mut self.lists[which];
        slot.received = true;
        slot.entries = entries;
        slot.total = total.max(slot.entries.len() as u32);
        slot.page = self.next_page;
        slot.arrived_ms = None;
        slot.awaiting = Some(slot.entries.iter().map(|e| e.item_entry).collect());
    }

    /// Re-query our listings next frame, only with a session to send it and a list we hold: the
    /// reference patches its cached rows, so a list it never fetched announces nothing.
    pub(crate) fn refresh_owner(&mut self) {
        self.pending_owner_refresh |= self.auctioneer.is_some() && self.lists[OWNER].received;
    }

    /// Re-query our bids next frame, under [`Self::refresh_owner`]'s rule.
    pub(crate) fn refresh_bidder(&mut self) {
        self.pending_bidder_refresh |= self.auctioneer.is_some() && self.lists[BIDDER].received;
    }

    /// The close (`0x4cd5f0`), client-side, sending nothing: the auctioneer and house zeroed
    /// (`0x4cd613`-`0x4cd61f`). The lists, their owed tails and the browse gate stand, as the
    /// reference leaves its counts and its throttle.
    pub(crate) fn clear(&mut self) {
        self.auctioneer = None;
        self.house_id = 0;
        self.show_requested = false;
        self.pending_owner_refresh = false;
        self.pending_bidder_refresh = false;
        self.list_result_landed = [false; 3];
    }

    /// The world-enter cascade's reset (`0x4cc280`): the session closed, every list emptied with
    /// its owed tail (`0x4cc296`-`0x4cc2cd`) and the browse gate zeroed (`0x4cc2b4`); the sort
    /// stacks stay.
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
                    // a landed template; the drain after the input pass so a click goes out the
                    // same frame.
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
                seed_sorts_at_world_entry,
            );
    }
}

/// The world-enter cascade's auction reset (`0x490a05` → `0x4cc280`), which a login, a cross-map
/// worldport and a `/reload` all run: the sell slot emptied (`0x4cd640`), the session and the
/// lists cleared ([`AuctionOpen::reset_world_state`]) and the selection with them, signalling
/// nothing else. The emptied lists reach the VM with the feed that follows.
fn reset_auction_on_world_enter(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
    mut auction: ResMut<AuctionOpen>,
) {
    hosted_vm.run(|script| {
        if cascades.read().count() == 0 {
            return;
        }
        auction.reset_world_state();
        if let Some(script) = script {
            script.clear_auction_sell_item();
            script.clear_auction_selections();
        }
    })
}

/// The world entry seeds the three stacks (`0x48f5f9` → `0x4cc0f0`, `0x4cc236`-`0x4cc274`), from
/// `ClientInitializeGame` (`0x401570`), which runs once per entry from the glue screens and not on
/// a worldport.
fn seed_sorts_at_world_entry(mut auction: ResMut<AuctionOpen>) {
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
/// list's update (`0x4cf2b0`), here the feed's diff. Any other row: `1` if the player holds the
/// bid, else nil (`0x4cf17b`–`0x4cf1a0`).
fn high_bidder(
    list: usize,
    entry: &AuctionListEntry,
    self_guid: Option<u64>,
    names: &NameCache,
    ask: Ask<'_>,
) -> AuctionHighBidder {
    let owned = list == OWNER && self_guid.is_some_and(|g| g == entry.owner_guid);
    if owned {
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

/// One wire entry of `list` as a display row, through the ask-once template and name caches; a
/// `None` fills in when its answer lands. The link carries the row's enchant, property and suffix.
/// Read as `ask` says: the list's packet looks each item up with a callback (`0x4cc9bf`,
/// `0x4ccbff`, `0x4cce4f`), and `GetAuctionItemInfo` each name (`0x4cf167`, `0x4cf1c8`).
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
        min_increment: entry.min_increment,
        buyout_price: entry.buyout,
        bid_amount: entry.current_bid,
        // The wire's milliseconds are unclamped (`AuctionHouseMgr.cpp:838`): one gone negative
        // wraps the deadline into the past, as the reference's add does.
        deadline: arrived_ms.wrapping_add(entry.time_left_ms),
        bidder_rank: bidder_rank(entry.bidder_guid, self_guid),
        base_name: template.map(|t| t.name.clone()),
        high_bidder: high_bidder(list, entry, self_guid, names, ask),
        owner: names
            .resolve_player(entry.owner_guid, ask)
            .map(str::to_string),
        link,
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

/// Push the auction house into the VM and fire its events on a change, diffed against `VmMemo`s.
fn feed_auction(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut auction: ResMut<AuctionOpen>,
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
    mut closes: CoreCloses,
) {
    hosted_vm.run(|script| {
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

        // A `_S` line whose item is not cached yet stays queued, as the reference defers it through
        // `0x4cd190`; the `items.template` call below asks for it.
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
                let get = |key: &str| script.lua().globals().get::<String>(key).ok();
                let err = match fill {
                    Some(s) => UiError::s(msg.key, s),
                    None => UiError::key(msg.key),
                };
                if let Some(text) = ui_error_text(&err, &get) {
                    lines.push(Shown::keyed(msg.key, text));
                }
            }
            auction.messages_asked = deferred.len();
            auction.messages = deferred;
            show_messages(script, &mut sink, "ui_auction", lines);
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
            let slot = &auction.lists[i];
            let ask = Ask::at(shown || auction.list_result_landed[i], &commands);
            let rows: Vec<AuctionItemRow> = slot
                .entries
                .iter()
                .map(|e| {
                    resolve_row(
                        i,
                        e,
                        slot.arrived_ms.unwrap_or(tick),
                        self_guid,
                        &items,
                        icons.as_deref(),
                        &names,
                        ask,
                        rolls,
                    )
                })
                .collect();
            AuctionListState {
                total: slot.total.max(rows.len() as u32),
                rows,
                sort: auction.sorts.0[i],
                page: slot.page,
                ..Default::default()
            }
        });
        let mut fresh = AuctionState {
            lists,
            deposit_percent: houses
                .as_deref()
                .and_then(|h| h.0.deposit_percent(auction.house_id))
                .unwrap_or(0),
            open: auction.auctioneer.is_some(),
        };

        // A close is the core's, which told the VM; a disconnect's clear tells it nothing.
        let opened = last_open.is_none() && auction.auctioneer.is_some();
        let show_requested = std::mem::take(&mut auction.show_requested);
        auction.list_result_landed = [false; 3];
        // Per list, the sort tail once every item its page waits on is answered, a record or a
        // refusal, as the pending counter's last decrement runs it (`0x4cc9e8`, `0x4cca10`),
        // whatever the session; otherwise, on the page the VM already holds, the update a landed
        // name or template owes, which sorts nothing (`0x4cf290`, `0x4cf2b0`, `0x4cf2d0`).
        let tails: [bool; 3] = std::array::from_fn(|i| {
            auction.lists[i]
                .awaiting
                .as_ref()
                .is_some_and(|items_of| items_of.iter().all(|&e| items.template_answered(e)))
        });
        let renamed: [bool; 3] = std::array::from_fn(|i| {
            !opened
                && auction.lists[i].awaiting.is_none()
                && last
                    .as_ref()
                    .is_some_and(|l| l.lists[i].page == fresh.lists[i].page)
                && list_content(Some(&fresh), i) != list_content(last.as_ref(), i)
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
        // Literal names, not a loop: `reference_ui`'s producer test finds fire sites by literal.
        if renamed[LIST] {
            script.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]);
        }
        if renamed[BIDDER] {
            script.fire_event("AUCTION_BIDDER_LIST_UPDATE", vec![]);
        }
        if renamed[OWNER] {
            script.fire_event("AUCTION_OWNED_LIST_UPDATE", vec![]);
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

/// A pushed list's rows and count: what a landed name or template changes.
fn list_content(state: Option<&AuctionState>, list: usize) -> Option<(&[AuctionItemRow], u32)> {
    state.map(|s| (s.lists[list].rows.as_slice(), s.lists[list].total))
}

/// Drain the Lua intents into the auction `CMSG`s.
fn drain_auction(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut auction: ResMut<AuctionOpen>,
    mut world_commands: Commands,
    commands: Res<NetCommands>,
    time: Res<Time>,
) {
    hosted_vm.run(|script| {
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
        let Some(auctioneer) = auction.auctioneer else {
            // No session: a stray close finds no cell (`0x4cd5fb`); every other intent is dropped.
            let _ = script.take_auction_close();
            let _ = script.take_auction_query();
            let _ = script.take_auction_owner_query();
            let _ = script.take_auction_bidder_query();
            let _ = script.take_auction_bids();
            let _ = script.take_auction_cancels();
            let _ = script.take_auction_starts();
            return;
        };

        // The browse query, behind the gate the Search button reads.
        if let Some(q) = script.take_auction_query() {
            let now = time.elapsed_secs_f64();
            if auction.query_gate.is_none_or(|gate| now >= gate) {
                auction.query_gate = Some(now + QUERY_THROTTLE_SECS);
                auction.wire.browse_sent += 1;
                let _ = commands.0.send(ClientCommand::AuctionListItems {
                    auctioneer,
                    list_from: q.page.saturating_mul(50),
                    searched_name: q.name,
                    level_min: u8::try_from(q.min_level).unwrap_or(u8::MAX),
                    level_max: u8::try_from(q.max_level).unwrap_or(u8::MAX),
                    slot_id: q.inv_type.unwrap_or(auction_filter::ANY),
                    // Already ids: the binding maps menu positions as `0x4ce980` does.
                    main_category: q.class.unwrap_or(auction_filter::ANY),
                    sub_category: q.sub_class.unwrap_or(auction_filter::ANY),
                    quality: q.quality.unwrap_or(auction_filter::ANY),
                    usable: u8::from(q.usable_only),
                });
            }
        }

        if std::mem::take(&mut auction.pending_owner_refresh) {
            let _ = commands.0.send(ClientCommand::AuctionListOwnerItems {
                auctioneer,
                list_from: 0,
            });
        }
        if std::mem::take(&mut auction.pending_bidder_refresh) {
            let _ = commands.0.send(ClientCommand::AuctionListBidderItems {
                auctioneer,
                list_from: 0,
                auction_ids: Vec::new(),
            });
        }

        if let Some(page) = script.take_auction_owner_query() {
            let _ = commands.0.send(ClientCommand::AuctionListOwnerItems {
                auctioneer,
                list_from: page.saturating_mul(50),
            });
        }
        if let Some(page) = script.take_auction_bidder_query() {
            // An empty refresh set: the reference sends the ids it was outbid on (it keeps eight),
            // which this does not track, so only the auctions we lead are listed.
            let _ = commands.0.send(ClientCommand::AuctionListBidderItems {
                auctioneer,
                list_from: page.saturating_mul(50),
                auction_ids: Vec::new(),
            });
        }

        // Bids and cancels name the auction their verb read at its row (`0x4cf610`, `0x4d0260`).
        for bid in script.take_auction_bids() {
            let _ = commands.0.send(ClientCommand::AuctionPlaceBid {
                auctioneer,
                auction_id: bid.auction_id,
                price: bid.amount,
            });
        }
        for auction_id in script.take_auction_cancels() {
            let _ = commands.0.send(ClientCommand::AuctionRemoveItem {
                auctioneer,
                auction_id,
            });
        }

        // `StartAuction` emptied the sell slot in the verb (`0x4ce92a`); it sends the guid the
        // attach stored (`0x4ce8f3`), wherever the item has since gone.
        for req in script.take_auction_starts() {
            let _ = commands.0.send(ClientCommand::AuctionSellItem {
                auctioneer,
                item_guid: req.item_guid,
                bid: req.min_bid,
                buyout: req.buyout,
                etime_minutes: req.duration,
            });
        }

        if script.take_auction_close() {
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

    #[test]
    fn a_refresh_never_introduces_a_list_the_interface_never_asked_for() {
        let mut open = AuctionOpen::default();
        open.open(0x1234, 1);

        open.refresh_owner();
        open.refresh_bidder();
        assert!(
            !open.pending_owner_refresh && !open.pending_bidder_refresh,
            "we hold neither list, so neither can have gone stale"
        );

        // The Auctions tab was shown and answered, even empty: now a sale makes it stale.
        open.set_list(OWNER, Vec::new(), 0);
        open.refresh_owner();
        assert!(open.pending_owner_refresh, "a list we hold can go stale");
        assert!(
            !open.pending_bidder_refresh,
            "and the other one still cannot"
        );

        // Closing forgets it: a notice after the close re-asks nothing.
        open.clear();
        open.refresh_owner();
        assert!(!open.pending_owner_refresh);
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

        // The window opens with the page; the bidder is unknown, so nil and asked for.
        let (_, bidder) = step(&mut app);
        assert_eq!(bidder, None);
        assert_eq!(name_queries(&rx), vec![BIDDER_GUID]);

        let (seen, bidder) = step(&mut app);
        assert!(seen.is_empty(), "nothing new, nothing fired: {seen:?}");
        assert_eq!(bidder, None);

        app.world_mut().resource_mut::<NameCache>().insert_player(
            BIDDER_GUID,
            "Bidder".into(),
            None,
        );
        let (seen, bidder) = step(&mut app);
        assert_eq!(bidder.as_deref(), Some("Bidder"));
        assert_eq!(seen, vec!["AUCTION_OWNED_LIST_UPDATE".to_string()]);
    }

    /// A list's packet looks its items up with a callback (`0x4ccbff`), and `GetAuctionItemInfo`
    /// the bidder's name (`0x4cf167`), on the list's update; the frames between read. A refused
    /// bidder is asked by no frame, and again by the list's next result.
    #[test]
    fn a_refused_bidder_is_asked_at_the_next_list_result_never_by_a_frame() {
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
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.world_mut().spawn((SelfPlayer, crate::net::Guid(ME)));
        app.world_mut()
            .resource_mut::<AuctionOpen>()
            .open(0x1234, 1);
        let list = |app: &mut App| {
            app.world_mut().resource_mut::<AuctionOpen>().set_list(
                OWNER,
                vec![entry(ME, BIDDER_GUID)],
                1,
            );
        };
        list(&mut app);
        app.update();
        assert_eq!(name_queries(&rx), vec![BIDDER_GUID]);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(BIDDER_GUID, String::new(), None);
        for _ in 0..600 {
            app.update();
        }
        assert!(name_queries(&rx).is_empty(), "no frame asks the refusal");
        list(&mut app);
        app.update();
        assert_eq!(name_queries(&rx), vec![BIDDER_GUID], "the next result does");
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
