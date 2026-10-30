//! The item lock through the whole client: the lock is the item object's bit (`item+0x314`
//! bit 0), which only the constructor zeroes (`0x5d821f`, `0x5d823e`). A destroy frees the
//! object inside its handler (`0x4674a0` → `0x467521`), so a listener ahead of it reads the lock
//! and one behind a rebuild under the same guid reads none; a release leaves the bit set, as the
//! object waits in the released table (`0x464920`) and a block that brings it back reuses it
//! (`0x464530`). Neither the free nor the rebuild signals.

use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
use benilla_protocol::messages::{ChatMessage, ObjectType, CHAT_MSG_SYSTEM};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use crate::net::ClientCommand;

use super::unit_view_tests::{created, wire_packets};

pub(super) const ME: u64 = 0x7;
/// In the backpack's first seat, 23: bag 0, slot 1. Its record is never asked, so no record
/// set-up runs its arrival's unlock (`0x5d85ab`).
pub(super) const SPARE: (u64, u32) = (item(2), 2361);

const fn item(low: u64) -> u64 {
    (benilla_protocol::guid::HIGH_ITEM as u64) << 48 | low
}

/// The spy: each system line with the backpack's first slot's lock as `GetContainerItemInfo`
/// reads it at the call (`0x4f9752`), and each `ITEM_LOCK_CHANGED`.
const SPY: &str = r#"Seen = {}
    local f = CreateFrame("Frame")
    f:RegisterEvent("CHAT_MSG_SYSTEM")
    f:RegisterEvent("ITEM_LOCK_CHANGED")
    f:SetScript("OnEvent", function()
        if event == "ITEM_LOCK_CHANGED" then
            table.insert(Seen, "LOCK")
            return
        end
        local _, _, locked = GetContainerItemInfo(0, 1)
        table.insert(Seen, arg1 .. ":" .. tostring(locked))
    end)"#;

/// The whole client, headless, in the world with its frame run.
pub(super) struct Client {
    pub(super) app: App,
    inbound: Sender<SessionEvent>,
    /// What the client sent.
    pub(super) sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame, with `packets` in the inbound channel, each one packet.
    pub(super) fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// What the spy heard since the last call.
    pub(super) fn seen(&mut self) -> Vec<String> {
        crate::script_calls::eval_in_vm(self.app.world(), "local s = Seen Seen = {} return s")
    }
}

/// Our item, created as a first create or a bring-back.
pub(super) fn spare() -> SessionEvent {
    SessionEvent::ItemCreate {
        guid: SPARE.0,
        container: false,
        fields: ObjectFields::from_pairs(&[
            (2, 0x3),
            (3, SPARE.1),
            (6, ME as u32),
            (8, ME as u32),
            (14, 1),
        ])
        .into_created(ObjectType::Item),
    }
}

pub(super) fn system(text: &str) -> SessionEvent {
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

/// The client in the world as [`ME`], carrying [`SPARE`] locked in the backpack's first seat; the
/// spy emptied.
fn client() -> Client {
    let mut c = client_with(SPY);
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| s.expect("a VM").lock_item(SPARE.0));
    c.frame(vec![]);
    c.seen();
    c
}

/// The client in the world as [`ME`], carrying [`SPARE`] in the backpack's first seat, its VM
/// running `spy`, whose `Seen` is emptied, and what it sent drained.
pub(super) fn client_with(spy: &str) -> Client {
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
    script.run(spy).expect("the spy");
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client { app, inbound, sent };
    let seat = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
    c.frame(vec![
        vec![spare()],
        vec![created(
            ME,
            EntityKind::Player,
            &[(seat, SPARE.0 as u32), (seat + 1, (SPARE.0 >> 32) as u32)],
        )],
    ]);
    c.frame(vec![]);
    c.seen();
    c.sent.try_iter().for_each(drop);
    c
}

/// A destroy of the locked item and a create of it again under the same guid, in one frame
/// between two lines: the line ahead of the destroy reads the old object's lock, and the line
/// behind the rebuild reads the new object unlocked, with no `ITEM_LOCK_CHANGED` for either.
#[test]
fn a_line_ahead_of_a_destroy_reads_the_lock_and_one_behind_the_rebuild_none() {
    let mut c = client();
    c.frame(vec![
        vec![system("before")],
        vec![SessionEvent::ObjectDestroyed(SPARE.0)],
        vec![spare()],
        vec![system("after")],
    ]);
    assert_eq!(c.seen(), ["before:1", "after:nil"]);
}

/// The out-of-range list releases the locked item and a later block brings it back: the
/// object is the one that left, its bit still set, so the line behind it reads it locked.
#[test]
fn a_released_item_brought_back_keeps_its_lock() {
    let mut c = client();
    c.frame(vec![
        vec![SessionEvent::ObjectsRemoved(vec![SPARE.0])],
        vec![spare()],
        vec![system("back")],
    ]);
    assert_eq!(c.seen(), ["back:1"]);
    c.frame(vec![vec![system("later")]]);
    assert_eq!(c.seen(), ["later:1"], "and keeps it past the frame");
}

/// A world load frees every object (`0x401bf7` → `0x467700` → `0x467800`): the item the new
/// world creates under the guid reads unlocked, with no `ITEM_LOCK_CHANGED`.
#[test]
fn a_world_load_frees_the_lock_with_its_item() {
    let mut c = client();
    c.frame(vec![vec![SessionEvent::Worldport {
        map_id: 1,
        position: [0.0; 3],
        orientation: 0.0,
        needs_ack: false,
    }]]);
    c.frame(vec![]);
    c.seen();
    let seat = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
    c.frame(vec![
        vec![spare()],
        vec![created(
            ME,
            EntityKind::Player,
            &[(seat, SPARE.0 as u32), (seat + 1, (SPARE.0 >> 32) as u32)],
        )],
        vec![system("arrived")],
    ]);
    let heard = c.seen();
    assert_eq!(heard.last().map(String::as_str), Some("arrived:nil"));
    assert!(!heard.iter().any(|l| l == "LOCK"), "no signal: {heard:?}");
}
