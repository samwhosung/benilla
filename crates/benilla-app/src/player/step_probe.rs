//! The step-up probe: when [`super::mover::step_up`] declines and a grounded walk frame achieves a
//! fraction of the asked distance, it reports the wall ahead, the surface profile from one-sided
//! down rays, the maneuver re-run at a ladder of advances and, traced, the candidate faces. A rung
//! committing beyond the live one blames the advance; `NO-FLOOR` on every rung, the geometry;
//! `STEEP` on every rung, the walkable gate. Output goes to the `stup` trace tag and to
//! [`latest`], which the debug panel shows live, so a run shows the probe fired at the intended
//! spot (`docs/METHOD.md`, loop step 5).

use avian3d::prelude::*;
use bevy::prelude::*;
use std::sync::Mutex;

use super::mover::{step_up, StepVerdict};
use super::{CAPSULE_HEIGHT, CAPSULE_RADIUS, SKIN_WIDTH, STEP_UP_HEIGHT};

/// A walk frame is blocked below this share of the asked horizontal distance: a push into a wall
/// achieves about 0, a 45° slide along one about 70%.
const BLOCKED_SHARE: f32 = 0.35;

/// Reports per second while blocked.
const REPORT_HZ: f32 = 5.0;

/// Forward offsets of the surface profile in yd; the last two lie past the maneuver's reach.
const PROFILE: [f32; 8] = [0.0, 0.1, 0.2, 0.35, 0.5, 0.7, 1.0, 1.4];

/// Advances the maneuver is re-run at in yd, after the live rung (this frame's own travel).
const LADDER: [f32; 6] = [0.1, 0.2, CAPSULE_RADIUS, 0.5, 0.8, 1.2];

/// How many candidate faces the geometry dump prints, nearest first.
const FACE_DUMP: usize = 24;

/// The rate limiter and the last report, for the panel.
struct Probe {
    /// App-elapsed seconds of the last report; `f32::MIN` before the first.
    last: f32,
    report: Vec<String>,
    /// App-elapsed seconds the report was taken; the panel greys out a stale one.
    at: f32,
}

static PROBE: Mutex<Probe> = Mutex::new(Probe {
    last: f32::MIN,
    report: Vec::new(),
    at: f32::MIN,
});

/// The last blocked report's lines and the app-elapsed time it was taken, for the debug panel.
pub(crate) fn latest() -> (Vec<String>, f32) {
    PROBE
        .lock()
        .map(|p| (p.report.clone(), p.at))
        .unwrap_or_default()
}

/// Reports one local grounded walk frame if the body went nowhere. `from`/`to` are the capsule
/// centre around [`super::mover::grounded_step`] alone, before the hover and water-walk moves.
pub(super) fn watch(
    world: &benilla_world::collision::WorldCollision<'_, '_>,
    capsule: &Collider,
    from: Vec3,
    to: Vec3,
    horiz_vel: Vec3,
    dt: f32,
    now: f32,
) {
    // Dev builds only: the debug panel is the result's reader.
    if !crate::run_mode::dev_affordances() {
        return;
    }
    let speed = horiz_vel.length();
    let wanted = speed * dt;
    // Under a millimetre of intent every ratio is noise.
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

    // WoW coordinates, as `.go xyz` takes them, and Bevy's, as the rest of the report uses.
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

    // A full radius ahead, so the face is found even when this frame's travel is tiny.
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

    // One-sided down rays, as height above the feet: `miss` over solid geometry is the facing law
    // rejecting a top face, which a shape cast cannot tell from empty space.
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

    // The same maneuver at advances the live one never tries, the live rung first.
    let rungs: Vec<String> = std::iter::once(wanted)
        .chain(LADDER)
        .map(|adv| {
            let v = step_up(&cast, from, dir_h, adv.max(wanted), adv, STEP_UP_HEIGHT).verdict;
            // `fwd`, the raised sweep's reach, tells a steep floor from a stop at head height.
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

    // The faces, traced runs only: two dozen lines, needed when the readings above disagree.
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
