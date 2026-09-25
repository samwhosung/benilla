//! The player's kinematic mover, one [`step`] a frame: ground classify, jump and fall, the slide,
//! step-up and step-down, over avian's `MoveAndSlide` with one-sided faces (a face blocks only the
//! motion its authored winding opposes, the reference's `0x632700`).

use avian3d::character_controller::move_and_slide::MoveHitData;
use avian3d::prelude::*;
use bevy::prelude::*;

use super::{
    move_trace, Player, CAPSULE_HEIGHT, FEATHER_TERMINAL_VELOCITY, FOOT_CONE_HEIGHT, GRAVITY,
    GROUND_COS, GROUND_PROBE, HOVER_CLIMB_RATE, HOVER_HEIGHT, JUMP_SPEED, LAND_PROBE, SKIN_WIDTH,
    STEP_SLOPE_RATIO, STEP_SNAP_SLACK, STEP_UP_ADVANCE_PER_YARD, STEP_UP_HEIGHT, TERMINAL_VELOCITY,
    WATER_WALK_PITCH_FLOOR, WEDGE_MIN_FALL, WEDGE_STALL_RATIO, WEDGE_STILL_FRAMES,
};

/// The trace-mask arm `0x6315f0` as a predicate: the liquid surface [`step`] may stand on this
/// frame, or `None`. Its three gates, in the reference's order:
/// - `water_walking` (`0x631610`) ORs the two ADT liquid layers into the walk trace's class mask
///   (`0x63162e`), which is the whole of the flag: the surface is swept geometry at its raw Z.
/// - `!swimming` (`0x631617`): the swim trace takes the same layers as a bound above (`0x6320fe`),
///   so the grant does not eject a swimmer, who stands on the water once the depth compare ends
///   SWIMMING under `0.75·h − 1/36` (`0x6030c0`). Levitate still lands a swimmer on the water:
///   the hover grant jumps the body first (`0x61a620`), clearing SWIMMING.
/// - `mover_pitch` strictly above [`WATER_WALK_PITCH_FLOOR`] (`0x63161e`): aiming lower drops the
///   layers and sinks the body into the swim, the reference's way back into the water.
pub(super) fn water_floor(
    water_walking: bool,
    swimming: bool,
    mover_pitch: f32,
    surface_y: Option<f32>,
) -> Option<f32> {
    (water_walking && !swimming && mover_pitch > WATER_WALK_PITCH_FLOOR)
        .then_some(surface_y)
        .flatten()
}

/// What [`step`] decided, for the move flags and the wire that follow it in `control`.
pub(super) struct Outcome {
    /// The settle hold while the world streams in after a teleport: frozen, gravity off.
    pub held: bool,
    /// Supported and not rising this frame.
    pub grounded: bool,
    /// A jump took off this frame.
    pub jumped: bool,
    /// The take-off was a knockback's, which also sets [`Self::jumped`]; the wire answers it with
    /// `CMSG_MOVE_KNOCK_BACK_ACK` alone. `MSG_MOVE_JUMP`'s one emission site is the jump arm
    /// (`0x615ed1`), and vmangos flags a JUMP this fast as `CHEAT_TYPE_OVERSPEED_JUMP`, the jump
    /// check it does not exempt during a knockback (`MovementAnticheat.cpp:650`).
    pub knocked: bool,
    /// The standstill-jump air nudge fired, which re-seeds the frozen airborne direction flags.
    pub air_nudged: bool,
    /// The walkable floor's collider: the snap's hit when it ran, else the classify's. Standing on
    /// a transport's collider attaches the mover to its frame.
    pub ground: Option<Entity>,
}

/// Advance the player mover one frame, writing its position and velocities.
pub(super) fn step(
    player: &mut Player,
    time: &Time,
    world: &benilla_world::collision::WorldCollision<'_, '_>,
    capsule: &Collider,
    moving: bool,
    dir: Vec3,
    speed: f32,
    want_jump: bool,
    wire_jump: bool,
    knockback: Option<Vec3>,
    water_floor: Option<f32>,
    nudge_speed: f32,
) -> Outcome {
    let dt = time.delta_secs();
    let input_horiz = if moving {
        dir.normalize() * speed
    } else {
        Vec3::ZERO
    };
    let half_h = Vec3::Y * (CAPSULE_HEIGHT * 0.5);
    let mut center = player.pos + half_h;
    let cast = |from: Vec3, disp: Vec3| world.cast_mover(capsule, from, disp, SKIN_WIDTH);
    let probe_down = |c: Vec3, dist: f32| cast(c, Vec3::NEG_Y * dist);

    // Airborne, the ground reach tightens to [`LAND_PROBE`] so the arc ends where the slide meets
    // the floor. A hovering body rests [`HOVER_HEIGHT`] up, so every ground reach grows by it.
    let hover_offset = if player.modes.hover {
        HOVER_HEIGHT
    } else {
        0.0
    };
    let base_reach = if player.airborne_since.is_some() {
        LAND_PROBE
    } else {
        GROUND_PROBE
    };
    let ground_reach = hover_offset + base_reach;
    let classify = probe_down(center, ground_reach);
    let on_walkable = classify.as_ref().is_some_and(|h| h.normal1.y >= GROUND_COS);
    // Water walking makes the liquid surface ground (`0x63162e`). Liquid is queried here, not
    // swept, so the classify asks the plane too, or a water-walker reads airborne and coasts. The
    // reach includes the hover offset: the finalize probe (`0x636e45` → `0x632ba0` → `0x631e70`)
    // measures to the water, so hover rests a yard over it. Feet under the surface count too.
    let on_water = water_floor.is_some_and(|s| center.y - half_h.y - s <= ground_reach);
    let mut ground_entity = if on_walkable {
        classify.map(|h| h.entity)
    } else {
        None
    };
    // The settle hold after a teleport or login: the world streams in over several frames, so the
    // body is frozen with gravity off. The terrain streamer releases it on the destination's
    // residency, never on ground contact, which a flyer or a swimmer never makes.
    let held = player.settling;
    // Rooted, nothing moves the body: `SetRoot` (`0x7c7340`) clears FALLING (`0x7c6290`) and the
    // direction bits, so the dispatcher (`0x634040`) skips the fall integrator (`0x635b00`) and the
    // walk resolver returns on a step under `2^-20`. A root caught mid-air hangs until `ClearRoot`
    // (`0x7c7370`) starts the fall (`0x7c61c0`, which refuses while rooted, `0x7c61d6`).
    let anchored = !held && player.modes.rooted;
    // A body part-way up a foot cone stands: the straight-down probe sees only the riser it rides,
    // and the ride is re-certified every frame it continues.
    let on_floor =
        !held && (on_walkable || on_water || player.steep_support) && player.vel_y <= 0.0;

    // The wedge rest holds until real ground takes over or its support goes; its reach stays
    // [`LAND_PROBE`], plus the hover offset.
    if player.wedged
        && (on_floor || held || probe_down(center, LAND_PROBE + hover_offset).is_none())
    {
        player.wedged = false;
    }
    let grounded = on_floor || player.wedged;

    let mut jumped = false;
    let mut knocked = false;
    if held || anchored {
        // Frozen by the settle hold or the root: no gravity, no momentum. A knockback here is
        // dropped, not deferred, and `knocked` stays false so no ack goes out; the reference drops
        // one under root too (`0x615c71`), and vmangos sends none to a rooted unit.
        player.vel_y = 0.0;
        player.horiz_vel = Vec3::ZERO;
    } else if let Some(launch) = knockback {
        // The knockback: the server aims the jump and we fly it, both axes absolute in world XY
        // whatever the facing or keys; `MSG_MOVE_FALL_LAND` ends it, and with it vmangos's
        // knockback grace (`MovementAnticheat.cpp:698`). Hover refuses only the keyboard jump
        // (`0x7c623a`), and
        // vmangos's gate tests stun, root and a live spline, never FALLING (`Unit.cpp:10192`).
        player.vel_y = launch.y;
        player.launch_vz = launch.y;
        player.horiz_vel = Vec3::new(launch.x, 0.0, launch.z);
        player.wedged = false;
        player.steep_support = false;
        jumped = true;
        knocked = true;
        // The launch plants FORWARD in the direction nibble (`0x617a18`) whatever its speed, so
        // even a vertical knockback never steers, and `knock_arc` keeps FORWARD on the wire.
        player.arc_dirs_set = true;
        player.knock_arc = true;
    } else if grounded {
        player.vel_y = 0.0;
        // Hover refuses the keyboard jump silently, the first test in `CMovement::Jump`
        // (`0x7c623a`): the key's replay passes `Jump(1)` (`0x615eba`) and drops a refusal
        // (`0x615ec5`), so no `MSG_MOVE_JUMP` and no retry. `wire_jump` is `Jump(0)`, which skips
        // the test (`0x7c6236`), so a hover grant can launch the hovering body. The mount flourish
        // is diverted upstream in `control` (`0x60dea0`), so hover does not gate it.
        if (want_jump && !player.modes.hover) || wire_jump {
            player.vel_y = JUMP_SPEED;
            player.launch_vz = JUMP_SPEED;
            player.wedged = false;
            player.steep_support = false;
            jumped = true;
            // A moving jump is locked to its momentum; a standing one may steer once.
            player.arc_dirs_set = moving;
        }
    } else if !player.airborne_prev {
        // A step-off: `StartFalling(0)` launches at exactly 0, which the wire tail sends and the
        // FALLINGFAR legs split on; the nibble seeds from the keys.
        player.launch_vz = 0.0;
        player.arc_dirs_set = moving;
    }

    // The fall step runs on every airborne frame, the take-off's included: the reference evaluates
    // its arc in closed form from `t = 0` at `StartFalling`. Feather fall swaps only the terminal
    // speed (`0x7c5d20`). `vel_y` keeps the end-of-step speed that the next frame and the wire
    // read; `fall_mean_vy` is the exact rate to move at ([`fall_step`]).
    let mut fall_mean_vy = player.vel_y;
    if !held && !anchored && (jumped || !grounded) {
        let terminal = if player.modes.feather_fall {
            FEATHER_TERMINAL_VELOCITY
        } else {
            TERMINAL_VELOCITY
        };
        let (end_vy, mean_vy) = fall_step(player.vel_y, dt, terminal);
        player.vel_y = end_vy;
        fall_mean_vy = mean_vy;
    }
    let mut air_nudged = false;
    // The `!anchored` terms keep a body rooted mid-jump stopped dead instead of coasting.
    if knocked {
        // The launch owns both axes: a knockback from standing is still `grounded` this frame, and
        // the walk arm would hand the horizontal back to the keys.
    } else if grounded && !anchored {
        player.horiz_vel = input_horiz;
    } else if !held && !anchored && moving && !player.arc_dirs_set {
        // Air control: one nudge for a jump taken standing, re-seeding the airborne direction
        // flags. The reference opens it only while the direction nibble (`+0x40 & 0xf`) is clear
        // (`0x7c6afc`, guarding the `arg = 1` openers of `0x7c5a20`/`0x7c5c20`); `nudge_speed` is
        // the live `min(walk, run)` (`0x7c4c90(1)`).
        player.horiz_vel = dir.normalize_or_zero() * nudge_speed;
        air_nudged = true;
        // The press sets the nibble, so there is no second steer.
        player.arc_dirs_set = true;
    }

    let pre_move = center;
    let (mut climb, mut snap_probe) = (None, None);
    if !held && !anchored && grounded && !jumped {
        let g = grounded_step(
            world,
            capsule,
            center,
            player.horiz_vel,
            time.delta(),
            Support {
                rise: STEP_UP_HEIGHT,
                offset: hover_offset,
                water: water_floor,
                steep: player.steep_support,
            },
        );
        // Only this mover has a fall to elect, so it spends the no-floor drop (`pos.z -= achieved`
        // at `0x636e52`) and a ledge's first fall frame starts lower. A walk frame that went
        // nowhere writes the `stup` report ([`super::step_probe`]).
        let resolved = g.center - Vec3::Y * g.unsupported.unwrap_or(0.0);
        super::step_probe::watch(
            world,
            capsule,
            center,
            resolved,
            player.horiz_vel,
            dt,
            time.elapsed_secs(),
        );
        center = resolved;
        climb = g.climb;
        snap_probe = g.snap;
        player.steep_support = g.steep_support;
        if let Some(e) = g.ground {
            ground_entity = Some(e);
        }
    } else {
        let velocity = if held || anchored {
            Vec3::ZERO
        } else {
            player.horiz_vel + Vec3::Y * fall_mean_vy
        };
        center = airborne_step(world, capsule, center, velocity, time.delta());
        player.steep_support = false;
    }
    // The wedge rest: falling, yet achieving under [`WEDGE_STALL_RATIO`] of gravity's intent for
    // [`WEDGE_STILL_FRAMES`] frames, is a capsule pinched between steep faces with no way down
    // (a tree trunk's funnel), so it lands there.
    if !held
        && !anchored
        && !grounded
        && !jumped
        && player.vel_y < -WEDGE_MIN_FALL
        && (pre_move.y - center.y) < -player.vel_y * dt * WEDGE_STALL_RATIO
    {
        player.wedge_still += 1;
        if player.wedge_still >= WEDGE_STILL_FRAMES {
            player.wedged = true;
            player.wedge_still = 0;
            player.vel_y = 0.0;
            let feet = center - half_h;
            benilla_assets::trace::line(
                "move",
                &format!(
                    "wedge rest at ({:8.2},{:7.2},{:8.2}) -> landed standing",
                    feet.x, feet.y, feet.z
                ),
            );
        }
    } else {
        player.wedge_still = 0;
    }
    // The detecting frame is grounded, so the landing (`MSG_MOVE_FALL_LAND`) goes out this frame.
    let mut grounded = grounded || player.wedged;

    // The water floor, after the slide: a body landing on water crosses more than [`LAND_PROBE`]
    // in a frame. Down is the reference's snap with its hover arm, `z − max(L − 1, 0)`
    // (`0x636e52`, `0x636e74`), yielding to a walkable collider, the nearer floor; up is the
    // rate-limited climb below (`0x636fa1`). The one push up is ours: the body may not end the
    // frame under the surface, which the reference's swept surface never lets it reach, and only
    // a body that is not rising is caught, so a hover launch under the waterline flies clear.
    // `SetPitch`'s standstill dive kick (`0x7c6f70`) is not built: it serves a walk resolver that
    // skips a body at rest, and our classify runs every frame, so dropping the layers is the dive.
    if let Some(surface) = water_floor {
        let feet = center.y - half_h.y;
        let rest = surface + hover_offset;
        if feet < surface && player.vel_y <= 0.0 {
            center.y = surface + half_h.y;
            player.vel_y = 0.0;
            player.wedged = false;
            grounded = true;
        } else if !on_walkable && feet > rest && feet - surface <= ground_reach {
            center.y = rest + half_h.y;
            player.vel_y = 0.0;
            player.wedged = false;
            grounded = true;
        }
    }

    // The hover climb, the walk resolver's second pass (`0x636fa1`-`0x6370f1`), which a root's
    // early return skips too. The water is one more floor under it, nearest wins.
    if hover_offset > 0.0 && !held && !anchored {
        let solid = probe_down(center, HOVER_HEIGHT + CAPSULE_HEIGHT).map(|h| h.distance);
        let water = water_floor.map(|s| (center.y - half_h.y - s).max(0.0));
        if let Some(clearance) = [solid, water].into_iter().flatten().reduce(f32::min) {
            if clearance < HOVER_HEIGHT {
                center.y += (HOVER_HEIGHT - clearance).min(HOVER_CLIMB_RATE * dt);
                player.vel_y = player.vel_y.max(0.0); // climbing, not falling
            }
        }
    }

    player.pos = center - half_h;
    move_trace::frame(move_trace::Frame {
        y_in: pre_move.y - half_h.y,
        y_out: player.pos.y,
        dx: (center - pre_move).xz().length(),
        grounded,
        on_walkable,
        moving,
        vel_y: player.vel_y,
        snap: snap_probe,
        climb,
        anchored,
    });

    // The mover's own airborne record: a launch frame counts though the body still stands, or the
    // next frame reads as a fresh step-off and re-seeds the arc.
    player.airborne_prev = !held && !anchored && (jumped || !grounded);
    Outcome {
        held,
        grounded,
        jumped,
        knocked,
        air_nudged,
        ground: if grounded && !held {
            ground_entity
        } else {
            None
        },
    }
}

/// The body facts [`grounded_step`] cannot work out for itself, passed in by its caller.
#[derive(Clone, Copy)]
pub(crate) struct Support {
    /// The rise budget, the reference's `H` (`0x617430`), which also scales the certify reach:
    /// [`STEP_UP_HEIGHT`] for a player body, [`super::CREATURE_STEP_UP_HEIGHT`] for a creature
    /// (`0x5fa550`'s false leg).
    pub(crate) rise: f32,
    /// How far above the floor the body rests: [`super::HOVER_HEIGHT`] while hovering, else 0.
    pub(crate) offset: f32,
    /// The liquid floor this frame (Bevy Y), [`water_floor`]'s answer. Liquid is queried, not
    /// swept, so every ground question asks this plane as well as the sweep, where the reference's
    /// one masked trace sees both (`0x63162e`).
    pub(crate) water: Option<f32>,
    /// The support entering the frame is a certified steep contact, the reference's `0x4000000`,
    /// which opens the step-down's deep reach.
    pub(crate) steep: bool,
}

impl Default for Support {
    /// A player body on an ordinary frame.
    fn default() -> Self {
        Self {
            rise: STEP_UP_HEIGHT,
            offset: 0.0,
            water: None,
            steep: false,
        }
    }
}

/// What one [`grounded_step`] resolved.
pub(crate) struct GroundedStep {
    /// The resolved capsule centre.
    pub(crate) center: Vec3,
    /// The walkable floor's collider the snap settled onto; `None` keeps what the caller believed.
    pub(crate) ground: Option<Entity>,
    /// The height (yd) this frame's pop or cone ride gained.
    pub(crate) climb: Option<f32>,
    /// On a certified steep contact, the reference's `0x4000000`: riding a cone up, or following a
    /// surface down off a ledge. The caller keeps it grounded until a walkable floor takes over.
    pub(crate) steep_support: bool,
    /// For the trace: the snap's reach and the `(distance, normal.y)` it found.
    pub(crate) snap: Option<(f32, Option<(f32, f32)>)>,
    /// The no-floor drop, when the snap found nothing and no ride is in progress; not in
    /// [`Self::center`]. The reference drops by it, then elects a fall (`0x636e52`), so only
    /// [`step`], which has a fall to elect, spends it. The open-loop creature and remote callers
    /// hold the server's pose instead, since spending it would ratchet them down through the world.
    pub(crate) unsupported: Option<f32>,
}

/// One grounded walk step against the world: step-up, slide, then the step-down snap. The
/// reference runs every mover through one controller (`0x616620`, whose local-player compare at
/// `0x6166a9` gates only a timing budget) and reads a grounded mover's Z off the surface
/// (`0x633840`, `0x6367b0`), so our mover, the remote dead-reckon and the creature path clamp all
/// walk through here; airborne and swimming frames do not.
pub(crate) fn grounded_step(
    world: &benilla_world::collision::WorldCollision<'_, '_>,
    capsule: &Collider,
    center: Vec3,
    horiz_vel: Vec3,
    dt: std::time::Duration,
    support: Support,
) -> GroundedStep {
    let surface_offset = support.offset;
    let cast = |from: Vec3, disp: Vec3| world.cast_mover(capsule, from, disp, SKIN_WIDTH);
    let speed = horiz_vel.length();
    let attempt = if speed > 1.0e-6 {
        let travel = speed * dt.as_secs_f32();
        // The look-ahead is this frame's travel; the advance is the body's certify reach,
        // `rise · tan 50°`, or the travel when a long frame covers more.
        step_up(
            &cast,
            center,
            horiz_vel / speed,
            travel,
            travel.max(support.rise * STEP_UP_ADVANCE_PER_YARD),
            support.rise,
        )
    } else {
        StepAttempt {
            contact: None,
            verdict: StepVerdict::NoFace,
        }
    };
    if let Some((point, n)) = attempt.contact {
        if benilla_assets::trace::enabled_for("step") {
            let feet_y = center.y - CAPSULE_HEIGHT * 0.5;
            benilla_assets::trace::line(
                "step",
                &format!(
                    "hit ({:8.2},{:7.2},{:8.2}) h={:+.2} n=({:+.2},{:+.2},{:+.2}) {}",
                    point.x,
                    point.y,
                    point.z,
                    point.y - feet_y,
                    n.x,
                    n.y,
                    n.z,
                    attempt.verdict
                ),
            );
        }
    }
    // The regime (`0x631be0`, `0x635c00`): the reference's solid is a cone below
    // [`FOOT_CONE_HEIGHT`] and a box above it, so a certified obstacle whose blocking edge sits
    // inside the cone is ridden up the 61.6° skirt, and one above it pops. The certification is
    // the gate, since a wall's capsule contact also sits in the band; and the edge is measured,
    // not the certified rise, which grows with the advance.
    let ride_to = match (attempt.verdict, attempt.contact) {
        (StepVerdict::Commit { landed, .. }, Some((p, _)))
            if p.y - (center.y - CAPSULE_HEIGHT * 0.5) <= FOOT_CONE_HEIGHT =>
        {
            Some(landed.y)
        }
        _ => None,
    };
    // The pop is a rise in place, never a lunge: the reference's certify distance is only a sweep
    // length (`ebx` at `0x636193`, fed to `0x632ba0` alone), its walk resolver writes position
    // only relatively (`0x6367b0`-`0x637140`), and the certified arm rises `(0, 0, H)`
    // (`0x635d5b`) before `0x63694d` resumes the heading. The snap then reaches past the rise, as
    // the reference keeps `0x4000000` set until its settle has sized the probe (`0x636edf`).
    let popped = match attempt.verdict {
        StepVerdict::Commit { up, .. } if ride_to.is_none() => Some(up),
        _ => None,
    };
    let start = center + Vec3::Y * popped.unwrap_or(0.0);
    let mut rode = false;
    let out = world.slide_mover(
        capsule,
        start,
        horiz_vel,
        dt,
        &MoveAndSlideConfig::default(),
        |hit| {
            if let Some(ride) = walkable_ride_velocity(**hit.normal, *hit.velocity) {
                *hit.velocity = ride;
                return MoveAndSlideHitResponse::Accept;
            }
            // The skirt, on a certified low edge only, up to the certification's landing height;
            // the snap takes back any overshoot.
            if ride_to.is_some_and(|ceiling| hit.position.y < ceiling) {
                if let Some(up) = foot_cone_ride(**hit.normal, *hit.velocity) {
                    *hit.velocity = up;
                    rode = true;
                    return MoveAndSlideHitResponse::Accept;
                }
            }
            if let Some(followed) = steep_contact_shear(**hit.normal, *hit.velocity) {
                *hit.velocity = followed;
            }
            MoveAndSlideHitResponse::Accept
        },
    );
    let mut slid = out.position;
    // The step-down snap, the reference's step-vs-fall election (`0x6367b0`): it reaches
    // `travel · slope + slack` below, plus `H` on a steep support (`0x636dfc`) or a pop, plus the
    // hover offset (`0x633e35`), and settles only onto a walkable floor. A deeper drop is a fall,
    // and a body at rest reaches only the slack, so it falls instead of being pulled down.
    let d = slid - start;
    let reach = d.x.hypot(d.z) * STEP_SLOPE_RATIO
        + STEP_SNAP_SLACK
        + if support.steep || popped.is_some() {
            support.rise
        } else {
            0.0
        }
        + surface_offset;
    let hit = cast(slid, Vec3::NEG_Y * reach);
    // The water is one more candidate within the same reach, flat, nearest wins, as in the
    // reference's one masked trace; with no entity it keeps the caller's support.
    let water = support.water.and_then(|s| {
        let d = slid.y - CAPSULE_HEIGHT * 0.5 - s;
        (d <= reach).then_some(d.max(0.0))
    });
    let floor = match (hit, water) {
        (Some(h), Some(w)) if w < h.distance => Some((w, 1.0, None)),
        (Some(h), _) => Some((h.distance, h.normal1.y, Some(h.entity))),
        (None, Some(w)) => Some((w, 1.0, None)),
        (None, None) => None,
    };
    let snap = Some((reach, floor.map(|(d, n, _)| (d, n))));
    let mut ground = None;
    let mut steep_support = false;
    let mut unsupported = None;
    // A ride's height is earned. The reference's cone rests on the edge it climbs, but our capsule
    // hangs clear of a vertical riser and the snap would find the floor it is leaving, so mid-ride
    // only a floor above the ride's start may end it.
    let ride_rise = (slid.y - start.y).max(0.0);
    // The descent is capped at a cone's worth a frame, which the reference's finalize is not: it
    // descends the whole sweep before classifying (`0x636e45`-`0x636e52`), but its foot cone keeps
    // the skirt on the edge, so each sweep is short and the descent comes out at the cone's slope.
    // Our capsule hangs clear and would spend the whole drop at once; the cap ports the cone's
    // effect. A steep landing stays grounded with the steep bit set, as the reference's multipass
    // returns (`0x636f21`), and the deep reach keeps the ground in sight.
    let cone_reach = d.x.hypot(d.z) * STEP_SLOPE_RATIO + STEP_SNAP_SLACK;
    if let Some((distance, normal_y, entity)) = floor {
        let drop = (distance - surface_offset).max(0.0);
        let walkable = normal_y >= GROUND_COS;
        if !(rode && walkable && drop >= ride_rise) {
            slid.y -= drop.min(cone_reach);
            if drop > cone_reach {
                // Further down than a cone's worth but in sight: a step-down in progress, so stay
                // grounded with the deep reach open and finish it over the next frames.
                steep_support = true;
            } else if walkable {
                ground = entity;
            } else {
                // Support is earned by descending, not by touching: within reach alone would perch
                // a motionless body on a steep bank.
                steep_support = drop > STEP_SNAP_SLACK;
            }
        }
    } else if !rode {
        // Nothing in sight: the reference still descends the whole sweep, then elects a fall.
        // Measured here under the same cap, and spent only by a caller with a fall to elect; a
        // ride is exempt, since mid-ride the body hangs clear by design.
        unsupported = Some((reach - surface_offset).max(0.0).min(cone_reach));
    }
    GroundedStep {
        center: slid,
        climb: (rode || popped.is_some()).then_some(slid.y - center.y),
        // A ride or a pop that found no walkable floor is held up by the edge it climbs.
        steep_support: ((rode || popped.is_some()) && ground.is_none()) || steep_support,
        ground,
        snap,
        unsupported,
    }
}

/// One frame of the fall, exactly: the end-of-step vertical speed and the mean speed to move at,
/// whose `mean · dt` is the closed form `v₀·dt − ½g·dt²`. The reference evaluates its arc in closed
/// form from the launch every substep (`+0x7c − D(+0x78)`, `0x7c5e70`), so it cannot drift with
/// the frame rate, and nor can this. A step reaching terminal partway is split at `t_c`, since a
/// clamped mean would under-move it.
pub(crate) fn fall_step(v0: f32, dt: f32, terminal: f32) -> (f32, f32) {
    let unclamped = v0 - GRAVITY * dt;
    if unclamped >= -terminal {
        // Wholly inside the clamp: the mean of the endpoints, exact.
        return (unclamped, 0.5 * (v0 + unclamped));
    }
    if v0 <= -terminal {
        // At terminal for the whole step: constant velocity.
        return (-terminal, -terminal);
    }
    // The clamp engages partway: accelerate for `t_c`, then coast the remainder.
    let t_c = (v0 + terminal) / GRAVITY;
    let displacement = 0.5 * (v0 - terminal) * t_c - terminal * (dt - t_c);
    (-terminal, displacement / dt)
}

/// One airborne step against the world: the arc's slide alone, with steep faces sheared
/// ([`steep_contact_shear`]); gravity owns the height and the next ground probe the landing.
/// Shared with the remote dead-reckon's arcs ([`crate::net::motion::remote`]).
pub(crate) fn airborne_step(
    world: &benilla_world::collision::WorldCollision<'_, '_>,
    capsule: &Collider,
    center: Vec3,
    velocity: Vec3,
    dt: std::time::Duration,
) -> Vec3 {
    world
        .slide_mover(
            capsule,
            center,
            velocity,
            dt,
            &MoveAndSlideConfig::default(),
            |hit| {
                if let Some(followed) = steep_contact_shear(**hit.normal, *hit.velocity) {
                    *hit.velocity = followed;
                }
                MoveAndSlideHitResponse::Accept
            },
        )
        .position
}

/// The overhang line, just below zero: an axis-aligned wall's normal has `y = ±0` by rounding, and
/// both signs must read as a wall.
const OVERHANG_EPS: f32 = -1.0e-4;

/// A face the mover meets side-on: too steep to stand on, not a ceiling.
fn is_steep_face(ny: f32) -> bool {
    (OVERHANG_EPS..GROUND_COS).contains(&ny)
}

/// The ride up a walkable slope: keep the horizontal velocity and set the vertical so the motion
/// lies in the plane. The reference's walk step is a horizontal distance (`0x6367b0`), so no
/// walkable slope slows or bends it, where a plane clip would cut it to `h·cos²θ`.
fn walkable_ride_velocity(n: Vec3, v: Vec3) -> Option<Vec3> {
    if n.y < GROUND_COS || v.dot(n) >= 0.0 {
        return None;
    }
    // `n.y ≥ cos 50°` keeps the division safe; a prior facet's vertical is replaced, not stacked.
    Some(Vec3::new(v.x, -(v.x * n.x + v.z * n.z) / n.y, v.z))
}

/// The foot cone's ride: the reference's solid is a box over a cone whose bevels rise
/// `radius · 1.8494` from the foot (`0x631440`), so a low edge meets the skirt and the slide runs
/// the body up it. The lift is `0x635c00`'s `1.8494 · cosθ · len`, the closing speed times the
/// slope, and it sets the vertical rather than adding to it. Whether the body may climb at all is
/// the caller's certification, since a tall wall's contact sits in the cone band too.
fn foot_cone_ride(n: Vec3, v: Vec3) -> Option<Vec3> {
    if !is_steep_face(n.y) {
        return None;
    }
    // Steepness bounds the horizontal part below by sin 50°, so the normalize is safe.
    let h = Vec3::new(n.x, 0.0, n.z).normalize();
    let into = -(v.x * h.x + v.z * h.z);
    if into <= 0.0 {
        return None;
    }
    Some(Vec3::new(v.x, into * STEP_SLOPE_RATIO, v.z))
}

/// The steep-contact response: a horizontal push-out, the descent untouched. The reference's
/// `0x635090` writes only an x/y push-out along the normal's horizontal part (the vertical is
/// zeroed at `0x635166`), which `0x635600` adds to `Δx`/`Δy` alone, so
/// `v' = v − (v·n)/|n_h|² · n_h`. No contact can add lift, and a fall along a steep face keeps its
/// full rate, sliding down it faster than an orthogonal clip would. [`is_steep_face`] bounds
/// `|n_h|` below by `sin 50°`, so the reference's degeneracy guard is not needed.
fn steep_contact_shear(n: Vec3, v: Vec3) -> Option<Vec3> {
    if !is_steep_face(n.y) {
        return None;
    }
    let vn = v.dot(n);
    // A contact the motion is leaving is not this response's business.
    if vn >= 0.0 {
        return None;
    }
    let h = Vec3::new(n.x, 0.0, n.z);
    Some(v - (vn / h.length_squared()) * h)
}

/// What one step-up attempt decided; [`super::step_probe`] reads it back off each rung it tries.
#[derive(Clone, Copy, Debug)]
pub(crate) enum StepVerdict {
    /// Nothing steep and opposing within the look-ahead: not a step-up frame.
    NoFace,
    /// The rise found no headroom above the capsule.
    NoHeadroom,
    /// The settle probe found no floor at all under the advanced point.
    NoFloor { up: f32, fwd: f32 },
    /// The settle found a floor, but one too steep to stand on (`ny` under [`GROUND_COS`]).
    SteepFloor {
        up: f32,
        fwd: f32,
        dist: f32,
        ny: f32,
    },
    /// The maneuver gained no height (a graze, a too-tall wall, a pinch's gap floor): the plain
    /// slide keeps the frame.
    NetZero {
        up: f32,
        fwd: f32,
        dist: f32,
        ny: f32,
        dy: f32,
    },
    /// Certified: the obstacle can be cleared, by a ride or a pop of `up` ([`grounded_step`]).
    /// `landed` is the settle's floor a full advance downrange, for the trace only.
    Commit {
        landed: Vec3,
        up: f32,
        fwd: f32,
        dy: f32,
    },
}

impl std::fmt::Display for StepVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            StepVerdict::NoFace => write!(f, "no opposing face"),
            StepVerdict::NoHeadroom => write!(f, "up=0.00 NO-HEADROOM -> slide"),
            StepVerdict::NoFloor { up, fwd } => {
                write!(f, "up={up:.2} fwd={fwd:.2} down=miss NO-FLOOR -> slide")
            }
            StepVerdict::SteepFloor { up, fwd, dist, ny } => write!(
                f,
                "up={up:.2} fwd={fwd:.2} down=(d={dist:.2} ny={ny:+.2}) STEEP-FLOOR -> slide"
            ),
            StepVerdict::NetZero {
                up,
                fwd,
                dist,
                ny,
                dy,
            } => write!(
                f,
                "up={up:.2} fwd={fwd:.2} down=(d={dist:.2} ny={ny:+.2}) dy={dy:+.3} NET-ZERO -> slide"
            ),
            StepVerdict::Commit { up, fwd, dy, .. } => {
                write!(f, "up={up:.2} fwd={fwd:.2} dy={dy:+.3} -> COMMIT")
            }
        }
    }
}

/// One step-up attempt: the face that triggered it and the verdict.
pub(crate) struct StepAttempt {
    /// The face's contact point and authored normal; `None` when there was nothing to step.
    pub(crate) contact: Option<(Vec3, Vec3)>,
    pub(crate) verdict: StepVerdict,
}

/// The step-up certification, a standard kinematic-controller probe rather than the reference
/// resolver's: against a steep opposing face within `look`, rise by the free headroom (at most
/// `rise`), advance by `advance` along the input, and settle by the snap's reach onto a walkable
/// floor that is higher. A graze or a wall taller than `rise` lands no higher, and a pinch between
/// trunks only on steep faces, so none commits and the plain slide keeps the frame.
/// [`super::step_probe`] sweeps `advance` to find where a step would clear.
pub(crate) fn step_up(
    cast: &impl Fn(Vec3, Vec3) -> Option<MoveHitData>,
    center: Vec3,
    dir_h: Vec3,
    look: f32,
    advance: f32,
    rise: f32,
) -> StepAttempt {
    let none = |verdict| StepAttempt {
        contact: None,
        verdict,
    };
    // A steep, non-overhanging face opposing the motion within `look`. The reference has no
    // incidence gate; a graze nets zero in the settle.
    let Some(ahead) = cast(center, dir_h * look) else {
        return none(StepVerdict::NoFace);
    };
    let n = ahead.normal1;
    if n.y >= GROUND_COS || n.y < OVERHANG_EPS || n.dot(dir_h) >= 0.0 {
        return none(StepVerdict::NoFace);
    }
    let at = |verdict| StepAttempt {
        contact: Some((ahead.point1, n)),
        verdict,
    };

    // Rise: the free headroom, at most `rise`.
    let up = cast(center, Vec3::Y * rise).map_or(rise, |h| h.distance);
    if up < 1e-3 {
        return at(StepVerdict::NoHeadroom);
    }
    // Advance: along the input dir, swept at the raised height.
    let raised = center + Vec3::Y * up;
    let fwd = cast(raised, dir_h * advance).map_or(advance, |h| h.distance);
    let over = raised + dir_h * fwd;
    // Settle: the rise undone plus the snap's reach, onto a walkable floor only.
    let reach = up + advance * STEP_SLOPE_RATIO + STEP_SNAP_SLACK;
    let Some(down) = cast(over, Vec3::NEG_Y * reach) else {
        return at(StepVerdict::NoFloor { up, fwd });
    };
    let (dist, ny) = (down.distance, down.normal1.y);
    if ny < GROUND_COS {
        return at(StepVerdict::SteepFloor { up, fwd, dist, ny });
    }
    let landed = over + Vec3::NEG_Y * dist;
    let dy = landed.y - center.y;
    // Commit only a landing that gained height: a net-zero one belongs to the slide, whose
    // deflection is what sliding along a fence is.
    if dy <= 0.05 {
        return at(StepVerdict::NetZero {
            up,
            fwd,
            dist,
            ny,
            dy,
        });
    }
    at(StepVerdict::Commit {
        landed,
        up,
        fwd,
        dy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::state::STEP_UP_ADVANCE;
    use crate::player::AIR_NUDGE_SPEED;
    use crate::player::CAPSULE_RADIUS;
    use bevy::ecs::system::RunSystemOnce;

    /// A Stormwind Trade District kerb: a 0.28 yd sidewalk behind a ~61° bevel, profiled from a
    /// `stup` scan at the spot.
    fn world_with_kerb() -> App {
        const PROFILE: [(f32, f32); 4] = [(-2.0, 0.0), (0.29, 0.0), (0.446, 0.28), (3.0, 0.28)];
        world_from_profile(&PROFILE)
    }

    /// A headless world from a `(x, y)` polyline extruded across `z`, wound so every face's
    /// authored normal points up and back at the approaching body: faces are one-sided.
    fn world_from_profile(profile: &[(f32, f32)]) -> App {
        const W: f32 = 3.0;
        let mut app = App::new();
        // `WorldCollision` needs the mover's trace exclusions, which the world plugins insert.
        app.init_resource::<benilla_world::collision::MoverTraceExclusions>();
        // avian's collider backend reads `Assets<Mesh>` and `SceneSpawner` even without meshes.
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>();
        let (mut verts, mut tris) = (Vec::new(), Vec::new());
        for w in profile.windows(2) {
            let (&(x0, y0), &(x1, y1)) = (&w[0], &w[1]);
            let b = verts.len() as u32;
            verts.extend([
                Vec3::new(x0, y0, -W),
                Vec3::new(x1, y1, -W),
                Vec3::new(x1, y1, W),
                Vec3::new(x0, y0, W),
            ]);
            // (a, c, b) / (a, d, c): normal = (-dy, dx, 0), up for the flats and up-and-back for
            // the riser; the reverse winding would block nothing.
            tris.extend([[b, b + 2, b + 1], [b, b + 3, b + 2]]);
        }
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::trimesh(verts, tris),
            Transform::default(),
        ));
        app.update(); // one frame builds Position/Rotation and the spatial-query trees
        app
    }

    fn player_capsule() -> Collider {
        Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT - 2.0 * CAPSULE_RADIUS)
    }

    /// Walk into the kerb until it stops the body, then run one step-up attempt at `advance`.
    fn step_at(advance: f32) -> StepVerdict {
        world_with_kerb()
            .world_mut()
            .run_system_once(move |world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let cast =
                    |from: Vec3, disp: Vec3| world.cast_mover(&capsule, from, disp, SKIN_WIDTH);
                let start = Vec3::new(-1.0, CAPSULE_HEIGHT * 0.5, 0.0);
                let run = cast(start, Vec3::X).map_or(1.0, |h| h.distance);
                let center = start + Vec3::X * run;
                step_up(
                    &cast,
                    center,
                    Vec3::X,
                    TRAVEL_60FPS,
                    advance,
                    STEP_UP_HEIGHT,
                )
                .verdict
            })
            .unwrap()
    }

    /// One frame's travel at a run (7.0 yd/s) and 60 fps.
    const TRAVEL_60FPS: f32 = 7.0 / 60.0;

    /// `walk_profile` on the kerb.
    fn walk_kerb(start_y: f32, frames: usize) -> Vec<Row> {
        walk_profile(world_with_kerb(), start_y, frames)
    }

    /// One frame of a walked fixture: `(feet height, climb, steep support, walkable ground)`.
    type Row = (f32, Option<f32>, bool, bool);

    /// Walk into `world` from `(-1, start_y)` along +X, then run `frames` whole grounded steps.
    fn walk_profile(world: App, start_y: f32, frames: usize) -> Vec<Row> {
        walk_from(world, Vec3::new(-1.0, start_y, 0.0), Vec3::X, frames)
    }

    /// Feet at `start`, walk in `dir` until blocked (a yard at most), then `frames` grounded steps.
    fn walk_from(mut world: App, start: Vec3, dir: Vec3, frames: usize) -> Vec<Row> {
        world
            .world_mut()
            .run_system_once(move |world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let cast =
                    |from: Vec3, disp: Vec3| world.cast_mover(&capsule, from, disp, SKIN_WIDTH);
                let start = start + Vec3::Y * (CAPSULE_HEIGHT * 0.5);
                let run = cast(start, dir).map_or(1.0, |h| h.distance);
                let mut center = start + dir * run;
                let dt = std::time::Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
                // The steep bit carries between frames, as in the mover, gating the deep reach.
                let mut support = Support::default();
                (0..frames)
                    .map(|_| {
                        let g = grounded_step(&world, &capsule, center, dir * 7.0, dt, support);
                        center = g.center;
                        support.steep = g.steep_support;
                        (
                            center.y - CAPSULE_HEIGHT * 0.5,
                            g.climb,
                            g.steep_support,
                            g.ground.is_some(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap()
    }

    /// Booty Bay's 0.40 yd vertical riser onto a ~22° ramp, at WoW `(-14314.3, 466.2, 18.5)`,
    /// leaning 0.002 yd so the sign of a `±0` normal cannot decide the test.
    fn world_with_vertical_step() -> App {
        const PROFILE: [(f32, f32); 4] = [(-2.0, 0.0), (0.30, 0.0), (0.302, 0.40), (3.0, 1.48)];
        world_from_profile(&PROFILE)
    }

    #[test]
    fn a_vertical_step_is_climbed_not_stalled() {
        // The 0.40 yd rise rides, and a capsule on a vertical riser hangs clear of the street, so
        // the snap must not pull the ride back down.
        let frames = walk_profile(world_with_vertical_step(), 0.0, 6);
        let top = frames.last().unwrap().0;
        assert!(
            top > 0.40,
            "the body must finish on the step, not stalled below it: {frames:?}"
        );
        for w in frames.windows(2) {
            assert!(
                w[1].0 >= w[0].0 - 1.0e-3,
                "the climb must never go backwards: {frames:?}"
            );
        }
        assert!(
            frames[0].0 < 0.40,
            "…and it is still a ride, not a one-frame pop: {frames:?}"
        );
    }

    #[test]
    fn a_tall_step_behind_an_unwalkable_face_is_climbed() {
        // A Stormwind step at WoW `(-8427.4, 607.4, 95.1)`: a 0.91 yd top behind a 66° bevel most
        // of a yard deep, so the settle must reach past the bevel to find it.
        const P: [(f32, f32); 4] = [(-2.0, -0.02), (0.28, -0.02), (0.694, 0.91), (3.0, 0.91)];
        let frames = walk_profile(world_from_profile(&P), -0.02, 8);
        assert!(
            frames.last().unwrap().0 > 0.88,
            "the body must finish on the 0.91 yd top: {frames:?}"
        );
        for w in frames.windows(2) {
            assert!(
                w[1].0 >= w[0].0 - 1.0e-3,
                "the climb must never go backwards: {frames:?}"
            );
        }
        // The blocking edge is low on the bevel, inside the cone, so this rides rather than pops.
        assert!(
            frames[0].0 < 0.3,
            "a 0.91 yd gain in one frame is the teleport the ride avoids: {frames:?}"
        );
    }

    #[test]
    fn stepping_off_a_kerb_follows_the_surface_down() {
        // Walking off the kerb the probe finds only its ~61° riser. Every frame must stay
        // supported, since the caller turns an unsupported frame into a fall.
        let frames = walk_from(world_with_kerb(), Vec3::new(1.6, 0.28, 0.0), Vec3::NEG_X, 8);
        for (i, f) in frames.iter().enumerate() {
            assert!(
                f.2 || f.3,
                "frame {i} left the surface entirely — that is the dive: {frames:?}"
            );
        }
        assert!(
            frames.last().unwrap().0 < 0.05,
            "the walk-off must reach the street: {frames:?}"
        );
    }

    #[test]
    fn a_face_steeper_than_the_cone_still_descends_what_it_can() {
        // A 66° face (`ny ≈ 0.41`) walked off at a run: past the lip the mover descends what it
        // can follow, rather than holding its height until it falls.
        const P: [(f32, f32); 4] = [(-2.0, 0.0), (0.60, 0.0), (1.0, -0.91), (3.0, -0.91)];
        // Nothing blocks the approach, which then moves a full yard; the start allows for it.
        let frames = walk_from(
            world_from_profile(&P),
            Vec3::new(-0.6, 0.0, 0.0),
            Vec3::X,
            5,
        );
        let cone = TRAVEL_60FPS * STEP_SLOPE_RATIO;
        let biggest = frames
            .windows(2)
            .map(|w| w[0].0 - w[1].0)
            .fold(frames[0].0.abs(), f32::max);
        assert!(
            biggest > cone * 0.5,
            "some frame past the lip must follow the face down rather than hold height: \
             {frames:?} (best drop {biggest:.3}, a cone's worth is {cone:.3})"
        );
    }

    #[test]
    fn a_ledge_deeper_than_the_probe_is_a_fall_not_an_absorbed_step() {
        // A 1.2 yd sheer drop, deeper than the snap reaches: the body leaves the ground before the
        // bottom, as the reference's does, instead of absorbing the drop in one grounded frame.
        const P: [(f32, f32); 4] = [(-2.0, 0.0), (0.60, 0.0), (0.62, -1.20), (3.0, -1.20)];
        let frames = walk_from(
            world_from_profile(&P),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::X,
            12,
        );
        // A frame's descent is the slide's plus the snap's, each at most a cone's reach.
        let bound = 2.0 * (TRAVEL_60FPS * STEP_SLOPE_RATIO + STEP_SNAP_SLACK);
        let worst = frames
            .windows(2)
            .map(|w| w[0].0 - w[1].0)
            .fold(0.0_f32, f32::max);
        assert!(
            worst <= bound,
            "no frame may drop more than {bound:.3} yd; worst was {worst:.3}\n{frames:#?}"
        );
    }

    #[test]
    fn an_idle_body_is_not_pulled_down_through_open_air() {
        // Standing still the snap reaches only the slack, so a body a yard up keeps its height.
        const P: [(f32, f32); 2] = [(-2.0, 0.0), (3.0, 0.0)];
        let (dy, ground) = world_from_profile(&P)
            .world_mut()
            .run_system_once(move |world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let dt = std::time::Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
                let center = Vec3::new(0.0, CAPSULE_HEIGHT * 0.5 + 1.0, 0.0);
                let g = grounded_step(&world, &capsule, center, Vec3::ZERO, dt, Support::default());
                (g.center.y - center.y, g.ground.is_some())
            })
            .unwrap();
        assert!(!ground, "a body a yard up is standing on nothing");
        assert!(
            dy.abs() <= STEP_SNAP_SLACK * 2.0,
            "it must keep its height and fall, not be snapped a yard down: dy={dy:+.3}"
        );
    }

    #[test]
    fn a_step_up_never_outruns_the_frame_it_happens_in() {
        // The reference's walk resolver holds `L / t_remaining` across every hit (`0x6367b0`), so a
        // pop never lunges. A Goldshire table: a slab from 0.62 to 0.737 over legs recessed past a
        // body radius, so the contact sits above the cone and the step pops rather than rides.
        const P: [(f32, f32); 6] = [
            (-2.0, 0.0),
            (0.95, 0.0),
            (0.95, 0.62),
            (0.60, 0.62),
            (0.60, 0.737),
            (3.0, 0.737),
        ];
        let track = world_from_profile(&P)
            .world_mut()
            .run_system_once(move |world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let dt = std::time::Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
                let mut center = Vec3::new(-1.0, CAPSULE_HEIGHT * 0.5, 0.0);
                let mut support = Support::default();
                let mut rows = Vec::new();
                for _ in 0..24 {
                    let g = grounded_step(&world, &capsule, center, Vec3::X * 7.0, dt, support);
                    rows.push((g.center.x - center.x, g.center.y - CAPSULE_HEIGHT * 0.5));
                    center = g.center;
                    support.steep = g.steep_support;
                }
                rows
            })
            .unwrap();
        let worst = track.iter().map(|r| r.0).fold(0.0_f32, f32::max);
        assert!(
            worst <= TRAVEL_60FPS + 1.0e-3,
            "no frame may travel further than the {TRAVEL_60FPS:.3} yd it asked to walk; worst \
             was {worst:.3}\n{track:#?}"
        );
        let top = track.last().expect("frames").1;
        assert!(
            top > 0.70,
            "and it still has to get onto the 0.737 yd table: ended at {top:.3}\n{track:#?}"
        );
    }

    #[test]
    fn stepping_off_a_fence_hugs_the_edge_down_instead_of_dropping_at_once() {
        // A 0.72 yd Goldshire fence with a ~53° chamfered lip. No frame may descend more than a
        // cone's worth, and none may lose support: a cap alone would fall one frame later.
        const P: [(f32, f32); 5] = [
            (-2.0, 0.72),
            (0.60, 0.72),
            (0.69, 0.60),
            (0.692, 0.0),
            (3.0, 0.0),
        ];
        let frames = walk_from(
            world_from_profile(&P),
            Vec3::new(-1.0, 0.72, 0.0),
            Vec3::X,
            14,
        );
        let cone = TRAVEL_60FPS * STEP_SLOPE_RATIO + STEP_SNAP_SLACK;
        let worst = frames
            .windows(2)
            .map(|w| w[0].0 - w[1].0)
            .fold(0.0_f32, f32::max);
        assert!(
            worst <= cone + 1.0e-3,
            "the step-down must take at most a cone's worth ({cone:.3}) a frame; worst was \
             {worst:.3}\n{frames:#?}"
        );
        let air = frames
            .iter()
            .position(|&(_, _, steep, walk)| !steep && !walk);
        assert!(
            air.is_none(),
            "and it must stay on the surface the whole way down, not fall the rest: first \
             unsupported frame {air:?}\n{frames:#?}"
        );
        let bottom = frames.last().expect("frames").0;
        assert!(
            bottom < 0.05,
            "and it does have to get all the way down: ended at {bottom:.3}\n{frames:#?}"
        );
    }

    #[test]
    fn a_frame_that_finds_nothing_still_spends_only_one_cone() {
        // The no-hit leg with the steep bit set and the floor beyond even the deep reach, which a
        // profiled fence cannot reach: its riser stays under the capsule.
        const P: [(f32, f32); 2] = [(-2.0, 0.0), (3.0, 0.0)];
        let dy = world_from_profile(&P)
            .world_mut()
            .run_system_once(move |world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let dt = std::time::Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
                let center = Vec3::new(0.0, CAPSULE_HEIGHT * 0.5 + 1.6, 0.0);
                let g = grounded_step(
                    &world,
                    &capsule,
                    center,
                    Vec3::X * 7.0,
                    dt,
                    Support {
                        steep: true,
                        ..Support::default()
                    },
                );
                g.center.y - center.y
            })
            .unwrap();
        let cone = TRAVEL_60FPS * STEP_SLOPE_RATIO + STEP_SNAP_SLACK;
        assert!(
            -dy <= cone + 1.0e-3,
            "a frame that found nothing may still spend only a cone's worth ({cone:.3}); it spent \
             {:.3}",
            -dy
        );
    }

    #[test]
    fn a_motionless_body_is_never_perched_on_a_steep_slope() {
        // A 60° bank, where a standing body's cone bound is the slack alone.
        const P: [(f32, f32); 3] = [(-2.0, 0.0), (0.0, 0.0), (3.0, -5.196)];
        let out = world_from_profile(&P)
            .world_mut()
            .run_system_once(move |world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let dt = std::time::Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
                let mut center = Vec3::new(0.6, CAPSULE_HEIGHT * 0.5 - 1.039, 0.0);
                let mut rows = Vec::new();
                let mut support = Support::default();
                for i in 0..4 {
                    let v = if i < 2 { Vec3::X * 7.0 } else { Vec3::ZERO };
                    let g = grounded_step(&world, &capsule, center, v, dt, support);
                    center = g.center;
                    support.steep = g.steep_support;
                    rows.push((v.length() > 0.0, g.steep_support));
                }
                rows
            })
            .unwrap();
        for (moving, supported) in out {
            assert!(
                moving || !supported,
                "a motionless body must never hold a steep face — it slides off"
            );
        }
    }

    #[test]
    fn an_exactly_vertical_riser_still_certifies() {
        // An axis-aligned riser's normal has `y = ±0` by rounding; either sign must climb.
        const PROFILE: [(f32, f32); 4] = [(-2.0, 0.0), (0.30, 0.0), (0.30, 0.40), (3.0, 1.48)];
        let frames = walk_profile(world_from_profile(&PROFILE), 0.0, 6);
        assert!(
            frames.last().unwrap().0 > 0.40,
            "an exactly-vertical riser must climb exactly like a tilted one: {frames:?}"
        );
    }

    #[test]
    fn the_kerb_is_ridden_up_its_skirt_never_popped() {
        // A 0.28 yd kerb is inside the foot cone: it must reach the tread, and not in one frame.
        let frames = walk_kerb(0.0, 4);
        let arrived = frames
            .iter()
            .position(|&(y, _, _, _)| (y - 0.28).abs() < 0.03)
            .expect("the ride should put the feet on the 0.28 yd tread");
        assert!(
            arrived > 0,
            "arriving on the very first frame is the pop, not a ride: {frames:?}"
        );
        assert!(
            frames[0].2,
            "the first frame is mid-ride and must report itself grounded: {frames:?}"
        );
        assert!(
            frames[0].0 > 0.0 && frames[0].0 < 0.28,
            "the first frame should be part-way up the skirt, got {:+.3}",
            frames[0].0
        );
    }

    #[test]
    fn a_wall_is_never_ridden() {
        // The kerb read from 2 yd below is a 2.3 yd wall, whose contact also sits in the cone band.
        for (y, climb, riding, _) in walk_kerb(-2.0, 4) {
            assert!(!riding, "a wall must never start a cone ride");
            assert!(climb.is_none(), "a wall must never register a climb");
            assert!(
                y < -2.0 + 0.05,
                "the body must not rise against a wall, feet at {y:+.3}"
            );
        }
    }

    #[test]
    fn the_cone_ride_gains_the_reference_slope() {
        // `0x635c00`'s `T = 1.8494 · cosθ · len`: head-on, the slope times the speed…
        let head_on = foot_cone_ride(Vec3::new(-1.0, 0.0, 0.0), Vec3::X * 7.0).unwrap();
        assert!(
            (head_on.y - 7.0 * STEP_SLOPE_RATIO).abs() < 1e-4,
            "{head_on:?}"
        );
        assert_eq!(
            head_on.x, 7.0,
            "horizontal speed is never touched by a ride"
        );
        // …and at 60° off the face, exactly cos 60° of it.
        let oblique = foot_cone_ride(
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(7.0 * 0.5, 0.0, 7.0 * 0.866_025_4),
        )
        .unwrap();
        assert!(
            (oblique.y - 7.0 * 0.5 * STEP_SLOPE_RATIO).abs() < 1e-3,
            "{oblique:?}"
        );
    }

    #[test]
    fn the_cone_ride_declines_what_is_not_its_business() {
        let v = Vec3::X * 7.0;
        // A walkable face already had its ride ([`walkable_ride_velocity`])…
        assert!(foot_cone_ride(Vec3::new(-0.5, 0.866, 0.0).normalize(), v).is_none());
        // …an overhang is a ceiling, not a skirt…
        assert!(foot_cone_ride(Vec3::new(-0.5, -0.866, 0.0).normalize(), v).is_none());
        // …and a face we are moving away from is not in the way at all.
        assert!(foot_cone_ride(Vec3::new(1.0, 0.0, 0.0), v).is_none());
    }

    #[test]
    fn the_kerb_is_out_of_reach_of_one_frames_travel() {
        // At one frame's travel the settle lands on the bevel, which the walkable gate refuses.
        let v = step_at(TRAVEL_60FPS);
        assert!(
            matches!(v, StepVerdict::SteepFloor { .. }),
            "one frame's travel should still be over the bevel, got {v}"
        );
    }

    #[test]
    fn a_body_scaled_advance_climbs_the_kerb() {
        // …and at the certify advance it is over the tread, with `dy` the kerb's height.
        let v = step_at(STEP_UP_ADVANCE);
        let StepVerdict::Commit { dy, .. } = v else {
            panic!("a body-radius advance should reach the tread, got {v}");
        };
        assert!(
            (dy - 0.28).abs() < 0.03,
            "should land on the 0.28 yd tread, gained {dy:+.3}"
        );
    }

    #[test]
    fn the_advance_never_climbs_past_the_rise_ceiling() {
        // The kerb read from 2 yd below is a wall taller than the rise budget: it clips the raised
        // sweep, so the settle lands back on the origin floor.
        let v = world_with_kerb()
            .world_mut()
            .run_system_once(move |world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let cast =
                    |from: Vec3, disp: Vec3| world.cast_mover(&capsule, from, disp, SKIN_WIDTH);
                let start = Vec3::new(-1.0, CAPSULE_HEIGHT * 0.5 - 2.0, 0.0);
                let run = cast(start, Vec3::X).map_or(1.0, |h| h.distance);
                step_up(
                    &cast,
                    start + Vec3::X * run,
                    Vec3::X,
                    TRAVEL_60FPS,
                    STEP_UP_ADVANCE,
                    STEP_UP_HEIGHT,
                )
                .verdict
            })
            .unwrap();
        assert!(
            !matches!(v, StepVerdict::Commit { .. }),
            "a face above the rise ceiling must never certify, got {v}"
        );
    }

    /// Outward normal of a face rising toward +x, tilted `deg` from horizontal.
    fn face(deg: f32) -> Vec3 {
        let r = deg.to_radians();
        Vec3::new(-r.sin(), r.cos(), 0.0)
    }

    #[test]
    fn a_walkable_ramp_rides_at_full_horizontal_speed() {
        // A plane clip would cut 45° uphill to `h·cos²45°` = 3.5.
        let n = face(45.0);
        let v = Vec3::new(7.0, 0.0, 0.0);
        let ride = walkable_ride_velocity(n, v).expect("must ride");
        assert_eq!((ride.x, ride.z), (7.0, 0.0));
        assert!(ride.y > 0.0);
        assert!(ride.dot(n).abs() < 1e-6);
    }

    #[test]
    fn a_diagonal_approach_is_not_deflected() {
        let v = Vec3::new(5.0, 0.0, 5.0);
        let ride = walkable_ride_velocity(face(40.0), v).expect("must ride");
        assert_eq!((ride.x, ride.z), (5.0, 5.0));
    }

    #[test]
    fn a_prior_facet_ride_is_recomputed_not_stacked() {
        // The incoming 3.0 is a prior facet's ride vertical.
        let n = face(45.0);
        let ride = walkable_ride_velocity(n, Vec3::new(7.0, 3.0, 0.0)).expect("must ride");
        assert_eq!((ride.x, ride.z), (7.0, 0.0));
        assert!(ride.dot(n).abs() < 1e-6);
    }

    #[test]
    fn steep_flat_and_receding_planes_never_ride() {
        let push = Vec3::new(7.0, 0.0, 0.0);
        assert!(walkable_ride_velocity(face(60.0), push).is_none());
        assert!(walkable_ride_velocity(Vec3::Y, push).is_none());
        assert!(walkable_ride_velocity(face(40.0), -push).is_none());
    }

    #[test]
    fn the_ride_covers_the_walkable_range_up_to_the_gate() {
        let v = Vec3::new(7.0, 0.0, 0.0);
        let ride = walkable_ride_velocity(face(49.9), v).expect("must ride");
        assert_eq!((ride.x, ride.z), (7.0, 0.0));
        assert!(ride.y <= 7.0 * 50.0_f32.to_radians().tan() + 1e-3);
        assert!(walkable_ride_velocity(face(50.1), v).is_none());
        assert!(steep_contact_shear(face(50.1), v).is_some());
    }

    #[test]
    fn walking_into_a_steep_face_spends_the_push_along_it() {
        let v = Vec3::new(7.0, 0.0, 0.0);
        let out = steep_contact_shear(face(60.0), v).expect("must strip");
        assert!(out.x.abs() < 1e-6 && out.y == 0.0);
    }

    #[test]
    fn a_fall_against_a_face_keeps_the_whole_of_its_descent() {
        // Falling at 4.9 yd/s into a 55° face with a run held into it, an orthogonal clip keeps ~1%
        // of the descent, which the wedge rest would read as a landing.
        let (n, v) = (face(55.0), Vec3::new(7.0, -4.9, 0.0));
        let orthogonal = v - v.dot(n) * n;
        assert!(
            orthogonal.y > -0.2,
            "the orthogonal clip must be the near-cancel it was: {:.3}",
            orthogonal.y
        );
        let out = steep_contact_shear(n, v).expect("a steep contact responds");
        assert!(
            (out.y - v.y).abs() < 1.0e-4,
            "the descent passes through untouched: {:.3} vs {:.3}",
            out.y,
            v.y
        );
        // The horizontal is `cot θ` per unit of drop, downhill; the push into the face cancels.
        let cot = 1.0 / 55.0_f32.to_radians().tan();
        assert!(
            (out.x - v.y * cot).abs() < 1.0e-3,
            "downhill drift must be cot55° = {cot:.3} per unit of drop: {:.3}",
            out.x
        );
        // The residual lies in the plane, so nothing further is clipped off it.
        assert!(out.dot(n).abs() < 1.0e-4, "Δ'·A = 0: {:.5}", out.dot(n));
    }

    #[test]
    fn a_plumb_fall_is_carried_down_the_face_at_full_speed() {
        // A plumb fall has no push to remove, yet following the surface is horizontal motion.
        let out = steep_contact_shear(face(60.0), Vec3::new(0.0, -10.0, 0.0))
            .expect("a plumb fall still follows the surface");
        assert!(
            (out.y + 10.0).abs() < 1.0e-4,
            "full free-fall rate: {:.3}",
            out.y
        );
        let cot = 1.0 / 60.0_f32.to_radians().tan();
        assert!(
            (out.x + 10.0 * cot).abs() < 1.0e-3,
            "downhill: {:.3}",
            out.x
        );
        // A contact the motion is leaving is not this response's: no push-out, no drift.
        assert!(steep_contact_shear(face(60.0), Vec3::new(7.0, 20.0, 0.0)).is_none());
    }

    #[test]
    fn a_rising_jump_keeps_its_own_lift() {
        let v = Vec3::new(7.0, 8.0, 0.0);
        let out = steep_contact_shear(face(60.0), v).expect("boost must strip");
        assert!((out.y - v.y).abs() < 1e-6);
        let n = face(60.0);
        // …and the true plane no longer opposes what is left, so nothing further is clipped.
        assert!(out.dot(n) >= -1e-6);
    }

    /// The three gates of `0x6315f0`; the pitch gate is strict, so −37.0° exactly sinks.
    #[test]
    fn the_water_is_geometry_only_while_all_three_gates_hold() {
        const SURFACE: f32 = 12.5;
        let eps = 1.0e-4;
        assert_eq!(water_floor(true, false, 0.0, Some(SURFACE)), Some(SURFACE));
        assert_eq!(
            water_floor(true, false, WATER_WALK_PITCH_FLOOR + eps, Some(SURFACE)),
            Some(SURFACE),
            "just shallower than -37 deg still walks on water"
        );
        assert_eq!(
            water_floor(true, false, WATER_WALK_PITCH_FLOOR, Some(SURFACE)),
            None,
            "-37.0 deg exactly is EXCLUDED — `0x63162c`'s jne reads ZF"
        );
        assert_eq!(
            water_floor(true, false, WATER_WALK_PITCH_FLOOR - eps, Some(SURFACE)),
            None,
            "aim past -37 deg and the liquid layers leave the mask: the dive"
        );
        // The arm has no upper bound.
        assert_eq!(
            water_floor(true, false, 1.5, Some(SURFACE)),
            Some(SURFACE),
            "the arm gates the nose-down half only"
        );
        // Swimming excludes it (`0x631617`), and so does not having the aura.
        assert_eq!(water_floor(true, true, 0.0, Some(SURFACE)), None);
        assert_eq!(water_floor(false, false, 0.0, Some(SURFACE)), None);
        assert_eq!(water_floor(true, false, 0.0, None), None);
    }

    /// Levitate's hover and water walking compose: the finalize probe's trace (`0x632d29`) is
    /// headed by the mask arm (`0x6320a0`), so it measures to the water and the body rests a yard
    /// over it, reached at the climb rate (`0x636fa1`) from below and stopped there from above.
    #[test]
    fn a_hovering_water_walker_floats_a_yard_over_the_waterline() {
        const SURFACE: f32 = 0.0;
        // Open water: nothing but the liquid plane can hold the body up.
        const OPEN: [(f32, f32); 2] = [(-3.0, -40.0), (3.0, -40.0)];
        let (first, rest, dropped) = world_from_profile(&OPEN)
            .world_mut()
            .run_system_once(|world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let mut time = Time::default();
                time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                let levitate = super::super::state::MoveModes {
                    water_walking: true,
                    hover: true,
                    ..Default::default()
                };
                let idle = |player: &mut Player, frames: usize| {
                    for _ in 0..frames {
                        step(
                            player,
                            &time,
                            &world,
                            &capsule,
                            false,
                            Vec3::ZERO,
                            7.0,
                            false,
                            false,
                            None,
                            Some(SURFACE),
                            AIR_NUDGE_SPEED,
                        );
                    }
                    player.pos.y
                };
                let mut player = Player {
                    modes: levitate,
                    pos: Vec3::new(0.0, SURFACE, 0.0),
                    ..Default::default()
                };
                let first = idle(&mut player, 1);
                let rest = idle(&mut player, 60);
                let mut faller = Player {
                    modes: levitate,
                    pos: Vec3::new(0.0, SURFACE + 8.0, 0.0),
                    ..Default::default()
                };
                let dropped = idle(&mut faller, 240);
                (first, rest, dropped)
            })
            .unwrap();

        assert!(
            first > SURFACE && first - SURFACE < HOVER_HEIGHT * 0.5,
            "the rise off the waterline is rate-limited, not a pop: {first} after one frame"
        );
        assert!(
            (rest - (SURFACE + HOVER_HEIGHT)).abs() < 1.0e-3,
            "a hovering water-walker rests exactly {HOVER_HEIGHT} yd over the surface: {rest}"
        );
        assert!(
            (dropped - (SURFACE + HOVER_HEIGHT)).abs() < 1.0e-3,
            "a hovering water-walker dropped in from above stops on the hover line: {dropped}"
        );
    }

    /// A wader in liquid too shallow to swim is lifted onto the surface, which the reference's
    /// swept surface keeps a body on top of.
    #[test]
    fn water_walking_lifts_a_wader_out_of_the_water() {
        const SURFACE: f32 = 0.0;
        // A lakebed 1.4 yd under the surface: wading depth for a human (swim starts at ~1.52).
        const BED: [(f32, f32); 2] = [(-4.0, -1.4), (4.0, -1.4)];
        let (walker, levitator) = world_from_profile(&BED)
            .world_mut()
            .run_system_once(|world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let mut time = Time::default();
                time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                let run = |modes: super::super::state::MoveModes, frames: usize| {
                    let mut player = Player {
                        modes,
                        pos: Vec3::new(0.0, -1.4, 0.0),
                        ..Default::default()
                    };
                    for _ in 0..frames {
                        step(
                            &mut player,
                            &time,
                            &world,
                            &capsule,
                            false,
                            Vec3::ZERO,
                            7.0,
                            false,
                            false,
                            None,
                            Some(SURFACE),
                            AIR_NUDGE_SPEED,
                        );
                    }
                    player.pos.y
                };
                let walker = run(
                    super::super::state::MoveModes {
                        water_walking: true,
                        ..Default::default()
                    },
                    30,
                );
                let levitator = run(
                    super::super::state::MoveModes {
                        water_walking: true,
                        hover: true,
                        ..Default::default()
                    },
                    60,
                );
                (walker, levitator)
            })
            .unwrap();
        assert!(
            (walker - SURFACE).abs() < 1.0e-3,
            "water walking must lift a wader onto the surface: {walker}"
        );
        assert!(
            (levitator - (SURFACE + HOVER_HEIGHT)).abs() < 1.0e-3,
            "…and Levitate's hover carries it the last yard: {levitator}"
        );
    }

    /// A water-walker walking off land keeps steering and stops when the keys go: over water the
    /// classify must see the plane, or every frame reads airborne and coasts on frozen momentum.
    #[test]
    fn a_water_walker_steers_and_stops_instead_of_coasting() {
        const SURFACE: f32 = 0.0;
        const SPEED: f32 = 7.0;
        // Land out to x = 0, flush with the water; open water beyond it, with no collider at all.
        const QUAY: [(f32, f32); 2] = [(-3.0, SURFACE), (0.0, SURFACE)];
        let (east, north, idle) = world_from_profile(&QUAY)
            .world_mut()
            .run_system_once(|world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let mut time = Time::default();
                time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                let mut player = Player {
                    modes: super::super::state::MoveModes {
                        water_walking: true,
                        ..Default::default()
                    },
                    pos: Vec3::new(-1.0, SURFACE, 0.0),
                    ..Default::default()
                };
                let drive = |player: &mut Player, dir: Vec3, frames: usize| {
                    for _ in 0..frames {
                        step(
                            player,
                            &time,
                            &world,
                            &capsule,
                            dir != Vec3::ZERO,
                            dir,
                            SPEED,
                            false,
                            false,
                            None,
                            Some(SURFACE),
                            AIR_NUDGE_SPEED,
                        );
                    }
                    player.pos
                };
                let east = drive(&mut player, Vec3::X, 60);
                let north = drive(&mut player, Vec3::Z, 30);
                let idle = drive(&mut player, Vec3::ZERO, 30);
                (east, north, idle)
            })
            .unwrap();

        assert!(
            east.x > 1.0,
            "the walk onto the water must actually travel: {east:?}"
        );
        for (label, p) in [("east", east), ("north", north), ("idle", idle)] {
            assert!(
                (p.y - SURFACE).abs() < 1.0e-3,
                "the feet must stay on the liquid surface ({label}): {p:?}"
            );
        }
        assert!(
            north.z - east.z > 1.0,
            "a water-walker must steer: z {} -> {}",
            east.z,
            north.z
        );
        assert!(
            (north.x - east.x).abs() < 0.1,
            "the abandoned direction must stop advancing: x {} -> {} (the frozen air-nudge drift)",
            east.x,
            north.x
        );
        assert!(
            idle.distance(north) < 1.0e-3,
            "released keys must stop a water-walker dead: {north:?} -> {idle:?}"
        );
    }

    #[test]
    fn a_hovering_mover_refuses_the_jump_and_leaves_no_trace() {
        // Hover is the first test in `CMovement::Jump` (`0x7c6230`, at `0x7c623a`); the press with
        // hover off is the control.
        let flat: [(f32, f32); 2] = [(-3.0, 0.0), (3.0, 0.0)];
        let rows = world_from_profile(&flat)
            .world_mut()
            .run_system_once(|world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let mut time = Time::default();
                time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                [true, false].map(|hover| {
                    let mut player = Player {
                        modes: super::super::state::MoveModes {
                            hover,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let out = step(
                        &mut player,
                        &time,
                        &world,
                        &capsule,
                        false,
                        Vec3::X,
                        0.0,
                        true,
                        false,
                        None,
                        None,
                        AIR_NUDGE_SPEED,
                    );
                    (out.jumped, player.launch_vz)
                })
            })
            .unwrap();
        let [(hover_jumped, hover_vy), (plain_jumped, plain_vy)] = rows;
        assert!(
            !hover_jumped && hover_vy == 0.0,
            "hover must refuse the jump silently: jumped {hover_jumped}, vel_y {hover_vy}"
        );
        assert!(
            plain_jumped && plain_vy > 0.0,
            "the hover-free control must take off: jumped {plain_jumped}, vel_y {plain_vy}"
        );
    }

    /// `Jump(0)` skips the hover test (`0x7c6236`), and its one caller is the hover grant
    /// (`0x61a62e`), so it launches the very body the keyboard cannot.
    #[test]
    fn the_wire_jump_launches_the_hovering_body_the_keyboard_cannot() {
        let flat: [(f32, f32); 2] = [(-3.0, 0.0), (3.0, 0.0)];
        let (keyboard, wire) = world_from_profile(&flat)
            .world_mut()
            .run_system_once(|world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let mut time = Time::default();
                time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                let press = |want_jump: bool, wire_jump: bool| {
                    let mut player = Player {
                        modes: super::super::state::MoveModes {
                            hover: true,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let out = step(
                        &mut player,
                        &time,
                        &world,
                        &capsule,
                        false,
                        Vec3::X,
                        0.0,
                        want_jump,
                        wire_jump,
                        None,
                        None,
                        AIR_NUDGE_SPEED,
                    );
                    (out.jumped, player.launch_vz)
                };
                (press(true, false), press(false, true))
            })
            .unwrap();
        assert_eq!(
            keyboard,
            (false, 0.0),
            "`0x7c623a` still refuses Space while hovering"
        );
        assert_eq!(
            wire,
            (true, JUMP_SPEED),
            "`force = 0` skips that test and takes off on the land seed `0xc0fe93d8` (the launch \
             speed, not the post-gravity `vel_y`)"
        );
    }

    /// One step of `dt` and `n` steps of `dt/n` land on the same height, as the reference's closed
    /// form does.
    #[test]
    fn the_fall_step_is_frame_rate_independent() {
        let total = 0.5_f32;
        let drop = |steps: u32| {
            let dt = total / steps as f32;
            let (mut v, mut y) = (JUMP_SPEED, 0.0_f32);
            for _ in 0..steps {
                let (end, mean) = fall_step(v, dt, TERMINAL_VELOCITY);
                y += mean * dt;
                v = end;
            }
            y
        };
        let analytic = JUMP_SPEED * total - 0.5 * GRAVITY * total * total;
        for steps in [1_u32, 2, 8, 30, 120] {
            let got = drop(steps);
            assert!(
                (got - analytic).abs() < 1.0e-4,
                "{steps} steps must reproduce the closed form {analytic}, got {got}"
            );
        }
    }

    /// The jump apex is `v₀²/2g` = 1.640 yd at any frame rate.
    #[test]
    fn the_jump_apex_is_the_analytic_one_at_any_frame_rate() {
        let apex = JUMP_SPEED * JUMP_SPEED / (2.0 * GRAVITY);
        for fps in [30.0_f32, 60.0, 144.0] {
            let dt = 1.0 / fps;
            let (mut v, mut y, mut high) = (JUMP_SPEED, 0.0_f32, 0.0_f32);
            while v > -1.0 {
                let (end, mean) = fall_step(v, dt, TERMINAL_VELOCITY);
                y += mean * dt;
                high = high.max(y);
                v = end;
            }
            // The apex falls between frames: a step's worth of sampling, not drift.
            assert!(
                (high - apex).abs() < 0.5 * GRAVITY * dt * dt + 1.0e-4,
                "at {fps} fps the apex should be ~{apex}, got {high}"
            );
        }
    }

    /// A step reaching terminal partway is two regimes; below terminal the clamp changes nothing.
    #[test]
    fn the_terminal_clamp_is_exact_on_the_step_that_reaches_it() {
        let dt = 1.0 / 60.0;
        let (end, mean) = fall_step(0.0, dt, TERMINAL_VELOCITY);
        assert!((end - -GRAVITY * dt).abs() < 1.0e-6);
        assert!((mean - -0.5 * GRAVITY * dt).abs() < 1.0e-6);
        // Straddling terminal: reach it at `t_c`, coast the rest.
        let v0 = -TERMINAL_VELOCITY + 0.5 * GRAVITY * dt;
        let (end, mean) = fall_step(v0, dt, TERMINAL_VELOCITY);
        assert_eq!(end, -TERMINAL_VELOCITY, "the step ends at terminal");
        let t_c = (v0 + TERMINAL_VELOCITY) / GRAVITY;
        let want = (0.5 * (v0 - TERMINAL_VELOCITY) * t_c - TERMINAL_VELOCITY * (dt - t_c)) / dt;
        assert!(
            (mean - want).abs() < 1.0e-6,
            "piecewise, not a clamped mean"
        );
        let (end, mean) = fall_step(-TERMINAL_VELOCITY, dt, TERMINAL_VELOCITY);
        assert_eq!((end, mean), (-TERMINAL_VELOCITY, -TERMINAL_VELOCITY));
    }

    /// Every knockback apply plants FORWARD (`0x6179c0`, at `0x617a18`) whatever its speed, and air
    /// control opens only while the nibble is clear (`0x7c6afc`), so a vertical toss never steers.
    #[test]
    fn a_vertical_knockback_is_no_more_steerable_than_a_fast_one() {
        let flat: [(f32, f32); 2] = [(-3.0, 0.0), (3.0, 0.0)];
        const STRAIGHT_UP: Vec3 = Vec3::new(0.0, 12.0, 0.0);
        let horiz = world_from_profile(&flat)
            .world_mut()
            .run_system_once(|world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let mut time = Time::default();
                time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                let mut player = Player::default();
                // Frame 1: the launch.
                let out = step(
                    &mut player,
                    &time,
                    &world,
                    &capsule,
                    false,
                    Vec3::ZERO,
                    0.0,
                    false,
                    false,
                    Some(STRAIGHT_UP),
                    None,
                    AIR_NUDGE_SPEED,
                );
                assert!(out.knocked && out.jumped, "the toss opened an arc");
                assert_eq!(player.horiz_vel, Vec3::ZERO, "and it is purely vertical");
                // Frames 2..6: airborne, with a movement key held.
                let mut nudged = false;
                for _ in 0..5 {
                    let out = step(
                        &mut player,
                        &time,
                        &world,
                        &capsule,
                        true,
                        Vec3::NEG_X,
                        7.0,
                        false,
                        false,
                        None,
                        None,
                        AIR_NUDGE_SPEED,
                    );
                    nudged |= out.air_nudged;
                }
                (player.horiz_vel, nudged)
            })
            .unwrap();
        assert_eq!(
            horiz,
            (Vec3::ZERO, false),
            "the planted nibble locks the arc: no nudge fires and the body does not drift, however \
             hard the key is held"
        );
    }

    /// The nudge is the live `min(walk, run)`: `0x7c5c20(1)` calls `0x7c4c90(1)`, whose walk
    /// override returns `min(+0x88, +0x8c)`.
    #[test]
    fn the_standstill_nudge_takes_the_speed_it_is_given() {
        let flat: [(f32, f32); 2] = [(-3.0, 0.0), (3.0, 0.0)];
        let steer = |nudge: f32| {
            world_from_profile(&flat)
                .world_mut()
                .run_system_once(move |world: benilla_world::collision::WorldCollision| {
                    let capsule = player_capsule();
                    let mut time = Time::default();
                    time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                    let mut player = Player::default();
                    // A standing jump: the nibble is clear, so one steer is in hand.
                    step(
                        &mut player,
                        &time,
                        &world,
                        &capsule,
                        false,
                        Vec3::ZERO,
                        0.0,
                        true,
                        false,
                        None,
                        None,
                        nudge,
                    );
                    let out = step(
                        &mut player,
                        &time,
                        &world,
                        &capsule,
                        true,
                        Vec3::NEG_X,
                        7.0,
                        false,
                        false,
                        None,
                        None,
                        nudge,
                    );
                    // A second press finds the nibble set and changes nothing.
                    step(
                        &mut player,
                        &time,
                        &world,
                        &capsule,
                        true,
                        Vec3::NEG_Z,
                        7.0,
                        false,
                        false,
                        None,
                        None,
                        nudge,
                    );
                    (out.air_nudged, player.horiz_vel)
                })
                .unwrap()
        };
        let (fired, v) = steer(2.5);
        assert!(fired, "a standstill jump has one steer in hand");
        assert!(
            (v.length() - 2.5).abs() < 1.0e-4,
            "at the speed given: {v:?}"
        );
        assert!(v.x < 0.0, "and in the pressed direction: {v:?}");
        // A walk aura lowers the live speed.
        let (fired, v) = steer(1.0);
        assert!(fired);
        assert!(
            (v.length() - 1.0).abs() < 1.0e-4,
            "a slowed body steers slower — the constant is not the law: {v:?}"
        );
        assert!(
            v.z.abs() < 1.0e-4,
            "the second press must not steer again: {v:?}"
        );
    }

    /// The launch seeds both axes from the server, not the keys, and only ROOT refuses it: the
    /// reference's drain tests nothing else before applying one, and discards it under root with
    /// no apply and no ack (`0x615c71`).
    #[test]
    fn a_knockback_launch_overrides_the_keys_and_only_a_root_refuses_it() {
        let flat: [(f32, f32); 2] = [(-3.0, 0.0), (3.0, 0.0)];
        // Due east at 25 yd/s with 12 yd/s of lift, while the player holds "run west" at 7 yd/s.
        const PUSH: Vec3 = Vec3::new(25.0, 12.0, 0.0);
        let (free, rooted) = world_from_profile(&flat)
            .world_mut()
            .run_system_once(|world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let mut time = Time::default();
                time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                let knock = |rooted: bool| {
                    let mut player = Player {
                        modes: super::super::state::MoveModes {
                            rooted,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let out = step(
                        &mut player,
                        &time,
                        &world,
                        &capsule,
                        true,        // moving …
                        Vec3::NEG_X, // … the other way, at a run
                        7.0,
                        false,
                        false,
                        Some(PUSH),
                        None,
                        AIR_NUDGE_SPEED,
                    );
                    (out.jumped, out.knocked, player.launch_vz, player.horiz_vel)
                };
                (knock(false), knock(true))
            })
            .unwrap();

        let (jumped, knocked, vel_y, horiz) = free;
        assert!(
            jumped && knocked,
            "the launch opens an arc, and says which kind it was: {free:?}"
        );
        assert_eq!(
            vel_y, PUSH.y,
            "the vertical is the server's take-off speed, not JUMP_SPEED ({JUMP_SPEED}) — read off \
             `launch_vz`, because `vel_y` has already taken this frame's gravity step"
        );
        assert_eq!(
            horiz,
            Vec3::new(PUSH.x, 0.0, PUSH.z),
            "the horizontal is the server's too — the held key must not claw it back"
        );

        assert_eq!(
            rooted,
            (false, false, 0.0, Vec3::ZERO),
            "a rooted body is anchored: no launch, and `knocked` false so no ack goes out either"
        );
    }

    /// The wire's `zspeed` is down-positive: a rising knockback's is negative (`0x7c61f0` stores it
    /// in `CMovement+0xa0`, where a jump seeds `0xc0fe93d8` = −7.955547), and the ack echoes it.
    #[test]
    fn a_rising_knockback_carries_negative_wire_zspeed_and_positive_lift() {
        let launch = benilla_protocol::JumpInfo {
            zspeed: -12.0, // down-positive: negative is upward
            cos_angle: 1.0,
            sin_angle: 0.0,
            xy_speed: 25.0,
        };
        // The controller's resolve, verbatim (`player::control`).
        let bevy_launch = benilla_assets::coords::wow_to_bevy([
            launch.cos_angle * launch.xy_speed,
            launch.sin_angle * launch.xy_speed,
            -launch.zspeed,
        ]);
        assert_eq!(bevy_launch.y, 12.0, "a negative wire zspeed lifts the body");

        // Back out as `stream_self_movement` builds the tail, negating the +up `jump_zspeed`.
        let jump_zspeed = bevy_launch.y;
        assert_eq!(
            -jump_zspeed, launch.zspeed,
            "the echo must be bit-identical — the server matches it within 0.01 before relaying"
        );
        let back = benilla_assets::coords::bevy_to_wow(bevy_launch.with_y(0.0));
        assert_eq!(
            [back[0], back[1]],
            [
                launch.cos_angle * launch.xy_speed,
                launch.sin_angle * launch.xy_speed
            ],
            "world XY is absolute and survives WoW → Bevy → WoW"
        );
    }

    /// The waterline push-out stands in for the reference's swept surface, so like a sweep it
    /// catches a sinking body and lets a rising one through.
    #[test]
    fn the_waterline_catches_a_sinking_body_and_lets_a_launched_one_through() {
        const SURFACE: f32 = 0.0;
        const OPEN: [(f32, f32); 2] = [(-3.0, -40.0), (3.0, -40.0)];
        const UNDER: f32 = SURFACE - 0.5;
        let (launched, at_rest) = world_from_profile(&OPEN)
            .world_mut()
            .run_system_once(|world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let mut time = Time::default();
                time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                let one_step = |vel_y: f32| {
                    let mut player = Player {
                        modes: super::super::state::MoveModes {
                            water_walking: true,
                            ..Default::default()
                        },
                        pos: Vec3::new(0.0, UNDER, 0.0),
                        vel_y,
                        ..Default::default()
                    };
                    step(
                        &mut player,
                        &time,
                        &world,
                        &capsule,
                        false,
                        Vec3::ZERO,
                        0.0,
                        false,
                        false,
                        None,
                        Some(SURFACE),
                        AIR_NUDGE_SPEED,
                    );
                    (player.pos.y, player.vel_y)
                };
                // The seed a `SetHover(true)` picks while SWIMMING (`0x7c6261` → `0xc1118c48`).
                (one_step(9.096_748), one_step(0.0))
            })
            .unwrap();

        let (up_y, up_vy) = launched;
        assert!(
            up_y > UNDER && up_y < SURFACE,
            "the launch keeps rising under its own seed, un-snapped: {up_y}"
        );
        assert!(
            up_vy > 0.0,
            "and keeps its velocity — a clamp to the surface would zero it: {up_vy}"
        );

        let (rest_y, rest_vy) = at_rest;
        assert!(
            (rest_y - SURFACE).abs() < 1.0e-3,
            "a body at rest under the surface is still lifted onto it: {rest_y}"
        );
        assert_eq!(rest_vy, 0.0, "and the floor takes its velocity");
    }

    /// A hovering water-walker holds the hover line walking and standing: the snap must see the
    /// water plane, or it takes the no-floor leg every frame and the climb fights it.
    #[test]
    fn a_levitating_walker_holds_its_line_over_water_moving_or_still() {
        const SURFACE: f32 = 0.0;
        const OPEN: [(f32, f32); 2] = [(-400.0, -40.0), (400.0, -40.0)];
        let (first_rise, settled, walk_low, walk_end, stopped) = world_from_profile(&OPEN)
            .world_mut()
            .run_system_once(|world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let mut time = Time::default();
                time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                let mut player = Player {
                    modes: super::super::state::MoveModes {
                        water_walking: true,
                        hover: true,
                        ..Default::default()
                    },
                    pos: Vec3::new(0.0, SURFACE, 0.0),
                    ..Default::default()
                };
                let run = |player: &mut Player, moving: bool, n: usize| {
                    let mut low = f32::INFINITY;
                    for _ in 0..n {
                        step(
                            player,
                            &time,
                            &world,
                            &capsule,
                            moving,
                            if moving { Vec3::X } else { Vec3::ZERO },
                            7.0,
                            false,
                            false,
                            None,
                            Some(SURFACE),
                            AIR_NUDGE_SPEED,
                        );
                        low = low.min(player.pos.y);
                    }
                    low
                };
                run(&mut player, false, 2);
                let first_rise = player.pos.y;
                run(&mut player, false, 88);
                let settled = player.pos.y;
                let walk_low = run(&mut player, true, 90);
                let walk_end = player.pos.y;
                run(&mut player, false, 60);
                (first_rise, settled, walk_low, walk_end, player.pos.y)
            })
            .unwrap();

        let line = SURFACE + HOVER_HEIGHT;
        assert!(
            (first_rise - 2.0 * HOVER_CLIMB_RATE / 60.0).abs() < 1.0e-5,
            "two frames of climb are exactly two frames of the rate, with nothing pulling back \
             against them: {first_rise}"
        );
        assert!(
            (settled - line).abs() < 1.0e-4,
            "an idle Levitator rests on the hover line: {settled}"
        );
        assert!(
            (walk_low - line).abs() < 1.0e-4,
            "and does not sag a millimetre while walking — lowest was {walk_low}, want {line}"
        );
        assert!(
            (walk_end - line).abs() < 1.0e-4,
            "still on the line after 90 walking frames: {walk_end}"
        );
        assert!(
            (stopped - line).abs() < 1.0e-4,
            "and stopping is not a rise, because nothing was lost: {stopped}"
        );
    }

    /// An Elwynn hillside out of flat ground, a 55° face (`ny = +0.574`, read at WoW
    /// `(-9236.8, -341.9, 101.6)`), too steep to stand on anywhere.
    fn world_with_steep_hillside() -> App {
        const P: [(f32, f32); 3] = [(-3.0, 0.0), (0.0, 0.0), (2.5, 3.570)];
        world_from_profile(&P)
    }

    /// One airborne frame's readings: `(feet height, vertical velocity, fraction of the descent
    /// gravity intended that the frame actually achieved)`.
    type AirRow = (f32, f32, f32);

    /// Jump from `start_feet`, flying the bare arc through [`airborne_step`] at a run along `dir`.
    fn jump_into(mut world: App, start_feet: Vec3, dir: Vec3, frames: usize) -> Vec<AirRow> {
        world
            .world_mut()
            .run_system_once(move |world: benilla_world::collision::WorldCollision| {
                let capsule = player_capsule();
                let dt = std::time::Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
                let secs = dt.as_secs_f32();
                let mut center = start_feet + Vec3::Y * (CAPSULE_HEIGHT * 0.5);
                let mut vel_y = JUMP_SPEED;
                (0..frames)
                    .map(|_| {
                        vel_y = (vel_y - GRAVITY * secs).max(-TERMINAL_VELOCITY);
                        let before = center.y;
                        center = airborne_step(
                            &world,
                            &capsule,
                            center,
                            dir * 7.0 + Vec3::Y * vel_y,
                            dt,
                        );
                        let intent = vel_y * secs;
                        let achieved = center.y - before;
                        // Only a descent has a stall to measure; a rise reports 1.0.
                        let frac = if intent < 0.0 { achieved / intent } else { 1.0 };
                        (center.y - CAPSULE_HEIGHT * 0.5, vel_y, frac)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap()
    }

    #[test]
    fn a_jump_into_a_steep_hillside_banks_no_height() {
        let arc = jump_into(
            world_with_steep_hillside(),
            Vec3::new(-0.6, 0.0, 0.0),
            Vec3::X,
            150,
        );
        let peak = arc.iter().map(|r| r.0).fold(f32::MIN, f32::max);
        let end = arc.last().unwrap().0;
        assert!(
            peak > 1.0,
            "the jump must actually leave the ground: {peak:.3}"
        );
        assert!(
            end < 0.05,
            "the arc must end back at the foot of the slope, not banked part-way up it: \
             ended at {end:.3} (peak {peak:.3})"
        );
        // Nor may the descent stall into a wedge rest, which would stand the body up part-way.
        let (mut run, mut worst_run) = (0u8, 0u8);
        for r in arc.iter().filter(|r| r.0 > 0.2) {
            run = if r.1 < -WEDGE_MIN_FALL && r.2 < WEDGE_STALL_RATIO {
                run + 1
            } else {
                0
            };
            worst_run = worst_run.max(run);
        }
        assert!(
            worst_run < WEDGE_STILL_FRAMES,
            "the descent must never stall long enough to read as a wedge rest: \
             {worst_run} consecutive stalled frames"
        );
    }

    #[test]
    fn a_steep_face_never_cancels_a_fall() {
        // A plane clip turns a run into the hill into lift (`v'.y - v.y = -(v·n)·n.y`), which all
        // but cancels a fall against a 55° face.
        let arc = jump_into(
            world_with_steep_hillside(),
            Vec3::new(-0.6, 0.0, 0.0),
            Vec3::X,
            150,
        );
        // Only frames clear of the flat ground, which the bare arc has no ground probe to stand on.
        let worst = arc
            .iter()
            .filter(|r| r.1 < -WEDGE_MIN_FALL && r.0 > 0.2)
            .map(|r| r.2)
            .fold(f32::MAX, f32::min);
        assert!(
            worst > 0.98,
            "a falling frame against the face keeps ALL of its descent — the reference's response \
             is horizontal only: worst was {worst:.2} of intent"
        );
    }

    #[test]
    fn walkable_and_overhanging_faces_are_untouched() {
        let push = Vec3::new(7.0, 0.0, 0.0);
        assert!(steep_contact_shear(face(40.0), push).is_none());
        // An overhang keeps the plain ceiling clip.
        assert!(steep_contact_shear(Vec3::new(-0.5, -0.7, 0.0).normalize(), push).is_none());
        assert!(steep_contact_shear(face(60.0), -push).is_none());
    }

    #[test]
    fn a_vertical_wall_takes_the_whole_push_and_no_more() {
        let v = Vec3::new(7.0, -4.0, 0.0);
        let out = steep_contact_shear(face(90.0), v).expect("must strip");
        assert!(out.x.abs() < 1e-6 && (out.y - v.y).abs() < 1e-6);
    }
}
