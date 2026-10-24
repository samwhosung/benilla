//! `AutoEquipCursorItem` (`0x5e1480`) and the verb that puts an item on the cursor before calling it
//! (`0x5e1840`): the bag's right-click, the action bar's equip and the bind-confirm accept.

use mlua::Lua;

use crate::script::tick::verb;
use crate::script::{ItemCall, Model};

use super::{
    clear, clear_cursor, set_cursor, Clear, CursorItem, CursorPayload, EQUIPMENT_BAG,
    ITEM_CLASS_PROJECTILE,
};

/// What `AutoEquipCursorItem` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoEquip {
    /// Nothing: no item held, no item to put on the cursor, or a held item whose object is gone
    /// (`0x5e15bb`), which stays held.
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
    /// The held item's record is not cached: the lookup asked it with the callback `0x5e17a0`
    /// and the call returned (`0x5e160c`), nothing sent and the item still held. The record's
    /// arrival runs the call again ([`crate::script::UiScript::replay_auto_equip`]).
    Waits { entry: u32 },
}

/// The template bonding `AutoEquipCursorItem` and `SwapItem` ask on (`0x5e163b`, `0x5e0e54`):
/// bind when equipped.
pub const BIND_WHEN_EQUIPPED: u32 = 2;

/// What the two equip questions read of the item object they would equip: its entry and whether
/// it is bound (`0x5da2c0`). Its record, the bonding and whether it is cached, they look up in
/// the item cache by the entry ([`equip_question`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct EquipFacts {
    pub(super) item_id: u32,
    pub(super) bound: bool,
}

impl EquipFacts {
    /// A bag slot's item.
    pub(super) fn of_slot(s: &crate::script::ContainerSlot) -> Self {
        Self {
            item_id: s.item_id,
            bound: s.already_bound,
        }
    }

    /// A player slot's item.
    pub(super) fn of_inv(v: &crate::script::InvSlotView) -> Self {
        Self {
            item_id: v.item_id,
            bound: v.already_bound,
        }
    }
}

/// The answer of an equip question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Question {
    /// No question: the move goes out.
    Send,
    /// The bind-on-equip question.
    Ask,
    /// The record of `entry` is not cached: the move waits for it.
    Wait(u32),
}

/// The question `AutoEquipCursorItem` and `SwapItem` share, in their order: a bound item
/// (`0x5e15d6`, `0x5e0d75`) or a set `suppress` (`0x5e15e3`, `0x5e0d82`) sends; else the record
/// lookup in the item cache (`0x5e1603`, `0x5e0db1`) waits on a miss; else the record's bonding
/// `+0x194` is 2 (`0x5e163b`, `0x5e0e54`) and the player can use the item (`0x5ea930`) to ask.
/// No quality leg. The cache is the VM's copy of the app's item records (`Model::item_templates`,
/// seated whole for a new VM and topped up as each record lands), not a bag view's fields.
pub(super) fn equip_question(model: &Model, item: EquipFacts, suppress: bool) -> Question {
    if item.bound || suppress {
        return Question::Send;
    }
    let Some(record) = model.item_templates.get(&item.item_id) else {
        return Question::Wait(item.item_id);
    };
    if record.bonding == BIND_WHEN_EQUIPPED
        && crate::script::item_stats::item_usable_by_id(model, item.item_id)
    {
        Question::Ask
    } else {
        Question::Send
    }
}

/// The held item's object as `AutoEquipCursorItem` finds it by guid (`0x5e15b2`): the view of the
/// slot it came from, else any bag slot or player slot holding that guid.
fn held_facts(model: &Model, item: &CursorItem) -> Option<EquipFacts> {
    let from_slot = |bag: i64, slot: u32| {
        if bag == EQUIPMENT_BAG {
            usize::try_from(slot)
                .ok()
                .and_then(|slot| model.player_slot_object(slot))
                .filter(|v| v.guid == item.guid)
                .map(|v| EquipFacts::of_inv(&v))
        } else {
            model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .filter(|s| s.guid == item.guid)
                .map(EquipFacts::of_slot)
        }
    };
    from_slot(item.bag, item.slot).or_else(|| {
        model
            .containers
            .values()
            .flat_map(|c| c.slots.values())
            .find(|s| s.guid == item.guid)
            .map(EquipFacts::of_slot)
    })
}

/// `AutoEquipCursorItem(suppress)` on the held item: an item from an equipment or equipped-bag
/// slot, the 0-based slot below 0x17, goes back with `ClearCursor(1,1)` (`0x5e155a`, `0x5e1567`);
/// any other item, from a bag, the backpack, the bank, a bank bag slot or the keyring, is found
/// by its guid (`0x5e15b2`, none returning with nothing done), then asked about, parked on its
/// record or sent ([`equip_question`]), the server picking the slot and swapping the displaced
/// piece back (`ItemHandler.cpp:138-228`). A split carry is no exception: the call never reads
/// the split count `0xb4b40c`. The send tail clears with `ClearCursor(ebx,1)` (`0x5e1761`): a
/// projectile loads by entry with `CMSG_SET_AMMO` and stays in its bag, so its arm sets
/// `ebx = 1` (`0x5e1704`, `0x5e1711`) and the item unlocks; every other arm keeps the lock for
/// the server's answer (`0x5e1577`). Neither locks anything new.
pub(crate) fn auto_equip_held(model: &mut Model, suppress: bool) -> AutoEquip {
    let item = match &model.cursor {
        Some(CursorPayload::Item(item)) => item.clone(),
        _ => return AutoEquip::None,
    };
    if item.bag == EQUIPMENT_BAG && item.slot <= 23 {
        clear_cursor(model);
        return AutoEquip::Back;
    }
    let Some(facts) = held_facts(model, &item) else {
        return AutoEquip::None;
    };
    let (bag, slot, guid) = (item.bag, item.slot, item.guid);
    match equip_question(model, facts, suppress) {
        Question::Ask => return AutoEquip::Asked { bag, slot, guid },
        Question::Wait(entry) => return AutoEquip::Waits { entry },
        Question::Send => {}
    }
    let how = if item.item_class == Some(ITEM_CLASS_PROJECTILE) {
        Clear::BOTH
    } else {
        Clear::KEEP_LOCK
    };
    clear(model, how);
    AutoEquip::Sent { bag, slot, guid }
}

/// File what [`auto_equip_held`] did for the host ([`ItemCall::AutoEquip`],
/// [`ItemCall::EquipAsk`], [`ItemCall::AutoEquipWait`]). Returns whether to repaint.
pub(super) fn file_auto_equip(model: &mut Model, step: AutoEquip) -> bool {
    match step {
        AutoEquip::Sent { bag, slot, .. } => {
            model.item_calls.push(ItemCall::AutoEquip { bag, slot });
            true
        }
        AutoEquip::Asked { bag, slot, guid } => {
            model
                .item_calls
                .push(ItemCall::EquipAsk { bag, slot, guid });
            false
        }
        AutoEquip::Waits { entry } => {
            model.item_calls.push(ItemCall::AutoEquipWait { entry });
            false
        }
        AutoEquip::Back => true,
        AutoEquip::None => false,
    }
}

/// `AutoEquipCursorItem()` (`0x48a040` into `0x5e1480`), a click or drop on the doll's model
/// (`PaperDollFrame.lua:31-35`), filing the send, the question or the wait for the host
/// ([`file_auto_equip`]). Returns whether to repaint.
pub(crate) fn auto_equip_cursor_item(model: &mut Model) -> bool {
    let step = auto_equip_held(model, false);
    file_auto_equip(model, step)
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
pub(crate) fn auto_equip_from(
    lua: &Lua,
    item: CursorItem,
    suppress: bool,
) -> mlua::Result<AutoEquip> {
    // An item offered in the trade window shows `ERR_EQUIP_TRADE_ITEM` inside the verb and ends
    // it before the set (`0x5e1897`-`0x5e18b3`, `0x4bfd20`).
    let in_trade = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .trade_holds(item.guid);
    if in_trade {
        crate::script::calls::display_error(lua, "ERR_EQUIP_TRADE_ITEM")?;
        return Ok(AutoEquip::None);
    }
    let control = has_cursor_control(&lua.app_data_ref::<Model>().expect("model app_data"));
    if control {
        set_cursor(lua, true, |_| Some(CursorPayload::Item(item)));
    }
    Ok(verb(lua, |model| auto_equip_held(model, suppress)))
}

impl crate::script::UiScript {
    /// The callback `0x5e17a0` for the found record of `entry`, which a parked
    /// `AutoEquipCursorItem` asked (a refusal does nothing, `0x5e17c3`): with the cursor still
    /// holding an item of that entry (`0x5e1800`-`0x5e1806`), `AutoEquipCursorItem(0)` runs again
    /// on it (`0x5e182b`), its send, question or wait applied through the host.
    pub fn replay_auto_equip(&mut self, entry: u32) -> mlua::Result<()> {
        verb(&self.lua, |model| {
            let holds = matches!(
                &model.cursor,
                Some(CursorPayload::Item(item)) if item.item_id == entry
            );
            if holds {
                auto_equip_cursor_item(model);
            }
        });
        crate::script::item_calls::submit_item_calls(&self.lua)
    }

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
        let Some(item) = item else {
            return AutoEquip::None;
        };
        auto_equip_from(&self.lua, item, suppress).unwrap_or_else(|e| {
            self.push_error(e);
            AutoEquip::None
        })
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
            name: None,
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
                            // The record cached.
                            quality: Some(1),
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
        s.seat_item_records();
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
        s.seat_item_records();
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

    /// `AutoEquipCursorItem` never reads the split count `0xb4b40c` (`0x5e1480`-`0x5e1790`): a
    /// held split stack of arrows equips like any whole one, sent and cleared with the unlock
    /// (`0x5e1711`, `0x5e1763`).
    #[test]
    fn a_held_split_stack_equips_like_any_other() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut arrows = bag(&[(1, 0x11, 6)]);
        arrows.slots.get_mut(&1).unwrap().count = 200;
        s.set_container(0, Some(arrows));
        s.seat_item_records();
        s.run("SplitContainerItem(0, 1, 50) AutoEquipCursorItem()")
            .unwrap();
        assert!(s.cursor_item().is_none(), "the split left the cursor");
        assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);
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
            name: None,
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
                    quality: Some(2),
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
        s.keep_calls();
        s.set_container(0, Some(boe_bag(2, false)));
        s.seat_item_records();
        s.run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
            .unwrap();

        assert_eq!(s.cursor_item().map(|i| i.guid), Some(0x11), "still held");
        assert!(s.item_locked(0x11), "and locked");
        assert!(s.take_container_autoequips().is_empty(), "nothing is sent");
        assert_eq!(s.take_container_equip_asks(), vec![(0, 1, 0x11)]);
    }

    /// The held item's record is not cached: `AutoEquipCursorItem` asks it with the callback
    /// `0x5e17a0` and returns (`0x5e160c`), nothing sent and the item still held.
    #[test]
    fn an_uncached_item_waits_for_its_record_held() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut pending = boe_bag(0, false);
        pending.slots.get_mut(&1).unwrap().quality = None;
        s.set_container(0, Some(pending));
        s.run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
            .unwrap();
        assert_eq!(s.cursor_item().map(|i| i.guid), Some(0x11), "still held");
        assert!(s.take_container_autoequips().is_empty(), "nothing is sent");
    }

    /// The record's callback (`0x5e17a0`) runs `AutoEquipCursorItem(0)` again while the cursor
    /// holds an item of that entry (`0x5e1800`-`0x5e182b`): another entry, or a cursor emptied
    /// meanwhile, does nothing; the held item sends once the view holds its record.
    #[test]
    fn the_records_arrival_runs_the_auto_equip_again() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut pending = boe_bag(0, false);
        pending.slots.get_mut(&1).unwrap().quality = None;
        s.set_container(0, Some(pending));
        s.run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
            .unwrap();
        s.set_container(0, Some(boe_bag(0, false)));
        s.seat_item_records();
        s.replay_auto_equip(870).unwrap();
        assert!(s.cursor_item().is_some(), "another entry");
        s.replay_auto_equip(871).unwrap();
        assert!(s.cursor_item().is_none(), "sent");
        assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);
        s.replay_auto_equip(871).unwrap();
        assert!(s.take_container_autoequips().is_empty(), "nothing held");
    }

    /// `AutoEquipCursorItem` looks the record up in the item cache (`0x5e1603`) and reads its
    /// bonding there (`0x5e163b`), whatever the bag view carries: no record cached waits, a
    /// cached bind-on-equip record asks.
    #[test]
    fn the_auto_equip_question_reads_the_record_cache() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_container(0, Some(boe_bag(0, false)));
        s.run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
            .unwrap();
        assert!(s.cursor_item().is_some(), "no record cached: it waits");
        assert!(s.take_container_autoequips().is_empty());
        s.set_item_template(
            871,
            crate::script::ItemTemplateView {
                bonding: 2,
                ..Default::default()
            },
        );
        s.run("AutoEquipCursorItem()").unwrap();
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
            s.keep_calls();
            s.set_container(0, Some(boe_bag(bonding, bound)));
            s.seat_item_records();
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
        s.keep_calls();
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
        s.keep_calls();
        s.set_container(0, Some(boe_bag(2, false)));
        s.seat_item_records();
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
        s.keep_calls();
        s.set_container(0, Some(boe_bag(0, false)));
        s.seat_item_records();
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
        s.keep_calls();
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
        s.keep_calls();
        s.open_trade_window(0x7);
        s.set_container(0, Some(bag(&[(1, 0x11, 4), (2, 0x12, 4)])));
        // Offer the second item, then hold the first.
        s.seat_item_records();
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
        // The check tests the partner cell first (`0x4bfd23`): the table outlives the close, the
        // refusal does not.
        s.close_trade_window();
        assert!(matches!(
            s.auto_equip_guid(0x12, false),
            AutoEquip::Sent { .. }
        ));
        assert!(s.take_ui_errors().is_empty());
    }

    /// `0x5e1840` sets the item on the cursor, the set's inner clear signalling `CURSOR_UPDATE`
    /// with the cursor emptied (`0x4952cc`) and the set its own with the item held, then sends it
    /// off, whose clear signals with the cursor empty again (`0x5e1761`). An engine step's handlers
    /// held for the host read the cursor as each of those steps left it, as they do in place.
    #[test]
    fn a_held_handler_reads_the_cursor_its_step_left() {
        for held in [false, true] {
            let mut s = UiScript::new().unwrap();
            s.set_container(0, Some(bag(&[(1, 0x11, 4), (2, 0x12, 4)])));
            s.seat_item_records();
            s.run(
                "SEEN = '' local f = CreateFrame('Frame') f:RegisterEvent('CURSOR_UPDATE') \
                 f:SetScript('OnEvent', function() SEEN = SEEN .. (CursorHasItem() and 1 or 0) end)",
            )
            .unwrap();
            let sent = if held {
                let sent = s.host_out(|s| s.auto_equip_guid(0x12, false));
                s.refuse_waiting();
                sent
            } else {
                s.auto_equip_guid(0x12, false)
            };
            assert!(matches!(sent, AutoEquip::Sent { guid: 0x12, .. }));
            assert_eq!(
                s.eval::<String>("return SEEN").unwrap(),
                "010",
                "held: {held}"
            );
            assert!(s.cursor_item().is_none(), "held: {held}");
        }
    }
}
