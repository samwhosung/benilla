//! The guild wire messages: roster, guild query, invitations, rank administration and the event
//! broadcast (opcodes `0x54`/`0x55`, `0x81`-`0x93`, `0x231`-`0x235`, `0x2FC`).
//!
//! Deviation: string reads are unbounded. The reference `0x4191b0` caps each field, counting the
//! NUL (roster name `0x30`, notes `0x80`, MOTD `0x200`, info `0x7d0`, guild name `0x60`, rank name
//! `0x40`), and on overflow empties it and no-ops every later read in the packet. Unbounded reads
//! cannot poison a packet, and every cap is far above the server's own length limits, so a
//! legitimate packet reads the same.

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_cstring, read_f32_le, read_u32_le, read_u64_le, read_u8};

/// Minimum rank count: the five defaults in [`guild_default_rank`] (`Guild/Guild.h:31`).
pub const GUILD_RANKS_MIN_COUNT: usize = 5;
/// Maximum rank count (`Guild/Guild.h:32`), and the exact number of rank names
/// `SMSG_GUILD_QUERY_RESPONSE` carries, unused ones empty (`Guild/Guild.cpp:868-872`).
pub const GUILD_RANKS_MAX_COUNT: usize = 10;

/// Rank-name cap in characters (`Guild/Guild.h:36`). The builders do not truncate, and an
/// over-long rank name gets the session kicked (`Handlers/GuildHandler.cpp:580-584`, `:600-604`).
pub const GUILD_RANK_MAX_LENGTH: usize = 15;
/// Guild-name cap, in characters (`Guild/Guild.h:37`).
pub const GUILD_NAME_MAX_LENGTH: usize = 24;
/// Public/officer note cap, in characters (`Guild/Guild.h:38`).
pub const GUILD_NOTE_MAX_LENGTH: usize = 31;
/// Guild info text cap, in characters (`Guild/Guild.h:39`).
pub const GUILD_INFO_MAX_LENGTH: usize = 500;
/// MOTD cap, in characters (`Guild/Guild.h:40`).
pub const GUILD_MOTD_MAX_LENGTH: usize = 128;

/// The five ranks every guild is created with and cannot delete (`Guild/Guild.h:44-54`). Ids are
/// 0-based with 0 = guild master; promote does `rank--`, demote `rank++`.
pub mod guild_default_rank {
    /// `GR_GUILDMASTER`; the server forces this rank's rights to `GR_RIGHT_ALL` whatever we send.
    pub const GUILDMASTER: u32 = 0;
    pub const OFFICER: u32 = 1;
    pub const VETERAN: u32 = 2;
    pub const MEMBER: u32 = 3;
    /// `GR_INITIATE`, the lowest default rank.
    pub const INITIATE: u32 = 4;
}

/// One rank's rights bitmask (`Guild/Guild.h:56-74`) with the `GR_RIGHT_EMPTY` bit factored out:
/// vmangos spells each right `0x40 | bit` for its test `(rights & right) != GR_RIGHT_EMPTY`.
pub mod guild_rank_right {
    /// `GR_RIGHT_EMPTY`, the sentinel vmangos ORs into every right; it grants nothing.
    pub const EMPTY: u32 = 0x0000_0040;
    /// Read guild chat (`GR_RIGHT_GCHATLISTEN`, vmangos `0x41`).
    pub const GCHAT_LISTEN: u32 = 0x0000_0001;
    /// Speak in guild chat (`GR_RIGHT_GCHATSPEAK`, vmangos `0x42`).
    pub const GCHAT_SPEAK: u32 = 0x0000_0002;
    /// Read officer chat (`GR_RIGHT_OFFCHATLISTEN`, vmangos `0x44`).
    pub const OFFCHAT_LISTEN: u32 = 0x0000_0004;
    /// Speak in officer chat (`GR_RIGHT_OFFCHATSPEAK`, vmangos `0x48`).
    pub const OFFCHAT_SPEAK: u32 = 0x0000_0008;
    /// Invite new members (`GR_RIGHT_INVITE`, vmangos `0x50`).
    pub const INVITE: u32 = 0x0000_0010;
    /// Kick members (`GR_RIGHT_REMOVE`, vmangos `0x60`).
    pub const REMOVE: u32 = 0x0000_0020;
    /// Promote members (`GR_RIGHT_PROMOTE`, vmangos `0xC0`).
    pub const PROMOTE: u32 = 0x0000_0080;
    /// Demote members (`GR_RIGHT_DEMOTE`, vmangos `0x140`).
    pub const DEMOTE: u32 = 0x0000_0100;
    /// Set the message of the day (`GR_RIGHT_SETMOTD`, vmangos `0x1040`).
    pub const SET_MOTD: u32 = 0x0000_1000;
    /// Edit any member's public note (`GR_RIGHT_EPNOTE`, vmangos `0x2040`).
    pub const EDIT_PUBLIC_NOTE: u32 = 0x0000_2000;
    /// See officer notes (`GR_RIGHT_VIEWOFFNOTE`, vmangos `0x4040`); without it the server blanks
    /// every roster `officerNote` (`Guild/Guild.cpp:821`, `:844`).
    pub const VIEW_OFFICER_NOTE: u32 = 0x0000_4000;
    /// Edit officer notes (`GR_RIGHT_EOFFNOTE`, vmangos `0x8040`).
    pub const EDIT_OFFICER_NOTE: u32 = 0x0000_8000;
    /// Edit the guild information text (`GR_RIGHT_MODIFY_GUILD_INFO`, vmangos `0x10040`).
    pub const MODIFY_GUILD_INFO: u32 = 0x0001_0000;
    /// `GR_RIGHT_ALL`, forced onto rank 0 by the server. A raw vmangos value: it includes
    /// [`EMPTY`] and bits no 1.12 right owns, so test ranks against the named bits, never this.
    pub const ALL: u32 = 0x000F_F1FF;
}

/// The thirteen rank-right checkboxes in the order `GuildControlGetRankFlags()` and
/// `GuildControlSetRankFlag(i, on)` index them, which is not bit order: Promote/Demote (5/6) come
/// before Invite/Remove (7/8), whose bits are lower.
pub const GUILD_RANK_RIGHT_ORDER: [u32; 13] = [
    guild_rank_right::GCHAT_LISTEN,
    guild_rank_right::GCHAT_SPEAK,
    guild_rank_right::OFFCHAT_LISTEN,
    guild_rank_right::OFFCHAT_SPEAK,
    guild_rank_right::PROMOTE,
    guild_rank_right::DEMOTE,
    guild_rank_right::INVITE,
    guild_rank_right::REMOVE,
    guild_rank_right::SET_MOTD,
    guild_rank_right::EDIT_PUBLIC_NOTE,
    guild_rank_right::VIEW_OFFICER_NOTE,
    guild_rank_right::EDIT_OFFICER_NOTE,
    guild_rank_right::MODIFY_GUILD_INFO,
];

/// A roster member's presence byte (`Guild/Guild.h:139-144`); only `0` changes the wire, adding
/// the `f32 lastOnlineTime`. The reference tests DND before AFK (`0x4d13e4`, `0x4d13fb`), so a
/// member flagged both shows as DND.
pub mod guild_presence {
    /// Offline; only this value carries the `lastOnlineTime` float.
    pub const OFFLINE: u8 = 0x00;
    /// `GRF_ONLINE` (vmangos value; the reference only tests the whole byte against zero).
    pub const ONLINE: u8 = 0x01;
    /// `GRF_AFK`, set alongside [`ONLINE`]; the reference's own `CHAT_FLAG_AFK`.
    pub const AFK: u8 = 0x02;
    /// `GRF_DND`, set alongside [`ONLINE`]; the reference's own `CHAT_FLAG_DND`.
    pub const DND: u8 = 0x04;
}

/// `SMSG_GUILD_EVENT`'s leading byte (`Guild/Guild.h:121-137`), the arms of the reference
/// handler's `switch` (jump table `0x5e74dc`).
pub mod guild_event {
    /// `GE_PROMOTION`, params: promoter, promoted, new rank name.
    pub const PROMOTION: u8 = 0x00;
    /// `GE_DEMOTION`, params: demoter, demoted, new rank name.
    pub const DEMOTION: u8 = 0x01;
    /// `GE_MOTD`, param: the new MOTD. The reference routes it through the CVar-gated profanity
    /// filter (`0x703f50`), not the normal event path.
    pub const MOTD: u8 = 0x02;
    /// `GE_JOINED`, param: the joiner's name.
    pub const JOINED: u8 = 0x03;
    /// `GE_LEFT`, param: the leaver's name.
    pub const LEFT: u8 = 0x04;
    /// `GE_REMOVED`, params: the removed player, then who removed them.
    pub const REMOVED: u8 = 0x05;
    /// `GE_LEADER_IS`, param: the current leader.
    pub const LEADER_IS: u8 = 0x06;
    /// `GE_LEADER_CHANGED`, params: old leader, new leader.
    pub const LEADER_CHANGED: u8 = 0x07;
    /// `GE_DISBANDED`, no params.
    pub const DISBANDED: u8 = 0x08;
    /// `GE_TABARDCHANGE`; the reference handles it in its default arm.
    pub const TABARD_CHANGE: u8 = 0x09;
    /// `GE_UPDATE_RANK_NAME`, params: rank id, new rank name. The reference writes it into the
    /// roster cache (`0x560e30`) and fires no FrameScript event.
    pub const UPDATE_RANK_NAME: u8 = 0x0A;
    /// `GE_UPDATE_ROSTER`: silent in the reference, and vmangos never sends it.
    pub const UPDATE_ROSTER: u8 = 0x0B;
    /// `GE_SIGNED_ON`, param: the guildmate's name; carries the trailing guid.
    pub const SIGNED_ON: u8 = 0x0C;
    /// `GE_SIGNED_OFF`, param: the guildmate's name; carries the trailing guid.
    pub const SIGNED_OFF: u8 = 0x0D;
}

/// `SMSG_GUILD_COMMAND_RESULT`'s leading `u32`, the verb the result is about
/// (`Guild/Guild.h:76-85`). Coarser than the request: vmangos reports "not in a guild" for most
/// verbs as [`CREATE`] and every permission refusal as [`INVITE`].
pub mod guild_command {
    /// `GUILD_CREATE_S`, also vmangos's catch-all for "you are not in a guild".
    pub const CREATE: u32 = 0x00;
    /// `GUILD_INVITE_S`, also vmangos's catch-all for a permission refusal.
    pub const INVITE: u32 = 0x01;
    /// `GUILD_QUIT_S`: leaving or being removed.
    pub const QUIT: u32 = 0x03;
    pub const FOUNDER: u32 = 0x0E;
    pub const UNK19: u32 = 0x13;
    pub const UNK20: u32 = 0x14;
}

/// `SMSG_GUILD_COMMAND_RESULT`'s trailing `u32` (`Guild/Guild.h:96-119`). [`LEADER_LEAVE`] and
/// [`PERMISSIONS`] share `0x08`: with command `QUIT` it is the leader-cannot-leave message,
/// otherwise the permission refusal.
pub mod guild_command_error {
    /// `ERR_PLAYER_NO_MORE_IN_GUILD`: "no message/error" per vmangos.
    pub const PLAYER_NO_MORE_IN_GUILD: u32 = 0x00;
    /// `ERR_GUILD_INTERNAL`.
    pub const INTERNAL: u32 = 0x01;
    /// `ERR_ALREADY_IN_GUILD`: you are.
    pub const ALREADY_IN_GUILD: u32 = 0x02;
    /// `ERR_ALREADY_IN_GUILD_S`: they are (the string names them).
    pub const ALREADY_IN_GUILD_S: u32 = 0x03;
    /// `ERR_INVITED_TO_GUILD`.
    pub const INVITED_TO_GUILD: u32 = 0x04;
    /// `ERR_ALREADY_INVITED_TO_GUILD_S`.
    pub const ALREADY_INVITED_TO_GUILD_S: u32 = 0x05;
    /// `ERR_GUILD_NAME_INVALID`.
    pub const NAME_INVALID: u32 = 0x06;
    /// `ERR_GUILD_NAME_EXISTS_S`.
    pub const NAME_EXISTS_S: u32 = 0x07;
    /// `ERR_GUILD_LEADER_LEAVE`: the guild master cannot `/gquit` while anyone else remains.
    pub const LEADER_LEAVE: u32 = 0x08;
    /// `ERR_GUILD_PERMISSIONS`, sharing `0x08` with [`LEADER_LEAVE`].
    pub const PERMISSIONS: u32 = 0x08;
    /// `ERR_GUILD_PLAYER_NOT_IN_GUILD`: you aren't.
    pub const PLAYER_NOT_IN_GUILD: u32 = 0x09;
    /// `ERR_GUILD_PLAYER_NOT_IN_GUILD_S`: they aren't.
    pub const PLAYER_NOT_IN_GUILD_S: u32 = 0x0A;
    /// `ERR_GUILD_PLAYER_NOT_FOUND_S`.
    pub const PLAYER_NOT_FOUND_S: u32 = 0x0B;
    /// `ERR_GUILD_NOT_ALLIED`: cross-faction, refused by config.
    pub const NOT_ALLIED: u32 = 0x0C;
    /// `ERR_GUILD_RANK_TOO_HIGH_S`.
    pub const RANK_TOO_HIGH_S: u32 = 0x0D;
    /// `ERR_GUILD_RANK_TOO_LOW_S`.
    pub const RANK_TOO_LOW_S: u32 = 0x0E;
    /// `ERR_GUILD_RANKS_LOCKED` (`0x0F`/`0x10` are unused at 5875).
    pub const RANKS_LOCKED: u32 = 0x11;
    /// `ERR_GUILD_RANK_IN_USE`: the lowest rank cannot be deleted while somebody holds it.
    pub const RANK_IN_USE: u32 = 0x12;
    /// `ERR_GUILD_IGNORING_YOU_S`.
    pub const IGNORING_YOU_S: u32 = 0x13;
    /// `ERR_GUILD_UNK20`: "for Typecommand 0x05 only" per vmangos.
    pub const UNK20: u32 = 0x14;
}

/// `SMSG_GUILD_QUERY_RESPONSE`: one guild's name, ten rank names and tabard, by guild id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildQueryResponse {
    /// The guild id we asked about.
    pub guild_id: u32,
    /// The guild's name; empty means no such guild, and the reference then drops the id from its
    /// cache instead of inserting it (`0x5552ae`).
    pub name: String,
    /// Indexed by rank id, uncreated ranks empty; the live rank count is the roster's
    /// `rank_rights` length.
    pub rank_names: [String; GUILD_RANKS_MAX_COUNT],
    /// Tabard emblem index, formatted into `Textures\GuildEmblems\Emblem_<style>_<color>_…`. All
    /// five tabard fields are `int32` (`Server/Packets/Guild.h:208`) and a new guild has each at
    /// `-1` (`Guild/Guild.cpp:86`), meaning no tabard designed.
    pub emblem_style: i32,
    /// Tabard emblem colour index. Signed, `-1` = no tabard designed.
    pub emblem_color: i32,
    /// Tabard border style index. Signed, `-1` = no tabard designed.
    pub border_style: i32,
    /// Tabard border colour index. Signed, `-1` = no tabard designed.
    pub border_color: i32,
    /// Tabard background colour index. Signed, `-1` = no tabard designed.
    pub background_color: i32,
}

/// Read `SMSG_GUILD_QUERY_RESPONSE` (`Server/Packets/Guild.cpp:118-131`, reference `0x62f260`):
/// `u32` id, name, exactly ten rank-name cstrings with no count, then five `i32` tabard fields.
pub(super) fn read_guild_query_response(r: &mut impl Read) -> io::Result<GuildQueryResponse> {
    let guild_id = read_u32_le(r)?;
    let name = read_cstring(r)?;
    let mut rank_names: [String; GUILD_RANKS_MAX_COUNT] = Default::default();
    for rank_name in &mut rank_names {
        *rank_name = read_cstring(r)?;
    }
    Ok(GuildQueryResponse {
        guild_id,
        name,
        rank_names,
        emblem_style: read_u32_le(r)? as i32,
        emblem_color: read_u32_le(r)? as i32,
        border_style: read_u32_le(r)? as i32,
        border_color: read_u32_le(r)? as i32,
        background_color: read_u32_le(r)? as i32,
    })
}

/// One row of `SMSG_GUILD_ROSTER`; every field the guild pane shows is on the wire.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GuildRosterMember {
    /// The member's player guid (vmangos always builds it as `ObjectGuid(HIGHGUID_PLAYER, …)`).
    pub guid: u64,
    /// [`guild_presence`] bits; `0` puts [`Self::last_online_days`] on the wire.
    pub presence: u8,
    pub name: String,
    /// Rank id, 0-based with 0 = guild master; indexes [`GuildQueryResponse::rank_names`] and
    /// [`GuildRoster::rank_rights`].
    pub rank_id: u32,
    pub level: u8,
    /// Class id (`ChrClasses.dbc`).
    pub class: u8,
    /// Zone id (`AreaTable.dbc`); an offline member's last known zone.
    pub zone: u32,
    /// Fractional days since logout (`Guild/Guild.cpp:841`), sent for offline members only;
    /// `GetGuildRosterLastOnline` splits it into years, months and days.
    pub last_online_days: f32,
    pub public_note: String,
    /// The member's officer note, blanked by the server unless the viewer holds
    /// [`guild_rank_right::VIEW_OFFICER_NOTE`] (`Guild/Guild.cpp:821`, `:844`).
    pub officer_note: String,
}

impl GuildRosterMember {
    /// Any presence bit set. A whole-byte test, not `& ONLINE`, as it is the wire's condition for
    /// `lastOnlineTime` in vmangos and in the reference (`0x4d0c12`, `0x4d0c1d`).
    pub fn is_online(&self) -> bool {
        self.presence != guild_presence::OFFLINE
    }
}

/// `SMSG_GUILD_ROSTER`: the whole guild, re-sent complete on any change. The server truncates the
/// member list to fit `GUILD_ROSTER_MAX_LENGTH` (`0x8000 - 4` bytes, `Guild/Guild.h:41`), so a
/// large guild can arrive short.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GuildRoster {
    pub motd: String,
    /// The "Guild Information" text, written only for builds above `CLIENT_BUILD_1_8_4`.
    pub info: String,
    /// One [`guild_rank_right`] mask per rank, by rank id; the length is the guild's real rank
    /// count. More than [`GUILD_RANKS_MAX_COUNT`] is malformed but read in full, so a ten-slot
    /// consumer bounds itself (the reference overruns its array at `0xb726d0`).
    pub rank_rights: Vec<u32>,
    /// The members in wire order, by low guid; the reference re-sorts its own copy after every
    /// packet (`0x4d0d32`), so sorting is the consumer's job.
    pub members: Vec<GuildRosterMember>,
}

/// Allocation cap for the wire member count; a `0x8000`-byte roster fits about 1.4k members.
const ROSTER_CAPACITY_HINT_CAP: usize = 2048;

/// Read `SMSG_GUILD_ROSTER` (`Server/Packets/Guild.cpp:143-173`, reference `0x4d0ad0`): `u32`
/// member count, MOTD, info text, `u32` rank count and that many `u32` rights, then per member
/// `u64` guid, `u8` presence, name, `u32` rank, `u8` level, `u8` class, `u32` zone, an `f32`
/// last-online time only when presence is `0` (`0x4d0cad`), public note and officer note.
/// Reading the float unconditionally misparses every later member.
pub(super) fn read_guild_roster(r: &mut impl Read) -> io::Result<GuildRoster> {
    let member_count = read_u32_le(r)?;
    let motd = read_cstring(r)?;
    let info = read_cstring(r)?;

    let rank_count = read_u32_le(r)?;
    let mut rank_rights = Vec::with_capacity(capacity_hint(rank_count, GUILD_RANKS_MAX_COUNT));
    for _ in 0..rank_count {
        rank_rights.push(read_u32_le(r)?);
    }

    let mut members = Vec::with_capacity(capacity_hint(member_count, ROSTER_CAPACITY_HINT_CAP));
    for _ in 0..member_count {
        let mut member = GuildRosterMember {
            guid: read_u64_le(r)?,
            presence: read_u8(r)?,
            name: read_cstring(r)?,
            rank_id: read_u32_le(r)?,
            level: read_u8(r)?,
            class: read_u8(r)?,
            zone: read_u32_le(r)?,
            ..Default::default()
        };
        if !member.is_online() {
            member.last_online_days = read_f32_le(r)?;
        }
        member.public_note = read_cstring(r)?;
        member.officer_note = read_cstring(r)?;
        members.push(member);
    }

    Ok(GuildRoster {
        motd,
        info,
        rank_rights,
        members,
    })
}

/// `SMSG_GUILD_EVENT`: a guild event id and its `%s` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildEventNotice {
    /// A [`guild_event`] id; the reference sends anything above `0x0D` to its default arm.
    pub event: u8,
    /// Every string `strCount` announced, uncapped like the reference read loop; the reference
    /// display passes none of them when there are four or more (`0x5e745f`).
    pub params: Vec<String>,
    /// The guildmate's guid, read only for [`guild_event::SIGNED_ON`] and
    /// [`guild_event::SIGNED_OFF`].
    pub guid: Option<u64>,
}

/// Read `SMSG_GUILD_EVENT` (`Server/Packets/Guild.cpp:133-141`, reference `0x5e7180`): `u8`
/// event, `u8` count, that many cstrings, then a `u64` guid that the reference reads only for
/// sign-on and sign-off; vmangos also sends it with `GE_JOINED` and `GE_LEFT`, unread there.
pub(super) fn read_guild_event(r: &mut impl Read) -> io::Result<GuildEventNotice> {
    let event = read_u8(r)?;
    let param_count = read_u8(r)?;
    // Events carry one to three strings; the count is unbounded (`Server/Packets/Guild.h:222`).
    let mut params = Vec::with_capacity(capacity_hint(param_count, 16));
    for _ in 0..param_count {
        params.push(read_cstring(r)?);
    }
    let guid = matches!(event, guild_event::SIGNED_ON | guild_event::SIGNED_OFF)
        .then(|| read_u64_le(r))
        .transpose()?;
    Ok(GuildEventNotice {
        event,
        params,
        guid,
    })
}

/// `SMSG_GUILD_COMMAND_RESULT`: the server's verdict on a guild verb we sent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildCommandResult {
    /// A [`guild_command`] tag; it decides which of result `0x08`'s two meanings applies.
    pub command: u32,
    /// The `%s` of the matching `ERR_GUILD_*` string: a player name, a guild name, or empty.
    pub name: String,
    /// A [`guild_command_error`] code.
    pub result: u32,
}

/// Read `SMSG_GUILD_COMMAND_RESULT` (`Server/Packets/Guild.cpp:96-101`): `u32`, cstring, `u32`.
pub(super) fn read_guild_command_result(r: &mut impl Read) -> io::Result<GuildCommandResult> {
    Ok(GuildCommandResult {
        command: read_u32_le(r)?,
        name: read_cstring(r)?,
        result: read_u32_le(r)?,
    })
}

/// `SMSG_GUILD_INFO`, answering `CMSG_GUILD_INFO`: founding date, member and account counts. The
/// field names are vmangos's; the reference formats wire fields 2, 1, 3 into its enUS `M/D/Y`
/// `GUILD_INFO_TEMPLATE` (`0x5e704d`), which fits day, month, year.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildInfo {
    pub name: String,
    pub created_day: u32,
    pub created_month: u32,
    pub created_year: u32,
    pub member_count: u32,
    /// How many distinct accounts the members belong to.
    pub account_count: u32,
}

/// Read `SMSG_GUILD_INFO` (`Server/Packets/Guild.cpp:103-111`): the name, then five `u32`s.
pub(super) fn read_guild_info(r: &mut impl Read) -> io::Result<GuildInfo> {
    Ok(GuildInfo {
        name: read_cstring(r)?,
        created_day: read_u32_le(r)?,
        created_month: read_u32_le(r)?,
        created_year: read_u32_le(r)?,
        member_count: read_u32_le(r)?,
        account_count: read_u32_le(r)?,
    })
}

/// Read `SMSG_GUILD_INVITE` (`Server/Packets/Guild.cpp:85-89`): inviter, then guild name. The
/// accept and decline replies name neither, so the pending invite is client state.
pub(super) fn read_guild_invite(r: &mut impl Read) -> io::Result<(String, String)> {
    let inviter = read_cstring(r)?;
    let guild = read_cstring(r)?;
    Ok((inviter, guild))
}

/// Read `SMSG_GUILD_DECLINE` (`Server/Packets/Guild.cpp:91-94`): who declined our invitation.
pub(super) fn read_guild_decline(r: &mut impl Read) -> io::Result<String> {
    read_cstring(r)
}

/// Body of `CMSG_GUILD_QUERY` (`Server/Packets/Guild.cpp:8-11`): the `u32` guild id. vmangos
/// answers an unknown id with `SMSG_GUILD_COMMAND_RESULT` `(CREATE, PLAYER_NOT_IN_GUILD)`
/// (`Handlers/GuildHandler.cpp:44`).
pub fn guild_query(guild_id: u32) -> Vec<u8> {
    guild_id.to_le_bytes().to_vec()
}

/// Body of `CMSG_GUILD_CREATE` (`Server/Packets/Guild.cpp:3-6`): the name. vmangos never handles
/// it (`STATUS_NEVER`, `Server/Protocol/Opcodes.cpp:210`); a 1.12 guild is founded by charter.
pub fn guild_create(name: &str) -> Vec<u8> {
    cstring_body(name)
}

/// Body of `CMSG_GUILD_INVITE` (`Server/Packets/Guild.cpp:13-16`): the name, which the server
/// case-normalises itself.
pub fn guild_invite(name: &str) -> Vec<u8> {
    cstring_body(name)
}

/// Body of `CMSG_GUILD_ACCEPT`: empty (`Server/Protocol/Opcodes.cpp:213`); the server holds the
/// pending invite.
pub fn guild_accept() -> Vec<u8> {
    Vec::new()
}

/// Body of `CMSG_GUILD_DECLINE`: empty (`Server/Protocol/Opcodes.cpp:214`); the inviter gets an
/// `SMSG_GUILD_DECLINE` naming us.
pub fn guild_decline() -> Vec<u8> {
    Vec::new()
}

/// Body of `CMSG_GUILD_INFO`: empty (`Server/Protocol/Opcodes.cpp:216`); asks about our guild.
pub fn guild_info() -> Vec<u8> {
    Vec::new()
}

/// Body of `CMSG_GUILD_ROSTER`: empty (`Server/Protocol/Opcodes.cpp:218`); the server also pushes
/// the roster unasked after any change.
pub fn guild_roster() -> Vec<u8> {
    Vec::new()
}

/// Body of `CMSG_GUILD_PROMOTE` (`Server/Packets/Guild.cpp:23-26`): the name, moved one rank up.
pub fn guild_promote(name: &str) -> Vec<u8> {
    cstring_body(name)
}

/// Body of `CMSG_GUILD_DEMOTE` (`Server/Packets/Guild.cpp:28-31`): the name, moved one rank down.
pub fn guild_demote(name: &str) -> Vec<u8> {
    cstring_body(name)
}

/// Body of `CMSG_GUILD_LEAVE`: empty (`Server/Protocol/Opcodes.cpp:222`). A guild master is
/// refused `(QUIT, LEADER_LEAVE)` while anyone else remains.
pub fn guild_leave() -> Vec<u8> {
    Vec::new()
}

/// Body of `CMSG_GUILD_REMOVE` (`Server/Packets/Guild.cpp:18-21`): the member to kick.
pub fn guild_remove(name: &str) -> Vec<u8> {
    cstring_body(name)
}

/// Body of `CMSG_GUILD_DISBAND`: empty (`Server/Protocol/Opcodes.cpp:224`); guild master only.
pub fn guild_disband() -> Vec<u8> {
    Vec::new()
}

/// Body of `CMSG_GUILD_LEADER` (`Server/Packets/Guild.cpp:33-36`): the new guild master's name.
pub fn guild_leader(name: &str) -> Vec<u8> {
    cstring_body(name)
}

/// Body of `CMSG_GUILD_MOTD` (`Server/Packets/Guild.cpp:38-42`): the new MOTD; `""`, a lone NUL,
/// clears it.
pub fn guild_motd(motd: &str) -> Vec<u8> {
    cstring_body(motd)
}

/// Body of `CMSG_GUILD_RANK` (`Server/Packets/Guild.cpp:78-83`): rank id, rights, name. Both are
/// set, so changing one resends the other; rank 0's rights become [`guild_rank_right::ALL`], and
/// a name over [`GUILD_RANK_MAX_LENGTH`] gets the session kicked.
pub fn guild_rank(rank_id: u32, rights: u32, name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(9 + name.len());
    body.extend_from_slice(&rank_id.to_le_bytes());
    body.extend_from_slice(&rights.to_le_bytes());
    push_cstring(&mut body, name);
    body
}

/// Body of `CMSG_GUILD_ADD_RANK` (`Server/Packets/Guild.cpp:73-76`): the name. The rank goes at
/// the bottom with `GCHAT_LISTEN | GCHAT_SPEAK` (`Handlers/GuildHandler.cpp:623`); the server
/// ignores the packet once the guild has [`GUILD_RANKS_MAX_COUNT`] ranks.
pub fn guild_add_rank(name: &str) -> Vec<u8> {
    cstring_body(name)
}

/// Body of `CMSG_GUILD_DEL_RANK`: empty (`Server/Protocol/Opcodes.cpp:655`); the server always
/// deletes the lowest rank.
pub fn guild_del_rank() -> Vec<u8> {
    Vec::new()
}

/// Body of `CMSG_GUILD_SET_PUBLIC_NOTE` (`Server/Packets/Guild.cpp:61-65`): name, then note.
pub fn guild_set_public_note(name: &str, note: &str) -> Vec<u8> {
    two_cstring_body(name, note)
}

/// Body of `CMSG_GUILD_SET_OFFICER_NOTE` (`Server/Packets/Guild.cpp:67-71`): name, then note;
/// the server requires [`guild_rank_right::EDIT_OFFICER_NOTE`].
pub fn guild_set_officer_note(name: &str, note: &str) -> Vec<u8> {
    two_cstring_body(name, note)
}

/// Body of `CMSG_GUILD_INFO_TEXT` (`Server/Packets/Guild.cpp:44-49`): the guild information text;
/// vmangos handles it only for builds above `CLIENT_BUILD_1_8_4`.
pub fn guild_info_text(text: &str) -> Vec<u8> {
    cstring_body(text)
}

fn cstring_body(s: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(s.len() + 1);
    push_cstring(&mut body, s);
    body
}

fn two_cstring_body(a: &str, b: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(a.len() + b.len() + 2);
    push_cstring(&mut body, a);
    push_cstring(&mut body, b);
    body
}

fn push_cstring(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_right_checkbox_order_is_not_bit_order() {
        assert_eq!(GUILD_RANK_RIGHT_ORDER[4], 0x0000_0080, "idx 5 = Promote");
        assert_eq!(GUILD_RANK_RIGHT_ORDER[5], 0x0000_0100, "idx 6 = Demote");
        assert_eq!(GUILD_RANK_RIGHT_ORDER[6], 0x0000_0010, "idx 7 = Invite");
        assert_eq!(GUILD_RANK_RIGHT_ORDER[7], 0x0000_0020, "idx 8 = Remove");
        assert!(
            GUILD_RANK_RIGHT_ORDER[4] > GUILD_RANK_RIGHT_ORDER[6],
            "the order is deliberately non-monotonic in bit value"
        );
        for right in GUILD_RANK_RIGHT_ORDER {
            assert_eq!(right.count_ones(), 1, "{right:#x} is one bit");
            assert_ne!(right, guild_rank_right::EMPTY);
        }
    }

    /// The expected values are vmangos's `GuildRankRights` (`Guild/Guild.h:56-74`).
    #[test]
    fn rank_rights_reconstruct_the_vmangos_constants() {
        use guild_rank_right::*;
        assert_eq!(GCHAT_LISTEN | EMPTY, 0x0000_0041);
        assert_eq!(GCHAT_SPEAK | EMPTY, 0x0000_0042);
        assert_eq!(OFFCHAT_LISTEN | EMPTY, 0x0000_0044);
        assert_eq!(OFFCHAT_SPEAK | EMPTY, 0x0000_0048);
        assert_eq!(PROMOTE | EMPTY, 0x0000_00C0);
        assert_eq!(DEMOTE | EMPTY, 0x0000_0140);
        assert_eq!(INVITE | EMPTY, 0x0000_0050);
        assert_eq!(REMOVE | EMPTY, 0x0000_0060);
        assert_eq!(SET_MOTD | EMPTY, 0x0000_1040);
        assert_eq!(EDIT_PUBLIC_NOTE | EMPTY, 0x0000_2040);
        assert_eq!(VIEW_OFFICER_NOTE | EMPTY, 0x0000_4040);
        assert_eq!(EDIT_OFFICER_NOTE | EMPTY, 0x0000_8040);
        assert_eq!(MODIFY_GUILD_INFO | EMPTY, 0x0001_0040);
    }
}
