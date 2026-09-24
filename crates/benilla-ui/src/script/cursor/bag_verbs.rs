//! The three BAG-SLOT verbs — `PutItemInBag`, `PutItemInBackpack`, `PickupBagFromSlot`.
//!
//! CARVED, not inferred. The three findings that shape everything below:
//!
//! 1 · **`PutItemInBackpack()` IS `PutItemInBag(0xFF)`.** `0x4c7ed0` is a ten-byte thunk —
//!   `mov ecx,0xff` / `jmp 0x4c7c00` — so there is one worker here too, and only the destination
//!   differs. The `0xFF` path is *not* "bag 0": the destination guid is the player's own, which
//!   makes the empty-slot leg unreachable and `0x4c7d3e je 0x4c7e0c` skip the swap fork outright,
//!   so a bag dropped on the backpack button is auto-stored INTO it, never swapped with it.
//!
//! 2 · **`PutItemInBag` forks three ways**, on the state of the target slot and the held object:
//!   empty slot → the paper-doll handler equips the held bag ([`super::doll::pickup_inventory_item`],
//!   which is `0x4c7300` — the same function the byte path calls); occupied + the held object is a
//!   CONTAINER → a bag↔bag swap; occupied + an ordinary item → auto-store into that container's
//!   contents, with the client picking no destination slot at all.
//!
//! 3 · **The return value answers "was this click consumed as a placement against an existing
//!   container?", not "did something get placed".** It is the Lua **number `1`**, never the boolean
//!   `true` — and it is `nil` on the empty-slot leg that really does equip the bag, and whenever
//!   the cursor is empty. That last one is the whole reason a plain left-click opens a bag:
//!   `BagSlotButton_OnClick`'s `if ( not hadItem ) then ToggleBag(...)`.
//!
//! `PickupBagFromSlot` has its own, much shorter worker (`0x4c7b00`) and is a *destructive*
//! pickup: it drops whatever the cursor already held before it looks at the slot.

use mlua::{Lua, MultiValue, Value};

use crate::script::container::ContainerMove;
use crate::script::Model;

use super::{clear_cursor, queue_cursor_update, queue_lock_changed, CursorItem, CursorPayload};
use super::{doll::pickup_inventory_item, EQUIPMENT_BAG};

/// The live-API inventory ids the two verbs' shared argument reader (`0x4c8520`) accepts, minus
/// what each binding's own floor gate (`cmp ecx,0x13; jl` on the 0-based value — `0x4c8f1f`,
/// `0x4c8fbf`) then cuts off. In the space a Lua caller writes:
///
/// | band | what it is |
/// |---|---|
/// | 20..=23 | the four EQUIPPED bag slots (`Bag0Slot`..`Bag3Slot`) |
/// | 40..=63 | the bank's 24 generic slots |
/// | 64..=69 | the six BANK BAG slots |
/// | 82..=113 | the keyring |
///
/// The reader rejects the backpack's own item slots (24..=39) and buyback (70..=81); the floor
/// cuts equipment (1..=19) and ammo. **So a macro may legally hand either verb a bank item slot
/// or a keyring slot, and nothing downstream catches it** — the reference sends a packet the
/// server refuses. See [`autostore_container`] for the one place benilla is tighter, and why.
fn accepts(live: u32) -> bool {
    (20..=23).contains(&live)
        || (40..=63).contains(&live)
        || (64..=69).contains(&live)
        || (82..=113).contains(&live)
}

/// The live-API **container** id a bag slot's own inventory id names — the inverse of
/// `ContainerIDToInventoryID` (`crate::script::container`'s own table): 20..=23 → 1..=4, 64..=69 →
/// 5..=10. `None` for an inventory id that is not a bag slot.
///
/// **This is the one place benilla's gate is tighter than the engine's**, and it is deliberate.
/// [`accepts`] admits the bank's generic slots and the keyring because the reference's argument
/// reader does; the reference then auto-stores *into* them, sending
/// `CMSG_AUTOSTORE_BAG_ITEM(…, dstbag = live − 1)` for a `dstbag` that names no container, which
/// the server refuses (`EQUIP_ERR_ITEM_DOESNT_GO_TO_SLOT`). benilla has no container id to name
/// there, so [`put_item_in_bag`]'s auto-store leg answers `nil` and sends nothing. No shipped
/// window produces the call — the reference's own two callers are `BagSlotButton_OnClick` and
/// `BankFrameItemButtonBag_OnClick`, both of which pass a bag slot — and a refusal the player
/// never sees is not worth a packet.
fn autostore_container(live: u32) -> Option<i64> {
    match live {
        20..=23 => Some(i64::from(live) - 19),
        64..=69 => Some(i64::from(live) - 59),
        _ => None,
    }
}

/// `Bag0Slot` — the first of the four EQUIPPED bag slots, and the tell that an item is a
/// container. See [`is_container`].
const BAG0_SLOT: u8 = 20;

/// The six BANK BAG slots' live-API ids (`BankButtonIDToInvSlotID(1..6, isBag)`).
pub(super) const BANK_BAG_INV_SLOTS: std::ops::RangeInclusive<u32> = 64..=69;

/// Whether the held item is a CONTAINER — the fork the reference reads straight off the object
/// (`0x4c7dc4 shr eax,2; test al,1`, `OBJECT_FIELD_TYPE`'s `TYPEMASK_CONTAINER` bit).
///
/// benilla has no object-type field on a cursor payload, and asks the equivalent question of the
/// data it does carry: `equip_slots` is the server's own `Player::FindEquipSlot`, and
/// `INVTYPE_BAG` is the ONLY inventory type in that table that yields the bag slots
/// (`ui_items::find_equip_slot`, arm 18). So "this item names Bag0Slot as a place it could be
/// worn" and "this item is a container" are the same set.
fn is_container(item: &CursorItem) -> bool {
    item.equip_slots.contains(&BAG0_SLOT)
}

/// Whether the held item may be dropped on doll slot `id` — the fit rule, one band wider than
/// `equip_slots` alone.
///
/// The four EQUIPPED bag slots are in `equip_slots` because the server's `FindEquipSlot` names
/// them; the six BANK BAG slots never are, because `FindEquipSlot` does not know about the bank
/// (it is `CanBankItem`'s business there). The client's own predicate `CursorCanGoInSlot`
/// (`0x5ea720`) is 0216 §5's unresolved residual, so the rule here is stated from the observable:
/// a bank bag slot takes a container and nothing else, and the server's `CanBankItem` referees
/// either way through `SMSG_INVENTORY_CHANGE_FAILURE`.
pub(super) fn fits_slot(item: &CursorItem, id: u32) -> bool {
    if BANK_BAG_INV_SLOTS.contains(&id) {
        return is_container(item);
    }
    u8::try_from(id).is_ok_and(|id| item.equip_slots.contains(&id))
}

/// `PutItemInBag(inventorySlot)` — module doc, finding 2. Returns the reference's own `1`/`nil`
/// as a bool the binding maps ("was this click consumed as a placement against an existing
/// container?", NOT "did something get placed").
pub(super) fn put_item_in_bag(model: &mut Model, live: u32) -> bool {
    if !accepts(live) {
        return false;
    }
    // `0x4c7ce4 je 0x4c7eaf` — an empty cursor is the FIRST thing tested, and it answers `nil`
    // with nothing sent. A Spell/Action/Macro/PetAction/StablePet payload is not the client's
    // item cursor either, and takes the same exit: this is what leaves a plain click free to open
    // the bag.
    let Some(CursorPayload::Item(held)) = model.cursor.clone() else {
        return false;
    };
    let occupied = usize::try_from(live)
        .ok()
        .and_then(|slot| model.inv_slot("player", slot))
        .is_some();

    if !occupied {
        // Leg 1 — the EMPTY slot. The byte path calls `0x4c7300`, the paper-doll handler, which
        // is `pickup_inventory_item`'s place arm here: the fit predicate, the queued move, the
        // clear and both events, all of it already the doll's. `0x4c7cae xor eax,eax` is
        // unconditional, so this answers `nil` whether or not the bag actually went in.
        pickup_inventory_item(model, live);
        return false;
    }

    // A pure cancel: the held item IS this bag (put back where it came from). Sends nothing, and
    // answers `1` — the click WAS consumed.
    if held.bag == EQUIPMENT_BAG && held.slot == live {
        model.cursor = None;
        queue_cursor_update(model);
        queue_lock_changed(model, held.bag, held.slot);
        return true;
    }

    if is_container(&held) || held.bag == EQUIPMENT_BAG {
        // Leg 2 — bag↔bag SWAP (`0x5e0c40`, `CMSG_SWAP_ITEM`/`CMSG_SWAP_INV_ITEM`). "Came from a
        // bag equipment slot" is the reference's second entry to this fork, hence the `||`. The
        // existing move drain already picks the opcode off the two endpoints.
        //
        // A split carry cannot swap: half a stack is not a bag. The reference reaches its SPLIT
        // sender only from the auto-store leg, so this falls through to a refusal, kept.
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

    // Leg 3 — AUTO-STORE into the container's contents (`CMSG_AUTOSTORE_BAG_ITEM 0x10B`, or
    // `CMSG_SPLIT_ITEM 0x10E` for a split carry). **The client never picks a destination slot**:
    // AUTOSTORE carries none and SPLIT carries the literal `0xFF`.
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

/// `PutItemInBackpack()` — module doc, finding 1: the same worker with the destination fixed to
/// the player's own guid, which makes the empty-slot and swap legs unreachable. Everything that
/// lands here auto-stores into the backpack.
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

/// `PickupBagFromSlot(inventorySlot)` — a **destructive** pickup, not a restricted
/// `PickupInventoryItem` (`0x4c7b00`). Returns nothing.
///
/// Three things it does that the doll pickup does not:
///
/// · **`ClearCursor(1, 1)` unconditionally, before it looks at anything** (`0x4c7b79`) — whatever
///   was held is dropped and unlocked, even if the slot then turns out to be empty.
/// · Resolves the occupant with **`TYPEMASK_CONTAINER`** (`0x4c7bb3 mov ecx,4`, where
///   `PickupInventoryItem` uses `ecx = 2`). This is what makes it silently no-op at the bank item
///   slots and the keyring that its floor-only gate still admits: those hold ordinary items.
/// · Has **no place/swap path at all**, and none of `0x4c7300`'s targeting reroute, repair-cursor
///   leg or slot-`−1` ammo leg.
///
/// There is **no client-side "a bag must be empty to be moved" rule** — both workers were read end
/// to end and neither inspects container occupancy. That refusal is the server's
/// `Player::CanUnequipItem` (`EQUIP_ERR_CAN_ONLY_DO_WITH_EMPTY_BAGS`), arriving as
/// `SMSG_INVENTORY_CHANGE_FAILURE`.
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
        // TYPEMASK_CONTAINER, applied after the view is built so the predicate reads off the same
        // `equip_slots` every other bag decision here does ([`is_container`]).
        .filter(is_container);
    if let Some(item) = picked {
        model.cursor = Some(CursorPayload::Item(item));
        queue_cursor_update(model);
        queue_lock_changed(model, EQUIPMENT_BAG, live);
    }
}

/// Register the three verbs — top-level globals, like every other cursor binding.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // The number `1` or nil — `0x6f3810` writes Lua tag 3 (number) with the IEEE-754 double
    // `0x3FF0000000000000`; tag 1 (boolean) appears in neither body. FrameXML only ever tests it
    // for truthiness, but an addon that compares against `true` must see what the reference gives
    // it.
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

    /// An item that IS a bag — `equip_slots` is `find_equip_slot(INVTYPE_BAG)`, the only inventory
    /// type in that table that names Bag0Slot, which is how [`super::is_container`] reads it.
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

    /// Finding 3, the one every caller depends on: an empty cursor answers **nil** and sends
    /// nothing, which is what leaves `BagSlotButton_OnClick`'s `if ( not hadItem ) then
    /// ToggleBag(...)` free to open the bag.
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

    /// Leg 1 — the EMPTY slot equips the held bag, and answers **nil** while doing it
    /// (`0x4c7cae xor eax,eax` is unconditional). That pairing is the reference's, not a slip:
    /// the click both puts the bag in and opens the window it just created.
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

    /// Leg 1's fit rule: a bank bag slot takes a CONTAINER and nothing else. An ordinary item
    /// dropped on an empty bank bag slot is refused and kept — the server's `CanBankItem` would
    /// refuse it anyway, and there is nothing to auto-store into.
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

    /// Leg 2 — an OCCUPIED slot plus a held container is a bag↔bag swap, and answers `1`.
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

    /// Leg 3 — an OCCUPIED slot plus an ORDINARY item auto-stores INTO that bag. The intent
    /// carries no destination slot, because the wire has none.
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
                // Live container id 5 — bank bag slot 1 (`ContainerIDToInventoryID(5) == 64`).
                dst_bag: 5,
                count: None,
            }]
        );
    }

    /// Finding 1 — `PutItemInBackpack()` skips both the empty-slot and the swap legs outright: a
    /// held BAG dropped on the backpack button is auto-stored, never swapped with it.
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

    /// Finding 3's literal shape: the reference pushes the Lua **number** `1` (`0x6f3810` writes
    /// tag 3 with the IEEE-754 double `0x3FF0…`), never the boolean `true`. FrameXML only tests
    /// truthiness; an addon comparing against `true` must see what the reference gives it.
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

    /// The floor gate (`cmp ecx,0x13; jl`): equipment slots and ammo are below it, so the verb
    /// never reaches the paper-doll handler through them.
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

    /// `PickupBagFromSlot` picks a bank bag up, returns NOTHING, and locks the slot.
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

    /// **Destructive, and that is the finding**: `ClearCursor(1,1)` runs unconditionally before
    /// the slot is even looked at (`0x4c7b79`), so a held payload is dropped even when the slot
    /// turns out to hold nothing to pick up.
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

    /// The `TYPEMASK_CONTAINER` resolve (`mov ecx,4`, where `PickupInventoryItem` uses 2): the
    /// bank's own item slots are inside the gate's range and hold ordinary items, so the verb
    /// silently no-ops there. A locked bag is refused too.
    #[test]
    fn pickup_bag_from_slot_only_takes_containers_and_never_a_locked_one() {
        let mut s = UiScript::new().unwrap();
        // Bank vault slot 3 → live id 42, an ordinary item: inside the gate, not a container.
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
