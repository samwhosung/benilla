//! **The Lua-callable half of the client-local error-display family** — FrameXML-facing globals
//! whose whole body, in the reference binary, is "push one catalog id at
//! `CGGameUI::DisplayError`'s dispatcher". The engine-side half (a Rust system raising a key
//! directly, e.g. the passive-on-bar refusal) pushes onto the same [`Model::ui_errors`] queue;
//! the app drains it, resolves each key against the VM's own GlobalStrings and fires
//! `UI_ERROR_MESSAGE` (`ui_action/feed.rs`) — which is byte-for-byte the reference route: catalog
//! row → GlobalStrings text → FrameScript event `0xe0`, rendered by UIErrorsFrame.
//!
//! First tenant: `NotWhileDeadError` (decision 1507's open item): registration pair `0x83e398`,
//! C body `0x48d340` = `push 0x7e; call 0x496720; xor eax,eax; ret` — no argument read, no
//! dead-check of its own, 0 return values, and its catalog row (`0xb4be70`, key
//! `ERR_PLAYER_DEAD`) names sound `"NONE"`, so the toast is silent. FrameXML decides *when* to
//! call it (ShowUIPanel's `whileDead` refusal, UIParent.lua l.663-666; ContainerFrame.lua l.147
//! and l.190); the binding only displays.

use mlua::Lua;

use super::model::Model;

/// The GlobalStrings key of catalog id `0x7e` — the row `NotWhileDeadError`'s pushed id resolves
/// to ("You can't do that when you're dead.").
const NOT_WHILE_DEAD_KEY: &str = "ERR_PLAYER_DEAD";

/// Register the error-display globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "NotWhileDeadError",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.ui_errors.push(NOT_WHILE_DEAD_KEY);
            Ok(())
        })?,
    )
}
