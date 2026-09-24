//! `TransportAnimation.dbc`, the keyframe paths of type-11 transports (elevators), and the
//! reference's cycle evaluator over them, in WoW coordinates. The server sends one anchor, the
//! create block's `UPDATE_FLAG_TRANSPORT` clock (time since creation, modulo the period), and the
//! client animates the car: bracket the keyframes around `(anchor + elapsed) % period`, lerp their
//! offsets, rotate by the spawn's `GAMEOBJECT_ROTATION` and add the spawn position (`0x5f6280`,
//! ticked by `0x5f5f10`), as vmangos's `ElevatorTransport::Update` does (`Transport.cpp:396-435`).
//! `TransportID` is the `gameobject_template` entry; `SequenceID`, the car's animation per span, is
//! not read.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const TRANSPORT_ANIMATION: &str = "DBFilesClient\\TransportAnimation.dbc";

/// One `TransportAnimation.dbc` row, a keyframe on a type-11 transport's local path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElevatorKeyframe {
    /// Cumulative cycle time in ms; the last frame's is the period.
    pub time_ms: u32,
    /// Offset from the spawn point in WoW axes, before the spawn rotation.
    pub pos: [f32; 3],
}

/// `TransportAnimation.dbc` grouped by `TransportID`, each path's frames sorted by `TimeIndex`.
pub struct ElevatorPaths {
    by_entry: HashMap<u32, Vec<ElevatorKeyframe>>,
}

impl ElevatorPaths {
    /// A template entry's keyframes in time order: at least two, the first at `time_ms == 0`.
    pub fn entry(&self, template_entry: u32) -> Option<&[ElevatorKeyframe]> {
        self.by_entry.get(&template_entry).map(Vec::as_slice)
    }

    /// The number of transport entries with a path.
    pub fn len(&self) -> usize {
        self.by_entry.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_entry.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("TransportAnimation");
    for name in ["ID", "TransportID", "TimeIndex"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for name in ["PosX", "PosY", "PosZ"] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s.add_field(SchemaField::new("SequenceID", FieldType::UInt32));
    s
}

/// Read `TransportAnimation.dbc` off the patch chain, silently dropping, as the reference does, a
/// path that cannot drive a cycle (under 2 frames, a zero period, a first frame past 0); its GO
/// stays at its spawn point.
pub fn load_elevator_paths(chain: &mut Chain) -> Result<ElevatorPaths> {
    let bytes = chain
        .read_file(TRANSPORT_ANIMATION)
        .context("reading TransportAnimation.dbc")?;
    let rs = parse(&bytes, schema(), "TransportAnimation")?;
    let mut by_entry: HashMap<u32, Vec<ElevatorKeyframe>> = HashMap::new();
    for r in rs.records() {
        let Some(entry) = u32_at(r, 1) else { continue };
        let Some(time_ms) = u32_at(r, 2) else {
            continue;
        };
        let (Some(x), Some(y), Some(z)) = (f32_at(r, 3), f32_at(r, 4), f32_at(r, 5)) else {
            continue;
        };
        by_entry.entry(entry).or_default().push(ElevatorKeyframe {
            time_ms,
            pos: [x, y, z],
        });
    }
    for frames in by_entry.values_mut() {
        frames.sort_by_key(|f| f.time_ms);
    }
    by_entry.retain(|_, f| f.len() >= 2 && f[0].time_ms == 0 && f[f.len() - 1].time_ms > 0);
    Ok(ElevatorPaths { by_entry })
}

/// The cycle period in ms, the last keyframe's time (`0x5f6280`'s modulus).
pub fn elevator_period_ms(frames: &[ElevatorKeyframe]) -> u32 {
    frames.last().map_or(1, |f| f.time_ms).max(1)
}

/// The reference's type-11 evaluator (`0x5f6280`): the car's world position at `cycle_ms`, from
/// the spawn position and its `GAMEOBJECT_ROTATION` quaternion `(x, y, z, w)`, and whether the
/// bracketing span moves. The offset turns by `R(q)`, as the reference's transposed 3×3 does;
/// vmangos's `d * q` with a y sign flip (`Transport.cpp:425-426`) agrees on the 1.12 data's
/// vertical paths and pure-yaw spawns.
pub fn elevator_sample(
    frames: &[ElevatorKeyframe],
    spawn_pos: [f32; 3],
    spawn_quat: [f32; 4],
    cycle_ms: u32,
) -> ([f32; 3], bool) {
    debug_assert!(frames.len() >= 2 && frames[0].time_ms == 0);
    let period = elevator_period_ms(frames);
    let target = cycle_ms % period;

    // The reference's bracket, found by search rather than its ring cursor.
    let hi = frames.partition_point(|f| f.time_ms <= target);
    let (prev, next) = (frames[hi - 1], frames[hi.min(frames.len() - 1)]);

    let local = if prev.pos == next.pos || next.time_ms == prev.time_ms {
        prev.pos
    } else {
        let frac = (target - prev.time_ms) as f32 / (next.time_ms - prev.time_ms) as f32;
        [
            prev.pos[0] + (next.pos[0] - prev.pos[0]) * frac,
            prev.pos[1] + (next.pos[1] - prev.pos[1]) * frac,
            prev.pos[2] + (next.pos[2] - prev.pos[2]) * frac,
        ]
    };

    let rotated = rotate_by_quat(local, spawn_quat);
    (
        [
            spawn_pos[0] + rotated[0],
            spawn_pos[1] + rotated[1],
            spawn_pos[2] + rotated[2],
        ],
        prev.pos != next.pos,
    )
}

/// `R(q)·v` for a unit quaternion `(x, y, z, w)`.
fn rotate_by_quat(v: [f32; 3], q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q;
    // t = 2·(q.xyz × v); v' = v + w·t + q.xyz × t
    let t = [
        2.0 * (y * v[2] - z * v[1]),
        2.0 * (z * v[0] - x * v[2]),
        2.0 * (x * v[1] - y * v[0]),
    ];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames() -> Vec<ElevatorKeyframe> {
        vec![
            ElevatorKeyframe {
                time_ms: 0,
                pos: [0.0, 0.0, 0.0],
            },
            ElevatorKeyframe {
                time_ms: 5000,
                pos: [0.0, 0.0, 0.0],
            },
            ElevatorKeyframe {
                time_ms: 10000,
                pos: [0.0, 0.0, -10.0],
            },
            ElevatorKeyframe {
                time_ms: 30000,
                pos: [0.0, 0.0, 0.0],
            },
        ]
    }

    const IDENT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    #[test]
    fn dwell_then_lerp_then_wrap() {
        let f = frames();
        let base = [100.0, 200.0, 50.0];
        let (p, moving) = elevator_sample(&f, base, IDENT, 2500);
        assert_eq!(p, base);
        assert!(!moving);
        // 7500 is halfway through the 5000 to 10000 span: z offset -5.
        let (p, moving) = elevator_sample(&f, base, IDENT, 7500);
        assert_eq!(p, [100.0, 200.0, 45.0]);
        assert!(moving);
        let (p0, _) = elevator_sample(&f, base, IDENT, 0);
        let (pw, _) = elevator_sample(&f, base, IDENT, 30000);
        assert_eq!(p0, pw);
        let (p, moving) = elevator_sample(&f, base, IDENT, 29999);
        assert!(moving);
        assert!((p[2] - 50.0).abs() < 0.01, "z = {}", p[2]);
    }

    /// A 90° yaw (1.12 transport spawns only yaw) turns `(x, y)` into `(-y, x)`.
    #[test]
    fn spawn_yaw_rotates_the_local_offset() {
        let half = std::f32::consts::FRAC_PI_4; // 90°/2
        let q = [0.0, 0.0, half.sin(), half.cos()];
        let f = vec![
            ElevatorKeyframe {
                time_ms: 0,
                pos: [10.0, 0.0, 0.0],
            },
            ElevatorKeyframe {
                time_ms: 1000,
                pos: [10.0, 0.0, 0.0],
            },
        ];
        let (p, _) = elevator_sample(&f, [0.0; 3], q, 500);
        assert!(
            (p[0] - 0.0).abs() < 1e-4 && (p[1] - 10.0).abs() < 1e-4,
            "{p:?}"
        );
    }

    /// The 5875 table: layout and load guarantees on three real paths.
    #[test]
    fn real_transport_animation_layout_sanity() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let paths = load_elevator_paths(&mut chain).expect("load TransportAnimation.dbc");
        // Thunder Bluff's Mesa Elevator cars are templates 4170 and 4171, Undercity's 152614.
        let top = paths.entry(4170).expect("Mesa Elevator 4170");
        assert_eq!(elevator_period_ms(top), 30033);
        assert_eq!(top[0].time_ms, 0);
        assert_eq!(top[0].pos, [0.0, 0.0, 0.0]);
        assert!(top.windows(2).all(|w| w[0].time_ms <= w[1].time_ms));
        // The car moves on z only (x and y are ~2e-6 yd of noise), 61.24 yd down.
        assert!(top
            .iter()
            .all(|f| f.pos[0].abs() < 1e-3 && f.pos[1].abs() < 1e-3));
        assert!((top.iter().map(|f| f.pos[2]).fold(0.0f32, f32::min) + 61.244).abs() < 1e-3);
        let bottom = paths.entry(4171).expect("Mesa Elevator 4171");
        assert_eq!(elevator_period_ms(bottom), 30000);
        assert!(paths.entry(152614).is_some(), "Undercity elevator");
        assert!(paths.entry(999_999).is_none());
    }
}
