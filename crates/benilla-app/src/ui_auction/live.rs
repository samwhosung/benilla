//! The auction getters' reads at the call, the call host's half of
//! [`benilla_ui::script::AuctionRowQuery`], [`benilla_ui::script::AuctionCanQuery`] and
//! [`benilla_ui::script::AuctionDepositRate`]: a row's record as the house holds it, resolved off
//! the item and name caches as the reference's getters read them as they run (`0x4cef96`,
//! `0x4cf144`, `0x4cf1b2`); the browse throttle `[0xb72638]` against the clock (`0x4cfbb0`); and
//! the house's deposit rate (`0x4ce170`). The VM keeps each list's order, which its sorts set.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{AuctionCanQuery, AuctionDepositRate, AuctionItemRow, AuctionRowQuery};

use super::{resolve_row, AuctionHouses, AuctionOpen};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{Guid, SelfPlayer};
use crate::query_cache::Ask;

/// What the auction getters read as they run; each absent in a harness without it, where the
/// getters read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct AuctionLive<'w, 's> {
    auction: Option<Res<'w, AuctionOpen>>,
    items: Option<Res<'w, Items>>,
    icons: Option<Res<'w, ItemDisplays>>,
    names: Option<Res<'w, NameCache>>,
    rolls: crate::items::Rolls<'w>,
    /// The player, whose bid a row's status ranks first (`0x468550`).
    self_q: Query<'w, 's, &'static Guid, With<SelfPlayer>>,
    /// The browse throttle's clock ([`AuctionOpen::query_gate`]).
    time: Option<Res<'w, Time>>,
    houses: Option<Res<'w, AuctionHouses>>,
}

impl AuctionLive<'_, '_> {
    /// `q`'s row as the house holds it, its item read without a callback (`0x4cef96`) and its
    /// names from the cache; `None` for a page the list no longer holds or a row it lacks.
    fn row(&self, q: &AuctionRowQuery) -> Option<AuctionItemRow> {
        let (auction, items, names) = (
            self.auction.as_deref()?,
            self.items.as_deref()?,
            self.names.as_deref()?,
        );
        let slot = auction.lists.get(q.list).filter(|s| s.page == q.page)?;
        let arrived = slot.arrived_ms?;
        let entry = slot.entries.iter().find(|e| e.auction_id == q.auction_id)?;
        Some(resolve_row(
            q.list,
            entry,
            arrived,
            self.self_q.iter().next().map(|g| g.0),
            items,
            self.icons.as_deref(),
            names,
            Ask::Read,
            self.rolls.get(),
        ))
    }

    /// No throttle armed, or the clock past it.
    fn can_query(&self) -> Option<bool> {
        let auction = self.auction.as_deref()?;
        let now = self.time.as_deref()?.elapsed_secs_f64();
        Some(auction.query_gate.is_none_or(|gate| now >= gate))
    }

    /// The `AuctionHouse.dbc` rate of the house the session holds, 0 with no row.
    fn deposit_rate(&self) -> Option<u32> {
        let auction = self.auction.as_deref()?;
        Some(
            self.houses
                .as_deref()
                .and_then(|h| h.0.deposit_percent(auction.house_id))
                .unwrap_or(0),
        )
    }
}

impl crate::script_calls::live::LiveFamily for AuctionLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &AuctionRowQuery| self.row(q))
            || ask.answer(|_: &AuctionCanQuery| self.can_query())
            || ask.answer(|_: &AuctionDepositRate| self.deposit_rate())
    }
}
