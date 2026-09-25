//! The colour picker's generated art. The reference's `<ColorWheelTexture>` and
//! `<ColorValueTexture>` (`ColorPickerFrame.xml:186-215`) name no `file=`: the 1.12 client
//! computes the pixels. Both images are static, since `hsv_to_rgb` scales by `v` alone. The
//! client's strip is an 8×8 `HSV(H, S, 1)` texture times a vertex gradient, black at the bottom
//! and white at the top (`0x78b8a0`, calling `0x77f910` at `0x78b92a`; winding `0x7705b0`); a
//! static grey ramp times the hue tint is the same product, as this pass composites in gamma.

use benilla_ui::widget::ColorSelectState;

/// The disc's texel size, the client's `0x80` and the XML element's 128: 1:1 at 1024×768.
pub(super) const WHEEL_PX: u32 = 128;

/// The ramp's height, one texel per output level so it adds no banding; it is one texel wide.
pub(super) const RAMP_PX: u32 = 256;

/// Cache keys for [`benilla_assets::WorldAssets::generated_sprite`], which builds each image once.
pub(super) const WHEEL_KEY: &str = "colorselect/wheel";
pub(super) const RAMP_KEY: &str = "colorselect/value-ramp";

/// A `0.0..=1.0` channel, already gamma-space, as a texel byte: a scale, not a transfer function.
fn byte(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// The hue disc as `0x78b580` fills it: integer `X, Y ∈ [−64, 63]`, drawn iff `X² + Y² ≤ 0x1000`
/// (the rim inside, a hard alpha cut, `0x00000000` outside), `S = √(X² + Y²)/64` and
/// `H = atan2(Y, X)` in degrees plus 180. Row 0 is the top, `Y = +63`. Not recentred or
/// anti-aliased: the pick law (`0x78bd80`) inverts exactly this lattice.
pub(super) fn wheel_pixels() -> (u32, u32, Vec<u8>) {
    let n = WHEEL_PX as usize;
    let half = (WHEEL_PX / 2) as i32;
    let mut rgba = vec![0u8; n * n * 4];
    for row in 0..n {
        // `Y = +63 … −64` down the image.
        let y = half - 1 - row as i32;
        for col in 0..n {
            let x = col as i32 - half;
            let r2 = x * x + y * y;
            if r2 > half * half {
                continue; // outside: stays transparent black
            }
            let hue = (y as f32).atan2(x as f32).to_degrees() + 180.0;
            let sat = (r2 as f32).sqrt() / half as f32;
            let rgb = ColorSelectState::hsv_to_rgb(&[hue, sat, 1.0]);
            let i = (row * n + col) * 4;
            rgba[i] = byte(rgb[0]);
            rgba[i + 1] = byte(rgb[1]);
            rgba[i + 2] = byte(rgb[2]);
            rgba[i + 3] = 255;
        }
    }
    (WHEEL_PX, WHEEL_PX, rgba)
}

/// The brightness ramp: one column, white at the top (`V = 1`) to black at the bottom (`V = 0`).
pub(super) fn ramp_pixels() -> (u32, u32, Vec<u8>) {
    let h = RAMP_PX as usize;
    let mut rgba = vec![0u8; h * 4];
    for row in 0..h {
        let v = 1.0 - (row as f32 + 0.5) / RAMP_PX as f32;
        let c = byte(v);
        let i = row * 4;
        rgba[i] = c;
        rgba[i + 1] = c;
        rgba[i + 2] = c;
        rgba[i + 3] = 255;
    }
    (1, RAMP_PX, rgba)
}

/// The disc's tint, the frame alpha alone: the reference draws the disc at `V = 1`, an immediate
/// in the fill loop (`0x78b68b`), not the widget's `[this+0x330]`, so the brightness slider never
/// dims it.
pub(super) fn wheel_tint(alpha: f32) -> [f32; 4] {
    [1.0, 1.0, 1.0, alpha]
}

/// The ramp's tint, `rgb(h, s, 1)`: times the ramp's grey it is `rgb(h, s, v)` at every row.
pub(super) fn ramp_tint(hue: f32, sat: f32, alpha: f32) -> [f32; 4] {
    let rgb = ColorSelectState::hsv_to_rgb(&[hue, sat, 1.0]);
    [rgb[0], rgb[1], rgb[2], alpha]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lattice(col: usize, row: usize) -> (i32, i32) {
        let half = (WHEEL_PX / 2) as i32;
        (col as i32 - half, half - 1 - row as i32)
    }

    /// The right side is the reference's pick law (`0x78bd80`), as in `benilla-ui`'s `wheel_hs`.
    #[test]
    fn every_texel_shows_the_colour_a_click_on_it_would_pick() {
        let (w, h, rgba) = wheel_pixels();
        assert_eq!((w, h), (WHEEL_PX, WHEEL_PX));
        let half = f32::from(u16::try_from(WHEEL_PX / 2).unwrap());
        for row in 0..h as usize {
            for col in 0..w as usize {
                let (x, y) = lattice(col, row);
                let i = (row * w as usize + col) * 4;
                if x * x + y * y > (WHEEL_PX / 2).pow(2) as i32 {
                    continue;
                }
                let (nx, ny) = (x as f32 / half, y as f32 / half);
                let hue = (ny.atan2(nx) + std::f32::consts::PI).to_degrees();
                let want =
                    ColorSelectState::hsv_to_rgb(&[hue, (nx * nx + ny * ny).sqrt().min(1.0), 1.0]);
                assert_eq!(
                    (rgba[i], rgba[i + 1], rgba[i + 2]),
                    (byte(want[0]), byte(want[1]), byte(want[2])),
                    "texel ({col}, {row}) shows a colour a click there would not pick"
                );
            }
        }
    }

    /// The reference's fill loop (`0x78b580`): red left, chartreuse bottom, cyan right, violet top.
    #[test]
    fn the_discs_cardinal_hues_are_the_pick_laws() {
        let (w, h, rgba) = wheel_pixels();
        let at = |col: usize, row: usize| {
            let i = (row * w as usize + col) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2]]
        };
        // `X = 0` at column 64 and `Y = 0` at row 63: the lattice is half a texel off centre.
        let mid = (WHEEL_PX / 2) as usize;
        let left = at(1, mid);
        let right = at(w as usize - 2, mid);
        assert!(
            left[0] > 200 && left[1] < 40 && left[2] < 40,
            "the left rim is 0°/360° — red, got {left:?}"
        );
        assert!(
            right[0] < 40 && right[1] > 200 && right[2] > 200,
            "the right rim is 180° — cyan, got {right:?}"
        );
        let top = at(mid, 1);
        let bottom = at(mid, h as usize - 2);
        assert!(
            top[2] > 200 && top[1] < 40,
            "the top rim is 270° — violet, got {top:?}"
        );
        assert!(
            bottom[1] > 200 && bottom[2] < 40,
            "the bottom rim is 90° — chartreuse, got {bottom:?}"
        );
    }

    #[test]
    fn the_alpha_is_the_clients_hard_inclusive_cut() {
        let (w, _, rgba) = wheel_pixels();
        let n = w as usize;
        let alpha = |col: usize, row: usize| rgba[(row * n + col) * 4 + 3];
        let half = (WHEEL_PX / 2) as i32;
        for row in 0..n {
            for col in 0..n {
                let (x, y) = lattice(col, row);
                let inside = x * x + y * y <= half * half;
                assert_eq!(
                    alpha(col, row) == 255,
                    inside,
                    "texel ({col}, {row}) = ({x}, {y}), r² = {}",
                    x * x + y * y
                );
            }
        }
        // The boundary itself: (0, −64) is exactly r² = 0x1000 and the client keeps it (`jg`).
        assert_eq!(alpha(half as usize, n - 1), 255, "the cut is inclusive");
        // Saturation 0 is white, at the lattice's zero: texel (64, 63).
        let c = ((half - 1) as usize * n + half as usize) * 4;
        assert!(
            rgba[c] > 250 && rgba[c + 1] > 250 && rgba[c + 2] > 250,
            "saturation 0 is white, got {:?}",
            &rgba[c..c + 3]
        );
    }

    #[test]
    fn the_ramp_times_its_tint_is_the_colour_at_that_brightness() {
        let (_, h, rgba) = ramp_pixels();
        for (hue, sat) in [(0.0, 1.0), (120.0, 0.5), (275.0, 1.0), (60.0, 0.25)] {
            let tint = ramp_tint(hue, sat, 1.0);
            for row in (0..h as usize).step_by(17) {
                let v = 1.0 - (row as f32 + 0.5) / h as f32;
                let want = ColorSelectState::hsv_to_rgb(&[hue, sat, v]);
                let ramp = f32::from(rgba[row * 4]) / 255.0;
                for ch in 0..3 {
                    let got = ramp * tint[ch];
                    assert!(
                        (got - want[ch]).abs() <= 2.0 / 255.0,
                        "row {row} (V={v:.3}) hue {hue}: channel {ch} is {got:.4}, \
                         hsv_to_rgb says {:.4}",
                        want[ch]
                    );
                }
            }
        }
    }

    /// The disc does not read the widget's brightness at all (`0x78b68b`'s literal `1.0f`).
    #[test]
    fn the_disc_ignores_the_brightness_slider() {
        assert_eq!(wheel_tint(1.0), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(
            wheel_tint(0.5),
            [1.0, 1.0, 1.0, 0.5],
            "only the frame alpha rides"
        );
    }
}
