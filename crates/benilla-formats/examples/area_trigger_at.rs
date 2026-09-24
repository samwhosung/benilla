//! Each area trigger's volume, with a `.go` spot outside it and the facing that walks you in.
//! `cargo run -p benilla-formats --example area_trigger_at -- <id|map:N>...`
//!
//! The volume lives only in `AreaTrigger.dbc`; the server's teleport row holds the destination. A
//! `.go` that lands inside races the server's re-check against its stored position and is
//! sometimes ignored, so a portal is tested by walking in.
//! Output is Blizzard data: never commit it.

/// Yards outside the volume for the approach spot.
const APPROACH: f32 = 12.0;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        anyhow::bail!("usage: area_trigger_at <id|map:N>...");
    }
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let cat = benilla_formats::load_area_trigger_catalog(&mut chain)?;
    println!("AreaTrigger.dbc — {} rows\n", cat.len());

    for arg in &args {
        let rows: Vec<_> = if let Some(m) = arg.strip_prefix("map:") {
            cat.on_map(m.parse()?).iter().collect()
        } else {
            cat.get(arg.parse()?).into_iter().collect()
        };
        if rows.is_empty() {
            println!("no rows for {arg:?}\n");
            continue;
        }
        for r in rows {
            let [x, y, z] = r.position;
            if r.radius > 0.0 {
                println!(
                    "id {:5}  map {:3}  SPHERE r={:.1} at ({x:.2}, {y:.2}, {z:.2})",
                    r.id, r.map_id, r.radius
                );
                // A sphere has no authored axis: approach along -Y, an arbitrary choice.
                let sy = y + r.radius + APPROACH;
                println!(
                    "        walk in:  .go xyz {x:.2} {sy:.2} {z:.2} {}   then face -Y (yaw {:.3}) and hold forward",
                    r.map_id,
                    std::f32::consts::FRAC_PI_2 * 3.0,
                );
            } else {
                let [bx, by, bz] = r.box_size;
                println!(
                    "id {:5}  map {:3}  BOX {bx:.1}x{by:.1}x{bz:.1} yaw {:.3} at ({x:.2}, {y:.2}, {z:.2})",
                    r.id, r.map_id, r.box_yaw
                );
                // Approach along the box's local +X axis, backing off half its depth plus the margin.
                let (s, c) = r.box_yaw.sin_cos();
                let back = bx * 0.5 + APPROACH;
                let (sx, sy) = (x - c * back, y - s * back);
                println!(
                    "        walk in:  .go xyz {sx:.2} {sy:.2} {z:.2} {}   then face yaw {:.3} and hold forward",
                    r.map_id, r.box_yaw
                );
            }
        }
        println!();
    }
    Ok(())
}
