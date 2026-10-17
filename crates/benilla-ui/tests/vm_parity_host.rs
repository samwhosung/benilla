//! The script host's Rust-facing behaviour: cases that need a host callback, which `vm_parity`
//! cannot write in Lua. Each expected answer is the one the C Lua build gave before benilla-lua
//! replaced it.

use benilla_ui::mlua::{self, Function, Lua, Table, Value, Variadic};
use benilla_ui::script::UiScript;

fn eval(s: &UiScript, src: &str) -> String {
    match s.lua().load(src).set_name("=t").eval::<String>() {
        Ok(v) => v,
        Err(e) => format!("<raised> {e}"),
    }
}

/// An error's message alone: the C build's carries mlua's category and a traceback.
fn message(e: &mlua::Error) -> String {
    let s = match e {
        mlua::Error::RuntimeError(m) | mlua::Error::LuaValue { message: m, .. } => m.clone(),
        other => other.to_string(),
    };
    s.lines().next().unwrap_or("").to_string()
}

/// `Table::get` inside a callback inside `xpcall` is its own protected call: its error is the
/// callback's to handle, and the enclosing handler does not run (mlua's `lua_pcall` has none).
#[test]
fn a_host_table_access_runs_no_enclosing_handler() {
    let s = UiScript::new().unwrap();
    let lua = s.lua();
    let cb = lua
        .create_function(|_, t: Table| {
            Ok(match t.get::<Value>("x") {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("err {}", message(&e)),
            })
        })
        .unwrap();
    lua.globals().set("cb", cb).unwrap();
    assert_eq!(
        eval(
            &s,
            r#"local n = 0
            local t = setmetatable({}, {__index = function() error("boom", 0) end})
            local ok, r = xpcall(function() return cb(t) end, function(m) n = n + 1 return "H:" .. m end)
            return tostring(ok) .. " " .. tostring(r) .. " " .. n"#
        ),
        "true err boom 0"
    );
}

/// `Function::set_environment` on a running function: its next global read sees the new one.
#[test]
fn a_running_function_sees_its_new_environment() {
    let s = UiScript::new().unwrap();
    let lua = s.lua();
    let setenv = lua
        .create_function(|_, (f, t): (Function, Table)| f.set_environment(t))
        .unwrap();
    lua.globals().set("setenvof", setenv).unwrap();
    assert_eq!(
        eval(
            &s,
            r#"x = "old"
            local E = {x = "new"}
            function F() local before = x setenvof(F, E) return before .. "/" .. x end
            return F()"#
        ),
        "old/new"
    );
}

/// A callback answering another state's value panics, as mlua's does; the panic is a Lua error
/// `pcall` catches, and the state stays sound however many times it happens.
#[test]
fn a_callback_answering_a_foreign_value_is_caught_and_leaves_the_state_sound() {
    let s = UiScript::new().unwrap();
    let lua = s.lua();
    let other = Lua::new();
    let foreign = other.create_table().unwrap();
    let cb = lua
        .create_function(move |_, ()| Ok(foreign.clone()))
        .unwrap();
    lua.globals().set("foreign", cb).unwrap();
    assert_eq!(
        eval(
            &s,
            r#"local ok, e = pcall(foreign) return tostring(ok) .. " " .. type(e)"#
        ),
        "false userdata"
    );
    assert_eq!(
        eval(
            &s,
            r#"for i = 1, 300 do pcall(foreign) end
            local f = loadstring("return 1 + 1") return tostring(f())"#
        ),
        "2"
    );
}

/// A Rust callback's arguments and results share 5.1's `LUAI_MAXCSTACK`, 8000 slots, as mlua's
/// `check_stack` allows: one argument and 7998 results fit, one more raises.
#[test]
fn a_callbacks_arguments_and_results_share_the_c_stack() {
    let s = UiScript::new().unwrap();
    let lua = s.lua();
    let many = lua
        .create_function(|_, k: usize| {
            let mut v = Variadic::with_capacity(k);
            v.extend(1..=k);
            Ok(v)
        })
        .unwrap();
    lua.globals().set("many", many).unwrap();
    assert_eq!(
        eval(
            &s,
            r#"local function n(...) return arg.n end
            local most = 0
            for k = 7990, 8005 do if pcall(function() return n(many(k)) end) then most = k end end
            return tostring(most)"#
        ),
        "7998"
    );
}
