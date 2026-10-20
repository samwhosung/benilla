//! `GetNetStats()` (`0x48b8b0`): `bandwidthIn, bandwidthOut, latency`, from `0x537f20`. The two
//! rates are the KB/s received and sent since the world link connected, the latency the average
//! of at most 15 round trips in the connection's ring (`HandlePong` `0x537d60`); the app keeps both
//! and pushes them here. The one stock caller, `MainMenuBarPerformanceBarFrame`, reads the latency
//! alone; bandwidth addons read the rates.

use mlua::Lua;

use super::Model;

impl super::UiScript {
    /// Push the average round trip in ms; `None`, before the first pong, reads 0, as the reference
    /// reports an unmeasured connection.
    pub fn set_latency_ms(&mut self, ms: Option<u32>) {
        self.model_mut().net_latency_ms = ms.unwrap_or(0);
    }

    /// Push the KB/s received and sent since the link connected; 0 before any link.
    pub fn set_bandwidth_kbps(&mut self, received: f32, sent: f32) {
        self.model_mut().net_bandwidth_kbps = (received, sent);
    }
}

/// Register the net-stats globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "GetNetStats",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (received, sent) = model.net_bandwidth_kbps;
            Ok((
                f64::from(received),
                f64::from(sent),
                i64::from(model.net_latency_ms),
            ))
        })?,
    )
}
