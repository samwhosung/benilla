//! C `printf` conversions for `string.format` and the number-to-text of `lua_number2str`. The
//! floating-point ones are the client's own MSVC runtime's (`_cfltcvt` `0x40b7fe`), which prints
//! a value from at most 17 significant digits, rounds by digit text, spells a non-finite value
//! from `1#INF` and its kin, and writes an exponent of three digits at least. Each conversion
//! writes straight into its caller's buffer, its digits on the stack.

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

/// Where a conversion writes: the caller's growing buffer, or a [`StackBuf`].
pub trait Out {
    fn len(&self) -> usize;
    fn push(&mut self, b: u8);
    fn extend(&mut self, s: &[u8]);
    /// `n` bytes of `b`.
    fn fill(&mut self, b: u8, n: usize);
    fn truncate(&mut self, len: usize);
    fn bytes_mut(&mut self) -> &mut [u8];
}

impl Out for Vec<u8> {
    fn len(&self) -> usize {
        Vec::len(self)
    }
    fn push(&mut self, b: u8) {
        Vec::push(self, b);
    }
    // An empty write is the common case and costs a call otherwise.
    #[inline]
    fn extend(&mut self, s: &[u8]) {
        if !s.is_empty() {
            self.extend_from_slice(s);
        }
    }
    #[inline]
    fn fill(&mut self, b: u8, n: usize) {
        if n > 0 {
            self.resize(Vec::len(self) + n, b);
        }
    }
    fn truncate(&mut self, len: usize) {
        Vec::truncate(self, len);
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        self
    }
}

/// A text of known bound on the stack, such as `%.14g`'s; writing past `N` bytes panics.
pub struct StackBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StackBuf<N> {
    pub fn new() -> Self {
        StackBuf {
            buf: [0; N],
            len: 0,
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl<const N: usize> Default for StackBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Out for StackBuf<N> {
    fn len(&self) -> usize {
        self.len
    }
    fn push(&mut self, b: u8) {
        self.buf[self.len] = b;
        self.len += 1;
    }
    fn extend(&mut self, s: &[u8]) {
        self.buf[self.len..self.len + s.len()].copy_from_slice(s);
        self.len += s.len();
    }
    fn fill(&mut self, b: u8, n: usize) {
        self.buf[self.len..self.len + n].fill(b);
        self.len += n;
    }
    fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len);
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.len]
    }
}

/// `b` put in at `at`, the bytes after it moved up.
fn insert(out: &mut impl Out, at: usize, b: u8) {
    out.push(b);
    out.bytes_mut()[at..].rotate_right(1);
}

/// The width of the item that starts at `start`, after a `prefix` bytes long: spaces after it for
/// `-`, zeros after the prefix for `0`, or spaces before it. The client's `sprintf` pads a string
/// as it pads a number.
fn pad(out: &mut impl Out, start: usize, prefix: usize, spec: &Spec) {
    let len = out.len() - start;
    if len >= spec.width {
        return;
    }
    let fill = spec.width - len;
    if spec.minus {
        out.fill(b' ', fill);
        return;
    }
    let (at, b) = if spec.zero {
        (start + prefix, b'0')
    } else {
        (start, b' ')
    };
    out.fill(b, fill);
    out.bytes_mut()[at..].rotate_right(fill);
}

fn sign(neg: bool, spec: &Spec) -> &'static [u8] {
    if neg {
        b"-"
    } else if spec.plus {
        b"+"
    } else if spec.space {
        b" "
    } else {
        b""
    }
}

/// The digits of `v` in `base`, at the end of `buf`.
fn digits_of(mut v: u64, base: u64, upper: bool, buf: &mut [u8; 20]) -> &[u8] {
    let set: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = set[(v % base) as usize];
        v /= base;
        if v == 0 {
            return &buf[i..];
        }
    }
}

/// `%d`/`%i` of a `long`.
pub fn write_int(out: &mut impl Out, v: i64, spec: &Spec) {
    let start = out.len();
    let prefix = sign(v < 0, spec);
    out.extend(prefix);
    let mut buf = [0; 20];
    let digits = match spec.precision {
        Some(0) if v == 0 => &[],
        _ => digits_of(v.unsigned_abs(), 10, false, &mut buf),
    };
    let zeros = spec.precision.map_or(0, |p| p.saturating_sub(digits.len()));
    out.fill(b'0', zeros);
    out.extend(digits);
    let s = Spec {
        zero: spec.zero && spec.precision.is_none(),
        ..*spec
    };
    pad(out, start, prefix.len(), &s);
}

/// `%o`, `%u`, `%x`, `%X` of an `unsigned long`, 32 bits in the client's runtime.
pub fn write_uint(out: &mut impl Out, v: u32, conv: u8, spec: &Spec) {
    let start = out.len();
    let mut buf = [0; 20];
    let digits = match spec.precision {
        Some(0) if v == 0 => &[],
        _ => {
            let base = match conv {
                b'o' => 8,
                b'x' | b'X' => 16,
                _ => 10,
            };
            digits_of(v as u64, base, conv == b'X', &mut buf)
        }
    };
    let zeros = spec.precision.map_or(0, |p| p.saturating_sub(digits.len()));
    let prefix: &[u8] = match conv {
        b'x' if spec.alt && v != 0 => b"0x",
        b'X' if spec.alt && v != 0 => b"0X",
        _ => b"",
    };
    out.extend(prefix);
    // `#o`: a leading `0` unless the digits already start with one.
    if conv == b'o' && spec.alt && zeros == 0 && digits.first() != Some(&b'0') {
        out.push(b'0');
    }
    out.fill(b'0', zeros);
    out.extend(digits);
    let s = Spec {
        zero: spec.zero && spec.precision.is_none(),
        ..*spec
    };
    pad(out, start, prefix.len(), &s);
}

/// What the client's `_fltout2` (`0x40ff70`) keeps of a `double`: its sign, the decimal point's
/// place, and up to 17 significant digits from `$I10_OUTPUT` (`0x4116fd`), which is `0`, decimal
/// point 0 and no sign for a zero, and `1#INF`, `1#IND`, `1#QNAN` or `1#SNAN` with decimal point 1
/// for a non-finite value (`0x4117aa`-`0x4117f2`).
struct Flt {
    neg: bool,
    decpt: i32,
    man: [u8; 18],
    len: usize,
}

impl Flt {
    fn man(&self) -> &[u8] {
        &self.man[..self.len]
    }
}

fn fltout(v: f64) -> Flt {
    let bits = v.to_bits();
    let neg = bits >> 63 == 1;
    let text = |neg, decpt, s: &[u8]| {
        let mut man = [0; 18];
        man[..s.len()].copy_from_slice(s);
        Flt {
            neg,
            decpt,
            man,
            len: s.len(),
        }
    };
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
        return text(neg, 1, man);
    }
    if v == 0.0 {
        return text(false, 0, b"0");
    }
    let (man, len, decpt) = i10::digits(v.abs());
    Flt {
        neg,
        decpt,
        man,
        len,
    }
}

/// `_fptostr`'s digits: `buf[lo..hi]`, then zeros up to `total` of them.
struct Rounded {
    buf: [u8; 19],
    lo: usize,
    hi: usize,
    total: usize,
}

impl Rounded {
    fn get(&self, i: usize) -> u8 {
        if i < self.hi - self.lo {
            self.buf[self.lo + i]
        } else {
            b'0'
        }
    }

    /// Digits `from..to`.
    fn put(&self, out: &mut impl Out, from: usize, to: usize) {
        let n = self.hi - self.lo;
        let (a, b) = (from.min(n), to.min(n));
        out.extend(&self.buf[self.lo + a..self.lo + b]);
        out.fill(b'0', (to - from) - (b - a));
    }
}

/// The client's `_fptostr` (`0x40fef9`): `digits` digits of the mantissa, padded with `0`, and
/// rounded half-up on the next one by its text (`0x40ff3d`), a `#` or a letter of `1#INF` too,
/// so `%.2f` of an infinity is `1.#J`. A carry out of the first digit moves the decimal point.
fn fptostr(f: &mut Flt, digits: i32) -> Rounded {
    let n = digits.max(0) as usize;
    let man = f.man();
    let take = n.min(man.len());
    let mut buf = [0; 19];
    buf[0] = b'0';
    buf[1..=take].copy_from_slice(&man[..take]);
    if digits >= 0 && n < man.len() && man[n] >= b'5' {
        let mut k = take;
        while buf[k] == b'9' {
            buf[k] = b'0';
            k -= 1;
        }
        buf[k] += 1;
    }
    if buf[0] == b'1' {
        f.decpt += 1;
        Rounded {
            buf,
            lo: 0,
            hi: take + 1,
            total: n + 1,
        }
    } else {
        Rounded {
            buf,
            lo: 1,
            hi: take + 1,
            total: n,
        }
    }
}

/// The client's `_cftoe2` (`0x40b5ad`): `d.ddd` and an exponent from the template `e+000`
/// (`0x8001e0`), its digits added in, three at least. `sig` is how many digits show: `ndec + 1`
/// for `%e`, `ndec` for `%g`. A zero's exponent stays `+000`.
fn cftoe2(out: &mut impl Out, f: &Flt, d: &Rounded, sig: usize, point: bool, caps: bool) {
    out.push(d.get(0));
    if point {
        out.push(b'.');
        d.put(out, 1, sig.max(1));
    }
    let exp = if f.man[0] != b'0' { f.decpt - 1 } else { 0 };
    let e = exp.unsigned_abs();
    out.extend(&[
        if caps { b'E' } else { b'e' },
        if exp < 0 { b'-' } else { b'+' },
        b'0' + (e / 100) as u8,
        b'0' + (e / 10 % 10) as u8,
        b'0' + (e % 10) as u8,
    ]);
}

/// The client's `_cftof2` (`0x40b6c4`): the digits with the point after `decpt` of them, a `0`
/// before it when there are none, and zeros after it up to the first digit: all of them for
/// `%g`, at most `ndec` for `%f`.
fn cftof2(out: &mut impl Out, f: &Flt, d: &Rounded, ndec: usize, g: bool) {
    let int = (f.decpt.max(0) as usize).min(d.total);
    if f.decpt <= 0 {
        out.push(b'0');
    } else {
        d.put(out, 0, int);
    }
    if ndec > 0 {
        out.push(b'.');
        if f.decpt < 0 {
            let zeros = (-f.decpt) as usize;
            out.fill(b'0', if g { zeros } else { zeros.min(ndec) });
        }
        d.put(out, int, d.total);
    }
}

/// `_forcdecpt`: a `.` after the leading digits of the text at `body`, for `#` with a precision
/// of 0.
fn force_point(out: &mut impl Out, body: usize) {
    let text = &out.bytes_mut()[body..];
    let mut i = 0;
    if !text.first().is_some_and(|c| c.eq_ignore_ascii_case(&b'e')) {
        i = 1;
        while text.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let at = body + i.min(text.len());
    insert(out, at, b'.');
}

/// `_cropzeros`: `%g`'s trailing zeros of the fraction in the text at `body` go, and a bare point
/// with them.
fn crop_zeros(out: &mut impl Out, body: usize) {
    let text = &mut out.bytes_mut()[body..];
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
    let len = text.len();
    text.copy_within(stop.., end);
    out.truncate(body + len - (stop - end));
}

/// `%e`, `%E`, `%f`, `%g`, `%G` of a `double`, as the client's `sprintf` (`0x73fec4`) writes
/// them: `_cfltcvt` (`0x40b7fe`) for the text, then the flags and the width.
pub fn write_float(out: &mut impl Out, v: f64, conv: u8, spec: &Spec) {
    let start = out.len();
    let caps = conv.is_ascii_uppercase();
    let g = conv.eq_ignore_ascii_case(&b'g');
    let mut prec = spec.precision.unwrap_or(6);
    if g && prec == 0 {
        prec = 1;
    }
    let mut f = fltout(v);
    let prefix = sign(f.neg, spec);
    out.extend(prefix);
    let body = out.len();
    match conv {
        // `_cftoe` (`0x40b54c`).
        b'e' | b'E' => {
            let d = fptostr(&mut f, prec as i32 + 1);
            cftoe2(out, &f, &d, prec + 1, prec > 0, caps);
        }
        // `_cftof` (`0x40b66f`).
        b'f' => {
            let digits = prec as i32 + f.decpt;
            let d = fptostr(&mut f, digits);
            cftof2(out, &f, &d, prec, false);
        }
        // `_cftog` (`0x40b76b`): the style by the exponent after rounding (`0x40b7b3`), and a
        // digit a carry added dropped for the fixed one (`0x40b7cd`).
        _ => {
            let before = f.decpt;
            let mut d = fptostr(&mut f, prec as i32);
            let mag = f.decpt - 1;
            if mag < -4 || mag >= prec as i32 {
                cftoe2(out, &f, &d, prec, true, caps);
            } else {
                if f.decpt > before {
                    d.total -= 1;
                    d.hi = d.hi.min(d.lo + d.total);
                }
                cftof2(out, &f, &d, prec, true);
            }
        }
    }
    if spec.alt && prec == 0 {
        force_point(out, body);
    }
    if g && !spec.alt {
        crop_zeros(out, body);
    }
    pad(out, start, prefix.len(), spec);
}

/// [`write_float`] as a string.
pub fn fmt_float(v: f64, conv: u8, spec: &Spec) -> String {
    let mut out = Vec::with_capacity(24);
    write_float(&mut out, v, conv, spec);
    String::from_utf8(out).expect("ASCII")
}

/// `%s` with an optional precision, and `%c`, padded as a number is: zeros on the left for the
/// `0` flag, unless `-` puts spaces on the right.
pub fn write_str(out: &mut impl Out, s: &[u8], spec: &Spec) {
    let s = match spec.precision {
        Some(p) => &s[..s.len().min(p)],
        None => s,
    };
    let start = out.len();
    out.extend(s);
    pad(out, start, 0, spec);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8(out).unwrap()
    }

    fn fmt_int(v: i64, spec: &Spec) -> String {
        text(|o| write_int(o, v, spec))
    }

    fn fmt_uint(v: u32, conv: u8, spec: &Spec) -> String {
        text(|o| write_uint(o, v, conv, spec))
    }

    fn fmt_str(s: &[u8], spec: &Spec) -> Vec<u8> {
        let mut out = Vec::new();
        write_str(&mut out, s, spec);
        out
    }

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
        assert_eq!(fmt_uint(8, b'o', &sp("#")), "010");
        assert_eq!(fmt_uint(0, b'o', &sp("#.0")), "0");
        assert_eq!(fmt_uint(u32::MAX, b'u', &sp("")), "4294967295");
        assert_eq!(fmt_int(i64::MIN, &sp("")), "-9223372036854775808");
    }

    /// An item is written after what its buffer holds, and a stack buffer writes as a vector.
    #[test]
    fn items_write_in_place() {
        let mut out = b"ab".to_vec();
        write_float(&mut out, 1.5, b'g', &sp("+08"));
        write_float(&mut out, 5.0, b'e', &sp("#-9.0"));
        write_float(&mut out, 0.25, b'g', &sp("6"));
        write_int(&mut out, -7, &sp("04"));
        write_str(&mut out, b"x", &sp("-3"));
        assert_eq!(out, b"ab+00001.55.e+000    0.25-007x  ");
        let mut stack = StackBuf::<32>::new();
        write_float(&mut stack, -1.0 / 3.0, b'g', &sp(".14"));
        assert_eq!(stack.as_slice(), b"-0.33333333333333");
    }
}
