//! The shared **ray caster** — the pick-geometry declarations and the triangle-accurate cast every
//! "what is under the cursor" consumer runs through: the inspector's mouseover, the target module's
//! GameObject hover, the `WOW_PICK` probe. Casts against **resident geometry** ([`PickMesh`] —
//! decision 0857), because the render meshes are `RENDER_WORLD`-only and a physics ray
//! misses colliderless props. Pick geometry is **declared, never inferred**.

use std::sync::Arc;

use benilla_assets::coords::wow_to_bevy;
use benilla_formats::RenderSubmesh;
use bevy::camera::primitives::Aabb;
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use super::WorldObject;

/// The resident **pick geometry** of one drawn batch: the model's decoded `RenderSubmesh`, `Arc`-shared
/// with the model asset itself. The render forms are `RENDER_WORLD`-only since 0834 —
/// their main-world vertex data is gone after extract, which is why Bevy's `MeshRayCast` silently
/// stopped hitting every static model (the GO hover, the inspector, `WOW_PICK`). So the pickers read
/// triangles from THIS instead, through the same WoW→Bevy bake the render form was built with
/// (`submesh_to_static_mesh`: `wow_to_bevy` per vertex, billboard cards centred at their pivot).
/// Attached beside `Mesh3d` at every spawn site that also attaches a pick key
/// ([`super::WorldObject`] / `ModelPart`). A keyed entity carrying neither this nor [`PickBox`] is
/// **not pickable** — pick geometry is declared, never inferred from a bound.
#[derive(Component, Clone)]
pub struct PickMesh(pub Arc<RenderSubmesh>);

/// "My `Aabb` **is** my pick geometry" — the model-less cube fallback ([`crate::entities::attach`]),
/// whose cuboid is exactly its bound, so a slab test is the exact answer and 12 triangles would only
/// be a slower way to say it.
///
/// It exists as a **positive declaration** because the alternative — inferring it from the *absence*
/// of a [`PickMesh`] — silently promoted every other keyed-but-mesh-less drawn entity into a solid
/// invisible box. That is what broke the inspector inside a city WMO: a WMO group's
/// MLIQ pool is a drawn `Mesh3d` with no resident geometry, it inherits the building's
/// [`super::WorldObject`] from the placement's blanket tag, and its render mesh keeps the **whole**
/// MLIQ vertex grid — dry cells included, sitting at height 0 while the drawn lava is 86–127 yd
/// below. Ironforge's thirteen pools therefore hung ~100-yard invisible boxes over the entire city,
/// and the box-entry hit won every pick: every hover, on anything, answered "Wmo · ironforge.wmo".
#[derive(Component, Clone, Copy)]
pub struct PickBox;

/// One placement's batch inside a **consolidated draw** — its identity, its resident geometry and
/// the world pose that geometry draws at.
///
/// The consolidating render lanes (the static merge's blobs, 1417/1418; the retained static pass's
/// cell/region bakes, 1429–1434) draw many placements as one thing. They win their draw calls by
/// *removing* the per-placement entity — and the pick declaration ([`PickMesh`] + [`super::WorldObject`])
/// used to ride that entity, so consolidating a placement silently un-named it: the inspector, the GO
/// hover and `WOW_PICK` all went quiet over most of the static world, and 0929's "declared, never
/// inferred" rule made the loss indistinguishable from correct transparency.
///
/// This is the declaration that survives the consolidation: identity + geometry + pose, per member,
/// held by whoever owns the consolidated draw. The pick walks members exactly as it walks parts, so
/// a blob or a retained cell answers with the *placement* under the cursor, never with the draw that
/// happens to contain it.
#[derive(Clone)]
pub struct PickMember {
    /// Shared per placement — one allocation per placement, not per batch.
    pub object: Arc<WorldObject>,
    pub geometry: Arc<RenderSubmesh>,
    /// The member's own WORLD pose (the placement transform its geometry was baked with), not a
    /// pose relative to the consolidated draw's entity.
    pub transform: Transform,
}

/// The members of a **blob** — a consolidated draw that is still one entity (the static merge's
/// lane, `terrain_stream::merge`). The entity keeps the broad phase and the drawn test it always
/// had (its union `Aabb`, its own `ViewVisibility`); this component is the narrow phase, and the
/// identity that comes back.
///
/// A blob carries no [`PickMesh`]: its baked mesh is the union of its members and would answer with
/// the blob, not with what the cursor is on.
#[derive(Component, Clone)]
pub struct PickBlob(pub Arc<[PickMember]>);

/// One hit from the **identified** cast ([`cast_object_ray`]): what was hit, and where.
///
/// The identity is *resolved here*, not left as an entity for the caller to look up, because most
/// of the static world has no entity to look anything up on — it draws from a consolidated lane.
/// `entity` is `Some` only when one owns the geometry, which is exactly when the
/// per-entity readouts (a unit's descriptor store, a GameObject's collision, a part's material)
/// exist to be read.
pub struct ObjectHit {
    /// What was hit. `None` only for an entity a caller made pickable WITHOUT a world identity —
    /// an equipped item's part or its billboard card, which `WOW_PICK` admits on `ModelPart` so a
    /// "my pauldron looks wrong" report has something to name. Consolidated
    /// content is always identified: identity is what its lane carries it by.
    pub object: Option<WorldObject>,
    pub entity: Option<Entity>,
    pub hit: RayHit,
}

/// Geometry that draws with **no entity to hang a [`PickMesh`] on** — today the retained static
/// pass ([`crate::static_gx`]), whose cell and region bakes replaced their placements' entities
/// outright.
///
/// A source answers the ray itself rather than publishing a side table, because **only the lane
/// knows what it actually drew this frame**: its selection is a CPU scene walk at cell/group/set
/// granularity, not a `ViewVisibility` bit an outside walker could read. Implementors must report
/// only selected content — that is the entity path's own rule (`ViewVisibility`), kept.
pub trait PickSource {
    /// Append every hit along `ray`, nearest triangle per member. `all_hits` asks for the whole
    /// ray (the `WOW_PICK` reading) rather than just this source's nearest.
    fn cast_objects(&self, ray: Ray3d, all_hits: bool, out: &mut Vec<(Arc<WorldObject>, RayHit)>);
}

/// One triangle-accurate hit from [`cast_pick_ray`]: the world-space point, the hit triangle's
/// geometric normal (two-sided — the orientation is the authored winding's), and the distance along
/// the ray.
pub struct RayHit {
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
}

/// The one query every mesh picker casts against: a part's resident geometry, its world pose, the
/// render world's own visibility verdict (the cast honours it, as `RayCastVisibility::VisibleInView`
/// did), its bound for the broad phase, whether that bound is itself the pick shape ([`PickBox`]),
/// and — for a consolidated blob — the members its baked mesh stands in for ([`PickBlob`]).
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

/// Cast a ray from the logical `cursor` position into the world and return the nearest hit among
/// `pickable`: `(entity, world point, distance)`. The `pickable` set restricts the cast (terrain,
/// particle billboards, and other un-identified meshes stay transparent), so callers choose *what* is
/// pickable while sharing *how* — the inspector's per-frame mouseover passes every
/// [`super::WorldObject`], the target picker passes only unit meshes (so a doodad in front doesn't
/// block a click on a mob).
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

/// The **identified** cast: the ECS pick set (parts, boxes and blob members) *plus* every
/// entity-less [`PickSource`], nearest-first — what a "name what I am pointing at" instrument
/// wants, where the plain entity cast answers "which entity".
///
/// The two populations are cast independently and merged by distance, because they are culled by
/// different machinery: an entity by its `ViewVisibility`, a source by its lane's own scene walk.
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
            // A consolidated member's identity travels with the hit; the entity is the DRAW's
            // (a blob), which is never what the cursor is on.
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

/// [`cast_object_ray`] through a screen pixel — the mouseover's whole job.
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

/// Cast `ray` against `pickable`'s **resident geometry** ([`PickMesh`] — decision 0857: the render
/// meshes are `RENDER_WORLD`-only, so there is no main-world mesh data to cast against) and return
/// the hits nearest-first: one per entity (its nearest triangle), the whole list with `all_hits`
/// (the `WOW_PICK` probe's everything-along-the-ray reading) or just the front entity without.
///
/// Broad phase: a world-space slab test on each candidate's `Aabb` (entry distance), nearest entry
/// first, so the narrow walk stops as soon as a confirmed hit is closer than every remaining box. A
/// part with no bound (a `NoFrustumCulling` fx part) is narrow-tested unconditionally; a
/// [`PickBox`] entity takes its box entry as the hit, its cuboid being exactly its bound.
/// Triangles test **two-sided**, like the unit picker's narrow phase — a generous pick beats a
/// strict one at silhouette edges.
///
/// **Pick geometry is required, never inferred**: an entity that is neither a
/// [`PickMesh`] nor a [`PickBox`] is not pickable, however identified or bounded it is. The box hit
/// used to be the fallback for *any* keyed entity with an `Aabb` and no mesh, which quietly turned
/// a WMO's MLIQ pool — a drawn mesh with no resident geometry, wearing its building's identity —
/// into a city-sized invisible occluder. See [`PickBox`].
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

/// [`cast_pick_ray`]'s **generous second pass** (resolve `0x7089c0` pass 2,
/// mouse-pick only): every vertex displaced by its **authored normal,
/// added raw** — 1 model-unit (× the part's world scale) outward, the same halo the unit picker
/// builds from skinned normals. A part without authored normals cannot build the halo and stays
/// exact-only, like the unit path. A [`PickBox`] inflates its cuboid by the same 1 model-unit —
/// the cube fallback stands in for a model, so the halo applies to it too. Returns **all** hits
/// (the caller ranks them by the reference's pass-2 priority ladder, not pure distance). The
/// reference clips its halo to the sequence bounds *sphere*; our broad bound is the tight mesh
/// `Aabb` inflated by the same 1 model-unit — strictly more permissive, never less (the target
/// module's standing direction of error).
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

/// The entity walk, with each hit's optional **member identity** (a blob hit names the placement
/// under the cursor, not the blob — [`PickBlob`]). [`cast_pick_ray`] drops that half; the identified
/// cast ([`cast_object_ray`]) is the reason it exists.
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
            continue; // not drawn in any view this frame — the old cast's VisibleInView rule
        }
        // A blob's bound is the union of its members and is authored (`NoAutoAabb`), so it is
        // both the broad phase and the only thing the ray can be rejected on before the members
        // are walked. A blob with no bound is narrow-tested unconditionally, like a bound-less part.
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
                // Inflated cast: the halo displaces at most 1 model-unit (local space), so the
                // local bound grown by exactly that can never clip it.
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
            // No pick geometry — a drawn mesh nobody armed for the ray (a liquid surface), or a
            // `PickBox` with no bound to be. Not pickable, rather than pickable as its bound.
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
            // A blob stands in for many placements: the hit is the MEMBER under the cursor, cast
            // at the member's own world pose (its geometry is baked into the blob's mesh, but the
            // resident submesh + placement transform is what it draws).
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
            // A `PickBox` (the broad phase admitted no other mesh-less candidate): its box entry is
            // exactly its surface, the cuboid BEING its Aabb (grown by the halo when inflated).
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
        // One hit for the whole cast — but a blob contributes one per member, so the truncation
        // has to come after the sort (it does) and cannot be a per-entity dedup.
        hits.truncate(1);
    }
    hits
}

/// The narrow phase for one part: ray-test every triangle of its resident geometry, in **model-local
/// space** — the world ray mapped through the part's inverse affine, under which the ray parameter is
/// unchanged, so a local `t` (against the world-normalized direction's image) *is* the world distance.
/// Vertices go through the render form's own bake (`build_submesh_mesh`): `wow_to_bevy` per vertex,
/// a billboard card centred at its pivot — so the pick tests exactly the surface the part draws
/// (a card's live camera-facing rotation rides its `GlobalTransform`, shared here too). With
/// `inflate` (the generous pass 2), each vertex is additionally displaced by its
/// authored normal, raw — 1 model-unit in local space, which the world transform then scales,
/// exactly the reference's `skinned_pos + rot·normal` with no extra constant. `wow_to_bevy` is a
/// pure axis permutation with sign flips (orthonormal), so applying it to the normal is exact. A
/// submesh authored without normals can't build the halo and reports a miss (the exact pass
/// already had its say).
fn ray_submesh(
    geo: &RenderSubmesh,
    gt: &GlobalTransform,
    origin: Vec3,
    dir: Vec3,
    inflate: bool,
) -> Option<RayHit> {
    if inflate && geo.normals.len() != geo.positions.len() {
        return None; // no authored normals — no halo to build
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
            continue; // an out-of-range index — corrupt authoring; skip the triangle, not the model
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

/// The narrow phase for one **skinned** part: skin its vertices through `palette` (world-from-bind-pose joint
/// matrices — the same transform GPU skinning applies) and ray-test every triangle. With `inflate`
/// (the reference's pass 2), each vertex is additionally displaced by its **skinned normal, un-
/// normalized** — the M2 normal is unit-length and the palette carries the world scale, so the
/// halo is 1 model-unit × scale outward, exactly the binary's `skinned_pos + rot(palette)·normal`
/// with no extra constant. Returns the nearest world-space hit distance, or `None`. Cost is
/// bounded by the broad phase: only units near the cursor get here.
///
/// This lives here, beside the cast it belongs to, because it composes a **skinning palette** —
/// the 0720 convention that retired Bevy's skin lane and put the joint data on the WOW attributes.
/// A caller that re-derives that composition goes wrong silently the day the convention moves
/// (it does not fail to compile; it stops hitting units), which is the same failure the light
/// packer's doc records one module over.
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
    // The joint data rides the WOW attributes (decision 0720 retired Bevy's skin lane, and
    // Bevy's `ATTRIBUTE_JOINT_INDEX` left our meshes with it). Every mesh that reaches this
    // function is a skinned twin by construction (`RigPart` parts only), so a missing
    // attribute is a broken authoring contract — not an unloaded asset — and it silently
    // un-picks the whole unit: say so, once, loudly.
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
    // Skin every vertex to world space once (blended matrix, as GPU skinning sums it), then walk
    // the index triangles. Pass 2 adds the rotated normal, translation-free, to the position.
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

/// Ray–triangle intersection (Möller–Trumbore), **two-sided**, returning `t ≥ 0` along `dir`
/// (unnormalized is fine — `t` is in `dir` lengths) or `None` on miss/parallel. Two-sided because a
/// posed mesh can present back faces at silhouette edges and a generous pick beats a strict one.
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

/// A mesh's model-local [`Aabb`] transformed into a world-space axis-aligned box `(min, max)` — its 8
/// corners run through the entity's world transform, then min/max'd. (An AABB of the rotated box: a hair
/// larger than the true oriented box, which only makes the hover more forgiving.)
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

/// Ray vs axis-aligned box (the slab test): the entry distance along `dir` if the ray hits, else `None`.
/// `dir` need not be normalized; a zero component is handled by the infinities `recip` produces.
pub(crate) fn ray_aabb(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let inv = dir.recip();
    let t1 = (min - origin) * inv;
    let t2 = (max - origin) * inv;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    (tmax >= tmin.max(0.0)).then_some(tmin.max(0.0))
}

/// The narrow phase one [`PickSource`] member needs: a resident submesh at a world pose, cast
/// exactly the way a drawn part's own geometry is ([`ray_submesh`] — the render form's `wow_to_bevy`
/// bake, billboard cards centred at their pivot). Exposed so a lane answering the ray never
/// re-derives that bake and drifts from what it draws.
pub fn ray_member(geometry: &RenderSubmesh, transform: Transform, ray: Ray3d) -> Option<RayHit> {
    ray_submesh(
        geometry,
        &GlobalTransform::from(transform),
        ray.origin,
        *ray.direction,
        false,
    )
}

/// Ray against a drawn entity's **world-space bound** — its `Aabb` taken through its
/// `GlobalTransform`, corner by corner, so a rotated model's box is the box of its rotated
/// corners and not its rotated box. The broad phase, and the whole test for a part whose exact
/// geometry is not resident.
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

    /// The picker ↔ mesh-builder attribute contract: [`ray_posed_mesh`] must read the WOW joint
    /// attributes the skinned twin is authored with ([`benilla_assets::ATTRIBUTE_WOW_JOINT_INDEX`],
    /// decision 0720) — reading Bevy's standard skin attributes made every unit silently
    /// unpickable, because a `None` here also blocks the AABB fallback (the `faithful` set).
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

        // Bone 1 lifts the triangle +2 on Y; bone 0 is a zero row (a hit through it would land
        // at the origin) — so a 3.0-distance hit proves the indices routed through the WOW
        // attribute into the right palette row.
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
        // Same triangle hit from below (reversed winding relative to the ray) still connects.
        assert!(ray_triangle(Vec3::new(0.0, -5.0, 0.0), Vec3::Y, &TRI).is_some());
    }

    #[test]
    fn ray_triangle_misses_outside_and_behind() {
        // Outside the triangle's extent.
        assert!(ray_triangle(Vec3::new(3.0, 5.0, 0.0), Vec3::NEG_Y, &TRI).is_none());
        // Triangle behind the ray origin (t < 0).
        assert!(ray_triangle(Vec3::new(0.0, 5.0, 0.0), Vec3::Y, &TRI).is_none());
        // Parallel to the plane.
        assert!(ray_triangle(Vec3::new(0.0, 5.0, 0.0), Vec3::X, &TRI).is_none());
    }

    /// [`TRI`]'s pre-image under `wow_to_bevy` (`bevy = (−y, z, −x)` ⇒ `wow = (−bz, −bx, by)`):
    /// a submesh authored with THESE WoW-space positions must be picked exactly where the render
    /// form draws it, i.e. at [`TRI`] in model-local Bevy space.
    fn wow_tri() -> RenderSubmesh {
        RenderSubmesh {
            positions: vec![[1.0, 1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, 0.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        }
    }

    /// The picker ↔ render-form bake contract: [`ray_submesh`] must test the
    /// resident WoW-axes geometry through the SAME `wow_to_bevy` bake `submesh_to_static_mesh`
    /// builds the drawn mesh with, under the part's world transform — the render mesh itself is
    /// `RENDER_WORLD`-only, so this path is the only thing keeping static models pickable.
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
        // …and a miss stays a miss.
        assert!(ray_submesh(&mesh.0, &gt, Vec3::new(20.0, 6.0, 0.0), Vec3::NEG_Y, false).is_none());
    }

    /// [`wow_tri`] with every vertex normal authored as model-local Bevy `+X` (WoW pre-image
    /// `(0, −1, 0)`): the inflated pass displaces the whole triangle +1 model-unit in x.
    fn wow_tri_with_x_normals() -> RenderSubmesh {
        RenderSubmesh {
            normals: vec![[0.0, -1.0, 0.0]; 3],
            ..wow_tri()
        }
    }

    /// **Decision 1071 — the generous pass.** The inflated narrow phase displaces each vertex by
    /// its authored normal, raw: 1 model-unit in local space, scaled by the part's world transform
    /// (the reference's `skinned_pos + rot·normal`, no extra constant). A ray 1 world-unit beyond
    /// the drawn silhouette misses the exact pass and hits the halo; the halo is 2 world-units wide
    /// here because the part's scale is 2.
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
        // …and the halo has its own edge: 2 world-units past the ×2 halo is still a miss.
        assert!(ray_submesh(&mesh.0, &gt, Vec3::new(16.0, 6.0, 0.0), Vec3::NEG_Y, true).is_none());
    }

    /// A submesh authored without normals cannot build the halo: the inflated pass reports a miss
    /// (never a panic, never a silently un-displaced hit) — the exact pass already had its say.
    #[test]
    fn no_authored_normals_means_no_halo() {
        let mesh = PickMesh(std::sync::Arc::new(wow_tri()));
        let gt = GlobalTransform::from(Transform::from_xyz(10.0, 1.0, 0.0));
        assert!(ray_submesh(&mesh.0, &gt, Vec3::new(10.0, 6.0, 0.0), Vec3::NEG_Y, true).is_none());
    }

    /// Cast a ray at a hand-built world and report `(entity, distance)` for every hit, nearest
    /// first — the whole ray, so a candidate that was silently dropped is distinguishable from one
    /// that merely lost. `inflate` runs the generous pass instead of the exact one.
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

    /// Everything a drawn, bounded, visible part needs to be a pick candidate — minus the geometry,
    /// which each test supplies (or deliberately withholds).
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

    /// **Decision 0929.** A drawn, identified, bounded entity with NO pick geometry — a WMO group's
    /// MLIQ pool, whose render mesh keeps the whole (mostly dry) vertex grid and so bounds a box
    /// ~100 yd tall over a city — must be transparent to the ray. It used to be picked at its box
    /// entry, which put an invisible occluder in front of everything: inside Ironforge every hover,
    /// on any NPC or GameObject, answered "Wmo · ironforge.wmo".
    #[test]
    fn a_bounded_mesh_less_entity_is_not_a_pick_occluder() {
        let mut world = World::new();
        // The pool's box straddles the camera (entry 0.0 — it wins any distance sort it enters).
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

    /// The other half of the same law: an entity that DECLARES its box is its shape ([`PickBox`] —
    /// the model-less cube fallback) is still picked, at the box entry.
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

    /// **Decision 1071, the broad phase.** The inflated cast must grow the candidate's bound by the
    /// same 1 model-unit as the halo, or the slab test rejects the entity before its halo is ever
    /// narrow-tested — a mesh whose bound the ray misses by half a unit must still halo-hit. And a
    /// [`PickBox`] inflates its declared cuboid the same way (its box IS its geometry).
    #[test]
    fn inflated_cast_grows_the_broad_bound_with_the_halo() {
        let mut world = World::new();
        // The mesh part: exact triangle spans x∈[-1,1] (bound half 1); +X normals → halo to x=2.
        let herb = drawn(&mut world, Vec3::new(0.0, -10.0, 0.0), Vec3::splat(1.0))
            .insert(PickMesh(std::sync::Arc::new(wow_tri_with_x_normals())))
            .id();
        // x=1.5: outside the exact bound (the uninflated cast drops the candidate entirely)…
        assert!(cast_in(&mut world, Vec3::new(1.5, 0.0, 0.0), Vec3::NEG_Y, false).is_empty());
        // …inside the halo (and the inflated bound that must admit it).
        let hits = cast_in(&mut world, Vec3::new(1.5, 0.0, 0.0), Vec3::NEG_Y, true);
        assert_eq!(hits.first().map(|(e, _)| *e), Some(herb));

        // The cube fallback: box spans x∈[-2,2] at y∈[-12,-8]; a ray at x=2.5 misses the exact
        // cuboid, hits the 1-unit-inflated one — at its grown near face (y=-7 → entry 7).
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

    /// **Decision 1534.** A consolidated blob draws many placements as one entity, and its baked
    /// mesh is their union — so casting against the blob could only ever answer with the blob
    /// ("static-merge", which is what its own `WorldObject` says). A [`PickBlob`] declares the
    /// members instead: the cast names the PLACEMENT under the cursor, at that placement's own
    /// pose, and the members are depth-ordered among themselves like separate parts would be.
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

    /// The blob's drawn test is still the ENTITY's: a blob the render world culled must be as
    /// transparent to the ray as any other undrawn part (the `ViewVisibility` rule the entity
    /// path has always had).
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

    /// A billboard card's render form is centred at its pivot (`build_submesh_mesh`); the pick must
    /// subtract the same pivot or it tests the card a whole pivot-offset away from where it draws.
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
        // The card entity sits AT the pivot's world spot (the spawn sites bake
        // `transform_point(pivot)` into its translation), so the authored triangle — 3 below the
        // pivot in model-local Bevy y — draws at world y = 0: a straight-down ray from y = 8 hits
        // at distance 8. Without the pivot subtraction it would (wrongly) hit at y = 3.
        let gt = GlobalTransform::from(Transform::from_xyz(0.0, 3.0, 0.0));
        let hit = ray_submesh(&mesh.0, &gt, Vec3::new(0.0, 8.0, 0.0), Vec3::NEG_Y, false)
            .expect("the pivot-centred card must be hit where it draws");
        assert!((hit.distance - 8.0).abs() < 1e-4);
        assert!(hit.point.abs_diff_eq(Vec3::ZERO, 1e-4));
    }
}
