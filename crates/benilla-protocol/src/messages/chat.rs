//! Inbound chat. `SMSG_MESSAGECHAT` (vmangos `Chat/Chat.cpp:2542-2599`) is `u8 type`,
//! `u32 language`, a per-type sender prefix, `u32 len` (NUL included), the text, `u8 chatTag`.
//! Also the small chat-adjacent bodies: player not found, notification, played time, random roll.

use std::io;

use crate::wire::{read_cstring, read_u32_le, read_u64_le, read_u8};

// ChatMsg values (vmangos `SharedDefines.h:1191-1301`), only those benilla receives or sends.
// Text emotes, channel notices and combat, skill, loot and money lines come from other opcodes.
pub const CHAT_MSG_SAY: u8 = 0x00;
pub const CHAT_MSG_PARTY: u8 = 0x01;
pub const CHAT_MSG_RAID: u8 = 0x02;
pub const CHAT_MSG_GUILD: u8 = 0x03;
pub const CHAT_MSG_OFFICER: u8 = 0x04;
pub const CHAT_MSG_YELL: u8 = 0x05;
pub const CHAT_MSG_WHISPER: u8 = 0x06;
/// Server-to-client only: the "You whisper to Name" self-echo (vmangos `MasterPlayerChat.cpp:54`).
pub const CHAT_MSG_WHISPER_INFORM: u8 = 0x07;
pub const CHAT_MSG_EMOTE: u8 = 0x08;
/// GM dot-command feedback and other server-authored lines; one sender guid, usually 0.
pub const CHAT_MSG_SYSTEM: u8 = 0x0A;
pub const CHAT_MSG_MONSTER_SAY: u8 = 0x0B;
pub const CHAT_MSG_MONSTER_YELL: u8 = 0x0C;
pub const CHAT_MSG_MONSTER_EMOTE: u8 = 0x0D;
pub const CHAT_MSG_CHANNEL: u8 = 0x0E;
pub const CHAT_MSG_AFK: u8 = 0x14;
pub const CHAT_MSG_DND: u8 = 0x15;
/// "Name is ignoring you": one sender guid, the ignoring player (`ChatHandler.cpp:755-763`).
pub const CHAT_MSG_IGNORED: u8 = 0x16;
pub const CHAT_MSG_MONSTER_WHISPER: u8 = 0x1A;
pub const CHAT_MSG_RAID_LEADER: u8 = 0x57;
pub const CHAT_MSG_RAID_WARNING: u8 = 0x58;
pub const CHAT_MSG_RAID_BOSS_WHISPER: u8 = 0x59;
pub const CHAT_MSG_RAID_BOSS_EMOTE: u8 = 0x5A;
/// The server's "your message was filtered" notice: its text is the addressee's name, which the
/// chat frame formats into `CHAT_FILTERED`; the client's spam and profanity filters exempt it
/// (`0x49aacc`).
pub const CHAT_MSG_FILTERED: u8 = 0x5B;
pub const CHAT_MSG_BATTLEGROUND: u8 = 0x5C;
pub const CHAT_MSG_BATTLEGROUND_LEADER: u8 = 0x5D;

/// The only chat types the 1.12 client runs its `$`-macro expander over; the rest reach the frame
/// verbatim. This is the wire gate (`0x49d5e4-0x49d606`); the re-expansion after a name query
/// (`0x49cf36`) omits `0x5A`.
pub const MACRO_EXPANDED_TYPES: [u8; 8] = [
    CHAT_MSG_MONSTER_SAY,        // 0x0B
    CHAT_MSG_MONSTER_YELL,       // 0x0C
    CHAT_MSG_MONSTER_EMOTE,      // 0x0D
    CHAT_MSG_MONSTER_WHISPER,    // 0x1A
    CHAT_MSG_BG_SYSTEM_NEUTRAL,  // 0x52
    CHAT_MSG_BG_SYSTEM_ALLIANCE, // 0x53
    CHAT_MSG_BG_SYSTEM_HORDE,    // 0x54
    CHAT_MSG_RAID_BOSS_EMOTE,    // 0x5A, wire gate only
];

/// The battleground system lines: one guid, the macro subject (`BuildChatPacket`'s `default:`).
pub const CHAT_MSG_BG_SYSTEM_NEUTRAL: u8 = 0x52;
pub const CHAT_MSG_BG_SYSTEM_ALLIANCE: u8 = 0x53;
pub const CHAT_MSG_BG_SYSTEM_HORDE: u8 = 0x54;

/// vmangos `PlayerChatTag`; the server picks GM over DND over AFK (`Player.cpp:1757-1767`).
pub mod chat_tag {
    pub const NONE: u8 = 0;
    pub const AFK: u8 = 1;
    pub const DND: u8 = 2;
    pub const GM: u8 = 3;
}

/// One decoded chat line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// The `ChatMsg` type byte (0x00 say, 0x0A system, 0x0B monster say, …).
    pub chat_type: u8,
    /// The `Language` id (0 universal, 7 common, …).
    pub language: u32,
    /// The speaker, when the type carries a guid (0 for system messages / name-only shapes).
    pub sender_guid: u64,
    /// The trailing addressee guid, 0 when the shape has none; the `$`-macro subject.
    pub target_guid: u64,
    /// The inline speaker name of the monster shapes; player names come from the name query.
    pub sender_name: Option<String>,
    /// The channel name, on `CHAT_MSG_CHANNEL` only.
    pub channel: Option<String>,
    pub text: String,
    /// The [`chat_tag`] byte, shown as the `<AFK>`/`<DND>`/`<GM>` prefix on the sender's name.
    pub chat_tag: u8,
}

impl ChatMessage {
    /// Whether this is addon traffic, which the chat frame never prints: 1.12 has no addon opcode
    /// or chat type, so `LANG_ADDON` on any lane is the only mark.
    ///
    /// Its text is `prefix`, a TAB, the payload; with no tab, the 1.12 client reads all as prefix.
    pub fn is_addon(&self) -> bool {
        self.language == super::LANGUAGE_ADDON
    }
}

/// Read the length-prefixed string shape (`u32 len` including NUL + bytes).
fn read_len_string(r: &mut &[u8]) -> io::Result<String> {
    let len = read_u32_le(r)? as usize;
    if len == 0 {
        return Ok(String::new());
    }
    // The NUL is inside `len`; read through it via the cstring reader and sanity-cap first.
    if len > r.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("chat text length {len} exceeds remaining {}", r.len()),
        ));
    }
    read_cstring(r)
}

/// Read an `SMSG_MESSAGECHAT` body.
pub(super) fn read_message_chat(r: &mut &[u8]) -> io::Result<ChatMessage> {
    let chat_type = read_u8(r)?;
    let language = read_u32_le(r)?;
    let mut sender_guid = 0u64;
    // 0 unless the shape carries an addressee; the macro expander then uses `sender_guid`.
    let mut target_guid = 0u64;
    let mut sender_name = None;
    let mut channel = None;
    match chat_type {
        CHAT_MSG_MONSTER_WHISPER
        | CHAT_MSG_RAID_BOSS_WHISPER
        | CHAT_MSG_RAID_BOSS_EMOTE
        | CHAT_MSG_MONSTER_EMOTE => {
            sender_name = Some(read_len_string(r)?);
            target_guid = read_u64_le(r)?;
        }
        CHAT_MSG_SAY | CHAT_MSG_PARTY | CHAT_MSG_YELL => {
            sender_guid = read_u64_le(r)?;
            // vmangos writes the sender's guid into both slots for these three.
            target_guid = read_u64_le(r)?;
        }
        CHAT_MSG_MONSTER_SAY | CHAT_MSG_MONSTER_YELL => {
            sender_guid = read_u64_le(r)?;
            sender_name = Some(read_len_string(r)?);
            target_guid = read_u64_le(r)?;
        }
        CHAT_MSG_CHANNEL => {
            channel = Some(read_cstring(r)?);
            let _player_rank = read_u32_le(r)?;
            sender_guid = read_u64_le(r)?;
        }
        // Every other type is `BuildChatPacket`'s `default:` shape: one sender guid.
        _ => {
            sender_guid = read_u64_le(r)?;
        }
    }
    let text = read_len_string(r)?;
    let chat_tag = read_u8(r)?;
    Ok(ChatMessage {
        chat_type,
        language,
        sender_guid,
        target_guid,
        sender_name,
        channel,
        text,
        chat_tag,
    })
}

/// Read `SMSG_CHAT_PLAYER_NOT_FOUND`: one cstring, the unnormalized name of an offline or
/// misspelled whisper target (vmangos `Server/Packets/Chat.cpp:26-29`).
pub(super) fn read_chat_player_not_found(r: &mut &[u8]) -> io::Result<String> {
    read_cstring(r)
}

/// Read `SMSG_NOTIFICATION`: one cstring of server-formatted notice text, which the 1.12 client
/// shows in the red UIErrorsFrame (vmangos `Server/WorldSession.cpp:900-915`).
pub(super) fn read_notification(r: &mut &[u8]) -> io::Result<String> {
    read_cstring(r)
}

/// Read `SMSG_PLAYED_TIME`, the `/played` answer: total then this-level played time, both `u32`
/// seconds (vmangos `Server/Packets/Misc.cpp:278-282`).
pub(super) fn read_played_time(r: &mut &[u8]) -> io::Result<(u32, u32)> {
    let total = read_u32_le(r)?;
    let level = read_u32_le(r)?;
    Ok((total, level))
}

/// Read the `MSG_RANDOM_ROLL` broadcast (`GroupHandler.cpp:394-422`): `u32` minimum, maximum and
/// roll, then the roller's unpacked `u64` guid; the request on this opcode is only min and max.
pub(super) fn read_random_roll(r: &mut &[u8]) -> io::Result<(u32, u32, u32, u64)> {
    let min = read_u32_le(r)?;
    let max = read_u32_le(r)?;
    let roll = read_u32_le(r)?;
    let guid = read_u64_le(r)?;
    Ok((min, max, roll, guid))
}
