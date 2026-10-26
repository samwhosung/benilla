//! Inspect: another player's public descriptor as the `GetInventoryItem*` verbs read it through a
//! token naming them, at the call ([`InspectLive`]). Their inventory guids are server-private, so
//! there are no item objects, only the `PLAYER_VISIBLE_ITEM_*` entries and their templates, which
//! is why the reference's inspect doll shows no counts and no durability.
//!
//! `NotifyInspect(unit)` sends `CMSG_INSPECT`, which also sets the server-side selection, and sets
//! the inspect target to the unit's guid (`0x5ded1c` → `0x4c6f70`); `SMSG_INSPECT` echoes only the
//! guid (`0x5e7d70`). The window paints inside the call that shows it
//! (`InspectPaperDollFrame.lua:57-79`), off the objects as they stand, so nothing here signals: a
//! change of the gear, as of the level, is the unit-field bridge's ([`crate::field_watch`]), fired
//! per token on a values block.

use benilla_ui::script::ItemRead;

use bevy::prelude::*;

use benilla_ui::script::{InspectCall, InvSlotView, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::query_cache::Ask;

mod live;

pub(crate) use live::InspectLive;

/// One slot from a player's visible-item entry; `slot0` is the 0-based `EQUIPMENT_SLOT_*` index,
/// `slot0 + 1` in Lua. With no item object the count is the reference's always-1 and durability,
/// flags and locks stay inert. The block's creator guid is the maker line's (`0x5333cd` copies it
/// to the tooltip's `+0x410`, read at `0x52e1d5`); its name is read, and the tooltip build asks
/// it. Only the enchants broadcast, at
/// `PLAYER_VISIBLE_ITEM_<slot>_0 + 1 + j`, and 1.12 fills PERM and TEMP alone (vmangos
/// `SetVisibleItemSlot`, `MAX_INSPECTED_ENCHANTMENT_SLOT`). The record is read, never asked: the
/// getters' lookup passes no callback (`0x4c8420` → `0x5f0d60` → `0x62eb40`).
fn inspect_slot_view(
    store: &ObjectStore,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    names: &crate::names::NameCache,
    slot0: u8,
) -> Option<InvSlotView> {
    let entry = store.0.player_visible_item_entry(slot0)?;
    let creator_guid = store.0.player_visible_item_creator(slot0);
    let creator = (creator_guid != 0)
        .then(|| names.resolve_player(creator_guid, Ask::Read))
        .flatten()
        .map(str::to_string);
    // The roll, the properties field's low word as the reference's inspect leg reads it into
    // `+0x424`, names the suffix only: its stat lines would sit in enchant slots 2-6, which 1.12
    // servers leave empty.
    let roll = store.0.player_visible_item_properties(slot0);
    // The link names the permanent enchant, the block's `+0xc`, and the suffix at `+0x28`
    // (`0x4c8d5c`-`0x4c8d74` → `0x52adb0`).
    let roll = benilla_ui::script::ItemRoll {
        enchant: store.0.player_visible_item_enchant(slot0, 0).unwrap_or(0),
        property: roll,
        suffix_factor: 0,
    };
    // By entry alone (guid 0), as the reference's item cache asks: there is no item object. What
    // the getters read of it as they run, read here the same way.
    let live = crate::ui_items::live::LiveItem::record(entry, roll, items, icons, rolls);
    Some(InvSlotView {
        item_id: entry,
        icon: live.texture(),
        count: 1,
        quality: live.quality().map_or(0, |q| q as i32),
        inventory_type: live.cached().then(|| live.inventory_type()),
        name: live.name(),
        link: live.link(),
        roll,
        creator,
        creator_guid,
        // `0x5da2c0` reads an item object's `ITEM_FIELD_FLAGS`, which inspected gear lacks, so the
        // tooltip prints the template's bind line, as the reference's does.
        already_bound: false,
        // All 7 slots, as the reference's inspect leg copies them (`0x533354`), with no charges
        // and no countdown: there is no item object.
        enchants: crate::items::enchant_lines(
            (0..7).map(|j| {
                let id = store.0.player_visible_item_enchant(slot0, j).unwrap_or(0);
                (j, id as i32, 0, None)
            }),
            rolls.enchants,
        ),
        ..Default::default()
    })
}

/// What a player's visible-item views read beside the descriptor: the item records, the icons,
/// the `SpellItemEnchantment` and `ItemRandomProperties` catalogs that name the enchant lines
/// and the roll suffix, and the name cache that names the creator.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct VisibleItemTables<'w> {
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
    names: Res<'w, crate::names::NameCache>,
}

impl VisibleItemTables<'_> {
    /// A player's slot `slot0`, the 0-based `EQUIPMENT_SLOT_*` index, as of `store`.
    pub(crate) fn slot(&self, store: &ObjectStore, slot0: u8) -> Option<InvSlotView> {
        let rolls = crate::items::RollCatalogs {
            enchants: self.enchants.as_deref(),
            props: self.props.as_deref(),
        };
        inspect_slot_view(
            store,
            &self.items,
            self.icons.as_deref(),
            rolls,
            &self.names,
            slot0,
        )
    }

    /// A player's 19 slots by slot id.
    #[cfg(test)]
    pub(crate) fn slots(&self, store: &ObjectStore) -> benilla_ui::script::InventorySlots {
        let mut slots: benilla_ui::script::InventorySlots = Default::default();
        for slot in 1..=19u8 {
            slots[usize::from(slot)] = self.slot(store, slot - 1);
        }
        slots
    }
}

/// The inspect verbs inside their calls ([`benilla_ui::script::InspectCall`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct InspectSteps<'w, 's> {
    /// The inspect target and its honor slot; absent in a harness without the inspect window.
    slot: Option<ResMut<'w, crate::ui_honor::InspectHonor>>,
    selection: Res<'w, crate::target::Selection>,
    tokens: crate::ui_unit::UnitTokens<'w, 's>,
    net: Res<'w, NetCommands>,
}

impl InspectSteps<'_, '_> {
    /// `call` inside its verb.
    pub(crate) fn apply(
        &mut self,
        _script: &mut UiScript,
        call: InspectCall,
    ) -> Option<benilla_ui::script::CallTail> {
        let dropped = match call {
            // `0x48a36a`: the token through the unit resolver `0x515970`, any unit; then
            // `0x5decc0`, for a nonzero guid (`0x5deccf`), `CMSG_INSPECT` and then the inspect
            // target set to it (`0x5ded10`, `0x5ded1c`).
            InspectCall::Notify(token) => {
                let Some(guid) = self.tokens.guid(&token, &self.selection) else {
                    debug!("inspect: {token:?} names nobody — nothing sent");
                    return None;
                };
                debug!("inspect: {token:?} -> {guid:#x}; sending CMSG_INSPECT");
                let _ = self.net.0.send(ClientCommand::Inspect { target: guid });
                self.slot.as_deref_mut().is_some_and(|s| s.set_target(guid))
            }
            // `ClearInspectPlayer()` (`0x4c9780`): the target set to 0 (`0x4c9784`).
            InspectCall::Clear => self.slot.as_deref_mut().is_some_and(|s| s.set_target(0)),
            // `RequestInspectHonorData()` past its two flags: for a nonzero target guid
            // (`0x4c80c2`), `MSG_INSPECT_HONOR_STATS` with it (`0x4c8112`), then pending set
            // (`0x4c811b`). With none it returns before both, and the pane asks again from its
            // next `OnShow`.
            InspectCall::Honor => {
                let Some(slot) = self.slot.as_deref_mut().filter(|s| s.target() != 0) else {
                    debug!("honor: inspect-honor request with no target — not sent");
                    return None;
                };
                let target = slot.target();
                debug!("honor: inspect-honor request -> {target:#x}");
                let _ = self.net.0.send(ClientCommand::InspectHonorStats { target });
                slot.asked();
                false
            }
        };
        // The dropped reply's repaint, the slot's `Deviation:` ([`crate::ui_honor::InspectHonor`]),
        // fired once the call returns, with the host back.
        dropped.then(|| -> benilla_ui::script::CallTail {
            Box::new(|script: &mut UiScript| script.fire_event("INSPECT_HONOR_UPDATE", Vec::new()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR;
    use bevy::ecs::system::RunSystemOnce;

    /// A world holding one player wearing a helm of `entry` with `extra` fields, its tables empty.
    fn world(entry: u32, extra: &[(u16, u32)]) -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<Items>();
        world.init_resource::<crate::names::NameCache>();
        let mut pairs = vec![(2, 0x19), (FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2, entry)];
        pairs.extend_from_slice(extra);
        let e = world
            .spawn(ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(&pairs)
                    .into_created(benilla_protocol::messages::ObjectType::Player),
            ))
            .id();
        (world, e)
    }

    /// The helm's view as the tables build it now.
    fn helm(world: &mut World, e: Entity) -> InvSlotView {
        world
            .run_system_once(
                move |tables: VisibleItemTables, stores: Query<&ObjectStore>| {
                    tables.slots(stores.get(e).unwrap())[1]
                        .clone()
                        .expect("the helm")
                },
            )
            .unwrap()
    }

    /// The visible item's link carries the slot's permanent enchant, the block's `+0xc`, which the
    /// getter hands the link builder (`0x4c8d60` → `0x52adb0`), beside the suffix at `+0x28`.
    #[test]
    fn an_inspected_link_carries_the_permanent_enchant() {
        const HELM: u32 = 7366;
        let (mut world, e) = world(HELM, &[(FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 3, 1900)]);
        world
            .resource_mut::<Items>()
            .insert_template(HELM, Some(crate::items::test_template("Helm")));
        let link = helm(&mut world, e).link.expect("a link");
        assert!(link.contains("|Hitem:7366:1900:0:0|h"), "{link}");
    }

    /// A slot's creator is the first two words of its visible-item block, which the tooltip copies
    /// to its `+0x410` (`0x5333cd`-`0x5333d5`) for the maker line (`0x52e1d5`), and which vmangos
    /// fills from the item's creator (`Player.cpp:10589`). The view carries the guid, and the
    /// name once the name cache holds it, read as the view is built.
    #[test]
    fn an_inspected_slot_carries_its_creator() {
        const SMITH: u64 = 0x77;
        const ROBE: u32 = 7366;
        let (mut world, e) = world(
            ROBE,
            &[
                (FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR, SMITH as u32),
                (
                    FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 1,
                    (SMITH >> 32) as u32,
                ),
            ],
        );
        world
            .resource_mut::<Items>()
            .insert_template(ROBE, Some(crate::items::test_template("Robe")));
        let v = helm(&mut world, e);
        assert_eq!((v.creator_guid, v.creator), (SMITH, None));
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(SMITH, "Smith".into(), None);
        assert_eq!(helm(&mut world, e).creator.as_deref(), Some("Smith"));
    }
}
