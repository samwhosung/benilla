//! **`Screenshot()`** — the engine verb behind the print-screen key.
//!
//! One global and one counter. The reference's binding is `SCREENSHOT` → `TakeScreenshot()`
//! (FrameXML `Bindings.xml`), whose body hides the status text and then calls this engine
//! function; the engine answers later with `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED`, which
//! is what puts "Screen Captured" on screen. **The answer is an EVENT, never a return value** —
//! and that is the whole mechanism that keeps the message out of the file it announces: the
//! capture is already taken by the time anything can say so.
//!
//! A **count**, like [`super::client`]'s `RequestTimePlayed` and `ConfirmBinder`: the request
//! carries no payload (the app owns the window, the folder and the clock), so two calls in a frame
//! are two captures rather than one collapsed intent. The app drains it in
//! [`crate::script::UiScript::take_screenshot_asks`].

use mlua::Lua;

use super::Model;

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // `Screenshot()` — take one. Returns nothing: the reference's does not either, because the
    // capture completes after the frame that asked for it and the outcome arrives as
    // SCREENSHOT_SUCCEEDED / SCREENSHOT_FAILED.
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
    /// Drain the `Screenshot()` calls queued since the last call — each one is one capture.
    /// [`super::UiScript::take_played_time_asks`]'s shape, for the same reason.
    pub fn take_screenshot_asks(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().screenshot_asks)
    }
}
