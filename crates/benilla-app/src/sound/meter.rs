//! The **output meter** — what the mix's level actually *is*, in numbers.
//!
//! The three crackle hunts before this one (1026, 1109, 1112/1114) each ended by adding the meter
//! that would have named the mechanism: the callback-deadline load, the stream-decoder liveness,
//! the HAL's own IO cycle, the recorded waveform. Every one of them watches *timing*. None of
//! them watches **amplitude** — and amplitude is the one thing that goes wrong when a lot of
//! sounds happen at once, which is exactly the condition the director reports ("a lot of mobs
//! attacking same time... it gets really dirty, like a speaker breaking").
//!
//! It cannot be read off any existing counter, because a clipped mix is *healthy* by all of them:
//! the callback met its deadline, no decoder starved, the OS delivered every cycle on time. The
//! mix was computed perfectly — it was just louder than full scale, and kira's renderer answers
//! that with a hard `clamp(-1.0, 1.0)` (`backend/renderer.rs`), which is a squared-off waveform,
//! i.e. broadband distortion. So this meter sits on the main track **ahead of the limiter** and
//! records what the game *asked* for:
//!
//! - **`peak`** — the largest `|sample|` the summed mix reached. `> 1.0` means the request did not
//!   fit; `4.7` means the game asked for 4.7× full scale and (before 1551) got a clipped 1.0.
//! - **`over`** — how many samples were past full scale. One is a tick; a hundred thousand is the
//!   report the director filed.
//! - **`reduction`** — the deepest gain the limiter had to pull to make it fit ([`super::limiter`]
//!   writes it here), so the log line reads as one story: asked for this, allowed that.
//! - **`nonfinite`** — samples that were NaN or infinite. This is the meter's own blind spot,
//!   closed deliberately: `f32::max` *discards* a NaN operand and `NaN > 1.0` is
//!   `false`, so a mix carrying NaN reads as flawless on `peak` and `over` alike — and sails
//!   through the limiter's `peak > CEILING` test untouched, straight into the driver. A single
//!   non-finite sample is broadband noise at whatever the hardware makes of the bit pattern,
//!   which is *also* "a speaker breaking", and no counter we had could see it. It must be tested
//!   for explicitly or not at all.
//!
//! The audio-thread half is three atomics and a compare per sample. The main-thread half drains
//! them once per report window in `sound::poll_mix_health`.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::Frame;

/// The mix's level, shared audio-thread → main-thread.
///
/// Peaks are stored as the **bit pattern** of a non-negative `f32`, which orders identically to
/// the float itself — so `fetch_max`/`fetch_min` on the bits are exactly max/min on the values,
/// with no lock and no CAS loop on the render path.
#[derive(Debug)]
pub(super) struct MixLevel {
    /// Peak `|sample|` of the summed mix since the last [`Self::take`].
    peak_bits: AtomicU32,
    /// Samples past full scale since the last [`Self::take`].
    over: AtomicU64,
    /// The limiter's deepest gain since the last [`Self::take`] (1.0 = it never engaged).
    reduction_bits: AtomicU32,
    /// Non-finite (NaN/inf) samples since the last [`Self::take`]. Never nonzero in a healthy
    /// mix; any nonzero reading is a defect upstream, not a loud passage.
    nonfinite: AtomicU64,
}

impl Default for MixLevel {
    fn default() -> Self {
        Self {
            peak_bits: AtomicU32::new(0),
            over: AtomicU64::new(0),
            reduction_bits: AtomicU32::new(1.0f32.to_bits()),
            nonfinite: AtomicU64::new(0),
        }
    }
}

/// One window's reading, as [`MixLevel::take`] hands it to the reporter.
#[derive(Clone, Copy, Debug)]
pub(super) struct LevelReading {
    /// Peak `|sample|` the summed mix reached. `> 1.0` = the mix did not fit in full scale.
    pub(super) peak: f32,
    /// Samples past full scale in the window.
    pub(super) over: u64,
    /// The limiter's deepest gain in the window (1.0 = never engaged).
    pub(super) reduction: f32,
    /// Non-finite samples in the window. Anything but zero is a bug upstream of the mix.
    pub(super) nonfinite: u64,
}

impl MixLevel {
    /// Fold one processed block's tally in (audio thread) — accumulated in locals first, so the
    /// render path pays atomics per *block*, not per sample.
    #[inline]
    fn block(&self, peak: f32, over: u64, nonfinite: u64) {
        self.peak_bits.fetch_max(peak.to_bits(), Ordering::Relaxed);
        if over > 0 {
            self.over.fetch_add(over, Ordering::Relaxed);
        }
        if nonfinite > 0 {
            self.nonfinite.fetch_add(nonfinite, Ordering::Relaxed);
        }
    }

    /// Fold the limiter's applied gain in (audio thread — [`super::limiter`]).
    #[inline]
    pub(super) fn gain(&self, gain: f32) {
        self.reduction_bits
            .fetch_min(gain.to_bits(), Ordering::Relaxed);
    }

    /// Read and reset — one window's story (main thread).
    pub(super) fn take(&self) -> LevelReading {
        LevelReading {
            peak: f32::from_bits(self.peak_bits.swap(0, Ordering::Relaxed)),
            over: self.over.swap(0, Ordering::Relaxed),
            reduction: f32::from_bits(
                self.reduction_bits
                    .swap(1.0f32.to_bits(), Ordering::Relaxed),
            ),
            nonfinite: self.nonfinite.swap(0, Ordering::Relaxed),
        }
    }
}

/// Install the meter on `builder` and return the cell it feeds. Always on: it is three atomics on
/// the render path, and the alternative is what the last four crackle hunts each had to start
/// from — the director's ear and no number.
pub(super) fn install(builder: &mut kira::track::MainTrackBuilder, level: &Arc<MixLevel>) {
    builder.add_effect(MeterBuilder {
        level: Arc::clone(level),
    });
}

struct MeterBuilder {
    level: Arc<MixLevel>,
}

impl EffectBuilder for MeterBuilder {
    type Handle = ();
    fn build(self) -> (Box<dyn Effect>, Self::Handle) {
        (Box::new(Meter { level: self.level }), ())
    }
}

/// The audio-thread half: measure, never modify.
struct Meter {
    level: Arc<MixLevel>,
}

impl Effect for Meter {
    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info) {
        let mut peak = 0.0f32;
        let mut over = 0u64;
        let mut nonfinite = 0u64;
        for f in input.iter() {
            for mag in [f.left.abs(), f.right.abs()] {
                // `is_finite` first: a NaN would otherwise vanish into `max` and compare `false`
                // against the over-scale test, i.e. register as a perfectly healthy sample.
                if mag.is_finite() {
                    peak = peak.max(mag);
                    over += u64::from(mag > 1.0);
                } else {
                    nonfinite += 1;
                }
            }
        }
        self.level.block(peak, over, nonfinite);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The float-bits ordering trick the lock-free peak/min rests on: for non-negative floats,
    /// `to_bits` is monotonic, so atomic max/min on the bits are max/min on the values.
    #[test]
    fn float_bits_order_like_the_floats_for_non_negatives() {
        let mut vals = [0.0f32, 1e-30, 0.5, 0.999, 1.0, 1.0001, 4.7, 1e30];
        for w in vals.windows(2) {
            assert!(w[0].to_bits() < w[1].to_bits(), "{} < {}", w[0], w[1]);
        }
        vals.reverse();
        let level = MixLevel::default();
        for v in vals {
            level.block(v, 0, 0);
        }
        assert_eq!(level.take().peak, 1e30);
    }

    /// A window's reading is the window's, not all time: `take` resets peak and count, and
    /// restores the "limiter never engaged" identity for the reduction.
    #[test]
    fn take_resets_the_window() {
        let level = MixLevel::default();
        level.block(0.5, 0, 0);
        level.block(2.0, 2, 0);
        level.block(1.5, 0, 0);
        level.gain(0.25);
        let r = level.take();
        assert_eq!(r.peak, 2.0);
        assert_eq!(r.over, 2); // the one block that carried over-scale samples
        assert_eq!(r.reduction, 0.25);
        let r = level.take();
        assert_eq!(r.peak, 0.0);
        assert_eq!(r.over, 0);
        assert_eq!(r.reduction, 1.0);
    }

    /// The blind spot this meter exists to not have. A NaN sample is invisible to both amplitude
    /// tests — `f32::max` returns the *other* operand and `NaN > 1.0` is `false` — so a mix full
    /// of NaN would otherwise report peak 0.0, zero over-scale, and perfect health while the
    /// driver plays noise. Only an explicit finiteness test sees it.
    #[test]
    fn non_finite_samples_are_counted_not_silently_swallowed() {
        // The trap, stated as an executable fact rather than a comment. `black_box` keeps these
        // runtime comparisons — a literal NaN comparison is (rightly) a lint, and the point here
        // is exactly that this is what the audio thread would have been doing.
        let nan = std::hint::black_box(f32::NAN);
        assert_eq!(0.0f32.max(nan), 0.0, "`max` discards a NaN operand");
        assert!(
            nan.partial_cmp(&1.0).is_none(),
            "a NaN is not 'over full scale' — it is unordered, so the test never fires"
        );

        let mut meter = Meter {
            level: Arc::new(MixLevel::default()),
        };
        let level = Arc::clone(&meter.level);
        let mut block = [
            Frame::from_mono(f32::NAN),
            Frame::from_mono(0.5),
            Frame::from_mono(f32::INFINITY),
        ];
        meter.process(
            &mut block,
            1.0 / 44_100.0,
            &kira::info::MockInfoBuilder::new().build(),
        );
        let r = level.take();
        assert_eq!(
            r.nonfinite, 4,
            "two non-finite frames, stereo — four samples"
        );
        assert_eq!(r.peak, 0.5, "the finite sample still sets the peak");
        assert_eq!(r.over, 0, "and a NaN is not over-scale, it is broken");
    }
}
