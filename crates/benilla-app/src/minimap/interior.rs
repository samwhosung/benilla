//! The WMO-interior minimap's group selection (the portal flood-fill) and tile-name stem.

use bevy::math::{Affine3A, Vec3};

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_assets::WmoModel;

/// MOGP flags: the flood skips `& 0x8` exterior groups (`0x6a5020`), the emit skips `& 0x88`
/// (`0x6a5270`).
const GROUP_EXTERIOR: u32 = 0x8;
const GROUP_NO_EMIT: u32 = 0x88;

/// The `md5translate.trs` key stem of a WMO's interior tiles: the model path without `World\` and
/// `.wmo` (name builder `0x6da330`), backslashed and lowercased, the trs lookup being
/// case-insensitive. `None` if not a `World\…\*.wmo`.
pub(super) fn wmo_minimap_stem(wmo_path: &str) -> Option<String> {
    let p = wmo_path.replace('/', "\\").to_ascii_lowercase();
    let stem = p.split_once("world\\")?.1.strip_suffix(".wmo")?;
    Some(stem.to_string())
}

/// Which groups' tiles draw, per absolute group index: the reference's portal flood-fill from the
/// player's group (`0x6a5020`) inside a player-centred query box.
///
/// The box is XY `±2·radius` and Z `[z − 1.5·radius, z + radius]`, mostly below the player, so a
/// stairwell reaches the storey below but not the whole tower. A group whose bbox misses the box in
/// XY neither emits nor floods onward (`0x6a51f4`); there is no per-group Z test. A portal is
/// crossed unless its polygon lies wholly outside one of the box's six planes, the only Z gate.
/// The reference builds the box in model space; testing in world space with the bboxes and portals
/// transformed gives the same answers.
pub(super) fn interior_group_selection(
    model: &WmoModel,
    world_from_local: &Affine3A,
    player_pos: Vec3,
    radius: f32,
    seed: usize,
) -> Vec<bool> {
    let n = model.group_nav.len();
    let mut drawable = vec![false; n];
    if seed >= n {
        return drawable;
    }
    let pw = bevy_to_wow(player_pos);
    let c = radius;
    let box_min = [pw[0] - 2.0 * c, pw[1] - 2.0 * c, pw[2] - 1.5 * c];
    let box_max = [pw[0] + 2.0 * c, pw[1] + 2.0 * c, pw[2] + c];

    // A model point to world (WoW axes), the query box's frame.
    let to_world = |m: [f32; 3]| bevy_to_wow(world_from_local.transform_point3(wow_to_bevy(m)));
    // A group's model bbox as a world AABB: a superset under a rotated placement, and the draw's
    // window cull trims the excess.
    let world_aabb = |bmin: [f32; 3], bmax: [f32; 3]| {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for &x in &[bmin[0], bmax[0]] {
            for &y in &[bmin[1], bmax[1]] {
                for &z in &[bmin[2], bmax[2]] {
                    let w = to_world([x, y, z]);
                    for k in 0..3 {
                        lo[k] = lo[k].min(w[k]);
                        hi[k] = hi[k].max(w[k]);
                    }
                }
            }
        }
        (lo, hi)
    };
    let xy_overlap = |lo: [f32; 3], hi: [f32; 3]| {
        lo[0] <= box_max[0] && hi[0] >= box_min[0] && lo[1] <= box_max[1] && hi[1] >= box_min[1]
    };
    // 6-plane outcode of a world point against the box, one bit per face it is outside.
    let outcode = |w: [f32; 3]| {
        let mut o = 0u8;
        for k in 0..3 {
            if w[k] < box_min[k] {
                o |= 1 << (2 * k);
            }
            if w[k] > box_max[k] {
                o |= 1 << (2 * k + 1);
            }
        }
        o
    };

    let mut visited = vec![false; n];
    let mut stack = vec![seed];
    while let Some(g) = stack.pop() {
        if g >= n || visited[g] {
            continue;
        }
        visited[g] = true;
        let gn = &model.group_nav[g];
        if gn.flags & GROUP_EXTERIOR != 0 {
            continue; // an exterior group is not flooded
        }
        let (lo, hi) = world_aabb(gn.bbox_min, gn.bbox_max);
        // The XY gate blocks emit and recursion both (`0x6a51f4`); a stacked storey shares the XY
        // footprint and passes it.
        if !xy_overlap(lo, hi) {
            continue;
        }
        if gn.flags & GROUP_NO_EMIT == 0 {
            drawable[g] = true;
        }
        // Cross a portal unless its polygon is wholly outside one box plane, Z included.
        let start = gn.ref_start as usize;
        let end = (start + gn.ref_count as usize).min(model.portal_refs.len());
        for r in &model.portal_refs[start..end] {
            let nb = r.group as usize;
            if r.group == u16::MAX || nb >= n || visited[nb] {
                continue;
            }
            let Some(info) = model.portal_infos.get(r.portal as usize) else {
                continue;
            };
            let vs = info.start_vertex as usize;
            let Some(poly) = model.portal_vertices.get(vs..vs + info.count as usize) else {
                continue;
            };
            let mut and = 0xFFu8;
            for v in poly {
                and &= outcode(to_world(*v));
                if and == 0 {
                    break; // outside no single plane: the portal reaches into the box
                }
            }
            if and == 0 {
                stack.push(nb);
            }
        }
    }
    drawable
}

#[cfg(test)]
mod tests {
    /// A 4-group building: g0 (the player's) and g1 (a cellar) join by a floor hole and both draw;
    /// g2 sits 1000 yd off in XY, reached but not drawn; g3 overlaps the box but is reachable only
    /// through g2, by a portal inside the box, so only g2's XY gate keeps it out (`0x6a51f4`).
    #[test]
    fn interior_flood_fill_gates_on_xy_and_blocks_recursion_through_a_missed_group() {
        use super::interior_group_selection;
        use benilla_assets::{WmoGroupNav, WmoModel, WmoPortalInfo, WmoPortalRef};
        use bevy::math::{Affine3A, Vec3};

        let nav = |zmin: f32, zmax: f32, x: f32, ref_start: u16, ref_count: u16| WmoGroupNav {
            flags: 0,
            bbox_min: [x - 5.0, -5.0, zmin],
            bbox_max: [x + 5.0, 5.0, zmax],
            ref_start,
            ref_count,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: benilla_formats::NO_GROUP_LIQUID,
        };
        // A 1-yd square portal polygon centred at (x, 0, z), lying in the plane x = const.
        let poly = |x: f32, z: f32| {
            [
                [x, -0.5, z - 0.5],
                [x, 0.5, z - 0.5],
                [x, 0.5, z + 0.5],
                [x, -0.5, z + 0.5],
            ]
        };
        let mut portal_vertices = Vec::new();
        portal_vertices.extend(poly(0.0, 0.0)); // portal 0: g0<->g1, a floor hole at the player
        portal_vertices.extend(poly(4.0, 1.0)); // portal 1: g0<->g2, well inside the query box
        portal_vertices.extend(poly(3.0, 1.0)); // portal 2: g2<->g3, also inside the box
        let info = |start: u16| WmoPortalInfo {
            start_vertex: start,
            count: 4,
            plane: [1.0, 0.0, 0.0, 0.0],
        };
        let pref = |portal: u16, group: u16, side: i16| WmoPortalRef {
            portal,
            group,
            side,
        };
        let model = WmoModel {
            wmo_id: 1,
            submeshes: Vec::new(),
            submesh_group: Vec::new(),
            portal_vertices,
            portal_infos: vec![info(0), info(4), info(8)],
            // g0 refs portals 0,1 · g1 refs portal 0 · g2 refs portals 1,2 · g3 refs portal 2
            portal_refs: vec![
                pref(0, 1, 1),
                pref(1, 2, 1),
                pref(0, 0, -1),
                pref(1, 0, -1),
                pref(2, 3, 1),
                pref(2, 2, -1),
            ],
            group_nav: vec![
                nav(0.0, 3.0, 0.0, 0, 2),    // g0 the player's floor
                nav(-10.0, -7.0, 0.0, 2, 1), // g1 cellar, stacked under g0
                nav(0.0, 3.0, 1000.0, 3, 2), // g2 far away in XY
                nav(0.0, 3.0, 0.0, 5, 1),    // g3 overlaps XY, reachable only through g2
            ],
            fogs: Vec::new(),
            skybox: None,
            group_collision_tris: Vec::new(),
            group_camera_only_tris: Vec::new(),
            group_collision_bounds: Vec::new(),
            group_collision_grids: Vec::new(),
            collision_bounds: None,
            collision: None,
            collision_camera: None,
            doodads: Vec::new(),
            doodad_sets: Vec::new(),
            lights: Vec::new(),
            group_bounds: Vec::new(),
            group_footprints: Vec::new(),
            material_ground_type: Vec::new(),
            material_diff_color: Vec::new(),
            group_footprint_bounds: Vec::new(),
            group_footprint_grids: Vec::new(),
            group_light_refs: Vec::new(),
            group_liquids: Vec::new(),
            doodad_base: Default::default(),
            doodad_owner: Default::default(),
            doodad_groups: Default::default(),
        };

        let drawable = interior_group_selection(&model, &Affine3A::IDENTITY, Vec3::ZERO, 25.0, 0);
        assert!(drawable[0], "the player's own group draws");
        assert!(
            drawable[1],
            "the cellar is stacked in XY and its floor-hole portal sits inside the box's Z extent"
        );
        assert!(
            !drawable[2],
            "a reached group whose bbox misses the query window in XY must not emit tiles"
        );
        assert!(
            !drawable[3],
            "the XY gate that rejected g2 must also block flooding THROUGH it (`0x6a51f4`)"
        );
    }

    /// The box's Z extent keeps a distant storey out: a portal far below the box floor is not
    /// crossed.
    #[test]
    fn interior_flood_fill_rejects_a_portal_outside_the_query_box_z_extent() {
        use super::interior_group_selection;
        use benilla_assets::{WmoGroupNav, WmoModel, WmoPortalInfo, WmoPortalRef};
        use bevy::math::{Affine3A, Vec3};

        let nav = |zmin: f32, zmax: f32, ref_start: u16, ref_count: u16| WmoGroupNav {
            flags: 0,
            bbox_min: [-5.0, -5.0, zmin],
            bbox_max: [5.0, 5.0, zmax],
            ref_start,
            ref_count,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: benilla_formats::NO_GROUP_LIQUID,
        };
        // The connecting portal sits at z ≈ -300, far below the box floor (player.z - 1.5·25).
        let model = WmoModel {
            wmo_id: 1,
            submeshes: Vec::new(),
            submesh_group: Vec::new(),
            portal_vertices: vec![
                [0.0, -0.5, -300.5],
                [0.0, 0.5, -300.5],
                [0.0, 0.5, -299.5],
                [0.0, -0.5, -299.5],
            ],
            portal_infos: vec![WmoPortalInfo {
                start_vertex: 0,
                count: 4,
                plane: [1.0, 0.0, 0.0, 0.0],
            }],
            portal_refs: vec![
                WmoPortalRef {
                    portal: 0,
                    group: 1,
                    side: 1,
                },
                WmoPortalRef {
                    portal: 0,
                    group: 0,
                    side: -1,
                },
            ],
            group_nav: vec![nav(0.0, 3.0, 0, 1), nav(-303.0, -300.0, 1, 1)],
            fogs: Vec::new(),
            skybox: None,
            group_collision_tris: Vec::new(),
            group_camera_only_tris: Vec::new(),
            group_collision_bounds: Vec::new(),
            group_collision_grids: Vec::new(),
            collision_bounds: None,
            collision: None,
            collision_camera: None,
            doodads: Vec::new(),
            doodad_sets: Vec::new(),
            lights: Vec::new(),
            group_bounds: Vec::new(),
            group_footprints: Vec::new(),
            material_ground_type: Vec::new(),
            material_diff_color: Vec::new(),
            group_footprint_bounds: Vec::new(),
            group_footprint_grids: Vec::new(),
            group_light_refs: Vec::new(),
            group_liquids: Vec::new(),
            doodad_base: Default::default(),
            doodad_owner: Default::default(),
            doodad_groups: Default::default(),
        };

        let drawable = interior_group_selection(&model, &Affine3A::IDENTITY, Vec3::ZERO, 25.0, 0);
        assert!(drawable[0]);
        assert!(
            !drawable[1],
            "a portal below the query box's Z floor is not crossed, so the deep storey never draws"
        );
    }

    #[test]
    fn wmo_stem_strips_world_prefix_and_extension() {
        use super::wmo_minimap_stem;
        // The stem plus `_001_00_00.blp` is the trs key
        // `WMO\KhazModan\Cities\Ironforge\ironforge_001_00_00.blp`.
        assert_eq!(
            wmo_minimap_stem("World/wmo/KhazModan/Cities/Ironforge/Ironforge.wmo").as_deref(),
            Some("wmo\\khazmodan\\cities\\ironforge\\ironforge"),
        );
        assert_eq!(
            wmo_minimap_stem("World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind.wmo").as_deref(),
            Some("wmo\\azeroth\\buildings\\stormwind\\stormwind"),
        );
        assert_eq!(wmo_minimap_stem("not/a/model.txt"), None);
    }
}
