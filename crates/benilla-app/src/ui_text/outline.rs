//! The outline blit recipe, byte for byte. The reference composites an outlined font's ring and
//! fill (`outline="NORMAL"`/`"THICK"`, the Number* fonts) into one atlas cell and blits it once
//! (the AA-outline blit `0x5cea30`, dispatched by `0x5cf310` off the font flags at
//! `[CGxFont+0x180]`), so ring and fill fade together at one alpha; a ring stamped behind a fill
//! would blacken mid-fade on the `α(1−α)` term.

/// An outline flag's ring reach in logical px (NORMAL 1, THICK 2), also the cache key's radius.
pub(super) fn radius_of(outline: benilla_ui::script::Outline) -> u8 {
    match outline {
        benilla_ui::script::Outline::None => 0,
        benilla_ui::script::Outline::Normal => 1,
        benilla_ui::script::Outline::Thick => 2,
    }
}

/// The reference's neighbour-count to outline-alpha table (`0x80a8ec`), indexed by the marked
/// texels in the in-bounds 3×3 box, centre included; its 4-bit alphas are widened × 17.
const AA_NEIGHBOUR_LUT: [u8; 10] = [0, 17, 17, 51, 85, 119, 153, 187, 221, 255];

/// Composite one glyph's coverage into an outlined cell, the reference's AA-outline blit:
///
/// 1. Mark every texel with coverage.
/// 2. Dilate: each pass marks every unmarked texel 8-adjacent to a marked one, 1 pass for NORMAL
///    and 2 for THICK.
/// 3. Alpha: the [`AA_NEIGHBOUR_LUT`] entry for the marked count in the texel's in-bounds 3×3
///    box, for ring and fill edge alike.
/// 4. Pack: the coverage as gray, so the pure ring is black and the fill ramps toward it.
///
/// Deviation: `r × round(dpi)` passes, so the ring keeps its logical weight on a high-DPI raster,
/// and 8-bit texels where the reference packs ARGB4444, so nothing bands.
///
/// It draws as one quad per glyph (`0x5ccbe0`) whose vertex colour tints the fill and leaves the
/// ring black. Returns `(rgba, out_w, out_h, pad)`: the cell grows `pad` texels each side (the
/// reference's `em+2`/`em+4`); the caller shifts the bearings by `pad`, and the advance stays.
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

    // 1. Mark the ink as 1; the coverage stays alongside for the pack.
    let mut map = vec![0u8; cells];
    for row in 0..h {
        for col in 0..w {
            if cov[(row * w + col) as usize] != 0 {
                map[((row + pad) * out_w + (col + pad)) as usize] = 1;
            }
        }
    }

    // 2. Pass k marks unmarked cells next to an earlier mark (the reference's mask-1-write-2 and
    //    mask-3-write-4 passes).
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

    // 3 and 4. Neighbourhood-count alpha, and pack.
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
            // The pure ring is black; the fill is its coverage as gray.
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
        // A 1×1 inked glyph, NORMAL at dpi 1: a 3×3 cell, all marked; in-bounds counts are 9 at
        // the centre, 6 on an edge, 4 in a corner.
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
        // THICK: 5×5, all marked; the inner ring is fully surrounded, the outer edge grades.
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
        // dpi 2, NORMAL: two passes, a 2 px (1 logical px) ring with no holes.
        let (rgba, w, h, pad) = outlined_cell(&[255], 1, 1, 1, 2.0);
        assert_eq!((w, h, pad), (5, 5, 2));
        assert_eq!(px(&rgba, w, 2, 2), [255, 255, 255, 255]);
        assert_ne!(px(&rgba, w, 1, 1)[3], 0, "no hole inside the 2-px ring");
        assert_eq!(px(&rgba, w, 0, 0), [0, 0, 0, 85]);
    }

    #[test]
    fn aa_fill_ramps_gray_toward_the_ring() {
        // Half coverage packs as gray at the table's alpha, not white at a thin alpha.
        let (rgba, w, _, _) = outlined_cell(&[128], 1, 1, 1, 1.0);
        assert_eq!(
            px(&rgba, w, 1, 1),
            [128, 128, 128, 255],
            "gray fill, LUT alpha"
        );
        assert_eq!(px(&rgba, w, 1, 0), [0, 0, 0, 153], "ring stays black");
    }
}
