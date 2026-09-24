//! `glueextent`: every shipped glue scene's [`benilla_formats::ArtExtent`], measured off the chain
//! (the numbers `SHIPPED_GLUE_SCENES` transcribes), against its authored 4:3 half-extents, and the
//! window aspects at which its art runs out of width and of height.

use anyhow::{Context, Result};
use benilla_formats::{
    authored_half_height, batch_footprint, glue_art_extent, parse_m2_camera,
    parse_m2_render_submeshes, Chain, Coverage, CoverageReader, ModelBlend, GLUE_AUTHORED_ASPECT,
};

/// The login gate and the six race stages (Gnome shares Dwarf's, Troll shares Orc's).
const SCENES: [&str; 7] = [
    "MainMenu", "Human", "Orc", "Dwarf", "NightElf", "Scourge", "Tauren",
];

pub fn glueextent(chain: &mut Chain, batches: bool) -> Result<()> {
    println!(
        "{:<9} {:>10} {:>7} {:>7} | {:>7} {:>7} | {:>9} {:>9} | {:>7} {:>7}",
        "scene", "fov", "t0", "h0", "half_w", "half_h", "wide@", "narrow@", "opaque", "+all"
    );
    for token in SCENES {
        let name = format!("Interface\\Glues\\Models\\UI_{token}\\UI_{token}.m2");
        let bytes = chain
            .read_file(&name)
            .with_context(|| format!("reading '{name}' from chain"))?;
        let subs = parse_m2_render_submeshes(&bytes, "", &[])
            .with_context(|| format!("parsing M2 render submeshes '{name}'"))?;
        let Some(cam) = parse_m2_camera(&bytes, 0) else {
            println!("{token:<9} (no camera 0)");
            continue;
        };
        let t0 = authored_half_height(cam.fov);
        let h0 = t0 * GLUE_AUTHORED_ASPECT;
        // The texel rule the shipped table is measured by, bracketed by opaque batches only and
        // by every batch painting fully.
        let mut reader = CoverageReader::new(chain);
        let mut paints: Vec<Option<Coverage>> = Vec::with_capacity(subs.len());
        for s in &subs {
            paints.push(reader.coverage(s)?);
        }
        let ext = {
            let mut i = 0;
            glue_art_extent(&subs, &cam, |_| {
                let c = paints[i].clone();
                i += 1;
                c
            })
        };
        let opaque_only = glue_art_extent(
            subs.iter().filter(|s| s.blend == ModelBlend::Opaque),
            &cam,
            |_| Some(Coverage::Full),
        )
        .half_w
            / t0;
        let with_all = glue_art_extent(&subs, &cam, |_| Some(Coverage::Full)).half_w / t0;
        println!(
            "{token:<9} {:>10.7} {:>7.4} {:>7.4} | {:>7.4} {:>7.4} | {:>9.3} {:>9.3} | {:>7.3} {:>7.3}",
            cam.fov,
            t0,
            h0,
            ext.half_w,
            ext.half_h,
            ext.half_w / t0,
            if ext.half_h > 0.0 { h0 / ext.half_h } else { f32::NAN },
            opaque_only,
            with_all,
        );
        let mut kinds = [0usize; 5];
        for s in &subs {
            kinds[match s.blend {
                ModelBlend::Opaque => 0,
                ModelBlend::AlphaTest => 1,
                ModelBlend::Blend => 2,
                ModelBlend::Mod => 3,
                ModelBlend::Mod2x => 4,
            }] += 1;
        }
        println!(
            "          {} batches: {} opaque, {} alpha-test, {} blend, {} mod, {} mod2x; camera eye {:?} target {:?} near {:.3}",
            subs.len(),
            kinds[0],
            kinds[1],
            kinds[2],
            kinds[3],
            kinds[4],
            cam.position,
            cam.target,
            cam.near_clip,
        );
        if batches {
            // Each batch's footprint in units of t0: ±1.333 is the 4:3 box's side, ±1.0 its top.
            println!(
                "          {:>3} {:<9} {:<3} {:<6} {:>5} {:>5} {:>4}  {:>14}  {:>14}  tex",
                "idx", "blend", "2s", "paints", "front", "back", "clip", "x'/t0", "y'/t0"
            );
            for (i, s) in subs.iter().enumerate() {
                let fp = batch_footprint(s, &cam);
                let paints = match &paints[i] {
                    Some(Coverage::Full) => "full",
                    Some(Coverage::Alpha(_)) => "alpha",
                    None => "no",
                };
                let range = |r: Option<(f32, f32)>| {
                    r.map_or("—".to_string(), |(lo, hi)| {
                        format!("{:+.2}..{:+.2}", lo / t0, hi / t0)
                    })
                };
                println!(
                    "          {i:>3} {:<9} {:<3} {:<6} {:>5} {:>5} {:>4}  {:>14}  {:>14}  {}",
                    format!("{:?}", s.blend),
                    if s.two_sided { "yes" } else { "no" },
                    paints,
                    fp.front,
                    fp.back,
                    fp.clipped,
                    range(fp.x),
                    range(fp.y),
                    s.texture.as_deref().unwrap_or("-"),
                );
            }
        }
    }
    println!();
    println!(
        "t0/h0: the authored 4:3 vertical/horizontal half-extents (tan units); half_w/half_h: the"
    );
    println!(
        "opaque art's measured half-extents across the authored opening; wide@: the window aspect"
    );
    println!(
        "past which the law holds the width and zooms (half_w/t0); narrow@: the aspect below which"
    );
    println!("it holds the height (h0/half_h). opaque / +all: wide@ counting opaque batches only / every batch.");
    Ok(())
}
