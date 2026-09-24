//! Every ground-clutter model's batch render state and each detail texture's alpha per mip, the
//! inputs to the ~70 yd clutter fade. The reference's detail pass (`0x6b2b80`) ignores the batches'
//! blend and two-sidedness and forces one state for every tuft; the fade multiplies the texture
//! alpha, so its per-mip distribution is the fade's shape.
//! `cargo run -p benilla-formats --example clutter_state`

use std::collections::{BTreeMap, BTreeSet};

fn main() -> anyhow::Result<()> {
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let catalog = benilla_formats::load_ground_effect_catalog(&mut chain, true)?;

    // Every distinct model the catalog can place; it exposes rows by id only, so sweep the ids.
    let mut models: BTreeSet<String> = BTreeSet::new();
    for id in 0..4096u32 {
        if let Some(e) = catalog.effect(id) {
            models.extend(e.models().into_iter().map(str::to_string));
        }
    }
    println!(
        "{} effect rows, {} distinct detail models",
        catalog.len(),
        models.len()
    );

    let mut textures: BTreeMap<String, ()> = BTreeMap::new();
    let mut blends: BTreeMap<String, usize> = BTreeMap::new();
    let mut two_sided = (0usize, 0usize);
    let mut densities: Vec<f32> = Vec::new();
    for path in &models {
        let Ok(bytes) = chain.read_file(path) else {
            println!("  MISSING {path}");
            continue;
        };
        let subs = match benilla_formats::parse_m2_render_submeshes(&bytes, "", &[]) {
            Ok(s) => s,
            Err(e) => {
                println!("  UNPARSED {path}: {e}");
                continue;
            }
        };
        for s in &subs {
            *blends.entry(format!("{:?}", s.blend)).or_default() += 1;
            if s.two_sided {
                two_sided.0 += 1;
            } else {
                two_sided.1 += 1;
            }
            if let Some(t) = s.texture.as_deref() {
                textures.insert(t.to_string(), ());
            }
            if let Some(dens) = texel_density(s, 256.0) {
                densities.push(dens);
            }
            println!(
                "  {path}: blend={:?} two_sided={} wrap=({},{}) tex={}",
                s.blend,
                s.two_sided,
                s.wrap_x,
                s.wrap_y,
                s.texture.as_deref().unwrap_or("<none>")
            );
        }
    }
    println!("\nbatch blend modes: {blends:?}");
    // Mip level per distance, for benilla's 45° vertical FOV on a 1080-tall viewport.
    let fov: f32 = std::f32::consts::PI / 4.0;
    let px_per_yard = |d: f32| (1080.0 / 2.0) / ((fov / 2.0).tan() * d);
    densities.sort_by(f32::total_cmp);
    if !densities.is_empty() {
        let med = densities[densities.len() / 2];
        println!(
            "\ntexel density over {} batches: min {:.1} / median {med:.1} / max {:.1} texels-per-yard",
            densities.len(),
            densities[0],
            densities[densities.len() - 1]
        );
        for d in [10.0f32, 20.0, 30.0, 40.0, 52.5, 61.2, 70.0] {
            let lod = (med / px_per_yard(d)).log2().max(0.0);
            println!(
                "    d={d:5.1} yd: {:6.1} px/yd -> mip {lod:.2}",
                px_per_yard(d)
            );
        }
    }
    println!("two_sided: {} yes / {} no", two_sided.0, two_sided.1);

    println!("\n=== texture alpha per mip (share of texels in each band) ===");
    for tex in textures.keys() {
        let chainx = match benilla_formats::read_texture_mip_chain(&mut chain, tex) {
            Ok(c) => c,
            Err(e) => {
                println!("  {tex}: unreadable ({e})");
                continue;
            }
        };
        if !chainx.is_rgba8() {
            println!("  {tex}: block-compressed levels (not CPU-readable here)");
            continue;
        }
        println!(
            "  {tex} ({}x{}, {} mips)",
            chainx.width,
            chainx.height,
            chainx.mips.len()
        );
        for (i, mip) in chainx.mips.iter().enumerate() {
            let (w, h) = chainx.mip_size(i as u32);
            let n = mip.len() / 4;
            let (mut zero, mut low, mut mid, mut high, mut full) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for px in mip.as_chunks::<4>().0 {
                match px[3] {
                    0 => zero += 1,
                    1..=63 => low += 1,
                    64..=191 => mid += 1,
                    192..=254 => high += 1,
                    255 => full += 1,
                }
            }
            let p = |v: u32| 100.0 * f64::from(v) / n.max(1) as f64;
            println!(
                "    mip{i} {w:3}x{h:3}: a=0 {:5.1}%  1-63 {:5.1}%  64-191 {:5.1}%  192-254 {:5.1}%  255 {:5.1}%",
                p(zero), p(low), p(mid), p(high), p(full)
            );
        }
    }
    Ok(())
}

/// Texels per yard for a batch, its UV span in atlas texels over its world span; against pixels per
/// yard at a distance it gives the mip sampled there.
fn texel_density(sub: &benilla_formats::RenderSubmesh, atlas: f32) -> Option<f32> {
    if sub.positions.is_empty() || sub.uvs.len() != sub.positions.len() {
        return None;
    }
    let span = |it: &mut dyn Iterator<Item = f32>| {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for v in it {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        hi - lo
    };
    // The tuft's tallest axis in world space (WoW +Z is up) against the same axis in UV (V).
    let world = span(&mut sub.positions.iter().map(|p| p[2]));
    let uv = span(&mut sub.uvs.iter().map(|t| t[1]));
    (world > 1e-3).then(|| uv * atlas / world)
}
