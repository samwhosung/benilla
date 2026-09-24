//! Handler firing — the FrameScript calling convention, for events, ticks, and
//! show/hide transitions.
//!
//! Every handler is invoked through the *same* protected path, which reproduces both calling
//! conventions the transition-era client supported:
//!
//! - **Legacy globals (byte-verified):** `this` (`0x872e64`) = the firing frame's wrapper, `event`
//!   (`0x84b648`) = the event name (OnEvent only), `arg1..argN` (`0x8722dc`) = the args — each
//!   **set then restored** around the call (`luaL_ref` saves the prior value; restored after), so
//!   nested fires are safe. The handler reads its inputs as globals; the client `pcall`s with
//!   **nargs = 0**.
//! - **Modern args:** the same values are *also* passed positionally as `(self, event, ...)` for
//!   OnEvent (`(self, elapsed)` for OnUpdate, `(self)` for OnShow/OnHide/OnLoad) — the form Era
//!   addons are written against.
//!
//! So inside one OnEvent handler `this == self` and `arg1` is the first positional argument (5.0's
//! own `arg[1]`; `...` as a value, and `select` with it, are not in this dialect). Handler errors are
//! caught (mlua's `Function::call` is a protected call) and returned to the caller, which records
//! them in [`super::Model::errors`] — never a panic, never a print.

use std::borrow::Cow;

use mlua::{Function, Lua, MultiValue, Table, Value};

use super::{Model, ScriptValue, REG_SCRIPTS};
use crate::script::object::frame_wrapper;
use crate::widget::{ButtonState, FrameHandle};

/// Fire `event` at every frame registered for it (the engine-internal twin of
/// `UiScript::fire_event`, for engine code holding only the Lua context — the compare drive's
/// `SHOW_COMPARE_TOOLTIP`), in registration order. **The same walk, not a second one**: this was
/// an index walk over the live list, so a handler that unregistered *itself* (the hook-once idiom
/// on `ADDON_LOADED`) shifted its successor into the slot the walk had just left, and that
/// listener never heard the event — the 1324 bug, fixed in [`super::tick::fire_event_into`] and
/// left standing here.
pub(super) fn fire_global(lua: &Lua, event: &str, args: &[ScriptValue]) {
    super::tick::fire_event_into(lua, event, args.to_vec());
}

/// The `RegisterAllEvents()` half of one dispatch: every frame that asked for the whole stream,
/// after the event's own listeners and skipping any frame that was already visited as one of them.
///
/// **After, and de-duplicated, because that is where such a frame would sit if the registration
/// were expanded.** A frame joins the all-events set later than the frames already listening for a
/// given event, so it takes the tail of that event's list; and a frame holding both an all-events
/// registration and a `RegisterEvent` for *this* event is one listener, not two — `RegisterEvent`'s
/// own `if !list.contains(&h)` is the same rule one level down.
///
/// Same mid-dispatch discipline as the walk above (`0x703ee8`): the next frame is
/// re-found by position each step, so a handler that unregisters the walk's successor stops the
/// dispatch there rather than robbing it.
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

/// Fire a frame's `OnUpdate` (`this` + `arg1 = elapsed`; modern `(self, elapsed)`).
pub(super) fn fire_update_handler(lua: &Lua, id: u32, elapsed: f32) -> mlua::Result<()> {
    fire(
        lua,
        id,
        "OnUpdate",
        None,
        vec![Value::Number(f64::from(elapsed))],
    )
}

/// Fire a plain widget handler — one that carries no `event` name: the mouse set
/// (`OnEnter`/`OnLeave(self, motion)`, `OnMouseDown`/`OnMouseUp(self, button)`,
/// `OnClick(self, button, down)`, `OnMouseWheel(self, delta)`) and the per-kind slots
/// (`OnValueChanged(self, value)`). `extra` is the handler's arguments (the `arg1..argN` legacy
/// globals plus the trailing modern positionals after `self`). Reuses the one [`fire`] path (so the
/// wrapper lookup + set/restore convention stay in a single home); a no-op if no such script is set.
pub(super) fn fire_widget_handler(
    lua: &Lua,
    id: u32,
    script: &str,
    extra: Vec<Value>,
) -> mlua::Result<()> {
    fire(lua, id, script, None, extra)
}

/// Drain [`Model::pending_size_changed`] and fire `OnSizeChanged(self, width, height)` for each.
///
/// The resolve pass ([`super::UiScript::resolve_layout`]) only ever holds a `&mut Model` — three of
/// its callers reach it that way — so it *queues* the frames whose resolved size moved and this
/// runs at the next `&Lua` seam. Errors are recorded, never propagated: a handler blowing up must
/// not abort a layout pass.
///
/// **Drains exactly once, deliberately.** A handler is free to resize things, `Show()` something,
/// or call `UpdateScrollChildRect` — each of which can re-enter the resolver and queue *more*
/// entries. Looping until the queue empties would let a handler that grows its own frame spin the
/// engine forever inside one call; taking one batch instead leaves the next batch for the next
/// resolve, which is exactly the reference's shape (its `ApplyRect` fires, the handler dirties the
/// layout, and the *next* layout pass fires again). A genuinely oscillating handler oscillates at
/// one fire per frame, visibly, instead of hanging the client.
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

/// Fire `OnShow`/`OnHide` for a set of frames whose effective visibility just changed (from
/// [`crate::widget::WidgetArena::set_shown`]/`set_parent`'s changed-list). Errors are recorded in
/// [`Model::errors`] rather than propagated — a handler error must not abort the `Show()` call.
///
/// **This is also where the `toplevel` raise fires** (the Show trigger `0x76ae10` @`0x76aee0`):
/// the binary tests the toplevel bit and raises *after* the subtree's visibility has propagated
/// and *before* that node's OnShow notify — which is exactly this seam, since the arena has
/// finished propagating by the time it hands back the changed list.
/// Per node and in list order, so a handler reading `GetFrameLevel()` sees the raised value the way
/// it would in the reference. It lives here rather than at the Lua `Show` binding because *every*
/// visibility transition this engine performs runs `0x76ae10` — an arena-level show from the
/// tooltip path (or a future host one) must raise too, and a second seam is how that gets forgotten.
pub(super) fn fire_visibility_changes(lua: &Lua, changed: Vec<FrameHandle>) {
    // Resolve (handle, id, now-visible?) under one short borrow, then fire with no borrow held.
    let items: Vec<(FrameHandle, u32, bool)> = {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        changed
            .into_iter()
            .filter_map(|h| {
                let vis = model.arena.frame(h)?.effective_visible;
                Some((h, model.frame_id(h), vis))
            })
            .collect()
    };
    // **The hover-hide law**: hiding the hovered frame — directly or through an ancestor's
    // cascade — fires its `OnLeave` SYNCHRONOUSLY, inside the hide and **before that frame's
    // `OnHide`** (`0x764ba0`'s kind-2 tail runs mid-`0x76ad50`, the leave at `0x764cce`; the
    // only silent case is destruction). It clears the hover cache and the drag-arm on that frame
    // (`+0x100/+0x104`, the arg-1 flavor), and schedules the re-pick — the pump re-hovers
    // whatever is now topmost at the unchanged cursor next tick ([`Model::hover_repick`]).
    // A SHOW arms the re-pick too (`0x764b8d`: any mouse-bucket insert): a window opening under
    // a stationary cursor gets hovered without a mouse move. This is what makes a slot button's
    // FrameXML `OnLeave → GameTooltip:Hide()` actually run when its window closes under the
    // cursor — the reference has NO engine-side owner-visibility fallback for the tooltip
    // (byte-censused negative), so the leave firing here is the whole mechanism.
    let left: Option<(FrameHandle, u32)> = {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        let hidden_hover = model
            .mouseover
            .filter(|&m| items.iter().any(|&(h, _, vis)| h == m && !vis));
        // The removal tail is a VIRTUAL dispatch — `0x764cce mov edx,[edi]; push 1;
        // call [edx+0x50]` — so a Button reaches its own `0x7794e0`, whose DISABLED guard skips
        // the base leave and with it the `<OnLeave>` script. Hiding a disabled hovered button is
        // therefore as silent as walking off one ([`super::button::hover_notify_runs`]); the
        // hover cache and the drag-arm still clear, because those are the *caller's* half.
        let notified = super::button::hover_notify_runs(&model, hidden_hover);
        if let Some(m) = hidden_hover {
            model.mouseover = None;
            if model.drag.as_ref().is_some_and(|d| d.source == m) {
                model.drag = None;
            }
        }
        model.hover_repick |= hidden_hover.is_some() || items.iter().any(|&(_, _, vis)| vis);
        hidden_hover
            .filter(|_| notified)
            .map(|m| (m, model.frame_id(m)))
    };
    if let Some((_, oid)) = left {
        if let Err(e) = fire_widget_handler(lua, oid, "OnLeave", vec![Value::Boolean(true)]) {
            lua.app_data_mut::<Model>()
                .expect("model")
                .record_script_error(e.to_string());
        }
    }
    for (h, id, visible) in items {
        if visible {
            let mut model = lua.app_data_mut::<Model>().expect("model");
            super::object::toplevel::raise_on_show(&mut model, h);
        } else {
            // **The button's HIDE edge** — `CSimpleButton` overrides the hide notify (`+0x34`,
            // `0x7791e0`) to un-press itself before tail-jumping the base, so a button hidden
            // while held does not come back up wearing its pushed art (the guard is
            // `state != DISABLED && locked == 0`).
            //
            // It hangs off the VISIBILITY transition, not off the hover, which is what it is in
            // the reference — a button hidden nowhere near the cursor un-presses too, and one
            // hidden under the cursor no longer needs the hover-drop above to notice.
            let mut model = lua.app_data_mut::<Model>().expect("model");
            super::button::edge(&mut model, h, ButtonState::on_hide);
        }
        let name = if visible { "OnShow" } else { "OnHide" };
        if let Err(e) = fire(lua, id, name, None, Vec::new()) {
            lua.app_data_mut::<Model>()
                .expect("model")
                .record_script_error(e.to_string());
        }
        // **And the EditBox's own OnShow/OnHide vtable overrides** (`0x81c910` +0x30/+0x34): an
        // `autoFocus` box grabs the keyboard when it appears if nothing else holds it, and hiding
        // the box that holds it releases it. Both overrides call the base notify FIRST and act
        // after, which is why this sits below the `fire` and not above it. A no-op for every frame
        // that is not an EditBox. Here rather than at the Lua `Show`/`Hide` bindings for the same
        // reason the toplevel raise is: every visibility transition this engine performs runs
        // through this seam, and a second one is how a path gets forgotten.
        super::editbox::visibility_focus(lua, h, visible);
    }
}

/// Is a handler bound under `script` for this frame id? — the **existence** question, asked
/// without firing anything.
///
/// It exists for the keyboard walk ([`super::keyboard`]), where the reference's consumption gate is
/// the presence of the slot rather than anything the handler does: `0x76b7d0` reads `[+0x188]`/
/// `[+0x190]`, consumes on either, and fires only the first. A lookup failure reads as "absent" —
/// the gate must never be able to raise out of the middle of a walk.
pub(super) fn has_widget_handler(lua: &Lua, id: u32, script: &str) -> bool {
    let Ok(scripts) = lua.named_registry_value::<Table>(REG_SCRIPTS) else {
        return false;
    };
    match scripts.get::<Value>(id) {
        Ok(Value::Table(per)) => matches!(per.get::<Value>(script), Ok(Value::Function(_))),
        _ => false,
    }
}

/// The one firing path. `event_name` is the `event` global + first modern positional (OnEvent only);
/// `extra` are the `arg1..argN` globals + the trailing modern positionals. Globals are saved before
/// and restored after (even on error) — nesting-safe. Holds no model borrow across the call.
fn fire(
    lua: &Lua,
    id: u32,
    script: &str,
    event_name: Option<&str>,
    extra: Vec<Value>,
) -> mlua::Result<()> {
    // Look up the handler (Lua-side, transient handle). Absent ⇒ nothing to do.
    let scripts: Table = lua.named_registry_value(REG_SCRIPTS)?;
    let func: Function = match scripts.get::<Value>(id)? {
        Value::Table(per) => match per.get::<Value>(script)? {
            Value::Function(f) => f,
            _ => return Ok(()),
        },
        _ => return Ok(()),
    };

    let wrapper = frame_wrapper(lua, id)?;
    // The attribution seam. Tight around the call and *after* `frame_wrapper` —
    // which documents that callers hold no model borrow here, so the profiler's own borrow cannot
    // land in the middle of one. Off, it is a relaxed load and a not-taken branch; the guard closes
    // the fire on the way out of this scope, including an unwind.
    let _fire =
        super::handler_prof::armed().then(|| super::handler_prof::Fire::open(lua, id, script));
    invoke_with_globals(lua, wrapper, &func, event_name, extra)
}

/// The legacy `argN` global names, pre-spelled through the arities that occur in practice so the
/// per-arg save/set/restore below allocates nothing.
const ARG_NAMES: [&str; 16] = [
    "arg1", "arg2", "arg3", "arg4", "arg5", "arg6", "arg7", "arg8", "arg9", "arg10", "arg11",
    "arg12", "arg13", "arg14", "arg15", "arg16",
];

/// The global name for arg `i` — **1-based**, matching the globals themselves (`arg1..`). Past
/// [`ARG_NAMES`] it falls back to allocating.
fn arg_name(i: usize) -> Cow<'static, str> {
    match ARG_NAMES.get(i - 1) {
        Some(&name) => Cow::Borrowed(name),
        None => Cow::Owned(format!("arg{i}")),
    }
}

/// The calling convention itself — the single home for it, shared by [`fire`] (registry
/// handlers: events, OnUpdate, OnShow/OnHide) and the loader's bottom-up `OnLoad` (which holds the
/// compiled `Function` directly). Sets the legacy `this`/`event`/`arg1..argN` globals and passes the
/// modern `(self[, event], extra…)` positionals, saving and restoring the globals around the call
/// (even on error) so nested handler firing is safe.
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

    let mut modern: Vec<Value> = Vec::with_capacity(2 + n);
    modern.push(Value::Table(wrapper));
    if let Some(ev) = event_name {
        modern.push(Value::String(lua.create_string(ev)?));
    }
    modern.extend(extra.iter().cloned());

    // Protected call. Capture the outcome, but restore globals first regardless.
    let outcome = func.call::<()>(MultiValue::from_vec(modern));

    g.set("this", saved_this)?;
    g.set("event", saved_event)?;
    for (i, v) in saved_args.into_iter().enumerate() {
        g.set(arg_name(i + 1).as_ref(), v)?;
    }

    outcome
}
