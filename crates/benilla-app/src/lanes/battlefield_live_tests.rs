//! The battleground getters as a listener reads them, and the battleground's events at their
//! packets. The getters read the module's globals at the call: the queue slots (`0x4ab4a0`), the
//! list (`0x4ab080`, `0x4ab1f0`), the board (`0x4ab9a0`, `0x4ab9d0`, `0x4abc40`) and the position
//! arrays, filtered by the party and the raid roster as they stand (`0x4abf00`, `0x4ac00b`). The
//! events fire inside the handler that raises them: the list's `BATTLEFIELDS_SHOW` (`0x4aa7d1`),
//! the status's `UPDATE_BATTLEFIELD_SCORE` and `UPDATE_BATTLEFIELD_STATUS` (`0x4aaa5a`,
//! `0x4aab05`), the log's rebuild (`0x4aacac`), and a name answer's callbacks (`0x4a9ef0`,
//! `0x4aaef0`, `0x4ac190`).

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{
    BattlefieldList, BattlefieldPosition, BattlefieldPositions, BattlefieldStatus, PvpLogData,
    PvpLogRow,
};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
const WSG: u32 = 489;
/// `UNIT_FIELD_BYTES_0` of a male human warrior.
const BYTES_0: u16 = 36;
const HUMAN_WARRIOR: u32 = 1 | (1 << 8);
/// A teammate, a scorer and a newcomer whose names the cache lacks.
const TEAMMATE: u64 = 0x31;
const SCORER: u64 = 0x32;
const NEWCOMER: u64 = 0x33;

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    _sent: Receiver<ClientCommand>,
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

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        self.eval(false, "local s = Seen Seen = {} return s")
    }

    /// `packets` run through their handlers with no frame: the world as the handlers leave it,
    /// which no feed has pushed.
    fn handle(&mut self, packets: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), packets);
    }
}

/// The client in the world as a human warrior, and a spy whose line for each battleground event
/// and chat line carries its name and first argument.
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
            ERR_BG_PLAYER_JOINED_SS = "%s has joined"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"BATTLEFIELDS_SHOW", "UPDATE_BATTLEFIELD_STATUS",
                    "UPDATE_BATTLEFIELD_SCORE", "WORLD_MAP_NAME_UPDATE", "CHAT_MSG_SYSTEM",
                    "CHAT_MSG_MONSTER_SAY"}) do
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
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (BYTES_0, HUMAN_WARRIOR),
        ],
    )]]);
    c.frame(vec![]);
    c.frame(vec![]);
    c.seen();
    c
}

fn status(slot: u32, status: u32) -> SessionEvent {
    SessionEvent::BattlefieldStatus(BattlefieldStatus {
        slot,
        map_id: WSG,
        bracket: 1,
        instance_id: 3,
        status,
        time_ms: None,
        in_progress: (status == 3).then_some((0, 0)),
        queued: (status == 1).then_some((30_000, 0)),
    })
}

fn list(instances: Vec<u32>) -> SessionEvent {
    SessionEvent::BattlefieldList(BattlefieldList {
        battlemaster: 0,
        map_id: WSG,
        bracket: 1,
        instances,
    })
}

fn log(ended: bool, guids: &[u64]) -> SessionEvent {
    SessionEvent::PvpLogData(PvpLogData {
        ended,
        winner: ended.then_some(1),
        rows: guids
            .iter()
            .map(|&guid| PvpLogRow {
                guid,
                rank: 0,
                killing_blows: 2,
                honorable_kills: 0,
                deaths: 0,
                honor_gained: 0,
                stats: Vec::new(),
            })
            .collect(),
    })
}

fn positions(guids: &[u64]) -> SessionEvent {
    SessionEvent::BattlefieldPositions(BattlefieldPositions {
        players: guids
            .iter()
            .map(|&guid| BattlefieldPosition {
                guid,
                x: 0.0,
                y: 0.0,
            })
            .collect(),
        carrier: None,
    })
}

fn named(guid: u64, name: &str) -> SessionEvent {
    SessionEvent::PlayerName {
        guid,
        name: name.into(),
        race: 2,
        gender: 0,
        class: 1,
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

/// Each getter reads what the handlers wrote at the call, with no push between: the slot, the
/// list, the board and the positions, the last filtered by the party as it stands; with no host
/// the VM's copies, which nothing pushes, answer as they were.
#[test]
fn the_getters_read_the_world_at_the_call() {
    let mut c = client();
    c.handle(vec![status(0, 1), list(vec![4, 9])]);
    assert_eq!(
        c.eval::<String>(true, "return (GetBattlefieldStatus(1))"),
        "queued"
    );
    assert_eq!(
        c.eval::<i64>(true, "return GetBattlefieldEstimatedWaitTime(1)"),
        30_000
    );
    assert_eq!(
        c.eval::<String>(false, "return (GetBattlefieldStatus(1))"),
        "none",
        "the VM's copy"
    );
    assert_eq!(
        c.eval::<(i64, i64)>(
            true,
            "return GetNumBattlefields(), GetBattlefieldInstanceInfo(2)"
        ),
        (2, 9)
    );
    c.eval::<()>(true, "SetSelectedBattlefield(2)");
    c.handle(vec![list(vec![9, 4])]);
    assert_eq!(
        c.eval::<i64>(true, "return GetSelectedBattlefield()"),
        1,
        "the selection follows its instance through the list the call finds"
    );

    c.handle(vec![named(SCORER, "Rik"), log(true, &[SCORER])]);
    assert_eq!(
        c.eval::<(i64, i64, String)>(
            true,
            "local n = GetNumBattlefieldScores() return n, GetBattlefieldWinner(), \
             (GetBattlefieldScore(1))"
        ),
        (1, 1, "Rik".into())
    );
    assert!(c.eval::<bool>(false, "return GetBattlefieldWinner() == nil"));

    c.handle(vec![positions(&[TEAMMATE])]);
    assert_eq!(
        c.eval::<i64>(true, "return GetNumBattlefieldPositions()"),
        1
    );
    // The teammate joins our party: the getter skips a party slot at the call (`0x4ac00b`).
    {
        let mut group = c
            .app
            .world_mut()
            .resource_mut::<crate::ui_party::GroupState>();
        group.in_group = true;
        group.members = vec![benilla_protocol::messages::GroupMemberEntry {
            name: "Mate".into(),
            guid: TEAMMATE,
            status: 1,
            flags: 0,
        }];
    }
    assert_eq!(
        c.eval::<i64>(true, "return GetNumBattlefieldPositions()"),
        0
    );
    assert_eq!(
        c.eval::<i64>(false, "return GetNumBattlefieldPositions()"),
        0
    );
}

/// A list, two statuses and a log in one frame each signal at their packets, between two
/// monster lines on the chat lane: the status 3's filter reset rebuilds the board and signals
/// before its status event (`0x4aaa5a`, `0x4aab05`), not after the frame's feeds.
#[test]
fn the_events_fire_at_their_packets_places() {
    let mut c = client();
    c.frame(vec![
        vec![list(vec![4])],
        vec![boar_says("one")],
        vec![status(0, 1)],
        vec![boar_says("two")],
        vec![status(0, 3)],
        vec![log(false, &[])],
    ]);
    assert_eq!(
        c.seen(),
        [
            "BATTLEFIELDS_SHOW:nil",
            "CHAT_MSG_MONSTER_SAY:one",
            "UPDATE_BATTLEFIELD_STATUS:nil",
            "CHAT_MSG_MONSTER_SAY:two",
            "UPDATE_BATTLEFIELD_SCORE:nil",
            "UPDATE_BATTLEFIELD_STATUS:nil",
            "UPDATE_BATTLEFIELD_SCORE:nil",
        ]
    );
}

/// A name answer runs the battleground's callbacks inside its packet: each map callback a
/// getter's call left signals `WORLD_MAP_NAME_UPDATE`, a refusal's too (`0x4ac190`); the board's
/// last pending name rebuilds it (`0x4aa580`); a joined notice shows its line (`0x4aaef0`).
#[test]
fn a_name_answer_runs_the_callbacks_at_its_packet() {
    let mut c = client();
    c.handle(vec![
        positions(&[TEAMMATE]),
        log(false, &[SCORER]),
        SessionEvent::BattlegroundPlayer {
            guid: NEWCOMER,
            joined: true,
        },
    ]);
    c.eval::<()>(
        true,
        "for _ = 1, 3 do GetBattlefieldPosition(1) end GetBattlefieldPosition(2)",
    );
    c.frame(vec![
        vec![boar_says("one")],
        vec![named(TEAMMATE, "Mate")],
        vec![boar_says("two")],
        vec![named(SCORER, "")],
        vec![named(NEWCOMER, "Newt")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:one",
            "WORLD_MAP_NAME_UPDATE:nil",
            "WORLD_MAP_NAME_UPDATE:nil",
            "WORLD_MAP_NAME_UPDATE:nil",
            "CHAT_MSG_MONSTER_SAY:two",
            "UPDATE_BATTLEFIELD_SCORE:nil",
            "CHAT_MSG_SYSTEM:Newt has joined",
        ]
    );
    c.frame(vec![vec![named(TEAMMATE, "Mate")]]);
    assert_eq!(c.seen(), Vec::<String>::new(), "the callbacks are spent");
}
