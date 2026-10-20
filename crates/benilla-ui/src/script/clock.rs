//! The two clocks Lua reads: `GetTime()`, the session seconds [`UiScript::tick`] advances, and
//! `GetGameTime()`, the game clock the host pushes. Both live in the [`Model`], out of an addon's
//! reach, as the reference's live in the client.
//!
//! [`UiScript::tick`]: super::UiScript::tick

use mlua::Lua;

use super::Model;

impl super::UiScript {
    /// The current `GetTime()` value in seconds; the app stamps absolute expiries with it, such as
    /// an aura's `expirationTime`.
    pub fn now(&self) -> f64 {
        self.model_ref().now
    }

    /// Start this VM's `GetTime()` clock at `secs`, so a rebuilt VM (a relog, a `ReloadUI`) keeps
    /// the process's clock. The reference's `GetTime` (`0x515ea0`, through `0x42c010` and
    /// `0x42b790`) is `GetTickCount` scaled by 0.001, an OS clock that never restarts, which stock
    /// `Cooldown.lua` relies on (`start > 0`). Set once, at construction; after that only
    /// [`Self::tick`] moves it.
    pub fn set_now(&mut self, secs: f64) {
        self.model_mut().now = secs;
    }

    /// Push the game clock `GetGameTime()` answers, `(hour, minute)`; -1, -1 until the first push.
    /// The clock holds either as the server sent it, so a field is -1 when unset and may run past
    /// a day's end (an hour to 30, a minute to 62).
    pub fn set_game_time(&mut self, hour: i32, minute: i32) {
        self.model_mut().game_time = Some((hour, minute));
    }
}

/// The start a cooldown binding answers: a held one (`enabled` false) reports the call's own now,
/// as the reference's held legs write the query time (`0x6e1517`, `0x6e15a1`), so the pushed
/// triple carries no moving start.
pub(crate) fn held_start(start: f64, enabled: bool, now: f64) -> f64 {
    if enabled {
        start
    } else {
        now
    }
}

/// `GetTime()`'s clock, for a caller holding no model borrow; one that holds it reads `model.now`.
pub(crate) fn now(lua: &Lua) -> f64 {
    lua.app_data_ref::<Model>().expect("model app_data").now
}

/// Register `GetTime` and `GetGameTime`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // GetTime() (`0x515ea0`): one number, `GetTickCount` times the f64 0.001 at `0x801608`.
    // Deviation: the reference reads the clock live at each call (`0x42c010`); ours is the session
    // clock the tick advances by the paced frame step, one value per frame, so `GetTime()` deltas
    // and the `elapsed` handed to OnUpdate agree, as FrameXML (CastingBarFrame and kin) assumes.
    g.set("GetTime", lua.create_function(|lua, ()| Ok(now(lua)))?)?;
    // GetGameTime() (`0x515ee0`): two numbers, the hour `[0xce853c]` then the minute `[0xce8538]`,
    // the clock the server's time messages set; minute resolution, no seconds. Read unguarded, so
    // before the server sets the clock it answers the -1 its constructor stores in every field
    // (`0x642300`, called at `0x643308`).
    g.set(
        "GetGameTime",
        lua.create_function(|lua, ()| {
            let clock = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .game_time;
            Ok(clock.map_or((-1.0, -1.0), |(hour, minute)| {
                (f64::from(hour), f64::from(minute))
            }))
        })?,
    )?;
    Ok(())
}
