//! Every render batch's animated alpha over one sequence, `colourAlpha × weight` in
//! `A = instanceAlpha × colourAlpha × weight`. A batch that never reaches exactly 1.0 stays on the
//! fade blend twin, which writes depth even for a `no_depth_write` batch (M2 render flag 0x10).
//! `cargo run -p benilla-formats --example batch_alpha -- 'Creature\Voidwalker\Voidwalker.m2' 0`

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let virt = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: batch_alpha <m2 path> [sequence slot]"))?;
    let slot: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let bytes = chain.read_file(&virt)?;
    let subs = benilla_formats::parse_m2_render_submeshes(&bytes, "", &[])?;
    println!("sequence slot {slot} — {} batches", subs.len());
    for (i, s) in subs.iter().enumerate() {
        let Some(a) = s.alpha_anim.as_ref() else {
            println!(
                "  batch {i:2}: no alpha_anim (static 1.0)  depth_write={}",
                !s.no_depth_write
            );
            continue;
        };
        let samples: Vec<f32> = (0..24u16)
            .map(|k| a.sample(Some(slot), 3.0 * f32::from(k) / 24.0, 0.0))
            .collect();
        let lo = samples.iter().copied().fold(f32::MAX, f32::min);
        let hi = samples.iter().copied().fold(f32::MIN, f32::max);
        let ever_one = samples.iter().any(|&v| v >= 1.0);
        println!(
            "  batch {i:2}: alpha {lo:.4}..{hi:.4}  ever==1.0: {ever_one:<5} \
             depth_write={:<5} additive={:<5} {}",
            !s.no_depth_write,
            s.additive,
            if ever_one {
                ""
            } else {
                "<-- STUCK on the blend twin"
            }
        );
    }
    Ok(())
}
