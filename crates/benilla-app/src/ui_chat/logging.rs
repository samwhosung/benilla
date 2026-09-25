//! The `LoggingChat`/`LoggingCombat` files, `WoWChatLog.txt` and `WoWCombatLog.txt`: the stock
//! `/chatlog` and `/combatlog` flip the VM's flag and print the notice (`ChatFrame.lua:675-695`);
//! this appends each line as the window shows it, stamped `M/D HH:MM:SS.mmm` in UTC, as the
//! process has no local time zone. Deviation: the files live in `benilla-config/Logs/`, not the
//! install's `Logs`, because the install is read-only. A file that fails to open leaves the flag
//! set: Lua already printed "enabled".

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

/// `M/D HH:MM:SS.mmm` of now, UTC.
fn stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let (_, month, day) = civil_from_days((secs / 86_400) as i64);
    let of_day = secs % 86_400;
    format!(
        "{month}/{day} {:02}:{:02}:{:02}.{:03}",
        of_day / 3600,
        (of_day / 60) % 60,
        of_day % 60,
        now.subsec_millis()
    )
}

/// Days since 1970-01-01 → `(year, month, day)`, proleptic Gregorian: the `civil_from_days`
/// algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
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
            .in_set(crate::char_select::InWorldGated),
    );
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn the_civil_conversion_lands_on_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_700), (2026, 9, 4));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
