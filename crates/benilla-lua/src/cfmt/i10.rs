//! The client's MSVC runtime making the digits of a `double`: `$I10_OUTPUT` (`0x4116fd`), which
//! scales the value to [0.1, 1) in a 12-byte real by `__multtenpow12` (`0x413111`) and
//! `__ld12mul` (`0x412ef1`), then reads 18 digits off it and rounds the 18th away half-up. The
//! arithmetic is its own, so a value whose digits past the 17th are a 5 and zeros rounds the
//! way that arithmetic says, as here.

use crate::ld12::{bits96, ld12mul, multtenpow12, Ld12, MASK96};

/// 0.1 as `$I10_OUTPUT` holds it on its stack (`0x41171e`-`0x41174a`): truncated, not rounded.
const ONE_TENTH: Ld12 = [0xcccc, 0xcccc, 0xcccc, 0xcccc, 0xcccc, 0x3ffb];

/// `$I10_OUTPUT` (`0x4116fd`) for a finite non-zero `a`, as `_fltout2` asks it (17 digits, no
/// flags): the digits, the first `len` of the array without trailing zeros, and the decimal
/// point's place (the value is `0.ddd * 10^decpt`).
pub(super) fn digits(a: f64) -> ([u8; 18], usize, i32) {
    // `___dtold` (`0x40ffcc`): the 80-bit real, a subnormal normalized.
    let bits = a.to_bits();
    let frac = bits & ((1 << 52) - 1);
    let e = ((bits >> 52) & 0x7ff) as u16;
    let (mut man, mut expn) = if e == 0 {
        (frac << 11, 0x3c01u16)
    } else {
        (1 << 63 | frac << 11, e + 0x3c00)
    };
    while man >> 63 == 0 {
        man <<= 1;
        expn -= 1;
    }
    // The estimate of log10, low (`0x411808`-`0x411839`).
    let est = ((expn as u32 >> 8) + (man >> 56) as u32 * 2)
        .wrapping_mul(0x4d)
        .wrapping_add((expn as u32).wrapping_mul(0x4d10))
        .wrapping_sub(0x1343_12f4);
    let mut nexp = ((est as i32) >> 16) as i16;
    let mut x: Ld12 = [
        0,
        man as u16,
        (man >> 16) as u16,
        (man >> 32) as u16,
        (man >> 48) as u16,
        expn,
    ];
    multtenpow12(&mut x, -(nexp as i32), true);
    if x[5] >= 0x3fff {
        nexp += 1;
        ld12mul(&mut x, &ONE_TENTH);
    }
    // To fixed point: the 96 bits with the binary point above the top byte (`0x41188e`).
    let ue = x[5] as i32 - 0x3ffe;
    x[5] = 0;
    let mut f = bits96(&x) << 8 & MASK96;
    if ue < 0 {
        f >>= (-ue) & 0xff;
    }
    // 18 digits, each the top byte after times ten (`0x4118de`). The fraction is below 2^88, so
    // no product leaves the 96 bits and the digits are those of `f * 10^18 / 2^88`, taken here
    // nine at a time.
    const LOW: u128 = (1 << 88) - 1;
    let p = f * 1_000_000_000;
    let mut hi = (p >> 88) as u32;
    let mut lo = (((p & LOW) * 1_000_000_000) >> 88) as u32;
    let mut d = [0u8; 18];
    for k in (0..9).rev() {
        d[k] = b'0' + (hi % 10) as u8;
        hi /= 10;
        d[k + 9] = b'0' + (lo % 10) as u8;
        lo /= 10;
    }
    let mut exp = nexp as i32;
    let mut len = 17;
    if d[17] >= b'5' {
        // Half-up (`0x411930`): nines carry, a carry out of the first digit moves the point.
        while len > 0 && d[len - 1] == b'9' {
            len -= 1;
        }
        if len == 0 {
            d[0] = b'1';
            len = 1;
            exp += 1;
        } else {
            d[len - 1] += 1;
        }
    } else {
        while len > 0 && d[len - 1] == b'0' {
            len -= 1;
        }
        if len == 0 {
            // All 17 zero: `$I10_OUTPUT`'s zero (`0x411978`).
            d[0] = b'0';
            return (d, 1, 0);
        }
    }
    (d, len, exp)
}
