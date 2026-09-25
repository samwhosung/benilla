//! The stuck-main-thread self-sampler (`WOW_STALL_SAMPLE=0` to disable; macOS only): a watchdog
//! thread notices the main-thread heartbeat has gone stale and shells the stock profiler
//! (`/usr/bin/sample`) at our own PID, so an intermittent stall or teardown hang **diagnoses
//! itself the next time it happens** — on anyone's run, no reproduction needed. **Except while
//! an audio device is open**: `sample` suspends every thread of the task,
//! CoreAudio's realtime IO thread included, and each suspended cycle is a crackle — the one
//! the director heard on every desktop switch and world entry, because those are exactly the
//! ≥600 ms main-thread stalls this watchdog fires on. In-frame sampling now waits for a run
//! nobody is listening to; `WOW_STALL_SAMPLE=force` opts a sounding run back in, knowingly. Two bugs that
//! each lost their reproduction motivated it: the ~1 s silent frame stalls at
//! the BWL pin, and the on-close beachball the director had to force-quit — where even the probe
//! backstop's `process::exit(0)` can wedge, because libc `exit(3)` runs the same atexit teardown
//! the hang may own. Samples land in `benilla-config/Diagnostics/` and the path prints on stderr
//! (the tracing subscriber may already be gone during teardown).
//!
//! After a teardown sample, an [`EXIT_KILL_MS`] backstop `_exit` ends the wedged process —
//! diagnosis first, then the force-quit the director would otherwise perform by hand.
//! `libc::_exit`, never `process::exit`: it skips the atexit handlers. It exits with the code
//! [`AppExit`] asked for, not a blanket 0 — a capture that already failed its own deadline must
//! not be laundered into a success by the thing that unwedges it.
//!
//! **The backstop is not a diagnostic, and it runs where the samples cannot.** A hermetic capture
//! run has no `benilla-config/` to write into ([`crate::local_state::home`] returns `None` under
//! `$WOW_CAPTURE`), and for one day in 1631 that turned the whole plugin off for exactly the runs
//! that most need a liveness guarantee — an agent-driven run with nobody watching it. So the split
//! is by concern, not by run: the watchdog thread and its `_exit` always run; the sample *file* is
//! written only where there is a folder that may hold one.
//!
//! Verification is end-to-end via two injectors (used by the 0713 probe rounds, kept as standing
//! test affordances): `WOW_STALL_INJECT=<at_secs>:<ms>` sleeps the main thread mid-run, and
//! `WOW_TEARDOWN_INJECT=<ms>` wedges World drop via a resource whose `Drop` sleeps — the
//! director's beachball, synthetically.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::time::Real;

/// The monotonic epoch every heartbeat is measured against. `Instant`, not `SystemTime`:
/// a wall-clock step (NTP) must not fake a stall — an instrument that lies once is poison.
static START: OnceLock<Instant> = OnceLock::new();
/// Last main-thread heartbeat, ms since [`START`]. 0 = no full frame yet, watchdog stays
/// quiet — startup (shader compiles, first loads) never false-positives.
static HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);
/// Latched the frame `AppExit` is written: switches the watchdog to the teardown threshold
/// and arms the post-sample `_exit` backstop.
static EXITING: AtomicBool = AtomicBool::new(false);
/// The status `AppExit` asked to exit with, latched alongside [`EXITING`] so the backstop can
/// honour it. A wedged teardown must not turn a failed run into a passing one — which is exactly
/// what a blanket `_exit(0)` would do to the capture deadline (`capture::deadline`).
static EXIT_CODE: AtomicU8 = AtomicU8::new(0);

/// In-frame staleness that means a real stall (ms) — above the ~250 ms loading-screen
/// hitches, below the ~1030 ms stall class it exists to catch.
const STALL_MS: u64 = 600;
/// In-frame sampling stays disarmed this long after launch: startup/login legitimately
/// stalls past [`STALL_MS`] (a 683 ms hitch at 1.7 s uptime, caught by the 0713 pristine-run
/// gate), and the classes this instrument hunts are steady-state. Teardown is unaffected.
const STARTUP_GRACE_MS: u64 = 15_000;
/// Post-`AppExit` staleness that means teardown is wedged, not merely slow (ms). A clean
/// teardown is sub-second (26/26 measured exit cycles); the probe hard-exit backstop sits
/// at 5 s, so a 2.5 s trigger + 2 s capture completes before it.
const EXIT_STALL_MS: u64 = 2_500;
/// Teardown backstop: still alive this long after `AppExit` (sample already taken) → `_exit`.
const EXIT_KILL_MS: u64 = 8_000;
/// Rate limit: at most this many samples per run, at least [`SAMPLE_GAP_MS`] apart.
const SAMPLE_CAP: u32 = 5;
const SAMPLE_GAP_MS: u64 = 20_000;

fn since_start_ms() -> u64 {
    START.get().map_or(0, |s| s.elapsed().as_millis() as u64)
}

/// `Last`-schedule heartbeat: stamp the clock; latch [`EXITING`] the frame the app decides
/// to quit (no later frame will run to unlatch anything).
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
    // The injectors are armed FIRST, deliberately: they are how the wedge is reproduced, and the
    // one A/B this instrument exists to support is "same wedge, watchdog off — does the process
    // still hang?". Registered after the `WOW_STALL_SAMPLE=0` gate, as they were until 1637, that
    // control silently disarms the wedge too and comes back a clean exit 0, which reads as "the
    // watchdog was never load-bearing" — the exact wrong conclusion. Arming a deliberate hang is
    // never something the sampler's off-switch should be able to veto.
    arm_injectors(app);
    if std::env::var("WOW_STALL_SAMPLE").is_ok_and(|v| v == "0") {
        return;
    }
    // Through `local_state`, never a hand-built path — the one-folder rule. This used to read
    // `$HOME` and write `~/Library/Logs/benilla/`; see `local_state::diagnostics_dir`. `None` is a
    // hermetic capture run, which gets **no sample file and the watchdog anyway** — see the header:
    // 1631 returned here instead and cost every capture its teardown backstop.
    //
    // The folder is NOT created here: an empty `Diagnostics/` on every single run would be this
    // instrument advertising itself in the player's folder for the ~always-case where nothing ever
    // stalls. The watchdog creates it at the moment it actually has a sample to write.
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

/// The injectors (module doc): a mid-run main-thread sleep, and a wedged World drop. Both are
/// standing test affordances, not diagnostics — see why they are armed ahead of the off-switch.
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
    // The third standing injector — `crash::arm_injector`, a deliberate main-thread panic — is
    // armed by `PerfPlugin` itself, beside this plugin's registration, not from here: this
    // module is macOS-only and the crash injector is not (its doc has the cross-platform incident).
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
            // SAFETY: terminates the process immediately; no locks, no atexit — the whole
            // point, since the wedge may own both.
            unsafe { libc::_exit(code as i32) };
        }
        let age = now.saturating_sub(hb);
        if !exiting && now < STARTUP_GRACE_MS {
            continue;
        }
        // `dir` is `None` on a hermetic capture: everything above still runs (the backstop is this
        // thread's other, unconditional job), there is just nowhere to put a sample.
        let Some(dir) = dir.as_deref() else { continue };
        let threshold = if exiting { EXIT_STALL_MS } else { STALL_MS };
        if age > threshold
            && taken < SAMPLE_CAP
            // The gap gates *between* samples only — gating the first one against
            // `last_sample_ms = 0` would silently forbid any sample in the first
            // [`SAMPLE_GAP_MS`] of process uptime (caught by the 0713 injector runs).
            && (taken == 0 || now.saturating_sub(last_sample_ms) > SAMPLE_GAP_MS)
        {
            // **Never while the speakers are live**. `sample` suspends the whole
            // task for each of its ~1000 snapshots — the CoreAudio IO thread and our realtime
            // render thread included — and coreaudiod times the client out: every stall this
            // watchdog sampled in a run with a device open was also a crackle ("client timeout"
            // × 4 per manual sample, zero overloads across 25 hitches with the sampler off).
            // 1232 cleared the sampler on a quiet-machine measurement of the hole; under real
            // load the hole is the sample. The instrument keeps its whole job on the runs that
            // cannot be heard (captures, probes, `WOW_NOSOUND` — no device is open there) and
            // at teardown, where the wedge outranks the last half-second of sound.
            // `WOW_STALL_SAMPLE=force` overrides for a deliberate diagnostic run with sound.
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
            // 1 s in-frame so the stall dominates its own capture (a ~1 s stall in a 2 s
            // window is half idle); 2 s for teardown, where the wedge holds the whole time.
            let _ = std::process::Command::new("/usr/bin/sample")
                .arg(&pid)
                .arg(if exiting { "2" } else { "1" })
                .arg("-file")
                .arg(&file)
                .status();
        }
    }
}

/// `WOW_STALL_INJECT=<at_secs>:<ms>` — one deliberate main-thread sleep, mid-run.
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

/// `WOW_TEARDOWN_INJECT=<ms>` — wedge World drop: this resource's `Drop` runs on the main
/// thread during `App` teardown and sleeps there, reproducing the beachball synthetically.
#[derive(Resource)]
struct TeardownWedge(u64);

impl Drop for TeardownWedge {
    fn drop(&mut self) {
        eprintln!("teardown-inject: wedging World drop {} ms", self.0);
        std::thread::sleep(Duration::from_millis(self.0));
    }
}
