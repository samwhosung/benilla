//! The app-side **auction feed** (decision 1511 P1) — the inward half of the auction seam around
//! [`benilla_ui::script`]'s `auction` module, the auctioneer twin of [`crate::ui_merchant`]'s
//! vendor seam and [`crate::ui_mail`]'s mailbox one.
//!
//! **The window is opened by the server, not by the click.** A right-click on an auctioneer
//! ([`crate::target`]) sends `MSG_AUCTION_HELLO` and nothing else happens; the *reply* — the same
//! opcode coming back with the auctioneer guid and an `AuctionHouse.dbc` house id — is what opens
//! the session here (the window's opener `0x4cd570` calls `SetInteractNPC` and fires
//! `AUCTION_HOUSE_SHOW` from inside the hello handler). That house id is load-bearing rather than
//! decorative: it keys the deposit rate the sell pane displays, and the six faction houses charge
//! 5% where the neutral goblin house charges 25%.
//!
//! **Three lists, one session — and three separate events.** Browse (`"list"`), Bids (`"bidder"`)
//! and Auctions (`"owner"`) are three independent server queries into one window. Each holds at
//! most one 50-row page — the server's own cap — plus the pre-cap match count the pager needs,
//! plus its own sort stack, plus its own update event. The reference keeps them just as far apart
//! (three arrays allocated by `0x4cc0f0`, three counts, three sort stacks, and every
//! `AUCTION_*_LIST_UPDATE` fired by whatever just touched *its* array).
//! So a list-update event here is a statement about **one** list; firing the other two runs the
//! stock addon's other tabs over state they were never given, and the Auctions tab answers that
//! by crashing.
//!
//! **Sorting is ours and paging is the server's**, which is the split that shapes this module: a
//! header click re-orders rows we already hold and sends nothing ([`sort`]), while a page turn
//! re-sends the whole query. Nothing about the sort ever reaches the wire.
//!
//! **The browse throttle is real and it is silent.** The client refuses a browse query inside 5 s
//! of the last one and *drops it with no failure event*, which is why the Search button polls
//! [`benilla_ui::script::UiScript::set_auction_can_query`] every frame instead of reacting to a
//! refusal. The window opening clears the gate, so the first search is always allowed. (INTERIM,
//! decision 1511.)
//!
//! The packet handlers ([`net`], in the net handler table) fill [`AuctionOpen`]
//! from the wire. Each frame
//! [`feed_auction`] resolves each [`AuctionListEntry`] to a Lua-facing row (name/quality/icon via
//! the ask-once item-template cache + `ItemDisplayInfo.dbc`, seller via the ask-once name cache,
//! the time-left bucket from the wire's milliseconds), applies the sort, pushes the snapshot, and
//! fires the events the reference Lua drives. [`drain_auction`] pulls the Lua intents back out
//! into the auction `CMSG`s. The standardized NPC-session range guard ([`crate::ui_session`])
//! client-side-closes the window when the player walks away from the auctioneer. That radius is
//! the auction window's own rather than borrowed: its interaction cell `[0xb72410]`
//! holds `30.864194869995117` — a radius of `5.5555553` yd — which is exactly the service gate
//! the cursor already greys at, so the guard's existing constant is the right one. The close
//! sends no packet, also byte-confirmed.

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

/// The browse query rate limit, in seconds (`QueryAuctionItems 0x4ce980`): the reference arms the
/// gate with `tick + 0x1388` *after* the packet goes out, re-checks it inside the query itself,
/// and its refusal path fires **nothing at all** — no event, no error. That silence is why the
/// Search button polls the gate every frame instead of waiting to be told no.
const QUERY_THROTTLE_SECS: f64 = 5.0;

/// The time-left buckets, in milliseconds — the thresholds the reference's four
/// `AUCTION_TIME_LEFT` strings key off (the table at `0x8072a8`).
const TIME_LEFT_SHORT_MS: u32 = 30 * 60 * 1000;
const TIME_LEFT_MEDIUM_MS: u32 = 2 * 60 * 60 * 1000;
const TIME_LEFT_LONG_MS: u32 = 8 * 60 * 60 * 1000;

/// Past this, the wire's `time_left_ms` is not a duration — it is an underflow.
///
/// The server writes `(expireTime - now) * 1000` with **no clamp** and only sweeps expiry on a
/// 60 s timer, so an auction that ran out less than a minute ago is still listed with a negative
/// remaining time that arrives as a `u32` near 4.29 billion. The longest auction anyone can create
/// is 24 h, so anything beyond a few days is that wrap and not a long auction — it reads as
/// **expired**, never as "Very Long" (decision 1511's verified section).
const TIME_LEFT_IMPLAUSIBLE_MS: u32 = 7 * 24 * 60 * 60 * 1000;

/// `ItemSubClass.dbc` `DisplayFlags` bit 1 — this subclass is **not offered** in the auction
/// house's category filter.
///
/// Read directly off the shipped table when the polarity was still ambiguous, and since
/// **independently confirmed at the bytes** (`0x4cf9c0`). Guessing it backwards would have
/// emptied the filter instead of trimming it: every row carrying the bit is an obsolete or unused subclass —
/// Spear, Buckler(OBSOLETE), the OBSOLETE quivers, bolts and wands, Engineering Bag — and every
/// subclass a player can actually buy (Cloth, Leather, Mail, Plate, Shield, Arrow, Bullet, the
/// weapon families, the nine Recipe professions) has it clear.
const SUBCLASS_HIDDEN_FROM_AUCTIONS: u32 = 0x2;

/// The ten item classes the auction house offers, in the reference's own menu order.
///
/// The set and the order are a structural fact of the window, not text: the four `(OBSOLETE)`
/// classes plus Quest and Key are simply not auctionable, and every id here was checked to be a
/// real `ItemClass.dbc` row. **Every string the filter displays comes from the player's own DBC**,
/// which is what keeps this feature clear of decisions 1234/1260 — we ship the structure, the
/// install supplies the words.
///
/// Each entry is the reference's `0x807060` row `{itemClassId, hasSubclassFilter}`: a class whose
/// flag is 0 offers no subclass rows and no inventory-slot rows.
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

/// `ItemSubClass.Flags` bit `0x200`: the subclass offers the fourteen inventory-slot rows beneath
/// it (`GetAuctionInvTypes`, `0x4cfb63 test ah,2`). In the shipped file it is set on exactly Armor
/// 0..4 (Miscellaneous, Cloth, Leather, Mail, Plate) — not on Shield, Libram, Idol or Totem.
const SUBCLASS_OFFERS_INV_TYPES: u32 = 0x200;

/// The `AuctionHouse.dbc` catalog, loaded with the other item DBCs ([`crate::ui_items`]). Optional
/// resource — absent, the sell pane shows no deposit rather than inventing one.
#[derive(Resource)]
pub(crate) struct AuctionHouses(pub(crate) benilla_formats::AuctionHouseCatalog);

/// A wire row resolved for display. Its 1-based position in [`AuctionListSlot::rows`] after the
/// sort is the index the Lua API addresses it by, which is why the sort lives on this side: the
/// index the player clicks has to mean the same thing as the index the drain maps back to
/// [`Self::auction_id`].
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AuctionRow {
    pub(crate) auction_id: u32,
    pub(crate) item_entry: u32,
    /// The listed item's random-suffix roll — the suffix in [`Self::name`], the enchant lines the
    /// row hover shows, and the link's third field.
    pub(crate) random_property_id: u32,
    pub(crate) count: u32,
    pub(crate) name: Option<String>,
    pub(crate) texture: Option<String>,
    pub(crate) quality: Option<u32>,
    /// The item's `RequiredLevel` — `0` until its template lands.
    pub(crate) level: u32,
    pub(crate) start_bid: u32,
    pub(crate) min_increment: u32,
    pub(crate) buyout: u32,
    pub(crate) current_bid: u32,
    /// `1..=4`, or `0` for a row whose remaining time we could not read.
    pub(crate) time_left: u32,
    /// Whether *the player* holds the high bid.
    pub(crate) high_bidder: bool,
    pub(crate) owner: Option<String>,
    pub(crate) link: Option<String>,
}

impl AuctionRow {
    /// What the row shows in its "current bid" column: the seller's opening price until somebody
    /// has actually bid. A `0` current bid is "no bids", not "free".
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
    /// The current page exactly as the wire delivered it.
    pub(crate) entries: Vec<AuctionListEntry>,
    /// `totalCount` — the pre-cap match count, which is what tells the pager there is a page 2.
    pub(crate) total: u32,
    /// A result for **this** list has landed since the window opened — the interface asked for it
    /// and the server answered. `false` is "we hold no such list", which is *not* "the list is
    /// empty": an empty page still arrives, and still counts. It is what the server-driven
    /// refreshes gate on.
    received: bool,
    sort: SortStack,
}

/// What the live end-to-end probe (`crate::capture::ProbeAuctionPlugin`) reads to tell an
/// **empty** answer apart from **no** answer.
///
/// Three of this arc's outcomes are, by design, invisible in the window's own state — and each is
/// a different failure a probe has to be able to name:
///
/// - a list result carrying **zero rows** changes nothing [`feed_auction`] can diff, so "the
///   auction house is empty" and "the query never came back" look identical from the snapshot;
/// - a *successful* `SMSG_AUCTION_COMMAND_RESULT` is consumed straight into a re-query
///   (`crate::ui_auction::net`) and leaves no record — only a *failed* one surfaces, and only
///   as an error line;
/// - a browse query the throttle refuses is dropped with **no event at all** (the silence this
///   module's header describes), so "refused" and "sent" differ only in what went out on the
///   wire.
///
/// Nothing in the client needs any of it: the window reacts to the *effects*. It therefore lives
/// in one clearly-named block that the client never reads, rather than being smeared through the
/// session's own fields — and it is a monotonic tally, deliberately **not** reset by
/// [`AuctionOpen::clear`], so a count means the same thing across a session close.
#[derive(Default)]
pub(crate) struct AuctionWireLog {
    /// How many list results have landed, per list, in [`LIST`]/[`BIDDER`]/[`OWNER`] order.
    pub(crate) list_results: [u32; 3],
    /// How many browse queries actually went out — the count the throttle refuses to raise.
    pub(crate) browse_sent: u32,
    /// The most recent `SMSG_AUCTION_COMMAND_RESULT`, as `(auction_id, action, error)`.
    pub(crate) last_command: Option<(u32, u32, u32)>,
}

/// The open auctioneer session, filled by the net bridge ([`crate::ui_auction::net`]) and read
/// by [`feed_auction`]. Cleared on a client-side close, on walking away, and on disconnect.
#[derive(Resource, Default)]
pub(crate) struct AuctionOpen {
    /// The auctioneer whose window is open; `None` = no session.
    pub(crate) auctioneer: Option<u64>,
    /// The `AuctionHouse.dbc` row this auctioneer serves (1..=7) — keys the deposit rate.
    pub(crate) house_id: u32,
    pub(crate) lists: [AuctionListSlot; 3],
    /// Fire `AUCTION_HOUSE_SHOW` next feed — set when the hello reply lands.
    show_requested: bool,
    /// Fire `NEW_AUCTION_UPDATE` next feed — the sell slot changed.
    sell_slot_dirty: bool,
    /// A list result landed **for that list**, so *its* event is owed whether or not the snapshot
    /// changed. Indexed [`LIST`]/[`BIDDER`]/[`OWNER`].
    ///
    /// Diffing the snapshot is not enough and the live probe is what proved it: an empty auction
    /// house, or re-running the same search, produces a result identical to what we already hold.
    /// The window's Browse pane clears its "Searching…" state only on `AUCTION_ITEM_LIST_UPDATE`,
    /// so on an empty server a search animated its dots forever and never reported a result.
    ///
    /// **Per list — that is the whole point**. This was one flag driving all three
    /// fires, on 1511's reading that "one routine invalidates all three lists". The reference has
    /// no such routine: three arrays, three counts, three sort stacks, **three separate events**,
    /// each fired by whatever just touched *its* array. Firing the other two crashed the stock
    /// `Blizzard_AuctionUI` outright — `AuctionFrameAuctions_Update` multiplies by
    /// `AuctionFrameAuctions.page`, which the addon initialises only in that tab's `OnShow`, so a
    /// browse result arriving while the player had never opened the Auctions tab raised
    /// `attempt to perform arithmetic on field 'page' (a nil value)`.
    list_result_landed: [bool; 3],
    /// Empty the sell slot next feed — a listing was accepted, so the staged item is *gone*.
    ///
    /// Also from the probe: without this the pane kept painting a phantom stack after a successful
    /// `StartAuction`, `Create Auction` stayed enabled, and a second click re-resolved the
    /// remembered `(bag, slot)` at send time — which by then addresses whatever else has landed in
    /// that slot. That is not a cosmetic bug: it auctions an item the player never chose.
    sell_slot_taken: bool,
    /// `Time::elapsed_secs_f64` before which a browse query is refused; `None` = allowed now.
    /// Cleared when the window opens, so the first search never waits.
    query_gate: Option<f64>,
    /// Client messages the net apply queued for [`feed_auction`] to resolve and show — the
    /// reference's `DisplayError(msgId)` split into (surface, GlobalStrings key, fill), so the line
    /// comes from the VM's own strings and lands where that message's catalog row says (decisions
    /// 0669 / 1523).
    ///
    /// **Not cleared by [`Self::clear`].** These outlive the window on purpose: "your auction sold"
    /// arrives wherever the player is standing, and closing an auctioneer's window must not swallow
    /// a notice that has not been printed yet.
    pub(crate) messages: Vec<AuctionMessage>,
    /// Set when the server tells us a list we hold is now wrong (our auction sold, we were
    /// outbid, a cancel went through). The drain re-asks rather than patching the local page: the
    /// server is the only thing that knows what the page looks like now, and the notification
    /// carries an auction id, not a row.
    pending_owner_refresh: bool,
    pending_bidder_refresh: bool,
    /// The live probe's observation window onto the wire. Written by the net bridge and the
    /// drain, read by nothing in the client — see [`AuctionWireLog`].
    pub(crate) wire: AuctionWireLog,
}

/// One queued auction message: which of the reference's twenty catalog rows, and the item name that
/// fills its `%s` when it has one.
///
/// The surface is the row's own: the twelve **precondition failures** (`0x16c`-`0x177`
/// — "you cannot auction a soulbound item", "your bid is too low") are kind 2 and land on the red
/// `UIErrorsFrame`; the eight **outcomes** (`0x178`-`0x17f` — created, cancelled, outbid, won, sold,
/// expired, removed, bid accepted) are kind 0 and land in the **chat window** as `CHAT_MSG_SYSTEM`.
/// That split is the whole reason this is a queue of records rather than a queue of red strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuctionMessage {
    pub(crate) key: &'static str,
    /// The item entry whose name fills the `%s`; `None` for the six fill-less lines.
    ///
    /// The reference composes a **plain** name here (`0x5d8b00`), never a coloured `|Hitem:` link —
    /// so "Your auction of Linen Cloth sold." is unlinked in the real client too, and matching that
    /// is deliberate. It also carries the random-property roll for a suffixed name; benilla's item
    /// names are unsuffixed today, so only the entry is kept.
    pub(crate) item: Option<u32>,
}

impl AuctionMessage {
    /// A line with no fill (`ERR_AUCTION_STARTED` and friends).
    pub(crate) fn chat(key: &'static str) -> Self {
        Self { key, item: None }
    }

    /// A line about one item — the five `_S` outcomes.
    pub(crate) fn chat_item(key: &'static str, item: u32) -> Self {
        Self {
            key,
            item: Some(item),
        }
    }

    /// A refusal — the twelve precondition failures, none of which take a fill.
    pub(crate) fn error(key: &'static str) -> Self {
        Self { key, item: None }
    }
}

impl AuctionOpen {
    /// The hello reply — open (or re-open) the session. A *different* auctioneer resets everything
    /// including the sort stacks; the same one keeps them and just re-shows, since the reference
    /// re-fires its show on every greeting.
    pub(crate) fn open(&mut self, auctioneer: u64, house_id: u32) {
        if self.auctioneer != Some(auctioneer) {
            self.auctioneer = Some(auctioneer);
            self.lists = Default::default();
        }
        self.house_id = house_id;
        // The window opening clears the throttle: the reference zeroes its gate in the same
        // handler that stores the auctioneer, so the first search after walking up is free.
        self.query_gate = None;
        self.show_requested = true;
    }

    /// Replace one list's page. Always owes **that list's** event, even for an identical page.
    pub(crate) fn set_list(&mut self, which: usize, entries: Vec<AuctionListEntry>, total: u32) {
        self.list_result_landed[which] = true;
        let slot = &mut self.lists[which];
        slot.received = true;
        slot.entries = entries;
        // A server that reports fewer total matches than it just sent us is not a case worth
        // trusting over our own eyes; the pager reads the larger of the two.
        slot.total = total.max(slot.entries.len() as u32);
    }

    /// The wire auction id at a 1-based **display** row — what the auction `CMSG`s address. Takes
    /// the sorted view rather than the list index, because that is the order the player clicked in.
    fn auction_id_at(index_1based: u32, rows: &[AuctionRow]) -> Option<u32> {
        index_1based
            .checked_sub(1)
            .and_then(|i| rows.get(i as usize))
            .map(|r| r.auction_id)
    }

    /// A listing was accepted: the staged item has left the bag, so the slot must let go of it.
    pub(crate) fn sell_slot_taken(&mut self) {
        self.sell_slot_taken = true;
    }

    /// Mark our own listings stale — the drain re-queries next frame.
    ///
    /// **Only a list we hold can be stale**. A notification says "the page you are
    /// showing is now wrong"; if the interface has never asked for the owned list, there is no
    /// such page, and re-asking would *introduce* one — landing an `AUCTION_OWNED_LIST_UPDATE` on
    /// a tab whose `page` field the stock addon has not initialised yet. The reference cannot
    /// reach that state at all: its notification handlers patch the cached row in place, so an
    /// array it never fetched has nothing to patch and announces nothing.
    pub(crate) fn refresh_owner(&mut self) {
        self.pending_owner_refresh |= self.lists[OWNER].received;
    }

    /// Mark the bids we hold stale — under the same rule as [`Self::refresh_owner`].
    pub(crate) fn refresh_bidder(&mut self) {
        self.pending_bidder_refresh |= self.lists[BIDDER].received;
    }

    /// Close the window (a client-side close — vanilla sends nothing).
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

    /// Disconnect: drop the open window (mirrors every other session clear).
    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

/// The auction window is an NPC session like any other: the standardized range guard closes it
/// when the player leaves the auctioneer's service range.
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
                // The merchant/mail ordering exactly: range-close before the feed so the clear
                // turns into AUCTION_HOUSE_CLOSED the same frame; feed before the input pass so an
                // open/close is on screen the same frame; drain after it so a click's intent goes
                // out the same frame. After the UnitFeed set so a row's tooltip reads a landed
                // item-template store.
                close_npc_session_out_of_range::<AuctionOpen>.before(feed_auction),
                // Gated on the interface being up: `AuctionOpen::messages` is
                // filled by the server's unprompted sold/outbid/expired notices, and one landing
                // in the same drain as the login burst would be resolved and shown on the boot
                // VM — lost — if this ran in 2214's one-frame window. The queue is bounded by
                // what the server sends while no interface is up, and the refresh flags the same
                // packets set survive as wire re-asks either way.
                feed_auction
                    .after(crate::ui_unit::UnitFeed)
                    .in_set(UiFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
                drain_auction.after(UiInput),
            ),
        );
    }
}

/// The wire's unclamped millisecond remainder → the reference's four buckets.
fn time_left_bucket(ms: u32) -> u32 {
    // An expired-but-still-listed auction arrives as an underflowed u32. Caught first and on its
    // own, because the alternative reading — a number larger than every threshold, so bucket 4 —
    // would show an auction that ended a minute ago as the longest one on the page.
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

/// Resolve one wire entry into a display row: name/quality/icon/required-level through the
/// ask-once item-template cache, seller through the ask-once name cache, the link built with the
/// row's full enchant/property/suffix tail (an auction row carries all three, so it gets the
/// complete link rather than the zeroed one a bag slot settles for). `None`s stay `None` while a
/// query is in flight — the row shows a placeholder and fills in when the answer lands.
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
    // The rolled name — one formatter for every display of an item's name (1547), so the row, the
    // link's bracket text and the hover's plate agree.
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
        // "High bidder" is about US, not about whether anyone has bid at all.
        high_bidder: self_guid.is_some_and(|g| g == entry.bidder_guid),
        owner: names
            .resolve(entry.owner_guid, commands)
            .map(str::to_string),
        link,
    }
}

/// Build the Browse tab's category tree from the player's own DBCs.
/// `pub(crate)`: the addon-corpus survey seats the same tree off the player's chain (2167), and a
/// second copy of the class set would be a second thing to keep right.
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

/// Resolve + sort one list into its display rows.
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

/// One display row → the Lua-facing row.
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

/// The DBC catalogs a row reads, as ONE system param — [`feed_auction`] is at the
/// 16-SystemParam ceiling, and these four belong together anyway: the two class tables the
/// category tree is built from, and the random-suffix pair a rolled listing needs
/// (`ItemRandomProperties` for the name, `SpellItemEnchantment` for its lines).
type AuctionCatalogs<'w> = (
    Option<Res<'w, crate::ui_items::ItemClasses>>,
    Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    Option<Res<'w, crate::items::RandomProperties>>,
    Option<Res<'w, crate::items::Enchants>>,
);

/// Push the current auction house into the VM and fire the show/close/list-update events on a
/// transition or content change. Diffed against a `VmMemo`, exactly like the merchant/mail feeds.
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

    // The Browse tab's class tree is login-scoped: the stock `AuctionFrameBrowse_OnLoad` reads
    // `GetAuctionItemClasses()` at the addon's LOAD — the first auctioneer, before the session's
    // snapshot lands — and the client answers off `ItemClass.dbc` with no session at all (1971).
    // Pushed once the two catalogs exist and whenever they change (a diff, like the inbox's).
    let classes_now = categories(catalogs.0.as_deref(), catalogs.1.as_deref());
    let last_classes = last_classes.get(&script);
    if *last_classes != classes_now {
        *last_classes = classes_now.clone();
        script.set_auction_item_classes(classes_now);
    }

    // The queued client messages. Resolved against the VM's own GlobalStrings and
    // shown on the surface the message's catalog row names — the `ui_quest` shape.
    //
    // A `_S` line whose item template has not landed is **kept queued, not dropped**: the reference
    // defers exactly this case through `0x4cd190` and prints the line when the item arrives. Ours
    // falls out of the same ask-once cache the rows use, so the deferral is just "leave it in the
    // queue and try again next frame" — and the `items.template` call below is what asks.
    {
        let mut deferred = Vec::new();
        let mut lines = Vec::new();
        for msg in std::mem::take(&mut auction.messages) {
            let fill = match msg.item {
                None => None,
                Some(entry) => match items.template(entry, 0, &commands) {
                    Some(t) => Some(t.name.clone()),
                    // Not named yet — hold it and ask again next frame.
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
            // The rate this house charges, keyed by the id the hello reply carried.
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
    // **Which events are owed, per list.** A landed result owes its own list's event outright; a
    // changed list owes it because an async name or template just filled one of its rows in. The
    // diff is per list rather than over the whole snapshot for the same reason the fires are
    // (2308): an event names one list, and firing the other two runs the stock Lua's other tabs
    // over state they were never given.
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
        // Three literal fires, not a loop over names: the producer tripwire
        // (`reference_ui::every_event_a_chain_file_registers_has_a_producer`) reads fire sites by
        // their literal, and the stock addon registers all three (1971). Browse first, then Bids,
        // then Auctions — the array's own order; the reference never fires two in one pass either,
        // so nothing rests on it.
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

    // A listing was accepted: drop the staged item before anything reads the slot again.
    if std::mem::take(&mut auction.sell_slot_taken) {
        script.clear_auction_sell_item();
    }

    // The sell slot's own event: the create pane re-reads the item, re-suggests a price and
    // re-computes the deposit off this.
    let sell = script.auction_sell_item();
    if sell != *last_sell || std::mem::take(&mut auction.sell_slot_dirty) {
        *last_sell = sell;
        script.fire_event("NEW_AUCTION_UPDATE", vec![]);
    }

    // The browse throttle, pushed every frame it changes — the Search button polls it.
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
    // The click→auction-id map re-derives the same sorted rows the feed pushed, so it resolves
    // them the same way, roll included (1547).
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
        // No session: honor a stray close and drop everything else on the floor.
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

    // The browse query, behind the same gate the Search button is reading.
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
                // Already ids — the binding maps menu positions the way `0x4ce980` does.
                main_category: q.class.unwrap_or(auction_filter::ANY),
                sub_category: q.sub_class.unwrap_or(auction_filter::ANY),
                quality: q.quality.unwrap_or(auction_filter::ANY),
                usable: u8::from(q.usable_only),
            });
        }
    }

    // Server-driven refreshes: something we hold went stale (a sale, an outbid, a cancel). These
    // are page-0 re-asks, deliberately separate from the Lua's own paging.
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
        // The refresh set is empty: it exists so a client can re-ask about auctions it was outbid
        // on, and we do not track that memory yet (the reference keeps eight). A plain page still
        // returns every auction we currently hold the bid on, which is the whole tab.
        let _ = commands.0.send(ClientCommand::AuctionListBidderItems {
            auctioneer,
            list_from: page.saturating_mul(50),
            auction_ids: Vec::new(),
        });
    }

    // Bids and cancels address the wire auction id behind the row the player clicked, which means
    // re-deriving the same sorted view the feed pushed.
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

    // Creating an auction: the sell slot's (bag, slot) resolves to the wire item guid HERE rather
    // than at attach time, because the reference re-reads the slot when the create fires.
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

    // Header clicks land LAST, and that ordering is load-bearing: a row pick above was made
    // against the order the feed pushed last frame, so re-sorting before resolving it would point
    // the same index at a different auction. Nothing here goes on the wire.
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

    /// The tree off the player's own DBCs carries the reference's two gates: the `0x807060` class
    /// flag, and `ItemSubClass.Flags & 0x200` (`0x4cfb63`) for the inventory-slot rows — which
    /// Shield does NOT carry, though it is Armor.
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

    /// The four buckets, and the one that matters: an expired auction the server has not swept yet
    /// arrives as an underflowed u32 and must read as Short, never as "Very Long".
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

    /// An identical list result still owes its event. Diffing the snapshot was the bug: an empty
    /// auction house and a repeated search both produce a page identical to the one we hold, and
    /// the Browse pane clears its "Searching…" state only when the event arrives — so on an empty
    /// server a search animated forever and never reported a result. Found by the live probe,
    /// which is the only place an empty house is the normal case.
    #[test]
    fn an_identical_list_result_still_owes_its_events() {
        let mut open = AuctionOpen::default();
        open.open(0x1234, 1);

        open.set_list(LIST, Vec::new(), 0);
        assert!(open.list_result_landed[LIST], "the first empty page");

        // The feed consumes the flag when it fires.
        open.list_result_landed = [false; 3];

        // The very same empty page again — nothing to diff, and still owed.
        open.set_list(LIST, Vec::new(), 0);
        assert!(
            open.list_result_landed[LIST],
            "an identical page owes the event too — this is the whole bug"
        );
    }

    /// **A result owes its OWN list's event and no other**.
    ///
    /// The regression this guards is a crash, not a repaint: the stock `Blizzard_AuctionUI`'s
    /// `AuctionFrameAuctions_Update` multiplies by `AuctionFrameAuctions.page`, and that field is
    /// initialised only in the Auctions tab's `OnShow`. A browse result that also fired
    /// `AUCTION_OWNED_LIST_UPDATE` therefore ran that tab's repaint before it had ever been
    /// shown — `attempt to perform arithmetic on field 'page' (a nil value)`, on screen, on the
    /// first search of the session. `auction_frame.rs` holds the Lua half of this.
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

    /// **A refresh can only re-ask for a list we hold** — the other half of the
    /// same crash.
    ///
    /// "Your auction sold" arrives whenever the server feels like it, including while the player
    /// is standing on the Browse tab having never opened the Auctions one. Turning that into a
    /// fresh owner query would land an `AUCTION_OWNED_LIST_UPDATE` on that never-shown tab — the
    /// nil `page` again, by a slower road. The reference cannot get there: its notification
    /// handlers patch the cached row, and an array it never fetched has no row to patch.
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

        // The Auctions tab has now been shown once and the server answered — even with nothing in
        // it. From here a sale really does invalidate what the player is looking at.
        open.set_list(OWNER, Vec::new(), 0);
        open.refresh_owner();
        assert!(open.pending_owner_refresh, "a list we hold can go stale");
        assert!(
            !open.pending_bidder_refresh,
            "and the other one still cannot"
        );

        // Closing forgets all of it — a notice landing after the window is gone re-asks nothing.
        open.clear();
        open.refresh_owner();
        assert!(!open.pending_owner_refresh);
    }

    /// An accepted listing releases the sell slot. Without it the pane kept painting a phantom
    /// stack, Create Auction stayed enabled, and a second click re-resolved the remembered
    /// (bag, slot) at send time — auctioning whatever had since landed in that slot.
    #[test]
    fn an_accepted_listing_releases_the_sell_slot() {
        let mut open = AuctionOpen::default();
        open.open(0x1234, 1);
        assert!(!open.sell_slot_taken, "nothing listed yet");

        open.sell_slot_taken();
        assert!(open.sell_slot_taken, "the feed owes the slot a clear");

        // And a closed session forgets it rather than clearing a slot it no longer owns.
        open.clear();
        assert!(!open.sell_slot_taken);
    }

    /// The displayed bid falls back to the opening price until somebody has bid — a zero current
    /// bid means "no bids", never "free".
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
