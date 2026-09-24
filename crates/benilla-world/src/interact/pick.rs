//! The shared ray caster: the pick-geometry declarations and the triangle-accurate cast. It casts
//! against resident geometry ([`PickMesh`]) because the render meshes are `RENDER_WORLD`-only, and
//! pick geometry is declared, never inferred from a bound.

use std::sync::Arc;

use benilla_assets::coords::wow_to_bevy;
use benilla_formats::RenderSubmesh;
use bevy::camera::primitives::Aabb;
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use super::WorldObject;

/// The resident pick geometry of one drawn batch, shared with the model asset; the render mesh is
/// `RENDER_WORLD`-only. A keyed entity with neither this nor [`PickBox`] is not pickable.
#[derive(Component, Clone)]
pub struct PickMesh(pub Arc<RenderSubmesh>);

/// Declares that the entity's `Aabb` is its pick geometry: the model-less fallback cube, whose
/// cuboid is its bound. Declared, not inferred from a missing [`PickMesh`], because a WMO's MLIQ
/// pool is a drawn, identified mesh with no resident geometry whose bound spans its whole grid.
#[derive(Component, Clone, Copy)]
pub struct PickBox;

/// One placement's batch inside a consolidated draw: its identity, resident geometry and world
/// pose, so the pick names the placement under the cursor rather than the draw.
#[derive(Clone)]
pub struct PickMember {
    /// One allocation per placement, shared by its batches.
    pub object: Arc<WorldObject>,
    pub geometry: Arc<RenderSubmesh>,
    /// The member's world pose (its placement transform), not relative to the draw's entity.
    pub transform: Transform,
}

/// The members of a blob, a consolidated draw that is still one entity (the static merge). The
/// entity keeps the broad phase (its union `Aabb`, its `ViewVisibility`); the members are the
/// narrow phase and the identity. A blob carries no [`PickMesh`], whose hit would name the blob.
#[derive(Component, Clone)]
pub struct PickBlob(pub Arc<[PickMember]>);

/// One hit from the identified cast, its identity resolved here since most of the static world has
/// no entity; `entity` is `Some` only when an entity owns the geometry.
pub struct ObjectHit {
    /// `None` only for an entity picked without a world identity (an equipped item's part).
    pub object: Option<WorldObject>,
    pub entity: Option<Entity>,
    pub hit: RayHit,
}

/// Geometry drawn with no entity to hang a [`PickMesh`] on ([`crate::static_gx`]). The lane answers
/// the ray itself, since only it knows what it drew, and reports only content it selected.
pub trait PickSource {
    /// Append the nearest hit per member along `ray`; `all_hits` asks for the whole ray.
    fn cast_objects(&self, ray: Ray3d, all_hits: bool, out: &mut Vec<(Arc<WorldObject>, RayHit)>);
}

/// One triangle-accurate hit; the normal follows the authored winding, not the ray.
pub struct RayHit {
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
}

/// The query every mesh picker casts against.
pub type PickParts<'w, 's> = Query<
    'w,
    's,
    (
        Option<&'static PickMesh>,
        &'static GlobalTransform,
        &'static ViewVisibility,
        Option<&'static Aabb>,
        Has<PickBox>,
        Option<&'static PickBlob>,
    ),
>;

/// The nearest hit among `pickable` under the logical `cursor`: `(entity, world point, distance)`.
/// Only `pickable` can be hit, so a caller that passes only unit meshes clicks through a doodad.
pub fn pick_at_cursor(
    cursor: Vec2,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    pickable: &HashSet<Entity>,
    parts: &PickParts,
) -> Option<(Entity, Vec3, f32)> {
    let ray = camera.viewport_to_world(cam_tf, cursor).ok()?;
    cast_pick_ray(ray, pickable, parts, false)
        .into_iter()
        .next()
        .map(|(e, hit)| (e, hit.point, hit.distance))
}

/// The identified cast: the entity pick set (parts, boxes, blob members) and every entity-less
/// [`PickSource`], each culled by its own machinery, cast apart and merged nearest-first.
pub fn cast_object_ray(
    ray: Ray3d,
    pickable: &HashSet<Entity>,
    parts: &PickParts,
    objects: &Query<&WorldObject>,
    sources: &[&dyn PickSource],
    all_hits: bool,
) -> Vec<ObjectHit> {
    let mut out: Vec<ObjectHit> = cast_pick_ray_impl(ray, pickable, parts, all_hits, false)
        .into_iter()
        .map(|(entity, member, hit)| match member {
            // A blob member: its identity travels with the hit, and the blob entity is dropped.
            Some(object) => ObjectHit {
                object: Some((*object).clone()),
                entity: None,
                hit,
            },
            None => ObjectHit {
                object: objects.get(entity).ok().cloned(),
                entity: Some(entity),
                hit,
            },
        })
        .collect();
    let mut lane: Vec<(Arc<WorldObject>, RayHit)> = Vec::new();
    for source in sources {
        source.cast_objects(ray, all_hits, &mut lane);
    }
    out.extend(lane.into_iter().map(|(object, hit)| ObjectHit {
        object: Some((*object).clone()),
        entity: None,
        hit,
    }));
    out.sort_unstable_by(|a, b| a.hit.distance.total_cmp(&b.hit.distance));
    if !all_hits {
        out.truncate(1);
    }
    out
}

/// [`cast_object_ray`] through a screen pixel.
pub fn pick_object_at_cursor(
    cursor: Vec2,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    pickable: &HashSet<Entity>,
    parts: &PickParts,
    objects: &Query<&WorldObject>,
    sources: &[&dyn PickSource],
) -> Option<ObjectHit> {
    let ray = camera.viewport_to_world(cam_tf, cursor).ok()?;
    cast_object_ray(ray, pickable, parts, objects, sources, false)
        .into_iter()
        .next()
}

/// Cast `ray` against `pickable`'s resident geometry, nearest-first: one hit per entity (its
/// nearest triangle), all of them with `all_hits`, else only the front one. Triangles test
/// two-sided, and an entity with neither [`PickMesh`] nor [`PickBox`] is never hit.
pub fn cast_pick_ray(
    ray: Ray3d,
    pickable: &HashSet<Entity>,
    parts: &PickParts,
    all_hits: bool,
) -> Vec<(Entity, RayHit)> {
    cast_pick_ray_impl(ray, pickable, parts, all_hits, false)
        .into_iter()
        .map(|(e, _, hit)| (e, hit))
        .collect()
}

/// [`cast_pick_ray`]'s generous second pass, mouse pick only (the reference's resolve `0x7089c0`,
/// pass 2): every vertex moves out along its authored normal, raw, a halo of 1 model-unit times
/// the part's scale; a part without normals stays exact-only, and a [`PickBox`] grows by the same
/// unit. Returns every hit, for the caller to rank by the reference's pass-2 priority, not
/// distance. Deviation: the broad bound is the mesh `Aabb` grown by 1 unit, not the reference's
/// sequence-bounds sphere, because it can only err permissive.
pub fn cast_pick_ray_inflated(
    ray: Ray3d,
    pickable: &HashSet<Entity>,
    parts: &PickParts,
) -> Vec<(Entity, RayHit)> {
    cast_pick_ray_impl(ray, pickable, parts, true, true)
        .into_iter()
        .map(|(e, _, hit)| (e, hit))
        .collect()
}

/// The entity walk; a blob hit carries its member's identity ([`PickBlob`]).
fn cast_pick_ray_impl(
    ray: Ray3d,
    pickable: &HashSet<Entity>,
    parts: &PickParts,
    all_hits: bool,
    inflate: bool,
) -> Vec<(Entity, Option<Arc<WorldObject>>, RayHit)> {
    let (origin, dir) = (ray.origin, *ray.direction);
    let mut candidates: Vec<(f32, Entity)> = Vec::new();
    for &entity in pickable {
        let Ok((mesh, gt, vis, aabb, is_box, blob)) = parts.get(entity) else {
            continue;
        };
        if !vis.get() {
            continue; // not drawn in any view this frame
        }
        // A blob's authored bound (`NoAutoAabb`) is its members' union; a blob with no bound is
        // narrow-tested unconditionally, like a bound-less part.
        let entry = match (aabb, mesh, is_box, blob) {
            (Some(aabb), _, _, Some(_)) => {
                let bound = if inflate {
                    Aabb {
                        center: aabb.center,
                        half_extents: aabb.half_extents + bevy::math::Vec3A::ONE,
                    }
                } else {
                    *aabb
                };
                let (min, max) = world_aabb(&bound, gt);
                match ray_aabb(origin, dir, min, max) {
                    Some(t) => t,
                    None => continue,
                }
            }
            (None, _, _, Some(_)) => 0.0,
            (Some(aabb), Some(_), _, None) | (Some(aabb), None, true, None) => {
                // The halo moves a vertex at most 1 model-unit in local space, so the bound grown
                // by that never clips it.
                let bound = if inflate {
                    Aabb {
                        center: aabb.center,
                        half_extents: aabb.half_extents + bevy::math::Vec3A::ONE,
                    }
                } else {
                    *aabb
                };
                let (min, max) = world_aabb(&bound, gt);
                match ray_aabb(origin, dir, min, max) {
                    Some(t) => t,
                    None => continue,
                }
            }
            (None, Some(_), _, None) => 0.0,
            // No pick geometry (a liquid surface), or a `PickBox` with no bound: not pickable.
            (_, None, _, None) => continue,
        };
        candidates.push((entry, entity));
    }
    candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let mut hits: Vec<(Entity, Option<Arc<WorldObject>>, RayHit)> = Vec::new();
    let mut best = f32::INFINITY;
    for (entry, entity) in candidates {
        if !all_hits && best < entry {
            break; // every remaining box starts beyond the confirmed nearest hit
        }
        let Ok((mesh, gt, _, _, _, blob)) = parts.get(entity) else {
            continue;
        };
        if let Some(blob) = blob {
            // The hit is the member under the cursor, cast at the member's own world pose.
            for m in blob.0.iter() {
                let gt = GlobalTransform::from(m.transform);
                if let Some(h) = ray_submesh(&m.geometry, &gt, origin, dir, inflate) {
                    best = best.min(h.distance);
                    hits.push((entity, Some(m.object.clone()), h));
                }
            }
            continue;
        }
        let hit = match mesh {
            Some(m) => ray_submesh(&m.0, gt, origin, dir, inflate),
            // A `PickBox`: its box entry is its surface (grown by the halo when inflated).
            None => Some(RayHit {
                point: origin + dir * entry,
                normal: -dir,
                distance: entry,
            }),
        };
        if let Some(h) = hit {
            best = best.min(h.distance);
            hits.push((entity, None, h));
        }
    }
    hits.sort_unstable_by(|a, b| a.2.distance.total_cmp(&b.2.distance));
    if !all_hits {
        // A blob contributes one hit per member, so truncate after the sort, not per entity.
        hits.truncate(1);
    }
    hits
}

/// The narrow phase for one part, in model-local space: the ray mapped through the inverse affine
/// keeps its parameter, so a local `t` is the world distance. Vertices take the render form's own
/// bake (`wow_to_bevy`, a billboard card centred at its pivot). With `inflate`, each vertex also
/// moves by its authored normal, raw (the reference's `skinned_pos + rot·normal`); a submesh
/// without normals then reports a miss.
fn ray_submesh(
    geo: &RenderSubmesh,
    gt: &GlobalTransform,
    origin: Vec3,
    dir: Vec3,
    inflate: bool,
) -> Option<RayHit> {
    if inflate && geo.normals.len() != geo.positions.len() {
        return None; // no authored normals, no halo
    }
    let inv = gt.affine().inverse();
    let local_origin = inv.transform_point3(origin);
    let local_dir = inv.transform_vector3(dir);
    let center = geo
        .billboard
        .as_ref()
        .map_or(Vec3::ZERO, |b| wow_to_bevy(b.pivot));
    let pos = |i: u32| {
        let p = wow_to_bevy(*geo.positions.get(i as usize)?) - center;
        Some(if inflate {
            p + wow_to_bevy(geo.normals[i as usize])
        } else {
            p
        })
    };
    let mut nearest: Option<(f32, [Vec3; 3])> = None;
    for t in geo.indices.as_chunks::<3>().0 {
        let (Some(a), Some(b), Some(c)) = (pos(t[0]), pos(t[1]), pos(t[2])) else {
            continue; // an out-of-range index: skip the triangle, not the model
        };
        let tri = [a, b, c];
        if let Some(d) = ray_triangle(local_origin, local_dir, &tri) {
            if nearest.is_none_or(|(nd, _)| d < nd) {
                nearest = Some((d, tri));
            }
        }
    }
    let (t, tri) = nearest?;
    let world = tri.map(|v| gt.transform_point(v));
    let normal = (world[1] - world[0])
        .cross(world[2] - world[0])
        .normalize_or_zero();
    Some(RayHit {
        point: origin + dir * t,
        normal,
        distance: t,
    })
}

/// The narrow phase for one skinned part: skin its vertices through `palette` (world-from-bind-pose
/// joint matrices, as GPU skinning applies them) and return the nearest triangle hit's distance.
/// With `inflate`, each vertex also moves by its skinned normal, un-normalized: a halo of 1
/// model-unit times the palette's scale (the reference's `skinned_pos + rot(palette)·normal`).
pub fn ray_posed_mesh(
    mesh_assets: &Assets<Mesh>,
    mesh_id: AssetId<Mesh>,
    palette: &[Mat4],
    origin: Vec3,
    dir: Vec3,
    inflate: bool,
) -> Option<f32> {
    let mesh = mesh_assets.get(mesh_id)?;
    let VertexAttributeValues::Float32x3(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
    else {
        return None;
    };
    // The joints ride the WOW attributes, not Bevy's skin attributes. Every mesh here is a skinned
    // twin (`RigPart` parts only), so a missing attribute is a broken authoring contract that
    // un-picks the whole unit: warn once.
    let (
        Some(VertexAttributeValues::Uint16x4(joints)),
        Some(VertexAttributeValues::Float32x4(weights)),
    ) = (
        mesh.attribute(benilla_assets::ATTRIBUTE_WOW_JOINT_INDEX),
        mesh.attribute(benilla_assets::ATTRIBUTE_WOW_JOINT_WEIGHT),
    )
    else {
        warn_once!(
            "a skinned part's mesh lacks the WOW joint attributes — the posed pick cannot hit it"
        );
        return None;
    };
    let normals = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(n)) if inflate => Some(n),
        _ if inflate => return None, // can't build the halo without normals
        _ => None,
    };
    // Skin each vertex once by its blended matrix; pass 2 adds the rotated normal.
    let world: Vec<Vec3> = positions
        .iter()
        .enumerate()
        .zip(joints.iter().zip(weights.iter()))
        .map(|((i, p), (j, w))| {
            let mut m = Mat4::ZERO;
            for k in 0..4 {
                if w[k] > 0.0 {
                    if let Some(mk) = palette.get(j[k] as usize) {
                        m += *mk * w[k];
                    }
                }
            }
            let mut out = (m * Vec4::new(p[0], p[1], p[2], 1.0)).truncate();
            if let Some(ns) = normals {
                let n = ns[i];
                out += m.transform_vector3(Vec3::new(n[0], n[1], n[2]));
            }
            out
        })
        .collect();
    let tri = |a: usize, b: usize, c: usize| -> Option<f32> {
        ray_triangle(origin, dir, &[world[a], world[b], world[c]])
    };
    let hits = match mesh.indices()? {
        Indices::U16(ix) => ix
            .as_chunks::<3>()
            .0
            .iter()
            .filter_map(|c| tri(c[0] as usize, c[1] as usize, c[2] as usize))
            .fold(None::<f32>, |acc, t| Some(acc.map_or(t, |a| a.min(t)))),
        Indices::U32(ix) => ix
            .as_chunks::<3>()
            .0
            .iter()
            .filter_map(|c| tri(c[0] as usize, c[1] as usize, c[2] as usize))
            .fold(None::<f32>, |acc, t| Some(acc.map_or(t, |a| a.min(t)))),
    };
    hits
}

/// Ray-triangle intersection (Möller-Trumbore): `t ≥ 0` in `dir` lengths, `dir` need not be unit.
/// Two-sided, since a posed mesh shows back faces at its silhouette edges.
pub(crate) fn ray_triangle(origin: Vec3, dir: Vec3, tri: &[Vec3; 3]) -> Option<f32> {
    let (e1, e2) = (tri[1] - tri[0], tri[2] - tri[0]);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-8 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - tri[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t)
}

/// A model-local [`Aabb`] as the world-space box `(min, max)` of its 8 transformed corners.
pub(crate) fn world_aabb(aabb: &Aabb, gt: &GlobalTransform) -> (Vec3, Vec3) {
    let center = Vec3::from(aabb.center);
    let he = Vec3::from(aabb.half_extents);
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for sx in [-1.0, 1.0] {
        for sy in [-1.0, 1.0] {
            for sz in [-1.0, 1.0] {
                let world = gt.transform_point(center + he * Vec3::new(sx, sy, sz));
                min = min.min(world);
                max = max.max(world);
            }
        }
    }
    (min, max)
}

/// The slab test's entry distance along `dir`; a zero component works through `recip`'s infinities.
pub(crate) fn ray_aabb(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let inv = dir.recip();
    let t1 = (min - origin) * inv;
    let t2 = (max - origin) * inv;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    (tmax >= tmin.max(0.0)).then_some(tmin.max(0.0))
}

/// A [`PickSource`] member's narrow phase: a resident submesh at a world pose, cast through the
/// same bake as a drawn part ([`ray_submesh`]) so no lane re-derives it.
pub fn ray_member(geometry: &RenderSubmesh, transform: Transform, ray: Ray3d) -> Option<RayHit> {
    ray_submesh(
        geometry,
        &GlobalTransform::from(transform),
        ray.origin,
        *ray.direction,
        false,
    )
}

/// Ray against an entity's `Aabb` taken to world space corner by corner: the broad phase, and the
/// whole test for a part with no resident geometry.
pub fn ray_mesh_bounds(origin: Vec3, dir: Vec3, aabb: &Aabb, gt: &GlobalTransform) -> Option<f32> {
    let (min, max) = world_aabb(aabb, gt);
    ray_aabb(origin, dir, min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POSED_TRI: [Vec3; 3] = [
        Vec3::new(-1.0, 0.0, -1.0),
        Vec3::new(1.0, 0.0, -1.0),
        Vec3::new(0.0, 0.0, 1.0),
    ];

    /// The skinned twin carries its joints in the WOW attributes
    /// ([`benilla_assets::ATTRIBUTE_WOW_JOINT_INDEX`]), not Bevy's.
    #[test]
    fn posed_pick_reads_the_wow_skin_attributes() {
        use bevy::asset::RenderAssetUsages;
        use bevy::mesh::PrimitiveTopology;

        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            POSED_TRI.iter().map(|v| v.to_array()).collect::<Vec<_>>(),
        );
        mesh.insert_attribute(
            benilla_assets::ATTRIBUTE_WOW_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(vec![[1, 0, 0, 0]; 3]),
        );
        mesh.insert_attribute(
            benilla_assets::ATTRIBUTE_WOW_JOINT_WEIGHT,
            VertexAttributeValues::Float32x4(vec![[1.0, 0.0, 0.0, 0.0]; 3]),
        );
        mesh.insert_indices(Indices::U16(vec![0, 1, 2]));
        let mut assets = Assets::<Mesh>::default();
        let handle = assets.add(mesh);

        // Bone 1 lifts the triangle 2 on Y and bone 0 is a zero row, so a hit at 3.0 proves the
        // WOW joint index picked row 1.
        let palette = [Mat4::ZERO, Mat4::from_translation(Vec3::Y * 2.0)];
        let t = ray_posed_mesh(
            &assets,
            handle.id(),
            &palette,
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::NEG_Y,
            false,
        )
        .expect("the posed pick must hit the WOW-attributed mesh");
        assert!((t - 3.0).abs() < 1e-5);
    }
    use benilla_formats::RenderSubmesh;

    const TRI: [Vec3; 3] = [
        Vec3::new(-1.0, 0.0, -1.0),
        Vec3::new(1.0, 0.0, -1.0),
        Vec3::new(0.0, 0.0, 1.0),
    ];

    #[test]
    fn ray_triangle_hits_at_the_right_distance() {
        let t = ray_triangle(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y, &TRI).expect("hit");
        assert!((t - 5.0).abs() < 1e-5);
    }

    #[test]
    fn ray_triangle_is_two_sided() {
        assert!(ray_triangle(Vec3::new(0.0, -5.0, 0.0), Vec3::Y, &TRI).is_some());
    }

    #[test]
    fn ray_triangle_misses_outside_and_behind() {
        assert!(ray_triangle(Vec3::new(3.0, 5.0, 0.0), Vec3::NEG_Y, &TRI).is_none());
        assert!(ray_triangle(Vec3::new(0.0, 5.0, 0.0), Vec3::Y, &TRI).is_none());
        assert!(ray_triangle(Vec3::new(0.0, 5.0, 0.0), Vec3::X, &TRI).is_none());
    }

    /// [`TRI`]'s pre-image under `wow_to_bevy` (`bevy = (−y, z, −x)`), so it draws at [`TRI`] in
    /// model-local Bevy space.
    fn wow_tri() -> RenderSubmesh {
        RenderSubmesh {
            positions: vec![[1.0, 1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, 0.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        }
    }

    #[test]
    fn resident_geometry_picks_where_the_render_form_draws() {
        let mesh = PickMesh(std::sync::Arc::new(wow_tri()));
        // A translated + uniformly scaled part: the triangle spans x∈[8,12], z∈[-2,2] at y=1.
        let gt =
            GlobalTransform::from(Transform::from_xyz(10.0, 1.0, 0.0).with_scale(Vec3::splat(2.0)));
        let hit = ray_submesh(&mesh.0, &gt, Vec3::new(10.0, 6.0, 0.0), Vec3::NEG_Y, false)
            .expect("the baked triangle must be hit under its world transform");
        assert!((hit.distance - 5.0).abs() < 1e-4);
        assert!(hit.point.abs_diff_eq(Vec3::new(10.0, 1.0, 0.0), 1e-4));
        assert!(hit.normal.abs_diff_eq(Vec3::Y, 1e-4) || hit.normal.abs_diff_eq(-Vec3::Y, 1e-4));
        assert!(ray_submesh(&mesh.0, &gt, Vec3::new(20.0, 6.0, 0.0), Vec3::NEG_Y, false).is_none());
    }

    /// [`wow_tri`] with every normal model-local Bevy `+X` (WoW `(0, −1, 0)`): the inflated pass
    /// moves the whole triangle 1 model-unit in x.
    fn wow_tri_with_x_normals() -> RenderSubmesh {
        RenderSubmesh {
            normals: vec![[0.0, -1.0, 0.0]; 3],
            ..wow_tri()
        }
    }

    #[test]
    fn inflated_pick_hits_the_one_model_unit_halo() {
        let mesh = PickMesh(std::sync::Arc::new(wow_tri_with_x_normals()));
        // Scale 2: the exact triangle spans x∈[8,12] at y=1; the halo shifts it to x∈[10,14].
        let gt =
            GlobalTransform::from(Transform::from_xyz(10.0, 1.0, 0.0).with_scale(Vec3::splat(2.0)));
        // x=13: 1 world-unit past the exact edge (12), inside the ×2-scaled halo (14).
        assert!(ray_submesh(&mesh.0, &gt, Vec3::new(13.0, 6.0, 0.0), Vec3::NEG_Y, false).is_none());
        let hit = ray_submesh(&mesh.0, &gt, Vec3::new(13.0, 6.0, 0.0), Vec3::NEG_Y, true)
            .expect("the halo must catch a ray 1 world-unit past the silhouette");
        assert!((hit.distance - 5.0).abs() < 1e-4);
        // The halo has its own edge: x=16 is 2 world-units past it.
        assert!(ray_submesh(&mesh.0, &gt, Vec3::new(16.0, 6.0, 0.0), Vec3::NEG_Y, true).is_none());
    }

    #[test]
    fn no_authored_normals_means_no_halo() {
        let mesh = PickMesh(std::sync::Arc::new(wow_tri()));
        let gt = GlobalTransform::from(Transform::from_xyz(10.0, 1.0, 0.0));
        assert!(ray_submesh(&mesh.0, &gt, Vec3::new(10.0, 6.0, 0.0), Vec3::NEG_Y, true).is_none());
    }

    /// Every hit along the ray at a hand-built world, nearest first, as `(entity, distance)`: the
    /// whole ray, so a dropped candidate differs from one that lost.
    fn cast_in(world: &mut World, origin: Vec3, dir: Vec3, inflate: bool) -> Vec<(Entity, f32)> {
        use bevy::ecs::system::RunSystemOnce;
        fn cast(
            In((origin, dir, inflate)): In<(Vec3, Vec3, bool)>,
            ids: Query<Entity, With<ViewVisibility>>,
            parts: PickParts,
        ) -> Vec<(Entity, f32)> {
            let all: HashSet<Entity> = ids.iter().collect();
            let ray = Ray3d::new(origin, Dir3::new(dir).expect("a non-zero ray"));
            cast_pick_ray_impl(ray, &all, &parts, true, inflate)
                .into_iter()
                .map(|(e, _, h)| (e, h.distance))
                .collect()
        }
        world
            .run_system_once_with(cast, (origin, dir, inflate))
            .expect("cast")
    }

    /// A drawn, bounded, visible part with no pick geometry; each test supplies its own.
    fn drawn(world: &mut World, at: Vec3, half: Vec3) -> EntityWorldMut<'_> {
        use bevy::camera::visibility::SetViewVisibility;
        let mut e = world.spawn((
            GlobalTransform::from(Transform::from_translation(at)),
            Aabb::from_min_max(-half, half),
            ViewVisibility::HIDDEN,
        ));
        e.get_mut::<ViewVisibility>()
            .expect("just spawned")
            .set_visible();
        e
    }

    /// A WMO group's MLIQ pool: drawn, identified and bounded, with no pick geometry.
    #[test]
    fn a_bounded_mesh_less_entity_is_not_a_pick_occluder() {
        let mut world = World::new();
        // The pool's box straddles the camera: entry 0.0 wins any distance sort it enters.
        let pool = drawn(&mut world, Vec3::ZERO, Vec3::new(60.0, 60.0, 60.0)).id();
        // The NPC behind it: real geometry at 10 yd (`wow_tri` spans x,z ∈ [-1,1] at y = 0).
        let npc = drawn(&mut world, Vec3::new(0.0, -10.0, 0.0), Vec3::splat(1.0))
            .insert(PickMesh(std::sync::Arc::new(wow_tri())))
            .id();
        let hits = cast_in(&mut world, Vec3::ZERO, Vec3::NEG_Y, false);
        assert!(
            !hits.iter().any(|(e, _)| *e == pool),
            "the mesh-less pool must not be pickable at all, yet it hit: {hits:?}",
        );
        assert_eq!(hits.first().map(|(e, _)| *e), Some(npc));
    }

    #[test]
    fn a_declared_pick_box_is_picked_at_its_box_entry() {
        let mut world = World::new();
        let cube = drawn(&mut world, Vec3::new(0.0, -10.0, 0.0), Vec3::splat(2.0))
            .insert(PickBox)
            .id();
        let hits = cast_in(&mut world, Vec3::ZERO, Vec3::NEG_Y, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, cube);
        assert!((hits[0].1 - 8.0).abs() < 1e-4, "{hits:?}"); // the box's near face
    }

    #[test]
    fn inflated_cast_grows_the_broad_bound_with_the_halo() {
        let mut world = World::new();
        // The mesh part: exact triangle spans x∈[-1,1] (bound half 1); +X normals → halo to x=2.
        let herb = drawn(&mut world, Vec3::new(0.0, -10.0, 0.0), Vec3::splat(1.0))
            .insert(PickMesh(std::sync::Arc::new(wow_tri_with_x_normals())))
            .id();
        // x=1.5: outside the exact bound, so the uninflated cast drops the candidate,
        assert!(cast_in(&mut world, Vec3::new(1.5, 0.0, 0.0), Vec3::NEG_Y, false).is_empty());
        // and inside the halo and the inflated bound.
        let hits = cast_in(&mut world, Vec3::new(1.5, 0.0, 0.0), Vec3::NEG_Y, true);
        assert_eq!(hits.first().map(|(e, _)| *e), Some(herb));

        // The cube fallback: box spans x∈[-2,2] at y∈[-12,-8]; a ray at x=2.5 misses the exact
        // cuboid and hits the inflated one at its grown near face (y=-7, entry 7).
        let mut world = World::new();
        let cube = drawn(&mut world, Vec3::new(0.0, -10.0, 0.0), Vec3::splat(2.0))
            .insert(PickBox)
            .id();
        assert!(cast_in(&mut world, Vec3::new(2.5, 0.0, 0.0), Vec3::NEG_Y, false).is_empty());
        let hits = cast_in(&mut world, Vec3::new(2.5, 0.0, 0.0), Vec3::NEG_Y, true);
        assert_eq!(hits.first().map(|(e, _)| *e), Some(cube));
        assert!((hits[0].1 - 7.0).abs() < 1e-4, "{hits:?}");
    }

    /// A test placement identity.
    fn object(uid: u32, label: &str) -> Arc<WorldObject> {
        Arc::new(WorldObject {
            kind: crate::model_render::ModelKind::Doodad,
            label: label.into(),
            id: uid,
            detail: String::new(),
        })
    }

    /// The identified cast at a hand-built world, nearest-first: `(label, uid, distance)` per hit.
    fn names_in(world: &mut World, origin: Vec3, dir: Vec3) -> Vec<(String, u32, f32)> {
        use bevy::ecs::system::RunSystemOnce;
        fn cast(
            In((origin, dir)): In<(Vec3, Vec3)>,
            ids: Query<Entity, With<ViewVisibility>>,
            objects: Query<&WorldObject>,
            parts: PickParts,
        ) -> Vec<(String, u32, f32)> {
            let all: HashSet<Entity> = ids.iter().collect();
            let ray = Ray3d::new(origin, Dir3::new(dir).expect("a non-zero ray"));
            cast_object_ray(ray, &all, &parts, &objects, &[], true)
                .into_iter()
                .map(|h| {
                    let o = h.object.expect("every fixture part is identified");
                    (o.label, o.id, h.hit.distance)
                })
                .collect()
        }
        world
            .run_system_once_with(cast, (origin, dir))
            .expect("cast")
    }

    #[test]
    fn a_blob_names_the_member_under_the_cursor() {
        let mut world = World::new();
        // Two members of one blob, 10 yd apart down the ray; the blob's own bound covers both.
        let near = object(11, "elwynntreecanopy02.m2");
        let far = object(22, "elwynnbush09.m2");
        drawn(&mut world, Vec3::ZERO, Vec3::new(4.0, 20.0, 4.0))
            .insert(WorldObject {
                kind: crate::model_render::ModelKind::Doodad,
                label: "static-merge".into(),
                id: 0,
                detail: "2 batches merged".into(),
            })
            .insert(PickBlob(Arc::from(
                [
                    PickMember {
                        object: near.clone(),
                        geometry: Arc::new(wow_tri()),
                        transform: Transform::from_xyz(0.0, -5.0, 0.0),
                    },
                    PickMember {
                        object: far.clone(),
                        geometry: Arc::new(wow_tri()),
                        transform: Transform::from_xyz(0.0, -15.0, 0.0),
                    },
                ]
                .as_slice(),
            )));
        let hits = names_in(&mut world, Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y);
        assert_eq!(
            hits.iter()
                .map(|(l, id, d)| (l.as_str(), *id, *d))
                .collect::<Vec<_>>(),
            vec![
                ("elwynntreecanopy02.m2", 11, 15.0),
                ("elwynnbush09.m2", 22, 25.0),
            ],
            "the members answer, in depth order — never the blob",
        );
    }

    #[test]
    fn an_undrawn_blob_is_transparent() {
        let mut world = World::new();
        let mut e = drawn(&mut world, Vec3::ZERO, Vec3::new(4.0, 20.0, 4.0));
        e.insert(PickBlob(Arc::from(
            [PickMember {
                object: object(11, "tree.m2"),
                geometry: Arc::new(wow_tri()),
                transform: Transform::from_xyz(0.0, -5.0, 0.0),
            }]
            .as_slice(),
        )));
        *e.get_mut::<ViewVisibility>().expect("spawned visible") = ViewVisibility::HIDDEN;
        assert!(names_in(&mut world, Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y).is_empty());
    }

    /// The render form centres a billboard card at its pivot, so the pick subtracts the same pivot.
    #[test]
    fn billboard_card_picks_pivot_centred() {
        let mut sub = wow_tri();
        // Pivot at WoW (0, 0, 3) → Bevy (0, 3, 0): the baked card shifts down 3 in model-local y.
        sub.billboard = Some(benilla_formats::Billboard {
            pivot: [0.0, 0.0, 3.0],
            bone: 0,
            kind: benilla_formats::BillboardKind::Spherical,
            scale_anim: None,
            seq_translations: Vec::new(),
        });
        let mesh = PickMesh(std::sync::Arc::new(sub));
        // The card entity sits at the pivot's world spot, so the triangle 3 below it draws at world
        // y = 0 and a ray down from y = 8 hits at 8; without the pivot subtraction, at y = 3.
        let gt = GlobalTransform::from(Transform::from_xyz(0.0, 3.0, 0.0));
        let hit = ray_submesh(&mesh.0, &gt, Vec3::new(0.0, 8.0, 0.0), Vec3::NEG_Y, false)
            .expect("the pivot-centred card must be hit where it draws");
        assert!((hit.distance - 8.0).abs() < 1e-4);
        assert!(hit.point.abs_diff_eq(Vec3::ZERO, 1e-4));
    }
}
