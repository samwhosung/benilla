//! The **reveal audit** (`WOW_REVEAL=<frames>`) — one line per frame from a snap, naming every
//! term that decides whether the world the player is about to be shown is actually there.
//!
//! "Teleported and the city wasn't drawn for a frame or more" is a *temporal* claim about the
//! frames either side of a reveal, and neither a screenshot nor the loading screen's own 3-second
//! wait line can hold one: the screenshot is one frame with no state attached, and the wait line
//! only speaks for loads slow enough to be stuck. This prints the whole window — the cover, the
//! residency terms behind [`WorldLoadProgress::presentable`], the settle hold, and the retained
//! pass's collected-vs-published region counts — so "which term let the reveal through" is read
//! off a column instead of guessed.
//!
//! ```text
//! WOW_USER=probe1 WOW_PASS=pprobe1 WOW_CHAR=Probeone \
//!   WOW_PROBE_CHAT=".go xyz -9250.45 160.86 67.90 0;.tele stormwind" \
//!   WOW_PROBE_CHAT_AT=20 WOW_PROBE_CHAT_EVERY=25 WOW_REVEAL=120 \
//!   WOW_PROBE_EXIT_AT=70 cargo run -q -p benilla | grep REVEAL
//! ```
//!
//! The arming edge is the snap itself (a same-map teleport or a worldport), so the window covers
//! the frames a cover would have to be up for — and, when none is, the frames the player is
//! looking at the destination through nothing at all.
//!
//! **It runs in `Last`, and that is load-bearing.** Half of what decides whether a frame has a
//! world in it is settled in `PostUpdate` — bevy's visibility check, the exterior-scene gate, the
//! retained pass's own scene walk — and a line printed in `Update` reports those columns from the
//! *previous* frame. Against a one-or-two-frame artefact an off-by-one instrument is worse than
//! none: it attributes the hole to the frame beside it. In `Last` every column below is this
//! frame's, residency and draw alike.
//!
//! The columns split into two halves on purpose. **Residency** (`res`, `focus`, `scene`, `place`,
//! `coll`, `merge`, `gx`) is *what the world has* — the terms behind
//! [`WorldLoadProgress::presentable`], the ones the cover is raised and cleared on. **Draw**
//! (`drawn`, `hid`, `sel`, `room`, `win`, `pvseye`) is *what the frame put on screen*. A reveal
//! defect where every residency term reads ready and the draw half has collapsed is not a
//! streaming bug at all — it is the visibility authority answering about somewhere else.

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
    /// [`ProbeClock`] seconds at the arming snap — the `t=` column. The wall clock, like every
    /// other probe schedule: a reveal window is measured in real milliseconds,
    /// and the virtual clock clamps exactly the hitching frames this instrument exists to see.
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
        // In `Last`: after the streamer published residency, after the screen decided what to do
        // with it, and after `PostUpdate` settled what actually draws (see the module docs).
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
    // How far the visibility authority's pose is from the eye this frame is actually drawn from
    // — ~0 in steady play, the whole jump on a snap frame it has not caught up with.
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
