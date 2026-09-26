//! The server-authored path walk ([`Spline`], `SMSG_MONSTER_MOVE`) and the terrain re-ground
//! that goes with it ([`ground_clamp_creatures`]).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_protocol::{CreateSpline, EntityKind};
use bevy::prelude::*;

use crate::entities::CollisionHeight;
use crate::player::swim_enter_depth;

use super::super::{NetEntity, ObjectStore};

/// A server-dictated path (`SMSG_MONSTER_MOVE`) in raw WoW coordinates, walked at constant speed
/// over `duration` from `start`; present only while the unit is on it.
#[derive(Component, Debug, Clone)]
pub(crate) struct Spline {
    pub(crate) points: Vec<[f32; 3]>,
    pub(crate) start: Instant,
    pub(crate) duration: Duration,
    /// The server's spline id, echoed in `CMSG_MOVE_SPLINE_DONE` when the spline drives our own
    /// body; the server accepts only its newest id (vmangos `MovementHandler.cpp:807`).
    pub(crate) id: u32,
    /// The `FLYING` bit was clear: the 1.12 client zeroes the spline's Δz (`0x616cec`) and takes Z
    /// from the terrain, so [`ground_clamp_creatures`] re-grounds this unit. Flying keeps its Z.
    pub(crate) grounded: bool,
    /// `SPLINEFLAG_RUNMODE`: the 1.12 client's commit `0x7c6a50` passes it to
    /// `CMovement::SetRunMode` (`0x7c71c0`, from `0x7c6ac2`), so a spline without it sets
    /// `MOVEFLAG_WALK_MODE`. Only the body we drive reads it ([`crate::player::server_ride`]); a
    /// creature's gait comes from [`Spline::speed`].
    pub(crate) run_mode: bool,
    /// The transport of a deck path (`SMSG_MONSTER_MOVE_TRANSPORT`), whose points are deck-local:
    /// sampled into the rider's [`crate::transport::TransportRider`] pose, never terrain-clamped.
    pub(crate) deck: Option<u64>,
}

/// The id of a spline the server ended with a stop, owed in `CMSG_MOVE_SPLINE_DONE`. vmangos arms
/// the pending ack for a player or a player-possessed unit (`MoveSplineInit.cpp:131`,
/// `Unit.cpp:9325`) and drops every movement packet from it until an ack carries this id, not the
/// interrupted path's (`MovementHandler.cpp:295`). [`crate::player::server_ride`] consumes it.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct SplineStopped(pub(crate) u32);

impl Spline {
    /// Average speed in yd/s (path length over duration); [`crate::creature_anim`] picks Walk or
    /// Run from it.
    pub(crate) fn speed(&self) -> f32 {
        let length: f32 = self
            .points
            .windows(2)
            .map(|w| {
                let (dx, dy, dz) = (w[1][0] - w[0][0], w[1][1] - w[0][1], w[1][2] - w[0][2]);
                (dx * dx + dy * dy + dz * dz).sqrt()
            })
            .sum();
        length / self.duration.as_secs_f32().max(1e-3)
    }

    /// Elapsed over duration, clamped to `0..=1`; the inspector's motion line reads it.
    pub(crate) fn elapsed_frac(&self) -> f32 {
        (Instant::now()
            .saturating_duration_since(self.start)
            .as_secs_f32()
            / self.duration.as_secs_f32().max(1e-3))
        .clamp(0.0, 1.0)
    }

    /// Raw-WoW position, facing (`None` when vertical) and pitch `asin(dz/len)` at `now`, at
    /// constant speed. Segments are located by chord length here in both modes; the reference
    /// uses the chord on a ground path only and a flying segment's Catmull-Rom length in 20 steps
    /// (`0x454320`, `0x453760`). The evaluator `0x4541b0` lerps a ground path and runs a uniform
    /// Catmull-Rom (`0x453580`) on a flying one.
    pub(crate) fn sample(&self, now: Instant) -> ([f32; 3], Option<f32>, f32) {
        let pts = self.points.as_slice();
        if pts.len() < 2 {
            return (pts.first().copied().unwrap_or([0.0; 3]), None, 0.0);
        }
        let seg = |a: [f32; 3], b: [f32; 3]| {
            let (dx, dy, dz) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
            (dx * dx + dy * dy + dz * dz).sqrt()
        };
        let lengths: Vec<f32> = pts.windows(2).map(|w| seg(w[0], w[1])).collect();
        let total: f32 = lengths.iter().sum();
        if total <= f32::EPSILON {
            return (pts[0], None, 0.0);
        }
        let frac = (now.saturating_duration_since(self.start).as_secs_f32()
            / self.duration.as_secs_f32().max(1e-3))
        .clamp(0.0, 1.0);
        let mut want = frac * total;
        for (i, &len) in lengths.iter().enumerate() {
            if want <= len || i + 1 == lengths.len() {
                let t = if len > f32::EPSILON {
                    (want / len).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (pos, dir) = if self.grounded {
                    let (a, b) = (pts[i], pts[i + 1]);
                    let pos = [
                        a[0] + (b[0] - a[0]) * t,
                        a[1] + (b[1] - a[1]) * t,
                        a[2] + (b[2] - a[2]) * t,
                    ];
                    let dir = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    (pos, dir)
                } else {
                    catmull_rom(pts, i, t)
                };
                let (dx, dy, dz) = (dir[0], dir[1], dir[2]);
                let facing = (dx * dx + dy * dy > 1e-6).then(|| dy.atan2(dx));
                let dlen = (dx * dx + dy * dy + dz * dz).sqrt();
                let pitch = if dlen > f32::EPSILON {
                    (dz / dlen).clamp(-1.0, 1.0).asin()
                } else {
                    0.0
                };
                return (pos, facing, pitch);
            }
            want -= len;
        }
        (*pts.last().unwrap(), None, 0.0)
    }

    /// The flying `(pitch, bank)` in radians (`0x7c5490`'s flying branch): pitch is the tangent's
    /// climb; bank is `2·sign(cross)·acos(t̂·d̂)` between the XY tangent and the XY direction to
    /// the point 1000 ms ahead (`0x7c5623`; 0 under the `2.384e-7` eps), snapped to ±π past ±π/2,
    /// unsmoothed. The lean direction (a left turn rolls left) is inferred: its use is untraced.
    pub(crate) fn flight_attitude(&self, now: Instant) -> (f32, f32) {
        let (pos, facing, pitch) = self.sample(now);
        let Some(f) = facing else {
            return (pitch, 0.0);
        };
        let (look, ..) = self.sample(now + Duration::from_millis(1000));
        let (dx, dy) = (look[0] - pos[0], look[1] - pos[1]);
        let dlen = (dx * dx + dy * dy).sqrt();
        if dlen <= 2.384e-7 {
            return (pitch, 0.0);
        }
        let (tx, ty) = (f.cos(), f.sin());
        let (ux, uy) = (dx / dlen, dy / dlen);
        let dot = (tx * ux + ty * uy).clamp(-1.0, 1.0);
        let cross = tx * uy - ty * ux;
        let theta = if cross < 0.0 { -dot.acos() } else { dot.acos() } * 2.0;
        let bank = if theta < -std::f32::consts::FRAC_PI_2 {
            -std::f32::consts::PI
        } else if theta >= std::f32::consts::FRAC_PI_2 {
            std::f32::consts::PI
        } else {
            theta
        };
        (pitch, bank)
    }
}

/// Uniform Catmull-Rom position and `d/du` tangent on `pts[i] → pts[i+1]` at `u ∈ [0,1]`, the
/// ends duplicated as the 1.12 client stores them (`0x601d30`, `0x601e66`), so the curve passes
/// every waypoint.
fn catmull_rom(pts: &[[f32; 3]], i: usize, u: f32) -> ([f32; 3], [f32; 3]) {
    let p0 = pts[i.saturating_sub(1)];
    let p1 = pts[i];
    let p2 = pts[i + 1];
    let p3 = pts[(i + 2).min(pts.len() - 1)];
    let (u2, u3) = (u * u, u * u * u);
    let mut pos = [0.0f32; 3];
    let mut dir = [0.0f32; 3];
    for a in 0..3 {
        // The standard uniform C-R basis: p(u) = ½·(2P₁ + (−P₀+P₂)u + (2P₀−5P₁+4P₂−P₃)u² +
        // (−P₀+3P₁−3P₂+P₃)u³); dir is its analytic d/du.
        let c1 = p2[a] - p0[a];
        let c2 = 2.0 * p0[a] - 5.0 * p1[a] + 4.0 * p2[a] - p3[a];
        let c3 = -p0[a] + 3.0 * p1[a] - 3.0 * p2[a] + p3[a];
        pos[a] = 0.5 * (2.0 * p1[a] + c1 * u + c2 * u2 + c3 * u3);
        dir[a] = 0.5 * (c1 + 2.0 * c2 * u + 3.0 * c3 * u2);
    }
    (pos, dir)
}

/// The [`Spline`] for one `SMSG_MONSTER_MOVE` along `path` (`[start, …, endpoint]`), or `None`
/// for a stop, a zero duration or a path under two points.
pub(in crate::net) fn monster_move_spline(
    path: Vec<[f32; 3]>,
    spline_id: u32,
    stop: bool,
    duration_ms: u32,
    flying: bool,
    run_mode: bool,
    deck: Option<u64>,
) -> Option<Spline> {
    if stop || duration_ms == 0 || path.len() < 2 {
        return None;
    }
    Some(Spline {
        points: path,
        start: Instant::now(),
        duration: Duration::from_millis(u64::from(duration_ms)),
        id: spline_id,
        // `grounded` also picks the sampler (lerp or Catmull-Rom); the clamp skips a deck path on
        // `deck`.
        grounded: !flying,
        run_mode,
        deck,
    })
}

/// The [`Spline`] a unit is already riding in its create block, back-dated by
/// [`CreateSpline::time_passed_ms`]; `None` for a degenerate path, a zero duration, a finished
/// ride, or anything under `WOW_CREATE_SPLINE=off`.
pub(in crate::net) fn create_spline(spline: CreateSpline) -> Option<Spline> {
    if !create_spline_enabled()
        || spline.duration_ms == 0
        || spline.path.len() < 2
        || spline.time_passed_ms >= spline.duration_ms
    {
        return None;
    }
    let passed = Duration::from_millis(u64::from(spline.time_passed_ms));
    Some(Spline {
        points: spline.path,
        // `checked_sub`: a spline older than the monotonic clock's origin would panic on `-`.
        start: Instant::now()
            .checked_sub(passed)
            .unwrap_or_else(Instant::now),
        duration: Duration::from_millis(u64::from(spline.duration_ms)),
        id: spline.id,
        grounded: !spline.flying,
        run_mode: spline.run_mode,
        // The create block's spline tail names no transport; its path is in world coordinates.
        deck: None,
    })
}

/// One `csp` trace line per create block with a live spline, in either `WOW_CREATE_SPLINE` leg;
/// `left` is the yards still ahead at create time.
pub(in crate::net) fn trace_create_spline(guid: u64, spline: Option<&CreateSpline>) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    let Some(s) = spline else { return };
    let length: f32 = s
        .path
        .windows(2)
        .map(|w| {
            let (dx, dy, dz) = (w[1][0] - w[0][0], w[1][1] - w[0][1], w[1][2] - w[0][2]);
            (dx * dx + dy * dy + dz * dz).sqrt()
        })
        .sum();
    let ridden = s.time_passed_ms as f32 / s.duration_ms.max(1) as f32;
    let left = length * (1.0 - ridden).clamp(0.0, 1.0);
    benilla_assets::trace::line(
        "csp",
        &format!(
            "{guid:#x} nodes={} len={length:.2} left={left:.2} t={}/{} world{}{}{}",
            s.path.len(),
            s.time_passed_ms,
            s.duration_ms,
            if s.flying { " flying" } else { "" },
            if s.cyclic { " cyclic" } else { "" },
            if create_spline_enabled() {
                ""
            } else {
                " DROPPED(WOW_CREATE_SPLINE=off)"
            },
        ),
    );
}

/// One `mmv` trace line per `SMSG_MONSTER_MOVE`: the jump to the new path's start, `xy` the
/// desync and `z` only our terrain against the wire's.
pub(in crate::net) fn trace_move_snap(
    guid: u64,
    from: Option<[f32; 3]>,
    start: [f32; 3],
    stop: bool,
    duration_ms: u32,
) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    let snap = from.map_or("xy=? z=?".to_string(), |f| {
        let (dx, dy, dz) = (start[0] - f[0], start[1] - f[1], start[2] - f[2]);
        format!("xy={:.2} z={dz:+.2}", (dx * dx + dy * dy).sqrt())
    });
    benilla_assets::trace::line(
        "mmv",
        &format!(
            "{guid:#x} {snap} start=[{:.2},{:.2},{:.2}] dur={duration_ms}{}",
            start[0],
            start[1],
            start[2],
            if stop { " stop" } else { "" },
        ),
    );
}

/// `WOW_CREATE_SPLINE=off` drops every create-block spline, freezing a unit at first sight until
/// its next `SMSG_MONSTER_MOVE`: an A/B lever for [`create_spline`].
fn create_spline_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        !std::env::var("WOW_CREATE_SPLINE").is_ok_and(|v| matches!(v.as_str(), "off" | "0"))
    })
}

/// Writes every path-walking entity's translation and rotation from its [`Spline`] each frame;
/// scale is left alone.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(in crate::net) fn sample_splines(
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &Spline,
        &mut Transform,
        Has<CreatureSwimming>,
        // For the `spl` trace only; the sampler never gates on it.
        Option<&super::super::Guid>,
        // A deck path's destination; `transport::compose_riders` carries it to the world later.
        Option<&mut crate::transport::TransportRider>,
    )>,
    mut trace_next: Local<f32>,
    time: Res<Time>,
) {
    let now = Instant::now();
    // The `spl` trace ticks once a second for the whole population, so its lines share an instant.
    let tracing = benilla_assets::trace::enabled_for("spl");
    let tick = tracing && time.elapsed_secs() >= *trace_next;
    if tick {
        *trace_next = time.elapsed_secs() + SPL_TRACE_SECS;
    }
    for (entity, spline, mut t, swimming, guid, rider) in &mut q {
        let (wow_pos, facing, pitch) = spline.sample(now);
        // A deck path's sample is the rider's transport-local pose, composed later this frame.
        if let Some(deck) = spline.deck {
            // Written as a world pose it would sit near the map origin, so a missing or mismatched
            // rider freezes the unit for the frame instead.
            if let Some(mut rider) = rider.filter(|r| r.transport_guid == deck) {
                rider.local_pos = wow_pos;
                if let Some(f) = facing {
                    rider.local_orientation = f;
                }
            }
            if now.saturating_duration_since(spline.start) >= spline.duration {
                commands.entity(entity).remove::<Spline>();
            }
            continue;
        }
        // The transform as the last frame left it: a `was=` that is not last frame's sample means
        // another system writes this transform.
        let was = tick.then(|| bevy_to_wow(t.translation));
        t.translation = wow_to_bevy(wow_pos);
        if let Some(f) = facing {
            // A flying spline takes the full attitude, a swimming unit its travel pitch
            // (`0x60a110`), a walker none; roll composes innermost, about the travel axis.
            let (pitch, bank) = if !spline.grounded {
                spline.flight_attitude(now)
            } else if swimming {
                (pitch, 0.0)
            } else {
                (0.0, 0.0)
            };
            t.rotation = if pitch != 0.0 || bank != 0.0 {
                Quat::from_rotation_y(f)
                    * Quat::from_rotation_x(pitch)
                    * Quat::from_rotation_z(bank)
            } else {
                Quat::from_rotation_y(f)
            };
        }
        if let Some(was) = was {
            trace_ride(guid.map_or(0, |g| g.0), spline, wow_pos, was, now);
        }
        // A finished path drops its spline: `creature_anim` reads the spline's presence as moving.
        if now.saturating_duration_since(spline.start) >= spline.duration {
            if tracing {
                benilla_assets::trace::line(
                    "spl",
                    &format!("{:#x} DONE — spline dropped", guid.map_or(0, |g| g.0)),
                );
            }
            commands.entity(entity).remove::<Spline>();
        }
    }
}

/// How often the `spl` tag samples a live ride; one second keeps a busy zone off the trace mutex.
const SPL_TRACE_SECS: f32 = 1.0;

/// One `spl` trace line per live [`Spline`] per [`SPL_TRACE_SECS`]: what is drawn, and in `was=`
/// how far the transform had moved from it coming in.
fn trace_ride(guid: u64, spline: &Spline, pos: [f32; 3], was: [f32; 3], now: Instant) {
    let elapsed = now.saturating_duration_since(spline.start).as_secs_f32();
    let (dx, dy, dz) = (pos[0] - was[0], pos[1] - was[1], pos[2] - was[2]);
    let drift = (dx * dx + dy * dy + dz * dz).sqrt();
    benilla_assets::trace::line(
        "spl",
        &format!(
            "{guid:#x} t={elapsed:.1}/{:.1} pos=[{:.2},{:.2},{:.2}] was={drift:.3} spd={:.2} nodes={} {}",
            spline.duration.as_secs_f32(),
            pos[0],
            pos[1],
            pos[2],
            spline.speed(),
            spline.points.len(),
            if spline.grounded { "ground" } else { "flying" },
        ),
    );
}

/// Whether benilla derives this body's Z and swim state, for both [`mark_swimming_creatures`] and
/// [`ground_clamp_creatures`]. The reference never asks the kind: `0x6187a0` links any unit a
/// `SMSG_MONSTER_MOVE` names (`0x619ca0`), and the per-frame vertical-zero gate
/// (`0x616cec`-`0x616d03`) reads only the mover's flags. So: every `Unit`; a `Player` a server
/// spline has moved, until the relay takes it back (`derived_before` is [`GroundClamped`]'s
/// presence); never the body we steer ([`crate::net::Embodied`]); nothing else. A relayed player's
/// Z comes grounded by its own client, and a down-ray would flatten its dead-reckoned jump arc.
pub(crate) fn ground_derived(
    kind: EntityKind,
    splined: bool,
    derived_before: bool,
    embodied: bool,
    relayed: bool,
) -> bool {
    if embodied {
        return false;
    }
    match kind {
        EntityKind::Unit => true,
        EntityKind::Player => !relayed && (splined || derived_before),
        _ => false,
    }
}

/// How far above the seat an idle unit's settle probe starts (yd): the swept-prism TOI
/// (`0x632830`) still counts a face up to `1/36` yd (`[0x7ff9c8]`) behind the probe as a hit.
const IDLE_UP_BAND: f32 = 1.0 / 36.0;
/// How far below the seat the settle probe reaches (yd). The reference's settle reaches
/// `d·1.849 + 1/36` and a body beyond it falls; an idle unit here has no fall, so a miss leaves it
/// at its seat, which is also the airborne gate.
const GROUND_CLAMP_DOWN: f32 = 4.0;

/// The Y a grounded mover ends its frame at (`0x634040`'s walk resolve), shared by the clamp and
/// [`crate::player::server_ride`]; a probe miss returns `seat_y`.
///
/// - `water_floor`: only under `MOVEFLAG_WATERWALKING`; the reference ORs the liquid layers into
///   one walk trace (`0x63162e`), which the `max` reproduces.
/// - `hover`: the reference ends at `z_before_snap − max(L − 1.0, 0)` (`0x636e81`-`0x636ea9`),
///   here `(y + 1.0).min(seat_y)`.
pub(crate) fn grounded_y(
    seat_y: f32,
    floor: Option<f32>,
    water_floor: Option<f32>,
    hover: bool,
) -> f32 {
    let mut y = floor.unwrap_or(seat_y);
    if let Some(w) = water_floor {
        y = y.max(w);
    }
    if hover {
        y = (y + crate::player::HOVER_HEIGHT).min(seat_y);
    }
    y
}

/// Grounds every [`ground_derived`] body on benilla's world: the 1.12 client zeroes a ground
/// spline's Δz and takes Z from the world trace, and an idle unit reads grounded too, by an
/// untraced path. Every probe starts at the body, measured from the seat (the Z the unit's
/// position owner last wrote), never from the clamp's last answer, so a late floor takes the unit
/// back. Skipped: flying and deck paths, and a swimming unit, whose wire Z is its depth (vmangos
/// paths it in 3D without the flying flag, `MoveSplineInit.cpp:191`; the reference keeps Δz under
/// SWIMMING, `[CMovement+0x40] & 0x200000` at `0x616cfa`).
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(in crate::net) fn ground_clamp_creatures(
    world: benilla_world::collision::WorldCollision,
    // The liquid query: a water-walker's floor is the surface.
    points: benilla_world::world_point::WorldPoint,
    epoch: Res<benilla_world::collision::ColliderEpoch>,
    // The body a walker's step sweeps: the shared player capsule. The reference sweeps each
    // unit's own `CreatureModelData` radius and height (`[CMovement+0xb0]`/`+0xb4`); per-unit
    // extents are not built.
    capsule: Res<crate::player::PlayerCapsule>,
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &NetEntity,
        Option<&Spline>,
        &mut Transform,
        Option<&mut GroundClamped>,
        Has<CreatureSwimming>,
        // HOVER and WATERWALKING both move the answer.
        Option<&crate::net::UnitMoveModes>,
        // [`ground_derived`]'s other Z authorities: the controller and the relay's dead-reckoning.
        Has<crate::net::Embodied>,
        Has<crate::net::RemoteMotion>,
    )>,
) {
    let cost = clamp_cost_enabled();
    let legacy = clamp_seat_disabled();
    let t0 = cost.then(std::time::Instant::now);
    let (mut visited, mut skipped, mut held, mut cast, mut swept, mut hit_n, mut moved) =
        (0u32, 0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    // Walker-arm counters: `noflr` counts frames whose swept step found no floor (nonzero while the
    // world streams, and harmless: the frame holds the server's pose); `ratchet` counts such frames
    // left below the seat and must read 0; `deepest` and `worst` name the offender.
    let (mut noflr, mut ratchet, mut deepest) = (0u32, 0u32, f32::NEG_INFINITY);
    let mut worst: Option<(u32, [f32; 2], f32)> = None;
    // What re-armed a cast: the unit's seat moved (`reseat`) or the colliders changed under it
    // (`armed`); a nonzero steady-state `armed` means the collider set is churning.
    let (mut reseat, mut armed) = (0u32, 0u32);

    for (entity, net, spline, mut t, mut clamped, swimming, modes, embodied, relayed) in &mut q {
        visited += 1;
        if !ground_derived(
            net.kind,
            spline.is_some(),
            clamped.is_some(),
            embodied,
            relayed,
        ) {
            skipped += 1;
            continue; // somebody else owns this body's Z
        }
        if spline.is_some_and(|s| !s.grounded) {
            skipped += 1;
            continue; // a flying path is authoritative on Z
        }
        if spline.is_some_and(|s| s.deck.is_some()) {
            skipped += 1;
            continue; // a deck path: no terrain under a deck, and the wire Z is deck-local
        }
        if swimming {
            skipped += 1;
            continue; // in-liquid: the wire Z is the creature's swim depth
        }
        let xz = [t.translation.x, t.translation.z];
        // The granted modes change the ray's answer, so they join the cache gate below.
        let granted = modes.copied().unwrap_or_default();
        // A Y other than the one our last clamp left was written by the unit's position owner and
        // is the new seat; with no memo, the spawn pose is the seat.
        let seat_y = match clamped.as_deref() {
            _ if legacy => t.translation.y, // `WOW_CLAMP_SEAT=off`: from our own last answer
            Some(c) if c.y_written == t.translation.y => c.seat_y,
            _ => t.translation.y,
        };
        // The cast gate: same XZ, same collider epoch, same modes, and our own answer still the Y
        // standing here, so the ray cannot answer differently. Testing the standing Y rather than
        // the seat also re-grounds a re-sent identical wire pose. A miss never caches.
        if let Some(c) = clamped.as_deref() {
            let same_question = if legacy {
                c.y_written == t.translation.y
            } else {
                c.y_written == t.translation.y && c.epoch == epoch.get() && c.modes == granted
            };
            if c.hit && c.xz == xz && same_question {
                held += 1;
                continue;
            }
            if cost && !legacy {
                if c.xz == xz && c.seat_y == seat_y {
                    armed += 1; // the world changed under a unit that didn't move
                } else {
                    reseat += 1; // the unit moved, or was moved
                }
            }
        }
        // Only for a unit not swimming: the reference takes this arm with the swim bit clear
        // (`0x631617`). Deviation: its rate-limited hover climb (`0x636fa1`, 7 yd/s) is not
        // reproduced, because the cache gate needs a pure function; it converges to this answer.
        let water = granted
            .water_walking()
            .then(|| {
                let wow = bevy_to_wow(t.translation);
                let who = benilla_world::world_point::Subject::Unit(entity);
                points
                    .liquid_at(who, wow)
                    .map(|l| t.translation.y + (l.surface_z - wow[2]))
            })
            .flatten();
        let hover = granted.hovering();
        // A continuing walker takes the swept step from last frame's pose with Δz = 0 (step-up
        // `H` 2.0) and keeps only Y, so it rides a hill its chord cuts under; an idle unit or a new
        // path settles down from the seat (`0x636dcd`-`0x636e45`, `d·1.849 + 1/36`).
        let path = spline.map(|s| (s.id, s.start));
        let continuing = clamped
            .as_deref()
            .filter(|c| path.is_some() && c.path == path)
            .map(|c| (c.xz, c.y_written));
        cast += 1;
        let (y, hit_ground) = if let Some((pxz, py)) = continuing {
            swept += 1;
            let half_h = Vec3::Y * (crate::player::CAPSULE_HEIGHT * 0.5);
            let from = Vec3::new(pxz[0], py, pxz[1]) + half_h;
            // The frame's horizontal displacement as a one-second velocity: `grounded_step` travels
            // `speed · dt`, and the reference's walk step takes a distance and a direction
            // (`0x6367b0`).
            let delta = Vec3::new(xz[0] - pxz[0], 0.0, xz[1] - pxz[1]);
            let g = crate::player::mover::grounded_step(
                &world,
                &capsule.0,
                from,
                delta,
                Duration::from_secs(1),
                crate::player::mover::Support {
                    rise: crate::player::CREATURE_STEP_UP_HEIGHT,
                    offset: if hover {
                        crate::player::HOVER_HEIGHT
                    } else {
                        0.0
                    },
                    water,
                    // Per-frame state the memo does not keep; the path's next sample corrects a
                    // miss on a steep face.
                    steep: false,
                },
            );
            // A sweep that finds no floor holds the server's pose, as `grounded_y` does: the step's
            // no-floor drop would ratchet below a surface the one-sided cast cannot find again.
            let y = if g.unsupported.is_some() {
                seat_y
            } else {
                g.center.y - half_h.y
            };
            if benilla_assets::trace::enabled_for("clmp")
                && clamp_trace_display().is_some_and(|d| net.display_id == Some(d))
            {
                let z_of = |y: f32| bevy_to_wow(Vec3::new(0.0, y, 0.0))[2];
                benilla_assets::trace::line(
                    "clmp",
                    &format!(
                        "display={} walk from_z={:.3} seat_z={:.3} d={:.3} ground={} z={:.3}",
                        net.display_id.unwrap_or(0),
                        z_of(py),
                        z_of(seat_y),
                        delta.length(),
                        g.ground.is_some() as u8,
                        z_of(y),
                    ),
                );
            }
            if g.unsupported.is_some() {
                noflr += 1;
                if seat_y - y > 1.0e-4 {
                    ratchet += 1;
                    if seat_y - y > deepest {
                        deepest = seat_y - y;
                        worst = Some((net.display_id.unwrap_or(0), xz, y));
                    }
                }
            }
            (y, g.ground.is_some())
        } else {
            let origin = Vec3::new(t.translation.x, seat_y + IDLE_UP_BAND, t.translation.z);
            let reach = IDLE_UP_BAND + GROUND_CLAMP_DOWN;
            // One-sided, as the player mover grounds: a face wound away is no floor.
            let hit = world.ray_body(origin, Dir3::NEG_Y, reach);
            let floor = hit.as_ref().map(|h| origin.y - h.distance);
            let y = grounded_y(seat_y, floor, water, hover);
            // The `clmp` trace for one display (`WOW_CLAMP_TRACE=<display id>`): per cast, the
            // seat, the ray origin, the hit and the Z written, in WoW coordinates.
            if benilla_assets::trace::enabled_for("clmp")
                && clamp_trace_display().is_some_and(|d| net.display_id == Some(d))
            {
                let seat = bevy_to_wow(Vec3::new(t.translation.x, seat_y, t.translation.z));
                let z_of = |y: f32| bevy_to_wow(Vec3::new(0.0, y, 0.0))[2];
                let hit_s = hit.as_ref().map_or("miss".to_string(), |h| {
                    format!("hit d={:.3} n.y={:+.3}", h.distance, h.normal.y)
                });
                benilla_assets::trace::line(
                    "clmp",
                    &format!(
                        "display={} idle seat=({:.2},{:.2},{:.3}) origin_z={:.3} reach={:.2} {hit_s} floor_z={} z={:.3}",
                        net.display_id.unwrap_or(0),
                        seat[0],
                        seat[1],
                        seat[2],
                        z_of(origin.y),
                        reach,
                        floor.map_or("-".to_string(), |f| format!("{:.3}", z_of(f))),
                        z_of(y),
                    ),
                );
            }
            (y, hit.is_some())
        };
        if hit_ground {
            hit_n += 1;
        }
        // Exact equality on purpose: Bevy's change detection fires on any write, and a real
        // sub-epsilon shift must still land.
        if y != t.translation.y {
            moved += 1;
            t.translation.y = y;
        }
        let state = GroundClamped {
            xz,
            seat_y,
            y_written: t.translation.y,
            hit: hit_ground,
            epoch: epoch.get(),
            modes: granted,
            path,
        };
        match clamped.as_deref_mut() {
            Some(c) => *c = state,
            None => {
                commands.entity(entity).insert(state);
            }
        }
    }

    if let Some(t0) = t0 {
        // `WOW_CLAMP_COST=1`: the sweep's cost per frame. `cast` against `visited` is what the
        // cast gate saves, `moved` against `hit` how often a write changes Y; quote `ms`.
        eprintln!(
            "[clamp-cost] visited={visited} skipped={skipped} held={held} cast={cast} swept={swept} reseat={reseat} armed={armed} hit={hit_n} moved={moved} noflr={noflr} ratchet={ratchet} deepest={} worst={} ms={:.3}",
            if deepest.is_finite() { format!("{deepest:+.2}") } else { "-".to_string() },
            worst.map_or_else(
                || "-".to_string(),
                |(d, xz, y)| format!("display={d}@({:.0},{:.0}) y={y:.2}", xz[0], xz[1])
            ),
            t0.elapsed().as_secs_f32() * 1000.0
        );
    }
}

/// The one display the `clmp` trace follows (`WOW_CLAMP_TRACE=<display id>`); `None` writes
/// nothing.
fn clamp_trace_display() -> Option<u32> {
    static ID: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *ID.get_or_init(|| std::env::var("WOW_CLAMP_TRACE").ok()?.trim().parse().ok())
}

/// Whether the ground-clamp meter is armed (`WOW_CLAMP_COST`). Read once, then a relaxed bool.
fn clamp_cost_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_CLAMP_COST").is_some())
}

/// `WOW_CLAMP_SEAT=off` measures from the clamp's own last answer and ignores the collider epoch:
/// the lever that reproduces a unit stuck under a late-attached floor on the same build.
fn clamp_seat_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        std::env::var("WOW_CLAMP_SEAT").is_ok_and(|v| matches!(v.as_str(), "off" | "0"))
    })
}

/// A stamped unit re-enters [`mark_swimming_creatures`] only when its `Transform` or its
/// descriptors change (a `UNIT_FIELD_FLAGS` delta must re-ask); an unstamped one every frame.
type SwimMarkGate = Or<(
    Changed<Transform>,
    Changed<ObjectStore>,
    Without<SwimEvaluated>,
)>;

type SwimMarkQuery = (
    Entity,
    &'static NetEntity,
    &'static Transform,
    Option<&'static CollisionHeight>,
    Option<&'static ObjectStore>,
    Has<CreatureSwimming>,
    Has<SwimEvaluated>,
    // [`ground_derived`]'s other inputs: the mark and the clamp must walk the same population.
    Has<Spline>,
    Has<GroundClamped>,
    Has<crate::net::Embodied>,
    Has<crate::net::RemoteMotion>,
);

/// The stamp [`mark_swimming_creatures`] leaves once a unit's room claim settles; 1.12 water is
/// static, so it holds until the unit moves or the `ColliderEpoch` bumps.
#[derive(Component)]
pub(in crate::net) struct SwimEvaluated;

/// The exit band (yd) below the swim boundary: the player boundary's 1/36-yd hysteresis
/// (`0x7ff9d0`), an absolute distance, not scaled with collision height.
const CREATURE_SWIM_EXIT_BAND: f32 = 1.0 / 36.0;

/// A server-moved body ([`ground_derived`]) past the swim boundary, which no packet carries for
/// one. The 1.12 client runs its depth decision `0x6030c0` on every linked CMovement (the frame
/// walk `0x616800 → 0x615b10 → 0x616620`, the move apply `0x618c30`) and sets SWIMMING itself on
/// a unit it does not steer (`0x60df70 → 0x61a130 → 0x61a230 → 0x7c6e50`).
///
/// The decision reads `UNIT_FIELD_FLAGS` on both legs ([`crate::player::may_swim`]): a unit with
/// none of the three bits walks the lakebed, where vmangos paths it (`Object.cpp:2007`). Depth
/// uses the unit's own collision height; the point the reference measures it from is untraced.
/// The levitating bail (`0x400`) is not read: a spline-walked unit has no live flag word, and
/// vmangos sets the bit on a creature only from a script (`boss_onyxia.cpp:679`). The splash
/// (`sound::water`) sits outside the flag gate, as `0x60314a` does.
#[derive(Component)]
pub(crate) struct CreatureSwimming;

/// The last ground clamp this unit took: its seat and the cast gate's inputs, each compared by bit.
#[derive(Component, Clone, Copy)]
pub(crate) struct GroundClamped {
    /// Where the last cast stood.
    xz: [f32; 2],
    /// The Y the answer came from: the pose the unit's position owner last wrote, never the
    /// clamp's own output.
    pub(crate) seat_y: f32,
    /// The Y standing after that cast; a different Y here is an external write, which re-seats.
    pub(crate) y_written: f32,
    /// Only a hit caches; a miss keeps casting until the collider streams in.
    pub(crate) hit: bool,
    /// The collider-set stamp the answer was computed against.
    epoch: u64,
    /// The granted modes the answer was computed under (HOVER and WATERWALKING change it).
    modes: crate::net::UnitMoveModes,
    /// The walker's `(spline id, start)` this continues; a new path re-seats, as the reference
    /// re-bases on every inbound movement packet (`0x7c6420`).
    path: Option<(u32, Instant)>,
}

impl GroundClamped {
    /// Whether the walker's swept step produced this answer rather than the idle settle.
    pub(crate) fn walking(&self) -> bool {
        self.path.is_some()
    }
}

/// The reference's `0x6030c0` decision for one creature:
///
/// ```text
/// enter iff  flags ∧ depth >  0.75·h              (0x603106 test ah,0x41 + jne: ZF, strict)
/// stay  iff  flags ∧ depth >= 0.75·h − 1/36       (0x6031c5 test ah,5 + jnp: parity, inclusive)
/// ```
///
/// A false flag term forces a stop at any depth (`0x6031eb → 0x60dff0`), not only a refused start.
fn creature_swim_state(marked: bool, depth: f32, boundary: f32, unit_flags: u32) -> bool {
    if !crate::player::may_swim(unit_flags) {
        return false; // both legs: without a permit a unit can neither start nor stay
    }
    if marked {
        depth >= boundary - CREATURE_SWIM_EXIT_BAND
    } else {
        depth > boundary
    }
}

/// Maintains [`CreatureSwimming`] on every [`ground_derived`] body from the water over its feet.
/// Chained before [`ground_clamp_creatures`] so a fresh mark exempts the clamp the same frame.
pub(in crate::net) fn mark_swimming_creatures(
    mut commands: Commands,
    units: Query<SwimMarkQuery, SwimMarkGate>,
    stamped: Query<Entity, With<SwimEvaluated>>,
    world: benilla_world::world_point::WorldPoint,
    epoch: Res<benilla_world::collision::ColliderEpoch>,
    mut last_epoch: Local<Option<u64>>,
) {
    // A collider epoch change (liquid streams with the colliders) drops every stamp.
    let now = epoch.get();
    if last_epoch.replace(now) != Some(now) {
        for e in &stamped {
            commands.entity(e).remove::<SwimEvaluated>();
        }
    }
    for (e, net, t, collision, store, marked, evaluated, splined, clamped, embodied, relayed) in
        &units
    {
        if !ground_derived(net.kind, splined, clamped, embodied, relayed) {
            continue; // GameObjects don't swim; a relayed player's flag arrives on the wire
        }
        // The unit's own room decides whose liquid answers (an interior's, not the ADT water over
        // it). An unsettled claim may not enter swim, but a stale mark may always clear.
        let who = benilla_world::world_point::Subject::Unit(e);
        if !marked && !world.room_settled(who) {
            continue; // unstamped, so retried until the claim lands
        }
        if !evaluated {
            commands.entity(e).insert(SwimEvaluated);
        }
        let wow = bevy_to_wow(t.translation);
        let depth = world
            .water_surface_at(who, wow)
            .map_or(f32::MIN, |s| s - wow[2]);
        let boundary = swim_enter_depth(collision.copied().unwrap_or_default().0);
        // Flags read 0 until the descriptor block lands; `Changed<ObjectStore>` re-asks then.
        let flags = store.map_or(0, |s| s.0.unit_flags());
        let swimming = creature_swim_state(marked, depth, boundary, flags);
        if swimming != marked {
            if swimming {
                commands.entity(e).insert(CreatureSwimming);
            } else {
                commands.entity(e).remove::<CreatureSwimming>();
            }
        }
    }
}

/// A unit created inside a building whose floor collider has not attached yet, and what happens
/// to it when the floor lands: the seat lets the probe reach the floor, the epoch makes it re-ask.
#[cfg(test)]
mod under_floor {
    use avian3d::prelude::*;
    use benilla_protocol::EntityKind;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;

    use super::{ground_clamp_creatures, GroundClamped};
    use crate::net::NetEntity;
    use benilla_world::collision::ColliderEpoch;

    /// Auberdine's geometry: terrain, the building's floor 2.08 above it, and the server's Z for
    /// the NPCs inside 0.08 above that.
    const TERRAIN_Y: f32 = 6.98;
    const FLOOR_Y: f32 = 9.06;
    const WIRE_Y: f32 = 9.14;

    /// A 10×10 up-wound quad at `y`: a floor the one-sided down-ray stands on.
    fn floor_at(app: &mut App, y: f32) -> Entity {
        let verts = vec![
            Vec3::new(-5.0, y, -5.0),
            Vec3::new(5.0, y, -5.0),
            Vec3::new(5.0, y, 5.0),
            Vec3::new(-5.0, y, 5.0),
        ];
        app.world_mut()
            .spawn((
                RigidBody::Static,
                Collider::trimesh(verts, vec![[0u32, 2, 1], [0, 3, 2]]),
                Transform::default(),
            ))
            .id()
    }

    /// A world with the terrain in and the building's floor still building, plus one idle NPC
    /// standing at the Z the server sent for it.
    fn half_arrived_world() -> (App, Entity) {
        let mut app = App::new();
        // `WorldCollision` needs the mover's trace exclusions, which the world plugins add.
        app.init_resource::<benilla_world::collision::MoverTraceExclusions>();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>().init_resource::<ColliderEpoch>();
        // The liquid and room facade, seeded empty: no liquid anywhere.
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        // The body a walker's swept step sweeps.
        app.insert_resource(crate::player::PlayerCapsule(Collider::capsule(
            crate::player::CAPSULE_RADIUS,
            crate::player::CAPSULE_HEIGHT - 2.0 * crate::player::CAPSULE_RADIUS,
        )));
        // `update()` never runs plugin `finish()`, where avian adds its diagnostics resources.
        app.finish();
        app.cleanup();
        floor_at(&mut app, TERRAIN_Y);
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Transform::from_xyz(0.0, WIRE_Y, 0.0),
            ))
            .id();
        app.update(); // seats Position/Rotation and the collider trees
        (app, npc)
    }

    fn clamp(app: &mut App) {
        app.world_mut()
            .run_system_once(ground_clamp_creatures)
            .expect("run the clamp");
    }

    fn y_of(app: &App, e: Entity) -> f32 {
        app.world().get::<Transform>(e).unwrap().translation.y
    }

    #[test]
    fn a_unit_sunk_by_a_late_floor_stands_back_up_when_it_lands() {
        let (mut app, npc) = half_arrived_world();

        // Only the terrain exists yet, so the clamp grounds onto it.
        clamp(&mut app);
        assert_eq!(
            y_of(&app, npc),
            TERRAIN_Y,
            "with only terrain built, the clamp grounds onto terrain"
        );
        assert_eq!(
            app.world().get::<GroundClamped>(npc).unwrap().seat_y,
            WIRE_Y,
            "the seat is the server's Z, not the answer the clamp just wrote"
        );

        // Nothing moves the NPC; the floor collider attaches and bumps the epoch.
        floor_at(&mut app, FLOOR_Y);
        app.world_mut().resource_mut::<ColliderEpoch>().bump();
        app.update();

        // It comes back up on its own.
        clamp(&mut app);
        assert_eq!(
            y_of(&app, npc),
            FLOOR_Y,
            "the floor arrived under an NPC that never moved; it belongs on it"
        );
    }

    /// A hovering unit rests a yard over its floor and never climbs: the reference ends at
    /// `z_before_snap − max(L − 1.0, 0)` (`0x636e81`-`0x636ea9`).
    #[test]
    fn a_hovering_creature_floats_a_yard_and_never_climbs() {
        let (mut app, npc) = half_arrived_world();
        clamp(&mut app);
        assert_eq!(y_of(&app, npc), TERRAIN_Y, "the plain snap, for reference");

        // The grant alone must re-arm the cast.
        app.world_mut()
            .entity_mut(npc)
            .insert(crate::net::UnitMoveModes(
                crate::creature_anim::move_flags::HOVER,
            ));
        clamp(&mut app);
        assert_eq!(
            y_of(&app, npc),
            TERRAIN_Y + crate::player::HOVER_HEIGHT,
            "a yard clear of the floor — and the grant alone re-armed the cast"
        );

        // Seated under a yard over its floor, it stays at its seat.
        let near = TERRAIN_Y + 0.25;
        app.world_mut()
            .entity_mut(npc)
            .get_mut::<Transform>()
            .unwrap()
            .translation
            .y = near;
        clamp(&mut app);
        assert_eq!(
            y_of(&app, npc),
            near,
            "inside the clearance the reference subtracts nothing — it must not climb"
        );
    }

    /// A re-sent identical pose (same seat) is re-grounded, not held at the wire Z.
    #[test]
    fn a_pose_re_sent_unchanged_is_re_grounded_not_held() {
        let (mut app, npc) = half_arrived_world();
        clamp(&mut app);
        assert_eq!(y_of(&app, npc), TERRAIN_Y);

        // The wire restates its pose: the same seat, but no longer the Y the clamp left.
        app.world_mut()
            .entity_mut(npc)
            .get_mut::<Transform>()
            .unwrap()
            .translation
            .y = WIRE_Y;
        clamp(&mut app);
        assert_eq!(
            y_of(&app, npc),
            TERRAIN_Y,
            "the question was unchanged, but the answer was no longer applied"
        );
    }

    /// With no liquid, water walking changes nothing: the `max` must not meet a sentinel surface.
    #[test]
    fn water_walking_with_no_liquid_changes_nothing() {
        let (mut app, npc) = half_arrived_world();
        app.world_mut()
            .entity_mut(npc)
            .insert(crate::net::UnitMoveModes(
                crate::creature_anim::move_flags::WATER_WALKING,
            ));
        clamp(&mut app);
        assert_eq!(
            y_of(&app, npc),
            TERRAIN_Y,
            "no liquid over this NPC: the terrain is still the only floor"
        );
    }

    #[test]
    fn a_settled_unit_is_not_re_cast_while_the_world_holds_still() {
        // The gate holds an answer whose inputs are unchanged.
        let (mut app, npc) = half_arrived_world();
        clamp(&mut app);
        let before = *app.world().get::<GroundClamped>(npc).unwrap();
        clamp(&mut app);
        let after = *app.world().get::<GroundClamped>(npc).unwrap();
        assert_eq!(
            (before.seat_y, before.y_written, before.epoch),
            (after.seat_y, after.y_written, after.epoch),
            "a held unit's memo is untouched — nothing re-asked the ground"
        );
    }

    #[test]
    fn a_unit_with_no_ground_in_reach_sits_where_the_server_put_it() {
        // The whole reach below the seat is empty air: the miss leaves the unit at its seat.
        let (mut app, npc) = half_arrived_world();
        app.world_mut()
            .get_mut::<Transform>(npc)
            .unwrap()
            .translation
            .y = TERRAIN_Y + 40.0;
        clamp(&mut app);
        assert_eq!(
            y_of(&app, npc),
            TERRAIN_Y + 40.0,
            "no surface in reach ⇒ the wire's Z stands"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spline `frac` of the way through its ride: 10 s duration, started `frac·10 s` ago.
    fn spline_at(points: Vec<[f32; 3]>, grounded: bool, frac: f32) -> (Spline, Instant) {
        let spline = Spline {
            deck: None,
            points,
            start: Instant::now() - Duration::from_secs_f32(10.0 * frac),
            duration: Duration::from_secs(10),
            id: 7,
            grounded,
            run_mode: true,
        };
        let now = Instant::now();
        (spline, now)
    }

    /// A ground path is a straight lerp: mid first leg of an L it sits on the chord, facing along.
    #[test]
    fn ground_path_samples_linearly() {
        let pts = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]];
        let (s, now) = spline_at(pts, true, 0.25); // half of segment 0 (20 yd total)
        let (pos, facing, pitch) = s.sample(now);
        assert!(
            (pos[0] - 5.0).abs() < 0.05 && pos[1].abs() < 1e-3,
            "{pos:?}"
        );
        assert!(facing.unwrap().abs() < 1e-3);
        assert_eq!(pitch, 0.0);
    }

    /// A flying path passes through every waypoint: at the chord-length boundary it is the corner.
    #[test]
    fn flying_path_passes_through_waypoints() {
        let pts = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]];
        let (s, now) = spline_at(pts, false, 0.5); // exactly the corner (10 of 20 yd)
        let (pos, _, _) = s.sample(now);
        assert!(
            (pos[0] - 10.0).abs() < 0.05 && pos[1].abs() < 0.05,
            "corner waypoint expected, got {pos:?}"
        );
    }

    /// A flying path curves off the straight chord mid-segment, where a ground path stays on it.
    #[test]
    fn flying_path_bends_off_the_chord() {
        let pts = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]];
        let (s, now) = spline_at(pts, false, 0.25);
        let (pos, _, _) = s.sample(now);
        assert!(
            pos[1].abs() > 0.1,
            "expected a curved deviation off the y=0 chord, got {pos:?}"
        );
    }

    /// A straight climb pitches by the tangent (45° here) and never banks: θ = 2·acos(1) = 0.
    #[test]
    fn a_straight_climb_pitches_by_the_tangent_and_never_banks() {
        let pts = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 10.0]];
        let (s, now) = spline_at(pts, false, 0.3);
        let (pitch, bank) = s.flight_attitude(now);
        assert!(
            (pitch - std::f32::consts::FRAC_PI_4).abs() < 0.05,
            "45° climb tangent, got pitch {pitch}"
        );
        assert_eq!(bank, 0.0, "no bank on a straight path");
    }

    /// A left turn (`cross > 0`) banks positive, a right turn negative, both inside the snap.
    #[test]
    fn a_turn_banks_into_itself() {
        let left = vec![[0.0, 0.0, 0.0], [30.0, 0.0, 0.0], [30.0, 30.0, 0.0]];
        let (s, now) = spline_at(left, false, 0.42);
        let (_, bank) = s.flight_attitude(now);
        assert!(
            bank > 0.1 && bank < std::f32::consts::FRAC_PI_2,
            "left turn leans left, unsnapped — got {bank}"
        );
        let right = vec![[0.0, 0.0, 0.0], [30.0, 0.0, 0.0], [30.0, -30.0, 0.0]];
        let (s, now) = spline_at(right, false, 0.42);
        let (_, bank) = s.flight_attitude(now);
        assert!(
            bank < -0.1 && bank > -std::f32::consts::FRAC_PI_2,
            "right turn leans right, unsnapped — got {bank}"
        );
    }

    /// A create-block spline joins the walk in progress: 9 s into a 12 s, 20-yd path is 15 yd in.
    #[test]
    fn a_create_spline_joins_the_walk_in_progress() {
        let s = create_spline(CreateSpline {
            path: vec![[0.0, 0.0, 0.0], [20.0, 0.0, 0.0]],
            id: 7,
            time_passed_ms: 9_000,
            duration_ms: 12_000,
            flying: false,
            cyclic: false,
            run_mode: true,
        })
        .expect("a live walk");
        let (pos, facing, _) = s.sample(Instant::now());
        assert!(
            (pos[0] - 15.0).abs() < 0.05 && pos[1].abs() < 1e-3,
            "expected three quarters along, got {pos:?}"
        );
        assert!(facing.unwrap().abs() < 1e-3, "facing down the path");
        assert!(s.grounded, "no Flying bit ⇒ terrain-clamped");
    }

    /// A finished or degenerate create-block ride is no walk.
    #[test]
    fn a_finished_or_degenerate_create_spline_is_no_walk() {
        let spline = |time_passed_ms, duration_ms, path: Vec<[f32; 3]>| CreateSpline {
            path,
            id: 1,
            time_passed_ms,
            duration_ms,
            flying: false,
            cyclic: false,
            run_mode: true,
        };
        let straight = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]];
        assert!(create_spline(spline(5_000, 5_000, straight.clone())).is_none());
        assert!(create_spline(spline(0, 0, straight)).is_none());
        assert!(create_spline(spline(0, 5_000, vec![[1.0, 2.0, 3.0]])).is_none());
    }

    /// Near a hairpin the doubled angle passes π/2 and the bank snaps to a full ±π roll.
    #[test]
    fn a_hairpin_snaps_the_bank_to_a_full_roll() {
        let pts = vec![[0.0, 0.0, 0.0], [80.0, 0.0, 0.0], [0.0, 4.0, 0.0]];
        let (s, now) = spline_at(pts, false, 0.47);
        let (_, bank) = s.flight_attitude(now);
        assert_eq!(
            bank.abs(),
            std::f32::consts::PI,
            "the antipodal guard writes the ±π constant, got {bank}"
        );
    }
}

/// [`creature_swim_state`] at a sea giant's numbers: with no swim flag it wades at any depth
/// (without `CREATURE_STATIC_FLAG_CAN_SWIM` vmangos clears `UNIT_FLAG_USE_SWIM_ANIMATION`,
/// `Creature.cpp:517`); with any one of the three gate bits it swims.
#[cfg(test)]
mod swim_gate {
    use super::{creature_swim_state, CREATURE_SWIM_EXIT_BAND};
    use crate::player::swim_enter_depth;

    /// The Shore Strider from the DBCs: display 4945 → CreatureModelData 35 (`SeaGiant.mdx`),
    /// `collisionHeight` 2.083, `CreatureDisplayInfo.scale` 1.75, `modelScale` 1.0.
    const SEA_GIANT_H: f32 = 2.083 * 1.75;

    const NO_FLAGS: u32 = 0;
    const USE_SWIM_ANIMATION: u32 = 0x8000;
    const PLAYER_CONTROLLED: u32 = 0x8;
    const PET_IN_COMBAT: u32 = 0x800;

    #[test]
    fn a_sea_giant_walks_the_lakebed_however_deep_the_water() {
        let boundary = swim_enter_depth(SEA_GIANT_H);
        // Just past the boundary, then far past it.
        for depth in [boundary + 0.01, boundary * 2.0, 100.0] {
            assert!(
                !creature_swim_state(false, depth, boundary, NO_FLAGS),
                "a unit with no swim flag must never enter swim (depth {depth})"
            );
        }
    }

    /// Any one of the three bits admits the unit on the same depth law.
    #[test]
    fn a_flagged_unit_still_enters_on_the_same_depth_law() {
        let boundary = swim_enter_depth(SEA_GIANT_H);
        for flags in [
            USE_SWIM_ANIMATION,
            PLAYER_CONTROLLED,
            PET_IN_COMBAT,
            USE_SWIM_ANIMATION | PLAYER_CONTROLLED | PET_IN_COMBAT,
        ] {
            assert!(
                creature_swim_state(false, boundary + 0.01, boundary, flags),
                "flags {flags:#x} deep enough → swims"
            );
            assert!(
                !creature_swim_state(false, boundary, boundary, flags),
                "flags {flags:#x} exactly at the boundary → not yet (the compare is STRICT)"
            );
        }
    }

    /// Leaving keeps the 1/36-yd band and is inclusive at its edge (`0x6031c5`, a parity test),
    /// where entering is strict (`0x603106`, a ZF test).
    #[test]
    fn leaving_keeps_its_hysteresis_and_is_inclusive_where_entering_is_strict() {
        let boundary = swim_enter_depth(SEA_GIANT_H);
        let flags = USE_SWIM_ANIMATION;
        assert!(
            creature_swim_state(true, boundary - CREATURE_SWIM_EXIT_BAND, boundary, flags),
            "exactly at the lower edge it holds (the stay compare is inclusive)"
        );
        assert!(
            !creature_swim_state(
                true,
                boundary - CREATURE_SWIM_EXIT_BAND - 1e-4,
                boundary,
                flags
            ),
            "below the band it leaves"
        );
        assert!(
            !creature_swim_state(true, f32::MIN, boundary, flags),
            "no liquid at all leaves"
        );
    }

    /// Losing the flag mid-water (a charm ending) stops the swim at any depth (`0x6031eb`).
    #[test]
    fn losing_the_flag_mid_water_stops_the_swim_at_any_depth() {
        let boundary = swim_enter_depth(SEA_GIANT_H);
        assert!(
            creature_swim_state(true, 100.0, boundary, PLAYER_CONTROLLED),
            "charmed and deep: swimming"
        );
        assert!(
            !creature_swim_state(true, 100.0, boundary, NO_FLAGS),
            "charm ends, still 100 yd down: the reference stops it anyway"
        );
    }

    /// Flags 0 before the descriptor block lands cannot enter; the real flags then can.
    #[test]
    fn an_unresolved_descriptor_cannot_enter_but_is_not_stuck() {
        let boundary = swim_enter_depth(SEA_GIANT_H);
        assert!(!creature_swim_state(false, 100.0, boundary, NO_FLAGS));
        assert!(creature_swim_state(
            false,
            100.0,
            boundary,
            USE_SWIM_ANIMATION
        ));
    }
}

/// A Player walked by a ground spline across a hollow is grounded like a creature, and the bodies
/// that must not be; the trench floor is flat under the whole mid-path window.
#[cfg(test)]
mod server_moved_players {
    use std::time::{Duration, Instant};

    use avian3d::prelude::*;
    use benilla_assets::coords::wow_to_bevy;
    use benilla_protocol::EntityKind;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;

    use super::{ground_clamp_creatures, ground_derived, CreatureSwimming, GroundClamped, Spline};
    use crate::net::{Embodied, NetEntity, RemoteMotion};
    use benilla_world::collision::ColliderEpoch;

    /// The rims the waypoints stand on, and the trench floor 2.5 yd below, within the probe.
    const RIM_Z: f32 = 10.0;
    const FLOOR_Y: f32 = 7.5;

    /// The two waypoints, raw WoW, both on a rim: the chord is level at [`RIM_Z`].
    const RIM_A: [f32; 3] = [0.0, 9.0, RIM_Z];
    const RIM_B: [f32; 3] = [0.0, -9.0, RIM_Z];

    /// A ground strip: the `(bevy x, bevy y)` profile extruded ±5 along Bevy Z, faces up.
    fn ground_strip(app: &mut App, profile: &[(f32, f32)]) -> Entity {
        let mut verts = Vec::new();
        let mut tris = Vec::new();
        for (i, &(x, y)) in profile.iter().enumerate() {
            verts.push(Vec3::new(x, y, -5.0));
            verts.push(Vec3::new(x, y, 5.0));
            if i > 0 {
                let b = (i as u32 - 1) * 2;
                tris.push([b, b + 1, b + 3]);
                tris.push([b, b + 3, b + 2]);
            }
        }
        app.world_mut()
            .spawn((
                RigidBody::Static,
                Collider::trimesh(verts, tris),
                Transform::default(),
            ))
            .id()
    }

    /// The hollow, and one `kind` body on the chord over its floor.
    fn hollow(kind: EntityKind) -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<benilla_world::collision::MoverTraceExclusions>();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>().init_resource::<ColliderEpoch>();
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        // The body a walker's swept step sweeps.
        app.insert_resource(crate::player::PlayerCapsule(Collider::capsule(
            crate::player::CAPSULE_RADIUS,
            crate::player::CAPSULE_HEIGHT - 2.0 * crate::player::CAPSULE_RADIUS,
        )));
        app.finish();
        app.cleanup();
        // rim ─╮        ╭─ rim
        //      ╰────────╯   the flat floor the chord flies over
        ground_strip(
            &mut app,
            &[(-9.0, RIM_Z), (-3.0, FLOOR_Y), (3.0, FLOOR_Y), (9.0, RIM_Z)],
        );
        let body = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind,
                    display_id: None,
                    scale: 1.0,
                },
                // Mid-chord: what the sampler writes half-way between the two rims.
                Transform::from_translation(wow_to_bevy([0.0, 0.0, RIM_Z])),
            ))
            .id();
        app.update(); // seats Position/Rotation and the collider trees
        (app, body)
    }

    /// A plain ground walk, as `SMSG_MONSTER_MOVE` sends for a bot or a charged or feared player.
    fn walking(app: &mut App, e: Entity) {
        app.world_mut().entity_mut(e).insert(Spline {
            deck: None,
            points: vec![RIM_A, RIM_B],
            start: Instant::now(),
            duration: Duration::from_secs(4),
            id: 77,
            grounded: true,
            run_mode: true,
        });
    }

    fn relayed() -> RemoteMotion {
        RemoteMotion {
            wow_pos: [0.0, 0.0, RIM_Z],
            orientation: 0.0,
            flags: crate::creature_anim::move_flags::FORWARD,
            pitch: 0.0,
            speed: 7.0,
            vertical_velocity: 0.0,
            jump_xy_vel: [0.0; 2],
            fall_start_z: None,
            pending: std::collections::VecDeque::new(),
            relay: Default::default(),
            last_apply_ms: 0.0,
            last_apply_pos: [0.0; 3],
        }
    }

    fn clamp(app: &mut App) {
        app.world_mut()
            .run_system_once(ground_clamp_creatures)
            .expect("run the clamp");
    }

    fn y_of(app: &App, e: Entity) -> f32 {
        app.world().get::<Transform>(e).unwrap().translation.y
    }

    /// On the trench floor to a millimetre: the ray's answer carries float error, a seat does not.
    #[track_caller]
    fn assert_grounded(app: &App, e: Entity, why: &str) {
        let y = y_of(app, e);
        assert!(
            (y - FLOOR_Y).abs() < 1e-3,
            "{why}: expected the trench floor {FLOOR_Y}, got {y}"
        );
    }

    /// A Player on a ground spline is grounded exactly like a Unit on the same path.
    #[test]
    fn a_bot_walking_a_hollow_is_put_on_the_ground_not_the_chord() {
        for kind in [EntityKind::Player, EntityKind::Unit] {
            let (mut app, body) = hollow(kind);
            walking(&mut app, body);
            assert_eq!(
                y_of(&app, body),
                RIM_Z,
                "{kind:?}: the chord is where the server's spline puts it — the symptom"
            );

            clamp(&mut app);
            assert_grounded(
                &app,
                body,
                &format!("{kind:?}: a grounded spline's Z is the terrain's, not the wire's"),
            );
            assert_eq!(
                app.world().get::<GroundClamped>(body).unwrap().seat_y,
                RIM_Z,
                "{kind:?}: the seat stays the server's pose, never the answer we just wrote"
            );
        }
    }

    /// A flying path keeps its own Z for a player too: `0x616cec` reads the flags, not the kind.
    #[test]
    fn a_flying_path_is_left_alone() {
        let (mut app, body) = hollow(EntityKind::Player);
        app.world_mut().entity_mut(body).insert(Spline {
            deck: None,
            points: vec![RIM_A, RIM_B],
            start: Instant::now(),
            duration: Duration::from_secs(4),
            id: 78,
            grounded: false,
            run_mode: true,
        });
        clamp(&mut app);
        assert_eq!(y_of(&app, body), RIM_Z, "a flight owns its own altitude");
    }

    /// Between paths a bot stays grounded: once `sample_splines` drops the finished spline, the
    /// [`GroundClamped`] memo carries membership, as nothing is known to undo `0x619ca0`'s link.
    #[test]
    fn a_bot_stays_grounded_between_paths() {
        let (mut app, body) = hollow(EntityKind::Player);
        walking(&mut app, body);
        clamp(&mut app);
        assert_grounded(&app, body, "mid-path");

        // The path finished: the sampler has taken the spline away.
        app.world_mut().entity_mut(body).remove::<Spline>();
        // The server puts it somewhere new, still on the chord's level.
        app.world_mut()
            .entity_mut(body)
            .get_mut::<Transform>()
            .unwrap()
            .translation
            .y = RIM_Z;
        clamp(&mut app);
        assert_grounded(
            &app,
            body,
            "a body the server has moved once stays ours to ground until the relay takes it back",
        );
    }

    /// The body we steer is the controller's, spline or not (`player::server_ride` owns its Y).
    #[test]
    fn the_body_we_steer_is_left_to_the_controller() {
        let (mut app, body) = hollow(EntityKind::Player);
        walking(&mut app, body);
        app.world_mut().entity_mut(body).insert(Embodied);
        clamp(&mut app);
        assert_eq!(y_of(&app, body), RIM_Z, "the controller owns its own body");
        assert!(
            app.world().get::<GroundClamped>(body).is_none(),
            "and it never joins the derived set, so it cannot inherit membership later"
        );
    }

    /// A remote player is the relay's, both freshly streamed in and under a live [`RemoteMotion`].
    #[test]
    fn a_relayed_player_is_left_to_the_relay() {
        let (mut app, body) = hollow(EntityKind::Player);
        clamp(&mut app);
        assert_eq!(
            y_of(&app, body),
            RIM_Z,
            "streamed in, never moved by anyone: not ours to ground"
        );

        app.world_mut().entity_mut(body).insert(relayed());
        walking(&mut app, body);
        clamp(&mut app);
        assert_eq!(
            y_of(&app, body),
            RIM_Z,
            "the relay stream outranks a spline that has not displaced it"
        );
    }

    /// A swimming body keeps its wire Z, a Player included.
    #[test]
    fn a_swimming_bot_keeps_its_wire_z() {
        let (mut app, body) = hollow(EntityKind::Player);
        walking(&mut app, body);
        app.world_mut().entity_mut(body).insert(CreatureSwimming);
        clamp(&mut app);
        assert_eq!(
            y_of(&app, body),
            RIM_Z,
            "in liquid the wire Z IS the depth — the reference keeps Δz on SWIMMING too"
        );
    }

    /// The swim mark walks the clamp's population: a dry world unmarks a splined player but not a
    /// relayed one, whose flag is the wire's. PLAYER_CONTROLLED (bit 3) is on every player object.
    #[test]
    fn the_swim_mark_reaches_a_bot_and_not_a_relayed_player() {
        let (mut app, bot) = hollow(EntityKind::Player);
        walking(&mut app, bot);
        app.world_mut().entity_mut(bot).insert(CreatureSwimming);

        let stream = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Player,
                    display_id: None,
                    scale: 1.0,
                },
                Transform::from_translation(wow_to_bevy([0.0, 0.0, RIM_Z])),
                CreatureSwimming,
                relayed(),
            ))
            .id();

        app.world_mut()
            .run_system_once(super::mark_swimming_creatures)
            .expect("run the swim mark");

        assert!(
            app.world().get::<CreatureSwimming>(bot).is_none(),
            "a splined player is the mark's to derive — dry ground unmarks it"
        );
        assert!(
            app.world().get::<CreatureSwimming>(stream).is_some(),
            "a relayed player's swim state is the wire's; the mark must not touch it"
        );
        assert!(
            crate::player::may_swim(0x8),
            "PLAYER_CONTROLLED alone admits a player to the depth decision (0x60310b bit 3)"
        );
    }

    /// The subject rule as a table, with a GameObject and the ways a Player leaves the set.
    #[test]
    fn the_subject_rule() {
        let unit = |splined, memo, embodied, relayed| {
            ground_derived(EntityKind::Unit, splined, memo, embodied, relayed)
        };
        let player = |splined, memo, embodied, relayed| {
            ground_derived(EntityKind::Player, splined, memo, embodied, relayed)
        };
        // A creature is ours walking or idle.
        assert!(unit(true, true, false, false));
        assert!(unit(false, false, false, false));
        // A possessed creature is the controller's.
        assert!(!unit(true, true, true, false));
        assert!(!unit(false, false, true, false));

        assert!(player(true, false, false, false), "a bot's first path");
        assert!(
            player(false, true, false, false),
            "and every frame after it"
        );
        assert!(
            !player(false, false, false, false),
            "before anything moves it"
        );
        assert!(!player(true, true, true, false), "the body we steer");
        assert!(!player(true, true, false, true), "under the relay stream");

        for kind in [EntityKind::GameObject, EntityKind::Corpse] {
            assert!(
                !ground_derived(kind, true, true, false, false),
                "{kind:?} sits at the Z it was authored at"
            );
        }
    }
}

/// A unit inside a GameObject's outward-wound collision box (Galen Goodward's cage), idle and
/// walking, where every face is a backface; a walker on a hill; re-basing on a new path.
#[cfg(test)]
mod inside_a_hull {
    use std::time::{Duration, Instant};

    use avian3d::prelude::*;
    use benilla_protocol::EntityKind;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;

    use super::{ground_clamp_creatures, GroundClamped, Spline};
    use crate::net::NetEntity;
    use benilla_world::collision::ColliderEpoch;

    /// Galen's cage over its terrain: box floor 0.4 yd up, lid 2.72 yd (`G_Cage.mdx`, z
    /// 0.398..2.724 at size 1), and his seat 0.07 over the box floor.
    const TERRAIN_Y: f32 = 22.0;
    const BOX_FLOOR_Y: f32 = TERRAIN_Y + 0.4;
    const LID_Y: f32 = TERRAIN_Y + 2.72;
    const SEAT_Y: f32 = BOX_FLOOR_Y + 0.07;
    const HALF: f32 = 1.43;

    /// A 30×30 up-wound quad at `y` around the origin.
    fn floor_at(app: &mut App, y: f32) -> Entity {
        floor_around(app, Vec3::new(0.0, y, 0.0))
    }

    fn floor_around(app: &mut App, c: Vec3) -> Entity {
        let (x, y, z) = (c.x, c.y, c.z);
        let verts = vec![
            Vec3::new(x - 15.0, y, z - 15.0),
            Vec3::new(x + 15.0, y, z - 15.0),
            Vec3::new(x + 15.0, y, z + 15.0),
            Vec3::new(x - 15.0, y, z + 15.0),
        ];
        app.world_mut()
            .spawn((
                RigidBody::Static,
                Collider::trimesh(verts, vec![[0u32, 2, 1], [0, 3, 2]]),
                Transform::default(),
            ))
            .id()
    }

    /// A closed box hull with every triangle wound outward, checked per triangle: a face wound
    /// inward would be a hole and pass these tests for the wrong reason.
    fn box_hull(app: &mut App, min: Vec3, max: Vec3) -> Entity {
        let c = |i: usize| {
            Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            )
        };
        let verts: Vec<Vec3> = (0..8).map(c).collect();
        let centre = (min + max) * 0.5;
        // Each face as its four corner indices; the diagonal split below, then the outward flip.
        const FACES: [[u32; 4]; 6] = [
            [0, 1, 3, 2], // y = min
            [4, 5, 7, 6], // y = max
            [0, 1, 5, 4], // z = min
            [2, 3, 7, 6], // z = max
            [0, 2, 6, 4], // x = min
            [1, 3, 7, 5], // x = max
        ];
        let mut tris = Vec::new();
        for f in FACES {
            for [a, b, d] in [[f[0], f[1], f[2]], [f[0], f[2], f[3]]] {
                let (va, vb, vd) = (verts[a as usize], verts[b as usize], verts[d as usize]);
                let n = (vb - va).cross(vd - va);
                let outward = n.dot((va + vb + vd) / 3.0 - centre) > 0.0;
                tris.push(if outward { [a, b, d] } else { [a, d, b] });
            }
        }
        app.world_mut()
            .spawn((
                RigidBody::Static,
                Collider::trimesh(verts, tris),
                Transform::default(),
            ))
            .id()
    }

    fn world() -> App {
        let mut app = App::new();
        app.init_resource::<benilla_world::collision::MoverTraceExclusions>();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>().init_resource::<ColliderEpoch>();
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        // The body a walker's swept step sweeps.
        app.insert_resource(crate::player::PlayerCapsule(Collider::capsule(
            crate::player::CAPSULE_RADIUS,
            crate::player::CAPSULE_HEIGHT - 2.0 * crate::player::CAPSULE_RADIUS,
        )));
        app.finish();
        app.cleanup();
        app
    }

    /// The cage on its terrain, and a unit spawned at Galen's seat inside it.
    fn caged() -> (App, Entity) {
        let mut app = world();
        floor_at(&mut app, TERRAIN_Y);
        box_hull(
            &mut app,
            Vec3::new(-HALF, BOX_FLOOR_Y, -HALF),
            Vec3::new(HALF, LID_Y, HALF),
        );
        let npc = spawn_unit(&mut app, Vec3::new(0.0, SEAT_Y, 0.0));
        app.update();
        (app, npc)
    }

    fn spawn_unit(app: &mut App, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Transform::from_translation(at),
            ))
            .id()
    }

    fn clamp(app: &mut App) {
        app.world_mut()
            .run_system_once(ground_clamp_creatures)
            .expect("run the clamp");
    }

    fn y_of(app: &App, e: Entity) -> f32 {
        app.world().get::<Transform>(e).unwrap().translation.y
    }

    /// A grounded path on `e`; the tests move the transform one "frame" at a time themselves.
    fn path(app: &mut App, e: Entity, id: u32, a: [f32; 3], b: [f32; 3], secs: u64) {
        app.world_mut().entity_mut(e).insert(Spline {
            deck: None,
            points: vec![a, b],
            start: Instant::now(),
            duration: Duration::from_secs(secs),
            id,
            grounded: true,
            run_mode: true,
        });
    }

    /// Idle inside the box, the down-ray passes its backface floor to the terrain, never the lid.
    #[test]
    fn an_idle_unit_inside_a_closed_hull_is_never_put_on_its_lid() {
        let (mut app, npc) = caged();
        clamp(&mut app);
        let y = y_of(&app, npc);
        assert!(
            y < BOX_FLOOR_Y + 0.1,
            "inside the box, not on it: got {y}, the lid is at {LID_Y}"
        );
        assert!(
            (y - TERRAIN_Y).abs() < 1e-3,
            "a down-ray from the seat passes the box's backface floor to the terrain: got {y}"
        );
        assert_eq!(
            app.world().get::<GroundClamped>(npc).unwrap().seat_y,
            SEAT_Y,
            "the seat stays the server's, never the answer just written"
        );
    }

    /// Walking inside the box meets only backfaces, so the unit keeps its floor.
    #[test]
    fn a_walker_inside_a_closed_hull_keeps_its_floor() {
        let (mut app, npc) = caged();
        clamp(&mut app); // the first frame: from the seat
        let start = y_of(&app, npc);
        path(&mut app, npc, 1, [0.0, 0.0, SEAT_Y], [0.0, 1.0, SEAT_Y], 2);
        // Twelve "frames" of a slow walk across the box, the chord at the seat's height.
        for i in 1..=12 {
            let z = -0.6 + i as f32 * 0.1;
            let mut t = app.world_mut().get_mut::<Transform>(npc).unwrap();
            t.translation = Vec3::new(0.0, SEAT_Y, z);
            clamp(&mut app);
            let y = y_of(&app, npc);
            assert!(
                y < BOX_FLOOR_Y + 0.1,
                "frame {i}: still inside the box, got {y} (lid {LID_Y})"
            );
            assert!(
                (y - start).abs() < 0.05,
                "frame {i}: walking level ground keeps its height, got {y} from {start}"
            );
        }
    }

    /// A walker rides a hill its chord cuts under: each step starts from the surface it stood on.
    #[test]
    fn a_walker_rides_a_hill_its_chord_cuts_under() {
        let mut app = world();
        // A 3-yd hill of 30° over ±5 yd, then flat: the profile in x, extruded across z.
        let profile: [(f32, f32); 5] = [
            (-20.0, 0.0),
            (-5.0, 0.0),
            (0.0, 2.9),
            (5.0, 0.0),
            (20.0, 0.0),
        ];
        let mut verts = Vec::new();
        let mut tris = Vec::new();
        for (i, &(x, y)) in profile.iter().enumerate() {
            verts.push(Vec3::new(x, y, -10.0));
            verts.push(Vec3::new(x, y, 10.0));
            if i > 0 {
                let b = (i as u32 - 1) * 2;
                tris.push([b, b + 1, b + 3]);
                tris.push([b, b + 3, b + 2]);
            }
        }
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::trimesh(verts, tris),
            Transform::default(),
        ));
        let npc = spawn_unit(&mut app, Vec3::new(-6.0, 0.0, 0.0));
        app.update();
        clamp(&mut app);
        // The chord runs flat at y = 0 from x = −6 to +6: under the hill the whole way.
        path(&mut app, npc, 7, [-6.0, 0.0, 0.0], [6.0, 0.0, 0.0], 4);
        let mut peak = f32::MIN;
        for i in 1..=120 {
            let x = -6.0 + i as f32 * 0.1;
            let mut t = app.world_mut().get_mut::<Transform>(npc).unwrap();
            t.translation = Vec3::new(x, 0.0, 0.0);
            clamp(&mut app);
            let y = y_of(&app, npc);
            let surface = if x.abs() < 5.0 {
                2.9 * (1.0 - x.abs() / 5.0)
            } else {
                0.0
            };
            assert!(
                y >= surface - 0.05,
                "frame {i} at x={x:.1}: under the hill ({y} vs surface {surface:.2})"
            );
            peak = peak.max(y);
        }
        assert!(peak > 2.5, "the walker climbed the hill: peak {peak}");
    }

    /// The shipped `G_Cage.mdx` hull at its spawn (guid 29361), the terrain under it, and a unit
    /// at Galen's spawn. Skips without client data.
    #[test]
    fn galens_cage_from_the_shipped_hull_grounds_him_inside() {
        use benilla_assets::coords::wow_to_bevy;
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let hull = benilla_formats::load_m2_collision_hull(&mut chain, "World\\Goober\\G_Cage.mdx")
            .expect("the cage hull");
        assert_eq!(hull.positions.len(), 8, "one box");
        let verts: Vec<Vec3> = hull.positions.iter().map(|p| wow_to_bevy(*p)).collect();
        let tris: Vec<[u32; 3]> = hull
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        let mut app = world();
        const TERRAIN: f32 = 21.958;
        let under_cage = wow_to_bevy([-9898.3, -3724.76, TERRAIN]);
        floor_around(&mut app, under_cage);
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::trimesh(verts, tris),
            Transform {
                translation: wow_to_bevy([-9898.3, -3724.76, 21.9428]),
                rotation: super::super::gameobject_rotation(
                    Some([0.0, 0.0, 0.909961, 0.414694]),
                    0.0,
                ),
                ..default()
            },
        ));
        let npc = spawn_unit(&mut app, wow_to_bevy([-9898.53, -3724.63, 22.4164]));
        app.update();
        clamp(&mut app);
        let y = y_of(&app, npc);
        assert!(
            (y - TERRAIN).abs() < 1e-3,
            "from his seat the ray passes the cage's own floor to the terrain: got {y} (seat 22.4164, lid 24.667)"
        );
    }

    /// A new path re-bases from the server's Z (`0x7c6420`): a walker's memo under a late floor
    /// does not carry into its next path.
    #[test]
    fn a_new_path_starts_again_from_the_seat() {
        let mut app = world();
        floor_at(&mut app, 0.0);
        let npc = spawn_unit(&mut app, Vec3::new(0.0, 0.1, 0.0));
        app.update();
        // Walking on the terrain, the building's floor 2 yd up not yet built.
        path(&mut app, npc, 1, [0.0, 0.0, 2.1], [4.0, 0.0, 2.1], 2);
        clamp(&mut app);
        assert!(
            (y_of(&app, npc) - 0.0).abs() < 1e-3,
            "first frame: from the seat, onto terrain"
        );
        for i in 1..=5 {
            let mut t = app.world_mut().get_mut::<Transform>(npc).unwrap();
            t.translation = Vec3::new(i as f32 * 0.1, 2.1, 0.0);
            clamp(&mut app);
        }
        // The floor lands under a walker mid-path: the path continues where it stood (under it).
        floor_at(&mut app, 2.0);
        app.world_mut().resource_mut::<ColliderEpoch>().bump();
        app.update();
        let mut t = app.world_mut().get_mut::<Transform>(npc).unwrap();
        t.translation = Vec3::new(0.6, 2.1, 0.0);
        clamp(&mut app);
        assert!(
            y_of(&app, npc) < 1.0,
            "mid-path the walker continues from where it stood: {}",
            y_of(&app, npc)
        );
        // The next packet starts a new path from the server's Z on the floor: re-based.
        path(&mut app, npc, 2, [0.6, 0.0, 2.1], [4.0, 0.0, 2.1], 2);
        let mut t = app.world_mut().get_mut::<Transform>(npc).unwrap();
        t.translation = Vec3::new(0.7, 2.1, 0.0);
        clamp(&mut app);
        assert!(
            (y_of(&app, npc) - 2.0).abs() < 1e-3,
            "a new path is seated from the server's own Z: {}",
            y_of(&app, npc)
        );
    }
}
