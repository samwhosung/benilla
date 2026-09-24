//! Tells one surface lit differently from two different surfaces. Equal channel ratios rule out
//! only a scalar multiply, since an added coloured light changes the ratios too; the spatial
//! pattern tells them apart. Re-lighting is an affine map per pixel, `bright ≈ gain·dim + offset`,
//! so the fit is tight, while two surfaces fit loosely however well their means line up. This fits
//! one frame's pixels onto the other's per channel and reports R².
//!
//! The fit spans the run's largest single frame-to-frame step, never the whole burst: the camera
//! pans, and a pixel names the same bit of world only while the image holds still under it.

use image::RgbImage;

use crate::Region;

/// A per-channel affine fit of one frame's pixels onto another's, over a [`Region`]'s own pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Relight {
    /// Per channel, the `a` in `to ≈ a·from + b`: near 1 with an offset is a light added, well
    /// above 1 with none a light scaled.
    pub gain: [f64; 3],
    /// Per channel, the `b` in `to ≈ a·from + b`, in 0..255 units.
    pub offset: [f64; 3],
    /// Per channel R², 0..1: near 1 is one surface re-lit, low is two different surfaces.
    pub r2: [f64; 3],
    /// Whether the channel varied across the run: a flat or clipped channel's R² of 0 is no
    /// evidence, and must not outvote the others.
    pub determinate: [bool; 3],
}

impl Relight {
    /// The weakest determinate channel's R², or 0 when no channel is determinate.
    pub fn worst_r2(&self) -> f64 {
        (0..3)
            .filter(|&c| self.determinate[c])
            .map(|c| self.r2[c])
            .reduce(f64::min)
            .unwrap_or(0.0)
    }
}

/// Least-squares fit of `to`'s pixels onto `from`'s per channel, over `region`'s own pixels; a flat
/// channel reports R² 0 and is not determinate.
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

/// The step where the run's mean luma moved most: the flip, and the pair [`relight`] fits across.
pub fn biggest_step(region: &Region, frames: &[RgbImage]) -> Option<usize> {
    extreme_step(region, frames, true)
}

/// The step where the run's mean luma moved least: the control. The pan alone decorrelates a
/// textured surface, so a low R² on the flip means two surfaces only if this step fits tightly.
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

    /// A patterned surface, with variation for a fit to explain.
    fn textured(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| Rgb(f(x, y)))
    }

    #[test]
    fn the_same_surface_plus_a_warm_light_fits_as_gain_one_and_an_offset() {
        let dim = textured(16, 16, |x, y| [(x * 8) as u8, (y * 6) as u8, (x + y) as u8]);
        // A warm constant added, the pattern untouched.
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
