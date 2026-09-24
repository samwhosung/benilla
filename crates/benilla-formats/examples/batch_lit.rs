//! Every render batch's UNLIT (0x01) and UNFOGGED (0x02) flags as our reader resolves them, with
//! blend mode and texture. A batch printed `lit` that the asset authors UNLIT (the render-flag
//! array at header 0x84) is a reader bug. Glue-scene ground overlays are authored UNLIT and drawn
//! fullbright (`0x70c190`), which is how UI_Tauren's ground is lit with no ambient light.
//! `cargo run -p benilla-formats --example batch_lit -- <m2 path>`

fn main() -> anyhow::Result<()> {
    let virt = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: batch_lit <m2 path>"))?;
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let bytes = chain.read_file(&virt)?;
    let subs = benilla_formats::parse_m2_render_submeshes(&bytes, "", &[])?;
    println!("{virt} — {} batches", subs.len());
    let mut unlit = 0;
    for (i, s) in subs.iter().enumerate() {
        if s.emissive {
            unlit += 1;
        }
        println!(
            "  batch {i:2}: {:<5} {:<9} blend={:<9} tex={}",
            if s.emissive { "UNLIT" } else { "lit" },
            if s.fog_policy as u8 == 0 {
                "unfogged?"
            } else {
                ""
            },
            format!("{:?}", s.blend),
            s.texture.as_deref().unwrap_or("<none>")
        );
    }
    println!("{unlit} of {} batches are UNLIT (fullbright)", subs.len());
    Ok(())
}
