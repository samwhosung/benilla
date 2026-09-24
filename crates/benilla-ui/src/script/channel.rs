//! `GetChannelName` — the joined-channel lookup, in both directions.
//!
//! **The state this reads already existed and was built for this verb.** `ui_chat::edit`'s
//! `ChannelState` has held `joined: Vec<String>` in join order since the chat arc, and its own doc
//! comment names `GetChannelName(n)` as the law it implements. Only the registration was missing —
//! the same silent-gap shape the loader arc kept finding, one layer up: the capability was built,
//! nothing exposed it, and nothing complained.
//!
//! Corpus demand, counted by reading every line rather than the grep total (1207): **17 sites
//! across 6 addons**, every one an unguarded call, no library replicated —
//! `ChatLog` 7, `Recap` 4, `SmartRes` 2, `Enchantrix` 2, `FuBar_AssistFu` 1, `_LazyPig` 1. Both
//! lookup directions are live in the corpus: by index (`GetChannelName(i)`) and by name
//! (`GetChannelName("world")`, `GetChannelName("Trade - City")`).
//!
//! Signature verified against the reference — `GetChannelName = 0x4a05e0`, **three** returns:
//!
//! | # | the client's | ours |
//! |---|---|---|
//! | 1 | `slot[+0x00]`, the client-local **1-based joined-slot index** (= `CHAT_MSG_*` arg8) | the position in `joined` |
//! | 2 | `slot[+0x04]`, the channel name | the `joined` entry |
//! | 3 | `slot[+0x98]`, FrameXML's `instanceID` (= arg10) | **0** — see below |
//!
//! **Return 3 is 0, not nil, and that is a recorded position rather than a shrug.** It is the split
//! index from `YOU_JOINED`'s second u32, which `ui_chat::event`'s doc already records as "0 on every
//! vanilla emulator" and deliberately does not store. A client that one day meets a server which
//! splits channels would need the field; nothing in the corpus reads it.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The 1-based slot of `name`, case-insensitively — `GetChannelName`'s first return.
/// One `ChatChannels.dbc` row as the VM needs it for `JoinChannelByName` (decision 1908),
/// `Add/RemoveChatWindowChannel` (`0x4a1000`/`0x4a1260`) and `EnumerateServerChannels`
/// (`0x4a1790`): the id, the **Shortcut** (`General`, `Trade`, … — what every one of those verbs
/// compares a typed name against, whole and case-folded), the name composed for the zone the
/// player is in (`General - Elwynn Forest`; `None` while the zone text is empty, which is the
/// verbs' nil leg), and whether the row is listed here at all (a `flags & 0x10` city row is, only
/// in a city).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneChannelRow {
    pub id: u32,
    pub shortcut: String,
    pub resolved: Option<String>,
    pub listed: bool,
}

/// A channel verb's ask, drained by the app into its `CMSG_*` (all of which
/// `benilla-protocol::messages::client` already builds). The verbs are the stock
/// `ChatFrame.lua` slash handlers' calls, one variant each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelCommand {
    /// `JoinChannelByName(name, password)` — sent on both of 1908's non-nil legs.
    Join {
        name: String,
        password: String,
    },
    /// `LeaveChannelByName(name)`.
    Leave {
        name: String,
    },
    /// `ListChannelByName(name)` — `CMSG_CHANNEL_LIST`.
    List {
        name: String,
    },
    /// `ListChannels()` — the roster of every joined channel.
    ListAll,
    /// `DisplayChannelOwner(name)` — `CMSG_CHANNEL_OWNER`.
    DisplayOwner {
        name: String,
    },
    /// `SetChannelOwner(name, player)`.
    SetOwner {
        name: String,
        player: String,
    },
    /// `SetChannelPassword(name, password)`.
    SetPassword {
        name: String,
        password: String,
    },
    Ban {
        name: String,
        player: String,
    },
    Invite {
        name: String,
        player: String,
    },
    Kick {
        name: String,
        player: String,
    },
    Moderator {
        name: String,
        player: String,
    },
    Unmoderator {
        name: String,
        player: String,
    },
    Mute {
        name: String,
        player: String,
    },
    Unmute {
        name: String,
        player: String,
    },
    Unban {
        name: String,
        player: String,
    },
    /// `ChannelModerate(name)` — toggles moderation.
    Moderate {
        name: String,
    },
    /// `ChannelToggleAnnouncements(name)`.
    ToggleAnnouncements {
        name: String,
    },
}

impl super::UiScript {
    /// The zone-channel catalog for the player's current zone — fed whenever the zone (and so
    /// every resolved name) changes.
    pub fn set_zone_channel_catalog(&mut self, rows: Vec<ZoneChannelRow>) {
        self.model_mut().zone_channel_catalog = rows;
    }

    /// Channel verbs called since the last drain, in call order.
    pub fn take_channel_commands(&mut self) -> Vec<ChannelCommand> {
        std::mem::take(&mut self.model_mut().channel_commands)
    }

    /// The guild-recruitment auto-join latch — `0` STANDARD, `1` AUTO (decision 2115).
    pub fn guild_recruitment_mode(&self) -> u8 {
        self.model_ref().guild_recruitment_mode
    }

    /// Seat the latch from the host (the per-character chat cache at login). A host write is not
    /// a player gesture, so it does **not** arm [`Self::take_guild_recruitment_change`] — the same
    /// split `set_cvar_from_host` keeps one store over, and what stops a login from composing the
    /// player's file out of a value the login itself just read.
    pub fn set_guild_recruitment_mode(&mut self, mode: u8) {
        let mut model = self.model_mut();
        model.guild_recruitment_mode = mode;
        model.guild_recruitment_changed = false;
    }

    /// Has Lua moved the latch since the last drain? The chat cache's dirty signal.
    pub fn take_guild_recruitment_change(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().guild_recruitment_changed)
    }

    /// Has Lua called `SetGuildRecruitmentMode(1)` since the last drain? The cascade's cue
    /// (`0x49ea70` → `0x49ea90`; decision 2144).
    pub fn take_guild_recruitment_cascade(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().guild_recruitment_cascade)
    }

    /// **A manual join or leave of `GuildRecruitment` forces the latch to 0** — the reference's
    /// `0x49ed3d`/`0x49ef8f`, `call 0x49ea70(0)`: a player gesture, so unlike the host seat it
    /// **does** arm the save. No cascade (mode 0 is the latch alone). Answers whether it moved.
    pub fn reset_guild_recruitment_mode(&mut self) -> bool {
        let mut model = self.model_mut();
        if model.guild_recruitment_mode == 0 {
            return false;
        }
        model.guild_recruitment_mode = 0;
        model.guild_recruitment_changed = true;
        true
    }
}

fn push(lua: &Lua, cmd: ChannelCommand) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    model.channel_commands.push(cmd);
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn slot_of(model: &Model, name: &str) -> Option<usize> {
    model
        .joined_channels
        .iter()
        .position(|c| c.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(name)))
        .map(|i| i + 1)
}

/// The name occupying slot `n` (1-based), or `None` for out of range **or a freed slot**.
///
/// The hole case is the reference's, not a convenience: its by-index lookup `0x49bf30` bounds-checks
/// against the record count and then demands the entry's own number field equal the index asked for
/// (`cmp esi,ecx / jnz`), which a leave zeroed (`0x49bbd0`). So a channel left is a number that
/// answers "not joined" while every channel above it keeps its own (1286).
fn name_at(model: &Model, n: usize) -> Option<&str> {
    model.joined_channels.get(n.checked_sub(1)?)?.as_deref()
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The guild-recruitment auto-join pair (decision 2115) ──────────────────────────────────
    //
    // `GetGuildRecruitmentMode 0x4a0040` / `SetGuildRecruitmentMode 0x4a0060` — the store behind
    // the reference's *Auto-join the Guild Recruitment Channel* option
    // (`UIOptionsFrameCheckButtons["AUTO_JOIN_GUILD_CHANNEL"]`, index 51). The table entry is a
    // bare `{ index = 51 }` with no cvar and no uvar, which reads as "unbound" and is not: the
    // store is the three special-case arms `UIOptionsFrame_Load` (l.243-248), `_Save` (l.326-331)
    // and `_SetDefaults` (l.649) keep for that key. Exactly the `SHOW_TUTORIALS` shape decision
    // 2077 corrected, and this tree's own `OptionsFrame.xml` carried the wrong reading until 2115.
    //
    // **The state is one int** — the reference's `[0x843608]`, written only by `0x49ea70`
    // (`mov ds:0x843608,ecx`; five call sites, no address-takes) and read by the Lua getter and by
    // the chat-cache writer. Ours is `Model::guild_recruitment_mode`, seated at login and rendered
    // back out by `ui_chat::settings`; the whole round trip is that file's one
    // `OPTION_GUILD_RECRUITMENT_CHANNEL STANDARD|AUTO` line, written at teardown (`0x499a80` at
    // `0x499b0e`: `latch == 1` → `AUTO`, else `STANDARD`), never at set time.
    //
    // **It boots at 1 (AUTO)**, and that is a `.data` initialiser rather than a BSS zero — `raw
    // 0x443608` is `01 00 00 00` — corroborated twice over: `_SetDefaults` calls
    // `SetGuildRecruitmentMode(1)`, and all 33 `chat-cache.txt` files the reference client itself
    // wrote in this repo's install say `AUTO`. See `Model::guild_recruitment_mode`.
    g.set(
        "GetGuildRecruitmentMode",
        // `[0x4a0040, 0x4a0057)` is 23 bytes and one path: `fild dword [0x843608]` (signed i32),
        // `fstp qword [esp]`, `lua_pushnumber`, `mov eax,1`, `ret`. No arguments, no gates — a
        // NUMBER, always, never nil. `UIOptionsFrame_Load` compares it `== 1`, so a nil here would
        // read as a silent "not auto".
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(f64::from(model.guild_recruitment_mode))
        })?,
    )?;
    // ── The `ecx == 1` cascade (decision 2144) ──────────────────────────────────────────
    // `0x49ea70` writes the latch and then tail-jumps into `0x49ea90` **only when the new
    // mode is 1**, and `0x49ea90` is not bookkeeping — it acts, on the wire: a guilded
    // player leaves `GuildRecruitment - City`, an unguilded one in a capital joins it, and
    // otherwise the one-shot `[0xb6e5e4]` is armed for the next autojoin pass. The whole of
    // it lives on the app side (`ui_chat::recruitment`), where the player's guild id, the
    // zone row and the wire are; this binding only raises the ask
    // (`take_guild_recruitment_cascade`). `Set(0)` is the latch alone — `0x49ea70`'s `jne`
    // stops at `0x49ea80`, no packet.
    //
    // The verb itself fires `UPDATE_CHAT_WINDOWS` once (`0x4a00a9`), unconditionally; the
    // cascade's two acting arms fire it again, so an acting `Set(1)` fires it twice.
    //
    // The mirror: `JoinChannelByName("GuildRecruitment")` (`0x49ed3d`) and
    // `LeaveChannelByName` (`0x49ef8f`) each force the latch back to 0 — the join's reset is
    // conditional on the caller's `flag` argument, which the Lua binding passes as 1 and
    // `0x49ea90`'s own internal join passes as 0 (so the cascade does not undo the mode it
    // is acting on). That is `reset_guild_recruitment_mode`, called from the app's drain of
    // both verbs.
    g.set(
        "SetGuildRecruitmentMode",
        // `[0x4a0060, 0x4a00c4)`. One argument at stack index 1, and **two ways to raise** — this
        // raises on a bad argument rather than swallowing a nil, unlike many bindings:
        //
        //   * `lua_isnumber 0x6f34d0` — so a NUMERIC STRING passes (`"1"` works) — else
        //     `luaL_error("Usage: SetGuildRecruitmentMode(mode)")`, which longjmps.
        //   * then `__ftol 0x40a2b0`, **truncating toward zero**, so `1.7` is silently accepted as
        //     1 and `-0.5` as 0.
        //   * then the range gate `0x4a0094 jl` / `0x4a009b jge 2` →
        //     `luaL_error("SetGuildRecruitmentMode: invalid mode")` for `-1`, `2`, `-1.7`.
        //
        // Success returns **0 values** (`xor eax,eax; ret`), not nil. Stock FrameXML can reach
        // neither raise: `UIOptionsFrame_Save` passes `button:GetChecked()` and coerces its nil to
        // 0 itself. An ADDON can, and getting the raise right is the difference between an addon
        // seeing its own bug and seeing ours.
        lua.create_function(|lua, mode: Option<Value>| {
            let n = match mode.as_ref() {
                Some(Value::Integer(i)) => *i as f64,
                Some(Value::Number(n)) => *n,
                // `lua_isnumber` is true for a string luaO_str2d fully consumes.
                Some(Value::String(s)) => s
                    .to_str()
                    .ok()
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .ok_or_else(|| mlua::Error::runtime("Usage: SetGuildRecruitmentMode(mode)"))?,
                _ => return Err(mlua::Error::runtime("Usage: SetGuildRecruitmentMode(mode)")),
            };
            let n = n.trunc();
            if !(0.0..2.0).contains(&n) {
                return Err(mlua::Error::runtime(
                    "SetGuildRecruitmentMode: invalid mode",
                ));
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let mode = n as u8;
            if model.guild_recruitment_mode != mode {
                model.guild_recruitment_mode = mode;
                model.guild_recruitment_changed = true;
            }
            // `0x49ea79 jne` / `0x49ea7b jmp 0x49ea90`: the cascade, on the new value alone.
            if mode == 1 {
                model.guild_recruitment_cascade = true;
            }
            // `0x4a00a4`/`0x4a00a9`: `UPDATE_CHAT_WINDOWS`, on every successful call.
            model
                .pending_events
                .push(("UPDATE_CHAT_WINDOWS".to_string(), Vec::new()));
            Ok(())
        })?,
    )?;

    g.set(
        "GetChannelName",
        lua.create_function(|lua, key: Value| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let slot = match &key {
                // The numeric form. The reference bounds-checks `1 <= n <= count` (`0x49bf30`) and
                // answers only for a CONFIRMED-joined slot; `joined` holds exactly the confirmed
                // ones (`ui_chat::feed` appends on the server's YOU_JOINED, not on the request), so
                // the bound is the whole check here.
                Value::Integer(_) | Value::Number(_) => {
                    let n = match &key {
                        Value::Integer(i) => *i,
                        Value::Number(n) => *n as i64,
                        _ => unreachable!(),
                    };
                    usize::try_from(n)
                        .ok()
                        .filter(|n| name_at(&model, *n).is_some())
                }
                // The name form. A numeric STRING arrives here and must still resolve as a number:
                // `ChatFrame.lua:2113` passes the result of a `gsub` — `GetChannelName("1")` — and
                // Lua's own coercion is what makes that work on the real client.
                Value::String(s) => {
                    let name = s.to_str()?;
                    match name.trim().parse::<usize>() {
                        Ok(n) if name_at(&model, n).is_some() => Some(n),
                        Ok(_) => None,
                        Err(_) => slot_of(&model, &name),
                    }
                }
                _ => None,
            };

            let Some(slot) = slot else {
                // **`0, nil, 0` — three values, not one.** This used to push the number alone,
                // reasoning that `channelName` is unread by every caller on this branch. True of
                // the callers; not true of the client. `0x4a05e0` pushes three on every path, and
                // slot 2 is neither the empty string nor the argument echoed back:
                // `0x4a0659 xor edx,edx` then `lua_pushstring(NULL)`, which tail-jumps to
                // `lua_pushnil`. Decision 1845.
                //
                // "Not joined" is also wider than a bad index: the lookup answers NULL while the
                // join-pending word is non-zero, so a channel already in the list but not yet
                // CONFIRMED reads `0, nil, 0` identically — which is what `joined` models here.
                return Ok(MultiValue::from_vec(vec![
                    Value::Integer(0),
                    Value::Nil,
                    Value::Integer(0),
                ]));
            };
            let name = name_at(&model, slot).unwrap_or_default().to_string();
            Ok(MultiValue::from_vec(vec![
                Value::Integer(slot as i64),
                Value::String(lua.create_string(&name)?),
                // instanceID — see the module doc: 0 on every vanilla emulator, and a number
                // rather than nil so a caller can compare it like the client's.
                Value::Integer(0),
            ]))
        })?,
    )?;

    // GetChannelList() → slot1, name1, slot2, name2, … over every joined channel, in join order.
    //
    // **The shape is settled by two independent consumers, not by a recorded signature** — the
    // reference gives the address (`0x4a02d0`) and no contract:
    //
    //  · the reference's own `FCFDropDown_LoadChannels(...)` walks `for i=1, arg.n, 2` and reads
    //    `arg[i+1]` as the NAME (FloatingChatFrame.lua l.445-455) — so the pair is (slot, name),
    //    in that order, and the caller steps by two;
    //  · `ChatLog.lua:424` packs it with `{ GetChannelList() }` and tests
    //    `type(value) == "number"` to spot an id — so it is a FLAT vararg, never a table.
    //
    // A third witness pins the flatness harder: `AceComm-2.0.lua:334` unpacks TEN pairs in one
    // statement, `local _,a,_,b,…,j = GetChannelList()`.
    //
    // The slot numbering is [`slot_of`]'s — position in `joined_channels` + 1 — so this verb and
    // `GetChannelName` can never disagree about which channel is 3.
    //
    // Zero joined channels is zero returns, not nil: `{ GetChannelList() }` is then an empty
    // table, which is what every caller above already handles.
    //
    // Demand: 4 addons, and only ONE of them names it in its own source (ChatLog). The other
    // three — FuBar_BGQueueNumber, FuBar_MageFu, FuBar_TankPointsFu — reach it through their
    // embedded AceComm-2.0. That gap between "greps for the name" and "wants the name" is why the
    // survey's own read-back exists (`--why`, d2fcef94) and why a hand grep is not the oracle here.
    g.set(
        "GetChannelList",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::with_capacity(model.joined_channels.len() * 2);
            // Occupied slots only — a freed one is a number nothing is on, so it has no pair to
            // contribute (the reference walks its record array and skips the cleared entries).
            for (i, name) in model.joined_channels.iter().enumerate() {
                let Some(name) = name.as_deref() else {
                    continue;
                };
                out.push(Value::Integer(i as i64 + 1));
                out.push(Value::String(lua.create_string(name)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // JoinChannelByName(name [, password [, frameId]]) — `0x49ff00` → `0x49eb70`, the contract
    // decision 1908 carved:
    //
    //   matched DBC row                        → (ChannelID, resolvedName)
    //   custom channel (no row)                → (0, nil)      — and 0 is truthy in Lua
    //   a space in the name, or a matched row  → nil           — no send
    //     whose zone substitution is empty
    //
    // `CMSG_JOIN_CHANNEL` goes out on both non-nil legs. **The third argument registers the
    // channel in that window's own list** (`0x49ec24`–`0x49ede7`):
    // `frameId` is `lua_tonumber`d, truncated and decremented, and anything outside `0..10`
    // after that — a missing or non-numeric argument yields `-1` — silently skips the
    // registration; otherwise `(Shortcut, ChannelID)` for a matched row and `(name, 0)` for a
    // custom one is appended to window `frameId - 1`'s parallel arrays, deduplicated by name
    // (`SStrCmpI`), on both sending legs. It is what puts a `/join`ed channel back into the
    // window after a `/leave` stripped it — the window's `ZONECHANNELS` word is the character's
    // registration for that channel's lines, and without this a leave-then-join-then-relog left
    // General joined and its every line dropped (decision 2144, live run F).
    g.set(
        "JoinChannelByName",
        lua.create_function(
            |lua, (name, password, frame): (Option<String>, Option<String>, Value)| {
                let Some(name) = non_empty(name) else {
                    return Ok(MultiValue::from_vec(vec![Value::Nil]));
                };
                if name.contains(' ') {
                    return Ok(MultiValue::from_vec(vec![Value::Nil]));
                }
                let row = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model
                        .zone_channel_catalog
                        .iter()
                        .find(|r| r.shortcut.eq_ignore_ascii_case(&name))
                        .cloned()
                };
                let (id, resolved, listed_as) = match row {
                    None => (0, None, name.clone()),
                    Some(ZoneChannelRow { resolved: None, .. }) => {
                        return Ok(MultiValue::from_vec(vec![Value::Nil]))
                    }
                    Some(ZoneChannelRow {
                        id,
                        resolved,
                        shortcut,
                        ..
                    }) => (id, resolved, shortcut),
                };
                // `0x49ff6b edi = -1` / `0x49ff8a dec edi`; `0x49ec27 cmp esi,0xa; jae` skips.
                let window = match frame {
                    Value::Integer(n) => Some(n as f64),
                    Value::Number(n) => Some(n),
                    _ => None,
                }
                .map(|n| n.trunc() as i64 - 1)
                .and_then(|i| usize::try_from(i).ok())
                .filter(|i| *i < 10);
                if let Some(i) = window {
                    lua.app_data_mut::<Model>()
                        .expect("model app_data")
                        .register_window_channel(i, listed_as, id);
                }
                push(
                    lua,
                    ChannelCommand::Join {
                        name: resolved.clone().unwrap_or_else(|| name.clone()),
                        password: password.unwrap_or_default(),
                    },
                );
                Ok(MultiValue::from_vec(vec![
                    Value::Integer(i64::from(id)),
                    match resolved {
                        Some(r) => Value::String(lua.create_string(&r)?),
                        None => Value::Nil,
                    },
                ]))
            },
        )?,
    )?;

    // EnumerateServerChannels() — `0x4a1790`: the `ChatChannels.dbc`
    // **shortcuts** in row order, a `flags & 0x10` row only when the zone is a city
    // (`AreaTable.Flags & 0x8`); 0 values while the zone is unresolvable (an empty catalog here).
    // Never the composed `<name> - <zone>`: `FCFDropDown_LoadServerChannels` shows these bare.
    g.set(
        "EnumerateServerChannels",
        lua.create_function(|lua, _ignored: MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::new();
            for row in model.zone_channel_catalog.iter().filter(|r| r.listed) {
                out.push(Value::String(lua.create_string(&row.shortcut)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // LeaveChannelByName(name) — `0x4a0000` → `0x49ee70`, the contract decision 2144 carved. The
    // only stock caller is
    // `SlashCmdList["LEAVE"]`, which passes the first whitespace token of the slash text.
    //
    //   1. a string or number, else it RAISES `Usage: LeaveChannelByName("name")`; 0 returns.
    //   2. `SStrToInt(name) != 0` → the key names joined slot n — resolved on the app side, which
    //      holds the slot states (a suspended or absent slot is a complete no-op there); no
    //      window strip, because a slot name never equals a window entry.
    //   3. else a case-insensitive Shortcut match → the composed name for this zone, or a
    //      complete no-op with no zone text; the window strip is keyed on the DBC's Shortcut.
    //   4. else a custom channel: the argument verbatim, stripped verbatim.
    //
    // **One deliberate divergence, recorded in 2144.** The reference composes the shortcut leg
    // with `GetRealZoneText` ALONE (`0x49efe8`), never the `"City"` row — so in a capital
    // `/leave Trade` sends `"Trade - Stormwind City"`, a channel the player is not in, and
    // leaves nothing. Ours composes with the join's own city-aware substitution (the catalog's
    // `resolved`), so `/leave Trade` leaves `Trade - City`. The mechanism, done right, not the
    // reference's broken form of it.
    //
    // Nothing local is freed here and no event fires: the slot goes when the server's
    // `YOU_LEFT` matches its stored name, and the mask bit with it — both the app's.
    g.set(
        "LeaveChannelByName",
        lua.create_function(|lua, name: Option<Value>| {
            let key = match name {
                Some(Value::String(s)) => s.to_str().map(|s| s.to_string())?,
                Some(Value::Integer(i)) => i.to_string(),
                Some(Value::Number(n)) => format!("{n:.14}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string(),
                _ => return Err(mlua::Error::runtime(r#"Usage: LeaveChannelByName("name")"#)),
            };
            if super::chat_window::leading_int(&key) != 0 {
                push(lua, ChannelCommand::Leave { name: key });
                return Ok(());
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let row = model
                .zone_channel_catalog
                .iter()
                .find(|r| r.shortcut.eq_ignore_ascii_case(&key))
                .cloned();
            let (wire, strip_key) = match row {
                Some(ZoneChannelRow { resolved: None, .. }) => return Ok(()),
                Some(ZoneChannelRow {
                    resolved: Some(r),
                    shortcut,
                    ..
                }) => (r, shortcut),
                None => (key.clone(), key),
            };
            model.strip_window_channel(&strip_key);
            model
                .channel_commands
                .push(ChannelCommand::Leave { name: wire });
            Ok(())
        })?,
    )?;

    // The one-name verbs: each queues its `CMSG_*` and returns nothing.
    for (verb, make) in [
        (
            "ListChannelByName",
            (|name| ChannelCommand::List { name }) as fn(String) -> ChannelCommand,
        ),
        ("DisplayChannelOwner", |name| ChannelCommand::DisplayOwner {
            name,
        }),
        ("ChannelModerate", |name| ChannelCommand::Moderate { name }),
        ("ChannelToggleAnnouncements", |name| {
            ChannelCommand::ToggleAnnouncements { name }
        }),
    ] {
        g.set(
            verb,
            lua.create_function(move |lua, name: Option<String>| {
                if let Some(name) = non_empty(name) {
                    push(lua, make(name));
                }
                Ok(())
            })?,
        )?;
    }

    g.set(
        "ListChannels",
        lua.create_function(|lua, _ignored: MultiValue| {
            push(lua, ChannelCommand::ListAll);
            Ok(())
        })?,
    )?;

    // The two-name verbs: (channel, player) — or (channel, password) for the password.
    for (verb, make) in [
        (
            "SetChannelOwner",
            (|name, player| ChannelCommand::SetOwner { name, player })
                as fn(String, String) -> ChannelCommand,
        ),
        ("SetChannelPassword", |name, password| {
            ChannelCommand::SetPassword { name, password }
        }),
        ("ChannelBan", |name, player| ChannelCommand::Ban {
            name,
            player,
        }),
        ("ChannelInvite", |name, player| ChannelCommand::Invite {
            name,
            player,
        }),
        ("ChannelKick", |name, player| ChannelCommand::Kick {
            name,
            player,
        }),
        ("ChannelModerator", |name, player| {
            ChannelCommand::Moderator { name, player }
        }),
        ("ChannelUnmoderator", |name, player| {
            ChannelCommand::Unmoderator { name, player }
        }),
        ("ChannelMute", |name, player| ChannelCommand::Mute {
            name,
            player,
        }),
        ("ChannelUnmute", |name, player| ChannelCommand::Unmute {
            name,
            player,
        }),
        ("ChannelUnban", |name, player| ChannelCommand::Unban {
            name,
            player,
        }),
    ] {
        g.set(
            verb,
            lua.create_function(
                move |lua, (name, second): (Option<String>, Option<String>)| {
                    if let Some(name) = non_empty(name) {
                        // A password may legitimately be empty (`/password General` clears it); a
                        // player name may not.
                        let second = second.unwrap_or_default();
                        if verb == "SetChannelPassword" || !second.trim().is_empty() {
                            push(lua, make(name, second.trim().to_string()));
                        }
                    }
                    Ok(())
                },
            )?,
        )?;
    }

    Ok(())
}

impl super::UiScript {
    /// Mirror the app's confirmed-joined channel list, in join order, for [`install`]'s verb.
    ///
    /// The `model.party` shape (`party.rs:172`), deliberately, and NOT the `open_chat_requests`
    /// shape the chat-window work used: that one is a QUEUE the app drains (Lua → app), and this is
    /// app state READ BY Lua, which is the opposite direction. `ui_chat::feed` owns both edges that
    /// change it — the server's YOU_JOINED and YOU_LEFT notices — so it pushes here from one place.
    pub fn set_joined_channels(&mut self, joined: Vec<Option<String>>) {
        self.model_mut().joined_channels = joined;
    }
}

#[cfg(test)]
mod command_tests {
    use super::{ChannelCommand, ZoneChannelRow};
    use crate::script::UiScript;

    fn catalog() -> Vec<ZoneChannelRow> {
        vec![
            ZoneChannelRow {
                id: 1,
                shortcut: "General".into(),
                resolved: Some("General - Elwynn Forest".into()),
                listed: true,
            },
            ZoneChannelRow {
                id: 2,
                shortcut: "Trade".into(),
                resolved: None,
                listed: false,
            },
        ]
    }

    /// Decision 1908's three legs, and that 0 is truthy.
    #[test]
    fn join_channel_by_name_answers_the_three_legs_of_1908() {
        let mut s = UiScript::new().unwrap();
        s.set_zone_channel_catalog(catalog());
        s.run(
            "A = {JoinChannelByName('General', 'pw', 1)} \
             B = {JoinChannelByName('MyChan', nil, 1)} \
             C = {JoinChannelByName('Trade', nil, 1)} \
             D = {JoinChannelByName('two words', nil, 1)}",
        )
        .unwrap();
        assert_eq!(
            s.eval::<(i64, String)>("return A[1], A[2]").unwrap(),
            (1, "General - Elwynn Forest".to_string())
        );
        assert!(s
            .eval::<bool>("return B[1] == 0 and B[2] == nil and table.getn(B) == 1")
            .unwrap());
        assert!(
            s.eval::<bool>("return C[1] == nil").unwrap(),
            "empty substitution"
        );
        assert!(s.eval::<bool>("return D[1] == nil").unwrap(), "a space");
        assert_eq!(
            s.take_channel_commands(),
            vec![
                ChannelCommand::Join {
                    name: "General - Elwynn Forest".into(),
                    password: "pw".into()
                },
                ChannelCommand::Join {
                    name: "MyChan".into(),
                    password: String::new()
                },
            ],
            "sent on both non-nil legs, the resolved name for a row"
        );
    }

    #[test]
    fn enumerate_server_channels_lists_the_shortcuts_the_zone_admits() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("EnumerateServerChannels()").unwrap(),
            0,
            "no zone yet — 0 values"
        );
        s.set_zone_channel_catalog(catalog());
        assert_eq!(
            s.eval::<Vec<String>>("return {EnumerateServerChannels()}")
                .unwrap(),
            vec!["General".to_string()],
            "the shortcut, never the composed name; a city row only in a city"
        );
    }

    #[test]
    fn the_management_verbs_queue_in_call_order_and_drop_empty_names() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "LeaveChannelByName('General') ListChannelByName('') ListChannels() \
             SetChannelPassword('General', '') ChannelKick('General', 'Bob') \
             ChannelKick('General', '') ChannelModerate('General') \
             ChannelToggleAnnouncements('General') SetChannelOwner('General', 'Ann')",
        )
        .unwrap();
        assert_eq!(
            s.take_channel_commands(),
            vec![
                ChannelCommand::Leave {
                    name: "General".into()
                },
                ChannelCommand::ListAll,
                ChannelCommand::SetPassword {
                    name: "General".into(),
                    password: String::new()
                },
                ChannelCommand::Kick {
                    name: "General".into(),
                    player: "Bob".into()
                },
                ChannelCommand::Moderate {
                    name: "General".into()
                },
                ChannelCommand::ToggleAnnouncements {
                    name: "General".into()
                },
                ChannelCommand::SetOwner {
                    name: "General".into(),
                    player: "Ann".into()
                },
            ]
        );
    }

    /// **`JoinChannelByName`'s third argument registers the channel in that window**
    /// (`0x49ec24`–`0x49ede7`; decision 2144): `frameId - 1` indexes the ten window records,
    /// `(Shortcut, ChannelID)` for a DBC row and `(name, 0)` for a custom channel, deduplicated by
    /// name; a missing, non-numeric or out-of-range frame skips the registration and nothing else.
    /// It is what puts a `/join`ed channel back into a window a `/leave` had stripped it from.
    #[test]
    fn join_channel_by_name_registers_the_channel_in_the_named_window() {
        let mut s = UiScript::new().unwrap();
        s.set_zone_channel_catalog(catalog());
        s.run(
            "JoinChannelByName('General', nil, 1) \
             JoinChannelByName('general', nil, 1) \
             JoinChannelByName('MyChan', nil, 2) \
             JoinChannelByName('Trade', nil, nil) \
             JoinChannelByName('LocalDefense', nil, 11) \
             JoinChannelByName('WorldDefense', nil, 0)",
        )
        .unwrap();
        let looks = s.chat_window_looks();
        assert_eq!(
            looks[0].channels,
            vec![("General".to_string(), 1)],
            "the DBC Shortcut and its id, once — the second join deduplicated by name"
        );
        assert_eq!(
            looks[1].channels,
            vec![("MyChan".to_string(), 0)],
            "a custom channel: the name and id 0"
        );
        assert!(
            looks[2..].iter().all(|l| l.channels.is_empty()),
            "nil, 11 and 0 register nowhere"
        );
        assert_eq!(
            s.take_chat_window_changes(),
            vec![0, 1],
            "…and both windows are owed a save"
        );
        // The joins themselves went out regardless of the frame argument — five, because this
        // catalog's Trade is unresolvable (the nil leg, asserted above), and that leg registers
        // nothing either.
        assert_eq!(s.take_channel_commands().len(), 5);
    }

    /// `LeaveChannelByName`'s legs (`0x4a0000` → `0x49ee70`; decision 2144): a number passes
    /// through for the app's slot lookup and strips nothing; a shortcut composes for the zone and
    /// strips the window entry it was registered under, in every window; an unresolvable shortcut
    /// is a complete no-op; a custom name goes verbatim and strips verbatim. Nil raises. Zero
    /// returns.
    #[test]
    fn leave_channel_by_name_composes_strips_and_raises_the_way_the_reference_does() {
        let mut s = UiScript::new().unwrap();
        s.set_zone_channel_catalog(catalog());
        s.run(
            "JoinChannelByName('General', nil, 1) JoinChannelByName('MyChan', nil, 1) \
             JoinChannelByName('General', nil, 2)",
        )
        .unwrap();
        s.take_channel_commands();
        s.take_chat_window_changes();

        s.run("LeaveChannelByName('2')").unwrap();
        assert_eq!(
            s.take_channel_commands(),
            vec![ChannelCommand::Leave { name: "2".into() }],
            "a number goes through as typed — the app holds the slot states"
        );
        assert!(
            s.take_chat_window_changes().is_empty(),
            "…and strips no window: a slot name never equals a window entry"
        );

        s.run("LeaveChannelByName('general')").unwrap();
        assert_eq!(
            s.take_channel_commands(),
            vec![ChannelCommand::Leave {
                name: "General - Elwynn Forest".into()
            }],
            "a shortcut composes for the zone"
        );
        let looks = s.chat_window_looks();
        assert_eq!(
            looks[0].channels,
            vec![("MyChan".to_string(), 0)],
            "General stripped from window 1, MyChan kept"
        );
        assert!(
            looks[1].channels.is_empty(),
            "…and from window 2 — all ten are walked"
        );
        assert_eq!(s.take_chat_window_changes(), vec![0, 1]);

        s.run("LeaveChannelByName('Trade')").unwrap();
        assert!(
            s.take_channel_commands().is_empty(),
            "unresolvable here (no city word in this catalog): a complete no-op"
        );

        s.run("LeaveChannelByName('mychan')").unwrap();
        assert_eq!(
            s.take_channel_commands(),
            vec![ChannelCommand::Leave {
                name: "mychan".into()
            }],
            "custom: verbatim"
        );
        assert!(
            s.chat_window_looks()[0].channels.is_empty(),
            "…and stripped verbatim, case-folded"
        );

        let e = s.run("LeaveChannelByName()").unwrap_err();
        assert!(e.to_string().contains("Usage: LeaveChannelByName"), "{e}");
        assert_eq!(s.arity("LeaveChannelByName('x')").unwrap(), 0);
    }
}
