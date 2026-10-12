//! `AutoEquipCursorItem` (`0x5e1480`) and the verb that puts an item on the cursor before calling it
//! (`0x5e1840`): the bag's right-click, the action bar's equip and the bind-confirm accept.

use mlua::Lua;

use crate::script::tick::verb;
use crate::script::Model;

use super::{
    clear, clear_cursor, set_cursor, Clear, CursorItem, CursorPayload, EQUIPMENT_BAG,
    ITEM_CLASS_PROJECTILE,
};

/// What `AutoEquipCursorItem` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoEquip {
    /// Nothing: no item held, a split carry (which stays held), or no item to put on the cursor.
    None,
    /// An item from an equipment or equipped-bag slot, put back (`0x5e155a`).
    Back,
    /// The equip is sent, the item from this Lua `(bag, slot)` cleared off the cursor. A
    /// projectile loads by entry with `CMSG_SET_AMMO`, the app's to tell.
    Sent { bag: i64, slot: u32 },
}

/// `AutoEquipCursorItem` on the held item: an item from an equipment or equipped-bag slot, the
/// 0-based slot below 0x17, goes back with `ClearCursor(1,1)` (`0x5e155a`, `0x5e1567`); any other
/// whole stack, from a bag, the backpack, the bank, a bank bag slot or the keyring, is sent
/// (`0x5e1575`), the server picking the slot and swapping the displaced piece back
/// (`ItemHandler.cpp:138-228`); a split stays held. The send tail clears with `ClearCursor(ebx,1)`
/// (`0x5e1761`): a projectile loads by entry with `CMSG_SET_AMMO` and stays in its bag, so its arm
/// sets `ebx = 1` (`0x5e1704`, `0x5e1711`) and the item unlocks; every other arm keeps the lock
/// for the server's answer (`0x5e1577`). Neither locks anything new.
pub(crate) fn auto_equip_held(model: &mut Model) -> AutoEquip {
    match &model.cursor {
        Some(CursorPayload::Item(item)) if item.bag == EQUIPMENT_BAG && item.slot <= 23 => {
            clear_cursor(model);
            AutoEquip::Back
        }
        Some(CursorPayload::Item(item)) if item.count.is_none() => {
            let (bag, slot) = (item.bag, item.slot);
            let how = if item.item_class == Some(ITEM_CLASS_PROJECTILE) {
                Clear::BOTH
            } else {
                Clear::KEEP_LOCK
            };
            clear(model, how);
            AutoEquip::Sent { bag, slot }
        }
        _ => AutoEquip::None,
    }
}

/// `AutoEquipCursorItem()` (`0x48a040` into `0x5e1480`), a click or drop on the doll's model
/// (`PaperDollFrame.lua:31-35`), queueing the send for the app. Returns whether to repaint.
pub(crate) fn auto_equip_cursor_item(model: &mut Model) -> bool {
    match auto_equip_held(model) {
        AutoEquip::Sent { bag, slot } => {
            model.container_autoequips.push((bag, slot));
            true
        }
        AutoEquip::Back => true,
        AutoEquip::None => false,
    }
}

/// `0x5e1840`: `SetCursorItem(1, …)` puts the item on the cursor, its inner clear dropping and
/// unlocking whatever is held (`0x494bb6`), then `AutoEquipCursorItem`. The item is not locked.
pub(crate) fn auto_equip_from(lua: &Lua, item: CursorItem) -> AutoEquip {
    set_cursor(lua, true, |_| Some(CursorPayload::Item(item)));
    verb(lua, auto_equip_held)
}

impl crate::script::UiScript {
    /// `0x5e1840` on the item at Lua `(bag, slot)`: the bind-confirm accept re-issues it. Nothing
    /// happens for an empty or unresolved slot (`0x5e18da`).
    pub fn auto_equip_slot(&mut self, bag: i64, slot: u32) -> AutoEquip {
        let item = {
            let model = self.model_ref();
            model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .filter(|s| s.guid != 0)
                .map(|s| s.cursor_item(bag, slot))
        };
        match item {
            Some(item) => auto_equip_from(&self.lua, item),
            None => AutoEquip::None,
        }
    }

    /// `0x5e1840` on the bag item `guid`, the action bar's equip: `UseAction` resolves an entry to
    /// the copy's container and slot (`0x6222b0`, `0x4e602a`).
    pub fn auto_equip_guid(&mut self, guid: u64) -> AutoEquip {
        let at = {
            let model = self.model_ref();
            model.containers.iter().find_map(|(&bag, c)| {
                c.slots
                    .iter()
                    .find(|(_, s)| s.guid == guid)
                    .map(|(&slot, _)| (bag, slot))
            })
        };
        match at {
            Some((bag, slot)) => self.auto_equip_slot(bag, slot),
            None => AutoEquip::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AutoEquip;
    use crate::script::cursor::EQUIPMENT_BAG;
    use crate::script::{ContainerSlot, ContainerState, UiScript};

    fn bag(slots: &[(u32, u64, u32)]) -> ContainerState {
        ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: slots
                .iter()
                .map(|&(slot, guid, class)| {
                    (
                        slot,
                        ContainerSlot {
                            guid,
                            item_id: 100 + slot,
                            count: 1,
                            item_class: Some(class),
                            equip_slots: vec![1],
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        }
    }

    /// An item the model does not hold is no `0x5e1840`: nothing is cleared, nothing set.
    #[test]
    fn an_unknown_item_leaves_the_cursor_alone() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(bag(&[(1, 0x11, 4), (2, 0x12, 4)])));
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert_eq!(s.auto_equip_guid(0x99), AutoEquip::None);
        assert_eq!(s.auto_equip_slot(0, 7), AutoEquip::None);
        assert_eq!(s.cursor_item().map(|i| i.guid), Some(0x11));
        assert!(s.item_locked(0x11));
    }

    /// The set's inner clear drops a held item with an unlock; the bar's item never locks; the
    /// send leaves the cursor empty and queues nothing, the caller sends.
    #[test]
    fn the_pick_drops_the_held_item_and_the_send_clears_the_cursor() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(bag(&[(1, 0x11, 4), (2, 0x12, 4)])));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.item_locked(0x11));

        assert_eq!(s.auto_equip_guid(0x12), AutoEquip::Sent { bag: 0, slot: 2 });
        assert!(s.cursor_item().is_none());
        assert!(!s.item_locked(0x11), "the held item went back unlocked");
        assert!(!s.item_locked(0x12));
        assert!(s.take_container_autoequips().is_empty());
    }

    /// A projectile's send clears with the unlock (`0x5e1711`, `0x5e1763`), signalling it.
    #[test]
    fn a_projectile_signals_the_unlock_of_its_clear() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(bag(&[(1, 0x11, 6)])));
        s.run(
            "SIGNALS = 0 local f = CreateFrame('Frame') f:RegisterEvent('ITEM_LOCK_CHANGED') \
             f:SetScript('OnEvent', function() SIGNALS = SIGNALS + 1 end)",
        )
        .unwrap();
        assert_eq!(s.auto_equip_slot(0, 1), AutoEquip::Sent { bag: 0, slot: 1 });
        assert_eq!(s.eval::<i64>("return SIGNALS").unwrap(), 1);
    }

    /// A worn item goes back (`0x5e155a`), nothing sent.
    #[test]
    fn a_worn_item_goes_back() {
        let mut s = UiScript::new().unwrap();
        let mut doll: crate::script::InventorySlots = Default::default();
        doll[1] = Some(crate::script::InvSlotView {
            item_id: 1234,
            guid: 0x31,
            equip_slots: vec![1],
            ..Default::default()
        });
        s.set_inventory_slots(doll);
        s.run("PickupInventoryItem(1)").unwrap();
        assert_eq!(
            s.cursor_item().map(|i| i.bag),
            Some(EQUIPMENT_BAG),
            "held from the doll"
        );
        s.run("AutoEquipCursorItem()").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_autoequips().is_empty());
    }
}
