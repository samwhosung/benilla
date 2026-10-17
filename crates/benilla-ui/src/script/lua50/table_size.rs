//! Lua 5.0's remembered table size and the `n`-based table library, as the script host answers
//! them: benilla-lua's natives for the client's `ltablib.c` (`0x822d40`) and base `unpack`
//! (`0x7030a0`).
//!
//! `luaL_setn` (`0x6f4ea0`) writes `t.n` only when a non-negative numeric `t.n` exists, else it
//! stores the size where no walk can see it (`getsizes` `0x6f4fc0`). `luaL_getn` (`0x6f5050`)
//! reads `t.n`, then that store, then counts `rawgeti` from 1 to the first nil. The store must
//! stay invisible: a stray `n` key breaks mixin walks such as `AceOO-2.0`'s, which error on any
//! field the target already has.

use crate::script::UiScript;

/// Mik's Scrolling Battle Text 4.43's capture tables: wipe, `setn(t, 0)`, then insert.
#[test]
fn a_wipe_then_setn_zero_then_insert_is_countable_again() {
    let s = UiScript::new().unwrap();
    assert_eq!(
        s.eval::<i64>(
            r#"
            local t = { "stale", "stale" }
            for k in pairs(t) do t[k] = nil end
            table.setn(t, 0)
            table.insert(t, "Kobold Vermin")
            table.insert(t, "12")
            return table.getn(t)
            "#
        )
        .unwrap(),
        2,
        "insert must update the remembered size, or getn answers the wiped 0 forever"
    );
}

#[test]
fn the_remembered_size_round_trips_and_never_appears_as_a_key() {
    let s = UiScript::new().unwrap();
    assert_eq!(s.eval::<i64>("return table.getn({1,2,3})").unwrap(), 3);
    assert_eq!(
        s.eval::<i64>("local t = {1,2,3} table.setn(t, 7) return table.getn(t)")
            .unwrap(),
        7
    );
    assert_eq!(
        s.eval::<i64>(
            "local t = {1,2,3} table.setn(t, 0) \
             local c = 0 for k in pairs(t) do c = c + 1 end return c"
        )
        .unwrap(),
        3,
        "a remembered size must not appear as a key"
    );
    // A table with a numeric `n` keeps using it: 5.0's first branch, which `arg.n` relies on.
    assert_eq!(
        s.eval::<i64>("local t = {1,2,3, n=3} table.setn(t, 7) return t.n")
            .unwrap(),
        7
    );
    // A negative `n` fails `checkint` and selects the side store (`0x6f4ee4`'s `jl`).
    assert_eq!(
        s.eval::<i64>("local t = {1,2, n=-1} table.setn(t, 5) return table.getn(t) + t.n")
            .unwrap(),
        4
    );
}

#[test]
fn the_fallback_count_stops_at_the_first_hole() {
    let s = UiScript::new().unwrap();
    assert_eq!(
        s.eval::<i64>("local t = {} t[1] = 'a' t[3] = 'c' return table.getn(t)")
            .unwrap(),
        1
    );
    assert_eq!(s.eval::<i64>("return table.getn({})").unwrap(), 0);
}

#[test]
fn insert_and_remove_maintain_the_size_the_way_the_reference_does() {
    let s = UiScript::new().unwrap();
    // A position past the remembered size grows it (`if (pos > n) n = pos`).
    assert_eq!(
        s.eval::<String>(
            "local t = {} table.insert(t, 3, 'c') \
             return table.getn(t) .. ':' .. tostring(t[3])"
        )
        .unwrap(),
        "3:c"
    );
    assert_eq!(
        s.eval::<String>(
            "local t = {'a','c'} table.insert(t, 2, 'b') \
             return table.concat(t, '') .. ':' .. table.getn(t)"
        )
        .unwrap(),
        "abc:3"
    );
    assert_eq!(
        s.eval::<String>(
            "local t = {'a','b','c'} local got = table.remove(t, 1) \
             return got .. table.concat(t, '') .. ':' .. table.getn(t)"
        )
        .unwrap(),
        "abc:2"
    );
    // At size 0, no value and no change (`if (n <= 0) return 0`).
    assert_eq!(
        s.eval::<i64>(
            "local t = {1,2,3} table.setn(t, 0) \
             local got = table.remove(t) \
             if got ~= nil then return -1 end return table.getn(t) + t[1]"
        )
        .unwrap(),
        1
    );
}

#[test]
fn concat_sort_foreachi_and_unpack_all_read_the_remembered_size() {
    let s = UiScript::new().unwrap();
    assert_eq!(
        s.eval::<String>("local t = {'a','b','c'} table.setn(t, 2) return table.concat(t, '')")
            .unwrap(),
        "ab"
    );
    assert_eq!(
        s.eval::<String>(
            "local t = {'c','a','z','y'} table.setn(t, 2) table.sort(t) \
             return table.concat(t, '', 1, 4)"
        )
        .unwrap(),
        "aczy"
    );
    assert_eq!(
        s.eval::<String>(
            "local t = {'a','b','c'} table.setn(t, 2) local out = '' \
             table.foreachi(t, function(i, v) out = out .. v end) return out"
        )
        .unwrap(),
        "ab"
    );
    assert_eq!(
        s.eval::<String>(
            "local t = {'a','b','c'} table.setn(t, 2) \
             local x, y, z = unpack(t) return tostring(x) .. tostring(y) .. tostring(z)"
        )
        .unwrap(),
        "abnil"
    );
    assert_eq!(
        s.eval::<String>(
            "local t = {} t[1] = 'a' t[3] = 'c' table.setn(t, 3) \
             local x, y, z = unpack(t) \
             return tostring(x) .. tostring(y) .. tostring(z)"
        )
        .unwrap(),
        "anilc"
    );
    assert_eq!(
        s.eval::<String>("return table.concat({'a','b','c'}, '')")
            .unwrap(),
        "abc"
    );
    assert_eq!(
        s.eval::<String>("local t = {'c','b','a'} table.sort(t) return table.concat(t, '')")
            .unwrap(),
        "abc"
    );
}

/// `getn`, `tinsert` and `tremove`, bound by the stdlib layer after this one, are these bodies.
#[test]
fn the_bare_aliases_bind_the_5_0_bodies() {
    let s = UiScript::new().unwrap();
    assert!(s.eval::<bool>("return getn == table.getn").unwrap());
    assert!(s.eval::<bool>("return tinsert == table.insert").unwrap());
    assert!(s.eval::<bool>("return tremove == table.remove").unwrap());
    assert_eq!(
        s.eval::<i64>("local t = {1,2,3} table.setn(t, 0) tinsert(t, 9) return t[1]")
            .unwrap(),
        9
    );
}

/// `foreachi` (`0x7fb4e0`, its `lua_call` at `0x7fb552`) and `sort_comp` (`0x7fbc50`, `0x7fbc93`)
/// call the function unprotected: an error inside goes on as it was raised, its value intact.
#[test]
fn a_callbacks_error_goes_on_as_it_was_raised() {
    let s = UiScript::new().unwrap();
    for call in [
        "table.foreachi({1}, function() error(E, 0) end)",
        "table.foreach({1}, function() error(E, 0) end)",
        "table.sort({1, 2}, function() error(E, 0) end)",
    ] {
        assert!(
            s.eval::<bool>(&format!(
                "E = {{}} local ok, e = pcall(function() {call} end) return e == E"
            ))
            .unwrap(),
            "{call}: the table raised must be the value caught"
        );
        // A message's bytes, which no UTF-8 copy may change.
        assert!(
            s.eval::<bool>(&format!(
                "E = '\\233' local ok, e = pcall(function() {call} end) return e == E"
            ))
            .unwrap(),
            "{call}: the message's bytes must survive"
        );
        // An `xpcall` handler runs once, at the raise.
        assert_eq!(
            s.eval::<i64>(&format!(
                "E = {{}} local n = 0 \
                 xpcall(function() {call} end, function(e) n = n + 1 return e end) return n"
            ))
            .unwrap(),
            1,
            "{call}"
        );
    }
}

/// `sort_comp` calls the comparator through `lua_call` (`0x7fbc93`), one of the 200 C levels
/// `luaD_call` counts (`0x6f65a6`..`0x6f65ae`); `table.sort` called from Lua counts none. So
/// sorts nested through their comparators go as deep as any other callback, `gsub`'s.
#[test]
fn a_nested_sort_costs_one_host_level_per_comparator() {
    let s = UiScript::new().unwrap();
    let depth = |nest: &str| -> i64 {
        s.eval::<i64>(&format!(
            "local d = 0 local f f = function() d = d + 1 {nest} return true end \
             local ok, e = pcall(f) return d"
        ))
        .unwrap()
    };
    let sort = depth("table.sort({1, 2}, f)");
    let gsub = depth("string.gsub('a', 'a', f)");
    assert!(sort > 150, "{sort} sorts nested");
    assert_eq!(
        sort, gsub,
        "a comparator call is one level, as a gsub callback is"
    );
}

/// `table.concat`'s separator is `luaL_optlstring`'s (`0x7fb84c`): any bytes.
#[test]
fn concat_joins_with_the_separators_bytes() {
    let s = UiScript::new().unwrap();
    assert!(s
        .eval::<bool>(r#"return table.concat({"a", "b"}, "\233") == "a\233b""#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return table.concat({"a", "b", "c"}, "\0|") == "a\0|b\0|c""#)
        .unwrap());
}

/// `tinsert` (`0x7fb6b0`) takes the position form for any argument count but 2 (`0x7fb6e0`), so
/// one argument reads a position that is not there.
#[test]
fn insert_with_no_value_reads_a_missing_position() {
    let s = UiScript::new().unwrap();
    let e: String = s
        .eval(
            "local t = {} local ok, e = pcall(function() table.insert(t) end) \
             return e .. ' ' .. table.getn(t)",
        )
        .unwrap();
    assert!(
        e.ends_with(":1: bad argument #2 to `insert' (number expected, got no value) 0"),
        "the table must not grow: {e}"
    );
}

/// `str_concat` (`0x7fb830`): an element neither string nor number fails argument 1's
/// `luaL_argcheck` (`0x7fb8d1`, text `0x8821f0`); an end of 0 is `getn` (`0x7fb893`); the
/// arguments are read 2, 3, 4, then the table.
#[test]
fn concat_checks_its_arguments_as_the_client_does() {
    let s = UiScript::new().unwrap();
    let raised = |call: &str| -> String {
        s.eval::<String>(&format!(
            "local ok, e = pcall(function() return {call} end) return tostring(e)"
        ))
        .unwrap()
    };
    assert!(
        raised("table.concat({1, {}, 3})")
            .ends_with(":1: bad argument #1 to `concat' (table contains non-strings)"),
        "{}",
        raised("table.concat({1, {}, 3})")
    );
    assert_eq!(
        s.eval::<String>("return table.concat({'a', 'b', 'c'}, ',', 1, 0)")
            .unwrap(),
        "a,b,c"
    );
    assert_eq!(
        s.eval::<String>("return table.concat({'a', 'b', 'c'}, ',', 2)")
            .unwrap(),
        "b,c"
    );
    assert!(
        raised("table.concat(nil, {})")
            .ends_with("bad argument #2 to `concat' (string expected, got table)"),
        "{}",
        raised("table.concat(nil, {})")
    );
}

/// `unpack` (`0x7030a0`) asks `luaL_checkstack` for its count (`0x7030c8`), and the client's
/// `lua_checkstack` (`0x6f2f30`) refuses once the slots in use, its argument among them, plus
/// that count pass 2048 (`0x6f2f43`): a table of 2047 unpacks, one of 2048 raises the client's
/// text (`0x8716b0`, `0x872bc4`) at its caller's line.
#[test]
fn unpack_refuses_past_the_clients_2048_slots() {
    let s = UiScript::new().unwrap();
    let unpack = |n: usize| -> String {
        s.eval::<String>(&format!(
            "local t = {{}} for i = 1, {n} do t[i] = i end \
             local ok, e = pcall(function() return table.getn({{unpack(t)}}) end) \
             return tostring(e)"
        ))
        .unwrap()
    };
    assert_eq!(unpack(2047), "2047");
    assert!(
        unpack(2048).ends_with(":1: stack overflow (table too big to unpack)"),
        "{}",
        unpack(2048)
    );
    // A remembered size counts as the elements do.
    assert!(s
        .eval::<String>("local t = {} table.setn(t, 5000) local ok, e = pcall(unpack, t) return e")
        .unwrap()
        .ends_with("stack overflow (table too big to unpack)"));
}

/// `luaL_getn` (`0x6f5050`) and `luaL_setn` (`0x6f4ea0`) read `t.n` through `checkint`
/// (`0x6f4f80`): `lua_tonumber`, so a numeric string is a size, and `luaL_setn` then writes a
/// number over it; a string that is no number is no size. `luaL_setn` keeps a negative size as
/// given, which `luaL_getn` then passes over.
#[test]
fn a_numeric_string_n_is_a_size() {
    let s = UiScript::new().unwrap();
    let eval = |src: &str| s.eval::<String>(src).unwrap();
    assert_eq!(eval("return tostring(table.getn({n = '2'}))"), "2");
    assert_eq!(eval("return tostring(table.getn({'a', n = ' 3 '}))"), "3");
    assert_eq!(
        eval("return tostring(table.getn({'a', 'b', n = 'x'}))"),
        "2"
    );
    assert_eq!(
        eval("local t = {n = '2'} table.setn(t, 5) return type(t.n) .. t.n"),
        "number5"
    );
    assert_eq!(
        eval("local t = {'a', n = '1'} table.insert(t, 'b') return type(t.n) .. t.n .. t[2]"),
        "number2b"
    );
    assert_eq!(
        eval("local t = {'a', 'b', 'c', n = '2'} return table.concat(t) .. unpack(t)"),
        "aba"
    );
    // A negative size: into `t.n` when one is there, else remembered; `getn` counts past both.
    assert_eq!(
        eval("local t = {'a', n = 3} table.setn(t, -1) return t.n .. ' ' .. table.getn(t)"),
        "-1 1"
    );
    assert_eq!(
        eval("local t = {'a', 'b'} table.setn(t, 5) table.setn(t, -1) return tostring(table.getn(t))"),
        "2"
    );
}

/// `luaH_next` raises the client's text for a key not in the table (`0x6fa467`, `0x871aac`),
/// through `next` and `table.foreach` alike.
#[test]
fn next_names_its_invalid_key_as_the_client_does() {
    let s = UiScript::new().unwrap();
    for call in [
        "next({}, 'nope')",
        "local t = {a = 1} \
         table.foreach(t, function(k) t[k] = nil for i = 1, 20 do t['x' .. i] = i end end)",
    ] {
        assert_eq!(
            s.eval::<String>(&format!(
                "local ok, e = pcall(function() {call} end) return e"
            ))
            .unwrap(),
            "invalid key for `next'"
        );
    }
}

/// `luaB_unpack` (`0x7030a0`) reads the table alone: items 1 to `getn`, every later argument
/// unread.
#[test]
fn unpack_reads_its_table_alone() {
    let s = UiScript::new().unwrap();
    assert_eq!(
        s.eval::<Vec<i64>>("return {unpack({1, 2, 3}, 2)}").unwrap(),
        vec![1, 2, 3]
    );
    assert_eq!(
        s.eval::<Vec<i64>>("return {unpack({1, 2, 3}, 2, 2)}")
            .unwrap(),
        vec![1, 2, 3]
    );
    assert_eq!(
        s.eval::<i64>("return table.getn({unpack({1, 2, 3}, 'x', {})})")
            .unwrap(),
        3
    );
    // The positional `format` still carries a trailing nil through `arg.n`.
    assert_eq!(
        s.eval::<String>("return format('%2$s %1$s', 'a', 'b')")
            .unwrap(),
        "b a"
    );
}
