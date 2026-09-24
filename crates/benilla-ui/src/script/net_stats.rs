//! `GetNetStats()` — the connection-telemetry read the main bar's performance ("ping") meter polls.
//!
//! One global over one pushed number. The reference binding (`0x48b8b0`) returns `bandwidthIn,
//! bandwidthOut, latency`; its latency is **not** the last round trip but the average over the
//! connection's RTT ring — `HandlePong 0x537d60` puts the RTT into that ring (head/tail wrap 16),
//! feeding `0x537f20`'s throughput/avg-RTT math (host telemetry; no wire bytes). The app owns
//! that ring
//! ([`crate`]'s consumer keeps it beside its ping clock) and pushes the average here; the engine
//! stays free of ECS/net reach (decision 0068 §3), exactly as [`super::unit`]'s `GetMoney` does.
//!
//! **The two bandwidth returns are `0`.** benilla measures no throughput — its socket threads tally
//! no bytes — and nothing in the 1.12 UI reads them: `MainMenuBarPerformanceBarFrame` is the whole
//! shipped caller of `GetNetStats` and discards both. They stay in the signature (numbers, not
//! `nil`, so arithmetic on them can't error) as a named gap, not a claim.

use mlua::Lua;

use super::Model;

impl super::UiScript {
    /// Push the reported round trip (ms) — the app's average over its RTT ring. `None` = no sample
    /// yet (before the connection's first pong, or after a disconnect), which reads as `0` through
    /// `GetNetStats`: the reference reports 0 for an unmeasured connection too, and the bar's
    /// thresholds put 0 in the green band, which is what the real client shows at login.
    pub fn set_latency_ms(&mut self, ms: Option<u32>) {
        self.model_mut().net_latency_ms = ms.unwrap_or(0);
    }
}

/// Register the net-stats globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // GetNetStats() → bandwidthIn, bandwidthOut, latency (see the module doc for the two zeros).
    lua.globals().set(
        "GetNetStats",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok((0.0_f64, 0.0_f64, i64::from(model.net_latency_ms)))
        })?,
    )
}
