//! The player trade sends.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_INITIATE_TRADE`: the target gets `BEGIN_TRADE`; we hear back only on a refusal.
    pub fn initiate_trade(&mut self, target: u64) -> Result<()> {
        self.send(
            opcode::CMSG_INITIATE_TRADE,
            &messages::initiate_trade(target),
        )
    }

    /// `CMSG_BEGIN_TRADE`, empty: the automatic reply to `BEGIN_TRADE`, after which the server
    /// sends both sides `OPEN_WINDOW`.
    pub fn begin_trade(&mut self) -> Result<()> {
        self.send(opcode::CMSG_BEGIN_TRADE, &[])
    }

    /// `CMSG_BUSY_TRADE`, empty: the initiator gets `TRADE_STATUS_BUSY`.
    pub fn busy_trade(&mut self) -> Result<()> {
        self.send(opcode::CMSG_BUSY_TRADE, &[])
    }

    /// `CMSG_IGNORE_TRADE`, empty: the initiator gets `TRADE_STATUS_IGNORE_YOU`.
    pub fn ignore_trade(&mut self) -> Result<()> {
        self.send(opcode::CMSG_IGNORE_TRADE, &[])
    }

    /// `CMSG_SET_TRADE_ITEM`: clears the partner's accept and re-arms the server's 200 ms delay.
    pub fn set_trade_item(&mut self, trade_slot: u8, bag: u8, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_SET_TRADE_ITEM,
            &messages::set_trade_item(trade_slot, bag, slot),
        )
    }

    /// `CMSG_CLEAR_TRADE_ITEM`: empty one trade slot.
    pub fn clear_trade_item(&mut self, trade_slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_CLEAR_TRADE_ITEM,
            &messages::clear_trade_item(trade_slot),
        )
    }

    /// `CMSG_SET_TRADE_GOLD`, in copper: clears the partner's accept and re-arms the 200 ms delay.
    pub fn set_trade_gold(&mut self, copper: u32) -> Result<()> {
        self.send(
            opcode::CMSG_SET_TRADE_GOLD,
            &messages::set_trade_gold(copper),
        )
    }

    /// `CMSG_ACCEPT_TRADE` with the partner's last `flag`: bounced (`TRADE_STATUS_BACK_TO_TRADE`)
    /// within 200 ms of a change; once both sides accept, the server swaps and sends both
    /// `COMPLETE`.
    pub fn accept_trade(&mut self, flag: u32) -> Result<()> {
        self.send(opcode::CMSG_ACCEPT_TRADE, &messages::accept_trade(flag))
    }

    /// `CMSG_UNACCEPT_TRADE`, empty: withdraws our accept.
    pub fn unaccept_trade(&mut self) -> Result<()> {
        self.send(opcode::CMSG_UNACCEPT_TRADE, &[])
    }

    /// `CMSG_CANCEL_TRADE`, empty: both sides get `TRADE_STATUS_TRADE_CANCELED`.
    pub fn cancel_trade(&mut self) -> Result<()> {
        self.send(opcode::CMSG_CANCEL_TRADE, &[])
    }
}
