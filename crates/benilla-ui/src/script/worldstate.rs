//! The world-state UI seam — `GetNumWorldStateUI` / `GetWorldStateUIInfo`, the two bindings behind
//! the always-up PvP readout (`WorldStateFrame`).
//!
//! The list is built app-side ([`crate::world_state_ui`](benilla_app)) because every one of its
//! gates reads app state — the last init's `(map, area)` scope, the joined chat channels, the live
//! world-state table — and pushed here as already-resolved rows. That is the same split every
//! other feed uses; what the reference keeps in a process global (`0xb71e7c`, an array of DBC row
//! ids re-walked on demand) we keep as the resolved answer.
//!
//! **Return shape** at `0x4c5a70`.
//! Worth stating what it is *not*: this is a **ten**-value return, and it carries **no `uiType` and
//! no `hidden`** — those are a later expansion of the API, and a client that answered twelve values
//! here would hand `WorldStateFrame` its icon path where it expects a number.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One row of the always-up world-state readout — the ten values `GetWorldStateUIInfo` returns,
/// in its order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldStateUiView {
    /// `uiState` — the row's `StateVariable` world state resolved through the table, or **`1`**
    /// when it carries none (`0x4c5ad8`: the miss is a literal `1.0`, not `0`, so a row without a
    /// state reads as "on" rather than "off").
    pub ui_state: i32,
    /// The label, already macro-expanded (`"Towers Controlled: 3"`). The **only** expanded string
    /// of the ten.
    pub text: String,
    /// The static icon's texture path, or `""`.
    pub icon: String,
    /// The icon that replaces it while the row's state is live (Warsong Gulch's enemy flag), or
    /// `""`.
    pub dynamic_icon: String,
    pub tooltip: String,
    pub dynamic_tooltip: String,
    /// A token naming an extra widget the row drives — `"CAPTUREPOINT"` on the one row that has
    /// one — or `""`.
    pub extended_ui: String,
    /// That widget's three world states, **resolved to values** (the DBC holds ids; the binding
    /// answers what they read).
    pub extended_ui_state: [i32; 3],
}

#[derive(Default)]
pub(crate) struct WorldStateUiState {
    pub(crate) rows: Vec<WorldStateUiView>,
}

impl super::UiScript {
    /// Push the always-up world-state readout's rows (see
    /// [`WorldStateUiView`](WorldStateUiView)). On change, queues
    /// **`UPDATE_WORLD_STATES`** — event `0x20e`, the reference's own name for it.
    ///
    /// **Named divergence, in the *trigger*, not the shape.** The reference fires that event from
    /// exactly two sites (`0x48fa12`, after an init rebuilds the list; `0x49bdd6`, when the
    /// zone-defense channel membership flips) and, verified by a whole-`.text` scan for the id,
    /// nowhere else — so a plain `SMSG_UPDATE_WORLD_STATE` changing a tower count fires nothing.
    /// It can afford that because its Lua re-reads live values off the table at whatever moment it
    /// repaints; we push resolved values, so a silent value change would simply never reach the
    /// frame. We therefore fire on any change to what the readout would *display*, which is the
    /// reference's two triggers plus the value updates its own frame must already be reacting to
    /// by some means the binary does not show.
    pub fn set_world_state_ui(&mut self, rows: Vec<WorldStateUiView>) {
        let mut model = self.model_mut();
        if model.worldstate.rows != rows {
            model.worldstate.rows = rows;
            model
                .pending_events
                .push(("UPDATE_WORLD_STATES".to_string(), Vec::new()));
        }
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumWorldStateUI() — how many always-up rows the current scope admits (`0x4c5a40`: pushes
    // the count, no validation, always one return).
    g.set(
        "GetNumWorldStateUI",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.worldstate.rows.len() as i64)
        })?,
    )?;

    // GetWorldStateUIInfo(index) — the i-th (1-based) row, ten values (`0x4c5a70`).
    //
    // Two legs of the reference's behaviour are easy to get wrong and are reproduced here:
    //
    // - a **non-number** argument RAISES `"Usage: GetWorldStateUIInfo(index)"`. The binding's
    //   `luaL_error` leg looks like it returns — its callee has a `ret` — but that `ret` is an
    //   MSVC epilogue, and closing the range two levels down (`luaG_errormsg`, then `luaD_throw`,
    //   neither of which contains a single `ret`) proves the longjmp. So this is an error, not a
    //   quiet nil.
    // - an **out-of-range** index returns exactly ONE value, the number `0` — not nil, and not a
    //   run of ten nils (`0x4c5be5`). The index falls through a row-id substitution of `0`, whose
    //   id-index slot is NULL, and lands on that shared bail.
    //
    // No string return is ever nil: an empty DBC column resolves to the string block's leading
    // NUL, so Lua gets `""`. The `__ftol` on the index truncates toward zero.
    g.set(
        "GetWorldStateUIInfo",
        lua.create_function(|lua, index: Value| {
            let index = match index {
                Value::Integer(i) => i as f64,
                Value::Number(n) => n,
                // The reference accepts a numeric STRING too — `lua_isnumber` coerces.
                Value::String(ref s) => match s.to_str().ok().and_then(|s| s.trim().parse().ok()) {
                    Some(n) => n,
                    None => return Err(mlua::Error::runtime("Usage: GetWorldStateUIInfo(index)")),
                },
                _ => return Err(mlua::Error::runtime("Usage: GetWorldStateUIInfo(index)")),
            };
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                // Truncate toward zero, then 1-based.
                usize::try_from(index.trunc() as i64)
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| model.worldstate.rows.get(i).cloned())
            };
            let Some(r) = row else {
                return Ok(MultiValue::from_vec(vec![Value::Integer(0)]));
            };
            let s = |v: &str| -> mlua::Result<Value> { Ok(Value::String(lua.create_string(v)?)) };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(r.ui_state)),
                s(&r.text)?,
                s(&r.icon)?,
                s(&r.dynamic_icon)?,
                s(&r.tooltip)?,
                s(&r.dynamic_tooltip)?,
                s(&r.extended_ui)?,
                Value::Integer(i64::from(r.extended_ui_state[0])),
                Value::Integer(i64::from(r.extended_ui_state[1])),
                Value::Integer(i64::from(r.extended_ui_state[2])),
            ]))
        })?,
    )?;

    Ok(())
}
