//! The WoW ⇄ Bevy coordinate transform — the single documented boundary between raw WoW
//! coordinates (kept everywhere in `benilla-formats` / `benilla-protocol`, matching wowdev.wiki + vmangos)
//! and Bevy's render space.
//!
//! WoW is right-handed with **+X north, +Y west, +Z up**, 1 unit = 1 yard. Bevy is right-handed
//! with **+Y up, −Z forward**. The map is the pure rotation `bevy = (−y, z, −x)` — determinant +1,
//! so it never mirrors (winding/normals are preserved) and 1 unit stays 1 yard. See.
//!
//! Extracted from `main.rs` into its own module so the transform — including the quaternion
//! conjugation used for model placement, historically the trickiest part — is unit-tested in
//! isolation (no game assets, no GPU, no running app).

use bevy::math::{Mat3, Quat, Vec3};
use bevy::transform::components::Transform;

/// WoW `[x, y, z]` → Bevy. Pure rotation, 1 unit = 1 yard.
pub fn wow_to_bevy(p: [f32; 3]) -> Vec3 {
    Vec3::new(-p[1], p[2], -p[0])
}

/// Inverse of [`wow_to_bevy`]: Bevy coords → raw WoW `[x, y, z]` (for sending our position upstream).
pub fn bevy_to_wow(b: Vec3) -> [f32; 3] {
    [-b.z, -b.x, b.y]
}

/// The fixed WoW→Bevy basis rotation (the rotation part of [`wow_to_bevy`]); used to conjugate
/// placement rotations, since the model meshes are baked into Bevy space.
fn wow_to_bevy_quat() -> Quat {
    // Columns = Bevy images of the WoW basis: X→−Z, Y→−X, Z→+Y.
    Quat::from_mat3(&Mat3::from_cols(
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    ))
}

/// Convert an MDDF/MODF Euler rotation (degrees) into the Bevy quaternion for a baked (Bevy-space)
/// mesh.
///
/// Derived from wowdev.wiki's ADT/v18 `createPlacementMatrix`, whose output frame is — verified
/// against our own MDDF→position math — exactly the WoW gameplay frame (Z up). The model→world
/// rotation it gives is `Rx(90°)·Ry(90°)·Ry(ry−270°)·Rz(−rx)·Rx(rz−90°)`, which simplifies to
/// `Rx(90°)·Ry(ry−180°)·Rz(−rx)·Rx(rz−90°)`; we then conjugate it into Bevy space (the meshes are
/// baked there). `ry` is the heading about the up axis; `rx`/`rz` are pitch/roll.
///
/// The leading `Rx(90°)` is the **M2-local→world base reorientation** the old formula skipped — it
/// only changes the result when pitch/roll (`rx`/`rz`) are non-zero, so flat doodads and symmetric
/// trees are unaffected, but leaning/tilted segments (fences) now orient correctly.
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

/// The Bevy-space local [`Transform`] of a **WMO doodad** (an MODD entry) within its WMO's model
/// space. Compose it onto the WMO instance's world transform — `wmo_world.mul_transform(this)` — to
/// place the doodad in the world.
///
/// Unlike MDDF/MODF map placements (Euler angles + the client's base reorientation, see
/// [`placement_rotation`]), an MODD carries the doodad's **full orientation as a quaternion**
/// `(x, y, z, w)` directly — no separate base reorientation — alongside a position and uniform scale,
/// all in WMO model space (WoW axes, Z up; the same space the WMO's own geometry lives in, which we
/// already bake into Bevy via [`wow_to_bevy`]). So we map position via `wow_to_bevy`, conjugate the
/// rotation by the WoW→Bevy basis (the meshes are baked there), and keep the uniform scale (a uniform
/// scale is basis-invariant). This mirrors the conjugation in [`placement_rotation`].
pub fn wmo_doodad_local(position: [f32; 3], orientation: [f32; 4], scale: f32) -> Transform {
    Transform {
        translation: wow_to_bevy(position),
        rotation: wow_rotation_to_bevy(orientation),
        scale: Vec3::splat(scale),
    }
}

/// A rotation authored in **WoW model space** `(x, y, z, w)` → the equivalent rotation of a mesh
/// already baked into Bevy space: the basis conjugation `B · q · B⁻¹`.
///
/// The conjugation (rather than a component shuffle) is the whole point — [`wow_to_bevy`] is a
/// change of basis, and a rotation changes basis by similarity, not by permuting its parts. Every
/// caller that has a stored WoW quaternion wants this: an MODD doodad's orientation
/// ([`wmo_doodad_local`]) and an M2 bone track's rotation key alike.
pub fn wow_rotation_to_bevy(q: [f32; 4]) -> Quat {
    let q_wow = Quat::from_xyzw(q[0], q[1], q[2], q[3]);
    let to_bevy = wow_to_bevy_quat();
    // q and −q are the same rotation; normalize guards a denormalized stored quat.
    (to_bevy * q_wow * to_bevy.inverse()).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    /// `bevy_to_wow` must exactly undo `wow_to_bevy` for arbitrary points (a sign/axis slip in
    /// either direction would desync our outbound position from the server's frame).
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

    /// The yaw-conjugation law the transport platform frame rests on (decision 0438 phase 2):
    /// a WoW orientation θ about WoW-up (+Z) is exactly a Bevy yaw θ about Bevy-up (+Y) through
    /// the basis map — `wow_to_bevy(rot_z(θ)·v) == rot_y(θ)·wow_to_bevy(v)`. This is what lets a
    /// rider's boat-local pose convert between spaces with the plain linear map and lets the
    /// wire's local orientation be `face_yaw − boat_yaw` with no correction term.
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

    /// Pin the basis mapping with hand-verified golden values: north/west/up map where the docs say.
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

    /// The conjugation quaternion must reproduce `wow_to_bevy` on every vector — otherwise placement
    /// rotations are conjugated by the wrong basis and asymmetric doodads/buildings face wrong.
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

    /// Determinant +1 ⇒ a proper rotation (no mirroring); the whole coordinate decision rests on this.
    #[test]
    fn transform_is_a_proper_rotation() {
        let m = Mat3::from_quat(wow_to_bevy_quat());
        assert!(
            (m.determinant() - 1.0).abs() < 1e-5,
            "det = {}",
            m.determinant()
        );
    }

    /// A pure MDDF/MODF heading (`ry`) is a yaw about WoW +Z, which conjugates to a yaw about
    /// Bevy +Y — so it must leave the vertical axis fixed. This is the invariant that actually
    /// validates the conjugation (a wrong basis would tilt the model when it should only spin).
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

    /// An MODD with identity orientation is a pure translate (by `wow_to_bevy(pos)`) + uniform
    /// scale — no rotation. A wrong conjugation basis would leak rotation in here.
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

    /// A doodad at the WMO origin, composed onto the WMO instance's world transform, must land
    /// exactly at the instance's world position — the property the whole compose relies on.
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

    /// An MODD heading — a rotation about WoW +Z (up) — must conjugate to a yaw about Bevy +Y, so it
    /// leaves the vertical axis fixed (the same invariant that validates `placement_rotation`'s
    /// conjugation, but via the quaternion path). A wrong basis would tilt the doodad.
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

    /// Placement rotations are always unit quaternions (a non-unit quat would scale/shear the mesh).
    #[test]
    fn placement_rotations_are_unit_quaternions() {
        for r in [[0.0, 0.0, 0.0], [12.0, 34.0, 56.0], [-90.0, 180.0, 45.0]] {
            let q = placement_rotation(r);
            assert!((q.length() - 1.0).abs() < 1e-4, "non-unit quat for {r:?}");
        }
    }

    /// Golden snapshot of the full placement rotation, including pitch/roll — captured after the
    /// fence orientation was confirmed correct on screen. Locks the MDDF convention so a future
    /// change to the formula can't silently regress doodad/building orientation. The first case is a
    /// real leaning fence segment (`[rx, ry, rz]` degrees). Quaternions double-cover rotations, so
    /// compare by |dot| ≈ 1 (q and −q are the same rotation).
    #[test]
    fn placement_rotation_goldens() {
        use std::f32::consts::FRAC_1_SQRT_2; // 0.70710678…
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
