//! The string library: 5.1's `lstrlib.c`, whose messages the script host carries (the pattern
//! matcher with `%f` and `%b`). A pattern ends at its first NUL, as the C matcher reads a C
//! string.

use crate::api::R;
use crate::cfmt::{self, Spec};
use crate::value::{Value, V};
use crate::vm::{LuaError, Vm};

pub fn open(vm: &mut Vm) {
    // `luaL_openlib` makes a library's table empty, by `lua_newtable` (`0x6f4dff`).
    let t = vm.heap.new_table(0, 0);
    let fns: [(&'static str, crate::heap::NativeFn); 12] = [
        ("len", len),
        ("sub", sub),
        ("lower", lower),
        ("upper", upper),
        ("char", char_),
        ("rep", rep),
        ("byte", byte),
        ("format", format),
        ("dump", dump),
        ("find", find),
        ("gfind", gfind),
        ("gsub", gsub),
    ];
    for (name, f) in fns {
        let v = vm.new_native(name, f);
        let k = vm.str(name.as_bytes());
        vm.heap.table_mut(t).set(k, v).unwrap();
    }
    vm.set_global("string", Value::Table(t));
}

/// 5.1's `posrelat`: a negative position counts back from the end, and one before the start is 0.
fn posrelat(pos: i64, len: usize) -> i64 {
    let pos = if pos < 0 {
        pos.saturating_add(len as i64 + 1)
    } else {
        pos
    };
    pos.max(0)
}

fn len(vm: &mut Vm, nargs: usize) -> R<usize> {
    let s = vm.check_str(nargs, 1)?;
    let n = vm.heap.str(s).len();
    Ok(vm.ret1(Value::Num(n as f64)))
}

fn sub(vm: &mut Vm, nargs: usize) -> R<usize> {
    let s = vm.check_str(nargs, 1)?;
    let l = vm.heap.str(s).len();
    let start = vm.check_integer(nargs, 2)?;
    let end = vm.opt_integer(nargs, 3, -1)?;
    let mut start = posrelat(start, l);
    let mut end = posrelat(end, l);
    if start < 1 {
        start = 1;
    }
    if end > l as i64 {
        end = l as i64;
    }
    if start <= end {
        let (a, e) = (start as usize - 1, end as usize);
        let r = vm
            .heap
            .intern_with(|h, b| b.extend_from_slice(&h.str(s)[a..e]));
        Ok(vm.ret1(Value::Str(r)))
    } else {
        Ok(vm.ret_str(b""))
    }
}

/// The client's `str_lower` (`0x7fbdf0`): each UTF-8 code point through `LowerCodepoint`
/// (`0x7fc0a0`), `A`-`Z` and U+00C0-U+00DE up by 32.
fn lower(vm: &mut Vm, nargs: usize) -> R<usize> {
    case_map(vm, nargs, |c| match c {
        0x41..=0x5a | 0xc0..=0xde => c + 0x20,
        _ => c,
    })
}

/// The client's `str_upper` (`0x7fc0c0`): the same walk through `UpperCodepoint` (`0x7fc180`),
/// `a`-`z` and U+00E0-U+00FE down by 32.
fn upper(vm: &mut Vm, nargs: usize) -> R<usize> {
    case_map(vm, nargs, |c| match c {
        0x61..=0x7a | 0xe0..=0xfe => c - 0x20,
        _ => c,
    })
}

/// `str_lower`'s loop: decode a code point, map it, encode it, advance by the bytes the decoder
/// read (`0x7fbe8a`). A string of ASCII without a NUL maps byte by byte, as the loop would.
///
/// Deviation: the decoder reads a NUL as no bytes (`0x7fbeda`), so the client's loop never
/// passes one and the call never returns; here the result ends there.
fn case_map(vm: &mut Vm, nargs: usize, map: fn(u32) -> u32) -> R<usize> {
    let s = vm.check_str(nargs, 1)?;
    let r = vm.heap.intern_with(|h, b| {
        let src = h.str(s);
        if src.iter().all(|&c| c != 0 && c < 0x80) {
            b.extend(src.iter().map(|&c| map(c as u32) as u8));
            return;
        }
        let mut i = 0;
        while i < src.len() {
            let (c, n) = utf8_decode(src, i);
            if n == 0 {
                break;
            }
            utf8_encode(map(c), b);
            i += n;
        }
    });
    Ok(vm.ret1(Value::Str(r)))
}

/// The client's `Utf8DecodeChar` (`0x7fbeb0`) at `s[i]`: the code point and the bytes read, the
/// string ending in the NUL a C string ends in. A lead byte gives its count of continuations by
/// its masks (`0x7fbef2`-`0x7fbf4e`), up to five, with no check of the value; a byte that leads
/// nothing, or a continuation that is not `10xxxxxx` (`0x7fbf7c`, counted first at `0x7fbf75`),
/// is `0x80000000`; a NUL is `0xffffffff`, its own byte not read.
fn utf8_decode(s: &[u8], i: usize) -> (u32, usize) {
    let at = |k: usize| s.get(k).copied().unwrap_or(0);
    let lead = at(i);
    let (bits, n) = match lead {
        0 => return (u32::MAX, 0),
        _ if lead & 0xfe == 0xfc => (lead & 0x01, 5),
        _ if lead & 0xfc == 0xf8 => (lead & 0x03, 4),
        _ if lead & 0xf8 == 0xf0 => (lead & 0x07, 3),
        _ if lead & 0xf0 == 0xe0 => (lead & 0x0f, 2),
        _ if lead & 0xe0 == 0xc0 => (lead & 0x1f, 1),
        _ if lead & 0x80 != 0 => return (0x8000_0000, 1),
        _ => return (lead as u32, 1),
    };
    let mut c = bits as u32;
    for k in 1..=n {
        let x = at(i + k);
        if x == 0 {
            return (u32::MAX, k);
        }
        if x & 0xc0 != 0x80 {
            return (0x8000_0000, k + 1);
        }
        c = c << 6 | (x & 0x3f) as u32;
    }
    (c, n + 1)
}

/// The client's `Utf8EncodeChar` (`0x7fbfb0`), by its limits (`0x7fbfb6`-`0x7fc05a`): one byte
/// below 0x80, two below 0x800, three below 0x10000, four below 0x200000, five below 0x400000,
/// six below 0x80000000, and none from there.
fn utf8_encode(c: u32, out: &mut Vec<u8>) {
    let cont = |shift: u32| 0x80 | (c >> shift) as u8 & 0x3f;
    match c {
        0..0x80 => out.push(c as u8),
        0x80..0x800 => out.extend([0xc0 | (c >> 6) as u8, cont(0)]),
        0x800..0x1_0000 => out.extend([0xe0 | (c >> 12) as u8, cont(6), cont(0)]),
        0x1_0000..0x20_0000 => out.extend([0xf0 | (c >> 18) as u8, cont(12), cont(6), cont(0)]),
        0x20_0000..0x40_0000 => {
            out.extend([0xf8 | (c >> 24) as u8, cont(18), cont(12), cont(6), cont(0)])
        }
        0x40_0000..0x8000_0000 => out.extend([
            0xfc | (c >> 30) as u8,
            cont(24),
            cont(18),
            cont(12),
            cont(6),
            cont(0),
        ]),
        _ => {}
    }
}

fn char_(vm: &mut Vm, nargs: usize) -> R<usize> {
    let mut out = Vec::with_capacity(nargs);
    for i in 1..=nargs {
        let c = vm.check_int(nargs, i)?;
        // `uchar(c) == c`.
        if !(0..=255).contains(&c) {
            return Err(vm.arg_error(i, "invalid value"));
        }
        out.push(c as u8);
    }
    Ok(vm.ret_str(&out))
}

/// 5.1's `rep`: `n` copies; a result past [`crate::api::MAX_STRING`] is `not enough memory`.
fn rep(vm: &mut Vm, nargs: usize) -> R<usize> {
    let s = vm.check_str(nargs, 1)?;
    let n = vm.check_int(nargs, 2)?;
    let l = vm.heap.str(s).len();
    if n <= 0 || l == 0 {
        return Ok(vm.ret_str(b""));
    }
    let total = l.saturating_mul(n as usize);
    let [mut out] = vm.take_bufs::<1>();
    let r = vm.reserve(&mut out, total);
    if r.is_ok() {
        // Doubling copies, as `slice::repeat` makes them.
        out.extend_from_slice(vm.heap.str(s));
        while out.len() <= total / 2 {
            out.extend_from_within(..);
        }
        out.extend_from_within(..total - out.len());
    }
    let r = r.map(|()| vm.ret_str(&out));
    vm.give_bufs([out]);
    r
}

/// The client's `str_byte` (`0x7fc210`): the byte at one position, `luaL_optnumber` (default 1,
/// `0x7fc238`) truncated to 32 bits by `__ftol` and counted from the end when negative
/// (`0x7fc247`), or nothing outside 1 to the length (`0x7fc24c`-`0x7fc252`). A third argument is
/// never read.
fn byte(vm: &mut Vm, nargs: usize) -> R<usize> {
    let s = vm.check_str(nargs, 1)?;
    let l = vm.heap.str(s).len();
    let pos = crate::api::c_int(vm.opt_number(nargs, 2, 1.0)?) as i64;
    let pos = if pos < 0 { pos + l as i64 + 1 } else { pos };
    if pos <= 0 || pos > l as i64 {
        return Ok(0);
    }
    let b = vm.heap.str(s)[pos as usize - 1];
    Ok(vm.ret1(Value::Num(b as f64)))
}

/// `str_dump` (`0x7fc330`): argument 1 must be a function (`0x7fc343`), but `lua_dump`
/// (`0x6f4370`) dumps the stack's top (`0x6f4373`), the last argument, and takes only a Lua
/// function with no upvalues (`0x6f4383`, `0x6f438a`), else `unable to dump given function`
/// (`0x882284`). Deviation: the
/// client writes 5.0 bytecode (`luaU_dump`, `0x6fb600`); benilla writes 5.1's header and the
/// function's source name. The client loads no binary chunk (`0x6f6a15`), so the bytes are only
/// data, and matching them would need 5.0's code generator.
fn dump(vm: &mut Vm, nargs: usize) -> R<usize> {
    vm.check_function(nargs, 1)?;
    let source = match vm.arg(nargs, nargs - 1).unpack() {
        V::Func(f) => match vm.heap.func(f) {
            crate::heap::FuncObj::Lua(l) if l.proto().upvals.is_empty() => {
                Some(l.proto().source.clone())
            }
            _ => None,
        },
        _ => None,
    };
    let Some(source) = source else {
        return Err(vm.error_at(1, "unable to dump given function"));
    };
    let mut out = b"\x1bLuaQ\x00\x01\x04\x08\x04\x08\x00".to_vec();
    out.extend_from_slice(&source);
    Ok(vm.ret_str(&out))
}

// ── string.format ────────────────────────────────────────────────────────────────────────────

/// The client's `str_format` (`0x7fd3d0`): in its Lua 5.0 library, this one function is Lua 4.0's
/// `lstrlib.c` code. The format is a C string, read up to its first NUL. Each item may name its
/// argument with one digit and `$` (`0x7fd4ac`), which sets the cursor the next items go on from;
/// its flags, width and precision are matched as `[-+ #0]*(%d*)%.?(%d*)` (`0x8823ec`), two digits
/// at most each and 18 bytes in all (`0x7fd50c`-`0x7fd51c`), and go to `sprintf` with the
/// conversion.
fn format(vm: &mut Vm, nargs: usize) -> R<usize> {
    let mut b = vm.take_bufs::<2>();
    let r = format_in(vm, nargs, &mut b);
    vm.give_bufs(b);
    r
}

fn format_in(vm: &mut Vm, nargs: usize, [f, out]: &mut [Vec<u8>; 2]) -> R<usize> {
    let fs = vm.check_str(nargs, 1)?;
    f.extend_from_slice(crate::lex::cstr(vm.heap.str(fs)));
    let f = &f[..];
    let at = |j: usize| f.get(j).copied().unwrap_or(0);
    let mut arg = 1;
    let mut i = 0;
    while i < f.len() {
        if f[i] != b'%' {
            out.push(f[i]);
            i += 1;
            continue;
        }
        i += 1;
        if at(i) == b'%' {
            out.push(b'%');
            i += 1;
            continue;
        }
        // A number's text is short: the bound on the whole is enough for it.
        vm.check_len(out)?;
        let mut initf = i;
        if at(i).is_ascii_digit() && at(i + 1) == b'$' {
            arg = (at(i) - b'0') as usize;
            initf = i + 2;
        }
        arg += 1;
        let mut spec = Spec::default();
        let mut j = initf;
        while at(j) != 0 && b"-+ #0".contains(&at(j)) {
            match at(j) {
                b'-' => spec.minus = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'#' => spec.alt = true,
                _ => spec.zero = true,
            }
            j += 1;
        }
        let width_at = j;
        while at(j).is_ascii_digit() {
            spec.width = spec.width.saturating_mul(10) + (at(j) - b'0') as usize;
            j += 1;
        }
        let width_len = j - width_at;
        if at(j) == b'.' {
            j += 1;
            spec.precision = Some(0);
        }
        let prec_at = j;
        while at(j).is_ascii_digit() {
            spec.precision = spec
                .precision
                .map(|p| p.saturating_mul(10) + (at(j) - b'0') as usize);
            j += 1;
        }
        let prec_len = j - prec_at;
        if width_len > 2 || prec_len > 2 || j - initf > 18 {
            return Err(vm.error_at(1, "invalid format (width or precision too long)"));
        }
        let conv = at(j);
        i = j + 1;
        match conv {
            b'c' | b'd' | b'i' | b'o' | b'u' | b'x' | b'X' => {
                // `luaL_checknumber` and `__ftol` (`0x7fd570`, `0x7fd575`): the low 32 bits.
                let n = crate::api::c_int(vm.check_number(nargs, arg)?);
                match conv {
                    b'c' => {
                        let start = out.len();
                        let spec = Spec {
                            precision: None,
                            ..spec
                        };
                        cfmt::write_str(out, &[n as u8], &spec);
                        // `luaL_addlstring(b, buff, strlen(buff))`: a `%c` of 0 ends the item.
                        if let Some(end) = out[start..].iter().position(|&b| b == 0) {
                            out.truncate(start + end);
                        }
                    }
                    b'd' | b'i' => cfmt::write_int(out, n as i64, &spec),
                    _ => cfmt::write_uint(out, n as u32, conv, &spec),
                }
            }
            b'e' | b'E' | b'f' | b'g' | b'G' => {
                let n = vm.check_number(nargs, arg)?;
                cfmt::write_float(out, n, conv, &spec);
            }
            b'q' => {
                let s = vm.check_str(nargs, arg)?;
                // `luaI_addquoted` (`0x7fd6d0`), by its table at `0x7fd7b0`: `"`, `\` and a
                // newline take a backslash, a NUL is `\000`, and every other byte is itself.
                let len = vm.heap.str(s).len();
                vm.reserve(out, len.saturating_mul(4).saturating_add(2))?;
                out.push(b'"');
                for &c in vm.heap.str(s) {
                    match c {
                        b'"' | b'\\' | b'\n' => out.extend_from_slice(&[b'\\', c]),
                        0 => out.extend_from_slice(b"\\000"),
                        _ => out.push(c),
                    }
                }
                out.push(b'"');
            }
            b's' => {
                let s = vm.check_str(nargs, arg)?;
                // At most the string and a two-digit width.
                let len = vm.heap.str(s).len();
                vm.reserve(out, len.saturating_add(99))?;
                let b = vm.heap.str(s);
                // No precision digits and 100 bytes or more: the string whole (`0x7fd609`).
                if prec_len == 0 && b.len() >= 100 {
                    out.extend_from_slice(b);
                } else {
                    // `sprintf` reads a C string.
                    let c = crate::lex::cstr(b);
                    if spec.is_plain() {
                        out.extend_from_slice(c);
                    } else {
                        cfmt::write_str(out, c, &spec);
                    }
                }
            }
            _ => return Err(vm.error_at(1, "invalid option in `format'")),
        }
    }
    Ok(vm.ret_str(out))
}

// ── the matcher ──────────────────────────────────────────────────────────────────────────────

const MAX_CAPTURES: usize = 32;

const CAP_UNFINISHED: isize = -1;
const CAP_POSITION: isize = -2;
const SPECIALS: &[u8] = b"^$*+?.([%-";

struct MatchState<'a> {
    src: &'a [u8],
    /// The pattern, NUL-terminated as 5.0's C string is.
    pat: &'a [u8],
    level: usize,
    capture: [(usize, isize); MAX_CAPTURES],
    /// The Rust stack address past which a nested match raises `pattern too complex`.
    stack_limit: usize,
}

/// A matcher error: the message `luaL_error` raises.
struct PatErr(&'static str);

type M<T> = Result<T, PatErr>;

fn class_match(c: u8, cl: u8) -> bool {
    let res = match cl.to_ascii_lowercase() {
        b'a' => c.is_ascii_alphabetic(),
        b'c' => c < 32 || c == 127,
        b'd' => c.is_ascii_digit(),
        b'l' => c.is_ascii_lowercase(),
        b'p' => c.is_ascii_punctuation(),
        b's' => matches!(c, 9..=13 | 32),
        b'u' => c.is_ascii_uppercase(),
        b'w' => c.is_ascii_alphanumeric(),
        b'x' => c.is_ascii_hexdigit(),
        b'z' => c == 0,
        _ => return cl == c,
    };
    if cl.is_ascii_lowercase() {
        res
    } else {
        !res
    }
}

impl MatchState<'_> {
    fn p(&self, i: usize) -> u8 {
        self.pat.get(i).copied().unwrap_or(0)
    }

    fn class_end(&self, p: usize) -> M<usize> {
        let mut p = p;
        let c = self.p(p);
        p += 1;
        match c {
            b'%' => {
                if self.p(p) == 0 {
                    return Err(PatErr("malformed pattern (ends with `%')"));
                }
                Ok(p + 1)
            }
            b'[' => {
                if self.p(p) == b'^' {
                    p += 1;
                }
                loop {
                    if self.p(p) == 0 {
                        return Err(PatErr("malformed pattern (missing `]')"));
                    }
                    let c = self.p(p);
                    p += 1;
                    if c == b'%' && self.p(p) != 0 {
                        p += 1;
                    }
                    if self.p(p) == b']' {
                        break;
                    }
                }
                Ok(p + 1)
            }
            _ => Ok(p),
        }
    }

    /// `matchbracketclass`: `p` at the `[`, `ec` at the `]`.
    fn bracket(&self, c: u8, p: usize, ec: usize) -> bool {
        let mut sig = true;
        let mut p = p;
        if self.p(p + 1) == b'^' {
            sig = false;
            p += 1;
        }
        loop {
            p += 1;
            if p >= ec {
                break;
            }
            if self.p(p) == b'%' {
                p += 1;
                if class_match(c, self.p(p)) {
                    return sig;
                }
            } else if self.p(p + 1) == b'-' && p + 2 < ec {
                p += 2;
                if self.p(p - 2) <= c && c <= self.p(p) {
                    return sig;
                }
            } else if self.p(p) == c {
                return sig;
            }
        }
        !sig
    }

    fn single(&self, c: u8, p: usize, ep: usize) -> bool {
        match self.p(p) {
            b'.' => true,
            b'%' => class_match(c, self.p(p + 1)),
            b'[' => self.bracket(c, p, ep - 1),
            pc => pc == c,
        }
    }

    /// A nested `do_match`, within the Rust stack the VM allows its calls. The first level,
    /// which `find`, `gsub` and `gfind` call, is not checked, so a pattern that never nests pays
    /// nothing. Deviation: 5.1's `match` recurses without a bound and overflows the C stack
    /// (near 32000 levels on 8 MB); this raises 5.2's `pattern too complex` instead. The bound
    /// is stack measured, not levels counted, so it holds at any optimisation level: a level
    /// takes about 110 bytes at opt-level 1 and about 1.5 KB at opt-level 0.
    #[inline(never)]
    fn sub_match(&mut self, s: usize, p: usize) -> M<Option<usize>> {
        let marker = 0u8;
        if (std::ptr::addr_of!(marker) as usize) < self.stack_limit {
            return Err(PatErr("pattern too complex"));
        }
        self.do_match(s, p)
    }

    fn do_match(&mut self, s: usize, p: usize) -> M<Option<usize>> {
        let (mut s, mut p) = (s, p);
        loop {
            match self.p(p) {
                b'(' => {
                    return if self.p(p + 1) == b')' {
                        self.start_capture(s, p + 2, CAP_POSITION)
                    } else {
                        self.start_capture(s, p + 1, CAP_UNFINISHED)
                    };
                }
                b')' => return self.end_capture(s, p + 1),
                0 => return Ok(Some(s)),
                b'$' if self.p(p + 1) == 0 => {
                    return Ok(if s == self.src.len() { Some(s) } else { None });
                }
                b'%' if self.p(p + 1) == b'b' => match self.balance(s, p + 2)? {
                    Some(e) => {
                        s = e;
                        p += 4;
                        continue;
                    }
                    None => return Ok(None),
                },
                b'%' if self.p(p + 1) == b'f' => {
                    p += 2;
                    if self.p(p) != b'[' {
                        return Err(PatErr("missing `[' after `%f' in pattern"));
                    }
                    let ep = self.class_end(p)?;
                    let prev = if s == 0 { 0 } else { self.src[s - 1] };
                    let cur = self.src.get(s).copied().unwrap_or(0);
                    if !self.bracket(prev, p, ep - 1) && self.bracket(cur, p, ep - 1) {
                        p = ep;
                        continue;
                    }
                    return Ok(None);
                }
                b'%' if self.p(p + 1).is_ascii_digit() => {
                    match self.match_capture(s, self.p(p + 1))? {
                        Some(e) => {
                            s = e;
                            p += 2;
                            continue;
                        }
                        None => return Ok(None),
                    }
                }
                _ => {
                    let ep = self.class_end(p)?;
                    let m = s < self.src.len() && self.single(self.src[s], p, ep);
                    match self.p(ep) {
                        b'?' => {
                            if m {
                                if let Some(r) = self.sub_match(s + 1, ep + 1)? {
                                    return Ok(Some(r));
                                }
                            }
                            p = ep + 1;
                        }
                        b'*' => return self.max_expand(s, p, ep),
                        b'+' => {
                            return if m {
                                self.max_expand(s + 1, p, ep)
                            } else {
                                Ok(None)
                            }
                        }
                        b'-' => return self.min_expand(s, p, ep),
                        _ => {
                            if !m {
                                return Ok(None);
                            }
                            s += 1;
                            p = ep;
                        }
                    }
                }
            }
        }
    }

    fn balance(&self, s: usize, p: usize) -> M<Option<usize>> {
        if self.p(p) == 0 || self.p(p + 1) == 0 {
            return Err(PatErr("unbalanced pattern"));
        }
        if s >= self.src.len() || self.src[s] != self.p(p) {
            return Ok(None);
        }
        let (b, e) = (self.p(p), self.p(p + 1));
        let mut cont = 1;
        let mut s = s + 1;
        while s < self.src.len() {
            let c = self.src[s];
            if c == e {
                cont -= 1;
                if cont == 0 {
                    return Ok(Some(s + 1));
                }
            } else if c == b {
                cont += 1;
            }
            s += 1;
        }
        Ok(None)
    }

    // Inlined into `do_match`: one frame per pattern level, so a deep match stays within the
    // CPU's return-address predictor.
    #[inline(always)]
    fn max_expand(&mut self, s: usize, p: usize, ep: usize) -> M<Option<usize>> {
        let mut i = 0;
        while s + i < self.src.len() && self.single(self.src[s + i], p, ep) {
            i += 1;
        }
        loop {
            if let Some(r) = self.sub_match(s + i, ep + 1)? {
                return Ok(Some(r));
            }
            if i == 0 {
                return Ok(None);
            }
            i -= 1;
        }
    }

    // Inlined into `do_match`: one frame per pattern level, so a deep match stays within the
    // CPU's return-address predictor.
    #[inline(always)]
    fn min_expand(&mut self, s: usize, p: usize, ep: usize) -> M<Option<usize>> {
        let mut s = s;
        loop {
            if let Some(r) = self.sub_match(s, ep + 1)? {
                return Ok(Some(r));
            }
            if s < self.src.len() && self.single(self.src[s], p, ep) {
                s += 1;
            } else {
                return Ok(None);
            }
        }
    }

    // Inlined into `do_match`: one frame per pattern level, so a deep match stays within the
    // CPU's return-address predictor.
    #[inline(always)]
    fn start_capture(&mut self, s: usize, p: usize, what: isize) -> M<Option<usize>> {
        if self.level >= MAX_CAPTURES {
            return Err(PatErr("too many captures"));
        }
        self.capture[self.level] = (s, what);
        self.level += 1;
        let r = self.sub_match(s, p)?;
        if r.is_none() {
            self.level -= 1;
        }
        Ok(r)
    }

    // Inlined into `do_match`: one frame per pattern level, so a deep match stays within the
    // CPU's return-address predictor.
    #[inline(always)]
    fn end_capture(&mut self, s: usize, p: usize) -> M<Option<usize>> {
        let l = self.capture_to_close()?;
        self.capture[l].1 = (s - self.capture[l].0) as isize;
        let r = self.sub_match(s, p)?;
        if r.is_none() {
            self.capture[l].1 = CAP_UNFINISHED;
        }
        Ok(r)
    }

    fn capture_to_close(&self) -> M<usize> {
        (0..self.level)
            .rev()
            .find(|&l| self.capture[l].1 == CAP_UNFINISHED)
            .ok_or(PatErr("invalid pattern capture"))
    }

    fn check_capture(&self, l: u8) -> M<usize> {
        let l = l as isize - b'1' as isize;
        if l < 0 || l as usize >= self.level || self.capture[l as usize].1 == CAP_UNFINISHED {
            return Err(PatErr("invalid capture index"));
        }
        Ok(l as usize)
    }

    fn match_capture(&self, s: usize, l: u8) -> M<Option<usize>> {
        let l = self.check_capture(l)?;
        let (init, len) = self.capture[l];
        let len = len as usize;
        if self.src.len() - s >= len && self.src[init..init + len] == self.src[s..s + len] {
            Ok(Some(s + len))
        } else {
            Ok(None)
        }
    }

    /// `push_onecapture`.
    fn capture_value(&self, vm: &mut Vm, i: usize) -> M<Value> {
        let (init, len) = self.capture[i];
        if len == CAP_UNFINISHED {
            return Err(PatErr("unfinished capture"));
        }
        Ok(if len == CAP_POSITION {
            Value::Num((init + 1) as f64)
        } else {
            vm.str(&self.src[init..init + len as usize])
        })
    }

    /// `push_captures`: the captures, or the whole match `s..e` when there are none.
    fn captures(&self, vm: &mut Vm, whole: Option<(usize, usize)>) -> M<Vec<Value>> {
        if self.level == 0 {
            if let Some((s, e)) = whole {
                return Ok(vec![vm.str(&self.src[s..e])]);
            }
        }
        (0..self.level).map(|i| self.capture_value(vm, i)).collect()
    }
}

fn pat_err(vm: &mut Vm, e: PatErr) -> LuaError {
    vm.error_at(1, e.0)
}

/// The pattern as 5.0 reads it: up to its first NUL, then a NUL.
/// The pattern as 5.0's matcher reads it, a C string: up to its first NUL, then one.
/// The byte a match of `pat` from `p` must start with, when its first item is one literal byte
/// that must occur (no `*`, `?` or `-` after it): `do_match` returns no match at once at any
/// position without it, before it reads the rest of the pattern, so skipping such positions
/// changes no result and no error.
fn first_literal(pat: &[u8], p: usize) -> Option<u8> {
    let (c, ep) = match *pat.get(p)? {
        b'%' => {
            let c = *pat.get(p + 1)?;
            if c == 0 || c.is_ascii_alphanumeric() {
                return None;
            }
            (c, p + 2)
        }
        c if c == 0 || c == b')' || SPECIALS.contains(&c) => return None,
        c => (c, p + 1),
    };
    match pat.get(ep) {
        Some(b'*' | b'?' | b'-') => None,
        _ => Some(c),
    }
}

fn c_pattern(p: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(&p[..p.iter().position(|&b| b == 0).unwrap_or(p.len())]);
    out.push(0);
}

fn new_state<'a>(vm: &Vm, src: &'a [u8], pat: &'a [u8]) -> MatchState<'a> {
    MatchState {
        src,
        pat,
        level: 0,
        capture: [(0, 0); MAX_CAPTURES],
        stack_limit: vm.rust_stack_limit(),
    }
}

// The pattern functions copy the subject and pattern out of the heap (a match calls back into
// the VM); the copies go into buffers the VM keeps, not fresh allocations.
fn find(vm: &mut Vm, nargs: usize) -> R<usize> {
    let mut b = vm.take_bufs::<3>();
    let r = find_in(vm, nargs, &mut b);
    vm.give_bufs(b);
    r
}

fn find_in(vm: &mut Vm, nargs: usize, [src, pat, cpat]: &mut [Vec<u8>; 3]) -> R<usize> {
    let s = vm.check_str(nargs, 1)?;
    let p = vm.check_str(nargs, 2)?;
    let l1 = vm.heap.str(s).len();
    let mut init = posrelat(vm.opt_integer(nargs, 3, 1)?, l1) - 1;
    if init < 0 {
        init = 0;
    } else if init as usize > l1 {
        init = l1 as i64;
    }
    let init = init as usize;
    let plain = vm.arg(nargs, 3).truthy();
    src.extend_from_slice(vm.heap.str(s));
    pat.extend_from_slice(vm.heap.str(p));
    c_pattern(pat, cpat);
    let (src, pat, cpat) = (&src[..], &pat[..], &cpat[..]);
    if plain || !cpat[..cpat.len() - 1].iter().any(|c| SPECIALS.contains(c)) {
        // `lmemfind` over the whole pattern, NULs included.
        let hay = &src[init..];
        let found = if pat.is_empty() {
            Some(0)
        } else if pat.len() > hay.len() {
            None
        } else {
            hay.windows(pat.len()).position(|w| w == pat)
        };
        if let Some(i) = found {
            vm.push(Value::Num((init + i + 1) as f64));
            vm.push(Value::Num((init + i + pat.len()) as f64));
            return Ok(2);
        }
        return Ok(vm.ret1(Value::Nil));
    }
    let (anchor, start) = if cpat[0] == b'^' {
        (true, 1)
    } else {
        (false, 0)
    };
    let mut ms = new_state(vm, src, cpat);
    let mut s1 = init;
    loop {
        ms.level = 0;
        match ms.do_match(s1, start) {
            Err(e) => return Err(pat_err(vm, e)),
            Ok(Some(e)) => {
                let caps = match ms.captures(vm, None) {
                    Ok(c) => c,
                    Err(e) => return Err(pat_err(vm, e)),
                };
                vm.push(Value::Num((s1 + 1) as f64));
                vm.push(Value::Num(e as f64));
                for c in &caps {
                    vm.push(*c);
                }
                return Ok(2 + caps.len());
            }
            Ok(None) => {}
        }
        s1 += 1;
        if s1 > src.len() || anchor {
            break;
        }
    }
    Ok(vm.ret1(Value::Nil))
}

/// `gfind`: an iterator closure over `(s, p, position)`.
fn gfind(vm: &mut Vm, nargs: usize) -> R<usize> {
    let s = vm.check_str(nargs, 1)?;
    let p = vm.check_str(nargs, 2)?;
    let f = vm.new_closure(
        "gfind_aux",
        gfind_aux,
        vec![Value::Str(s), Value::Str(p), Value::Num(0.0)],
    );
    Ok(vm.ret1(f))
}

fn gfind_aux(vm: &mut Vm, nargs: usize) -> R<usize> {
    let mut b = vm.take_bufs::<2>();
    let r = gfind_in(vm, nargs, &mut b);
    vm.give_bufs(b);
    r
}

fn gfind_in(vm: &mut Vm, _nargs: usize, [src, cpat]: &mut [Vec<u8>; 2]) -> R<usize> {
    let (V::Str(s), V::Str(p)) = ((vm.upvalue(0)).unpack(), (vm.upvalue(1)).unpack()) else {
        return Ok(0);
    };
    let pos = vm.tonumber(vm.upvalue(2)).unwrap_or(0.0) as usize;
    src.extend_from_slice(vm.heap.str(s));
    c_pattern(vm.heap.str(p), cpat);
    let (src, cpat) = (&src[..], &cpat[..]);
    let mut ms = new_state(vm, src, cpat);
    let mut at = pos;
    while at <= src.len() {
        ms.level = 0;
        match ms.do_match(at, 0) {
            Err(e) => return Err(pat_err(vm, e)),
            Ok(Some(e)) => {
                let newstart = if e == at { e + 1 } else { e };
                vm.set_upvalue(2, Value::Num(newstart as f64));
                let caps = match ms.captures(vm, Some((at, e))) {
                    Ok(c) => c,
                    Err(e) => return Err(pat_err(vm, e)),
                };
                for c in &caps {
                    vm.push(*c);
                }
                return Ok(caps.len());
            }
            Ok(None) => at += 1,
        }
    }
    Ok(0)
}

/// The client's `str_gsub` (`0x7fd0e0`): 5.0's, a string or a function replacement.
fn gsub(vm: &mut Vm, nargs: usize) -> R<usize> {
    let mut b = vm.take_bufs::<4>();
    let r = gsub_in(vm, nargs, &mut b);
    vm.give_bufs(b);
    r
}

fn gsub_in(vm: &mut Vm, nargs: usize, [src, cpat, out, repl_buf]: &mut [Vec<u8>; 4]) -> R<usize> {
    let s = vm.check_str(nargs, 1)?;
    let p = vm.check_str(nargs, 2)?;
    let srcl = vm.heap.str(s).len();
    let repl = vm.arg(nargs, 2);
    // `luaL_optint(L, 4, srcl+1)`: the default cast to `int` as C casts it.
    let max_s = vm.opt_int(nargs, 4, (srcl as i64 + 1) as i32)?;
    // `lua_isstring(L, 3) || lua_type(L, 3) == LUA_TFUNCTION`, else `string or function expected`
    // (`0x7fd163`-`0x7fd195`): no table.
    let kind = match repl.unpack() {
        _ if nargs < 3 => None,
        V::Str(_) | V::Num(_) => Some(0),
        V::Func(_) => Some(1),
        _ => None,
    };
    let Some(kind) = kind else {
        return Err(vm.arg_error(3, "string or function expected"));
    };
    if kind == 0 {
        // `lua_tolstring(L, 3)`: a number replacement is its text.
        let r = vm.tostr(repl).unwrap();
        repl_buf.extend_from_slice(vm.heap.str(r));
    }
    src.extend_from_slice(vm.heap.str(s));
    c_pattern(vm.heap.str(p), cpat);
    let (src, cpat, repl_bytes) = (&src[..], &cpat[..], &repl_buf[..]);
    let (anchor, pstart) = if cpat[0] == b'^' {
        (true, 1)
    } else {
        (false, 0)
    };
    let mut ms = new_state(vm, src, cpat);
    let mut at = 0usize;
    let mut n = 0;
    let lit = if anchor {
        None
    } else {
        first_literal(cpat, pstart)
    };
    while n < max_s {
        // Straight to the next place the first byte occurs: `do_match` fails at once anywhere
        // else, and those bytes are copied as the loop below would copy them one by one.
        if let Some(c) = lit {
            match src[at..].iter().position(|&b| b == c) {
                Some(k) => {
                    out.extend_from_slice(&src[at..at + k]);
                    at += k;
                }
                None => break,
            }
        }
        ms.level = 0;
        let e = match ms.do_match(at, pstart) {
            Ok(e) => e,
            Err(e) => return Err(pat_err(vm, e)),
        };
        if let Some(e) = e {
            n += 1;
            add_value(vm, &ms, out, kind, repl, repl_bytes, at, e)?;
        }
        match e {
            Some(e) if e > at => at = e,
            _ if at < src.len() => {
                out.push(src[at]);
                at += 1;
            }
            _ => break,
        }
        if anchor {
            break;
        }
    }
    out.extend_from_slice(&src[at..]);
    let r = vm.str(out);
    vm.push(r);
    vm.push(Value::Num(n as f64));
    Ok(2)
}

/// The client's `add_s` (`0x7fd2a0`): one match's replacement onto `out`.
#[allow(clippy::too_many_arguments)]
fn add_value(
    vm: &mut Vm,
    ms: &MatchState,
    out: &mut Vec<u8>,
    kind: u8,
    repl: Value,
    repl_bytes: &[u8],
    s: usize,
    e: usize,
) -> R<()> {
    if kind == 0 {
        // `%` and a digit is a capture through `check_capture` (`0x7fd34d`), so `%0`, and `%1`
        // with no capture, raise `invalid capture index`; `%` and anything else is that byte.
        vm.check_len(out)?;
        let mut i = 0;
        while i < repl_bytes.len() {
            let c = repl_bytes[i];
            if c != b'%' {
                out.push(c);
            } else {
                i += 1;
                let d = repl_bytes.get(i).copied().unwrap_or(0);
                if !d.is_ascii_digit() {
                    out.push(d);
                } else {
                    let v = ms
                        .check_capture(d)
                        .and_then(|l| ms.capture_value(vm, l))
                        .map_err(|err| pat_err(vm, err))?;
                    let sv = vm.tostr(v).unwrap();
                    vm.reserve(out, vm.heap.str(sv).len())?;
                    out.extend_from_slice(vm.heap.str(sv));
                }
            }
            i += 1;
        }
        return Ok(());
    }
    let caps = ms
        .captures(vm, Some((s, e)))
        .map_err(|err| pat_err(vm, err))?;
    let v = vm.call_value(repl, &caps, 1)?[0];
    // A string or number result is added, anything else dropped with the match
    // (`0x7fd392`-`0x7fd3b7`).
    if let Some(sv) = matches!(v.unpack(), V::Str(_) | V::Num(_))
        .then(|| vm.tostr(v))
        .flatten()
    {
        vm.reserve(out, vm.heap.str(sv).len())?;
        out.extend_from_slice(vm.heap.str(sv));
    }
    Ok(())
}
