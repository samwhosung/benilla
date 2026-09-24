//! `dump_globals`: print benilla's Lua global namespace, asked of a real VM.
//!
//! ```text
//! cargo run -q -p benilla-ui --example dump_globals            # name<TAB>type, one per line
//! cargo run -q -p benilla-ui --example dump_globals --members  # plus the stdlib tables' members
//! ```
//!
//! The namespace is [`UiScript::new`]'s, the engine half before any FrameXML loads, which
//! `scripts/api-coverage.sh` compares with `reference/1.12-globals.tsv`. `--members` adds what `_G`
//! cannot show: the stdlib tables' members (`table.setn` is one) and the per-type metatables.
use benilla_ui::script::UiScript;

/// The stdlib tables whose members an addon can observe: a fixed list, since `_G` would recurse
/// and a frame's members are the widget API.
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

        // The per-type metatables. Every row should read `nil`: 1.12's `lua_setmetatable`
        // (`0x6f4020`) takes only tables and userdata, where 5.1 gives strings one whose `__index`
        // makes `("x"):upper()` work. Probed one by one, since a `nil` has no table entry.
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
