//! The guild bindings: roster, ranks, notes, MOTD and the membership verbs. The app pushes a
//! display-ready [`GuildState`] ([`UiScript::set_guild`]) that joins two caches: the roster (MOTD,
//! info text, rank rights, members) and the guild query (the name and ten rank names). Each verb
//! queues a [`GuildRequest`] the app drains ([`UiScript::take_guild_requests`]).
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
    /// The tenth return: `""`, `"<AFK>"` or `"<DND>"`. Only the status view reads it, to choose
    /// between the Online label and the tag (`FriendsFrame.lua:541`, `:548-551`).
    pub status: String,
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
    /// `GetNumGuildMembers()`, the UI's loop bound, which respects [`Self::show_offline`]. The
    /// index-taking bindings bound against the full count (`[0xb73118]`) and never read the flag,
    /// so an index past this still returns a real offline member.
    pub num_members: usize,
    /// Every rank, 0 the guild master; the length is `GuildControlGetNumRanks`.
    pub ranks: Vec<GuildRankInfo>,
    /// The selected row, 1-based into [`Self::roster`], 0 for none (`FriendsFrame.lua:347` tests
    /// `> 0`).
    pub selection: u32,
    /// Changes [`Self::num_members`] only, never [`Self::roster`].
    pub show_offline: bool,
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

/// Outbound guild intents, drained by the app ([`UiScript::take_guild_requests`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuildRequest {
    /// `GuildRoster()` (`CMSG_GUILD_ROSTER`). The app throttles it to one per 10 s as the reference
    /// does; the `GUILD_ROSTER_UPDATE` arg1 path clears the throttle first, so the FrameXML's own
    /// request is never the one dropped.
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
    /// `GuildRosterSetPublicNote(index, note)`: a 1-based display row the app resolves to the name
    /// the wire takes.
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
    /// `SetGuildRosterSelection(index)`, mirrored so the next push agrees.
    Select(u32),
    /// `SetGuildRosterShowOffline(flag)`, mirrored so the next push agrees.
    SetShowOffline(bool),
    /// `SortGuildRoster(field)`: `"name"`, `"zone"`, `"level"`, `"class"`, `"rank"`, `"note"` or
    /// `"online"`, the headers' `sortType`s (`FriendsFrame.xml:1895-1940`, `:2086-2131`). Sorting
    /// is client-side: the app re-orders its roster and pushes it back.
    Sort(String),
}

impl super::UiScript {
    /// Push the guild snapshot, replacing it. The app fires `GUILD_ROSTER_UPDATE`,
    /// `PLAYER_GUILD_UPDATE` and `GUILD_MOTD` on the edges.
    pub fn set_guild(&mut self, state: GuildState) {
        self.model_mut().guild = state;
    }

    /// Drain the guild intents queued since the last call.
    pub fn take_guild_requests(&mut self) -> Vec<GuildRequest> {
        std::mem::take(&mut self.model_mut().guild_requests)
    }

    /// Queue an intent from a slash command (`/ginvite`, `/gquit`, `/gpromote`, ...). The
    /// reference's slash commands are Lua calling the same bindings; benilla parses them in Rust.
    pub fn queue_guild_request(&mut self, request: GuildRequest) {
        self.model_mut().guild_requests.push(request);
    }
}

/// A roster row by 1-based index.
fn member_at(roster: &[GuildMemberInfo], index: i64) -> Option<&GuildMemberInfo> {
    usize::try_from(index)
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| roster.get(i))
}

/// A flag argument: `nil`, `false` and `0` are off, anything else on. The reference reads both
/// flags with `GetBoolOrDefault` (`0x6f1c10`, `binding_abi::bool_or_default`), which differs on
/// strings and fractions; the FrameXML's checkbox states, `1` or `nil`, read the same in both.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Nil | Value::Boolean(false) => false,
        Value::Integer(0) => false,
        Value::Number(n) => *n != 0.0,
        _ => true,
    }
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
    // `PaperDollFrame_SetGuild` calls it for `"player"` unguarded (`PaperDollFrame.lua:117`).
    g.set(
        "GetGuildInfo",
        lua.create_function(|lua, unit: Option<String>| {
            crate::script::unit::check_unit_token(&unit)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(state) = unit.as_deref().and_then(|u| model.unit(u)) else {
                return Ok((Value::Nil, Value::Nil, Value::Integer(0)));
            };
            let Some(guild) = state.guild.as_ref() else {
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
    // GetNumGuildMembers() (`0x4d1190`): the loop bound (`FriendsFrame.lua:446`), which respects
    // show-offline, not the addressable range (`GuildState::num_members`).
    g.set(
        "GetNumGuildMembers",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
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
        lua.create_function(|lua, index: i64| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(m) = member_at(&model.guild.roster, index) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil; 10]));
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
                Value::String(lua.create_string(&m.status)?),
            ]))
        })?,
    )?;

    // GetGuildRosterLastOnline(index) → years, months, days, hours (`0x4d14a0`), all 0 for an
    // online member, whom the wire sends no time for.
    g.set(
        "GetGuildRosterLastOnline",
        lua.create_function(|lua, index: i64| {
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

    // GetGuildRosterSelection() (`0x4d1890`) / SetGuildRosterSelection(index) (`0x4d1820`):
    // 1-based, 0 for none. The setter writes the snapshot and queues the intent: the same Lua pass
    // reads it back, and the app's state must follow so the next push agrees.
    g.set(
        "GetGuildRosterSelection",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.guild.selection))
        })?,
    )?;
    g.set(
        "SetGuildRosterSelection",
        lua.create_function(|lua, index: i64| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let len = model.guild.roster.len();
            let index = u32::try_from(index.clamp(0, len as i64)).unwrap_or(0);
            model.guild.selection = index;
            model.guild_requests.push(GuildRequest::Select(index));
            Ok(())
        })?,
    )?;

    // GetGuildRosterShowOffline() (`0x4d1e30`) / SetGuildRosterShowOffline(flag): written in
    // place and mirrored, as the selection is.
    g.set(
        "GetGuildRosterShowOffline",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(model.guild.show_offline))
        })?,
    )?;
    // With no argument it turns show-offline on, unlike an explicit `nil`, hence `MultiValue`.
    g.set(
        "SetGuildRosterShowOffline",
        lua.create_function(|lua, args: MultiValue| {
            let on = match args.into_iter().next() {
                None => true,
                Some(flag) => truthy(&flag),
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.guild.show_offline = on;
            model.guild_requests.push(GuildRequest::SetShowOffline(on));
            Ok(())
        })?,
    )?;

    // SortGuildRoster(field) (`0x4d1cb0`): the header's OnClick calls no `GuildStatus_Update` and
    // no `GuildRoster` (`FriendsFrame.xml:442-446`), so the app re-orders and fires
    // `GUILD_ROSTER_UPDATE` on the drain, without asking the server.
    g.set(
        "SortGuildRoster",
        lua.create_function(|lua, field: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.guild_requests.push(GuildRequest::Sort(field));
            Ok(())
        })?,
    )?;

    // CloseGuildRoster() is `xor eax,eax; ret` in the reference: a no-op that sends nothing.
    g.set("CloseGuildRoster", lua.create_function(|_, ()| Ok(()))?)?;

    // The argument-less verbs; the app throttles `GuildRoster()` (`GuildRequest::Roster`).
    for (global, request) in [
        ("GuildRoster", GuildRequest::Roster),
        ("GuildInfo", GuildRequest::Info),
        ("AcceptGuild", GuildRequest::Accept),
        ("DeclineGuild", GuildRequest::Decline),
        ("GuildLeave", GuildRequest::Leave),
        ("GuildDisband", GuildRequest::Disband),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, ()| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.guild_requests.push(request.clone());
                Ok(())
            })?,
        )?;
    }

    // ── The by-name verbs ────────────────────────────────────────────────────────────────────
    // Five name-taking verbs (`0x48aef0`, `0x48afb0`, `0x48b050`, `0x48b0f0`, `0x48b190`). An empty
    // or whitespace name is not sent. vmangos ignores an empty one without a reply, except an
    // invite, which it answers with `ERR_GUILD_PLAYER_NOT_FOUND_S`
    // (`Handlers/GuildHandler.cpp:77-83`).
    for (global, make) in [
        ("GuildInviteByName", GuildRequest::Invite as fn(String) -> _),
        (
            "GuildUninviteByName",
            GuildRequest::Uninvite as fn(String) -> _,
        ),
        (
            "GuildPromoteByName",
            GuildRequest::Promote as fn(String) -> _,
        ),
        ("GuildDemoteByName", GuildRequest::Demote as fn(String) -> _),
        (
            "GuildSetLeaderByName",
            GuildRequest::SetLeader as fn(String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, name: String| {
                if !name.trim().is_empty() {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    model.guild_requests.push(make(name));
                }
                Ok(())
            })?,
        )?;
    }

    // GuildSetMOTD(text) (`0x48b270`) / SetGuildInfoText(text) (`0x4d2380`): an empty string is
    // sent, and clears the text.
    for (global, make) in [
        ("GuildSetMOTD", GuildRequest::SetMotd as fn(String) -> _),
        (
            "SetGuildInfoText",
            GuildRequest::SetInfoText as fn(String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, text: String| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.guild_requests.push(make(text));
                Ok(())
            })?,
        )?;
    }

    // GuildRosterSetPublicNote(index, note) (`0x4d15e0`) / GuildRosterSetOfficerNote (`0x4d1700`)
    // take a roster row, which the app resolves to the name the wire carries.
    for (global, make) in [
        (
            "GuildRosterSetPublicNote",
            (|index, note| GuildRequest::SetPublicNote { index, note }) as fn(u32, String) -> _,
        ),
        (
            "GuildRosterSetOfficerNote",
            (|index, note| GuildRequest::SetOfficerNote { index, note }) as fn(u32, String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, (index, note): (i64, String)| {
                let Ok(index) = u32::try_from(index) else {
                    return Ok(());
                };
                if index >= 1 {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    model.guild_requests.push(make(index, note));
                }
                Ok(())
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
        lua.create_function(|lua, index: i64| {
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
        lua.create_function(|lua, index: i64| {
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
        lua.create_function(|lua, (index, enabled): (i64, Value)| {
            let Some(slot) = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .filter(|i| *i < RANK_RIGHT_BITS.len())
            else {
                return Ok(());
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.guild_control.staged[slot] = truthy(&enabled);
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
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
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
                model.guild_requests.push(GuildRequest::SaveRank {
                    rank_index: edit.rank - 1,
                    rights,
                    name,
                });
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
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.guild.ranks.len() < MAX_RANKS {
                model.guild_requests.push(GuildRequest::AddRank(name));
            }
            Ok(())
        })?,
    )?;

    // GuildControlDelRank() (`0x4d22e0`) reads no argument and sends an empty `0x233`: the server
    // deletes the last rank. The FrameXML passes a name (`FriendsFrame.lua:895`), which is ignored.
    // It refuses silently at `MIN_RANKS`, before any packet.
    g.set(
        "GuildControlDelRank",
        lua.create_function(|lua, _ignored: MultiValue| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.guild.ranks.len() > MIN_RANKS {
                model.guild_requests.push(GuildRequest::DelRank);
            }
            Ok(())
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
            s.take_guild_requests()
                .into_iter()
                .filter(|r| matches!(r, GuildRequest::SaveRank { .. }))
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
        assert!(s.take_guild_requests().is_empty());
        s.run(r#"GuildControlAddRank(" ")"#).unwrap();
        assert_eq!(s.take_guild_requests().len(), 1);
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
