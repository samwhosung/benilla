//! `GetNumWorldStateUI` and `GetWorldStateUIInfo`, behind the always-up PvP readout, over rows the
//! app resolves; the reference re-walks an array of DBC row ids (`0xb71e7c`) on demand. The info
//! is ten values (`0x4c5a70`), with no `uiType` or `hidden`, which came later.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One row of the readout: the ten values `GetWorldStateUIInfo` returns, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldStateUiView {
    /// `uiState`: the row's `StateVariable` world state, or 1 (on) when it has none (`0x4c5ad8`).
    pub ui_state: i32,
    /// The label, macro-expanded, the only expanded string of the ten.
    pub text: String,
    /// The static icon's texture path, or `""`.
    pub icon: String,
    /// The icon that replaces it while the state is live (Warsong Gulch's enemy flag), or `""`.
    pub dynamic_icon: String,
    pub tooltip: String,
    pub dynamic_tooltip: String,
    /// The extra widget the row drives, `"CAPTUREPOINT"` on the one row that has one, or `""`.
    pub extended_ui: String,
    /// That widget's three world states, resolved to values; the DBC holds ids.
    pub extended_ui_state: [i32; 3],
}

#[derive(Clone, Default)]
pub(crate) struct WorldStateUiState {
    pub(crate) rows: Vec<WorldStateUiView>,
}

impl super::UiScript {
    /// Push the readout's rows. The reference signals `UPDATE_WORLD_STATES` on its triggers (a
    /// packet, the defense flip: `0x48fa12`, `0x49bdd6`), not on a changed value, and the app's
    /// world-state lane does; this signals nothing.
    pub fn set_world_state_ui(&mut self, rows: Vec<WorldStateUiView>) {
        self.model_mut().worldstate.rows = rows;
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x4c5a40`: the count, no validation, always one return.
    g.set(
        "GetNumWorldStateUI",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.worldstate.rows.len() as i64)
        })?,
    )?;

    // A non-number raises (the `luaL_error` longjmps), an out-of-range index answers the single
    // number 0 (`0x4c5be5`), and no string is ever nil: an empty DBC column reads `""`.
    g.set(
        "GetWorldStateUIInfo",
        lua.create_function(|lua, index: Value| {
            // `lua_isnumber` (`0x6f34d0`), so a numeric string too; `_ftol` (`0x4c5aad`), then
            // `dec` and an unsigned compare with the row count (`0x4c5ab8`, `0x4c5abb`).
            let index =
                super::binding_abi::number_arg(lua, index, "Usage: GetWorldStateUIInfo(index)")?;
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let i = (index as u32).wrapping_sub(1) as usize;
                model.worldstate.rows.get(i).cloned()
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
