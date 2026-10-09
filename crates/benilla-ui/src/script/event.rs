//! Handler firing: the FrameScript calling convention for events, ticks and show/hide. The
//! reference sets `this` (`0x872e64`), `event` (`0x84b648`) and `arg1..argN` (`0x8722dc`) around
//! the call, restoring each after, and `pcall`s with no arguments; the handler here also gets the
//! same values as `(self, event, ...)` arguments, which 1.12 does not pass.
//! Handler errors return to the caller, which records them in [`super::Model::errors`].

use std::borrow::Cow;

use mlua::{Function, Lua, Table, Value};

use super::{Model, ScriptValue, REG_SCRIPTS};
use crate::script::object::frame_wrapper;

/// `UiScript::fire_event` for engine code that holds only the Lua context.
pub(super) fn fire_global(lua: &Lua, event: &str, args: &[ScriptValue]) {
    super::tick::fire_event_into(lua, event, args.to_vec());
}

/// The `RegisterAllEvents()` half of a dispatch: the all-events frames, after the event's own
/// listeners and skipping any already visited. The next frame is re-found by position each step
/// (`0x703ee8`), so a handler that unregisters the walk's successor ends the dispatch there.
pub(super) fn fire_all_event_listeners(lua: &Lua, event: &str, args: &[ScriptValue]) {
    let model_mut = || lua.app_data_mut::<Model>().expect("model app_data set");
    let mut at = model_mut().all_event_frames.first().copied();
    while let Some(h) = at {
        let mut model = model_mut();
        let Some(pos) = model.all_event_frames.iter().position(|&x| x == h) else {
            break;
        };
        let next = model.all_event_frames.get(pos + 1).copied();
        let already = model
            .event_to_frames
            .get(event)
            .is_some_and(|l| l.contains(&h));
        let id = model.frame_id(h);
        drop(model);
        if !already {
            if let Err(e) = fire_event_handler(lua, id, event, args) {
                lua.app_data_mut::<Model>()
                    .expect("model app_data")
                    .record_script_error(e.to_string());
            }
        }
        at = next;
    }
}

/// Fire a frame's `OnEvent` (both conventions) with the given args.
pub(super) fn fire_event_handler(
    lua: &Lua,
    id: u32,
    event: &str,
    args: &[ScriptValue],
) -> mlua::Result<()> {
    let extra: Vec<Value> = args
        .iter()
        .cloned()
        .map(|a| a.into_lua(lua))
        .collect::<mlua::Result<_>>()?;
    fire(lua, id, "OnEvent", Some(event), extra)
}

/// Fire a frame's `OnUpdate` with `arg1 = elapsed`.
pub(super) fn fire_update_handler(lua: &Lua, id: u32, elapsed: f32) -> mlua::Result<()> {
    fire(
        lua,
        id,
        "OnUpdate",
        None,
        vec![Value::Number(f64::from(elapsed))],
    )
}

/// Fire a handler that carries no `event` name (`OnEnter`, `OnClick`, `OnValueChanged`, ...), with
/// `extra` as its `arg1..argN`; a no-op when the frame has no such script.
pub(super) fn fire_widget_handler(
    lua: &Lua,
    id: u32,
    script: &str,
    extra: Vec<Value>,
) -> mlua::Result<()> {
    fire(lua, id, script, None, extra)
}

/// Drain [`Model::pending_size_changed`], queued by the resolve pass, and fire `OnSizeChanged` for
/// each, recording errors. It drains once: a size a handler changes fires on the next resolve, as
/// the reference's `ApplyRect` does, so a handler that grows its own frame cannot spin forever.
pub(super) fn fire_size_changes(lua: &Lua) {
    let pending = std::mem::take(
        &mut lua
            .app_data_mut::<Model>()
            .expect("model")
            .pending_size_changed,
    );
    for (id, w, h) in pending {
        if let Err(e) = fire(
            lua,
            id,
            "OnSizeChanged",
            None,
            vec![Value::Number(f64::from(w)), Value::Number(f64::from(h))],
        ) {
            lua.app_data_mut::<Model>()
                .expect("model")
                .record_script_error(e.to_string());
        }
    }
}

/// Whether a handler is bound under `script`, without firing it: the keyboard walk consumes on
/// the slot's presence (`0x76b7d0`). A failed lookup reads absent, so the gate never raises.
pub(super) fn has_widget_handler(lua: &Lua, id: u32, script: &str) -> bool {
    let Ok(scripts) = lua.named_registry_value::<Table>(REG_SCRIPTS) else {
        return false;
    };
    match scripts.get::<Value>(id) {
        Ok(Value::Table(per)) => matches!(per.get::<Value>(script), Ok(Value::Function(_))),
        _ => false,
    }
}

/// The one firing path: `event_name` is `OnEvent`'s, `extra` the `arg1..argN`. Holds no model
/// borrow across the call.
fn fire(
    lua: &Lua,
    id: u32,
    script: &str,
    event_name: Option<&str>,
    extra: Vec<Value>,
) -> mlua::Result<()> {
    let scripts: Table = lua.named_registry_value(REG_SCRIPTS)?;
    let func: Function = match scripts.get::<Value>(id)? {
        Value::Table(per) => match per.get::<Value>(script)? {
            Value::Function(f) => f,
            _ => return Ok(()),
        },
        _ => return Ok(()),
    };

    let wrapper = frame_wrapper(lua, id)?;
    // The profiler's fire opens after `frame_wrapper`, where no model borrow is held, and closes
    // when the guard drops, unwinds included.
    let _fire =
        super::handler_prof::armed().then(|| super::handler_prof::Fire::open(lua, id, script));
    invoke_with_globals(lua, wrapper, &func, event_name, extra)
}

/// The `argN` global names, spelled out so the common arities allocate nothing.
const ARG_NAMES: [&str; 16] = [
    "arg1", "arg2", "arg3", "arg4", "arg5", "arg6", "arg7", "arg8", "arg9", "arg10", "arg11",
    "arg12", "arg13", "arg14", "arg15", "arg16",
];

/// The global name for arg `i`, 1-based like the globals.
fn arg_name(i: usize) -> Cow<'static, str> {
    match ARG_NAMES.get(i - 1) {
        Some(&name) => Cow::Borrowed(name),
        None => Cow::Owned(format!("arg{i}")),
    }
}

/// The calling convention, shared by [`fire`] and the loader's `OnLoad`: sets `this`, `event` and
/// `arg1..argN` and calls the handler with no arguments, as `0x705199` calls `lua_pcall` with
/// zero; the globals are restored after the call, even on error, so nested fires are safe.
pub(crate) fn invoke_with_globals(
    lua: &Lua,
    wrapper: Table,
    func: &Function,
    event_name: Option<&str>,
    extra: Vec<Value>,
) -> mlua::Result<()> {
    let g = lua.globals();

    let saved_this: Value = g.get("this")?;
    let saved_event: Value = g.get("event")?;
    let n = extra.len();
    let mut saved_args: Vec<Value> = Vec::with_capacity(n);
    for i in 1..=n {
        saved_args.push(g.get::<Value>(arg_name(i).as_ref())?);
    }

    g.set("this", wrapper.clone())?;
    if let Some(ev) = event_name {
        g.set("event", lua.create_string(ev)?)?;
    }
    for (i, v) in extra.iter().enumerate() {
        g.set(arg_name(i + 1).as_ref(), v.clone())?;
    }

    // A protected call; the globals are restored before its outcome returns.
    let outcome = func.call::<()>(());

    g.set("this", saved_this)?;
    g.set("event", saved_event)?;
    for (i, v) in saved_args.into_iter().enumerate() {
        g.set(arg_name(i + 1).as_ref(), v)?;
    }

    outcome
}
