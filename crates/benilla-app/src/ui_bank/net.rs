//! The bank's packet handlers: they fill [`BankOpen`] and [`BankErrors`] for the bank feed.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{BankErrors, BankOpen};
use crate::net::NetHandlerApp;
use crate::ui_gossip::GossipState;
use crate::ui_quest::QuestGiver;

/// Register the bank handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::ShowBank, on_show_bank)
        .net_handler(K::BuyBankSlotResult, on_buy_slot_result)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_show_bank(
    In(ev): In<SessionEvent>,
    mut bank: ResMut<BankOpen>,
    mut gossip: ResMut<GossipState>,
    mut quest: ResMut<QuestGiver>,
) {
    if let SessionEvent::ShowBank { banker } = ev {
        show_bank(banker, &mut bank, &mut gossip, &mut quest);
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

/// `SMSG_SHOW_BANK`, sent for `CMSG_BANKER_ACTIVATE` and unprompted for the gossip bank option.
/// It ends an open gossip interaction: vmangos sends no `SMSG_GOSSIP_COMPLETE` for that option
/// (`Player::OnGossipSelect`), and the bank's `pushable = 6` (`UIParent.lua:28`) would open beside
/// the gossip frame rather than replace it. The reference's gossip menu is gone once the vault
/// shows; its mechanism for that is untraced.
fn show_bank(banker: u64, bank: &mut BankOpen, gossip: &mut GossipState, quest: &mut QuestGiver) {
    debug!("net: bank opened at {banker:#x}");
    if gossip.npc.is_some() {
        crate::ui_gossip::end_interaction(gossip, quest);
    }
    bank.open(banker);
}

/// `SMSG_BUY_BANK_SLOT_RESULT`, a refusal: vmangos sends it only on failure.
fn bank_buy_slot_result(result: u32, errors: &mut BankErrors) {
    debug!("net: bank slot purchase refused (code {result})");
    errors.0.push(result);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_bank_ends_the_gossip_interaction() {
        let mut bank = BankOpen::default();
        let mut gossip = GossipState::default();
        let mut quest = QuestGiver::default();
        gossip.npc = Some(0x42);

        show_bank(0x42, &mut bank, &mut gossip, &mut quest);
        assert_eq!(bank.banker, Some(0x42));
        assert_eq!(gossip.npc, None, "the gossip session ended with the menu");

        // No gossip open: a plain open, nothing else touched.
        bank.clear();
        show_bank(0x43, &mut bank, &mut gossip, &mut quest);
        assert_eq!(bank.banker, Some(0x43));
    }
}
