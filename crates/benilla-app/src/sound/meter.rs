//! The output meter: the summed mix's level, measured on the main track ahead of the limiter.
//!
//! A clipped mix meets every timing deadline, and kira answers it with a hard
//! `clamp(-1.0, 1.0)`, which is broadband distortion; so this records what the game asked for:
//!
//! - `peak`: the largest `|sample|`; above 1.0 the mix did not fit full scale.
//! - `over`: how many samples were past full scale.
//! - `reduction`: the deepest gain [`super::limiter`] pulled.
//! - `nonfinite`: NaN or infinite samples, which `peak` and `over` cannot see (`f32::max` drops a
//!   NaN and `NaN > 1.0` is false) and which pass the limiter untouched.
//!
//! The audio thread pays three atomics per block; `sound::poll_mix_health` drains them per report
//! window.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::Frame;

/// The mix's level, shared from the audio thread to the main thread. Peaks are stored as the bit
/// pattern of a non-negative `f32`, which orders like the float, so `fetch_max`/`fetch_min` on
/// the bits need no lock.
#[derive(Debug)]
pub(super) struct MixLevel {
    /// Peak `|sample|` of the summed mix since the last [`Self::take`].
    peak_bits: AtomicU32,
    /// Samples past full scale since the last [`Self::take`].
    over: AtomicU64,
    /// The limiter's deepest gain since the last [`Self::take`] (1.0 = it never engaged).
    reduction_bits: AtomicU32,
    /// Non-finite samples since the last [`Self::take`]; any is a defect upstream.
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
    pub(super) peak: f32,
    pub(super) over: u64,
    pub(super) reduction: f32,
    pub(super) nonfinite: u64,
}

impl MixLevel {
    /// Fold one processed block's tally in, on the audio thread.
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

    /// Fold the limiter's applied gain in, on the audio thread.
    #[inline]
    pub(super) fn gain(&self, gain: f32) {
        self.reduction_bits
            .fetch_min(gain.to_bits(), Ordering::Relaxed);
    }

    /// Read and reset one window, on the main thread.
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

/// Install the meter on `builder`, feeding `level`. Always on.
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

/// The audio-thread half: it measures and never modifies.
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
                // `is_finite` first: a NaN vanishes into `max` and fails the over-scale test.
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

    /// For non-negative floats `to_bits` is monotonic, which the lock-free peak and min rest on.
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

    /// `take` resets the peak and count, and the reduction to 1.0.
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

    /// A NaN sample is invisible to both amplitude tests; only the finiteness test counts it.
    #[test]
    fn non_finite_samples_are_counted_not_silently_swallowed() {
        // `black_box` keeps these runtime comparisons; a literal NaN comparison is a lint.
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
