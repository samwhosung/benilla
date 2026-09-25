//! Remote-player dead-reckoning from relayed `MSG_MOVE_*`: flag-driven locomotion between the
//! 500 ms heartbeats, and a jump played locally as a ballistic arc.

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_protocol::{JumpInfo, MoveSpeeds};
use bevy::prelude::*;
use bevy::time::Real;

use crate::creature_anim::move_flags;
use crate::player::{GRAVITY, TERMINAL_VELOCITY};

use super::super::{ActiveMover, UnitSpeeds};
use super::relay::{PendingMove, RelayChain, RelayMove};
use super::{yaw_of, Spline};

/// Another player's movement state, set by each relayed move and dead-reckoned from `flags` in
/// between; the WoW-space pose the `Transform` is derived from. While `FALLING` the horizontal is
/// frozen at the launch and the height is a parabola, re-seeded by each jump packet's tail.
#[derive(Component, Clone)]
pub(crate) struct RemoteMotion {
    pub(crate) wow_pos: [f32; 3],
    pub(crate) orientation: f32,
    /// The `CMovement` `moveFlags` the mover last reported.
    pub(crate) flags: u32,
    /// The reported swim pitch, radians up, `0.0` unless `SWIMMING`.
    pub(crate) pitch: f32,
    /// The horizontal speed being applied, yd/s, which picks and rate-scales the gait.
    pub(crate) speed: f32,
    /// Yd/s, +Z up, while airborne; `0` on the ground.
    pub(crate) vertical_velocity: f32,
    /// The launch's `(cos, sin) * xyspeed`, frozen for the arc; `[0; 2]` on the ground.
    pub(crate) jump_xy_vel: [f32; 2],
    /// The takeoff Z of this arc, `None` if the mover entered view airborne; on landing,
    /// `fall_start_z - landing_z` is the fall height that gates the grunt and dust puff.
    pub(crate) fall_start_z: Option<f32>,
    /// Not-yet-due relayed moves by fire-time, the reference's `CMovement+0x150` queue.
    pub(crate) pending: std::collections::VecDeque<PendingMove>,
    pub(crate) relay: RelayChain,
    /// Real-time ms and position of the last applied packet, `0.0` before the first; everything
    /// since is our extrapolation.
    pub(crate) last_apply_ms: f64,
    pub(crate) last_apply_pos: [f32; 3],
}

/// What `unit_move` did with an inbound relayed move: the `out=` field of the `rly` trace, which
/// logs discarded arrivals too.
#[derive(Clone, Copy)]
pub(in crate::net) enum RelayOutcome {
    /// Due, with an empty queue.
    Now,
    Queued,
    /// The unit's first packet, placed at once.
    Seed,
    /// A server-authored pose for our own mover, handed to the controller as a
    /// [`crate::net::SelfMoveMessage`] at arrival. Deviation: the reference paces a self move
    /// through the same replay chain as a remote's, which would only delay a correction to our own
    /// pose.
    SelfMover,
    /// No entity for this guid: the packet changes nothing.
    Unknown,
}

impl RelayOutcome {
    fn tag(self) -> &'static str {
        match self {
            Self::Now => "now",
            Self::Queued => "queued",
            Self::Seed => "seed",
            Self::SelfMover => "self",
            Self::Unknown => "UNKNOWN-GUID",
        }
    }
}

/// The `rly` trace line per inbound relayed move: stamp, flags, outcome, schedule lead and queue
/// depth.
pub(in crate::net) fn trace_relay(
    guid: u64,
    mv: &RelayMove,
    chain: &RelayChain,
    now_ms: f64,
    queued: usize,
    out: RelayOutcome,
) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    let lead = chain.lead_ms(now_ms);
    let kind = match mv.verb {
        benilla_protocol::RelayVerb::Heartbeat => "hb",
        benilla_protocol::RelayVerb::Teleport => "tp",
        benilla_protocol::RelayVerb::Root(_) => "rt",
        benilla_protocol::RelayVerb::Pose => "tr",
    };
    benilla_assets::trace::line(
        "rly",
        &format!(
            "guid={guid:#x} {kind} wire={} flags={:#x} pitch={:+.3} out={} lead={lead:7.1} q={queued}",
            mv.wire_ms,
            mv.flags,
            mv.pitch,
            out.tag()
        ),
    );
}

/// Ms of dead-reckoning with nothing queued before the runaway watch reports; a moving unit
/// sends a heartbeat at least every 500 ms.
const RUNAWAY_SILENCE_MS: f64 = 2000.0;

/// The runaway watch (tag `run`): one line per silent second for a mover still moving on our
/// extrapolation alone, with its drift from the last applied position. Reporting only.
fn trace_runaway(guid_hint: Entity, rm: &RemoteMotion, now_ms: f64, silent_s: u32) {
    let d = [
        rm.wow_pos[0] - rm.last_apply_pos[0],
        rm.wow_pos[1] - rm.last_apply_pos[1],
    ];
    // Packets still landing means the server stopped relaying this unit; none means the socket.
    let (pkts, age) = crate::net::io::inbound_census();
    let age = age.map_or_else(|| "never".to_string(), |ms| format!("{ms}ms"));
    benilla_assets::trace::line(
        "run",
        &format!(
            "{guid_hint} RUNAWAY flags={:#x} silent={silent_s}s drift={:.1}yd since={:.0}ms pos=[{:.1},{:.1}] netpkts={pkts} lastpkt={age}",
            rm.flags,
            d[0].hypot(d[1]),
            now_ms - rm.last_apply_ms,
            rm.wow_pos[0],
            rm.wow_pos[1],
        ),
    );
}

/// The `rem` trace's sampling period, ms.
const REMOTE_TRACE_MS: f64 = 500.0;

/// The body tilt the swim-pitch render law presents for this mover, traced beside the reported
/// pitch.
fn rendered_pitch(rm: &RemoteMotion) -> f32 {
    let (_, x, _) = crate::creature_anim::swim_body_rotation(0.0, rm.flags, rm.pitch)
        .to_euler(bevy::math::EulerRot::YXZ);
    x
}

/// The remote-pose watch (tag `rem`), one line per mover per [`REMOTE_TRACE_MS`]:
///
/// - `dz`: this frame's height change, excluding a packet applied this frame.
/// - `held`: the horizontal travel the world took away this frame.
/// - `drop`: how far below its last packet's Z the mover is drawn.
/// - `pitch` / `tilt`: the reported swim pitch and the tilt rendered for it.
/// - `age`: ms since a packet last applied.
fn trace_remote(e: Entity, guid: u64, rm: &RemoteMotion, held: f32, dz: f32, now_ms: f64) {
    benilla_assets::trace::line(
        "rem",
        &format!(
            "{e} guid={guid:#x} flags={:#x} pos=[{:.2},{:.2},{:.2}] pitch={:+.3} tilt={:+.3} dz={dz:+.3} \
             drop={:+.3} held={held:.3} age={:.0}ms",
            rm.flags,
            rm.wow_pos[0],
            rm.wow_pos[1],
            rm.wow_pos[2],
            rm.pitch,
            rendered_pitch(rm),
            rm.last_apply_pos[2] - rm.wow_pos[2],
            now_ms - rm.last_apply_ms,
        ),
    );
}

/// `WOW_REMOTE_SNAP=1`: apply every relayed move at arrival as a raw snap, with no queue and no
/// reconcile lerp, for an A/B.
pub(in crate::net) fn arrival_snap() -> bool {
    static SNAP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SNAP.get_or_init(|| std::env::var_os("WOW_REMOTE_SNAP").is_some())
}

/// `WOW_REMOTE_FLAT=1`: dead-reckon without the world (height frozen at the last packet's Z, the
/// step unswept), for an A/B.
fn flat_extrapolation() -> bool {
    static FLAT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAT.get_or_init(|| std::env::var_os("WOW_REMOTE_FLAT").is_some())
}

/// `WOW_REMOTE_IDLE_GATE=off`: resolve flag-still movers against the world too, which sinks one
/// standing over a missing floor collider by 1/36 yd a frame; an A/B lever.
fn idle_gate_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        std::env::var("WOW_REMOTE_IDLE_GATE").is_ok_and(|v| matches!(v.as_str(), "off" | "0"))
    })
}

/// `0x80c744`, square yards (0.0278 yd squared), compared in 2D; Z joins only while swimming
/// (`0x619090`).
const RECONCILE_TOL_SQ: f32 = 7.716e-4;

/// One frame of the pre-fire reconcile (`0x619090` arm, `0x6191c0` lerp): when the pose predicted
/// at the fire-time misses `target`, blend linearly in time so `pos` lands on it at the fire-time.
pub(super) fn reconcile_lerp(
    mut pos: [f32; 3],
    predicted: [f32; 3],
    target: [f32; 3],
    swimming: bool,
    dt: f32,
    remaining_s: f32,
) -> [f32; 3] {
    let d = [
        predicted[0] - target[0],
        predicted[1] - target[1],
        predicted[2] - target[2],
    ];
    let dist_sq = d[0] * d[0] + d[1] * d[1] + if swimming { d[2] * d[2] } else { 0.0 };
    if dist_sq < RECONCILE_TOL_SQ {
        return pos;
    }
    let f = dt / (dt + remaining_s);
    for (p, t) in pos.iter_mut().zip(target) {
        *p += (t - *p) * f;
    }
    pos
}

/// `0x8026bc`, the guard on the `0x618f80` angular velocity.
const FACING_DEAD_ZONE: f32 = 9.5367e-7;

/// One frame of the pre-fire facing interp, a remote's only smoothed facing (`0x618f80`
/// shortest-arc rate into `+0x144`, integrated by `0x7c4f30`): linear in time, landing at the
/// fire-time, where the apply snaps and `0x617e90` zeroes `+0x148`.
pub(super) fn facing_lerp(orientation: f32, target: f32, dt: f32, remaining_s: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut d = (target - orientation) % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    if d.abs() < FACING_DEAD_ZONE {
        return orientation;
    }
    orientation + d * (dt / (dt + remaining_s))
}

/// Applies one relayed move, the reference's snap and re-seed at arrival (`0x7c6420`) or at fire
/// (`0x617e90` to `0x7c69a0`): the pose outright, the jump tail, the landing edge, the rider tail.
pub(in crate::net) fn apply_move(
    e: Entity,
    ev: &RelayMove,
    rm: &mut RemoteMotion,
    now_ms: f64,
    commands: &mut Commands,
    landings: &mut MessageWriter<crate::creature_anim::HardLanding>,
) {
    use crate::creature_anim::move_flags::FALLING;
    // A transport-local pose, re-anchored each frame by `compose_riders`; no tail means off.
    match &ev.transport {
        Some(t) => {
            commands.entity(e).insert(crate::transport::TransportRider {
                transport_guid: t.guid,
                local_pos: [t.pos.x, t.pos.y, t.pos.z],
                local_orientation: t.orientation,
            });
        }
        None => {
            commands
                .entity(e)
                .remove::<crate::transport::TransportRider>();
        }
    }
    let (vertical_velocity, jump_xy_vel) =
        jump_seed(ev.jump, ev.fall_time, ev.flags & move_flags::SAFE_FALL != 0);
    let now_falling = ev.flags & FALLING != 0;
    // The `FALLING` to grounded edge is a landing; its fall height gates the grunt and dust puff.
    let was_falling = rm.flags & FALLING != 0;
    let (new_start, descent) =
        fall_arc_step(was_falling, now_falling, rm.fall_start_z, ev.position[2]);
    rm.fall_start_z = new_start;
    if let Some(descent) = descent {
        landings.write(crate::creature_anim::HardLanding { entity: e, descent });
    }
    rm.wow_pos = ev.position;
    rm.orientation = ev.orientation;
    // A merge through `0x75a07dff` (`0x618de7`): client-owned bits such as `ON_TRANSPORT` survive
    // a relayed word.
    rm.flags = move_flags::merge_server_authored(rm.flags, ev.flags);
    // The root verb outranks the word, after the merge: `SetRoot` (`0x7c7340`) sets `0x1000` and
    // wipes the motion bits (`& 0xffe07f00`); `ClearRoot` (`0x7c7370`) only clears the bit.
    match ev.verb {
        benilla_protocol::RelayVerb::Root(true) => {
            rm.flags = (rm.flags | move_flags::ROOT) & super::modes::ROOT_APPLY_WIPE;
        }
        benilla_protocol::RelayVerb::Root(false) => rm.flags &= !move_flags::ROOT,
        _ => {}
    }
    rm.pitch = ev.pitch;
    rm.vertical_velocity = vertical_velocity;
    rm.jump_xy_vel = jump_xy_vel;
    rm.last_apply_ms = now_ms;
    rm.last_apply_pos = ev.position;
}

/// Fires every due queued move (the drain `0x615c30`); runs before [`extrapolate_remote_units`].
/// Real time: the reference's clock is the OS wall clock (`0x42c010`), and Bevy's virtual clock
/// clamps each delta to 250 ms, so a throttled window would fall behind the schedule.
#[allow(clippy::type_complexity)]
pub(in crate::net) fn drain_pending_moves(
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut landings: MessageWriter<crate::creature_anim::HardLanding>,
    mut q: Query<(Entity, &mut RemoteMotion), (Without<Spline>, Without<ActiveMover>)>,
) {
    let now_ms = time.elapsed_secs_f64() * 1000.0;
    for (e, mut rm) in &mut q {
        while rm.pending.front().is_some_and(|ev| ev.fire_ms <= now_ms) {
            let ev = rm.pending.pop_front().expect("front checked");
            apply_move(e, &ev.mv, &mut rm, now_ms, &mut commands, &mut landings);
        }
    }
}

/// Dead-reckons every remote mover each frame from its flags, then resolves the step against the
/// world with our own controller's capsule and step election, as the reference drives every mover
/// through one controller. A mover with no [`move_flags::INTEGRATED`] bit is not integrated, as in
/// the reference. The wire has no blocked bit: a mover held by a wall is held by ours. Real time,
/// the clock the fire-times are on.
#[allow(clippy::type_complexity)]
pub(in crate::net) fn extrapolate_remote_units(
    time: Res<Time<Real>>,
    mut commands: Commands,
    // The local controller's own sweep and capsule.
    world: benilla_world::collision::WorldCollision,
    // Liquid is queried, not swept, so a water-walker's plane is handed to the step.
    points: benilla_world::world_point::WorldPoint,
    capsule: Res<crate::player::PlayerCapsule>,
    // The runaway watch's last reported silent second per mover.
    mut warned: Local<bevy::platform::collections::HashMap<Entity, u32>>,
    // When each mover last wrote a `rem` line.
    mut sampled: Local<bevy::platform::collections::HashMap<Entity, f64>>,
    mut q: Query<
        (
            Entity,
            &mut Transform,
            &mut RemoteMotion,
            Option<&UnitSpeeds>,
            Option<&mut crate::creature_anim::BodyTwist>,
            Has<super::FacingStep>,
            Has<crate::transport::TransportRider>,
            // Trace-only: joins a `rem` line to its `rly` lines.
            Option<&crate::net::Guid>,
            // Modes from `SMSG_SPLINE_MOVE_*`, which vmangos sends only for a unit it drives; a
            // player's ride the pose, so the union is the reference's one `CMovement+0x40` word.
            Option<&crate::net::UnitMoveModes>,
        ),
        (Without<Spline>, Without<ActiveMover>),
    >,
) {
    use crate::creature_anim::{ease_strafe_yaw, strafe_body_offset, wrap_pi};
    let dt = time.delta_secs();
    let now_ms = time.elapsed_secs_f64() * 1000.0;
    for (e, mut t, mut rm, speeds, twist, latched, riding, guid, granted) in &mut q {
        if benilla_assets::trace::enabled() {
            let silent = now_ms - rm.last_apply_ms;
            let moving = rm.flags & move_flags::ANY_MOVE != 0;
            if moving && rm.pending.is_empty() && silent > RUNAWAY_SILENCE_MS {
                let silent_s = (silent / 1000.0) as u32;
                if warned.insert(e, silent_s) != Some(silent_s) {
                    trace_runaway(e, &rm, now_ms, silent_s);
                }
            } else {
                warned.remove(&e);
            }
        }
        let s = speeds.map_or_else(MoveSpeeds::default, |u| u.0);
        let prev = rm.wow_pos;
        let (mut pos, mut orientation, vertical_velocity, speed) = rm.advance(s, dt);
        // Horizontal travel the world took away, measured before the reconcile lerp.
        let mut held = 0.0f32;
        // The step meets the world: grounded, the swept capsule and step election; airborne, walls
        // only, since the arc owns its Z. A swimmer (the wire Z is its depth) and a transport rider
        // (a transport-local pose) are not resolved.
        let airborne = rm.flags & move_flags::FALLING != 0;
        let afloat = rm.flags & move_flags::SWIMMING != 0;
        // A flag-still mover is not integrated: `CMovement::Update`'s substep loop (`0x616e20`)
        // and the per-mover tick (`0x6166f5`) both bail on `0x20ff`, so its pose is the last
        // packet's. This also keeps a standing mover from sinking where a WMO floor collider has
        // not attached yet. `modes` is the whole flags word and feeds `Support`; the gate reads
        // only the pose's bits.
        let modes = rm.flags | granted.map_or(0, |m| m.0);
        let integrating = rm.flags & move_flags::INTEGRATED != 0 || idle_gate_disabled();
        if integrating && !afloat && !riding && !flat_extrapolation() {
            let half_h = Vec3::Y * (crate::player::CAPSULE_HEIGHT * 0.5);
            let from = wow_to_bevy(rm.wow_pos) + half_h;
            // Grounded this is horizontal; airborne it carries the arc's vertical.
            let vel = if dt > 1.0e-6 {
                (wow_to_bevy(pos) + half_h - from) / dt
            } else {
                Vec3::ZERO
            };
            let resolved_center = if airborne {
                crate::player::mover::airborne_step(&world, &capsule.0, from, vel, time.delta())
            } else {
                // The mover's granted modes: `HOVER` raises it a yard (`0x636e81`), and
                // `WATER_WALKING` makes the liquid plane floor (`0x63162e`) unless swimming
                // (`0x631617`). The local mover's aim-pitch gate has no meaning here. No
                // steep-support bit is carried between packets; the next packet's Z corrects that.
                let water = (modes & move_flags::WATER_WALKING != 0
                    && modes & move_flags::SWIMMING == 0)
                    .then(|| {
                        points
                            .liquid_at(benilla_world::world_point::Subject::Unit(e), pos)
                            .map(|hit| wow_to_bevy(pos).y + (hit.surface_z - pos[2]))
                    })
                    .flatten();
                let g = crate::player::mover::grounded_step(
                    &world,
                    &capsule.0,
                    from,
                    vel,
                    time.delta(),
                    crate::player::mover::Support {
                        // `0x5fa550` is true for an uncharmed player, so the step height is 1.0.
                        rise: crate::player::STEP_UP_HEIGHT,
                        offset: if modes & move_flags::HOVER != 0 {
                            crate::player::HOVER_HEIGHT
                        } else {
                            0.0
                        },
                        water,
                        steep: false,
                    },
                );
                // No floor found: a remote has no fall election, so keep the packet's height and
                // take only the horizontal.
                match g.unsupported {
                    Some(_) => Vec3::new(g.center.x, from.y, g.center.z),
                    None => g.center,
                }
            };
            let resolved = bevy_to_wow(resolved_center - half_h);
            held = (pos[0] - resolved[0]).hypot(pos[1] - resolved[1]);
            pos = resolved;
        }
        // The pre-fire reconcile toward the queued head, facing (armed by `0x619030`) and position
        // (`0x619090`); a teleport arms neither (`@0x61904b`, `@0x6190bb`).
        if let Some(ev) = rm.pending.front() {
            let remaining_s = ((ev.fire_ms - now_ms) / 1000.0) as f32;
            if remaining_s > 0.0 && ev.mv.reconciles() {
                orientation = facing_lerp(orientation, ev.mv.orientation, dt, remaining_s);
                // Predicted from the pre-frame state to the fire-time.
                let (predicted, ..) = rm.advance(s, dt + remaining_s);
                let swimming = ev.mv.flags & move_flags::SWIMMING != 0;
                pos = reconcile_lerp(pos, predicted, ev.mv.position, swimming, dt, remaining_s);
            }
        }
        // A mouse-turning mover sends no turn flag, only facing packets, so a standing, grounded
        // mover's applied yaw latches the shuffle (the reference's display-facing latch,
        // `0x607ed0` to `+0xd58` `0x800`/`0x1000` to `0x712090`, anims 11 and 12).
        let grounded_still = rm.flags
            & (move_flags::ANY_MOVE
                | move_flags::TURN_LEFT
                | move_flags::TURN_RIGHT
                | move_flags::FALLING
                | move_flags::SWIMMING)
            == 0;
        let step = if grounded_still {
            wrap_pi(orientation - rm.orientation)
        } else {
            0.0
        };
        if step.abs() > super::facing::TURN_LATCH_BAND {
            commands.entity(e).insert(super::FacingStep(step));
        } else if latched {
            commands.entity(e).remove::<super::FacingStep>();
        }
        rm.wow_pos = pos;
        rm.orientation = orientation;
        rm.vertical_velocity = vertical_velocity;
        rm.speed = speed;
        if benilla_assets::trace::enabled()
            && sampled
                .get(&e)
                .is_none_or(|t| now_ms - t >= REMOTE_TRACE_MS)
        {
            sampled.insert(e, now_ms);
            trace_remote(
                e,
                guid.map_or(0, |g| g.0),
                &rm,
                held,
                pos[2] - prev[2],
                now_ms,
            );
        }
        // Write only on change, so an idle crowd does not trip every `Changed<Transform>` gate.
        let translation = wow_to_bevy(pos);
        if t.translation != translation {
            t.translation = translation;
        }
        // The strafe body pose, as our own avatar's; swimming snaps the display facing to the aim
        // (`0x607ed0`'s `mov [esi+0xc94],[esi+0xc98]`).
        let swimming = rm.flags & move_flags::SWIMMING != 0;
        let offset = if swimming {
            0.0
        } else {
            strafe_body_offset(rm.flags)
        };
        let yaw = if offset != 0.0 {
            ease_strafe_yaw(yaw_of(t.rotation), orientation, offset, dt)
        } else {
            orientation
        };
        // The swim body pitch from the reported pitch, as for our own avatar (`0x60a110`).
        let rotation = crate::creature_anim::swim_body_rotation(yaw, rm.flags, rm.pitch);
        if t.rotation != rotation {
            t.rotation = rotation;
        }
        if let Some(mut twist) = twist {
            let gap = wrap_pi(orientation - yaw);
            if twist.yaw_gap != gap {
                twist.yaw_gap = gap;
            }
        }
    }
}

/// A jump packet's ballistic seed: the vertical speed now (+Z up) and the frozen horizontal.
/// The wire `zspeed` is down-positive (the 1.12 client sends -7.955547 rising; vmangos sets
/// 7.958 up at `MovementHandler.cpp:322`), so the speed is `-zspeed - g * t`. `feather` picks
/// `[0x87d898]` = 7.0 over `[0x87d894]` = 60.148 (`0x7c5d20`, on the mover's `SAFE_FALL`).
pub(crate) fn jump_seed(jump: Option<JumpInfo>, fall_time: u32, feather: bool) -> (f32, [f32; 2]) {
    let terminal = if feather {
        crate::player::FEATHER_TERMINAL_VELOCITY
    } else {
        TERMINAL_VELOCITY
    };
    match jump {
        Some(j) => {
            let t = fall_time as f32 / 1000.0;
            let vertical = (-j.zspeed - GRAVITY * t).max(-terminal);
            (
                vertical,
                [j.cos_angle * j.xy_speed, j.sin_angle * j.xy_speed],
            )
        }
        None => (0.0, [0.0, 0.0]),
    }
}

/// One packet's step of the landing predictor: `(fall_start_z, descent)`, the descent
/// `takeoff - landing` only on the `FALLING` to grounded edge with a known takeoff.
pub(in crate::net) fn fall_arc_step(
    was_falling: bool,
    now_falling: bool,
    fall_start_z: Option<f32>,
    packet_z: f32,
) -> (Option<f32>, Option<f32>) {
    match (was_falling, now_falling) {
        (false, true) => (Some(packet_z), None), // takeoff
        (true, true) => (fall_start_z, None),    // still airborne
        (true, false) => (None, fall_start_z.map(|start| start - packet_z)), // landing edge
        (false, false) => (None, None),          // grounded
    }
}

impl RemoteMotion {
    /// Whether a scheduled move applies at arrival: due and the queue empty. The empty-queue term
    /// matters because the drain runs after us; applied first, a Stop would be overwritten by an
    /// older queued move. The reference's due test (`@0x618dd2`) reads the manager's clock cell
    /// stamped once per update (`0x616800`), where a due arrival never meets a non-empty queue.
    pub(crate) fn fires_at_arrival(&self, fire_ms: f64, now_ms: f64) -> bool {
        self.pending.is_empty() && fire_ms <= now_ms
    }

    /// One frame of dead-reckoning: `(position, facing, vertical speed, horizontal speed)`.
    pub(super) fn advance(&self, speeds: MoveSpeeds, dt: f32) -> ([f32; 3], f32, f32, f32) {
        // Airborne, the direction and turn flags are ignored.
        if self.flags & move_flags::FALLING != 0 {
            let mut pos = self.wow_pos;
            pos[0] += self.jump_xy_vel[0] * dt;
            pos[1] += self.jump_xy_vel[1] * dt;
            // The local mover's own fall step. `SAFE_FALL` is inside the server-authored merge
            // mask, so this word is the one `0x7c5d23` tests.
            let terminal = if self.flags & move_flags::SAFE_FALL != 0 {
                crate::player::FEATHER_TERMINAL_VELOCITY
            } else {
                TERMINAL_VELOCITY
            };
            let (vertical, mean_vy) =
                crate::player::mover::fall_step(self.vertical_velocity, dt, terminal);
            pos[2] += mean_vy * dt;
            let speed = self.jump_xy_vel[0].hypot(self.jump_xy_vel[1]);
            return (pos, self.orientation, vertical, speed);
        }

        // `TURN_LEFT` raises the facing.
        let mut turn = 0.0f32;
        if self.flags & move_flags::TURN_LEFT != 0 {
            turn += 1.0;
        }
        if self.flags & move_flags::TURN_RIGHT != 0 {
            turn -= 1.0;
        }
        let orientation = self.orientation + turn * speeds.turn_rate * dt;

        // Swimming, the forward axis is pitched: `0x7c5880` writes `(cosY cosP, sinY cosP, sinP)`;
        // the strafe axis stays level.
        let swimming = self.flags & move_flags::SWIMMING != 0;
        let (hp, vp) = if swimming {
            (self.pitch.cos(), self.pitch.sin())
        } else {
            (1.0, 0.0)
        };
        let (fwd, left) = (
            [orientation.cos(), orientation.sin()],
            [-orientation.sin(), orientation.cos()],
        );
        let mut fwd_amt = 0.0f32;
        if self.flags & move_flags::FORWARD != 0 {
            fwd_amt += 1.0;
        }
        if self.flags & move_flags::BACKWARD != 0 {
            fwd_amt -= 1.0;
        }
        let mut left_amt = 0.0f32;
        if self.flags & move_flags::STRAFE_LEFT != 0 {
            left_amt += 1.0;
        }
        if self.flags & move_flags::STRAFE_RIGHT != 0 {
            left_amt -= 1.0;
        }
        let dx = fwd_amt * fwd[0] * hp + left_amt * left[0];
        let dy = fwd_amt * fwd[1] * hp + left_amt * left[1];
        let dz = fwd_amt * vp;

        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        // `GetCurrentSpeed` (`0x7c4c90`), shared with the local controller; its walk arm
        // (`0x7c4d11`) comes before the run arm (`0x7c4d1d`), so a walking backpedal is a walk.
        let base = crate::net::current_speed(&speeds, self.flags);
        let mut pos = self.wow_pos;
        let speed = if len > 1.0e-4 {
            let step = base * dt / len; // normalized
            pos[0] += dx * step;
            pos[1] += dy * step;
            pos[2] += dz * step;
            base
        } else {
            0.0
        };
        // A swimmer's vertical is in the pitched axis, not a persisted velocity.
        (pos, orientation, 0.0, speed)
    }
}

/// A watched player standing in a building whose floor collider has not attached yet: a
/// flag-still mover is not integrated (`0x20ff`), so the missing floor cannot sink it.
#[cfg(test)]
mod under_floor {
    use avian3d::prelude::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;
    use bevy::time::Real;
    use std::time::Duration;

    use super::{extrapolate_remote_units, RelayChain, RemoteMotion};
    use crate::creature_anim::move_flags;
    use crate::player::{PlayerCapsule, CAPSULE_HEIGHT, CAPSULE_RADIUS};
    use benilla_world::collision::ColliderEpoch;

    /// A building in Auberdine: terrain, the floor above it, and a wire Z a hair over the floor.
    const TERRAIN_Y: f32 = 6.98;
    const FLOOR_Y: f32 = 9.06;
    const WIRE_Y: f32 = 9.08;
    /// 60 fps.
    const FRAME: Duration = Duration::from_nanos(16_666_667);

    /// A 10x10 up-wound quad at `y`, a floor the one-sided down-cast stands on.
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

    fn mover(flags: u32, wow_z: f32) -> RemoteMotion {
        RemoteMotion {
            wow_pos: [0.0, 0.0, wow_z],
            orientation: 0.0,
            flags,
            pitch: 0.0,
            speed: 0.0,
            vertical_velocity: 0.0,
            jump_xy_vel: [0.0; 2],
            fall_start_z: None,
            pending: std::collections::VecDeque::new(),
            relay: RelayChain::default(),
            last_apply_ms: 0.0,
            last_apply_pos: [0.0, 0.0, wow_z],
        }
    }

    /// The terrain in, the building's floor not yet, and one mover at the wire's Z.
    fn half_arrived_world(flags: u32) -> (App, Entity) {
        let mut app = App::new();
        // `WorldCollision` needs the trace exclusions the world plugins would initialise.
        app.init_resource::<benilla_world::collision::MoverTraceExclusions>();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>().init_resource::<ColliderEpoch>();
        // An empty liquid world: no water anywhere.
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        app.insert_resource(PlayerCapsule(Collider::capsule(
            CAPSULE_RADIUS,
            CAPSULE_HEIGHT - 2.0 * CAPSULE_RADIUS,
        )));
        // `update()` never runs plugin `finish()`, where avian seats its diagnostics resources.
        app.finish();
        app.cleanup();
        floor_at(&mut app, TERRAIN_Y);
        let e = app
            .world_mut()
            .spawn((mover(flags, WIRE_Y), Transform::from_xyz(0.0, WIRE_Y, 0.0)))
            .id();
        app.update(); // seats Position/Rotation and the collider trees
        (app, e)
    }

    fn frames(app: &mut App, n: usize) {
        for _ in 0..n {
            app.world_mut()
                .resource_mut::<Time<Real>>()
                .advance_by(FRAME);
            app.world_mut()
                .run_system_once(extrapolate_remote_units)
                .expect("extrapolate");
        }
    }

    fn z_of(app: &App, e: Entity) -> f32 {
        app.world().get::<RemoteMotion>(e).unwrap().wow_pos[2]
    }

    #[test]
    fn a_standing_mover_never_leaves_the_z_the_wire_gave_it() {
        let (mut app, e) = half_arrived_world(0);

        // Four seconds before the floor arrives.
        frames(&mut app, 240);
        assert_eq!(
            z_of(&app, e),
            WIRE_Y,
            "a flag-still mover is not integrated: no floor under us is OUR gap, not its business"
        );

        // The building's floor collider attaches, and stamps the world.
        floor_at(&mut app, FLOOR_Y);
        app.world_mut().resource_mut::<ColliderEpoch>().bump();
        app.update();

        // It was already standing on that floor.
        frames(&mut app, 60);
        assert_eq!(
            z_of(&app, e),
            WIRE_Y,
            "the floor arrived under a mover that was already standing on it"
        );
    }

    /// A moving mover walks off the 10x10 quad; with no floor of ours in reach, the wire's height
    /// stands.
    #[test]
    fn a_moving_mover_still_meets_the_world() {
        let (mut app, e) = half_arrived_world(move_flags::FORWARD);
        frames(&mut app, 240);
        assert_eq!(
            z_of(&app, e),
            WIRE_Y,
            "the terrain 2.10 yd down is outside any walking frame's election reach, so there is \
             no floor of ours to find and the wire's height stands — instead of a per-frame \
             descent that nothing here would ever end (2174)"
        );
    }

    /// A moving mover with ground inside the election's reach is put on it.
    #[test]
    fn a_moving_mover_is_still_settled_onto_ground_it_can_see() {
        let (mut app, e) = half_arrived_world(move_flags::FORWARD);
        // At 2 yd/s a frame travels 0.033 yd and reaches `0.033 * 1.8494 + 1/36` = 0.089 yd down.
        app.world_mut().entity_mut(e).insert(crate::net::UnitSpeeds(
            benilla_protocol::events::MoveSpeeds {
                walk: 2.0,
                run: 2.0,
                run_back: 2.0,
                swim: 2.0,
                swim_back: 2.0,
                turn_rate: 0.0,
            },
        ));
        // A floor 0.06 under the wire Z: past the standing slack (0.028), inside the moving reach.
        floor_at(&mut app, WIRE_Y - 0.06);
        app.world_mut().resource_mut::<ColliderEpoch>().bump();
        app.update();
        frames(&mut app, 4);
        let z = z_of(&app, e);
        assert!(
            z < WIRE_Y,
            "the floor is inside a moving frame's reach, so the election still settles the body \
             onto it — 2174 declines the no-FLOOR drop, never the resolve (z={z:.3})"
        );
        assert!(
            z > WIRE_Y - 0.5,
            "…and it settles ONTO that floor rather than carrying on down (z={z:.3})"
        );
    }

    /// The lever sinks a standing mover; it runs only with the env var set, since the lever is a
    /// process-wide `OnceLock`.
    #[test]
    fn the_lever_reproduces_the_sink() {
        if !super::idle_gate_disabled() {
            return; // WOW_REMOTE_IDLE_GATE=off cargo test -p benilla-app the_lever_reproduces
        }
        let (mut app, e) = half_arrived_world(0);
        frames(&mut app, 240);
        // `-- --nocapture` prints the depth.
        println!(
            "pre-1545 leg, 240 frames: z={:.4} (wire {WIRE_Y})",
            z_of(&app, e)
        );
        assert!(
            z_of(&app, e) < TERRAIN_Y + 0.1,
            "the pre-1545 leg walks a standing mover down onto the terrain under the building"
        );
    }
}
