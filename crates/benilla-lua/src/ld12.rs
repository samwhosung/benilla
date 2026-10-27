//! The client's MSVC runtime's 12-byte real, in which it converts between text and `double` both
//! ways: six 16-bit words, low first, a 16-bit extension below the 64-bit mantissa and its leading
//! 1, then the sign over a 15-bit exponent biased by 0x3fff. `$I10_OUTPUT` (`cfmt::i10`) scales a
//! value by powers of ten in it to make digits, and `___strgtold12` (`strtod`) to read them.

/// The 12-byte real: `[extension, mantissa low .. mantissa high, sign | exponent]`.
pub(crate) type Ld12 = [u16; 6];

pub(crate) const MASK96: u128 = (1 << 96) - 1;

pub(crate) fn bits96(x: &Ld12) -> u128 {
    x.iter().rev().fold(0, |a, &w| a << 16 | w as u128)
}

pub(crate) fn words96(v: u128) -> Ld12 {
    std::array::from_fn(|i| (v >> (16 * i)) as u16)
}

/// `__ld12mul` (`0x412ef1`): `x *= y`. The 80-bit mantissas multiply by their 16-bit words,
/// only the partial products from the fifth word up kept, each added as 32 bits into the
/// running sum with its carry into the word two up (`0x412fe6`, `0x412ff5`); the sum is
/// normalized and rounded to nearest-even on its low 16 bits (`0x413081`).
pub(crate) fn ld12mul(x: &mut Ld12, y: &Ld12) {
    let sign = (x[5] ^ y[5]) & 0x8000;
    let (ex, ey) = (x[5] & 0x7fff, y[5] & 0x7fff);
    let inf = |x: &mut Ld12| *x = [0, 0, 0, 0, 0x8000, 0x7fff | sign];
    let mut sum = ex as u32 + ey as u32;
    if ex >= 0x7fff || ey >= 0x7fff || sum > 0xbffd {
        return inf(x);
    }
    if sum <= 0x3fbf {
        *x = [0; 6];
        return;
    }
    let zero = |x: &Ld12| x[..5].iter().all(|&w| w == 0) && x[5] & 0x7fff == 0;
    if ex == 0 {
        sum += 1;
        if zero(x) {
            x[5] = 0;
            return;
        }
    }
    if ey == 0 {
        sum += 1;
        if zero(y) {
            *x = [0; 6];
            return;
        }
    }
    let mut t = [0u16; 6];
    for i in 0..5 {
        for j in 0..5 - i {
            let prod = x[i + j] as u32 * y[4 - j] as u32;
            let r = t[i] as u32 | (t[i + 1] as u32) << 16;
            let (s, carry) = r.overflowing_add(prod);
            t[i] = s as u16;
            t[i + 1] = (s >> 16) as u16;
            // The full product is below 2^160, so the top word never carries.
            if carry {
                t[i + 2] = t[i + 2].wrapping_add(1);
            }
        }
    }
    let mut es = (sum as u16).wrapping_add(0xc002);
    let mut m = bits96(&t);
    while (es as i16) > 0 && m >> 95 == 0 {
        m = m << 1 & MASK96;
        es = es.wrapping_sub(1);
    }
    if (es as i16) <= 0 {
        es = es.wrapping_sub(1);
        let mut sticky = false;
        while (es as i16) < 0 {
            sticky |= m & 1 != 0;
            m >>= 1;
            es = es.wrapping_add(1);
        }
        if sticky {
            m |= 1;
        }
    }
    let mut t = words96(m);
    if t[0] > 0x8000 || (m as u32) & 0x1ffff == 0x18000 {
        let top = bits96(&t) >> 16;
        if top == (1 << 80) - 1 {
            t[1..].copy_from_slice(&[0, 0, 0, 0, 0x8000]);
            es = es.wrapping_add(1);
        } else {
            let up = words96((top + 1) << 16);
            t[1..].copy_from_slice(&up[1..]);
        }
    }
    if es >= 0x7fff {
        return inf(x);
    }
    *x = [t[1], t[2], t[3], t[4], t[5], es | sign];
}

/// `__multtenpow12` (`0x413111`): `x *= 10^pow`, by the octal digits of `pow` from the low
/// one, each through its row of the tables at `0x832490` and `0x8325f0`. A row holds the power
/// with a 64-bit mantissa rounded to nearest and the next 16 bits rounded too, and a row whose
/// extension is half or more is first taken one down in its mantissa (`0x413170`): the factor
/// is 10^n rounded to nearest at 80 bits. `TEN_POS` and `TEN_NEG` are the exe's rows at
/// `0x832490` and `0x8325f0` with the `0x413170` adjustment already applied, so none is adjusted
/// here. With `mult12` clear the extension of `x` is cleared first (`0x413139`), unless `pow` is 0.
pub(crate) fn multtenpow12(x: &mut Ld12, pow: i32, mult12: bool) {
    if pow == 0 {
        return;
    }
    let table = if pow < 0 { &TEN_NEG } else { &TEN_POS };
    if !mult12 {
        x[0] = 0;
    }
    let mut pow = pow.unsigned_abs();
    let mut row = 0;
    while pow != 0 {
        let digit = (pow & 7) as usize;
        pow >>= 3;
        if digit != 0 {
            ld12mul(x, &table[row * 7 + digit - 1]);
        }
        row += 1;
    }
}

const TEN_POS: [[u16; 6]; 29] = [
    [0x0000, 0x0000, 0x0000, 0x0000, 0xa000, 0x4002], // 10^1
    [0x0000, 0x0000, 0x0000, 0x0000, 0xc800, 0x4005], // 10^2
    [0x0000, 0x0000, 0x0000, 0x0000, 0xfa00, 0x4008], // 10^3
    [0x0000, 0x0000, 0x0000, 0x0000, 0x9c40, 0x400c], // 10^4
    [0x0000, 0x0000, 0x0000, 0x0000, 0xc350, 0x400f], // 10^5
    [0x0000, 0x0000, 0x0000, 0x0000, 0xf424, 0x4012], // 10^6
    [0x0000, 0x0000, 0x0000, 0x8000, 0x9896, 0x4016], // 10^7
    [0x0000, 0x0000, 0x0000, 0x2000, 0xbebc, 0x4019], // 10^8
    [0x0000, 0x0000, 0x0400, 0xc9bf, 0x8e1b, 0x4034], // 10^16
    [0x0000, 0xa100, 0xcced, 0x1bce, 0xd3c2, 0x404e], // 10^24
    [0xf020, 0xb59d, 0x2b70, 0xada8, 0x9dc5, 0x4069], // 10^32
    [0x5dd0, 0x25fd, 0x1ae5, 0x4f8e, 0xeb19, 0x4083], // 10^40
    [0x9671, 0x95d6, 0x0e43, 0x8d05, 0xaf29, 0x409e], // 10^48
    [0xbff9, 0x449f, 0x81ed, 0x8f12, 0x8281, 0x40b9], // 10^56
    [0x3cbf, 0xa6d5, 0xffcf, 0x1f49, 0xc278, 0x40d3], // 10^64
    [0xc66f, 0x8cdf, 0x80e9, 0x47c9, 0x93ba, 0x41a8], // 10^128
    [0x85bc, 0x556a, 0x3927, 0xf78d, 0xe070, 0x427c], // 10^192
    [0xddbc, 0xde8d, 0x9df9, 0xebfb, 0xaa7e, 0x4351], // 10^256
    [0xe6a1, 0xe375, 0xf2cc, 0x2f29, 0x8184, 0x4426], // 10^320
    [0x1028, 0xaa17, 0xaef8, 0xe310, 0xc4c5, 0x44fa], // 10^384
    [0xa7eb, 0xf3d3, 0xebf7, 0x4ae1, 0x957a, 0x45cf], // 10^448
    [0xcc65, 0x91c6, 0xa60e, 0xa0ae, 0xe319, 0x46a3], // 10^512
    [0x650d, 0x0c17, 0x8175, 0x7586, 0xc976, 0x4d48], // 10^1024
    [0x4258, 0xa7e4, 0x3993, 0x353b, 0xb2b8, 0x53ed], // 10^1536
    [0xa74d, 0x5de4, 0xc53d, 0x3b5d, 0x9e8b, 0x5a92], // 10^2048
    [0x5dff, 0xf0a6, 0x20a1, 0x54c0, 0x8ca5, 0x6137], // 10^2560
    [0xfdd1, 0x5a8a, 0xd88b, 0x5d25, 0xf989, 0x67db], // 10^3072
    [0x95aa, 0xf3f7, 0xbf27, 0xc8a2, 0xdd5d, 0x6e80], // 10^3584
    [0xc94c, 0x979a, 0x8a20, 0x5202, 0xc460, 0x7525], // 10^4096
];
const TEN_NEG: [[u16; 6]; 29] = [
    [0xcccd, 0xcccc, 0xcccc, 0xcccc, 0xcccc, 0x3ffb], // 10^-1
    [0x3d71, 0xd70a, 0x70a3, 0x0a3d, 0xa3d7, 0x3ff8], // 10^-2
    [0x645a, 0xdf3b, 0x8d4f, 0x6e97, 0x8312, 0x3ff5], // 10^-3
    [0xd3c3, 0x652b, 0xe219, 0x1758, 0xd1b7, 0x3ff1], // 10^-4
    [0x0fd0, 0x8423, 0x1b47, 0xac47, 0xa7c5, 0x3fee], // 10^-5
    [0xa640, 0x69b5, 0xaf6c, 0xbd05, 0x8637, 0x3feb], // 10^-6
    [0x3d33, 0x42bc, 0xe57a, 0x94d5, 0xd6bf, 0x3fe7], // 10^-7
    [0xfdc2, 0xcefc, 0x8461, 0x7711, 0xabcc, 0x3fe4], // 10^-8
    [0x4c2f, 0xe15b, 0xc44d, 0x94be, 0xe695, 0x3fc9], // 10^-16
    [0xc492, 0x3b52, 0x4475, 0x14cd, 0x9abe, 0x3faf], // 10^-24
    [0x67de, 0x94ba, 0x4539, 0x1ead, 0xcfb1, 0x3f94], // 10^-32
    [0x2324, 0xe2c6, 0xbabc, 0x313b, 0x8b61, 0x3f7a], // 10^-40
    [0x5561, 0xc159, 0xb17e, 0x7c53, 0xbb12, 0x3f5f], // 10^-48
    [0xeed7, 0x8d2e, 0xbe06, 0x8592, 0xfb15, 0x3f44], // 10^-56
    [0x3f24, 0xe9a5, 0xa539, 0xea27, 0xa87f, 0x3f2a], // 10^-64
    [0xac7d, 0xe4a0, 0x64bc, 0x467c, 0xddd0, 0x3e55], // 10^-128
    [0x7b63, 0xcc06, 0x5423, 0x8377, 0x91ff, 0x3d81], // 10^-192
    [0xfa91, 0x1939, 0x637a, 0x4325, 0xc031, 0x3cac], // 10^-256
    [0x8921, 0x38d0, 0x4782, 0xb897, 0xfd00, 0x3bd7], // 10^-320
    [0x88dc, 0x0857, 0xb11b, 0xe3e8, 0xa686, 0x3b03], // 10^-384
    [0x84c6, 0x4244, 0xb607, 0x7599, 0xdb37, 0x3a2e], // 10^-448
    [0x7133, 0xd21c, 0xdb23, 0xee32, 0x9049, 0x395a], // 10^-512
    [0x87a6, 0xc0bd, 0xda57, 0x82a5, 0xa2a6, 0x32b5], // 10^-1024
    [0x68e2, 0x11b2, 0x52a7, 0x449f, 0xb759, 0x2c10], // 10^-1536
    [0x4925, 0x2de4, 0x3436, 0x534f, 0xceae, 0x256b], // 10^-2048
    [0x598f, 0xa404, 0xdec0, 0x7dc2, 0xe8fb, 0x1ec6], // 10^-2560
    [0xe79e, 0x5a87, 0x9157, 0xbf3c, 0x8350, 0x1822], // 10^-3072
    [0x4b4e, 0x6265, 0x83fd, 0xaf8f, 0x9406, 0x117d], // 10^-3584
    [0x2de4, 0x9fde, 0xd2ce, 0x04c8, 0xa6dd, 0x0ad8], // 10^-4096
];
