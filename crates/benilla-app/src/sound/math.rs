//! WoW's owned audio math — behavioral ports of the reference's own functions (decision 0070
//! "the audible fingerprint").
//!
//! Ports are **behavioral**, not bit-exact: the originals reproduce x87 PC_53 double-rounding
//! spill-for-spill; here the formulas run in plain f32/f64 (differences are below audibility and
//! below the FMOD 255-level quantum the original fed). Each item cites its RE anchor. Two
//! deliberate drops from the original pipeline, both quantization shims for FMOD's integer API:
//! the mantissa-bit volume curve (`0x7a5d70`, `(v·255+512.5).bits >> 14 & 0xff` — a ≤0.4%
//! nonlinearity that only existed to make a 0..255 level) and the `__ftol` truncations. Our
//! backend takes float amplitudes, so the linear value they quantized is fed directly.

/// Per-shot volume variation — `0x458c60` `variation_volume`. With a PRNG `draw` (kit variation
/// enabled): `v = (draw − 15)·0.01 + base`; without: `v = base`. Scaled by `mult`, clamped `[0,1]`
/// (NaN → 0, matching the binary's unordered branch).
pub(crate) fn variation_volume(draw: Option<i32>, base: f32, mult: f32) -> f32 {
    let v = match draw {
        None => base as f64,
        Some(r) => (r - 0xf) as f64 * 0.01 + base as f64,
    };
    let v = v * mult as f64;
    // `!(v > 0.0)` is deliberate, NOT `v <= 0.0`: NaN must land in the zero arm (the binary's
    // unordered `fcom` branch).
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if !(v > 0.0) {
        0.0
    } else if v >= 1.0 {
        1.0
    } else {
        v as f32
    }
}

/// Per-shot pitch variation — `0x458da0`: the playback frequency in Hz handed
/// to `FSOUND_SetFrequency` when the kit varies pitch: `freq = (22050 · (draw + 0x55)) / 100`
/// (32-bit wrapping mul, truncating division — exactly the binary's integer path). The backend
/// consumes a *rate*, so callers divide by the file's sample rate: a 22050 Hz WAV at draw 15
/// plays at rate 1.0; the absolute-Hz semantics (a 44.1 kHz file would halve) are FMOD's and are
/// preserved deliberately.
pub(crate) fn variation_pitch_freq(draw: i32) -> i32 {
    22050i32.wrapping_mul(draw.wrapping_add(0x55)) / 100
}

/// The variation draw: raw PRNG word → `{0..30}` via **multiply-high scale-to-range**
/// (`mulhi(31, raw)` — `mov ecx,0x1f` @ `0x458d35`/`0x458e85` through the `0x455c70` mul-high
/// helper). NOT a modulo: `floor(31·raw / 2³²)`, uniform
/// over 31 values given a uniform word. Volume spans ±0.15 about base, pitch ×0.85..×1.15.
pub(crate) fn variation_draw(raw: u32) -> i32 {
    ((u64::from(raw) * 31) >> 32) as i32
}

/// Squared listener↔source distance (`0x45ce16` idiom — all gates compare squared).
pub(crate) fn dist_sq(a: bevy::math::Vec3, b: bevy::math::Vec3) -> f32 {
    a.distance_squared(b)
}

/// Selection-time audibility — `0x45cdf0`: play iff `maxdist² > d²`
/// (strict; equal/NaN inaudible). `maxdist` is the kit's `DistanceCutoff`; `0` means
/// non-positional (callers skip the gate — the `0x7a5ca0` sentinel).
pub(crate) fn audible(d_sq: f32, maxdist: f32) -> bool {
    maxdist * maxdist > d_sq
}

/// The near-field attenuation WoW layers ON TOP of the backend's rolloff — `0x7a5000`, the
/// per-frame value at channel `+0x78`:
/// `atten = 1 − clamp(√d² − maxdist·0.9, 0, maxdist·0.1) / (maxdist·0.1)` — full volume inside
/// 90% of `maxdist`, a linear 1→0 ramp across the last 10%. Callers virtualize the channel
/// beyond `maxdist` instead of calling this.
pub(crate) fn near_field_atten(d_sq: f32, maxdist: f32) -> f32 {
    let band = maxdist * 0.1;
    if band <= 0.0 {
        return 1.0;
    }
    let over = d_sq.sqrt() - maxdist * 0.9;
    let clamped = over.clamp(0.0, band);
    1.0 - clamped / band
}

// NOTE: `0x457960` is deliberately NOT ported here. benilla once modelled it as a music
// "transition crossfade"; it is the SFX-bus auto-duck (the SoundVolume category dips to 50 %
// while SFX one-shots play, restoring over 3 s), unrelated to music transitions, which benilla
// does not model.
// The real music transition is a 4.0 s backend fade-stop (see `sound::zone`).

/// The global 3D rolloff factor — `FSOUND_3D_SetRolloffFactor(4.0)` at device init
/// (`0x7a47b1`, prefill `0x7a495a` = 4.0f; `SoundRolloffFactor` CVar default "4" round-trips it).
pub(crate) const ROLLOFF_FACTOR: f32 = 4.0;

/// The distance rolloff FMOD computed between the kit's `MinDistance` and the far plane —
/// FMOD 3.x's logarithmic/inverse model with the global rolloff factor:
/// full volume inside `min_dist`, `min_dist / (min_dist + factor·(d − min_dist))` beyond
/// (WoW handed FMOD `Sample_SetMinMaxDistance(min_dist, 100000.0)` — `0x458ed0` — and
/// [`ROLLOFF_FACTOR`] at init). We disabled the backend's attenuation (gain is
/// ours), so this model moves to our side of the seam; the audible total is
/// `rolloff · near_field_atten`.
pub(crate) fn fmod_rolloff(d_sq: f32, min_dist: f32) -> f32 {
    if min_dist <= 0.0 {
        return 1.0;
    }
    let d = d_sq.sqrt();
    if d <= min_dist {
        1.0
    } else {
        min_dist / (min_dist + ROLLOFF_FACTOR * (d - min_dist))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variation_volume_matches_the_re_formula() {
        // draw 15 is the identity draw: (15−15)·0.01 + base = base.
        assert_eq!(variation_volume(Some(15), 0.8, 1.0), 0.8);
        // Extremes of the 0..=30 domain: ±0.15 around base.
        assert!((variation_volume(Some(30), 0.8, 1.0) - 0.95).abs() < 1e-6);
        assert!((variation_volume(Some(0), 0.8, 1.0) - 0.65).abs() < 1e-6);
        // Clamps: over 1 → 1, mult 0 → 0, no-draw passthrough.
        assert_eq!(variation_volume(Some(30), 0.95, 1.0), 1.0);
        assert_eq!(variation_volume(Some(15), 0.8, 0.0), 0.0);
        assert_eq!(variation_volume(None, 0.5, 1.0), 0.5);
    }

    #[test]
    fn variation_pitch_matches_the_re_integer_math() {
        assert_eq!(variation_pitch_freq(15), 22050); // identity draw
        assert_eq!(variation_pitch_freq(0), 18742); // 22050·85/100, truncated
        assert_eq!(variation_pitch_freq(30), 25357); // 22050·115/100, truncated
    }

    #[test]
    fn near_field_ramp_covers_the_last_ten_percent() {
        let md = 100.0;
        assert_eq!(near_field_atten(0.0, md), 1.0);
        assert_eq!(near_field_atten(80.0 * 80.0, md), 1.0); // inside the near field
        assert_eq!(near_field_atten(90.0 * 90.0, md), 1.0); // ramp start boundary
        assert!((near_field_atten(95.0 * 95.0, md) - 0.5).abs() < 1e-5); // mid-band
        assert!(near_field_atten(100.0 * 100.0, md).abs() < 1e-5); // maxdist → 0
    }

    #[test]
    fn audibility_gate_is_strict() {
        assert!(audible(99.9 * 99.9, 100.0));
        assert!(!audible(100.0 * 100.0, 100.0)); // equal = inaudible (the binary's fcompp)
    }

    #[test]
    fn rolloff_is_inverse_with_factor_four_beyond_min_distance() {
        assert_eq!(fmod_rolloff(4.0 * 4.0, 8.0), 1.0); // inside min_dist: full volume
        assert_eq!(fmod_rolloff(8.0 * 8.0, 8.0), 1.0); // boundary
                                                       // Double the distance with factor 4: 8/(8 + 4·8) = 0.2 (was 0.5 at factor 1).
        assert!((fmod_rolloff(16.0 * 16.0, 8.0) - 0.2).abs() < 1e-6);
        assert_eq!(fmod_rolloff(100.0, 0.0), 1.0); // 0 = non-positional sentinel
    }

    #[test]
    fn variation_draw_is_the_mulhi_scale() {
        assert_eq!(variation_draw(0), 0);
        assert_eq!(variation_draw(u32::MAX), 30); // floor(31·(2³²−1)/2³²) = 30
                                                  // The mulhi differs from a modulo: raw = 2³¹ → floor(31/2) = 15 (identity draw).
        assert_eq!(variation_draw(1 << 31), 15);
        for raw in [0u32, 1, 0x1234_5678, 0xdead_beef, u32::MAX] {
            let r = variation_draw(raw);
            assert!((0..=30).contains(&r));
        }
    }
}
