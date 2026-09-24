//! The server-authored path walk ([`Spline`], `SMSG_MONSTER_MOVE`) and the terrain re-ground that
//! goes with it ([`ground_clamp_creatures`]) — the creature half of [`super`]'s motion model
//! (decisions 0052/0059/0097).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_protocol::{CreateSpline, EntityKind};
use bevy::prelude::*;

use crate::entities::CollisionHeight;
use crate::player::swim_enter_depth;

use super::super::{NetEntity, ObjectStore};

/// A server-dictated movement path (`SMSG_MONSTER_MOVE`): the unit traverses `points` at constant
/// speed over `duration` from `start`. All points are raw WoW coords; [`sample_splines`] interpolates
/// each frame into the entity's [`Transform`]. Present only while the unit is path-walking.
#[derive(Component, Debug, Clone)]
pub(crate) struct Spline {
    pub(crate) points: Vec<[f32; 3]>,
    pub(crate) start: Instant,
    pub(crate) duration: Duration,
    /// The server's spline id (`SMSG_MONSTER_MOVE`). The controller echoes it in
    /// `CMSG_MOVE_SPLINE_DONE` when this spline drove our OWN player (Charge/knockback/taxi) — the
    /// server validates the ack against the newest spline id. Irrelevant for a creature's walk.
    pub(crate) id: u32,
    /// A **ground walk** (the spline's `FLYING` bit was clear): the real client discards the path's Z
    /// and re-derives it from the terrain under the unit (byte-verified — decision 0059), so
    /// [`ground_clamp_creatures`] snaps this unit onto benilla's terrain. `false` for a flying path,
    /// which keeps the spline's own Z.
    pub(crate) grounded: bool,
    /// The path's `SPLINEFLAG_RUNMODE` bit — **a run when set, a walk when clear**, and the clear
    /// case is the one that does something: the real client's `SMSG_MONSTER_MOVE` commit
    /// `0x7c6a50` feeds this bit to `CMovement::SetRunMode 0x7c71c0` (`0x7c6ac2 and edi,0x100`;
    /// `0x7c6acb call`), whose argument is *run*, so **a spline without RUNMODE sets
    /// `MOVEFLAG_WALK_MODE` on the unit it moves.** Every incoming spline re-authors the bit
    /// (decision 1758).
    ///
    /// Only the body **we drive** reads it ([`crate::player::server_ride`]) — a creature's own
    /// gait is derived from [`Spline::speed`], the path's arc length over its duration, so its
    /// walk bit would select nothing.
    pub(crate) run_mode: bool,
    /// **A deck path** (`SMSG_MONSTER_MOVE_TRANSPORT`): the transport guid whose frame every
    /// point in `points` is expressed in. `None` — the overwhelmingly common case — is the plain
    /// `SMSG_MONSTER_MOVE`, whose points are absolute world coordinates.
    ///
    /// A deck path is sampled into the rider's [`crate::transport::TransportRider`] local pose
    /// rather than straight into the [`Transform`]; `compose_riders` then carries it through the
    /// transport's live matrix, exactly as it already does for a rider whose pose came off a
    /// `MSG_MOVE_*` relay. It is also exempt from [`ground_clamp_creatures`]: the re-ground is a
    /// world-space terrain probe, there is no terrain under a deck, and the wire Z is already the
    /// deck-local height (decision 1936).
    pub(crate) deck: Option<u64>,
}

/// **A spline the server ended by decree**, carrying the id the acknowledgement owes (decision
/// 1281). `SMSG_MONSTER_MOVE`'s stop form has no path to walk, so it leaves no [`Spline`] behind —
/// but it is a spline launch like any other on the server side, with its own fresh id, and vmangos
/// arms `HasPendingSplineDone` for it whenever the unit is a player or a player's possessed
/// creature (`MoveSplineInit::Launch`, and `Unit::StopMoving` says so in as many words: *"Will
/// trigger CMSG_MOVE_SPLINE_DONE from client"*).
///
/// Until that acknowledgement lands — carrying **this** id, not the interrupted path's —
/// `HandleMovementOpcodes` drops every movement packet the client sends for that unit. So an
/// interrupted flee, charge or knockback needs the id kept, not discarded with the path.
/// [`crate::player::server_ride`] consumes it for the body in our hands; on anything else it is
/// inert bookkeeping, replaced by the next stop and cleared by the next real path.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct SplineStopped(pub(crate) u32);

impl Spline {
    /// Average ground speed of the path in yards/second (total length ÷ duration) — what the creature
    /// animation selector ([`crate::creature_anim`]) reads to choose Walk vs Run.
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

    /// How far through the ride this path is, `0..=1` — elapsed over duration, clamped. The
    /// inspector's motion line reads it; [`sample`](Self::sample) computes the same fraction to
    /// locate the unit, and a path at `1` is one frame from being dropped.
    pub(crate) fn elapsed_frac(&self) -> f32 {
        (Instant::now()
            .saturating_duration_since(self.start)
            .as_secs_f32()
            / self.duration.as_secs_f32().max(1e-3))
        .clamp(0.0, 1.0)
    }

    /// Interpolated `(raw-WoW position, facing, travel pitch)` at `now`, constant speed along the
    /// path (clamped at the ends). `facing` is the WoW orientation of the travel direction in
    /// progress (`atan2` of its horizontal component), `None` for a degenerate/vertical direction so
    /// the caller keeps prior facing. The **travel pitch** (radians, +up) is the direction's
    /// `asin(dz/len)` — the observed-mover pitch rule `asin(dir.z)` — `0` at rest/level; read by the
    /// swimming-creature body pitch (its consumer gates on [`CreatureSwimming`], so a ground
    /// walker's slope never tilts it).
    ///
    /// A **ground** path evaluates each segment as a straight lerp. A **flying** path (`!grounded`
    /// — the wire's `Flying`/`Mask_CatmullRom` bit) evaluates a uniform **Catmull-Rom** through the
    /// same waypoints, neighbours phantom-duplicated at the ends per the client's CCurve commit
    /// (`0x601d30`/`0x601e66`) — smooth arcs, no corners on a taxi flight. Decision 0496 folds back
    /// 0484 I3: the flying commit writes curve mode 1 (`0x7c6a50` → `[movInfo+0x54]`), and the
    /// point-at-t evaluator `0x4541b0` is itself the mode dispatcher — mode 1 calls the
    /// Horner-basis Catmull-Rom cubic (`0x453580`), mode 0 the 2-point lerp. Segment *location*
    /// stays chord-length parameterised in both modes (`0x4532e0` fills per-segment chord + total
    /// arc-length).
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

    /// The **flying attitude** `(pitch, bank)` in radians — the byte law (`0x7c5490`'s flying
    /// branch; decision 0516, correcting 0501's INTERIM look-ahead *pitch*):
    ///
    /// - **Pitch** is the instantaneous tangent's own climb — [`Spline::sample`]'s travel pitch,
    ///   `asin(t̂z)` — not a look-ahead difference: the client orients the mover's forward along
    ///   the spline tangent (`mover+0x5c`), so the climb angle IS that tangent's vertical
    ///   component. (The `mover+0x20` asin scalar is the SWIM branch's — flight never reaches it.)
    /// - **Bank**: `θ = 2·sign(cross)·acos(clamp(t̂ₓᵧ·d̂ₓᵧ, −1, 1))` between the XY-normalized
    ///   tangent and the XY direction to the **1000 world look-ahead** point (`0x3e8` @ `0x7c5623`;
    ///   a degenerate XY length — the client's eps `2.384e-7` — banks 0), then the **snap** — not
    ///   a soft clamp: `θ < −π/2 → −π`, `θ ≥ +π/2 → +π` (the sharp-corner/antipodal guard: a
    ///   hairpin reads as a momentary full roll, visible on the reference at switchbacks). No
    ///   ease, no damping — recomputed and applied per frame, exactly like the client. The lean
    ///   DIRECTION (into the turn: `cross > 0` = a left turn = roll left) is INFERRED — the
    ///   stored row's downstream matrix plumbing wasn't traced; magnitude, ×2, and snap are
    ///   byte-pinned.
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

/// Uniform Catmull-Rom position + travel direction on the polyline segment `pts[i] → pts[i+1]` at
/// local parameter `u ∈ [0,1]`, with the neighbour control points phantom-duplicated at the path
/// ends — the real client's CCurve commit stores `[first, first, …, last, last]`
/// (`0x601d30`/`0x601e66`), which makes the curve pass through every waypoint and start/end
/// tangent to the end segments. Returns `(position, d/du tangent)`; the tangent's scale is
/// meaningless to callers (they normalize or `atan2` it), only its direction matters.
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

/// Build the [`Spline`] implied by one `SMSG_MONSTER_MOVE`: the unit rides `path` — the full travel-order
/// polyline `[start, …waypoints…, endpoint]` the protocol decoded — at constant (arc-length) speed over
/// `duration_ms`. [`Spline::sample`] interpolates a ground path piecewise-linearly, which is faithful: the
/// real client's ground creature-follow evaluates the path with a segment lerp (`0x4541b0`),
/// arc-length parameterised, through every waypoint; a **flying** path
/// takes the Catmull-Rom family instead (the taxi/flight look — see [`Spline::sample`]'s INTERIM note).
/// Returns `None` — "stationary, clear any path" — for a `Stop`, a zero duration, or a path with fewer
/// than two points (nothing to travel along).
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
        // A deck path's Z is the deck-local height and nothing re-derives it, so `grounded` — the
        // terrain-clamp opt-in — is meaningless there; `deck` is what the clamp actually skips on.
        // The flag still rides along because the SAMPLER reads it too: `grounded` selects the
        // linear follow, `!grounded` the Catmull-Rom flight family (decision 1936).
        grounded: !flying,
        run_mode,
        deck,
    })
}

/// Build the [`Spline`] a unit is **already riding** at the moment it streams into view — its create
/// block's `MOVEFLAG_SPLINE_ENABLED` tail (decision 0708). Same path, same sampler as
/// [`monster_move_spline`]; the one difference is *where the ride starts*: the server tells us how much
/// of the path it has already covered ([`CreateSpline::time_passed_ms`]), so the spline's clock is
/// **back-dated** by that much and [`Spline::sample`] picks the walk up exactly where the server has it,
/// rather than restarting it from the top.
///
/// Returns `None` — "this unit is not walking, leave it at its create pose" — for a degenerate path, a
/// zero duration, or a ride the server has already finished (`time_passed ≥ duration`: the create pose
/// *is* the endpoint). `WOW_CREATE_SPLINE=off` also returns `None` for everything, restoring the
/// pre-0708 behaviour for an A/B.
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
        // `checked_sub` because `Instant` has no epoch to spare: a client started seconds after boot
        // can be handed a spline older than its own monotonic clock, and plain `-` panics there.
        start: Instant::now()
            .checked_sub(passed)
            .unwrap_or_else(Instant::now),
        duration: Duration::from_millis(u64::from(spline.duration_ms)),
        id: spline.id,
        grounded: !spline.flying,
        run_mode: spline.run_mode,
        // The create block's spline tail carries no transport of its own — a unit created on a
        // deck takes its `TransportRider` from the same block's movement tail, and its ride is in
        // world coordinates like any other create spline.
        deck: None,
    })
}

/// One `csp` line per create block that carried a live spline — the **supply** half of the
/// spawn-freeze instrument (decision 0708), written to the shared `WOW_MOVE_TRACE` sink so it
/// interleaves with everything else on one clock. Logged from the wire, *before* the ride is
/// interpreted, so the `WOW_CREATE_SPLINE=off` leg records the same lines as the fixed one and the two
/// are directly diffable.
///
/// `left` is the number that matters: the yards of path still ahead of the server at create time — i.e.
/// exactly how far this unit will drift from us while we hold it still, and therefore how big a jump
/// its next `SMSG_MONSTER_MOVE` has to make up. Read it against the `mmv` lines' realized snaps.
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

/// One `mmv` line per `SMSG_MONSTER_MOVE` — the **realized** half of the same instrument: the distance
/// from where we were drawing the unit to where the server says its new path starts, which is the
/// teleport the director sees. `?` when the unit has no transform yet (its spawn command hasn't
/// flushed), which is not a snap at all.
///
/// Split into `xy` and `z` on purpose: only the **horizontal** part is a desync. A grounded creature's
/// Z is deliberately ours, not the server's — [`ground_clamp_creatures`] re-derives it from our terrain
/// (decision 0059), so on a slope a correctly-followed creature still reads a Z difference of a yard or
/// two against the wire. Read `xy`; `z` is the terrain disagreement, and reading the 3-D total instead
/// would bury a clean follow in hill noise.
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

/// The A/B switch behind [`create_spline`]: `WOW_CREATE_SPLINE=off` drops every create-block spline,
/// which is exactly what the client did before decision 0708 (creatures frozen at first sight until
/// their next `SMSG_MONSTER_MOVE` snapped them forward).
fn create_spline_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        !std::env::var("WOW_CREATE_SPLINE").is_ok_and(|v| matches!(v.as_str(), "off" | "0"))
    })
}

/// Interpolate every path-walking entity along its [`Spline`] into its [`Transform`] each frame — so
/// motion stays smooth between the sparse server `MSG_MOVE` packets. Writes translation + rotation
/// only (scale, set by the renderer when it attaches the model, is preserved).
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(in crate::net) fn sample_splines(
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &Spline,
        &mut Transform,
        Has<CreatureSwimming>,
        // The rider's server identity, for the `spl` trace alone — `Option` because the sampler
        // must never gate on it (a spline entity with no guid still has to be flown).
        Option<&super::super::Guid>,
        // A **deck path**'s destination: the sample is the rider's pose in the transport's frame,
        // and `transport::compose_riders` — which runs after this stage — carries it out to the
        // world. `Option` because only a `SMSG_MONSTER_MOVE_TRANSPORT` rider has one; a deck
        // spline whose component is missing falls back to the world write below rather than
        // freezing (decision 1936).
        Option<&mut crate::transport::TransportRider>,
    )>,
    mut trace_next: Local<f32>,
    time: Res<Time>,
) {
    let now = Instant::now();
    // The `spl` sampler's own clock (see [`trace_ride`]): one tick per second for the WHOLE
    // population, not per entity — a shared deadline keeps every rider's line on the same instant,
    // which is what makes two units' progress directly comparable in the file.
    let tracing = benilla_assets::trace::enabled_for("spl");
    let tick = tracing && time.elapsed_secs() >= *trace_next;
    if tick {
        *trace_next = time.elapsed_secs() + SPL_TRACE_SECS;
    }
    for (entity, spline, mut t, swimming, guid, rider) in &mut q {
        let (wow_pos, facing, pitch) = spline.sample(now);
        // **The deck fork.** A transport path's points are offsets in the named transport's frame,
        // so the sample IS the rider's local pose; writing it to the `Transform` would place the
        // unit a few yards from the world origin. `compose_riders` composes it through the deck's
        // live matrix later this frame — the same route an observed rider's relayed pose takes.
        if let Some(deck) = spline.deck {
            // A deck sample lands on the rider or NOWHERE. Falling through to the world write
            // below would put the body a few yards from the map origin, because that is what a
            // transport-local offset reads as in world coordinates — so a rider component that is
            // missing (the insert is deferred by one sync point) or that names a different
            // transport freezes the unit for that frame instead. Freezing is recoverable; a body
            // teleported to the middle of the map is what a bug report looks like.
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
        // What the transform held *coming in* — i.e. whatever the previous frame left there after
        // every other writer had its turn. Traced beside the fresh sample as `was=`: a rider whose
        // `was` is not last frame's sample is being stomped by another system, which no amount of
        // staring at the sampler would ever show.
        let was = tick.then(|| bevy_to_wow(t.translation));
        t.translation = wow_to_bevy(wow_pos);
        if let Some(f) = facing {
            // The swim body pitch (render law `0x60a110`, applied to the spline movers): a swimming
            // creature moving along its path renders its root pitched to the segment's travel
            // pitch, nose-up positive about the body's local X; ground walkers render level. A
            // FLYING spline (the taxi) takes the full attitude — the tangent's climb plus the
            // look-ahead BANK ([`Spline::flight_attitude`], decision 0516) — on the unit
            // transform, so mount and rider tilt and lean as one composite, any model (0501's
            // placement law). Roll composes innermost: about the body's travel axis.
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
        // Path finished: the final pose is written above (the sample clamps to the last point), so drop
        // the spline. It's what makes a `Spline` mean "actively moving" — otherwise a completed path
        // lingers until the next packet and a creature reads as walking forever after one move
        // (`creature_anim` keys Walk/Run on the spline's presence).
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

/// How often the `spl` tag samples a live ride. One second is the resolution a "does this thing
/// actually move?" question needs, and it keeps a zone full of walkers off the shared trace mutex
/// (see [`benilla_assets::trace`] on why a busy tag distorts the run it measures).
const SPL_TRACE_SECS: f32 = 1.0;

/// One `spl` line per live [`Spline`] per [`SPL_TRACE_SECS`] — the **ride** half of the movement
/// instrument, beside `csp`'s supply and `mmv`'s realized snaps (decision 0708). Those two log at
/// the wire; this one logs what the client is actually *drawing*, which is the only thing that
/// answers "the server says it is flying and it looks frozen to me".
///
/// `t=` is the ride's own progress (elapsed/duration, seconds), `pos` the sampled raw-WoW point,
/// `spd` the path's constant speed (its full length over its full duration), and `was=` the
/// distance from what the transform held *coming into this frame* to the fresh sample. The three
/// separate the three faults that all look identical on screen: a ride whose `pos` does not change
/// while `t` advances is a **sampler** fault; a ride whose `spd` is wrong is a **wire** fault; and
/// a `was=` far larger than one frame of travel is a **stomp** — some other system writing this
/// transform after us.
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

/// **Whose Z and swim state does benilla derive?** — the one subject test
/// [`mark_swimming_creatures`] and [`ground_clamp_creatures`] share (decision 1921, bug B357).
///
/// Both used to ask `kind == Unit`, and **kind is the wrong question**. The reference has no such
/// test: `SMSG_MONSTER_MOVE`'s apply `0x6187a0` splices *whatever unit the packet named* into the
/// movement manager's list (`0x618801` installs the spline, `0x618808 call 0x619ca0` links it; its
/// only early-out is `6187bf test ah,0x10`, ROOT), the per-frame walk `0x615b10 → 0x616620` then
/// runs **once per frame for every registered CMovement**, and the vertical-zero gate inside it
/// reads only that CMovement's own flag word — `MI.flags & 0x4`, `MI.flags & 0x200` (the spline's
/// FLYING bit), `[CMovement+0x40] & 0x200800`, else `Δz := 0` (`0x616cec`-`0x616d03`). Not one of
/// those bytes asks what kind of object it is holding: nothing in the movement system is the
/// player's — every scalar, every flag, every packet is the mover's.
///
/// So a **Player the server moves along a ground spline has its Z re-derived from the terrain
/// exactly like a creature's**. cmangos's Playerbots drives its bots that way — they are Player
/// objects moved by `SMSG_MONSTER_MOVE` — and with the kind test in place they kept the wire Z
/// verbatim and strode a body-height over the slopes (B357). Nobody saw it on vmangos because a
/// *real* remote player's movement arrives relayed, carrying that player's own client-grounded Z.
///
/// The set, and why each member is in it:
/// - **`Unit`** — walking or idle. Decision 0059's law.
/// - **`Player`, once a server spline has moved it and until the relay takes it back.** That is
///   what `splined || derived_before` says: a bot enters on its first path and never leaves (it
///   sends no `MSG_MOVE_*`, so nothing ever hands it back), while a remote player yanked by a
///   Charge, a knockback or a fear is derived for the ride and returns to [`RemoteMotion`] on their
///   next packet. Sticky is the *reference's* shape too — `0x619ca0` splices, and no unlink of it
///   is known — and it is what keeps a bot grounded in the frames between paths,
///   including the one where [`sample_splines`] has just dropped the finished [`Spline`].
/// - **Never the body we steer** ([`crate::net::Embodied`]) — *whatever kind it is*, which is the
///   half decision 1927 corrected. Our own avatar is a Player and was already out; a **possessed
///   creature** is a `Unit` and was not, so the clamp cast a ray at it every frame and wrote a Y
///   that `player::body_pose` then overwrote from the controller's own swept capsule — dead work,
///   and worse than dead once `player::server_ride` began deriving that body's ride Z too: the
///   clamp's output would have become the ride probe's input, the feedback loop 1384 removed here.
///   One body, one owner: `player::` owns everything it is attached to.
/// - **Nobody else.** A GameObject sits at its authored Z (a lamp on a table). And a player under
///   [`RemoteMotion`] is 0059's deliberate deferral: their Z arrives already grounded by the client
///   that sent it, and our dead-reckoning owns their jump arc, which a down-ray would flatten.
///
/// The one thing that leaves with the `Embodied` exclusion: a body we are attached to but not
/// driving — a free-fly detach (`F`) parks `control` *and* the ride, so a possessed creature left
/// standing there keeps its wire Z. It is the same treatment a remote player gets, and the detach
/// is a dev affordance.
///
/// `derived_before` is [`GroundClamped`]'s presence — the memo the clamp leaves, which exists on a
/// Player only because it was once a subject. It is membership *and* memo deliberately: the
/// alternative was a second marker with two write sites to keep in step, and the two facts are the
/// same fact.
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

/// How far **above the seat** an idle unit's settle probe starts (yd): the reference's own
/// backface band. The swept-prism TOI (`0x632830`) still
/// counts a face the prism has already passed as a hit at `t = 0` when it is within `1/36` yd
/// (`[0x7ff9c8]`) behind the probe, so a floor a hair above the feet supports the body; anything
/// further above is simply not there. **Not** a lift: this clamp once started 2.5 yd above the seat
/// (0059's "clear the little hill"), which is a probe the reference never casts — from up there the
/// lid of a cage a unit stands *inside* reads as its floor (B372, decision 2018). The hill is the
/// walker's business now (the swept step below), and an idle unit is grounded exactly as far as the
/// reference's settle would ground it: downward, from where it stands.
const IDLE_UP_BAND: f32 = 1.0 / 36.0;
/// Distance (yd) below the seat the settle probe reaches, so a unit follows a small step/slope
/// *down* onto the surface — the reference's settle is `d·1.849 + 1/36` and a body it does not
/// reach *falls*; a stand-in for the fall, since an idle unit here has no arc to run. A **miss**
/// leaves the unit at its seat, which is where a genuinely airborne one belongs and doubles as the
/// airborne gate (a hovering/flying unit farther than this above the ground is never clamped).
const GROUND_CLAMP_DOWN: f32 = 4.0;

/// **The Y a grounded mover ends its frame at** — the reference's `0x634040` walk-resolve outcome
/// plus the two granted modes that move it (decision 1780), as a pure function so the two callers
/// cannot drift apart: [`ground_clamp_creatures`] for every streamed mover, and
/// [`crate::player::server_ride`] for the body we are attached to while a server spline drives it
/// (decision 1927). Before that second caller existed, the ride simply kept the wire Z — the same
/// premise B357 corrected for bots, one layer up, and the same defect: a straight chord over a
/// hollow, measured at +0.47 yd on a 16-yd Elwynn charge.
///
/// - `seat_y` — where the server put the body. A probe **miss** returns it unchanged: no walkable
///   surface in reach is a genuinely airborne pose, or ground that has not streamed in, and an
///   unclamped body belongs exactly where the server said.
/// - `floor` — the walkable surface the down-probe found.
/// - `water_floor` — the liquid surface, and **only** when `MOVEFLAG_WATERWALKING` makes it floor:
///   `0x63162e or edi,0x30000` ORs the two ADT liquid layers into the walk trace's class mask, so
///   one trace elects between water and terrain. Ours queries liquid separately, so the election is
///   written out as the `max` — which is what a single trace against both would have returned.
/// - `hover` — the granted HOVER bit. The reference finalises `z = z_before_snap − max(L − 1.0, 0)`
///   (`0x636e81`-`0x636ea9`), with `L` the achieved probe distance; here `L` is `seat_y − y`, so the
///   same expression is `(y + 1.0).min(seat_y)` — a clearance the body never *climbs* to.
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

/// Ground every **grounded creature** on benilla's own world — the path-walkers (a ground spline)
/// **and the idle ones standing at their raw spawn Z** (the "NPCs floating a bit"). The real client
/// doesn't trust the wire Z for a walking unit: the grounded fork zeroes the spline Z-delta and the
/// WALK resolver reads Z off the world trace (byte-verified, decision 0059); an idle unit reads
/// grounded against the reference too (the exact idle path isn't byte-pinned yet — 0059's open
/// follow-up, dispatched again under decision 2018).
///
/// **The probe geometry is the reference's, and it starts at the body** (decision 2018). A walker
/// continuing its path runs the shared swept step ([`crate::player::mover::grounded_step`]) from
/// where it stood last frame with Δz = 0 — the ride up a walkable rise, the atomic step-up with a
/// creature's own rise budget, the election snap down — and takes only the Y; the spline keeps the
/// xy. An idle unit, and a walker on its first frame of a new path, casts a one-sided ray **down
/// from its seat** (with the reference's `1/36` band above it) against the terrain/WMO **walking**
/// colliders — the same set the player stands on
/// ([`benilla_world::collision::WorldCollision::body_filter`]) — and takes the hit. Nothing here
/// ever probes from *above* the body: that was 0059's `seat + 2.5` origin, and from up there a
/// unit the server stood inside a GameObject's collision box found the box's lid as its floor
/// (Galen Goodward on his cage, B372).
///
/// Scope is [`ground_derived`]: every `Unit`, plus a **`Player` the server moves** — a Playerbot,
/// or anyone under a Charge/knockback/fear — because a body the server splines has no other Z
/// authority here and the reference re-derives it the same way for either kind (B357, decision
/// 1921). A **flying** spline keeps its own Z, and so does a **swimming** mover
/// ([`CreatureSwimming`]): its wire Z *is* its swim depth — vmangos paths a water creature in 3D
/// through the volume with a plain non-FLYING spline (verified
/// `MoveSplineInit`/`WaypointMovementGenerator.cpp`: only `CanFly()` sets the flag), so
/// ground-clamping it dragged murlocs to the lakebed, and the reference agrees at the bytes (the
/// vertical-zero gate keeps Δz whenever `[CMovement+0x40] & 0x200000` SWIMMING is set —
/// `0x616cfa`). Runs right after [`sample_splines`] so a walker's freshly-sampled XY+Z is what we
/// re-ground.
///
/// **The probe measures from the unit's SEAT, never from the clamp's own last answer** (decision
/// 1384). The seat is the Z whoever owns this unit's position last wrote — the create block, an
/// `SMSG_UPDATE_OBJECT` move, its spline, a transport deck composing its rider — and it is what the
/// server means by "where this unit is". Feeding the previous *output* back in as the next input is
/// what made a single wrong answer permanent: a unit created inside a building at world entry, in
/// the window before that building's own floor collider had attached, found the terrain under the
/// building instead, dropped onto it, and from down there the floor was above the probe's reach
/// forever after. Deriving from the seat makes every frame's answer a pure function of (the
/// server's pose, the colliders) — so the moment the floor lands, the unit is back on it, with no
/// movement needed to shake it loose. A probe **miss** — genuinely airborne, or the ground here
/// hasn't streamed in — puts the unit at its seat, which is exactly where an unclamped unit belongs.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(in crate::net) fn ground_clamp_creatures(
    world: benilla_world::collision::WorldCollision,
    // The liquid query — a water-walking creature's floor is the surface, not the lakebed
    // (decision 1780).
    points: benilla_world::world_point::WorldPoint,
    epoch: Res<benilla_world::collision::ColliderEpoch>,
    // The body a walker's step sweeps — the one player capsule every body shares, as the remote
    // dead-reckon does (decision 0626). The reference sweeps each unit's own `CreatureModelData`
    // radius/height (`[CMovement+0xb0]`/`+0xb4`, decision 1125); per-creature extents are the
    // refinement, not this record's.
    capsule: Res<crate::player::PlayerCapsule>,
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &NetEntity,
        Option<&Spline>,
        &mut Transform,
        Option<&mut GroundClamped>,
        Has<CreatureSwimming>,
        // The server-granted movement modes (decision 1780). Two of them move this answer: HOVER
        // rests the body a yard clear of the floor it found, and WATERWALKING makes the liquid
        // surface one of the floors it can find.
        Option<&crate::net::UnitMoveModes>,
        // [`ground_derived`]'s other two inputs — the two OTHER Z authorities a body can have:
        // the controller (the body we steer) and the relay stream's dead-reckoning.
        Has<crate::net::Embodied>,
        Has<crate::net::RemoteMotion>,
    )>,
) {
    let cost = clamp_cost_enabled();
    let legacy = clamp_seat_disabled();
    let t0 = cost.then(std::time::Instant::now);
    let (mut visited, mut skipped, mut held, mut cast, mut swept, mut hit_n, mut moved) =
        (0u32, 0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    // **The walker arm's two ground numbers** (decision 2174).
    //
    // - `noflr` — walker frames whose swept step found no walkable floor at all. Expected nonzero
    //   while the world streams (our colliders arrive after the units do) and it is *harmless*:
    //   such a frame holds the server's pose. It is the streaming-race health number.
    // - `ratchet` — walker frames that found no floor and were nonetheless left BELOW the server's
    //   seat. **This is a tripwire and it must read 0.** It cannot happen by construction now — the
    //   miss branch writes `seat_y` verbatim — so any nonzero here means the no-floor drop has
    //   found its way back into this arm, which is what walked Stormwind's patrolling guards 31 yd
    //   down through the world (census `worst=+31.45`, one guard's `clmp` at 0.105 yd a frame).
    //   `deepest` and `worst` name the offender, because a count alone cannot be reconciled against
    //   `WOW_GROUND_CENSUS`'s per-unit rows.
    //
    // (The idle arm's miss is not either event: it has always left the unit at its seat.)
    let (mut noflr, mut ratchet, mut deepest) = (0u32, 0u32, f32::NEG_INFINITY);
    let mut worst: Option<(u32, [f32; 2], f32)> = None;
    // What re-armed each cast (1384): the unit's own seat moved, or the world's colliders changed
    // under a unit that didn't. The second must be ~0 in a settled scene — a nonzero steady-state
    // `armed` means the collider set is churning and the gate is holding nothing.
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
            continue; // somebody else owns this body's Z — see the predicate
        }
        if spline.is_some_and(|s| !s.grounded) {
            skipped += 1;
            continue; // a flying path is authoritative on Z
        }
        if spline.is_some_and(|s| s.deck.is_some()) {
            skipped += 1;
            continue; // a deck path: no terrain under a boat, and the wire Z is deck-local (1936)
        }
        if swimming {
            skipped += 1;
            continue; // in-liquid: the wire Z is the creature's swim depth
        }
        let xz = [t.translation.x, t.translation.z];
        // The granted-mode word is a *fourth input to the ray's answer*, so it joins the cache
        // gate below: a creature that is handed HOVER while standing perfectly still has not moved
        // and the world has not changed, and without this its cached ground answer would outlive
        // the grant (decision 1780).
        let granted = modes.copied().unwrap_or_default();
        // **Re-seat on any write that wasn't ours.** The Y standing here differs from the one this
        // unit's last clamp left behind exactly when somebody who owns this unit's position moved
        // it — the wire, its spline, a transport deck — and *that* Y is the authority the probe
        // measures from. With no memo at all (a unit's first frame), the pose it spawned at is the
        // seat, which is the create block's own Z.
        let seat_y = match clamped.as_deref() {
            _ if legacy => t.translation.y, // the pre-1384 leg: measure from our own last answer
            Some(c) if c.y_written == t.translation.y => c.seat_y,
            _ => t.translation.y,
        };
        // The cast gate (decision 1357): a unit whose seat and XZ are bit-identical to the cast
        // that produced its cached hit, in a world whose collider set has not changed since, cannot
        // get a different answer — those three ARE the ray's inputs. A moving floor (a lift, a
        // transport deck) reaches its rider through the wire's own position writes, which move the
        // seat and re-arm the cast; a building whose floor collider attaches a few frames after the
        // unit was created moves the epoch and re-arms it (decision 1384 — before the epoch was in
        // the gate, that unit's wrong answer was cached for the session). A MISS never caches, so a
        // unit standing on a tile whose collider hasn't streamed in keeps asking until it lands.
        //
        // **And the answer must still be STANDING, not merely unchanged** (decision 1921). The
        // seat compare `c.seat_y == seat_y` cannot see the one case where those two come apart:
        // somebody rewrites this unit's Y to a pose whose *seat* equals the cached one, which is
        // exactly what a re-sent identical wire pose is (a periodic `SMSG_UPDATE_OBJECT`, a stop
        // packet restating where the unit already was). The question was unchanged, so the gate
        // held — and left the unit standing at the raw wire Z it had just been handed, un-grounded
        // until something moved it. `c.y_written == t.translation.y` is the strictly stronger
        // test: it *implies* the seat compare (the derivation above reads `c.seat_y` on precisely
        // that condition) and additionally asserts our own answer is the Y still under the unit.
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
        // The water-walker's floor, and **only** for a creature the swim mark already let through:
        // the reference takes this arm only when the swim bit is clear (`0x631617`), and a
        // swimming creature `continue`d above. The plane is the
        // liquid surface in Bevy Y, which both answers below read the same way.
        //
        // The reference's second, rate-limited hover pass (`0x636fa1`, climbing back toward the
        // clearance at 7 yd/s) is deliberately NOT reproduced: this clamp is a pure function of
        // (server pose, colliders) by construction, which is what makes its cache gate sound
        // (decision 1384), and a per-frame ramp is state. A creature's seat is the server's, so the
        // static answer is the one it converges to anyway.
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
        // ── Two answers, one probe law (decision 2018). ──
        // The reference has exactly one vertical probe geometry and it starts AT THE BODY: the WALK
        // resolver's settle sweeps DOWN from the current position (`0x636dcd`–`0x636e45`, `d·1.849
        // + 1/36`), and the only way up is the multipass step-up, which runs after a blocking hit
        // on the horizontal leg and rises at most `H`. Nothing ever probes from above the body —
        // and that is what this clamp did (`seat + 2.5`), so a unit the server stood INSIDE a
        // GameObject's collision box had its ray start above the box's lid, find the lid as a
        // floor, and stand on it (Galen Goodward on his cage, B372: seat 22.42, ray from 24.92, lid
        // at 24.67).
        //
        // - A **walker** continuing a path runs the reference's own step: Δz = 0 from where it
        //   stood last frame, then the shared swept resolve ([`grounded_step`] — the ride, the
        //   step-up with a creature's own `H` of 2.0, the election snap), and only Y is taken; the
        //   spline keeps the xy. This is what keeps a chord under a hill ON the hill (0059's
        //   "creatures run through small hills"): the surface rises under the body a frame at a
        //   time and the sweep rides it up, as the reference does, instead of a probe from above
        //   finding the hilltop. Over a hollow the snap misses and the step descends by its own
        //   reach a frame — the fall's stand-in — until the floor is under it again.
        // - An **idle** unit, and a walker's first frame on a new path (the reference re-bases its
        //   mover on every inbound packet), is the settle alone: DOWN from the seat, with the
        //   reference's own `1/36` band above it. Memoryless, from the seat: decision 1384's law.
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
            // The frame's horizontal displacement as a velocity over one second: `grounded_step`
            // takes `speed · dt` as its travel, and the reference's walk step is likewise a
            // horizontal distance and a direction (`0x6367b0`'s own signature).
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
                    // Per-frame state the memo does not keep (1129): a walker following a steep
                    // face down gets the ordinary cone reach, and its path's next sample corrects
                    // whatever that misses.
                    steep: false,
                },
            );
            // **A walker whose sweep found no floor holds the server's pose** (decision 2174) —
            // the same law the idle branch below obeys, and the one [`grounded_y`] states in as
            // many words: a probe miss is either a genuinely airborne pose or ground that has not
            // streamed in, and an unclamped body belongs exactly where the server said. Taking
            // `grounded_step`'s no-floor drop instead made this arm a ratchet: it sweeps again next
            // frame from *this* answer, and once the body is under the surface the down-cast can
            // never find the terrain above it again (one-sided) and the step-up can never fire
            // (it needs a blocking horizontal contact, which a flat up-wound face never gives). It
            // ran Stormwind's patrolling guards 31 yd below their own seat, at 0.1 yd a frame,
            // until the next `SMSG_MONSTER_MOVE` re-based them.
            //
            // 2018's hill is untouched: a chord cutting under a rise meets that rise on the
            // horizontal leg and the sweep HITS, so it rides as before. This arm is only what
            // happens when our world cannot answer at all — and then the server is the only
            // authority there is.
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
            // The one-sided down-ray (decision 0970): a creature grounds like the player grounds
            // — a face whose winding points away is no floor, or an idle NPC would stand mid-air
            // on the very shell face the player mover now falls through.
            let hit = world.ray_body(origin, Dir3::NEG_Y, reach);
            let floor = hit.as_ref().map(|h| origin.y - h.distance);
            let y = grounded_y(seat_y, floor, water, hover);
            // The clamp trace (`WOW_MOVE_TRACE`, tag `clmp`, filtered to one display by
            // `WOW_CLAMP_TRACE=<display id>`): one line per cast for that display — the seat the
            // probe measured from, where the ray started, what it hit and the Z it wrote, in WoW
            // coordinates. B372's reading instrument: "the NPC stands on the cage" is a claim
            // about which surface this ray found, and no screenshot can say whether that was
            // the lid or the floor.
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
        // Exact bit equality is deliberate, not a sloppy float compare: the question is "would the
        // write change anything" — Bevy's change detection fires on the DerefMut regardless of
        // value, so writing an equal Y every frame marked every standing creature's transform
        // subtree dirty. An epsilon would answer a different question: a real sub-epsilon ground
        // shift must still land.
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
        // `WOW_CLAMP_COST=1` — the premise check for gating this sweep (0732 slice S), which
        // 0732 sized at 0.42 traced against avian's `SpatialQuery::cast_ray`. **0970 replaced
        // that ray** with a broadphase-plus-BVH gather, so the recorded price measures a
        // mechanism that no longer exists and the lane is honestly unsized until this prints.
        //
        // The two fields that decide the two possible gates, and their kill conditions:
        //   · `cast` vs `visited`  — how much of the walk even reaches a ray. A movement gate can
        //     only ever save the `cast` share; if `ms` is small this whole item is dead.
        //   · `moved` vs `hit`     — how often the write actually CHANGES Y. Every hit writes
        //     `Transform` unconditionally today, and a write dirties the transform subtree whether
        //     or not the value differs. If `moved` is a small fraction of `hit`, an equality gate
        //     on the write is the cheap half and needs no movement tracking at all.
        //
        // Per 0734's law (~10.5 ns per row visit), the walk itself is never the cost here: at ~800
        // units it is ~8 µs. Only `ms` justifies the slice — quote it, not the counts.
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

/// The one display the `clmp` trace follows (`WOW_CLAMP_TRACE=<display id>`); `None` = the tag
/// writes nothing. A filter rather than a firehose: every idle unit in a city re-casts on each
/// collider epoch, and one unit's line per cast is what a report reads.
fn clamp_trace_display() -> Option<u32> {
    static ID: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *ID.get_or_init(|| std::env::var("WOW_CLAMP_TRACE").ok()?.trim().parse().ok())
}

/// Whether the ground-clamp meter is armed (`WOW_CLAMP_COST`). Read once, then a relaxed bool.
fn clamp_cost_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_CLAMP_COST").is_some())
}

/// The A/B switch behind decision 1384: `WOW_CLAMP_SEAT=off` restores the pre-1384 clamp, which
/// measured from its own previous answer and cached a hit without dating it against the collider
/// set. That is the leg where a unit created inside a building before the building's floor collider
/// attached stays under the floor for the session (B197) — kept as the lever that reproduces the bug
/// on the fixed binary, so the fix's evidence never depends on two different builds.
fn clamp_seat_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        std::env::var("WOW_CLAMP_SEAT").is_ok_and(|v| matches!(v.as_str(), "off" | "0"))
    })
}

/// What [`mark_swimming_creatures`] reads per unit: identity, kind, pose, its own collision height
/// (`None` only on a unit's first frame, before the stamp), its `UNIT_FIELD_FLAGS` (the enter gate
/// — [`crate::player::may_swim`]), its own WMO-room claim (whose liquid may answer for it —
/// decision 0696), and whether it is already marked.
/// The gate: a stamped unit re-enters the query only when its `Transform` moved (spline ticks,
/// teleports, clamp writes) or its descriptors changed (a `UNIT_FIELD_FLAGS` delta — a pet losing
/// PLAYER_CONTROLLED, a GM `.modify unitflag` — must re-ask); an unstamped one is retried every
/// frame until it evaluates. `Changed<ObjectStore>` also covers the *insert*, which is what lets a
/// unit whose descriptor block lands a frame after its transform still evaluate.
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
    // [`ground_derived`]'s four other inputs — this mark and the clamp walk the same population,
    // and must, or a server-splined body in water would be dragged to the lakebed by a clamp its
    // own swim state was never derived to exempt it from (B357).
    Has<Spline>,
    Has<GroundClamped>,
    Has<crate::net::Embodied>,
    Has<crate::net::RemoteMotion>,
);

/// This unit has been through [`mark_swimming_creatures`] with its room claim settled — the
/// change-gate's stamp (decision 1445). A stationary unit's swim state cannot change while the
/// world stands still (1.12 water is static; the boundary depends only on the unit's own
/// position and height), so a stamped unit re-evaluates only on `Changed<Transform>` — which
/// every spline tick, teleport and clamp correction raises. The world NOT standing still is the
/// [`ColliderEpoch`] (1384's law, liquid-flavoured): liquid arrives with the same streamed
/// tiles/WMOs that attach colliders, so an epoch bump drops every stamp and the population
/// re-evaluates once. Before the gate, `water_surface_at` ran for every streamed unit every
/// frame — ~0.13 traced ms/frame at the Goldshire pin, almost all of it standing idlers.
#[derive(Component)]
pub(in crate::net) struct SwimEvaluated;

/// The exit band (yd) below the swim boundary where a marked swimmer stays marked — the player
/// boundary's **VERIFIED** 1/36-yd hysteresis (`0x7ff9d0`), an absolute distance independent of
/// collision height, so it is subtracted from the boundary rather than scaled with it.
const CREATURE_SWIM_EXIT_BAND: f32 = 1.0 / 36.0;

/// A **server-moved body** ([`ground_derived`]) currently past the swim boundary — the client-side
/// derivation of the swim state the wire never carries for one: vmangos sets `MOVEFLAG_SWIMMING`
/// only from player packets, and a water creature's create block + splines carry no swim marker at
/// all (verified `vmangos-src`, 2026-07-17). Neither does a **spline-driven player** — a Playerbot
/// sends no `MSG_MOVE_*` of its own and `SMSG_MONSTER_MOVE` carries no flag word, so the same
/// nothing arrives for it, and it is derived here for the same reason (B357, decision 1921). The
/// gate below admits it: bit 3 PLAYER_CONTROLLED is set on every player object, so
/// [`crate::player::may_swim`] is true. The real client runs its own depth decision `0x6030c0` on **remote
/// units and creatures too**, not only on the body it steers.
/// `0x6030c0` has exactly one caller and it reaches
/// per-unit three ways: a **per-frame registry walk** (`0x616800`, a registered frame callback →
/// `0x615b10` over the movement manager's intrusive CMovement list → `0x616620` per node, whose own
/// active-mover GUID compare gates only the `0xee` heartbeat), the inbound move-message apply
/// (`0x618c30`), and `CGUnit_C::Initialize` for a unit that streams in already dead. A
/// spline-driven creature is spliced into that list by the `SMSG_MONSTER_MOVE` apply itself
/// (`0x6018f0 → 0x6187a0 → 0x619ca0`), and `0x60df70`'s not-the-mover arm sets SWIMMING
/// **synchronously** (`0x61a130 → 0x61a230 → 0x7c6e50`) — the GUID compare only chooses whether the
/// transition is additionally *reported* to the server. So deriving it here is faithful, and always
/// was. Evaluating **every streamed unit**, as this does, is a safe superset of the reference's
/// population: the walk covers a unit only while its CMovement is linked and `+0x40 & 0x8000000`
/// is clear.
///
/// **What was missing is the gate** (B311, decisions 1568 + 1572). `0x6030c0` reads the unit's
/// `UNIT_FIELD_FLAGS` on **both** legs of its decision ([`crate::player::may_swim`] —
/// PLAYER_CONTROLLED, PET_IN_COMBAT, USE_SWIM_ANIMATION): a set bit permits entry, and on the exit
/// leg a set bit *prevents* the stop, so a unit with none of them **walks the lakebed at any
/// depth** and is driven out of swim if anything else ever put the flag on it. That is what
/// vmangos means by *"Giant type creatures walk underwater"*: the Shore Strider off the Forgotten
/// Coast is a sea giant with no `CREATURE_STATIC_FLAG_CAN_SWIM`, so the reference wades it
/// chest-deep on its legs while benilla, gating on depth alone, slid it along on a swim gait.
///
/// Depth is measured on the creature's **own** collision height: marked once its feet sit deeper
/// than `0.75·h` ([`swim_enter_depth`]), unmarked once they rise a [`CREATURE_SWIM_EXIT_BAND`]
/// above it so the state can't flicker. The flat 2.0-yd stand-in it used before decision 0645 is
/// retired (0464's `collisionHeight` plumb was what it was waiting on); what stays open is the
/// *reference point* the real client measures a creature's depth from.
///
/// `0x6030c0`'s other guard — bail entirely while `MOVEFLAG_LEVITATING` (`0x400`) is set, so
/// neither leg runs — has no creature-side expression here, for two independent reasons: a
/// spline-walked creature carries no live move-flag word at all (`creature_anim::select::unify`'s
/// creature leg synthesises one), and vmangos never puts the bit on a creature anyway
/// (`Unit::SetLevitate` has **zero callers**; the only writer is the GM-fly path on a *player*).
/// The bail is real and ungated in the reference — its bit arrives through `0x618c30`'s wire
/// flag-merge mask, which has no active-mover compare — so this is a "nothing to read", not a
/// "doesn't apply". The local avatar's copy of the same guard is `update_swimming`'s first branch.
///
/// Consumers: the swim-gait leg of the animation selector (`creature_anim::select::unify`), the
/// swimming body pitch ([`sample_splines`]), and the [`ground_clamp_creatures`] exemption — which
/// is why the gate reaches further than the gait. A creature the server did NOT flag is now
/// re-grounded like any other walker, and that is right by the same authority: vmangos paths an
/// unflagged creature along the **bottom** of the water (`Object.cpp`'s *"Giant type creatures
/// walk underwater"* early-return, no upward randomisation), so its wire Z is the seabed and
/// clamping it to our terrain is the same answer, not a different one.
///
/// **Not** a consumer, deliberately: the enter-water splash (`sound::water`) reads `0.4·h` against
/// its own depth, exactly like the reference's `0x60314a` splash compare — which sits *outside*
/// the flag gate. A giant wading in still splashes.
#[derive(Component)]
pub(crate) struct CreatureSwimming;

/// The last ground clamp this unit took (decisions 1357 + 1384) — [`ground_clamp_creatures`]' seat
/// and its cast gate. Every field compares by bit: the gate's question is "are the ray's inputs
/// identical to the cast that produced this", never "close enough".
#[derive(Component, Clone, Copy)]
pub(crate) struct GroundClamped {
    /// Where the last cast stood.
    xz: [f32; 2],
    /// **The authoritative Y the answer was derived from** — the pose whoever owns this unit's
    /// position last wrote, never the clamp's own output. This is what makes the clamp a pure
    /// function of (server pose, colliders) instead of a ratchet that can only ever fall (1384).
    pub(crate) seat_y: f32,
    /// The Y standing after that cast (written or left). It differs from [`Self::seat_y`] by
    /// exactly the clamp's own correction, so an *external* write is detectable as "the Y here is
    /// not the one I left", which re-seats.
    pub(crate) y_written: f32,
    /// Only a HIT caches: a miss keeps casting, so a tile whose collider streams in late still
    /// catches its standing units.
    pub(crate) hit: bool,
    /// The collider-set stamp the answer was computed against — a cached answer outlives neither
    /// the unit's own pose nor the world it described.
    epoch: u64,
    /// The granted-mode word the answer was computed under (decision 1780). HOVER and WATERWALKING
    /// both change what the same ray, from the same seat, in the same world, resolves to — so they
    /// are as much an input to the cached answer as the other three.
    modes: crate::net::UnitMoveModes,
    /// **The path this answer continues** — a walker's `(spline id, start)` — or `None` for an idle
    /// answer. A walker's frame is the reference's: Δz = 0 from where it stood last frame, then the
    /// swept resolve (decision 2018). That continuity is exactly as long as one server path: the
    /// reference re-bases its mover on every inbound movement packet (`0x7c6420`, `base :=
    /// packet pos`), so a new spline starts again from the server's own Z, and the late-floor
    /// ratchet 1384 removed cannot outlive a path here either.
    path: Option<(u32, Instant)>,
}

impl GroundClamped {
    /// **Which arm produced this answer** — `true` for the walker's swept step (this unit is
    /// continuing a server path), `false` for the idle settle from the seat. The two obey different
    /// laws on a probe MISS, so a readout that names [`Self::hit`] without naming the arm is
    /// ambiguous: an idle miss leaves the unit at its seat, a walker miss descends.
    pub(crate) fn walking(&self) -> bool {
        self.path.is_some()
    }
}

/// The reference's `0x6030c0` decision for one creature, as a pure function of everything it
/// reads — [`CreatureSwimming`]'s law in one place, so both asymmetries are assertable without a
/// world:
///
/// ```text
/// enter iff  flags ∧ depth >  0.75·h              (0x603106 test ah,0x41 + jne — a ZF test, STRICT)
/// stay  iff  flags ∧ depth >= 0.75·h − 1/36       (0x6031c5 test ah,5 + jnp — parity, INCLUSIVE at ==)
/// ```
///
/// **The flag term is on both legs**, which is the part decision 1568 shipped wrong and 1572
/// corrects: [`crate::player::may_swim`] is not an entry permit but the predicate *"may this unit
/// be locally SWIMMING at all"*, and its false value forces a stop **at any depth**
/// (`0x6031eb → 0x60dff0`), not merely a refusal to start. So a creature that loses its bit
/// mid-water leaves the gait immediately, exactly as the reference drives it out on the next tick.
///
/// The two depth boundaries are deliberately different comparisons, not a rounding accident — the
/// enter compare is strict and the stay compare is inclusive at equality, which is what makes the
/// 1/36-yd band a band rather than a knife edge.
fn creature_swim_state(marked: bool, depth: f32, boundary: f32, unit_flags: u32) -> bool {
    if !crate::player::may_swim(unit_flags) {
        return false; // both legs — a permit-less unit cannot start AND cannot stay
    }
    if marked {
        depth >= boundary - CREATURE_SWIM_EXIT_BAND
    } else {
        depth > boundary
    }
}

/// Maintain [`CreatureSwimming`] on every [`ground_derived`] body from the water over its feet
/// (module docs on the INTERIM boundary). Runs chained before [`ground_clamp_creatures`] so a fresh
/// mark exempts the clamp the same frame (Bevy inserts the deferred-command sync point for the
/// chain) — and walks **the same population**, which is the half B357 would have broken silently:
/// admit a splined player to the clamp without deriving its swim state and a bot swimming a lake
/// is dragged to the bed, the murloc bug wearing a different body.
pub(in crate::net) fn mark_swimming_creatures(
    mut commands: Commands,
    units: Query<SwimMarkQuery, SwimMarkGate>,
    stamped: Query<Entity, With<SwimEvaluated>>,
    world: benilla_world::world_point::WorldPoint,
    epoch: Res<benilla_world::collision::ColliderEpoch>,
    mut last_epoch: Local<Option<u64>>,
) {
    // A world edge (a streamed tile/WMO attached or dropped colliders — and, with them, their
    // liquid) invalidates every standing answer: drop the stamps, the whole population
    // re-evaluates next frame ([`SwimEvaluated`]'s doc). Rare, so the steady state stays gated.
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
            continue; // GameObjects don't swim; a relayed player carries the real flag on the wire
        }
        // The unit's OWN room decides whose liquid answers (0696). Before it, both sources
        // answered: Undercity's NPCs read Tirisfal's ADT water 95 yd over their heads and swam on
        // dry stone in rooms the player walked.
        //
        // A unit the room tracker hasn't reached yet cannot ENTER swim — an unsettled claim admits
        // both sources, the very false positive that fix removes, so a freshly streamed NPC
        // standing in an interior would flash the swim gait for the frame before its room lands.
        // It can still LEAVE: a stale mark must always be able to clear.
        let who = benilla_world::world_point::Subject::Unit(e);
        if !marked && !world.room_settled(who) {
            continue; // …and stays unstamped, so it is retried until the claim lands
        }
        if !evaluated {
            commands.entity(e).insert(SwimEvaluated);
        }
        let wow = bevy_to_wow(t.translation);
        let depth = world
            .water_surface_at(who, wow)
            .map_or(f32::MIN, |s| s - wow[2]);
        let boundary = swim_enter_depth(collision.copied().unwrap_or_default().0);
        // A unit whose descriptor block has not landed yet reads flags 0 and simply cannot enter
        // this frame; `Changed<ObjectStore>` brings it straight back when the block arrives.
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

/// B197's mechanism, in a world small enough to assert on: a unit created inside a building whose
/// floor collider has not attached yet, and what happens to it when the floor lands.
///
/// The real thing is a race between two async collider builds during a loading screen; here it is
/// two `spawn`s and a `bump`, which is the same three facts — the terrain is under the unit, the
/// floor is not there yet, and it arrives later. Both halves of decision 1384 are load-bearing in
/// the second assertion: the seat is what lets the probe reach back up to the floor at all, and the
/// epoch is what makes anything re-ask after the world changed under a unit standing still.
#[cfg(test)]
mod under_floor {
    use avian3d::prelude::*;
    use benilla_protocol::EntityKind;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;

    use super::{ground_clamp_creatures, GroundClamped};
    use crate::net::NetEntity;
    use benilla_world::collision::ColliderEpoch;

    /// Auberdine's geometry, to the yard: terrain at 6.98, the building's floor 2.08 above it, and
    /// the server's Z for the NPCs inside 0.08 above *that* (the small float every wire Z carries).
    const TERRAIN_Y: f32 = 6.98;
    const FLOOR_Y: f32 = 9.06;
    const WIRE_Y: f32 = 9.14;

    /// A 10×10 up-wound quad at `y` — a floor the one-sided down-ray will stand on.
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
        // surface (decision 1780). Seeded empty: this harness is about geometry, and an empty
        // world answers "no liquid here", which is the case every test below is written for.
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        // The body a walker's swept step sweeps (decision 2018) — idle units never touch it.
        app.insert_resource(crate::player::PlayerCapsule(Collider::capsule(
            crate::player::CAPSULE_RADIUS,
            crate::player::CAPSULE_HEIGHT - 2.0 * crate::player::CAPSULE_RADIUS,
        )));
        // `update()` never runs plugin `finish()`, where avian seats its diagnostics resources —
        // and the second `update()` below (the one that lands the late floor) does step physics.
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

        // The window B197 lives in: the only surface under this NPC is the terrain the building
        // sits on, so that is what the clamp finds. This drop is CORRECT given what exists.
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

        // Nothing moves the NPC. The building's floor collider attaches, and stamps the world.
        floor_at(&mut app, FLOOR_Y);
        app.world_mut().resource_mut::<ColliderEpoch>().bump();
        app.update();

        // The fix: it comes back up on its own. Before 1384 it stayed on the terrain for the rest
        // of the session — the cached hit was never re-asked, and even re-asked it would have
        // measured from down there.
        clamp(&mut app);
        assert_eq!(
            y_of(&app, npc),
            FLOOR_Y,
            "the floor arrived under an NPC that never moved; it belongs on it"
        );
    }

    /// **A hovering creature rests exactly a yard over the floor it found** — and, faithfully,
    /// **does not descend at all when that floor is under a yard away** (decision 1780). The
    /// reference finalises `z = z_before_snap − max(L − 1.0, 0)` (`0x636e81`-`0x636ea9`), so the
    /// grant is not "+1 yd" but "stop a yard short of the snap, and never move *up*".
    ///
    /// The second half is the one worth pinning: written as a plain `floor + 1.0` the same grant
    /// would *raise* a creature the server had already placed inside that yard, which is a body
    /// popping upward on an aura that is supposed to leave it where it is.
    #[test]
    fn a_hovering_creature_floats_a_yard_and_never_climbs() {
        let (mut app, npc) = half_arrived_world();
        clamp(&mut app);
        assert_eq!(y_of(&app, npc), TERRAIN_Y, "the plain snap, for reference");

        // The grant arrives on a body that has not moved: the cached answer must not outlive it.
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

        // Now the near case: seat the body less than a yard over its floor. `min(seat)` holds it
        // where the server put it rather than lifting it to the clearance.
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

    /// **A re-sent identical pose must not be held at the wire Z** (decision 1921). The cache gate
    /// asks whether the ray's inputs changed; until this test it did not also ask whether its own
    /// answer was still *standing*, so a unit whose Y was rewritten to a pose with the **same seat**
    /// — which is exactly what a periodic `SMSG_UPDATE_OBJECT` or a stop packet restating a
    /// stationary unit's position is — matched the cached question and was left floating at the
    /// wire Z until something moved it.
    ///
    /// Found by B357's reproduction, on a bot between two paths; the defect is the creature's too,
    /// which is why it is pinned here.
    #[test]
    fn a_pose_re_sent_unchanged_is_re_grounded_not_held() {
        let (mut app, npc) = half_arrived_world();
        clamp(&mut app);
        assert_eq!(y_of(&app, npc), TERRAIN_Y);

        // The wire restates the pose it already sent: same XZ, same Z, so the same seat — and a
        // standing Y that is no longer the one the clamp left.
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

    /// The water-walker's floor election, and the one that proves it is an *election*: the surface
    /// wins over a lakebed below it, and loses to ground above it. The reference gets this for
    /// free — `MOVEFLAG_WATERWALKING` ORs the liquid layers into the walk trace's class mask
    /// (`0x63162e`), so one trace returns whichever is nearer the feet. Ours queries liquid
    /// separately, so the max() is that election written out (decision 1780).
    ///
    /// With no liquid in this harness the mode must be inert — which is the assertion that would
    /// catch a `max` against a garbage surface (a `f32::MIN` sentinel, an unwrapped `None`).
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
        // 1357's saving, still intact: the gate holds an answer whose three inputs are unchanged.
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
        // The miss branch: an airborne/unstreamed unit is left at its seat, never at whatever the
        // clamp last wrote. The whole `GROUND_CLAMP_DOWN` reach below the seat is empty air here.
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

    /// A ground path is a straight segment lerp: midway through the first leg of an L-shaped path
    /// sits exactly on the chord, facing along it.
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

    /// A flying path still passes through every waypoint (the C-R curve interpolates, it doesn't
    /// approximate): at the chord-length boundary the sample is the corner point exactly.
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

    /// A flying path *curves*: mid-segment on the same L-path the C-R sample leaves the straight
    /// chord (the taxi look — decision 0484 I3), where the ground sampler would sit on it.
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

    /// The flying attitude on a straight climb (decision 0516 — `0x7c5490`'s flying branch): the
    /// pitch is the TANGENT's own climb (45° here), and a straight path never banks — the
    /// look-ahead direction coincides with the tangent, so θ = 2·acos(1) = 0.
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

    /// The bank's sign law (0516): the θ between the XY tangent and the 1 s look-ahead direction,
    /// ×2 — a LEFT turn (WoW CCW, `cross > 0`) leans left (+), the mirrored right turn leans
    /// right (−), and a gentle divergence stays inside the ±π/2 snap threshold.
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

    /// A create-block spline is joined **where the server already is**, not restarted from the top
    /// (decision 0708): a 20-yd path 12 s long, 9 s of it already ridden, samples three quarters
    /// along — 15 yd in. Restarting it (the naive read of the same packet) would sample at 0 and put
    /// the creature back at the start.
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

    /// A ride the server has already finished — and a degenerate one — leave the unit at its create
    /// pose rather than replaying a walk that is over.
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

    /// The SNAP past ±π/2 (0516 — `0x7c5573..`: a snap to ±π, not a soft clamp): approaching a
    /// hairpin, the look-ahead lands on the return leg, the divergence doubles past π/2, and the
    /// bank pins to a full ±π roll — the reference's momentary switchback barrel-roll.
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

/// **B311, in a world small enough to assert on** — [`creature_swim_state`] at the reported
/// giant's own numbers. The Shore Strider is a sea giant with no `CREATURE_STATIC_FLAG_CAN_SWIM`,
/// so no `UNIT_FLAG_USE_SWIM_ANIMATION` reaches the client: the reference wades it on its legs
/// however deep the water. The same depth with any one of the three gate bits set must still
/// swim — before this gate benilla swam both, which is the "gliding in deep water" the report
/// saw, and a fix that stopped every creature swimming would be the same bug wearing a hat.
#[cfg(test)]
mod swim_gate {
    use super::{creature_swim_state, CREATURE_SWIM_EXIT_BAND};
    use crate::player::swim_enter_depth;

    /// The Shore Strider's real numbers, from the shipped DBCs: display 4945 → CreatureModelData 35
    /// (`Creature\SeaGiant\SeaGiant.mdx`), `collisionHeight` 2.083, `CreatureDisplayInfo.scale`
    /// 1.75 over `modelScale` 1.0 — so `h = 2.083 × 1.75` and the boundary is `0.75·h`.
    const SEA_GIANT_H: f32 = 2.083 * 1.75;

    const NO_FLAGS: u32 = 0;
    const USE_SWIM_ANIMATION: u32 = 0x8000;
    const PLAYER_CONTROLLED: u32 = 0x8;
    const PET_IN_COMBAT: u32 = 0x800;

    #[test]
    fn a_sea_giant_walks_the_lakebed_however_deep_the_water() {
        let boundary = swim_enter_depth(SEA_GIANT_H);
        // Chest-deep — past the boundary that used to be the whole test — and then absurdly deep.
        for depth in [boundary + 0.01, boundary * 2.0, 100.0] {
            assert!(
                !creature_swim_state(false, depth, boundary, NO_FLAGS),
                "a unit with no swim flag must never enter swim (depth {depth})"
            );
        }
    }

    /// The control that must not change: the gate is a gate, not a switch-off. Any ONE of the three
    /// bits still admits the unit, and the depth law under it is untouched.
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

    /// Leaving keeps the 1/36-yd hysteresis, and its compare is **inclusive at equality** while
    /// enter's is strict — `0x6031c5 test ah,5` + `jnp` is a parity test on C0|C2, so `depth ==
    /// thr` does not stop, whereas `0x603106 test ah,0x41` + `jne` is a ZF test and `depth ==
    /// boundary` does not start. Deliberately asymmetric; that asymmetry is the band.
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

    /// **The correction 1572 makes to 1568.** The three bits are one predicate — *may this unit be
    /// locally SWIMMING at all* — and `0x6030c0` tests it on the **exit** leg too, inverted: with
    /// all three clear the exit leg falls straight through to `StopSwim` (`0x6031eb → 0x60dff0`)
    /// however deep the water. A charm ending over deep water is the case: the creature loses
    /// PLAYER_CONTROLLED and must drop to the seabed on the next tick, not keep swimming until it
    /// finds a shallow.
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

    /// A unit whose descriptor block has not landed reads flags 0 — it must not enter on that,
    /// and it must not be *stuck*: the system's `Changed<ObjectStore>` gate re-asks on the insert.
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

/// **B357, in a world small enough to assert on**: a Playerbot walking a hollow, driven by a
/// grounded server spline, and the four bodies that must *not* be dragged along with it.
///
/// The reported picture is an orc a body-height clear of a snow slope in mid-stride, and this is
/// the mechanism behind it in three facts: the server's waypoints sit on the ground at the two
/// rims, the spline between them is a **straight chord**, and a hollow is exactly where a chord
/// departs from the surface. A creature riding it is put back on the ground every frame; a Player
/// riding the identical path was skipped by a `kind == Unit` test and kept the chord's Z verbatim.
///
/// The trench is trapezoidal rather than a V on purpose: its floor is flat under the whole
/// mid-path window, so every assertion below is an exact equality that no sampling instant can
/// move.
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

    /// The rims the server's two waypoints stand on, and the trench floor between them. The 2.5-yd
    /// difference is the mid-stride error the report photographed — a body-height, and well inside
    /// the probe's own reach so what the test measures is the subject rule, not the window.
    const RIM_Z: f32 = 10.0;
    const FLOOR_Y: f32 = 7.5;

    /// The two server waypoints, raw WoW. Both stand on a rim, so the chord between them is
    /// **level at [`RIM_Z`] for its whole length** — which is what makes every assertion here
    /// independent of the instant the spline is sampled at.
    const RIM_A: [f32; 3] = [0.0, 9.0, RIM_Z];
    const RIM_B: [f32; 3] = [0.0, -9.0, RIM_Z];

    /// A ground strip: the `(bevy x, bevy y)` profile extruded ±5 along Bevy Z, wound so its faces
    /// point up — the one-sided down-ray (decision 0970) stands only on those.
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

    /// The hollow, and one body standing on the chord over its floor. `spawn` says what kind of
    /// body and what else is driving it; the pose is the one [`sample_splines`] writes mid-path.
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
        // The body a walker's swept step sweeps (decision 2018) — idle units never touch it.
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

    /// The path the server has this body on — a plain ground walk, the shape `SMSG_MONSTER_MOVE`
    /// sends for a bot and for a charged/knocked-back/feared player alike.
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

    /// Standing on the trench floor, to a millimetre. The clamp answers `origin.y − hit.distance`,
    /// so its result carries the ray's own float error (7.4999995 for this geometry) — and the
    /// thing under test is a 2.5-yd wire-vs-terrain disagreement, which no tolerance this side of a
    /// yard could hide. The *seat* assertions stay exact: a seat is copied, never computed.
    #[track_caller]
    fn assert_grounded(app: &App, e: Entity, why: &str) {
        let y = y_of(app, e);
        assert!(
            (y - FLOOR_Y).abs() < 1e-3,
            "{why}: expected the trench floor {FLOOR_Y}, got {y}"
        );
    }

    /// **The report.** A Player object the server walks along a ground spline is grounded exactly
    /// like the creature on the identical path — and the creature is asserted beside it, because
    /// "the same answer for both kinds" is the whole claim.
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

    /// **A flying path keeps its own Z, for a player too.** The reference's vertical-zero gate
    /// keeps Δz when the spline carries `FLYING` (`0x616cec` reads the MI flag word, not the
    /// object's kind), so a taxi is not walked into the hillside it passes over.
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

    /// **A bot does not pop back onto the chord between paths.** `sample_splines` drops a finished
    /// [`Spline`] before the clamp runs, so on the last frame of every path the live-spline half of
    /// the subject rule is already false; membership is carried by the [`GroundClamped`] memo the
    /// ride left. That is the reference's shape as well — `0x619ca0` splices the CMovement into the
    /// per-frame list and nothing in the recorded notes unlinks it.
    #[test]
    fn a_bot_stays_grounded_between_paths() {
        let (mut app, body) = hollow(EntityKind::Player);
        walking(&mut app, body);
        clamp(&mut app);
        assert_grounded(&app, body, "mid-path");

        // The path finished: the sampler has taken the spline away.
        app.world_mut().entity_mut(body).remove::<Spline>();
        // The server puts it somewhere new — a fresh pose, still on the chord's level.
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

    /// **The body we steer is the controller's**, spline or no spline. Our own Charge rides
    /// `player::server_ride`, whose pose the swept capsule owns; a second opinion from a down-ray
    /// would be two systems writing one Y.
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

    /// **A relayed remote player is the relay's** — decision 0059's deliberate deferral, kept. Their
    /// Z arrives already grounded by the client that sent it, and the dead-reckon owns their jump
    /// arc, which a down-ray would flatten. Both halves are asserted: one who has just streamed in
    /// and moved nobody's way yet, and one under a live [`RemoteMotion`].
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

    /// **A swimming body keeps its wire Z**, and a Player is now one of the bodies that can be
    /// marked. This is the half B357's fix would have broken silently: admit a splined player to
    /// the clamp without deriving its swim state and a bot crossing a lake is dragged to the bed.
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

    /// **The swim mark walks the clamp's population, not a second one.** Pinned through its EXIT
    /// leg, which needs no liquid: in a dry world a marked body must be unmarked, and only a body
    /// the mark actually visits can be. The relayed player beside it is the control — its swim flag
    /// arrives on the wire, so deriving a second answer for it is not ours to do.
    ///
    /// The mark's own depth law (`creature_swim_state`) is pinned as a pure function elsewhere;
    /// what is new here is *who* it is asked about — and that a player passes the flag gate at all,
    /// which is the last assertion: bit 3 PLAYER_CONTROLLED is set on every player object.
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

    /// The subject rule as a table, including the rows no world above can reach: a GameObject
    /// (authored Z — a lamp on a table) and the two ways a Player leaves the set again.
    #[test]
    fn the_subject_rule() {
        let unit = |splined, memo, embodied, relayed| {
            ground_derived(EntityKind::Unit, splined, memo, embodied, relayed)
        };
        let player = |splined, memo, embodied, relayed| {
            ground_derived(EntityKind::Player, splined, memo, embodied, relayed)
        };
        // A creature is ours walking or idle — decision 0059's law, untouched.
        assert!(unit(true, true, false, false));
        assert!(unit(false, false, false, false));
        // …until we are steering it. A possessed creature is the controller's (decision 1927).
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

/// **B372, and the probe law behind it** (decision 2018): a unit the server stands INSIDE a
/// GameObject's collision box — Galen Goodward in his cage, to the yard — and what a probe that
/// starts at the body finds there, idle and walking; then the walker's two other duties, in the
/// same harness: riding a hill it would otherwise chord under, and re-basing on a new path.
///
/// The box is wound outward like every M2 collision hull (`G_Cage.mdx` is one such box, 8 vertices
/// / 12 triangles), so from inside every face is a backface: a down-ray passes the box's floor to the
/// terrain, an up-probe passes its lid. The one thing that can ever put a body on the lid is a ray
/// that STARTS above it — which is exactly what the clamp did before this record.
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

    /// Galen's cage, relative to the terrain under it: the collision box's floor 0.4 yd up, its
    /// lid 2.72 yd up (`G_Cage.mdx`: z 0.398..2.724 at size 1), and his seat 0.07 above the box
    /// floor (the deployed spawn: 22.4164 against a box floor at 22.34).
    const TERRAIN_Y: f32 = 22.0;
    const BOX_FLOOR_Y: f32 = TERRAIN_Y + 0.4;
    const LID_Y: f32 = TERRAIN_Y + 2.72;
    const SEAT_Y: f32 = BOX_FLOOR_Y + 0.07;
    const HALF: f32 = 1.43;

    /// A 30×30 up-wound quad at `y`, around the origin — or, for the shipped-placement test,
    /// around the cage's own footprint.
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

    /// A closed box hull wound **outward** on every face — checked, not assumed: each triangle's
    /// winding normal is flipped to point away from the box's centre, because under the one-sided
    /// law a face wound inward is a hole, and a fixture with a hole would pass this module for
    /// the wrong reason.
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
        // The walker's swept step needs the body it sweeps, exactly as the remote dead-reckon does.
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

    /// A grounded path from `a` to `b` over `secs`, installed on `e` (the sampler is not run here;
    /// the tests move the transform themselves, one "frame" at a time, which is what the clamp
    /// sees: a fresh sample's xz, with the chord's Z as its seat).
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

    /// **B372.** Idle at the server's seat inside the box, the unit stands anywhere but on the
    /// lid: from the seat, the box's own floor is a backface for a down-ray and the lid is above
    /// and behind, so the ray finds the terrain — never a surface 2.2 yd over its head.
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

    /// The same body walking inside the cage: the swept step from where it stood, Δz = 0, meets
    /// only backfaces — the walls from inside, the lid from below — so it walks its floor and is
    /// never lifted onto the lid.
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

    /// **0059's hill, kept.** A server chord between two hilltop waypoints cuts through the hill;
    /// a walker riding it frame by frame stays ON the hill because each frame's swept step starts
    /// from the surface it stood on and rides the rise — the reference's mechanism, which needs no
    /// probe from above.
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

    /// **B372 at the shipped mesh.** `G_Cage.mdx`'s real hull at the deployed placement (guid
    /// 29361: (−9898.3, −3724.76, 21.9428), `GAMEOBJECT_ROTATION` `(0, 0, 0.909961, 0.414694)`),
    /// the terrain under it at the height the live clamp measured before the cage's collider
    /// attached (21.958), and a unit at Galen's spawn. Skips without client data.
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

    /// **A new path re-bases from the server's own Z.** Continuity is one path long: the memo of a
    /// walker under a floor that streamed in late is not carried into its next path, whose start
    /// is the server's position on that floor — the reference re-bases its mover on every inbound
    /// movement packet (`0x7c6420`), and this is that.
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
