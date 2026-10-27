//! Fills the `%` holes of a `GlobalStrings.lua` or `GlueStrings.lua` template read at runtime, as
//! the client's `SStrPrintf` does. Storm's ordered formatter `vsnoprintf` (`0x6697d0`), which
//! `ClientMain` switches on (`0x402141`), binds each hole to an argument before it prints: a plain
//! hole takes the running index, `%N$` sets that index to N - 1 (`0x669af7`-`0x669b0b`, N = 0 gives
//! 0), and after any hole that takes an argument the running index is its index plus one
//! (`0x669892`-`0x669895`). So `%2$s` then `%s` takes arguments 2 and 3 (`DUEL_WINNER_RETREAT`
//! reorders its two names). An argument no hole names is skipped. `%s`, `%d`, `%c`, `%f` and `%g`
//! print their argument, `%%` is `%`, and the precision is the template's. [`fill`] copies a hole
//! with no argument, or one this does not know, through literally so a mis-modelled template
//! shows; [`fill_exact`] refuses the line instead. Reals are the MSVC CRT's behind `SStrPrintf`
//! ([`crt_float`]).

use std::fmt::Write as _;

/// The client's string-to-bool `0x6f1b30` for the words it knows, `None` for any other string. A
/// first byte of `1`-`9`, `T`, `t`, `Y` or `y` is true and `0`, `F`, `f`, `N` or `n` false (the
/// jump table `0x6f1bc0` over `0x6f1bb4`); every other string, an empty one included, is compared
/// whole and case-insensitively (`0x64a4c0`): `off` (`0x871460`) and `disabled` (`0x853758`) are
/// false, `on` (`0x87145c`) and `enabled` (`0x853764`) true.
pub fn keyword_bool(bytes: &[u8]) -> Option<bool> {
    match bytes.first() {
        Some(b'0' | b'F' | b'N' | b'f' | b'n') => Some(false),
        Some(b'1'..=b'9' | b'T' | b'Y' | b't' | b'y') => Some(true),
        _ if bytes.eq_ignore_ascii_case(b"off") || bytes.eq_ignore_ascii_case(b"disabled") => {
            Some(false)
        }
        _ if bytes.eq_ignore_ascii_case(b"on") || bytes.eq_ignore_ascii_case(b"enabled") => {
            Some(true)
        }
        _ => None,
    }
}

/// `0x6f1b30` whole: anything [`keyword_bool`] does not know is false. The `.toc` reader's
/// `LoadOnDemand:` and `Secure:` and the XML loaders' bool attributes (`hidden`, `toplevel`,
/// `movable`, ...) all read through it.
pub fn string_to_bool(s: &str) -> bool {
    keyword_bool(s.as_bytes()).unwrap_or(false)
}

/// The grammar-escape rewrite `0x7716f0` that `SetText` (`0x771d80`) runs on its text, flag set,
/// before the separator pass and with no locale gate (`0x771d94`-`0x771d98`). It scans for a `|`
/// that is not doubled (`||` skips its second) and is followed by a number that `SStrToInt`
/// (`0x64ac60`) reads as 1 or 2, then rewrites every escape of that kind in the text
/// ([`korean_particles`] for `|1`, [`french_articles`] for `|2`), and starts over on the result
/// until neither is left. Deviation: a pass that changes nothing (`|01`, `|02`) ends the scan, where
/// the reference loops on it for ever. Text with no `|` is returned as it came.
pub fn expand_grammar_escapes(text: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    let mut cur = Cow::Borrowed(text);
    while let Some(kind) = first_grammar_escape(cur.as_bytes()) {
        let units: Vec<u16> = cur.encode_utf16().take(UNITS_CAP).collect();
        let units = if kind == 1 {
            korean_particles(&units)
        } else {
            french_articles(&units)
        };
        let mut next = String::from_utf16_lossy(&units);
        // The static buffer `0xcf1cc8` holds `0xfff` bytes and the NUL (`0x771887`); keep whole
        // characters, as [`swap_number_separators`] does.
        if next.len() > BYTES_CAP {
            let mut end = BYTES_CAP;
            while !next.is_char_boundary(end) {
                end -= 1;
            }
            next.truncate(end);
        }
        if next == *cur {
            break;
        }
        cur = Cow::Owned(next);
    }
    cur
}

/// The conversion buffers of `0x771770` and `0x7719a0`: `0x1000` UTF-16 units with the NUL
/// (`0x771789`, `0x7719b9`), and `0x1000` bytes with the NUL for the static result (`0x771887`).
const UNITS_CAP: usize = 0xfff;
const BYTES_CAP: usize = 0xfff;

/// The first escape `0x7716f0` hands on: 1 for `|1` (`0x771757`), 2 for `|2` (`0x77174c`), `None`
/// when the text has neither. The digits after a `|` are read as one wrapping 32-bit number.
fn first_grammar_escape(b: &[u8]) -> Option<u32> {
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'|' {
            match b.get(i + 1) {
                Some(b'|') => i += 1,
                Some(d) if d.is_ascii_digit() => {
                    let n = b[i + 1..]
                        .iter()
                        .take_while(|c| c.is_ascii_digit())
                        .fold(0u32, |n, c| {
                            n.wrapping_mul(10).wrapping_add(u32::from(c - b'0'))
                        });
                    if n == 1 || n == 2 {
                        return Some(n);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// The `|1` pass `0x771770`: `|1first;second;` becomes one of its parts, chosen by the text built
/// so far ([`particle_part`]). A `|` followed by anything but `1`, or by `1` and a digit, is copied
/// with that character; a part ends at its `;` or the end, and a missing part is empty.
fn korean_particles(units: &[u16]) -> Vec<u16> {
    let at = |i: usize| units.get(i).copied().unwrap_or(0);
    let end_of = |from: usize| {
        units[from.min(units.len())..]
            .iter()
            .position(|&u| u == u16::from(b';'))
            .map_or(units.len(), |k| from + k)
    };
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        if units[i] != u16::from(b'|') {
            out.push(units[i]);
            i += 1;
        } else if at(i + 1) != u16::from(b'1') || is_digit_unit(at(i + 2)) {
            // `0x77185e`: the bar and the character after it are copied, the second never read
            // as a bar; a bar ending the text is copied alone (the reference reads on past it).
            out.push(units[i]);
            out.extend(units.get(i + 1));
            i += 2;
        } else {
            let first = i + 2..end_of(i + 2);
            let second_from = (first.end + 1).min(units.len());
            let second = second_from..end_of(second_from);
            let part = particle_part(&out, &units[first], &units[second.clone()]);
            out.extend_from_slice(part);
            i = (second.end + 1).min(units.len());
        }
    }
    out
}

fn is_digit_unit(u: u16) -> bool {
    (u16::from(b'0')..=u16::from(b'9')).contains(&u)
}

/// `0x7718b0`: which part of a particle pair follows the text `before`. It walks back from the last
/// unit and stops at the first that decides, skipping a unit below `0x100` that follows a `|`
/// (`0x771902`). A Hangul syllable (`0xac00..=0xd7a3`) with a final consonant, `(u - 0xac00) % 28`
/// not 0, picks the first part, one without it the second, except that a final `ㄹ`
/// (`(u - 0xac08) % 28 == 0`) picks the second when the first part starts with `으` (`0xc73c`,
/// `0x7718ef`). A digit picks by the byte table at `0x81c6c4` (first for 0, 1, 3, 6, 7, 8) and an
/// ASCII letter, lower-cased, by the one at `0x81c6d0` (first for b, c, k, l, m, n, p, q). Any
/// other unit is passed over, and with nothing decided the first part wins (`0x771930`).
fn particle_part<'a>(before: &[u16], first: &'a [u16], second: &'a [u16]) -> &'a [u16] {
    const DIGITS: [bool; 10] = [
        true, true, false, true, false, false, true, true, true, false,
    ];
    const LETTERS: [bool; 26] = [
        false, true, true, false, false, false, false, false, false, false, true, true, true, true,
        false, true, true, false, false, false, false, false, false, false, false, false,
    ];
    for (j, &u) in before.iter().enumerate().rev() {
        if u >= 0x100 {
            if (0xac00..=0xd7a3).contains(&u) {
                let with_rieul_rule =
                    (i32::from(u) - 0xac08) % 28 == 0 && first.first() == Some(&0xc73c);
                let final_consonant = (i32::from(u) - 0xac00) % 28 != 0;
                return if !with_rieul_rule && final_consonant {
                    first
                } else {
                    second
                };
            }
            continue;
        }
        if j > 0 && before[j - 1] == u16::from(b'|') {
            continue;
        }
        let c = u as u8;
        let decided = match c {
            b'0'..=b'9' => Some(DIGITS[usize::from(c - b'0')]),
            b'a'..=b'z' | b'A'..=b'Z' => Some(LETTERS[usize::from(c.to_ascii_lowercase() - b'a')]),
            _ => None,
        };
        if let Some(first_wins) = decided {
            return if first_wins { first } else { second };
        }
    }
    first
}

/// The `|2` pass `0x7719a0`: `|2` and the spaces after it turn the word that follows into its "of
/// the" form. `L'` becomes `l'`, `Le ` `du ` and `La ` `de la `; any other word gets `d'` when it
/// starts with a vowel ([`french_vowel`]), with `h` or `H`, or with `y` or `Y` before a vowel, else
/// `de `. The character after the escape is then copied whatever it is (`0x771b1a`), so it is never
/// read as a bar. As in [`korean_particles`], `|` and a character other than `2`, and `|2` and a
/// digit, are copied as they stand. A text that ends inside an escape ends there; the reference
/// reads on past its terminator.
fn french_articles(units: &[u16]) -> Vec<u16> {
    let at = |i: usize| units.get(i).copied().unwrap_or(0);
    let ch = |c: char| c as u16;
    let mut out = Vec::with_capacity(units.len() + 8);
    let mut i = 0;
    while i < units.len() {
        if units[i] != ch('|') {
            out.push(units[i]);
            i += 1;
            continue;
        }
        if at(i + 1) != ch('2') || is_digit_unit(at(i + 2)) {
            out.push(units[i]);
            out.extend(units.get(i + 1));
            i += 2;
            continue;
        }
        i += 2;
        while at(i) == ch(' ') {
            i += 1;
        }
        let (c0, c1) = (at(i), at(i + 1));
        let words: &[char] = if c0 == ch('L') && c1 == ch('\'') {
            i += 2;
            &['l', '\'']
        } else if c0 == ch('L') && c1 == ch('e') && at(i + 2) == ch(' ') {
            i += 3;
            &['d', 'u', ' ']
        } else if c0 == ch('L') && c1 == ch('a') && at(i + 2) == ch(' ') {
            i += 3;
            &['d', 'e', ' ', 'l', 'a', ' ']
        } else if french_vowel(c0)
            || c0 == ch('h')
            || c0 == ch('H')
            || ((c0 == ch('y') || c0 == ch('Y')) && french_vowel(c1))
        {
            &['d', '\'']
        } else {
            &['d', 'e', ' ']
        };
        out.extend(words.iter().map(|&c| ch(c)));
        out.extend(units.get(i));
        i += 1;
    }
    out
}

/// `0x771b60`: an ASCII vowel (`AEIOUaeiou`, the jump table `0x771bf4` over `0x771bfc`) or a
/// Latin-1 letter in `0xc0..=0xc6`, `0xc8..=0xcf`, `0xd2..=0xd6`, `0xd8..=0xdc` or the same
/// ranges plus `0x20`.
fn french_vowel(u: u16) -> bool {
    matches!(
        u8::try_from(u),
        Ok(b'A' | b'E' | b'I' | b'O' | b'U' | b'a' | b'e' | b'i' | b'o' | b'u')
    ) || matches!(u, 0xc0..=0xc6 | 0xc8..=0xcf | 0xd2..=0xd6 | 0xd8..=0xdc)
        || matches!(u, 0xe0..=0xe6 | 0xe8..=0xef | 0xf2..=0xf6 | 0xf8..=0xfc)
}

/// The number-separator pass `0x771c50` that `SetText` (`0x771d80`) runs on its text while
/// `SetEuropeanNumbers` holds `[0xcf4d24]` set (`0x771d9f`-`0x771daa`). A number is a run of ASCII
/// digits, `.` and `,` that starts at a digit. Its `.` and `,` trade places (`0x771d16`,
/// `0x771d23`) when it has exactly one `.` (`0x771cc3`), no `,` after that `.` (`0x771ccc`), a byte
/// after the `.` that is at least `0` and a byte after that which is at most `9` (`0x771cd1`,
/// `0x771cd7`), both compared as signed bytes, so `1.5g` stays and `1.55g` becomes `1,55g`.
/// Every other number, and every other character, is copied unchanged. The result stops at the
/// 4095th byte of the static buffer `0xcf1cc8` (`0x771c70`, `0x771cf5`).
pub(crate) fn swap_number_separators(text: &str) -> String {
    /// `0xcf2cc7 - 0xcf1cc8`: the bytes the pass writes before it stops.
    const CAP: usize = 0xfff;
    let b = text.as_bytes();
    // `cmp al,0x30; jl` / `cmp al,0x39; jg` are signed, so a byte of 0x80 or more is below `0`.
    let digit = |c: u8| c.is_ascii_digit();
    let number_byte = |c: u8| digit(c) || c == b'.' || c == b',';
    let signed_at = |k: usize| b.get(k).copied().unwrap_or(0) as i8;
    let mut out = Vec::with_capacity(b.len().min(CAP));
    let mut i = 0;
    while i < b.len() && out.len() < CAP {
        if !digit(b[i]) {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let end = (i..b.len())
            .find(|&k| !number_byte(b[k]))
            .unwrap_or(b.len());
        let run = &b[i..end];
        let dots = run.iter().filter(|&&c| c == b'.').count();
        let swap = dots == 1 && {
            let dot = i + run.iter().position(|&c| c == b'.').unwrap_or(0);
            let comma_after = run
                .iter()
                .rposition(|&c| c == b',')
                .is_some_and(|k| i + k >= dot);
            !comma_after && signed_at(dot + 1) >= b'0' as i8 && signed_at(dot + 2) <= b'9' as i8
        };
        for &c in run {
            if out.len() >= CAP {
                break;
            }
            out.push(match c {
                b'.' if swap => b',',
                b',' if swap => b'.',
                c => c,
            });
        }
        i = end;
    }
    // The input is valid UTF-8 and only ASCII bytes change, so the cap alone can cut a sequence:
    // drop the cut character, which the reference's one-byte characters never have, rather than
    // show U+FFFD.
    match String::from_utf8(out) {
        Ok(text) => text,
        Err(e) => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).unwrap_or_default()
        }
    }
}

/// One argument to [`fill`]; any variant renders in any hole, so the template's order decides.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arg<'a> {
    /// Text; also what `%c` takes, whose one 1.12 use is the `+`/`-` sign of `ITEM_MOD_*` and
    /// `ITEM_RESIST_*`.
    S(&'a str),
    D(i64),
    /// A real, for the rate lines (`DPS_TEMPLATE`, the `AMMO_*` damage templates).
    F(f64),
}

impl<'a> From<&'a str> for Arg<'a> {
    fn from(s: &'a str) -> Self {
        Arg::S(s)
    }
}

impl<'a> From<&'a String> for Arg<'a> {
    fn from(s: &'a String) -> Self {
        Arg::S(s.as_str())
    }
}

macro_rules! arg_from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Arg<'_> {
            fn from(n: $t) -> Self {
                Arg::D(i64::from(n))
            }
        }
    )*};
}
arg_from_int!(u8, u16, u32, i8, i16, i32);

impl From<f32> for Arg<'_> {
    fn from(x: f32) -> Self {
        Arg::F(f64::from(x))
    }
}

impl From<f64> for Arg<'_> {
    fn from(x: f64) -> Self {
        Arg::F(x)
    }
}

impl Arg<'_> {
    /// Whether this is the kind a `conv` hole prints as itself, for [`fill_exact`].
    fn fits(&self, conv: char) -> bool {
        matches!(
            (conv, self),
            ('s' | 'c', Arg::S(_)) | ('d', Arg::D(_)) | ('f' | 'g', Arg::F(_))
        )
    }

    /// `%s` and `%c`.
    fn as_s(&self, out: &mut String) {
        match self {
            Arg::S(s) => out.push_str(s),
            Arg::D(n) => {
                let _ = write!(out, "{n}");
            }
            Arg::F(x) => {
                let _ = write!(out, "{x}");
            }
        }
    }

    fn as_d(&self, out: &mut String) {
        match self {
            Arg::D(n) => {
                let _ = write!(out, "{n}");
            }
            // A string in a `%d` hole is a caller bug; showing it beats showing nothing.
            Arg::S(s) => out.push_str(s),
            // C truncates toward zero here; so does `as`.
            Arg::F(x) => {
                let _ = write!(out, "{}", *x as i64);
            }
        }
    }

    /// `%f`/`%.Nf` by the CRT, the precision defaulting to C's 6.
    fn as_f(&self, prec: Option<usize>, out: &mut String) {
        match self {
            Arg::F(x) => out.push_str(&crt_float(*x, b'f', prec)),
            Arg::D(n) => out.push_str(&crt_float(*n as f64, b'f', prec)),
            Arg::S(s) => out.push_str(s),
        }
    }

    /// `%g`/`%.Ng` by the CRT, the precision counting significant digits: 1.12's five `%.3g`
    /// cast and cooldown templates need it, a 100-second cooldown being "1.67 min".
    fn as_g(&self, prec: Option<usize>, out: &mut String) {
        let Arg::F(x) = self else {
            return self.as_s(out);
        };
        out.push_str(&crt_float(*x, b'g', prec));
    }
}

/// A real as `SStrPrintf` (`0x64a7f0`) writes it: `ISStrVPrintf` (`0x64a820`) hands the
/// template to the CRT's `_vsnprintf` (`0x73f6ad`), or with Storm's positional option set to
/// `vsnoprintf` (`0x6697d0`), which passes each hole to the CRT's `_snprintf` (`0x6699fb`);
/// either way MSVC's `__output` formats it, half up on the digit text, with three exponent
/// digits and `1.#INF` for an infinity.
fn crt_float(x: f64, conv: u8, prec: Option<usize>) -> String {
    mlua::benilla_lua::crt_float(x, conv, prec)
}

/// One parsed `%…` specifier.
struct Spec {
    /// The 1-based `N` of `%N$`.
    positional: Option<usize>,
    /// `.N`.
    precision: Option<usize>,
    conv: char,
    /// Index just past the conversion letter.
    end: usize,
}

/// Scan `[N$][flags][width][.prec]conv` just after a `%`; `None` for a conversion this does not
/// know.
fn parse_spec(chars: &[char], after_percent: usize) -> Option<Spec> {
    let mut i = after_percent;
    let digits = |i: &mut usize| {
        let start = *i;
        while chars.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
        chars[start..*i].iter().collect::<String>()
    };
    // Digits are a position only when a `$` follows; otherwise they are a width.
    let mut positional = None;
    let mut probe = i;
    let n = digits(&mut probe);
    if !n.is_empty() && chars.get(probe) == Some(&'$') {
        positional = n.parse::<usize>().ok();
        i = probe + 1;
    }
    while matches!(chars.get(i), Some('-' | '+' | ' ' | '#' | '0')) {
        i += 1;
    }
    let _width = digits(&mut i);
    let precision = (chars.get(i) == Some(&'.')).then(|| {
        i += 1;
        digits(&mut i).parse::<usize>().unwrap_or(0)
    });
    let conv = *chars.get(i)?;
    matches!(conv, 's' | 'd' | 'c' | 'f' | 'g').then_some(Spec {
        positional,
        precision,
        conv,
        end: i + 1,
    })
}

/// A string from the VM's globals, where the install's `GlobalStrings.lua` is loaded at boot;
/// `None`, and so no line, when absent or empty. `FrameScript_GetText` returns an empty string
/// (`0x882748`) for a missing key, and callers test both (`GetPVPRankInfo`, `0x51aa1c`/`0x51aa20`).
pub fn global(lua: &mlua::Lua, key: &str) -> Option<String> {
    // Bound before the filter so `globals().get::<String>` stays one unbroken substring, which is
    // what `benilla-app/tests/reference_strings.rs` matches as a key lookup.
    let value = lua.globals().get::<String>(key).ok();
    value.filter(|s| !s.is_empty())
}

/// Where the engine's text reads find their strings (`FrameScript_GetText`, `0x703bf0`). A bare
/// key is a global's text (`0x704350`); a gendered or counted one is a call of the Lua `GetText`,
/// and a plural index one of `GetPluralIndex` (`0x703cf0`), plain FrameXML functions an addon can
/// replace (`LocaleProperties.lua:26`, `:56`).
#[derive(Clone, Copy)]
pub enum Strings<'a> {
    /// A running VM: its globals, its `GetText` and its `GetPluralIndex`, called under the runners'
    /// error function.
    Vm(&'a mlua::Lua),
    /// No VM runs: a key lookup, with the stock `LocaleProperties.lua` rule run over it here.
    Host(&'a dyn Fn(&str) -> Option<String>),
}

impl Strings<'_> {
    /// A key's global as a string, the empty one included; `None` when it is none.
    pub fn lookup(&self, key: &str) -> Option<String> {
        match self {
            Self::Vm(lua) => lua.globals().get::<String>(key).ok(),
            Self::Host(get) => get(key),
        }
    }

    /// A key's text, `None` when absent or empty: `0x704350`'s answer, which callers test for
    /// the empty string (`GetPVPRankInfo`, `0x51aa1c`/`0x51aa20`).
    pub fn global(&self, key: &str) -> Option<String> {
        self.lookup(key).filter(|s| !s.is_empty())
    }

    /// `FrameScript_GetText(key, ordinal, gender)` (`0x703bf0`), `None` for the empty string.
    /// With neither a gender nor an ordinal it is [`Self::global`] (`0x703c0d`). Otherwise it calls
    /// `GetText(key, gender, ordinal)`, each absent one nil (`0x703c67`-`0x703c9b`), and takes a
    /// string or number result whatever the call's status (`0x703cb2`), so a failed call's text is
    /// what the error function answered.
    pub fn get_text(&self, key: &str, gender: Option<u32>, ordinal: Option<u32>) -> Option<String> {
        if gender.is_none() && ordinal.is_none() {
            return self.global(key);
        }
        let text = match self {
            Self::Vm(lua) => {
                let args = (key, gender.map(f64::from), ordinal.map(f64::from));
                let answer = crate::script::call_global_under_error_function(lua, "GetText", args)
                    .unwrap_or_else(|failed| failed);
                lua.coerce_string(answer)
                    .ok()
                    .flatten()
                    .map(|s| s.to_string_lossy())
            }
            Self::Host(get) => host_get_text(*get, key, gender, ordinal),
        };
        text.filter(|s| !s.is_empty())
    }

    /// `FrameScriptGetPluralIndex(count)` (`0x703cf0`): `GetPluralIndex(count)` truncated, minus
    /// one (`0x703d68`-`0x703d6f`), and 0 for a failed call or a result that is no number.
    pub fn plural_index(&self, count: i32) -> i32 {
        match self {
            Self::Vm(lua) => crate::script::call_global_under_error_function(
                lua,
                "GetPluralIndex",
                f64::from(count),
            )
            .ok()
            .and_then(|v| lua.coerce_number(v).ok().flatten())
            .map_or(0, |n| crate::script::ftol(n).wrapping_sub(1)),
            Self::Host(_) => i32::from(count != 1),
        }
    }
}

/// The stock `GetText` (`LocaleProperties.lua:26-54`) over `get`: the plural tag `_P1` for any
/// ordinal but 1 (`GetPluralIndex`, `:56`), the gender tag by `GenderTagInfo` (`:20`), the tagged
/// key, then its fallbacks. A key that is defined, even as `""`, ends the search, as Lua's
/// `not string` does.
fn host_get_text(
    get: &dyn Fn(&str) -> Option<String>,
    token: &str,
    gender: Option<u32>,
    ordinal: Option<u32>,
) -> Option<String> {
    let plural = ordinal.filter(|&n| n != 1).map(|_| "_P1");
    let gender = match gender {
        Some(1) => Some("_NONE"),
        Some(3) => Some("_FEMALE"),
        Some(4) => Some("_MPLURAL"),
        Some(5) => Some("_FPLURAL"),
        Some(6) => Some("_MIXED"),
        _ => None,
    };
    let tagged = format!("{token}{}{}", plural.unwrap_or(""), gender.unwrap_or(""));
    get(&tagged).or_else(|| match (plural, gender) {
        (Some(p), Some(g)) => get(&format!("{token}{p}"))
            .or_else(|| get(&format!("{token}{g}")))
            .or_else(|| get(token)),
        _ => get(token),
    })
}

/// `FACTION_STANDING_LABEL<rank + 1>` through the player's gendered text (`0x612bf0`): `GetText`
/// with gender 3 for a female, 2 otherwise (`0x612c12`-`0x612c1a`), then the bare key when that
/// gives the empty string (`0x612c36`). A label absent altogether is the empty string, as
/// `0x703bf0` never returns NULL. Read by the standing chat line, the quest log's faction objective
/// and the item and spell tooltips' reputation lines.
pub fn standing_label(strings: Strings<'_>, rank: u32, female: bool) -> String {
    let key = format!("FACTION_STANDING_LABEL{}", rank.wrapping_add(1));
    strings
        .get_text(&key, Some(if female { 3 } else { 2 }), None)
        .or_else(|| strings.global(&key))
        .unwrap_or_default()
}

/// `GetText(token, nil, ordinal)`, the counted read `0x52fa50` and the other formatters make
/// (`0x703bf0`): with the stock `GetPluralIndex`, one or no ordinal takes the bare token and
/// anything else its `_P1` twin, falling back to the bare token. Zero is plural: the test at
/// `0x703c89` is signed, only a negative becomes nil, and the number `0` is truthy in Lua.
pub fn plural(token: &str, ordinal: Option<u32>, strings: Strings<'_>) -> Option<String> {
    strings.get_text(token, None, ordinal)
}

/// Fill a reference template, by the rules in the module doc.
pub fn fill(template: &str, args: &[Arg<'_>]) -> String {
    run(template, args, false).unwrap_or_default()
}

/// [`fill`] for a caller whose arguments are the template's by construction: `None`, for no line,
/// when a hole has no argument, meets one of another kind (`%s` and `%c` take text, `%d` a
/// number, `%f` and `%g` a real) or is one this does not know. The reference reads each argument
/// by its hole's kind up to the highest index a hole names (`0x6698b5`-`0x669919`), with no check
/// against the arguments it was given, so such a template has no sentence to match: a defect to
/// show. An argument no hole names is not one.
pub fn fill_exact(template: &str, args: &[Arg<'_>]) -> Option<String> {
    run(template, args, true)
}

/// The walk both fills share; `exact` picks the refusal over the literal copy.
fn run(template: &str, args: &[Arg<'_>], exact: bool) -> Option<String> {
    let mut out = String::with_capacity(template.len() + 16);
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    // The running index, `[ebp-0x8]` of `0x6697d0`.
    let mut next = 0usize;
    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if chars.get(i + 1) == Some(&'%') {
            out.push('%');
            i += 2;
            continue;
        }
        let Some(spec) = parse_spec(&chars, i + 1) else {
            if exact {
                return None;
            }
            // Unknown: copy the `%` and rescan from the next char, so `50% off` survives.
            out.push('%');
            i += 1;
            continue;
        };
        let idx = spec.positional.map_or(next, |n| n.saturating_sub(1));
        next = idx + 1;
        match args.get(idx) {
            Some(a) if exact && !a.fits(spec.conv) => return None,
            Some(a) => match spec.conv {
                's' | 'c' => a.as_s(&mut out),
                'd' => a.as_d(&mut out),
                'f' => a.as_f(spec.precision, &mut out),
                _ => a.as_g(spec.precision, &mut out),
            },
            None if exact => return None,
            // Starved: copy the specifier through so a mis-modelled template shows.
            None => out.extend(&chars[i..spec.end]),
        }
        i = spec.end;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_fills_stop_at_the_arguments_they_have() {
        assert_eq!(
            fill("%s: %d/%d", &[Arg::S("MC"), Arg::D(2), Arg::D(5)]),
            "MC: 2/5"
        );
        assert_eq!(fill("%s: %d/%d", &[Arg::S("MC"), Arg::D(2)]), "MC: 2/%d");
        assert_eq!(fill("100%% sure", &[]), "100% sure");
        assert_eq!(fill("no fills", &[Arg::D(7)]), "no fills");
    }

    #[test]
    fn g_is_significant_digits_and_drops_trailing_zeros() {
        let g = |t: &str, v: f64| fill(t, &[Arg::F(v)]);
        assert_eq!(g("%.3g min cooldown", 100.0 / 60.0), "1.67 min cooldown");
        assert_eq!(g("%.3g sec cast", 1.5), "1.5 sec cast");
        assert_eq!(g("%.3g sec cast", 2.0), "2 sec cast");
        // The exponent is read after rounding.
        assert_eq!(g("%.3g", 9.999), "10");
        // A bare `%g` is C's default precision of six.
        assert_eq!(g("%g", 1.0 / 3.0), "0.333333");
        // Outside `P > X >= -4` the style is `%e`, with MSVC's three exponent digits.
        assert_eq!(g("%.3g", 0.000_012_345), "1.23e-005");
        assert_eq!(g("%.3g", 123_456.0), "1.23e+005");
        assert_eq!(g("%g", f64::INFINITY), "1.#INF");
    }

    #[test]
    fn f_takes_the_templates_precision() {
        assert_eq!(
            fill("(%.1f damage per second)", &[Arg::F(41.25)]),
            "(41.3 damage per second)"
        );
        assert_eq!(fill("%.2f", &[Arg::F(41.25)]), "41.25");
    }

    /// The CRT rounds a tie half up on its decimal digits (`_fptostr`'s `>= '5'`), where Rust
    /// rounds it to even; a value just short of the tie still rounds down.
    #[test]
    fn reals_round_ties_up_as_the_crt_does() {
        let f = |t: &str, v: f64| fill(t, &[Arg::F(v)]);
        assert_eq!(f("%.1f", 68.25), "68.3");
        assert_eq!(f("%.1f", 0.25), "0.3");
        assert_eq!(f("%.1f", 0.05), "0.1", "a carry past the point");
        assert_eq!(f("%.1f", 99.96), "100.0", "a carry out of the first digit");
        assert_eq!(f("%.1f", 0.004), "0.0");
        assert_eq!(
            f("%.1f", f64::from(0.35f32)),
            "0.3",
            "0.3499999940… is not a tie"
        );
        assert_eq!(f("%.1f", -2.25), "-2.3");
        assert_eq!(f("%.2f", 0.125), "0.13");
        // `%.3g` rounds at its third significant digit the same way: 67.5 s is 1.125 minutes.
        assert_eq!(f("%.3g min", 1.125), "1.13 min");
        assert_eq!(f("%.3g", 2.675), "2.67", "2.67499999… is not a tie");
    }

    #[test]
    fn plural_takes_the_twin_for_zero_and_the_bare_token_for_none() {
        let table = |key: &str| -> Option<String> {
            match key {
                "T" => Some("<one>".into()),
                "T_P1" => Some("<many>".into()),
                "ONLY" => Some("<only>".into()),
                _ => None,
            }
        };
        let table = Strings::Host(&table);
        assert_eq!(plural("T", Some(1), table).as_deref(), Some("<one>"));
        assert_eq!(plural("T", Some(2), table).as_deref(), Some("<many>"));
        assert_eq!(
            plural("T", Some(0), table).as_deref(),
            Some("<many>"),
            "zero is plural — the signed test, not `> 1`"
        );
        assert_eq!(
            plural("T", None, table).as_deref(),
            Some("<one>"),
            "no ordinal at all takes GetText's short arm"
        );
        assert_eq!(plural("ONLY", Some(7), table).as_deref(), Some("<only>"));
        assert_eq!(plural("ABSENT", Some(2), table), None);
    }

    /// The engine's counted and gendered text calls the Lua `GetText` at the call
    /// (`0x703c43`-`0x703cad`): an addon's replacement changes it, the arguments are the key, the
    /// gender or nil and the ordinal or nil, and a bare key reads the global with no call
    /// (`0x703c0d`).
    #[test]
    fn counted_and_gendered_text_calls_the_lua_get_text() {
        let s = crate::script::UiScript::new().unwrap();
        s.run(
            r#"
            T = "<one>"  T_P1 = "<many>"
            function GetText(token, gender, ordinal)
                CALLS = (CALLS or "") .. token .. ":" .. tostring(gender) .. ":" .. tostring(ordinal) .. ";"
                return "[" .. token .. "]"
            end
        "#,
        )
        .unwrap();
        let vm = Strings::Vm(s.lua());
        assert_eq!(plural("T", Some(2), vm).as_deref(), Some("[T]"));
        assert_eq!(standing_label(vm, 4, true), "[FACTION_STANDING_LABEL5]");
        assert_eq!(vm.get_text("T", None, None).as_deref(), Some("<one>"));
        assert_eq!(
            s.eval::<String>("return CALLS").unwrap(),
            "T:nil:2;FACTION_STANDING_LABEL5:3:nil;"
        );
    }

    /// A failed `GetText` reaches the error handler, and the text is what the error function
    /// answered, a string or number taken whatever the status (`0x703cb2`); `GetPluralIndex` is
    /// truncated minus one, and 0 when it fails or answers no number (`0x703d4c`-`0x703d6f`).
    #[test]
    fn a_failed_call_answers_the_error_function_and_the_plural_index_is_lua_s() {
        let s = crate::script::UiScript::new().unwrap();
        s.run(
            r#"
            function GetText() error("boom") end
            seterrorhandler(function(m) HEARD = m return 7 end)
            function GetPluralIndex(n) return n + 0.5 end
        "#,
        )
        .unwrap();
        let vm = Strings::Vm(s.lua());
        assert_eq!(vm.get_text("T", Some(2), None).as_deref(), Some("7"));
        assert!(s.eval::<String>("return HEARD").unwrap().contains("boom"));
        assert_eq!(vm.plural_index(3), 2);
        s.run("function GetPluralIndex() return 'x' end").unwrap();
        assert_eq!(vm.plural_index(3), 0);
        s.run("function GetPluralIndex() error('no') end").unwrap();
        assert_eq!(vm.plural_index(3), 0);
        // No `GetText` at all: the call itself fails, and the handler answers.
        s.run("GetText = nil seterrorhandler(function() return 'none' end)")
            .unwrap();
        assert_eq!(vm.get_text("T", None, Some(2)).as_deref(), Some("none"));
    }

    /// The duel pair, `GlobalStrings.lua:958-959`.
    #[test]
    fn positional_specifiers_reorder_the_arguments() {
        let (a, b) = (Arg::S("Alice"), Arg::S("Bob"));
        assert_eq!(
            fill("%1$s has defeated %2$s in a duel", &[a, b]),
            "Alice has defeated Bob in a duel"
        );
        assert_eq!(
            fill("%2$s has fled from %1$s in a duel", &[a, b]),
            "Bob has fled from Alice in a duel"
        );
        assert_eq!(fill("%1$s vs %1$s", &[a]), "Alice vs Alice");
    }

    /// `vsnoprintf` binds each hole before it prints (`0x6697d0`): after a hole that takes an
    /// argument the running index is that hole's index plus one (`0x669892`), so a plain hole
    /// after `%N$` takes argument N + 1, and `%0$` is the first argument (`0x669afa`-`0x669b0b`).
    #[test]
    fn a_plain_hole_after_a_positional_takes_the_next_argument() {
        let (a, b, c) = (Arg::S("A"), Arg::S("B"), Arg::S("C"));
        assert_eq!(fill("%2$s then %s", &[a, b, c]), "B then C");
        assert_eq!(fill("%1$s then %s", &[a, b]), "A then B");
        assert_eq!(fill("%s, %s, %1$s, %s", &[a, b, c]), "A, B, A, B");
        assert_eq!(fill("%0$s", &[a]), "A");
        // A hole with no argument still moves the index: the plain hole after it is starved too.
        assert_eq!(fill("%3$s %s", &[a, b]), "%3$s %s");
    }

    /// [`fill_exact`] binds as [`fill`] does and skips an argument no hole names
    /// (`0x6698b5`-`0x669919`), but refuses a hole it cannot fill: none left, of another kind, or
    /// one it does not know.
    #[test]
    fn an_exact_fill_skips_unnamed_arguments_and_refuses_a_hole_it_cannot_fill() {
        let (name, number) = (Arg::S("Ragnaros"), Arg::D(12));
        let exact = |template| fill_exact(template, &[name, number]);
        assert_eq!(
            exact("You hit %s for %d.").as_deref(),
            Some("You hit Ragnaros for 12.")
        );
        assert_eq!(
            exact("For %2$d.").as_deref(),
            Some("For 12."),
            "the name is unused"
        );
        assert_eq!(
            exact("%2$d, %1$s: 100%%").as_deref(),
            Some("12, Ragnaros: 100%")
        );
        assert_eq!(
            exact("%1$s hits you for %d.").as_deref(),
            Some("Ragnaros hits you for 12.")
        );
        assert_eq!(exact("%3$s"), None, "no third argument");
        assert_eq!(exact("%1$d"), None, "text in a number's hole");
        assert_eq!(exact("%2$s"), None, "a number in a text hole");
        assert_eq!(exact("50% off"), None, "a hole it does not know");
        assert_eq!(exact("%s %d %d"), None, "starved after two");
    }

    #[test]
    fn each_hole_takes_its_own_argument() {
        assert_eq!(
            fill(
                "%s has promoted %s to %s.",
                &[Arg::S("A"), Arg::S("B"), Arg::S("Knight")]
            ),
            "A has promoted B to Knight."
        );
    }

    #[test]
    fn starved_and_unknown_specifiers_survive_visibly() {
        assert_eq!(fill("%1$s and %2$s", &[Arg::S("only")]), "only and %2$s");
        assert_eq!(fill("50% off", &[]), "50% off");
        assert_eq!(fill("%q", &[Arg::S("x")]), "%q");
    }

    #[test]
    fn arguments_render_for_whichever_hole_they_meet() {
        assert_eq!(fill("%s/%d", &[Arg::D(3), Arg::D(5)]), "3/5");
        assert_eq!(fill("%d", &[Arg::S("many")]), "many");
    }

    /// `ITEM_MOD_AGILITY`, `ITEM_RESIST_SINGLE` and `DPS_TEMPLATE`, `GlobalStrings.lua:2416`,
    /// `:2441` and `:942`.
    #[test]
    fn the_sign_and_rate_holes_the_item_builder_uses() {
        assert_eq!(
            fill("%c%d Agility", &[Arg::S("+"), Arg::D(9)]),
            "+9 Agility"
        );
        assert_eq!(
            fill(
                "%c%d %s Resistance",
                &[Arg::S("-"), Arg::D(10), Arg::S("Frost")]
            ),
            "-10 Frost Resistance"
        );
        assert_eq!(
            fill("(%.1f damage per second)", &[Arg::F(24.428_571)]),
            "(24.4 damage per second)"
        );
        assert_eq!(fill("%.3f", &[Arg::F(1.5)]), "1.500");
        assert_eq!(fill("%f", &[Arg::F(1.5)]), "1.500000", "C's default is 6");
        assert_eq!(fill("Adds %g damage", &[Arg::F(7.0)]), "Adds 7 damage");
        assert_eq!(fill("Adds %g damage", &[Arg::F(7.5)]), "Adds 7.5 damage");
    }

    #[test]
    fn widths_and_flags_do_not_derail_the_scan() {
        assert_eq!(fill("%5d|%-3s", &[Arg::D(7), Arg::S("x")]), "7|x");
        assert_eq!(fill("%.1f and %.1f", &[Arg::F(1.0)]), "1.0 and %.1f");
        // `%1 ` never reaches a conversion letter, so the `%` stays literal.
        assert_eq!(fill("100%1 of it", &[Arg::D(3)]), "100%1 of it");
    }

    /// `0x771c50`, byte for byte: a number with one `.` and no `,` after it trades them when the
    /// byte after the `.` is at least `0` and the next is at most `9`, so the quirks below are the
    /// reference's, not slips.
    #[test]
    fn number_separators_swap_as_the_bytes_decide() {
        let swapped = swap_number_separators;
        for (text, want) in [
            ("1.5", "1,5"),
            ("1,234.56", "1.234,56"),
            ("12.50 gold", "12,50 gold"),
            ("1.5 to 2.5", "1,5 to 2,5"),
            ("10.5%", "10,5%"),
            // No `.`: unchanged. Two `.`: unchanged. A `,` after the `.`: unchanged.
            ("1,234,567", "1,234,567"),
            ("1.2.3", "1.2.3"),
            ("1.234,5", "1.234,5"),
            // A number starts at a digit, so a leading `.` is text.
            (".5", ".5"),
            ("a.b", "a.b"),
            // The byte after the `.` must be at least `0`: a space, a newline or the end is below.
            ("Level 5. Next", "Level 5. Next"),
            ("5.", "5."),
            ("5.\n", "5.\n"),
            // The second byte after the `.` must be at most `9`: a letter there leaves the number
            // whole (`1.5g`), while a two-digit fraction before a letter swaps (`1.55g`).
            ("1.5g", "1.5g"),
            ("0.5s", "0.5s"),
            ("1.55g", "1,55g"),
            ("1.5 g", "1,5 g"),
            // A letter straight after the `.` passes the first test, so `3.x ` swaps.
            ("3.x ", "3,x "),
            // Bytes of 0x80 and over are below `0` as signed bytes: after the `.` they refuse the
            // swap, and in the second place they pass it.
            ("1.\u{e9}", "1.\u{e9}"),
            ("1.5\u{e9}", "1,5\u{e9}"),
            ("", ""),
        ] {
            assert_eq!(swapped(text), want, "{text:?}");
        }
    }

    /// The pass writes into a 4095-byte static buffer and stops there.
    #[test]
    fn number_separators_stop_at_the_static_buffer() {
        let long = "ab".repeat(3000);
        assert_eq!(swap_number_separators(&long).len(), 4095);
        let numbers = "1.5 ".repeat(2000);
        let out = swap_number_separators(&numbers);
        assert_eq!(out.len(), 4095);
        assert!(out.starts_with("1,5 1,5 "));
    }

    /// The cap cuts at a character boundary: a two-byte character straddling byte 4095 is dropped
    /// whole, never left as a replacement character.
    #[test]
    fn number_separators_cut_at_a_character_boundary() {
        let out = swap_number_separators(&"\u{e9}".repeat(3000));
        assert_eq!(out.len(), 4094);
        assert_eq!(out.chars().count(), 2047);
        assert!(!out.contains('\u{fffd}'));
        let out = swap_number_separators(&format!("1.5 {}", "\u{e9}".repeat(3000)));
        assert!(out.starts_with("1,5 "));
        assert!(out.len() <= 4095 && !out.contains('\u{fffd}'));
    }

    /// `0x771770` and `0x7718b0`: the part follows the sound before the escape, read off the
    /// bytes of the Hangul test, the digit table `0x81c6c4` and the letter table `0x81c6d0`.
    #[test]
    fn the_korean_particle_follows_the_sound_before_it() {
        let x = |t: &str| expand_grammar_escapes(t).into_owned();
        // 책 (U+CC45) has a final consonant: first. 과 (U+ACFC) has none: second.
        assert_eq!(x("\u{CC45}|1\u{C774};\u{AC00};"), "\u{CC45}\u{C774}");
        assert_eq!(x("\u{ACFC}|1\u{C774};\u{AC00};"), "\u{ACFC}\u{AC00}");
        // 길 (U+AE38) ends in ㄹ: the second part only when the first starts with 으.
        assert_eq!(
            x("\u{AE38}|1\u{C73C}\u{B85C};\u{B85C};"),
            "\u{AE38}\u{B85C}"
        );
        assert_eq!(x("\u{AE38}|1\u{C774};\u{AC00};"), "\u{AE38}\u{C774}");
        // Digits: first for 0, 1, 3, 6, 7, 8.
        for (d, first) in [
            ('0', true),
            ('1', true),
            ('2', false),
            ('3', true),
            ('4', false),
        ] {
            assert_eq!(
                x(&format!("{d}|1A;B;")),
                format!("{d}{}", if first { "A" } else { "B" })
            );
        }
        for (d, first) in [
            ('5', false),
            ('6', true),
            ('7', true),
            ('8', true),
            ('9', false),
        ] {
            assert_eq!(
                x(&format!("{d}|1A;B;")),
                format!("{d}{}", if first { "A" } else { "B" })
            );
        }
        // Letters, either case: first for b, c, k, l, m, n, p, q.
        assert_eq!(x("b|1A;B;"), "bA");
        assert_eq!(x("Q|1A;B;"), "QA");
        assert_eq!(x("a|1A;B;"), "aB");
        assert_eq!(x("Z|1A;B;"), "ZB");
        // Spaces and punctuation are passed over; nothing to look at: the first part.
        assert_eq!(x("a !|1A;B;"), "a !B");
        assert_eq!(x("|1A;B;"), "A");
        assert_eq!(x(" |1A;B;"), " A");
        // A unit that follows a bar is passed over: "|c" is not a `c`.
        assert_eq!(x("b|c|1A;B;"), "b|cA");
        // Every escape in the text, and an unfinished one: parts end at `;` or the end.
        assert_eq!(x("1|1A;B; 2|1A;B;"), "1A 2B");
        assert_eq!(x("1|1A"), "1A");
        assert_eq!(x("2|1A"), "2");
        assert_eq!(x("2|1"), "2");
    }

    /// `0x771770`: a bar and the character after it are copied together, so `||` is not a bar
    /// before `1`; and a `1` with a digit after it is another number.
    #[test]
    fn the_korean_escape_leaves_what_is_not_one() {
        for t in ["||1A;B;", "|cFF00FF00x|r", "|15 A;B;", "a|10", "100%", "|"] {
            assert_eq!(expand_grammar_escapes(t), t, "{t:?}");
        }
        // The digits after the bar are one number, wrapping at 32 bits: 4294967297 reads as 1.
        assert_eq!(
            expand_grammar_escapes("b|4294967297A;B;"),
            "b|4294967297A;B;"
        );
        // `|01` reads as 1 but is no escape; the reference loops on it for ever, here it ends.
        assert_eq!(expand_grammar_escapes("|01A;B;"), "|01A;B;");
    }

    /// `0x7719a0`: the word after `|2` turns into its "of the" form.
    #[test]
    fn the_french_article_follows_the_word_after_it() {
        let x = |t: &str| expand_grammar_escapes(t).into_owned();
        assert_eq!(x("Bataille |2Orgrimmar"), "Bataille d'Orgrimmar");
        assert_eq!(x("|2Hurlevent"), "d'Hurlevent");
        assert_eq!(x("|2hache"), "d'hache");
        assert_eq!(x("|2Yeti"), "d'Yeti");
        assert_eq!(x("|2yeti"), "d'yeti");
        assert_eq!(x("|2Yser"), "de Yser");
        assert_eq!(x("|2Mulgore"), "de Mulgore");
        // The spaces after the escape go; the vowel table has the Latin-1 letters and no other.
        assert_eq!(x("|2   Orgrimmar"), "d'Orgrimmar");
        assert_eq!(x("|2\u{C9}cu"), "d'\u{C9}cu");
        assert_eq!(x("|2\u{E8}re"), "d'\u{E8}re");
        assert_eq!(x("|2\u{D1}u"), "de \u{D1}u");
        assert_eq!(x("|2\u{D7}"), "de \u{D7}");
        // L' / Le / La, the last two only before a space.
        assert_eq!(x("|2L'Arche"), "l'Arche");
        assert_eq!(x("|2Le Fort"), "du Fort");
        assert_eq!(x("|2La Forge"), "de la Forge");
        assert_eq!(x("|2Lac"), "de Lac");
        assert_eq!(x("|2Le"), "de Le");
        assert_eq!(x("|2Lune"), "de Lune");
        // The character after the escape is copied without a look, so a bar there is not one
        // in that pass; the rescan from the start then takes the second `|2`.
        assert_eq!(x("|2|2Orgrimmar"), "de d'Orgrimmar");
        // Colour codes and a number after the `2` are left alone.
        assert_eq!(x("|cFFFFFFFF|2Orgrimmar|r"), "|cFFFFFFFFd'Orgrimmar|r");
        assert_eq!(x("|22 gold"), "|22 gold");
        assert_eq!(x("||2Orgrimmar"), "||2Orgrimmar");
        // A text that ends inside the escape ends there.
        assert_eq!(x("a |2"), "a de ");
    }

    /// `0x7716f0` hands on to whichever escape it meets first and starts over on the result.
    #[test]
    fn both_escapes_in_one_text_are_rewritten() {
        let x = |t: &str| expand_grammar_escapes(t).into_owned();
        assert_eq!(x("|2Orgrimmar 2|1A;B;"), "d'Orgrimmar 2B");
        assert_eq!(x("2|1A;B; |2Orgrimmar"), "2B d'Orgrimmar");
        // A part may carry the other escape, which the next pass takes.
        assert_eq!(x("2|1|2Orgrimmar;B;"), "2B");
        assert_eq!(x("1|1|2Orgrimmar;B;"), "1d'Orgrimmar");
    }

    /// The rewrite goes through the `0xcf1cc8` buffer, `0xfff` bytes and the NUL.
    #[test]
    fn the_rewrite_stops_at_the_static_buffer() {
        let t = format!("|2Orgrimmar{}", "ab".repeat(3000));
        let out = expand_grammar_escapes(&t);
        assert_eq!(out.len(), 4095);
        assert!(out.starts_with("d'Orgrimmarab"));
        // Text with no escape is not touched, however long.
        let plain = "ab".repeat(3000);
        assert!(matches!(
            expand_grammar_escapes(&plain),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
