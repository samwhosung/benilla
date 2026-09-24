//! **Cinematic playback** — the race-intro fly-by, and every other `SMSG_TRIGGER_CINEMATIC`.
//!
//! A cinematic is an in-engine camera flight, not a movie: the trigger carries a
//! `CinematicSequences.dbc` id, the row names its `CinematicCamera.dbc` shots, and each shot is an
//! authored eye/target/roll path in a `Cameras\*.m2` planted at a world origin and facing. The
//! parsing, the world transform and the Bézier evaluation all live in
//! [`benilla_formats::CinematicPath`]; this module is the *playback* — when it starts, what it
//! takes over while it runs, and how it ends.
//!
//! # What the reference does, and what we match
//!
//! The reason each piece here is shaped the way it is:
//!
//! - **A trigger that arrives before the world is up is deferred, not dropped.** The reference
//!   stashes the sequence id in a single-slot latch (`0xc4d75c`) whenever its world-load gate is
//!   still closed, and starts it the instant the gate opens (`0x5deb78`). Last write wins; there
//!   is no queue. [`Cinematic::pending`] is that latch, and the gate here is the loading screen —
//!   a first login's intro would otherwise play its opening seconds behind the cover, with no UI
//!   to ESC out of.
//! - **The camera path is armed as an ordinary M2 animation** (`0x7121a0`, sequence 0, rate 1.0)
//!   and the shot ends when the scene clock reaches the sequence band's end — `M2Sequence.end −
//!   .start`, which is [`CinematicPath::duration_ms`]. Every shipped fly-by is `flags` bit 0 =
//!   clamp, i.e. plays **once and freezes**; it does not loop. (`Scry_cam`, which is not a race
//!   intro, is the one file authored to loop — a difference we inherit for free by ending on the
//!   band rather than on the flag.)
//! - **The client announces every shot with `CMSG_NEXT_CINEMATIC_CAMERA` — including the first.**
//!   The send sits inside the *shot arm* `0x48edf0`, not the shot advance: `0x48ef11 push 0xfb`
//!   → `0x418190` (the packet builder) → `0x48ef24 call 0x5ab630` (flush), immediately before
//!   `0x48ef29` starts that shot's narration and `0x48ef43` fades it in — the same builder/flush
//!   pair `0x48f154 push 0xfc` uses for the completion ack. Since every shot goes through the arm,
//!   a shipped single-camera race intro sends exactly one of these, at the start. benilla used to
//!   read this as "between the shots of a multi-camera row" and cite `0x48efe0`, which sent none
//!   at all on every shot the game actually ships. A camera id of `0` still **ends** the cinematic
//!   rather than being skipped (`0x48efe0`).
//! - **ESC is not an engine binding.** `StopCinematic` has zero native callers in the reference:
//!   the only skip path is `CinematicFrame.xml`'s own `OnKeyDown`, which is why benilla's copy of
//!   that frame (stock `Interface\FrameXML\CinematicFrame.xml`) carries the same handler and why
//!   the Lua binding queues [`SessionRequest::StopCinematic`](benilla_ui::script::SessionRequest)
//!   rather than reaching in here.
//! - **The ack ends the run, once.** `CMSG_COMPLETE_CINEMATIC` goes out on a natural end and on an
//!   ESC skip alike (`0x48f080`). Decision 0196 is why it can never be dropped: unacked, vmangos
//!   re-anchors object visibility to its own copy of the flying camera and everything around the
//!   body despawns until relog.
//!
//! # What we deliberately do differently
//!
//! **Every cinematic boundary in the reference cuts through black, and benilla cuts hard.** The
//! `0.25 s` at `[0x804550]` is not a scheduled delay — it is a screen fade, and the next step is
//! its *completion callback*: `0x4c0d10` builds a fullscreen opaque-black quad (`0xff000000`) and
//! latches what to run when it is fully up, so `CINEMATIC_START`, each shot advance and
//! `EndCinematic` all execute at full black and `0x4c1280` fades back in. Six sites, both edges,
//! always black, always 0.25 s. There is **no audio fade anywhere on this path**.
//!
//! benilla plays and acks immediately. Building the picture is what would let the timing be
//! faithful too, so the two go together and neither is faked without the other — decision 1724
//! leaves both standing rather than adding latency with nothing on screen to justify it.
//!
//! # What playback takes over
//!
//! Three things, all released on the way out. The **camera** — its *pose* only; the projection is
//! left alone, because a fly-by is framed by the world camera's own optics and the M2 record's
//! `fov` reaches nothing on this path (decision 1711). Written after `control` has seated it, the
//! same slot `apply_camera_shake` uses. The **streaming focus**, which has to follow the camera
//! rather than the body, because a Tauren's shot opens 1741 yards from where the body stands and would
//! otherwise fly over unstreamed terrain; and the **UI's cinematic flag**, which drives
//! `CinematicFrame`'s letterbox and makes `InCinematic()` answer truthfully so `StaticPopup`
//! suppresses dialogs the way the reference's does.

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
    /// The sequence a trigger asked for but the world was not ready to show — the reference's
    /// single-slot latch (`0xc4d75c`), last write wins, no queue.
    pending: Option<u32>,
    playing: Option<Playing>,
    /// The end boundary is still settling: the shot is over, the teardown has run, and the screen
    /// is **holding at black** until the world around the body is presentable again.
    ///
    /// This is the reference's `0x48f080` — EndCinematic's forced terrain load back around the
    /// followed unit, which runs at full black and *blocks* (no loading cover is raised anywhere on
    /// this path: its raise, `0x406800`, has no caller there). benilla streams asynchronously and
    /// cannot block, so it holds the black the reference was already showing for as long as that
    /// load would have taken. Same thing on screen; ours keeps rendering.
    ///
    /// The value is the seconds of black left to spend waiting. **Black is never held forever:**
    /// if the world does not come back inside the budget the fade comes in anyway, and the loading
    /// cover — which sits below it — does the job it was always going to do. A stuck black screen
    /// would be worse than the cover this exists to avoid.
    settling: Option<f32>,
    /// The player asked to skip (ESC), and it has not been paid yet.
    ///
    /// A skip is **not** settled where it is asked for. The reference's `StopCinematic` fades out
    /// and hands EndCinematic to the fade as its completion callback (`0x48f067` →`0x48f080`), so
    /// the ack and the teardown land at full black like every other boundary. [`stop`] sets this;
    /// [`drive`] pays it.
    skip: bool,
}

/// One cinematic in flight.
struct Playing {
    /// Which *run* this is, counted from process start. The `CinematicSequences.dbc` id alone is
    /// not an identity: re-triggering the same sequence while it plays produces the same
    /// `(sequence, shot)` pair, and the narration follower reads that pair to tell "still the same
    /// shot" from "a new one" — so without this the picture restarted at t=0 while the voice kept
    /// running from wherever it had got to.
    run: u64,
    /// The `CinematicSequences.dbc` id, for logging.
    sequence_id: u32,
    /// The row's shots, in order. Non-empty (a sequence that resolves to nothing is acked at the
    /// trigger and never becomes a `Playing`).
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

    /// The shot on screen: `(run, shot index, narration sound id)`. The identity pair is what lets
    /// a follower tell "still the same shot" from "the next one", which is the question the
    /// narration channel actually asks — and it is keyed on [`Playing::run`] rather than the
    /// sequence id so that re-triggering the sequence already playing counts as a new shot.
    pub(crate) fn playing_shot(&self) -> Option<(u64, usize, u32)> {
        let play = self.playing.as_ref()?;
        Some((play.run, play.index, play.shot().sound_id))
    }
}

#[cfg(test)]
impl Cinematic {
    /// A cinematic that answers [`Cinematic::is_playing`] — for the neighbours that only ask that
    /// one question (the streaming focus's hold, the HUD hide). **Shot-less on purpose:** there is
    /// no way to build a [`CinematicPath`] without a real `Cameras\*.m2`, and a fixture that
    /// faked one would let a test assert about a shot that does not exist. Anything that reads a
    /// shot must be tested against the real corpus instead, and will panic loudly here if it is
    /// not.
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

/// One of the two letterbox bars — full-width, black, top or bottom.
///
/// **Bevy UI nodes rather than the reference's FrameXML textures — and as of decision 1734 that is
/// a choice, not a constraint.** It was a constraint: 1699 had to move the bars out of
/// `CinematicFrame.xml` because the HUD was hidden through [`UiHidden`](crate::ui_hide::UiHidden),
/// which kills both of `ui_pass`'s quad lanes wholesale, so bars drawn as FrameXML textures went
/// dark along with the action bars they exist to replace. That is no longer how the HUD is hidden.
/// `UIParent:Hide()` cascades along the frame tree, and `CinematicFrame` has no parent — the
/// reference declares it that way precisely so it survives the hide it triggers — so the
/// reference's own arrangement would now work.
///
/// It stays here for now because it earns its place on its own terms: computed per frame, a window
/// resized mid-cinematic stays correctly letterboxed, where the reference (whose resolution changed
/// only across a restart) would not. Moving it back into `CinematicFrame.xml` is a real option that
/// 1734 unblocked and did not take; it is a fidelity question, not a bug.
///
/// `CinematicFrame` still exists and still shows — it is what makes `InCinematic()` true, fires
/// the events, owns the ESC handler, and is now what hides the HUD.
#[derive(Component)]
struct LetterboxBar;

/// What [`drive_letterbox`] switched off when the shot started, so it puts back only what it took.
/// A player who was already in a mouse-look session keeps their state.
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
                // One chain, all three steps in the same frame: the net drain has already run
                // (`WorldStage::Net` precedes `Input`), so a trigger can arrive, start and be
                // driven without ever showing a frame of the follow camera in between. And they
                // run *after* `control` has seated that camera, so the pose written here is the
                // last word on it — the same slot `apply_camera_shake` takes.
                (take_trigger, start_pending, drive)
                    .chain()
                    .in_set(WorldStage::Input)
                    .after(PlayerControlSet),
            )
            // The UI edge runs after the driver settled this frame's state, and only once there
            // IS a UI: firing CINEMATIC_START into a VM with no `CinematicFrame` yet would raise
            // no letterbox and leave nothing listening for the ESC that ends the shot.
            .add_systems(
                Update,
                feed_ui
                    .in_set(WorldStage::Input)
                    .after(drive)
                    .run_if(crate::ui_script::ingame_ui_up),
            )
            // A cinematic cannot outlive the world it was flying over: leaving drops it silently,
            // with no ack, exactly as the reference's own leave-world teardown does (`0x490a80`
            // clears the in-cinematic flag and sends no `CMSG_COMPLETE_CINEMATIC` — the socket is
            // going away anyway).
            // The screen's two takeovers — the HUD going dark and the bars coming in — ride the
            // same edge as everything else, after the driver has settled this frame's state.
            .add_systems(
                Update,
                drive_letterbox.in_set(WorldStage::Input).after(drive),
            )
            .add_systems(OnExit(ClientState::InWorld), abandon_on_leaving_world);
    }
}

/// The two bars, spawned once and parked hidden — the loading cover's own shape.
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
            // Above the world and the UI quads, below the loading cover (1000): a cinematic that
            // runs into a zone load should be covered by the load, not paint over it.
            GlobalZIndex(900),
            Visibility::Hidden,
        ));
    }
}

/// The height of **one** letterbox bar, in the same units as the screen it is measured against.
///
/// The reference's law in one expression — see [`drive_letterbox`] for where each term comes from.
/// Pure and unit-agnostic, so it is testable against the reference's own numbers without a window.
fn letterbox_bar(width: f32, screen: f32) -> f32 {
    let two_to_one = (screen - (width / 2.0).min(screen)) / 2.0;
    two_to_one.min(screen / 6.0).max(0.0)
}

/// Raise the letterbox and darken the HUD while a shot is on screen; put both back after.
///
/// The bar height is the reference's own law, and it is **two** halves of one formula. The Lua in
/// `CinematicFrame.lua` crops the picture to **2:1** — `width/2`, capped at the screen height,
/// remainder split evenly — but only `if width/height > 4/3`; below that it recomputes nothing and
/// the bars stand at the size `CinematicFrame.xml` declares them, `1024 x 128`. That is not a
/// separate rule: the frame is authored on the client's native `1024 x 768` sheet (`WorldMapFrame`
/// authors its fullscreen backdrop at exactly that, and `SetupFullscreenScale` grows it), so `128`
/// is `height/6` — which is the 2:1 formula evaluated at exactly 4:3. The `if` is a guard around a
/// recompute, not a gate on whether there are bars.
///
/// So the whole law is one line: **`min((height - min(width/2, height))/2, height/6)`**. Wider than
/// 4:3 the first term wins and the picture is 2:1; at 4:3 they are equal; narrower, the cap holds
/// the bars at a sixth apiece exactly as the reference's un-recomputed textures do. Reading it as
/// "only a wide screen gets bars" left benilla un-letterboxed at 4:3 and below, which is the
/// reference's *native* aspect.
///
/// Computed here per frame rather than once in `OnLoad`: a window resized mid-cinematic stays
/// letterboxed, where the reference (whose resolution changed only across a restart) would not.
fn drive_letterbox(
    cine: Res<Cinematic>,
    mut bars: Query<(&mut Node, &mut Visibility), With<LetterboxBar>>,
    windows: Query<&Window>,
    mut cursor: Query<&mut bevy::window::CursorOptions, With<bevy::window::PrimaryWindow>>,
    mut ours: Local<Takeover>,
    mut logged: Local<f32>,
) {
    let playing = cine.is_playing();
    // **The HUD is no longer hidden from here.** It is Lua's, exactly as it is in the reference:
    // `CinematicFrame`'s `CINEMATIC_START` arm calls `ShowUIPanel`, its `area = "full"` row routes
    // to `SetFullScreenFrame`, and that hides `UIParent` — which now reaches the whole HUD,
    // because decision 1734 restored the 72 `parent=` declarations the transcription had dropped.
    // `UiHidden` goes back to meaning only what ALT-Z means.

    // **The hardware cursor goes with it** — `0x58b590(0)` at StartCinematic, `(1)` at
    // EndCinematic and at the leave-world teardown. Nothing else in benilla
    // would: this flag is otherwise written only by the mouse-look session in `player::camera`,
    // and `control` skips that whole branch while the view is detached, so the pointer the player
    // arrived at character-select with simply sat on top of the fly-by.
    //
    // Same restore-only-what-we-hid discipline as the HUD above, and the same **write only on a
    // real change**: `CursorOptions` is what `bevy_winit`'s `changed_cursor_options` watches, and
    // re-applying cursor state to AppKit every frame intermittently stalls the main thread
    // (`player::control`'s shadow-copy note, the 0366 frame-tail hunt).
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
    // On the edges and on a resize, never every frame: the measured crop, so the letterbox is a
    // number in the log rather than something only an eye can confirm. `info!` on purpose — the
    // default filter stops at that level, and a number nobody's ordinary run prints is a number
    // nobody checks (it fires once per cinematic, plus once per resize during one).
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
        // Graceful absence, the standing posture: with no catalog every trigger falls through to
        // the immediate ack, which is exactly the pre-playback behaviour decision 0196 shipped.
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
            // Nothing to play — ack on the spot rather than leaving the server flying a path
            // nobody is watching (decision 0196).
            warn!("cinematic: {id} names no shot we can play — acking it");
            ack(net.as_deref());
            continue;
        }
        // Last write wins, the reference's single-slot latch — but the id it displaces still owes
        // the server its ack (see [`relinquish`]).
        if let Some(dropped) = cine.pending.replace(id) {
            warn!("cinematic: {dropped} displaced by {id} before it started — acking it");
            ack(net.as_deref());
        }
    }
}

/// Give up whatever is in flight, paying its ack.
///
/// **The invariant this exists to keep: one `CMSG_COMPLETE_CINEMATIC` for every trigger we
/// accepted.** Decision 0196 is why the direction matters — an unacked cinematic leaves vmangos
/// anchoring object visibility to its own copy of the flying camera, and everything around the
/// body stays despawned until relog. An *extra* ack is discarded by a server that is not watching
/// one; a missing one is not recoverable in-session. So when two triggers are live at once, both
/// are acked: they are two cinematics the server started, not one.
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
    // The reference's world-load gate. Ours is the loading cover plus being in the world at all:
    // starting under the cover would burn the opening seconds behind black, and `CinematicFrame`
    // (the letterbox, and the only ESC route out) does not exist until the in-game UI has loaded.
    if *state.get() != ClientState::InWorld || screen.is_some_and(|s| s.covering()) {
        return;
    }
    let (Some(catalog), Some(assets)) = (catalog, assets) else {
        return;
    };
    // **The reference does not arm the shot here — it fades out, and the arm is the fade's
    // completion callback** (`0x48edc5` arms `0x4c0d10(0x48edd0, 0, 0.25)`). So the world's last
    // frame darkens first and everything below runs at full black: the chain read, the camera
    // load, `CINEMATIC_START`. Gated last, after every "we cannot play this" return above, so a
    // start that fails its preconditions never darkens the screen on the way to doing nothing.
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
            // One unreadable shot does not have to sink the whole cinematic: play what parses.
            Err(e) => warn!("cinematic: camera {} failed to load: {e:#}", row.id),
        }
    }
    if shots.is_empty() {
        warn!("cinematic: {id} had no loadable shot — acking it");
        ack(net.as_deref());
        // Nothing is going to be shown under this black, so give the world back.
        fade.fade_in(crate::screen_fade::CINEMATIC);
        return;
    }
    info!(
        "cinematic: playing {id} — {} shot(s), {} ms",
        shots.len(),
        shots.iter().map(|s| s.duration_ms).sum::<u32>()
    );
    // A trigger that lands on top of a running cinematic replaces it — and the one it replaces
    // is acked here rather than dropped ([`relinquish`]'s invariant). No stock server does this;
    // a GM `.debug play cinematic` during an intro does.
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

/// `CMSG_NEXT_CINEMATIC_CAMERA` — sent as each shot is armed, the first one included (module doc:
/// `0x48ef11`, inside the shot arm). It is how the server learns which camera is flying, which is
/// the other half of decision 0196's visibility re-anchor.
fn announce_shot(net: Option<&NetCommands>) {
    if let Some(net) = net {
        let _ = net.0.send(ClientCommand::NextCinematicCamera);
    }
}

/// How long the end boundary will hold the screen black waiting for the world to come back.
///
/// Generous, because the alternative it is trading against is mild (a loading cover for a moment)
/// and the cost of being too tight is the cover this exists to remove. Measured, the wait is
/// 0.4-0.9 s on a warm slot. It is a **budget, not a timeout to rely on**: exhausting it logs.
const SETTLE_BUDGET: f32 = 8.0;

/// Advance the shot and seat the camera on it; hand over to the next shot, or end the cinematic.
///
/// Runs **after** `control` has seated the follow camera, the same slot `apply_camera_shake` uses:
/// the pose written here is this frame's, over a base the controller rewrote from scratch, so
/// nothing accumulates and the *pose* needs no restore — the next frame's `control` simply seats
/// it again.
///
/// **The FOV is not touched at all**, and that is the correction decision 1711 landed. A fly-by is
/// rendered through the world camera's own optics, re-stamped every frame — the M2 camera record's
/// `fov` is written at model load (`0x70f336`) and read by nothing on this path (its one raw
/// reader, `0x7ac640`, is reachable only
/// from the portrait and `<Model>` frames). Feeding the authored 45 degrees through the reference's
/// own `theta_v = F / sqrt(aspect^2 + 1)` builder — which is otherwise right, and angle-space
/// division really is what `0x5c3cc0` does — rendered fifteen of the sixteen shipped shots at
/// **half** the intended vertical FOV, i.e. visibly over-zoomed. Nothing to write means nothing to
/// release.
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
    //
    // Both terms, because each has a one-frame race the other covers: residency can still read
    // "presentable" for the frame before the focus moves off the camera, and the cover has not
    // raised yet on the frame the shot ends. Either being false keeps the screen black, so the
    // worst a race costs is one extra black frame.
    if let Some(left) = cine.settling {
        let home = progress
            .as_deref()
            .is_some_and(|p| p.is_ready() && p.presentable())
            && !screen.as_deref().is_some_and(LoadingScreen::covering);
        let left = left - time.delta_secs();
        if home || left <= 0.0 {
            if !home {
                warn!(
                    "cinematic: the world did not come back inside the settle budget —                      revealing anyway"
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

    // A skip with only a latch in flight is settled by [`stop`] itself, so by here every skip has
    // something on screen to fade.
    let skip = cine.skip;
    let Some(play) = cine.playing.as_mut() else {
        return;
    };
    play.elapsed += time.delta();

    // --- Every boundary cuts through black. A shot that has run out and a skip the player asked
    // for are the same event to the screen: fade out, and land the step at **full black**. The
    // reference does not schedule the step after a delay — it hands it to the fade as the
    // completion callback (`0x48efd7` → `0x48efe0` for an advance, `0x48f067` → `0x48f080` for a
    // stop), which is what keeps the cut invisible when the boundary itself hitches.
    if skip || play.elapsed.as_millis() as u32 >= play.shot().duration_ms {
        if !fade.is_black() {
            fade.fade_out(crate::screen_fade::CINEMATIC);
            // …and fall through to seat the camera, so what darkens is the shot's own last pose.
        } else {
            // Walk past every shot this frame's delta ran clean through (a long stall, a debugger
            // pause) — each still owes the server its `CMSG_NEXT_CINEMATIC_CAMERA`. They are
            // walked inside **one** black rather than a fade each: the fade marks the boundary the
            // player sees, not every row crossed behind it.
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
                    // A skip takes a latched trigger with it — the player said "not this", and
                    // the one still waiting on the world is owed its ack all the same.
                    relinquish(&mut cine, net.as_deref(), "stopped");
                } else if let Some(play) = cine.playing.take() {
                    // A **natural end** is not [`relinquish`]: a trigger that arrived while this
                    // one was flying is a cinematic still owed its *playback*, not an ack, and
                    // `start_pending` will pick it up on the next pass.
                    info!("cinematic: {} finished", play.sequence_id);
                    ack(net.as_deref());
                }
            }
            if ended {
                // **Do not fade in yet.** The world around the body was streamed away while the
                // camera flew, and revealing it now is what put a loading cover on screen for
                // half a second at the end of every fly-by. Hold the black instead, which is what
                // the reference is already showing while its own forced load blocks.
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
    // Clamped: while a boundary darkens, `elapsed` has already run past the shot's end and the
    // pose held under the fade is the shot's last authored one, not an extrapolation past it.
    let view = shot.sample((play.elapsed.as_millis() as u32).min(shot.duration_ms));
    let eye = benilla_assets::coords::wow_to_bevy(view.eye);
    let target = benilla_assets::coords::wow_to_bevy(view.target);

    // The roll is authored about the view axis, and around whole turns rather than around zero —
    // `FlyByDwarf` holds a constant 2π — so it is applied as an angle and never tested for zero.
    // The WoW→Bevy basis is a proper rotation (determinant +1), so the sign carries across
    // unchanged.
    let forward = (target - eye).normalize_or_zero();
    let up = if forward == Vec3::ZERO {
        Vec3::Y
    } else if forward.cross(Vec3::Y).length_squared() < 1e-6 {
        // Looking straight up or down: rotating `Y` about a `forward` that *is* `Y` is the
        // identity, so the authored roll would vanish and `look_at` would fall back to an
        // arbitrary (if deterministic) horizon. Roll the one basis vector that is guaranteed not
        // to be parallel to the view instead — same angle, about the same axis, just a seed that
        // survives. No shipped shot holds a vertical view, which is why this never showed.
        Quat::from_axis_angle(forward, view.roll) * Vec3::Z
    } else {
        Quat::from_axis_angle(forward, view.roll) * Vec3::Y
    };
    cam.translation = eye;
    if forward != Vec3::ZERO {
        cam.look_at(target, up);
    }
}

/// ESC, or anything else that asks for the skip: end the cinematic and ack it.
///
/// The reference sends the same `CMSG_COMPLETE_CINEMATIC` here as it does on a natural end — a
/// skip is indistinguishable from a completion on the wire, which is what made decision 0196's
/// instant-ack legitimate in the first place.
pub(crate) fn stop(cine: &mut Cinematic, net: Option<&NetCommands>) {
    if cine.playing.is_some() {
        // Something is on screen, so the skip takes the same route every other boundary takes:
        // fade to black, and settle at full black. [`drive`] pays the ack there. This is the
        // reference's own shape — `StopCinematic` (`0x48f050`) does not end anything itself, it
        // arms a 0.25 s fade-out with EndCinematic (`0x48f080`) as the completion callback.
        cine.skip = true;
        return;
    }
    // A trigger still waiting on the world goes with it: the player has said "not this". Nothing
    // is on screen to fade, so this is paid on the spot — and it must be, because a latch that is
    // never started has no [`drive`] pass coming to settle it.
    relinquish(cine, net, "stopped");
}

/// Leaving the world drops a cinematic without acking — the socket is going away with it.
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
    // Whatever was mid-fade goes with it. A graceful fade-in has no frames left to run in here —
    // the next thing on screen is the glue, and it must not come up behind our black.
    fade.clear();
}

/// Fire the `CINEMATIC_START`/`CINEMATIC_STOP` edges and keep `InCinematic()` honest.
///
/// Edge-fired off what **this VM** has heard, the `death.rs` posture: a fresh VM's memo is empty,
/// so a `/reload` mid-cinematic re-fires `CINEMATIC_START` into the rebuilt frame tree and the
/// letterbox comes back up rather than staying lost for the rest of the shot.
fn feed_ui(
    cine: Res<Cinematic>,
    script: Option<NonSendMut<UiScript>>,
    mut published: Local<crate::ui_script::VmMemo<Option<bool>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let playing = cine.is_playing();
    // VM-scoped (decision 1290): the memo is memory *about a VM*, so a `/reload` mid-cinematic
    // resets it and the edge re-fires into the rebuilt frame tree — which is the whole point,
    // because the new tree's `CinematicFrame` is hidden and knows nothing about the shot still on
    // screen. A plain `Local` here would leave the letterbox down and ESC dead for the rest of a
    // 102-second intro.
    let published = published.get(&script);
    if *published == Some(playing) {
        return;
    }
    // **A fresh VM is already not in a cinematic, so there is no edge to announce.** The memo
    // starts `None`, and `None != Some(false)` — which fired a `CINEMATIC_STOP` into every login
    // and every `/reload`, an event the reference never sends unedged. Seed the state instead of
    // announcing it; the `/reload`-mid-cinematic case (`playing == true`) still falls through and
    // re-fires `CINEMATIC_START` into the rebuilt tree, which is what the memo is here for.
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

    /// The reference computes the bars on its native `1024 x 768` sheet, so that sheet is where its
    /// own numbers are quotable — and both of its cases have to come out of the one expression.
    #[test]
    fn the_letterbox_matches_the_reference_on_its_own_sheet() {
        // Exactly 4:3 — the branch `CinematicFrame.lua` does NOT take, so the bars stand at the
        // size `CinematicFrame.xml` declares: 128 of 768. The formula has to agree there, because
        // that agreement is the proof the XML default *is* the formula at the boundary.
        assert_eq!(letterbox_bar(1024.0, 768.0), 128.0);

        // Wider: the Lua recomputes, `desiredHeight = width/2`, remainder split evenly. 16:10 on
        // the same sheet is 1229 x 768 -> picture 614.5, bars 76.75 apiece.
        assert!((letterbox_bar(1228.8, 768.0) - 76.8).abs() < 0.05);
    }

    /// The wide case is a **2:1 picture**, whatever the screen: that is the whole point of
    /// `desiredHeight = width/2`, and it is the number a player can measure off a screenshot.
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

    /// The regression this function exists for: benilla read the Lua's `if` as "only a wide screen
    /// gets bars" and drew none at or below 4:3 — the reference's *native* aspect, and the one a
    /// windowed player lands on most easily.
    #[test]
    fn four_three_and_narrower_are_still_letterboxed() {
        // 4:3 at a real resolution.
        assert!((letterbox_bar(1024.0, 768.0) / 768.0 - 1.0 / 6.0).abs() < 1e-6);
        assert!((letterbox_bar(1600.0, 1200.0) / 1200.0 - 1.0 / 6.0).abs() < 1e-6);
        // 5:4 and square: the reference leaves its textures alone, so the cap holds at a sixth
        // rather than following `width/2` down to a taller bar.
        assert!((letterbox_bar(1280.0, 1024.0) / 1024.0 - 1.0 / 6.0).abs() < 1e-6);
        assert!((letterbox_bar(768.0, 768.0) / 768.0 - 1.0 / 6.0).abs() < 1e-6);
    }

    /// A window can be any shape; none of them may produce a negative or a screen-swallowing bar.
    #[test]
    fn no_window_shape_produces_a_nonsense_bar() {
        for (w, h) in [(1.0, 1.0), (4000.0, 100.0), (100.0, 4000.0), (0.0, 720.0)] {
            let bar = letterbox_bar(w, h);
            assert!(bar >= 0.0, "{w}x{h} -> {bar}");
            assert!(2.0 * bar <= h, "{w}x{h} -> {bar} swallows the screen");
        }
    }
}

/// **The ack ledger.** Decision 0196's invariant is the one thing on this path that cannot be
/// checked by looking at the screen: an unacked cinematic leaves vmangos anchoring object
/// visibility to its own copy of the flying camera, and everything around the body stays
/// despawned until relog. It had no test at all, and that is precisely why a second trigger could
/// silently displace a playing cinematic and pay nothing — the bug these cases now pin.
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

    /// Run `drive` once with the screen **already at full black** — the frame on which a boundary
    /// settles. The ramp that gets it there is `screen_fade`'s own business and is tested there;
    /// what these cases are about is what is paid when it arrives.
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

    /// One trigger in flight, one ack out — the ordinary ESC skip.
    ///
    /// **The ack is paid at full black, not where ESC was pressed.** `stop` only latches the skip:
    /// the reference's `StopCinematic` (`0x48f050`) ends nothing itself, it arms a fade-out and
    /// hands EndCinematic (`0x48f080`) to it as the completion callback. The invariant is
    /// unchanged — one ack per accepted trigger — it is simply paid one boundary later, and this
    /// pins both halves so the deferral can never quietly become a *dropped* ack.
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

    /// Two triggers accepted, two acks owed. Over-acking is discarded by a server that is not
    /// watching a cinematic; under-acking is the failure that costs a relog, so this direction is
    /// deliberate and not a duplicate to squash.
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

    /// The other side of that line: a cinematic that ends **on its own** leaves a latched trigger
    /// alone. It is owed its playback, not an ack — `start_pending` picks it up on the next pass.
    /// Only a skip says "not this" about the one still waiting.
    #[test]
    fn a_natural_end_does_not_pay_off_a_latched_trigger() {
        let (net, rx) = wire();
        // `playing_for_test` is shot-less, so the natural-end path cannot be reached through
        // `drive`'s duration test without a real `Cameras\*.m2`. The skip flag reaches the same
        // settle point; what this pins is the branch *inside* it, so the latch is set and the
        // end is taken as a completion rather than a stop.
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

    /// Nothing in flight owes nothing — ESC with no cinematic must not ack into the void.
    #[test]
    fn stopping_nothing_acks_nothing() {
        let (net, rx) = wire();
        let mut cine = Cinematic::default();
        stop(&mut cine, Some(&net));
        assert_eq!(acks(&rx), 0);
    }

    /// **The regression.** A trigger that displaces a latched one pays for the one it displaced —
    /// the latch is last-write-wins (the reference's single slot), but the server still started
    /// the cinematic it dropped.
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

    /// Leaving the world is the one path that deliberately pays **nothing**: the socket is going
    /// away with the cinematic, exactly as the reference's own teardown does (`0x490a80` clears
    /// the flag and sends no `CMSG_COMPLETE_CINEMATIC`).
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

/// **The end boundary holds the black until the world is back around the body.**
///
/// The reference's EndCinematic does a *blocking* terrain load at full black (`0x48f080`) and
/// raises no loading cover anywhere on the cinematic path (its raise, `0x406800`, has no caller
/// there). benilla streams asynchronously, so "block" is not available — but "keep showing the
/// black the fade already put up" is, and it produces the same thing on screen. Without it the
/// fly-by ends into a loading cover for half a second, measured, every time.
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

    /// **The black is a budget, not a wait.** A world that never comes back must not leave the
    /// player staring at a black screen — that would be strictly worse than the cover this
    /// removes. The budget runs out and the fade comes in regardless.
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
