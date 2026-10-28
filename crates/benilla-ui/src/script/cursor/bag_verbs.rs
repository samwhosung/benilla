//! The bag-slot verbs: `PutItemInBag`, `PutItemInBackpack` and `PickupBagFromSlot`.

use mlua::{Lua, MultiValue, Value};

use crate::script::container::ContainerMove;
use crate::script::merchant::VendorAtCall;
use crate::script::tick::verb;
use crate::script::{ItemCall, Model};

use super::doll::{doll_click, DollClick};
use super::{
    clear, clear_cursor, item_locked, lock_item, pick_up_item, Clear, CursorItem, CursorPayload,
    EQUIPMENT_BAG,
};

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

/// Where the bag worker `0x4c7c00` puts the held item: a bag slot by its live id, or the backpack,
/// whose thunk passes `0xFF` (`0x4c7ed0`) for the player's own guid (`0x4c7c75`-`0x4c7c82`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dest {
    Slot(u32),
    Backpack,
}

/// `PutItemInBag(inventorySlot)` (`0x4c7c00`): true (the reference's `1`) when the click was
/// consumed against an occupied slot, else false (nil). An empty cursor's nil is what lets
/// `BagSlotButton_OnClick` open the bag.
pub(super) fn put_item_in_bag(
    lua: &Lua,
    model: &mut Model,
    live: u32,
    vendor: VendorAtCall,
) -> bool {
    accepts(live) && put_item(lua, model, Dest::Slot(live), vendor)
}

/// `PutItemInBackpack()`, the worker on the player's own guid (`0x4c7ed0`): the empty-slot leg is
/// unreachable and `0x4c7d3e` skips the swap, so even a bag is stored into the backpack.
pub(super) fn put_item_in_backpack(lua: &Lua, model: &mut Model, vendor: VendorAtCall) -> bool {
    put_item(lua, model, Dest::Backpack, vendor)
}

/// The worker `0x4c7c00`, in its order:
/// - player control, and an open bank for a bank bag slot (`0x4c7c33`, `0x4c7c40`-`0x4c7c55`);
/// - an empty bag slot goes to the doll click (`0x4c7ca7`), answering nil (`0x4c7cae`);
/// - the held item is the destination bag itself: a cancel (`0x4c7cb5`-`0x4c7cca`);
/// - holding no item and no vendor row: `ClearCursor(1,1)` and nil (`0x4c7cdb`-`0x4c7ce4` →
///   `0x4c7eaf`), so a spell, coins or a bar item is dropped;
/// - an item held from this very bag, or from the backpack onto the backpack, cancels
///   (`0x4c7cea`-`0x4c7d11`);
/// - a vendor row buys into the bag (`0x4c7e66`), then clears and answers nil;
/// - a bag held from a bag slot, or any container, swaps (`0x4c7d44`-`0x4c7df5`), a backpack
///   destination excepted (`0x4c7d3e`); anything else is stored (`0x4c7e0c`-`0x4c7e55`).
fn put_item(lua: &Lua, model: &mut Model, dest: Dest, vendor: VendorAtCall) -> bool {
    if !model.player_control {
        return false;
    }
    let bag_guid = match dest {
        Dest::Backpack => None,
        Dest::Slot(live) => {
            if BANK_BAG_INV_SLOTS.contains(&live) && model.bank.is_none() {
                return false;
            }
            let occupant = usize::try_from(live)
                .ok()
                .and_then(|slot| crate::script::char_stats::own_inv_view(lua, model, slot))
                .map(|s| s.guid);
            let Some(occupant) = occupant else {
                // The doll click on the empty slot: it places or buys what is held, and with
                // nothing held finds nothing to pick up, so it owes no pickup. A held item takes
                // the click before an armed item pick (`0x4c769a`), so no gate is reached.
                // A swap into the empty slot has no occupant to lock.
                let click = doll_click(lua, model, live, true, vendor);
                debug_assert!(matches!(click, DollClick::Done(_) | DollClick::Swapped(0)));
                return false;
            };
            Some(occupant)
        }
    };
    match model.cursor.clone() {
        // The held bag is this slot's own: `ClearCursor(1,1)` (`0x4c7cca`), answer 1.
        Some(CursorPayload::Item(held)) if bag_guid.is_some_and(|g| g == held.guid) => {
            clear_cursor(model);
            true
        }
        Some(CursorPayload::Item(held)) => place_held(lua, model, dest, held),
        // Mode 5 with a vendor open: one of the row's item into the bag, slot `0xFF`, count 1
        // (`0x4c7e6f`-`0x4c7eaa`), the item the row names now. Deviation: a row index the list no
        // longer holds buys nothing and drops, where this site reads through a null row
        // (`0x4c7e9a`, `0x4c7ea5`).
        Some(CursorPayload::Merchant(_)) => {
            if let Some(entry) = vendor.held_entry {
                model.merchant_call_in_verb(crate::script::MerchantCall::BuyIntoBag {
                    bag_guid,
                    entry,
                });
            }
            clear_cursor(model);
            false
        }
        _ => {
            clear_cursor(model);
            false
        }
    }
}

/// The worker's item legs past its cancels' guid test (`0x4c7cea` on).
fn place_held(lua: &Lua, model: &mut Model, dest: Dest, held: CursorItem) -> bool {
    // A held item that lives in this very bag cancels: its container guid is the destination's
    // (`0x4c7cea`-`0x4c7cf6`), the backpack's being the player's, where only an item from the
    // backpack's own slots 23-38 cancels (`0x4c7cf8`-`0x4c7d0f`).
    let own_bag = match dest {
        Dest::Slot(live) => autostore_container(live) == Some(held.bag),
        Dest::Backpack => held.bag == 0,
    };
    if own_bag {
        clear_cursor(model);
        return true;
    }
    // The swap fork: the held item comes from the player's own bag slots 19-22 (`0x4c7d44`-
    // `0x4c7d5e`, 20-23 here) or is a container (`0x4c7dc4`). Any other item, a worn helm
    // included, is stored; the backpack skips the fork (`0x4c7d3e`).
    if let Dest::Slot(live) = dest {
        let from_bag_slot = held.bag == EQUIPMENT_BAG && (20..=23).contains(&held.slot);
        if from_bag_slot || is_container(&held) {
            // A swap: `LockItem` on the occupant (`0x4c7d62`, `0x4c7dca`), then `SwapItem`
            // (`0x4c7d8d`, `0x4c7df5`), whose send clears (`ClearCursor(0,1)`, `0x5e0fe7`) and
            // whose question leaves the bag held; nothing clears after it. Neither leg reads the
            // split count, so a split carry swaps its whole stack.
            let occupant = usize::try_from(live)
                .ok()
                .and_then(|slot| crate::script::char_stats::own_inv_view(lua, model, slot))
                .map_or(0, |s| s.guid);
            lock_item(model, occupant);
            super::swap::swap_and_file(
                lua,
                model,
                ContainerMove {
                    src_bag: held.bag,
                    src_slot: held.slot,
                    dst_bag: EQUIPMENT_BAG,
                    dst_slot: live,
                    count: None,
                },
            );
            return true;
        }
    }
    // Store into the bag: `CMSG_AUTOSTORE_BAG_ITEM` (`0x10B`), or `CMSG_SPLIT_ITEM` (`0x10E`) for
    // a split. The client picks no slot: the store carries none, the split carries `0xFF`.
    let dst_bag = match dest {
        Dest::Backpack => 0,
        Dest::Slot(live) => match autostore_container(live) {
            Some(bag) => bag,
            None => return false,
        },
    };
    model.item_call(ItemCall::AutoStore(crate::script::BagAutoStore {
        src_bag: held.bag,
        src_slot: held.slot,
        dst_bag,
        count: held.count,
    }));
    // `ClearCursor(0,1)` past the send (`0x4c7e55`), locking nothing new.
    clear(model, Clear::KEEP_LOCK);
    true
}

/// `PickupBagFromSlot(inventorySlot)` (`0x4c8fa0` into `0x4c7b00`). Past its gates, player
/// control and an open bank for a bank bag slot (`0x4c7b33`, `0x4c7b40`-`0x4c7b55`), it clears
/// the cursor only when an item is held (`0x4c7b6a`-`0x4c7b79`), so a spell or coins survive a
/// pickup that does not happen. An empty slot goes to the paper-doll handler (`0x4c7b9b`); an
/// occupant that is no container, or is locked, returns (`0x4c7bbf`, `0x4c7bc8`); a container is
/// picked up, its setter's clear dropping any other payload. It never places or swaps, and the
/// client has no empty-bag rule: the server refuses with `EQUIP_ERR_CAN_ONLY_DO_WITH_EMPTY_BAGS`.
pub(super) fn pickup_bag_from_slot(lua: &Lua, live: u32) -> mlua::Result<()> {
    if !accepts(live) {
        return Ok(());
    }
    let (gated, occupied) = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let gated =
            !model.player_control || (BANK_BAG_INV_SLOTS.contains(&live) && model.bank.is_none());
        let occupied = usize::try_from(live)
            .ok()
            .and_then(|slot| crate::script::char_stats::own_inv_view(lua, &model, slot))
            .is_some_and(|s| s.item_id != 0);
        (gated, occupied)
    };
    if gated {
        return Ok(());
    }
    verb(lua, |model| {
        if matches!(model.cursor, Some(CursorPayload::Item(_))) {
            clear_cursor(model);
        }
    });
    if !occupied {
        super::doll::pickup_inventory_item(lua, live)?;
        return Ok(());
    }
    let picked = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        grab_bag(lua, &model, live)
    };
    if let Some(item) = picked {
        pick_up_item(lua, item);
    }
    Ok(())
}

/// `PickupBagFromSlot`'s grab: an unlocked container in `live` (`0x4c7bc1`), which it picks up
/// with `SetCursorItem(1, …)` and `LockItem` (`0x4c7be2`, `0x4c7be9`).
fn grab_bag(lua: &Lua, model: &Model, live: u32) -> Option<CursorItem> {
    usize::try_from(live)
        .ok()
        .and_then(|slot| crate::script::char_stats::own_inv_view(lua, model, slot))
        .filter(|s| s.item_id != 0 && !item_locked(model, s.guid))
        .map(|s| CursorItem {
            item_class: None,
            guid: s.guid,
            bag: EQUIPMENT_BAG,
            slot: live,
            item_id: s.item_id,
            texture: s.icon.clone(),
            link: s.link.clone(),
            quality: Some(u32::try_from(s.quality).unwrap_or(0)),
            count: None,
            bar_placeable: s.bar_placeable,
            consumable: s.consumable,
            equip_slots: s.equip_slots.clone(),
        })
        .filter(is_container)
}

/// The 1-based slot of a bag verb: the slot reader `0x4c8520` passes it and its 0-based slot is
/// `0x13` or more, else `message`.
fn bag_slot(lua: &Lua, v: Value, message: &'static str) -> mlua::Result<u32> {
    match crate::script::inventory_verbs::inventory_slot_arg(lua, v, message) {
        Ok(slot0) if slot0 >= 0x13 => Ok((slot0 + 1) as u32),
        _ => Err(mlua::Error::RuntimeError(message.into())),
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
        lua.create_function(|lua, live: Value| {
            // `Invalid bag slot in PutItemInBag` (`0x848b84`) unless the slot reader passes and
            // the slot is a bag's or beyond, Lua 20 up (`0x4c8f13`, `0x4c8f1f`).
            let live = bag_slot(lua, live, "Invalid bag slot in PutItemInBag")?;
            let vendor = crate::script::merchant::at_call(lua);
            let answer = verb(lua, |model| put_item_in_bag(lua, model, live, vendor));
            consumed(answer)
        })?,
    )?;
    g.set(
        "PutItemInBackpack",
        lua.create_function(|lua, ()| {
            let vendor = crate::script::merchant::at_call(lua);
            let answer = verb(lua, |model| put_item_in_backpack(lua, model, vendor));
            consumed(answer)
        })?,
    )?;
    g.set(
        "PickupBagFromSlot",
        lua.create_function(|lua, live: Value| {
            // `Invalid bag slot in PickupBagFromSlot` (`0x848ba8`) on the same two tests
            // (`0x4c8fb3`, `0x4c8fbf`).
            let live = bag_slot(lua, live, "Invalid bag slot in PickupBagFromSlot")?;
            pickup_bag_from_slot(lua, live)
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::cursor::EQUIPMENT_BAG;
    use crate::script::tick::heard;
    use crate::script::{
        BagAutoStore, BankBagSlots, ContainerMove, ContainerSlot, ContainerState, InvSlotView,
        InventorySlots, UiScript,
    };

    /// With nothing held the reference skips the clear (`0x4c7b6a`-`0x4c7b70`), so a drag from an
    /// empty bag slot signals nothing.
    #[test]
    fn an_empty_cursor_skips_the_clear_and_its_signal() {
        let s = UiScript::new().unwrap();
        heard::listen(&s, &["CURSOR_UPDATE"], None);
        s.run("PickupBagFromSlot(20)").unwrap();
        assert!(s.cursor_payload().is_none());
        assert!(heard::take(&s).is_empty());
    }

    /// An item that is a bag: `equip_slots` is `find_equip_slot(INVTYPE_BAG)`.
    fn bag_slot_view(item_id: u32) -> InvSlotView {
        InvSlotView {
            item_id,
            icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 1,
            equip_slots: vec![20, 21, 22, 23],
            guid: 0x8000_0000 | u64::from(item_id),
            // `INVTYPE_BAG`, the record cached.
            inventory_type: Some(18),
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
                guid: 0x201,
                // The record cached.
                quality: Some(1),
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
                guid: 0x202,
                // The record cached.
                quality: Some(1),
                ..Default::default()
            },
        );
        ContainerState {
            name: None,
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
        s.keep_calls();
        s.set_bank(Some(Default::default()));
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(Default::default());
        s.seat_item_records();
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

    /// An empty slot goes to the doll handler (`0x4c7c00` into `0x4c7300`), where an ordinary item
    /// misfits a bag slot and is auto-equipped (`0x4c78ef`); the answer stays nil.
    #[test]
    fn an_empty_bag_slot_auto_equips_an_ordinary_item() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_bank(Some(Default::default()));
        s.set_container(0, Some(backpack()));
        s.seat_item_records();
        s.run("PickupContainerItem(0, 2)").unwrap();

        assert!(s.eval::<bool>("return PutItemInBag(64) == nil").unwrap());
        assert!(s.cursor_item().is_none());
        assert_eq!(s.take_container_autoequips(), vec![(0, 2)]);
        assert!(s.take_container_moves().is_empty());
        assert!(s.take_bag_autostores().is_empty());
    }

    #[test]
    fn an_occupied_bag_slot_swaps_with_a_held_bag_and_answers_one() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_bank(Some(Default::default()));
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(one_bank_bag());
        s.seat_item_records();
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
        s.keep_calls();
        s.set_bank(Some(Default::default()));
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

    /// The swap locks the occupant first (`0x4c7d62`, `0x4c7dca`), its `ITEM_LOCK_CHANGED` ahead
    /// of the send's clear (`0x5e0fe7`); a store into a bag (`0x4c7e55`) and the doll's
    /// auto-equip clear with no lock event, the held item keeping its lock.
    #[test]
    fn only_the_swap_signals_a_lock_and_it_leads_the_clear() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_bank(Some(Default::default()));
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(one_bank_bag());
        s.seat_item_records();
        let place = |s: &UiScript, pick: &str, put: &str| {
            s.run(pick).unwrap();
            heard::listen(
                s,
                &["ITEM_LOCK_CHANGED", "CURSOR_UPDATE"],
                Some("IsInventoryItemLocked(64)"),
            );
            s.run(put).unwrap();
            heard::take(s)
        };
        assert_eq!(
            place(&s, "PickupContainerItem(0, 1)", "PutItemInBag(64)"),
            ["ITEM_LOCK_CHANGED 1", "CURSOR_UPDATE 1"],
            "the swap"
        );
        assert!(s.eval::<bool>("return IsInventoryItemLocked(64)").unwrap());
        // The server's answer releases both ends.
        s.unlock_item(0x201);
        s.unlock_item(0x8000_0000 | 4500);
        let locked = |s: &UiScript| {
            s.eval::<bool>("local _, _, l = GetContainerItemInfo(0, 2) return l == 1")
                .unwrap()
        };
        assert_eq!(
            place(&s, "PickupContainerItem(0, 2)", "PutItemInBag(64)"),
            ["CURSOR_UPDATE nil"],
            "the store into the bank bag"
        );
        assert!(locked(&s), "the stored item keeps its pickup's lock");
        s.unlock_item(0x202);
        assert_eq!(
            place(&s, "PickupContainerItem(0, 2)", "PutItemInBackpack()"),
            ["ITEM_LOCK_CHANGED nil", "CURSOR_UPDATE nil"],
            "a backpack item onto the backpack is a cancel (`0x4c7d11`), which unlocks"
        );
        assert!(!locked(&s));
        s.unlock_item(0x202);
        s.set_bank_bag_slots(Default::default());
        assert_eq!(
            place(&s, "PickupContainerItem(0, 2)", "PutItemInBag(64)"),
            ["CURSOR_UPDATE nil"],
            "the doll's auto-equip of a misfit (`0x4c78ef`)"
        );
        assert!(locked(&s));
    }

    /// A worn non-bag item dropped on an occupied bag slot is stored in that bag
    /// (`0x4c7e06` into `0x5e12e0`), not swapped: only an item held from the bag slots 19-22 or a
    /// container swaps (`0x4c7d44`-`0x4c7d5e`, `0x4c7dc4`).
    #[test]
    fn a_worn_item_on_an_occupied_bag_slot_is_stored_not_swapped() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut doll: InventorySlots = Default::default();
        doll[1] = Some(InvSlotView {
            item_id: 1234,
            icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
            count: 1,
            equip_slots: vec![1],
            guid: 0x301,
            ..Default::default()
        });
        doll[20] = Some(bag_slot_view(4500));
        s.set_inventory_slots(doll);
        s.run("PickupInventoryItem(1)").unwrap();
        assert_eq!(s.cursor_item().map(|i| i.bag), Some(EQUIPMENT_BAG));

        assert_eq!(s.eval::<i64>("return PutItemInBag(20)").unwrap(), 1);
        assert!(s.take_container_moves().is_empty(), "a store, not a swap");
        assert_eq!(
            s.take_bag_autostores(),
            vec![BagAutoStore {
                src_bag: EQUIPMENT_BAG,
                src_slot: 1,
                dst_bag: 1,
                count: None,
            }]
        );
        assert!(s.cursor_item().is_none());
    }

    /// A bag held from its own bag slot still swaps (`0x4c7d44`-`0x4c7d5e`).
    #[test]
    fn a_bag_held_from_a_bag_slot_swaps_with_an_occupied_one() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut doll: InventorySlots = Default::default();
        doll[20] = Some(bag_slot_view(4500));
        doll[21] = Some(bag_slot_view(4501));
        s.set_inventory_slots(doll);
        s.run("PickupInventoryItem(20)").unwrap();

        assert_eq!(s.eval::<i64>("return PutItemInBag(21)").unwrap(), 1);
        assert!(s.take_bag_autostores().is_empty());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: EQUIPMENT_BAG,
                src_slot: 20,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 21,
                count: None,
            }]
        );
    }

    /// An item dropped on the bag it already lives in is a cancel (`0x4c7cea`-`0x4c7d11`): the
    /// cursor clears with an unlock and nothing goes out, where a store would name the same bag.
    #[test]
    fn an_item_dropped_on_its_own_bag_cancels() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut doll: InventorySlots = Default::default();
        doll[20] = Some(bag_slot_view(4500));
        s.set_inventory_slots(doll);
        s.set_container(1, Some(backpack()));
        s.run("PickupContainerItem(1, 2)").unwrap();
        assert!(s.item_locked(0x202));

        assert_eq!(s.eval::<i64>("return PutItemInBag(20)").unwrap(), 1);
        assert!(s.cursor_item().is_none());
        assert!(!s.item_locked(0x202), "ClearCursor(1,1) unlocked it");
        assert!(s.take_bag_autostores().is_empty());
        assert!(s.take_container_moves().is_empty());

        // Another bag's button is still a store.
        let mut doll: InventorySlots = Default::default();
        doll[21] = Some(bag_slot_view(4501));
        s.set_inventory_slots(doll);
        s.run("PickupContainerItem(1, 2)").unwrap();
        assert_eq!(s.eval::<i64>("return PutItemInBag(21)").unwrap(), 1);
        assert_eq!(s.take_bag_autostores().len(), 1);
    }

    /// The backpack skips the swap fork (`0x4c7d3e`): a bag held from its bag slot is stored.
    #[test]
    fn the_backpack_button_always_auto_stores_even_a_bag() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut doll: InventorySlots = Default::default();
        doll[20] = Some(bag_slot_view(4500));
        s.set_inventory_slots(doll);
        s.run("PickupInventoryItem(20)").unwrap();

        assert_eq!(s.eval::<i64>("return PutItemInBackpack()").unwrap(), 1);
        assert!(s.take_container_moves().is_empty());
        assert_eq!(
            s.take_bag_autostores(),
            vec![BagAutoStore {
                src_bag: EQUIPMENT_BAG,
                src_slot: 20,
                dst_bag: 0,
                count: None,
            }]
        );
    }

    #[test]
    fn the_answer_is_the_number_one_not_the_boolean_true() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_bank(Some(Default::default()));
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

        // Below Lua 20 the verb raises before any move (`0x4c8f1f`, `0x848b84`).
        for slot in [16, 0] {
            let e = s
                .run(&format!("PutItemInBag({slot})"))
                .unwrap_err()
                .to_string();
            assert!(e.contains("Invalid bag slot in PutItemInBag"), "{e}");
        }
        assert!(s.cursor_item().is_some(), "untouched");
        assert!(s.take_container_moves().is_empty());
        assert!(s.take_bag_autostores().is_empty());
    }

    #[test]
    fn pickup_bag_from_slot_takes_the_bag_and_returns_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_bank(Some(Default::default()));
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
    fn pickup_bag_from_slot_drops_a_held_item_first() {
        let mut s = UiScript::new().unwrap();
        s.set_bank(Some(Default::default()));
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(Default::default());
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("PickupBagFromSlot(65)").unwrap();
        assert!(s.cursor_item().is_none(), "dropped, and nothing picked up");
        assert!(s.take_container_moves().is_empty(), "no place path exists");
    }

    /// The clear's events (the unlock `0x495455`, then `0x4952cc`) run before the grab reads the
    /// slot (`0x4c7be2`), so its handlers see the bag still in place; the grab's own inner clear
    /// signals once more on the empty cursor (`0x494bb6`); its set's `CURSOR_UPDATE` (`0x494c54`)
    /// sees the bag held and still unlocked, and only `LockItem`'s event (`0x495415`) sees it
    /// locked.
    #[test]
    fn pickup_bag_from_slot_fires_the_clear_before_the_grab() {
        let mut s = UiScript::new().unwrap();
        s.set_bank(Some(Default::default()));
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(one_bank_bag());
        s.run("PickupContainerItem(0, 2)").unwrap();
        heard::listen(
            &s,
            &["CURSOR_UPDATE", "ITEM_LOCK_CHANGED"],
            Some("IsInventoryItemLocked(64)"),
        );
        s.run("PickupBagFromSlot(64)").unwrap();
        assert_eq!(
            heard::take(&s),
            [
                "ITEM_LOCK_CHANGED nil",
                "CURSOR_UPDATE nil",
                "CURSOR_UPDATE nil",
                "CURSOR_UPDATE nil",
                "ITEM_LOCK_CHANGED 1"
            ]
        );
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
            guid: 0x113,
            ..bag_slot_view(4500)
        });
        s.set_bank_bag_slots(bags);
        s.run("PickupBagFromSlot(64)").unwrap();
        assert!(s.cursor_item().is_none(), "locked — refused");
    }

    /// Only a held item is cleared up front (`0x4c7b6a`-`0x4c7b79`): a spell survives an empty
    /// slot, which goes to the doll handler (`0x4c7b9b`), and is dropped by a real pickup's
    /// setter. A bank bag slot needs the bank open (`0x4c7b40`-`0x4c7b55`).
    #[test]
    fn pickup_bag_from_slot_keeps_another_payload_unless_it_picks_up() {
        use crate::script::cursor::{CursorPayload, CursorSpell};
        let mut s = UiScript::new().unwrap();
        s.set_bank_bag_slots(one_bank_bag());
        let spell = CursorPayload::Spell(CursorSpell {
            spell_id: 133,
            texture: None,
            passive: false,
        });
        s.set_cursor_for_test(spell.clone());
        s.run("PickupBagFromSlot(64)").unwrap();
        assert_eq!(
            s.cursor_payload(),
            Some(spell.clone()),
            "the bank is closed"
        );
        s.set_bank(Some(Default::default()));
        s.run("PickupBagFromSlot(65)").unwrap();
        assert_eq!(s.cursor_payload(), Some(spell), "an empty slot keeps it");
        s.run("PickupBagFromSlot(64)").unwrap();
        assert_eq!(s.cursor_item().map(|i| i.slot), Some(64));
    }

    /// A bank bag slot answers nil with the bank closed, in `PutItemInBag` (`0x4c7c40`-`0x4c7c55`)
    /// and in the doll handler it reaches for an empty slot (`0x4c7366`-`0x4c7386`): the held item
    /// stays, nothing is sent.
    #[test]
    fn a_bank_bag_slot_needs_the_bank_open() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(one_bank_bag());
        s.run("PickupContainerItem(0, 1)").unwrap();
        for live in [64, 65] {
            assert!(s
                .eval::<bool>(&format!("return PutItemInBag({live}) == nil"))
                .unwrap());
            s.run(&format!("PickupInventoryItem({live})")).unwrap();
        }
        assert!(s.cursor_item().is_some(), "kept");
        assert!(s.take_container_moves().is_empty());
        assert!(s.take_container_autoequips().is_empty());
    }

    /// Past the empty-slot leg the worker reads the cursor (`0x4c7c65`, `0x4c7c70`): holding no
    /// item and no vendor row it clears with `ClearCursor(1,1)` and answers nil (`0x4c7cdb`-
    /// `0x4c7ce4` → `0x4c7eaf`), so a held spell, coins or bar item is dropped, and the clear's
    /// `CURSOR_UPDATE` fires over an empty cursor too. The backpack is the same worker
    /// (`0x4c7ed0`).
    #[test]
    fn a_payload_that_is_no_item_is_dropped_at_an_occupied_bag() {
        use crate::script::cursor::{CursorAction, CursorMoney, CursorPayload, CursorSpell};
        let payloads = [
            None,
            Some(CursorPayload::Spell(CursorSpell {
                spell_id: 133,
                texture: None,
                passive: false,
            })),
            Some(CursorPayload::Money(CursorMoney { copper: 50 })),
            Some(CursorPayload::Action(CursorAction {
                src_slot: 1,
                kind: Default::default(),
                action: 117,
                texture: None,
                count: 0,
                consumable: false,
                state: None,
            })),
        ];
        for verb in ["PutItemInBag(20)", "PutItemInBackpack()"] {
            for payload in payloads.clone() {
                let mut s = UiScript::new().unwrap();
                s.keep_calls();
                let mut doll: InventorySlots = Default::default();
                doll[20] = Some(bag_slot_view(4500));
                s.set_inventory_slots(doll);
                if let Some(p) = payload.clone() {
                    s.set_cursor_for_test(p);
                }
                heard::listen(&s, &["CURSOR_UPDATE"], None);
                assert!(
                    s.eval::<bool>(&format!("return {verb} == nil")).unwrap(),
                    "{verb} {payload:?}"
                );
                assert!(s.cursor_payload().is_none(), "{verb} {payload:?}: dropped");
                assert_eq!(heard::take(&s), ["CURSOR_UPDATE"], "{verb} {payload:?}");
                assert!(s.take_script_calls().is_empty(), "{verb}: nothing sent");
                assert!(s.take_bag_autostores().is_empty());
            }
        }
    }

    /// A held vendor row on an empty bag slot reaches the doll click (`0x4c7ca7`), which buys
    /// into that slot (`0x4c78fd`): the payload test comes after the empty-slot leg.
    #[test]
    fn a_vendor_row_on_an_empty_bag_slot_buys_into_the_slot() {
        use crate::script::{MerchantItem, MerchantState};
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(MerchantState {
            items: vec![MerchantItem {
                item_id: 159,
                quantity: 1,
                num_available: -1,
                ..Default::default()
            }],
            ..Default::default()
        }));
        s.run("PickupMerchantItem(1)").unwrap();
        assert!(s.eval::<bool>("return PutItemInBag(21) == nil").unwrap());
        assert_eq!(s.take_merchant_slot_buys(), vec![(EQUIPMENT_BAG, 21, 159)]);
        assert!(s.cursor_payload().is_none());
    }

    /// The bag worker's buy reads the held row's item from the list as it stands
    /// (`0x4c7e9a`-`0x4c7eaa`): a row whose item changed since the pickup buys what it now names,
    /// and a row the list no longer holds buys nothing and drops.
    #[test]
    fn a_held_vendor_row_buys_what_the_row_now_names() {
        use crate::script::{MerchantCall, MerchantItem, MerchantState};
        let vendor = |item_id: Option<u32>| MerchantState {
            items: item_id
                .map(|item_id| MerchantItem {
                    item_id,
                    quantity: 1,
                    num_available: -1,
                    ..Default::default()
                })
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        for (now, bought) in [
            (
                Some(2589),
                vec![MerchantCall::BuyIntoBag {
                    bag_guid: None,
                    entry: 2589,
                }],
            ),
            (None, vec![]),
        ] {
            s.set_merchant(Some(vendor(Some(159))));
            s.run("PickupMerchantItem(1)").unwrap();
            s.set_merchant(Some(vendor(now)));
            s.run("PutItemInBackpack()").unwrap();
            assert_eq!(s.take_merchant_calls(), bought, "{now:?}");
            assert!(s.cursor_payload().is_none(), "{now:?}: the row drops");
        }
    }

    /// `PutItemInBackpack` is the worker with the player as the destination (`0x4c7ed0`,
    /// `0x4c7c7d`): its control gate holds (`0x4c7c33`), and an item held from the backpack itself
    /// is a cancel, its container being the player and its slot one of 23-38 (`0x4c7cf8`-
    /// `0x4c7d11`): `ClearCursor(1,1)`, answer 1, nothing stored.
    #[test]
    fn put_item_in_backpack_cancels_a_backpack_item_and_needs_control() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 2)").unwrap();
        assert_eq!(s.eval::<i64>("return PutItemInBackpack()").unwrap(), 1);
        assert!(s.cursor_item().is_none());
        assert!(s.take_bag_autostores().is_empty(), "a cancel, not a store");
        assert!(
            !s.eval::<bool>("local _, _, l = GetContainerItemInfo(0, 2) return l == 1")
                .unwrap(),
            "the cancel unlocks"
        );

        let mut doll: InventorySlots = Default::default();
        doll[1] = Some(InvSlotView {
            item_id: 1234,
            count: 1,
            equip_slots: vec![1],
            guid: 0x301,
            ..Default::default()
        });
        s.set_inventory_slots(doll);
        s.run("PickupInventoryItem(1)").unwrap();
        s.set_player_control(false);
        assert!(s.eval::<bool>("return PutItemInBackpack() == nil").unwrap());
        assert!(s.cursor_item().is_some(), "no control: kept");
        assert!(s.take_bag_autostores().is_empty());
        s.set_player_control(true);
        assert_eq!(s.eval::<i64>("return PutItemInBackpack()").unwrap(), 1);
        assert_eq!(
            s.take_bag_autostores(),
            vec![BagAutoStore {
                src_bag: EQUIPMENT_BAG,
                src_slot: 1,
                dst_bag: 0,
                count: None,
            }],
            "a worn item is stored"
        );
    }
}
