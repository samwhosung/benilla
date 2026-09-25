//! `WOW_REVEAL=<frames>`: one `REVEAL` line per frame after a snap (a same-map teleport or a
//! worldport), naming every term that decides whether the destination is drawn. Residency columns
//! (`res`, `focus`, `scene`, `place`, `coll`, `merge`, `gx`) are the terms behind
//! [`WorldLoadProgress::presentable`], which the cover waits on; draw columns (`drawn`, `hid`,
//! `sel`, `room`, `win`, `pvslag`) are what the frame put on screen.
//!
//! Runs in `Last`: visibility, the exterior-scene gate and the retained pass settle in
//! `PostUpdate`, so an `Update` line would report them a frame late.

use bevy::prelude::*;

use super::ProbeClock;
use benilla_world::static_gx::StaticGx;
use benilla_world::terrain_stream::WorldLoadProgress;
use benilla_world::world_census::WorldCensus;

/// The audit's window: how many frames after a snap to report, from `WOW_REVEAL`.
#[derive(Resource)]
struct RevealAudit {
    frames: u32,
    /// Frames printed since the current arm; `None` = not armed.
    n: Option<u32>,
    /// [`ProbeClock`] seconds at the arming snap, the base of the `t=` column.
    since: f32,
}

pub(crate) struct RevealAuditPlugin;

impl Plugin for RevealAuditPlugin {
    fn build(&self, app: &mut App) {
        let frames = std::env::var("WOW_REVEAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120u32);
        app.insert_resource(RevealAudit {
            frames,
            n: None,
            since: 0.0,
        })
        // `Last`: after residency, the loading screen and `PostUpdate`'s visibility.
        .add_systems(Last, drive_reveal_audit);
    }
}

fn drive_reveal_audit(
    mut audit: ResMut<RevealAudit>,
    progress: Res<WorldLoadProgress>,
    gx: Option<Res<StaticGx>>,
    screen: Res<crate::loading_screen::LoadingScreen>,
    player: Option<Res<crate::player::Player>>,
    census: WorldCensus,
    cam: Query<&GlobalTransform, With<benilla_world::view::WorldCamera>>,
    time: ProbeClock,
    mut teleports: MessageReader<crate::net::TeleportMessage>,
    mut worldports: MessageReader<crate::net::WorldportMessage>,
) {
    let now = time.elapsed_secs();
    if teleports.read().next().is_some() || worldports.read().next().is_some() {
        audit.n = Some(0);
        audit.since = now;
    }
    let Some(n) = audit.n else {
        return;
    };
    if n >= audit.frames {
        audit.n = None;
        return;
    }
    audit.n = Some(n + 1);

    let (collected, published, selected) = gx
        .as_deref()
        .map_or((0, 0, 0), benilla_world::static_gx::StaticGx::wmo_census);
    let (cells_drawn, wmo_drawn, groups_drawn) = gx
        .as_deref()
        .map_or((0, 0, 0), benilla_world::static_gx::StaticGx::draw_census);
    let seen = census.take();
    // Distance from the visibility authority's eye to the drawn camera: ~0 in steady play.
    let pvs_lag = match (seen.pvs_eye, cam.iter().next()) {
        (Some(eye), Some(now_eye)) => eye.distance(now_eye.translation()),
        _ => f32::NAN,
    };
    info!(
        "REVEAL n={n} t={:.0}ms cover={} res={}/{} focus={} scene={} place={} coll={} merge={} \
         gx={} wmo={collected}/{published}/{selected} settling={} | drawn={}/{} hid={}/{} \
         sel={cells_drawn}/{wmo_drawn}/{groups_drawn} room={} win={} pvslag={pvs_lag:.0}",
        (now - audit.since) * 1000.0,
        u8::from(screen.covering()),
        progress.ready,
        progress.total,
        u8::from(progress.focus_resident),
        u8::from(progress.scene_ready),
        progress.placements_pending,
        progress.colliders_pending,
        progress.merge_pending,
        progress.gx_pending,
        u8::from(player.is_some_and(|p| p.settling)),
        seen.drawn,
        seen.submeshes,
        seen.hidden,
        seen.tagged,
        seen.room.as_deref().unwrap_or("-"),
        seen.windows.as_deref().unwrap_or("-"),
    );
}
