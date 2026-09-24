//! The shared debug-trace sink behind `WOW_MOVE_TRACE=<path>`: one file, one clock, written by
//! the player mover's frame lines (`benilla::player`'s `move_trace`), its outbound wire lines
//! (`snd`), the anim driver's event lines (`benilla::creature_anim`), and the remote-replay lines
//! (`rly`/`run`, `benilla::net::motion`) — so the layers interleave on a common timeline and a feel
//! report ("it snaps when I land") can be read across all of them.
//!
//! **The two wire tags are `in` and `out`, and between them every packet is accounted for by
//! name.** `in` is the full inbound opcode stream; `out` is the full outbound one,
//! written by the write thread from the writer's own post-write log — so an `out`
//! line is a *transmission*, where the mover's `snd` line is a decision taken before the command is
//! even queued and `wire` carries only failures. Each file opens with a
//! `# t0=<unix epoch>` header so **two clients' traces align with each other**, which is what a
//! sender-vs-observer question needs. Costs one `OnceLock` read per call when the env var is unset.
//!
//! **It lives down here, below both sides of decision 1160's line, because both sides write to it**
//! — the engine's billboard/zfill/particle traces and the game's mover and wire traces share one
//! file and one clock, which is the whole point of it. An instrument at the *top* of the stack
//! could not be called by the engine at all; a leaf with no dependencies can be called by
//! everything. It carries no domain knowledge: a tag, a message, a mutex and a file.
//!
//! **`WOW_MOVE_TRACE_TAGS="move,snd,in"` narrows it to those tags** (unset = everything). This is
//! not tidiness: every line is an unbuffered `write` under one global mutex on the main thread, so a
//! busy tag distorts the run it is meant to measure. A 600-yd fall probe wrote
//! ~4,200 lines/s, nearly all of them `card`, and the `move` line's own `t=` then carried whatever
//! the queue ahead of it cost — frames read back as 0.011 s with 2.86 yd of travel, or 0.040 s with
//! 0.96 yd, against a physics integration that was exact. A trace whose clock cannot be trusted for
//! per-frame timing is half an instrument; filtering to the tags the question needs gives it back.

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
        // The wall-clock epoch of this file's `t=0`, so **two traces can be read against each other**
        // — a sender's `snd` lines beside an observer's `rly`/`run` lines from a different process.
        // `t=` alone is per-process seconds and says nothing across clients; with
        // this header, wall time is `t0 + t`.
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

/// The optional tag allow-list from `WOW_MOVE_TRACE_TAGS`; `None` (unset or empty) = keep everything.
static TAGS: OnceLock<Option<Vec<String>>> = OnceLock::new();

/// Whether `tag` survives the allow-list. Checked **before** the mutex, so a filtered-out tag costs
/// a slice compare rather than a lock and a syscall — which is the whole point (see the module note).
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

/// Whether the trace is enabled — lets callers skip building their line when it's off.
pub fn enabled() -> bool {
    sink().is_some()
}

/// Whether this *tag* is being written — [`enabled`] plus the allow-list, for a caller whose line is
/// expensive to build (a per-frame `format!` over a hot loop) and which is filtered out anyway.
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
