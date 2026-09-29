//! The reference's software single-precision arithmetic, which `GetEffectPoints 0x6e3800` and its
//! modifier applier `0x6e6c30` run on instead of the FPU: an int-to-float (`0x761260`), an add
//! (`0x760e20`) and a multiply (`0x760be0`) that truncate where IEEE rounds, and a floor
//! (`0x760fb0`) and ceil (`0x761040`). Values are `f32` bit patterns throughout.

/// `0x761260`: an integer's float; exact below 2^24, the low bits shifted out above it.
fn from_int(v: i32) -> u32 {
    if v == 0 {
        return 0;
    }
    let sign = if v < 0 { 0x8000_0000 } else { 0 };
    let a = v.unsigned_abs();
    let lz = a.leading_zeros() as i32;
    let shift = lz - 8;
    let mant = if shift >= 0 { a << shift } else { a >> -shift };
    let exp = (31 - lz + 0x7f) as u32;
    sign | exp << 23 | mant & 0x7f_ffff
}

/// `0x760be0`: multiply the 24-bit significands and keep the high word, truncating. A power of
/// two on either side (a zero fraction) adds exponents and keeps the other fraction exactly; a
/// zero exponent on either side, or an exponent sum at or below zero, is 0.
fn mul(a: u32, b: u32) -> u32 {
    let sign = (a ^ b) & 0x8000_0000;
    let (ae, be) = (a & 0x7f80_0000, b & 0x7f80_0000);
    let (af, bf) = (a & 0x7f_ffff, b & 0x7f_ffff);
    let exp = be.wrapping_add(ae).wrapping_sub(0x3f80_0000);
    let underflow = (exp.wrapping_add(0xff80_0000) as i32) < 0;
    if af == 0 || bf == 0 {
        if ae == 0 || be == 0 || underflow {
            return 0;
        }
        return exp | bf | af | sign;
    }
    let product = u64::from((af | 0x80_0000) << 8) * u64::from((bf | 0x80_0000) << 8);
    let mut hi = (product >> 32) as u32;
    let carry = hi >> 31;
    hi >>= carry;
    if underflow {
        return 0;
    }
    (hi >> 7) & 0x7f_ffff | (carry << 23).wrapping_add(exp) | sign
}

/// `0x760e20`: align the two-guard-bit significands by an arithmetic shift, add, renormalise by
/// shifting out, truncating. A zero exponent returns the other operand; an exponent gap of 23 or
/// more returns the larger.
fn add(a: u32, b: u32) -> u32 {
    let ae = a & 0x7f80_0000;
    if ae == 0 {
        return b;
    }
    let be = b & 0x7f80_0000;
    if be == 0 {
        return a;
    }
    let significand = |x: u32| {
        let m = ((x & 0x7f_ffff | 0x80_0000) << 1) as i32;
        if x & 0x8000_0000 != 0 {
            -m
        } else {
            m
        }
    };
    let (mut sa, mut sb) = (significand(a), significand(b));
    let gap = be.wrapping_sub(ae) as i32;
    let exp = if gap > 0 {
        if gap >= 0xb80_0000 {
            return b;
        }
        sa >>= gap >> 23;
        be
    } else {
        if gap <= -0xb80_0000 {
            return a;
        }
        sb >>= (ae - be) >> 23;
        ae
    };
    let sum = sa.wrapping_add(sb);
    if sum == 0 {
        return 0;
    }
    let sign = sum as u32 & 0x8000_0000;
    let mag = sum.unsigned_abs();
    let shift = 31 - mag.leading_zeros() as i32 - 23;
    let mant = if shift >= 0 {
        mag >> shift
    } else {
        mag << -shift
    };
    (((shift - 1) << 23) as u32).wrapping_add(exp) | sign | mant & 0x7f_ffff
}

/// `0x760fb0`: floor; under one in magnitude a negative nonzero value is -1.0 and anything else
/// +0.0.
fn floor(x: u32) -> u32 {
    let v = f32::from_bits(x);
    if v.abs() < 1.0 {
        return if v < 0.0 { (-1.0f32).to_bits() } else { 0 };
    }
    v.floor().to_bits()
}

/// `0x761040`: ceil; under one in magnitude a negative nonzero value is +0.0 and anything else
/// 1.0, zero included (`76105d`: the sign mask of a zero tests clear).
fn ceil(x: u32) -> u32 {
    let v = f32::from_bits(x);
    if v.abs() < 1.0 {
        return if v < 0.0 { 0 } else { 1.0f32.to_bits() };
    }
    v.ceil().to_bits()
}

/// The applier `0x6e6c30`: `((value + flat) × pct) × 0.01`, each step in the soft arithmetic,
/// the 0.01 the single-precision constant `0xcf0b0c` (`0x3c23d70a`).
pub fn modify(value: f32, flat: i32, pct: i32) -> f32 {
    let sum = add(value.to_bits(), from_int(flat));
    f32::from_bits(mul(mul(sum, from_int(pct)), 0x3c23_d70a))
}

/// An integer's float as `0x761260` builds it.
pub(crate) fn int_to_float(v: i32) -> f32 {
    f32::from_bits(from_int(v))
}

/// `GetEffectPoints`' tail (`6e39ee`-`6e3a65`): to the nearest 1/128, as `floor(x·128 + 0.5) /
/// 128` with the ×128 and ÷128 done on the exponent field and the add and floor in the soft
/// arithmetic (`0x761160`, its 0.5 at `0xcf0b2c`).
pub(crate) fn quantize(x: f32) -> f32 {
    let bits = x.to_bits();
    let scaled = if bits & 0x7f80_0000 != 0 {
        bits.wrapping_add(0x380_0000)
    } else {
        bits
    };
    let rounded = floor(add(scaled, 0.5f32.to_bits()));
    // A zero, either sign, flushes to +0.0; otherwise the exponent drops by 7.
    let shrunk = rounded.wrapping_add(0xfc80_0000);
    let flips = ((rounded.wrapping_sub(0x400_0000) ^ rounded) as i32) < 0;
    f32::from_bits(if flips { 0 } else { shrunk })
}

/// The integral bounds `GetEffectPoints` hands a rounding effect (`6e3a71`-`6e3a89`).
pub(crate) fn floor_ceil(min: f32, max: f32) -> (f32, f32) {
    (
        f32::from_bits(floor(min.to_bits())),
        f32::from_bits(ceil(max.to_bits())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vectors from a bit-level transcription of the routines: the soft results sit one unit in
    /// the last place under the FPU's where the FPU rounds up.
    #[test]
    fn the_applier_truncates_where_the_fpu_rounds() {
        for (value, flat, pct, bits) in [
            (55.0, 0, 125, 0x4289_7fff),
            (93.0, 0, 102, 0x42bd_b851),
            (1000.0, -5, 90, 0x445f_dfff),
            (3.0, 0, 100, 0x403f_ffff),
            (0.0, 5, 110, 0x40af_ffff),
            (-31.0, 0, 125, 0xc21a_ffff),
            (325.0, 0, 115, 0x43ba_dfff),
        ] {
            assert_eq!(
                modify(value, flat, pct).to_bits(),
                bits,
                "({value} + {flat}) × {pct}%"
            );
        }
        let chained = modify(modify(93.0, 0, 102), 0, 101);
        assert_eq!(chained.to_bits(), 0x42bf_9dfe);
        assert_eq!(quantize(chained), 12_263.0 / 128.0);
    }

    #[test]
    fn the_quantizer_rounds_to_128ths_and_flushes_zero() {
        assert_eq!(quantize(f32::from_bits(0x4289_7fff)), 68.75);
        assert_eq!(quantize(0.3), 0.296_875);
        assert_eq!(quantize(-2.3), -2.296_875);
        assert_eq!(quantize(0.0).to_bits(), 0);
        assert_eq!(quantize(-0.0).to_bits(), 0);
        assert_eq!(quantize(0.001), 0.0);
        assert_eq!(quantize(55.0), 55.0);
        assert_eq!(
            f32::from_bits(add((-31.0f32).to_bits(), 0.3f32.to_bits())),
            -30.7
        );
    }

    #[test]
    fn floor_and_ceil_keep_the_reference_quirks() {
        assert_eq!(floor_ceil(4.2, 4.2), (4.0, 5.0));
        assert_eq!(floor_ceil(-0.5, -0.5), (-1.0, 0.0));
        assert_eq!(floor_ceil(0.0, 0.0), (0.0, 1.0), "ceil(+0.0) is 1.0");
        assert_eq!(floor_ceil(7.0, 7.0), (7.0, 7.0));
        assert_eq!(int_to_float(-1_000_000), -1_000_000.0);
    }
}
