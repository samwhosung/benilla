//! The WoW to Bevy coordinate transform, the one boundary between raw WoW coordinates (kept in
//! `benilla-formats` and `benilla-protocol`) and Bevy's render space. WoW is right-handed with +X
//! north, +Y west, +Z up; Bevy is +Y up, −Z forward. The map is the rotation `bevy = (−y, z, −x)`,
//! determinant +1, so it never mirrors a winding and 1 unit stays 1 yard.

use bevy::math::{Mat3, Quat, Vec3};
use bevy::transform::components::Transform;

/// WoW `[x, y, z]` to Bevy.
pub fn wow_to_bevy(p: [f32; 3]) -> Vec3 {
    Vec3::new(-p[1], p[2], -p[0])
}

/// Inverse of [`wow_to_bevy`], for sending our position upstream.
pub fn bevy_to_wow(b: Vec3) -> [f32; 3] {
    [-b.z, -b.x, b.y]
}

/// The rotation of [`wow_to_bevy`], which conjugates placement rotations into Bevy space, where
/// the meshes are baked.
fn wow_to_bevy_quat() -> Quat {
    // Columns = Bevy images of the WoW basis: X→−Z, Y→−X, Z→+Y.
    Quat::from_mat3(&Mat3::from_cols(
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    ))
}

/// An MDDF/MODF Euler rotation (degrees) as the Bevy quaternion of a baked mesh: wowdev.wiki's ADT
/// `createPlacementMatrix`, `Rx(90°)·Ry(ry−180°)·Rz(−rx)·Rx(rz−90°)` in the WoW frame, conjugated
/// into Bevy space. `ry` is the heading, `rx` and `rz` pitch and roll; the leading `Rx(90°)` is the
/// M2-local base reorientation, which only shows on a tilted placement.
pub fn placement_rotation(rotation_deg: [f32; 3]) -> Quat {
    use std::f32::consts::{FRAC_PI_2, PI};
    let (rx, ry, rz) = (
        rotation_deg[0].to_radians(),
        rotation_deg[1].to_radians(),
        rotation_deg[2].to_radians(),
    );
    let in_wow = Quat::from_rotation_x(FRAC_PI_2)
        * Quat::from_rotation_y(ry - PI)
        * Quat::from_rotation_z(-rx)
        * Quat::from_rotation_x(rz - FRAC_PI_2);
    let to_bevy = wow_to_bevy_quat();
    to_bevy * in_wow * to_bevy.inverse()
}

/// An MODD doodad's Bevy-space [`Transform`] in its WMO's model space, to compose onto the
/// instance's (`wmo_world.mul_transform(this)`). Unlike MDDF and MODF, an MODD carries its full
/// orientation as a quaternion `(x, y, z, w)`, with no base reorientation.
pub fn wmo_doodad_local(position: [f32; 3], orientation: [f32; 4], scale: f32) -> Transform {
    Transform {
        translation: wow_to_bevy(position),
        rotation: wow_rotation_to_bevy(orientation),
        scale: Vec3::splat(scale),
    }
}

/// A WoW-space rotation `(x, y, z, w)` as the rotation of a mesh baked into Bevy space: the basis
/// conjugation `B · q · B⁻¹`, not a component shuffle.
pub fn wow_rotation_to_bevy(q: [f32; 4]) -> Quat {
    let q_wow = Quat::from_xyzw(q[0], q[1], q[2], q[3]);
    let to_bevy = wow_to_bevy_quat();
    // Normalized against a denormalized stored quaternion.
    (to_bevy * q_wow * to_bevy.inverse()).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn wow_bevy_round_trips() {
        for p in [
            [1.0, 2.0, 3.0],
            [-8949.95, -132.49, 83.53], // the Human start (Northshire)
            [0.0, 0.0, 0.0],
            [100.0, -50.0, 7.0],
        ] {
            let back = bevy_to_wow(wow_to_bevy(p));
            assert!(
                (0..3).all(|i| (back[i] - p[i]).abs() < 1e-3),
                "round-trip {p:?} → {back:?}"
            );
        }
    }

    /// `wow_to_bevy(rot_z(θ)·v) == rot_y(θ)·wow_to_bevy(v)`, which lets a transport rider's wire
    /// orientation be `face_yaw − boat_yaw` with no correction term.
    #[test]
    fn yaw_conjugates_through_the_basis_map() {
        for theta in [0.0f32, 0.7, 2.4, -1.1, 5.9] {
            let (s, c) = theta.sin_cos();
            for v in [[3.0f32, -2.0, 1.5], [10.0, 0.0, -4.0]] {
                let rotated_wow = [c * v[0] - s * v[1], s * v[0] + c * v[1], v[2]];
                let a = wow_to_bevy(rotated_wow);
                let b = Quat::from_rotation_y(theta) * wow_to_bevy(v);
                assert!(close(a, b), "θ={theta}: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn wow_bevy_basis_is_golden() {
        // WoW +X (north) → Bevy −Z (forward).
        assert!(close(
            wow_to_bevy([1.0, 0.0, 0.0]),
            Vec3::new(0.0, 0.0, -1.0)
        ));
        // WoW +Y (west) → Bevy −X.
        assert!(close(
            wow_to_bevy([0.0, 1.0, 0.0]),
            Vec3::new(-1.0, 0.0, 0.0)
        ));
        // WoW +Z (up) → Bevy +Y (up).
        assert!(close(
            wow_to_bevy([0.0, 0.0, 1.0]),
            Vec3::new(0.0, 1.0, 0.0)
        ));
    }

    #[test]
    fn placement_quat_matches_the_position_transform() {
        for w in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [3.0, -2.0, 5.0],
        ] {
            let via_quat = wow_to_bevy_quat() * Vec3::new(w[0], w[1], w[2]);
            assert!(close(via_quat, wow_to_bevy(w)), "quat vs fn for {w:?}");
        }
    }

    #[test]
    fn transform_is_a_proper_rotation() {
        let m = Mat3::from_quat(wow_to_bevy_quat());
        assert!(
            (m.determinant() - 1.0).abs() < 1e-5,
            "det = {}",
            m.determinant()
        );
    }

    #[test]
    fn heading_only_placement_spins_about_vertical() {
        for deg in [0.0, 30.0, 90.0, 250.0] {
            let q = placement_rotation([0.0, deg, 0.0]);
            assert!(
                close(q * Vec3::Y, Vec3::Y),
                "heading {deg}° should fix +Y, got {:?}",
                q * Vec3::Y
            );
        }
    }

    #[test]
    fn wmo_doodad_identity_is_translate_scale() {
        let t = wmo_doodad_local([3.0, -2.0, 5.0], [0.0, 0.0, 0.0, 1.0], 2.5);
        assert!(close(t.translation, wow_to_bevy([3.0, -2.0, 5.0])));
        assert!(
            t.rotation.dot(Quat::IDENTITY).abs() > 0.9999,
            "rot = {:?}",
            t.rotation
        );
        assert!((t.scale - Vec3::splat(2.5)).length() < 1e-5);
    }

    #[test]
    fn wmo_doodad_at_origin_lands_on_instance() {
        let wmo_world = Transform {
            translation: Vec3::new(10.0, 20.0, -30.0),
            rotation: placement_rotation([0.0, 137.0, 0.0]),
            scale: Vec3::ONE,
        };
        let local = wmo_doodad_local([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], 1.0);
        let world = wmo_world.mul_transform(local);
        assert!(close(world.translation, wmo_world.translation));
    }

    #[test]
    fn wmo_doodad_heading_spins_about_vertical() {
        for deg in [0.0_f32, 35.0, 90.0, 215.0] {
            let q = Quat::from_rotation_z(deg.to_radians()); // yaw about WoW +Z (up)
            let t = wmo_doodad_local([0.0, 0.0, 0.0], q.to_array(), 1.0);
            assert!(
                close(t.rotation * Vec3::Y, Vec3::Y),
                "heading {deg}° should fix +Y, got {:?}",
                t.rotation * Vec3::Y
            );
        }
    }

    #[test]
    fn placement_rotations_are_unit_quaternions() {
        for r in [[0.0, 0.0, 0.0], [12.0, 34.0, 56.0], [-90.0, 180.0, 45.0]] {
            let q = placement_rotation(r);
            assert!((q.length() - 1.0).abs() < 1e-4, "non-unit quat for {r:?}");
        }
    }

    /// The first case is a real leaning fence segment, `[rx, ry, rz]` in degrees. q and −q are the
    /// same rotation, so the check is |dot| ≈ 1.
    #[test]
    fn placement_rotation_goldens() {
        use std::f32::consts::FRAC_1_SQRT_2;
        let cases = [
            (
                [86.0_f32, 252.0, 93.5],
                Quat::from_xyzw(-0.691160, -0.107332, -0.156292, 0.697388),
            ),
            (
                [0.0, 0.0, 90.0],
                Quat::from_xyzw(FRAC_1_SQRT_2, -FRAC_1_SQRT_2, 0.0, 0.0),
            ),
            (
                [30.0, 45.0, 60.0],
                Quat::from_xyzw(0.360423, -0.822363, -0.391904, 0.200562),
            ),
        ];
        for (deg, expected) in cases {
            let q = placement_rotation(deg);
            assert!(
                q.dot(expected).abs() > 0.9999,
                "placement_rotation({deg:?}) = {q:?}, expected ~{expected:?}"
            );
        }
    }
}
