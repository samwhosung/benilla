//! The auction getters as a listener reads them, and the house's events at their packets. The
//! getters read the records, the item cache and the throttle at the call (`0x4cee40`, `0x4cef96`,
//! `0x4cfbb0`); the events fire inside the handler that raises them: the hello's
//! `AUCTION_HOUSE_SHOW` (`0x4cd5e6`), a notice's line once its item's record lands (`0x4cd190`),
//! a page's update (`0x4cca10`) and a close's `AUCTION_HOUSE_CLOSED` (`0x4cd602`).

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::messages::{AuctionListEntry, AuctionOwnerNotification};
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
    let (tx, _sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            ERR_AUCTION_SOLD_S = "A buyer has been found for your auction of %s."
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
    let mut c = Client { app, inbound };
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
/// then fills it for a host-lent getter at the call, with no push between, while the VM's copy
/// still holds none until the page's tail runs. The throttle reads the house at the call too.
#[test]
fn the_auction_getters_read_the_house_at_the_call() {
    let mut c = client();
    c.frame(vec![vec![hello()], vec![page()]]);
    assert_eq!(c.eval::<String>(true, NAME), "nil", "the record is out");
    c.handle(vec![silk()]);
    assert_eq!(c.eval::<String>(true, NAME), "Silk Cloth");
    assert_eq!(c.eval::<String>(false, NAME), "nil", "the VM's copy waits");
    c.frame(vec![]);
    assert_eq!(
        c.eval::<String>(false, NAME),
        "Silk Cloth",
        "the tail's push"
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
