//! The output limiter, the mix's last stage.
//!
//! Deviation: the reference sums at full scale and clips (a close 1.0 kit reaches
//! `FSOUND_SetVolume(255)` through `0x7a5dc0`, FMOD's MMX mixers saturate with `paddsw`, and no
//! gain depends on the live voice count); this limits, because every WoW SFX is mastered to full
//! scale, so overlapping kits clip into broadband distortion, as when a mass buff lands five
//! sample-aligned copies of one 0 dBFS file. `SoundOutputLimiter 0` turns it off. The reference's
//! voice and bus caps bound the count, not the level, and its SFX auto-duck (`0x457960`) is armed
//! only by server-pushed voice lines.
//!
//! A look-ahead brickwall limiter, stereo-linked and allocation-free on the render path:
//!
//! 1. Required gain per frame: `min(1, CEILING / max(|L|, |R|))`.
//! 2. Anticipation: a sliding minimum of the required gain over [`LOOKAHEAD_MS`], then a moving
//!    average of that minimum over the same span, with the audio delayed by one look-ahead. Every
//!    term of the average is a minimum over a window holding the sample being scaled, so the gain
//!    never overshoots.
//! 3. Release: the gain returns to unity exponentially over [`RELEASE_MS`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::Frame;

use super::meter::MixLevel;

/// The output ceiling, a hair under full scale so the renderer's `clamp` never shapes the waveform.
const CEILING: f32 = 0.99;

/// The look-ahead span and the added latency: longer than a shield clang's ~1 ms rise, and small
/// beside the 43 ms device buffer.
const LOOKAHEAD_MS: f32 = 2.0;

/// The release time constant: slow enough not to modulate the bass at audio rate, fast enough
/// that a burst does not leave the world quiet.
const RELEASE_MS: f32 = 120.0;

/// The ceiling, for the offline harness.
#[cfg(test)]
pub(super) fn ceiling() -> f32 {
    CEILING
}

/// Installs the limiter as the main track's last stage. `enabled` is the live `SoundOutputLimiter`
/// cell, read once per block; the delay line runs either way, so the toggle never steps.
pub(super) fn install(
    builder: &mut kira::track::MainTrackBuilder,
    level: &Arc<MixLevel>,
    enabled: &Arc<AtomicBool>,
) {
    builder.add_effect(LimiterBuilder {
        level: Arc::clone(level),
        enabled: Arc::clone(enabled),
    });
}

struct LimiterBuilder {
    level: Arc<MixLevel>,
    enabled: Arc<AtomicBool>,
}

impl EffectBuilder for LimiterBuilder {
    type Handle = ();
    fn build(self) -> (Box<dyn Effect>, Self::Handle) {
        (
            Box::new(Limiter {
                level: self.level,
                enabled: self.enabled,
                core: LimiterCore::new(),
            }),
            (),
        )
    }
}

struct Limiter {
    level: Arc<MixLevel>,
    enabled: Arc<AtomicBool>,
    core: LimiterCore,
}

impl Effect for Limiter {
    fn init(&mut self, sample_rate: u32, _internal_buffer_size: usize) {
        self.core.resize(sample_rate);
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.core.resize(sample_rate);
    }

    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info) {
        let bypass = !self.enabled.load(Ordering::Relaxed);
        let mut deepest = 1.0f32;
        for f in input.iter_mut() {
            let (out, gain) = self.core.step(*f, bypass);
            *f = out;
            deepest = deepest.min(gain);
        }
        self.level.gain(deepest);
    }
}

/// The limiter's DSP, free of kira and the atomics so the guarantee tests offline.
pub(super) struct LimiterCore {
    /// The look-ahead audio delay, `lookahead` frames deep.
    delay: Vec<Frame>,
    delay_w: usize,
    /// The sliding-minimum window of `(sample index, required gain)`, at most `lookahead + 1`
    /// entries in reserved capacity, so pushes never allocate.
    win: VecDeque<(u64, f32)>,
    /// Index of the next input sample.
    n: u64,
    /// The last `lookahead` sliding minimums and their running sum: the smoothing average.
    hist: Vec<f32>,
    hist_w: usize,
    hist_sum: f64,
    /// The gain actually applied to the sample leaving the delay line.
    gain: f32,
    /// Per-sample exponential approach to unity.
    release: f32,
    lookahead: usize,
}

impl LimiterCore {
    pub(super) fn new() -> Self {
        Self {
            delay: Vec::new(),
            delay_w: 0,
            win: VecDeque::new(),
            n: 0,
            hist: Vec::new(),
            hist_w: 0,
            hist_sum: 0.0,
            gain: 1.0,
            release: 0.0,
            lookahead: 0,
        }
    }

    /// Sizes every buffer for `sample_rate`: the only allocating call, run off the render path
    /// (`Effect::init`, `on_change_sample_rate`).
    pub(super) fn resize(&mut self, sample_rate: u32) {
        let rate = sample_rate.max(1) as f32;
        self.lookahead = ((LOOKAHEAD_MS / 1000.0 * rate).round() as usize).max(1);
        self.delay.clear();
        self.delay.resize(self.lookahead, Frame::ZERO);
        self.delay_w = 0;
        self.win.clear();
        self.win.reserve(self.lookahead + 2);
        self.n = 0;
        self.hist.clear();
        self.hist.resize(self.lookahead, 1.0);
        self.hist_w = 0;
        self.hist_sum = self.lookahead as f64;
        self.gain = 1.0;
        // Exponential approach to unity with a time constant of RELEASE_MS.
        self.release = (-1000.0 / (RELEASE_MS * rate)).exp();
    }

    /// One frame in, one frame out with its gain. `bypass` steers the target to unity rather than
    /// skipping the delay, so a toggle mid-sound fades instead of stepping.
    #[inline]
    pub(super) fn step(&mut self, input: Frame, bypass: bool) -> (Frame, f32) {
        // Stage 1: this sample's required gain, stereo-linked.
        let peak = input.left.abs().max(input.right.abs());
        let required = if peak > CEILING { CEILING / peak } else { 1.0 };

        // Stage 2a: sliding minimum of `required` over the look-ahead window ending here.
        while self.win.back().is_some_and(|&(_, g)| g >= required) {
            self.win.pop_back();
        }
        self.win.push_back((self.n, required));
        let oldest = self.n.saturating_sub(self.lookahead as u64);
        while self.win.front().is_some_and(|&(i, _)| i < oldest) {
            self.win.pop_front();
        }
        let window_min = self.win.front().map_or(1.0, |&(_, g)| g);

        // Stage 2b: moving average of that minimum. Every term is a minimum over a window holding
        // the sample leaving the delay line, so the average never exceeds its required gain.
        self.hist_sum += f64::from(window_min) - f64::from(self.hist[self.hist_w]);
        self.hist[self.hist_w] = window_min;
        self.hist_w = (self.hist_w + 1) % self.lookahead;
        let smoothed = if bypass {
            1.0
        } else {
            (self.hist_sum / self.lookahead as f64) as f32
        };

        // Stage 3: instant attack (the anticipation made it gradual), slow release.
        let recovered = 1.0 - (1.0 - self.gain) * self.release;
        self.gain = smoothed.min(recovered);

        // Emit the frame from one look-ahead ago, scaled by this gain.
        let out = self.delay[self.delay_w] * self.gain;
        self.delay[self.delay_w] = input;
        self.delay_w = (self.delay_w + 1) % self.lookahead;
        self.n += 1;
        (out, self.gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn core() -> LimiterCore {
        let mut c = LimiterCore::new();
        c.resize(RATE);
        c
    }

    /// Runs `frames` through and returns the output, look-ahead priming included.
    fn run(c: &mut LimiterCore, frames: &[Frame]) -> Vec<Frame> {
        frames.iter().map(|f| c.step(*f, false).0).collect()
    }

    /// A tone at N× full scale (five is the mass-buff case) never leaves past the ceiling.
    #[test]
    fn never_exceeds_the_ceiling_at_any_overload() {
        for n in [2.0f32, 5.0, 12.0, 40.0] {
            let mut c = core();
            // 200 ms of a continuous 440 Hz tone, so the steady-state bound is exercised too.
            let frames: Vec<Frame> = (0..RATE / 5)
                .map(|i| {
                    let s = n * (i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin();
                    Frame::from_mono(s)
                })
                .collect();
            let out = run(&mut c, &frames);
            let peak = out
                .iter()
                .map(|f| f.left.abs().max(f.right.abs()))
                .fold(0.0f32, f32::max);
            assert!(
                peak <= CEILING + 1e-5,
                "{n}x overload leaked {peak} past the {CEILING} ceiling"
            );
        }
    }

    /// Silence, then a lone sample at 5× scale: the look-ahead has the gain down already.
    #[test]
    fn a_bare_transient_out_of_silence_does_not_leak() {
        let mut c = core();
        let mut frames = vec![Frame::ZERO; 500];
        frames[300] = Frame::from_mono(5.0);
        frames[301] = Frame::from_mono(-5.0);
        let out = run(&mut c, &frames);
        let peak = out
            .iter()
            .map(|f| f.left.abs().max(f.right.abs()))
            .fold(0.0f32, f32::max);
        assert!(peak <= CEILING + 1e-5, "transient leaked at {peak}");
    }

    /// Below the ceiling the limiter is a pure delay, sample for sample.
    #[test]
    fn quiet_material_passes_through_untouched() {
        let mut c = core();
        let frames: Vec<Frame> = (0..2000)
            .map(|i| {
                Frame::from_mono(
                    0.5 * (i as f32 * 220.0 * std::f32::consts::TAU / RATE as f32).sin(),
                )
            })
            .collect();
        let out = run(&mut c, &frames);
        let lookahead = c.lookahead;
        for (i, f) in out.iter().enumerate().skip(lookahead) {
            let want = frames[i - lookahead].left;
            assert!(
                (f.left - want).abs() < 1e-6,
                "sample {i}: {} != {want}",
                f.left
            );
        }
    }

    /// The gain returns to unity after the burst: 250 ms is ~2.1 τ of [`RELEASE_MS`] (the gap near
    /// 12 % of its start), 1 s is ~8.3 τ.
    #[test]
    fn the_gain_recovers_after_the_burst() {
        let mut c = core();
        let loud: Vec<Frame> = (0..RATE / 20).map(|_| Frame::from_mono(5.0)).collect();
        run(&mut c, &loud);
        assert!(c.gain < 0.3, "did not engage: {}", c.gain);
        let engaged = c.gain;
        run(&mut c, &vec![Frame::ZERO; (RATE / 4) as usize]);
        let after_250ms = c.gain;
        assert!(
            after_250ms > 1.0 - (1.0 - engaged) * 0.2,
            "recovery is slower than its own time constant: {after_250ms}"
        );
        run(&mut c, &vec![Frame::ZERO; (RATE * 3 / 4) as usize]);
        assert!(c.gain > 0.999, "did not recover: {}", c.gain);
    }

    /// A peak on one channel scales both by the same gain.
    #[test]
    fn limiting_is_stereo_linked() {
        let mut c = core();
        let frames: Vec<Frame> = (0..2000)
            .map(|_| Frame {
                left: 4.0,
                right: 1.0,
            })
            .collect();
        let out = run(&mut c, &frames);
        for f in out.iter().skip(c.lookahead + 200) {
            assert!(
                (f.left / f.right - 4.0).abs() < 1e-3,
                "image moved: {} / {}",
                f.left,
                f.right
            );
        }
    }

    /// Bypass keeps the delay running and walks the gain back to unity without a step.
    #[test]
    fn bypass_keeps_the_delay_and_fades_rather_than_steps() {
        let mut c = core();
        for _ in 0..RATE / 20 {
            c.step(Frame::from_mono(5.0), false);
        }
        let engaged = c.gain;
        assert!(engaged < 0.3);
        let mut last = engaged;
        // Half a second is ~4.2 release time constants: no step on the way, unity at the end.
        for _ in 0..RATE / 2 {
            let (_, g) = c.step(Frame::from_mono(5.0), true);
            assert!(g - last < 0.01, "stepped from {last} to {g}");
            last = g;
        }
        assert!(last > 0.98, "bypass did not reach unity: {last}");
    }
}
