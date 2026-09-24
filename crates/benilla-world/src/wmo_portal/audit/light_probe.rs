//! Entity-light down-ray probes over the audit harness's placed subjects: which group's face wins
//! under a standing point, and which light lane the entity classifier applies there (exterior,
//! day/night matte or footprint bake), printed as maps.

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_formats::{load_tile_mesh, mcsh_shadowed_at, open_chain};

use super::super::interior::footprint_sample;
use super::super::seed::{area_down_ray, down_ray_claim};
use super::super::EXTERIOR_LIT;
use super::{
    floor_z_at, load_subject, reachable_spots, WmoModel, BLACKSMITH, EXTERIOR, GOLDSHIRE, WALK_STEP,
};

/// Dump a tile's WMO placements (model, uid, position), for pinning a new [`Site`]:
/// `WOW_DUMP_TILE=map,x,y`, default Goldshire's `Azeroth,31,49`.
#[test]
#[ignore = "instrument: run by hand — cargo test -p benilla-world --lib wmo_portal::audit::light_probe -- --ignored --nocapture"]
fn dump_tile_placements() {
    let spec = std::env::var("WOW_DUMP_TILE").unwrap_or_else(|_| "Azeroth,31,49".into());
    let parts: Vec<&str> = spec.split(',').collect();
    let (map, tx, ty) = (
        parts[0].to_string(),
        parts[1].trim().parse::<u32>().unwrap(),
        parts[2].trim().parse::<u32>().unwrap(),
    );
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = open_chain(&data).expect("open MPQ chain");
    let tile = load_tile_mesh(&mut chain, &map, tx, ty).expect("load tile");
    println!(
        "== {map} tile ({tx},{ty}) — {} WMO placements ==",
        tile.wmos.len()
    );
    for w in &tile.wmos {
        println!(
            "  uid {:>8}  ({:8.1},{:8.1},{:6.1})  {}",
            w.unique_id, w.position[0], w.position[1], w.position[2], w.model
        );
    }
}

/// The nearest collision face below the probe as `(group, z)`, before the EXTERIOR classification.
fn raw_downray_winner(model: &WmoModel, probe: [f32; 3]) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (gi, tris) in model.group_collision_tris.iter().enumerate() {
        for t in tris {
            if let Some(z) = floor_z_at(t, probe[0], probe[1]) {
                if z <= probe[2] && best.is_none_or(|(_, bz)| z > bz) {
                    best = Some((gi, z));
                }
            }
        }
    }
    best
}

/// The forge floor's lane map, per 0.5 yd cell: a digit is a footprint bake (group mod 10), `D`
/// day/night by MOPY&1, `d` day/night by a footprint miss, `X` an exterior face, `·` no floor.
#[test]
#[ignore = "instrument: run by hand — cargo test -p benilla-world --lib wmo_portal::audit::light_probe -- --ignored --nocapture"]
fn forge_floor_lane_map() {
    let subject = load_subject(BLACKSMITH.wmo, Some(&BLACKSMITH));
    let model = &subject.model;
    println!(
        "== {} — {} groups ==",
        BLACKSMITH.wmo,
        model.group_nav.len()
    );
    for (gi, g) in model.group_nav.iter().enumerate() {
        println!(
            "  g{gi:02} flags {:#07x}{} z[{:7.2},{:7.2}]  floors {}  footprint {}",
            g.flags,
            if g.flags & EXTERIOR != 0 {
                " EXT"
            } else {
                "    "
            },
            g.bbox_min[2],
            g.bbox_max[2],
            model.group_collision_tris.get(gi).map_or(0, Vec::len),
            model
                .group_footprints
                .get(gi)
                .and_then(|f| f.as_ref())
                .map_or(0, |f| f.indices.len() / 3),
        );
    }
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for g in &model.group_bounds {
        for a in 0..2 {
            lo[a] = lo[a].min(g.bbox_min[a]);
            hi[a] = hi[a].max(g.bbox_max[a]);
        }
    }
    println!(
        "\n-- lane map (x {:.0}..{:.0} →, y {:.0}..{:.0} ↓, 0.5 yd cells, ground storey z<4) --",
        lo[0], hi[0], lo[1], hi[1]
    );
    let mut y = lo[1];
    while y <= hi[1] {
        let mut row = String::new();
        let mut x = lo[0];
        while x <= hi[0] {
            let floor = model
                .group_collision_tris
                .iter()
                .flatten()
                .filter_map(|t| floor_z_at(t, x, y))
                .filter(|&z| z <= 4.0)
                .fold(f32::NEG_INFINITY, f32::max);
            row.push(if floor == f32::NEG_INFINITY {
                '·'
            } else {
                let probe = [x, y, floor + 0.1];
                let verdict = area_down_ray(
                    &model.group_collision_tris,
                    &model.group_collision_bounds,
                    &model.group_collision_grids,
                    &model.group_nav,
                    probe,
                    subject.terrain_z(probe),
                    EXTERIOR | EXTERIOR_LIT,
                );
                match verdict {
                    None => 'X',
                    Some(_) => match footprint_sample(model, probe) {
                        Some((fg, _, false)) => char::from_digit((fg % 10) as u32, 10).unwrap(),
                        Some((_, _, true)) => 'D',
                        None => 'd',
                    },
                }
            });
            x += 0.5;
        }
        println!("  y{y:+6.1} {row}");
        y += 0.5;
    }

    // The face stack along a transect: coincident-z faces with different flags are coplanar
    // layers, between which the nearest-face tie-break can land on either.
    println!("\n-- face stacks, y=+0.1, x -2..+4 --");
    let mut x = -2.0f32;
    while x <= 4.0 {
        face_stack(model, x, 0.1, 4.0);
        x += 0.5;
    }
}

/// Print every footprint face over `(x, y)` at `z <= max_z` (z, group, MOPY flags), highest first.
fn face_stack(model: &WmoModel, x: f32, y: f32, max_z: f32) {
    let mut hits: Vec<(f32, usize, u8)> = Vec::new();
    for (gi, fp) in model.group_footprints.iter().enumerate() {
        let Some(fp) = fp else { continue };
        for (ti, tri) in fp.indices.as_chunks::<3>().0.iter().enumerate() {
            let (Some(&a), Some(&b), Some(&c)) = (
                fp.positions.get(tri[0] as usize),
                fp.positions.get(tri[1] as usize),
                fp.positions.get(tri[2] as usize),
            ) else {
                continue;
            };
            if let Some(z) = floor_z_at(&[a, b, c], x, y) {
                if z <= max_z {
                    hits.push((z, gi, fp.mopy_flags.get(ti).copied().unwrap_or(0)));
                }
            }
        }
    }
    hits.sort_by(|p, q| q.0.total_cmp(&p.0));
    let stack: Vec<String> = hits
        .iter()
        .map(|(z, gi, fl)| format!("z{z:+.3} g{gi:02} mopy {fl:#04x}"))
        .collect();
    println!("  ({x:+5.1},{y:+5.1})  [{}]", stack.join(" | "));
}

/// The Goldshire inn: per reachable standing point, the raw nearest-face winner and the verdict
/// (`area_down_ray` at feet + 0.1). The reference lights the entrance corridor as indoors, and
/// its minimap names it "Lion's Pride Inn".
#[test]
#[ignore = "instrument: run by hand — cargo test -p benilla-world --lib wmo_portal::audit::light_probe -- --ignored --nocapture"]
fn inn_corridor_light_probe() {
    let subject = load_subject(GOLDSHIRE.wmo, Some(&GOLDSHIRE));
    let model = &subject.model;
    println!("== {} — {} groups ==", GOLDSHIRE.wmo, model.group_nav.len());
    for (gi, g) in model.group_nav.iter().enumerate() {
        println!(
            "  g{gi:02} flags {:#07x}{} z[{:7.2},{:7.2}]  floors {}",
            g.flags,
            if g.flags & EXTERIOR != 0 {
                " EXT"
            } else {
                "    "
            },
            g.bbox_min[2],
            g.bbox_max[2],
            model.group_collision_tris.get(gi).map_or(0, Vec::len),
        );
    }

    // Feet-level standing points, flooding from the porch doorway.
    let spots = reachable_spots(&subject, [17.0, -2.6, 1.5]);
    println!("reachable spots: {}", spots.len());

    type SpotRow = ([f32; 3], Option<(usize, f32)>, Option<usize>);
    let mut rows: Vec<SpotRow> = Vec::new();
    for s in &spots {
        let probe = [s[0], s[1], s[2] + 0.1];
        let terrain = subject.terrain_z(probe);
        let raw = raw_downray_winner(model, probe);
        let verdict = area_down_ray(
            &model.group_collision_tris,
            &model.group_collision_bounds,
            &model.group_collision_grids,
            &model.group_nav,
            probe,
            terrain,
            EXTERIOR | EXTERIOR_LIT,
        );
        rows.push((*s, raw, verdict));
    }

    // The doorway map, one cell per WALK_STEP: '·' no spot, 'o' outdoors, a digit the interior
    // verdict's group (mod 10), 'X' an EXTERIOR-flagged raw winner.
    let (x_lo, x_hi, y_lo, y_hi) = (2.0f32, 26.0f32, -12.0f32, 12.0f32);
    let cols = ((x_hi - x_lo) / WALK_STEP) as usize + 1;
    let rows_n = ((y_hi - y_lo) / WALK_STEP) as usize + 1;
    let mut grid = vec![vec!['·'; cols]; rows_n];
    for (s, raw, verdict) in &rows {
        if s[0] < x_lo || s[0] > x_hi || s[1] < y_lo || s[1] > y_hi {
            continue;
        }
        let c = ((s[0] - x_lo) / WALK_STEP).round() as usize;
        let r = ((s[1] - y_lo) / WALK_STEP).round() as usize;
        let ch = match (verdict, raw) {
            (Some(g), _) => char::from_digit((*g % 10) as u32, 10).unwrap(),
            (None, Some((g, _))) if model.group_nav[*g].flags & EXTERIOR != 0 => 'X',
            (None, _) => 'o',
        };
        grid[r][c] = ch;
    }
    println!("\n-- verdict map (x {x_lo}..{x_hi} →, y {y_lo}..{y_hi} ↓; digit = interior group, X = EXT-group face won, o = no face/terrain) --");
    for (r, row) in grid.iter().enumerate() {
        let y = y_lo + r as f32 * WALK_STEP;
        println!("  y{y:+6.1} {}", row.iter().collect::<String>());
    }

    // The MOPY layers under the corridor's walk line, which decide whether the reference's
    // flag-filtered sample bakes here rather than taking day/night.
    println!("\n-- corridor face stacks, y=-2.6 --");
    for x in [19.0f32, 17.5, 16.0] {
        face_stack(model, x, -2.6, 4.0);
    }

    // The corridor transect: y ≈ -2.6 (the doorway seed), x from the porch inward.
    println!("\n-- transect y=-2.6, x 22→6 --");
    let mut x = 22.0f32;
    while x >= 6.0 {
        let probe_xy = [x, -2.6f32];
        let floor = model
            .group_collision_tris
            .iter()
            .flatten()
            .filter_map(|t| floor_z_at(t, probe_xy[0], probe_xy[1]))
            .filter(|&z| z <= 4.0)
            .fold(f32::NEG_INFINITY, f32::max);
        if floor > f32::NEG_INFINITY {
            let probe = [x, -2.6, floor + 0.1];
            let terrain = subject.terrain_z(probe);
            let raw = raw_downray_winner(model, probe);
            let verdict = area_down_ray(
                &model.group_collision_tris,
                &model.group_collision_bounds,
                &model.group_collision_grids,
                &model.group_nav,
                probe,
                terrain,
                EXTERIOR | EXTERIOR_LIT,
            );
            let raw_s = raw.map_or("—".to_string(), |(g, z)| {
                format!(
                    "g{g:02}{} z {z:.2}",
                    if model.group_nav[g].flags & EXTERIOR != 0 {
                        " EXT"
                    } else {
                        ""
                    }
                )
            });
            // The classifier's bake or day/night fork: the footprint (MOCV) sample at the probe.
            let lane = if verdict.is_some() {
                match footprint_sample(model, probe) {
                    Some((fg, mocv, false)) => format!("BAKE g{fg:02} mocv {mocv:?}"),
                    Some((fg, _, true)) => format!("DAYNIGHT (MOPY&1, g{fg:02})"),
                    None => "DAYNIGHT (footprint miss)".to_string(),
                }
            } else {
                "exterior".to_string()
            };
            // The terrain MCSH bit under the point, which sets the exterior intensity only over
            // open terrain (on a WMO face the skip-shadow bit, `0x6a8bc7`, forces 2.5), and the
            // world coords for a live `.go` to stand here.
            let (mcsh, world) = match subject.placed.as_ref() {
                Some(p) => {
                    let w = bevy_to_wow(p.world_from_local.transform_point3(wow_to_bevy(probe)));
                    (mcsh_shadowed_at(&p.chunks, w), Some(w))
                }
                None => (None, None),
            };
            println!(
                "  x {x:5.1}  floor {floor:6.2}  raw {raw_s:<18} verdict {:?}  lane {lane}  mcsh {mcsh:?}  world {:?}",
                verdict,
                world.map(|w| [
                    (w[0] * 10.0).round() / 10.0,
                    (w[1] * 10.0).round() / 10.0,
                    (w[2] * 10.0).round() / 10.0
                ])
            );
        } else {
            println!("  x {x:5.1}  (no floor)");
        }
        x -= 0.5;
    }

    // Terrain only, past the porch steps: where the inn's ground shadow ends along the walk-in
    // line (open terrain lights at 2.5, or 0.5 in MCSH shadow).
    println!("\n-- outward y=-2.6, x 22→32 (terrain MCSH) --");
    if let Some(p) = subject.placed.as_ref() {
        let mut x = 22.0f32;
        while x <= 32.0 {
            let local = [x, -2.6f32, 0.5];
            let w = bevy_to_wow(p.world_from_local.transform_point3(wow_to_bevy(local)));
            println!(
                "  x {x:5.1}  world [{:8.1},{:6.1}]  mcsh {:?}",
                w[0],
                w[1],
                mcsh_shadowed_at(&p.chunks, w)
            );
            x += 1.0;
        }
    }
}

/// Classify any world point as the entity light classifier does, per placement in its 3×3 tile
/// block (the raw winner and its MOGP flags, the verdict, the footprint fork, the MCSH bit), from
/// `WOW_LIGHT_AT="map,x,y,z"` in raw WoW world coordinates. A GameObject anchors at its world
/// bounding-box centre, not its position, so probe one at `z + centre`
/// (`benilla-extract m2coll <model>` prints the box).
#[test]
#[ignore = "instrument: run by hand — cargo test -p benilla-world --lib wmo_portal::audit::light_probe -- --ignored --nocapture"]
fn world_point_light_probe() {
    let spec =
        std::env::var("WOW_LIGHT_AT").unwrap_or_else(|_| "Kalimdor,1629.4,-4373.4,31.3".into());
    let parts: Vec<&str> = spec.split(',').collect();
    let map = parts[0].trim().to_string();
    let wx: f32 = parts[1].trim().parse().unwrap();
    let wy: f32 = parts[2].trim().parse().unwrap();
    let wz: f32 = parts[3].trim().parse().unwrap();
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = open_chain(&data).expect("open MPQ chain");
    let (cx, cy) = benilla_formats::world_to_tile(wx, wy);
    let mut chunks = Vec::new();
    let mut placements: Vec<benilla_formats::WmoInstance> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ty in cy.saturating_sub(1)..=cy + 1 {
        for tx in cx.saturating_sub(1)..=cx + 1 {
            let Ok(tile) = load_tile_mesh(&mut chain, &map, tx, ty) else {
                continue;
            };
            for w in tile.wmos {
                if seen.insert(w.unique_id) {
                    placements.push(w);
                }
            }
            chunks.extend(tile.chunks);
        }
    }
    let terrain_wow_z = benilla_formats::terrain_height_at(&chunks, [wx, wy, wz]);
    let mcsh = mcsh_shadowed_at(&chunks, [wx, wy, wz]);
    println!(
        "== {map} ({wx:.1},{wy:.1},{wz:.1}) tile ({cx},{cy}) — {} placements, terrain z \
         {terrain_wow_z:?}, MCSH shadowed {mcsh:?} ==",
        placements.len()
    );
    // `WOW_LIGHT_GRID=radius,step`: the MCSH shadow field around the point, north up and west
    // left; `#` shadowed, `.` lit, blank no chunk. Structured shadows mean the decode is sane.
    if let Ok(grid) = std::env::var("WOW_LIGHT_GRID") {
        let g: Vec<f32> = grid
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        let (radius, step) = (g[0], g[1]);
        let n = (radius / step) as i32;
        println!("-- MCSH field ±{radius} yd @ {step} yd (# shadow, . lit) --");
        for i in (-n..=n).rev() {
            let row: String = (-n..=n)
                .rev()
                .map(|j| {
                    let p = [wx + i as f32 * step, wy + j as f32 * step, wz];
                    match mcsh_shadowed_at(&chunks, p) {
                        Some(true) => '#',
                        Some(false) => '.',
                        None => ' ',
                    }
                })
                .collect();
            println!("  x{:+7.1} {row}", wx + i as f32 * step);
        }
    }
    // The classifier's own probe: the position plus the float-safety lift.
    let probe_world = wow_to_bevy([wx, wy, wz + super::super::interior::POSITION_PROBE_LIFT]);
    let mut subjects: std::collections::HashMap<String, super::Subject> =
        std::collections::HashMap::new();
    for w in &placements {
        let world_from_local = bevy::math::Affine3A::from_scale_rotation_translation(
            bevy::math::Vec3::ONE,
            benilla_assets::coords::placement_rotation(w.rotation),
            wow_to_bevy(w.position),
        );
        let local_from_world = world_from_local.inverse();
        let probe_local = bevy_to_wow(local_from_world.transform_point3(probe_world));
        let subject = subjects
            .entry(w.model.clone())
            .or_insert_with(|| load_subject(&w.model, None));
        let model = &subject.model;
        // The runtime's column pre-filter: only a placement that can own the column prints.
        let owns = model.collision_bounds.is_some_and(|(min, max)| {
            probe_local[0] >= min[0]
                && probe_local[0] <= max[0]
                && probe_local[1] >= min[1]
                && probe_local[1] <= max[1]
                && min[2] <= probe_local[2]
        });
        if !owns {
            continue;
        }
        let terrain_local = terrain_wow_z
            .map(|tz| super::super::interior::terrain_z_local(&local_from_world, probe_world, tz));
        let raw = raw_downray_winner(model, probe_local);
        let zone = area_down_ray(
            &model.group_collision_tris,
            &model.group_collision_bounds,
            &model.group_collision_grids,
            &model.group_nav,
            probe_local,
            terrain_local,
            EXTERIOR,
        );
        let light = down_ray_claim(
            &model.group_collision_tris,
            &model.group_collision_bounds,
            &model.group_collision_grids,
            &model.group_nav,
            probe_local,
            terrain_local,
            EXTERIOR | EXTERIOR_LIT,
        );
        println!(
            "-- uid {} {}  local ({:+.1},{:+.1},{:+.1})  terrain_local {terrain_local:?}",
            w.unique_id, w.model, probe_local[0], probe_local[1], probe_local[2]
        );
        match raw {
            Some((gi, z)) => {
                let f = model.group_nav.get(gi).map_or(0, |g| g.flags);
                println!(
                    "   raw winner g{gi:03} z {z:+.2} flags {f:#08x} [{}{}]",
                    if f & EXTERIOR != 0 { "EXT " } else { "" },
                    if f & EXTERIOR_LIT != 0 { "EXTLIT" } else { "" },
                );
            }
            None => println!("   raw winner: none"),
        }
        // This placement's own lane; the runtime takes the nearest claim across placements
        // (`indoor_verdict_at`), so read the row with the smallest depth.
        let lane = match light {
            None => "exterior-on-terrain (sun + MCSH intensity)".to_string(),
            Some(c) if c.outdoor => format!(
                "exterior-on-wmo g{:02} depth {:.1} (forced lit 2.5 — the skip-shadow bit, 0480)",
                c.group, c.depth
            ),
            Some(c) => match footprint_sample(model, probe_local) {
                Some((fg, mocv, false)) => {
                    format!("BAKE g{fg:02} depth {:.1} mocv {mocv:?}", c.depth)
                }
                Some((fg, _, true)) => format!("DAYNIGHT (MOPY&1, g{fg:02})"),
                None => "DAYNIGHT (footprint miss)".to_string(),
            },
        };
        println!("   zone-text indoor {zone:?}  lane {lane}");
    }
}
