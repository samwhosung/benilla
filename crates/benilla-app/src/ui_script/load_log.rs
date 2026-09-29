//! `Logs\FrameXML.log` (`0x842ecc`): the UI loads' records ([`benilla_ui::status`]), written a
//! line at a time as the reference's log layer writes one (`0x65ac20`): the stamp
//! `"%u/%u %02u:%02u:%02u.%03u  "` (`0x866aa0`), the text, then `"\r\n"` (`0x82edfc`). Deviation:
//! the file is `benilla-config/Logs/FrameXML.log`, since the install is read-only.

use std::io::Write as _;

use bevy::prelude::*;

use benilla_ui::status::LogWrite;

/// The log's name, the reference's.
const FILE: &str = "FrameXML.log";

/// Write the VM's drains in the order they were made. A drain with no lines opens nothing, so a
/// clean UI load leaves the last load's file in place, as the reference's lazy open does
/// (`0x65a930`).
pub(super) fn write(writes: Vec<LogWrite>) {
    if writes.iter().all(|w| lines(w).is_empty()) {
        return;
    }
    let Some(dir) = crate::local_state::logs_dir() else {
        return; // hermetic capture, or no state folder
    };
    write_under(&dir, writes);
}

fn lines(write: &LogWrite) -> &[String] {
    match write {
        LogWrite::Rewrite(lines) | LogWrite::Append(lines) => lines,
    }
}

/// [`write`] into `dir`.
fn write_under(dir: &std::path::Path, writes: Vec<LogWrite>) {
    let path = dir.join(FILE);
    for w in writes {
        let (append, lines) = match w {
            LogWrite::Rewrite(lines) => (false, lines),
            LogWrite::Append(lines) => (true, lines),
        };
        if lines.is_empty() {
            continue;
        }
        let mut body = String::new();
        for line in &lines {
            body.push_str(&crate::ui_chat::logging::stamp());
            body.push_str("  ");
            body.push_str(line);
            body.push_str("\r\n");
        }
        let written = std::fs::create_dir_all(dir).and_then(|()| {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(append)
                .truncate(!append)
                .open(&path)?;
            file.write_all(body.as_bytes())
        });
        if let Err(e) = written {
            warn!("ui_script: cannot write {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join(FILE))
            .unwrap()
            .split("\r\n")
            .filter(|l| !l.is_empty())
            // `M/D HH:MM:SS.mmm` and two spaces.
            .map(|l| l.splitn(3, ' ').nth(2).unwrap().trim_start().to_string())
            .collect()
    }

    /// A UI load's drain replaces the file, a `LoadAddOn` drain appends, and an empty UI drain
    /// leaves the last file standing.
    #[test]
    fn a_load_rewrites_a_load_addon_appends_and_a_clean_load_touches_nothing() {
        let dir = std::env::temp_dir().join(format!("benilla-framexml-log-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        write_under(&dir, vec![LogWrite::Rewrite(vec!["first load".into()])]);
        write_under(&dir, vec![LogWrite::Rewrite(vec!["second load".into()])]);
        write_under(&dir, vec![LogWrite::Append(vec!["on demand".into()])]);
        assert_eq!(read(&dir), ["second load", "on demand"]);
        write_under(&dir, vec![LogWrite::Rewrite(vec![])]);
        assert_eq!(read(&dir), ["second load", "on demand"]);
        let raw = std::fs::read_to_string(dir.join(FILE)).unwrap();
        let first = raw.split("\r\n").next().unwrap();
        let (stamp, text) = first.split_at(first.find("  ").unwrap());
        assert_eq!(text, "  second load");
        let (date, time) = stamp.split_once(' ').unwrap();
        assert!(date.split('/').all(|p| p.parse::<u32>().is_ok()), "{date}");
        assert_eq!(time.len(), "HH:MM:SS.mmm".len(), "{time}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
