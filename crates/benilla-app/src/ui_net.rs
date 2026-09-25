//! The latency behind `GetNetStats()`, the main bar's latency meter's one input (`MainMenuBar.xml`
//! polls it every 10 s): the average `SMSG_PONG` round trip the net read thread records.
//!
//! Pushed every frame, not on change: `UiScript` is created after a connection may already have
//! settled, so a change-gated feed could miss its only edge and leave the meter at 0.

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

fn feed_net_stats(script: Option<NonSendMut<UiScript>>, ping: Res<PingShared>) {
    let Some(mut script) = script else { return };
    // Recovered, not unwrapped: a panic on a net thread holding this lock ends the connection,
    // not the app.
    let latency = ping.0.lock_recover().avg_latency_ms();
    script.set_latency_ms(latency);
}
