//! **A crash leaves an artefact**.
//!
//! Until this module, a panic printed to stderr and the process was gone — and with it everything
//! a reporter could have attached. The only log sink was the terminal; a packaged Windows build
//! has no console; of bug B390's four reporters one attached a backtrace, because he had set
//! `RUST_BACKTRACE=1` on his own. `shutdown.rs` says the other half out loud: *"A crash, a
//! `SIGKILL`, a wedged teardown. Nothing survives those."*
//!
//! [`install`] chains a panic hook in front of Rust's default one. The default still prints to
//! stderr exactly as before (so every log a session already greps keeps reading the same); then
//! the report goes to **`benilla-config/Diagnostics/crash-<unix-seconds>.txt`**, resolved through
//! [`crate::local_state`] like every other file benilla writes (the one-folder rule, 0954/1175 —
//! never a platform log dir, never beside the install). The file carries: the build id (the sha
//! is the version, `build_id`), the time and the uptime, the thread, the panic's location and
//! payload, a backtrace captured **regardless of `RUST_BACKTRACE`**, and the last
//! [`benilla_world::log_ring::CAPACITY`] log lines — the minute before, which is what turns a
//! symptom into a story. The path is printed to stderr as the last line, so "attach the file it
//! names" is the whole instruction to a reporter.
//!
//! **What it does not cover, said out loud.** A non-unwinding panic — an allocation failure's
//! abort, a `panic` inside a `Drop` during unwinding, a foreign frame — never reaches any hook;
//! B390's own was one (`thread caused non-unwinding panic. aborting.`). The hook covers the
//! unwinding class (B383's shape: four of the six panics the ledger holds).
//! Flushing saved variables and the CVar diff from inside a panic is a second, riskier item —
//! the world is half-torn by then — and is deliberately not bundled here (2265 §B2).
//!
//! A hermetic capture run (`local_state::home()` answers `None`) writes no file and loses
//! nothing: the harness already has stderr.

use std::fmt::Write as _;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::BuildId;

/// Chain the crash-report hook in front of the current panic hook. Called once, from
/// [`crate::run`], before the `App` exists — a panic while plugins build is still a crash.
pub(crate) fn install(build: BuildId) {
    let started = Instant::now();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        report(info, build, started.elapsed());
    }));
}

// The standing test affordance — `WOW_CRASH_INJECT=<at_secs>`, one deliberate main-thread panic
// mid-run — is `perf::crash_inject`, a dev root: this module ships to players and knows nothing
// of the `dev` seam (run_mode's one-door rule, 1176), and an instrument's home is with the
// instruments. Its module doc carries the cross-platform incident that moved it there.

/// Re-entrancy latch: a panic *inside* the hook (a poisoned lock, a failed write) must not
/// recurse into it. Never cleared on purpose — after one report the process is on its way out,
/// and a second panic's report would only overwrite the useful one with a worse one.
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

/// Everything the report says, gathered before rendering so the renderer is a pure function the
/// test can drive without panicking.
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

/// Write the report under `dir` as `crash-<unix>.txt`, creating the folder on demand (an empty
/// `Diagnostics/` on every run would be this instrument advertising itself, `perf::stall`'s
/// reason). Plain `fs::write`, not `local_state::write_atomic`: the process is dying, and a
/// half-written report beats none.
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

    /// The report names the build, the place, the payload and the log — the four things a
    /// reporter cannot reconstruct after the fact — and lands in the folder it was given.
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
