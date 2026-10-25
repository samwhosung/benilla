//! The 8×8 inverse DCT of 14496-2 Annex A, separable and in fixed point, to the IEEE 1180-1990
//! accuracy the standard requires of a decoder's IDCT.
//!
//! Each pass multiplies by the orthonormal basis scaled by 2^13. The row pass keeps 8 fraction
//! bits, the column pass (in 64 bits) drops them; a row of zero coefficients is skipped in both,
//! so a block costs 128 multiplies per coded frequency row. Three fraction bits miss the standard's
//! overall mean square error, 0.022 against 0.02.

/// `round(2^13 · c(u) · cos((2x + 1)uπ / 16))`, `c(0) = 1/√8`, else `1/2`; indexed `[u][x]`.
const BASIS: [[i32; 8]; 8] = [
    [2896, 2896, 2896, 2896, 2896, 2896, 2896, 2896],
    [4017, 3406, 2276, 799, -799, -2276, -3406, -4017],
    [3784, 1567, -1567, -3784, -3784, -1567, 1567, 3784],
    [3406, -799, -4017, -2276, 2276, 4017, 799, -3406],
    [2896, -2896, -2896, 2896, 2896, -2896, -2896, 2896],
    [2276, -4017, 799, 3406, -3406, -799, 4017, -2276],
    [1567, -3784, 3784, -1567, -1567, 3784, -3784, 1567],
    [799, -2276, 3406, -4017, 4017, -3406, 2276, -799],
];

const FRACTION: u32 = 8;
const ROW_SHIFT: u32 = 13 - FRACTION;
const COL_SHIFT: u32 = 13 + FRACTION;

/// Transform `block` (coefficients `[v * 8 + u]`, each in [-2048, 2047]) into its samples
/// `[y * 8 + x]`, unclipped. A row sum stays under 2^13 · 2048 · 3.9 and its output under
/// 2048 · 3.9 · 2^8, both inside `i32`; the column sums take `i64`.
pub(crate) fn idct(block: &mut [i32; 64]) {
    let mut rows = [[0i32; 8]; 8];
    let mut live = [false; 8];
    for v in 0..8 {
        let f = &block[v * 8..v * 8 + 8];
        if f.iter().all(|&c| c == 0) {
            continue;
        }
        live[v] = true;
        let out = &mut rows[v];
        for (u, &c) in f.iter().enumerate() {
            if c != 0 {
                for (o, &b) in out.iter_mut().zip(&BASIS[u]) {
                    *o += b * c;
                }
            }
        }
        for o in out.iter_mut() {
            *o = (*o + (1 << (ROW_SHIFT - 1))) >> ROW_SHIFT;
        }
    }
    let mut out = [1i64 << (COL_SHIFT - 1); 64];
    for v in (0..8).filter(|&v| live[v]) {
        let r = &rows[v];
        for (y, line) in out.as_chunks_mut::<8>().0.iter_mut().enumerate() {
            let b = i64::from(BASIS[v][y]);
            for (o, &s) in line.iter_mut().zip(r) {
                *o += b * i64::from(s);
            }
        }
    }
    for (dst, o) in block.iter_mut().zip(out) {
        *dst = (o >> COL_SHIFT) as i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_basis_is_the_rounded_orthonormal_cosine() {
        for (u, row) in BASIS.iter().enumerate() {
            let c = if u == 0 { (1.0f64 / 8.0).sqrt() } else { 0.5 };
            for (x, &b) in row.iter().enumerate() {
                let want = 8192.0
                    * c
                    * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0).cos();
                assert_eq!(b, want.round() as i32, "[{u}][{x}]");
            }
        }
    }

    /// The IEEE 1180-1990 generator: a 32-bit LCG scaled to [-l, h].
    struct Ieee1180Rand(i64);
    impl Ieee1180Rand {
        fn next(&mut self, l: i64, h: i64) -> i64 {
            self.0 = (self.0.wrapping_mul(1103515245).wrapping_add(12345)) & 0xffff_ffff;
            let i = self.0 & 0x7fff_fffe;
            let x = i as f64 / f64::from(0x7fff_ffff) * (l + h + 1) as f64;
            x as i64 - l
        }
    }

    fn basis_f64() -> [[f64; 8]; 8] {
        let mut b = [[0.0; 8]; 8];
        for (u, row) in b.iter_mut().enumerate() {
            let c = if u == 0 { (1.0f64 / 8.0).sqrt() } else { 0.5 };
            for (x, v) in row.iter_mut().enumerate() {
                *v = c * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0).cos();
            }
        }
        b
    }

    /// The separable 2-D transform in double precision: `forward` takes samples to coefficients,
    /// else coefficients to samples.
    fn transform(b: &[[f64; 8]; 8], input: &[f64; 64], forward: bool) -> [f64; 64] {
        let k = |o: usize, i: usize| if forward { b[o][i] } else { b[i][o] };
        let mut tmp = [0.0; 64];
        for r in 0..8 {
            for o in 0..8 {
                tmp[r * 8 + o] = (0..8).map(|i| k(o, i) * input[r * 8 + i]).sum();
            }
        }
        let mut out = [0.0; 64];
        for c in 0..8 {
            for o in 0..8 {
                out[o * 8 + c] = (0..8).map(|i| k(o, i) * tmp[i * 8 + c]).sum();
            }
        }
        out
    }

    /// The IEEE 1180-1990 procedure: random sample blocks in [-l, h], forward-transformed in double
    /// precision, rounded and clipped, then inverse-transformed by the reference (double, rounded,
    /// clipped to [-256, 255]) and by ours; each range is run again with the input negated.
    /// Its limits: peak error 1, per-sample mean square error 0.06 and mean error 0.015, overall
    /// mean square error 0.02 and mean error 0.0015, and an all-zero block maps to zero.
    #[test]
    fn meets_ieee_1180_accuracy() {
        const BLOCKS: usize = 10_000;
        let basis = basis_f64();
        for (l, h) in [(256, 255), (5, 5), (300, 300)] {
            for negate in [false, true] {
                let mut rng = Ieee1180Rand(1);
                let mut err_sum = [0i64; 64];
                let mut err_sq = [0i64; 64];
                let mut peak = 0;
                for _ in 0..BLOCKS {
                    let mut px = [0i64; 64];
                    for p in px.iter_mut() {
                        let v = rng.next(l, h);
                        *p = if negate { -v } else { v };
                    }
                    let px = px.map(|p| p as f64);
                    let coef =
                        transform(&basis, &px, true).map(|c| (c.round() as i32).clamp(-2048, 2047));
                    let want = transform(&basis, &coef.map(f64::from), false)
                        .map(|s| (s.round() as i32).clamp(-256, 255));
                    let mut got = coef;
                    idct(&mut got);
                    for i in 0..64 {
                        let e = i64::from(got[i].clamp(-256, 255) - want[i]);
                        peak = peak.max(e.abs());
                        err_sum[i] += e;
                        err_sq[i] += e * e;
                    }
                }
                let n = BLOCKS as f64;
                let ctx = format!("range [-{l}, {h}], negated {negate}");
                assert!(peak <= 1, "{ctx}: peak error {peak}");
                for i in 0..64 {
                    let pmse = err_sq[i] as f64 / n;
                    let pme = err_sum[i] as f64 / n;
                    assert!(pmse <= 0.06, "{ctx}: sample {i} mean square error {pmse}");
                    assert!(pme.abs() <= 0.015, "{ctx}: sample {i} mean error {pme}");
                }
                let omse = err_sq.iter().sum::<i64>() as f64 / (64.0 * n);
                let ome = err_sum.iter().sum::<i64>() as f64 / (64.0 * n);
                assert!(omse <= 0.02, "{ctx}: overall mean square error {omse}");
                assert!(ome.abs() <= 0.0015, "{ctx}: overall mean error {ome}");
            }
        }
        let mut zero = [0i32; 64];
        idct(&mut zero);
        assert_eq!(zero, [0; 64]);
    }

    #[test]
    fn a_dc_only_block_is_flat() {
        let mut b = [0i32; 64];
        b[0] = 8 * 100;
        idct(&mut b);
        assert!(b.iter().all(|&s| s == 100), "{b:?}");
    }
}
