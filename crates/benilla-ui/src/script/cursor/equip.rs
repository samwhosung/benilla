//! `AutoEquipCursorItem` (`0x5e1480`) and the verb that puts an item on the cursor before calling it
//! (`0x5e1840`): the bag's right-click, the action bar's equip and the bind-confirm accept.

use mlua::Lua;

use crate::script::tick::verb;
use crate::script::{ItemCall, Model};

use super::{
    clear, clear_cursor, set_cursor, Clear, CursorItem, CursorPayload, Setter, EQUIPMENT_BAG,
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
    /// The equip is sent, the item `guid` from this Lua `(bag, slot)` cleared off the cursor after
    /// it. A projectile loads by entry with `CMSG_SET_AMMO` ([`ItemCall::AutoEquip`]'s `ammo`).
    Sent { bag: i64, slot: u32, guid: u64 },
    /// An unbound bind-on-equip item raised `AUTOEQUIP_BIND_CONFIRM` (`0x5e16c6`) and the call
    /// returned (`0x5e16ce`) with no `ClearCursor`: the item stays on the cursor, with whatever
    /// lock it held, for the question's answer. The app files the record under this `(bag, slot)`
    /// and `guid`.
    Asked { bag: i64, slot: u32, guid: u64 },
    /// Held ammo, mode 9, loads by its `entry` with `CMSG_SET_AMMO` and the cursor clears with
    /// `ClearCursor(1,1)` (`0x5e1486`-`0x5e14ec`).
    Ammo { entry: u32 },
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

/// Hand what `AutoEquipCursorItem` decided to the host in its place among the segment's signals
/// ([`Model::item_call`]): the reference sends in its body ahead of its clear (`0x5e1757` then
/// `0x5e1763`; held ammo `0x5e14e0` then `0x5e14ec`), and raises its question (`0x5e16c6`) or asks
/// the record (`0x5e1603`) there too. `ammo`: the sent item is a projectile of that entry
/// ([`projectile_entry`]). `picked`: `0x5e1840` put the item on the cursor in the same call, so
/// the send carries the pickup and put-down that land inside it.
fn hand_over(model: &mut Model, step: AutoEquip, ammo: Option<u32>, picked: bool) {
    let call = match step {
        AutoEquip::None | AutoEquip::Back => return,
        AutoEquip::Sent { bag, slot, .. } => ItemCall::AutoEquip {
            bag,
            slot,
            ammo,
            picked,
        },
        AutoEquip::Asked { bag, slot, guid } => ItemCall::EquipAsk { bag, slot, guid },
        AutoEquip::Waits { entry } => ItemCall::AutoEquipWait { entry },
        AutoEquip::Ammo { entry } => ItemCall::SetAmmo { entry },
    };
    model.item_call(call);
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
/// No quality leg. `record` is the item's record as the lookup finds it in the cache at the call
/// ([`crate::script::item_stats::cached_record`]), not a bag view's fields.
pub(super) fn equip_question(
    lua: &Lua,
    model: &Model,
    item: EquipFacts,
    record: Option<&crate::script::ItemTemplateView>,
    suppress: bool,
) -> Question {
    if item.bound || suppress {
        return Question::Send;
    }
    let Some(record) = record else {
        return Question::Wait(item.item_id);
    };
    if record.bonding == BIND_WHEN_EQUIPPED
        && crate::script::item_stats::record_refusal(lua, model, record).is_none()
    {
        Question::Ask
    } else {
        Question::Send
    }
}

/// The held item's object as `AutoEquipCursorItem` finds it by guid (`0x5e15b2`): the view of the
/// slot it came from, else any bag slot or player slot holding that guid.
pub(super) fn held_facts(lua: &Lua, model: &Model, item: &CursorItem) -> Option<EquipFacts> {
    let from_slot = |bag: i64, slot: u32| {
        if bag == EQUIPMENT_BAG {
            usize::try_from(slot)
                .ok()
                .and_then(|slot| crate::script::char_stats::player_slot_object(lua, model, slot))
                .filter(|v| v.guid == item.guid)
                .map(|v| EquipFacts::of_inv(&v))
        } else {
            crate::script::held::held_slot(lua, model, bag, slot)
                .filter(|s| s.guid == item.guid)
                .map(|s| EquipFacts::of_slot(&s))
        }
    };
    from_slot(item.bag, item.slot).or_else(|| {
        crate::script::held::find_guid(lua, model, item.guid)
            .map(|(_, _, s)| EquipFacts::of_slot(&s))
    })
}

/// `AutoEquipCursorItem(suppress)` on the held item: an item from an equipment or equipped-bag
/// slot, the 0-based slot below 0x17, goes back with `ClearCursor(1,1)` (`0x5e155a`, `0x5e1567`);
/// any other item, from a bag, the backpack, the bank, a bank bag slot or the keyring, is found
/// by its guid (`0x5e15b2`, none returning with nothing done), then asked about, parked on its
/// record or sent ([`equip_question`]), the server picking the slot and swapping the displaced
/// piece back (`ItemHandler.cpp:138-228`). A split carry is no exception: the call never reads
/// the split count `0xb4b40c`. The send (`0x5e1757`) goes to the host in place ahead of its clear,
/// `ClearCursor(ebx,1)` (`0x5e1763`) ([`hand_over`], `picked` passed on): a projectile, by the
/// found item's record ([`projectile_entry`]), loads by entry with `CMSG_SET_AMMO` and stays in its bag, so its arm sets
/// `ebx = 1` (`0x5e1704`, `0x5e1711`) and the item unlocks; every other arm keeps the lock for
/// the server's answer (`0x5e1577`). Neither locks anything new.
pub(crate) fn auto_equip_held(
    lua: &Lua,
    model: &mut Model,
    suppress: bool,
    picked: bool,
) -> AutoEquip {
    let item = match &model.cursor {
        Some(CursorPayload::Item(item)) => item.clone(),
        // A zero entry sends nothing (`0x5e14a6`); else the send, then `ClearCursor(1,1)`.
        Some(CursorPayload::Ammo(ammo)) if ammo.item_id != 0 => {
            let step = AutoEquip::Ammo {
                entry: ammo.item_id,
            };
            hand_over(model, step, None, picked);
            clear_cursor(model);
            return step;
        }
        _ => return AutoEquip::None,
    };
    if item.bag == EQUIPMENT_BAG && item.slot <= 23 {
        clear_cursor(model);
        return AutoEquip::Back;
    }
    let Some(facts) = held_facts(lua, model, &item) else {
        return AutoEquip::None;
    };
    let (bag, slot, guid) = (item.bag, item.slot, item.guid);
    let record = crate::script::item_stats::cached_record(lua, model, facts.item_id);
    let step = match equip_question(lua, model, facts, record.as_ref(), suppress) {
        Question::Ask => AutoEquip::Asked { bag, slot, guid },
        Question::Wait(entry) => AutoEquip::Waits { entry },
        Question::Send => AutoEquip::Sent { bag, slot, guid },
    };
    let ammo = projectile_entry(record.as_ref(), facts.item_id);
    hand_over(model, step, ammo, picked);
    if let AutoEquip::Sent { .. } = step {
        clear(
            model,
            if ammo.is_some() {
                Clear::BOTH
            } else {
                Clear::KEEP_LOCK
            },
        );
    }
    step
}

/// The sent item's entry when its record, in the item cache `GetItemClass` reads (`0x5d9f30`
/// at `0x5e16ff`), is a projectile's (`0x5e1704`): the one place that decides between
/// `CMSG_SET_AMMO` and `CMSG_AUTOEQUIP_ITEM`, and the clear's unlock. An uncached record is none.
fn projectile_entry(record: Option<&crate::script::ItemTemplateView>, item_id: u32) -> Option<u32> {
    record
        .is_some_and(|t| t.class == ITEM_CLASS_PROJECTILE)
        .then_some(item_id)
}

/// Whether what [`auto_equip_held`] did changed the cursor, so the caller repaints.
pub(super) fn repaints(step: AutoEquip) -> bool {
    matches!(
        step,
        AutoEquip::Sent { .. } | AutoEquip::Ammo { .. } | AutoEquip::Back
    )
}

/// `AutoEquipCursorItem()` (`0x48a040` into `0x5e1480`), a click or drop on the doll's model
/// (`PaperDollFrame.lua:31-35`), its send, question or wait handed to the host in place. Returns
/// whether to repaint.
pub(crate) fn auto_equip_cursor_item(lua: &Lua, model: &mut Model) -> bool {
    repaints(auto_equip_held(lua, model, false, false))
}

/// `SetCursorItem`'s control gate (`0x494b8d`): without control of the character
/// (`[0xb4b3e4]`, `SMSG_CLIENT_CONTROL_UPDATE`) the set returns before its clear, unless the
/// player is on a taxi (`UNIT_FLAG_TAXI_FLIGHT`), read off the active player's object at the call
/// (`0x494b97`-`0x494ba6`) through the lent host, else off `model`'s copy. The UseContainerItem
/// equip arm (`0x4fa3ce`-`0x4fa3ec`), `PickupContainerItem` (`0x4f9b87`-`0x4f9ba3`) and
/// `PickupInventoryItem` (`0x4c7345`-`0x4c7360`) test the same pair first.
pub(crate) fn has_cursor_control(lua: &Lua, model: &Model) -> bool {
    use crate::script::SelfRead;
    if model.player_control {
        return true;
    }
    let mut live = None;
    crate::script::host::read_host(lua, |h| h.read_self(&mut |own| live = Some(own.on_taxi())));
    live.unwrap_or_else(|| model.on_taxi())
}

/// `0x5e1840`: `SetCursorItem(1, …)` puts the item on the cursor, its inner clear dropping and
/// unlocking whatever is held (`0x494bb6`), then `AutoEquipCursorItem(suppress)`. The item is not
/// locked. An item in the trade window is refused first with its line. A refused set (no control,
/// not on a taxi, [`Setter::Item`]) clears nothing and sets nothing, and `AutoEquipCursorItem` runs
/// all the same (`0x5e18c9` falls through to `0x5e18d5` with no test), on whatever the cursor
/// holds.
pub(crate) fn auto_equip_from(
    lua: &Lua,
    item: CursorItem,
    suppress: bool,
) -> mlua::Result<AutoEquip> {
    let Some(picked) = put_for_equip(lua, item)? else {
        return Ok(AutoEquip::None);
    };
    Ok(auto_equip_on_cursor(lua, suppress, picked))
}

/// The first half of `0x5e1840`: the trade window's refusal (`0x5e1897`-`0x5e18b3`, `0x4bfd20`),
/// shown inside the verb, ends it before the set, and the set itself (`0x5e18c9`), whose signals
/// all fire ahead of `AutoEquipCursorItem`. `None` when the refusal ended it, else whether the
/// set put the item on: with no control `SetCursorItem` returns before its clear and its pickup
/// (`0x494b8d`-`0x494ba9` → `0x494c59`), and `AutoEquipCursorItem` still runs on what is held.
pub(crate) fn put_for_equip(lua: &Lua, item: CursorItem) -> mlua::Result<Option<bool>> {
    let in_trade = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .trade_holds(item.guid);
    if in_trade {
        crate::script::calls::display_error(lua, "ERR_EQUIP_TRADE_ITEM")?;
        return Ok(None);
    }
    Ok(Some(set_cursor(lua, true, Setter::Item, |_| {
        Some(CursorPayload::Item(item))
    })))
}

/// The second half of `0x5e1840`: `AutoEquipCursorItem(suppress)` (`0x5e18d5`) on whatever the
/// cursor holds now, the signals of the set and of any listener between having run. Its send
/// carries the set's pickup when the set ran (`picked`, [`hand_over`]).
pub(crate) fn auto_equip_on_cursor(lua: &Lua, suppress: bool, picked: bool) -> AutoEquip {
    verb(lua, |model| auto_equip_held(lua, model, suppress, picked))
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
                auto_equip_cursor_item(&self.lua, model);
            }
        });
        Ok(())
    }

    /// The item at Lua `(bag, slot)` as the cursor takes it; none for an empty or unresolved slot
    /// (`0x5e18da`).
    fn item_at(&self, bag: i64, slot: u32) -> Option<CursorItem> {
        let model = self.model_ref();
        crate::script::held::held_slot(&self.lua, &model, bag, slot)
            .filter(|s| s.guid != 0)
            .map(|s| s.cursor_item(bag, slot))
    }

    /// `0x5e1840` on the item at Lua `(bag, slot)`: the bind-confirm accept re-issues it with
    /// `suppress` set (`0x5e1c23`). Nothing happens for an empty or unresolved slot (`0x5e18da`).
    /// With the host out applying the accept, the set's signals, the send handed over in place
    /// and the clear's signals run in that order as it settles.
    pub fn auto_equip_slot(&mut self, bag: i64, slot: u32, suppress: bool) -> AutoEquip {
        let Some(item) = self.item_at(bag, slot) else {
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
            crate::script::held::find_guid(&self.lua, &model, guid)
                .map(|(bag, slot, _)| (bag, slot))
        };
        match at {
            Some((bag, slot)) => self.auto_equip_slot(bag, slot, suppress),
            None => AutoEquip::None,
        }
    }

    /// The first half of `0x5e1840` on the bag item `guid` ([`put_for_equip`]), for the bar's
    /// equip, whose set signals (`0x494bb6`, `0x494c54`) fire before the send. Returns whether
    /// [`Self::auto_equip_cursor`] follows: an item the model does not hold is no `0x5e1840`
    /// (`0x5e18da`), and the trade window's refusal ends it. `Some(picked)`: whether the set put
    /// the item on, which [`Self::auto_equip_cursor`] takes.
    pub fn put_on_cursor_for_equip(&mut self, guid: u64) -> Option<bool> {
        let at = {
            let model = self.model_ref();
            crate::script::held::find_guid(&self.lua, &model, guid)
                .map(|(bag, slot, _)| (bag, slot))
        };
        let item = at.and_then(|(bag, slot)| self.item_at(bag, slot))?;
        put_for_equip(&self.lua, item).unwrap_or_else(|e| {
            self.push_error(e);
            None
        })
    }

    /// The second half of `0x5e1840`: `AutoEquipCursorItem(suppress)` on whatever the cursor
    /// holds now ([`auto_equip_on_cursor`]), its send handed to the host in place: the bar's
    /// equip, whose segment is held while its call applies and runs as it settles. `picked`: the
    /// first half's set put the item on ([`Self::put_on_cursor_for_equip`]).
    pub fn auto_equip_cursor(&mut self, suppress: bool, picked: bool) -> AutoEquip {
        auto_equip_on_cursor(&self.lua, suppress, picked)
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
        s.keep_calls();
        s.set_container(0, Some(bag(&[(1, 0x11, 4), (2, 0x12, 4)])));
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert_eq!(s.auto_equip_guid(0x99, false), AutoEquip::None);
        assert_eq!(s.auto_equip_slot(0, 7, false), AutoEquip::None);
        assert_eq!(s.cursor_item().map(|i| i.guid), Some(0x11));
        assert!(s.item_locked(0x11));
    }

    /// The set's inner clear drops a held item with an unlock; the bar's item never locks; the
    /// send is handed over in place and leaves the cursor empty.
    #[test]
    fn the_pick_drops_the_held_item_and_the_send_clears_the_cursor() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        assert_eq!(s.take_container_autoequips(), vec![(0, 2)]);
    }

    /// The bar's second half hands its send over in place, carrying the set's pickup.
    #[test]
    fn the_bars_second_half_hands_its_send_over() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_container(0, Some(bag(&[(1, 0x11, 4)])));
        s.seat_item_records();
        assert_eq!(s.put_on_cursor_for_equip(0x11), Some(true));
        assert!(matches!(
            s.auto_equip_cursor(false, true),
            AutoEquip::Sent { guid: 0x11, .. }
        ));
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_item_calls(),
            [crate::script::ItemCall::AutoEquip {
                bag: 0,
                slot: 1,
                ammo: None,
                picked: true
            }]
        );
    }

    /// A projectile's send clears with the unlock (`0x5e1711`, `0x5e1763`), signalling it.
    #[test]
    fn a_projectile_signals_the_unlock_of_its_clear() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);
    }

    /// The item cache's record decides the projectile (`0x5e16ff` → `GetItemClass 0x5d9f30`), not
    /// the class the item took the cursor with: arrows picked up before their record landed load
    /// by entry and unlock once it has, and the call carries that decision.
    #[test]
    fn a_projectile_held_before_its_record_landed_loads_by_the_record() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let mut arrows = bag(&[(1, 0x11, 6)]);
        arrows.slots.get_mut(&1).unwrap().item_class = None;
        s.set_container(0, Some(arrows));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.item_locked(0x11));
        s.set_item_template(
            101,
            crate::script::ItemTemplateView {
                class: super::ITEM_CLASS_PROJECTILE,
                ..Default::default()
            },
        );
        s.run("AutoEquipCursorItem()").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(!s.item_locked(0x11), "the clear unlocked it");
        assert_eq!(
            s.take_item_calls(),
            [crate::script::ItemCall::AutoEquip {
                bag: 0,
                slot: 1,
                ammo: Some(101),
                picked: false
            }]
        );
    }

    /// With no control `SetCursorItem` returns before its clear and its pickup (`0x494b8d` →
    /// `0x494c59`), and `AutoEquipCursorItem` sends what is held: the send carries no pickup.
    #[test]
    fn a_refused_set_sends_the_held_item_without_its_pickup() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_container(0, Some(bag(&[(1, 0x11, 4), (2, 0x12, 4)])));
        s.seat_item_records();
        s.run("PickupContainerItem(0, 1)").unwrap();
        s.set_player_control(false);
        assert_eq!(s.put_on_cursor_for_equip(0x12), Some(false));
        assert!(matches!(
            s.auto_equip_cursor(false, false),
            AutoEquip::Sent { guid: 0x11, .. }
        ));
        assert_eq!(
            s.take_item_calls(),
            [crate::script::ItemCall::AutoEquip {
                bag: 0,
                slot: 1,
                ammo: None,
                picked: false
            }]
        );
        s.set_player_control(true);
        s.unlock_item(0x11);
        s.run("PickupContainerItem(0, 1)").unwrap();
        s.set_player_control(false);
        assert!(matches!(
            s.auto_equip_slot(0, 2, true),
            AutoEquip::Sent { guid: 0x11, .. }
        ));
        assert!(matches!(
            s.take_item_calls()[..],
            [crate::script::ItemCall::AutoEquip { picked: false, .. }]
        ));
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
        assert_eq!(s.take_container_equip_asks(), vec![(0, 1, 0x11)]);
        assert!(s.take_script_calls().is_empty(), "the question alone");
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
        assert_eq!(
            s.take_item_calls(),
            [crate::script::ItemCall::AutoEquip {
                bag: 0,
                slot: 1,
                ammo: None,
                picked: true
            }]
        );
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
            s.keep_calls();
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
