//! The LIVE probe shot — one screenshot (or an adjacent-frame burst) of a NORMAL connected run,
//! with the two validity gates that make its output *evidence*: the death gate (a dead/ghost
//! avatar renders the whole world through the death filter) and the subject gate
//! (`WOW_SHOT_REQUIRE` — a frame that cannot contain the reported subject is not a measurement of
//! it). Split from `probes` when the gates outgrew it; the sibling probe one-shots stay there.

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use benilla_protocol::guid;

use super::probes::ProbeClock;
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use benilla_world::view::WorldCamera;

/// How far the subject gate's WARN looks when listing what *is* nearby — wide enough to catch the
/// "right room, wrong wing" mis-aim even when `WOW_SHOT_REQUIRE_DIST` is tight.
const REQUIRE_NEARBY_RANGE: f32 = 100.0;

/// Seconds between subject-gate WARNs while waiting. Frequent enough to be read live, rare enough
/// that a log grep isn't wading through it.
const REQUIRE_REWARN_SECS: f32 = 5.0;

/// The LIVE probe shot (`WOW_LIVE_SHOT=<png>`, delay via `WOW_LIVE_SHOT_AT` seconds, default 12):
/// one screenshot of the primary window on a NORMAL connected run — the "what does the live scene
/// actually look like" instrument (server GameObjects, event spawns, NPCs — everything the
/// server-less harness deliberately excludes). The app keeps running after the save; pair with
/// `WOW_USER`/`WOW_CHAR` and an outer `timeout` for unattended probes. Unlike
/// [`super::CapturePlugin`], nothing is pinned — the shot shows the scene as a player would see it.
///
/// **`WOW_LIVE_SHOT_COUNT=<n>` makes it a burst** — `n` shots written as `<stem>-000.png`,
/// `<stem>-001.png`, … , spaced by `WOW_LIVE_SHOT_EVERY` seconds (**default 0 = one per frame**,
/// i.e. *adjacent* frames). One shot still writes the bare path, so every existing invocation is
/// unchanged.
///
/// The burst exists because **a flicker is a temporal artefact and a single frame cannot show one**.
/// "This object's textures flicker" was un-diagnosable with the instruments we had:
/// the reporter's stills prove nothing, and grading a live run by eye is exactly the loop rule 7
/// forbids. Adjacent frames from a *parked* camera ([`crate::player`]'s `WOW_PROBE_CAM`) turn it
/// into arithmetic: `benilla-visual flicker <dir>` collapses the burst to the envelope of what
/// would not hold still, and the lit pixels are the defect.
///
/// **`WOW_SHOT_REQUIRE=<name>` is the SUBJECT gate** (substring, case-insensitive; range via
/// `WOW_SHOT_REQUIRE_DIST`, default 60): no shot fires until a unit with that name is inside the
/// viewport and range — the same contract as the death gate below, because a frame that cannot
/// contain the reported subject is not a measurement of it either. Ten banshee runs were once read
/// as "can't reproduce" with the banshee never in frame, and a voidwalker crop-negative was a model
/// that had wandered out of a fixed window. While the subject is missing the gate
/// WARNs with what *is* nearby — names and distances — so a mis-aimed run re-aims itself instead of
/// ending as a silent blank; a run that exits with 0 shots written names its reason in the log.
/// The unit's feet anchor decides "in frame", and in frame does not mean unoccluded — the gate
/// closes the aim trap, not line-of-sight.
pub(crate) struct LiveShotPlugin;

impl Plugin for LiveShotPlugin {
    fn build(&self, app: &mut App) {
        let out = std::env::var("WOW_LIVE_SHOT").unwrap_or_default();
        let at = std::env::var("WOW_LIVE_SHOT_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(12.0);
        let count = std::env::var("WOW_LIVE_SHOT_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1u32)
            .max(1);
        let every = std::env::var("WOW_LIVE_SHOT_EVERY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let require = std::env::var("WOW_SHOT_REQUIRE")
            .ok()
            .filter(|v| !v.is_empty());
        let require_dist = std::env::var("WOW_SHOT_REQUIRE_DIST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60.0);
        app.insert_resource(LiveShot {
            out,
            count,
            every,
            taken: 0,
            next_at: at,
            require,
            require_dist,
        })
        .add_systems(Update, fire_live_shot);
    }
}

/// [`LiveShotPlugin`] state: the output path, how many shots the burst wants, their spacing
/// (`0` = one per frame), how many have gone out, and when the next one may — seeded to
/// `WOW_LIVE_SHOT_AT`, so the first-fire delay and the burst spacing are the same clock.
#[derive(Resource)]
struct LiveShot {
    out: String,
    count: u32,
    every: f32,
    taken: u32,
    next_at: f32,
    /// The subject gate: no shot until a unit whose name contains this is in frame and in range.
    require: Option<String>,
    require_dist: f32,
}

/// `stem-007.png` for shot 7 of a burst — the bare path when the burst is one shot, so the
/// single-shot filename (which existing probes and docs name literally) never moves.
fn burst_path(out: &str, index: u32, count: u32) -> String {
    if count <= 1 {
        return out.to_string();
    }
    match out.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-{index:03}.{ext}"),
        None => format!("{out}-{index:03}"),
    }
}

/// Fire the live screenshot(s) once the startup delay has elapsed — one per frame by default, so a
/// burst samples *adjacent* frames and a per-frame instability has nowhere to hide.
///
/// **Refuses to fire while the avatar is dead or a ghost.** `preflight` already reports it, but a
/// single WARN in a four-hundred-line log is not a gate: a ghost renders the whole world through the
/// death filter, so every image in the burst is a desaturated wash and *every* measurement correlated
/// against it — pixel series, hotspot runs, depth readback, the phase census — silently describes the
/// filter instead of the scene. That cost a whole B38 session: eight bursts and three retracted
/// mechanisms measured on a corpse, because the images still looked like a plausible frame. A loud
/// no-op is worth far more than a burst that has to be recognised as garbage after the fact.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn fire_live_shot(
    mut shot: ResMut<LiveShot>,
    time: ProbeClock,
    mut commands: Commands,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    subjects: Query<(&Guid, &Transform), (With<NetEntity>, Without<SelfPlayer>)>,
    camera: Query<(&Camera, &Transform), With<WorldCamera>>,
    names: Res<NameCache>,
    net_commands: Res<NetCommands>,
    mut refused: Local<bool>,
    mut next_require_warn: Local<f32>,
) {
    if shot.taken >= shot.count || time.elapsed_secs() < shot.next_at {
        return;
    }
    if let Ok(store) = self_q.single() {
        let what = if store.0.unit_is_dead() {
            Some("DEAD")
        } else if store.0.player_is_ghost() {
            Some("A GHOST")
        } else {
            None
        };
        if let Some(what) = what {
            if !*refused {
                *refused = true;
                error!(
                    "live-shot: REFUSING to capture — the character is {what}, so the world renders \
                     through the death filter and every pixel of this burst would describe that \
                     filter, not the scene. Send WOW_PROBE_CHAT=\".revive\" (probe accounts are \
                     gmlevel 6) and run again."
                );
            }
            return;
        }
    }
    // The SUBJECT gate — same contract as the death gate above: a frame that cannot contain the
    // reported subject is not a measurement of it, and a "clean" burst of such frames is the
    // costliest false negative a live run produces. Every shot of a burst
    // re-passes the gate, so a subject that wanders off mid-burst pauses the burst instead of
    // padding it with blanks.
    if let Some(want) = shot.require.as_deref() {
        let Ok((cam, cam_pose)) = camera.single() else {
            return;
        };
        let cam_tf = GlobalTransform::from(*cam_pose);
        let viewport = cam.logical_viewport_size().unwrap_or(Vec2::ZERO);
        let want_lc = want.to_lowercase();
        // One pass: find the nearest in-frame match, and keep the neighbourhood for the WARN —
        // `resolve` sends the name query on a miss, so just scanning fills the cache within frames.
        let mut found: Option<(String, f32, Vec2)> = None;
        let mut nearby: Vec<(f32, String)> = Vec::new();
        for (guid, tf) in &subjects {
            // Only name-bearing families: a GameObject can never resolve (its name rides its own
            // unmodeled query), so it would sit in the listing as noise and never match the gate.
            if !guid::is_player(guid.0)
                && !guid::is_creature_or_pet(guid.0)
                && guid::pet_number(guid.0).is_none()
            {
                continue;
            }
            let dist = cam_pose.translation.distance(tf.translation);
            let name = names
                .resolve(guid.0, &net_commands)
                .map(str::to_string)
                .unwrap_or_else(|| format!("<unresolved 0x{:x}>", guid.0));
            if dist <= shot.require_dist.max(REQUIRE_NEARBY_RANGE) {
                nearby.push((dist, name.clone()));
            }
            if !name.to_lowercase().contains(&want_lc) || dist > shot.require_dist {
                continue;
            }
            let Ok(screen) = cam.world_to_viewport(&cam_tf, tf.translation) else {
                continue; // behind the camera
            };
            if screen.x < 0.0 || screen.y < 0.0 || screen.x > viewport.x || screen.y > viewport.y {
                continue; // in front, but outside the frame
            }
            if found.as_ref().is_none_or(|(_, d, _)| dist < *d) {
                found = Some((name, dist, screen));
            }
        }
        match found {
            Some((name, dist, screen)) => {
                info!(
                    "live-shot: subject gate — {name:?} in frame at {dist:.1} yd, \
                     screen ({:.0}, {:.0})",
                    screen.x, screen.y
                );
            }
            None => {
                if time.elapsed_secs() >= *next_require_warn {
                    *next_require_warn = time.elapsed_secs() + REQUIRE_REWARN_SECS;
                    nearby.sort_by(|a, b| a.0.total_cmp(&b.0));
                    nearby.truncate(10);
                    let listing = if nearby.is_empty() {
                        "nothing within range at all".to_string()
                    } else {
                        nearby
                            .iter()
                            .map(|(d, n)| format!("{n:?} at {d:.1} yd"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    warn!(
                        "live-shot: WAITING — no unit named like {want:?} is in frame within \
                         {:.0} yd. Nearby: {listing}. Re-aim (`.go`, WOW_PROBE_CAM) or raise \
                         WOW_SHOT_REQUIRE_DIST; a run that exits with 0 shots written failed \
                         this gate.",
                        shot.require_dist
                    );
                }
                return;
            }
        }
    }
    let path = burst_path(&shot.out, shot.taken, shot.count);
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path.clone()));
    shot.taken += 1;
    shot.next_at = time.elapsed_secs() + shot.every;
    if shot.count > 1 {
        info!("live-shot: writing {path} ({}/{})", shot.taken, shot.count);
    } else {
        info!("live-shot: writing {path}");
    }
}
