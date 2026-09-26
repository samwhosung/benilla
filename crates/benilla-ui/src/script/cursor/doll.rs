//! The paper-doll cursor verbs: an equipped slot is an item source under
//! [`super::EQUIPMENT_BAG`], so the shared cursor code handles its cancel, clear and world drop.

use mlua::Lua;

use crate::script::binding_abi::flag;
use crate::script::container::ContainerMove;
use crate::script::Model;

use super::{queue_cursor_update, queue_lock_changed, CursorItem, CursorPayload, EQUIPMENT_BAG};

/// `PickupInventoryItem(id)` (`0x4c7300`), the doll slot's click, on the bag click's model: a
/// fitting whole item moves in and the cursor clears, with no hop; a split or a misfit stays
/// held. Returns whether to repaint.
pub(super) fn pickup_inventory_item(model: &mut Model, id: u32) -> bool {
    // Head 1 to Tabard 19, the bag slots 20-23, and the bank bag slots, which only `PutItemInBag`
    // reaches (`0x4c7c00` calls `0x4c7300`). Ammo (0) is refused; its leg is not built.
    if !(1..=23).contains(&id) && !super::bag_verbs::BANK_BAG_INV_SLOTS.contains(&id) {
        return false;
    }
    // Only a held item (`0x4c769a`) or vendor row (`0x4c76a5`) takes the click first. Past them
    // an empty slot or a locked item does nothing (`0x4c76af`, `0x4c76d2`), an item-targeting
    // spell binds the item, so a worn weapon can be poisoned (`0x4c76df`: `0x6e48a0` IsTargeting,
    // `0x6e6330` TargetingWantsItem, `0x495d60` bind), and repair mode repairs a worn item
    // (`0x4c7714`), any other payload staying held.
    let placing = matches!(
        model.cursor,
        Some(CursorPayload::Item(_) | CursorPayload::Merchant(_))
    );
    if !placing && (model.item_pick_armed || model.repair_mode) {
        let usable = model
            .inv_slot("player", id as usize)
            .is_some_and(|s| s.item_id != 0 && !s.locked);
        if usable && model.item_pick_armed {
            model.item_picks.push((EQUIPMENT_BAG, id));
        } else if usable && (1..=19).contains(&id) {
            model.inventory_repairs.push(id);
        }
        return false;
    }
    match model.cursor.take() {
        None => {
            let picked = model
                .inv_slot("player", id as usize)
                .filter(|s| s.item_id != 0 && !s.locked)
                .map(|s| CursorItem {
                    bag: EQUIPMENT_BAG,
                    slot: id,
                    item_id: s.item_id,
                    texture: s.icon.clone(),
                    link: s.link.clone(),
                    quality: Some(u32::try_from(s.quality).unwrap_or(0)),
                    count: None,
                    bar_placeable: s.bar_placeable,
                    equip_slots: s.equip_slots.clone(),
                });
            match picked {
                Some(item) => {
                    model.cursor = Some(CursorPayload::Item(item));
                    queue_cursor_update(model);
                    queue_lock_changed(model, EQUIPMENT_BAG, id);
                    true
                }
                None => false,
            }
        }
        Some(CursorPayload::Item(held)) if held.bag == EQUIPMENT_BAG && held.slot == id => {
            queue_cursor_update(model);
            queue_lock_changed(model, held.bag, held.slot);
            true
        }
        Some(CursorPayload::Item(held)) => {
            // A split or a misfit stays held, with no event: nothing changed.
            if held.count.is_some() || !super::bag_verbs::fits_slot(&held, id) {
                model.cursor = Some(CursorPayload::Item(held));
                return false;
            }
            model.container_moves.push(ContainerMove {
                src_bag: held.bag,
                src_slot: held.slot,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: id,
                count: None,
            });
            queue_cursor_update(model);
            queue_lock_changed(model, held.bag, held.slot);
            true
        }
        // Mode 5, a buy into this slot: `0x4c78fd` sends `CMSG_BUY_ITEM_IN_SLOT` (`0x1a3`) and the
        // server equips or refuses. A stale row buys nothing: of the reference's three drop sites
        // only this one checks it; the other two (`0x4f9f35`, `0x4c7ea5`) dereference it.
        Some(CursorPayload::Merchant(held)) => {
            let entry = model
                .merchant
                .as_ref()
                .and_then(|m| m.items.get(held.row as usize))
                .filter(|it| it.item_id == held.item_id)
                .map(|it| it.item_id);
            if let Some(entry) = entry {
                model.merchant_slot_buys.push((EQUIPMENT_BAG, id, entry));
            }
            queue_cursor_update(model);
            true
        }
        Some(
            other @ (CursorPayload::Spell(_)
            | CursorPayload::Action(_)
            | CursorPayload::Macro(_)
            | CursorPayload::PetAction(_)
            // Mode 10: a stabled pet stays held for the stable window.
            | CursorPayload::StablePet(_)
            // Mode 2: coins stay held for a money frame's drop. On an occupied slot the reference
            // picks the item up over either, as it tests only for a held item (`0x4c769a`) or a
            // preview row (`0x4c76a5`) before the pickup (`0x4c7838`).
            | CursorPayload::Money(_)),
        ) => {
            model.cursor = Some(other);
            false
        }
    }
}

/// `EquipCursorItem(id)`: placing the held item on doll slot `id`.
pub(super) fn equip_cursor_item(model: &mut Model, id: u32) -> bool {
    pickup_inventory_item(model, id)
}

/// `CursorCanGoInSlot(id)`, the doll slot's highlight on `CURSOR_UPDATE`
/// (`PaperDollFrame.lua:609-615`): `equip_slots`, which follows vmangos `Player::FindEquipSlot`,
/// where the reference's `IsValidForSlot` (`0x5da1d0`) tests a static slot mask per
/// `InventoryType`.
pub(super) fn cursor_can_go_in_slot(model: &Model, id: u32) -> bool {
    match &model.cursor {
        Some(CursorPayload::Item(item)) => super::bag_verbs::fits_slot(item, id),
        _ => false,
    }
}

/// `AutoEquipCursorItem()`, a click or drop on the doll's model (`PaperDollFrame.lua:31-35`): a
/// whole stack from a bag becomes `CMSG_AUTOEQUIP_ITEM`: the server picks the slot and swaps
/// the displaced piece back (`ItemHandler.cpp:138-228`); an equipped item or a split stays held.
/// Returns whether to repaint.
pub(super) fn auto_equip_cursor_item(model: &mut Model) -> bool {
    match model.cursor.take() {
        Some(CursorPayload::Item(item)) if item.bag >= 0 && item.count.is_none() => {
            model.container_autoequips.push((item.bag, item.slot));
            queue_cursor_update(model);
            queue_lock_changed(model, item.bag, item.slot);
            true
        }
        other => {
            model.cursor = other;
            false
        }
    }
}

/// `UseInventoryItem(id)`, the doll slot's right-click (`PaperDollFrame.lua:658-659`): the app
/// sends `CMSG_USE_ITEM` with bag 255 and the 0-based slot, dropping an empty slot unsent. In
/// repair mode the cursor clears first (`0x4c79a9`) and a worn item is repaired, locked or not
/// (`0x4c79c4`): a held payload goes back, never placed.
pub(super) fn use_inventory_item(model: &mut Model, id: u32) {
    if !model.repair_mode {
        model.inventory_uses.push(id);
        return;
    }
    super::clear_cursor(model);
    if (1..=19).contains(&id)
        && model
            .inv_slot("player", id as usize)
            .is_some_and(|s| s.item_id != 0)
    {
        model.inventory_repairs.push(id);
    }
}

/// `IsInventoryItemLocked(id)`: true while `id` is the held item's source or the app reports a
/// pending operation on it. Reads [`Model::inv_slot`], since `BankFrameItemButton_UpdateLock`
/// asks about bank slots too.
pub(super) fn is_inventory_item_locked(model: &Model, id: u32) -> bool {
    let held_here = matches!(&model.cursor, Some(CursorPayload::Item(c)) if c.bag == EQUIPMENT_BAG && c.slot == id);
    let fed = usize::try_from(id)
        .ok()
        .and_then(|i| model.inv_slot("player", i))
        .is_some_and(|s| s.locked);
    held_here || fed
}

/// Register the paper-doll verbs as globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "PickupInventoryItem",
        lua.create_function(|lua, id: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(pickup_inventory_item(&mut model, id))
        })?,
    )?;
    g.set(
        "EquipCursorItem",
        lua.create_function(|lua, id: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(equip_cursor_item(&mut model, id))
        })?,
    )?;
    g.set(
        "CursorCanGoInSlot",
        lua.create_function(|lua, id: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(cursor_can_go_in_slot(&model, id))
        })?,
    )?;
    g.set(
        "AutoEquipCursorItem",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(auto_equip_cursor_item(&mut model))
        })?,
    )?;
    g.set(
        "UseInventoryItem",
        lua.create_function(|lua, id: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            use_inventory_item(&mut model, id);
            Ok(())
        })?,
    )?;
    g.set(
        "IsInventoryItemLocked",
        lua.create_function(|lua, id: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(is_inventory_item_locked(&model, id)))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::cursor::{CursorAction, CursorPayload, CursorSpell, EQUIPMENT_BAG};
    use crate::script::{ContainerMove, ContainerSlot, ContainerState, InvSlotView, UiScript};

    /// Head (1), a ring in finger slot 11 that also fits 12, and Tabard (19).
    fn doll_slots() -> crate::script::InventorySlots {
        let mut slots: crate::script::InventorySlots = Default::default();
        slots[1] = Some(InvSlotView {
            duration_ms: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            flags: 0,
            item_id: 1234,
            icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
            count: 1,
            contents_count: None,
            quality: 2,
            name: Some("Test Helm".into()),
            link: Some("|cff1eff00|Hitem:1234:0:0:0|h[Test Helm]|h|r".into()),
            locked: false,
            equip_slots: vec![1],
            creator: None,
            enchants: Vec::new(),
        });
        slots[11] = Some(InvSlotView {
            duration_ms: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            flags: 0,
            item_id: 555,
            icon: Some("Interface\\Icons\\INV_Jewelry_Ring_01".into()),
            count: 1,
            contents_count: None,
            quality: 1,
            name: Some("Test Ring".into()),
            link: Some("|cffffffff|Hitem:555:0:0:0|h[Test Ring]|h|r".into()),
            locked: false,
            equip_slots: vec![11, 12], // either finger: the doll-to-doll swap
            creator: None,
            enchants: Vec::new(),
        });
        slots[19] = Some(InvSlotView {
            duration_ms: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            flags: 0,
            item_id: 999,
            icon: Some("Interface\\Icons\\INV_Shirt_White_01".into()),
            count: 1,
            contents_count: None,
            quality: 1,
            name: Some("Test Tabard".into()),
            link: Some("|cffffffff|Hitem:999:0:0:0|h[Test Tabard]|h|r".into()),
            locked: false,
            equip_slots: vec![19],
            creator: None,
            enchants: Vec::new(),
        });
        slots
    }

    fn one_fitting_bag_item() -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Helmet_02".into()),
                count: 1,
                quality: Some(3),
                item_id: 2000,
                link: Some("|cff0070dd|Hitem:2000:0:0:0|h[Another Helm]|h|r".into()),
                locked: false,
                equip_slots: vec![1], // fits HeadSlot only
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    #[test]
    fn pickup_from_doll_slot_holds_and_locks() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());

        assert!(!s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());
        assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        let held = s.cursor_item().expect("picked up");
        assert_eq!(
            (held.bag, held.slot, held.item_id),
            (EQUIPMENT_BAG, 1, 1234)
        );
        assert_eq!(held.equip_slots, vec![1]);
        assert!(s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());
        assert!(s.eval::<bool>("return CursorHasItem()").unwrap());
    }

    #[test]
    fn pickup_same_doll_slot_cancels_no_move() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
        assert!(!s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());
    }

    #[test]
    fn place_fitting_bag_item_onto_doll_slot_queues_move_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_container(0, Some(one_fitting_bag_item()));

        assert!(s.eval::<bool>("return PickupContainerItem(0, 1)").unwrap());
        assert_eq!(s.cursor_item().unwrap().equip_slots, vec![1]);

        assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(s.cursor_item().is_none(), "a plain clear, no hop");
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 1,
                count: None,
            }]
        );
    }

    #[test]
    fn place_non_fitting_item_is_a_no_op() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        // The bag item only fits HeadSlot (1); try placing it on NeckSlot (2).
        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(!s.eval::<bool>("return PickupInventoryItem(2)").unwrap());
        let held = s.cursor_item().expect("kept — doesn't fit slot 2");
        assert_eq!(held.item_id, 2000);
        assert!(s.take_container_moves().is_empty());
    }

    #[test]
    fn doll_to_doll_ring_swap_fits_via_shared_equip_slots() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());

        assert!(s.eval::<bool>("return PickupInventoryItem(11)").unwrap());
        assert_eq!(s.cursor_item().unwrap().equip_slots, vec![11, 12]);

        assert!(s.eval::<bool>("return PickupInventoryItem(12)").unwrap());
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: EQUIPMENT_BAG,
                src_slot: 11,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 12,
                count: None,
            }]
        );
    }

    #[test]
    fn split_carry_refuses_to_equip() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        let mut state = one_fitting_bag_item();
        state.slots.get_mut(&1).unwrap().count = 5;
        s.set_container(0, Some(state));

        s.run("SplitContainerItem(0, 1, 2)").unwrap();
        let held = s.cursor_item().expect("a split carry");
        assert_eq!(held.count, Some(2));

        assert!(!s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        let held = s.cursor_item().expect("kept — can't equip a partial stack");
        assert_eq!(held.count, Some(2));
        assert!(s.take_container_moves().is_empty());
    }

    #[test]
    fn pickup_inventory_item_refuses_ammo_but_takes_a_bag_slot() {
        let mut s = UiScript::new().unwrap();
        let mut slots = doll_slots();
        slots[0] = Some(InvSlotView {
            durability: None,
            flags: 0,
            item_id: 2512,
            equip_slots: Vec::new(),
            ..Default::default()
        });
        // An equipped bag in Bag0Slot (id 20): its equip_slots is the four bag slots.
        slots[20] = Some(InvSlotView {
            duration_ms: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            flags: 0,
            item_id: 4496,
            icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 1,
            contents_count: None,
            quality: 1,
            name: Some("Small Brown Pouch".into()),
            link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
            locked: false,
            equip_slots: vec![20, 21, 22, 23],
            creator: None,
            enchants: Vec::new(),
        });
        s.set_inventory_slots(slots);

        // Ammo (0) is refused; Bag0Slot (20) picks up the equipped bag.
        assert!(!s.eval::<bool>("return PickupInventoryItem(0)").unwrap());
        assert!(s.eval::<bool>("return PickupInventoryItem(20)").unwrap());
        let held = s.cursor_item().expect("bag picked up from the bar");
        assert_eq!(
            (held.bag, held.slot, held.item_id),
            (EQUIPMENT_BAG, 20, 4496)
        );
    }

    #[test]
    fn place_a_bag_onto_a_bag_slot_queues_the_equip_move() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            3,
            ContainerSlot {
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
                count: 1,
                quality: Some(1),
                item_id: 4496,
                link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
                locked: false,
                equip_slots: vec![20, 21, 22, 23], // INVTYPE_BAG → any bag slot
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        s.set_container(
            0,
            Some(ContainerState {
                name: Some("Backpack".into()),
                num_slots: 16,
                slots,
            }),
        );
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert!(s.eval::<bool>("return CursorCanGoInSlot(21)").unwrap());

        assert!(s.eval::<bool>("return PickupInventoryItem(21)").unwrap());
        assert!(s.cursor_item().is_none(), "a plain clear on place");
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 3,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 21,
                count: None,
            }]
        );
    }

    #[test]
    fn cursor_can_go_in_slot_per_arm() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());

        assert!(!s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());

        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());
        assert!(!s.eval::<bool>("return CursorCanGoInSlot(2)").unwrap());
        s.run("ClearCursor()").unwrap();

        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 1,
            texture: None,
        }));
        assert!(!s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0,
            action: 1,
            texture: None,
        }));
        assert!(!s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());
    }

    #[test]
    fn auto_equip_cursor_item_queues_source_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(s.eval::<bool>("return AutoEquipCursorItem()").unwrap());
        assert!(s.cursor_item().is_none());
        assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);
    }

    #[test]
    fn auto_equip_cursor_item_is_a_no_op_from_the_doll_or_a_split_carry() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());

        s.run("PickupInventoryItem(1)").unwrap();
        assert!(!s.eval::<bool>("return AutoEquipCursorItem()").unwrap());
        assert!(s.cursor_item().is_some(), "kept");
        assert!(s.take_container_autoequips().is_empty());
        s.run("ClearCursor()").unwrap();

        let mut state = one_fitting_bag_item();
        state.slots.get_mut(&1).unwrap().count = 5;
        s.set_container(0, Some(state));
        s.run("SplitContainerItem(0, 1, 2)").unwrap();
        assert!(!s.eval::<bool>("return AutoEquipCursorItem()").unwrap());
        assert!(s.cursor_item().is_some(), "kept");
        assert!(s.take_container_autoequips().is_empty());
    }

    #[test]
    fn use_inventory_item_queues_the_id() {
        let mut s = UiScript::new().unwrap();
        s.run("UseInventoryItem(1)").unwrap();
        s.run("UseInventoryItem(19)").unwrap();
        assert_eq!(s.take_inventory_uses(), vec![1, 19]);
        assert!(s.take_inventory_uses().is_empty(), "drained");
    }

    #[test]
    fn repair_mode_queues_an_equipped_slot_without_picking_it_up() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_merchant(Some(crate::script::MerchantState {
            can_repair: true,
            ..Default::default()
        }));
        s.run("ShowRepairCursor()").unwrap();

        assert!(!s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(!s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        s.run("UseInventoryItem(1)").unwrap();
        assert!(s.cursor_item().is_none());
        assert_eq!(s.take_inventory_repairs(), vec![1, 1, 1]);
        assert!(s.take_inventory_uses().is_empty(), "no item-use intent");
        assert!(s.take_inventory_repairs().is_empty(), "drained");
    }

    #[test]
    fn repair_mode_follows_the_doll_clicks_order() {
        use crate::script::cursor::CursorMoney;
        let mut s = UiScript::new().unwrap();
        let mut slots = doll_slots();
        if let Some(head) = slots[1].as_mut() {
            head.locked = true;
        }
        s.set_inventory_slots(slots);
        s.model_mut().repair_mode = true;

        s.run("PickupInventoryItem(1)").unwrap();
        assert!(
            s.take_inventory_repairs().is_empty(),
            "a locked item does nothing (`0x4c76d2`)"
        );
        let coins = CursorPayload::Money(CursorMoney { copper: 50 });
        s.model_mut().cursor = Some(coins.clone());
        s.run("PickupInventoryItem(11)").unwrap();
        assert_eq!(
            s.take_inventory_repairs(),
            vec![11],
            "coins do not block it"
        );
        assert_eq!(s.cursor_payload(), Some(coins), "and stay held");

        // A held item takes the click first (`0x4c769a`): the ring goes back, nothing repairs.
        s.model_mut().cursor = None;
        s.model_mut().repair_mode = false;
        s.run("PickupInventoryItem(11)").unwrap();
        s.model_mut().repair_mode = true;
        s.run("PickupInventoryItem(11)").unwrap();
        assert!(s.cursor_payload().is_none());
        assert!(s.take_inventory_repairs().is_empty());
    }

    #[test]
    fn repair_mode_right_click_puts_a_held_item_back_then_repairs() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.run("PickupInventoryItem(11)").unwrap();
        assert!(s.cursor_item().is_some());
        s.model_mut().repair_mode = true;

        s.run("UseInventoryItem(19)").unwrap();
        assert!(
            s.cursor_payload().is_none(),
            "the ring went back (`0x4c79a9`)"
        );
        assert!(s.take_container_moves().is_empty(), "never placed");
        assert_eq!(s.take_inventory_repairs(), vec![19]);
        assert!(s.take_inventory_uses().is_empty(), "no use either");
    }

    #[test]
    fn is_inventory_item_locked_reads_held_or_fed() {
        let mut s = UiScript::new().unwrap();
        let mut slots = doll_slots();
        slots[19].as_mut().unwrap().locked = true; // the app's own PendingItemOps feed
        s.set_inventory_slots(slots);

        assert!(
            s.eval::<bool>("return IsInventoryItemLocked(19)").unwrap(),
            "fed lock"
        );
        assert!(!s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());

        s.run("PickupInventoryItem(1)").unwrap();
        assert!(
            s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap(),
            "held-here lock"
        );
    }

    #[test]
    fn equip_cursor_item_routes_through_pickup() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(s.eval::<bool>("return EquipCursorItem(1)").unwrap());
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 1,
                count: None,
            }]
        );
    }

    /// The held-payload test runs first in both seams: `0x4f9c38` before `0x4f9c54` in the bag,
    /// `0x4c73af` before `0x4c76df` on the doll.
    #[test]
    fn the_armed_item_half_reroutes_bag_and_doll_clicks() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_container(0, Some(one_fitting_bag_item()));

        // Unarmed, both seams do their ordinary gesture and queue nothing.
        assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(s.cursor_item().is_some());
        assert!(s.take_item_picks().is_empty());

        // Armed but holding: the payload wins.
        s.set_item_pick_armed(true);
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(
            s.take_item_picks().is_empty(),
            "a click while carrying an item is a place, not a bind"
        );

        // Armed with an empty cursor: both seams bind and leave the cursor empty.
        s.run("ClearCursor()").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(!s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(!s.eval::<bool>("return PickupContainerItem(0, 1)").unwrap());
        assert_eq!(
            s.take_item_picks(),
            vec![(EQUIPMENT_BAG, 1), (0, 1)],
            "the doll reports in the ONE bag space; the bag reports its own id"
        );
        assert!(s.cursor_item().is_none(), "a bind never stages a payload");
    }
}
