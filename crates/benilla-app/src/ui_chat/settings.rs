//! The chat windows' saved state, the whole `chat-cache.txt` record: the per-type colours and each
//! window's tint, alpha, font size, lock, dock, shown flag, name, message groups and channels. It
//! lives in the VM, written and read by the stock verbs (`SetChatWindow*`, `ChangeChatColor`,
//! `GetChatWindowInfo` and kin); this module loads it at login and saves it at logout, in the
//! grammar of the reference's `WTF/Account/<ACC>/<REALM>/<CHAR>/chat-cache.txt` (written whole by
//! `0x499a80`, read by `0x498a60`):
//!
//! ```text
//! VERSION 2
//! ADDEDVERSION 2
//! OPTION_GUILD_RECRUITMENT_CHANNEL AUTO
//! CHANNELS … END           ← the custom channels, one per line, re-joined at login
//! ZONECHANNELS 18874371    ← the joined zone channels, as bits 1<<(ChannelID-1)
//! COLORS … END             ← the chat-type registry, one SAY 255 255 255 row per type
//! WINDOW 1                 ← then per window:
//! NAME General             ← only when a name was stored
//! SIZE 0
//! COLOR 0 0 0 0            ← R G B A bytes, from the record's packed BGRA quad
//! LOCKED 1
//! DOCKED 1
//! SHOWN 1
//! MESSAGES … END           ← the enabled groups of the 68-entry CHATMSGGROUP table, in order
//! CHANNELS … END           ← this window's custom channels; its zone channels are bits
//! ZONECHANNELS 18874371    ← this window's zone channels, masked by the joined set
//! END
//! ```
//!
//! The loader zeroes every window's flags first, so a `MESSAGES` block is the whole set; a window
//! the file omits keeps its boot init ([`ChatWindowLook::stock`]), and a file older than
//! `ADDEDVERSION 2` gets the groups added since. `LOCKED` is an `i32` (`CHATWINDOW+0x8c`) written
//! through `setne`, so only 0 and 1 round-trip. Deviation: `set_chat_window_looks` drops an
//! out-of-range window, where the reference's `WINDOW` bound is off by one (`0x498d1c`, `ja` where
//! the array needs `jae`), because that write would land past the array.
//!
//! Ours is `benilla-config/chat/<realm>-<character>.txt`
//! ([`crate::local_state::chat_character_path`]); its reader is as lenient as the reference's
//! (keys case-insensitive, unknown keys and blank lines skipped) and also reads benilla's older
//! one-line `WINDOW 1  SIZE 0  COLOR …` rows. `UPDATE_CHAT_WINDOWS`, then `UPDATE_CHAT_COLOR` per
//! registry entry, fire file or no file (`0x4996b9`, `0x499934`). Saves wait one quiet second, as
//! the opacity slider writes on every drag step, and flush at the session end and at `AppExit`.

use std::path::PathBuf;

use bevy::prelude::*;

use benilla_ui::script::{ChatTypeColor, ChatWindowLook, UiScript, MESSAGE_GROUPS};

use super::edit::zone_bit;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_script::VmMemo;

/// How long a dirty look sits before the save fires, as in `crate::cvars`.
const SAVE_QUIET: std::time::Duration = std::time::Duration::from_secs(1);

/// The file's header, as comment lines: the reference's reader has no comments, ours skips them.
const HEADER: &str = "\
# benilla chat cache, in the reference's chat-cache.txt grammar (written by
# 0x499a80, read by 0x498a60): the custom channels to re-join, the joined zone channels as bits, the
# per-type COLORS table, then one WINDOW block per chat frame — NAME (when one was set), SIZE,
# COLOR r g b a as bytes, LOCKED, DOCKED, SHOWN, the MESSAGES … END list of the groups the window
# shows, its CHANNELS … END list of custom channels, and its zone channels as ZONECHANNELS bits.
# Written whole; the tab menu, /join and ChangeChatColor are what move it.
";

/// benilla's repair marker, a header comment line, not the reference's grammar. A file without it
/// may hold `ZONECHANNELS 0` words from an empty roster, which strip window 1 of its channels;
/// [`restore_chat_looks`] re-seeds such a file once, and the next save stamps the marker. It is
/// matched with `contains`, so a marker line with more text after it still counts.
const WRITER_GENERATION: &str = "# benilla-writer 2";

/// Which character's file we are on, where it lives, and whether it is owed a write.
#[derive(Resource, Default)]
pub(super) struct ChatWindowFile {
    path: Option<PathBuf>,
    /// The `(realm, character)` [`Self::path`] was built for, per VM: a fresh VM restores again.
    identity: VmMemo<Option<(String, String)>>,
    /// Whether this VM has unsaved Lua writes, per VM: a replaced VM's stock table must not save.
    dirty: VmMemo<bool>,
    last_change: Option<std::time::Instant>,
}

/// A parsed file: its windows (0-based), its `COLORS` rows in file order, its custom channels.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Parsed {
    pub(super) looks: Vec<(usize, ChatWindowLook)>,
    pub(super) colors: Vec<(String, [u8; 3])>,
    pub(super) joined: Vec<String>,
    /// The header's `ZONECHANNELS` word, the reference's `[0xb6e5e0]`, overwritten from the file
    /// (`0x498d83`); `None` without the line, so the loader seeds from the DBC rather than 0.
    pub(super) zone_mask: Option<u32>,
    /// `OPTION_GUILD_RECRUITMENT_CHANNEL`, the latch `GetGuildRecruitmentMode` returns: `STANDARD`
    /// is 0 and any other word, or none, is 1 (`0x49ea70`, `ecx=0` only for `STANDARD`).
    pub(super) guild_recruitment_auto: bool,
}

impl Default for Parsed {
    fn default() -> Self {
        Self {
            looks: Vec::new(),
            colors: Vec::new(),
            joined: Vec::new(),
            zone_mask: None,
            // The boot value: the reference writes `AUTO` for characters that never set it.
            guild_recruitment_auto: true,
        }
    }
}

/// Render the file as the writer does (`0x499a80`). `custom` fills the header's `CHANNELS`, custom
/// channels only: a zone channel travels as its bit (`0x499ba1`). `zone_mask` goes raw into the
/// header (`0x499c19`) and is ANDed with each window's bits (`0x49a133`, `0x49a138`). Both are the
/// durable lists, never the live roster, which the session end clears before the flush reads it.
fn render(
    looks: &[ChatWindowLook],
    colors: &[ChatTypeColor],
    custom: &[String],
    zone_mask: u32,
    guild_recruitment_auto: bool,
) -> String {
    let mut out = String::from(HEADER);
    // The repair marker, a comment line.
    out.push_str(WRITER_GENERATION);
    out.push('\n');
    // The latch `SetGuildRecruitmentMode` writes; this is the one place it persists.
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
    /// The header's `CHANNELS … END`: the custom channels to re-join.
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
                    // A word that is a `ChatChannels.dbc` Shortcut is a zone channel that lost its
                    // id: the reference never stores one by name (`AddChatWindowChannel`
                    // `0x4a1000` resolves shortcuts first), and `ChatFrame.lua:1379` matches ids.
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
            // benilla's older one-line row: the keys follow on the same line.
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
                // Overwrites `[0xb6e5e0]` (`0x498d83`), never ORs: the file holds the whole mask.
                out.zone_mask = it.next().and_then(|v| v.trim().parse::<u32>().ok());
            } else if head.eq_ignore_ascii_case("OPTION_GUILD_RECRUITMENT_CHANNEL") {
                // The reference's test, whole-word and case-folded: only `STANDARD` means 0.
                out.guild_recruitment_auto = !it
                    .next()
                    .is_some_and(|w| w.eq_ignore_ascii_case("STANDARD"));
            }
            // `VERSION` is not read.
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
    // The loader's EOF back-fill (`0x49967c`): an older file gets the groups added since, the
    // first ten into window 1 and the rest, both `addedVersion` rows among them, into window 2.
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

/// Restore the file into a fresh VM, once per character per VM, and fire the loader's two events,
/// file or no file. Called from the world-entry UI load, not `Update`, for those events:
/// `UPDATE_CHAT_WINDOWS` registers a window for any `CHAT_MSG_*` (`ChatFrame.lua:1261-1273`), so
/// it must precede the server's MOTD after `SMSG_LOGIN_VERIFY_WORLD`, and the `UPDATE_CHAT_COLOR`
/// burst must precede `PLAYER_LOGIN`: its `WHISPER` row also repaints `ChatTypeInfo["REPLY"]`
/// (`ChatFrame.lua:1357-1365`), whose `.id` is 0, and `UpdateColorByID(0, …)` recolours every line
/// printed with no explicit colour, an addon's login `Print` among them.
pub(crate) fn restore_chat_looks(world: &mut World, script: &mut UiScript) {
    let Some(id) = world
        .get_resource::<crate::char_select::Roster>()
        .and_then(crate::ui_macro::identity)
    else {
        return;
    };
    // Owned rows up front: `ChatWindowFile` is borrowed mutably below.
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
    // The DBC seed: every `ChatChannels.dbc` row the client joins by itself (`flags & 1`).
    let seed_mask = auto_rows.iter().fold(0, |m, (_, id)| m | zone_bit(*id));
    // Scoped, so `ChannelState` can take the mask afterwards.
    let mut parsed;
    {
        let Some(mut file) = world.get_resource_mut::<ChatWindowFile>() else {
            return;
        };
        if file.identity.get(script).as_ref() == Some(&id) {
            return; // already restored for this character into this VM
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
        // No marker: its `ZONECHANNELS` words may come from an empty roster. Repaired below, once.
        let damaged = text
            .as_deref()
            .is_some_and(|t| !t.contains(WRITER_GENERATION));
        parsed = text.map(|t| parse(&t, &rows)).unwrap_or_default();
        if !had_file {
            // The loader's no-file path (`0x4997ad`): window 1 gets each seed row as
            // `(Shortcut, id)`, which `ChatFrame_RegisterForChannels` matches zone speech by.
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
            // The one-time repair: re-seed window 1 and OR the seed into the mask, additive and
            // deduplicated like the in-window `ZONECHANNELS` arm (`0x499332`-`0x4994e7`).
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
            // Owed a write, which stamps the marker; it saves next frame (`last_change` is `None`).
            *file.dirty.get(script) = true;
            info!("chat cache: repaired an unmarked file's zone channels for {who}");
        }
    }
    // The durable mask: the file's word, else the DBC seed; confirmed joins OR into it after.
    let mask = parsed.zone_mask.unwrap_or(seed_mask);
    if let Some(mut channels) = world.get_resource_mut::<super::edit::ChannelState>() {
        // `Some` is the reference's "chat system ready" flag (`0x499a18`): the zone walk and the
        // guild-recruitment cascade wait for it.
        channels.zone_mask = Some(mask);
        // The custom re-join list, seated here, then moved only by a confirmed join or a leave.
        channels.custom = parsed.joined.clone();
    }
    script.set_guild_recruitment_mode(u8::from(parsed.guild_recruitment_auto));
    script.set_chat_colors(parsed.colors);
    script.set_chat_window_looks(parsed.looks);
    // `UPDATE_CHAT_WINDOWS` once, then `UPDATE_CHAT_COLOR` for every registry entry, file or no
    // file (`0x4996b9`, `0x499934`).
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
    // The header's `CHANNELS` re-join at login, as in the reference; zone channels are the walk's.
    if let Some(commands) = commands {
        for name in parsed.joined {
            let _ = commands.send(ClientCommand::JoinChannel {
                name,
                password: String::new(),
            });
        }
    }
}

/// Arm the debounce when Lua writes the chat state.
fn watch_chat_looks(script: Option<NonSendMut<UiScript>>, mut file: ResMut<ChatWindowFile>) {
    let Some(mut script) = script else { return };
    let moved = !script.take_chat_window_changes().is_empty();
    let coloured = script.take_chat_color_changes();
    // `SetGuildRecruitmentMode` persists here too; a Lua call arms this, the login seat never.
    let recruitment = script.take_guild_recruitment_change();
    if !(moved || coloured || recruitment) {
        return;
    }
    *file.dirty.get(&script) = true;
    file.last_change = Some(std::time::Instant::now());
}

fn write(script: &UiScript, channels: &super::edit::ChannelState, path: &std::path::Path) {
    // Never write from an unseated mask: `None` means the file was never read, and writing would
    // zero every block's `ZONECHANNELS`.
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

/// Whether a write is owed now.
///
/// - A flush (`exiting`: session end or window close) writes whenever this VM restored the file
///   (`restored`, never a fresh VM's stock table), as the reference rewrites it whole at chat
///   teardown (`0x499a80` from `0x490c55`) with no dirty check (`[0xb6e5c4]` is never read); host
///   state such as the zone mask never marks the file dirty.
/// - Deviation: the debounce also writes Lua's changes (`dirty`) after the quiet time, where the
///   reference writes only at teardown, so a crash loses at most a second of changes.
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

/// The session-end flush, the reference's chat teardown (`0x499a80` from `0x490c55`), called from
/// [`crate::ui_script::end_ui_session`] after the shutdown events and before the VM is replaced,
/// on logout, character switch and `/reload` alike. Not an `OnExit(InWorld)` system: `/reload`
/// never crosses it, and the debounce's state dies with the VM.
pub(crate) fn fold_dying_vm_chat_cache(world: &mut World) {
    // No plugin in this world (a test world): nothing to flush.
    if !world.contains_resource::<ChatWindowFile>() {
        return;
    }
    // Lift the file out, leaving the VM and the roster as shared borrows of the world.
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
        // The restore runs from the world-entry UI load, not here: its events must precede the
        // first chat line and `PLAYER_LOGIN`, which no `Update` ordering guarantees.
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
    // The session-end flush is `fold_dying_vm_chat_cache`; the quit flush rides the exit edge,
    // as the close button's `AppExit` is written in `PostUpdate`, after an `Update` save.
    crate::shutdown::on_app_exit(app, save_chat_looks.into_configs());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Window `n`'s (1-based) boot-init record, which differs per window, with the look fields set.
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

    /// A subset of `ChatChannels.dbc` as [`parse`] takes it, with the three `INITIAL` rows.
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
        // The mask and the custom list are passed in, not derived from the live roster.
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

    /// The stock `ChatFrame_OnEvent` matches channel lines by id (`ChatFrame.lua:1379`), so a
    /// shortcut stored with id 0 would lose its channel's lines.
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

        // The next save writes them as bits, not names.
        let out = render(&[parsed.looks[0].1.clone()], &[], &[], 0x0020_0003, false);
        let window = out.split("WINDOW 1").nth(1).unwrap();
        assert!(window.contains("CHANNELS\nMyChan\nEND"), "{out}");
        assert!(window.contains("ZONECHANNELS 2097155\n"), "{out}");
    }

    #[test]
    fn a_windows_zone_bits_are_masked_by_the_joined_set() {
        let mut w = look(1, 0, 0, 0, 0, 0);
        w.channels = vec![("General".into(), 1), ("Trade".into(), 2)];
        // The mask holds General alone: Trade was left, so its window bit is ANDed away.
        let text = render(&[w], &[], &[], 1, true);
        let windows: Vec<&str> = text.split("WINDOW 1").collect();
        assert!(windows[1].contains("ZONECHANNELS 1\n"), "{text}");
    }

    /// The header and window words come from the durable mask, not the live roster.
    #[test]
    fn an_empty_roster_does_not_erase_a_windows_zone_channels() {
        let mut w = look(1, 0, 0, 0, 0, 0);
        w.channels = vec![("General".into(), 1), ("Trade".into(), 2)];
        // In both channels by the mask, with the live roster empty, as at a session-end save.
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
            "window 1 keeps both channels, where an unmarked file's came back empty"
        );
    }

    /// The repair is keyed on the [`WRITER_GENERATION`] marker, not on the zone words.
    #[test]
    fn a_written_file_carries_the_writer_generation_and_a_damaged_one_does_not() {
        let text = render(&[ChatWindowLook::stock(0)], &[], &[], 0b11, true);
        assert!(
            text.contains(WRITER_GENERATION),
            "every file we write is stamped, so the repair fires once: {text}"
        );
        // A zeroed file: a header the reference's reader accepts, with no marker.
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
        // Unseated, a join is dropped.
        state.note_zone_channel_joined("General - Elwynn Forest");
        assert_eq!(state.zone_mask, None, "not seated: nothing to OR into");
        state.zone_mask = Some(0);
        state.note_zone_channel_joined("General - Elwynn Forest");
        state.note_zone_channel_joined("Trade - City");
        assert_eq!(state.zone_mask, Some(0b11));
        state.note_zone_channel_joined("MyChan");
        assert_eq!(state.zone_mask, Some(0b11), "a custom channel has no bit");
        // The clear is keyed on the slot the wire name finds and its id (`0x49f0f4`-`0x49f11a`);
        // a name with no slot clears nothing.
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

    /// The teardown write ignores the dirty flag, as the reference's does, so a `/leave` persists.
    #[test]
    fn a_flush_writes_whatever_was_restored_and_the_debounce_writes_only_lua_moves() {
        // The flush: restored is the whole condition.
        assert!(owes_write(true, true, false, false));
        assert!(owes_write(true, true, true, true));
        assert!(
            !owes_write(true, false, true, true),
            "a VM that never read the file has nothing of the player's to write"
        );
        // The debounce: dirty and quiet.
        assert!(owes_write(false, true, true, true));
        assert!(!owes_write(false, true, true, false), "still being dragged");
        assert!(!owes_write(false, true, false, true), "nothing Lua moved");
    }

    #[test]
    fn the_header_is_skipped_not_parsed() {
        assert!(render(&[ChatWindowLook::default()], &[], &[], 0, true).starts_with('#'));
        assert_eq!(parse(HEADER, &rows()), Parsed::default());
    }

    /// benilla's older one-line rows (no `ADDEDVERSION`), with the back-fill into window 2.
    #[test]
    fn one_line_rows_without_addedversion_still_parse() {
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

    #[test]
    fn dock_positions_round_trip_and_an_absent_key_keeps_the_init() {
        let moved = ChatWindowLook {
            docked: Some(3),
            ..look(1, 0, 0, 0, 0, 0)
        };
        let text = render(std::slice::from_ref(&moved), &[], &[], 0, true);
        assert!(text.contains("DOCKED 3"));
        assert_eq!(parse(&text, &rows()).looks, vec![(0, moved.clone())]);
        // The boot init: window 1 shown and undocked with ten groups, window 2 shown at dock
        // index 1 with 34, window 3 hidden.
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

    #[test]
    fn junk_costs_only_its_own_line() {
        let got = parse(
            "WINDOW\nnot a window line\nWINDOW 0 SIZE 1\nWINDOW 2 SIZE 18\n",
            &rows(),
        );
        assert_eq!(got.looks, vec![(1, look(2, 0, 0, 0, 0, 18))]);
    }

    /// A `MESSAGES` block replaces the init set: the loader zeroes every flag before it reads.
    #[test]
    fn an_empty_messages_block_is_an_empty_set() {
        let got = parse("ADDEDVERSION 2\nWINDOW 1\nMESSAGES\nEND\nEND\n", &rows());
        assert!(got.looks[0].1.messages.is_empty());
    }

    /// A rename inside [`SAVE_QUIET`] of a `/reload`, which never crosses `OnExit(InWorld)`,
    /// reaches the file through the flush in `end_ui_session` and comes back in the rebuilt VM.
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

        // A Lua rename of window 1, which arms the debounce.
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

        // `/reload` at once, inside `SAVE_QUIET`.
        world.insert_resource(State::new(crate::char_select::ClientState::InWorld));
        world.resource_mut::<crate::ui_script::ReloadUiPending>().0 = true;
        crate::ui_script::run_pending_reload(&mut world);

        let on_disk = std::fs::read_to_string(&path).expect("the flush wrote the file");
        assert!(
            on_disk.contains("Reloaded"),
            "the rename must survive the reload — the dying VM's cache is folded out by \
             `fold_dying_vm_chat_cache`; file was:\n{on_disk}"
        );

        // The rebuilt VM reads it back.
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

    /// A roster with one character picked, which keys the chat cache path.
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

    /// The disconnect clears the live roster a frame before `end_ui_session` flushes, so the
    /// header's re-join list must come from the durable custom list; the same functions run here.
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

        // The server confirms the re-join (`YOU_JOINED`) through the real feed arm.
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
