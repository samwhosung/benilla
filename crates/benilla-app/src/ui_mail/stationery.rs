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
use crate::net::NetCommands;
use crate::query_cache::Ask;

/// The list as last rebuilt, in the picker's order, and the papers' items the world-enter init
/// still awaits (`0xb6efbc`), one per row whose item missed (`0x4acbc3`).
#[derive(Resource, Default)]
pub(crate) struct UsableStationery {
    list: Vec<StationeryView>,
    awaited: Vec<u32>,
}

impl UsableStationery {
    /// The list as last rebuilt.
    pub(crate) fn list(&self) -> &[StationeryView] {
        &self.list
    }

    /// The leave-world step's cancel (`0x4acc00`): each paper's callback `0x4acbf0` goes
    /// (`0x55c0c0` at `0x4acc2d`), and the count stays as it stands.
    pub(crate) fn cancel_waits(items: &Items) {
        items.cancel_waits(crate::items::RecordCallback::Stationery);
    }
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

    /// `0x4ad970`: the list rebuilt off the player's items now. With no player it returns at once
    /// (`0x4ad9aa`), the list and its count kept.
    pub(crate) fn rebuild(&mut self, items: &Items) {
        let (Some(usable), Some(catalog)) = (self.usable.as_deref_mut(), self.catalog.as_deref())
        else {
            return;
        };
        let Some(store) = self
            .objects
            .player()
            .and_then(|e| self.stores.get(e).ok())
            .map(|s| &s.0)
        else {
            return;
        };
        let objects = self.stores.item_objects(&self.objects);
        usable.list = stationeries(catalog, Some(store), &objects, items, self.icons.as_deref());
    }

    /// `GetNumStationeries`' rebuild (`0x4ae1f6`-`0x4ae202`): only while no paper's item is
    /// awaited.
    pub(crate) fn rebuild_unless_awaited(&mut self, items: &Items) {
        if self.usable.as_deref().is_some_and(|u| u.awaited.is_empty()) {
            self.rebuild(items);
        }
    }

    /// One awaited row's callback on `entry`'s answer, a record or a refusal (`0x4acbf0`): it
    /// counts one down, and the one that reaches zero rebuilds the list (`0x4acbf8`).
    pub(crate) fn answered(&mut self, entry: u32, items: &Items) {
        let Some(usable) = self.usable.as_deref_mut() else {
            return;
        };
        let Some(at) = usable.awaited.iter().position(|&e| e == entry) else {
            return;
        };
        usable.awaited.remove(at);
        if usable.awaited.is_empty() {
            self.rebuild(items);
        }
    }
}

/// The mail init's stationery half (`0x4acb73`-`0x4acbd7`) in each world-enter cascade's place
/// ([`crate::ui_world_enter`]): the count starts over, each row's item is looked up with the
/// callback `0x4acbf0` (`0x4acbba`), one awaited per miss (`0x4acbc3`), and the list is rebuilt
/// when none is. The lookups ask without the rebuild's reads, in a harness without them.
pub(crate) fn count_papers_on_enter(
    commands: Res<NetCommands>,
    items: Option<Res<Items>>,
    stationery: Option<Res<Stationery>>,
    paper: Option<StationeryRebuild>,
) {
    let Some(items) = items.as_deref() else {
        return;
    };
    let missed: Vec<u32> = stationery
        .iter()
        .flat_map(|s| s.0.rows())
        .filter(|row| items.template(row.item, 0, Ask::Event(&commands)).is_none())
        .map(|row| row.item)
        .collect();
    for &entry in &missed {
        items.wait(entry, crate::items::RecordCallback::Stationery);
    }
    if let Some(mut paper) = paper {
        if let Some(usable) = paper.usable.as_deref_mut() {
            usable.awaited = missed;
        }
        paper.rebuild_unless_awaited(items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{ObjectStore, SelfPlayer};
    use bevy::ecs::system::RunSystemOnce;

    /// The rebuild with no player returns at once (`0x4ad9aa je 0x4ada85`), ahead of the count's
    /// reset (`0x4ad9b8`): the list stands as last rebuilt.
    #[test]
    fn a_rebuild_with_no_player_keeps_the_list() {
        const PAPER: u32 = 9_311;
        const ITEM: u64 = 0x4000_0000_0000_0001;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut items = Items::default();
        items.insert_template(PAPER, Some(crate::items::test_template("Plain Paper")));
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<UsableStationery>()
            .insert_resource(items)
            .insert_resource(NetCommands(tx))
            .insert_resource(Stationery(benilla_formats::StationeryCatalog::from_rows(
                vec![benilla_formats::StationeryRow {
                    id: 5,
                    item: PAPER,
                    texture: "PAPER".into(),
                    flags: 0,
                }],
            )));
        crate::items::test_spawn_item(
            app.world_mut(),
            ITEM,
            benilla_protocol::ObjectFields::from_pairs(&[(3, PAPER), (14, 1)]),
            false,
        );
        let backpack = benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
        let player = app
            .world_mut()
            .spawn((
                SelfPlayer,
                crate::net::Guid(7),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (backpack, ITEM as u32),
                    (backpack + 1, (ITEM >> 32) as u32),
                ])),
            ))
            .id();
        let rebuilt = |app: &mut App| {
            app.world_mut()
                .run_system_once(|mut paper: StationeryRebuild, items: Res<Items>| {
                    paper.rebuild(&items);
                    paper.list().map_or(0, <[_]>::len)
                })
                .unwrap()
        };
        assert_eq!(rebuilt(&mut app), 1, "the carried paper");
        app.world_mut().entity_mut(player).remove::<SelfPlayer>();
        assert_eq!(rebuilt(&mut app), 1, "no player, the list kept");
    }
}
