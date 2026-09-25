//! The auction window's app side: [`net`] fills [`AuctionOpen`] from the wire, [`feed_auction`]
//! pushes it into the VM and fires its events, and [`drain_auction`] sends the Lua intents.
//!
//! The hello reply opens the window, not the click (`0x4cd570` fires `AUCTION_HOUSE_SHOW` inside
//! the hello handler). Browse, Bids and Auctions are independent lists, as in the reference's
//! three arrays (`0x4cc0f0`), each one 50-row page with its own match count, sort stack and update
//! event. Walking out of the 5.56 yd service range closes the window with no packet (`[0xb72410]`
//! holds 30.864, that radius squared).

use bevy::prelude::*;

use benilla_protocol::messages::{auction_filter, AuctionListEntry};
use benilla_ui::script::{
    AuctionCategory, AuctionItemRow, AuctionListState, AuctionState, AuctionSubCategory, UiScript,
    BIDDER, LIST, OWNER,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_action::{show_messages, ui_error_text, MessageSink, Shown, UiError};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

mod sort;

use sort::SortStack;

mod net;

/// The browse rate limit: `0x4ce980` arms `tick + 0x1388` once sent; a refused query fires nothing.
const QUERY_THROTTLE_SECS: f64 = 5.0;

/// The upper bounds, in ms, of time-left buckets 1 to 3 (the reference's table at `0x8072a8`).
const TIME_LEFT_SHORT_MS: u32 = 30 * 60 * 1000;
const TIME_LEFT_MEDIUM_MS: u32 = 2 * 60 * 60 * 1000;
const TIME_LEFT_LONG_MS: u32 = 8 * 60 * 60 * 1000;

/// Past this, `time_left_ms` is an underflow and reads as expired: vmangos writes it unclamped
/// (`AuctionHouseMgr.cpp:838`) and sweeps expiry every 60 s, and no auction runs past 24 h.
const TIME_LEFT_IMPLAUSIBLE_MS: u32 = 7 * 24 * 60 * 60 * 1000;

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

/// A wire row resolved for display. Its 1-based position after the sort is the index the Lua API
/// uses, so the drain maps a click back to [`Self::auction_id`] through the same sort.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AuctionRow {
    pub(crate) auction_id: u32,
    pub(crate) item_entry: u32,
    /// The random-suffix roll behind the name's suffix, the hover's enchant lines and the link.
    pub(crate) random_property_id: u32,
    pub(crate) count: u32,
    pub(crate) name: Option<String>,
    pub(crate) texture: Option<String>,
    pub(crate) quality: Option<u32>,
    /// The item's `RequiredLevel`, 0 until its template lands.
    pub(crate) level: u32,
    pub(crate) start_bid: u32,
    pub(crate) min_increment: u32,
    pub(crate) buyout: u32,
    pub(crate) current_bid: u32,
    /// The time-left bucket, `1..=4`.
    pub(crate) time_left: u32,
    /// Whether the player holds the high bid.
    pub(crate) high_bidder: bool,
    pub(crate) owner: Option<String>,
    pub(crate) link: Option<String>,
}

impl AuctionRow {
    /// The "current bid" column: the opening price until someone bids, since 0 means no bids.
    pub(crate) fn displayed_bid(&self) -> u32 {
        if self.current_bid == 0 {
            self.start_bid
        } else {
            self.current_bid
        }
    }
}

/// One of the three lists.
#[derive(Debug, Default)]
pub(crate) struct AuctionListSlot {
    /// The current page in wire order.
    pub(crate) entries: Vec<AuctionListEntry>,
    /// `totalCount`, the match count before the page cap; the pager reads it.
    pub(crate) total: u32,
    /// A result for this list landed, an empty page included; the refreshes gate on it.
    received: bool,
    sort: SortStack,
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

/// The open auctioneer session; cleared on a client-side close, on walking away and on disconnect.
#[derive(Resource, Default)]
pub(crate) struct AuctionOpen {
    pub(crate) auctioneer: Option<u64>,
    /// The `AuctionHouse.dbc` row (1..=7), which keys the deposit rate.
    pub(crate) house_id: u32,
    pub(crate) lists: [AuctionListSlot; 3],
    /// Fire `AUCTION_HOUSE_SHOW` next feed: a hello reply landed.
    show_requested: bool,
    /// Fire `NEW_AUCTION_UPDATE` next feed: the sell slot changed.
    sell_slot_dirty: bool,
    /// Per list, a result landed, so its event is owed even for an identical page: the Browse
    /// pane leaves "Searching" only on it (`Blizzard_AuctionUI.lua:187`). Only that list's: the
    /// Auctions tab's update needs the `page` its `OnShow` sets (`Blizzard_AuctionUI.lua:817`).
    list_result_landed: [bool; 3],
    /// Empty the sell slot next feed: a listing was accepted, and the stale `(bag, slot)` would
    /// auction whatever lands there next.
    sell_slot_taken: bool,
    /// The `Time::elapsed_secs_f64` before which a browse query is refused; opening clears it.
    query_gate: Option<f64>,
    /// Messages queued for [`feed_auction`]; [`Self::clear`] keeps them, since a sale notice can
    /// land after a close.
    pub(crate) messages: Vec<AuctionMessage>,
    /// A list we hold went stale (a sale, an outbid, a cancel): the drain re-asks its page 0.
    pending_owner_refresh: bool,
    pending_bidder_refresh: bool,
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
    /// The hello reply: a new auctioneer resets the lists and sort stacks; the same one keeps them
    /// and re-shows, as the reference re-fires its show on every greeting.
    pub(crate) fn open(&mut self, auctioneer: u64, house_id: u32) {
        if self.auctioneer != Some(auctioneer) {
            self.auctioneer = Some(auctioneer);
            self.lists = Default::default();
        }
        self.house_id = house_id;
        // The reference zeroes the browse gate in the handler that stores the auctioneer.
        self.query_gate = None;
        self.show_requested = true;
    }

    /// Replace one list's page; that list's event is owed even for an identical page.
    pub(crate) fn set_list(&mut self, which: usize, entries: Vec<AuctionListEntry>, total: u32) {
        self.list_result_landed[which] = true;
        let slot = &mut self.lists[which];
        slot.received = true;
        slot.entries = entries;
        slot.total = total.max(slot.entries.len() as u32);
    }

    /// The auction id at a 1-based row of the sorted view, the order the player clicked in.
    fn auction_id_at(index_1based: u32, rows: &[AuctionRow]) -> Option<u32> {
        index_1based
            .checked_sub(1)
            .and_then(|i| rows.get(i as usize))
            .map(|r| r.auction_id)
    }

    pub(crate) fn sell_slot_taken(&mut self) {
        self.sell_slot_taken = true;
    }

    /// Re-query our listings next frame, only if we hold them: the reference patches its cached
    /// rows, so a list it never fetched announces nothing.
    pub(crate) fn refresh_owner(&mut self) {
        self.pending_owner_refresh |= self.lists[OWNER].received;
    }

    /// Re-query our bids next frame, under [`Self::refresh_owner`]'s rule.
    pub(crate) fn refresh_bidder(&mut self) {
        self.pending_bidder_refresh |= self.lists[BIDDER].received;
    }

    /// Close the window client-side; the reference sends nothing.
    pub(crate) fn clear(&mut self) {
        self.auctioneer = None;
        self.house_id = 0;
        self.lists = Default::default();
        self.show_requested = false;
        self.sell_slot_dirty = false;
        self.query_gate = None;
        self.pending_owner_refresh = false;
        self.pending_bidder_refresh = false;
        self.list_result_landed = [false; 3];
        self.sell_slot_taken = false;
    }

    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

impl NpcSession for AuctionOpen {
    fn npc(&self) -> Option<u64> {
        self.auctioneer
    }

    fn close(&mut self) {
        self.clear();
    }
}

pub(crate) struct UiAuctionPlugin;

impl Plugin for UiAuctionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<AuctionOpen>().add_systems(
            Update,
            (
                // Range-close before the feed so the close fires the same frame; the feed before
                // the input pass, and after `UnitFeed` so a row's tooltip reads a landed template;
                // the drain after the input pass so a click goes out the same frame.
                close_npc_session_out_of_range::<AuctionOpen>.before(feed_auction),
                // Only with the interface up: a sale notice landing in the login burst would
                // otherwise be shown on the boot VM and lost.
                feed_auction
                    .after(crate::ui_unit::UnitFeed)
                    .in_set(UiFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
                drain_auction.after(UiInput),
            ),
        );
    }
}

/// The wire's unclamped milliseconds left, as the reference's bucket 1 to 4.
fn time_left_bucket(ms: u32) -> u32 {
    if ms >= TIME_LEFT_IMPLAUSIBLE_MS {
        return 1;
    }
    if ms < TIME_LEFT_SHORT_MS {
        1
    } else if ms < TIME_LEFT_MEDIUM_MS {
        2
    } else if ms < TIME_LEFT_LONG_MS {
        3
    } else {
        4
    }
}

/// One wire entry as a display row, through the ask-once template and name caches; a `None`
/// fills in when its answer lands. The link carries the row's enchant, property and suffix.
fn resolve_row(
    entry: &AuctionListEntry,
    self_guid: Option<u64>,
    items: &Items,
    icons: Option<&ItemDisplays>,
    names: &NameCache,
    commands: &NetCommands,
    rolls: crate::items::RollCatalogs,
) -> AuctionRow {
    let template = items.template(entry.item_entry, 0, commands);
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

    AuctionRow {
        auction_id: entry.auction_id,
        item_entry: entry.item_entry,
        random_property_id: roll,
        count: entry.count,
        name,
        texture,
        quality,
        level,
        start_bid: entry.start_bid,
        min_increment: entry.min_increment,
        buyout: entry.buyout,
        current_bid: entry.current_bid,
        time_left: time_left_bucket(entry.time_left_ms),
        high_bidder: self_guid.is_some_and(|g| g == entry.bidder_guid),
        owner: names
            .resolve(entry.owner_guid, commands)
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

fn rows_for(
    slot: &AuctionListSlot,
    self_guid: Option<u64>,
    items: &Items,
    icons: Option<&ItemDisplays>,
    names: &NameCache,
    commands: &NetCommands,
    rolls: crate::items::RollCatalogs,
) -> Vec<AuctionRow> {
    let mut rows: Vec<AuctionRow> = slot
        .entries
        .iter()
        .map(|e| resolve_row(e, self_guid, items, icons, names, commands, rolls))
        .collect();
    slot.sort.apply(&mut rows);
    rows
}

fn to_script_row(r: &AuctionRow) -> AuctionItemRow {
    AuctionItemRow {
        auction_id: r.auction_id,
        item_id: r.item_entry,
        random_property_id: r.random_property_id,
        name: r.name.clone(),
        texture: r.texture.clone(),
        count: r.count,
        quality: r.quality,
        level: r.level,
        min_bid: r.start_bid,
        min_increment: r.min_increment,
        buyout_price: r.buyout,
        bid_amount: r.current_bid,
        high_bidder: r.high_bidder,
        owner: r.owner.clone(),
        time_left: r.time_left,
        link: r.link.clone(),
    }
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
    script: Option<NonSendMut<UiScript>>,
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
    mut last: Local<crate::ui_script::VmMemo<Option<AuctionState>>>,
    mut last_open: Local<crate::ui_script::VmMemo<Option<u64>>>,
    mut last_can_query: Local<crate::ui_script::VmMemo<bool>>,
    mut last_sell: Local<crate::ui_script::VmMemo<Option<(i64, u32)>>>,
    mut last_classes: Local<crate::ui_script::VmMemo<Vec<AuctionCategory>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_open = last_open.get(&script);
    let last_can_query = last_can_query.get(&script);
    let last_sell = last_sell.get(&script);

    // The class tree needs no session: the stock `AuctionFrameBrowse_OnLoad` reads
    // `GetAuctionItemClasses()` when the addon loads (`Blizzard_AuctionUI.lua:174`).
    let classes_now = categories(catalogs.0.as_deref(), catalogs.1.as_deref());
    let last_classes = last_classes.get(&script);
    if *last_classes != classes_now {
        *last_classes = classes_now.clone();
        script.set_auction_item_classes(classes_now);
    }

    // A `_S` line whose item is not cached yet stays queued, as the reference defers it through
    // `0x4cd190`; the `items.template` call below asks for it.
    {
        let mut deferred = Vec::new();
        let mut lines = Vec::new();
        for msg in std::mem::take(&mut auction.messages) {
            let fill = match msg.item {
                None => None,
                Some(entry) => match items.template(entry, 0, &commands) {
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
        auction.messages = deferred;
        show_messages(&mut script, &mut sink, "ui_auction", lines);
    }

    let self_guid = self_q.iter().next().map(|g| g.0);
    let rolls = crate::items::RollCatalogs {
        props: catalogs.2.as_deref(),
        enchants: catalogs.3.as_deref(),
    };
    let fresh = auction.auctioneer.map(|_| {
        let lists = [LIST, BIDDER, OWNER].map(|i| {
            let slot = &auction.lists[i];
            let rows = rows_for(
                slot,
                self_guid,
                &items,
                icons.as_deref(),
                &names,
                &commands,
                rolls,
            );
            AuctionListState {
                rows: rows.iter().map(to_script_row).collect(),
                total: slot.total.max(rows.len() as u32),
                sort: slot.sort.pairs(),
            }
        });
        AuctionState {
            lists,
            deposit_percent: houses
                .as_deref()
                .and_then(|h| h.0.deposit_percent(auction.house_id))
                .unwrap_or(0),
        }
    });

    let opened = last_open.is_none() && auction.auctioneer.is_some();
    let closed = last_open.is_some() && auction.auctioneer.is_none();
    let show_requested = std::mem::take(&mut auction.show_requested);
    let landed = std::mem::take(&mut auction.list_result_landed);
    let changed = fresh != *last;
    // Owed per list: a result landed, or an async name or template filled one of its rows.
    let owed: [bool; 3] = std::array::from_fn(|i| {
        landed[i] || fresh.as_ref().map(|s| &s.lists[i]) != last.as_ref().map(|s| &s.lists[i])
    });
    if changed {
        script.set_auction(fresh.clone());
    }
    if opened {
        script.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    } else if closed {
        script.fire_event("AUCTION_HOUSE_CLOSED", vec![]);
        script.clear_auction_sell_item();
    } else if auction.auctioneer.is_some() {
        if show_requested {
            script.fire_event("AUCTION_HOUSE_SHOW", vec![]);
        }
        // Literal names, not a loop: `reference_ui`'s producer test finds fire sites by literal.
        if owed[LIST] {
            script.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]);
        }
        if owed[BIDDER] {
            script.fire_event("AUCTION_BIDDER_LIST_UPDATE", vec![]);
        }
        if owed[OWNER] {
            script.fire_event("AUCTION_OWNED_LIST_UPDATE", vec![]);
        }
    }
    *last = fresh;
    *last_open = auction.auctioneer;

    // A listing was accepted: drop the staged item before anything reads the slot.
    if std::mem::take(&mut auction.sell_slot_taken) {
        script.clear_auction_sell_item();
    }

    let sell = script.auction_sell_item();
    if sell != *last_sell || std::mem::take(&mut auction.sell_slot_dirty) {
        *last_sell = sell;
        script.fire_event("NEW_AUCTION_UPDATE", vec![]);
    }

    // The browse gate, pushed when it changes; the Search button polls it every frame.
    let now = time.elapsed_secs_f64();
    let can_query = auction.query_gate.is_none_or(|gate| now >= gate);
    if can_query != *last_can_query {
        script.set_auction_can_query(can_query);
        *last_can_query = can_query;
    }
}

/// Drain the Lua intents into the auction `CMSG`s.
fn drain_auction(
    script: Option<NonSendMut<UiScript>>,
    mut auction: ResMut<AuctionOpen>,
    commands: Res<NetCommands>,
    time: Res<Time>,
    objects: Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    names: Res<NameCache>,
    self_q: Query<(&ObjectStore, &crate::net::Guid), With<SelfPlayer>>,
    props: Option<Res<crate::items::RandomProperties>>,
    enchants: Option<Res<crate::items::Enchants>>,
) {
    let rolls = crate::items::RollCatalogs {
        props: props.as_deref(),
        enchants: enchants.as_deref(),
    };
    let Some(mut script) = script else {
        return;
    };
    let Some(auctioneer) = auction.auctioneer else {
        // No session: honor a stray close and drop every other intent.
        if script.take_auction_close() {
            auction.clear();
        }
        let _ = script.take_auction_query();
        let _ = script.take_auction_owner_query();
        let _ = script.take_auction_bidder_query();
        let _ = script.take_auction_bids();
        let _ = script.take_auction_cancels();
        let _ = script.take_auction_start();
        let _ = script.take_auction_sorts();
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

    // Bids and cancels map the clicked row through the sorted view the feed pushed, roll included.
    let self_guid = self_q.iter().next().map(|(_, g)| g.0);
    let bids = script.take_auction_bids();
    let cancels = script.take_auction_cancels();
    if !bids.is_empty() || !cancels.is_empty() {
        for bid in bids {
            let rows = rows_for(
                &auction.lists[bid.list],
                self_guid,
                &items,
                icons.as_deref(),
                &names,
                &commands,
                rolls,
            );
            if let Some(auction_id) = AuctionOpen::auction_id_at(bid.index, &rows) {
                let _ = commands.0.send(ClientCommand::AuctionPlaceBid {
                    auctioneer,
                    auction_id,
                    price: bid.amount,
                });
            }
        }
        for index in cancels {
            let rows = rows_for(
                &auction.lists[OWNER],
                self_guid,
                &items,
                icons.as_deref(),
                &names,
                &commands,
                rolls,
            );
            if let Some(auction_id) = AuctionOpen::auction_id_at(index, &rows) {
                let _ = commands.0.send(ClientCommand::AuctionRemoveItem {
                    auctioneer,
                    auction_id,
                });
            }
        }
    }

    // The `(bag, slot)` resolves at send time, as the reference re-reads the slot on create.
    if let Some(req) = script.take_auction_start() {
        let item_guid = script
            .auction_sell_item()
            .and_then(|(bag, slot)| {
                self_q.iter().next().and_then(|(store, _)| {
                    crate::ui_items::slot_guid(&store.0, bag, (slot.max(1) - 1) as u8, &objects)
                })
            })
            .unwrap_or(0);
        if item_guid != 0 {
            let _ = commands.0.send(ClientCommand::AuctionSellItem {
                auctioneer,
                item_guid,
                bid: req.min_bid,
                buyout: req.buyout,
                etime_minutes: req.duration,
            });
        }
    }

    // Header clicks last: a row pick above indexes last frame's order, so sorting first would
    // point it at another auction. A sort sends nothing.
    for (which, key) in script.take_auction_sorts() {
        if let Some(slot) = auction.lists.get_mut(which) {
            slot.sort.click(&key);
        }
    }

    if script.take_auction_close() {
        auction.clear();
    }
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

    #[test]
    fn the_time_left_buckets_read_an_underflow_as_expired() {
        assert_eq!(time_left_bucket(0), 1, "already gone");
        assert_eq!(time_left_bucket(29 * 60 * 1000), 1);
        assert_eq!(time_left_bucket(31 * 60 * 1000), 2);
        assert_eq!(time_left_bucket(3 * 60 * 60 * 1000), 3);
        assert_eq!(time_left_bucket(20 * 60 * 60 * 1000), 4, "a 24h auction");
        // `(expireTime - now) * 1000` one second past the deadline, unclamped.
        assert_eq!(
            time_left_bucket(u32::MAX - 1000),
            1,
            "an underflow is expired, not eternal"
        );
    }

    #[test]
    fn an_identical_list_result_still_owes_its_events() {
        let mut open = AuctionOpen::default();
        open.open(0x1234, 1);

        open.set_list(LIST, Vec::new(), 0);
        assert!(open.list_result_landed[LIST], "the first empty page");

        // The feed consumes the flag when it fires.
        open.list_result_landed = [false; 3];

        open.set_list(LIST, Vec::new(), 0);
        assert!(
            open.list_result_landed[LIST],
            "an identical page owes the event too — this is the whole bug"
        );
    }

    #[test]
    fn a_result_owes_only_its_own_lists_event() {
        let mut open = AuctionOpen::default();
        open.open(0x1234, 1);

        open.set_list(LIST, Vec::new(), 0);
        assert_eq!(
            open.list_result_landed,
            [true, false, false],
            "a browse result is not news about the player's bids or listings"
        );

        open.list_result_landed = [false; 3];
        open.set_list(OWNER, Vec::new(), 0);
        assert_eq!(open.list_result_landed, [false, false, true]);

        open.list_result_landed = [false; 3];
        open.set_list(BIDDER, Vec::new(), 0);
        assert_eq!(open.list_result_landed, [false, true, false]);
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

    #[test]
    fn an_accepted_listing_releases_the_sell_slot() {
        let mut open = AuctionOpen::default();
        open.open(0x1234, 1);
        assert!(!open.sell_slot_taken, "nothing listed yet");

        open.sell_slot_taken();
        assert!(open.sell_slot_taken, "the feed owes the slot a clear");

        // A closed session forgets it rather than clearing a slot it does not own.
        open.clear();
        assert!(!open.sell_slot_taken);
    }

    #[test]
    fn an_unbid_row_shows_its_opening_price() {
        let unbid = AuctionRow {
            start_bid: 5000,
            current_bid: 0,
            ..Default::default()
        };
        assert_eq!(unbid.displayed_bid(), 5000);
        let bid = AuctionRow {
            start_bid: 5000,
            current_bid: 7500,
            ..Default::default()
        };
        assert_eq!(bid.displayed_bid(), 7500);
    }
}
