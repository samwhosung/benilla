//! Where the ADT's impassable chunks are around a pin:
//! `cargo run -p benilla-formats --example impass_here -- <map> <x> <y> [radius_chunks]`
//! e.g. `impass_here Azeroth -6601.98 -531.87 10`.
//!
//! An "invisible wall" report names a *place* — a `.go xyz` pin — and the first question is
//! whether the wall is authored terrain at all: an MCNK with header flag bit 1 (`MCNK_IMPASSABLE`)
//! set, or a WMO/M2 hull the reporter walked into. This prints
//! the flag as a top-down chunk map — north up, west left, the in-game map's orientation, one
//! glyph per 33.333 yd chunk — so the answer is a picture of the band rather than a guess, and the
//! pin's own chunk is marked. `what_is_here` answers the other half (is there a model here).
//!
//! The map crosses tile seams on purpose: the flag is per chunk and a band runs across ADT
//! boundaries, so a single-tile view is exactly how you miss the wall you are standing against —
//! B129's pin sits 1.5 yd from its wall, with the flagged chunk in the *next* tile east.
//!
//! Output is Blizzard data — pipe it to the scratchpad, never into the repo.

use std::collections::HashMap;

use benilla_formats::{impassable_at, world_to_tile, TileMesh, CHUNK_SIZE, TILE_SIZE};

/// The map's NW origin: world x/y both run *down* from `+32 tiles` as the grid runs south/east.
const MAP_CENTER: f32 = 32.0 * TILE_SIZE;

/// Global chunk index of a world coordinate on either axis — row from x (grows south), column from
/// y (grows east). The same falling-from-`MAP_CENTER` addressing the client's per-chunk lookups use
/// (the MCSH texel law, `0x69b350`).
fn chunk_index(coord: f32) -> i32 {
    ((MAP_CENTER - coord) / CHUNK_SIZE).floor() as i32
}

/// The world coordinate of a global chunk index's centre (inverse of [`chunk_index`], +½ chunk).
fn chunk_centre(index: i32) -> f32 {
    MAP_CENTER - (index as f32 + 0.5) * CHUNK_SIZE
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = || anyhow::anyhow!("usage: impass_here <map> <x> <y> [radius_chunks]");
    let map = args.next().ok_or_else(usage)?;
    let x: f32 = args.next().ok_or_else(usage)?.parse()?;
    let y: f32 = args.next().ok_or_else(usage)?.parse()?;
    let radius: i32 = args.next().map_or(Ok(8), |r| r.parse())?;

    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;

    let mut tiles: HashMap<(u32, u32), Option<TileMesh>> = HashMap::new();
    let mut flag_at = |chain: &mut _, sx: f32, sy: f32| -> Option<bool> {
        let (tx, ty) = world_to_tile(sx, sy);
        let tile = tiles
            .entry((tx, ty))
            .or_insert_with(|| benilla_formats::load_tile_mesh(chain, &map, tx, ty).ok());
        impassable_at(&tile.as_ref()?.chunks, [sx, sy, 0.0])
    };

    let (pin_row, pin_col) = (chunk_index(x), chunk_index(y));
    let (ptx, pty) = world_to_tile(x, y);
    println!(
        "{map}: pin ({x}, {y}) is tile {ptx}_{pty}, chunk (col {}, row {}) of that tile",
        pin_col.rem_euclid(16),
        pin_row.rem_euclid(16),
    );
    println!("global chunk (row {pin_row}, col {pin_col}); {CHUNK_SIZE:.3} yd per glyph\n");

    // North up (rows step south = falling x), west left (columns step east = falling y).
    println!("  '#' impassable · '.' passable · '@' the pin's chunk · ' ' no tile");
    print!("      ");
    for dc in -radius..=radius {
        // A tile seam every 16 chunks: mark it so a band's tile ownership is readable at a glance.
        print!(
            "{}",
            if (pin_col + dc).rem_euclid(16) == 0 {
                '|'
            } else {
                ' '
            }
        );
    }
    println!();
    for dr in -radius..=radius {
        let row = pin_row + dr;
        print!("{:5} ", row);
        for dc in -radius..=radius {
            let col = pin_col + dc;
            let glyph = match flag_at(&mut chain, chunk_centre(row), chunk_centre(col)) {
                _ if (row, col) == (pin_row, pin_col) => '@',
                Some(true) => '#',
                Some(false) => '.',
                None => ' ',
            };
            print!("{glyph}");
        }
        println!();
    }

    // The pin's own verdict, and how far the wall is in each cardinal direction — the number a
    // retest needs ("walk east 1.5 yd and you should stop").
    let pin_flag = flag_at(&mut chain, x, y);
    println!(
        "\npin chunk: {}",
        match pin_flag {
            Some(true) => "IMPASSABLE (the pin is inside the band)",
            Some(false) => "passable",
            None => "no tile loaded here",
        }
    );
    for (name, dx, dy) in [
        ("north (+x)", 1.0, 0.0),
        ("south (-x)", -1.0, 0.0),
        ("west  (+y)", 0.0, 1.0),
        ("east  (-y)", 0.0, -1.0),
    ] {
        // Step chunk by chunk from the pin until the flag flips, then report the distance to that
        // chunk's *near boundary* — where a mover would actually be stopped.
        let hit = (1..=radius).find(|n| {
            let (sx, sy) = (
                x + dx * *n as f32 * CHUNK_SIZE,
                y + dy * *n as f32 * CHUNK_SIZE,
            );
            flag_at(&mut chain, sx, sy) == Some(true)
        });
        match hit {
            Some(n) => {
                let index =
                    chunk_index(if dx != 0.0 { x } else { y }) + if dx + dy > 0.0 { -n } else { n };
                let near_edge = MAP_CENTER
                    - (if dx + dy > 0.0 { index + 1 } else { index }) as f32 * CHUNK_SIZE;
                let d = (near_edge - if dx != 0.0 { x } else { y }).abs();
                println!("  {name}: impassable chunk {n} away, wall at {d:.2} yd");
            }
            None => println!("  {name}: clear for {radius} chunks"),
        }
    }
    Ok(())
}
