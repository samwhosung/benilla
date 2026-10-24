//! The WMO-interior minimap's group selection (the portal flood-fill) and tile-name stem.

use bevy::ecs::entity::Entity;
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

/// The interior query window (`0x6d95f8`-`0x6d96ab`): the `c`-wide cell holding the player,
/// snapped as `round(p·r − 0.5)·c` with `r = 1/c` (half to even), and `[z − c/2, z]` high, in WoW
/// world axes. x takes `r` as a stored float (`0x6d9629`), y the unrounded register (`0x6d9617`);
/// each product is stored as a float before the 0.5 comes off.
fn interior_window(p: [f32; 3], c: f32) -> ([f32; 3], [f32; 3]) {
    let r = 1.0 / f64::from(c);
    #[allow(clippy::cast_possible_truncation)] // the reference's float stores
    let snap = |v: f32, r: f64| {
        let cell = (f64::from((f64::from(v) * r) as f32) - 0.5).round_ties_even();
        (cell * f64::from(c)) as f32
    };
    #[allow(clippy::cast_possible_truncation)]
    let (x, y) = (snap(p[0], f64::from(r as f32)), snap(p[1], r));
    let z = p[2] - c * 0.5;
    ([x, y, z], [x + c, y + c, c * 0.5 + z])
}

/// The interior window as its last rebuild stored it (`0xcea9c4`-`0xcea9d8`), what the rebuild
/// keyed on, and the groups its flood chose, which draw until the next rebuild.
pub(super) struct InteriorWindow {
    /// The WMO instance the player is in (the pair `0xceaa68`/`0xceaa64`).
    instance: Entity,
    /// The player's group (`0x86f6a0`).
    group: usize,
    /// The zoom radius `c`; a zoom change sets the dirty bit (`SetZoomIndex`, `0x6da915`).
    radius: f32,
    min: [f32; 3],
    max: [f32; 3],
    drawable: Vec<bool>,
}

impl InteriorWindow {
    /// Whether `UpdateMinimap` keeps the window (`0x6d95f0`): no change of instance (`0x6d94ef`)
    /// or group (`0x6d9523`), no dirty bit, and x and y strictly inside the window's x/y bounds
    /// (`0x6d952c`-`0x6d9579`). Nothing compares z, so a climb inside one cell and one group keeps
    /// the window's z range, and the flood's box floor, as stored.
    fn holds(&self, instance: Entity, group: usize, radius: f32, p: [f32; 3]) -> bool {
        self.instance == instance
            && self.group == group
            && self.radius == radius
            && p[0] > self.min[0]
            && p[0] < self.max[0]
            && p[1] > self.min[1]
            && p[1] < self.max[1]
    }
}

/// The groups the interior minimap draws, per absolute group index: the stored window's flood,
/// rebuilt from the player's position (`0x6d95f8`-`0x6d96ab`) only when [`InteriorWindow::holds`]
/// fails.
pub(super) fn interior_drawable<'a>(
    stored: &'a mut Option<InteriorWindow>,
    model: &WmoModel,
    world_from_local: &Affine3A,
    instance: Entity,
    player_pos: Vec3,
    radius: f32,
    seed: usize,
) -> &'a [bool] {
    let p = bevy_to_wow(player_pos);
    if !stored
        .as_ref()
        .is_some_and(|w| w.holds(instance, seed, radius, p))
    {
        let (min, max) = interior_window(p, radius);
        *stored = Some(InteriorWindow {
            instance,
            group: seed,
            radius,
            min,
            max,
            drawable: interior_group_selection(model, world_from_local, (min, max), radius, seed),
        });
    }
    stored.as_ref().map_or(&[], |w| &w.drawable)
}

/// Which groups' tiles draw, per absolute group index: the reference's portal flood-fill from the
/// player's group (`0x6a5020`) inside the query box of the stored `window`.
///
/// The box is [`interior_window`] grown by `c` on every side (`0x6d96b6`-`0x6d96f3`), moved into
/// the model frame as the AABB of its image (`0x6dc540`), its top z then replaced by the seed
/// group's ceiling (`0x6a524b`, the group's `+0x28`). A group whose bbox misses the box in XY
/// neither emits nor floods onward (`0x6a51f4`); there is no per-group Z test. A portal is crossed
/// unless its polygon lies wholly outside one of the box's six planes, the only Z gate.
fn interior_group_selection(
    model: &WmoModel,
    world_from_local: &Affine3A,
    window: ([f32; 3], [f32; 3]),
    radius: f32,
    seed: usize,
) -> Vec<bool> {
    let n = model.group_nav.len();
    let mut drawable = vec![false; n];
    if seed >= n {
        return drawable;
    }
    let c = radius;
    let (wmin, wmax) = (window.0.map(|v| v - c), window.1.map(|v| v + c));

    // A world point (WoW axes) to the model frame, where the reference floods.
    let local_from_world = world_from_local.inverse();
    let to_model = |w: [f32; 3]| bevy_to_wow(local_from_world.transform_point3(wow_to_bevy(w)));
    let (mut box_min, mut box_max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for x in [wmin[0], wmax[0]] {
        for y in [wmin[1], wmax[1]] {
            for z in [wmin[2], wmax[2]] {
                let m = to_model([x, y, z]);
                for k in 0..3 {
                    box_min[k] = box_min[k].min(m[k]);
                    box_max[k] = box_max[k].max(m[k]);
                }
            }
        }
    }
    box_max[2] = model.group_nav[seed].bbox_max[2];

    let xy_overlap = |lo: [f32; 3], hi: [f32; 3]| {
        lo[0] <= box_max[0] && hi[0] >= box_min[0] && lo[1] <= box_max[1] && hi[1] >= box_min[1]
    };
    // 6-plane outcode of a model point against the box, one bit per face it is outside.
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
        // The XY gate blocks emit and recursion both (`0x6a51f4`); a stacked storey shares the XY
        // footprint and passes it.
        if !xy_overlap(gn.bbox_min, gn.bbox_max) {
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
                and &= outcode(*v);
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
    use benilla_assets::WmoModel;
    use bevy::math::{Affine3A, Vec3};

    /// The flood of the window a rebuild at `pos` stores, the model at the origin.
    fn select(model: &WmoModel, pos: Vec3, c: f32, seed: usize) -> Vec<bool> {
        let window = super::interior_window(benilla_assets::coords::bevy_to_wow(pos), c);
        super::interior_group_selection(model, &Affine3A::IDENTITY, window, c, seed)
    }

    /// `UpdateMinimap` rebuilds the window only on a change of instance or group, the dirty bit
    /// or x/y leaving the window (`0x6d952c`-`0x6d9579`); z is never compared, so climbing in one
    /// cell and one group floods from the stored box floor `pz − 1.5c`, not the player's.
    #[test]
    fn a_climb_in_one_cell_keeps_the_stored_window() {
        use super::interior_drawable;
        use benilla_assets::coords::wow_to_bevy;
        use bevy::ecs::entity::Entity;
        // A 60-yd tall seed and a neighbour through a portal at z −40, below the box floor
        // −37.5 of a rebuild at z 0 with `c` 25, above the floor −47.5 of one at z −10.
        let model = two_groups(60.0, [-5.0, 5.0], 0.0, -40.0);
        let instance = Entity::from_raw_u32(1).unwrap();
        let at = |stored: &mut Option<super::InteriorWindow>, p: [f32; 3], c: f32| {
            interior_drawable(
                stored,
                &model,
                &Affine3A::IDENTITY,
                instance,
                wow_to_bevy(p),
                c,
                0,
            )[1]
        };
        let mut stored = None;
        assert!(!at(&mut stored, [1.0, 1.0, 0.0], 25.0), "rebuilt at z 0");
        assert!(
            !at(&mut stored, [1.0, 1.0, -10.0], 25.0),
            "10 yd lower in the same cell: the window's z stays"
        );
        assert!(
            at(&mut stored, [1.0, 1.0, -10.0], 25.5),
            "a zoom change rebuilds at the current height"
        );
        let mut stored = None;
        at(&mut stored, [1.0, 1.0, 0.0], 25.0);
        assert!(
            at(&mut stored, [-1.0, 1.0, -10.0], 25.0),
            "x leaving the cell rebuilds at the current height"
        );
    }
    /// A 4-group building: g0 (the player's) and g1 (a cellar) join by a floor hole and both draw;
    /// g2 sits 1000 yd off in XY, reached but not drawn; g3 overlaps the box but is reachable only
    /// through g2, by a portal inside the box, so only g2's XY gate keeps it out (`0x6a51f4`).
    #[test]
    fn interior_flood_fill_gates_on_xy_and_blocks_recursion_through_a_missed_group() {
        use benilla_assets::{WmoGroupNav, WmoModel, WmoPortalInfo, WmoPortalRef};

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
            convex_volume: Vec::new(),
        };

        let drawable = select(&model, Vec3::ZERO, 25.0, 0);
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
        use benilla_assets::{WmoGroupNav, WmoModel, WmoPortalInfo, WmoPortalRef};

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
            convex_volume: Vec::new(),
        };

        let drawable = select(&model, Vec3::ZERO, 25.0, 0);
        assert!(drawable[0]);
        assert!(
            !drawable[1],
            "a portal below the query box's Z floor is not crossed, so the deep storey never draws"
        );
    }

    /// A seed group `g0` at the origin, 10 yd square, `[0, seed_top]` high, and a neighbour `g1`
    /// spanning `x` through a portal: a 1-yd square at (`portal_x`, 0, `portal_z`).
    fn two_groups(
        seed_top: f32,
        g1_x: [f32; 2],
        portal_x: f32,
        portal_z: f32,
    ) -> benilla_assets::WmoModel {
        use benilla_assets::{WmoGroupNav, WmoModel, WmoPortalInfo, WmoPortalRef};
        let nav = |bmin: [f32; 3], bmax: [f32; 3], ref_start: u16| WmoGroupNav {
            flags: 0,
            bbox_min: bmin,
            bbox_max: bmax,
            ref_start,
            ref_count: 1,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: benilla_formats::NO_GROUP_LIQUID,
        };
        let (x, z) = (portal_x, portal_z);
        WmoModel {
            portal_vertices: vec![
                [x, -0.5, z - 0.5],
                [x, 0.5, z - 0.5],
                [x, 0.5, z + 0.5],
                [x, -0.5, z + 0.5],
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
            group_nav: vec![
                nav([-5.0, -5.0, 0.0], [5.0, 5.0, seed_top], 0),
                nav([g1_x[0], -5.0, 0.0], [g1_x[1], 5.0, 3.0], 1),
            ],
            ..Default::default()
        }
    }

    /// The query window is the snapped cell `round(p·r − 0.5)·c` (`0x6d95f8`-`0x6d96ab`) grown
    /// by `c` on every side (`0x6d96b6`-`0x6d96f3`): at the origin with `c` 25, x spans
    /// `[−25, 50]`, so a group 30 yd west misses it and one 40 yd east is inside.
    #[test]
    fn the_query_window_is_the_snapped_cell_grown_by_its_width() {
        let at = |g1_x: [f32; 2], portal_x: f32| {
            let model = two_groups(3.0, g1_x, portal_x, 1.0);
            select(&model, Vec3::ZERO, 25.0, 0)[1]
        };
        assert!(
            !at([-45.0, -30.0], -4.0),
            "west of the window's x minimum −25"
        );
        assert!(at([-45.0, -24.0], -4.0), "reaching into it");
        assert!(
            at([45.0, 52.0], 4.0),
            "east, inside the window's x maximum 50"
        );
        assert!(!at([51.0, 60.0], 4.0), "past it");
        // At x = 30 the cell is [25, 50], so the window is [0, 75].
        let model = two_groups(3.0, [-12.0, -1.0], -4.0, 1.0);
        let west = select(
            &model,
            benilla_assets::coords::wow_to_bevy([30.0, 0.0, 0.0]),
            25.0,
            0,
        );
        assert!(
            !west[1],
            "the snap moved the window off it, though it lies within 2c"
        );
    }

    /// The flood's box top is the seed group's ceiling (`0x6a524b`, its `+0x28`), not the
    /// player's height plus `c`: in a hall taller than `c` a portal above `z + c` is crossed, and
    /// under a low ceiling a portal above it is not.
    #[test]
    fn the_flood_box_tops_out_at_the_seed_groups_ceiling() {
        let crossed = |seed_top: f32, portal_z: f32| {
            let model = two_groups(seed_top, [-5.0, 5.0], 0.0, portal_z);
            select(&model, Vec3::ZERO, 25.0, 0)[1]
        };
        assert!(
            crossed(40.0, 35.0),
            "a 40-yd hall: the portal at 35 is under its ceiling"
        );
        assert!(
            !crossed(3.0, 11.0),
            "a 3-yd room: the portal at 11 is above its ceiling"
        );
        assert!(crossed(3.0, 2.0), "and one under it is crossed");
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
