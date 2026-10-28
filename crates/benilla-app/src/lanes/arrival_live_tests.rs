//! The arrival lane ([`crate::ui_world_enter`]) through the whole client: the arrival's signals fire
//! at its create's stamp, between the packets around it (`0x5dec7b`, inside the update packet), and
//! its listeners read the world as each step leaves it.

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{guild_event, ChatMessage, GuildEventNotice, CHAT_MSG_SYSTEM};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// Three stance-bar spells, by `StanceBarOrder`: the warded form, then Bear, then Cat.
const BEAR: u32 = 90_021;
const CAT: u32 = 90_022;
const WARDED: u32 = 90_023;
/// A spell on no bar.
const FIREBALL: u32 = 133;

/// The whole client, headless, logged in before our create.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    _sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// What the spy heard.
    fn seen(&mut self) -> Vec<String> {
        crate::script_calls::eval_in_vm(self.app.world(), "local s = Seen Seen = {} return s")
    }
}

fn spells() -> crate::ui_action::Spells {
    let form = |id: u32, form: u32, order: i32| {
        (
            id,
            SpellDisplay {
                id,
                name: format!("Form {id}"),
                shapeshift_form: Some(form),
                stance_bar_order: order,
                ..Default::default()
            },
        )
    };
    let fireball = SpellDisplay {
        id: FIREBALL,
        name: "Fireball".into(),
        ..Default::default()
    };
    crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(
            [
                form(BEAR, 5, 1),
                form(CAT, 1, 2),
                form(WARDED, 6, 0),
                (FIREBALL, fireball),
            ]
            .into_iter()
            .collect::<HashMap<_, _>>(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// The client logged in as [`ME`], our create not yet arrived, the chat display latched as from
/// start-up (`0x8435fc`), and `spy` run in its VM.
fn client(spy: &str) -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    app.insert_resource(spells());
    let script = UiScript::new().expect("a VM");
    script.run(spy).expect("the spy");
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    app.world().resource::<crate::ui_chat::ChatLog>().latch();
    Client {
        app,
        inbound,
        _sent: sent,
    }
}

fn me() -> SessionEvent {
    created(
        ME,
        EntityKind::Player,
        &[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)],
    )
}

/// The arrival learns the initial spells one at a time in their wire order (`0x5dea96`-
/// `0x5deab9`), each form's add signalling `UPDATE_SHAPESHIFT_FORMS` in place (`0x4b2904`), so the
/// k-th listener counts k forms, in bar order as that learn left the list (`0x4b28f7`); a spell on
/// no bar signals nothing.
#[test]
fn the_kth_form_signal_reads_k_forms() {
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("UPDATE_SHAPESHIFT_FORMS")
        f:SetScript("OnEvent", function()
            local names = {}
            for i = 1, GetNumShapeshiftForms() do
                local _, name = GetShapeshiftFormInfo(i)
                names[i] = name
            end
            table.insert(Seen, GetNumShapeshiftForms() .. ":" .. table.concat(names, ","))
        end)"#,
    );
    c.frame(vec![
        vec![SessionEvent::SpellBook {
            spell_ids: vec![CAT, FIREBALL, BEAR, WARDED],
            cooldowns: Vec::new(),
        }],
        vec![me()],
    ]);
    assert_eq!(
        c.seen(),
        [
            "1:Form 90022",
            "2:Form 90021,Form 90022",
            "3:Form 90023,Form 90021,Form 90022",
        ]
    );
}

/// The arrival's signals fire at its create's stamp: an event a packet ahead of the create raises
/// at once (`GUILD_MOTD`, `0x5e728d`) before `PLAYER_ENTERING_WORLD`, and a line from a packet
/// after it behind. A system line ahead of the create is queued behind the display latch
/// (`0x49db5c` → `0x49cae0`) and shown by the cascade's chat release (`0x490974`), after both
/// login events, so it follows `PLAYER_ENTERING_WORLD` too, ahead of the line after the create.
#[test]
fn the_arrival_fires_between_the_packets_around_its_create() {
    let mut c = client(
        r#"Seen = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_ENTERING_WORLD", "GUILD_MOTD", "CHAT_MSG_SYSTEM"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function()
            table.insert(Seen, event .. ":" .. tostring(arg1))
        end)"#,
    );
    let system = |text: &str| {
        SessionEvent::Chat(ChatMessage {
            chat_type: CHAT_MSG_SYSTEM,
            language: 0,
            sender_guid: 0,
            target_guid: 0,
            sender_name: None,
            channel: None,
            text: text.into(),
            chat_tag: 0,
        })
    };
    c.frame(vec![
        vec![SessionEvent::GuildEvent(GuildEventNotice {
            event: guild_event::MOTD,
            params: vec!["motd".into()],
            guid: None,
        })],
        vec![system("before")],
        vec![me()],
        vec![system("after")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "GUILD_MOTD:motd",
            "PLAYER_ENTERING_WORLD:nil",
            "CHAT_MSG_SYSTEM:before",
            "CHAT_MSG_SYSTEM:after",
        ]
    );
}
