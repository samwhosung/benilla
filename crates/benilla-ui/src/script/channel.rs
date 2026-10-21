//! The chat-channel globals: the joined-channel lookups, the join, leave and management verbs the
//! host applies inside their calls ([`super::ScriptCall::Channel`]), and the guild-recruitment
//! auto-join latch. `GetChannelName` (`0x4a05e0`) looks up by 1-based slot or by name and returns
//! three values: the slot (`CHAT_MSG_*` arg8), the name and `instanceID` (arg10).

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One `ChatChannels.dbc` row for the channel verbs: the id, the Shortcut a typed name is matched
/// against case-folded, the name composed for the player's zone (`None` while the zone text is
/// empty, the verbs' nil leg), whether it is listed (a `flags & 0x10` city row only in a city),
/// and whether a binding's join or leave of it turns the guild-recruitment auto-join off
/// (`flags & 0x20000`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneChannelRow {
    pub id: u32,
    pub shortcut: String,
    pub resolved: Option<String>,
    pub listed: bool,
    pub guild_recruitment: bool,
}

/// What the recruitment cascade `0x49ea90` would do now, as the app resolves it from the player,
/// the zone row and the `GuildRecruitment` row, so `SetGuildRecruitmentMode(1)` runs it in place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RecruitmentPlan {
    /// The zone row or the chat cache is unresolved (`0x49eaf5`), or an unguilded player is
    /// outside a capital (`0x49eb33`): the run stays owed.
    #[default]
    Retry,
    /// No player object (`0x49eac3`): the cascade returns with nothing done and no run owed.
    NoPlayer,
    /// No `GuildRecruitment` row or city word to compose the name from: the reference joins or
    /// leaves whatever its name buffer holds, which is nothing to send here, and still signals
    /// `UPDATE_CHAT_WINDOWS` (`0x49eb21`, `0x49eb5f`).
    Unnamed,
    /// Guilded: leave the composed name (`0x49eb17` → `0x49ee70`).
    Leave(String),
    /// Unguilded in a capital: join the composed name (`0x49eb55` → `0x49eb70`), window 1 taking
    /// the row's Shortcut and id.
    Join {
        name: String,
        shortcut: String,
        id: u32,
    },
}

/// What `0x49ea90` does on the pushed plan, the latch already AUTO: `None` while the run stays
/// owed; else the leave (`0x49eb17` → `0x49ee70`) or the join (`0x49eb55` → `0x49eb70`, whose body
/// first registers window 1's entry, `0x49ec24`) it sends, if any, and whether it then signals
/// `UPDATE_CHAT_WINDOWS` (`0x49eb21`, `0x49eb5f`). Neither passes the binding's reset flag
/// (`0x49eb0e`, `0x49eb47`), so neither turns the auto-join off.
fn cascade_step(model: &mut Model) -> Option<(Option<ChannelCommand>, bool)> {
    Some(match model.guild_recruitment_plan.clone() {
        RecruitmentPlan::Retry => return None,
        RecruitmentPlan::NoPlayer => (None, false),
        RecruitmentPlan::Unnamed => (None, true),
        RecruitmentPlan::Leave(name) => (Some(ChannelCommand::Leave { name }), true),
        RecruitmentPlan::Join { name, shortcut, id } => {
            model.register_window_channel(0, shortcut, id);
            let join = ChannelCommand::Join {
                name,
                password: String::new(),
                id,
            };
            (Some(join), true)
        }
    })
}

/// `0x49ea90` on the pushed plan: its join or leave, which the host applies inside the call, then
/// its `UPDATE_CHAT_WINDOWS`, whose handlers read the slot the join took. Returns whether the run
/// is spent, `false` when it stays owed.
pub(crate) fn run_recruitment_cascade(lua: &Lua) -> mlua::Result<bool> {
    let Some((send, signals)) = super::tick::verb(lua, cascade_step) else {
        return Ok(false);
    };
    if let Some(cmd) = send {
        submit(lua, cmd)?;
    }
    if signals {
        super::tick::verb(lua, |model| model.raise("UPDATE_CHAT_WINDOWS", Vec::new()));
    }
    Ok(true)
}

/// A channel verb past its binding, for the host to apply inside the call
/// ([`super::ScriptCall::Channel`]): the stock `ChatFrame.lua` slash handlers' calls, one variant
/// each, and the recruitment cascade's two.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelCommand {
    /// `JoinChannelByName(name, password)`, sent on both non-nil legs, or the cascade's join; `id`
    /// is the matched row's ChannelID, 0 for a custom channel, which the allocator stores with the
    /// slot (`0x49ede8`).
    Join {
        name: String,
        password: String,
        id: u32,
    },
    /// `LeaveChannelByName(name)`, or the cascade's leave.
    Leave {
        name: String,
    },
    /// `ListChannelByName(name)`: `CMSG_CHANNEL_LIST`.
    List {
        name: String,
    },
    /// `ListChannels()` (`0x4a01c0`): the confirmed slots as one `CHAT_MSG_CHANNEL_LIST` line.
    ListAll,
    /// `DisplayChannelOwner(name)`: `CMSG_CHANNEL_OWNER`.
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
    /// `ChannelModerate(name)`: toggles moderation.
    Moderate {
        name: String,
    },
    /// `ChannelToggleAnnouncements(name)`.
    ToggleAnnouncements {
        name: String,
    },
}

impl super::UiScript {
    /// Feed the zone-channel catalog, whenever the zone, and so every resolved name, changes.
    pub fn set_zone_channel_catalog(&mut self, rows: Vec<ZoneChannelRow>) {
        self.model_mut().zone_channel_catalog = rows;
    }

    /// The channel verbs a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_channel_commands(&mut self) -> Vec<ChannelCommand> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Channel(cmd) => Some(cmd.clone()),
            _ => None,
        })
    }

    /// The guild-recruitment auto-join latch: `0` STANDARD, `1` AUTO.
    pub fn guild_recruitment_mode(&self) -> u8 {
        self.model_ref().guild_recruitment_mode
    }

    /// Seat the latch from the chat cache at login. A host write is not a player gesture, so it
    /// does not arm [`Self::take_guild_recruitment_change`]: a login must not save what it read.
    pub fn set_guild_recruitment_mode(&mut self, mode: u8) {
        let mut model = self.model_mut();
        model.guild_recruitment_mode = mode;
        model.guild_recruitment_changed = false;
    }

    /// Whether Lua moved the latch since the last drain: the chat cache's dirty signal.
    pub fn take_guild_recruitment_change(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().guild_recruitment_changed)
    }

    /// Whether a `SetGuildRecruitmentMode(1)` since the last drain left its cascade owed
    /// (`0x49ea70` → `0x49ea90`, the plan [`RecruitmentPlan::Retry`]).
    pub fn take_guild_recruitment_cascade(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().guild_recruitment_cascade)
    }

    /// What the cascade would do now; the verb runs it in place.
    pub fn set_guild_recruitment_plan(&mut self, plan: RecruitmentPlan) {
        self.model_mut().guild_recruitment_plan = plan;
    }

    /// The host's own run of the cascade (the retry, the guild-id watcher): as the verb's, its
    /// join or leave applied and its `UPDATE_CHAT_WINDOWS` fired before this returns. Returns
    /// whether the run is spent; one whose call the host refused is spent, its error recorded.
    pub fn run_guild_recruitment_cascade(&mut self) -> bool {
        match run_recruitment_cascade(&self.lua) {
            Ok(spent) => spent,
            Err(e) => {
                self.model_mut().record_lua_error(&e);
                true
            }
        }
    }
}

impl Model {
    /// `0x49ea70(0)` from a binding's join or leave of the `GuildRecruitment` row (`0x49ed3d`,
    /// `0x49ef8f`): the latch to 0 and, as a player gesture, the save armed; mode 0 runs no
    /// cascade.
    fn reset_guild_recruitment_mode(&mut self) {
        if self.guild_recruitment_mode != 0 {
            self.guild_recruitment_mode = 0;
            self.guild_recruitment_changed = true;
        }
    }
}

/// Hand `cmd` to the host, which applies it before this returns ([`super::calls::submit`]).
fn submit(lua: &Lua, cmd: ChannelCommand) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Channel(cmd))
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn slot_of(model: &Model, name: &str) -> Option<usize> {
    model
        .joined_channels
        .iter()
        .position(|c| {
            c.as_ref()
                .is_some_and(|c| c.name.eq_ignore_ascii_case(name))
        })
        .map(|i| i + 1)
}

/// Slot `n` (1-based) when the server has confirmed it, the by-number lookups `0x49bf30` and
/// `0x49be50`: below 1 or past the end (`0x49bf34`, `0x49bf3c`), a freed slot, whose number the
/// leave zeroed (`0x49bf52`, `0x49bbd0`), or a slot not in state 0 (`0x49bf5c`-`0x49bf68`) is
/// `None`, so a left channel's number answers "not joined" while every channel above it keeps
/// its own.
pub(crate) fn confirmed_channel(model: &Model, n: i64) -> Option<&JoinedChannel> {
    let i = usize::try_from(n).ok()?.checked_sub(1)?;
    model
        .joined_channels
        .get(i)?
        .as_ref()
        .filter(|c| c.confirmed)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The guild-recruitment auto-join pair ──────────────────────────────────
    // `GetGuildRecruitmentMode` (`0x4a0040`) and `SetGuildRecruitmentMode` (`0x4a0060`) store the
    // Auto-join Guild Recruitment Channel option, which has no cvar: `UIOptionsFrame.lua` binds it
    // by special arms (`:243`, `:328`, `:647`). The state is one int, `[0x843608]`, a `.data`
    // initialiser of 1 written only by `0x49ea70`, and the chat cache saves it at teardown
    // (`0x499a80`), never at set time.
    g.set(
        "GetGuildRecruitmentMode",
        // Always a number, never nil: `UIOptionsFrame_Load` tests it `== 1`.
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(f64::from(model.guild_recruitment_mode))
        })?,
    )?;
    // ── The `ecx == 1` cascade ──────────────────────────────────────────
    // For mode 1 only, `0x49ea70` tail-jumps into `0x49ea90`: a guilded player leaves
    // `GuildRecruitment - City`, an unguilded one in a capital joins it, and an acting cascade
    // fires `UPDATE_CHAT_WINDOWS` before the verb's own. It runs here on the app's plan; the host
    // takes the join's slot and sends the packet inside the call.
    g.set(
        "SetGuildRecruitmentMode",
        // As `0x4a0060`: `lua_isnumber` (`0x6f34d0`, so `"1"` passes), truncation toward zero
        // (`0x40a2b0`), then the 0..2 range gate (`0x4a0094`); success returns zero values.
        lua.create_function(|lua, mode: Option<Value>| {
            let n = match mode.as_ref() {
                Some(Value::Integer(i)) => *i as f64,
                Some(Value::Number(n)) => *n,
                // `lua_isnumber` is true for a string `luaO_str2d` fully consumes.
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
            let mode = n as u8;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                if model.guild_recruitment_mode != mode {
                    model.guild_recruitment_mode = mode;
                    model.guild_recruitment_changed = true;
                }
            }
            // `0x49ea79 jne` / `0x49ea7b jmp 0x49ea90`: the cascade, on the new value alone.
            if mode == 1 && !run_recruitment_cascade(lua)? {
                lua.app_data_mut::<Model>()
                    .expect("model app_data")
                    .guild_recruitment_cascade = true;
            }
            // `0x4a00a4`/`0x4a00a9`: `UPDATE_CHAT_WINDOWS`, on every successful call, in place.
            super::tick::verb(lua, |model| model.raise("UPDATE_CHAT_WINDOWS", Vec::new()));
            Ok(())
        })?,
    )?;

    // GetChannelName(x) (`0x4a05e0`): a number, or a string Lua reads as one (`lua_isnumber`,
    // `0x4a05ed`; `ChatFrame.lua:2113` passes the "1" of `/1`), selects a confirmed slot after
    // `__ftol` (`0x4a0602` → `0x49bf30`); any other string selects the first slot of that name in
    // any state (`0x4a0627` → `0x49be90`), and anything else none (`0x4a0693`). Three values on
    // every path: the number, the name or nil (`lua_pushstring(NULL)`, `0x4a0659`), the instance.
    g.set(
        "GetChannelName",
        lua.create_function(|lua, key: Value| {
            let number = super::binding_abi::is_number(lua, key.clone());
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let found = match (number, &key) {
                (Some(n), _) => {
                    let n = i64::from(super::binding_abi::ftol(n));
                    confirmed_channel(&model, n).map(|c| (n, c.name.clone()))
                }
                (None, Value::String(s)) => {
                    let name = s.to_str()?;
                    slot_of(&model, &name).and_then(|n| {
                        let slot = model.joined_channels[n - 1].as_ref()?;
                        Some((n as i64, slot.name.clone()))
                    })
                }
                _ => None,
            };
            let Some((n, name)) = found else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Integer(0),
                    Value::Nil,
                    Value::Integer(0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(n),
                Value::String(lua.create_string(&name)?),
                // `instanceID`: `YOU_JOINED`'s split index, 0 on vanilla servers; never nil.
                Value::Integer(0),
            ]))
        })?,
    )?;

    // GetChannelList() (`0x4a02d0`): a flat vararg of number, name pairs over the slots in state 0
    // (`0x4a02f8`-`0x4a030b`), as `FCFDropDown_LoadChannels` steps it (`FloatingChatFrame.lua:445`);
    // a slot whose ChannelID names a row answers that row's Shortcut (`0x4a0323`-`0x4a034f`), any
    // other its own name (`0x4a0351`). None confirmed is zero returns.
    g.set(
        "GetChannelList",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::with_capacity(model.joined_channels.len() * 2);
            for (i, slot) in model.joined_channels.iter().enumerate() {
                let Some(slot) = slot.as_ref().filter(|c| c.confirmed) else {
                    continue;
                };
                let name = model
                    .zone_channel_catalog
                    .iter()
                    .find(|r| slot.id != 0 && r.id == slot.id)
                    .map_or(slot.name.as_str(), |r| r.shortcut.as_str());
                out.push(Value::Integer(i as i64 + 1));
                out.push(Value::String(lua.create_string(name)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // JoinChannelByName(name [, password [, frameId]]) (`0x49ff00` → `0x49eb70`): `name` is a
    // string or number (anything else raises `Usage: JoinChannelByName("name")`, `0x49ffe7`), the
    // password a string or number, else empty. A DBC row answers `(ChannelID, Shortcut)`
    // (`0x49ecec`-`0x49ecfa`, `0x49ffbc`-`0x49ffc5`) and a custom channel `(0, nil)`, two values
    // either way (`0x49ffcc`, `0x49ffdd`), both sending; a name with a space (`0x49ff5e`), a row
    // whose zone text is empty, or a failed join (`0x49ffaa`) pushes nothing and sends nothing. A
    // `frameId` (a number or numeric string) that truncates to 1..=10 registers the channel in that
    // window's list, deduplicated by name (`0x49ec24`), which is what brings a `/leave`-stripped
    // channel's lines back to the window; then the host takes the join's slot and sends it inside
    // the call (`0x49ede8`-`0x49ee42`).
    g.set(
        "JoinChannelByName",
        lua.create_function(
            |lua, (name, password, frame): (Option<Value>, Option<Value>, Option<Value>)| {
                let name = super::binding_abi::string_arg(
                    lua,
                    name.unwrap_or(Value::Nil),
                    r#"Usage: JoinChannelByName("name")"#,
                )?;
                let password = password
                    .as_ref()
                    .and_then(|v| super::binding_abi::optional_string(lua, v))
                    .unwrap_or_default();
                if name.is_empty() || name.contains(' ') {
                    return Ok(MultiValue::new());
                }
                let row = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model
                        .zone_channel_catalog
                        .iter()
                        .find(|r| r.shortcut.eq_ignore_ascii_case(&name))
                        .cloned()
                };
                let (id, wire, shortcut, guild_recruitment) = match row {
                    None => (0, name.clone(), None, false),
                    Some(ZoneChannelRow { resolved: None, .. }) => return Ok(MultiValue::new()),
                    Some(ZoneChannelRow {
                        id,
                        resolved: Some(resolved),
                        shortcut,
                        guild_recruitment,
                        ..
                    }) => (id, resolved, Some(shortcut), guild_recruitment),
                };
                // `0x49ff6b edi = -1` / `0x49ff8a dec edi`; `0x49ec27 cmp esi,0xa; jae` skips.
                let window = match frame {
                    Some(v) => lua.coerce_number(v)?,
                    None => None,
                }
                .map(|n| super::binding_abi::ftol(n).wrapping_sub(1))
                .and_then(|i| usize::try_from(i).ok())
                .filter(|i| *i < 10);
                {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    if let Some(i) = window {
                        let entry = shortcut.clone().unwrap_or_else(|| name.clone());
                        model.register_window_channel(i, entry, id);
                    }
                    // The row the Shortcut matched turns the auto-join off (`0x49ed19`-`0x49ed3d`),
                    // before the slot and the packet.
                    if guild_recruitment {
                        model.reset_guild_recruitment_mode();
                    }
                }
                submit(
                    lua,
                    ChannelCommand::Join {
                        name: wire,
                        password,
                        id,
                    },
                )?;
                Ok(MultiValue::from_vec(vec![
                    Value::Integer(i64::from(id)),
                    match shortcut {
                        Some(r) => Value::String(lua.create_string(&r)?),
                        None => Value::Nil,
                    },
                ]))
            },
        )?,
    )?;

    // EnumerateServerChannels() (`0x4a1790`): the `ChatChannels.dbc` Shortcuts in row order, never
    // the composed names, a `flags & 0x10` row only in a city (`AreaTable.Flags & 0x8`); zero
    // values while the zone is unresolved.
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

    // LeaveChannelByName(name) (`0x4a0000` → `0x49ee70`) returns nothing and raises `Usage:` on a
    // non-string, non-number. A key whose `SStrToInt` is non-zero is a joined slot, resolved
    // app-side and never a window entry; a Shortcut leaves the name composed for this zone (a
    // no-op with no zone text) and strips by Shortcut; anything else is a custom channel. The key,
    // a number read as its confirmed slot's name (`0x49e880`), turns the auto-join off when it is
    // the `GuildRecruitment` row's composed name (`0x49eeec`-`0x49ef8f`, `SStrCmpI` at
    // `0x49ef84`), so `/leave GuildRecruitment` keeps it on.
    // Deviation: the reference composes the Shortcut with `GetRealZoneText` alone (`0x49efe8`), so
    // in a capital `/leave Trade` sends `Trade - Stormwind City` and leaves nothing; ours uses the
    // join's city-aware name, because the reference's form cannot leave the channel it joined.
    // The slot is freed app-side when the server's `YOU_LEFT` arrives.
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
            let number = super::binding_abi::s_str_to_int(&key);
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                // A number naming no confirmed slot names nothing (`0x49e880`), and the app ends
                // the leave there (`0x49ee8a`).
                let named = match number {
                    0 => Some(key.clone()),
                    n => confirmed_channel(&model, i64::from(n)).map(|c| c.name.clone()),
                };
                let resets = named.is_some_and(|named| {
                    model.zone_channel_catalog.iter().any(|r| {
                        r.guild_recruitment
                            && r.resolved
                                .as_deref()
                                .is_some_and(|composed| composed.eq_ignore_ascii_case(&named))
                    })
                });
                if resets {
                    model.reset_guild_recruitment_mode();
                }
            }
            if number != 0 {
                return submit(lua, ChannelCommand::Leave { name: key });
            }
            let wire = {
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
                wire
            };
            submit(lua, ChannelCommand::Leave { name: wire })
        })?,
    )?;

    // The one-name verbs: each hands the host its `CMSG_*` and returns nothing.
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
            lua.create_function(move |lua, name: Option<String>| match non_empty(name) {
                Some(name) => submit(lua, make(name)),
                None => Ok(()),
            })?,
        )?;
    }

    g.set(
        "ListChannels",
        lua.create_function(|lua, _ignored: MultiValue| submit(lua, ChannelCommand::ListAll))?,
    )?;

    // The two-name verbs: (channel, player), or (channel, password) for the password.
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
                        // A password may be empty (`/password General` clears it); a player name
                        // may not.
                        let second = second.unwrap_or_default();
                        if verb == "SetChannelPassword" || !second.trim().is_empty() {
                            return submit(lua, make(name, second.trim().to_string()));
                        }
                    }
                    Ok(())
                },
            )?,
        )?;
    }

    Ok(())
}

/// A joined channel's slot, as the VM mirrors the app's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinedChannel {
    pub name: String,
    /// `+0x94`, the slot's `ChatChannels.dbc` ChannelID, 0 for a custom channel.
    pub id: u32,
    /// The slot is in state 0 (`+0x9c`), which the by-number lookups `0x49be50` and `0x49bf30`
    /// require: not renamed with its re-join pending, not suspended.
    pub confirmed: bool,
}

impl From<&str> for JoinedChannel {
    /// A confirmed custom slot of that name.
    fn from(name: &str) -> Self {
        JoinedChannel {
            name: name.to_string(),
            id: 0,
            confirmed: true,
        }
    }
}

impl super::UiScript {
    /// Mirror the app's joined channels in join order, `None` for a freed slot; the app pushes it
    /// after every step that writes a slot's name or state.
    pub fn set_joined_channels(&mut self, joined: Vec<Option<JoinedChannel>>) {
        self.model_mut().joined_channels = joined;
    }

    /// `SendChatMessage`'s channel target (`0x49f4d9`-`0x49f4ea`): `SStrToInt` (`0x64ac60`)
    /// straight into `0x49be50`, with no name leg, so only a number naming a confirmed slot
    /// answers, with that slot's name; anything else, a name included, is `None`.
    pub fn channel_send_target(&self, target: &str) -> Option<String> {
        let n = super::binding_abi::s_str_to_int(target);
        confirmed_channel(&self.model_ref(), i64::from(n)).map(|c| c.name.clone())
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
                guild_recruitment: false,
            },
            ZoneChannelRow {
                id: 2,
                shortcut: "Trade".into(),
                resolved: None,
                listed: false,
                guild_recruitment: false,
            },
        ]
    }

    /// The row, custom-channel and nil legs, and that 0 is truthy.
    #[test]
    fn join_channel_by_name_answers_the_row_custom_and_nil_legs() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
            (1, "General".to_string()),
            "the row's Shortcut"
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
                    password: "pw".into(),
                    id: 1,
                },
                ChannelCommand::Join {
                    name: "MyChan".into(),
                    password: String::new(),
                    id: 0,
                },
            ],
            "sent on both non-nil legs, the resolved name for a row"
        );
    }

    #[test]
    fn enumerate_server_channels_lists_the_shortcuts_the_zone_admits() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        s.keep_calls();
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
                    name: "General".into(),
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

    /// `frameId - 1` indexes the ten window records (`0x49ec24`); a missing, non-numeric or
    /// out-of-range frame skips the registration and nothing else.
    #[test]
    fn join_channel_by_name_registers_the_channel_in_the_named_window() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        // The joins went out whatever the frame argument: five, as this catalog's Trade is the
        // nil leg, which registers nothing either.
        assert_eq!(s.take_channel_commands().len(), 5);
    }

    /// `LeaveChannelByName`'s four legs and its raise (`0x4a0000` → `0x49ee70`); a Shortcut strips
    /// its entry from every window.
    #[test]
    fn leave_channel_by_name_composes_strips_and_raises_the_way_the_reference_does() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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

        // `SStrToInt` takes no `+`: "+2" is a channel name, so its window entry is stripped, which
        // the numeric leg (a slot, never a window entry) does not do.
        s.run("AddChatWindowChannel(1, '+2')").unwrap();
        s.take_channel_commands();
        s.take_chat_window_changes();
        s.run("LeaveChannelByName('+2')").unwrap();
        assert!(
            !s.chat_window_looks()[0]
                .channels
                .iter()
                .any(|(c, _)| c == "+2"),
            "a '+'-prefixed key is a name"
        );
        s.take_channel_commands();
        s.take_chat_window_changes();

        s.run("LeaveChannelByName('general')").unwrap();
        assert_eq!(
            s.take_channel_commands(),
            vec![ChannelCommand::Leave {
                name: "General - Elwynn Forest".into(),
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
                name: "mychan".into(),
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
