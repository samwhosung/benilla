//! The client-built bodies (auth, char create, chat, emotes, channels, movement, ping) and the
//! name and creature query replies.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, MovementInfo};
use benilla_protocol::wire::Vector3d;
use benilla_protocol::ServerPacket;
use common::hx;

/// Alliance races learn Common (spell 668), Horde races Orcish (669) in `playercreateinfo_spell`;
/// `LANG_COMMON = 7`, `LANG_ORCISH = 1` (vmangos `SharedDefines.h:256-261`).
#[test]
fn faction_language_per_race() {
    for race in [1u8, 3, 4, 7] {
        assert_eq!(
            messages::faction_language(race),
            messages::LANGUAGE_COMMON,
            "alliance race {race}"
        );
    }
    for race in [2u8, 5, 6, 8] {
        assert_eq!(
            messages::faction_language(race),
            messages::LANGUAGE_ORCISH,
            "horde race {race}"
        );
    }
}

#[test]
fn client_bodies_golden() {
    let proof: [u8; 20] = std::array::from_fn(|i| (i as u8).wrapping_mul(3).wrapping_add(7));
    // The tail is the addon block: `u32` 342 uncompressed, then the zlib stream of the stock
    // twelve, the same 130 bytes a retail 1.12.1 client sends.
    assert_eq!(
        messages::auth_session(
            5875,
            "TESTUSER",
            0x1122_3344,
            &proof,
            &messages::STOCK_SECURE_ADDONS
        ),
        hx(concat!(
            "f31600000000000054455354555345520044332211070a0d101316191c1f2225282b2e3134373a3d40",
            "56010000",
            "789c75ccbd0ec2300c04e0f21ebc0c614095c842c38c4ce2220bc7a98ccb4f9f1e16240673eb777781",
            "695940cb693367a326c7be5bd5c77adf7d12be16c08c7124e41249a8c2e495480ac9c53dd8b67a064b",
            "f8340f15467367bb38cc7ac7978bbddc26ccfe3042d6e6ca01a8b8908051fcb7a45070b812f33f2641",
            "fdb5379019668f",
        )),
        "CMSG_AUTH_SESSION body"
    );
    // No secure addons: the tail is absent, never a zero size, which no real client sends.
    assert_eq!(
        messages::auth_session(5875, "TESTUSER", 0x1122_3344, &proof, &[]),
        hx("f31600000000000054455354555345520044332211070a0d101316191c1f2225282b2e3134373a3d40"),
        "CMSG_AUTH_SESSION body with no secure addons"
    );
    // CMSG_CHAR_CREATE in vmangos's read order (`Packets/Character.cpp:4-19`): name, race, class,
    // gender, skin, face, hairStyle, hairColor, facialHair, outfit.
    assert_eq!(
        messages::char_create(&messages::CharCreateReq {
            name: "Benilla".into(),
            race: 1,
            class: 1,
            gender: 0,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
        }),
        hx("42656e696c6c6100010100000000000000"),
        "CMSG_CHAR_CREATE body (zero appearance)"
    );
    assert_eq!(
        messages::char_create(&messages::CharCreateReq {
            name: "Benilla".into(),
            race: 1,
            class: 1,
            gender: 0,
            skin: 3,
            face: 4,
            hair_style: 5,
            hair_color: 6,
            facial_hair: 7,
        }),
        hx("42656e696c6c6100010100030405060700"),
        "CMSG_CHAR_CREATE body (distinct appearance)"
    );
    assert_eq!(
        messages::messagechat(0, 7, ".tele Westfall"),
        hx("00000000070000002e74656c65205765737466616c6c00"),
        "CMSG_MESSAGECHAT body"
    );
    // CMSG_TEXT_EMOTE (vmangos `Misc.cpp:60-65`): textEmote, emoteNum (0) and the full target
    // guid; the server silently drops a body without the guid.
    assert_eq!(
        messages::text_emote(101, 0x2A),
        hx("65000000000000002a00000000000000"),
        "CMSG_TEXT_EMOTE body"
    );
    assert_eq!(
        messages::stand_state_change(1),
        hx("01000000"),
        "CMSG_STANDSTATECHANGE body: one u32 animState (Misc.cpp:35-38)"
    );
    // The CMSG_MESSAGECHAT type values (vmangos `SharedDefines.h:1194..1202`).
    assert_eq!(messages::CHAT_TYPE_SAY, 0x0, "ChatMsg::CHAT_MSG_SAY");
    assert_eq!(messages::CHAT_TYPE_YELL, 0x5, "ChatMsg::CHAT_MSG_YELL");
    assert_eq!(
        messages::CHAT_TYPE_WHISPER,
        0x6,
        "ChatMsg::CHAT_MSG_WHISPER"
    );
    assert_eq!(messages::CHAT_TYPE_EMOTE, 0x8, "ChatMsg::CHAT_MSG_EMOTE");
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_YELL, 7, "for the horde"),
        hx("0500000007000000666f722074686520686f72646500"),
        "CMSG_MESSAGECHAT (yell) body"
    );
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_EMOTE, 7, "dances"),
        hx("080000000700000064616e63657300"),
        "CMSG_MESSAGECHAT (emote) body"
    );
    // A Horde say speaks Orcish (1): vmangos drops a send in an unknown language at its
    // `KnowsLanguage` gate, dot-commands included, and answers only with SMSG_NOTIFICATION.
    assert_eq!(
        messages::messagechat(
            messages::CHAT_TYPE_SAY,
            messages::LANGUAGE_ORCISH,
            "for the horde"
        ),
        hx("0000000001000000666f722074686520686f72646500"),
        "CMSG_MESSAGECHAT (say, Orcish) body"
    );
    // Whisper: type, language, then the target and the message cstrings, target first (vmangos
    // `Server/Packets/Chat.cpp:3-12`).
    assert_eq!(
        messages::messagechat_whisper(7, "Bob", "hi there"),
        hx("0600000007000000426f6200686920746865726500"),
        "CMSG_MESSAGECHAT (whisper) body"
    );
    // CMSG_PLAYER_LOGIN, CMSG_SET_ACTIVE_MOVER and CMSG_SET_SELECTION all read a raw `u64` guid,
    // not a packed one.
    assert_eq!(
        messages::full_guid(0x1234_5678_9abc_def0),
        hx("f0debc9a78563412"),
        "full guid body"
    );
    // CMSG_SET_SELECTION is opcode 317 (vmangos `Opcodes_1_12_1.h`).
    assert_eq!(
        messages::opcode::CMSG_SET_SELECTION,
        0x013D,
        "CMSG_SET_SELECTION opcode"
    );
    // CMSG_INSPECT is opcode 276 (vmangos `Opcodes_1_12_1.h`), its body a raw guid, not packed.
    assert_eq!(
        messages::opcode::CMSG_INSPECT,
        0x0114,
        "CMSG_INSPECT opcode"
    );
    assert_eq!(
        messages::full_guid(0x1234_5678_9abc_def0),
        hx("f0debc9a78563412"),
        "CMSG_INSPECT body (a raw guid, same shape as full_guid)"
    );
    let mi = MovementInfo {
        flags: 0x1,
        timestamp: 0x0102_0304,
        position: Vector3d {
            x: -8949.95,
            y: -132.493,
            z: 83.5312,
        },
        orientation: 1.25,
        transport: None,
        pitch: 0.0,
        fall_time: 0,
        jump: None,
    };
    assert_eq!(
        messages::movement(&mi),
        hx("0100000004030201cdd70bc6357e04c3f90fa7420000a03f00000000"),
        "MSG_MOVE_* body"
    );
    assert_eq!(
        messages::teleport_ack(0x1234_5678_9abc_def0, 7, 0x0102_0304),
        hx("f0debc9a785634120700000004030201"),
        "MSG_MOVE_TELEPORT_ACK body"
    );
}

/// The other sendable `CMSG_MESSAGECHAT` types (vmangos `Handlers/ChatHandler.cpp:253-655`) use
/// the plain shape, except CHANNEL, which carries its channel name where WHISPER carries its
/// target (`Server/Packets/Chat.cpp:3-12`).
#[test]
fn messagechat_sendable_types_golden() {
    assert_eq!(messages::CHAT_TYPE_PARTY, 0x1, "ChatMsg::CHAT_MSG_PARTY");
    assert_eq!(messages::CHAT_TYPE_RAID, 0x2, "ChatMsg::CHAT_MSG_RAID");
    assert_eq!(messages::CHAT_TYPE_GUILD, 0x3, "ChatMsg::CHAT_MSG_GUILD");
    assert_eq!(
        messages::CHAT_TYPE_OFFICER,
        0x4,
        "ChatMsg::CHAT_MSG_OFFICER"
    );
    assert_eq!(
        messages::CHAT_TYPE_CHANNEL,
        0xE,
        "ChatMsg::CHAT_MSG_CHANNEL"
    );
    assert_eq!(messages::CHAT_TYPE_AFK, 0x14, "ChatMsg::CHAT_MSG_AFK");
    assert_eq!(messages::CHAT_TYPE_DND, 0x15, "ChatMsg::CHAT_MSG_DND");
    assert_eq!(
        messages::CHAT_TYPE_RAID_LEADER,
        0x57,
        "ChatMsg::CHAT_MSG_RAID_LEADER"
    );
    assert_eq!(
        messages::CHAT_TYPE_RAID_WARNING,
        0x58,
        "ChatMsg::CHAT_MSG_RAID_WARNING"
    );
    assert_eq!(
        messages::CHAT_TYPE_BATTLEGROUND,
        0x5C,
        "ChatMsg::CHAT_MSG_BATTLEGROUND"
    );
    assert_eq!(
        messages::CHAT_TYPE_BATTLEGROUND_LEADER,
        0x5D,
        "ChatMsg::CHAT_MSG_BATTLEGROUND_LEADER"
    );

    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_PARTY, 7, "MT is icon 8"),
        hx("01000000070000004d542069732069636f6e203800"),
        "CMSG_MESSAGECHAT (party) body"
    );
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_RAID, 7, "form up"),
        hx("0200000007000000666f726d20757000"),
        "CMSG_MESSAGECHAT (raid) body"
    );
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_GUILD, 7, "hi guild"),
        hx("03000000070000006869206775696c6400"),
        "CMSG_MESSAGECHAT (guild) body"
    );
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_OFFICER, 7, "officers only"),
        hx("04000000070000006f66666963657273206f6e6c7900"),
        "CMSG_MESSAGECHAT (officer) body"
    );
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_RAID_LEADER, 7, "pull in 5"),
        hx("570000000700000070756c6c20696e203500"),
        "CMSG_MESSAGECHAT (raid leader) body"
    );
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_RAID_WARNING, 7, "BLOODLUST NOW"),
        hx("5800000007000000424c4f4f444c555354204e4f5700"),
        "CMSG_MESSAGECHAT (raid warning) body"
    );
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_BATTLEGROUND, 7, "defend flag"),
        hx("5c00000007000000646566656e6420666c616700"),
        "CMSG_MESSAGECHAT (battleground) body"
    );
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_BATTLEGROUND_LEADER, 7, "push mid"),
        hx("5d0000000700000070757368206d696400"),
        "CMSG_MESSAGECHAT (battleground leader) body"
    );
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_AFK, 7, "be back soon"),
        hx("14000000070000006265206261636b20736f6f6e00"),
        "CMSG_MESSAGECHAT (afk) body"
    );
    // A bare `/afk` (no message) still needs the NUL-terminated empty string, not a truncated body.
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_AFK, 7, ""),
        hx("140000000700000000"),
        "CMSG_MESSAGECHAT (afk, empty message) body"
    );
    assert_eq!(
        messages::messagechat(messages::CHAT_TYPE_DND, 7, "do not disturb"),
        hx("1500000007000000646f206e6f74206469737475726200"),
        "CMSG_MESSAGECHAT (dnd) body"
    );
    assert_eq!(
        messages::messagechat_channel(0, "General", "wtb boar livers"),
        hx("0e0000000000000047656e6572616c0077746220626f6172206c697665727300"),
        "CMSG_MESSAGECHAT (channel) body"
    );
    assert_eq!(
        messages::messagechat_kind(messages::CHAT_TYPE_SAY, 7, None, "hi"),
        messages::messagechat(messages::CHAT_TYPE_SAY, 7, "hi"),
        "messagechat_kind(None) == messagechat"
    );
    assert_eq!(
        messages::messagechat_kind(messages::CHAT_TYPE_WHISPER, 7, Some("Bob"), "hi"),
        messages::messagechat_whisper(7, "Bob", "hi"),
        "messagechat_kind(Some) == messagechat_whisper"
    );
}

/// `SendAddonMessage` (reference `0x49f920`) sends `CMSG_MESSAGECHAT` on one of four lanes: `u32`
/// type, `u32` language `LANG_ADDON`, then one cstring of prefix, TAB and message (`0x49f9b3`);
/// there is no prefix or target field. The receiver splits on the first TAB (`0x49a8d0`); vmangos
/// relays the TAB intact, as it skips `SanitizeChatMessage` for addon chat (`ChatHandler.cpp:49`).
#[test]
fn addon_message_bodies_golden() {
    assert_eq!(
        messages::LANGUAGE_ADDON,
        0xFFFF_FFFF,
        "LANG_ADDON (vmangos SharedDefines.h:270)"
    );

    // oRA2's `SendAddonMessage("CTRA", msg, "RAID")` (`Core.lua:563`).
    assert_eq!(
        messages::messagechat(
            messages::CHAT_TYPE_RAID,
            messages::LANGUAGE_ADDON,
            "CTRA\tstatus"
        ),
        hx("02000000ffffffff435452410973746174757300"),
        "CMSG_MESSAGECHAT (addon, RAID) body"
    );

    // The other lanes of the reference's four-lane whitelist (`0x49fa3f`-`0x49fa4e`).
    assert_eq!(
        messages::messagechat(
            messages::CHAT_TYPE_PARTY,
            messages::LANGUAGE_ADDON,
            "oRA\thello"
        ),
        hx("01000000ffffffff6f52410968656c6c6f00"),
        "CMSG_MESSAGECHAT (addon, PARTY) body"
    );
    assert_eq!(
        messages::messagechat(
            messages::CHAT_TYPE_GUILD,
            messages::LANGUAGE_ADDON,
            "oRA\thello"
        ),
        hx("03000000ffffffff6f52410968656c6c6f00"),
        "CMSG_MESSAGECHAT (addon, GUILD) body"
    );
    assert_eq!(
        messages::messagechat(
            messages::CHAT_TYPE_BATTLEGROUND,
            messages::LANGUAGE_ADDON,
            "oRA\thello"
        ),
        hx("5c000000ffffffff6f52410968656c6c6f00"),
        "CMSG_MESSAGECHAT (addon, BATTLEGROUND) body"
    );

    // An empty message still carries its TAB: the reference composes `"%s\t%s"` unconditionally,
    // so AceEvent-2.0's `SendAddonMessage("LOOT_OPENED", "", "RAID")` ends TAB, NUL.
    assert_eq!(
        messages::messagechat(
            messages::CHAT_TYPE_RAID,
            messages::LANGUAGE_ADDON,
            "LOOT_OPENED\t"
        ),
        hx("02000000ffffffff4c4f4f545f4f50454e45440900"),
        "CMSG_MESSAGECHAT (addon, empty message keeps its TAB) body"
    );

    let addon = messages::messagechat(messages::CHAT_TYPE_PARTY, messages::LANGUAGE_ADDON, "x\ty");
    let speech =
        messages::messagechat(messages::CHAT_TYPE_PARTY, messages::LANGUAGE_COMMON, "x\ty");
    assert_eq!(addon.len(), speech.len(), "same shape");
    assert_eq!(addon[0..4], speech[0..4], "same chat type");
    assert_eq!(addon[8..], speech[8..], "same payload");
    assert_eq!(&addon[4..8], &[0xFF, 0xFF, 0xFF, 0xFF], "LANG_ADDON");
    assert_eq!(&speech[4..8], &[0x07, 0x00, 0x00, 0x00], "LANG_COMMON");
}

/// The channel CMSG bodies: a channel-name cstring, then optionally a password or target-name
/// cstring (vmangos `Server/Packets/Channel.cpp`).
#[test]
fn channel_client_bodies_golden() {
    assert_eq!(
        messages::join_channel("General", ""),
        hx("47656e6572616c0000"),
        "CMSG_JOIN_CHANNEL body (no password)"
    );
    assert_eq!(
        messages::join_channel("Secret", "hunter2"),
        hx("5365637265740068756e7465723200"),
        "CMSG_JOIN_CHANNEL body (with password)"
    );
    assert_eq!(
        messages::leave_channel("General"),
        hx("47656e6572616c00"),
        "CMSG_LEAVE_CHANNEL body"
    );
    assert_eq!(
        messages::channel_list("Trade"),
        hx("547261646500"),
        "CMSG_CHANNEL_LIST body"
    );
    assert_eq!(
        messages::channel_password("General", "hunter2"),
        hx("47656e6572616c0068756e7465723200"),
        "CMSG_CHANNEL_PASSWORD body"
    );
    assert_eq!(
        messages::channel_set_owner("General", "Bob"),
        hx("47656e6572616c00426f6200"),
        "CMSG_CHANNEL_SET_OWNER body"
    );
    assert_eq!(
        messages::channel_owner("General"),
        hx("47656e6572616c00"),
        "CMSG_CHANNEL_OWNER body"
    );
    assert_eq!(
        messages::channel_moderator("General", "Bob"),
        hx("47656e6572616c00426f6200"),
        "CMSG_CHANNEL_MODERATOR body"
    );
    assert_eq!(
        messages::channel_unmoderator("General", "Bob"),
        hx("47656e6572616c00426f6200"),
        "CMSG_CHANNEL_UNMODERATOR body"
    );
    assert_eq!(
        messages::channel_mute("General", "Bob"),
        hx("47656e6572616c00426f6200"),
        "CMSG_CHANNEL_MUTE body"
    );
    assert_eq!(
        messages::channel_unmute("General", "Bob"),
        hx("47656e6572616c00426f6200"),
        "CMSG_CHANNEL_UNMUTE body"
    );
    assert_eq!(
        messages::channel_invite("General", "Bob"),
        hx("47656e6572616c00426f6200"),
        "CMSG_CHANNEL_INVITE body"
    );
    assert_eq!(
        messages::channel_kick("General", "Bob"),
        hx("47656e6572616c00426f6200"),
        "CMSG_CHANNEL_KICK body"
    );
    assert_eq!(
        messages::channel_ban("General", "Bob"),
        hx("47656e6572616c00426f6200"),
        "CMSG_CHANNEL_BAN body"
    );
    assert_eq!(
        messages::channel_unban("General", "Bob"),
        hx("47656e6572616c00426f6200"),
        "CMSG_CHANNEL_UNBAN body"
    );
    assert_eq!(
        messages::channel_announcements("General"),
        hx("47656e6572616c00"),
        "CMSG_CHANNEL_ANNOUNCEMENTS body"
    );
    assert_eq!(
        messages::channel_moderate("General"),
        hx("47656e6572616c00"),
        "CMSG_CHANNEL_MODERATE body"
    );

    // The family's opcodes, 151 to 168 (vmangos `Server/Protocol/Opcodes_1_12_1.h:154-171`).
    assert_eq!(messages::opcode::CMSG_JOIN_CHANNEL, 0x0097);
    assert_eq!(messages::opcode::CMSG_LEAVE_CHANNEL, 0x0098);
    assert_eq!(messages::opcode::SMSG_CHANNEL_NOTIFY, 0x0099);
    assert_eq!(messages::opcode::CMSG_CHANNEL_LIST, 0x009A);
    assert_eq!(messages::opcode::SMSG_CHANNEL_LIST, 0x009B);
    assert_eq!(messages::opcode::CMSG_CHANNEL_PASSWORD, 0x009C);
    assert_eq!(messages::opcode::CMSG_CHANNEL_SET_OWNER, 0x009D);
    assert_eq!(messages::opcode::CMSG_CHANNEL_OWNER, 0x009E);
    assert_eq!(messages::opcode::CMSG_CHANNEL_MODERATOR, 0x009F);
    assert_eq!(messages::opcode::CMSG_CHANNEL_UNMODERATOR, 0x00A0);
    assert_eq!(messages::opcode::CMSG_CHANNEL_MUTE, 0x00A1);
    assert_eq!(messages::opcode::CMSG_CHANNEL_UNMUTE, 0x00A2);
    assert_eq!(messages::opcode::CMSG_CHANNEL_INVITE, 0x00A3);
    assert_eq!(messages::opcode::CMSG_CHANNEL_KICK, 0x00A4);
    assert_eq!(messages::opcode::CMSG_CHANNEL_BAN, 0x00A5);
    assert_eq!(messages::opcode::CMSG_CHANNEL_UNBAN, 0x00A6);
    assert_eq!(messages::opcode::CMSG_CHANNEL_ANNOUNCEMENTS, 0x00A7);
    assert_eq!(messages::opcode::CMSG_CHANNEL_MODERATE, 0x00A8);
}

/// `CMSG_CHAT_IGNORED` is a raw guid (vmangos `Server/Packets/Misc.cpp:127-130`),
/// `CMSG_PLAYED_TIME` is empty, and the `MSG_RANDOM_ROLL` request is `u32` min and max
/// (`Server/Packets/Group.cpp:39-43`).
#[test]
fn chat_ignored_played_time_random_roll_golden() {
    assert_eq!(
        messages::full_guid(0x1234_5678_9abc_def0),
        hx("f0debc9a78563412"),
        "CMSG_CHAT_IGNORED body (a raw guid, same shape as full_guid)"
    );
    assert_eq!(messages::opcode::CMSG_CHAT_IGNORED, 0x0225);

    assert_eq!(
        messages::played_time(),
        Vec::<u8>::new(),
        "CMSG_PLAYED_TIME body: empty"
    );
    assert_eq!(messages::opcode::CMSG_PLAYED_TIME, 0x01CC);
    assert_eq!(messages::opcode::SMSG_PLAYED_TIME, 0x01CD);

    assert_eq!(
        messages::random_roll(1, 100),
        hx("0100000064000000"),
        "MSG_RANDOM_ROLL (request) body"
    );
    assert_eq!(messages::opcode::MSG_RANDOM_ROLL, 0x01FB);
}

#[test]
fn name_and_creature_query_roundtrip() {
    // CMSG_CREATURE_QUERY: `u32` entry and the full guid (vmangos `QueryCreature`).
    assert_eq!(
        messages::creature_query(69, 0x1234_5678_9abc_def0),
        hx("45000000f0debc9a78563412"),
        "CMSG_CREATURE_QUERY body"
    );

    // CMSG_NAME_QUERY: the full guid (vmangos `QueryPlayerName`), built by `full_guid`.
    assert_eq!(
        messages::full_guid(7),
        hx("0700000000000000"),
        "CMSG_NAME_QUERY body"
    );

    // SMSG_NAME_QUERY_RESPONSE (vmangos `NameQueryResponse::AppendBodyTo`): guid, name, an empty
    // realm cstring, then race, gender and class as `u32`.
    let body = hx("070000000000000042656e696c6c610000010000000000000001000000");
    match messages::parse_server(messages::opcode::SMSG_NAME_QUERY_RESPONSE, &body).unwrap() {
        ServerPacket::NameQueryResponse {
            guid,
            name,
            race,
            gender,
            class,
        } => {
            assert_eq!(guid, 7);
            assert_eq!(name, "Benilla");
            assert_eq!((race, gender, class), (1, 0, 1));
        }
        _ => panic!("name query response"),
    }

    // A hit. Family, rank and display id get distinct non-zero values so a one-dword slip between
    // adjacent fields cannot read as plausible data.
    let body = hx(concat!(
        "45000000",
        "596f756e6720576f6c6600", // "Young Wolf"
        "000000",                 // name2..4 empty
        "5465737400",             // subname "Test"
        "10000000",               // type_flags = 0x10 (hide-faction-tooltip)
        "01000000",               // type = 1 (Beast)
        "01000000",               // pet_family = 1 (Wolf)
        "02000000",               // rank = 2 (rare elite)
        "0000000000000000",       // unk, pet_spell_list_id
        "15020000",               // display_id = 533, the model a stabled pet is drawn from
        "0101"                    // civilian, racial_leader
    ));
    match messages::parse_server(messages::opcode::SMSG_CREATURE_QUERY_RESPONSE, &body).unwrap() {
        ServerPacket::CreatureQueryResponse { entry, info } => {
            assert_eq!(entry, 69);
            assert_eq!(
                info,
                Some(benilla_protocol::messages::CreatureQueryInfo {
                    name: "Young Wolf".into(),
                    subname: "Test".into(),
                    creature_type: 1,
                    pet_family: 1,
                    rank: 2,
                    type_flags: 0x10,
                    civilian: true,
                    racial_leader: true,
                    display_id: 533,
                })
            );
        }
        _ => panic!("creature query response"),
    }

    // Miss: the lone entry echoed with the top bit set (vmangos "NO CREATURE INFO" branch).
    let body = hx("d2040080");
    match messages::parse_server(messages::opcode::SMSG_CREATURE_QUERY_RESPONSE, &body).unwrap() {
        ServerPacket::CreatureQueryResponse { entry, info } => {
            assert_eq!(entry, 1234);
            assert_eq!(info, None);
        }
        _ => panic!("creature query miss"),
    }
}

/// An empty wire subname is no subname: vmangos sends "" for a creature without one, and the
/// reference renders no line for it (`0x608f50`).
#[test]
fn creature_query_empty_subname_decodes_to_none() {
    let body = |subname_hex: &str| {
        hx(&format!(
            "45000000{}000000{}{}",
            "4465707574792057696c6c656d00", // "Deputy Willem" (+3 empty names after)
            subname_hex,
            // type_flags, type=7 Humanoid, family..display_id (5 × u32), civilian, racial_leader
            "000000000700000000000000000000000000000000000000000000000000"
        ))
    };
    let parse = |b: &[u8]| {
        messages::parse_server(messages::opcode::SMSG_CREATURE_QUERY_RESPONSE, b).unwrap()
    };
    match decode(parse(&body("00"))).as_slice() {
        [SessionEvent::CreatureName {
            entry,
            name,
            subname,
            ..
        }] => {
            assert_eq!(*entry, 69);
            assert_eq!(name.as_deref(), Some("Deputy Willem"));
            assert_eq!(*subname, None, "an empty wire subname is NO subname");
        }
        other => panic!("expected one CreatureName event, got {other:?}"),
    }
    match decode(parse(&body("5465737400"))).as_slice() {
        [SessionEvent::CreatureName { subname, .. }] => {
            assert_eq!(subname.as_deref(), Some("Test"));
        }
        other => panic!("expected one CreatureName event, got {other:?}"),
    }
}

/// `CMSG_PING` is `u32` sequence then `u32` last RTT (reference `0x537e10`, vmangos
/// `_HandlePing`); `SMSG_PONG` echoes the sequence.
#[test]
fn ping_body_golden_and_pong_roundtrip() {
    assert_eq!(
        messages::ping(1, 0),
        hx("0100000000000000"),
        "first ping of a connection: seq 1, no RTT yet"
    );
    assert_eq!(
        messages::ping(0x0A0B_0C0D, 23),
        hx("0d0c0b0a17000000"),
        "CMSG_PING body: sequence then lastRtt, both u32 LE"
    );
    let packet = messages::parse_server(messages::opcode::SMSG_PONG, &hx("0d0c0b0a")).unwrap();
    assert!(matches!(
        packet,
        ServerPacket::Pong {
            sequence: 0x0A0B_0C0D
        }
    ));
    match decode(packet).as_slice() {
        [SessionEvent::Pong {
            sequence: 0x0A0B_0C0D,
        }] => {}
        other => panic!("expected one Pong event, got {other:?}"),
    }
}

/// `SMSG_ADDON_INFO` has no count or names: the client reads one record per addon it sent. A
/// retail capture is 12 records of `{status 2, info 1, key 0, u32 revision 0, url 0}`; status 2
/// hides the addon from the Lua index (reference `0x51db84`).
#[test]
fn the_addon_info_reply_pairs_its_statuses_back_against_what_we_sent() {
    let capture: Vec<u8> = std::iter::repeat_n(hx("0201000000000000"), 12)
        .flatten()
        .collect();
    assert_eq!(capture.len(), 96, "the capture is 12 x 8 bytes");
    let packet = messages::parse_server(messages::opcode::SMSG_ADDON_INFO, &capture).unwrap();
    let ServerPacket::AddonInfo { statuses } = packet else {
        panic!("expected AddonInfo");
    };
    assert_eq!(statuses, vec![2u8; 12]);

    // Every stock addon comes back hidden, so the reference's AddOns list shows none of Blizzard's.
    let hidden = messages::hidden_from_reply(&statuses, &messages::STOCK_SECURE_ADDONS);
    assert_eq!(hidden.len(), 12);
    assert!(
        hidden.iter().all(|n| n.starts_with("Blizzard_")),
        "{hidden:?}"
    );

    // A record with a key and a url is 8 + 256 + 256 bytes; status 1 leaves the addon visible,
    // only status 2 hides it.
    let mut fat = vec![1u8, 1, 1];
    fat.extend(std::iter::repeat_n(0xABu8, 256)); // modulus
    fat.extend([0u8; 4]); // revision
    fat.push(1); // urlProvided
    fat.extend(std::iter::repeat_n(b'x', 256)); // url
    let packet = messages::parse_server(messages::opcode::SMSG_ADDON_INFO, &fat).unwrap();
    let ServerPacket::AddonInfo { statuses } = packet else {
        panic!("expected AddonInfo");
    };
    assert_eq!(statuses, vec![1u8], "the fat record parsed whole");
    assert!(messages::hidden_from_reply(&statuses, &messages::STOCK_SECURE_ADDONS).is_empty());

    let mut short = hx("0201000000000000").to_vec();
    short.extend([2u8, 1]); // a second record that stops mid-way
    let packet = messages::parse_server(messages::opcode::SMSG_ADDON_INFO, &short).unwrap();
    let ServerPacket::AddonInfo { statuses } = packet else {
        panic!("expected AddonInfo");
    };
    assert_eq!(statuses, vec![2u8], "only the whole record counted");
}
