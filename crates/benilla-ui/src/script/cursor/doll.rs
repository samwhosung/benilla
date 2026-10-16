//! The paper-doll cursor verbs: an equipped slot is an item source under
//! [`super::EQUIPMENT_BAG`], so the shared cursor code handles its cancel, clear and world drop.

use mlua::{Lua, Value};

use crate::script::binding_abi::flag;
use crate::script::container::ContainerMove;
use crate::script::protected::{self, Tag};
use crate::script::tick::verb;
use crate::script::Model;

use super::{
    clear, clear_cursor, equip::auto_equip_cursor_item, item_locked, lock_item, pick_up_item,
    Clear, CursorItem, CursorPayload, EQUIPMENT_BAG,
};

/// `PickupInventoryItem(id)` (`0x4c7300`), the doll slot's click, on the bag click's model: a
/// fitting whole item moves in and the cursor clears, with no hop; a split or a misfit stays
/// held. Returns whether to repaint.
pub(super) fn pickup_inventory_item(lua: &Lua, id: u32) -> bool {
    // An armed item-targeting spell binds the item through `BindSpellTargetObject`, behind the
    // gate's tag 2.
    let may_cast = protected::gate(lua, Tag::CAST);
    match verb(lua, |model| doll_click(model, id, may_cast)) {
        DollClick::Done(repaint) => repaint,
        DollClick::PickUp(item) => {
            pick_up_item(lua, item);
            true
        }
    }
}

/// What a doll click did inside its segment, or the pickup it owes past it.
pub(super) enum DollClick {
    /// Handled; whether to repaint.
    Done(bool),
    /// The item to pick up: `SetCursorItem(1, …)` and `LockItem` (`0x4c7838`, `0x4c783f`), whose
    /// inner clear signals in a segment of its own ([`pick_up_item`]).
    PickUp(CursorItem),
}

/// [`pickup_inventory_item`]'s decision and every arm but the pickup.
pub(super) fn doll_click(model: &mut Model, id: u32, may_cast: bool) -> DollClick {
    // Head 1 to Tabard 19, the bag slots 20-23, and the bank bag slots, which only `PutItemInBag`
    // reaches (`0x4c7c00` calls `0x4c7300`). Ammo (0) is refused; its leg is not built.
    if !(1..=23).contains(&id) && !super::bag_verbs::BANK_BAG_INV_SLOTS.contains(&id) {
        return DollClick::Done(false);
    }
    // A bank bag slot needs the bank open (`0x4c7366`-`0x4c7386`).
    if super::bag_verbs::BANK_BAG_INV_SLOTS.contains(&id) && model.bank.is_none() {
        return DollClick::Done(false);
    }
    // Only a held item (`0x4c769a`) or a held row, a vendor's or the bar's (`0x4c76a5`), takes the
    // click first. Past them an empty slot or a locked item does nothing (`0x4c76af`,
    // `0x4c76d2`), an item-targeting spell binds the item, so a worn weapon can be poisoned
    // (`0x4c76df`: `0x6e48a0` IsTargeting, `0x6e6330` TargetingWantsItem, `0x495d60` bind), and
    // repair mode repairs a worn item (`0x4c7714`), any other payload staying held. Else the item
    // is picked up (`0x4c7838`), its setter's clear dropping whatever else is held.
    let placing = matches!(
        model.cursor,
        Some(CursorPayload::Item(_) | CursorPayload::Merchant(_) | CursorPayload::Action(_))
    );
    if !placing && (model.item_pick_armed || model.repair_mode) {
        let usable = model
            .inv_slot("player", id as usize)
            .is_some_and(|s| s.item_id != 0 && !item_locked(model, s.guid));
        if usable && model.item_pick_armed {
            model.item_picks.push(super::ItemPick {
                bag: EQUIPMENT_BAG,
                slot: id,
                may_cast,
            });
        } else if usable && (1..=19).contains(&id) {
            model.inventory_repairs.push(id);
        }
        return DollClick::Done(false);
    }
    match model.cursor.clone() {
        None
        | Some(
            CursorPayload::Spell(_)
            | CursorPayload::Macro(_)
            | CursorPayload::PetAction(_)
            | CursorPayload::StablePet(_)
            | CursorPayload::Money(_),
        ) => {
            let picked = model
                .inv_slot("player", id as usize)
                .filter(|s| s.item_id != 0 && !item_locked(model, s.guid))
                .map(|s| CursorItem {
                    item_class: None,
                    guid: s.guid,
                    bag: EQUIPMENT_BAG,
                    slot: id,
                    item_id: s.item_id,
                    texture: s.icon.clone(),
                    link: s.link.clone(),
                    quality: Some(u32::try_from(s.quality).unwrap_or(0)),
                    count: None,
                    bar_placeable: s.bar_placeable,
                    consumable: s.consumable,
                    equip_slots: s.equip_slots.clone(),
                });
            match picked {
                Some(item) => DollClick::PickUp(item),
                None => DollClick::Done(false),
            }
        }
        // The cancel is `ClearCursor(1,1)` (`0x4c794d`): the unlock, then `CURSOR_UPDATE`.
        Some(CursorPayload::Item(held)) if held.bag == EQUIPMENT_BAG && held.slot == id => {
            clear_cursor(model);
            DollClick::Done(true)
        }
        Some(CursorPayload::Item(held)) => {
            // A locked occupant refuses the drop, the payload kept (`0x4c787b`).
            let occupant = model
                .inv_slot("player", id as usize)
                .filter(|s| s.item_id != 0)
                .map_or(0, |s| s.guid);
            if item_locked(model, occupant) {
                return DollClick::Done(false);
            }
            // A split stays held, with no event: nothing changed.
            if held.count.is_some() {
                return DollClick::Done(false);
            }
            // A misfit (`0x5ea720` false): an equipped item, the 0-based slot below 0x13, goes back
            // with `ClearCursor(1,1)` (`0x4c78e9`, `0x4c794d`); anything else is auto-equipped
            // (`0x4c78ef`, `0x5e1480`).
            if !super::bag_verbs::fits_slot(&held, id) {
                if held.bag == EQUIPMENT_BAG && held.slot <= 19 {
                    clear_cursor(model);
                    return DollClick::Done(true);
                }
                return DollClick::Done(auto_equip_cursor_item(model));
            }
            model.container_moves.push(ContainerMove {
                src_bag: held.bag,
                src_slot: held.slot,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: id,
                count: None,
            });
            // The swap send clears with `ClearCursor(0,1)` (`0x5e0fe7`), then `LockItem` on the
            // occupant (`0x4c78c8`), an empty slot's zero guid signalling nothing (`0x4953ed`).
            clear(model, Clear::KEEP_LOCK);
            lock_item(model, occupant);
            DollClick::Done(true)
        }
        // Mode 5, a buy into this slot: `0x4c78fd` sends `CMSG_BUY_ITEM_IN_SLOT` (`0x1a3`) and the
        // server equips or refuses, then `ClearCursor(1,1)` (`0x4c7954`). With no vendor open or
        // the row out of its list the row stays held (`0x4c7916`-`0x4c7931`); of the reference's
        // three drop sites only this one checks the row, the other two (`0x4f9f35`, `0x4c7ea5`)
        // dereference it. A row whose item has changed buys nothing.
        Some(CursorPayload::Merchant(held)) => {
            let Some(row) = model
                .merchant
                .as_ref()
                .and_then(|m| m.items.get(held.row as usize))
            else {
                return DollClick::Done(false);
            };
            if row.item_id == held.item_id {
                model
                    .merchant_slot_buys
                    .push((EQUIPMENT_BAG, id, row.item_id));
            }
            clear_cursor(model);
            DollClick::Done(true)
        }
        // Mode 7, an item off the bar, is a held row but no vendor's: it stays held (`0x4c78fd`).
        Some(CursorPayload::Action(_)) => DollClick::Done(false),
    }
}

/// `EquipCursorItem(id)`: placing the held item on doll slot `id`.
pub(super) fn equip_cursor_item(lua: &Lua, id: u32) -> bool {
    pickup_inventory_item(lua, id)
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

/// `UseInventoryItem(id)`, the doll slot's right-click (`PaperDollFrame.lua:658-659`): the cursor
/// clears before the slot is read (`0x4c79a9`), so a held payload goes back, never placed, its
/// events running first; then the app sends `CMSG_USE_ITEM` with bag 255 and the 0-based slot,
/// dropping an empty slot unsent, or in repair mode a worn item is repaired, locked or not
/// (`0x4c79c4`).
pub(super) fn use_inventory_item(lua: &Lua, id: u32, on_self: bool) {
    verb(lua, clear_cursor);
    // The item's on-use spell casts through `TryCastSpell`, behind the gate's tag 2.
    let may_cast = protected::gate(lua, Tag::CAST);
    verb(lua, |model| use_cleared(model, id, on_self, may_cast));
}

/// `UseInventoryItem` past its clear. Outside repair mode a locked worn item is refused in the
/// call, `CGItem::Use`'s first gate (`0x4c7af0` → `0x5d8d32`), so a later call's lock does not
/// reach it.
fn use_cleared(model: &mut Model, id: u32, on_self: bool, may_cast: bool) {
    if !model.repair_mode {
        let locked = model
            .inv_slot("player", id as usize)
            .is_some_and(|s| super::item_locked(model, s.guid));
        // Then the dispatcher's requirement and control gates (`0x5d8e0e`, `0x5d8eaa`), which an
        // empty slot never reaches.
        let refusal = model
            .inv_slot("player", id as usize)
            // A container returns ahead of both gates (`0x5d8d26`).
            .filter(|s| s.item_id != 0 && s.contents_count.is_none())
            .and_then(|s| {
                crate::script::use_gates::item_use_refusal(
                    model,
                    s.use_fork,
                    s.item_id,
                    s.use_spell,
                    s.link.is_some(),
                )
            });
        if let Some(key) = refusal.filter(|_| !locked) {
            model.push_ui_error(key);
        } else if !locked {
            model.queue_call(crate::script::ScriptCall::UseInventoryItem {
                slot: id,
                on_self,
                may_cast,
            });
        }
        return;
    }
    if (1..=19).contains(&id)
        && model
            .inv_slot("player", id as usize)
            .is_some_and(|s| s.item_id != 0)
    {
        model.inventory_repairs.push(id);
    }
}

/// `IsInventoryItemLocked(id)`: whether the item in player slot `id` is locked ([`item_locked`]).
/// Reads [`Model::inv_slot`], since `BankFrameItemButton_UpdateLock` asks about bank slots too.
pub(super) fn is_inventory_item_locked(model: &Model, id: u32) -> bool {
    usize::try_from(id)
        .ok()
        .and_then(|i| model.inv_slot("player", i))
        .is_some_and(|s| s.item_id != 0 && item_locked(model, s.guid))
}

/// Register the paper-doll verbs as globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "PickupInventoryItem",
        lua.create_function(|lua, id: u32| {
            pickup_inventory_item(lua, id);
            Ok(())
        })?,
    )?;
    g.set(
        "EquipCursorItem",
        lua.create_function(|lua, id: u32| {
            equip_cursor_item(lua, id);
            Ok(())
        })?,
    )?;
    g.set(
        "CursorCanGoInSlot",
        lua.create_function(|lua, id: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(cursor_can_go_in_slot(&model, id)))
        })?,
    )?;
    g.set(
        "AutoEquipCursorItem",
        lua.create_function(|lua, ()| {
            verb(lua, auto_equip_cursor_item);
            Ok(())
        })?,
    )?;
    g.set(
        "UseInventoryItem",
        lua.create_function(|lua, (id, rest): (u32, mlua::MultiValue)| {
            // `onSelf` through the client's flag coercion (`0x4c8e37`, `0x6f1c10`, default 0).
            let on_self = crate::script::binding_abi::bool_or_default(rest.front(), false);
            use_inventory_item(lua, id, on_self);
            Ok(())
        })?,
    )?;
    g.set(
        "IsInventoryItemLocked",
        lua.create_function(|lua, slot: Value| {
            // The slot reader raises for any id outside its whitelist, a negative included
            // (`0x4c8520`, `0x4c8e7c`), and the ammo leg (Lua 0) answers nil (`0x4c8e98`).
            let slot0 = crate::script::inventory_verbs::inventory_slot_arg(
                lua,
                slot,
                "Invalid inventory slot in IsInventoryItemLocked",
            )?;
            if slot0 < 0 {
                return Ok(Value::Nil);
            }
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(is_inventory_item_locked(&model, slot0 as u32 + 1)))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{auto_equip_cursor_item, equip_cursor_item, pickup_inventory_item};
    use crate::script::cursor::{CursorAction, CursorPayload, CursorSpell, EQUIPMENT_BAG};
    use crate::script::ItemPick;
    use crate::script::{ContainerMove, ContainerSlot, ContainerState, InvSlotView, UiScript};

    /// `IsInventoryItemLocked`'s slot reader (`0x4c8520`) raises for any id outside {0, 1-23,
    /// 40-69, 82-113}, a negative included (`0x4c8e7c`); the ammo leg (0) answers nil; 99 is the
    /// keyring's slot 98 and answers.
    #[test]
    fn is_inventory_item_locked_raises_outside_the_readers_whitelist() {
        let s = UiScript::new().unwrap();
        for id in ["-1", "24", "39", "70", "81", "114", "'x'", "nil"] {
            let err = s
                .eval::<String>(&format!(
                    "local ok, e = pcall(IsInventoryItemLocked, {id}) return ok and 'ok' or tostring(e)"
                ))
                .unwrap();
            assert!(
                err.contains("Invalid inventory slot in IsInventoryItemLocked"),
                "{id}: {err}"
            );
        }
        for id in ["0", "1", "23", "40", "69", "82", "99", "113", "1.9"] {
            assert_eq!(
                s.eval::<i64>(&format!(
                    "return (function(...) return arg.n end)(IsInventoryItemLocked({id}))"
                ))
                .unwrap(),
                1,
                "{id} answers one value"
            );
        }
    }

    /// Head (1), a ring in finger slot 11 that also fits 12, and Tabard (19).
    fn doll_slots() -> crate::script::InventorySlots {
        let mut slots: crate::script::InventorySlots = Default::default();
        slots[1] = Some(InvSlotView {
            duration_ms: None,
            already_bound: false,
            occupied_seats: None,
            bonding: 0,
            use_spell: 0,
            use_fork: Default::default(),
            bar_placeable: true,
            consumable: false,
            durability: None,
            flags: 0,
            item_id: 1234,
            icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
            count: 1,
            contents_count: None,
            quality: 2,
            name: Some("Test Helm".into()),
            link: Some("|cff1eff00|Hitem:1234:0:0:0|h[Test Helm]|h|r".into()),
            guid: 0x10c,
            equip_slots: vec![1],
            creator: None,
            gift_creator: None,
            creator_guid: 0,
            gift_creator_guid: 0,
            enchants: Vec::new(),
            spell_0: false,
        });
        slots[11] = Some(InvSlotView {
            duration_ms: None,
            already_bound: false,
            occupied_seats: None,
            bonding: 0,
            use_spell: 0,
            use_fork: Default::default(),
            bar_placeable: true,
            consumable: false,
            durability: None,
            flags: 0,
            item_id: 555,
            icon: Some("Interface\\Icons\\INV_Jewelry_Ring_01".into()),
            count: 1,
            contents_count: None,
            quality: 1,
            name: Some("Test Ring".into()),
            link: Some("|cffffffff|Hitem:555:0:0:0|h[Test Ring]|h|r".into()),
            guid: 0x10d,
            equip_slots: vec![11, 12], // either finger: the doll-to-doll swap
            creator: None,
            gift_creator: None,
            creator_guid: 0,
            gift_creator_guid: 0,
            enchants: Vec::new(),
            spell_0: false,
        });
        slots[19] = Some(InvSlotView {
            duration_ms: None,
            already_bound: false,
            occupied_seats: None,
            bonding: 0,
            use_spell: 0,
            use_fork: Default::default(),
            bar_placeable: true,
            consumable: false,
            durability: None,
            flags: 0,
            item_id: 999,
            icon: Some("Interface\\Icons\\INV_Shirt_White_01".into()),
            count: 1,
            contents_count: None,
            quality: 1,
            name: Some("Test Tabard".into()),
            link: Some("|cffffffff|Hitem:999:0:0:0|h[Test Tabard]|h|r".into()),
            guid: 0x10e,
            equip_slots: vec![19],
            creator: None,
            gift_creator: None,
            creator_guid: 0,
            gift_creator_guid: 0,
            enchants: Vec::new(),
            spell_0: false,
        });
        slots
    }

    fn one_fitting_bag_item() -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                use_fork: Default::default(),
                item_class: None,
                duration_ms: None,
                petition: None,
                already_bound: false,
                occupied_seats: None,
                bonding: 0,
                use_spell: 0,
                bar_placeable: true,
                consumable: false,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Helmet_02".into()),
                count: 1,
                quality: Some(3),
                inventory_type: 0,
                item_id: 2000,
                link: Some("|cff0070dd|Hitem:2000:0:0:0|h[Another Helm]|h|r".into()),
                guid: 0x10f,
                equip_slots: vec![1], // fits HeadSlot only
                cooldown: None,
                readable: false,
                creator: None,
                gift_creator: None,
                creator_guid: 0,
                gift_creator_guid: 0,
                flags: 0,
                enchants: Vec::new(),
                spell_0: false,
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
        assert!(pickup_inventory_item(&s.lua, 1));
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
        assert!(pickup_inventory_item(&s.lua, 1));
        assert!(pickup_inventory_item(&s.lua, 1));
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
        assert!(!s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());
    }

    #[test]
    fn place_fitting_bag_item_onto_doll_slot_queues_move_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_container(0, Some(one_fitting_bag_item()));

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(s.cursor_item().unwrap().equip_slots, vec![1]);

        assert!(pickup_inventory_item(&s.lua, 1));
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

    /// A misfit from a bag is auto-equipped (`0x4c78ef`, `0x5e1480`); an equipped misfit goes back
    /// with `ClearCursor(1,1)` (`0x4c78e9`, `0x4c794d`).
    #[test]
    fn a_misfit_drop_auto_equips_or_goes_back() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        // The bag item only fits HeadSlot (1); place it on NeckSlot (2).
        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(pickup_inventory_item(&s.lua, 2));
        assert!(s.cursor_item().is_none());
        assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);
        assert!(s.take_container_moves().is_empty());

        // The helm off HeadSlot (1) onto Tabard (19): back to its slot, nothing sent.
        s.run("PickupInventoryItem(1)").unwrap();
        assert!(pickup_inventory_item(&s.lua, 19));
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_autoequips().is_empty());
        assert!(s.take_container_moves().is_empty());
    }

    #[test]
    fn doll_to_doll_ring_swap_fits_via_shared_equip_slots() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());

        assert!(pickup_inventory_item(&s.lua, 11));
        assert_eq!(s.cursor_item().unwrap().equip_slots, vec![11, 12]);

        assert!(pickup_inventory_item(&s.lua, 12));
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

        assert!(!pickup_inventory_item(&s.lua, 1));
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
            occupied_seats: None,
            bonding: 0,
            use_spell: 0,
            use_fork: Default::default(),
            bar_placeable: true,
            consumable: false,
            durability: None,
            flags: 0,
            item_id: 4496,
            icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 1,
            contents_count: None,
            quality: 1,
            name: Some("Small Brown Pouch".into()),
            link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
            guid: 0x110,
            equip_slots: vec![20, 21, 22, 23],
            creator: None,
            gift_creator: None,
            creator_guid: 0,
            gift_creator_guid: 0,
            enchants: Vec::new(),
            spell_0: false,
        });
        s.set_inventory_slots(slots);

        // Ammo (0) is refused; Bag0Slot (20) picks up the equipped bag.
        assert!(!pickup_inventory_item(&s.lua, 0));
        assert!(pickup_inventory_item(&s.lua, 20));
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
                use_fork: Default::default(),
                item_class: None,
                duration_ms: None,
                petition: None,
                already_bound: false,
                occupied_seats: None,
                bonding: 0,
                use_spell: 0,
                bar_placeable: true,
                consumable: false,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
                count: 1,
                quality: Some(1),
                inventory_type: 0,
                item_id: 4496,
                link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
                guid: 0x111,
                equip_slots: vec![20, 21, 22, 23], // INVTYPE_BAG → any bag slot
                cooldown: None,
                readable: false,
                creator: None,
                gift_creator: None,
                creator_guid: 0,
                gift_creator_guid: 0,
                flags: 0,
                enchants: Vec::new(),
                spell_0: false,
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

        assert!(pickup_inventory_item(&s.lua, 21));
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
            spell_id: 1,
            texture: None,
        }));
        assert!(!s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0,
            action: 1,
            texture: None,
            count: 0,
            consumable: false,
        }));
        assert!(!s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());
    }

    #[test]
    fn auto_equip_cursor_item_queues_source_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(auto_equip_cursor_item(&mut s.model_mut()));
        assert!(s.cursor_item().is_none());
        assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);
    }

    /// The doll place locks the occupant after the send's clear (`0x5e0fe7`, then `0x4c78c8`); an
    /// empty slot's zero guid signals nothing (`0x4953ed`), and the moved item keeps its lock
    /// for the server's answer.
    #[test]
    fn a_doll_place_locks_an_occupant_after_its_clear_and_an_empty_slot_signals_nothing() {
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        let slots = doll_slots();
        let head = slots[1].as_ref().unwrap().guid;
        let ring = slots[11].as_ref().unwrap().guid;
        s.set_inventory_slots(slots);
        s.set_container(0, Some(one_fitting_bag_item()));
        let helm = one_fitting_bag_item().slots[&1].guid;
        let place = |s: &UiScript, pick: &str, put: &str| {
            s.run(pick).unwrap();
            heard::listen(
                s,
                &["ITEM_LOCK_CHANGED", "CURSOR_UPDATE"],
                Some("IsInventoryItemLocked(1)"),
            );
            s.run(put).unwrap();
            heard::take(s)
        };
        assert_eq!(
            place(&s, "PickupContainerItem(0, 1)", "PickupInventoryItem(1)"),
            ["CURSOR_UPDATE nil", "ITEM_LOCK_CHANGED 1"],
            "onto the worn helm"
        );
        assert!(s.item_locked(helm) && s.item_locked(head));
        assert_eq!(
            place(&s, "PickupInventoryItem(11)", "PickupInventoryItem(12)"),
            ["CURSOR_UPDATE 1"],
            "the ring onto the empty finger"
        );
        assert!(s.item_locked(ring), "the ring keeps its pickup's lock");
    }

    /// Arrows auto-equip by entry and unlock in the send's clear (`ClearCursor(1,1)`, `0x5e1711`,
    /// `0x5e1763`), on the doll drop and on `AutoEquipCursorItem` alike; anything else keeps its
    /// lock for the server's answer (`0x5e1577`).
    #[test]
    fn auto_equipped_arrows_unlock_and_other_items_keep_their_lock() {
        use crate::script::tick::heard;
        const ARROWS: u64 = 0x4000_0000_0000_0a12;
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        let mut bag = one_fitting_bag_item();
        bag.slots.insert(
            2,
            ContainerSlot {
                item_id: 2512,
                item_class: Some(6),
                inventory_type: 24,
                count: 200,
                guid: ARROWS,
                equip_slots: vec![0],
                ..Default::default()
            },
        );
        s.set_container(0, Some(bag));
        let helm = one_fitting_bag_item().slots[&1].guid;
        for verb in ["PickupInventoryItem(1)", "AutoEquipCursorItem()"] {
            s.run("PickupContainerItem(0, 2)").unwrap();
            heard::listen(&s, &["ITEM_LOCK_CHANGED"], None);
            s.run(verb).unwrap();
            assert_eq!(heard::take(&s), ["ITEM_LOCK_CHANGED"], "{verb}");
            assert!(!s.item_locked(ARROWS), "{verb}: the arrows unlock");
            assert_eq!(s.take_container_autoequips(), vec![(0, 2)], "{verb}");
        }
        s.run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
            .unwrap();
        assert!(s.item_locked(helm), "the helm keeps its lock");
    }

    /// A locked occupant refuses the drop and the payload stays held (`0x4c787b`).
    #[test]
    fn a_drop_on_a_locked_occupant_is_refused() {
        let mut s = UiScript::new().unwrap();
        let slots = doll_slots();
        let guid = slots[1].as_ref().unwrap().guid;
        s.set_inventory_slots(slots);
        s.lock_item(guid);
        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(!pickup_inventory_item(&s.lua, 1));
        assert!(s.cursor_item().is_some(), "kept");
        assert!(s.take_container_moves().is_empty());
    }

    /// From the doll, `AutoEquipCursorItem` puts the item back with `ClearCursor(1,1)`
    /// (`0x5e1567`) and sends nothing; a split carry stays held.
    #[test]
    fn auto_equip_cursor_item_puts_a_worn_item_back_and_keeps_a_split_carry() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());

        s.run("PickupInventoryItem(1)").unwrap();
        assert!(auto_equip_cursor_item(&mut s.model_mut()));
        assert!(s.cursor_item().is_none(), "put back");
        assert!(s.take_container_autoequips().is_empty());

        let mut state = one_fitting_bag_item();
        state.slots.get_mut(&1).unwrap().count = 5;
        s.set_container(0, Some(state));
        s.run("SplitContainerItem(0, 1, 2)").unwrap();
        assert!(!auto_equip_cursor_item(&mut s.model_mut()));
        assert!(s.cursor_item().is_some(), "kept");
        assert!(s.take_container_autoequips().is_empty());
    }

    /// `UseInventoryItem` tests the worn item's lock in its call (`0x4c7af0` → `0x5d8d32`): a
    /// locked item queues nothing, and a pickup after the call does not take the use back.
    #[test]
    fn use_inventory_item_reads_the_lock_in_its_call() {
        let mut s = UiScript::new().unwrap();
        let slots = doll_slots();
        let head = slots[1].as_ref().unwrap().guid;
        s.set_inventory_slots(slots);
        s.lock_item(head);
        s.run("UseInventoryItem(1)").unwrap();
        assert!(s.take_inventory_uses().is_empty(), "locked");
        s.unlock_item(head);
        s.run("UseInventoryItem(1) PickupInventoryItem(1)").unwrap();
        assert_eq!(s.take_inventory_uses(), vec![1], "the use was made first");
        assert!(s.item_locked(head), "then the pickup locked it");
    }

    #[test]
    fn use_inventory_item_queues_the_id() {
        let mut s = UiScript::new().unwrap();
        s.run("UseInventoryItem(1)").unwrap();
        s.run("UseInventoryItem(19)").unwrap();
        assert_eq!(s.take_inventory_uses(), vec![1, 19]);
        assert!(s.take_inventory_uses().is_empty(), "drained");
    }

    /// `onSelf`, argument 2, through the flag coercion (`0x4c8e37`, `0x6f1c10`, default 0).
    #[test]
    fn use_inventory_item_carries_on_self() {
        let mut s = UiScript::new().unwrap();
        for (chunk, on_self) in [
            ("UseInventoryItem(1)", false),
            ("UseInventoryItem(1, nil)", false),
            ("UseInventoryItem(1, 0)", false),
            ("UseInventoryItem(1, 1)", true),
            ("UseInventoryItem(1, true)", true),
        ] {
            s.run(chunk).unwrap();
            assert_eq!(
                s.take_script_calls(),
                [crate::script::ScriptCall::UseInventoryItem {
                    slot: 1,
                    on_self,
                    may_cast: true
                }],
                "{chunk}"
            );
        }
    }

    /// A right-click clears the cursor before it reads the slot (`0x4c79a9`): a held item goes
    /// back, and the worn item is still used.
    #[test]
    fn right_click_puts_a_held_item_back_then_uses_the_worn_one() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.run("PickupInventoryItem(11)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("UseInventoryItem(19)").unwrap();
        assert!(s.cursor_payload().is_none(), "the ring went back");
        assert!(
            !s.eval::<bool>("return IsInventoryItemLocked(11)").unwrap(),
            "its slot unlocks"
        );
        assert!(s.take_container_moves().is_empty(), "never placed");
        assert_eq!(s.take_inventory_uses(), vec![19]);
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

        assert!(!pickup_inventory_item(&s.lua, 1));
        assert!(!pickup_inventory_item(&s.lua, 1));
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
        let slots = doll_slots();
        let guid = slots[1].as_ref().unwrap().guid;
        s.set_inventory_slots(slots);
        s.lock_item(guid);
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
    fn is_inventory_item_locked_reads_the_item_lock() {
        let mut s = UiScript::new().unwrap();
        let slots = doll_slots();
        let guid = slots[19].as_ref().unwrap().guid;
        s.set_inventory_slots(slots);
        s.lock_item(guid); // a send's lock, from the host

        assert!(
            s.eval::<bool>("return IsInventoryItemLocked(19)").unwrap(),
            "host lock"
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

        assert!(equip_cursor_item(&s.lua, 1));
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
    /// The worn item's on-use spell casts through `TryCastSpell`, behind the gate's tag 2 (`0x4c7af0`
    /// → `0x5d8d00` → `0x6e4bc1`); an armed item-targeting spell's pick binds through
    /// `BindSpellTargetObject`, behind it too (`0x4c76df` → `0x495d60` → `0x6e5b9e`).
    #[test]
    fn doll_uses_and_picks_carry_what_the_gate_allowed() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.run("UseInventoryItem(1)").unwrap();
        s.run(r#"RunScript("UseInventoryItem(1)")"#).unwrap();
        s.run_binding(r#"RunScript("UseInventoryItem(1)")"#)
            .unwrap();
        assert_eq!(
            s.take_script_calls()
                .into_iter()
                .map(|c| c.may_cast())
                .collect::<Vec<_>>(),
            [true, false, true]
        );

        s.set_item_pick_armed(true);
        s.run("PickupInventoryItem(1)").unwrap();
        s.run(r#"RunScript("PickupInventoryItem(1)")"#).unwrap();
        s.run_binding(r#"RunScript("PickupInventoryItem(1)")"#)
            .unwrap();
        assert_eq!(
            s.take_item_picks()
                .iter()
                .map(|p| (p.slot, p.may_cast))
                .collect::<Vec<_>>(),
            [(1, true), (1, false), (1, true)]
        );
    }

    #[test]
    fn the_armed_item_half_reroutes_bag_and_doll_clicks() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_container(0, Some(one_fitting_bag_item()));

        // Unarmed, both seams do their ordinary gesture and queue nothing.
        assert!(pickup_inventory_item(&s.lua, 1));
        assert!(s.cursor_item().is_some());
        assert!(s.take_item_picks().is_empty());

        // Armed but holding: the payload wins.
        s.set_item_pick_armed(true);
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(
            s.take_item_picks().is_empty(),
            "a click while carrying an item is a place, not a bind"
        );
        // The server's answer to the swap releases both ends.
        let worn = doll_slots()[1].as_ref().unwrap().guid;
        s.unlock_item(worn);
        s.unlock_item(one_fitting_bag_item().slots[&1].guid);

        // Armed with an empty cursor: both seams bind and leave the cursor empty.
        s.run("ClearCursor()").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(!pickup_inventory_item(&s.lua, 1));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(
            s.take_item_picks(),
            vec![
                ItemPick {
                    bag: EQUIPMENT_BAG,
                    slot: 1,
                    may_cast: true
                },
                ItemPick {
                    bag: 0,
                    slot: 1,
                    may_cast: true
                }
            ],
            "the doll reports in the ONE bag space; the bag reports its own id"
        );
        assert!(s.cursor_item().is_none(), "a bind never stages a payload");
    }

    /// A doll click over a spell, macro, pet action, stabled pet or coins picks the worn item up,
    /// its setter's clear dropping the payload (`0x4c7838`); an empty slot keeps the payload
    /// (`0x4c76af`); an item off the bar and a vendor row with no vendor open stay held
    /// (`0x4c78fd`, `0x4c7916`).
    #[test]
    fn a_doll_click_over_another_payload_picks_up_or_keeps_it() {
        use crate::script::cursor::{CursorAction, CursorMacro, CursorMerchantItem};
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        let macro_payload = CursorPayload::Macro(CursorMacro {
            index: 1,
            texture: None,
        });
        s.set_cursor_for_test(macro_payload.clone());
        assert!(!pickup_inventory_item(&s.lua, 2), "an empty slot");
        assert_eq!(s.cursor_payload(), Some(macro_payload));
        assert!(pickup_inventory_item(&s.lua, 1));
        assert_eq!(s.cursor_item().map(|i| i.slot), Some(1));
        s.run("ClearCursor()").unwrap();

        for held in [
            CursorPayload::Action(CursorAction {
                src_slot: 1,
                kind: 0x80,
                action: 4540,
                texture: None,
                count: 0,
                consumable: false,
            }),
            CursorPayload::Merchant(CursorMerchantItem {
                item_id: 159,
                row: 0,
                texture: None,
            }),
        ] {
            s.set_cursor_for_test(held.clone());
            assert!(!pickup_inventory_item(&s.lua, 1));
            assert_eq!(s.cursor_payload(), Some(held));
        }
    }

    /// A bank item or a bank bag on its slot auto-equips too: `0x5e1480` sends for any item at
    /// a 0-based player slot from 0x17 (bank 39-62, bank bags 63-68) or in a bag.
    #[test]
    fn auto_equip_sends_a_bank_item_and_a_bank_bag() {
        let mut s = UiScript::new().unwrap();
        let mut vault = std::collections::HashMap::new();
        vault.insert(
            3,
            ContainerSlot {
                item_id: 2000,
                count: 1,
                equip_slots: vec![1],
                ..Default::default()
            },
        );
        s.set_container(
            -1,
            Some(ContainerState {
                name: None,
                num_slots: 24,
                slots: vault,
            }),
        );
        s.run("PickupContainerItem(-1, 3)").unwrap();
        assert!(auto_equip_cursor_item(&mut s.model_mut()));
        assert_eq!(s.take_container_autoequips(), vec![(-1, 3)]);
        assert!(s.cursor_item().is_none());

        s.set_cursor_for_test(CursorPayload::Item(crate::script::cursor::CursorItem {
            item_class: None,
            guid: 0x112,
            bag: EQUIPMENT_BAG,
            slot: 64,
            item_id: 4500,
            texture: None,
            link: None,
            count: None,
            quality: None,
            bar_placeable: false,
            consumable: false,
            equip_slots: vec![20, 21, 22, 23],
        }));
        assert!(auto_equip_cursor_item(&mut s.model_mut()));
        assert_eq!(s.take_container_autoequips(), vec![(EQUIPMENT_BAG, 64)]);
    }

    /// `UseInventoryItem` reaches the dispatcher too (`0x4c7af0`), so a worn trinket meets its
    /// requirement and control gates (`0x5d8e0e`, `0x5d8eaa`); a bypass spell passes the latter.
    #[test]
    fn a_worn_item_use_meets_the_dispatchers_gates() {
        let mut s = UiScript::new().unwrap();
        let mut doll: crate::script::InventorySlots = Default::default();
        doll[13] = Some(InvSlotView {
            item_id: 871,
            guid: 0x31,
            link: Some("|Hitem:871:0:0:0|h[Trinket]|h".into()),
            use_spell: 7744,
            ..Default::default()
        });
        s.set_inventory_slots(doll);
        s.set_player_control(false);
        s.run("UseInventoryItem(13)").unwrap();
        assert_eq!(s.take_ui_errors(), vec!["ERR_CLIENT_LOCKED_OUT"]);
        assert!(s.take_script_calls().is_empty());

        s.set_control_bypass([7744].into());
        s.run("UseInventoryItem(13)").unwrap();
        assert!(s.take_ui_errors().is_empty());
        assert_eq!(s.take_script_calls().len(), 1);

        s.set_control_bypass(Default::default());
        s.set_player_control(true);
        s.set_player_req_state(crate::script::PlayerReqState {
            level: 10,
            class_id: 1,
            race_id: 1,
            ..Default::default()
        });
        s.set_item_template(
            871,
            crate::script::ItemTemplateView {
                required_level: 60,
                allowable_class: -1,
                allowable_race: -1,
                ..Default::default()
            },
        );
        s.run("UseInventoryItem(13)").unwrap();
        assert_eq!(s.take_ui_errors(), vec!["ERR_CANT_USE_ITEM"]);
        assert!(s.take_script_calls().is_empty());
    }

    /// `CGItem::Use` returns silently for a container (`0x5d8d26`), ahead of both gates.
    #[test]
    fn a_worn_container_meets_no_gate() {
        let mut s = UiScript::new().unwrap();
        let mut doll: crate::script::InventorySlots = Default::default();
        doll[13] = Some(InvSlotView {
            item_id: 4500,
            guid: 0x31,
            link: Some("|Hitem:4500:0:0:0|h[Bag]|h".into()),
            contents_count: Some(0),
            ..Default::default()
        });
        s.set_inventory_slots(doll);
        s.set_player_control(false);
        s.run("UseInventoryItem(13)").unwrap();
        assert!(s.take_ui_errors().is_empty());
    }
}
