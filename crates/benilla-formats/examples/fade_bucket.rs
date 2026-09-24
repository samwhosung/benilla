//! Each matching M2's bounding-sphere radius (the reference's `rec+0x68` source) and the
//! distance-fade band it selects, which depends on size alone. Radii are model-local; a
//! placement's `scale` multiplies them. Output is Blizzard data: never commit it.
//! `cargo run -p benilla-formats --example fade_bucket -- fence`

fn main() -> anyhow::Result<()> {
    let pat = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: fade_bucket <substring>"))?
        .to_lowercase();
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let names: Vec<String> = chain
        .list()?
        .into_iter()
        .map(|e| e.name)
        .filter(|n| {
            let l = n.to_lowercase();
            l.ends_with(".m2") && l.contains(&pat)
        })
        .collect();
    for name in names {
        let Ok(b) = benilla_formats::load_m2_bounds(&mut chain, &name) else {
            continue;
        };
        println!(
            "{:>8.2}  {:<22}  {}",
            b.sphere_radius,
            band(b.sphere_radius),
            name
        );
    }
    Ok(())
}

/// The size buckets of `0x683f80`, in sync with `model_fade.rs`.
fn band(r: f32) -> &'static str {
    match r {
        r if r > 7.0 => "never fades",
        r if r <= 0.5 => "40 -> 50",
        r if r <= 2.5 => "100 -> 125",
        _ => "150 -> 200",
    }
}
