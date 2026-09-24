//! Every global-sequence bone channel with its raw keys: for a twinkle the scale values are the
//! effect, and a 0-1 flicker and a 0-20 flare can share a period. `gseqdump <internal\path.m2>`
//! Output is Blizzard data: never commit it.

fn main() -> anyhow::Result<()> {
    let virt = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: gseqdump <m2 path>"))?;
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let bytes = chain.read_file(&virt)?;
    for g in benilla_formats::parse_m2_global_sequence_bones(&bytes) {
        println!("bone {}", g.bone);
        if let Some(t) = &g.translation {
            println!("  T period {}ms keys {:?}", t.period_ms, t.keys);
        }
        if let Some(r) = &g.rotation {
            println!("  R period {}ms keys {:?}", r.period_ms, r.keys);
        }
        if let Some(s) = &g.scale {
            println!("  S period {}ms keys {:?}", s.period_ms, s.keys);
        }
    }
    Ok(())
}
