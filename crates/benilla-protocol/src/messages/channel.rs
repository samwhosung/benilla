//! The channel notices and member roster (vmangos `Chat/Channel.cpp`).

use std::io;

use crate::wire::{capacity_hint, read_cstring, read_u32_le, read_u64_le, read_u8};

/// `SMSG_CHANNEL_NOTIFY`'s notice byte (`ChatNotify`, vmangos `Chat/Channel.h:35-71`).
pub mod channel_notice {
    pub const JOINED: u8 = 0x00;
    pub const LEFT: u8 = 0x01;
    pub const YOU_JOINED: u8 = 0x02;
    pub const YOU_LEFT: u8 = 0x03;
    pub const WRONG_PASSWORD: u8 = 0x04;
    pub const NOT_MEMBER: u8 = 0x05;
    pub const NOT_MODERATOR: u8 = 0x06;
    pub const PASSWORD_CHANGED: u8 = 0x07;
    pub const OWNER_CHANGED: u8 = 0x08;
    pub const PLAYER_NOT_FOUND: u8 = 0x09;
    pub const NOT_OWNER: u8 = 0x0A;
    pub const CHANNEL_OWNER: u8 = 0x0B;
    pub const MODE_CHANGE: u8 = 0x0C;
    pub const ANNOUNCEMENTS_ON: u8 = 0x0D;
    pub const ANNOUNCEMENTS_OFF: u8 = 0x0E;
    pub const MODERATION_ON: u8 = 0x0F;
    pub const MODERATION_OFF: u8 = 0x10;
    pub const MUTED: u8 = 0x11;
    pub const PLAYER_KICKED: u8 = 0x12;
    pub const BANNED: u8 = 0x13;
    pub const PLAYER_BANNED: u8 = 0x14;
    pub const PLAYER_UNBANNED: u8 = 0x15;
    pub const PLAYER_NOT_BANNED: u8 = 0x16;
    pub const PLAYER_ALREADY_MEMBER: u8 = 0x17;
    pub const INVITE: u8 = 0x18;
    pub const INVITE_WRONG_FACTION: u8 = 0x19;
    pub const WRONG_FACTION: u8 = 0x1A;
    pub const INVALID_NAME: u8 = 0x1B;
    pub const NOT_MODERATED: u8 = 0x1C;
    pub const PLAYER_INVITED: u8 = 0x1D;
    pub const PLAYER_INVITE_BANNED: u8 = 0x1E;
    pub const THROTTLED: u8 = 0x1F;
}

/// What follows the notice byte and channel name, by notice (`Chat/Channel.cpp:804-1008`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelNoticeTail {
    /// The member who joined or left.
    Guid(u64),
    /// Our channel flags, then the instance number the client keeps on the slot (`+0x98`,
    /// `0x49bb20`); vmangos always sends 0 (`Chat/Channel.cpp:827`).
    YouJoined { flags: u32, instance: u32 },
    /// Nothing follows the channel name.
    Empty,
    /// The player who made the change or sent the invite.
    Actor(u64),
    /// A player name; `CHANNEL_OWNER` may send the literal `"Nobody"` or `"PLAYER_NOT_FOUND"`.
    Name(String),
    /// The member and its `ChannelMemberFlags` before and after.
    ModeChange {
        guid: u64,
        old_flags: u8,
        new_flags: u8,
    },
    /// The affected player, then who acted.
    Actors { target: u64, source: u64 },
}

/// One decoded `SMSG_CHANNEL_NOTIFY`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelNotify {
    pub notice: u8,
    pub channel: String,
    pub tail: ChannelNoticeTail,
}

/// A notice byte past 0x1F errors, since the length of its tail is unknown.
pub(super) fn read_channel_notify(r: &mut &[u8]) -> io::Result<ChannelNotify> {
    use channel_notice as n;

    let notice = read_u8(r)?;
    let channel = read_cstring(r)?;
    let tail = match notice {
        n::JOINED | n::LEFT => ChannelNoticeTail::Guid(read_u64_le(r)?),
        n::YOU_JOINED => {
            let flags = read_u32_le(r)?;
            let instance = read_u32_le(r)?;
            ChannelNoticeTail::YouJoined { flags, instance }
        }
        n::YOU_LEFT
        | n::WRONG_PASSWORD
        | n::NOT_MEMBER
        | n::NOT_MODERATOR
        | n::NOT_OWNER
        | n::MUTED
        | n::BANNED
        | n::INVITE_WRONG_FACTION
        | n::WRONG_FACTION
        | n::INVALID_NAME
        | n::NOT_MODERATED
        | n::THROTTLED => ChannelNoticeTail::Empty,
        n::PASSWORD_CHANGED
        | n::OWNER_CHANGED
        | n::ANNOUNCEMENTS_ON
        | n::ANNOUNCEMENTS_OFF
        | n::MODERATION_ON
        | n::MODERATION_OFF
        | n::PLAYER_ALREADY_MEMBER
        | n::INVITE => ChannelNoticeTail::Actor(read_u64_le(r)?),
        n::PLAYER_NOT_FOUND
        | n::CHANNEL_OWNER
        | n::PLAYER_NOT_BANNED
        | n::PLAYER_INVITED
        | n::PLAYER_INVITE_BANNED => ChannelNoticeTail::Name(read_cstring(r)?),
        n::MODE_CHANGE => ChannelNoticeTail::ModeChange {
            guid: read_u64_le(r)?,
            old_flags: read_u8(r)?,
            new_flags: read_u8(r)?,
        },
        n::PLAYER_KICKED | n::PLAYER_BANNED | n::PLAYER_UNBANNED => ChannelNoticeTail::Actors {
            target: read_u64_le(r)?,
            source: read_u64_le(r)?,
        },
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("SMSG_CHANNEL_NOTIFY: unknown notice type {other:#04x}"),
            ))
        }
    };
    Ok(ChannelNotify {
        notice,
        channel,
        tail,
    })
}

/// `SMSG_CHANNEL_LIST` (`Chat/Channel.cpp:513-556`): `(channel, flags, members)`, each member a
/// guid and its `ChannelMemberFlags` (`Chat/Channel.h:119-130`).
#[allow(clippy::type_complexity)]
pub(super) fn read_channel_list(r: &mut &[u8]) -> io::Result<(String, u8, Vec<(u64, u8)>)> {
    let channel = read_cstring(r)?;
    let flags = read_u8(r)?;
    let count = read_u32_le(r)?;
    // The count is wire-controlled, so it only hints the allocation.
    let mut members = Vec::with_capacity(capacity_hint(count, 256));
    for _ in 0..count {
        let guid = read_u64_le(r)?;
        let member_flags = read_u8(r)?;
        members.push((guid, member_flags));
    }
    Ok((channel, flags, members))
}
