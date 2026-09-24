//! The **emission shape kernel** and its RNG — where one birth's position + velocity direction
//! come from: the reference's three vtable type-spawn generators (`0x7b8890`/`0x7b8d70`/
//! `0x7b9500`). Split from the emitter module face (`particles.rs`) purely along
//! this concern; the laws and cites live on [`emit_local`] itself.

use benilla_formats::ParticleEmitterDef;
use bevy::prelude::*;

/// xorshift32 — a dependency-free PRNG for particle jitter (visual only; determinism not required).
pub(super) fn next_u32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// A uniform random `f32` in `[0, 1)`.
pub(crate) fn rand01(state: &mut u32) -> f32 {
    (next_u32(state) >> 8) as f32 / (1u32 << 24) as f32
}

/// A symmetric random `f32` in `(−1, 1)` — the reference's `S11` draw (its RNG builds ±spans with
/// a ×2.0 constant, `0x7b88dd`; every emission distribution below is plain uniform — we mirror
/// the distributions, not the bit stream).
pub(super) fn rand_s11(state: &mut u32) -> f32 {
    rand01(state) * 2.0 - 1.0
}

/// The **emission shape kernel** (the three vtable type-spawn generators below): one birth's
/// position + unit velocity direction, in the emitter's
/// local WoW frame (Z up, origin at the emitter record's `position`). The caller applies the speed
/// roll and the space-mode transform.
///
/// - **Plane** (`0x7b8890`): position uniform in the ±½·area rectangle — **local x takes
///   `areaLength`, local y takes `areaWidth`** (`p = (r2·A_x·0.5, r1·A_y·0.5, 0)` with
///   `A_x = rt+0x290 = EmissionAreaLength`,
///   `A_y = rt+0x294 = EmissionAreaWidth`). The pairing is only observable on an **anisotropic**
///   rectangle, which is why it stayed wrong through 0563/0566 — every emitter checked until
///   Gressil's blade smoke authored a square area. Direction: a cone around +Z with **symmetric**
///   angles θ = S11·verticalRange, φ = S11·horizontalRange (the reference draws ±range, not
///   [0, range] — a one-sided cone tilted every flame the same way).
/// - **Sphere** (`0x7b8d70`): radius uniform in [areaLength, areaWidth] (= min/max radius for this
///   shape), position on the shell at latitude S11·verticalRange / longitude S11·horizontalRange;
///   direction radial outward (flag `0x4000` ⇒ straight +Z instead).
/// - **Spline** (`0x7b9500`): born ON the authored arc-length-parameterized Bézier chain at a
///   uniform arc fraction in `[tMin, tMax]`; velocity = +Z spun about the local tangent by
///   ψ = S11·spin (zero velocity when no spin; radial-from-pivot when zSource authored), plus
///   an optional along-velocity scatter jitter — see the arm's comment.
/// - Any shape with `zSource ≠ 0`: direction is **radial from the pivot** `(0, 0, zSource)` toward
///   the birth point (fountains that arc outward from a point below the basin).
pub(super) fn emit_local(
    def: &ParticleEmitterDef,
    now: &benilla_formats::ParamsNow,
    rng: &mut u32,
) -> (Vec3, Vec3) {
    let origin = Vec3::from(def.position);
    // The emitter-frame R(+Z, 90°) applied at every branch's return — see the law note at the
    // tail. The prepend is per-EMITTER and subclass-independent (gated only by has-emitters,
    // `0x71907f`), so the spline kernel takes it exactly like sphere/plane (the
    // 0563 landing missed this branch — caught by the compensation audit).
    let rot90 = |v: Vec3| Vec3::new(-v.y, v.x, v.z);
    // SPLINE (`0x7b9500`, incl. the scatter bytes): born ON the authored Bézier chain at arc
    // fraction `t ∈ [tMin, tMax]` (the
    // repurposed area fields). Velocity: radial from the zSource pivot when authored; else +Z
    // rotated about the local spline tangent by ψ = S11·spin (the reference extracts the
    // transposed row — a −ψ rotation — indistinguishable under the symmetric draw), with an
    // optional `U01·scatter` position jitter along the velocity; else ZERO (the particle sits
    // on the curve and only gravity/drag move it). A degenerate record (no parsed chain)
    // falls through to the plane kernel, as before.
    if let (benilla_formats::ParticleShape::Spline, Some(spline)) = (def.shape, &def.spline) {
        let (t0, t1) = (
            now.area_length.clamp(0.0, 1.0),
            now.area_width.clamp(0.0, 1.0),
        );
        let t = t0 + rand01(rng) * (t1 - t0);
        let mut pos = origin + Vec3::from(spline.eval(t));
        let dir = if now.z_source != 0.0 {
            (pos - origin - Vec3::new(0.0, 0.0, now.z_source)).normalize_or(Vec3::Z)
        } else if now.vertical_range != 0.0 {
            let tangent = Vec3::from(spline.tangent(t)).normalize_or(Vec3::Z);
            let (s, c) = (rand_s11(rng) * now.vertical_range).sin_cos();
            let dir = Vec3::Z * c + tangent.cross(Vec3::Z) * s + tangent * (tangent.z * (1.0 - c));
            if now.horizontal_range != 0.0 {
                pos += rand01(rng) * now.horizontal_range * dir;
            }
            dir
        } else {
            Vec3::ZERO
        };
        return (origin + rot90(pos - origin), rot90(dir));
    }
    // Birth offset in the emitter frame (before the record-position translation). A sphere
    // draws ONE lat/lon unit vector serving both the shell point and (below) the radial
    // velocity — the reference reuses the exact sincos pair (`0x7b8fba`), which is what keeps a
    // ZERO-radius sphere (the fireball impact's plume burst: min = max = 0) spraying uniformly
    // instead of collapsing every direction to the degenerate normalize fallback.
    let (local, shell) = if def.shape == benilla_formats::ParticleShape::Sphere {
        let r = now.area_length + rand01(rng) * (now.area_width - now.area_length).max(0.0);
        let lat = rand_s11(rng) * now.vertical_range;
        let lon = rand_s11(rng) * now.horizontal_range;
        let (slat, clat) = lat.sin_cos();
        let (slon, clon) = lon.sin_cos();
        let shell = Vec3::new(clat * clon, clat * slon, slat); // unit by construction
        (r * shell, Some(shell))
    } else {
        // x ← areaLength (rt+0x290), y ← areaWidth (rt+0x294) — the VERIFIED pairing, see the
        // Plane bullet above. Both draws are iid S11, so which one feeds which axis changes the
        // rectangle's ORIENTATION but not its distribution; that is the whole bug.
        (
            Vec3::new(
                rand_s11(rng) * 0.5 * now.area_length,
                rand_s11(rng) * 0.5 * now.area_width,
                0.0,
            ),
            None,
        )
    };
    let dir = if now.z_source != 0.0 {
        // Radial from the (0, 0, zSource) pivot — degenerate at the pivot itself falls back to +Z.
        (local - Vec3::new(0.0, 0.0, now.z_source)).normalize_or(Vec3::Z)
    } else if let Some(shell) = shell {
        if def.sphere_up() {
            Vec3::Z
        } else {
            shell // radial outward — the same unit draw as the birth point
        }
    } else {
        let theta = rand_s11(rng) * now.vertical_range;
        let phi = rand_s11(rng) * now.horizontal_range;
        let (st, ct) = theta.sin_cos();
        let (sp, cp) = phi.sin_cos();
        Vec3::new(st * cp, st * sp, ct)
    };
    // The emitter-frame R(+Z, 90°) (`0x719114–0x719142`: axis literal (0,0,1), angle π·0.5,
    // Rodrigues + mat4_mul into the
    // per-frame emitter matrix rt+0x1fc): the reference prepends a fixed +90°-about-local-+Z
    // to EVERY M2 particle emitter's bone matrix, applied to the kernel-relative vectors only
    // (the record-position translation stays outside it). It is what turns the sphere kernel's
    // local-XZ ring into the wheel PERPENDICULAR to a rotor bone's +X spin — the InstancePortal
    // vortex swirling in place instead of tumbling edge-on — and what maps a billboard-bone
    // starburst's spray plane onto the screen plane (the impact flashes). Applied at emission,
    // so every consumer (anchored bake, model-space storage, child emitters) inherits it.
    (origin + rot90(local), rot90(dir))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use benilla_formats::ParticleShape;

    /// The kernel-test parameter set (the values the old flattened def carried).
    pub(crate) fn now() -> benilla_formats::ParamsNow {
        crate::testing::particle_params_now()
    }

    /// A minimal def for kernel tests — only the fields [`emit_local`] reads matter; the
    /// sampled-parameter side is [`now`] (constant-baked into `params` for the sim tests).
    pub(crate) fn def(shape: ParticleShape) -> ParticleEmitterDef {
        crate::testing::particle_def(shape)
    }

    /// Plane births stay in the ±½·area rectangle around the record position, and the cone is
    /// SYMMETRIC (θ = S11·range, `0x7b8890`'s correction of our old
    /// [0, range) draw): with a wide sample, x-velocities must land on both signs. The
    /// rectangle rides the R(+Z,90°) emitter frame (`emit_local`'s tail): the kernel's
    /// length-along-x/width-along-y rectangle lands **width-along-x, length-along-y**. Asserted on
    /// an ANISOTROPIC area (2 × 4) so the pairing is actually pinned — a square area passes either
    /// way, which is exactly how the swapped pairing survived 0563/0566 (Gressil's 0.1 × 1.1 blade
    /// smoke drew the 1.1 yd curtain ACROSS the blade).
    #[test]
    fn plane_kernel_rect_bounds_and_symmetric_cone() {
        let d = def(ParticleShape::Plane);
        let n = now();
        let mut rng = 12345u32;
        let (mut neg, mut pos) = (false, false);
        let (mut max_dx, mut max_dy) = (0.0f32, 0.0f32);
        for _ in 0..256 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            assert!(
                (p.x - 1.0).abs() <= 2.0 + 1e-4,
                "x within ±½·width of position (post-R)"
            );
            assert!(
                (p.y - 2.0).abs() <= 1.0 + 1e-4,
                "y within ±½·length of position (post-R)"
            );
            assert_eq!(p.z, 3.0);
            assert!(dir.z > 0.0, "cone around +Z stays upward for range < π/2");
            max_dx = max_dx.max((p.x - 1.0).abs());
            max_dy = max_dy.max((p.y - 2.0).abs());
            neg |= dir.x < -1e-3;
            pos |= dir.x > 1e-3;
        }
        assert!(neg && pos, "symmetric cone covers both x signs");
        // The PIN, not just the bound: the LONG extent must land on x. A swapped
        // areaLength/areaWidth pairing caps `max_dx` at ½·length = 1.0 and pushes `max_dy` past it,
        // so this pair of asserts is what fails on the 0563-era kernel.
        assert!(
            max_dx > 1.5,
            "the wide extent (½·width = 2.0) rides x post-R, reached {max_dx}"
        );
        assert!(
            max_dy <= 1.0 + 1e-4,
            "the narrow extent (½·length = 1.0) rides y post-R, reached {max_dy}"
        );
    }

    /// Sphere births sit on a shell with radius in [areaLength, areaWidth] (min/max radius for
    /// this shape) and fly radially outward through the shell point.
    #[test]
    fn sphere_kernel_radius_bounds_and_radial_velocity() {
        let d = def(ParticleShape::Sphere);
        let n = now();
        let mut rng = 99u32;
        for _ in 0..256 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            let rel = p - Vec3::new(1.0, 2.0, 3.0);
            let r = rel.length();
            assert!(
                (2.0 - 1e-3..=4.0 + 1e-3).contains(&r),
                "shell radius {r} within [min, max]"
            );
            assert!(
                rel.normalize().dot(dir) > 0.999,
                "velocity radial through the shell point"
            );
        }
    }

    /// A ZERO-radius sphere (min = max = 0 — the fireball impact's plume burst) still sprays its
    /// velocities across the authored lat/lon spread: the direction is the shell unit vector
    /// itself (`0x7b8fba` reuses the sincos pair), never a normalize of the (zero) birth offset.
    #[test]
    fn zero_radius_sphere_still_disperses() {
        let d = def(ParticleShape::Sphere);
        let mut n = now();
        n.area_length = 0.0;
        n.area_width = 0.0;
        n.vertical_range = std::f32::consts::PI; // the MoltenBlast plume: full ±π latitude fan
        n.horizontal_range = 0.0;
        let mut rng = 4242u32;
        let (mut up, mut down, mut fwd, mut back) = (false, false, false, false);
        for _ in 0..256 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            assert_eq!(p, Vec3::new(1.0, 2.0, 3.0), "births at the centre");
            assert!((dir.length() - 1.0).abs() < 1e-4, "unit direction");
            assert_eq!(
                dir.x, 0.0,
                "zero longitude range pins the fan to the YZ plane (the R(+Z,90°) emitter frame)"
            );
            up |= dir.z > 0.5;
            down |= dir.z < -0.5;
            fwd |= dir.y > 0.5;
            back |= dir.y < -0.5;
        }
        assert!(
            up && down && fwd && back,
            "the fan covers the full vertical ring, not a single degenerate ray"
        );
    }

    /// The SPLINE kernel (`0x7b9500`): births sit ON the authored chain (origin-composed) at
    /// arc fractions inside [tMin, tMax]; zero spin ⇒ ZERO velocity; a spin range fans +Z
    /// about the local tangent (here +X ⇒ the fan stays in the YZ plane), and scatter jitters
    /// the position along the velocity by at most its own length.
    #[test]
    fn spline_kernel_births_on_the_chain() {
        let x = |v: f32| [v, 0.0, 0.0];
        let mut d = def(ParticleShape::Spline);
        d.spline = benilla_formats::SplineData::new(vec![
            x(0.0),
            x(1.0),
            x(2.0),
            x(3.0), // one straight segment along +X
        ]);
        let mut n = now();
        n.area_length = 0.25; // tMin
        n.area_width = 0.75; // tMax
        n.vertical_range = 0.0;
        n.z_source = 0.0;
        let mut rng = 31u32;
        // The R(+Z,90°) emitter frame (`emit_local`'s tail) turns the authored +X chain to +Y.
        for _ in 0..64 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            let local = p - Vec3::new(1.0, 2.0, 3.0); // minus the record position
            assert!(
                (0.75 - 1e-3..=2.25 + 1e-3).contains(&local.y),
                "on the chain inside [tMin, tMax] (post-R along +Y): {}",
                local.y
            );
            assert_eq!((local.x, local.z), (0.0, 0.0));
            assert_eq!(dir, Vec3::ZERO, "no spin, no zSource: the flame stands");
        }
        // A spin range fans +Z about the +X tangent — the kernel's YZ fan lands in XZ post-R.
        n.vertical_range = 1.0;
        n.horizontal_range = 0.5; // scatter
        let (mut low, mut high) = (false, false);
        for _ in 0..256 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            assert!(dir.y.abs() < 1e-4, "fan in the XZ plane (post-R)");
            assert!((dir.length() - 1.0).abs() < 1e-4);
            assert!(dir.z > 0.0, "±1 rad about +Z stays upward");
            low |= dir.x > 0.5;
            high |= dir.x < -0.5;
            // The scatter jitter is along dir, bounded by its own length.
            let local = p - Vec3::new(1.0, 2.0, 3.0);
            assert!(local.x.hypot(local.z) <= 0.5 + 1e-3, "jitter ≤ scatter");
        }
        assert!(low && high, "the fan covers both spin signs");
    }

    /// zSource ≠ 0 redirects any shape's velocity radially away from the (0, 0, zSource) pivot.
    #[test]
    fn z_source_pivots_the_velocity() {
        let d = def(ParticleShape::Plane);
        let mut n = now();
        n.z_source = -1.0; // pivot below the emitter → births fly up-and-outward
        let mut rng = 7u32;
        for _ in 0..64 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            let rel = (p - Vec3::new(1.0, 2.0, 3.0)) - Vec3::new(0.0, 0.0, -1.0);
            assert!(rel.normalize().dot(dir) > 0.999, "radial from the pivot");
        }
    }
}
