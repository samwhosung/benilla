//! The argument and error ABI every registered Lua C-binding of the 1.12 client opens with.
//!
//! The reference's helpers: `is-number` (`0x6f34d0`) accepts tag 3 or a string coerced through
//! `0x6f7c20`; `is-number-or-string` (`0x6f3510`) accepts tag 3 or 4; `tonumber` (`0x6f3620`)
//! feeds `0x40a2b0`, a double-to-int32 cast that truncates toward zero (the x87 rounding control
//! set to chop), so -2.9 is -2.
//!
//! A bad argument raises: `0x6f4940` is `luaL_error`, which never returns (it ends in `longjmp` or
//! `exit(1)` through `0x6f4440`, `0x6fc780`, `0x6f5d80`), so the `xor eax,eax; ret` after it is
//! unreachable and the caller's statement is abandoned. An empty answer is settled per binding:
//!
//! 1. push nil, one value (`UnitAffectingCombat` on a unit out of combat, or a token naming none);
//! 2. zero values, distinct from nil to anything counting the returns (`arg.n`), reached only by a
//!    `xor eax,eax; ret` that skips `0x6f4940` (`GetDefaultLanguage`'s failure edges);
//! 3. raise.

use mlua::{Lua, Value};

/// `_ftol` (`0x40a2b0`) as the bindings consume it: the x87 rounding control set to chop, a
/// `fistp` to a qword, and its low dword. What the qword cannot hold (an infinity, NaN, ±2^63 and
/// beyond) stores the integer-indefinite pattern `0x8000_0000_0000_0000`, whose low dword is 0.
pub(crate) fn ftol(n: f64) -> i32 {
    const TWO_63: f64 = 9_223_372_036_854_775_808.0;
    let t = n.trunc();
    if (-TWO_63..TWO_63).contains(&t) {
        t as i64 as i32
    } else {
        0
    }
}

/// Shape A: `is-number` (`0x6f34d0`), `tonumber` (`0x6f3620`), then the truncating `0x40a2b0`
/// ([`ftol`]), raising the binding's `Usage:` on failure. A missing argument fails like a
/// wrong-typed one (`0x6f3410` returns NULL past `L->top`); a numeric string passes.
pub(crate) fn number_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<i32> {
    match lua.coerce_number(v)? {
        Some(n) => Ok(ftol(n)),
        None => Err(mlua::Error::RuntimeError(usage.into())),
    }
}

/// Shape D: an `is-number`-gated flag whose failure keeps its zero instead of raising. The value
/// must be strictly `> 0.0` (`0x53303f`), so 0, a negative and NaN are false and `0.5` or `"1"`
/// true; `SetInventoryItem`'s `nameOnly` is one.
pub(crate) fn positive_number_flag(lua: &Lua, v: Value) -> mlua::Result<bool> {
    Ok(lua.coerce_number(v)?.is_some_and(|n| n > 0.0))
}

/// Shape C: a bare `lua_tonumber` (`0x6f3620`) with no `is-number` guard, so it cannot fail:
/// anything but a number or numeric string reads as 0.0. The reference uses it for colour and
/// coordinate tuples (`Set*Color`, `SetTexCoord`, `SetPosition`) and shape A for scalar setters
/// (`SetAlpha`, `SetWidth`, `SetValue`, `SetID`); which applies is per binding.
pub(crate) fn coerced_number(lua: &Lua, v: Option<Value>) -> f64 {
    v.and_then(|v| lua.coerce_number(v).ok().flatten())
        .unwrap_or(0.0)
}

/// `is-number-or-string` (`0x6f3510`) then `0x6f3690`, raising the binding's `Usage:` on failure:
/// a number is stringified (`UnitAffectingCombat(5)` passes here, then the unit resolver raises
/// `Unknown unit name` for `"5"`, `0x515c14`), and nil, a boolean, a table or a function raises.
pub(crate) fn string_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<String> {
    match lua.coerce_string(v)? {
        // Lossy: a Lua string is bytes, so invalid UTF-8 costs a glyph, never the call.
        Some(s) => Ok(s.to_string_lossy()),
        None => Err(mlua::Error::RuntimeError(usage.into())),
    }
}

/// [`string_arg`]'s shape-C partner, for a binding that does not test the parse: a string or a
/// number is coerced (`CreateFrame("Frame", 5)` names the frame `"5"`), anything else is absent.
/// `CreateFrame`'s `name` and `inherits` and `CreateFontString`/`CreateTexture`'s `name` and
/// `layer` take this; `Model:SetModel`, which tests it, takes [`string_arg`]. A caller that must
/// refuse a number (the fourth argument of `CreateTexture`/`CreateFontString`) checks the tag.
pub(crate) fn optional_string(lua: &Lua, v: &Value) -> Option<String> {
    match v {
        // Lossy: invalid UTF-8 must not read as an absent argument.
        Value::String(_) | Value::Number(_) | Value::Integer(_) => lua
            .coerce_string(v.clone())
            .ok()
            .flatten()
            .map(|s| s.to_string_lossy()),
        _ => None,
    }
}

/// A free-text argument for the text sinks (`SetText`, `SetFormattedText`, `EditBox:Insert`):
/// `Option<String>`'s conversion, except that a string that is not valid UTF-8 becomes lossy text
/// instead of raising, as the reference takes bytes.
pub(crate) fn text_arg(lua: &Lua, v: Option<Value>) -> mlua::Result<Option<String>> {
    match v {
        None | Some(Value::Nil) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.to_string_lossy())),
        Some(other) => mlua::FromLua::from_lua(other, lua).map(Some),
    }
}

/// Lua's number-to-string as the 1.12 client compiles it: MSVC's `sprintf("%.14g")`
/// (`luaV_tostring` `0x6f7c80`, format `0x871960`). `EditBox:SetNumber` (`0x798690`) formats
/// through it too, being byte-identical to `SetText` (`0x7984c0`). The exponent has at least three
/// digits: `1e20` prints `1e+020`.
///
/// Deviation: the result is correctly rounded, where the reference rounds to 17 digits half-up and
/// then again to 14, differing in the last digit on about 0.05% of doubles, because no consumer
/// reads that digit.
pub(crate) fn lua_number_text(v: f64) -> String {
    if v.is_nan() {
        // MSVC's own spellings, which are not "NaN".
        return if v.is_sign_negative() {
            "-1.#IND".into()
        } else {
            "1.#QNAN".into()
        };
    }
    if v.is_infinite() {
        return if v < 0.0 {
            "-1.#INF".into()
        } else {
            "1.#INF".into()
        };
    }
    if v == 0.0 {
        // `%g` prints negative zero without the sign.
        return "0".into();
    }
    const SIG: i32 = 14;
    let exp = v.abs().log10().floor() as i32;
    // Re-derive the exponent from the rounded form: 9.9999e2 rounds to 1e3 and changes style.
    let exp = {
        let probe = format!("{:.*e}", (SIG - 1) as usize, v);
        probe
            .rsplit('e')
            .next()
            .and_then(|e| e.parse::<i32>().ok())
            .unwrap_or(exp)
    };
    // C's `%g` style rule verbatim: exponential when `exp < -4 || exp >= P`, else fixed.
    if !(-4..SIG).contains(&exp) {
        let mantissa = format!("{:.*e}", (SIG - 1) as usize, v);
        let (m, _) = mantissa.split_once('e').unwrap_or((mantissa.as_str(), "0"));
        let m = trim_g(m);
        let sign = if exp < 0 { '-' } else { '+' };
        // At least three exponent digits, more when needed.
        format!("{m}e{sign}{:03}", exp.abs())
    } else {
        let decimals = (SIG - 1 - exp).max(0) as usize;
        trim_g(&format!("{v:.decimals$}")).to_string()
    }
}

/// Strip `%g`'s trailing zeros, and the decimal point if nothing follows it.
fn trim_g(s: &str) -> &str {
    if !s.contains('.') {
        return s;
    }
    s.trim_end_matches('0').trim_end_matches('.')
}

/// Storm's `SStrToInt` (`0x64ac60`): an optional leading `-` (and no `+`), then the decimal digits
/// up to the first other byte, with no whitespace skip; none is 0. The accumulator is a 32-bit
/// register that wraps (`0x64ac98`-`0x64aca5`) and `neg` wraps too, so `"4294967297"` is 1.
/// `RemoveChatWindowChannel` and `LeaveChannelByName` key on it (`0x49e880`), and the `item:` link
/// fields and `SStrToFloat`'s exponent read it.
pub(crate) fn s_str_to_int(s: &str) -> i64 {
    let b = s.as_bytes();
    let (neg, mut i) = match b.first() {
        Some(b'-') => (true, 1),
        _ => (false, 0),
    };
    let mut n: i32 = 0;
    while let Some(&c) = b.get(i) {
        if !c.is_ascii_digit() {
            break;
        }
        n = n.wrapping_mul(10).wrapping_add(i32::from(c - b'0'));
        i += 1;
    }
    i64::from(if neg { n.wrapping_neg() } else { n })
}

/// Storm's `SStrToFloat` (`0x64aaa0`), which `EditBox:GetNumber` (`0x798790`) pushes: a prefix
/// parse that stops at the first byte outside its grammar and skips no whitespace, evaluated with
/// the x87 at 53-bit precision (`_PC_53`), so every operation below rounds as an `f64` does.
///
/// - an optional `-` (no `+`)
/// - the integer digits, which may be none (`".5"`, `"-"`), summed in chunks: a 32-bit register
///   takes digits until it reaches `0x19999999` (`0x64ab19`), then `value = value * 10^n + chunk`
///   (`0x64ab21`-`0x64ab48`) and a new chunk starts
/// - a `.` is consumed whether or not a digit follows it (`0x64ab9f`: `esi` is advanced before the
///   test), so `"5.e3"` is 5000; each fraction digit `d` at place `k` (0 for the first) then adds
///   the table term `d * 10^-(k+1)` (`0x64abe4`, table `0xc52560` built at `0x64a680`; past place
///   19 the same product is formed on the spot, `0x64abbb`-`0x64abda`), so `"0.12"` is
///   `0.1 + 0.02` = 0.12000000000000001
/// - `e` or `E`, an optional `+`, then [`s_str_to_int`] of what follows scales the value by
///   `pow(10, exp)` (`0x64ac13`-`0x64ac40`): `"1e"` is 1, `"1e+-2"` is 0.01, and the wrapped
///   exponent means `"1e4294967297"` is 10
///
/// Deviation: `pow(10, n)` is the platform's, where the reference's (`0x73f940`, `fyl2x`) rounds
/// its extended-precision steps at 53 bits, so a power or a fraction term can differ in the last
/// place. The reference also keeps the product `value * 10^exp` in the x87's wider exponent range
/// until it is stored, so an exponent past +-308 against a value that pulls it back in range
/// (`"0.000...1e320"`) is reached here only by scaling in steps of 300 digits, which rounds more
/// often; `"0e999"` is 0 as there, and only an exponent past +-4932 makes 0 * inf, NaN.
pub(crate) fn s_str_to_float(s: &str) -> f64 {
    let b = s.as_bytes();
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    let neg = at(0) == b'-';
    let mut i = usize::from(neg);
    let mut value = 0.0f64;
    let (mut chunk, mut count) = (0u32, 0i32);
    while at(i).is_ascii_digit() {
        chunk = chunk * 10 + u32::from(at(i) - b'0');
        count += 1;
        i += 1;
        if chunk >= 0x1999_9999 {
            value = value * 10f64.powf(f64::from(count)) + f64::from(chunk);
            (chunk, count) = (0, 0);
        }
    }
    value = value * 10f64.powf(f64::from(count)) + f64::from(chunk);
    if at(i) == b'.' {
        i += 1;
        let mut place = 1.0;
        while at(i).is_ascii_digit() {
            value += f64::from(at(i) - b'0') * 10f64.powf(-place);
            place += 1.0;
            i += 1;
        }
    }
    if matches!(at(i), b'e' | b'E') {
        i += 1;
        if at(i) == b'+' {
            i += 1;
        }
        value = scale_by_pow10(value, s_str_to_int(&s[i..]) as i32);
    }
    if neg {
        -value
    } else {
        value
    }
}

/// `value * pow(10, exp)` as the x87 forms it: the power keeps the extended exponent range until
/// the product is stored as a double (see [`s_str_to_float`]).
fn scale_by_pow10(value: f64, exp: i32) -> f64 {
    let p = 10f64.powf(f64::from(exp));
    if p.is_finite() && p != 0.0 {
        return value * p;
    }
    if value == 0.0 || !value.is_finite() {
        // 0 * a finite extended power is 0; the power overflows the extended range only past 4932.
        return if exp > 4932 || !value.is_finite() {
            value * p
        } else {
            value * 0.0
        };
    }
    let mut v = value;
    let mut left = exp;
    while left.abs() > 300 {
        v *= 10f64.powf(f64::from(300 * left.signum()));
        left -= 300 * left.signum();
        if v == 0.0 || v.is_infinite() {
            return v;
        }
    }
    v * 10f64.powf(f64::from(left))
}

/// `GetBoolOrDefault` (`0x6f1c10`, jump table `0x6f1ce8`), the reference's boolean argument
/// coercion, which is not Lua truthiness:
///
/// - absent (`None`, `LUA_TNONE`) takes `default`, but nil is false; a binding whose default is
///   true takes a `MultiValue`, since mlua turns a missing argument into nil
/// - a boolean is itself; a number truncates toward zero (`0x40a2b0`), then `!= 0`
/// - a string goes by its first byte, through the remap table at `0x6f1d08`: `'0' 'F' 'N' 'f' 'n'`
///   false, `'1'..='9' 'T' 'Y' 't' 'y'` true; any other is compared whole, case-insensitively
///   (`0x64a4c0`): `"off"`/`"disabled"` false, `"on"`/`"enabled"` true, anything else `default`
/// - lightuserdata, a table, function, userdata or thread takes `default`
///
/// So `"0"` is false and `"0.5"` goes by its first byte: the Interface panel passes its options
/// the strings `"0"` and `"1"`.
pub(crate) fn bool_or_default(v: Option<&Value>, default: bool) -> bool {
    let Some(v) = v else {
        return default; // LUA_TNONE
    };
    match v {
        Value::Nil => false,
        Value::Boolean(b) => *b,
        Value::LightUserData(_) => default,
        // Truncated like `0x40a2b0` ([`ftol`]), which keeps only the low dword: 2^32 reads 0, and
        // an infinity or NaN too.
        Value::Integer(i) => (*i as i32) != 0,
        Value::Number(n) => ftol(*n) != 0,
        Value::String(s) => {
            let bytes = s.as_bytes();
            match bytes.first() {
                Some(b'0' | b'F' | b'N' | b'f' | b'n') => false,
                Some(b'1'..=b'9' | b'T' | b'Y' | b't' | b'y') => true,
                // The keyword arm, also reached by every byte the remap table cannot index (and
                // by an empty string).
                _ => {
                    if bytes.eq_ignore_ascii_case(b"off") || bytes.eq_ignore_ascii_case(b"disabled")
                    {
                        false
                    } else if bytes.eq_ignore_ascii_case(b"on")
                        || bytes.eq_ignore_ascii_case(b"enabled")
                    {
                        true
                    } else {
                        default
                    }
                }
            }
        }
        _ => default,
    }
}

/// The 1.12 predicate return: `1` for true, nil for false, never a Lua boolean. The widget and
/// `Unit*` predicates push the number 1 (`0x6f3810`) or nil (`0x6f37f0`); outside the base library
/// (`rawequal`, `pcall`, `xpcall`), the one binding that calls `lua_pushboolean` (`0x6f39f0`) is
/// `IsPetAttackActive` (`0x4be0e0`). Direct comparisons see it: stock `UIOptionsFrame.xml:310`
/// saves `tostring(this:GetChecked())` and `BuffFrame.lua:71` compares it with `"1"`. The false
/// leg is per binding: `IsEnabled` (`0x7800b0`) and numeric getters such as `UnitLevel`
/// (`0x518144`) answer 0 and do not use this.
pub(crate) fn flag(b: bool) -> Value {
    if b {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        Lua::new()
    }

    /// `SStrToInt` (`0x64ac60`): an optional `-`, digits, nothing skipped, a 32-bit wrap.
    #[test]
    fn s_str_to_int_is_the_references_atoi() {
        assert_eq!(s_str_to_int("2589"), 2589);
        assert_eq!(s_str_to_int("-5"), -5);
        assert_eq!(s_str_to_int(""), 0);
        assert_eq!(s_str_to_int("abc"), 0);
        assert_eq!(s_str_to_int(" 12"), 0, "no leading-whitespace skip");
        assert_eq!(s_str_to_int("+12"), 0, "no unary plus");
        assert_eq!(s_str_to_int("12abc"), 12, "stops at the first non-digit");
        assert_eq!(s_str_to_int("-"), 0);
        assert_eq!(
            s_str_to_int("2147483648"),
            -2_147_483_648,
            "the register wraps"
        );
        assert_eq!(s_str_to_int("4294967297"), 1);
        assert_eq!(s_str_to_int("-4294967297"), -1);
    }

    /// `SStrToFloat` (`0x64aaa0`): the grammar's quirks, each read off the bytes.
    #[test]
    fn s_str_to_float_is_the_references_prefix_parse() {
        for (text, want) in [
            ("12", 12.0),
            ("12abc", 12.0),
            ("-12.5x", -12.5),
            (" 12", 0.0),
            ("+12", 0.0),
            ("inf", 0.0),
            ("1e", 1.0),
            ("1e3", 1000.0),
            ("1E+3", 1000.0),
            ("1e-2", 0.01),
            ("1e+-2", 0.01),
            ("1e3.5", 1000.0),
            (".5", 0.5),
            ("-.5", -0.5),
            ("5.", 5.0),
            ("5.e2", 500.0),
            ("5.5.5", 5.5),
            ("e5", 0.0),
            ("0x1f", 0.0),
            ("1234567890123", 1_234_567_890_123.0),
        ] {
            assert_eq!(s_str_to_float(text), want, "{text:?}");
        }
        // The fraction is a sum of per-digit table terms, not one correctly rounded parse.
        assert_eq!(s_str_to_float("0.12"), 0.1 + 0.02);
        assert_ne!(s_str_to_float("0.12"), 0.12);
        assert_eq!(s_str_to_float("1.5"), 1.5);
        // The exponent is `SStrToInt`'s 32-bit wrap.
        assert_eq!(s_str_to_float("1e4294967297"), 10.0);
        // The integer part sums in chunks of up to ten digits.
        assert_eq!(
            s_str_to_float("12345678901234567890"),
            (1_234_567_890.0 * 1e10) + 1_234_567_890.0
        );
        // The product keeps the x87's wide exponent until it is stored.
        assert_eq!(s_str_to_float("0e999"), 0.0);
        assert!(
            s_str_to_float("0e5000").is_nan(),
            "0 * inf past the extended range"
        );
        assert_eq!(s_str_to_float("1e-999"), 0.0);
        assert_eq!(s_str_to_float("1e999"), f64::INFINITY);
        assert!(s_str_to_float("-").is_sign_negative(), "'-' alone is -0");
    }

    /// `_ftol`'s low dword: chopped toward zero, wrapped to 32 bits, and 0 for whatever the qword
    /// `fistp` cannot hold.
    #[test]
    fn ftol_keeps_the_low_dword_of_the_x87_qword() {
        assert_eq!(ftol(-2.9), -2);
        assert_eq!(ftol(2.9), 2);
        assert_eq!(ftol(3e9), 3_000_000_000_i64 as i32);
        assert_eq!(ftol(4_294_967_301.0), 5);
        assert_eq!(ftol(-9_223_372_036_854_775_808.0), 0);
        for none in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 9.3e18, -9.3e18] {
            assert_eq!(ftol(none), 0, "{none}");
        }
    }

    #[test]
    fn bool_or_default_is_not_lua_truthiness() {
        let lua = lua();
        let st = |t: &str| Value::String(lua.create_string(t).unwrap());

        // Absent takes the default; an explicit nil is false.
        assert!(bool_or_default(None, true));
        assert!(!bool_or_default(None, false));
        assert!(!bool_or_default(Some(&Value::Nil), true));

        assert!(bool_or_default(Some(&Value::Boolean(true)), false));
        assert!(!bool_or_default(Some(&Value::Boolean(false)), true));

        // Numbers truncate toward zero: `0.5` is off.
        assert!(!bool_or_default(Some(&Value::Number(0.5)), true));
        assert!(!bool_or_default(Some(&Value::Number(-0.5)), true));
        assert!(!bool_or_default(Some(&Value::Number(-0.999)), true));
        assert!(bool_or_default(Some(&Value::Number(1.5)), false));
        assert!(bool_or_default(Some(&Value::Number(-1.5)), false));
        // Only the low dword of the truncation counts (`0x40a2b0`): 2^32 reads 0, and so does an
        // infinity or NaN, which the `fistp` stores as the integer indefinite.
        assert!(!bool_or_default(
            Some(&Value::Number(4_294_967_296.0)),
            true
        ));
        assert!(bool_or_default(
            Some(&Value::Number(4_294_967_297.0)),
            false
        ));
        for n in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 9.3e18] {
            assert!(!bool_or_default(Some(&Value::Number(n)), true), "{n}");
        }
        assert!(!bool_or_default(Some(&Value::Integer(0)), true));
        assert!(bool_or_default(Some(&Value::Integer(7)), false));

        // `"0"` is false, where Lua truthiness says true.
        assert!(!bool_or_default(Some(&st("0")), true));
        assert!(bool_or_default(Some(&st("1")), false));
        // First byte only: `"0.5"` is false because it starts with `'0'`.
        assert!(!bool_or_default(Some(&st("0.5")), true));
        assert!(bool_or_default(Some(&st("9lives")), false));

        for t in ["false", "F", "no", "NIL", "nope"] {
            assert!(
                !bool_or_default(Some(&st(t)), true),
                "{t} starts F/N → false"
            );
        }
        for t in ["true", "T", "yes", "Yup"] {
            assert!(
                bool_or_default(Some(&st(t)), false),
                "{t} starts T/Y → true"
            );
        }

        // The keyword arm, whole-string and case-insensitive.
        assert!(!bool_or_default(Some(&st("off")), true));
        assert!(!bool_or_default(Some(&st("OFF")), true));
        assert!(!bool_or_default(Some(&st("Disabled")), true));
        assert!(bool_or_default(Some(&st("on")), false));
        assert!(bool_or_default(Some(&st("ENABLED")), false));

        // Unrecognised strings, and every byte the remap table cannot index, take the default.
        for t in ["", "-1", "?", "maybe", "@"] {
            assert!(bool_or_default(Some(&st(t)), true), "{t:?} → default true");
            assert!(
                !bool_or_default(Some(&st(t)), false),
                "{t:?} → default false"
            );
        }

        // A table takes the default, like an absent argument.
        let tbl = Value::Table(lua.create_table().unwrap());
        assert!(bool_or_default(Some(&tbl), true));
        assert!(!bool_or_default(Some(&tbl), false));
    }

    #[test]
    fn number_arg_truncates_toward_zero_not_floor() {
        let lua = lua();
        assert_eq!(number_arg(&lua, Value::Number(2.9), "u").unwrap(), 2);
        assert_eq!(number_arg(&lua, Value::Number(-2.9), "u").unwrap(), -2);
        assert_eq!(number_arg(&lua, Value::Number(-0.5), "u").unwrap(), 0);
    }

    #[test]
    fn number_arg_accepts_a_numeric_string_and_raises_on_everything_else() {
        let lua = lua();
        let s = lua.create_string("7").unwrap();
        assert_eq!(number_arg(&lua, Value::String(s), "u").unwrap(), 7);
        // Missing and wrong-typed arguments take the same raise.
        for v in [Value::Nil, Value::Boolean(true)] {
            let err = number_arg(&lua, v, "Usage: Thing(n)").unwrap_err();
            assert!(format!("{err}").contains("Usage: Thing(n)"));
        }
    }

    #[test]
    fn string_arg_accepts_a_number_and_raises_on_nil() {
        let lua = lua();
        assert_eq!(string_arg(&lua, Value::Integer(5), "u").unwrap(), "5");
        let err = string_arg(&lua, Value::Nil, "Usage: Thing(\"s\")").unwrap_err();
        assert!(format!("{err}").contains("Usage: Thing(\"s\")"));
    }
}
