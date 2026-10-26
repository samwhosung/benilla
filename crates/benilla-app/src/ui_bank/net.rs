//! The bank's packet handlers: the open writes [`BankOpen`], and each signals on the bank lane
//! at its packet ([`super::lane`]).

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{BankEvent, BankLane, BankOpen};
use crate::net::NetHandlerApp;

/// Register the bank handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::ShowBank, on_show_bank)
        .net_handler(K::BuyBankSlotResult, on_buy_slot_result)
        .net_handler(K::Disconnected, on_session_end);
}

/// `SMSG_SHOW_BANK`, sent for `CMSG_BANKER_ACTIVATE` and unprompted for the gossip bank option:
/// its open registers on the latch, which closes whatever window holds it, the gossip menu too.
fn on_show_bank(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::ShowBank { banker } = ev {
        debug!("net: bank opened at {banker:#x}");
        commands.queue(move |world: &mut World| super::open_bank(world, banker));
    }
}

/// `SMSG_BUY_BANK_SLOT_RESULT` (`0x5e3f8d`), a refusal: vmangos sends it only on failure. Its
/// line shows inside the handler, on the bank lane.
fn on_buy_slot_result(In(ev): In<SessionEvent>, mut lane: ResMut<BankLane>) {
    if let SessionEvent::BuyBankSlotResult { result } = ev {
        debug!("net: bank slot purchase refused (code {result})");
        if let Some(key) = super::bank_slot_error_key(result) {
            lane.push(BankEvent::Line(key));
        }
    }
}

/// The bank window closes with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut bank: ResMut<BankOpen>) {
    bank.clear_session();
}
