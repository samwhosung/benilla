//! Each detail model's size and the view depth one tuft spans. The fade boundary is a plane at a
//! fixed view depth (the reference texgens camera-space Z), so a deep tuft is sliced and a thin
//! one pops. `cargo run -p benilla-formats --example clutter_size`

use std::collections::BTreeSet;

/// Pixels per yard at `distance` for benilla's 45° vertical FOV (the reference's is ≈44.1° at
/// 16:9) on a 1080-tall viewport, the same basis as `clutter_state`.
fn px_per_yard(distance: f32) -> f32 {
    let fov: f32 = std::f32::consts::PI / 4.0;
    (1080.0 / 2.0) / ((fov / 2.0).tan() * distance)
}

/// The view depth where the 128/255 cutout erases even an opaque texel.
const CROSSING: f32 = 61.113;

fn main() -> anyhow::Result<()> {
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let catalog = benilla_formats::load_ground_effect_catalog(&mut chain, true)?;

    let mut models: BTreeSet<String> = BTreeSet::new();
    for id in 0..4096u32 {
        if let Some(e) = catalog.effect(id) {
            models.extend(e.models().into_iter().map(str::to_string));
        }
    }

    println!(
        "{} detail models. WoW model space: +X north, +Y west, +Z up — so Z is the tuft's HEIGHT \
         and the XY diagonal is the most depth it can present to the camera.",
        models.len()
    );
    println!(
        "\n{:<44} {:>6} {:>6} {:>7} {:>8} {:>8}",
        "model", "w(yd)", "h(yd)", "depth", "px tall", "px deep"
    );

    let mut heights: Vec<f32> = Vec::new();
    let mut depths: Vec<f32> = Vec::new();
    let mut shown = 0;
    for path in &models {
        let Ok(bytes) = chain.read_file(path) else {
            continue;
        };
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, "", &[]) else {
            continue;
        };
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for s in &subs {
            for p in &s.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        if lo[0] > hi[0] {
            continue;
        }
        let (dx, dy, dz) = (hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]);
        // The widest footprint the tuft can turn toward the camera: its horizontal diagonal.
        let depth = (dx * dx + dy * dy).sqrt();
        let width = dx.max(dy);
        heights.push(dz);
        depths.push(depth);
        if shown < 14 {
            let name = path.rsplit('\\').next().unwrap_or(path);
            println!(
                "{name:<44} {width:6.2} {dz:6.2} {depth:7.2} {:8.1} {:8.1}",
                dz * px_per_yard(CROSSING),
                depth * px_per_yard(CROSSING),
            );
            shown += 1;
        }
    }

    let stat = |v: &mut Vec<f32>| {
        v.sort_by(f32::total_cmp);
        (v[0], v[v.len() / 2], v[v.len() - 1])
    };
    let (hlo, hmed, hhi) = stat(&mut heights);
    let (dlo, dmed, dhi) = stat(&mut depths);
    println!(
        "\nheight  min {hlo:.2}  median {hmed:.2}  max {hhi:.2} yd   \
         ({:.0} px tall at the {CROSSING:.1} yd crossing)",
        hmed * px_per_yard(CROSSING)
    );
    println!(
        "depth   min {dlo:.2}  median {dmed:.2}  max {dhi:.2} yd   \
         ({:.0} px deep — the width of the slice the cutout plane can show)",
        dmed * px_per_yard(CROSSING)
    );
    println!(
        "\nA median tuft occupies {dmed:.2} yd of view depth. The cutout plane sweeps that in the \
         time the player walks {dmed:.2} yd, and while it does, the tuft is drawn PARTLY — that is \
         the per-pixel slice. Below roughly a tenth of a yard it is a single-frame pop instead."
    );

    // The sampled mip decides the cut: on the binary-alpha atlases a fragment keeps full alpha only
    // inside an all-opaque 2x2 neighbourhood, which runs out between mip 2 and 3, so past ~3 a tuft
    // erodes and below it leaves cross whole. The reference biases stage 0 by +0.25 (`0x6813d0`);
    // its lambda was measured at 2.9-3.3 at 1152x648, and ~2.45 at 1920x1080 is computed from that.
    const DENSITY: f32 = 100.0; // texels per yard, the median from `clutter_state`
    println!("\nsampled mip at the {CROSSING:.1} yd crossing (median {DENSITY:.0} texels/yd art):");
    for height in [648.0f32, 1080.0, 1440.0, 2160.0] {
        let fov: f32 = std::f32::consts::PI / 4.0;
        let px = (height / 2.0) / ((fov / 2.0).tan() * CROSSING);
        let lambda = (DENSITY / px).log2();
        println!(
            "  {height:5.0}p   lambda {lambda:.2} unbiased   {:.2} with the reference's +0.25",
            lambda + 0.25
        );
    }
    println!(
        "  (past ~3.0 a leaf's texels no longer share a death depth and the cut goes per pixel; \
         below it, whole leaves cross together)"
    );
    Ok(())
}
