//! The ground-decal projector, the reference's `0x6d7330` → `0x6d6fa0` → `0x6d7480` chain: the
//! triangles of every [`GroundDecalSurface`] collider (terrain and WMO faces, never doodads or
//! GameObjects) in a box, clipped to it and emitted as world-space [`EffectVertex`] triangles with
//! planar top-down UVs, so a vertical face smears its texel column as in the reference. They are
//! coplanar with the drawn surface only through the world shaders' `clip_from_world`
//! (`DECAL_WORLD_CLIP`). Collector flags: the ring `0x200122`, the blob shadow `0x2f0122`, which
//! adds liquid; liquid is not a receiver here yet.

use avian3d::parry::bounding_volume::{Aabb as ParryAabb, BoundingVolume};
use avian3d::prelude::Collider;
use bevy::prelude::*;

use crate::collision::GroundDecalSurface;
use crate::particles::buffer::EffectVertex;

/// A decal's projection box around `center` (the owner's feet): a yaw-rotated rectangle, bounds in
/// the rotated frame (`x' = dx·cos − dz·sin`, `z' = dz·cos + dx·sin`), times a vertical slab.
pub struct DecalFrame {
    pub center: Vec3,
    pub sin: f32,
    pub cos: f32,
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
    /// Vertical bounds relative to `center.y` (`min_y` below, `max_y` above).
    pub min_y: f32,
    pub max_y: f32,
}

impl DecalFrame {
    /// In-frame horizontal coordinates of a world point (the same −θ rotation the UVs use).
    fn in_frame(&self, p: Vec3) -> (f32, f32) {
        let (dx, dz) = (p.x - self.center.x, p.z - self.center.z);
        (dx * self.cos - dz * self.sin, dz * self.cos + dx * self.sin)
    }

    /// The texture square as the frame rectangle, `[min_x, max_x] × [min_z, max_z] → [0,1]²`.
    pub fn rect_uv(&self, x: f32, z: f32) -> [f32; 2] {
        [
            (x - self.min_x) / (self.max_x - self.min_x),
            (z - self.min_z) / (self.max_z - self.min_z),
        ]
    }

    /// The world-axis-aligned gather AABB bounding the rotated box (for the BVH broad phase).
    fn gather_aabb(&self) -> ParryAabb {
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for (x, z) in [
            (self.min_x, self.min_z),
            (self.min_x, self.max_z),
            (self.max_x, self.min_z),
            (self.max_x, self.max_z),
        ] {
            // Inverse of `in_frame`: world offset = R(θ)·(x', z').
            let dx = x * self.cos + z * self.sin;
            let dz = z * self.cos - x * self.sin;
            lo = lo.min(Vec2::new(dx, dz));
            hi = hi.max(Vec2::new(dx, dz));
        }
        ParryAabb::new(
            Vec3::new(
                self.center.x + lo.x,
                self.center.y + self.min_y,
                self.center.z + lo.y,
            ),
            Vec3::new(
                self.center.x + hi.x,
                self.center.y + self.max_y,
                self.center.z + hi.y,
            ),
        )
    }
}

/// Project a decal into `out` as world-space, fan-unrolled white triangles, alpha from
/// `alpha(x', y_rel, z')` and UVs from `uv(x', z')`. `false` when no surface is in the box: the
/// caller hides the decal, the reference's no-ground gate (`0x6d74b5` skips the whole draw).
pub(crate) fn project_decal(
    out: &mut Vec<EffectVertex>,
    surfaces: &Query<&Collider, With<GroundDecalSurface>>,
    frame: &DecalFrame,
    alpha: impl Fn(Vec3) -> f32,
    uv: impl Fn(f32, f32) -> [f32; 2],
) -> bool {
    let gather = frame.gather_aabb();
    if frame.max_x - frame.min_x <= 0.0 || frame.max_z - frame.min_z <= 0.0 {
        return false;
    }
    let start = out.len();
    for collider in surfaces {
        // Marked colliders are identity-pose trimeshes: local AABBs and triangles are world ones.
        let Some(trimesh) = collider.shape().as_trimesh() else {
            continue;
        };
        if !trimesh.local_aabb().intersects(&gather) {
            continue;
        }
        for i in trimesh.bvh().intersect_aabb(&gather) {
            let tri = trimesh.triangle(i);
            let (poly, n) = clip_to_frame([tri.a, tri.b, tri.c], frame);
            if n < 3 {
                continue;
            }
            let vert = |p: Vec3| {
                let (u, v) = frame.in_frame(p);
                let a = alpha(Vec3::new(u, p.y - frame.center.y, v));
                EffectVertex {
                    pos: p.to_array(),
                    uv: uv(u, v),
                    color: [1.0, 1.0, 1.0, a],
                }
            };
            // Fan-unrolled: the stream is a triangle list with no shared vertices.
            for k in 1..n - 1 {
                out.push(vert(poly[0]));
                out.push(vert(poly[k]));
                out.push(vert(poly[k + 1]));
            }
        }
    }
    out.len() > start
}

/// Sutherland-Hodgman clip of a triangle to the frame's box, in the rotated frame so UVs stay in
/// `[0,1]` and never wrap; edges interpolate in 3D, so the result stays on the triangle's plane.
fn clip_to_frame(tri: [Vec3; 3], frame: &DecalFrame) -> ([Vec3; 9], usize) {
    let rx = |p: Vec3| frame.in_frame(p).0;
    let rz = |p: Vec3| frame.in_frame(p).1;
    // Signed inside-distances for the six half-planes of the box.
    let planes: [&dyn Fn(Vec3) -> f32; 6] = [
        &|p: Vec3| frame.max_x - rx(p),
        &|p: Vec3| rx(p) - frame.min_x,
        &|p: Vec3| frame.max_z - rz(p),
        &|p: Vec3| rz(p) - frame.min_z,
        &|p: Vec3| (frame.center.y + frame.max_y) - p.y,
        &|p: Vec3| p.y - (frame.center.y + frame.min_y),
    ];
    // A convex clip adds at most one vertex per plane, so 3 + 6 = 9 bounds both stack buffers.
    let mut poly = [Vec3::ZERO; 9];
    let mut next = [Vec3::ZERO; 9];
    poly[..3].copy_from_slice(&tri);
    let mut len = 3;
    for dist in planes {
        let mut out = 0;
        for i in 0..len {
            let (a, b) = (poly[i], poly[(i + 1) % len]);
            let (da, db) = (dist(a), dist(b));
            if da >= 0.0 {
                next[out] = a;
                out += 1;
            }
            // The edge crosses the plane: emit the intersection point.
            if (da >= 0.0) != (db >= 0.0) {
                next[out] = a + (b - a) * (da / (da - db));
                out += 1;
            }
        }
        std::mem::swap(&mut poly, &mut next);
        len = out;
        if len < 3 {
            return (poly, len);
        }
    }
    (poly, len)
}

/// Projects decals onto the world's receiving surfaces, for every lane that draws flat on the
/// ground; which colliders receive is decided at bake time by the MOPY material class.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WorldDecal<'w, 's> {
    surfaces: Query<'w, 's, &'static Collider, With<GroundDecalSurface>>,
}

impl WorldDecal<'_, '_> {
    /// [`project_decal`] over the receiving surfaces; `false` means hide the decal (`0x6d74b5`).
    pub fn project(
        &self,
        out: &mut Vec<crate::particles::buffer::EffectVertex>,
        frame: &DecalFrame,
        alpha: impl Fn(Vec3) -> f32,
        uv: impl Fn(f32, f32) -> [f32; 2],
    ) -> bool {
        project_decal(out, &self.surfaces, frame, alpha, uv)
    }

    /// How many receiving surfaces are resident; with none, a decal has nothing to land on.
    pub fn receiver_count(&self) -> usize {
        self.surfaces.iter().count()
    }

    /// Whether this collider receives ground decals, which the mount tilt asks of its down-ray hit.
    pub fn receives(&self, entity: Entity) -> bool {
        self.surfaces.contains(entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An axis-aligned 2×2×2 box around the origin, so in-frame coordinates are world `(x, z)`.
    fn frame() -> DecalFrame {
        DecalFrame {
            center: Vec3::ZERO,
            sin: 0.0,
            cos: 1.0,
            min_x: -1.0,
            max_x: 1.0,
            min_z: -1.0,
            max_z: 1.0,
            min_y: -1.0,
            max_y: 1.0,
        }
    }

    #[test]
    fn a_triangle_inside_the_box_passes_through_unchanged() {
        let tri = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.5),
        ];
        let (poly, n) = clip_to_frame(tri, &frame());
        assert_eq!(n, 3);
        // Same vertices, same order: the caller's fan rides it.
        assert_eq!(&poly[..3], &tri);
    }

    #[test]
    fn a_half_clipped_triangle_emits_the_hand_computed_polygon() {
        // Crosses only the `max_x` plane; every value below is exact in f32.
        let tri = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.5),
        ];
        let (poly, n) = clip_to_frame(tri, &frame());
        assert_eq!(n, 4);
        // Each surviving vertex first, then its edge's crossing point.
        assert_eq!(
            &poly[..4],
            &[
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.25),
                Vec3::new(0.0, 0.0, 0.5),
            ]
        );
    }

    #[test]
    fn a_triangle_outside_the_box_clips_to_nothing() {
        let tri = [
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 1.0),
        ];
        let (_, n) = clip_to_frame(tri, &frame());
        assert!(n < 3, "the caller drops it before fanning");
    }
}
