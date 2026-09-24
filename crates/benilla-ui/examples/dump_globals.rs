//! `dump_globals` — print benilla's Lua global namespace, asked of a real VM.
//!
//! ```text
//! cargo run -q -p benilla-ui --example dump_globals            # name<TAB>type, one per line
//! cargo run -q -p benilla-ui --example dump_globals --members  # ...the stdlib TABLES' members too
//! ```
//!
//! **The point is that it is a run, not a grep**. Every wrong number in the
//! addon arc came from measuring our API surface by pattern-matching Rust source: a regex over
//! `.set("Name", …)` misses the Lua prelude, miscounts a `format!`-registered family, and cannot
//! see what the sandbox removed. When the question is what a running system exposes, ask the
//! running system — this is the four-minute answer 1188 §4 describes, made permanent.
//!
//! This is [`UiScript::new`]'s namespace: the Rust bindings plus the Lua stdlib prelude, i.e. what
//! an addon sees *before* any FrameXML loads. That is deliberately the engine half — it is
//! deterministic, needs no install, and is exactly what `scripts/api-coverage.sh` compares against
//! `reference/1.12-globals.tsv`'s `engine` and `lua` rows.
//!
//! ## `--members`, and the blind spot it closes
//!
//! `_G` is not the whole surface an addon can tell apart. `table.setn` is not a global — it is a
//! member of the `table` table — and it stopped **61 of 218** real addons dead, because mlua's Lua
//! 5.1 raises `'setn' is obsolete` where the 1.12 client's Lua 5.0 does the thing. A `_G`-only
//! instrument cannot see that, and did not: the arc measured coverage for two sessions with this
//! gap wide open. `--members` prints `table.setn`-style rows for the stdlib tables, so the dialect
//! is measurable the same way the API surface is.
//!
//! It also prints the **per-type metatables**, which is the same blind spot one
//! layer further down and stayed open longer because a metatable is not a *name*: 5.1 gives the
//! string type one and 5.0 cannot have one, so `("x"):upper()` worked here and raised on the
//! reference for as long as nothing thought to look.
use benilla_ui::script::UiScript;

/// The tables whose membership an addon can observe and depend on.
///
/// Deliberately a fixed list, not "every table-valued global": `_G` would recurse, and a *frame*
/// table's members are the widget API, which is a different question with a different reference.
const STDLIB: &[&str] = &["string", "table", "math", "coroutine", "os", "io", "debug"];

fn main() -> mlua::Result<()> {
    let script = UiScript::new()?;
    // Joined Lua-side into one `name<TAB>type` string per entry: a `Vec<String>` crosses the mlua
    // boundary directly, where a `Vec<(String, String)>` does not.
    let mut rows: Vec<String> = script.eval(
        "local out = {} \
         for k, v in pairs(_G) do \
           if type(k) == 'string' then table.insert(out, k .. '\\t' .. type(v)) end \
         end \
         return out",
    )?;

    if std::env::args().any(|a| a == "--members") {
        for lib in STDLIB {
            let members: Vec<String> = script.eval(&format!(
                "local t = {lib} \
                 local out = {{}} \
                 if type(t) == 'table' then \
                   for k, v in pairs(t) do \
                     if type(k) == 'string' then \
                       table.insert(out, '{lib}.' .. k .. '\\t' .. type(v)) \
                     end \
                   end \
                 end \
                 return out"
            ))?;
            rows.extend(members);
        }

        // **The per-type metatables — the surface `_G` and the member lists both miss** (decision
        // 2171). `table.setn` taught 1194 that a `_G`-only instrument cannot see a *member*; the
        // string metatable is the same lesson one layer further down, and it stayed invisible for
        // longer because it is not a name at all. 1.12 installs none — `lua_setmetatable 0x6f4020`
        // accepts only LUA_TTABLE and LUA_TUSERDATA and returns 0 for every other tag, so a
        // primitive type there *cannot* carry one — while stock 5.1 gives `string` a metatable
        // whose `__index` is the `string` table itself, which is what makes `("x"):upper()` work
        // on 5.1 and raise on 5.0. Every row here should read `nil`.
        //
        // Written as a fixed list rather than a table because a `nil` value has no entry to
        // iterate — the one type whose metatable slot is easiest to forget is the one a
        // constructor silently drops.
        let metatables: Vec<String> = script.eval(
            "local out = {} \
             local function probe(name, v) \
               table.insert(out, '<' .. name .. '>\\tmetatable ' .. type(getmetatable(v))) \
             end \
             probe('string', '') \
             probe('number', 0) \
             probe('boolean', true) \
             probe('nil', nil) \
             probe('function', probe) \
             return out",
        )?;
        rows.extend(metatables);
    }

    rows.sort();
    for row in rows {
        println!("{row}");
    }
    Ok(())
}
