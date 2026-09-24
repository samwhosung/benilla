//! The innkeeper-bind **Era API surface** — two globals, no snapshot.
//!
//! These are the whole Lua side of setting a hearthstone, and they exist because the reference's
//! `CONFIRM_BINDER` dialog is written against exactly them (`StaticPopup.lua:1308-1322`):
//! `OnAccept` calls [`ConfirmBinder`], and `OnUpdate` hides the dialog the moment
//! [`CheckBinderDist`] goes false. Both are engine bindings in 1.12 (`reference/1.12-globals.tsv`
//! lists `ConfirmBinder` and `CheckBinderDist` as `function`/`engine`), and there are no others in
//! the family — the question itself arrives as the `CONFIRM_BINDER` event's argument, so there is
//! nothing to read back, exactly like [`super::duel`].
//!
//! The app owns the innkeeper's guid ([`crate::script::UiScript::set_binder_pending`]'s writer),
//! which is why `ConfirmBinder()` takes no arguments and queues only a count.

use mlua::Lua;

use super::Model;

impl super::UiScript {
    /// Drain the `ConfirmBinder()` calls queued since the last drain — each one is a
    /// `CMSG_BINDER_ACTIVATE`. [`super::UiScript::take_played_time_asks`]'s shape, and a count for
    /// the same reason: the intent has no payload of its own.
    pub fn take_binder_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().binder_confirms)
    }

    /// Push whether an innkeeper's bind question is live and still in range — the host's half of
    /// `CheckBinderDist()`. Idempotent; `false` covers both "no question pending" and "you walked
    /// away", which is all the dialog's OnUpdate needs to decide to hide.
    pub fn set_binder_pending(&mut self, pending: bool) {
        let mut model = self.model_mut();
        if model.binder_pending != pending {
            model.binder_pending = pending;
        }
    }
}

/// Register the two binder globals (the style [`super::duel`] registers its four).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ConfirmBinder() — the CONFIRM_BINDER dialog's Accept. The one call in the client that binds
    // a hearthstone: everything before it is a question.
    g.set(
        "ConfirmBinder",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.binder_confirms += 1;
            Ok(())
        })?,
    )?;

    // CheckBinderDist() — polled from the dialog's OnUpdate; false hides it.
    g.set(
        "CheckBinderDist",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.binder_pending)
        })?,
    )?;

    Ok(())
}
