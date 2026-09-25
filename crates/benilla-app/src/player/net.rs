//! The player's packet handlers: `SMSG_STANDSTATE_UPDATE` goes to [`super::posture`] as a
//! [`ServerStandState`], applied in `control`, which holds the body query and sheath queue.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::ServerStandState;
use crate::net::NetHandlerApp;

/// Registers the handlers; called from [`super::PlayerPlugin`].
pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::StandStateUpdate, on_stand_state_update);
}

fn on_stand_state_update(In(ev): In<SessionEvent>, mut out: MessageWriter<ServerStandState>) {
    if let SessionEvent::StandStateUpdate { state } = ev {
        out.write(ServerStandState { state });
    }
}
