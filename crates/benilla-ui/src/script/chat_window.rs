//! The chat window settings bindings: `GetChatWindowInfo`, its setters and `ChatFrame_OpenChat`.
//!
//! The reference keeps one record per window (`0xb4fe50`, stride 0x98, 10 slots), loaded from and
//! saved to `chat-cache.txt`: name `+0x00`, message-group flags `+0x20`, channel names and ids
//! `+0x64`/`+0x74`, font size `i32` `+0x84`, the colour one packed `CImVector` in BGRA order
//! (`+0x88` B, `+0x89` G, `+0x8a` R, `+0x8b` A), locked `+0x8c`, docked `+0x90`, shown `+0x94`.
//! `GetChatWindowInfo(id)` (`0x4a0ba0`) answers `name, fontSize, r, g, b, a, shown, locked,
//! docked`, each colour byte times the f32 1/255 at `0x8026c8`.
//!
//! Three traps in that tuple. `name` is `""` until a window is named, and FrameXML's
//! `FCF_SetWindowName` (FloatingChatFrame.lua:681) substitutes "General" and "Combat Log".
//! `shown`, `locked` and `docked` are a number or nil, never 0, because FrameXML tests them bare
//! (FloatingChatFrame.lua:59, 69). `docked` is a dock position, the index `FCF_DockFrame` takes.
//!
//! `SetChatWindowColor` (`0x4a14f0`) and `SetChatWindowAlpha` (`0x4a15d0`) store
//! `__ftol(x · 255.0)` (`[0x806498]` is the 255.0; `__ftol` `0x40a2b0` truncates), so a round trip
//! quantises: alpha 0.4 reads back 102/255, 0.5 reads 127/255. Deviation: a value outside 0..1
//! clamps, where the reference keeps the low byte (alpha 2.0 stores 254, -1.0 stores 1), because no
//! FrameXML or addon caller passes one.

use mlua::{Lua, MultiValue, Value};

use super::channel::ZoneChannelRow;
use super::Model;

/// The reference's record slots (`0xb4fe50`); FrameXML builds only [`NUM_CHAT_WINDOWS`] frames.
pub(super) const ENGINE_CHAT_WINDOW_SLOTS: usize = 10;

/// `NUM_CHAT_WINDOWS = 7` (ChatFrame.lua:5), the frames `ChatFrame1`..`ChatFrame7`.
pub(super) const NUM_CHAT_WINDOWS: usize = 7;

/// `CHATMSGGROUP`, the client's 68-entry message-group table (`0x805fb0`, stride 0xc):
/// `(name, defaultOn, addedVersion)` in the order of the record's `+0x20` flags. Not the 94-entry
/// colour table: `CREATURE` is only here, and 27 colour types have no group. The boot init
/// (`0x4982c0`) enables the `defaultOn` groups in window 2; the loader (`0x498a60`) back-fills by
/// `addedVersion` for a file older than `ADDEDVERSION 2`.
pub const MESSAGE_GROUPS: [(&str, bool, u8); 68] = [
    ("SYSTEM", true, 0),
    ("SAY", true, 0),
    ("YELL", true, 0),
    ("WHISPER", true, 0),
    ("PARTY", true, 0),
    ("GUILD", true, 0),
    ("CREATURE", true, 0),
    ("CHANNEL", true, 0),
    ("SKILL", true, 0),
    ("LOOT", true, 0),
    ("COMBAT_MISC_INFO", true, 0),
    ("COMBAT_SELF_HITS", true, 0),
    ("COMBAT_SELF_MISSES", true, 0),
    ("COMBAT_PET_HITS", true, 0),
    ("COMBAT_PET_MISSES", true, 0),
    ("COMBAT_PARTY_HITS", false, 0),
    ("COMBAT_PARTY_MISSES", false, 0),
    ("COMBAT_FRIENDLYPLAYER_HITS", false, 0),
    ("COMBAT_FRIENDLYPLAYER_MISSES", false, 0),
    ("COMBAT_HOSTILEPLAYER_HITS", true, 0),
    ("COMBAT_HOSTILEPLAYER_MISSES", true, 0),
    ("COMBAT_CREATURE_VS_SELF_HITS", true, 0),
    ("COMBAT_CREATURE_VS_SELF_MISSES", true, 0),
    ("COMBAT_CREATURE_VS_PARTY_HITS", false, 0),
    ("COMBAT_CREATURE_VS_PARTY_MISSES", false, 0),
    ("COMBAT_CREATURE_VS_CREATURE_HITS", false, 0),
    ("COMBAT_CREATURE_VS_CREATURE_MISSES", false, 0),
    ("COMBAT_FRIENDLY_DEATH", true, 0),
    ("COMBAT_HOSTILE_DEATH", true, 0),
    ("COMBAT_XP_GAIN", true, 0),
    ("SPELL_SELF_DAMAGE", true, 0),
    ("SPELL_SELF_BUFF", true, 0),
    ("SPELL_PET_DAMAGE", true, 0),
    ("SPELL_PET_BUFF", true, 0),
    ("SPELL_PARTY_DAMAGE", false, 0),
    ("SPELL_PARTY_BUFF", false, 0),
    ("SPELL_FRIENDLYPLAYER_DAMAGE", false, 0),
    ("SPELL_FRIENDLYPLAYER_BUFF", false, 0),
    ("SPELL_HOSTILEPLAYER_DAMAGE", true, 0),
    ("SPELL_HOSTILEPLAYER_BUFF", true, 0),
    ("SPELL_CREATURE_VS_SELF_DAMAGE", true, 0),
    ("SPELL_CREATURE_VS_SELF_BUFF", true, 0),
    ("SPELL_CREATURE_VS_PARTY_DAMAGE", false, 0),
    ("SPELL_CREATURE_VS_PARTY_BUFF", false, 0),
    ("SPELL_CREATURE_VS_CREATURE_DAMAGE", false, 0),
    ("SPELL_CREATURE_VS_CREATURE_BUFF", false, 0),
    ("SPELL_TRADESKILLS", true, 0),
    ("SPELL_DAMAGESHIELDS_ON_SELF", true, 0),
    ("SPELL_DAMAGESHIELDS_ON_OTHERS", false, 0),
    ("SPELL_AURA_GONE_SELF", true, 0),
    ("SPELL_AURA_GONE_PARTY", false, 0),
    ("SPELL_AURA_GONE_OTHER", false, 0),
    ("SPELL_ITEM_ENCHANTMENTS", true, 0),
    ("SPELL_BREAK_AURA", true, 0),
    ("SPELL_PERIODIC_SELF_DAMAGE", true, 0),
    ("SPELL_PERIODIC_SELF_BUFFS", true, 0),
    ("SPELL_PERIODIC_PARTY_DAMAGE", false, 0),
    ("SPELL_PERIODIC_PARTY_BUFFS", false, 0),
    ("SPELL_PERIODIC_FRIENDLYPLAYER_DAMAGE", false, 0),
    ("SPELL_PERIODIC_FRIENDLYPLAYER_BUFFS", false, 0),
    ("SPELL_PERIODIC_HOSTILEPLAYER_DAMAGE", true, 0),
    ("SPELL_PERIODIC_HOSTILEPLAYER_BUFFS", true, 0),
    ("SPELL_PERIODIC_CREATURE_DAMAGE", true, 0),
    ("SPELL_PERIODIC_CREATURE_BUFFS", true, 0),
    ("SPELL_FAILED_LOCALPLAYER", false, 0),
    ("COMBAT_HONOR_GAIN", true, 0),
    ("COMBAT_FACTION_CHANGE", true, 1),
    ("MONEY", true, 2),
];

/// The flag index of a group name, case-folded as the loader's `SStrCmpI` matches it.
pub fn message_group_index(name: &str) -> Option<usize> {
    MESSAGE_GROUPS
        .iter()
        .position(|(n, _, _)| n.eq_ignore_ascii_case(name))
}

/// One window's record as `chat-cache.txt` persists it, in the reference's own field types.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChatWindowLook {
    /// Background tint, one byte per channel.
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// Background alpha byte; stock 0, which the FrameXML hover fade lifts.
    pub a: u8,
    /// `SIZE`: the font height, or 0 for the font's own; the setter never stores `<= 0`.
    pub font_size: i32,
    /// `LOCKED`, stock true for every window; the tab menu's `FCF_ToggleLock` moves it.
    pub locked: bool,
    /// `DOCKED`: the 1-based dock position `FCF_DockFrame(frame, docked)` takes, `None` undocked.
    pub docked: Option<u8>,
    /// `+0x00`: `""` until `SetChatWindowName` stores one; `NAME` is written only when non-empty.
    pub name: String,
    /// `+0x94`, `SHOWN`.
    pub shown: bool,
    /// `+0x20`: the enabled message-group names, the `MESSAGES … END` block.
    pub messages: Vec<String>,
    /// `+0x64`/`+0x74`: `(name, zone channel id)` pairs, the `CHANNELS … END` block.
    pub channels: Vec<(String, u32)>,
}

impl Default for ChatWindowLook {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl ChatWindowLook {
    /// The boot record of windows 3 to 10 (`0x4982c0`), which [`Self::stock`] builds 1 and 2 on.
    pub(super) const DEFAULT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
        font_size: 0,
        locked: true,
        docked: None,
        name: String::new(),
        shown: false,
        messages: Vec::new(),
        channels: Vec::new(),
    };

    /// Window `index`'s (0-based) record at boot (`0x4982c0`), what a client with no
    /// `chat-cache.txt` runs from: window 1 shown and undocked (FrameXML docks it,
    /// FloatingChatFrame.xml:835) with groups 1-10; window 2 shown, docked at 1, with the
    /// `defaultOn` groups of 11-68; the rest hidden with no groups.
    pub fn stock(index: usize) -> Self {
        let groups = |range: std::ops::Range<usize>, all: bool| -> Vec<String> {
            MESSAGE_GROUPS[range]
                .iter()
                .filter(|(_, on, _)| all || *on)
                .map(|(n, _, _)| (*n).to_string())
                .collect()
        };
        match index {
            0 => Self {
                shown: true,
                messages: groups(0..10, true),
                ..Self::DEFAULT
            },
            1 => Self {
                shown: true,
                docked: Some(1),
                messages: groups(10..68, false),
                ..Self::DEFAULT
            },
            _ => Self::DEFAULT,
        }
    }

    /// Drops unknown names and duplicates and puts the rest in table order, so
    /// `GetChatWindowMessages` (`0x4a0d20`) answers as the reference's flag walk does.
    pub fn normalize_messages(&mut self) {
        let mut flags = [false; MESSAGE_GROUPS.len()];
        for m in &self.messages {
            if let Some(i) = message_group_index(m) {
                flags[i] = true;
            }
        }
        self.messages = MESSAGE_GROUPS
            .iter()
            .zip(flags)
            .filter(|(_, on)| *on)
            .map(|((n, _, _), _)| (*n).to_string())
            .collect();
    }
}

/// The leading decimal integer after optional whitespace and a sign, 0 when there is none. The
/// reference's `SStrToInt` (`0x64ac60`) takes neither the whitespace nor a `+`.
pub(super) fn leading_int(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let n: i64 = digits
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .fold(0i64, |acc, c| {
            acc.saturating_mul(10)
                .saturating_add(i64::from(c as u8 - b'0'))
        });
    if neg {
        -n
    } else {
        n
    }
}

/// The byte `__ftol(x · 255.0)` stores, clamped (the module's deviation); non-finite stores 0.
fn to_byte(x: f64) -> u8 {
    if !x.is_finite() {
        return 0;
    }
    (x * 255.0).trunc().clamp(0.0, 255.0) as u8
}

fn from_byte(b: u8) -> f64 {
    f64::from(b) / 255.0
}

/// The 0-based index of a Lua window id. Deviation: an id outside the seven built windows raises,
/// where the reference serves ten records and silently ignores any other id (`0x4a0be1`), because
/// benilla keeps only the seven and every caller loops `1, NUM_CHAT_WINDOWS`.
fn window_index(id: i64) -> mlua::Result<usize> {
    if id < 1 || id as usize > NUM_CHAT_WINDOWS {
        return Err(mlua::Error::runtime(format!(
            "chat window {id} out of range — benilla builds ChatFrame1..ChatFrame{NUM_CHAT_WINDOWS} \
             (the client's settings array holds {ENGINE_CHAT_WINDOW_SLOTS} slots, but only \
             {NUM_CHAT_WINDOWS} have frames)"
        )));
    }
    Ok(id as usize - 1)
}

impl super::Model {
    /// Appends `(name, id)` to window `window`'s channels unless the name is there (case-folded):
    /// the shared tail of `JoinChannelByName` (`0x49ec24`-`0x49ede7`) and `AddChatWindowChannel`
    /// (`0x4a1000`). `name` is the DBC Shortcut for a matched row, else the argument verbatim.
    pub(super) fn register_window_channel(&mut self, window: usize, name: String, id: u32) -> bool {
        let Some(look) = self.chat_window_looks.get_mut(window) else {
            return false;
        };
        if look
            .channels
            .iter()
            .any(|(c, _)| c.eq_ignore_ascii_case(&name))
        {
            return false;
        }
        look.channels.push((name, id));
        self.chat_window_changes.insert(window);
        true
    }

    /// Removes the first entry named `key` (case-folded) from every window, as leave-by-name
    /// `0x49ee70` does (`0x49f001`-`0x49f085`). `key` is the DBC Shortcut for a matched row, else
    /// the argument; a numeric leave strips nothing, as a slot name never equals a window entry.
    pub(super) fn strip_window_channel(&mut self, key: &str) -> bool {
        let mut stripped = false;
        for (i, look) in self.chat_window_looks.iter_mut().enumerate() {
            if let Some(at) = look
                .channels
                .iter()
                .position(|(c, _)| c.eq_ignore_ascii_case(key))
            {
                look.channels.remove(at);
                self.chat_window_changes.insert(i);
                stripped = true;
            }
        }
        stripped
    }
}

impl super::UiScript {
    /// Host-side [`Model::register_window_channel`], for the guild-recruitment join, which
    /// registers `GuildRecruitment` in window 1 as `0x49eb70(frameIdx = 0)` does.
    pub fn register_chat_window_channel(&mut self, window: usize, name: &str, id: u32) -> bool {
        self.model_mut()
            .register_window_channel(window, name.to_string(), id)
    }

    /// Seeds the records from the host's persisted store. A load queues no change: an echo would
    /// re-dirty the file just read.
    pub fn set_chat_window_looks(
        &mut self,
        looks: impl IntoIterator<Item = (usize, ChatWindowLook)>,
    ) {
        let mut model = self.model_mut();
        for (i, look) in looks {
            if let Some(slot) = model.chat_window_looks.get_mut(i) {
                *slot = look;
                slot.normalize_messages();
            }
        }
    }

    /// Every window's record, index 0 being `ChatFrame1`, for the saver.
    pub fn chat_window_looks(&self) -> Vec<ChatWindowLook> {
        self.model_mut().chat_window_looks.to_vec()
    }

    /// Drains the 0-based windows Lua changed since the last call, deduplicated and ascending.
    pub fn take_chat_window_changes(&mut self) -> Vec<usize> {
        let mut v: Vec<usize> = std::mem::take(&mut self.model_mut().chat_window_changes)
            .into_iter()
            .collect();
        v.sort_unstable();
        v
    }
}

impl super::UiScript {
    /// Drains the texts `ChatFrame_OpenChat` queued since the last call.
    pub fn take_open_chat_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().open_chat_requests)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // GetChatWindowInfo(id) → name, fontSize, r, g, b, a, shown, locked, docked.
    lua.globals().set(
        "GetChatWindowInfo",
        lua.create_function(|lua, id: i64| {
            let i = window_index(id)?;
            let look = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .chat_window_looks[i]
                .clone();
            let shown = if look.shown { Some(1) } else { None };
            let docked = look.docked.map(i64::from);
            let num = |v: Option<i64>| match v {
                Some(n) => Value::Integer(n),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&look.name)?),
                Value::Integer(i64::from(look.font_size)),
                Value::Number(from_byte(look.r)),
                Value::Number(from_byte(look.g)),
                Value::Number(from_byte(look.b)),
                Value::Number(from_byte(look.a)),
                num(shown), // 1 or nil, never 0
                // locked: 1 or nil, never 0
                if look.locked {
                    Value::Integer(1)
                } else {
                    Value::Nil
                },
                num(docked), // the dock position or nil
            ]))
        })?,
    )?;

    // SetChatWindowColor(id, r, g, b) (`0x4a14f0`) only stores: `FCF_SetWindowColor`
    // (FloatingChatFrame.lua:699) tints the textures itself.
    lua.globals().set(
        "SetChatWindowColor",
        lua.create_function(|lua, (id, r, g, b): (i64, f64, f64, f64)| {
            let i = window_index(id)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let look = &mut model.chat_window_looks[i];
            let next = ChatWindowLook {
                r: to_byte(r),
                g: to_byte(g),
                b: to_byte(b),
                ..look.clone()
            };
            if next != *look {
                *look = next;
                model.chat_window_changes.insert(i);
            }
            Ok(())
        })?,
    )?;

    // SetChatWindowAlpha(id, alpha) (`0x4a15d0`), called on every opacity-slider step.
    lua.globals().set(
        "SetChatWindowAlpha",
        lua.create_function(|lua, (id, alpha): (i64, f64)| {
            let i = window_index(id)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let look = &mut model.chat_window_looks[i];
            let a = to_byte(alpha);
            if a != look.a {
                look.a = a;
                model.chat_window_changes.insert(i);
            }
            Ok(())
        })?,
    )?;

    // SetChatWindowLocked(id, isLocked) (`0x4a1650`): the record's `+0x8c` (`0x4a16a2`), 1 at
    // boot (`0x4984e4`). A `bool` holds it, as the cache writer stores it through `setne`
    // (`0x499e8b`). The argument is Lua truthiness, where the reference coerces through `0x6f1c10`
    // (default 0): `0` and "off" unlock there and lock here. FrameXML passes only 1 and nil.
    lua.globals().set(
        "SetChatWindowLocked",
        lua.create_function(|lua, (id, locked): (i64, bool)| {
            let i = window_index(id)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let look = &mut model.chat_window_looks[i];
            if locked != look.locked {
                look.locked = locked;
                model.chat_window_changes.insert(i);
            }
            Ok(())
        })?,
    )?;
    // SetChatWindowDocked(id, position) (`0x4a16b0`): `FCF_SaveDock` (FloatingChatFrame.lua:1282)
    // writes each docked window's 1-based position, `FCF_UnDockFrame` nil. Nil or a position
    // outside 1..=255 clears it here; the reference stores a number truncated, a non-number as 0.
    lua.globals().set(
        "SetChatWindowDocked",
        lua.create_function(|lua, (id, position): (i64, Option<i64>)| {
            let i = window_index(id)?;
            let position = position
                .filter(|p| *p > 0)
                .and_then(|p| u8::try_from(p).ok());
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let look = &mut model.chat_window_looks[i];
            if position != look.docked {
                look.docked = position;
                model.chat_window_changes.insert(i);
            }
            Ok(())
        })?,
    )?;
    // SetChatWindowSize(id, fontSize) (`0x4a1470`) only stores; FrameXML sets the font itself.
    lua.globals().set(
        "SetChatWindowSize",
        lua.create_function(|lua, (id, size): (i64, f64)| {
            let i = window_index(id)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let look = &mut model.chat_window_looks[i];
            // A size `<= 0` after truncation is dropped (`0x4a14bc jle`). Deviation: one past
            // `i32::MAX` clamps where the reference keeps the low 32 bits, for the colours' reason.
            let s = if size.is_finite() && size >= 1.0 {
                size.trunc().min(f64::from(i32::MAX)) as i32
            } else {
                return Ok(());
            };
            if s != look.font_size {
                look.font_size = s;
                model.chat_window_changes.insert(i);
            }
            Ok(())
        })?,
    )?;

    // ChatFrame_OpenChat(text, chatFrame) queues `text` for the host to open the chat box with. The
    // frame is inert: in 1.12 every chat frame's `editBox` is the one `ChatFrameEditBox`
    // (FloatingChatFrame.lua:30, FloatingChatFrame.xml:742). The stock ChatFrame.lua:1545 defines
    // the same global and replaces this one when it loads.
    lua.globals().set(
        "ChatFrame_OpenChat",
        lua.create_function(|lua, (text, _chat_frame): (Option<String>, Value)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.open_chat_requests.push(text.unwrap_or_default());
            Ok(())
        })?,
    )?;

    // ── The rest of the record: name, shown, message groups, channels ──────────────────────
    //
    // `SetChatWindowName` `0x4a13f0`, `SetChatWindowShown` `0x4a1730`,
    // `Add/Remove/GetChatWindowMessages` `0x4a0e80`/`0x4a0f40`/`0x4a0d20` and
    // `Add/Remove/GetChatWindowChannel(s)` `0x4a1000`/`0x4a1260`/`0x4a0dc0`.

    fn with_look<T>(
        lua: &Lua,
        id: i64,
        f: impl FnOnce(&mut ChatWindowLook) -> T,
    ) -> mlua::Result<T> {
        let i = window_index(id)?;
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let before = model.chat_window_looks[i].clone();
        let out = f(&mut model.chat_window_looks[i]);
        if model.chat_window_looks[i] != before {
            model.chat_window_changes.insert(i);
        }
        Ok(out)
    }

    // Group names, case-folded; an unknown name is skipped (`0x4a0ea9`).
    fn names_of(args: MultiValue) -> Vec<&'static str> {
        args.iter()
            .filter_map(|v| match v {
                Value::String(s) => s.to_str().ok().and_then(|s| message_group_index(s.trim())),
                _ => None,
            })
            .map(|i| MESSAGE_GROUPS[i].0)
            .collect()
    }

    // SetChatWindowName(id, name): `SStrCopy(record+0, name, 0x20)`, 31 bytes and the NUL. The
    // reference stores "" for a name neither string nor number (`0x4a1436`); here a missing or nil
    // name stores "", and a boolean or table raises.
    lua.globals().set(
        "SetChatWindowName",
        lua.create_function(|lua, (id, name): (i64, Option<String>)| {
            let mut name = name.unwrap_or_default();
            if name.len() > 31 {
                let mut cut = 31;
                while !name.is_char_boundary(cut) {
                    cut -= 1;
                }
                name.truncate(cut);
            }
            with_look(lua, id, |look| look.name = name)
        })?,
    )?;

    // SetChatWindowShown(id [, shown]): `+0x94` through the flag coercion `0x6f1c10`, default 1.
    // No argument stores 1; nil, false, 0, "0", "off" and "disabled" store 0. The reference also
    // stores 0 for a string opening F or N ("false", "no"), which stores 1 here.
    lua.globals().set(
        "SetChatWindowShown",
        lua.create_function(|lua, (id, rest): (i64, MultiValue)| {
            let shown = match rest.front() {
                None => true,
                Some(Value::Nil) | Some(Value::Boolean(false)) => false,
                Some(Value::Boolean(true)) => true,
                Some(Value::Integer(n)) => *n != 0,
                Some(Value::Number(n)) => n.is_finite() && n.trunc() != 0.0,
                Some(Value::String(s)) => {
                    let s = s
                        .to_str()
                        .map(|s| s.trim().to_ascii_lowercase())
                        .unwrap_or_default();
                    if s.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
                        leading_int(&s) != 0
                    } else {
                        !(s == "off" || s == "disabled")
                    }
                }
                Some(_) => true,
            };
            with_look(lua, id, |look| look.shown = shown)
        })?,
    )?;

    lua.globals().set(
        "AddChatWindowMessages",
        lua.create_function(|lua, (id, names): (i64, MultiValue)| {
            let names = names_of(names);
            with_look(lua, id, |look| {
                for name in names {
                    if !look.messages.iter().any(|m| m == name) {
                        look.messages.push(name.to_string());
                    }
                }
                look.normalize_messages();
            })
        })?,
    )?;

    lua.globals().set(
        "RemoveChatWindowMessages",
        lua.create_function(|lua, (id, names): (i64, MultiValue)| {
            let names = names_of(names);
            with_look(lua, id, |look| {
                look.messages.retain(|m| !names.contains(&m.as_str()));
            })
        })?,
    )?;

    // GetChatWindowMessages(id): the enabled group names as varargs, in table order.
    lua.globals().set(
        "GetChatWindowMessages",
        lua.create_function(|lua, id: i64| {
            let i = window_index(id)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::new();
            for name in &model.chat_window_looks[i].messages {
                out.push(Value::String(lua.create_string(name)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // AddChatWindowChannel(id, name) (`0x4a1000`): a whole, case-folded match on a
    // `ChatChannels.dbc` Shortcut stores the DBC's spelling and answers the row id; no match stores
    // the name and answers 0, which `ChatFrame_AddChannel` reads as truthy; a matched row with no
    // zone text yet answers and stores nothing. A name already there only answers the id.
    lua.globals().set(
        "AddChatWindowChannel",
        lua.create_function(|lua, (id, name): (i64, Option<String>)| {
            let Some(name) = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) else {
                return Ok(MultiValue::new());
            };
            let matched = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .zone_channel_catalog
                    .iter()
                    .find(|r| r.shortcut.eq_ignore_ascii_case(&name))
                    .cloned()
            };
            let (name, zone_id) = match matched {
                None => (name, 0),
                Some(ZoneChannelRow { resolved: None, .. }) => return Ok(MultiValue::new()),
                Some(row) => (row.shortcut, row.id),
            };
            with_look(lua, id, |look| {
                if !look
                    .channels
                    .iter()
                    .any(|(c, _)| c.eq_ignore_ascii_case(&name))
                {
                    look.channels.push((name, zone_id));
                }
            })?;
            Ok(MultiValue::from_vec(vec![Value::Integer(i64::from(
                zone_id,
            ))]))
        })?,
    )?;

    // RemoveChatWindowChannel(id, name | n) (`0x4a1260`): a numeric key names the joined channel
    // in slot n, and an empty slot removes nothing; the key then resolves to its DBC Shortcut as
    // above, and the entry of that name is freed.
    lua.globals().set(
        "RemoveChatWindowChannel",
        lua.create_function(|lua, (id, key): (i64, Option<String>)| {
            let Some(key) = key else {
                return Ok(());
            };
            let key = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let n = leading_int(&key);
                let key = if n > 0 {
                    match usize::try_from(n)
                        .ok()
                        .and_then(|n| model.joined_channels.get(n - 1))
                        .and_then(|c| c.clone())
                    {
                        Some(name) => name,
                        None => return Ok(()),
                    }
                } else {
                    key.trim().to_string()
                };
                match model
                    .zone_channel_catalog
                    .iter()
                    .find(|r| r.shortcut.eq_ignore_ascii_case(&key))
                {
                    Some(ZoneChannelRow { resolved: None, .. }) => return Ok(()),
                    Some(row) => row.shortcut.clone(),
                    None => key,
                }
            };
            with_look(lua, id, |look| {
                look.channels.retain(|(c, _)| !c.eq_ignore_ascii_case(&key));
            })
        })?,
    )?;

    // GetChatWindowChannels(id): `name, zoneId` pairs in slot order.
    lua.globals().set(
        "GetChatWindowChannels",
        lua.create_function(|lua, id: i64| {
            let i = window_index(id)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::new();
            for (name, zone_id) in &model.chat_window_looks[i].channels {
                out.push(Value::String(lua.create_string(name)?));
                out.push(Value::Integer(i64::from(*zone_id)));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn get_chat_window_info_answers_the_nine_value_tuple() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetChatWindowInfo(1)").unwrap(), 9);
    }

    #[test]
    fn every_window_name_is_the_empty_string_not_a_label() {
        let s = UiScript::new().unwrap();
        for id in 1..=7 {
            let name: String = s
                .eval(&format!("return (GetChatWindowInfo({id}))"))
                .unwrap();
            assert_eq!(name, "", "window {id} name");
        }
    }

    /// At boot (`0x4982c0`) window 1 is shown and undocked, window 2 shown and docked at 1, and
    /// windows 3 to 7 neither.
    #[test]
    fn hidden_and_undocked_windows_answer_nil_never_zero() {
        let s = UiScript::new().unwrap();
        let probe = |id: i32| -> (String, String) {
            let shown = s
                .eval::<String>(&format!(
                    "local _,_,_,_,_,_,shown = GetChatWindowInfo({id}) return type(shown)"
                ))
                .unwrap();
            let docked = s
                .eval::<String>(&format!(
                    "local _,_,_,_,_,_,_,_,docked = GetChatWindowInfo({id}) return type(docked)"
                ))
                .unwrap();
            (shown, docked)
        };
        assert_eq!(probe(1), ("number".into(), "nil".into()));
        assert_eq!(probe(2), ("number".into(), "number".into()));
        for id in 3..=7 {
            assert_eq!(probe(id), ("nil".into(), "nil".into()), "window {id}");
        }
        // The truthiness FrameXML branches on.
        assert!(s
            .eval::<bool>(
                "for i = 3, 7 do local _,_,_,_,_,_,shown = GetChatWindowInfo(i) \
                 if shown then return false end end return true"
            )
            .unwrap());
    }

    /// The boot init docks window 2 at 1 (`0x4982c0`: `mov ds:0xb4ff78, 1`).
    #[test]
    fn docked_is_a_dock_position_not_a_flag() {
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("local _,_,_,_,_,_,_,_,d = GetChatWindowInfo(1) return d == nil")
            .unwrap());
        assert_eq!(
            s.eval::<i64>("local _,_,_,_,_,_,_,_,d = GetChatWindowInfo(2) return d")
                .unwrap(),
            1
        );
        s.run("SetChatWindowDocked(1, 1) SetChatWindowDocked(2, 2)")
            .unwrap();
        assert_eq!(
            s.eval::<i64>("local _,_,_,_,_,_,_,_,d = GetChatWindowInfo(2) return d")
                .unwrap(),
            2
        );
        let _ = s.take_chat_window_changes();
    }

    /// The debug-window walk of MikScrollingBattleText and EnhTooltip, whose `string.lower(name)`
    /// needs a string.
    #[test]
    fn the_corpus_debug_window_walk_completes_and_finds_none() {
        let s = UiScript::new().unwrap();
        let found: i64 = s
            .eval(
                "local debugWin = 0\n\
                 for i = 1, 7 do\n\
                   local name, _, _, _, _, _, shown = GetChatWindowInfo(i)\n\
                   if string.lower(name) == 'debug' then debugWin = i break end\n\
                 end\n\
                 return debugWin",
            )
            .unwrap();
        assert_eq!(found, 0);
    }

    #[test]
    fn a_window_past_the_last_frame_raises() {
        let s = UiScript::new().unwrap();
        assert!(s.eval::<i64>("return (GetChatWindowInfo(8))").is_err());
        assert!(s.eval::<i64>("return (GetChatWindowInfo(0))").is_err());
    }

    /// The reference's `jle` at `0x4a14bc`.
    #[test]
    fn a_non_positive_font_size_is_dropped_not_stored() {
        let s = UiScript::new().unwrap();
        s.run("SetChatWindowSize(1, 16)").unwrap();
        for bad in ["0", "-1", "0.5"] {
            s.run(&format!("SetChatWindowSize(1, {bad})")).unwrap();
            assert_eq!(
                s.eval::<i64>("local _, size = GetChatWindowInfo(1) return size")
                    .unwrap(),
                16,
                "SetChatWindowSize(1, {bad}) must change nothing"
            );
        }
    }

    /// `__ftol(x · 255.0)` truncates: 0.4 stores 102 and 0.5 stores 127.
    #[test]
    fn the_setters_round_trip_through_the_engine_byte() {
        let s = UiScript::new().unwrap();
        s.run("SetChatWindowAlpha(1, 0.4)").unwrap();
        let a: f64 = s
            .eval("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
            .unwrap();
        assert_eq!(a, 102.0 / 255.0, "alpha quantises to the stored byte");

        s.run("SetChatWindowColor(1, 1, 0.5, 0)").unwrap();
        let (r, g, b): (f64, f64, f64) = s
            .eval("local _,_,r,g,b = GetChatWindowInfo(1) return r, g, b")
            .unwrap();
        assert_eq!((r, g, b), (1.0, 127.0 / 255.0, 0.0));

        s.run("SetChatWindowSize(1, 16)").unwrap();
        let size: i64 = s
            .eval("local _, size = GetChatWindowInfo(1) return size")
            .unwrap();
        assert_eq!(size, 16);

        s.run("SetChatWindowAlpha(1, 0.5)").unwrap();
        let a: f64 = s
            .eval("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
            .unwrap();
        assert_eq!(
            a,
            127.0 / 255.0,
            "__ftol truncates; 128/255 would be a round"
        );
    }

    #[test]
    fn a_setter_moves_only_the_window_it_names() {
        let s = UiScript::new().unwrap();
        s.run("SetChatWindowAlpha(2, 1)").unwrap();
        let one: f64 = s
            .eval("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
            .unwrap();
        let two: f64 = s
            .eval("local _,_,_,_,_,a = GetChatWindowInfo(2) return a")
            .unwrap();
        assert_eq!(one, 0.0);
        assert_eq!(two, 1.0);
    }

    #[test]
    fn the_setters_raise_on_a_window_with_no_frame() {
        let s = UiScript::new().unwrap();
        assert!(s.run("SetChatWindowAlpha(8, 1)").is_err());
        assert!(s.run("SetChatWindowColor(0, 1, 1, 1)").is_err());
        assert!(s.run("SetChatWindowSize(8, 14)").is_err());
    }

    /// The deviation: the reference stores the low byte of `__ftol(2.0 · 255) = 510`, answering
    /// 254/255.
    #[test]
    fn an_out_of_domain_alpha_clamps_rather_than_wrapping() {
        let s = UiScript::new().unwrap();
        s.run("SetChatWindowAlpha(1, 2.0)").unwrap();
        let a: f64 = s
            .eval("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
            .unwrap();
        assert_eq!(a, 1.0);
        s.run("SetChatWindowAlpha(1, -1)").unwrap();
        let a: f64 = s
            .eval("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
            .unwrap();
        assert_eq!(a, 0.0);
    }

    #[test]
    fn the_host_seam_dedupes_writes_and_stays_quiet_on_load() {
        let mut s = UiScript::new().unwrap();
        for step in 0..40 {
            s.run(&format!(
                "SetChatWindowAlpha(1, {})",
                f64::from(step) / 40.0
            ))
            .unwrap();
        }
        s.run("SetChatWindowColor(2, 0.2, 0.2, 0.2)").unwrap();
        assert_eq!(s.take_chat_window_changes(), vec![0, 1]);
        assert!(
            s.take_chat_window_changes().is_empty(),
            "the drain is a take"
        );

        s.set_chat_window_looks([(
            0,
            crate::script::ChatWindowLook {
                r: 1,
                g: 2,
                b: 3,
                a: 4,
                font_size: 14,
                locked: true,
                docked: Some(1),
                ..Default::default()
            },
        )]);
        assert!(
            s.take_chat_window_changes().is_empty(),
            "the load path never echoes"
        );
        assert_eq!(s.chat_window_looks()[0].font_size, 14);
    }

    #[test]
    fn a_write_that_moves_nothing_queues_nothing() {
        let mut s = UiScript::new().unwrap();
        s.run("SetChatWindowAlpha(1, 0)").unwrap();
        s.run("SetChatWindowColor(1, 0, 0, 0)").unwrap();
        s.run("SetChatWindowSize(1, 0)").unwrap();
        assert!(s.take_chat_window_changes().is_empty());
    }

    #[test]
    fn chat_frame_open_chat_queues_its_text_and_ignores_the_frame() {
        let mut s = UiScript::new().unwrap();
        s.run("ChatFrame_OpenChat('/w Bob ')").unwrap();
        s.run("ChatFrame_OpenChat('', 'not even a frame')").unwrap();
        assert_eq!(
            s.take_open_chat_requests(),
            vec!["/w Bob ".to_string(), String::new()]
        );
        assert!(
            s.take_open_chat_requests().is_empty(),
            "the drain is a take"
        );
    }
}

#[cfg(test)]
mod record_tests {
    use super::{ChatWindowLook, MESSAGE_GROUPS};
    use crate::script::{UiScript, ZoneChannelRow};

    /// The boot init (`0x4982c0`); 34 of groups 11 to 68 are `defaultOn`.
    #[test]
    fn the_stock_records_are_the_boot_init() {
        let s = UiScript::new().unwrap();
        let looks = s.chat_window_looks();
        assert!(looks[0].shown && looks[1].shown && !looks[2].shown);
        assert_eq!((looks[0].docked, looks[1].docked), (None, Some(1)));
        let general: Vec<String> = MESSAGE_GROUPS[..10]
            .iter()
            .map(|(n, _, _)| (*n).to_string())
            .collect();
        assert_eq!(looks[0].messages, general);
        assert_eq!(looks[1].messages.len(), 34);
        assert_eq!(looks[1].messages[0], "COMBAT_MISC_INFO");
        assert_eq!(looks[1].messages[33], "MONEY");
        assert!(!looks[1].messages.iter().any(|m| m == "COMBAT_PARTY_HITS"));
        assert!(looks[2].messages.is_empty());
        assert!(looks
            .iter()
            .all(|l| l.channels.is_empty() && l.name.is_empty()));
        assert_eq!(
            s.eval::<Vec<String>>("return {GetChatWindowMessages(1)}")
                .unwrap(),
            general
        );
    }

    #[test]
    fn name_and_shown_round_trip_through_the_getter_and_cue_the_persist() {
        let mut s = UiScript::new().unwrap();
        s.run("SetChatWindowName(3, 'Loot') SetChatWindowShown(3) SetChatWindowShown(1, nil)")
            .unwrap();
        assert_eq!(
            s.eval::<(String, Option<i64>)>(
                "local n, _, _, _, _, _, sh = GetChatWindowInfo(3) return n, sh"
            )
            .unwrap(),
            ("Loot".to_string(), Some(1))
        );
        assert!(s
            .eval::<bool>("local _, _, _, _, _, _, sh = GetChatWindowInfo(1) return sh == nil")
            .unwrap());
        assert_eq!(s.take_chat_window_changes(), vec![0, 2]);
        s.run("SetChatWindowName(3, 'Loot')").unwrap();
        assert!(s.take_chat_window_changes().is_empty(), "no move, no cue");
        // The flag coercion (`0x6f1c10`, default 1) and the 31-byte name cap.
        s.run(
            "SetChatWindowShown(4, 'off') SetChatWindowShown(5, '1') SetChatWindowShown(6, 0) \
             SetChatWindowName(4, 'abcdefghijklmnopqrstuvwxyz0123456789')",
        )
        .unwrap();
        let looks = s.chat_window_looks();
        assert_eq!(
            (looks[3].shown, looks[4].shown, looks[5].shown),
            (false, true, false)
        );
        assert_eq!(looks[3].name.len(), 31, "31 bytes and the NUL");
    }

    #[test]
    fn message_types_are_flags_answered_in_table_order() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "AddChatWindowMessages(3, 'yell', 'SAY', 'BOGUS', 'SAY', 'money') \
             RemoveChatWindowMessages(1, 'Say', 'LOOT', 'NOPE') \
             AddChatWindowMessages(3)",
        )
        .unwrap();
        assert_eq!(
            s.eval::<Vec<String>>("return {GetChatWindowMessages(3)}")
                .unwrap(),
            vec!["SAY".to_string(), "YELL".to_string(), "MONEY".to_string()]
        );
        let general = s.chat_window_looks()[0].messages.clone();
        assert!(!general.iter().any(|m| m == "SAY" || m == "LOOT"));
        assert_eq!(general.len(), 8);
        assert_eq!(s.take_chat_window_changes(), vec![0, 2]);
        s.set_chat_window_looks([(
            4,
            ChatWindowLook {
                messages: vec!["money".into(), "bogus".into(), "SAY".into(), "say".into()],
                ..Default::default()
            },
        )]);
        assert_eq!(
            s.chat_window_looks()[4].messages,
            vec!["SAY".to_string(), "MONEY".to_string()]
        );
    }

    #[test]
    fn channels_carry_the_zone_id_and_answer_it_on_add() {
        let mut s = UiScript::new().unwrap();
        s.set_zone_channel_catalog(vec![
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
        ]);
        s.run(
            "A = {AddChatWindowChannel(1, 'general')} \
             B = {AddChatWindowChannel(1, 'MyChan')} \
             C = {AddChatWindowChannel(1, 'mychan')} \
             D = {AddChatWindowChannel(1, 'Trade')} \
             E = {AddChatWindowChannel(1, '')}",
        )
        .unwrap();
        assert_eq!(
            s.eval::<(i64, i64, i64)>("return A[1], B[1], C[1]")
                .unwrap(),
            (1, 0, 0)
        );
        assert!(s
            .eval::<bool>("return table.getn(D) == 0 and table.getn(E) == 0")
            .unwrap());
        assert_eq!(
            s.eval::<Vec<mlua::Value>>("return {GetChatWindowChannels(1)}")
                .unwrap()
                .len(),
            4,
            "name, id, name, id — the duplicate was not stored twice"
        );
        assert_eq!(
            s.chat_window_looks()[0].channels,
            vec![("General".to_string(), 1), ("MyChan".to_string(), 0)],
            "the DBC's spelling for the shortcut, the typed one for the custom channel"
        );
        s.run("RemoveChatWindowChannel(1, 'MYCHAN') RemoveChatWindowChannel(1, '3')")
            .unwrap();
        assert_eq!(
            s.chat_window_looks()[0].channels,
            vec![("General".to_string(), 1)],
            "a number names a joined slot; with none joined it removes nothing"
        );
        assert_eq!(s.take_chat_window_changes(), vec![0]);
    }

    #[test]
    fn a_host_loaded_record_is_what_the_verbs_read() {
        let mut s = UiScript::new().unwrap();
        s.set_chat_window_looks([(
            4,
            ChatWindowLook {
                name: "Trade".into(),
                shown: true,
                messages: vec!["CHANNEL".into()],
                channels: vec![("Trade".into(), 2)],
                ..Default::default()
            },
        )]);
        assert_eq!(
            s.eval::<Vec<String>>("return {GetChatWindowMessages(5)}")
                .unwrap(),
            vec!["CHANNEL".to_string()]
        );
        assert_eq!(
            s.eval::<(String, i64)>("return GetChatWindowChannels(5)")
                .unwrap(),
            ("Trade".to_string(), 2)
        );
        assert!(
            s.take_chat_window_changes().is_empty(),
            "the load path never echoes"
        );
    }
}
