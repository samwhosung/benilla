//! The inspect feed: each held player's public descriptor as the slot views the `GetInventoryItem*`
//! verbs read through a token naming them. Their inventory guids are server-private, so there are
//! no item objects, only the `PLAYER_VISIBLE_ITEM_*` entries and their templates, which is why the
//! reference's inspect doll shows no counts and no durability.
//!
//! `NotifyInspect(unit)` sends `CMSG_INSPECT`, which also sets the server-side selection, and
//! latches the token; `SMSG_INSPECT` echoes only the guid (`0x5e7d70`). The window paints inside
//! the call that shows it (`InspectPaperDollFrame.lua:57-79`), off what the VM already holds, so
//! nothing here signals: a change of the gear, as of the level, is the unit-field bridge's
//! ([`crate::field_watch`]), fired per token on a values block.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_ui::script::{InvSlotView, InventorySlots};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore};
use crate::query_cache::Ask;
use crate::ui_script::{gate, UiFeed};

/// The reference's `InspectFrame.unit`, latched by `NotifyInspect`: the token the honor feed asks
/// `MSG_INSPECT_HONOR_STATS` for.
#[derive(Resource, Default)]
pub(crate) struct InspectTarget {
    pub(crate) token: Option<String>,
}

/// The views pushed, per VM ([`crate::ui_script::VmMemo`]), so a `/reload`'s new VM gets every
/// held player's view again.
#[derive(Resource, Default)]
pub(crate) struct InspectFeedState {
    vm: crate::ui_script::VmMemo<InspectFeedMemo>,
}

/// The per-VM change bases.
#[derive(Default)]
struct InspectFeedMemo {
    /// What was last pushed, by player guid.
    pushed: HashMap<u64, Pushed>,
    templates: gate::Watch,
    /// How many players' views were built, the feed's cost.
    builds: u64,
}

/// One player's last push and the visible-item fields it was built from.
struct Pushed {
    block: VisibleBlock,
    slots: InventorySlots,
}

/// The fields of a player's visible-item block (`PLAYER_VISIBLE_ITEM_1_CREATOR` on, 12 a slot)
/// that its views read: per slot the entry, the 7 enchants and the properties.
type VisibleBlock = [u32; 19 * 9];

fn visible_block(store: &ObjectStore) -> VisibleBlock {
    let mut block = [0; 19 * 9];
    for slot0 in 0..19u8 {
        let at = usize::from(slot0) * 9;
        block[at] = store.0.player_visible_item_entry(slot0).unwrap_or(0);
        for j in 0..7u8 {
            block[at + 1 + usize::from(j)] =
                store.0.player_visible_item_enchant(slot0, j).unwrap_or(0);
        }
        block[at + 8] = store.0.player_visible_item_properties(slot0);
    }
    block
}

pub(crate) struct InspectUiPlugin;

impl Plugin for InspectUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InspectTarget>()
            .init_resource::<InspectFeedState>()
            // After the aura feed seats the held guids, and ahead of the events whose handlers
            // read these slots: the field events' `UNIT_INVENTORY_CHANGED`, and the unit events'
            // `PLAYER_TARGET_CHANGED`, which re-shows the window on the new target
            // (`Blizzard_InspectUI.lua:29-31`).
            .add_systems(
                Update,
                feed_inspect
                    .in_set(UiFeed)
                    .in_set(crate::ui_unit::field_events::FieldEventPushes)
                    .after(crate::ui_aura::AuraEvents)
                    .before(crate::ui_unit::UnitEvents),
            );
    }
}

/// One slot from a player's visible-item entry; `slot0` is the 0-based `EQUIPMENT_SLOT_*` index,
/// `slot0 + 1` in Lua. With no item object the count is the reference's always-1 and durability,
/// flags, locks and creator stay inert. Only the enchants broadcast, at
/// `PLAYER_VISIBLE_ITEM_<slot>_0 + 1 + j`, and 1.12 fills PERM and TEMP alone (vmangos
/// `SetVisibleItemSlot`, `MAX_INSPECTED_ENCHANTMENT_SLOT`). The record is read, never asked: the
/// getters' lookup passes no callback (`0x4c8420` → `0x5f0d60` → `0x62eb40`).
fn inspect_slot_view(
    store: &ObjectStore,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    slot0: u8,
) -> Option<InvSlotView> {
    let entry = store.0.player_visible_item_entry(slot0)?;
    // The roll, the properties field's low word as the reference's inspect leg reads it into
    // `+0x424`, names the suffix only: its stat lines would sit in enchant slots 2-6, which 1.12
    // servers leave empty.
    let roll = store.0.player_visible_item_properties(slot0);
    // By entry alone (guid 0), as the reference's item cache asks: there is no item object.
    let t = items.template(entry, 0, Ask::Read);
    let (name, quality, display) = match t {
        Some(t) => (
            Some(rolls.name(&t.name, roll)),
            t.quality,
            t.display_info_id,
        ),
        None => (None, 0, 0),
    };
    // The link names the permanent enchant, the block's `+0xc`, and the suffix at `+0x28`
    // (`0x4c8d5c`-`0x4c8d74` → `0x52adb0`).
    let perm = store.0.player_visible_item_enchant(slot0, 0).unwrap_or(0);
    let link = name
        .as_ref()
        .map(|n| crate::ui_items::item_link_full(entry, perm, roll, 0, n, quality));
    Some(InvSlotView {
        item_id: entry,
        icon: icons
            .and_then(|i| i.catalog.get(display))
            .and_then(|d| d.icon.clone()),
        count: 1,
        quality: quality as i32,
        inventory_type: t.map(|t| t.inventory_type),
        name,
        link,
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
/// and the `SpellItemEnchantment` and `ItemRandomProperties` catalogs that name the enchant lines
/// and the roll suffix.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct VisibleItemTables<'w> {
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
}

impl VisibleItemTables<'_> {
    /// A player's 19 slots by slot id.
    pub(crate) fn slots(&self, store: &ObjectStore) -> InventorySlots {
        let rolls = crate::items::RollCatalogs {
            enchants: self.enchants.as_deref(),
            props: self.props.as_deref(),
        };
        let mut slots: InventorySlots = Default::default();
        for slot in 1..=19u8 {
            slots[usize::from(slot)] =
                inspect_slot_view(store, &self.items, self.icons.as_deref(), rolls, slot - 1);
        }
        slots
    }

    /// Whether a landed record, a new icon or a new catalog can have changed any view since
    /// `templates` last watched.
    fn moved(&self, templates: &mut gate::Watch) -> bool {
        let templates_moved = templates.moved(self.items.template_epoch());
        templates_moved
            || self.icons.as_ref().is_some_and(|i| i.is_changed())
            || self.enchants.as_ref().is_some_and(|c| c.is_changed())
            || self.props.as_ref().is_some_and(|c| c.is_changed())
    }
}

pub(crate) fn feed_inspect(
    // What the host also reaches, read before the VM runs with the host lent.
    mut hosted: ParamSet<(Res<crate::target::Selection>, crate::script_calls::HostedVm)>,
    mut target: ResMut<InspectTarget>,
    mut feed: ResMut<InspectFeedState>,
    tables: VisibleItemTables,
    commands: Res<NetCommands>,
    index: Res<GuidIndex>,
    stores: Query<Ref<ObjectStore>>,
    group: Res<crate::ui_party::GroupState>,
    me: Res<crate::net::SelfGuid>,
) {
    let p = hosted.p0();
    let selection = *p;
    hosted.p1().run(|script| {
        let Some(script) = script else {
            return;
        };
        let memo = feed.vm.get(script);

        // `NotifyInspect(unit)`: send `CMSG_INSPECT` for the unit's guid and latch its token.
        for token in script.take_inspect_notifies() {
            match crate::ui_unit::player_token_guid(&token, &selection, &group) {
                Some(guid) => {
                    debug!("inspect: {token:?} -> {guid:#x}; sending CMSG_INSPECT");
                    let _ = commands.0.send(ClientCommand::Inspect { target: guid });
                    target.token = Some(token);
                }
                None => {
                    debug!("inspect: {token:?} did not resolve to a player guid — nothing sent");
                }
            }
        }
        // `ClearInspectPlayer()`, from `InspectFrame_OnHide`.
        if script.take_inspect_clear() {
            target.token = None;
        }

        let tables_moved = tables.moved(&mut memo.templates);
        // Every player a token can name, but the active one, whose own items the getters read.
        let mut held = HashSet::new();
        for guid in script.held_unit_guids() {
            if Some(guid) == me.0 {
                continue;
            }
            let Some(store) = index.0.get(&guid).and_then(|&e| stores.get(e).ok()) else {
                continue;
            };
            if store.0.object_type() != Some(benilla_protocol::messages::ObjectType::Player) {
                continue;
            }
            held.insert(guid);
            // Rebuilt when the views' fields moved, not on any descriptor write.
            let last = memo.pushed.get(&guid);
            if last.is_some() && !tables_moved && !store.is_changed() {
                continue;
            }
            let block = visible_block(&store);
            if !tables_moved && last.is_some_and(|p| p.block == block) {
                continue;
            }
            let slots = tables.slots(&store);
            memo.builds += 1;
            if last.map(|p| &p.slots) != Some(&slots) {
                script.set_visible_equipment(guid, Some(slots.clone()));
            }
            memo.pushed.insert(guid, Pushed { block, slots });
        }
        memo.pushed.retain(|&guid, _| {
            let keep = held.contains(&guid);
            if !keep {
                script.set_visible_equipment(guid, None);
            }
            keep
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::UiScript;
    use bevy::ecs::system::RunSystemOnce;

    /// A world holding the players `guids` with a helm of `entry` showing, the VM seated with
    /// `"target"` and `"party1"` on the first two, both cooperative and 4 yards away, and a frame
    /// logging `UNIT_INVENTORY_CHANGED`.
    fn world(guids: [u64; 2], entry: u32) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        use benilla_protocol::field::FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR;
        let mut app = App::new();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx))
            .init_resource::<InspectTarget>()
            .init_resource::<InspectFeedState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::net::SelfGuid>()
            .insert_resource(crate::target::Selection {
                guid: Some(guids[0]),
                ..Default::default()
            })
            .init_resource::<crate::ui_party::GroupState>()
            .add_systems(Update, feed_inspect);
        for guid in guids {
            let e = app
                .world_mut()
                .spawn(ObjectStore(
                    // `OBJECT_FIELD_TYPE`: object, unit, player.
                    benilla_protocol::ObjectFields::from_pairs(&[
                        (2, 0x19),
                        (FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2, entry),
                    ])
                    .into_created(benilla_protocol::messages::ObjectType::Player),
                ))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
        }
        let mut script = UiScript::new().unwrap();
        script.set_unit_guids(&benilla_ui::script::UnitGuids {
            target: guids[0],
            party: [guids[1], 0, 0, 0],
            held: guids.iter().map(|&g| (g, 0)).collect(),
            ..Default::default()
        });
        script.set_unit_reach(
            guids
                .iter()
                .map(|&g| {
                    let reach = benilla_ui::script::UnitReach {
                        dist_sq: 16.0,
                        inspectable: true,
                    };
                    (g, reach)
                })
                .collect(),
        );
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_INVENTORY_CHANGED")
                f:SetScript("OnEvent", function() table.insert(SEEN, arg1) end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    /// The slot getters' visible-item lookups pass no callback (`0x4c8420` → `0x5f0d60` →
    /// `0x62eb40`), and a lookup with none sends nothing (`0x55ba30`): neither an inspect nor any
    /// frame asks the record, which the model's own display asks (`0x5ed764`).
    #[test]
    fn the_visible_item_lookups_ask_nothing() {
        const MATE: u64 = 0x2001;
        let (mut app, rx) = world([MATE, 0x2002], 955);
        let asked = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            crate::query_cache::queries(rx)
                .into_iter()
                .filter(|c| matches!(c, ClientCommand::ItemQuery { entry: 955, .. }))
                .count()
        };
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(r#"NotifyInspect("target")"#)
            .unwrap();
        for _ in 0..10 {
            app.update();
        }
        assert_eq!(asked(&rx), 0);
    }

    /// Every held player's visible items are in the VM before any call reads them, so the show
    /// paints inside the call that latched the token (`InspectPaperDollFrame.lua:57-79`) and no
    /// feed signals: a change of the gear is the visible-item bridge's (`0x51bd70`), and a record
    /// landing repaints nothing (`0x5ed880`).
    #[test]
    fn the_feed_pushes_each_held_players_visible_items_and_fires_nothing() {
        const MATE: u64 = 0x2001;
        const OTHER: u64 = 0x2002;
        const HELM: u32 = 7366;
        let (mut app, _rx) = world([MATE, OTHER], HELM);
        let link = |app: &mut App, token: &str| {
            app.world_mut().run_system_once(feed_inspect).unwrap();
            app.world_mut()
                .non_send_resource::<UiScript>()
                .eval::<Option<String>>(&format!("return GetInventoryItemLink({token:?}, 1)"))
                .unwrap()
        };
        assert_eq!(link(&mut app, "target"), None, "the record is not cached");
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(HELM, Some(crate::items::test_template("Helm")));
        for token in ["target", "party1"] {
            assert!(
                link(&mut app, token).is_some_and(|l| l.contains("[Helm]")),
                "{token}"
            );
        }
        let s = app.world_mut().non_send_resource::<UiScript>();
        assert!(s.eval::<Vec<String>>("return SEEN").unwrap().is_empty());
    }

    /// The visible item's link carries the slot's permanent enchant, the block's `+0xc`, which the
    /// getter hands the link builder (`0x4c8d60` → `0x52adb0`), beside the suffix at `+0x28`.
    #[test]
    fn an_inspected_link_carries_the_permanent_enchant() {
        use benilla_protocol::field::FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR;
        const MATE: u64 = 0x2001;
        const HELM: u32 = 7366;
        let (mut app, _rx) = world([MATE, 0x2002], HELM);
        let mate = app.world().resource::<GuidIndex>().0[&MATE];
        app.world_mut()
            .entity_mut(mate)
            .get_mut::<ObjectStore>()
            .unwrap()
            .0
            .merge(benilla_protocol::ObjectFields::from_pairs(&[(
                FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 3,
                1900,
            )]));
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(HELM, Some(crate::items::test_template("Helm")));
        app.update();
        let link = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval::<String>(r#"return GetInventoryItemLink("target", 1)"#)
            .unwrap();
        assert!(link.contains("|Hitem:7366:1900:0:0|h"), "{link}");
    }

    /// A player's 19 views are rebuilt when their visible-item block moves, not on any write to
    /// the descriptor: a health tick builds nothing, a new helm builds that player's again.
    #[test]
    fn only_a_visible_item_write_rebuilds_a_players_views() {
        use benilla_protocol::field::FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR;
        const MATE: u64 = 0x2001;
        /// `UNIT_FIELD_HEALTH`.
        const HEALTH: u16 = 22;
        let (mut app, _rx) = world([MATE, 0x2002], 7366);
        let builds = |app: &mut App| {
            app.update();
            app.world_mut()
                .resource_scope(|world, mut feed: Mut<InspectFeedState>| {
                    feed.vm.get(world.non_send_resource::<UiScript>()).builds
                })
        };
        assert_eq!(builds(&mut app), 2, "both players, first seen");
        let mate = app.world().resource::<GuidIndex>().0[&MATE];
        let write = |app: &mut App, field: u16, value: u32| {
            app.world_mut()
                .entity_mut(mate)
                .get_mut::<ObjectStore>()
                .unwrap()
                .0
                .merge(benilla_protocol::ObjectFields::from_pairs(&[(
                    field, value,
                )]));
        };
        write(&mut app, HEALTH, 50);
        assert_eq!(builds(&mut app), 2, "a health tick");
        write(&mut app, FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2, 7367);
        assert_eq!(builds(&mut app), 3, "a new helm");
    }

    /// The inspected unit's level is the bridge's to announce: the feed fires no `UNIT_LEVEL`,
    /// at the first push or on a change.
    #[test]
    fn the_inspect_feed_fires_no_level_event() {
        const MATE: u64 = 0x2001;
        const UNIT_FIELD_LEVEL: u16 = 34;
        let (mut app, _rx) = world([MATE, 0x2002], 7366);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"
                LEVELS = 0
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_LEVEL")
                f:SetScript("OnEvent", function() LEVELS = LEVELS + 1 end)
            "#,
            )
            .unwrap();
        let mate = app.world().resource::<GuidIndex>().0[&MATE];
        for level in [10, 11] {
            app.world_mut()
                .entity_mut(mate)
                .get_mut::<ObjectStore>()
                .unwrap()
                .0
                .merge(benilla_protocol::ObjectFields::from_pairs(&[(
                    UNIT_FIELD_LEVEL,
                    level,
                )]));
            app.world_mut().run_system_once(feed_inspect).unwrap();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            assert_eq!(s.eval::<i64>("return LEVELS").unwrap(), 0, "level {level}");
        }
    }
}
