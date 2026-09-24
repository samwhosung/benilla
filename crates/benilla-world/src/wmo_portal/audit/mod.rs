//! The WMO portal-cull **audit harness** (test-only; data-gated, not ignored — see below): load a real
//! building *at its real placement*, with the real ADT terrain under it, and sweep the two invariants
//! the client upholds by construction. Every violation prints as a deterministic repro (exact
//! model-space coordinates + a per-portal hop trace), so a director-found "it vanishes here" becomes a
//! fixture instead of a guess.
//!
//! - [`wmo_pvs_audit`] — the **inside** invariant: sweep reachable player standing points × orbiting
//!   third-person camera positions (line-of-sight enforced against the camera-collision mesh, like the
//!   real camera's pull-in) and assert **a camera that can see the player draws the player's group**.
//!   Subject: the Goldshire inn (`WOW_AUDIT_WMO=<internal-path>` overrides it, without a placement).
//! - [`wmo_outside_audit`] — the **outside** invariant: a camera in the open air above the terrain is
//!   OUTSIDE, and the building's exterior shell draws. Subject: Fargodeep Mine, whose 21 tunnel groups
//!   sprawl 150 yd under an Elwynn hillside while its one exterior group covers only the entrance
//!   mound. Before the terrain race (decision 0258) a camera on the grass above the tunnels seeded
//!   *inside* one of them and the flood culled the mine out from under the director's cursor. The same
//!   test asserts the race does not over-fire and seal the mine: a standing point on a tunnel floor,
//!   under the hill, still reads INSIDE.
//!
//! **In the gate since 2331.** The seven audits that assert an invariant (this file's four, and
//! `pin`'s three site regressions) are ordinary data-gated tests: `wow_data_or_skip!` skips them
//! where there is no install and `BENILLA_REQUIRE_DATA` refuses that skip where the gate found
//! one (2329). They were `#[ignore]`d for "needs the game data" from before 1175 put an install
//! beside every slot, and nothing ever ran them — the `--ignored` line below was the only runner,
//! and it named a crate this module had left. Cost, measured 2026-09-22: six under a second each,
//! `wmo_pvs_audit` ~14 s (the standing-point × camera sweep). What stays `#[ignore]`d is the
//! instruments — `pin`'s census and probe (aimed by `WOW_PIN_*`) and `light_probe`'s four — run by
//! hand, `--nocapture`, from the command each attribute carries.
//!
//! Run the whole harness, instruments included:
//! `cargo test -p benilla-world --lib wmo_portal::audit -- --include-ignored --nocapture`
//!
//! [`light_probe`] (sibling module) reuses this harness's placed subjects for the entity-LIGHT
//! down-ray probes — per-point verdict/lane maps of the inn corridor and the forge floor.
//!
//! The inside oracle, refined (round 3): the invariant is **not** upheld by the real client at every
//! camera position — a camera over a floorless pocket (under an open staircase, beside the basement
//! ramp) reads "outside" in the real mechanism too, and the interior culls there as a *client*
//! artifact. So seed-`None` violations are reported as **faithful-cull residue**, not failures; any
//! seed-`Some` violation — the camera standing in one room while the flood can't reach the player's —
//! is a hard failure.

use benilla_assets::coords::{bevy_to_wow, placement_rotation, wow_to_bevy};
use benilla_assets::WmoGroupNav;
use benilla_formats::{
    accumulate_wmo_group_camera_collision, accumulate_wmo_group_collision, load_tile_mesh,
    open_chain, parse_wmo_root, terrain_height_at, wmo_group_footprint_tris, wmo_group_header,
    wmo_root_id, ChunkMesh,
};
use bevy::math::{Affine3A, Mat4, Vec3};

use super::{compute_pvs, down_ray_seeds, floor_z_at, terrain_z_local, WmoModel, EXTERIOR};

mod light_probe;
mod pin;

/// A building at its real spot on a real map: the model, plus the MODF placement to find (by its
/// `uniqueId`) in `tile`, whose 3×3 tile block supplies the terrain the down-ray races.
#[derive(Clone, Copy)]
struct Site {
    wmo: &'static str,
    map: &'static str,
    tile: (u32, u32),
    uid: u32,
}

/// The Goldshire inn — a building standing ON the terrain (0233's repro site).
const GOLDSHIRE: Site = Site {
    wmo: r"World\wmo\Azeroth\Buildings\GoldshireInn\GoldshireInn.wmo",
    map: "Azeroth",
    tile: (31, 49),
    uid: 71414,
};

/// Fargodeep Mine — an interior buried UNDER the terrain (decision 0258's repro site: the director's
/// "wrongly not visible from the outside at a specific angle", `md_goldmine_varianta`, id 210351).
const FARGODEEP: Site = Site {
    wmo: r"world\wmo\dungeon\md_goldmine\md_goldmine_varianta.wmo",
    map: "Azeroth",
    tile: (31, 50),
    uid: 210351,
};

/// The Deadmines dungeon shell — decision 0692's repro site: the zone-in tunnel (g35) opens through
/// the swirl-portal doorway into a sealed pocket (g39) floored entirely with DETAIL faces, where the
/// faithful down-ray legs find nothing and the client blanks the building around its own camera.
const DEADMINES: Site = Site {
    wmo: r"world\wmo\dungeon\az_deadmines\az_deadmines_b.wmo",
    map: "DeadminesInstance",
    tile: (32, 32),
    uid: 170633,
};

/// The Goldshire blacksmith — the fire-lit forge floor (the 2026-07-13 per-step light-flash report).
const BLACKSMITH: Site = Site {
    wmo: r"world\wmo\azeroth\buildings\goldshireblacksmith\goldshireblacksmith.wmo",
    map: "Azeroth",
    tile: (31, 49),
    uid: 96048,
};

/// Undercity — B26's site: 200+ groups under Tirisfal, whose one reported doorway culls the room on
/// the far side from *both* sides of the arch (`.go xyz 1558.66 415.39 -62.16 0`).
const UNDERCITY: Site = Site {
    wmo: r"world\wmo\lorderon\undercity\undercity.wmo",
    map: "Azeroth",
    tile: (31, 28),
    uid: 239598,
};

/// Ironforge — B65's site: the lava chasm under the Great Forge walkways. The walkable metal
/// grates at lava level (world z≈421, e.g. `.go xyz -4845 -995 421 0`) live in cavern group g89
/// (one portal, p114→g91); a camera over a grate/bed column seeds that group and the scene draws,
/// while a column with nothing walkable below (the shaft-mouth sliver, the moat's bed gaps) reads
/// "outside" and blanks the interior — which decision 1096 pins as the CLIENT'S OWN behaviour
/// (the down-ray's liquid raycast is a dead edge here, `0x6be345`, and a miss retries nothing,
/// `0x682207`; the asset's patchy beds are the only thing keeping retail drawn over lava). The
/// 2026-08-07 pins: `WOW_PIN_EYE=-4990,-960,470`
/// reproduces a faithful blank column; `WOW_PIN_EYE=-4830,-1090,480` seeds g91 and draws; the
/// still-open half of B65 (partial grate loss from seeded rim poses) is 1096's "NOT settled" leaf.
#[allow(dead_code)] // pinned for `WOW_PIN_*` runs: the B65 repro coordinates live here
const IRONFORGE: Site = Site {
    wmo: r"world\wmo\khazmodan\cities\ironforge\ironforge.wmo",
    map: "Azeroth",
    tile: (34, 41),
    uid: 7706,
};

/// Shadowfang Keep — B335's site: the courtyard whose far doorways read as a flat blue-cyan wash.
/// Its two courtyard groups (g38, g72) are EXTERIOR_LIT (`0x40`) while the rooms behind their
/// arches (g61, g60) are true interiors, which is the shape the interior-fog chain gate turns on.
/// `.go xyz -214.30 2211.84 79.78 33` looks at the arches; `-213.90 2236.15 79.78` is inside g61.
const SHADOWFANG: Site = Site {
    wmo: r"world\wmo\dungeon\ld_shadowfang\ld_shadowfanginterior.wmo",
    map: "Shadowfang",
    tile: (27, 32),
    uid: 218202,
};

/// Darnassus — the 2026-09-02 report's site: one placement, 104 groups, **50 of them exterior**, and
/// the city's outdoor areas stitched to each other and to every shop by portals. That shape is what
/// makes it the discriminating subject for Pass 2: standing in a shop, the doorway admits the one
/// outdoor group beyond it, and everything else the director can see through that doorway is reached
/// only by WALKING ON from there (decision 1853). `.go xyz 10188.36 2348.35 1328.96 1` is the pin.
const DARNASSUS: Site = Site {
    wmo: r"world\wmo\kalimdor\darnassis\darnassis.wmo",
    map: "Kalimdor",
    tile: (27, 12),
    uid: 352798,
};

/// Eye height above a floor point for both the standing player and the seated camera samples.
const EYE_HEIGHT: f32 = 1.7;

/// The subject model + the camera-collision mesh (for the LOS pull-in) + the flattened
/// walking-collision mesh (for the reachability flood's headroom probes) + the real placement.
struct Subject {
    model: WmoModel,
    cam_pos: Vec<[f32; 3]>,
    cam_idx: Vec<u32>,
    walk_pos: Vec<[f32; 3]>,
    walk_idx: Vec<u32>,
    names: Vec<String>,
    /// `None` for a `WOW_AUDIT_WMO` override — then the down-ray runs with no terrain leg.
    placed: Option<Placed>,
}

/// A subject's placement in the world, and the ADT terrain around it — everything the terrain leg of
/// the down-ray needs, in exactly the frames the runtime uses.
struct Placed {
    world_from_local: Affine3A,
    local_from_world: Affine3A,
    chunks: Vec<ChunkMesh>,
}

impl Subject {
    /// The terrain surface under a model-space eye, in model-space `z` — what `compute_wmo_pvs` hands
    /// [`down_ray_seeds`] each frame. `None` with no placement, off-tile, or over an MCNK hole.
    fn terrain_z(&self, eye_local: [f32; 3]) -> Option<f32> {
        let p = self.placed.as_ref()?;
        let eye_world = p.world_from_local.transform_point3(wow_to_bevy(eye_local));
        let wow = bevy_to_wow(eye_world);
        let tz = terrain_height_at(&p.chunks, wow)?;
        Some(terrain_z_local(&p.local_from_world, eye_world, tz))
    }

    /// [`down_ray_seeds`] with this subject's terrain leg supplied.
    fn seeds(&self, eye_local: [f32; 3]) -> super::DownRaySeeds {
        down_ray_seeds(&self.model, eye_local, self.terrain_z(eye_local))
    }
}

/// Load `internal` from the local game data, mirroring the asset loader's nav/collision construction
/// (`benilla-assets::wmo`): MOGI bounds, MOGP flags/portal-ref spans, and the per-group
/// walking-collision face set (every non-DETAIL face — the down-ray's Leg A). With a [`Site`], also
/// resolve the MODF placement and load the terrain the down-ray races.
fn load_subject(internal: &str, site: Option<&Site>) -> Subject {
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = open_chain(&data).expect("open MPQ chain");
    let placed = site.map(|s| load_placement(&mut chain, s));
    let root_bytes = chain.read(internal).expect("read root wmo");
    let root = parse_wmo_root(&root_bytes).expect("parse root");
    let stem = internal.strip_suffix(".wmo").unwrap_or(internal);

    let n = root.group_count() as usize;
    let mut group_nav: Vec<WmoGroupNav> = (0..n)
        .map(|gi| {
            let (bbox_min, bbox_max) = root
                .group_infos()
                .get(gi)
                .map_or(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |g| {
                    (g.bbox_min, g.bbox_max)
                });
            WmoGroupNav {
                flags: 0,
                bbox_min,
                bbox_max,
                ref_start: 0,
                ref_count: 0,
                area_table_id: 0,
                fog_indices: [0; 4],
                group_liquid: benilla_formats::NO_GROUP_LIQUID,
            }
        })
        .collect();
    let mut group_collision_tris: Vec<Vec<[[f32; 3]; 3]>> = vec![Vec::new(); n];
    let mut group_camera_only_tris: Vec<Vec<[[f32; 3]; 3]>> = vec![Vec::new(); n];
    let mut group_footprints: Vec<Option<benilla_formats::FootprintTris>> =
        (0..n).map(|_| None).collect();
    let mut cam_pos = Vec::new();
    let mut cam_idx = Vec::new();
    let mut walk_pos = Vec::new();
    let mut walk_idx = Vec::new();
    let mut names = vec![String::new(); n];
    let mut group_liquids: Vec<Option<benilla_formats::LiquidMesh>> = vec![None; n];
    for gi in 0..n {
        let Ok(gbytes) = chain.read(&format!("{stem}_{gi:03}.wmo")) else {
            continue;
        };
        // The group's MLIQ surface, exactly as the asset loader stores it — the down-ray's liquid
        // leg (B65) needs it, so the pin probe must carry it too or an over-lava eye can't replay.
        group_liquids[gi] = benilla_formats::wmo_group_liquid_mesh(&gbytes);
        group_footprints[gi] = wmo_group_footprint_tris(&gbytes);
        if let (Some(h), Some(nav)) = (wmo_group_header(&gbytes), group_nav.get_mut(gi)) {
            nav.flags = h.flags;
            nav.ref_start = h.portal_ref_start;
            nav.ref_count = h.portal_ref_count;
            nav.area_table_id = h.area_table_id;
            nav.fog_indices = h.fog_indices;
        }
        accumulate_wmo_group_camera_collision(&gbytes, &mut cam_pos, &mut cam_idx);
        // The down-ray's Leg A face set — the per-group walking-collision gather (every non-DETAIL
        // face, no orientation filter), exactly what the asset loader now stores. The flat walk mesh
        // (headroom probes) is fed from the same buffers.
        let mut cp: Vec<[f32; 3]> = Vec::new();
        let mut ci: Vec<u32> = Vec::new();
        accumulate_wmo_group_collision(&gbytes, &mut cp, &mut ci);
        for t in ci.as_chunks::<3>().0 {
            if let (Some(&a), Some(&b), Some(&c)) = (
                cp.get(t[0] as usize),
                cp.get(t[1] as usize),
                cp.get(t[2] as usize),
            ) {
                group_collision_tris[gi].push([a, b, c]);
            }
        }
        let base = walk_pos.len() as u32;
        walk_pos.extend_from_slice(&cp);
        walk_idx.extend(ci.iter().map(|i| i + base));
        // Leg C's fallback set — the camera-only complement (DETAIL set, NOCAMCOLLIDE clear),
        // exactly as the asset loader stores it (decision 0692).
        let (mut dp, mut di): (Vec<[f32; 3]>, Vec<u32>) = (Vec::new(), Vec::new());
        benilla_formats::accumulate_wmo_group_camera_only_collision(&gbytes, &mut dp, &mut di);
        for t in di.as_chunks::<3>().0 {
            if let (Some(&a), Some(&b), Some(&c)) = (
                dp.get(t[0] as usize),
                dp.get(t[1] as usize),
                dp.get(t[2] as usize),
            ) {
                group_camera_only_tris[gi].push([a, b, c]);
            }
        }
        names[gi] = root
            .group_infos()
            .get(gi)
            .map(|_| format!("g{gi:02}"))
            .unwrap_or_default();
    }
    let portals = root.portals();
    let (group_collision_bounds, collision_bounds) =
        benilla_assets::collision_tri_bounds(&group_collision_tris);
    let model = WmoModel {
        wmo_id: wmo_root_id(&root_bytes),
        submeshes: Vec::new(),
        submesh_group: Vec::new(),
        portal_vertices: portals.vertices.clone(),
        portal_infos: portals.infos.clone(),
        portal_refs: portals.refs.clone(),
        group_nav,
        fogs: root.fogs().to_vec(),
        skybox: root.skybox().map(str::to_owned),
        group_collision_grids: benilla_assets::collision_tri_grids(&group_collision_tris),
        group_collision_tris,
        group_camera_only_tris,
        group_collision_bounds,
        collision_bounds,
        collision: None,
        collision_camera: None,
        doodads: Vec::new(),
        doodad_sets: Vec::new(),
        lights: Vec::new(),
        group_bounds: root.group_infos().to_vec(),
        group_footprint_bounds: benilla_assets::footprint_tri_bounds(&group_footprints),
        group_footprint_grids: benilla_assets::footprint_tri_grids(&group_footprints),
        group_footprints,
        material_ground_type: Vec::new(),
        material_diff_color: Vec::new(),
        group_light_refs: Vec::new(),
        group_liquids,
        doodad_base: Vec::new(),
        doodad_owner: Vec::new(),
        doodad_groups: Vec::new(),
    };
    Subject {
        model,
        cam_pos,
        cam_idx,
        walk_pos,
        walk_idx,
        names,
        placed,
    }
}

/// Resolve a [`Site`]'s MODF placement (by `uniqueId`) and gather the terrain of its 3×3 tile block —
/// a big building's footprint, and any camera orbiting it, can leave the centre tile. The transform is
/// built exactly as the streamer builds it (`terrain_stream`: `wow_to_bevy` position × MODF Euler).
fn load_placement(chain: &mut benilla_formats::Chain, site: &Site) -> Placed {
    let (cx, cy) = site.tile;
    let mut chunks = Vec::new();
    let mut found = None;
    for ty in cy.saturating_sub(1)..=cy + 1 {
        for tx in cx.saturating_sub(1)..=cx + 1 {
            let Ok(tile) = load_tile_mesh(chain, site.map, tx, ty) else {
                continue;
            };
            if let Some(w) = tile.wmos.iter().find(|w| w.unique_id == site.uid) {
                found = Some((w.position, w.rotation));
            }
            chunks.extend(tile.chunks);
        }
    }
    let (position, rotation) = found.unwrap_or_else(|| {
        panic!(
            "MODF uid {} not found in {} tile {:?}",
            site.uid, site.map, site.tile
        )
    });
    let world_from_local = Affine3A::from_scale_rotation_translation(
        Vec3::ONE,
        placement_rotation(rotation),
        wow_to_bevy(position),
    );
    Placed {
        local_from_world: world_from_local.inverse(),
        world_from_local,
        chunks,
    }
}

/// The cell size of the reachability flood's walk grid (yd).
const WALK_STEP: f32 = 0.5;
/// Max step-up per grid step — stairs are ~0.4-yd risers; a player mounts them, not walls.
const MAX_CLIMB: f32 = 1.05;
/// Max step-down per grid step (walking down stairs / small ledges; falls are not walking).
const MAX_DROP: f32 = 2.5;
/// Required clearance above a standing point (yd) — the player capsule.
const HEADROOM: f32 = 1.8;

/// **Reachable** standing points: a BFS over a [`WALK_STEP`] grid across the walking-collision faces,
/// seeded at `start` — only spots a player can actually walk to, stepping at most [`MAX_CLIMB`] up /
/// [`MAX_DROP`] down per cell with [`HEADROOM`] clear above. This kills the phantom "standing spots"
/// (beam tops, doorway lintels, sealed roof-void ceilings) that the raw face-centroid sweep produced —
/// every violation from these spots is a place the director could actually stand.
fn reachable_spots(subject: &Subject, start: [f32; 3]) -> Vec<[f32; 3]> {
    let model = &subject.model;
    let key = |x: f32, y: f32, z: f32| {
        (
            (x / WALK_STEP).round() as i32,
            (y / WALK_STEP).round() as i32,
            (z / WALK_STEP).round() as i32,
        )
    };
    // Ground the seed onto the nearest face below it.
    let ground = |x: f32, y: f32, below: f32, above: f32| -> Option<f32> {
        model
            .group_collision_tris
            .iter()
            .flatten()
            .filter_map(|t| floor_z_at(t, x, y))
            .filter(|&z| z >= below && z <= above)
            .fold(None, |acc: Option<f32>, z| {
                Some(acc.map_or(z, |a: f32| a.max(z)))
            })
    };
    let clear_above = |x: f32, y: f32, z: f32| {
        nearest_hit_mesh(
            &subject.walk_pos,
            &subject.walk_idx,
            [x, y, z + 0.1],
            [0.0, 0.0, 1.0],
            HEADROOM,
        )
        .is_none()
    };
    // No walking through walls: the chest-height segment between the two cells must be clear of the
    // walking mesh (risers sit below it, door lintels above; a wall or rail blocks it). Without this
    // the flood leaks into sealed voids wherever floor levels align across a wall (the porch roof
    // void off the stair landing — the phantom family the flood exists to kill).
    let clear_between = |x: f32, y: f32, z: f32, nx: f32, ny: f32, nz: f32| {
        let h = z.max(nz) + 0.6;
        let (dx, dy) = (nx - x, ny - y);
        let len = (dx * dx + dy * dy).sqrt();
        nearest_hit_mesh(
            &subject.walk_pos,
            &subject.walk_idx,
            [x, y, h],
            [dx / len, dy / len, 0.0],
            len,
        )
        .is_none()
    };
    let Some(z0) = ground(start[0], start[1], start[2] - 3.0, start[2] + 1.0) else {
        panic!("reachability seed has no floor under it");
    };
    let mut spots: Vec<[f32; 3]> = Vec::new();
    let mut queue: Vec<[f32; 3]> = vec![[start[0], start[1], z0]];
    let mut seen = std::collections::HashSet::new();
    seen.insert(key(start[0], start[1], z0));
    while let Some([x, y, z]) = queue.pop() {
        spots.push([x, y, z]);
        for (dx, dy) in [
            (WALK_STEP, 0.0),
            (-WALK_STEP, 0.0),
            (0.0, WALK_STEP),
            (0.0, -WALK_STEP),
        ] {
            let (nx, ny) = (x + dx, y + dy);
            let Some(nz) = ground(nx, ny, z - MAX_DROP, z + MAX_CLIMB) else {
                continue;
            };
            if !clear_above(nx, ny, nz) || !clear_between(x, y, z, nx, ny, nz) {
                continue;
            }
            let k = key(nx, ny, nz);
            if seen.insert(k) {
                queue.push([nx, ny, nz]);
            }
        }
    }
    spots
}

/// Möller–Trumbore, WoW model space: nearest hit `t` along `dir` (unit) from `orig`, within `max_t`,
/// against an indexed triangle mesh.
fn nearest_hit_mesh(
    positions: &[[f32; 3]],
    indices: &[u32],
    orig: [f32; 3],
    dir: [f32; 3],
    max_t: f32,
) -> Option<f32> {
    let mut best: Option<f32> = None;
    for t in indices.as_chunks::<3>().0 {
        let (Some(&a), Some(&b), Some(&c)) = (
            positions.get(t[0] as usize),
            positions.get(t[1] as usize),
            positions.get(t[2] as usize),
        ) else {
            continue;
        };
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let p = [
            dir[1] * e2[2] - dir[2] * e2[1],
            dir[2] * e2[0] - dir[0] * e2[2],
            dir[0] * e2[1] - dir[1] * e2[0],
        ];
        let det = e1[0] * p[0] + e1[1] * p[1] + e1[2] * p[2];
        if det.abs() < 1.0e-8 {
            continue;
        }
        let inv = 1.0 / det;
        let s = [orig[0] - a[0], orig[1] - a[1], orig[2] - a[2]];
        let u = (s[0] * p[0] + s[1] * p[1] + s[2] * p[2]) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = [
            s[1] * e1[2] - s[2] * e1[1],
            s[2] * e1[0] - s[0] * e1[2],
            s[0] * e1[1] - s[1] * e1[0],
        ];
        let v = (dir[0] * q[0] + dir[1] * q[1] + dir[2] * q[2]) * inv;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t_hit = (e2[0] * q[0] + e2[1] * q[1] + e2[2] * q[2]) * inv;
        if t_hit > 1.0e-3 && t_hit < max_t && best.is_none_or(|bt| t_hit < bt) {
            best = Some(t_hit);
        }
    }
    best
}

/// [`nearest_hit_mesh`] against the camera-collision mesh (the LOS pull-in surface).
fn nearest_hit(subject: &Subject, orig: [f32; 3], dir: [f32; 3], max_t: f32) -> Option<f32> {
    nearest_hit_mesh(&subject.cam_pos, &subject.cam_idx, orig, dir, max_t)
}

/// The audit's camera: perspective * look-at, both in Bevy axes (`world_from_local` = identity, so
/// world space == `wow_to_bevy(model space)` — the same convention `compute_pvs` projects with).
fn clip_from_world(eye_local: [f32; 3], target_local: [f32; 3]) -> Mat4 {
    let eye = wow_to_bevy(eye_local);
    let target = wow_to_bevy(target_local);
    let proj = Mat4::perspective_rh(0.9, 16.0 / 9.0, 0.1, 1000.0);
    let view = Mat4::look_at_rh(eye, target, Vec3::Y);
    proj * view
}

/// One flood re-run with the shared [`TraceLog`] recorder — the diagnosis attached to a violation.
fn trace_flood(subject: &Subject, eye_local: [f32; 3], clip: &Mat4) {
    let terrain = subject.terrain_z(eye_local);
    let mut log = super::probe::TraceLog::new(&subject.model, eye_local, terrain);
    super::compute_pvs_traced(
        &subject.model,
        eye_local,
        terrain,
        clip,
        &Affine3A::IDENTITY,
        &mut log,
    );
    print!("{}", log.text);
}

/// Sampling step (yd) of the outside audit's world-column grid, and the camera heights above the
/// terrain surface it probes — a third-person camera's whole vertical band over open ground.
const OUTSIDE_STEP: f32 = 3.0;
const OUTSIDE_HEIGHTS: [f32; 4] = [0.5, 1.7, 4.0, 9.0];

/// **The outside invariant.** Over a column where the whole building lies **below the ground surface**,
/// a camera anywhere above that ground is outside it — and the building's exterior shell therefore
/// draws. Fargodeep Mine is the extreme shape: a 150-yd tunnel network buried under an Elwynn hillside,
/// with a single exterior group covering only the entrance mound. Without the terrain race, every camera
/// column over the hill seeds *inside* a tunnel, the flood starts in a room the camera cannot see out
/// of, and the mine's entrance — the only part of it above ground — is culled (decision 0258).
///
/// The oracle is deliberately restricted to **buried** columns, because "the eye is above the terrain"
/// alone does not mean "the eye is outdoors": a WMO surface can sit above the ADT ground (the mine's own
/// entrance mound stands ~9 yd proud of it; an inn's floor is a foot above it), and an eye over such a
/// surface reads INSIDE in the real client too — its WMO hit is genuinely nearer than the terrain's. The
/// mound columns are covered by the second class below, through the exterior-group rule.
///
/// The last section is the guard against the race *over*-firing and sealing the mine: a player standing
/// on a tunnel floor is below the hill's surface, so the terrain is not on their down-segment at all,
/// and the WMO must still win the column.
#[test]
fn wmo_outside_audit() {
    // The data gate (2331): a skip where no install is, a failure where the gate says one is.
    let _data = benilla_formats::wow_data_or_skip!();
    let subject = load_subject(FARGODEEP.wmo, Some(&FARGODEEP));
    let model = &subject.model;
    let placed = subject.placed.as_ref().expect("placed");
    let exteriors: Vec<usize> = (0..model.group_nav.len())
        .filter(|&gi| model.group_nav[gi].flags & EXTERIOR != 0)
        .collect();
    assert!(!exteriors.is_empty(), "subject has no exterior group");

    // The model's XY extent, walked as a world grid: the columns of hillside a camera can stand over.
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for g in &model.group_bounds {
        for a in 0..2 {
            lo[a] = lo[a].min(g.bbox_min[a]);
            hi[a] = hi[a].max(g.bbox_max[a]);
        }
    }
    let origin = bevy_to_wow(placed.world_from_local.translation.into());
    let reach = (hi[0] - lo[0]).max(hi[1] - lo[1]) * 0.5 + 10.0;

    let mut violations: Vec<([f32; 3], usize)> = Vec::new();
    let mut shell_culled = 0u32;
    let (mut buried, mut proud, mut bare, mut samples) = (0u32, 0u32, 0u32, 0u32);
    let steps = (reach / OUTSIDE_STEP) as i32;
    for i in -steps..=steps {
        for j in -steps..=steps {
            let (wx, wy) = (
                origin[0] + i as f32 * OUTSIDE_STEP,
                origin[1] + j as f32 * OUTSIDE_STEP,
            );
            // The ground here, if this column carries terrain at all (a hole hands it to the WMO).
            let Some(tz) = terrain_height_at(&placed.chunks, [wx, wy, 0.0]) else {
                continue;
            };
            let eye_at = |h: f32| {
                let eye_world = wow_to_bevy([wx, wy, tz + h]);
                bevy_to_wow(placed.local_from_world.transform_point3(eye_world))
            };
            // The building's topmost collision face in this column, in the eye's own frame.
            let probe = eye_at(0.0);
            let Some((top_g, top_z)) = column_top(model, probe[0], probe[1]) else {
                bare += 1; // no building under this column at all
                continue;
            };
            if top_z >= probe[2] {
                // A WMO surface stands above the ground here (the entrance mound). Only the columns
                // whose topmost surface is an EXTERIOR group are asserted — an eye over an *indoor*
                // surface that pokes above ground reads inside in the client too, faithfully.
                proud += 1;
                if model.group_nav[top_g].flags & EXTERIOR == 0 {
                    continue;
                }
            } else {
                buried += 1;
            }
            for h in OUTSIDE_HEIGHTS {
                let eye = eye_at(top_z.max(probe[2]) - probe[2] + h);
                samples += 1;
                let seed = subject.seeds(eye).in_group;
                let pvs = compute_pvs(
                    model,
                    eye,
                    subject.terrain_z(eye),
                    &clip_from_world(eye, [eye[0], eye[1], eye[2] - 10.0]),
                    &Affine3A::IDENTITY,
                );
                let culled = exteriors.iter().any(|&e| !pvs[e]);
                if culled {
                    shell_culled += 1;
                }
                if let Some(g) = seed {
                    violations.push((eye, g));
                }
            }
        }
    }
    println!(
        "== outside audit: {samples} open-air camera samples ==\n\
         columns: {buried} buried, {proud} with a WMO surface above ground, {bare} bare terrain\n\
         cameras that seeded INSIDE: {}   ·   cameras whose exterior shell culled: {shell_culled}",
        violations.len()
    );
    for (eye, g) in violations.iter().take(3) {
        println!(
            "\n-- open-air camera at model ({:.2},{:.2},{:.2}) seeded g{g:02} --",
            eye[0], eye[1], eye[2]
        );
        trace_flood(&subject, *eye, &clip_from_world(*eye, [0.0, 0.0, 0.0]));
    }
    let inside_violations = violations;

    // The race must not seal the mine: ground a standing point on each interior group's own collision
    // and assert the eye there still reads INSIDE (the hill's surface is above it, off the segment).
    let mut sealed: Vec<usize> = Vec::new();
    let mut probed = 0u32;
    for gi in 0..model.group_nav.len() {
        if model.group_nav[gi].flags & EXTERIOR != 0 {
            continue;
        }
        let Some(eye) = interior_standing_eye(model, gi) else {
            continue;
        };
        probed += 1;
        if subject.seeds(eye).in_group.is_none() {
            sealed.push(gi);
            println!(
                "  SEALED g{gi:02}: standing at model ({:.2},{:.2},{:.2}) reads OUTSIDE  (terrain z {:?})",
                eye[0], eye[1], eye[2], subject.terrain_z(eye)
            );
        }
    }
    println!(
        "interior standing points probed: {probed}, sealed: {}",
        sealed.len()
    );

    assert_eq!(
        shell_culled, 0,
        "{shell_culled} open-air cameras culled the building's exterior shell — the director's \
         \"wrongly not visible from the outside\""
    );
    assert!(
        inside_violations.is_empty(),
        "{} open-air cameras seeded inside a buried interior — the terrain race is not winning \
         the column",
        inside_violations.len()
    );
    assert!(
        sealed.is_empty(),
        "the terrain race over-fired: {} interior group(s) {sealed:?} read OUTSIDE from a standing \
         point on their own floor",
        sealed.len()
    );
}

/// The building's topmost collision face over model column `(x, y)`: `(group, z)` of the highest face
/// any group carries there, or `None` if no group's mesh covers the column. Used to tell a *buried*
/// column (whole building below ground) from one where a WMO surface stands proud of the terrain.
fn column_top(model: &WmoModel, x: f32, y: f32) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (gi, tris) in model.group_collision_tris.iter().enumerate() {
        for t in tris {
            if let Some(z) = floor_z_at(t, x, y) {
                if best.is_none_or(|(_, bz)| z > bz) {
                    best = Some((gi, z));
                }
            }
        }
    }
    best
}

/// A standing eye inside group `gi`: drop onto the highest of the group's own collision faces beneath
/// its face-vertex centroid column, then stand [`EYE_HEIGHT`] above it. `None` when the centroid column
/// has no face under it (a ring-shaped room) — the caller skips that group.
fn interior_standing_eye(model: &WmoModel, gi: usize) -> Option<[f32; 3]> {
    let tris = model.group_collision_tris.get(gi)?;
    if tris.is_empty() {
        return None;
    }
    let n = (tris.len() * 3) as f32;
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    for t in tris {
        for v in t {
            cx += v[0];
            cy += v[1];
        }
    }
    let (cx, cy) = (cx / n, cy / n);
    let ceiling = model.group_nav.get(gi)?.bbox_max[2];
    let floor = tris
        .iter()
        .filter_map(|t| floor_z_at(t, cx, cy))
        .filter(|&z| z <= ceiling - EYE_HEIGHT)
        .fold(f32::NEG_INFINITY, f32::max);
    (floor > f32::NEG_INFINITY).then_some([cx, cy, floor + EYE_HEIGHT])
}

#[test]
fn wmo_pvs_audit() {
    // The data gate (2331): a skip where no install is, a failure where the gate says one is.
    let _data = benilla_formats::wow_data_or_skip!();
    // The env override names a subject with no known placement — then there is no terrain leg.
    let over = std::env::var("WOW_AUDIT_WMO").ok();
    let internal = over.clone().unwrap_or_else(|| GOLDSHIRE.wmo.to_string());
    let subject = load_subject(&internal, over.is_none().then_some(&GOLDSHIRE));
    let model = &subject.model;
    let nav = &model.group_nav;

    // --- the graph, for reading the traces ---
    println!("== {internal} — {} groups ==", nav.len());
    for (gi, g) in nav.iter().enumerate() {
        println!(
            "  g{gi:02} flags {:#07x}{} z[{:7.2},{:7.2}]  refs [{}..+{}]  floors {}",
            g.flags,
            if g.flags & 0x8 != 0 { " EXT" } else { "    " },
            g.bbox_min[2],
            g.bbox_max[2],
            g.ref_start,
            g.ref_count,
            model.group_collision_tris.get(gi).map_or(0, Vec::len),
        );
    }
    for (pi, p) in model.portal_infos.iter().enumerate() {
        let verts: Vec<String> = (p.start_vertex..p.start_vertex + p.count)
            .filter_map(|i| model.portal_vertices.get(i as usize))
            .map(|v| format!("({:.1},{:.1},{:.1})", v[0], v[1], v[2]))
            .collect();
        println!(
            "  p{pi:02} plane [{:+.2},{:+.2},{:+.2},{:+.2}]  {}",
            p.plane[0],
            p.plane[1],
            p.plane[2],
            p.plane[3],
            verts.join(" ")
        );
    }
    for (ri, r) in model.portal_refs.iter().enumerate() {
        println!(
            "  ref{ri:02} p{} -> g{:02} side {:+}",
            r.portal, r.group, r.side
        );
    }

    // --- the sweep ---
    // Standing spots come from the reachability flood, seeded at the porch doorway (override with
    // `WOW_AUDIT_START=x,y,z` for another subject) — every spot is somewhere a player can walk.
    let start: [f32; 3] = std::env::var("WOW_AUDIT_START")
        .ok()
        .and_then(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        })
        .unwrap_or([17.0, -2.6, 1.5]);
    let spots = reachable_spots(&subject, start);
    println!("\nreachable spots: {} (from {start:?})", spots.len());

    // (player eye, camera eye, player group, camera seed)
    type Violation = ([f32; 3], [f32; 3], usize, Option<usize>);
    let mut violations: Vec<Violation> = Vec::new();
    let mut samples = 0u32;
    {
        for c in spots.iter().step_by(5) {
            let player = [c[0], c[1], c[2] + EYE_HEIGHT];
            let Some(gp) = subject.seeds(player).in_group else {
                continue; // outside / exterior standing spot — no interior invariant to hold
            };
            for az_i in 0..8 {
                let az = az_i as f32 * std::f32::consts::TAU / 8.0;
                for elev_deg in [-25.0f32, 0.0, 25.0, 55.0] {
                    let el = elev_deg.to_radians();
                    let dir = [az.cos() * el.cos(), az.sin() * el.cos(), el.sin()];
                    for radius in [2.0f32, 4.5, 8.0] {
                        // LOS pull-in: the camera never ends up behind a wall it can't see through.
                        let reach = nearest_hit(&subject, player, dir, radius)
                            .map_or(radius, |t| (t - 0.3).max(t * 0.5));
                        let eye = [
                            player[0] + dir[0] * reach,
                            player[1] + dir[1] * reach,
                            player[2] + dir[2] * reach,
                        ];
                        let clip = clip_from_world(eye, player);
                        let pvs = compute_pvs(
                            model,
                            eye,
                            subject.terrain_z(eye),
                            &clip,
                            &Affine3A::IDENTITY,
                        );
                        samples += 1;
                        if !pvs.get(gp).copied().unwrap_or(false) {
                            violations.push((player, eye, gp, subject.seeds(eye).in_group));
                        }
                    }
                }
            }
        }
    }

    println!(
        "\n== sweep: {samples} samples, {} violations ==",
        violations.len()
    );
    // Group by (player group, camera seed) so one mechanism prints once, with a trace for the first.
    let mut seen: Vec<(usize, Option<usize>)> = Vec::new();
    for (player, eye, gp, gc) in &violations {
        if seen.contains(&(*gp, *gc)) {
            continue;
        }
        seen.push((*gp, *gc));
        let class: Vec<_> = violations
            .iter()
            .filter(|(_, _, vgp, vgc)| vgp == gp && vgc == gc)
            .collect();
        let count = class.len();
        let (mut pz_min, mut pz_max, mut ez_min, mut ez_max) = (
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        );
        for (vp, ve, _, _) in &class {
            pz_min = pz_min.min(vp[2]);
            pz_max = pz_max.max(vp[2]);
            ez_min = ez_min.min(ve[2]);
            ez_max = ez_max.max(ve[2]);
        }
        println!(
            "\n-- player g{gp:02} not in PVS, camera seed {gc:?}  ({count} cases, player z [{pz_min:.1},{pz_max:.1}], eye z [{ez_min:.1},{ez_max:.1}]) --\n   player ({:.2},{:.2},{:.2})  eye ({:.2},{:.2},{:.2})",
            player[0], player[1], player[2], eye[0], eye[1], eye[2]
        );
        trace_flood(&subject, *eye, &clip_from_world(*eye, *player));
    }
    println!("\ngroup names: {:?}", subject.names);
    // Seed-None residue is the real client's own cull at that camera spot (module doc); only a
    // seed-Some violation is ours.
    let hard: Vec<_> = violations
        .iter()
        .filter(|(_, _, _, gc)| gc.is_some())
        .collect();
    let residue = violations.len() - hard.len();
    println!(
        "hard violations: {} · faithful-cull residue: {residue}",
        hard.len()
    );
    assert!(
        hard.is_empty(),
        "{} hard PVS violations (camera seeded a group but the flood missed the player's — see trace)",
        hard.len()
    );
}

/// Decision 0692's repro, on the shipped data: the camera pulled back from the Deadmines zone-in
/// crosses the swirl-portal doorway into the sealed g35/g39 pocket, whose floor is entirely DETAIL
/// faces — camera collision holds the eye there (every back-hemisphere ray hits a camera face within
/// ~16 yd) while Legs A+B find nothing, and the client's own verdict blanks the building around its
/// own camera. Leg C must name the pocket's room; the terrain race must still own the columns where
/// its answer is right (the hilltop lid ~272 yd above the tunnel).
#[test]
fn wmo_camera_void_audit() {
    // The data gate (2331): a skip where no install is, a failure where the gate says one is.
    let _data = benilla_formats::wow_data_or_skip!();
    let subject = load_subject(DEADMINES.wmo, Some(&DEADMINES));
    // The zone-in head: areatrigger 78's destination (-14.5732, -385.475, 62.4561) + head height,
    // mapped through the MODF placement (uid 170633) into B-local space.
    let head = [268.56, -137.17, 34.61];
    assert_eq!(
        subject.seeds(head).in_group,
        Some(35),
        "the entrance tunnel must stay Leg A's answer — Leg C never touches a faithful verdict"
    );
    // Eyes along the swept camera path behind the head: level-back at 8/12/13.2 yd and the raised
    // zoom arc. All sit past the last walking-gather floor; each must still name the pocket's room.
    for eye in [
        [260.6, -137.3, 34.6],
        [256.6, -137.4, 34.6],
        [255.4, -137.4, 34.6],
        [256.0, -137.4, 41.9],
    ] {
        let s = subject.seeds(eye);
        let g = s.in_group.unwrap_or_else(|| {
            panic!("camera-void eye {eye:?} read OUTSIDE — the building blanks around the camera")
        });
        assert!(
            g == 35 || g == 39,
            "camera-void eye {eye:?} named g{g}, not the entrance pocket (g35/g39)"
        );
        assert!(s.across.is_none(), "Leg C seeds a single root");
    }
    // Above the hilltop lid the terrain race owns the column: an eye over open ground must stay
    // outside — the fallback's terrain gate keeps the divergence out of the client's right answers.
    assert_eq!(
        subject.seeds([255.4, -137.4, 280.0]).in_group,
        None,
        "an eye above the terrain lid must remain OUTSIDE"
    );
}

/// **The Deeprun Tram's undersea tube — the map with no `Light.dbc` row.**
///
/// Map 369 carries no `Light.dbc` sphere at all (not even a falloff-0 global, unlike maps 0/1), so
/// every scrap of its atmosphere has to come from the building: the root's MFOG. `Subway.wmo`
/// authors four records, and the undersea stretch is record **2** — pos `(25, -1256, -117)`,
/// radii 154/247, colour RGB(30,53,100), end 236.1 yd, start scalar 0.05. The camera's group
/// there (`Subway_002`, flags `0x2805` ⇒ INTERIOR) names it: `fogIds = (2,0,0,0)`.
///
/// This pins the two links that have to hold for that fog to reach the frame — the down-ray seed
/// claiming the tunnel group, and the selector resolving record 2 over the record-0 seed — on a
/// **global (WDT `MODF`) WMO**, the placement shape the rest of this module's sites never exercise.
#[test]
fn deeprun_tram_undersea_claims_its_own_mfog() {
    // The data gate (2331): a skip where no install is, a failure where the gate says one is.
    let _data = benilla_formats::wow_data_or_skip!();
    let subject = load_subject(r"World\wmo\Dungeon\AZ_Subway\Subway.wmo", None);
    let model = &subject.model;
    // The undersea tunnel group, by its authored fog list rather than a hardcoded index.
    let gi = 2usize;
    let nav = &model.group_nav[gi];
    assert_eq!(
        nav.fog_indices,
        [2, 0, 0, 0],
        "g02 should name MFOG record 2"
    );
    assert_eq!(nav.flags & EXTERIOR, 0, "g02 should be an INTERIOR group");
    assert!(model.fogs.len() >= 2, "4 MFOG records; no count==1 bail");

    let eye = interior_standing_eye(model, gi).expect("a standing eye inside g02");
    println!("eye_local = {eye:?}");
    let seeds = subject.seeds(eye);
    println!("down-ray seeds = {seeds:?}");
    assert_eq!(
        seeds.in_group,
        Some(gi),
        "the down-ray must claim the tunnel group — otherwise no MFOG ever engages"
    );

    let target = crate::wmo_portal::fog::select_wmo_fog(&model.fogs, nav.fog_indices, eye)
        .expect("record 2 covers this eye, so a target must resolve");
    println!("resolved MFOG target = {target:?}");
    let rgb = target.color.map(|c| (c * 255.0).round() as i32);
    assert_eq!(rgb, [30, 53, 100], "the undersea record's authored colour");
    assert!(
        (target.end - 236.1).abs() < 0.5,
        "end {} should be the record's 236.1 yd",
        target.end
    );
}
