//! The order the interface hears what the packets raise. The reference's poll takes the queued
//! packets in order (`0x5384d0`) and runs each handler whole (`0x537aa0`); a handler's events fire
//! inside it (`0x703e50`), the chat lines' inside the line function (`0x496720` → `0x49a870` →
//! `0x49ac9a`), and a line held for its sender's name inside the name's answer (`0x49ccc0`).

use benilla_protocol::messages::ChatMessage;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::field_watch::FieldNotice;
use crate::net::handlers::NetHandlerApp;

/// The notice lane a test handler writes.
type Notices<'w> = ResMut<'w, crate::field_watch::NoticeLane>;

/// The frame's firing of what the packets queued, after the frame's pushes.
fn fire_frame(world: &mut World) {
    super::fire_in_order(world);
}

/// The field notices alone, as their firing runs them.
fn fire_notices(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(crate::ui_unit::field_events::fire_field_notices)
        .unwrap();
}

/// A sender whose name the cache lacks until its answer.
const STRANGER: u64 = 0x51;
/// A sender the cache knows.
const FRIEND: u64 = 0x52;

/// Four packets of four kinds, each handled by its own test handler.
fn p1() -> SessionEvent {
    SessionEvent::LoggedOut
}
fn p2() -> SessionEvent {
    SessionEvent::LoginQueued {
        position: None,
        realm: None,
    }
}
fn p3() -> SessionEvent {
    SessionEvent::LogoutCancelled
}
fn p4() -> SessionEvent {
    SessionEvent::LogoutResponse {
        reason: 0,
        instant: false,
    }
}

fn whisper(from: u64, text: &str) -> ChatMessage {
    ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_WHISPER,
        language: 0,
        sender_guid: from,
        target_guid: 0,
        sender_name: None,
        channel: None,
        text: text.into(),
        chat_tag: 0,
    }
}

/// The name answer for [`STRANGER`], as the server sends it.
fn stranger_named() -> SessionEvent {
    SessionEvent::PlayerName {
        guid: STRANGER,
        name: "Stranger".into(),
        race: 1,
        gender: 0,
        class: 1,
    }
}

/// A client with the unit plugin, the chat drain's reads, the name handlers and a VM whose spy
/// records every event it hears, `CHAT_MSG_*` with its text.
fn client(handlers: impl FnOnce(&mut App)) -> App {
    let mut app = App::new();
    app.add_plugins(crate::ui_unit::UiUnitPlugin);
    crate::ui_chat::add_chat_lane(&mut app);
    crate::names::net::register(&mut app);
    handlers(&mut app);
    let world = app.world_mut();
    let (tx, _rx) = crossbeam_channel::unbounded();
    world.insert_resource(crate::net::NetCommands(tx));
    crate::ui_chat::insert_drain_reads(world);
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(FRIEND, "Friend".into(), None);
    let s = benilla_ui::script::UiScript::new().unwrap();
    s.run(
        r#"
        ERR_EXHAUSTION_NORMAL = "You feel normal."
        Heard = {}
        local f = CreateFrame("Frame")
        f:RegisterAllEvents()
        f:SetScript("OnEvent", function()
            if string.find(event, "^CHAT_MSG_") then
                table.insert(Heard, event .. ":" .. tostring(arg1))
            else
                table.insert(Heard, event)
            end
        end)
    "#,
    )
    .unwrap();
    world.insert_non_send_resource(s);
    app
}

/// What the spy heard since the last call.
fn heard(app: &mut App) -> Vec<String> {
    let s = app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>();
    let out = s.eval::<Vec<String>>("return Heard").unwrap();
    s.run("Heard = {}").unwrap();
    out
}

/// One frame: the packets through the handler table, then the firing.
fn frame(app: &mut App, packets: Vec<SessionEvent>) -> Vec<String> {
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, packets);
    fire_frame(world);
    heard(app)
}

fn enter_then_regen(_: In<SessionEvent>, log: Res<crate::ui_chat::ChatLog>, mut n: Notices) {
    log.push_combat_edge(true);
    n.write(FieldNotice::Global {
        id: crate::field_watch::id::PLAYER_REGEN_DISABLED,
    });
}
fn unghost_then_leave(_: In<SessionEvent>, log: Res<crate::ui_chat::ChatLog>, mut n: Notices) {
    n.write(FieldNotice::Global {
        id: crate::field_watch::id::PLAYER_UNGHOST,
    });
    log.push_combat_edge(false);
}
fn enter(_: In<SessionEvent>, log: Res<crate::ui_chat::ChatLog>) {
    log.push_combat_edge(true);
}
fn regen(_: In<SessionEvent>, mut n: Notices) {
    n.write(FieldNotice::Global {
        id: crate::field_watch::id::PLAYER_REGEN_DISABLED,
    });
}
fn rest(_: In<SessionEvent>, mut n: Notices) {
    n.write(FieldNotice::RestState { old: 1, new: 2 });
}
fn stranger_whispers(_: In<SessionEvent>, log: Res<crate::ui_chat::ChatLog>) {
    log.push_wire(whisper(STRANGER, "psst"));
}
fn friend_whispers(_: In<SessionEvent>, log: Res<crate::ui_chat::ChatLog>) {
    log.push_wire(whisper(FRIEND, "hi"));
}

/// Each handler runs whole, in the packets' order (`0x5384d0`, `0x537aa0`): what one handler
/// raises, a chat item or a field notice, fires in the order it raised them, and all of it before
/// the next packet's.
#[test]
fn each_handlers_events_fire_whole_in_the_packets_order() {
    let mut app = client(|app| {
        app.net_handler(SessionEventKind::LoggedOut, enter_then_regen)
            .net_handler(SessionEventKind::LoginQueued, unghost_then_leave);
    });
    assert_eq!(
        frame(&mut app, vec![p1(), p2()]),
        [
            "PLAYER_ENTER_COMBAT",
            "PLAYER_REGEN_DISABLED",
            "PLAYER_UNGHOST",
            "PLAYER_LEAVE_COMBAT"
        ]
    );
    assert_eq!(
        frame(&mut app, vec![p2(), p1()]),
        [
            "PLAYER_UNGHOST",
            "PLAYER_LEAVE_COMBAT",
            "PLAYER_ENTER_COMBAT",
            "PLAYER_REGEN_DISABLED"
        ]
    );
}

/// A line held for its sender's name shows inside the name's answer (`0x49ccc0` → `0x49a870`,
/// `0x49cfc1`): after what the packets ahead of the answer raised, and before what the packets
/// behind it raise.
#[test]
fn a_held_line_fires_where_its_name_answers() {
    let mut app = client(|app| {
        app.net_handler(SessionEventKind::LoggedOut, enter)
            .net_handler(SessionEventKind::LoginQueued, regen)
            .net_handler(SessionEventKind::LogoutCancelled, stranger_whispers);
    });
    assert!(
        frame(&mut app, vec![p3()]).is_empty(),
        "the stranger's line waits for the name"
    );
    assert_eq!(
        frame(&mut app, vec![p1(), stranger_named(), p2()]),
        [
            "PLAYER_ENTER_COMBAT",
            "CHAT_MSG_WHISPER:psst",
            "PLAYER_REGEN_DISABLED"
        ]
    );
}

/// A line whose sender's name answers later in the same frame still waits for that answer: the
/// reference's handler found the cache empty at its packet and queued it (`0x49cae0`), so it shows
/// at the answer, behind the packets between, though the drain runs after both were handled.
#[test]
fn a_line_whose_name_answers_later_in_the_frame_fires_at_the_answer() {
    let mut app = client(|app| {
        app.net_handler(SessionEventKind::LoggedOut, enter)
            .net_handler(SessionEventKind::LoginQueued, regen)
            .net_handler(SessionEventKind::LogoutCancelled, stranger_whispers);
    });
    assert_eq!(
        frame(&mut app, vec![p3(), p2(), stranger_named(), p1()]),
        [
            "PLAYER_REGEN_DISABLED",
            "CHAT_MSG_WHISPER:psst",
            "PLAYER_ENTER_COMBAT"
        ]
    );
}

/// A line whose name never answers holds nothing up: the packets behind it fire as they come.
#[test]
fn a_line_that_never_resolves_blocks_nothing() {
    let mut app = client(|app| {
        app.net_handler(SessionEventKind::LoggedOut, enter)
            .net_handler(SessionEventKind::LoginQueued, regen)
            .net_handler(SessionEventKind::LogoutCancelled, stranger_whispers)
            .net_handler(SessionEventKind::LogoutResponse, friend_whispers);
    });
    assert_eq!(
        frame(&mut app, vec![p3(), p1(), p4(), p2()]),
        [
            "PLAYER_ENTER_COMBAT",
            "CHAT_MSG_WHISPER:hi",
            "PLAYER_REGEN_DISABLED"
        ]
    );
    assert_eq!(
        frame(&mut app, vec![p1()]),
        ["PLAYER_ENTER_COMBAT"],
        "still held, still in nobody's way"
    );
}

/// The rest-state watcher shows its line through `DisplayError` (`0x5de4e0` → `0x496720`), which
/// hands a chat-row message to the line function at once (`0x496822`-`0x496842`), so its
/// `CHAT_MSG_SYSTEM` fires inside the watcher's run, not later from the chat queue.
#[test]
fn a_notices_chat_line_fires_inside_the_notice() {
    let mut app = client(|app| {
        app.net_handler(SessionEventKind::LoginQueued, rest);
    });
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, vec![p2()]);
    fire_notices(world);
    assert_eq!(
        heard(&mut app),
        ["UPDATE_EXHAUSTION", "CHAT_MSG_SYSTEM:You feel normal."]
    );
}

/// A rest-state change and then a whisper, in one frame: the rest line shows first, as its
/// watcher shows it inside the update's handler.
#[test]
fn a_rest_line_shows_ahead_of_a_later_packets_whisper() {
    let mut app = client(|app| {
        app.net_handler(SessionEventKind::LoginQueued, rest)
            .net_handler(SessionEventKind::LogoutResponse, friend_whispers);
    });
    assert_eq!(
        frame(&mut app, vec![p2(), p4()]),
        [
            "UPDATE_EXHAUSTION",
            "CHAT_MSG_SYSTEM:You feel normal.",
            "CHAT_MSG_WHISPER:hi"
        ]
    );
}

/// Linen Cloth, the item a crafting line names.
const LINEN: u32 = 2589;

fn crafted(_: In<SessionEvent>, log: Res<crate::ui_chat::ChatLog>) {
    use crate::ui_chat::combat;
    let line = combat::queue(
        crate::ui_chat::ChatEventKind::SpellTradeskills,
        combat::TRADESKILL_LOG,
        (ME, combat::UnitClass::Me),
        (ME, combat::UnitClass::Me),
        combat::Fills::default(),
        combat::Named::Item(LINEN),
    )
    .unwrap();
    log.push_combat(line);
}

/// The item cache's answer as the item handler lands it: the record, then the combat log's
/// callback for the lines that waited on it (`0x6294b0`).
fn linen_answers(
    In(ev): In<SessionEvent>,
    mut items: ResMut<crate::items::Items>,
    log: Res<crate::ui_chat::ChatLog>,
) {
    if let SessionEvent::ItemTemplate { entry, info } = ev {
        items.insert_template(entry, info.map(|b| *b));
        log.answer_landed(crate::ui_chat::Answer::Item(entry));
    }
}

fn linen_named() -> SessionEvent {
    SessionEvent::ItemTemplate {
        entry: LINEN,
        info: Some(Box::new(crate::items::test_template("Linen Cloth"))),
    }
}

/// The active player, whose crafting line speaks in the first person.
const ME: u64 = 7;

/// A crafting line whose item the cache lacks, then a whisper, then the item's answer, in one
/// frame: the line waited at its packet in the combat log's list (`0x6293a0`) and is written
/// inside the answer's callback (`0x6294b0`), behind the whisper.
#[test]
fn a_line_whose_item_answers_later_in_the_frame_fires_at_the_answer() {
    let mut app = client(|app| {
        app.net_handler(SessionEventKind::LoggedOut, crafted)
            .net_handler(SessionEventKind::LogoutResponse, friend_whispers)
            .net_handler(SessionEventKind::ItemTemplate, linen_answers);
    });
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    app.world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>()
        .run(r#"TRADESKILL_LOG_FIRSTPERSON = "You create %s.""#)
        .unwrap();
    assert_eq!(
        frame(&mut app, vec![p1(), p4(), linen_named()]),
        [
            "CHAT_MSG_WHISPER:hi",
            "CHAT_MSG_SPELL_TRADESKILLS:You create Linen Cloth."
        ]
    );
}

/// A boar's line to [`STRANGER`], whose `$n` names its addressee.
fn monster_hails_the_stranger(_: In<SessionEvent>, log: Res<crate::ui_chat::ChatLog>) {
    log.push_wire(ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_MONSTER_SAY,
        language: 0,
        sender_guid: 0xF130_0000_0045_0003,
        target_guid: STRANGER,
        sender_name: Some("Boar".into()),
        channel: None,
        text: "Hello $n".into(),
        chat_tag: 0,
    });
}

/// A monster line whose `$n` names a subject the name cache lacks waits for the subject's name,
/// a player's as any other (`0x49daea` asks `0x55f080` by guid with `0x49ccc0`), and one whose
/// name answers later in its frame shows at the answer, behind the packets between, as a player
/// sender's line does.
#[test]
fn a_monster_line_whose_subject_answers_later_in_the_frame_fires_at_the_answer() {
    let mut app = client(|app| {
        app.net_handler(SessionEventKind::LoggedOut, monster_hails_the_stranger)
            .net_handler(SessionEventKind::LogoutResponse, friend_whispers);
    });
    assert_eq!(
        frame(&mut app, vec![p1(), p4(), stranger_named()]),
        ["CHAT_MSG_WHISPER:hi", "CHAT_MSG_MONSTER_SAY:Hello Stranger"]
    );
}

/// A boar's `$`-free line to [`STRANGER`].
fn monster_greets_the_stranger(_: In<SessionEvent>, log: Res<crate::ui_chat::ChatLog>) {
    log.push_wire(ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_MONSTER_SAY,
        language: 0,
        sender_guid: 0xF130_0000_0045_0003,
        target_guid: STRANGER,
        sender_name: Some("Boar".into()),
        channel: None,
        text: "Hello there".into(),
        chat_tag: 0,
    });
}

/// A monster line with no `$`-macro expands at its packet whatever the cache holds, so it waits
/// for no name: only a failed expansion queues the line (`0x49dac2`).
#[test]
fn a_monster_line_with_no_macro_fires_at_its_packet() {
    let mut app = client(|app| {
        app.net_handler(SessionEventKind::LoggedOut, monster_greets_the_stranger)
            .net_handler(SessionEventKind::LogoutResponse, friend_whispers);
    });
    assert_eq!(
        frame(&mut app, vec![p1(), p4(), stranger_named()]),
        ["CHAT_MSG_MONSTER_SAY:Hello there", "CHAT_MSG_WHISPER:hi"]
    );
}

/// The handlers the answers come by queue the release behind a line that reads them: the item
/// handler's (`0x6294b0`) and the name handler's (`0x49ccc0`), for a monster line's subject and a
/// player line's sender.
#[test]
fn each_answer_handler_queues_a_release_behind_a_line_that_reads_it() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let world = app.world_mut();
    let log = || crate::ui_chat::ChatLog::default();
    for (line, answer) in [
        (
            crafted as fn(In<SessionEvent>, Res<crate::ui_chat::ChatLog>),
            linen_named(),
        ),
        (monster_hails_the_stranger, stranger_named()),
        (stranger_whispers, stranger_named()),
    ] {
        world.insert_resource(log());
        // Each answer finds the cache without it, so its lookup's callback runs.
        world.insert_resource(crate::names::NameCache::default());
        bevy::ecs::system::RunSystemOnce::run_system_once_with(&mut *world, line, p1()).unwrap();
        crate::net::handlers::dispatch(world, vec![answer]);
        let kinds = world.resource::<crate::ui_chat::ChatLog>().pending_kinds();
        assert_eq!(kinds.last(), Some(&"release"), "{kinds:?}");
    }
}
