//! C `printf` conversions for `string.format` and the number-to-text of `lua_number2str`. The
//! floating-point ones are the client's own MSVC runtime's (`_cfltcvt` `0x40b7fe`), which prints
//! a value from at most 17 significant digits, rounds by digit text, spells a non-finite value
//! from `1#INF` and its kin, and writes an exponent of three digits at least.

mod i10;

#[derive(Default, Clone, Copy)]
pub struct Spec {
    pub minus: bool,
    pub plus: bool,
    pub space: bool,
    pub alt: bool,
    pub zero: bool,
    pub width: usize,
    pub precision: Option<usize>,
}

impl Spec {
    /// No flag, width or precision: the conversion's own text.
    pub fn is_plain(&self) -> bool {
        !(self.minus || self.plus || self.space || self.alt || self.zero)
            && self.width == 0
            && self.precision.is_none()
    }
}

fn pad(body: String, prefix: &str, spec: &Spec, numeric: bool) -> String {
    let len = prefix.len() + body.len();
    if len >= spec.width {
        if prefix.is_empty() {
            return body;
        }
        return format!("{prefix}{body}");
    }
    let fill = spec.width - len;
    if spec.minus {
        format!("{prefix}{body}{}", " ".repeat(fill))
    } else if spec.zero && numeric {
        format!("{prefix}{}{body}", "0".repeat(fill))
    } else {
        format!("{}{prefix}{body}", " ".repeat(fill))
    }
}

fn sign(neg: bool, spec: &Spec) -> &'static str {
    if neg {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    }
}

/// `%d`/`%i` of a `long`.
pub fn fmt_int(v: i64, spec: &Spec) -> String {
    let mut digits = v.unsigned_abs().to_string();
    let zero_ok = spec.precision.is_none();
    if let Some(p) = spec.precision {
        if p == 0 && v == 0 {
            digits.clear();
        }
        while digits.len() < p {
            digits.insert(0, '0');
        }
    }
    let s = Spec {
        zero: spec.zero && zero_ok,
        ..*spec
    };
    pad(digits, sign(v < 0, spec), &s, true)
}

/// `%o`, `%u`, `%x`, `%X` of an `unsigned long`.
pub fn fmt_uint(v: u64, conv: u8, spec: &Spec) -> String {
    let mut digits = match conv {
        b'o' => format!("{v:o}"),
        b'x' => format!("{v:x}"),
        b'X' => format!("{v:X}"),
        _ => v.to_string(),
    };
    if let Some(p) = spec.precision {
        if p == 0 && v == 0 {
            digits.clear();
        }
        while digits.len() < p {
            digits.insert(0, '0');
        }
    }
    let prefix = match conv {
        b'o' if spec.alt && !digits.starts_with('0') => {
            digits.insert(0, '0');
            ""
        }
        b'x' if spec.alt && v != 0 => "0x",
        b'X' if spec.alt && v != 0 => "0X",
        _ => "",
    };
    let s = Spec {
        zero: spec.zero && spec.precision.is_none(),
        ..*spec
    };
    pad(digits, prefix, &s, true)
}

/// What the client's `_fltout2` (`0x40ff70`) keeps of a `double`: its sign, the decimal point's
/// place, and up to 17 significant digits from `$I10_OUTPUT` (`0x4116fd`), which is `0`, decimal
/// point 0 and no sign for a zero, and `1#INF`, `1#IND`, `1#QNAN` or `1#SNAN` with decimal point 1
/// for a non-finite value (`0x4117aa`-`0x4117f2`).
struct Flt {
    neg: bool,
    decpt: i32,
    man: Vec<u8>,
}

fn fltout(v: f64) -> Flt {
    let bits = v.to_bits();
    let neg = bits >> 63 == 1;
    if !v.is_finite() {
        // The value as the 80-bit `long double` `$I10_OUTPUT` reads: the quiet bit is the
        // mantissa's second.
        let frac = bits & ((1 << 52) - 1);
        let man: &[u8] = if frac == 0 {
            b"1#INF"
        } else if neg && frac == 1 << 51 {
            b"1#IND"
        } else if frac & 1 << 51 != 0 {
            b"1#QNAN"
        } else {
            b"1#SNAN"
        };
        return Flt {
            neg,
            decpt: 1,
            man: man.to_vec(),
        };
    }
    if v == 0.0 {
        return Flt {
            neg: false,
            decpt: 0,
            man: b"0".to_vec(),
        };
    }
    let (man, decpt) = i10::digits(v.abs());
    Flt { neg, decpt, man }
}

/// The client's `_fptostr` (`0x40fef9`): `digits` digits of the mantissa, padded with `0`, and
/// rounded half-up on the next one by its text (`0x40ff3d`), a `#` or a letter of `1#INF` too,
/// so `%.2f` of an infinity is `1.#J`. A carry out of the first digit moves the decimal point.
fn fptostr(f: &mut Flt, digits: i32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(digits.max(0) as usize + 1);
    buf.push(b'0');
    let mut m = f.man.iter().copied();
    for _ in 0..digits.max(0) {
        buf.push(m.next().unwrap_or(b'0'));
    }
    if digits >= 0 && m.next().is_some_and(|c| c >= b'5') {
        let mut k = buf.len() - 1;
        while buf[k] == b'9' {
            buf[k] = b'0';
            k -= 1;
        }
        buf[k] += 1;
    }
    if buf[0] == b'1' {
        f.decpt += 1;
    } else {
        buf.remove(0);
    }
    buf
}

/// The client's `_cftoe2` (`0x40b5ad`): `d.ddd` and an exponent from the template `e+000`
/// (`0x8001e0`), its digits added in, three at least. `sig` is how many digits show: `ndec + 1`
/// for `%e`, `ndec` for `%g`. A zero's exponent stays `+000`.
fn cftoe2(f: &Flt, digits: &[u8], sig: usize, point: bool, caps: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(sig + 8);
    if f.neg {
        out.push(b'-');
    }
    let digit = |i: usize| digits.get(i).copied().unwrap_or(b'0');
    out.push(digit(0));
    if point {
        out.push(b'.');
        out.extend((1..sig).map(digit));
    }
    out.push(if caps { b'E' } else { b'e' });
    let mut exp = if f.man[0] != b'0' { f.decpt - 1 } else { 0 };
    out.push(if exp < 0 { b'-' } else { b'+' });
    exp = exp.abs();
    out.push(b'0' + (exp / 100) as u8);
    out.push(b'0' + (exp / 10 % 10) as u8);
    out.push(b'0' + (exp % 10) as u8);
    out
}

/// The client's `_cftof2` (`0x40b6c4`): the digits with the point after `decpt` of them, a `0`
/// before it when there are none, and zeros after it up to the first digit: all of them for
/// `%g`, at most `ndec` for `%f`.
fn cftof2(f: &Flt, digits: &[u8], ndec: usize, g: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(digits.len() + ndec + 4);
    if f.neg {
        out.push(b'-');
    }
    let int = f.decpt.max(0) as usize;
    if int == 0 {
        out.push(b'0');
    } else {
        out.extend_from_slice(&digits[..int.min(digits.len())]);
    }
    if ndec > 0 {
        out.push(b'.');
        if f.decpt < 0 {
            let zeros = (-f.decpt) as usize;
            let zeros = if g { zeros } else { zeros.min(ndec) };
            out.extend(std::iter::repeat_n(b'0', zeros));
        }
        out.extend_from_slice(&digits[int.min(digits.len())..]);
    }
    out
}

/// `_forcdecpt`: a `.` after the leading digits, for `#` with a precision of 0.
fn force_point(text: &mut Vec<u8>) {
    let mut i = 0;
    if !text.first().is_some_and(|c| c.eq_ignore_ascii_case(&b'e')) {
        i = 1;
        while text.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    text.insert(i.min(text.len()), b'.');
}

/// `_cropzeros`: `%g`'s trailing zeros of the fraction go, and a bare point with them.
fn crop_zeros(text: &mut Vec<u8>) {
    let Some(point) = text.iter().position(|&c| c == b'.') else {
        return;
    };
    let stop = text[point..]
        .iter()
        .position(|&c| c == b'e' || c == b'E')
        .map_or(text.len(), |i| point + i);
    let mut end = stop;
    while text[end - 1] == b'0' {
        end -= 1;
    }
    if end - 1 == point {
        end -= 1;
    }
    text.drain(end..stop);
}

/// `%e`, `%E`, `%f`, `%g`, `%G` of a `double`, as the client's `sprintf` (`0x73fec4`) writes
/// them: `_cfltcvt` (`0x40b7fe`) for the text, then the flags and the width.
pub fn fmt_float(v: f64, conv: u8, spec: &Spec) -> String {
    let caps = conv.is_ascii_uppercase();
    let g = conv.eq_ignore_ascii_case(&b'g');
    let mut prec = spec.precision.unwrap_or(6);
    if g && prec == 0 {
        prec = 1;
    }
    let mut f = fltout(v);
    let mut text = match conv {
        // `_cftoe` (`0x40b54c`).
        b'e' | b'E' => {
            let d = fptostr(&mut f, prec as i32 + 1);
            cftoe2(&f, &d, prec + 1, prec > 0, caps)
        }
        // `_cftof` (`0x40b66f`).
        b'f' => {
            let digits = prec as i32 + f.decpt;
            let d = fptostr(&mut f, digits);
            cftof2(&f, &d, prec, false)
        }
        // `_cftog` (`0x40b76b`): the style by the exponent after rounding (`0x40b7b3`), and a
        // digit a carry added dropped for the fixed one (`0x40b7cd`).
        _ => {
            let before = f.decpt;
            let mut d = fptostr(&mut f, prec as i32);
            let mag = f.decpt - 1;
            if mag < -4 || mag >= prec as i32 {
                cftoe2(&f, &d, prec, true, caps)
            } else {
                if f.decpt > before {
                    d.pop();
                }
                cftof2(&f, &d, prec, true)
            }
        }
    };
    if spec.alt && prec == 0 {
        force_point(&mut text);
    }
    if g && !spec.alt {
        crop_zeros(&mut text);
    }
    let neg = text.first() == Some(&b'-');
    if neg {
        text.remove(0);
    }
    pad(
        String::from_utf8(text).expect("ASCII"),
        sign(neg, spec),
        spec,
        true,
    )
}

/// `%s` with an optional precision.
pub fn fmt_str(s: &[u8], spec: &Spec) -> Vec<u8> {
    let s = match spec.precision {
        Some(p) => &s[..s.len().min(p)],
        None => s,
    };
    let fill = spec.width.saturating_sub(s.len());
    let mut out = Vec::with_capacity(s.len() + fill);
    if !spec.minus {
        out.extend(std::iter::repeat_n(b' ', fill));
    }
    out.extend_from_slice(s);
    if spec.minus {
        out.extend(std::iter::repeat_n(b' ', fill));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(f: &str) -> Spec {
        let mut s = Spec::default();
        let b = f.as_bytes();
        let mut i = 0;
        while i < b.len() && b"-+ #0".contains(&b[i]) {
            match b[i] {
                b'-' => s.minus = true,
                b'+' => s.plus = true,
                b' ' => s.space = true,
                b'#' => s.alt = true,
                _ => s.zero = true,
            }
            i += 1;
        }
        let mut w = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            w = w * 10 + (b[i] - b'0') as usize;
            i += 1;
        }
        s.width = w;
        if i < b.len() && b[i] == b'.' {
            i += 1;
            let mut p = 0;
            while i < b.len() && b[i].is_ascii_digit() {
                p = p * 10 + (b[i] - b'0') as usize;
                i += 1;
            }
            s.precision = Some(p);
        }
        s
    }

    #[test]
    fn printf_shapes() {
        assert_eq!(fmt_int(42, &sp("05")), "00042");
        assert_eq!(fmt_int(-42, &sp("5")), "  -42");
        assert_eq!(fmt_int(-42, &sp("-5")), "-42  ");
        assert_eq!(fmt_int(7, &sp("+.3")), "+007");
        assert_eq!(fmt_uint(255, b'x', &sp("#")), "0xff");
        assert_eq!(fmt_uint(255, b'X', &sp("04")), "00FF");
        // The client's `sprintf` (`0x73fec4`), run on each.
        let cases: &[(f64, &str, u8, &str)] = &[
            (1.23456, ".2", b'f', "1.23"),
            (1234.5, "", b'e', "1.234500e+003"),
            (12345.678, "", b'e', "1.234568e+004"),
            (0.0001, "", b'g', "0.0001"),
            (1e-5, "", b'g', "1e-005"),
            (100000.0, "", b'g', "100000"),
            (1e6, "", b'g', "1e+006"),
            (2.5, "08.3", b'f', "0002.500"),
            (123456.0, "#", b'g', "123456."),
            (1.0, "#.0", b'g', "1."),
            (1.0, "#", b'g', "1.00000"),
            (5.0, "#.0", b'f', "5."),
            (5.0, "#.0", b'e', "5.e+000"),
            (5.0, ".0", b'e', "5e+000"),
            (0.5, ".0", b'e', "5e-001"),
            (1e20, "#.0", b'G', "1.E+020"),
            (1e20, "", b'E', "1.000000E+020"),
            (1e-20, "", b'G', "1E-020"),
            (0.5, "", b'G', "0.5"),
            (999999.5, "", b'g', "1e+006"),
            (9.9999995, "", b'g', "10"),
            (1234567.0, ".3", b'g', "1.23e+006"),
            (100.0, "#.3", b'g', "100."),
            (0.0001234, ".3", b'g', "0.000123"),
            // Half-up by the digit text, where C99 rounds half-even.
            (0.5, ".0", b'f', "1"),
            (2.5, ".0", b'f', "3"),
            (0.25, ".1", b'f', "0.3"),
            (0.125, ".2", b'f', "0.13"),
            (0.0005, ".3", b'f', "0.001"),
            (0.0015, ".3", b'f', "0.002"),
            // An 18th digit of 5 and zeros: `$I10_OUTPUT`'s own arithmetic rounds these two
            // down and up.
            (
                f64::from_bits(0x4317_1eee_124d_37a7),
                ".16",
                b'e',
                "1.6269830806113057e+015",
            ),
            (
                f64::from_bits(0x4319_1f38_6560_ea63),
                ".16",
                b'e',
                "1.7678003743115768e+015",
            ),
            (
                f64::from_bits(0x431b_d9d0_248a_4ae7),
                "",
                b'f',
                "1959828090163897.800000",
            ),
            // 17 significant digits, then zeros.
            (0.1, ".20", b'f', "0.10000000000000001000"),
            (0.1, ".20", b'g', "0.10000000000000001"),
            (0.1, ".16", b'g', "0.1"),
            (1e-300, "", b'f', "0.000000"),
            (0.0, "", b'e', "0.000000e+000"),
            (-0.0, "", b'f', "0.000000"),
            (-0.0, "+.3", b'e', "+0.000e+000"),
            (1.0, " ", b'f', " 1.000000"),
            (f64::from_bits(1), "", b'g', "4.94066e-324"),
            (f64::from_bits(1), "", b'e', "4.940656e-324"),
            (1e-310, "", b'g', "1e-310"),
            // `1#INF` and its kin are digits to the rounding.
            (f64::INFINITY, "", b'f', "1.#INF00"),
            (f64::INFINITY, "", b'e', "1.#INF00e+000"),
            (f64::INFINITY, "", b'g', "1.#INF"),
            (f64::INFINITY, "", b'G', "1.#INF"),
            (f64::INFINITY, "", b'E', "1.#INF00E+000"),
            (f64::INFINITY, ".2", b'f', "1.#J"),
            (f64::INFINITY, ".3", b'f', "1.#IO"),
            (f64::INFINITY, ".0", b'f', "1"),
            (f64::INFINITY, "5.1", b'f', "  1.$"),
            (f64::INFINITY, "010.3", b'f', "000001.#IO"),
            (f64::INFINITY, "+", b'f', "+1.#INF00"),
            (f64::INFINITY, "-10", b'f', "1.#INF00  "),
            (f64::NEG_INFINITY, "", b'f', "-1.#INF00"),
            (f64::NEG_INFINITY, "010", b'g', "-0001.#INF"),
            (f64::from_bits(0xfff8_0000_0000_0000), "", b'f', "-1.#IND00"),
            (
                f64::from_bits(0xfff8_0000_0000_0000),
                "",
                b'e',
                "-1.#IND00e+000",
            ),
            (f64::from_bits(0xfff8_0000_0000_0000), ".2", b'f', "-1.#J"),
            (f64::from_bits(0x7ff8_0000_0000_0000), "", b'f', "1.#QNAN0"),
        ];
        for &(v, flags, conv, want) in cases {
            let got = fmt_float(v, conv, &sp(flags));
            assert_eq!(got, want, "%{flags}{} of {v:e}", conv as char);
        }
        assert_eq!(fmt_str(b"abc", &sp("5")), b"  abc");
        assert_eq!(fmt_str(b"abcdef", &sp(".2")), b"ab");
    }
}
