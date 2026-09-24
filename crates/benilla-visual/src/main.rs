//! `benilla-visual` — diff captures from the Phase-5 visual A/B harness.
//!
//! Usage:
//!   benilla-visual diff     <a.png> <b.png>   [--out <diff.png>] [--fail <mae>] [--amplify <n>]
//!   benilla-visual diff-dir <dir_a> <dir_b>   [--out <diff_dir>] [--fail <mae>] [--amplify <n>]
//!   benilla-visual flicker  <burst_dir>       [--out <envelope.png>] [--fail <mae>] [--amplify <n>]
//!   benilla-visual stat     <img.png>         [--rect <x>,<y>,<w>,<h>]
//!
//! `diff` compares two images; `diff-dir` compares every `*.png` present in *both* directories by name.
//! Prints the metrics; writes amplified heatmap(s) when `--out` is given; exits non-zero if any image's
//! MAE exceeds `--fail` (when given). Typical loop: capture baselines, change the renderer, re-capture,
//! `diff-dir baseline candidate --out diff --fail 1.5`.
//!
//! `flicker` is the same arithmetic over *time*: point it at a `WOW_LIVE_SHOT_COUNT` burst (adjacent
//! frames, parked camera) and it prints the frame-to-frame table and the whole-stack envelope, and
//! writes the envelope heatmap — the picture of which pixels would not hold still.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use benilla_visual::{
    compare, compose_side_by_side, contact_strip, crop, diff_image, envelope, shape, toggles, zoom,
    Metrics, Rect, OVER_THRESHOLD,
};

/// A frame-to-frame step must move a channel by this much to count as a direction change (override
/// with `--toggle-delta`). Above PNG/dither noise, below any real surface swap.
///
/// **The pan rate matters more than this number.** A moving camera sweeps texture detail across
/// every pixel, and detail is not monotone — at 8°/s (≈15 px/frame at 3200 px) the map saturates at
/// 93% and says nothing. Keep the sweep **sub-pixel per frame** (≈0.2°/s) so ordinary surfaces
/// barely move while a depth-comparison flip still swaps whole surfaces at full amplitude.
const TOGGLE_MIN_DELTA: u8 = 4;

/// Reversal counts are small integers, so the toggle map needs its own (much larger) gain than the
/// 0..255 swings the envelope amplifies: at 60, three reversals saturate.
const TOGGLE_AMPLIFY: u32 = 60;

/// Gap (px) between the two halves of a side-by-side compose.
const COMPOSE_GAP: u32 = 6;

/// Tiles per row on a `hotspot` contact sheet — wide enough that a 24-frame burst reads as a few
/// rows of adjacent frames rather than one unscannable ribbon.
const STRIP_COLS: u32 = 6;

/// Pixels of context kept around a hotspot crop (override with `--pad`). Enough to see what the
/// toggling region borders — which is usually the whole point.
const DEFAULT_PAD: u32 = 24;

/// How many runs `hotspot` reports a time series for. Enough to see whether the top runs flip on the
/// same frames (one cause) or independently (several), narrow enough to stay one screen.
const SERIES_RUNS: usize = 4;

/// Default amplification for the heatmap output (per-channel abs-diff ×N, clamped).
const DEFAULT_AMPLIFY: u32 = 8;

struct Opts {
    out: Option<PathBuf>,
    fail: Option<f64>,
    amplify: u32,
    toggle_delta: u8,
    pad: u32,
    at: Option<String>,
    rect: Option<String>,
    scale: u32,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut positional: Vec<String> = Vec::new();
    let mut opts = Opts {
        out: None,
        fail: None,
        amplify: DEFAULT_AMPLIFY,
        toggle_delta: TOGGLE_MIN_DELTA,
        pad: DEFAULT_PAD,
        at: None,
        rect: None,
        scale: 1,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => opts.out = Some(PathBuf::from(next(&mut it, "--out")?)),
            "--fail" => {
                opts.fail = Some(
                    next(&mut it, "--fail")?
                        .parse()
                        .context("--fail not a number")?,
                )
            }
            "--toggle-delta" => {
                opts.toggle_delta = next(&mut it, "--toggle-delta")?
                    .parse()
                    .context("--toggle-delta not a 0..255 integer")?
            }
            "--amplify" => {
                opts.amplify = next(&mut it, "--amplify")?
                    .parse()
                    .context("--amplify not an integer")?
            }
            "--pad" => {
                opts.pad = next(&mut it, "--pad")?
                    .parse()
                    .context("--pad not an integer")?
            }
            "--at" => opts.at = Some(next(&mut it, "--at")?),
            "--rect" => opts.rect = Some(next(&mut it, "--rect")?),
            "--scale" => {
                opts.scale = next(&mut it, "--scale")?
                    .parse()
                    .context("--scale not an integer")?
            }
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            _ => positional.push(a.clone()),
        }
    }

    let Some((cmd, rest)) = positional.split_first() else {
        print_usage();
        bail!("no subcommand given");
    };

    match cmd.as_str() {
        "diff" => {
            let [a, b] = two(rest, "diff")?;
            let m = diff_one(
                Path::new(a),
                Path::new(b),
                opts.out.as_deref(),
                opts.amplify,
            )?;
            println!("{:<28} {}", format!("{a} vs {b}"), fmt_metrics(&m));
            if over_fail(&m, opts.fail) {
                bail!("MAE {:.3} exceeds --fail {:.3}", m.mae, opts.fail.unwrap());
            }
        }
        "diff-dir" => {
            let [da, db] = two(rest, "diff-dir")?;
            let DirDiff { worst, unpaired } = diff_dir(
                Path::new(da),
                Path::new(db),
                opts.out.as_deref(),
                opts.amplify,
            )?;
            // Under `--fail` this is a gate, and a shot that never landed must sink it — see the
            // note in `diff_dir`.
            if opts.fail.is_some() && !unpaired.is_empty() {
                bail!(
                    "{} unpaired image(s) — a shot is missing from one side, so this comparison is \
                     incomplete: {}",
                    unpaired.len(),
                    unpaired.join(", ")
                );
            }
            if let Some((name, m)) = worst {
                if over_fail(&m, opts.fail) {
                    bail!(
                        "worst image {name:?} MAE {:.3} exceeds --fail {:.3}",
                        m.mae,
                        opts.fail.unwrap()
                    );
                }
            }
        }
        "flicker" => {
            let [dir] = one(rest, "flicker")?;
            let worst = flicker(
                Path::new(dir),
                opts.out.as_deref(),
                opts.amplify,
                opts.toggle_delta,
            )?;
            if over_fail(&worst, opts.fail) {
                bail!(
                    "frame-to-frame MAE {:.3} exceeds --fail {:.3}",
                    worst.mae,
                    opts.fail.unwrap()
                );
            }
        }
        "hotspot" => {
            let [dir] = one(rest, "hotspot")?;
            let out = opts
                .out
                .as_deref()
                .context("hotspot needs --out <strip.png>")?;
            hotspot(Path::new(dir), out, opts.toggle_delta, opts.pad)?;
        }
        "series" => {
            let [dir] = one(rest, "series")?;
            let at = opts
                .at
                .as_deref()
                .context("series needs --at \"<x>,<y>[;<x>,<y>…]\"")?;
            series(Path::new(dir), at)?;
        }
        "flow" => {
            let [dir] = one(rest, "flow")?;
            flow(Path::new(dir), opts.rect.as_deref())?;
        }
        "edge" => {
            let [dir] = one(rest, "edge")?;
            let at = opts
                .at
                .as_deref()
                .context("edge needs --at \"<x0>,<y>,<x1>[;…]\"")?;
            edge(Path::new(dir), at)?;
        }
        "compose-dir" => {
            let [da, db] = two(rest, "compose-dir")?;
            let out = opts
                .out
                .as_deref()
                .context("compose-dir needs --out <dir>")?;
            compose_dir(Path::new(da), Path::new(db), out)?;
        }
        "stat" => {
            let [img] = one(rest, "stat")?;
            stat_cmd(Path::new(img), opts.rect.as_deref())?;
        }
        "crop" => {
            let [img] = one(rest, "crop")?;
            let rect = opts
                .rect
                .as_deref()
                .context("crop needs --rect <x>,<y>,<w>,<h> (source pixels)")?;
            let out = opts.out.as_deref().context("crop needs --out <crop.png>")?;
            crop_cmd(Path::new(img), rect, opts.at.as_deref(), opts.scale, out)?;
        }
        other => {
            print_usage();
            bail!("unknown subcommand {other:?}");
        }
    }
    Ok(())
}

/// What one named pixel *did*, frame by frame — the colour counterpart to the client's `WOW_DEPTH`.
///
/// The aggregate readings (`flicker`, `hotspot`) answer "which pixels would not hold still" over a
/// whole region, which is the right question when you are looking for a defect and the wrong one once
/// you have found it. Correlating a renderer-side probe against the pixels needs the opposite: *this*
/// pixel, *these* frames, no averaging. Run `WOW_DEPTH` and `--at` on the same coordinates and the two
/// logs line up frame for frame — "the colour changed here but the depth did not" is a fact you cannot
/// get from a region mean, because a run mean over 19 000 pixels hides which of them moved.
///
/// It also guards the trap this exists inside: a burst that happens to contain **no** flip reads as a
/// clean negative. The per-frame Δ makes an absent phenomenon obvious instead of invisible.
fn series(dir: &Path, at: &str) -> Result<()> {
    let pixels = parse_at(at)?;
    let names = pngs(dir)?;
    if names.is_empty() {
        bail!("no PNGs in {}", dir.display());
    }
    let frames: Vec<image::RgbImage> = names
        .iter()
        .map(|n| load(&dir.join(n)))
        .collect::<Result<_>>()?;
    for (x, y) in pixels {
        let (w, h) = (frames[0].width(), frames[0].height());
        if x >= w || y >= h {
            println!("({x}, {y}): outside the {w}x{h} frame");
            continue;
        }
        println!("({x}, {y}):");
        let mut prev: Option<f64> = None;
        for (i, f) in frames.iter().enumerate() {
            let p = f.get_pixel(x, y).0;
            let luma =
                0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
            let delta = prev.map_or(String::new(), |q| format!("  Δ{:+.2}", luma - q));
            println!(
                "  {i:3}  rgb({:3}, {:3}, {:3})  luma {luma:6.2}{delta}",
                p[0], p[1], p[2]
            );
            prev = Some(luma);
        }
    }
    Ok(())
}

/// Cut a window out of a capture, magnify it, and report what was actually cut — the vetted
/// replacement for the ad-hoc `sips`/`ffmpeg` crop pipelines that kept minting false findings
/// (`sips --cropOffset` takes (y, x) and silently ignores what it can't parse; a guessed ffmpeg
/// window read "no eyes" off a frame the model had left). Three honesty rules: a rect that misses
/// the frame entirely is an ERROR, never an edge-clamped guess; a rect that only partially fits is
/// clamped *out loud*; and every pixel sample prints in SOURCE coordinates at source resolution, so
/// nothing downstream does coordinate math on a zoomed image.
/// Per-channel min / mean / max over a rect (or the whole frame) — the "is this region flat?"
/// instrument. A frame edge that shows the render target's clear colour instead of art reads as
/// `min == max` on every channel (the glue framing's void check); a region of art
/// never does. Printed, not judged: the caller compares two rects or two captures.
fn stat_cmd(path: &Path, rect: Option<&str>) -> Result<()> {
    let img = load(path)?;
    let (w, h) = img.dimensions();
    let r = match rect {
        Some(spec) => {
            let want = parse_rect(spec)?;
            if want.x0 >= w || want.y0 >= h {
                bail!("--rect {spec} starts outside the {w}x{h} frame");
            }
            Rect {
                x0: want.x0,
                y0: want.y0,
                x1: want.x1.min(w),
                y1: want.y1.min(h),
            }
        }
        None => Rect {
            x0: 0,
            y0: 0,
            x1: w,
            y1: h,
        },
    };
    let mut min = [u8::MAX; 3];
    let mut max = [u8::MIN; 3];
    let mut sum = [0u64; 3];
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            let p = img.get_pixel(x, y).0;
            for c in 0..3 {
                min[c] = min[c].min(p[c]);
                max[c] = max[c].max(p[c]);
                sum[c] += u64::from(p[c]);
            }
        }
    }
    let n = u64::from(r.width()) * u64::from(r.height());
    let mean = sum.map(|s| s as f64 / n as f64);
    let flat = min == max;
    println!(
        "{}: {w}x{h} rect [{}..{}, {}..{}] ({} px)  min {:?}  mean [{:.1}, {:.1}, {:.1}]  max {:?}  {}",
        path.display(),
        r.x0,
        r.x1,
        r.y0,
        r.y1,
        n,
        min,
        mean[0],
        mean[1],
        mean[2],
        max,
        if flat { "FLAT" } else { "varied" }
    );
    Ok(())
}

fn crop_cmd(path: &Path, rect: &str, at: Option<&str>, scale: u32, out: &Path) -> Result<()> {
    let img = load(path)?;
    let (w, h) = img.dimensions();
    let want = parse_rect(rect)?;
    if want.x0 >= w || want.y0 >= h {
        bail!(
            "--rect {rect} starts outside the {w}x{h} frame — re-aim; a silently moved window is \
             exactly the mis-crop trap this tool exists to close"
        );
    }
    let r = Rect {
        x0: want.x0,
        y0: want.y0,
        x1: want.x1.min(w),
        y1: want.y1.min(h),
    };
    let clamped = r != want;
    let cut = crop(&img, r);
    let zoomed = zoom(&cut, scale);
    zoomed
        .save(out)
        .with_context(|| format!("writing {}", out.display()))?;
    println!(
        "{}: {w}x{h} → rect [{}..{}, {}..{}] ({}x{}){}  ×{}  → {} ({}x{})",
        path.display(),
        r.x0,
        r.x1,
        r.y0,
        r.y1,
        r.width(),
        r.height(),
        if clamped { "  CLAMPED to frame" } else { "" },
        scale.max(1),
        out.display(),
        zoomed.width(),
        zoomed.height(),
    );
    // Samples: the rect's centre always, plus any --at points — all in source coordinates.
    let mut samples = vec![((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)];
    if let Some(at) = at {
        samples.extend(parse_at(at)?);
    }
    for (x, y) in samples {
        if x >= w || y >= h {
            println!("  ({x}, {y}): outside the {w}x{h} frame");
            continue;
        }
        let p = img.get_pixel(x, y).0;
        let luma = 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
        let outside = if x < r.x0 || x >= r.x1 || y < r.y0 || y >= r.y1 {
            "  (outside the crop)"
        } else {
            ""
        };
        println!(
            "  ({x}, {y}): rgb({:3}, {:3}, {:3})  luma {luma:.2}{outside}",
            p[0], p[1], p[2]
        );
    }
    Ok(())
}

/// `"x,y,w,h"` → the rect it names. Strict for the same reason as [`parse_at`]: a half-parsed
/// window silently crops the wrong place, and a wrong crop reads as a finding.
fn parse_rect(spec: &str) -> Result<Rect> {
    let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
    let [x, y, w, h] = parts.as_slice() else {
        bail!("--rect wants \"<x>,<y>,<w>,<h>\", got {spec:?}");
    };
    let parse = |v: &str, name: &str| -> Result<u32> {
        v.parse().with_context(|| format!("--rect {name} {v:?}"))
    };
    let (x, y) = (parse(x, "x")?, parse(y, "y")?);
    let (w, h) = (parse(w, "w")?, parse(h, "h")?);
    if w == 0 || h == 0 {
        bail!("--rect wants a non-empty window, got {w}x{h}");
    }
    Ok(Rect {
        x0: x,
        y0: y,
        x1: x + w,
        y1: y + h,
    })
}

/// `"x,y;x,y"` → pixels. Strict: a typo here would silently report the wrong pixel's history.
fn parse_at(spec: &str) -> Result<Vec<(u32, u32)>> {
    spec.split(';')
        .filter(|s| !s.trim().is_empty())
        .map(|pair| {
            let (x, y) = pair.split_once(',').context("--at wants \"<x>,<y>;…\"")?;
            Ok((
                x.trim()
                    .parse()
                    .with_context(|| format!("bad x in {pair:?}"))?,
                y.trim()
                    .parse()
                    .with_context(|| format!("bad y in {pair:?}"))?,
            ))
        })
        .collect()
}

/// Stitch every `*.png` present in both dirs into `left | right` side-by-side images under `out`.
/// **The sub-pixel motion ruler** (`flow <burst_dir> [--rect "<x>,<y>,<w>,<h>"]`).
///
/// `flicker` and `series` answer *how much* a pixel changed. Neither can answer the question a
/// "the animation ticks" report actually asks — is the thing in front of the pixel advancing
/// **evenly**? — and the obvious way to ask it is a trap: recover one silhouette edge's sub-pixel
/// position frame by frame, and the ruler is built out of 8-bit pixels, so it carries its own
/// 1/255 quantisation and a 0.2 px step cannot be separated from the tool's noise floor. That
/// ambiguity is exactly what left the first staircase reading unattributed.
///
/// So estimate the whole window's displacement at once, by least squares over every pixel in it
/// (Lucas–Kanade, the standard first-order flow solve): thousands of gradients vote on one
/// `(dx, dy)`, the 8-bit floor averages down by `√N`, and the estimate lands two orders of
/// magnitude under a pixel. A breathing body is not a rigid translation and does not need to be —
/// what is read off the series is whether the aggregate advances smoothly, and a staircase in the
/// RENDER shows up as a staircase here whatever the body underneath is doing.
///
/// Columns are the per-frame displacement, its magnitude, and the **second difference** — the
/// curvature-per-frame discriminator the world-space jitter meter uses, on this side of the glass.
/// The summary counts **stalled** frames (under a fifth of the median step): a smooth pan has
/// none, a staircase is mostly them.
fn flow(dir: &Path, rect: Option<&str>) -> Result<()> {
    let names = pngs(dir)?;
    if names.len() < 3 {
        bail!(
            "flow wants at least 3 frames, {} has {}",
            dir.display(),
            names.len()
        );
    }
    let first = load(&dir.join(names.iter().next().expect("non-empty")))?;
    let (w, h) = first.dimensions();
    // The default window is the whole frame minus the one-pixel border the central differences
    // need. A named rect is clamped out loud, never silently (the `crop` honesty rule).
    let win = match rect {
        Some(spec) => {
            let want = parse_rect(spec)?;
            let got = Rect {
                x0: want.x0.min(w.saturating_sub(1)),
                y0: want.y0.min(h.saturating_sub(1)),
                x1: want.x1.min(w),
                y1: want.y1.min(h),
            };
            if got.x0 >= got.x1 || got.y0 >= got.y1 {
                bail!("--rect {spec:?} lies outside the {w}x{h} frame");
            }
            if got != want {
                println!(
                    "flow: --rect clamped to [{}..{}, {}..{}]",
                    got.x0, got.x1, got.y0, got.y1
                );
            }
            got
        }
        None => Rect {
            x0: 0,
            y0: 0,
            x1: w,
            y1: h,
        },
    };
    let (ww, wh) = ((win.x1 - win.x0) as usize, (win.y1 - win.y0) as usize);
    if ww < 3 || wh < 3 {
        bail!("flow wants a window at least 3x3, got {ww}x{wh}");
    }
    println!(
        "flow: {} frames, {w}x{h}, window [{}..{}, {}..{}] ({ww}x{wh} = {} px)",
        names.len(),
        win.x0,
        win.x1,
        win.y0,
        win.y1,
        ww * wh
    );
    println!("  frame        dx        dy      |d|      |d2|");
    let mut prev = luma_window(&first, &win);
    let mut steps: Vec<(f64, f64)> = Vec::new();
    for (i, name) in names.iter().enumerate().skip(1) {
        let cur = luma_window(&load(&dir.join(name))?, &win);
        let Some((dx, dy)) = lucas_kanade(&prev, &cur, ww, wh) else {
            println!("  {i:5}   (no gradient in the window — flow is undefined here)");
            prev = cur;
            continue;
        };
        let d2 = steps
            .last()
            .map(|&(px, py)| (dx - px).hypot(dy - py))
            .unwrap_or(f64::NAN);
        println!(
            "  {i:5}  {dx:+8.4}  {dy:+8.4}  {:7.4}  {:8.4}",
            dx.hypot(dy),
            d2
        );
        steps.push((dx, dy));
        prev = cur;
    }
    let mags: Vec<f64> = steps.iter().map(|&(x, y)| x.hypot(y)).collect();
    let curv: Vec<f64> = steps
        .windows(2)
        .map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1))
        .collect();
    if mags.is_empty() {
        return Ok(());
    }
    let med = |v: &mut Vec<f64>| -> f64 {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let med_mag = med(&mut mags.clone());
    let stalled = mags.iter().filter(|m| **m < 0.2 * med_mag).count();
    println!(
        "  --- steps: median |d| {med_mag:.4} px   max |d| {:.4}   stalled (<20% of median) {stalled}/{}",
        mags.iter().copied().fold(0.0, f64::max),
        mags.len()
    );
    if !curv.is_empty() {
        let med_c = med(&mut curv.clone());
        println!(
            "  --- curvature: median |d2| {med_c:.4} px   max |d2| {:.4}   ratio |d2|/|d| {:.3}",
            curv.iter().copied().fold(0.0, f64::max),
            med_c / med_mag.max(f64::MIN_POSITIVE)
        );
    }
    Ok(())
}

/// One frame's luma over `win`, row-major — the flow solve's input (f32 is exact for 8-bit sums
/// and halves the working set against a 3200x1800 f64 buffer).
fn luma_window(img: &image::RgbImage, win: &Rect) -> Vec<f32> {
    let mut out = Vec::with_capacity(((win.x1 - win.x0) * (win.y1 - win.y0)) as usize);
    for y in win.y0..win.y1 {
        for x in win.x0..win.x1 {
            let p = img.get_pixel(x, y).0;
            out.push(
                0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]),
            );
        }
    }
    out
}

/// Least-squares first-order optical flow between two luma windows: the `(dx, dy)` that best
/// explains `b` as `a` shifted, solved once over every interior pixel. `None` when the window
/// carries no usable gradient structure (a flat sky — the normal matrix is singular and any
/// answer would be invented).
fn lucas_kanade(a: &[f32], b: &[f32], w: usize, h: usize) -> Option<(f64, f64)> {
    let (mut sxx, mut sxy, mut syy, mut sxt, mut syt) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let ix = 0.5 * f64::from(a[i + 1] - a[i - 1]);
            let iy = 0.5 * f64::from(a[i + w] - a[i - w]);
            let it = f64::from(b[i] - a[i]);
            sxx += ix * ix;
            sxy += ix * iy;
            syy += iy * iy;
            sxt += ix * it;
            syt += iy * it;
        }
    }
    let det = sxx * syy - sxy * sxy;
    // Scale-relative: the normal matrix grows with the window, so an absolute epsilon would call
    // a big flat window solvable and a small textured one singular.
    if det <= 1e-12 * (sxx * syy).max(f64::MIN_POSITIVE) {
        return None;
    }
    Some(((sxy * syt - syy * sxt) / det, (sxy * sxt - sxx * syt) / det))
}

fn compose_dir(da: &Path, db: &Path, out: &Path) -> Result<()> {
    let names: BTreeSet<String> = pngs(da)?.intersection(&pngs(db)?).cloned().collect();
    if names.is_empty() {
        bail!(
            "no common *.png files between {} and {}",
            da.display(),
            db.display()
        );
    }
    std::fs::create_dir_all(out).ok();
    for name in &names {
        let (l, r) = (load(&da.join(name))?, load(&db.join(name))?);
        let composed = compose_side_by_side(&l, &r, COMPOSE_GAP);
        let dst = out.join(name);
        composed
            .save(&dst)
            .with_context(|| format!("writing {}", dst.display()))?;
        println!("{} -> {}", name, dst.display());
    }
    Ok(())
}

/// Report how far a burst of adjacent frames moved: the frame-to-frame table, then the whole-stack
/// envelope. Returns the worst consecutive-pair metrics (the `--fail` subject) — the envelope is a
/// *stack* number and can exceed any single pair, so failing on the pair is the conservative gate.
fn flicker(dir: &Path, out: Option<&Path>, amplify: u32, toggle_delta: u8) -> Result<Metrics> {
    let names: Vec<String> = pngs(dir)?.into_iter().collect(); // BTreeSet — already shot order
    if names.len() < 2 {
        bail!(
            "{} holds {} *.png — a flicker burst needs at least 2 (WOW_LIVE_SHOT_COUNT=<n>)",
            dir.display(),
            names.len()
        );
    }
    let frames: Vec<image::RgbImage> = names
        .iter()
        .map(|n| load(&dir.join(n)))
        .collect::<Result<_>>()?;

    let mut worst = Metrics {
        mae: 0.0,
        rmse: 0.0,
        max_delta: 0,
        pct_over: 0.0,
        changed: 0,
        worst_at: (0, 0),
    };
    for (pair, window) in frames.windows(2).enumerate() {
        let m = compare(&window[0], &window[1])?;
        println!(
            "{:<24} {}",
            format!("{} -> {}", names[pair], names[pair + 1]),
            fmt_metrics(&m)
        );
        if m.mae > worst.mae {
            worst = m;
        }
    }

    let e = envelope(&frames, amplify)?;
    println!(
        "envelope of {} frames: max swing {:>3}  mean {:>6.3}  unstable pixels {:>6.2}%",
        frames.len(),
        e.max_swing,
        e.mean_swing,
        e.pct_unstable * 100.0
    );
    // The moving-camera reading. Printed always, because which of the two numbers is meaningful
    // depends on whether the camera was parked — and a burst does not carry that fact.
    let t = toggles(&frames, toggle_delta, TOGGLE_AMPLIFY)?;
    println!(
        "toggles  of {} frames (delta {toggle_delta}): max reversals {:>3}  toggling pixels {:>6.2}%  \
         (the moving-camera reading — a monotone sweep scores 0)",
        frames.len(),
        t.max_reversals,
        t.pct_toggling * 100.0
    );
    if let Some(out) = out {
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).ok();
        }
        e.image
            .save(out)
            .with_context(|| format!("writing envelope {}", out.display()))?;
        println!("envelope -> {}", out.display());
        let toggle_out = sibling(out, "-toggle");
        t.image
            .save(&toggle_out)
            .with_context(|| format!("writing toggle map {}", toggle_out.display()))?;
        println!("toggles  -> {}", toggle_out.display());
    }
    Ok(worst)
}

/// Follow the toggle map to the thing it found: measure the toggling pixels' **shape**, then crop
/// the largest run out of every frame into one contact sheet.
///
/// The map says *where*; this says *what kind*, and shows the frames that prove it. A run that fills
/// its box and holds together is one surface blinking (visibility/culling); many thin runs are
/// z-fighting resolving per fragment. Both read identically on the toggle percentage.
fn hotspot(dir: &Path, out: &Path, toggle_delta: u8, pad: u32) -> Result<()> {
    let names: Vec<String> = pngs(dir)?.into_iter().collect();
    let frames: Vec<image::RgbImage> = names
        .iter()
        .map(|n| load(&dir.join(n)))
        .collect::<Result<_>>()?;
    let t = toggles(&frames, toggle_delta, TOGGLE_AMPLIFY)?;
    let s = shape(&t);
    let (w, h) = t.image.dimensions();
    println!(
        "toggling {:.2}%  coherence {:.3}  {} runs >= {} px  ({} px scattered)",
        t.pct_toggling * 100.0,
        s.coherence,
        s.regions.len(),
        benilla_visual::MIN_REGION_PIXELS,
        s.scattered,
    );
    for (i, r) in s.regions.iter().take(5).enumerate() {
        println!(
            "  #{i}  {:>8} px  fill {:.2}  box {}x{} at ({}, {})",
            r.pixels,
            r.fill(),
            r.bounds.width(),
            r.bounds.height(),
            r.bounds.x0,
            r.bounds.y0,
        );
        // Pasteable straight into the in-game ray pick, which is the next question every time:
        // WOW_PICK="<this>" names the surfaces at these exact pixels, front to back.
        let pick = r
            .samples()
            .iter()
            .map(|(x, y)| format!("{x},{y}"))
            .collect::<Vec<_>>()
            .join(";");
        println!("      WOW_PICK=\"{pick}\"");
    }
    let Some(biggest) = s.regions.first() else {
        bail!(
            "no toggling run of {} px or more — nothing to crop",
            benilla_visual::MIN_REGION_PIXELS
        );
    };
    // What the top runs are actually *doing*, frame by frame: re-shaded together, or edges moving.
    // Side by side, because whether they flip on the SAME steps is the difference between one global
    // cause and several local ones — and that is invisible in any single run's series.
    let series: Vec<Vec<benilla_visual::Step>> = s
        .regions
        .iter()
        .take(SERIES_RUNS)
        .map(|r| r.steps(&frames))
        .collect();
    println!("  step-by-step per run — mean luma, its delta, and how much of the run agreed:");
    for (i, _) in frames.windows(2).enumerate() {
        let cols: Vec<String> = series
            .iter()
            .map(|s| {
                format!(
                    "{:6.2}{:+7.2} @{:.2}",
                    s[i].mean_from, s[i].mean_delta, s[i].agreement
                )
            })
            .collect();
        println!("    {:>2} -> {:<2}  {}", i, i + 1, cols.join("  "));
    }
    // The two extremes of run #0, per channel — the *levels*, which is all this line is. Read the
    // ratios as description, never as a diagnosis: equal ratios rule out a scalar multiply and
    // nothing else, since an ADDED light moves the three channels by different factors too. The
    // reading that actually separates "one surface, re-lit" from "two surfaces" is the per-pixel
    // affine fit printed below it (`benilla_visual::relight`).
    let s0 = &series[0];
    let lo = s0
        .iter()
        .min_by(|a, b| a.mean_from.total_cmp(&b.mean_from))
        .expect("a burst has at least one step");
    let hi = s0
        .iter()
        .max_by(|a, b| a.mean_from.total_cmp(&b.mean_from))
        .expect("a burst has at least one step");
    let ratio: Vec<String> = (0..3)
        .map(|c| format!("{:.3}", hi.mean_rgb_from[c] / lo.mean_rgb_from[c].max(1e-9)))
        .collect();
    println!(
        "  #0 extremes: dim rgb ({:.1}, {:.1}, {:.1})  bright rgb ({:.1}, {:.1}, {:.1})  per-channel ratio {}",
        lo.mean_rgb_from[0], lo.mean_rgb_from[1], lo.mean_rgb_from[2],
        hi.mean_rgb_from[0], hi.mean_rgb_from[1], hi.mean_rgb_from[2],
        ratio.join(" / "),
    );
    // Same surface re-lit, or a different surface? Fit each run's pixels across its own biggest
    // frame-to-frame step. High R² = the pattern survived the flip and only its gain/offset moved,
    // so it is one surface being shaded differently; low R² = the two frames show different
    // surfaces, whatever the means do.
    println!(
        "  across each run's biggest step — is it one surface re-lit? (R² near 1 = yes), \
         against its quietest step as the control:"
    );
    for (i, r) in s.regions.iter().take(SERIES_RUNS).enumerate() {
        let (Some(flip), Some(quiet)) = (
            benilla_visual::relight::biggest_step(r, &frames),
            benilla_visual::relight::quietest_step(r, &frames),
        ) else {
            continue;
        };
        let fit = benilla_visual::relight::relight(r, &frames[flip], &frames[flip + 1]);
        let ctl = benilla_visual::relight::relight(r, &frames[quiet], &frames[quiet + 1]);
        let per: Vec<String> = (0..3)
            .map(|c| {
                format!(
                    "{:.2}x{:+.0} r²{:.3}",
                    fit.gain[c], fit.offset[c], fit.r2[c]
                )
            })
            .collect();
        // The control is what licenses the verdict: a low R² on the flip only means "different
        // surfaces" if the same pixels under the same pan fit tightly when they did NOT flip.
        let verdict = if ctl.worst_r2() < 0.9 {
            "no control — the pan alone decorrelates these pixels; fit says nothing"
        } else if fit.worst_r2() > 0.9 {
            "ONE SURFACE, re-lit"
        } else if fit.worst_r2() < 0.5 {
            "DIFFERENT SURFACES"
        } else {
            "inconclusive"
        };
        println!(
            "    #{i} flip {:>2}->{:<2}  {}   worst r² {:.3}   | quiet {:>2}->{:<2} r² {:.3}  =>  {verdict}",
            flip,
            flip + 1,
            per.join("  "),
            fit.worst_r2(),
            quiet,
            quiet + 1,
            ctl.worst_r2(),
        );
    }
    let rect = biggest.bounds.padded(pad, w, h);
    // The toggle map goes in as the first tile, so the sheet carries its own legend: this is the
    // region, and here is what the frames did inside it.
    let mut tiles = vec![crop(&t.image, rect)];
    tiles.extend(frames.iter().map(|f| crop(f, rect)));
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).ok();
    }
    contact_strip(&tiles, STRIP_COLS, COMPOSE_GAP)
        .save(out)
        .with_context(|| format!("writing hotspot strip {}", out.display()))?;
    println!(
        "crop {}x{} at ({}, {}) x {} tiles -> {}",
        rect.width(),
        rect.height(),
        rect.x0,
        rect.y0,
        tiles.len(),
        out.display()
    );
    Ok(())
}

/// `foo.png` + `-toggle` → `foo-toggle.png`. The two maps answer the same question under opposite
/// camera conditions, so they are written as a pair rather than behind a second flag nobody would
/// remember to pass.
fn sibling(out: &Path, suffix: &str) -> PathBuf {
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("png");
    let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    out.with_file_name(format!("{stem}{suffix}.{ext}"))
}

fn next(it: &mut std::slice::Iter<String>, flag: &str) -> Result<String> {
    it.next()
        .cloned()
        .with_context(|| format!("{flag} needs a value"))
}

fn two<'a>(rest: &'a [String], cmd: &str) -> Result<[&'a str; 2]> {
    match rest {
        [a, b] => Ok([a, b]),
        _ => bail!("{cmd} needs exactly two paths, got {}", rest.len()),
    }
}

fn one<'a>(rest: &'a [String], cmd: &str) -> Result<[&'a str; 1]> {
    match rest {
        [a] => Ok([a]),
        _ => bail!("{cmd} needs exactly one path, got {}", rest.len()),
    }
}

fn over_fail(m: &Metrics, fail: Option<f64>) -> bool {
    fail.is_some_and(|t| m.mae > t)
}

/// Load an image as RGB (dropping any alpha — capture windows are opaque).
/// One silhouette's sub-pixel position along a scanline, per frame — the shading-independent
/// answer to "did the rendered geometry MOVE smoothly?".
///
/// Per frame and per scanline: take the row's own darkest and brightest samples, put the
/// threshold halfway between them, and linearly interpolate the crossing. Renormalising to the
/// row's own ends every frame is the whole point — a uniform brightness change (the body turning
/// into the light) moves `lo` and `hi` together and leaves the crossing where it was, while a
/// geometric shift moves it. That is the property [`flow`] lacks.
fn edge(dir: &Path, at: &str) -> Result<()> {
    let names = pngs(dir)?;
    if names.len() < 3 {
        bail!(
            "edge wants at least 3 frames, {} has {}",
            dir.display(),
            names.len()
        );
    }
    let mut lines: Vec<(u32, u32, u32)> = Vec::new();
    for spec in at.split(';').filter(|s| !s.trim().is_empty()) {
        let p: Vec<u32> = spec
            .trim()
            .split(',')
            .map(|v| {
                v.trim()
                    .parse::<u32>()
                    .context("edge --at wants <x0>,<y>,<x1>")
            })
            .collect::<Result<_>>()?;
        let [x0, y, x1] = p[..] else {
            bail!("edge --at wants <x0>,<y>,<x1>, got {spec:?}");
        };
        if x1 <= x0 + 1 {
            bail!("edge scanline {spec:?} needs x1 > x0+1");
        }
        lines.push((x0, y, x1));
    }
    for &(x0, y, x1) in &lines {
        println!("scanline y={y}  x {x0}..{x1}");
        let mut pos: Vec<f64> = Vec::new();
        for name in &names {
            let img = load(&dir.join(name))?;
            if y >= img.height() || x1 >= img.width() {
                bail!(
                    "scanline ({x0},{y},{x1}) outside {}x{}",
                    img.width(),
                    img.height()
                );
            }
            let row: Vec<f64> = (x0..=x1)
                .map(|x| {
                    let p = img.get_pixel(x, y).0;
                    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
                })
                .collect();
            let lo = row.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = row.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            // Too little contrast to call an edge — report it rather than invent a position.
            if hi - lo < 12.0 {
                pos.push(f64::NAN);
                continue;
            }
            let mid = 0.5 * (lo + hi);
            let mut found = f64::NAN;
            for i in 0..row.len() - 1 {
                let (a, b) = (row[i] - mid, row[i + 1] - mid);
                if (a <= 0.0 && b > 0.0) || (a >= 0.0 && b < 0.0) {
                    found = f64::from(x0) + i as f64 + a / (a - b);
                    break;
                }
            }
            pos.push(found);
        }
        let good = pos.iter().filter(|p| p.is_finite()).count();
        for (i, p) in pos.iter().enumerate().take(28) {
            let d = if i == 0 { f64::NAN } else { p - pos[i - 1] };
            println!("    {i:4}  x={p:10.4}  d={d:+8.4}");
        }
        let d1: Vec<f64> = pos
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .filter(|v| v.is_finite())
            .collect();
        let d2: Vec<f64> = pos
            .windows(3)
            .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
            .filter(|v| v.is_finite())
            .collect();
        if d1.is_empty() || d2.is_empty() {
            println!(
                "  no usable edge on {}/{} frames",
                names.len() - good,
                names.len()
            );
            continue;
        }
        let med = |mut v: Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
            v[v.len() / 2]
        };
        let (m1, m2) = (med(d1.clone()), med(d2.clone()));
        let mx = d1.iter().copied().fold(0.0, f64::max);
        println!(
            "  frames with an edge {good}/{}   median |d| {m1:.4} px   max |d| {mx:.4} px",
            names.len()
        );
        println!(
            "  median |d2| {m2:.4} px   roughness |d2|/|d| {:.3}   (a smooth curve at 60 Hz is <<1)",
            m2 / m1.max(1e-9)
        );
        let stalled = d1.iter().filter(|&&v| v < 0.2 * m1).count();
        println!("  stalled steps (<20% of median): {stalled}/{}", d1.len());
    }
    Ok(())
}

fn load(path: &Path) -> Result<image::RgbImage> {
    Ok(image::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .to_rgb8())
}

fn diff_one(a: &Path, b: &Path, out: Option<&Path>, amplify: u32) -> Result<Metrics> {
    let (ia, ib) = (load(a)?, load(b)?);
    let m = compare(&ia, &ib)?;
    if let Some(out) = out {
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).ok();
        }
        diff_image(&ia, &ib, amplify)?
            .save(out)
            .with_context(|| format!("writing diff {}", out.display()))?;
    }
    Ok(m)
}

/// Compare every `*.png` present in both dirs (by file name). Returns the worst (highest-MAE) result.
/// What one `diff-dir` run found: the worst pair by MAE, and every image present on only ONE side.
struct DirDiff {
    worst: Option<(String, Metrics)>,
    /// Files with no counterpart, so not diffed at all — see the note in [`diff_dir`].
    unpaired: Vec<String>,
}

fn diff_dir(da: &Path, db: &Path, out: Option<&Path>, amplify: u32) -> Result<DirDiff> {
    let (a, b) = (pngs(da)?, pngs(db)?);
    let names: BTreeSet<String> = a.intersection(&b).cloned().collect();
    if names.is_empty() {
        bail!(
            "no common *.png files between {} and {}",
            da.display(),
            db.display()
        );
    }
    // UNPAIRED shots are ANNOUNCED, never skipped in silence. Pairing on the intersection alone is
    // how a sweep reports all-green with a shot missing: on 2026-07-28 one `water-night` capture
    // exited 0 without writing its PNG, and `selfcheck` diffed the other eight and passed clean. A
    // gate that quietly narrows its own scope is worse than no gate. Reported always; fatal to the
    // CALLER when `--fail` is in force, i.e. whenever this is being used as a gate.
    let unpaired: Vec<String> = a
        .difference(&b)
        .map(|n| format!("{}/{n}", da.display()))
        .chain(b.difference(&a).map(|n| format!("{}/{n}", db.display())))
        .collect();
    for u in &unpaired {
        eprintln!("UNPAIRED (no counterpart — NOT diffed): {u}");
    }
    if let Some(out) = out {
        std::fs::create_dir_all(out).ok();
    }
    let mut worst: Option<(String, Metrics)> = None;
    for name in &names {
        let diff_out = out.map(|o| o.join(name));
        let m = diff_one(&da.join(name), &db.join(name), diff_out.as_deref(), amplify)?;
        println!("{:<24} {}", name, fmt_metrics(&m));
        if worst.as_ref().is_none_or(|(_, w)| m.mae > w.mae) {
            worst = Some((name.clone(), m));
        }
    }
    if let Some((name, m)) = &worst {
        println!("worst: {name} (MAE {:.3})", m.mae);
    }
    Ok(DirDiff { worst, unpaired })
}

fn pngs(dir: &Path) -> Result<BTreeSet<String>> {
    let mut set = BTreeSet::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading dir {}", dir.display()))? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                set.insert(name.to_string());
            }
        }
    }
    Ok(set)
}

fn fmt_metrics(m: &Metrics) -> String {
    // `px` and the worst pixel's coordinate are what tell a render change from an MSAA tie: both
    // show `MAE 0.000` with an alarming `max`, and only the count and the location separate them
    // (a handful of pixels, at the same coordinate every build, is a tie).
    let where_ = if m.changed == 0 {
        String::new()
    } else {
        format!(" @{},{}", m.worst_at.0, m.worst_at.1)
    };
    format!(
        "MAE {:>6.3}  RMSE {:>6.3}  max {:>3}{:<12}  px {:>8}  >{}: {:>6.2}%",
        m.mae,
        m.rmse,
        m.max_delta,
        where_,
        m.changed,
        OVER_THRESHOLD,
        m.pct_over * 100.0
    )
}

fn print_usage() {
    eprintln!(
        "benilla-visual — diff Phase-5 visual-harness captures\n\
         \n\
         USAGE:\n  \
           benilla-visual diff        <a.png> <b.png> [--out <diff.png>] [--fail <mae>] [--amplify <n>]\n  \
           benilla-visual diff-dir    <dir_a> <dir_b> [--out <diff_dir>] [--fail <mae>] [--amplify <n>]\n  \
           benilla-visual flicker     <burst_dir>     [--out <envelope.png>] [--fail <mae>] [--amplify <n>] [--toggle-delta <n>]\n  \
           benilla-visual hotspot     <burst_dir>     --out <strip.png> [--toggle-delta <n>] [--pad <n>]\n  \
           benilla-visual series      <burst_dir>     --at \"<x>,<y>[;<x>,<y>…]\"\n  \
           benilla-visual flow        <burst_dir>     [--rect \"<x>,<y>,<w>,<h>\"]\n  \
           benilla-visual edge        <burst_dir>     --at \"<x0>,<y>,<x1>[;…]\"\n  \
           benilla-visual compose-dir <dir_a> <dir_b> --out <dir>   (side-by-side `a | b` per image)\n  \
           benilla-visual stat        <img.png>       [--rect \"<x>,<y>,<w>,<h>\"]\n\
           benilla-visual crop        <img.png>       --rect \"<x>,<y>,<w>,<h>\" --out <crop.png> [--scale <n>] [--at \"<x>,<y>;…\"]\n\
         \n\
         flicker reads a WOW_LIVE_SHOT_COUNT burst (adjacent frames) in shot order and reports both\n\
         the parked reading (envelope) and the moving-camera one (toggles).\n\
         hotspot follows the toggle map: it measures the toggling pixels' shape — one solid run is a\n\
         surface blinking, many thin ones are z-fighting — and crops the largest into a contact sheet.\n\
         series drops the averaging: one named pixel's rgb/luma per frame, to line up against the\n\
         client's WOW_DEPTH log at the same coordinates.\n\
         edge NOISE FLOOR: about +/-0.5 px per frame on real content. VALIDATED against a provably
smooth 0.5 deg/s camera pan over static geometry (true motion 0.28 px/frame, monotone): edge
reported steps of -0.241 +0.212 +0.998 +0.732 -0.749 — NEGATIVE steps on a monotone pan. Its
MEAN is right, its per-frame value is not. So it can only resolve discrete events well above
~1 px (the attachment staircase's 0.83-1.1 px hop clears it; a 0.1-0.2 px breathing motion does
not, and a roughness statistic computed at that scale is measuring this floor, not the render).
edge tracks a SILHOUETTE's sub-pixel position along one scanline, per frame. Unlike flow it
does not assume brightness constancy — it renormalises to the scanline's own light/dark ends
every frame, so a shaded, deforming surface (skin, cloth) whose lighting changes as it moves
still reads its GEOMETRY. flow is invalid on exactly that content: on a run where the palette
was provably 33x rougher it reported *smoother*, because Lucas-Kanade read the shading change
as motion. Use edge for a body, flow only for rigid, uniformly-lit subjects.
flow measures MOTION rather than change: one least-squares sub-pixel displacement per frame\n\
         pair over the whole window, so a render that advances in steps reads as steps and one that\n\
         advances smoothly reads smooth — the question a per-pixel delta cannot answer.\n\
         crop cuts a window for close reading (nearest-neighbour zoom only, samples printed in\n\
         SOURCE coordinates) — use it instead of sips/ffmpeg pipelines, which have minted false\n\
         findings (sips --cropOffset is (y, x) and silently ignores what it can't parse).\n\
         Exits non-zero if any image's MAE exceeds --fail."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_spec_is_strict() {
        // "x,y,w,h" and nothing else — a half-parsed window crops the wrong place silently.
        let r = parse_rect("10, 20, 30, 40").unwrap();
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (10, 20, 40, 60));
        assert!(parse_rect("10,20,30").is_err());
        assert!(parse_rect("10,20,30,40,50").is_err());
        assert!(parse_rect("10,20,0,40").is_err());
        assert!(parse_rect("a,20,30,40").is_err());
    }
}
