//! Drunkenness — the client-side inebriation math (B210).
//!
//! The server streams a drunk value in `PLAYER_BYTES_3` byte 1
//! ([`benilla_protocol::messages::ObjectFields::player_drunk_byte`]); the reference client turns
//! it into a fraction `min(byte, 100) × 0.01` (`0x5e2a90`) and, per frame for the **active
//! player while moving** (`flags & 0xf`, the caller band `0x60a984–0x60ab14`), staggers the walk
//! by a wobble angle:
//!
//! - **facing** `+= pulse(now, fraction)` every frame, 2π-wrapped, committed through the normal
//!   facing pipeline (`0x60de30`) — the sign of the pulse oscillates slowly, so the character
//!   veers left and right as it walks. Skipped while a keyboard turn is held (`flags & 0x30`,
//!   non-mouselook) so deliberate turning stays crisp.
//! - **swim pitch** `+= pulse × 4.0` (`_DAT_0080306c`), clamped, while the swim flag is up
//!   (`0x200000`) — a drunk swimmer porpoises.
//!
//! The pulse itself (`0x60ab20`) is a 3-harmonic cosine blend whose **amplitude and frequencies
//! both scale with the
//! fraction**: `p = now_ms·(π/180)·f`, `pulse = mean(cos(p·0.093), cos(p·0.154), cos(p·0.195))
//! · (π/240)·f`. At full drunk (fraction 1.0) that is a veer rate peaking near ±0.75°/frame with
//! beat periods of a few seconds; tipsy values scale both the swing and how fast it meanders.
//! The three harmonics never phase-lock, so the stagger reads as aimless, not metronomic.
//!
//! Faithful-arithmetic note: the binary computes on the x87 stack at PC_53 (f64) and rounds to
//! f32 only at explicit stores; the port widens each f32 operand to f64 and rounds back only
//! where the binary stores (the mid-pulse `fst` of the phase is reproduced — the first cosine
//! sees the unrounded phase, the second and third the f32-rounded one). Constants carry the
//! binary's exact bit patterns.

/// `_DAT_008029d0` — 0.01 (bits `0x3c23d70a`): the inebriation byte → fraction scale (`0x5e2a90`).
const DRUNK_SCALE: f32 = f32::from_bits(0x3c23_d70a);
/// `_DAT_007ffaac` — π/180 (bits `0x3c8efa35`): the pulse phase's degrees→radians factor.
const DEG2RAD: f32 = f32::from_bits(0x3c8e_fa35);
/// The three harmonic frequency scales of the pulse (`_DAT_0080c5e0/dc/d8`).
const PULSE_F1: f32 = f32::from_bits(0x3dbe_76c9); // 0.093
/// See [`PULSE_F1`].
const PULSE_F2: f32 = f32::from_bits(0x3e1d_b22d); // 0.154
/// See [`PULSE_F1`].
const PULSE_F3: f32 = f32::from_bits(0x3e47_ae14); // 0.195
/// `_DAT_0080c5d4` — 1/3 (bits `0x3eaaaa9f`): the pulse mean. (Not exactly 1.0/3.0 — the
/// binary's constant is one ULP low, and this is that exact bit pattern.)
const PULSE_MEAN: f32 = f32::from_bits(0x3eaa_aa9f);
/// `_DAT_0080c4bc` — π/240 (bits `0x3c567750`): the pulse amplitude-vs-fraction scale.
const PULSE_AMP: f32 = f32::from_bits(0x3c56_7750);
/// `_DAT_0080306c` — 4.0: the swim-pitch multiplier on the wobble (`0x60aaf1`).
pub(super) const SWIM_PITCH_WOBBLE_SCALE: f32 = 4.0;

/// The drunk fraction the reference stores for a raw inebriation byte: `min(byte, 100) × 0.01`
/// (`0x5e2a90`: `cmp al,0x64; jb; mov al,0x64` then `fild; fmul 0.01; fstp dword`).
pub(super) fn fraction(byte: u8) -> f32 {
    let clamped = byte.min(100);
    (f64::from(clamped) * f64::from(DRUNK_SCALE)) as f32
}

/// The per-frame wobble angle (radians): the reference's 3-harmonic cosine pulse `0x60ab20`
/// with `n` = the frame's time in milliseconds and `t` = the drunk fraction. Port of the
/// reference's pulse — see the module doc for the formula and the x87 rounding subtlety.
pub(super) fn wobble(now_ms: u32, fraction: f32) -> f32 {
    if fraction == 0.0 {
        return 0.0;
    }
    // `fild qword` of the zero-extended 32-bit time — exact in f64.
    let p = f64::from(now_ms) * f64::from(DEG2RAD) * f64::from(fraction);
    // The binary's `fst [ebp-4]` rounds the phase to f32 for the 2nd/3rd cosines while the 1st
    // consumes the unrounded st0.
    let p_f32 = f64::from(p as f32);
    let c1 = (p * f64::from(PULSE_F1)).cos();
    let c2 = (p_f32 * f64::from(PULSE_F2)).cos();
    let c3 = (p_f32 * f64::from(PULSE_F3)).cos();
    let mean = ((c1 + c2) + c3) * f64::from(PULSE_MEAN);
    (mean * (f64::from(PULSE_AMP) * f64::from(fraction))) as f32
}

// The binary also holds a drunk FOV lane (camera channel `+0x10c`, setter `0x511250`, target
// `(179° − 90°)·fraction` into the projection), which benilla
// briefly shipped as a fisheye. The director's ref observation — the real client shows NO fov
// change at any drunk value in normal play — held: the lane's ease rate is the CVar
// `cameraFoVSmoothSpeed` (default 0.5°/s → a 178 s full swing whose zero-slope cosine onset
// restarts on every drunk-byte tick, creeping <1° before each reset). Effectively invisible in
// ordinary play, so benilla renders none (decision 1018). The one visible corner — entering
// world already drunk SNAPs the full fisheye instantly (camera-acquisition path, `0x50d0f0`) —
// is deliberately unbuilt.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_scales_and_clamps() {
        assert_eq!(fraction(0), 0.0);
        assert_eq!(fraction(50), 0.5);
        // 100 and everything above clamp to the same full-drunk fraction.
        assert_eq!(fraction(100), fraction(255));
        assert!((fraction(100) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn wobble_zero_when_sober() {
        assert_eq!(wobble(123_456, 0.0), 0.0);
    }

    #[test]
    fn wobble_phase_zero_is_full_amplitude() {
        // At n = 0 every cosine is 1, so the pulse is exactly mean(1,1,1)·amp·t = amp·t — the
        // amplitude anchor: ±(π/240)·fraction radians (≈0.75°/frame at full drunk).
        let full = wobble(0, 1.0);
        assert!((full - PULSE_AMP).abs() < 1e-7, "got {full}");
        let half = wobble(0, 0.5);
        assert!((half - PULSE_AMP * 0.5).abs() < 1e-7, "got {half}");
    }

    #[test]
    fn wobble_stays_within_amplitude_and_oscillates() {
        // Sweep a minute of frames at full drunk: never beyond the amplitude, and both signs
        // visited (the harmonics must actually oscillate).
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for ms in (0..60_000).step_by(16) {
            let w = wobble(ms, 1.0);
            assert!(w.abs() <= PULSE_AMP * 1.0001, "|{w}| > amp at {ms}ms");
            lo = lo.min(w);
            hi = hi.max(w);
        }
        assert!(
            hi > PULSE_AMP * 0.5 && lo < -PULSE_AMP * 0.5,
            "range [{lo}, {hi}]"
        );
    }
}
