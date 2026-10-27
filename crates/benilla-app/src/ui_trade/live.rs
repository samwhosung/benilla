//! The trade getters' reads at the call, the call host's half of
//! [`benilla_ui::script::TradeSlotQuery`] and [`benilla_ui::script::TradeMoneyQuery`]: both offers'
//! records as the session holds them, each resolved off the item cache and the spell names as the
//! reference's getters read them as they run (`0x4c017f`, `0x4c0582`, `0x4c02b7`, `0x4c05d1`), the
//! partner cell `[0xb71728]` and the money offers `[0xc4be70]` (`0x5d4760`, `0x5d4770`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::messages::TRADE_SLOT_COUNT;
use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{TradeMoney, TradeMoneyQuery, TradeSlotItem, TradeSlotQuery};

use super::{resolve_own_slot, resolve_slot, TradeSession, ENCHANT_SLOT};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::ui_session::LatchedWindow;

/// What the trade getters read as they run; the session, the item cache and the name cache each
/// absent in a harness without them, where the getters read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct TradeLive<'w> {
    trade: Option<Res<'w, TradeSession>>,
    items: Option<Res<'w, Items>>,
    icons: Option<Res<'w, ItemDisplays>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    names: Option<Res<'w, NameCache>>,
    rolls: crate::items::Rolls<'w>,
}

impl TradeLive<'_> {
    /// Our 0-based `slot` (`player`) or the partner's, its records only read: a getter's own
    /// lookup is its call's ask. `None` with no session to read.
    fn slot(&self, player: bool, slot: usize) -> Option<Option<TradeSlotItem>> {
        let (trade, items, names) = (
            self.trade.as_deref()?,
            self.items.as_deref()?,
            self.names.as_deref()?,
        );
        if slot >= TRADE_SLOT_COUNT {
            return Some(None);
        }
        let (offer, other) = match player {
            true => (&trade.our, &trade.their),
            false => (&trade.their, &trade.our),
        };
        let Some(item) = offer.slots[slot] else {
            return Some(None);
        };
        let icons = self.icons.as_deref();
        let mut row = match player {
            true => resolve_own_slot(&item, items, icons, names, self.rolls.get()),
            false => resolve_slot(&item, items, icons, names, self.rolls.get()),
        };
        // The column's slot 7 names the spell the other side cast on it; a spell id with no
        // positive `Spell.dbc` row names nothing (`0x4c02be`-`0x4c02d5`).
        if slot == ENCHANT_SLOT {
            let id = other.enchant_spell_id;
            row.enchantment = (id != 0)
                .then(|| self.spells.as_deref()?.catalog.get(id))
                .flatten()
                .map(|d| d.name.clone());
        }
        Some(Some(row))
    }

    /// The cell and both money offers.
    fn money(&self) -> Option<TradeMoney> {
        let trade = self.trade.as_deref()?;
        Some(TradeMoney {
            partner: trade.cell().unwrap_or(0),
            player: trade.our.gold,
            target: trade.their.gold,
        })
    }
}

impl crate::script_calls::live::LiveFamily for TradeLive<'_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &TradeSlotQuery| self.slot(q.player, q.slot))
            || ask.answer(|_: &TradeMoneyQuery| self.money())
    }
}
