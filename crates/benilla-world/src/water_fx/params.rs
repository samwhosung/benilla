//! The `CWater0Ripple` driver math as pure functions: `0x5fa760`'s per-emission parameters, the
//! record lifecycle and the render texgen.

/// Ring pulse interval in seconds: `400 + U[0, 50)` ms (`0x5fac41`..`0x5fac53`, the roll
/// `(50·rng) >> 32` at `0x455c70`, millisecond ticks from `0x42b790`).
pub(super) const RING_INTERVAL: (f32, f32) = (0.4, 0.45);

/// Wake cooldown in seconds at `speed` yd/s: `k·625/min(speed, 20)` ms, one decal per 0.625 yd of
/// travel. `k` is uniform in [0.9, 1.1); the reference's spread is about ±15%, its distribution
/// untraced.
pub(super) fn wake_cooldown(speed: f32, rng: &mut u32) -> f32 {
    let k = 0.9 + 0.2 * rand01(rng);
    k * 0.625 / speed.clamp(0.1, 20.0)
}

/// A unit's motion in the water, from the selection bits `MOVEMENTFLAGS & 0xf` and `& 0x30`.
#[derive(Clone, Copy)]
pub(super) enum WadeState {
    Translating { speed: f32, heading: f32 },
    Turning,
    Standing,
}

/// One record's emission parameters, as the driver computes them (`0x5fa760`).
pub(super) struct FoamParams {
    pub(crate) size0: f32,
    /// yd/s.
    pub(crate) growth: f32,
    /// s.
    pub(crate) lifetime: f32,
    /// Peak vertex alpha, `min(6 × driver alpha, 1)` (`0x68be62`).
    pub(crate) peak: f32,
    /// Render category: ring (`splash.blp`) vs wake (`wake.blp`).
    pub(crate) ring: bool,
}

/// The driver's parameters for one emission: `scale` is `OBJECT_FIELD_SCALE_X`, `gate` the
/// emission depth gate and `depth` the surface minus the feet, in yd.
pub(super) fn foam_params(
    state: WadeState,
    oneshot: bool,
    scale: f32,
    gate: f32,
    depth: f32,
    rng: &mut u32,
) -> Option<FoamParams> {
    if depth <= 0.0 || depth >= gate {
        return None;
    }
    let mut uni = |a: f32, b: f32| a + (b - a) * rand01(rng);
    let mut size0 = (scale * (1.0 / 3.0) * uni(0.9, 1.1)).clamp(1.0 / 3.0, 5.0 / 3.0);
    let mut lifetime = uni(0.6, 0.7);
    let mut growth = uni(1.0, 1.5);
    let mut alpha = 1.0 / 6.0;
    let ring = oneshot || !matches!(state, WadeState::Translating { .. });
    match (oneshot, state) {
        (false, WadeState::Translating { speed, .. }) => {
            growth *= speed.min(20.0) / 2.5;
        }
        (false, WadeState::Standing) => {
            alpha *= 0.8;
            growth *= 0.25;
            size0 *= 0.6;
        }
        // Turning and the one-shot take the unreduced ring params: the driver's standing branch
        // skips them, and `flagtable[1|3] = 0` keeps them ring-textured.
        _ => {}
    }
    // Past half the gate depth, alpha, lifetime and size0 (never growth) ramp down toward ×0.5.
    let half = gate * 0.5;
    if depth > half {
        let k = 0.5 + 0.5 * (gate - depth) / half;
        alpha *= k;
        lifetime *= k;
        size0 *= k;
    }
    Some(FoamParams {
        size0,
        growth,
        lifetime,
        peak: (6.0 * alpha).min(1.0),
        ring,
    })
}

/// A record's size at `now`. Deviation: the first draw is `size0`, where the reference first ages
/// the record one `dt·growth` step, a difference too small to see at our frame rates.
pub(super) fn record_size(size0: f32, growth: f32, born: f32, now: f32) -> f32 {
    size0 + growth * (now - born)
}

/// A record's alpha at `now`: up to `peak` over the first 0.4 of its life and down to 0 over the
/// rest (rate table `0x810348`, both categories).
pub(super) fn record_alpha(peak: f32, lifetime: f32, born: f32, now: f32) -> f32 {
    let age = (now - born) / lifetime;
    if age <= 0.4 {
        peak * (age / 0.4).max(0.0)
    } else {
        peak * (1.0 - (age - 0.4) / 0.6).max(0.0)
    }
}

/// The texgen `uv = Rz(heading − π/2)·(p − center)/(2s) + 0.5`, with the −π/2 folded into the
/// heading: `u` runs across the track and `v` against the heading, so the wake chevron's apex
/// (low `v` in `wake.blp`) lands ahead of the unit.
pub(super) fn foam_uv(center: [f32; 2], heading: f32, size: f32, p: [f32; 2]) -> [f32; 2] {
    let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
    let (s, c) = heading.sin_cos();
    let inv = 1.0 / (2.0 * size);
    [
        (-s * dx + c * dy) * inv + 0.5,
        (-c * dx - s * dy) * inv + 0.5,
    ]
}

/// xorshift32: the reference's RNG differs, so this matches its distribution, not its stream.
fn next_u32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

pub(super) fn rand01(state: &mut u32) -> f32 {
    (next_u32(state) >> 8) as f32 / (1u32 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signs of the affine texgen fitted to the reference's frames: `u` across the track, `v`
    /// against the heading, and the box `[c−s, c+s]` one UV unit.
    #[test]
    fn texgen_matches_the_reference_fit() {
        let c = [-9016.0_f32, -226.0];
        let h = 0.4637_f32; // about 26.6°, a heading from the reference capture
        let s = 1.25_f32;
        let (sh, ch) = h.sin_cos();
        let uv_a = foam_uv(c, h, s, [c[0] + ch, c[1] + sh]); // 1 yd ahead
        let uv_b = foam_uv(c, h, s, [c[0] - ch, c[1] - sh]); // 1 yd behind
        let uv_l = foam_uv(c, h, s, [c[0] - sh, c[1] + ch]); // 1 yd across
        assert!((uv_a[0] - 0.5).abs() < 1e-4 && uv_a[1] < 0.5, "{uv_a:?}");
        assert!(uv_b[1] > 0.5, "{uv_b:?}");
        assert!(
            (uv_l[1] - 0.5).abs() < 1e-4 && (uv_l[0] - 0.5).abs() > 0.1,
            "{uv_l:?}"
        );
        let edge = foam_uv(c, h, s, [c[0] + ch * s, c[1] + sh * s]);
        assert!(edge[1].abs() < 1e-4, "box edge → v = 0: {edge:?}");
    }

    #[test]
    fn params_match_the_verified_formulas() {
        let mut rng = 1u32;
        for _ in 0..64 {
            let translating = WadeState::Translating {
                speed: 7.0,
                heading: 0.0,
            };
            let p = foam_params(translating, false, 1.0, 1.0, 0.3, &mut rng).unwrap();
            assert!(!p.ring);
            assert!((0.3..=0.37).contains(&p.size0), "wake size0 {}", p.size0);
            assert!((2.8..=4.2).contains(&p.growth), "wake growth {}", p.growth);
            assert!((0.6..0.7).contains(&p.lifetime));
            assert!((p.peak - 1.0).abs() < 1e-6);

            let p = foam_params(WadeState::Standing, false, 1.0, 1.0, 0.3, &mut rng).unwrap();
            assert!(p.ring);
            assert!((0.16..=0.23).contains(&p.size0), "ring size0 {}", p.size0);
            assert!(
                (0.25..=0.375).contains(&p.growth),
                "ring growth {}",
                p.growth
            );
            assert!((0.6..0.7).contains(&p.lifetime), "ring life {}", p.lifetime);
            assert!((p.peak - 0.8).abs() < 1e-6);

            let p = foam_params(translating, true, 1.0, 1.0, 0.3, &mut rng).unwrap();
            assert!(p.ring, "a one-shot is ring-category");
            assert!(
                (0.3..=0.377).contains(&p.size0),
                "one-shot size0 {}",
                p.size0
            );
            assert!(
                (1.0..1.5).contains(&p.growth),
                "one-shot growth unscaled {}",
                p.growth
            );
            assert!((p.peak - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn cadence_laws() {
        let mut rng = 3u32;
        for _ in 0..32 {
            let w = wake_cooldown(7.0, &mut rng);
            assert!((0.080..=0.103).contains(&w), "wake@7 {w}");
            let w = wake_cooldown(50.0, &mut rng);
            assert!((0.028..=0.036).contains(&w), "wake@cap {w}");
        }
        assert!(RING_INTERVAL.0 >= 0.4 && RING_INTERVAL.1 <= 0.45);
    }

    #[test]
    fn depth_gate_and_attenuation() {
        let mut rng = 7u32;
        assert!(foam_params(WadeState::Standing, false, 1.0, 1.0, 1.05, &mut rng).is_none());
        assert!(foam_params(WadeState::Standing, false, 1.0, 1.0, -0.1, &mut rng).is_none());
        // Near the gate depth, k → 0.5: a standing ring's peak → 6 × (0.8/6 × ~0.5) ≈ 0.4.
        let deep = foam_params(WadeState::Standing, false, 1.0, 1.0, 0.99, &mut rng).unwrap();
        assert!((deep.peak - 0.8 * 0.505).abs() < 0.02, "peak {}", deep.peak);
    }

    #[test]
    fn alpha_ramp_and_size_growth() {
        assert!((record_alpha(0.8, 1.0, 0.0, 0.2) - 0.4).abs() < 1e-6);
        assert!((record_alpha(0.8, 1.0, 0.0, 0.4) - 0.8).abs() < 1e-6);
        assert!((record_alpha(0.8, 1.0, 0.0, 0.7) - 0.4).abs() < 1e-6);
        assert!(record_alpha(0.8, 1.0, 0.0, 1.0) < 1e-6);
        assert!((record_size(0.5, 1.0, 0.0, 0.5) - 1.0).abs() < 1e-6);
    }
}
