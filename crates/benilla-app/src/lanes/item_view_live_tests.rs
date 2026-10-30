//! The item record as a getter reads it at the call, through the whole client: the tooltip build
//! expands each spell line's description at the build (`0x5075f0` at `0x52da24`), against the
//! active player as it stands, so a listener's build reads the player its own packet left; and
//! `GetBindLocation` reads the bind point at the call (`0x48dae3` → `0x5ed420`).

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::messages::{ChatMessage, ItemSpellEntry, CHAT_MSG_SYSTEM};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Sender;

use super::unit_view_tests::{created, values, wire_packets};

const ME: u64 = 0x7;
/// The charm's record and the spell its use names, whose description branches on the caster's
/// gender (`$g`, `0x508214`).
const CHARM: u32 = 6_001;
const SPELL: u32 = 9_001;
/// `UNIT_FIELD_BYTES_0` of a human mage, and its gender byte's bit for female.
const BYTES_0: u16 = 36;
const HUMAN_MAGE: u32 = 1 | (8 << 8);
const FEMALE: u32 = 1 << 16;
/// The bind zone, named through `AreaTable`.
const GOLDSHIRE: u32 = 87;

/// The spy: at each system line, the charm's use line as `SetHyperlink` builds it.
const SPY: &str = r#"
    ITEM_SPELL_TRIGGER_ONUSE = "Use:"
    Seen = {}
    local a = CreateFrame("Button", "B"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
    CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
    local f = CreateFrame("Frame")
    f:RegisterEvent("CHAT_MSG_SYSTEM")
    f:SetScript("OnEvent", function()
        TT:SetOwner(B, "ANCHOR_RIGHT"); TT:SetHyperlink("item:6001:0:0:0")
        local line = "none"
        for i = 1, TT:NumLines() do
            local t = getglobal("TTTextLeft" .. i):GetText()
            if t and string.find(t, "^Use:") then line = t end
        end
        table.insert(Seen, arg1 .. ":" .. line)
    end)
"#;

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
}

impl Client {
    /// One frame, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        crate::script_calls::eval_in_vm(self.app.world(), "local s = Seen Seen = {} return s")
    }
}

fn system(text: &str) -> SessionEvent {
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
}

/// The client in the world as [`ME`], a male human mage, with the charm's record cached and a
/// VM whose spy builds its tooltip at each system line.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, _sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    let spell = SpellDisplay {
        id: SPELL,
        name: "Charm".into(),
        description: Some("Charms $ghis:her; foes.".into()),
        ..Default::default()
    };
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(SPELL, spell)])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.insert_resource(crate::ui_quest_log::QuestHeaderNamesRes(
        benilla_formats::QuestHeaderNames::from_zones([(GOLDSHIRE, "Goldshire")]),
    ));
    app.update();
    let script = UiScript::new().expect("a VM");
    crate::ui_script::test_ui::load_tooltip_template(&script);
    script.run(SPY).expect("the spy");
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut t = crate::items::test_template("Charm of Gender");
    t.spells = vec![ItemSpellEntry {
        index: 0,
        spell_id: SPELL,
        trigger: 0,
        charges: 0,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    }];
    app.world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(CHARM, Some(t));
    let mut c = Client { app, inbound };
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[(BYTES_0, HUMAN_MAGE)],
    )]]);
    c.frame(vec![]);
    c.seen();
    c
}

/// Two writes of our gender byte in one frame, and a line after each: each line's tooltip build
/// expands the charm's use line against the player as the packets before it left it (`$g` reads
/// the caster at the build, `0x508214`), not as the frame's last packet left it.
#[test]
fn a_tooltip_between_two_player_writes_expands_against_the_first() {
    let mut c = client();
    c.frame(vec![
        vec![values(ME, &[(BYTES_0, HUMAN_MAGE | FEMALE)])],
        vec![system("first")],
        vec![values(ME, &[(BYTES_0, HUMAN_MAGE)])],
        vec![system("second")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "first:Use: Charms her foes.",
            "second:Use: Charms his foes."
        ]
    );
}

/// `GetBindLocation` reads the bind point at the call, through the lent host (`0x48dae3` →
/// `0x5ed420`): a bind stored since the last frame's feed answers at once.
#[test]
fn get_bind_location_reads_the_bind_point_at_the_call() {
    let mut c = client();
    c.app.world_mut().resource_mut::<crate::net::HomeBind>().0 = Some(GOLDSHIRE);
    let named = crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM")
            .eval::<String>("return GetBindLocation()")
            .expect("the chunk runs")
    });
    assert_eq!(named, "Goldshire");
}
