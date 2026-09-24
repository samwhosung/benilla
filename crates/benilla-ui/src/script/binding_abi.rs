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

/// Shape A: `is-number` (`0x6f34d0`), `tonumber` (`0x6f3620`), then the truncating `0x40a2b0`,
/// raising the binding's `Usage:` on failure. A missing argument fails like a wrong-typed one
/// (`0x6f3410` returns NULL past `L->top`); a numeric string passes. `0x40a2b0` stores a qword but
/// the bindings consume only the low dword, hence `i32`.
///
/// Deviation: past ±2^63 Rust saturates where the x87 stores its integer-indefinite pattern,
/// because no real argument reaches that range.
pub(crate) fn number_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<i32> {
    match lua.coerce_number(v)? {
        Some(n) => Ok(n as i64 as i32),
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
        // Truncated like `0x40a2b0`, testing only the low dword.
        Value::Integer(i) => (*i as i32) != 0,
        Value::Number(n) => (*n as i64 as i32) != 0,
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
