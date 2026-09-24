//! Image-diff metrics for the capture harness (`WOW_CAPTURE`, the `capture` module in
//! `benilla-app`): how far two equal-size images diverge ([`Metrics`]) and where ([`diff_image`]),
//! and over a burst of frames, which pixels would not hold still ([`envelope`], [`toggles`]).

pub mod relight;

use image::RgbImage;

/// A pixel counts toward [`Metrics::pct_over`] when a channel differs by more than this (8/255,
/// about 3%): above dithering noise, below a real visual shift.
pub const OVER_THRESHOLD: u8 = 8;

/// Per-image difference metrics: channel deltas in 0..255 byte units, fractions in 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Mean absolute per-channel difference (0..255).
    pub mae: f64,
    /// Root-mean-square per-channel difference (0..255), weighting large local deltas.
    pub rmse: f64,
    /// Largest single-channel difference anywhere (0..255).
    pub max_delta: u8,
    /// Fraction of pixels (0..1) whose largest channel delta exceeds [`OVER_THRESHOLD`].
    pub pct_over: f64,
    /// Pixels that differ at all: a handful is an MSAA tie flipping, a region a regression, though
    /// both show a large `max_delta` and an `mae` of 0.000.
    pub changed: u64,
    /// Where the largest delta is, `(0, 0)` when nothing differs; a tie flip recurs at the same
    /// spot build after build.
    pub worst_at: (u32, u32),
}

/// Compare two equally-sized RGB images. Errors if the dimensions differ.
pub fn compare(a: &RgbImage, b: &RgbImage) -> anyhow::Result<Metrics> {
    if a.dimensions() != b.dimensions() {
        anyhow::bail!(
            "image size mismatch: {:?} vs {:?}",
            a.dimensions(),
            b.dimensions()
        );
    }
    let mut sum_abs = 0u64;
    let mut sum_sq = 0u64;
    let mut max_delta = 0u8;
    let mut over = 0u64;
    let mut changed = 0u64;
    let mut worst_at = (0u32, 0u32);
    for (i, (pa, pb)) in a.pixels().zip(b.pixels()).enumerate() {
        let mut pixel_max = 0u8;
        for c in 0..3 {
            let d = (pa[c] as i32 - pb[c] as i32).unsigned_abs() as u8;
            sum_abs += d as u64;
            sum_sq += (d as u64) * (d as u64);
            if d > max_delta {
                max_delta = d;
                let i = i as u32;
                worst_at = (i % a.width(), i / a.width());
            }
            pixel_max = pixel_max.max(d);
        }
        if pixel_max > 0 {
            changed += 1;
        }
        if pixel_max > OVER_THRESHOLD {
            over += 1;
        }
    }
    let n_pixels = (a.width() as u64) * (a.height() as u64);
    let n_chan = (n_pixels * 3).max(1) as f64;
    Ok(Metrics {
        mae: sum_abs as f64 / n_chan,
        rmse: (sum_sq as f64 / n_chan).sqrt(),
        max_delta,
        pct_over: over as f64 / n_pixels.max(1) as f64,
        changed,
        worst_at,
    })
}

/// The per-channel difference `|a - b| * amplify`, clamped to 255, so a red shift shows red.
pub fn diff_image(a: &RgbImage, b: &RgbImage, amplify: u32) -> anyhow::Result<RgbImage> {
    if a.dimensions() != b.dimensions() {
        anyhow::bail!(
            "image size mismatch: {:?} vs {:?}",
            a.dimensions(),
            b.dimensions()
        );
    }
    let (w, h) = a.dimensions();
    let mut out = RgbImage::new(w, h);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let pa = a.get_pixel(x, y);
        let pb = b.get_pixel(x, y);
        for c in 0..3 {
            let d = (pa[c] as i32 - pb[c] as i32).unsigned_abs();
            p[c] = (d * amplify).min(255) as u8;
        }
    }
    Ok(out)
}

/// How far a burst of frames moved, and where: [`envelope`]'s output.
#[derive(Debug, Clone)]
pub struct Envelope {
    /// Per-channel `max - min` across the stack, amplified.
    pub image: RgbImage,
    /// Largest single-channel swing anywhere (0..255).
    pub max_swing: u8,
    /// Mean per-channel swing (0..255) over the whole frame.
    pub mean_swing: f64,
    /// Fraction of pixels (0..1) whose largest channel swing exceeds [`OVER_THRESHOLD`].
    pub pct_unstable: f64,
}

/// A burst's per-pixel envelope, `max - min` per channel across the whole stack: from a parked
/// camera only what refuses to settle lights up, however wrong a still pixel is. Errors on fewer
/// than two frames or a size mismatch.
pub fn envelope(frames: &[RgbImage], amplify: u32) -> anyhow::Result<Envelope> {
    let (w, h) = burst_dimensions(frames, 2)?;
    let mut image = RgbImage::new(w, h);
    let mut sum_swing = 0u64;
    let mut max_swing = 0u8;
    let mut unstable = 0u64;
    for (x, y, out) in image.enumerate_pixels_mut() {
        let mut pixel_max = 0u8;
        for c in 0..3 {
            let (mut lo, mut hi) = (u8::MAX, u8::MIN);
            for f in frames {
                let v = f.get_pixel(x, y)[c];
                lo = lo.min(v);
                hi = hi.max(v);
            }
            let swing = hi - lo;
            sum_swing += swing as u64;
            max_swing = max_swing.max(swing);
            pixel_max = pixel_max.max(swing);
            out[c] = (swing as u32 * amplify).min(255) as u8;
        }
        if pixel_max > OVER_THRESHOLD {
            unstable += 1;
        }
    }
    let n_pixels = (w as u64) * (h as u64);
    Ok(Envelope {
        image,
        max_swing,
        mean_swing: sum_swing as f64 / (n_pixels * 3).max(1) as f64,
        pct_unstable: unstable as f64 / n_pixels.max(1) as f64,
    })
}

/// How often a burst reversed direction, and where: [`toggles`]'s output.
#[derive(Debug, Clone)]
pub struct Toggles {
    /// Per-channel reversal count across the stack, amplified.
    pub image: RgbImage,
    /// Most reversals any one channel made (0..frames-2).
    pub max_reversals: u32,
    /// Fraction of pixels (0..1) that reversed at least [`TOGGLE_MIN_REVERSALS`] times.
    pub pct_toggling: f64,
    /// Row-major per-pixel toggle flags for [`shape`], exact where re-thresholding `image` is not.
    pub mask: Vec<bool>,
}

/// Direction reversals that make a pixel toggle: two cannot be a single overshoot, and smooth
/// motion across an edge never makes the up-down-up run they need.
pub const TOGGLE_MIN_REVERSALS: u32 = 2;

/// Count, per pixel and channel, the direction reversals across the burst, ignoring steps under
/// `min_delta`: the flicker reading for a moving camera. Smooth motion is monotone; z-fighting and
/// an unstable draw order alternate however the camera moves.
pub fn toggles(frames: &[RgbImage], min_delta: u8, amplify: u32) -> anyhow::Result<Toggles> {
    let (w, h) = burst_dimensions(frames, 3)?;
    let mut image = RgbImage::new(w, h);
    let mut max_reversals = 0u32;
    let mut toggling = 0u64;
    let mut mask = vec![false; (w as usize) * (h as usize)];
    for (x, y, out) in image.enumerate_pixels_mut() {
        let mut pixel_max = 0u32;
        for c in 0..3 {
            // `dir` is the last counted step's sign; smaller steps neither reverse nor reset it.
            let (mut dir, mut reversals) = (0i8, 0u32);
            for pair in frames.windows(2) {
                let (a, b) = (
                    pair[0].get_pixel(x, y)[c] as i32,
                    pair[1].get_pixel(x, y)[c] as i32,
                );
                let step = b - a;
                if step.unsigned_abs() < min_delta as u32 {
                    continue;
                }
                let sign = if step > 0 { 1i8 } else { -1i8 };
                if dir != 0 && sign != dir {
                    reversals += 1;
                }
                dir = sign;
            }
            max_reversals = max_reversals.max(reversals);
            pixel_max = pixel_max.max(reversals);
            out[c] = (reversals * amplify).min(255) as u8;
        }
        if pixel_max >= TOGGLE_MIN_REVERSALS {
            toggling += 1;
            mask[(y as usize) * (w as usize) + x as usize] = true;
        }
    }
    let n_pixels = (w as u64) * (h as u64);
    Ok(Toggles {
        image,
        max_reversals,
        pct_toggling: toggling as f64 / n_pixels.max(1) as f64,
        mask,
    })
}

/// A rectangle of the frame in pixels, `x1` and `y1` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Rect {
    pub fn width(&self) -> u32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> u32 {
        self.y1 - self.y0
    }
    /// Grow by `pad` on every side, clamped to a `w × h` frame.
    pub fn padded(&self, pad: u32, w: u32, h: u32) -> Rect {
        Rect {
            x0: self.x0.saturating_sub(pad),
            y0: self.y0.saturating_sub(pad),
            x1: (self.x1 + pad).min(w),
            y1: (self.y1 + pad).min(h),
        }
    }
}

/// One connected run of toggling pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub bounds: Rect,
    /// Toggling pixels in the run (≤ the bounds' area).
    pub pixels: u64,
    /// Every pixel in the run, in scan order; [`Region::steps`] reads these, not the bounding box.
    pub members: Vec<(u32, u32)>,
}

/// How many representative pixels [`Region::samples`] hands back.
pub const REGION_SAMPLES: usize = 8;

/// One frame-to-frame step of a run, from [`Region::steps`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    /// The run's mean luma (0..255) in the earlier frame.
    pub mean_from: f64,
    /// The run's mean R, G and B (0..255) in the earlier frame.
    pub mean_rgb_from: [f64; 3],
    /// Change in the run's mean luma (0..255 units) from this frame to the next.
    pub mean_delta: f64,
    /// Fraction of the run's pixels (0..1) that moved the same way as the mean: near 1 the surface
    /// changed together, near 0.5 as many went up as down.
    pub agreement: f64,
}

impl Region {
    /// How much of its bounding box the run fills, 0..1: a solid surface nears 1, speckle 0.
    pub fn fill(&self) -> f64 {
        let area = u64::from(self.bounds.width()) * u64::from(self.bounds.height());
        self.pixels as f64 / area.max(1) as f64
    }

    /// Up to [`REGION_SAMPLES`] pixels spread through the run, for the ray pick (`WOW_PICK`).
    pub fn samples(&self) -> Vec<(u32, u32)> {
        let step = (self.members.len() / REGION_SAMPLES).max(1);
        self.members
            .iter()
            .copied()
            .step_by(step)
            .take(REGION_SAMPLES)
            .collect()
    }

    /// The run's frame-to-frame series: the whole run brightening and dimming together (`agreement`
    /// near 1) is a surface re-shaded; half up and half down (`agreement` near 0.5, `mean_delta`
    /// near 0) is an edge sweeping across it.
    pub fn steps(&self, frames: &[RgbImage]) -> Vec<Step> {
        frames
            .windows(2)
            .map(|pair| {
                let mut from = 0.0;
                let mut rgb = [0.0f64; 3];
                let deltas: Vec<f64> = self
                    .members
                    .iter()
                    .map(|&(x, y)| {
                        let p = pair[0].get_pixel(x, y);
                        for c in 0..3 {
                            rgb[c] += f64::from(p[c]);
                        }
                        let a = luma(p);
                        from += a;
                        luma(pair[1].get_pixel(x, y)) - a
                    })
                    .collect();
                let n = deltas.len().max(1) as f64;
                let mean_delta = deltas.iter().sum::<f64>() / n;
                // A pixel that did not move counts against the surface moving together.
                let same = deltas
                    .iter()
                    .filter(|d| **d != 0.0 && d.is_sign_positive() == mean_delta.is_sign_positive())
                    .count();
                Step {
                    mean_from: from / n,
                    mean_rgb_from: [rgb[0] / n, rgb[1] / n, rgb[2] / n],
                    mean_delta,
                    agreement: same as f64 / n,
                }
            })
            .collect()
    }
}

/// Rec. 601 luma.
fn luma(p: &image::Rgb<u8>) -> f64 {
    0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])
}

/// The spatial structure of a toggle map. Z-fighting resolves per fragment, landing as many small
/// runs of low coherence; a whole surface blinking (a culling, portal-visibility or draw-submission
/// flip) lands as one large solid run. Both score the same on [`Toggles::pct_toggling`].
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// Connected runs of at least [`MIN_REGION_PIXELS`], largest first.
    pub regions: Vec<Region>,
    /// Toggling pixels that fell in runs too small to list.
    pub scattered: u64,
    /// Mean fraction of a toggling pixel's 4-neighbours that also toggle, 0..1: solid areas near 1,
    /// isolated pixels near 0, a one-pixel moiré band near 0.5.
    pub coherence: f64,
}

/// A run smaller than this is noise, counted in [`Shape::scattered`].
pub const MIN_REGION_PIXELS: u64 = 16;

/// Group a toggle mask into connected runs (4-connectivity) and measure their coherence.
pub fn shape(t: &Toggles) -> Shape {
    let (w, h) = t.image.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let idx = |x: usize, y: usize| y * wu + x;

    let mut coherent = 0u64;
    let mut toggling = 0u64;
    for y in 0..hu {
        for x in 0..wu {
            if !t.mask[idx(x, y)] {
                continue;
            }
            toggling += 1;
            // Off-frame neighbours count as non-toggling, so a run on the border is not inflated.
            let n = u64::from(x > 0 && t.mask[idx(x - 1, y)])
                + u64::from(x + 1 < wu && t.mask[idx(x + 1, y)])
                + u64::from(y > 0 && t.mask[idx(x, y - 1)])
                + u64::from(y + 1 < hu && t.mask[idx(x, y + 1)]);
            coherent += n;
        }
    }

    // Iterative: a recursive flood fill overflows the stack on a full-screen region.
    let mut seen = vec![false; wu * hu];
    let mut regions: Vec<Region> = Vec::new();
    let mut scattered = 0u64;
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for y0 in 0..hu {
        for x0 in 0..wu {
            if !t.mask[idx(x0, y0)] || seen[idx(x0, y0)] {
                continue;
            }
            seen[idx(x0, y0)] = true;
            stack.push((x0, y0));
            let (mut pixels, mut b) = (
                0u64,
                Rect {
                    x0: x0 as u32,
                    y0: y0 as u32,
                    x1: x0 as u32 + 1,
                    y1: y0 as u32 + 1,
                },
            );
            let mut members: Vec<(u32, u32)> = Vec::new();
            while let Some((x, y)) = stack.pop() {
                pixels += 1;
                members.push((x as u32, y as u32));
                b.x0 = b.x0.min(x as u32);
                b.y0 = b.y0.min(y as u32);
                b.x1 = b.x1.max(x as u32 + 1);
                b.y1 = b.y1.max(y as u32 + 1);
                let mut push = |nx: usize, ny: usize, stack: &mut Vec<(usize, usize)>| {
                    if t.mask[idx(nx, ny)] && !seen[idx(nx, ny)] {
                        seen[idx(nx, ny)] = true;
                        stack.push((nx, ny));
                    }
                };
                if x > 0 {
                    push(x - 1, y, &mut stack);
                }
                if x + 1 < wu {
                    push(x + 1, y, &mut stack);
                }
                if y > 0 {
                    push(x, y - 1, &mut stack);
                }
                if y + 1 < hu {
                    push(x, y + 1, &mut stack);
                }
            }
            if pixels >= MIN_REGION_PIXELS {
                // Scan order, so `Region::samples` spreads through the run, not the traversal.
                members.sort_unstable_by_key(|&(x, y)| (y, x));
                regions.push(Region {
                    bounds: b,
                    pixels,
                    members,
                });
            } else {
                scattered += pixels;
            }
        }
    }
    regions.sort_by_key(|r| std::cmp::Reverse(r.pixels));
    Shape {
        regions,
        scattered,
        coherence: coherent as f64 / (toggling.max(1) * 4) as f64,
    }
}

/// Cut `rect` out of `img`, clamped to its bounds.
pub fn crop(img: &RgbImage, rect: Rect) -> RgbImage {
    let r = rect.padded(0, img.width(), img.height());
    let (w, h) = (r.width().max(1), r.height().max(1));
    RgbImage::from_fn(w, h, |x, y| {
        *img.get_pixel(
            (r.x0 + x).min(img.width() - 1),
            (r.y0 + y).min(img.height() - 1),
        )
    })
}

/// Magnify by an integer factor, nearest-neighbour, so every output pixel is a source pixel: a
/// resampler's blended edge reads as a render defect. It never shrinks.
pub fn zoom(img: &RgbImage, factor: u32) -> RgbImage {
    let s = factor.max(1);
    RgbImage::from_fn(img.width() * s, img.height() * s, |x, y| {
        *img.get_pixel(x / s, y / s)
    })
}

/// Lay tiles out in a `cols`-wide grid on a dark grey mat, one contact sheet; each keeps its size.
pub fn contact_strip(tiles: &[RgbImage], cols: u32, gap: u32) -> RgbImage {
    let cols = cols.max(1);
    let (tw, th) = tiles
        .iter()
        .fold((1, 1), |(w, h), t| (w.max(t.width()), h.max(t.height())));
    let rows = (tiles.len() as u32).div_ceil(cols);
    let out_w = cols * tw + (cols + 1) * gap;
    let out_h = rows * th + (rows + 1) * gap;
    let mut out = RgbImage::from_pixel(out_w, out_h, image::Rgb([32, 32, 32]));
    for (i, tile) in tiles.iter().enumerate() {
        let (cx, cy) = (i as u32 % cols, i as u32 / cols);
        let (ox, oy) = (gap + cx * (tw + gap), gap + cy * (th + gap));
        for (x, y, p) in tile.enumerate_pixels() {
            if ox + x < out_w && oy + y < out_h {
                out.put_pixel(ox + x, oy + y, *p);
            }
        }
    }
    out
}

fn burst_dimensions(frames: &[RgbImage], min: usize) -> anyhow::Result<(u32, u32)> {
    if frames.len() < min {
        anyhow::bail!("needs at least {min} frames, got {}", frames.len());
    }
    let (w, h) = frames[0].dimensions();
    for (i, f) in frames.iter().enumerate() {
        if f.dimensions() != (w, h) {
            anyhow::bail!(
                "frame {i} is {:?}, expected {:?} — a burst must be one camera, one window size",
                f.dimensions(),
                (w, h)
            );
        }
    }
    Ok((w, h))
}

/// Stitch two images side by side with a `gap`-px dark separator, top-aligned at the taller height.
pub fn compose_side_by_side(left: &RgbImage, right: &RgbImage, gap: u32) -> RgbImage {
    let h = left.height().max(right.height());
    let w = left.width() + gap + right.width();
    let mut out = image::RgbImage::from_pixel(w, h, image::Rgb([24, 24, 24]));
    image::imageops::overlay(&mut out, left, 0, 0);
    image::imageops::overlay(&mut out, right, (left.width() + gap) as i64, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb(rgb))
    }

    #[test]
    fn identical_images_are_zero() {
        let img = solid(4, 4, [120, 60, 200]);
        let m = compare(&img, &img).unwrap();
        assert_eq!(m.max_delta, 0);
        assert_eq!(m.mae, 0.0);
        assert_eq!(m.rmse, 0.0);
        assert_eq!(m.pct_over, 0.0);
        assert_eq!(m.changed, 0);
        assert_eq!(m.worst_at, (0, 0));
    }

    #[test]
    fn one_flipped_pixel_is_counted_and_located() {
        let a = solid(100, 80, [40, 40, 40]);
        let mut b = a.clone();
        b.put_pixel(37, 22, image::Rgb([40, 61, 40]));
        let m = compare(&a, &b).unwrap();
        assert_eq!(m.changed, 1, "exactly one pixel differs");
        assert_eq!(m.worst_at, (37, 22), "and the report says where");
        assert_eq!(m.max_delta, 21);
        assert!(m.mae < 0.001, "a lone pixel vanishes into the mean");
    }

    #[test]
    fn constant_offset_matches_offset() {
        // `b` is 10 below `a`: mae == rmse == max_delta == 10, and 10 > 8 puts every pixel over.
        let a = solid(8, 5, [100, 100, 100]);
        let b = solid(8, 5, [90, 90, 90]);
        let m = compare(&a, &b).unwrap();
        assert_eq!(m.max_delta, 10);
        assert!((m.mae - 10.0).abs() < 1e-9);
        assert!((m.rmse - 10.0).abs() < 1e-9);
        assert!((m.pct_over - 1.0).abs() < 1e-9);
    }

    #[test]
    fn small_offset_is_under_threshold() {
        let a = solid(8, 5, [100, 100, 100]);
        let b = solid(8, 5, [95, 95, 95]);
        let m = compare(&a, &b).unwrap();
        assert_eq!(m.max_delta, 5);
        assert_eq!(m.pct_over, 0.0);
    }

    #[test]
    fn single_channel_delta() {
        // Only the red channel differs by 30 in one of four pixels.
        let a = solid(2, 2, [10, 10, 10]);
        let mut b = a.clone();
        b.put_pixel(0, 0, Rgb([40, 10, 10]));
        let m = compare(&a, &b).unwrap();
        assert_eq!(m.max_delta, 30);
        // one changed pixel of four
        assert!((m.pct_over - 0.25).abs() < 1e-9);
        // 30 over 12 channels = 2.5
        assert!((m.mae - 2.5).abs() < 1e-9);
    }

    #[test]
    fn zoom_is_pixel_verbatim() {
        let mut img = solid(2, 1, [10, 20, 30]);
        img.put_pixel(1, 0, Rgb([200, 100, 50]));
        let z = zoom(&img, 3);
        assert_eq!(z.dimensions(), (6, 3));
        for (x, _, p) in z.enumerate_pixels() {
            let want = if x < 3 { [10, 20, 30] } else { [200, 100, 50] };
            assert_eq!(p.0, want);
        }
        // Factor 0 clamps to 1 (identity), never a panic or an empty image.
        assert_eq!(zoom(&img, 0).dimensions(), img.dimensions());
    }

    #[test]
    fn size_mismatch_errors() {
        let a = solid(4, 4, [0, 0, 0]);
        let b = solid(4, 5, [0, 0, 0]);
        assert!(compare(&a, &b).is_err());
    }

    #[test]
    fn diff_image_amplifies_and_clamps() {
        let a = solid(2, 1, [10, 10, 10]);
        let mut b = a.clone();
        b.put_pixel(0, 0, Rgb([20, 10, 10])); // red delta 10
        let d = diff_image(&a, &b, 8).unwrap();
        // 10*8 = 80 in red at (0,0); other channels/pixels zero.
        assert_eq!(d.get_pixel(0, 0), &Rgb([80, 0, 0]));
        assert_eq!(d.get_pixel(1, 0), &Rgb([0, 0, 0]));
        // amplify saturates: 10*32 = 320 -> 255.
        let d2 = diff_image(&a, &b, 32).unwrap();
        assert_eq!(d2.get_pixel(0, 0), &Rgb([255, 0, 0]));
    }

    #[test]
    fn a_still_burst_has_no_envelope() {
        let img = solid(4, 4, [200, 30, 90]);
        let e = envelope(&[img.clone(), img.clone(), img], 8).unwrap();
        assert_eq!(e.max_swing, 0);
        assert_eq!(e.mean_swing, 0.0);
        assert_eq!(e.pct_unstable, 0.0);
        assert_eq!(e.image.get_pixel(0, 0), &Rgb([0, 0, 0]));
    }

    #[test]
    fn envelope_spans_the_whole_stack_not_just_neighbours() {
        // 10, 60, 10 swings 50 per pair; 10, 35, 60 only 25 per pair, yet both envelopes are 50.
        let base = solid(2, 1, [10, 10, 10]);
        let alternating = {
            let mut m = base.clone();
            m.put_pixel(0, 0, Rgb([60, 10, 10]));
            m
        };
        let drifting = {
            let mut m = base.clone();
            m.put_pixel(0, 0, Rgb([35, 10, 10]));
            m
        };
        for stack in [
            [base.clone(), alternating.clone(), base.clone()],
            [base.clone(), drifting, alternating],
        ] {
            let e = envelope(&stack, 1).unwrap();
            assert_eq!(e.max_swing, 50);
            assert_eq!(e.image.get_pixel(0, 0), &Rgb([50, 0, 0]));
            assert_eq!(e.image.get_pixel(1, 0), &Rgb([0, 0, 0]));
            // one unstable pixel of two
            assert!((e.pct_unstable - 0.5).abs() < 1e-9);
        }
    }

    #[test]
    fn envelope_amplifies_and_clamps_like_the_diff() {
        let a = solid(1, 1, [10, 10, 10]);
        let b = solid(1, 1, [20, 10, 10]);
        assert_eq!(
            envelope(&[a.clone(), b.clone()], 8)
                .unwrap()
                .image
                .get_pixel(0, 0),
            &Rgb([80, 0, 0])
        );
        assert_eq!(
            envelope(&[a, b], 32).unwrap().image.get_pixel(0, 0),
            &Rgb([255, 0, 0])
        );
    }

    #[test]
    fn envelope_needs_a_burst_and_one_window_size() {
        let a = solid(4, 4, [0, 0, 0]);
        assert!(envelope(&[], 8).is_err());
        assert!(envelope(std::slice::from_ref(&a), 8).is_err());
        assert!(envelope(&[a, solid(4, 5, [0, 0, 0])], 8).is_err());
    }

    /// One red value per frame, at pixel (0,0) of a 1×1 burst.
    fn ramp(reds: &[u8]) -> Vec<RgbImage> {
        reds.iter()
            .map(|r| RgbImage::from_pixel(1, 1, Rgb([*r, 0, 0])))
            .collect()
    }

    #[test]
    fn a_monotone_sweep_never_toggles_however_far_it_moves() {
        let sweep = ramp(&[0, 60, 120, 180, 240]);
        assert_eq!(envelope(&sweep, 1).unwrap().max_swing, 240);
        let t = toggles(&sweep, 4, 60).unwrap();
        assert_eq!(t.max_reversals, 0);
        assert_eq!(t.pct_toggling, 0.0);
    }

    #[test]
    fn an_alternating_pixel_toggles_every_step() {
        // A/B/A/B/A, z-fighting's signature: four steps, three of them reversals.
        let t = toggles(&ramp(&[10, 90, 10, 90, 10]), 4, 60).unwrap();
        assert_eq!(t.max_reversals, 3);
        assert_eq!(t.pct_toggling, 1.0);
        assert_eq!(t.image.get_pixel(0, 0), &Rgb([180, 0, 0])); // 3 × 60, unclamped
    }

    #[test]
    fn sub_threshold_noise_neither_toggles_nor_forgets_the_direction() {
        // ±1 dither makes no reversals,
        let t = toggles(&ramp(&[50, 51, 50, 51, 50, 51]), 4, 60).unwrap();
        assert_eq!(t.max_reversals, 0);
        // nor does it reset the direction in the middle of a real climb.
        let t = toggles(&ramp(&[0, 40, 41, 40, 41, 80]), 4, 60).unwrap();
        assert_eq!(t.max_reversals, 0);
    }

    #[test]
    fn a_single_overshoot_is_one_reversal_and_below_the_toggle_bar() {
        let t = toggles(&ramp(&[10, 90, 40]), 4, 60).unwrap();
        assert_eq!(t.max_reversals, 1);
        assert_eq!(t.pct_toggling, 0.0);
    }

    #[test]
    fn toggles_needs_three_frames_and_one_window_size() {
        let a = solid(4, 4, [0, 0, 0]);
        assert!(toggles(&[a.clone(), a.clone()], 4, 60).is_err());
        assert!(toggles(&[a.clone(), a.clone(), solid(4, 5, [0, 0, 0])], 4, 60).is_err());
        assert!(toggles(&[a.clone(), a.clone(), a], 4, 60).is_ok());
    }

    #[test]
    fn compose_places_both_with_gap() {
        let l = solid(3, 2, [10, 20, 30]);
        let r = solid(4, 2, [40, 50, 60]);
        let out = compose_side_by_side(&l, &r, 1);
        assert_eq!(out.dimensions(), (3 + 1 + 4, 2));
        assert_eq!(out.get_pixel(0, 0), &Rgb([10, 20, 30])); // left image
        assert_eq!(out.get_pixel(3, 0), &Rgb([24, 24, 24])); // separator
        assert_eq!(out.get_pixel(4, 0), &Rgb([40, 50, 60])); // right image
    }

    /// A burst where the pixels `flips` selects alternate 0/200 and the rest hold still.
    fn alternating(
        w: u32,
        h: u32,
        frames: usize,
        flips: impl Fn(u32, u32) -> bool,
    ) -> Vec<RgbImage> {
        (0..frames)
            .map(|i| {
                RgbImage::from_fn(w, h, |x, y| {
                    let on = flips(x, y) && i % 2 == 1;
                    Rgb([if on { 200 } else { 0 }, 0, 0])
                })
            })
            .collect()
    }

    #[test]
    fn one_solid_block_reads_as_a_single_filled_run() {
        let frames = alternating(20, 20, 6, |x, y| {
            (4..12).contains(&x) && (4..12).contains(&y)
        });
        let s = shape(&toggles(&frames, 4, 60).unwrap());
        assert_eq!(s.regions.len(), 1);
        assert_eq!(s.regions[0].pixels, 64);
        assert_eq!(
            s.regions[0].bounds,
            Rect {
                x0: 4,
                y0: 4,
                x1: 12,
                y1: 12
            }
        );
        assert!((s.regions[0].fill() - 1.0).abs() < 1e-9);
        assert_eq!(s.scattered, 0);
        // 8x8 block: 4 corners see 2 neighbours, 24 edges see 3, 36 interior see 4 → 224/256.
        assert!(
            (s.coherence - 224.0 / 256.0).abs() < 1e-9,
            "{}",
            s.coherence
        );
    }

    #[test]
    fn a_checkerboard_covers_the_same_area_with_none_of_the_shape() {
        let frames = alternating(20, 20, 6, |x, y| {
            (4..12).contains(&x) && (4..12).contains(&y) && (x + y) % 2 == 0
        });
        let s = shape(&toggles(&frames, 4, 60).unwrap());
        assert_eq!(s.coherence, 0.0);
        assert!(s.regions.is_empty(), "isolated pixels are not runs");
        assert_eq!(s.scattered, 32);
    }

    #[test]
    fn runs_come_back_largest_first_and_short_ones_are_scattered() {
        let frames = alternating(40, 20, 6, |x, y| {
            let big = (2..12).contains(&x) && (2..12).contains(&y); // 100 px
            let small = (20..25).contains(&x) && (2..7).contains(&y); // 25 px
            let speck = (30..33).contains(&x) && y == 10; // 3 px, noise
            big || small || speck
        });
        let s = shape(&toggles(&frames, 4, 60).unwrap());
        assert_eq!(
            s.regions.iter().map(|r| r.pixels).collect::<Vec<_>>(),
            vec![100, 25]
        );
        assert_eq!(s.scattered, 3);
    }

    #[test]
    fn a_still_burst_has_no_shape_at_all() {
        let frames = alternating(8, 8, 6, |_, _| false);
        let s = shape(&toggles(&frames, 4, 60).unwrap());
        assert!(s.regions.is_empty());
        assert_eq!(s.scattered, 0);
        assert_eq!(s.coherence, 0.0); // no toggling pixels, and no divide-by-zero
    }

    #[test]
    fn crop_takes_the_named_rect() {
        let img = RgbImage::from_fn(10, 10, |x, y| Rgb([x as u8, y as u8, 0]));
        let out = crop(
            &img,
            Rect {
                x0: 3,
                y0: 4,
                x1: 6,
                y1: 9,
            },
        );
        assert_eq!(out.dimensions(), (3, 5));
        assert_eq!(out.get_pixel(0, 0), &Rgb([3, 4, 0]));
        assert_eq!(out.get_pixel(2, 4), &Rgb([5, 8, 0]));
    }

    #[test]
    fn padding_a_rect_stops_at_the_frame_edge() {
        let r = Rect {
            x0: 2,
            y0: 2,
            x1: 5,
            y1: 5,
        };
        assert_eq!(
            r.padded(10, 8, 8),
            Rect {
                x0: 0,
                y0: 0,
                x1: 8,
                y1: 8
            }
        );
    }

    #[test]
    fn a_contact_strip_lays_tiles_out_in_a_grid() {
        let tiles: Vec<RgbImage> = (0..3)
            .map(|i| RgbImage::from_pixel(2, 2, Rgb([10 * (i + 1) as u8, 0, 0])))
            .collect();
        let out = contact_strip(&tiles, 2, 1);
        assert_eq!(out.dimensions(), (2 * 2 + 3, 2 * 2 + 3)); // 2 cols, 2 rows, 1px mat
        assert_eq!(out.get_pixel(1, 1), &Rgb([10, 0, 0])); // tile 0
        assert_eq!(out.get_pixel(4, 1), &Rgb([20, 0, 0])); // tile 1, next column
        assert_eq!(out.get_pixel(1, 4), &Rgb([30, 0, 0])); // tile 2, wrapped
        assert_eq!(out.get_pixel(4, 4), &Rgb([32, 32, 32])); // empty cell stays mat
    }
}
