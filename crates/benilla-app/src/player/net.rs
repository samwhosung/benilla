//! The player's packet handlers: `SMSG_STANDSTATE_UPDATE` applies in its packet
//! ([`super::posture::server_stand_state`]); `SMSG_PLAYER_SKINNED`, among the player object's
//! messages in the reference (`0x5e3010`), signals `PLAYER_SKINNED` at its packet, on the notice
//! lane.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::net::NetHandlerApp;

/// Registers the handlers; called from [`super::PlayerPlugin`].
pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::StandStateUpdate, on_stand_state_update)
        .net_handler(SessionEventKind::PlayerSkinned, on_player_skinned);
}

/// `SMSG_STANDSTATE_UPDATE` (`0x603e50`), applied to the active player in its packet
/// ([`super::posture::server_stand_state`]).
fn on_stand_state_update(
    In(ev): In<SessionEvent>,
    mut pending: ResMut<super::StandPending>,
    me: super::SelfPosture,
    player: Option<Res<super::Player>>,
    mut commands: Commands,
    mut sheath: MessageWriter<crate::creature_anim::SheathRequest>,
    mut loot_close: crate::ui_loot::MoveStartClose,
) {
    if let SessionEvent::StandStateUpdate { state } = ev {
        super::posture::server_stand_state(
            state,
            &mut pending,
            &me,
            player.as_deref().map_or(0, super::Player::move_flags),
            &mut commands,
            &mut sheath,
            &mut loot_close,
        );
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
