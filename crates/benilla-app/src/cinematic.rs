//! Cinematic playback: the race-intro fly-by and every other `SMSG_TRIGGER_CINEMATIC`.
//!
//! A cinematic is an in-engine camera flight: the trigger's `CinematicSequences.dbc` row names its
//! `CinematicCamera.dbc` shots, each an eye/target/roll path in a `Cameras\*.m2`, evaluated by
//! [`benilla_formats::CinematicPath`]. This module is the playback.
//!
//! - A trigger before the world is up waits in a single-slot latch (`0xc4d75c`), last write wins,
//!   and starts when the world-load gate opens (`0x5deb78`); here the gate is the loading screen.
//! - The path plays as an M2 animation (`0x7121a0`, sequence 0, rate 1.0), and a shot ends at its
//!   sequence band's end ([`CinematicPath::duration_ms`]); shipped fly-bys clamp and play once.
//! - Every shot, the first included, is announced with `CMSG_NEXT_CINEMATIC_CAMERA` from the shot
//!   arm (`0x48edf0`, sent at `0x48ef11`, after a far shot's load). A camera id of 0 ends the cinematic (`0x48efe0`).
//! - ESC is no engine binding: `StopCinematic` has no native caller, and the only skip is
//!   `CinematicFrame.xml`'s `OnKeyDown`, whose Lua queues
//!   [`SessionRequest::StopCinematic`](benilla_ui::script::SessionRequest).
//! - `CMSG_COMPLETE_CINEMATIC` goes out once, on a natural end and on a skip alike (`0x48f080`).
//!   Unacked, vmangos keeps object visibility on its own copy of the flying camera
//!   (`Player.cpp:1448`), and everything around the body stays despawned until relog.
//! - Every boundary (start, shot advance, end) cuts through black: a 0.25 s fade-out
//!   (`[0x804550]`, `0x4c0d10`) whose completion runs the step, then a fade-in (`0x4c1280`). There
//!   is no audio fade on this path.
//! - A shot that moves the camera more than 30 yards (`0x48eec1`; start and advance share the
//!   arm `0x48edf0`) loads its destination in full before it fades in: `0x48eed3` calls
//!   `0x66fc80`, which runs the synchronous map update `0x694920` at the black, and only then are
//!   the clock restarted, the shot's sound played (`0x48ef2e`) and the shot announced. Here the
//!   black is held instead, until the camera's spot is presentable or the teleport hold's
//!   ceiling runs out ([`ShotHold`]). The end does the same for the body's spot when the camera
//!   ends more than 30 yards from the followed object (`0x48f0fc` to `0x48f118`).
//!
//! Playback takes over, and releases, the camera pose, the streaming focus (which follows the
//! camera, so the flight is over streamed terrain) and the UI's cinematic flag, which drives
//! `CinematicFrame` and makes `InCinematic()` true so `StaticPopup` suppresses dialogs.

use std::time::Duration;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::{CinematicCatalog, CinematicPath};
use benilla_world::schedule::WorldStage;
use benilla_world::view::WorldCamera;
use bevy::prelude::*;

use crate::client_state::ClientState;
use crate::loading_screen::LoadingScreen;
use crate::net::{CinematicTriggeredMessage, ClientCommand, NetCommands};
use crate::player::{PlayerControlSet, SETTLE_TIMEOUT};
use crate::ui_script::SweepApp;

/// Both cinematic DBCs, read once at startup.
#[derive(Resource, Default)]
pub(crate) struct Cinematics(pub(crate) CinematicCatalog);

/// The shot being played, plus the deferred-start latch.
#[derive(Resource, Default)]
pub(crate) struct Cinematic {
    /// The reference's single-slot latch (`0xc4d75c`): a sequence the world was not ready to show.
    pending: Option<u32>,
    playing: Option<Playing>,
    /// Seconds of black left while the world comes back after the end. The reference's
    /// EndCinematic (`0x48f080`) blocks on a terrain load at full black and raises no loading cover
    /// (`0x406800` has no caller there); streaming cannot block, so the black holds instead, for
    /// at most [`SETTLE_BUDGET`].
    settling: Option<f32>,
    /// A skip not yet paid: `StopCinematic` runs EndCinematic as its fade-out's completion
    /// (`0x48f067`, `0x48f080`), so [`stop`] sets this and [`drive`] pays it at full black.
    skip: bool,
    /// A far shot's black, held until the world at its first eye is in ([`ShotHold`]).
    hold: Option<ShotHold>,
    /// How many far shots have asked for a reload so far: the stream's focus carries it, so its
    /// load re-arms for each (a cinematic's focus is unpaced throughout, with no pacing edge).
    reloads: u32,
}

/// The black a shot that moved the camera past the reload gate holds before it fades in.
///
/// The reference's arm (`0x48edf0`, and `0x48efe0` into it) loads that spot synchronously with the
/// screen at black and only then restarts the shot's clock and fades in (`0x48eed3` to `0x48ef43`).
/// The stream cannot block, so the black holds with the shot's clock stopped. It ends on the same
/// residency a teleport's hold does ([`crate::player::residency_reached`] and a presentable
/// world), or at [`SETTLE_TIMEOUT`] from the shot's arm, which nothing extends.
struct ShotHold {
    /// `Time::elapsed_secs` at which the hold gives up.
    ceiling: f32,
    /// The arm's own frame: the stream has not yet seen the camera at the new spot, so the world
    /// it reports is the last shot's.
    fresh: bool,
}

/// One cinematic in flight.
struct Playing {
    /// The run's number since process start: a re-trigger of the playing sequence is a new run,
    /// which the narration follower must see as a new shot.
    run: u64,
    /// The `CinematicSequences.dbc` id, for logging.
    sequence_id: u32,
    /// The row's shots in order, never empty.
    shots: Vec<CinematicPath>,
    /// Which shot is on screen.
    index: usize,
    /// Time inside the current shot.
    elapsed: Duration,
}

impl Playing {
    fn shot(&self) -> &CinematicPath {
        &self.shots[self.index]
    }
}

impl Cinematic {
    /// Is a trigger latched for the world to show (`0xc4d75c`)?
    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Is a cinematic on screen right now? The engine half of `InCinematic()`.
    pub(crate) fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// The far reloads shots have asked for so far.
    pub(crate) fn reloads(&self) -> u32 {
        self.reloads
    }

    /// A far shot asks the stream for a new load.
    pub(crate) fn mark_reload(&mut self) {
        self.reloads = self.reloads.wrapping_add(1);
    }

    /// The shot on screen: `(run, shot index, narration sound id)`, keyed on [`Playing::run`] so a
    /// re-trigger counts as a new shot. None while a far shot's world loads (the black holds): its
    /// sound plays after the load (`0x48ef2e`), and the shot left had its sound cut at the advance.
    pub(crate) fn playing_shot(&self) -> Option<(u64, usize, u32)> {
        if self.hold.is_some() {
            return None;
        }
        let play = self.playing.as_ref()?;
        Some((play.run, play.index, play.shot().sound_id))
    }
}

#[cfg(test)]
impl Cinematic {
    /// A latched trigger, as one the world was not yet ready to show.
    pub(crate) fn pending_for_test(id: u32) -> Self {
        Self {
            pending: Some(id),
            ..Self::default()
        }
    }

    /// A shot-less cinematic for tests that only ask [`Cinematic::is_playing`]; a [`CinematicPath`]
    /// needs a real `Cameras\*.m2`, so anything that reads a shot panics here.
    pub(crate) fn playing_for_test() -> Self {
        Self {
            pending: None,
            playing: Some(Playing {
                run: 0,
                sequence_id: 0,
                shots: Vec::new(),
                index: 0,
                elapsed: Duration::ZERO,
            }),
            settling: None,
            skip: false,
            hold: None,
            reloads: 0,
        }
    }

    /// A cinematic playing `shots`, for tests of the driver.
    pub(crate) fn playing_shots_for_test(shots: Vec<CinematicPath>) -> Self {
        let mut cine = Self::playing_for_test();
        if let Some(play) = cine.playing.as_mut() {
            play.shots = shots;
        }
        cine
    }

    /// Is a far shot's black being held for its world?
    pub(crate) fn holds_black_for_shot(&self) -> bool {
        self.hold.is_some()
    }
}

/// One of the two letterbox bars, full-width black, top or bottom.
/// Deviation: Bevy UI nodes computed per frame, not `CinematicFrame.xml`'s textures, so a window
/// resized mid-cinematic stays letterboxed. `CinematicFrame` still shows and owns ESC.
#[derive(Component)]
struct LetterboxBar;

/// What [`drive_letterbox`] switched off at the start, so it puts back only what it took.
#[derive(Default)]
struct Takeover {
    cursor: bool,
}

pub(crate) struct CinematicPlugin;

impl Plugin for CinematicPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Cinematic>()
            .add_systems(
                Startup,
                (load_catalog.after(AssetSet::Open), spawn_letterbox),
            )
            .add_systems(
                Update,
                // One chain in one frame, after the net drain and after `control` seats the
                // camera: no follow-camera frame shows, and this pose is the last word on it.
                (take_stop, take_trigger, start_pending, drive)
                    .chain()
                    .in_set(WorldStage::Input)
                    .after(PlayerControlSet),
            )
            // Only once the in-game UI is up: `CINEMATIC_START` into a VM with no `CinematicFrame`
            // would leave nothing listening for ESC.
            .add_systems(
                Update,
                feed_ui
                    .in_set(WorldStage::Input)
                    .after(drive)
                    .run_if(crate::ui_script::ingame_ui_up),
            )
            // The cursor and the bars, after the driver settled this frame's state.
            .add_systems(
                Update,
                drive_letterbox.in_set(WorldStage::Input).after(drive),
            )
            // Leaving the world drops a cinematic with no ack, as the reference's teardown does
            // (`0x490a80` clears the flag and sends no `CMSG_COMPLETE_CINEMATIC`); the exit
            // edge drops a latch too, where a departure with the sweep spent leaves one.
            .add_sweep_leg(
                crate::ui_script::SweepLeg::Cinematic,
                abandon_cinematic_on_leave_world,
            )
            // After the shutdown, whose sweep tells the VM `CINEMATIC_STOP` for a cinematic in
            // flight (`0x490b77`), and before the glue VM goes in.
            .add_systems(
                OnExit(ClientState::InWorld),
                abandon_on_leaving_world.in_set(crate::ui_script::SessionEnd::Clears),
            );
    }
}

/// The two bars, spawned once and parked hidden.
fn spawn_letterbox(mut commands: Commands) {
    for top in [true, false] {
        commands.spawn((
            LetterboxBar,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: if top { Val::Px(0.0) } else { Val::Auto },
                bottom: if top { Val::Auto } else { Val::Px(0.0) },
                width: Val::Percent(100.0),
                height: Val::Px(0.0),
                ..default()
            },
            BackgroundColor(Color::BLACK),
            // Above the world and the UI quads, below the loading cover's `GlobalZIndex` of 1000.
            GlobalZIndex(900),
            Visibility::Hidden,
        ));
    }
}

/// The height of one letterbox bar, in the screen's units ([`drive_letterbox`] has the law).
fn letterbox_bar(width: f32, screen: f32) -> f32 {
    let two_to_one = (screen - (width / 2.0).min(screen)) / 2.0;
    two_to_one.min(screen / 6.0).max(0.0)
}

/// Hide the cursor and raise the letterbox while a shot is on screen; put both back after.
///
/// `CinematicFrame.lua:9` crops to 2:1 (`width/2` capped at the height) only when wider than 4:3;
/// otherwise the bars keep the XML's `1024 x 128` on the native `1024 x 768` sheet, a sixth of the
/// height, which is the 2:1 formula at exactly 4:3. The whole law is
/// `min((height - min(width/2, height))/2, height/6)`, so 4:3 and narrower still get bars.
fn drive_letterbox(
    cine: Res<Cinematic>,
    mut bars: Query<(&mut Node, &mut Visibility), With<LetterboxBar>>,
    windows: Query<&Window>,
    mut cursor: Query<&mut bevy::window::CursorOptions, With<bevy::window::PrimaryWindow>>,
    mut ours: Local<Takeover>,
    mut logged: Local<f32>,
) {
    let playing = cine.is_playing();
    // The HUD is Lua's, as in the reference: `CINEMATIC_START` runs `ShowUIPanel`, whose
    // `area = "full"` routes to `SetFullScreenFrame`, which hides `UIParent`.

    // The hardware cursor hides at StartCinematic and returns at EndCinematic and the leave-world
    // teardown (`0x58b590(0)`/`(1)`); nothing else hides it while the view is detached.
    if let Ok(mut opts) = cursor.single_mut() {
        if let Some(visible) =
            crate::cursor::pointer_visibility(playing, &mut ours.cursor, opts.visible)
        {
            opts.visible = visible;
        }
    }

    let height = playing
        .then(|| windows.iter().next())
        .flatten()
        .map_or(0.0, |w| {
            let (width, screen) = (w.width(), w.height().max(1.0));
            letterbox_bar(width, screen)
        });
    // The measured crop, logged at `info!` on the edges and on a resize, never every frame.
    if height != *logged {
        *logged = height;
        if height > 0.0 {
            info!("cinematic: letterbox bar {height:.1} px");
        }
    }
    for (mut node, mut vis) in &mut bars {
        let want = if height > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        if node.height != Val::Px(height) {
            node.height = Val::Px(height);
        }
    }
}

fn load_catalog(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let mut chain = assets.chain.lock_recover();
    match benilla_formats::load_cinematics(&mut chain) {
        Ok(cat) => {
            info!(
                "cinematic: {} sequences, {} cameras",
                cat.sequence_count(),
                cat.camera_count()
            );
            commands.insert_resource(Cinematics(cat));
        }
        // With no catalog every trigger falls through to the immediate ack.
        Err(e) => warn!("cinematic: tables failed to load: {e:#}"),
    }
}

/// A posted `StopCinematic()` ([`crate::ui_logout::SessionPosts`]), at the chain's head.
fn take_stop(
    posts: Option<Res<crate::ui_logout::SessionPosts>>,
    mut cine: ResMut<Cinematic>,
    net: Option<Res<NetCommands>>,
) {
    if posts.is_some_and(|p| p.take_stop_cinematic()) {
        stop(&mut cine, net.as_deref());
    }
}

/// Latch a triggered cinematic (or ack it immediately, if it names nothing we can play).
fn take_trigger(
    mut triggered: MessageReader<CinematicTriggeredMessage>,
    mut cine: ResMut<Cinematic>,
    catalog: Option<Res<Cinematics>>,
    net: Option<Res<NetCommands>>,
) {
    for msg in triggered.read() {
        let id = msg.cinematic_id;
        let playable = catalog
            .as_deref()
            .is_some_and(|c| !c.0.shots(id).is_empty());
        if !playable {
            // Nothing to play: ack at once rather than leave the server flying a path unwatched.
            warn!("cinematic: {id} names no shot we can play — acking it");
            ack(net.as_deref());
            continue;
        }
        // Last write wins, but the displaced id is still owed its ack ([`relinquish`]).
        if let Some(dropped) = cine.pending.replace(id) {
            warn!("cinematic: {dropped} displaced by {id} before it started — acking it");
            ack(net.as_deref());
        }
    }
}

/// Give up whatever is in flight, paying one `CMSG_COMPLETE_CINEMATIC` per accepted trigger: an
/// extra ack is harmless, a missing one costs a relog (module doc), so two live triggers pay two.
fn relinquish(cine: &mut Cinematic, net: Option<&NetCommands>, why: &str) {
    if let Some(play) = cine.playing.take() {
        info!("cinematic: {} {why}", play.sequence_id);
        cine.hold = None;
        ack(net);
    }
    if let Some(id) = cine.pending.take() {
        info!("cinematic: {id} {why} before it started");
        ack(net);
    }
}

/// Start a latched cinematic once the world is actually up.
fn start_pending(
    mut cine: ResMut<Cinematic>,
    catalog: Option<Res<Cinematics>>,
    assets: Option<Res<WorldAssets>>,
    screen: Option<Res<LoadingScreen>>,
    net: Option<Res<NetCommands>>,
    state: Res<State<ClientState>>,
    mut fade: ResMut<crate::screen_fade::ScreenFade>,
    time: Res<Time>,
    camera: Query<&Transform, With<WorldCamera>>,
) {
    let Some(id) = cine.pending else { return };
    // The world-load gate: in the world, where `CinematicFrame` (the only ESC route) exists, and
    // not under the loading cover, which would hide the opening seconds.
    if *state.get() != ClientState::InWorld || screen.is_some_and(|s| s.covering()) {
        return;
    }
    let (Some(catalog), Some(assets)) = (catalog, assets) else {
        return;
    };
    // The shot is armed as the fade-out's completion (`0x48edc5` arms `0x4c0d10(0x48edd0, 0,
    // 0.25)`), so all below runs at full black. Gated after every can't-play return, so a failed
    // start never darkens the screen.
    if !fade.is_black() {
        fade.fade_out(crate::screen_fade::CINEMATIC);
        return;
    }
    cine.pending = None;

    let rows: Vec<_> = catalog.0.shots(id).into_iter().cloned().collect();
    let mut chain = assets.chain.lock_recover();
    let mut shots = Vec::with_capacity(rows.len());
    for row in &rows {
        match CinematicPath::load(&mut chain, row) {
            Ok(p) => shots.push(p),
            // One unreadable shot does not sink the cinematic: play what parses.
            Err(e) => warn!("cinematic: camera {} failed to load: {e:#}", row.id),
        }
    }
    if shots.is_empty() {
        warn!("cinematic: {id} had no loadable shot — acking it");
        ack(net.as_deref());
        // Nothing will show under this black, so give the world back.
        fade.fade_in(crate::screen_fade::CINEMATIC);
        return;
    }
    info!(
        "cinematic: playing {id} — {} shot(s), {} ms",
        shots.len(),
        shots.iter().map(|s| s.duration_ms).sum::<u32>()
    );
    // A trigger on top of a running cinematic replaces it and acks the old one; a GM
    // `.debug play cinematic` during an intro does this.
    if let Some(old) = cine.playing.take() {
        warn!(
            "cinematic: {} replaced by {id} while playing — acking it",
            old.sequence_id
        );
        ack(net.as_deref());
    }
    cine.hold = None;
    static RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    cine.playing = Some(Playing {
        run: RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        sequence_id: id,
        shots,
        index: 0,
        elapsed: Duration::ZERO,
    });
    let from = camera.single().ok().map(|t| t.translation);
    open_shot(
        &mut cine,
        &mut fade,
        net.as_deref(),
        from,
        time.elapsed_secs(),
    );
}

/// The tail of the shot arm (`0x48edf0`), for the first shot and every advance alike: the shot
/// comes in from the black it was armed under (`0x48ef43`), unless its first eye is more than 30
/// yards from the camera (`0x48eec1`), where the arm first loads that spot at the black
/// (`0x48eed3`) and restarts the shot's clock. So a far shot keeps the black with its clock at
/// zero ([`ShotHold`]); `from` is the camera as it stands, none with no camera to move. The shot's
/// `CMSG_NEXT_CINEMATIC_CAMERA` (`0x48ef11`) follows that load, so a far shot sends it when the
/// hold ends.
fn open_shot(
    cine: &mut Cinematic,
    fade: &mut crate::screen_fade::ScreenFade,
    net: Option<&NetCommands>,
    from: Option<Vec3>,
    now: f32,
) {
    let Some(play) = cine.playing.as_mut() else {
        return;
    };
    let eye = benilla_assets::coords::wow_to_bevy(play.shot().sample(0).eye);
    if from.is_some_and(|from| crate::player::is_far_reload(from, eye)) {
        play.elapsed = Duration::ZERO;
        cine.mark_reload();
        cine.hold = Some(ShotHold {
            ceiling: now + SETTLE_TIMEOUT,
            fresh: true,
        });
        info!("cinematic: far shot — the black holds while its world loads");
    } else {
        announce_shot(net);
        fade.fade_in(crate::screen_fade::CINEMATIC);
    }
}

/// Is a held far shot's world in? `Some(true)` once the camera's spot is resident and presentable,
/// `Some(false)` when the ceiling ran out first, `None` while it waits. With no world progress to
/// read, or no camera, there is nothing to wait on.
fn shot_hold_over(
    hold: &mut ShotHold,
    progress: Option<&benilla_world::terrain_stream::WorldLoadProgress>,
    at: Option<Vec3>,
    now: f32,
) -> Option<bool> {
    // The arm's own frame reads a world that has not seen the new spot.
    if std::mem::take(&mut hold.fresh) {
        return None;
    }
    if now >= hold.ceiling {
        return Some(false);
    }
    let (Some(p), Some(at)) = (progress, at) else {
        return Some(true);
    };
    (crate::player::residency_reached(p, at) && p.presentable()).then_some(true)
}

/// `CMSG_NEXT_CINEMATIC_CAMERA`, sent as each shot is shown, the first included (`0x48ef11`): at
/// its arm, or for a far shot once its world is in ([`open_shot`]).
/// vmangos ignores it (`MiscHandler.cpp:919`) and flies its camera copy on its own clock.
fn announce_shot(net: Option<&NetCommands>) {
    if let Some(net) = net {
        let _ = net.0.send(ClientCommand::NextCinematicCamera);
    }
}

/// The most seconds the end boundary holds black for the world, which usually takes under one;
/// running out logs and reveals anyway.
const SETTLE_BUDGET: f32 = 8.0;

/// Advance the shot and seat the camera on it; hand over to the next shot, or end the cinematic.
///
/// Runs after `control` seats the follow camera, which it re-seats every frame, so the pose needs
/// no restore. The FOV is left alone: the M2 camera's `fov` (written at `0x70f336`) is read by
/// nothing on this path, its one reader `0x7ac640` serving only portraits and `<Model>` frames.
fn drive(
    mut cine: ResMut<Cinematic>,
    time: Res<Time>,
    net: Option<Res<NetCommands>>,
    mut fade: ResMut<crate::screen_fade::ScreenFade>,
    progress: Option<Res<benilla_world::terrain_stream::WorldLoadProgress>>,
    screen: Option<Res<LoadingScreen>>,
    player: Option<Res<crate::player::Player>>,
    mut camera: Query<&mut Transform, With<WorldCamera>>,
) {
    // --- The end boundary's tail: hold the black while the world comes back around the body.
    // Both terms, as each covers the other's one-frame race; either false keeps the black.
    if let Some(left) = cine.settling {
        let home = progress
            .as_deref()
            .is_some_and(|p| p.is_ready() && p.presentable())
            && !screen.as_deref().is_some_and(LoadingScreen::covering);
        let left = left - time.delta_secs();
        if home || left <= 0.0 {
            if !home {
                warn!(
                    "cinematic: the world did not come back inside the settle budget — \
                     revealing anyway"
                );
            }
            cine.settling = None;
            // `0x48f199`: the tail of EndCinematic fades back in to the live world.
            fade.fade_in(crate::screen_fade::CINEMATIC);
        } else {
            cine.settling = Some(left);
        }
        return;
    }

    // A skip with only a latch in flight is settled by [`stop`], so here a skip has a shot to fade.
    let skip = cine.skip;
    // A far shot's clock restarts once its world is in (`0x50ca10` after the load), so it stands
    // at zero under the black.
    let holding = cine.hold.is_some();
    let camera_at = camera.single().ok().map(|t| t.translation);
    let Some(play) = cine.playing.as_mut() else {
        return;
    };
    if !holding {
        play.elapsed += time.delta();
    }

    // --- Every boundary cuts through black: a run-out shot or a skip fades out and steps at full
    // black, as the fade's completion (`0x48efd7` to `0x48efe0` to advance, `0x48f067` to
    // `0x48f080` to stop), which keeps the cut invisible under a hitch.
    if skip || (!holding && play.elapsed.as_millis() as u32 >= play.shot().duration_ms) {
        if !fade.is_black() {
            fade.fade_out(crate::screen_fade::CINEMATIC);
            // Fall through to seat the camera: what darkens is the shot's own last pose.
        } else {
            // Walk past every shot this frame's delta ran through, each owed its
            // `CMSG_NEXT_CINEMATIC_CAMERA`, inside one black.
            let mut ended = skip;
            let mut passed = 0usize;
            while !ended {
                let play = cine.playing.as_mut().expect("checked above");
                if (play.elapsed.as_millis() as u32) < play.shot().duration_ms {
                    break;
                }
                let over = play.elapsed - Duration::from_millis(u64::from(play.shot().duration_ms));
                if play.index + 1 >= play.shots.len() {
                    ended = true;
                    break;
                }
                play.index += 1;
                play.elapsed = over;
                passed += 1;
            }
            // The shot left up announces itself from [`open_shot`], after its load; the ones
            // passed through are owed theirs now.
            for _ in 0..(if ended {
                passed
            } else {
                passed.saturating_sub(1)
            }) {
                announce_shot(net.as_deref());
            }
            if ended {
                cine.skip = false;
                cine.hold = None;
                if skip {
                    // A skip takes a latched trigger with it, acked all the same.
                    relinquish(&mut cine, net.as_deref(), "stopped");
                } else if let Some(play) = cine.playing.take() {
                    // A natural end is not [`relinquish`]: a latched trigger is owed its playback,
                    // which `start_pending` picks up next.
                    info!("cinematic: {} finished", play.sequence_id);
                    ack(net.as_deref());
                }
            }
            if ended {
                // Not faded in yet: the world around the body streamed away during the flight, so
                // hold the black the reference shows while its own load blocks. EndCinematic
                // reloads the body's spot only when the camera ended more than 30 yards from the
                // followed object (`0x48f0fc` to `0x48f118`); counts latched under the camera say
                // nothing of it, so the stream is asked for a new load behind the same gate.
                let body = player.as_deref().filter(|p| p.active).map(|p| p.pos);
                if camera_at
                    .zip(body)
                    .is_some_and(|(cam, body)| crate::player::is_far_reload(cam, body))
                {
                    cine.mark_reload();
                }
                cine.settling = Some(SETTLE_BUDGET);
                return;
            }
            // `0x48ef43`: the next shot comes in from the black it was armed under, after its
            // world if it is far.
            open_shot(
                &mut cine,
                &mut fade,
                net.as_deref(),
                camera_at,
                time.elapsed_secs(),
            );
        }
    }

    // --- A far shot's black ends with its world, or at the ceiling.
    if let Some(hold) = cine.hold.as_mut().filter(|_| !skip) {
        let over = shot_hold_over(hold, progress.as_deref(), camera_at, time.elapsed_secs());
        if let Some(resident) = over {
            if !resident {
                warn!(
                    "cinematic: the shot's world did not load inside the settle budget — \
                     revealing anyway"
                );
            }
            cine.hold = None;
            // `0x48ef11`: the shot is announced after its load, then it fades in.
            announce_shot(net.as_deref());
            fade.fade_in(crate::screen_fade::CINEMATIC);
        }
    }

    let Ok(mut cam) = camera.single_mut() else {
        return;
    };
    let play = cine.playing.as_ref().expect("checked above");
    let shot = play.shot();
    // Clamped: under a boundary's fade the pose holds at the shot's last authored one.
    let view = shot.sample((play.elapsed.as_millis() as u32).min(shot.duration_ms));
    let eye = benilla_assets::coords::wow_to_bevy(view.eye);
    let target = benilla_assets::coords::wow_to_bevy(view.target);

    // The roll is about the view axis and authored around whole turns (`FlyByDwarf` holds 2π), so
    // it is applied as an angle, never tested for zero. The WoW-to-Bevy basis is a proper rotation,
    // so its sign carries.
    let forward = (target - eye).normalize_or_zero();
    let up = if forward == Vec3::ZERO {
        Vec3::Y
    } else if forward.cross(Vec3::Y).length_squared() < 1e-6 {
        // Looking straight up or down, rolling `Y` about `Y` is the identity: roll `Z` instead.
        Quat::from_axis_angle(forward, view.roll) * Vec3::Z
    } else {
        Quat::from_axis_angle(forward, view.roll) * Vec3::Y
    };
    cam.translation = eye;
    if forward != Vec3::ZERO {
        cam.look_at(target, up);
    }
}

/// ESC, or anything else asking for the skip: end the cinematic and ack it. On the wire a skip is
/// a completion.
fn stop(cine: &mut Cinematic, net: Option<&NetCommands>) {
    if cine.playing.is_some() {
        // Something is on screen: [`drive`] pays at full black, as `StopCinematic` (`0x48f050`)
        // arms a 0.25 s fade-out with EndCinematic (`0x48f080`) as its completion.
        cine.skip = true;
        return;
    }
    // A latch still waiting on the world goes too, paid now: no [`drive`] pass will settle it.
    relinquish(cine, net, "stopped");
}

/// The leave-world sweep's last step (`0x490b69`-`0x490bbb`), past the interaction close and
/// outside the sweep's sound scope: a cinematic in flight (`[0xb4e310]`) is dropped, the VM the
/// sweep runs in told `CINEMATIC_STOP` (`0x490b77`), with no `CMSG_COMPLETE_CINEMATIC`.
fn abandon_cinematic_on_leave_world(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut cine: ResMut<Cinematic>,
    mut fade: ResMut<crate::screen_fade::ScreenFade>,
) {
    hosted_vm.run(|script| {
        if !cine.is_playing() {
            return;
        }
        // The world-ready latch (`0xc4d75c`) is another cell, which the sweep leaves.
        let pending = cine.pending;
        abandon(&mut cine, &mut fade);
        cine.pending = pending;
        if let Some(script) = script {
            script.set_in_cinematic(false);
            script.fire_event("CINEMATIC_STOP", vec![]);
        }
    })
}

/// Leaving the world drops a cinematic without acking; the socket goes with it.
fn abandon_on_leaving_world(
    mut cine: ResMut<Cinematic>,
    mut fade: ResMut<crate::screen_fade::ScreenFade>,
) {
    abandon(&mut cine, &mut fade);
}

/// Drop the cinematic and its latch with no ack, and any fade.
fn abandon(cine: &mut Cinematic, fade: &mut crate::screen_fade::ScreenFade) {
    if let Some(play) = cine.playing.take() {
        info!("cinematic: {} abandoned (left the world)", play.sequence_id);
    }
    cine.pending = None;
    cine.skip = false;
    cine.settling = None;
    cine.hold = None;
    // Clear any fade: the glue comes up next and must not sit behind our black.
    fade.clear();
}

/// Fire the `CINEMATIC_START`/`CINEMATIC_STOP` edges and keep `InCinematic()` current. The memo
/// is per VM and per world: the leave-world sweep drops a cinematic and tells the VM itself.
fn feed_ui(
    cine: Res<Cinematic>,
    mut hosted_vm: crate::script_calls::HostedVm,
    mut published: crate::ui_script::WindowMemo<Option<bool>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let playing = cine.is_playing();
        let published = published.get(script);
        if *published == Some(playing) {
            return;
        }
        // A fresh VM is already out of a cinematic: seed the memo rather than fire a
        // `CINEMATIC_STOP` the reference never sends unedged.
        if published.is_none() && !playing {
            *published = Some(false);
            return;
        }
        *published = Some(playing);
        script.set_in_cinematic(playing);
        script.fire_event(
            if playing {
                "CINEMATIC_START"
            } else {
                "CINEMATIC_STOP"
            },
            vec![],
        );
    })
}

fn ack(net: Option<&NetCommands>) {
    if let Some(net) = net {
        let _ = net.0.send(ClientCommand::CompleteCinematic);
    }
}

#[cfg(test)]
mod tests {
    use super::letterbox_bar;

    /// Both of the reference's cases, on its native `1024 x 768` sheet, from the one expression.
    #[test]
    fn the_letterbox_matches_the_reference_on_its_own_sheet() {
        // Exactly 4:3, the branch the Lua does not take: the XML's 128 of 768.
        assert_eq!(letterbox_bar(1024.0, 768.0), 128.0);

        // Wider, the Lua's `desiredHeight = width/2`: 16:10 on the sheet leaves bars of 76.8.
        assert!((letterbox_bar(1228.8, 768.0) - 76.8).abs() < 0.05);
    }

    #[test]
    fn a_widescreen_picture_is_cropped_to_two_to_one() {
        for (w, h) in [(1920.0, 1080.0), (2560.0, 1440.0), (1600.0, 900.0)] {
            let bar = letterbox_bar(w, h);
            let picture = h - 2.0 * bar;
            assert!(
                (w / picture - 2.0).abs() < 1e-3,
                "{w}x{h}: bar {bar} leaves {picture}, aspect {}",
                w / picture
            );
        }
    }

    #[test]
    fn four_three_and_narrower_are_still_letterboxed() {
        // 4:3 at a real resolution.
        assert!((letterbox_bar(1024.0, 768.0) / 768.0 - 1.0 / 6.0).abs() < 1e-6);
        assert!((letterbox_bar(1600.0, 1200.0) / 1200.0 - 1.0 / 6.0).abs() < 1e-6);
        // 5:4 and square: the reference's textures stay put, so the cap holds at a sixth.
        assert!((letterbox_bar(1280.0, 1024.0) / 1024.0 - 1.0 / 6.0).abs() < 1e-6);
        assert!((letterbox_bar(768.0, 768.0) / 768.0 - 1.0 / 6.0).abs() < 1e-6);
    }

    #[test]
    fn no_window_shape_produces_a_nonsense_bar() {
        for (w, h) in [(1.0, 1.0), (4000.0, 100.0), (100.0, 4000.0), (0.0, 720.0)] {
            let bar = letterbox_bar(w, h);
            assert!(bar >= 0.0, "{w}x{h} -> {bar}");
            assert!(2.0 * bar <= h, "{w}x{h} -> {bar} swallows the screen");
        }
    }
}

/// The ack ledger: one `CMSG_COMPLETE_CINEMATIC` per accepted trigger, which no screen shows.
#[cfg(test)]
mod ack_ledger {
    use super::*;
    use crate::net::ClientCommand;

    fn wire() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    fn acks(rx: &crossbeam_channel::Receiver<ClientCommand>) -> usize {
        rx.try_iter()
            .filter(|c| matches!(c, ClientCommand::CompleteCinematic))
            .count()
    }

    /// Run `drive` once with the screen already at full black, the frame a boundary settles on.
    fn settle_at_black(cine: Cinematic, net: NetCommands) -> App {
        let mut fade = crate::screen_fade::ScreenFade::default();
        fade.fade_out(0.0);
        assert!(fade.is_black());
        let mut app = App::new();
        app.insert_resource(cine);
        app.insert_resource(net);
        app.insert_resource(fade);
        app.init_resource::<Time>();
        app.add_systems(Update, drive);
        app.update();
        app
    }

    /// The ESC skip's one ack is paid at full black, not at the key press (`0x48f050`).
    #[test]
    fn stopping_a_playing_cinematic_acks_it_once() {
        let (net, rx) = wire();
        let mut cine = Cinematic::playing_for_test();
        stop(&mut cine, Some(&net));
        assert_eq!(acks(&rx), 0, "nothing is paid before the screen is black");
        assert!(cine.is_playing(), "and the shot is still up, fading out");

        let app = settle_at_black(cine, net);
        assert_eq!(acks(&rx), 1, "paid once, at black");
        assert!(!app.world().resource::<Cinematic>().is_playing());
    }

    /// Two triggers accepted, two acks owed: not a duplicate to squash.
    #[test]
    fn a_playing_cinematic_and_a_latched_one_each_pay() {
        let (net, rx) = wire();
        let mut cine = Cinematic::playing_for_test();
        cine.pending = Some(41);
        stop(&mut cine, Some(&net));
        let app = settle_at_black(cine, net);
        assert_eq!(acks(&rx), 2);
        assert!(app.world().resource::<Cinematic>().pending.is_none());
    }

    /// A natural end leaves a latched trigger to be played, not acked.
    #[test]
    fn a_natural_end_does_not_pay_off_a_latched_trigger() {
        let (net, rx) = wire();
        // A shot-less `playing_for_test` cannot run out through `drive`, so this runs the
        // natural-end branch itself.
        let mut cine = Cinematic::playing_for_test();
        cine.pending = Some(41);
        let net_ref = &net;
        if let Some(play) = cine.playing.take() {
            info!("cinematic: {} finished", play.sequence_id);
            ack(Some(net_ref));
        }
        assert_eq!(acks(&rx), 1, "only the one that was on screen");
        assert_eq!(cine.pending, Some(41), "the latch survives to be played");
    }

    #[test]
    fn stopping_nothing_acks_nothing() {
        let (net, rx) = wire();
        let mut cine = Cinematic::default();
        stop(&mut cine, Some(&net));
        assert_eq!(acks(&rx), 0);
    }

    /// A displaced latch is acked: the server still started the cinematic it dropped.
    #[test]
    fn a_displaced_latch_is_acked() {
        let (net, rx) = wire();
        let mut cine = Cinematic {
            pending: Some(41),
            ..Default::default()
        };
        // What `take_trigger` does on the second message.
        if let Some(_dropped) = cine.pending.replace(81) {
            ack(Some(&net));
        }
        assert_eq!(acks(&rx), 1);
        assert_eq!(cine.pending, Some(81));
    }

    /// Leaving the world pays nothing, as the reference's teardown (`0x490a80`).
    #[test]
    fn abandoning_the_world_acks_nothing() {
        let (net, rx) = wire();
        let mut app = App::new();
        app.insert_resource(Cinematic::playing_for_test());
        app.insert_resource(net);
        app.init_resource::<crate::screen_fade::ScreenFade>();
        app.add_systems(Update, abandon_on_leaving_world);
        app.update();
        assert_eq!(acks(&rx), 0);
        assert!(!app.world().resource::<Cinematic>().is_playing());
    }

    /// A `/reload` mid-cinematic: the sweep's tail drops it (`0x490b69`), telling the dying VM
    /// `CINEMATIC_STOP` with no ack, and the new VM starts out of a cinematic.
    #[test]
    fn a_reload_drops_the_cinematic_and_the_new_vm_starts_nothing() {
        use crate::ui_script::sweep_test::{reload_in_place, seen, seen_in, spied_vm};
        const EVENTS: [&str; 2] = ["CINEMATIC_START", "CINEMATIC_STOP"];
        let (net, rx) = wire();
        let mut app = App::new();
        app.insert_resource(Cinematic::playing_for_test())
            .insert_resource(net)
            .init_resource::<crate::screen_fade::ScreenFade>()
            .add_sweep_leg(
                crate::ui_script::SweepLeg::Cinematic,
                abandon_cinematic_on_leave_world,
            )
            .add_systems(Update, feed_ui);
        app.insert_non_send_resource(spied_vm(&EVENTS));
        app.update();
        assert_eq!(seen_in(&app), ["CINEMATIC_START"]);

        let dying = reload_in_place(&mut app, &EVENTS);
        assert_eq!(seen(&dying), ["CINEMATIC_START", "CINEMATIC_STOP"]);
        assert_eq!(acks(&rx), 0);
        assert!(!app.world().resource::<Cinematic>().is_playing());

        app.update();
        assert!(seen_in(&app).is_empty(), "the new VM is out of a cinematic");
    }
}

/// The end boundary holds the black until the world is back around the body, where the
/// reference blocks on its terrain load (`0x48f080`).
#[cfg(test)]
mod settling_under_black {
    use super::*;
    use crate::net::ClientCommand;
    use benilla_world::terrain_stream::WorldLoadProgress;

    fn wire() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    /// A world that is, or is not, presentable at the focus.
    fn world(home: bool) -> WorldLoadProgress {
        WorldLoadProgress {
            ready: 1,
            total: 1,
            focus_resident: home,
            scene_ready: home,
            ..Default::default()
        }
    }

    /// An app sitting on the end boundary at full black, with the world still away.
    fn ended_at_black() -> App {
        let (net, _rx) = wire();
        let mut fade = crate::screen_fade::ScreenFade::default();
        fade.fade_out(0.0);
        let mut cine = Cinematic::playing_for_test();
        cine.skip = true; // the shot is over; `playing_for_test` has no shot to run out
        let mut app = App::new();
        app.insert_resource(cine);
        app.insert_resource(net);
        app.insert_resource(fade);
        app.insert_resource(world(false));
        app.init_resource::<Time>();
        app.add_systems(Update, drive);
        app.update();
        app
    }

    #[test]
    fn the_shot_ends_into_black_and_stays_there_while_the_world_is_away() {
        let app = ended_at_black();
        assert!(
            !app.world().resource::<Cinematic>().is_playing(),
            "the shot is over and acked"
        );
        assert!(
            app.world().resource::<Cinematic>().settling.is_some(),
            "…and the end boundary latched a settle rather than finishing"
        );
        assert!(
            app.world()
                .resource::<crate::screen_fade::ScreenFade>()
                .is_black(),
            "**the screen is still black** — this is the loading cover that never gets shown"
        );
    }

    #[test]
    fn the_world_coming_home_is_what_reveals_it() {
        let mut app = ended_at_black();
        app.insert_resource(world(true));
        app.update();
        assert!(
            app.world().resource::<Cinematic>().settling.is_none(),
            "the settle is done"
        );
        assert!(
            !app.world()
                .resource::<crate::screen_fade::ScreenFade>()
                .is_black(),
            "and the world fades back in"
        );
    }

    #[test]
    fn a_world_that_never_returns_still_gets_the_screen_back() {
        let mut app = ended_at_black();
        // Spend the whole budget with the world still away.
        app.world_mut().resource_mut::<Cinematic>().settling = Some(0.0);
        app.update();
        assert!(app.world().resource::<Cinematic>().settling.is_none());
        assert!(
            !app.world()
                .resource::<crate::screen_fade::ScreenFade>()
                .is_black(),
            "the screen comes back even though the world did not"
        );
    }
}

/// A shot that moves the camera past the reload gate keeps the black until its world is in
/// (`0x48eec1`, then the synchronous load `0x66fc80` at the black, `0x48eed3`).
#[cfg(test)]
mod far_shots {
    use super::*;
    use crate::net::ClientCommand;
    use benilla_formats::{CinematicCameraRow, CinematicPath};
    use benilla_world::terrain_stream::WorldLoadProgress;
    use bevy::ecs::system::RunSystemOnce;

    /// The receiving end of the cinematic's outgoing packets.
    #[derive(Resource)]
    struct Wire(crossbeam_channel::Receiver<ClientCommand>);

    /// Where the camera stands before the shot, wow coordinates.
    const HERE: [f32; 3] = [-8_949.0, -132.0, 83.0];
    /// Ten minutes of flight from `HERE`, on another tile.
    const FAR: [f32; 3] = [-3_000.0, 2_500.0, 90.0];

    fn tile_of(wow: [f32; 3]) -> (i32, i32) {
        let (tx, ty) = benilla_formats::world_to_tile(wow[0], wow[1]);
        (tx as i32, ty as i32)
    }

    fn shot(eye: [f32; 3], ms: u32) -> CinematicPath {
        let row = CinematicCameraRow {
            id: 1,
            model: String::new(),
            sound_id: 0,
            origin: [0.0; 3],
            origin_facing: 0.0,
        };
        CinematicPath::held_pose(&row, eye, [eye[0] + 10.0, eye[1], eye[2]], ms)
    }

    /// `HERE`, moved `yards` along x.
    fn from_here(yards: f32) -> [f32; 3] {
        [HERE[0] + yards, HERE[1], HERE[2]]
    }

    /// The world's report about the tile at `wow`: in, or still out.
    fn world_at(wow: [f32; 3], resident: bool) -> WorldLoadProgress {
        WorldLoadProgress {
            ready: usize::from(resident),
            total: 1,
            focus_resident: resident,
            scene_ready: resident,
            focus_tile: Some(tile_of(wow)),
            ..Default::default()
        }
    }

    fn black() -> crate::screen_fade::ScreenFade {
        let mut fade = crate::screen_fade::ScreenFade::default();
        fade.fade_out(0.0);
        assert!(fade.is_black());
        fade
    }

    /// The driver over a camera at `HERE`, at full black, the clock stepping 100 ms a frame.
    fn app_with(cine: Cinematic, world: WorldLoadProgress) -> App {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(cine)
            .insert_resource(NetCommands(tx))
            .insert_resource(Wire(rx))
            .insert_resource(black())
            .insert_resource(world)
            .init_resource::<Time>();
        app.world_mut().spawn((
            WorldCamera,
            Transform::from_translation(benilla_assets::coords::wow_to_bevy(HERE)),
        ));
        app.add_systems(Update, drive);
        app
    }

    /// One frame: the clock steps, then the driver runs.
    fn frame(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(100));
        app.update();
    }

    fn black_now(app: &App) -> bool {
        app.world()
            .resource::<crate::screen_fade::ScreenFade>()
            .is_black()
    }

    fn holding(app: &App) -> bool {
        app.world().resource::<Cinematic>().holds_black_for_shot()
    }

    /// How many `CMSG_NEXT_CINEMATIC_CAMERA` went out since the last look.
    fn announced(app: &App) -> usize {
        app.world()
            .resource::<Wire>()
            .0
            .try_iter()
            .filter(|c| matches!(c, ClientCommand::NextCinematicCamera))
            .count()
    }

    fn set_world(app: &mut App, world: WorldLoadProgress) {
        app.insert_resource(world);
    }

    /// What `start_pending` does with a started cinematic: the shot arm's tail, from the camera.
    fn open_first_shot(app: &mut App) {
        app.world_mut()
            .run_system_once(
                |mut cine: ResMut<Cinematic>,
                 mut fade: ResMut<crate::screen_fade::ScreenFade>,
                 time: Res<Time>,
                 net: Res<NetCommands>,
                 camera: Query<&Transform, With<WorldCamera>>| {
                    let from = camera.single().ok().map(|t| t.translation);
                    open_shot(&mut cine, &mut fade, Some(&net), from, time.elapsed_secs());
                },
            )
            .expect("runs");
    }

    /// The first shot, far: the black holds through stale and half-loaded worlds, and the fade-in
    /// comes with the destination.
    #[test]
    fn a_far_shot_start_holds_the_black_until_its_world_is_in() {
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(FAR, 60_000)]),
            // The world as the body's spot left it, resident there.
            world_at(HERE, true),
        );
        open_first_shot(&mut app);
        assert!(holding(&app), "past 30 yards the black is held");
        assert_eq!(
            app.world().resource::<Cinematic>().reloads(),
            1,
            "and the stream is asked for a new load"
        );
        assert!(black_now(&app), "and there is no fade-in");

        // The old spot's residency is not the destination's, whatever the frame count.
        for _ in 0..5 {
            frame(&mut app);
        }
        assert!(black_now(&app) && holding(&app), "the old world is not it");
        set_world(&mut app, world_at(FAR, false));
        frame(&mut app);
        assert!(black_now(&app) && holding(&app), "its tile still loading");

        set_world(&mut app, world_at(FAR, true));
        frame(&mut app);
        assert!(!holding(&app), "the destination is in");
        assert!(!black_now(&app), "and the shot fades in");
    }

    /// The arm's own frame reads a world that has not seen the new spot, even when it reports the
    /// destination's tile (a shot within one tile of the camera).
    #[test]
    fn the_arms_own_frame_does_not_release_the_hold() {
        // 100 yards along x is far, and the same tile.
        let there = from_here(100.0);
        assert_eq!(tile_of(there), tile_of(HERE));
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(there, 60_000)]),
            world_at(there, true),
        );
        open_first_shot(&mut app);
        frame(&mut app);
        assert!(
            holding(&app),
            "the stream has not looked from the new spot yet"
        );
        frame(&mut app);
        assert!(!holding(&app), "the next frame's report is about it");
        assert!(!black_now(&app));
    }

    /// The gate is `d² > 900` (`0x48eec1`: the compare falls through for a strictly larger one).
    #[test]
    fn a_shot_within_30_yards_fades_in_at_once() {
        for yards in [0.0, 12.0, 29.5, 30.0] {
            let mut app = app_with(
                Cinematic::playing_shots_for_test(vec![shot(from_here(yards), 60_000)]),
                world_at(HERE, false),
            );
            open_first_shot(&mut app);
            assert!(!holding(&app), "{yards} yd holds nothing");
            assert_eq!(
                app.world().resource::<Cinematic>().reloads(),
                0,
                "{yards} yd asks for no load"
            );
            assert!(!black_now(&app), "{yards} yd fades in as it always has");
        }
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(from_here(30.5), 60_000)]),
            world_at(HERE, false),
        );
        open_first_shot(&mut app);
        assert!(holding(&app) && black_now(&app), "30.5 yd holds");
    }

    /// The shot's clock stands at zero under the hold and runs from release.
    #[test]
    fn the_shots_clock_restarts_when_the_world_is_in() {
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(FAR, 60_000)]),
            world_at(HERE, true),
        );
        open_first_shot(&mut app);
        for _ in 0..8 {
            frame(&mut app);
        }
        let elapsed = |app: &App| {
            app.world()
                .resource::<Cinematic>()
                .playing
                .as_ref()
                .expect("playing")
                .elapsed
        };
        assert_eq!(
            elapsed(&app),
            Duration::ZERO,
            "no time passes under the black"
        );
        set_world(&mut app, world_at(FAR, true));
        frame(&mut app);
        assert!(!holding(&app));
        frame(&mut app);
        assert_eq!(elapsed(&app), Duration::from_millis(100), "then it runs");
    }

    /// A world that never loads is let through at the teleport hold's budget from the arm.
    #[test]
    fn the_ceiling_releases_a_stalled_load_and_is_never_extended() {
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(FAR, 60_000)]),
            world_at(FAR, false),
        );
        open_first_shot(&mut app);
        let frames = |secs: f32| (secs * 10.0).round() as usize;
        for i in 0..frames(SETTLE_TIMEOUT - 0.5) {
            // The stream keeps advancing, which a stall budget would count as progress.
            let mut world = world_at(FAR, false);
            world.ready = i;
            world.total = i + 1;
            set_world(&mut app, world);
            frame(&mut app);
        }
        assert!(
            holding(&app) && black_now(&app),
            "inside the budget it holds"
        );
        for _ in 0..frames(1.0) {
            frame(&mut app);
        }
        assert!(!holding(&app), "past it the hold is over");
        assert!(
            !black_now(&app),
            "and the shot fades in on the world as it is"
        );
    }

    /// A shot's advance is the same arm: the next shot, far from where the last one ended, keeps
    /// the black; a near one does not.
    #[test]
    fn a_far_shot_advance_holds_like_a_far_start() {
        let two = |next: [f32; 3]| {
            let mut app = app_with(
                Cinematic::playing_shots_for_test(vec![shot(HERE, 1_000), shot(next, 60_000)]),
                world_at(HERE, true),
            );
            // The first shot has run out, and the fade-out has reached black.
            app.world_mut()
                .resource_mut::<Cinematic>()
                .playing
                .as_mut()
                .expect("playing")
                .elapsed = Duration::from_millis(1_000);
            frame(&mut app);
            app
        };

        let mut app = two(FAR);
        assert_eq!(
            app.world()
                .resource::<Cinematic>()
                .playing
                .as_ref()
                .map(|p| p.index),
            Some(1),
            "the next shot is up"
        );
        assert!(holding(&app) && black_now(&app), "its world is not in");
        for _ in 0..5 {
            frame(&mut app);
        }
        assert!(
            holding(&app) && black_now(&app),
            "the last shot's world is not it"
        );
        set_world(&mut app, world_at(FAR, true));
        frame(&mut app);
        assert!(
            !holding(&app) && !black_now(&app),
            "its world is in, and it fades in"
        );

        let app = two(from_here(10.0));
        assert!(
            !holding(&app) && !black_now(&app),
            "a near shot fades in at once"
        );
    }

    /// The reference sends the shot's `CMSG_NEXT_CINEMATIC_CAMERA` after the synchronous load
    /// (`0x48ef11` follows `0x48eed3`), so a far start sends it with the world, a near one at once.
    #[test]
    fn a_far_start_announces_its_shot_when_the_world_is_in() {
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(FAR, 60_000)]),
            world_at(HERE, true),
        );
        open_first_shot(&mut app);
        for _ in 0..5 {
            frame(&mut app);
        }
        assert_eq!(announced(&app), 0, "nothing goes out while it loads");
        set_world(&mut app, world_at(FAR, true));
        frame(&mut app);
        assert!(!holding(&app));
        assert_eq!(announced(&app), 1, "it goes out with the fade-in");
        frame(&mut app);
        assert_eq!(announced(&app), 0, "and only once");

        let mut near = app_with(
            Cinematic::playing_shots_for_test(vec![shot(from_here(10.0), 60_000)]),
            world_at(HERE, true),
        );
        open_first_shot(&mut near);
        assert_eq!(announced(&near), 1, "a near shot announces at the arm");
    }

    /// The ceiling's release shows the shot, so it announces it too.
    #[test]
    fn a_stalled_far_start_announces_its_shot_at_the_ceiling() {
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(FAR, 60_000)]),
            world_at(FAR, false),
        );
        open_first_shot(&mut app);
        for _ in 0..((SETTLE_TIMEOUT * 10.0) as usize + 2) {
            frame(&mut app);
        }
        assert!(!holding(&app));
        assert_eq!(announced(&app), 1);
    }

    /// An advance is the same arm: one announce for the shot, with its world if it is far.
    #[test]
    fn a_far_advance_announces_its_shot_when_the_world_is_in() {
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(HERE, 1_000), shot(FAR, 60_000)]),
            world_at(HERE, true),
        );
        app.world_mut()
            .resource_mut::<Cinematic>()
            .playing
            .as_mut()
            .expect("playing")
            .elapsed = Duration::from_millis(1_000);
        frame(&mut app);
        for _ in 0..3 {
            frame(&mut app);
        }
        assert!(holding(&app));
        assert_eq!(announced(&app), 0, "not while it loads");
        set_world(&mut app, world_at(FAR, true));
        frame(&mut app);
        assert_eq!(announced(&app), 1, "once, with the world");
    }

    /// A hitch that runs through two shots owes each its announce: the one passed through at the
    /// black, and the one shown.
    #[test]
    fn a_hitch_through_two_shots_announces_both() {
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![
                shot(HERE, 1_000),
                shot(HERE, 1_000),
                shot(from_here(5.0), 60_000),
            ]),
            world_at(HERE, true),
        );
        app.world_mut()
            .resource_mut::<Cinematic>()
            .playing
            .as_mut()
            .expect("playing")
            .elapsed = Duration::from_millis(2_000);
        frame(&mut app);
        assert_eq!(
            app.world()
                .resource::<Cinematic>()
                .playing_shot()
                .map(|s| s.1),
            Some(2)
        );
        assert_eq!(announced(&app), 2);
    }

    /// The shot's narration starts after the load (`0x48ef2e` follows `0x48eed3`): the shot is not
    /// the one on screen, as the voice follows it, until its hold ends. A far first shot, as the
    /// Tauren intro's, and a far advance alike; the voice of the shot left is cut at the black.
    #[test]
    fn a_held_shot_is_not_narrated_until_its_world_is_in() {
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(FAR, 60_000)]),
            world_at(HERE, true),
        );
        let on_screen = |app: &App| app.world().resource::<Cinematic>().playing_shot();
        assert!(on_screen(&app).is_some(), "before the arm it is the shot");
        open_first_shot(&mut app);
        for _ in 0..4 {
            frame(&mut app);
        }
        assert!(holding(&app));
        assert_eq!(on_screen(&app), None, "no voice under the load");
        set_world(&mut app, world_at(FAR, true));
        frame(&mut app);
        assert_eq!(
            on_screen(&app).map(|s| s.1),
            Some(0),
            "the voice starts with it"
        );

        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(HERE, 1_000), shot(FAR, 60_000)]),
            world_at(HERE, true),
        );
        app.world_mut()
            .resource_mut::<Cinematic>()
            .playing
            .as_mut()
            .expect("playing")
            .elapsed = Duration::from_millis(1_000);
        frame(&mut app);
        assert!(holding(&app));
        assert_eq!(
            on_screen(&app),
            None,
            "the last shot's voice is cut, the next waits"
        );
        set_world(&mut app, world_at(FAR, true));
        frame(&mut app);
        assert_eq!(on_screen(&app).map(|s| s.1), Some(1));
    }

    /// EndCinematic (`0x48f080`) reloads the body's spot when the camera ended more than 30 yards
    /// from the followed object (`0x48f0fc` to `0x48f118`), so the end asks the stream for a new
    /// load behind the same gate; counts latched under the camera say nothing of the body's.
    #[test]
    fn the_end_asks_for_a_load_only_when_the_camera_is_far_from_the_body() {
        let ended_with_body_at = |body: [f32; 3]| {
            let mut app = app_with(
                Cinematic::playing_shots_for_test(vec![shot(HERE, 60_000)]),
                world_at(HERE, true),
            );
            let mut player = crate::player::Player::default();
            player.active = true;
            player.pos = benilla_assets::coords::wow_to_bevy(body);
            app.insert_resource(player);
            app.world_mut().resource_mut::<Cinematic>().skip = true;
            frame(&mut app);
            let cine = app.world().resource::<Cinematic>();
            assert!(!cine.is_playing() && cine.settling.is_some(), "it ended");
            cine.reloads()
        };
        assert_eq!(
            ended_with_body_at(FAR),
            1,
            "the camera is far from the body"
        );
        assert_eq!(ended_with_body_at(from_here(29.5)), 0, "29.5 yd");
        assert_eq!(ended_with_body_at(from_here(30.5)), 1, "30.5 yd");
    }

    /// ESC under a far shot's black ends the cinematic, and the hold goes with it.
    #[test]
    fn a_skip_under_the_hold_ends_it() {
        let mut app = app_with(
            Cinematic::playing_shots_for_test(vec![shot(FAR, 60_000)]),
            world_at(HERE, true),
        );
        open_first_shot(&mut app);
        frame(&mut app);
        assert!(holding(&app));
        app.world_mut().resource_mut::<Cinematic>().skip = true;
        frame(&mut app);
        let cine = app.world().resource::<Cinematic>();
        assert!(!cine.is_playing() && !cine.holds_black_for_shot());
        assert!(cine.settling.is_some(), "and the end holds its own black");
    }
}
