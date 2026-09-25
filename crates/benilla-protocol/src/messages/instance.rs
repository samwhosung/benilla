//! The instance lockout messages and `CMSG_RESET_INSTANCES`. The 1.12 client prints a system chat
//! line for a new save (`INSTANCE_SAVED`), a raid warning, a reset (`INSTANCE_RESET_SUCCESS`,
//! which also clears the last instance) and a failed reset, and keeps the last instance and the
//! permanent-bind flag as state. Maps travel as `Map.dbc` ids; the reference looks the name up
//! itself (`0xc0daa8`) and prints the id when the row is missing.

use std::io;

use crate::wire::read_u32_le;

/// `SMSG_RAID_INSTANCE_MESSAGE`'s `type` (`Objects/Player.h:568-573`). Outside 1-4 the reference
/// prints nothing (jump table `0x49e45c`), even for 5, later clients' `RAID_INSTANCE_EXPIRED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidInstanceWarning {
    /// 1: `RAID_INSTANCE_WARNING_HOURS`, filled with `resetTime / 3600`.
    Hours,
    /// 2: `RAID_INSTANCE_WARNING_MIN`, filled with `resetTime / 60`.
    Minutes,
    /// 3: `RAID_INSTANCE_WARNING_MIN_SOON`, filled with `resetTime / 60`.
    MinutesSoon,
    /// 4: `RAID_INSTANCE_WELCOME`, filled with the `d`/`h`/`m` breakdown of `resetTime`.
    Welcome,
}

impl RaidInstanceWarning {
    pub fn from_wire(ty: u32) -> Option<Self> {
        match ty {
            1 => Some(Self::Hours),
            2 => Some(Self::Minutes),
            3 => Some(Self::MinutesSoon),
            4 => Some(Self::Welcome),
            _ => None,
        }
    }

    /// The reference's GlobalStrings token (`0x49e276`, `0x49e2e5`, `0x49e354`, `0x49e40b`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Hours => "RAID_INSTANCE_WARNING_HOURS",
            Self::Minutes => "RAID_INSTANCE_WARNING_MIN",
            Self::MinutesSoon => "RAID_INSTANCE_WARNING_MIN_SOON",
            Self::Welcome => "RAID_INSTANCE_WELCOME",
        }
    }
}

/// `SMSG_RAID_INSTANCE_MESSAGE`: a raid lockout's welcome or countdown line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaidInstanceMessage {
    /// The raw wire type; [`RaidInstanceWarning::from_wire`] resolves it.
    pub message_type: u32,
    pub map: u32,
    /// Seconds until the reset, a duration rather than a timestamp (`SendInstanceResetWarning`).
    pub reset: u32,
}

/// `SMSG_INSTANCE_RESET_FAILED`'s `reason` (ladder at `0x49e5b7`; vmangos
/// `Maps/MapPersistentStateMgr.h:270-276`, whose `INSTANCERESET_FAIL_SILENTLY` is 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceResetFailure {
    /// 0: `INSTANCE_RESET_FAILED`, players still inside.
    General,
    /// 1: `INSTANCE_RESET_FAILED_OFFLINE`, someone in the party is offline.
    Offline,
    /// 2: `INSTANCE_RESET_FAILED_ZONING`, someone in the party is zoning in.
    Zoning,
}

impl InstanceResetFailure {
    /// Deviation: a reason of 3 or more is silent, where the reference prints an uninitialized
    /// stack buffer, because 3 is the server's `INSTANCERESET_FAIL_SILENTLY`.
    pub fn from_wire(reason: u32) -> Option<Self> {
        match reason {
            0 => Some(Self::General),
            1 => Some(Self::Offline),
            2 => Some(Self::Zoning),
            _ => None,
        }
    }

    /// The reference's GlobalStrings token (`0x49e61b`, `0x49e5f6`, `0x49e5d1`).
    pub fn token(self) -> &'static str {
        match self {
            Self::General => "INSTANCE_RESET_FAILED",
            Self::Offline => "INSTANCE_RESET_FAILED_OFFLINE",
            Self::Zoning => "INSTANCE_RESET_FAILED_ZONING",
        }
    }
}

/// `SMSG_INSTANCE_RESET_FAILED`: the group leader's reset was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceResetFailed {
    /// The raw wire reason; [`InstanceResetFailure::from_wire`] resolves it.
    pub reason: u32,
    pub map: u32,
}

/// Read `SMSG_RAID_INSTANCE_MESSAGE`: type, map, seconds to reset (reference reads `0x49e1cd`,
/// `0x49e1d8`, `0x49e1e3`).
pub(super) fn read_raid_instance_message(r: &mut &[u8]) -> io::Result<RaidInstanceMessage> {
    Ok(RaidInstanceMessage {
        message_type: read_u32_le(r)?,
        map: read_u32_le(r)?,
        reset: read_u32_le(r)?,
    })
}

/// Read `SMSG_INSTANCE_RESET_FAILED`: `u32 reason`, `u32 mapId` (`0x49e54d`/`0x49e558`).
pub(super) fn read_instance_reset_failed(r: &mut &[u8]) -> io::Result<InstanceResetFailed> {
    Ok(InstanceResetFailed {
        reason: read_u32_le(r)?,
        map: read_u32_le(r)?,
    })
}

/// Read a one-`u32` body: `SMSG_INSTANCE_RESET`'s map (`0x49e481`), `SMSG_UPDATE_LAST_INSTANCE`'s
/// map (`0x49e676`), `SMSG_UPDATE_INSTANCE_OWNERSHIP`'s permanent-bind flag (`0x49e6c6`) and
/// `SMSG_INSTANCE_SAVE_CREATED`'s flag (`0x4e7e6c`).
pub(super) fn read_u32_body(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// Body of `CMSG_RESET_INSTANCES`: empty (reference `ResetInstances`, `0x48a6b0`).
pub fn reset_instances() -> Vec<u8> {
    Vec::new()
}

/// `SMSG_RAID_GROUP_ONLY` (reference `0x5e48f1`): a nonzero `delay_ms` arms the instance-boot
/// deadline (`INSTANCE_BOOT_START`); `0` clears it (`INSTANCE_BOOT_STOP`), and then `reason` 1 or
/// 2 shows `ERR_RAID_GROUP_ONLY` or `ERR_RAID_GROUP_FULL`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RaidGroupOnly {
    pub delay_ms: u32,
    pub reason: u32,
}

/// Parse `SMSG_RAID_GROUP_ONLY`: `u32 delayMs`, `u32 reason`.
pub(super) fn read_raid_group_only(r: &mut impl std::io::Read) -> std::io::Result<RaidGroupOnly> {
    Ok(RaidGroupOnly {
        delay_ms: crate::wire::read_u32_le(r)?,
        reason: crate::wire::read_u32_le(r)?,
    })
}
