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
    /// The equip is sent, the item `guid` from this Lua `(bag, slot)` cleared off the cursor. A
    /// projectile loads by entry with `CMSG_SET_AMMO`, the app's to tell.
    Sent { bag: i64, slot: u32, guid: u64 },
    /// An unbound bind-on-equip item raised `AUTOEQUIP_BIND_CONFIRM` (`0x5e16c6`) and the call
    /// returned (`0x5e16ce`) with no `ClearCursor`: the item stays on the cursor, with whatever
    /// lock it held, for the question's answer. The app files the record under this `(bag, slot)`
    /// and `guid`.
    Asked { bag: i64, slot: u32, guid: u64 },
}

/// The template bonding `AutoEquipCursorItem` and `SwapItem` ask on (`0x5e163b`, `0x5e0e54`):
/// bind when equipped.
pub const BIND_WHEN_EQUIPPED: u32 = 2;

/// Whether `AutoEquipCursorItem` would ask about `item` (`0x5e15d6`-`0x5e1655`): the item is not
/// already bound (`0x5da2c0`), its template is cached with `bonding == 2` and the player can use
/// it (`0x5ea930`). The cache-hit conjunct fails open, as an item whose source slot no longer
/// holds it does here. No quality leg.
fn equip_asks(model: &Model, item: &CursorItem) -> bool {
    let view = if item.bag == EQUIPMENT_BAG {
        usize::try_from(item.slot)
            .ok()
            .and_then(|slot| model.inv_slot("player", slot))
            .filter(|v| v.guid == item.guid)
            .map(|v| (v.bonding, v.already_bound))
    } else {
        model
            .containers
            .get(&item.bag)
            .and_then(|c| c.slots.get(&item.slot))
            .filter(|s| s.guid == item.guid)
            .map(|s| (s.bonding, s.already_bound))
    };
    view.is_some_and(|(bonding, bound)| {
        bonding == BIND_WHEN_EQUIPPED
            && !bound
            && crate::script::item_stats::item_usable_by_id(model, item.item_id)
    })
}

/// `AutoEquipCursorItem(suppress)` on the held item: an item from an equipment or equipped-bag
/// slot, the 0-based slot below 0x17, goes back with `ClearCursor(1,1)` (`0x5e155a`, `0x5e1567`);
/// any other whole stack, from a bag, the backpack, the bank, a bank bag slot or the keyring, is
/// asked about when it binds on equip and `suppress` is clear, else sent (`0x5e1575`), the server
/// picking the slot and swapping the displaced piece back (`ItemHandler.cpp:138-228`); a split
/// stays held. The send tail clears with `ClearCursor(ebx,1)` (`0x5e1761`): a projectile loads by
/// entry with `CMSG_SET_AMMO` and stays in its bag, so its arm sets `ebx = 1` (`0x5e1704`,
/// `0x5e1711`) and the item unlocks; every other arm keeps the lock for the server's answer
/// (`0x5e1577`). Neither locks anything new.
pub(crate) fn auto_equip_held(model: &mut Model, suppress: bool) -> AutoEquip {
    match &model.cursor {
        Some(CursorPayload::Item(item)) if item.bag == EQUIPMENT_BAG && item.slot <= 23 => {
            clear_cursor(model);
            AutoEquip::Back
        }
        Some(CursorPayload::Item(item)) if item.count.is_none() => {
            let (bag, slot, guid) = (item.bag, item.slot, item.guid);
            if !suppress && equip_asks(model, item) {
                return AutoEquip::Asked { bag, slot, guid };
            }
            let how = if item.item_class == Some(ITEM_CLASS_PROJECTILE) {
                Clear::BOTH
            } else {
                Clear::KEEP_LOCK
            };
            clear(model, how);
            AutoEquip::Sent { bag, slot, guid }
        }
        _ => AutoEquip::None,
    }
}

/// `AutoEquipCursorItem()` (`0x48a040` into `0x5e1480`), a click or drop on the doll's model
/// (`PaperDollFrame.lua:31-35`), queueing the send or the question for the app. Returns whether
/// to repaint.
pub(crate) fn auto_equip_cursor_item(model: &mut Model) -> bool {
    match auto_equip_held(model, false) {
        AutoEquip::Sent { bag, slot, .. } => {
            model.container_autoequips.push((bag, slot));
            true
        }
        AutoEquip::Asked { bag, slot, guid } => {
            model.container_equip_asks.push((bag, slot, guid));
            false
        }
        AutoEquip::Back => true,
        AutoEquip::None => false,
    }
}

/// `SetCursorItem`'s control gate (`0x494b8d`): without control of the character
/// (`[0xb4b3e4]`, `SMSG_CLIENT_CONTROL_UPDATE`) the set returns before its clear, unless the
/// player is on a taxi (`UNIT_FLAG_TAXI_FLIGHT`). The UseContainerItem equip arm tests the same
/// pair first (`0x4fa3ce`-`0x4fa3ec`).
pub(crate) fn has_cursor_control(model: &Model) -> bool {
    model.player_control || model.taxi_riding
}

/// `0x5e1840`: `SetCursorItem(1, …)` puts the item on the cursor, its inner clear dropping and
/// unlocking whatever is held (`0x494bb6`), then `AutoEquipCursorItem(suppress)`. The item is not
/// locked. An item in the trade window is refused first with its line. A refused set (no control, not on a taxi) clears and sets nothing, and
/// `AutoEquipCursorItem` runs all the same (`0x5e18c9` falls through to `0x5e18d5` with no test),
/// on whatever the cursor holds.
pub(crate) fn auto_equip_from(lua: &Lua, item: CursorItem, suppress: bool) -> AutoEquip {
    // An item offered in the trade window shows `ERR_EQUIP_TRADE_ITEM` and ends the verb before
    // the set (`0x5e1897`-`0x5e18b3`, `0x4bfd20`).
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if model
            .trade_offered
            .iter()
            .flatten()
            .any(|offered| offered.guid == item.guid)
        {
            model.push_ui_error("ERR_EQUIP_TRADE_ITEM");
            return AutoEquip::None;
        }
    }
    let control = has_cursor_control(&lua.app_data_ref::<Model>().expect("model app_data"));
    if control {
        set_cursor(lua, true, |_| Some(CursorPayload::Item(item)));
    }
    verb(lua, |model| auto_equip_held(model, suppress))
}

impl crate::script::UiScript {
    /// `0x5e1840` on the item at Lua `(bag, slot)`: the bind-confirm accept re-issues it with
    /// `suppress` set (`0x5e1c23`). Nothing happens for an empty or unresolved slot (`0x5e18da`).
    pub fn auto_equip_slot(&mut self, bag: i64, slot: u32, suppress: bool) -> AutoEquip {
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
            Some(item) => auto_equip_from(&self.lua, item, suppress),
            None => AutoEquip::None,
        }
    }

    /// `0x5e1840` on the bag item `guid`, the action bar's equip: `UseAction` resolves an entry to
    /// the copy's container and slot (`0x6222b0`, `0x4e602a`).
    pub fn auto_equip_guid(&mut self, guid: u64, suppress: bool) -> AutoEquip {
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
            Some((bag, slot)) => self.auto_equip_slot(bag, slot, suppress),
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

        assert_eq!(s.auto_equip_guid(0x99, false), AutoEquip::None);
        assert_eq!(s.auto_equip_slot(0, 7, false), AutoEquip::None);
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

        assert_eq!(
            s.auto_equip_guid(0x12, false),
            AutoEquip::Sent {
                bag: 0,
                slot: 2,
                guid: 0x12
            }
        );
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
        assert_eq!(
            s.auto_equip_slot(0, 1, false),
            AutoEquip::Sent {
                bag: 0,
                slot: 1,
                guid: 0x11
            }
        );
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

    /// A one-slot backpack holding an item of `bonding`, `bound` or not, that equips in the head
    /// slot.
    fn boe_bag(bonding: u32, bound: bool) -> ContainerState {
        ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: [(
                1,
                ContainerSlot {
                    guid: 0x11,
                    item_id: 871,
                    count: 1,
                    bonding,
                    already_bound: bound,
                    item_class: Some(4),
                    equip_slots: vec![1],
                    use_fork: crate::script::UseFork::Equip,
                    ..Default::default()
                },
            )]
            .into(),
        }
    }

    /// An unbound bind-on-equip item keeps the cursor, and its lock, under the question: no
    /// `ClearCursor` (`0x5e16c6`, `0x5e16ce`), nothing sent.
    #[test]
    fn a_bind_on_equip_item_stays_held_and_locked_under_the_question() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(boe_bag(2, false)));
        s.run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
            .unwrap();

        assert_eq!(s.cursor_item().map(|i| i.guid), Some(0x11), "still held");
        assert!(s.item_locked(0x11), "and locked");
        assert!(s.take_container_autoequips().is_empty(), "nothing is sent");
        assert_eq!(s.take_container_equip_asks(), vec![(0, 1, 0x11)]);
    }

    /// The question is the reference's three conjuncts: bonding 2 (no other value, no quality
    /// leg), not already bound, and an item the player can use (`0x5e15d6`-`0x5e1655`).
    #[test]
    fn only_an_unbound_usable_bind_on_equip_item_asks() {
        for (bonding, bound, asks) in [
            (2, false, true),
            (2, true, false),
            (0, false, false),
            (1, false, false),
            (3, false, false),
        ] {
            let mut s = UiScript::new().unwrap();
            s.set_container(0, Some(boe_bag(bonding, bound)));
            s.run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
                .unwrap();
            assert_eq!(
                s.take_container_equip_asks().len(),
                usize::from(asks),
                "bonding {bonding}, bound {bound}"
            );
            assert_eq!(s.cursor_item().is_some(), asks, "bonding {bonding}");
        }
        // A level-60 axe on a level-10 player is never asked about (`0x5ea930`).
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(boe_bag(2, false)));
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
        s.run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
            .unwrap();
        assert!(s.take_container_equip_asks().is_empty());
        assert!(s.cursor_item().is_none());
    }

    /// The bag's right-click puts the item on the cursor first (`0x5e1840`) and, asked, leaves
    /// it there; the lock follows the call (`0x4fa418`).
    #[test]
    fn a_right_click_on_a_bind_on_equip_item_holds_it_under_the_question() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(boe_bag(2, false)));
        s.run("UseContainerItem(0, 1)").unwrap();

        assert_eq!(s.cursor_item().map(|i| i.guid), Some(0x11));
        assert!(s.item_locked(0x11));
        let calls = s.take_script_calls();
        let [crate::script::ScriptCall::UseContainerItem(u)] = calls.as_slice() else {
            panic!("one use call: {calls:?}");
        };
        assert!(u.asks);
        assert!(s.take_container_autoequips().is_empty());
    }

    /// Unbound and not asked, the right-click puts the item on the cursor and the send clears it
    /// again, the item locked by the call.
    #[test]
    fn a_right_click_on_an_ordinary_item_sends_and_leaves_the_cursor_empty() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(boe_bag(0, false)));
        s.run("UseContainerItem(0, 1)").unwrap();

        assert!(s.cursor_item().is_none());
        assert!(s.item_locked(0x11));
        let calls = s.take_script_calls();
        let [crate::script::ScriptCall::UseContainerItem(u)] = calls.as_slice() else {
            panic!("one use call: {calls:?}");
        };
        assert!(!u.asks);
    }

    /// The accept re-issues from the slot with `suppress` set (`0x5e1c23`): the held item drops
    /// and unlocks with `SetCursorItem`'s clear, and the equip goes out.
    #[test]
    fn the_accept_clears_the_cursor_and_sends_without_asking_again() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(boe_bag(2, false)));
        s.run("UseContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        assert_eq!(
            s.auto_equip_slot(0, 1, true),
            AutoEquip::Sent {
                bag: 0,
                slot: 1,
                guid: 0x11
            }
        );
        assert!(s.cursor_item().is_none());
        assert!(!s.item_locked(0x11), "SetCursorItem's clear unlocked it");
    }

    /// `0x5e1840` refuses an item offered in the trade window (`0x4bfd20`) with
    /// `ERR_EQUIP_TRADE_ITEM` before it touches the cursor: a held item stays held, nothing
    /// queues.
    #[test]
    fn an_item_in_the_trade_window_is_refused_before_the_set() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(bag(&[(1, 0x11, 4), (2, 0x12, 4)])));
        // Offer the second item, then hold the first.
        s.run("PickupContainerItem(0, 2) ClickTradeButton(1) ClearCursor()")
            .unwrap();
        s.unlock_item(0x12);
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert_eq!(s.auto_equip_guid(0x12, false), AutoEquip::None);
        assert_eq!(s.take_ui_errors(), vec!["ERR_EQUIP_TRADE_ITEM"]);
        assert_eq!(s.cursor_item().map(|i| i.guid), Some(0x11), "still held");
        assert!(s.item_locked(0x11), "and still locked");
        // An item not offered goes through.
        assert!(matches!(
            s.auto_equip_guid(0x11, false),
            AutoEquip::Sent { .. }
        ));
    }
}
