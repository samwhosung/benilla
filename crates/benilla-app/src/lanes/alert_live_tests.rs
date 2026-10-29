//! The inventory alerts through the whole client: the recompute (`0x4c7ee0`) runs where its
//! watchers call it, inside the update packet's notify pass (the durability's `0x5d9920`, the
//! flags' `0x5d97aa`, a visible item's `0x5dee7a`), and at the world entry's set-up (`0x49097e` →
//! `0x4c6d30`), so `UPDATE_INVENTORY_ALERTS` fires between the packets around it, and
//! `GetInventoryAlertStatus` reads the statuses the last pass stored (`0xb71f60`).

use benilla_protocol::field::{
    FIELD_PLAYER_INV_SLOT_HEAD, FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR, FIELD_UNIT_HEALTH,
    FIELD_UNIT_MAXHEALTH,
};
use benilla_protocol::messages::{ChatMessage, ObjectType, CHAT_MSG_SYSTEM};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Sender;

use super::unit_view_tests::{created, values, wire_packets};

const ME: u64 = 0x7;
/// Another player in view.
const OTHER: u64 = 0x8;
/// Worn on the head, seat 0, the first alert region.
const HELM: (u64, u32) = (item(1), 7366);
/// In the backpack's first seat, 23: in no alert region.
const SPARE: (u64, u32) = (item(2), 2361);
/// In the main hand, seat 15, the ninth alert region.
const SWORD: (u64, u32) = (item(3), 25);

/// `ITEM_FIELD_FLAGS`, the temporary enchantment's id (`ITEM_FIELD_ENCHANTMENT` slot 1),
/// `ITEM_FIELD_DURABILITY` and `_MAXDURABILITY`.
const FLAGS: u16 = 21;
const TEMP_ENCHANT: u16 = 25;
const DURABILITY: u16 = 46;
const MAX_DURABILITY: u16 = 47;

const fn item(low: u64) -> u64 {
    (benilla_protocol::guid::HIGH_ITEM as u64) << 48 | low
}

/// The spy: each `UPDATE_INVENTORY_ALERTS` with the head's status as it fires, each system line
/// and each tutorial with the head's status as it shows, and `PLAYER_ENTERING_WORLD`.
const SPY: &str = r#"Seen = {}
    local f = CreateFrame("Frame")
    for _, e in ipairs({"UPDATE_INVENTORY_ALERTS", "CHAT_MSG_SYSTEM", "PLAYER_ENTERING_WORLD",
            "TUTORIAL_TRIGGER"}) do
        f:RegisterEvent(e)
    end
    f:SetScript("OnEvent", function()
        if event == "UPDATE_INVENTORY_ALERTS" then
            table.insert(Seen, "ALERTS:" .. GetInventoryAlertStatus(1))
        elseif event == "CHAT_MSG_SYSTEM" then
            table.insert(Seen, arg1 .. ":" .. GetInventoryAlertStatus(1))
        elseif event == "TUTORIAL_TRIGGER" then
            table.insert(Seen, "TUTORIAL:" .. arg1 .. ":" .. GetInventoryAlertStatus(1))
        else
            table.insert(Seen, event)
        end
    end)"#;

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

/// One of our items, `durability` of 50.
fn item_create((guid, entry): (u64, u32), durability: u32) -> SessionEvent {
    SessionEvent::ItemCreate {
        guid,
        container: false,
        fields: ObjectFields::from_pairs(&[
            (2, 0x3),
            (3, entry),
            (6, ME as u32),
            (8, ME as u32),
            (DURABILITY, durability),
            (MAX_DURABILITY, 50),
        ])
        .into_created(ObjectType::Item),
    }
}

/// `guid` in flat seat `seat` of our array.
fn seat(seat: u16, guid: u64) -> [(u16, u32); 2] {
    let lo = FIELD_PLAYER_INV_SLOT_HEAD + 2 * seat;
    [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)]
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

/// The client in the world as [`ME`], wearing [`HELM`] at `helm` durability and carrying
/// [`SPARE`], with [`OTHER`] in view; what the spy heard of the arrival.
fn client(helm: u32) -> (Client, Vec<String>) {
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
    script.run(SPY).expect("the spy");
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    {
        let mut items = app.world_mut().resource_mut::<crate::items::Items>();
        for (_, entry) in [HELM, SPARE, SWORD] {
            items.insert_template(entry, Some(crate::items::test_template("Gear")));
        }
    }
    let mut c = Client { app, inbound };
    let mut me = vec![(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)];
    me.extend(seat(0, HELM.0));
    me.extend(seat(23, SPARE.0));
    me.extend(seat(15, SWORD.0));
    c.frame(vec![
        vec![item_create(HELM, helm)],
        vec![item_create(SPARE, 50)],
        vec![item_create(SWORD, 50)],
        vec![created(ME, EntityKind::Player, &me)],
        vec![created(
            OTHER,
            EntityKind::Player,
            &[(FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2, HELM.1)],
        )],
    ]);
    let arrival = c.seen();
    (c, arrival)
}

/// The world entry's cascade sets the alerts up after its login events (`0x49096a`, then
/// `0x49097e` → `0x4c6d30` → `0x4c7ee0`): one recompute, over the gear we arrived in.
#[test]
fn the_world_entry_recomputes_after_entering_the_world() {
    let (_, arrival) = client(0);
    assert_eq!(arrival, ["PLAYER_ENTERING_WORLD", "ALERTS:4"]);
}

/// A durability write runs its watcher's recompute inside its packet (`0x5d97c0` →
/// `0x5d9920`): its signal fires between the packets around it, and a listener of each reads the
/// statuses as of its own.
#[test]
fn a_durability_change_signals_between_its_neighbours() {
    let (mut c, _) = client(40);
    c.frame(vec![
        vec![system("before")],
        vec![values(HELM.0, &[(DURABILITY, 0)])],
        vec![system("after")],
    ]);
    assert_eq!(c.seen(), ["before:0", "ALERTS:4", "after:4"]);
}

/// The getter reads the statuses the last pass stored, packet by packet: a break, a line, then
/// a repair in one frame read broken at the break and the line, whole at the repair.
#[test]
fn the_status_getter_reads_each_packets_pass() {
    let (mut c, _) = client(40);
    c.frame(vec![
        vec![values(HELM.0, &[(DURABILITY, 0)])],
        vec![system("between")],
        vec![values(HELM.0, &[(DURABILITY, 3)])],
        vec![values(HELM.0, &[(DURABILITY, 40)])],
    ]);
    assert_eq!(c.seen(), ["ALERTS:4", "between:4", "ALERTS:3", "ALERTS:0"]);
}

/// Every pass signals, a status moved or not (`0x4c8085` past the loop): the durability watcher
/// runs it for an item in no region (`0x5d97c0`), and a visible-item change runs it for any
/// player (`0x5dee7a`, ahead of `0x5dee82`'s test for us). A field with no recompute watcher,
/// `MAXDURABILITY`, or a flag bit other than the force-red one (`0x5d9722`), signals nothing.
#[test]
fn every_watched_pass_signals_and_nothing_else_does() {
    let (mut c, _) = client(40);
    c.frame(vec![vec![values(SPARE.0, &[(DURABILITY, 10)])]]);
    assert_eq!(c.seen(), ["ALERTS:0"], "a backpack item's durability");
    c.frame(vec![vec![values(
        OTHER,
        &[(FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2, SPARE.1)],
    )]]);
    assert_eq!(c.seen(), ["ALERTS:0"], "another player's visible item");
    c.frame(vec![vec![values(HELM.0, &[(MAX_DURABILITY, 60)])]]);
    assert_eq!(c.seen(), Vec::<String>::new(), "the maximum alone");
    c.frame(vec![vec![values(HELM.0, &[(FLAGS, 0x1)])]]);
    assert_eq!(c.seen(), Vec::<String>::new(), "a flag bit but force-red");
    c.frame(vec![vec![values(HELM.0, &[(FLAGS, 0x11)])]]);
    assert_eq!(c.seen(), ["ALERTS:4"], "the force-red bit");
}

/// The temporary-enchant arm (`0x4c8035`-`0x4c806d`) through the watchers: the enchantment's
/// write runs the recompute (`0x5d9b2f`), whose main hand reads 1 with more than 30 s left
/// (`0x5d9d00(1)`, `0x4c8060`). The time packet's setter (`0x5d9cc0`) runs no recompute, so the
/// status stays 1 past the 30 s mark until a watcher's next pass reads 2.
#[test]
fn a_temporary_enchant_reads_one_then_two_at_the_next_pass() {
    let (mut c, _) = client(40);
    let main_hand = |c: &mut Client| -> i64 {
        crate::script_calls::eval_in_vm(c.app.world(), "return GetInventoryAlertStatus(9)")
    };
    let time = |seconds: u32| SessionEvent::ItemEnchantTime {
        item_guid: SWORD.0,
        slot: 1,
        seconds,
    };
    c.frame(vec![
        vec![time(600)],
        vec![values(SWORD.0, &[(TEMP_ENCHANT, 7)])],
    ]);
    assert_eq!(c.seen(), ["ALERTS:0"]);
    assert_eq!(main_hand(&mut c), 1, "more than 30 s left");
    c.frame(vec![vec![time(20)]]);
    assert_eq!(
        c.seen(),
        Vec::<String>::new(),
        "the time packet recomputes nothing"
    );
    assert_eq!(main_hand(&mut c), 1, "as the last pass left it");
    c.frame(vec![vec![values(SWORD.0, &[(DURABILITY, 49)])]]);
    assert_eq!(c.seen(), ["ALERTS:0"]);
    assert_eq!(main_hand(&mut c), 2, "30 s or fewer left");
}

/// The pass fires its tutorials in its loop (`0x4b5390` with delay 0, `0x4c8002`), each region's
/// after its status is stored and ahead of the pass's signal (`0x4c8085`): Broken Items (36,
/// published 37) between the packets around the break, its listener reading the head broken.
#[test]
fn a_breaks_tutorial_fires_in_the_pass_ahead_of_its_signal() {
    let (mut c, _) = client(40);
    c.frame(vec![vec![SessionEvent::TutorialFlags(vec![0; 32])]]);
    c.frame(vec![]);
    c.seen();
    c.frame(vec![
        vec![system("before")],
        vec![values(HELM.0, &[(DURABILITY, 0)])],
        vec![system("after")],
    ]);
    assert_eq!(
        c.seen(),
        ["before:0", "TUTORIAL:37:4", "ALERTS:4", "after:4"]
    );
    c.frame(vec![]);
    assert_eq!(
        c.seen(),
        Vec::<String>::new(),
        "fired once, not again a frame late"
    );
}
