//! The paper doll (decision 0208 phase 1b): equipment joins the ONE payload space via
//! [`super::EQUIPMENT_BAG`]. One extra transition ([`pickup_inventory_item`]) plus four small
//! satellites ([`equip_cursor_item`], [`cursor_can_go_in_slot`], [`auto_equip_cursor_item`],
//! [`use_inventory_item`], [`is_inventory_item_locked`]) — everything else (cancel, clear, the
//! world drop, `SplitContainerItem`) already generalizes from [`super`] because it only ever
//! matches on the `CursorPayload` enum, never on `bag`.

use mlua::Lua;

use crate::script::binding_abi::flag;
use crate::script::container::ContainerMove;
use crate::script::Model;

use super::{queue_cursor_update, queue_lock_changed, CursorItem, CursorPayload, EQUIPMENT_BAG};

/// `PickupInventoryItem(id)` — the paper-doll slot's left-click entry point (decision 0208 phase
/// 1b). Mirrors `container::pickup_container_item`'s click model exactly, keyed on the sentinel
/// [`EQUIPMENT_BAG`] rather than a real bag id (decision 0216 §1's ONE payload space, extended to
/// equipment). `id` is the 1-based live-API inventory slot (`GetInventorySlotInfo`'s own
/// numbering — HeadSlot=1 … TabardSlot=19, the four equipped-bag icons Bag0Slot=20 … Bag3Slot=23).
/// The bag icons ride the SAME transition (a bag's `equip_slots` is `[20,21,22,23]` from
/// `find_equip_slot(INVTYPE_BAG)`, so the fit rule already routes a bag onto any bag slot, and
/// `wire_pos`'s `EQUIPMENT_BAG` arm maps live id 20..23 → the wire's equipped-bag inventory slots
/// 19..22); ammo (0) stays a named deferral — refused outright, cursor untouched.
///
/// **The bank-bag band (64..=69) rides this too**, and only through one door: `PutItemInBag`'s
/// empty-slot leg IS this function in the reference (`0x4c7c00` calls `0x4c7300` there), which is
/// how a held bag reaches a bank bag slot at all. Its fit rule is one band wider than
/// `equip_slots` alone — see [`super::bag_verbs::fits_slot`].
///
/// - empty cursor + an occupied, UNLOCKED doll slot → picks it up (payload `Item { bag:
///   EQUIPMENT_BAG, slot: id, … }`), the same lock/event pair as a bag pickup.
/// - holding + the SAME slot → cancel (mirrors `pickup_container_item`'s own same-slot branch).
/// - holding an Item payload whose `equip_slots` contains `id` (the fit rule — `equip_slots` rides
///   the payload from wherever it was picked up, doll or bag alike) → queues the move (dst =
///   `(EQUIPMENT_BAG, id)`) and CLEARS (a plain clear, no hop) — UNLESS it's a
///   split carry (`count: Some(_)`): refused outright, kept (you can't equip a partial stack).
/// - holding a non-fitting Item, or any Spell/Action payload → no-op, kept (mirrors
///   `pickup_container_item`'s own refusal of a payload a bag slot can't take).
///
/// Returns whether the caller should repaint (the source lock, or the held payload, changed).
pub(super) fn pickup_inventory_item(model: &mut Model, id: u32) -> bool {
    // Equipment slots (1..=19) + the four equipped-bag icons (20..=23) + the six BANK bag slots
    // (64..=69, reached only through `PutItemInBag`); ammo (0) is out of scope.
    if !(1..=23).contains(&id) && !super::bag_verbs::BANK_BAG_INV_SLOTS.contains(&id) {
        return false;
    }
    // The targeting cursor's item half, on the doll. The reference's doll pickup
    // `0x4c7300` carries the byte-IDENTICAL rung to the bag one — `4c76df: call 0x6e48a0`
    // (IsTargeting), `4c76e8: call 0x6e6330` (TargetingWantsItem), `4c76fb: call 0x495d60` (bind
    // this item), then return with nothing picked up — and it is what makes poisoning or
    // sharpening the weapon you are *wearing* possible at all. Reported in the ONE bag space
    // ([`EQUIPMENT_BAG`] + the 1-based slot), so the app's drain resolves both seams identically.
    // A held payload wins, exactly as in the bag path (the ref's own `4c73af` pair precedes it).
    if model.item_pick_armed && model.cursor.is_none() {
        model.item_picks.push((EQUIPMENT_BAG, id));
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
            // A split carry can't equip a partial stack, and a non-fitting item has nowhere to
            // land on this slot — both refuse, kept exactly as picked up (no event: nothing
            // transitioned).
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
        // Mode 5 — the buy, aimed at an equipment slot. `0x4c78fd` tests `[0xb4d900] == 5` inside
        // the paper-doll click and sends the same `CMSG_BUY_ITEM_IN_SLOT 0x1a3` the bag drop does;
        // the server equips it or refuses. Reported in the ONE bag space ([`EQUIPMENT_BAG`] + the
        // 1-based inventory slot), the seam this file already uses for every other doll intent.
        //
        // Of the reference's three drop consumers this is the one that handles a stale vendor row
        // correctly — the other two dereference it (`0x4f9f35`, `0x4c7ea5`) — so its refusal is
        // what we reproduce in all three.
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
            // Mode 10 — a stabled pet refuses a bag/doll slot exactly as the
            // spell/action family does, and stays on the cursor for the stable window to take.
            | CursorPayload::StablePet(_)
            // Mode 2 (1962) — coins have no slot to land in; a money frame's DropFunc takes them.
            | CursorPayload::Money(_)),
        ) => {
            model.cursor = Some(other);
            false
        }
    }
}

/// `EquipCursorItem(id)` — the ref's item-context "Equip" popup entry (not built this slice: no
/// context menu ships yet). Same transition as placing the held item directly onto doll slot
/// `id` — [`pickup_inventory_item`]'s place arm already IS that transition, so this simply routes
/// there (decision 0208 phase 1b: "route both through the same transition").
pub(super) fn equip_cursor_item(model: &mut Model, id: u32) -> bool {
    pickup_inventory_item(model, id)
}

/// `CursorCanGoInSlot(id)` — whether the held payload could be dropped on doll slot `id`
/// (decision 0208 phase 1b; the reference's `CURSOR_UPDATE` → `LockHighlight`/`UnlockHighlight`
/// driver, `PaperDollFrame.lua:609-615`). An Item payload whose `equip_slots` contains `id`;
/// empty cursor or a Spell/Action payload both answer `false` (the same fit-blind refusal
/// [`pickup_inventory_item`] gives them — asking a hypothetical doesn't relax the rule).
///
/// INTERIM: the byte body's terminal equip-fit check (`0x5da1d0`) was left unresolved by 0216 §5
/// (0218's own residual, §4 "confirmed as built" — everything else on that list is pinned, this
/// one function wasn't among the byte-verified findings). `equip_slots` derives from the item
/// template's `inventoryType` via the SERVER's own mapping instead (`ui_items::find_equip_slot`,
/// transcribed from vmangos `Player::FindEquipSlot`/`ItemPrototype::GetAllowedEquipSlots`) — a
/// verifiable authority `SMSG_INVENTORY_CHANGE_FAILURE` referees either way, corrected if a
/// future pin disagrees with the client's own table.
pub(super) fn cursor_can_go_in_slot(model: &Model, id: u32) -> bool {
    match &model.cursor {
        Some(CursorPayload::Item(item)) => super::bag_verbs::fits_slot(item, id),
        _ => false,
    }
}

/// `AutoEquipCursorItem()` — the model-pane's click-with-payload path (decision 0208 phase 1b,
/// ref `PaperDollFrame.xml`'s frame-level `OnReceiveDrag`/`OnMouseUp`): an Item payload picked up
/// from a CONTAINER (`bag >= 0`, a whole stack) queues its `(bag, slot)` source as a NEW
/// `container_autoequips` intent (the app sends `CMSG_AUTOEQUIP_ITEM` — the server picks the
/// destination slot itself and swaps any displaced piece back, vmangos `ItemHandler.cpp:138-228`)
/// and clears the cursor — a plain clear, the same contract as every other placement since
/// decision 0218. A payload already carried FROM the equipment (`bag == EQUIPMENT_BAG` — a
/// doll→doll re-drop has nothing for the server to "auto" pick) or a split carry (can't
/// auto-equip a partial stack) is a no-op, kept; so is any Spell/Action payload.
///
/// Returns whether the caller should repaint.
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

/// `UseInventoryItem(id)` — the doll slot's right-click entry point (ref
/// `PaperDollItemSlotButton_OnClick`, `PaperDollFrame.lua:658-659`): queues `id` for the app to
/// resolve to the equipped item's guid and send (`CMSG_USE_ITEM`, bag 255 + the 0-based wire slot
/// — vmangos `HandleUseItemOpcode` takes equipped positions the same as bag ones). No engine-side
/// refusal — an empty/out-of-range slot is a harmless no-op the app's drain silently drops
/// (mirrors `UseContainerItem`'s own contract: it never checks the slot either).
pub(super) fn use_inventory_item(model: &mut Model, id: u32) {
    model.inventory_uses.push(id);
}

/// `IsInventoryItemLocked(id)` — true while `id` is the ACTIVE cursor payload's source (the
/// picked-up slot dims immediately, no server round-trip — the doll twin of
/// `GetContainerItemInfo`'s `held_here` derivation) OR the app's fed `InvSlotView.locked` says so
/// (an outstanding pending op the app's `PendingItemOps` tracks).
///
/// Reads through [`Model::inv_slot`], not the doll array, so it answers over the WHOLE live-id
/// space that binding covers — the reference's `BankFrameItemButton_UpdateLock` calls it with
/// `BankButtonIDToInvSlotID(...)` for every bank slot, and a lock the vault feed reports has to
/// dim the button the same as an equipped one.
pub(super) fn is_inventory_item_locked(model: &Model, id: u32) -> bool {
    let held_here = matches!(&model.cursor, Some(CursorPayload::Item(c)) if c.bag == EQUIPMENT_BAG && c.slot == id);
    let fed = usize::try_from(id)
        .ok()
        .and_then(|i| model.inv_slot("player", i))
        .is_some_and(|s| s.locked);
    held_here || fed
}

/// Register the paper-doll's cursor globals — all top-level, matching the reference
/// (`PickupInventoryItem` &c. are not namespaced any more than `PickupContainerItem`'s cursor
/// siblings are).
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

    /// Slots 1 (Head) and 19 (Tabard) occupied, plus the two finger rings (11/12) — a doll
    /// fixture wide enough to exercise the fit rule, the same-slot cancel, and a doll↔doll swap.
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
            equip_slots: vec![11, 12], // FINGER1|FINGER2 — the doll↔doll swap fixture
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

        // Ammo (0) is still refused; the bag icon (20) now picks up the equipped bag.
        assert!(!s.eval::<bool>("return PickupInventoryItem(0)").unwrap());
        assert!(s.eval::<bool>("return PickupInventoryItem(20)").unwrap());
        let held = s.cursor_item().expect("bag picked up from the bar");
        assert_eq!(
            (held.bag, held.slot, held.item_id),
            (EQUIPMENT_BAG, 20, 4496)
        );
    }

    /// Drag-to-equip: a bag carried from the backpack drops onto an empty bag slot (id 21), queuing
    /// the move to `(EQUIPMENT_BAG, 21)` — the same transition the paper doll uses, and the wire the
    /// app maps onto equipped-bag inventory slot 20.
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

        // Empty cursor: false everywhere.
        assert!(!s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());

        // Item arm: true only for a slot its equip_slots names.
        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());
        assert!(!s.eval::<bool>("return CursorCanGoInSlot(2)").unwrap());
        s.run("ClearCursor()").unwrap();

        // Spell/Action arms: always false (the fit-blind refusal).
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

        // From the doll itself: nothing for the server to "auto" pick.
        s.run("PickupInventoryItem(1)").unwrap();
        assert!(!s.eval::<bool>("return AutoEquipCursorItem()").unwrap());
        assert!(s.cursor_item().is_some(), "kept");
        assert!(s.take_container_autoequips().is_empty());
        s.run("ClearCursor()").unwrap();

        // A split carry: can't auto-equip a partial stack.
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

    /// The targeting cursor's item half reroutes BOTH pickup seams, and the
    /// held-payload check precedes it in both — the reference's own order (`4f9c38` before
    /// `4f9c54`; `4c73af` before `4c76df`). Without the doll seam a rogue could not poison the
    /// weapon they are wearing; without the payload gate, dragging an item across a bag while a
    /// poison is armed would silently fire the cast at whatever you dropped it on.
    #[test]
    fn the_armed_item_half_reroutes_bag_and_doll_clicks() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_container(0, Some(one_fitting_bag_item()));

        // Unarmed, both seams do their ordinary gesture and queue nothing.
        assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(s.cursor_item().is_some());
        assert!(s.take_item_picks().is_empty());

        // Armed but HOLDING: the payload wins, exactly as the reference orders it.
        s.set_item_pick_armed(true);
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(
            s.take_item_picks().is_empty(),
            "a click while carrying an item is a place, not a bind"
        );

        // Armed with an empty cursor: both seams bind instead of picking up, and neither
        // disturbs the cursor.
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
