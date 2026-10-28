//! The usable stationery list as its last rebuild left it (`0x4ad970`). The reference rebuilds it
//! at three places only: the world-enter init when no paper's item is awaited (`0x4acbd7`), the
//! last awaited item's answer (`0x4acbf8`) and `GetNumStationeries` when none is awaited
//! (`0x4ae202`). Every other reader (the stationery getters, the postage, the list parse's default
//! paper) reads it as it stands, so a paper picked up or used between those places changes
//! nothing until the next.

use benilla_ui::script::StationeryView;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{stationeries, Stationery};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, WorldEnterCascadeMessage};
use crate::query_cache::Ask;

/// The list as last rebuilt, in the picker's order, and the papers' items the world-enter init
/// still awaits (`0xb6efbc`), one per row whose item missed (`0x4acbc3`).
#[derive(Resource, Default)]
pub(crate) struct UsableStationery {
    list: Vec<StationeryView>,
    awaited: Vec<u32>,
}

/// The list and what its rebuild reads: the store, the item records and icons, and the player's
/// items as of the drain's place (`0x4ad9fc` → `0x622270`).
#[derive(SystemParam)]
pub(crate) struct StationeryRebuild<'w, 's> {
    usable: Option<ResMut<'w, UsableStationery>>,
    catalog: Option<Res<'w, Stationery>>,
    icons: Option<Res<'w, ItemDisplays>>,
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
}

impl StationeryRebuild<'_, '_> {
    /// The list as last rebuilt; `None` with no store to build it from, or no list held.
    pub(crate) fn list(&self) -> Option<&[StationeryView]> {
        self.catalog.as_deref()?;
        Some(&self.usable.as_deref()?.list)
    }

    /// `0x4ad970`: the list rebuilt off the player's items now, none carried with no player.
    pub(crate) fn rebuild(&mut self, items: &Items) {
        let (Some(usable), Some(catalog)) = (self.usable.as_deref_mut(), self.catalog.as_deref())
        else {
            return;
        };
        let store = self
            .objects
            .player()
            .and_then(|e| self.stores.get(e).ok())
            .map(|s| &s.0);
        let objects = self.stores.item_objects(&self.objects);
        usable.list = stationeries(catalog, store, &objects, items, self.icons.as_deref());
    }

    /// `GetNumStationeries`' rebuild (`0x4ae1f6`-`0x4ae202`): only while no paper's item is
    /// awaited.
    pub(crate) fn rebuild_unless_awaited(&mut self, items: &Items) {
        if self.usable.as_deref().is_some_and(|u| u.awaited.is_empty()) {
            self.rebuild(items);
        }
    }

    /// `entry`'s answer, a record or a refusal: each awaited row's callback counts it down
    /// (`0x4acbf0`), and the one that reaches zero rebuilds the list (`0x4acbf8`).
    pub(super) fn answered(&mut self, entry: u32, items: &Items) {
        let Some(usable) = self.usable.as_deref_mut() else {
            return;
        };
        let before = usable.awaited.len();
        usable.awaited.retain(|&e| e != entry);
        if usable.awaited.len() < before && usable.awaited.is_empty() {
            self.rebuild(items);
        }
    }
}

/// The mail init's stationery half (`0x4acb73`-`0x4acbd7`) at each world-enter cascade: the count
/// starts over, each row's item is looked up with the callback `0x4acbf0` (`0x4acbba`), one
/// awaited per miss (`0x4acbc3`), and the list is rebuilt when none is. The lookups ask without the
/// rebuild's reads, in a harness without them.
pub(super) fn count_papers_on_enter(
    mut cascades: MessageReader<WorldEnterCascadeMessage>,
    commands: Res<NetCommands>,
    items: Option<Res<Items>>,
    stationery: Option<Res<Stationery>>,
    paper: Option<StationeryRebuild>,
) {
    if cascades.read().next().is_none() {
        return;
    }
    let Some(items) = items.as_deref() else {
        return;
    };
    let missed: Vec<u32> = stationery
        .iter()
        .flat_map(|s| s.0.rows())
        .filter(|row| items.template(row.item, 0, Ask::Event(&commands)).is_none())
        .map(|row| row.item)
        .collect();
    if let Some(mut paper) = paper {
        if let Some(usable) = paper.usable.as_deref_mut() {
            usable.awaited = missed;
        }
        paper.rebuild_unless_awaited(items);
    }
}
