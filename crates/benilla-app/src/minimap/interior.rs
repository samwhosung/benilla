//! The WMO-interior minimap's group selection — the portal flood-fill + the tile-name stem
//! (split from the tile renderer; the mechanism and its byte provenance are unchanged).

use bevy::math::{Affine3A, Vec3};

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_assets::WmoModel;

/// MOGP group flags read by the interior flood-fill: `EXTERIOR` (an outdoor shell group — not part of
/// the interior tile flood) and `UNREACHABLE`/no-render (`0x80`) which also suppresses tile emit. The
/// client skips `& 0x8` at the flood level (`0x6a5020`) and `& 0x88` at the emit level
/// (`0x6a5270`).
const GROUP_EXTERIOR: u32 = 0x8;
const GROUP_NO_EMIT: u32 = 0x88;

/// The `md5translate.trs` key stem for a WMO's interior minimap tiles, from its `.wmo` asset path.
/// The reference builds the tile name by stripping the `World\` prefix and the `.wmo` extension off
/// the model filename (name builder `0x6da330`), e.g.
/// `World\wmo\KhazModan\Cities\Ironforge\Ironforge.wmo` → `wmo\khazmodan\cities\ironforge\ironforge`.
/// Backslash-separated + lowercased (the trs lookup is case-insensitive). `None` if not a
/// `World\…\*.wmo`.
pub(super) fn wmo_minimap_stem(wmo_path: &str) -> Option<String> {
    let p = wmo_path.replace('/', "\\").to_ascii_lowercase();
    let stem = p.split_once("world\\")?.1.strip_suffix(".wmo")?;
    Some(stem.to_string())
}

/// The interior minimap's group selection: a **portal flood-fill** from the player's current group,
/// gated by a player-centred query box — the client mechanism (the flood `0x6a5020`). Returns, per
/// absolute group index, whether that group's tiles should be drawn (reached through portals AND
/// its bbox overlaps the view in XY). Replaces the naive draw-every-group-in-the-window, which
/// painted unreachable/far floors over the current one.
///
/// The query box (WoW **world** axes; the client builds it in the WMO model frame, but the overlap
/// decisions are frame-agnostic so we test in world space — the group bboxes and portal polygons are
/// transformed by the placement to match): XY = the view window grown to `±2·radius`; **Z is
/// asymmetric, `[player.z − 1.5·radius, player.z + radius]`** — mostly *below* the eye, so from a floor
/// you reach the storey just under it through a stairwell but not the whole tower. A group EMITS when
/// its bbox overlaps the box in **XY only** (no per-group Z test — the client has none). The **same
/// XY overlap gates the portal RECURSION** (a miss returns at `0x6a51f4`): a group that fails the
/// XY test neither emits nor floods onward. For an XY-passing group, each portal is CROSSED when
/// its polygon is not fully outside any of the box's **6 planes** (a 3-D outcode cull *including Z*
/// — the only per-floor Z gate the client applies; stacked floors share an XY footprint, so an
/// adjacent storey passes its own XY gate and is reached through the stairwell portal). `EXTERIOR`
/// (`0x8`) groups are not interior-flooded; `0x88` groups emit no tiles.
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

    // A model-space point → world (WoW axes), the frame the query box lives in.
    let to_world = |m: [f32; 3]| bevy_to_wow(world_from_local.transform_point3(wow_to_bevy(m)));
    // A group's model bbox → its world AABB (8 transformed corners) — a conservative superset under a
    // rotated placement, fine for the emit gate (an over-included far group is window-culled at draw).
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
    // 6-plane outcode of a world point against the box (1 bit per face it is outside of).
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
            continue; // an outdoor shell group — not part of the interior flood
        }
        let (lo, hi) = world_aabb(gn.bbox_min, gn.bbox_max);
        // XY-OVERLAP GATE (a miss returns at `0x6a51f4`): a group whose model-XY bbox misses the
        // query window emits NOTHING and does NOT flood onward — the gate blocks emit AND recursion
        // both. Stacked storeys share an XY footprint, so an adjacent floor still passes it (and is
        // reached via the stairwell portal's 3-D cull below).
        if !xy_overlap(lo, hi) {
            continue;
        }
        if gn.flags & GROUP_NO_EMIT == 0 {
            drawable[g] = true;
        }
        // PORTAL RECURSION: cross a portal iff its polygon is not fully outside any box plane (3-D,
        // Z included). This is the only place a floor above/below is gated out.
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
                    break; // not fully outside any one plane ⇒ the portal reaches into the box
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
    /// The interior flood-fill's two gates, on a synthetic 4-group building (all sharing one XY
    /// footprint except `far`, which sits 1000 yd away):
    ///   `player(g0)` ──floor-hole portal──> `cellar(g1)`  both under the query box ⇒ both drawn
    ///   `player(g0)` ──in-box portal─────-> `far(g2)`      reached, bbox misses XY ⇒ NOT drawn
    ///   `far(g2)`    ──portal────────────-> `behind(g3)`   must NOT be reached: the XY gate that
    ///                                                      rejected g2 also blocks flooding THROUGH it
    /// The last leg is the reference's recursion gate (the XY gate blocks emit *and* recursion,
    /// `0x6a51f4`). `behind`'s own bbox does overlap the box, so it would draw if recursion had
    /// leaked through `far`.
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
                                                // portal 2: g2<->g3, deliberately placed INSIDE the query box, so the 3-D outcode cull would
                                                // happily cross it. The ONLY thing that stops g3 being reached is g2 failing its own XY gate
                                                // — which is exactly the behaviour under test (a
                                                // box-outside portal would pass this test even with
                                                // the pre-correction emit-only gate, and so would
                                                // prove nothing).
        portal_vertices.extend(poly(3.0, 1.0));
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

    /// The box's Z extent is what keeps a distant storey out: put the connecting portal far below the
    /// box floor and the 3-D outcode cull stops the flood before it.
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
        // The interior tile key stem = the .wmo path minus `World\` and `.wmo`, lowercased. Appending
        // `_001_00_00.blp` reproduces the trs key `WMO\KhazModan\Cities\Ironforge\ironforge_001_00_00.blp`.
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
