//! The connection-telemetry feed — the app side of `benilla_ui::script::net_stats`' seam.
//!
//! One number: the latency `GetNetStats()` reports, which is the main bar's performance ("ping")
//! meter's whole input (stock `MainMenuBar.xml`'s `MainMenuBarPerformanceBarFrame` polls it every 10 s and
//! colors the bar green/yellow/red). The measurement itself belongs to the **net read thread** —
//! every `SMSG_PONG` is timed where it lands and filed in the connection clock's RTT history at the
//! reference's own depth — and this feed only carries its average across the engine boundary (decision
//! 0068 §3: the engine never reaches into ECS or the socket). Reading it takes the connection's
//! lock, exactly as the reference's `GetNetStats` takes the stats critical section; it is
//! uncontended (one writer every 30 s) and this is three words of copy a frame.
//!
//! The push is unconditional rather than change-gated, for the reason [`crate::minimap`]'s
//! containment feed documents: `UiScript` is created when the UI loads, which is *after* a
//! connection can already have settled, so a feed that only spoke on change could miss its only
//! edge and leave the meter reading 0 forever. Three words of copy a frame is nothing next to that.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use benilla_assets::LockRecover;

use crate::net::PingShared;
use crate::ui_script::UiFeed;

pub(crate) struct UiNetPlugin;

impl Plugin for UiNetPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, feed_net_stats.in_set(UiFeed));
    }
}

/// Push the averaged round trip behind `GetNetStats()`.
fn feed_net_stats(script: Option<NonSendMut<UiScript>>, ping: Res<PingShared>) {
    let Some(mut script) = script else { return };
    // Recovered, not unwrapped: the net threads hold this lock too, and a panic there must end
    // the connection, not the app.
    let latency = ping.0.lock_recover().avg_latency_ms();
    script.set_latency_ms(latency);
}
