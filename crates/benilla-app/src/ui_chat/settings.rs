//! The chat windows' **saved state** — the whole `chat-cache.txt` record: the per-type colour
//! table and, per window, the tint, alpha, font size, lock, dock, shown flag, name, the message
//! types it shows and the channels it carries (decisions 1589 → 1714 → 1948).
//!
//! The state itself lives in the VM ([`benilla_ui::script::ChatWindowLook`] and the chat-type
//! registry, written by the reference's own `SetChatWindow*`, `Add/RemoveChatWindowMessages`,
//! `Add/RemoveChatWindowChannel`, `ChangeChatColor`, and read straight back out of
//! `GetChatWindowInfo`/`GetChatWindowMessages`/`GetChatWindowChannels`/`GetChatTypeIndex`); this
//! module is the two ends the VM cannot own — **where it comes from at login and where it goes at
//! logout**.
//!
//! ## The grammar is the reference's
//!
//! 1.12 keeps these in `WTF/Account/<ACC>/<REALM>/<CHAR>/chat-cache.txt`, written whole by
//! `0x499a80` and read by `0x498a60` (every line below is the reference's):
//!
//! ```text
//! VERSION 2
//! ADDEDVERSION 2
//! OPTION_GUILD_RECRUITMENT_CHANNEL AUTO
//! CHANNELS                 ← the custom channels the client is in, re-joined at login
//! MyChannel
//! END
//! ZONECHANNELS 18874371    ← the joined zone channels, as bits 1<<(ChannelID-1)
//! COLORS                   ← the chat-type registry, R G B bytes
//! SAY 255 255 255
//! …
//! END
//!
//! WINDOW 1
//! NAME General             ← only when a name was stored (name[0] != 0)
//! SIZE 0
//! COLOR 0 0 0 0            ← R G B A bytes, from the record's packed BGRA quad
//! LOCKED 1
//! DOCKED 1
//! SHOWN 1
//!
//! MESSAGES                 ← the enabled groups of the 68-entry CHATMSGGROUP table, table order
//! SYSTEM
//! …
//! END
//! CHANNELS                 ← this window's CUSTOM channels only — a zone channel is never a name
//! END
//! ZONECHANNELS 18874371    ← this window's zone channels, as bits, masked by the joined set
//! END
//! ```
//!
//! Two consequences worth stating. **The file is the sole source**: the loader zeroes every
//! window's flags before parsing, so a `MESSAGES` block is the set, not an addition — and a
//! window the file does not mention keeps the boot init ([`ChatWindowLook::stock`]). **Zone
//! channels are bits, not names**: the record stores the DBC Shortcut (`General`) with its id,
//! the file stores the bit, and the loader's in-window `ZONECHANNELS` arm turns bits back into
//! `(Shortcut, id)` rows. A file older than `ADDEDVERSION 2` gets the groups added since
//! back-filled (`COMBAT_FACTION_CHANGE`, `MONEY` — into window 2), exactly as the loader does.
//!
//! Ours is `benilla-config/chat/<realm>-<character>.txt`
//! ([`crate::local_state::chat_character_path`]) in that grammar, so a stock file reads here and
//! ours reads there. The reader is as lenient as the reference's — case-insensitive keys, an
//! unknown key skipped rather than failing the line, blank lines nothing — and still accepts the
//! one-line `WINDOW 1  SIZE 0  COLOR …` rows the files written before 1948 hold, so no player's
//! saved looks are lost to the grammar change.
//!
//! `LOCKED` is an `i32` in the record (`CHATWINDOW+0x8c`) but the cache writer booleanises it
//! through `setne`, so only `{0,1}` round-trip there — which is why ours is a `bool`.
//!
//! **The loader's `WINDOW` bound is off by one in the reference** — `0x498d1c` uses `ja` where the
//! array wants `jae`. Ours cannot: the index is bounds-checked at the seam that consumes it
//! (`set_chat_window_looks` uses `get_mut`), and an out-of-range window is simply dropped.
//!
//! ## The login events
//!
//! The loader fires `UPDATE_CHAT_WINDOWS` once and then `UPDATE_CHAT_COLOR` for **every** registry
//! entry — file or no file (`0x4996b9`, `0x499934`). The first is what `FloatingChatFrame_Update`
//! docks, hides and colours the windows from; the second is how a saved colour reaches
//! `ChatTypeInfo` and repaints the lines already in the window (`ChatFrame_OnEvent`'s arm). So does
//! ours, once per character per VM.
//!
//! ## Why per character, and not a CVar
//!
//! Both halves matter. *Per character*, because it is where the reference puts it and because it
//! is what the setting means — a raid alt reading a 40-man combat log wants a solid box where a
//! questing alt wants glass. And *not a CVar*, because `SetChatWindowAlpha` is the API 1.12 addons
//! are written against: an addon that reads a window's alpha calls `GetChatWindowInfo`, and
//! routing benilla's store through `config.toml` instead would have given the same player setting
//! two different names depending on who asked.
//!
//! ## The write posture
//!
//! **Debounced by one quiet second, plus both session edges** — the colour picker's opacity slider
//! drives `FCF_SetChatWindowOpacity` on *every drag step*, so this is a slider, not a discrete
//! edit: [`crate::cvars`]'s `SAVE_QUIET` reasoning applies verbatim ("long enough to coalesce a
//! slider drag, short enough that a crash loses one gesture, not a session"). The edges are
//! `OnExit(InWorld)` and `AppExit`, the same two the camera pose and the saved variables use.

use std::path::PathBuf;

use bevy::prelude::*;

use benilla_ui::script::{ChatTypeColor, ChatWindowLook, UiScript, MESSAGE_GROUPS};

use super::edit::zone_bit;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_script::VmMemo;

/// How long a dirty look sits before the save fires. [`crate::cvars`]'s own constant and its own
/// reasoning — an opacity drag is exactly the gesture it was sized for.
const SAVE_QUIET: std::time::Duration = std::time::Duration::from_secs(1);

/// The file's header — where these values come from and where the law lives. Comment lines; the
/// reference's reader has none, ours skips them.
const HEADER: &str = "\
# benilla chat cache (decisions 1589, 1948) — the reference's chat-cache.txt grammar (written by
# 0x499a80, read by 0x498a60): the custom channels to re-join, the joined zone channels as bits, the
# per-type COLORS table, then one WINDOW block per chat frame — NAME (when one was set), SIZE,
# COLOR r g b a as bytes, LOCKED, DOCKED, SHOWN, the MESSAGES … END list of the groups the window
# shows, its CHANNELS … END list of custom channels, and its zone channels as ZONECHANNELS bits.
# Written whole; the tab menu, /join and ChangeChatColor are what move it.
";

/// **The writer generation, carried in the header comment** — the one thing in
/// this file that is ours and not the reference's grammar, and it is a repair marker, not a
/// format version.
///
/// Every file this client wrote before 2120 composed its `ZONECHANNELS` words from the LIVE
/// channel roster. A save taken while that roster was momentarily empty — the session-end flush
/// racing `end_session_channels` on the same unordered `OnExit(InWorld)` edge — wrote
/// `ZONECHANNELS 0` in the header *and*, through `window bits AND header mask`, in every window
/// block. The next login then rebuilt window 1 with no channels, and the stock
/// `ChatFrame_OnEvent` silently drops every `CHANNEL*` line a window does not carry — so the
/// character lost its `Joined Channel:` notices and all General/Trade speech, permanently, because
/// nothing but the loader's no-file path ever seeds a window's channel list.
///
/// A file without this line is one of those. [`restore_chat_looks`] re-seeds window 1 from
/// `ChatChannels.dbc`'s `INITIAL` rows exactly as the no-file path does, and the next save stamps
/// the marker, so the repair fires once per character and then never again. Bump it only for
/// another repair of our own making; it is a comment line, so a reference client reading this file
/// skips it like the rest of the header.
const WRITER_GENERATION: &str = "# benilla-writer 2 (decision 2120)";

/// Which character's file we are on, where it lives, and whether it is owed a write.
#[derive(Resource, Default)]
pub(super) struct ChatWindowFile {
    path: Option<PathBuf>,
    /// The `(realm, character)` [`Self::path`] was built for. Session-keyed (1290) like the macro
    /// and binding loads: the *same* character coming back still meets a fresh VM whose look table
    /// is back at the stock row.
    identity: VmMemo<Option<(String, String)>>,
    /// Whether **this VM** has unsaved writes. Session-keyed for a reason that is one-way and
    /// therefore worth the wrapper: the values live in the VM, so a plain `bool` surviving a VM
    /// replacement would let a save compose the player's file out of a table that is back at the
    /// stock row — the "refusing to compose the file from nothing" hazard `crate::cvars` guards
    /// against, one store over. A fresh VM starts undirty and cannot write until Lua writes.
    dirty: VmMemo<bool>,
    last_change: Option<std::time::Instant>,
}

/// What a file parses to: the windows it names (0-based index, the record), the `COLORS` rows
/// it carries in file order, and the custom channels its header lists for re-joining.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Parsed {
    pub(super) looks: Vec<(usize, ChatWindowLook)>,
    pub(super) colors: Vec<(String, [u8; 3])>,
    pub(super) joined: Vec<String>,
    /// The header's `ZONECHANNELS` word — the reference's `ds:0xb6e5e0`, loaded from the file as
    /// an overwrite (`0x498d83`). `None` when the file carried no such line, which is what makes
    /// the loader fall back to the DBC seed rather than to a mask of 0.
    pub(super) zone_mask: Option<u32>,
    /// `OPTION_GUILD_RECRUITMENT_CHANNEL` — the auto-join latch `GetGuildRecruitmentMode` returns.
    /// `STANDARD` is 0 and **anything else, a missing word included, is 1**: that
    /// is the reference's own reading (`"STANDARD"` takes the
    /// `0x49ea70(ecx=0)` leg and every other word takes `ecx=1`), and it is why a file with no such
    /// line at all reads as AUTO here, exactly as it does there.
    pub(super) guild_recruitment_auto: bool,
}

impl Default for Parsed {
    fn default() -> Self {
        Self {
            looks: Vec::new(),
            colors: Vec::new(),
            joined: Vec::new(),
            zone_mask: None,
            // The boot value, and it is evidence rather than a guess: all 33 `chat-cache.txt`
            // files the reference client itself wrote in this repo's install say `AUTO`, on
            // characters that never opened the option.
            guild_recruitment_auto: true,
        }
    }
}

/// Render the file exactly as the writer does (`0x499a80`), window order.
///
/// `custom` is [`super::edit::ChannelState::custom`] and supplies the header's `CHANNELS` names —
/// **the custom channels only**: a zone channel is never written as a name, it travels as its bit
/// (`0x499ba1`, which is why all 34 stock files have an empty per-window
/// `CHANNELS` block beside a non-zero `ZONECHANNELS`). Durable, not the live roster, for the
/// mask's own reason: the session end clears the roster before the flush reads it.
///
/// `zone_mask` is [`super::edit::ChannelState::zone_mask`], written raw into the header
/// (`0x499c19`) and ANDed with each window's own bits for its block (`0x49a133`/`0x49a138`). It is
/// **passed in rather than derived from the live roster** — decision 2120, and the whole bug: derived, it
/// was 0 on any save taken while the roster was empty, and `window AND 0` erased the window's
/// channel list for good.
fn render(
    looks: &[ChatWindowLook],
    colors: &[ChatTypeColor],
    custom: &[String],
    zone_mask: u32,
    guild_recruitment_auto: bool,
) -> String {
    let mut out = String::from(HEADER);
    // The repair marker — a comment line, so the reference's own reader skips it.
    out.push_str(WRITER_GENERATION);
    out.push('\n');
    // The latch is the live one now, not a literal: `SetGuildRecruitmentMode`
    // writes it and this is the one place it persists.
    let recruitment = if guild_recruitment_auto {
        "AUTO"
    } else {
        "STANDARD"
    };
    out.push_str(&format!(
        "\nVERSION 2\n\nADDEDVERSION 2\n\nOPTION_GUILD_RECRUITMENT_CHANNEL {recruitment}\n\nCHANNELS\n"
    ));
    for name in custom {
        out.push_str(name);
        out.push('\n');
    }
    out.push_str(&format!("END\n\nZONECHANNELS {zone_mask}\n\nCOLORS\n"));
    for c in colors {
        out.push_str(&format!(
            "{} {} {} {}\n",
            c.name, c.rgb[0], c.rgb[1], c.rgb[2]
        ));
    }
    out.push_str("END\n\n");
    for (i, l) in looks.iter().enumerate() {
        out.push_str(&format!("WINDOW {}\n", i + 1));
        if !l.name.is_empty() {
            out.push_str(&format!("NAME {}\n", l.name));
        }
        out.push_str(&format!(
            "SIZE {}\nCOLOR {} {} {} {}\nLOCKED {}\nDOCKED {}\nSHOWN {}\n\nMESSAGES\n",
            l.font_size,
            l.r,
            l.g,
            l.b,
            l.a,
            i32::from(l.locked),
            l.docked.unwrap_or(0),
            i32::from(l.shown),
        ));
        for m in &l.messages {
            out.push_str(m);
            out.push('\n');
        }
        out.push_str("END\n\nCHANNELS\n");
        let mut window_mask = 0;
        for (name, id) in &l.channels {
            if *id == 0 {
                out.push_str(name);
                out.push('\n');
            } else {
                window_mask |= zone_bit(*id);
            }
        }
        out.push_str(&format!(
            "END\n\nZONECHANNELS {}\n\nEND\n\n",
            window_mask & zone_mask
        ));
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    /// Top level, or inside a `WINDOW` whose keys arrive one per line.
    Top,
    /// The header's `CHANNELS … END` — the custom channels to re-join.
    Joined,
    Colors,
    Messages,
    Channels,
}

/// Parse a file. `rows` is `ChatChannels.dbc` as `(id, Shortcut)`, which the in-window
/// `ZONECHANNELS` arm needs to turn bits back into `(Shortcut, id)` channel rows.
fn parse(text: &str, rows: &[(u32, String)]) -> Parsed {
    let mut out = Parsed::default();
    let mut current: Option<(usize, ChatWindowLook)> = None;
    let mut block = Block::Top;
    let mut added_version: u8 = 0;
    let byte = |s: Option<&str>| -> u8 { s.and_then(|v| v.parse::<u8>().ok()).unwrap_or(0) };
    let flush = |current: &mut Option<(usize, ChatWindowLook)>, out: &mut Parsed| {
        if let Some(w) = current.take() {
            out.looks.push(w);
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let Some(head) = it.next() else { continue };
        let is_end = head.eq_ignore_ascii_case("END");
        match block {
            Block::Colors => {
                if is_end {
                    block = Block::Top;
                } else {
                    let rgb = [byte(it.next()), byte(it.next()), byte(it.next())];
                    out.colors.push((head.to_ascii_uppercase(), rgb));
                }
                continue;
            }
            Block::Joined => {
                if is_end {
                    block = Block::Top;
                } else {
                    out.joined.push(line.to_string());
                }
                continue;
            }
            Block::Messages => {
                if is_end {
                    block = Block::Top;
                } else if let Some((_, look)) = current.as_mut() {
                    look.messages.push(head.to_ascii_uppercase());
                }
                continue;
            }
            Block::Channels => {
                if is_end {
                    block = Block::Top;
                } else if let Some((_, look)) = current.as_mut() {
                    // The loader takes the first word, id 0 — **unless the word is a
                    // `ChatChannels.dbc` Shortcut**, in which case it is a zone channel that lost
                    // its id and this restores it.
                    //
                    // Not a heuristic: the reference's engine resolves every name through the
                    // shortcut walk before it stores one (`AddChatWindowChannel 0x4a1000`),
                    // so a *custom* entry can never be spelled exactly
                    // like a built-in shortcut — all 34 stock files carry an empty per-window
                    // `CHANNELS` block beside a non-zero `ZONECHANNELS`. Ours could, because the
                    // VM's catalog was still empty when the window was written, and `General` came
                    // back as a custom channel with the id the id-match at `ChatFrame.lua:1379`
                    // needs. The write side is fixed; this is what heals the files it already
                    // damaged, and it is idempotent — a healed file has no such name to match.
                    let id = rows
                        .iter()
                        .find(|(_, shortcut)| shortcut.eq_ignore_ascii_case(head))
                        .map_or(0, |(id, _)| *id);
                    look.channels.push((head.to_string(), id));
                }
                continue;
            }
            Block::Top => {}
        }
        if head.eq_ignore_ascii_case("COLORS") {
            block = Block::Colors;
            continue;
        }
        if head.eq_ignore_ascii_case("WINDOW") {
            flush(&mut current, &mut out);
            let Some(index) = it.next().and_then(|n| n.parse::<usize>().ok()) else {
                warn!("chat cache: WINDOW line with no number ignored: {line}");
                continue;
            };
            if index == 0 {
                continue;
            }
            let mut look = ChatWindowLook::stock(index - 1);
            // The pre-1948 one-line row: the keys follow on the same line.
            while let Some(key) = it.next() {
                apply_key(&mut look, key, &mut it, rows);
            }
            current = Some((index - 1, look));
            continue;
        }
        let Some((_, look)) = current.as_mut() else {
            if head.eq_ignore_ascii_case("CHANNELS") {
                block = Block::Joined;
            } else if head.eq_ignore_ascii_case("ADDEDVERSION") {
                added_version = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            } else if head.eq_ignore_ascii_case("ZONECHANNELS") {
                // The reference's top-level arm is an OVERWRITE of `ds:0xb6e5e0` (`0x498d83`), not
                // an OR — the file is the whole truth about which zone channels this character
                // holds.
                out.zone_mask = it.next().and_then(|v| v.trim().parse::<u32>().ok());
            } else if head.eq_ignore_ascii_case("OPTION_GUILD_RECRUITMENT_CHANNEL") {
                // The reference's own test, whole and case-folded: `STANDARD` is the only word
                // that means 0; every other word — and no word at all — means 1.
                out.guild_recruitment_auto = !it
                    .next()
                    .is_some_and(|w| w.eq_ignore_ascii_case("STANDARD"));
            }
            // VERSION is the reference's and not read here.
            continue;
        };
        if is_end {
            flush(&mut current, &mut out);
        } else if head.eq_ignore_ascii_case("MESSAGES") {
            look.messages.clear();
            block = Block::Messages;
        } else if head.eq_ignore_ascii_case("CHANNELS") {
            look.channels.retain(|(_, id)| *id != 0);
            block = Block::Channels;
        } else if head.eq_ignore_ascii_case("NAME") {
            look.name = line[4..].trim().to_string();
        } else {
            apply_key(look, head, &mut it, rows);
        }
    }
    flush(&mut current, &mut out);
    // The loader's EOF back-fill (`0x49967c`): a file older than the groups added since gets them,
    // into window 1 for the first ten groups and window 2 otherwise — the two `addedVersion` rows
    // are both window 2's.
    if added_version < 2 {
        for (i, (name, on, ver)) in MESSAGE_GROUPS.iter().enumerate() {
            if *on && *ver > added_version {
                let target = usize::from(i >= 10);
                if let Some((_, look)) = out.looks.iter_mut().find(|(w, _)| *w == target) {
                    if !look.messages.iter().any(|m| m == name) {
                        look.messages.push((*name).to_string());
                    }
                }
            }
        }
    }
    for (_, look) in &mut out.looks {
        look.normalize_messages();
    }
    out
}

/// One `KEY value…` of a window block, whichever line it arrived on.
fn apply_key<'a>(
    look: &mut ChatWindowLook,
    key: &str,
    it: &mut impl Iterator<Item = &'a str>,
    rows: &[(u32, String)],
) {
    let byte = |s: Option<&str>| -> u8 { s.and_then(|v| v.parse::<u8>().ok()).unwrap_or(0) };
    let flag = |s: Option<&str>| -> bool { s.is_none_or(|v| v.trim() != "0") };
    if key.eq_ignore_ascii_case("SIZE") {
        look.font_size = it
            .next()
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0)
            .max(0);
    } else if key.eq_ignore_ascii_case("COLOR") {
        look.r = byte(it.next());
        look.g = byte(it.next());
        look.b = byte(it.next());
        look.a = byte(it.next());
    } else if key.eq_ignore_ascii_case("LOCKED") {
        look.locked = flag(it.next());
    } else if key.eq_ignore_ascii_case("DOCKED") {
        look.docked = it
            .next()
            .and_then(|v| v.trim().parse::<u8>().ok())
            .filter(|p| *p > 0);
    } else if key.eq_ignore_ascii_case("SHOWN") {
        look.shown = flag(it.next());
    } else if key.eq_ignore_ascii_case("ZONECHANNELS") {
        // The in-window arm: every DBC row whose bit is set joins the window as (Shortcut, id).
        let mask = it
            .next()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0);
        for (id, shortcut) in rows {
            if mask & zone_bit(*id) != 0
                && !look
                    .channels
                    .iter()
                    .any(|(c, _)| c.eq_ignore_ascii_case(shortcut))
            {
                look.channels.push((shortcut.clone(), *id));
            }
        }
    }
}

/// The `(id, Shortcut)` rows the parser needs, off the loaded `ChatChannels.dbc`.
fn shortcut_rows(channels: &super::edit::ChannelState) -> Vec<(u32, String)> {
    channels
        .channels
        .rows()
        .iter()
        .map(|r| (r.id, r.shortcut.clone()))
        .collect()
}

/// Restore the file into a fresh VM — once per character per VM — and fire the loader's two
/// events, file or no file.
///
/// **This runs INSIDE the world-entry UI load, not from `Update`**, and the two
/// events are why. It is the only caller of `UPDATE_CHAT_WINDOWS`, and that event is the only
/// thing that registers a chat frame for any `CHAT_MSG_*` — `ChatFrame_OnEvent`'s arm (ref
/// `ChatFrame.lua` l.1261-1273) is what calls
/// `ChatFrame_RegisterForMessages(GetChatWindowMessages(this:GetID()))`. Anything fired before it
/// reaches no window and is gone with no trace (1784: a rejected `AddMessage` is a silent skip).
/// As an `Update` system this landed on the same frame the parked VM came back and the whole
/// queued login burst drained, with no ordering between them — so the server's MOTD, sent as
/// `CHAT_MSG_SYSTEM` right after `SMSG_LOGIN_VERIFY_WORLD`, was routed at a window registered for
/// nothing. Measured on a live login: `net: server says — Welcome to World of Warcraft!` at
/// `…45.155929`, this restore at `…45.157625`.
///
/// It is also the only caller of the `UPDATE_CHAT_COLOR` burst, and that one has to precede
/// `PLAYER_LOGIN`. The burst's `WHISPER` row mirrors itself into `ChatTypeInfo["REPLY"]`
/// (`ChatFrame.lua` l.1357-1365) — an entry the engine's 94-row registry does not carry, so its
/// `.id` is **0** (`ui_script::chat_tests` asserts exactly that) — and `UpdateColorByID(0, …)`
/// repaints every line already in the window whose id is 0, which is every line printed with no
/// explicit colour. `AceConsole-2.0`'s `Print` is exactly that call
/// (`AddMessage(text, nil, nil, nil, nil, 5)`), so running the burst after the addons had printed
/// repainted their login lines whisper-pink: measured `(255,128,255)` on Bartender2's login line
/// against the reference's `(255,255,255)`, in a run where the same `AddMessage` a second later
/// came out white.
pub(crate) fn restore_chat_looks(world: &mut World, script: &mut UiScript) {
    let Some(id) = world
        .get_resource::<crate::char_select::Roster>()
        .and_then(crate::ui_macro::identity)
    else {
        return;
    };
    // The `ChannelState` reads are taken as owned rows up front: the restore needs the DBC
    // shortcut table and the auto-join rows while it also holds `ChatWindowFile` mutably, and a
    // `&mut World` hands out one resource borrow at a time.
    let Some(channels) = world.get_resource::<super::edit::ChannelState>() else {
        return;
    };
    let rows = shortcut_rows(channels);
    let auto_rows: Vec<(String, u32)> = channels
        .channels
        .auto_join_rows()
        .map(|r| (r.shortcut.clone(), r.id))
        .collect();
    let commands = world.get_resource::<NetCommands>().map(|c| c.0.clone());
    // The DBC seed — every `ChatChannels.dbc` row the client joins by itself (`flags & 1`).
    let seed_mask = auto_rows.iter().fold(0, |m, (_, id)| m | zone_bit(*id));
    // `ChatWindowFile`'s borrow is scoped, because the mask has to go home to `ChannelState`
    // afterwards and a `&mut World` hands out one resource borrow at a time.
    let mut parsed;
    {
        let Some(mut file) = world.get_resource_mut::<ChatWindowFile>() else {
            return;
        };
        if file.identity.get(script).as_ref() == Some(&id) {
            return; // already restored for this character, into the VM that is live now
        }
        let who = format!("{} on {}", id.1, id.0);
        file.path = crate::local_state::chat_character_path(&id.0, &id.1);
        *file.identity.get(script) = Some(id);
        *file.dirty.get(script) = false;
        file.last_change = None;
        let text = file
            .path
            .as_ref()
            .and_then(|path| match std::fs::read_to_string(path) {
                Ok(t) => Some(t),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    warn!("chat cache: cannot read {}: {e}", path.display());
                    None
                }
            });
        let had_file = text.is_some();
        // A file our own pre-2120 writer damaged: no marker, so its `ZONECHANNELS` words may have been
        // composed from an empty roster and its window channel lists erased with them. Repaired below,
        // once — the next save stamps the marker.
        let damaged = text
            .as_deref()
            .is_some_and(|t| !t.contains(WRITER_GENERATION));
        parsed = text.map(|t| parse(&t, &rows)).unwrap_or_default();
        if !had_file {
            // The loader's no-file path (`0x4997ad`): the mask is seeded from those rows, and
            // window 1's channel slots get each of them as `(Shortcut, id)` — the rows
            // `ChatFrame_RegisterForChannels` will match zone speech against by id. The rest of the
            // record is the boot init the VM already holds.
            let mut general = ChatWindowLook::stock(0);
            general.channels = auto_rows.clone();
            parsed.looks.push((0, general));
        }
        if !parsed.looks.is_empty() || !parsed.colors.is_empty() {
            info!(
                "chat cache: {} windows, {} colour rows, {} custom channels restored",
                parsed.looks.len(),
                parsed.colors.len(),
                parsed.joined.len()
            );
        }
        if damaged {
            // **The one-time repair** ([`WRITER_GENERATION`]). Re-seed window 1 the way
            // the loader's no-file path seeds it, and OR the DBC bits back into the mask. Additive and
            // deduplicated, exactly like the in-window `ZONECHANNELS` arm (`0x499332`-`0x4994e7`), so a
            // file that survived intact is left alone and one that was zeroed gets its channels back.
            if let Some((_, general)) = parsed.looks.iter_mut().find(|(w, _)| *w == 0) {
                for (shortcut, id) in &auto_rows {
                    if !general
                        .channels
                        .iter()
                        .any(|(c, _)| c.eq_ignore_ascii_case(shortcut))
                    {
                        general.channels.push((shortcut.clone(), *id));
                    }
                }
            }
            parsed.zone_mask = Some(parsed.zone_mask.unwrap_or(0) | seed_mask);
            // Owed a write, so the repair is genuinely ONCE: the next save composes the repaired
            // record and stamps [`WRITER_GENERATION`], and this branch never runs for the character
            // again. `last_change` is `None` here, so that save is the very next frame's.
            *file.dirty.get(script) = true;
            info!("chat cache: repaired a pre-2120 file's zone channels for {who}");
        }
    } // …and `ChatWindowFile`'s borrow ends here.
      // The mask is durable state from here on: the file's word when it carried one,
      // the DBC seed when it did not, and from then on the confirmed joins' own OR.
    let mask = parsed.zone_mask.unwrap_or(seed_mask);
    if let Some(mut channels) = world.get_resource_mut::<super::edit::ChannelState>() {
        // `Some` is the reference's "chat system ready" flag (`0x499a18`): the walk and the
        // guild-recruitment cascade both hold until this line has run.
        channels.zone_mask = Some(mask);
        // The custom re-join list is durable on the same terms: seated from the file here, then
        // moved only by a confirmed join and an explicit leave (`ChannelState::custom`).
        channels.custom = parsed.joined.clone();
    }
    script.set_guild_recruitment_mode(u8::from(parsed.guild_recruitment_auto));
    script.set_chat_colors(parsed.colors);
    script.set_chat_window_looks(parsed.looks);
    // UPDATE_CHAT_WINDOWS once, then UPDATE_CHAT_COLOR for every registry entry, on the file
    // path and the no-file path alike (`0x4996b9`, `0x499934`).
    script.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
    let renorm = |b: u8| f64::from(b as f32 * (1.0f32 / 255.0f32));
    for entry in script.chat_colors() {
        script.fire_event(
            "UPDATE_CHAT_COLOR",
            vec![
                benilla_ui::script::ScriptValue::Str(entry.name),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[0])),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[1])),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[2])),
            ],
        );
    }
    // The header's CHANNELS: the custom channels the character was in, re-joined the way the
    // reference re-joins them at login (the zone channels are the zone walk's, not the file's).
    if let Some(commands) = commands {
        for name in parsed.joined {
            let _ = commands.send(ClientCommand::JoinChannel {
                name,
                password: String::new(),
            });
        }
    }
}

/// A Lua-side write landed — arm the debounce.
fn watch_chat_looks(script: Option<NonSendMut<UiScript>>, mut file: ResMut<ChatWindowFile>) {
    let Some(mut script) = script else { return };
    let moved = !script.take_chat_window_changes().is_empty();
    let coloured = script.take_chat_color_changes();
    // `SetGuildRecruitmentMode` is a Lua write into the same file — a host seat at
    // login does not arm this, only a script call does.
    let recruitment = script.take_guild_recruitment_change();
    if !(moved || coloured || recruitment) {
        return;
    }
    *file.dirty.get(&script) = true;
    file.last_change = Some(std::time::Instant::now());
}

fn write(script: &UiScript, channels: &super::edit::ChannelState, path: &std::path::Path) {
    // Never compose a file from an unseated mask: `ZONECHANNELS 0` in every block is exactly the
    // damage 2120 repaired, and a `None` here means the character's file was never read.
    let Some(zone_mask) = channels.zone_mask else {
        warn!(
            "chat cache: refusing to write {} — the zone mask has not been seated",
            path.display()
        );
        return;
    };
    let body = render(
        &script.chat_window_looks(),
        &script.chat_colors(),
        &channels.custom,
        zone_mask,
        script.guild_recruitment_mode() != 0,
    );
    if let Err(e) = crate::local_state::write_atomic(path, &body) {
        warn!("chat cache: cannot write {}: {e}", path.display());
    }
}

/// Is a write owed **now**? Two reasons, and they gate differently:
///
/// - **A flush** (`exiting`) — the session end or the window close — writes **unconditionally**,
///   provided this VM restored the character's file (`restored`): the reference rewrites the
///   whole file at chat teardown (`0x499a80` from `0x490c55`) with no dirty flag in the way
///   (`[0xb6e5c4]` has three writers and no reader). The
///   `ZONECHANNELS` word and the guild-recruitment latch are host state that no Lua write ever
///   dirties — gating the flush on `dirty` is how a `/leave General` was persisted only when the
///   player also happened to drag a window, and 2120's "durable state" was durable on paper.
/// - **The debounce** writes only what Lua moved (`dirty`) once the quiet time has passed — our
///   own improvement over the reference's teardown-only write, so a crash loses at most a second
///   of drags; it never fires for host-only changes, which the flush carries.
///
/// `restored` is the guard that keeps a fresh VM — whose look table is back at the stock row —
/// from composing the player's file out of nothing: it is exactly "the file was read into this
/// VM", the `identity` memo.
fn owes_write(exiting: bool, restored: bool, dirty: bool, quiet: bool) -> bool {
    if exiting {
        restored
    } else {
        dirty && quiet
    }
}

/// The write itself, on the terms [`owes_write`] set.
fn flush(
    script: &UiScript,
    channels: &super::edit::ChannelState,
    file: &mut ChatWindowFile,
    exiting: bool,
) {
    let restored = file.identity.get(script).is_some();
    let dirty = *file.dirty.get(script);
    let quiet = file.last_change.is_none_or(|t| t.elapsed() >= SAVE_QUIET);
    if !owes_write(exiting, restored, dirty, quiet) {
        return;
    }
    if let Some(path) = file.path.clone() {
        write(script, channels, &path);
    }
    *file.dirty.get(script) = false;
}

/// The debounced save, and the `AppExit` flush.
fn save_chat_looks(
    script: Option<NonSendMut<UiScript>>,
    channels: Res<super::edit::ChannelState>,
    mut file: ResMut<ChatWindowFile>,
    mut exits: MessageReader<AppExit>,
) {
    let exiting = exits.read().next().is_some();
    let Some(script) = script else { return };
    flush(&script, &channels, &mut file, exiting);
}

/// The session-end flush — the reference's chat teardown (`0x499a80` from `0x490c55`), called
/// from [`crate::ui_script::end_ui_session`] beside [`crate::cvars::fold_dying_vm_cvars`]:
/// **after** the shutdown events, **before** the VM is replaced.
///
/// **It is not an `OnExit(InWorld)` system, and that is the whole point.** `/reload` rebuilds the
/// VM without ever leaving the world (1291), so that edge does not fire for it — and nothing else
/// covers the gap, because both halves of the debounce beside it are VM-keyed: [`ChatWindowFile::dirty`] is
/// a [`VmMemo`] that resets to `false` with the new VM, and [`restore_chat_looks`] then re-seats
/// the look table from the file **on disk**. So a window dragged, resized, recoloured or renamed
/// within [`SAVE_QUIET`] of a `/reload` was written nowhere and read straight back stale: the
/// player's change was discarded with no error and no trace.
///
/// `end_ui_session` is the one place every root meets — it is what the `OnExit(InWorld)`
/// registration runs, and what `run_pending_reload` calls — so one call here covers the logout,
/// the character switch and the reload. It also retires the hand-stated ordering this flush used
/// to need: two unconstrained systems on one `OnExit` edge are placed by the executor, and that
/// placement was measured (bevy 0.18) to move with nothing but their registration positions.
/// Being *inside* the ender is an ordering no arrangement can lose.
///
/// A session whose UI never loaded writes nothing, by construction rather than by a guard:
/// `restored` is false for a VM that never read the character's file, and [`owes_write`] refuses
/// on it — which is the same protection against composing the player's file out of a stock table
/// that [`ChatWindowFile::dirty`] exists for.
pub(crate) fn fold_dying_vm_chat_cache(world: &mut World) {
    // A world with no chat-cache state never added the plugin — a test world or a stripped
    // scenario. Checked up front so the fetch below can be non-optional, exactly as
    // `fold_dying_vm_cvars` checks its own.
    if !world.contains_resource::<ChatWindowFile>() {
        return;
    }
    // `resource_scope` rather than a `SystemState`: it lifts the file out for the call, which
    // leaves the VM and the channel roster as two plain shared borrows of the world — the whole
    // fetch, with no system machinery and nothing to keep in step with the signature.
    world.resource_scope(|world, mut file: Mut<ChatWindowFile>| {
        let (Some(script), Some(channels)) = (
            world.get_non_send_resource::<UiScript>(),
            world.get_resource::<super::edit::ChannelState>(),
        ) else {
            return;
        };
        flush(script, channels, &mut file, true);
    });
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<ChatWindowFile>()
        // **The restore is not here.** `UPDATE_CHAT_WINDOWS`/`UPDATE_CHAT_COLOR` have to precede
        // the session's first chat line and `PLAYER_LOGIN` respectively, and no `run_if` can buy
        // that: 1978's UI-is-up gate (`ingame_ui_up`) puts the restore on exactly the frame the
        // parked VM comes back and `feed_chat` drains the whole queued login burst, with nothing
        // ordering the two. It is called from `ui_script::lifecycle`'s world-entry load instead —
        // [`restore_chat_looks`]. Only the watcher belongs in `Update`.
        .add_systems(
            Update,
            // After the tick: every write it watches is Lua's.
            watch_chat_looks
                .after(crate::ui_script::UiInput)
                .in_set(crate::char_select::InWorldGated),
        )
        .add_systems(
            Update,
            save_chat_looks
                .after(crate::ui_script::UiInput)
                .in_set(crate::char_select::InWorldGated),
        );
    // **The session-end flush is not registered here.** It is called from inside
    // `ui_script::end_ui_session` ([`fold_dying_vm_chat_cache`]) so that it covers `/reload` — a
    // VM rebuild that never crosses `OnExit(InWorld)` — as well as the logout edge, and so that
    // its position relative to the ender is structural instead of a registration-order bet.
    // The quit flush rides the exit edge rather than `Update` for decision 1528's reason: the
    // close button's `AppExit` is not written until `PostUpdate`, so a save chained beside the
    // watcher would lose the last second of drags to the process ending.
    crate::shutdown::on_app_exit(app, save_chat_looks.into_configs());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record for WINDOW `n` (1-based) — the window's own boot-init row with the look fields
    /// set. The window number is a parameter because `DOCKED`, `SHOWN` and `MESSAGES` differ per
    /// window at init, so an expectation that does not say which window it is cannot be right
    /// for all of them.
    fn look(n: usize, r: u8, g: u8, b: u8, a: u8, font_size: i32) -> ChatWindowLook {
        ChatWindowLook {
            r,
            g,
            b,
            a,
            font_size,
            locked: true,
            ..ChatWindowLook::stock(n - 1)
        }
    }

    /// `ChatChannels.dbc` as the loader hands it to [`parse`] — a subset of the shipped six, but
    /// carrying all three `INITIAL` rows, because the ids 1 / 2 / 22 are what the window blocks are
    /// actually about.
    fn rows() -> Vec<(u32, String)> {
        vec![
            (1, "General".to_string()),
            (2, "Trade".to_string()),
            (22, "LocalDefense".to_string()),
            (24, "LookingForGroup".to_string()),
        ]
    }

    fn colors(rows: &[(&str, [u8; 3])]) -> Vec<ChatTypeColor> {
        rows.iter()
            .map(|(n, rgb)| ChatTypeColor {
                name: (*n).to_string(),
                rgb: *rgb,
            })
            .collect()
    }

    /// The file round-trips — every field of the record, the colour rows, the header's custom
    /// channels, the zone bits back into `(Shortcut, id)` rows — indices included.
    #[test]
    fn the_file_round_trips() {
        let mut named = look(3, 9, 8, 7, 6, 12);
        named.name = "Loot & Trade".into();
        named.shown = true;
        named.messages = vec!["LOOT".into(), "MONEY".into()];
        named.channels = vec![("MyChan".into(), 0), ("Trade".into(), 2)];
        let looks = vec![
            look(1, 0, 0, 0, 64, 14),
            look(2, 255, 128, 0, 255, 0),
            named,
        ];
        let colors = colors(&[("SAY", [1, 2, 3]), ("CHANNEL7", [4, 5, 6])]);
        // The mask and the custom list are the character's durable state (2120), passed in rather
        // than derived from the live roster.
        let text = render(&looks, &colors, &["MyChan".to_string()], 0b11, true);
        assert!(
            text.contains("\nCHANNELS\nMyChan\nEND\n\nZONECHANNELS 3\n"),
            "{text}"
        );
        let parsed = parse(&text, &rows());
        let mut expect = looks.clone();
        // The zone channel comes back by its Shortcut row, after the custom names.
        expect[2].channels = vec![("MyChan".into(), 0), ("Trade".into(), 2)];
        assert_eq!(
            parsed.looks,
            vec![
                (0, expect[0].clone()),
                (1, expect[1].clone()),
                (2, expect[2].clone())
            ],
            "0-based indices, values intact"
        );
        assert_eq!(
            parsed.colors,
            vec![
                ("SAY".to_string(), [1, 2, 3]),
                ("CHANNEL7".to_string(), [4, 5, 6])
            ]
        );
        assert_eq!(parsed.joined, vec!["MyChan".to_string()]);
    }

    /// **A window `CHANNELS` name that IS a DBC shortcut is a zone channel, not a custom one**
    /// — the repair for the files our own writer damaged.
    ///
    /// The director's `Onewarrior` file carried this verbatim: `CHANNELS / General / LocalDefense
    /// / END` beside `ZONECHANNELS 2`, so window 1 held General and LocalDefense with id **0** and
    /// only Trade kept its bit. The stock `ChatFrame_OnEvent` matches a channel line by
    /// `zoneChannelList[index] == arg7` (`ChatFrame.lua:1379`), so that character silently lost
    /// every General and LocalDefense line — join notices and speech alike — while Trade still
    /// worked.
    ///
    /// The write side no longer produces it (the VM's catalog is fed before any verb can ask), and
    /// this heals what it already wrote. Idempotent by construction: a healed file writes the name
    /// as a bit, so there is no name left here to match.
    #[test]
    fn a_window_channel_named_like_a_dbc_shortcut_regains_its_id() {
        let text = "WINDOW 1\nSIZE 0\n\nMESSAGES\nEND\n\nCHANNELS\nGeneral\nMyChan\n                    LocalDefense\nEND\n\nZONECHANNELS 2\n\nEND\n";
        let parsed = parse(text, &rows());
        assert_eq!(
            parsed.looks[0].1.channels,
            vec![
                ("General".to_string(), 1),
                ("MyChan".to_string(), 0),
                ("LocalDefense".to_string(), 22),
                ("Trade".to_string(), 2),
            ],
            "the two shortcuts come back as their DBC rows; the genuinely custom name keeps id 0, \
             and the ZONECHANNELS bit still contributes Trade"
        );

        // …and the next save writes them as bits again rather than as names, which is what makes
        // the repair one-way.
        let out = render(&[parsed.looks[0].1.clone()], &[], &[], 0x0020_0003, false);
        let window = out.split("WINDOW 1").nth(1).unwrap();
        assert!(window.contains("CHANNELS\nMyChan\nEND"), "{out}");
        assert!(window.contains("ZONECHANNELS 2097155\n"), "{out}");
    }

    /// A window's zone bits are masked by the joined set — a zone channel the client has left
    /// is not written for the window either, the way the writer ANDs the two words.
    #[test]
    fn a_windows_zone_bits_are_masked_by_the_joined_set() {
        let mut w = look(1, 0, 0, 0, 0, 0);
        w.channels = vec![("General".into(), 1), ("Trade".into(), 2)];
        // The mask holds General alone — Trade was explicitly left, so the window's own Trade
        // bit is ANDed away on the way out.
        let text = render(&[w], &[], &[], 1, true);
        let windows: Vec<&str> = text.split("WINDOW 1").collect();
        assert!(windows[1].contains("ZONECHANNELS 1\n"), "{text}");
    }

    /// **The bug 2120 fixes, in one assertion: an empty roster must not erase a window's
    /// channels.**
    ///
    /// The mask used to be `joined.fold(|m, (_, id)| m | zone_bit(id))`, so a save taken while
    /// `ChannelState::joined` was momentarily empty — the session-end flush racing
    /// `end_session_channels` on the same unordered `OnExit(InWorld)` edge — wrote `ZONECHANNELS
    /// 0` in the header AND, through `window bits AND header mask`, in every window block. Nothing
    /// but the loader's no-file path ever seeds a window's channel list, so the next login rebuilt
    /// window 1 with none, and the stock `ChatFrame_OnEvent` silently drops every `CHANNEL*` line
    /// a window does not carry: no `Joined Channel:` notices and no General/Trade speech, for
    /// good. Ten of the twenty files in this repo's own config folder had reached that state.
    #[test]
    fn an_empty_roster_does_not_erase_a_windows_zone_channels() {
        let mut w = look(1, 0, 0, 0, 0, 0);
        w.channels = vec![("General".into(), 1), ("Trade".into(), 2)];
        // The character IS in both channels — the mask says so — but the live roster is empty,
        // which is exactly the state a session-end save can be taken in.
        let text = render(&[w], &[], &[], 0b11, true);
        assert!(
            text.contains("\nZONECHANNELS 3\n"),
            "the header carries the durable mask, not the roster's shadow: {text}"
        );
        let parsed = parse(&text, &rows());
        assert_eq!(parsed.zone_mask, Some(0b11), "the header word round-trips");
        assert_eq!(
            parsed.looks[0].1.channels,
            vec![("General".to_string(), 1), ("Trade".to_string(), 2)],
            "window 1 keeps both channels — pre-2120 this came back empty and stayed empty"
        );
    }

    /// A file our own pre-2120 writer zeroed carries no [`WRITER_GENERATION`] line, and that is
    /// the whole discriminator: the repair is keyed on the marker, not on guessing whether a
    /// player meant to leave every zone channel.
    #[test]
    fn a_written_file_carries_the_writer_generation_and_a_damaged_one_does_not() {
        let text = render(&[ChatWindowLook::stock(0)], &[], &[], 0b11, true);
        assert!(
            text.contains(WRITER_GENERATION),
            "every file we write is stamped, so the repair fires once: {text}"
        );
        // The shape the damage takes: a header the reference's own reader accepts, with no marker.
        let damaged = "VERSION 2\n\nCHANNELS\nEND\n\nZONECHANNELS 0\n\n\
             WINDOW 1\nSIZE 0\nSHOWN 1\n\nMESSAGES\nSYSTEM\nEND\n\n\
             CHANNELS\nEND\n\nZONECHANNELS 0\n\nEND\n";
        assert!(!damaged.contains(WRITER_GENERATION));
        let parsed = parse(damaged, &rows());
        assert_eq!(parsed.zone_mask, Some(0), "the zeroed header parses as 0");
        assert!(
            parsed.looks[0].1.channels.is_empty(),
            "and window 1 comes back with no channels — the symptom"
        );
    }

    /// The mask is durable state: a confirmed join sets a bit, an explicit leave clears one, and
    /// a custom channel (no DBC id) has no bit at all.
    #[test]
    fn the_zone_mask_moves_on_join_and_explicit_leave_only() {
        let mut state = super::super::edit::ChannelState {
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: 1,
                    flags: 0x11,
                    pattern: "General - %s".into(),
                    shortcut: "General".into(),
                },
                benilla_formats::ChatChannelRow {
                    id: 2,
                    flags: 0x3b,
                    pattern: "Trade - %s".into(),
                    shortcut: "Trade".into(),
                },
            ]),
            ..Default::default()
        };
        // Unseated, a join is dropped rather than absorbed into a word nobody has read yet.
        state.note_zone_channel_joined("General - Elwynn Forest");
        assert_eq!(state.zone_mask, None, "not seated: nothing to OR into");
        state.zone_mask = Some(0);
        state.note_zone_channel_joined("General - Elwynn Forest");
        state.note_zone_channel_joined("Trade - City");
        assert_eq!(state.zone_mask, Some(0b11));
        state.note_zone_channel_joined("MyChan");
        assert_eq!(state.zone_mask, Some(0b11), "a custom channel has no bit");
        // The clear is keyed on the SLOT the wire name finds, and its own id (
        // `0x49f0f4`–`0x49f11a`) — a name we hold no slot for clears nothing.
        state.note_zone_channel_left("Trade - City");
        assert_eq!(state.zone_mask, Some(0b11), "no slot carries it yet");
        state.claim_slot("Trade - City");
        state.note_zone_channel_left("Trade - City");
        assert_eq!(
            state.zone_mask,
            Some(0b01),
            "an explicit leave clears one bit"
        );
    }

    /// The custom re-join list moves on the same two edges as the mask — a confirmed join of a
    /// channel with no DBC id, and an explicit leave — and a zone channel never enters it.
    #[test]
    fn the_custom_list_moves_on_join_and_explicit_leave_only() {
        let mut state = super::super::edit::ChannelState {
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: 1,
                    flags: 0x11,
                    pattern: "General - %s".into(),
                    shortcut: "General".into(),
                },
            ]),
            ..Default::default()
        };
        state.note_custom_channel_joined("General - Elwynn Forest");
        assert!(state.custom.is_empty(), "a zone channel travels as its bit");
        state.note_custom_channel_joined("MyChan");
        state.note_custom_channel_joined("mychan");
        assert_eq!(state.custom, vec!["MyChan".to_string()], "once, by name");
        state.note_custom_channel_left("MYCHAN");
        assert!(state.custom.is_empty(), "an explicit leave drops it");
    }

    /// **The teardown write is unconditional; only the debounce reads the dirty flag** (decision
    /// 2144). A `/leave General` moves host state alone — the mask — and the reference persists
    /// it because its teardown saver has no dirty flag; ours gated the same write on a Lua-side
    /// flag, so the leave came back on the next login unless a window had also been dragged.
    #[test]
    fn a_flush_writes_whatever_was_restored_and_the_debounce_writes_only_lua_moves() {
        // The flush: restored is the whole condition.
        assert!(owes_write(true, true, false, false));
        assert!(owes_write(true, true, true, true));
        assert!(
            !owes_write(true, false, true, true),
            "a VM that never read the file has nothing of the player's to write"
        );
        // The debounce: dirty AND quiet, never on host-only state.
        assert!(owes_write(false, true, true, true));
        assert!(!owes_write(false, true, true, false), "still being dragged");
        assert!(!owes_write(false, true, false, true), "nothing Lua moved");
    }

    /// The header is a comment block and survives the round trip as one — a reader that choked on
    /// its own header would lose the player's settings on the second launch.
    #[test]
    fn the_header_is_skipped_not_parsed() {
        assert!(render(&[ChatWindowLook::default()], &[], &[], 0, true).starts_with('#'));
        assert_eq!(parse(HEADER, &rows()), Parsed::default());
    }

    /// The rows the files written before 1948 hold — keys on the `WINDOW` line, no
    /// `ADDEDVERSION` — still read, case-insensitively, an unknown key skipped and a missing
    /// field left at its init value; and the loader's back-fill lands `COMBAT_FACTION_CHANGE` and
    /// `MONEY` in window 2 as it would for any pre-`ADDEDVERSION 2` file.
    #[test]
    fn the_pre_1948_one_line_rows_still_parse() {
        let got = parse(
            "window 1  size 16  color 10 20 30 40\n\
             WINDOW 2  SHOWN 0  COLOR 1 2 3 4  DOCKED 2  BOGUS 9\n\
             WINDOW 3  SIZE 12\n",
            &rows(),
        );
        let mut w2 = look(2, 1, 2, 3, 4, 0);
        w2.shown = false;
        w2.docked = Some(2);
        assert_eq!(
            got.looks,
            vec![
                (0, look(1, 10, 20, 30, 40, 16)),
                (1, w2),
                (2, look(3, 0, 0, 0, 0, 12)),
            ]
        );
        assert!(got.looks[1].1.messages.iter().any(|m| m == "MONEY"));
    }

    /// A file in the reference's own layout — the one a stock client writes — parses: the global
    /// blocks are read for what they are, the colours land, both dock windows carry their sets,
    /// and the window's `ZONECHANNELS` word comes back as `(Shortcut, id)` rows.
    #[test]
    fn a_stock_reference_file_parses() {
        let text = "VERSION 2\n\nADDEDVERSION 2\n\nOPTION_GUILD_RECRUITMENT_CHANNEL AUTO\n\n\
                    CHANNELS\nMyGuildChat\nEND\n\nZONECHANNELS 8388611\n\n\
                    COLORS\nSAY 255 255 255\nSYSTEM 200 200 0\nEND\n\n\
                    WINDOW 1\nSIZE 0\nCOLOR 0 0 0 0\nLOCKED 1\nDOCKED 1\nSHOWN 1\n\n\
                    MESSAGES\nSYSTEM\nSAY\nEND\n\nCHANNELS\nMyGuildChat\nEND\n\n\
                    ZONECHANNELS 8388611\n\nEND\n\n\
                    WINDOW 2\nNAME Combat Log\nSIZE 0\nCOLOR 0 0 0 0\nLOCKED 1\nDOCKED 2\nSHOWN 0\n\n\
                    MESSAGES\nCOMBAT_XP_GAIN\nEND\n\nCHANNELS\nEND\n\nZONECHANNELS 0\n\nEND\n";
        let got = parse(text, &rows());
        assert_eq!(
            got.colors,
            vec![
                ("SAY".to_string(), [255, 255, 255]),
                ("SYSTEM".to_string(), [200, 200, 0])
            ]
        );
        assert_eq!(got.joined, vec!["MyGuildChat".to_string()]);
        assert_eq!(got.looks.len(), 2);
        let (i, w1) = &got.looks[0];
        assert_eq!(*i, 0);
        assert!(w1.shown && w1.name.is_empty());
        assert_eq!(w1.messages, vec!["SYSTEM".to_string(), "SAY".to_string()]);
        assert_eq!(
            w1.channels,
            vec![
                ("MyGuildChat".to_string(), 0),
                ("General".to_string(), 1),
                ("Trade".to_string(), 2),
                ("LookingForGroup".to_string(), 24),
            ],
            "bits 0, 1 and 23 of 8388611 — the shortcut rows, in DBC order"
        );
        let (_, w2) = &got.looks[1];
        assert!(!w2.shown);
        assert_eq!(w2.name, "Combat Log");
        assert_eq!(w2.docked, Some(2));
        assert_eq!(w2.messages, vec!["COMBAT_XP_GAIN".to_string()]);
    }

    /// `LOCKED` round-trips, and a file that never mentions it keeps the init's **locked** row —
    /// the lenient default that matters, because reading it as "unlocked" would hand every
    /// pre-existing player's chat window to a stray drag on their next login.
    #[test]
    fn the_lock_round_trips_and_an_absent_key_stays_locked() {
        let unlocked = ChatWindowLook {
            locked: false,
            ..look(1, 0, 0, 0, 64, 14)
        };
        let text = render(std::slice::from_ref(&unlocked), &[], &[], 0, true);
        assert!(text.contains("LOCKED 0"));
        assert_eq!(parse(&text, &rows()).looks, vec![(0, unlocked.clone())]);
        assert_eq!(
            parse("WINDOW 1\nSIZE 14\nCOLOR 0 0 0 64\nEND\n", &rows()).looks,
            vec![(0, look(1, 0, 0, 0, 64, 14))],
            "no LOCKED key = the init's LOCKED 1"
        );
    }

    /// **`DOCKED` round-trips, and an absent key keeps the window's own init position** — the
    /// leniency `LOCKED` gets, in the one field where a single flat default could not express it
    /// (1714). `SHOWN` and the message sets get the same treatment for the same reason.
    #[test]
    fn dock_positions_round_trip_and_an_absent_key_keeps_the_init() {
        let moved = ChatWindowLook {
            docked: Some(3),
            ..look(1, 0, 0, 0, 0, 0)
        };
        let text = render(std::slice::from_ref(&moved), &[], &[], 0, true);
        assert!(text.contains("DOCKED 3"));
        assert_eq!(parse(&text, &rows()).looks, vec![(0, moved.clone())]);
        // No DOCKED/SHOWN/MESSAGES at all: the boot init stands — window 1 shown and undocked
        // with the ten General groups, window 2 shown at dock index 1 with the 34, window 3 out.
        let got = parse(
            "WINDOW 1  SIZE 0\nWINDOW 2  SIZE 0\nWINDOW 3  SIZE 0\n",
            &rows(),
        );
        assert_eq!(
            got.looks
                .iter()
                .map(|(_, l)| (l.docked, l.shown, l.messages.len()))
                .collect::<Vec<_>>(),
            vec![(None, true, 10), (Some(1), true, 34), (None, false, 0)]
        );
    }

    /// Junk costs the line it is on and nothing more.
    #[test]
    fn junk_costs_only_its_own_line() {
        let got = parse(
            "WINDOW\nnot a window line\nWINDOW 0 SIZE 1\nWINDOW 2 SIZE 18\n",
            &rows(),
        );
        assert_eq!(got.looks, vec![(1, look(2, 0, 0, 0, 0, 18))]);
    }

    /// A `MESSAGES` block replaces the init set rather than adding to it — an empty block is a
    /// window that shows nothing, which is what the writer means by it (the loader zeroes every
    /// flag before it reads).
    #[test]
    fn an_empty_messages_block_is_an_empty_set() {
        let got = parse("ADDEDVERSION 2\nWINDOW 1\nMESSAGES\nEND\nEND\n", &rows());
        assert!(got.looks[0].1.messages.is_empty());
    }

    /// **The `/reload` flush** (the reload half of decision 1290's class): a window renamed in the
    /// last second before a `ReloadUI()` must reach the player's file.
    ///
    /// Against the pre-fix shape this fails, and it is worth saying exactly why, because all three
    /// of the mechanisms that look like they should cover it are VM-keyed and reset together:
    /// `save_on_session_end` hung on `OnExit(InWorld)`, which `run_pending_reload` never crosses
    /// (1291); the debounce beside it had not fired, because [`SAVE_QUIET`] had not elapsed; and
    /// the new VM's [`ChatWindowFile::dirty`] memo came up `false` while [`restore_chat_looks`]
    /// re-seated the look table from the file **on disk**. So the rename was discarded with no
    /// error — and the assertion on the rebuilt VM below is the half the player would actually
    /// see: the window comes back under its old name.
    #[test]
    fn a_window_renamed_just_before_a_reload_reaches_the_file() {
        use bevy::ecs::system::RunSystemOnce;

        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-chat-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("benilla-config")).expect("hermetic home");
        let _capture = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _home = crate::local_state::test_env::EnvGuard::set(
            "BENILLA_HOME",
            tmp.join("benilla-config")
                .to_str()
                .expect("utf-8 temp path"),
        );

        let mut world = World::new();
        world.init_resource::<crate::ui_script::AddOnIdentity>();
        world.init_resource::<crate::minimap::MinimapZoom>();
        world.init_resource::<crate::ui_script::ReloadUiPending>();
        world.init_resource::<super::super::edit::ChannelState>();
        world.init_resource::<ChatWindowFile>();
        crate::ui_script::setup_script(&mut world);
        world.insert_resource(probe_roster("Reloadprobe"));
        crate::ui_script::load_ingame_ui_on_world_entry(&mut world);

        let path = world
            .resource::<ChatWindowFile>()
            .path
            .clone()
            .expect("the character's chat cache path is seated by the entry load");

        // The player renames window 1 — a Lua write, exactly what the debounce exists to coalesce.
        world
            .non_send_resource_mut::<UiScript>()
            .run(r#"SetChatWindowName(1, "Reloaded")"#)
            .expect("rename");
        world
            .run_system_once(watch_chat_looks)
            .expect("the watcher arms the debounce");
        assert!(
            world.resource::<ChatWindowFile>().last_change.is_some(),
            "precondition: the rename armed the debounce, so the write is owed but not yet due"
        );

        // `/reload`, immediately — inside `SAVE_QUIET`, which is the whole window of the bug.
        world.insert_resource(State::new(crate::char_select::ClientState::InWorld));
        world.resource_mut::<crate::ui_script::ReloadUiPending>().0 = true;
        crate::ui_script::run_pending_reload(&mut world);

        let on_disk = std::fs::read_to_string(&path).expect("the flush wrote the file");
        assert!(
            on_disk.contains("Reloaded"),
            "the rename must survive the reload — the dying VM's cache is folded out by \
             `fold_dying_vm_chat_cache`; file was:\n{on_disk}"
        );

        // And the half the player sees: the rebuilt VM read it back.
        let name = world
            .non_send_resource_mut::<UiScript>()
            .eval::<String>("GetChatWindowInfo(1)")
            .unwrap_or_default();
        assert_eq!(
            name, "Reloaded",
            "the rebuilt VM restored the renamed window, not the stale one"
        );

        drop(world);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A roster with one character picked — what `load_ingame_ui_on_world_entry` keys the
    /// character's chat cache path on.
    fn probe_roster(name: &str) -> crate::char_select::Roster {
        crate::char_select::Roster::with_pending_pick(
            vec![benilla_protocol::Character {
                guid: 1,
                name: name.into(),
                race: 1,  // Human → Alliance
                class: 1, // Warrior
                gender: 0,
                level: 60,
                skin: 0,
                face: 0,
                hair_style: 0,
                hair_color: 0,
                facial_hair: 0,
                zone: 0,
                map: 0,
                position: benilla_protocol::wire::Vector3d {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                flags: 0,
                equipment: [benilla_protocol::CharEnumItem::default(); 19],
                pet_display_id: 0,
                pet_level: 0,
                pet_family: 0,
            }],
            1,
        )
    }

    /// **A custom channel survives `/logout`** (2184 §6, the question it left open). The header's
    /// `CHANNELS` list is the character's custom channels to re-join at the next login. It was
    /// composed from the live roster, and the logout clears that roster first: the
    /// `Disconnected{LoggedOut}` that arrives in the same net drain as the logout runs
    /// `end_session_channels_on_disconnect` in `Update`, a frame before `OnExit(InWorld)` reaches
    /// `end_ui_session`'s flush. So the file was written with an empty list and the channel was
    /// never re-joined. The same order is driven here, by the same functions.
    #[test]
    fn a_custom_channel_survives_a_logout() {
        use bevy::ecs::system::RunSystemOnce;

        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-chat-custom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("benilla-config")).expect("hermetic home");
        let _capture = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _home = crate::local_state::test_env::EnvGuard::set(
            "BENILLA_HOME",
            tmp.join("benilla-config")
                .to_str()
                .expect("utf-8 temp path"),
        );

        let roster = probe_roster("Channelprobe");
        let id = crate::ui_macro::identity(&roster).expect("a picked character");
        let path = crate::local_state::chat_character_path(&id.0, &id.1).expect("a cache path");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the character dir");
        // A file this writer made (the marker), listing one custom channel in the header.
        let file = format!(
            "{WRITER_GENERATION}\nVERSION 2\n\nCHANNELS\nMyChan\nEND\n\nZONECHANNELS 3\n\n\
             COLORS\nEND\n\nWINDOW 1\nSIZE 0\n\nMESSAGES\nSAY\nEND\n\nCHANNELS\nEND\n\n\
             ZONECHANNELS 3\n\nEND\n"
        );
        std::fs::write(&path, file).expect("seed the file");

        let mut world = World::new();
        world.init_resource::<crate::ui_script::AddOnIdentity>();
        world.init_resource::<crate::minimap::MinimapZoom>();
        world.init_resource::<crate::ui_script::ReloadUiPending>();
        world.init_resource::<super::super::edit::ChannelState>();
        world.init_resource::<super::super::channels::ZoneChannelWalk>();
        world.init_resource::<super::super::recruitment::GuildRecruitmentCascade>();
        world.init_resource::<Messages<crate::net::DisconnectedMessage>>();
        world.init_resource::<ChatWindowFile>();
        crate::ui_script::setup_script(&mut world);
        world.insert_resource(roster);
        crate::ui_script::load_ingame_ui_on_world_entry(&mut world);

        // The login's re-join is confirmed: the server's YOU_JOINED, through the real feed arm.
        world.resource_scope(
            |world, mut channels: Mut<super::super::edit::ChannelState>| {
                let mut script = world.non_send_resource_mut::<UiScript>();
                let mut windows = super::super::frames::ChatWindows::default();
                let mut e = super::super::event::ChatEvent::text_only(
                    super::super::event::ChatEventKind::ChannelNotice,
                    String::new(),
                );
                e.channel = "MyChan".into();
                e.notice = "2".into(); // YOU_JOINED
                super::super::feed::deliver(&mut script, &mut windows, &mut channels, &mut e);
            },
        );
        assert_eq!(
            world
                .resource::<super::super::edit::ChannelState>()
                .number_of("MyChan"),
            Some(1),
            "precondition: the channel is joined and numbered"
        );

        // `/logout`: the disconnect lands in `Update` first…
        world.write_message(crate::net::DisconnectedMessage::new(
            "logged out".into(),
            benilla_protocol::SessionEnd::LoggedOut,
        ));
        world
            .run_system_once(super::super::channels::end_session_channels_on_disconnect)
            .expect("the disconnect twin runs");
        // …and `OnExit(InWorld)` flushes the dying VM's cache a frame later.
        crate::ui_script::end_ui_session(&mut world);

        let on_disk = std::fs::read_to_string(&path).expect("the flush wrote the file");
        assert!(
            on_disk.contains("\nCHANNELS\nMyChan\nEND\n\nZONECHANNELS"),
            "the custom channel is still in the header's re-join list; file was:\n{on_disk}"
        );

        drop(world);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
