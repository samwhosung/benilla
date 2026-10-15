//! The four globals the reference registers only to refuse: `GMRequestPlayerInfo` (`0x48d4c0`)
//! and the file verbs `ReadFile`, `DeleteFile` and `AppendToFile` (`0x515f50`, `0x515f70`,
//! `0x515f90`). Each is `luaL_error("Access Denied")` (`0x842bd0`) before it reads an argument.

use mlua::{Lua, Value, Variadic};

/// Register the four refusals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    for name in [
        "GMRequestPlayerInfo",
        "ReadFile",
        "DeleteFile",
        "AppendToFile",
    ] {
        g.set(
            name,
            lua.create_function(|_, _args: Variadic<Value>| -> mlua::Result<()> {
                Err(mlua::Error::runtime("Access Denied"))
            })?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn the_four_refusals_raise_access_denied_whatever_they_are_given() {
        let s = UiScript::new().unwrap();
        for name in [
            "GMRequestPlayerInfo",
            "ReadFile",
            "DeleteFile",
            "AppendToFile",
        ] {
            for args in ["", "'x'", "'x', 'y'", "nil", "1, 2, 3"] {
                let call = format!("{name}({args})");
                let err = s.run(&call).expect_err(&call).to_string();
                assert!(err.contains("Access Denied"), "{call}: {err}");
            }
        }
        // The message alone, as `luaL_error` at a C function adds no position.
        let err = s.run("ReadFile('x')").unwrap_err().to_string();
        assert!(!err.contains("attempt"), "{err}");
    }
}
