//! Whether the ground-clutter fade erodes a tuft or flips whole leaves, per detail atlas and mip.
//! `cargo run -p benilla-formats --example clutter_cut [substring]`
//!
//! The draw alpha-tests `texel.a × ramp(view_depth)` against 128/255, so the alpha distribution at
//! the sampled mip decides the cut: a level with only 2 distinct alphas crosses all at once.

use std::collections::BTreeSet;

/// The reference's ramp (`0x6b1b6e`): a 64-texel CLAMP/LINEAR table read at texel centres, capped
/// at texel 0's `252/255`.
fn ramp(view_depth: f32, far: f32) -> f32 {
    let near = far * 0.75;
    let u = (view_depth - near) / (far - near);
    ((254.0 - 256.0 * u) / 255.0).clamp(0.0, 252.0 / 255.0)
}

/// The detail-doodad alpha-test reference (`detailDoodadAlpha` = 128).
const CUTOUT: f32 = 128.0 / 255.0;

/// Depths to sample coverage at, clustered at 61.11 yd, where even an opaque texel fails the test.
const BANDS: [f32; 8] = [52.5, 54.0, 56.0, 58.0, 60.0, 61.0, 61.1, 61.2];

/// The BLP2 header's `compression` (1 palettized, 2 DXT, 3 BGRA8), `alpha_bits`, `alpha_type`
/// (1 DXT3, 7 DXT5) and `has_mips` (0x0B), which in the detail art tracks a thresholded chain.
fn blp_header(bytes: &[u8]) -> Option<(u8, u8, u8, u8)> {
    (bytes.len() >= 20 && &bytes[0..4] == b"BLP2")
        .then(|| (bytes[8], bytes[9], bytes[10], bytes[11]))
}

fn main() -> anyhow::Result<()> {
    let want = std::env::args().nth(1).unwrap_or_default().to_uppercase();
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let catalog = benilla_formats::load_ground_effect_catalog(&mut chain, true)?;

    let mut models: BTreeSet<String> = BTreeSet::new();
    for id in 0..4096u32 {
        if let Some(e) = catalog.effect(id) {
            models.extend(e.models().into_iter().map(str::to_string));
        }
    }
    let mut textures: BTreeSet<String> = BTreeSet::new();
    for path in &models {
        let Ok(bytes) = chain.read_file(path) else {
            continue;
        };
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, "", &[]) else {
            continue;
        };
        for s in subs {
            if let Some(t) = s.texture {
                textures.insert(t.to_uppercase());
            }
        }
    }

    println!("cutout {CUTOUT:.4} (detailDoodadAlpha 128); view depth → ramp:");
    for d in BANDS {
        println!("   {d:5.1} yd  ramp {:.4}", ramp(d, 70.0));
    }
    println!("\ncoverage% drawn at each of those depths; `distinct 2` = binary = cannot erode");

    let (mut binary_atlases, mut graded_atlases) = (Vec::new(), Vec::new());
    for tex in textures.iter().filter(|t| t.contains(&want)) {
        let header = chain.read_file(tex).ok().and_then(|b| blp_header(&b));
        let Ok(mips) = benilla_formats::read_texture_mip_chain(&mut chain, tex) else {
            continue;
        };
        if !mips.is_rgba8() {
            continue; // block-compressed levels are not CPU-readable here
        }
        let name = tex.rsplit('\\').next().unwrap_or(tex);
        match header {
            Some((c, ab, at, hm)) => {
                println!("\n{name}  compression={c} alpha_bits={ab} alpha_type={at} has_mips={hm}")
            }
            None => println!("\n{name}"),
        }

        // The control: mip0's alpha box-filtered down, what an averaged chain would carry.
        let mip0_alpha: Vec<u8> = mips.mips[0]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3])
            .collect();
        let mut averaged: Vec<Vec<u8>> = vec![mip0_alpha];
        for i in 1..5usize {
            let (pw, ph) = mips.mip_size(i as u32 - 1);
            let (w, h) = mips.mip_size(i as u32);
            let prev = &averaged[i - 1];
            if prev.len() != (pw * ph) as usize {
                break;
            }
            let mut next = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                for x in 0..w {
                    let sample = |dx: u32, dy: u32| {
                        let (sx, sy) = ((2 * x + dx).min(pw - 1), (2 * y + dy).min(ph - 1));
                        u32::from(prev[(sy * pw + sx) as usize])
                    };
                    let sum = sample(0, 0) + sample(1, 0) + sample(0, 1) + sample(1, 1);
                    next.push((sum / 4) as u8);
                }
            }
            averaged.push(next);
        }

        // Mips 0..4 span roughly 10 yd to 90 yd of viewing distance; 2-3 is the fade band itself.
        let mut binary_below_zero = false;
        for (i, mip) in mips.mips.iter().enumerate().take(5) {
            let (w, h) = mips.mip_size(i as u32);
            let alpha: Vec<u8> = mip.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
            if alpha.len() != (w * h) as usize {
                println!(
                    "  mip{i}: SIZE MISMATCH — {} texels for {w}x{h}",
                    alpha.len()
                );
                continue;
            }
            let mut seen = [false; 256];
            for &v in &alpha {
                seen[v as usize] = true;
            }
            let distinct = seen.iter().filter(|&&s| s).count();
            if i > 0 && distinct <= 2 {
                binary_below_zero = true;
            }

            let drawn = |a: u8, r: f32| f32::from(a) / 255.0 * r >= CUTOUT;
            let coverage = |r: f32| {
                alpha.iter().filter(|&&a| drawn(a, r)).count() as f64 / alpha.len() as f64 * 100.0
            };
            let before = coverage(ramp(61.0, 70.0));
            let after = coverage(ramp(61.2, 70.0));
            let cliff = if before > 0.0 {
                100.0 * (before - after) / before
            } else {
                0.0
            };
            let covs: Vec<String> = BANDS
                .iter()
                .map(|d| format!("{:5.1}", coverage(ramp(*d, 70.0))))
                .collect();
            println!(
                "  mip{i} {w:3}x{h:3} distinct {distinct:<4} [{}]  cliff@61 {cliff:3.0}%",
                covs.join(" ")
            );
            if i > 0 && distinct <= 2 {
                if let Some(avg) = averaged.get(i) {
                    let mut seen = [false; 256];
                    for &v in avg {
                        seen[v as usize] = true;
                    }
                    let cov_avg = |r: f32| {
                        avg.iter().filter(|&&a| drawn(a, r)).count() as f64 / avg.len() as f64
                            * 100.0
                    };
                    let b = cov_avg(ramp(61.0, 70.0));
                    let a2 = cov_avg(ramp(61.2, 70.0));
                    let covs: Vec<String> = BANDS
                        .iter()
                        .map(|d| format!("{:5.1}", cov_avg(ramp(*d, 70.0))))
                        .collect();
                    println!(
                        "    ^ averaged  distinct {:<4} [{}]  cliff@61 {:3.0}%",
                        seen.iter().filter(|&&s| s).count(),
                        covs.join(" "),
                        if b > 0.0 { 100.0 * (b - a2) / b } else { 0.0 }
                    );
                }
            }
        }
        if binary_below_zero {
            binary_atlases.push(name.to_string());
        } else {
            graded_atlases.push(name.to_string());
        }
    }

    println!(
        "\n{} atlases carry a BINARY mip chain below level 0 — every leaf on them flips whole at \
         the crossing, at any distance:",
        binary_atlases.len()
    );
    for a in &binary_atlases {
        println!("   {a}");
    }
    println!(
        "\n{} carry a graded chain and erode normally.",
        graded_atlases.len()
    );
    Ok(())
}
