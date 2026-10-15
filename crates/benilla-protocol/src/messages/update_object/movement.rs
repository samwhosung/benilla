use std::io::{self, Read};

use crate::messages::movement::{
    TransportPose, MOVEMENT_FLAG_JUMPING, MOVEMENT_FLAG_ON_TRANSPORT,
    MOVEMENT_FLAG_SPLINE_ELEVATION, MOVEMENT_FLAG_SPLINE_ENABLED, MOVEMENT_FLAG_SWIMMING,
};
use crate::wire::{
    capacity_hint, read_f32_le, read_packed_guid, read_u32_le, read_u64_le, read_u8, Vector3d,
};

/// Object class (the `TypeId` on a create packet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    Object,
    Item,
    Container,
    Unit,
    Player,
    GameObject,
    DynamicObject,
    Corpse,
}

impl ObjectType {
    pub(super) fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Item,
            2 => Self::Container,
            3 => Self::Unit,
            4 => Self::Player,
            5 => Self::GameObject,
            6 => Self::DynamicObject,
            7 => Self::Corpse,
            _ => Self::Object,
        }
    }
}

const UPDATE_FLAG_TRANSPORT: u8 = 0x02;
const UPDATE_FLAG_MELEE_ATTACKING: u8 = 0x04;
const UPDATE_FLAG_HIGH_GUID: u8 = 0x08;
const UPDATE_FLAG_ALL: u8 = 0x10;
const UPDATE_FLAG_LIVING: u8 = 0x20;
const UPDATE_FLAG_HAS_POSITION: u8 = 0x40;
const SPLINE_FLAG_FINAL_POINT: u32 = 0x1_0000;
const SPLINE_FLAG_FINAL_TARGET: u32 = 0x2_0000;
const SPLINE_FLAG_FINAL_ANGLE: u32 = 0x4_0000;
/// `MoveSplineFlag::Flying`; in vmangos it alone is `Mask_CatmullRom` (`MoveSplineFlag.h:48,77`).
const SPLINE_FLAG_FLYING: u32 = 0x200;
const SPLINE_FLAG_RUNMODE: u32 = 0x100;
/// `MoveSplineFlag::Cyclic` (`MoveSplineFlag.h:59`).
const SPLINE_FLAG_CYCLIC: u32 = 0x10_0000;

/// The spline a `LIVING` unit is already riding when it streams in (`packet_builder.cpp:152`).
/// vmangos builds every spline as Catmull-Rom (`spline.cpp:52`), so the wire array is
/// `[phantom, p₀, …, pₙ, tail]` and [`Self::path`] is its walkable range `1 ..= len − 2`.
#[derive(Debug, Clone)]
pub struct CreateSpline {
    /// The travel-order polyline in raw WoW coords, start to endpoint.
    pub path: Vec<[f32; 3]>,
    /// The server's spline id (`MoveSpline::GetId`), as on `MonsterMove`.
    pub id: u32,
    /// Ms of the path already ridden (`MoveSpline::timePassed`); start the ride at now minus this.
    pub time_passed_ms: u32,
    /// Duration in ms at one constant speed, time uniform in arc length (`MoveSpline.cpp:125-135`).
    pub duration_ms: u32,
    /// `MoveSplineFlag::Flying`: a 3-D flight path (keep the spline's Z); clear for a ground walk.
    pub flying: bool,
    /// `MoveSplineFlag::Cyclic`: the server loops this path forever.
    pub cyclic: bool,
    /// `MoveSplineFlag::Runmode`; its absence forces walk mode on.
    pub run_mode: bool,
}

/// A `LIVING` block's live mover state: the `MOVEMENTFLAGS` word and the swim pitch that rides it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoverState {
    /// The raw `MOVEMENTFLAGS` word (`CMovement+0x40`).
    pub flags: u32,
    /// Swim pitch in radians, +up; on the wire only with `MOVEFLAG_SWIMMING`, else `0.0`.
    pub pitch: f32,
}

/// The decoded fields of a movement block; the rest is read and discarded to stay aligned.
pub struct MovementBlock {
    pub position: Option<(Vector3d, f32)>,
    /// A `LIVING` block's mover state. The reference merges these flags under the relay's
    /// `0x75a07dff` mask (`0x618c30`) and commits the pitch at once (`0x7c6420`), so a unit
    /// streaming in mid-swim is pitched on its first frame.
    pub mover: Option<MoverState>,
    /// A `LIVING` block's speeds in wire order `[walk, run, run_back, swim, swim_back, turn_rate]`.
    pub speeds: Option<[f32; 6]>,
    /// A ship's or elevator's path progress in ms at create, the anchor its cycle runs from
    /// (`Object.cpp:590-605`); no other object sets `UPDATE_FLAG_TRANSPORT`.
    pub transport_progress: Option<u32>,
    /// A `LIVING` block's rider pose (`MOVEFLAG_ON_TRANSPORT`), local to the transport's frame.
    pub transport: Option<TransportPose>,
    /// The path this unit is already walking at create (`MOVEFLAG_SPLINE_ENABLED`).
    pub spline: Option<CreateSpline>,
}

impl MovementBlock {
    pub(super) fn read(r: &mut impl Read) -> io::Result<Self> {
        let update_flag = read_u8(r)?;
        let mut position = None;
        let mut mover = None;
        let mut speeds = None;
        let mut transport = None;
        let mut transport_progress = None;
        let mut spline = None;

        if update_flag & UPDATE_FLAG_LIVING != 0 {
            let flags = read_u32_le(r)?;
            let _timestamp = read_u32_le(r)?;
            let living_position = Vector3d::read(r)?;
            let living_orientation = read_f32_le(r)?;
            position = Some((living_position, living_orientation));

            if flags & MOVEMENT_FLAG_ON_TRANSPORT != 0 {
                // A full u64, not packed, as in `MovementInfo::Write` (`Object.cpp:524`).
                transport = Some(TransportPose {
                    guid: read_u64_le(r)?,
                    pos: Vector3d::read(r)?,
                    orientation: read_f32_le(r)?,
                });
            }
            let pitch = if flags & MOVEMENT_FLAG_SWIMMING != 0 {
                read_f32_le(r)?
            } else {
                0.0
            };
            mover = Some(MoverState { flags, pitch });
            let _fall_time = read_f32_le(r)?;
            if flags & MOVEMENT_FLAG_JUMPING != 0 {
                for _ in 0..4 {
                    let _ = read_f32_le(r)?; // z_speed, cos_angle, sin_angle, xy_speed
                }
            }
            if flags & MOVEMENT_FLAG_SPLINE_ELEVATION != 0 {
                let _ = read_f32_le(r)?;
            }
            let mut s = [0.0f32; 6];
            for slot in &mut s {
                *slot = read_f32_le(r)?;
            }
            speeds = Some(s);
            if flags & MOVEMENT_FLAG_SPLINE_ENABLED != 0 {
                let spline_flags = read_u32_le(r)?;
                // The final facing, three independent reads in the reference's order: a point
                // (`0x47f0bf`), a guid (`0x47f0e8`), an angle (`0x47f0f9`). vmangos writes at most
                // one (`packet_builder.cpp:162-167`); a unit on a path faces along it.
                if spline_flags & SPLINE_FLAG_FINAL_POINT != 0 {
                    let _ = Vector3d::read(r)?;
                }
                if spline_flags & SPLINE_FLAG_FINAL_TARGET != 0 {
                    let _ = read_u64_le(r)?;
                }
                if spline_flags & SPLINE_FLAG_FINAL_ANGLE != 0 {
                    let _ = read_f32_le(r)?;
                }
                let time_passed_ms = read_u32_le(r)?;
                let duration_ms = read_u32_le(r)?;
                let id = read_u32_le(r)?;
                let amount_of_nodes = read_u32_le(r)?;
                let mut nodes = Vec::with_capacity(capacity_hint(amount_of_nodes, 0xFFFF));
                for _ in 0..amount_of_nodes {
                    let v = Vector3d::read(r)?;
                    nodes.push([v.x, v.y, v.z]);
                }
                // The final destination repeats the path's last point, or is zero on a cyclic path.
                let _final_node = Vector3d::read(r)?;
                // Drop the two virtual control points; under four nodes there is no path to ride.
                spline = (nodes.len() >= 4).then(|| CreateSpline {
                    path: nodes[1..nodes.len() - 1].to_vec(),
                    id,
                    time_passed_ms,
                    duration_ms,
                    flying: spline_flags & SPLINE_FLAG_FLYING != 0,
                    cyclic: spline_flags & SPLINE_FLAG_CYCLIC != 0,
                    run_mode: spline_flags & SPLINE_FLAG_RUNMODE != 0,
                });
            }
        } else if update_flag & UPDATE_FLAG_HAS_POSITION != 0 {
            let pos = Vector3d::read(r)?;
            let orientation = read_f32_le(r)?;
            position = Some((pos, orientation));
        }

        if update_flag & UPDATE_FLAG_HIGH_GUID != 0 {
            let _ = read_u32_le(r)?;
        }
        if update_flag & UPDATE_FLAG_ALL != 0 {
            let _ = read_u32_le(r)?;
        }
        if update_flag & UPDATE_FLAG_MELEE_ATTACKING != 0 {
            let _ = read_packed_guid(r)?;
        }
        if update_flag & UPDATE_FLAG_TRANSPORT != 0 {
            transport_progress = Some(read_u32_le(r)?);
        }

        Ok(Self {
            position,
            mover,
            speeds,
            transport_progress,
            transport,
            spline,
        })
    }
}
