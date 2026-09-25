//! The `MovementInfo` body every `MSG_MOVE_*` carries, its flag bits, and the movement verbs.

use std::io::{self, Read};

use crate::wire::{read_f32_le, read_u32_le, read_u64_le, Vector3d};

// MovementFlags bits (vmangos `MovementInfo.h`). ON_TRANSPORT is 1.12's 0x0200_0000
// (`MovementInfo.h:56`), not TBC's 0x200, which 1.12 names `MOVEFLAG_UNUSED10`.
pub(super) const MOVEMENT_FLAG_ON_TRANSPORT: u32 = 0x0200_0000;
pub(super) const MOVEMENT_FLAG_JUMPING: u32 = 0x2000;
pub(super) const MOVEMENT_FLAG_SWIMMING: u32 = 0x20_0000;
pub(super) const MOVEMENT_FLAG_SPLINE_ENABLED: u32 = 0x40_0000;
pub(super) const MOVEMENT_FLAG_SPLINE_ELEVATION: u32 = 0x400_0000;

/// `MSG_MOVE_TIME_SKIPPED` inbound: a packed guid and the ms that mover's client skipped
/// (reference handler `0x603b40`; vmangos `MovementHandler.cpp:1005-1011`). Our own send uses a
/// plain 8-byte guid ([`super::client::move_time_skipped`]); the asymmetry is the reference's.
pub(super) fn read_move_time_skipped(r: &mut &[u8]) -> io::Result<(u64, u32)> {
    let guid = crate::wire::read_packed_guid(r)?;
    let lag_ms = read_u32_le(r)?;
    Ok((guid, lag_ms))
}

/// vmangos `MovementInfo::Read` order: flags, time, position, facing, the transport pose if
/// `ON_TRANSPORT` (1.12 has no transport time), the swim pitch if `SWIMMING`, `u32` fall time, the
/// jump tail if `JUMPING`, then a spline-elevation `f32` if `SPLINE_ELEVATION`, read and dropped.
pub(super) fn read_movement_info(r: &mut impl Read) -> io::Result<MovementInfo> {
    let flags = read_u32_le(r)?;
    let timestamp = read_u32_le(r)?;
    let position = Vector3d::read(r)?;
    let orientation = read_f32_le(r)?;
    let transport = if flags & MOVEMENT_FLAG_ON_TRANSPORT != 0 {
        Some(TransportPose {
            // A full u64, not packed (vmangos `ObjectGuid.cpp:180`, `buf.read<uint64>()`).
            guid: read_u64_le(r)?,
            pos: Vector3d::read(r)?,
            orientation: read_f32_le(r)?,
        })
    } else {
        None
    };
    let pitch = if flags & MOVEMENT_FLAG_SWIMMING != 0 {
        read_f32_le(r)?
    } else {
        0.0
    };
    let fall_time = read_u32_le(r)?;
    let jump = if flags & MOVEMENT_FLAG_JUMPING != 0 {
        Some(JumpInfo {
            zspeed: read_f32_le(r)?,
            cos_angle: read_f32_le(r)?,
            sin_angle: read_f32_le(r)?,
            xy_speed: read_f32_le(r)?,
        })
    } else {
        None
    };
    if flags & MOVEMENT_FLAG_SPLINE_ELEVATION != 0 {
        let _ = read_f32_le(r)?;
    }
    Ok(MovementInfo {
        flags,
        timestamp,
        position,
        orientation,
        transport,
        pitch,
        fall_time,
        jump,
    })
}

// --- movement vocabulary ---

/// A mover speed, each with its own force-change opcode pair; the order is the `LIVING` block's
/// speed array and vmangos `UnitMoveType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedKind {
    Walk,
    Run,
    RunBack,
    Swim,
    SwimBack,
    TurnRate,
}

impl SpeedKind {
    /// The `CMSG_FORCE_*_SPEED_CHANGE_ACK` opcode answering this kind's SMSG.
    pub fn ack_opcode(self) -> u16 {
        use super::opcode;
        match self {
            SpeedKind::Walk => opcode::CMSG_FORCE_WALK_SPEED_CHANGE_ACK,
            SpeedKind::Run => opcode::CMSG_FORCE_RUN_SPEED_CHANGE_ACK,
            SpeedKind::RunBack => opcode::CMSG_FORCE_RUN_BACK_SPEED_CHANGE_ACK,
            SpeedKind::Swim => opcode::CMSG_FORCE_SWIM_SPEED_CHANGE_ACK,
            SpeedKind::SwimBack => opcode::CMSG_FORCE_SWIM_BACK_SPEED_CHANGE_ACK,
            SpeedKind::TurnRate => opcode::CMSG_FORCE_TURN_RATE_CHANGE_ACK,
        }
    }
}

/// A mode the server grants the controlling client, one `MOVEMENTFLAGS` bit each (vmangos
/// `MovementInfo.h:28-62`, reference setters `0x7c7280`-`0x7c7370`). Root stops translation and
/// falling but not turning (`0x618054`); water walk makes liquid walkable (aura 104); feather fall
/// caps the fall at 7 yd/s instead of 60.148 (aura 105, `0x7c5d20`); hover lifts ground contact by
/// 1.0 yd (aura 106, `0x6367b0`). Levitate (spell 1706) grants the last three at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveMode {
    Root,
    WaterWalk,
    FeatherFall,
    Hover,
}

impl MoveMode {
    /// This mode's `MOVEMENTFLAGS` bit.
    pub fn flag(self) -> u32 {
        match self {
            MoveMode::Root => 0x0000_1000,
            MoveMode::WaterWalk => 0x1000_0000,
            MoveMode::FeatherFall => 0x2000_0000,
            MoveMode::Hover => 0x4000_0000,
        }
    }

    /// The `CMSG_*_ACK` answering this mode's SMSG. Only root acks apply and remove on different
    /// opcodes (vmangos routes both to `HandleMoveRootAck`).
    pub fn ack_opcode(self, apply: bool) -> u16 {
        use super::opcode;
        match (self, apply) {
            (MoveMode::Root, true) => opcode::CMSG_FORCE_MOVE_ROOT_ACK,
            (MoveMode::Root, false) => opcode::CMSG_FORCE_MOVE_UNROOT_ACK,
            (MoveMode::WaterWalk, _) => opcode::CMSG_MOVE_WATER_WALK_ACK,
            (MoveMode::FeatherFall, _) => opcode::CMSG_MOVE_FEATHER_FALL_ACK,
            (MoveMode::Hover, _) => opcode::CMSG_MOVE_HOVER_ACK,
        }
    }

    /// Whether the ack ends in a `u32 apply`: not root's, whose handler infers it from the opcode
    /// (vmangos `Server/Packets/Movement.cpp:38-59`).
    pub fn ack_carries_apply(self) -> bool {
        !matches!(self, MoveMode::Root)
    }
}

/// A mode set on a unit we do not control, by the twelve `SMSG_SPLINE_MOVE_*`: a bare packed guid,
/// no counter, no ack (client handler `0x603c80`, any unit). vmangos sends the four shared modes
/// only for units no player moves, and walk/run always. Walk mode and swimming have no acked form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplineMode {
    Root,
    WaterWalk,
    FeatherFall,
    Hover,
    /// `MOVEFLAG_WALK_MODE` (`0x100`); `apply` is the flag's direction, so `SET_WALK_MODE` applies
    /// and `SET_RUN_MODE` removes (the reference `0x617e80` feeds `SetRunMode`, `0x7c71c0`).
    WalkMode,
    /// `MOVEFLAG_SWIMMING` (`0x20_0000`), by `START_SWIM`/`STOP_SWIM`. The reference handles them
    /// (`0x61a130`, `0x61a160`); vmangos never sends them (`Opcodes.cpp:888`).
    Swimming,
}

impl SplineMode {
    /// This mode's `MOVEMENTFLAGS` bit, in the same word as [`MoveMode::flag`].
    pub fn flag(self) -> u32 {
        match self {
            SplineMode::Root => 0x0000_1000,
            SplineMode::WaterWalk => 0x1000_0000,
            SplineMode::FeatherFall => 0x2000_0000,
            SplineMode::Hover => 0x4000_0000,
            SplineMode::WalkMode => 0x0000_0100,
            SplineMode::Swimming => 0x0020_0000,
        }
    }
}

/// What a relayed move's opcode means beyond its pose. The 23 `[packed guid][MovementInfo]` relays
/// share one reference handler (`0x603bb0`, `0x601580`), and for most the opcode adds nothing the
/// flags word does not say. The variants beyond `Pose` name the opcodes a receiver tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RelayVerb {
    /// Every other relay (start, stop, strafe, jump, turn, swim, land, facing, modes, knockback).
    #[default]
    Pose,
    /// `MSG_MOVE_HEARTBEAT`, the periodic mid-move pulse. Both reference blends apply to it
    /// (`0x619030`, `0x619090`); the tag they skip, `0x26`, is only pushed for a teleport.
    Heartbeat,
    /// `MSG_MOVE_TELEPORT`, an observed near-teleport (Blink, `.tele`): the one relay the client
    /// never smooths toward (tag `0x26`). It also re-bases the mover, zeroes its interpolation
    /// (`0x617e90`) and past 30 yd re-anchors the world.
    Teleport,
    /// `MSG_MOVE_ROOT` (`true`) / `MSG_MOVE_UNROOT` (`false`): the opcode, not the flags word,
    /// decides. The client then runs `SetRoot` (`0x7c7340`, also wiping motion) or `ClearRoot`
    /// (`0x7c7370`) unconditionally; vmangos's flags already agree (`MovementHandler.cpp:1064`).
    Root(bool),
}

impl RelayVerb {
    /// The verb a relay opcode carries.
    pub fn of(opcode: u16) -> Self {
        use super::opcode as op;
        match opcode {
            op::MSG_MOVE_HEARTBEAT => Self::Heartbeat,
            op::MSG_MOVE_TELEPORT => Self::Teleport,
            op::MSG_MOVE_ROOT => Self::Root(true),
            op::MSG_MOVE_UNROOT => Self::Root(false),
            _ => Self::Pose,
        }
    }
}

/// The `JUMPING` tail: `zspeed, cosAngle, sinAngle, xyspeed`, cos before sin (vmangos
/// `MovementInfo::Read`). The launch is `(cos, sin) * xy_speed` in world XY, frozen at take-off,
/// so a receiver can replay the arc under `g = 19.291105` (vmangos `ExtrapolateMovement`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct JumpInfo {
    /// Take-off vertical speed (yd/s), down-positive: the 1.12 client sends `-7.955547` for a
    /// rising jump, so the up-speed is `-zspeed`.
    pub zspeed: f32,
    pub cos_angle: f32,
    pub sin_angle: f32,
    /// Horizontal launch speed (yd/s); the frozen ground speed at take-off.
    pub xy_speed: f32,
}

/// The `ON_TRANSPORT` tail of `MovementInfo` and of a `LIVING` block: a full `u64` transport guid,
/// then position and facing in the transport's own frame (vmangos `MovementInfo::Read`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportPose {
    /// The transport GameObject's guid, `HIGH_MO_TRANSPORT` or `HIGH_TRANSPORT`.
    pub guid: u64,
    pub pos: Vector3d,
    pub orientation: f32,
}

/// The body of every `MSG_MOVE_*`, in both directions, and of the teleport ack.
pub struct MovementInfo {
    pub flags: u32,
    pub timestamp: u32,
    pub position: Vector3d,
    pub orientation: f32,
    /// `Some` iff `ON_TRANSPORT` is set: the sender's pose in the transport's frame.
    pub transport: Option<TransportPose>,
    /// Swim pitch in radians, up-positive; on the wire only while `SWIMMING`, else read as `0.0`.
    pub pitch: f32,
    /// Milliseconds airborne, a `u32` (vmangos `MovementInfo::fallTime`): the jump arc's clock.
    pub fall_time: u32,
    /// `Some` iff `JUMPING` is set: the launch an observer replays the arc from.
    pub jump: Option<JumpInfo>,
}

impl MovementInfo {
    pub(super) fn write(&self, w: &mut Vec<u8>) {
        w.extend_from_slice(&self.flags.to_le_bytes());
        w.extend_from_slice(&self.timestamp.to_le_bytes());
        w.extend_from_slice(&self.position.x.to_le_bytes());
        w.extend_from_slice(&self.position.y.to_le_bytes());
        w.extend_from_slice(&self.position.z.to_le_bytes());
        w.extend_from_slice(&self.orientation.to_le_bytes());
        // Each conditional tail must travel with its flag or vmangos `MovementInfo::Read` desyncs.
        if self.flags & MOVEMENT_FLAG_ON_TRANSPORT != 0 {
            let t = self.transport.unwrap_or(TransportPose {
                guid: 0,
                pos: Vector3d {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                orientation: 0.0,
            });
            w.extend_from_slice(&t.guid.to_le_bytes());
            w.extend_from_slice(&t.pos.x.to_le_bytes());
            w.extend_from_slice(&t.pos.y.to_le_bytes());
            w.extend_from_slice(&t.pos.z.to_le_bytes());
            w.extend_from_slice(&t.orientation.to_le_bytes());
        }
        if self.flags & MOVEMENT_FLAG_SWIMMING != 0 {
            w.extend_from_slice(&self.pitch.to_le_bytes());
        }
        w.extend_from_slice(&self.fall_time.to_le_bytes());
        if self.flags & MOVEMENT_FLAG_JUMPING != 0 {
            let j = self.jump.unwrap_or_default();
            w.extend_from_slice(&j.zspeed.to_le_bytes());
            w.extend_from_slice(&j.cos_angle.to_le_bytes());
            w.extend_from_slice(&j.sin_angle.to_le_bytes());
            w.extend_from_slice(&j.xy_speed.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Vector3d;

    fn info(flags: u32) -> MovementInfo {
        MovementInfo {
            flags,
            timestamp: 12345,
            position: Vector3d {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            orientation: 0.5,
            transport: None,
            pitch: -0.7,
            fall_time: 42,
            jump: None,
        }
    }

    #[test]
    fn swim_pitch_round_trips_when_swimming() {
        let mut bytes = Vec::new();
        info(MOVEMENT_FLAG_SWIMMING).write(&mut bytes);
        let back = read_movement_info(&mut &bytes[..]).expect("parse");
        assert_eq!(back.flags, MOVEMENT_FLAG_SWIMMING);
        assert_eq!(back.pitch, -0.7, "the swim pitch survives the round-trip");
        assert_eq!(back.orientation, 0.5);
        assert_eq!(
            back.fall_time, 42,
            "fall_time still lands after the pitch tail"
        );
    }

    #[test]
    fn transport_tail_round_trips_when_riding() {
        let mut riding = info(MOVEMENT_FLAG_ON_TRANSPORT);
        riding.transport = Some(TransportPose {
            guid: 0x1FC0_0000_0000_2B10, // a HIGH_MO_TRANSPORT-shaped guid
            pos: Vector3d {
                x: -5.5,
                y: 2.25,
                z: 8.0,
            },
            orientation: 1.5,
        });
        let mut bytes = Vec::new();
        riding.write(&mut bytes);
        let mut walking = Vec::new();
        info(0).write(&mut walking);
        assert_eq!(
            bytes.len() - walking.len(),
            24,
            "the transport tail adds exactly u64 + 4×f32"
        );
        let back = read_movement_info(&mut &bytes[..]).expect("parse");
        let t = back.transport.expect("transport pose survives");
        assert_eq!(t.guid, 0x1FC0_0000_0000_2B10);
        assert_eq!((t.pos.x, t.pos.y, t.pos.z), (-5.5, 2.25, 8.0));
        assert_eq!(t.orientation, 1.5);
        assert_eq!(back.fall_time, 42, "fall_time lands after the tail");
    }

    #[test]
    fn no_pitch_tail_when_not_swimming() {
        let mut swimming = Vec::new();
        info(MOVEMENT_FLAG_SWIMMING).write(&mut swimming);
        let mut dry = Vec::new();
        info(0).write(&mut dry);
        assert_eq!(
            swimming.len() - dry.len(),
            4,
            "the swim pitch adds exactly one f32 to the packet, and only when swimming"
        );
        let back = read_movement_info(&mut &dry[..]).expect("parse");
        assert_eq!(back.pitch, 0.0, "a dry packet parses a level (0) pitch");
        assert_eq!(back.fall_time, 42);
    }
}
