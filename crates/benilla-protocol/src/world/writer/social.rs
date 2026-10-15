//! The social sends: friends, ignores and `/who`. The wire adds by name and removes by guid, so a
//! removal must find the guid in the list first.

use anyhow::Result;

use crate::messages::{self, opcode, WhoRequest};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_FRIEND_LIST`, empty: `ShowFriends()`'s refresh; the server also pushes it at login.
    pub fn friend_list(&mut self) -> Result<()> {
        self.send(opcode::CMSG_FRIEND_LIST, &messages::friend_list())
    }

    /// `CMSG_ADD_FRIEND`: always answered by `SMSG_FRIEND_STATUS`, `FRIEND_ADDED_*` or a refusal.
    pub fn add_friend(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_ADD_FRIEND, &messages::add_friend(name))
    }

    /// `CMSG_SET_LOOKING_FOR_GROUP`: the LFG slots and comment. vmangos leaves the opcode unhandled
    /// (`Opcodes.cpp:603`), so the readback is local.
    pub fn set_looking_for_group(&mut self, slots: [u32; 3], comment: &str) -> Result<()> {
        self.send(
            opcode::CMSG_SET_LOOKING_FOR_GROUP,
            &messages::set_looking_for_group(slots, comment),
        )
    }

    /// `MSG_LOOKING_FOR_GROUP`: `LFGQuery`'s inquiry, answered with the result list.
    pub fn looking_for_group_query(&mut self, criterion: u32, class: u32) -> Result<()> {
        self.send(
            opcode::MSG_LOOKING_FOR_GROUP,
            &messages::looking_for_group_query(criterion, class),
        )
    }

    /// Drop a friend by guid (`CMSG_DEL_FRIEND`); acked with `FRIEND_REMOVED`.
    pub fn del_friend(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_DEL_FRIEND, &messages::del_friend(guid))
    }

    /// Ignore a character by name (`CMSG_ADD_IGNORE`); acked with `FRIEND_IGNORE_ADDED`.
    pub fn add_ignore(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_ADD_IGNORE, &messages::add_ignore(name))
    }

    /// Stop ignoring, by guid (`CMSG_DEL_IGNORE`); acked with `FRIEND_IGNORE_REMOVED`.
    pub fn del_ignore(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_DEL_IGNORE, &messages::del_ignore(guid))
    }

    /// `CMSG_WHO`: the server runs one query per session at a time, as an async task, so the reply
    /// lags a tick or two and a second query sent meanwhile is dropped (`ReceivedWhoRequest`).
    pub fn who(&mut self, request: &WhoRequest) -> Result<()> {
        self.send(opcode::CMSG_WHO, &messages::who(request))
    }
}
