//! The shared debug-trace sink behind `WOW_MOVE_TRACE=<path>`: one file and one clock for every
//! layer's tagged lines, engine and game alike, so it sits below both. `in` and `out` are the full
//! inbound and outbound opcode streams; `out` is written after the send, where the mover's `snd`
//! is a decision before queueing. Each file opens with a `# t0=<unix epoch>` header so two
//! clients' traces align. `WOW_MOVE_TRACE_TAGS="move,snd,in"` keeps only those tags: every line is
//! an unbuffered write under one mutex on the main thread, so a busy tag skews every `t=`.

use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Sink {
    out: File,
    t0: Instant,
}

static SINK: OnceLock<Option<Mutex<Sink>>> = OnceLock::new();

fn sink() -> Option<&'static Mutex<Sink>> {
    SINK.get_or_init(|| {
        let path = std::env::var("WOW_MOVE_TRACE").ok()?;
        let mut out = File::create(&path)
            .map_err(|e| eprintln!("dbg-trace: cannot create {path}: {e}"))
            .ok()?;
        // Wall time is `t0 + t`, which aligns the traces of two processes.
        let t0_wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        let _ = writeln!(out, "# t0={t0_wall:.3} (unix epoch seconds at t=0)");
        Some(Mutex::new(Sink {
            out,
            t0: Instant::now(),
        }))
    })
    .as_ref()
}

/// The tag allow-list from `WOW_MOVE_TRACE_TAGS`; `None`, unset or empty, keeps everything.
static TAGS: OnceLock<Option<Vec<String>>> = OnceLock::new();

/// Whether `tag` survives the allow-list, checked before the mutex so a filtered tag takes no lock.
fn tag_allowed(tag: &str) -> bool {
    TAGS.get_or_init(|| {
        let raw = std::env::var("WOW_MOVE_TRACE_TAGS").ok()?;
        let allow: Vec<String> = raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        (!allow.is_empty()).then_some(allow)
    })
    .as_ref()
    .is_none_or(|allow| allow.iter().any(|t| t == tag))
}

/// Whether the trace is on, so a caller can skip building its line.
pub fn enabled() -> bool {
    sink().is_some()
}

/// [`enabled`] plus the allow-list, for a caller whose line is expensive to build.
pub fn enabled_for(tag: &str) -> bool {
    enabled() && tag_allowed(tag)
}

/// Append one tagged line, stamped with the sink's shared clock.
pub fn line(tag: &str, msg: &str) {
    let Some(sink) = sink() else { return };
    if !tag_allowed(tag) {
        return;
    }
    let Ok(mut s) = sink.lock() else { return };
    let t = s.t0.elapsed().as_secs_f32();
    let _ = writeln!(s.out, "t={t:9.3} {tag:4} {msg}");
}
