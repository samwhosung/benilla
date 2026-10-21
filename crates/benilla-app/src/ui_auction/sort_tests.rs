//! The three lists end to end, their sort, their time left and their life across the window: the
//! wire's results through the handlers, the feed and the drain to the VM's getters, as the window
//! reads them.

use benilla_protocol::messages::AuctionListEntry;
use benilla_protocol::SessionEvent;
use benilla_ui::script::{UiScript, BIDDER, LIST, OWNER};
use bevy::prelude::*;

use crate::client_state::ClientState;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, SelfPlayer};

const ME: u64 = 0x77;
const OTHER: u64 = 0x99;
const SELLER: u64 = 0x55;
const AUCTIONEER: u64 = 0xF130_0000_0000_0A0C;

/// The auction plugin in a world of its own, its getters read through a VM that records each list
/// update with the list's order at the moment it fires.
struct House {
    app: App,
    rx: crossbeam_channel::Receiver<ClientCommand>,
}

impl House {
    fn new() -> Self {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::InWorld)
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .configure_sets(
                Update,
                (crate::ui_script::UiFeed, crate::ui_script::UiDrain).chain(),
            )
            .add_plugins(super::UiAuctionPlugin);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.world_mut().spawn((SelfPlayer, crate::net::Guid(ME)));
        let mut house = Self { app, rx };
        house.new_vm();
        house
    }

    /// A fresh VM, as a world entry builds one, listening for the three list updates.
    fn new_vm(&mut self) {
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                function ORDER(list)
                    local t = {}
                    for i = 1, (GetNumAuctionItems(list)) do
                        t[i] = (GetAuctionItemInfo(list, i)) or "?"
                    end
                    return table.concat(t, ",")
                end
                function SELLERS(list)
                    local t = {}
                    for i = 1, (GetNumAuctionItems(list)) do
                        local _, _, _, _, _, _, _, _, _, _, _, owner = GetAuctionItemInfo(list, i)
                        t[i] = owner or "?"
                    end
                    return table.concat(t, ",")
                end
                SEEN = {}
                local lists = {
                    AUCTION_ITEM_LIST_UPDATE = "list",
                    AUCTION_BIDDER_LIST_UPDATE = "bidder",
                    AUCTION_OWNED_LIST_UPDATE = "owner",
                }
                local f = CreateFrame("Frame")
                for e in pairs(lists) do f:RegisterEvent(e) end
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, event .. "=" .. ORDER(lists[event]))
                end)
                "#,
            )
            .unwrap();
        self.app.insert_non_send_resource(script);
    }

    fn item(&mut self, entry: u32, name: &str, level: u32, quality: u32) {
        let mut t = crate::items::test_template(name);
        t.required_level = level;
        t.quality = quality;
        self.app
            .world_mut()
            .resource_mut::<Items>()
            .insert_template(entry, Some(t));
    }

    fn name(&mut self, guid: u64, name: &str) {
        self.app
            .world_mut()
            .resource_mut::<NameCache>()
            .insert_player(guid, name.into(), None);
    }

    fn dispatch(&mut self, event: SessionEvent) {
        crate::net::handlers::dispatch(self.app.world_mut(), vec![event]);
        self.app.update();
    }

    fn open(&mut self, auctioneer: u64) {
        self.dispatch(SessionEvent::AuctionHello {
            auctioneer,
            house_id: 1,
        });
    }

    fn land(&mut self, which: usize, auctions: Vec<AuctionListEntry>) {
        let total_count = auctions.len() as u32;
        self.dispatch(match which {
            LIST => SessionEvent::AuctionListResult {
                auctions,
                total_count,
            },
            BIDDER => SessionEvent::AuctionBidderListResult {
                auctions,
                total_count,
            },
            _ => SessionEvent::AuctionOwnerListResult {
                auctions,
                total_count,
            },
        });
    }

    /// The close core, as the window's hide or a walk-away runs it.
    fn close(&mut self) {
        super::close_core(self.app.world_mut());
        self.app.update();
    }

    /// The world-enter cascade (`0x4908c0`): a login, a cross-map worldport or a `/reload`.
    fn cascade(&mut self) {
        self.app
            .world_mut()
            .write_message(crate::net::WorldEnterCascadeMessage);
        self.app.update();
    }

    /// `GetTime()`'s clock, the one the list results and `GetAuctionItemTimeLeft` read.
    fn clock(&mut self, secs: f64) {
        self.script().set_now(secs);
    }

    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, chunk: &str) -> T {
        self.script().eval(chunk).unwrap()
    }

    fn script(&mut self) -> Mut<'_, UiScript> {
        self.app.world_mut().non_send_resource_mut::<UiScript>()
    }

    /// A chunk of Lua, then two frames: whatever the chunk leaves for the drain has reached the
    /// getters by the second.
    fn lua(&mut self, chunk: &str) {
        self.script().run(chunk).unwrap();
        self.app.update();
        self.app.update();
    }

    fn order(&mut self, list: &str) -> String {
        self.script()
            .eval(&format!("return ORDER({list:?})"))
            .unwrap()
    }

    fn sellers(&mut self, list: &str) -> String {
        self.script()
            .eval(&format!("return SELLERS({list:?})"))
            .unwrap()
    }

    /// The list updates since the last call, each with its list's order as it fired.
    fn seen(&mut self) -> Vec<String> {
        let script = self.script();
        let seen = script.eval::<Vec<String>>("return SEEN").unwrap();
        script.run("SEEN = {}").unwrap();
        seen
    }

    fn bids(&mut self) -> Vec<u32> {
        self.rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::AuctionPlaceBid { auction_id, .. } => Some(auction_id),
                _ => None,
            })
            .collect()
    }

    /// Leaving the world and entering it again: character select, a new VM, the world.
    fn enter_world(&mut self) {
        self.app
            .world_mut()
            .resource_mut::<NextState<ClientState>>()
            .set(ClientState::CharSelect);
        self.app.update();
        self.new_vm();
        self.app
            .world_mut()
            .resource_mut::<NextState<ClientState>>()
            .set(ClientState::InWorld);
        self.app.update();
    }
}

/// A row of item `item`, sold by [`SELLER`] for 100c with no bid, an hour left.
fn entry(auction_id: u32, item: u32) -> AuctionListEntry {
    AuctionListEntry {
        auction_id,
        item_entry: item,
        perm_enchant: 0,
        random_property_id: 0,
        suffix_factor: 0,
        count: 1,
        spell_charges: 0,
        owner_guid: SELLER,
        start_bid: 100,
        min_increment: 0,
        buyout: 0,
        time_left_ms: 60 * 60 * 1000,
        bidder_guid: 0,
        current_bid: 0,
    }
}

/// The world entry fills each list's stack with all eight keys (`0x4cc236`-`0x4cc274`), so a
/// page arrives sorted with no header clicked: Browse by level first (`0x4cc246`), the Auctions
/// list by status first (`0x849064`), a bid-on auction ahead of one without.
#[test]
fn a_page_arrives_sorted_by_the_world_entrys_eight_keys() {
    let mut h = House::new();
    h.item(1, "Charlie", 30, 1);
    h.item(2, "Alpha", 10, 1);
    h.item(3, "Bravo", 20, 1);
    h.open(AUCTIONEER);
    h.land(LIST, vec![entry(1, 1), entry(2, 2), entry(3, 3)]);
    assert_eq!(h.order("list"), "Alpha,Bravo,Charlie");

    let mut unbid = entry(4, 2);
    unbid.owner_guid = ME;
    let mut bid_on = entry(5, 3);
    bid_on.owner_guid = ME;
    bid_on.bidder_guid = OTHER;
    bid_on.current_bid = 500;
    h.land(OWNER, vec![unbid, bid_on]);
    assert_eq!(h.order("owner"), "Bravo,Alpha");
}

/// Nothing but the world entry and the header drivers writes a stack (`0x4cd850`), so a column
/// the player chose stays the primary key after the window closes, at any auctioneer.
#[test]
fn a_header_click_outlasts_the_window_and_the_auctioneer() {
    let mut h = House::new();
    h.item(1, "Low", 10, 1);
    h.item(2, "High", 20, 4);
    h.open(AUCTIONEER);
    h.lua("SortAuctionItems('list', 'quality')");
    h.lua("CloseAuctionHouse()");
    h.open(AUCTIONEER + 1);
    h.land(LIST, vec![entry(1, 1), entry(2, 2)]);
    assert_eq!(
        h.order("list"),
        "High,Low",
        "quality is still the primary key"
    );
}

/// The stacks are filled at world entry (`0x48f5f9` → `0x4cc0f0`) and only there, so a new
/// session in the world starts from the reference's eight keys again.
#[test]
fn a_world_entry_seeds_the_stacks_again() {
    let mut h = House::new();
    h.item(1, "Low", 10, 1);
    h.item(2, "High", 20, 4);
    h.open(AUCTIONEER);
    h.lua("SortAuctionItems('list', 'quality')");
    h.enter_world();
    h.open(AUCTIONEER);
    h.land(LIST, vec![entry(1, 1), entry(2, 2)]);
    assert_eq!(
        h.order("list"),
        "Low,High",
        "level is the primary key again"
    );
}

/// A page with an item the cache lacks waits: each miss counts (`0x4cc9c8`), the result sorts and
/// signals only at zero (`0x4cc9e8`-`0x4cc9f1`), and the last arrival does it (`0x4cca10`), once.
#[test]
fn a_page_waits_for_its_items_then_sorts_and_signals_once() {
    let mut h = House::new();
    h.item(1, "Bravo", 20, 1);
    h.open(AUCTIONEER);
    h.seen();
    h.land(LIST, vec![entry(1, 1), entry(2, 2)]);
    h.app.update();
    assert!(
        h.seen().is_empty(),
        "no update while an item is outstanding"
    );
    h.item(2, "Alpha", 10, 1);
    h.app.update();
    assert_eq!(h.seen(), ["AUCTION_ITEM_LIST_UPDATE=Alpha,Bravo"]);
    h.app.update();
    h.app.update();
    assert!(h.seen().is_empty(), "the page signalled once");
}

/// A seller's name arriving signals the list and sorts nothing (`0x4cf290`), so its row holds
/// still under the seller sort until the next page or header click.
#[test]
fn a_sellers_name_signals_the_list_without_sorting_it() {
    const AL: u64 = 0x61;
    const BOB: u64 = 0x62;
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.name(BOB, "Bob");
    h.open(AUCTIONEER);
    let mut al = entry(1, 1);
    al.owner_guid = AL;
    let mut bob = entry(2, 1);
    bob.owner_guid = BOB;
    h.land(LIST, vec![al, bob]);
    h.lua("SortAuctionItems('list', 'seller')");
    assert_eq!(h.sellers("list"), "Bob,?");
    h.seen();
    h.name(AL, "Al");
    h.app.update();
    assert_eq!(h.seen().len(), 1, "the name signals the list");
    assert_eq!(h.sellers("list"), "Bob,Al", "and does not sort it");
}

/// The duration sort compares each row's deadline, the result's tick plus the wire's
/// milliseconds (`0x4cc966`-`0x4cc975`, mode 3 `0x4cdad4`), not its time-left band.
#[test]
fn the_time_left_sort_orders_one_band_by_each_rows_deadline() {
    let mut h = House::new();
    h.item(1, "Later", 10, 1);
    h.item(2, "Sooner", 10, 1);
    h.open(AUCTIONEER);
    let mut later = entry(1, 1);
    later.time_left_ms = (3 * 60 + 10) * 60 * 1000;
    let mut sooner = entry(2, 2);
    sooner.time_left_ms = (2 * 60 + 30) * 60 * 1000;
    h.land(LIST, vec![later, sooner]);
    h.lua("SortAuctionItems('list', 'duration')");
    assert_eq!(
        h.order("list"),
        "Sooner,Later",
        "both are Long; the sooner first"
    );
    h.lua("SortAuctionItems('list', 'duration')");
    assert_eq!(
        h.order("list"),
        "Later,Sooner",
        "reversed, the latest first"
    );
}

/// The status sort ranks each row's bidder (`0x4cdb10`-`0x4cdb7c`): the player 2, another bidder
/// 1, no bid 0, the higher rank first.
#[test]
fn the_status_sort_ranks_the_players_bid_then_anothers_then_none() {
    let mut h = House::new();
    h.item(1, "None", 10, 1);
    h.item(2, "Other", 10, 1);
    h.item(3, "Mine", 10, 1);
    h.open(AUCTIONEER);
    let mut other = entry(2, 2);
    other.bidder_guid = OTHER;
    other.current_bid = 100;
    let mut mine = entry(3, 3);
    mine.bidder_guid = ME;
    mine.current_bid = 100;
    h.land(LIST, vec![entry(1, 1), other, mine]);
    h.lua("SortAuctionItems('list', 'status')");
    assert_eq!(h.order("list"), "Mine,Other,None");
}

/// The level sort floors both rows at 1 (`0x4cda98`-`0x4cdab4`), so an item with no requirement
/// ties a level-1 item and the next key decides: here the bid, two keys down the Bids stack.
#[test]
fn the_level_sort_ranks_no_requirement_with_level_one() {
    let mut h = House::new();
    h.item(1, "Zero", 0, 1);
    h.item(2, "One", 1, 1);
    h.open(AUCTIONEER);
    let mut zero = entry(1, 1);
    zero.bidder_guid = ME;
    zero.current_bid = 200;
    let mut one = entry(2, 2);
    one.bidder_guid = ME;
    one.current_bid = 100;
    h.land(BIDDER, vec![zero, one]);
    h.lua("SortAuctionItems('bidder', 'level')");
    assert_eq!(
        h.order("bidder"),
        "One,Zero",
        "level 0 ranks as 1; the bid decides"
    );
}

/// The seller sort compares names without case, and a seller the cache has not named sorts after
/// every named one (`0x4cdc23`, `0x4cdc31`), before them when reversed.
#[test]
fn the_seller_sort_puts_an_unknown_seller_after_every_named_one() {
    const NOBODY: u64 = 0x60;
    const BOB: u64 = 0x62;
    const CAL: u64 = 0x63;
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.name(BOB, "bob");
    h.name(CAL, "Cal");
    h.open(AUCTIONEER);
    let mut unknown = entry(1, 1);
    unknown.owner_guid = NOBODY;
    let mut cal = entry(2, 1);
    cal.owner_guid = CAL;
    let mut bob = entry(3, 1);
    bob.owner_guid = BOB;
    h.land(LIST, vec![unknown, cal, bob]);
    h.lua("SortAuctionItems('list', 'seller')");
    assert_eq!(h.sellers("list"), "bob,Cal,?");
    h.lua("SortAuctionItems('list', 'seller')");
    assert_eq!(
        h.sellers("list"),
        "?,Cal,bob",
        "reversed, the unknown first"
    );
}

/// `SortAuctionItems` sorts before it returns, and `PlaceAuctionBid` reads the record at its row
/// when called (`0x4cf610`), so a bid on the line after a header click names the auction the
/// click put on that row.
#[test]
fn a_bid_after_a_header_click_names_the_auction_the_click_put_on_its_row() {
    let mut h = House::new();
    h.item(1, "Cheap", 10, 1);
    h.item(2, "Dear", 10, 1);
    h.open(AUCTIONEER);
    let mut cheap = entry(1, 1);
    cheap.buyout = 100;
    let mut dear = entry(2, 2);
    dear.buyout = 900;
    h.land(LIST, vec![dear, cheap]);
    h.bids();
    h.lua(
        "SortAuctionItems('list', 'buyout') SortAuctionItems('list', 'buyout') \
         PlaceAuctionBid('list', 1, 100)",
    );
    assert_eq!(
        h.bids(),
        [1],
        "the cheaper auction, first under a reversed buyout"
    );
}

/// `GetAuctionItemTimeLeft` reads the deadline against the clock at the call (`0x4cf548`), and the
/// deadline is the result's tick plus the wire's milliseconds (`0x4cc865`, `0x4cc966`): a row
/// counts down between results.
#[test]
fn the_time_left_counts_down_from_the_results_tick() {
    let mut h = House::new();
    h.item(1, "Copper Bar", 1, 1);
    h.clock(100.0);
    h.open(AUCTIONEER);
    let mut row = entry(1, 1);
    row.time_left_ms = 31 * 60 * 1000;
    h.land(LIST, vec![row]);
    let left = "return GetAuctionItemTimeLeft('list', 1)";
    assert_eq!(h.eval::<i64>(left), 2, "31 minutes: Medium");
    h.clock(220.0);
    assert_eq!(h.eval::<i64>(left), 1, "two minutes on, 29 left: Short");
}

/// The close (`0x4cd5f0`) zeroes the auctioneer, the house and the one-shot flags and empties the
/// sell slot, nothing more: the lists, their stacks, the selection and the query throttle stand,
/// and the getters answer them. The world-enter cascade's reset (`0x490a05` → `0x4cc280`) empties
/// the counts, the selection and the throttle, and leaves the stacks.
#[test]
fn a_close_keeps_the_lists_and_the_world_enter_cascade_empties_them() {
    let mut h = House::new();
    h.item(1, "Copper Bar", 1, 1);
    h.item(2, "Tin Bar", 1, 2);
    h.open(AUCTIONEER);
    h.land(LIST, vec![entry(1, 1), entry(2, 2)]);
    h.lua(
        "SortAuctionItems('list', 'quality') SortAuctionItems('list', 'quality') \
         SetSelectedAuctionItem('list', 2) QueryAuctionItems('')",
    );
    assert_eq!(h.order("list"), "Copper Bar,Tin Bar", "quality reversed");
    let read = "local n, total = GetNumAuctionItems('list') return n, total, \
                GetSelectedAuctionItem('list'), IsAuctionSortReversed('list', 'quality'), \
                CanSendAuctionQuery()";
    type Read = (i64, i64, i64, Option<i64>, Option<i64>);
    assert_eq!(h.eval::<Read>(read), (2, 2, 2, Some(1), None));
    h.close();
    assert_eq!(h.eval::<Read>(read), (2, 2, 2, Some(1), None), "the close");
    assert_eq!(h.order("list"), "Copper Bar,Tin Bar");
    h.cascade();
    assert_eq!(
        h.eval::<Read>(read),
        (0, 0, 0, Some(1), Some(1)),
        "the cascade's reset"
    );
}

/// The hello (`0x4cd570`) zeroes every list's count and total (`0x4cd5b4`-`0x4cd5cd`), from the
/// auctioneer already open as from any other.
#[test]
fn a_hello_empties_every_list_even_from_the_same_auctioneer() {
    let mut h = House::new();
    h.item(1, "Copper Bar", 1, 1);
    h.open(AUCTIONEER);
    h.land(LIST, vec![entry(1, 1), entry(2, 1)]);
    h.land(OWNER, vec![entry(3, 1)]);
    h.open(AUCTIONEER);
    let read = "local a, b = GetNumAuctionItems('list') local c, d = \
                GetNumAuctionItems('owner') return a, b, c, d";
    assert_eq!(h.eval::<(i64, i64, i64, i64)>(read), (0, 0, 0, 0));
}

/// A result has its lists whatever the session (`0x4cc7f0` reads no auctioneer): one landing
/// after the close is written, sorted and signalled.
#[test]
fn a_result_after_the_close_still_lands_and_signals() {
    let mut h = House::new();
    h.item(1, "Copper Bar", 1, 1);
    h.open(AUCTIONEER);
    h.close();
    h.seen();
    h.land(LIST, vec![entry(1, 1)]);
    assert_eq!(
        h.eval::<(i64, i64)>("return GetNumAuctionItems('list')"),
        (1, 1)
    );
    assert_eq!(h.seen(), ["AUCTION_ITEM_LIST_UPDATE=Copper Bar"]);
}

/// The selection is an auction id (`0x849058`) that neither the close nor the hello writes: the
/// same auction on a later page is selected again.
#[test]
fn the_selection_outlasts_the_close_and_the_hello() {
    let mut h = House::new();
    h.item(1, "Copper Bar", 1, 1);
    h.item(2, "Tin Bar", 1, 1);
    h.open(AUCTIONEER);
    h.land(LIST, vec![entry(1, 1), entry(2, 2)]);
    h.lua("SetSelectedAuctionItem('list', 2)");
    h.close();
    h.open(AUCTIONEER);
    h.land(LIST, vec![entry(2, 2), entry(1, 1), entry(3, 1)]);
    assert_eq!(h.order("list"), "Copper Bar,Copper Bar,Tin Bar");
    assert_eq!(h.eval::<i64>("return GetSelectedAuctionItem('list')"), 3);
}
