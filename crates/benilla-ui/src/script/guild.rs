//! The guild **Era API surface** — roster, ranks, notes, MOTD, and the membership verbs.
//!
//! [`super::social`]'s shape exactly: the app pushes a [`GuildState`] snapshot built from its own
//! wire mirror ([`UiScript::set_guild`]) and the getters here read that plain data; every verb
//! queues a [`GuildRequest`] the app drains ([`UiScript::take_guild_requests`]) into the matching
//! `CMSG_*` send. No ECS or net reach from the engine.
//!
//! **The snapshot is already display-ready** — member names, localized class names, zone names,
//! rank names, and the last-online decomposition — because every one of those is a *lookup the
//! engine owns* in the real client too. Unlike the friend list and `/who`, the guild roster
//! carries level/class/zone for **offline** members as well (the server keeps them in the guild
//! table), which is why those columns stay populated and merely grey out.
//!
//! **Guild identity and guild roster are two different caches, and this file joins them.**
//! `SMSG_GUILD_ROSTER` carries the MOTD, the info text, the rank *rights* and the members — but
//! **not** the guild's name and **not** the rank *names*. Those come only from
//! `SMSG_GUILD_QUERY_RESPONSE`, which answers a `CMSG_GUILD_QUERY(guildId)` with the name plus a
//! fixed array of ten rank names. The app does the joining before it pushes; [`GuildState`] is
//! the joined result.
//!
//! **Two index bases coexist, and callers must convert.** A roster row's `rankIndex` (from
//! `GetGuildRosterInfo`) is **0-based**, `0` = guild master — the reference computes
//! `maxRankIndex = GuildControlGetNumRanks() - 1` and compares roster indices against it
//! (`FriendsFrame.lua:338`, `:393-398`). The whole `GuildControl*` family is **1-based**, driven
//! straight off dropdown IDs `1..GuildControlGetNumRanks()` (`:820-822`, `:855-857`, `:866-868`).
//! Mixing them silently promotes the wrong rank, so every conversion here is spelled out.
//!
//! **Rank-control edits are staged, and the staging is write-only.** `GuildControlSetRank(i)`
//! loads rank `i` into [`GuildRankEdit`], `GuildControlSetRankFlag(idx, on)` writes one staged
//! slot, and `GuildControlSaveRank(name)` folds the thirteen slots over a freshly re-read *live*
//! baseline and queues one intent. The buffer is deliberately *not* part of the pushed snapshot:
//! it is scratch the popup owns between open and Accept, and a snapshot push mid-edit would
//! otherwise discard the user's unsaved checkbox clicks.
//!
//! The counter-intuitive half, and it is verified: **`GuildControlGetRankFlags` reads the LIVE
//! rights, never the staging buffer.** A checkbox click is invisible through the getter until the
//! save round-trips and a fresh roster lands. Reading the buffer back would feel more responsive
//! and would be a different client.
//!
//! **The roster is never filtered.** `GetGuildRosterInfo`, `GetGuildRosterLastOnline` and
//! `SetGuildRosterSelection` all address the *whole* member list; only `GetNumGuildMembers`
//! respects the show-offline toggle, and indices past it are valid and return real offline
//! members. See [`GuildState::num_members`] — this is the opposite of the obvious design, and
//! collapsing the two is the mistake it exists to prevent.
//!
//! **Era booleans are `1`/`nil`, never `true`/`false`** — `IsInGuild`, `IsGuildLeader` and every
//! `Can*` predicate below. The FrameXML branches on truthiness and stores the result, so a
//! `false` where the reference gives `nil` survives into places a `nil` would not.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The thirteen guild rank permissions, **in `GuildControlGetRankFlags`' return order** — which is
/// also `GuildControlPopupFrameCheckbox1..13`'s order and the index `GuildControlSetRankFlag`
/// takes. That ordering *is* the API contract: `GuildControlCheckboxUpdate` loops
/// `for i=1, arg.n` over the returns and drives `getglobal("GuildControlPopupFrameCheckbox"..i)`
/// (`FriendsFrame.lua:874-884`), so nothing but position connects a flag to its checkbox.
///
/// The labels, in this order, are `GUILDCONTROL_OPTION1..13`
/// (`GlobalStrings.lua:2036-2048`): Guildchat Listen · Guildchat Speak · Officerchat Listen ·
/// Officerchat Speak · Promote · Demote · Invite Member · Remove Member · Set MOTD ·
/// Edit Public Note · View Officer Note · Edit Officer Note · Modify Guild Info.
///
/// **The map is not monotonic, and that is the whole reason it is a table rather than a shift.**
/// Promote (index 5) is `0x80` and Demote (6) is `0x100`, both *above* Invite (7) `0x10` and
/// Remove (8) `0x20` in bit value while *below* them in checkbox order. `1 << (i - 1)` is wrong
/// for nine of the thirteen. Bits `0x40`, `0x400` and `0x800` are unused.
///
/// Bits from vmangos `src/game/Guild/Guild.h:56-74` (`GuildRankRights`), with that enum's
/// `GR_RIGHT_EMPTY` (`0x40`) stripped — it is OR'd into every one of its entries, so it carries no
/// per-permission information.
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

/// Named indices into [`RANK_RIGHT_BITS`], for the `Can*` predicates. One-based, matching the
/// checkbox numbering the reference's strings use, so a reader can check them against
/// `GUILDCONTROL_OPTION<n>` without arithmetic.
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

/// The lowest and highest rank counts a guild can have — the reference's own guard rails, which
/// it enforces client-side before the server ever sees the request:
/// `GuildControlPopupFrameAddRankButton_OnUpdate` disables Add at `>= 10`
/// (`FriendsFrame.lua:887`) and `..RemoveRankButton_OnUpdate` only shows Remove when the selected
/// rank is the last *and* the count exceeds 5 (`:908`). Same numbers as vmangos'
/// `GUILD_RANKS_MIN_COUNT` / `GUILD_RANKS_MAX_COUNT` (`Guild.h:31-32`).
pub const MIN_RANKS: usize = 5;
/// See [`MIN_RANKS`].
pub const MAX_RANKS: usize = 10;

/// How long ago a member was last seen, already decomposed into the four units
/// `GetGuildRosterLastOnline` returns. The wire carries one `f32` — days since last login, sent
/// only for *offline* members — and the decomposition is engine math, so the app does it once and
/// the VM just reports it.
///
/// The reference reads these largest-unit-first and stops at the first non-zero
/// (`GuildFrame_GetLastOnline`, `FriendsFrame.lua:957-978`), so the contract that matters is that
/// exactly the largest applicable unit is non-zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LastOnline {
    pub years: u32,
    pub months: u32,
    pub days: u32,
    pub hours: u32,
}

/// A unit's guild membership, as `GetGuildInfo(unit)` reports it — hung off
/// [`super::unit::UnitState`] because the binding is per-unit and the descriptor is public.
///
/// `PLAYER_GUILDID` (field 191) and `PLAYER_GUILDRANK` (field 192) are PUBLIC, so these arrive
/// for *every visible player*, not just us (vmangos `Objects/UpdateFields_1_12_1.h:121-122`;
/// UNIT_END = 188, the same arithmetic that pins `PLAYER_FLAGS` = 190). The app turns the id into
/// a name through its guild-identity cache and the rank field into a rank name through that
/// identity's ten rank names — which is exactly why the query response carries a fixed ten.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitGuild {
    /// The guild's name — **never empty**.
    ///
    /// A cache miss does not produce a blank-named `UnitGuild`: it produces `None`. The binding
    /// takes the *same* nil path on the cache-miss leg (`0x4c93d7 je 0x4c943c`) as on the
    /// guildless leg, so "we have not resolved this id yet" and "this unit has no guild" are
    /// indistinguishable to Lua by design. That matters — `UnitPopup.lua:575` passes the result
    /// straight into a StaticPopup format, and an empty name would render `<>` there.
    pub name: String,
    /// The unit's rank name within that guild.
    pub rank_name: String,
    /// The unit's rank, **0-based**, `0` = guild master.
    pub rank_index: u32,
}

/// One guild rank: its name (from the query response) and its rights word (from the roster).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildRankInfo {
    /// The rank's display name, e.g. "Guild Master". Empty for a rank the query response has not
    /// named yet.
    pub name: String,
    /// The rank's permission bits — test with [`RANK_RIGHT_BITS`].
    pub rights: u32,
}

/// One roster row, already resolved for display. Mirrors `GetGuildRosterInfo`'s nine returns in
/// field order, which is how `FriendsFrame.lua` destructures them (`:344`, `:447`, `:479`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildMemberInfo {
    /// The character's name. Carried on the wire in the roster itself, so unlike a friend row
    /// this never waits on a name query.
    pub name: String,
    /// The member's rank *name*, e.g. "Officer" — `GetGuildRosterInfo`'s second return.
    pub rank: String,
    /// The member's rank as a **0-based** index, `0` = guild master. See the module doc: this is
    /// the opposite base from the `GuildControl*` family.
    pub rank_index: u32,
    pub level: u32,
    /// Localized class name ("Warrior"). Populated for offline members too.
    pub class: String,
    /// Zone name ("Elwynn Forest"), empty when the id has no `AreaTable` row.
    pub zone: String,
    /// The public note.
    pub note: String,
    /// The officer note. The server sends this empty when the *viewer* lacks
    /// `View Officer Note`, so an empty string here is "not permitted" and "not set" alike —
    /// which is exactly what the reference can distinguish too (it gates on `CanViewOfficerNote`
    /// separately rather than on emptiness).
    pub officer_note: String,
    pub online: bool,
    /// The away tag, exactly as the friends list spells it: `""`, `"<AFK>"` or `"<DND>"` —
    /// `GetGuildRosterInfo`'s **tenth** return.
    ///
    /// It is easy to miss that there is a tenth. Six of the reference's seven call sites
    /// destructure only nine (`FriendsFrame.lua:344`, `:447`, `:479`, `:709`,
    /// `StaticPopup.lua:1008`, `:1045`) — but the player-status view takes ten
    /// (`FriendsFrame.lua:541`) and then branches on `status == ""` to choose between the
    /// "Online" label and the tag itself (`:548-551`). An engine that answered nil there would
    /// blank that column silently, and every nine-value call site would keep working.
    ///
    /// This is also what the wire's presence byte is *for*: `GRF_ONLINE`/`GRF_AFK`/`GRF_DND`
    /// (0x01/0x02/0x04) would not need three bits if the API only exposed a boolean.
    pub status: String,
    /// How long since this member was last seen — meaningful only when `online` is false. The
    /// wire omits it entirely for online members.
    pub last_online: LastOnline,
}

/// The guild snapshot, pushed whole by the app whenever it changes ([`UiScript::set_guild`]).
/// `default()` is the guildless shape, which is also the fresh-login shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildState {
    /// Whether the player is in a guild at all — `IsInGuild`. Driven by the player's own
    /// `PLAYER_GUILDID` descriptor field (191), so it is true from the moment the descriptor
    /// streams, *before* any roster has arrived.
    pub in_guild: bool,
    /// The guild's name. Only `SMSG_GUILD_QUERY_RESPONSE` carries it, so it stays empty in the
    /// window between `in_guild` going true and that query answering.
    pub name: String,
    /// The player's own rank name.
    pub rank_name: String,
    /// The player's own rank, **0-based**, `0` = guild master.
    pub rank_index: u32,
    /// Whether the player is the guild master — `IsGuildLeader`.
    pub is_leader: bool,
    /// The player's own rank's rights word, which every `Can*` predicate tests.
    pub rights: u32,
    /// The message of the day.
    pub motd: String,
    /// The guild info text (the `GuildInfoFrame` body).
    pub info_text: String,
    /// The **whole** roster, sorted — never filtered. `GetGuildRosterInfo` indexes this directly,
    /// 1-based, and so do `GetGuildRosterLastOnline` and `SetGuildRosterSelection`.
    ///
    /// **It is NOT filtered by [`Self::show_offline`], and its length is NOT
    /// `GetNumGuildMembers()`.** See [`Self::num_members`] — this asymmetry is verified and it is
    /// the opposite of what the obvious design would do.
    pub roster: Vec<GuildMemberInfo>,
    /// What `GetNumGuildMembers()` reports: the count the UI loops to, which **does** respect
    /// [`Self::show_offline`].
    ///
    /// **This is an advisory loop bound, not the addressable range.** In the real client the three
    /// index-taking bindings all bound against the *full* member count (`[0xb73118]`) and never
    /// read the show-offline flag at all, while `GetNumGuildMembers` answers a different number
    /// entirely — so **indices past it are valid and return real offline members**.
    ///
    /// Collapsing the two — filtering the vector and reporting its length — is the natural design
    /// and it is wrong: it would make a selection that survives a show-offline toggle address a
    /// different member, or none.
    pub num_members: usize,
    /// Every rank, index 0 = guild master. Length is the guild's real rank count, which is what
    /// `GuildControlGetNumRanks` reports.
    pub ranks: Vec<GuildRankInfo>,
    /// The selected roster row as a **1-based** index into [`Self::roster`], `0` = none — the
    /// scale `GetGuildRosterSelection` reports and `FriendsFrame.lua:347` tests with `> 0`.
    pub selection: u32,
    /// Whether the UI is showing offline members. Affects [`Self::num_members`] only — never
    /// [`Self::roster`]'s contents.
    pub show_offline: bool,
}

/// The rank-control popup's staging buffer — the rank it has loaded and the rights word it has
/// been editing. See the module doc on why this is not part of the pushed snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildRankEdit {
    /// The **1-based** rank `GuildControlSetRank` last loaded; `0` = none loaded yet.
    pub rank: u32,
    /// The thirteen staged checkbox states, positionally matching [`RANK_RIGHT_BITS`].
    ///
    /// Held per-flag rather than as a packed word because that is what the binding does — it
    /// stages `mask[k] & rights` into thirteen separate slots and `SetRankFlag` writes exactly
    /// one of them. Keeping a word here instead would work only as long as nothing outside the
    /// thirteen bits mattered, and `0x40` is precisely such a bit.
    pub staged: [bool; 13],
}

/// Outbound guild intents queued by the Era API, drained by the app
/// ([`UiScript::take_guild_requests`]). Plain data — [`super::social::SocialRequest`]'s twin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuildRequest {
    /// `GuildRoster()` — ask the server for a fresh roster (`CMSG_GUILD_ROSTER`). Throttled
    /// app-side to one per 10 s, as the reference is; the `GUILD_ROSTER_UPDATE` arg1 path clears
    /// that throttle first so the FrameXML's response request is never the one swallowed.
    Roster,
    /// `GuildInfo()` — the `/ginfo` query (`CMSG_GUILD_INFO`), whose answer is a chat line.
    Info,
    /// `GuildInviteByName(name)`.
    Invite(String),
    /// `GuildUninviteByName(name)` — remove a member (`CMSG_GUILD_REMOVE`).
    Uninvite(String),
    /// `GuildPromoteByName(name)`.
    Promote(String),
    /// `GuildDemoteByName(name)`.
    Demote(String),
    /// `GuildSetLeaderByName(name)` — hand over the guild.
    SetLeader(String),
    /// `AcceptGuild()` — the `GUILD_INVITE` popup's Accept.
    Accept,
    /// `DeclineGuild()` — its Decline.
    Decline,
    /// `GuildLeave()`.
    Leave,
    /// `GuildDisband()`.
    Disband,
    /// `GuildSetMOTD(text)`. An empty string is a real value meaning "clear it".
    SetMotd(String),
    /// `SetGuildInfoText(text)`.
    SetInfoText(String),
    /// `GuildRosterSetPublicNote(index, note)` — the index is the 1-based display row, which the
    /// app resolves to the name the wire wants.
    SetPublicNote { index: u32, note: String },
    /// `GuildRosterSetOfficerNote(index, note)`.
    SetOfficerNote { index: u32, note: String },
    /// `GuildControlSaveRank(name)` — the staged buffer flushed as one `CMSG_GUILD_RANK`. The
    /// rank is carried **0-based**, converted here once, because that is the base the wire uses.
    SaveRank {
        rank_index: u32,
        rights: u32,
        name: String,
    },
    /// `GuildControlAddRank(name)`.
    AddRank(String),
    /// `GuildControlDelRank()` — delete the last rank. **Carries nothing**: the binding reads no
    /// argument and the packet (`0x233`) is body-less; the server infers which rank.
    DelRank,
    /// `SetGuildRosterSelection(index)` — mirrored into the app so the next push agrees.
    Select(u32),
    /// `SetGuildRosterShowOffline(flag)` — the app re-filters and pushes back.
    SetShowOffline(bool),
    /// `SortGuildRoster(field)` — one of `"name"`, `"zone"`, `"level"`, `"class"`, `"rank"`,
    /// `"note"`, `"online"` (the seven the reference's two column sets set as `this.sortType`,
    /// `FriendsFrame.xml:1895-1940` and `:2086-2131`). Sorting is client-side; the app re-orders
    /// its own roster and pushes it back.
    Sort(String),
}

impl super::UiScript {
    /// Push the guild snapshot, replacing whatever was there. A bare setter — firing
    /// `GUILD_ROSTER_UPDATE`/`PLAYER_GUILD_UPDATE`/`GUILD_MOTD` on the edges is the app's job.
    pub fn set_guild(&mut self, state: GuildState) {
        self.model_mut().guild = state;
    }

    /// Drain the guild intents queued since the last call.
    pub fn take_guild_requests(&mut self) -> Vec<GuildRequest> {
        std::mem::take(&mut self.model_mut().guild_requests)
    }

    /// Queue an intent from the app side — the slash commands (`/ginvite`, `/gquit`, `/gpromote`,
    /// …). In the reference these ARE Lua (`SlashCmdList["GUILDINVITE"]` calls
    /// `GuildInviteByName`); benilla parses slash lines in Rust, so the same intents enter the
    /// same queue here. [`super::social::SocialRequest`]'s `queue_social_request` twin.
    pub fn queue_guild_request(&mut self, request: GuildRequest) {
        self.model_mut().guild_requests.push(request);
    }
}

/// A roster row by 1-based display index, or `None` past the end.
fn member_at(roster: &[GuildMemberInfo], index: i64) -> Option<&GuildMemberInfo> {
    usize::try_from(index)
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| roster.get(i))
}

/// The Era truthiness coercion for a flag argument — `nil`, `false` and `0` are off, everything
/// else is on. [`super::social`]'s `SetWhoToUI` does the same, and for the same reason: the
/// FrameXML passes a checkbox's `GetChecked()`, which is `1`/`nil`, but addons pass booleans.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Nil | Value::Boolean(false) => false,
        Value::Integer(0) => false,
        Value::Number(n) => *n != 0.0,
        _ => true,
    }
}

/// The Era boolean: `1` for true, `nil` for false. See the module doc — this is not `Boolean`.
fn era_bool(on: bool) -> Value {
    if on {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// Whether the player's own rank holds the permission at `index` (1-based into
/// [`RANK_RIGHT_BITS`]).
fn has_right(model: &Model, index: usize) -> bool {
    model.guild.in_guild && model.guild.rights & RANK_RIGHT_BITS[index - 1] != 0
}

/// The **live** rights word of a 1-based rank, `0` for "no rank loaded" or out of range.
///
/// The rank editor reads live rights, never its own staging buffer — see
/// `GuildControlGetRankFlags`. Factored out because `GuildControlSaveRank` needs the same value as
/// its fold baseline, and the two reading different things is exactly how a save would drop bits.
fn live_rights(model: &Model, rank_one_based: u32) -> u32 {
    (rank_one_based as usize)
        .checked_sub(1)
        .and_then(|i| model.guild.ranks.get(i))
        .map(|r| r.rights)
        .unwrap_or(0)
}

/// The rank-name gate `GuildControlSaveRank` (`0x4d20d0`) and `GuildControlAddRank` (`0x4d2210`)
/// both run before any packet, byte-for-byte the same: the C string (so up to the first NUL) must
/// be non-empty and `0x65b250(name) - 1` — its UTF-16 code-unit count — must lie in `1..=16`.
/// Nothing else: no trim, so a whitespace name passes. The name the gate measured is the name
/// sent. vmangos's own cap is 15 code points and it kicks over it; that server-side limit is the
/// app's to enforce at the send (`ui_guild::feed::capped_rank_name`), not the binding's.
fn gated_rank_name(name: &str) -> Option<&str> {
    let c_name = name.split('\0').next().unwrap_or_default();
    (1..=16)
        .contains(&c_name.encode_utf16().count())
        .then_some(c_name)
}

/// Register the guild globals against the snapshot store.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── Membership and identity ──────────────────────────────────────────────────────────────
    // IsInGuild() (`0x516de0`) — reads the player's own PLAYER_GUILDID, so it is answerable
    // before any roster arrives. `InGuildCheck` gates the whole tab on it.
    g.set(
        "IsInGuild",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(era_bool(model.guild.in_guild))
        })?,
    )?;

    // IsGuildLeader() (`0x516e40`) — takes no argument; it reports whether the PLAYER is the
    // guild master, which is rank 0.
    g.set(
        "IsGuildLeader",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(era_bool(model.guild.is_leader))
        })?,
    )?;

    // GetGuildInfo(unit) → guildName, rankName, rankIndex (`0x4c9330`). Every call site in 1.12's
    // FrameXML passes "player" (`FriendsFrame.lua:114`, `:337`, `PaperDollFrame.lua:117`,
    // `TabardFrame.lua:96`, `UnitPopup.lua:575`), but the binding is per-unit and the wire
    // supports it: PLAYER_GUILDID (191) and PLAYER_GUILDRANK (192) are PUBLIC descriptor fields,
    // and the query response's fixed ten rank names exist precisely so another unit's rank field
    // can be turned into a rank name. So it reads the unit snapshot, not the player's guild.
    //
    // A guildless unit answers `nil, nil, 0` — three values, and the third is the NUMBER zero, not
    // nil. The binding pushes 3 on both legs (`mov eax,3`), so a caller destructuring the third
    // gets `0`. Returning nil there would be a different contract.
    //
    // **RECOGNISED and UNRESOLVABLE are two different things**, and this binding conflated them
    // until the stock character sheet came off the chain. It resolves its argument
    // through the GENERAL unit-token resolver `0x515940`, so an *unrecognised* token raises via the
    // L-less `luaL_error 0x7040e0` at `0x515c1a` — but "a recognised-but-unresolvable token, a
    // non-player, a guildless player and a not-yet-arrived cache record ALL return (nil, nil, 0)"
    // (`0x4c9330`).
    //
    // Ours raised on any token the model held no unit for, which includes `"player"` before the
    // app has pushed the first snapshot. Stock `PaperDollFrame_SetGuild` calls
    // `GetGuildInfo("player")` UNGUARDED out of `PaperDollFrame_OnShow` (`PaperDollFrame.lua:117`,
    // reached from `:569`), so opening the character window in the frames before that push raised
    // where the reference prints an empty guild line. [`check_unit_token`] is the resolver's own
    // three-way split and every other unit binding already goes through it.
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
    // GetNumGuildMembers() (`0x4d1190`) — the loop bound `GuildStatus_Update` iterates to
    // (`FriendsFrame.lua:446`), NOT the addressable range. It respects show-offline;
    // GetGuildRosterInfo does not. See `GuildState::num_members` for why the two differ.
    g.set(
        "GetNumGuildMembers",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.guild.num_members as i64)
        })?,
    )?;

    // GetGuildRosterInfo(index) → name, rank, rankIndex, level, class, zone, note, officernote,
    // online, status (`0x4d1200`). TEN returns, in that order.
    //
    // The tenth is easy to miss and expensive to miss: six of the reference's seven call sites
    // take only nine (`FriendsFrame.lua:344`, `:447`, `:479`, `:709`, `StaticPopup.lua:1008`,
    // `:1045`), and only the player-status view takes `status` (`:541`) to choose between the
    // "Online" label and the `<AFK>`/`<DND>` tag (`:548-551`). Answering nine would leave that one
    // column permanently blank while every other caller looked correct.
    //
    // An out-of-range index answers all-nils rather than erroring: the reference calls it with
    // `GetGuildRosterSelection()`, which is 0 when nothing is selected, on every single
    // `GuildStatus_Update` (`:344`) and only afterwards checks `> 0`.
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
                era_bool(m.online),
                Value::String(lua.create_string(&m.status)?),
            ]))
        })?,
    )?;

    // GetGuildRosterLastOnline(index) → years, months, days, hours (`0x4d14a0`). The reference
    // reads them largest-first and stops at the first non-zero (`GuildFrame_GetLastOnline`,
    // `FriendsFrame.lua:957-978`), and treats nil exactly as 0, so an online member — for whom
    // the wire sends no value at all — correctly falls through to "online now".
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

    // GetGuildRosterMOTD() / GetGuildInfoText() — the two free-text blocks the roster carries.
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

    // GetGuildRosterSelection() (`0x4d1890`) / SetGuildRosterSelection(index) (`0x4d1820`).
    // 1-based, 0 = nothing selected. The setter mutates the snapshot in place *and* queues the
    // intent, for [`super::social`]'s stated reason: the same Lua tick reads the new value back —
    // `GuildStatus_Update` calls `GetGuildRosterSelection()` four times in one pass — and the
    // app's own state has to follow so the next push agrees.
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

    // GetGuildRosterShowOffline() (`0x4d1e30`) / SetGuildRosterShowOffline(flag). Same
    // mutate-and-mirror shape: the checkbox reads its own state back through the getter.
    g.set(
        "GetGuildRosterShowOffline",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(era_bool(model.guild.show_offline))
        })?,
    )?;
    //
    // **Called with NO argument it defaults to TRUE**, not false — verified, and the opposite of
    // what `truthy(nil)` would give. So an absent argument and an explicit `nil` differ here, and
    // the binding must be able to tell them apart: hence `MultiValue` rather than `Value`.
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

    // SortGuildRoster(field) (`0x4d1cb0`) — the column headers' only action: their OnClick is
    // `if (this.sortType) then SortGuildRoster(this.sortType) end` and nothing else
    // (`FriendsFrame.xml:442-446`). No GuildStatus_Update, no GuildRoster — so the engine both
    // re-orders and fires GUILD_ROSTER_UPDATE itself, and plainly must not re-request the roster
    // from the server because a user clicked a header. The app does both on the drain.
    g.set(
        "SortGuildRoster",
        lua.create_function(|lua, field: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.guild_requests.push(GuildRequest::Sort(field));
            Ok(())
        })?,
    )?;

    // CloseGuildRoster() is `xor eax,eax; ret` in the real client — a TOTAL no-op that sends
    // nothing and touches nothing. It exists as a symbol and does not do anything. Wiring it to a
    // packet would invent traffic 1.12 never sends, so it is registered and does nothing here too.
    g.set("CloseGuildRoster", lua.create_function(|_, ()| Ok(()))?)?;

    // GuildRoster() — ask the server for a fresh roster. Note the app throttles this to one
    // request per 10 s, as the reference does, and the `GUILD_ROSTER_UPDATE` arg1 path clears that
    // throttle so the FrameXML's own response request is never the one swallowed.
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
    // Five globals of one shape (`0x48aef0`, `0x48afb0`, `0x48b050`, `0x48b0f0`, `0x48b190`): take
    // a character name, queue the matching CMSG. An empty name is dropped rather than sent — the
    // server answers a blank lookup with nothing at all, so it would look like a hang
    // ([`super::social`]'s `AddFriend` note).
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

    // GuildSetMOTD(text) (`0x48b270`) / SetGuildInfoText(text) (`0x4d2380`). Unlike the by-name
    // verbs, an EMPTY string is a real value here: it clears the block, and the wire has an
    // explicit empty-body case for it. So these do not drop empties.
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

    // GuildRosterSetPublicNote(index, note) (`0x4d15e0`) / ...SetOfficerNote (`0x4d1700`) — the
    // note dialogs address the roster ROW, not a name; the app resolves the row to the name the
    // wire carries, because only it knows the display order the index refers to.
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
    // Nine one-line globals, each testing one bit of the player's own rank's rights word
    // (`0x4d18c0`, `0x4d1930`, `0x4d19a0`, `0x4d1a10`, `0x4d1c40` and the four whose names lack
    // "Guild"). CanEditMOTD/CanEditPublicNote/CanViewOfficerNote/CanEditOfficerNote are called at
    // `FriendsFrame.lua:427`, `:359`, `:370`, `:371`.
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
                Ok(era_bool(has_right(&model, index)))
            })?,
        )?;
    }

    // ── The rank-control popup ───────────────────────────────────────────────────────────────
    // GuildControlGetNumRanks() (`0x4d1e60`) — the guild's real rank count, which is what
    // `maxRankIndex = GuildControlGetNumRanks() - 1` turns into the 0-based bottom rank
    // (`FriendsFrame.lua:338`).
    g.set(
        "GuildControlGetNumRanks",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.guild.ranks.len() as i64)
        })?,
    )?;

    // GuildControlGetRankName(index) (`0x4d1e90`) — ONE-BASED, straight off the dropdown's ID
    // (`FriendsFrame.lua:857`, `:868`, `:895`, `:899`). See the module doc on the two bases.
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

    // GuildControlSetRank(index) (`0x4d1fa0`) — load a rank into the staging buffer. ONE-based;
    // it stores `index - 1` internally and seeds each of the thirteen staged slots from that
    // rank's live rights.
    //
    // Passing 0 computes -1 and **silently clears the staged selection** — verified, and the
    // reason this is `checked_sub` rather than a clamp: a caller that handed a raw 0-based roster
    // `rankIndex` straight in (the standing trap, see the module doc) gets nothing loaded rather
    // than rank 1 quietly loaded in its place.
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

    // GuildControlGetRankFlags() (`0x4d1fe0`) — THIRTEEN returns, one per checkbox, in
    // RANK_RIGHT_BITS order. `GuildControlCheckboxUpdate` loops `for i=1, arg.n` over them
    // (`FriendsFrame.lua:874-884`), so both the count and the order are load-bearing.
    //
    // It is called at `GuildControlPopupFrame_OnLoad` (`:801`) — BEFORE `_OnShow` ever calls
    // `GuildControlSetRank`, so it must answer thirteen well-formed values with nothing loaded.
    // An unloaded buffer reads as all-off, which leaves every checkbox unchecked: the same thing
    // the reference shows on a popup that has not selected a rank yet.
    //
    // **It reads the LIVE rank's rights, NOT the staging buffer** — verified. A checkbox click
    // stages a change that is invisible through this getter until `GuildControlSaveRank`
    // round-trips through the server and a fresh roster lands. Reading the buffer here would look
    // more responsive and would be a different client: the reference's checkboxes genuinely do
    // not "stick" until the save comes back.
    g.set(
        "GuildControlGetRankFlags",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let rights = live_rights(&model, model.guild_control.rank);
            Ok(MultiValue::from_vec(
                RANK_RIGHT_BITS
                    .iter()
                    .map(|bit| era_bool(rights & bit != 0))
                    .collect(),
            ))
        })?,
    )?;

    // GuildControlSetRankFlag(index, enabled) (`0x4d2070`) — toggle one bit in the staging
    // buffer. The index is the checkbox's, 1..13, and RANK_RIGHT_BITS maps it to the bit. This is
    // the mapping that a `1 << (index - 1)` would get wrong for nine of the thirteen.
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

    // GuildControlSaveRank(name) (`0x4d20d0`) — flush the staged edits as one CMSG_GUILD_RANK.
    // The argument is the rank's (possibly renamed) NAME, taken from the popup's edit box
    // (`FriendsFrame.lua:839`); the rank it applies to is whichever GuildControlSetRank loaded.
    // Converted to the wire's 0-based rank id here, once. A name that fails
    // [`gated_rank_name`] — empty above all, which vmangos would otherwise persist as the rank's
    // name guild-wide — is a silent no-op, before any packet.
    //
    // **The fold is a masked set-or-clear over a freshly re-read LIVE baseline** — verified: the
    // binding does `or mask[k]` / `and ~mask[k]` per flag against the current rights, never a
    // plain OR of a staged word. Two consequences, both of which a naive "send the staged word"
    // implementation gets wrong:
    //   · a plain OR could never turn a permission OFF, only on;
    //   · bits outside the thirteen — notably `0x40`, which is tested NOWHERE in the image and is
    //     a genuine gap rather than an always-set flag — must ride through untouched from whatever
    //     the server last sent. Sending the staged word wholesale would clear them.
    g.set(
        "GuildControlSaveRank",
        lua.create_function(|lua, name: String| {
            let Some(name) = gated_rank_name(&name).map(str::to_string) else {
                return Ok(());
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let edit = model.guild_control.clone();
            // `[0x84966c] >= [0xb73120]`: no rank loaded (the reference's -1, our 0) or one at or
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

    // GuildControlAddRank(name) (`0x4d2210`) — append a rank, behind the same silent name gate
    // as SaveRank ([`gated_rank_name`]). The client refuses at
    // `numRanks >= MAX_RANKS` **silently**, before any packet, which is why the reference's Add
    // button greys itself at ten (`FriendsFrame.lua:887`) rather than relying on a refusal.
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

    // GuildControlDelRank() (`0x4d22e0`) — **reads no Lua argument at all**, and its packet
    // (`0x233`) is body-less: the server deletes the LAST rank and infers which that is. The
    // reference passes a name (`FriendsFrame.lua:895`) and the binding discards it, which is why
    // the reference only offers the button when the selected rank IS the last (`:908`).
    //
    // Accepting-and-ignoring is the faithful shape. Taking the name and sending it would invent a
    // wire field; refusing to compile against the reference's call would be worse.
    //
    // The client also refuses at `numRanks <= MIN_RANKS`, silently, before any packet.
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

    /// **`GetGuildInfo` splits three ways, not two** — `0x4c9330`: only an *unrecognised* token
    /// raises; a **recognised** token that resolves to nothing answers `(nil, nil, 0)` like a
    /// guildless player does.
    ///
    /// The distinction is not academic. Stock `PaperDollFrame_SetGuild` calls
    /// `GetGuildInfo("player")` unguarded from `PaperDollFrame_OnShow`, so before this split
    /// existed, opening the character window in the frames before the app pushed its first player
    /// snapshot raised — found by putting the reference's own `PaperDollFrame.xml` on the chain.
    /// `"player"` with no unit behind it is the whole test.
    #[test]
    fn a_recognised_but_unresolved_unit_answers_nils_and_only_an_unknown_token_raises() {
        let s = crate::script::UiScript::new().unwrap();
        // Recognised, resolves to nothing — the paper doll's own case at world entry.
        let (name, rank, idx): (Option<String>, Option<String>, i64) = s
            .eval(r#"return GetGuildInfo("player")"#)
            .expect("a recognised token must not raise");
        assert_eq!((name, rank, idx), (None, None, 0));
        // …and the third value is the NUMBER zero on that leg, not nil (`mov eax,3` on both).
        assert!(s
            .eval::<bool>(r#"local a,b,c = GetGuildInfo("party3") return c == 0"#)
            .unwrap());
        // Unrecognised: the resolver's own raise, unchanged.
        assert!(s
            .eval::<i64>(r#"return GetGuildInfo("nosuchunit")"#)
            .is_err());
    }

    /// The thirteen-bit table is the API contract, and its non-monotonicity is the trap: Promote
    /// and Demote sit at indices 5 and 6 with bits ABOVE Invite and Remove at 7 and 8. A
    /// `1 << (i - 1)` implementation passes indices 1-4 and fails the other nine, so this asserts
    /// the whole table rather than a property of it.
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

    /// **`SaveRank` folds over the live baseline, so bits outside the thirteen survive.**
    ///
    /// `0x40` is tested nowhere in the 1.12 image — it is a genuine gap in the bit space, not an
    /// always-set flag — and `SaveRank`'s fold neither sets nor clears it, so whatever the server
    /// last sent rides through. An implementation that sent the staged word wholesale would clear
    /// it (and any other unknown bit) on the first rank edit, silently, forever.
    ///
    /// The fold must also be able to turn a permission OFF, which a plain `OR` of a staged word
    /// cannot.
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

    /// A guild with `n` named ranks.
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

    /// **`0x4d20d0` refuses a bad name silently, before any packet** — NULL, empty, or a length
    /// outside 1..16 UTF-16 code units (`0x65b250` counts units plus the terminator), and a rank
    /// index at or past the rank count. vmangos persists whatever name arrives — an empty one
    /// blanks the rank guild-wide — so this gate is the only thing standing in the way.
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

        // No rank loaded (a fresh popup — the selection persists across saves, as
        // `[0x84966c]` does), or one past the ladder.
        let mut fresh = guild_with_ranks(5);
        fresh.run(r#"GuildControlSaveRank("Officer")"#).unwrap();
        assert_eq!(saves(&mut fresh), 0, "nothing selected");
        s.run(r#"GuildControlSetRank(6); GuildControlSaveRank("Officer")"#)
            .unwrap();
        assert_eq!(saves(&mut s), 0, "rank index 5 >= 5 ranks");
    }

    /// `0x4d2210` runs the same name gate as `SaveRank` — and nothing more: a whitespace name is
    /// not empty to it.
    #[test]
    fn adding_a_rank_runs_the_same_name_gate() {
        let mut s = guild_with_ranks(5);
        s.run(r#"GuildControlAddRank(""); GuildControlAddRank(string.rep("a", 17))"#)
            .unwrap();
        assert!(s.take_guild_requests().is_empty());
        s.run(r#"GuildControlAddRank(" ")"#).unwrap();
        assert_eq!(s.take_guild_requests().len(), 1);
    }

    /// A 1-based index past the end, and the 0 the reference passes on every `GuildStatus_Update`
    /// before it checks the selection, both have to answer `None` rather than panic.
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
