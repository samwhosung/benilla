//! The emission shape kernel and its RNG: one birth's position and direction, as the reference's
//! three spawn generators make them (`0x7b8890` plane, `0x7b8d70` sphere, `0x7b9500` spline).

use benilla_formats::ParticleEmitterDef;
use bevy::prelude::*;

/// xorshift32 for particle jitter: visual only, determinism not required.
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

/// Uniform in (−1, 1): the reference's `S11` draw (`0x7b88dd`), same distribution, not stream.
pub(super) fn rand_s11(state: &mut u32) -> f32 {
    rand01(state) * 2.0 - 1.0
}

/// One birth's position and unit direction in the emitter's local WoW frame (Z up, origin at the
/// record's `position`), before the caller's speed roll and space mode:
/// - plane (`0x7b8890`): uniform in the ±½·area rectangle, x from `areaLength` (`rt+0x290`) and
///   y from `areaWidth` (`rt+0x294`), in a cone about +Z at θ = S11·verticalRange and
///   φ = S11·horizontalRange;
/// - sphere (`0x7b8d70`): radius uniform in [areaLength, areaWidth], on the shell at those two
///   angles as latitude and longitude, direction radial (`sphere_up`: +Z);
/// - any shape with `zSource ≠ 0`: direction radial from the pivot `(0, 0, zSource)`.
pub(super) fn emit_local(
    def: &ParticleEmitterDef,
    now: &benilla_formats::ParamsNow,
    rng: &mut u32,
) -> (Vec3, Vec3) {
    let origin = Vec3::from(def.position);
    // The emitter frame's R(+Z, 90°), applied at every return, spline included: the reference
    // prepends it per emitter, whatever the shape (`0x71907f`).
    let rot90 = |v: Vec3| Vec3::new(-v.y, v.x, v.z);
    // Spline (`0x7b9500`): born on the authored Bézier chain at arc fraction t in [tMin, tMax]
    // (the area fields). Direction: radial from the zSource pivot when authored; else +Z turned
    // about the tangent by S11·spin (`vertical_range`; the reference's −ψ is the same
    // distribution) and jittered along it by U01·scatter (`horizontal_range`); else zero. No
    // parsed chain falls through to the plane kernel.
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
    // A sphere's one lat/lon unit vector is both the shell point and the radial direction (the
    // reference reuses the sincos pair, `0x7b8fba`), so a zero-radius sphere still sprays.
    let (local, shell) = if def.shape == benilla_formats::ParticleShape::Sphere {
        let r = now.area_length + rand01(rng) * (now.area_width - now.area_length).max(0.0);
        let lat = rand_s11(rng) * now.vertical_range;
        let lon = rand_s11(rng) * now.horizontal_range;
        let (slat, clat) = lat.sin_cos();
        let (slon, clon) = lon.sin_cos();
        let shell = Vec3::new(clat * clon, clat * slon, slat); // unit by construction
        (r * shell, Some(shell))
    } else {
        // x from areaLength, y from areaWidth: swapped, an anisotropic rectangle turns 90°.
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
        // Radial from the (0, 0, zSource) pivot; +Z at the pivot itself.
        (local - Vec3::new(0.0, 0.0, now.z_source)).normalize_or(Vec3::Z)
    } else if let Some(shell) = shell {
        if def.sphere_up() {
            Vec3::Z
        } else {
            shell // radial: the same unit draw as the birth point
        }
    } else {
        let theta = rand_s11(rng) * now.vertical_range;
        let phi = rand_s11(rng) * now.horizontal_range;
        let (st, ct) = theta.sin_cos();
        let (sp, cp) = phi.sin_cos();
        Vec3::new(st * cp, st * sp, ct)
    };
    // The reference prepends a fixed +90° about local +Z to every M2 particle emitter's matrix
    // (`0x719114`..`0x719142`, into `rt+0x1fc`), on the kernel-relative vectors only, not the
    // record position; applied here at emission, every consumer inherits it.
    (origin + rot90(local), rot90(dir))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use benilla_formats::ParticleShape;

    pub(crate) fn now() -> benilla_formats::ParamsNow {
        crate::testing::particle_params_now()
    }

    pub(crate) fn def(shape: ParticleShape) -> ParticleEmitterDef {
        crate::testing::particle_def(shape)
    }

    /// The cone is symmetric (`0x7b8890`), and after the R(+Z, 90°) frame width lies along x and
    /// length along y, pinned on an anisotropic 2 × 4 area (a square one passes either way).
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
        // The long extent must land on x: a swapped pairing caps `max_dx` at ½·length = 1.0.
        assert!(
            max_dx > 1.5,
            "the wide extent (½·width = 2.0) rides x post-R, reached {max_dx}"
        );
        assert!(
            max_dy <= 1.0 + 1e-4,
            "the narrow extent (½·length = 1.0) rides y post-R, reached {max_dy}"
        );
    }

    /// A sphere's `areaLength` and `areaWidth` are its min and max radius.
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

    /// The direction is the shell unit vector (`0x7b8fba`), not the zero offset normalized.
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

    /// `0x7b9500`: no spin, no velocity; a spin range fans +Z about the tangent, scatter along it.
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
        // The R(+Z, 90°) frame turns the authored +X chain to +Y.
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
        // A spin range fans +Z about the +X tangent; after R the fan lies in XZ.
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
            let local = p - Vec3::new(1.0, 2.0, 3.0);
            assert!(local.x.hypot(local.z) <= 0.5 + 1e-3, "jitter ≤ scatter");
        }
        assert!(low && high, "the fan covers both spin signs");
    }

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
