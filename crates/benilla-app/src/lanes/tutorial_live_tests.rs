//! The tutorials at their places. Each trigger site fires inside the handler or the input that
//! raises it (`0x4b5390`, the callers in benilla-re's `tables/tutorial_triggers.tsv`): the
//! battleground status's (`0x4aa91d`, `0x4aaa93`), the vendor list's (`0x4fad32`), the dynamic
//! flags watcher's Looting (`0x60049b`) and the item-received function's, after its line
//! (`0x491cdd`, then `0x491cec`). So a tutorial raised by a packet fires between the neighbouring
//! packets' events, and its fire-once test reads the bank as the listeners before it left it.
//! `TutorialsEnabled` reads the acknowledged bank at its call (`0x4b5960`).

use benilla_protocol::field::{FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_NPC_FLAGS};
use benilla_protocol::messages::{BattlefieldStatus, ItemPushResult, VendorItem};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
const VENDOR: u64 = 0xF130_0000_0000_0043;
const MOB: u64 = 0xF130_0000_0000_0051;
const WSG: u32 = 489;
const LINEN: u32 = 2589;
/// `TUTORIAL_TRIGGER`'s published ids, each the 0-based id plus one.
const BATTLEGROUND_QUEUE: &str = "TUTORIAL_TRIGGER:48";
const VENDORS: &str = "TUTORIAL_TRIGGER:20";
const LOOTING: &str = "TUTORIAL_TRIGGER:7";
const BACKPACK: &str = "TUTORIAL_TRIGGER:8";

/// The whole client, headless, in the world with its frame run.
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

    /// `chunk`'s answer through the lent host when `live`, else off the VM's copies alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// The tutorial flags sent since the last call.
    fn flags_sent(&mut self) -> Vec<u32> {
        self.sent
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::TutorialFlag { id } => Some(id),
                _ => None,
            })
            .collect()
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        self.eval(false, "local s = Seen Seen = {} return s")
    }
}

const SPY: &str = r#"
    Seen = {}
    LOOT_ITEM_PUSHED_SELF = "You receive item: %s."
    local f = CreateFrame("Frame")
    for _, e in ipairs({"TUTORIAL_TRIGGER", "UPDATE_BATTLEFIELD_STATUS", "MERCHANT_SHOW",
            "CHAT_MSG_MONSTER_SAY", "CHAT_MSG_LOOT"}) do
        f:RegisterEvent(e)
    end
    f:SetScript("OnEvent", function()
        if event == "CHAT_MSG_LOOT" then
            table.insert(Seen, event)
        else
            table.insert(Seen, event .. ":" .. tostring(arg1))
        end
        -- The Show Tutorials option unticked (`UIOptionsFrame.lua:324`).
        if event == "CHAT_MSG_MONSTER_SAY" and arg1 == "clear" then
            ClearTutorials()
        end
    end)
"#;

/// The client in the world, a vendor and a mob beside it, the tutorial banks clear, and a spy
/// whose line for each event carries its name and first argument.
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
    script.run(SPY).unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client { app, inbound, sent };
    c.frame(vec![vec![
        created(ME, EntityKind::Player, &[(FIELD_UNIT_HEALTH, 100)]),
        created(VENDOR, EntityKind::Unit, &[(FIELD_UNIT_NPC_FLAGS, 0x80)]),
        created(MOB, EntityKind::Unit, &[(FIELD_UNIT_HEALTH, 0)]),
    ]]);
    c.frame(vec![vec![SessionEvent::TutorialFlags(vec![0; 32])]]);
    c.frame(vec![]);
    c.seen();
    c
}

fn queued(slot: u32) -> SessionEvent {
    SessionEvent::BattlefieldStatus(BattlefieldStatus {
        slot,
        map_id: WSG,
        bracket: 1,
        instance_id: 3,
        status: 1,
        time_ms: None,
        in_progress: None,
        queued: Some((30_000, 0)),
    })
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

fn vendor_list() -> SessionEvent {
    SessionEvent::VendorInventory {
        vendor: VENDOR,
        items: vec![VendorItem {
            slot: 1,
            entry: 159,
            display_id: 0,
            current_count: 5,
            price: 25,
            max_durability: 0,
            buy_count: 1,
        }],
    }
}

/// The battleground status's tutorial (`0x4aaa93`) fires inside its handler, ahead of its own
/// `UPDATE_BATTLEFIELD_STATUS` (`0x4aab05`) and between the lines of the packets around it.
#[test]
fn a_tutorial_raised_by_a_packet_fires_between_its_neighbours_events() {
    let mut c = client();
    c.frame(vec![
        vec![boar_says("a")],
        vec![queued(0)],
        vec![boar_says("b")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            BATTLEGROUND_QUEUE,
            "UPDATE_BATTLEFIELD_STATUS:nil",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
}

/// The vendor list's handler triggers Vendors (`0x4fad32`) ahead of its `MERCHANT_SHOW`
/// (`0x4fad92`), between the neighbouring packets' lines; a later list fires none, its bit set.
#[test]
fn the_vendor_tutorial_fires_at_the_lists_handler() {
    let mut c = client();
    c.frame(vec![
        vec![boar_says("a")],
        vec![vendor_list()],
        vec![boar_says("b")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            VENDORS,
            "MERCHANT_SHOW:nil",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
    c.frame(vec![vec![vendor_list()]]);
    assert_eq!(c.seen(), ["MERCHANT_SHOW:nil"], "fire-once");
}

/// A unit's lootable bit rising triggers Looting inside the update's notify pass
/// (`0x600440`, `0x60049b`), at its packet's place.
#[test]
fn the_looting_tutorial_fires_at_the_dynamic_flags_watchers_place() {
    let mut c = client();
    c.frame(vec![
        vec![boar_says("a")],
        vec![values(MOB, &[(FIELD_UNIT_DYNAMIC_FLAGS, 0x1)])],
        vec![boar_says("b")],
    ]);
    assert_eq!(
        c.seen(),
        ["CHAT_MSG_MONSTER_SAY:a", LOOTING, "CHAT_MSG_MONSTER_SAY:b"]
    );
}

/// Our own push with its record held fires its line, then Backpack (`0x491cdd`, `0x491cec`),
/// ahead of the next packet's line.
#[test]
fn an_item_push_fires_its_tutorials_after_its_line() {
    let mut c = client();
    c.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(LINEN, Some(crate::items::test_template("Linen Cloth")));
    let push = SessionEvent::ItemPushResult(ItemPushResult {
        player_guid: ME,
        from_npc: true,
        created: false,
        show_in_chat: true,
        bag_slot: 255,
        item_slot: 23,
        item_entry: LINEN,
        suffix_factor: 0,
        random_property_id: 0,
        count: 1,
    });
    c.frame(vec![vec![push], vec![boar_says("b")]]);
    assert_eq!(
        c.seen(),
        ["CHAT_MSG_LOOT", BACKPACK, "CHAT_MSG_MONSTER_SAY:b"]
    );
}

/// The Show Tutorials option unticked in a listener, `ClearTutorials()` (`0x4b55c0`), sets every
/// bit before a later packet of the same frame triggers, so that trigger fires nothing
/// (`0x4b53c8`).
#[test]
fn tutorials_turned_off_by_an_earlier_listener_stop_the_next_trigger() {
    let mut c = client();
    c.frame(vec![vec![boar_says("clear")], vec![queued(0)]]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:clear",
            "UPDATE_BATTLEFIELD_STATUS:nil"
        ]
    );
    assert!(c.eval::<bool>(true, "return TutorialsEnabled() == nil"));
}

/// `TutorialsEnabled` reads the acknowledged bank at the call through the host, with nothing
/// pushed: a fresh VM (a `/reload`'s) reads it as it stands, and a clear in the chunk reads nil on
/// the next line.
#[test]
fn tutorials_enabled_reads_the_bank_at_the_call() {
    let mut c = client();
    assert_eq!(c.eval::<i64>(true, "return TutorialsEnabled()"), 1);
    assert!(
        c.eval::<bool>(false, "return TutorialsEnabled() == nil"),
        "no copy pushed to the VM"
    );
    c.app
        .insert_non_send_resource(UiScript::new().expect("a fresh VM"));
    assert_eq!(c.eval::<i64>(true, "return TutorialsEnabled()"), 1);
    assert!(c.eval::<bool>(true, "ClearTutorials() return TutorialsEnabled() == nil"));
}

/// `SetTrainer` (`0x4d7440`) on a new trainer acknowledges Learning Talents (`0x0c`) for our own
/// guid and Trainers (`0x0d`) for another (`0x4d7492`-`0x4d74ae`), its flag sent at the list's
/// place; a re-list from the open trainer acknowledges nothing (`0x4d7459`).
#[test]
fn a_trainer_list_acknowledges_its_tutorial_by_whose_it_is() {
    for (trainer, want) in [(VENDOR, 0x0d), (ME, 0x0c)] {
        let mut c = client();
        c.flags_sent();
        let list = || SessionEvent::TrainerList {
            trainer,
            trainer_type: 0,
            services: Vec::new(),
            greeting: String::new(),
        };
        c.app
            .world_mut()
            .resource_mut::<crate::ui_trainer::TrainerOpen>()
            .asked = trainer;
        c.frame(vec![vec![list()]]);
        assert_eq!(c.flags_sent(), [want], "trainer {trainer:#x}");
        c.frame(vec![vec![list()]]);
        assert!(c.flags_sent().is_empty(), "the open trainer's re-list");
    }
}
