//! `ConfirmBinder` and `CheckBinderDist`, the engine globals the `CONFIRM_BINDER` dialog calls
//! (`StaticPopup.lua:1321-1335`): Accept binds, and the dialog hides once the distance check
//! fails. The question arrives as the event's argument, so there is nothing to read back.

use mlua::Lua;

use super::binding_abi::flag;
use super::Model;

/// The binder's verb the host applies inside its call ([`super::ScriptCall::Binder`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinderCall {
    /// `ConfirmBinder()` (`0x48dd00` → `0x5dfdc0(0)`): `CMSG_BINDER_ACTIVATE` with the remembered
    /// innkeeper, nothing with none.
    Confirm,
}

impl super::UiScript {
    /// The `ConfirmBinder()` calls a harness's VM kept ([`Self::keep_calls`]).
    pub fn take_binder_confirms(&mut self) -> u32 {
        self.take_calls_like(&super::ScriptCall::Binder(BinderCall::Confirm)) as u32
    }

    /// Push whether a bind question is pending with the innkeeper still in range.
    pub fn set_binder_pending(&mut self, pending: bool) {
        let mut model = self.model_mut();
        if model.binder_pending != pending {
            model.binder_pending = pending;
        }
    }
}

/// Register the two binder globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `ConfirmBinder()`: the dialog's Accept, the one call that binds a hearthstone.
    g.set(
        "ConfirmBinder",
        lua.create_function(|lua, ()| {
            super::calls::submit(lua, super::ScriptCall::Binder(BinderCall::Confirm))
        })?,
    )?;

    // `CheckBinderDist()`, polled by the dialog's OnUpdate: 1 or nil (`0x48d210`).
    g.set(
        "CheckBinderDist",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.binder_pending))
        })?,
    )?;

    Ok(())
}
