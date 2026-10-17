//! Numbers to and from text as the client reads and writes them: its `luaO_str2d` and `strtod`,
//! its 32-bit `strtoul` for `tonumber`, and `lua_number2str`'s `%.14g` in its MSVC runtime's
//! spelling.

use std::fmt::Write;

/// C's `isspace`.
fn is_space(b: u8) -> bool {
    matches!(b, 0x20 | 0x09..=0x0D)
}

/// The 1.12.1 client's `luaO_str2d` (`0x6f5900`): its `strtod` (`0x741e39`) and a trailing-space
/// skip, up to the C string's end at its first NUL. That `strtod` reads a decimal numeral only (no
/// hexadecimal, `inf` or `nan`), and its exponent marker is `e`, `E`, `d` or `D` (`0x4112d1`).
pub fn str2d(s: &[u8]) -> Option<f64> {
    let s = &s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())];
    let end = decimal_end(s)?;
    if !s[end..].iter().all(|&b| is_space(b)) {
        return None;
    }
    let mut num = &s[..end];
    while is_space(num[0]) {
        num = &num[1..];
    }
    let neg = num[0] == b'-';
    if matches!(num[0], b'+' | b'-') {
        num = &num[1..];
    }
    let mark = num
        .iter()
        .position(|b| matches!(b, b'e' | b'E' | b'd' | b'D'))
        .unwrap_or(num.len());
    let (mant, exp) = num.split_at(mark);
    // The exponent, saturated far past any double's range.
    let mut e: i64 = 0;
    if let Some(&sign) = exp.get(1) {
        for &c in exp[1..].iter().filter(|c| c.is_ascii_digit()) {
            e = (e * 10 + (c - b'0') as i64).min(1 << 40);
        }
        if sign == b'-' {
            e = -e;
        }
    }
    let int_len = mant.iter().position(|&b| b == b'.').unwrap_or(mant.len());
    let mut digits = mant.iter().copied().filter(u8::is_ascii_digit).peekable();
    let mut lead = 0;
    while digits.next_if_eq(&b'0').is_some() {
        lead += 1;
    }
    let v = if digits.peek().is_none() {
        0.0
    } else {
        // `0.<digits> * 10^scale`, the digits whole and the exponent small: Rust's parser drops
        // exponent digits past 65536 where glibc's `strtod` offsets them against the digits.
        let scale = (int_len as i64 - lead + e).clamp(-1000, 1000);
        let mut text = String::with_capacity(mant.len() + 8);
        text.push_str("0.");
        text.extend(digits.map(char::from));
        let _ = write!(text, "e{scale}");
        text.parse().ok()?
    };
    Some(if neg { -v } else { v })
}

/// The end of the client's `strtod` numeral at the head of `s` (`0x4111f0`): spaces, a sign,
/// digits with at most one `.`, then an exponent marker, a sign and digits only when a digit
/// follows. `None` when it has no digit.
fn decimal_end(s: &[u8]) -> Option<usize> {
    let digit = |i: usize| s.get(i).is_some_and(u8::is_ascii_digit);
    let mut i = 0;
    while s.get(i).is_some_and(|&b| is_space(b)) {
        i += 1;
    }
    if matches!(s.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    let mut digits = 0;
    while digit(i) {
        i += 1;
        digits += 1;
    }
    if s.get(i) == Some(&b'.') {
        i += 1;
        while digit(i) {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    if matches!(s.get(i), Some(b'e' | b'E' | b'd' | b'D')) {
        let mut k = i + 1;
        if matches!(s.get(k), Some(b'+' | b'-')) {
            k += 1;
        }
        if digit(k) {
            while digit(k) {
                k += 1;
            }
            i = k;
        }
    }
    Some(i)
}

/// C's `ldexp` (`scalbn`): `x * 2^n` with one rounding, in steps that neither overflow nor
/// underflow early.
pub fn ldexp(x: f64, n: i32) -> f64 {
    let two = |e: i32| f64::from_bits(((0x3ff + e) as u64) << 52);
    let (mut y, mut n) = (x, n);
    if n > 1023 {
        y *= two(1023);
        n -= 1023;
        if n > 1023 {
            y *= two(1023);
            n -= 1023;
            n = n.min(1023);
        }
    } else if n < -1022 {
        // Two steps that keep the last one's result above the subnormal range, so it rounds once.
        y *= two(-1022) * two(53);
        n += 1022 - 53;
        if n < -1022 {
            y *= two(-1022) * two(53);
            n += 1022 - 53;
            n = n.max(-1022);
        }
    }
    y * two(n)
}

/// The client's `strtoul` (`0x40dd0e`), MSVC's `_strtoxl` (`0x40db09`) with its 32-bit
/// `unsigned long`, for a base from 2 to 36: the value and the index after the last digit, that
/// index `0` when no digit was read. Spaces and one sign lead; base 16 skips a `0x` or `0X`
/// whether or not a digit follows (`0x40dbb7`); a digit is `0`-`9` or a letter, up to the base.
/// An overflow keeps reading and answers `0xffffffff` (`0x40dccc`), and a `-` then negates
/// modulo 2^32 (`0x40dcf4`), so `-fffffffff` is 1.
pub fn strtoul(s: &[u8], base: u32) -> (u32, usize) {
    debug_assert!((2..=36).contains(&base));
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    let neg = s.get(i) == Some(&b'-');
    if matches!(s.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    if base == 16 && s.get(i) == Some(&b'0') && matches!(s.get(i + 1), Some(b'x' | b'X')) {
        i += 2;
    }
    let at = i;
    let (mut n, mut over) = (0u32, false);
    while let Some(d) = s.get(i).and_then(|&c| (c as char).to_digit(36)) {
        if d >= base {
            break;
        }
        match n.checked_mul(base).and_then(|x| x.checked_add(d)) {
            Some(x) => n = x,
            None => over = true,
        }
        i += 1;
    }
    if i == at {
        return (0, 0);
    }
    let n = if over { u32::MAX } else { n };
    (if neg { n.wrapping_neg() } else { n }, i)
}

/// The NaN the client's x87 makes of an invalid operation, the real indefinite: the sign set, so
/// it prints `-1.#IND`.
pub(crate) const INDEFINITE: u64 = 0xFFF8_0000_0000_0000;

/// `r = a op b` as the client's x87 answers it: a NaN the operation made from operands that are
/// not NaN is the indefinite, and a NaN operand passes through with its own sign. x86's SSE makes
/// the same indefinite and passes a NaN operand through, so only another host (arm64's default NaN
/// has no sign) pays the test.
#[inline(always)]
pub fn x87_op(r: f64, a: f64, b: f64) -> f64 {
    // Of two NaN operands SSE answers the first, where the x87 answers the one with the larger
    // significand (Intel SDM vol. 1, table 4-7), and of two canonical NaNs the client's `faddp`
    // answers the positive one in emulation: `(0/0) + -(0/0)` and `-(0/0) + 0/0` are both
    // `1.#QNAN` there, and the first is `-1.#IND` on an x86 host. benilla does not follow there: no addon meets two NaNs of opposite
    // sign, and the test would cost every operation on x86.
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    if r.is_nan() {
        return x87_fn(r, a, b);
    }
    let _ = (a, b);
    r
}

/// [`x87_op`] for a C library function of `a` and `b` (`b` 0 for one argument), on every host,
/// when `r` is NaN: a NaN operand is the answer, the positive one of two (the x87's pick of two
/// canonical NaNs), and a NaN made from numbers is the indefinite. A host's libm picks its own
/// sign: macOS's `acos(2)` is positive, its `acos` flips a NaN operand's sign and its `cos` and
/// `fmod` answer a positive NaN for a negative one.
#[cold]
pub fn x87_fn(r: f64, a: f64, b: f64) -> f64 {
    match (a.is_nan(), b.is_nan()) {
        (true, true) if a.is_sign_negative() => b,
        (true, _) => a,
        (false, true) => b,
        (false, false) if r.is_nan() => f64::from_bits(INDEFINITE),
        (false, false) => r,
    }
}

/// `lua_number2str`: the client's `luaV_tostring` (`0x6f7c80`) formats with `%.14g` (`0x871960`)
/// through its MSVC `sprintf` (`0x73fec4`): `1e+020`, `1.#INF`, `-1.#IND` and `0` for `-0`.
pub fn fmt_g14(n: f64) -> String {
    with_g14(n, |b| String::from_utf8(b.to_vec()).expect("ASCII"))
}

/// `f` of [`fmt_g14`]'s text, which lives on the stack.
pub fn with_g14<R>(n: f64, f: impl FnOnce(&[u8]) -> R) -> R {
    // At most a sign, 14 digits, a point and `e-308`.
    let mut b = crate::cfmt::StackBuf::<32>::new();
    write_g14(n, &mut b);
    f(b.as_slice())
}

/// [`fmt_g14`]'s text appended to `out`.
pub fn push_g14(n: f64, out: &mut Vec<u8>) {
    write_g14(n, out);
}

fn write_g14(n: f64, out: &mut impl crate::cfmt::Out) {
    if let Some(mut v) = small_integer(n) {
        if v < 0 {
            out.push(b'-');
            v = -v;
        }
        let mut digits = [0u8; 20];
        let mut i = digits.len();
        while v > 0 {
            i -= 1;
            digits[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        out.extend(&digits[i..]);
    } else {
        let spec = crate::cfmt::Spec {
            precision: Some(14),
            ..Default::default()
        };
        crate::cfmt::write_float(out, n, b'g', &spec);
    }
}

/// A non-zero integer below 10^14, which `%.14g` prints as its digits.
fn small_integer(n: f64) -> Option<i64> {
    (n.fract() == 0.0 && n.abs() < 1e14 && n != 0.0).then_some(n as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client's `%.14g`, as `fmtg_probe` in benilla-re pins it by running its `sprintf`.
    #[test]
    fn g14_is_the_clients() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "0"),
            (3.0, "3"),
            (0.8, "0.8"),
            (-0.5, "-0.5"),
            (0.1, "0.1"),
            (0.1 + 0.2, "0.3"),
            (1.0 / 3.0, "0.33333333333333"),
            (1e13, "10000000000000"),
            (1e14, "1e+014"),
            (1e15, "1e+015"),
            (1e20, "1e+020"),
            (1e100, "1e+100"),
            (1e-4, "0.0001"),
            (1e-5, "1e-005"),
            (1.5e-5, "1.5e-005"),
            (2147483648.0, "2147483648"),
            (1.2345678901234568e17, "1.2345678901235e+017"),
            (1e308, "1e+308"),
            (1e-308, "1e-308"),
            (5e-324, "4.9406564584125e-324"),
            (f64::INFINITY, "1.#INF"),
            (f64::NEG_INFINITY, "-1.#INF"),
            (f64::from_bits(0x7ff8_0000_0000_0000), "1.#QNAN"),
            (f64::from_bits(0xfff8_0000_0000_0000), "-1.#IND"),
            // 17 digits half-up, then 14 half-up: a correctly rounded `%.14g` differs on these.
            (f64::from_bits(0xbfbd_ae46_18cc_a433), "-0.11594045739607"),
            (123456789012345.0, "1.2345678901235e+014"),
        ];
        for &(n, want) in cases {
            assert_eq!(fmt_g14(n), want, "{n:e}");
            let mut out = Vec::new();
            push_g14(n, &mut out);
            assert_eq!(out, want.as_bytes(), "{n:e}");
        }
    }

    #[test]
    fn str2d_is_the_clients_strtod() {
        assert_eq!(str2d(b"10"), Some(10.0));
        assert_eq!(str2d(b"  1.5e3  "), Some(1500.0));
        assert_eq!(str2d(b".5"), Some(0.5));
        assert_eq!(str2d(b"-.5"), Some(-0.5));
        assert_eq!(str2d(b"5."), Some(5.0));
        assert_eq!(str2d(b"+3"), Some(3.0));
        assert_eq!(str2d(b"5.e2"), Some(500.0));
        assert_eq!(str2d(b"1d2"), Some(100.0), "`d` is an exponent marker");
        assert_eq!(str2d(b"-2.5D-1"), Some(-0.25));
        assert_eq!(str2d(b" 1d+1 "), Some(10.0));
        let long = format!("1{}d-70", "0".repeat(70));
        assert_eq!(str2d(long.as_bytes()), Some(1.0));
        for text in [
            &b"0x10"[..],
            b"0X1A",
            b"0x1p4",
            b"-0x2",
            b" 0x10 ",
            b"0x",
            b"inf",
            b"-Infinity",
            b"nan",
            b"1e",
            b"1d",
            b"1f2",
            b"1e+",
            b".",
            b"- 1",
            b"1 2",
            b"",
            b"  ",
            b"\0 1",
        ] {
            assert_eq!(str2d(text), None, "{:?}", String::from_utf8_lossy(text));
        }
        assert_eq!(str2d(b"1e400"), Some(f64::INFINITY));
        assert_eq!(str2d(b"-0"), Some(-0.0));
        assert!(str2d(b"-0").unwrap().is_sign_negative());
        assert_eq!(str2d(b"0.000e99999999999"), Some(0.0));
        // A seven-digit exponent offset by as many digits, which glibc reads exactly.
        let z = "0".repeat(699_999);
        assert_eq!(str2d(format!("0.{z}1e700000").as_bytes()), Some(1.0));
        assert_eq!(str2d(format!("1{z}0e-700000").as_bytes()), Some(1.0));
        assert_eq!(str2d(b"1e-99999999999999999999"), Some(0.0));
        assert_eq!(str2d(b"1\0x"), Some(1.0), "a C string ends at its NUL");
        assert_eq!(str2d(b"12\0 "), Some(12.0));
    }

    /// `ldexp` against the C library the build links.
    #[cfg(target_os = "linux")]
    #[test]
    fn ldexp_is_cs() {
        extern "C" {
            #[link_name = "ldexp"]
            fn c_ldexp(x: f64, e: i32) -> f64;
        }
        let xs = [
            0.5,
            1.0,
            1.5,
            3.0,
            -0.0,
            0.1,
            f64::from_bits(1),
            f64::from_bits(0x3ff << 40) * 1.75,
            1e300,
            1e-300,
            f64::INFINITY,
            f64::MAX,
            0.75,
        ];
        let es = [
            0,
            1,
            -1,
            1024,
            -1074,
            -1075,
            1074,
            2098,
            -2000,
            2000,
            -1022,
            -1023,
            1023,
            -1100,
            -2100,
            i32::MAX,
            i32::MIN,
            53,
            -53,
        ];
        for &x in &xs {
            for &e in &es {
                // SAFETY: a pure libm call.
                let want = unsafe { c_ldexp(x, e) };
                assert_eq!(ldexp(x, e).to_bits(), want.to_bits(), "ldexp({x:e}, {e})");
            }
        }
    }

    /// Against the client's own `strtoul`, run at `0x40dd0e`.
    #[test]
    fn strtoul_is_msvc_32() {
        assert_eq!(strtoul(b"ff", 16), (255, 2));
        assert_eq!(strtoul(b"0x10", 16), (16, 4));
        assert_eq!(strtoul(b"-0x10", 16), (0xffff_fff0, 5));
        assert_eq!(strtoul(b"ffffffff", 16), (u32::MAX, 8));
        assert_eq!(strtoul(b"fffffffff", 16), (u32::MAX, 9));
        assert_eq!(strtoul(b"100000000", 16), (u32::MAX, 9));
        assert_eq!(strtoul(b"4294967296", 10), (u32::MAX, 10));
        assert_eq!(strtoul(b"-1", 16), (u32::MAX, 2));
        assert_eq!(strtoul(b"-ffffffff", 16), (1, 9));
        assert_eq!(strtoul(b"-fffffffff", 16), (1, 10));
        assert_eq!(strtoul(b"zz", 36), (1295, 2));
        assert_eq!(strtoul(b"101", 2), (5, 3));
        assert_eq!(strtoul(b"8", 8), (0, 0));
        assert_eq!(strtoul(b"  +ff", 16), (255, 5));
        assert_eq!(strtoul(b"0x", 16), (0, 0));
        assert_eq!(strtoul(b"0xg", 16), (0, 0));
    }
}
