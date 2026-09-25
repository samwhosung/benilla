//! The mover's side of the `WOW_MOVE_TRACE` debug trace ([`benilla_assets::trace`]):
//!
//! - **`move`** — one line per *interesting* frame of the player mover ([`frame`]): a step-down snap,
//!   a grounded flip, an airborne frame, or any sizeable vertical delta — so a movement-feel report
//!   ("it pops when I step off the fence") can be read back as per-frame numbers instead of
//!   re-guessed from watching the screen.
//! - **`swim`** — one line per frame *over liquid* ([`swim`]): the waterline, the feet, the depth
//!   against the two latch thresholds, and which physics regime ran. A water-feel report ("it
//!   jitters at the surface") is a question about the depth signal frame by frame, and no other
//!   tag carries it: `move` is emitted by the walk arm only, so the swim frames of a flapping
//!   latch are exactly the ones it drops.
//! - **`mvr`** — one line per mover-claim packet ([`mover_claim`]): who we told the server we are
//!   driving, which is what decides whether anything under `snd` is accepted.
//! - **`sit`** — one line per **stand-state** decision ([`posture`]): every commit, and every press
//!   the client's own sit-down gate refused, each with the movement word that decided it. B155's
//!   reading instrument ("you can sit while swimming"): the refusal is silent in the reference and
//!   therefore silent in ours, so without this the *whole* mechanism is invisible from a live run —
//!   a granted sit and a refused one look identical on screen, and the granted-underwater bug and a
//!   gate that refuses everything are the same picture. Covers the `X` key and the `/sit` family
//!   alike, because both land on the one commit it is emitted from.
//! - **`gait`** — one line per **walk/run toggle** decision ([`gait`]): the `TOGGLERUN` press and
//!   what the reference's guard chain did with it. Same argument as `sit`, half applied: the
//!   *granted* half is observable twice over (the body's gait changes and `snd` logs the
//!   `SET_WALK_MODE`/`SET_RUN_MODE` that went out), but a **refused** press is silent by design —
//!   indistinguishable from a key that isn't bound, or from a binding whose dispatch never fired.
//!   The chain also has an open byte (`0x1200`'s second bit), so a live run is
//!   how any answer about it gets checked against a real refusal instead of re-reasoned.
//! - **`knb`** — one line per **knockback** ([`knockback`]): the launch quad the server aimed and
//!   whether the mover flew it. The one mover edge with no observable of its own — its ack is not a
//!   `MSG_MOVE_*` so `snd` misses it, its refusal is silent by design, and a wrong ack's whole
//!   symptom is *other players seeing nothing*.
//! - **`snd`** — one line per outbound `MSG_MOVE_*` ([`sent`]), the send-side twin of `net::motion`'s
//!   `rly`: it makes **our own wire cadence measurable**, which is the only way to
//!   compare it against the reference's — the 1.12.1 sniff's client stream is a list of exactly these
//!   fields, so `grep snd` on a run of mouse-turning and strafing is directly diffable against it.
//!
//! The anim driver writes its own `anim` lines into the same file, on the same clock, and the
//! transport carry writes `ride` from [`super::ride`] — beside the mechanism it measures, because
//! reading it needs the deck's transform and the attach walk, neither of which belongs here.

use std::sync::atomic::{AtomicBool, Ordering};

use benilla_assets::trace;

/// What the mover did this frame, filled at the end of the physics step in [`super`].
pub(super) struct Frame {
    /// Feet height entering the step (yd).
    pub y_in: f32,
    /// Feet height leaving the step (yd) — after the slide *and* the step-down snap.
    pub y_out: f32,
    /// Horizontal distance covered this frame (yd). Paired with the vertical it gives the frame's
    /// **descent slope**, which is the whole question behind "we dive over the edge instead of
    /// stepping down it": the reference bounds a step-down to the foot cone's own
    /// [`super::STEP_SLOPE_RATIO`] per unit of horizontal travel (`0x636dda`), so
    /// a walk-off that reads far steeper — or worse, flat-then-plummet across two frames — is the
    /// mover leaving the surface instead of following it down.
    pub dx: f32,
    pub grounded: bool,
    pub on_walkable: bool,
    /// Whether the keys asked for any horizontal motion this frame. A frame that travelled with
    /// **no** input is a body being moved by the resolve alone — a push-out, a slide off a
    /// contact — and that is the whole shape of "I sat down and ended up beside the chair",
    /// so it is interesting on its own even while grounded and level.
    pub moving: bool,
    pub vel_y: f32,
    /// The step-down snap, when the walk-mode block ran: `(probe reach, what the probe found)`;
    /// the inner pair is `(hit distance, hit normal.y)` — a steep hit is recorded too, so a lip
    /// contact that killed the snap shows up in the trace.
    pub snap: Option<(f32, Option<(f32, f32)>)>,
    /// The atomic step-up's committed height gain this frame (yd), when the maneuver ran.
    pub climb: Option<f32>,
    /// The root's anchor ran this frame — no gravity, no slide, no snap. Without it
    /// on the line, a rooted hang and a genuinely stuck mover are the same column of `dy=+0.000`.
    pub anchored: bool,
}

/// One `snd` line per outbound movement packet: the opcode we chose, the live move-flags it carries,
/// and the facing. Read against the sniff's client stream (opcode + flags + orientation per line) this
/// answers "does our wire look like the reference's?" without anyone squinting at a second window.
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

/// One `sit` line per **stand-state decision** — the commits and the refusals, with the movement
/// word that decided each.
///
/// `what` is `commit` or `REFUSED`; `state` is the `UnitStandStateType` asked for (0 STAND · 1 SIT ·
/// 2 SIT_CHAIR · 3 SLEEP · 8 KNEEL), `from` the state it was asked from, and `flags` the live
/// `CMovement` word the client's gate reads (`[[this+0x118]+0x40]`) — with its two decisive
/// sub-readings spelled out rather than left to be masked by eye, because "is `0x220000f` swimming?"
/// is exactly the question a trace exists to stop anyone answering from memory.
///
/// Emitted for **both** outcomes on purpose. The reference refuses a sit-down silently — no error
/// text, no sound — so a refusal has no observable of its own; and a *granted* sit is equally
/// unreadable, since the pose that follows is identical whether the gate ran or was never there.
/// One line per decision is what makes B155's fix falsifiable in a live run instead of assertable
/// from a screenshot of a seated character.
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

/// One `gait` line per **walk/run toggle** decision, granted or refused.
///
/// `what` is `commit` or `REFUSED`; `to` is the gait the press asked for, and the three flags are
/// the reference's own guard chain at `0x513d8e`–`0x513dcd` spelled out one by one — because
/// "why did nothing happen?" is exactly the question this tag exists to stop anyone answering from
/// memory, and three silent gates share the same silence.
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

/// One `knb` line per **knockback**, whether we flew it or refused it.
///
/// The tag exists because a knockback is the one mover edge with **no observable of its own on
/// either side of the failure**. Its ack is not a `MSG_MOVE_*`, so `snd` never shows it; its refusal
/// is silent by design (the reference discards the record under a root, and vmangos logs
/// `OnFailedToAckChange` server-side where we cannot see it); and if the ack's four floats are ever
/// wrong the server drops it as `OnWrongAckData` and simply never relays the knockback — a bug whose
/// entire symptom is *other people not seeing something*, which no local capture can show.
///
/// So the line carries the quad as it arrived, next to the verdict. `up` is the derived +Z take-off
/// speed, printed beside the wire's own down-positive `zspeed` precisely because the sign flip
/// between them is where a knockback that drives you into the ground would come from — the two must
/// always read as negatives of each other.
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

/// One `mvr` line per **mover-claim** packet — `CMSG_SET_ACTIVE_MOVER` and
/// `CMSG_MOVE_NOT_ACTIVE_MOVER`. Neither is a `MSG_MOVE_*`, so neither appears under `snd`, and yet
/// the pair decides whether every packet that *does* is accepted at all: vmangos matches each
/// against the session's confirmed mover and answers a mismatch with a server-side error log the
/// client never sees. A possession that goes wrong is unreadable from `snd` alone — the stream looks
/// perfect and lands nowhere (the gap cost a live probe round to notice).
pub(super) fn mover_claim(what: &str, guid: u64) {
    if !trace::enabled() {
        return;
    }
    trace::line("mvr", &format!("{what} guid={guid:#x}"));
}

/// One `swim` line per frame the avatar is over liquid: the waterline, the feet, the submersion
/// depth and the two latch thresholds it is being compared against, plus the regime that ran. The
/// `<` / `>` markers flag the frames where the depth crossed a threshold, so a latch that is
/// flapping reads as a column of them rather than something to be inferred from a Z column.
///
/// `h` is the avatar's own collision height, and the thresholds are printed **derived from it**
/// rather than from a constant — a trace that quoted a human's 1.52 while a gnome
/// latched at 0.86 would read as a bug in the latch instead of the height, which is precisely the
/// misdirection that let the constant survive this long. It is echoed on the line so a capture says
/// which body it was taken on.
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

/// One `sett` line per `CMSG_MOVE_TIME_SKIPPED` we send — how many milliseconds
/// of movement simulation the hold ran through without integrating, and for which mover. It rides
/// the settle's own tag because it reports the settle's own cost: a `skipped` far larger than the
/// `sett` line beside it means the hold outlived the stream it was waiting for.
pub(super) fn skipped_time(guid: u64, lag_ms: u32) {
    if !trace::enabled() {
        return;
    }
    trace::line(
        "sett",
        &format!("skipped {lag_ms:5} ms for mover {guid:#x}"),
    );
}

/// How the post-teleport settle hold **ended** — the whole diagnosis of a fall-through report.
///
/// `resident` means the destination's world arrived (scene spawned + collider queue quiet —
/// decision 0737's release) and the hold released onto it; `!resident` is the
/// [`super::SETTLE_TIMEOUT`] backstop firing, which since decision 1303 means the stream
/// made **no progress at all** for the whole budget — a genuinely dead stream, never a slow one —
/// and gravity comes on with the world never having become resident. `waited` is measured from
/// the snap either way. The distinction is invisible from inside the game (both look like "the
/// loading screen went away") and it is the difference between a world that arrived and one that
/// died, so the timeout end is also a `warn!` on the ordinary log — a reporter's paste can then
/// name it without owning a trace.
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

/// One `rid` line per frame of a **self-spline ride** — the Charge/knockback/fear/taxi instrument,
/// and the A/B that measured.
///
/// It exists because the ride is the one mover in the client the `move` trace cannot see: the
/// controller — and with it the mover's own per-frame line — is parked behind `control`'s ride
/// guard for the whole ride, so a charge across a hillside left no record at all.
///
/// The three numbers, and why all three:
/// - **`wire`** — the Z `sample_splines` put us at, i.e. the server's chord.
/// - **`ground`** — the walkable surface under that XZ. `miss` is not a failure: nothing in reach
///   is a genuinely airborne pose or ground that has not streamed in, and the ride keeps its own Z
///   there, the same answer the creature clamp gives.
/// - **`z`** — where the ride actually left us, and `gap` is `z − ground`: **the number that says
///   whether the ride is riding the world or the wire.**
///
/// `chord` is `wire − ground` — the error the wire carried, which stays visible after the fix so
/// the instrument still measures the defect it closed rather than reporting a flat zero. A charge
/// down a gentle Elwynn slope reads `chord` rising to ~0.5 yd at mid-path while `gap` holds at 0;
/// **before 1927 the two were the same number**, because `z` was `wire`.
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
    // Input-less travel: the resolve moved a body nobody was steering. A millimetre a frame is
    // the signature (1458's chair extrusion ran at 1–2 mm), so the floor is well under it.
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
    // The descent slope, printed only while actually going down and moving: `>cone` marks a frame
    // steeper than the foot cone allows, which is the signature of a dive rather than a step-down.
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
    // The dive's real signature, and it is not steepness — it is the *absence* of descent. A body
    // that leaves the surface travels forward with `dy≈0` while the probe reports a steep face right
    // beneath it; everything after that is a ballistic arc, and by the time the fall looks steep the
    // frame that caused it is long gone. `>cone` was the first guess and the director's capture
    // showed it never fires: the arc starts at slope 0.05 and only reaches 0.83. This is the marker
    // that would have named it on the first read.
    let left = f
        .snap
        .and_then(|(_, hit)| hit)
        // **A face the body is touching is not a face it left.** The steep hit must be at a
        // *distance* — `d = 0.000` is the body resting flush against a bank, sliding along it, which
        // is the ordinary rub the mover is built to do and which reads identically on every other
        // term here (`dy = 0`, forward travel, a steep normal below). Eighteen of those in one
        // hillside capture buried the one real signature this marker exists to name.
        .is_some_and(|(dist, ny)| ny < super::GROUND_COS && dist > 1.0e-3)
        // **Flat is the signature — not "anything that isn't descending".** A dive frame is one that
        // travels forward and goes *nowhere* vertically while a steep surface sits below it. Written
        // as `dy > -eps` this also swallows every frame that is climbing, where not descending is
        // exactly right: first the atomic step-ups, then — once those were excluded — the ordinary
        // walk up a hill, which rises through the slope ride and carries no `climb` at all. Three
        // false-positive classes on three consecutive captures, the second of which buried nine real
        // takeoffs under fifty-five phantoms. A window around zero admits none of them.
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
