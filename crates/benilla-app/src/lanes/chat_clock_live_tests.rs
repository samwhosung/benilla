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

    /// `packets` run through their handlers with no frame: the world as the handlers leave it.
    fn handle(&mut self, packets: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), packets);
        self.app.world_mut().flush();
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

/// `SMSG_GAMETIME_SET` naming `hour:minute`, the date unset.
fn time_set(hour: i32, minute: i32) -> SessionEvent {
    SessionEvent::GameTimeSet {
        time: benilla_protocol::WowTime {
            minute,
            hour,
            ..benilla_protocol::WowTime::UNSET
        },
    }
}

/// `GetGameTime` reads the clock object at the call (`0x515ee0`): each time message is read through
/// the host with no frame after it, where the VM's copy keeps the seed it was built with.
#[test]
fn the_game_clock_reads_at_the_call() {
    let mut c = client();
    const READ: &str = "return GetGameTime()";
    c.handle(vec![time_set(10, 30)]);
    assert_eq!(c.eval::<(i64, i64)>(true, READ), (10, 30));
    c.handle(vec![time_set(11, 45)]);
    assert_eq!(c.eval::<(i64, i64)>(true, READ), (11, 45));
    assert_eq!(
        c.eval::<(i64, i64)>(false, READ),
        (-1, -1),
        "the copy, seeded before any time"
    );
}

/// A group member's `MSG_MINIMAP_PING` (`0x5e41bd` → `0x4ee330`) stores the point and signals
/// `MINIMAP_PING` with the pinger's token inside its handler, at its packet's place among the lines
/// around it; two pings in one frame are two signals.
#[test]
fn a_members_ping_signals_at_its_packet() {
    let mut c = client();
    c.app
        .world_mut()
        .resource_mut::<crate::player::Player>()
        .active = true;
    c.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"local f = CreateFrame("Frame") f:RegisterEvent("MINIMAP_PING")
               f:SetScript("OnEvent", function() table.insert(Seen, event .. ":" .. arg1) end)"#,
        )
        .unwrap();
    c.frame(vec![]);
    let ping = |x: f32| SessionEvent::MinimapPing {
        guid: 0xDEAD,
        x,
        y: 0.0,
    };
    c.frame(vec![
        vec![boar_says("a")],
        vec![ping(10.0)],
        vec![boar_says("b")],
        vec![ping(20.0)],
    ]);
    assert_eq!(
        c.seen(),
        [
            said("a"),
            "MINIMAP_PING:player".into(),
            said("b"),
            "MINIMAP_PING:player".into(),
        ]
    );
    assert_eq!(
        c.app
            .world()
            .resource::<crate::minimap::MinimapPing>()
            .point(),
        (20.0, 0.0)
    );
}

/// Spy on `events` too, each heard as its name and first argument.
fn spy_on(c: &mut Client, events: &[&str]) {
    let list: Vec<String> = events.iter().map(|e| format!("{e:?}")).collect();
    c.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(&format!(
            r#"local f = CreateFrame("Frame")
               for _, e in ipairs({{{}}}) do f:RegisterEvent(e) end
               f:SetScript("OnEvent", function() table.insert(Seen, event .. ":" .. arg1) end)"#,
            list.join(", ")
        ))
        .unwrap();
}

/// The world broadcasts print inside their handlers (`0x49de30`, `0x5e4a09`, `0x4019a0`), each
/// line at its packet's place among the lines around it: a defence message once per defence
/// channel joined (`0x49dd94`), the chat restriction and a transfer abort as system lines.
#[test]
fn the_world_broadcasts_print_at_their_packets() {
    let mut c = client();
    spy_on(&mut c, &["CHAT_MSG_SYSTEM", "CHAT_MSG_CHANNEL"]);
    c.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"ERR_CHAT_RESTRICTED = "restricted" TRANSFER_ABORT_MAX_PLAYERS = "full""#)
        .unwrap();
    let world_defense =
        benilla_formats::ChatChannelsCatalog::from_rows(vec![benilla_formats::ChatChannelRow {
            id: 23,
            flags: 0x1_0004,
            pattern: "WorldDefense".into(),
            shortcut: "WorldDefense".into(),
        }]);
    c.app.insert_resource(crate::ui_chat::ChannelState {
        joined: vec![crate::ui_chat::edit::ChannelSlot::confirmed("WorldDefense", 23).into()],
        channels: world_defense,
        ..Default::default()
    });
    c.frame(vec![
        vec![boar_says("a")],
        vec![SessionEvent::DefenseMessage {
            zone_id: 40,
            text: "under attack".into(),
        }],
        vec![boar_says("b")],
        vec![SessionEvent::ChatRestricted],
        vec![boar_says("c")],
        vec![SessionEvent::TransferAborted { reason: 1 }],
        vec![boar_says("d")],
    ]);
    assert_eq!(
        c.seen(),
        [
            said("a"),
            "CHAT_MSG_CHANNEL:under attack".into(),
            said("b"),
            "CHAT_MSG_SYSTEM:restricted".into(),
            said("c"),
            "CHAT_MSG_SYSTEM:full".into(),
            said("d"),
        ]
    );
}

/// The channel lookups read the slot array at the call (`0x49be50`, `0x49bf30`): a VM built with
/// no copy of the slots answers them through the host at once, with no frame to seed it.
#[test]
fn the_channel_lookups_read_the_slots_at_the_call() {
    let mut c = client();
    c.app.insert_resource(crate::ui_chat::ChannelState {
        joined: vec![
            crate::ui_chat::edit::ChannelSlot::confirmed("General - Elwynn Forest", 1).into(),
            crate::ui_chat::edit::ChannelSlot::confirmed("MyChan", 0).into(),
        ],
        ..Default::default()
    });
    c.app
        .insert_non_send_resource(UiScript::new().expect("a VM"));
    const READ: &str = "local _, a = GetChannelName(2) return a, (GetChannelName('MyChan')), \
        table.getn({GetChannelList()})";
    assert_eq!(
        c.eval::<(Option<String>, i64, i64)>(true, READ),
        (Some("MyChan".into()), 2, 4)
    );
    assert_eq!(
        c.eval::<(Option<String>, i64, i64)>(false, READ),
        (None, 0, 0),
        "the copy holds none"
    );
}
