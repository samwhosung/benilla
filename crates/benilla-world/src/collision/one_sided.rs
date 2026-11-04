//! One-sided movement collision. The reference gathers faces at their file winding (`0x671cc0`)
//! and its resolver skips any with `n·dir > −1e-5` (`0x632700`) for every caller of `0x632ba0`:
//! falling, walking, step-up, ground settle, transports and the water surface, so a floor is
//! filtered exactly like a wall.
//!
//! parry's trimesh is two-sided, and a first-hit cast cannot yield the front face behind a
//! backface, so this re-runs avian's `move_and_slide` over its public pieces and gates each
//! triangle in its own BVH walk. The winding seen is the authored one: `Collider::trimesh` keeps
//! it, and every transform is a rotation. Convex colliders stay whole-shape, and camera and
//! line-of-sight casts stay two-sided, as the reference's segment kernel is (`0x7c29f0`).
//!
//! Deviation: the slide, step and snap are avian's kinematic controller, with parry's edge-hit
//! normals, not the reference's resolver, because pieces of that resolver grafted onto the
//! controller left the mover in states the reference never reaches; only its face filters, the
//! facing law and the backface band, are ported, the band on the reference's own volume standing
//! at the capsule's feet ([`Band`]). A cast also reports the hit face's own normal, and every
//! class the reference takes from its face record reads it: the ground classify, the settle snap,
//! the step trigger and the step-up's floor (`0x6334e6`, `0x636ecf`, `0x636592`), except on a
//! shared edge ([`EDGE_TIE`]).
//!
//! There is no depenetration: the reference's resolver (`0x634040`) is a sweep with no push-out, so
//! a body inside geometry, such as a player the server seats inside a chair's box, walks out
//! through the faces wound away from it instead of being shoved sideways.

use avian3d::character_controller::move_and_slide::{
    MoveAndSlide, MoveAndSlideConfig, MoveAndSlideHitData, MoveAndSlideHitResponse,
    MoveAndSlideOutput, MoveHitData,
};
use avian3d::parry::bounding_volume::Aabb as ParryAabb;
use avian3d::parry::math::Pose3;
use avian3d::parry::query::{
    cast_shapes, contact, Ray, RayCast, ShapeCastOptions, ShapeCastStatus,
};
use avian3d::parry::shape::Triangle;
use avian3d::prelude::*;
use bevy::prelude::*;
use core::time::Duration;

use super::BodyHit;

/// The facing gate (`[0x80c5c4]`, −1e-5): a face blocks only when `n·dir` is at most this, `n` its
/// authored normal and `dir` the unit motion.
const FACING_EPS: f32 = f32::from_bits(0xb727_c5ac);

/// avian's `pull_back` floor for a near-zero `n·dir` (its `move_and_slide.rs`).
const DOT_EPSILON: f32 = 0.005;

/// The backface band, 1/36 yd (`[0x7ff9c8]` holds −1/36): `0x632830` drops a facet whose part inside the
/// volume lies wholly more than this behind a leading face along the motion ([`Band`]).
const BACKFACE_BAND: f32 = 1.0 / 36.0;

/// `0x632830`'s all-behind bound (`[0x80dff4]`, −2^-20): a clipped vertex above it is ahead of the
/// face, and the facet is a contact.
const BEHIND_EPS: f32 = f32::from_bits(0xb580_0000);

/// The polygon clip's band (`[0x80dfec]`, 1/720 yd, `0x6318c0`).
const CLIP_EPS: f32 = f32::from_bits(0x3ab6_0b61);

/// The volume's bevel normal, horizontal and vertical parts (`[0x80dfe4]`, `[0x80dfe0]`): the
/// sine and cosine of 61.6°.
const BEVEL_H: f32 = f32::from_bits(0x3f61_3036);
const BEVEL_V: f32 = f32::from_bits(0x3ef3_86a4);

/// The bevels' rise per yard out from the apex, `BEVEL_H / BEVEL_V` (`[0x80c740]`, `0x631c0b`).
const BEVEL_RISE: f32 = f32::from_bits(0x3fec_b91b);

/// parry's cast reaches both faces of one edge within this (yd) of each other; 1e-4 splits them.
/// Deviation: the reference keeps the later record in file order on an exact tie (`0x6327e2`),
/// and its box-and-cone volume meets a lip where our capsule does not, so a capsule hit on an
/// edge two different front faces share is classified by its contact normal. A face the facing
/// law drops (a riser under a down sweep) never ties, so a stair's nosing reads as its tread.
const EDGE_TIE: f32 = 1.0e-3;

/// One-sided [`MoveAndSlide::cast_move`]: sweeps the axis-aligned `shape` along `movement` to
/// `skin_width` short of the first face whose authored winding opposes the motion, with that face's
/// own normal beside the contact's.
pub(crate) fn cast_move(
    ms: &MoveAndSlide<'_, '_>,
    shape: &Collider,
    from: Vec3,
    movement: Vec3,
    skin_width: f32,
    filter: &SpatialQueryFilter,
) -> Option<BodyHit> {
    let (dir, len) = Dir3::new_and_length(movement).unwrap_or((Dir3::X, 0.0));
    let max_toi = len + skin_width;

    let a0 = shape.aabb(from, Quat::IDENTITY);
    let a1 = shape.aabb(from + movement, Quat::IDENTITY);
    let swept = ColliderAabb {
        min: a0.min.min(a1.min) - Vec3::splat(skin_width),
        max: a0.max.max(a1.max) + Vec3::splat(skin_width),
    };

    let band = Band::new(shape, from, dir, len);
    let mut best: Option<BodyHit> = None;
    let mut best_toi = max_toi;
    for entity in ms.spatial_query.aabb_intersections_with_aabb(swept) {
        let Ok((collider, pos, rot, layers)) = ms.colliders.get(entity) else {
            continue;
        };
        if !filter.test(entity, layers.copied().unwrap_or_default()) {
            continue;
        }

        if let Some(trimesh) = collider.shape_scaled().as_trimesh() {
            // In the trimesh's local frame; a rotation keeps the sign of every `n·dir`.
            let inv_rot = rot.0.inverse();
            let local_from = inv_rot * (from - pos.0);
            let local_dir = inv_rot * *dir;
            let local_pose = Pose3::from_parts(local_from, inv_rot);
            for tri_id in trimesh
                .bvh()
                .intersect_aabb(&aabb_to_local(swept, pos.0, inv_rot))
            {
                let tri = trimesh.triangle(tri_id);
                let Some(n) = tri.normal() else {
                    continue; // a degenerate face has no winding
                };
                if n.dot(local_dir) > FACING_EPS {
                    continue; // approached from its back
                }
                let Ok(Some(hit)) = cast_shapes(
                    &Pose3::IDENTITY,
                    Vec3::ZERO,
                    &tri,
                    &local_pose,
                    local_dir,
                    shape.shape_scaled().as_ref(),
                    ShapeCastOptions {
                        max_time_of_impact: best_toi + EDGE_TIE,
                        target_distance: 0.0,
                        stop_at_penetration: false,
                        compute_impact_geometry_on_penetration: true,
                    },
                ) else {
                    continue;
                };
                // An overlap at `t = 0` is a hit only inside the band.
                if hit.status == ShapeCastStatus::PenetratingOrWithinTargetDist
                    && !band.keeps_local(&tri, pos.0, rot.0)
                {
                    continue;
                }
                let face_normal = rot.0 * n;
                let tie = best.as_ref().is_some_and(|b| {
                    (hit.time_of_impact - best_toi).abs() <= EDGE_TIE
                        && !face_normal.abs_diff_eq(b.face_normal, 1.0e-4)
                });
                if hit.time_of_impact < best_toi || tie {
                    let earlier = hit.time_of_impact < best_toi;
                    let found = BodyHit {
                        hit: MoveHitData {
                            entity,
                            distance: 0.0, // pulled back below, once, for the winner
                            point1: pos.0 + rot.0 * hit.witness1,
                            point2: pos.0 + rot.0 * (hit.witness2 + local_dir * hit.time_of_impact),
                            normal1: rot.0 * hit.normal1,
                            normal2: rot.0 * hit.normal2,
                            collision_distance: hit.time_of_impact,
                        },
                        face_normal,
                    };
                    let mut won = match (earlier, best) {
                        (true, _) | (_, None) => found,
                        (false, Some(b)) => b,
                    };
                    // An edge two different front faces share names neither: the contact's own
                    // normal stands for it.
                    if tie {
                        won.face_normal = won.hit.normal1;
                    }
                    best_toi = best_toi.min(hit.time_of_impact);
                    best = Some(won);
                }
            }
        } else {
            // A convex collider has no reachable backface: avian's whole-shape sweep.
            let Ok(Some(hit)) = cast_shapes(
                &Pose3::from_parts(pos.0, rot.0),
                Vec3::ZERO,
                collider.shape_scaled().as_ref(),
                &Pose3::from_parts(from, Quat::IDENTITY),
                *dir,
                shape.shape_scaled().as_ref(),
                ShapeCastOptions {
                    max_time_of_impact: best_toi,
                    target_distance: 0.0,
                    stop_at_penetration: false,
                    compute_impact_geometry_on_penetration: true,
                },
            ) else {
                continue;
            };
            if hit.time_of_impact < best_toi {
                best_toi = hit.time_of_impact;
                let normal1 = rot.0 * hit.normal1;
                best = Some(BodyHit {
                    hit: MoveHitData {
                        entity,
                        distance: 0.0,
                        point1: pos.0 + rot.0 * hit.witness1,
                        point2: from + hit.witness2 + *dir * hit.time_of_impact,
                        normal1,
                        normal2: hit.normal2,
                        collision_distance: hit.time_of_impact,
                    },
                    face_normal: normal1,
                });
            }
        }
    }

    best.map(
        |BodyHit {
             mut hit,
             face_normal,
         }| {
            // avian's skin-width pull-back: `skin / |n·dir|` short of the surface, never negative.
            hit.distance = if max_toi == 0.0 {
                0.0
            } else {
                let dot = dir.dot(-hit.normal1).max(DOT_EPSILON);
                (hit.collision_distance - skin_width / dot).max(0.0)
            };
            BodyHit { hit, face_normal }
        },
    )
}

/// One-sided [`SpatialQuery::cast_ray`] for movement probes that are rays, such as the creature
/// ground clamp: the reference's walk resolver gates its down-probe like every other caller.
pub(crate) fn cast_ray(
    ms: &MoveAndSlide<'_, '_>,
    origin: Vec3,
    dir: Dir3,
    max_distance: f32,
    filter: &SpatialQueryFilter,
) -> Option<RayHitData> {
    let end = origin + *dir * max_distance;
    let swept = ColliderAabb {
        min: origin.min(end),
        max: origin.max(end),
    };
    let mut best: Option<RayHitData> = None;
    let mut best_toi = max_distance;
    for entity in ms.spatial_query.aabb_intersections_with_aabb(swept) {
        let Ok((collider, pos, rot, layers)) = ms.colliders.get(entity) else {
            continue;
        };
        if !filter.test(entity, layers.copied().unwrap_or_default()) {
            continue;
        }
        let inv_rot = rot.0.inverse();
        let local_ray = Ray::new(inv_rot * (origin - pos.0), inv_rot * *dir);
        if let Some(trimesh) = collider.shape_scaled().as_trimesh() {
            for tri_id in trimesh
                .bvh()
                .intersect_aabb(&aabb_to_local(swept, pos.0, inv_rot))
            {
                let tri = trimesh.triangle(tri_id);
                let Some(n) = tri.normal() else {
                    continue;
                };
                if n.dot(local_ray.dir) > FACING_EPS {
                    continue;
                }
                let Some(hit) = tri.cast_local_ray_and_get_normal(&local_ray, best_toi, true)
                else {
                    continue;
                };
                if hit.time_of_impact < best_toi {
                    best_toi = hit.time_of_impact;
                    best = Some(RayHitData {
                        entity,
                        distance: hit.time_of_impact,
                        normal: rot.0 * hit.normal,
                    });
                }
            }
        } else {
            let Some(hit) = collider
                .shape_scaled()
                .cast_local_ray_and_get_normal(&local_ray, best_toi, true)
            else {
                continue;
            };
            if hit.time_of_impact < best_toi {
                best_toi = hit.time_of_impact;
                best = Some(RayHitData {
                    entity,
                    distance: hit.time_of_impact,
                    normal: rot.0 * hit.normal,
                });
            }
        }
    }
    best
}

/// One-sided [`MoveAndSlide::move_and_slide`]: avian's sweep, plane collection and velocity
/// projection over the gated faces, without its depenetration; `on_hit` keeps avian's contract.
pub(crate) fn move_and_slide(
    ms: &MoveAndSlide<'_, '_>,
    shape: &Collider,
    shape_position: Vec3,
    mut velocity: Vec3,
    delta_time: Duration,
    config: &MoveAndSlideConfig,
    filter: &SpatialQueryFilter,
    mut on_hit: impl FnMut(MoveAndSlideHitData) -> MoveAndSlideHitResponse,
) -> MoveAndSlideOutput {
    let mut position = shape_position;
    let mut time_left = delta_time.as_secs_f32();
    let skin_width = ms.length_unit.0 * config.skin_width;

    for _ in 0..config.move_and_slide_iterations {
        let sweep = time_left * velocity;
        let Ok((vel_dir, distance)) = Dir3::new_and_length(sweep) else {
            break;
        };
        const MIN_DISTANCE: f32 = 1e-4;
        if distance < MIN_DISTANCE {
            break;
        }

        let Some(sweep_hit) = cast_move(ms, shape, position, sweep, skin_width, filter) else {
            position += sweep;
            break;
        };

        time_left -= time_left * (sweep_hit.distance / distance);
        position += *vel_dir * sweep_hit.distance;

        let mut planes: Vec<Dir3> = config.planes.clone();
        let mut first_normal = Dir3::new_unchecked(sweep_hit.normal1);
        let hit_response = on_hit(MoveAndSlideHitData {
            entity: sweep_hit.entity,
            point: sweep_hit.point2,
            normal: &mut first_normal,
            collision_distance: sweep_hit.collision_distance,
            distance: sweep_hit.distance,
            position: &mut position,
            velocity: &mut velocity,
        });
        if hit_response == MoveAndSlideHitResponse::Accept {
            planes.push(first_normal);
        } else if hit_response == MoveAndSlideHitResponse::Abort {
            break;
        }

        // avian's contact-plane pass, gated with the velocity as the motion.
        let mut aborted = false;
        for_each_contact(
            ms,
            shape,
            position,
            skin_width * 2.0,
            filter,
            velocity,
            (time_left * velocity).length(),
            |entity, point, normal, _dist| {
                let mut normal = Dir3::new_unchecked(normal);
                // Prune nearly-parallel planes, keeping the most blocking version (avian's rule).
                for existing in planes.iter_mut() {
                    if normal.dot(**existing) >= config.plane_similarity_dot_threshold {
                        if normal.dot(velocity) < existing.dot(velocity) {
                            *existing = normal;
                        }
                        return true;
                    }
                }
                if planes.len() >= config.max_planes {
                    return false;
                }
                let hit_response = on_hit(MoveAndSlideHitData {
                    entity,
                    point,
                    normal: &mut normal,
                    collision_distance: sweep_hit.collision_distance,
                    distance: sweep_hit.distance,
                    position: &mut position,
                    velocity: &mut velocity,
                });
                match hit_response {
                    MoveAndSlideHitResponse::Accept => {
                        planes.push(normal);
                        true
                    }
                    MoveAndSlideHitResponse::Ignore => true,
                    MoveAndSlideHitResponse::Abort => {
                        aborted = true;
                        false
                    }
                }
            },
        );

        velocity = MoveAndSlide::project_velocity(velocity, &planes);

        if aborted {
            break;
        }
    }

    MoveAndSlideOutput {
        position,
        projected_velocity: velocity,
    }
}

/// Visits each contact of `shape` within `prediction` for the slide's planes: a triangle counts
/// when its authored normal opposes `motion` and the mover is on its front side, and an overlapped
/// one only inside the band of a `sweep_len` sweep. The callback gets
/// `(entity, point, normal toward the mover, distance, negative when penetrating)` and returns
/// `false` to stop; convex colliders are ungated.
#[allow(clippy::too_many_arguments)]
fn for_each_contact(
    ms: &MoveAndSlide<'_, '_>,
    shape: &Collider,
    position: Vec3,
    prediction: f32,
    filter: &SpatialQueryFilter,
    motion: Vec3,
    sweep_len: f32,
    mut callback: impl FnMut(Entity, Vec3, Vec3, f32) -> bool,
) {
    let Ok(motion_dir) = Dir3::new(motion) else {
        return; // a zero velocity has nothing to clip
    };
    let band = Band::new(shape, position, motion_dir, sweep_len);
    let a = shape.aabb(position, Quat::IDENTITY);
    let grown = ColliderAabb {
        min: a.min - Vec3::splat(prediction),
        max: a.max + Vec3::splat(prediction),
    };
    'outer: for entity in ms.spatial_query.aabb_intersections_with_aabb(grown) {
        let Ok((collider, pos, rot, layers)) = ms.colliders.get(entity) else {
            continue;
        };
        if !filter.test(entity, layers.copied().unwrap_or_default()) {
            continue;
        }

        if let Some(trimesh) = collider.shape_scaled().as_trimesh() {
            let inv_rot = rot.0.inverse();
            let local_pose = Pose3::from_parts(inv_rot * (position - pos.0), inv_rot);
            let local_dir = inv_rot * *motion_dir;
            for tri_id in trimesh
                .bvh()
                .intersect_aabb(&aabb_to_local(grown, pos.0, inv_rot))
            {
                let tri = trimesh.triangle(tri_id);
                let Some(n) = tri.normal() else {
                    continue;
                };
                if n.dot(local_dir) > FACING_EPS {
                    continue;
                }
                let Ok(Some(c)) = contact(
                    &Pose3::IDENTITY,
                    &tri,
                    &local_pose,
                    shape.shape_scaled().as_ref(),
                    prediction,
                ) else {
                    continue;
                };
                // Front side only: behind a face, its plane does not exist for movement.
                if c.normal1.dot(n) <= 0.0 {
                    continue;
                }
                // A face passed beyond the band is no slide plane, as the sweep never reports it.
                if c.dist < 0.0 && !band.keeps_local(&tri, pos.0, rot.0) {
                    continue;
                }
                if !callback(entity, pos.0 + rot.0 * c.point1, rot.0 * c.normal1, c.dist) {
                    break 'outer;
                }
            }
        } else {
            let Ok(Some(c)) = contact(
                &Pose3::from_parts(pos.0, rot.0),
                collider.shape_scaled().as_ref(),
                &Pose3::from_parts(position, Quat::IDENTITY),
                shape.shape_scaled().as_ref(),
                prediction,
            ) else {
                continue;
            };
            if !callback(entity, c.point1, c.normal1, c.dist) {
                break 'outer;
            }
        }
    }
}

/// A plane `n·q + d`, with `q` inside at or below zero, as the reference stores its planes.
#[derive(Clone, Copy)]
struct Plane {
    n: Vec3,
    d: f32,
}

impl Plane {
    fn eval(&self, q: Vec3) -> f32 {
        self.n.dot(q) + self.d
    }

    /// `0x6329e0`: the travel along the unit `dir` after which the plane reaches `q`, negative
    /// behind it; `n·q + d` undivided when `|n·dir|` is below 2^-22.
    fn travel_to(&self, q: Vec3, dir: Vec3) -> f32 {
        let den = self.n.dot(dir);
        if den.abs() < f32::from_bits(0x3480_0000) {
            self.eval(q)
        } else {
            self.eval(q) / den
        }
    }
}

/// A convex polygon of at most 15 vertices, the reference's clip buffer (`0x6318c0`).
#[derive(Clone, Copy)]
struct Poly {
    v: [Vec3; 15],
    len: usize,
}

impl Poly {
    fn triangle(t: [Vec3; 3]) -> Self {
        let mut v = [Vec3::ZERO; 15];
        v[..3].copy_from_slice(&t);
        Self { v, len: 3 }
    }

    fn verts(&self) -> &[Vec3] {
        &self.v[..self.len]
    }

    /// `0x6318c0`: keeps the part inside `plane` with the 1/720 band: wholly inside within the
    /// band is untouched, wholly outside within it is emptied, and a crossing is emitted only
    /// from a vertex more than the band inside. Fewer than three vertices empty it.
    fn clip(&mut self, plane: &Plane) {
        if self.len == 0 {
            return;
        }
        let mut s = [0.0f32; 15];
        for (s, v) in s.iter_mut().zip(self.verts()) {
            *s = -plane.eval(*v);
        }
        let s = &s[..self.len];
        if s.iter().copied().fold(f32::INFINITY, f32::min) > -CLIP_EPS {
            return;
        }
        if s.iter().copied().fold(f32::NEG_INFINITY, f32::max) < CLIP_EPS {
            self.len = 0;
            return;
        }
        let old = self.v;
        let mut out = 0;
        let mut emit = |p: Vec3, out: &mut usize| {
            if *out < 15 {
                self.v[*out] = p;
                *out += 1;
            }
        };
        let cross = |p: usize, c: usize| old[p] + (old[c] - old[p]) * (s[p] / (s[p] - s[c]));
        let mut prev = self.len - 1;
        for cur in 0..self.len {
            // At or above zero is inside, and so is NaN (`0x6319e7`).
            let inside = |s: f32| s.is_nan() || s >= 0.0;
            match (inside(s[prev]), inside(s[cur])) {
                (true, true) => emit(old[cur], &mut out),
                (false, true) => {
                    if s[cur] > CLIP_EPS {
                        emit(cross(prev, cur), &mut out);
                    }
                    emit(old[cur], &mut out);
                }
                (true, false) => {
                    if s[prev] > CLIP_EPS {
                        emit(cross(prev, cur), &mut out);
                    }
                }
                (false, false) => {}
            }
            prev = cur;
        }
        self.len = if out < 3 { 0 } else { out };
    }
}

/// The vertex indices of each volume face, in plane order (`0x631be0`'s index bytes).
const VOLUME_FACES: [&[usize]; 9] = [
    &[1, 2, 6, 5],
    &[3, 4, 8, 7],
    &[2, 3, 7, 6],
    &[4, 1, 5, 8],
    &[5, 6, 7, 8],
    &[0, 1, 2],
    &[0, 3, 4],
    &[0, 2, 3],
    &[0, 4, 1],
];

/// The reference's backface band for one sweep: its moving volume (`0x631440`, `0x631be0`), a box
/// of half-width `r` up to `h` over four 61.6° bevels whose apex is the feet, placed at the
/// capsule's feet with its radius and height, and each leading face's swept prism (`0x632460`,
/// `0x632f80`). Coordinates are relative to the feet, on the world axes.
struct Band {
    planes: [Plane; 9],
    dir: Vec3,
    /// Each leading face's index and prism edge planes; a degenerate prism skips its face.
    leading: [(usize, [Plane; 4], usize); 9],
    count: usize,
    feet: Vec3,
}

impl Band {
    /// The band of `shape` at `center` sweeping `len` along `dir`, floored at 1/36 yd
    /// (`[0x7ff9d0]`) as `0x632ba0` floors its sweep.
    fn new(shape: &Collider, center: Vec3, dir: Dir3, len: f32) -> Self {
        let aabb = shape.aabb(center, Quat::IDENTITY);
        let r = 0.5 * (aabb.max.x - aabb.min.x).max(aabb.max.z - aabb.min.z);
        let feet = Vec3::new(center.x, aabb.min.y, center.z);
        Self::of_volume(feet, r, aabb.max.y - aabb.min.y, *dir, len)
    }

    /// The band of the volume of half-width `r` and height `h` standing at `feet`.
    fn of_volume(feet: Vec3, r: f32, h: f32, dir: Vec3, len: f32) -> Self {
        let s = dir * len.max(BACKFACE_BAND);
        let (a, b, w) = (BEVEL_H, BEVEL_V, r * BEVEL_RISE);
        let plane = |x, y, z, d| Plane {
            n: Vec3::new(x, y, z),
            d,
        };
        // Bevy's axes: up is +Y, so the reference's (x, y, z) is our (x, z, y).
        let planes = [
            plane(-1.0, 0.0, 0.0, -r),
            plane(1.0, 0.0, 0.0, -r),
            plane(0.0, 0.0, 1.0, -r),
            plane(0.0, 0.0, -1.0, -r),
            plane(0.0, 1.0, 0.0, -h),
            plane(-a, -b, 0.0, 0.0),
            plane(a, -b, 0.0, 0.0),
            plane(0.0, -b, a, 0.0),
            plane(0.0, -b, -a, 0.0),
        ];
        let corners = [(-r, -r), (-r, r), (r, r), (r, -r)];
        let mut verts = [Vec3::ZERO; 9];
        for (i, (x, z)) in corners.into_iter().enumerate() {
            verts[1 + i] = Vec3::new(x, w, z);
            verts[5 + i] = Vec3::new(x, h, z);
        }
        let mut leading = [(0, [planes[0]; 4], 0); 9];
        let mut count = 0;
        'faces: for (k, face) in VOLUME_FACES.iter().enumerate() {
            // A face leads when `s·n > 0` (`0x6322ff`, `0x632ddd`).
            if planes[k].n.dot(s) <= 0.0 {
                continue;
            }
            let m = face.len();
            let mut prism = [planes[0]; 4];
            for j in 0..m {
                let (a, b) = (verts[face[j]], verts[face[(j + 1) % m]]);
                let c = (b - a).cross(s);
                if c.length_squared() < f32::from_bits(0x3580_0000) {
                    continue 'faces; // an edge along the motion bounds nothing (`0x6325b5`)
                }
                let n = c.normalize();
                let mut p = Plane { n, d: -n.dot(a) };
                // The vertex off the edge: the triangle's next, the quad's previous (`0x632629`).
                let off = verts[face[if m == 3 { (j + 2) % 3 } else { (j + 3) % 4 }]];
                if p.eval(off) > 0.0 {
                    p = Plane { n: -n, d: -p.d };
                }
                prism[j] = p;
            }
            leading[count] = (k, prism, m);
            count += 1;
        }
        Self {
            planes,
            dir,
            leading,
            count,
            feet,
        }
    }

    /// Whether a facet the body overlaps, its vertices relative to [`Self::feet`], is a contact:
    /// some leading face keeps it (`0x632700`, `0x632830`). The facet is clipped to the face's
    /// prism; a clipped vertex ahead of the face keeps it; else it is clipped by the volume's other
    /// planes, and a vertex left within [`BACKFACE_BAND`] behind the face keeps it.
    ///
    /// Deviation: a facet ahead of the face is kept at any distance, where `0x632700` keeps it only
    /// when its distance is at most the sweep length (`0x6327e2`), reset for each face (`0x632324`,
    /// `0x632e15`), the distance being the least of the clipped vertices' `max(t, 0)`
    /// (`0x632888`-`0x63289a`), because the capsule bulges past the volume's bevels (0.2 yd up, a
    /// half-width of 0.305 against 0.108), so a facet the capsule already overlaps is held as a
    /// contact rather than passed through, as before the clip.
    fn keeps(&self, tri: [Vec3; 3]) -> bool {
        self.leading[..self.count].iter().any(|&(k, prism, m)| {
            let mut poly = Poly::triangle(tri);
            for p in &prism[..m] {
                poly.clip(p);
            }
            let face = &self.planes[k];
            let ahead = |poly: &Poly, bound: f32| {
                poly.verts()
                    .iter()
                    .any(|&v| face.travel_to(v, self.dir) > bound)
            };
            if poly.len == 0 {
                return false;
            }
            if ahead(&poly, BEHIND_EPS) {
                return true;
            }
            for (i, p) in self.planes.iter().enumerate() {
                if i != k {
                    poly.clip(p);
                }
            }
            poly.len > 0 && ahead(&poly, -BACKFACE_BAND)
        })
    }

    /// [`Self::keeps`] for a trimesh-local triangle of a collider at `pos`, `rot`.
    fn keeps_local(&self, tri: &Triangle, pos: Vec3, rot: Quat) -> bool {
        let feet = rot.inverse() * (self.feet - pos);
        self.keeps([tri.a, tri.b, tri.c].map(|v| rot * (v - feet)))
    }
}

/// The local-frame box bounding a world AABB's eight corners.
fn aabb_to_local(aabb: ColliderAabb, pos: Vec3, inv_rot: Quat) -> ParryAabb {
    let (mut mins, mut maxs) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 == 0 { aabb.min.x } else { aabb.max.x },
            if i & 2 == 0 { aabb.min.y } else { aabb.max.y },
            if i & 4 == 0 { aabb.min.z } else { aabb.max.z },
        );
        let local = inv_rot * (corner - pos);
        mins = mins.min(local);
        maxs = maxs.max(local);
    }
    ParryAabb::new(mins, maxs)
}

/// One collision triangle near a point, as the movement law sees it, for the step-up probe.
pub struct FaceProbe {
    pub entity: Entity,
    /// World-space normal at the authored winding, the vector the gate tests.
    pub normal: Vec3,
    /// World-space vertices.
    pub(crate) verts: [Vec3; 3],
}

impl FaceProbe {
    /// Whether this face may block a sweep heading `dir`: [`cast_move`]'s gate.
    pub fn blocks(&self, dir: Vec3) -> bool {
        self.normal.dot(dir) <= FACING_EPS
    }

    /// World centroid, where a face's distance ahead is measured from.
    pub fn centroid(&self) -> Vec3 {
        (self.verts[0] + self.verts[1] + self.verts[2]) / 3.0
    }
}

/// Every trimesh face in the box `at ± half` that `filter` reaches, nearest centroid first. A
/// diagnostic, ungated on purpose: it shows the faces the law rejects beside those it keeps.
pub(crate) fn faces_near(
    ms: &MoveAndSlide<'_, '_>,
    at: Vec3,
    half: Vec3,
    filter: &SpatialQueryFilter,
    limit: usize,
) -> Vec<FaceProbe> {
    let box_aabb = ColliderAabb {
        min: at - half,
        max: at + half,
    };
    let mut out = Vec::new();
    for entity in ms.spatial_query.aabb_intersections_with_aabb(box_aabb) {
        let Ok((collider, pos, rot, layers)) = ms.colliders.get(entity) else {
            continue;
        };
        if !filter.test(entity, layers.copied().unwrap_or_default()) {
            continue;
        }
        let Some(trimesh) = collider.shape_scaled().as_trimesh() else {
            continue;
        };
        let inv_rot = rot.0.inverse();
        for tri_id in trimesh
            .bvh()
            .intersect_aabb(&aabb_to_local(box_aabb, pos.0, inv_rot))
        {
            let tri = trimesh.triangle(tri_id);
            let Some(n) = tri.normal() else {
                continue;
            };
            out.push(FaceProbe {
                entity,
                normal: rot.0 * n,
                verts: [tri.a, tri.b, tri.c].map(|v| pos.0 + rot.0 * v),
            });
        }
    }
    out.sort_by(|a, b| {
        a.centroid()
            .distance_squared(at)
            .total_cmp(&b.centroid().distance_squared(at))
    });
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// One 10×10 quad at y = 0, wound as a floor (+Y) when `up`, else as a shell face (−Y).
    fn world_with_quad(up: bool) -> App {
        let mut app = App::new();
        // avian's collider backend reads `Assets<Mesh>` and `SceneSpawner` even with no meshes.
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>();
        let verts = vec![
            Vec3::new(-5.0, 0.0, -5.0),
            Vec3::new(5.0, 0.0, -5.0),
            Vec3::new(5.0, 0.0, 5.0),
            Vec3::new(-5.0, 0.0, 5.0),
        ];
        let tris = if up {
            vec![[0u32, 2, 1], [0, 3, 2]]
        } else {
            vec![[0u32, 1, 2], [0, 2, 3]]
        };
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::trimesh(verts, tris),
            Transform::default(),
        ));
        // One frame builds Position/Rotation and the spatial-query trees.
        app.update();
        app
    }

    fn capsule() -> Collider {
        Collider::capsule(0.4, 1.0)
    }

    fn world_with_mesh(verts: Vec<Vec3>, tris: Vec<[u32; 3]>) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>();
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::trimesh(verts, tris),
            Transform::default(),
        ));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    fn sweep(app: &mut App, from: Vec3, movement: Vec3) -> Option<f32> {
        app.world_mut()
            .run_system_once(move |ms: MoveAndSlide| {
                cast_move(
                    &ms,
                    &capsule(),
                    from,
                    movement,
                    0.02,
                    &SpatialQueryFilter::default(),
                )
                .map(|h| h.collision_distance)
            })
            .unwrap()
    }

    /// The capsule is 1.8 tall (radius 0.4, segment 1.0), its centre 0.9 above its feet.
    #[test]
    fn the_band_keeps_what_the_volume_reaches_and_drops_a_passed_face() {
        // A 0.6 × 0.6 seat top at y = 0.5, up-wound; the body's feet at y = 0 on its centre.
        let seat = world_with_mesh(
            vec![
                Vec3::new(-0.3, 0.5, -0.3),
                Vec3::new(0.3, 0.5, -0.3),
                Vec3::new(0.3, 0.5, 0.3),
                Vec3::new(-0.3, 0.5, 0.3),
            ],
            vec![[0u32, 2, 1], [0, 3, 2]],
        );
        let centre = Vec3::new(0.0, 0.9, 0.0);
        let mut seat = seat;
        assert_eq!(
            sweep(&mut seat, centre, Vec3::X * 0.2),
            None,
            "a seat top is square to the walk (n·dir = 0, above −1e-5): the facing gate drops it"
        );

        // A 10-yd slope rising 1 yd (about 6°), the feet 5 cm under it, one vertex well below.
        let mut slope = world_with_mesh(
            vec![
                Vec3::new(-5.0, -0.5, -5.0),
                Vec3::new(5.0, 0.5, -5.0),
                Vec3::new(5.0, 0.5, 5.0),
                Vec3::new(-5.0, -0.5, 5.0),
            ],
            vec![[0u32, 2, 1], [0, 3, 2]],
        );
        let feet_under = Vec3::new(0.0, 0.9 - 0.05, 0.0);
        assert_eq!(
            sweep(&mut slope, feet_under, Vec3::NEG_Y * 0.2),
            Some(0.0),
            "a sloped floor the feet sit 5 cm under still supports: a vertex of it is below them"
        );

        let mut flat = world_with_mesh(
            vec![
                Vec3::new(-5.0, 0.0, -5.0),
                Vec3::new(5.0, 0.0, -5.0),
                Vec3::new(5.0, 0.0, 5.0),
                Vec3::new(-5.0, 0.0, 5.0),
            ],
            vec![[0u32, 2, 1], [0, 3, 2]],
        );
        assert_eq!(
            sweep(&mut flat, feet_under, Vec3::NEG_Y * 0.2),
            Some(0.0),
            "a flat floor 5 cm above the feet is still ahead of the bevels beside them"
        );
        // The bevels rise 1.849 per yard out from the feet, so a floor this far up the body is
        // still ahead of them at its edges.
        let deep = Vec3::new(0.0, 0.9 - 0.3, 0.0);
        assert_eq!(
            sweep(&mut flat, deep, Vec3::NEG_Y * 0.2),
            Some(0.0),
            "a flat floor 0.3 yd above the feet is a hit at t = 0"
        );
    }

    /// A wall leaning and slanting across the motion, passed within the body's cross-section by
    /// more than the band: 1.12.1 measures only the part inside the swept body, so the far end
    /// ahead of the body does not hold it.
    #[test]
    fn the_band_measures_only_the_part_of_a_face_inside_the_body() {
        let wall = |y: f32, z: f32| Vec3::new(-0.05 + 0.2 * y + 0.1 * z, y, z);
        // Wound to face −x, against the motion: normal ∝ (−1, 0.2, 0.1).
        let mut slant = world_with_mesh(
            vec![
                wall(-1.0, -10.0),
                wall(-1.0, 10.0),
                wall(3.0, 10.0),
                wall(3.0, -10.0),
            ],
            vec![[0u32, 1, 2], [0, 2, 3]],
        );
        assert_eq!(
            sweep(&mut slant, Vec3::new(0.0, 0.9, 0.0), Vec3::X * 0.2),
            None,
            "the face is behind every leading face inside the body: the walk goes on through it"
        );
    }

    #[test]
    fn a_floor_blocks_a_fall_and_a_backface_does_not() {
        for (up, expect_block) in [(true, true), (false, false)] {
            let hit = world_with_quad(up)
                .world_mut()
                .run_system_once(|ms: MoveAndSlide| {
                    cast_move(
                        &ms,
                        &capsule(),
                        Vec3::new(0.0, 3.0, 0.0),
                        Vec3::NEG_Y * 5.0,
                        0.05,
                        &SpatialQueryFilter::default(),
                    )
                })
                .unwrap();
            assert_eq!(
                hit.is_some(),
                expect_block,
                "winding up={up}: reference {} on this face",
                if expect_block {
                    "blocks"
                } else {
                    "falls through"
                }
            );
            if let Some(h) = hit {
                // The capsule's bottom is 0.9 below its centre: contact after 2.1 yd.
                assert!((h.collision_distance - 2.1).abs() < 1e-3);
                assert!(h.normal1.y > 0.99);
            }
        }
    }

    #[test]
    fn a_floor_is_no_ceiling_from_below() {
        // From below, an up-wound floor is a backface and a down-wound quad blocks.
        for (up, expect_block) in [(true, false), (false, true)] {
            let hit = world_with_quad(up)
                .world_mut()
                .run_system_once(|ms: MoveAndSlide| {
                    cast_move(
                        &ms,
                        &capsule(),
                        Vec3::new(0.0, -3.0, 0.0),
                        Vec3::Y * 5.0,
                        0.05,
                        &SpatialQueryFilter::default(),
                    )
                })
                .unwrap();
            assert_eq!(hit.is_some(), expect_block, "winding up={up} from below");
        }
    }

    #[test]
    fn the_slide_falls_through_a_backface_and_rests_on_a_floor() {
        for (up, expect_above) in [(true, true), (false, false)] {
            let end = world_with_quad(up)
                .world_mut()
                .run_system_once(|ms: MoveAndSlide| {
                    move_and_slide(
                        &ms,
                        &capsule(),
                        Vec3::new(0.0, 3.0, 0.0),
                        Vec3::NEG_Y * 10.0,
                        Duration::from_secs(1),
                        &MoveAndSlideConfig::default(),
                        &SpatialQueryFilter::default(),
                        |_| MoveAndSlideHitResponse::Accept,
                    )
                    .position
                })
                .unwrap();
            if expect_above {
                assert!((end.y - 0.9).abs() < 0.1, "rests on the floor, got {end}");
            } else {
                assert!(end.y < -6.0, "falls straight through, got {end}");
            }
        }
    }
}
