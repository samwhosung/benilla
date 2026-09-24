//! Lua globals whose whole body pushes one catalog id at `CGGameUI::DisplayError`'s dispatcher.
//! Engine-side refusals share the [`Model::ui_errors`] queue, which the app resolves against
//! GlobalStrings and fires as `UI_ERROR_MESSAGE` (event `0xe0`), the reference's route.
//!
//! `NotWhileDeadError` (`0x48d340`, registered at `0x83e398`) is `push 0x7e; call 0x496720`: no
//! argument, no dead check, no return, and a silent catalog row (`0xb4be70`). FrameXML decides
//! when to call it (`UIParent.lua:663-666`, `ContainerFrame.lua:147` and `:190`).

use mlua::Lua;

use super::model::Model;

/// Catalog id `0x7e`'s GlobalStrings key ("You can't do that when you're dead.").
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
