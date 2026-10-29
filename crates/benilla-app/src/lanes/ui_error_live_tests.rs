//! The UI error line at its handlers' places. `DisplayError` (`0x496720`) and the sink
//! `0x4945b0` signal inside whatever calls them, so a refusal or a notice a packet raises fires
//! between the neighbouring packets' events, in its handler's order against the handler's other
//! signals: the ticket answer's line ahead of its `UPDATE_TICKET` (`0x5e4504`, `0x5e4527`), the
//! mount result's (`0x5ec6f3`), the tame failure's (`0x6e6ab4`), the notification's and the area
//! trigger's (`0x401828`, `0x48f921`), the guild invite's after its popup (`0x5e6f53`,
//! `0x5e6f65`), a guild event's ahead of its roster update (`0x5e745f`-`0x5e74c4`), a disband's
//! ahead of the party's end (`0x5e688b`, `0x5e6898`), and a ready check's ahead of its popup
//! (`0x4ba51e`, `0x4ba53a`).

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::messages::{member_status, GroupMemberEntry};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Sender;

use super::unit_view_tests::{created, wire_packets};

const ME: u64 = 0x7;
const MATE: u64 = 0x22;

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
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

const SPY: &str = r#"
    Seen = {}
    ERR_TICKET_DB_ERROR = "Ticket database error"
    ERR_MOUNT_TOOFARAWAY = "Too far away"
    ERR_TAME_FAILED = "%s."
    PETTAME_TOOHIGHLEVEL = "Creature is too high level for you to tame"
    ERR_INVITED_TO_GUILD_SS = "%s invites you to join %s"
    ERR_GROUP_DISBANDED = "Your group has been disbanded."
    ERR_GUILD_JOIN_S = "%s has joined the guild."
    ERR_RAID_LEADER_READY_CHECK_START_S = "%s has initiated a ready check."
    local f = CreateFrame("Frame")
    for _, e in ipairs({"UI_ERROR_MESSAGE", "UI_INFO_MESSAGE", "CHAT_MSG_SYSTEM",
            "CHAT_MSG_MONSTER_SAY", "UPDATE_TICKET", "GUILD_INVITE_REQUEST",
            "PARTY_MEMBERS_CHANGED", "GUILD_ROSTER_UPDATE", "READY_CHECK"}) do
        f:RegisterEvent(e)
    end
    local bare = {UPDATE_TICKET = 1, PARTY_MEMBERS_CHANGED = 1, GUILD_ROSTER_UPDATE = 1,
        READY_CHECK = 1}
    f:SetScript("OnEvent", function()
        if bare[event] then
            table.insert(Seen, event)
        else
            table.insert(Seen, event .. ":" .. tostring(arg1))
        end
    end)
"#;

/// The client in the world with our player held, and a spy whose line for each event carries its
/// name and first argument.
fn client() -> Client {
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
    script.run(SPY).unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client { app, inbound };
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[(FIELD_UNIT_HEALTH, 100)],
    )]]);
    c.frame(vec![]);
    c.seen();
    c
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

/// `packet` between two of the boar's lines, in one frame: what the spy heard.
fn between(c: &mut Client, packet: SessionEvent) -> Vec<String> {
    c.frame(vec![
        vec![boar_says("a")],
        vec![packet],
        vec![boar_says("b")],
    ]);
    c.seen()
}

/// A refusal by key: the ticket answer's status 0 shows `ERR_TICKET_DB_ERROR` (`0x5e4504`), then
/// signals `UPDATE_TICKET` (`0x5e4527`), both between the neighbours' lines.
#[test]
fn a_refusal_fires_between_its_neighbours_events_in_its_handlers_order() {
    let mut c = client();
    let ticket = SessionEvent::GmTicket {
        status: 0,
        ticket: None,
    };
    assert_eq!(
        between(&mut c, ticket),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "UI_ERROR_MESSAGE:Ticket database error",
            "UPDATE_TICKET",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
}

/// The mount result's line (`0x5ec6f3`) fires at its packet's place.
#[test]
fn a_mount_refusal_fires_at_its_packet() {
    let mut c = client();
    let refused = SessionEvent::MountResult {
        mount: true,
        code: 1,
    };
    assert_eq!(
        between(&mut c, refused),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "UI_ERROR_MESSAGE:Too far away",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
}

/// The tame failure's line (`0x6e6ab4`), its reason's string in `%s`, fires at its packet's
/// place.
#[test]
fn a_tame_failure_fires_at_its_packet() {
    let mut c = client();
    assert_eq!(
        between(&mut c, SessionEvent::PetTameFailure { reason: 9 }),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "UI_ERROR_MESSAGE:Creature is too high level for you to tame.",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
}

/// A notification's red line and an area trigger's yellow one (`0x401828`, `0x48f921`) fire at
/// their packets' places.
#[test]
fn the_wires_texts_fire_at_their_packets() {
    let mut c = client();
    c.frame(vec![
        vec![boar_says("a")],
        vec![SessionEvent::Notification {
            text: "GM mode is ON".into(),
        }],
        vec![boar_says("b")],
        vec![SessionEvent::AreaTriggerMessage {
            text: "You must be level 58.".into(),
        }],
        vec![boar_says("c")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "UI_ERROR_MESSAGE:GM mode is ON",
            "CHAT_MSG_MONSTER_SAY:b",
            "UI_INFO_MESSAGE:You must be level 58.",
            "CHAT_MSG_MONSTER_SAY:c",
        ]
    );
}

/// The guild invite shows its popup (`0x5e6f53`), then its chat line (`0x5e6f65`).
#[test]
fn a_guild_invite_shows_its_line_after_its_popup() {
    let mut c = client();
    let invite = SessionEvent::GuildInvite {
        inviter: "Dorn".into(),
        guild: "Ironforge Guard".into(),
    };
    assert_eq!(
        between(&mut c, invite),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "GUILD_INVITE_REQUEST:Dorn",
            "CHAT_MSG_SYSTEM:Dorn invites you to join Ironforge Guard",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
}

/// A disband with a party slot held shows its line (`0x5e688b`), then ends the party
/// (`0x5e6898`), whose `PARTY_MEMBERS_CHANGED` follows it.
#[test]
fn a_disband_shows_its_line_ahead_of_the_partys_end() {
    let mut c = client();
    grouped(&mut c, ME);
    let seen = between(&mut c, SessionEvent::GroupDestroyed);
    assert_eq!(
        seen.first().map(String::as_str),
        Some("CHAT_MSG_MONSTER_SAY:a")
    );
    assert_eq!(
        seen.get(1).map(String::as_str),
        Some("CHAT_MSG_SYSTEM:Your group has been disbanded."),
        "{seen:?}"
    );
    assert_eq!(
        seen.get(2).map(String::as_str),
        Some("PARTY_MEMBERS_CHANGED"),
        "{seen:?}"
    );
    assert_eq!(
        seen.last().map(String::as_str),
        Some("CHAT_MSG_MONSTER_SAY:b"),
        "{seen:?}"
    );
}

/// A party of us and Dorn, [`MATE`], led by `leader`, the spy emptied.
fn grouped(c: &mut Client, leader: u64) {
    c.frame(vec![vec![SessionEvent::GroupList {
        group_type: 0,
        own_flags: 0,
        members: vec![GroupMemberEntry {
            name: "Dorn".into(),
            guid: MATE,
            status: member_status::ONLINE,
            flags: 0,
        }],
        leader,
        loot: None,
    }]]);
    c.frame(vec![]);
    c.seen();
}

/// A guild event's arm shows its line (`0x5e745f`-`0x5e74b7`), then the roster update every arm
/// but the sign-on pair's ends in (`0x5e74c4`).
#[test]
fn a_guild_event_shows_its_line_ahead_of_its_roster_update() {
    let mut c = client();
    let joined = SessionEvent::GuildEvent(benilla_protocol::messages::GuildEventNotice {
        event: benilla_protocol::messages::guild_event::JOINED,
        params: vec!["Brisca".into()],
        guid: None,
    });
    assert_eq!(
        between(&mut c, joined),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "CHAT_MSG_SYSTEM:Brisca has joined the guild.",
            "GUILD_ROSTER_UPDATE",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
}

/// A ready check another leads shows the leader's line (`0x4ba51e`) when the name cache holds the
/// leader (`0x4ba50f`, no callback), then `READY_CHECK` (`0x4ba53a`); with the name still to
/// come the line is skipped and `READY_CHECK` fires all the same (`0x4ba514`-`0x4ba53a`).
#[test]
fn a_ready_check_shows_its_line_ahead_of_its_popup() {
    let mut c = client();
    grouped(&mut c, MATE);
    assert_eq!(
        between(&mut c, SessionEvent::ReadyCheckRequest),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "READY_CHECK",
            "CHAT_MSG_MONSTER_SAY:b",
        ],
        "the leader's name is not cached"
    );
    c.frame(vec![vec![SessionEvent::PlayerName {
        guid: MATE,
        name: "Dorn".into(),
        race: 1,
        gender: 0,
        class: 1,
    }]]);
    c.seen();
    assert_eq!(
        between(&mut c, SessionEvent::ReadyCheckRequest),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "CHAT_MSG_SYSTEM:Dorn has initiated a ready check.",
            "READY_CHECK",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
}

/// The ready check's lines print inside the handler that composes them (`0x49a870`), between the
/// neighbouring packets' events: a member's not-ready answer prints `RAID_MEMBER_NOT_READY` for
/// them (`0x4ba48a`), and with nobody left pending the close runs the timeout worker at once
/// (`0x4ba4d9` → `0x4bb310`), whose summary follows (`0x4bb49a`).
#[test]
fn a_ready_check_answer_prints_its_lines_at_its_packet() {
    let mut c = client();
    c.frame(vec![vec![SessionEvent::GroupList {
        group_type: crate::ui_party::GROUPTYPE_RAID,
        own_flags: 0,
        members: vec![GroupMemberEntry {
            name: "Dorn".into(),
            guid: MATE,
            status: member_status::ONLINE,
            flags: 0,
        }],
        leader: ME,
        loot: None,
    }]]);
    c.frame(vec![]);
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM")
            .run(
                r#"RAID_MEMBER_NOT_READY = "%s is not ready"
                   READY_CHECK_NO_AFK = "No players are away"
                   DoReadyCheck()"#,
            )
            .expect("the check starts")
    });
    c.seen();
    let answer = SessionEvent::ReadyCheckAnswer {
        guid: MATE,
        ready: 0,
    };
    assert_eq!(
        between(&mut c, answer),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "CHAT_MSG_SYSTEM:Dorn is not ready",
            "CHAT_MSG_SYSTEM:No players are away",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
    c.frame(vec![]);
    assert_eq!(c.seen(), Vec::<String>::new(), "once");
}

/// A world session that ends before the world is entered releases what it held behind its entry
/// load (`net::apply`'s `InboundHold`), its notice's line among them, with no world left to clear
/// it; the next session's start drops it, so that session's interface never shows it.
#[test]
fn a_session_that_ends_before_the_world_leaves_no_line_for_the_next() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, _sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let connected = || SessionEvent::Connected {
        self_guid: ME,
        name: "Me".into(),
        billing: Default::default(),
        tutorial_flags: None,
        addon_info: None,
    };
    for ev in [
        connected(),
        SessionEvent::Notification {
            text: "stale".into(),
        },
        SessionEvent::Disconnected {
            reason: "lost".into(),
            end: benilla_protocol::SessionEnd::Lost,
        },
    ] {
        inbound.send(ev).unwrap();
    }
    app.update();
    app.update();
    assert_eq!(
        app.world()
            .resource::<crate::ui_action::UiErrorKeys>()
            .texts()
            .len(),
        1,
        "the released notice is queued, with no world to clear it"
    );

    inbound.send(connected()).unwrap();
    app.update();
    assert!(
        app.world()
            .resource::<crate::ui_action::UiErrorKeys>()
            .texts()
            .is_empty(),
        "the next session's start drops it"
    );
    // The next session's interface up: its lane fires nothing of the last session's.
    let script = UiScript::new().expect("a VM");
    script.run(SPY).unwrap();
    app.insert_non_send_resource(script);
    crate::ui_action::fire_ui_error_lane(app.world_mut());
    let seen: Vec<String> =
        crate::script_calls::eval_in_vm(app.world(), "local s = Seen Seen = {} return s");
    assert!(
        seen.is_empty(),
        "no stale line in the next session: {seen:?}"
    );
}
