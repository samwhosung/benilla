//! Group messages: invites, leadership, loot method, roster, member stats, minimap pings,
//! subgroups, raid target icons, ready checks and lockouts (vmangos `Server/Packets/Group.cpp`).
//! Every guid here is a full `u64` except the party member stats subject, which is packed.

use std::io::{self, Read};

use crate::messages::update_object::power_display_scale;
use crate::wire::{
    capacity_hint, read_cstring, read_f32_le, read_packed_guid, read_u16_le, read_u32_le,
    read_u64_le, read_u8,
};

/// The raid-assistant bit of a member's flags byte; bits 0-2 are the subgroup
/// (`Group/Group.cpp:1382,1394`).
pub const GROUP_MEMBER_ASSISTANT: u8 = 0x80;

/// vmangos `GroupMemberStatus` (`Group/Group.h:102-111`); bit `0x20` is never set.
pub mod member_status {
    pub const OFFLINE: u8 = 0x00;
    pub const ONLINE: u8 = 0x01;
    pub const PVP: u8 = 0x02;
    pub const DEAD: u8 = 0x04;
    pub const GHOST: u8 = 0x08;
    pub const PVP_FFA: u8 = 0x10;
    pub const AFK: u8 = 0x40;
    pub const DND: u8 = 0x80;
}

/// vmangos `PartyOperation` (`WorldSession.h:94-98`): which command a party result answers.
pub mod party_operation {
    pub const INVITE: u32 = 0;
    pub const LEAVE: u32 = 2;
}

/// vmangos `PartyResult` (`WorldSession.h:100-111`), the verdict of `SMSG_PARTY_COMMAND_RESULT`.
pub mod party_result {
    pub const OK: u32 = 0;
    pub const BAD_PLAYER_NAME: u32 = 1;
    pub const TARGET_NOT_IN_GROUP: u32 = 2;
    pub const GROUP_FULL: u32 = 3;
    pub const ALREADY_IN_GROUP: u32 = 4;
    pub const NOT_IN_GROUP: u32 = 5;
    pub const NOT_LEADER: u32 = 6;
    pub const WRONG_FACTION: u32 = 7;
    pub const IGNORING_YOU: u32 = 8;
}

/// One `SMSG_GROUP_LIST` member row (`Group.cpp:161-167`); the recipient has no row, their flags
/// ride the list's own-flags byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMemberEntry {
    pub name: String,
    pub guid: u64,
    /// [`member_status`] bits (`GetGroupMemberStatus`, `Group/Group.cpp:45-63`).
    pub status: u8,
    /// Subgroup in bits 0-2, plus [`GROUP_MEMBER_ASSISTANT`].
    pub flags: u8,
}

/// The loot tail `SMSG_GROUP_LIST` carries only when it lists members (`Group.cpp:170-179`);
/// `threshold` is an item quality, 2 to 4 in practice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupLootInfo {
    /// 0 free-for-all, 1 round-robin, 2 master, 3 group, 4 need before greed (`LootMethod`).
    pub method: u8,
    /// The master looter, 0 unless `method` is 2.
    pub master: u64,
    pub threshold: u8,
}

/// Read `SMSG_GROUP_LIST` (`Group.cpp:155-180`): type (0 party, 1 raid), own flags, members,
/// leader, then the loot tail only with members. "You left the group" is the 14-byte empty list.
#[allow(clippy::type_complexity)]
pub(super) fn read_group_list(
    r: &mut &[u8],
) -> io::Result<(u8, u8, Vec<GroupMemberEntry>, u64, Option<GroupLootInfo>)> {
    let group_type = read_u8(r)?;
    let own_flags = read_u8(r)?;
    let count = read_u32_le(r)?;
    // A raid is the widest list this carries: vmangos `MAX_RAID_SIZE` 40 (`Group/Group.h:50`).
    let mut members = Vec::with_capacity(capacity_hint(count, 40));
    for _ in 0..count {
        members.push(GroupMemberEntry {
            name: read_cstring(r)?,
            guid: read_u64_le(r)?,
            status: read_u8(r)?,
            flags: read_u8(r)?,
        });
    }
    let leader = read_u64_le(r)?;
    let loot = if count > 0 {
        let method = read_u8(r)?;
        let master = read_u64_le(r)?;
        let threshold = read_u8(r)?;
        let _dungeon_difficulty = read_u8(r)?; // always 0 at 5875 (Group.h:281)
        Some(GroupLootInfo {
            method,
            master,
            threshold,
        })
    } else {
        None
    };
    Ok((group_type, own_flags, members, leader, loot))
}

/// Read `SMSG_GROUP_INVITE`: the inviter's name (`Group.cpp:107-110`).
pub(super) fn read_group_invite(r: &mut impl Read) -> io::Result<String> {
    read_cstring(r)
}

/// Read `SMSG_GROUP_DECLINE`: the decliner's name (`Group.cpp:112-115`).
pub(super) fn read_group_decline(r: &mut impl Read) -> io::Result<String> {
    read_cstring(r)
}

/// Read `SMSG_GROUP_SET_LEADER`: the new leader's name (`Group.cpp:150-153`).
pub(super) fn read_group_set_leader(r: &mut impl Read) -> io::Result<String> {
    read_cstring(r)
}

/// Read `SMSG_PARTY_COMMAND_RESULT` (`GroupHandler.cpp:47-54`): operation, member name, result;
/// the name can be empty, as in the raid-convert confirmation (`GroupHandler.cpp:466`).
pub(super) fn read_party_command_result(r: &mut impl Read) -> io::Result<(u32, String, u32)> {
    Ok((read_u32_le(r)?, read_cstring(r)?, read_u32_le(r)?))
}

/// vmangos `GROUP_UPDATE_FLAG_*` (`Group.h:124-151`): the member stats that follow, in bit order.
pub mod party_member_mask {
    pub const STATUS: u32 = 0x0000_0001;
    pub const CUR_HP: u32 = 0x0000_0002;
    pub const MAX_HP: u32 = 0x0000_0004;
    pub const POWER_TYPE: u32 = 0x0000_0008;
    pub const CUR_POWER: u32 = 0x0000_0010;
    pub const MAX_POWER: u32 = 0x0000_0020;
    pub const LEVEL: u32 = 0x0000_0040;
    pub const ZONE: u32 = 0x0000_0080;
    pub const POSITION: u32 = 0x0000_0100;
    pub const AURAS: u32 = 0x0000_0200;
    pub const AURAS_NEGATIVE: u32 = 0x0000_0400;
    pub const PET_GUID: u32 = 0x0000_0800;
    pub const PET_NAME: u32 = 0x0000_1000;
    pub const PET_MODEL_ID: u32 = 0x0000_2000;
    pub const PET_CUR_HP: u32 = 0x0000_4000;
    pub const PET_MAX_HP: u32 = 0x0000_8000;
    pub const PET_POWER_TYPE: u32 = 0x0001_0000;
    pub const PET_CUR_POWER: u32 = 0x0002_0000;
    pub const PET_MAX_POWER: u32 = 0x0004_0000;
    pub const PET_AURAS: u32 = 0x0008_0000;
    pub const PET_AURAS_NEGATIVE: u32 = 0x0010_0000;
}

/// One `SMSG_PARTY_MEMBER_STATS` or `_FULL` payload (`GroupHandler.cpp:590-742`): only the masked
/// fields are present. The plain opcode carries what changed; `_FULL` everything the server has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartyMemberStatsInfo {
    pub status: Option<u8>,
    pub cur_hp: Option<u16>,
    pub max_hp: Option<u16>,
    pub power_type: Option<u8>,
    pub cur_power: Option<u16>,
    pub max_power: Option<u16>,
    pub level: Option<u16>,
    pub zone: Option<u16>,
    /// Raw WoW x and y, truncated to `i16` by the server (`GroupHandler.cpp:625`).
    pub position: Option<(i16, i16)>,
    /// Active buff spell ids in slot order; the wire's `u32` slot mask is not kept.
    pub auras: Option<Vec<u16>>,
    /// Active debuff spell ids, likewise, from a `u16` slot mask.
    pub auras_negative: Option<Vec<u16>>,
    pub pet_guid: Option<u64>,
    pub pet_name: Option<String>,
    pub pet_model_id: Option<u16>,
    pub pet_cur_hp: Option<u16>,
    pub pet_max_hp: Option<u16>,
    pub pet_power_type: Option<u8>,
    pub pet_cur_power: Option<u16>,
    pub pet_max_power: Option<u16>,
    pub pet_auras: Option<Vec<u16>>,
    pub pet_auras_negative: Option<Vec<u16>>,
}

impl PartyMemberStatsInfo {
    /// The record of a member new to the roster: 1/1 health and power, so an unseen member shows
    /// full bars until stats arrive (`0x4e833d`), and the roster's online bit.
    pub fn placeholder(online: bool) -> Self {
        Self {
            status: Some(u8::from(online)),
            cur_hp: Some(1),
            max_hp: Some(1),
            cur_power: Some(1),
            max_power: Some(1),
            ..Self::default()
        }
    }

    /// Snapshot a member's live descriptor as the 1.12 client does (`0x5f0880`) when the member's
    /// object leaves view, just before the stats request, so the frame never reads 0/0. Values are
    /// raw; [`Self::shown_power`] divides at the read. AFK and DND are cleared, as in the client.
    ///
    /// Deviation: zone and position are kept, not set to the viewer's zone and the object's
    /// position, and the aura and pet blocks are not copied, because nothing in benilla reads them.
    pub fn snapshot_descriptor(&mut self, fields: &crate::messages::update_object::ObjectFields) {
        let mut status = member_status::ONLINE;
        if fields.player_is_ghost() {
            status |= member_status::GHOST;
        }
        // `UNIT_FIELD_FLAGS` PvP bit.
        if fields.unit_flags() & 0x1000 != 0 {
            status |= member_status::PVP;
        }
        // `PLAYER_FLAGS` bit 7 (`0x5f08be`).
        if fields.player_flags() & 0x80 != 0 {
            status |= member_status::PVP_FFA;
        }
        // Raw health, not `unit_reads_dead`: a feigning member reads alive (`0x5f08d3`).
        if fields.unit_health().unwrap_or(0) == 0 {
            status |= member_status::DEAD;
        }
        self.status = Some(status);
        // The record holds `u16`s; the 1.12 client keeps each dword's low word (`0x5f08ea`).
        let power_type = fields.unit_power_type();
        self.cur_hp = Some(fields.unit_health().unwrap_or(0) as u16);
        self.max_hp = Some(fields.unit_max_health().unwrap_or(0) as u16);
        self.power_type = Some(power_type);
        self.cur_power = Some(fields.unit_power(power_type).unwrap_or(0) as u16);
        self.max_power = Some(fields.unit_max_power(power_type).unwrap_or(0) as u16);
        self.level = Some(fields.unit_level().unwrap_or(0) as u16);
    }

    /// `UnitPowerType` from the record, 0 when it has none (the binding's miss value).
    pub fn shown_power_type(&self) -> u8 {
        self.power_type.unwrap_or(0)
    }

    /// `UnitMana` from the record (`0x517744`): the stored power divided by
    /// [`power_display_scale`], as for a live unit.
    pub fn shown_power(&self) -> u32 {
        u32::from(self.cur_power.unwrap_or(0))
            / power_display_scale(u32::from(self.shown_power_type()))
    }

    /// `UnitManaMax` from the record (`0x5178af`), the same divide.
    pub fn shown_max_power(&self) -> u32 {
        u32::from(self.max_power.unwrap_or(0))
            / power_display_scale(u32::from(self.shown_power_type()))
    }
}

/// One `u16` spell id per set bit of `mask`, in bit order (`GroupHandler.cpp:627-648`).
fn read_aura_spells(r: &mut impl Read, mask: u32, bits: u32) -> io::Result<Vec<u16>> {
    let mut spells = Vec::new();
    for bit in 0..bits {
        if mask & (1 << bit) != 0 {
            spells.push(read_u16_le(r)?);
        }
    }
    Ok(spells)
}

/// Read `SMSG_PARTY_MEMBER_STATS` or `_FULL`; the guid is packed (`GroupHandler.cpp:593`).
pub(super) fn read_party_member_stats(
    r: &mut impl Read,
) -> io::Result<(u64, PartyMemberStatsInfo)> {
    let guid = read_packed_guid(r)?;
    let mask = read_u32_le(r)?;
    let mut info = PartyMemberStatsInfo::default();

    if mask & party_member_mask::STATUS != 0 {
        info.status = Some(read_u8(r)?);
    }
    if mask & party_member_mask::CUR_HP != 0 {
        info.cur_hp = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::MAX_HP != 0 {
        info.max_hp = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::POWER_TYPE != 0 {
        info.power_type = Some(read_u8(r)?);
    }
    if mask & party_member_mask::CUR_POWER != 0 {
        info.cur_power = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::MAX_POWER != 0 {
        info.max_power = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::LEVEL != 0 {
        info.level = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::ZONE != 0 {
        info.zone = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::POSITION != 0 {
        let x = read_u16_le(r)? as i16;
        let y = read_u16_le(r)? as i16;
        info.position = Some((x, y));
    }
    if mask & party_member_mask::AURAS != 0 {
        let pos_mask = read_u32_le(r)?;
        info.auras = Some(read_aura_spells(r, pos_mask, 32)?);
    }
    if mask & party_member_mask::AURAS_NEGATIVE != 0 {
        let neg_mask = u32::from(read_u16_le(r)?);
        info.auras_negative = Some(read_aura_spells(r, neg_mask, 16)?);
    }
    if mask & party_member_mask::PET_GUID != 0 {
        info.pet_guid = Some(read_u64_le(r)?);
    }
    if mask & party_member_mask::PET_NAME != 0 {
        info.pet_name = Some(read_cstring(r)?);
    }
    if mask & party_member_mask::PET_MODEL_ID != 0 {
        info.pet_model_id = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::PET_CUR_HP != 0 {
        info.pet_cur_hp = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::PET_MAX_HP != 0 {
        info.pet_max_hp = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::PET_POWER_TYPE != 0 {
        info.pet_power_type = Some(read_u8(r)?);
    }
    if mask & party_member_mask::PET_CUR_POWER != 0 {
        info.pet_cur_power = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::PET_MAX_POWER != 0 {
        info.pet_max_power = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::PET_AURAS != 0 {
        let pos_mask = read_u32_le(r)?;
        info.pet_auras = Some(read_aura_spells(r, pos_mask, 32)?);
    }
    if mask & party_member_mask::PET_AURAS_NEGATIVE != 0 {
        let neg_mask = u32::from(read_u16_le(r)?);
        info.pet_auras_negative = Some(read_aura_spells(r, neg_mask, 16)?);
    }
    Ok((guid, info))
}

/// Read the relayed `MSG_MINIMAP_PING`: the pinger's guid, which the server adds, then x and y
/// (`GroupHandler.cpp:382-391`).
pub(super) fn read_minimap_ping(r: &mut impl Read) -> io::Result<(u64, f32, f32)> {
    Ok((read_u64_le(r)?, read_f32_le(r)?, read_f32_le(r)?))
}

/// A server `MSG_RAID_TARGET_UPDATE` (`Group.cpp:132-147`): mode 0 is one changed icon, mode 1
/// every set icon, an empty list when none is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RaidTargetUpdate {
    Delta { icon: u8, guid: u64 },
    List(Vec<(u8, u64)>),
}

pub(super) fn read_raid_target_update(r: &mut &[u8]) -> io::Result<RaidTargetUpdate> {
    let mode = read_u8(r)?;
    if mode == 0 {
        Ok(RaidTargetUpdate::Delta {
            icon: read_u8(r)?,
            guid: read_u64_le(r)?,
        })
    } else {
        let mut entries = Vec::new();
        while !r.is_empty() {
            entries.push((read_u8(r)?, read_u64_le(r)?));
        }
        Ok(RaidTargetUpdate::List(entries))
    }
}

/// A server `MSG_RAID_READY_CHECK`: empty when the leader starts a check, else one member's answer,
/// sent to the leader only (`Group.cpp:94-96`, `126-130`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyCheck {
    Started,
    Answer { guid: u64, ready: u8 },
}

pub(super) fn read_ready_check(r: &mut &[u8]) -> io::Result<ReadyCheck> {
    if r.is_empty() {
        return Ok(ReadyCheck::Started);
    }
    Ok(ReadyCheck::Answer {
        guid: read_u64_le(r)?,
        ready: read_u8(r)?,
    })
}

/// `CMSG_GROUP_INVITE` body: the invitee's name (`Group.cpp:4-7`).
pub fn group_invite(member_name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(member_name.len() + 1);
    body.extend_from_slice(member_name.as_bytes());
    body.push(0);
    body
}

/// `CMSG_GROUP_ACCEPT` body: empty (vmangos `NullClientPacket`).
pub fn group_accept() -> Vec<u8> {
    Vec::new()
}

/// `CMSG_GROUP_DECLINE` body: empty; the inviter gets `SMSG_GROUP_DECLINE`.
pub fn group_decline() -> Vec<u8> {
    Vec::new()
}

/// `CMSG_GROUP_UNINVITE` body: the kicked player's name (`Group.cpp:9-12`).
pub fn group_uninvite(member_name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(member_name.len() + 1);
    body.extend_from_slice(member_name.as_bytes());
    body.push(0);
    body
}

/// `CMSG_GROUP_UNINVITE_GUID` body, the raid frame's kick: a full guid (`Group.cpp:14-17`).
pub fn group_uninvite_guid(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `CMSG_GROUP_SET_LEADER` body: a full guid (`Group.cpp:57-64`, vmangos's post-1.11.2 branch).
pub fn group_set_leader(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `CMSG_LOOT_METHOD` body (`Group.cpp:26-31`): `u32` method, the master looter's full guid
/// (ignored unless the method is 2), `u32` quality threshold.
pub fn loot_method(method: u32, loot_master: u64, threshold: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&method.to_le_bytes());
    body.extend_from_slice(&loot_master.to_le_bytes());
    body.extend_from_slice(&threshold.to_le_bytes());
    body
}

/// `CMSG_GROUP_DISBAND` body: empty.
pub fn group_disband() -> Vec<u8> {
    Vec::new()
}

/// `CMSG_GROUP_RAID_CONVERT` body: empty; turns the sender's party into a raid.
pub fn group_raid_convert() -> Vec<u8> {
    Vec::new()
}

/// `CMSG_REQUEST_PARTY_MEMBER_STATS` body: the member's full guid (`Group.cpp:20-23`); answered
/// by `SMSG_PARTY_MEMBER_STATS_FULL`.
pub fn request_party_member_stats(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `CMSG_GROUP_CHANGE_SUB_GROUP` body: a member's name, then the subgroup (`Group.cpp:45-49`).
pub fn group_change_sub_group(name: &str, group_nr: u8) -> Vec<u8> {
    let mut body = Vec::with_capacity(name.len() + 2);
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body.push(group_nr);
    body
}

/// `CMSG_GROUP_SWAP_SUB_GROUP` body: the two members' names, mover first (`Group.cpp:51-55`).
pub fn group_swap_sub_group(name: &str, swap_with: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(name.len() + swap_with.len() + 2);
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body.extend_from_slice(swap_with.as_bytes());
    body.push(0);
    body
}

/// `CMSG_GROUP_ASSISTANT_LEADER` body: a full guid, then 1 to grant raid assistant or 0 to revoke
/// it (`Group.cpp:66-74`).
pub fn group_assistant_leader(guid: u64, grant: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.extend_from_slice(&guid.to_le_bytes());
    body.push(u8::from(grant));
    body
}

/// Our `MSG_MINIMAP_PING` body: x and y, no guid (`Group.cpp:33-37`).
pub fn minimap_ping(x: f32, y: f32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&x.to_le_bytes());
    body.extend_from_slice(&y.to_le_bytes());
    body
}

/// `MSG_RAID_TARGET_UPDATE` body: icon 0 to 7, then a full guid, 0 to clear; the client sends no
/// mode byte (`Group.cpp:77-82`).
pub fn raid_target_set(icon: u8, guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.push(icon);
    body.extend_from_slice(&guid.to_le_bytes());
    body
}

/// `MSG_RAID_TARGET_UPDATE` body asking for every icon: `0xFF` alone, no guid (`Group.cpp:80-81`).
pub fn raid_target_request() -> Vec<u8> {
    vec![0xFF]
}

/// `MSG_RAID_READY_CHECK` body starting a check: empty (`Group.cpp:84-92`).
pub fn ready_check_start() -> Vec<u8> {
    Vec::new()
}

/// `MSG_RAID_READY_CHECK` body answering one: 1 ready or 0 not, no guid (`Group.cpp:84-92`).
pub fn ready_check_answer(ready: bool) -> Vec<u8> {
    vec![u8::from(ready)]
}

/// One `SMSG_RAID_INSTANCE_INFO` row, a permanent raid lockout (vmangos `Player::SendRaidInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaidInstanceEntry {
    /// `Map.dbc` id; the name shown is the client's own lookup.
    pub map: u32,
    /// Seconds until the lockout resets.
    pub reset: u32,
    /// The instance id, `GetSavedInstanceInfo`'s second return.
    pub instance: u32,
}

/// Read `SMSG_RAID_INSTANCE_INFO`; the count is authoritative, so a short body is an error.
pub(super) fn read_raid_instance_info(r: &mut &[u8]) -> io::Result<Vec<RaidInstanceEntry>> {
    let count = read_u32_le(r)?;
    // 1024 caps the preallocation far above any real lockout list.
    let mut out = Vec::with_capacity(capacity_hint(count, 1024));
    for _ in 0..count {
        out.push(RaidInstanceEntry {
            map: read_u32_le(r)?,
            reset: read_u32_le(r)?,
            instance: read_u32_le(r)?,
        });
    }
    Ok(out)
}

/// `CMSG_REQUEST_RAID_INFO` body: empty (vmangos `HandleRequestRaidInfoOpcode`).
pub fn request_raid_info() -> Vec<u8> {
    Vec::new()
}
