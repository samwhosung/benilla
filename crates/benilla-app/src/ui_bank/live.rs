//! The bank getters' reads at the call, the call host's half of
//! [`benilla_ui::script::BankSlotsQuery`]: the active player's bought bag slots and the price of
//! the next, as `GetNumBankSlots` and `GetBankSlotCost` read them (`0x4f85b0`, `0x4f8630`), the
//! player's store as of the drain's place ([`crate::ui_unit::views`]).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{BankSlotsQuery, BankState};

use super::BankPrices;
use crate::net::Objects;

/// What the bank getters read as they run; the price table absent without client data, where
/// every price reads 0.
#[derive(SystemParam)]
pub(crate) struct BankLive<'w, 's> {
    objects: Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    prices: Option<Res<'w, BankPrices>>,
}

impl BankLive<'_, '_> {
    /// `PLAYER_BYTES_2` byte 2 of the active player as of the drain's place (`0x4f85dd`) and the
    /// price row after it (`0x4f8690`), both 0 with no player (`0x4f85e3`, `0x4f866a`); no banker
    /// is tested.
    fn slots(&self) -> BankState {
        let Some(player) = self.objects.player().and_then(|e| self.stores.get(e).ok()) else {
            return BankState::default();
        };
        let bought = player.0.player_bank_bag_slots_purchased().unwrap_or(0);
        BankState {
            num_purchased: u32::from(bought),
            next_cost: self
                .prices
                .as_deref()
                .and_then(|p| p.0.next_slot_price(bought))
                .unwrap_or(0),
        }
    }
}

impl crate::script_calls::live::LiveFamily for BankLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &BankSlotsQuery| Some(self.slots()))
    }
}
