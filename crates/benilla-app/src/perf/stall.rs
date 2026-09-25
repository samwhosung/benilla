//! The stuck-main-thread self-sampler (macOS only; `WOW_STALL_SAMPLE=0` disables it): a watchdog
//! thread sees the main-thread heartbeat go stale and runs `/usr/bin/sample` on our own PID, so a
//! stall or teardown hang diagnoses itself on any run. Samples land in
//! `benilla-config/Diagnostics/` and the path prints on stderr, since the tracing subscriber may
//! be gone during teardown.
//!
//! In-frame sampling is skipped while an audio device is open: `sample` suspends every thread,
//! the audio IO thread included, and each suspension is a crackle. `WOW_STALL_SAMPLE=force`
//! samples anyway.
//!
//! After a teardown sample, an [`EXIT_KILL_MS`] backstop `libc::_exit`s the wedged process with
//! the code [`AppExit`] asked for; `_exit` skips the atexit teardown the hang may own. The
//! watchdog and its backstop always run; the sample file is written only where
//! `benilla-config/` exists, never under `$WOW_CAPTURE`.
//!
//! Two injectors test it end to end: `WOW_STALL_INJECT=<at_secs>:<ms>` sleeps the main thread
//! mid-run, and `WOW_TEARDOWN_INJECT=<ms>` wedges World drop.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::time::Real;

/// The monotonic epoch of every heartbeat, so a wall-clock step cannot fake a stall.
static START: OnceLock<Instant> = OnceLock::new();
/// Last main-thread heartbeat, ms since [`START`]; 0 until the first full frame, and the
/// watchdog stays quiet until then.
static HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);
/// Latched the frame `AppExit` is written: switches the watchdog to the teardown threshold
/// and arms the post-sample `_exit` backstop.
static EXITING: AtomicBool = AtomicBool::new(false);
/// The status `AppExit` asked for, so the backstop never turns a failed run into a passing one.
static EXIT_CODE: AtomicU8 = AtomicU8::new(0);

/// In-frame staleness that means a real stall (ms), above the ~250 ms loading-screen hitches.
const STALL_MS: u64 = 600;
/// In-frame sampling stays disarmed this long after launch, since startup legitimately stalls
/// past [`STALL_MS`]; teardown is unaffected.
const STARTUP_GRACE_MS: u64 = 15_000;
/// Post-`AppExit` staleness that means teardown is wedged (ms); with a 2 s sample it finishes
/// inside the probe's 5 s hard-exit backstop.
const EXIT_STALL_MS: u64 = 2_500;
/// Teardown backstop: still alive this long after `AppExit`, the process `_exit`s.
const EXIT_KILL_MS: u64 = 8_000;
/// Rate limit: at most this many samples per run, at least [`SAMPLE_GAP_MS`] apart.
const SAMPLE_CAP: u32 = 5;
const SAMPLE_GAP_MS: u64 = 20_000;

fn since_start_ms() -> u64 {
    START.get().map_or(0, |s| s.elapsed().as_millis() as u64)
}

/// `Last`-schedule heartbeat; latches [`EXITING`] the frame the app decides to quit.
fn beat(mut exits: MessageReader<AppExit>) {
    if let Some(exit) = exits.read().next() {
        if let AppExit::Error(code) = exit {
            EXIT_CODE.store(code.get(), Ordering::SeqCst);
        }
        EXITING.store(true, Ordering::SeqCst);
    }
    HEARTBEAT_MS.store(since_start_ms().max(1), Ordering::SeqCst);
}

pub(super) fn plugin(app: &mut App) {
    // Armed before the off-switch, so "same wedge, watchdog off" still wedges.
    arm_injectors(app);
    if std::env::var("WOW_STALL_SAMPLE").is_ok_and(|v| v == "0") {
        return;
    }
    // `None` on a hermetic capture, which still gets the watchdog. The folder is created only
    // when there is a sample to write.
    let dir = crate::local_state::diagnostics_dir();
    START
        .set(Instant::now())
        .expect("stall_sample plugin built twice");
    app.add_systems(Last, beat);
    std::thread::Builder::new()
        .name("stall-sample".into())
        .spawn(move || watchdog(dir))
        .expect("spawn stall-sample watchdog");
}

/// The injectors: a mid-run main-thread sleep, and a wedged World drop.
fn arm_injectors(app: &mut App) {
    if let Some((at, ms)) = std::env::var("WOW_STALL_INJECT")
        .ok()
        .and_then(|v| v.split_once(':').map(|(a, m)| (a.to_owned(), m.to_owned())))
        .and_then(|(a, m)| Some((a.parse::<f32>().ok()?, m.parse::<u64>().ok()?)))
    {
        app.insert_resource(StallInject {
            at,
            ms,
            fired: false,
        })
        .add_systems(Update, stall_inject);
    }
    if let Ok(ms) = std::env::var("WOW_TEARDOWN_INJECT") {
        if let Ok(ms) = ms.parse::<u64>() {
            app.insert_resource(TeardownWedge(ms));
        }
    }
    // The crash injector is armed by `PerfPlugin`, not here: this module is macOS-only.
}

fn watchdog(dir: Option<std::path::PathBuf>) {
    let pid = std::process::id().to_string();
    let force = std::env::var("WOW_STALL_SAMPLE").is_ok_and(|v| v == "force");
    let mut declined = false;
    let mut taken = 0u32;
    let mut last_sample_ms = 0u64;
    let mut exit_seen_ms = 0u64;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let hb = HEARTBEAT_MS.load(Ordering::SeqCst);
        if hb == 0 {
            continue;
        }
        let now = since_start_ms();
        let exiting = EXITING.load(Ordering::SeqCst);
        if exiting && exit_seen_ms == 0 {
            exit_seen_ms = now;
        }
        if exiting && now.saturating_sub(exit_seen_ms) > EXIT_KILL_MS {
            let code = EXIT_CODE.load(Ordering::SeqCst);
            eprintln!(
                "stall-sample: teardown still wedged {} ms after AppExit — _exit({code})",
                now.saturating_sub(exit_seen_ms)
            );
            use std::io::Write;
            let _ = std::io::stdout().flush();
            // SAFETY: terminates the process at once, taking no locks and running no atexit
            // handlers, which the wedge may own.
            unsafe { libc::_exit(code as i32) };
        }
        let age = now.saturating_sub(hb);
        if !exiting && now < STARTUP_GRACE_MS {
            continue;
        }
        // On a hermetic capture the backstop above still runs; there is nowhere to put a sample.
        let Some(dir) = dir.as_deref() else { continue };
        let threshold = if exiting { EXIT_STALL_MS } else { STALL_MS };
        if age > threshold
            && taken < SAMPLE_CAP
            // The gap gates between samples only, or no sample could land in the first
            // `SAMPLE_GAP_MS` of uptime.
            && (taken == 0 || now.saturating_sub(last_sample_ms) > SAMPLE_GAP_MS)
        {
            // Not while an audio device is open: `sample` suspends the audio IO thread and the
            // sound server times the client out, a crackle. Teardown samples regardless.
            if !exiting && crate::sound::output::device_open() && !force {
                if !declined {
                    eprintln!(
                        "stall-sample: main thread stale {age} ms — NOT sampled: an audio \
                         device is open, and `sample` suspending the process is itself a \
                         crackle (1857). `WOW_STALL_SAMPLE=force` to sample anyway."
                    );
                    declined = true;
                }
                continue;
            }
            taken += 1;
            last_sample_ms = now;
            let _ = std::fs::create_dir_all(dir);
            let file = dir.join(format!(
                "stall-{}{}.txt",
                std::process::id(),
                if exiting {
                    format!("-teardown-{now}")
                } else {
                    format!("-{now}")
                }
            ));
            eprintln!(
                "stall-sample: main thread stale {age} ms{} — sampling to {}",
                if exiting { " (teardown)" } else { "" },
                file.display()
            );
            // 1 s in-frame so the stall dominates the capture; 2 s at teardown, where the wedge
            // holds throughout.
            let _ = std::process::Command::new("/usr/bin/sample")
                .arg(&pid)
                .arg(if exiting { "2" } else { "1" })
                .arg("-file")
                .arg(&file)
                .status();
        }
    }
}

/// `WOW_STALL_INJECT=<at_secs>:<ms>`: one deliberate main-thread sleep, mid-run.
#[derive(Resource)]
struct StallInject {
    at: f32,
    ms: u64,
    fired: bool,
}

fn stall_inject(mut inject: ResMut<StallInject>, time: Res<Time<Real>>) {
    if !inject.fired && time.elapsed_secs() >= inject.at {
        inject.fired = true;
        warn!("stall-inject: sleeping the main thread {} ms", inject.ms);
        std::thread::sleep(Duration::from_millis(inject.ms));
    }
}

/// `WOW_TEARDOWN_INJECT=<ms>`: this resource's `Drop` sleeps on the main thread during `App`
/// teardown, wedging World drop.
#[derive(Resource)]
struct TeardownWedge(u64);

impl Drop for TeardownWedge {
    fn drop(&mut self) {
        eprintln!("teardown-inject: wedging World drop {} ms", self.0);
        std::thread::sleep(Duration::from_millis(self.0));
    }
}
