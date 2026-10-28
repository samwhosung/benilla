//! The small handler signals and the getters beside them, as a listener reads them: each event
//! fires inside the handler that raises it (`0x537ae5`, `0x703f50`), at its packet's place among a
//! chat line's of the packets around it, and each getter reads the live state at the call.

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;

/// The whole client, headless, in the world.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    #[allow(dead_code)]
    sent: Receiver<ClientCommand>,
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
}

/// The client in the world, and a spy whose line for each event it hears carries its name and
/// first two arguments.
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
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"TIME_PLAYED_MSG", "PLAYER_SKINNED", "CHAT_MSG_MONSTER_SAY"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(arg1) .. ":" .. tostring(arg2))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client { app, inbound, sent };
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[(FIELD_UNIT_HEALTH, 100)],
    )]]);
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

/// What the spy writes for a monster line `text`.
fn said(text: &str) -> String {
    format!("CHAT_MSG_MONSTER_SAY:{text}:Boar")
}

/// `SMSG_PLAYED_TIME` (`0x401850`) signals `TIME_PLAYED_MSG` inside itself (`0x4018df`) and
/// `SMSG_PLAYER_SKINNED` `PLAYER_SKINNED` (`0x5e7d40` → `0x48f412`), each at its packet's place
/// among the lines around it; two answers in one frame are two signals.
#[test]
fn the_played_time_and_the_skin_signal_at_their_packets() {
    let mut c = client();
    c.frame(vec![
        vec![boar_says("a")],
        vec![SessionEvent::PlayedTime {
            total: 3_600,
            level: 60,
        }],
        vec![boar_says("b")],
        vec![SessionEvent::PlayerSkinned { free_repop: true }],
        vec![boar_says("c")],
        vec![SessionEvent::PlayedTime {
            total: 3_601,
            level: 61,
        }],
        vec![SessionEvent::PlayerSkinned { free_repop: false }],
    ]);
    assert_eq!(
        c.seen(),
        [
            said("a"),
            "TIME_PLAYED_MSG:3600:60".into(),
            said("b"),
            "PLAYER_SKINNED:1:nil".into(),
            said("c"),
            "TIME_PLAYED_MSG:3601:61".into(),
            "PLAYER_SKINNED:0:nil".into(),
        ]
    );
    c.frame(vec![]);
    assert!(c.seen().is_empty(), "nothing fires after");
}
