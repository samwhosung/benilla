//! The mount and dismount result packets, onto the red error line, and the game entry's empty
//! bar.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{ActionSlots, MountErrors};
use crate::net::NetHandlerApp;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::MountResult, on_mount_result)
        .net_handler(SessionEventKind::Connected, on_game_entry);
}

/// The game entry, at the login's handler ahead of the world's first packet: the bar emptied, as
/// `CGGameUI::InitializeGame` zeroes it (`0x48f5a4` → `ResetActionBarState 0x4e4f50`), so a new
/// character's book and bar never meet the last one's, however that session ended.
fn on_game_entry(In(ev): In<SessionEvent>, mut slots: ResMut<ActionSlots>) {
    if let SessionEvent::Connected { .. } = ev {
        slots.game_entry();
    }
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
