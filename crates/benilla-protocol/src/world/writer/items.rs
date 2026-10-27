//! The bag and equipment sends. A move succeeds silently, as values deltas on both slots, and is
//! refused with `SMSG_INVENTORY_CHANGE_FAILURE`.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_ITEM_QUERY_SINGLE`: an item template by entry; `guid` is 0 for a template-only ask.
    pub fn item_query(&mut self, entry: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_ITEM_QUERY_SINGLE,
            &messages::item_query(entry, guid),
        )
    }

    /// `CMSG_ITEM_NAME_QUERY`: an item's name by entry; `guid` is the asker's, 0 for none.
    pub fn item_name_query(&mut self, entry: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_ITEM_NAME_QUERY,
            &messages::item_name_query(entry, guid),
        )
    }

    /// `CMSG_USE_ITEM`: use the item at a bag position; a GameObject `target` is how a key opens a
    /// locked door. Refused with `SMSG_CAST_RESULT`.
    pub fn use_item(
        &mut self,
        bag_index: u8,
        slot: u8,
        spell_slot: u8,
        target: messages::UseItemTarget,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_USE_ITEM,
            &messages::use_item(bag_index, slot, spell_slot, target),
        )
    }

    /// `CMSG_OPEN_ITEM`: open a clam, lockbox or gift; the loot window comes back on the item's
    /// own guid.
    pub fn open_item(&mut self, bag_index: u8, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_OPEN_ITEM,
            &messages::open_item(bag_index, slot),
        )
    }

    /// `CMSG_READ_ITEM`: read a book; the server answers `SMSG_READ_ITEM_OK` or
    /// `SMSG_READ_ITEM_FAILED` on the item's guid.
    pub fn read_item(&mut self, bag: u8, slot: u8) -> Result<()> {
        self.send(opcode::CMSG_READ_ITEM, &messages::read_item(bag, slot))
    }

    /// `CMSG_WRAP_ITEM`: wrap an item in the `ITEM_FLAG_WRAPPER` paper; success is silent and
    /// consumes one paper.
    pub fn wrap_item(
        &mut self,
        gift_bag: u8,
        gift_slot: u8,
        item_bag: u8,
        item_slot: u8,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_WRAP_ITEM,
            &messages::wrap_item(gift_bag, gift_slot, item_bag, item_slot),
        )
    }

    /// `CMSG_AUTOEQUIP_ITEM`: equip a bag item; the server picks the slot.
    pub fn auto_equip_item(&mut self, bag_index: u8, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_AUTOEQUIP_ITEM,
            &messages::auto_equip_item(bag_index, slot),
        )
    }

    /// `CMSG_SET_AMMO`: what the 1.12 client sends to auto-equip ammo. It names an item entry, not
    /// a slot; the stack stays in the bag and `PLAYER_AMMO_ID` points at it.
    pub fn set_ammo(&mut self, entry: u32) -> Result<()> {
        self.send(opcode::CMSG_SET_AMMO, &messages::set_ammo(entry))
    }

    /// `CMSG_SWAP_INV_ITEM`: two slots of the player's own grid; an empty destination is a move.
    pub fn swap_inv_item(&mut self, src_slot: u8, dst_slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_SWAP_INV_ITEM,
            &messages::swap_inv_item(src_slot, dst_slot),
        )
    }

    /// `CMSG_SWAP_ITEM`: a `(bag, slot)` on each side, so either end may be in an equipped bag.
    pub fn swap_item(
        &mut self,
        dst_bag: u8,
        dst_slot: u8,
        src_bag: u8,
        src_slot: u8,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_SWAP_ITEM,
            &messages::swap_item(dst_bag, dst_slot, src_bag, src_slot),
        )
    }

    /// `CMSG_AUTOSTORE_BAG_ITEM`: into `dst_bag` at a slot the server picks; the wire of
    /// `PutItemInBackpack` and of `PutItemInBag`'s auto-store.
    pub fn auto_store_bag_item(&mut self, src_bag: u8, src_slot: u8, dst_bag: u8) -> Result<()> {
        self.send(
            opcode::CMSG_AUTOSTORE_BAG_ITEM,
            &messages::auto_store_bag_item(src_bag, src_slot, dst_bag),
        )
    }

    /// `CMSG_SPLIT_ITEM`: move `count` off a stack; either end may be in an equipped bag.
    pub fn split_item(
        &mut self,
        src_bag: u8,
        src_slot: u8,
        dst_bag: u8,
        dst_slot: u8,
        count: u8,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_SPLIT_ITEM,
            &messages::split_item(src_bag, src_slot, dst_bag, dst_slot, count),
        )
    }

    /// `CMSG_DESTROYITEM`: `count` 0 destroys the whole stack; there is no reply packet.
    pub fn destroy_item(&mut self, bag: u8, slot: u8, count: u8) -> Result<()> {
        self.send(
            opcode::CMSG_DESTROYITEM,
            &messages::destroy_item(bag, slot, count),
        )
    }
}
