//! `Screenshot()`, the engine verb behind the print-screen key: the `SCREENSHOT` binding calls
//! `TakeScreenshot()` (`WorldFrame.lua:46`), which hides the status text and calls this. It
//! returns nothing; the outcome arrives later as `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED`,
//! so the "Screen Captured" message never lands in the capture.

use mlua::Lua;

use super::Model;

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "Screenshot",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .screenshot_asks += 1;
            Ok(())
        })?,
    )?;
    Ok(())
}

impl super::UiScript {
    /// `Screenshot()` calls since the last drain, each one capture.
    pub fn take_screenshot_asks(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().screenshot_asks)
    }
}
