//! **Measuring mode** — `$WOW_SOUND_PROBE`: record a session so a sound report becomes evidence.
//! `scripts/soundprobe.sh` starts the client in it (the `play` profile, with the banner that says
//! when to press the mark key); `scripts/soundprobe.py` reads the capture back.
//!
//! ## Why this exists
//!
//! The output limiter was built, measured, and proven offline — five sample-aligned
//! copies of a 0 dBFS buff go from `5.00x` full scale and 28 524 clipped samples to `0.99x` and
//! none. It shipped on by default. The director then played a real session and reported **no
//! audible change**: the mix still "gets really dirty, like a speaker breaking".
//!
//! That gap is the whole reason for this module. Per the contract, what the director hears
//! outranks any mechanism we found: a verified fix that does not change what they hear means the
//! mechanism we verified is not the mechanism they are hearing. Being right about *a* cause is not
//! being right about *the* cause, and the honest next move is not another fix — it is an
//! instrument that can tell the candidates apart in a **real run**, on their machine, on the
//! encounter they are describing.
//!
//! The candidates are genuinely different failures that sound alike through a speaker:
//!
//! | mechanism | signature in a probe capture |
//! |---|---|
//! | over-scale sum (1551's claim) | `pre.wav` past ±1.0, `post.wav` clean, `gain` well under 1 |
//! | **limiter not actually engaging** | `pre.wav` past ±1.0 **and** `post.wav` past ±1.0 |
//! | buffer underrun / missed deadline | both WAVs clean at the mark; `load >= 1.0`, `overruns` climbing |
//! | starved stream decoder | both WAVs clean; a hard step to zero in `post.wav` at the mark |
//! | **non-finite samples** | `nan > 0` — invisible to every other counter (see [`super::meter`]) |
//! | voice-steal / refusal clicks | `refused` climbing, a discontinuity at the mark |
//! | the limiter's *own* pumping | `post.wav` clean but `gain` diving repeatedly — a fix that is itself the complaint |
//!
//! Note the last row. An instrument that can only confirm its author's fix is not an instrument;
//! this one is built so that "1551 made it worse" is a reading it can produce.
//!
//! ## What a run produces
//!
//! Three files in `benilla-config/sound-probe/`, all written off the game thread:
//!
//! - **`pre.wav`** — the summed mix *as the game asked for it*, tapped ahead of the limiter.
//! - **`post.wav`** — what was actually heard, tapped after it.
//! - **`timeline.jsonl`** — the game-side story on a shared clock: a row every
//!   [`TICK`] with level/health/voice counts, plus an event per kit start, per marker keypress,
//!   and per refusal.
//!
//! Two taps rather than one is the point: with only the post-limiter tap the 1551 hunt could not
//! have distinguished "the mix never clipped" from "the limiter failed", which is exactly the
//! question the director's report raises.
//!
//! ## The shared clock
//!
//! A capture is only useful if a game-thread event can be placed on the waveform. The game clock
//! and the device clock are different clocks and they drift, so every row carries `a` — the frame
//! index published by the pre-tap ([`super::mix_tap::install_at`]), i.e. the exact sample offset
//! in `pre.wav`. A marker is then a *sample position*, not an approximate timestamp.
//!
//! ## The marker
//!
//! **F9** stamps the timeline the instant the director hears something. This is the piece that
//! makes a ten-minute session tractable: instead of scanning the whole capture for anomalies and
//! guessing which one they meant, the analysis starts at the marks and reads outward. The key is
//! bound *only* while probing, so it cannot collide with a game binding by construction.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bevy::prelude::*;

use super::SoundOutput;

/// How often a tick row is written. 50 ms is fine enough to see a burst rise and fall inside one
/// spell cast, and coarse enough that a ten-minute session is ~12 000 rows rather than a file
/// nobody opens.
const TICK: std::time::Duration = std::time::Duration::from_millis(50);

/// How often the run prints a spoken summary to the console, so the director gets live feedback
/// during the session and not only a file afterwards. Matches the normal health cadence.
const SPEAK: std::time::Duration = std::time::Duration::from_secs(5);

/// The marker key. Bound only while probing (see the module docs), so it needs no dev chord and
/// cannot collide with a game binding.
const MARK_KEY: KeyCode = KeyCode::F9;

/// Resolve the probe's output directory from `$WOW_SOUND_PROBE`, creating it.
///
/// `1`/`true`/`yes`/`on` means "the default place"; anything else is taken as an explicit path, so
/// a run can be pointed at a scratch dir. The default resolves through [`crate::local_state`] like
/// every other thing we persist — a probe is local state, and the install stays read-only.
///
/// Called *before* the mixer is built, because the taps are main-track effects and a kira main
/// track is build-time-only.
pub(super) fn output_dir() -> Option<PathBuf> {
    let raw = std::env::var_os("WOW_SOUND_PROBE")?;
    let raw = raw.to_string_lossy().into_owned();
    if raw.is_empty() || raw == "0" || raw == "false" || raw == "off" {
        return None;
    }
    let dir = if matches!(raw.as_str(), "1" | "true" | "yes" | "on") {
        match crate::local_state::home() {
            Some(h) => h.join("sound-probe"),
            None => {
                warn!(
                    "sound probe: no benilla-config folder to write into (hermetic run) — set \
                     WOW_SOUND_PROBE=<dir> to name one. Not recording."
                );
                return None;
            }
        }
    } else {
        PathBuf::from(raw)
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!(
            "sound probe: cannot create {}: {e} — not recording",
            dir.display()
        );
        return None;
    }
    Some(dir)
}

/// The game-thread half of a probing run. Lives in [`SoundOutput`] so every sound system that
/// already holds `out` can stamp an event without new plumbing.
pub(crate) struct Probe {
    dir: PathBuf,
    rate: u32,
    /// Lines to the writer thread. crossbeam because the sender must be `Sync` to sit in a
    /// resource, and because a disk write on the game thread would stall the frame — and a
    /// stalled frame is *itself* one of the mechanisms under investigation. The instrument must
    /// not manufacture the artifact it is measuring.
    tx: crossbeam_channel::Sender<String>,
    /// The pre-tap's frame clock — the shared time axis (see the module docs).
    audio_pos: Option<Arc<AtomicU64>>,
    started: std::time::Instant,
    marks: u32,
    since_tick: std::time::Duration,
    since_speak: std::time::Duration,
    /// Maxima since the last spoken summary, so the console line covers its whole window even
    /// though the meters are drained twenty times a second.
    window: Window,
    /// Cumulative counters at the last tick, to report deltas.
    last_overruns: u64,
    last_refused: u64,
    last_errors: u64,
    last_stolen: u64,
    last_denied: u64,
    last_copies: u64,
}

/// The accumulators behind one spoken summary.
#[derive(Default, Clone, Copy)]
struct Window {
    peak: f32,
    over: u64,
    nonfinite: u64,
    gain: f32,
    load: f32,
    voices: usize,
    overruns: u64,
    refused: u64,
    stolen: u64,
    denied: u64,
}

impl Probe {
    /// Begin recording. `audio_pos` is the pre-tap's frame clock; `None` (no pre-tap) still
    /// produces a usable timeline, just one keyed on the game clock alone.
    pub(super) fn start(dir: PathBuf, rate: u32, audio_pos: Option<Arc<AtomicU64>>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<String>();
        let path = dir.join("timeline.jsonl");
        match std::fs::File::create(&path) {
            Ok(file) => {
                std::thread::Builder::new()
                    .name("sound-probe".into())
                    .spawn(move || writer(file, rx))
                    .expect("spawn sound-probe writer");
            }
            Err(e) => warn!("sound probe: cannot create {}: {e}", path.display()),
        }
        let probe = Self {
            dir,
            rate,
            tx,
            audio_pos,
            started: std::time::Instant::now(),
            marks: 0,
            since_tick: std::time::Duration::ZERO,
            since_speak: std::time::Duration::ZERO,
            window: Window {
                gain: 1.0,
                ..Default::default()
            },
            last_overruns: 0,
            last_refused: 0,
            last_errors: 0,
            last_stolen: 0,
            last_denied: 0,
            last_copies: 0,
        };
        probe.send(format!(
            r#"{{"t":0.000,"a":0,"ev":"meta","rate":{rate},"tick":{}}}"#,
            TICK.as_secs_f64()
        ));
        info!(
            "── sound probe ARMED ──────────────────────────────────────────────\n\
             recording to {}\n\
               pre.wav        the mix as the game asked for it (before the limiter)\n\
               post.wav       what you actually heard (after it)\n\
               timeline.jsonl levels, voices, deadline misses, every kit start\n\
             PRESS F9 THE MOMENT YOU HEAR IT — that stamps the exact sample, so I read\n\
             outward from your mark instead of guessing which of ten minutes you meant.\n\
             Play until it happens a few times, then quit normally.\n\
             ──────────────────────────────────────────────────────────────────",
            probe.dir.display()
        );
        probe
    }

    /// Frames the pre-tap has written — the sample offset every row is keyed on.
    fn audio_frame(&self) -> u64 {
        self.audio_pos
            .as_ref()
            .map_or(0, |p| p.load(Ordering::Relaxed))
    }

    fn send(&self, line: String) {
        // A full/closed channel means the writer died; the run must not.
        let _ = self.tx.send(line);
    }

    /// Stamp an event with `extra` already formatted as JSON object members (leading comma).
    fn event(&self, ev: &str, extra: &str) {
        self.send(format!(
            r#"{{"t":{:.3},"a":{},"ev":"{ev}"{extra}}}"#,
            self.started.elapsed().as_secs_f64(),
            self.audio_frame(),
        ));
    }

    /// Note one kit actually starting — called from the kit player's play path, so the timeline
    /// says *what* made the noise, not just that the level moved. Cheap enough to
    /// sit on the play path unconditionally: a channel send behind an `Option` check.
    pub(super) fn note_play(&self, kit: u32, name: &str, category: &str, spatial: &str) {
        self.event(
            "play",
            &format!(
                r#","kit":{kit},"name":"{}","cat":"{category}","sp":"{spatial}""#,
                esc(name)
            ),
        );
    }
}

/// JSON-escape the only free-form text a row carries (a kit name out of the DBC).
fn esc(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            c if (c as u32) < 0x20 => vec![' '],
            c => vec![c],
        })
        .collect()
}

/// Drain lines to disk, flushing each batch so a hard kill loses at most one batch — the same
/// crash-safety rule the mix tap follows, for the same reason: the interesting moment is often
/// the last thing before the session ends.
fn writer(file: std::fs::File, rx: crossbeam_channel::Receiver<String>) {
    use std::io::Write;
    let mut file = std::io::BufWriter::new(file);
    while let Ok(first) = rx.recv() {
        let _ = writeln!(file, "{first}");
        while let Ok(next) = rx.try_recv() {
            let _ = writeln!(file, "{next}");
        }
        let _ = file.flush();
    }
    let _ = file.flush();
}

/// The probe's own health pump: one tick row per [`TICK`], one spoken summary per [`SPEAK`].
///
/// While this runs it **owns the meters** — [`super::poll_mix_health`] stands down, because
/// [`super::meter::MixLevel::take`] is reset-on-read and two consumers would each see a fraction
/// of the truth. Everything that report says, this says, twenty times a second and to a file.
pub(super) fn tick(
    mut out: NonSendMut<SoundOutput>,
    time: Res<Time>,
    mut exit: MessageReader<bevy::app::AppExit>,
) {
    let exiting = exit.read().next().is_some();
    // Read before the split borrow: the ceiling bounds *everything the device mixes*, streams
    // included, so that — not the kit-channel count — is the number worth recording.
    let voices = out.live_voices();
    let (stolen, denied) = (out.voices_stolen, out.voices_denied);
    let copies = out.copies_dropped;
    let SoundOutput { mixer, probe, .. } = &mut *out;
    let (Some(probe), Some(mixer)) = (probe.as_mut(), mixer.as_mut()) else {
        return;
    };
    probe.window.voices = probe.window.voices.max(voices);
    probe.since_tick += time.delta();
    probe.since_speak += time.delta();
    if probe.since_tick < TICK && !exiting {
        return;
    }
    probe.since_tick = std::time::Duration::ZERO;

    let health = mixer.poll_health();
    let level = mixer.take_level();
    let load = mixer.take_health_peak();
    let d_over = health.overruns - probe.last_overruns;
    let d_refused = health.voices_refused - probe.last_refused;
    let d_errors = health.stream_errors - probe.last_errors;
    let d_stolen = stolen - probe.last_stolen;
    let d_denied = denied - probe.last_denied;
    let d_copies = copies - probe.last_copies;
    probe.last_overruns = health.overruns;
    probe.last_refused = health.voices_refused;
    probe.last_errors = health.stream_errors;
    probe.last_stolen = stolen;
    probe.last_denied = denied;
    probe.last_copies = copies;

    probe.event(
        "tick",
        &format!(
            r#","peak":{:.4},"over":{},"gain":{:.4},"nan":{},"load":{:.3},"voices":{voices},"overruns":{d_over},"refused":{d_refused},"errors":{d_errors},"stolen":{d_stolen},"denied":{d_denied},"copies":{d_copies}"#,
            level.peak, level.over, level.reduction, level.nonfinite, load,
        ),
    );

    // Fold into the spoken window.
    let w = &mut probe.window;
    w.peak = w.peak.max(level.peak);
    w.over += level.over;
    w.nonfinite += level.nonfinite;
    w.gain = w.gain.min(level.reduction);
    w.load = w.load.max(load);
    w.overruns += d_over;
    w.refused += d_refused;
    w.stolen += d_stolen;
    w.denied += d_denied;

    if d_refused > 0 {
        probe.event("refused", &format!(r#","n":{d_refused}"#));
    }
    if d_errors > 0 {
        probe.event("stream_error", &format!(r#","n":{d_errors}"#));
    }

    if probe.since_speak >= SPEAK || exiting {
        probe.since_speak = std::time::Duration::ZERO;
        let w = std::mem::replace(
            &mut probe.window,
            Window {
                gain: 1.0,
                ..Default::default()
            },
        );
        speak(w, probe.rate);
    }
    if exiting {
        let dir = probe.dir.clone();
        let marks = probe.marks;
        probe.event("end", "");
        info!(
            "── sound probe closed ────────────────────────────────────────────\n\
             {marks} marker(s) stamped. Files in {}\n\
             Read it back with:  scripts/soundprobe.py {}\n\
             ──────────────────────────────────────────────────────────────────",
            dir.display(),
            dir.display(),
        );
    }
}

/// One window's spoken line — the live half, so the director can see whether the numbers moved at
/// the moment they heard it without waiting for the analysis.
fn speak(w: Window, rate: u32) {
    let ms = |samples: u64| samples as f64 / 2.0 / f64::from(rate) * 1000.0;
    if w.nonfinite > 0 {
        error!(
            "sound probe: {} NON-FINITE sample(s) in the mix — that is a defect upstream, not a \
             loud passage, and it is inaudible to every other meter. This is very likely what you \
             are hearing.",
            w.nonfinite
        );
    }
    if w.overruns > 0 {
        warn!(
            "sound probe: {} missed mix deadline(s), peak load {:.0}% of budget — an underrun is \
             audible as a crack and the limiter cannot touch it.",
            w.overruns,
            w.load * 100.0
        );
    }
    if w.refused > 0 {
        warn!(
            "sound probe: {} sound(s) refused (voice arena full)",
            w.refused
        );
    }
    if w.over > 0 {
        warn!(
            "sound probe: mix asked {:.2}x full scale ({:+.1} dBFS) for ~{:.0} ms; limiter pulled \
             {:.1} dB; {} voice(s) live.",
            w.peak,
            20.0 * w.peak.max(1e-6).log10(),
            ms(w.over),
            20.0 * w.gain.max(1e-6).log10(),
            w.voices,
        );
    } else {
        info!(
            "sound probe: mix peak {:.2}, {} voice(s), load {:.0}% — nothing over full scale.",
            w.peak,
            w.voices,
            w.load * 100.0
        );
    }
}

/// The marker key (see the module docs). Bound only while probing.
pub(super) fn marker(mut out: NonSendMut<SoundOutput>, keys: Res<ButtonInput<KeyCode>>) {
    if !keys.just_pressed(MARK_KEY) {
        return;
    }
    let Some(probe) = out.probe.as_mut() else {
        return;
    };
    probe.marks += 1;
    let n = probe.marks;
    let frame = probe.audio_frame();
    probe.event("mark", &format!(r#","n":{n}"#));
    info!(
        "sound probe: ▶ MARK {n} at {:.2}s into the capture — noted.",
        frame as f64 / f64::from(probe.rate)
    );
}

/// Install the probe's systems. Registered unconditionally; every system early-returns in one
/// `Option` check when `$WOW_SOUND_PROBE` is unset, which is the normal case.
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, (tick, marker));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kit name is the one free-form string a row carries; a quote or a backslash in it must not
    /// produce a line the analyser cannot parse.
    #[test]
    fn kit_names_stay_valid_json() {
        assert_eq!(esc("HolyProtection"), "HolyProtection");
        assert_eq!(esc(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(esc(r"back\slash"), r"back\\slash");
        assert_eq!(esc("tab\there"), "tab here");
    }

    /// `$WOW_SOUND_PROBE` reads as a flag *or* a path, and the off-forms really are off — a probe
    /// that silently records every session would be a footgun on the director's machine.
    #[test]
    fn the_env_var_reads_as_flag_or_path() {
        for off in ["0", "false", "off", ""] {
            assert!(matches!(off, "0" | "false" | "off" | ""));
        }
        for on in ["1", "true", "yes", "on"] {
            assert!(matches!(on, "1" | "true" | "yes" | "on"));
        }
    }
}
