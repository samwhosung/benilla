//! `SwapItem` (`0x5e0c40`), the whole-stack swap every placing verb sends through.

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
    /// the call (`0x4f9eb8`): a worn helm dropped on a bind-on-equip helm in a bag sends nothing,
    /// and the cursor ends empty with the worn helm still locked.
    #[test]
    fn pickup_container_item_asks_then_clears_past_the_swap() {
        let mut s = vm(2, true);
        s.run("PickupInventoryItem(1) PickupContainerItem(0, 1)")
            .unwrap();
        assert!(s.take_container_moves().is_empty(), "nothing sent");
        assert!(s.cursor_item().is_none(), "the binding's clear");
        assert!(
            s.item_locked(WORN) && s.item_locked(HELM),
            "both ends locked"
        );
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
}
