//! The live-run probe instruments, riding a connected session (unlike the server-less
//! [`super::CapturePlugin`] harness). Each is env-gated and registered by `main`; the live
//! screenshot is [`super::live_shot`].

use bevy::prelude::*;

/// The particle-draw census (`WOW_FX_CENSUS=1`).
mod fx_draw_census;
pub(crate) use fx_draw_census::plugin as fx_draw_census_plugin;
mod act;
pub(crate) use act::{
    ProbeChatPlugin, ProbeDragPlugin, ProbeHoverPlugin, ProbeKeyPlugin, ProbeLuaPlugin,
};

/// The probe run's shell: its bounded lifetime and its window's level, parking and size.
mod run;
pub(crate) use run::{ProbeExitPlugin, ProbeFocusPlugin, ProbeResizePlugin};

/// The live frame-time sample, `FPS_PROBE`, and the lines printed from the same window.
mod live_fps;
pub(crate) use live_fps::LiveFpsPlugin;

/// The particle census: per-emitter state and draw-distance accounting.
mod particle_census;
pub(crate) use particle_census::ParticleCensusPlugin;

/// The under-floor census: per unit, where the server put it against where we drew it.
mod ground_census;
pub(crate) use ground_census::GroundCensusPlugin;

/// The transport census: per type-11/15 GameObject, its arm stage, visibility and mesh count.
mod lift_census;
pub(crate) use lift_census::LiftCensusPlugin;
mod trail_census;
pub(crate) use trail_census::TrailCensusPlugin;

/// The unit-visual census: per entity, which visual its display got, telling a debug cube from a
/// model that draws nothing.
mod visual_census;
pub(crate) use visual_census::UnitVisualsPlugin;

/// The motion-jitter meter: per frame, the camera, root and pose terms of a subject's rendered
/// position as first and second differences.
mod jitter;
pub(crate) use jitter::JitterMeterPlugin;

/// The dress census: per player, what the wire asked for, what we resolved and what hangs off the
/// skeleton.
mod dress_census;
pub(crate) use dress_census::DressCensusPlugin;

/// The reveal audit: per frame from a snap, every term deciding whether the world is drawable.
mod reveal;
pub(crate) use reveal::RevealAuditPlugin;

/// The two exclusive-`World` dumps: the bevy_ui node inventory and the archetype census.
mod world_census;
pub(crate) use world_census::{EntityCensusPlugin, NodeProbePlugin};

/// The schedule census: per schedule in both worlds, every system with its executor flags.
mod sched_census;
pub(crate) use sched_census::SchedCensusPlugin;

/// The frame-stall injector: blocks the main loop on a schedule to produce one huge frame delta;
/// at `WOW_STALL=0` a plain frame-delta and occlusion monitor.
mod stall;
pub(crate) use stall::StallPlugin;

/// The clock every probe schedule reads: real time. `Time<Virtual>` clamps each frame delta to
/// `max_delta` (250 ms), so on a hitching leg it falls behind and drifts every `<secs>` knob.
pub(crate) type ProbeClock<'w> = Res<'w, Time<bevy::time::Real>>;

#[cfg(test)]
mod tests {
    /// No probe system under `capture/` reads the virtual clock, except the [`ALLOWED`] entries.
    #[test]
    fn probe_schedules_read_the_wall_clock() {
        /// `(file, system, why it is genuinely an age or a delta on the animating clock)`.
        const ALLOWED: &[(&str, &str, &str)] = &[
            (
                "capture/fxview.rs",
                "drive_fx_view",
                "the other half of `drive_capture`'s fixture age below: the driver spawns, flies \
                 and reaps the subject on the same clock the effect animates on, which the capture \
                 freezes at save time. A wall clock here would age the fixture apart from the \
                 visuals it exists to show — an age, not a schedule",
            ),
            (
                "capture/mod.rs",
                "drive_capture",
                "the fixture AGE runs on the clock the effect animates on, and the capture freezes \
                 that same clock at save time — an age, not a schedule",
            ),
            (
                "capture/probes/stall.rs",
                "drive_stall",
                "the clamp IS the subject: this probe reports `Time<Virtual>`'s delta beside \
                 `Time<Real>`'s so a stalled frame's divergence is a printed number rather than a \
                 remembered constant. Its own schedule is [`ProbeClock`], bound on the line above",
            ),
            (
                "capture/waterfx.rs",
                "spawn / drive",
                "the foam fixture is a SIMULATION rig, not a schedule: it walks a synthetic dummy \
                 through the shipped emitter path, and the emitter reads its velocity on the same \
                 animating clock. A wall clock here would desync the dummy from the thing it is \
                 feeding — the one case where matching the virtual clock is the correctness \
                 requirement rather than the bug",
            ),
        ];

        // Needles are assembled at runtime so the checker does not flag its own source.
        let bare = format!(": Res<{}>,", "Time");
        let bare_last = format!(": Res<{}>", "Time");
        let explicit = format!("Res<{}<{}>>", "Time", "Virtual");

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut allowed_seen = 0usize;
        // The probe harness is everything under `capture/`.
        let mut stack = vec![src.join("capture")];
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("probe harness dir is readable") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        for path in files {
            let rel = path
                .strip_prefix(&src)
                .expect("under src")
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path).expect("source is readable");
            for (n, line) in text.lines().enumerate() {
                let t = line.trim();
                // A param binding the virtual clock; `ResMut<Time<Virtual>>` is clock control.
                let virtual_clock =
                    (t.ends_with(&bare) || t.ends_with(&bare_last)) || t.contains(&explicit);
                if !virtual_clock {
                    continue;
                }
                match ALLOWED.iter().find(|(f, _, _)| *f == rel) {
                    Some(_) => allowed_seen += 1,
                    None => offenders.push(format!("{rel}:{}  {t}", n + 1)),
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "the probe harness must schedule on the wall clock (`ProbeClock`), not the virtual \
             clock — it is clamped to max_delta (250 ms), so any hitching leg silently drifts every \
             `<secs>` knob out from under the operator. Offenders:\n  {}",
            offenders.join("\n  "),
        );
        assert!(
            allowed_seen > 0,
            "the ALLOWED exception list is stale — nothing matched it. If the fixture-age clock \
             moved or went away, drop its entry rather than leaving a rule guarding nothing.",
        );
    }
}
