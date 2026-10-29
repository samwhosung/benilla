//! The action bar's item recount, `0x4e5c70(entry)`, for each entry an item watcher passes it
//! ([`ItemNotice::recounts`]) and for the bar's own record callback (`0x4e5b80`), run in its
//! watcher's run on the item lane ([`crate::item_lane`]): the entry is counted once (`0x4e5c75`),
//! and every item slot holding it, in slot order, stores that count (`0x4e5caa`) and signals
//! `ACTIONBAR_SLOT_CHANGED` with no change test (`0x4e5cb1` into `0x4e58e0`). An item slot's count
//! moves only there and where a slot is written (`0x4e5d90`, `0x4e66f1`), never by polling the
//! bags. The counts are the host's ([`super::ActionStateMemory`]), which the getter reads at the
//! call (`0x4e6c70`).
//!
//! [`ItemNotice::recounts`]: crate::item_watch::ItemNotice::recounts

use bevy::prelude::*;

use benilla_ui::script::ScriptValue;

/// The bar's 120 slots, as Lua action ids.
const ACTION_SLOTS: std::ops::RangeInclusive<u32> = 1..=120;

/// Our own player's arrival pass over the bar, run in the arrival's place
/// ([`crate::ui_world_enter`]): the 120 words written through `0x4e5d60` with no send and quiet
/// set (`0x5deafc`, `0x5deafe`), into the VM's copy, so each item word is recounted (`0x4e5d90`)
/// and each slot's usable pair stored (`0x4e58e0`), with no signal
/// ([`super::ActionStates::arrival_slots`]), then `ACTIONBAR_SLOT_CHANGED` once with 0
/// (`0x5deb20`), with the host lent.
pub(crate) fn arrival_slot_pass(mut vm: crate::script_calls::VmWith<()>) {
    vm.engine_arrival_slots();
    vm.run(|script| {
        if let Some(script) = script {
            script.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(0)]);
        }
    });
}

/// `0x4e5c70(entry)`: the entry counted once through the host, no active player counting 0
/// (`0x4e6d20`). Each slot is read as the bar holds it when its turn comes, so a handler that
/// rewrites a later slot is seen. Each slot's write is `0x4e58e0`'s: the count stored
/// (`0x4e5caa`), then the slot's usable pair (`0x4e5903`-`0x4e5914`), then
/// `ACTIONBAR_SLOT_CHANGED` (`0x4e5998`) with the host lent. A VM with no host to count through
/// signals the slots its copy holds the entry in; with no VM nothing is read.
pub(crate) fn recount<P: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut crate::script_calls::VmWith<P>,
    entry: u32,
) {
    let count = vm.engine_item_count(entry);
    for action in ACTION_SLOTS {
        let held = match count {
            Some(count) => vm.engine_recounted(action, entry, count),
            None => vm.run(|script| {
                script.and_then(|s| s.action_slot(action)).is_some_and(|s| {
                    s.kind == benilla_protocol::messages::ACTION_KIND_ITEM && s.action == entry
                })
            }),
        };
        if !held {
            continue;
        }
        vm.run(|script| {
            if let Some(script) = script {
                script.fire_event(
                    "ACTIONBAR_SLOT_CHANGED",
                    vec![ScriptValue::Int(i64::from(action))],
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_watch::ItemNotice;
    use crate::items::Items;
    use crate::net::{ObjectStore, SelfPlayer};
    use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY};
    use benilla_protocol::messages::{ActionButton, ACTION_KIND_ITEM};
    use benilla_protocol::ObjectFields;
    use benilla_ui::script::{ActionSlot, UiScript};

    const ME: u64 = 0x7;
    const POTIONS: u64 = 0x4000_0000_0000_0101;
    const POTION: u32 = 118;
    const ARROW: u32 = 2512;
    /// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = 532;

    /// The recount over the client's world, whose bar holds the potion on slots 50 and 3 and an
    /// arrow on 7, with five potions in backpack slot 1, and a spy on `ACTIONBAR_SLOT_CHANGED`.
    fn app() -> App {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        app.insert_non_send_resource(rx); // a send must not fail
        let world = app.world_mut();
        world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
        crate::items::test_spawn_item(
            world,
            POTIONS,
            ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, POTION), (FIELD_ITEM_STACK_COUNT, 5)]),
            false,
        );
        let me = world
            .spawn((
                SelfPlayer,
                crate::net::Guid(ME),
                ObjectStore(ObjectFields::from_pairs(&[
                    (F_PACK_SLOT_1, POTIONS as u32),
                    (F_PACK_SLOT_1 + 1, (POTIONS >> 32) as u32),
                ])),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(ME, me);
        let mut script = UiScript::new().unwrap();
        for (action, entry) in [(50, POTION), (3, POTION), (7, ARROW)] {
            let slot = (action - 1) as u8;
            world
                .resource_mut::<crate::ui_action::ActionSlots>()
                .buttons
                .insert(
                    slot,
                    ActionButton {
                        slot,
                        action: entry,
                        kind: ACTION_KIND_ITEM,
                    },
                );
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

    /// Each notice's run on the item lane, with no signals, fired.
    fn notify(app: &mut App, notices: impl IntoIterator<Item = ItemNotice>) {
        let world = app.world_mut();
        for notice in notices {
            world
                .resource_mut::<crate::item_lane::ItemLane>()
                .push(crate::item_watch::Run {
                    notice: Some(notice),
                    signals: Vec::new(),
                    ..Default::default()
                });
        }
        crate::item_lane::fire_item_lane(app.world_mut());
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
        assert_eq!(asked(&app), 0, "nothing asks a refusal by itself");
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
        assert!(seen(&mut app).is_empty(), "no notice, no signal");
        // Through the host, which holds the bar's counts.
        let count = |app: &mut App| -> i64 {
            crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
                s.expect("a VM").eval("return GetActionCount(3)").unwrap()
            })
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

    /// Our player's arrival loads the staged words, stores every item slot's count with no
    /// per-slot signal, then signals slot 0 once (`0x5dea50`): a worldport's re-created bags
    /// recount here and nowhere else.
    #[test]
    fn our_arrival_recounts_every_item_slot_quietly_and_signals_slot_zero() {
        let mut app = app();
        {
            let mut slots = app
                .world_mut()
                .resource_mut::<crate::ui_action::ActionSlots>();
            slots.staged = std::mem::take(&mut slots.buttons);
        }
        app.world_mut()
            .run_system_cached(arrival_slot_pass)
            .expect("the arrival's slot pass runs");
        assert_eq!(seen(&mut app), ["0=0"], "one signal, slot 0");
        let chunk = "return GetActionCount(3), GetActionCount(50), GetActionCount(7)";
        let live: (i64, i64, i64) = crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM").eval(chunk).unwrap()
        });
        assert_eq!(live, (5, 5, 0));
        let mirror: (i64, i64, i64) = crate::script_calls::eval_in_vm(app.world(), chunk);
        assert_eq!(mirror, (5, 5, 0), "the VM's copy with them");
    }
}
