//! The partner live probe (`WOW_PROBE=partner`): a second client that accepts every group
//! invite and duel challenge, so the party and duel surfaces can be exercised with it as the
//! other player. It never swings, so an unstruck duel times out; the switches are
//! `docs/CONTRIBUTING.md`, "Running it unattended".

use bevy::prelude::*;

use crate::net::SelfPlayer;
use crate::ui_party::GroupState;

pub(crate) struct ProbePartnerPlugin;

impl Plugin for ProbePartnerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, partner_probe);
    }
}

/// Accept a pending invite the frame it lands; taking `pending_invite` keeps the invite popup
/// from arming, and a racing `DeclineGroup` is a no-op once grouped (`GroupHandler.cpp:200`).
fn partner_probe(
    self_player: Query<(), With<SelfPlayer>>,
    mut group: ResMut<GroupState>,
    mut duel: ResMut<crate::ui_duel::DuelState>,
    net: Res<crate::net::NetCommands>,
) {
    if self_player.is_empty() {
        return;
    }
    if let Some(inviter) = group.pending_invite.take() {
        info!("partner probe: accepting {inviter}'s group invite");
        let _ = net.0.send(crate::net::ClientCommand::GroupAccept);
    }
    // Taking the challenger discharges the DUEL_REQUESTED popup, as the UI feed would.
    let arbiter = duel.arbiter;
    if let Some(challenger) = duel.take_challenger() {
        info!("partner probe: accepting duel from {challenger:#018x} (arbiter {arbiter:#018x})");
        let _ = net
            .0
            .send(crate::net::ClientCommand::DuelAccepted { arbiter });
    }
}
