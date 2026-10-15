//! The bank's packet handlers: they fill [`BankOpen`] and [`BankErrors`] for the bank feed.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{BankErrors, BankOpen};
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

fn on_buy_slot_result(In(ev): In<SessionEvent>, mut errors: ResMut<BankErrors>) {
    if let SessionEvent::BuyBankSlotResult { result } = ev {
        bank_buy_slot_result(result, &mut errors);
    }
}

/// The bank window closes with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut bank: ResMut<BankOpen>) {
    bank.clear_session();
}

/// `SMSG_BUY_BANK_SLOT_RESULT`, a refusal: vmangos sends it only on failure.
fn bank_buy_slot_result(result: u32, errors: &mut BankErrors) {
    debug!("net: bank slot purchase refused (code {result})");
    errors.0.push(result);
}
