//! `$WOW_MIX_TAP=<path.wav>` — record the final mix to disk.
//!
//! The crackle hunts (1026, 1109) kept hitting the same wall: every scheduling meter reads
//! healthy while the director's ear still catches a crackle — because the meters watch the
//! *pipeline* (deadlines, decoder liveness, stream position) and a crackle can live purely in
//! the *waveform* (a clipped sum, a stepped parameter, a discontinuity none of our counters
//! model). This closes the gap at the root: a [`kira::effect::Effect`] on the **main track**
//! copies every processed frame into a lock-free ring, and a writer thread drains it to a
//! standard WAV — so a reported crackle becomes data to scan, timestamp, and classify instead
//! of a sound to argue about.
//!
//! Two properties matter:
//! - **What is actually heard.** The tap is the main track's *last* effect, downstream of the
//!   meter and the limiter, so the capture is the audible result — the waveform
//!   to scan when a report survives everything upstream. The mix's raw, pre-limiter level is the
//!   [`super::meter`]'s job and lands in the health report as a number; the tap no longer has to
//!   double as the clipping detector it once was.
//! - **Crash-safe file.** The writer patches the RIFF/data sizes on every flush, so the WAV is
//!   valid up to the last second even if the app exits hard; there is no finalize step to miss.
//!
//! The audio-thread half never allocates or blocks (a fixed ring push per sample; overflow is
//! counted, not waited on). The ring holds [`RING_SECONDS`] of stereo audio — the writer wakes
//! every [`FLUSH_EVERY`] and would have to stall ~50× past its cadence before a sample drops.
//!
//! Read a capture back with `scripts/mixsum.py <path.wav>`: what the mix was doing, when, in
//! numbers (clipping, level, silence, steps).

use bevy::prelude::*;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::Frame;
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Ring depth in seconds of stereo audio. Deep enough that only a wedged writer drops samples.
const RING_SECONDS: usize = 8;
/// Writer wake cadence. Also the upper bound on audio lost at a hard kill.
const FLUSH_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// Install the tap on `main_track_builder` if `$WOW_MIX_TAP` names a path. Returns the builder
/// unchanged otherwise. `sample_rate` is the device rate the mixer negotiated — the WAV header
/// must match what the renderer actually produces, so an unprobeable device (`None`) skips the
/// tap rather than record on a guessed time axis.
pub(super) fn install(
    builder: kira::track::MainTrackBuilder,
    sample_rate: Option<u32>,
) -> kira::track::MainTrackBuilder {
    let mut builder = builder;
    if let Some(path) = std::env::var_os("WOW_MIX_TAP") {
        if let Some(rate) = sample_rate {
            install_at(&mut builder, std::path::Path::new(&path), rate);
        } else {
            warn!("mix tap: device sample rate unknown — not recording");
        }
    }
    builder
}

/// Install a tap writing `path`, and hand back its **frame clock**: a running count of the frames
/// this tap has processed, i.e. the exact write position in the file it is producing.
///
/// The clock is what makes a capture *correlatable*. A WAV on its own answers "what did the mix
/// sound like"; it cannot answer "…at the moment the director pressed the key", because the game
/// clock and the device clock are different clocks that drift. Reading this counter from a game
/// system converts a game-thread event into a sample offset in the file, so a mark, a kit start
/// and a missed deadline all land on the waveform they belong to.
///
/// Returns `None` if the file cannot be created — a probe that cannot record says so and the run
/// continues, rather than taking the client down over an instrument.
pub(super) fn install_at(
    builder: &mut kira::track::MainTrackBuilder,
    path: &std::path::Path,
    sample_rate: u32,
) -> Option<Arc<AtomicU64>> {
    let file = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            warn!("mix tap: cannot create {}: {e}", path.display());
            return None;
        }
    };
    let (producer, consumer) = rtrb::RingBuffer::new(sample_rate as usize * 2 * RING_SECONDS);
    std::thread::Builder::new()
        .name("mix-tap".into())
        .spawn(move || writer(file, consumer, sample_rate))
        .expect("spawn mix-tap writer");
    info!("mix tap: recording to {}", path.display());
    let frames = Arc::new(AtomicU64::new(0));
    builder.add_effect(TapBuilder {
        producer,
        frames: Arc::clone(&frames),
    });
    Some(frames)
}

struct TapBuilder {
    producer: rtrb::Producer<f32>,
    frames: Arc<AtomicU64>,
}

impl EffectBuilder for TapBuilder {
    type Handle = ();
    fn build(self) -> (Box<dyn Effect>, Self::Handle) {
        (
            Box::new(Tap {
                producer: self.producer,
                frames: self.frames,
                dropped: 0,
            }),
            (),
        )
    }
}

/// The audio-thread half: copy each frame into the ring. Push-and-count on overflow — the one
/// thing this must never do is block or allocate on the render path.
struct Tap {
    producer: rtrb::Producer<f32>,
    /// Frames pushed so far — the file's write position, published for the game thread to read.
    frames: Arc<AtomicU64>,
    dropped: u64,
}

impl Effect for Tap {
    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info) {
        for f in input.iter() {
            if self.producer.push(f.left).is_err() || self.producer.push(f.right).is_err() {
                self.dropped += 1;
            }
        }
        // Published after the push so a reader never names a frame the file does not yet hold.
        self.frames.fetch_add(input.len() as u64, Ordering::Relaxed);
    }
}

/// The writer half: drain the ring to the WAV, patching the header sizes each flush so the file
/// on disk is always valid. Ends (and logs the tally) when the producer side is dropped — i.e.
/// when the mixer itself is torn down at app exit.
fn writer(mut file: std::fs::File, mut consumer: rtrb::Consumer<f32>, sample_rate: u32) {
    if let Err(e) = file.write_all(&wav_header(sample_rate, 0)) {
        warn!("mix tap: header write failed: {e}");
        return;
    }
    let mut samples_written: u64 = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    loop {
        let done = consumer.is_abandoned();
        buf.clear();
        while let Ok(s) = consumer.pop() {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        if !buf.is_empty() {
            if let Err(e) = file.write_all(&buf) {
                warn!("mix tap: write failed: {e}");
                return;
            }
            samples_written += (buf.len() / 4) as u64;
            // Patch the sizes so the file is valid as-is (crash-safe: no finalize step).
            let header = wav_header(sample_rate, samples_written);
            if file.seek(SeekFrom::Start(0)).is_ok() {
                let _ = file.write_all(&header);
                let _ = file.seek(SeekFrom::End(0));
            }
        }
        if done {
            info!(
                "mix tap: closed — {:.1}s of mix recorded",
                samples_written as f64 / 2.0 / f64::from(sample_rate)
            );
            return;
        }
        std::thread::sleep(FLUSH_EVERY);
    }
}

/// A 44-byte WAV header for stereo IEEE-float-32 at `sample_rate`, sized for `samples` samples
/// (total across both channels). Rewritten in place on every flush.
fn wav_header(sample_rate: u32, samples: u64) -> [u8; 44] {
    let data_bytes = (samples * 4) as u32;
    let byte_rate = sample_rate * 2 * 4;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&3u16.to_le_bytes()); // IEEE float
    h[22..24].copy_from_slice(&2u16.to_le_bytes()); // stereo
    h[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    h[32..34].copy_from_slice(&8u16.to_le_bytes()); // block align: 2 ch × 4 B
    h[34..36].copy_from_slice(&32u16.to_le_bytes()); // bits per sample
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header is a valid stereo float-32 WAV that a strict parser accepts, and the patched
    /// sizes match the sample count — the crash-safety of the tap rests on every flush leaving
    /// a well-formed file.
    #[test]
    fn wav_header_is_well_formed_and_patchable() {
        let h = wav_header(48_000, 96_000); // 1 s of stereo
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(h[4..8].try_into().unwrap()),
            36 + 384_000
        );
        assert_eq!(&h[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes(h[20..22].try_into().unwrap()), 3);
        assert_eq!(u16::from_le_bytes(h[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 48_000);
        assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 384_000);
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 384_000);

        // kira's own symphonia decoder accepts the produced file — the exact consumer a
        // captured tap goes back through when we analyze it.
        let mut file = Vec::new();
        file.extend_from_slice(&wav_header(48_000, 4));
        for s in [0.5f32, -0.5, 0.25, -0.25] {
            file.extend_from_slice(&s.to_le_bytes());
        }
        let sound =
            kira::sound::static_sound::StaticSoundData::from_cursor(std::io::Cursor::new(file))
                .expect("tap WAV decodes");
        assert_eq!(sound.frames.len(), 2);
        assert!((sound.frames[0].left - 0.5).abs() < 1e-6);
        assert!((sound.frames[1].right - -0.25).abs() < 1e-6);
    }
}
