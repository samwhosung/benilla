//! `SwapItem` (`0x5e0c40`), the whole-stack swap every placing verb sends through: the doll click,
//! `PickupContainerItem`, `EquipCursorItem`, the bag worker and the bind-confirm accept. It asks
//! the bind-on-equip question, or parks the move on an uncached record, before it sends, and only
//! the send clears the cursor (`0x5e0fe7`): a question or a wait leaves the item held.

use mlua::Lua;

use crate::script::container::ContainerMove;
use crate::script::tick::verb;
use crate::script::{ItemCall, Model};

use super::equip::{equip_question, EquipFacts, Question};
use super::{clear, Clear, EQUIPMENT_BAG};

/// What `SwapItem` did with a whole-stack move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Swap {
    /// No question: the move goes out and the cursor clears with `ClearCursor(0,1)` (`0x5e0fe7`),
    /// the moved items keeping their locks for the server's answer.
    Sent(ContainerMove),
    /// The item the move would equip binds on equip: `EQUIP_BIND_CONFIRM` (`0x5e0ee4`), then the
    /// exit past the clear (`0x5e0eec` → `0x5e0f63`), the item still held.
    Asked(ContainerMove),
    /// The record of the item the move would equip, `entry`, is not cached: the move is parked
    /// and the record asked with the callback `0x5e1020` (`0x5e0d9c`-`0x5e0e2e`), nothing sent
    /// and the item still held.
    Waits { mv: ContainerMove, entry: u32 },
    /// The bag at the move's other end has no object: the call exits past the send and its
    /// clear (`0x5e0d23`, `0x5e0d32` → `0x5e0fec`), nothing sent and the item still held.
    NoBag(ContainerMove),
}

/// An equipment position of the player's own list, the side a swap equips into (`0x5e0c9a`-
/// `0x5e0cd5`): 0-based slots 0 to 22 (the doll and the equipped bags) and 63 to 68 (the bank bag
/// slots).
fn equip_position(bag: i64, slot: u32) -> bool {
    bag == EQUIPMENT_BAG
        && ((1..=23).contains(&slot) || super::bag_verbs::BANK_BAG_INV_SLOTS.contains(&slot))
}

/// What `SwapItem` finds at the move's other end when exactly one end is an equipment position.
enum OtherEnd {
    /// Both or neither end is one, or the other end's slot holds no item object: the move goes
    /// out unasked (`0x5e0ce7`, `0x5e0cf6`, `0x5e0d6d` → `0x5e0eee`).
    Unasked,
    /// The other end's bag has no object (`0x5e0d23`) or no slot list (`0x5e0d32`): the call
    /// exits past the send and its clear (`0x5e0fec`).
    NoBag,
    /// The item the swap would equip (`0x5e0d64`).
    Item(EquipFacts),
}

/// Whether container `bag` has an object for `SwapItem`'s lookup by its guid (`0x5e0d1a`): the
/// active player's own list for the player's slots, the backpack, the vault and the keyring, the
/// bag item in the container's inventory slot (`ContainerIDToInventoryID`'s two bands) for bags
/// 1-10.
fn bag_object(lua: &Lua, model: &Model, bag: i64) -> bool {
    match bag {
        EQUIPMENT_BAG | -2..=0 => true,
        1..=10 => {
            let live = if bag <= 4 { bag + 19 } else { bag + 59 };
            crate::script::char_stats::player_slot_object(lua, model, live as usize)
                .is_some_and(|v| v.guid != 0)
        }
        _ => false,
    }
}

/// The move's other end when exactly one end is an equipment position (`0x5e0ce1`-`0x5e0d69`).
fn other_end(lua: &Lua, model: &Model, mv: &ContainerMove) -> OtherEnd {
    let (bag, slot) = match (
        equip_position(mv.src_bag, mv.src_slot),
        equip_position(mv.dst_bag, mv.dst_slot),
    ) {
        (true, false) => (mv.dst_bag, mv.dst_slot),
        (false, true) => (mv.src_bag, mv.src_slot),
        _ => return OtherEnd::Unasked,
    };
    if !bag_object(lua, model, bag) {
        return OtherEnd::NoBag;
    }
    crate::script::held::held_slot(lua, model, bag, slot)
        .filter(|s| s.item_id != 0 && s.guid != 0)
        .map_or(OtherEnd::Unasked, |s| {
            OtherEnd::Item(EquipFacts::of_slot(&s))
        })
}

/// `SwapItem(mv, confirm)` (`0x5e0c40`): the question on the item the move would equip, unless
/// `confirm` (the accept's re-issue, `0x5e1c46`) skips it; else the send (`0x5e0fdb`) and its
/// clear (`0x5e0fe7`). The send, the question or the wait goes to the host in its place
/// ([`Model::item_call`]), ahead of the clear's signals.
pub(crate) fn swap_items(lua: &Lua, model: &mut Model, mv: ContainerMove, confirm: bool) -> Swap {
    let question = match other_end(lua, model, &mv) {
        OtherEnd::NoBag => return Swap::NoBag(mv),
        OtherEnd::Unasked => Question::Send,
        OtherEnd::Item(item) => equip_question(lua, model, item, confirm),
    };
    let (swap, call) = match question {
        Question::Ask => (Swap::Asked(mv.clone()), ItemCall::SwapAsk(mv)),
        Question::Wait(entry) => (
            Swap::Waits {
                mv: mv.clone(),
                entry,
            },
            ItemCall::SwapWait { mv, entry },
        ),
        Question::Send => (Swap::Sent(mv.clone()), ItemCall::Move(mv)),
    };
    model.item_call(call);
    if let Swap::Sent(_) = swap {
        clear(model, Clear::KEEP_LOCK);
    }
    swap
}

/// [`swap_items`] with the question asked, from a verb's segment: what it did goes to the host in
/// its place, the send ([`ItemCall::Move`]) ahead of its clear's signals, or the question
/// ([`ItemCall::SwapAsk`]) or the wait ([`ItemCall::SwapWait`]).
pub(crate) fn swap_and_file(lua: &Lua, model: &mut Model, mv: ContainerMove) {
    swap_items(lua, model, mv, false);
}

impl crate::script::UiScript {
    /// `SwapItem` from the host: the bind-confirm accept re-issues the parked move with `confirm`
    /// set (`0x5e1c46`), so it sends and clears whatever the cursor holds then, the send handed
    /// over in place.
    pub fn swap_items(&mut self, mv: ContainerMove, confirm: bool) -> Swap {
        verb(&self.lua, |model| swap_items(&self.lua, model, mv, confirm))
    }

    /// The callback `0x5e1020` for a move parked on its record: with the record found
    /// (`0x5e1099`-`0x5e10a0`) `SwapItem` runs again unconfirmed (`0x5e10c2`), its send, question
    /// or wait applied through the host. The host frees the parked entry after.
    pub fn replay_swap(&mut self, mv: ContainerMove) -> mlua::Result<()> {
        verb(&self.lua, |model| swap_and_file(&self.lua, model, mv));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::script::{ContainerSlot, ContainerState, InvSlotView, InventorySlots, UiScript};

    const HELM: u64 = 0x11;
    const WORN: u64 = 0x31;
    const BAG: u64 = 0x21;
    const WORN_BAG: u64 = 0x41;

    /// Backpack slot 1 holding an unbound helm of `bonding`, its record cached or not, and slot 2
    /// a bag of `bonding`.
    fn backpack(bonding: u32, cached: bool) -> ContainerState {
        let slot = |guid, item_id, equip_slots: Vec<u8>| ContainerSlot {
            guid,
            item_id,
            count: 1,
            bonding,
            quality: cached.then_some(2),
            equip_slots,
            ..Default::default()
        };
        ContainerState {
            name: None,
            num_slots: 16,
            slots: [
                (1, slot(HELM, 871, vec![1])),
                (2, slot(BAG, 4500, vec![20, 21, 22, 23])),
            ]
            .into(),
        }
    }

    /// A worn helm in doll slot 1 and a bag in bag slot 20.
    fn doll() -> InventorySlots {
        let mut slots: InventorySlots = Default::default();
        slots[1] = Some(InvSlotView {
            item_id: 1234,
            guid: WORN,
            inventory_type: Some(1),
            equip_slots: vec![1],
            ..Default::default()
        });
        slots[20] = Some(InvSlotView {
            item_id: 4501,
            guid: WORN_BAG,
            inventory_type: Some(18),
            equip_slots: vec![20, 21, 22, 23],
            ..Default::default()
        });
        slots
    }

    fn vm(bonding: u32, cached: bool) -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_container(0, Some(backpack(bonding, cached)));
        s.set_inventory_slots(doll());
        s.seat_item_records();
        s
    }

    /// Every swap path that equips an unbound bind-on-equip item asks inside `SwapItem` and
    /// leaves by its exit (`0x5e0eec` → `0x5e0f63`), past the send and its clear (`0x5e0fe7`): the
    /// doll click (`0x4c78be`), `EquipCursorItem` (`0x489757`) and the bag worker (`0x4c7df5`)
    /// keep the item held, so `CursorHasItem()` answers 1 under the question, and nothing is sent.
    #[test]
    fn a_bind_on_equip_swap_keeps_the_item_held_under_the_question() {
        for (pick, place, guid) in [
            ("PickupContainerItem(0, 1)", "PickupInventoryItem(1)", HELM),
            ("PickupContainerItem(0, 1)", "EquipCursorItem(1)", HELM),
            ("PickupContainerItem(0, 2)", "PutItemInBag(20)", BAG),
        ] {
            let mut s = vm(2, true);
            s.run(pick).unwrap();
            s.run(place).unwrap();
            assert_eq!(s.cursor_item().map(|i| i.guid), Some(guid), "{place}: held");
            assert!(
                s.eval::<bool>("return CursorHasItem() == 1").unwrap(),
                "{place}: the cursor answers held"
            );
            assert!(s.take_container_moves().is_empty(), "{place}: nothing sent");
        }
    }

    /// `PickupContainerItem` asks inside `SwapItem` too, but its own `ClearCursor(0,1)` follows
    /// the call (`0x4f9eb8`): a worn helm dropped on a bind-on-equip helm in a bag files the
    /// question, sends nothing, and the cursor ends empty. The end state past the dialog, its
    /// cancel unlocking both ends, runs through the stock FrameXML in the app's
    /// `the_bag_clicks_clear_cancels_its_bind_question_and_unlocks_both_ends`.
    #[test]
    fn pickup_container_item_asks_then_clears_past_the_swap() {
        let mut s = vm(2, true);
        s.run("PickupInventoryItem(1) PickupContainerItem(0, 1)")
            .unwrap();
        let calls = s.take_item_calls();
        assert!(
            matches!(calls.as_slice(), [crate::script::ItemCall::SwapAsk(_)]),
            "the question alone, nothing sent: {calls:?}"
        );
        assert!(s.cursor_item().is_none(), "the binding's clear");
    }

    /// The doll click (`0x4c7300`) and the bag worker's swap leg (`0x4c7d8d`, `0x4c7df5`) never
    /// read the split count `0xb4b40c` (its only readers are `0x48988f`, `0x4c7e0c`, `0x4f9e1c`
    /// and `0x4fb792`; `SetCursorItem` writes it at `0x494bf8`): a held split goes through
    /// `SwapItem` from its source slot, the whole stack moving.
    #[test]
    fn a_held_split_swaps_its_whole_stack() {
        for (slot, place, dst_slot) in [
            (1, "PickupInventoryItem(1)", 1),
            (2, "PutItemInBag(20)", 20),
        ] {
            let mut s = vm(0, true);
            let mut pack = backpack(0, true);
            pack.slots.get_mut(&slot).unwrap().count = 5;
            s.set_container(0, Some(pack));
            s.seat_item_records();
            s.run(&format!("SplitContainerItem(0, {slot}, 2)")).unwrap();
            assert!(s.cursor_item().is_some_and(|i| i.count == Some(2)));
            s.run(place).unwrap();
            assert!(s.cursor_item().is_none(), "{place}: sent and cleared");
            assert_eq!(
                s.take_container_moves(),
                vec![crate::script::ContainerMove {
                    src_bag: 0,
                    src_slot: slot,
                    dst_bag: super::EQUIPMENT_BAG,
                    dst_slot,
                    count: None,
                }],
                "{place}: the whole stack"
            );
        }
    }

    /// The question reads the item cache, not the bag view (`0x5e0db1`, then `[rec+0x194]` at
    /// `0x5e0e54`): a record the cache does not hold waits however the slot reads, and a held
    /// record's bonding asks whatever the slot carries.
    #[test]
    fn the_swap_question_reads_the_record_cache() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_container(0, Some(backpack(0, true)));
        s.set_inventory_slots(doll());
        s.run("PickupContainerItem(0, 1) PickupInventoryItem(1)")
            .unwrap();
        assert!(s.cursor_item().is_some(), "no record cached: it waits");
        assert!(s.take_container_moves().is_empty());

        s.set_item_template(
            871,
            crate::script::ItemTemplateView {
                bonding: 2,
                ..Default::default()
            },
        );
        s.run("PickupInventoryItem(1)").unwrap();
        assert!(
            s.cursor_item().is_some(),
            "the record binds on equip: it asks"
        );
        assert!(s.take_container_moves().is_empty());
    }

    /// The other end's bag is looked up by its guid first (`0x5e0d1a`): with no bag object, or
    /// none holding a slot list, `SwapItem` exits past both the send and its clear (`0x5e0d23`,
    /// `0x5e0d32` → `0x5e0fec`), the accept's re-issue included. The held item stays held.
    #[test]
    fn a_swap_from_a_missing_bag_sends_nothing_and_keeps_the_cursor() {
        let mut s = vm(0, true);
        s.run("PickupContainerItem(0, 1)").unwrap();
        let from_no_bag = crate::script::ContainerMove {
            src_bag: 3,
            src_slot: 1,
            dst_bag: super::EQUIPMENT_BAG,
            dst_slot: 1,
            count: None,
        };
        for confirm in [false, true] {
            assert!(
                !matches!(
                    s.swap_items(from_no_bag.clone(), confirm),
                    super::Swap::Sent(_)
                ),
                "confirm {confirm}: not sent"
            );
            assert_eq!(
                s.cursor_item().map(|i| i.guid),
                Some(HELM),
                "confirm {confirm}: nothing cleared"
            );
        }
    }

    /// With nothing to bind the same swaps send and clear (`0x5e0fe7`).
    #[test]
    fn an_unbound_swap_sends_and_clears() {
        let mut s = vm(0, true);
        s.run("PickupContainerItem(0, 1) PickupInventoryItem(1)")
            .unwrap();
        assert!(s.cursor_item().is_none());
        assert_eq!(s.take_container_moves().len(), 1);
    }

    /// The item the swap would equip has no cached record: `SwapItem` parks the move on it
    /// (`0x5e0dbe`) and returns, nothing sent and nothing cleared, the item still held.
    #[test]
    fn a_swap_onto_an_uncached_record_waits_with_the_item_held() {
        for place in ["PickupInventoryItem(1)", "EquipCursorItem(1)"] {
            let mut s = vm(0, false);
            s.run("PickupContainerItem(0, 1)").unwrap();
            s.run(place).unwrap();
            assert_eq!(s.cursor_item().map(|i| i.guid), Some(HELM), "{place}");
            assert!(s.take_container_moves().is_empty(), "{place}");
        }
    }

    /// The record's callback (`0x5e1020`) replays the parked swap unconfirmed (`0x5e10c2`) once
    /// the view holds the record: an unbound helm now sends and clears; the accept's re-issue
    /// (`0x5e1c46`) sends a bind-on-equip one without asking.
    #[test]
    fn the_replay_and_the_accept_run_swap_item_again() {
        let mv = crate::script::ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: super::EQUIPMENT_BAG,
            dst_slot: 1,
            count: None,
        };
        let mut s = vm(0, false);
        s.run("PickupContainerItem(0, 1) PickupInventoryItem(1)")
            .unwrap();
        assert!(s.take_container_moves().is_empty(), "parked");
        s.set_container(0, Some(backpack(0, true)));
        s.seat_item_records();
        s.replay_swap(mv.clone()).unwrap();
        assert_eq!(s.take_container_moves(), vec![mv.clone()], "sent");
        assert!(s.cursor_item().is_none(), "and cleared");

        let mut s = vm(2, true);
        s.run("PickupContainerItem(0, 1) PickupInventoryItem(1)")
            .unwrap();
        assert!(s.cursor_item().is_some(), "asked");
        assert_eq!(s.swap_items(mv.clone(), true), super::Swap::Sent(mv));
        assert!(s.cursor_item().is_none(), "the accept's send clears");
    }
}
