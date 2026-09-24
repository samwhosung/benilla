//! The current-group down-ray: which room the camera is in, and which flood roots follow.
//!
//! The 1.12 client's per-frame probe (`0x6821f0` → `0x6be250` → `0x6a3f80`) casts 1760 yd down
//! from the eye, and the nearest crossing wins:
//!
//! - Leg A, walking-collision faces (the collision BSP, MOPY mask `0x84`): every non-DETAIL face,
//!   with no orientation filter.
//! - Leg B, portal crossings (`0x6a3f80`): the eye's side of the portal plane picks the group
//!   (`group_a iff (signed_dist ≥ 0) == (side > 0)`). A near-parallel portal (`|denom| < 1e-4`)
//!   counts only within the 0.1 yd snap of its plane (`0x7c22b0`), and a crossing within about
//!   1e-4 of the nearest face hit still wins (`0x80c4f4`).
//! - The terrain race: the WMO hit is dropped only when terrain on the same segment is strictly
//!   nearer (`0x6822a2`; `GetAreaID` `0x670250` arbitrates alike at `0x670345`). Terrain above the
//!   eye is off the segment, so a tunnel under a hill stays inside.
//!
//! The verdict is the in-group plus, for a portal win, the across-group: the containing-group set
//! `0xc7cd88`, each flooded as its own root (`0x6b3bd4`–`0x6b3c10`). An exterior winner or no
//! crossing is outside.
//!
//! Deviation: when legs A and B both miss and no terrain lies at or below the eye, Leg C runs the
//! same race over the camera-only faces (DETAIL without NOCAMCOLLIDE), because the 1.12 client
//! blanks the whole building from a pocket floored only with DETAIL faces, as at the Deadmines
//! entrance.

use benilla_assets::column_grid::ColumnGrid;
use benilla_assets::{WmoGroupNav, WmoModel, WmoPortalInfo, WmoPortalRef};
pub use benilla_formats::triangle_z_at as floor_z_at;

use super::{
    point_in_poly_2d, portal_poly, EXTERIOR, MAX_FLOOR_DROP, NEAREST_TIE_EPS, PORTAL_NEAR_PARALLEL,
    PORTAL_PLANE_SNAP,
};

/// The down-ray's verdict: the camera's group and, when the nearest crossing was a portal, the
/// group across it, both flood roots (the containing-group set `0xc7cd88`). Outside, both are
/// `None`.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct DownRaySeeds {
    pub in_group: Option<usize>,
    pub(crate) across: Option<usize>,
}

/// [`down_ray_pick`] over the asset's stored pieces; `terrain_z` is in this model's local space.
pub fn down_ray_seeds(model: &WmoModel, eye: [f32; 3], terrain_z: Option<f32>) -> DownRaySeeds {
    down_ray_pick(
        &model.group_collision_tris,
        &model.group_collision_grids,
        &model.group_camera_only_tris,
        &model.group_nav,
        &model.portal_vertices,
        &model.portal_infos,
        &model.portal_refs,
        eye,
        terrain_z,
    )
}

/// The eye's current group, by the module doc's mechanism. `terrain_z` is the terrain height under
/// the eye in WMO model space, or `None` where there is none to hit: off the streamed tiles, or a
/// hole cut through the ground into this interior.
pub(crate) fn down_ray_pick(
    tris: &[Vec<[[f32; 3]; 3]>],
    grids: &[Option<ColumnGrid>],
    camera_only_tris: &[Vec<[[f32; 3]; 3]>],
    nav: &[WmoGroupNav],
    portal_vertices: &[[f32; 3]],
    portal_infos: &[WmoPortalInfo],
    portal_refs: &[WmoPortalRef],
    eye: [f32; 3],
    terrain_z: Option<f32>,
) -> DownRaySeeds {
    // Broad phase: the column inside the group's XY bounds, its bottom at or below the eye.
    let in_column = |g: &WmoGroupNav| {
        eye[0] >= g.bbox_min[0]
            && eye[0] <= g.bbox_max[0]
            && eye[1] >= g.bbox_min[1]
            && eye[1] <= g.bbox_max[1]
            && g.bbox_min[2] <= eye[2]
    };

    // Leg A, walking-collision faces: the highest crossing at or below the eye.
    let mut best_z = f32::NEG_INFINITY;
    let mut best: Option<usize> = None;
    for (gi, g) in nav.iter().enumerate() {
        if !in_column(g) {
            continue;
        }
        // The column index, where the group has one, yields the same faces in the same order.
        let mut consider = |tri: &[[f32; 3]; 3]| {
            if let Some(z) = floor_z_at(tri, eye[0], eye[1]) {
                if z <= eye[2] && z > best_z {
                    best_z = z;
                    best = Some(gi);
                }
            }
        };
        let group_tris = tris.get(gi).map(Vec::as_slice).unwrap_or_default();
        match grids.get(gi).and_then(Option::as_ref) {
            Some(grid) => {
                for i in grid.candidates(eye[0], eye[1]) {
                    if let Some(tri) = group_tris.get(i) {
                        consider(tri);
                    }
                }
            }
            None => group_tris.iter().for_each(&mut consider),
        }
    }

    // Leg B, portal crossings, racing the face hit; a coincident portal wins the tie.
    let mut across: Option<usize> = None;
    for (gi, g) in nav.iter().enumerate() {
        if !in_column(g) {
            continue;
        }
        let start = g.ref_start as usize;
        let end = (start + g.ref_count as usize).min(portal_refs.len());
        for r in &portal_refs[start..end] {
            let Some(info) = portal_infos.get(r.portal as usize) else {
                continue;
            };
            let [nx, ny, nz, d] = info.plane;
            let z = if nz.abs() < PORTAL_NEAR_PARALLEL {
                // A vertical doorway: crossed, at the eye, only within the 0.1 yd snap.
                if (nx * eye[0] + ny * eye[1] + nz * eye[2] + d).abs() > PORTAL_PLANE_SNAP {
                    continue;
                }
                eye[2]
            } else {
                let z = -(nx * eye[0] + ny * eye[1] + d) / nz;
                if z > eye[2] {
                    continue; // the crossing is above the eye (t < 0)
                }
                z
            };
            if z < best_z - NEAREST_TIE_EPS {
                continue; // a face hit is strictly nearer
            }
            let Some(verts) = portal_poly(portal_vertices, info) else {
                continue;
            };
            if !point_in_poly_dominant(verts, info.plane, [eye[0], eye[1], z]) {
                continue;
            }
            // The plane-side pick (`0x6a4130..0x6a4159`): the eye's side, oriented by `side`, picks
            // the walked group or the neighbour.
            let d_signed = nx * eye[0] + ny * eye[1] + nz * eye[2] + d;
            let chosen = if (d_signed >= 0.0) == (r.side > 0) {
                gi
            } else {
                r.group as usize
            };
            // A dead neighbour ref (`0xffff`) is no room: leave the crossing to the mirrored ref.
            if nav.get(chosen).is_none() {
                continue;
            }
            let other = if chosen == gi { r.group as usize } else { gi };
            best_z = z;
            best = Some(chosen);
            across = (nav.get(other).is_some() && other != chosen).then_some(other);
        }
    }

    // Outside: nothing within the ray, or an exterior winner; the client appends no seeds.
    let Some(in_group) = best else {
        // Leg C, the camera-void fallback (the module doc's deviation): only when both legs missed
        // and no terrain lies at or below the eye. Leg A's rules; one root, no `across`.
        if terrain_z.is_some_and(|tz| tz <= eye[2]) {
            return DownRaySeeds::default();
        }
        let mut fb_z = f32::NEG_INFINITY;
        let mut fb: Option<usize> = None;
        for (gi, g) in nav.iter().enumerate() {
            if !in_column(g) {
                continue;
            }
            for tri in camera_only_tris.get(gi).into_iter().flatten() {
                if let Some(z) = floor_z_at(tri, eye[0], eye[1]) {
                    if z <= eye[2] && z > fb_z {
                        fb_z = z;
                        fb = Some(gi);
                    }
                }
            }
        }
        let named = fb.filter(|&g| {
            eye[2] - fb_z <= MAX_FLOOR_DROP && nav.get(g).is_some_and(|n| n.flags & EXTERIOR == 0)
        });
        return DownRaySeeds {
            in_group: named,
            across: None,
        };
    };
    if eye[2] - best_z > MAX_FLOOR_DROP || nav.get(in_group).is_none_or(|g| g.flags & EXTERIOR != 0)
    {
        return DownRaySeeds::default();
    }
    // The terrain race: terrain above the eye is off the segment; terrain strictly nearer than the
    // WMO hit is open ground, outside; a tie keeps the WMO (the client's strict `<`).
    if terrain_z.is_some_and(|tz| tz <= eye[2] && tz > best_z) {
        return DownRaySeeds::default();
    }
    DownRaySeeds {
        in_group: Some(in_group),
        across,
    }
}

/// The zone-text ray length, the client's `[0x8022cc]` = 1000.0; the render probe's is 1760.
const ZONE_RAY_LEN: f32 = 1000.0;

/// The position-cast indoor predicate, faces only: the CGLight node's down-ray attach `0x6a8a20`
/// casts [`ZONE_RAY_LEN`] down, races terrain against WMO faces with the WMO winning ties
/// (`0x6a8b15`), and classifies the winning group's flags against `outdoor_mask`. The classify
/// `0x6a87f0` has two sinks:
///
/// - zone-text (`[node+0x90]` bit 0): [`EXTERIOR`] alone, so a `0x40`-only street is indoors;
/// - unit lighting (`[node+0xc]`): `EXTERIOR | EXTERIOR_LIT`, the fork on `MOGI & 0x48`
///   (`0x6a880d`, `0x6a8823`).
///
/// The portal leg belongs to the camera's render seed alone: a doorway plane under the eye must
/// not claim the interior. `None` is outdoors; [`down_ray_claim`] keeps an outdoor-class winner.
pub(crate) fn area_down_ray(
    tris: &[Vec<[[f32; 3]; 3]>],
    bounds: &[Option<([f32; 3], [f32; 3])>],
    grids: &[Option<ColumnGrid>],
    nav: &[WmoGroupNav],
    eye: [f32; 3],
    terrain_z: Option<f32>,
    outdoor_mask: u32,
) -> Option<usize> {
    down_ray_claim(tris, bounds, grids, nav, eye, terrain_z, outdoor_mask)
        .and_then(|c| (!c.outdoor).then_some(c.group))
}

/// A position-cast ray's claim on one placement before the outdoor collapse: the winning face's
/// group, its depth, and its outdoor class under the caller's mask.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DownRayClaim {
    pub(crate) group: usize,
    /// Depth (yd) of the winning face along the cast. Placements are rigid, so depths compare
    /// across placements: the light chain's one global nearest hit.
    pub(crate) depth: f32,
    /// The winning group carries a flag in the caller's `outdoor_mask`.
    pub(crate) outdoor: bool,
}

/// [`area_down_ray`] without the outdoor collapse: the light chain tells an outdoor-class face (a
/// deck) from open terrain.
pub(crate) fn down_ray_claim(
    tris: &[Vec<[[f32; 3]; 3]>],
    bounds: &[Option<([f32; 3], [f32; 3])>],
    grids: &[Option<ColumnGrid>],
    nav: &[WmoGroupNav],
    eye: [f32; 3],
    terrain_z: Option<f32>,
    outdoor_mask: u32,
) -> Option<DownRayClaim> {
    ray_claim(
        tris,
        bounds,
        grids,
        nav,
        eye,
        terrain_z,
        outdoor_mask,
        false,
    )
}

/// The same claim cast upward: the containment lane's retry, and only that. `0x6a8ed0` anchors a
/// GameObject's light node at its world bounding-box centre (`[node+0x5c]`), and that box is the M2
/// header's authored one (`0x713640` copies `MD20+0xB4`, not the collision box `0x713700` copies),
/// which can put a particle-heavy model's anchor 40 yd under its floor. The reference retries
/// 1000 yd upward (`0x6a908d`, `0x6a9093`). No terrain race: a cast up from under a floor cannot
/// lose to the ground below it.
pub(crate) fn up_ray_claim(
    tris: &[Vec<[[f32; 3]; 3]>],
    bounds: &[Option<([f32; 3], [f32; 3])>],
    grids: &[Option<ColumnGrid>],
    nav: &[WmoGroupNav],
    eye: [f32; 3],
    outdoor_mask: u32,
) -> Option<DownRayClaim> {
    ray_claim(tris, bounds, grids, nav, eye, None, outdoor_mask, true)
}

/// The shared body: `up` flips which side of the probe a face must lie on; the nearest in the
/// cast's direction wins.
fn ray_claim(
    tris: &[Vec<[[f32; 3]; 3]>],
    bounds: &[Option<([f32; 3], [f32; 3])>],
    grids: &[Option<ColumnGrid>],
    nav: &[WmoGroupNav],
    eye: [f32; 3],
    terrain_z: Option<f32>,
    outdoor_mask: u32,
    up: bool,
) -> Option<DownRayClaim> {
    // Candidates are per face: the client's query has no bbox, portals or camera (`0x6a8a20`).
    // The broad phase is the AABB of the faces themselves (`group_collision_bounds`), never a
    // group's authored box, which can float above its own floor; so the cull is exact.
    let column_owned = |gi: usize| {
        bounds.get(gi).copied().flatten().is_some_and(|(min, max)| {
            eye[0] >= min[0]
                && eye[0] <= max[0]
                && eye[1] >= min[1]
                && eye[1] <= max[1]
                && if up {
                    max[2] >= eye[2]
                } else {
                    min[2] <= eye[2]
                }
        })
    };
    let mut best_z = if up { f32::INFINITY } else { f32::NEG_INFINITY };
    let mut best: Option<usize> = None;
    for (gi, group_tris) in tris.iter().enumerate() {
        if !column_owned(gi) {
            continue;
        }
        // Narrow phase: the column index, else every face. The index keeps ascending order, so an
        // exact-`z` tie resolves first-wins as the linear scan does.
        let mut consider = |tri: &[[f32; 3]; 3]| {
            if let Some(z) = floor_z_at(tri, eye[0], eye[1]) {
                let ahead = if up { z >= eye[2] } else { z <= eye[2] };
                let nearer = if up { z < best_z } else { z > best_z };
                if ahead && nearer {
                    best_z = z;
                    best = Some(gi);
                }
            }
        };
        match grids.get(gi).and_then(Option::as_ref) {
            Some(grid) => {
                for i in grid.candidates(eye[0], eye[1]) {
                    if let Some(tri) = group_tris.get(i) {
                        consider(tri);
                    }
                }
            }
            None => group_tris.iter().for_each(&mut consider),
        }
    }
    let in_group = best?;
    // Beyond the ray: no claim.
    let depth = if up { best_z - eye[2] } else { eye[2] - best_z };
    if depth > ZONE_RAY_LEN {
        return None;
    }
    // The terrain race: strictly nearer terrain wins the column; a tie keeps the WMO
    // (`0x6a8b15`, `t_wmo <= t_terr`).
    if terrain_z.is_some_and(|tz| tz <= eye[2] && tz > best_z) {
        return None;
    }
    // The winner's class under the caller's law (`0x6a87f0` against `outdoor_mask`).
    let outdoor = nav
        .get(in_group)
        .is_none_or(|g| g.flags & outdoor_mask != 0);
    Some(DownRayClaim {
        group: in_group,
        depth,
        outdoor,
    })
}

/// Point-in-polygon with the plane's dominant axis projected out (the client's `0x7c23e0`), for a
/// point on or near the plane.
fn point_in_poly_dominant(verts: &[[f32; 3]], plane: [f32; 4], p: [f32; 3]) -> bool {
    let (u, v) = dominant_axes(plane);
    point_in_poly_2d(verts.iter().map(|q| (q[u], q[v])), (p[u], p[v]))
}

/// The two axes spanning a plane's dominant-axis projection (drop the normal's largest component).
pub(super) fn dominant_axes(plane: [f32; 4]) -> (usize, usize) {
    let [nx, ny, nz, _] = plane;
    if nx.abs() >= ny.abs() && nx.abs() >= nz.abs() {
        (1, 2)
    } else if ny.abs() >= nz.abs() {
        (0, 2)
    } else {
        (0, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{EXTERIOR, EXTERIOR_LIT};
    use super::*;

    fn nav(
        flags: u32,
        min: [f32; 3],
        max: [f32; 3],
        ref_start: u16,
        ref_count: u16,
    ) -> WmoGroupNav {
        WmoGroupNav {
            flags,
            bbox_min: min,
            bbox_max: max,
            ref_start,
            ref_count,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: benilla_formats::NO_GROUP_LIQUID,
        }
    }

    /// A horizontal floor quad (two triangles, radius `r`) at height `z`, WoW-local.
    fn quad(z: f32, r: f32) -> Vec<[[f32; 3]; 3]> {
        vec![
            [[-r, -r, z], [r, -r, z], [r, r, z]],
            [[-r, -r, z], [r, r, z], [-r, r, z]],
        ]
    }

    /// `down_ray_pick` with no portal graph and no terrain (the face-only leg), in-group only.
    fn faces_only(
        tris: &[Vec<[[f32; 3]; 3]>],
        nav: &[WmoGroupNav],
        eye: [f32; 3],
    ) -> Option<usize> {
        down_ray_pick(
            tris,
            &benilla_assets::collision_tri_grids(tris),
            &[],
            nav,
            &[],
            &[],
            &[],
            eye,
            None,
        )
        .in_group
    }

    /// `down_ray_pick` with a portal graph and no terrain.
    fn no_terrain(
        tris: &[Vec<[[f32; 3]; 3]>],
        nav: &[WmoGroupNav],
        portal_vertices: &[[f32; 3]],
        portal_infos: &[WmoPortalInfo],
        portal_refs: &[WmoPortalRef],
        eye: [f32; 3],
    ) -> DownRaySeeds {
        down_ray_pick(
            tris,
            &benilla_assets::collision_tri_grids(tris),
            &[],
            nav,
            portal_vertices,
            portal_infos,
            portal_refs,
            eye,
            None,
        )
    }

    #[test]
    fn face_leg_picks_the_group_of_the_nearest_face_below() {
        // A tall outer group (floor z=0) with a small room nested inside it (floor z=10).
        let outer = nav(0, [-30.0, -30.0, 0.0], [30.0, 30.0, 100.0], 0, 0);
        let room = nav(0, [-5.0, -5.0, 10.0], [5.0, 5.0, 16.0], 0, 0);
        let groups = [outer, room];
        let tris = [quad(0.0, 30.0), quad(10.0, 5.0)];
        assert_eq!(faces_only(&tris, &groups, [0.0, 0.0, 12.0]), Some(1));
        // Below the room floor: only the outer floor is below.
        assert_eq!(faces_only(&tris, &groups, [0.0, 0.0, 5.0]), Some(0));
        assert_eq!(faces_only(&tris, &groups, [20.0, 20.0, 5.0]), Some(0));
        // Past MAX_FLOOR_DROP: outside.
        assert_eq!(faces_only(&tris, &groups, [0.0, 0.0, 5000.0]), None);
    }

    #[test]
    fn face_leg_is_orientation_agnostic() {
        // A near-wall ramp face (|n.z|/|n| ≈ 0.1) still owns the column: the down-ray's segment
        // query, run with the walking mask `0x84`, has no normal filter in its leaf (`0x6bc700`).
        let g = nav(0, [-10.0, -10.0, 0.0], [10.0, 10.0, 120.0], 0, 0);
        let steep = vec![[[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 20.0]]];
        assert_eq!(faces_only(&[steep], &[g], [0.0, 0.0, 30.0]), Some(0));
    }

    #[test]
    fn face_leg_exterior_face_reads_as_outside() {
        let ext = nav(EXTERIOR, [-30.0, -30.0, 0.0], [30.0, 30.0, 100.0], 0, 0);
        assert_eq!(
            faces_only(&[quad(0.0, 30.0)], &[ext], [0.0, 0.0, 5.0]),
            None
        );
    }

    /// A horizontal portal quad (radius `r`) at height `z`, normal +Z, from vertex 0.
    fn horizontal_portal(z: f32, r: f32) -> (Vec<[f32; 3]>, WmoPortalInfo) {
        let verts = vec![[-r, -r, z], [r, -r, z], [r, r, z], [-r, r, z]];
        let info = WmoPortalInfo {
            start_vertex: 0,
            count: 4,
            plane: [0.0, 0.0, 1.0, -z],
        };
        (verts, info)
    }

    /// Per-group triangle sets, as the model stores them.
    type GroupTris = [Vec<[[f32; 3]; 3]>; 2];

    /// The Goldshire stairs: group 0 is the stairwell (floor z=0), group 1 the taproom whose
    /// floor-hole portal sits at z=10 over the stairwell.
    fn stairwell() -> ([WmoGroupNav; 2], GroupTris) {
        let stair = nav(0, [-5.0, -5.0, 0.0], [5.0, 5.0, 11.0], 0, 1);
        let taproom = nav(0, [-20.0, -20.0, 10.0], [20.0, 20.0, 20.0], 1, 1);
        ([stair, taproom], [quad(0.0, 5.0), Vec::new()])
    }

    #[test]
    fn portal_crossing_flips_the_group_and_seeds_both_sides() {
        let (groups, tris) = stairwell();
        let (pverts, pinfo) = horizontal_portal(10.0, 5.0);
        let infos = [pinfo];
        let refs = [
            WmoPortalRef {
                portal: 0,
                group: 1,
                side: -1,
            }, // stair's ref: stair is on the −normal side
            WmoPortalRef {
                portal: 0,
                group: 0,
                side: 1,
            }, // taproom's ref: taproom is on the +normal side
        ];
        // Eye above the hole (z=12): the crossing beats the stair floor, the side pick names the
        // taproom, and the stairwell rides along as the across-group.
        assert_eq!(
            no_terrain(&tris, &groups, &pverts, &infos, &refs, [0.0, 0.0, 12.0]),
            DownRaySeeds {
                in_group: Some(1),
                across: Some(0)
            }
        );
        // Eye below the plane (z=5): the face leg alone, the stairs with no across-group.
        assert_eq!(
            no_terrain(&tris, &groups, &pverts, &infos, &refs, [0.0, 0.0, 5.0]),
            DownRaySeeds {
                in_group: Some(0),
                across: None
            }
        );
        // Above the plane but outside the hole: no crossing, no floor, outside.
        assert_eq!(
            no_terrain(&tris, &groups, &pverts, &infos, &refs, [10.0, 10.0, 12.0]),
            DownRaySeeds::default()
        );
    }

    #[test]
    fn portal_crossing_respects_the_side_orientation() {
        // The same stairwell with the portal normal pointing down (−Z): the side flags mirror.
        let (groups, tris) = stairwell();
        let verts = vec![
            [-5.0, -5.0, 10.0],
            [5.0, -5.0, 10.0],
            [5.0, 5.0, 10.0],
            [-5.0, 5.0, 10.0],
        ];
        let infos = [WmoPortalInfo {
            start_vertex: 0,
            count: 4,
            plane: [0.0, 0.0, -1.0, 10.0], // −z + 10 = 0: below the plane is the + side
        }];
        let refs = [
            WmoPortalRef {
                portal: 0,
                group: 1,
                side: 1,
            }, // stair's ref: stair on the +normal (lower) side
            WmoPortalRef {
                portal: 0,
                group: 0,
                side: -1,
            },
        ];
        assert_eq!(
            no_terrain(&tris, &groups, &verts, &infos, &refs, [0.0, 0.0, 12.0]).in_group,
            Some(1)
        );
        assert_eq!(
            no_terrain(&tris, &groups, &verts, &infos, &refs, [0.0, 0.0, 5.0]).in_group,
            Some(0)
        );
    }

    #[test]
    fn coincident_portal_beats_the_slab_face_by_the_tie_eps() {
        // The slab's collision face and the floor-hole portal share z=10: the portal's side pick
        // (the taproom) wins the tie by the client's ~1e-4 nearest-accept.
        let (groups, mut tris) = stairwell();
        tris[0].extend(quad(10.0, 5.0)); // a slab face owned by the stair group, coincident with p0
        let (pverts, pinfo) = horizontal_portal(10.0, 5.0);
        let refs = [
            WmoPortalRef {
                portal: 0,
                group: 1,
                side: -1,
            },
            WmoPortalRef {
                portal: 0,
                group: 0,
                side: 1,
            },
        ];
        assert_eq!(
            no_terrain(&tris, &groups, &pverts, &[pinfo], &refs, [0.0, 0.0, 12.0]).in_group,
            Some(1)
        );
    }

    #[test]
    fn vertical_doorway_snaps_only_within_the_window() {
        // A vertical doorway in the x=0 plane between room A (x<0) and room B (x>0), with room A's
        // floor running past it to x=0.5: the face under the column lags the room boundary.
        let room_a = nav(0, [-10.0, -10.0, 0.0], [0.5, 10.0, 10.0], 0, 1);
        let room_b = nav(0, [-0.5, -10.0, 0.0], [10.0, 10.0, 10.0], 1, 1);
        let groups = [room_a, room_b];
        let tris = [
            vec![
                [[-10.0, -10.0, 0.0], [0.5, -10.0, 0.0], [0.5, 10.0, 0.0]],
                [[-10.0, -10.0, 0.0], [0.5, 10.0, 0.0], [-10.0, 10.0, 0.0]],
            ],
            vec![
                [[0.5, -10.0, 0.0], [10.0, -10.0, 0.0], [10.0, 10.0, 0.0]],
                [[0.5, -10.0, 0.0], [10.0, 10.0, 0.0], [0.5, 10.0, 0.0]],
            ],
        ];
        let verts = vec![
            [0.0, -2.0, 0.0],
            [0.0, 2.0, 0.0],
            [0.0, 2.0, 4.0],
            [0.0, -2.0, 4.0],
        ];
        let infos = [WmoPortalInfo {
            start_vertex: 0,
            count: 4,
            plane: [1.0, 0.0, 0.0, 0.0], // +normal points into room B (x > 0)
        }];
        let refs = [
            WmoPortalRef {
                portal: 0,
                group: 1,
                side: -1,
            }, // room A is on the −normal side
            WmoPortalRef {
                portal: 0,
                group: 0,
                side: 1,
            }, // room B is on the +normal side
        ];
        // Eye 0.05 into room B, within the 0.1 yd snap: the doorway is crossed at the eye and beats
        // room A's floor; B is the in-group and A rides along.
        assert_eq!(
            no_terrain(&tris, &groups, &verts, &infos, &refs, [0.05, 0.0, 1.7]),
            DownRaySeeds {
                in_group: Some(1),
                across: Some(0)
            }
        );
        // Eye 0.3 in, beyond the snap: the doorway is not crossed and room A's floor decides. Real
        // collision meshes track the room boundary to sub-yard (`0x6b92b0`).
        assert_eq!(
            no_terrain(&tris, &groups, &verts, &infos, &refs, [0.3, 0.0, 1.7]),
            DownRaySeeds {
                in_group: Some(0),
                across: None
            }
        );
    }

    /// `area_down_ray` on the zone-text law (`0x8` alone), with the loader's own face bounds.
    fn area_ray(
        tris: &[Vec<[[f32; 3]; 3]>],
        nav: &[WmoGroupNav],
        eye: [f32; 3],
        terrain_z: Option<f32>,
    ) -> Option<usize> {
        let (bounds, _) = benilla_assets::collision_tri_bounds(tris);
        let grids = benilla_assets::collision_tri_grids(tris);
        area_down_ray(tris, &bounds, &grids, nav, eye, terrain_z, EXTERIOR)
    }

    /// A body planted under its floor is invisible to the position cast; `room_at`'s lifted second
    /// pass finds the same floor, and reaches no further than the tolerance.
    #[test]
    fn an_under_floor_origin_is_found_only_by_the_lifted_cast() {
        let floor = [quad(0.0, 20.0)];
        let nav = [nav(0, [-20.0, -20.0, 0.0], [20.0, 20.0, 0.0], 0, 0)];
        let lift = super::super::interior::POSITION_PROBE_LIFT;
        let tol = super::super::interior::ROOM_UNDER_FLOOR_TOLERANCE;
        // Orgrimmar's measured burials, 1.16 yd the worst (bonfire 177026).
        for buried in [0.45_f32, 1.16] {
            let feet = -buried;
            assert_eq!(
                area_ray(&floor, &nav, [0.0, 0.0, feet + lift], None),
                None,
                "the position cast must miss a floor above the origin ({buried} yd)"
            );
            assert_eq!(
                area_ray(&floor, &nav, [0.0, 0.0, feet + lift + tol], None),
                Some(0),
                "the under-floor fallback must find it ({buried} yd)"
            );
        }
        // A floor further above than the tolerance stays unclaimed: no reach to the storey above.
        assert_eq!(
            area_ray(&floor, &nav, [0.0, 0.0, -(tol + 1.0) + lift + tol], None),
            None,
            "a floor beyond the tolerance must stay unclaimed"
        );
    }

    /// The column index never changes a down-ray claim: group, depth and outdoor class all match
    /// the linear scan, column for column.
    #[test]
    fn down_ray_column_index_is_exact() {
        // Three stacked storeys over the same XY range, so every group owns every column.
        let storey = |z: f32| -> Vec<[[f32; 3]; 3]> {
            let mut out = Vec::new();
            for ix in 0..10 {
                for iy in 0..10 {
                    let (x, y) = (ix as f32 * 3.0, iy as f32 * 3.0);
                    // Vary z within the storey so the winning tile is observable.
                    let tz = z + ((ix + iy) % 4) as f32 * 0.25;
                    out.push([[x, y, tz], [x + 3.0, y, tz], [x + 3.0, y + 3.0, tz]]);
                    out.push([[x, y, tz], [x + 3.0, y + 3.0, tz], [x, y + 3.0, tz]]);
                }
            }
            out
        };
        let tris = vec![storey(0.0), storey(8.0), storey(16.0)];
        let navs: Vec<WmoGroupNav> = (0..3)
            .map(|i| {
                nav(
                    0,
                    [0.0, 0.0, i as f32 * 8.0],
                    [30.0, 30.0, i as f32 * 8.0 + 8.0],
                    0,
                    0,
                )
            })
            .collect();
        let (bounds, _) = benilla_assets::collision_tri_bounds(&tris);
        let grids = benilla_assets::collision_tri_grids(&tris);
        assert!(
            grids.iter().all(Option::is_some),
            "each storey must actually be indexed"
        );
        let mut hits = 0;
        for gx in -1..=32 {
            for gy in -1..=32 {
                for eye_z in [1.0f32, 9.5, 17.0, 25.0] {
                    let eye = [gx as f32 * 0.97, gy as f32 * 1.03, eye_z];
                    let linear = down_ray_claim(&tris, &bounds, &[], &navs, eye, None, EXTERIOR);
                    let indexed =
                        down_ray_claim(&tris, &bounds, &grids, &navs, eye, None, EXTERIOR);
                    assert_eq!(
                        linear, indexed,
                        "column {eye:?}: the index changed the down-ray claim"
                    );
                    hits += usize::from(linear.is_some());
                }
            }
        }
        assert!(
            hits > 1000,
            "the sweep must land on the storeys, got {hits}"
        );
    }

    #[test]
    fn area_ray_face_bounds_survive_a_lying_authored_box() {
        // Northshire Abbey group 3: the authored box bottom (z 1.84) floats above the group's own
        // floor (z 0.3), so a position probe (z 0.4) is under the box. Any authored-box cull fails.
        let lying_box = nav(0, [-10.0, -10.0, 1.84], [10.0, 10.0, 20.0], 0, 0);
        let tris = [quad(0.3, 10.0)]; // the group's real floor, below its authored box
        assert_eq!(
            area_ray(&tris, &[lying_box], [0.0, 0.0, 0.4], None),
            Some(0)
        );
    }

    #[test]
    fn area_ray_broad_phase_is_exactly_conservative() {
        // Two groups side by side: the broad phase never changes an answer.
        let g0 = nav(0, [-10.0, -10.0, 0.0], [0.0, 10.0, 10.0], 0, 0);
        let g1 = nav(0, [5.0, -10.0, 0.0], [15.0, 10.0, 10.0], 0, 0);
        let west = vec![
            [[-10.0, -10.0, 0.0], [0.0, -10.0, 0.0], [0.0, 10.0, 0.0]],
            [[-10.0, -10.0, 0.0], [0.0, 10.0, 0.0], [-10.0, 10.0, 0.0]],
        ];
        let east = vec![
            [[5.0, -10.0, 2.0], [15.0, -10.0, 2.0], [15.0, 10.0, 2.0]],
            [[5.0, -10.0, 2.0], [15.0, 10.0, 2.0], [5.0, 10.0, 2.0]],
        ];
        let tris = [west, east];
        let navs = [g0, g1];
        assert_eq!(area_ray(&tris, &navs, [-5.0, 0.0, 1.0], None), Some(0));
        assert_eq!(area_ray(&tris, &navs, [10.0, 0.0, 3.0], None), Some(1));
        // In the gap between the buildings: no face owns the column.
        assert_eq!(area_ray(&tris, &navs, [2.5, 0.0, 5.0], None), None);
        // Over a group but below its faces.
        assert_eq!(area_ray(&tris, &navs, [10.0, 0.0, 1.0], None), None);
    }

    #[test]
    fn area_ray_exterior_and_terrain_race_hold() {
        let ext = nav(EXTERIOR, [-10.0, -10.0, 0.0], [10.0, 10.0, 10.0], 0, 0);
        assert_eq!(
            area_ray(&[quad(0.0, 10.0)], &[ext], [0.0, 0.0, 1.0], None),
            None
        );
        let int = nav(0, [-10.0, -10.0, 0.0], [10.0, 10.0, 10.0], 0, 0);
        assert_eq!(
            area_ray(&[quad(0.0, 10.0)], &[int], [0.0, 0.0, 1.0], Some(0.5)),
            None
        );
        assert_eq!(
            area_ray(&[quad(0.0, 10.0)], &[int], [0.0, 0.0, 1.0], Some(0.0)),
            Some(0)
        );
    }

    #[test]
    fn exterior_lit_group_splits_the_two_laws() {
        // A Stormwind street: a `0x40`-only group is indoors on the zone-text law (`0x8` alone)
        // but outdoors on the unit-lighting law (`MOGI & 0x48`).
        let street = nav(EXTERIOR_LIT, [-10.0, -10.0, 0.0], [10.0, 10.0, 10.0], 0, 0);
        let tris = [quad(0.0, 10.0)];
        let (bounds, _) = benilla_assets::collision_tri_bounds(&tris);
        let eye = [0.0, 0.0, 1.0];
        assert_eq!(
            area_down_ray(&tris, &bounds, &[], &[street], eye, None, EXTERIOR),
            Some(0),
            "zone-text law: indoors"
        );
        assert_eq!(
            area_down_ray(
                &tris,
                &bounds,
                &[],
                &[street],
                eye,
                None,
                EXTERIOR | EXTERIOR_LIT
            ),
            None,
            "lighting law: outdoors"
        );
    }

    #[test]
    fn the_upward_retry_recovers_a_floor_from_beneath_it() {
        // An Onyxia lava trap anchors about 40 yd under its chamber floor (its authored box reaches
        // 98 yd past its geometry): the downward cast finds nothing and the upward retry
        // (`0x6a908d`) finds the floor.
        let room = nav(0, [-10.0, -10.0, 0.0], [10.0, 10.0, 10.0], 0, 0);
        let tris = [quad(0.0, 10.0)]; // the chamber floor at z = 0
        let (bounds, _) = benilla_assets::collision_tri_bounds(&tris);
        let mask = EXTERIOR | EXTERIOR_LIT;
        let under = [0.0, 0.0, -40.0];
        assert_eq!(
            down_ray_claim(&tris, &bounds, &[], &[room], under, None, mask),
            None,
            "nothing is below an anchor that is itself below the floor"
        );
        let c = up_ray_claim(&tris, &bounds, &[], &[room], under, mask)
            .expect("the floor above claims the column on the retry");
        assert!(!c.outdoor && c.group == 0);
        assert!(
            (c.depth - 40.0).abs() < 1e-6,
            "depth is measured along the cast's own direction, got {}",
            c.depth
        );
        // The retry is still a ray: a face below the probe never claims it.
        assert_eq!(
            up_ray_claim(&tris, &bounds, &[], &[room], [0.0, 0.0, 40.0], mask),
            None
        );
    }

    #[test]
    fn down_ray_claim_distinguishes_deck_from_terrain() {
        // A Booty Bay boardwalk: the claim tells an outdoor-class face from open terrain, which
        // `area_down_ray` collapses.
        let deck = nav(
            EXTERIOR | EXTERIOR_LIT,
            [-10.0, -10.0, 0.0],
            [10.0, 10.0, 10.0],
            0,
            0,
        );
        let tris = [quad(5.0, 10.0)]; // the deck surface, 30-ish yd over terrain at z -25
        let (bounds, _) = benilla_assets::collision_tri_bounds(&tris);
        let mask = EXTERIOR | EXTERIOR_LIT;
        // On the deck, terrain far below: an outdoor claim, depth = probe − face.
        let c = down_ray_claim(
            &tris,
            &bounds,
            &[],
            &[deck],
            [0.0, 0.0, 5.5],
            Some(-25.0),
            mask,
        )
        .expect("the deck face claims the column");
        assert!(c.outdoor && c.group == 0);
        assert!((c.depth - 0.5).abs() < 1e-6);
        // Terrain strictly nearer, over a buried deck: no claim.
        assert_eq!(
            down_ray_claim(
                &tris,
                &bounds,
                &[],
                &[deck],
                [0.0, 0.0, 8.0],
                Some(7.0),
                mask
            ),
            None
        );
        let room = nav(0, [-10.0, -10.0, 0.0], [10.0, 10.0, 10.0], 0, 0);
        let c = down_ray_claim(&tris, &bounds, &[], &[room], [0.0, 0.0, 5.5], None, mask)
            .expect("the room face claims the column");
        assert!(!c.outdoor);
    }

    #[test]
    fn floor_z_at_interpolates_inside_and_rejects_outside() {
        let flat = [[0.0, 0.0, 4.0], [10.0, 0.0, 4.0], [0.0, 10.0, 4.0]];
        assert_eq!(floor_z_at(&flat, 1.0, 1.0), Some(4.0)); // inside the triangle
        assert!(floor_z_at(&flat, 9.0, 9.0).is_none()); // outside (x+y > 10)
        let slope = [[0.0, 0.0, 0.0], [10.0, 0.0, 10.0], [0.0, 10.0, 0.0]];
        assert_eq!(floor_z_at(&slope, 5.0, 0.0), Some(5.0)); // halfway up the ramp
        let vertical = [[0.0, 0.0, 0.0], [0.0, 0.0, 10.0], [10.0, 0.0, 5.0]];
        assert!(floor_z_at(&vertical, 0.0, 0.0).is_none()); // degenerate in XY
    }

    #[test]
    fn terrain_race_drops_a_buried_interior_but_spares_one_under_the_hill() {
        // A mine tunnel with its floor at z=0, its box to z=10.
        let mine = nav(0, [-20.0, -20.0, 0.0], [20.0, 20.0, 10.0], 0, 0);
        let tris = [quad(0.0, 20.0)];
        // A camera 2 yd over the tunnel floor, no terrain: inside.
        assert_eq!(
            down_ray_pick(
                &tris,
                &[],
                &[],
                &[mine],
                &[],
                &[],
                &[],
                [0.0, 0.0, 2.0],
                None
            )
            .in_group,
            Some(0)
        );
        // Hillside terrain at z=8 is above the eye, off the segment: the tunnel stays inside.
        assert_eq!(
            down_ray_pick(
                &tris,
                &[],
                &[],
                &[mine],
                &[],
                &[],
                &[],
                [0.0, 0.0, 2.0],
                Some(8.0)
            )
            .in_group,
            Some(0)
        );
        // A camera 12 yd up over ground at z=8, above the tunnel floor: the terrain wins, outside.
        assert_eq!(
            down_ray_pick(
                &tris,
                &[],
                &[],
                &[mine],
                &[],
                &[],
                &[],
                [0.0, 0.0, 12.0],
                Some(8.0)
            )
            .in_group,
            None
        );
        // Terrain exactly at the WMO floor keeps the WMO (the client's strict `<`).
        assert_eq!(
            down_ray_pick(
                &tris,
                &[],
                &[],
                &[mine],
                &[],
                &[],
                &[],
                [0.0, 0.0, 2.0],
                Some(0.0)
            )
            .in_group,
            Some(0)
        );
        // Terrain just above the floor steals the column.
        assert_eq!(
            down_ray_pick(
                &tris,
                &[],
                &[],
                &[mine],
                &[],
                &[],
                &[],
                [0.0, 0.0, 2.0],
                Some(0.1)
            )
            .in_group,
            None
        );
    }

    #[test]
    fn camera_void_fallback_names_the_detail_floored_room() {
        // The Deadmines pocket: an interior group floored only with DETAIL faces, which are in the
        // camera-only set and not the walking set.
        let pocket = nav(0, [-10.0, -10.0, 0.0], [10.0, 10.0, 12.0], 0, 0);
        let walk: [Vec<[[f32; 3]; 3]>; 1] = [Vec::new()];
        let detail = [quad(0.0, 10.0)];
        // Legs A and B miss, no terrain below: Leg C names the room, a single root.
        let s = down_ray_pick(
            &walk,
            &[],
            &detail,
            &[pocket],
            &[],
            &[],
            &[],
            [0.0, 0.0, 2.0],
            None,
        );
        assert_eq!(s.in_group, Some(0));
        assert_eq!(s.across, None);
        // Terrain above the eye is off the segment: Leg C still fires.
        assert_eq!(
            down_ray_pick(
                &walk,
                &[],
                &detail,
                &[pocket],
                &[],
                &[],
                &[],
                [0.0, 0.0, 2.0],
                Some(8.0)
            )
            .in_group,
            Some(0)
        );
    }

    #[test]
    fn camera_void_fallback_defers_to_terrain_and_to_the_walking_leg() {
        let pocket = nav(0, [-10.0, -10.0, 0.0], [10.0, 10.0, 12.0], 0, 0);
        let detail = [quad(0.0, 10.0)];
        // Any terrain at or below the eye keeps the client's outside verdict.
        assert_eq!(
            down_ray_pick(
                &[Vec::new()],
                &[],
                &detail,
                &[pocket],
                &[],
                &[],
                &[],
                [0.0, 0.0, 2.0],
                Some(1.9)
            )
            .in_group,
            None
        );
        // A walking-leg answer, even a lower floor of another group, pre-empts Leg C.
        let hall = nav(0, [-30.0, -30.0, -5.0], [30.0, 30.0, 12.0], 0, 0);
        let walk = [Vec::new(), quad(-5.0, 30.0)];
        let det2: [Vec<[[f32; 3]; 3]>; 2] = [quad(0.0, 10.0), Vec::new()];
        assert_eq!(
            down_ray_pick(
                &walk,
                &[],
                &det2,
                &[pocket, hall],
                &[],
                &[],
                &[],
                [0.0, 0.0, 2.0],
                None
            )
            .in_group,
            Some(1)
        );
        // An EXTERIOR group's detail surface reads as outside, as in Leg A.
        let ext = nav(EXTERIOR, [-10.0, -10.0, 0.0], [10.0, 10.0, 12.0], 0, 0);
        assert_eq!(
            down_ray_pick(
                &[Vec::new()],
                &[],
                &detail,
                &[ext],
                &[],
                &[],
                &[],
                [0.0, 0.0, 2.0],
                None
            )
            .in_group,
            None
        );
    }
}
