//! The mail getters as a listener reads them, and the mailbox's events at their packets. The
//! getters read the mailbox cell, the records and the pending-mail time at the call (`0x4aeb90`,
//! `0x4aebc0`, `0x4afea0`); the events fire inside the handler that raises them: the list's tail
//! (`0x4ad5d6`), the result's (`0x4ad16a`), the time reply's (`0x4ad605`) and the record
//! callbacks' (`0x4af080`, `0x4af0a0`), and the open and close cores' halves wait behind what the
//! lane still holds.

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::messages::{mail_action, mail_error, mail_message_type, MailListEntry};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;
use crate::ui_mail::{MailEvent, MailLane, MailOpen, MailPending};

const ME: u64 = 0x7;
const BOB: u64 = 0x42;
const MAILBOX: u64 = 0xF110_0000_0000_0B0C;
const BANKER: u64 = 0xF130_0000_0102_0001;
const WOOL: u32 = 2592;

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

    /// What went on the wire since the last call, as debug text.
    fn sent(&mut self) -> Vec<String> {
        self.sent.try_iter().map(|c| format!("{c:?}")).collect()
    }

    /// `packets` run through their handlers with no frame: the world as the handlers leave it.
    fn handle(&mut self, packets: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), packets);
        self.app.world_mut().flush();
    }

    /// A mailbox use, as the world click's command makes it.
    fn open_mailbox(&mut self) {
        crate::ui_mail::open_mailbox(self.app.world_mut(), MAILBOX);
    }
}

/// The client in the world, and a spy whose line for each mail event and monster line carries its
/// name and first argument.
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
            for _, e in ipairs({"MAIL_SHOW", "MAIL_CLOSED", "MAIL_INBOX_UPDATE", "MAIL_FAILED",
                    "UPDATE_PENDING_MAIL", "BANKFRAME_OPENED", "CHAT_MSG_MONSTER_SAY"}) do
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
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[(FIELD_UNIT_HEALTH, 100)],
    )]]);
    c.frame(vec![]);
    c.seen();
    c.sent();
    c
}

/// Bob's letter, `subject`, holding `item` when nonzero.
fn letter(id: u32, subject: &str, item: u32) -> MailListEntry {
    MailListEntry {
        message_id: id,
        message_type: mail_message_type::NORMAL,
        sender_guid: Some(BOB),
        sender_id: None,
        subject: subject.into(),
        item_text_id: 0,
        package: 0,
        stationery: 41,
        item: (item != 0).then_some(benilla_protocol::messages::MailAttachment {
            entry: item,
            perm_enchant: 0,
            random_prop_id: 0,
            suffix_factor: 0,
            count: 1,
            charges: 0,
            durability_max: 0,
            durability: 0,
        }),
        money: 0,
        cod: 0,
        checked: 0,
        expire_days: 29.0,
        mail_template_id: 0,
    }
}

fn list(mails: Vec<MailListEntry>) -> SessionEvent {
    SessionEvent::MailList { mails }
}

/// A successful send's result.
fn sent_ok() -> SessionEvent {
    SessionEvent::SendMailResult {
        mail_id: 0,
        action: mail_action::SEND,
        error: mail_error::OK,
        equip_error: None,
        item: None,
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

fn bob_named() -> SessionEvent {
    SessionEvent::PlayerName {
        guid: BOB,
        name: "Bob".into(),
        race: 1,
        gender: 0,
        class: 1,
    }
}

const READ: &str = "local _, _, sender, subject = GetInboxHeaderInfo(1) \
    return GetInboxNumItems() .. '/' .. tostring(subject) .. '/' .. tostring(sender) .. '/' \
    .. tostring(HasNewMail())";

/// A list and the time reply land with no frame between: the getters read the session at the
/// call, the next packet's rows and time included, with no push; with no host the VM's copy, which
/// nothing pushes, holds none.
#[test]
fn the_mail_getters_read_the_session_at_the_call() {
    let mut c = client();
    c.open_mailbox();
    c.handle(vec![list(vec![letter(1, "Hello", 0)])]);
    assert_eq!(c.eval::<String>(true, READ), "1/Hello/nil/nil");
    c.app
        .world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(BOB, "Bob".into(), None);
    c.handle(vec![
        list(vec![letter(1, "Hello", 0), letter(2, "Again", 0)]),
        SessionEvent::NextMailTime { seconds: 0.0 },
    ]);
    assert_eq!(c.eval::<String>(true, READ), "2/Hello/Bob/1");
    assert_eq!(
        c.eval::<String>(false, READ),
        "0/nil/nil/nil",
        "the VM's copy holds nothing"
    );
}

/// The list's tail, a result's tail and the time reply's signal sit on the mail lane at their
/// handlers, between the stamps of the handlers before and after.
#[test]
fn the_mail_events_are_stamped_at_their_handlers() {
    let mut c = client();
    c.open_mailbox();
    c.seen();
    let before = crate::lanes::stamp();
    c.handle(vec![
        list(vec![letter(1, "Hello", 0)]),
        sent_ok(),
        SessionEvent::NextMailTime { seconds: 0.0 },
    ]);
    let after = crate::lanes::stamp();
    let lane = c.app.world().resource::<MailLane>();
    let stamped = lane.stamped();
    assert!(
        stamped.iter().all(|(at, _)| before < *at && *at < after),
        "stamped inside the handlers"
    );
    let events: Vec<&MailEvent> = stamped.into_iter().map(|(_, e)| e).collect();
    assert!(
        matches!(
            events[..],
            [MailEvent::List, MailEvent::Result(_), MailEvent::Pending]
        ),
        "{events:?}"
    );
    assert!(c.seen().is_empty(), "nothing fired in the handlers");
}

/// Each mail event fires at its packet's place among a chat line's of the packets around it. The
/// mailbox is no object here, so the frame's walk-away watch closes it (`0x4818c0` → `0x493305`),
/// in the frame time, behind the packets' items.
#[test]
fn the_mail_events_fire_at_their_packets() {
    let mut c = client();
    c.open_mailbox();
    assert_eq!(
        c.seen(),
        ["MAIL_SHOW:nil"],
        "the use shows the window in place"
    );
    c.frame(vec![
        vec![boar_says("a")],
        vec![list(vec![letter(1, "Hello", 0)])],
        vec![boar_says("b")],
        vec![sent_ok()],
        vec![boar_says("c")],
        vec![SessionEvent::NextMailTime { seconds: 0.0 }],
        vec![boar_says("d")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "MAIL_INBOX_UPDATE:nil",
            "CHAT_MSG_MONSTER_SAY:b",
            "MAIL_FAILED:nil",
            "CHAT_MSG_MONSTER_SAY:c",
            "UPDATE_PENDING_MAIL:nil",
            "CHAT_MSG_MONSTER_SAY:d",
            "MAIL_CLOSED:nil",
        ]
    );
}

/// A getter whose name and item lookups miss asks inside its call and keeps one callback per call
/// (`0x4aedf6`, `0x4aecc7`); each answer's callbacks signal `MAIL_INBOX_UPDATE` at its packet
/// (`0x4af08f`, `0x4af0af`), and an answer nothing waits on signals nothing.
#[test]
fn the_record_answers_signal_the_updates_their_getters_waited_for() {
    let mut c = client();
    c.open_mailbox();
    c.frame(vec![vec![list(vec![letter(1, "Hello", WOOL)])]]);
    c.seen();
    c.sent();
    c.eval::<()>(true, "GetInboxHeaderInfo(1) GetInboxHeaderInfo(1)");
    let asked = c.sent();
    assert_eq!(
        asked.iter().filter(|s| s.starts_with("NameQuery")).count(),
        1,
        "asked once while in flight: {asked:?}"
    );
    assert_eq!(
        asked.iter().filter(|s| s.starts_with("ItemQuery")).count(),
        1,
        "{asked:?}"
    );
    c.frame(vec![
        vec![boar_says("a")],
        vec![bob_named()],
        vec![boar_says("b")],
        vec![SessionEvent::ItemTemplate {
            entry: WOOL,
            info: Some(Box::new(crate::items::test_template("Wool Cloth"))),
        }],
        vec![boar_says("c")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "MAIL_INBOX_UPDATE:nil",
            "MAIL_INBOX_UPDATE:nil",
            "CHAT_MSG_MONSTER_SAY:b",
            "MAIL_INBOX_UPDATE:nil",
            "MAIL_INBOX_UPDATE:nil",
            "CHAT_MSG_MONSTER_SAY:c",
        ]
    );
    assert_eq!(
        c.eval::<String>(true, "local _, _, s = GetInboxHeaderInfo(1) return s"),
        "Bob"
    );
    c.frame(vec![vec![bob_named()]]);
    assert!(c.seen().is_empty(), "no callback waits on a held name");
}

/// A won notice whose item the list parse misses keeps the callback `0x4ad010` (`0x4acf06`), and
/// its invoice getter whose buyer's name misses keeps `0x4af5b0` (`0x4af4b7`): each answer signals
/// `MAIL_INBOX_UPDATE` at its packet.
#[test]
fn the_notice_callbacks_signal_at_their_answers() {
    const SELLER: u64 = 0x51;
    let mut c = client();
    c.open_mailbox();
    let notice = MailListEntry {
        message_type: mail_message_type::AUCTION,
        sender_guid: None,
        sender_id: Some(1),
        subject: format!("{WOOL}:0:1"),
        item_text_id: 9,
        ..letter(1, "", 0)
    };
    c.app
        .world_mut()
        .resource_mut::<MailOpen>()
        .bodies
        .insert(9, format!("{SELLER:016X}:500:0"));
    c.frame(vec![vec![list(vec![notice])]]);
    c.seen();
    let asked = c.sent();
    assert!(
        asked.iter().any(|s| s.starts_with("ItemQuery")),
        "the parse asks the notice's item: {asked:?}"
    );
    let invoice = "return (GetInboxInvoiceInfo(1))";
    assert_eq!(c.eval::<Option<String>>(true, invoice), None);
    assert!(
        c.sent().iter().any(|s| s.starts_with("NameQuery")),
        "the getter asks the buyer's name"
    );
    c.frame(vec![
        vec![boar_says("a")],
        vec![SessionEvent::ItemTemplate {
            entry: WOOL,
            info: Some(Box::new(crate::items::test_template("Wool Cloth"))),
        }],
        vec![boar_says("b")],
        vec![SessionEvent::PlayerName {
            guid: SELLER,
            name: "Sal".into(),
            race: 1,
            gender: 0,
            class: 1,
        }],
        vec![boar_says("c")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "MAIL_INBOX_UPDATE:nil",
            "CHAT_MSG_MONSTER_SAY:b",
            "MAIL_INBOX_UPDATE:nil",
            "CHAT_MSG_MONSTER_SAY:c",
        ]
    );
    assert_eq!(
        c.eval::<Option<String>>(true, invoice).as_deref(),
        Some("buyer")
    );
}

/// The list's tail is on the lane when the mailbox is used and a bank opens over it, all with no
/// frame between: the open's show and the close's reset, signal and refresh wait behind the list,
/// so they fire whole at their place, the refresh's query with them, and leave the window shut.
#[test]
fn a_one_frame_open_then_close_leaves_no_stale_state() {
    let mut c = client();
    c.handle(vec![list(vec![letter(1, "Hello", 0)])]);
    c.app
        .world_mut()
        .resource_mut::<MailPending>()
        .arm_refresh();
    c.open_mailbox();
    c.handle(vec![SessionEvent::ShowBank { banker: BANKER }]);
    assert!(c.seen().is_empty(), "nothing fired ahead of the list");
    assert!(
        !c.sent().iter().any(|s| s.starts_with("QueryNextMailTime")),
        "the close's query waits with its signal"
    );
    assert_eq!(c.app.world().resource::<MailOpen>().mailbox, None);
    c.frame(vec![]);
    assert_eq!(
        c.seen(),
        [
            "MAIL_INBOX_UPDATE:nil",
            "MAIL_SHOW:nil",
            "MAIL_CLOSED:nil",
            "BANKFRAME_OPENED:nil",
        ]
    );
    assert_eq!(
        c.sent()
            .iter()
            .filter(|s| s.starts_with("QueryNextMailTime"))
            .count(),
        1
    );
    // The rows outlive the close, and no verb finds a mailbox, through the host or the VM's copy.
    assert_eq!(c.eval::<i64>(true, "return GetInboxNumItems()"), 1);
    c.eval::<()>(true, "CheckInbox()");
    c.eval::<()>(false, "CheckInbox()");
    assert!(c.sent().is_empty(), "no list asked of a shut mailbox");
    c.frame(vec![]);
    c.frame(vec![]);
    assert!(c.seen().is_empty(), "nothing fires after");
}

/// The world frame's step signals the countdown's crossing behind the frame's packets
/// (`0x4818d0` → `0x4adeba`).
#[test]
fn the_pending_mail_crossing_fires_behind_the_frames_packets() {
    let mut c = client();
    c.app
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(16),
        ));
    c.handle(vec![SessionEvent::ReceivedMail { seconds: 0.001 }]);
    c.frame(vec![vec![boar_says("a")], vec![boar_says("b")]]);
    let seen = c.seen();
    let crossing = seen.iter().position(|s| s == "UPDATE_PENDING_MAIL:nil");
    assert_eq!(crossing, Some(2), "after both lines, once: {seen:?}");
    assert_eq!(c.eval::<i64>(true, "return HasNewMail()"), 1);
}

/// A frame whose walk-away watch closes the mailbox and whose step crosses the countdown fires the
/// close first: the world frame's update runs the watch (`0x4818c0` → `0x493305`) before the step
/// (`0x4818d0` → `0x4adeba`).
#[test]
fn the_walk_away_close_fires_ahead_of_the_frames_pending_crossing() {
    let mut c = client();
    c.app
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(16),
        ));
    c.handle(vec![SessionEvent::ReceivedMail { seconds: 0.001 }]);
    c.open_mailbox();
    assert_eq!(c.seen(), ["MAIL_SHOW:nil"]);
    c.frame(vec![]);
    assert_eq!(c.seen(), ["MAIL_CLOSED:nil", "UPDATE_PENDING_MAIL:nil"]);
}
