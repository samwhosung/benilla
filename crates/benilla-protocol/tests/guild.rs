//! The guild wire: the query and roster caches, invitations, member verbs, rank administration,
//! the event broadcast and command results, laid out per vmangos.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{
    self, guild_command, guild_command_error, guild_event, guild_presence, guild_rank_right,
    opcode, GUILD_RANKS_MAX_COUNT,
};
use benilla_protocol::ServerPacket;
use common::hx;

#[test]
fn cmsg_bodies_golden() {
    // CMSG_GUILD_QUERY (vmangos Server/Packets/Guild.cpp:8-11): one u32, the guild id.
    assert_eq!(
        messages::guild_query(0x1234),
        hx("34120000"),
        "CMSG_GUILD_QUERY body"
    );

    // The one-cstring verbs (Guild.cpp:3-6, 13-16, 18-21, 23-26, 28-31, 33-36, 73-76, 44-49).
    assert_eq!(
        messages::guild_create("Legacy"),
        hx("4c656761637900"),
        "CMSG_GUILD_CREATE body"
    );
    assert_eq!(
        messages::guild_invite("Bob"),
        hx("426f6200"),
        "CMSG_GUILD_INVITE body"
    );
    assert_eq!(
        messages::guild_promote("Bob"),
        hx("426f6200"),
        "CMSG_GUILD_PROMOTE body"
    );
    assert_eq!(
        messages::guild_demote("Bob"),
        hx("426f6200"),
        "CMSG_GUILD_DEMOTE body"
    );
    assert_eq!(
        messages::guild_remove("Bob"),
        hx("426f6200"),
        "CMSG_GUILD_REMOVE body"
    );
    assert_eq!(
        messages::guild_leader("Bob"),
        hx("426f6200"),
        "CMSG_GUILD_LEADER body"
    );
    assert_eq!(
        messages::guild_add_rank("Grunt"),
        hx("4772756e7400"),
        "CMSG_GUILD_ADD_RANK body"
    );
    assert_eq!(
        messages::guild_info_text("We raid."),
        hx("576520726169642e00"),
        "CMSG_GUILD_INFO_TEXT body"
    );

    // CMSG_GUILD_MOTD (Guild.cpp:38-42): one cstring.
    assert_eq!(
        messages::guild_motd("Hello"),
        hx("48656c6c6f00"),
        "CMSG_GUILD_MOTD body"
    );

    // The empty verbs (vmangos Opcodes.cpp:213, 214, 216, 218, 222, 224, 655: NullClientPacket).
    assert_eq!(
        messages::guild_accept(),
        Vec::<u8>::new(),
        "CMSG_GUILD_ACCEPT body"
    );
    assert_eq!(
        messages::guild_decline(),
        Vec::<u8>::new(),
        "CMSG_GUILD_DECLINE body"
    );
    assert_eq!(
        messages::guild_info(),
        Vec::<u8>::new(),
        "CMSG_GUILD_INFO body"
    );
    assert_eq!(
        messages::guild_roster(),
        Vec::<u8>::new(),
        "CMSG_GUILD_ROSTER body"
    );
    assert_eq!(
        messages::guild_leave(),
        Vec::<u8>::new(),
        "CMSG_GUILD_LEAVE body"
    );
    assert_eq!(
        messages::guild_disband(),
        Vec::<u8>::new(),
        "CMSG_GUILD_DISBAND body"
    );
    assert_eq!(
        messages::guild_del_rank(),
        Vec::<u8>::new(),
        "CMSG_GUILD_DEL_RANK body"
    );

    // CMSG_GUILD_RANK (Guild.cpp:78-83): u32 rankId, u32 rights, cstring rankName.
    let rights = guild_rank_right::GCHAT_LISTEN
        | guild_rank_right::GCHAT_SPEAK
        | guild_rank_right::OFFCHAT_LISTEN
        | guild_rank_right::OFFCHAT_SPEAK
        | guild_rank_right::INVITE
        | guild_rank_right::PROMOTE
        | guild_rank_right::DEMOTE;
    assert_eq!(rights, 0x0000_019F, "the officer rights mask");
    assert_eq!(
        messages::guild_rank(2, rights, "Veteran"),
        hx(concat!("02000000", "9f010000", "5665746572616e00")),
        "CMSG_GUILD_RANK body"
    );

    // CMSG_GUILD_SET_PUBLIC_NOTE / _OFFICER_NOTE (Guild.cpp:61-65 / 67-71): name, then note.
    assert_eq!(
        messages::guild_set_public_note("Bob", "hi"),
        hx(concat!("426f6200", "686900")),
        "CMSG_GUILD_SET_PUBLIC_NOTE body"
    );
    assert_eq!(
        messages::guild_set_officer_note("Bob", "hi"),
        hx(concat!("426f6200", "686900")),
        "CMSG_GUILD_SET_OFFICER_NOTE body"
    );
}

/// An empty MOTD is a one-byte empty cstring; vmangos (`Server/Packets/Guild.cpp:38-42`) reads a
/// zero-byte body as a clear too.
#[test]
fn guild_motd_clears_with_an_empty_cstring_not_an_empty_body() {
    assert_eq!(
        messages::guild_motd(""),
        hx("00"),
        "CMSG_GUILD_MOTD body, cleared"
    );
    assert_eq!(messages::guild_motd("").len(), 1, "one NUL, not zero bytes");
}

/// `SMSG_GUILD_QUERY_RESPONSE` always carries ten rank-name cstrings, not a counted list (vmangos
/// `Server/Packets/Guild.cpp:118-131`); the distinct emblem values after them catch a short read.
#[test]
fn guild_query_response_always_carries_ten_rank_names() {
    let mut body = Vec::new();
    body.extend_from_slice(&7u32.to_le_bytes()); // guildId
    body.extend_from_slice(b"Legacy of Steel\0"); // guildName
    for name in [
        b"Guild Master".as_slice(),
        b"Officer".as_slice(),
        b"Veteran".as_slice(),
        b"Member".as_slice(),
        b"Initiate".as_slice(),
    ] {
        body.extend_from_slice(name);
        body.push(0);
    }
    // The five uncreated ranks: an empty string each, a bare NUL, not absent.
    body.extend_from_slice(&[0u8; GUILD_RANKS_MAX_COUNT - 5]);
    body.extend_from_slice(&1u32.to_le_bytes()); // emblemStyle
    body.extend_from_slice(&2u32.to_le_bytes()); // emblemColor
    body.extend_from_slice(&3u32.to_le_bytes()); // borderStyle
    body.extend_from_slice(&4u32.to_le_bytes()); // borderColor
    body.extend_from_slice(&5u32.to_le_bytes()); // backgroundColor

    let packet = messages::parse_server(opcode::SMSG_GUILD_QUERY_RESPONSE, &body).unwrap();
    let response = match &packet {
        ServerPacket::GuildQueryResponse(r) => r.clone(),
        other => panic!("expected GuildQueryResponse, got {}", other.name()),
    };
    assert_eq!(response.guild_id, 7);
    assert_eq!(response.name, "Legacy of Steel");
    assert_eq!(response.rank_names[0], "Guild Master");
    assert_eq!(response.rank_names[4], "Initiate");
    assert_eq!(
        &response.rank_names[5..],
        &["", "", "", "", ""],
        "the five uncreated ranks are empty strings on the wire"
    );
    assert_eq!(response.emblem_style, 1, "emblemStyle — the desync canary");
    assert_eq!(response.emblem_color, 2);
    assert_eq!(response.border_style, 3);
    assert_eq!(response.border_color, 4);
    assert_eq!(response.background_color, 5);

    match decode(packet).as_slice() {
        [SessionEvent::GuildQueryResponse(r)] => assert_eq!(r.name, "Legacy of Steel"),
        other => panic!("guild query response decode: {other:?}"),
    }
}

/// "No such guild" is a complete `SMSG_GUILD_QUERY_RESPONSE` with an empty name: the reference
/// branches on it (`0x5552ae`) and still consumes the whole record.
#[test]
fn guild_query_response_reports_no_such_guild_as_an_empty_name() {
    let mut body = Vec::new();
    body.extend_from_slice(&404u32.to_le_bytes()); // the id we asked about
    body.push(0); // guildName: empty == "this id names nothing"
    body.extend_from_slice(&[0u8; GUILD_RANKS_MAX_COUNT]); // ten empty rank names, still sent
    for _ in 0..5 {
        body.extend_from_slice(&0u32.to_le_bytes()); // the emblem block, still sent
    }

    match &messages::parse_server(opcode::SMSG_GUILD_QUERY_RESPONSE, &body).unwrap() {
        ServerPacket::GuildQueryResponse(response) => {
            assert_eq!(response.guild_id, 404);
            assert_eq!(response.name, "", "the empty name IS the not-found signal");
            assert!(response.rank_names.iter().all(String::is_empty));
            assert_eq!(response.background_color, 0, "the full record was consumed");
        }
        other => panic!("expected GuildQueryResponse, got {}", other.name()),
    }
}

/// Append one `SMSG_GUILD_ROSTER` member (vmangos `Server/Packets/Guild.cpp:155-172`); the caller
/// passes `last_online` itself, `Some` only for an offline member.
fn push_member(
    body: &mut Vec<u8>,
    guid: u64,
    presence: u8,
    name: &str,
    rank_id: u32,
    level: u8,
    class: u8,
    zone: u32,
    last_online: Option<f32>,
    public_note: &str,
    officer_note: &str,
) {
    body.extend_from_slice(&guid.to_le_bytes());
    body.push(presence);
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body.extend_from_slice(&rank_id.to_le_bytes());
    body.push(level);
    body.push(class);
    body.extend_from_slice(&zone.to_le_bytes());
    if let Some(days) = last_online {
        body.extend_from_slice(&days.to_le_bytes());
    }
    body.extend_from_slice(public_note.as_bytes());
    body.push(0);
    body.extend_from_slice(officer_note.as_bytes());
    body.push(0);
}

/// An `SMSG_GUILD_ROSTER` head (Server/Packets/Guild.cpp:143-153): member count, MOTD, info text,
/// then the counted rank-rights array.
fn roster_head(member_count: u32, motd: &str, info: &str, rank_rights: &[u32]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&member_count.to_le_bytes());
    body.extend_from_slice(motd.as_bytes());
    body.push(0);
    body.extend_from_slice(info.as_bytes());
    body.push(0);
    body.extend_from_slice(&(rank_rights.len() as u32).to_le_bytes());
    for rights in rank_rights {
        body.extend_from_slice(&rights.to_le_bytes());
    }
    body
}

/// `SMSG_GUILD_ROSTER`'s `f32 lastOnlineTime` is sent only when the presence byte is 0 (vmangos
/// `Server/Packets/Guild.cpp:164-165`); the third member, after a wider offline one, checks the
/// parse stays in step.
#[test]
fn guild_roster_conditional_float_keeps_later_members_in_sync() {
    let mut body = roster_head(
        3,
        "Raid at 8",
        "We are a guild.",
        &[guild_rank_right::ALL, 0x0000_019F, 0x0000_0003],
    );
    push_member(
        &mut body,
        0x1111,
        guild_presence::ONLINE,
        "Alice",
        0,
        60,
        2,
        1519,
        None,
        "the GM",
        "trusted",
    );
    push_member(
        &mut body,
        0x2222,
        guild_presence::OFFLINE,
        "Bob",
        1,
        58,
        4,
        1497,
        Some(3.5),
        "on holiday",
        "back next week",
    );
    push_member(
        &mut body,
        0x3333,
        guild_presence::ONLINE | guild_presence::AFK,
        "Carol",
        3,
        42,
        9,
        141,
        None,
        "levelling",
        "promote soon",
    );

    let packet = messages::parse_server(opcode::SMSG_GUILD_ROSTER, &body).unwrap();
    let roster = match &packet {
        ServerPacket::GuildRoster(r) => r.clone(),
        other => panic!("expected GuildRoster, got {}", other.name()),
    };

    assert_eq!(roster.motd, "Raid at 8");
    assert_eq!(roster.info, "We are a guild.");
    assert_eq!(
        roster.rank_rights,
        vec![guild_rank_right::ALL, 0x0000_019F, 0x0000_0003],
        "rank_rights' length is the guild's real rank count"
    );
    assert_eq!(roster.members.len(), 3);

    let alice = &roster.members[0];
    assert!(alice.is_online());
    assert_eq!(alice.name, "Alice");
    assert_eq!(alice.last_online_days, 0.0, "online: the float is not sent");
    assert_eq!(alice.public_note, "the GM");
    assert_eq!(alice.officer_note, "trusted");

    let bob = &roster.members[1];
    assert!(!bob.is_online());
    assert_eq!(bob.name, "Bob");
    assert_eq!(bob.last_online_days, 3.5, "offline: the float IS sent");
    assert_eq!(bob.public_note, "on holiday");
    assert_eq!(bob.officer_note, "back next week");

    let carol = &roster.members[2];
    assert_eq!(carol.guid, 0x3333, "third member guid — the desync canary");
    assert_eq!(carol.name, "Carol");
    assert_eq!(carol.presence, guild_presence::ONLINE | guild_presence::AFK);
    assert!(carol.is_online());
    assert_eq!(carol.rank_id, 3);
    assert_eq!(carol.level, 42);
    assert_eq!(carol.class, 9);
    assert_eq!(carol.zone, 141);
    assert_eq!(carol.last_online_days, 0.0);
    assert_eq!(carol.public_note, "levelling");
    assert_eq!(carol.officer_note, "promote soon");

    match decode(packet).as_slice() {
        [SessionEvent::GuildRoster(r)] => assert_eq!(r.members.len(), 3),
        other => panic!("guild roster decode: {other:?}"),
    }
}

#[test]
fn guild_roster_with_no_members_is_just_the_head() {
    let body = roster_head(0, "", "", &[guild_rank_right::ALL]);
    let packet = messages::parse_server(opcode::SMSG_GUILD_ROSTER, &body).unwrap();
    match &packet {
        ServerPacket::GuildRoster(roster) => {
            assert!(roster.members.is_empty());
            assert_eq!(roster.motd, "");
            assert_eq!(roster.info, "");
            assert_eq!(roster.rank_rights, vec![guild_rank_right::ALL]);
        }
        other => panic!("expected GuildRoster, got {}", other.name()),
    }
}

/// A viewer without `GR_RIGHT_VIEWOFFNOTE` gets `""` for every officer note (vmangos
/// `Guild/Guild.cpp:819`, `:846`): the cstring's NUL is still on the wire.
#[test]
fn guild_roster_reads_the_empty_officer_notes_of_an_unprivileged_viewer() {
    let mut body = roster_head(2, "motd", "info", &[guild_rank_right::ALL, 0x0000_0003]);
    push_member(
        &mut body,
        0x1111,
        guild_presence::OFFLINE,
        "Alice",
        0,
        60,
        2,
        1519,
        Some(0.25),
        "note A",
        "",
    );
    push_member(
        &mut body,
        0x2222,
        guild_presence::ONLINE,
        "Bob",
        1,
        58,
        4,
        1497,
        None,
        "note B",
        "",
    );

    let packet = messages::parse_server(opcode::SMSG_GUILD_ROSTER, &body).unwrap();
    match &packet {
        ServerPacket::GuildRoster(roster) => {
            assert_eq!(roster.members.len(), 2);
            assert_eq!(roster.members[0].officer_note, "");
            assert_eq!(
                roster.members[1].name, "Bob",
                "the second row still lines up"
            );
            assert_eq!(roster.members[1].public_note, "note B");
            assert_eq!(roster.members[1].officer_note, "");
        }
        other => panic!("expected GuildRoster, got {}", other.name()),
    }
}

/// The float's condition is the whole presence byte against zero, as the reference tests it
/// (`0x4d0c12`), never `presence & ONLINE`; `0x08`, a bit no 1.12 flag owns, still means online.
#[test]
fn roster_presence_is_tested_whole_byte_not_masked_against_online() {
    let mut body = roster_head(2, "", "", &[guild_rank_right::ALL]);
    push_member(
        &mut body, 0x1111, 0x08, // an unnamed presence bit: online, so no float
        "Alice", 0, 60, 2, 1519, None, "first", "",
    );
    push_member(
        &mut body,
        0x2222,
        guild_presence::ONLINE,
        "Bob",
        1,
        58,
        4,
        1497,
        None,
        "second",
        "",
    );

    match &messages::parse_server(opcode::SMSG_GUILD_ROSTER, &body).unwrap() {
        ServerPacket::GuildRoster(roster) => {
            assert!(
                roster.members[0].is_online(),
                "an unnamed presence bit still means online"
            );
            assert_eq!(roster.members[0].last_online_days, 0.0);
            assert_eq!(
                roster.members[1].name, "Bob",
                "a masked test would have desynchronised here"
            );
            assert_eq!(roster.members[1].public_note, "second");
        }
        other => panic!("expected GuildRoster, got {}", other.name()),
    }
}

/// `SMSG_GUILD_EVENT`'s trailing guid rides on the event id: the reference (`0x5e7180`) reads it
/// only for 0x0c and 0x0d, though vmangos also writes one for `GE_JOINED` and `GE_LEFT`
/// (`Handlers/GuildHandler.cpp:218`, `:405`).
#[test]
fn guild_event_trailing_guid_rides_on_the_event_id() {
    // GE_SIGNED_ON (0x0c): one param and the guid (Server/Packets/Guild.cpp:133-141).
    let mut body = vec![guild_event::SIGNED_ON, 1];
    body.extend_from_slice(b"Alice\0");
    body.extend_from_slice(&0xDEAD_BEEFu64.to_le_bytes());
    let packet = messages::parse_server(opcode::SMSG_GUILD_EVENT, &body).unwrap();
    match &packet {
        ServerPacket::GuildEvent(notice) => {
            assert_eq!(notice.event, guild_event::SIGNED_ON);
            assert_eq!(notice.params, vec!["Alice".to_string()]);
            assert_eq!(notice.guid, Some(0xDEAD_BEEF));
        }
        other => panic!("expected GuildEvent, got {}", other.name()),
    }
    match decode(packet).as_slice() {
        [SessionEvent::GuildEvent(notice)] => assert_eq!(notice.guid, Some(0xDEAD_BEEF)),
        other => panic!("guild event decode: {other:?}"),
    }

    // GE_MOTD (0x02): one param, no guid.
    let mut body = vec![guild_event::MOTD, 1];
    body.extend_from_slice(b"Raid at 8\0");
    let packet = messages::parse_server(opcode::SMSG_GUILD_EVENT, &body).unwrap();
    match &packet {
        ServerPacket::GuildEvent(notice) => {
            assert_eq!(notice.event, guild_event::MOTD);
            assert_eq!(notice.params, vec!["Raid at 8".to_string()]);
            assert_eq!(notice.guid, None);
        }
        other => panic!("expected GuildEvent, got {}", other.name()),
    }

    // GE_JOINED (0x03): vmangos appends a guid here, which the reference never reads.
    let mut body = vec![guild_event::JOINED, 1];
    body.extend_from_slice(b"Carol\0");
    body.extend_from_slice(&0x1234u64.to_le_bytes());
    match &messages::parse_server(opcode::SMSG_GUILD_EVENT, &body).unwrap() {
        ServerPacket::GuildEvent(notice) => {
            assert_eq!(notice.event, guild_event::JOINED);
            assert_eq!(notice.params, vec!["Carol".to_string()]);
            assert_eq!(
                notice.guid, None,
                "the client reads the guid for 0x0c/0x0d only — this one is trailing slack"
            );
        }
        other => panic!("expected GuildEvent, got {}", other.name()),
    }

    // GE_PROMOTION (0x00): the three-param maximum (promoter, promoted, new rank name).
    let mut body = vec![guild_event::PROMOTION, 3];
    body.extend_from_slice(b"Alice\0Bob\0Officer\0");
    match &messages::parse_server(opcode::SMSG_GUILD_EVENT, &body).unwrap() {
        ServerPacket::GuildEvent(notice) => {
            assert_eq!(notice.params, vec!["Alice", "Bob", "Officer"]);
            assert_eq!(notice.guid, None);
        }
        other => panic!("expected GuildEvent, got {}", other.name()),
    }

    // GE_DISBANDED (0x08): no params, no guid, the two-byte minimum body.
    let packet = messages::parse_server(
        opcode::SMSG_GUILD_EVENT,
        &hx(concat!("08", "00")), // event, paramCount
    )
    .unwrap();
    match &packet {
        ServerPacket::GuildEvent(notice) => {
            assert_eq!(notice.event, guild_event::DISBANDED);
            assert!(notice.params.is_empty());
            assert_eq!(notice.guid, None);
        }
        other => panic!("expected GuildEvent, got {}", other.name()),
    }
}

/// `SMSG_GUILD_COMMAND_RESULT` (Server/Packets/Guild.cpp:96-101): `u32` command, a cstring in the
/// middle, `u32` result.
#[test]
fn guild_command_result_wire() {
    let mut body = Vec::new();
    body.extend_from_slice(&guild_command::INVITE.to_le_bytes());
    body.extend_from_slice(b"Bob\0");
    body.extend_from_slice(&guild_command_error::ALREADY_IN_GUILD_S.to_le_bytes());
    let packet = messages::parse_server(opcode::SMSG_GUILD_COMMAND_RESULT, &body).unwrap();
    match &packet {
        ServerPacket::GuildCommandResult(result) => {
            assert_eq!(result.command, guild_command::INVITE);
            assert_eq!(result.name, "Bob");
            assert_eq!(result.result, guild_command_error::ALREADY_IN_GUILD_S);
        }
        other => panic!("expected GuildCommandResult, got {}", other.name()),
    }
    match decode(packet).as_slice() {
        [SessionEvent::GuildCommandResult(result)] => assert_eq!(result.name, "Bob"),
        other => panic!("guild command result decode: {other:?}"),
    }

    // Result 0x08 is ERR_GUILD_LEADER_LEAVE under QUIT and ERR_GUILD_PERMISSIONS otherwise
    // (vmangos Guild/Guild.h:106-107); the command tag tells them apart.
    assert_eq!(
        guild_command_error::LEADER_LEAVE,
        guild_command_error::PERMISSIONS
    );
    let mut body = Vec::new();
    body.extend_from_slice(&guild_command::QUIT.to_le_bytes());
    body.extend_from_slice(b"\0");
    body.extend_from_slice(&guild_command_error::LEADER_LEAVE.to_le_bytes());
    match &messages::parse_server(opcode::SMSG_GUILD_COMMAND_RESULT, &body).unwrap() {
        ServerPacket::GuildCommandResult(result) => {
            assert_eq!(result.command, guild_command::QUIT);
            assert_eq!(
                result.name, "",
                "the empty middle cstring is still consumed"
            );
            assert_eq!(result.result, 0x08);
        }
        other => panic!("expected GuildCommandResult, got {}", other.name()),
    }
}

/// `SMSG_GUILD_INFO` (Server/Packets/Guild.cpp:103-111): the guild name, then day/month/year and
/// the member and account counts, five `u32`s in that order.
#[test]
fn guild_info_wire() {
    let mut body = Vec::new();
    body.extend_from_slice(b"Legacy of Steel\0");
    body.extend_from_slice(&11u32.to_le_bytes()); // createdDay
    body.extend_from_slice(&9u32.to_le_bytes()); // createdMonth
    body.extend_from_slice(&2004u32.to_le_bytes()); // createdYear
    body.extend_from_slice(&57u32.to_le_bytes()); // memberCount
    body.extend_from_slice(&41u32.to_le_bytes()); // accountCount

    let packet = messages::parse_server(opcode::SMSG_GUILD_INFO, &body).unwrap();
    match &packet {
        ServerPacket::GuildInfo(info) => {
            assert_eq!(info.name, "Legacy of Steel");
            assert_eq!(info.created_day, 11);
            assert_eq!(info.created_month, 9);
            assert_eq!(info.created_year, 2004);
            assert_eq!(info.member_count, 57);
            assert_eq!(
                info.account_count, 41,
                "accounts, not characters — the second count"
            );
        }
        other => panic!("expected GuildInfo, got {}", other.name()),
    }
    match decode(packet).as_slice() {
        [SessionEvent::GuildInfo(info)] => assert_eq!(info.account_count, 41),
        other => panic!("guild info decode: {other:?}"),
    }
}

/// `SMSG_GUILD_INVITE` (Server/Packets/Guild.cpp:85-89) is inviter then guild, two cstrings;
/// `SMSG_GUILD_DECLINE` (`:91-94`) is one cstring, the decliner.
#[test]
fn guild_invite_and_decline_wire() {
    let packet = messages::parse_server(
        opcode::SMSG_GUILD_INVITE,
        &hx(concat!("416c69636500", "4c656761637900")),
    )
    .unwrap();
    match &packet {
        ServerPacket::GuildInvite { inviter, guild } => {
            assert_eq!(inviter, "Alice", "the inviter comes first");
            assert_eq!(guild, "Legacy");
        }
        other => panic!("expected GuildInvite, got {}", other.name()),
    }
    match decode(packet).as_slice() {
        [SessionEvent::GuildInvite { inviter, guild }] => {
            assert_eq!(inviter, "Alice");
            assert_eq!(guild, "Legacy");
        }
        other => panic!("guild invite decode: {other:?}"),
    }

    let packet = messages::parse_server(opcode::SMSG_GUILD_DECLINE, &hx("426f6200")).unwrap();
    match &packet {
        ServerPacket::GuildDecline { name } => assert_eq!(name, "Bob"),
        other => panic!("expected GuildDecline, got {}", other.name()),
    }
    match decode(packet).as_slice() {
        [SessionEvent::GuildDecline { name }] => assert_eq!(name, "Bob"),
        other => panic!("guild decline decode: {other:?}"),
    }
}
