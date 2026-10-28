//! The loot getters' reads at the call, the call host's half of
//! [`benilla_ui::script::LootWindowQuery`], [`benilla_ui::script::LootSlotQuery`] and
//! [`benilla_ui::script::MasterLootCandidateQuery`]: the window's cells as the session holds them
//! (the source `0xb71b48`, the money `0xb71ba0`, the type `0xb71bbc`, the rows `0xb71968`), each
//! row resolved off the item cache as the reference's accessors read it as they run
//! (`0x4c22a0`-`0x4c2670`), the source's object (`0x468460`, typemask 8) and the candidate array
//! `0xc4dc38` named off the player-name cache alone (`0x4c2f70`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    LootRow, LootSlotQuery, LootWindow, LootWindowQuery, MasterLootCandidate,
    MasterLootCandidateQuery,
};

use super::{LootSourceObjects, LootState};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::query_cache::Ask;

/// What the loot getters read as they run; the session absent in a harness without it, where they
/// read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct LootLive<'w, 's> {
    loot: Option<Res<'w, LootState>>,
    items: Option<Res<'w, Items>>,
    icons: Option<Res<'w, ItemDisplays>>,
    rolls: crate::items::Rolls<'w>,
    names: Option<Res<'w, NameCache>>,
    objects: LootSourceObjects<'w, 's>,
}

impl LootLive<'_, '_> {
    /// The window's cells; `None` with no session to read.
    fn window(&self) -> Option<LootWindow> {
        let w = &self.loot.as_deref()?.window;
        Some(LootWindow {
            open: w.source.is_some(),
            count: w.count(),
            loot_type: w.loot_type,
            source_unit: w.source.is_some_and(|g| self.objects.is_unit(g)),
        })
    }

    /// The 0-based display row `slot`; `None` with no session to read.
    fn slot(&self, slot: usize) -> Option<Option<Option<LootRow>>> {
        let (loot, items) = (self.loot.as_deref()?, self.items.as_deref()?);
        Some(
            loot.window
                .row(slot, items, self.icons.as_deref(), self.rolls.get()),
        )
    }

    /// The candidate at the 0-based `index` (`0x4c2f50` → `0x61c660`), named off the player-name
    /// cache alone (`0x4c2f70`, `0xc0e228`), never the roster, so a member the cache lacks reads nil
    /// and takes the miss's callback (`0x4c2f65`); `None` with no session to read.
    fn candidate(&self, index: usize) -> Option<MasterLootCandidate> {
        let loot = self.loot.as_deref()?;
        let Some(guid) = loot.master_candidate(index) else {
            return Some(MasterLootCandidate::Nobody);
        };
        let name = self
            .names
            .as_deref()?
            .resolve_player(guid, Ask::Read)
            .filter(|n| !n.is_empty())
            .map(str::to_string);
        Some(match name {
            Some(name) => MasterLootCandidate::Named(name),
            None => MasterLootCandidate::Unnamed(guid),
        })
    }
}

impl crate::script_calls::live::LiveFamily for LootLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &LootWindowQuery| self.window())
            || ask.answer(|q: &LootSlotQuery| self.slot(q.slot))
            || ask.answer(|q: &MasterLootCandidateQuery| self.candidate(q.index))
    }
}
