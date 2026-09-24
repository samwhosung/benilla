//! The bag-slot verbs: `PutItemInBag`, `PutItemInBackpack` and `PickupBagFromSlot`.

use mlua::{Lua, MultiValue, Value};

use crate::script::container::ContainerMove;
use crate::script::Model;

use super::{clear_cursor, queue_cursor_update, queue_lock_changed, CursorItem, CursorPayload};
use super::{doll::pickup_inventory_item, EQUIPMENT_BAG};

/// The ids `PutItemInBag` and `PickupBagFromSlot` take: what their argument reader (`0x4c8520`)
/// accepts above each floor gate (`0x4c8f1f`, `0x4c8fbf`), so the equipped bags 20-23, the
/// bank's slots 40-63, its bag slots 64-69 and the keyring 82-113.
fn accepts(live: u32) -> bool {
    (20..=23).contains(&live)
        || (40..=63).contains(&live)
        || (64..=69).contains(&live)
        || (82..=113).contains(&live)
}

/// The live container id of a bag slot, the inverse of `ContainerIDToInventoryID`. Deviation:
/// an auto-store into a bank item or keyring slot sends nothing, because no container id names
/// it and no stock caller passes one; the reference sends it and the server refuses.
fn autostore_container(live: u32) -> Option<i64> {
    match live {
        20..=23 => Some(i64::from(live) - 19),
        64..=69 => Some(i64::from(live) - 59),
        _ => None,
    }
}

/// `Bag0Slot`, the first equipped bag slot: only a container names it in `equip_slots`.
const BAG0_SLOT: u8 = 20;

/// The six bank bag slots' ids (`BankButtonIDToInvSlotID(1..6, isBag)`).
pub(super) const BANK_BAG_INV_SLOTS: std::ops::RangeInclusive<u32> = 64..=69;

/// Whether the held item is a container, the reference's `TYPEMASK_CONTAINER` test (`0x4c7dc4`):
/// only `INVTYPE_BAG` maps to the bag slots in `FindEquipSlot`, so no other item names Bag0Slot.
fn is_container(item: &CursorItem) -> bool {
    item.equip_slots.contains(&BAG0_SLOT)
}

/// Whether the held item may go in doll slot `id`: its `equip_slots`, or a bank bag slot for a
/// container, which `FindEquipSlot` never names. The reference's fit check behind
/// `CursorCanGoInSlot` (`0x5ea720`) passes any item at a bank bag slot, as `IsValidForSlot`
/// answers 1 for every 0-based slot from 23 (`0x5da215`); the server's `CanBankItem` referees
/// either way.
pub(super) fn fits_slot(item: &CursorItem, id: u32) -> bool {
    if BANK_BAG_INV_SLOTS.contains(&id) {
        return is_container(item);
    }
    u8::try_from(id).is_ok_and(|id| item.equip_slots.contains(&id))
}

/// `PutItemInBag(inventorySlot)` (`0x4c7c00`): true (the reference's `1`) when the click was
/// consumed against an occupied slot, else false (nil). An empty cursor's nil is what lets
/// `BagSlotButton_OnClick` open the bag.
pub(super) fn put_item_in_bag(model: &mut Model, live: u32) -> bool {
    if !accepts(live) {
        return false;
    }
    // An empty or non-item cursor answers nil and sends nothing. At an occupied slot the
    // reference clears such a cursor (`0x4c7ce4`, `0x4c7eaf`), buying a held vendor row first
    // (`0x4c7e66`).
    let Some(CursorPayload::Item(held)) = model.cursor.clone() else {
        return false;
    };
    let occupied = usize::try_from(live)
        .ok()
        .and_then(|slot| model.inv_slot("player", slot))
        .is_some();

    if !occupied {
        // An empty slot: the doll handler equips the bag (`0x4c7300`), and the answer is nil
        // whether or not it went in (`0x4c7cae xor eax,eax`).
        pickup_inventory_item(model, live);
        return false;
    }

    // The held bag is this slot's own: a cancel that sends nothing and answers 1.
    if held.bag == EQUIPMENT_BAG && held.slot == live {
        model.cursor = None;
        queue_cursor_update(model);
        queue_lock_changed(model, held.bag, held.slot);
        return true;
    }

    if is_container(&held) || held.bag == EQUIPMENT_BAG {
        // A swap (`0x5e0c40`); an item from a bag equipment slot enters this fork too, hence the
        // `||`. A split stack cannot swap and stays held: the reference splits only when storing.
        if held.count.is_none() {
            model.cursor = None;
            model.container_moves.push(ContainerMove {
                src_bag: held.bag,
                src_slot: held.slot,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: live,
                count: None,
            });
            queue_cursor_update(model);
            queue_lock_changed(model, held.bag, held.slot);
            return true;
        }
        model.cursor = Some(CursorPayload::Item(held));
        return true;
    }

    // Store into the bag: `CMSG_AUTOSTORE_BAG_ITEM` (`0x10B`), or `CMSG_SPLIT_ITEM` (`0x10E`) for
    // a split. The client picks no slot: the store carries none, the split carries `0xFF`.
    let Some(dst_bag) = autostore_container(live) else {
        return false;
    };
    model.cursor = None;
    model.bag_autostores.push(crate::script::BagAutoStore {
        src_bag: held.bag,
        src_slot: held.slot,
        dst_bag,
        count: held.count,
    });
    queue_cursor_update(model);
    queue_lock_changed(model, held.bag, held.slot);
    true
}

/// `PutItemInBackpack()`, a thunk to `PutItemInBag(0xFF)` (`0x4c7ed0`) whose target is the
/// player's own guid: the empty-slot leg is unreachable and `0x4c7d3e` skips the swap, so even a
/// bag is stored into the backpack.
pub(super) fn put_item_in_backpack(model: &mut Model) -> bool {
    let Some(CursorPayload::Item(held)) = model.cursor.clone() else {
        return false;
    };
    model.cursor = None;
    model.bag_autostores.push(crate::script::BagAutoStore {
        src_bag: held.bag,
        src_slot: held.slot,
        dst_bag: 0,
        count: held.count,
    });
    queue_cursor_update(model);
    queue_lock_changed(model, held.bag, held.slot);
    true
}

/// `PickupBagFromSlot(inventorySlot)` (`0x4c7b00`): clears the cursor before it looks at the slot,
/// where the reference clears only a held item (`0x4c7b70`, `0x4c7b79`); takes only a container
/// (`0x4c7bb3`), and never places or swaps. The client has no empty-bag rule: the server refuses
/// with `EQUIP_ERR_CAN_ONLY_DO_WITH_EMPTY_BAGS`.
pub(super) fn pickup_bag_from_slot(model: &mut Model, live: u32) {
    if !accepts(live) {
        return;
    }
    clear_cursor(model);
    let picked = usize::try_from(live)
        .ok()
        .and_then(|slot| model.inv_slot("player", slot))
        .filter(|s| s.item_id != 0 && !s.locked)
        .map(|s| CursorItem {
            bag: EQUIPMENT_BAG,
            slot: live,
            item_id: s.item_id,
            texture: s.icon.clone(),
            link: s.link.clone(),
            quality: Some(u32::try_from(s.quality).unwrap_or(0)),
            count: None,
            bar_placeable: s.bar_placeable,
            equip_slots: s.equip_slots.clone(),
        })
        .filter(is_container);
    if let Some(item) = picked {
        model.cursor = Some(CursorPayload::Item(item));
        queue_cursor_update(model);
        queue_lock_changed(model, EQUIPMENT_BAG, live);
    }
}

/// Register the three verbs as globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // The number 1 or nil, never a boolean: `0x6f3810` pushes Lua tag 3, a number.
    fn consumed(yes: bool) -> mlua::Result<MultiValue> {
        Ok(MultiValue::from_vec(vec![if yes {
            Value::Number(1.0)
        } else {
            Value::Nil
        }]))
    }

    g.set(
        "PutItemInBag",
        lua.create_function(|lua, live: u32| {
            let yes = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                put_item_in_bag(&mut model, live)
            };
            consumed(yes)
        })?,
    )?;
    g.set(
        "PutItemInBackpack",
        lua.create_function(|lua, ()| {
            let yes = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                put_item_in_backpack(&mut model)
            };
            consumed(yes)
        })?,
    )?;
    g.set(
        "PickupBagFromSlot",
        lua.create_function(|lua, live: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            pickup_bag_from_slot(&mut model, live);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::cursor::EQUIPMENT_BAG;
    use crate::script::{
        BagAutoStore, BankBagSlots, ContainerMove, ContainerSlot, ContainerState, InvSlotView,
        InventorySlots, UiScript,
    };

    /// An item that is a bag: `equip_slots` is `find_equip_slot(INVTYPE_BAG)`.
    fn bag_slot_view(item_id: u32) -> InvSlotView {
        InvSlotView {
            item_id,
            icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 1,
            equip_slots: vec![20, 21, 22, 23],
            ..Default::default()
        }
    }

    /// A backpack holding one bag in slot 1 and one ordinary (head-slot) item in slot 2.
    fn backpack() -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                item_id: 4500,
                count: 1,
                texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
                equip_slots: vec![20, 21, 22, 23],
                ..Default::default()
            },
        );
        slots.insert(
            2,
            ContainerSlot {
                item_id: 1234,
                count: 1,
                texture: Some("Interface\\Icons\\INV_Helmet_01".into()),
                equip_slots: vec![1],
                ..Default::default()
            },
        );
        ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    /// Bank bag slot 1 (live id 64) holds a bag; the equipped bag slots stay empty.
    fn one_bank_bag() -> BankBagSlots {
        let mut bags: BankBagSlots = Default::default();
        bags[0] = Some(bag_slot_view(4500));
        bags
    }

    #[test]
    fn an_empty_cursor_answers_nil_and_sends_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_bank_bag_slots(one_bank_bag());
        assert!(s.eval::<bool>("return PutItemInBag(64) == nil").unwrap());
        assert!(s.eval::<bool>("return PutItemInBag(20) == nil").unwrap());
        assert!(s.eval::<bool>("return PutItemInBackpack() == nil").unwrap());
        assert!(s.take_container_moves().is_empty());
        assert!(s.take_bag_autostores().is_empty());
    }

    /// The nil after the equip is the reference's (`0x4c7cae`), so the same click opens the bag.
    #[test]
    fn an_empty_bag_slot_equips_the_held_bag_and_still_answers_nil() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(Default::default());
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(s.eval::<bool>("return PutItemInBag(64) == nil").unwrap());
        assert!(s.cursor_item().is_none(), "placed, not kept");
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 64,
                count: None,
            }]
        );
        assert!(s.take_bag_autostores().is_empty(), "an equip, not a store");
    }

    #[test]
    fn an_empty_bag_slot_refuses_an_ordinary_item() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 2)").unwrap();

        assert!(s.eval::<bool>("return PutItemInBag(64) == nil").unwrap());
        assert!(s.cursor_item().is_some(), "refused, kept");
        assert!(s.take_container_moves().is_empty());
        assert!(s.take_bag_autostores().is_empty());
    }

    #[test]
    fn an_occupied_bag_slot_swaps_with_a_held_bag_and_answers_one() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(one_bank_bag());
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert_eq!(s.eval::<i64>("return PutItemInBag(64)").unwrap(), 1);
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 64,
                count: None,
            }]
        );
    }

    #[test]
    fn an_occupied_bag_slot_auto_stores_an_ordinary_item() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(one_bank_bag());
        s.run("PickupContainerItem(0, 2)").unwrap();

        assert_eq!(s.eval::<i64>("return PutItemInBag(64)").unwrap(), 1);
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty(), "a store, not a swap");
        assert_eq!(
            s.take_bag_autostores(),
            vec![BagAutoStore {
                src_bag: 0,
                src_slot: 2,
                // Container 5 is bank bag slot 1 (`ContainerIDToInventoryID(5) == 64`).
                dst_bag: 5,
                count: None,
            }]
        );
    }

    #[test]
    fn the_backpack_button_always_auto_stores_even_a_bag() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert_eq!(s.eval::<i64>("return PutItemInBackpack()").unwrap(), 1);
        assert!(s.take_container_moves().is_empty());
        assert_eq!(
            s.take_bag_autostores(),
            vec![BagAutoStore {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                count: None,
            }]
        );
    }

    #[test]
    fn the_answer_is_the_number_one_not_the_boolean_true() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(one_bank_bag());
        s.run("PickupContainerItem(0, 2)").unwrap();
        assert!(s
            .eval::<bool>("return PutItemInBag(64) == 1 and type(PutItemInBackpack()) ~= 'boolean'")
            .unwrap());
    }

    #[test]
    fn equipment_slots_are_below_the_floor() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        let mut doll: InventorySlots = Default::default();
        doll[16] = Some(bag_slot_view(9999));
        s.set_inventory_slots(doll);
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(s.eval::<bool>("return PutItemInBag(16) == nil").unwrap());
        assert!(s.eval::<bool>("return PutItemInBag(0) == nil").unwrap());
        assert!(s.cursor_item().is_some(), "untouched");
        assert!(s.take_container_moves().is_empty());
        assert!(s.take_bag_autostores().is_empty());
    }

    #[test]
    fn pickup_bag_from_slot_takes_the_bag_and_returns_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_bank_bag_slots(one_bank_bag());
        assert_eq!(
            s.arity("PickupBagFromSlot(64)").unwrap(),
            0,
            "the delegate's eax is never tested — no return values at all"
        );
        let held = s.cursor_item().expect("picked up");
        assert_eq!(
            (held.bag, held.slot, held.item_id),
            (EQUIPMENT_BAG, 64, 4500)
        );
        assert!(s.eval::<bool>("return IsInventoryItemLocked(64)").unwrap());
    }

    #[test]
    fn pickup_bag_from_slot_drops_whatever_was_held_first() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(Default::default());
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("PickupBagFromSlot(65)").unwrap();
        assert!(s.cursor_item().is_none(), "dropped, and nothing picked up");
        assert!(s.take_container_moves().is_empty(), "no place path exists");
    }

    #[test]
    fn pickup_bag_from_slot_only_takes_containers_and_never_a_locked_one() {
        let mut s = UiScript::new().unwrap();
        // Bank slot 3 is live id 42: inside the gate, but an ordinary item.
        let mut vault = std::collections::HashMap::new();
        vault.insert(
            3,
            ContainerSlot {
                item_id: 1234,
                count: 1,
                equip_slots: vec![1],
                ..Default::default()
            },
        );
        s.set_container(
            -1,
            Some(ContainerState {
                name: Some("Bank".into()),
                num_slots: 24,
                slots: vault,
            }),
        );
        s.run("PickupBagFromSlot(42)").unwrap();
        assert!(s.cursor_item().is_none(), "not a container — silent no-op");

        let mut bags = one_bank_bag();
        bags[0] = Some(InvSlotView {
            locked: true,
            ..bag_slot_view(4500)
        });
        s.set_bank_bag_slots(bags);
        s.run("PickupBagFromSlot(64)").unwrap();
        assert!(s.cursor_item().is_none(), "locked — refused");
    }
}
