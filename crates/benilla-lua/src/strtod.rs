//! Text to a number as the client's MSVC runtime reads it (benilla-re `systems/crt/strtod.md`):
//! `___strgtold12` scans a decimal numeral into at most 24 digits and a power of ten,
//! `___mtold12` makes the digits a 12-byte real, `___multtenpow12` scales it, and `__ld12cvt`
//! rounds it to a `double` or straight to a `float`. The arithmetic is the runtime's own, so the
//! answer is not always the one nearest the text: `6e23` reads one below the nearest double.
//! `strtod` (`0x741e39`) is what `luaO_str2d` (`0x6f5900`) reads every numeral and `tonumber`
//! with, and `_atoflt` (`0x40fecc`) what `sscanf`'s `%f` stores.

use crate::ld12::{multtenpow12, Ld12};

/// The digits `___strgtold12` keeps: 25 are stored (`0x411358`), 24 converted (`0x41157a`).
const KEPT: usize = 25;

/// `___strgtold12`'s answer: the 12-byte real, the index after the numeral, and its flags: 1 an
/// exponent below -5200, 2 one above 5200, 4 no digit.
struct Scan {
    ld: Ld12,
    end: usize,
    flags: u32,
}

/// `___strgtold12` (`0x4111f0`) over `s` as a C string, its bytes then NULs, with its last four
/// arguments 0, as `__fltin2` and `_atoflt` call it (`0x74757c`, `0x40fee3`). Blanks, a sign,
/// digits with at most one `.` and a digit on either side, then an exponent after `e`, `E`, `d`
/// or `D` (`0x4112d1`): a sign and digits, held at 5201 once past 5200 (`0x41151e`). Of the
/// significant digits the first 25 are stored; a further integer digit raises the power of ten
/// (`0x41136e`) and a further fraction digit is dropped. With 25 stored, the 24th is raised when
/// it is 5 or more (`0x41157f`), whatever the 25th; trailing zeros are dropped into the power
/// (`0x4115a1`).
fn strgtold12(s: &[u8]) -> Scan {
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let nonzero = |c: u8| (b'1'..=b'9').contains(&c);
    let mark = |c: u8| matches!(c, b'D' | b'E' | b'd' | b'e');
    let mut p = 0;
    while matches!(at(p), b' ' | b'\t' | b'\n' | b'\r') {
        p += 1;
    }
    // Where a numeral that fails after a sign, a lone point or an exponent mark ends: the start,
    // or the mark (`0x411432`, `0x4114d8`).
    let mut saved = 0;
    let mut digits = [0u8; KEPT];
    let (mut len, mut power) = (0, 0i32);
    let (mut found_digit, mut exp_neg, mut exp) = (false, false, 0i32);
    let mut sign = 0u16;
    // The state machine of `jmp [eax*4+0x411691]` (`0x411253`).
    let mut state = 0;
    loop {
        let mut c = at(p);
        p += 1;
        match state {
            // The start (`0x41125a`) and after a sign (`0x411300`).
            0 | 2 => {
                if nonzero(c) {
                    state = 3;
                    p -= 1;
                } else if c == b'.' {
                    state = 5;
                } else if c == b'0' {
                    state = 1;
                } else if state == 0 && matches!(c, b'+' | b'-') {
                    sign = if c == b'-' { 0x8000 } else { 0 };
                    state = 2;
                } else {
                    p = if state == 0 { p - 1 } else { saved };
                    break;
                }
            }
            // After a leading zero (`0x4112a9`).
            1 => {
                found_digit = true;
                if nonzero(c) {
                    state = 3;
                    p -= 1;
                } else if c == b'.' {
                    state = 4;
                } else if c == b'0' {
                } else if mark(c) {
                    state = 6;
                } else {
                    // A sign after the mantissa ends it (`0x411496`), as anything else does.
                    p -= 1;
                    break;
                }
            }
            // Integer digits (`0x41132a`) and fraction digits (`0x411385`).
            3 | 4 => {
                found_digit = true;
                if state == 4 && len == 0 {
                    // Zeros before the first fraction digit (`0x411396`).
                    while c == b'0' {
                        power -= 1;
                        c = at(p);
                        p += 1;
                    }
                }
                while c.is_ascii_digit() {
                    if len < KEPT {
                        digits[len] = c - b'0';
                        len += 1;
                        if state == 4 {
                            power -= 1;
                        }
                    } else if state == 3 {
                        power += 1;
                    }
                    c = at(p);
                    p += 1;
                }
                if state == 3 && c == b'.' {
                    state = 4;
                } else if mark(c) {
                    state = 6;
                } else {
                    p -= 1;
                    break;
                }
            }
            // A point before any digit (`0x4113fc`).
            5 => {
                if c.is_ascii_digit() {
                    state = 4;
                    p -= 1;
                } else {
                    p = saved;
                    break;
                }
            }
            // After the mark (`0x411432`) and after the exponent's sign (`0x41147c`).
            6 | 7 => {
                if state == 6 {
                    saved = p - 2;
                }
                if nonzero(c) {
                    state = 9;
                    p -= 1;
                } else if c == b'0' {
                    state = 8;
                } else if state == 6 && matches!(c, b'+' | b'-') {
                    exp_neg = c == b'-';
                    state = 7;
                } else {
                    p = saved;
                    break;
                }
            }
            // The exponent's zeros (`0x41145b`).
            8 => {
                while c == b'0' {
                    c = at(p);
                    p += 1;
                }
                if nonzero(c) {
                    state = 9;
                    p -= 1;
                } else {
                    p -= 1;
                    break;
                }
            }
            // The exponent's digits (`0x4114e0`).
            _ => {
                while c.is_ascii_digit() {
                    exp = exp * 10 + i32::from(c - b'0');
                    if exp > 5200 {
                        exp = 5201;
                        break;
                    }
                    c = at(p);
                    p += 1;
                }
                while c.is_ascii_digit() {
                    c = at(p);
                    p += 1;
                }
                p -= 1;
                break;
            }
        }
    }
    let end = p;
    let mut ld: Ld12 = [0; 6];
    if !found_digit {
        ld[5] = sign;
        return Scan { ld, end, flags: 4 };
    }
    if len > 24 {
        if digits[23] >= 5 {
            digits[23] += 1;
        }
        len = 24;
        power += 1;
    }
    let mut flags = 0;
    if len > 0 {
        while digits[len - 1] == 0 {
            len -= 1;
            power += 1;
        }
        ld = mtold12(&digits[..len]);
        let e = (if exp_neg { -exp } else { exp }) + power;
        if e > 5200 {
            ld = [0, 0, 0, 0, 0x8000, 0x7fff];
            flags = 2;
        } else if e < -5200 {
            ld = [0; 6];
            flags = 1;
        } else {
            multtenpow12(&mut ld, e, false);
        }
    }
    // The sign last (`0x411679`): a product `___ld12mul` flushed to +0 is a zero of this sign.
    ld[5] |= sign;
    Scan { ld, end, flags }
}

/// `___mtold12` (`0x411129`): the digits, none zero last, as an integer by `x * 10 + d`,
/// normalized until bit 79 is set (`0x41119d`, `0x4111cf`), the exponent 0x404e less the shift.
/// 24 digits are below 2^80, so no step wraps the 96 bits.
fn mtold12(digits: &[u8]) -> Ld12 {
    let v = digits.iter().fold(0u128, |v, &d| v * 10 + u128::from(d));
    let shift = v.leading_zeros() - (128 - 80);
    let m = v << shift;
    std::array::from_fn(|i| {
        if i < 5 {
            (m >> (16 * i)) as u16
        } else {
            0x404e - shift as u16
        }
    })
}

/// A format `__ld12cvt` rounds to, of the six dwords at `0x831928` (`_ld12tod`) and `0x831940`
/// (`_ld12tof`): the maximum and minimum exponents, the precision and the exponent's width.
struct Format {
    max_exp: i32,
    min_exp: i32,
    precision: u32,
    exp_width: u32,
}

const DOUBLE: Format = Format {
    max_exp: 1024,
    min_exp: -1023,
    precision: 53,
    exp_width: 11,
};

const FLOAT: Format = Format {
    max_exp: 128,
    min_exp: -127,
    precision: 24,
    exp_width: 8,
};

/// `__ld12cvt`'s mantissa: 96 bits, word 0 the highest, bit `n` counted from its top.
type Man = [u32; 3];

/// `__RoundMan` (`0x40fb04`): with bit `prec` set and any bit after bit `prec + 1` set, adds 1 at
/// bit `prec - 1`, carrying up; bit `prec + 1` alone does not round, since `__ZeroTail` is asked
/// from `prec + 1` (`0x40fb47`) and its mask covers only the bits after that one (`0x40fa85`).
/// Clears the bits after bit `prec`, and answers the carry out of the top.
fn round_man(m: &mut Man, prec: u32) -> bool {
    let v = u128::from(m[0]) << 64 | u128::from(m[1]) << 32 | u128::from(m[2]);
    let bit = |n: u32| 1u128 << (95 - n);
    let after = bit(prec + 1) - 1;
    let mut r = v & !(bit(prec) - 1);
    let carry = v & bit(prec) != 0 && v & after != 0 && {
        // `__IncMan` (`0x40faae`) at bit `prec - 1`.
        r += bit(prec - 1);
        r >> 96 != 0
    };
    *m = [(r >> 64) as u32, (r >> 32) as u32, r as u32];
    carry
}

/// `__ShrMan` (`0x40fbd2`): the 96 bits shifted right by `n`, zeros in at the top.
fn shr_man(m: &mut Man, n: u32) {
    let v = (u128::from(m[0]) << 64 | u128::from(m[1]) << 32 | u128::from(m[2])) >> n.min(96);
    *m = [(v >> 64) as u32, (v >> 32) as u32, v as u32];
}

/// What `__ld12cvt` answers besides the bits: 1 for an overflow to the infinity, 2 for a result
/// below the normal range, a subnormal or zero.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Range {
    Normal,
    Overflow,
    Under,
}

/// `__ld12cvt` (`0x40fc5f`): the 12-byte real in `f`'s format, as its bits (a float's in the low
/// 32), and where it fell against the format's range. A carry out
/// of the rounding raises the exponent; at or below the minimum the unrounded mantissa is shifted
/// into a subnormal and rounded again (`0x40fd05`), and below the minimum less the precision it is
/// zero (`0x40fcf5`); at the maximum or above it is the infinity (`0x40fd48`).
fn ld12cvt(x: &Ld12, f: &Format) -> (u64, Range) {
    let sign = x[5] & 0x8000 != 0;
    let mut e = i32::from(x[5] & 0x7fff) - 0x3fff;
    let mut m: Man = [
        u32::from(x[3]) | u32::from(x[4]) << 16,
        u32::from(x[1]) | u32::from(x[2]) << 16,
        u32::from(x[0]) << 16,
    ];
    let bias = f.max_exp - 1;
    let (biased, range) = if e == -0x3fff {
        // An exponent field of 0 (`0x40fca4`): zero, below the range unless all of it is.
        let range = if m == [0; 3] {
            Range::Normal
        } else {
            Range::Under
        };
        m = [0; 3];
        (0, range)
    } else {
        let save = m;
        if round_man(&mut m, f.precision) {
            e += 1;
        }
        if e < f.min_exp - f.precision as i32 {
            m = [0; 3];
            (0, Range::Under)
        } else if e <= f.min_exp {
            m = save;
            shr_man(&mut m, (f.min_exp - e) as u32);
            round_man(&mut m, f.precision);
            shr_man(&mut m, f.exp_width + 1);
            (0, Range::Under)
        } else if e >= f.max_exp {
            m = [0x8000_0000, 0, 0];
            shr_man(&mut m, f.exp_width);
            (bias + f.max_exp, Range::Overflow)
        } else {
            m[0] &= 0x7fff_ffff;
            shr_man(&mut m, f.exp_width);
            (bias + e, Range::Normal)
        }
    };
    // `0x40fd8d`: the exponent over the sign and the top of the mantissa.
    let hi = (biased as u32) << (31 - f.exp_width) | u32::from(sign) << 31 | m[0];
    let bits = if f.precision == 53 {
        u64::from(hi) << 32 | u64::from(m[1])
    } else {
        u64::from(hi)
    };
    (bits, range)
}

/// `strtod`'s answer: the `double`, the index its end pointer is set to, and whether it set
/// `errno` to `ERANGE`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strtod {
    pub value: f64,
    pub end: usize,
    pub erange: bool,
}

/// The client's `strtod` (`0x741e39`) over `s` as a C string: blanks, 9 to 13 and space, skipped
/// (`0x741e46`), then `__fltin2` (`0x747560`), `___strgtold12` and `_ld12tod`. No digit is +0
/// with the end at the start (`0x741e9d`, `0x741eaa`). An overflow is the infinity of the sign
/// the first byte after the blanks gives (`0x741eae`), and a result below 2^-1022 is +0 whatever
/// its sign (`0x741ed3`), each with `ERANGE`; a product the 12-byte real flushed to zero stays a
/// zero of the numeral's sign, `errno` untouched.
pub fn strtod(s: &[u8]) -> Strtod {
    let blanks = s
        .iter()
        .take_while(|&&c| c == b' ' || (0x09..=0x0d).contains(&c))
        .count();
    let rest = &s[blanks..];
    let scan = strgtold12(rest);
    if scan.flags & 4 != 0 {
        return Strtod {
            value: 0.0,
            end: 0,
            erange: false,
        };
    }
    let end = blanks + scan.end;
    let (bits, range) = ld12cvt(&scan.ld, &DOUBLE);
    // `__fltin2`'s flags: 0x80 for the scan's flag 2 or an overflow, 0x100 for its flag 1 or a
    // result below the range (`0x7475a6`, `0x7475b6`).
    if scan.flags & 2 != 0 || range == Range::Overflow {
        let inf = if rest.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
        return Strtod {
            value: inf,
            end,
            erange: true,
        };
    }
    if scan.flags & 1 != 0 || range == Range::Under {
        return Strtod {
            value: 0.0,
            end,
            erange: true,
        };
    }
    Strtod {
        value: f64::from_bits(bits),
        end,
        erange: false,
    }
}

/// The client's `_atoflt` (`0x40fecc`), which `sscanf`'s `%f` stores through `_fassign`
/// (`0x40b53b`): `___strgtold12` over `s` as a C string, then `_ld12tof` (`0x40feef`), one rounding
/// from the 12-byte real straight to the `float`. Nothing tests the answer, so an overflow is the
/// infinity and a result below the normal range is `__ld12cvt`'s subnormal.
pub fn atoflt(s: &[u8]) -> f32 {
    let (bits, _) = ld12cvt(&strgtold12(s).ld, &FLOAT);
    f32::from_bits(bits as u32)
}
