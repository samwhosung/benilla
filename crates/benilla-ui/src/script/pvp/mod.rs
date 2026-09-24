//! `TogglePVP` and the thirteen honor bindings: the character and inspect windows' Honor tabs and
//! the rank any unit frame can read off another player.
//!
//! The local player's counters are private descriptor fields the app pushes as a [`HonorState`];
//! another player's arrive as a `MSG_INSPECT_HONOR_STATS` reply ([`InspectHonorData`]), whose
//! presence is `HasInspectHonorData`. With nothing held every getter answers zeros at full width,
//! as the reference reads zero-initialised slots ungated. A foreign player's current rank is
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
use super::unit::{check_unit_token, is_civilian_kill};
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

impl super::UiScript {
    /// Drain the queued `TogglePVP` calls, one `CMSG_TOGGLE_PVP` each; the packet is empty, so
    /// two toggles in a frame are two sends.
    pub fn take_pvp_toggles(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().pvp_toggles)
    }

    /// Queue a toggle for `/pvp`, which benilla parses in Rust; the reference's `/pvp` is Lua over
    /// `TogglePVP`.
    pub fn queue_pvp_toggle(&mut self) {
        self.model_mut().pvp_toggles += 1;
    }

    /// Push or clear the local player's honor snapshot, the self getters' only source. It fires
    /// no event: the app fires `PLAYER_PVP_KILLS_CHANGED`/`PLAYER_PVP_RANK_CHANGED` alongside.
    pub fn set_honor(&mut self, honor: Option<HonorState>) {
        self.model_mut().honor = honor;
    }

    /// Push or clear the last `MSG_INSPECT_HONOR_STATS` reply, paired with the pane's
    /// `INSPECT_HONOR_UPDATE`. Its presence is `HasInspectHonorData`, so the app clears it on
    /// `ClearInspectPlayer` and the pane's `OnShow` asks again for the next player. Either way
    /// the in-flight latch clears, as the engine's reply (`0x4c6f4c`) and re-key (`0x4c6f9d`) do.
    pub fn set_inspect_honor(&mut self, data: Option<InspectHonorData>) {
        let mut model = self.model_mut();
        model.inspect_honor = data;
        model.inspect_honor_pending = false;
    }

    /// Drain the queued `RequestInspectHonorData` calls, each a `MSG_INSPECT_HONOR_STATS` send for
    /// the app's inspect target. At most 1: the `pending` latch (`0x4c80b6`) refuses another until
    /// [`Self::set_inspect_honor`] resolves the first.
    pub fn take_inspect_honor_requests(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().inspect_honor_requests)
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
        rank_title_gendered(self.lua(), i64::from(rank), i64::from(team), female)
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

fn honor(lua: &Lua) -> Option<HonorState> {
    lua.app_data_ref::<Model>().expect("model app_data").honor
}

fn inspect_honor(lua: &Lua) -> Option<InspectHonorData> {
    lua.app_data_ref::<Model>()
        .expect("model app_data")
        .inspect_honor
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
        return Ok(model.unit("player").map_or(0, team_of));
    };
    let token = Some(token.to_str()?.to_owned());
    check_unit_token(&token)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    Ok(token
        .as_ref()
        .and_then(|t| model.unit(t))
        // `0x51a9af`'s player gate (`shr edx,4; test dl,1`), applied here unlike in `UnitPVPRank`:
        // on this arm nothing else tells a creature, which has a race too, from a player.
        .filter(|u| u.is_player)
        .map_or(0, team_of))
}

/// `PVP_RANK_<internal>_<team>`, ungendered: `GetPVPRankInfo`'s lookup, `0x703bf0(key, -1, 0)`
/// (`0x51aa0f` pushes gender 0), so the panes show a female character the default title. Do not
/// gender it: that would diverge from the reference.
fn rank_title_ungendered(lua: &Lua, rank: i64, team: i64) -> Option<String> {
    global_string(lua, &format!("PVP_RANK_{rank}_{team}"))
}

/// The same title gendered, `0x612bf0`: it passes 2 (male) or 3 (female) to `0x703bf0`, which
/// asks FrameXML's `GetText` to append `_FEMALE` for 3 (the engine never builds that key), and
/// retries ungendered on a miss (`0x612c2d`). Used by `UnitPVPName` and the credit line.
fn rank_title_gendered(lua: &Lua, rank: i64, team: i64, female: bool) -> Option<String> {
    female
        .then(|| global_string(lua, &format!("PVP_RANK_{rank}_{team}_FEMALE")))
        .flatten()
        .or_else(|| rank_title_ungendered(lua, rank, team))
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

/// `0x609370`, the name builder behind `UnitPVPName` (its legs are at the binding); `name` is the
/// plain `UnitName`.
fn pvp_name(lua: &Lua, u: &super::UnitState, name: &str, player_level: u32) -> String {
    // Leg A: a ranked player; the title is gendered by this unit and range-unchecked.
    if u.is_player && u.pvp_rank != 0 {
        let title = rank_title_gendered(lua, i64::from(u.pvp_rank), team_of(u), u.sex == 3);
        if let (Some(fmt), Some(title)) = (global_string(lua, "UNIT_PVP_NAME"), title) {
            let mut out = format_two_strings(&fmt, &title, name);
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
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pvp_toggles += 1;
            Ok(())
        })?,
    )?;

    // ── The self getters ─────────────────────────────────────────────────────────────────────
    // Full arity on every path: the panes destructure straight into `format()` arguments, so
    // before the first push they read as a zeroed snapshot, as a new character's would.

    // GetPVPSessionStats() → hk, dk.
    g.set(
        "GetPVPSessionStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
            Ok((i64::from(h.session_hk), i64::from(h.session_dk)))
        })?,
    )?;

    // GetPVPYesterdayStats() → hk, dk, contribution.
    g.set(
        "GetPVPYesterdayStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
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
            let h = honor(lua).unwrap_or_default();
            Ok((i64::from(h.this_week_hk), i64::from(h.this_week_honor)))
        })?,
    )?;

    // GetPVPLastWeekStats() → hk, dk, contribution, standing: the ladder standing, which the pane
    // prints through `PVP_RANK_LAST_WEEK`, not a rank.
    g.set(
        "GetPVPLastWeekStats",
        lua.create_function(|lua, ()| {
            let h = honor(lua).unwrap_or_default();
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
            let h = honor(lua).unwrap_or_default();
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
        lua.create_function(|lua, ()| Ok(rank_progress(honor(lua).unwrap_or_default().rank_bar)))?,
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
                .and_then(|t| model.unit(t))
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
    // No snapshot or an unresolved name answers nil, and so does a token resolving to guid 0,
    // where the reference returns one value it never pushed (`0x517343`).
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
                    token.as_ref().and_then(|t| model.unit(t)).cloned(),
                    model.player_req.level,
                )
            };
            let Some(u) = unit else {
                return Ok(Value::Nil);
            };
            let Some(name) = u.name.clone() else {
                return Ok(Value::Nil);
            };
            let decorated = pvp_name(lua, &u, &name, player_level);
            Ok(Value::String(lua.create_string(&decorated)?))
        })?,
    )?;

    // ── The inspect half ─────────────────────────────────────────────────────────────────────

    // RequestInspectHonorData(): `InspectHonorFrame_OnShow` calls it while
    // `HasInspectHonorData()` is false; the app sends `MSG_INSPECT_HONOR_STATS` for its inspect
    // target. Zero return values (`0x4c9610`).
    //
    // `0x4c80a0` bails silently if data is held (`0x4c80a6`), if a query is in flight (`pending`,
    // `0x4c80b6`, `[0xb71fcc]`) or if the target guid is 0 (`0x4c80c2`). The first two are here;
    // the guid is the app's, which drops a request with no target. Our `pending` latches on
    // queue, not send, so a dropped request holds it until the next `set_inspect_honor`, which
    // `ClearInspectPlayer` calls as `0x4c6f70` clears the engine's.
    g.set(
        "RequestInspectHonorData",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.inspect_honor.is_some() || model.inspect_honor_pending {
                return Ok(());
            }
            model.inspect_honor_requests += 1;
            model.inspect_honor_pending = true;
            Ok(())
        })?,
    )?;

    // HasInspectHonorData() → 1 or nil: whether a reply is held.
    g.set(
        "HasInspectHonorData",
        lua.create_function(|lua, ()| {
            Ok(if inspect_honor(lua).is_some() {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetInspectHonorData() → the reference's twelve, in its order:
    //   sessionHK, sessionDK, yesterdayHK, yesterdayHonor, thisweekHK, thisweekHonor,
    //   lastweekHK, lastweekHonor, lastweekStanding, lifetimeHK, lifetimeDK, lifetimeRank
    // Twelve on every path: `0x4c9620` never checks for data and reads zero-initialised slots.
    // Clearing drops the data here, while the reference's re-key zeroes only the flags and can
    // still answer the previous target's numbers; matching that needs a latch apart from the data.
    g.set(
        "GetInspectHonorData",
        lua.create_function(|lua, ()| {
            let d = inspect_honor(lua).unwrap_or_default();
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
        lua.create_function(|lua, ()| {
            Ok(rank_progress(
                inspect_honor(lua).unwrap_or_default().rank_bar,
            ))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests;
