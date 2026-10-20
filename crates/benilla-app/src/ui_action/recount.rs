//! The action bar's item recount, `0x4e5c70(entry)`, for each entry an item watcher passes it
//! ([`ItemNotice::recounts`]): the entry is counted once (`0x4e5c75`), and every item slot holding
//! it, in slot order, stores that count (`0x4e5caa`) and signals `ACTIONBAR_SLOT_CHANGED` with no
//! change test (`0x4e5cb1` into `0x4e58e0`). An item slot's count moves only here and where a
//! slot is written (`0x4e5d90`, `0x4e66f1`), never by polling the bags.

use bevy::prelude::*;

use benilla_protocol::messages::ACTION_KIND_ITEM;
use benilla_ui::script::{ScriptValue, UiScript};

use crate::item_watch::ItemNotice;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer, WorldEnterCascadeMessage};
use crate::query_cache::Ask;
use crate::ui_items::action_count;

/// The bar's 120 slots, as Lua action ids.
const ACTION_SLOTS: std::ops::RangeInclusive<u32> = 1..=120;

/// Run our own player's arrival pass, if it arrived, then the recounts the frame's item notices
/// owe, in their order.
pub(crate) fn recount_item_actions(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut notices: MessageReader<ItemNotice>,
    mut arrivals: MessageReader<WorldEnterCascadeMessage>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
) {
    hosted_vm.run(|script| {
        let arrived = arrivals.read().count() > 0;
        let notices: Vec<ItemNotice> = notices.read().cloned().collect();
        let Some(script) = script else {
            return;
        };
        let store = self_q.iter().next();
        let counter = Counter {
            store,
            objects: &objects,
            items: &items,
            commands: &commands,
        };
        if arrived {
            arrival_counts(script, &counter);
        }
        for entry in notices.iter().flat_map(ItemNotice::recounts) {
            recount(script, &counter, entry);
        }
    })
}

/// Our own player's arrival (`0x5dea50`, its manager flag `+0xc8` always clear) writes the bar's
/// 120 words through `0x4e5d60` with no send and quiet set (`0x5deafc`, `0x5deafe`), so each item
/// word is recounted (`0x4e5d90`) with no signal, then signals `ACTIONBAR_SLOT_CHANGED` once with
/// 0 (`0x5deb20`). Nothing else recounts a worldport: its items are created again before our
/// player, outside the world latch, and run no record set-up.
fn arrival_counts(script: &mut UiScript, counter: &Counter) {
    let mut counts = std::collections::HashMap::new();
    for action in ACTION_SLOTS {
        let Some(mut slot) = script
            .action_slot(action)
            .filter(|s| s.kind == ACTION_KIND_ITEM)
        else {
            continue;
        };
        slot.count = *counts
            .entry(slot.action)
            .or_insert_with(|| counter.count(slot.action));
        script.set_action(action, Some(slot));
    }
    script.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(0)]);
}

/// What `0x4e6d20` reads: the active player, the item objects and the template cache.
struct Counter<'a> {
    store: Option<&'a ObjectStore>,
    objects: &'a Objects<'a, 'a>,
    items: &'a Items,
    commands: &'a NetCommands,
}

impl Counter<'_> {
    /// `0x4e6d20(entry)`: 0 without an active player, else [`action_count`] over the entry's
    /// template, requested here as the reference does (`0x4e6d78`).
    fn count(&self, entry: u32) -> u32 {
        self.store.map_or(0, |s| {
            let template = self.items.template(entry, 0, Ask::Event(self.commands));
            action_count(&s.0, self.objects, entry, template)
        })
    }
}

/// `0x4e5c70(entry)`: no active player counts 0 (`0x4e6d20`). Each slot is read as the VM holds it
/// when its turn comes, so a handler that rewrites a later slot is seen.
fn recount(script: &mut UiScript, counter: &Counter, entry: u32) {
    let count = counter.count(entry);
    for action in ACTION_SLOTS {
        let Some(mut slot) = script
            .action_slot(action)
            .filter(|s| s.kind == ACTION_KIND_ITEM && s.action == entry)
        else {
            continue;
        };
        slot.count = count;
        script.set_action(action, Some(slot));
        script.fire_event(
            "ACTIONBAR_SLOT_CHANGED",
            vec![ScriptValue::Int(i64::from(action))],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY};
    use benilla_protocol::ObjectFields;
    use benilla_ui::script::ActionSlot;

    const ME: u64 = 0x7;
    const POTIONS: u64 = 0x4000_0000_0000_0101;
    const POTION: u32 = 118;
    const ARROW: u32 = 2512;
    /// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = 532;

    /// The recount over a VM whose bar holds the potion on slots 50 and 3 and an arrow on 7,
    /// with five potions in backpack slot 1, and a spy on `ACTIONBAR_SLOT_CHANGED`.
    fn app() -> App {
        let mut app = App::new();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.add_message::<ItemNotice>()
            .add_message::<WorldEnterCascadeMessage>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, recount_item_actions);
        app.insert_non_send_resource(rx); // a send must not fail
        let world = app.world_mut();
        crate::items::test_spawn_item(
            world,
            POTIONS,
            ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, POTION), (FIELD_ITEM_STACK_COUNT, 5)]),
            false,
        );
        world.spawn((
            SelfPlayer,
            crate::net::Guid(ME),
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, POTIONS as u32),
                (F_PACK_SLOT_1 + 1, (POTIONS >> 32) as u32),
            ])),
        ));
        let mut script = UiScript::new().unwrap();
        for (action, entry) in [(50, POTION), (3, POTION), (7, ARROW)] {
            script.set_action(
                action,
                Some(ActionSlot {
                    kind: ACTION_KIND_ITEM,
                    action: entry,
                    ..Default::default()
                }),
            );
        }
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("ACTIONBAR_SLOT_CHANGED")
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, arg1 .. "=" .. GetActionCount(arg1))
                end)
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    fn seen(app: &mut App) -> Vec<String> {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("local s = SEEN SEEN = {} return s")
            .unwrap()
    }

    fn notify(app: &mut App, notices: impl IntoIterator<Item = ItemNotice>) {
        app.world_mut()
            .resource_mut::<Messages<ItemNotice>>()
            .write_batch(notices);
        app.update();
    }

    /// `0x4e5c70`: the entry's slots in slot order, each with the count it stores, and the same
    /// count again signals again.
    #[test]
    fn a_notice_signals_every_slot_of_its_entry_in_slot_order_with_no_change_test() {
        let mut app = app();
        notify(
            &mut app,
            [ItemNotice::StackCount {
                guid: POTIONS,
                entry: POTION,
            }],
        );
        assert_eq!(seen(&mut app), ["3=5", "50=5"]);
        notify(
            &mut app,
            [ItemNotice::Enchantment {
                guid: POTIONS,
                entry: POTION,
            }],
        );
        assert_eq!(
            seen(&mut app),
            ["3=5", "50=5"],
            "nothing moved, both signal again"
        );
        // A seat change recounts the entry that left, then the one that came.
        notify(
            &mut app,
            [ItemNotice::Seat {
                old: Some(ARROW),
                new: Some(POTION),
            }],
        );
        assert_eq!(seen(&mut app), ["7=0", "3=5", "50=5"]);
    }

    /// The recount looks the entry up with the callback `0x4e5b80` (`0x4e6d78`) at each notice: a
    /// refused entry is asked by no frame after it, only by the next notice.
    #[test]
    fn a_refused_entry_is_asked_by_the_next_notice_never_by_a_frame() {
        let mut app = app();
        let asked = |app: &App| {
            crate::query_cache::queries(
                app.world()
                    .non_send_resource::<crossbeam_channel::Receiver<crate::net::ClientCommand>>(),
            )
            .len()
        };
        let notice = ItemNotice::StackCount {
            guid: POTIONS,
            entry: POTION,
        };
        notify(&mut app, [notice.clone()]);
        assert_eq!(asked(&app), 1);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(POTION, None);
        for _ in 0..600 {
            app.update();
        }
        assert_eq!(asked(&app), 0, "no frame asks a refusal");
        notify(&mut app, [notice]);
        assert_eq!(asked(&app), 1, "the next notice asks again");
    }

    /// The count is taken at the notice: a stack the store changes runs nothing by itself, and
    /// the next notice of the entry stores what the bags hold then.
    #[test]
    fn the_count_is_stored_at_the_notice() {
        let mut app = app();
        let potions = app.world().resource::<crate::net::GuidIndex>().0[&POTIONS];
        app.world_mut()
            .get_mut::<ObjectStore>(potions)
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 2)]));
        app.update();
        app.update();
        assert!(seen(&mut app).is_empty(), "no notice, no signal");
        let count = |app: &mut App| {
            app.world_mut()
                .non_send_resource::<UiScript>()
                .eval::<i64>("return GetActionCount(3)")
                .unwrap()
        };
        assert_eq!(count(&mut app), 0, "the slot keeps what it last stored");
        notify(
            &mut app,
            [ItemNotice::Destroyed {
                guid: 0x4000_0000_0000_0999,
                entry: POTION,
            }],
        );
        assert_eq!(seen(&mut app), ["3=2", "50=2"]);
        assert_eq!(count(&mut app), 2);
    }

    /// `0x4e6d20`: the recount walks mask 0, so a copy worn in the ranged slot (player slot 17)
    /// adds to the bag's, and a template whose first charges are neither 0 nor -1 stores the
    /// summed absolute charges instead of the stacks.
    #[test]
    fn a_recount_stores_worn_stacks_and_then_charges() {
        use benilla_protocol::field::FIELD_ITEM_SPELL_CHARGES;
        const WORN: u64 = 0x4000_0000_0000_0102;
        /// Player slot 17, the ranged slot.
        const F_RANGED: u16 = 486 + 2 * 17;
        let mut app = app();
        let world = app.world_mut();
        crate::items::test_spawn_item(
            world,
            WORN,
            ObjectFields::from_pairs(&[
                (FIELD_OBJECT_ENTRY, POTION),
                (FIELD_ITEM_STACK_COUNT, 2),
                (FIELD_ITEM_SPELL_CHARGES, (-4i32) as u32),
            ]),
            false,
        );
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(world)
            .unwrap();
        world
            .get_mut::<ObjectStore>(me)
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(&[
                (F_RANGED, WORN as u32),
                (F_RANGED + 1, (WORN >> 32) as u32),
            ]));
        let stacked = || ItemNotice::StackCount {
            guid: POTIONS,
            entry: POTION,
        };
        notify(&mut app, [stacked()]);
        assert_eq!(seen(&mut app), ["3=7", "50=7"], "5 in the pack and 2 worn");

        let mut template = crate::items::test_template("Elixir");
        template.spell_charges_0 = -2;
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(POTION, Some(template));
        notify(&mut app, [stacked()]);
        assert_eq!(
            seen(&mut app),
            ["3=4", "50=4"],
            "the worn copy's |-4| charges and the pack copy's none"
        );
    }

    /// Our player's arrival stores every item slot's count with no per-slot signal, then signals
    /// slot 0 once (`0x5dea50`): a worldport's re-created bags recount here and nowhere else.
    #[test]
    fn our_arrival_recounts_every_item_slot_quietly_and_signals_slot_zero() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<Messages<WorldEnterCascadeMessage>>()
            .write(WorldEnterCascadeMessage);
        app.update();
        assert_eq!(seen(&mut app), ["0=0"], "one signal, slot 0");
        let counts: (i64, i64, i64) = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval("return GetActionCount(3), GetActionCount(50), GetActionCount(7)")
            .unwrap();
        assert_eq!(counts, (5, 5, 0));
    }
}
