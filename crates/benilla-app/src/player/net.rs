//! The player's packet handlers: `SMSG_STANDSTATE_UPDATE` goes to [`super::posture`] as a
//! [`ServerStandState`], applied in `control`, which holds the body query and sheath queue;
//! `SMSG_PLAYER_SKINNED`, among the player object's messages in the reference (`0x5e3010`),
//! signals `PLAYER_SKINNED` at its packet, on the notice lane.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::ServerStandState;
use crate::net::NetHandlerApp;

/// Registers the handlers; called from [`super::PlayerPlugin`].
pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::StandStateUpdate, on_stand_state_update)
        .net_handler(SessionEventKind::PlayerSkinned, on_player_skinned);
}

fn on_stand_state_update(In(ev): In<SessionEvent>, mut out: MessageWriter<ServerStandState>) {
    if let SessionEvent::StandStateUpdate { state } = ev {
        out.write(ServerStandState { state });
    }
}

/// `SMSG_PLAYER_SKINNED` (`0x5e7d40`): `PLAYER_SKINNED` with the byte's truth as `arg1`
/// (`0x5e7d54`, `0x48f407`), at the packet's place among the lanes. Absent in a harness without the
/// notice lane.
fn on_player_skinned(
    In(ev): In<SessionEvent>,
    lane: Option<ResMut<crate::field_watch::NoticeLane>>,
) {
    if let (SessionEvent::PlayerSkinned { free_repop }, Some(mut lane)) = (ev, lane) {
        lane.write(crate::field_watch::FieldNotice::Skinned { free_repop });
    }
}
