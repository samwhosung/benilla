//! **Is it the same surface lit differently, or a different surface?** — the reading that separates
//! the two explanations a two-state flip always admits.
//!
//! [`Region::steps`] measures that a run alternates between two levels and that its pixels move
//! together. That is where the evidence used to stop, and the gap mattered: B38's tent was read off
//! the per-channel *ratios* as "two materials trading places", on the argument that one light term
//! switching would scale all three channels equally. **That argument is wrong.** Equal ratios rule
//! out a scalar multiply and nothing else — an *additive* term (a coloured light arriving or
//! leaving) changes the ratios freely. Measured there: dim `(77, 42, 32)` → bright `(192, 79, 35)`
//! is "×2.50 / ×1.89 / ×1.12", which reads as two materials, and is *also* exactly the dim state
//! plus a warm `(115, 37, 3)`. The ratios cannot tell those apart, so they were never evidence.
//!
//! What does tell them apart is the **spatial pattern**, which the means throw away. Re-lighting a
//! surface is an affine map on its pixels — every pixel keeps its place in the pattern, so
//! `bright ≈ gain·dim + offset` holds *per pixel* and the fit is tight. Two different surfaces have
//! unrelated patterns (canvas weave vs plank grain), and no affine map relates them: the fit is
//! loose however well the means happen to line up. So: least-squares fit one frame's pixels onto the
//! other's, per channel, and report R². That is a number, not an inference from a number.
//!
//! Fit across the run's **largest single frame-to-frame step**, never across the whole burst: the
//! camera pans during a capture, and a pixel only names the same bit of world for as long as the
//! image holds still under it. Adjacent frames at the sub-pixel-per-frame pan the toggle map needs
//! are the same view; frames twenty apart are not.

use image::RgbImage;

use crate::Region;

/// A per-channel affine fit of one frame's pixels onto another's, over a [`Region`]'s own pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Relight {
    /// Per channel, the `a` in `to ≈ a·from + b`. Near 1 with a non-zero [`Relight::offset`] is a
    /// light being *added*; well above 1 with an offset near 0 is one being *scaled*.
    pub gain: [f64; 3],
    /// Per channel, the `b` in `to ≈ a·from + b`, in 0..255 units.
    pub offset: [f64; 3],
    /// Per channel coefficient of determination, 0..1 — how much of the target frame's variation
    /// across the run the fit explains. **This is the reading.** Near 1: one surface, re-lit (the
    /// pattern survived, only its scale and offset moved). Low: the two frames show *different*
    /// surfaces, and no amount of re-lighting maps one to the other.
    pub r2: [f64; 3],
    /// Did this channel vary across the run at all? A flat channel (uniform, or clipped to 0/255)
    /// has nothing for a fit to explain, so its R² is 0 for a reason that is **not** evidence of
    /// different surfaces — and without this flag that 0 would outvote the channels that do carry
    /// evidence, turning a plain re-light into a false "different surfaces".
    pub determinate: [bool; 3],
}

impl Relight {
    /// The weakest **determinate** channel's R² — the honest summary, since "one surface, re-lit"
    /// has to hold in every channel that carries evidence. 0 when no channel does.
    pub fn worst_r2(&self) -> f64 {
        (0..3)
            .filter(|&c| self.determinate[c])
            .map(|c| self.r2[c])
            .reduce(f64::min)
            .unwrap_or(0.0)
    }
}

/// Least-squares fit of `to`'s pixels onto `from`'s, per channel, over `region`'s own pixels.
///
/// A channel with no variation across the run (a flat, saturated, or clipped channel) has nothing
/// for a fit to explain; its R² is reported as 0 rather than dividing by zero, which reads as
/// "this channel is not evidence" — the conservative direction.
pub fn relight(region: &Region, from: &RgbImage, to: &RgbImage) -> Relight {
    let mut out = Relight {
        gain: [0.0; 3],
        offset: [0.0; 3],
        r2: [0.0; 3],
        determinate: [false; 3],
    };
    let n = region.members.len() as f64;
    if n < 2.0 {
        return out;
    }
    for c in 0..3 {
        let (mut sx, mut sy) = (0.0f64, 0.0f64);
        for &(x, y) in &region.members {
            sx += f64::from(from.get_pixel(x, y)[c]);
            sy += f64::from(to.get_pixel(x, y)[c]);
        }
        let (mx, my) = (sx / n, sy / n);
        let (mut sxx, mut sxy, mut syy) = (0.0f64, 0.0f64, 0.0f64);
        for &(x, y) in &region.members {
            let dx = f64::from(from.get_pixel(x, y)[c]) - mx;
            let dy = f64::from(to.get_pixel(x, y)[c]) - my;
            sxx += dx * dx;
            sxy += dx * dy;
            syy += dy * dy;
        }
        // A flat source or a flat target leaves the fit undetermined; report it as no evidence.
        if sxx <= f64::EPSILON || syy <= f64::EPSILON {
            out.gain[c] = 0.0;
            out.offset[c] = my;
            out.r2[c] = 0.0;
            continue;
        }
        let a = sxy / sxx;
        out.gain[c] = a;
        out.offset[c] = my - a * mx;
        // r² for a simple linear fit is the squared Pearson correlation.
        out.r2[c] = (sxy * sxy) / (sxx * syy);
        out.determinate[c] = true;
    }
    out
}

/// The index of the frame-to-frame step where the run's mean luma moved most — the flip itself,
/// and the pair [`relight`] should be fitted across. `None` for a burst with fewer than two frames.
pub fn biggest_step(region: &Region, frames: &[RgbImage]) -> Option<usize> {
    extreme_step(region, frames, true)
}

/// The step where the run's mean luma moved **least** — the **control**, and the reason a low R² on
/// the biggest step can be believed at all.
///
/// The capture pans, so consecutive frames are never quite the same image, and a textured surface
/// sliding a fraction of a pixel decorrelates on its own. That means a low R² is only evidence of
/// *different surfaces* if a step where the run did **not** flip scores high across the same pixels
/// under the same motion. Quiet step near 1 and flip step near 0 is a real reading; both low means
/// the fit is measuring the pan and nothing else.
pub fn quietest_step(region: &Region, frames: &[RgbImage]) -> Option<usize> {
    extreme_step(region, frames, false)
}

fn extreme_step(region: &Region, frames: &[RgbImage], want_max: bool) -> Option<usize> {
    region
        .steps(frames)
        .iter()
        .enumerate()
        .map(|(i, s)| (i, s.mean_delta.abs()))
        .reduce(|a, b| {
            let take_b = if want_max { b.1 > a.1 } else { b.1 < a.1 };
            if take_b {
                b
            } else {
                a
            }
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;
    use image::Rgb;

    /// A run covering a whole small frame, so the fits are over every pixel.
    fn whole(w: u32, h: u32) -> Region {
        Region {
            bounds: Rect {
                x0: 0,
                y0: 0,
                x1: w - 1,
                y1: h - 1,
            },
            pixels: u64::from(w) * u64::from(h),
            members: (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).collect(),
        }
    }

    /// A textured surface: a pattern with real variation for a fit to have to explain.
    fn textured(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| Rgb(f(x, y)))
    }

    #[test]
    fn the_same_surface_plus_a_warm_light_fits_as_gain_one_and_an_offset() {
        let dim = textured(16, 16, |x, y| [(x * 8) as u8, (y * 6) as u8, (x + y) as u8]);
        // Exactly the B38 shape: add a warm constant, leave the pattern alone.
        let bright = textured(16, 16, |x, y| {
            [(x * 8 + 115) as u8, (y * 6 + 37) as u8, (x + y + 3) as u8]
        });
        let r = relight(&whole(16, 16), &dim, &bright);
        assert!(
            r.worst_r2() > 0.999,
            "an added light is an exact fit: {r:?}"
        );
        for c in 0..2 {
            assert!(
                (r.gain[c] - 1.0).abs() < 1e-6,
                "gain {c} should be 1: {r:?}"
            );
        }
        assert!((r.offset[0] - 115.0).abs() < 1e-6, "{r:?}");
        assert!((r.offset[1] - 37.0).abs() < 1e-6, "{r:?}");
    }

    #[test]
    fn the_same_surface_scaled_fits_as_a_gain_with_no_offset() {
        let dim = textured(16, 16, |x, y| [(x * 4) as u8, (y * 4) as u8, (x + y) as u8]);
        let bright = textured(16, 16, |x, y| {
            [(x * 8) as u8, (y * 8) as u8, ((x + y) * 2) as u8]
        });
        let r = relight(&whole(16, 16), &dim, &bright);
        assert!(r.worst_r2() > 0.999, "a scale is an exact fit: {r:?}");
        assert!((r.gain[0] - 2.0).abs() < 1e-6, "{r:?}");
        assert!(r.offset[0].abs() < 1e-6, "{r:?}");
    }

    /// The discrimination the module exists for: two surfaces whose *means* differ exactly as a
    /// re-light would, but whose patterns are unrelated. The means cannot tell them apart; R² can.
    #[test]
    fn two_different_surfaces_do_not_fit_however_well_their_means_line_up() {
        // Vertical stripes vs horizontal stripes: same mean, same spread, no affine relation.
        let plank = textured(16, 16, |x, _| [(x % 2) as u8 * 60 + 40, 40, 30]);
        let canvas = textured(16, 16, |_, y| [(y % 2) as u8 * 60 + 40, 40, 30]);
        let r = relight(&whole(16, 16), &plank, &canvas);
        assert!(r.r2[0] < 0.1, "unrelated patterns must not fit, got {r:?}");
    }

    #[test]
    fn a_flat_channel_is_reported_as_no_evidence_rather_than_a_divide_by_zero() {
        let a = textured(8, 8, |_, _| [50, 50, 50]);
        let b = textured(8, 8, |_, _| [90, 90, 90]);
        let r = relight(&whole(8, 8), &a, &b);
        assert_eq!(r.r2, [0.0; 3], "a flat run explains nothing: {r:?}");
        assert_eq!(r.determinate, [false; 3], "and says so: {r:?}");
        assert!((r.offset[0] - 90.0).abs() < 1e-9, "{r:?}");
    }

    /// The flat-channel rule has to be a *skip*, not a zero vote: a surface whose blue is clipped
    /// flat but whose red and green track perfectly is one surface being re-lit, and reporting the
    /// dead channel's 0 as the verdict would call it two.
    #[test]
    fn a_flat_channel_does_not_outvote_the_ones_carrying_evidence() {
        let dim = textured(16, 16, |x, y| [(x * 8) as u8, (y * 6) as u8, 255]);
        let bright = textured(16, 16, |x, y| [(x * 8 + 60) as u8, (y * 6 + 20) as u8, 255]);
        let r = relight(&whole(16, 16), &dim, &bright);
        assert_eq!(r.determinate, [true, true, false], "{r:?}");
        assert!(
            r.worst_r2() > 0.999,
            "the clipped blue must not decide it: {r:?}"
        );
    }

    #[test]
    fn the_biggest_step_is_the_flip_and_the_quietest_is_the_control() {
        let flat = textured(8, 8, |x, _| [(x * 10) as u8, 30, 30]);
        let nudged = textured(8, 8, |x, _| [(x * 10 + 2) as u8, 30, 30]);
        let jumped = textured(8, 8, |x, _| [(x * 10 + 90) as u8, 30, 30]);
        let frames = vec![flat.clone(), nudged, jumped];
        assert_eq!(biggest_step(&whole(8, 8), &frames), Some(1));
        assert_eq!(quietest_step(&whole(8, 8), &frames), Some(0));
    }
}
