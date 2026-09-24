//! **One faithful substitution for reference string templates** — the client's `SStrPrintf` face.
//!
//! Every user-visible sentence benilla shows comes from `GlobalStrings.lua` or `GlueStrings.lua`,
//! and most of those carry `%s`/`%d` holes the caller fills. Doing that filling
//! correctly is not `format!`'s job and never was: the template is *data read at runtime from the
//! player's install*, so the holes have to be walked, not compiled.
//!
//! **Why this module exists.** Before it, the substitution had been reinvented at least eight
//! times across the workspace, with semantics that did not agree:
//!
//! - `ui_instance::fill_template` — ordered, `%%`-aware, starvation-safe, and tested. The best of
//!   them, and the one this is derived from.
//! - `ui_duel::winner_line` — a hand-rolled `%1$s`/`%2$s` positional pass, because the duel's
//!   retreat wording swaps its two names and no ordered filler can express that.
//! - `ui_action::errors::ui_error_text` — `str::replace`, which fills **every** `%s` with the
//!   *same* argument. Latent rather than live (no message it currently carries has two), but it
//!   was a real trap waiting for the first two-argument template to reach that queue.
//! - `ui_guild::fill`, `ui_petition::fill`, and one-off `replacen` calls in `ui_binder`,
//!   `ui_trade`, `ui_items::feed` and `equip_error`.
//!
//! Eight copies of one primitive is how the `ui_error_text` bug survived: there was no single
//! place where "how do we fill a reference template" could be got right once.
//!
//! **The semantics, and why each is what it is.**
//!
//! - `%s` and `%d` consume the next argument, **left to right**. This is `SStrPrintf`'s own order.
//! - `%N$s` / `%N$d` take argument `N` (1-based) and do **not** move the sequential cursor. 1.12
//!   uses these exactly where a translation needs to reorder the holes —
//!   `DUEL_WINNER_RETREAT = "%2$s has fled from %1$s in a duel"` is the canonical case, and it is
//!   also why hardcoding an English sentence is a localization bug and not merely untidy.
//! - `%%` collapses to one `%`.
//! - **A specifier whose argument is missing is copied through literally.** A template we
//!   mis-modelled should look wrong, not look plausible — `ui_instance` established this and it is
//!   the right call: a visibly broken line gets reported, a quietly wrong one does not.
//! - Any other specifier is copied through untouched.
//!
//! **`%c` and `%f`/`%g` are here because 1.12's own tables use them**, not for completeness. The
//! item tooltip's stat and resistance families are spelled with a *character* hole for the sign —
//! `ITEM_MOD_AGILITY = "%c%d Agility"`, `ITEM_RESIST_SINGLE = "%c%d %s Resistance"` — and its
//! rate lines with a float one: `DPS_TEMPLATE = "(%.1f damage per second)"`,
//! `AMMO_DAMAGE_TEMPLATE = "Adds %g damage per second"`. A filler that only knew `%s`/`%d` would
//! copy those holes through visibly, which is the same as not being able to show the line.
//!
//! **The precision is the template's, never the caller's** — `%.1f` is one decimal because
//! `DPS_TEMPLATE` says so, and a locale that respells it to `%.2f` gets two without a code change.
//! That is the whole point of reading the template at runtime.

use std::fmt::Write as _;

/// One argument to [`fill`]. Every hole renders every variant — `%s` as text, `%d` as an integer,
/// `%f`/`%g` as a real — so a caller that groups its arguments differently from the template still
/// fills in the order the template asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arg<'a> {
    /// A string argument — a player name, an item link, a zone. Also what a `%c` hole takes: the
    /// only `%c` 1.12 spells is the `+`/`-` sign char of the `ITEM_MOD_*`/`ITEM_RESIST_*` family,
    /// and a one-character string is how a caller says that.
    S(&'a str),
    /// A numeric argument.
    D(i64),
    /// A real argument — the rate lines (`DPS_TEMPLATE`, the `AMMO_*` damage templates).
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
    /// `%s` — and `%c`, whose only 1.12 use is a one-character sign string.
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
            // A `%d` handed a string is a caller bug, not a display decision; showing the string
            // is strictly more useful than showing nothing and matches what varargs would do with
            // a pointer-sized value far better than a zero would.
            Arg::S(s) => out.push_str(s),
            // C truncates toward zero here; so does `as`.
            Arg::F(x) => {
                let _ = write!(out, "{}", *x as i64);
            }
        }
    }

    /// `%f` / `%.Nf` — the precision is the template's, defaulting to C's 6.
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

    /// `%g` / `%.Ng` — C's rule, **including the precision**, which is significant digits here
    /// rather than decimal places.
    ///
    /// The precision is not decoration on this one. Five 1.12 templates are spelled `%.3g` —
    /// `SPELL_CAST_TIME_SEC`, `SPELL_CAST_TIME_MIN`, `SPELL_RECAST_TIME_SEC`,
    /// `SPELL_RECAST_TIME_MIN`, `SPELL_CAST_TIME_RANGED` — and the numbers reaching them are
    /// divisions: a 100-second cooldown is 1.666… minutes. Under C that is "1.67 min cooldown".
    /// Under Rust's shortest round-trip `{}` it is "1.6666666666666667 min cooldown", which is
    /// why `{}` cannot stand in for `%g` in general even though it agrees with it on the one- and
    /// two-decimal values the ammo templates carry.
    ///
    /// C's rule, from the standard: let X be the exponent of the value written in `%e` style at
    /// precision P−1; use `%f` with precision P−1−X when P > X ≥ −4, and `%e` otherwise; then
    /// drop trailing zeros from the fraction, and the point with them if nothing is left. Taking
    /// X *after* the rounding is what makes 9.999 at P=3 read "10" rather than "10.0".
    fn as_g(&self, prec: Option<usize>, out: &mut String) {
        let Arg::F(x) = self else {
            return self.as_s(out);
        };
        let p = prec.unwrap_or(6).max(1);
        if *x == 0.0 {
            out.push('0');
            return;
        }
        // Round first, then read the exponent off the result — the standard's own order.
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

/// Drop a fraction's trailing zeros, and the point with them if nothing survives — `%g`'s last
/// step, and the reason "1.50" prints as "1.5" and "2.00" as "2".
fn trim_zeros(s: &str) -> &str {
    match s.contains('.') {
        true => s.trim_end_matches('0').trim_end_matches('.'),
        false => s,
    }
}

/// One parsed `%…` specifier: where it ends, which argument it names, how precise, and what
/// conversion. Split out because the scan now has four optional parts before the conversion
/// letter and inlining it made the fill loop unreadable.
struct Spec {
    /// 1-based `%N$` argument, when the specifier is positional.
    positional: Option<usize>,
    /// `.N`, when the specifier carries one.
    precision: Option<usize>,
    conv: char,
    /// Index just past the conversion letter.
    end: usize,
}

/// Scan `[N$][flags][width][.prec]conv` starting just after a `%`. `None` = not a conversion this
/// module knows, which the caller copies through untouched.
fn parse_spec(chars: &[char], after_percent: usize) -> Option<Spec> {
    let mut i = after_percent;
    let digits = |i: &mut usize| {
        let start = *i;
        while chars.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
        chars[start..*i].iter().collect::<String>()
    };
    // `%N$…` — positional. Bare digits without the `$` are a width, so this only commits when the
    // `$` is actually there.
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

/// **The other half of decision 2045**: a reference string off the VM's own globals, `None` when
/// the key is absent **or empty**. `load_global_strings` runs the player's own
/// `GlobalStrings.lua` — and the `Localize()` patch over it — into this VM at boot, so this is
/// the whole of "read the sentence from the install".
///
/// **Empty counts as missing, and that is the reference's own test, not a convenience.**
/// `FrameScript_GetText` hands back a pre-seeded empty string (`0x882748`) for a key the table
/// does not carry, and the callers that care test both — `GetPVPRankInfo` checks the pointer at
/// `0x51aa1c` and then its first byte at `0x51aa20`. A caller here gets `None` for both, and the
/// house rule is that **`None` emits no line at all rather than an invented one** (the
/// disposition [`crate::script`]'s `duration_text` took, and 0620 §5 before it).
///
/// Spelled `globals().get::<String>` deliberately: that is the shape
/// `benilla-app/tests/reference_strings.rs` recognises as a key lookup, so a function that
/// resolves through this is visible to the tripwire as resolving.
pub fn global(lua: &mlua::Lua, key: &str) -> Option<String> {
    // Bound before the filter so the lookup stays one unbroken `globals().get::<String>` — a
    // rustfmt-split chain is a resolver the tripwire's substring match cannot see.
    let value = lua.globals().get::<String>(key).ok();
    value.filter(|s| !s.is_empty())
}

/// **`GetText(token, nil, ordinal)`'s plural pick**, resolved: the bare token for exactly one, the
/// `_P1` twin for anything else — **zero included** — and `GetText`'s own fall-back to the bare
/// token when the twin is absent.
///
/// **Byte-pinned, and the `> 1` reading is refuted.** `0x52fa50` snprintf's the bare key, then
/// `0x703bf0` looks up the Lua global `GetText` and pcalls it with `(key, ordinal, gender)`; the
/// predicate is `LocaleProperties.lua`'s `GetPluralIndex`, singular iff `not ordinal or
/// ordinal == 1`, and `703c89: 7d 07` is a **signed** test — a zero ordinal is pushed as the
/// number `0`, which Lua reads as truthy, so it takes the plural arm. Only a negative becomes
/// nil. Had zero mapped to nil, a lapsing aura would read "0 second remaining".
///
/// Lives here, beside [`fill`], because it is the second half of the same job and it had grown
/// **four** private copies that had to agree and could not be made to: the aura ladder's, the
/// instance lockouts', the talent pane's, and one inlined in the death screen's sickness timer.
/// That is the shape decision 2045 wrote about — the first `fill` had eight.
///
/// `None` = neither the twin nor the bare token resolves, which is the reference's
/// data-suppression face and means **no line at all** rather than an invented one.
pub fn plural(
    token: &str,
    ordinal: Option<u32>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    // `not ordinal or ordinal == 1` is the SINGULAR arm, so an absent ordinal takes the bare
    // token — the same `GetText(token)` short arm a nil `genderTag` sends the reference down.
    if ordinal.is_some_and(|n| n != 1) {
        if let Some(twin) = get(&format!("{token}_P1")).filter(|s| !s.is_empty()) {
            return Some(twin);
        }
    }
    get(token).filter(|s| !s.is_empty())
}

/// Fill a reference template. See the module doc for the rules.
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
        // `%%`
        if chars.get(i + 1) == Some(&'%') {
            out.push('%');
            i += 2;
            continue;
        }
        let Some(spec) = parse_spec(&chars, i + 1) else {
            // Not a conversion we know — copy the `%` and rescan from the next char, so `50% off`
            // survives intact.
            out.push('%');
            i += 1;
            continue;
        };
        // A positional names its argument outright and does NOT move the sequential cursor.
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
            // starved — copy the whole specifier through so a mis-modelled template looks wrong
            None => out.extend(&chars[i..spec.end]),
        }
        i = spec.end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ui_instance::fill_template`'s own contract, which this subsumes — ordered, starvation-safe,
    /// `%%`-aware. These are its test's cases verbatim, so the migration cannot change behaviour.
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

    /// **`%.3g` is significant digits, and the precision is load-bearing.** Five 1.12 templates
    /// carry it, all of them fed by a division: `SPELL_RECAST_TIME_MIN = "%.3g min cooldown"`
    /// against a 100-second cooldown is 1.666… minutes. Rust's shortest round-trip `{}` — the
    /// obvious stand-in for `%g`, and what this filler shipped with for a day — writes all
    /// seventeen digits of that. C writes three significant ones.
    #[test]
    fn g_is_significant_digits_and_drops_trailing_zeros() {
        let g = |t: &str, v: f64| fill(t, &[Arg::F(v)]);
        assert_eq!(g("%.3g min cooldown", 100.0 / 60.0), "1.67 min cooldown");
        assert_eq!(g("%.3g sec cast", 1.5), "1.5 sec cast");
        // Trailing zeros go, and the point with them — 2.00 is "2", not "2.00".
        assert_eq!(g("%.3g sec cast", 2.0), "2 sec cast");
        // The exponent is taken AFTER the rounding, which is what makes this "10" and not "10.0".
        assert_eq!(g("%.3g", 9.999), "10");
        // A bare `%g` is C's default precision of six.
        assert_eq!(g("%g", 1.0 / 3.0), "0.333333");
        // Outside `P > X >= -4` the style is `%e`, with C's signed two-digit exponent.
        assert_eq!(g("%.3g", 0.000_012_345), "1.23e-05");
        assert_eq!(g("%.3g", 123_456.0), "1.23e+05");
    }

    /// `%f` takes the template's precision, not the caller's — `DPS_TEMPLATE` is one decimal
    /// because the shipped string says `%.1f`, and a locale that respells it gets two for free.
    #[test]
    fn f_takes_the_templates_precision() {
        assert_eq!(
            fill("(%.1f damage per second)", &[Arg::F(41.25)]),
            "(41.2 damage per second)"
        );
        assert_eq!(fill("%.2f", &[Arg::F(41.25)]), "41.25");
    }

    /// **The plural pick's zero, which is the half people get wrong.** `GetText`'s predicate is
    /// `not ordinal or ordinal == 1` — so an ABSENT ordinal is singular, and a zero one is
    /// **plural**, because `703c89` is a signed test and Lua reads the number `0` as truthy. The
    /// tempting `> 1` reading makes a lapsing aura read "0 second remaining".
    #[test]
    fn plural_takes_the_twin_for_zero_and_the_bare_token_for_none() {
        let table = |key: &str| -> Option<String> {
            // Deliberately not the shipped wording: which key is reached is the whole question.
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
        // A token with no `_P1` twin falls back to itself, at any count.
        assert_eq!(plural("ONLY", Some(7), &table).as_deref(), Some("<only>"));
        // Neither resolving is no line at all, never an invented one.
        assert_eq!(plural("ABSENT", Some(2), &table), None);
    }

    /// **The case an ordered filler cannot express**, and the reason positional specifiers exist:
    /// 1.12's duel pair reorders the same two names, so filling them left-to-right names the wrong
    /// winner. Both templates are quoted from GlobalStrings (958/959).
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
        // A positional may repeat an argument, which no cursor-based fill can do.
        assert_eq!(fill("%1$s vs %1$s", &[a]), "Alice vs Alice");
    }

    /// The bug that eight copies of this hid: `str::replace` fills every `%s` with the *same*
    /// argument. Two holes must take two arguments.
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

    /// A starved positional is copied through too, and an unknown specifier is left alone.
    #[test]
    fn starved_and_unknown_specifiers_survive_visibly() {
        assert_eq!(fill("%1$s and %2$s", &[Arg::S("only")]), "only and %2$s");
        assert_eq!(fill("50% off", &[]), "50% off");
        assert_eq!(fill("%q", &[Arg::S("x")]), "%q");
    }

    /// Numbers reach `%s` as digits and strings reach `%d` as themselves — a caller whose argument
    /// grouping differs from the template's still fills in the template's order.
    #[test]
    fn arguments_render_for_whichever_hole_they_meet() {
        assert_eq!(fill("%s/%d", &[Arg::D(3), Arg::D(5)]), "3/5");
        assert_eq!(fill("%d", &[Arg::S("many")]), "many");
    }

    /// The item tooltip's own two exotic holes, quoted from GlobalStrings (2416, 2441, 942): a
    /// `%c` sign character and a `%.1f` rate. Both consume an argument like any other hole — the
    /// bug they replace is a filler that copied them through and showed the player `"%c+5"`.
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
        // The PRECISION is the template's: respelling it changes the line with no code change.
        assert_eq!(fill("%.3f", &[Arg::F(1.5)]), "1.500");
        assert_eq!(fill("%f", &[Arg::F(1.5)]), "1.500000", "C's default is 6");
        // `%g` drops the trailing zeros the ammo templates would otherwise show.
        assert_eq!(fill("Adds %g damage", &[Arg::F(7.0)]), "Adds 7 damage");
        assert_eq!(fill("Adds %g damage", &[Arg::F(7.5)]), "Adds 7.5 damage");
    }

    /// A width or flag between the `%` and the letter is skipped rather than breaking the scan,
    /// and a starved `%.1f` still copies through whole.
    #[test]
    fn widths_and_flags_do_not_derail_the_scan() {
        assert_eq!(fill("%5d|%-3s", &[Arg::D(7), Arg::S("x")]), "7|x");
        assert_eq!(fill("%.1f and %.1f", &[Arg::F(1.0)]), "1.0 and %.1f");
        // A bare `%` before a digit that never reaches a conversion letter is still just a `%`.
        assert_eq!(fill("100%1 of it", &[Arg::D(3)]), "100%1 of it");
    }
}
