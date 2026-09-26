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
//!   arm (`0x48edf0`, sent at `0x48ef11`). A camera id of 0 ends the cinematic (`0x48efe0`).
//! - ESC is no engine binding: `StopCinematic` has no native caller, and the only skip is
//!   `CinematicFrame.xml`'s `OnKeyDown`, whose Lua queues
//!   [`SessionRequest::StopCinematic`](benilla_ui::script::SessionRequest).
//! - `CMSG_COMPLETE_CINEMATIC` goes out once, on a natural end and on a skip alike (`0x48f080`).
//!   Unacked, vmangos keeps object visibility on its own copy of the flying camera
//!   (`Player.cpp:1448`), and everything around the body stays despawned until relog.
//! - Every boundary (start, shot advance, end) cuts through black: a 0.25 s fade-out
//!   (`[0x804550]`, `0x4c0d10`) whose completion runs the step, then a fade-in (`0x4c1280`). There
//!   is no audio fade on this path.
//!
//! Playback takes over, and releases, the camera pose, the streaming focus (which follows the
//! camera, so the flight is over streamed terrain) and the UI's cinematic flag, which drives
//! `CinematicFrame` and makes `InCinematic()` true so `StaticPopup` suppresses dialogs.

use std::time::Duration;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::{CinematicCatalog, CinematicPath};
use benilla_ui::script::UiScript;
use benilla_world::schedule::WorldStage;
use benilla_world::view::WorldCamera;
use bevy::prelude::*;

use crate::char_select::ClientState;
use crate::loading_screen::LoadingScreen;
use crate::net::{CinematicTriggeredMessage, ClientCommand, NetCommands};
use crate::player::PlayerControlSet;

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
    /// Is a cinematic on screen right now? The engine half of `InCinematic()`.
    pub(crate) fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// The shot on screen: `(run, shot index, narration sound id)`, keyed on [`Playing::run`] so a
    /// re-trigger counts as a new shot.
    pub(crate) fn playing_shot(&self) -> Option<(u64, usize, u32)> {
        let play = self.playing.as_ref()?;
        Some((play.run, play.index, play.shot().sound_id))
    }
}

#[cfg(test)]
impl Cinematic {
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
        }
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
                (take_trigger, start_pending, drive)
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
            // (`0x490a80` clears the flag and sends no `CMSG_COMPLETE_CINEMATIC`).
            .add_systems(OnExit(ClientState::InWorld), abandon_on_leaving_world);
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
    // teardown (`0x58b590(0)`/`(1)`); nothing else hides it while the view is detached. Written
    // only on a real change: `bevy_winit` re-applies `CursorOptions` to AppKit on every change,
    // which can stall the main thread.
    if let Ok(mut opts) = cursor.single_mut() {
        if playing && opts.visible {
            opts.visible = false;
            ours.cursor = true;
        } else if !playing && ours.cursor {
            ours.cursor = false;
            if !opts.visible {
                opts.visible = true;
            }
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
    static RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    cine.playing = Some(Playing {
        run: RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        sequence_id: id,
        shots,
        index: 0,
        elapsed: Duration::ZERO,
    });
    announce_shot(net.as_deref());
    // `0x48ef43`, the tail of the shot arm: the shot fades in from the black it was armed under.
    fade.fade_in(crate::screen_fade::CINEMATIC);
}

/// `CMSG_NEXT_CINEMATIC_CAMERA`, sent as each shot is armed, the first included (`0x48ef11`).
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
    let Some(play) = cine.playing.as_mut() else {
        return;
    };
    play.elapsed += time.delta();

    // --- Every boundary cuts through black: a run-out shot or a skip fades out and steps at full
    // black, as the fade's completion (`0x48efd7` to `0x48efe0` to advance, `0x48f067` to
    // `0x48f080` to stop), which keeps the cut invisible under a hitch.
    if skip || play.elapsed.as_millis() as u32 >= play.shot().duration_ms {
        if !fade.is_black() {
            fade.fade_out(crate::screen_fade::CINEMATIC);
            // Fall through to seat the camera: what darkens is the shot's own last pose.
        } else {
            // Walk past every shot this frame's delta ran through, each owed its
            // `CMSG_NEXT_CINEMATIC_CAMERA`, inside one black.
            let mut ended = skip;
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
                announce_shot(net.as_deref());
            }
            if ended {
                cine.skip = false;
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
                // hold the black the reference shows while its own load blocks.
                cine.settling = Some(SETTLE_BUDGET);
                return;
            }
            // `0x48ef43`: the next shot comes in from the black it was armed under.
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
pub(crate) fn stop(cine: &mut Cinematic, net: Option<&NetCommands>) {
    if cine.playing.is_some() {
        // Something is on screen: [`drive`] pays at full black, as `StopCinematic` (`0x48f050`)
        // arms a 0.25 s fade-out with EndCinematic (`0x48f080`) as its completion.
        cine.skip = true;
        return;
    }
    // A latch still waiting on the world goes too, paid now: no [`drive`] pass will settle it.
    relinquish(cine, net, "stopped");
}

/// Leaving the world drops a cinematic without acking; the socket goes with it.
fn abandon_on_leaving_world(
    mut cine: ResMut<Cinematic>,
    mut fade: ResMut<crate::screen_fade::ScreenFade>,
) {
    if let Some(play) = cine.playing.take() {
        info!("cinematic: {} abandoned (left the world)", play.sequence_id);
    }
    cine.pending = None;
    cine.skip = false;
    cine.settling = None;
    // Clear any fade: the glue comes up next and must not sit behind our black.
    fade.clear();
}

/// Fire the `CINEMATIC_START`/`CINEMATIC_STOP` edges and keep `InCinematic()` current. The memo
/// is per VM, so a `/reload` mid-cinematic re-fires `CINEMATIC_START` into the rebuilt tree.
fn feed_ui(
    cine: Res<Cinematic>,
    script: Option<NonSendMut<UiScript>>,
    mut published: Local<crate::ui_script::VmMemo<Option<bool>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let playing = cine.is_playing();
    // VM-scoped: a plain `Local` would leave ESC dead after a mid-cinematic `/reload`.
    let published = published.get(&script);
    if *published == Some(playing) {
        return;
    }
    // A fresh VM is already out of a cinematic: seed the memo rather than fire a `CINEMATIC_STOP`
    // the reference never sends unedged.
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
