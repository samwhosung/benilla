//! The mount and dismount result packets, onto the red error line.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::MountErrors;
use crate::net::NetHandlerApp;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::MountResult, on_mount_result);
}

fn on_mount_result(In(ev): In<SessionEvent>, mut errors: ResMut<MountErrors>) {
    if let SessionEvent::MountResult { mount, code } = ev {
        mount_result(mount, code, &mut errors);
    }
}

/// `SMSG_MOUNTRESULT`/`SMSG_DISMOUNTRESULT`: OK (`MOUNTRESULT_OK` 10, `DISMOUNTRESULT_OK` 3) is
/// silent in the reference; a failure queues the red error line.
fn mount_result(mount: bool, code: u32, errors: &mut MountErrors) {
    let ok = if mount { code == 10 } else { code == 3 };
    if !ok {
        info!(
            "net: {}mount refused (code {code})",
            if mount { "" } else { "dis" }
        );
        errors.0.push((mount, code));
    }
}
