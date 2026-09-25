//! The partner live probe (`WOW_PROBE=partner`) — the party arc's instrument:
//! a second client that says YES. Once in-world it auto-accepts every group invite, so the
//! director can exercise the whole party surface — invite, roster lines, member frames,
//! leader/loot management, pings — solo, with the probe account as the other member. It also
//! accepts **duel** challenges, which is what makes the duel arc's two-client
//! questions answerable at all: whether the opponent turns hostile, whether the arbiter flag
//! plants. Non-combat (the unattended-combat ban, `docs/METHOD.md` "The local vmangos server", stands
//! untouched: this probe answers a challenge but never swings, and a duel nobody strikes in
//! simply times out). Pair with the checkout's probe identity (the `probe` skill).

use bevy::prelude::*;

use crate::net::SelfPlayer;
use crate::ui_party::GroupState;

pub(crate) struct ProbePartnerPlugin;

impl Plugin for ProbePartnerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, partner_probe);
    }
}

/// A pending invite is answered with `CMSG_GROUP_ACCEPT` the frame it lands. Clearing
/// `pending_invite` here keeps the probe's own PARTY_INVITE popup from ever arming (and if the
/// popup won a same-frame race, its hide-path `DeclineGroup` is a server no-op once we're
/// grouped — vmangos ignores a decline with no pending invite).
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
    // A pending challenge is answered with `CMSG_DUEL_ACCEPTED` the frame it lands. Taking the
    // challenger discharges the popup debt the same way the UI feed's DUEL_REQUESTED edge would,
    // so no dialog is left owed on the probe.
    let arbiter = duel.arbiter;
    if let Some(challenger) = duel.take_challenger() {
        info!("partner probe: accepting duel from {challenger:#018x} (arbiter {arbiter:#018x})");
        let _ = net
            .0
            .send(crate::net::ClientCommand::DuelAccepted { arbiter });
    }
}
