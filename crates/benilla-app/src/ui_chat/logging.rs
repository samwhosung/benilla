//! The `LoggingChat`/`LoggingCombat` files, `WoWChatLog.txt` and `WoWCombatLog.txt`: the stock
//! `/chatlog` and `/combatlog` flip the VM's flag and print the notice (`ChatFrame.lua:675-695`);
//! this appends each line as the window shows it, stamped `M/D HH:MM:SS.mmm` in local time.
//! Deviation: the files live in `benilla-config/Logs/`, not the install's `Logs`, because the
//! install is read-only. A file that fails to open leaves the flag set: Lua already printed
//! "enabled".

use std::io::Write as _;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

/// The open log files, kept on the chat windows since every line passes [`super::frames::route`].
#[derive(Default)]
pub(crate) struct ChatLogFiles {
    chat: Option<std::fs::File>,
    combat: Option<std::fs::File>,
}

impl ChatLogFiles {
    /// Append a rendered line to the combat or the chat log, when that file is open.
    pub(super) fn record(&mut self, combat: bool, line: &str) {
        let slot = if combat {
            &mut self.combat
        } else {
            &mut self.chat
        };
        if let Some(file) = slot.as_mut() {
            if writeln!(file, "{}  {line}", stamp()).is_err() {
                *slot = None;
            }
        }
    }

    fn set(&mut self, combat: bool, on: bool) {
        let name = if combat {
            "WoWCombatLog.txt"
        } else {
            "WoWChatLog.txt"
        };
        let slot = if combat {
            &mut self.combat
        } else {
            &mut self.chat
        };
        if !on {
            *slot = None;
            return;
        }
        if slot.is_some() {
            return;
        }
        let Some(dir) = crate::local_state::logs_dir() else {
            return; // hermetic capture, or no state folder
        };
        let path = dir.join(name);
        let opened = std::fs::create_dir_all(&dir).and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
        });
        match opened {
            Ok(file) => {
                info!("chat: logging to {}", path.display());
                *slot = Some(file);
            }
            Err(e) => warn!("chat: cannot open {}: {e}", path.display()),
        }
    }
}

/// `M/D HH:MM:SS.mmm` of now, local time: the reference's log-line stamp
/// (`"%u/%u %02u:%02u:%02u.%03u  "`, `0x866aa0`, from `GetLocalTime` at `0x65a871`), which
/// `crate::ui_script::load_log` shares.
pub(crate) fn stamp() -> String {
    let [month, day, hour, min, sec, milli] = local_now();
    format!("{month}/{day} {hour:02}:{min:02}:{sec:02}.{milli:03}")
}

/// `[month, day, hour, minute, second, millisecond]` of now, from the reference's `GetLocalTime`.
#[cfg(windows)]
fn local_now() -> [u32; 6] {
    use windows_sys::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};
    // SAFETY: `GetLocalTime` fully writes the out-param and reads nothing from it; zeroed is
    // valid for a struct of plain integers.
    let t = unsafe {
        let mut t = std::mem::zeroed::<SYSTEMTIME>();
        GetLocalTime(&mut t);
        t
    };
    [
        t.wMonth,
        t.wDay,
        t.wHour,
        t.wMinute,
        t.wSecond,
        t.wMilliseconds,
    ]
    .map(u32::from)
}

/// `[month, day, hour, minute, second, millisecond]` of now, in the process's time zone.
#[cfg(unix)]
fn local_now() -> [u32; 6] {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let [month, day, hour, min, sec] = local_fields(now.as_secs() as i64);
    [month, day, hour, min, sec, now.subsec_millis()]
}

/// `[month, day, hour, minute, second]` of an epoch second, through `localtime_r`; UTC when that
/// fails, which POSIX allows only on `EOVERFLOW`, a year past `int`.
#[cfg(unix)]
fn local_fields(secs: i64) -> [u32; 5] {
    let time = secs as libc::time_t;
    // SAFETY: `localtime_r` fully writes the out-param and reads nothing from it; zeroed is valid
    // for its integers and its nullable zone-name pointer.
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        let c = benilla_ui::civil::from_unix(secs);
        return [c.month, c.day, c.hour, c.min, c.sec];
    }
    [tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec].map(|v| v as u32)
}

/// The VM's two flags → the two files, on the frame either flag moves.
pub(super) fn sync_chat_logging(
    script: Option<NonSendMut<UiScript>>,
    mut windows: ResMut<super::frames::ChatWindows>,
) {
    let Some(mut script) = script else { return };
    if !script.take_logging_changes() {
        return;
    }
    let (chat, combat) = script.logging_flags();
    windows.logs.set(false, chat);
    windows.logs.set(true, combat);
}

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        // After the UI tick, so a flag Lua moved is read the same frame.
        sync_chat_logging
            .after(crate::ui_script::UiInput)
            .in_set(crate::client_state::InWorldGated),
    );
}

#[cfg(all(test, unix))]
mod tests {
    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    extern "C" {
        fn tzset();
    }

    /// Under a zone 5:45 east of UTC, the epoch reads 05:45, and 20:00 UTC is past local midnight.
    #[test]
    fn the_stamp_fields_follow_the_local_zone() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tz = EnvGuard::set("TZ", "NPT-05:45");
        // SAFETY: `tzset` rereads `TZ`; the lock keeps other env tests out.
        unsafe { tzset() };
        let fields = [super::local_fields(0), super::local_fields(72_000)];
        drop(tz);
        unsafe { tzset() };
        assert_eq!(fields, [[1, 1, 5, 45, 0], [1, 2, 1, 45, 0]]);
    }
}
