//! The current-group **down-ray** — which room is the camera in, and which flood roots follow.
//!
//! The faithful port of the client's per-frame probe
//! (`FUN_006821f0` → `FUN_006be250` → `0x6a3f80`): a vertical ray from the eye down `1760` yd races
//! two legs, nearest crossing wins —
//!
//! - **Leg A — walking-collision faces** (the client's collision BSP, MOPY mask `0x84`): every
//!   non-DETAIL face, **no orientation filter** — stair treads, risers, ledges, sloped trim all count.
//!   A render-face `|n.z|` floor heuristic here is exactly what mis-seeded doorways (the straddle) and
//!   slab edges: the real face set tracks the room boundary to sub-yard, the proxy lagged it.
//! - **Leg B — portal crossings** (`0x6a3f80`): where the ray crosses a portal polygon, the group is
//!   picked by the eye's **side of the portal plane** (`group_a iff (signed_dist ≥ 0) == (side > 0)`,
//!   else the neighbour) — the vertically-stacked tie-break (a stairwell's floor-hole portal flips the
//!   room the instant the eye crosses the plane). A near-parallel portal (a vertical doorway vs the
//!   vertical ray, `|denom| < 1e-4`) counts as crossed only while the eye is within the `0.1`-yd snap
//!   window of its plane (`0x7c22b0`'s parallel branch). A crossing within `~1e-4` of the nearest face
//!   hit still wins (`0x80c4f4`) — a floor-hole portal coincident with the slab's faces flips the room.
//!
//! The verdict is a **set**, not a single group: the in-group plus (for a portal win) the
//! across-group, both appended to the client's visible-group set `0xc7cd88` and each flooded as an
//! independent root (`0x6b3bd4`–`0x6b3c10`). An exterior winner or no crossing within the ray ⇒
//! outside (empty set, the outside leg).
//!
//! - **The terrain race** (`terrain_z`): the same down-segment is cast against the **terrain** — the
//!   client runs both probes and drops the WMO hit when the ground is *strictly* nearer
//!   (`FUN_006821f0` `0x6822a2 fld t_terrain; fcomp t_wmo; test ah,5; jp` — falls through to clear the
//!   WMO flag only on `t_terrain < t_wmo`; `GetAreaID 0x670250` arbitrates identically at `0x670345`).
//!   Equal keeps the WMO. Without this leg a camera standing on open ground **above a buried interior**
//!   — a mine dug into a hillside — reads "inside a tunnel" and the flood culls the building out from
//!   under it (it is what 0233 deferred). Terrain *above* the eye is not on the
//!   down-segment at all, which is exactly what keeps a real tunnel reading INSIDE, and a column in an
//!   MCNK hole has no terrain surface to hit, which is how a mine entrance stays walkable.
//!
//! - **Leg C — the camera-void fallback** (**ours**, not the client's): only when Legs
//!   A and B both miss AND no terrain surface sits at or below the eye's column, the same race runs
//!   once more over the **camera-only** faces (DETAIL set, NOCAMCOLLIDE clear — the faces that stop
//!   the camera but never carried the walking BSP). The client returns "outside" here and blanks the
//!   building — reproducible in 5875 at the Deadmines entrance, whose behind-the-portal pocket is
//!   floored entirely with DETAIL faces: the camera is sealed in by its own collision yet the flood
//!   loses it. An eye that camera-collision itself proves is between interior surfaces, under ground
//!   level, names the room that owns the surface below it instead. The gates keep the divergence out
//!   of every place the client's verdict is *right*: any terrain at/below the eye (doorsteps, streets,
//!   open country) or any Leg A/B result leaves the faithful answer untouched.

use benilla_assets::column_grid::ColumnGrid;
use benilla_assets::{WmoGroupNav, WmoModel, WmoPortalInfo, WmoPortalRef};
pub use benilla_formats::triangle_z_at as floor_z_at;

use super::{
    point_in_poly_2d, portal_poly, EXTERIOR, MAX_FLOOR_DROP, NEAREST_TIE_EPS, PORTAL_NEAR_PARALLEL,
    PORTAL_PLANE_SNAP,
};

/// The down-ray's verdict: the camera's current group, plus — when the nearest crossing below the eye
/// was a portal — the group on that portal's other side. Both become flood roots (the client's
/// visible-group set `0xc7cd88`, cardinality 0/1/2). `in_group == None` ⇒ outside (and `across` is
/// `None` too — the client appends nothing when the instance is cleared).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct DownRaySeeds {
    pub in_group: Option<usize>,
    pub(crate) across: Option<usize>,
}

/// [`down_ray_pick`] from the asset's stored pieces. `terrain_z` is the terrain surface height in
/// **this model's local space** under the eye's column (see [`down_ray_pick`]).
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

/// The eye's current group (see the module doc for the mechanism and its provenance).
///
/// `terrain_z` is the ADT terrain surface height under the eye's column, expressed in **WMO model
/// space** (the down-ray's own frame), or `None` where there is no terrain surface to hit — off the
/// streamed tiles, or a hole cut through the ground into this very interior. It is the client's second
/// probe, and it wins the column whenever it is *strictly* nearer to the eye than the WMO's hit.
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
    // The group broad-phase the whole probe shares: the eye's column must fall within the group's XY
    // bounds, and the group must reach at/below the eye.
    let in_column = |g: &WmoGroupNav| {
        eye[0] >= g.bbox_min[0]
            && eye[0] <= g.bbox_max[0]
            && eye[1] >= g.bbox_min[1]
            && eye[1] <= g.bbox_max[1]
            && g.bbox_min[2] <= eye[2]
    };

    // Leg A — walking-collision faces: the highest crossing of the column at or below the eye.
    let mut best_z = f32::NEG_INFINITY;
    let mut best: Option<usize> = None;
    for (gi, g) in nav.iter().enumerate() {
        if !in_column(g) {
            continue;
        }
        // Narrowed by the group's column index where it has one — same faces, same order (see
        // `down_ray_claim`).
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

    // Leg B — portal crossings, racing the face hit with the tie-break (a coincident portal wins).
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
                // Near-parallel (a vertical doorway vs the vertical ray): crossed only while the eye
                // is embedded in the plane — the 0.1-yd snap window — and then at the eye itself.
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
            // The plane-side pick (client `0x6a4130..0x6a4159`): the eye's side of the portal plane,
            // oriented by `side`, chooses between the walked group and the neighbour.
            let d_signed = nx * eye[0] + ny * eye[1] + nz * eye[2] + d;
            let chosen = if (d_signed >= 0.0) == (r.side > 0) {
                gi
            } else {
                r.group as usize
            };
            // A dead neighbour ref (`0xffff`) can't be a room — leave the crossing to the other
            // side's ref (same portal, mirrored), or to the face leg.
            if nav.get(chosen).is_none() {
                continue;
            }
            let other = if chosen == gi { r.group as usize } else { gi };
            best_z = z;
            best = Some(chosen);
            across = (nav.get(other).is_some() && other != chosen).then_some(other);
        }
    }

    // Outside: nothing within the ray length, or the winning surface belongs to an exterior group
    // (the client clears the instance for either — and then appends no seeds).
    let Some(in_group) = best else {
        // Leg C — the camera-void fallback (ours, the module doc has the case).
        // Fires only when BOTH faithful legs found nothing AND no terrain surface sits at or below
        // the eye: an under-ground eye with a camera-collidable DETAIL surface below it is between
        // interior surfaces the camera itself cannot leave, and blanking the building there is the
        // client artifact the director rejected. Same race, same broad phase, same exterior and
        // ray-length rules as Leg A; a single root, no `across` (no portal was crossed).
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
    // The terrain race. A terrain surface above the eye was never crossed by the downward segment, so
    // it is not a hit at all (this is what keeps a tunnel under a hill reading INSIDE). A terrain hit
    // that is **strictly** nearer than the WMO's — i.e. sits above it — means the eye is standing over
    // open ground: outside. A tie keeps the WMO, matching the client's strict `<`.
    if terrain_z.is_some_and(|tz| tz <= eye[2] && tz > best_z) {
        return DownRaySeeds::default();
    }
    DownRaySeeds {
        in_group: Some(in_group),
        across,
    }
}

/// The ZONE-TEXT down-ray length: the client's `[0x8022cc] = 1000.0` — shorter than the render
/// current-group probe's 1760.
const ZONE_RAY_LEN: f32 = 1000.0;

/// The position-cast indoor predicate's ray — **faces only, no portal leg**: the CGLight node's
/// down-ray attach `0x6a8a20` casts the position straight down [`ZONE_RAY_LEN`] and races
/// {terrain hit, WMO face hit} — WMO wins ties
/// (`t_wmo <= t_terr`, `0x6a8b15`) — then classifies by the winning group's MOGP flags against
/// `outdoor_mask`: a masked flag → outdoors, else indoors. The classify `0x6a87f0` has TWO sinks
/// with DIFFERENT masks, so the caller picks its law:
///
/// - **zone-text/area** (`[node+0x90]` bit 0): [`EXTERIOR`] alone — a `0x40`-only street group is
///   "indoors" for area naming (the bit is set before the `0x40` test);
/// - **unit lighting class** (`[node+0xc]` bit 2/4): `EXTERIOR | `[`super::EXTERIOR_LIT`] — the
///   fork is `MOGI & 0x48` (`6a880d test al,0x8` / `6a8823 test al,0x40`, both → `or
///   [node+0xc],0x4`), so Stormwind/Orgrimmar street units light as sunlit outdoors (0475).
///
/// The portal-crossing leg of [`down_ray_pick`] belongs to the CAMERA's current-group system (the
/// render seed), **not** this predicate — running it here was the abbey-yard bug: a doorway plane
/// under the eye claimed the interior while the real client's face ray still saw terrain/exterior
/// pavement.
///
/// Returns the interior group index, or `None` = outdoors (no face, terrain won, or an
/// outdoor-class winner — [`down_ray_claim`] is the un-collapsed form that keeps the distinction).
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

/// A position-cast down-ray's un-collapsed claim on one placement: the winning face's group, its
/// depth below the probe, and the caller's-mask outdoor classification. `None` when no face is
/// within the ray or strictly-nearer terrain steals the column — this placement sees the position
/// standing over open ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DownRayClaim {
    pub(crate) group: usize,
    /// Depth of the winning face below the probe (yd). Placements are rigid (rotation +
    /// translation), so depths are comparable ACROSS placements — the entity light chain
    /// arbitrates its multi-placement winner by it (the client's one global nearest-hit).
    pub(crate) depth: f32,
    /// The winning group carries a flag in the caller's `outdoor_mask` (an outdoor-class surface).
    pub(crate) outdoor: bool,
}

/// [`area_down_ray`] without the outdoor collapse (see it for the mechanism + provenance): the
/// entity LIGHT chain needs to distinguish "outdoors because an EXTERIOR/EXTERIOR_LIT face won"
/// from "outdoors because only terrain is below" — a deck unit and a field unit take different
/// intensity laws.
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

/// The same claim cast **upward** — the containment lane's retry, and only its retry.
///
/// `0x6a8ed0` anchors a GameObject's light node at the world bounding-box CENTRE (`[node+0x5c]`,
/// decision 0776), and that box is the M2's **authored header** box — `0x713640` copies
/// `MD20+0xB4` into it, where the sibling `0x713700` copies the *collision* box at `+0xD0`. For a
/// particle-heavy model the authored box is enormous: `ONYZIASLAIRLAVATRAP.M2` authors one
/// reaching 98.42 yd past its own bind pose (`benilla-extract animboundscan`), putting the anchor
/// ~40 yd under the floor the object is standing on.
///
/// **The reference's own anchor is underground too**, and it recovers with a `+1000`-yd upward
/// retry on the face ray — `6a908d fld [ebp-0x38]` / `6a9093 fadd ds:0x8022cc`, inside the
/// containment body `0x6a8ed0` — which finds the floor from beneath it.
///
/// [`footprint_sample_above`](super::interior::footprint_sample_above) is the SAME retry one stage
/// later, and benilla had that half and not this one: 104 of Onyxia's lava traps never reached the
/// footprint at all, because the claim that gates it came back empty and the verdict was
/// "outdoors" before any colour was sampled.
///
/// **No terrain race here.** The downward cast loses its column to strictly-nearer terrain
/// (standing over open ground); a cast going *up* from under a floor cannot meaningfully lose to
/// the ground below it.
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

/// The shared body: `up` flips which side of the probe a face must lie on and which candidate
/// wins — the nearest one in the cast's own direction, exactly as `footprint_scan` does it.
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
    // Candidate selection is per FACE — the client's query is bbox-free (`0x6a8a20`: no bbox, no
    // portals, no camera; the column containment lives inside `floor_z_at`).
    // An AUTHORED group-box pre-cull once lived here, and it broke exactly where a MOGI box
    // understates its geometry: NSabbey group 3's box bottom (z 1.84) floats above its own floor
    // polys (z ≈ 0.3), so a probe at feet+0.1 dipped under the box, culled the whole group's
    // faces, and the standing player's indoor verdict flapped (director-caught, 2026-07-12).
    // The broad phase below is different in kind: `bounds` is the AABB **of the faces themselves**
    // ([`WmoModel::group_collision_bounds`], computed at load), so the cull is exact — `floor_z_at`
    // only hits inside a triangle's XY projection at z ≥ that group's min, and a group whose face
    // bounds exclude the column can contribute nothing. Dropping the pre-cull entirely instead
    // scanned every face of every resident building every frame: the 2026-07-12 fps regression
    // (~8 ms/frame at Northshire).
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
        // The narrow phase: the group's column index when it has one, else its whole face list.
        // The index yields a superset in ascending order (`column_grid`), so the first-wins tie on
        // an exact `z` match below resolves exactly as the linear scan resolved it.
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
    // Beyond the ray → no claim at all.
    let depth = if up { best_z - eye[2] } else { eye[2] - best_z };
    if depth > ZONE_RAY_LEN {
        return None;
    }
    // The terrain race: strictly-nearer terrain wins the column (standing over open ground);
    // a tie keeps the WMO (`0x6a8b15`'s `t_wmo <= t_terr`).
    if terrain_z.is_some_and(|tz| tz <= eye[2] && tz > best_z) {
        return None;
    }
    // The caller's-law classification of the winner (`0x6a87f0` against `outdoor_mask`).
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
/// point already on/near the plane.
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

    /// `down_ray_pick` with a portal graph, no terrain — the pre-terrain-race fixtures.
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
        // A tall outer group (floor z=0) with a small room nested inside it (floor z=10) — the
        // vertical-nest case the bbox heuristic got wrong.
        let outer = nav(0, [-30.0, -30.0, 0.0], [30.0, 30.0, 100.0], 0, 0);
        let room = nav(0, [-5.0, -5.0, 10.0], [5.0, 5.0, 16.0], 0, 0);
        let groups = [outer, room];
        let tris = [quad(0.0, 30.0), quad(10.0, 5.0)];
        // Standing in the room (z=12): the room's floor (z=10) is nearer than the outer floor (z=0).
        assert_eq!(faces_only(&tris, &groups, [0.0, 0.0, 12.0]), Some(1));
        // Below the room floor (z=5): only the outer floor is below ⇒ outer.
        assert_eq!(faces_only(&tris, &groups, [0.0, 0.0, 5.0]), Some(0));
        // Out where only the big outer floor spans ⇒ outer.
        assert_eq!(faces_only(&tris, &groups, [20.0, 20.0, 5.0]), Some(0));
        // No face anywhere near below (past MAX_FLOOR_DROP) ⇒ outside.
        assert_eq!(faces_only(&tris, &groups, [0.0, 0.0, 5000.0]), None);
    }

    #[test]
    fn face_leg_is_orientation_agnostic() {
        // A steep ramp face (nearly a wall — |n.z|/|n| ≈ 0.1, well under the old 0.3 floor filter)
        // still owns the column: the walking-collision leg has no normal filter (`0x6bc700`).
        let g = nav(0, [-10.0, -10.0, 0.0], [10.0, 10.0, 120.0], 0, 0);
        let steep = vec![[[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 20.0]]];
        assert_eq!(faces_only(&[steep], &[g], [0.0, 0.0, 30.0]), Some(0));
    }

    #[test]
    fn face_leg_exterior_face_reads_as_outside() {
        // Standing over an EXTERIOR group's surface ⇒ outside (null), like the client clearing the
        // instance.
        let ext = nav(EXTERIOR, [-30.0, -30.0, 0.0], [30.0, 30.0, 100.0], 0, 0);
        assert_eq!(
            faces_only(&[quad(0.0, 30.0)], &[ext], [0.0, 0.0, 5.0]),
            None
        );
    }

    /// A horizontal portal polygon (quad, radius `r`) at height `z`, plane `z - h = 0`
    /// (normal +Z), starting at `start_vertex` 0.
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

    /// The Goldshire-stairs shape: group 0 = the stairwell (floor z=0), group 1 = the taproom whose
    /// floor-hole portal sits at z=10 over the stairwell column.
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
        // Eye above the hole plane (z=12): the portal crossing (z=10) beats the stair floor (z=0);
        // the eye's side picks the TAPROOM as in-group, and the stairwell rides along as the
        // across-group — BOTH are flood roots (the client's two-member seed set).
        assert_eq!(
            no_terrain(&tris, &groups, &pverts, &infos, &refs, [0.0, 0.0, 12.0]),
            DownRaySeeds {
                in_group: Some(1),
                across: Some(0)
            }
        );
        // Eye below the plane (z=5, on the stairs): the crossing is above the eye ⇒ face leg ⇒
        // stairs alone (no portal crossing ⇒ no across-group).
        assert_eq!(
            no_terrain(&tris, &groups, &pverts, &infos, &refs, [0.0, 0.0, 5.0]),
            DownRaySeeds {
                in_group: Some(0),
                across: None
            }
        );
        // Eye above the plane but outside the hole polygon ⇒ no crossing, no floor ⇒ outside.
        assert_eq!(
            no_terrain(&tris, &groups, &pverts, &infos, &refs, [10.0, 10.0, 12.0]),
            DownRaySeeds::default()
        );
    }

    #[test]
    fn portal_crossing_respects_the_side_orientation() {
        // Same stairwell but the portal plane's normal points DOWN (−Z), so the side flags mirror.
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
        // The slab's collision face and the floor-hole portal share z=10 exactly. The eye above must
        // resolve to the portal's side pick (the taproom), not the slab face's own group — the
        // client's ~1e-4 nearest-accept lets the portal win the tie.
        let (groups, mut tris) = stairwell();
        tris[0].extend(quad(10.0, 5.0)); // a slab face owned by the STAIR group, coincident with p0
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
        // A vertical doorway portal in the x=0 plane between room A (x<0, floor z=0) and room B
        // (x>0, floor z=0), both floors owned by room A's mesh right up to x=+0.5 — the straddle:
        // the face under the column lags the room boundary.
        let room_a = nav(0, [-10.0, -10.0, 0.0], [0.5, 10.0, 10.0], 0, 1);
        let room_b = nav(0, [-0.5, -10.0, 0.0], [10.0, 10.0, 10.0], 1, 1);
        let groups = [room_a, room_b];
        // Room A's floor extends past the doorway to x=0.5; room B's floor starts at 0.5.
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
        // Eye 0.05 past the plane into room B, inside the doorway: within the 0.1-yd snap — the
        // doorway counts as crossed at the eye, beating room A's floor face; the side pick says B,
        // and A rides along as the across-group. The straddle is covered by the seed set.
        assert_eq!(
            no_terrain(&tris, &groups, &verts, &infos, &refs, [0.05, 0.0, 1.7]),
            DownRaySeeds {
                in_group: Some(1),
                across: Some(0)
            }
        );
        // Eye 0.3 past the plane: beyond the snap window — the doorway is NOT crossed (the client's
        // parallel branch returns no hit) and the face under the column decides: still room A's mesh
        // at x=0.3, so the seed is room A. The real client is protected here by the collision mesh
        // tracking the boundary to sub-yard — the mechanism, not a doorway tolerance (`0x6b92b0`).
        assert_eq!(
            no_terrain(&tris, &groups, &verts, &infos, &refs, [0.3, 0.0, 1.7]),
            DownRaySeeds {
                in_group: Some(0),
                across: None
            }
        );
    }

    /// `area_down_ray` over per-group tris on the zone-text law (`0x8` alone), with the bounds
    /// computed from the faces (the load-time path, via the shared helper — fixtures and the
    /// loader can't disagree).
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

    /// **A body planted UNDER the floor it stands on is invisible to the position cast, and the
    /// under-floor fallback is what finds it**. Orgrimmar's bonfires, braziers,
    /// meat racks and shop signs are authored 0.45–1.16 yd below the group floor they sit in — a
    /// ray down from the origin passes beneath that floor and reports open ground, which the
    /// exterior-scene election reads as "outdoors" and hides. `room_at`'s second pass re-casts from
    /// `ROOM_UNDER_FLOOR_TOLERANCE` higher; this pins both halves of why that works: the cast from
    /// the origin really does miss, the lifted one really does find the same floor, and the lift is
    /// far enough to cover the measured spread and no further.
    #[test]
    fn an_under_floor_origin_is_found_only_by_the_lifted_cast() {
        let floor = [quad(0.0, 20.0)];
        let nav = [nav(0, [-20.0, -20.0, 0.0], [20.0, 20.0, 0.0], 0, 0)];
        let lift = super::super::interior::POSITION_PROBE_LIFT;
        let tol = super::super::interior::ROOM_UNDER_FLOOR_TOLERANCE;
        // The worst measured burial (Orgrimmar bonfire 177026, 1.16 yd under group 134's floor).
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
        // …and the lift is a tolerance, not a licence: a floor further above than it stays unclaimed,
        // which is what keeps the storey above out of reach.
        assert_eq!(
            area_ray(&floor, &nav, [0.0, 0.0, -(tol + 1.0) + lift + tol], None),
            None,
            "a floor beyond the tolerance must stay unclaimed"
        );
    }

    /// **The column index never changes a down-ray verdict.** A dungeon-shaped face set (a big
    /// stack of small floor quads at varying heights, spread over several groups) answered with
    /// the per-group column index must equal the answer from the plain linear scan, column for
    /// column — group, depth and outdoor class. The index is a narrow phase, and a narrow phase
    /// that can re-rank moves a unit's room, its light and the zone name.
    #[test]
    fn down_ray_column_index_is_exact() {
        // Three stacked storeys of floor tiles in the same XY column range — the Blackrock shape
        // that defeats the per-group bounds broad phase (every group owns the column).
        let storey = |z: f32| -> Vec<[[f32; 3]; 3]> {
            let mut out = Vec::new();
            for ix in 0..10 {
                for iy in 0..10 {
                    let (x, y) = (ix as f32 * 3.0, iy as f32 * 3.0);
                    // Vary z within the storey so which tile wins is observable.
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
        // The NSabbey group-3 shape (director-caught flap, 2026-07-12): the AUTHORED nav box
        // bottom (z 1.84) floats above the group's own floor polys (z 0.3). A probe cast from the
        // position (feet + 0.1 ≈ 0.4) dips under the authored box — the old authored-box pre-cull
        // dropped the group here. The broad phase must be the FACE-derived bounds, which reach
        // down to the real floor; reintroducing any authored-box cull fails this test.
        let lying_box = nav(0, [-10.0, -10.0, 1.84], [10.0, 10.0, 20.0], 0, 0);
        let tris = [quad(0.3, 10.0)]; // the group's real floor, below its authored box
        assert_eq!(
            area_ray(&tris, &[lying_box], [0.0, 0.0, 0.4], None),
            Some(0)
        );
    }

    #[test]
    fn area_ray_broad_phase_is_exactly_conservative() {
        // Two groups side by side; the broad phase must scan only the owning column — and must
        // never change an answer: inside either column the verdict matches the face scan, outside
        // both it is None (nothing to hit).
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
        // Over a group but BELOW its faces: the column reaches nothing at/below the eye.
        assert_eq!(area_ray(&tris, &navs, [10.0, 0.0, 1.0], None), None);
    }

    #[test]
    fn area_ray_exterior_and_terrain_race_hold() {
        // The EXTERIOR-flag and terrain-race legs are unchanged by the broad phase: an exterior
        // group's surface reads outdoors, and strictly-nearer terrain steals the column.
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
        // The Stormwind-street shape: a `0x40`-only group (EXTERIOR_LIT, no
        // EXTERIOR) is "indoors" on the zone-text law (`[node+0x90]` bit 0 keys on `0x8` alone)
        // but OUTDOORS on the unit-lighting law (the `0x6a87f0` class fork keys on `MOGI & 0x48`).
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
        // The Onyxia lava trap's shape. A GameObject's light node anchors at its AUTHORED-box
        // centre, and that model's box reaches 98 yd past its own geometry, so the anchor sits
        // ~40 yd under the chamber floor it stands on. The downward cast finds nothing — every
        // face is ABOVE the probe — and the reference recovers with the containment lane's
        // `+1000` upward retry (`6a908d`/`6a9093`). Before it, 104 traps in a WMO-only interior
        // classified as outdoors and took the day/night sun, meshes and particles alike.
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
        // The retry is still a RAY, not a licence: a face below the probe never claims it.
        assert_eq!(
            up_ray_claim(&tris, &bounds, &[], &[room], [0.0, 0.0, 40.0], mask),
            None
        );
    }

    #[test]
    fn down_ray_claim_distinguishes_deck_from_terrain() {
        // The Booty Bay boardwalk shape: the un-collapsed claim keeps the
        // "outdoors ON an outdoor-class WMO face" vs "outdoors over terrain" distinction the
        // collapsed `area_down_ray` erases — a deck unit forces the lit 2.5, a field unit takes
        // the MCSH fork. Depth is the cross-placement arbitration key.
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
        // Standing on the deck (terrain far below): an OUTDOOR claim, depth = probe − face.
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
        // Terrain strictly nearer (probe over open ground above a buried deck): no claim at all.
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
        // An interior group's face under the same probe: a non-outdoor claim.
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
        // One indoor group: a mine tunnel with its floor at z=0, ceiling bbox to z=10.
        let mine = nav(0, [-20.0, -20.0, 0.0], [20.0, 20.0, 10.0], 0, 0);
        let tris = [quad(0.0, 20.0)];
        // A camera 2 yd over the tunnel floor. With no terrain leg (or terrain far below), it is inside.
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
        // The hillside surface sits at z=8, between the eye (z=2) and... no: the eye is BELOW the
        // surface, so the surface is above the eye and off the down-segment — a real tunnel under a
        // hill still reads INSIDE.
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
        // Now the camera is in open air 12 yd up, and the ground surface is at z=8 — below the eye and
        // ABOVE the tunnel floor (z=0). The terrain wins the column: outside.
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
        // A terrain surface exactly coincident with the WMO floor keeps the WMO (the client's strict
        // `<`: equal is not "strictly nearer").
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
        // A terrain surface just above the floor (z=0.1 vs floor 0) steals the column.
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
        // The Deadmines-pocket shape: an interior group whose only floor is DETAIL
        // faces — present in the camera-only set, absent from the walking set.
        let pocket = nav(0, [-10.0, -10.0, 0.0], [10.0, 10.0, 12.0], 0, 0);
        let walk: [Vec<[[f32; 3]; 3]>; 1] = [Vec::new()];
        let detail = [quad(0.0, 10.0)];
        // Both faithful legs miss, no terrain below ⇒ Leg C names the room, single root.
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
        // Terrain under a hill (surface above the eye) is off the down-segment: Leg C still fires.
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
        // Any terrain surface at/below the eye (a doorstep decal over open ground) keeps the client's
        // outside verdict — the fallback's gate.
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
        // A walking-leg answer anywhere (even a lower floor of another group) pre-empts Leg C: the
        // faithful verdict is never touched.
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
        // An EXTERIOR group's detail surface reads as outside, mirroring Leg A's rule.
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
