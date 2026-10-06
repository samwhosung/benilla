//! The meeting-stone queue's sends.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_MEETINGSTONE_JOIN`: a meeting stone's right-click, after the client's own checks.
    /// Never `CMSG_GAMEOBJ_USE`: the server has no case for type 23 there.
    pub fn meeting_stone_join(&mut self, go_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_MEETINGSTONE_JOIN,
            &messages::meeting_stone_join(go_guid),
        )
    }

    /// `CMSG_MEETINGSTONE_LEAVE`, empty: `CancelMeetingStoneRequest()`, from the party leader or a
    /// player in no party.
    pub fn meeting_stone_leave(&mut self) -> Result<()> {
        self.send(
            opcode::CMSG_MEETINGSTONE_LEAVE,
            &messages::meeting_stone_leave(),
        )
    }

    /// `CMSG_MEETINGSTONE_STATUS_QUERY`, empty: the reference sends it at each world-enter
    /// cascade, so at login, after every cross-map worldport and on every `/reload`.
    pub fn meeting_stone_status_query(&mut self) -> Result<()> {
        self.send(opcode::CMSG_MEETINGSTONE_STATUS_QUERY, &[])
    }
}
