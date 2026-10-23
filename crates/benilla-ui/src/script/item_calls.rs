//! The engine half of the bag, cursor and paper-doll verbs, which the host applies inside each
//! verb's call ([`ItemCall`]): the reference's bindings send their packets in their own bodies,
//! so a chunk's item packets leave in call order with every other verb's, and its next line reads
//! what the verb did. A verb's segment files its calls on the model as it decides
//! ([`super::Model::item_calls`]), and the binding hands them to the host as the segment ends
//! ([`submit_item_calls`]).

use mlua::Lua;

use super::{BagAutoStore, ContainerMove, Model, PendingEquipAnswer, ScriptCall};

/// One item verb's send or answer past its binding's checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemCall {
    /// A split from the held item's source onto a slot (`0x5e1210`, `PickupContainerItem`'s
    /// `0x4f9e43`), or a whole stack's swap that `SwapItem` (`0x5e0c40`) sends past its question:
    /// `PickupContainerItem`'s (`0x4f9eac`), the doll click's (`0x4c78be`), `EquipCursorItem`'s
    /// (`0x489757`), the bag worker's (`0x4c7d8d`, `0x4c7df5`) and the accept's (`0x5e1c46`).
    Move(ContainerMove),
    /// `SwapItem`'s bind question (`0x5e0ee4`): the record filed and `EQUIP_BIND_CONFIRM`
    /// signalled with its index, the item still held.
    SwapAsk(ContainerMove),
    /// `SwapItem` on an item whose record is not cached (`0x5e0dbe`): the move parked in the
    /// pending-equip table and the record asked with the callback `0x5e1020`, which replays it
    /// ([`super::UiScript::replay_swap`]).
    SwapWait { mv: ContainerMove, entry: u32 },
    /// The bag worker's store or split into a bag (`0x4c7e2e`, `0x4c7e49`).
    AutoStore(BagAutoStore),
    /// The armed gift wrap's send (`0x4f9db3`): the paper's Lua `(bag, slot)`, then the target's.
    Wrap {
        gift_bag: i64,
        gift_slot: u32,
        item_bag: i64,
        item_slot: u32,
    },
    /// A repair-cursor click on a bag item (`0x4f9c7b`, `0x4fa1da`): the purse test, then
    /// `CMSG_REPAIR_ITEM`.
    Repair { bag: i64, slot: u32 },
    /// A repair-cursor click on a worn item, its 1-based doll slot (`0x4c7714`, `0x4c79c4`).
    RepairWorn(u32),
    /// `AutoEquipCursorItem`'s send past its question (`0x5e1480`): the item from Lua
    /// `(bag, slot)`.
    AutoEquip { bag: i64, slot: u32 },
    /// `AutoEquipCursorItem`'s bind question (`0x5e16c6`): the record and its event, filed with
    /// the item held.
    EquipAsk { bag: i64, slot: u32, guid: u64 },
    /// `AutoEquipCursorItem` on an item whose record is not cached: the record asked with the
    /// callback `0x5e17a0` (`0x5e15f4`-`0x5e1603`), which runs the call again
    /// ([`super::UiScript::replay_auto_equip`]).
    AutoEquipWait { entry: u32 },
    /// `DeleteCursorItem`'s destroy (`0x489854`), a 0 count being the whole stack.
    Destroy { bag: i64, slot: u32, count: u32 },
    /// `EquipPendingItem` (`0x4898f0`) or `CancelPendingEquip` (`0x489960`), into `0x5e1be0`.
    EquipAnswer(PendingEquipAnswer),
    /// `ConfirmBindOnUse()` (`0x48d770`): the parked use re-issued (`0x5d92c1`), its cast behind
    /// the gate's tag 2.
    BindOnUse { may_cast: bool },
}

/// Hand the host the item calls the verb's last segment filed, in the order filed, before the
/// verb returns.
pub(super) fn submit_item_calls(lua: &Lua) -> mlua::Result<()> {
    let calls = std::mem::take(
        &mut lua
            .app_data_mut::<Model>()
            .expect("model app_data")
            .item_calls,
    );
    for call in calls {
        super::calls::submit(lua, ScriptCall::Item(call))?;
    }
    Ok(())
}

/// Hand the host one item call now.
pub(super) fn submit(lua: &Lua, call: ItemCall) -> mlua::Result<()> {
    super::calls::submit(lua, ScriptCall::Item(call))
}

/// The item calls a harness's VM kept ([`super::UiScript::keep_calls`]), by kind.
impl super::UiScript {
    fn take_item_calls_where<T>(&mut self, mut pick: impl FnMut(&ItemCall) -> Option<T>) -> Vec<T> {
        self.take_calls_where(|call| match call {
            ScriptCall::Item(call) => pick(call),
            _ => None,
        })
    }

    /// Every item call kept, in call order.
    pub fn take_item_calls(&mut self) -> Vec<ItemCall> {
        self.take_item_calls_where(|call| Some(call.clone()))
    }

    /// The swaps and splits kept ([`ItemCall::Move`]).
    pub fn take_container_moves(&mut self) -> Vec<ContainerMove> {
        self.take_item_calls_where(|call| match call {
            ItemCall::Move(mv) => Some(mv.clone()),
            _ => None,
        })
    }

    /// The bag worker's stores kept ([`ItemCall::AutoStore`]).
    pub fn take_bag_autostores(&mut self) -> Vec<BagAutoStore> {
        self.take_item_calls_where(|call| match call {
            ItemCall::AutoStore(store) => Some(store.clone()),
            _ => None,
        })
    }

    /// The `(bag, slot)` auto-equip sends kept ([`ItemCall::AutoEquip`]).
    pub fn take_container_autoequips(&mut self) -> Vec<(i64, u32)> {
        self.take_item_calls_where(|call| match *call {
            ItemCall::AutoEquip { bag, slot } => Some((bag, slot)),
            _ => None,
        })
    }

    /// The `(bag, slot, guid)` auto-equip questions kept ([`ItemCall::EquipAsk`]).
    pub fn take_container_equip_asks(&mut self) -> Vec<(i64, u32, u64)> {
        self.take_item_calls_where(|call| match *call {
            ItemCall::EquipAsk { bag, slot, guid } => Some((bag, slot, guid)),
            _ => None,
        })
    }

    /// The `(bag, slot)` repair clicks kept ([`ItemCall::Repair`]).
    pub fn take_container_repairs(&mut self) -> Vec<(i64, u32)> {
        self.take_item_calls_where(|call| match *call {
            ItemCall::Repair { bag, slot } => Some((bag, slot)),
            _ => None,
        })
    }

    /// The worn repair clicks kept, by 1-based doll slot ([`ItemCall::RepairWorn`]).
    pub fn take_inventory_repairs(&mut self) -> Vec<u32> {
        self.take_item_calls_where(|call| match *call {
            ItemCall::RepairWorn(id) => Some(id),
            _ => None,
        })
    }

    /// The `(bag, slot, count)` destroys kept ([`ItemCall::Destroy`]).
    pub fn take_container_destroys(&mut self) -> Vec<(i64, u32, u32)> {
        self.take_item_calls_where(|call| match *call {
            ItemCall::Destroy { bag, slot, count } => Some((bag, slot, count)),
            _ => None,
        })
    }

    /// The `(giftBag, giftSlot, itemBag, itemSlot)` wraps kept ([`ItemCall::Wrap`]).
    pub fn take_container_wraps(&mut self) -> Vec<(i64, u32, i64, u32)> {
        self.take_item_calls_where(|call| match *call {
            ItemCall::Wrap {
                gift_bag,
                gift_slot,
                item_bag,
                item_slot,
            } => Some((gift_bag, gift_slot, item_bag, item_slot)),
            _ => None,
        })
    }

    /// The bind answers kept, in call order ([`ItemCall::EquipAnswer`]).
    pub fn take_pending_equip_answers(&mut self) -> Vec<PendingEquipAnswer> {
        self.take_item_calls_where(|call| match *call {
            ItemCall::EquipAnswer(answer) => Some(answer),
            _ => None,
        })
    }

    /// The `ConfirmBindOnUse()` calls kept, each with what the gate's tag 2 allowed it
    /// ([`ItemCall::BindOnUse`]).
    pub fn take_bind_on_use_casts(&mut self) -> Vec<bool> {
        self.take_item_calls_where(|call| match *call {
            ItemCall::BindOnUse { may_cast } => Some(may_cast),
            _ => None,
        })
    }

    /// How many `ConfirmBindOnUse()` calls were kept.
    pub fn take_bind_on_use_confirms(&mut self) -> u32 {
        self.take_bind_on_use_casts().len() as u32
    }
}

#[cfg(test)]
impl super::UiScript {
    /// Hand the host the item calls a model step filed outside its binding, as the binding would
    /// as its segment ends ([`submit_item_calls`]).
    pub(crate) fn submit_filed_item_calls(&mut self) {
        submit_item_calls(&self.lua).expect("the filed item calls submit");
    }
}
