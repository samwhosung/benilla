//! `ConfirmBinder` and `CheckBinderDist`, the engine globals the `CONFIRM_BINDER` dialog calls
//! (`StaticPopup.lua:1321-1335`): Accept binds, and the dialog hides once the distance check
//! fails. The question arrives as the event's argument, so there is nothing to read back.

use mlua::Lua;

use super::Model;

impl super::UiScript {
    /// `ConfirmBinder()` calls since the last drain, each a `CMSG_BINDER_ACTIVATE`; a count, since
    /// the app holds the innkeeper's guid.
    pub fn take_binder_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().binder_confirms)
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
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.binder_confirms += 1;
            Ok(())
        })?,
    )?;

    // `CheckBinderDist()`, polled by the dialog's OnUpdate. This answers a boolean; the
    // reference (`0x48d210`) answers 1 or nil.
    g.set(
        "CheckBinderDist",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.binder_pending)
        })?,
    )?;

    Ok(())
}
