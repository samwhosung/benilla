//! The **registered-binding argument/error ABI** — the marshalling contract every one of build
//! 5875's ~700 registered Lua C-bindings opens with, in one place because getting it wrong is
//! wrong 700 times.
//!
//! ## The three helpers the reference uses, and what they accept
//!
//! | addr | contract |
//! |---|---|
//! | `0x6f34d0` | `is-number(L, idx)` — tag 3, **or** a string coerced through `0x6f7c20` |
//! | `0x6f3510` | `is-number-**or**-string(L, idx)` — strictly wider: tag 3 or tag 4 |
//! | `0x6f3620` → `0x40a2b0` | `tonumber` then `double → int32`, **truncating toward zero** |
//!
//! `0x40a2b0` sets the x87 rounding control to chop (`or ah,0xc`) before `fistp`, so it is a C
//! cast and **not** `floor`: `−2.9 → −2`. [`number_arg`] reproduces that (Rust's `as` on a float
//! truncates toward zero as well), and hands back the low dword the bindings actually consume.
//!
//! ## The headline: the `Usage:` arm RAISES
//!
//! `0x6f4940` is `luaL_error`, and it **does not return**: `luaL_where` + `lua_pushvfstring` +
//! `lua_concat` + `lua_error`, whose own chain (`0x6f4440` → `0x6fc780` → `0x6f5d80`) ends in a
//! CRT `longjmp` or `exit(1)` on both legs. The `xor eax,eax; ret` that MSVC emits after every
//! such call inside a binding is **unreachable boilerplate**, not a "returns zero values" arm.
//!
//! So a binding's bad-argument path **abandons the caller's statement** and unwinds to the
//! enclosing protected call. It returns neither `nil` nor zero values, and a client that answers
//! `nil` there keeps executing a statement the real client never finishes.
//!
//! Stated as the negative, because it is as load-bearing as the positive: the **zero Lua values**
//! outcome is real only where a binding reaches `xor eax,eax; ret` *without* passing through
//! `0x6f4940` — `GetDefaultLanguage`'s four object/bounds failure edges are this repo's one
//! example ([`super::session`]). Those arms genuinely return nothing.
//!
//! ## The three shapes an "empty answer" can take
//!
//! Settled per binding, never generalised:
//!
//! 1. **push nil**, one Lua value (`UnitAffectingCombat` on a false/unresolved unit,
//!    `GetActionText` on a non-macro slot, `UnitInRaid` on a miss).
//! 2. **zero Lua values** — distinct from `nil` for anything that counts the return list (5.0's
//!    `arg.n`, or [`crate::script::UiScript::arity`] on the host side) and for a multiple
//!    assignment, identical to it for a single-value caller (`GetDefaultLanguage`'s failure edges).
//! 3. **raise** — the statement is abandoned.

use mlua::{Lua, Value};

/// `is-number` (`0x6f34d0`) → `tonumber` (`0x6f3620`) → `double → int32` truncating toward zero
/// (`0x40a2b0`), with the binding's own `Usage:` string on the failure edge.
///
/// A **missing** argument fails the same test a wrong-typed one does — `0x6f3410` returns NULL
/// past `L->top`, which `0x6f34d0` reports as "not a number" — so `f()` and `f({})` take the same
/// raise. A numeric *string* passes: Lua 5.1's `lua_isnumber` coerces, and so does
/// [`Lua::coerce_number`].
///
/// The `i32` is deliberate: `0x40a2b0` stores a qword but every binding here consumes only `eax`,
/// the low dword. Rust's `i64 as i32` truncates the same way. (Past ±2^63 the two diverge — the
/// x87 stores the "integer indefinite" pattern where Rust saturates — which no caller can reach
/// and no consumer could tell apart, since both answers are garbage.)
pub(crate) fn number_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<i32> {
    match lua.coerce_number(v)? {
        Some(n) => Ok(n as i64 as i32),
        None => Err(mlua::Error::RuntimeError(usage.into())),
    }
}

/// **Shape D** — a `lua_isnumber`-gated *flag* whose failure edge does NOT raise: the local keeps
/// the zero it was seeded with and the binding carries on. `SetInventoryItem`'s optional third
/// argument, the one the image's own usage string (`0x8552dc`) calls `nameOnly`, is this repo's
/// example, and it is stricter than the other shapes on one point: after the
/// `is-number(L, idx)` guard (`0x6f34d0`, so a numeric STRING passes it) the value is compared
/// `> 0.0` **strictly** — `0x53303f fcomp qword [0x802970]` against a literal `0.0`, then
/// `0x533047 test ah,0x41; 0x53304a jne` onto the block that keeps the zero. Masked AH is `0x00`
/// only for greater-than; less (`0x01`), equal (`0x40`) and unordered (`0x41`) all jump. So `0`,
/// a negative and NaN are false, and `1`, `0.5` and `"1"` are true.
///
/// The distinction from [`number_arg`] is the whole point: shape A *raises* `Usage:` on a
/// non-number, shape D shrugs. Which shape an argument takes is per binding.
pub(crate) fn positive_number_flag(lua: &Lua, v: Value) -> mlua::Result<bool> {
    Ok(lua.coerce_number(v)?.is_some_and(|n| n > 0.0))
}

/// **Shape C** — a numeric argument the binding reads with a bare `lua_tonumber 0x6f3620` and NO
/// `lua_isnumber` guard, so it cannot fail: absent, `nil`, `true`, a table, a function, or an
/// unparseable string all land on **`0.0`** and the binding completes.
///
/// This is the counterpart to [`number_arg`] (shape A, which raises `Usage:`), and which shape a
/// given argument takes is **per binding, not a global law** — a census of all 408
/// widget-registrar entries found 110 gated positions against 64 ungated ones. The clustering is
/// the useful part: C is the
/// colour/coordinate tuples (`Set*Color`'s r/g/b, `SetTexCoord`, `SetPosition`), A is the single
/// scalar setters (`SetAlpha`, `SetWidth`, `SetValue`, `SetID`). Do not reach for this one because
/// an argument "looks optional" — check the census.
///
/// One instruction settles why nil and a table are not distinguished: `0x6f7c32 cmp ecx,4;
/// jne 0x6f7c6a` refuses every non-string tag without inspecting the value.
pub(crate) fn coerced_number(lua: &Lua, v: Option<Value>) -> f64 {
    v.and_then(|v| lua.coerce_number(v).ok().flatten())
        .unwrap_or(0.0)
}

/// `is-number-or-string` (`0x6f3510`) → `arg-as-C-string` (`0x6f3690`), with the binding's own
/// `Usage:` string on the failure edge.
///
/// Strictly wider than [`number_arg`]'s guard: a *number* is accepted and stringified, which is
/// why `UnitAffectingCombat(5)` does not raise in the reference — it resolves the token `"5"`,
/// finds nothing, and answers `nil`. Everything else (nil, boolean, table, function) raises.
pub(crate) fn string_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<String> {
    match lua.coerce_string(v)? {
        // Lossy, per decision 1193: a Lua 5.0 string is BYTES and the reference reads it as
        // bytes, so an argument that is not valid UTF-8 costs a glyph here — it does not cost
        // the call. `to_str()` used to raise, which turned one stray byte in one cp1252 addon's
        // literal into a dead handler (2138).
        Some(s) => Ok(s.to_string_lossy()),
        None => Err(mlua::Error::RuntimeError(usage.into())),
    }
}

/// **[`string_arg`]'s shape-C partner**: coerce like the reference, and treat everything else as
/// **absent** rather than raising.
///
/// The pair is 1717's rule applied to string positions — *which* shape a position takes is settled
/// per binding, never generalised — and the model-pane and region constructors are the two ends of
/// it, verified together:
///
/// | position | fetch | a table there |
/// |---|---|---|
/// | `Model:SetModel` arg 1 | `0x6f3510` → `0x6f3690`, result **tested** | **raises** ([`string_arg`]) |
/// | `CreateFontString`/`CreateTexture` `name`, `layer` | `0x6f3510` → `0x6f3690`, result **not** tested | absent — this fn |
/// | `CreateFrame` `name`, `inherits` | `0x6f3690` **unguarded** | absent — this fn |
///
/// **The discriminator is not the argument's type; it is whether the binding TESTS its parser's
/// return.**
///
/// A **number** is accepted and stringified, exactly as `lua_tostring` coerces it — so
/// `CreateFrame("Frame", 5)` names the frame `"5"`. Callers that must *not* coerce a number (the
/// `lua_type(L,idx) == LUA_TSTRING` gate `CreateTexture`/`CreateFontString` put on their **fourth**
/// argument, and only there) test the tag themselves before calling this.
pub(crate) fn optional_string(lua: &Lua, v: &Value) -> Option<String> {
    match v {
        // Lossy, per decision 1193 and for the same reason as [`string_arg`] — with a sharper
        // symptom, because this one's failure was SILENT: `to_str().ok()` folded a string that
        // was not valid UTF-8 into `None`, which every caller here reads as *the argument was
        // not there*. `AddMessage` printed nothing, `CreateFrame` produced an unnamed frame, and
        // nothing anywhere said why (2138).
        Value::String(_) | Value::Number(_) | Value::Integer(_) => lua
            .coerce_string(v.clone())
            .ok()
            .flatten()
            .map(|s| s.to_string_lossy()),
        _ => None,
    }
}

/// **A free-text argument — the player-visible kind — as the reference takes it** (decision 2138).
///
/// The text sinks (`FontString`/`Button`/`EditBox`/`SimpleHTML` `SetText`, `SetFormattedText`,
/// `EditBox:Insert`) took their argument as mlua's `Option<String>`, whose `FromLua` demands valid
/// UTF-8 and **raises** otherwise. Decision 1193 settled that a UI source file is bytes and that
/// "the right place to turn bytes into text is the boundary that actually requires text… A stray
/// byte should cost one glyph, not the file" — and then these boundaries cost the whole call.
/// `strsub` is byte-indexed (1193 keeps it that way deliberately), so *any* addon slicing
/// non-ASCII text hands a sink a broken sequence, and a cp1252 literal is a broken sequence to
/// begin with.
///
/// This is `Option<String>`'s conversion with exactly one arm changed: a **string** becomes its
/// lossy text. Every other type behaves as before — absent and `nil` are `None`, a number
/// coerces, a table or a function raises mlua's own conversion error — so the raise/absent split
/// each binding already had ([`optional_string`]'s table) is untouched.
pub(crate) fn text_arg(lua: &Lua, v: Option<Value>) -> mlua::Result<Option<String>> {
    match v {
        None | Some(Value::Nil) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.to_string_lossy())),
        Some(other) => mlua::FromLua::from_lua(other, lua).map(Some),
    }
}

/// `GetBoolOrDefault` (`0x6f1c10`) — the reference's **boolean argument** coercion, which is *not*
/// Lua truthiness and gets three arms backwards from the obvious reading. The whole jump table
/// sits at `0x6f1ce8`.
///
/// `v` is `None` for an **absent** argument (`LUA_TNONE` = −1, unsigned-compares above the table's
/// bound and takes the default arm). mlua collapses "missing" into `Value::Nil` for a plain
/// `Value` parameter, and the two are *different* here, so a binding whose default is `true` must
/// take its argument as a `MultiValue` to tell them apart.
///
/// | Lua type | result |
/// |---|---|
/// | absent (`None`) | `default` |
/// | `nil` | **false** |
/// | boolean | itself |
/// | lightuserdata | `default` |
/// | number | truncate toward zero (`0x40a2b0`), then `!= 0` |
/// | string | the table below |
/// | table / function / userdata / thread | `default` |
///
/// The **string** arm is the one that bites. It does not parse the number; it dispatches on the
/// **first byte** through the 0x4a-entry remap table at `0x6f1d08` (indexed `byte − 0x30`,
/// *signed*, so anything outside `'0'..='y'` — an empty string's terminator included — falls to the
/// keyword arm):
///
/// - first byte `'0' 'F' 'N' 'f' 'n'` → **false**
/// - first byte `'1'..='9' 'T' 'Y' 't' 'y'` → **true**
/// - anything else → whole-string, case-insensitive (`SStrCmpI 0x64a4c0`): `"off"`/`"disabled"` →
///   false, `"on"`/`"enabled"` → true, and **anything unrecognised takes the `default`**
///
/// So `"0"` is FALSE where Lua truthiness says true (in Lua every string is truthy), which is
/// load-bearing rather than pedantic: 1.12's own Interface panel hands its option bindings the
/// *strings* `"0"`/`"1"`, and a re-implementation wired to `lua_toboolean` inverts every
/// string-valued option in the game. Equally, `"0.5"` and `"-1"` are decided by their first byte
/// alone — false and `default` — never by their numeric value.
/// Lua 5.0's own number→string rule as the 1.12 client compiles it: `sprintf("%.14g")`
/// (`luaV_tostring 0x6f7c80`, format string `0x871960`). Decision 1831.
///
/// This is the whole of `EditBox:SetNumber`'s formatting, because **`SetNumber 0x798690` and
/// `SetText 0x7984c0` are byte-identical functions** — 245 bytes each, zero differences after
/// masking rel32 and absolute-VA operands, differing only in which usage string they carry. The
/// numeric verb does no numeric work of its own; the shared `lua_tostring` marshalling does it all.
///
/// The rule, and the three places it is not Rust's `{}`:
///
/// - **14 significant digits**, then trailing zeros and a bare trailing `.` are dropped.
/// - **Exponential iff `exp < -4 || exp >= 14`** — the C `%g` switch.
/// - **A three-digit exponent, always.** `1e20` prints `1e+020`, not `1e+20`: the exponent is
///   copied from the literal template `"e+000"` and digits are written onto it. This is MSVC's
///   `%g`, not C99's, and it is the single most surprising thing here.
///
/// **What this deliberately does NOT reproduce**, stated so it is not mistaken for an oversight:
/// the reference's printf is not correctly rounded. It generates 17 digits half-up and then
/// re-rounds *that string* to 14, also half-up and with no sticky bit, so it disagrees with a
/// correctly-rounded formatter on roughly 0.05% of doubles (`-0.11594045739607` where a correct
/// one gives `…606`). Reproducing that needs the two-stage decimal path, not a format string, and
/// no consumer we have is sensitive to the fourteenth digit. If one ever is, this is the note that
/// says where to look.
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
        // Three digits minimum, and MORE if the exponent needs them — the template is padded,
        // never truncated.
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

pub(crate) fn bool_or_default(v: Option<&Value>, default: bool) -> bool {
    let Some(v) = v else {
        return default; // LUA_TNONE
    };
    match v {
        Value::Nil => false,
        Value::Boolean(b) => *b,
        Value::LightUserData(_) => default,
        // `lua_tonumber` then `0x40a2b0`, whose RC = chop makes it a C cast; the caller tests the
        // low dword only, which is why the double round-trips through `i64 as i32` (see
        // [`number_arg`]).
        Value::Integer(i) => (*i as i32) != 0,
        Value::Number(n) => (*n as i64 as i32) != 0,
        Value::String(s) => {
            let bytes = s.as_bytes();
            match bytes.first() {
                Some(b'0' | b'F' | b'N' | b'f' | b'n') => false,
                Some(b'1'..=b'9' | b'T' | b'Y' | b't' | b'y') => true,
                // The keyword arm — and the fall-through for every byte the remap table cannot
                // index, which is why an empty string lands here too.
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

/// **The 1.12 predicate return** — `1` for true, `nil` for false, and never a Lua boolean.
///
/// The reference client has essentially no boolean-returning query. Across the 1503 rows
/// `reference/1.12-shapes.tsv` marks `kinds_conf = agree`, exactly two slots in the whole binding
/// surface are `boolean`: `IsPetAttackActive` (which really does call `lua_pushboolean 0x6f39f0`,
/// noted in [`super::pet`]) and Lua's own `rawequal`. Every other `Is*`, `Can*`, `Has*` and every
/// predicate slot of a multi-value answer is `(nil) | (number)`.
///
/// **The difference is invisible to `if x then` and decisive to everything else** — `x == 1`,
/// `x == nil`, `tostring(x)`, arithmetic, a table key, a value round-tripped through a saved
/// variable. That is why it survived so long: the transcribed FrameXML reads every one of these
/// with a plain `if`, so `true` and `1` were interchangeable for as long as *we* wrote the callers.
/// Real 1.12 addons are not our callers — `ColorPickerPlus.lua:121` writes
/// `if IsShiftKeyDown() == 1 then`, and reads a `true` as "not held" (decision 2118).
///
/// Enforced, not remembered: `ui_script::shape_gate::no_query_binding_answers_a_lua_boolean`
/// probes the whole registered query surface and fails on any Lua boolean.
///
/// ## What `predicate` said, folded in (2142)
///
/// This helper had a byte-identical TWIN sixty lines up the same file — `predicate`, 49 call
/// sites to this one's 77 — carrying its own half of the evidence. 2118 consolidated five *local*
/// copies into this one and left that one standing, which is the same drift one level up: a house
/// rule with two homes gets re-derived at each new file, and the file that picks the wrong one is
/// invisible against the others. The twin is gone; everything it recorded is below.
///
///
/// The return-side counterpart to [`bool_or_default`], and the same class of fact: 1.12's widget
/// predicates (1830) and its 29 `Unit*` predicates (2043) do not push booleans. `lua_pushboolean
/// 0x6f39f0` has seven call sites in the whole image; **not one is inside a widget registrar body,
/// and no binding in the 83-entry unit table at `0x850438` calls it at all** — those predicates go
/// through `lua_pushnumber 0x6f3810` (the constant double `1.0`) / `lua_pushnil 0x6f37f0`, exactly
/// one value at every live `ret`.
///
/// **Not a whole-image absolute, and 2048 corrects 2043 for saying so.** `lua_pushboolean` is real
/// and one of its seven callers *is* a registered FrameScript binding: `IsPetAttackActive
/// 0x4be0e0` answers a genuine Lua `true`/`false` and **never nil** (ours matches — see
/// [`super::pet`]). The other six are `rawequal`/`pcall`/`xpcall` in the base table `0x811e28` and
/// two unregistered helpers. So "1.12 has no `lua_pushboolean`" is false; "these two predicate
/// families never reach it" is what the bytes support, and is what this helper encodes.
///
/// **The false leg is settled per body, never inherited from a neighbour.** `IsEnabled 0x7800b0`
/// is number-`1`/number-`0`-and-never-nil (`setne` + `fild`), which is what makes
/// `FriendsFrame.lua:404`'s `== 0` live code — so it does NOT come through here. Inside the unit
/// table itself the *numeric getters* answer the number `0.0` where the predicates answer `nil`
/// (`UnitLevel 0x518144`, `UnitMana 0x5177c8`, `UnitXP 0x5173f5`, ~15 more; `UnitSex 0x517f9f`
/// pushes `2.0`).
///
/// **Truthiness hides the difference and direct comparison inverts it**, which is why this survived
/// so long. `if frame:IsVisible()` reads the same either way; `if frame:IsVisible() == nil` does
/// not — under a boolean a hidden frame answers `false`, and `false == nil` is FALSE, so the caller
/// concludes the frame is visible exactly when it is not. The 1.12 addon corpus has 21 such direct
/// comparisons (`IsVisible() == nil` ×9, `GetChecked() == 1` ×6, `IsVisible() ~= nil` ×4, and one
/// each of `~= 1` / `== 1`) across Questie, AtlasQuest, CT_BagMod, MikScrollingBattleText's options
/// and `_dl`.
///
/// The reference proves its own shape without needing the bytes: stock `UIOptionsFrame.xml:310`
/// saves a checkbox as `SHOW_BUFF_DURATIONS = tostring(this:GetChecked())` and stock
/// `BuffFrame.lua:71` reads it back as `== "1"`. That round-trip only closes if `GetChecked`
/// returns the number 1 — `tostring(true)` is `"true"`, and buff timers would never appear.
///
/// Adopting it is strictly safer than what it replaces: every `if x` and `not x` site reads
/// identically, and only the direct comparisons change — from wrong to right.
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

    /// Every arm of `0x6f1c10`, including the three a plausible implementation gets backwards.
    #[test]
    fn bool_or_default_is_not_lua_truthiness() {
        let lua = lua();
        let st = |t: &str| Value::String(lua.create_string(t).unwrap());

        // Absent takes the DEFAULT; an explicit nil is FALSE. The two are different arms.
        assert!(bool_or_default(None, true));
        assert!(!bool_or_default(None, false));
        assert!(!bool_or_default(Some(&Value::Nil), true));

        // Booleans pass through.
        assert!(bool_or_default(Some(&Value::Boolean(true)), false));
        assert!(!bool_or_default(Some(&Value::Boolean(false)), true));

        // Numbers TRUNCATE toward zero — they do not round. `0.5` is off, not on.
        assert!(!bool_or_default(Some(&Value::Number(0.5)), true));
        assert!(!bool_or_default(Some(&Value::Number(-0.5)), true));
        assert!(!bool_or_default(Some(&Value::Number(-0.999)), true));
        assert!(bool_or_default(Some(&Value::Number(1.5)), false));
        assert!(bool_or_default(Some(&Value::Number(-1.5)), false));
        assert!(!bool_or_default(Some(&Value::Integer(0)), true));
        assert!(bool_or_default(Some(&Value::Integer(7)), false));

        // THE headline: `"0"` is FALSE, where Lua truthiness would say true.
        assert!(!bool_or_default(Some(&st("0")), true));
        assert!(bool_or_default(Some(&st("1")), false));
        // First byte only — `"0.5"` is false because it starts `'0'`, not because it truncates.
        assert!(!bool_or_default(Some(&st("0.5")), true));
        assert!(bool_or_default(Some(&st("9lives")), false));

        // The letter arms.
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

        // The keyword arm, case-insensitive whole-string.
        assert!(!bool_or_default(Some(&st("off")), true));
        assert!(!bool_or_default(Some(&st("OFF")), true));
        assert!(!bool_or_default(Some(&st("Disabled")), true));
        assert!(bool_or_default(Some(&st("on")), false));
        assert!(bool_or_default(Some(&st("ENABLED")), false));

        // Unrecognised strings — and every byte the remap table cannot index — take the DEFAULT.
        for t in ["", "-1", "?", "maybe", "@"] {
            assert!(bool_or_default(Some(&st(t)), true), "{t:?} → default true");
            assert!(
                !bool_or_default(Some(&st(t)), false),
                "{t:?} → default false"
            );
        }

        // Table/function take the default arm, like an absent argument.
        let tbl = Value::Table(lua.create_table().unwrap());
        assert!(bool_or_default(Some(&tbl), true));
        assert!(!bool_or_default(Some(&tbl), false));
    }

    #[test]
    fn number_arg_truncates_toward_zero_not_floor() {
        let lua = lua();
        // `0x40a2b0` sets RC = chop before `fistp` — a C cast, not `floor`.
        assert_eq!(number_arg(&lua, Value::Number(2.9), "u").unwrap(), 2);
        assert_eq!(number_arg(&lua, Value::Number(-2.9), "u").unwrap(), -2);
        assert_eq!(number_arg(&lua, Value::Number(-0.5), "u").unwrap(), 0);
    }

    #[test]
    fn number_arg_accepts_a_numeric_string_and_raises_on_everything_else() {
        let lua = lua();
        let s = lua.create_string("7").unwrap();
        assert_eq!(number_arg(&lua, Value::String(s), "u").unwrap(), 7);
        // Missing and wrong-typed take the SAME arm: `0x6f3410` hands `0x6f34d0` a NULL for an
        // absent argument, which reports "not a number" exactly as a table does.
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
