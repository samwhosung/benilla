//! The stable getters' reads at the call, the call host's half of
//! [`benilla_ui::script::StableQuery`]: the master's cell, the slot count and the records the list
//! left (`0xb72240`, `0xb7224c`, `0xb72058`, `0xb720c8`), each record's creature read off the
//! cache as the getters read it (`0x4cb2ce`), the next slot's price (`StableSlotPrices.dbc` row
//! `[0xb7224c] + 1`), the live-pet test (`[0xb714a0]|[0xb714a4]`) and the summoned pet's display
//! (`[unit+0xb30]`, `0x4cb99e`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{StableQuery, StableView};

use super::{snapshot, StableOpen, StableSlotPrices};
use crate::names::NameCache;
use crate::net::Objects;
use crate::query_cache::Ask;
use crate::ui_pet::PetBar;
use crate::ui_pet_stats::{PetFamilyTables, PetStatTables};

/// What the stable getters read as they run; the window, the pet bar and the name cache each
/// absent in a harness without them, where the getters read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct StableLive<'w, 's> {
    open: Option<Res<'w, StableOpen>>,
    bar: Option<Res<'w, PetBar>>,
    names: Option<Res<'w, NameCache>>,
    families: Option<Res<'w, PetFamilyTables>>,
    stats: Option<Res<'w, PetStatTables>>,
    prices: Option<Res<'w, StableSlotPrices>>,
    objects: Objects<'w, 's>,
}

impl StableLive<'_, '_> {
    /// The stable now; its records only read: a getter's own lookup is its call's ask.
    pub(super) fn view(&self) -> Option<StableView> {
        let (open, bar, names) = (
            self.open.as_deref()?,
            self.bar.as_deref()?,
            self.names.as_deref()?,
        );
        let next_slot_cost = self
            .prices
            .as_deref()
            .and_then(|p| p.0.next_slot_price(open.num_stable_slots))
            .unwrap_or(0);
        // A dismissed pet fails the live test.
        let pet_guid = bar.spells.pet_guid;
        let live_pet_display = (pet_guid != 0)
            .then(|| self.objects.store(pet_guid))
            .flatten()
            .and_then(|s| s.0.object_entry())
            .and_then(|entry| names.creature_record(entry))
            .map(|r| r.display_id);
        Some(StableView {
            state: snapshot(
                open,
                names,
                self.families.as_deref(),
                self.stats.as_deref(),
                next_slot_cost,
                pet_guid != 0,
                Ask::Read,
            ),
            live_pet_display,
        })
    }
}

impl StableLive<'_, '_> {
    /// The list the window holds and the stable as it stands, the copy a list's signal leaves in
    /// the VM.
    pub(super) fn listed(&self) -> (u64, StableView) {
        let list = self.open.as_deref().map_or(0, |o| o.list);
        (list, self.view().unwrap_or_default())
    }
}

impl crate::script_calls::live::LiveFamily for StableLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &StableQuery| {
            // A list landed whose `PET_STABLE_SHOW` has not fired: this moment reads the copy.
            self.open.as_deref().filter(|o| o.list == q.list)?;
            self.view()
        })
    }
}
