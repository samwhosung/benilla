//! The quest log getters as a listener reads them, and the quest log's events at their packets.
//! The getters read the live object and caches at the call: the title's `isComplete` and the
//! leaderboard read the row's slot in the player's descriptor (`0x4dfa41`, `0x4e01df`), the
//! coinage (`0x4df70b`), the held count of an item (`0x622130`) and a faction's standing
//! (`0x4d6370`); the timers read the clock the time response set (`0x4deef0`). The events fire
//! inside the handler that raises them: a template's record callbacks (`0x562c80`), a quest
//! update's toast and `QUEST_WATCH_UPDATE` (`0x5e5d61`), a standing message's rebuild (`0x4d58f3`)
//! and the time response's `QUEST_LOG_UPDATE` (`0x4def1b`).

use benilla_formats::FactionCatalog;
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;
use benilla_protocol::messages::{
    QuestDetails, QuestGiverList, QuestListEntry, QuestRequestItems, QuestRewardItem,
};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::{QuestPanel, UiScript};
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// The quest in slot 0: three wolves, five pelts, a standing of 3000 with Stormwind, 100 copper.
const QUEST: u32 = 50;
const WOLF: u32 = 40;
const PELT: u32 = 900;
const PELT_GUID: u64 = 0x4000_0000_0000_00F1;
const STORMWIND: (u32, u32) = (72, 19);
/// `UNIT_FIELD_BYTES_0` of a male human warrior, `PLAYER_FIELD_COINAGE`, `PLAYER_FIELD_PACK_SLOT_1`
/// and `OBJECT_FIELD_TYPE`.
const BYTES_0: u16 = 36;
const HUMAN_WARRIOR: u32 = 1 | (1 << 8);
const COINAGE: u16 = 1176;
const PACK_SLOT_1: u16 = 532;
const OBJECT_TYPE: u16 = 2;

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

    /// Fields written into the player's store, with no packet and no frame: the world as a
    /// packet's handler leaves it, which the VM's copies have not heard of.
    fn write_player(&mut self, pairs: &[(u16, u32)]) {
        let world = self.app.world_mut();
        let entity = world.resource::<crate::net::GuidIndex>().0[&ME];
        world
            .get_mut::<crate::net::ObjectStore>(entity)
            .expect("the player's store")
            .0
            .merge(ObjectFields::from_pairs(pairs));
    }

    /// The `isComplete` of the quest's row, as a listener asks.
    fn complete(&mut self, live: bool) -> Option<i64> {
        self.eval(
            live,
            "local _, _, _, _, _, c = GetQuestLogTitle(2) return c",
        )
    }
}

/// The catalog: Stormwind a human starts Friendly with (4000) under the Alliance header.
fn catalog() -> FactionCatalog {
    FactionCatalog::from_rows(Default::default(), Default::default())
        .with_reputation(469, 11, 1, 4000)
        .with_faction_text(469, 0, "Alliance", "")
        .with_reputation(STORMWIND.0, STORMWIND.1 as i32, 1, 4000)
        .with_faction_text(STORMWIND.0, 469, "Stormwind", "")
}

fn template() -> benilla_protocol::messages::QuestTemplate {
    let mut t = crate::ui_quest::test_template(QUEST, "Wolves and Pelts", 0);
    t.rep_objective_faction = STORMWIND.0;
    t.rep_objective_value = 3000;
    t.money = -100;
    t.objectives[0].creature_or_go = WOLF;
    t.objectives[0].required_count = 3;
    t.objectives[1].item_id = PELT;
    t.objectives[1].item_count = 5;
    t
}

/// The count word of a slot: three kills in the first counter.
const KILLED: u32 = 3;

/// The client in the world as a human warrior holding quest [`QUEST`] with every requirement
/// met, five pelts, 150 copper and Friendly with Stormwind; and a spy whose line for each quest
/// event carries its name and first argument.
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
    app.insert_resource(crate::target::Factions::from_catalog(catalog()));
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            QUEST_MONSTERS_KILLED = "%s slain: %d/%d"
            QUEST_ITEMS_NEEDED = "%s: %d/%d"
            ERR_QUEST_ADD_KILL_SII = "%s slain: %d/%d"
            ERR_QUEST_OBJECTIVE_COMPLETE_S = "%s (Complete)"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"QUEST_LOG_UPDATE", "QUEST_WATCH_UPDATE", "UI_INFO_MESSAGE",
                    "UPDATE_FACTION"}) do
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
    let world = app.world_mut();
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    crate::ui_quest::hold_for_test(world, GIVER);
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_creature(
            WOLF,
            Some(crate::names::CreatureRecord {
                name: "Timber Wolf".into(),
                subname: None,
                creature_type: 1,
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                pet_spell_list: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );
    world
        .resource_mut::<crate::ui_quest_log::QuestLog>()
        .insert_template(template());
    let mut pelt = crate::items::test_template("Wolf Pelt");
    pelt.quality = 1;
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(PELT, Some(pelt));
    crate::items::test_spawn_item(
        world,
        PELT_GUID,
        ObjectFields::from_pairs(&[
            (OBJECT_TYPE, 0x3),
            (3, PELT),
            (6, ME as u32),
            (7, (ME >> 32) as u32),
            (14, 5),
        ])
        .into_created(benilla_protocol::messages::ObjectType::Item),
        false,
    );
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    // The list as the server sends it ahead of the create: Stormwind met, Friendly.
    let mut list = vec![(0u8, 0i32); 64];
    list[11] = (0x09, 0);
    list[STORMWIND.1 as usize] = (0x01, 0);
    c.frame(vec![vec![SessionEvent::Reputations { standings: list }]]);
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (BYTES_0, HUMAN_WARRIOR),
            (COINAGE, 150),
            (PACK_SLOT_1, PELT_GUID as u32),
            (PACK_SLOT_1 + 1, (PELT_GUID >> 32) as u32),
            (FIELD_PLAYER_QUEST_LOG_1_1, QUEST),
            (FIELD_PLAYER_QUEST_LOG_1_1 + 1, KILLED),
        ],
    )]]);
    c.frame(vec![]);
    c.frame(vec![]);
    c.seen();
    c
}

const SLOT_STATE: u16 = FIELD_PLAYER_QUEST_LOG_1_1 + 1;

/// The row's `isComplete` and its lines follow the world at the call, with no push between: each
/// input written into the world flips the turn-in test (`0x4df580`) at once.
#[test]
fn the_turn_in_test_reads_the_world_at_the_call() {
    let mut c = client();
    assert_eq!(c.complete(true), Some(1), "every requirement met");

    // A counter a packet has not yet announced (`0x4e0302`).
    c.write_player(&[(SLOT_STATE, 2)]);
    assert_eq!(c.complete(true), None, "two of three wolves");
    c.write_player(&[(SLOT_STATE, KILLED)]);
    assert_eq!(c.complete(true), Some(1));

    // The failed bit of the state byte (`0x4dfa61`).
    c.write_player(&[(SLOT_STATE, KILLED | (2 << 24))]);
    assert_eq!(c.complete(true), Some(-1));
    c.write_player(&[(SLOT_STATE, KILLED)]);

    // The coinage a negative money asks (`0x4df70b`, `0x4df72e`).
    c.write_player(&[(COINAGE, 99)]);
    assert_eq!(c.complete(true), None, "99 of 100 copper");
    c.write_player(&[(COINAGE, 150)]);

    // The held count (`0x4df778`), a stack a handler has changed.
    let world = c.app.world_mut();
    let pelts = world.resource::<crate::net::GuidIndex>().0[&PELT_GUID];
    world
        .get_mut::<crate::net::ObjectStore>(pelts)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[(14, 4)]));
    assert_eq!(c.complete(true), None, "four of five pelts");
    let line = "local t, _, f = GetQuestLogLeaderBoard(2, 2) return t .. '|' .. tostring(f)";
    assert_eq!(c.eval::<String>(true, line), "Wolf Pelt: 4/5|nil");
    // A broken copy is not counted (`0x6224fc`-`0x62250e`).
    let world = c.app.world_mut();
    world
        .get_mut::<crate::net::ObjectStore>(pelts)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[(14, 5), (46, 0), (47, 20)]));
    assert_eq!(c.eval::<String>(true, line), "Wolf Pelt: 0/5|nil");
    let world = c.app.world_mut();
    world
        .get_mut::<crate::net::ObjectStore>(pelts)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[(46, 20)]));
    assert_eq!(c.complete(true), Some(1), "whole again");

    // The standing, the base for a human plus the wire's (`0x4d6370`).
    c.app
        .world_mut()
        .resource_mut::<crate::net::Reputations>()
        .set_standing(STORMWIND.1 as usize, -1500);
    assert_eq!(c.complete(true), None, "2500 of 3000");
}

/// The leaderboard's line reads the counter in the world (`0x4e0302`), and the row's slot is the
/// one the list placed it in.
#[test]
fn a_lines_count_is_the_slots_at_the_call() {
    let mut c = client();
    c.frame(vec![]);
    let line =
        "local t, k, f = GetQuestLogLeaderBoard(1, 2) return t .. '|' .. k .. '|' .. tostring(f)";
    assert_eq!(
        c.eval::<String>(true, line),
        "Timber Wolf slain: 3/3|monster|1"
    );
    c.write_player(&[(SLOT_STATE, 1)]);
    assert_eq!(
        c.eval::<String>(true, line),
        "Timber Wolf slain: 1/3|monster|nil"
    );
    assert_eq!(
        c.eval::<String>(false, line),
        "... slain: 3/3|monster|1",
        "with no host the copies answer, and the VM holds no creature name"
    );
}

/// The timers read the clock the time response set: a sample whose local second is the server's is
/// the reference's zero offset, stored as 1 (`0x4def02`), so a countdown reads a second more, and
/// a later skewed sample clears it. The deadline is the descriptor's third dword (`0x4e0905`).
#[test]
fn the_timers_count_by_the_clock_the_world_holds() {
    let mut c = client();
    c.write_player(&[(FIELD_PLAYER_QUEST_LOG_1_1 + 2, 4_000_000_000)]);
    let left = |c: &mut Client, server: u32, local: i64| {
        c.app
            .world_mut()
            .resource_mut::<crate::net::ServerWallClock>()
            .sample(server, local);
        c.eval::<i64>(true, "return GetQuestTimers()")
    };
    let skewed = left(&mut c, 1_000_000, 1_000_005);
    let zero = left(&mut c, 1_000_000, 1_000_000);
    assert_eq!(zero, skewed + 1, "a second more");
    assert_eq!(left(&mut c, 1_000_000, 999_999), skewed);
    assert_eq!(
        c.eval::<i64>(true, "return GetQuestLogTimeLeft()"),
        skewed,
        "synced, the selection is slot 0's"
    );
}

/// A toast, a time response and a standing message in one frame each signal at their packets
/// (`0x5e5d61`, `0x4def1b`, `0x4d58f3`): the log's events and the reputation's `UPDATE_FACTION`
/// interleave as the packets came, not the log's after the frame's feeds.
#[test]
fn the_logs_events_fire_at_their_packets_places() {
    let mut c = client();
    let kill = |count| {
        vec![SessionEvent::QuestObjectiveKill {
            quest_id: QUEST,
            entry: WOLF,
            count,
            required: 3,
        }]
    };
    let sync = |t| vec![SessionEvent::ServerUnixTime { unix_time: t }];
    // 5100 more standing: Friendly to Honored, a rank moved.
    let rank = |standing| {
        vec![SessionEvent::ReputationDelta {
            standings: vec![(STORMWIND.1, standing)],
        }]
    };
    c.frame(vec![
        sync(1_000_000),
        rank(5100),
        kill(1),
        rank(1000),
        sync(1_000_001),
    ]);
    let seen = c.seen();
    let names: Vec<&str> = seen
        .iter()
        .map(|e| e.split(':').next().unwrap_or(""))
        .collect();
    assert_eq!(
        names,
        [
            // The time response's signal, ahead of the next packet.
            "QUEST_LOG_UPDATE",
            // The standing message: the rank move rebuilds the log, then the message signals.
            "QUEST_LOG_UPDATE",
            "UPDATE_FACTION",
            // The quest update's toast and its watch update.
            "UI_INFO_MESSAGE",
            "QUEST_WATCH_UPDATE",
            // The second standing message moved a rank back.
            "QUEST_LOG_UPDATE",
            "UPDATE_FACTION",
            "QUEST_LOG_UPDATE",
        ],
        "{seen:?}"
    );
}

/// The client with quest 52 in slot 1, selected, whose text reads world state 2000 and the
/// player's name, and a listener that notes `GetQuestLogQuestText` at each `QUEST_LOG_UPDATE`.
fn holding_a_macro_quest() -> Client {
    let mut c = client();
    let mut t = crate::ui_quest::test_template(52, "Hold the Line", 0);
    t.details = "Hold for $2000w waves, $N.".into();
    t.objectives_text = "$2000W left.".into();
    let world = c.app.world_mut();
    world
        .resource_mut::<crate::ui_quest_log::QuestLog>()
        .insert_template(t);
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ME, "Tri".into(), None);
    c.frame(vec![vec![SessionEvent::ObjectValues {
        guid: ME,
        fields: ObjectFields::from_pairs(&[(FIELD_PLAYER_QUEST_LOG_1_1 + 3, 52)]),
    }]]);
    c.frame(vec![]);
    c.eval::<()>(
        false,
        r#"for i = 1, GetNumQuestLogEntries() do
            if GetQuestLogTitle(i) == "Hold the Line" then SelectQuestLogEntry(i) end
        end
        Texts = {}
        local g = CreateFrame("Frame")
        g:RegisterEvent("QUEST_LOG_UPDATE")
        g:SetScript("OnEvent", function()
            local d, o = GetQuestLogQuestText()
            table.insert(Texts, tostring(d) .. "|" .. tostring(o))
        end)"#,
    );
    c.seen();
    c
}

fn texts_seen(c: &mut Client) -> Vec<String> {
    c.eval(false, "local t = Texts Texts = {} return t")
}

fn world_state(value: u32) -> SessionEvent {
    SessionEvent::WorldStates {
        scope: None,
        states: vec![(2000, value)],
    }
}

/// `GetQuestLogQuestText` expands its text's `$` macros at the call (`0x4dff89`, `0x4dffae` →
/// `0x506f70`): a listener fired at a packet, with the packets after it not yet in, reads the
/// world state and the name as its packet left them, not as a push before it did.
#[test]
fn a_quest_log_listener_reads_its_packets_quest_text() {
    let mut c = holding_a_macro_quest();
    for value in [3, 7] {
        let world = c.app.world_mut();
        crate::net::handlers::dispatch(world, vec![world_state(value)]);
        crate::net::handlers::dispatch(world, sync());
        crate::lanes::fire_in_order(world);
        assert_eq!(
            texts_seen(&mut c),
            [format!("Hold for {value} waves, Tri.|{value} left.")]
        );
    }
    let world = c.app.world_mut();
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ME, "Tria".into(), None);
    crate::net::handlers::dispatch(world, sync());
    crate::lanes::fire_in_order(world);
    assert_eq!(texts_seen(&mut c), ["Hold for 7 waves, Tria.|7 left."]);
}

/// The quest-slot watcher's accept line shows inside the update packet that filled the slot
/// (`0x5ddd80`, `0x5dde66`), at its place: a later packet of the same frame that empties the slot
/// again takes nothing back. The line's listener reads the log as it stood when the slot's watcher
/// ran, ahead of the log's own watcher on the same block (`0x4de430`), so without the quest.
#[test]
fn the_accept_line_shows_at_the_packet_that_fills_the_slot() {
    let mut c = client();
    c.app
        .world_mut()
        .resource_mut::<crate::ui_quest_log::QuestLog>()
        .insert_template(crate::ui_quest::test_template(51, "Kobolds", 0));
    c.eval::<()>(
        false,
        r#"ERR_QUEST_ACCEPTED_S = "Quest accepted: %s"
        Lines = {}
        local g = CreateFrame("Frame")
        g:RegisterEvent("CHAT_MSG_SYSTEM")
        g:SetScript("OnEvent", function()
            local _, n = GetNumQuestLogEntries()
            table.insert(Lines, arg1 .. "|" .. n)
        end)"#,
    );
    let slot = |quest: u32| {
        vec![SessionEvent::ObjectValues {
            guid: ME,
            fields: ObjectFields::from_pairs(&[(FIELD_PLAYER_QUEST_LOG_1_1 + 3, quest)]),
        }]
    };
    c.frame(vec![slot(51), slot(0)]);
    let lines: Vec<String> = c.eval(false, "local l = Lines Lines = {} return l");
    assert_eq!(lines, ["Quest accepted: Kobolds|1"]);
}

/// The client with a second quest in slot 1 whose record is not cached, so the rebuild awaits it,
/// and a listener that notes the log's row count at each `QUEST_LOG_UPDATE`.
fn awaiting_a_record() -> Client {
    let mut c = client();
    c.frame(vec![vec![SessionEvent::ObjectValues {
        guid: ME,
        fields: ObjectFields::from_pairs(&[(FIELD_PLAYER_QUEST_LOG_1_1 + 3, 51)]),
    }]]);
    c.eval::<()>(
        false,
        r#"Rows = {}
        local g = CreateFrame("Frame")
        g:RegisterEvent("QUEST_LOG_UPDATE")
        g:SetScript("OnEvent", function() table.insert(Rows, (GetNumQuestLogEntries())) end)"#,
    );
    c.seen();
    c
}

fn rows_seen(c: &mut Client) -> Vec<i64> {
    c.eval(false, "local r = Rows Rows = {} return r")
}

fn sync() -> Vec<SessionEvent> {
    vec![SessionEvent::ServerUnixTime {
        unix_time: 1_000_000,
    }]
}

/// A template that lands inside its packet runs the callbacks the rebuild left on it
/// (`0x4de8b0`): the listener of its `QUEST_LOG_UPDATE` finds the quest's row, and the time
/// response's, whichever side of the template its packet falls, finds the log as that packet left
/// it.
#[test]
fn a_template_lands_with_its_callbacks_at_its_packet() {
    let landing = || {
        vec![SessionEvent::QuestTemplate(Box::new(
            crate::ui_quest::test_template(51, "Second", 0),
        ))]
    };
    let mut c = awaiting_a_record();
    let before: i64 = c.eval(true, "return GetNumQuestLogEntries()");
    c.frame(vec![landing(), sync()]);
    assert_eq!(rows_seen(&mut c), [before + 1, before + 1]);
    assert_eq!(
        c.eval::<i64>(true, "return GetNumQuestLogEntries()"),
        before + 1
    );

    let mut c = awaiting_a_record();
    c.frame(vec![sync(), landing()]);
    assert_eq!(
        rows_seen(&mut c),
        [before, before + 1],
        "the clock's signal ahead of the landing finds the log without the quest"
    );
}

/// A template the server refuses runs no rebuild (`0x4de8bc`): the log stays as it was, and only
/// the time response signals.
#[test]
fn a_refused_template_leaves_the_log_and_signals_nothing() {
    let mut c = awaiting_a_record();
    let before: i64 = c.eval(true, "return GetNumQuestLogEntries()");
    let refused = crate::ui_quest::test_template(51, "", 0);
    c.frame(vec![
        vec![SessionEvent::QuestTemplate(Box::new(refused))],
        sync(),
    ]);
    assert_eq!(rows_seen(&mut c), [before], "the clock's signal alone");
    assert_eq!(
        c.eval::<i64>(true, "return GetNumQuestLogEntries()"),
        before
    );
}

/// The entries that run Lua come in with the host lent (`tick_script`, the lanes' firings, the
/// entry load through `lent_by_world` all run inside `HostedVm::run`, and a bare world with none
/// is a test's): an `OnUpdate` in the frame's tick and a listener on the quest events' lane read
/// the live clock and slots, which no copy holds any more.
#[test]
fn the_ticks_and_the_lanes_listeners_read_live() {
    let mut c = client();
    c.write_player(&[(FIELD_PLAYER_QUEST_LOG_1_1 + 2, 4_000_000_000)]);
    c.eval::<()>(
        false,
        r#"Ticked, Heard = nil, nil
        local f = CreateFrame("Frame")
        f:SetScript("OnUpdate", function() Ticked = GetQuestTimers() end)
        local g = CreateFrame("Frame")
        g:RegisterEvent("QUEST_LOG_UPDATE")
        g:SetScript("OnEvent", function() Heard = GetQuestTimers() end)"#,
    );
    c.app
        .world_mut()
        .resource_mut::<crate::net::ServerWallClock>()
        .sample(1_000_000, 1_000_005);
    c.frame(vec![sync()]);
    // The frame's tick, as `tick_script` runs it: inside `HostedVm::run`.
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| s.expect("a VM").tick(0.1));
    let (ticked, heard): (Option<i64>, Option<i64>) = c.eval(false, "return Ticked, Heard");
    assert!(ticked.is_some(), "the OnUpdate found the timer");
    assert!(heard.is_some(), "the lane's listener found the timer");
}

/// The LFG window's quest list is every quest in the log's slots (`0x4e8c1e`-`0x4e8c32`), which
/// the slot watcher rebuilds at every slot change (`0x5dde98`, `0x4e8be0`) whether or not the
/// quest log's own list waits on a record: after a refused template the list follows a new slot.
#[test]
fn the_lfg_quest_list_follows_the_slots_after_a_refused_template() {
    let mut c = awaiting_a_record();
    let refused = crate::ui_quest::test_template(51, "", 0);
    c.frame(vec![vec![SessionEvent::QuestTemplate(Box::new(refused))]]);
    let listed = "local t = {GetLFGTypeEntries(3)} return table.getn(t)";
    assert_eq!(c.eval::<i64>(true, listed), 2, "quests 50 and 51");
    c.frame(vec![vec![SessionEvent::ObjectValues {
        guid: ME,
        fields: ObjectFields::from_pairs(&[(FIELD_PLAYER_QUEST_LOG_1_1 + 6, QUEST)]),
    }]]);
    for live in [true, false] {
        assert_eq!(c.eval::<i64>(live, listed), 3, "live {live}");
    }
}

// ── The questgiver panel ─────────────────────────────────────────────────────────────────────

const GIVER: u64 = 0xF130_0000_0000_0042;

fn quest_row(quest_id: u32, icon: u32, level: u32, title: &str) -> QuestListEntry {
    QuestListEntry {
        quest_id,
        icon,
        level,
        title: title.into(),
    }
}

fn greeting() -> SessionEvent {
    SessionEvent::QuestGreeting(QuestGiverList {
        npc: GIVER,
        greeting: "Well met.".into(),
        emote_delay: 0,
        emote: 0,
        quests: vec![
            quest_row(QUEST, 3, 12, "Wolves and Pelts"),
            quest_row(51, 2, 9, "Kobolds"),
        ],
    })
}

fn detail() -> SessionEvent {
    SessionEvent::QuestDetail(QuestDetails {
        npc: GIVER,
        quest_id: 51,
        title: "Kobolds".into(),
        details: "Clear the mine, $N.".into(),
        objectives: String::new(),
        auto_finish: 0,
        choices: Vec::new(),
        rewards: Vec::new(),
        money: 0,
        reward_spell: 0,
    })
}

fn progress(pelts: u32) -> SessionEvent {
    SessionEvent::QuestProgress(QuestRequestItems {
        npc: GIVER,
        quest_id: QUEST,
        title: "Wolves and Pelts".into(),
        request_text: "The pelts?".into(),
        emote: 0,
        close_on_cancel: 0,
        required_money: 0,
        required_items: vec![QuestRewardItem {
            item_id: PELT,
            count: pelts,
            display_id: 1,
        }],
        is_complete: true,
    })
}

/// The questgiver getters with the host lent read the panel's cells as the packets' handlers left
/// them, with no push between: each text its own buffer (`0xbde288`, `0xbdf0f8`), expanded at its
/// packet (`0x500cc9`); the greeting's pools past the panel, nil past their count (`0x501b05`);
/// and `IsQuestCompletable` the packet's flag with the pelts held at the call (`0x501230`).
#[test]
fn the_giver_getters_read_the_panel_cells_at_the_call() {
    let mut c = client();
    c.app
        .world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ME, "Tri".into(), None);
    for ev in [greeting(), detail(), progress(5)] {
        crate::net::handlers::dispatch(c.app.world_mut(), vec![ev]);
    }
    let texts = "return GetQuestText(), GetProgressText(), GetRewardText()";
    assert_eq!(
        c.eval::<(String, String, String)>(true, texts),
        (
            "Clear the mine, Tri.".into(),
            "The pelts?".into(),
            String::new()
        )
    );
    let copy: (String, String, String) = c.eval(false, texts);
    assert_eq!(copy.1, "", "the VM's copy has heard of no panel");
    let titles = "return GetActiveTitle(1), GetAvailableTitle(1), GetActiveTitle(2), \
                  GetAvailableLevel(1)";
    assert_eq!(
        c.eval::<(Option<String>, Option<String>, Option<String>, i64)>(true, titles),
        (
            Some("Wolves and Pelts".into()),
            Some("Kobolds".into()),
            None,
            9
        )
    );
    let completable = "return IsQuestCompletable()";
    assert_eq!(
        c.eval::<Option<i64>>(true, completable),
        Some(1),
        "five held"
    );
    crate::net::handlers::dispatch(c.app.world_mut(), vec![progress(6)]);
    assert_eq!(
        c.eval::<Option<i64>>(true, completable),
        None,
        "five of six held, whatever the packet's flag"
    );
}

/// A panel's event sits on the questgiver lane at its handler's place and fires there, and a
/// refusal's close in the same frame runs behind it: `QUEST_DETAIL`, the line, then
/// `QUEST_FINISHED` (`0x5dbd50`, `0x5dbca0`, `0x501217`).
#[test]
fn a_panel_and_its_refusal_fire_on_the_lane_at_their_packets() {
    let mut c = client();
    c.eval::<()>(
        false,
        r#"
        ERR_QUEST_ALREADY_ON = "already on"
        local f = CreateFrame("Frame")
        for _, e in ipairs({"QUEST_DETAIL", "QUEST_FINISHED", "CHAT_MSG_SYSTEM"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function() table.insert(Seen, event) end)
        "#,
    );
    let before = crate::lanes::stamp();
    crate::net::handlers::dispatch(c.app.world_mut(), vec![detail()]);
    let after = crate::lanes::stamp();
    let stamped = c
        .app
        .world()
        .resource::<crate::ui_quest::QuestGiverLane>()
        .stamped();
    assert!(
        matches!(
            stamped.as_slice(),
            [(at, crate::ui_quest::QuestGiverEvent::Panel(QuestPanel::Detail, GIVER))]
                if before < *at && *at < after
        ),
        "stamped inside the handler: {stamped:?}"
    );
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::QuestGiverInvalid { reason: 13 }],
    );
    assert!(c.seen().is_empty(), "nothing fires ahead of the drain");
    crate::lanes::fire_in_order(c.app.world_mut());
    assert_eq!(
        c.seen(),
        ["QUEST_DETAIL", "CHAT_MSG_SYSTEM", "QUEST_FINISHED"]
    );
}

/// A panel's event fires once per panel, and the giver's name, which the frame looks up with a
/// callback at that event (`0x5171b8`), signals nothing when it lands: no questgiver event fires
/// off its answer (`QUEST_ITEM_UPDATE` only off an item callback, `0x501903`).
#[test]
fn the_givers_name_landing_fires_no_quest_event() {
    const SHARER: u64 = 0x99;
    let mut c = client();
    c.eval::<()>(
        false,
        r#"
        local f = CreateFrame("Frame")
        for _, e in ipairs({"QUEST_GREETING", "QUEST_DETAIL", "QUEST_ITEM_UPDATE"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function() table.insert(Seen, event) end)
        "#,
    );
    let mut offer = detail();
    if let SessionEvent::QuestDetail(d) = &mut offer {
        d.npc = SHARER;
    }
    crate::ui_quest::hold_for_test(c.app.world_mut(), SHARER);
    crate::net::handlers::dispatch(c.app.world_mut(), vec![offer]);
    crate::lanes::fire_in_order(c.app.world_mut());
    assert_eq!(c.seen(), ["QUEST_DETAIL"]);
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::PlayerName {
            guid: SHARER,
            name: "Sharer".into(),
            race: 1,
            gender: 0,
            class: 1,
        }],
    );
    let lane = c
        .app
        .world()
        .resource::<crate::ui_quest::QuestGiverLane>()
        .stamped();
    assert!(lane.is_empty(), "the name queues nothing: {lane:?}");
    crate::lanes::fire_in_order(c.app.world_mut());
    assert!(c.seen().is_empty(), "a landed name fires nothing");
}
