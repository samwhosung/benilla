//! The WMO and M2 doodad placements within a radius of a pin, nearest first. Two batches of one WMO
//! share its authored MOBA order; a WMO and a nearby doodad have no ordering relationship.
//! Output is Blizzard data: never commit it.
//! `cargo run -p benilla-formats --example what_is_here -- <map> <x> <y> [radius_yd]`

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = || anyhow::anyhow!("usage: what_is_here <map> <x> <y> [radius_yd]");
    let map = args.next().ok_or_else(usage)?;
    let x: f32 = args.next().ok_or_else(usage)?.parse()?;
    let y: f32 = args.next().ok_or_else(usage)?.parse()?;
    let radius: f32 = args.next().map_or(Ok(60.0), |r| r.parse())?;

    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let tiles = benilla_formats::MapTiles::load(&mut chain, &map)?;
    let (tx, ty) = tiles.tile_at(x, y);
    println!("{map}: pin ({x}, {y}) is tile {tx}_{ty}; scanning r={radius} yd across 3x3");

    // Sweep the 3x3 tiles; a placement spanning several is deduped by its uniqueId.
    let mut seen_wmo = std::collections::HashSet::new();
    let mut seen_doodad = std::collections::HashSet::new();
    let mut wmos: Vec<(f32, String, [f32; 3], u32)> = Vec::new();
    let mut doodads: Vec<(f32, String, [f32; 3], f32, u32)> = Vec::new();
    let dist = |p: [f32; 3]| ((p[0] - x).powi(2) + (p[1] - y).powi(2)).sqrt();

    for (tx, ty) in tiles.existing_in_radius(x, y, 1) {
        let tile = benilla_formats::load_tile_mesh(&mut chain, &map, tx, ty)?;
        for w in &tile.wmos {
            let d = dist(w.position);
            if d <= radius && seen_wmo.insert(w.unique_id) {
                wmos.push((d, w.model.clone(), w.position, w.unique_id));
            }
        }
        for m in &tile.doodads {
            let d = dist(m.position);
            if d <= radius && seen_doodad.insert(m.unique_id) {
                doodads.push((d, m.model.clone(), m.position, m.scale, m.unique_id));
            }
        }
    }
    wmos.sort_by(|a, b| a.0.total_cmp(&b.0));
    doodads.sort_by(|a, b| a.0.total_cmp(&b.0));

    println!("\n=== WMOs ({}) ===", wmos.len());
    for (d, model, p, id) in &wmos {
        println!(
            "{d:7.1} yd  #{id:<10} ({:.2}, {:.2}, {:.2})  {model}",
            p[0], p[1], p[2]
        );
    }
    println!("\n=== doodads ({}) ===", doodads.len());
    for (d, model, p, scale, id) in &doodads {
        println!(
            "{d:7.1} yd  #{id:<10} x{scale:.2}  ({:.2}, {:.2}, {:.2})  {model}",
            p[0], p[1], p[2]
        );
    }
    Ok(())
}
