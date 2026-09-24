//! The shared world-surface **decal projector** — the reference's own ground-decal mechanism
//! (the `0x6d7330` → `0x6d6fa0` matrices → `0x6d7480` emit chain): gather the triangles of every
//! [`GroundDecalSurface`] collider (terrain
//! tiles + WMO faces — **never** doodads/GameObjects) whose BVH overlaps a projection box, clip
//! each to the box ([`clip_to_frame`]), and emit them with planar top-down UVs. Because the
//! emitted triangles are exact sub-pieces of the drawn surfaces, a decal is pixel-coplanar with
//! what's on screen — **provided it transforms through the same `clip_from_world` matrix as the
//! world-mesh shaders** (the `DECAL_WORLD_CLIP` lane variant; the cam-relative route reaches
//! the same plane through different arithmetic and misses by more than the bias at WoW-scale
//! coordinates — decision 0781), the rasterizer `depth_bias` settling the depth test — and
//! drapes down
//! steps and ledge faces precisely like the reference (a vertical face gets the smeared texel
//! column of its XZ spot: projective texturing, faithfully).
//!
//! Since 0733 the projector emits **effect-stream triangles** ([`EffectVertex`], world-space,
//! fan-unrolled) instead of `Mesh` assets: its clients cache the projected slice and push it
//! into the shared lane per frame — zero mesh churn, tint/fade applied at push time.
//!
//! Three clients — the same emit loop in the binary: the **selection ring**
//! ([`crate::target`]`::ring`, collector flags `0x200122`), the **unit blob shadow**
//! ([`crate::blob_shadow`], flags `0x2f0122` — the ring's + the liquid receivers, a gap here:
//! liquid surfaces aren't in the [`GroundDecalSurface`] set yet), and the ground-fx spell
//! decals ([`crate::ground_fx`]).

use avian3d::parry::bounding_volume::{Aabb as ParryAabb, BoundingVolume};
use avian3d::prelude::Collider;
use bevy::prelude::*;

use crate::collision::GroundDecalSurface;
use crate::particles::buffer::EffectVertex;

/// A decal's projection box: a yaw-rotated horizontal rectangle × a vertical slab, all relative
/// to `center` (the owning object's feet). The horizontal bounds live in the **rotated frame**
/// (`x' = dx·cos − dz·sin`, `z' = dz·cos + dx·sin`); UVs map `[min_x, max_x] × [min_z, max_z]`
/// to `[0,1]²`, so the texture square IS this rectangle. An axis-aligned box passes
/// `(sin, cos) = (0, 1)`.
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

    /// The default UV map — the texture square IS the frame rectangle:
    /// `[min_x, max_x] × [min_z, max_z] → [0,1]²` (the ring's and blob shadow's mapping). The
    /// ground-fx lane substitutes a bilinear map over the source quad's authored corner UVs.
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

/// Project a surface decal into `out` as **world-space, fan-unrolled effect triangles**: the
/// [`GroundDecalSurface`] triangles are gathered, clipped to `frame`'s box, and emitted with
/// top-down UVs. `alpha` computes each vertex's colour alpha from its in-frame position
/// `(x', y_rel, z')` (vertical fades, edge ramps); the colour is white × that alpha — the
/// pushing client multiplies its tint/fade in. `uv` maps in-frame `(x', z')` to the emitted
/// texture coordinate ([`DecalFrame::rect_uv`] for the plain texture-square decals; the
/// ground-fx lane bilerps its quad's authored corner UVs). Returns `false` when nothing was
/// gathered (no receiving surface in the box) — the caller hides the decal, the reference's
/// own no-ground gate (`0x6d74b5`: the whole draw is skipped).
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
        // The marked colliders are static trimeshes with world-space vertices (identity pose),
        // so their local AABB/triangles are world AABB/triangles.
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
            // Fan-triangulate the clipped convex polygon, unrolled (the stream's tri-list
            // topology: identity indices, no shared vertices).
            for k in 1..n - 1 {
                out.push(vert(poly[0]));
                out.push(vert(poly[k]));
                out.push(vert(poly[k + 1]));
            }
        }
    }
    out.len() > start
}

/// Sutherland–Hodgman clip of a triangle against the frame's projection box: the yaw-rotated
/// horizontal rectangle (clipping in the rotated frame is exactly the texture frame, so UVs stay
/// in `[0,1]` and the texture can never wrap ghost copies in at the corners) and the vertical
/// slab. Interpolates full 3D positions along clipped edges, so the result stays on the source
/// triangle's plane. Returns the polygon buffer and its vertex count — fewer than 3 when the
/// triangle lies outside the box.
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
    // Stack ping-pong (this runs per gathered triangle per frame): a convex clip grows the
    // polygon by at most one vertex per plane, so 3 + 6 = 9 bounds both buffers.
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
            // The edge crosses the plane → emit the intersection point.
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

/// **Project a decal onto the world's receiving surfaces** — the face for every lane that draws
/// something flat on the ground.
///
/// Four lanes (the blob shadow, footprints, the selection ring, the targeting reticle) each held
/// `Query<&Collider, With<GroundDecalSurface>>` and passed it to [`project_decal`]. The receiver
/// set is the world's business — which colliders take a decal is decided at bake time by the MOPY
/// material class, not by the lane drawing on them — so the query goes here and the lanes ask a
/// question instead of carrying an answer.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WorldDecal<'w, 's> {
    surfaces: Query<'w, 's, &'static Collider, With<GroundDecalSurface>>,
}

impl WorldDecal<'_, '_> {
    /// Clip every receiving triangle in `frame`'s box and emit it into `out` with top-down UVs.
    ///
    /// `alpha` computes each vertex's colour alpha from its in-frame position (vertical fades,
    /// edge ramps); `uv` maps in-frame `(x', z')` to a texture coordinate. Returns `false` when
    /// nothing was gathered — no receiving surface in the box — and the caller hides its decal,
    /// which is the reference's own no-ground gate (`0x6d74b5`: the whole draw is skipped).
    pub fn project(
        &self,
        out: &mut Vec<crate::particles::buffer::EffectVertex>,
        frame: &DecalFrame,
        alpha: impl Fn(Vec3) -> f32,
        uv: impl Fn(f32, f32) -> [f32; 2],
    ) -> bool {
        project_decal(out, &self.surfaces, frame, alpha, uv)
    }

    /// How many receiving surfaces are resident — the decal lanes' census line, which is how a
    /// "my shadow vanished" report gets answered with "there was nothing under you".
    pub fn receiver_count(&self) -> usize {
        self.surfaces.iter().count()
    }

    /// Does this collider **receive** ground decals?
    ///
    /// The mount-tilt probe asks it of whatever its down-ray hit, to decide whether the surface is
    /// ground worth tilting to. That is a different question from projecting and deserves to read
    /// like one — it had been spelling the marker into its own query filter.
    pub fn receives(&self, entity: Entity) -> bool {
        self.surfaces.contains(entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An axis-aligned 2×2×2 box around the origin (`(sin, cos) = (0, 1)`), so in-frame
    /// coordinates ARE world `(x, z)` and the expected polygons can be written by hand.
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
        // Same vertices, same order — the caller's fan (`poly[0], poly[k], poly[k+1]`) rides it.
        assert_eq!(&poly[..3], &tri);
    }

    #[test]
    fn a_half_clipped_triangle_emits_the_hand_computed_polygon() {
        // Crosses only the `max_x` plane; every value below is exact in f32, so `==` is honest.
        let tri = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.5),
        ];
        let (poly, n) = clip_to_frame(tri, &frame());
        assert_eq!(n, 4);
        // The emit order: each surviving vertex first, then its edge's crossing point.
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
