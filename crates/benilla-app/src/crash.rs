//! A panic leaves a crash report. [`install`] chains a hook after Rust's default one, which still
//! prints to stderr, and writes `benilla-config/Diagnostics/crash-<unix-seconds>.txt` through
//! [`crate::local_state`]: the build id, time and uptime, thread, location and payload, a
//! backtrace captured regardless of `RUST_BACKTRACE`, and the last
//! [`benilla_world::log_ring::CAPACITY`] log lines. The path is the last stderr line.
//!
//! A non-unwinding panic (an allocation-failure abort, a panic in `Drop` during unwinding) never
//! reaches a hook. Saved variables and the CVar diff are not flushed from inside a panic.
//! Without a local state home (a hermetic capture run) no file is written.

use std::fmt::Write as _;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::BuildId;

/// Chains the crash-report hook after the current one. Called once from [`crate::run`], before
/// the `App` exists, so a panic while plugins build is covered.
pub(crate) fn install(build: BuildId) {
    let started = Instant::now();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        report(info, build, started.elapsed());
    }));
}

// The injected test panic (`WOW_CRASH_INJECT=<at_secs>`) lives in `perf::crash_inject`, behind
// the `dev` feature.

/// Re-entrancy latch, never cleared: a panic inside the hook must not recurse, and only the
/// first report is kept.
static REPORTING: AtomicBool = AtomicBool::new(false);

fn report(info: &PanicHookInfo<'_>, build: BuildId, uptime: Duration) {
    if REPORTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(dir) = crate::local_state::diagnostics_dir() else {
        return;
    };
    let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_owned()
    };
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_owned());
    let thread = std::thread::current();
    let text = render(&Report {
        build,
        unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        uptime,
        thread: thread.name().unwrap_or("<unnamed>"),
        location: &location,
        payload: &payload,
        backtrace: &std::backtrace::Backtrace::force_capture().to_string(),
        log: &benilla_world::log_ring::recent(),
    });
    if let Some(path) = write(&dir, &text) {
        eprintln!("crash report written to {}", path.display());
    }
}

/// Everything the report says, gathered so rendering is pure.
struct Report<'a> {
    build: BuildId,
    unix: u64,
    uptime: Duration,
    thread: &'a str,
    location: &'a str,
    payload: &'a str,
    backtrace: &'a str,
    log: &'a [String],
}

fn render(r: &Report<'_>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "benilla crash report");
    let _ = writeln!(
        out,
        "build:     {} (sha {})",
        r.build.summary(),
        r.build.sha
    );
    let _ = writeln!(
        out,
        "time:      {} unix, {:.1} s after launch",
        r.unix,
        r.uptime.as_secs_f64()
    );
    let _ = writeln!(
        out,
        "platform:  {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(out, "thread:    {}", r.thread);
    let _ = writeln!(out, "at:        {}", r.location);
    let _ = writeln!(out, "panic:     {}", r.payload);
    let _ = writeln!(out, "\nbacktrace:\n{}", r.backtrace);
    let _ = writeln!(out, "last {} log lines (oldest first):", r.log.len());
    for line in r.log {
        let _ = writeln!(out, "{line}");
    }
    out
}

/// Writes `crash-<unix>.txt` under `dir`, creating the folder only now. Plain `fs::write`, not
/// `write_atomic`: the process is dying, and a partial report beats none.
fn write(dir: &Path, text: &str) -> Option<PathBuf> {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{unix}.txt"));
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(&path, text).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: BuildId = BuildId {
        sha: "0123456789abcdef0123456789abcdef01234567",
        short: "0123456",
        date: "2026-09-16",
        profile: "debug",
    };

    #[test]
    fn a_report_carries_the_build_the_place_the_payload_and_the_log() {
        let log = vec!["+  1.000s  INFO benilla_app::net: entered world".to_owned()];
        let text = render(&Report {
            build: BUILD,
            unix: 1_800_000_000,
            uptime: Duration::from_millis(12_345),
            thread: "main",
            location: "crates/benilla-app/src/net/apply.rs:100:5",
            payload: "index out of bounds: the len is 3 but the index is 7",
            backtrace: "   0: benilla_app::net::apply::apply_net_updates",
            log: &log,
        });
        for needle in [
            "sha 0123456789abcdef0123456789abcdef01234567",
            "0123456 · 2026-09-16 · debug",
            "12.3 s after launch",
            "thread:    main",
            "at:        crates/benilla-app/src/net/apply.rs:100:5",
            "panic:     index out of bounds: the len is 3 but the index is 7",
            "apply_net_updates",
            "last 1 log lines",
            "entered world",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }

        let dir = std::env::temp_dir().join(format!("benilla-crash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = write(&dir, &text).expect("report written");
        assert!(path.starts_with(&dir), "{}", path.display());
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("crash-"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
