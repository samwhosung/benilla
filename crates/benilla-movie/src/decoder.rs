//! I- and P-VOP decoding for MPEG-4 Part 2 Simple profile (ISO/IEC 14496-2): the macroblock
//! layer, intra DC and AC prediction, the second (H.263) inverse quantisation method, motion
//! vector prediction and decoding, and half-sample motion compensation with unrestricted vectors.

use crate::bits::BitReader;
use crate::headers::{Vop, VopType};
use crate::idct::idct;
use crate::scan::{ALT_HORIZONTAL, ALT_VERTICAL, ZIGZAG};
use crate::vlc::{
    Tcoef, CBPY, DC_SIZE_CHROMA, DC_SIZE_LUMA, MCBPC_I, MCBPC_P, MVD, STUFFING, TCOEF_INTER,
    TCOEF_INTRA,
};

/// One sample plane. `width` and `height` are the VOL's; the buffer covers whole macroblocks.
pub(crate) struct Plane {
    pub(crate) data: Vec<u8>,
    pub(crate) stride: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

impl Plane {
    fn new(stride: usize, rows: usize, width: usize, height: usize, fill: u8) -> Plane {
        Plane {
            data: vec![fill; stride * rows],
            stride,
            width,
            height,
        }
    }
}

/// A 4:2:0 picture.
pub(crate) struct Picture {
    pub(crate) y: Plane,
    pub(crate) cb: Plane,
    pub(crate) cr: Plane,
}

impl Picture {
    /// Black: what a P-VOP predicts from when no I-VOP came before it.
    fn black(mb_w: usize, mb_h: usize, width: usize, height: usize) -> Picture {
        let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
        Picture {
            y: Plane::new(mb_w * 16, mb_h * 16, width, height, 16),
            cb: Plane::new(mb_w * 8, mb_h * 8, cw, ch, 128),
            cr: Plane::new(mb_w * 8, mb_h * 8, cw, ch, 128),
        }
    }
}

/// A motion vector in half samples.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Mv {
    x: i32,
    y: i32,
}

/// What later macroblocks of the same VOP predict from.
#[derive(Clone, Copy, Default)]
struct MbState {
    intra: bool,
    qp: i32,
    /// One vector per luminance block; all four alike for a one-vector macroblock, zero for an
    /// intra or not-coded one.
    mv: [Mv; 4],
}

/// An intra block's predictors: its reconstructed DC, `F[0][0]`, and its quantised first row
/// (`QF[0][1..8]`) and first column (`QF[1..8][0]`).
#[derive(Clone, Copy, Default)]
struct BlockState {
    dc: i32,
    row: [i32; 7],
    col: [i32; 7],
}

/// `dquant`'s two bits as a quantiser change.
const DQUANT: [i32; 4] = [-1, -2, 1, 2];

/// The running quantiser below which intra DC takes its own VLC, by `intra_dc_vlc_thr`: 0
/// always, 7 never.
const DC_VLC_BELOW: [i32; 8] = [32, 13, 15, 17, 19, 21, 23, 0];

/// The QF level bound: a conforming stream's levels reconstruct inside [-2048, 2047] and so stay
/// far inside it; the clamp keeps a hostile stream's chained AC predictions from overflowing.
const QF_BOUND: i32 = 2048;

/// The nonlinear DC scaler: luminance, then chrominance.
fn dc_scaler(qp: i32, luma: bool) -> i32 {
    match (luma, qp) {
        (_, ..=4) => 8,
        (true, 5..=8) => 2 * qp,
        (true, 9..=24) => qp + 8,
        (true, _) => 2 * qp - 16,
        (false, 5..=24) => (qp + 13) / 2,
        (false, _) => qp - 6,
    }
}

/// The standard's `//`: division rounded to the nearest integer, halves away from zero. `b > 0`.
fn div_round(a: i32, b: i32) -> i32 {
    if a >= 0 {
        (a + b / 2) / b
    } else {
        -((-a + b / 2) / b)
    }
}

/// The second inverse quantisation method: `|F| = (2|QF| + 1) · q`, less one for an
/// even `q`, signed and saturated to [-2048, 2047].
#[inline]
fn dequant(qf: i32, q: i32) -> i32 {
    if qf == 0 {
        return 0;
    }
    let mag = (2 * qf.abs() + 1) * q - ((q & 1) ^ 1);
    if qf < 0 {
        (-mag).max(-2048)
    } else {
        mag.min(2047)
    }
}

/// The median of three.
fn median(a: i32, b: i32, c: i32) -> i32 {
    a.min(b).max(a.max(b).min(c))
}

/// Read one block's coefficient events into `coef` through `scan`, from scan index `start`.
#[inline]
fn read_coefs(
    r: &mut BitReader,
    table: &Tcoef,
    scan: &[u8; 64],
    start: usize,
    coef: &mut [i32; 64],
) -> Result<(), &'static str> {
    let mut i = start;
    loop {
        let c = table.decode(r)?;
        i += c.run as usize;
        let Some(&pos) = scan.get(i) else {
            return Err("coefficient run past the end of the block");
        };
        coef[pos as usize] = c.level;
        i += 1;
        if c.last {
            return Ok(());
        }
    }
}

/// One component of a motion vector difference: `motion_code`, then for `fcode > 1` and a
/// nonzero code an `fcode - 1`-bit residual.
fn read_mvd(r: &mut BitReader, fcode: u32) -> Result<i32, &'static str> {
    let code = i32::from(MVD.decode(r).ok_or("invalid motion vector code")?) - 32;
    if fcode == 1 || code == 0 {
        return Ok(code);
    }
    let r_size = fcode - 1;
    let mag = ((code.abs() - 1) << r_size) + r.read(r_size) as i32 + 1;
    Ok(if code < 0 { -mag } else { mag })
}

/// Predictor plus difference, wrapped into the `fcode`'s range [-32f, 32f - 1].
fn wrap_mv(v: i32, fcode: u32) -> i32 {
    let f = 1 << (fcode - 1);
    if v < -32 * f {
        v + 64 * f
    } else if v > 32 * f - 1 {
        v - 64 * f
    } else {
        v
    }
}

/// A chrominance vector component from the sum of `n` (1 or 4) luminance components: the sum
/// over `2n`, its fraction moved to the nearest half sample by the standard's tables of
/// sixteenths (four vectors) and quarters (one vector).
fn chroma_mv(sum: i32, n: i32) -> i32 {
    const SIXTEENTHS: [i32; 16] = [0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2];
    const QUARTERS: [i32; 4] = [0, 1, 1, 1];
    let a = sum.abs();
    let mag = if n == 4 {
        (a >> 4) * 2 + SIXTEENTHS[(a & 15) as usize]
    } else {
        (a >> 2) * 2 + QUARTERS[(a & 3) as usize]
    };
    if sum < 0 {
        -mag
    } else {
        mag
    }
}

/// Predict an `n`×`n` block at (`x`, `y`) of `dst` from `src` displaced by `mv`: the four
/// half-sample interpolations with `rounding_control`, and a sample outside the reference taken
/// from its nearest edge (unrestricted motion compensation).
fn motion_compensate(
    dst: &mut Plane,
    src: &Plane,
    x: usize,
    y: usize,
    mv: Mv,
    n: usize,
    rounding: bool,
) {
    let sx = x as isize + (mv.x >> 1) as isize;
    let sy = y as isize + (mv.y >> 1) as isize;
    let (fx, fy) = ((mv.x & 1) as usize, (mv.y & 1) as usize);
    let (w, h) = (src.width as isize, src.height as isize);
    let mut edge = [0u8; 17 * 17];
    let (s, ss): (&[u8], usize) =
        if sx >= 0 && sy >= 0 && sx + (n + fx) as isize <= w && sy + (n + fy) as isize <= h {
            (
                &src.data[sy as usize * src.stride + sx as usize..],
                src.stride,
            )
        } else {
            for j in 0..=n {
                let row = (sy + j as isize).clamp(0, h - 1) as usize * src.stride;
                for i in 0..=n {
                    edge[j * 17 + i] = src.data[row + (sx + i as isize).clamp(0, w - 1) as usize];
                }
            }
            (&edge[..], 17)
        };
    let rnd = u32::from(rounding);
    let ds = dst.stride;
    let d = &mut dst.data[y * ds + x..];
    match (fx, fy) {
        (0, 0) => {
            for j in 0..n {
                d[j * ds..j * ds + n].copy_from_slice(&s[j * ss..j * ss + n]);
            }
        }
        (1, 0) => {
            for j in 0..n {
                let (o, a) = (&mut d[j * ds..j * ds + n], &s[j * ss..j * ss + n + 1]);
                for i in 0..n {
                    o[i] = ((u32::from(a[i]) + u32::from(a[i + 1]) + 1 - rnd) >> 1) as u8;
                }
            }
        }
        (0, 1) => {
            for j in 0..n {
                let (a, b) = (&s[j * ss..j * ss + n], &s[(j + 1) * ss..(j + 1) * ss + n]);
                let o = &mut d[j * ds..j * ds + n];
                for i in 0..n {
                    o[i] = ((u32::from(a[i]) + u32::from(b[i]) + 1 - rnd) >> 1) as u8;
                }
            }
        }
        _ => {
            for j in 0..n {
                let a = &s[j * ss..j * ss + n + 1];
                let b = &s[(j + 1) * ss..(j + 1) * ss + n + 1];
                let o = &mut d[j * ds..j * ds + n];
                for i in 0..n {
                    let sum = u32::from(a[i])
                        + u32::from(a[i + 1])
                        + u32::from(b[i])
                        + u32::from(b[i + 1]);
                    o[i] = ((sum + 2 - rnd) >> 2) as u8;
                }
            }
        }
    }
}

/// Write an intra block's samples.
fn put_block(p: &mut Plane, x: usize, y: usize, s: &[i32; 64]) {
    for (j, line) in s.as_chunks::<8>().0.iter().enumerate() {
        let o = &mut p.data[(y + j) * p.stride + x..][..8];
        for (d, &v) in o.iter_mut().zip(line) {
            *d = v.clamp(0, 255) as u8;
        }
    }
}

/// Add an inter block's residual to its prediction.
fn add_block(p: &mut Plane, x: usize, y: usize, s: &[i32; 64]) {
    for (j, line) in s.as_chunks::<8>().0.iter().enumerate() {
        let o = &mut p.data[(y + j) * p.stride + x..][..8];
        for (d, &v) in o.iter_mut().zip(line) {
            *d = (i32::from(*d) + v).clamp(0, 255) as u8;
        }
    }
}

/// The decoder's state across VOPs: the picture being built, the last one decoded, and the
/// per-macroblock and per-block predictors of the VOP in progress.
pub(crate) struct Decoder {
    mb_w: usize,
    mb_h: usize,
    cur: Picture,
    prev: Picture,
    mbs: Vec<MbState>,
    luma: Vec<BlockState>,
    cb: Vec<BlockState>,
    cr: Vec<BlockState>,
}

impl Decoder {
    pub(crate) fn new(width: usize, height: usize) -> Decoder {
        let (mb_w, mb_h) = (width.div_ceil(16), height.div_ceil(16));
        Decoder {
            mb_w,
            mb_h,
            cur: Picture::black(mb_w, mb_h, width, height),
            prev: Picture::black(mb_w, mb_h, width, height),
            mbs: vec![MbState::default(); mb_w * mb_h],
            luma: vec![BlockState::default(); mb_w * mb_h * 4],
            cb: vec![BlockState::default(); mb_w * mb_h],
            cr: vec![BlockState::default(); mb_w * mb_h],
        }
    }

    /// The last picture decoded.
    pub(crate) fn picture(&self) -> &Picture {
        &self.prev
    }

    /// Decode one coded VOP's macroblocks, which follow its header in `r`.
    pub(crate) fn decode_vop(&mut self, r: &mut BitReader, vop: &Vop) -> Result<(), &'static str> {
        let mut qp = vop.quant as i32;
        for mby in 0..self.mb_h {
            for mbx in 0..self.mb_w {
                match vop.kind {
                    VopType::I => {
                        let v = loop {
                            let v = MCBPC_I.decode(r).ok_or("invalid MCBPC")?;
                            if v != STUFFING {
                                break v;
                            }
                        };
                        self.intra_mb(r, mbx, mby, v, &mut qp, vop)?;
                    }
                    VopType::P => {
                        let v = loop {
                            if r.bit() {
                                break None;
                            }
                            let v = MCBPC_P.decode(r).ok_or("invalid MCBPC")?;
                            if v != STUFFING {
                                break Some(v);
                            }
                        };
                        match v {
                            None => self.skipped_mb(mbx, mby, qp),
                            Some(v) if v >> 2 >= 3 => {
                                self.intra_mb(r, mbx, mby, v, &mut qp, vop)?
                            }
                            Some(v) => self.inter_mb(r, mbx, mby, v, &mut qp, vop)?,
                        }
                    }
                }
                if r.overrun() {
                    return Err("VOP data ends inside a macroblock");
                }
            }
        }
        std::mem::swap(&mut self.cur, &mut self.prev);
        Ok(())
    }

    /// A not-coded P macroblock: the reference's co-sited samples, a zero vector.
    fn skipped_mb(&mut self, mbx: usize, mby: usize, qp: i32) {
        self.mbs[mby * self.mb_w + mbx] = MbState {
            intra: false,
            qp,
            mv: [Mv::default(); 4],
        };
        let (x, y) = (mbx * 16, mby * 16);
        motion_compensate(
            &mut self.cur.y,
            &self.prev.y,
            x,
            y,
            Mv::default(),
            16,
            false,
        );
        motion_compensate(
            &mut self.cur.cb,
            &self.prev.cb,
            x / 2,
            y / 2,
            Mv::default(),
            8,
            false,
        );
        motion_compensate(
            &mut self.cur.cr,
            &self.prev.cr,
            x / 2,
            y / 2,
            Mv::default(),
            8,
            false,
        );
    }

    /// An intra macroblock after its MCBPC (`mcbpc` is `mb_type << 2 | cbpc`, type 3 or 4).
    fn intra_mb(
        &mut self,
        r: &mut BitReader,
        mbx: usize,
        mby: usize,
        mcbpc: u16,
        qp: &mut i32,
        vop: &Vop,
    ) -> Result<(), &'static str> {
        let ac_pred = r.bit();
        let cbpy = u32::from(CBPY.decode(r).ok_or("invalid CBPY")?);
        if mcbpc >> 2 == 4 {
            *qp = (*qp + DQUANT[r.read(2) as usize]).clamp(1, 31);
        }
        self.mbs[mby * self.mb_w + mbx] = MbState {
            intra: true,
            qp: *qp,
            mv: [Mv::default(); 4],
        };
        let use_dc_vlc = *qp < DC_VLC_BELOW[vop.intra_dc_vlc_thr as usize];
        let cbp = cbpy << 2 | u32::from(mcbpc & 3);
        for b in 0..6 {
            let coded = cbp >> (5 - b) & 1 != 0;
            self.intra_block(r, mbx, mby, b, coded, ac_pred, *qp, use_dc_vlc)?;
        }
        Ok(())
    }

    /// One intra block: its DC, its coefficients, DC and AC prediction from the left or the
    /// block above, the inverse quantiser and the IDCT.
    fn intra_block(
        &mut self,
        r: &mut BitReader,
        mbx: usize,
        mby: usize,
        b: usize,
        coded: bool,
        ac_pred: bool,
        qp: i32,
        use_dc_vlc: bool,
    ) -> Result<(), &'static str> {
        let luma = b < 4;
        let mb_w = self.mb_w;
        // The block grid of this component, and this block's place in it.
        let (gw, bx, by) = if luma {
            (2 * mb_w, 2 * mbx + (b & 1), 2 * mby + (b >> 1))
        } else {
            (mb_w, mbx, mby)
        };
        let mbs = &self.mbs;
        let mb_of = |x: usize, y: usize| {
            if luma {
                (y / 2) * mb_w + x / 2
            } else {
                y * mb_w + x
            }
        };
        // A neighbour predicts only from inside the VOP and from an intra macroblock.
        let neighbour = |dx: usize, dy: usize| -> Option<(usize, i32)> {
            let (x, y) = (bx.checked_sub(dx)?, by.checked_sub(dy)?);
            let m = mbs[mb_of(x, y)];
            m.intra.then_some((y * gw + x, m.qp))
        };
        let (a, bl, c) = (neighbour(1, 0), neighbour(1, 1), neighbour(0, 1));
        let states = match b {
            0..=3 => &mut self.luma,
            4 => &mut self.cb,
            _ => &mut self.cr,
        };
        let dc_of = |n: Option<(usize, i32)>| n.map_or(1024, |(i, _)| states[i].dc);
        let (fa, fb, fc) = (dc_of(a), dc_of(bl), dc_of(c));
        let from_above = (fa - fb).abs() < (fb - fc).abs();

        let mut coef = [0i32; 64];
        let mut start = 0;
        if use_dc_vlc {
            let sizes = if luma { &DC_SIZE_LUMA } else { &DC_SIZE_CHROMA };
            let size = u32::from(sizes.decode(r).ok_or("invalid dct_dc_size")?);
            if size > 0 {
                let v = r.read(size) as i32;
                coef[0] = if v < 1 << (size - 1) {
                    v - (1 << size) + 1
                } else {
                    v
                };
                if size > 8 && !r.bit() {
                    return Err("missing marker after a DC differential");
                }
            }
            start = 1;
        }
        if coded {
            let scan = match (ac_pred, from_above) {
                (false, _) => &ZIGZAG,
                (true, true) => &ALT_HORIZONTAL,
                (true, false) => &ALT_VERTICAL,
            };
            read_coefs(r, &TCOEF_INTRA, scan, start, &mut coef)?;
        }

        let scaler = dc_scaler(qp, luma);
        coef[0] += div_round(if from_above { fc } else { fa }, scaler);
        if ac_pred {
            // From above, the first row predicts the first row; from the left, the first column.
            match (from_above, c, a) {
                (true, Some((i, qc)), _) => {
                    for (k, &p) in states[i].row.iter().enumerate() {
                        coef[1 + k] += div_round(p * qc, qp);
                    }
                }
                (false, _, Some((i, qa))) => {
                    for (k, &p) in states[i].col.iter().enumerate() {
                        coef[8 * (1 + k)] += div_round(p * qa, qp);
                    }
                }
                _ => {}
            }
        }
        for v in coef.iter_mut() {
            *v = (*v).clamp(-QF_BOUND, QF_BOUND);
        }
        let dc = (coef[0] * scaler).clamp(-2048, 2047);
        let state = &mut states[by * gw + bx];
        state.dc = dc;
        for k in 0..7 {
            state.row[k] = coef[1 + k];
            state.col[k] = coef[8 * (1 + k)];
        }
        for v in coef.iter_mut().skip(1) {
            *v = dequant(*v, qp);
        }
        coef[0] = dc;
        idct(&mut coef);
        let plane = match b {
            0..=3 => &mut self.cur.y,
            4 => &mut self.cur.cb,
            _ => &mut self.cur.cr,
        };
        put_block(plane, bx * 8, by * 8, &coef);
        Ok(())
    }

    /// The candidate predictors of luminance block `blk`: left, above and
    /// above-right, `cur` holding this macroblock's vectors decoded so far.
    fn predict_mv(&self, mbx: usize, mby: usize, blk: usize, cur: &[Mv; 4]) -> Mv {
        let at = |dx: isize, dy: isize, b: usize| -> Option<Mv> {
            let x = mbx.checked_add_signed(dx)?;
            let y = mby.checked_add_signed(dy)?;
            (x < self.mb_w).then(|| self.mbs[y * self.mb_w + x].mv[b])
        };
        let cand = match blk {
            0 => [at(-1, 0, 1), at(0, -1, 2), at(1, -1, 2)],
            1 => [Some(cur[0]), at(0, -1, 3), at(1, -1, 2)],
            2 => [at(-1, 0, 3), Some(cur[0]), Some(cur[1])],
            _ => [Some(cur[2]), Some(cur[0]), Some(cur[1])],
        };
        // A candidate outside the VOP counts as zero, unless it is one of two outside: then both
        // take the third's value, which is the median.
        if let [Some(only), None, None] | [None, Some(only), None] | [None, None, Some(only)] = cand
        {
            return only;
        }
        let [m1, m2, m3] = cand.map(Option::unwrap_or_default);
        Mv {
            x: median(m1.x, m2.x, m3.x),
            y: median(m1.y, m2.y, m3.y),
        }
    }

    /// An inter macroblock after its MCBPC (type 0, 1 or 2).
    fn inter_mb(
        &mut self,
        r: &mut BitReader,
        mbx: usize,
        mby: usize,
        mcbpc: u16,
        qp: &mut i32,
        vop: &Vop,
    ) -> Result<(), &'static str> {
        let mb_type = mcbpc >> 2;
        let cbpy = 15 - u32::from(CBPY.decode(r).ok_or("invalid CBPY")?);
        if mb_type == 1 {
            *qp = (*qp + DQUANT[r.read(2) as usize]).clamp(1, 31);
        }
        let mut mv = [Mv::default(); 4];
        let vectors = if mb_type == 2 { 4 } else { 1 };
        for k in 0..vectors {
            let p = self.predict_mv(mbx, mby, k, &mv);
            let dx = read_mvd(r, vop.fcode)?;
            let dy = read_mvd(r, vop.fcode)?;
            mv[k] = Mv {
                x: wrap_mv(p.x + dx, vop.fcode),
                y: wrap_mv(p.y + dy, vop.fcode),
            };
        }
        if vectors == 1 {
            mv = [mv[0]; 4];
        }
        self.mbs[mby * self.mb_w + mbx] = MbState {
            intra: false,
            qp: *qp,
            mv,
        };

        let (x, y) = (mbx * 16, mby * 16);
        let rnd = vop.rounding;
        let cmv = if vectors == 4 {
            let sx = mv.iter().map(|v| v.x).sum();
            let sy = mv.iter().map(|v| v.y).sum();
            for (k, &v) in mv.iter().enumerate() {
                let (bx, by) = (x + (k & 1) * 8, y + (k >> 1) * 8);
                motion_compensate(&mut self.cur.y, &self.prev.y, bx, by, v, 8, rnd);
            }
            Mv {
                x: chroma_mv(sx, 4),
                y: chroma_mv(sy, 4),
            }
        } else {
            motion_compensate(&mut self.cur.y, &self.prev.y, x, y, mv[0], 16, rnd);
            Mv {
                x: chroma_mv(mv[0].x, 1),
                y: chroma_mv(mv[0].y, 1),
            }
        };
        motion_compensate(&mut self.cur.cb, &self.prev.cb, x / 2, y / 2, cmv, 8, rnd);
        motion_compensate(&mut self.cur.cr, &self.prev.cr, x / 2, y / 2, cmv, 8, rnd);

        let cbp = cbpy << 2 | u32::from(mcbpc & 3);
        for b in 0..6 {
            if cbp >> (5 - b) & 1 == 0 {
                continue;
            }
            let mut coef = [0i32; 64];
            read_coefs(r, &TCOEF_INTER, &ZIGZAG, 0, &mut coef)?;
            for v in coef.iter_mut() {
                *v = dequant(*v, *qp);
            }
            idct(&mut coef);
            let (plane, px, py) = match b {
                0..=3 => (&mut self.cur.y, x + (b & 1) * 8, y + (b >> 1) * 8),
                4 => (&mut self.cur.cb, x / 2, y / 2),
                _ => (&mut self.cur.cr, x / 2, y / 2),
            };
            add_block(plane, px, py, &coef);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_scaler_follows_table_7_1() {
        let luma: Vec<i32> = [1, 4, 5, 8, 9, 24, 25, 31]
            .iter()
            .map(|&q| dc_scaler(q, true))
            .collect();
        assert_eq!(luma, [8, 8, 10, 16, 17, 32, 34, 46]);
        let chroma: Vec<i32> = [1, 4, 5, 24, 25, 31]
            .iter()
            .map(|&q| dc_scaler(q, false))
            .collect();
        assert_eq!(chroma, [8, 8, 9, 18, 19, 25]);
    }

    #[test]
    fn rounding_division_and_the_h263_quantiser() {
        assert_eq!(div_round(5, 2), 3);
        assert_eq!(div_round(-5, 2), -3);
        assert_eq!(div_round(4, 3), 1);
        assert_eq!(div_round(-1024, 8), -128);
        // q odd: (2|QF| + 1)q; q even: one less.
        assert_eq!(dequant(1, 5), 15);
        assert_eq!(dequant(-2, 4), -19);
        assert_eq!(dequant(0, 9), 0);
        assert_eq!(dequant(2047, 31), 2047);
        assert_eq!(dequant(-2047, 31), -2048);
    }

    #[test]
    fn motion_vectors_decode_wrap_and_derive_chroma() {
        // fcode 2: motion code -3 with residual 1 is -((3 - 1) * 2 + 1 + 1) = -6.
        let data = [0b0001_1100, 0, 0, 0, 0, 0, 0, 0];
        let mut r = BitReader::new(&data);
        assert_eq!(read_mvd(&mut r, 2), Ok(-6));
        assert_eq!(wrap_mv(64, 2), -64);
        assert_eq!(wrap_mv(-65, 2), 63);
        assert_eq!(wrap_mv(31, 1), 31);
        // One vector: quarter positions round to the half sample.
        assert_eq!(
            [0, 1, 2, 3, 4, 5, -1, -3, -5].map(|v| chroma_mv(v, 1)),
            [0, 1, 1, 1, 2, 3, -1, -1, -3]
        );
        // Four vectors summed: by sixteenths.
        assert_eq!(
            [0, 2, 3, 13, 14, 16, -14].map(|v| chroma_mv(v, 4)),
            [0, 0, 1, 1, 2, 2, -2]
        );
    }

    #[test]
    fn vector_prediction_takes_the_median_and_the_edge_rules() {
        let mut d = Decoder::new(48, 32);
        let mv = |x, y| Mv { x, y };
        let set = |d: &mut Decoder, i: usize, v: Mv| d.mbs[i].mv = [v; 4];
        // Top row: above and above-right lie outside, so both take the left vector.
        set(&mut d, 0, mv(4, -2));
        assert_eq!(d.predict_mv(1, 0, 0, &[Mv::default(); 4]), mv(4, -2));
        // First column: the left candidate alone is outside and counts as zero.
        set(&mut d, 1, mv(6, 6));
        assert_eq!(d.predict_mv(0, 1, 0, &[Mv::default(); 4]), mv(4, 0));
        // Inside: the median of left (-2, 3), above (1, 5) and above-right (-1, 7).
        set(&mut d, 3, mv(-2, 3));
        set(&mut d, 1, mv(1, 5));
        set(&mut d, 2, mv(-1, 7));
        assert_eq!(d.predict_mv(1, 1, 0, &[Mv::default(); 4]), mv(-1, 5));
        // The last column has no above-right: it is zero, and the median of (2, 2), (-1, 7) and
        // zero is (0, 2).
        set(&mut d, 4, mv(2, 2));
        assert_eq!(d.predict_mv(2, 1, 0, &[Mv::default(); 4]), mv(0, 2));
        // The top-left macroblock has no candidate at all.
        assert_eq!(d.predict_mv(0, 0, 0, &[Mv::default(); 4]), Mv::default());
    }

    #[test]
    fn half_sample_prediction_rounds_by_rounding_control_and_clamps_at_the_edge() {
        let mut src = Plane::new(16, 16, 16, 16, 0);
        for (i, s) in src.data.iter_mut().enumerate() {
            *s = (i % 16 * 2 + i / 16) as u8;
        }
        let mut dst = Plane::new(16, 16, 16, 16, 0);
        // Half right: (A + B + 1 - rounding) / 2 of 2k + j and 2k + 2 + j.
        motion_compensate(&mut dst, &src, 0, 0, Mv { x: 1, y: 0 }, 8, false);
        assert_eq!(dst.data[0], 1);
        motion_compensate(&mut dst, &src, 0, 0, Mv { x: 1, y: 1 }, 8, true);
        // (0 + 2 + 1 + 3 + 2 - 1) / 4
        assert_eq!(dst.data[0], 1);
        // Far outside to the top left: every sample is the corner's.
        motion_compensate(&mut dst, &src, 0, 0, Mv { x: -200, y: -90 }, 8, false);
        assert!(dst.data[..8].iter().all(|&s| s == 0));
        // Far outside to the bottom right: the far corner, 15 * 2 + 15.
        motion_compensate(&mut dst, &src, 8, 8, Mv { x: 300, y: 300 }, 8, false);
        assert_eq!(dst.data[8 * 16 + 8], 45);
    }
}
