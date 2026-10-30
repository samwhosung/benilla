//! The auction getters as a listener reads them, and the house's events at their packets. The
//! getters read the records, the item cache and the throttle at the call (`0x4cee40`, `0x4cef96`,
//! `0x4cfbb0`); the events fire inside the handler that raises them: the hello's
//! `AUCTION_HOUSE_SHOW` (`0x4cd5e6`), a notice's line once its item's record lands (`0x4cd190`),
//! a page's update (`0x4cca10`) and a close's `AUCTION_HOUSE_CLOSED` (`0x4cd602`).

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::messages::{
    AuctionBidderNotification, AuctionListEntry, AuctionOwnerNotification,
};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Sender;

use super::unit_view_tests::{created, wire_packets};
use crate::ui_auction::AuctionOpen;

const ME: u64 = 0x7;
const AUCTIONEER: u64 = 0xF130_0000_0000_0A0C;
const BANKER: u64 = 0xF130_0000_0000_0B0B;
const SILK: u32 = 4306;

/// The whole client, headless, in the world.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    sent: crossbeam_channel::Receiver<crate::net::ClientCommand>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// `chunk`'s answer through the lent host when `live`, else off the VM's copy alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        self.eval(false, "local s = Seen Seen = {} return s")
    }

    /// What went on the wire since the last call, as debug text.
    fn sent(&mut self) -> Vec<String> {
        self.sent.try_iter().map(|c| format!("{c:?}")).collect()
    }

    /// `packets` run through their handlers with no frame: the world as the handlers leave it.
    fn handle(&mut self, packets: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), packets);
        self.app.world_mut().flush();
    }
}

/// The client in the world, and a spy whose line for each auction event, system line and monster
/// line carries its name and first argument, and which keeps whether the window is up.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            ERR_AUCTION_SOLD_S = "A buyer has been found for your auction of %s."
            ERR_AUCTION_OUTBID_S = "You have been outbid on %s."
            Seen, Up = {}, false
            local f = CreateFrame("Frame")
            for _, e in ipairs({"AUCTION_HOUSE_SHOW", "AUCTION_HOUSE_CLOSED",
                    "AUCTION_ITEM_LIST_UPDATE", "BANKFRAME_OPENED", "CHAT_MSG_SYSTEM",
                    "CHAT_MSG_MONSTER_SAY"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                if event == "AUCTION_HOUSE_SHOW" then Up = true end
                if event == "AUCTION_HOUSE_CLOSED" then Up = false end
                table.insert(Seen, event .. ":" .. tostring(arg1))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client { app, inbound, sent };
    // The auctioneer beside us, so the latch's leash (`0x493230`) holds.
    c.frame(vec![vec![
        created(ME, EntityKind::Player, &[(FIELD_UNIT_HEALTH, 100)]),
        created(AUCTIONEER, EntityKind::Unit, &[(FIELD_UNIT_HEALTH, 100)]),
    ]]);
    c.frame(vec![]);
    c.seen();
    c
}

fn hello() -> SessionEvent {
    SessionEvent::AuctionHello {
        auctioneer: AUCTIONEER,
        house_id: 1,
    }
}

/// A browse page of one silk auction, 100c to start, an hour left.
fn page() -> SessionEvent {
    SessionEvent::AuctionListResult {
        auctions: vec![AuctionListEntry {
            auction_id: 7,
            item_entry: SILK,
            perm_enchant: 0,
            random_property_id: 0,
            suffix_factor: 0,
            count: 1,
            spell_charges: 0,
            owner_guid: 0x55,
            start_bid: 100,
            min_increment: 0,
            buyout: 0,
            time_left_ms: 60 * 60 * 1000,
            bidder_guid: 0,
            current_bid: 0,
        }],
        total_count: 1,
    }
}

/// Silk's record, the item query's answer.
fn silk() -> SessionEvent {
    SessionEvent::ItemTemplate {
        entry: SILK,
        info: Some(Box::new(crate::items::test_template("Silk Cloth"))),
    }
}

/// A monster's line, which shows inside its own packet's handler on the chat lane.
fn boar_says(text: &str) -> SessionEvent {
    SessionEvent::Chat(benilla_protocol::messages::ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_MONSTER_SAY,
        language: 0,
        sender_guid: 0xF130_0000_0045_0003,
        target_guid: 0,
        sender_name: Some("Boar".into()),
        channel: None,
        text: text.into(),
        chat_tag: 0,
    })
}

const NAME: &str = "return tostring((GetAuctionItemInfo('list', 1)))";

/// The page lands with silk's record out, so the VM's row holds no name; the record's answer
/// then fills it for a host-lent getter at the call, and the page's tail, which runs at the
/// answer's packet, fills the VM's copy there. The throttle reads the house at the call too.
#[test]
fn the_auction_getters_read_the_house_at_the_call() {
    let mut c = client();
    c.frame(vec![vec![hello()], vec![page()]]);
    assert_eq!(c.eval::<String>(true, NAME), "nil", "the record is out");
    c.handle(vec![silk()]);
    assert_eq!(c.eval::<String>(true, NAME), "Silk Cloth");
    assert_eq!(
        c.eval::<String>(false, NAME),
        "Silk Cloth",
        "the tail's push at the answer's packet"
    );

    const CAN: &str = "return CanSendAuctionQuery() == 1";
    c.app.world_mut().resource_mut::<AuctionOpen>().query_gate = Some(f64::MAX);
    assert!(!c.eval::<bool>(true, CAN), "the throttle is armed");
    assert!(
        c.eval::<bool>(false, CAN),
        "the VM's copy, which nothing pushed"
    );
    c.app.world_mut().resource_mut::<AuctionOpen>().query_gate = None;
    assert!(c.eval::<bool>(true, CAN));
}

/// The hello's show, a sold notice's line once its item's record lands, and the page's update
/// each fire at their packet's place among a chat line's of the packets around them.
#[test]
fn the_auction_events_fire_at_their_packets() {
    let mut c = client();
    let sold = SessionEvent::AuctionOwnerNotification(AuctionOwnerNotification {
        auction_id: 9,
        bid: 500,
        out_bid: 0,
        bidder_guid: 0,
        item_entry: SILK,
        random_property_id: 0,
    });
    c.frame(vec![
        vec![boar_says("a")],
        vec![hello()],
        vec![boar_says("b")],
        vec![sold],
        vec![boar_says("c")],
        vec![page()],
        vec![boar_says("d")],
        vec![silk()],
        vec![boar_says("e")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "AUCTION_HOUSE_SHOW:nil",
            "CHAT_MSG_MONSTER_SAY:b",
            "CHAT_MSG_MONSTER_SAY:c",
            "CHAT_MSG_MONSTER_SAY:d",
            "CHAT_MSG_SYSTEM:A buyer has been found for your auction of Silk Cloth.",
            "AUCTION_ITEM_LIST_UPDATE:nil",
            "CHAT_MSG_MONSTER_SAY:e",
        ]
    );
}

/// A bank's open in the hello's frame closes the auction house on the latch (`0x4930d0`), whose
/// walk clears the latch before the core runs (`0x493310`), so it closes once: the close follows
/// the show it closes, the window ends down, and the VM holds no house.
#[test]
fn a_close_in_the_opens_frame_follows_the_show() {
    let mut c = client();
    c.frame(vec![
        vec![hello()],
        vec![SessionEvent::ShowBank { banker: BANKER }],
    ]);
    assert_eq!(
        c.seen(),
        [
            "AUCTION_HOUSE_SHOW:nil",
            "AUCTION_HOUSE_CLOSED:nil",
            "BANKFRAME_OPENED:nil",
        ]
    );
    assert!(!c.eval::<bool>(false, "return Up"), "the window is down");
    assert_eq!(c.app.world().resource::<AuctionOpen>().auctioneer, None);
    assert_eq!(
        c.eval::<i64>(false, "return GetAuctionHouseDepositRate()"),
        0,
        "the VM's copy of the house is closed"
    );
}

/// A stock-like auction window: `AUCTION_HOUSE_SHOW` shows it, and `AUCTION_HOUSE_CLOSED` hides it,
/// whose `OnHide` calls `CloseAuctionHouse()` (`Blizzard_AuctionUI.xml:1977`).
const STOCK_FRAME: &str = r#"
    local w = CreateFrame("Frame")
    w:RegisterEvent("AUCTION_HOUSE_SHOW")
    w:RegisterEvent("AUCTION_HOUSE_CLOSED")
    w:SetScript("OnEvent", function()
        if event == "AUCTION_HOUSE_SHOW" then Shown = true
        elseif Shown then Shown = false CloseAuctionHouse() end
    end)
"#;

/// Two greetings in one frame (a double right-click): the second's latch closes the first, behind
/// its show, and the window's hide re-enters the close core (`0x4cd5f0`) for the session it is
/// closing, never the one the second greeting opened, which stays up with its verbs live.
#[test]
fn a_close_nested_in_a_parked_close_leaves_the_reopened_session() {
    let mut c = client();
    c.eval::<()>(true, STOCK_FRAME);
    c.frame(vec![vec![hello()], vec![hello()]]);
    // The hide re-enters the first session's core, whose signal comes before its cell's clear
    // (`0x4cd602`, `0x4cd613`), so it closes twice; then the second session shows.
    assert_eq!(
        c.seen(),
        [
            "AUCTION_HOUSE_SHOW:nil",
            "AUCTION_HOUSE_CLOSED:nil",
            "AUCTION_HOUSE_CLOSED:nil",
            "AUCTION_HOUSE_SHOW:nil",
        ]
    );
    assert_eq!(
        c.app.world().resource::<AuctionOpen>().auctioneer,
        Some(AUCTIONEER),
        "the second greeting's session stands"
    );
    assert!(c.eval::<bool>(false, "return Shown"));
    let _ = c.sent.try_iter().count();
    c.eval::<()>(true, "QueryAuctionItems('')");
    assert!(
        c.sent
            .try_iter()
            .any(|cmd| matches!(cmd, crate::net::ClientCommand::AuctionListItems { .. })),
        "a verb reaches the session"
    );
}

/// An outbid notice whose item is cached shows its line first (`0x4ccfd5`), and only then keeps
/// the pair (`0x4ccff3`) and patches the rows (`0x4cd033`): the line's listener reads our bid as
/// the high one, and its bidder request sends no new pair.
#[test]
fn an_outbid_lines_listener_reads_the_rows_before_the_patch() {
    let mut c = client();
    c.frame(vec![vec![silk()]]);
    let ours = AuctionListEntry {
        bidder_guid: ME,
        current_bid: 500,
        min_increment: 25,
        ..match page() {
            SessionEvent::AuctionListResult { auctions, .. } => auctions[0],
            _ => unreachable!(),
        }
    };
    c.frame(vec![
        vec![hello()],
        vec![SessionEvent::AuctionBidderListResult {
            auctions: vec![ours],
            total_count: 1,
        }],
    ]);
    c.eval::<()>(
        true,
        r#"
        local g = CreateFrame("Frame")
        g:RegisterEvent("CHAT_MSG_SYSTEM")
        g:SetScript("OnEvent", function()
            local _,_,_,_,_,_,_,_,_,bid,high = GetAuctionItemInfo("bidder", 1)
            Read = tostring(bid) .. "/" .. tostring(high)
            GetBidderAuctionItems(0)
        end)
        "#,
    );
    let _ = c.sent.try_iter().count();
    c.frame(vec![vec![SessionEvent::AuctionBidderNotification(
        AuctionBidderNotification {
            house_id: 1,
            auction_id: 7,
            bidder_guid: 0x99,
            bid_or_zero: 900,
            out_bid: 45,
            item_entry: SILK,
            random_property_id: 0,
        },
    )]]);
    assert_eq!(c.eval::<String>(false, "return Read"), "500/1");
    let asked: Vec<Vec<u32>> = c
        .sent
        .try_iter()
        .filter_map(|cmd| match cmd {
            crate::net::ClientCommand::AuctionListBidderItems { auction_ids, .. } => {
                Some(auction_ids)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        asked,
        [Vec::<u32>::new()],
        "the pair is kept after the line"
    );
    assert_eq!(
        c.eval::<String>(
            true,
            "local _,_,_,_,_,_,_,_,_,bid = GetAuctionItemInfo('bidder', 1) return tostring(bid)"
        ),
        "900",
        "and the row patched after it"
    );
}

/// An outbid notice whose item's record is out shows no line in the handler (`0x4ccfcc`), so the
/// pair and the patches run there (`0x4ccff3`, `0x4cd033`); the record's callback (`0x4cd190`)
/// then shows the line alone.
#[test]
fn an_outbid_notice_waiting_on_its_item_patches_in_the_handler() {
    let mut c = client();
    let ours = AuctionListEntry {
        bidder_guid: ME,
        current_bid: 500,
        min_increment: 25,
        ..match page() {
            SessionEvent::AuctionListResult { auctions, .. } => auctions[0],
            _ => unreachable!(),
        }
    };
    c.frame(vec![
        vec![hello()],
        vec![SessionEvent::AuctionBidderListResult {
            auctions: vec![ours],
            total_count: 1,
        }],
    ]);
    c.seen();
    let held = |c: &Client| {
        let a = c.app.world().resource::<AuctionOpen>();
        let e = &a.lists[benilla_ui::script::BIDDER].entries[0];
        (a.outbid.clone(), e.current_bid, e.bidder_guid)
    };
    c.handle(vec![SessionEvent::AuctionBidderNotification(
        AuctionBidderNotification {
            house_id: 1,
            auction_id: 7,
            bidder_guid: 0x99,
            bid_or_zero: 900,
            out_bid: 45,
            item_entry: SILK,
            random_property_id: 0,
        },
    )]);
    assert_eq!(
        held(&c),
        (vec![(1, 7)], 900, 0x99),
        "kept and patched in the handler"
    );
    c.frame(vec![]);
    assert!(
        !c.seen().iter().any(|e| e.starts_with("CHAT_MSG_SYSTEM")),
        "no line while the record is out"
    );
    c.frame(vec![vec![silk()]]);
    assert_eq!(
        c.seen()
            .into_iter()
            .filter(|e| e.starts_with("CHAT_MSG_SYSTEM"))
            .collect::<Vec<_>>(),
        ["CHAT_MSG_SYSTEM:You have been outbid on Silk Cloth."]
    );
    assert_eq!(
        held(&c),
        (vec![(1, 7)], 900, 0x99),
        "the callback shows the line alone"
    );
}

const WOOL: u32 = 2592;

const fn item_guid(low: u64) -> u64 {
    (benilla_protocol::guid::HIGH_ITEM as u64) << 48 | low
}

/// The house open on `AuctionHouse.dbc`'s row 1 (15% deposit), and an item object of ours, `guid`
/// of `WOOL` stacked 3, seated in the backpack's first slot with its record left to the cache and
/// taken into the sell slot, a counter on `NEW_AUCTION_UPDATE` (`Infos`) started from there.
fn sell_wool(guid: u64) -> Client {
    use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_PLAYER_INV_SLOT_HEAD};
    use benilla_protocol::messages::ObjectType;
    let mut c = client();
    c.app.insert_resource(crate::ui_auction::AuctionHouses(
        benilla_formats::AuctionHouseCatalog::from_houses(std::collections::HashMap::from([(
            1,
            benilla_formats::AuctionHouseInfo {
                faction: 12,
                deposit_percent: 15,
                cut_percent: 5,
                name: "Stormwind Auction House".into(),
            },
        )])),
    ));
    c.frame(vec![vec![hello()]]);
    c.frame(vec![vec![SessionEvent::ItemCreate {
        guid,
        container: false,
        fields: benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x3),
            (3, WOOL),
            (6, ME as u32),
            (8, ME as u32),
            (FIELD_ITEM_STACK_COUNT, 3),
        ])
        .into_created(ObjectType::Item),
    }]]);
    let world = c.app.world_mut();
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    let slot = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
    world
        .get_mut::<crate::net::ObjectStore>(me)
        .expect("our store")
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[
            (slot, guid as u32),
            (slot + 1, (guid >> 32) as u32),
        ]));
    c.frame(vec![]);
    c.eval::<()>(
        true,
        r#"Infos = 0
        local f = CreateFrame("Frame")
        f:RegisterEvent("NEW_AUCTION_UPDATE")
        f:SetScript("OnEvent", function() Infos = Infos + 1 end)
        PickupContainerItem(0, 1) ClickAuctionSellItemButton() Infos = 0"#,
    );
    c.sent();
    c
}

fn wool_record() -> SessionEvent {
    let mut record = crate::items::test_template("Wool Cloth");
    (record.sell_price, record.inventory_type, record.quality) = (25, 4, 2);
    SessionEvent::ItemTemplate {
        entry: WOOL,
        info: Some(Box::new(record)),
    }
}

const SELL_INFO: &str = "local n, _, c, q, _, p = GetAuctionSellItemInfo() \
    return tostring(n) .. '/' .. c .. '/' .. q .. '/' .. p";
const DEPOSIT: &str = "return CalculateAuctionDeposit(120)";

/// `CalculateAuctionDeposit` and `GetAuctionSellItemInfo` on a sell slot item whose record the
/// cache lacks answer the miss (`0x4ce2ca`, `0x4ce726`) and keep the callback `0x4ce2f0` with the
/// item's guid (`0x4ce25e`, `0x4ce5e3`), one per call, which signals `NEW_AUCTION_UPDATE` at the
/// record's packet (`0x4ce2fa`), their lookups finding the query the item's create sent in
/// flight (`0x5d8300`); the getters then answer its price and the deposit on it.
#[test]
fn a_sell_item_whose_record_lands_later_signals_the_new_auction_update() {
    let guid = item_guid(1);
    let mut c = sell_wool(guid);
    assert_eq!(c.eval::<String>(true, SELL_INFO), "nil/1/-1/0", "no record");
    assert_eq!(c.eval::<i64>(true, DEPOSIT), 0);
    let queries: Vec<String> = c
        .sent()
        .into_iter()
        .filter(|s| s.starts_with("ItemQuery"))
        .collect();
    assert!(
        queries.is_empty(),
        "the create's query is in flight: {queries:?}"
    );
    c.frame(vec![vec![wool_record()]]);
    assert_eq!(c.eval::<i64>(true, "return Infos"), 2, "one per call");
    assert_eq!(c.eval::<String>(true, SELL_INFO), "Wool Cloth/3/2/75");
    // 15 percent of 75 copper.
    assert_eq!(c.eval::<i64>(true, DEPOSIT), 11);
    assert_eq!(
        c.eval::<i64>(true, "return Infos"),
        2,
        "a found record asks nothing"
    );
}

/// The lookup sits behind the item's object (`0x4ce24e`, `0x4ce5cf`): with it held, a refused
/// record is asked again by each getter; with it gone, neither asks.
#[test]
fn a_sell_item_whose_object_is_gone_asks_no_record() {
    let guid = item_guid(1);
    let mut c = sell_wool(guid);
    let refuse = |c: &mut Client| {
        c.frame(vec![vec![SessionEvent::ItemTemplate {
            entry: WOOL,
            info: None,
        }]]);
        c.sent();
    };
    let asks = |c: &mut Client| {
        c.eval::<()>(
            true,
            "GetAuctionSellItemInfo() CalculateAuctionDeposit(120)",
        );
        c.sent()
            .iter()
            .filter(|s| s.starts_with("ItemQuery"))
            .count()
    };
    refuse(&mut c);
    assert_eq!(asks(&mut c), 1, "a held object's refused record is asked");
    refuse(&mut c);
    c.frame(vec![vec![SessionEvent::ObjectDestroyed(guid)]]);
    refuse(&mut c);
    assert_eq!(asks(&mut c), 0, "a gone object asks nothing");
}

/// A page that waits on silk's record keeps the list's callback (`0x4cc9bf`, dedupe 0) ahead of
/// a later sold notice's (`0x4ccfa2`); the answer runs them in that order (`0x55bf12`-`0x55bf39`),
/// so the page's sort and update (`0x4cca10` → `0x4cd8b0`) fire ahead of the notice's line.
#[test]
fn a_pages_callback_runs_ahead_of_a_later_notices_in_the_answers_list() {
    let mut c = client();
    let sold = SessionEvent::AuctionOwnerNotification(AuctionOwnerNotification {
        auction_id: 9,
        bid: 500,
        out_bid: 0,
        bidder_guid: 0,
        item_entry: SILK,
        random_property_id: 0,
    });
    c.frame(vec![vec![hello()], vec![page()], vec![sold], vec![silk()]]);
    assert_eq!(
        c.seen(),
        [
            "AUCTION_HOUSE_SHOW:nil",
            "AUCTION_ITEM_LIST_UPDATE:nil",
            "CHAT_MSG_SYSTEM:A buyer has been found for your auction of Silk Cloth.",
        ]
    );
}
