//! The inspect feed: another player's public descriptor as the slot views the inspect window's
//! Lua reads. Their inventory guids are server-private, so there are no item objects, only the
//! `PLAYER_VISIBLE_ITEM_*` entries and their templates, which is why the reference's inspect doll
//! shows no counts and no durability.
//!
//! `NotifyInspect(unit)` sends `CMSG_INSPECT`, which also sets the server-side selection, and
//! latches the token; the window paints without waiting for `SMSG_INSPECT`, a bare guid echo.
//! `UNIT_INVENTORY_CHANGED` fires for the inspected token when a new inspectee's view first
//! lands (`InspectPaperDollFrame.lua:2-5`, `82`); a change of its gear, as its `UNIT_LEVEL`, is
//! the unit-field bridge's ([`crate::field_watch`]), fired per token on a values block.

use bevy::prelude::*;

use benilla_ui::script::{InspectView, InvSlotView, InventorySlots, ScriptValue};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore};
use crate::query_cache::Ask;
use crate::ui_script::UiFeed;

/// The reference's `InspectFrame.unit`. The token, not the guid, is the identity: the reference
/// re-reads it on every retarget, so `"target"` follows the selection.
#[derive(Resource, Default)]
pub(crate) struct InspectTarget {
    pub(crate) token: Option<String>,
}

/// The last view pushed, per VM ([`crate::ui_script::VmMemo`]), so a `/reload`'s
/// new VM gets the view re-pushed and its events re-fired.
#[derive(Resource, Default)]
pub(crate) struct InspectFeedState {
    vm: crate::ui_script::VmMemo<InspectFeedMemo>,
}

/// The per-VM change bases.
#[derive(Default)]
struct InspectFeedMemo {
    last: Option<InspectView>,
}

pub(crate) struct InspectUiPlugin;

impl Plugin for InspectUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InspectTarget>()
            .init_resource::<InspectFeedState>()
            // Ahead of the field events: the bridge's `UNIT_INVENTORY_CHANGED` for the inspected
            // token repaints the slots from this view.
            .add_systems(
                Update,
                feed_inspect
                    .in_set(UiFeed)
                    .in_set(crate::ui_unit::field_events::FieldEventPushes),
            );
    }
}

/// One inspected slot from the target's visible-item entry; `slot0` is the 0-based
/// `EQUIPMENT_SLOT_*` index, `slot0 + 1` in Lua. With no item object the count is the reference's
/// always-1 and durability, flags, locks and creator stay inert. Only the enchants broadcast, at
/// `PLAYER_VISIBLE_ITEM_<slot>_0 + 1 + j`, and 1.12 fills PERM and TEMP alone (vmangos
/// `SetVisibleItemSlot`, `MAX_INSPECTED_ENCHANTMENT_SLOT`).
fn inspect_slot_view(
    store: &ObjectStore,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    ask: Ask<'_>,
    slot0: u8,
) -> Option<InvSlotView> {
    let entry = store.0.player_visible_item_entry(slot0)?;
    // The roll, the properties field's low word as the reference's inspect leg reads it into
    // `+0x424`, names the suffix only: its stat lines would sit in enchant slots 2-6, which 1.12
    // servers leave empty.
    let roll = store.0.player_visible_item_properties(slot0);
    // By entry alone (guid 0), as the reference's item cache asks: there is no item object.
    let (name, quality, display) = match items.template(entry, 0, ask) {
        Some(t) => (
            Some(rolls.name(&t.name, roll)),
            t.quality,
            t.display_info_id,
        ),
        None => (None, 0, 0),
    };
    let link = name
        .as_ref()
        .map(|n| crate::ui_items::item_link_full(entry, 0, roll, 0, n, quality));
    Some(InvSlotView {
        item_id: entry,
        icon: icons
            .and_then(|i| i.catalog.get(display))
            .and_then(|d| d.icon.clone()),
        count: 1,
        quality: quality as i32,
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

pub(crate) fn feed_inspect(
    // What the host also reaches, read before the VM runs with the host lent.
    mut hosted: ParamSet<(Res<crate::target::Selection>, crate::script_calls::HostedVm)>,
    mut target: ResMut<InspectTarget>,
    mut feed: ResMut<InspectFeedState>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    // `SpellItemEnchantment` names the enchant lines, `ItemRandomProperties` the roll suffix.
    catalogs: (
        Option<Res<crate::items::Enchants>>,
        Option<Res<crate::items::RandomProperties>>,
    ),
    commands: Res<NetCommands>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    group: Res<crate::ui_party::GroupState>,
) {
    let p = hosted.p0();
    let selection = *p;
    hosted.p1().run(|script| {
        let Some(script) = script else {
            return;
        };
        // A `/reload` keeps the latched token, and the fresh memo re-pushes the view.
        let memo = feed.vm.get(script);

        // `NotifyInspect(unit)`: send `CMSG_INSPECT` for the unit's guid and latch its token.
        let mut notified = false;
        for token in script.take_inspect_notifies() {
            match crate::ui_unit::player_token_guid(&token, &selection, &group) {
                Some(guid) => {
                    debug!("inspect: {token:?} -> {guid:#x}; sending CMSG_INSPECT");
                    let _ = commands.0.send(ClientCommand::Inspect { target: guid });
                    target.token = Some(token);
                    notified = true;
                }
                None => {
                    debug!("inspect: {token:?} did not resolve to a player guid — nothing sent");
                }
            }
        }
        // `ClearInspectPlayer()`, from `InspectFrame_OnHide`: the resolve stops.
        if script.take_inspect_clear() {
            target.token = None;
        }

        // Re-resolve the latched token every frame, so the window follows a retarget as the
        // reference's `PLAYER_TARGET_CHANGED` arm does.
        let resolved = target.token.as_ref().and_then(|token| {
            let guid = crate::ui_unit::player_token_guid(token, &selection, &group)?;
            let entity = *index.0.get(&guid)?;
            Some((token.clone(), guid, entity))
        });

        let Some((token, guid, entity)) = resolved else {
            // Nothing inspected, or the target is not streamed: clear the view.
            if memo.last.is_some() {
                script.set_inspect(None);
                memo.last = None;
            }
            return;
        };

        let Ok(store) = stores.get(entity) else {
            return;
        };
        // The frame's show and a new inspectee are its slots' getter calls, whose lookups pass a
        // callback (`0x4c83ce`); the frames between only see a slot change, which the visible
        // item's own lookup asked (`0x5ed764`).
        let shown = notified || memo.last.as_ref().map(|v| v.guid) != Some(guid);
        let ask = Ask::at(shown, &commands);
        let mut slots: InventorySlots = Default::default();
        for slot in 1..=19u8 {
            slots[usize::from(slot)] = inspect_slot_view(
                store,
                &items,
                icons.as_deref(),
                crate::items::RollCatalogs {
                    enchants: catalogs.0.as_deref(),
                    props: catalogs.1.as_deref(),
                },
                ask,
                slot - 1,
            );
        }
        let view = InspectView {
            unit: token.clone(),
            guid,
            slots,
        };
        if memo.last.as_ref() != Some(&view) {
            script.set_inspect(Some(view.clone()));
            let inspectee = |v: &InspectView| (v.unit.clone(), v.guid);
            let fresh = memo.last.as_ref().map(inspectee) != Some(inspectee(&view));
            memo.last = Some(view);
            // The show paints the slots inside the call that latches the token
            // (`InspectPaperDollFrame.lua:57-79`), a frame ahead of this view, so a new
            // inspectee's first view signals its token, which the slot buttons filter on `arg1`.
            // A change of its gear is the visible-item bridge's (`0x51bd70`), and a record
            // landing repaints nothing (`0x5ed880`).
            if fresh {
                script.fire_event(
                    "UNIT_INVENTORY_CHANGED",
                    vec![ScriptValue::Str(token.clone())],
                );
            }
        }
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
                    benilla_protocol::ObjectFields::from_pairs(&[(
                        FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2,
                        entry,
                    )])
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

    /// The inspected unit's level is the bridge's to announce: the feed fires no `UNIT_LEVEL`,
    /// at the first resolve or on a change.
    #[test]
    fn the_inspect_feed_fires_no_level_event() {
        const MATE: u64 = 0x2001;
        const UNIT_FIELD_LEVEL: u16 = 34;
        let mut app = App::new();
        let (tx, rx) = crossbeam_channel::unbounded();
        std::mem::forget(rx);
        app.insert_resource(NetCommands(tx))
            .insert_resource(InspectTarget {
                token: Some("target".into()),
            })
            .init_resource::<InspectFeedState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
            .insert_resource(crate::target::Selection {
                guid: Some(MATE),
                ..Default::default()
            })
            .init_resource::<crate::ui_party::GroupState>();
        let mate = app
            .world_mut()
            .spawn(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (UNIT_FIELD_LEVEL, 10),
            ])))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(MATE, mate);
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = 0
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_LEVEL")
                f:SetScript("OnEvent", function() SEEN = SEEN + 1 end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
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
            assert_eq!(s.eval::<i64>("return SEEN").unwrap(), 0, "level {level}");
        }
    }
}
