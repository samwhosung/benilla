//! The chat and channel wire: every `SMSG_MESSAGECHAT` shape, the emotes, the channel notices and
//! list, and the small chat replies, bytes built from the vmangos layout.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, ChannelNoticeTail};
use benilla_protocol::ServerPacket;
use common::hx;

/// The five `SMSG_MESSAGECHAT` shapes (vmangos `Chat/Chat.cpp:2542-2599`): SAY/PARTY/YELL send
/// the sender guid twice; MONSTER_SAY/YELL a guid, a length-prefixed name and a target guid;
/// CHANNEL a cstring channel, a `u32` rank and a guid; MONSTER_WHISPER, the raid-boss types and
/// MONSTER_EMOTE the name and target guid only; every other type one sender guid. Each ends with a
/// `u32` length (NUL included), the text and a `u8` chat tag (`Chat/Chat.h:86-92`).
#[test]
fn message_chat_decodes_every_wire_shape() {
    let body = hx("0007000000887766554433221188776655443322110900000068692074686572650001");
    match messages::parse_server(messages::opcode::SMSG_MESSAGECHAT, &body).unwrap() {
        ServerPacket::MessageChat(m) => {
            assert_eq!(m.chat_type, messages::CHAT_MSG_SAY);
            assert_eq!(m.language, 7);
            assert_eq!(m.sender_guid, 0x1122_3344_5566_7788);
            assert_eq!(m.sender_name, None);
            assert_eq!(m.channel, None);
            assert_eq!(m.text, "hi there");
            assert_eq!(m.chat_tag, messages::chat_tag::AFK);
        }
        _ => panic!("expected MessageChat"),
    }

    let body = hx("0b00000000ddccbbaa000000000f00000054696d6d792074686520576f6c6600667788990000000005000000477272210000");
    match messages::parse_server(messages::opcode::SMSG_MESSAGECHAT, &body).unwrap() {
        ServerPacket::MessageChat(m) => {
            assert_eq!(m.chat_type, messages::CHAT_MSG_MONSTER_SAY);
            assert_eq!(m.sender_guid, 0xAABB_CCDD);
            // The target guid is what the reference's `$`-macro expander resolves the line against.
            assert_eq!(m.target_guid, 0x9988_7766);
            assert_eq!(m.sender_name.as_deref(), Some("Timmy the Wolf"));
            assert_eq!(m.channel, None);
            assert_eq!(m.text, "Grr!");
            assert_eq!(m.chat_tag, messages::chat_tag::NONE);
        }
        _ => panic!("expected MessageChat"),
    }

    let body = hx("0e0000000047656e6572616c000000000034120000000000001000000077746220626f6172206c69766572730000");
    match messages::parse_server(messages::opcode::SMSG_MESSAGECHAT, &body).unwrap() {
        ServerPacket::MessageChat(m) => {
            assert_eq!(m.chat_type, messages::CHAT_MSG_CHANNEL);
            assert_eq!(m.channel.as_deref(), Some("General"));
            assert_eq!(m.sender_guid, 0x1234);
            assert_eq!(m.sender_name, None);
            assert_eq!(m.text, "wtb boar livers");
        }
        _ => panic!("expected MessageChat"),
    }

    let body = hx("1a000000000e000000496e6e6b656570657220426f620055000000000000001900000057656c636f6d652c2077656172792074726176656c6572210000");
    match messages::parse_server(messages::opcode::SMSG_MESSAGECHAT, &body).unwrap() {
        ServerPacket::MessageChat(m) => {
            assert_eq!(m.chat_type, messages::CHAT_MSG_MONSTER_WHISPER);
            assert_eq!(m.sender_guid, 0, "no leading guid on this shape");
            assert_eq!(m.target_guid, 0x55, "the whispered-at player");
            assert_eq!(m.sender_name.as_deref(), Some("Innkeeper Bob"));
            assert_eq!(m.text, "Welcome, weary traveler!");
        }
        _ => panic!("expected MessageChat"),
    }

    let body = hx(
        "0a0000000000000000000000001a0000005468697320697320612073797374656d206d6573736167652e0003",
    );
    match messages::parse_server(messages::opcode::SMSG_MESSAGECHAT, &body).unwrap() {
        ServerPacket::MessageChat(m) => {
            assert_eq!(m.chat_type, messages::CHAT_MSG_SYSTEM);
            assert_eq!(m.sender_guid, 0);
            assert_eq!(m.text, "This is a system message.");
            assert_eq!(m.chat_tag, messages::chat_tag::GM);
        }
        _ => panic!("expected MessageChat"),
    }
}

/// `SMSG_TEXT_EMOTE` (vmangos `Handlers/ChatHandler.cpp:681-702`): unpacked `u64` guid, `u32`
/// textEmote, `u32` emoteNum, `u32` namelen and the target name with its NUL; `SMSG_EMOTE`
/// (`Server/Packets/Misc.cpp:670-674`): `u32` emoteId and `u64` guid.
#[test]
fn text_emote_and_emote_decode() {
    let body = hx("7700000000000000650000000000000004000000426f6200");
    let p = messages::parse_server(messages::opcode::SMSG_TEXT_EMOTE, &body).unwrap();
    // The target name picks the sentence form; the NUL counted in `namelen` is trimmed.
    assert!(matches!(
        &p,
        ServerPacket::TextEmote {
            guid: 0x77,
            text_emote: 101,
            target_name,
        } if target_name == "Bob"
    ));
    assert!(matches!(
        &decode(p)[..],
        [SessionEvent::TextEmote {
            guid: 0x77,
            text_emote: 101,
            target_name,
        }] if target_name == "Bob"
    ));

    // Untargeted, vmangos sends `namelen == 1` and a lone NUL: an empty name, not a one-byte one.
    let body = hx("770000000000000065000000000000000100000000");
    assert!(matches!(
        messages::parse_server(messages::opcode::SMSG_TEXT_EMOTE, &body).unwrap(),
        ServerPacket::TextEmote { ref target_name, .. } if target_name.is_empty()
    ));

    let body = hx("060000008800000000000000");
    let p = messages::parse_server(messages::opcode::SMSG_EMOTE, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::Emote {
            guid: 0x88,
            emote_id: 6,
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::Emote {
            guid: 0x88,
            emote_id: 6,
        }]
    ));
}

/// One `SMSG_CHANNEL_NOTIFY` per tail shape (vmangos `Chat/Channel.cpp:804-1008`).
#[test]
fn channel_notify_decodes_each_tail_shape() {
    let body = hx("0047656e6572616c00aa00000000000000");
    match messages::parse_server(messages::opcode::SMSG_CHANNEL_NOTIFY, &body).unwrap() {
        ServerPacket::ChannelNotify(n) => {
            assert_eq!(n.notice, messages::channel_notice::JOINED);
            assert_eq!(n.channel, "General");
            assert_eq!(n.tail, ChannelNoticeTail::Guid(0xAA));
        }
        _ => panic!("expected ChannelNotify"),
    }

    // 0x02 YOU_JOINED: u32 flags + u32 instance (`0x49bb20` stores it on the slot).
    for (body, instance) in [
        ("0247656e6572616c001800000000000000", 0),
        ("0247656e6572616c001800000007000000", 7),
    ] {
        let body = hx(body);
        match messages::parse_server(messages::opcode::SMSG_CHANNEL_NOTIFY, &body).unwrap() {
            ServerPacket::ChannelNotify(n) => {
                assert_eq!(n.notice, messages::channel_notice::YOU_JOINED);
                assert_eq!(
                    n.tail,
                    ChannelNoticeTail::YouJoined {
                        flags: 0x18,
                        instance
                    }
                );
            }
            _ => panic!("expected ChannelNotify"),
        }
    }

    // 0x05 NOT_MEMBER: empty tail; `MakeNotMember` and `MakeNotOnPacket` both send it.
    let body = hx("0553656372657400");
    match messages::parse_server(messages::opcode::SMSG_CHANNEL_NOTIFY, &body).unwrap() {
        ServerPacket::ChannelNotify(n) => {
            assert_eq!(n.notice, messages::channel_notice::NOT_MEMBER);
            assert_eq!(n.channel, "Secret");
            assert_eq!(n.tail, ChannelNoticeTail::Empty);
        }
        _ => panic!("expected ChannelNotify"),
    }

    let body = hx("1847656e6572616c00bb00000000000000");
    match messages::parse_server(messages::opcode::SMSG_CHANNEL_NOTIFY, &body).unwrap() {
        ServerPacket::ChannelNotify(n) => {
            assert_eq!(n.notice, messages::channel_notice::INVITE);
            assert_eq!(n.tail, ChannelNoticeTail::Actor(0xBB));
        }
        _ => panic!("expected ChannelNotify"),
    }

    let body = hx("0947656e6572616c0047686f737400");
    match messages::parse_server(messages::opcode::SMSG_CHANNEL_NOTIFY, &body).unwrap() {
        ServerPacket::ChannelNotify(n) => {
            assert_eq!(n.notice, messages::channel_notice::PLAYER_NOT_FOUND);
            assert_eq!(n.tail, ChannelNoticeTail::Name("Ghost".into()));
        }
        _ => panic!("expected ChannelNotify"),
    }

    // 0x0B CHANNEL_OWNER: the "Nobody" literal vmangos sends for an ownerless non-constant channel.
    let body = hx("0b47656e6572616c004e6f626f647900");
    match messages::parse_server(messages::opcode::SMSG_CHANNEL_NOTIFY, &body).unwrap() {
        ServerPacket::ChannelNotify(n) => {
            assert_eq!(n.notice, messages::channel_notice::CHANNEL_OWNER);
            assert_eq!(n.tail, ChannelNoticeTail::Name("Nobody".into()));
        }
        _ => panic!("expected ChannelNotify"),
    }

    let body = hx("0c47656e6572616c00cc000000000000000002");
    match messages::parse_server(messages::opcode::SMSG_CHANNEL_NOTIFY, &body).unwrap() {
        ServerPacket::ChannelNotify(n) => {
            assert_eq!(n.notice, messages::channel_notice::MODE_CHANGE);
            assert_eq!(
                n.tail,
                ChannelNoticeTail::ModeChange {
                    guid: 0xCC,
                    old_flags: 0x00,
                    new_flags: 0x02,
                }
            );
        }
        _ => panic!("expected ChannelNotify"),
    }

    let body = hx("1247656e6572616c00dd00000000000000ee00000000000000");
    match messages::parse_server(messages::opcode::SMSG_CHANNEL_NOTIFY, &body).unwrap() {
        ServerPacket::ChannelNotify(n) => {
            assert_eq!(n.notice, messages::channel_notice::PLAYER_KICKED);
            assert_eq!(
                n.tail,
                ChannelNoticeTail::Actors {
                    target: 0xDD,
                    source: 0xEE,
                }
            );
        }
        _ => panic!("expected ChannelNotify"),
    }

    let body = hx("0047656e6572616c00aa00000000000000");
    let p = messages::parse_server(messages::opcode::SMSG_CHANNEL_NOTIFY, &body).unwrap();
    match &decode(p)[..] {
        [SessionEvent::ChannelNotify {
            notice,
            channel,
            tail,
        }] => {
            assert_eq!(*notice, messages::channel_notice::JOINED);
            assert_eq!(channel, "General");
            assert_eq!(*tail, ChannelNoticeTail::Guid(0xAA));
        }
        other => panic!("channel notify decode: {} events", other.len()),
    }

    // A notice byte past vmangos's 0x00..=0x1F errors rather than guessing a tail.
    let body = hx("2047656e6572616c00");
    assert!(messages::parse_server(messages::opcode::SMSG_CHANNEL_NOTIFY, &body).is_err());
}

/// `SMSG_CHANNEL_LIST` (vmangos `Chat/Channel.cpp:513-556`): cstring channel, `u8` flags, `u32`
/// count, then `(u64 guid, u8 memberFlags)` rows.
#[test]
fn channel_list_decodes_roster() {
    let body = hx("47656e6572616c001802000000011000000000000001021000000000000000");
    match messages::parse_server(messages::opcode::SMSG_CHANNEL_LIST, &body).unwrap() {
        ServerPacket::ChannelList {
            channel,
            flags,
            members,
        } => {
            assert_eq!(channel, "General");
            assert_eq!(flags, 0x18);
            assert_eq!(members, vec![(0x1001, 0x01), (0x1002, 0x00)]);
        }
        _ => panic!("expected ChannelList"),
    }
}

/// `SMSG_CHAT_PLAYER_NOT_FOUND` (cstring name, vmangos `Server/Packets/Chat.cpp:26-29`) and
/// `SMSG_CHAT_WRONG_FACTION` (empty body, `Server/Packets/Chat.cpp:16-18`).
#[test]
fn chat_player_not_found_and_wrong_faction_decode() {
    let body = hx("47686f73746e616d6500");
    match messages::parse_server(messages::opcode::SMSG_CHAT_PLAYER_NOT_FOUND, &body).unwrap() {
        ServerPacket::ChatPlayerNotFound { name } => assert_eq!(name, "Ghostname"),
        _ => panic!("expected ChatPlayerNotFound"),
    }
    assert!(matches!(
        decode(
            messages::parse_server(messages::opcode::SMSG_CHAT_PLAYER_NOT_FOUND, &body).unwrap()
        )[..],
        [SessionEvent::ChatPlayerNotFound { .. }]
    ));

    let p = messages::parse_server(messages::opcode::SMSG_CHAT_WRONG_FACTION, &[]).unwrap();
    assert!(matches!(p, ServerPacket::ChatWrongFaction));
    assert!(matches!(decode(p)[..], [SessionEvent::ChatWrongFaction]));
}

/// `SMSG_NOTIFICATION` (vmangos `Server/WorldSession.cpp:900-915`): one cstring, formatted by the
/// server; a send in a language the sender does not know fails with this reply.
#[test]
fn notification_decodes() {
    let body = hx("596f7520646f206e6f74206b6e6f772074686174206c616e677561676500");
    match messages::parse_server(messages::opcode::SMSG_NOTIFICATION, &body).unwrap() {
        ServerPacket::Notification { ref text } => {
            assert_eq!(text, "You do not know that language")
        }
        other => panic!("expected Notification, got {}", other.name()),
    }
    match &decode(messages::parse_server(messages::opcode::SMSG_NOTIFICATION, &body).unwrap())[..] {
        [SessionEvent::Notification { text }] => assert_eq!(text, "You do not know that language"),
        other => panic!("notification decode: {} events", other.len()),
    }
}

/// `SMSG_PLAYED_TIME` (vmangos `Server/Packets/Misc.cpp:278-282`): total and level, `u32` seconds.
#[test]
fn played_time_decodes() {
    let body = hx("40e20100100e0000"); // total = 123456, level = 3600
    let p = messages::parse_server(messages::opcode::SMSG_PLAYED_TIME, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::PlayedTime {
            total: 123_456,
            level: 3600,
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PlayedTime {
            total: 123_456,
            level: 3600,
        }]
    ));
}

/// `MSG_RANDOM_ROLL`'s broadcast (vmangos `Handlers/GroupHandler.cpp:394-422`): `u32` min, max
/// and roll, then an unpacked `u64` guid.
#[test]
fn random_roll_broadcast_decodes() {
    let body = hx("01000000640000002a0000009999000000000000");
    let p = messages::parse_server(messages::opcode::MSG_RANDOM_ROLL, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::RandomRoll {
            min: 1,
            max: 100,
            roll: 42,
            guid: 0x9999,
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::RandomRoll {
            min: 1,
            max: 100,
            roll: 42,
            guid: 0x9999,
        }]
    ));
}

/// The `$`-macro types are the reference's wire gate: parser `0x49d560` sends 0x0B, 0x0C, 0x0D,
/// 0x1A and 0x5A to the expander at `0x49da3d`, and 0x52 to 0x54 to the one at `0x49d961`. The
/// chain at `0x49cf36-0x49cf5d` is the pending-chat gate, which omits 0x5A; it is not this one.
#[test]
fn the_macro_expanded_chat_types_are_the_reference_wire_gate() {
    let mut got = messages::MACRO_EXPANDED_TYPES;
    got.sort_unstable();
    assert_eq!(got, [0x0B, 0x0C, 0x0D, 0x1A, 0x52, 0x53, 0x54, 0x5A]);
    // 0x59 RAID_BOSS_WHISPER takes its own branch (`0x49d610`): never expanded, remapped to a
    // plain whisper.
    for t in [
        messages::CHAT_MSG_SAY,
        messages::CHAT_MSG_YELL,
        messages::CHAT_MSG_WHISPER,
        messages::CHAT_MSG_GUILD,
        messages::CHAT_MSG_CHANNEL,
        messages::CHAT_MSG_SYSTEM,
        messages::CHAT_MSG_RAID_BOSS_WHISPER,
    ] {
        assert!(
            !messages::MACRO_EXPANDED_TYPES.contains(&t),
            "type {t:#04x} must reach the frame verbatim"
        );
    }
}

/// 1.12.1 has no addon opcode or chat type: addon chat is ordinary chat in language `LANG_ADDON`
/// (`0xFFFFFFFF`), its payload `prefix`, a TAB and the message, which vmangos passes through
/// unsanitized (`Handlers/ChatHandler.cpp:49`). The bytes are what `addon_chat_probe` reads back.
#[test]
fn addon_chat_is_an_ordinary_type_with_the_sentinel_language() {
    let body = hx("01ffffffff21000000000000002100000000000000150000005175697665720956455253494f4e3a332e312e340000");
    match messages::parse_server(messages::opcode::SMSG_MESSAGECHAT, &body).unwrap() {
        ServerPacket::MessageChat(m) => {
            assert_eq!(
                m.chat_type,
                messages::CHAT_MSG_PARTY,
                "no addon-only type byte"
            );
            assert_eq!(m.language, messages::LANGUAGE_ADDON);
            assert_eq!(m.text, "Quiver\tVERSION:3.1.4");
            assert!(m.is_addon());
        }
        _ => panic!("expected MessageChat"),
    }
    // Party speech as vmangos sends it back: `HandleChatMessageOpcode` rewrites the language to
    // LANG_UNIVERSAL (0) for GMs and two-side-enabled lanes, and never rewrites LANG_ADDON.
    let body = hx("01000000002100000000000000210000000000000013000000706172747920636f6e74726f6c206c696e650000");
    match messages::parse_server(messages::opcode::SMSG_MESSAGECHAT, &body).unwrap() {
        ServerPacket::MessageChat(m) => {
            assert_eq!(m.chat_type, messages::CHAT_MSG_PARTY);
            assert_eq!(m.text, "party control line");
            assert!(!m.is_addon(), "speech must survive the addon gate");
        }
        _ => panic!("expected MessageChat"),
    }
    // Every speakable language is speech: the gate is equality with 0xFFFFFFFF, not a range.
    for lang in [0, 1, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 33] {
        let m = messages::ChatMessage {
            chat_type: messages::CHAT_MSG_PARTY,
            language: lang,
            sender_guid: 0x21,
            target_guid: 0x21,
            sender_name: None,
            channel: None,
            text: "hello".into(),
            chat_tag: 0,
        };
        assert!(
            !m.is_addon(),
            "language {lang} is a tongue, not the sentinel"
        );
    }
}
