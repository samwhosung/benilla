//! Print every render batch's **lighting-relevant material state** — the UNLIT (0x01) / UNFOGGED
//! (0x02) render flags as our reader resolves them, beside the blend mode and texture.
//!
//! `cargo run -p benilla-formats --example batch_lit -- 'Interface\Glues\Models\UI_Tauren\UI_Tauren.m2'`
//!
//! Built for B121: a glue scene's ground is a multi-layer stack whose overlay layers are authored
//! **UNLIT** (drawn fullbright, `colour = c28[0]·E_SH + c28[1]` with `c28[0] = 0`, `0x70c190`),
//! which is why UI_Tauren can author no ambient-fill light and still show a lit ground in the
//! reference. The question this answers is the one a grep cannot: whether
//! the flag survives *our* batch reader's render-flag indexing to the `RenderSubmesh` the material
//! builder consumes. A batch printed `LIT` here that the asset authors UNLIT is a reader bug; the
//! asset's own flag table is a separate read (the render-flag array at header 0x84).

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
