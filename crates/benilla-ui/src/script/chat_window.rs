//! The chat **window** surface: `GetChatWindowInfo` (the engine's per-window settings getter) and
//! `ChatFrame_OpenChat` (the FrameXML verb that opens the edit box prefilled).
//!
//! The windows themselves are frames, declared in the app's shipped `ChatFrame.xml` —
//! `ChatFrame1`..`ChatFrame7`, of which 1 and 2 are docked and 3..7 ship hidden and undocked, the
//! reference's own arrangement. This module is the *settings* half: what an addon learns when it
//! asks about window `i` without touching the frame.
//!
//! ## `GetChatWindowInfo(id)` → `name, fontSize, r, g, b, a, shown, locked, docked`
//!
//! A registered Lua binding in the real client (`0x4a0ba0`), reading the per-window struct array at
//! `0xb4fe50` — stride 0x98, **10 windows**, colour bytes at +0xd8..+0xdb renormalised by the f32
//! 1/255 (`0x8026c8`). Those structs are loaded from the character's `chat-cache.txt`, and a real
//! stock one, written by the reference client itself, is on disk — every value below is quoted from
//! it:
//!
//! ```text
//! WINDOW 1   SIZE 0  COLOR 0 0 0 0  LOCKED 1  DOCKED 1  SHOWN 1
//! WINDOW 2   SIZE 0  COLOR 0 0 0 0  LOCKED 1  DOCKED 2  SHOWN 0
//! WINDOW 3…10 SIZE 0 COLOR 0 0 0 0  LOCKED 1  DOCKED 0  SHOWN 0
//! ```
//!
//! **Three traps live in that tuple, and all three are load-bearing:**
//!
//! 1. **`name` is the empty string, not "General".** Nothing writes a window name until the user
//!    renames a tab, so a stock client answers `""` for every window including the first two. The
//!    familiar labels are a *FrameXML fallback*, not data: `FCF_SetWindowName` (FloatingChatFrame.lua
//!    l.680-684) reads `if ( not name or name == "" )` and substitutes `GENERAL` / `COMBAT_LOG` /
//!    `format(CHAT_NAME_TEMPLATE, id)`. Our own tab labels are that fallback, hardcoded. An addon
//!    proves the contract from the other side: `Enchantrix/EnxConfig.lua:113` does
//!    `local name = GetChatWindowInfo(i); if ( name == "" ) then if (i == 1) then name =
//!    _ENCH('TextGeneral') …` — it would not have been written if the getter answered "General".
//!    Answering "General" here would look friendlier and would be a divergence.
//! 2. **`shown`/`locked`/`docked` are numbers or `nil` — never `0`.** FrameXML branches on them
//!    bare (`if ( shown ) then chatFrame:Show()`, `if ( docked ) then FCF_DockFrame(…)` —
//!    FloatingChatFrame.lua l.59/69), and `0` is TRUE in Lua. A getter that returned the cache's
//!    literal `0` would show every hidden window.
//! 3. **`docked` is a dock POSITION, not a flag.** Window 1 answers `1`, window 2 answers `2` —
//!    the order tabs sit in the dock, which is why `FCF_DockFrame(frame, docked)` takes it as an
//!    index. Reading it as a boolean happens to work; reading it as "is docked" and writing back
//!    `1` would silently reorder the dock.
//!
//! `fontSize` is the cache's `SIZE`, and `0` is what a stock client stores — "use the font's own
//! height". No FrameXML path applies it (`FloatingChatFrame_Update` destructures it and never
//! reads it; the options dropdown checks `FCF_GetCurrentChatFrame():GetFont()` instead), and no
//! corpus addon reads it.
//!
//! ## The three setters, and why this getter stopped being a constant
//!
//! Until 1589 the whole tuple above was a frozen table: benilla had no way to *change* a window's
//! look, so answering with the stock cache row was the honest answer. B246 ("no chat options at
//! all — background transparency has no home") is the report that ends that, and the reference's
//! own home for it is the chat tab's right-click menu → **Background**, a colour swatch with an
//! opacity slider. So three of the nine values are now real state, written by the reference's own
//! three setters and read straight back out of this getter:
//!
//! | binding | address | what it stores |
//! |---|---|---|
//! | `SetChatWindowColor(id, r, g, b)` | `0x4a14f0` | `__ftol(x · 255.0)` per channel |
//! | `SetChatWindowAlpha(id, a)` | `0x4a15d0` | `__ftol(a · 255.0)` |
//! | `SetChatWindowSize(id, size)` | `0x4a1470` | the cache's `SIZE` |
//! | `SetChatWindowLocked(id, isLocked)` | `0x4a1650` | the cache's `LOCKED` |
//!
//! (`[0x806498]` is the 255.0 the first two multiply by.) **Bytes, not floats** — that is
//! why the getter renormalises by 1/255, and why [`ChatWindowLook`] stores the colour as `u8`: a
//! set→get round trip through the real client quantises, and a store of `f32` here would
//! round-trip values the reference cannot hold. `SetChatWindowAlpha(1, 0.4)` answers
//! `102/255 = 0.4`, not `0.4`.
//!
//! **The record's own layout**: the colour is ONE packed `CImVector` at
//! **`+0x88` B, `+0x89` G, `+0x8a` R, `+0x8b` A** — note the **BGRA** order — and the font size is
//! an `i32` at **`+0x84`**, not a byte and with no `× 255` anywhere near it. (Both correct this
//! module's earlier `+0xd8..+0xdb`, which was the low half of the *absolute* operand
//! `[esi + 0xb4fed8]`: `0xb4fed8 − 0xb4fe50 = 0x88`.) `chat-cache.txt`'s `COLOR` line is written
//! **R G B A** from `+0x8a,+0x89,+0x88,+0x8b` and parsed straight back as bytes, so the file
//! round-trip is bit-exact and the `×255` / `×1/255` pair exists only at the Lua boundary.
//!
//! Two behaviours of the reference's setters that are easy to miss and are transcribed here:
//!
//! - **`SetChatWindowSize` silently drops a size `<= 0`** (`0x4a14bc jle`). A stock cache holds
//!   `SIZE 0`, so "no size stored" and "cannot store 0" coexist: the field is only ever written by
//!   a real pick off the Font Size menu.
//! - **Nothing clamps.** `__ftol` (`0x40a2b0`) truncates, and the setters store the low byte of
//!   the result with no bound: on a real client `SetChatWindowAlpha(1, 2.0)` stores **254**,
//!   `(1, -1.0)` stores **1**, and `0.5` stores **127**, not 128.
//!
//! **We clamp where it wraps, and that is the one deliberate divergence.** Nothing in FrameXML or
//! the corpus can reach the out-of-domain case (the colour picker's channels and its opacity
//! slider are all `0..1`), and the camera pose file already set this posture: a value nothing can
//! produce is not a thing to be faithful to. The truncation itself IS kept — `0.5` stores 127 here
//! too.
//!
//! **`locked` joined them the day the windows could move.** `SetChatWindowLocked(id, isLocked)` is
//! the fourth setter here now: the chat tab's *Lock/Unlock Window* row is what turns the resize
//! grips and the tab drag on, so a value that used to be a constant has a player-reachable writer
//! and belongs in the record with the rest. The other five stay constants, and that is the honest
//! tree rather than an omission: benilla has no rename, no undock and no window create/close, so
//! `name`, `shown` and `docked` still have nothing that could move them.
//!
//! Measured demand: **3 of the 5 corpus addons that iterate `NUM_CHAT_WINDOWS` call this on the
//! very next line** — `EnhTooltip/Tooltip.lua:1302`, `MikScrollingBattleText.lua:1951` and
//! `Enchantrix/EnxConfig.lua:110`. The first two are the same idiom (look for a window the user
//! named "debug"/"ettdebug"); the third builds a name→index map for its `/enx print-in` config.
//! Without this getter, declaring `NUM_CHAT_WINDOWS` hands those three a loop that raises on its
//! first iteration, so the constant and the getter are one change, not two.
//!
//! ## `ChatFrame_OpenChat(text, chatFrame)`
//!
//! FrameXML in the reference (ChatFrame.lua l.1545), and the same seam as its neighbour
//! `ChatFrame_SendTell` ([`super::party`]): benilla's chat edit machine is app-side
//! (`benilla_app::ui_chat::edit`), so the verb queues the request and the app opens the box.

use mlua::{Lua, MultiValue, Value};

use super::channel::ZoneChannelRow;
use super::Model;

/// How many chat windows the client's own settings array holds — **10**, both in the engine
/// (`0xb4fe50`, stride 0x98) and in `chat-cache.txt`, which writes `WINDOW 1` … `WINDOW 10`.
/// FrameXML declares `ChatFrame1TabDockRegion`..`ChatFrame10TabDockRegion` to match, and then
/// builds only seven `ChatFrame`s: `NUM_CHAT_WINDOWS = 7` is the *UI's* count, not the engine's.
/// Recorded so the 7 below reads as a deliberate choice rather than a miscount.
pub(super) const ENGINE_CHAT_WINDOW_SLOTS: usize = 10;

/// The windows benilla actually builds — `ChatFrame1`..`ChatFrame7` in the app's `ChatFrame.xml`,
/// which is also `NUM_CHAT_WINDOWS`. `benilla_app`'s `chat_tests` cross-checks every entry against
/// the shipped XML so the two cannot drift.
pub(super) const NUM_CHAT_WINDOWS: usize = 7;

/// One chat window's **look** — the mutable slice of the engine's per-window record (decision
/// 1589): the background tint, the background alpha, and the font size. The colour is bytes
/// because the engine's colour is bytes (`0xb4fe50 + n*0x98 + 0x88..0x8b`, packed BGRA,
/// `__ftol(x · 255.0)` in and `× 1/255` out); the font size is an `i32` because the engine's is
/// (`+0x84`). See this module's docs for why that quantisation is kept rather than smoothed over.
///
/// [`Self::default`] is the stock `chat-cache.txt` row — `COLOR 0 0 0 0`, `SIZE 0`, `LOCKED 1` —
/// i.e. a black box at alpha 0 that cannot be dragged, which is the classic "chat is text over the
/// world until you mouse over it".
/// `CHATMSGGROUP` — the client's 68-entry message-group table at `0x805fb0`:
/// `(name, defaultOn, addedVersion)`, stride 0xc,
/// in the order that IS the record's `+0x20` flag index. **Not** the 94-entry colour table: 67
/// names are shared, `CREATURE` exists only here, and 27 colour names (RAID, OFFICER,
/// WHISPER_INFORM, EMOTE, TEXT_EMOTE, MONSTER_*, CHANNEL_JOIN/LEAVE/LIST/NOTICE[_USER], AFK, DND,
/// IGNORED, BG_SYSTEM_*, RAID_LEADER/WARNING/BOSS_EMOTE, FOREIGN_TELL, FILTERED, BATTLEGROUND[_LEADER])
/// have no group. `defaultOn` is what the boot init (`0x4982c0`) copies into window 2's flags;
/// `addedVersion` is the loader's (`0x498a60`) back-fill for a file older than `ADDEDVERSION 2`.
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

/// The flag index of a group name, matched the loader's way (`SStrCmpI` — an ASCII case-fold).
pub fn message_group_index(name: &str) -> Option<usize> {
    MESSAGE_GROUPS
        .iter()
        .position(|(n, _, _)| n.eq_ignore_ascii_case(name))
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChatWindowLook {
    /// Background tint, `0..=255` each. The engine renormalises by 1/255 on the way out.
    pub r: u8,
    /// See [`Self::r`].
    pub g: u8,
    /// See [`Self::r`].
    pub b: u8,
    /// Background alpha, `0..=255`. `0` is the stock value and what benilla shipped as a constant
    /// until 1589 — the hover fade lifts it to `DEFAULT_CHATFRAME_ALPHA` and drops it back.
    pub a: u8,
    /// The cache's `SIZE` — the chat font height in points, or `0` for "the font's own height".
    /// The reference's own values are `CHAT_FONT_HEIGHTS = {12, 14, 16, 18}`. An `i32` because the
    /// record's field is (`+0x84`), and never `<= 0` once set: the setter drops those.
    pub font_size: i32,
    /// The cache's `LOCKED` — whether the window refuses to be dragged or resized.
    ///
    /// **`true` out of the box**, because the stock row is `LOCKED 1` for every window: a fresh
    /// character's chat box cannot be nudged out of place by a stray drag, and unlocking it is a
    /// deliberate trip through the tab menu (`FCF_ToggleLock`). It joined this struct with the
    /// move/resize arc: until something could *move* a window, writing the key would have
    /// persisted state nothing could change, which is the honest-tree rule (1134 §4) at the
    /// persistence layer, and the reason 1589 §6 left it out.
    pub locked: bool,
    /// The cache's `DOCKED` — the window's **position in the dock**, 1-based, or `None` for a
    /// window that is not docked. Not a flag: window 1 answers `1` and window 2 `2`, which is the
    /// index `FCF_DockFrame(frame, docked)` takes (this module's §3 note), and the order the tabs
    /// sit in.
    ///
    /// It joined this struct with the dock verbs (1714), by the same rule `locked` did: until
    /// `FCF_DockFrame`/`FCF_UnDockFrame` existed there was nothing that could move it, so it was a
    /// per-window constant read straight out of [`WINDOW_STATE`]. There is now, so it is state.
    pub docked: Option<u8>,
    /// `+0x00` — the window's name as `SetChatWindowName` stored it: `""` until FrameXML names
    /// it (the getter's first return; the `NAME` line is emitted only when non-empty).
    pub name: String,
    /// `+0x94` — `SHOWN`; the getter answers 1 or nil off it.
    pub shown: bool,
    /// `+0x20` — the enabled message-type names, the `MESSAGES … END` block.
    pub messages: Vec<String>,
    /// `+0x64` / `+0x74` — the channels this window shows, `(name, zone channel id)` pairs: the
    /// `CHANNELS … END` block.
    pub channels: Vec<(String, u32)>,
}

impl Default for ChatWindowLook {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl ChatWindowLook {
    /// The stock `chat-cache.txt` row, as a const so [`super::Model`]'s array literal can use it.
    pub(super) const DEFAULT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
        font_size: 0,
        locked: true,
        // The stock row's `DOCKED` differs per window, so the array literal that uses this const
        // seeds each slot from [`WINDOW_STATE`] afterwards. `None` here is only the value a
        // slot holds before that seeding, and the value a *hand-built* look starts at.
        docked: None,
        name: String::new(),
        shown: false,
        messages: Vec::new(),
        channels: Vec::new(),
    };

    /// The stock row for window `index` (0-based) — [`Self::DEFAULT`] with that window's own
    /// `DOCKED` position from [`WINDOW_STATE`]. The seed for the model's array and for the
    /// settings parser's per-line default, so a file that never mentions `DOCKED` keeps the
    /// shipped dock rather than silently undocking both windows.
    /// The boot init `0x4982c0` — the record a client with no
    /// `chat-cache.txt` runs from, and what FrameXML's own `FloatingChatFrame_Update` then docks,
    /// hides and saves: window 1 shown and undocked (it IS the dock — `FCF_DockFrame(ChatFrame1, 1)`
    /// at FloatingChatFrame.xml's file scope), groups 1–10 by a literal fill; window 2 shown and
    /// `docked 1`, every `defaultOn` group of 11–68 (the 34 the stock file lists); windows 3–10
    /// hidden with nothing enabled. Every window locked, size 0, colour 0, name empty, ten empty
    /// channel slots. The init fires no event; the loader (`0x498a60`) does.
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

    /// The loader's discipline over the message set — unknown names dropped, duplicates folded,
    /// **table order** — so `GetChatWindowMessages` (`0x4a0d20`) answers the way a flag walk does —
    /// table order 0..67 — whatever order a file or a caller listed them in.
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

/// `x` in the reference's `0..1` float domain → the byte its `__ftol(x · 255.0)` stores, clamped
/// (see the module docs' divergence note). `NaN` lands on `0` rather than propagating.
/// `SStrToInt` — the leading decimal integer of a string (an optional sign, then digits), 0 when
/// there is none.
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

fn to_byte(x: f64) -> u8 {
    if !x.is_finite() {
        return 0;
    }
    (x * 255.0).trunc().clamp(0.0, 255.0) as u8
}

/// The byte the engine stores → the `0..1` float `GetChatWindowInfo` answers.
fn from_byte(b: u8) -> f64 {
    f64::from(b) / 255.0
}

/// The 1-based Lua window id → a live index, or the raise every out-of-range ask gets. Shared by
/// the getter and the three setters so a window that cannot be read cannot be written either.
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
    /// **Register a channel in window `window`'s list** — the shared tail of `JoinChannelByName`
    /// (`0x49ec24`–`0x49ede7`) and `AddChatWindowChannel` (`0x4a1000`): `(name, id)` appended
    /// unless an entry of that name is already there (`SStrCmpI`), and the window marked for the
    /// persist pass. `name` is the DBC's own Shortcut for a matched row, the argument verbatim for
    /// a custom channel. Answers whether anything was added.
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

    /// **Strip `key` from every window's list** — leave-by-name `0x49ee70`'s `0x49f001`–`0x49f085`:
    /// all ten windows, the first entry whose name
    /// equals `key` case-insensitively, the cell blanked. `key` is the DBC Shortcut when the
    /// argument matched a row, else the argument verbatim; the numeric form strips nothing,
    /// because a slot name never equals a window entry. Answers whether anything was stripped.
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
    /// Host-side [`Model::register_window_channel`] — the guild-recruitment cascade's join arm,
    /// which registers `GuildRecruitment` in window 1 the way `0x49eb70(frameIdx = 0)` does.
    pub fn register_chat_window_channel(&mut self, window: usize, name: &str, id: u32) -> bool {
        self.model_mut()
            .register_window_channel(window, name.to_string(), id)
    }

    /// Seed the per-window looks from the host's persisted store — the load path, so it queues no
    /// change (an echo would re-dirty the file it was just read from; [`Self::set_cvar_host`]'s
    /// reason, one store over).
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

    /// Snapshot every window's look, index 0 = `ChatFrame1` — what the saver writes out.
    pub fn chat_window_looks(&self) -> Vec<ChatWindowLook> {
        self.model_mut().chat_window_looks.to_vec()
    }

    /// Drain the 0-based indices whose look Lua moved since the last call — the host's cue to
    /// persist. Deduplicated and ascending, so a slider drag that wrote one window forty times
    /// costs the saver one entry.
    pub fn take_chat_window_changes(&mut self) -> Vec<usize> {
        let mut v: Vec<usize> = std::mem::take(&mut self.model_mut().chat_window_changes)
            .into_iter()
            .collect();
        v.sort_unstable();
        v
    }
}

impl super::UiScript {
    /// Drain the `ChatFrame_OpenChat` requests queued since the last call — each is the text the
    /// caller wants the chat edit box to open prefilled with. The app opens and focuses the box,
    /// applies its own sticky-type law, and lets the live parse take the text from there.
    pub fn take_open_chat_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().open_chat_requests)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // GetChatWindowInfo(id) → name, fontSize, r, g, b, a, shown, locked, docked.
    //
    // Out of range RAISES rather than answering. The reference's array has 10 slots and benilla
    // builds 7 windows; a question about window 8 is a question about a window that does not exist
    // here, and the two honest answers are "raise" or "invent a row". Every corpus caller loops
    // `1, NUM_CHAT_WINDOWS`, so the raise is unreachable from real code and exists to keep the
    // next caller from reading a fabricated window as a real one.
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
            // `docked` is the LIVE position now, not the table's — `FCF_DockFrame`/
            // `FCF_UnDockFrame` move it through `SetChatWindowDocked` below.
            let docked = look.docked.map(i64::from);
            let num = |v: Option<i64>| match v {
                Some(n) => Value::Integer(n),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                // name: "" until `SetChatWindowName` stores one — never a label of ours. See
                // the module docs' trap 1.
                Value::String(lua.create_string(&look.name)?),
                // The three the setters below own (1589). Stock is SIZE 0 / COLOR 0 0 0 0 — the
                // same tuple this getter answered as a constant before those setters existed.
                Value::Integer(i64::from(look.font_size)),
                Value::Number(from_byte(look.r)),
                Value::Number(from_byte(look.g)),
                Value::Number(from_byte(look.b)),
                Value::Number(from_byte(look.a)),
                num(shown), // 1 or nil, never 0
                // locked — the cache's LOCKED, stock `1`, moved by `SetChatWindowLocked` below.
                // `1` or nil like `shown`, never `0`: the reference's own boolean-in-a-number.
                if look.locked {
                    Value::Integer(1)
                } else {
                    Value::Nil
                },
                num(docked), // the dock POSITION (1, 2) or nil
            ]))
        })?,
    )?;

    // SetChatWindowColor(id, r, g, b) — 0x4a14f0, `__ftol(x · 255.0)` per channel.
    //
    // FrameXML's caller is `FCF_SetWindowColor` (FloatingChatFrame.lua l.696-704), which tints the
    // nine CHAT_FRAME_TEXTURES and then saves through here; the tab menu's Background swatch
    // reaches it via `FCF_SetChatWindowBackGroundColor`. The write is what makes the tint survive
    // a session — the *visible* tint is the Lua SetVertexColor, not this.
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

    // SetChatWindowAlpha(id, alpha) — 0x4a15d0, `__ftol(alpha · 255.0)`.
    //
    // FrameXML's caller is `FCF_SetWindowAlpha` (l.706-716), the other half of the tab menu's
    // Background row: its opacity slider drives `FCF_SetChatWindowOpacity` on every drag step.
    // `frame.oldAlpha` — the value the hover fade returns to — is that same number.
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

    // SetChatWindowLocked(id, isLocked) — `0x4a1650`, writing the record's `+0x8c` locked field
    // (`0x4a16a2 mov [esi+0xb4fedc],eax`, initialised to **1** at `0x4984e4`, read back as
    // `GetChatWindowInfo`'s 8th return at `0x4a0cbf`).
    //
    // **A `bool` is the faithful store even though the field is an `i32`**: the cache writer
    // booleanises it through `setne` (`0x499e8b`, fmt `LOCKED %d`), so nothing outside `{0,1}`
    // survives a file round trip — the same note records that as the difference between this field
    // and `SIZE`, which does round-trip arbitrary values.
    //
    // The one value of the tuple the *player* moves rather than the layout: `FCF_SetLocked`
    // (FloatingChatFrame.lua l.802-805) writes the frame field and this store in the same breath,
    // and `FloatingChatFrame_Update` (l.56) seats the frame back from here at load.
    //
    // Lua truthiness, not a strict boolean, because the reference's callers pass `1` and `nil` —
    // `FCF_ToggleLock`'s two arms and `FCF_OpenNewWindow`'s `SetChatWindowLocked(i, nil)`. mlua
    // marshals both the way the reference binding's own `toboolean` does.
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
    // SetChatWindowDocked(id, position) — the dock half of the record, written by `FCF_SaveDock`
    // (FloatingChatFrame.lua l.1276-1285) once per docked window on every dock change, and by
    // `FCF_UnDockFrame` (l.1182) with `nil` to clear it.
    //
    // **The argument is a POSITION, not a flag** — this module's §3 trap, and the reason this
    // takes a number rather than the `bool` `SetChatWindowLocked` takes: `FCF_SaveDock` passes the
    // running `count`, so window 1 stores `1` and window 2 stores `2`, and that is what
    // `GetChatWindowInfo`'s 9th return hands back to `FCF_DockFrame(frame, docked)` as its index.
    // A `nil` clears it (undocked); anything `<= 0` is treated as a clear too, since a dock
    // position is 1-based and the reference has no zeroth slot.
    //
    // **Not persisted yet, and that is a stated gap.** The `chat-cache` writer
    // (`benilla_app`'s `ui_chat::settings`) renders `DOCKED` alongside `LOCKED`, but an absent key
    // must keep the *per-window* stock rather than one flat default — see
    // [`ChatWindowLook::stock`], which is what the parser seeds from.
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
    // SetChatWindowSize(id, fontSize) — 0x4a1470.
    //
    // FrameXML's caller is `FCF_SetChatWindowFontSize` (l.752-763), which does the visible half
    // (`chatFrame:SetFont(file, size, flags)`) and then saves through here. A size the reference
    // never stores is `0` — "the font's own height" — which is what a stock cache holds and what
    // benilla ships until the player picks one off the Font Size submenu.
    lua.globals().set(
        "SetChatWindowSize",
        lua.create_function(|lua, (id, size): (i64, f64)| {
            let i = window_index(id)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let look = &mut model.chat_window_looks[i];
            // The cache's SIZE is a small integer; anything unrepresentable clamps rather than
            // wrapping (the module docs' divergence, same reason).
            // `<= 0` is dropped, not stored — the reference's own `jle` at `0x4a14bc`. That is
            // why a stock `SIZE 0` and "the player picked a size" are different states rather
            // than the same one written twice.
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

    // ChatFrame_OpenChat(text, chatFrame) — open the chat edit box prefilled with `text`.
    //
    // **The second argument is inert, and that is the reference's doing, not a shortcut.** The ref
    // reaches the box through `chatFrame.editBox`, and in 1.12 every chat frame's `.editBox` is
    // the *same* global `ChatFrameEditBox`: `FloatingChatFrame_OnLoad` sets
    // `this.editBox = ChatFrameEditBox` for each frame (FloatingChatFrame.lua l.30), and
    // ChatFrame1 — which loads before the edit box exists and so skips that `if` — is patched by
    // the box's own OnLoad, `DEFAULT_CHAT_FRAME.editBox = this` (FloatingChatFrame.xml l.742).
    // Per-frame edit boxes are a 2.x feature. So `ChatFrame_OpenChat(text, ChatFrame5)` opens the
    // one and only edit box on a real 1.12 client too, and accepting-and-ignoring the frame here
    // is exact rather than approximate. `benilla_app`'s `chat_tests` states that as a claim.
    //
    // Measured demand: 3 distinct addons, and all three are the same shape — check the box is not
    // already up, then open it prefilled, else fall back to SetText/Insert:
    //   FuBar_HeyFu/Core.lua:281,292      ChatFrame_OpenChat(reply, DEFAULT_CHAT_FRAME)
    //   FuBar_FriendsFu/FriendsFu.lua:434 ChatFrame_OpenChat(format("/w %s ", name))   -- 1 arg
    //   TipBuddy/TipBuddy.xml:2715,2717   ChatFrame_OpenChat("", chatFrame) / ("/", chatFrame)
    // Two of the three prefill `/w <name> `, which the app's live parse then turns into whisper
    // mode with the target extracted — the same path a human typing those characters takes.
    //
    // The ref's tail — the PARTY/RAID/BATTLEGROUND sticky downgrade (l.1554-1565) — is not skipped,
    // it is the app's: `ui_chat::edit`'s open path already applies exactly that law (a sticky PARTY
    // with nobody in the party opens as SAY), so running it here would be running it twice.
    lua.globals().set(
        "ChatFrame_OpenChat",
        lua.create_function(|lua, (text, _chat_frame): (Option<String>, Value)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.open_chat_requests.push(text.unwrap_or_default());
            Ok(())
        })?,
    )?;

    // ── The rest of the 0x98 record: name, shown, message types, channels ──────────────────
    //
    // `SetChatWindowName 0x4a13f0` / `SetChatWindowShown 0x4a1730` are registrar entries beside
    // the look setters; `Add/Remove/GetChatWindowMessages`
    // (`0x4a0e80`/`0x4a0f40`/`0x4a0d20`) and `Add/Remove/GetChatWindowChannel(s)`
    // (`0x4a1000`/`0x4a1260`/`0x4a0dc0`) read and write the `+0x20` flags and the `+0x64`/`+0x74`
    // arrays the same census bounded. Every write that moves the record queues the window
    // for the app's persist pass, like the look setters.

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

    // The names, resolved against `CHATMSGGROUP` the loader's way; a name with no group is
    // silently nothing (`0x4a0ea9` scan, no match → no write).
    fn names_of(args: MultiValue) -> Vec<&'static str> {
        args.iter()
            .filter_map(|v| match v {
                Value::String(s) => s.to_str().ok().and_then(|s| message_group_index(s.trim())),
                _ => None,
            })
            .map(|i| MESSAGE_GROUPS[i].0)
            .collect()
    }

    // SetChatWindowName(id, name) — `SStrCopy(record+0, name, 0x20)`: 31 bytes and the NUL; a
    // non-string second argument is the empty string (`0x4a1436`).
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

    // SetChatWindowShown(id [, shown]) — `+0x94 = 0x6f1c10(L, 2, default 1)`: NO second argument
    // stores 1; an explicit nil, `false`, `0` or `"off"` store 0 — the reference's own flag
    // coercion (a boolean, a number by `__ftol`, a string by its digits or by
    // on/off/enabled/disabled).
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

    // AddChatWindowMessages(id, "TYPE", "TYPE", …) — each name set in the window's flags.
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

    // GetChatWindowMessages(id) — the enabled names as a vararg list, which
    // `ChatFrame_RegisterForMessages(GetChatWindowMessages(id))` consumes directly.
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

    // AddChatWindowChannel(id, "channel") — `0x4a1000`: (i) the
    // `ChatChannels.dbc` walk compares the argument with each row's **Shortcut**, whole, case-
    // folded; no match → id 0 and the Lua string; a match with no zone text yet → **nil** (0
    // values) and nothing stored; a match → the row's id and **the DBC's own Shortcut string**,
    // not the argument. (ii) The id is the one push. (iii) A name already in the window answers
    // the id and stores nothing. `ChatFrame_AddChannel` keys its bookkeeping on the answer's
    // truthiness, and 0 is truthy.
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

    // RemoveChatWindowChannel(id, "channel" | n) — `0x4a1260`: a numeric argument (`SStrToInt`,
    // n ≠ 0) names joined slot n, whose name is the key — or nothing, when no such slot; 0 (no
    // digits) keeps the string. Then the shortcut walk above resolves the key to the DBC's own
    // spelling, and the slot with that name is freed. 0 values.
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

    // GetChatWindowChannels(id) — name, zoneId, name, zoneId, … which
    // `ChatFrame_RegisterForChannels` walks two at a time.
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

    /// The tuple is nine values wide and in the reference's own order.
    #[test]
    fn get_chat_window_info_answers_the_nine_value_tuple() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetChatWindowInfo(1)").unwrap(), 9);
    }

    /// Trap 1: a stock client has never been told a window's name, so the getter answers `""` and
    /// FrameXML supplies "General"/"Combat Log". Enchantrix's `if ( name == "" )` fallback is only
    /// correct because of this.
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

    /// Trap 2: `shown`/`docked` are `nil` where the record stores 0, because FrameXML branches
    /// on them bare and `0` is true in Lua. At the boot init (`0x4982c0`) window 1
    /// is shown and undocked — it is the dock, `FCF_DockFrame(ChatFrame1, 1)` at file scope —
    /// window 2 is shown with `docked 1`, and 3..7 are neither; the `DOCKED 1`/`DOCKED 2`,
    /// `SHOWN 1`/`SHOWN 0` a stock file carries are what FrameXML's own dock pass then saved.
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
        // And the truthiness FrameXML actually branches on.
        assert!(s
            .eval::<bool>(
                "for i = 3, 7 do local _,_,_,_,_,_,shown = GetChatWindowInfo(i) \
                 if shown then return false end end return true"
            )
            .unwrap());
    }

    /// Trap 3: `docked` is the dock POSITION `FCF_DockFrame(frame, index)` inserts at — the
    /// init's window 2 answers 1 (`0x4982c0`: `mov ds:0xb4ff78, 1`), and once FrameXML has saved
    /// its dock (`FCF_SaveDock`) the stored positions are whatever it wrote.
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

    /// The corpus's own debug-window walk (MikScrollingBattleText, EnhTooltip) runs to completion
    /// and finds nothing — `string.lower(name)` needs a string, and no window is named "debug".
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

    /// A window benilla has no frame for raises rather than answering with an invented row.
    #[test]
    fn a_window_past_the_last_frame_raises() {
        let s = UiScript::new().unwrap();
        assert!(s.eval::<i64>("return (GetChatWindowInfo(8))").is_err());
        assert!(s.eval::<i64>("return (GetChatWindowInfo(0))").is_err());
    }

    /// A size `<= 0` is DROPPED, not stored — the reference's own `jle` at `0x4a14bc`, so a stock
    /// `SIZE 0` cannot be re-written by a caller handing it 0.
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

    /// The three setters round-trip through the getter — and they round-trip through the
    /// reference's BYTE quantisation, not through the float they were handed. `0.4 × 255 = 102`,
    /// and `102/255` is what comes back — and `0.5` comes back `127/255`, not `128/255`, because
    /// `__ftol` truncates (a real client stores 127 there).
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

        // The truncation, on the value that shows it: `0.5 × 255 = 127.5`.
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

    /// Windows are independent: writing window 2 leaves window 1 on the stock row.
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

    /// The setters raise on the same range the getter does — a window benilla has no frame for
    /// cannot be written any more than it can be read.
    #[test]
    fn the_setters_raise_on_a_window_with_no_frame() {
        let s = UiScript::new().unwrap();
        assert!(s.run("SetChatWindowAlpha(8, 1)").is_err());
        assert!(s.run("SetChatWindowColor(0, 1, 1, 1)").is_err());
        assert!(s.run("SetChatWindowSize(8, 14)").is_err());
    }

    /// We clamp where the reference wraps (the module docs' one named divergence): a real client
    /// stores the low byte of `ftol(2.0 × 255) = 510` and answers `254/255`.
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

    /// The host seam: Lua writes queue one index per touched window however many steps the drag
    /// took, the host load path queues nothing, and the drain is a take.
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

    /// A write that changes nothing queues nothing — the reference's own setters are called on
    /// every colour-picker drag step, including the ones that land on the value already stored.
    #[test]
    fn a_write_that_moves_nothing_queues_nothing() {
        let mut s = UiScript::new().unwrap();
        s.run("SetChatWindowAlpha(1, 0)").unwrap();
        s.run("SetChatWindowColor(1, 0, 0, 0)").unwrap();
        s.run("SetChatWindowSize(1, 0)").unwrap();
        assert!(s.take_chat_window_changes().is_empty());
    }

    /// `ChatFrame_OpenChat` queues the text for the app; the optional frame argument is accepted
    /// and dropped (in 1.12 every chat frame shares one edit box — see the install comment).
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

    /// The boot init (`0x4982c0`): window 1 shown with groups 1–10, window 2 shown
    /// at dock index 1 with the 34 `defaultOn` groups of 11–68, the rest empty and hidden.
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
        // The flag coercion (`0x6f1c10`, default 1) and the name cap (`SStrCopy`, 0x20).
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

    /// The flags: a name resolves against `CHATMSGGROUP` case-folded, an unknown name is
    /// nothing, and the answer is the table's order whatever order the calls came in.
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
        // A host-loaded set is normalised the same way.
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

    /// `AddChatWindowChannel`'s legs (`0x4a1000`): a shortcut match stores the
    /// DBC's own spelling with its id, a custom name stores as typed with 0, a shortcut with no
    /// zone text yet is nil and nothing; a duplicate answers the id and stores nothing.
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
