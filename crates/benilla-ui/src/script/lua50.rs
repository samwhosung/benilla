//! The Lua 5.0 dialect: what the script host sets over the client-shaped state benilla-lua opens.
//!
//! The reference embeds Lua 5.0 (`$Lua: Lua 5.0` at `0x811b30`), and `InitLua 0x7039e0` opens
//! five libraries: base (`0x811e28`, 36 entries), `string` (`0x822d88`, 12), `table`
//! (`0x822d40`, 8), `math` (`0x822c60`, 24) and `bit` (`0x822c18`, 8), with no `os`, `io`,
//! `debug` or `coroutine`. The grammar is benilla-lua's (`crates/benilla-lua/GRAMMAR.md`).
//!
//! `...` is a parameter-list token only: a vararg function reads its arguments through 5.0's
//! `arg` table (`arg.n`, `unpack(arg)`). So Lua written here forwards an unknown argument list
//! only through a table, and a verb whose arity matters (`GetPoint()` against `GetPoint(nil)`)
//! stays in Rust.

use mlua::Lua;

/// Lua 5.0's remembered table size and the `n`-based table library that reads and updates it,
/// which are benilla-lua's own natives.
#[cfg(test)]
mod table_size;

/// Install the dialect. Runs before the WoW stdlib layer, so its aliases bind these functions.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // The state opens as the client's (`benilla_lua::new_state`): no 5.1 member, `coroutine`,
    // `select`, `print` or `_VERSION` is in it, and none is set to nil here, which would make
    // the key (5.0's `luaH_set` makes one even for nil) and move the layout.

    // 1.12's string type has no metatable, so `("x"):upper()` raises `attempt to index a string
    // value`. There it cannot carry one: `luaT_gettmbyobj 0x6f7bd0` finds metamethods only on
    // tables and userdata, `lua_setmetatable 0x6f4020` writes only those two tags, and
    // `luaopen_string 0x7fd810` installs none, where 5.1's does. Base `setmetatable` (`0x702a40`)
    // rejects a non-table in both.
    lua.set_type_metatable::<mlua::String>(None);

    // `__pow` is a 1.12 global, `luaopen_math`'s `math_pow` (`0x7fb4c4`), which `^` on two
    // numbers calls (`0x6f9b52`).
    g.set(
        "__pow",
        lua.create_native("__pow", mlua::benilla_lua::math_pow())?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// `0x703200`: `lua_getgccount` and `lua_getgcthreshold`, each `>> 10`, two values.
    #[test]
    fn gcinfo_answers_two_numbers_as_1_12_does() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("gcinfo()").unwrap(),
            2,
            "`mov eax, 2` at 0x703239 — one value is 5.1's shape, not 1.12's"
        );
        assert_eq!(
            s.eval::<Vec<String>>("local a, b = gcinfo() return { type(a), type(b) }")
                .unwrap(),
            vec!["number".to_string(), "number".to_string()],
            "(number,number) — `bindings.tsv` types the row `agree`"
        );
        // `luaC_checkGC` keeps 5.0's threshold above the live count.
        assert!(
            s.eval::<bool>("local c, t = gcinfo() return t > c")
                .unwrap(),
            "the threshold must sit above the live count, as 5.0's collector guarantees"
        );
        // AceAddon-2.0's memory report divides the second value by 1024.
        assert!(
            s.eval::<String>(
                "local mem, threshold = gcinfo() \
                 return string.format('%.3f', threshold / 1024)"
            )
            .is_ok(),
            "AceAddon-2.0's memory report divides the second slot by 1024"
        );
    }

    /// `InitLua` sets the threshold to 256 KB right after `lua_open` (`0x7039f2`), so a new state
    /// that has not counted that much answers 256 for it.
    #[test]
    fn a_new_state_answers_init_luas_256_kb_threshold() {
        let s = UiScript::new().unwrap();
        let info = s.eval::<Vec<f64>>("return { gcinfo() }").unwrap();
        assert!(info[0] < 256.0, "the state counts {} KB", info[0]);
        assert_eq!(info[1], 256.0);
    }

    /// `0x703250`: `luaL_optnumber(L, 1, 0.0)`, `_ftol`, `lua_setgcthreshold`, no values, and no
    /// string options.
    #[test]
    fn collectgarbage_answers_nothing_and_takes_a_number() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("collectgarbage()").unwrap(),
            0,
            "`xor eax,eax` at 0x70326f — 5.1 returns one value here"
        );
        // The reference's only argument form, a number, which stock 5.1 rejects.
        assert_eq!(s.arity("collectgarbage(0)").unwrap(), 0);
        assert!(s.eval::<()>("collectgarbage(2048)").is_ok());
        // `lua_tonumber` coerces a numeric string through `strtod`.
        assert!(s.eval::<()>(r#"collectgarbage("100")"#).is_ok());
        // A non-numeric string is a type error, in 5.0's words.
        let e = s
            .eval::<()>(r#"collectgarbage("count")"#)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("bad argument #1 to `collectgarbage' (number expected, got string)"),
            "5.1's string options must not exist here: {e}"
        );
        // An explicit threshold above the live count is what `gcinfo` reports back.
        assert!(s
            .eval::<bool>(
                "local c = gcinfo() collectgarbage(c * 4) local _, t = gcinfo() return t >= c"
            )
            .unwrap());
    }

    /// `collectgarbage(n)` sets the collector's own threshold (`lua_setgcthreshold 0x6f4400`): one
    /// far above the count lets garbage pile up uncollected, and `gcinfo` counts it.
    #[test]
    fn collectgarbage_paces_the_collector_itself() {
        let s = UiScript::new().unwrap();
        let grown: f64 = s
            .eval(
                "collectgarbage(1000000) local a = gcinfo() \
                 for i = 1, 400000 do local t = {i, i, i, i} end \
                 local b = gcinfo() return b - a",
            )
            .unwrap();
        assert!(
            grown > 30_000.0,
            "400000 uncollected tables of 96 bytes, counted as {grown} KB"
        );
        // A threshold at or below the count collects now.
        assert!(s
            .eval::<bool>("local a = gcinfo() collectgarbage(0) return gcinfo() < a - 30000")
            .unwrap());
    }

    /// `^` calls the global `__pow`, `luaopen_math`'s `math_pow` (`0x7fb4c4`): the client's `pow`
    /// answers the indefinite for `1 ^ inf`, and its argument check words the error.
    #[test]
    fn pow_is_the_clients_math_pow() {
        let s = UiScript::new().unwrap();
        assert!(
            s.eval::<bool>("local x = 1 ^ (1/0) return x ~= x").unwrap(),
            "C99's 1 is not the client's `-1.#IND`"
        );
        let e: String = s
            .eval("local ok, e = pcall(function() return __pow('x', 2) end) return e")
            .unwrap();
        assert!(
            e.ends_with("bad argument #1 to `__pow' (number expected, got string)"),
            "{e}"
        );
    }

    /// `loadstring` answers the function alone on success (`mov eax,1` at `0x7032ba`).
    #[test]
    fn loadstring_answers_one_value() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("loadstring('x = 1')").unwrap(), 1);
        assert_eq!(s.arity("loadstring('x = ')").unwrap(), 2);
    }

    /// `luaB_loadstring 0x703280` names a chunk by its source unless given a name (`0x70329a`),
    /// and `luaO_chunkid 0x6f5c40` renders each form.
    #[test]
    fn a_loadstring_chunk_is_named_by_its_own_source() {
        let s = UiScript::new().unwrap();
        let raised = |src: &str| -> String {
            s.eval::<String>(&format!(
                "local f = loadstring({src}) local ok, e = pcall(f) return tostring(e)"
            ))
            .unwrap()
        };
        assert!(
            raised(r#""error('boom')""#).starts_with(r#"[string "error('boom')"]:1: boom"#),
            "the default name is the source: {}",
            raised(r#""error('boom')""#)
        );
        // `=`: printed verbatim, the marker removed once.
        assert!(raised(r#""error('boom')", "=myname""#).starts_with("myname:1: boom"));
        // `@`: the file form, the path printed plainly.
        assert!(raised(r#""error('boom')", "@a/b.lua""#).starts_with("a/b.lua:1: boom"));
        // No marker: wrapped as a string chunk.
        assert!(raised(r#""error('boom')", "plain""#).starts_with(r#"[string "plain"]:1: boom"#));

        // The failure leg returns Lua's message unchanged (`load_aux`), without the category word
        // mlua's `Display` adds.
        let e: String = s
            .eval(r#"local f, e = loadstring("return 1+") return tostring(e)"#)
            .unwrap();
        assert_eq!(
            e, "[string \"return 1+\"]:1: unexpected symbol near `<eof>'",
            "the second return is the message verbatim, with no mlua decoration"
        );
    }

    /// `luaB_assert 0x7031a0`: `lua_settop(L, 1)` at `0x7031e5`, then `mov eax,1` at `0x7031ea`;
    /// only a call with more than one argument tells 5.0 from 5.1.
    #[test]
    fn assert_answers_one_value_and_keeps_5_0_s_messages() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("assert(1, 2, 3)").unwrap(),
            1,
            "5.0 truncates to the first argument; 5.1 returns them all"
        );
        assert_eq!(s.eval::<i64>("return (assert(7, 'x'))").unwrap(), 7);
        // 0 is truthy: only nil and false raise.
        assert_eq!(s.arity("assert(0)").unwrap(), 1);

        let raised = |call: &str| -> String {
            s.eval::<String>(&format!(
                "local ok, e = pcall(function() {call} end) return tostring(e)"
            ))
            .unwrap()
        };
        // `luaL_optlstring(L, 2, "assertion failed!" /*0x872bdc*/)`, then `luaL_error`.
        assert!(raised("assert(false)").contains("assertion failed!"));
        assert!(raised("assert(nil, 'my message')").contains("my message"));
        // A number is a string to `luaL_optlstring`; anything else is a bad argument.
        assert!(raised("assert(false, 42)").contains("42"));
        assert!(
            raised("assert(false, {})").contains("bad argument #2 to `assert' (string expected"),
            "{}",
            raised("assert(false, {})")
        );
        // `luaL_checkany 0x6f4bb0`: an absent argument 1 raises, an explicit nil does not.
        assert!(
            raised("assert()").contains("bad argument #1 to `assert' (value expected)"),
            "{}",
            raised("assert()")
        );
    }

    /// `AceLibrary.lua`'s opening: it takes the 5.0 branch from `GetBuildInfo()` and calls
    /// `table.setn`, which stock 5.1 raises on.
    #[test]
    fn acelibrarys_own_dialect_probe_takes_the_5_0_branch_and_works() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            local table_setn
            local version = GetBuildInfo()
            if string.find(version, "^2%.") then
                table_setn = function() end
            else
                table_setn = table.setn
            end
            local t = { "a", "b", "c" }
            for k in pairs(t) do t[k] = nil end
            table_setn(t, 0)
            AceProbe = table.getn(t)
            "#,
        )
        .expect("Ace's dialect probe must not raise");
        assert_eq!(s.eval::<i64>("return AceProbe").unwrap(), 0);
    }

    /// The remembered size is neither a no-op nor a visible `n` key.
    #[test]
    fn getn_and_setn_remember_a_size_without_polluting_the_table() {
        let s = UiScript::new().unwrap();
        // No remembered size: a count from 1 to the first nil.
        assert_eq!(s.eval::<i64>("return table.getn({1,2,3})").unwrap(), 3);
        // The size round-trips...
        assert_eq!(
            s.eval::<i64>("local t = {1,2,3} table.setn(t, 7) return table.getn(t)")
                .unwrap(),
            7
        );
        // ...and is invisible to `pairs`: AceOO-2.0's `_Embed` errors on any field its target
        // already has.
        assert_eq!(
            s.eval::<i64>(
                "local t = {1,2,3} table.setn(t, 0) \
                 local c = 0 for k in pairs(t) do c = c + 1 end return c"
            )
            .unwrap(),
            3,
            "a remembered size must not appear as a key — 39 corpus addons died on a stray 'n'"
        );
        // A table that already has a numeric `n` keeps its size there, 5.0's own branch.
        assert_eq!(
            s.eval::<i64>("local t = {1,2,3, n=3} table.setn(t, 7) return t.n")
                .unwrap(),
            7
        );
        // ...and the bare global is the same function.
        assert!(
            s.eval::<bool>("return getn == table.getn").unwrap(),
            "a divergent `getn` global is how the two answers start disagreeing"
        );
    }

    /// The 5.1-only members are gone, so an addon's feature detection picks 1.12's branch.
    #[test]
    fn the_5_1_only_members_are_not_offered() {
        let s = UiScript::new().unwrap();
        for expr in [
            "string.gmatch",
            "string.match",
            "string.reverse",
            "table.maxn",
            "math.fmod",
            "math.modf",
            "math.huge",
            "math.cosh",
            "coroutine",
        ] {
            assert!(
                s.eval::<bool>(&format!("return {expr} == nil")).unwrap(),
                "{expr} is a Lua 5.1 addition — 1.12 does not have it"
            );
        }
        // ...and the 5.0 spellings are present.
        for expr in ["string.gfind", "math.mod", "table.setn", "table.foreach"] {
            assert!(
                s.eval::<bool>(&format!("return {expr} ~= nil")).unwrap(),
                "{expr} is Lua 5.0's own spelling and 1.12 has it"
            );
        }
    }

    /// `math_atan2`, `math_mod` and `math_pow` (`0x7faf30`, `0x7fafd0`, `0x7fb050`) check
    /// argument 1, then 2 (`0x7fb057`, `0x7fb066`); only `math_ldexp` (`0x7fb1f0`) checks 2 first.
    #[test]
    fn two_argument_math_checks_its_first_argument_first() {
        let s = UiScript::new().unwrap();
        let raised = |call: &str| -> String {
            s.eval::<String>(&format!(
                "local ok, e = pcall(function() return {call} end) return e"
            ))
            .unwrap()
        };
        for (call, want) in [
            (
                "math.pow()",
                "bad argument #1 to `pow' (number expected, got no value)",
            ),
            (
                "math.atan2('x', 'y')",
                "bad argument #1 to `atan2' (number expected, got string)",
            ),
            (
                "math.mod(nil, {})",
                "bad argument #1 to `mod' (number expected, got nil)",
            ),
            (
                "math.ldexp()",
                "bad argument #2 to `ldexp' (number expected, got no value)",
            ),
        ] {
            assert!(raised(call).ends_with(want), "{call}: {}", raised(call));
        }
    }

    /// The bare `mod` is `math.mod`, so it survives `math.fmod`'s removal.
    #[test]
    fn the_bare_math_family_survives_the_removals() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<f64>("return mod(7, 3)").unwrap(), 1.0);
        assert_eq!(s.eval::<f64>("return floor(2.7)").unwrap(), 2.0);
    }

    /// `...` parses in a parameter list and nowhere else; `arg` carries the arguments.
    #[test]
    fn vararg_is_a_declaration_only() {
        let s = UiScript::new().unwrap();
        let parses = |src: &str| s.run(src).is_ok();
        // Declaration, and 5.0's way to read what it captured.
        assert!(parses("return function(x, ...) return x end"));
        assert!(parses("return function(x, ...) return arg.n end"));
        assert!(parses(
            "local f = tostring return function(x, ...) return f(x, unpack(arg)) end"
        ));
        // Every use of `...` as an expression fails, at chunk level too, where 5.1 allows it.
        assert!(!parses("return function(x, ...) return ... end"));
        assert!(!parses(
            "local f = tostring return function(x, ...) return f(x, ...) end"
        ));
        assert!(!parses("local a = ... return a"));
    }

    /// The client's `luaO_str2d` (`0x6f5900`) is `strtod` (`0x741e39`) and a trailing-space skip:
    /// a decimal numeral or nothing. Every coercion goes through it, so hexadecimal, `inf` and
    /// `nan` are not numbers to `tonumber`, to arithmetic or to a binding's argument, and a source
    /// `0x10` does not compile.
    #[test]
    fn only_a_decimal_numeral_is_a_number() {
        let s = UiScript::new().unwrap();
        for text in [
            "0x10", "0X1A", "0x1p4", "-0x2", " 0x10 ", "inf", "nan", "infinity",
            // A marker with no digit after it ends the numeral before it (`0x411432`), so it
            // trails; `f` is no marker.
            "1e", "1d", "1f2",
        ] {
            assert!(
                s.eval::<bool>(&format!("return tonumber({text:?}) == nil"))
                    .unwrap(),
                "tonumber({text:?})"
            );
        }
        for (text, n) in [
            (" 12 ", 12.0),
            ("1e2", 100.0),
            ("-.5", -0.5),
            ("5.", 5.0),
            ("+3", 3.0),
            // The client's strtod also takes `d`/`D` as the exponent marker (`0x4112d1`).
            ("1d2", 100.0),
            ("-2.5D-1", -0.25),
            (" 1d+1 ", 10.0),
        ] {
            assert_eq!(
                s.eval::<f64>(&format!("return tonumber({text:?})"))
                    .unwrap(),
                n,
                "tonumber({text:?})"
            );
        }
        // A numeral too long for the conversion's stack buffer takes the same path.
        let long = format!("1{}d-70", "0".repeat(70));
        assert_eq!(
            s.eval::<f64>(&format!("return tonumber({long:?})"))
                .unwrap(),
            1.0
        );
        // A base still reads hexadecimal digits: that is `strtoul`, not `luaO_str2d`.
        assert_eq!(s.eval::<f64>(r#"return tonumber("10", 16)"#).unwrap(), 16.0);
        assert!(!s
            .eval::<bool>(r#"return (pcall(function() return "0x2" + 1 end))"#)
            .unwrap());
        assert_eq!(s.eval::<f64>(r#"return "2" + 1"#).unwrap(), 3.0);
        // A binding's numeric argument: `GetCraftInfo`'s index raises on a non-number.
        assert!(!s
            .eval::<bool>(r#"return (pcall(GetCraftInfo, "0x2"))"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return (pcall(GetCraftInfo, "2"))"#)
            .unwrap());
        // The numeral reader (`0x6ffd20`) stops at the `x`: `0x10` is 0 and then the name `x10`.
        assert!(s.run("return 0x10").is_err());
    }

    /// The client's numeral reader (`0x6ffd20`, 5.0's `read_numeral`) takes digits, one `.`,
    /// digits, then `e`/`E`, a sign and digits, and nothing more: a letter or `_` after a number
    /// starts the next token, and `1..2` is its own error (`0x8721dc`).
    #[test]
    fn a_numeral_ends_where_the_clients_reader_stops() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<(i64, i64)>("a=1b=2 return a, b").unwrap(),
            (1, 2),
            "two statements, as the client reads them"
        );
        assert!(
            s.run("a = 0x10()").is_err(),
            "compiles as `a = 0` then a call to `x10`, which is nil"
        );
        assert!(s
            .eval::<bool>("return loadstring('a = 0x10()') ~= nil")
            .unwrap());
        assert_eq!(s.eval::<f64>("return 1.5e+3").unwrap(), 1500.0);
        assert_eq!(s.eval::<f64>("return .5E-1").unwrap(), 0.05);
        assert_eq!(s.eval::<f64>("return 2.").unwrap(), 2.0);
        let err = s.run("return 1..2").unwrap_err().to_string();
        assert!(
            err.contains("ambiguous syntax (decimal point x string concatenation)"),
            "{err}"
        );
        // One `.` per numeral: `1.2.3` is `1.2` then `.3`.
        let err = s.run("return 1.2.3").unwrap_err().to_string();
        assert!(err.contains("near `.3'"), "{err}");
        for malformed in ["return 3e", "return .5.3"] {
            let err = s.run(malformed).unwrap_err().to_string();
            assert!(err.contains("malformed number"), "{malformed}: {err}");
        }
    }

    #[test]
    fn sort_foreach_and_foreachi_are_bare_globals_like_the_reference() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<String>("local t = {'c','a','b'} sort(t) return table.concat(t)")
                .unwrap(),
            "abc"
        );
        assert_eq!(
            s.eval::<i64>("local n = 0 foreach({1,2,3}, function() n = n + 1 end) return n")
                .unwrap(),
            3
        );
        assert_eq!(
            s.eval::<i64>("local n = 0 foreachi({1,2,3}, function() n = n + 1 end) return n")
                .unwrap(),
            3
        );
    }

    #[test]
    fn the_bit_library_is_present_with_the_references_eight_functions() {
        let s = UiScript::new().unwrap();
        for name in [
            "bnot", "band", "bor", "bxor", "lshift", "rshift", "arshift", "mod",
        ] {
            assert!(
                s.eval::<bool>(&format!("return type(bit.{name}) == 'function'"))
                    .unwrap(),
                "bit.{name} is one of the array's eight entries"
            );
        }
        // Unsigned out, the word loaded by `fild qword` with its high word zeroed (`0x7faa6d`).
        assert_eq!(s.eval::<i64>("return bit.bnot(0)").unwrap(), 4_294_967_295);
        assert_eq!(
            s.eval::<i64>("return bit.band(4294967295, 4294967295)")
                .unwrap(),
            4_294_967_295
        );
        assert_eq!(
            s.eval::<i64>("return bit.bor(-1, 0)").unwrap(),
            4_294_967_295
        );
        assert_eq!(
            s.eval::<i64>("return bit.bxor(-1, 1)").unwrap(),
            4_294_967_294
        );
        assert_eq!(
            s.eval::<i64>("return bit.lshift(1, 31)").unwrap(),
            2_147_483_648
        );
        assert_eq!(
            s.eval::<i64>("return bit.rshift(-1, 0)").unwrap(),
            4_294_967_295
        );
        // `arshift` and `mod` alone load `fild dword`: signed (`0x7fad06`, `0x7fadab`).
        assert_eq!(s.eval::<i64>("return bit.arshift(-8, 1)").unwrap(), -4);
        assert_eq!(s.eval::<i64>("return bit.mod(-7, 4)").unwrap(), -3);
        // `__ftol` truncates to 64 bits and the low word is read: past 64 bits is 0.
        assert_eq!(s.eval::<i64>("return bit.bor(2^32 + 5, 0)").unwrap(), 5);
        assert_eq!(
            s.eval::<i64>("return bit.bnot(2^64)").unwrap(),
            4_294_967_295
        );
        // A shift reads its count, argument 2, first (`0x7fac18`).
        assert!(s
            .eval::<String>("local ok, e = pcall(function() return bit.lshift() end) return e")
            .unwrap()
            .ends_with("bad argument #2 to `lshift' (number expected, got no value)"));
        assert_eq!(s.eval::<i64>("return bit.band(4660, 255)").unwrap(), 0x34);
        assert_eq!(s.eval::<i64>("return bit.bor(240, 15)").unwrap(), 0xFF);
        assert_eq!(s.eval::<i64>("return bit.bxor(255, 15)").unwrap(), 0xF0);
        assert_eq!(s.eval::<i64>("return bit.lshift(1, 4)").unwrap(), 16);
        // `rshift` is logical, `arshift` propagates the sign.
        assert_eq!(s.eval::<i64>("return bit.rshift(-1, 28)").unwrap(), 15);
        assert_eq!(s.eval::<i64>("return bit.arshift(-1, 28)").unwrap(), -1);
        // The shift count masks to 5 bits, as the x86 instruction does.
        assert_eq!(s.eval::<i64>("return bit.lshift(1, 32)").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return bit.mod(17, 5)").unwrap(), 2);
    }

    /// `bit.band`, `bor` and `bxor` fold every argument from 2 to `lua_gettop` into the first
    /// (`0x7faac0`..`0x7faad9`): one argument answers itself, and each is checked in turn.
    #[test]
    fn band_bor_and_bxor_fold_every_argument() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return bit.bor(1, 2, 4)").unwrap(), 7);
        assert_eq!(s.eval::<i64>("return bit.band(15, 7, 3)").unwrap(), 3);
        assert_eq!(s.eval::<i64>("return bit.bxor(1, 3, 7, 15)").unwrap(), 10);
        assert_eq!(s.eval::<i64>("return bit.band(-1)").unwrap(), 4_294_967_295);
        assert!(s
            .eval::<String>(
                "local ok, e = pcall(function() return bit.bor(1, 2, 'x') end) return e"
            )
            .unwrap()
            .ends_with("bad argument #3 to `bor' (number expected, got string)"));
        assert!(s
            .eval::<String>("local ok, e = pcall(function() return bit.band() end) return e")
            .unwrap()
            .ends_with("bad argument #1 to `band' (number expected, got no value)"));
    }

    /// `bit.mod` tests only the low word of its divisor's `__ftol` (`0x7fad32`); a 0 there answers
    /// 1 divided by the divisor (`0x7fad42`), with argument 1 never read, and raises nothing.
    #[test]
    fn bit_mod_by_a_zero_word_answers_one_over_the_divisor() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<f64>("return bit.mod(7, 0.5)").unwrap(), 2.0);
        assert_eq!(
            s.eval::<f64>("return bit.mod(7, 0)").unwrap(),
            f64::INFINITY
        );
        assert_eq!(
            s.eval::<f64>("return bit.mod(7, 2^32)").unwrap(),
            1.0 / 4_294_967_296.0
        );
        assert_eq!(s.eval::<f64>("return bit.mod(nil, '0.25')").unwrap(), 4.0);
        assert_eq!(s.eval::<i64>("return bit.mod(7, 2^32 + 3)").unwrap(), 1);
    }

    /// `print` and `_VERSION` are not in 1.12's `_G`, and `__pow` is.
    #[test]
    fn the_base_library_matches_the_captured_globals() {
        let s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return print == nil").unwrap());
        assert!(s.eval::<bool>("return _VERSION == nil").unwrap());
        // 5.0 implements `^` by calling `__pow`.
        assert_eq!(s.eval::<f64>("return __pow(2, 10)").unwrap(), 1024.0);
    }

    /// Ace2's `lua51` probe, `loadstring("return function(...) return ... end")`, fails as on
    /// 1.12: the reference's `simpleexp` (`0x6fd240`) has no `...` arm.
    #[test]
    fn the_vararg_expression_is_a_syntax_error_as_it_is_on_1_12() {
        let s = UiScript::new().unwrap();
        assert!(
            s.eval::<bool>(
                r#"return (loadstring("return function(...) return ... end") and true or false) == false"#
            )
            .unwrap(),
            "the Ace2 lua51 probe must answer false, as it does on the 1.12 client"
        );
        // `prefixexp` then accepts only `(` or a name, so this is 5.0's own `unexpected symbol`.
        let msg: String = s
            .eval(r#"local f, e = loadstring("return ...") return tostring(e)"#)
            .unwrap();
        assert!(
            msg.contains("unexpected symbol"),
            "5.0's own message for this, not a bespoke one: {msg}"
        );
        // The 5.0 half of the question: `arg`.
        assert_eq!(
            s.eval::<i64>("local f = function(...) return arg.n end return f(1, 2, 3)")
                .unwrap(),
            3,
            "`arg` is 5.1's compat-vararg table, which is the form all of 1.12 FrameXML uses"
        );
        // Every vararg function gets `arg`, 5.0's rule: without that arm nothing clears
        // `VARARG_NEEDSARG`.
        assert_eq!(
            s.eval::<i64>("local f = function(a, ...) return arg.n end return f(1, 2, 3)")
                .unwrap(),
            2
        );
        // A `...` in a parameter list still parses: the deletion is `simpleexp`'s arm, not
        // `parlist`'s.
        assert!(s
            .eval::<bool>(r#"return loadstring("return function(...) return arg.n end") ~= nil"#)
            .unwrap());
    }

    /// The reference's `getunopr` (`0x6fe0a0`) knows only `-` and `not`, its `getbinopr`
    /// (`0x6fe0c0`) has no `%`, and its metamethod names (`0x871896`) have no `__len` or `__mod`.
    #[test]
    fn the_length_and_modulo_operators_are_not_in_the_grammar() {
        let s = UiScript::new().unwrap();
        for probe in ["return #t", "return 7 % 3", "local n = #({1,2}) return n"] {
            assert!(
                s.eval::<bool>(&format!("return loadstring({:?}) == nil", probe))
                    .unwrap(),
                "{probe} is 5.1-only syntax; 1.12's parser rejects it"
            );
        }
        // 5.0's spellings of both.
        assert_eq!(s.eval::<i64>("return table.getn({1,2,3})").unwrap(), 3);
        assert_eq!(s.eval::<i64>("return getn({1,2,3})").unwrap(), 3);
        assert_eq!(s.eval::<f64>("return math.mod(7, 3)").unwrap(), 1.0);
        assert_eq!(s.eval::<i64>("return string.len('abcd')").unwrap(), 4);
    }

    /// The 5.0 grammar the stock files use still parses, including the three constructs 5.1
    /// dropped: the iterator-less generic-for, nested long strings (`read_long_string`,
    /// `0x700010`) and the constructor's extra semicolon.
    #[test]
    fn the_5_0_grammar_the_reference_writes_still_parses() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            -- 5.0 varargs: the implicit `arg` table, the form all of 1.12 FrameXML uses.
            local function count(...) local n = 0 for i = 1, arg.n do n = n + arg[i] end return n end
            -- the iterator-less generic-for
            local sum = 0
            for k, v in { 3, 4 } do sum = sum + v end
            -- a table constructor with 5.0's compat semicolon
            local t = { a = 1; b = 2; }
            -- nesting long strings, 5.0's
            local s2 = [[outer [[inner]] outer]]
            -- 5.0's own spellings for what `#`/`%` would say
            BENILLA_GRAMMAR_OK = count(1, 2, 3) + sum + t.a + t.b
                + table.getn({ 1, 2 }) + math.mod(7, 3) + string.len(s2)
            "#,
        )
        .expect("the reference's own grammar must still load");
        assert_eq!(
            s.eval::<i64>("return BENILLA_GRAMMAR_OK").unwrap(),
            6 + 7 + 3 + 2 + 1 + 21
        );
    }

    /// Six of the reference's eight `debug*` functions (`[0x7027e0, 0x702840)`) are
    /// `xor eax,eax; ret`, returning nothing; only `debugprofilestart`/`stop` are real.
    #[test]
    fn the_debug_family_is_six_stubs_and_two_real_ones() {
        let s = UiScript::new().unwrap();
        // Zero values, not one nil: only a count tells them apart.
        for name in [
            "debuginfo",
            "debugload",
            "debugprint",
            "debugdump",
            "debugbreak",
            "debugtimestamp",
        ] {
            assert_eq!(
                s.arity(&format!("{name}()")).unwrap(),
                0,
                "{name} returns nothing at all"
            );
        }
        // ...and the two real ones answer milliseconds.
        let ms: f64 = s
            .eval("debugprofilestart() return debugprofilestop()")
            .unwrap();
        assert!(ms >= 0.0, "elapsed milliseconds, not nil: {ms}");
    }

    /// `table.insert` appends after the remembered size (its `luaL_getn` call, `0x7fb6d8`).
    #[test]
    fn table_insert_consults_the_remembered_size() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("local t = {1,2,3} table.setn(t, 0) table.insert(t, 9) return t[1]")
                .unwrap(),
            9,
            "5.0's insert writes index getn+1, which after setn(t, 0) is index 1"
        );
    }

    /// A `{ [1] = a, [2] = b }` constructor builds an array part, so `pairs` walks it in index
    /// order. The reference's `recfield` credits no size hint for a `[expr] =` field (its
    /// `TK_NAME` test at `0x6fd5a4`), so the table starts on the dummy node and the first store's
    /// rehash sizes an array part, which `luaH_next` walks before the hash. Saved variables are
    /// written in this form, as the reference writes them, so a saved list reloads in order.
    #[test]
    fn a_bracketed_key_constructor_is_an_array_and_walks_in_index_order() {
        let s = UiScript::new().unwrap();
        // Bagnon's own table, as its saved-variables file spells it.
        assert_eq!(
            s.eval::<String>(
                "local t = { [1] = 0, [2] = 1, [3] = 2, [4] = 3, [5] = 4, [6] = -2 }
                 local out = '' for _, v in pairs(t) do out = out .. v .. ',' end return out"
            )
            .unwrap(),
            "0,1,2,3,4,-2,",
            "the keyring (-2) is LAST — the order Bagnon's grid shows"
        );
        // The order the fields are written in does not change it.
        assert_eq!(
            s.eval::<String>(
                "local t = { [3] = 'c', [1] = 'a', [2] = 'b' }
                 local out = '' for k in pairs(t) do out = out .. k .. ',' end return out"
            )
            .unwrap(),
            "1,2,3,"
        );
        // A key the array part cannot hold comes after the run, in hash order.
        assert_eq!(
            s.eval::<String>(
                "local t = { ['a'] = 1, [1] = 10, [2] = 20, [3] = 30 }
                 local out = '' for k in pairs(t) do out = out .. tostring(k) .. ',' end return out"
            )
            .unwrap(),
            "1,2,3,a,"
        );
        // Control: the positional spelling is an array too.
        assert_eq!(
            s.eval::<String>(
                "local t = { 0, 1, 2, 3, 4, -2 }
                 local out = '' for _, v in pairs(t) do out = out .. v .. ',' end return out"
            )
            .unwrap(),
            "0,1,2,3,4,-2,"
        );
    }
}

#[cfg(test)]
mod error_quoting_tests {
    use crate::script::UiScript;

    /// Error messages quote a program element as the reference does, `` `x' ``, 5.0's
    /// `LUA_QL`, and `debugstack` renders its own frames the same way
    /// (`0x703760`).
    #[test]
    fn errors_quote_program_elements_the_way_lua_5_0_does() {
        let s = UiScript::new().unwrap();
        let err = |lua: &str| {
            s.eval::<String>(&format!(
                "local ok, e = pcall(function() {lua} end) return tostring(e)"
            ))
            .unwrap()
        };

        // luaG_typeerror.
        let e = err("local t = nil return t.x");
        assert!(
            e.contains("attempt to index local `t' (a nil value)"),
            "typeerror keeps 5.1's quoting: {e}"
        );
        // luaL_argerror, through a library function that raises one.
        let e = err(r#"return string.rep(nil, 2)"#);
        assert!(
            e.contains("bad argument #1 to `rep'"),
            "argerror keeps 5.1's quoting: {e}"
        );
        // luaX_lexerror, the client's `0x87217c` format.
        let e = s
            .eval::<String>(r#"local f, e = loadstring("return 1 +") return tostring(e)"#)
            .unwrap();
        assert!(
            e.contains("near `<eof>'"),
            "the lexer keeps 5.1's quoting: {e}"
        );
        // No message opens a quote with an apostrophe; the closing quote is one in both dialects.
        for e in [
            err("local t = nil return t.x"),
            err("return nosuchfn()"),
            err("return string.rep(nil, 2)"),
        ] {
            assert!(
                !e.contains(" '") && !e.contains("('"),
                "a 5.1-quoted element survives in: {e}"
            );
        }
    }

    /// The client's engine functions and libraries raise through `luaL_error` (`0x6f4940`), which
    /// prefixes `luaL_where(1)` (`0x6f48e0`), the calling Lua function's `chunk:line: `, and word
    /// a bad argument as `luaL_argerror` (`0x6f4810`) does. Only `PlaySound`, `PlayMusic` and
    /// `PlaySoundFile` raise bare, through `lua_error` (`0x458708`, `0x458758`, `0x4587a1`).
    #[test]
    fn a_rust_callbacks_error_carries_its_callers_position() {
        let s = UiScript::new().unwrap();
        let raised = |body: &str| -> String {
            s.lua()
                .load(format!(
                    "local ok, e = pcall(function()\n{body}\nend) return tostring(e)"
                ))
                .set_name("=X.lua")
                .eval::<String>()
                .unwrap()
        };
        assert_eq!(raised("GetCVar()"), "X.lua:2: Usage: GetCVar(\"cvar\")");
        assert_eq!(raised("assert(false, 'x')"), "X.lua:2: x");
        assert_eq!(raised("assert(nil)"), "X.lua:2: assertion failed!");
        for sound in ["PlaySound()", "PlayMusic()", "PlaySoundFile()"] {
            assert!(
                raised(sound).starts_with("Usage: "),
                "{sound}: {}",
                raised(sound)
            );
        }
        // A typed argument fails as `luaL_checktype` words it, under the name it was called by.
        let typed = s
            .lua()
            .create_function(|_, (_t, _n): (mlua::Table, f64)| Ok(()))
            .unwrap();
        s.lua().globals().set("Typed", typed).unwrap();
        assert_eq!(
            raised("Typed(nil)"),
            "X.lua:2: bad argument #1 to `Typed' (table expected, got nil)"
        );
        assert_eq!(
            raised("local f = Typed f({})"),
            "X.lua:2: bad argument #2 to `f' (number expected, got no value)"
        );
        assert_eq!(
            raised("local o = {m = Typed} o:m()"),
            "X.lua:2: bad argument #1 to `m' (number expected, got no value)"
        );
    }

    /// Error text is bytes, as the client's C strings are: `assert`'s message goes through
    /// `luaL_error("%s")` (`0x7031d0`), byte for byte up to a NUL, and `luaL_argerror`
    /// (`0x6f4810`) and the type errors name a function, field or chunk with its raw bytes.
    #[test]
    fn error_text_keeps_its_bytes() {
        let s = UiScript::new().unwrap();
        let caught = |body: &str| -> String {
            s.eval::<String>(&format!(
                "local ok, e = pcall(function() {body} end) \
                 local o = {{}} for i = 1, string.len(e) do o[i] = string.byte(e, i) end \
                 return table.concat(o, ' ')"
            ))
            .unwrap()
        };
        let ends = |body: &str, want: &[u8]| {
            let got: Vec<u8> = caught(body)
                .split(' ')
                .map(|b| b.parse::<u8>().unwrap())
                .collect();
            assert!(got.ends_with(want), "{body}: {got:?}");
        };
        ends("assert(false, '\\233')", b":1: \xe9");
        ends("assert(false, 'a\\0b')", b":1: a");
        // A function stored under a Latin-1 name, then called with a bad argument and as a
        // non-function.
        ends(
            "getfenv(0)['\\233'] = string.rep getfenv(0)['\\233']()",
            b"bad argument #1 to `\xe9' (string expected, got no value)",
        );
        ends(
            "local t = {} t['\\233'] = 1 t['\\233']()",
            b"attempt to call field `\xe9' (a number value)",
        );
        // A chunk named by its own source keeps its bytes, through `RunScript` too.
        ends(
            "local f, e = loadstring('return \\233 +') error(e, 0)",
            b"[string \"return \xe9 +\"]:1: unexpected symbol near `\xe9'",
        );
    }

    /// The error handler is called with the value raised, as `lua_pcall` hands its error
    /// function the value (`0x703b40`): a table as itself, a message with its bytes.
    #[test]
    fn the_error_handler_hears_the_value_raised() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "caught = {} seterrorhandler(function(e) table.insert(caught, e) end) \
             E = {} local f = CreateFrame('Frame') f:RegisterEvent('PLAYER_ALIVE') \
             f:SetScript('OnEvent', function() error(E) end) \
             local g = CreateFrame('Frame') g:RegisterEvent('PLAYER_ALIVE') \
             g:SetScript('OnEvent', function() error('\\233', 0) end)",
        )
        .unwrap();
        s.fire_event("PLAYER_ALIVE", vec![]);
        s.run("RunScript('return \\233 +')").unwrap();
        s.dispatch_script_errors_to_handler();
        // `RunScript`'s chunk is named by its source's bytes (`0x48b9bb`).
        assert!(s
            .eval::<bool>(
                "return caught[3] == '[string \"return \\233 +\"]:1: unexpected symbol near `\\233\\''"
            )
            .unwrap());
        assert!(s.eval::<bool>("return caught[1] == E").unwrap());
        assert!(s.eval::<bool>("return caught[2] == '\\233'").unwrap());
    }

    /// A Lua error that comes back through a Rust callback goes on as it was raised: the value
    /// itself, as `lua_error` raises what a protected call left, never its text again, and never
    /// with a second position.
    #[test]
    fn a_lua_error_passes_through_a_callback_unchanged() {
        let s = UiScript::new().unwrap();
        let through = s
            .lua()
            .create_function(|_, f: mlua::Function| f.call::<()>(()))
            .unwrap();
        s.lua().globals().set("Through", through).unwrap();
        assert!(s
            .eval::<bool>(
                "local E = {} local ok, e = pcall(Through, function() error(E) end) return e == E"
            )
            .unwrap());
        assert_eq!(
            s.lua()
                .load(
                    "local ok, e = pcall(function() Through(function() error('boom') end) end) \
                     return e"
                )
                .set_name("=Y.lua")
                .eval::<String>()
                .unwrap(),
            "Y.lua:1: boom"
        );
        assert!(s
            .eval::<bool>(
                "local ok, e = pcall(Through, function() error('\\233', 0) end) return e == '\\233'"
            )
            .unwrap());
    }

    /// `select` is 5.1's; 5.0 counts arguments with `arg.n`, and the host with
    /// [`UiScript::arity`]. Both must tell zero returns from one `nil`, or an arity check passes a
    /// binding that returns nothing.
    #[test]
    fn select_is_not_a_1_12_global_and_arg_n_answers_instead() {
        let s = UiScript::new().unwrap();
        assert!(
            s.eval::<bool>("return select == nil").unwrap(),
            "`select` is 5.1's base library; `reference/lua_globals.tsv` has no row for it"
        );

        // 5.0's spelling, as an addon writes it.
        assert_eq!(
            s.eval::<Vec<i64>>(
                "local function n(...) return arg.n end \
                 local function two_with_a_nil() return 1, nil end \
                 local function nothing() end \
                 return { n(), n(nil), n(two_with_a_nil()), n(nothing()), n(1, 2, 3) }",
            )
            .unwrap(),
            vec![0, 1, 2, 0, 3],
            "arg.n must count trailing nils AND tell zero returns from one nil"
        );

        // And the host's.
        assert_eq!(s.arity("nil").unwrap(), 1, "one nil is one value");
        assert_eq!(s.arity("ShowNameplates()").unwrap(), 0, "and zero is zero");
    }

    /// 1.12's string type has no metatable, so a method call on a string raises, in the
    /// reference's words: ``attempt to %s a %s value`` (`0x871c10`) and
    /// ``attempt to %s %s `%s' (a %s value)`` (`0x871c2c`).
    #[test]
    fn the_string_type_has_no_metatable() {
        let s = UiScript::new().unwrap();
        assert!(
            s.eval::<bool>(r#"return getmetatable("") == nil"#).unwrap(),
            "5.1's luaopen_string ends in a createmetatable 5.0's does not have"
        );

        let literal: String = s
            .eval(
                r#"local ok, err = pcall(function() return ("abc"):upper() end) return tostring(err)"#,
            )
            .unwrap();
        assert!(
            literal.contains("attempt to index a string value"),
            "5.0's own luaG_typeerror wording: {literal:?}"
        );
        let named: String = s
            .eval(
                "local ok, err = pcall(function() local v = 'abc' return v:sub(1, 2) end) \
                 return tostring(err)",
            )
            .unwrap();
        assert!(
            named.contains("attempt to index local `v' (a string value)"),
            "the named form, with 5.0's backtick-apostrophe quoting: {named:?}"
        );

        // Only the method dispatch goes: the string library and its bare aliases still work.
        s.run(
            r#"assert(string.upper("ab") == "AB")
               assert(string.sub("hello", 2, 3) == "el")
               assert(string.find("hello", "ll") == 3)
               assert(strupper("ab") == "AB" and strsub("hello", 2, 3) == "el")
               assert(format("%2$s %1$s", "a", "b") == "b a")"#,
        )
        .unwrap();
    }

    /// `bit.mod(INT32_MIN, -1)` is 0: Rust's `%` panics on it, in a release build too.
    #[test]
    fn bit_mod_of_int_min_by_minus_one_is_zero() {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(|| {
                let s = UiScript::new().unwrap();
                let got: Vec<i64> = s
                    .eval("return { bit.mod(-2^31, -1), bit.mod(2^31, -1), bit.mod(-7, 2) }")
                    .unwrap();
                assert_eq!(got, vec![0, 0, -1]);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
