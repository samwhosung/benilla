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
//! facing law and the backface band, are ported.
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

/// The facing gate (`[0x80c5c4]`, −1e-5): a face blocks only when `n·dir` is at most this, `n` its
/// authored normal and `dir` the unit motion.
const FACING_EPS: f32 = f32::from_bits(0xb727_c5ac);

/// avian's `pull_back` floor for a near-zero `n·dir` (its `move_and_slide.rs`).
const DOT_EPSILON: f32 = 0.005;

/// The backface band (`[0x7ff9c8]`, 1/36 yd): `0x632830`'s clip drops a face whose every vertex
/// lies more than this behind the mover's leading plane along the motion, and keeps one with any
/// vertex nearer as a hit at `t = 0`. Measured on the vertices: a penetration depth would drop a
/// sloped floor just above the feet, and the body would fall through the world.
const BACKFACE_BAND: f32 = 1.0 / 36.0;

/// One-sided [`MoveAndSlide::cast_move`]: sweeps the axis-aligned `shape` along `movement` to
/// `skin_width` short of the first face whose authored winding opposes the motion.
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
                        max_time_of_impact: best_toi,
                        target_distance: 0.0,
                        stop_at_penetration: false,
                        compute_impact_geometry_on_penetration: true,
                    },
                ) else {
                    continue;
                };
                // An overlap at `t = 0` is a hit only inside the band.
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
        // avian's skin-width pull-back: `skin / |n·dir|` short of the surface, never negative.
        hit.distance = if max_toi == 0.0 {
            0.0
        } else {
            let dot = dir.dot(-hit.normal1).max(DOT_EPSILON);
            (hit.collision_distance - skin_width / dot).max(0.0)
        };
        hit
    })
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
/// when its authored normal opposes `motion` and the mover is on its front side. The callback gets
/// `(entity, point, normal toward the mover, distance, negative when penetrating)` and returns
/// `false` to stop; convex colliders are ungated.
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
                // Front side only: behind a face, its plane does not exist for movement.
                if c.normal1.dot(n) <= 0.0 {
                    continue;
                }
                // A face passed beyond the band is no slide plane, as the sweep never reports it.
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

/// Whether every vertex lies more than [`BACKFACE_BAND`] behind the shape's support point along
/// `dir`, `0x632830`'s per-vertex test.
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
            None,
            "a flat floor more than 1/36 yd above the feet has been passed"
        );
        let feet_just_under = Vec3::new(0.0, 0.9 - 0.02, 0.0);
        assert_eq!(
            sweep(&mut flat, feet_just_under, Vec3::NEG_Y * 0.2),
            Some(0.0),
            "a floor within the band is a hit at t = 0"
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
