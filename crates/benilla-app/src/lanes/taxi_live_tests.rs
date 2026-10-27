//! The taxi getters as a listener reads them, and the flight master's events at their packets.
//! The getters read the slots the open's builder made and take each fare at the call off the
//! flight master's cell (`0xbb4aa8`, `0x4dc3d0`, `0x4dc423`); the events fire inside the handler
//! that raises them: the builder's `TAXIMAP_OPENED` (`0x4dba96`), the activate reply's refusal
//! line (`0x5ed1e0`) and the first-visit learn's (`ERR_NEWTAXIPATH`).

use benilla_protocol::field::{FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS};
use benilla_protocol::messages::TaxiMask;
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
const MASTER: u64 = 0xF130_0000_0000_0099;
/// `UNIT_FIELD_BYTES_0` and `PLAYER_BYTES_3`, absolute.
const BYTES_0: u16 = 36;
const PLAYER_BYTES_3: u16 = 195;
/// At Stormwind (2), knowing Sentinel Hill (4), Ironforge (6) and Thelsamar (8).
const KNOWN: TaxiMask = TaxiMask([0b1010_1010, 0, 0, 0, 0, 0, 0, 0]);

/// The whole client, headless, in the world.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    _sent: Receiver<ClientCommand>,
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

    /// The fare the getters read of the node named `name`.
    fn fare(&mut self, name: &str) -> i64 {
        self.eval(
            true,
            &format!(
                "for i = 1, NumTaxiNodes() do \
                   if TaxiNodeName(i) == {name:?} then return TaxiNodeCost(i) end \
                 end"
            ),
        )
    }
}

/// The client in the world as an honored human warrior of honor rank byte 6 beside a PvP-flagged
/// Stormwind flight master, which makes the fares' discount 0.15, over the shipped taxi tables;
/// and a spy whose line for each map event and message line carries its name and first argument.
fn client(data: &std::path::Path) -> Client {
    let mut chain = benilla_formats::open_chain(data).expect("open chain");
    let catalogs = crate::ui_taxi::TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
    let (factions, template, reps) = crate::target::stormwind_fixture(&mut chain, 9000);
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    app.insert_resource(catalogs)
        .insert_resource(factions)
        .insert_resource(reps);
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            ERR_TAXINOTENOUGHMONEY = "no money"
            ERR_NEWTAXIPATH = "new path"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"TAXIMAP_OPENED", "TAXIMAP_CLOSED", "UI_ERROR_MESSAGE",
                    "UI_INFO_MESSAGE", "CHAT_MSG_MONSTER_SAY"}) do
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
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    c.frame(vec![vec![
        created(
            ME,
            EntityKind::Player,
            &[
                (FIELD_UNIT_FACTIONTEMPLATE, 1),
                (FIELD_UNIT_FLAGS, 0x8),
                (BYTES_0, crate::target::HUMAN_WARRIOR),
                (PLAYER_BYTES_3, 6 << 24),
            ],
        ),
        created(
            MASTER,
            EntityKind::Unit,
            &[
                (FIELD_UNIT_FACTIONTEMPLATE, template),
                (FIELD_UNIT_FLAGS, 0x1000),
            ],
        ),
    ]]);
    c.frame(vec![]);
    c.seen();
    c
}

fn shown() -> SessionEvent {
    SessionEvent::TaxiNodesShown {
        flightmaster: MASTER,
        nearest_node: 2,
        known_mask: KNOWN,
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

/// The slots read as the open's handler built them, with no push between, and each fare taken
/// at the call: the master's discount while its cell holds it, 0 once the map closes, the slots
/// kept (`0x4dc0f0`); with no host the VM's copy, which nothing pushes, answers as it was.
#[test]
fn the_taxi_getters_read_the_world_at_the_call() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut c = client(&data);
    c.handle(vec![shown()]);
    assert_eq!(c.eval::<i64>(true, "return NumTaxiNodes()"), 4);
    assert_eq!(c.eval::<i64>(false, "return NumTaxiNodes()"), 0);
    assert_eq!(c.fare("Sentinel Hill, Westfall"), 94, "110 less 0.15");
    crate::ui_taxi::close_core(c.app.world_mut());
    assert_eq!(c.eval::<i64>(true, "return NumTaxiNodes()"), 4, "kept");
    assert_eq!(c.fare("Sentinel Hill, Westfall"), 0, "no flight master");
}

/// The open's `TAXIMAP_OPENED`, a refused activate's line and a first-visit learn's line fire at
/// their packets' places, around the chat lines of the packets between them.
#[test]
fn the_taxi_events_fire_at_their_packets() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut c = client(&data);
    c.frame(vec![
        vec![shown()],
        vec![boar_says("a")],
        vec![SessionEvent::ActivateTaxiReply {
            code: benilla_protocol::messages::taxi_reply::NOT_ENOUGH_MONEY,
        }],
        vec![boar_says("b")],
        vec![SessionEvent::NewTaxiPath],
    ]);
    assert_eq!(
        c.seen(),
        [
            "TAXIMAP_OPENED:nil",
            "CHAT_MSG_MONSTER_SAY:a",
            "UI_INFO_MESSAGE:no money",
            "CHAT_MSG_MONSTER_SAY:b",
            "UI_INFO_MESSAGE:new path",
        ]
    );
}

/// The open's `TAXIMAP_OPENED` sits on the taxi lane at its handler's stamp; a map that builds no
/// slot signals nothing and leaves no cell (`0x4dba40`, `0x4dbab0`).
#[test]
fn the_open_is_stamped_at_its_handler() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut c = client(&data);
    let before = crate::lanes::stamp();
    c.handle(vec![shown()]);
    let after = crate::lanes::stamp();
    let stamped = c
        .app
        .world()
        .resource::<crate::ui_taxi::TaxiLane>()
        .stamped();
    assert_eq!(stamped.len(), 1);
    let (at, event) = stamped[0];
    assert_eq!(event, crate::ui_taxi::TaxiEvent::Opened);
    assert!(before < at && at < after, "stamped inside the handler");
    c.frame(vec![]);
    crate::ui_taxi::close_core(c.app.world_mut());
    c.seen();
    // Two known nodes, neither on the current node's continent row: no slot.
    c.handle(vec![SessionEvent::TaxiNodesShown {
        flightmaster: MASTER,
        nearest_node: 9999,
        known_mask: KNOWN,
    }]);
    c.frame(vec![]);
    assert!(c.seen().is_empty());
    assert!(c
        .app
        .world()
        .resource::<crate::ui_taxi::TaxiState>()
        .open
        .is_none());
}

/// A map and its close in one frame signal in packet order: the activate's `OK` (`0x5ed203`) or a
/// map from another flight master (`0x4db73a`) runs the close core (`0x4dc0f0`) after the
/// `TAXIMAP_OPENED` of the map it closes, so a listener that shows the frame on the open sees it
/// hidden by the close, and no map is left shown with no flight master.
#[test]
fn a_close_after_the_open_fires_after_it() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut c = client(&data);
    c.eval::<()>(
        false,
        r#"
        Shown = false
        local g = CreateFrame("Frame")
        g:RegisterEvent("TAXIMAP_OPENED")
        g:RegisterEvent("TAXIMAP_CLOSED")
        g:SetScript("OnEvent", function() Shown = (event == "TAXIMAP_OPENED") end)
        "#,
    );
    c.frame(vec![
        vec![shown()],
        vec![SessionEvent::ActivateTaxiReply {
            code: benilla_protocol::messages::taxi_reply::OK,
        }],
    ]);
    // The map holds the latch, so the release re-enters the core: two closes (`0x4dc105`).
    assert_eq!(
        c.seen(),
        [
            "TAXIMAP_OPENED:nil",
            "TAXIMAP_CLOSED:nil",
            "TAXIMAP_CLOSED:nil"
        ]
    );
    assert!(!c.eval::<bool>(false, "return Shown"), "the map is hidden");
    let state = c.app.world().resource::<crate::ui_taxi::TaxiState>();
    assert!(state.open.is_none(), "no cell");

    // Another flight master's map closes the first through the core (`0x4db73a`) and opens its
    // own, in order; the drain runs with no frame, which would have the leash watch close a
    // master the world does not hold.
    c.handle(vec![
        shown(),
        SessionEvent::TaxiNodesShown {
            flightmaster: MASTER + 1,
            nearest_node: 2,
            known_mask: KNOWN,
        },
    ]);
    c.app.world_mut().flush();
    crate::lanes::fire_in_order(c.app.world_mut());
    assert_eq!(
        c.seen(),
        [
            "TAXIMAP_OPENED:nil",
            "TAXIMAP_CLOSED:nil",
            "TAXIMAP_CLOSED:nil",
            "TAXIMAP_OPENED:nil"
        ]
    );
    assert!(
        c.eval::<bool>(false, "return Shown"),
        "the second map is up"
    );
}
