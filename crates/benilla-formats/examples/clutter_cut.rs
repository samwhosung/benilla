//! **How the ground-clutter fade boundary cuts** — pixel by pixel, or leaf by leaf?
//!
//! `cargo run -p benilla-formats --example clutter_cut [substring]`
//!
//! The detail-doodad draw alpha-tests `texel.a × ramp(view_depth)` against `detailDoodadAlpha`
//! (128/255), so as the ramp falls the surviving set is `{ texel.a ≥ 0.502 / ramp }`. Whether that
//! reads as a tuft *eroding* — the boundary sweeping through a leaf a pixel at a time — or as the
//! leaf *flipping whole* is decided entirely by the alpha distribution **at the mip the tuft is
//! sampled at**. The alpha arithmetic is settled and identical either way, so when the cut looks
//! wrong the answer is in what is being cut, not in the cutting.
//!
//! Per atlas, per mip level, three numbers:
//!
//! * **`distinct`** — how many distinct alpha values the level carries. **`2` means binary**: the
//!   product `a × ramp` is then either `0` or `ramp`, so the alpha test degenerates to a global
//!   on/off and *every texel of every leaf crosses the threshold at the same view depth*. No
//!   erosion is possible at such a level, at any distance, ever.
//! * **coverage** across the fade band — the share of the atlas still drawn at each depth. Sliding
//!   down = the tuft erodes. Holding flat then dropping to zero = the tuft pops.
//! * **cliff** — how much of the still-drawn coverage goes in the last 0.2 yd before the crossing.
//!   100% means everything that was left leaves in one step.
//!
//! The header line also prints the BLP's own `compression / alpha_bits / alpha_type / has_mips`,
//! because the shipped art is not uniform: some detail atlases carry a properly averaged mip
//! pyramid and some carry a **binary-thresholded** one, and the two behave completely differently
//! at the boundary. Parsed inline from the documented BLP2 layout rather than through the decoder,
//! so this reports what the *file* says independently of how we read it.

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

/// Where in the band to sample coverage — clustered at the 61.11 yd crossing, where even a fully
/// opaque texel fails the test and the whole question lives.
const BANDS: [f32; 8] = [52.5, 54.0, 56.0, 58.0, 60.0, 61.0, 61.1, 61.2];

/// What the BLP2 header says about itself: `compression` (1 = palettized, 2 = DXT, 3 = BGRA8),
/// `alpha_bits`, `alpha_type` (DXT sub-form: 1 = DXT3, 7 = DXT5) and the `has_mipmaps` byte at
/// offset 0x0B — whose value tracks, in the shipped detail art, whether the mip chain is averaged
/// or thresholded.
fn blp_header(bytes: &[u8]) -> Option<(u8, u8, u8, u8)> {
    (bytes.len() >= 20 && &bytes[0..4] == b"BLP2")
        .then(|| (bytes[8], bytes[9], bytes[10], bytes[11]))
}

fn main() -> anyhow::Result<()> {
    let want = std::env::args().nth(1).unwrap_or_default().to_uppercase();
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let catalog = benilla_formats::load_ground_effect_catalog(&mut chain, true)?;

    // Every distinct detail texture the catalog can place.
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

        // What an AVERAGED chain would carry, for comparison — mip0's alpha box-filtered down.
        // This is the control for a binary chain: it says how much of the flat coverage is the
        // art's own and how much is the authored mip pyramid throwing the gradient away.
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
            // The averaged control, printed only where the authored level is binary — that is the
            // only case where the two can disagree, and seeing them side by side is the point.
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
