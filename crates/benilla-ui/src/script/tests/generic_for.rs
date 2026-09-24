//! Lua 5.0's iterator-less generic-for, restored in the vendored VM (decision 1215).
//!
//! `for k, v in someTable do` is Lua 5.0 syntax that 5.1 removed at the OPCODE level. 183 of 218
//! corpus addons are reached by it and it is the first session-start error for 60 of them — three
//! quarters of everything that breaks in a running session. The fix is 5.0's `OP_TFORPREP` folded
//! into `OP_TFORLOOP` in `third_party/lua-src/lua-5.1.5/lvm.c`.
//!
//! Each test below pins one detail that was byte-verified in the real 1.12.1 client, because each
//! is a place where a *reasonable* guess diverges from what the client does. The `__call` case is
//! the sharpest: two of the three conditions somebody would naturally write for this behave
//! differently there.

use super::common::script;

/// **The construct works at all** — the whole point, in the shape the corpus uses.
///
/// `Ace.lua`'s is the canonical one and Ace is embedded across half the ecosystem:
/// `for _, lang in self.langs do … end`, with no `pairs` and no iterator function.
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

    // The array shape too, and the KEY must be the real key rather than a counter.
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

/// **The condition is a bare type-tag test, so a table carrying `__call` STILL gets `next`.**
///
/// This is the discriminating case, and the reason the patch does not test "is not callable" or
/// "has no `__call`" — both of those are reasonable readings and both diverge from the client here.
/// The 1.12 handler makes **no** metatable access whatsoever (no `luaT_gettmbyobj`, no
/// `tryfuncTM`), so the `__call` is never consulted and the table is iterated.
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

/// **The callee is the global `next`, read fresh at each loop entry — so an addon can replace it.**
///
/// Observable behaviour of the real client, not an accident: the lookup is a raw read of the
/// thread's globals table, performed once per loop entry. An addon that assigns `next = myfn`
/// changes every later generic-for in the session. Reproduced rather than improved on.
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

/// **Only a table is substituted.** A function generator is untouched (the ordinary 5.1 path), and
/// a non-table, non-function generator still raises on the FIRST ITERATION rather than at entry —
/// the client's tag mismatch falls through to the normal call path.
#[test]
fn only_a_table_is_substituted_and_other_types_still_raise() {
    let s = script();

    // The ordinary iterator form is completely unaffected.
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

    // A custom iterator function still drives its own loop.
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

    // A number generator is not a table: it must still raise, and say so.
    let err = s
        .eval::<i64>("local n = 0 for k, v in 42 do n = n + 1 end return n")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("attempt to call a number value"),
        "a non-table, non-function generator must still raise: {err}"
    );
}

/// **An empty table terminates immediately** rather than looping — `next(t, nil)` on an empty table
/// returns nil, and the loop's own continue test is unchanged by the substitution.
#[test]
fn an_empty_table_generator_terminates() {
    let s = script();
    let n: i64 = s
        .eval("local n = 0 for k, v in {} do n = n + 1 end return n")
        .unwrap();
    assert_eq!(n, 0);
}

/// **Nested and repeated loops each get their own substitution.** The patch rewrites the loop's own
/// registers, so an inner loop must not disturb an outer one, and a loop entered twice must
/// substitute twice (the registers are reloaded from the expression each entry).
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

    // Entering the same loop a second time must work identically.
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

/// **5.0's long-string NESTING, restored** — the second dialect divergence in this file's family.
///
/// `[[ ... [[ ... ]] ... ]]` nests in Lua 5.0, which is what 1.12.1 ships. 5.1 kept the machinery
/// (`llex.c` `read_long_string`'s `cont` counter, guarded by `LUA_COMPAT_LSTR == 2`) and put an
/// advisory error in front of it — *"nesting of [[...]] is deprecated"* — which is a 5.1 opinion
/// about a dialect we are not targeting. Two corpus addons stopped LOADING on it.
///
/// The fix is Lua's own documented switch, `LUA_COMPAT_LSTR` 1 -> 2 in our vendored `luaconf.h`,
/// not a patch to the lexer: the code path was already there and already correct.
#[test]
fn a_nested_long_string_parses_as_lua_50_does() {
    let s = crate::script::UiScript::new().unwrap();
    // The shape an addon writes: a long comment containing a long string.
    s.run("--[[ outer [[ inner ]] still comment ]]\nNESTED_OK = 1")
        .unwrap();
    assert_eq!(s.eval::<i64>("return NESTED_OK").unwrap(), 1);

    // And a nested long STRING keeps its inner brackets, 5.0's own semantics: the outer runs to
    // the matching close, so the inner `]]` does not terminate it.
    let v = s
        .eval::<String>("return [[a [[b]] c]]")
        .expect("nested long string must parse");
    assert_eq!(v, "a [[b]] c");
}

/// **5.0's constructor compat-semicolon, restored** — the third dialect divergence (decision 1315).
///
/// Lua 5.0's `constructor()` opens its field loop with `testnext(ls, ';')  /* compatibility
/// only */` — a Lua 4.0 leftover (4.0 separated a constructor's list part from its record part
/// with `;`) that silently eats ONE extra `;` per separator position inside a table constructor.
/// 5.1 deleted the line. AtlasLoot's `ButtonRegistry.lua` writes `Back_Title = AL["Factions"];;`
/// twenty times; the real client loads it (director-verified on the live 1.12.1 client), stock
/// 5.1 answered "unexpected symbol near ';'" and the addon's registry global never existed (B266).
#[test]
fn a_double_semicolon_inside_a_constructor_parses_as_lua_50_does() {
    let s = script();

    // The exact corpus shape: a record field, its `;` separator, and the compat extra.
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

    // The compat skip runs at loop TOP, so a lone `;` before `}` — legal 5.0, an empty record
    // part in 4.0 terms — is eaten the same way.
    assert_eq!(s.eval::<i64>("local t = {;} return 1").unwrap(), 1);

    // List context too: the separator may be `;` and the extra one is still absorbed.
    let n: i64 = s.eval("local t = { 1;; 2 } return t[1] + t[2]").unwrap();
    assert_eq!(n, 3);
}

/// **The restoration is exactly one semicolon wide, and constructor-only.** Both halves of the
/// boundary are the client's: 5.0 eats ONE extra `;` per position (a third dies in `listfield`),
/// and statement-level `;;` is rejected by 5.0 and 5.1 alike — confirmed by the client's own parser
/// (`luaL_loadbuffer` → `chunk 0x6fcc90`).
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
