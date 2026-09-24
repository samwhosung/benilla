//! The Lua 5.0 dialect the vendored 5.1 VM restores: the iterator-less generic-for (5.0's
//! `OP_TFORPREP` folded into `OP_TFORLOOP` in `third_party/lua-src/lua-5.1.5/lvm.c`), long-string
//! nesting and the constructor compat-semicolon.

use super::common::script;

#[test]
fn a_table_generator_iterates_like_lua_50() {
    let s = script();
    let seen: i64 = s
        .eval(
            r#"
            local t = { a = 1, b = 2, c = 3 }
            local sum = 0
            for k, v in t do sum = sum + v end
            return sum
        "#,
        )
        .unwrap();
    assert_eq!(seen, 6, "every pair must be visited exactly once");

    // The array shape: the key is the real key, not a counter.
    let keys: String = s
        .eval(
            r#"
            local t = { "x", "y" }
            local out = ""
            for i, v in t do out = out .. i .. v end
            return out
        "#,
        )
        .unwrap();
    assert_eq!(keys, "1x2y");
}

/// The 1.12 handler tests only the type tag, never the metatable, so `__call` is not consulted.
#[test]
fn a_table_with_a_call_metamethod_is_still_iterated_not_called() {
    let s = script();
    let out: String = s
        .eval(
            r#"
            local called = false
            local t = { only = "pair" }
            setmetatable(t, { __call = function() called = true return nil end })
            local keys = ""
            for k, v in t do keys = keys .. k end
            return keys .. "|" .. tostring(called)
        "#,
        )
        .unwrap();
    assert_eq!(
        out, "only|false",
        "the table must be ITERATED and its __call never invoked"
    );
}

/// The callee is a raw read of the global `next` at each loop entry, so an addon can replace it.
#[test]
fn the_substituted_generator_is_the_live_global_next() {
    let s = script();
    let out: String = s
        .eval(
            r#"
            local realnext = next
            local calls = 0
            next = function(t, k) calls = calls + 1 return realnext(t, k) end
            local t = { a = 1, b = 2 }
            local n = 0
            for k, v in t do n = n + 1 end
            next = realnext
            return n .. "|" .. (calls > 0 and "hooked" or "not hooked")
        "#,
        )
        .unwrap();
    assert_eq!(
        out, "2|hooked",
        "a replaced global `next` must drive the loop, as it does on the client"
    );
}

/// Any other generator takes the normal call path, so a number raises on the first iteration.
#[test]
fn only_a_table_is_substituted_and_other_types_still_raise() {
    let s = script();

    let via_pairs: i64 = s
        .eval(
            r#"
            local t = { a = 1, b = 2, c = 3 }
            local n = 0
            for k, v in pairs(t) do n = n + 1 end
            return n
        "#,
        )
        .unwrap();
    assert_eq!(via_pairs, 3);

    let custom: i64 = s
        .eval(
            r#"
            local function upto(state, i)
                i = i + 1
                if i <= state then return i end
            end
            local sum = 0
            for i in upto, 3, 0 do sum = sum + i end
            return sum
        "#,
        )
        .unwrap();
    assert_eq!(
        custom, 6,
        "an explicit generator/state/control triple is untouched"
    );

    let err = s
        .eval::<i64>("local n = 0 for k, v in 42 do n = n + 1 end return n")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("attempt to call a number value"),
        "a non-table, non-function generator must still raise: {err}"
    );
}

#[test]
fn an_empty_table_generator_terminates() {
    let s = script();
    let n: i64 = s
        .eval("local n = 0 for k, v in {} do n = n + 1 end return n")
        .unwrap();
    assert_eq!(n, 0);
}

/// The patch rewrites the loop's own registers, reloaded from the expression at each entry.
#[test]
fn nested_and_repeated_loops_each_substitute_independently() {
    let s = script();
    let out: i64 = s
        .eval(
            r#"
            local outer = { a = 1, b = 2 }
            local inner = { x = 10, y = 20 }
            local total = 0
            for k, v in outer do
                for k2, v2 in inner do total = total + v2 end
                total = total + v
            end
            return total
        "#,
        )
        .unwrap();
    // Each of the 2 outer pairs runs the full inner loop (30) plus its own value.
    assert_eq!(out, 30 + 30 + 1 + 2);

    let twice: i64 = s
        .eval(
            r#"
            local t = { a = 1, b = 2 }
            local function count() local n = 0 for k, v in t do n = n + 1 end return n end
            return count() + count()
        "#,
        )
        .unwrap();
    assert_eq!(twice, 4);
}

/// `[[ ... [[ ... ]] ... ]]` nests in Lua 5.0, as 1.12.1 ships it; the vendored `luaconf.h` sets
/// `LUA_COMPAT_LSTR` to 2, which enables 5.1's own nesting path in `read_long_string`.
#[test]
fn a_nested_long_string_parses_as_lua_50_does() {
    let s = crate::script::UiScript::new().unwrap();
    // The shape an addon writes: a long comment containing a long string.
    s.run("--[[ outer [[ inner ]] still comment ]]\nNESTED_OK = 1")
        .unwrap();
    assert_eq!(s.eval::<i64>("return NESTED_OK").unwrap(), 1);

    // A nested long string keeps its inner brackets: the inner `]]` does not close it.
    let v = s
        .eval::<String>("return [[a [[b]] c]]")
        .expect("nested long string must parse");
    assert_eq!(v, "a [[b]] c");
}

/// Lua 5.0's `constructor()` opens its field loop with `testnext(ls, ';')`, eating one extra `;`
/// per separator position; 5.1 deleted the line, and AtlasLoot's `ButtonRegistry.lua` writes `;;`.
#[test]
fn a_double_semicolon_inside_a_constructor_parses_as_lua_50_does() {
    let s = script();

    let out: String = s
        .eval(
            r#"
            local AL = { Factions = "Factions" }
            local reg = {
                ["REP1"] = { Title = "The Aldor"; Back_Page = "REPMENU"; Back_Title = AL["Factions"];; };
            }
            return reg["REP1"].Back_Title
        "#,
        )
        .expect("the ButtonRegistry shape must parse");
    assert_eq!(out, "Factions");

    // The skip runs at the loop's top, so the lone `;` in `{;}` is eaten too.
    assert_eq!(s.eval::<i64>("local t = {;} return 1").unwrap(), 1);

    let n: i64 = s.eval("local t = { 1;; 2 } return t[1] + t[2]").unwrap();
    assert_eq!(n, 3);
}

/// A third `;` dies in `listfield`, and statement-level `;;` is rejected by the client's own parser
/// too (`luaL_loadbuffer`, `chunk 0x6fcc90`).
#[test]
fn the_compat_semicolon_is_one_wide_and_statement_level_stays_rejected() {
    let s = script();

    let err = s
        .eval::<i64>("local t = { 1;;; 2 } return t[1]")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("unexpected symbol"),
        "a THIRD semicolon must still be a syntax error, as on 5.0: {err}"
    );

    let err = s
        .eval::<i64>("local x = 1;; return x")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("unexpected symbol"),
        "statement-level `;;` must stay rejected — that half is byte-verified: {err}"
    );
}
