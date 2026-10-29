//! The mount and dismount result packets, onto the red error line, and the game entry's empty
//! bar.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{ActionSlots, UiError, UiErrorKeys};
use crate::net::NetHandlerApp;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::MountResult, on_mount_result)
        .net_handler(SessionEventKind::Connected, on_game_entry);
}

/// The game entry, at the login's handler ahead of the world's first packet: the bar and its usable
/// flags emptied, as `CGGameUI::InitializeGame` zeroes them (`0x48f5a4` → `ResetActionBarState
/// 0x4e4f50`), so a new
/// character's book and bar never meet the last one's, however that session ended.
fn on_game_entry(
    In(ev): In<SessionEvent>,
    mut slots: ResMut<ActionSlots>,
    mut states: ResMut<super::ActionStateMemory>,
) {
    if let SessionEvent::Connected { .. } = ev {
        slots.game_entry();
        states.game_entry();
    }
}

fn on_mount_result(
    In(ev): In<SessionEvent>,
    errors: Res<UiErrorKeys>,
    me: Query<(), With<crate::net::SelfPlayer>>,
) {
    if let SessionEvent::MountResult { mount, code } = ev {
        mount_result(mount, code, !me.is_empty(), &errors);
    }
}

/// `SMSG_MOUNTRESULT`/`SMSG_DISMOUNTRESULT` (arms of `0x5e38c0`): with the active player
/// resolved (`0x5e3d02`, `0x5e3dd9`), the code's line through `0x5ec6e0` or `0x5ec700`
/// ([`super::errors::mount_result_key`]), the handler's one signal. OK (`MOUNTRESULT_OK` 10,
/// `DISMOUNTRESULT_OK` 3) is the tables' none row, `0x1d1`, which shows nothing.
fn mount_result(mount: bool, code: u32, player: bool, errors: &UiErrorKeys) {
    let Some(key) = super::errors::mount_result_key(mount, code).filter(|_| player) else {
        return;
    };
    info!(
        "net: {}mount refused (code {code})",
        if mount { "" } else { "dis" }
    );
    errors.push(UiError::key(key));
}
