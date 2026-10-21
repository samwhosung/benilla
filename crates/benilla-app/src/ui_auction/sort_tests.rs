//! The three lists end to end, their sort, their time left and their life across the window: the
//! wire's results through the handlers, the feed and the drain to the VM's getters, as the window
//! reads them.

use benilla_protocol::messages::{
    AuctionBidderNotification, AuctionCommandTail, AuctionListEntry, AuctionOwnerNotification,
};
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
        app.insert_resource(NetCommands(tx))
            .init_resource::<crate::net::GuidIndex>();
        // A purse for the bids (`PLAYER_FIELD_COINAGE`).
        app.world_mut().spawn((
            SelfPlayer,
            crate::net::Guid(ME),
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                1176, 1_000_000,
            )])),
        ));
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

    /// A chunk of Lua with the auction verbs' host lent, then two frames: whatever the chunk
    /// leaves for the drain has reached the getters by the second.
    fn lua(&mut self, chunk: &str) {
        let world = self.app.world_mut();
        let mut state =
            bevy::ecs::system::SystemState::<(NonSendMut<UiScript>, super::AuctionVerbs)>::new(
                world,
            );
        {
            let (mut script, mut verbs) = state.get_mut(world);
            script
                .hosted(&mut AuctionHost(&mut verbs), |s| s.run(chunk))
                .unwrap();
        }
        state.apply(world);
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

    /// The packet through its handlers alone, with no frame after: what the handler signals
    /// itself.
    fn handle(&mut self, event: SessionEvent) {
        crate::net::handlers::dispatch(self.app.world_mut(), vec![event]);
    }

    /// From here on, each list update with its first row's bid as the update's handler reads it,
    /// and each red line, in the order they fire.
    fn hear_bids(&mut self) {
        self.script()
            .run(
                r#"
                HEARD = {}
                ERR_AUCTION_HIGHER_BID = "ERR_AUCTION_HIGHER_BID"
                local lists = {
                    AUCTION_ITEM_LIST_UPDATE = "list",
                    AUCTION_BIDDER_LIST_UPDATE = "bidder",
                    AUCTION_OWNED_LIST_UPDATE = "owner",
                }
                local f = CreateFrame("Frame")
                for e in pairs(lists) do f:RegisterEvent(e) end
                f:RegisterEvent("UI_ERROR_MESSAGE")
                f:SetScript("OnEvent", function()
                    if event == "UI_ERROR_MESSAGE" then
                        table.insert(HEARD, arg1)
                        return
                    end
                    local _, _, _, _, _, _, _, _, _, bid = GetAuctionItemInfo(lists[event], 1)
                    table.insert(HEARD, event .. "=" .. tostring(bid))
                end)
                "#,
            )
            .unwrap();
    }

    /// What [`Self::hear_bids`] heard since the last call.
    fn heard(&mut self) -> Vec<String> {
        let script = self.script();
        let heard = script.eval::<Vec<String>>("return HEARD").unwrap();
        script.run("HEARD = {}").unwrap();
        heard
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

/// The call host's auction applier alone, lent by [`House::lua`]: this world has no other
/// engine half.
struct AuctionHost<'a, 'w, 's>(&'a mut super::AuctionVerbs<'w, 's>);

impl benilla_ui::script::CallHost for AuctionHost<'_, '_, '_> {
    fn apply(
        &mut self,
        script: &mut UiScript,
        call: benilla_ui::script::ScriptCall,
    ) -> Option<benilla_ui::script::CallTail> {
        if let benilla_ui::script::ScriptCall::Auction(call) = call {
            self.0.apply(script, call);
        }
        None
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

/// A seller's name arriving signals the list whose read asked it and sorts nothing
/// (`0x4cf290`), so its row holds still under the seller sort until the next page or header
/// click. The owner list shows the same seller but nothing read it, so it registered no callback
/// (`0x55f080`) and stays silent; a second read of the list registers nothing more.
#[test]
fn a_sellers_name_signals_the_list_without_sorting_it() {
    const AL: u64 = 0x61;
    const BOB: u64 = 0x62;
    let mut h = House::new();
    // A VM whose recorder reads no rows, so only the reads below ask names.
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"
            function SELLERS(list)
                local t = {}
                for i = 1, (GetNumAuctionItems(list)) do
                    local _, _, _, _, _, _, _, _, _, _, _, owner = GetAuctionItemInfo(list, i)
                    t[i] = owner or "?"
                end
                return table.concat(t, ",")
            end
            EVENTS = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("AUCTION_ITEM_LIST_UPDATE")
            f:RegisterEvent("AUCTION_OWNED_LIST_UPDATE")
            f:RegisterEvent("AUCTION_BIDDER_LIST_UPDATE")
            f:SetScript("OnEvent", function() table.insert(EVENTS, event) end)
            "#,
        )
        .unwrap();
    h.app.insert_non_send_resource(script);
    h.item(1, "Cloth", 10, 1);
    h.name(BOB, "Bob");
    h.open(AUCTIONEER);
    let mut al = entry(1, 1);
    al.owner_guid = AL;
    let mut bob = entry(2, 1);
    bob.owner_guid = BOB;
    h.land(LIST, vec![al, bob]);
    h.land(OWNER, vec![al]);
    h.lua("SortAuctionItems('list', 'seller')");
    assert_eq!(h.sellers("list"), "Bob,?");
    assert_eq!(h.sellers("list"), "Bob,?");
    h.script().run("EVENTS = {}").unwrap();
    h.name(AL, "Al");
    h.app.update();
    assert_eq!(
        h.eval::<Vec<String>>("return EVENTS"),
        ["AUCTION_ITEM_LIST_UPDATE"],
        "the name signals the list that asked, once"
    );
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

/// The wire's sends since the last call that list or name, as text.
fn requests(h: &House) -> Vec<String> {
    h.rx.try_iter()
        .filter(|c| {
            matches!(
                c,
                ClientCommand::AuctionListOwnerItems { .. }
                    | ClientCommand::AuctionListBidderItems { .. }
                    | ClientCommand::AuctionListItems { .. }
                    | ClientCommand::NameQuery { .. }
            )
        })
        .map(|c| format!("{c:?}"))
        .collect()
}

/// A sale removes the auction from each list that holds it (`0x4cd468` → `0x4cdfe0`), browse
/// then owner, each signalled with the row gone and the list's order kept, and asks for no list:
/// the reference re-queries nothing.
#[test]
fn a_sale_removes_the_auction_in_place_of_a_requery() {
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.item(2, "Silk", 20, 1);
    h.name(SELLER, "Seller");
    h.name(ME, "Me");
    h.open(AUCTIONEER);
    let mine = |auction_id, item| AuctionListEntry {
        owner_guid: ME,
        ..entry(auction_id, item)
    };
    h.land(LIST, vec![entry(1, 1), mine(2, 2), entry(3, 1)]);
    h.land(OWNER, vec![mine(2, 2), mine(4, 1)]);
    h.lua("SortAuctionItems('list', 'level')");
    assert_eq!(h.order("list"), "Silk,Cloth,Cloth", "level reversed");
    h.seen();
    let _ = requests(&h);
    h.dispatch(SessionEvent::AuctionOwnerNotification(
        AuctionOwnerNotification {
            auction_id: 2,
            bid: 500,
            out_bid: 0,
            bidder_guid: 0,
            item_entry: 2,
            random_property_id: 0,
        },
    ));
    assert_eq!(
        h.seen(),
        [
            "AUCTION_ITEM_LIST_UPDATE=Cloth,Cloth",
            "AUCTION_OWNED_LIST_UPDATE=Cloth"
        ]
    );
    assert_eq!(
        h.eval::<(i64, i64)>("return GetNumAuctionItems('list')"),
        (2, 3),
        "the count drops, the total stays"
    );
    h.app.update();
    h.app.update();
    assert!(h.seen().is_empty(), "signalled once");
    assert!(requests(&h).is_empty(), "no list asked again");
}

/// A placed bid's browse row takes the bid last sent and the player as its bidder
/// (`0x4cc564`-`0x4cc590`), and the browse list signals it.
#[test]
fn a_placed_bid_shows_on_its_browse_row() {
    let mut h = House::new();
    h.app
        .world_mut()
        .insert_resource(crate::net::SelfGuid(Some(ME)));
    h.item(1, "Cloth", 10, 1);
    h.open(AUCTIONEER);
    h.land(LIST, vec![entry(1, 1)]);
    h.seen();
    h.app
        .world_mut()
        .resource_mut::<super::AuctionOpen>()
        .last_bid = 150;
    h.dispatch(SessionEvent::AuctionCommandResult {
        auction_id: 1,
        action: 2,
        error: 0,
        tail: AuctionCommandTail::BidPlaced { new_min_outbid: 7 },
    });
    assert_eq!(h.seen(), ["AUCTION_ITEM_LIST_UPDATE=Cloth"]);
    assert_eq!(
        h.eval::<(i64, i64, i64)>(
            "local _,_,_,_,_,_,_,inc,_,bid,high = GetAuctionItemInfo('list', 1) \
             return inc, bid, high"
        ),
        (7, 150, 1)
    );
}

/// A landed page asks no name, and a seller sort reads the name cache without asking
/// (`0x4cdbda`); `GetAuctionItemInfo` asks the seller it misses (`0x4cf1c8`), and the name's
/// landing signals the list.
#[test]
fn a_rows_names_are_asked_by_its_read_not_by_its_page() {
    const AL: u64 = 0x61;
    const BO: u64 = 0x62;
    let mut h = House::new();
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"UPDATES = 0
            local f = CreateFrame("Frame")
            f:RegisterEvent("AUCTION_ITEM_LIST_UPDATE")
            f:SetScript("OnEvent", function() UPDATES = UPDATES + 1 end)"#,
        )
        .unwrap();
    h.app.insert_non_send_resource(script);
    h.item(1, "Cloth", 10, 1);
    h.open(AUCTIONEER);
    let seller = |auction_id, guid| AuctionListEntry {
        owner_guid: guid,
        ..entry(auction_id, 1)
    };
    h.land(LIST, vec![seller(1, AL), seller(2, BO)]);
    h.lua("SortAuctionItems('list', 'seller')");
    assert!(
        requests(&h).is_empty(),
        "neither the page nor the sort asks"
    );
    h.lua("GetAuctionItemInfo('list', 1)");
    let asked = requests(&h);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].contains(&format!("{}", AL)), "{asked:?}");
    let before = h.eval::<i64>("return UPDATES");
    h.name(AL, "Al");
    h.app.update();
    assert_eq!(h.eval::<i64>("return UPDATES"), before + 1);
}

/// An outbid on house 1, auction 7: `OTHER` now bids 900 with a 45c step.
fn outbid() -> SessionEvent {
    SessionEvent::AuctionBidderNotification(AuctionBidderNotification {
        house_id: 1,
        auction_id: 7,
        bidder_guid: OTHER,
        bid_or_zero: 900,
        out_bid: 45,
        item_entry: 1,
        random_property_id: 0,
    })
}

/// An outbid patches the bidder row and signals its list, then the browse row and its list,
/// inside the handler (`0x4cd033`-`0x4cd09e`, `0x4cd0d8`-`0x4cd143`): each update's handler reads
/// its row patched, and the frame after signals nothing more. The same notice again, the rows
/// unchanged, signals both again.
#[test]
fn an_outbid_signals_the_bidder_then_the_browse_list_in_its_handler() {
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.open(AUCTIONEER);
    h.land(BIDDER, vec![entry(7, 1)]);
    h.land(LIST, vec![entry(7, 1)]);
    h.hear_bids();
    h.handle(outbid());
    let both = [
        "AUCTION_BIDDER_LIST_UPDATE=900",
        "AUCTION_ITEM_LIST_UPDATE=900",
    ];
    assert_eq!(h.heard(), both);
    h.app.update();
    assert!(h.heard().is_empty(), "the frame after signals nothing");
    h.handle(outbid());
    assert_eq!(h.heard(), both, "an unchanged row signals too");
}

/// A bid on one of ours patches the owner row and signals its list, then the browse row and its
/// list, inside the handler (`0x4cd296`-`0x4cd301`, `0x4cd306`-`0x4cd3a8`).
#[test]
fn a_bid_on_ours_signals_the_owned_then_the_browse_list_in_its_handler() {
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.open(AUCTIONEER);
    let mine = AuctionListEntry {
        owner_guid: ME,
        ..entry(7, 1)
    };
    h.land(OWNER, vec![mine]);
    h.land(LIST, vec![mine]);
    h.hear_bids();
    h.handle(SessionEvent::AuctionOwnerNotification(
        AuctionOwnerNotification {
            auction_id: 7,
            bid: 500,
            out_bid: 25,
            bidder_guid: OTHER,
            item_entry: 1,
            random_property_id: 0,
        },
    ));
    assert_eq!(
        h.heard(),
        [
            "AUCTION_OWNED_LIST_UPDATE=500",
            "AUCTION_ITEM_LIST_UPDATE=500"
        ]
    );
    h.app.update();
    assert!(h.heard().is_empty(), "the frame after signals nothing");
}

/// Error 5 patches the browse row and signals it, then the bidder row and its list, and shows its
/// line last (`0x4cc6c9`-`0x4cc703`, `0x4cc72f`-`0x4cc769`, `0x4cc76e`), inside the handler.
#[test]
fn a_higher_bid_signals_browse_then_bidder_then_its_line() {
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.open(AUCTIONEER);
    h.land(LIST, vec![entry(7, 1)]);
    h.land(BIDDER, vec![entry(7, 1)]);
    h.hear_bids();
    h.handle(SessionEvent::AuctionCommandResult {
        auction_id: 7,
        action: 2,
        error: 5,
        tail: AuctionCommandTail::HigherBid {
            new_bidder_guid: OTHER,
            new_bid: 900,
            new_min_outbid: 45,
        },
    });
    assert_eq!(
        h.heard(),
        [
            "AUCTION_ITEM_LIST_UPDATE=900",
            "AUCTION_BIDDER_LIST_UPDATE=900",
            "ERR_AUCTION_HIGHER_BID"
        ]
    );
    h.app.update();
    assert!(h.heard().is_empty(), "the frame after signals nothing");
}

/// A placed bid patches the browse row and signals it inside the handler (`0x4cc564`-`0x4cc5d0`).
#[test]
fn a_placed_bid_signals_the_browse_list_in_its_handler() {
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.open(AUCTIONEER);
    h.land(LIST, vec![entry(7, 1)]);
    h.app
        .world_mut()
        .resource_mut::<super::AuctionOpen>()
        .last_bid = 150;
    h.hear_bids();
    h.handle(SessionEvent::AuctionCommandResult {
        auction_id: 7,
        action: 2,
        error: 0,
        tail: AuctionCommandTail::BidPlaced { new_min_outbid: 7 },
    });
    assert_eq!(h.heard(), ["AUCTION_ITEM_LIST_UPDATE=150"]);
    h.app.update();
    assert!(h.heard().is_empty(), "the frame after signals nothing");
}

/// A list still waiting on an item record takes the patch without its update, as each patch's
/// event tests its list's pending count (`0x4cd090`, `0x4cd135`): here the bidder list waits and
/// the browse list does not.
#[test]
fn a_pending_list_takes_a_patch_without_its_update() {
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.open(AUCTIONEER);
    h.land(BIDDER, vec![entry(7, 2)]);
    h.land(LIST, vec![entry(7, 1)]);
    h.hear_bids();
    h.handle(outbid());
    assert_eq!(h.heard(), ["AUCTION_ITEM_LIST_UPDATE=900"]);
}

/// A won auction leaves each list inside the handler, each list signalled as it goes
/// (`0x4cd173` → `0x4cdfe0`), and the frame after signals nothing more.
#[test]
fn a_won_auction_leaves_the_lists_in_its_handler() {
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.item(2, "Silk", 20, 1);
    h.open(AUCTIONEER);
    h.land(LIST, vec![entry(7, 1), entry(8, 2)]);
    h.land(BIDDER, vec![entry(7, 1)]);
    h.seen();
    h.handle(SessionEvent::AuctionBidderNotification(
        AuctionBidderNotification {
            house_id: 1,
            auction_id: 7,
            bidder_guid: 0,
            bid_or_zero: 0,
            out_bid: 0,
            item_entry: 1,
            random_property_id: 0,
        },
    ));
    assert_eq!(
        h.seen(),
        [
            "AUCTION_ITEM_LIST_UPDATE=Silk",
            "AUCTION_BIDDER_LIST_UPDATE="
        ]
    );
    h.app.update();
    assert!(h.seen().is_empty(), "the frame after signals nothing");
}

/// The world entry's start-up empties the outbid list and resets its room to eight
/// (`0x4cc1d0`, `0x4cc207`), so after a relog the bidder request sends no old session's ids.
#[test]
fn a_relog_empties_the_outbid_list() {
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.open(AUCTIONEER);
    h.dispatch(outbid());
    {
        let mut auction = h.app.world_mut().resource_mut::<super::AuctionOpen>();
        assert_eq!(auction.outbid, [(1, 7)]);
        auction.outbid_doublings = 2;
    }
    h.enter_world();
    h.open(AUCTIONEER);
    let _ = requests(&h);
    h.lua("GetBidderAuctionItems()");
    assert_eq!(
        requests(&h),
        [format!(
            "{:?}",
            ClientCommand::AuctionListBidderItems {
                auctioneer: AUCTIONEER,
                list_from: 0,
                auction_ids: vec![],
            }
        )]
    );
    assert_eq!(
        h.app
            .world()
            .resource::<super::AuctionOpen>()
            .outbid_doublings,
        0
    );
}

/// A page whose items are all cached is sorted and signalled by its result handler
/// (`0x4cc9e8`-`0x4cc9f1`), so a bid patch later in the same frame signals the list again
/// (`0x4cd13e`).
#[test]
fn a_patch_in_its_pages_frame_signals_after_the_page() {
    let mut h = House::new();
    h.item(1, "Cloth", 10, 1);
    h.open(AUCTIONEER);
    h.hear_bids();
    crate::net::handlers::dispatch(
        h.app.world_mut(),
        vec![
            SessionEvent::AuctionListResult {
                auctions: vec![entry(7, 1)],
                total_count: 1,
            },
            outbid(),
        ],
    );
    assert_eq!(
        h.heard(),
        ["AUCTION_ITEM_LIST_UPDATE=0", "AUCTION_ITEM_LIST_UPDATE=900"]
    );
    h.app.update();
    assert!(h.heard().is_empty(), "the frame after signals nothing");
}
