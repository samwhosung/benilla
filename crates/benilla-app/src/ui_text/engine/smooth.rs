//! The anti-aliased raster of a glyph: the reference loads it unhinted (`0x5d1300`, load flags
//! `0x208a`, which carry `FT_LOAD_NO_HINTING`) and renders it in FreeType's normal mode
//! (`0x7cec20`, mode 0), the smooth rasterizer's exact-area coverage over the 26.6 outline. `zeno`'s
//! scan converter ports a later version of that rasterizer, at its 1/256 px precision.

use zeno::{Command, Format, Mask, Origin, Point, Vector};

use super::sfnt::Outline26;

/// A coverage bitmap, one byte per pixel, rows from the top, over the control box rounded out as
/// FreeType sizes the bitmap; `top` is its top row over the baseline in whole px.
pub(super) struct SmoothGlyph {
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) top: i32,
    pub(super) coverage: Vec<u8>,
}

/// Rasterize an outline (26.6, y up, from the pen); `None` for one with no area to cover.
pub(super) fn rasterize(o: &Outline26) -> Option<SmoothGlyph> {
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
    let path = commands(o)?;
    let (w, h) = (w as u32, h as u32);
    let mut coverage = vec![0u8; (w * h) as usize];
    Mask::new(&path[..])
        .format(Format::Alpha)
        .origin(Origin::BottomLeft)
        .size(w, h)
        .offset(Vector::new(-(left as f32), -(bottom as f32)))
        .render_into(&mut coverage, None);
    Some(SmoothGlyph {
        w,
        h,
        top: top as i32,
        coverage,
    })
}

/// The outline as FreeType's `FT_Outline_Decompose` walks it: on-curve points join by lines, an
/// off-curve point is a conic's control, and two in a row imply the on-curve point between them,
/// their 26.6 sum halved toward zero. A contour opening on a control starts on its last point when
/// that is on the curve, else on the midpoint of the two. `None` for malformed contours.
fn commands(o: &Outline26) -> Option<Vec<Command>> {
    let px = |(x, y): (i64, i64)| Point::new(x as f32 / 64.0, y as f32 / 64.0);
    let mid = |a: (i64, i64), b: (i64, i64)| ((a.0 + b.0) / 2, (a.1 + b.1) / 2);
    let mut out = Vec::new();
    let mut first = 0usize;
    for &last in &o.ends {
        if last < first || last >= o.pts.len() {
            return None;
        }
        let (start, walk) = match (o.on[first], o.on[last]) {
            (true, _) => (o.pts[first], first + 1..last + 1),
            (false, true) => (o.pts[last], first..last),
            (false, false) => (mid(o.pts[first], o.pts[last]), first..last + 1),
        };
        out.push(Command::MoveTo(px(start)));
        let mut ctrl = None;
        for i in walk {
            let p = o.pts[i];
            if o.on[i] {
                out.push(match ctrl.take() {
                    Some(c) => Command::QuadTo(px(c), px(p)),
                    None => Command::LineTo(px(p)),
                });
            } else {
                if let Some(c) = ctrl {
                    out.push(Command::QuadTo(px(c), px(mid(c, p))));
                }
                ctrl = Some(p);
            }
        }
        out.push(match ctrl {
            Some(c) => Command::QuadTo(px(c), px(start)),
            None => Command::LineTo(px(start)),
        });
        out.push(Command::Close);
        first = last + 1;
    }
    Some(out)
}
