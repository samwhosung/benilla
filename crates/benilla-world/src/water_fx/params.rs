//! The CWater0Ripple **driver math** — pure functions (no ECS): the per-emission parameter
//! formulas of `0x5fa760`, the record lifecycle (size growth + the 0.4/0.6 alpha ramp), and the
//! render texgen. Every formula is byte-verified and validated against the two reference-trace
//! reconstructions; the tests pin the envelopes.
//!
//! The 0264 INTERIM constants are gone: the wake-size factor was the reference
//! *render* aging a record one frame before its first draw (an artifact of the capture's ~20 fps,
//! not an emit multiplier — negligible at our frame rates), and the ring-lifetime factor was a
//! circular fps estimate in the reconstruction (no such factor exists at the bytes). The cadences
//! are now the byte laws: ring `400 ms + U[0,50)`, wake `~625/min(speed,20)` ms — one decal per
//! ~0.6 yd of travel at any speed.

/// Ring pulse interval (s): `400 + U[0,50)` cooldown ticks (VERIFIED `0x5fac41`–`0x5fac53` +
/// FUN_00455c70's `(50·rng)>>32`; the tick rides the QPC-ms thunk `0x42b790`).
pub(super) const RING_INTERVAL: (f32, f32) = (0.4, 0.45);

/// Wake emission cooldown (s) at `speed` yd/s: `625/min(speed,20)` ms — a **distance law**, one
/// decal per ~0.625 yd of travel (VERIFIED: the driver's `−1000.0` sign-flip makes a future
/// deadline `now + k·625/min(speed,20)`; ≈ 89 ms at run speed). The jitter factor `k`'s spread is
/// the one remaining INTERIM: the traces show ±~15%, the exact distribution wasn't pinned.
pub(super) fn wake_cooldown(speed: f32, rng: &mut u32) -> f32 {
    let k = 0.9 + 0.2 * rand01(rng);
    k * 0.625 / speed.clamp(0.1, 20.0)
}

/// What a unit is doing in the water this frame — the reference's two selection bits
/// (`MOVEMENTFLAGS & 0xf` / `& 0x30`), resolved.
#[derive(Clone, Copy)]
pub(super) enum WadeState {
    Translating { speed: f32, heading: f32 },
    Turning,
    Standing,
}

/// Emission params for one record — the driver's computed values (`0x5fa760`), byte-verified.
pub(super) struct FoamParams {
    pub(crate) size0: f32,
    /// yd/s.
    pub(crate) growth: f32,
    /// s.
    pub(crate) lifetime: f32,
    /// Peak vertex alpha (`min(6 × driverAlpha, 1)` — the `0x68be62` transform, alpha-only).
    pub(crate) peak: f32,
    /// Render category: ring (`splash.blp`) vs wake (`wake.blp`).
    pub(crate) ring: bool,
}

/// The driver's parameter computation. `scale` = `OBJECT_FIELD_SCALE_X`; `gate` = the depth
/// gate `max(2 × collisionHeight, 1.0)` (`[unit+0x297]` = CMovement+0xb4, ≈4.06 yd for a human
/// ); `depth` = surface − feet (yd, > 0 in water). `None` when the depth gate
/// rejects: not in water, or dived deeper than ~2 body heights — surface swimming (rest depth
/// ~0.75·h) stays well inside and emits.
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
        // Turning in place and the step-in one-shot take the unreduced ring params (the driver's
        // standing-only branch skips them; `flagtable[1|3] = 0` keeps them ring-textured).
        _ => {}
    }
    // Depth attenuation: past half the gate depth, everything ramps down linearly toward ×0.5
    // (alpha, lifetime, size0 — never growth; VERIFIED).
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

/// A live record's size at `now`: `size0 + growth · age`. (The reference render integrates one
/// `dt·growth` step *before* a record's first draw — visible at a capture's ~20 fps, negligible
/// at ours; we draw `size0` on the first frame.)
pub(super) fn record_size(size0: f32, growth: f32, born: f32, now: f32) -> f32 {
    size0 + growth * (now - born)
}

/// A live record's alpha at `now`: the 0.4/0.6 rise/decay ramp to `peak` (rate table `0x810348 =
/// [0.4, 0.4]`, both categories), 0 at (and past) the lifetime.
pub(super) fn record_alpha(peak: f32, lifetime: f32, born: f32, now: f32) -> f32 {
    let age = (now - born) / lifetime;
    if age <= 0.4 {
        peak * (age / 0.4).max(0.0)
    } else {
        peak * (1.0 - (age - 0.4) / 0.6).max(0.0)
    }
}

/// The texgen (byte-verified: `uv = Rz(heading − π/2)·(v − center)/(2s) + 0.5`, world-axis
/// aligned, geometry static, growth purely via this box; the fixed −π/2 is absorbed into our
/// heading convention, which was fitted directly to the reference traces). `u` runs across the
/// track, `v` runs *against* the heading — the wake chevron's apex (low `v` in `wake.blp`) lands
/// ahead of the unit, arms trailing. Pinned by the golden test below.
pub(super) fn foam_uv(center: [f32; 2], heading: f32, size: f32, p: [f32; 2]) -> [f32; 2] {
    let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
    let (s, c) = heading.sin_cos();
    let inv = 1.0 / (2.0 * size);
    [
        (-s * dx + c * dy) * inv + 0.5,
        (-c * dx - s * dy) * inv + 0.5,
    ]
}

/// xorshift32 + a 24-bit uniform — the same local-RNG idiom as `particles::rand01` (the reference
/// rolls its own RNG per emission; we mirror the distribution, not the stream).
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

    /// GOLDEN — the texgen signs against the reference trace fits (2026-07-08/10 captures): the
    /// fitted affine `uv = A·p + t` measured `u` ACROSS the track and `v` AGAINST the heading —
    /// the chevron apex (low `v`) lands ahead of the unit, arms trailing; the box `[c−s, c+s]`
    /// spans one UV unit.
    #[test]
    fn texgen_matches_the_reference_fit() {
        let c = [-9016.0_f32, -226.0];
        let h = 0.4637_f32; // ≈ 26.6° — the wade capture's early leg
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

    /// The driver params at the byte formulas' operating points: a running wake (7 yd/s), a
    /// standing ring, and the step-in one-shot.
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

    /// The cadence laws: ring 400–450 ms; wake = the ~0.625-yd distance law (~89 ms at run
    /// speed, capped at speed 20).
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

    /// The depth gate and its attenuation: reject outside (0, radius2), attenuate past half.
    #[test]
    fn depth_gate_and_attenuation() {
        let mut rng = 7u32;
        assert!(foam_params(WadeState::Standing, false, 1.0, 1.0, 1.05, &mut rng).is_none());
        assert!(foam_params(WadeState::Standing, false, 1.0, 1.0, -0.1, &mut rng).is_none());
        // Near the gate depth, k → 0.5: a standing ring's peak → 6 × (0.8/6 × ~0.5) ≈ 0.4.
        let deep = foam_params(WadeState::Standing, false, 1.0, 1.0, 0.99, &mut rng).unwrap();
        assert!((deep.peak - 0.8 * 0.505).abs() < 0.02, "peak {}", deep.peak);
    }

    /// The lifecycle: 0.4/0.6 rise/decay to the peak, zero at the lifetime; linear size growth.
    #[test]
    fn alpha_ramp_and_size_growth() {
        assert!((record_alpha(0.8, 1.0, 0.0, 0.2) - 0.4).abs() < 1e-6);
        assert!((record_alpha(0.8, 1.0, 0.0, 0.4) - 0.8).abs() < 1e-6);
        assert!((record_alpha(0.8, 1.0, 0.0, 0.7) - 0.4).abs() < 1e-6);
        assert!(record_alpha(0.8, 1.0, 0.0, 1.0) < 1e-6);
        assert!((record_size(0.5, 1.0, 0.0, 0.5) - 1.0).abs() < 1e-6);
    }
}
