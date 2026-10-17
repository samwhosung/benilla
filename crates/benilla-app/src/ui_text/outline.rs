//! The outline blit recipes, byte for byte. The reference composites an outlined font's ring and
//! fill (`outline="NORMAL"`/`"THICK"`, the Number* fonts) into one atlas cell and blits it once
//! (dispatched by `0x5cf310` off the font flags at `[CGxFont+0x180]`: the AA-outline blit
//! `0x5cea30`, or the monochrome one `0x5ce440`), so ring and fill fade together at one alpha; a
//! ring stamped behind a fill would blacken mid-fade on the `α(1−α)` term.

use benilla_ui::script::Outline;

/// What a glyph's cell is baked from, the font flags `0x5cf310` picks its bake by: the outline's
/// ring reach in logical px (NORMAL 1, THICK 2) and the monochrome flag (bit 1). Part of the cell
/// cache's key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct Bake {
    pub(super) radius: u8,
    pub(super) mono: bool,
}

impl Bake {
    pub(super) fn of(outline: Outline, mono: bool) -> Bake {
        let radius = match outline {
            Outline::None => 0,
            Outline::Normal => 1,
            Outline::Thick => 2,
        };
        Bake { radius, mono }
    }

    /// An antialiased bake of ring reach `radius`, for a test.
    #[cfg(test)]
    pub(super) fn ring(radius: u8) -> Bake {
        Bake {
            radius,
            mono: false,
        }
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
    let pad = passes_of(r, dpi);
    // 1 and 2. The ink marked and dilated; the coverage stays alongside for the pack.
    let (map, out_w, out_h) = ringed(cov, w, h, pad);
    let cells = (out_w * out_h) as usize;

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

/// Dilation passes for ring reach `r` (logical px) on a `dpi` raster, also the cell's growth on
/// every side.
fn passes_of(r: u8, dpi: f32) -> u32 {
    (u32::from(r) * (dpi.round().max(1.0) as u32)).max(1)
}

/// The ink (`ink != 0`) of a `w × h` glyph marked 1 in a grid grown `passes` texels each side, then
/// `passes` rings around it: pass `k` marks `k + 2` on every unmarked texel 8-adjacent to an
/// earlier mark (the reference's mask-1-write-2 and mask-3-write-4 passes, `0x5cea30`; `0x5ce440`
/// writes 4 then 2). Returns the marks and the grid's size.
fn ringed(ink: &[u8], w: u32, h: u32, passes: u32) -> (Vec<u8>, u32, u32) {
    let (out_w, out_h) = (w + 2 * passes, h + 2 * passes);
    let mut map = vec![0u8; (out_w * out_h) as usize];
    for row in 0..h {
        for col in 0..w {
            if ink[(row * w + col) as usize] != 0 {
                map[((row + passes) * out_w + (col + passes)) as usize] = 1;
            }
        }
    }
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
    (map, out_w, out_h)
}

/// A monochrome font's 1-bit glyph (`bits`, one byte per texel) baked into its cell. Plain
/// (`0x5ce910`), a set bit is opaque white (`0xffff`) and a clear one empty. Outlined
/// (`0x5ce440`), the glyph's 8-neighbours are an opaque black ring (`0xf000`), and under THICK
/// theirs a second black ring at alpha 7/15 (`0x7000`): no neighbour-count alpha, so a small
/// glyph's ring follows its strokes and its counters stay open past the first ring.
///
/// Deviation: `r × round(dpi)` passes as in [`outlined_cell`], the first `round(dpi)` of them the
/// opaque ring. Same contract as [`outlined_cell`]: `(rgba, out_w, out_h, pad)`, `pad` 0 when plain.
pub(super) fn mono_cell(bits: &[u8], w: u32, h: u32, r: u8, dpi: f32) -> (Vec<u8>, u32, u32, u32) {
    if r == 0 {
        let rgba = bits
            .iter()
            .flat_map(|&b| if b != 0 { [255; 4] } else { [0; 4] })
            .collect();
        return (rgba, w, h, 0);
    }
    let pad = passes_of(r, dpi);
    // The first ring is the inner `pad / r` passes, a logical pixel's worth.
    let inner = pad / u32::from(r);
    let (map, out_w, out_h) = ringed(bits, w, h, pad);
    let rgba = map
        .iter()
        .flat_map(|&m| match u32::from(m) {
            0 => [0; 4],
            1 => [255; 4],
            g if g - 1 <= inner => [0, 0, 0, 255],
            _ => [0, 0, 0, 7 * 17],
        })
        .collect();
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

#[cfg(test)]
mod mono_cell_tests {
    use super::*;

    fn px(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn a_plain_bit_is_opaque_white_and_a_clear_one_empty() {
        let (rgba, w, h, pad) = mono_cell(&[1, 0], 2, 1, 0, 1.0);
        assert_eq!((w, h, pad), (2, 1, 0));
        assert_eq!(px(&rgba, w, 0, 0), [255; 4]);
        assert_eq!(px(&rgba, w, 1, 0), [0; 4]);
    }

    #[test]
    fn thick_rings_a_bit_opaque_then_at_seven_fifteenths() {
        let (rgba, w, h, pad) = mono_cell(&[1], 1, 1, 2, 1.0);
        assert_eq!((w, h, pad), (5, 5, 2));
        assert_eq!(px(&rgba, w, 2, 2), [255; 4], "the glyph");
        for (x, y) in [(1, 1), (2, 1), (3, 2), (1, 3)] {
            assert_eq!(px(&rgba, w, x, y), [0, 0, 0, 255], "first ring at {x},{y}");
        }
        for (x, y) in [(0, 0), (2, 0), (4, 2), (4, 4)] {
            assert_eq!(px(&rgba, w, x, y), [0, 0, 0, 119], "second ring at {x},{y}");
        }
    }

    #[test]
    fn the_rings_follow_the_strokes_where_the_antialiased_bake_fills_a_box() {
        // Two one-texel strokes four apart, as a small numeral's: THICK leaves the middle column
        // at the second ring's 7/15, where the AA bake's neighbour count turns it opaque.
        let bits = [1, 0, 0, 0, 1];
        let (rgba, w, _, pad) = mono_cell(&bits, 5, 1, 2, 1.0);
        assert_eq!(px(&rgba, w, pad + 2, pad)[3], 119);
        let (aa, aw, _, apad) = outlined_cell(&[255, 0, 0, 0, 255], 5, 1, 2, 1.0);
        assert_eq!(px(&aa, aw, apad + 2, apad)[3], 255);
    }

    #[test]
    fn a_ring_keeps_its_logical_weight_at_dpi_two() {
        // NORMAL at dpi 2: two passes, both the opaque first ring.
        let (rgba, w, h, pad) = mono_cell(&[1], 1, 1, 1, 2.0);
        assert_eq!((w, h, pad), (5, 5, 2));
        assert_eq!(px(&rgba, w, 0, 0), [0, 0, 0, 255]);
        assert_eq!(px(&rgba, w, 1, 1), [0, 0, 0, 255]);
    }
}
