//! The Lua 5.0 dialect: the 5.1 VM's libraries reshaped to answer as the 1.12 client's Lua 5.0.
//!
//! The reference embeds Lua 5.0 (`$Lua: Lua 5.0` at `0x811b30`), and `InitLua 0x7039e0` opens
//! five libraries: base (`0x811e28`, 36 entries), `string` (`0x822d88`, 12), `table`
//! (`0x822d40`, 8), `math` (`0x822c60`, 24) and `bit` (`0x822c18`, 8), with no `os`, `io`,
//! `debug` or `coroutine`. The grammar is the Lua fork's, in `third_party/lua-src`.
//!
//! `...` is a parameter-list token only: a vararg function reads its arguments through 5.0's
//! `arg` table (`arg.n`, `unpack(arg)`). So Lua written here forwards an unknown argument list
//! only through a table, and a verb whose arity matters (`GetPoint()` against `GetPoint(nil)`)
//! stays in Rust.

use mlua::{Lua, Table, Value};

/// The Lua 5.1 library members 5.0 lacks, removed so an addon's feature test
/// (`string.gmatch or string.gfind`) takes the branch 1.12 gives it.
const REMOVED: &[(&str, &[&str])] = &[
    // 5.0 has `gfind`, and no `gmatch`, `match` or `reverse`.
    ("string", &["gmatch", "match", "reverse"]),
    // 5.0's eight are concat, foreach, foreachi, getn, setn, sort, insert and remove.
    ("table", &["maxn"]),
    // 5.0's `mod` is 5.1's `fmod` (and a bare global in 1.12); 5.0 has no `modf`, `huge` or
    // hyperbolics.
    ("math", &["fmod", "modf", "huge", "cosh", "sinh", "tanh"]),
];

/// Lua 5.0's remembered table size and the `n`-based table library that reads and updates it.
mod table_size;

/// Install the dialect. Runs before the WoW stdlib layer, so its aliases bind the 5.0-shaped
/// functions rather than the 5.1 ones they replace.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── 1 · the `n`-based table library ───────────────────────────────────────────────────────
    table_size::install(lua)?;

    // ── 2 · the 5.1-only members go ───────────────────────────────────────────────────────────
    for (lib, names) in REMOVED {
        let Ok(t) = g.get::<Table>(*lib) else {
            continue;
        };
        for name in *names {
            t.set(*name, Value::Nil)?;
        }
    }
    // `InitLua` never opens `coroutine`.
    g.set("coroutine", Value::Nil)?;

    // `select` is 5.1's base library and has no row in 1.12's `_G`. A vararg function counts its
    // arguments with `arg.n`, and host code asks `UiScript::arity`.
    g.set("select", Value::Nil)?;

    // 1.12's string type has no metatable, so `("x"):upper()` raises `attempt to index a string
    // value`. There it cannot carry one: `luaT_gettmbyobj 0x6f7bd0` finds metamethods only on
    // tables and userdata, `lua_setmetatable 0x6f4020` writes only those two tags, and
    // `luaopen_string 0x7fd810` installs none, where 5.1's does. Base `setmetatable` (`0x702a40`)
    // rejects a non-table in both.
    lua.set_type_metatable::<mlua::String>(None);

    // ── 3 · the 5.0 compat globals ────────────────────────────────────────────────────────────
    // The client's compat chunk (`0x8722e8`) publishes these three bare, and no bare `setn`
    // (`reference/1.12-globals.tsv`).
    let table: Table = g.get("table")?;
    for name in ["sort", "foreach", "foreachi"] {
        if let Ok(f) = table.get::<Value>(name) {
            if !matches!(f, Value::Nil) {
                g.set(name, f)?;
            }
        }
    }
    // Bound here, not left to the stdlib layer, so the bare `getn` is `table.getn` itself.
    g.set("getn", table.get::<Value>("getn")?)?;

    // ── 4 · the base library's own shape ──────────────────────────────────────────────────────
    // `print` and `_VERSION` are neither in the base array (`0x811e28`) nor in 1.12's `_G`.
    g.set("print", Value::Nil)?;
    g.set("_VERSION", Value::Nil)?;

    // `__pow` is a 1.12 global, which 5.0 calls to implement `^`; nothing here calls it, but an
    // addon can see it.
    g.set(
        "__pow",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(a.powf(b)))?,
    )?;

    install_bit(lua)?;
    install_gc(lua)?;
    install_assert(lua)?;

    Ok(())
}

/// The `bit` library 1.12 opens (`0x7fadc0`, array `0x822c18`): its eight functions, in the
/// array's order, on 32-bit two's complement.
fn install_bit(lua: &Lua) -> mlua::Result<()> {
    let bit = lua.create_table()?;
    // The client truncates to a 32-bit int and answers signed. `as i64 as u32` is C's wrapping
    // `(unsigned)(int)x`, not a saturating cast, so `bnot(0)` is -1 as there.
    fn u32_of(v: f64) -> u32 {
        v as i64 as u32
    }
    fn out(v: u32) -> i64 {
        v as i32 as i64
    }
    bit.set(
        "bnot",
        lua.create_function(|_, a: f64| Ok(out(!u32_of(a))))?,
    )?;
    bit.set(
        "band",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(out(u32_of(a) & u32_of(b))))?,
    )?;
    bit.set(
        "bor",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(out(u32_of(a) | u32_of(b))))?,
    )?;
    bit.set(
        "bxor",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(out(u32_of(a) ^ u32_of(b))))?,
    )?;
    // Shift counts mask to 5 bits, as the client's x86 shifts do: `lshift(1, 32)` is 1.
    bit.set(
        "lshift",
        lua.create_function(|_, (a, n): (f64, f64)| Ok(out(u32_of(a) << (u32_of(n) & 31))))?,
    )?;
    bit.set(
        "rshift",
        lua.create_function(|_, (a, n): (f64, f64)| Ok(out(u32_of(a) >> (u32_of(n) & 31))))?,
    )?;
    bit.set(
        "arshift",
        lua.create_function(|_, (a, n): (f64, f64)| {
            Ok(out(((u32_of(a) as i32) >> (u32_of(n) & 31)) as u32))
        })?,
    )?;
    // `bit.mod`: an integer remainder, unlike `math.mod`'s float one.
    bit.set(
        "mod",
        lua.create_function(|_, (a, b): (f64, f64)| {
            let b = u32_of(b) as i32;
            if b == 0 {
                return Err(mlua::Error::runtime("bit.mod: division by zero"));
            }
            Ok(((u32_of(a) as i32) % b) as i64)
        })?,
    )?;
    lua.globals().set("bit", bit)
}

/// `gcinfo` and `collectgarbage` in their 5.0 shapes. `gcinfo` (`0x703200`) takes nothing and
/// answers `(count_KB, threshold_KB)` (`lua_getgccount 0x6f43f0`, `lua_getgcthreshold
/// 0x6f43e0`); `collectgarbage` (`0x703250`) takes an optional number, sets the threshold to that
/// many KB (`lua_setgcthreshold 0x6f4400`) and answers nothing, so 5.1's string options raise.
/// 5.1's collector has no threshold, so it is kept here by 5.0's rules against mlua's heap: a
/// live count that reaches it collects, and a collection resets it to twice the live count.
fn install_gc(lua: &Lua) -> mlua::Result<()> {
    /// 5.0's `GCthreshold` (`[G+0x24]`), in bytes.
    const REG_GC_THRESHOLD: &str = "__benilla_gc_threshold";
    /// `lua_setgcthreshold 0x6f4400`'s unsigned bound, in kilobytes.
    const MAX_THRESHOLD_KB: i64 = 0x3f_ffff;

    fn live_bytes(lua: &Lua) -> usize {
        lua.used_memory()
    }
    /// 5.0's post-collection rule, `luaC_collectgarbage`: `GCthreshold = 2 * nblocks`.
    fn settle(lua: &Lua) -> mlua::Result<()> {
        let t = u64::try_from(live_bytes(lua))
            .unwrap_or(u64::MAX)
            .saturating_mul(2);
        lua.set_named_registry_value(REG_GC_THRESHOLD, t as f64)
    }

    settle(lua)?;

    // `gcinfo()`: no argument, two numbers, never raises.
    lua.globals().set(
        "gcinfo",
        lua.create_function(|lua, ()| {
            let live = live_bytes(lua);
            let mut threshold: f64 = lua.named_registry_value(REG_GC_THRESHOLD).unwrap_or(0.0);
            // 5.0 collects the moment the live count reaches the threshold (`luaC_checkGC`), so a
            // script never sees a threshold at or below the count. With no allocation hook here,
            // the same rule runs on read.
            if (live as f64) >= threshold {
                settle(lua)?;
                threshold = lua.named_registry_value(REG_GC_THRESHOLD).unwrap_or(0.0);
            }
            // Both in KB, the reference's `shr eax,0xa`.
            Ok(((live >> 10) as f64, (threshold as u64 >> 10) as f64))
        })?,
    )?;

    // `collectgarbage([n])`: a number, no return values, and a raise on anything else.
    lua.globals().set(
        "collectgarbage",
        lua.create_function(|lua, arg: Value| {
            // `luaL_optnumber(L, 1, 0.0)`: absent or nil is the default, a numeric string is
            // coerced (`lua_tonumber` runs `strtod`), anything else is a type error.
            let n = match &arg {
                Value::Nil => 0.0,
                Value::Integer(i) => *i as f64,
                Value::Number(n) => *n,
                Value::String(s) => {
                    match s.to_str().ok().and_then(|t| t.trim().parse::<f64>().ok()) {
                        Some(v) => v,
                        None => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "bad argument #1 to `collectgarbage' (number expected, got {})",
                                type_name(&arg)
                            )))
                        }
                    }
                }
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "bad argument #1 to `collectgarbage' (number expected, got {})",
                        type_name(other)
                    )))
                }
            };
            // `_ftol` truncates toward zero; `0x6f4400`'s unsigned compare saturates a negative
            // or oversized count to `0xffffffff` bytes.
            let kb = n.trunc();
            let threshold_bytes: f64 = if !(0.0..=MAX_THRESHOLD_KB as f64).contains(&kb) {
                u32::MAX as f64
            } else {
                kb * 1024.0
            };
            lua.set_named_registry_value(REG_GC_THRESHOLD, threshold_bytes)?;
            // `luaC_checkGC`: a live count at the threshold collects, and the collection
            // re-settles it to `2 * nblocks`.
            if (live_bytes(lua) as f64) >= threshold_bytes {
                lua.gc_collect()?;
                settle(lua)?;
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

/// `assert` in its 5.0 shape (`luaB_assert 0x7031a0`): it answers only its first argument
/// (`lua_settop(L, 1)` at `0x7031e5`) where 5.1 answers them all, and a false value raises
/// argument 2 or `assertion failed!` (`0x872bdc`) through `luaL_error 0x6f4940`.
fn install_assert(lua: &Lua) -> mlua::Result<()> {
    let f = lua.create_function(|lua, args: mlua::MultiValue| {
        let mut it = args.into_iter();
        // `luaL_checkany 0x6f4bb0`: an absent argument 1 raises; an explicit `nil` takes the
        // falsy leg below.
        let Some(v) = it.next() else {
            return Err(mlua::Error::RuntimeError(
                "bad argument #1 to `assert' (value expected)".into(),
            ));
        };
        if matches!(v, Value::Nil | Value::Boolean(false)) {
            // `luaL_optlstring`: a string or a number; anything else raises its own bad
            // argument, the type spelled as `luaT_typenames 0x811cd0` spells it.
            let msg = match it.next() {
                None | Some(Value::Nil) => "assertion failed!".to_string(),
                Some(other) => match lua.coerce_string(other.clone())? {
                    Some(s) => s.to_string_lossy(),
                    None => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "bad argument #2 to `assert' (string expected, got {})",
                            type_name(&other)
                        )))
                    }
                },
            };
            return Err(mlua::Error::RuntimeError(msg));
        }
        // `lua_settop(L, 1)`: the first argument alone, whatever else was passed.
        Ok(v)
    })?;
    lua.globals().set("assert", f)
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Nil => "no value",
        Value::Boolean(_) => "boolean",
        Value::Integer(_) | Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Table(_) => "table",
        Value::Function(_) => "function",
        Value::Thread(_) => "thread",
        _ => "userdata",
    }
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
            "(number,number) — `1.12-shapes.tsv` types the row `agree`"
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
        // Two's complement, signed out: `bnot(0)` is -1, not 4294967295.
        assert_eq!(s.eval::<i64>("return bit.bnot(0)").unwrap(), -1);
        assert_eq!(
            s.eval::<i64>("return bit.band(0x1234, 0xFF)").unwrap(),
            0x34
        );
        assert_eq!(s.eval::<i64>("return bit.bor(0xF0, 0x0F)").unwrap(), 0xFF);
        assert_eq!(s.eval::<i64>("return bit.bxor(0xFF, 0x0F)").unwrap(), 0xF0);
        assert_eq!(s.eval::<i64>("return bit.lshift(1, 4)").unwrap(), 16);
        // `rshift` is logical, `arshift` propagates the sign.
        assert_eq!(s.eval::<i64>("return bit.rshift(-1, 28)").unwrap(), 15);
        assert_eq!(s.eval::<i64>("return bit.arshift(-1, 28)").unwrap(), -1);
        // The shift count masks to 5 bits, as the x86 instruction does.
        assert_eq!(s.eval::<i64>("return bit.lshift(1, 32)").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return bit.mod(17, 5)").unwrap(), 2);
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

    /// The 5.0 grammar the stock files use still parses, including the three constructs the fork
    /// restores: the iterator-less generic-for, nested long strings (`LUA_COMPAT_LSTR`) and the
    /// constructor's extra semicolon.
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
            -- nesting long strings (LUA_COMPAT_LSTR = 2)
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

    /// Error messages quote a program element as the reference does, `` `x' ``: the fork's
    /// `luaconf.h` restores 5.0's `LUA_QL`, and `debugstack` renders its own frames the same way
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

    /// `select` is 5.1's; 5.0 counts arguments with `arg.n`, and the host with
    /// [`UiScript::arity`]. Both must tell zero returns from one `nil`, or an arity check passes a
    /// binding that returns nothing.
    #[test]
    fn select_is_not_a_1_12_global_and_arg_n_answers_instead() {
        let s = UiScript::new().unwrap();
        assert!(
            s.eval::<bool>("return select == nil").unwrap(),
            "`select` is 5.1's base library; `reference/1.12-globals.tsv` has no row for it"
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
}
