//! The guild bindings: roster, ranks, notes, MOTD and the membership verbs. The app pushes a
//! display-ready [`GuildState`] ([`UiScript::set_guild`]) that joins two caches: the roster (MOTD,
//! info text, rank rights, members) and the guild query (the name and ten rank names). Each verb
//! hands its [`GuildCall`] to the host, which applies it before the verb returns, as the
//! reference's bindings send and re-sort in their own bodies (`0x48aef0`, `0x4d10d0`, `0x4d0fb0`).
//!
//! Two index bases: a roster `rankIndex` is 0-based with 0 the guild master
//! (`FriendsFrame.lua:338`, `:393-398`), while the `GuildControl*` family is 1-based, driven by
//! the dropdown IDs (`FriendsFrame.lua:820-822`, `:855-857`, `:866-868`).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// The thirteen rank rights in `GuildControlGetRankFlags`' return order, which is also the
/// checkbox order and `GuildControlSetRankFlag`'s index: only position ties a flag to its checkbox
/// (`FriendsFrame.lua:874-884`). The labels are `GUILDCONTROL_OPTION1..13`
/// (`GlobalStrings.lua:2036-2048`).
///
/// Not bit order: Promote and Demote (5, 6) are `0x80` and `0x100`, above Invite and Remove
/// (7, 8) at `0x10` and `0x20`, so `1 << (i - 1)` is wrong for nine of the thirteen. The bits are
/// vmangos `GuildRankRights` (`Guild/Guild.h:56-74`) without the `GR_RIGHT_EMPTY` `0x40` it ORs
/// into each; `0x40`, `0x400` and `0x800` are none of the thirteen.
pub const RANK_RIGHT_BITS: [u32; 13] = [
    0x0000_0001, // 1  Guildchat Listen
    0x0000_0002, // 2  Guildchat Speak
    0x0000_0004, // 3  Officerchat Listen
    0x0000_0008, // 4  Officerchat Speak
    0x0000_0080, // 5  Promote
    0x0000_0100, // 6  Demote
    0x0000_0010, // 7  Invite Member
    0x0000_0020, // 8  Remove Member
    0x0000_1000, // 9  Set MOTD
    0x0000_2000, // 10 Edit Public Note
    0x0000_4000, // 11 View Officer Note
    0x0000_8000, // 12 Edit Officer Note
    0x0001_0000, // 13 Modify Guild Info
];

/// One-based indices into [`RANK_RIGHT_BITS`], numbered as `GUILDCONTROL_OPTION<n>`.
mod right {
    pub(super) const PROMOTE: usize = 5;
    pub(super) const DEMOTE: usize = 6;
    pub(super) const INVITE: usize = 7;
    pub(super) const REMOVE: usize = 8;
    pub(super) const SET_MOTD: usize = 9;
    pub(super) const EDIT_PUBLIC_NOTE: usize = 10;
    pub(super) const VIEW_OFFICER_NOTE: usize = 11;
    pub(super) const EDIT_OFFICER_NOTE: usize = 12;
    pub(super) const MODIFY_GUILD_INFO: usize = 13;
}

/// The fewest ranks: `GuildControlDelRank` refuses at it and the Remove button shows only above it
/// (`FriendsFrame.lua:908`); vmangos `GUILD_RANKS_MIN_COUNT` (`Guild/Guild.h:31`).
pub const MIN_RANKS: usize = 5;
/// The most ranks: `GuildControlAddRank` refuses at it and the Add button disables there
/// (`FriendsFrame.lua:887`); vmangos `GUILD_RANKS_MAX_COUNT` (`Guild/Guild.h:32`).
pub const MAX_RANKS: usize = 10;

/// `GetGuildRosterLastOnline`'s four units, decomposed by the app from the wire's one `f32` of
/// days offline. `GuildFrame_GetLastOnline` shows the largest non-zero unit, reading nil as 0
/// (`FriendsFrame.lua:957-978`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LastOnline {
    pub years: u32,
    pub months: u32,
    pub days: u32,
    pub hours: u32,
}

/// A unit's guild as `GetGuildInfo(unit)` reports it. `PLAYER_GUILDID` (191) and
/// `PLAYER_GUILDRANK` (192) are public descriptor fields (vmangos
/// `Objects/UpdateFields_1_12_1.h:121-122`), so every visible player carries one; the app names
/// them through its guild query cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitGuild {
    /// Never empty: an unresolved guild is `None`, as the binding takes the same nil path on a
    /// cache miss as for no guild (`0x4c93d7 je 0x4c943c`).
    pub name: String,
    pub rank_name: String,
    /// 0-based, 0 the guild master.
    pub rank_index: u32,
}

/// One guild rank: its name from the guild query, its rights from the roster.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildRankInfo {
    /// Empty until the guild query names it.
    pub name: String,
    /// Tested with [`RANK_RIGHT_BITS`].
    pub rights: u32,
}

/// One roster row resolved for display: `GetGuildRosterInfo`'s ten returns in order, then the
/// last-online time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildMemberInfo {
    /// The member's guid, which `SetGuildRosterSelection` stores (`0xb73128`); no getter returns
    /// it.
    pub guid: u64,
    /// On the wire in the roster itself, so it never waits on a name query.
    pub name: String,
    /// The rank's name.
    pub rank: String,
    /// 0-based, 0 the guild master; the `GuildControl*` family is 1-based.
    pub rank_index: u32,
    pub level: u32,
    /// The localized class name. The roster carries class, level and zone for offline members too.
    pub class: String,
    /// The zone name, empty when the id has no `AreaTable` row.
    pub zone: String,
    /// The public note.
    pub note: String,
    /// Empty both when unset and when the server blanks it for a viewer without View Officer
    /// Note; the frame gates on `CanViewOfficerNote` instead.
    pub officer_note: String,
    pub online: bool,
    /// The record's presence byte (`+0x4c`), whose bits the tenth return reads at the call: bit 4
    /// `CHAT_FLAG_DND`, else bit 2 `CHAT_FLAG_AFK`, else `""` (`0x4d13e4`, `0x4d13fb`).
    pub presence: u8,
    /// Meaningful only offline; the wire omits it for online members.
    pub last_online: LastOnline,
}

/// The guild snapshot the app pushes whole when it changes ([`UiScript::set_guild`]); the default
/// is guildless.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildState {
    /// `IsInGuild`, from the player's own `PLAYER_GUILDID` (191), so true before any roster.
    pub in_guild: bool,
    /// Only `SMSG_GUILD_QUERY_RESPONSE` carries it, so it is empty until that query answers.
    pub name: String,
    pub rank_name: String,
    /// The player's own rank, 0-based, 0 the guild master.
    pub rank_index: u32,
    /// `IsGuildLeader`: the player is the guild master.
    pub is_leader: bool,
    /// The player's own rank's rights, which every `Can*` predicate tests.
    pub rights: u32,
    pub motd: String,
    /// The `GuildInfoFrame` body.
    pub info_text: String,
    /// The whole roster, sorted and never filtered: `GetGuildRosterInfo`,
    /// `GetGuildRosterLastOnline` and `SetGuildRosterSelection` index it 1-based.
    pub roster: Vec<GuildMemberInfo>,
    /// `GetNumGuildMembers()`, the UI's loop bound, which respects the show-offline flag. The
    /// index-taking bindings bound against the full count (`[0xb73118]`) and never read the flag,
    /// so an index past this still returns a real offline member.
    pub num_members: usize,
    /// Every rank, 0 the guild master; the length is `GuildControlGetNumRanks`.
    pub ranks: Vec<GuildRankInfo>,
}

/// The rank-control popup's staging buffer, kept out of [`GuildState`] so a push mid-edit does not
/// discard unsaved checkbox clicks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildRankEdit {
    /// The 1-based rank `GuildControlSetRank` last loaded, 0 for none.
    pub rank: u32,
    /// The thirteen staged checkboxes in [`RANK_RIGHT_BITS`] order, one slot per flag as the
    /// binding stages them, so the save leaves every other bit of the live rights alone.
    pub staged: [bool; 13],
}

/// A guild verb past its binding's checks, which the host applies inside the call
/// ([`super::ScriptCall::Guild`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuildCall {
    /// `GuildRoster()` (`0x4d10d0`): `CMSG_GUILD_ROSTER` at most once per 10 000 ms of the
    /// deadline `0xb73130`, which a stale `GUILD_ROSTER_UPDATE` clears (`0x4d1170`).
    Roster,
    /// `GuildInfo()`, the `/ginfo` query (`CMSG_GUILD_INFO`), answered as a chat line.
    Info,
    /// `GuildInviteByName(name)`.
    Invite(String),
    /// `GuildUninviteByName(name)` (`CMSG_GUILD_REMOVE`).
    Uninvite(String),
    /// `GuildPromoteByName(name)`.
    Promote(String),
    /// `GuildDemoteByName(name)`.
    Demote(String),
    /// `GuildSetLeaderByName(name)`.
    SetLeader(String),
    /// `AcceptGuild()`.
    Accept,
    /// `DeclineGuild()`.
    Decline,
    /// `GuildLeave()`.
    Leave,
    /// `GuildDisband()`.
    Disband,
    /// `GuildSetMOTD(text)`; an empty string clears it.
    SetMotd(String),
    /// `SetGuildInfoText(text)`.
    SetInfoText(String),
    /// `GuildRosterSetPublicNote(index, note)`: a 1-based roster row the host resolves to the
    /// member whose name the wire takes.
    SetPublicNote { index: u32, note: String },
    /// `GuildRosterSetOfficerNote(index, note)`.
    SetOfficerNote { index: u32, note: String },
    /// `GuildControlSaveRank(name)` as one `CMSG_GUILD_RANK`; the rank is 0-based, as on the wire.
    SaveRank {
        rank_index: u32,
        rights: u32,
        name: String,
    },
    /// `GuildControlAddRank(name)`.
    AddRank(String),
    /// `GuildControlDelRank()`: no argument and an empty `0x233`; the server deletes the last rank.
    DelRank,
    /// `SetGuildRosterShowOffline(flag)` (`0x4d1e10` → `0x4d0f70`) changed the flag with a member
    /// held: the host re-sorts by [`UiScript::guild_show_offline`] and signals.
    SetShowOffline,
    /// `SortGuildRoster(field)` (`0x4d1cb0` → `0x4d0fb0`): `"name"`, `"zone"`, `"level"`, `"class"`,
    /// `"rank"`, `"note"` or `"online"`, the headers' `sortType`s (`FriendsFrame.xml:1895-1940`,
    /// `:2086-2131`).
    Sort(String),
}

/// The roster module's cells a `/reload` keeps: `SetGuildRosterShowOffline`'s integer
/// (`0xb73124`) and `SetGuildRosterSelection`'s member guid (`0xb73128`). Only `InitGuildRoster`
/// zeroes them (`0x4d0a25`, `0x4d0a2b`), from `InitializeGame` (`0x48f5ea`), which the login runs
/// and the reload (`0x495664`, `0x495669`) does not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuildCells {
    /// The integer as stored, every reader testing it against 0.
    pub show_offline: i32,
    /// The selected member's guid, 0 for none.
    pub selection: u64,
}

impl super::UiScript {
    /// Push the guild snapshot, replacing it. The app fires `GUILD_ROSTER_UPDATE`,
    /// `PLAYER_GUILD_UPDATE` and `GUILD_MOTD` on the edges.
    pub fn set_guild(&mut self, state: GuildState) {
        self.model_mut().guild = state;
    }

    /// `SetGuildRosterShowOffline`'s integer as `0x4d0f70` stored it (`0xb73124`), every reader
    /// testing it against 0: the roster's order and `GetNumGuildMembers` follow it.
    pub fn guild_show_offline(&self) -> i32 {
        self.model_ref().guild_cells.show_offline
    }

    /// Take the roster's cells out of a dying VM, for [`Self::seed_guild_cells`] to seat in the
    /// next.
    pub fn take_guild_cells(&mut self) -> GuildCells {
        std::mem::take(&mut self.model_mut().guild_cells)
    }

    /// Seat a `/reload`'s VM with the roster's cells the last one held, before anything runs on it.
    pub fn seed_guild_cells(&mut self, cells: GuildCells) {
        self.model_mut().guild_cells = cells;
    }

    /// The guilds `GetGuildInfo` found no record for since the last take, each `(guild_id, guid)`:
    /// the lookup with its callback (`0x4c93d2`), which asks and leaves a waiter for the unit.
    pub fn take_guild_info_asks(&mut self) -> Vec<(u32, u64)> {
        std::mem::take(&mut self.model_mut().guild_info_asks)
    }

    /// The guild calls a harness VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_guild_calls(&mut self) -> Vec<GuildCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Guild(call) => Some(call.clone()),
            _ => None,
        })
    }
}

/// Hand `call` to the host, which applies it before the verb returns.
fn submit(lua: &Lua, call: GuildCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Guild(call))
}

/// A roster row by 1-based index.
fn member_at(roster: &[GuildMemberInfo], index: i64) -> Option<&GuildMemberInfo> {
    usize::try_from(index)
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| roster.get(i))
}

/// Whether the player's own rank holds right `index`, 1-based into [`RANK_RIGHT_BITS`].
fn has_right(model: &Model, index: usize) -> bool {
    model.guild.in_guild && model.guild.rights & RANK_RIGHT_BITS[index - 1] != 0
}

/// The live rights of a 1-based rank, 0 for none or out of range. `GuildControlGetRankFlags` and
/// `GuildControlSaveRank`'s fold both read this, never the staging buffer.
fn live_rights(model: &Model, rank_one_based: u32) -> u32 {
    (rank_one_based as usize)
        .checked_sub(1)
        .and_then(|i| model.guild.ranks.get(i))
        .map(|r| r.rights)
        .unwrap_or(0)
}

/// The rank-name gate `GuildControlSaveRank` (`0x4d20d0`) and `GuildControlAddRank` (`0x4d2210`)
/// run before any packet: the name up to the first NUL must be 1 to 16 UTF-16 code units
/// (`0x65b250`), untrimmed, and is sent as measured. vmangos kicks a name over 15 code points; the
/// app caps that at the send (`ui_guild::feed::capped_rank_name`).
fn gated_rank_name(name: &str) -> Option<&str> {
    let c_name = name.split('\0').next().unwrap_or_default();
    (1..=16)
        .contains(&c_name.encode_utf16().count())
        .then_some(c_name)
}

/// Register the guild globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── Membership and identity ──────────────────────────────────────────────────────────────
    // IsInGuild() (`0x516de0`) reads the player's own `PLAYER_GUILDID`, so it answers before any
    // roster arrives.
    g.set(
        "IsInGuild",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(model.guild.in_guild))
        })?,
    )?;

    // IsGuildLeader() (`0x516e40`) takes no argument: whether the player is the guild master.
    g.set(
        "IsGuildLeader",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(model.guild.is_leader))
        })?,
    )?;

    // GetGuildInfo(unit) → guildName, rankName, rankIndex (`0x4c9330`), per unit because the rank
    // fields are public: it reads the unit snapshot, not the player's guild. The general resolver
    // (`0x515940`) raises on an unrecognised token (`0x515c1a`); a recognised but unresolvable
    // token, a non-player, a guildless player and an unarrived guild record all answer
    // `nil, nil, 0`, the third the number 0 (`mov eax,3` on both legs). Stock
    // `PaperDollFrame_SetGuild` calls it for `"player"` unguarded (`PaperDollFrame.lua:117`). A
    // player's guild the cache lacks is asked with a waiter of the call's own
    // ([`UiScript::take_guild_info_asks`]); the zero key never is (`0x560e3d`).
    g.set(
        "GetGuildInfo",
        lua.create_function(|lua, unit: Option<String>| {
            crate::script::unit::check_unit_token(&unit)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let Some(state) = unit.as_deref().and_then(|u| model.unit(u)) else {
                return Ok((Value::Nil, Value::Nil, Value::Integer(0)));
            };
            let Some(guild) = state.guild.as_ref() else {
                let miss = (state.has_object && state.is_player && state.guild_id != 0)
                    .then_some((state.guild_id, state.guid));
                model.guild_info_asks.extend(miss);
                return Ok((Value::Nil, Value::Nil, Value::Integer(0)));
            };
            Ok((
                Value::String(lua.create_string(&guild.name)?),
                Value::String(lua.create_string(&guild.rank_name)?),
                Value::Integer(i64::from(guild.rank_index)),
            ))
        })?,
    )?;

    // ── The roster ───────────────────────────────────────────────────────────────────────────
    // GetNumGuildMembers([includeOffline]) (`0x4d1190`): the loop bound (`FriendsFrame.lua:446`),
    // which respects show-offline, not the addressable range (`GuildState::num_members`).
    g.set(
        "GetNumGuildMembers",
        lua.create_function(|lua, args: MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // The flag is `GetBoolOrDefault` (`0x4d11a0`, default 0): a set one counts the whole
            // roster (`[0xb73118]`) whatever show-offline says, else the loop bound.
            if binding_abi::bool_or_default(args.front(), false) {
                return Ok(model.guild.roster.len() as i64);
            }
            Ok(model.guild.num_members as i64)
        })?,
    )?;

    // GetGuildRosterInfo(index) → name, rank, rankIndex, level, class, zone, note, officernote,
    // online, status (`0x4d1200`). Ten returns: only the status view takes the tenth
    // (`FriendsFrame.lua:541`), every other call site nine. Out of range answers ten nils rather
    // than raising: `GuildStatus_Update` passes the selection, 0 when none, before testing it
    // (`FriendsFrame.lua:344`, `:347`).
    g.set(
        "GetGuildRosterInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetGuildRosterInfo(index)` (`0x849aec`, `0x4d122b`).
            let index = i64::from(binding_abi::number_arg(
                lua,
                index,
                "Usage: GetGuildRosterInfo(index)",
            )?);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(m) = member_at(&model.guild.roster, index) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil; 10]));
            };
            // `FrameScript_GetText` at the call, the empty string for a missing global
            // (`0x703bf0`): do-not-disturb before away (`0x4d13ed`, `0x4d1404`).
            let status = if m.presence & 0x4 != 0 {
                super::glue_core::global_text(lua, "CHAT_FLAG_DND")
            } else if m.presence & 0x2 != 0 {
                super::glue_core::global_text(lua, "CHAT_FLAG_AFK")
            } else {
                String::new()
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&m.name)?),
                Value::String(lua.create_string(&m.rank)?),
                Value::Integer(i64::from(m.rank_index)),
                Value::Integer(i64::from(m.level)),
                Value::String(lua.create_string(&m.class)?),
                Value::String(lua.create_string(&m.zone)?),
                Value::String(lua.create_string(&m.note)?),
                Value::String(lua.create_string(&m.officer_note)?),
                binding_abi::flag(m.online),
                Value::String(lua.create_string(&status)?),
            ]))
        })?,
    )?;

    // GetGuildRosterLastOnline(index) → years, months, days, hours (`0x4d14a0`), all 0 for an
    // online member, whom the wire sends no time for.
    g.set(
        "GetGuildRosterLastOnline",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetGuildRosterLastOnline(index)` (`0x849b10`, `0x4d14b7`).
            let index = i64::from(binding_abi::number_arg(
                lua,
                index,
                "Usage: GetGuildRosterLastOnline(index)",
            )?);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(m) = member_at(&model.guild.roster, index) else {
                return Ok((Value::Nil, Value::Nil, Value::Nil, Value::Nil));
            };
            let last = m.last_online;
            Ok((
                Value::Integer(i64::from(last.years)),
                Value::Integer(i64::from(last.months)),
                Value::Integer(i64::from(last.days)),
                Value::Integer(i64::from(last.hours)),
            ))
        })?,
    )?;

    // GetGuildRosterMOTD() / GetGuildInfoText(): the two free-text blocks the roster carries.
    g.set(
        "GetGuildRosterMOTD",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.guild.motd)
        })?,
    )?;
    g.set(
        "GetGuildInfoText",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.guild.info_text)
        })?,
    )?;

    // GetGuildRosterSelection() (`0x4d1890`): the stored member's 1-based row, searched for at
    // the call (`0x4d1030`), so a re-sort keeps the same player selected and one who left reads
    // 0, as does no selection (`FriendsFrame.lua:347` tests `> 0`).
    g.set(
        "GetGuildRosterSelection",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let guid = model.guild_cells.selection;
            let row = (guid != 0)
                .then(|| model.guild.roster.iter().position(|m| m.guid == guid))
                .flatten();
            Ok(row.map_or(0, |i| i as i64 + 1))
        })?,
    )?;
    g.set(
        "SetGuildRosterSelection",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: SetGuildRosterSelection(index)` (`0x849b98`, `0x4d1831`).
            let index = i64::from(binding_abi::number_arg(
                lua,
                index,
                "Usage: SetGuildRosterSelection(index)",
            )?);
            // The row's member's guid, or 0 for an index out of range (`0x4d186a`/`0x4d1872`).
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.guild_cells.selection =
                member_at(&model.guild.roster, index).map_or(0, |m| m.guid);
            Ok(())
        })?,
    )?;

    // GetGuildRosterShowOffline() (`0x4d1e30`) answers 1 for any stored value but 0.
    g.set(
        "GetGuildRosterShowOffline",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(model.guild_cells.show_offline != 0))
        })?,
    )?;
    // SetGuildRosterShowOffline([flag]) (`0x4d1e10`): `GetScriptFlagArg` (`0x4d1e17`) with default
    // 1, so no argument turns show-offline on, unlike an explicit `nil`, hence `MultiValue`. Its
    // integer is what `0x4d0f70` compares and stores (`cmp ecx,[0xb73124]`), so a 2 after a 1 is a
    // change; with a member held the host re-sorts and signals it (`0x4d0f85`-`0x4d0f9f`).
    g.set(
        "SetGuildRosterShowOffline",
        lua.create_function(|lua, args: MultiValue| {
            let flag = binding_abi::flag_or_default(args.front(), 1);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.guild_cells.show_offline == flag {
                return Ok(());
            }
            model.guild_cells.show_offline = flag;
            if model.guild.roster.is_empty() {
                return Ok(());
            }
            drop(model);
            submit(lua, GuildCall::SetShowOffline)
        })?,
    )?;

    // SortGuildRoster(field) (`0x4d1cb0`): the host re-sorts and signals `GUILD_ROSTER_UPDATE`
    // inside the call (`0x4d1022`), without asking the server; the header's OnClick calls no
    // `GuildStatus_Update` and no `GuildRoster` (`FriendsFrame.xml:442-446`).
    g.set(
        "SortGuildRoster",
        lua.create_function(|lua, field: Value| {
            // `lua_isstring` (`0x4d1cb8`), else the binary's own spelling (`0x849bf0`).
            let field = binding_abi::string_arg(lua, field, "Usgae: SortGuildRoster(\"type\")")?;
            submit(lua, GuildCall::Sort(field))
        })?,
    )?;

    // CloseGuildRoster() is `xor eax,eax; ret` in the reference: a no-op that sends nothing.
    g.set("CloseGuildRoster", lua.create_function(|_, ()| Ok(()))?)?;

    // The argument-less verbs; the host throttles `GuildRoster()` ([`GuildCall::Roster`]).
    for (global, call) in [
        ("GuildRoster", GuildCall::Roster),
        ("GuildInfo", GuildCall::Info),
        ("AcceptGuild", GuildCall::Accept),
        ("DeclineGuild", GuildCall::Decline),
        ("GuildLeave", GuildCall::Leave),
        ("GuildDisband", GuildCall::Disband),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, ()| submit(lua, call.clone()))?,
        )?;
    }

    // ── The by-name verbs ────────────────────────────────────────────────────────────────────
    // Five name-taking verbs (`0x48aef0`, `0x48afb0`, `0x48b050`, `0x48b0f0`, `0x48b190`). An empty
    // or whitespace name is not sent. vmangos ignores an empty one without a reply, except an
    // invite, which it answers with `ERR_GUILD_PLAYER_NOT_FOUND_S`
    // (`Handlers/GuildHandler.cpp:77-83`).
    for (global, make) in [
        ("GuildInviteByName", GuildCall::Invite as fn(String) -> _),
        (
            "GuildUninviteByName",
            GuildCall::Uninvite as fn(String) -> _,
        ),
        ("GuildPromoteByName", GuildCall::Promote as fn(String) -> _),
        ("GuildDemoteByName", GuildCall::Demote as fn(String) -> _),
        (
            "GuildSetLeaderByName",
            GuildCall::SetLeader as fn(String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, name: String| {
                if name.trim().is_empty() {
                    return Ok(());
                }
                submit(lua, make(name))
            })?,
        )?;
    }

    // GuildSetMOTD(text) (`0x48b270`) / SetGuildInfoText(text) (`0x4d2380`): an empty string is
    // sent, and clears the text.
    for (global, make) in [
        ("GuildSetMOTD", GuildCall::SetMotd as fn(String) -> _),
        (
            "SetGuildInfoText",
            GuildCall::SetInfoText as fn(String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, text: String| submit(lua, make(text)))?,
        )?;
    }

    // GuildRosterSetPublicNote(index, note) (`0x4d15e0`) / GuildRosterSetOfficerNote (`0x4d1700`)
    // take a roster row, which the host resolves to the name the wire carries.
    for (global, make) in [
        (
            "GuildRosterSetPublicNote",
            (|index, note| GuildCall::SetPublicNote { index, note }) as fn(u32, String) -> _,
        ),
        (
            "GuildRosterSetOfficerNote",
            (|index, note| GuildCall::SetOfficerNote { index, note }) as fn(u32, String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, (index, note): (i64, String)| {
                let Ok(index) = u32::try_from(index) else {
                    return Ok(());
                };
                if index < 1 {
                    return Ok(());
                }
                submit(lua, make(index, note))
            })?,
        )?;
    }

    // ── The permission predicates ────────────────────────────────────────────────────────────
    // Nine predicates, each one bit of the player's own rank's rights (`0x4d18c0`, `0x4d1930`,
    // `0x4d19a0`, `0x4d1a10`, `0x4d1c40` among them).
    for (global, index) in [
        ("CanGuildPromote", right::PROMOTE),
        ("CanGuildDemote", right::DEMOTE),
        ("CanGuildInvite", right::INVITE),
        ("CanGuildRemove", right::REMOVE),
        ("CanEditMOTD", right::SET_MOTD),
        ("CanEditPublicNote", right::EDIT_PUBLIC_NOTE),
        ("CanViewOfficerNote", right::VIEW_OFFICER_NOTE),
        ("CanEditOfficerNote", right::EDIT_OFFICER_NOTE),
        ("CanEditGuildInfo", right::MODIFY_GUILD_INFO),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, ()| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                Ok(binding_abi::flag(has_right(&model, index)))
            })?,
        )?;
    }

    // ── The rank-control popup ───────────────────────────────────────────────────────────────
    // GuildControlGetNumRanks() (`0x4d1e60`): the guild's rank count.
    g.set(
        "GuildControlGetNumRanks",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.guild.ranks.len() as i64)
        })?,
    )?;

    // GuildControlGetRankName(index) (`0x4d1e90`): 1-based, the dropdown's ID.
    g.set(
        "GuildControlGetRankName",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GuildControlGetRankName(index)` (`0x849c10`, `0x4d1ebe`).
            let index = i64::from(binding_abi::number_arg(
                lua,
                index,
                "Usage: GuildControlGetRankName(index)",
            )?);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let name = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.guild.ranks.get(i))
                .map(|r| r.name.as_str())
                .unwrap_or_default();
            lua.create_string(name)
        })?,
    )?;

    // GuildControlSetRank(index) (`0x4d1fa0`): load a 1-based rank into the staging buffer, each
    // slot seeded from its live rights. 0 clears the selection rather than loading rank 1, so a
    // 0-based `rankIndex` passed by mistake loads nothing.
    g.set(
        "GuildControlSetRank",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GuildControlSetRank(rank)` (`0x849c38`, `0x4d1fb1`).
            let index = i64::from(binding_abi::number_arg(
                lua,
                index,
                "Usage: GuildControlSetRank(rank)",
            )?);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let rank = u32::try_from(index).ok().filter(|i| *i >= 1).unwrap_or(0);
            let rights = live_rights(&model, rank);
            let mut staged = [false; 13];
            for (slot, bit) in staged.iter_mut().zip(RANK_RIGHT_BITS) {
                *slot = rights & bit != 0;
            }
            model.guild_control = GuildRankEdit { rank, staged };
            Ok(())
        })?,
    )?;

    // GuildControlGetRankFlags() (`0x4d1fe0`): thirteen returns in `RANK_RIGHT_BITS` order, from
    // the loaded rank's live rights, never the staging buffer, so a click shows only once the save
    // round-trips and a new roster lands. `GuildControlPopupFrame_OnLoad` calls it before any rank
    // is loaded (`FriendsFrame.lua:801`), when all thirteen are nil.
    g.set(
        "GuildControlGetRankFlags",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let rights = live_rights(&model, model.guild_control.rank);
            Ok(MultiValue::from_vec(
                RANK_RIGHT_BITS
                    .iter()
                    .map(|bit| binding_abi::flag(rights & bit != 0))
                    .collect(),
            ))
        })?,
    )?;

    // GuildControlSetRankFlag(index, enabled) (`0x4d2070`): stage one checkbox, 1 to 13.
    g.set(
        "GuildControlSetRankFlag",
        lua.create_function(|lua, (index, enabled): (Value, Option<Value>)| {
            // `is-number` else `Usage: GuildControlSetRankFlag(index, enabled)` (`0x849c5c`,
            // `0x4d2082`).
            let index = i64::from(binding_abi::number_arg(
                lua,
                index,
                "Usage: GuildControlSetRankFlag(index, enabled)",
            )?);
            let Some(slot) = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .filter(|i| *i < RANK_RIGHT_BITS.len())
            else {
                return Ok(());
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `GetBoolOrDefault` (`0x4d20b7`), default 0.
            model.guild_control.staged[slot] =
                binding_abi::bool_or_default(enabled.as_ref(), false);
            Ok(())
        })?,
    )?;

    // GuildControlSaveRank(name) (`0x4d20d0`): the edit box's name (`FriendsFrame.lua:839`) and the
    // staged flags as one `CMSG_GUILD_RANK` for the loaded rank. A name failing `gated_rank_name`
    // is a silent no-op; vmangos stores an empty one (`Handlers/GuildHandler.cpp:586`). The fold
    // sets or clears each staged bit over the freshly read live rights (`or mask[k]` /
    // `and ~mask[k]`), so a right can be turned off and bits outside the thirteen, such as `0x40`,
    // which the reference never tests, ride through as the server sent them.
    g.set(
        "GuildControlSaveRank",
        lua.create_function(|lua, name: String| {
            let Some(name) = gated_rank_name(&name).map(str::to_string) else {
                return Ok(());
            };
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let edit = model.guild_control.clone();
            // The reference's `[0x84966c] >= [0xb73120]`: no rank loaded (its -1, our 0) or one
            // past the rank count.
            if edit.rank >= 1 && (edit.rank as usize) <= model.guild.ranks.len() {
                let mut rights = live_rights(&model, edit.rank);
                for (i, bit) in RANK_RIGHT_BITS.iter().enumerate() {
                    if edit.staged[i] {
                        rights |= bit;
                    } else {
                        rights &= !bit;
                    }
                }
                drop(model);
                return submit(
                    lua,
                    GuildCall::SaveRank {
                        rank_index: edit.rank - 1,
                        rights,
                        name,
                    },
                );
            }
            Ok(())
        })?,
    )?;

    // GuildControlAddRank(name) (`0x4d2210`): the same name gate, and a silent refusal at
    // `MAX_RANKS`, both before any packet.
    g.set(
        "GuildControlAddRank",
        lua.create_function(|lua, name: String| {
            let Some(name) = gated_rank_name(&name).map(str::to_string) else {
                return Ok(());
            };
            let ranks = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .guild
                .ranks
                .len();
            if ranks >= MAX_RANKS {
                return Ok(());
            }
            submit(lua, GuildCall::AddRank(name))
        })?,
    )?;

    // GuildControlDelRank() (`0x4d22e0`) reads no argument and sends an empty `0x233`: the server
    // deletes the last rank. The FrameXML passes a name (`FriendsFrame.lua:895`), which is ignored.
    // It refuses silently at `MIN_RANKS`, before any packet.
    g.set(
        "GuildControlDelRank",
        lua.create_function(|lua, _ignored: MultiValue| {
            let ranks = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .guild
                .ranks
                .len();
            if ranks <= MIN_RANKS {
                return Ok(());
            }
            submit(lua, GuildCall::DelRank)
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x4c9330`: only an unrecognised token raises; `"player"` with no unit behind it, the paper
    /// doll's case at world entry, answers `nil, nil, 0`.
    #[test]
    fn a_recognised_but_unresolved_unit_answers_nils_and_only_an_unknown_token_raises() {
        let s = crate::script::UiScript::new().unwrap();
        let (name, rank, idx): (Option<String>, Option<String>, i64) = s
            .eval(r#"return GetGuildInfo("player")"#)
            .expect("a recognised token must not raise");
        assert_eq!((name, rank, idx), (None, None, 0));
        // The third value is the number 0, not nil (`mov eax,3` on both legs).
        assert!(s
            .eval::<bool>(r#"local a,b,c = GetGuildInfo("party3") return c == 0"#)
            .unwrap());
        assert!(s
            .eval::<i64>(r#"return GetGuildInfo("nosuchunit")"#)
            .is_err());
    }

    #[test]
    fn the_rank_right_table_is_not_a_shift() {
        assert_eq!(RANK_RIGHT_BITS.len(), 13, "one bit per GUILDCONTROL_OPTION");
        assert_eq!(RANK_RIGHT_BITS[right::PROMOTE - 1], 0x80);
        assert_eq!(RANK_RIGHT_BITS[right::DEMOTE - 1], 0x100);
        assert_eq!(RANK_RIGHT_BITS[right::INVITE - 1], 0x10);
        assert_eq!(RANK_RIGHT_BITS[right::REMOVE - 1], 0x20);
        assert!(
            RANK_RIGHT_BITS[right::PROMOTE - 1] > RANK_RIGHT_BITS[right::INVITE - 1],
            "promote's bit is above invite's while its index is below — the whole reason this is \
             a table and not a shift"
        );
        for (i, bit) in RANK_RIGHT_BITS.iter().enumerate() {
            assert_eq!(bit.count_ones(), 1, "flag {} is not a single bit", i + 1);
        }
        let mut seen = 0u32;
        for bit in RANK_RIGHT_BITS {
            assert_eq!(seen & bit, 0, "duplicate bit {bit:#x}");
            seen |= bit;
        }
    }

    /// `0x40`, vmangos's `GR_RIGHT_EMPTY`, is none of the thirteen and never tested by the
    /// reference.
    #[test]
    fn saving_a_rank_preserves_bits_outside_the_thirteen_and_can_clear() {
        // Live: Invite (0x10) + Set MOTD (0x1000), plus the untouchable 0x40.
        let live = 0x0000_0040 | RANK_RIGHT_BITS[right::INVITE - 1] | RANK_RIGHT_BITS[8];
        // Staged: the user unticked Set MOTD and ticked Promote.
        let mut staged = [false; 13];
        staged[right::INVITE - 1] = true;
        staged[right::PROMOTE - 1] = true;

        let mut folded = live;
        for (i, bit) in RANK_RIGHT_BITS.iter().enumerate() {
            if staged[i] {
                folded |= bit;
            } else {
                folded &= !bit;
            }
        }

        assert_eq!(
            folded & 0x0000_0040,
            0x0000_0040,
            "0x40 is outside the thirteen and must ride through untouched"
        );
        assert_ne!(folded & RANK_RIGHT_BITS[right::PROMOTE - 1], 0, "ticked");
        assert_eq!(
            folded & RANK_RIGHT_BITS[8],
            0,
            "unticked Set MOTD must actually clear — a plain OR could never do this"
        );
    }

    fn guild_with_ranks(n: usize) -> crate::script::UiScript {
        let mut s = crate::script::UiScript::new().unwrap();
        s.keep_calls();
        s.set_guild(GuildState {
            in_guild: true,
            ranks: (0..n)
                .map(|i| GuildRankInfo {
                    name: format!("Rank{i}"),
                    rights: 0,
                })
                .collect(),
            ..Default::default()
        });
        s
    }

    /// `0x4d20d0` refuses silently: a name outside 1 to 16 UTF-16 units (`0x65b250` counts them
    /// plus the NUL), no rank loaded, or a rank past the count.
    #[test]
    fn saving_a_rank_refuses_what_the_reference_refuses() {
        let mut s = guild_with_ranks(5);
        let saves = |s: &mut crate::script::UiScript| {
            s.take_guild_calls()
                .into_iter()
                .filter(|r| matches!(r, GuildCall::SaveRank { .. }))
                .count()
        };

        s.run(r#"GuildControlSetRank(2); GuildControlSaveRank("")"#)
            .unwrap();
        assert_eq!(saves(&mut s), 0, "an empty name is a silent no-op");

        // Sixteen UTF-16 units is the ceiling; seventeen is refused.
        s.run(r#"GuildControlSetRank(2); GuildControlSaveRank(string.rep("a", 16))"#)
            .unwrap();
        assert_eq!(saves(&mut s), 1);
        s.run(r#"GuildControlSetRank(2); GuildControlSaveRank(string.rep("a", 17))"#)
            .unwrap();
        assert_eq!(saves(&mut s), 0, "seventeen units");
        // Units, not bytes: sixteen two-byte letters are sixteen units.
        s.run(r#"GuildControlSetRank(2); GuildControlSaveRank(string.rep("é", 16))"#)
            .unwrap();
        assert_eq!(saves(&mut s), 1, "32 bytes, 16 units");
        // Whitespace is not empty to the reference.
        s.run(r#"GuildControlSetRank(2); GuildControlSaveRank(" ")"#)
            .unwrap();
        assert_eq!(saves(&mut s), 1);

        // No rank loaded in a fresh popup (the selection persists across saves, as `[0x84966c]`
        // does), or one past the count.
        let mut fresh = guild_with_ranks(5);
        fresh.run(r#"GuildControlSaveRank("Officer")"#).unwrap();
        assert_eq!(saves(&mut fresh), 0, "nothing selected");
        s.run(r#"GuildControlSetRank(6); GuildControlSaveRank("Officer")"#)
            .unwrap();
        assert_eq!(saves(&mut s), 0, "rank index 5 >= 5 ranks");
    }

    /// `0x4d2210` runs the same name gate; a whitespace name passes it.
    #[test]
    fn adding_a_rank_runs_the_same_name_gate() {
        let mut s = guild_with_ranks(5);
        s.run(r#"GuildControlAddRank(""); GuildControlAddRank(string.rep("a", 17))"#)
            .unwrap();
        assert!(s.take_guild_calls().is_empty());
        s.run(r#"GuildControlAddRank(" ")"#).unwrap();
        assert_eq!(s.take_guild_calls().len(), 1);
    }

    fn roster_of(guids: &[u64]) -> GuildState {
        GuildState {
            in_guild: true,
            roster: guids
                .iter()
                .map(|&guid| GuildMemberInfo {
                    guid,
                    name: format!("M{guid}"),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    /// `SetGuildRosterSelection` stores the row's guid, 0 out of range (`0xb73128`, `0x4d186a`);
    /// `GetGuildRosterSelection` searches for it at the call (`0x4d1030`), so a re-sort keeps the
    /// player and one who left reads 0. Neither is a call: the binding is the whole verb.
    #[test]
    fn the_selection_is_the_members_guid() {
        let mut s = crate::script::UiScript::new().unwrap();
        s.set_guild(roster_of(&[11, 22]));
        let selection = |s: &crate::script::UiScript| {
            s.eval::<i64>("return GetGuildRosterSelection()").unwrap()
        };
        s.run("SetGuildRosterSelection(2)").unwrap();
        assert_eq!(selection(&s), 2);
        s.set_guild(roster_of(&[22, 11]));
        assert_eq!(selection(&s), 1, "the same player, a new row");
        s.set_guild(roster_of(&[11]));
        assert_eq!(selection(&s), 0, "no longer listed");
        s.set_guild(roster_of(&[22, 11]));
        assert_eq!(selection(&s), 1, "the guid stays stored");
        s.run("SetGuildRosterSelection(9)").unwrap();
        assert_eq!(selection(&s), 0, "past the end selects nothing");
        s.run("SetGuildRosterSelection(1) SetGuildRosterSelection(0)")
            .unwrap();
        assert_eq!(selection(&s), 0);
    }

    /// `0x4d0f70` stores a changed flag, and only with a member held (`[0xb73118]`, `0x4d0f85`)
    /// sorts and signals: only then is there a call for the host.
    #[test]
    fn show_offline_calls_the_host_only_for_a_change_with_members() {
        let mut s = crate::script::UiScript::new().unwrap();
        s.keep_calls();
        s.run("SetGuildRosterShowOffline(1)").unwrap();
        assert_eq!(s.guild_show_offline(), 1);
        assert!(s.take_guild_calls().is_empty(), "no member held");
        s.set_guild(roster_of(&[11]));
        s.run("SetGuildRosterShowOffline(1) SetGuildRosterShowOffline(2)")
            .unwrap();
        assert_eq!(s.guild_show_offline(), 2);
        assert_eq!(s.take_guild_calls(), [GuildCall::SetShowOffline]);
    }

    /// `GuildStatus_Update` passes 0 before it tests the selection.
    #[test]
    fn member_lookup_is_one_based_and_tolerates_zero() {
        let roster = vec![
            GuildMemberInfo {
                name: "Alice".into(),
                ..Default::default()
            },
            GuildMemberInfo {
                name: "Bob".into(),
                ..Default::default()
            },
        ];
        assert_eq!(
            member_at(&roster, 1).map(|m| m.name.as_str()),
            Some("Alice")
        );
        assert_eq!(member_at(&roster, 2).map(|m| m.name.as_str()), Some("Bob"));
        assert!(member_at(&roster, 0).is_none(), "0 = nothing selected");
        assert!(member_at(&roster, 3).is_none(), "past the end");
        assert!(member_at(&roster, -1).is_none(), "negative");
    }
}
