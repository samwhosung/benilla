//! The 1-bit raster of a `monochrome` font: the reference loads the glyph unhinted (`0x5d1300`,
//! load flags `0x20c2`) and renders it in FreeType's mono mode (`0x7cec20`, mode 1), whose scan
//! converter sets a pixel when its centre lies inside the outline. A span that holds no centre, a
//! stroke thinner than a pixel, sets the pixel on its low side instead, unless the span is the tip
//! of a stub or its high-side pixel is already set: the converter's default drop-out control, run
//! along the rows and then, as a second pass, down the columns.

use super::sfnt::Outline26;

/// A 1-bit glyph, one byte per pixel (0 or 1), rows from the top, over the control box rounded out
/// as FreeType sizes the bitmap; `top` is its top row over the baseline in whole px.
pub(super) struct MonoGlyph {
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) top: i32,
    pub(super) bits: Vec<u8>,
}

type Pt = (f64, f64);

/// One TrueType segment in px, y up, already monotonic along the swept axis when swept.
#[derive(Clone, Copy)]
enum Piece {
    Line(Pt, Pt),
    Quad(Pt, Pt, Pt),
}

impl Piece {
    fn ends(self) -> (Pt, Pt) {
        match self {
            Piece::Line(a, b) | Piece::Quad(a, _, b) => (a, b),
        }
    }

    /// The same piece with x and y exchanged, so a column sweep runs as a row sweep.
    fn transposed(self) -> Piece {
        let t = |p: Pt| (p.1, p.0);
        match self {
            Piece::Line(a, b) => Piece::Line(t(a), t(b)),
            Piece::Quad(a, c, b) => Piece::Quad(t(a), t(c), t(b)),
        }
    }

    /// Split at the y extremum, so each part is monotonic in y.
    fn monotonic(self, out: &mut Vec<Piece>) {
        if let Piece::Quad(a, c, b) = self {
            let den = a.1 - 2.0 * c.1 + b.1;
            if den != 0.0 {
                let t = (a.1 - c.1) / den;
                if t > 0.0 && t < 1.0 {
                    let lerp = |p: Pt, q: Pt| (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
                    let (ac, cb) = (lerp(a, c), lerp(c, b));
                    let m = lerp(ac, cb);
                    out.push(Piece::Quad(a, ac, m));
                    out.push(Piece::Quad(m, cb, b));
                    return;
                }
            }
        }
        out.push(self);
    }

    /// Where this y-monotonic piece crosses height `y`, if it does.
    fn x_at(self, y: f64) -> Option<f64> {
        let (a, b) = self.ends();
        let (lo, hi) = (a.1.min(b.1), a.1.max(b.1));
        if y < lo || y > hi || lo == hi {
            return None;
        }
        match self {
            Piece::Line(..) => Some(a.0 + (y - a.1) * (b.0 - a.0) / (b.1 - a.1)),
            Piece::Quad(_, c, _) => {
                // `qa·t² + qb·t + qc = 0`, the root in [0, 1].
                let (qa, qb, qc) = (a.1 - 2.0 * c.1 + b.1, 2.0 * (c.1 - a.1), a.1 - y);
                let t = if qa.abs() < 1e-12 {
                    -qc / qb
                } else {
                    let disc = (qb * qb - 4.0 * qa * qc).max(0.0).sqrt();
                    let q = -0.5 * (qb + qb.signum() * disc);
                    let (t1, t2) = (q / qa, if q != 0.0 { qc / q } else { f64::NAN });
                    if (-1e-9..=1.0 + 1e-9).contains(&t1) {
                        t1
                    } else {
                        t2
                    }
                }
                .clamp(0.0, 1.0);
                let mt = 1.0 - t;
                Some(mt * mt * a.0 + 2.0 * mt * t * c.0 + t * t * b.0)
            }
        }
    }
}

/// A run of one contour monotonic in the swept axis, the scan converter's profile: the sample
/// lines it spans, and the next profile along its contour, which tells a stub's tip.
struct Profile {
    up: bool,
    pieces: Vec<Piece>,
    first: i64,
    last: i64,
    next: usize,
}

impl Profile {
    /// Its crossing of sample line `y`; where two pieces meet on the line, the later one's.
    fn x_at(&self, y: f64) -> Option<f64> {
        self.pieces.iter().rev().find_map(|p| p.x_at(y))
    }
}

/// Every contour's profiles; a run that spans no sample line is dropped and its neighbours join.
fn profiles(contours: &[Vec<Piece>]) -> Vec<Profile> {
    let mut out = Vec::new();
    for contour in contours {
        let mut split = Vec::new();
        for p in contour {
            p.monotonic(&mut split);
        }
        let mut runs: Vec<(bool, Vec<Piece>)> = Vec::new();
        for p in split {
            let (a, b) = p.ends();
            if a.1 == b.1 {
                continue; // flat along the sweep: no crossing, and it ends no run
            }
            let up = b.1 > a.1;
            match runs.last_mut() {
                Some((u, v)) if *u == up => v.push(p),
                _ => runs.push((up, vec![p])),
            }
        }
        if runs.len() > 1 && runs[0].0 == runs[runs.len() - 1].0 {
            let (_, mut tail) = runs.pop().expect("more than one run");
            tail.append(&mut runs[0].1);
            runs[0].1 = tail;
        }
        let base = out.len();
        for (up, pieces) in runs {
            let (lo, hi) = pieces.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
                let (a, b) = p.ends();
                (lo.min(a.1).min(b.1), hi.max(a.1).max(b.1))
            });
            // Sample line `k` sits at `k + 0.5`; a profile spans those within its extent.
            let (first, last) = ((lo - 0.5).ceil() as i64, (hi - 0.5).floor() as i64);
            if first <= last {
                out.push(Profile {
                    up,
                    pieces,
                    first,
                    last,
                    next: 0,
                });
            }
        }
        let n = out.len() - base;
        for i in 0..n {
            out[base + i].next = base + (i + 1) % n;
        }
    }
    out
}

/// One pass over `lines` sample lines of `len` pixels each. Each line pairs its rising crossings
/// with its falling ones in order; a span wider than a pixel fills the centres it holds (when
/// `fill` is the span tolerance, the row pass), one at most a pixel wide is resolved after the
/// line's spans by the drop-out rule. `at(line, pos)` indexes `bits`.
fn sweep(
    profiles: &[Profile],
    lines: i64,
    len: i64,
    fill: Option<f64>,
    bits: &mut [u8],
    at: impl Fn(i64, i64) -> usize,
) {
    let set = |bits: &mut [u8], k: i64, pos: i64| {
        if (0..len).contains(&pos) {
            bits[at(k, pos)] = 1;
        }
    };
    for k in 0..lines {
        let y = k as f64 + 0.5;
        let (mut ups, mut downs) = (Vec::new(), Vec::new());
        for (i, p) in profiles.iter().enumerate() {
            if (p.first..=p.last).contains(&k) {
                if let Some(x) = p.x_at(y) {
                    (if p.up { &mut ups } else { &mut downs }).push((x, i));
                }
            }
        }
        ups.sort_by(|a, b| a.0.total_cmp(&b.0));
        downs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut drops = Vec::new();
        for (&(xa, a), &(xb, b)) in ups.iter().zip(&downs) {
            let (x1, x2) = if xa <= xb { (xa, xb) } else { (xb, xa) };
            // Pixel `n`'s centre is at `n + 0.5`: in `u = x − 0.5` it is the integer `n`.
            let (u1, u2) = (x1 - 0.5, x2 - 0.5);
            if x2 - x1 <= 1.0 && u2.ceil() - u1.floor() == 1.0 {
                drops.push((u1, u2, a, b));
            } else if let Some(jitter) = fill {
                let e1 = u1.ceil() as i64;
                // A span within a pixel and the jitter fills one pixel even when both ends sit on
                // centres (`0x7dac90`, `0x7dacb2`-`0x7dacbd`).
                let e2 = if x2 - x1 <= 1.0 + jitter {
                    e1
                } else {
                    u2.floor() as i64
                };
                for pos in e1..=e2 {
                    set(bits, k, pos);
                }
            }
        }
        for (u1, u2, a, b) in drops {
            let (e1, e2) = (u1.ceil() as i64, u2.floor() as i64);
            if e1 <= e2 {
                set(bits, k, e1); // it holds a centre after all
                continue;
            }
            // `a` rises, `b` falls: joined at a peak this is the last line under, or at a valley
            // this is the first line over, the span is a stub's tip and stays empty.
            let (pa, pb) = (&profiles[a], &profiles[b]);
            if (pa.next == b && k == pa.last) || (pb.next == a && k == pa.first) {
                continue;
            }
            if (0..len).contains(&e1) && bits[at(k, e1)] != 0 {
                continue;
            }
            set(bits, k, e2);
        }
    }
}

/// The span procedure's jitter in px: 24/1024 at high precision, which a size under 24 px sets,
/// else 2/64 (`0x7dabe0`; the size test `0x7d34b4`-`0x7d34bb`, read at `0x7daae4`).
fn jitter(ppem: u16) -> f64 {
    if ppem < 24 {
        24.0 / 1024.0
    } else {
        2.0 / 64.0
    }
}

/// Rasterize an outline (26.6, y up, from the pen) at `ppem`; `None` for one with no area to
/// cover.
pub(super) fn rasterize(o: &Outline26, ppem: u16) -> Option<MonoGlyph> {
    let n = o.pts.len();
    if n == 0 || o.on.len() != n {
        return None;
    }
    // FreeType's bitmap: the control box of every point, rounded out to whole pixels.
    let (mut x0, mut x1, mut y0, mut y1) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
    for &(x, y) in &o.pts {
        (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
    }
    let (left, bottom) = (x0.div_euclid(64), y0.div_euclid(64));
    let (right, top) = ((x1 + 63).div_euclid(64), (y1 + 63).div_euclid(64));
    let (w, h) = (right - left, top - bottom);
    if w <= 0 || h <= 0 {
        return None;
    }
    // Points in px from the bitmap's bottom-left corner.
    let px = |i: usize| {
        let (x, y) = o.pts[i];
        (
            (x - left * 64) as f64 / 64.0,
            (y - bottom * 64) as f64 / 64.0,
        )
    };
    let mut contours = Vec::new();
    let mut start = 0;
    for &end in &o.ends {
        if end < start || end >= n {
            break;
        }
        contours.push(contour_pieces(
            &(start..=end).collect::<Vec<_>>(),
            &o.on,
            &px,
        ));
        start = end + 1;
    }
    let (wi, hi) = (w as usize, h as usize);
    let mut bits = vec![0u8; wi * hi];
    // Rows bottom-up, stored top-down.
    let row_at = |k: i64, pos: i64| (hi - 1 - k as usize) * wi + pos as usize;
    sweep(
        &profiles(&contours),
        h,
        w,
        Some(jitter(ppem)),
        &mut bits,
        row_at,
    );
    let transposed: Vec<Vec<Piece>> = contours
        .iter()
        .map(|c| c.iter().map(|p| p.transposed()).collect())
        .collect();
    let col_at = |k: i64, pos: i64| (hi - 1 - pos as usize) * wi + k as usize;
    sweep(&profiles(&transposed), w, h, None, &mut bits, col_at);
    Some(MonoGlyph {
        w: w as u32,
        h: h as u32,
        top: top as i32,
        bits,
    })
}

/// One contour's segments: on-curve points join by lines, an off-curve point is a quadratic's
/// control, and two off-curve points in a row imply the on-curve midpoint between them.
fn contour_pieces(idx: &[usize], on: &[bool], px: &impl Fn(usize) -> Pt) -> Vec<Piece> {
    let m = idx.len();
    let mid = |p: Pt, q: Pt| ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0);
    // Start on an on-curve point, or on the midpoint of the last and first when neither is.
    let (first, start) = match (0..m).find(|&i| on[idx[i]]) {
        Some(i) => (i, px(idx[i])),
        None => (0, mid(px(idx[m - 1]), px(idx[0]))),
    };
    // From an on-curve start, every point after it and the start again; from a midpoint, every
    // point, the first being a control.
    let order: Vec<usize> = if on[idx[first]] {
        (1..=m).map(|s| idx[(first + s) % m]).collect()
    } else {
        idx.to_vec()
    };
    let mut pieces = Vec::new();
    let mut cur = start;
    let mut ctrl: Option<Pt> = None;
    for i in order {
        let p = px(i);
        if on[i] {
            pieces.push(match ctrl.take() {
                Some(c) => Piece::Quad(cur, c, p),
                None => Piece::Line(cur, p),
            });
            cur = p;
        } else {
            if let Some(c) = ctrl {
                let q = mid(c, p);
                pieces.push(Piece::Quad(cur, c, q));
                cur = q;
            }
            ctrl = Some(p);
        }
    }
    // Close back to the start.
    match ctrl {
        Some(c) => pieces.push(Piece::Quad(cur, c, start)),
        None if cur != start => pieces.push(Piece::Line(cur, start)),
        None => {}
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed outline of on-curve points, in px.
    fn poly(contours: &[&[(f64, f64)]]) -> Outline26 {
        let mut o = Outline26::default();
        for c in contours {
            for &(x, y) in *c {
                o.pts.push(((x * 64.0) as i64, (y * 64.0) as i64));
                o.on.push(true);
            }
            o.ends.push(o.pts.len() - 1);
        }
        o
    }

    /// The bitmap as rows of `@` and `.`, from the top.
    fn rows(g: &MonoGlyph) -> Vec<String> {
        g.bits
            .chunks(g.w as usize)
            .map(|r| r.iter().map(|&b| if b != 0 { '@' } else { '.' }).collect())
            .collect()
    }

    #[test]
    fn a_pixel_is_set_when_its_centre_is_inside() {
        // Clockwise, y up. Centres at 0.5 and 1.5 inside [0.25, 1.75] by x, only 0.5 by y.
        let g = rasterize(
            &poly(&[&[(0.25, 0.25), (0.25, 1.0), (1.75, 1.0), (1.75, 0.25)]]),
            12,
        )
        .unwrap();
        assert_eq!((g.w, g.h, g.top), (2, 1, 1), "the control box rounded out");
        assert_eq!(rows(&g), ["@@"]);
    }

    #[test]
    fn a_counter_stays_open() {
        // A 4 × 4 square with a 2 × 2 hole, the hole counter-clockwise.
        let g = rasterize(
            &poly(&[
                &[(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0)],
                &[(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)],
            ]),
            12,
        )
        .unwrap();
        assert_eq!(rows(&g), ["@@@@", "@..@", "@..@", "@@@@"]);
    }

    #[test]
    fn a_stroke_thinner_than_a_pixel_drops_out_to_its_low_side_but_not_at_its_tips() {
        // A bar 0.3 px wide between the centres 1.5 and 2.5, five rows tall: each row's span holds
        // no centre and sets the pixel on its left; the top and bottom rows are the stub's tips,
        // where its two edges join along the bar's ends, and stay empty.
        let g = rasterize(
            &poly(&[&[(1.6, 0.0), (1.6, 5.0), (1.9, 5.0), (1.9, 0.0)]]),
            12,
        )
        .unwrap();
        assert_eq!(rows(&g), [".", "@", "@", "@", "."]);
    }

    #[test]
    fn a_thin_horizontal_stroke_drops_out_downward_in_the_column_pass() {
        // A 6 px bar 0.3 px tall between the row centres 0.5 and 1.5, joined to a full-height post
        // at its left so its ends are no stub tips: the row pass sets nothing in it, the column
        // pass sets the lower row.
        let g = rasterize(
            &poly(&[&[
                (0.0, 0.0),
                (0.0, 2.0),
                (1.0, 2.0),
                (1.0, 0.9),
                (6.0, 0.9),
                (6.0, 0.6),
                (1.0, 0.6),
                (1.0, 0.0),
            ]]),
            12,
        )
        .unwrap();
        assert_eq!(rows(&g), ["@.....", "@@@@@."]);
    }

    #[test]
    fn a_span_just_over_a_pixel_fills_one_within_the_jitter() {
        let bar = |x1: f64, x2: f64| poly(&[&[(x1, 0.0), (x1, 1.0), (x2, 1.0), (x2, 0.0)]]);
        // 65/64 px wide, both centres 0.5 and 1.5 inside: under the 24/1024 px jitter, one pixel.
        let g = rasterize(&bar(0.5, 97.0 / 64.0), 12).unwrap();
        assert_eq!(rows(&g), ["@."]);
        // 66/64 px: past the high-precision jitter both centres fill; within 2/64 at 24 px, one.
        let g = rasterize(&bar(31.0 / 64.0, 97.0 / 64.0), 12).unwrap();
        assert_eq!(rows(&g), ["@@"]);
        let g = rasterize(&bar(31.0 / 64.0, 97.0 / 64.0), 24).unwrap();
        assert_eq!(rows(&g), ["@."]);
    }

    /// The hotkey numerals of 1.12.1's action bar, `NumberFontNormalSmallGray` (Arial Narrow,
    /// `monochrome`, THICK), as a 1.12.1 capture shows them at 15 px: each texel the client drew
    /// in the font's flat gray is `@`.
    #[test]
    fn arial_narrow_numerals_match_the_reference_capture() {
        let Some(mut e) = super::super::test_engine(1.0) else {
            eprintln!("skipping: no client install / patch chain");
            return;
        };
        let face = e.face_for(Some("Fonts\\ARIALN.TTF"));
        let golden: [(char, &[&str]); 3] = [
            (
                '2',
                &[
                    "..@@@..", ".@@.@@.", ".@...@.", ".....@.", ".....@.", "....@..", "...@@..",
                    "..@@...", ".@@....", ".@.....", "@@@@@@.",
                ],
            ),
            (
                '1',
                &[
                    "...@", "..@@", ".@@@", "@.@@", "..@@", "..@@", "..@@", "..@@", "..@@", "..@@",
                    "..@@",
                ],
            ),
            (
                '=',
                &[".@@@@@.", ".@@@@@.", ".......", ".......", ".@@@@@."],
            ),
        ];
        for (ch, want) in golden {
            e.ensure_metrics(face, 15, &ch.to_string());
            let gid = e.char_cell(face, 15, ch).expect("shaped").glyphs[0].glyph_id;
            let sfnt = e.faces[face].sfnt.clone().expect("Arial Narrow's tables");
            let g = rasterize(&sfnt.outline(gid, 15).expect("outline"), 15).expect("ink");
            assert_eq!(rows(&g), want, "{ch:?}");
        }
    }
}
