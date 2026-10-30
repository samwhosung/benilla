//! The bank getters as a listener reads them, and the bank's events at their packets. The getters
//! read the active player's bought bag slots and the next price at the call (`0x4f85b0`,
//! `0x4f8630`), no banker tested; `SMSG_SHOW_BANK`'s open signals `BANKFRAME_OPENED` inside it
//! (`0x4f8522`), a refusal shows its line inside `SMSG_BUY_BANK_SLOT_RESULT`'s handler
//! (`0x5e3f8d`), and the bought count's watch signals `PLAYERBANKBAGSLOTS_CHANGED` at its packet
//! (`0x4f8490`).

use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Sender;

use super::unit_view_tests::{created, values, wire_packets};

const ME: u64 = 0x7;
const BANKER: u64 = 0xF130_0000_0000_0043;
/// `PLAYER_BYTES_2`, absolute: byte 2 is the bought bank bag slots.
const PLAYER_BYTES_2: u16 = 194;

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
    }
}

/// The client in the world with a banker beside us, two bag slots bought, the price table's first
/// rows, and a spy whose line for each bank event and message line carries its name and first
/// argument.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, _sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.insert_resource(crate::ui_bank::BankPrices(
        benilla_formats::BankBagSlotPrices::from_rows([(1, 1_000), (2, 10_000), (3, 100_000)]),
    ));
    app.update();
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            ERR_BANKSLOT_INSUFFICIENT_FUNDS = "too poor"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"BANKFRAME_OPENED", "BANKFRAME_CLOSED",
                    "PLAYERBANKBAGSLOTS_CHANGED", "UI_ERROR_MESSAGE", "CHAT_MSG_MONSTER_SAY"}) do
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
    let mut c = Client { app, inbound };
    c.frame(vec![vec![
        created(ME, EntityKind::Player, &[(PLAYER_BYTES_2, 2 << 16)]),
        created(BANKER, EntityKind::Unit, &[]),
    ]]);
    c.frame(vec![]);
    c.seen();
    c
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

const SLOTS: &str = "local n, full = GetNumBankSlots() return n, full == 1, GetBankSlotCost()";

/// The purchase row reads the player at the call, no banker open, with no push between; with no
/// host the VM's copy, which nothing pushes, holds none.
#[test]
fn the_bank_getters_read_the_player_at_the_call() {
    let mut c = client();
    assert_eq!(c.eval::<(i64, bool, i64)>(true, SLOTS), (2, false, 100_000));
    assert_eq!(c.eval::<(i64, bool, i64)>(false, SLOTS), (0, false, 0));
    c.handle(vec![
        values(ME, &[(PLAYER_BYTES_2, 6 << 16)]),
        SessionEvent::ObjectUpdateEnd,
    ]);
    assert_eq!(
        c.eval::<(i64, bool, i64)>(true, SLOTS),
        (6, true, 0),
        "six bought: full, and no seventh row"
    );
}

/// The open's `BANKFRAME_OPENED` fires inside its packet's dispatch, with no frame after it: the
/// handler's signal runs every listener before the next packet's handler (`0x537ae5`, `0x703e50`).
#[test]
fn the_open_fires_inside_its_dispatch() {
    let mut c = client();
    c.handle(vec![SessionEvent::ShowBank { banker: BANKER }]);
    let lane = c.app.world().resource::<crate::ui_bank::BankLane>();
    assert!(lane.stamped().is_empty(), "nothing left on the lane");
    assert!(
        c.seen().iter().any(|s| s == "BANKFRAME_OPENED:nil"),
        "the open fired inside the dispatch"
    );
}

/// The open, a refusal's line and a bought slot's watch each fire at their packet's place around
/// a chat line of a packet between them; the watch fires with no banker open too.
#[test]
fn the_bank_events_fire_at_their_packets() {
    let mut c = client();
    c.frame(vec![
        vec![boar_says("a")],
        vec![SessionEvent::ShowBank { banker: BANKER }],
        vec![boar_says("b")],
        vec![SessionEvent::BuyBankSlotResult {
            result: benilla_protocol::messages::bank_slot_result::INSUFFICIENT_FUNDS,
        }],
        vec![boar_says("c")],
        vec![values(ME, &[(PLAYER_BYTES_2, 3 << 16)])],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "BANKFRAME_OPENED:nil",
            "CHAT_MSG_MONSTER_SAY:b",
            "UI_ERROR_MESSAGE:too poor",
            "CHAT_MSG_MONSTER_SAY:c",
            "PLAYERBANKBAGSLOTS_CHANGED:nil",
        ]
    );
    crate::ui_bank::close_core(c.app.world_mut());
    c.seen();
    c.frame(vec![vec![values(ME, &[(PLAYER_BYTES_2, 4 << 16)])]]);
    assert_eq!(c.seen(), ["PLAYERBANKBAGSLOTS_CHANGED:nil"]);
}

/// A close in the open's frame fires after the open, which a close core run ahead of the drain
/// would otherwise beat. The close signals first and then releases the latch, which re-enters it
/// with the cell still set (`0x4f84df`, `0x4f84eb` → `0x493310` → `0x4f84d0`), so
/// `BANKFRAME_CLOSED` signals twice.
#[test]
fn a_close_fires_after_the_open() {
    let mut c = client();
    c.handle(vec![SessionEvent::ShowBank { banker: BANKER }]);
    crate::ui_bank::close_core(c.app.world_mut());
    c.frame(vec![]);
    assert_eq!(
        c.seen(),
        [
            "BANKFRAME_OPENED:nil",
            "BANKFRAME_CLOSED:nil",
            "BANKFRAME_CLOSED:nil"
        ]
    );
    assert!(!c
        .app
        .world()
        .resource::<crate::ui_bank::BankOpen>()
        .is_open());
}

/// Two bought slots in one frame, each its own packet: the watch's listener reads the count its
/// own packet left (`0x4f8490` signals inside the packet, `0x4f85dd` reads the field at the call),
/// not the frame's last.
#[test]
fn the_bank_slots_read_as_of_each_packet() {
    let mut c = client();
    c.eval::<()>(
        false,
        "Slots = {} local f = CreateFrame('Frame') \
         f:RegisterEvent('PLAYERBANKBAGSLOTS_CHANGED') \
         f:SetScript('OnEvent', function() table.insert(Slots, (GetNumBankSlots())) end)",
    );
    c.frame(vec![
        vec![values(ME, &[(PLAYER_BYTES_2, 3 << 16)])],
        vec![values(ME, &[(PLAYER_BYTES_2, 4 << 16)])],
    ]);
    assert_eq!(c.eval::<Vec<i64>>(false, "return Slots"), [3, 4]);
}
