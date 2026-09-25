//! The mover's side of the `WOW_MOVE_TRACE` debug trace ([`benilla_assets::trace`]), one tag per
//! question a live run answers in numbers:
//!
//! - `move`: an interesting frame of the player mover ([`frame`]): a step-down snap, a grounded
//!   flip, an airborne frame or a sizeable vertical delta. The walk arm alone emits it.
//! - `swim`: a frame over liquid ([`swim`]): the depth against the two latch thresholds.
//! - `mvr`: a mover-claim packet ([`mover_claim`]), which decides whether the server accepts `snd`.
//! - `sit`: a stand-state decision ([`posture`]), committed or refused, from `X` or `/sit`.
//! - `gait`: a walk/run toggle decision ([`gait`]), committed or refused.
//! - `knb`: a knockback ([`knockback`]), flown or refused.
//! - `snd`: an outbound `MSG_MOVE_*` ([`sent`]), diffable against a 1.12.1 capture's client
//!   stream; `net::motion`'s `rly` is the receive side.
//! - `sett`: how the settle hold ended ([`settle`]) and the time it skipped ([`skipped_time`]).
//! - `rid`: a frame of a self-spline ride ([`ride`]).
//!
//! The anim driver writes `anim` lines into the same file on the same clock, and
//! [`super::ride`] writes the transport carry's `ride`.

use std::sync::atomic::{AtomicBool, Ordering};

use benilla_assets::trace;

/// What the mover did this frame, filled at the end of the physics step in [`super`].
pub(super) struct Frame {
    /// Feet height entering the step (yd).
    pub y_in: f32,
    /// Feet height leaving the step (yd), after the slide and the step-down snap.
    pub y_out: f32,
    /// Horizontal distance this frame (yd). With the drop it is the descent slope, which the
    /// reference bounds at [`super::STEP_SLOPE_RATIO`] per unit of travel (`0x636dda`).
    pub dx: f32,
    pub grounded: bool,
    pub on_walkable: bool,
    /// The keys asked for horizontal motion; travel without it is the resolve moving the body.
    pub moving: bool,
    pub vel_y: f32,
    /// The step-down snap when it ran: `(probe reach, Some((hit distance, hit normal.y)))`, a steep
    /// hit included.
    pub snap: Option<(f32, Option<(f32, f32)>)>,
    /// The atomic step-up's committed height gain this frame (yd), when the maneuver ran.
    pub climb: Option<f32>,
    /// The root's anchor ran (no gravity, slide or snap), telling a rooted hang from a stuck mover.
    pub anchored: bool,
}

/// One `snd` line per outbound movement packet: the opcode, the move flags, facing and position.
pub(super) fn sent(kind: crate::net::MoveKind, flags: u32, facing: f32, pos: [f32; 3]) {
    if !trace::enabled() {
        return;
    }
    trace::line(
        "snd",
        &format!(
            "{kind:?} flags={flags:#x} o={facing:.4} pos=[{:.2},{:.2},{:.2}]",
            pos[0], pos[1], pos[2]
        ),
    );
}

/// One `sit` line per stand-state decision. `what` is `commit` or `REFUSED`, `state` the
/// `UnitStandStateType` asked for (0 STAND, 1 SIT, 2 SIT_CHAIR, 3 SLEEP, 8 KNEEL), `from` the
/// current one, and `flags` the `CMovement` word the reference's gate reads
/// (`[[this+0x118]+0x40]`), its swim and move bits spelled out. The reference refuses silently.
pub(super) fn posture(what: &str, state: u8, from: u8, flags: u32) {
    if !trace::enabled() {
        return;
    }
    use crate::creature_anim::move_flags as f;
    trace::line(
        "sit",
        &format!(
            "{what} state={state} from={from} flags={flags:#x} swim={} moving={}",
            u8::from(flags & f::SWIMMING != 0),
            u8::from(flags & f::ANY_MOVE != 0),
        ),
    );
}

/// One `gait` line per walk/run toggle: `commit` or `REFUSED`, the gait asked for, and the three
/// silent gates of the reference's chain (`0x513d8e`–`0x513dcd`) one by one.
pub(super) fn gait(what: &str, to: bool, dead: bool, rooted: bool, on_spline: bool) {
    if !trace::enabled() {
        return;
    }
    trace::line(
        "gait",
        &format!(
            "{what} to={} dead={} rooted={} spline={}",
            if to { "walk" } else { "run" },
            u8::from(dead),
            u8::from(rooted),
            u8::from(on_spline),
        ),
    );
}

/// One `knb` line per knockback, flown or refused, with the launch quad as it arrived. Nothing else
/// shows one: the ack is not a `MSG_MOVE_*`, the reference discards the record under a root, and
/// vmangos drops a wrong ack (`OnWrongAckData`) without relaying the knockback. `up` is the +Z
/// takeoff speed beside the wire's down-positive `zspeed`; the two always read as negatives.
pub(super) fn knockback(flown: bool, launch: benilla_protocol::JumpInfo) {
    if !trace::enabled() {
        return;
    }
    trace::line(
        "knb",
        &format!(
            "{} cos={:.4} sin={:.4} xy={:.3} zspeed={:.3} up={:.3}",
            if flown { "flown" } else { "REFUSED" },
            launch.cos_angle,
            launch.sin_angle,
            launch.xy_speed,
            launch.zspeed,
            -launch.zspeed,
        ),
    );
}

/// One `mvr` line per `CMSG_SET_ACTIVE_MOVER` or `CMSG_MOVE_NOT_ACTIVE_MOVER`. vmangos checks every
/// movement packet against the confirmed mover and logs a mismatch only server-side.
pub(super) fn mover_claim(what: &str, guid: u64) {
    if !trace::enabled() {
        return;
    }
    trace::line("mvr", &format!("{what} guid={guid:#x}"));
}

/// One `swim` line per frame over liquid: waterline, feet, depth, the two latch thresholds and the
/// regime, `<` or `>` marking a depth past a threshold. The thresholds derive from `h`, the
/// avatar's own collision height, which the line echoes.
pub(super) fn swim(feet_y: f32, surface_y: f32, swimming: bool, h: f32) {
    if !trace::enabled() {
        return;
    }
    let (enter, exit) = (
        super::swim::swim_enter_depth(h),
        super::swim::swim_exit_depth(h),
    );
    let depth = surface_y - feet_y;
    let band = if depth > enter {
        '>'
    } else if depth < exit {
        '<'
    } else {
        '='
    };
    trace::line(
        "swim",
        &format!(
            "y {feet_y:9.3} surf {surface_y:9.3} depth {depth:6.3} {band} [{exit:.3}..{enter:.3}] h {h:.3} mode={}",
            if swimming { "swim" } else { "walk" }
        ),
    );
}

/// One `sett` line per `CMSG_MOVE_TIME_SKIPPED` we send: the ms the settle hold skipped, and whose.
pub(super) fn skipped_time(guid: u64, lag_ms: u32) {
    if !trace::enabled() {
        return;
    }
    trace::line(
        "sett",
        &format!("skipped {lag_ms:5} ms for mover {guid:#x}"),
    );
}

/// How the post-teleport settle hold ended: `resident`, the destination's world arrived (scene
/// spawned, collider queue quiet); otherwise the [`super::SETTLE_TIMEOUT`] backstop, a stream with
/// no progress for the whole budget. `waited` runs from the snap. The two look alike in game, so a
/// timeout also warns on the ordinary log.
pub(super) fn settle(resident: bool, waited: f32, pos: bevy::prelude::Vec3) {
    if !resident {
        bevy::log::warn!(
            "settle: TIMED OUT {waited:.2}s after the snap with the stream stalled and the world \
             never resident at ({:.1},{:.1},{:.1}) — releasing anyway. If a building stands \
             here, its collider never streamed and the body is about to fall through it.",
            pos.x,
            pos.y,
            pos.z,
        );
    }
    if !trace::enabled() {
        return;
    }
    trace::line(
        "sett",
        &format!(
            "{} after {waited:6.2}s at ({:8.2},{:7.2},{:8.2})",
            if resident {
                "world resident"
            } else {
                "TIMED OUT     "
            },
            pos.x,
            pos.y,
            pos.z,
        ),
    );
}

/// One `rid` line per frame of a self-spline ride (Charge, knockback, fear, taxi), during which the
/// controller and its `move` line are parked. `wire` is the Z the spline put us at, `ground` the
/// walkable surface under it (`miss`: nothing in reach, and the ride keeps its own Z), `z` where
/// the ride left us; `gap` is `z − ground` and `chord` is `wire − ground`.
pub(super) fn ride(
    spline_id: u32,
    grounded: bool,
    pos: bevy::prelude::Vec3,
    wire_y: f32,
    ground: Option<f32>,
) {
    if !trace::enabled_for("rid") {
        return;
    }
    let wow = benilla_assets::coords::bevy_to_wow(pos);
    trace::line(
        "rid",
        &format!(
            "{spline_id} {} pos=({:8.2},{:8.2},{:7.2}) z={:7.2} wire={wire_y:7.2} {}",
            if grounded { "ground" } else { "flying" },
            wow[0],
            wow[1],
            wow[2],
            pos.y,
            match ground {
                Some(g) => format!(
                    "ground={g:7.2} gap={:+7.3} chord={:+7.3}",
                    pos.y - g,
                    wire_y - g
                ),
                None => "ground=miss".to_string(),
            },
        ),
    );
}

static PREV_GROUNDED: AtomicBool = AtomicBool::new(true);

pub(super) fn frame(f: Frame) {
    if !trace::enabled() {
        return;
    }
    let dy = f.y_out - f.y_in;
    let snap_dist = f
        .snap
        .and_then(|(_, hit)| hit)
        .map_or(0.0, |(dist, _)| dist);
    let flipped = f.grounded != PREV_GROUNDED.swap(f.grounded, Ordering::Relaxed);
    // Travel with no input, the resolve moving the body; a chair push-out runs 1–2 mm a frame.
    let creep = !f.moving && f.dx > 1.0e-4;
    if !(flipped
        || !f.grounded
        || dy.abs() > 0.05
        || snap_dist > 0.05
        || f.climb.is_some()
        || creep)
    {
        return;
    }
    let snap = match f.snap {
        None => "snap -".to_string(),
        Some((reach, None)) => format!("snap miss (reach {reach:.2})"),
        Some((reach, Some((dist, ny)))) => format!(
            "snap d={dist:.3} ny={ny:.3} (reach {reach:.2}){}",
            if ny >= super::GROUND_COS {
                ""
            } else {
                " STEEP"
            }
        ),
    };
    // While descending and moving, `>cone` marks a frame steeper than the foot cone allows.
    let slope = if dy < -1.0e-4 && f.dx > 1.0e-4 {
        let s = -dy / f.dx;
        format!(
            " dx={:.3} slope={s:.2}{}",
            f.dx,
            if s > super::STEP_SLOPE_RATIO {
                " >cone"
            } else {
                ""
            }
        )
    } else {
        format!(" dx={:.3}", f.dx)
    };
    // A body leaving the surface travels forward with `dy ≈ 0` over a steep face below it; the
    // arc that follows starts shallow (slope 0.05), so `>cone` misses the takeoff.
    let left = f
        .snap
        .and_then(|(_, hit)| hit)
        // The steep hit must be at a distance: `d = 0` is the body sliding flush along a bank.
        .is_some_and(|(dist, ny)| ny < super::GROUND_COS && dist > 1.0e-3)
        // Flat, not merely not descending: a window around zero keeps out step-ups and hill climbs.
        && dy.abs() < 1.0e-3
        && f.dx > 1.0e-4;
    let left = if left { " LEFT-SURFACE" } else { "" };
    let climb = f.climb.map_or(String::new(), |t| format!(" climb={t:+.3}"));
    let anchored = if f.anchored { " ROOTED" } else { "" };
    let creep = if creep { " CREEP" } else { "" };
    trace::line(
        "move",
        &format!(
            "y {:9.3} -> {:9.3} dy={:+.3}{} grounded={} walk={} vy={:+7.2} {}{}{}{}{}",
            f.y_in,
            f.y_out,
            dy,
            slope,
            f.grounded as u8,
            f.on_walkable as u8,
            f.vel_y,
            snap,
            left,
            climb,
            anchored,
            creep
        ),
    );
}
