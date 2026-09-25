//! The melee live probe (`WOW_PROBE=melee`), inert without the env: once in-world it fires the
//! attack-nearest core ([`AttackNearestRequest`]) every few seconds, so the character fights
//! whatever is closest while `WOW_MOVE_TRACE=<path>` records the swing, impact and combat-text
//! timeline.
//!
//! Never run it unattended (`docs/METHOD.md`, "The local server"): it fights with no health
//! awareness and the character dies.

use bevy::prelude::*;

use super::probes::ProbeClock;
use crate::net::SelfPlayer;
use crate::target::AttackNearestRequest;

pub(crate) struct ProbeMeleePlugin;

impl Plugin for ProbeMeleePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, melee_probe);
    }
}

/// Every 3 s: swing at the selection (idempotent while auto-attacking, and it covers an attacker
/// the acquire core skips), or with none, run the acquire core.
fn melee_probe(
    time: ProbeClock,
    mut last: Local<f64>,
    self_player: Query<(), With<SelfPlayer>>,
    selection: Res<crate::target::Selection>,
    net: Res<crate::net::NetCommands>,
    mut requests: MessageWriter<AttackNearestRequest>,
) {
    if self_player.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    if now - *last >= 3.0 {
        *last = now;
        if let Some(guid) = selection.guid {
            info!("melee probe: swinging at {guid:#x}");
            let _ = net.0.send(crate::net::ClientCommand::AttackSwing { guid });
        } else {
            info!("melee probe: acquiring nearest");
            requests.write(AttackNearestRequest);
        }
    }
}
