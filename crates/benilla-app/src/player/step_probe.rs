//! **The step-up probe** — the instrument that answers "why won't it climb *this* kerb?".
//!
//! The symptom is always the same from outside the window: you walk into something small, the
//! avatar keeps running on the spot, and nothing in the ordinary trace says why — the atomic
//! step-up ([`super::mover::step_up`]) declined, and one of six different reasons is the truth.
//! Reasoning cannot pick between them (0209's whole history is sessions that tried), and neither
//! can a screenshot, so this measures instead:
//!
//! 1. **The blocked frame is detected, not hunted.** A grounded walk frame whose achieved
//!    horizontal displacement is a fraction of what the input asked for *is* the report. No
//!    keybind to remember, no guessing which of the director's frames were the bad ones.
//! 2. **The surface profile ahead is measured** with one-sided down rays at a ladder of forward
//!    offsets: how tall the thing actually is, whether it has a walkable top at all, and — the
//!    reason the rays are one-sided like the mover's own probes — whether that top is even
//!    *visible* to a downward sweep under the 0970 facing law.
//! 3. **The maneuver is re-run at a ladder of advances.** The live step-up advances by this
//!    frame's own travel, which is 12 cm at 60 fps and 3 cm at 240 — while the capsule
//!    radius is [`super::CAPSULE_RADIUS`]. If a rung further out commits where the live one did
//!    not, the advance is the defect; if *every* rung says `NO-FLOOR`, the geometry is; if every
//!    rung says `STEEP-FLOOR`, the walkable gate is. One line, one verdict.
//! 4. **The candidate faces are dumped** ([`benilla_world::collision::WorldCollision::faces_near_body`]), with the
//!    facing gate's own answer per face, when the trace asks for them.
//!
//! Output goes two places: the `stup` tag of `WOW_MOVE_TRACE` (the record we read afterwards) and
//! [`latest`], a small ring the debug panel renders live — so the director can *see* the probe
//! fire at the spot they meant, which is what makes a capture worth reading (method §6).
//!
//! Cost: nothing at all on a frame that moves. On a blocked one it is rate-limited to
//! [`REPORT_HZ`] reports a second, each a handful of shape casts and rays — so leaning on a wall
//! forever cannot turn into a frame-rate story of its own.

use avian3d::prelude::*;
use bevy::prelude::*;
use std::sync::Mutex;

use super::mover::{step_up, StepVerdict};
use super::{CAPSULE_HEIGHT, CAPSULE_RADIUS, SKIN_WIDTH, STEP_UP_HEIGHT};

/// A walk frame counts as blocked when it achieved less than this share of the horizontal distance
/// the input asked for. A square push into a wall achieves ~0; a legitimate slide along one at 45°
/// still achieves ~70%, and a walkable-slope ride achieves 100% by construction —
/// so the band below this is "went nowhere", not "was deflected".
const BLOCKED_SHARE: f32 = 0.35;

/// Reports per second while blocked. Enough to catch a bump-and-retry, cheap enough to lean on.
const REPORT_HZ: f32 = 5.0;

/// Forward offsets the surface profile is sampled at (yd). The last two are past anything the
/// maneuver could reach — they say what the geometry *is*, which is how a "there is no top surface
/// here" reads differently from "the top is 0.9 yd away".
const PROFILE: [f32; 8] = [0.0, 0.1, 0.2, 0.35, 0.5, 0.7, 1.0, 1.4];

/// Advances the maneuver is re-run at (yd). The live rung — this frame's own travel — is prepended
/// at report time, so the table always opens with what actually happened.
const LADDER: [f32; 6] = [0.1, 0.2, CAPSULE_RADIUS, 0.5, 0.8, 1.2];

/// How many candidate faces the geometry dump prints, nearest first.
const FACE_DUMP: usize = 24;

/// The probe's own state: the rate limiter, and the last report for the panel to render.
struct Probe {
    /// App-elapsed seconds of the last report; `f32::MIN` before the first.
    last: f32,
    /// The most recent report, one string per line — what [`latest`] hands the debug panel.
    report: Vec<String>,
    /// App-elapsed seconds the report was taken, so the panel can grey out a stale one.
    at: f32,
}

static PROBE: Mutex<Probe> = Mutex::new(Probe {
    last: f32::MIN,
    report: Vec::new(),
    at: f32::MIN,
});

/// The last blocked report (lines) and the app-elapsed time it was taken — the debug panel's read.
pub(crate) fn latest() -> (Vec<String>, f32) {
    PROBE
        .lock()
        .map(|p| (p.report.clone(), p.at))
        .unwrap_or_default()
}

/// Watch one **local** grounded walk frame, and report it if the body went nowhere.
///
/// `from`/`to` are the capsule centre either side of [`super::mover::grounded_step`] — the walk
/// resolve alone, before the hover climb and the water-walk clamp, both of which move the body for
/// reasons that have nothing to do with a kerb.
pub(super) fn watch(
    world: &benilla_world::collision::WorldCollision<'_, '_>,
    capsule: &Collider,
    from: Vec3,
    to: Vec3,
    horiz_vel: Vec3,
    dt: f32,
    now: f32,
) {
    // **Nobody is reading this in a player build**. Unlike its neighbour
    // `move_trace`, which gates on `trace::enabled()` in its first line, this ran unconditionally:
    // every blocked walk frame — routine play, walking into a wall — paid a body shape-cast, eight
    // down-rays and seven full re-runs of `step_up`, up to 5×/s, to fill a `static` whose only
    // reader is the debug panel. 1174 filed this file as residue, "weight, not behaviour"; that was
    // wrong, and this is the correction. (Narrowing it further in a DEV build — to "the panel is
    // open or the trace is on" — is a live question the panel's readout has an opinion about, and
    // is deliberately not done here.)
    if !crate::run_mode::dev_affordances() {
        return;
    }
    let speed = horiz_vel.length();
    let wanted = speed * dt;
    // Below a millimetre of intent there is no "blocked" to speak of — standing still, or a frame
    // so short that every ratio is noise.
    if wanted < 1.0e-3 {
        return;
    }
    let d = to - from;
    let got = d.x.hypot(d.z);
    if got >= wanted * BLOCKED_SHARE {
        return;
    }
    {
        let Ok(mut probe) = PROBE.lock() else { return };
        if now - probe.last < 1.0 / REPORT_HZ {
            return;
        }
        probe.last = now;
    }

    let dir_h = horiz_vel / speed;
    let feet_y = from.y - CAPSULE_HEIGHT * 0.5;
    let cast = |c: Vec3, disp: Vec3| world.cast_body(capsule, c, disp, SKIN_WIDTH);

    // The spot is stamped in **WoW** coordinates as well as Bevy's: every other probe, every `.go
    // xyz`, and the director's own `/gps` speak that frame, so a blocked report is walkable-back-to
    // without anyone converting by hand. (Bevy's are what the rest of this report's geometry is in,
    // so both are on the line.)
    let feet_wow = benilla_assets::coords::bevy_to_wow(Vec3::new(from.x, feet_y, from.z));
    let mut lines = Vec::with_capacity(5);
    lines.push(format!(
        "BLOCKED wow ({:9.2},{:9.2},{:7.2}) bevy ({:9.2},{:7.2},{:9.2}) dir({:+.2},{:+.2}) \
         sp {speed:.2} dt {dt:.4} want {wanted:.3} got {got:.3} ({:.0}%) dy {:+.3}",
        feet_wow[0],
        feet_wow[1],
        feet_wow[2],
        from.x,
        from.y,
        from.z,
        dir_h.x,
        dir_h.z,
        100.0 * got / wanted,
        d.y,
    ));

    // What we are pressed against — a look-ahead a full radius deep, so the face is found even
    // when this frame's own travel is a centimetre and the live maneuver never saw it.
    let wall = cast(from, dir_h * CAPSULE_RADIUS);
    lines.push(match &wall {
        None => format!("  wall  none within {CAPSULE_RADIUS:.2} yd ahead"),
        Some(h) => format!(
            "  wall  d={:.3} n=({:+.2},{:+.2},{:+.2}) contact ({:8.2},{:7.2},{:8.2}) h={:+.2} {:?}",
            h.distance,
            h.normal1.x,
            h.normal1.y,
            h.normal1.z,
            h.point1.x,
            h.point1.y,
            h.point1.z,
            h.point1.y - feet_y,
            h.entity,
        ),
    });

    // The surface profile ahead: one-sided down rays from above the capsule, reported as height
    // above the feet. `miss` on a rung with solid geometry under it is the facing law rejecting a
    // top face — the reading no shape cast can give, because a sweep that finds nothing and a
    // sweep that finds a backface are the same `None`.
    let eye = from + Vec3::Y * STEP_UP_HEIGHT;
    let ray_len = STEP_UP_HEIGHT + CAPSULE_HEIGHT * 0.5 + 1.0;
    let profile: Vec<String> = PROFILE
        .iter()
        .map(
            |&o| match world.ray_body(eye + dir_h * o, Dir3::NEG_Y, ray_len) {
                None => format!("{o:+.2}:miss"),
                Some(h) => format!(
                    "{o:+.2}:{:+.2}/{:+.2}",
                    eye.y - h.distance - feet_y,
                    h.normal.y
                ),
            },
        )
        .collect();
    lines.push(format!("  ahead {}", profile.join(" ")));

    // The ladder: the same maneuver, at advances the live one never tries. The live rung first.
    let rungs: Vec<String> = std::iter::once(wanted)
        .chain(LADDER)
        .map(|adv| {
            let v = step_up(&cast, from, dir_h, adv.max(wanted), adv, STEP_UP_HEIGHT).verdict;
            // `fwd` — how far the elevated sweep ACTUALLY got — is on every rung, because without
            // it a failing far rung is two different stories: "we advanced that far and the floor
            // there is steep" and "something at head height stopped us short of it".
            let tag = match v {
                StepVerdict::NoFace => "no-face".to_string(),
                StepVerdict::NoHeadroom => "NO-HEADROOM".to_string(),
                StepVerdict::NoFloor { fwd, .. } => format!("NO-FLOOR fwd{fwd:.2}"),
                StepVerdict::SteepFloor { fwd, ny, .. } => format!("STEEP fwd{fwd:.2} ny{ny:+.2}"),
                StepVerdict::NetZero { fwd, dy, .. } => format!("net-zero fwd{fwd:.2} dy{dy:+.3}"),
                StepVerdict::Commit { fwd, dy, .. } => format!("COMMIT fwd{fwd:.2} dy{dy:+.3}"),
            };
            format!("{adv:.2}:{tag}")
        })
        .collect();
    lines.push(format!("  ladder {}", rungs.join(" | ")));

    // The faces themselves, when the trace asks. Kept off the panel and off an untraced run: it is
    // two dozen lines, and it is the answer only when the three readings above disagree.
    let mut faces = Vec::new();
    if benilla_assets::trace::enabled_for("stup") {
        let at = from + dir_h * CAPSULE_RADIUS;
        let half = Vec3::new(1.0, CAPSULE_HEIGHT * 0.5 + STEP_UP_HEIGHT, 1.0);
        for f in world.faces_near_body(at, half, FACE_DUMP) {
            let c = f.centroid();
            faces.push(format!(
                "  face  n=({:+.2},{:+.2},{:+.2}) mid ({:8.2},{:7.2},{:8.2}) h={:+.2} \
                 down={} fwd={} {:?}",
                f.normal.x,
                f.normal.y,
                f.normal.z,
                c.x,
                c.y,
                c.z,
                c.y - feet_y,
                if f.blocks(Vec3::NEG_Y) {
                    "block"
                } else {
                    "PASS "
                },
                if f.blocks(dir_h) { "block" } else { "PASS " },
                f.entity,
            ));
        }
    }

    for l in lines.iter().chain(&faces) {
        benilla_assets::trace::line("stup", l);
    }
    if let Ok(mut probe) = PROBE.lock() {
        probe.report = lines;
        probe.at = now;
    }
}
