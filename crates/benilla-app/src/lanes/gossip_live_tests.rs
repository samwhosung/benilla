//! The gossip getters as a listener reads them, and `GOSSIP_SHOW` at its packet. The getters read
//! the message's slots (`0xbbbe90`, `0x4e28d0`), its quest rows (`0xbb74c0`) and the text buffer
//! (`0xbbb678`, `0x4e28c0`) at the call; the show fires inside `0x4e2010` (`0x4e22b5`), right after
//! the text's write (`0x4e229a`), which the message runs (`0x4e27df`) and so does its text's
//! arrival (`0x4e22f2`).

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::messages::{GossipOption, NpcTextBlock};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
const NPC: u64 = 0xF130_0000_0000_0043;
/// `OBJECT_FIELD_ENTRY`, absolute, and the NPC's creature record.
const OBJECT_ENTRY: u16 = 3;
const NPC_ENTRY: u32 = 0x45;

/// The whole client, headless, in the world.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
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

    /// The pet-name queries sent since the last call, by pet number.
    fn pet_name_queries(&mut self) -> Vec<u32> {
        crate::query_cache::queries(&self.sent)
            .into_iter()
            .filter_map(|c| match c {
                ClientCommand::PetNameQuery { pet_number, .. } => Some(pet_number),
                _ => None,
            })
            .collect()
    }

    /// `packets` run through their handlers with no frame: the world as the handlers leave it.
    fn handle(&mut self, packets: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), packets);
    }
}

/// The client in the world with an NPC beside us, a VM `prepare` seeds, and a spy whose line for
/// each gossip event carries its name and first argument.
fn client_with(prepare: impl FnOnce(&mut UiScript)) -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let mut script = UiScript::new().expect("a VM");
    prepare(&mut script);
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"GOSSIP_SHOW", "GOSSIP_CLOSED", "CHAT_MSG_MONSTER_SAY"}) do
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
    let mut c = Client { app, inbound, sent };
    c.frame(vec![vec![
        // Alive, so a panel opens (`ShowUIPanel`'s `UnitIsDead` test, `UIParent.lua`).
        created(ME, EntityKind::Player, &[(FIELD_UNIT_HEALTH, 100)]),
        created(NPC, EntityKind::Unit, &[(OBJECT_ENTRY, NPC_ENTRY)]),
    ]]);
    c.frame(vec![]);
    c.seen();
    c
}

fn client() -> Client {
    client_with(|_| {})
}

fn option(label: &str) -> GossipOption {
    GossipOption {
        index: 0,
        icon: 1,
        coded: false,
        message: label.into(),
    }
}

/// `NPC`'s message on text `text_id` with one option labelled `label`, and one available quest.
fn menu(text_id: u32, label: &str) -> SessionEvent {
    SessionEvent::GossipMenu {
        npc: NPC,
        text_id,
        options: vec![option(label)],
        quests: vec![(783, 2, 5, "A Threat Within".into())],
    }
}

/// Text `text_id`'s record, `line` in both columns.
fn greeting(text_id: u32, line: &str) -> SessionEvent {
    SessionEvent::NpcGreeting {
        text_id,
        blocks: vec![NpcTextBlock {
            probability: 1.0,
            male: line.into(),
            female: line.into(),
        }],
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

const STORE: &str = "local function n(...) return arg.n end \
    return GetGossipText() .. ',' .. (GetGossipOptions()) .. ',' .. n(GetGossipOptions()) \
    .. ',' .. (GetGossipAvailableQuests())";

/// Each getter reads what the handlers wrote at the call, with no push between: the drawn text,
/// the slots and the quest rows; with no host the VM's copy, which nothing pushes, holds none.
#[test]
fn the_gossip_getters_read_the_world_at_the_call() {
    let mut c = client();
    c.handle(vec![menu(7, "Browse"), greeting(7, "Hello")]);
    assert_eq!(
        c.eval::<String>(true, STORE),
        "Hello,Browse,2,A Threat Within"
    );
    assert_eq!(
        c.eval::<String>(false, "return GetGossipText()"),
        "",
        "with no host to ask, the empty buffer, never nil"
    );
    // The next message's slots read at once; its text, still out, leaves the last.
    c.handle(vec![menu(8, "Fly")]);
    assert_eq!(c.eval::<String>(true, STORE), "Hello,Fly,2,A Threat Within");
    crate::ui_gossip::close_core(c.app.world_mut());
    assert_eq!(
        c.eval::<String>(true, STORE),
        "Hello,Fly,2,A Threat Within",
        "a close leaves the store (`0x4e1fa0`)"
    );
}

/// The draw's `GOSSIP_SHOW` sits on the gossip lane at the text's handler, between the stamps of
/// the handlers before and after it.
#[test]
fn the_show_is_stamped_at_its_handler() {
    let mut c = client();
    c.handle(vec![menu(7, "Browse")]);
    assert!(c
        .app
        .world()
        .resource::<crate::ui_gossip::GossipLane>()
        .stamped()
        .is_empty());
    let before = crate::lanes::stamp();
    c.handle(vec![greeting(7, "Hello")]);
    let after = crate::lanes::stamp();
    let stamped = c
        .app
        .world()
        .resource::<crate::ui_gossip::GossipLane>()
        .stamped();
    assert_eq!(stamped.len(), 1);
    let (at, event) = stamped[0];
    assert_eq!(event, crate::ui_gossip::GossipEvent::Show);
    assert!(before < at && at < after, "stamped inside the handler");
}

/// A cached text shows at its message and a first visit at its text's arrival, each at its
/// packet's place around a chat line of a packet between; the round trip fires nothing.
#[test]
fn the_show_fires_at_its_packet() {
    let mut c = client();
    c.handle(vec![greeting(7, "Hello")]);
    c.frame(vec![
        vec![boar_says("a")],
        vec![menu(7, "Browse")],
        vec![boar_says("b")],
        vec![menu(8, "Fly")],
        vec![boar_says("c")],
        vec![greeting(8, "Safe travels")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "GOSSIP_SHOW:nil",
            "CHAT_MSG_MONSTER_SAY:b",
            "CHAT_MSG_MONSTER_SAY:c",
            "GOSSIP_SHOW:nil",
        ]
    );
    assert_eq!(
        c.eval::<String>(true, "return GetGossipText()"),
        "Safe travels"
    );
}

/// A close in the show's frame fires after it. A text landing after a close draws nothing: the
/// close cancels no wait (`0x4e1fa0`), but the draw finds no object under the zeroed cell and
/// bails (`0x4e20bb` → `0x4e22ba`), with no text write and no signal. The close's latch release
/// re-enters it with the cell still set (`0x4e1fc5` → `0x493310` → `0x4e1fa0`), so
/// `GOSSIP_CLOSED` signals twice (`0x4e1fdc`).
#[test]
fn a_close_fires_after_the_show_and_a_late_text_draws_nothing() {
    let mut c = client();
    c.handle(vec![greeting(7, "Hello"), menu(7, "Browse")]);
    crate::ui_gossip::close_core(c.app.world_mut());
    c.frame(vec![]);
    assert_eq!(
        c.seen(),
        ["GOSSIP_SHOW:nil", "GOSSIP_CLOSED:nil", "GOSSIP_CLOSED:nil"]
    );
    c.frame(vec![vec![menu(9, "Fly")]]);
    crate::ui_gossip::close_core(c.app.world_mut());
    c.frame(vec![vec![greeting(9, "Too late")]]);
    assert_eq!(c.seen(), ["GOSSIP_CLOSED:nil", "GOSSIP_CLOSED:nil"]);
    assert_eq!(c.eval::<String>(true, "return GetGossipText()"), "Hello");
}

/// A message whose NPC is no object here draws nothing either (`0x4e20bb`).
#[test]
fn a_menu_of_an_npc_not_held_draws_nothing() {
    let mut c = client();
    c.handle(vec![greeting(7, "Hello")]);
    c.frame(vec![vec![SessionEvent::GossipMenu {
        npc: 0xF130_0000_0000_0099,
        text_id: 7,
        options: vec![option("Browse")],
        quests: Vec::new(),
    }]]);
    assert!(!c.seen().iter().any(|e| e == "GOSSIP_SHOW:nil"));
    assert_eq!(c.eval::<String>(true, "return GetGossipText()"), "");
}

/// The show asks the NPC's name as the stock window's `UnitName("npc")` does there
/// (`GossipFrame.lua:33`, `0x5171b8`), at the draw that signals it, a first visit's text; never
/// the round trip before it. A held creature's name is its record, which its create asked for, so
/// the ask is seen on an NPC that names itself by its pet number (`0x609210`).
#[test]
fn the_show_asks_the_npcs_name() {
    /// `UNIT_FIELD_PETNUMBER`, absolute.
    const PET_NUMBER: u16 = 139;
    let mut c = client();
    c.handle(vec![
        values(NPC, &[(PET_NUMBER, 5)]),
        SessionEvent::ObjectUpdateEnd,
    ]);
    c.app
        .world_mut()
        .resource_mut::<crate::names::NameCache>()
        .forget_pet(5);
    c.pet_name_queries();
    c.handle(vec![menu(8, "Fly")]);
    assert!(c.pet_name_queries().is_empty(), "no show, no ask");
    c.handle(vec![greeting(8, "Safe travels")]);
    assert_eq!(c.pet_name_queries(), [5], "asked at the draw");
}

/// The greeting's macros expand once per message, as `0x4e2010` writes the expanded text to
/// `0xbbb678` at its draw (`0x4e2274`, `0x4e229a`): a later change to the player's name leaves it
/// until the next message.
#[test]
fn the_greeting_expands_once_per_message() {
    let mut c = client();
    let named = |c: &mut Client, name: &str| {
        c.app
            .world_mut()
            .resource_mut::<crate::names::NameCache>()
            .insert_player(ME, name.into(), None);
    };
    named(&mut c, "Ann");
    c.handle(vec![greeting(7, "Well met, $N."), menu(7, "Browse")]);
    assert_eq!(
        c.eval::<String>(true, "return GetGossipText()"),
        "Well met, Ann."
    );
    named(&mut c, "Bea");
    assert_eq!(
        c.eval::<String>(true, "return GetGossipText()"),
        "Well met, Ann.",
        "held until the next message"
    );
    c.handle(vec![menu(7, "Browse")]);
    assert_eq!(
        c.eval::<String>(true, "return GetGossipText()"),
        "Well met, Bea."
    );
}

/// Stock `GossipFrame.xml` through a first-visit sub-menu: the window stays up on the first menu
/// while the text is out, with nothing signalled, and repaints in place once it lands.
#[test]
fn stock_gossip_frame_stays_visible_through_a_first_visit_submenu() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut c = client_with(|s| {
        s.set_screen_size(1024.0, 768.0);
        s.set_text_measurer(Box::new(crate::ui_script::FixedWidthFont(6.0)));
        for f in crate::ui_script::test_ui::production_order(&[
            crate::ui_script::test_ui::GOSSIP_UI,
            &[r"Interface\FrameXML\GossipFrame.xml"],
        ]) {
            crate::ui_script::test_ui::load_ui(s, f);
        }
    });
    // What the window shows: visible?, the greeting, the first row's label.
    let window = |c: &mut Client| -> (bool, String, String) {
        let s = c.app.world().non_send_resource::<UiScript>();
        (
            s.eval::<bool>("return GossipFrame:IsVisible()").unwrap(),
            s.eval::<String>("return GossipGreetingText:GetText() or ''")
                .unwrap(),
            s.eval::<String>("return GossipTitleButton1:GetText() or ''")
                .unwrap(),
        )
    };
    // A message with one option and no quest row.
    let menu = |text_id: u32, label: &str| SessionEvent::GossipMenu {
        npc: NPC,
        text_id,
        options: vec![option(label)],
        quests: Vec::new(),
    };
    c.frame(vec![
        vec![greeting(50, "What are you looking for?")],
        vec![menu(50, "The bank")],
    ]);
    assert_eq!(c.seen(), ["GOSSIP_SHOW:nil"]);
    assert_eq!(
        window(&mut c),
        (
            true,
            "What are you looking for?".to_string(),
            "The bank".to_string(),
        ),
        "the greeting menu opens"
    );
    for _ in 0..3 {
        c.frame(vec![vec![menu(51, "Goodbye")]]);
        assert!(c.seen().is_empty(), "nothing fires during the hold");
        assert_eq!(
            window(&mut c),
            (
                true,
                "What are you looking for?".to_string(),
                "The bank".to_string(),
            ),
            "the window stays up, unchanged, for the round trip"
        );
    }
    c.frame(vec![vec![greeting(51, "The bank is north of here.")]]);
    assert_eq!(c.seen(), ["GOSSIP_SHOW:nil"]);
    assert_eq!(
        window(&mut c),
        (
            true,
            "The bank is north of here.".to_string(),
            "Goodbye".to_string(),
        ),
        "an in-place repaint"
    );
}

/// `SMSG_GOSSIP_COMPLETE` closes the menu inside its handler (`0x4e1fa0` with a zero guid), whose
/// release re-enters it (`0x4e1fc5` → `0x493310`): both `GOSSIP_CLOSED`s fire at the packet's
/// place, after an earlier packet's chat line and ahead of a later one's, though the gossip lane
/// holds nothing when the handler runs.
#[test]
fn a_server_close_fires_at_its_packet() {
    let mut c = client();
    c.frame(vec![vec![greeting(7, "Hello")], vec![menu(7, "Browse")]]);
    assert_eq!(c.seen(), ["GOSSIP_SHOW:nil"]);
    c.frame(vec![
        vec![boar_says("a")],
        vec![SessionEvent::GossipComplete],
        vec![boar_says("b")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "GOSSIP_CLOSED:nil",
            "GOSSIP_CLOSED:nil",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
}
