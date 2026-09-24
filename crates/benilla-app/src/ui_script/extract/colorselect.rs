//! The colour picker's **generated art** — the hue disc and the brightness ramp.
//!
//! The reference's `<ColorWheelTexture>` and `<ColorValueTexture>` (`ColorPickerFrame.xml` l.186-221)
//! carry no `file=`, and that is not an oversight: there is no BLP anywhere in the 1.12 MPQ chain
//! that is a colour wheel. The client computes those pixels. The engine core therefore hands us the
//! rect and the widget's HSV ([`QuadContent::ColorWheel`]/[`QuadContent::ColorValue`]) and the
//! pixels are made here — the same division of labour the `<Minimap>` slot uses.
//!
//! **Both images are static**, which is why a live drag regenerates nothing: the disc does not
//! depend on the colour at all, and the strip is `rgb(h, s, 1)` times a fixed greyscale ramp
//! (`hsv_to_rgb` scales all three outputs by `v` and by nothing else, so `rgb(h, s, v)` really is
//! `v · rgb(h, s, 1)`).
//!
//! The reference's drawn appearance, and what each fact cost to get wrong:
//!
//! * **The disc is a 128×128 generated ARGB texture** (`0x78b580`, 16384 dwords into a
//!   function-local static, uploaded once per process), drawn unmodulated — not a vertex-coloured
//!   fan. It never reaches the gradient helper `0x77f910`.
//! * **It is drawn at `V = 1.0`, a `mov`-immediate (`0x78b68b mov [ebp-0x24],0x3f800000`), never
//!   the widget's `[this+0x330]`.** Lowering the brightness slider does not darken the wheel — the
//!   thumb moves and nothing else does. This file first shipped the opposite, as a flagged guess.
//! * **The texel lattice is integers**, `X, Y ∈ [−64, 63]`, drawn iff `X² + Y² ≤ 0x1000` —
//!   boundary *inclusive* — and outside is `0x00000000`, a hard alpha cut with no mask and no
//!   feathering. `S = √(X²+Y²)/64`; `H = (atan2(Y, X) + π)·180/π`, the exact inverse of the pick
//!   law (`0x78bd80`), which is what makes clicking a texel select the colour
//!   that texel shows. Row 0 is `Y = +63`: the top.
//! * **The strip is an 8×8 solid texture of `HSV(H, S, 1)` times a vertical vertex gradient**,
//!   black at the bottom and white at the top (`0x78b8a0` + `0x78b92a call 0x77f910`, and
//!   `0x7705b0` fixes the winding TL/BL/TR/BR). The product is `HSV(H, S, v)` at every row — so
//!   `V` moves no strip pixel either. We compute the same product the other way round, one static
//!   greyscale ramp times the hue tint, which is exact because **this pass composites in gamma
//!   bytes** (decision 0254): the quad's `color × texel` multiply *is* the client's.

use benilla_ui::widget::ColorSelectState;

/// The disc's texel size — the client's own `0x80`, and the size of the reference's element, so at
/// the reference's 1024×768 the image draws 1:1.
pub(super) const WHEEL_PX: u32 = 128;

/// The ramp's texel height (its width is 1 — every column is identical). 256 rows is one step per
/// output level, so the gradient has no banding of its own to add.
pub(super) const RAMP_PX: u32 = 256;

/// The generated-sprite cache keys ([`benilla_assets::WorldAssets::generated_sprite`]).
pub(super) const WHEEL_KEY: &str = "colorselect/wheel";
pub(super) const RAMP_KEY: &str = "colorselect/value-ramp";

/// A colour channel `0.0..=1.0` as the byte an sRGB-format texture wants. The client's colours are
/// already gamma-space values — the same space `Rgba8UnormSrgb` art is authored in — so this is a
/// scale, not a transfer function.
fn byte(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// The hue disc: `WHEEL_PX²` RGBA texels, `0x00000000` outside the unit circle.
///
/// Faithful to `0x78b580` down to the lattice. The client walks **integer** `X, Y ∈ [−64, 63]` and
/// tests `X² + Y² ≤ 0x1000` in integers — so the circle is centred half a texel off the image
/// centre, its boundary texels are *inside*, and its edge is a hard alpha cut with no feathering.
/// Each of those is visible at the rim, and each is reproduced rather than improved: a
/// half-texel-recentred, anti-aliased disc would be a different circle from the one the pick law
/// inverts, and the pick law is what decides which colour a click lands on.
///
/// Row 0 is the image's TOP and `Y = +63` — the first dword the client writes is `(X=−64, Y=+63)`.
/// The widget's y runs up and the pick law reads the *cursor* in that space, so a flipped row order
/// would mirror every hue about the horizontal.
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
                continue; // outside: left as the transparent black it was initialised to
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

/// The brightness ramp: one column, white at the top and black at the bottom, which the strip's
/// quad tints to the live hue. `V` runs 0 at the strip's bottom (the pick law's
/// `(y − bottom)/(top − bottom)`), and row 0 is the top, so row 0 is `V = 1`.
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

/// The tint the **disc** quad draws with: nothing but the frame's alpha.
///
/// The wheel is drawn unmodulated at `V = 1`. That is a `mov`-immediate in the fill loop
/// (`0x78b68b`), not a read of `[this+0x330]`, and a census of `[edi+0x3NN]` over the whole
/// function finds only the two `+0x318` hits — so the brightness slider genuinely does not dim the
/// disc. It reads odd next to every other picker on the planet, and it is what the client does.
pub(super) fn wheel_tint(alpha: f32) -> [f32; 4] {
    [1.0, 1.0, 1.0, alpha]
}

/// The tint the **ramp** quad draws with: the current hue and saturation at full value. The ramp's
/// own greyscale supplies the `v` factor, so the product is `rgb(h, s, v)` at every row.
pub(super) fn ramp_tint(hue: f32, sat: f32, alpha: f32) -> [f32; 4] {
    let rgb = ColorSelectState::hsv_to_rgb(&[hue, sat, 1.0]);
    [rgb[0], rgb[1], rgb[2], alpha]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texel's own `(X, Y)` — the client's integer lattice, `X, Y ∈ [−64, 63]`, row 0 at the
    /// top. Stated once here so a test cannot quietly disagree with [`wheel_pixels`] about it.
    fn lattice(col: usize, row: usize) -> (i32, i32) {
        let half = (WHEEL_PX / 2) as i32;
        (col as i32 - half, half - 1 - row as i32)
    }

    /// The one property the disc must have: the colour a texel *shows* is the colour a click on
    /// that texel *selects*. The right-hand side here is the reference's pick law
    /// (`0x78bd80`), which `benilla-ui`'s own `wheel_hs` transcribes — if the two
    /// ever part company, one of these sides moves and this fails.
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
                // The pick law, in the widget's own normalised coordinates.
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

    /// The disc's cardinal points, spelled out — this is what pins the *orientation* of the whole
    /// image, and it is the half a sign error would silently mirror. Off the reference's fill loop
    /// (`0x78b580`): **red LEFT, chartreuse BOTTOM, cyan RIGHT, violet TOP**, hue rising
    /// counter-clockwise.
    #[test]
    fn the_discs_cardinal_hues_are_the_pick_laws() {
        let (w, h, rgba) = wheel_pixels();
        let at = |col: usize, row: usize| {
            let i = (row * w as usize + col) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2]]
        };
        // The row/col where the lattice coordinate is 0 — not the image centre, because the
        // integer lattice is half a texel off it (`Y = 63 − row` has no zero-crossing at 63.5).
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

    /// The alpha is a hard cut at `X² + Y² ≤ 0x1000`, boundary **inclusive** — no feathering, no
    /// mask. A disc with an opaque square around it would paint a black box over the window; a
    /// disc one texel small would clip the fully-saturated rim the pick law can still reach.
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
        // Saturation 0 is white, and the lattice's zero is at (64, 63) — one texel up from centre.
        let c = ((half - 1) as usize * n + half as usize) * 4;
        assert!(
            rgba[c] > 250 && rgba[c + 1] > 250 && rgba[c + 2] > 250,
            "saturation 0 is white, got {:?}",
            &rgba[c..c + 3]
        );
    }

    /// The ramp times its tint reproduces `hsv_to_rgb` at that row's `V` — the identity the whole
    /// two-static-images design rests on, and the reason this can be one image instead of one per
    /// hue. Checked in the gamma space both sides live in (decision 0254).
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
