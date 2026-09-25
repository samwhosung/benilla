//! Drunkenness, from the drunk byte (`PLAYER_BYTES_3` byte 1). For the active player while moving
//! (`flags & 0xf`, callers `0x60a984`–`0x60ab14`), the reference adds a wobble each frame to the
//! facing, 2π-wrapped through `0x60de30`, unless a keyboard turn is held (`flags & 0x30`, not
//! mouselook), and four times the wobble to the swim pitch, clamped, while swimming (`0x200000`).
//! Its amplitude and frequency both scale with the fraction: ±0.75° a frame at full drunk.
//! The binary computes on the x87 stack at 53-bit precision and rounds to f32 only at its stores,
//! as the port does in f64, with the binary's constants bit for bit.

/// 0.01 at `0x8029d0`: the drunk byte's fraction scale (`0x5e2a90`).
const DRUNK_SCALE: f32 = f32::from_bits(0x3c23_d70a);
/// π/180 at `0x7ffaac`: the phase's degrees-to-radians factor.
const DEG2RAD: f32 = f32::from_bits(0x3c8e_fa35);
/// The three harmonic frequencies, at `0x80c5e0`, `0x80c5dc` and `0x80c5d8`.
const PULSE_F1: f32 = f32::from_bits(0x3dbe_76c9); // 0.093
const PULSE_F2: f32 = f32::from_bits(0x3e1d_b22d); // 0.154
const PULSE_F3: f32 = f32::from_bits(0x3e47_ae14); // 0.195
/// The mean's 1/3 at `0x80c5d4`, 12 ULPs below the nearest f32 to 1/3.
const PULSE_MEAN: f32 = f32::from_bits(0x3eaa_aa9f);
/// π/240 at `0x80c4bc`: the amplitude per unit of fraction.
const PULSE_AMP: f32 = f32::from_bits(0x3c56_7750);
/// 4.0 at `0x80306c`: the wobble's swim-pitch multiplier (`0x60aaf1`).
pub(super) const SWIM_PITCH_WOBBLE_SCALE: f32 = 4.0;

/// The drunk fraction, `min(byte, 100) × 0.01` (`0x5e2a90`).
pub(super) fn fraction(byte: u8) -> f32 {
    let clamped = byte.min(100);
    (f64::from(clamped) * f64::from(DRUNK_SCALE)) as f32
}

/// The frame's wobble in radians for the time in ms and the drunk fraction (`0x60ab20`).
pub(super) fn wobble(now_ms: u32, fraction: f32) -> f32 {
    if fraction == 0.0 {
        return 0.0;
    }
    // `fild qword` of the zero-extended time: exact in f64.
    let p = f64::from(now_ms) * f64::from(DEG2RAD) * f64::from(fraction);
    // `fst [ebp-4]` rounds the phase to f32 for the second and third cosines only.
    let p_f32 = f64::from(p as f32);
    let c1 = (p * f64::from(PULSE_F1)).cos();
    let c2 = (p_f32 * f64::from(PULSE_F2)).cos();
    let c3 = (p_f32 * f64::from(PULSE_F3)).cos();
    let mean = ((c1 + c2) + c3) * f64::from(PULSE_MEAN);
    (mean * (f64::from(PULSE_AMP) * f64::from(fraction))) as f32
}

// Deviation: no drunk field-of-view lane, because the reference's is invisible in play: its
// target `(179° − 90°)·fraction` (camera `+0x10c`, setter `0x511250`) eases at
// `cameraFoVSmoothSpeed`, 0.5°/s, and restarts on every drunk-byte change, so it creeps under 1°.
// Not built: entering the world already drunk, the reference snaps the full fisheye (`0x50d0f0`).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_scales_and_clamps() {
        assert_eq!(fraction(0), 0.0);
        assert_eq!(fraction(50), 0.5);
        assert_eq!(fraction(100), fraction(255));
        assert!((fraction(100) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn wobble_zero_when_sober() {
        assert_eq!(wobble(123_456, 0.0), 0.0);
    }

    #[test]
    fn wobble_phase_zero_is_full_amplitude() {
        // At 0 ms every cosine is 1, so the pulse is the amplitude times the fraction.
        let full = wobble(0, 1.0);
        assert!((full - PULSE_AMP).abs() < 1e-7, "got {full}");
        let half = wobble(0, 0.5);
        assert!((half - PULSE_AMP * 0.5).abs() < 1e-7, "got {half}");
    }

    #[test]
    fn wobble_stays_within_amplitude_and_oscillates() {
        // A minute at full drunk: never past the amplitude, and both signs visited.
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
