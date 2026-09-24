//! The order-2 SH light fold of the reference's `Model2.bls`: one closed form for the interior-prop
//! probes and the exterior sun rows, evaluated per fragment in `wow_model.wgsl`. There is no
//! separate amplitude scalar; intensity lives entirely in the committed colour.

use bevy::math::{Vec3, Vec4};

/// Folds committed lights into the 7-row order-2 SH probe, the closed form of the reference's
/// accumulators: ambient ×2√π into L00 (`0x71bc70`) and each directional lobe ×16π/17 on the
/// 9-term basis (`0x71bce0`), with band factors 1, 2/3 and 1/4, reduce to
/// `E += C·(4/17)·(0.375 + 2μ + 1.875μ²)`, μ = n·u. On the shader basis
/// `(n, 1, n.xy, n.yz, n.z², n.xz, n.x²−n.y²)`, per lobe of colour C and toward-light unit u:
///
///   DC     += C·(4/17)·(0.375 + 0.9375·(uₓ² + u_y²))
///   linear += C·(8/17)·u
///   n.xy   += C·(15/17)·uₓu_y      (and yz/xz alike)
///   n.z²   += C·(7.5/17)·(u_z² − ½(uₓ² + u_y²))
///   x²−y²  += C·(7.5/34)·(uₓ² − u_y²)
///
/// Ambient adds to DC ×1. The curve depends only on μ, so folding in Bevy space is exact. `lobes`
/// are `(toward-light unit, colour)`, pre-gained. E(μ=1) = C, the FFP peak, but the lobe is softer
/// than `max(N·L, 0)`: E(μ=0) = 0.088·C and E(μ=−1) = 0.059·C, the reference's authored wrap.
pub fn prop_probe_coeffs(ambient: [f32; 3], lobes: &[(Vec3, [f32; 3])]) -> [Vec4; 7] {
    const K: f32 = 4.0 / 17.0;
    let mut c = [Vec4::ZERO; 7];
    for (ch, a) in ambient.iter().enumerate() {
        c[ch].w = *a;
    }
    c[6].w = 1.0;
    for (u, col) in lobes {
        let u = u.normalize_or_zero();
        let (ux, uy, uz) = (u.x, u.y, u.z);
        let dc = K * (0.375 + 0.9375 * (ux * ux + uy * uy));
        let z2 = 1.875 * K * (uz * uz - 0.5 * (ux * ux + uy * uy));
        let x2y2 = 0.9375 * K * (ux * ux - uy * uy);
        for ch in 0..3 {
            let s = col[ch];
            // c10 = linear (2K·s·u) + DC lane.
            c[ch] += Vec4::new(2.0 * K * s * ux, 2.0 * K * s * uy, 2.0 * K * s * uz, s * dc);
            // c13 = quad: (n.xy, n.yz, n.z², n.xz) coefficients.
            c[3 + ch] += Vec4::new(
                3.75 * K * s * ux * uy,
                3.75 * K * s * uy * uz,
                s * z2,
                3.75 * K * s * ux * uz,
            );
            c[6][ch] += s * x2y2;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluate a prop probe at a normal, mirroring the shader's basis exactly.
    fn eval_probe(c: &[Vec4; 7], n: Vec3) -> [f32; 3] {
        let quad = Vec4::new(n.x * n.y, n.y * n.z, n.z * n.z, n.x * n.z);
        let n1 = n.extend(1.0);
        let x2y2 = n.x * n.x - n.y * n.y;
        [0usize, 1, 2].map(|ch| c[ch].dot(n1) + c[3 + ch].dot(quad) + c[6][ch] * x2y2)
    }

    /// The fixed interior axis travels `(−0.30822, −0.30822, −0.9)` in WoW space, so a world-up
    /// normal reads μ = 0.9 and a mesh gets `ambient + 0.869118·diffuse`, while the prop's
    /// particles take the fixed-function `ambient + 0.9·diffuse`
    /// ([`crate::terrain_stream::interior_light_up`]); the two lanes are meant to differ.
    #[test]
    fn a_world_up_normal_reads_the_fixed_interior_axis_at_a_constant() {
        let u = Vec3::new(-0.30822, 0.9, -0.30822);
        assert!((u.length() - 1.0).abs() < 1e-5, "the axis is authored unit");
        let c = prop_probe_coeffs([0.1, 0.2, 0.3], &[(u, [1.0, 1.0, 1.0])]);
        let got = eval_probe(&c, Vec3::Y);
        for (k, ambient) in [0.1f32, 0.2, 0.3].into_iter().enumerate() {
            assert!(
                (got[k] - (ambient + 0.869_118)).abs() < 1e-4,
                "ch{k}: got {} want {}",
                got[k],
                ambient + 0.869_118
            );
        }
    }

    /// The closed form at μ = 1, 0 and −1: `ambient + C`, `+ 0.0882·C` and `+ 0.0588·C`. The
    /// inputs are an abbey stand's (MODD[24]): ambient (61,59,96)/255, diffuse (90,86,141)/255,
    /// the fixed axis and no point lobes.
    #[test]
    fn prop_probe_matches_the_closed_form_at_the_anchor_normals() {
        let ambient = [61.0 / 255.0, 59.0 / 255.0, 96.0 / 255.0];
        let diffuse = [90.0 / 255.0, 86.0 / 255.0, 141.0 / 255.0];
        // The fixed interior axis, toward-light, in Bevy space: wow (0.30822, 0.30822, 0.9) →
        // (−wow.y, wow.z, −wow.x).
        let u = Vec3::new(-0.30822, 0.9, -0.30822).normalize();
        let c = prop_probe_coeffs(ambient, &[(u, diffuse)]);
        let close = |got: [f32; 3], want: [f32; 3], who: &str| {
            for k in 0..3 {
                assert!(
                    (got[k] - want[k]).abs() < 1e-5,
                    "{who}: ch{k} got {} want {}",
                    got[k],
                    want[k]
                );
            }
        };
        // μ = +1: ambient + diffuse, exactly.
        close(
            eval_probe(&c, u),
            [
                ambient[0] + diffuse[0],
                ambient[1] + diffuse[1],
                ambient[2] + diffuse[2],
            ],
            "facing",
        );
        // μ = −1: ambient + 0.0588·diffuse (the SH wrap floor).
        let k_back = 4.0 / 17.0 * 0.25;
        close(
            eval_probe(&c, -u),
            [
                ambient[0] + k_back * diffuse[0],
                ambient[1] + k_back * diffuse[1],
                ambient[2] + k_back * diffuse[2],
            ],
            "away",
        );
        // μ = 0: ambient + 0.0882·diffuse.
        let side = u.cross(Vec3::Y).normalize_or_zero();
        let side = if side.length_squared() < 0.5 {
            Vec3::X
        } else {
            side
        };
        let k_side = 4.0 / 17.0 * 0.375;
        close(
            eval_probe(&c, side),
            [
                ambient[0] + k_side * diffuse[0],
                ambient[1] + k_side * diffuse[1],
                ambient[2] + k_side * diffuse[2],
            ],
            "side-on",
        );
        // No lobes: the flat ambient probe.
        let flat = prop_probe_coeffs(ambient, &[]);
        close(eval_probe(&flat, Vec3::Y), ambient, "ambient-only");
        // Lobes are additive: a second lobe adds its own closed form independently.
        let two = prop_probe_coeffs(ambient, &[(u, diffuse), (Vec3::Y, [0.1, 0.2, 0.3])]);
        let one_flame = prop_probe_coeffs([0.0; 3], &[(Vec3::Y, [0.1, 0.2, 0.3])]);
        let (a, b, base) = (
            eval_probe(&two, u),
            eval_probe(&one_flame, u),
            eval_probe(&c, u),
        );
        for k in 0..3 {
            assert!((a[k] - (base[k] + b[k])).abs() < 1e-5, "additivity ch{k}");
        }
    }
}
