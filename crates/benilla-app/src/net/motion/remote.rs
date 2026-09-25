//! Remote-player dead-reckoning ([`RemoteMotion`], relayed `MSG_MOVE_*`) — the player half of
//! [`super`]'s motion model: flag-driven ground locomotion between the ~2 Hz
//! heartbeats, and the jump as a locally-played ballistic event.

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_protocol::{JumpInfo, MoveSpeeds};
use bevy::prelude::*;
use bevy::time::Real;

use crate::creature_anim::move_flags;
use crate::player::{GRAVITY, TERMINAL_VELOCITY};

use super::super::{ActiveMover, UnitSpeeds};
use super::relay::{PendingMove, RelayChain, RelayMove};
use super::{yaw_of, Spline};

/// The server-authoritative movement state of a *remote* mover (another player), set from each relayed
/// `MSG_MOVE_*` packet ([`benilla_protocol::SessionEvent::UnitMove`]). Between packets — which arrive at
/// the mover's heartbeat rate (~2 Hz) plus each transition — [`extrapolate_remote_units`] integrates the
/// pose from `flags` so motion is smooth rather than a 2 Hz snap. Holds the canonical WoW-space pose (the
/// entity `Transform` is derived from it each frame); a packet overwrites it (a correction/snap). Not
/// added to our own avatar (the controller drives that) nor to creatures (they ride a server [`Spline`]).
///
/// **A jump is a ballistic event, not flag-driven walking**: while `JUMPING`
/// ([`move_flags::FALLING`]) is set, the horizontal velocity is *frozen* at the launch
/// ([`Self::jump_xy_vel`]) and the height follows a parabola under gravity ([`Self::vertical_velocity`])
/// — the launch played out locally — rather than the ground locomotion the direction flags imply. Each
/// relayed jump packet re-seeds both from its [`JumpInfo`] tail (a correction); a non-jumping packet
/// (e.g. `FALL_LAND`) clears them and resumes ground extrapolation.
#[derive(Component, Clone)]
pub(crate) struct RemoteMotion {
    /// Last authoritative position (raw WoW yards), advanced by extrapolation between packets.
    pub(crate) wow_pos: [f32; 3],
    /// Facing (WoW orientation, radians), advanced while a `TURN_*` flag is set (on the ground).
    pub(crate) orientation: f32,
    /// Live CMovement `moveFlags` (matches [`move_flags`]) — the direction/mode the mover last reported.
    pub(crate) flags: u32,
    /// The swim pitch (radians, +up) the mover last reported — the `MovementInfo` tail present while
    /// `SWIMMING` is set (`0.0` otherwise). The swim dead-reckon applies it the way the client's swim
    /// velocity basis does (`0x7c5880`, pitch folded into the travel direction): vertical
    /// `sin(pitch)·swim speed`, horizontal scaled by `cos(pitch)`.
    pub(crate) pitch: f32,
    /// Current horizontal ground speed (yd/s) the extrapolation is applying — read by the animation
    /// selector ([`crate::creature_anim`]) to choose + rate-scale the gait, the way a [`Spline`]'s speed
    /// does for a creature.
    pub(crate) speed: f32,
    /// Current vertical speed (yd/s, WoW +Z up) while airborne — seeded from a jump packet's `zspeed`
    /// minus gravity over its `fall_time`, then integrated down by gravity each frame. `0` on the ground.
    pub(crate) vertical_velocity: f32,
    /// Frozen horizontal velocity (world XY yd/s) during a jump — `(cos, sin)·xyspeed` from the launch.
    /// Replaces the flag-driven horizontal while `JUMPING` (you can't change direction mid-air). `[0; 2]`
    /// on the ground.
    pub(crate) jump_xy_vel: [f32; 2],
    /// WoW-Z of this airborne arc's takeoff — snapshotted when a packet first sets `FALLING`, held
    /// across the arc, cleared on landing. `None` on the ground (or if the mover was already airborne
    /// when it entered view — no takeoff seen, so no fall-height reference). Feeds the remote
    /// **landing predictor**: on the `FALLING → grounded` edge, `fall_start_z − landing_z` is the fall
    /// height that gates the grunt + dust puff (the launch-height apex proxy the
    /// self-player path uses, applied identically to observed movers).
    pub(crate) fall_start_z: Option<f32>,
    /// Not-yet-due relayed moves, fire-time ascending — the reference's per-unit move-event queue
    /// (`CMovement+0x150`): a remote's packet is **scheduled**, not applied at arrival (decision
    /// 0601). [`drain_pending_moves`] applies each head when the
    /// clock reaches its [`PendingMove::fire_ms`]; until then the dead-reckon covers the mover's
    /// own timeline, so the residual at apply time is structurally small.
    pub(crate) pending: std::collections::VecDeque<PendingMove>,
    /// This mover's replay chain — the per-unit timing cells that pick each packet's fire-time
    /// ([`super::relay`]). Per unit, exactly as the reference holds them on the
    /// unit's own CMovement.
    pub(crate) relay: RelayChain,
    /// Real-time ms when a packet last applied to this mover (`0.0` until the first one), and the
    /// position it applied at. The dead-reckon's anchor — everything since is *our extrapolation*,
    /// not anything the server said. Read by the runaway watch in [`extrapolate_remote_units`].
    pub(crate) last_apply_ms: f64,
    pub(crate) last_apply_pos: [f32; 3],
}

/// What [`crate::net::apply`]'s `unit_move` did with an inbound relayed move — the `out=` field of
/// the `rly` trace. Every arrival gets a line, **including the ones we discard**: "the packet never
/// showed up" and "the packet showed up and lost" are different bugs with the same symptom, and the
/// trace has to tell them apart.
#[derive(Clone, Copy)]
pub(in crate::net) enum RelayOutcome {
    /// Applied at arrival — it was due and the unit's queue was empty.
    Now,
    /// Scheduled: waiting in the unit's queue for its fire-time.
    Queued,
    /// The unit's first packet: seeds the chain and places it immediately.
    Seed,
    /// **Ours** — a server-authored pose for our own mover (`.go forward`, `.cheat fly`, an
    /// anticheat snap-back). Handed to the controller as a [`crate::net::SelfMoveMessage`] rather
    /// than applied down this lane; the reference has no mover-guid gate at all.
    SelfMover,
    /// **No entity for this guid.** The mover isn't in our object index — so this packet, Stop or
    /// not, changes nothing. A stale mover that keeps running while these accumulate is a streaming
    /// bug, not a replay bug.
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

/// One line of the `WOW_MOVE_TRACE` sink per inbound relayed move (tag `rly`), so replay is
/// **measured** rather than eyeballed (method §4): the mover, its wire stamp, the move-flags the
/// packet carries, what we did with it, the lead the schedule gave it (`fire − arrival`, ms), and how
/// deep the unit's queue is. Together with the sender's `snd` lines on the other client, a run's
/// `rly` lines answer the only question that matters when a mover misbehaves: *did the packet arrive,
/// and did it win?* Costs one `OnceLock` read per packet when the trace is off.
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
    // One tag per verb, `tp` and `rt` included: a mover that appears in the wrong place, or keeps
    // sliding through a root, is the report where "did that packet arrive?" is the whole question,
    // and the trace has to answer it without a second run.
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

/// How long a mover may dead-reckon with **nothing queued** before the runaway watch starts
/// reporting it (ms). A moving unit is normally fed every frame while it turns and at worst every
/// 500 ms by the heartbeat, so two seconds of silence with a direction flag still
/// live means we are inventing motion the server never described.
const RUNAWAY_SILENCE_MS: f64 = 2000.0;

/// The **runaway watch** (tag `run`): one line per silent second per mover that is still moving under
/// our own extrapolation with an empty queue — its flags, how long since anything applied, and how
/// far we have carried it from the last position the server actually gave us.
///
/// This is the instrument the "he keeps running off into the distance" report needed: it timestamps
/// the moment precisely, so the mover client's `snd` lines at that instant say whether the Stop was
/// ever sent, and the observer's `rly` lines say whether it arrived. Nothing in the client — or in
/// the reference — otherwise notices a mover that has travelled a hundred yards on dead reckoning
/// alone. Reporting only; it never corrects the pose.
fn trace_runaway(guid_hint: Entity, rm: &RemoteMotion, now_ms: f64, silent_s: u32) {
    let d = [
        rm.wow_pos[0] - rm.last_apply_pos[0],
        rm.wow_pos[1] - rm.last_apply_pos[1],
    ];
    // The inbound census rides every line, because it is the discriminator: a starving mover with
    // packets still landing means the **server** stopped relaying that unit; a starving mover with
    // the whole census frozen means the **socket** died with nobody noticing.
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

/// How often the remote-pose watch samples a mover (ms) — see [`trace_remote`].
const REMOTE_TRACE_MS: f64 = 500.0;

/// What the swim-pitch render law will actually *present* for this mover — `rm.pitch` passed
/// through [`crate::creature_anim::swim_body_rotation`]'s gate, so the `rem` line reports the two
/// numbers side by side: what the wire said (`pitch=`) and what the body is tilted by (`tilt=`).
///
/// The pair is the whole point. A reported pitch with a zero tilt is the gate refusing (idle or
/// strafe-only swim, or not swimming at all); a zero on *both* is a mover whose client never sent
/// a pitch. Before this, neither number appeared anywhere — the observed-swimmer tilt landed in
/// July 2026 with no trace and no test, so "do other swimmers tilt?" was a
/// question only the director's eye could answer.
fn rendered_pitch(rm: &RemoteMotion) -> f32 {
    let (_, x, _) = crate::creature_anim::swim_body_rotation(0.0, rm.flags, rm.pitch)
        .to_euler(bevy::math::EulerRot::YXZ);
    x
}

/// The **remote-pose watch** (tag `rem`): one line per mover per [`REMOTE_TRACE_MS`], reporting what
/// the dead-reckon asked for and what the world gave back. Until decision 0626 nothing measured the
/// *pose* of a watched player at all — `rly` measures the packets that arrive and `run` only fires
/// when a mover starves — so "he sinks into the ground / into the wall and pops back out" had no
/// instrument behind it and could only be argued from reading code. Two numbers close that:
///
/// - **`dz`** — the height the mover gained or lost **this frame**, *excluding* the packet applied
///   this frame (the drain runs first, so the anchor already carries it). A grounded mover riding the
///   surface moves in Z on every sloped frame; before 0626 the only thing that could move it was the
///   pre-fire reconcile lerp, which *this client* then wrongly skipped for heartbeats (decision
///   2064: tag `0x26` is the teleport's, not the heartbeat's) — so a mover running *straight* read
///   `+0.000` sample after sample and took its height as a 2 Hz snap, which is the "delayed terrain
///   snap" report. Read it against `age`: a nonzero `dz` at a large `age` is the ground, not the
///   server.
/// - **`held`** — how much of this frame's intended horizontal travel the world took away. Sustained
///   nonzero is a mover being *held* against a wall by our colliders (right); a flat zero for a mover
///   whose own client is stopped dead is the dead-reckon marching into the geometry (wrong), and it
///   is the "sinks in and pops out again and again" report.
///
/// - **`drop`** — how far **below the Z its last packet carried** we are currently drawing this
///   mover (`last_apply_pos[2] − wow_pos[2]`; negative means we are drawing it *above*). This is
///   the number that falsifies the whole under-the-floor class, and `rem` shipped without it:
///   `dz` is a per-frame delta, so a 1/36-yd-a-frame creep through a WMO floor reads as an
///   ordinary healthy ground-follow sample after sample while it accumulates into decision 1545's
///   measured 2.14 yd. A sustained nonzero `drop` on a mover whose `age` keeps climbing is us
///   inventing a height the server never described.
///
/// - **`pitch` / `tilt`** — what this mover *reported* as its swim pitch, and what the body-pitch
///   render law actually *presents* for it ([`rendered_pitch`]). The pair, not either alone: a zero
///   tilt is correct three ways (an idle floater, a strafe-only swim, dry land), so a single number
///   makes every one of those look like the defect the director reported. Reported-nonzero with
///   tilt-zero is the gate refusing; zero on both is a mover whose client never sent a pitch.
///
/// `age` (ms since a packet last applied) rides along so a sample is never read as server truth.
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

/// `WOW_REMOTE_SNAP=1` — the A/B escape: apply every relayed move at arrival as a raw snap
/// (pre-0601 behavior), bypassing the scheduled queue and the reconcile lerp.
pub(in crate::net) fn arrival_snap() -> bool {
    static SNAP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SNAP.get_or_init(|| std::env::var_os("WOW_REMOTE_SNAP").is_some())
}

/// `WOW_REMOTE_FLAT=1` — the other A/B escape: dead-reckon a remote mover **without
/// the world**, the pre-0626 behaviour — height frozen at the last packet's Z, the step unswept.
/// Restores both defects on demand (a watched player sinking into rising ground and floating over
/// falling ground; a mover marching into the wall its own client is stopped at), which is what makes
/// the fix measurable side by side rather than asserted — and what re-answers "is the ground resolve
/// still doing anything?" for one env var instead of a bisect. Its twin above earns its keep the
/// same way.
fn flat_extrapolation() -> bool {
    static FLAT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAT.get_or_init(|| std::env::var_os("WOW_REMOTE_FLAT").is_some())
}

/// The A/B switch behind decision 1545: `WOW_REMOTE_IDLE_GATE=off` resolves **every** remote mover
/// against the world, flag-still ones included — the pre-1545 leg, where a watched player standing
/// inside a building whose floor collider has not attached yet is walked down through the floor at
/// `STEP_SNAP_SLACK` (1/36 yd) a frame — 1.67 yd/s at 60 fps — and left on the terrain under
/// it for the session. Kept as the lever that reproduces B197's player site on the *fixed* binary,
/// the twin of [`flat_extrapolation`] and `WOW_CLAMP_SEAT=off` (1384), and for the same
/// reason: the fix's evidence never has to depend on two different builds.
fn idle_gate_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        std::env::var("WOW_REMOTE_IDLE_GATE").is_ok_and(|v| matches!(v.as_str(), "off" | "0"))
    })
}

/// The reconcile-arm tolerance (squared yards): predicted-vs-event disagreement below this needs
/// no correction. The reference's `0x80c744` = 7.716e-4 ≈ (0.0278 yd)², compared in 2D — Z joins
/// only while SWIMMING (`0x619090`).
const RECONCILE_TOL_SQ: f32 = 7.716e-4;

/// One frame of the pre-fire reconcile (the reference's `0x619090` arm + `0x6191c0` lerp): if
/// `predicted` (the dead-reckoned pose at the event's fire-time) misses `target` by ≥ the
/// tolerance (2D; Z joins while `swimming`), blend `pos` toward `target` by this frame's share of
/// the time left — linear in time, landing exactly on `target` as the clock reaches the
/// fire-time. Below tolerance the prediction agrees and `pos` returns untouched.
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
    // This frame spans dt of the (dt + remaining) window from the previous frame to the fire.
    let f = dt / (dt + remaining_s);
    for (p, t) in pos.iter_mut().zip(target) {
        *p += (t - *p) * f;
    }
    pos
}

/// The remote facing-interp dead-zone: an angular step below this isn't worth turning for — the
/// reference's `0x8026bc` = 9.5367e-7 guard on the `0x618f80` angular velocity.
const FACING_DEAD_ZONE: f32 = 9.5367e-7;

/// One frame of the pre-fire facing interp — the reference's remote facing smoothing
/// (`0x618f80` shortest-arc ω into `+0x144`, integrated by `0x7c4f30`: the **only** smoothed
/// facing path a remote unit has; every other facing write is a snap).
/// Rotate `orientation` along the shortest arc toward the queued event's `target`,
/// linear in time so it lands exactly as the clock reaches the fire-time; the apply then snaps
/// the (structurally zero) remainder and clears the interp, as `0x617e90` zeroes `+0x148`. The
/// ±π fold picks the short way around; the dead-zone skips a negligible turn.
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

/// Apply one relayed move to a unit — the pose snap + integrator re-seed the reference performs
/// at arrival (`0x7c6420`) or at scheduled fire (`0x617e90` → `0x7c69a0`): position/facing/flags
/// committed outright onto the ONE simulated pose, ballistic re-seeded from the jump tail, the
/// landing predictor stepped, and the rider tail re-anchored. Shared by the
/// arrival path ([`crate::net::apply`]'s `unit_move`) and the queue drain ([`drain_pending_moves`]).
pub(in crate::net) fn apply_move(
    e: Entity,
    ev: &RelayMove,
    rm: &mut RemoteMotion,
    now_ms: f64,
    commands: &mut Commands,
    landings: &mut MessageWriter<crate::creature_anim::HardLanding>,
) {
    use crate::creature_anim::move_flags::FALLING;
    // The rider tail: a mover ON a transport carries its local pose — store it so
    // `compose_riders` re-anchors it through the boat's live matrix each frame; a tail-less
    // packet from a known rider means they stepped off.
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
    // The remote landing predictor: on the FALLING → grounded edge the fall
    // height gates the grunt + dust puff, exactly as the self controller does for us.
    let was_falling = rm.flags & FALLING != 0;
    let (new_start, descent) =
        fall_arc_step(was_falling, now_falling, rm.fall_start_z, ev.position[2]);
    rm.fall_start_z = new_start;
    if let Some(descent) = descent {
        landings.write(crate::creature_anim::HardLanding { entity: e, descent });
    }
    rm.wow_pos = ev.position;
    rm.orientation = ev.orientation;
    // **A merge, not an assignment**: the reference folds a relayed word through
    // `0x75a07dff` at `0x618de7`, keeping the client-owned bits outside it — `ON_TRANSPORT` above
    // all, which is why a relayed pose relocates a rider without deboarding them. Today every bit
    // benilla holds on a watched mover comes from the wire and lands inside the mask, so this
    // changes no behaviour; it stops the next bit added from silently being the one that does.
    rm.flags = move_flags::merge_server_authored(rm.flags, ev.flags);
    // …and then the ONE verb whose opcode outranks the word it arrived with. `SetRoot 0x7c7340`
    // sets `0x1000` and wipes the motion bits once (`& 0xffe07f00` — this is what stops a rooted
    // body coasting); `ClearRoot 0x7c7370` only clears the bit and invents no movement. Both run
    // unconditionally after the merge, so a root holds whether or not the server was careful to
    // send an already-wiped word.
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
    // The dead-reckon's anchor: when this mover was last *told* anything, and from where. Read only
    // by the runaway watch ([`trace_runaway`]) — a mover extrapolating far past its last packet is
    // the shape of every "he ran off into the distance" report.
    rm.last_apply_ms = now_ms;
    rm.last_apply_pos = ev.position;
}

/// Fire every due queued move (the reference's drain `0x615c30`: bail while `now < ev[+8]`,
/// dequeue + dispatch once due). Runs before [`extrapolate_remote_units`], which then advances
/// the freshly-applied state and runs the pre-fire reconcile lerp against the next queued head.
///
/// **REAL time, deliberately**: this is a replay clock paced against the server's
/// stamps, and the reference's is the OS wall clock (`0x42c010`, a QPC-derived ms counter). Bevy's
/// virtual clock clamps every frame delta to `max_delta` (250 ms), so under macOS occlusion
/// throttling (~1 fps for a backgrounded client — i.e. exactly the side-by-side A/B against the
/// reference) it falls minutes behind real time and drags the whole replay schedule with it. Same
/// lesson, same fix as the UI script clock (`ui_script/extract`).
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

/// Integrate every remote mover's pose from its [`RemoteMotion`] each frame, so another player walks
/// *smoothly* between the sparse relay packets instead of snapping at the ~2 Hz heartbeat rate. The
/// horizontal velocity is derived from the live `moveFlags` in the mover's facing frame at its run /
/// run-back / swim speed; the `TURN_*` flags rotate the facing at `turn_rate`. A creature [`Spline`]
/// (server-authored path) and the body we are steering ([`ActiveMover`]) are excluded — they have their own
/// motion source.
///
/// **The step is then resolved against the world** ([`crate::player::mover::grounded_step`], decision
/// 0626) — the same swept capsule, colliders and step-vs-fall election our own avatar walks on,
/// because the reference drives every mover through one controller. Height therefore
/// comes from the ground under the mover **every frame**, and the dead-reckon cannot walk a watched
/// player into geometry.
///
/// **…for a mover the reference integrates at all**. One controller, every mover —
/// but a mover with none of [`move_flags::INTEGRATED`] (`0x20ff`) set does not reach that controller
/// in the first place: `CMovement::Update`'s substep loop and the manager's per-mover tick both bail
/// on the same mask, and a flag-less unit is not even in the mover list. So a *standing* watched
/// player keeps its last packet's pose verbatim — which is also the only safe answer, because the
/// resolve has anything to correct only when our world is missing a floor the server has, and a
/// building's late collider is exactly that.
///
/// Without it the extrapolation ran in a vacuum, and the two symptoms that follow are the ones the
/// director reported. **Height:** [`RemoteMotion::advance`] never moves a grounded mover's Z, so Z
/// changed only where a *packet* put it — at the apply, or dragged there by the pre-fire reconcile
/// lerp. The lerp skipped heartbeats *in this client* — a mis-attribution corrected by 2064, where
/// the reference skips the teleport instead — and a mover running **straight** sends nothing else
/// that carries position (0617's frame-cadence stream is `SET_FACING`, sent only while turning), so
/// a straight run gave a 2 Hz height staircase under a continuous XY, which sinks a watched player
/// into ground that rises under them and floats them over ground that falls away, at the same rate.
/// (The clamp is still the answer — our floor and the server's differ, which no blend can fix — but
/// that particular staircase had a second cause, and it was ours.) **Geometry:** a
/// mover held against a wall by its own client keeps reporting `FORWARD` with an unchanged position —
/// there is no "I am blocked" bit on the wire, and there needn't be, because the watching client is
/// meant to be stopped by the same wall. Ours wasn't: it advanced into the geometry for the whole
/// packet interval, and the next packet popped it back out.
///
/// This is the client's own dead-reckoning, in miniature: extrapolate from the last reported state,
/// snap to the truth when the next packet lands. The pose lives canonically in WoW space on the
/// component; the [`Transform`] is derived from it (translation + facing only — scale is preserved).
///
/// **REAL time, deliberately** — the same clock the replay schedule runs on
/// ([`drain_pending_moves`]); a dead-reckon that advanced on a different (virtual, clamped) clock
/// than the fire-times it converges toward would never land on them.
#[allow(clippy::type_complexity)]
pub(in crate::net) fn extrapolate_remote_units(
    time: Res<Time<Real>>,
    mut commands: Commands,
    // Avian's kinematic move-and-slide + the player-body capsule — the *same* pair the local
    // controller sweeps: one controller, every mover.
    world: benilla_world::collision::WorldCollision,
    // The liquid query, for a water-walking mover's surface. Liquid is asked on our
    // side rather than swept, so — exactly as the local controller does — the plane has to be handed
    // to the step ([`crate::player::mover::Support::water`]).
    points: benilla_world::world_point::WorldPoint,
    capsule: Res<crate::player::PlayerCapsule>,
    // The runaway watch's per-mover throttle: the last whole silent second reported, so a stuck
    // mover writes one line a second rather than one a frame. Trace-only, hence a `Local` and not
    // state on the component.
    mut warned: Local<bevy::platform::collections::HashMap<Entity, u32>>,
    // The remote-pose watch's per-mover throttle: when each mover last wrote a `rem` line.
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
            // Trace-only: the mover's wire guid, so a `rem` line can be JOINED to the `rly` lines
            // that fed it. `rem` printed a Bevy `Entity` and nothing else, which correlates with
            // nothing in the file — not `rly`'s `guid=`, not the sender's own trace, not a `.gps`
            // report — so two movers in one capture could not be told apart at all. `spl` has
            // carried its guid since it was written; this was the odd one out.
            Option<&crate::net::Guid>,
            // The server-granted modes this unit was handed by the `SMSG_SPLINE_MOVE_*` family.
            // Normally empty on a relayed **player** — vmangos only broadcasts
            // that family for a unit it is driving itself — which is exactly why it is OR'd with
            // the pose's own flags below rather than replacing them: for a player the modes ride
            // the pose (they are inside the server-authored merge mask), for a creature they ride
            // this component, and the union is the reference's one `CMovement+0x40` word.
            Option<&crate::net::UnitMoveModes>,
        ),
        (Without<Spline>, Without<ActiveMover>),
    >,
) {
    use crate::creature_anim::{ease_strafe_yaw, strafe_body_offset, wrap_pi};
    let dt = time.delta_secs();
    let now_ms = time.elapsed_secs_f64() * 1000.0;
    for (e, mut t, mut rm, speeds, twist, latched, riding, guid, granted) in &mut q {
        // The runaway watch (trace-only): a mover still carrying a direction flag, with nothing
        // queued behind it and nothing applied for seconds, is running on our extrapolation alone.
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
        // How much of the frame's intended horizontal travel the world took away — measured at the
        // resolve, before the reconcile lerp, so it is the *collision's* doing and not the
        // correction's. Stays 0 for a mover the resolve skips (swimming, on a boat).
        let mut held = 0.0f32;
        // **The step meets the ground**. What [`RemoteMotion::advance`] produced is
        // our *invention* — the mover's own client has not told us anything since the last packet —
        // and an invention that ignores the world is what a watched player sinking into a hillside
        // and popping back out actually is. Run it through the local controller's own grounded
        // resolve: swept capsule (so a mover held against a wall by its own collision is held
        // against ours too, instead of marching into it) and the step-vs-fall election (so height
        // comes from the surface every frame, instead of standing frozen at the last packet's Z
        // until the next one snaps it). The reference makes no distinction here — one controller
        // integrates and commits every mover.
        //
        // **An airborne arc resolves too — but only against walls**. A jump owns its
        // Z (the ballistic arc is the whole point), so it gets no election snap and no step-up; what
        // it does get is the same swept capsule, because a watched player who jumps into a building
        // has to be stopped by it. Unswept, our invented arc carried them *inside* the wall for the
        // length of the jump and the landing packet popped them back out — the airborne half of the
        // very same defect, closed by the local controller's own airborne slide
        // ([`crate::player::mover::airborne_step`]).
        //
        // Still excluded: a **swimmer** (the wire Z is its depth in the water volume, not a surface
        // to resolve against) and a **transport rider** (its pose is transport-local; `compose_riders`
        // re-anchors it through the boat's live matrix each frame, and a world-space resolve would
        // fight it).
        let airborne = rm.flags & move_flags::FALLING != 0;
        let afloat = rm.flags & move_flags::SWIMMING != 0;
        // …and **a flag-still mover is not integrated at all** — the reference's
        // own gate, [`move_flags::INTEGRATED`] = `0x20ff`: `CMovement::Update`'s substep loop
        // (`0x616e20`) and the manager's per-mover tick (`0x6166f5`) both bail on it: such a unit
        // is not even in the mover list. Its pose is the last packet's, verbatim.
        //
        // That is not merely faithful, it is the only safe answer, because **our world can be
        // missing a floor the server has**: the WMO's collider attaches structurally later than the
        // terrain under it (1384 §1 — the ADT decode *starts* the WMO load, `finish_colliders`
        // heads the chain, and the paced form furnisher gates the bake; 1303 measured the gap in
        // seconds at a city pin). Standing still the election's reach collapses to
        // `STEP_SNAP_SLACK` (1/36 yd), and `grounded_step`'s no-hit branch spends that whole
        // reach *descending* — right for the local mover, whose next frame elects a real fall, and
        // open-loop for a remote, which has no fall election at all. So the resolve walked a
        // watched player through the floor at 1/36 yd a frame — 1.67 yd/s at 60 fps, 3.3 at 120 —
        // onto the terrain below, where the (now deleted) settled memo froze them for the session.
        // A standing player sends no packets, so nothing ever re-seated them: they stayed sunk
        // until they moved, and then popped back up.
        //
        // Deleting the memo with the gate is the point, not a side effect: it existed only to skip
        // this resolve for a flag-still mover whose answer was proven identical (1473 §3), and the
        // gate skips it strictly earlier and for a stated reason — taking the last un-dated
        // collision cache (1384's part 3, never fixed on this lane) with it.
        //
        // **This mover's whole `MOVEMENTFLAGS` word, as the reference holds it**:
        // the pose's own flags plus whatever the `SMSG_SPLINE_MOVE_*` family granted. Read by the
        // ground step's `Support` below — not by the integration gate on the next line, which is
        // the reference's own `0x20ff` over the *pose's* direction bits and none of the modes.
        let modes = rm.flags | granted.map_or(0, |m| m.0);
        let integrating = rm.flags & move_flags::INTEGRATED != 0 || idle_gate_disabled();
        if integrating && !afloat && !riding && !flat_extrapolation() {
            let half_h = Vec3::Y * (crate::player::CAPSULE_HEIGHT * 0.5);
            let from = wow_to_bevy(rm.wow_pos) + half_h;
            // The frame's velocity by construction. Grounded, [`RemoteMotion::advance`] never moves
            // Z (the surface does, in the snap), so it is purely horizontal; airborne, the arc's
            // vertical rides along so the sweep sees the real displacement.
            let vel = if dt > 1.0e-6 {
                (wow_to_bevy(pos) + half_h - from) / dt
            } else {
                Vec3::ZERO
            };
            let resolved_center = if airborne {
                crate::player::mover::airborne_step(&world, &capsule.0, from, vel, time.delta())
            } else {
                // **The observed mover's own granted modes, honoured** (0866 built
                // the family for our own mover and left this site reading `Support::default()`,
                // which drew a hovering watched player a yard low and sagged a water-walker
                // through the surface between packets). `modes` is the union of the pose's flags
                // and the `SMSG_SPLINE_MOVE_*` component, which is the reference's single
                // `CMovement+0x40` word; see the query docs for why a union is exact here.
                //
                // Both halves are the local controller's, unchanged: [`Support::offset`] is
                // HOVER's yard (`0x636e81`) and [`Support::water`] is the liquid plane
                // `MOVEFLAG_WATERWALKING` ORs into the reference's trace mask (`0x63162e`), which
                // ours has to be handed because liquid is queried rather than swept.
                //
                // `swimming` is read off the same word — the water plane is **not** floor while
                // swimming (`0x631617`: the arm is taken only when the swim bit is
                // clear), which is what keeps a water-walker who is already in deep water from
                // being popped onto the surface. The pitch gate the local mover applies is the
                // *aim* pitch of a diving player and has no meaning for an observed one, so it is
                // deliberately not reproduced: `water_floor`'s three gates are asked here as the
                // two that apply.
                //
                // Still **no carried steep-support bit** (1129): it is per-frame state the
                // dead-reckon does not keep between packets, so a watched player descending a
                // steep face gets the ordinary cone reach, and the next packet's wire Z corrects
                // whatever that misses.
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
                        // A watched player is a player body: `0x5fa550` is TRUE for an
                        // uncharmed player object, so its `H` is the player's own 1.0.
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
                // **The no-floor drop is declined here too** (closing 1545's own
                // residual). 1545 named this exact defect — *"`grounded_step`'s no-hit branch
                // spends that whole reach descending — right for the local mover, whose next frame
                // elects a real fall, and open-loop for a remote, which has no fall election at
                // all"* — and closed only the flag-still case with the `INTEGRATED` gate; a mover
                // that is *moving* kept ratcheting. A miss means our world could not answer, so the
                // frame keeps the height the last packet gave and takes only the horizontal.
                match g.unsupported {
                    Some(_) => Vec3::new(g.center.x, from.y, g.center.z),
                    None => g.center,
                }
            };
            let resolved = bevy_to_wow(resolved_center - half_h);
            held = (pos[0] - resolved[0]).hypot(pos[1] - resolved[1]);
            pos = resolved;
        }
        // The pre-fire reconcile toward the queued head:
        // - **Facing** interpolates toward a NON-heartbeat event's facing (the reference's
        //   `0x618f80` ω armed by `0x619030`, integrated by `0x7c4f30` — its only smoothed
        //   facing path; a mouse-turning mover streams facing in SET_FACING packets, so without
        //   this a watched turn snaps per-packet — the director's "snappy turning").
        // - **Position** (the 0x619090 arm + 0x6191c0 lerp): while a NON-heartbeat event waits and
        //   the prediction at its fire-time would miss its position by ≥ the 0.0278-yd tolerance,
        //   blend the SIMULATED pose so it lands on the event position at the fire-time.
        // **Exactly ONE relay is excluded from both arms, and it is the TELEPORT** — the queued
        // node's tag `0x26` that `0x619030 @0x61904b` and `0x619090 @0x6190bb` bail on. Decision
        // 2064 corrects 0601/0603, which had that tag down as the heartbeat's: a blink is the one
        // pose smoothing cannot help, because blending toward a discontinuity walks the mover —
        // swept capsule and all — across the gap the teleport exists to skip. A deferred heartbeat
        // blends like anything else.
        if let Some(ev) = rm.pending.front() {
            let remaining_s = ((ev.fire_ms - now_ms) / 1000.0) as f32;
            if remaining_s > 0.0 && ev.mv.reconciles() {
                orientation = facing_lerp(orientation, ev.mv.orientation, dt, remaining_s);
                // Predict from the pre-frame state to the fire-time (this frame's dt + what's left).
                let (predicted, ..) = rm.advance(s, dt + remaining_s);
                let swimming = ev.mv.flags & move_flags::SWIMMING != 0;
                pos = reconcile_lerp(pos, predicted, ev.mv.position, swimming, dt, remaining_s);
            }
        }
        // The standing mouse-turn shuffle: a mouse-turning mover streams NO turn flag — only its
        // SET_FACING packets — so while the pre-fire facing blend is still covering meaningful
        // yaw on a stationary, grounded mover, latch [`super::FacingStep`] and the anim layer
        // plays ShuffleLeft/Right exactly as the idle re-face does. (In the reference the
        // standing turn-anim is the display-facing chase's toggle — `0x607ed0` → `+0xd58`
        // `0x800/0x1000` → `0x712090`, anims 11/12 — not the movement interp, whose integrator
        // doesn't run flag-less; our display-facing layer models the chase with this latch, the
        // same confirmed outcome. A keyboard turner needs none of it — its TURN flags pick the
        // shuffle already.) Dropped the frame the body stops moving.
        //
        // **The yaw this frame APPLIED, against the ±1e-5 sign band** — the client's own latch
        // input and its own band (`0x607ed0` `60843b`–`608473`), not the gap still
        // to cover measured against an eyeballed ~3°. `facing_lerp` above is what moved it; with
        // nothing queued it does not move at all, so the `pending` test the old form needed is
        // carried by the quantity itself.
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
        // The remote-pose watch (trace-only): one sample per mover per half-second.
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
        // Compare-then-write, all three (1362's no-op-write law, as `spline.rs`'s clamp and the
        // camera seat already do): the loop visits every remote mover, and a flag-still one —
        // the whole idle population of a city — reproduces last frame's pose bit for bit. Writing
        // it anyway marked every such body's `Transform` changed on every frame: its whole model
        // subtree re-propagated, the visibility walk's per-part skip (1979) missed on every one
        // of its parts, and the swim-mark and splash gates re-asked the water for it.
        let translation = wow_to_bevy(pos);
        if t.translation != translation {
            t.translation = translation;
        }
        // The strafe body pose, same as our own avatar's (the client's display-facing blend): a
        // strafing remote player renders its body at `orientation ± 90°/45°`, eased in aim-relative
        // offset space (a left↔right flip swings around the front, never the 180°-tie back path),
        // with the SpineLow/Head counter-twist walking the upper body back onto its aim.
        // SWIMMING snaps the display facing to the aim instead — no strafe offset, no ease (the
        // client's facing SNAP list: dead or swimming, `0x607ed0`'s
        // `mov [esi+0xc94],[esi+0xc98]`) — same gate the local controller applies.
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
        // The swim body pitch — [`crate::creature_anim::swim_body_rotation`], the same law our own
        // avatar's pose owner calls, on the pitch this mover reported. The reference's render law
        // (`0x60a110`) takes the observed mover's *reported* pitch here, exactly as the local one
        // takes its own.
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

/// The ballistic seed a relayed jump packet implies: the current vertical speed (yd/s, **+Z up**) and
/// the frozen horizontal velocity `(cos, sin)·xyspeed` (world XY). `None` (a non-jumping packet — a
/// ground move or `FALL_LAND`) → grounded: zero vertical, no horizontal freeze.
///
/// **The wire `zspeed` is *down-positive*** — the real 1.12.1 client sends `-7.955547` for a rising
/// jump (VERIFIED, vanilla-sniffs `dwarf_rogue_dun_morogh` MSG_MOVE_JUMP; vmangos likewise forces
/// `+7.958` *up* via the opcode, discarding the wire value). So the take-off **up**-speed is `-zspeed`,
/// and the current up-speed is `-zspeed - g·t`. Mirrors vmangos `Unit.cpp` `ExtrapolateMovement`
/// (`z = start.z + jumpInitialSpeed·t - ½g·t²`, `jumpInitialSpeed = -zspeed`) under the same `gravity`
/// (sign corrected by the sniff).
///
/// `feather` picks the clamp the recovered speed lands on — the reference's `0x7c5d20` chooses
/// `[0x87d898]` = 7.0 over `[0x87d894]` = 60.148 on `MOVEFLAG_SAFE_FALL`, and it is the *mover's*
/// bit, so a watched Slow Fall recovers to 7 yd/s exactly as ours does.
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

/// The remote landing predictor's per-packet arc step — pure so it's unit-tested.
/// Given the mover's prior/new `FALLING` state, the takeoff Z tracked so far, and this packet's Z,
/// return `(new fall_start_z, landing descent)`. `descent` is `Some(fall height)` **only** on the
/// `FALLING → grounded` edge with a known takeoff — the value that gates the grunt + dust puff. WoW
/// Z is up, so the height is `takeoff − landing`; `wow_to_bevy` preserves that magnitude, matching
/// the self path's Bevy-Y descent. A mover that entered view mid-fall (no takeoff seen) yields
/// `None` and simply doesn't predict for that arc.
pub(in crate::net) fn fall_arc_step(
    was_falling: bool,
    now_falling: bool,
    fall_start_z: Option<f32>,
    packet_z: f32,
) -> (Option<f32>, Option<f32>) {
    match (was_falling, now_falling) {
        (false, true) => (Some(packet_z), None), // takeoff: this arc's launch height
        (true, true) => (fall_start_z, None),    // still airborne: keep the reference
        (true, false) => (None, fall_start_z.map(|start| start - packet_z)), // landing edge
        (false, false) => (None, None),          // grounded: nothing to track
    }
}

impl RemoteMotion {
    /// Does a freshly-scheduled move apply **at arrival**, or go on the unit's queue?
    ///
    /// Due (`fire ≤ now`) **and the queue empty**. Fire-times are monotone per unit,
    /// so a due arrival means every queued packet is due too — and [`drain_pending_moves`]
    /// runs *after* us in the same frame ([`crate::net`] chains apply → drain). Applying the arrival
    /// directly therefore writes the newest state and then lets the drain replay the older queued
    /// packets over the top of it. Last write wins, and the last write is stale: a Stop that races the
    /// tail of a burst is undone by the FORWARD packet queued in front of it, the mover then goes
    /// silent (a still player sends nothing at all), and the observer extrapolates a run that ended —
    /// off into the distance until some unrelated correction lands.
    ///
    /// **The reference needs no such test, and tracing why is what makes this faithful rather than
    /// defensive.** Its due test (`@0x618dd2`: `ecx = [edx+0x128]; sub ecx,fire; jns apply`) compares
    /// against `mgr+0x128` — the movement manager's clock *cell*, stamped once per movement update
    /// (`0x616800`), not the live ms counter. Its drain (`0x615c30`) runs in that same update against
    /// that same stamped value. So by the time packets are processed, every event still queued has
    /// `fire > mgr+0x128`, and monotonicity puts a new packet's fire at or beyond those: a due arrival
    /// and a non-empty queue are mutually exclusive *by construction*. We read a live frame clock with
    /// the drain still ahead of us, which breaks that exclusivity; this term restores it.
    ///
    /// Queuing instead costs nothing: the drain applies every due packet later in the same frame,
    /// this one included, in arrival order.
    pub(crate) fn fires_at_arrival(&self, fire_ms: f64, now_ms: f64) -> bool {
        self.pending.is_empty() && fire_ms <= now_ms
    }

    /// Advance one frame of dead-reckoning: the new `(WoW position, facing, vertical speed, horizontal
    /// speed)` given the unit's `speeds` and `dt`. On the **ground**, integrates the velocity the current
    /// `flags` imply in the facing frame (forward/back/strafe summed, normalized, at the run / run-back /
    /// walk / swim speed the flags pick) and rotates the facing while a `TURN_*` flag is set. **Airborne**
    /// (`JUMPING`/`FALLING`), it's a ballistic event instead: the frozen launch horizontal
    /// ([`Self::jump_xy_vel`]) plus a parabola under gravity ([`Self::vertical_velocity`]) — the launch
    /// played out locally, not flag-driven walking. Pure, so the signs + speed choice + arc are
    /// unit-tested (mirrors [`Spline::sample`]); the system writes the result back + to the transform.
    pub(super) fn advance(&self, speeds: MoveSpeeds, dt: f32) -> ([f32; 3], f32, f32, f32) {
        // A jump/fall is one ballistic event: horizontal frozen at the launch, height a parabola under
        // gravity (the same `g` the controller uses). Direction can't change mid-air, so the ground
        // direction flags are ignored here; the facing is corrected by packets (no in-air turn).
        if self.flags & move_flags::FALLING != 0 {
            let mut pos = self.wow_pos;
            pos[0] += self.jump_xy_vel[0] * dt;
            pos[1] += self.jump_xy_vel[1] * dt;
            // **The same exact fall step the local mover runs** ([`crate::player::mover::fall_step`],
            // decision 1740) — one shared machine, which is what the reference has. This used to
            // move at the START-of-step speed and only then apply gravity (explicit Euler), so a
            // watched player's jump overshot by `½·g·dt²` every frame and floated; the local mover
            // had the mirror-image bug in the other direction. The mean velocity is the exact
            // displacement rate under constant acceleration, so neither drifts now.
            //
            // **Feather fall is the mover's own bit**: `MOVEFLAG_SAFE_FALL` is
            // inside the reference's server-authored merge mask, so a watched player under Slow
            // Fall or Levitate arrives carrying it in every relayed pose — this word IS the word
            // `0x7c5d23` tests, and picking the clamp off it is the whole of the mechanism.
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

        // Turn-in-place / turning while moving: TURN_LEFT raises the facing, TURN_RIGHT lowers it
        // (matching the controller's A/D turn and the WoW orientation convention).
        let mut turn = 0.0f32;
        if self.flags & move_flags::TURN_LEFT != 0 {
            turn += 1.0;
        }
        if self.flags & move_flags::TURN_RIGHT != 0 {
            turn -= 1.0;
        }
        let orientation = self.orientation + turn * speeds.turn_rate * dt;

        // Travel direction in the facing frame (WoW: forward = (cos o, sin o), left = +90°).
        // Swimming, the FORWARD axis is pitched by the reported swim pitch — the client's swim
        // velocity basis `0x7c5880` writes `(cosY·cosP, sinY·cosP, sinP)` — so a diving swimmer
        // descends between packets instead of sliding flat; the STRAFE axis stays level, and a
        // backward swimmer travels along the negated pitched axis (nose-up backpedal descends).
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
        // The speed this mover's flags imply — the reference's `GetCurrentSpeed 0x7c4c90`, stated
        // once in [`crate::net::current_speed`] so this side and the local controller cannot drift
        // apart about a cascade whose whole content is its ORDER. Everything it reads is the
        // sender's own word, relayed verbatim: the direction bits, the swim bit and the walk bit.
        //
        // **The walk arm outranks the backward min, and this block used to have it backwards**:
        // it tested the backpedal first, so a walking backpedaller extrapolated
        // at `min(runBack, run)` = 4.5 yd/s. The bytes take the walk arm at `0x7c4d11` *before*
        // the run arm's `0x7c4d1d`, so the answer is `min(walk, run)` = 2.5 — a walking backpedal
        // is exactly as slow as a walk, and the observed body now plays Walk instead of Run.
        let base = crate::net::current_speed(&speeds, self.flags);
        let mut pos = self.wow_pos;
        let speed = if len > 1.0e-4 {
            let step = base * dt / len; // normalize the 3D direction, then advance by base·dt
            pos[0] += dx * step;
            pos[1] += dy * step;
            pos[2] += dz * step;
            base
        } else {
            0.0
        };
        // Grounded/floating: no ballistic vertical (a jump/fall arc returns earlier; a swimmer's
        // vertical is the pitched axis above, position-integrated, not a persisted velocity).
        (pos, orientation, 0.0, speed)
    }
}

/// **B197's fourth site — the *player* one**, in a world small enough to assert on:
/// a watched mover standing inside a building whose floor collider has not attached yet.
///
/// The same three facts as `spline::under_floor`, which is that record's creature twin: the terrain
/// is under the mover, the floor is not there yet, and it arrives later. What differs is the lane —
/// a player owns its Z through [`extrapolate_remote_units`], never through the creature clamp — and
/// therefore what the fix is: not a seat and an epoch, but **not running at all**. A flag-still
/// mover is not integrated by the reference (`0x20ff`, [`move_flags::INTEGRATED`]), so it is not
/// integrated here, and the incomplete world under it never gets to say anything.
///
/// Run under `WOW_REMOTE_IDLE_GATE=off` the standing test fails with **7.00 against 9.08** — the
/// mover buried on the terrain under the building — which is the bug, on this same binary. So the
/// test is known to test something (1384's own standard for its creature twin).
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

    /// Auberdine's geometry, to the yard (the pin `spline::under_floor` is built on): terrain at
    /// 6.98, the building's floor 2.08 above it, and the wire Z for a body standing on that floor
    /// a hair above it — for a *player* that hair is their own client's resting clearance against
    /// the very geometry we baked our collider from, so it is small by construction.
    const TERRAIN_Y: f32 = 6.98;
    const FLOOR_Y: f32 = 9.06;
    const WIRE_Y: f32 = 9.08;
    /// 60 fps, the rate the pre-1545 creep was measured at (it is frame-rate dependent — the
    /// descent is per *frame*, not per second, which is half of why it had to go).
    const FRAME: Duration = Duration::from_nanos(16_666_667);

    /// A 10×10 up-wound quad at `y` — a floor the one-sided down-cast will stand on.
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

    /// The terrain in, the building's floor still building, and one mover standing at the Z the
    /// wire gave it.
    fn half_arrived_world(flags: u32) -> (App, Entity) {
        let mut app = App::new();
        // `WorldCollision` takes the mover's trace exclusions (the ghost/DOOR set, 1767); the
        // real one is initialised by the world plugins, which a headless harness does not run.
        app.init_resource::<benilla_world::collision::MoverTraceExclusions>();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>().init_resource::<ColliderEpoch>();
        // The liquid/room facade the clamp and the dead-reckon both ask for a water-walker's
        // surface. Seeded empty: this harness is about geometry, and an empty
        // world answers "no liquid here", which is the case every test below is written for.
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        app.insert_resource(PlayerCapsule(Collider::capsule(
            CAPSULE_RADIUS,
            CAPSULE_HEIGHT - 2.0 * CAPSULE_RADIUS,
        )));
        // `update()` never runs plugin `finish()`, where avian seats its diagnostics resources —
        // and the update that lands the late floor below does step physics.
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

        // Four seconds inside a building we have not finished building. Pre-1545 this was a
        // 1/36-yd-a-frame descent: z=9.05 after one frame, on the terrain (6.98 + the capsule's
        // SKIN_WIDTH = 7.00) inside 1.3 s, and stuck there — the reported screenshot.
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

        // Still nothing to do — it was already standing on that floor, which is what its own
        // client resolved before the packet was ever sent. (Pre-1545 this is where it stayed
        // buried: the settled memo had armed on the terrain hit and carried no epoch.)
        frames(&mut app, 60);
        assert_eq!(
            z_of(&app, e),
            WIRE_Y,
            "the floor arrived under a mover that was already standing on it"
        );
    }

    /// The control: 0626's resolve is untouched for a mover the reference *does* integrate — the
    /// `INTEGRATED` gate lets it through and it is stepped through the world.
    ///
    /// **Its Z assertion is 2174's, and it is the opposite of the one 1545 wrote here.** 1545
    /// proved "integrated and resolved" by watching this mover *descend*, which works because four
    /// seconds of FORWARD walk it clean off the 10x10 terrain quad — so what it was actually
    /// pinning, past the edge, was `grounded_step`'s no-floor drop running open-loop, the very
    /// ratchet 1545's own comment called out two screens up and closed only for a flag-still mover.
    /// Past the edge our world has nothing to answer with, so the wire's height is the answer.
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

    /// …and the **vertical** half of that resolve still runs, which is what 2174 must not have
    /// taken away with the drop: a moving mover with ground actually inside the election's reach is
    /// put on it. Without this the test above would be satisfied by an extrapolator that had
    /// stopped meeting the world at all.
    #[test]
    fn a_moving_mover_is_still_settled_onto_ground_it_can_see() {
        let (mut app, e) = half_arrived_world(move_flags::FORWARD);
        // This harness's mover carries no speeds, so a direction bit alone travels nowhere and the
        // election's reach collapses to its slack. Give it a walk speed: now the frame travels
        // 0.033 yd and reaches `·1.8494 + 1/36` = 0.089 below the feet.
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

    /// …and the A/B lever still reproduces the bug on this binary, which is what makes the row
    /// above evidence rather than an assertion (0626's `WOW_REMOTE_FLAT`, 1384's `WOW_CLAMP_SEAT`).
    /// Serialised against the other two by running in-process only when the env var is set, since
    /// the lever is a process-wide `OnceLock`; `cargo test` sees it unset and skips.
    #[test]
    fn the_lever_reproduces_the_sink() {
        if !super::idle_gate_disabled() {
            return; // WOW_REMOTE_IDLE_GATE=off cargo test -p benilla-app the_lever_reproduces
        }
        let (mut app, e) = half_arrived_world(0);
        frames(&mut app, 240);
        // `-- --nocapture` prints the number decision 1545's table quotes.
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
