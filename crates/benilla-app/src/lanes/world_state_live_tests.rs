//! The world-state readout's rows are read at the call (`0x4c5a70`), and the zone-defense flag that
//! admits its `Type` 1 rows moves inside the channel notice that confirms or frees the defense
//! slot (`0x49bbbf`, `0x49bc41` → `0x49bd50`), which signals `UPDATE_WORLD_STATES` there
//! (`0x49bdd6`): the confirm's ahead of its `CHAT_MSG_CHANNEL_NOTICE` (`0x49c0c2` before
//! `0x49c5b0`), the free's after it (`0x49c5c2`). A listener reads the flag its own notice left,
//! not the frame's last.

use benilla_protocol::messages::{channel_notice, ChannelNoticeTail};
use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Sender;

use super::unit_view_tests::wire_packets;

/// Eastern Plaguelands, the scope of the init.
const EPL: u32 = 139;
const DEFENSE: &str = "LocalDefense - Eastern Plaguelands";

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
}

impl Client {
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    fn heard(&mut self) -> Vec<String> {
        crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
            let s = s.expect("a VM");
            let heard = s.eval::<Vec<String>>("return Heard").expect("the spy");
            s.run("Heard = {}").expect("the reset");
            heard
        })
    }
}

/// The client with two readout rows for Eastern Plaguelands, one always up and one up only with
/// the defense channel joined, our join of that channel sent (its slot pending), and a VM whose
/// listeners note the rows they read on `UPDATE_WORLD_STATES` and on each channel notice.
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
            Heard = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UPDATE_WORLD_STATES")
            f:RegisterEvent("CHAT_MSG_CHANNEL_NOTICE")
            f:SetScript("OnEvent", function()
                local tag = event == "UPDATE_WORLD_STATES" and "readout" or arg1
                table.insert(Heard, tag .. ":" .. GetNumWorldStateUI())
            end)
            "#,
        )
        .expect("the spy");
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    let row = |ui_type| benilla_formats::WorldStateUiRow {
        map_id: 0,
        area_id: EPL,
        icon: String::new(),
        text: String::new(),
        tooltip: String::new(),
        state_variable: 0,
        ui_type,
        dynamic_icon: String::new(),
        dynamic_tooltip: String::new(),
        extended_ui: String::new(),
        extended_ui_state: [0; 3],
    };
    app.insert_resource(crate::world_state_ui::WorldStateUiRes(
        benilla_formats::WorldStateUiCatalog::from_rows(vec![(1, row(0)), (2, row(1))]),
    ));
    let mut channels = crate::ui_chat::ChannelState {
        channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
            benilla_formats::ChatChannelRow {
                id: 22,
                flags: 0x1_0003,
                pattern: "LocalDefense - %s".into(),
                shortcut: "LocalDefense".into(),
            },
        ]),
        zone_mask: Some(0),
        ..Default::default()
    };
    channels.claim_slot(DEFENSE, 22);
    app.insert_resource(channels);
    let mut c = Client { app, inbound };
    c.frame(vec![vec![SessionEvent::WorldStates {
        scope: Some((0, EPL)),
        states: Vec::new(),
    }]]);
    c.frame(vec![]);
    c
}

fn notice(byte: u8, tail: ChannelNoticeTail) -> Vec<SessionEvent> {
    vec![SessionEvent::ChannelNotify {
        notice: byte,
        channel: DEFENSE.into(),
        tail,
    }]
}

/// The join's confirm sets the flag and signals the readout ahead of its own notice, both reading
/// the defense row; the leave's notice in the same frame still reads it, and its free clears the
/// flag and signals after it, reading the row gone.
#[test]
fn the_defense_flag_moves_and_signals_inside_its_notice() {
    let mut c = client();
    assert_eq!(c.heard(), ["readout:1"], "control: the init's signal");
    c.frame(vec![
        notice(
            channel_notice::YOU_JOINED,
            ChannelNoticeTail::YouJoined {
                flags: 0,
                instance: 0,
            },
        ),
        notice(channel_notice::YOU_LEFT, ChannelNoticeTail::Empty),
    ]);
    assert_eq!(
        c.heard(),
        ["readout:2", "YOU_JOINED:2", "YOU_LEFT:2", "readout:1"]
    );
}
