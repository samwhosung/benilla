//! `TransportAnimation.dbc` — the type-11 TRANSPORT (elevator/lift/tram) keyframe paths, and the
//! client's own cycle evaluator. Pure math, no Bevy, **WoW coordinates throughout**.
//!
//! A type-11 GameObject never streams a position: the server sends one anchor (the create
//! movement block's `UPDATE_FLAG_TRANSPORT` u32, its `time-since-GO-create % period` clock) and
//! the client animates the car itself — `target = (anchor + local_elapsed) % period`, bracket the
//! two keyframes around `target`, lerp their **local offsets**, rotate the offset by the spawn's
//! `GAMEOBJECT_ROTATION` quaternion, and add the stationary spawn position. The mechanism is
//! the client's path evaluator `0x5f6280` + the type-11 tick `0x5f5f10`; vmangos's
//! `ElevatorTransport::Update` (`Transport.cpp:396-437`) is the same math on the server side,
//! which is what keeps the two in sync. This module implements the *mechanism* idiomatically
//! (f32, binary-search bracket), not the reference's x87 spill pattern.
//!
//! **7 fields:** `ID(0), TransportID(1), TimeIndex(2), PosX(3), PosY(4), PosZ(5), SequenceID(6)`
//! — `TransportID` is the **gameobject_template entry** (not a display or path id), `TimeIndex`
//! is cumulative ms, and the last frame's `TimeIndex` IS the cycle period (`0x5f6280` reads
//! `segments[count-1].time` as its modulus; vmangos `TotalTime` likewise). `SequenceID` names the
//! car M2's animation per span (162 moving / 164 stationary on the live rows) — not consumed
//! here (deferred polish; the car still renders its idle).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const TRANSPORT_ANIMATION: &str = "DBFilesClient\\TransportAnimation.dbc";

/// One `TransportAnimation.dbc` row — a keyframe on a type-11 transport's authored local path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElevatorKeyframe {
    /// Cumulative cycle time, ms. The path's last frame's `time_ms` is the cycle period.
    pub time_ms: u32,
    /// Local offset from the spawn point (WoW axes), **before** the spawn-rotation is applied.
    pub pos: [f32; 3],
}

/// `TransportAnimation.dbc` grouped by `TransportID` (= `gameobject_template` entry), each
/// path's frames sorted by `TimeIndex`.
pub struct ElevatorPaths {
    by_entry: HashMap<u32, Vec<ElevatorKeyframe>>,
}

impl ElevatorPaths {
    /// A template entry's keyframes in time order, or `None` for a GO with no authored path.
    /// A returned slice is never empty and always starts at `time_ms == 0` (load guarantees).
    pub fn entry(&self, template_entry: u32) -> Option<&[ElevatorKeyframe]> {
        self.by_entry.get(&template_entry).map(Vec::as_slice)
    }

    /// Number of distinct transport entries carrying a path.
    pub fn len(&self) -> usize {
        self.by_entry.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_entry.is_empty()
    }
}

/// 7 fields per the module doc.
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

/// Read `TransportAnimation.dbc` off the patch chain into an [`ElevatorPaths`]. Paths that could
/// not drive a cycle (fewer than 2 frames, a zero period, or a first frame not at `t=0`) are
/// dropped with the same silence the client affords them — a GO whose entry isn't here simply
/// renders frozen at its spawn point.
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

/// The path's cycle period, ms — the last keyframe's cumulative time (`0x5f6280`'s modulus).
pub fn elevator_period_ms(frames: &[ElevatorKeyframe]) -> u32 {
    frames.last().map_or(1, |f| f.time_ms).max(1)
}

/// The client's type-11 cycle evaluator (`0x5f6280`, mechanism form):
/// world position of the car at `cycle_ms ∈ [0, period)`, given the spawn's stationary position
/// and its `GAMEOBJECT_ROTATION` quaternion `(x, y, z, w)`. Also reports whether the bracketing
/// span is in motion (the dock/depart edge for consumers' instruments).
///
/// The rotation is a plain quaternion rotation of the local offset — the binary builds the
/// standard 3×3 of `q` row-major and combines transposed, which IS `R(q)·d`. vmangos reproduces
/// it as `(d * q)` + a y sign flip
/// (`Transport.cpp:426-428` "magical sign flip but it works"); the two agree on every live 1.12
/// row with `x = y = 0` offsets and on pure-yaw spawn quats — the tests pin one worked case.
pub fn elevator_sample(
    frames: &[ElevatorKeyframe],
    spawn_pos: [f32; 3],
    spawn_quat: [f32; 4],
    cycle_ms: u32,
) -> ([f32; 3], bool) {
    debug_assert!(frames.len() >= 2 && frames[0].time_ms == 0);
    let period = elevator_period_ms(frames);
    let target = cycle_ms % period;

    // Bracket: the last frame with `time_ms <= target` and its successor. `partition_point` on
    // the sorted times replaces the binary's ring-cursor scan (same bracket, stateless).
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

/// `R(q)·v` — rotate a vector by a unit quaternion `(x, y, z, w)`.
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
        // Dwell window: both bracketing frames at the same offset -> parked, not moving.
        let (p, moving) = elevator_sample(&f, base, IDENT, 2500);
        assert_eq!(p, base);
        assert!(!moving);
        // Mid-descent: t=7500 is halfway through the 5000->10000 span -> z offset -5.
        let (p, moving) = elevator_sample(&f, base, IDENT, 7500);
        assert_eq!(p, [100.0, 200.0, 45.0]);
        assert!(moving);
        // The cycle wraps at period (30000): cycle 30000 IS cycle 0.
        let (p0, _) = elevator_sample(&f, base, IDENT, 0);
        let (pw, _) = elevator_sample(&f, base, IDENT, 30000);
        assert_eq!(p0, pw);
        // Just before wrap: nearly back at the spawn offset.
        let (p, moving) = elevator_sample(&f, base, IDENT, 29999);
        assert!(moving);
        assert!((p[2] - 50.0).abs() < 0.01, "z = {}", p[2]);
    }

    /// One worked rotation case: a pure-yaw spawn quat (the only kind on live 1.12 spawn rows)
    /// turning a lateral offset. q = 90° about +Z: (x, y) -> (-y, x).
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

    /// The real 5875 table: the Mesa Elevator pair + the Undercity elevator prove layout and the
    /// load guarantees (sorted, t0 = 0, period = last frame). Skips without client data.
    #[test]
    fn real_transport_animation_layout_sanity() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let paths = load_elevator_paths(&mut chain).expect("load TransportAnimation.dbc");
        // The two Thunder Bluff Mesa Elevator cars (gameobject_template 4170/4171) and the
        // Undercity elevator (152614) — verified against the extracted table this session.
        let top = paths.entry(4170).expect("Mesa Elevator 4170");
        assert_eq!(elevator_period_ms(top), 30033);
        assert_eq!(top[0].time_ms, 0);
        assert_eq!(top[0].pos, [0.0, 0.0, 0.0]);
        assert!(top.windows(2).all(|w| w[0].time_ms <= w[1].time_ms));
        // The car travels on z only — x/y are authoring noise at ~2e-6 yd (the rotation
        // question is moot for pure-vertical paths). Full descent: 61.24 yd.
        assert!(top
            .iter()
            .all(|f| f.pos[0].abs() < 1e-3 && f.pos[1].abs() < 1e-3));
        assert!((top.iter().map(|f| f.pos[2]).fold(0.0f32, f32::min) + 61.244).abs() < 1e-3);
        let bottom = paths.entry(4171).expect("Mesa Elevator 4171");
        assert_eq!(elevator_period_ms(bottom), 30000);
        assert!(paths.entry(152614).is_some(), "Undercity elevator");
        // A no-path entry answers None (999999 is no template).
        assert!(paths.entry(999_999).is_none());
    }
}
