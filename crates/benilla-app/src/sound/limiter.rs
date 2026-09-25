//! The **output limiter** — the mix's last stage, and the one that stops a busy soundscape from
//! tearing.
//!
//! ## What it replaces
//!
//! Every WoW SFX ships mastered to full scale: `HolyProtection.wav` (the Fortitude buff, the
//! director's own example) peaks at **1.000**, `MetalShieldBlock1.wav` at 1.000,
//! `LightningBoltImpact.wav` at 0.992 — measured off the shipped `sound.MPQ`. benilla's owned
//! gain chain is `category · v · rolloff · near_field`, every factor `<= 1.0`, so a single close
//! kit at `SoundEntries.Volume 1.0` (2 175 of the 4 623 rows) *already sits on 0 dBFS*. There is
//! no headroom anywhere in the chain, and the mix is a plain sum.
//!
//! So two of them is +6 dB past full scale and five is +14, and kira answers that with
//! `frame.clamp(-1.0, 1.0)` (`backend/renderer.rs`) — a squared-off waveform, which is broadband
//! distortion, which is what a speaker breaking sounds like. A priest's Prayer of Fortitude
//! lands `HolyProtection` on five party members inside one frame: five sample-aligned copies of
//! one 0 dBFS file, summing to exactly 5.0. Nothing upstream prevents it — `SoundEntries` id 3116
//! carries `Flags 0x0000`, so the no-duplicate bit (0x20, which 2 776 of the 4 623 rows *do*
//! carry) does not apply to it.
//!
//! ## What it does
//!
//! A **look-ahead brickwall limiter**, stereo-linked, with a mathematically exact guarantee of no
//! overshoot — so nothing ever reaches the renderer's clamp and the clamp stops being audible.
//! Three stages, all fixed-size and allocation-free on the render path:
//!
//! 1. **Required gain.** Per frame, `required = min(1, CEILING / max(|L|, |R|))` — stereo-linked so
//!    a limited transient never shifts the stereo image.
//! 2. **Anticipation.** A sliding *minimum* of `required` over the next [`LOOKAHEAD_MS`], then a
//!    moving *average* of that minimum over the same span. The audio is delayed by one look-ahead
//!    so the gain is already down when the transient arrives. The two windows are what make the
//!    bound exact: every term of the average is a minimum over a window that still contains the
//!    sample being scaled, so the average is `<= required` at that sample — a smooth gain curve
//!    that provably never overshoots. (The naive one-pole attack does overshoot: it approaches
//!    its target asymptotically and lets the first millisecond of a +14 dB transient straight
//!    through, which is the part you actually hear.)
//! 3. **Release.** The gain returns to unity exponentially over [`RELEASE_MS`] instead of snapping
//!    back the instant the peak passes. A limiter whose gain recovers in two milliseconds is
//!    modulating the signal at audio rate, which is its own distortion — worse on bass than the
//!    clipping it replaces.
//!
//! ## Why this and not fidelity
//!
//! This is **below** the decision-0070 seam, not across it: WoW's owned parameter math upstream is
//! untouched, and the thing being replaced is not a reference behaviour but *kira's* hard clamp.
//!
//! **The reference has no headroom mechanism at all — on either side of its API boundary.** That
//! is now read, not assumed: no constant factor exists anywhere in `WoW.exe` (a close-up 1.0 kit
//! reaches `FSOUND_SetVolume(255)` through `0x7a5dc0` with the SFX master also 255), and
//! decision 1563 opened `fmod.dll` itself and found the other half — a complete multiply census
//! over all three software mixers turns up per-channel volume, `1/255`, `1/256`, `2⁻³¹` and a
//! mix-rate-derived ramp constant, and **nothing that depends on how many voices are live**. The
//! FPU mixer sums into a float buffer and clamps once; the MMX mixers (what the reference
//! actually runs) accumulate with **saturating** `paddsw` and their output stage is a bare
//! `movq` copy. So the real client sums at full scale and clips, exactly as we did.
//!
//! Two things it does have are often mistaken for headroom and are neither: the 12-voice device
//! ceiling with the 13 per-bus caps (ported — 1555, 1557), which bounds the *count*; and the SFX
//! auto-duck (`0x457960`) — it is a **sidechain for server-pushed
//! unit voice lines** (`SMSG_PLAY_OBJECT_SOUND`), armed only by that one packet's pool, with the
//! arming sound itself exempt. Footsteps, spell impacts, weapon hits, UI sounds, creature barks,
//! music and ambience never arm it. It is not a bus limiter and would not have saved the mass
//! buff.
//!
//! Modern engines put a limiter here for exactly this reason — FMOD Studio ships one on the
//! master bus by default, and Wwise's own guidance is a peak limiter on the master output.
//! `SoundOutputLimiter 0` turns it off to A/B against what the director reported.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::Frame;

use super::meter::MixLevel;

/// The output ceiling. A hair under full scale so the renderer's own `clamp` is never the thing
/// shaping the waveform — if this ever fires, something upstream of it is broken.
const CEILING: f32 = 0.99;

/// The look-ahead span: both the anticipation window and the added output latency.
///
/// 2 ms is long enough to swallow the attack of a percussive impact (the shortest thing in this
/// game's SFX is a shield clang, whose rise is ~1 ms) and short enough to be nothing at all as
/// latency — nothing in WoW is rhythm-critical, and the device buffer above it is already 43 ms
/// (decision 1026's `TARGET_BUFFER_FRAMES`).
const LOOKAHEAD_MS: f32 = 2.0;

/// How long the gain takes to return to unity once the peak has passed. Long enough that the
/// gain is not modulating at audio rate (which would intermodulate the bass), short enough that
/// a burst of impacts does not leave the world quiet behind it.
const RELEASE_MS: f32 = 120.0;

/// The ceiling, for the seam's offline harness to assert against.
#[cfg(test)]
pub(super) fn ceiling() -> f32 {
    CEILING
}

/// Install the limiter as the main track's last stage. `enabled` is the live `SoundOutputLimiter`
/// CVar cell — read once per block, so flipping it applies without a restart; the delay line runs
/// either way, so the toggle never steps the output.
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

/// The limiter's DSP, free of kira and of the atomics — so the whole guarantee is testable
/// offline (see the tests below, which are the proof that "never overshoots" is a fact and not a
/// hope).
pub(super) struct LimiterCore {
    /// The look-ahead audio delay, `lookahead` frames deep.
    delay: Vec<Frame>,
    delay_w: usize,
    /// Monotonically-increasing `(sample index, required gain)` — the sliding-minimum window.
    /// Never holds more than `lookahead + 1` entries, and its capacity is reserved up front, so
    /// the pushes cannot allocate.
    win: VecDeque<(u64, f32)>,
    /// Index of the next input sample.
    n: u64,
    /// The last `lookahead` sliding-minimum outputs and their running sum — the smoothing average.
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

    /// (Re)size every buffer for `sample_rate`. The only allocating call — kira guarantees it runs
    /// off the render path (`Effect::init` / `on_change_sample_rate`).
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

    /// Consume one input frame, emit one output frame plus the gain applied to it.
    ///
    /// `bypass` steers the *target* to unity rather than short-circuiting the delay: the release
    /// envelope then carries the gain back smoothly, so toggling the CVar mid-sound is a fade and
    /// never a step.
    #[inline]
    pub(super) fn step(&mut self, input: Frame, bypass: bool) -> (Frame, f32) {
        // Stage 1 — this sample's required gain, stereo-linked.
        let peak = input.left.abs().max(input.right.abs());
        let required = if peak > CEILING { CEILING / peak } else { 1.0 };

        // Stage 2a — sliding minimum of `required` over the look-ahead window ending here.
        while self.win.back().is_some_and(|&(_, g)| g >= required) {
            self.win.pop_back();
        }
        self.win.push_back((self.n, required));
        let oldest = self.n.saturating_sub(self.lookahead as u64);
        while self.win.front().is_some_and(|&(i, _)| i < oldest) {
            self.win.pop_front();
        }
        let window_min = self.win.front().map_or(1.0, |&(_, g)| g);

        // Stage 2b — moving average of that minimum over the same span. Every term is a minimum
        // over a window that still contains the sample leaving the delay line, so the average
        // cannot exceed that sample's own required gain. That is the no-overshoot proof.
        self.hist_sum += f64::from(window_min) - f64::from(self.hist[self.hist_w]);
        self.hist[self.hist_w] = window_min;
        self.hist_w = (self.hist_w + 1) % self.lookahead;
        let smoothed = if bypass {
            1.0
        } else {
            (self.hist_sum / self.lookahead as f64) as f32
        };

        // Stage 3 — instant attack (the anticipation already made it gradual), slow release.
        let recovered = 1.0 - (1.0 - self.gain) * self.release;
        self.gain = smoothed.min(recovered);

        // The delay line: emit the frame from one look-ahead ago, scaled by this gain.
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

    /// Run `frames` through and return the output, dropping the priming look-ahead.
    fn run(c: &mut LimiterCore, frames: &[Frame]) -> Vec<Frame> {
        frames.iter().map(|f| c.step(*f, false).0).collect()
    }

    /// A mono impulse train at N× full scale — the mass-buff case, five sample-aligned copies of
    /// one 0 dBFS file. **Nothing leaves the limiter past the ceiling**, at any overload.
    #[test]
    fn never_exceeds_the_ceiling_at_any_overload() {
        for n in [2.0f32, 5.0, 12.0, 40.0] {
            let mut c = core();
            // 200 ms of a loud 440 Hz tone: continuous, so the release never gets to recover and
            // the steady-state bound is exercised, not just the transient.
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

    /// The hard case a one-pole attack fails: silence, then a single full-amplitude sample at
    /// 5× scale with no warning. The look-ahead must already have the gain down.
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

    /// Below the ceiling the limiter is a pure delay: unity gain, sample-for-sample, no colour.
    /// A limiter that touches a quiet mix is a bug, not a safety net.
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

    /// The gain returns to unity after the loud part ends — a limiter that stays down has eaten
    /// the world's volume. [`RELEASE_MS`] is an exponential *time constant*, so the check is
    /// against its arithmetic: 250 ms is ~2.1 τ (the gap should be ~12 % of its start), 1 s is
    /// ~8.3 τ (essentially closed).
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

    /// Stereo linking: a peak on one channel scales both by the same gain, so a limited transient
    /// cannot swing the stereo image.
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

    /// Bypass is a *target* change, not a short circuit: the delay keeps running, so the output
    /// stays sample-aligned and the gain walks back to unity instead of stepping.
    #[test]
    fn bypass_keeps_the_delay_and_fades_rather_than_steps() {
        let mut c = core();
        for _ in 0..RATE / 20 {
            c.step(Frame::from_mono(5.0), false);
        }
        let engaged = c.gain;
        assert!(engaged < 0.3);
        let mut last = engaged;
        // Half a second is ~4.2 release time constants: no single-sample step anywhere along the
        // way, and unity by the end.
        for _ in 0..RATE / 2 {
            let (_, g) = c.step(Frame::from_mono(5.0), true);
            assert!(g - last < 0.01, "stepped from {last} to {g}");
            last = g;
        }
        assert!(last > 0.98, "bypass did not reach unity: {last}");
    }
}
