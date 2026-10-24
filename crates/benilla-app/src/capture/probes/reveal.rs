//! `WOW_REVEAL=<frames>`: one `REVEAL` line per frame after a snap (a same-map teleport or a
//! worldport), naming every term that decides whether the destination is drawn. Residency columns
//! (`res`, `focus`, `scene`, `place`, `coll`, `merge`, `gx`) are the terms behind
//! [`WorldLoadProgress::presentable`], which the cover waits on; draw columns (`drawn`, `hid`,
//! `sel`, `room`, `win`, `pvslag`) are what the frame put on screen, and `pres` the frames the
//! window has presented since the snap ([`crate::loading_screen::hold::Presents`]).
//!
//! At the settle release one `REVEAL_SUM` line closes the window from the snap to residency:
//! `frames`, the frames it took; `presented`, how many the window presented (counted at the
//! release frame, so the release frame's own present is not in it); and `drawn` and `wmo`, the
//! submeshes and retained-pass WMOs that came on screen after the first frame shown, the sum of
//! each shown frame's growth over the last one shown (a frame
//! [`crate::loading_screen::hold::PresentHold`] holds is not shown).
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
    /// The snap-to-residency window's tally, `None` once its summary is printed.
    window: Option<RevealWindow>,
}

/// One snap's tally up to the settle release, for the `REVEAL_SUM` line.
struct RevealWindow {
    /// [`crate::loading_screen::hold::Presents`] at the snap.
    presents_at_snap: u64,
    frames: u32,
    /// Submeshes and WMOs drawn in the last frame shown, the baseline for growth.
    last_drawn: Option<(usize, usize)>,
    /// What came on screen in frames shown.
    drawn_grew: usize,
    wmo_grew: usize,
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
            window: None,
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
    presents: Option<Res<crate::loading_screen::hold::Presents>>,
    hold: Option<Res<crate::loading_screen::hold::PresentHold>>,
    mut teleports: MessageReader<crate::net::TeleportMessage>,
    mut worldports: MessageReader<crate::net::WorldportMessage>,
) {
    let now = time.elapsed_secs();
    let presented = presents
        .as_deref()
        .map_or(0, crate::loading_screen::hold::Presents::count);
    if teleports.read().next().is_some() || worldports.read().next().is_some() {
        audit.n = Some(0);
        audit.since = now;
        audit.window = Some(RevealWindow {
            presents_at_snap: presented,
            frames: 0,
            last_drawn: None,
            drawn_grew: 0,
            wmo_grew: 0,
        });
    }
    // The per-frame lines run for `frames` after the snap; the summary's tally runs to the release.
    let in_window = audit.n.is_some_and(|n| n < audit.frames);
    let n = audit.n.unwrap_or(0);
    audit.n = in_window.then_some(n + 1);
    if !in_window && audit.window.is_none() {
        return;
    }

    let (collected, published, selected) = gx
        .as_deref()
        .map_or((0, 0, 0), benilla_world::static_gx::StaticGx::wmo_census);
    let (cells_drawn, wmo_drawn, groups_drawn) = gx
        .as_deref()
        .map_or((0, 0, 0), benilla_world::static_gx::StaticGx::draw_census);
    let seen = census.take();
    let settling = player.as_ref().is_some_and(|p| p.settling);
    let since = audit.since;
    if let Some(w) = audit.window.as_mut() {
        w.frames += 1;
        // A held frame never reaches the window, so what it drew is not on screen.
        if !hold
            .as_deref()
            .is_some_and(crate::loading_screen::hold::PresentHold::held)
        {
            let shown = (seen.drawn, wmo_drawn);
            if let Some((d, g)) = w.last_drawn {
                w.drawn_grew += shown.0.saturating_sub(d);
                w.wmo_grew += shown.1.saturating_sub(g);
            }
            w.last_drawn = Some(shown);
        }
        // A snap the stream already holds releases on its own frame: `frames=1`.
        if !settling {
            info!(
                "REVEAL_SUM frames={} t={:.0}ms presented={} drawn={} wmo={} cover={}",
                w.frames,
                (now - since) * 1000.0,
                presented - w.presents_at_snap,
                w.drawn_grew,
                w.wmo_grew,
                u8::from(screen.covering()),
            );
            audit.window = None;
        }
    }
    if !in_window {
        return;
    }
    // Distance from the visibility authority's eye to the drawn camera: ~0 in steady play.
    let pvs_lag = match (seen.pvs_eye, cam.iter().next()) {
        (Some(eye), Some(now_eye)) => eye.distance(now_eye.translation()),
        _ => f32::NAN,
    };
    info!(
        "REVEAL n={n} t={:.0}ms cover={} res={}/{} focus={} scene={} place={} coll={} merge={} \
         gx={} wmo={collected}/{published}/{selected} settling={} pres={} | drawn={}/{} hid={}/{} \
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
        u8::from(settling),
        audit
            .window
            .as_ref()
            .map_or(0, |w| presented - w.presents_at_snap),
        seen.drawn,
        seen.submeshes,
        seen.hidden,
        seen.tagged,
        seen.room.as_deref().unwrap_or("-"),
        seen.windows.as_deref().unwrap_or("-"),
    );
}
