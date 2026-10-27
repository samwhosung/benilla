//! The trade getters as a listener reads them, and the trade's events at their packets. The
//! getters read the window's records, the partner cell and the money offers at the call
//! (`0x4c00f0`, `0x4c0470`, `0x4c0700`, `0x4c0750`); the events fire inside the handler that
//! raises them: the open's `TRADE_SHOW` (`0x4bf644`), the accept flags' `TRADE_ACCEPT_UPDATE`
//! (`0x4bf29f`) and the extended status's compare (`0x4bf2b0`).

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::messages::{TradeItem, TradeStatus, TradeStatusExtended, TRADE_SLOT_COUNT};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Sender;

use super::unit_view_tests::{created, wire_packets};
use crate::ui_trade::{TradeEvent, TradeLane, TradeSession};

const ME: u64 = 0x7;
const BOB: u64 = 0x9;
const SWORD: u32 = 1234;

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

/// The client in the world with Bob, a player, beside us, the sword's record cached, and a spy
/// whose line for each trade event and monster line carries its name and first argument.
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
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"TRADE_SHOW", "TRADE_ACCEPT_UPDATE", "TRADE_TARGET_ITEM_CHANGED",
                    "TRADE_MONEY_CHANGED", "CHAT_MSG_MONSTER_SAY"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(arg1))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    app.world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(SWORD, Some(crate::items::test_template("Iron Sword")));
    let mut c = Client { app, inbound };
    c.frame(vec![vec![
        created(ME, EntityKind::Player, &[(FIELD_UNIT_HEALTH, 100)]),
        created(BOB, EntityKind::Player, &[(FIELD_UNIT_HEALTH, 100)]),
    ]]);
    c.frame(vec![]);
    c.seen();
    c
}

fn status(status: TradeStatus) -> SessionEvent {
    SessionEvent::TradeStatus { status }
}

/// Bob's column: the sword in slot 1 when `sword`, and `gold`.
fn bobs(sword: bool, gold: u32) -> SessionEvent {
    let mut slots = [None; TRADE_SLOT_COUNT];
    slots[0] = sword.then_some(TradeItem {
        entry: SWORD,
        display_id: 0,
        count: 1,
        wrapped: false,
        gift_creator: 0,
        perm_enchant: 0,
        creator: 0,
        charges: 0,
        suffix_factor: 0,
        random_prop_id: 0,
        lock_id: 0,
        max_durability: 0,
        durability: 0,
    });
    SessionEvent::TradeStatusExtended {
        state: Box::new(TradeStatusExtended {
            their_window: true,
            sequence: 7,
            flag: 7,
            gold,
            enchant_spell_id: 0,
            slots,
        }),
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

const READ: &str = "return tostring((GetTradeTargetItemInfo(1))) .. '/' .. \
    GetTargetTradeMoney() .. '/' .. GetPlayerTradeMoney()";

/// Bob's request passes the ladder inside its status and his window opens: the getters read the
/// session at the call, a later packet's offer included, with no push between; with no host the
/// VM's copy, which nothing pushes, holds none.
#[test]
fn the_trade_getters_read_the_session_at_the_call() {
    let mut c = client();
    c.handle(vec![status(TradeStatus::BeginTrade { partner: BOB })]);
    assert!(
        !c.app.world().resource::<TradeSession>().is_open(),
        "accepted, the window waits for the server's open"
    );
    c.handle(vec![status(TradeStatus::OpenWindow), bobs(true, 500)]);
    assert_eq!(c.eval::<String>(true, READ), "Iron Sword/500/0");
    assert_eq!(
        c.eval::<String>(false, READ),
        "nil/0/0",
        "the VM's copy holds nothing"
    );
    c.handle(vec![bobs(false, 300)]);
    assert_eq!(c.eval::<String>(true, READ), "nil/300/0", "the next offer");
    c.app
        .world_mut()
        .resource_mut::<TradeSession>()
        .set_own_gold(40);
    assert_eq!(
        c.eval::<String>(true, READ),
        "nil/300/40",
        "our offer behind the open cell"
    );
}

/// The open's `TRADE_SHOW`, the accept's `TRADE_ACCEPT_UPDATE` and the offer's compare sit on the
/// trade lane at their handlers, between the stamps of the handlers before and after.
#[test]
fn the_trade_events_are_stamped_at_their_handlers() {
    let mut c = client();
    c.handle(vec![status(TradeStatus::BeginTrade { partner: BOB })]);
    let before = crate::lanes::stamp();
    c.handle(vec![
        status(TradeStatus::OpenWindow),
        status(TradeStatus::Accept),
        bobs(true, 500),
    ]);
    let after = crate::lanes::stamp();
    let stamped = c.app.world().resource::<TradeLane>().stamped();
    assert!(
        stamped.iter().all(|(at, _)| before < *at && *at < after),
        "stamped inside the handlers"
    );
    assert_eq!(
        stamped.into_iter().map(|(_, e)| e).collect::<Vec<_>>(),
        [
            TradeEvent::Show {
                partner: BOB,
                purse: None
            },
            TradeEvent::Accept(false, true),
            TradeEvent::Accept(false, false),
            TradeEvent::TargetItem(0),
            TradeEvent::TargetMoney,
        ]
    );
    assert!(c.seen().is_empty(), "nothing fired in the handlers");
}

/// Each trade event fires at its packet's place among a chat line's of the packets around it.
#[test]
fn the_trade_events_fire_at_their_packets() {
    let mut c = client();
    c.handle(vec![status(TradeStatus::BeginTrade { partner: BOB })]);
    c.frame(vec![
        vec![boar_says("a")],
        vec![status(TradeStatus::OpenWindow)],
        vec![boar_says("b")],
        vec![bobs(true, 500)],
        vec![boar_says("c")],
        vec![status(TradeStatus::Accept)],
        vec![boar_says("d")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "TRADE_SHOW:nil",
            "CHAT_MSG_MONSTER_SAY:b",
            "TRADE_TARGET_ITEM_CHANGED:1",
            "TRADE_MONEY_CHANGED:nil",
            "CHAT_MSG_MONSTER_SAY:c",
            "TRADE_ACCEPT_UPDATE:0",
            "CHAT_MSG_MONSTER_SAY:d",
        ]
    );
}
