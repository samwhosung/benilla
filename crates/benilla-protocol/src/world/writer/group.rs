//! The group and raid sends.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Invite a player to group, answered by `SMSG_PARTY_COMMAND_RESULT`.
    pub fn group_invite(&mut self, member_name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_GROUP_INVITE,
            &messages::group_invite(member_name),
        )
    }

    /// Accept a pending group invite (`CMSG_GROUP_ACCEPT`, empty body).
    pub fn group_accept(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GROUP_ACCEPT, &messages::group_accept())
    }

    /// Decline a pending group invite; the inviter gets `SMSG_GROUP_DECLINE`.
    pub fn group_decline(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GROUP_DECLINE, &messages::group_decline())
    }

    /// Kick a group member by name (`CMSG_GROUP_UNINVITE`).
    pub fn group_uninvite(&mut self, member_name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_GROUP_UNINVITE,
            &messages::group_uninvite(member_name),
        )
    }

    /// Kick a group member by guid (`CMSG_GROUP_UNINVITE_GUID`): what `0x5e9510` sends for a
    /// player whose object the client does not hold (`0x5e953e`); a held one is kicked by name.
    pub fn group_uninvite_guid(&mut self, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_GROUP_UNINVITE_GUID,
            &messages::group_uninvite_guid(guid),
        )
    }

    /// Hand off group leadership (`CMSG_GROUP_SET_LEADER`, a full guid in 1.12).
    pub fn group_set_leader(&mut self, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_GROUP_SET_LEADER,
            &messages::group_set_leader(guid),
        )
    }

    /// Set the group's loot method (`CMSG_LOOT_METHOD`): `method` 0 free-for-all, 1 round-robin,
    /// 2 master (the only one that reads `loot_master`), 3 group, 4 need-before-greed;
    /// `threshold` is an `ItemQualities` value.
    pub fn loot_method(&mut self, method: u32, loot_master: u64, threshold: u32) -> Result<()> {
        self.send(
            opcode::CMSG_LOOT_METHOD,
            &messages::loot_method(method, loot_master, threshold),
        )
    }

    /// Disband the group (`CMSG_GROUP_DISBAND`, empty body).
    pub fn group_disband(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GROUP_DISBAND, &messages::group_disband())
    }

    /// Ask a member's live stats, answered by `SMSG_PARTY_MEMBER_STATS_FULL`.
    pub fn request_party_member_stats(&mut self, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_REQUEST_PARTY_MEMBER_STATS,
            &messages::request_party_member_stats(guid),
        )
    }

    /// Move a raid member to another subgroup (`CMSG_GROUP_CHANGE_SUB_GROUP`).
    pub fn group_change_sub_group(&mut self, name: &str, group_nr: u8) -> Result<()> {
        self.send(
            opcode::CMSG_GROUP_CHANGE_SUB_GROUP,
            &messages::group_change_sub_group(name, group_nr),
        )
    }

    /// Swap two raid members' subgroups (`CMSG_GROUP_SWAP_SUB_GROUP`).
    pub fn group_swap_sub_group(&mut self, name: &str, swap_with: &str) -> Result<()> {
        self.send(
            opcode::CMSG_GROUP_SWAP_SUB_GROUP,
            &messages::group_swap_sub_group(name, swap_with),
        )
    }

    /// Convert the party into a raid (`CMSG_GROUP_RAID_CONVERT`, empty body); leader only, one-way.
    pub fn group_raid_convert(&mut self) -> Result<()> {
        self.send(
            opcode::CMSG_GROUP_RAID_CONVERT,
            &messages::group_raid_convert(),
        )
    }

    /// Grant or revoke raid assistant (`CMSG_GROUP_ASSISTANT_LEADER`).
    pub fn group_assistant_leader(&mut self, guid: u64, grant: bool) -> Result<()> {
        self.send(
            opcode::CMSG_GROUP_ASSISTANT_LEADER,
            &messages::group_assistant_leader(guid, grant),
        )
    }

    /// Ping the minimap (`MSG_MINIMAP_PING`); the server adds our guid and relays it to the group.
    pub fn minimap_ping(&mut self, x: f32, y: f32) -> Result<()> {
        self.send(opcode::MSG_MINIMAP_PING, &messages::minimap_ping(x, y))
    }

    /// Set one raid-target icon, or clear it with `guid == 0` (`MSG_RAID_TARGET_UPDATE`).
    pub fn raid_target_set(&mut self, icon: u8, guid: u64) -> Result<()> {
        self.send(
            opcode::MSG_RAID_TARGET_UPDATE,
            &messages::raid_target_set(icon, guid),
        )
    }

    /// Ask the raid-target icons (`MSG_RAID_TARGET_UPDATE`), answered with the full list.
    pub fn raid_target_request(&mut self) -> Result<()> {
        self.send(
            opcode::MSG_RAID_TARGET_UPDATE,
            &messages::raid_target_request(),
        )
    }

    /// Start a raid ready check (`MSG_RAID_READY_CHECK`, empty body); leader only.
    pub fn ready_check_start(&mut self) -> Result<()> {
        self.send(opcode::MSG_RAID_READY_CHECK, &messages::ready_check_start())
    }

    /// Answer a raid ready check (`MSG_RAID_READY_CHECK`) with its status byte.
    pub fn ready_check_answer(&mut self, status: u8) -> Result<()> {
        self.send(
            opcode::MSG_RAID_READY_CHECK,
            &messages::ready_check_answer(status),
        )
    }

    /// Ask our saved-instance list, answered by `SMSG_RAID_INSTANCE_INFO`.
    pub fn request_raid_info(&mut self) -> Result<()> {
        self.send(
            opcode::CMSG_REQUEST_RAID_INFO,
            &messages::request_raid_info(),
        )
    }
}
