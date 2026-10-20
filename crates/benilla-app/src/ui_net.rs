//! What `GetNetStats()` answers: the world link's KB/s in and out since it connected, and the
//! average `SMSG_PONG` round trip the net read thread records, the main bar's latency meter's one
//! input (`MainMenuBar.xml` polls it every 10 s).
//!
//! Pushed every frame, not on change: `UiScript` is created after a connection may already have
//! settled, so a change-gated feed could miss its only edge and leave the meter at 0.

use bevy::prelude::*;

use benilla_assets::LockRecover;

use crate::net::PingShared;
use crate::ui_script::UiFeed;

pub(crate) struct UiNetPlugin;

impl Plugin for UiNetPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, feed_net_stats.in_set(UiFeed));
    }
}

fn feed_net_stats(mut hosted_vm: crate::script_calls::HostedVm, ping: Res<PingShared>) {
    hosted_vm.run(|script| {
        let Some(script) = script else { return };
        // Recovered, not unwrapped: a panic on a net thread holding this lock ends the connection,
        // not the app.
        let (latency, rates) = {
            let clock = ping.0.lock_recover();
            let rates = clock
                .traffic
                .as_ref()
                .map_or((0.0, 0.0), |t| t.rates_kbps());
            (clock.avg_latency_ms(), rates)
        };
        script.set_latency_ms(latency);
        script.set_bandwidth_kbps(rates.0, rates.1);
    })
}
