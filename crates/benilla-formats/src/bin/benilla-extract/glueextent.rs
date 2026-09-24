//! `glueextent` — how wide each shipped glue scene's art is, off the chain.
//!
//! The instrument behind the glue framing ceiling: for every `UI_*` diorama it prints the measured
//! [`benilla_formats::ArtExtent`] (the same call the client makes at scene spawn), the authored
//! 4:3 half-extents it is measured against, and the derived facts the framing law acts on — the
//! window aspect at which the art runs out of **width** under 1587's hor+ (past it the law zooms
//! in, holding the width) and the aspect at which it runs out of **height** for a narrow window.
//!
//! Beside the law's own reading (opaque batches, front faces) it prints two looser ones — opaque +
//! alpha-tested, and every batch — so a scene whose measured edge looks wrong against a capture can
//! be read for *which* batches sit at that edge without a second tool.

use anyhow::{Context, Result};
use benilla_formats::{
    authored_half_height, batch_footprint, glue_art_extent, parse_m2_camera,
    parse_m2_render_submeshes, Chain, Coverage, CoverageReader, ModelBlend, GLUE_AUTHORED_ASPECT,
};

/// The seven shipped scenes: the login gate + the six race stages (Gnome→Dwarf, Troll→Orc share).
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
        // The law's reading — every batch painting by its texels (the module's rule) — and the
        // two readings on either side of it, so a scene whose measured edge looks wrong against
        // a capture can be read for which rule moved it: opaque batches only, and every batch
        // as if it painted fully.
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
        // Which batches sit at the measured edge: every opaque batch, with its blend, so a
        // suspicious number can be traced to a card without a second command.
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
            // Every batch's footprint in the frame, in units of t0 (so `±1.333` is the authored
            // 4:3 box's side, `±1.0` its top/bottom): which card sets the edge.
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
