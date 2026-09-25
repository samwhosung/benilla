//! Sound measuring mode, `$WOW_SOUND_PROBE`: records a session so a sound report can be read
//! from numbers. `scripts/soundprobe.sh` starts the client in it; `scripts/soundprobe.py` reads
//! the capture back.
//!
//! Faults that sound alike through a speaker, and their signatures:
//!
//! | mechanism | signature in a probe capture |
//! |---|---|
//! | over-scale sum | `pre.wav` past ±1.0, `post.wav` clean, `gain` well under 1 |
//! | limiter not engaging | `pre.wav` and `post.wav` both past ±1.0 |
//! | underrun / missed deadline | both WAVs clean at the mark; `load >= 1.0`, `overruns` rising |
//! | starved stream decoder | both WAVs clean; a hard step to zero in `post.wav` at the mark |
//! | non-finite samples | `nan > 0`, invisible to every other counter ([`super::meter`]) |
//! | voice-steal / refusal clicks | `refused` climbing, a discontinuity at the mark |
//! | the limiter's own pumping | `post.wav` clean but `gain` diving repeatedly |
//!
//! A run writes three files to `benilla-config/sound-probe/`, off the game thread: `pre.wav` (the
//! mix ahead of the limiter), `post.wav` (after it) and `timeline.jsonl` (a row every [`TICK`] with
//! level, health and voice counts, plus an event per kit start, marker and refusal).
//!
//! Every row carries `a`, the pre-tap's frame index ([`super::mix_tap::install_at`]), so an event
//! is a sample position in `pre.wav`, free of drift between the game and device clocks. F9 stamps a
//! marker; it is bound only while probing.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bevy::prelude::*;

use super::SoundOutput;

/// The tick-row interval: fine enough to see a burst within one cast, ~12 000 rows in ten minutes.
const TICK: std::time::Duration = std::time::Duration::from_millis(50);

/// The console summary interval, the normal health cadence.
const SPEAK: std::time::Duration = std::time::Duration::from_secs(5);

/// The marker key, bound only while probing.
const MARK_KEY: KeyCode = KeyCode::F9;

/// The probe's output directory from `$WOW_SOUND_PROBE`, created: `1`/`true`/`yes`/`on` is the
/// [`crate::local_state`] default, anything else a path. Called before the mixer is built, since
/// the taps are main-track effects and a kira main track is fixed at build.
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

/// The game-thread half of a probing run, in [`SoundOutput`] so any sound system can stamp events.
pub(crate) struct Probe {
    dir: PathBuf,
    rate: u32,
    /// Lines to the writer thread: a disk write on the game thread would stall the frame, one of
    /// the faults being measured. crossbeam, because the sender must be `Sync`.
    tx: crossbeam_channel::Sender<String>,
    /// The pre-tap's frame clock, the shared time axis.
    audio_pos: Option<Arc<AtomicU64>>,
    started: std::time::Instant,
    marks: u32,
    since_tick: std::time::Duration,
    since_speak: std::time::Duration,
    /// Maxima since the last console summary; the meters drain every tick.
    window: Window,
    /// Cumulative counters at the last tick, to report deltas.
    last_overruns: u64,
    last_refused: u64,
    last_errors: u64,
    last_stolen: u64,
    last_denied: u64,
    last_copies: u64,
}

/// The accumulators behind one console summary.
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
    /// Begins recording. With no pre-tap (`audio_pos` `None`) the timeline has the game clock only.
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

    /// Frames the pre-tap has written: the sample offset every row is keyed on.
    fn audio_frame(&self) -> u64 {
        self.audio_pos
            .as_ref()
            .map_or(0, |p| p.load(Ordering::Relaxed))
    }

    fn send(&self, line: String) {
        // A full/closed channel means the writer died; the run must not.
        let _ = self.tx.send(line);
    }

    /// Stamps an event; `extra` is JSON object members with a leading comma.
    fn event(&self, ev: &str, extra: &str) {
        self.send(format!(
            r#"{{"t":{:.3},"a":{},"ev":"{ev}"{extra}}}"#,
            self.started.elapsed().as_secs_f64(),
            self.audio_frame(),
        ));
    }

    /// Notes a kit starting, from the kit player's play path, so the timeline says what made the
    /// noise.
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

/// Drains lines to disk, flushing each batch so a hard kill loses at most one.
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

/// The probe's health pump: a row per [`TICK`], a console summary per [`SPEAK`]. It owns the
/// meters while it runs: [`super::meter::MixLevel::take`] resets on read, so
/// [`super::poll_mix_health`] stands down.
pub(super) fn tick(
    mut out: NonSendMut<SoundOutput>,
    time: Res<Time>,
    mut exit: MessageReader<bevy::app::AppExit>,
) {
    let exiting = exit.read().next().is_some();
    // Before the split borrow. The voice ceiling bounds everything the device mixes, streams
    // included, so that count is recorded, not the kit channels.
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

    // Fold into the console window.
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

/// One window's console line, live during the session.
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

/// Stamps a marker on F9.
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

/// Registered always; every system returns at once when `$WOW_SOUND_PROBE` is unset.
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, (tick, marker));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quote or backslash in a kit name must not break the row's JSON.
    #[test]
    fn kit_names_stay_valid_json() {
        assert_eq!(esc("HolyProtection"), "HolyProtection");
        assert_eq!(esc(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(esc(r"back\slash"), r"back\\slash");
        assert_eq!(esc("tab\there"), "tab here");
    }

    /// `$WOW_SOUND_PROBE` reads as a flag or a path, and the off forms are off.
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
