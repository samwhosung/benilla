//! **The outline blit recipe** — the client's baked-outline architecture, byte-for-byte.
//!
//! An outlined font (`outline="NORMAL"/"THICK"`, the Number* fonts) does not draw a ring and a fill
//! as two things. The real client composites both into **one** atlas cell and blits it once
//! (the AA-outline blit `0x5cea30`, dispatched by `0x5cf310` off the font flags at
//! `[CGxFont+0x180]`), which is what makes an outlined string fade correctly: one quad, one alpha,
//! ring and fill thinning together. Stamping a ring behind a fill instead — which this codebase did
//! before the composite cell — blackens mid-fade on the `α(1−α)` compositing term.
//!
//! Under the size ladder these variants had to be **planned**: a census walked the shipped
//! `Fonts.xml` registry for every `(face, size, radius)` triple anything could ask for, baked those,
//! and left any unplanned runtime combination to a legacy stamped-halo fallback. On demand
//! there is nothing to plan and no fallback to keep: a radius that is asked for is a
//! radius that is rasterized, so the composite cell is now the only outline path there is.

/// The cell radius for a font's outline flag: the ring reach in **logical** px (`r=1` NORMAL,
/// `r=2` THICK) and the glyph cache key's variant discriminant.
pub(super) fn radius_of(outline: benilla_ui::script::Outline) -> u8 {
    match outline {
        benilla_ui::script::Outline::None => 0,
        benilla_ui::script::Outline::Normal => 1,
        benilla_ui::script::Outline::Thick => 2,
    }
}

/// The client's AA neighbour-count → outline-alpha LUT (`DAT_0080a8ec`): index = the number of
/// marked cells in the in-bounds 3×3 box (centre included), value = the 4-bit alpha, here
/// pre-widened to 8-bit (`nibble × 17`).
const AA_NEIGHBOUR_LUT: [u8; 10] = [0, 17, 17, 51, 85, 119, 153, 187, 221, 255];

/// Composite one glyph's coverage bitmap into an **outlined cell** — the byte recipe of the
/// client's AA-outline blit (`0x5cea30`, dispatched by `0x5cf310`):
///
/// 1. **Mark** every texel with non-zero coverage.
/// 2. **Dilate** iteratively — each pass marks every virgin texel 8-adjacent to a marked one.
///    The binary runs 1 pass for NORMAL, 2 for THICK; we run `r × round(dpi)` so the ring keeps
///    its logical weight under the device-resolution raster (at `dpi = 1` this IS the byte recipe;
///    the finer grid at retina is the same deliberate resolution upgrade as the raster itself).
/// 3. **Alpha** per texel = [`AA_NEIGHBOUR_LUT`]`[count of marked cells in the in-bounds 3×3
///    box]` — the ring AND the fill edge take the same neighbourhood-graded alpha (interior = 9
///    marked ⇒ opaque; a THICK ring's outer corners grade down to ~⅓ — the real softer outer edge).
/// 4. **Pack**: unmarked ⇒ transparent; zero-coverage (pure ring) ⇒ black at the LUT alpha; else
///    `RGB = the coverage as gray` (the fill ramps *toward the ring's black* at AA edges — not
///    white-with-thin-alpha) at the LUT alpha. The binary quantizes to ARGB4444; we keep 8-bit
///    (same law, no banding — the one deliberate widening, like the LUT `×17`).
///
/// Draw-side law this feeds: one quad per glyph (the quad builder `0x5ccbe0`), vertex color
/// MODULATE — white fill takes the text tint, black ring stays black — and ONE alpha per pass, so a
/// frame fade thins ring+fill together (no `α(1−α)` blackening, the defect the stamped halos had).
///
/// Returns `(rgba, out_w, out_h, pad)`: the cell grows by `pad = r·round(dpi)` texels each side
/// (the binary's cell `em+2/+4` and origin col `1/2`, generalized); the caller shifts bearings by
/// `pad`. The advance is untouched — the step law owns tracking (THICK `+1`, NORMAL none).
pub(super) fn outlined_cell(
    cov: &[u8],
    w: u32,
    h: u32,
    r: u8,
    dpi: f32,
) -> (Vec<u8>, u32, u32, u32) {
    let passes = (u32::from(r) * (dpi.round().max(1.0) as u32)).max(1);
    let pad = passes;
    let (out_w, out_h) = (w + 2 * pad, h + 2 * pad);
    let cells = (out_w * out_h) as usize;

    // 1. Mark coverage into the map (1 = glyph ink), coverage kept alongside for the pack.
    let mut map = vec![0u8; cells];
    for row in 0..h {
        for col in 0..w {
            if cov[(row * w + col) as usize] != 0 {
                map[((row + pad) * out_w + (col + pad)) as usize] = 1;
            }
        }
    }

    // 2. Iterative 8-neighbour dilation: pass k marks virgin cells adjacent to any marked cell
    //    (the binary's mask-1-write-2 / mask-3-write-4 passes, generalized to k marks).
    for pass in 0..passes {
        let mark = (pass + 2) as u8; // 1 = ink, 2.. = ring generations
        for row in 0..out_h {
            for col in 0..out_w {
                let c = (row * out_w + col) as usize;
                if map[c] != 0 {
                    continue;
                }
                'probe: for dr in -1i32..=1 {
                    for dc in -1i32..=1 {
                        let (rr, cc) = (row as i32 + dr, col as i32 + dc);
                        if rr < 0 || rr >= out_h as i32 || cc < 0 || cc >= out_w as i32 {
                            continue;
                        }
                        let n = map[(rr as u32 * out_w + cc as u32) as usize];
                        if n != 0 && n < mark {
                            map[c] = mark;
                            break 'probe;
                        }
                    }
                }
            }
        }
    }

    // 3.+4. Neighbourhood-count alpha + pack.
    let mut rgba = vec![0u8; cells * 4];
    for row in 0..out_h {
        for col in 0..out_w {
            let c = (row * out_w + col) as usize;
            if map[c] == 0 {
                continue;
            }
            let mut count = 0usize;
            for dr in -1i32..=1 {
                for dc in -1i32..=1 {
                    let (rr, cc) = (row as i32 + dr, col as i32 + dc);
                    if rr < 0 || rr >= out_h as i32 || cc < 0 || cc >= out_w as i32 {
                        continue;
                    }
                    if map[(rr as u32 * out_w + cc as u32) as usize] != 0 {
                        count += 1;
                    }
                }
            }
            let alpha = AA_NEIGHBOUR_LUT[count];
            let fill = if row >= pad && row < pad + h && col >= pad && col < pad + w {
                cov[((row - pad) * w + (col - pad)) as usize]
            } else {
                0
            };
            let idx = c * 4;
            // Pure ring: black. Fill: the coverage as gray (ramps toward the ring at AA edges).
            rgba[idx] = fill;
            rgba[idx + 1] = fill;
            rgba[idx + 2] = fill;
            rgba[idx + 3] = alpha;
        }
    }
    (rgba, out_w, out_h, pad)
}

#[cfg(test)]
mod outlined_cell_tests {
    use super::*;

    /// Read texel (x, y) of an RGBA cell.
    fn px(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn normal_ring_takes_the_lut_grades() {
        // A 1×1 fully-inked glyph, NORMAL (1 pass), dpi 1 → a 3×3 cell, all 9 texels marked.
        // Per the byte recipe the alpha is AA_NEIGHBOUR_LUT[in-bounds 3×3 marked count]:
        // centre 9 → 255, edge-mid 6 → 153, corner 4 → 85 — the graded ring, not hard black.
        let (rgba, w, h, pad) = outlined_cell(&[255], 1, 1, 1, 1.0);
        assert_eq!((w, h, pad), (3, 3, 1));
        assert_eq!(px(&rgba, w, 1, 1), [255, 255, 255, 255], "fill core");
        assert_eq!(px(&rgba, w, 1, 0), [0, 0, 0, 153], "edge-mid ring: count 6");
        assert_eq!(px(&rgba, w, 0, 1), [0, 0, 0, 153]);
        assert_eq!(px(&rgba, w, 0, 0), [0, 0, 0, 85], "corner ring: count 4");
        assert_eq!(px(&rgba, w, 2, 2), [0, 0, 0, 85]);
    }

    #[test]
    fn thick_outer_ring_is_the_soft_second_pass() {
        // THICK = a second dilation pass (the binary's mask-3→4 pass): 5×5, all marked; the
        // inner ring sits fully surrounded (count 9 → opaque) while the outer edge grades
        // 153/85 — the real client's softer THICK outer edge.
        let (rgba, w, h, pad) = outlined_cell(&[255], 1, 1, 2, 1.0);
        assert_eq!((w, h, pad), (5, 5, 2));
        assert_eq!(px(&rgba, w, 2, 2), [255, 255, 255, 255], "fill core");
        assert_eq!(
            px(&rgba, w, 1, 1),
            [0, 0, 0, 255],
            "inner ring: fully surrounded"
        );
        assert_eq!(
            px(&rgba, w, 2, 0),
            [0, 0, 0, 153],
            "outer edge-mid: count 6"
        );
        assert_eq!(px(&rgba, w, 0, 0), [0, 0, 0, 85], "outer corner: count 4");
    }

    #[test]
    fn retina_scales_the_pass_count_with_the_raster() {
        // dpi 2, NORMAL: 2 passes ⇒ a 2-physical-px (1 logical) ring, dense — the iterative
        // dilation leaves no stride holes (the legacy stamp offsets did).
        let (rgba, w, h, pad) = outlined_cell(&[255], 1, 1, 1, 2.0);
        assert_eq!((w, h, pad), (5, 5, 2));
        assert_eq!(px(&rgba, w, 2, 2), [255, 255, 255, 255]);
        assert_ne!(px(&rgba, w, 1, 1)[3], 0, "no hole inside the 2-px ring");
        assert_eq!(px(&rgba, w, 0, 0), [0, 0, 0, 85]);
    }

    #[test]
    fn aa_fill_ramps_gray_toward_the_ring() {
        // Half coverage (128): the fill texel takes the coverage as GRAY at the LUT alpha —
        // the byte recipe's `RGB = coverage, A = LUT[count]` — never white-with-thin-alpha.
        let (rgba, w, _, _) = outlined_cell(&[128], 1, 1, 1, 1.0);
        assert_eq!(
            px(&rgba, w, 1, 1),
            [128, 128, 128, 255],
            "gray fill, LUT alpha"
        );
        assert_eq!(px(&rgba, w, 1, 0), [0, 0, 0, 153], "ring stays black");
    }
}
