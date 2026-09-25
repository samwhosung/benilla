//! The bank's packet handlers (in the net handler table since 2318, moved out of
//! the drain's npc arm file) — the [`BankOpen`] session and the [`BankErrors`] line queue the bank
//! feed ([`super`]) reads.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{BankErrors, BankOpen};
use crate::net::NetHandlerApp;
use crate::ui_gossip::GossipState;
use crate::ui_quest::QuestGiver;

/// Register the bank handlers — called from [`super::UiBankPlugin`]. One per kind, plus the
/// session-end listener.
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

/// The bank window dies with the socket — a reconnect re-opens via the banker.
/// A listener on the session end (a second handler on the kind, after the bridge's own teardown).
fn on_session_end(In(_): In<SessionEvent>, mut bank: ResMut<BankOpen>) {
    bank.clear_session();
}

/// The bank opened (`SMSG_SHOW_BANK`): point the [`BankOpen`] session the bank feed
/// ([`super`]) reads at the banker. Sent for our own `CMSG_BANKER_ACTIVATE` *and*
/// volunteered by the server for the gossip menu's bank option (`GOSSIP_OPTION_BANKER` →
/// `SendShowBank`) — so this never assumes we asked. The vault's contents are
/// descriptor fields already streamed; the window renders from local state.
///
/// Opening the bank ends any open gossip interaction (the `SMSG_GOSSIP_COMPLETE` clear): vmangos
/// sends no `SMSG_GOSSIP_COMPLETE` for the gossip menu's bank option (VERIFIED
/// `Player::OnGossipSelect` — BANKER only calls `SendShowBank`), and the panel slots alone can't
/// close the menu — the bank's `pushable = 6` sends it to the *center* slot beside a pushable-0
/// gossip instead of replacing it, so the menu would linger beside the vault
/// (director-observed). The real client ends the old NPC interaction when the new one starts
/// (INFERRED — the exact C++ `GOSSIP_CLOSED` fire isn't RE'd; the observable is vanilla's: the
/// menu is gone once the vault is up).
fn show_bank(banker: u64, bank: &mut BankOpen, gossip: &mut GossipState, quest: &mut QuestGiver) {
    debug!("net: bank opened at {banker:#x}");
    if gossip.npc.is_some() {
        crate::ui_gossip::end_interaction(gossip, quest);
    }
    bank.open(banker);
}

/// A bank-slot purchase refusal (`SMSG_BUY_BANK_SLOT_RESULT` — vmangos sends it only on failure;
/// success is the PLAYER_BYTES_2 delta): queue it for the bank feed's red error line.
fn bank_buy_slot_result(result: u32, errors: &mut BankErrors) {
    debug!("net: bank slot purchase refused (code {result})");
    errors.0.push(result);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SMSG_SHOW_BANK` ends an open gossip interaction (the gossip menu's bank option arrives
    /// with no `SMSG_GOSSIP_COMPLETE` — `show_bank`'s doc): the gossip/quest sessions clear and
    /// the bank session opens. A direct right-click (no gossip open) just opens the bank.
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
