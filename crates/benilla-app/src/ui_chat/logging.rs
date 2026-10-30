//! The `LoggingChat`/`LoggingCombat` files, `WoWChatLog.txt` and `WoWCombatLog.txt`: the stock
//! `/chatlog` and `/combatlog` flip the VM's flag and print the notice (`ChatFrame.lua:675-695`);
//! the verb itself opens the log when the flag turns on with none open, inside the call, and a
//! failed open clears the flag (`0x49fe79`-`0x49feab`). This appends each line as the window shows
//! it, stamped `M/D HH:MM:SS.mmm` in local time.
//!
//! The files close only at the chat teardown (`0x498700` → `0x659ec0` on each open handle, zeroing
//! the handles and both flags): a flag turned off stops the writes (the chat writer tests
//! `[0xb4fdb4]` at `0x49acde` before `[0xb50540]`, `0x49ad16`; the combat writer `[0xb4fdb8]` at
//! `0x49b1d3` before `[0xb50544]`) and leaves the file open, and a later on with the handle set
//! opens nothing (`0x49fe89`). Here that teardown is [`ChatLogFiles::close_all`], run at the
//! session end that clears the chat windows.
//!
//! Deviation: the files live in `benilla-config/Logs/`, not the install's `Logs`, because the
//! install is read-only.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;

/// The open log files, shared by the chat windows (every line passes [`super::frames::route`]) and
/// by the verbs' host, which opens and closes them inside the call.
#[derive(Resource, Clone)]
pub(crate) struct ChatLogFiles(Arc<Mutex<Files>>);

struct Files {
    /// Where the logs live; none in a hermetic capture or with no state folder.
    dir: Option<PathBuf>,
    chat: Log,
    combat: Log,
}

/// One log: its file, once opened, and its flag, which gates the writes.
#[derive(Default)]
struct Log {
    file: Option<std::fs::File>,
    on: bool,
}

impl Files {
    fn log(&mut self, combat: bool) -> &mut Log {
        if combat {
            &mut self.combat
        } else {
            &mut self.chat
        }
    }
}

impl Default for ChatLogFiles {
    fn default() -> Self {
        Self::new(crate::local_state::logs_dir())
    }
}

impl ChatLogFiles {
    /// Log files under `dir`.
    pub(crate) fn new(dir: Option<PathBuf>) -> Self {
        Self(Arc::new(Mutex::new(Files {
            dir,
            chat: Log::default(),
            combat: Log::default(),
        })))
    }

    fn files(&self) -> std::sync::MutexGuard<'_, Files> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Append a rendered line to the combat or the chat log, when its flag is on and its file open.
    pub(super) fn record(&self, combat: bool, line: &str) {
        let mut files = self.files();
        let log = files.log(combat);
        if !log.on {
            return;
        }
        if let Some(file) = log.file.as_mut() {
            if writeln!(file, "{}  {line}", stamp()).is_err() {
                log.file = None;
            }
        }
    }

    /// The verb's host step after the flag was set to `on`: the writes follow the flag. With it
    /// on and no file open, open one; the answer is whether the flag stands (false is the failed
    /// open that clears it, `0x49feab`). With it off the file stays open.
    pub(crate) fn apply(&self, combat: bool, on: bool) -> bool {
        let mut files = self.files();
        let name = if combat {
            "WoWCombatLog.txt"
        } else {
            "WoWChatLog.txt"
        };
        let dir = files.dir.clone();
        let log = files.log(combat);
        log.on = on;
        if !on || log.file.is_some() {
            return on;
        }
        let Some(dir) = dir else {
            log.on = false;
            return false; // hermetic capture, or no state folder
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
                log.file = Some(file);
                true
            }
            Err(e) => {
                warn!("chat: cannot open {}: {e}", path.display());
                log.on = false;
                false
            }
        }
    }

    /// The chat teardown (`0x498700`): close both files and clear both flags.
    pub(crate) fn close_all(&self) {
        let mut files = self.files();
        files.chat = Log::default();
        files.combat = Log::default();
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

#[cfg(all(test, unix))]
mod tests {
    use benilla_ui::civil::ZoneGuard;

    /// Under a zone 5:45 east of UTC, the epoch reads 05:45, and 20:00 UTC is past local midnight.
    /// The stamp is `GetLocalTime`'s (`0x65a871`), the system's zone as it is now, so it reads the
    /// platform's `localtime_r` and not [`benilla_ui::civil`]'s once-read zone; the guard only
    /// holds `TZ` (and runs `tzset`) for the test.
    #[test]
    fn the_stamp_fields_follow_the_local_zone() {
        let fields = {
            let _zone = ZoneGuard::set("NPT-05:45");
            [super::local_fields(0), super::local_fields(72_000)]
        };
        assert_eq!(fields, [[1, 1, 5, 45, 0], [1, 2, 1, 45, 0]]);
    }
}

#[cfg(test)]
mod verb_tests {
    use super::ChatLogFiles;
    use benilla_ui::script::UiScript;

    /// A world whose log files live under `dir`, with a VM in it.
    fn world_with_logs(dir: Option<std::path::PathBuf>) -> bevy::prelude::World {
        let (mut world, _rx) = super::super::input::chat_world(UiScript::new().expect("a VM"));
        world.insert_resource(ChatLogFiles::new(dir));
        world
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("benilla-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// An open that fails (`0x49fea2`) clears the flag (`0x49feab`), so the verb answers nil in the
    /// call and a bare call answers nil after; the "directory" is a file, so it cannot be made.
    #[test]
    fn a_log_that_cannot_open_clears_the_flag_inside_the_call() {
        let blocked = scratch("logging-blocked");
        std::fs::write(&blocked, b"a file, not a folder").unwrap();
        for verb in ["LoggingChat", "LoggingCombat"] {
            let mut world = world_with_logs(Some(blocked.join("Logs")));
            super::super::run_chat(&mut world, &format!("ANSWER = {verb}(1) AFTER = {verb}()"))
                .expect("lua");
            let script = world.non_send_resource::<UiScript>();
            assert!(
                script.eval::<bool>("return ANSWER == nil").unwrap(),
                "{verb}(1)"
            );
            assert!(
                script.eval::<bool>("return AFTER == nil").unwrap(),
                "{verb}()"
            );
        }
        let _ = std::fs::remove_file(&blocked);
    }

    /// An open that works answers 1 and the file takes a line written in the same chunk. Off
    /// stops the writes and keeps the file; on again appends to it; the teardown closes it.
    #[test]
    fn an_opened_log_answers_one_and_takes_a_line_in_the_same_chunk() {
        for (verb, combat, file) in [
            ("LoggingChat", false, "WoWChatLog.txt"),
            ("LoggingCombat", true, "WoWCombatLog.txt"),
        ] {
            let dir = scratch(&format!("logging-open-{file}"));
            let mut world = world_with_logs(Some(dir.clone()));
            super::super::run_chat(&mut world, &format!("ANSWER = {verb}(1)")).expect("lua");
            let script = world.non_send_resource::<UiScript>();
            assert_eq!(script.eval::<i64>("return ANSWER").unwrap(), 1, "{verb}(1)");
            let logs = world.resource::<ChatLogFiles>().clone();
            logs.record(combat, "a line");
            let read = || std::fs::read_to_string(dir.join(file)).expect("the file opened");
            assert!(read().ends_with("  a line\n"), "{verb}: {:?}", read());
            // Off stops the writes; the file stays open.
            super::super::run_chat(&mut world, &format!("{verb}(nil)")).expect("lua");
            logs.record(combat, "while off");
            assert!(!read().contains("while off"), "{verb}: {:?}", read());
            assert!(logs.files().log(combat).file.is_some(), "{verb}: kept open");
            // On again appends to the same file.
            super::super::run_chat(&mut world, &format!("{verb}(1)")).expect("lua");
            logs.record(combat, "again");
            let text = read();
            assert!(
                text.contains("a line") && text.ends_with("  again\n"),
                "{verb}: {text:?}"
            );
            // The teardown closes it and clears the flag.
            logs.close_all();
            logs.record(combat, "after teardown");
            assert!(!read().contains("after teardown"), "{verb}");
            assert!(logs.files().log(combat).file.is_none(), "{verb}: closed");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
