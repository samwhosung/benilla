//! `GetNetStats()` (`0x48b8b0`): `bandwidthIn, bandwidthOut, latency` for the main bar's latency
//! meter. The latency is the average over the connection's 16-slot RTT ring (`HandlePong`
//! `0x537d60`, `0x537f20`), which the app keeps and pushes here. Both bandwidths read 0: benilla
//! measures no throughput, and the one stock caller, `MainMenuBarPerformanceBarFrame`, drops them.

use mlua::Lua;

use super::Model;

impl super::UiScript {
    /// Push the average round trip in ms; `None`, before the first pong, reads 0, as the reference
    /// reports an unmeasured connection.
    pub fn set_latency_ms(&mut self, ms: Option<u32>) {
        self.model_mut().net_latency_ms = ms.unwrap_or(0);
    }
}

/// Register the net-stats globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "GetNetStats",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok((0.0_f64, 0.0_f64, i64::from(model.net_latency_ms)))
        })?,
    )
}
