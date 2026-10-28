//! A close a packet handler runs, parked behind its window's lane, through the stock frames: the
//! engine half (the cell, the latch, the sends, the model) runs inside the handler, as the
//! reference's close core runs there, and only the signals wait for their place. The stock
//! `OnHide` then calls the window's close verb from the parked signal's listeners, which acts on
//! the session that close closed, never on the one a later open in the same frame began
//! ([`crate::ui_session::in_parked_close`]).

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::messages::{
    GossipOption, NpcTextBlock, QuestDetails, QuestGiverList, QuestListEntry, TradeStatus,
};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
const NPC: u64 = 0xF130_0000_0000_0043;
const OTHER: u64 = 0xF130_0000_0000_0044;
const BOB: u64 = 0x9;
const CAROL: u64 = 0xA;
const MAILBOX: u64 = 0xF110_0000_0000_0B0C;

/// The stock files every window here needs under it, in the production order with its own.
const KIT: &[&str] = &[
    r"Interface\FrameXML\GameTooltip.xml",
    r"Interface\FrameXML\Cooldown.xml",
    r"Interface\FrameXML\ItemButtonTemplate.xml",
];

/// The whole client, headless, in the world, with stock frames loaded.
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

    /// `chunk`'s answer with the host lent.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, chunk: &str) -> T {
        crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
            s.expect("a VM").eval::<T>(chunk).expect("runs")
        })
    }

    /// What went out since the last call, the record queries left out.
    fn sent(&mut self) -> Vec<String> {
        self.sent
            .try_iter()
            .map(|c| format!("{c:?}"))
            .filter(|c| !c.contains("Query"))
            .collect()
    }
}

/// The client in the world with the stock `files` loaded over [`KIT`], and `units` created beside
/// us.
fn client(files: &[&str], units: &[(u64, EntityKind)]) -> Client {
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
    script.set_screen_size(1024.0, 768.0);
    script.set_text_measurer(Box::new(crate::ui_script::FixedWidthFont(6.0)));
    for f in crate::ui_script::test_ui::production_order(&[
        crate::ui_script::test_ui::GOSSIP_UI,
        KIT,
        files,
    ]) {
        crate::ui_script::test_ui::load_ui(&script, f);
    }
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client { app, inbound, sent };
    let mut packets = vec![vec![created(
        ME,
        EntityKind::Player,
        &[(FIELD_UNIT_HEALTH, 100)],
    )]];
    for &(guid, kind) in units {
        packets.push(vec![created(guid, kind, &[(FIELD_UNIT_HEALTH, 100)])]);
    }
    c.frame(packets);
    c.frame(vec![]);
    c.sent();
    c
}

fn visible(c: &mut Client, frame: &str) -> bool {
    c.eval(&format!("return {frame}:IsVisible() == 1"))
}

fn greeting(text_id: u32) -> SessionEvent {
    SessionEvent::NpcGreeting {
        text_id,
        blocks: vec![NpcTextBlock {
            probability: 1.0,
            male: "Hello".into(),
            female: "Hello".into(),
        }],
    }
}

fn menu(npc: u64, text_id: u32) -> SessionEvent {
    SessionEvent::GossipMenu {
        npc,
        text_id,
        options: vec![GossipOption {
            index: 0,
            icon: 0,
            coded: false,
            message: "Browse".into(),
        }],
        quests: Vec::new(),
    }
}

/// A giver's greeting, then its detail in a later frame: the detail's open registers with no test
/// of the cell (`0x4930d0`), which closes the greeting through the teardown inside the handler
/// (`0x501130`), and the stock `QUEST_FINISHED` handler's `HideUIPanel` → `OnHide` →
/// `CloseQuest()` (`QuestFrame.lua:19`, `:293`) re-enters a teardown that finds its cell zero. The
/// detail's panel keeps its giver, so the Accept answers it.
#[test]
fn a_quest_detail_after_a_greeting_accepts_with_the_givers_guid() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut c = client(
        &[r"Interface\FrameXML\QuestFrame.xml"],
        &[(NPC, EntityKind::Unit)],
    );
    c.frame(vec![vec![SessionEvent::QuestGreeting(QuestGiverList {
        npc: NPC,
        greeting: "Well met.".into(),
        emote_delay: 0,
        emote: 0,
        quests: vec![QuestListEntry {
            quest_id: 51,
            icon: 2,
            level: 9,
            title: "Kobolds".into(),
        }],
    })]]);
    assert!(visible(&mut c, "QuestFrame"));
    c.frame(vec![vec![SessionEvent::QuestDetail(QuestDetails {
        npc: NPC,
        quest_id: 51,
        title: "Kobolds".into(),
        details: "Clear the mine.".into(),
        objectives: String::new(),
        auto_finish: 0,
        choices: Vec::new(),
        rewards: Vec::new(),
        money: 0,
        reward_spell: 0,
    })]]);
    assert!(visible(&mut c, "QuestFrame"), "the detail panel is up");
    c.sent();
    let _: () = c.eval("AcceptQuest()");
    assert_eq!(
        c.sent(),
        [format!(
            "{:?}",
            ClientCommand::QuestgiverAccept {
                npc: NPC,
                quest: 51
            }
        )]
    );
}

/// A menu from a second NPC in a later frame: its register closes the first's menu inside the
/// handler (`0x4e1fa0`), whose `GOSSIP_CLOSED` hides the stock frame (`GossipFrame.lua:19`), and
/// `OnHide`'s `CloseGossip()` re-enters a core that finds its cell zero; the second menu stays, so
/// its option goes to the second NPC.
#[test]
fn a_second_npcs_menu_takes_the_option() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut c = client(
        &[r"Interface\FrameXML\GossipFrame.xml"],
        &[(NPC, EntityKind::Unit), (OTHER, EntityKind::Unit)],
    );
    c.frame(vec![vec![greeting(7)], vec![menu(NPC, 7)]]);
    assert!(visible(&mut c, "GossipFrame"));
    c.frame(vec![vec![menu(OTHER, 7)]]);
    assert!(visible(&mut c, "GossipFrame"), "the second menu is up");
    c.sent();
    let _: () = c.eval("SelectGossipOption(1)");
    assert_eq!(
        c.sent(),
        [format!(
            "{:?}",
            ClientCommand::GossipSelectOption {
                guid: OTHER,
                option: 0,
                code: None,
            }
        )]
    );
}

/// A cancelled trade and a new request in one frame: the cancel's close sends `CMSG_CANCEL_TRADE`
/// inside its handler (`0x4bf527`), ahead of the ladder's `CMSG_BEGIN_TRADE` for the next
/// (`0x4bf7ee`); the stock `OnHide`'s `CloseTrade()` (`TradeFrame.lua:145`) re-enters it after a
/// trade has begun, and sends nothing that would end the new one.
#[test]
fn a_cancel_and_a_new_request_send_in_their_order() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut c = client(
        &[
            r"Interface\FrameXML\MoneyInputFrame.lua",
            r"Interface\FrameXML\MoneyInputFrame.xml",
            r"Interface\FrameXML\TradeFrame.xml",
        ],
        &[(BOB, EntityKind::Player), (CAROL, EntityKind::Player)],
    );
    let status = |status| SessionEvent::TradeStatus { status };
    c.frame(vec![
        vec![status(TradeStatus::BeginTrade { partner: BOB })],
        vec![status(TradeStatus::OpenWindow)],
    ]);
    assert!(visible(&mut c, "TradeFrame"));
    c.sent();
    c.frame(vec![
        vec![status(TradeStatus::Canceled)],
        vec![status(TradeStatus::BeginTrade { partner: CAROL })],
    ]);
    let trade: Vec<String> = c
        .sent()
        .into_iter()
        .filter(|s| s.contains("Trade"))
        .collect();
    // The status arm enters the core with the partner latched, so its release runs it once more
    // inside it (`0x493365`): two cancels, both inside the handler.
    assert_eq!(
        trade,
        [
            format!("{:?}", ClientCommand::CancelTrade),
            format!("{:?}", ClientCommand::CancelTrade),
            format!("{:?}", ClientCommand::BeginTrade),
        ]
    );
    assert!(!visible(&mut c, "TradeFrame"));
}

/// The mailbox's close inside a later handler (a register's, `0x493310` → `0x4acd50`) runs its
/// tail there, the deferred refresh's query and its "no mail" stamp (`0x4acda8`, `0x4acdb1`), so
/// an arrival a later packet of the frame brings (`0x4ad620`) stamps over it: mail is waiting. The
/// stock mail window inherits the friends window's tabs, so this runs over the kit alone: what it
/// shows is the engine's.
#[test]
fn an_arrival_after_a_mailbox_close_in_one_frame_leaves_mail_waiting() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut c = client(
        &[],
        &[(NPC, EntityKind::Unit), (MAILBOX, EntityKind::GameObject)],
    );
    crate::ui_mail::open_mailbox(c.app.world_mut(), MAILBOX);
    c.frame(vec![]);
    assert_eq!(
        c.app.world().resource::<crate::ui_mail::MailOpen>().mailbox,
        Some(MAILBOX)
    );
    c.app
        .world_mut()
        .resource_mut::<crate::ui_mail::MailPending>()
        .arm_refresh();
    c.frame(vec![
        vec![greeting(7)],
        vec![menu(NPC, 7)],
        vec![SessionEvent::ReceivedMail { seconds: 0.0 }],
    ]);
    assert_eq!(
        c.app.world().resource::<crate::ui_mail::MailOpen>().mailbox,
        None
    );
    assert_eq!(c.eval::<Option<i64>>("return HasNewMail()"), Some(1));
}

/// `SMSG_SHOW_BANK` from the banker whose window is open (`0x4f84d0(banker)`) closes it first,
/// with no test of the cell (`0x4f8507`), and opens it again; the stock `OnHide`'s
/// `CloseBankFrame()` (`BankFrame.xml:523`) re-enters the close of the session it hid and leaves
/// the new one open.
#[test]
fn a_bank_reopened_while_open_stays_open() {
    const BANKER: u64 = NPC;
    let _data = benilla_formats::wow_data_or_skip!();
    // The bag kit: the window's `OnHide` closes the bank's bags (`CloseBag`) before it calls
    // `CloseBankFrame()`.
    let mut c = client(
        crate::ui_script::test_ui::BAG_UI,
        &[(BANKER, EntityKind::Unit)],
    );
    c.frame(vec![vec![SessionEvent::ShowBank { banker: BANKER }]]);
    assert!(visible(&mut c, "BankFrame"));
    c.frame(vec![vec![SessionEvent::ShowBank { banker: BANKER }]]);
    assert_eq!(
        c.app.world().resource::<crate::ui_bank::BankOpen>().banker,
        Some(BANKER)
    );
    assert!(visible(&mut c, "BankFrame"));
}
