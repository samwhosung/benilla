//! The player's packet handlers (the net handler table) — today one: the server's
//! own stand state for our body (`SMSG_STANDSTATE_UPDATE`), handed to
//! [`super::posture`] as a [`ServerStandState`] message because the apply wants the frame's body
//! query and sheath queue, which live in `control`.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::ServerStandState;
use crate::net::NetHandlerApp;

/// Register the handler — called from [`super::PlayerPlugin`].
pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::StandStateUpdate, on_stand_state_update);
}

fn on_stand_state_update(In(ev): In<SessionEvent>, mut out: MessageWriter<ServerStandState>) {
    if let SessionEvent::StandStateUpdate { state } = ev {
        out.write(ServerStandState { state });
    }
}
