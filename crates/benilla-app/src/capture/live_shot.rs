//! The live probe shot: a screenshot or adjacent-frame burst of a connected run, gated on a live
//! avatar (a ghost renders through the death filter) and on the subject being in frame.

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use benilla_protocol::guid;

use super::probes::ProbeClock;
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use benilla_world::view::WorldCamera;

/// How far the subject gate's warning looks when listing what is nearby.
const REQUIRE_NEARBY_RANGE: f32 = 100.0;

/// Seconds between subject-gate warnings while waiting.
const REQUIRE_REWARN_SECS: f32 = 5.0;

/// The live probe shot (`WOW_LIVE_SHOT=<png>`, after `WOW_LIVE_SHOT_AT` seconds, default 12): the
/// primary window of a connected run, nothing pinned; the app keeps running after the save.
///
/// `WOW_LIVE_SHOT_COUNT=<n>` makes a burst of `<stem>-000.png`, ... spaced `WOW_LIVE_SHOT_EVERY`
/// seconds (default 0, adjacent frames), so a flicker from a parked camera (`WOW_PROBE_CAM`) can
/// be measured with `benilla-visual flicker <dir>`.
///
/// `WOW_SHOT_REQUIRE=<name>` (case-insensitive substring; range `WOW_SHOT_REQUIRE_DIST`, default
/// 60) holds every shot until a unit so named has its feet in the viewport and in range, and warns
/// with what is nearby while it waits. In frame does not mean unoccluded.
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

/// [`LiveShotPlugin`] state; `next_at` starts at `WOW_LIVE_SHOT_AT`, then steps by `every`.
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

/// `stem-007.png` for shot 7 of a burst; the bare path for a single shot.
fn burst_path(out: &str, index: u32, count: u32) -> String {
    if count <= 1 {
        return out.to_string();
    }
    match out.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-{index:03}.{ext}"),
        None => format!("{out}-{index:03}"),
    }
}

/// Fires the live shots once the delay has elapsed. Refuses while the avatar is dead or a ghost:
/// the world then renders through the death filter, and every frame would measure the filter.
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
    // The subject gate: every shot of a burst re-passes it, so a subject that wanders off pauses
    // the burst.
    if let Some(want) = shot.require.as_deref() {
        let Ok((cam, cam_pose)) = camera.single() else {
            return;
        };
        let cam_tf = GlobalTransform::from(*cam_pose);
        let viewport = cam.logical_viewport_size().unwrap_or(Vec2::ZERO);
        let want_lc = want.to_lowercase();
        // `resolve` sends the name query on a miss, so scanning fills the cache within frames.
        let mut found: Option<(String, f32, Vec2)> = None;
        let mut nearby: Vec<(f32, String)> = Vec::new();
        for (guid, tf) in &subjects {
            // Only name-bearing families: a GameObject's name rides its own query, not this cache.
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
