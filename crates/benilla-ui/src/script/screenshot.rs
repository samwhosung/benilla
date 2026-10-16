//! `Screenshot()`, the engine verb behind the print-screen key, in both states. In-game the
//! `SCREENSHOT` binding calls `TakeScreenshot()` (`WorldFrame.lua:46`), which hides the status text
//! and calls this; the glue screens call it from their `OnKeyDown` on `PRINTSCREEN`
//! (`AccountLogin.lua:81`). It returns nothing: each state's binding (`0x46d3d0` in the glue table
//! `0x8373b8`) arms the capture with its own callback (`0x442a10`, the glue's slot), and the outcome
//! arrives later as an event, so the "Screen Captured" message never lands in the capture. The
//! callback `0x46c520` fires `GLUE_SCREENSHOT_SUCCEEDED` (id 14) for a non-zero argument, else
//! `GLUE_SCREENSHOT_FAILED` (15); in-game it is `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED`
//! ([`UiScript::screenshot_event`]).

use mlua::Lua;

use super::{Model, VmKind};

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

    /// The event that answers a capture in this state: the glue's callback `0x46c520` signals id
    /// 14 (`saved`) or 15, the in-game one `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED`. An
    /// in-game name sent to the glue VM would reach no frame (`VmKind::fires`).
    pub fn screenshot_event(&self, saved: bool) -> &'static str {
        match (self.vm_kind(), saved) {
            (VmKind::Glue, true) => "GLUE_SCREENSHOT_SUCCEEDED",
            (VmKind::Glue, false) => "GLUE_SCREENSHOT_FAILED",
            (VmKind::InGame, true) => "SCREENSHOT_SUCCEEDED",
            (VmKind::InGame, false) => "SCREENSHOT_FAILED",
        }
    }
}
