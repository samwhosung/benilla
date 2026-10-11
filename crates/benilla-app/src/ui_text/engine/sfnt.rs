//! A glyph's metrics as the reference's FreeType computes them off the raw TrueType tables: the
//! pixel advance its pen steps by, the ink its measure ends a line on, and the negative pair kern
//! its step takes. The reference loads every glyph unscaled-then-scaled with no hinting
//! (`0x5d1300`, `FT_LOAD_NO_HINTING`), so each value is FreeType's fixed-point arithmetic on the
//! font units, which a float shaper only approximates: a 4.0 px advance it reads as 3.996 steps a
//! pixel short.

use std::collections::HashMap;

/// FreeType's `FT_MulFix`: `a·b / 65536`, rounded half away from zero.
fn mul_fix(a: i64, b: i64) -> i64 {
    let s = if (a < 0) != (b < 0) { -1 } else { 1 };
    s * ((a.abs() * b.abs() + 0x8000) >> 16)
}

/// FreeType's `FT_DivFix`: `a·65536 / b`, rounded half away from zero.
fn div_fix(a: i64, b: i64) -> i64 {
    let s = if (a < 0) != (b < 0) { -1 } else { 1 };
    let (a, b) = (a.abs(), b.abs());
    s * (((a << 16) + (b >> 1)) / b)
}

/// One glyph at one pixel size, in device px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GlyphPx {
    /// The advance in whole pixels: FreeType's 26.6 advance shifted down (`0x5d1298`), before the
    /// step law's `+1` (`0x5d12a9`).
    pub(super) advance: f32,
    /// The glyph as its line's last: `ceil(horiBearingX) + bitmap width` (`0x5c6b70` → `0x5cb080`),
    /// before the outline's growth of the bitmap.
    pub(super) ink: f32,
}

/// The tables a glyph's metrics come off, located once per face.
pub(super) struct Sfnt {
    data: Vec<u8>,
    upem: u16,
    n_hmetrics: u16,
    num_glyphs: u16,
    hmtx: usize,
    loca: usize,
    loca_long: bool,
    glyf: usize,
    /// The negative pairs of the first horizontal format-0 `kern` subtable, the one FreeType 2.1
    /// loads; `FT_Get_Kerning` reads it unscaled (mode 2, `0x7ce87e`), and the step keeps only a
    /// negative value (`0x5ca350`–`0x5ca358`).
    kern: HashMap<(u16, u16), i16>,
}

fn be16(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn be16s(d: &[u8], at: usize) -> Option<i16> {
    Some(i16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

impl Sfnt {
    /// Locate the tables; `None` for a font that is not TrueType-outlined.
    pub(super) fn parse(data: Vec<u8>) -> Option<Self> {
        let d = &data[..];
        let num = usize::from(be16(d, 4)?);
        let mut table = HashMap::new();
        for i in 0..num {
            let rec = 12 + 16 * i;
            let tag: [u8; 4] = d.get(rec..rec + 4)?.try_into().ok()?;
            table.insert(
                tag,
                (be32(d, rec + 8)? as usize, be32(d, rec + 12)? as usize),
            );
        }
        let head = table.get(b"head")?.0;
        let maxp = table.get(b"maxp")?.0;
        let hhea = table.get(b"hhea")?.0;
        let kern = table
            .get(b"kern")
            .and_then(|&(at, _)| read_kern(d, at))
            .unwrap_or_default();
        Some(Self {
            upem: be16(d, head + 18)?,
            loca_long: be16s(d, head + 50)? != 0,
            num_glyphs: be16(d, maxp + 4)?,
            n_hmetrics: be16(d, hhea + 34)?,
            hmtx: table.get(b"hmtx")?.0,
            loca: table.get(b"loca")?.0,
            glyf: table.get(b"glyf")?.0,
            kern,
            data,
        })
    }

    /// The step's kern scale `[CGxFont+0x188] = x_ppem / units_per_EM`, narrowed to f32
    /// (`0x5ca142`–`0x5ca151`).
    pub(super) fn kern_scale(&self, ppem: u16) -> f32 {
        f32::from(ppem) / f32::from(self.upem)
    }

    /// The negative kern between two glyphs in font units, 0 for none.
    pub(super) fn neg_kern(&self, left: u16, right: u16) -> i16 {
        self.kern.get(&(left, right)).copied().unwrap_or(0)
    }

    fn hmetric(&self, gid: u16) -> Option<(u16, i16)> {
        let d = &self.data[..];
        if gid < self.n_hmetrics {
            let at = self.hmtx + 4 * usize::from(gid);
            return Some((be16(d, at)?, be16s(d, at + 2)?));
        }
        let last = self.hmtx + 4 * usize::from(self.n_hmetrics.checked_sub(1)?);
        let lsb_at =
            self.hmtx + 4 * usize::from(self.n_hmetrics) + 2 * usize::from(gid - self.n_hmetrics);
        Some((be16(d, last)?, be16s(d, lsb_at)?))
    }

    fn glyph_range(&self, gid: u16) -> Option<(usize, usize)> {
        if gid >= self.num_glyphs {
            return None;
        }
        let d = &self.data[..];
        let i = usize::from(gid);
        let (a, b) = if self.loca_long {
            (
                be32(d, self.loca + 4 * i)? as usize,
                be32(d, self.loca + 4 * i + 4)? as usize,
            )
        } else {
            (
                usize::from(be16(d, self.loca + 2 * i)?) * 2,
                usize::from(be16(d, self.loca + 2 * i + 2)?) * 2,
            )
        };
        Some((self.glyf + a, self.glyf + b))
    }

    /// The glyph header's `xMin` in font units, 0 for an empty glyph, and every outline point at
    /// the scale `xs`, in 26.6. A composite is built as the reference's FreeType builds it
    /// (`0x7d40e6`–`0x7d4246`): each component loaded already scaled, its 2×2 matrix applied to
    /// the scaled points with `FT_MulFix` (`FT_Vector_Transform`, `0x7d0340`; the 2.14 entries as
    /// 16.16), then its offset scaled on its own and, under `ROUND_XY_TO_GRID` (`0x4`), rounded to a
    /// whole pixel, `(x + 32) & −64` (`0x7d4229`–`0x7d423b`), hinted or not. `None` when the glyph
    /// data is malformed.
    fn points(&self, gid: u16, depth: u8, xs: i64, out: &mut Vec<(i64, i64)>) -> Option<i16> {
        let (start, end) = self.glyph_range(gid)?;
        if start == end {
            return Some(0);
        }
        let d = &self.data[..];
        let contours = be16s(d, start)?;
        let x_min = be16s(d, start + 2)?;
        if contours >= 0 {
            let mut fu = Vec::new();
            simple_points(d, start, contours as usize, &mut fu)?;
            out.extend(
                fu.into_iter()
                    .map(|(x, y)| (mul_fix(x, xs), mul_fix(y, xs))),
            );
            return Some(x_min);
        }
        if depth > 8 {
            return None;
        }
        let mut at = start + 10;
        loop {
            let flags = be16(d, at)?;
            let gid = be16(d, at + 2)?;
            at += 4;
            let (a1, a2) = if flags & 0x1 != 0 {
                let v = (i64::from(be16s(d, at)?), i64::from(be16s(d, at + 2)?));
                at += 4;
                v
            } else {
                let v = (
                    i64::from(*d.get(at)? as i8),
                    i64::from(*d.get(at + 1)? as i8),
                );
                at += 2;
                v
            };
            // The 2.14 entries as 16.16: `[xx, yx, xy, yy]` in the file's order.
            let f16 = |v: i16| i64::from(v) << 2;
            let m = if flags & 0x8 != 0 {
                let s = f16(be16s(d, at)?);
                at += 2;
                Some([s, 0, 0, s])
            } else if flags & 0x40 != 0 {
                let m = [f16(be16s(d, at)?), 0, 0, f16(be16s(d, at + 2)?)];
                at += 4;
                Some(m)
            } else if flags & 0x80 != 0 {
                let m = [
                    f16(be16s(d, at)?),
                    f16(be16s(d, at + 2)?),
                    f16(be16s(d, at + 4)?),
                    f16(be16s(d, at + 6)?),
                ];
                at += 8;
                Some(m)
            } else {
                None
            };
            let (mut dx, mut dy) = if flags & 0x2 != 0 {
                (mul_fix(a1, xs), mul_fix(a2, xs))
            } else {
                (0, 0)
            };
            if flags & 0x2 != 0 && flags & 0x4 != 0 {
                (dx, dy) = ((dx + 32) & -64, (dy + 32) & -64);
            }
            let mut sub = Vec::new();
            self.points(gid, depth + 1, xs, &mut sub)?;
            for (x, y) in sub {
                let (tx, ty) = match m {
                    Some(m) => (
                        mul_fix(x, m[0]) + mul_fix(y, m[2]),
                        mul_fix(x, m[1]) + mul_fix(y, m[3]),
                    ),
                    None => (x, y),
                };
                out.push((tx + dx, ty + dy));
            }
            if flags & 0x20 == 0 {
                return Some(x_min);
            }
        }
    }

    /// Glyph `gid` at `ppem` px as FreeType 2.1 sizes it unhinted (`FT_Set_Pixel_Sizes`, then
    /// `FT_Load_Glyph` with `FT_LOAD_NO_HINTING`): the scale `x_scale = FT_DivFix(ppem·64, upem)`,
    /// the phantom points `pp1 = xMin − lsb` and `pp2 = pp1 + advance` scaled by `FT_MulFix`, the
    /// outline scaled the same and moved by `−pp1`. The advance is `(pp2 − pp1) >> 6`; the bitmap
    /// spans the outline's control box rounded out, `ceil(xMax) − floor(xMin)`, and the bearing
    /// is the box's `xMin`. With no ink, the rasterizer makes a `(ppem + 3) >> 2` px blank
    /// (`0x5d11da`).
    pub(super) fn glyph(&self, gid: u16, ppem: u16) -> Option<GlyphPx> {
        let (adv_fu, lsb) = self.hmetric(gid)?;
        let xs = div_fix(i64::from(ppem) * 64, i64::from(self.upem));
        let mut pts = Vec::new();
        let x_min = self.points(gid, 0, xs, &mut pts)?;
        let origin = i64::from(x_min) - i64::from(lsb);
        let pp1 = mul_fix(origin, xs);
        let adv = mul_fix(origin + i64::from(adv_fu), xs) - pp1;
        // `0x5d1292`–`0x5d1298`: `cdq; and edx,0x3f; add; sar 6`, toward zero.
        let advance = (if adv < 0 { -((-adv) >> 6) } else { adv >> 6 }) as f32;
        let blank = f32::from((ppem + 3) >> 2);
        if pts.is_empty() {
            return Some(GlyphPx {
                advance,
                ink: blank,
            });
        }
        let xs_px: Vec<i64> = pts.iter().map(|&(x, _)| x - pp1).collect();
        let ys_px: Vec<i64> = pts.iter().map(|&(_, y)| y).collect();
        let (x0, x1) = (*xs_px.iter().min()?, *xs_px.iter().max()?);
        let (y0, y1) = (*ys_px.iter().min()?, *ys_px.iter().max()?);
        let floor = |v: i64| v.div_euclid(64);
        let ceil = |v: i64| (v + 63).div_euclid(64);
        let (width, rows) = (ceil(x1) - floor(x0), ceil(y1) - floor(y0));
        // `0x5cb080` rounds the 26.6 bearing up (`0x73fdf5`, a ceil); an outline with no extent
        // still keeps its bearing beside the blank bitmap.
        let width = if width == 0 || rows == 0 {
            i64::from((ppem + 3) >> 2)
        } else {
            width
        };
        Some(GlyphPx {
            advance,
            ink: (ceil(x0) + width) as f32,
        })
    }
}

/// A simple glyph's points in font units, appended to `out`.
fn simple_points(d: &[u8], start: usize, contours: usize, out: &mut Vec<(i64, i64)>) -> Option<()> {
    if contours == 0 {
        return Some(());
    }
    let ends_at = start + 10;
    let n = usize::from(be16(d, ends_at + 2 * (contours - 1))?) + 1;
    let ins = usize::from(be16(d, ends_at + 2 * contours)?);
    let mut at = ends_at + 2 * contours + 2 + ins;
    let mut flags = Vec::with_capacity(n);
    while flags.len() < n {
        let f = *d.get(at)?;
        at += 1;
        flags.push(f);
        if f & 0x8 != 0 {
            let r = *d.get(at)?;
            at += 1;
            for _ in 0..r {
                flags.push(f);
            }
        }
    }
    flags.truncate(n);
    let mut coord = |short: u8, same: u8| -> Option<Vec<i64>> {
        let mut v = 0i64;
        let mut out = Vec::with_capacity(n);
        for &f in &flags {
            if f & short != 0 {
                let b = i64::from(*d.get(at)?);
                at += 1;
                v += if f & same != 0 { b } else { -b };
            } else if f & same == 0 {
                v += i64::from(be16s(d, at)?);
                at += 2;
            }
            out.push(v);
        }
        Some(out)
    };
    let xs = coord(0x2, 0x10)?;
    let ys = coord(0x4, 0x20)?;
    out.extend(xs.into_iter().zip(ys));
    Some(())
}

/// The first horizontal format-0 subtable's negative pairs (coverage `0x0001`).
fn read_kern(d: &[u8], at: usize) -> Option<HashMap<(u16, u16), i16>> {
    let n = usize::from(be16(d, at + 2)?);
    let mut sub = at + 4;
    for _ in 0..n {
        let len = usize::from(be16(d, sub + 2)?);
        let coverage = be16(d, sub + 4)?;
        if coverage == 0x0001 {
            let pairs = usize::from(be16(d, sub + 6)?);
            let mut map = HashMap::new();
            for j in 0..pairs {
                let p = sub + 14 + 6 * j;
                let v = be16s(d, p + 4)?;
                if v < 0 {
                    map.insert((be16(d, p)?, be16(d, p + 2)?), v);
                }
            }
            return Some(map);
        }
        sub += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_rounds_half_away_from_zero() {
        assert_eq!(mul_fix(3, 0x8000), 2);
        assert_eq!(mul_fix(-3, 0x8000), -2);
        assert_eq!(div_fix(12 * 64, 1000), 50332);
    }
}
