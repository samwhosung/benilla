//! Bank messages: open the window, buy a bag slot, move items in and out. The vault itself
//! streams in the player descriptor (bank slots 39-62, bank bags 63-68).

use std::io;

use crate::wire::{read_u32_le, read_u64_le};

/// `SMSG_BUY_BANK_SLOT_RESULT`'s reason (`BANK_SLOT_*`, vmangos `Player.h:91-94`).
pub mod bank_slot_result {
    pub const FAILED_TOO_MANY: u32 = 0;
    pub const INSUFFICIENT_FUNDS: u32 = 1;
    pub const NOTBANKER: u32 = 2;
    pub const OK: u32 = 3;
}

/// `CMSG_BANKER_ACTIVATE` (vmangos `Npc.h:58-66`), for a pure banker (`UNIT_NPC_FLAGS` bit 8
/// `0x100` set, bits 0-7 clear); a gossip banker opens its bank through the gossip option.
pub fn banker_activate(banker_guid: u64) -> Vec<u8> {
    banker_guid.to_le_bytes().to_vec()
}

/// `CMSG_BUY_BANK_SLOT` (vmangos `Item.h:157-163`): no slot index, the server buys the next one;
/// its price is in `BankBagSlotPrices.dbc`.
pub fn buy_bank_slot(banker_guid: u64) -> Vec<u8> {
    banker_guid.to_le_bytes().to_vec()
}

/// `CMSG_AUTOBANK_ITEM` (`ItemHandler.cpp:938`): deposit only, into the first free bank slot.
pub fn autobank_item(bag: u8, slot: u8) -> Vec<u8> {
    vec![bag, slot]
}

/// `CMSG_AUTOSTORE_BANK_ITEM` (`ItemHandler.cpp:971`): withdraws from a bank position, deposits
/// from anywhere else; the reference's right-click auto-move uses it both ways.
pub fn autostore_bank_item(bag: u8, slot: u8) -> Vec<u8> {
    vec![bag, slot]
}

/// `SMSG_SHOW_BANK` (vmangos `Npc.cpp:94`): the banker guid. Also sent unprompted for the gossip
/// bank option (`Player.cpp:12426`), not only after [`banker_activate`].
pub(super) fn read_show_bank(r: &mut &[u8]) -> io::Result<u64> {
    read_u64_le(r)
}

/// `SMSG_BUY_BANK_SLOT_RESULT` (vmangos `Item.cpp:137-140`), sent only on failure: a bought slot
/// shows only as the `PLAYER_BYTES_2` bank-bag count rising and the coinage falling.
pub(super) fn read_buy_bank_slot_result(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}
