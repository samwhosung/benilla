//! The taxi getters' reads at the call, the call host's half of
//! [`benilla_ui::script::TaxiMapQuery`] and [`benilla_ui::script::TaxiNodeQuery`]: the slots the
//! open's builder made (`0xbb4aa8`), and each fare taken at the call off the flight master's cell
//! (`0xbb4ab8`) with its discount for the player (`0x4dc3d0`, `0x4dc45d`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{TaxiMapHead, TaxiMapQuery, TaxiNodeQuery, TaxiUiNode};

use super::routing::{shown_fare, TaxiRouteCache};
use super::TaxiState;
use crate::net::Objects;

/// What the taxi getters read as they run; the map absent in a harness without it, where the
/// getters read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct TaxiLive<'w, 's> {
    state: Option<Res<'w, TaxiState>>,
    cache: Option<Res<'w, TaxiRouteCache>>,
    objects: Objects<'w, 's>,
    relations: crate::target::RelationWorld<'w, 's>,
}

impl TaxiLive<'_, '_> {
    /// The flight master's price discount for the player, `None` while the cell, the master's
    /// unit or the player does not resolve (`0x4dc3fa`, `0x4dc423`).
    fn discount(&self) -> Option<f64> {
        let guid = self.state.as_deref()?.open.as_ref()?.flightmaster;
        let master = self.objects.store(guid)?;
        let player = self.objects.store_of(self.objects.player()?)?;
        let me = self.relations.me()?;
        let master = self.relations.unit_of(guid, master);
        Some(crate::target::vendor_price_discount(
            &self.relations.get(),
            &master,
            &me,
            player,
        ))
    }

    /// Slot `index` with its fare now.
    fn node(&self, index: usize) -> Option<Option<TaxiUiNode>> {
        let slot = self.cache.as_deref()?.slots.get(index);
        Some(slot.map(|slot| TaxiUiNode {
            cost: self.discount().map_or(0, |d| shown_fare(slot.price, d)),
            ..slot.view.clone()
        }))
    }
}

impl TaxiLive<'_, '_> {
    /// The whole map as the getters read it now, `None` with no art built.
    #[cfg(test)]
    pub(super) fn view(&self) -> Option<benilla_ui::script::TaxiUiState> {
        let cache = self.cache.as_deref()?;
        Some(benilla_ui::script::TaxiUiState {
            art: cache.art.clone()?,
            nodes: (0..cache.slots.len())
                .filter_map(|i| self.node(i).flatten())
                .collect(),
        })
    }
}

impl crate::script_calls::live::LiveFamily for TaxiLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &TaxiMapQuery| {
            let cache = self.cache.as_deref()?;
            Some(TaxiMapHead {
                art: cache.art.clone(),
                nodes: cache.slots.len(),
            })
        }) || ask.answer(|q: &TaxiNodeQuery| self.node(q.0))
    }
}
