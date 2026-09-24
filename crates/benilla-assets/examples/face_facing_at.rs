//! Which way do the collidable faces under a world pin actually FACE — and would the reference
//! block on them?
//! `cargo run -p benilla-assets --example face_facing_at -- <map> <x> <y> <z> [wmo-substring]`
//! e.g. `face_facing_at Kalimdor -8137.9 -4897.2 2.0 caverns`.
//!
//! The reference's movement collision is **one-sided** (decision 0967): `0x671cc0` emits each
//! candidate face's plane at the **unflipped file winding**, and `0x632700` then processes it
//! only if `n·dir <= -1e-5`. A player falling straight down has `dir` = −up, so **a face blocks
//! the fall iff its authored normal points up**. Ours is a parry trimesh — two-sided — so it
//! blocks either way.
//!
//! This tool turns that difference into a number at a specific `.go xyz` pin: every walking-collidable
//! WMO face whose footprint contains the pin, with its world height, its authored normal's up
//! component, and the reference's verdict. Where a face reads `PASSES`, the reference falls through
//! it and we stand on it.
//!
//! It goes through the **same transform the app bakes colliders with** — `wow_to_bevy` per vertex
//! then the placement `Transform`, exactly as `terrain_stream::collider::placement_collider_data`
//! does — so the geometry here is the geometry the player collides with, not a re-derivation. Both
//! maps are proper rotations, so a face's winding (and the sign of `n·dir`) survives them.
//!
//! Output is Blizzard data — pipe it to the scratchpad, never into the repo.

use benilla_assets::coords::{placement_rotation, wow_to_bevy};
use bevy::prelude::*;

/// The reference's facing gate: `[0x80c5c4] = 0xb727c5ac`. A face is processed iff `n·dir <= EPS`.
const FACING_EPS: f32 = f32::from_bits(0xb727_c5ac);

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = || anyhow::anyhow!("usage: face_facing_at <map> <x> <y> <z> [wmo-substring]");
    let map = args.next().ok_or_else(usage)?;
    let x: f32 = args.next().ok_or_else(usage)?.parse()?;
    let y: f32 = args.next().ok_or_else(usage)?.parse()?;
    let z: f32 = args.next().ok_or_else(usage)?.parse()?;
    let filter = args.next().unwrap_or_default().to_lowercase();

    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let tiles = benilla_formats::MapTiles::load(&mut chain, &map)?;
    let pin = wow_to_bevy([x, y, z]);
    // Falling: WoW −Z, which `wow_to_bevy` carries to Bevy −Y.
    let dir = Vec3::NEG_Y;
    println!("{map}: pin ({x}, {y}, {z}) — faces under a straight-down approach\n");

    // Every placement in the 3x3 tile neighbourhood, deduped by uniqueId (a building spans tiles).
    let mut seen = std::collections::HashSet::new();
    let mut placements: Vec<(String, Transform, u32)> = Vec::new();
    for (tx, ty) in tiles.existing_in_radius(x, y, 1) {
        let tile = benilla_formats::load_tile_mesh(&mut chain, &map, tx, ty)?;
        for w in &tile.wmos {
            if !filter.is_empty() && !w.model.to_lowercase().contains(&filter) {
                continue;
            }
            if seen.insert(w.unique_id) {
                placements.push((
                    w.model.clone(),
                    Transform {
                        translation: wow_to_bevy(w.position),
                        rotation: placement_rotation(w.rotation),
                        scale: Vec3::ONE,
                    },
                    w.unique_id,
                ));
            }
        }
    }

    let mut hits: Vec<Hit> = Vec::new();
    for (model, transform, uid) in &placements {
        let stem = model.to_ascii_lowercase().replace('\\', "/");
        let stem = stem.strip_suffix(".wmo").unwrap_or(&stem).to_string();
        // Walk group files by index — the same `<stem>_NNN.wmo` naming the loader uses. A gap is not
        // expected, but tolerate a few misses rather than trusting a count we did not read.
        let (mut gi, mut misses) = (0u32, 0u32);
        while misses < 4 {
            let Ok(gbytes) = chain.read_file(&format!("{stem}_{gi:03}.wmo")) else {
                misses += 1;
                gi += 1;
                continue;
            };
            misses = 0;
            // The WALKING gather — the player-body audience, skip DETAIL (`0x04`). Identical call to
            // the one the WMO asset loader makes.
            let (mut pos, mut idx) = (Vec::new(), Vec::new());
            benilla_formats::accumulate_wmo_group_collision(&gbytes, &mut pos, &mut idx);
            let verts: Vec<Vec3> = pos
                .iter()
                .map(|p| transform.transform_point(wow_to_bevy(*p)))
                .collect();
            for t in idx.as_chunks::<3>().0 {
                let (Some(&a), Some(&b), Some(&c)) = (
                    verts.get(t[0] as usize),
                    verts.get(t[1] as usize),
                    verts.get(t[2] as usize),
                ) else {
                    continue;
                };
                let Some(height) = plane_height_under(pin, [a, b, c]) else {
                    continue;
                };
                let n = (b - a).cross(c - a);
                if n.length_squared() < 1e-18 {
                    continue;
                }
                hits.push(Hit {
                    height,
                    dot: n.normalize().dot(dir),
                    group: gi,
                    uid: *uid,
                    model: model.clone(),
                });
            }
            gi += 1;
        }
    }

    if hits.is_empty() {
        println!("no walking-collidable WMO face has this pin in its footprint");
        return Ok(());
    }
    hits.sort_by(|p, q| {
        (p.height - pin.y)
            .abs()
            .total_cmp(&(q.height - pin.y).abs())
    });
    println!(
        "{:>9} {:>9} {:>9} {:>8}  {:>4}  wmo",
        "worldZ", "Δ pin", "n·dir", "ref", "grp"
    );
    let (mut blocks, mut passes) = (0usize, 0usize);
    for h in &hits {
        // The pin is Bevy-space; its Y is the WoW Z the tester read off the panel.
        let reference_blocks = h.dot <= FACING_EPS;
        if reference_blocks {
            blocks += 1;
        } else {
            passes += 1;
        }
        println!(
            "{:>9.2} {:>9.2} {:>9.4} {:>8}  {:>4}  #{} {}",
            h.height,
            h.height - pin.y,
            h.dot,
            if reference_blocks { "BLOCKS" } else { "PASSES" },
            h.group,
            h.uid,
            h.model,
        );
    }
    println!(
        "\n{} face(s) under the pin: reference blocks on {blocks}, falls through {passes}. \
         We block on all {} (parry trimesh is two-sided).",
        hits.len(),
        hits.len()
    );
    Ok(())
}

struct Hit {
    /// World height (Bevy +Y = WoW +Z) where the pin's vertical line meets this face's plane.
    height: f32,
    /// The authored winding normal dotted with the fall direction — the reference's gate input.
    dot: f32,
    group: u32,
    uid: u32,
    model: String,
}

/// Where the vertical line through `pin` meets the triangle's plane — `None` unless the pin's
/// horizontal footprint is inside the triangle (the XZ projection in Bevy space, since +Y is up).
fn plane_height_under(pin: Vec3, tri: [Vec3; 3]) -> Option<f32> {
    let p = Vec2::new(pin.x, pin.z);
    let [a, b, c] = tri.map(|v| Vec2::new(v.x, v.z));
    let area = (b - a).perp_dot(c - a);
    if area.abs() < 1e-9 {
        return None; // edge-on to the vertical: no footprint to stand in
    }
    let (u, v) = (
        (b - p).perp_dot(c - p) / area,
        (c - p).perp_dot(a - p) / area,
    );
    let w = 1.0 - u - v;
    (u >= 0.0 && v >= 0.0 && w >= 0.0).then(|| u * tri[0].y + v * tri[1].y + w * tri[2].y)
}
