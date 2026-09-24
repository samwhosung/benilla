//! Fills the `%` holes of a `GlobalStrings.lua` or `GlueStrings.lua` template read at runtime, as
//! the client's `SStrPrintf` does: `%s`, `%d`, `%c`, `%f` and `%g` take the next argument left to
//! right, `%N$s` takes argument N without moving that cursor (`DUEL_WINNER_RETREAT` reorders its
//! two names), `%%` is `%`, and the precision is the template's. A hole with no argument, or one
//! this does not know, is copied through literally so a mis-modelled template shows.

use std::fmt::Write as _;

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

    /// `%f`/`%.Nf`, the precision defaulting to C's 6.
    fn as_f(&self, prec: Option<usize>, out: &mut String) {
        let p = prec.unwrap_or(6);
        match self {
            Arg::F(x) => {
                let _ = write!(out, "{x:.p$}");
            }
            Arg::D(n) => {
                let _ = write!(out, "{:.p$}", *n as f64);
            }
            Arg::S(s) => out.push_str(s),
        }
    }

    /// `%g`/`%.Ng` by C's rule, the precision P counting significant digits: with X the exponent
    /// of the value in `%e` at precision P−1, use `%f` at P−1−X when P > X ≥ −4, else `%e`, then
    /// drop trailing zeros. 1.12's five `%.3g` cast and cooldown templates need it: a 100-second
    /// cooldown is "1.67 min", where Rust's shortest round-trip `{}` prints 17 digits.
    fn as_g(&self, prec: Option<usize>, out: &mut String) {
        let Arg::F(x) = self else {
            return self.as_s(out);
        };
        let p = prec.unwrap_or(6).max(1);
        if *x == 0.0 {
            out.push('0');
            return;
        }
        // Round first, then read the exponent, so 9.999 at P=3 is "10", not "10.0".
        let sci = format!("{:.*e}", p - 1, x);
        let (mantissa, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
        let exp: i32 = exp.parse().unwrap_or(0);
        if exp < -4 || exp >= p as i32 {
            // `%e`: C writes at least two exponent digits and always a sign; Rust writes neither.
            out.push_str(trim_zeros(mantissa));
            let _ = write!(out, "e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
        } else {
            let decimals = (p as i32 - 1 - exp).max(0) as usize;
            out.push_str(trim_zeros(&format!("{x:.decimals$}")));
        }
    }
}

/// `%g`'s last step: drop a fraction's trailing zeros, and the point if nothing is left.
fn trim_zeros(s: &str) -> &str {
    match s.contains('.') {
        true => s.trim_end_matches('0').trim_end_matches('.'),
        false => s,
    }
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

/// `GetText(token, nil, ordinal)`'s plural pick: `0x52fa50` formats the key and `0x703bf0`
/// pcalls the Lua `GetText`, where `GetPluralIndex` (`LocaleProperties.lua:56`) is singular for
/// `not ordinal or ordinal == 1`. So one or no ordinal takes the bare token, anything else the
/// `_P1` twin, falling back to the bare token. Zero is plural: the test at `0x703c89` is signed,
/// only a negative becomes nil, and the number `0` is truthy in Lua.
pub fn plural(
    token: &str,
    ordinal: Option<u32>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if ordinal.is_some_and(|n| n != 1) {
        if let Some(twin) = get(&format!("{token}_P1")).filter(|s| !s.is_empty()) {
            return Some(twin);
        }
    }
    get(token).filter(|s| !s.is_empty())
}

/// Fill a reference template, by the rules in the module doc.
pub fn fill(template: &str, args: &[Arg<'_>]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    let mut next = 0usize; // the sequential cursor
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
            // Unknown: copy the `%` and rescan from the next char, so `50% off` survives.
            out.push('%');
            i += 1;
            continue;
        };
        // A positional does not move the sequential cursor.
        let idx = match spec.positional {
            Some(n) => n.checked_sub(1),
            None => Some(next),
        };
        match idx.and_then(|k| args.get(k)) {
            Some(a) => {
                match spec.conv {
                    's' | 'c' => a.as_s(&mut out),
                    'd' => a.as_d(&mut out),
                    'f' => a.as_f(spec.precision, &mut out),
                    _ => a.as_g(spec.precision, &mut out),
                }
                if spec.positional.is_none() {
                    next += 1;
                }
            }
            // Starved: copy the specifier through so a mis-modelled template shows.
            None => out.extend(&chars[i..spec.end]),
        }
        i = spec.end;
    }
    out
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
        // Outside `P > X >= -4` the style is `%e`, with C's signed two-digit exponent.
        assert_eq!(g("%.3g", 0.000_012_345), "1.23e-05");
        assert_eq!(g("%.3g", 123_456.0), "1.23e+05");
    }

    #[test]
    fn f_takes_the_templates_precision() {
        assert_eq!(
            fill("(%.1f damage per second)", &[Arg::F(41.25)]),
            "(41.2 damage per second)"
        );
        assert_eq!(fill("%.2f", &[Arg::F(41.25)]), "41.25");
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
        assert_eq!(plural("T", Some(1), &table).as_deref(), Some("<one>"));
        assert_eq!(plural("T", Some(2), &table).as_deref(), Some("<many>"));
        assert_eq!(
            plural("T", Some(0), &table).as_deref(),
            Some("<many>"),
            "zero is plural — the signed test, not `> 1`"
        );
        assert_eq!(
            plural("T", None, &table).as_deref(),
            Some("<one>"),
            "no ordinal at all takes GetText's short arm"
        );
        assert_eq!(plural("ONLY", Some(7), &table).as_deref(), Some("<only>"));
        assert_eq!(plural("ABSENT", Some(2), &table), None);
    }

    /// The duel pair, `GlobalStrings.lua:958-959`.
    #[test]
    fn positional_specifiers_reorder_rather_than_consume() {
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
}
