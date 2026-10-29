//! `TogglePVP` and the thirteen honor bindings: the character and inspect windows' Honor tabs and
//! the rank any unit frame can read off another player.
//!
//! The local player's counters are private descriptor fields, read off the active player's block
//! at the call ([`HonorNow`]); another player's arrive as a `MSG_INSPECT_HONOR_STATS` reply held in
//! the client's one inspect slot ([`InspectHonorNow`]), whose has-data flag is
//! `HasInspectHonorData`. With nothing held every getter answers zeros at full width, as the
//! reference reads zero-initialised slots ungated. With no host free to ask, the getters read the
//! snapshots ([`super::UiScript::set_honor`], [`super::UiScript::set_inspect_honor`]). A foreign player's current rank is
//! public (`PLAYER_BYTES_3` byte 3) and rides [`UnitState::pvp_rank`](super::UnitState::pvp_rank).
//!
//! The internal rank (0 none, 1..=4 dishonorable, 5..=18 Scout or Private to High Warlord or
//! Grand Marshal, 19 the racial "Leader") is what the wire carries and what keys the
//! `PVP_RANK_<rank>_<team>` GlobalStrings; the visual rank indexes the badge texture, and every
//! conversion goes through [`visual_rank`].
//!
//! Rank titles are read off the VM's globals, which the install's `GlobalStrings.lua` fills: the
//! key is `"PVP_RANK_%d_%d"` (`0x8445b4`) with team 0 Horde and 1 Alliance. There is no
//! `PVP_RANK_0_*`, and both panes fall back to `NONE` on its nil.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{number_arg, string_arg};
use super::unit::{check_unit_token, is_civilian_kill, read_identity, read_record, unknownobject};
use super::Model;

/// The local player's honor snapshot, decoded by the app from the private honor descriptor
/// fields. Kill counts are halves of `TWO_SHORT` fields, low honorable and high dishonorable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HonorState {
    /// `PLAYER_FIELD_SESSION_KILLS` low half.
    pub session_hk: u16,
    /// `PLAYER_FIELD_SESSION_KILLS` high half.
    pub session_dk: u16,
    /// `PLAYER_FIELD_YESTERDAY_KILLS` halves.
    pub yesterday_hk: u16,
    pub yesterday_dk: u16,
    /// `PLAYER_FIELD_YESTERDAY_CONTRIBUTION`.
    pub yesterday_honor: u32,
    /// `PLAYER_FIELD_THIS_WEEK_KILLS` low half; `GetPVPThisWeekStats` has no dishonorable count.
    pub this_week_hk: u16,
    /// `PLAYER_FIELD_THIS_WEEK_CONTRIBUTION`.
    pub this_week_honor: u32,
    /// `PLAYER_FIELD_LAST_WEEK_KILLS` halves.
    pub last_week_hk: u16,
    pub last_week_dk: u16,
    /// `PLAYER_FIELD_LAST_WEEK_CONTRIBUTION`.
    pub last_week_honor: u32,
    /// `PLAYER_FIELD_LAST_WEEK_RANK`: last week's ladder standing, not a rank.
    pub last_week_standing: u32,
    /// `PLAYER_FIELD_LIFETIME_HONORBALE_KILLS` / `…_DISHONORBALE_KILLS` (the server's spellings).
    pub lifetime_hk: u32,
    pub lifetime_dk: u32,
    /// `PLAYER_FIELD_BYTES` byte 3: the highest lifetime rank, internal scale. Carried raw: only
    /// `GetPVPLifetimeStats` hides a value below 5 (`0x51a843`).
    pub highest_rank: u8,
    /// `PLAYER_BYTES_3` byte 3: the current rank, internal scale, 0 for none; the same public
    /// byte as [`UnitState::pvp_rank`](super::UnitState).
    pub rank: u8,
    /// `PLAYER_FIELD_BYTES2` byte 0: progress through the current rank, the raw byte.
    pub rank_bar: u8,
}

/// One `MSG_INSPECT_HONOR_STATS` reply as the app decoded it. It has no dishonorable counts past
/// the session pair (the server writes zeros there), so `GetInspectHonorData` returns twelve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InspectHonorData {
    /// The player the reply was for, the app's key; never returned to Lua.
    pub guid: u64,
    pub session_hk: u16,
    pub session_dk: u16,
    pub yesterday_hk: u16,
    pub yesterday_honor: u32,
    pub this_week_hk: u16,
    pub this_week_honor: u32,
    pub last_week_hk: u16,
    pub last_week_honor: u32,
    /// The reply's `lastWeekRank`, the ladder standing: `GetInspectHonorData`'s ninth return.
    pub last_week_standing: u32,
    pub lifetime_hk: u32,
    pub lifetime_dk: u32,
    /// The reply's `highestRank`, internal scale: `GetInspectHonorData`'s twelfth return
    /// (`lifetimeRank`), unfiltered, as `0x4c9620` pushes it with no below-5 gate.
    pub highest_rank: u8,
    /// The reply's trailing `rankBar`, read only by `GetInspectPVPRankProgress`.
    pub rank_bar: u8,
}

/// The self getters' question, through the lent host ([`super::live::ask`]): the active player's
/// honor block as of the call (`0x51a4b0`-`0x51aa90` read `[+0xe68]`), zeros with no active player
/// (`0x51a524`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HonorNow;

impl super::live::LiveQuery for HonorNow {
    type Answer = HonorState;
}

/// The client's one inspect-honor slot as of the call: its has-data (`0xb71fc8`) and pending
/// (`0xb71fcc`) flags and the figures, which a new target leaves in place (`0x4c6f70`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InspectHonorSlot {
    pub has_data: bool,
    pub pending: bool,
    pub figures: InspectHonorData,
}

/// The inspect getters' question, through the lent host ([`super::live::ask`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InspectHonorNow;

impl super::live::LiveQuery for InspectHonorNow {
    type Answer = InspectHonorSlot;
}

/// The PvP flag's verb the host applies inside its call ([`super::ScriptCall::Pvp`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PvpCall {
    /// `TogglePVP()` (`0x48d700`): the empty `CMSG_TOGGLE_PVP` (`0x48d736`), one per call.
    Toggle,
}

impl super::UiScript {
    /// The `TogglePVP` calls a harness's VM kept ([`Self::keep_calls`]).
    #[cfg(test)]
    pub fn take_pvp_toggles(&mut self) -> u32 {
        self.take_calls_like(&super::ScriptCall::Pvp(PvpCall::Toggle)) as u32
    }

    /// `/pvp`, which benilla parses in Rust: the reference's `/pvp` is Lua over `TogglePVP`, so the
    /// same call.
    pub fn toggle_pvp(&mut self) {
        let lua = self.lua().clone();
        if let Err(e) = super::calls::submit(&lua, super::ScriptCall::Pvp(PvpCall::Toggle)) {
            self.model_mut().record_lua_error(&e);
        }
    }

    /// Push or clear the local player's honor snapshot, which the self getters read with no host
    /// free to ask ([`HonorNow`]). It fires no event.
    pub fn set_honor(&mut self, honor: Option<HonorState>) {
        self.model_mut().honor = honor;
    }

    /// Push or clear the snapshot of the last `MSG_INSPECT_HONOR_STATS` reply, which the inspect
    /// getters read with no host free to ask ([`InspectHonorNow`]); its presence is
    /// `HasInspectHonorData`. Either way the in-flight latch clears, as the engine's reply
    /// (`0x4c6f4c`) and re-key (`0x4c6f9d`) do.
    pub fn set_inspect_honor(&mut self, data: Option<InspectHonorData>) {
        let mut model = self.model_mut();
        model.inspect_honor = data;
        model.inspect_honor_pending = false;
    }

    /// The `RequestInspectHonorData` calls a harness's VM kept ([`Self::keep_calls`]), each a
    /// `MSG_INSPECT_HONOR_STATS` send for the app's inspect target. The `pending` latch
    /// (`0x4c80b6`) is the host's to set, after its send ([`Self::latch_inspect_honor_ask`]).
    pub fn take_inspect_honor_requests(&mut self) -> u32 {
        self.take_calls_like(&super::ScriptCall::Inspect(super::InspectCall::Honor)) as u32
    }

    /// The snapshot's inspect honor query is in flight (`[0xb71fcc] = 1`, `0x4c811b`), cleared by
    /// [`Self::set_inspect_honor`]; a lent host keeps its own.
    pub fn latch_inspect_honor_ask(&mut self) {
        self.model_mut().inspect_honor_pending = true;
    }

    /// A rank's localized title for the app's `SMSG_PVP_CREDIT` chat line, `None` if missing or
    /// empty. Gendered, unlike the pane's: the credit formatter resolves through `0x612bf0`. No
    /// range check, so rank 19 names "Leader"; `rank` is the internal rank the credit carries
    /// (`HonorMgr::SendPVPCredit` sends `GetRank().rank`).
    ///
    /// The caller owns two facts: the gender is the local player's while the team is the
    /// victim's (`0x625374`), and a victim whose team is -1 gets no line at all (`0x625321`),
    /// hence `team: u8`.
    pub fn pvp_rank_title(&self, rank: u8, team: u8, female: bool) -> Option<String> {
        rank_title_gendered(
            self.lua(),
            RankTitle {
                rank,
                // Past `i8` names no key, the lookup's own miss.
                team: i8::try_from(team).ok()?,
                female,
            },
        )
    }
}

/// Internal rank to visual rank (`0x51aa31`): 5..=18 subtract 4, and 1..=4 subtract 5 to give
/// -4..=-1 (`0x51aa38`). The server's `visualRank` negates instead (`HonorMgr.cpp:991`), so the
/// two disagree on ranks 1..=4; this is the client's. Only 1..=18 reach it, as `GetPVPRankInfo`
/// gates first.
fn visual_rank(rank: i64) -> i64 {
    if rank >= 5 {
        rank - 4
    } else {
        rank - 5
    }
}

/// The f32 at `0x8026c8` (bytes `81 80 80 3b`) that both rank-progress getters multiply the bar
/// byte by: the f32 nearest 1/255, `0.003921568859368563`, and not 1/255.
const RANK_BAR_SCALE: f64 = f32::from_bits(0x3B80_8081) as f64;

/// The rank bar's byte as the fraction the reference feeds a 0..1 `StatusBar`: `fild` then `fmul`
/// by [`RANK_BAR_SCALE`] (`0x51aace`), with no clamp. It differs from `bar / 255.0` for every byte
/// but 0, and a full bar gives `1.0000000091389835`, over the bar's maximum.
fn rank_progress(bar: u8) -> f64 {
    f64::from(bar) * RANK_BAR_SCALE
}

/// The active player's honor block as of the call, the snapshot's with no host free to ask.
fn honor(lua: &Lua) -> HonorState {
    super::live::ask(lua, &HonorNow).unwrap_or_else(|| {
        lua.app_data_ref::<Model>()
            .expect("model app_data")
            .honor
            .unwrap_or_default()
    })
}

/// The inspect-honor slot as of the call, the snapshot's with no host free to ask.
fn inspect_honor(lua: &Lua) -> InspectHonorSlot {
    super::live::ask(lua, &InspectHonorNow).unwrap_or_else(|| {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        InspectHonorSlot {
            has_data: model.inspect_honor.is_some(),
            pending: model.inspect_honor_pending,
            figures: model.inspect_honor.unwrap_or_default(),
        }
    })
}

/// A GlobalString off the VM's globals, `None` when absent or empty, as `GetPVPRankInfo` tests
/// both (`0x51aa1c` NULL, `0x51aa20` empty).
fn global_string(lua: &Lua, key: &str) -> Option<String> {
    lua.globals()
        .get::<Option<String>>(key)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// A unit's PvP team digit, `0x5efe00`'s tri-state from its race (`ChrRaces` to `FactionTemplate`,
/// group mask `& 4` then `& 2`): 0 Horde, 1 Alliance, -1 no side; resolved by the app as
/// [`UnitState::pvp_team`](super::UnitState). Not `faction_group`, the live faction template: a
/// vmangos GM's template 35 has no side while his race keeps one. -1 misses every key.
fn team_of(u: &super::UnitState) -> i64 {
    i64::from(u.pvp_team)
}

/// `GetPVPRankInfo`'s second argument as a team digit, the engine's three-way dispatch
/// (`0x51a98c`/`0x51a9af`/`0x51a9c8`): a number is the digit itself, truncated; a string is that
/// unit's [`team_of`], or 0 if it names nothing or a non-player; anything else is the local
/// player's, or 0 with no player. That 0 is the team register's initial value, so a miss names
/// off the Horde list, as in the reference. An unknown token raises, as `0x515970` does.
fn team_arg(lua: &Lua, v: Value) -> mlua::Result<i64> {
    // `lua_isnumber` before `lua_isstring`, which is what puts a numeric string on the number arm.
    if let Some(n) = lua.coerce_number(v.clone())? {
        return Ok(i64::from(n as i64 as i32));
    }
    let Some(token) = lua.coerce_string(v)? else {
        // Absent, boolean or table: the local player.
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        return Ok(crate::script::unit::view(lua, &model, "player").map_or(0, |u| team_of(&u)));
    };
    let token = Some(token.to_str()?.to_owned());
    check_unit_token(&token)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    Ok(token
        .as_ref()
        .and_then(|t| crate::script::unit::view(lua, &model, t))
        // `0x51a9af`'s player gate (`shr edx,4; test dl,1`), applied here unlike in `UnitPVPRank`:
        // on this arm nothing else tells a creature, which has a race too, from a player.
        .filter(|u| u.is_player)
        .map_or(0, |u| team_of(&u)))
}

/// `PVP_RANK_<internal>_<team>`, ungendered: `GetPVPRankInfo`'s lookup, `0x703bf0(key, -1, 0)`
/// (`0x51aa0f` pushes gender 0), so the panes show a female character the default title. Do not
/// gender it: that would diverge from the reference.
fn rank_title_ungendered(lua: &Lua, rank: i64, team: i64) -> Option<String> {
    global_string(lua, &format!("PVP_RANK_{rank}_{team}"))
}

/// The `PVP_RANK_<rank>_<team>[_FEMALE]` inputs, kept together because the live lookup and the
/// world-text pass's builder key on the same three.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RankTitle {
    /// `PLAYER_BYTES_3` byte 3 (`UnitPVPRank`), the internal rank 0..=18; 19 is the racial
    /// "Leader" the keys still name.
    pub rank: u8,
    /// The team digit, 0 Horde, 1 Alliance, -1 no side: a unit name's from its race, an honor
    /// credit's from its victim's faction template group (`0x6252de`).
    pub team: i8,
    /// Female takes the `_FEMALE` twin (`0x612bf0`) and falls back to the base key.
    pub female: bool,
}

/// The same title gendered, `0x612bf0`: it passes 2 (male) or 3 (female) to `0x703bf0`, which
/// asks FrameXML's `GetText` to append `_FEMALE` for 3 (the engine never builds that key), and
/// retries ungendered on a miss (`0x612c2d`). The credit line's lookup; `UnitPVPName` and the
/// overhead name reach the same keys through [`decorated_name`].
fn rank_title_gendered(lua: &Lua, title: RankTitle) -> Option<String> {
    rank_title(&|key| global_string(lua, key), title)
}

/// [`rank_title_gendered`]'s key construction over a string lookup, so a caller with no VM (the
/// world-text pass) resolves the same `_FEMALE` twin and fallback.
fn rank_title(lookup: &impl Fn(&str) -> Option<String>, title: RankTitle) -> Option<String> {
    title
        .female
        .then(|| lookup(&format!("PVP_RANK_{}_{}_FEMALE", title.rank, title.team)))
        .flatten()
        .or_else(|| lookup(&format!("PVP_RANK_{}_{}", title.rank, title.team)))
}

/// Fill a two-`%s` C format string, the install's template (enUS `UNIT_PVP_NAME` is `"%s %s"`);
/// `%%` is a literal `%` and any other specifier passes through. Deviation: a third `%s` gets an
/// empty string, because the reference pushes two varargs (`0x6093f2 add esp,0x14`) and would
/// read uninitialised stack.
fn format_two_strings(fmt: &str, a: &str, b: &str) -> String {
    let mut out = String::with_capacity(fmt.len() + a.len() + b.len());
    let mut args = [a, b].into_iter();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push_str(args.next().unwrap_or("")),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// The install keys a name-line rank prefix can resolve: `UNIT_PVP_NAME` and every
/// `PVP_RANK_<rank>_<team>[_FEMALE]` for ranks 1..=19 and both team digits. The range reaches 19
/// because the builder is range-unchecked and the install's `PVP_RANK_19_*` names the racial
/// "Leader", past the player rank byte's 0..=18. The world-text pass, which runs without the VM,
/// reads them off each VM once; `UnitPVPName` looks them up live.
pub fn pvp_name_global_keys() -> Vec<String> {
    let mut keys = Vec::with_capacity(1 + 19 * 2 * 2);
    keys.push("UNIT_PVP_NAME".to_string());
    for rank in 1..=19 {
        for team in 0..=1 {
            keys.push(format!("PVP_RANK_{rank}_{team}"));
            keys.push(format!("PVP_RANK_{rank}_{team}_FEMALE"));
        }
    }
    keys
}

/// `0x609370`'s ranked-player leg over a string lookup: `UNIT_PVP_NAME` filled rank first, the
/// same builder [`pvp_name`] runs for `UnitPVPName`. `None` for rank 0, or a title or template
/// the lookup lacks, where the callers keep the plain name: the reference prints a missing title
/// as `""` (`" " + name`), which a playable race's 0..=18 rank never reaches in the stock strings.
pub fn decorated_name(
    lookup: impl Fn(&str) -> Option<String>,
    key: RankTitle,
    name: &str,
) -> Option<String> {
    if key.rank == 0 {
        return None;
    }
    let title = rank_title(&lookup, key)?;
    let template = lookup("UNIT_PVP_NAME")?;
    Some(format_two_strings(&template, &title, name))
}

/// `0x609370`, the name builder behind `UnitPVPName` (its legs are at the binding) and the unit
/// tooltip's title (`0x52a1ab`), both with the decoration flag `1`; `name` is the plain `UnitName`.
pub(super) fn pvp_name(lua: &Lua, u: &super::UnitState, name: &str, player_level: u32) -> String {
    // Leg A: a ranked player; the title is gendered by this unit and range-unchecked.
    if u.is_player && u.pvp_rank != 0 {
        let decorated = decorated_name(
            |key| global_string(lua, key),
            RankTitle {
                rank: u.pvp_rank,
                team: u.pvp_team,
                female: u.sex == 3,
            },
            name,
        );
        if let Some(mut out) = decorated {
            // Leg A′: the city-protector medal, on its own line, ungendered (`0x60941d push 0`).
            if u.pvp_medal != 0 {
                if let Some(medal) = global_string(lua, &format!("PVP_MEDAL{}", u.pvp_medal)) {
                    out.push('\n');
                    out.push_str(&medal);
                }
            }
            return out;
        }
        return name.to_owned();
    }
    // Leg B: the dishonorable-kill warning, on the same predicate the tooltip's CIVILIAN line uses.
    if is_civilian_kill(u, player_level) {
        if let Some(civilian) = global_string(lua, "PVP_RANK_CIVILIAN") {
            return format!("{civilian} {name}");
        }
    }
    // Leg C.
    name.to_owned()
}

/// Register the PvP + honor globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // TogglePVP(): no argument in 1.12, as the opcode's one-byte state form has no binding.
    // Registered at `0x48d700`; the shipped UI calls it only from `SlashCmdList["PVP"]`.
    g.set(
        "TogglePVP",
        lua.create_function(|lua, ()| {
            super::calls::submit(lua, super::ScriptCall::Pvp(PvpCall::Toggle))
        })?,
    )?;

    // ── The self getters ─────────────────────────────────────────────────────────────────────
    // Full arity on every path: the panes destructure straight into `format()` arguments, so
    // before the first push they read as a zeroed snapshot, as a new character's would.

    // GetPVPSessionStats() → hk, dk.
    g.set(
        "GetPVPSessionStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua);
            Ok((i64::from(h.session_hk), i64::from(h.session_dk)))
        })?,
    )?;

    // GetPVPYesterdayStats() → hk, dk, contribution.
    g.set(
        "GetPVPYesterdayStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua);
            Ok((
                i64::from(h.yesterday_hk),
                i64::from(h.yesterday_dk),
                i64::from(h.yesterday_honor),
            ))
        })?,
    )?;

    // GetPVPThisWeekStats() → hk, contribution: two, with no this-week dishonorable count.
    g.set(
        "GetPVPThisWeekStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua);
            Ok((i64::from(h.this_week_hk), i64::from(h.this_week_honor)))
        })?,
    )?;

    // GetPVPLastWeekStats() → hk, dk, contribution, standing: the ladder standing, which the pane
    // prints through `PVP_RANK_LAST_WEEK`, not a rank.
    g.set(
        "GetPVPLastWeekStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua);
            Ok((
                i64::from(h.last_week_hk),
                i64::from(h.last_week_dk),
                i64::from(h.last_week_honor),
                i64::from(h.last_week_standing),
            ))
        })?,
    )?;

    // GetPVPLifetimeStats() → hk, dk, highestRank, the highest lifetime rank (internal). One
    // below 5 answers 0 (`0x51a843 cmp al,5; jb`, unsigned), so the pane shows NONE for a
    // dishonorable best; 0, not nil, so its `GetPVPRankInfo(highestRank)` still gets a number.
    g.set(
        "GetPVPLifetimeStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua);
            let highest = if h.highest_rank >= 5 {
                h.highest_rank
            } else {
                0
            };
            Ok((
                i64::from(h.lifetime_hk),
                i64::from(h.lifetime_dk),
                i64::from(highest),
            ))
        })?,
    )?;

    // GetPVPRankProgress() → the bar fraction (`rank_progress`), 0.0 before the first push.
    g.set(
        "GetPVPRankProgress",
        lua.create_function(|lua, ()| Ok(rank_progress(honor(lua).rank_bar)))?,
    )?;

    // GetPVPRankInfo(rank [, unit]) → rankName, rankNumber: the title and the visual rank for an
    // internal rank, always two values; every failure answers `nil, 0`, which both panes turn
    // into `NONE`. `0x51a930` accepts [1, 18], signed (`0x51a9f0`/`0x51a9f5`), so rank 19,
    // "Leader", is refused although its GlobalStrings exist and the server sends it. The second
    // argument is `team_arg`'s three-way dispatch.
    g.set(
        "GetPVPRankInfo",
        lua.create_function(|lua, (rank, unit): (Value, Value)| {
            let rank = i64::from(number_arg(
                lua,
                rank,
                "Usage: GetPVPRankInfo(rank [, unit])",
            )?);
            let team = team_arg(lua, unit)?;
            // The range gate first: outside [1, 18] no key is even built.
            let name = if (1..=18).contains(&rank) {
                rank_title_ungendered(lua, rank, team)
            } else {
                None
            };
            let Some(name) = name else {
                return Ok(MultiValue::from_vec(vec![Value::Nil, Value::Integer(0)]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                Value::Integer(visual_rank(rank)),
            ]))
        })?,
    )?;

    // UnitPVPRank(unit) → the unit's current rank, internal, 0 for none. `0x51a8a0` reads the
    // named object's own byte (`[+0xe68]+0x1f`, in public `PLAYER_BYTES_3`), which is how the
    // inspect pane reads a foreign rank. Anything unresolved reads 0, never nil (`0x51a916`); a
    // non-string argument raises (`0x51a8ac`).
    //
    // The engine's player gate (`0x51a8e1`) is not re-applied: it tests the type mask, while our
    // `is_player` comes from the app's later guid-keyed enrichment, so a snapshot pushed before
    // it would zero a real player's rank. A creature has no player block, so its byte is 0.
    g.set(
        "UnitPVPRank",
        lua.create_function(|lua, token: Value| {
            let token = Some(string_arg(lua, token, "Usage: UnitPVPRank(\"unit\")")?);
            check_unit_token(&token)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(token
                .as_ref()
                .and_then(|t| crate::script::unit::view(lua, &model, t))
                .map_or(0i64, |u| i64::from(u.pvp_rank)))
        })?,
    )?;

    // UnitPVPName(unit) → the name decorated by rank, `0x5172b0` into the builder `0x609370`:
    //   A  a ranked player: `UNIT_PVP_NAME` filled title first, then name; the title is gendered
    //      by this unit (`0x5efe60`, `0x612bf0`) with no range check, so rank 19 renders "Leader".
    //   A′ and, if `PLAYER_BYTES_3` byte 2 (the city-protector title) is set, a second line with
    //      `PVP_MEDAL<n>`, ungendered.
    //   B  else a unit passing the civilian predicate (`0x612550`): `PVP_RANK_CIVILIAN`, a space
    //      and the name. Only creatures reach it.
    //   C  else the plain name.
    // No snapshot or no object (the unit lookup `0x517309`, typemask 8, finds none and pushes nil
    // at `0x51733e`) answers nil. A held unit whose name is not cached yet is built from
    // `UNKNOWNOBJECT` (`0x609210` returns it on a miss, `0x609324`-`0x60933c`; `0x5172b0` always
    // pushes the buffer), and the miss asks the name cache with its callback (`0x55f080`,
    // `0x5fb840`) as `UnitName`'s does.
    //
    // Deviation: a token resolving to guid 0 answers nil too. The reference jumps from the guid
    // test (`0x5172f6`) to `return 1` (`0x517343`) with nothing pushed, so the one value is its
    // top argument, the token it read (`0x5172e6`); that is a stack quirk, not an answer.
    //
    // Deviation: with `UNIT_PVP_NAME` or the title missing from `_G` (no install) this answers
    // the plain name, where the reference would format an empty string, because the decoration
    // is install data.
    g.set(
        "UnitPVPName",
        lua.create_function(|lua, token: Value| {
            let token = Some(string_arg(lua, token, "Usage: UnitPVPName(\"unit\")")?);
            check_unit_token(&token)?;
            let (unit, player_level) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                (
                    token
                        .as_ref()
                        .and_then(|t| crate::script::unit::view(lua, &model, t))
                        .map(std::borrow::Cow::into_owned),
                    crate::script::requirements::player_head(lua, &model).map_or(0, |h| h.level),
                )
            };
            let Some(mut u) = unit.filter(|u| u.has_object) else {
                return Ok(Value::Nil);
            };
            // The name (`GetUnitName`, `0x609210`) and the civilian leg (`0x612550`) are read off
            // the name caches as the call runs, the snapshot's only with no host free to ask.
            let tok = token.as_deref().unwrap_or_default();
            let name = match read_identity(lua, tok, |_, id| id.name()).flatten() {
                Some(name) => name,
                None => unknownobject(lua)?.to_str()?.to_string(),
            };
            if let Some(civilian) = read_record(lua, tok, |_, r| r.civilian) {
                u.civilian = civilian;
            }
            let decorated = pvp_name(lua, &u, &name, player_level);
            Ok(Value::String(lua.create_string(&decorated)?))
        })?,
    )?;

    // ── The inspect half ─────────────────────────────────────────────────────────────────────

    // RequestInspectHonorData(): `InspectHonorFrame_OnShow` calls it while
    // `HasInspectHonorData()` is false; the app sends `MSG_INSPECT_HONOR_STATS` for its inspect
    // target, inside the call. Zero return values (`0x4c9610`).
    //
    // `0x4c80a0` bails silently if data is held (`0x4c80a6`), if a query is in flight (`pending`,
    // `0x4c80b6`, `[0xb71fcc]`) or if the target guid is 0 (`0x4c80c2`). The first two are here,
    // read as of the call; the guid is the host's, which sends and then latches `pending`
    // (`0x4c811b`), so a request with no target leaves it clear.
    g.set(
        "RequestInspectHonorData",
        lua.create_function(|lua, ()| {
            let slot = inspect_honor(lua);
            if slot.has_data || slot.pending {
                return Ok(());
            }
            super::calls::submit(lua, super::ScriptCall::Inspect(super::InspectCall::Honor))
        })?,
    )?;

    // HasInspectHonorData() → 1 or nil: whether a reply is held.
    g.set(
        "HasInspectHonorData",
        lua.create_function(|lua, ()| {
            Ok(if inspect_honor(lua).has_data {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetInspectHonorData() → the reference's twelve, in its order:
    //   sessionHK, sessionDK, yesterdayHK, yesterdayHonor, thisweekHK, thisweekHonor,
    //   lastweekHK, lastweekHonor, lastweekStanding, lifetimeHK, lifetimeDK, lifetimeRank
    // Twelve on every path: `0x4c9620` never checks the flag and reads the cells, zeros before
    // any reply; the app's slot zeroes them on a re-key too, its own `Deviation:`.
    g.set(
        "GetInspectHonorData",
        lua.create_function(|lua, ()| {
            let d = inspect_honor(lua).figures;
            let out = vec![
                Value::Integer(i64::from(d.session_hk)),
                Value::Integer(i64::from(d.session_dk)),
                Value::Integer(i64::from(d.yesterday_hk)),
                Value::Integer(i64::from(d.yesterday_honor)),
                Value::Integer(i64::from(d.this_week_hk)),
                Value::Integer(i64::from(d.this_week_honor)),
                Value::Integer(i64::from(d.last_week_hk)),
                Value::Integer(i64::from(d.last_week_honor)),
                Value::Integer(i64::from(d.last_week_standing)),
                Value::Integer(i64::from(d.lifetime_hk)),
                Value::Integer(i64::from(d.lifetime_dk)),
                Value::Integer(i64::from(d.highest_rank)),
            ];
            debug_assert_eq!(out.len(), 12, "the tuple is twelve wide on every path");
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetInspectPVPRankProgress() → the reply's `rankBar` through `rank_progress`: `0x51ab00`'s
    // kernel at `0x51ab04` is byte-identical to `0x51aace`, so it too reads 0.0 when empty.
    g.set(
        "GetInspectPVPRankProgress",
        lua.create_function(|lua, ()| Ok(rank_progress(inspect_honor(lua).figures.rank_bar)))?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests;
