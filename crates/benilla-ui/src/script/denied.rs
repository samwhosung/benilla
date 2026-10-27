//! The four globals the reference registers only to refuse: `GMRequestPlayerInfo` (`0x48d4c0`),
//! in-game only, and the file verbs `ReadFile`, `DeleteFile` and `AppendToFile` (`0x515f50`,
//! `0x515f70`, `0x515f90`), in both states. Each is `luaL_error("Access Denied")` (`0x842bd0`)
//! before it reads an argument.

use mlua::{Lua, Value, Variadic};

/// Register `name` as a refusal: it raises "Access Denied" before it reads an argument.
fn refuse(lua: &Lua, name: &str) -> mlua::Result<()> {
    lua.globals().set(
        name,
        lua.create_function(|_, _args: Variadic<Value>| -> mlua::Result<()> {
            Err(mlua::Error::runtime("Access Denied"))
        })?,
    )
}

/// The file trio, which the registrar both states share runs (`0x51bb20`, table `0x8503f4`, after
/// the clock and `ConsoleExec`): the glue builder (`0x46abb0`) as well as `UI_Init`.
pub(super) fn install_files(lua: &Lua) -> mlua::Result<()> {
    for name in ["ReadFile", "DeleteFile", "AppendToFile"] {
        refuse(lua, name)?;
    }
    Ok(())
}

/// `GMRequestPlayerInfo` (`0x48d4c0`), which only `UI_Init` registers.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    refuse(lua, "GMRequestPlayerInfo")
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// The glue VM has the file trio and not the in-game `GMRequestPlayerInfo` (`0x51bb20` runs in
    /// the glue builder `0x46abb0`).
    #[test]
    fn the_glue_vm_refuses_the_file_trio() {
        let s = UiScript::new_glue().unwrap();
        for name in ["ReadFile", "DeleteFile", "AppendToFile"] {
            let call = format!("{name}('x')");
            let err = s.run(&call).expect_err(&call).to_string();
            assert!(err.contains("Access Denied"), "{call}: {err}");
        }
        assert!(s.eval::<bool>("return GMRequestPlayerInfo == nil").unwrap());
    }

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
