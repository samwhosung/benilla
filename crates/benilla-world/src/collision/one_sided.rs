//! **One-sided movement collision** — the reference's facing law, applied at the only seam where it
//! can live: per candidate triangle, inside the sweep.
//!
//! The real client's movement collision is an orientation-blind gather followed by a strictly
//! one-sided resolver: `0x671cc0` emits every candidate face's plane at the **unflipped file
//! winding**, and `0x632700` opens each record with `dot = n·dir` against `[0x80c5c4]`
//! (`0xb727c5ac` = −9.99999975e-6) — **a face is processed iff `n·dir ≤ −1e-5`**. A face approached
//! from its back is discarded before any distance is computed, and this is not a wall-slide special
//! case: the 16 callers of `0x632ba0 earliest_contact` span falling, walking, step-up, ground-settle,
//! transports and the water-surface arm — "the surface you stand on is filtered exactly like the wall
//! you slide along" (confirmed
//! at B86's exact pin in decision 0968). parry's trimesh is two-sided by construction, which is why
//! benilla stood on CoT's inward-wound shell where 1.12.1 falls through it.
//!
//! Why this is a mirrored loop and not a post-filter: a shape cast reports only the *first* hit. If
//! that hit is a backface, the correct result is the next front-face hit *behind* it — information a
//! finished cast no longer has. So the gate must run where candidates are enumerated, and avian's
//! enumeration (`cast_shape_predicate`, `contact_manifolds`) carries no triangle identity out. This
//! module therefore re-runs avian's own `move_and_slide` algorithm — same iteration structure, same
//! skin-width pull-back, same depenetration solver (`depenetrate_intersections`), same velocity
//! projection — over its public building blocks, walking each trimesh's BVH itself so every triangle
//! is gated on its **authored winding** before it may block. `Collider::trimesh` stores vertices and
//! indices exactly as passed (parry `TriMeshFlags::empty()`), and every transform between the file
//! and the collider is a proper rotation, so `TriMesh::triangle(i)`'s winding *is*
//! the authored winding.
//!
//! What is ported is the **law**, not the reference's resolver: the slide/step/snap machinery stays
//! the standard kinematic controller (that direction was closed in 0207), contact normals for edge
//! hits stay parry's, and convex (non-trimesh) colliders stay whole-shape — a convex volume has no
//! reachable backface. The camera/LOS path is untouched *on purpose*: the reference's segment
//! kernel `0x7c29f0` is two-sided, so avian's ordinary cast is already faithful there.
//!
//! **There is no depenetration pass, and that is the law too**. avian's
//! `move_and_slide` opens and closes with a Gauss–Seidel push-out of every face the shape overlaps;
//! the reference's resolver has no such stage anywhere in `0x634040`'s closure. Its world query is a
//! time-of-impact sweep and nothing else: a face the prism starts *behind* by more than 1/36 yd
//! (`[0x7ff9c8]`, the backface band in `0x632830`'s clip) is not a hit at all, one it straddles is a
//! hit at `t = 0` that the slide projects against, and neither ever *moves* a body that asked to go
//! nowhere. The push-out was benilla's, and it is what shoved a seated body sideways off its stool:
//! the server seats a player at a chair's origin, inside the chair's own collision box, and the
//! minimum-translation vector for a tall capsule inside a small box is horizontal — out through the
//! nearest side (measured at 0.555 yd on the first frame, with no input). A body that starts
//! inside geometry leaves it the way the reference does: by walking out through the faces wound
//! away from it, which the sweep never counts.

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

/// The reference's facing gate: `[0x80c5c4] = 0xb727c5ac`. A face may block iff `n·dir ≤ EPS`,
/// with `n` the authored winding normal and `dir` the unit direction of the motion being resolved.
const FACING_EPS: f32 = f32::from_bits(0xb727_c5ac);

/// avian's own stabilizer for `pull_back` when `n·dir` is nearly zero (`move_and_slide.rs`).
const DOT_EPSILON: f32 = 0.005;

/// The reference's **backface band**: `[0x7ff9c8]` = `1/36` yd. A candidate face the mover has
/// already passed by more than this is not a hit — `0x632830`'s clip
/// rejects a face **every vertex of which** sits further than the
/// band behind the prism's leading plane along the motion, and keeps one with any vertex nearer
/// than that as a hit at `t = 0`. It is what makes the resolver a *sweep* and not a solver: a body
/// the server placed inside a hull (a seated player at a chair's origin, a caged creature) meets
/// faces it already straddles, and the band says which of those still count — a seat top half a
/// yard into the body, every vertex of it behind the body's leading edge, does not exist; a sloped
/// terrain triangle a few centimetres into the feet, one vertex of which still lies below them,
/// does. **Measured against the vertices, not the penetration depth**: the first
/// port used parry's contact depth, and on a 19° slope under a stool that read the terrain as
/// behind the feet — the body fell 26 yd through the world. Ported at this seam, like the facing
/// law, because it can only be applied where the *overlapping* triangle is known: avian's own
/// sweep skips every origin-penetrating hit (`ignore_origin_penetration`) and leaves the overlap to
/// the depenetration pass this module no longer runs.
const BACKFACE_BAND: f32 = 1.0 / 36.0;

/// One-sided drop-in for [`MoveAndSlide::cast_move`]: sweep `shape` (world-axis-aligned, as every
/// mover capsule is) along `movement`, stopping `skin_width` short of the first face whose
/// **authored winding opposes the motion**. Backfaces are not candidates at all — the sweep passes
/// through them to whatever front-face lies beyond, which no post-filter on a first-hit cast can do.
pub(crate) fn cast_move(
    ms: &MoveAndSlide<'_, '_>,
    shape: &Collider,
    from: Vec3,
    movement: Vec3,
    skin_width: f32,
    filter: &SpatialQueryFilter,
) -> Option<MoveHitData> {
    let (dir, len) = Dir3::new_and_length(movement).unwrap_or((Dir3::X, 0.0));
    let max_toi = len + skin_width;

    // The swept broad-phase box: the shape's AABB at both ends of the motion, grown by the skin.
    let a0 = shape.aabb(from, Quat::IDENTITY);
    let a1 = shape.aabb(from + movement, Quat::IDENTITY);
    let swept = ColliderAabb {
        min: a0.min.min(a1.min) - Vec3::splat(skin_width),
        max: a0.max.max(a1.max) + Vec3::splat(skin_width),
    };

    let mut best: Option<MoveHitData> = None;
    let mut best_toi = max_toi;
    for entity in ms.spatial_query.aabb_intersections_with_aabb(swept) {
        let Ok((collider, pos, rot, layers)) = ms.colliders.get(entity) else {
            continue;
        };
        if !filter.test(entity, layers.copied().unwrap_or_default()) {
            continue;
        }

        if let Some(trimesh) = collider.shape_scaled().as_trimesh() {
            // Work in the trimesh's local frame: the cast pose and direction come in, world hit
            // data goes out. A proper rotation preserves the sign of every `n·dir`.
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
                    continue; // degenerate face: no winding, no plane, nothing to block with
                };
                if n.dot(local_dir) > FACING_EPS {
                    continue; // the law: a face approached from its back is not a candidate
                }
                let Ok(Some(hit)) = cast_shapes(
                    &Pose3::IDENTITY,
                    Vec3::ZERO,
                    &tri,
                    &local_pose,
                    local_dir,
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
                // An overlap at `t = 0` is a hit only inside the band: a face the body has passed
                // deeper than that is behind it, whatever its winding says about the motion.
                if hit.status == ShapeCastStatus::PenetratingOrWithinTargetDist
                    && behind_the_band(&tri, &local_pose, shape, local_dir)
                {
                    continue;
                }
                if hit.time_of_impact < best_toi {
                    best_toi = hit.time_of_impact;
                    best = Some(MoveHitData {
                        entity,
                        distance: 0.0, // pulled back below, once, for the winner
                        point1: pos.0 + rot.0 * hit.witness1,
                        point2: pos.0 + rot.0 * (hit.witness2 + local_dir * hit.time_of_impact),
                        normal1: rot.0 * hit.normal1,
                        normal2: rot.0 * hit.normal2,
                        collision_distance: hit.time_of_impact,
                    });
                }
            }
        } else {
            // A convex collider has no reachable backface: whole-shape sweep, exactly avian's.
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
                best = Some(MoveHitData {
                    entity,
                    distance: 0.0,
                    point1: pos.0 + rot.0 * hit.witness1,
                    point2: from + hit.witness2 + *dir * hit.time_of_impact,
                    normal1: rot.0 * hit.normal1,
                    normal2: hit.normal2,
                    collision_distance: hit.time_of_impact,
                });
            }
        }
    }

    best.map(|mut hit| {
        // avian's skin-width pull-back: stop short of the surface by `skin / |n·dir|`, never negative.
        hit.distance = if max_toi == 0.0 {
            0.0
        } else {
            let dot = dir.dot(-hit.normal1).max(DOT_EPSILON);
            (hit.collision_distance - skin_width / dot).max(0.0)
        };
        hit
    })
}

/// One-sided drop-in for [`SpatialQuery::cast_ray`], for the movement-law probes that are rays
/// rather than shape sweeps (the creature ground clamp mirrors the WALK resolver's Z re-derivation,
/// and that resolver filters its down-probe like every other arm — 0967's caller census). `dir`
/// must be a unit direction; the gate and the semantics are [`cast_move`]'s, minus the skin.
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

/// One-sided drop-in for [`MoveAndSlide::move_and_slide`]: avian's algorithm — sweep, collect
/// contact planes, project velocity, repeat — with every stage running over the gated candidate set
/// instead of the two-sided trimesh, and **without** avian's opening and closing depenetration
/// (see the module note: the reference's resolver has no push-out stage, and the one this carried
/// is what displaced a seated body out of its chair). The `on_hit` callback contract is avian's
/// ([`MoveAndSlideHitData`] / [`MoveAndSlideHitResponse`]), so the mover's ride/steep-wall handlers
/// move over unchanged.
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

        // Collect nearby contact planes for velocity clipping — avian's `intersections` pass, per
        // gated triangle. The gate here is the same law with the *velocity* as the motion: a face
        // may clip the slide iff its authored winding opposes where we are going AND we sit on its
        // front side (behind a face, its plane does not exist for us — that is the fall-through).
        let mut aborted = false;
        for_each_contact(
            ms,
            shape,
            position,
            skin_width * 2.0,
            filter,
            velocity,
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

/// Visit every contact of `shape` (world-axis-aligned) against the filtered world within
/// `prediction`, gated by the one-sided law with `motion` as the direction being resolved — the
/// slide's plane collection: a triangle participates iff its authored normal opposes the velocity
/// (`n·v̂ ≤ EPS`) *and* the mover sits on its front side (behind a face, its plane does not exist
/// for movement).
///
/// The callback receives `(entity, world contact point, world contact normal toward the mover,
/// contact distance — negative when penetrating)` and returns `false` to stop the visit. Convex
/// colliders are visited ungated.
fn for_each_contact(
    ms: &MoveAndSlide<'_, '_>,
    shape: &Collider,
    position: Vec3,
    prediction: f32,
    filter: &SpatialQueryFilter,
    motion: Vec3,
    mut callback: impl FnMut(Entity, Vec3, Vec3, f32) -> bool,
) {
    let Ok(motion_dir) = Dir3::new(motion) else {
        return; // a zero velocity has nothing to clip
    };
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
                // Front-side gate: the contact must push the mover along the authored normal.
                // Behind the face (`c.normal1 ≈ −n`) the plane does not exist for movement.
                if c.normal1.dot(n) <= 0.0 {
                    continue;
                }
                // The backface band (see [`BACKFACE_BAND`]): a face the body has already passed
                // is not a plane to slide along — the sweep does not count it, and a slide plane
                // the sweep never reported would clip the body against geometry it is meant to
                // walk straight out of.
                if c.dist < 0.0 && behind_the_band(&tri, &local_pose, shape, local_dir) {
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

/// Has the mover already passed this triangle by more than the [`BACKFACE_BAND`]? The reference's
/// own measure: every vertex of the face against the mover's **leading extent along the motion**
/// (`0x632830`'s per-vertex plane test against the prism's leading plane). The leading extent is
/// the shape's support point along `dir` — for the capsule, the far segment end plus the radius.
/// A shape with no support map (none on the movement audience) is never behind: the hit stands.
fn behind_the_band(tri: &Triangle, shape_pose: &Pose3, shape: &Collider, dir: Vec3) -> bool {
    let Some(support) = shape.shape_scaled().as_support_map() else {
        return false;
    };
    let lead = support.support_point(shape_pose, dir).dot(dir);
    let ahead = [tri.a, tri.b, tri.c]
        .into_iter()
        .map(|v| v.dot(dir) - lead)
        .fold(f32::MIN, f32::max);
    ahead < -BACKFACE_BAND
}

/// The conservative local-frame box of a world AABB under `world→local` = `inv_rot · (x − pos)`:
/// transform the eight corners and take their extent.
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

/// One collision triangle near a point, as the **movement law** sees it — the geometry readout
/// behind the step-up probe ([`crate::player::step_probe`]).
pub struct FaceProbe {
    /// The collider it belongs to (a WMO walk bake, a terrain tile, a doodad hull…).
    pub entity: Entity,
    /// World-space normal at the **authored winding** — the vector [`cast_move`]'s gate tests.
    pub normal: Vec3,
    /// World-space vertices.
    pub(crate) verts: [Vec3; 3],
}

impl FaceProbe {
    /// Whether this face may block a sweep heading `dir` — [`cast_move`]'s gate, exactly.
    pub fn blocks(&self, dir: Vec3) -> bool {
        self.normal.dot(dir) <= FACING_EPS
    }

    /// World centroid — what "how far ahead is this face" is measured from.
    pub fn centroid(&self) -> Vec3 {
        (self.verts[0] + self.verts[1] + self.verts[2]) / 3.0
    }
}

/// Every trimesh face inside the box `at ± half` that `filter` can reach, nearest centroid first.
///
/// **Diagnostic only, and deliberately UNGATED**: it reports the faces the one-sided law *rejects*
/// alongside the ones it keeps, which is the entire point of having it. "The kerb top is authored
/// downward-facing, so our settle probe passes straight through it" and "the advance was too short
/// to reach the tread" produce the same silent non-step from the outside, and opposite readings
/// here. Convex (non-trimesh) colliders have no per-face winding to report and are skipped — the
/// law leaves them whole-shape too.
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

    /// A physics world holding one 10×10 quad at y = 0 with the given winding — `up: true` is a
    /// floor (authored normal +Y), `up: false` is 0968's shell face (authored normal −Y).
    fn world_with_quad(up: bool) -> App {
        let mut app = App::new();
        // avian's collider backend reads `Assets<Mesh>` and `SceneSpawner` even in a meshless
        // world, so the headless asset/scene plugins ride along.
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

    /// A headless world holding one up-wound trimesh built from `(verts, tris)`.
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

    /// Sweep the test capsule from `from` by `movement` and report the hit distance, `None` on a miss.
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

    /// **The backface band, both faces of it**. The capsule here is 1.8 tall
    /// (radius 0.4, segment 1.0), its centre 0.9 above its feet.
    ///
    /// A small horizontal face the body straddles — a seat top 0.5 yd into it, every vertex behind
    /// the body's leading edge along a horizontal walk — is not a hit: the body walks out of the
    /// chair. A large **sloped** face the feet sit 5 cm under — the terrain under a stool — is
    /// still a hit for a downward probe, because a vertex of it lies below the feet; the first port
    /// measured parry's penetration depth instead and dropped the floor, and the body fell through
    /// the world. And a **flat** face every vertex of which is more than the band above the feet
    /// is gone, which is the reference's own answer for a body placed under flat ground.
    #[test]
    fn the_band_keeps_a_straddled_slope_and_drops_a_passed_face() {
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
            "a seat top the body straddles is behind its leading edge: the walk out is free"
        );

        // A 10-yd terrain triangle rising 1 yd across its span (a ~6° slope), the feet 5 cm
        // under its plane at the body's own x — but one vertex of it lies well below the feet.
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

        // The same feet 5 cm under a FLAT floor: every vertex more than the band behind — gone.
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
            None,
            "a flat floor more than 1/36 yd above the feet has been passed"
        );
        // …and 2 cm under it — inside the band — is still standing on it.
        let feet_just_under = Vec3::new(0.0, 0.9 - 0.02, 0.0);
        assert_eq!(
            sweep(&mut flat, feet_just_under, Vec3::NEG_Y * 0.2),
            Some(0.0),
            "a floor within the band is a hit at t = 0"
        );
    }

    #[test]
    fn a_floor_blocks_a_fall_and_a_backface_does_not() {
        // The whole bug in one assertion pair: the same quad, same cast, opposite winding.
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
                // Capsule bottom = center − (0.5 + 0.4); surface at 0 ⇒ contact after ~2.1 yd,
                // pulled back by the skin.
                assert!((h.collision_distance - 2.1).abs() < 1e-3);
                assert!(h.normal1.y > 0.99);
            }
        }
    }

    #[test]
    fn a_floor_is_no_ceiling_from_below() {
        // Rising through an up-wound floor from beneath: its backface must not bump the head —
        // and the down-wound quad, approached from below, is the face that must.
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
        // The full resolve, not just the cast: one second of straight fall through/onto the quad.
        // Backface: every stage — sweep and contact planes — must let the body pass.
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
