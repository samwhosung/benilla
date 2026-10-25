//! The guild getters' questions of the roster and the guild record, answered through the lent host
//! ([`crate::script::live::ask`]) as of the place the getter runs. The reference's getters read
//! the roster module's cells (`0xb73118`, `0xb72704`, `0xb726d0`, `0xb72f00`) and the active
//! player's guild fields (`[[player+0xe68]+0xc]`, `+0x10`) at the call, and look the guild's
//! record up with a callback (`0x560e30`, callback `0x4d1480`): `GetGuildRosterInfo` (`0x4d1200`),
//! `GuildControlGetRankName` (`0x4d1e90`) and `GetGuildInfo` (`0x4c9330`) answer nil while the
//! record is not cached. So a listener reads the roster its own packet left. With no host free to
//! ask, the getters read the pushed snapshot ([`super::GuildState`]).

use super::{GuildMemberInfo, UnitGuild};

/// What a guild getter asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuildRead {
    /// The active player's own standing: `IsInGuild` (`0x516de0`), `IsGuildLeader` (`0x516e40`) and
    /// the nine `Can*` predicates (`0x4d18c0`).
    Own,
    /// `GetGuildRosterMOTD` (`0x4d11f0`) and `GetGuildInfoText`: the roster's two free-text blocks.
    Texts,
    /// `GetNumGuildMembers` (`0x4d1190`): the whole roster when `all` or show-offline is set, else
    /// the online tally. The roster's size for the index-taking bindings, whatever the flag.
    Count { all: bool, show_offline: bool },
    /// `GetGuildRosterInfo(index)` and `GetGuildRosterLastOnline(index)`: the row at the 1-based
    /// `index` of the order show-offline gives. `named` is set where the getter reads the guild
    /// record for the rank's name and looks it up with its callback (`0x4d1200` at `0x4d1291`), so
    /// a record not cached fails the call; `GetGuildRosterLastOnline` reads the row alone.
    Member {
        index: i64,
        show_offline: bool,
        named: bool,
    },
    /// `SetGuildRosterSelection(index)` (`0x4d1820`): the guid at the 1-based `index`.
    GuidAt { index: i64, show_offline: bool },
    /// `GetGuildRosterSelection` (`0x4d1890` → `0x4d1030`): the 0-based row holding `guid`.
    RowOf { guid: u64, show_offline: bool },
    /// `GuildControlGetNumRanks` (`0x4d1e60`): the rank count the roster carried (`0xb73120`).
    RankCount,
    /// `GuildControlGetRankName(index)` (`0x4d1e90`): the 0-based rank's name from the guild
    /// record.
    RankName(u32),
    /// The rank-rights word of the 0-based rank (`0xb726d0`), 0 at ten and past.
    Rights(u32),
    /// `GetGuildInfo(unit)` (`0x4c9330`) for the unit under `guid`.
    Unit(u64),
}

/// The active player's guild standing, from the descriptor fields (`[[player+0xe68]+0xc]`,
/// `+0x10`) and the rank-rights word the roster carried.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnGuild {
    /// `PLAYER_GUILDID` is set.
    pub in_guild: bool,
    /// `PLAYER_GUILDRANK`, 0-based, 0 the guild master.
    pub rank_index: u32,
    /// The rights of that rank (`0xb726d0`, 0 at ten and past).
    pub rights: u32,
}

/// What `GetGuildInfo(unit)` finds (`0x4c9330`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitGuildRead {
    /// No held object, not a player, or no `PLAYER_GUILDID`: `nil, nil, 0`.
    None,
    /// The guild's record is not cached: `nil, nil, 0`, and the lookup left the unit's waiter
    /// (`0x4c93d2`).
    Miss { guild_id: u32 },
    /// The record's name and the unit's rank in it.
    Named(UnitGuild),
}

/// A [`GuildRead`]'s answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuildAnswer {
    /// [`GuildRead::Own`]; `None` while no active player is held.
    Own(Option<OwnGuild>),
    /// [`GuildRead::Texts`].
    Texts { motd: String, info: String },
    /// [`GuildRead::Count`] and [`GuildRead::RankCount`].
    Count(usize),
    /// [`GuildRead::Member`]; `None` for the failing tuple (`0x4d1412`): no player, an index out of
    /// range, or the record not cached.
    Member(Option<GuildMemberInfo>),
    /// [`GuildRead::GuidAt`].
    Guid(Option<u64>),
    /// [`GuildRead::RowOf`].
    Row(Option<usize>),
    /// [`GuildRead::RankName`]; `None` for nil (`0x4d1f80`).
    RankName(Option<String>),
    /// [`GuildRead::Rights`].
    Rights(u32),
    /// [`GuildRead::Unit`].
    Unit(UnitGuildRead),
}

impl crate::script::live::LiveQuery for GuildRead {
    type Answer = GuildAnswer;
}

/// `read` through the lent host; `None` with no host free to ask, where the snapshot answers.
pub(crate) fn read_live(lua: &mlua::Lua, read: GuildRead) -> Option<GuildAnswer> {
    crate::script::live::ask(lua, &read)
}
