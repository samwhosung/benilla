//! The loading screen: the full-screen splash and progress bar over world entry and cross-map
//! teleports. The art resolves `Map.dbc` → `LoadingScreens.dbc` → BLP for every map kind; the bar
//! is two layers, `Loading-BarBorder` and `Loading-BarFill` (Background, Glow and Glass are never
//! drawn in 1.12).
//!
//! Event-raised and readiness-cleared, as in the reference: it rises at the character pick's
//! `Connected` edge and at `SMSG_TRANSFER_PENDING` (the reference's two entries into `0x406640`'s
//! tail) and clears on a per-frame readiness poll. The reference's one blocking stretch
//! (`SMSG_NEW_WORLD`'s `0x401b00` defers `0x401bc0` onto the deadline heap, drained at `0x420d0c`)
//! also suppresses input, which this client builds explicitly ([`input`]).
//!
//! A destination snap only ends a raise's wait ([`LoadingScreen::snap_landed`]). The screen clears
//! when the destination is scene-presentable ([`WorldLoadProgress`]), never on the body reaching
//! the ground.

use bevy::prelude::*;
use std::collections::HashMap;

use benilla_formats::{load_loading_screens, LoadingScreenCatalog};

use benilla_assets::LockRecover;
use benilla_assets::MapCatalogRes;
use benilla_assets::{AssetSet, LooseOpen, WorldAssets};
use benilla_world::schedule::WorldStage;
use benilla_world::terrain_stream::WorldLoadProgress;

/// The cover's input half: while it is up the client takes no input (a source cut in `PreUpdate`).
mod input;
pub(crate) use input::CoverInput;

// The bar, from the `LoadingScreen.cpp` descriptor table at `0x7ffd34` (read by `0x407150`):
// entry {cx, cy, halfW, halfH}, rect [cx ± halfW·0.5] × [cy ± halfH·0.5], fill right edge
// left + progress·halfW; Border {0.5, 0.075, 0.600, 0.050}, Fill {0.5, 0.075, 0.525, 0.025}.
// Viewport fractions, `cy` measured up from the bottom edge (Bevy UI `bottom:`).
const BORDER_LEFT: f32 = 0.200; // 0.5 − 0.600·0.5
const BORDER_WIDTH: f32 = 0.600;
const BORDER_BOTTOM: f32 = 0.050; // 0.075 − 0.050·0.5
const BORDER_HEIGHT: f32 = 0.050;
const FILL_LEFT: f32 = 0.2375; // 0.5 − 0.525·0.5
const FILL_BOTTOM: f32 = 0.0625; // 0.075 − 0.025·0.5
const FILL_HEIGHT: f32 = 0.025;
const FILL_MAX_WIDTH: f32 = 0.525; // halfW; fill width = progress · FILL_MAX_WIDTH

/// The factor `0x41ad10` applies to the display aspect (`0x8012cc`): the picture is authored 4:3, so
/// `a = 0.75 · width/height` is 1.0 at 4:3 and the fit below turns on it.
const ASPECT_TO_FIT: f32 = 0.75;

/// Whether the `widescreen` CVar is on: its default, and anything but an applied 0.
pub(crate) fn widescreen(cvars: Option<&crate::cvars::Cvars>) -> bool {
    cvars.and_then(|c| c.flag("widescreen")) != Some(false)
}

/// `a = [0x832a4c]`, the display aspect relative to 4:3 (`0x41ad10`, read by `0x406a60` at
/// `0x406ac4` and by the tip at `0x40668c` and `0x4066b9`). A window with no size reads as 4:3.
///
/// Both setters (the glue's `0x46ab40` through `0x46a828`, the interface's `0x4901e0` through
/// `0x48fe2b`) pass `gxResolution`'s width over height only while `widescreen` is on, else 4/3,
/// and `RN24(0.75 · f32(4/3))` is exactly 1.0: with it off the picture fills the window (the
/// `0x406b02` path) and the tip's wrap is `515·W/1024`.
///
/// Deviation: the live window's aspect stands in for `gxResolution`'s, as in `portrait`'s
/// `GxAspect` (fullscreen is the monitor's own size with no mode list, where the two agree).
pub(crate) fn scaled_aspect(window: Vec2, widescreen: bool) -> f32 {
    if widescreen && window.x > 0.0 && window.y > 0.0 {
        window.x / window.y * ASPECT_TO_FIT
    } else {
        1.0
    }
}

/// A rectangle in the window's logical pixels, from its top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FitRect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// The viewport the paint callback `0x406a60` installs (`0x58af60` at `0x406b3d`) for a window of
/// this size, which the art, the bar and the tip are drawn in. Wider than 4:3 (`a > 1`,
/// `0x406ac9`-`0x406ad4`) it is `1/a` of the width, centred; narrower (`a < 1`, `0x406af7`-
/// `0x406b02`) it is `a` of the height, centred; at exactly 4:3, or with no usable aspect, the
/// whole window (`0x406b02` keeps the defaults for `a == 1` and a NaN). The edges are fractions of
/// the window, `(1 − extent) · 0.5` from the near side.
pub(crate) fn fit_rect(window: Vec2, widescreen: bool) -> FitRect {
    let a = scaled_aspect(window, widescreen);
    let (mut left, mut bottom, mut across, mut up) = (0.0, 0.0, 1.0, 1.0);
    if a > 1.0 {
        across = 1.0 / a;
        left = (1.0 - across) * 0.5;
    } else if a < 1.0 {
        up = a;
        bottom = (1.0 - up) * 0.5;
    }
    // The bands above and below are equal, so the bottom edge is also the top edge's distance.
    FitRect {
        x: left * window.x,
        y: bottom * window.y,
        width: across * window.x,
        height: up * window.y,
    }
}
/// Consecutive fully resident frames before the clear: debounces the post-teleport frame where
/// `ready` reads 0 against a nonzero `total`.
const CLEAR_AFTER_READY_FRAMES: u32 = 3;
/// Once the screen has been up this long, log which term still blocks the clear, every
/// [`WAIT_LOG_EVERY`] seconds.
const WAIT_LOG_AFTER: f32 = 3.0;
const WAIT_LOG_EVERY: f32 = 2.0;

/// Consecutive covered, in-world frames before the cover is on the glass: renders are serial, so
/// at 3 the two frames before have presented.
const COVER_PRESENT_FRAMES: u32 = 3;

/// Whether the entry cover has reached the glass. The first covered frame is the first whose
/// render can draw the cover, so synchronous work on it freezes character select on screen.
/// Counted in `First` so `PreUpdate` and `Update` readers see the same frame's answer.
#[derive(Resource, Default)]
pub(crate) struct EntryCover {
    /// Consecutive covered+in-world frames; reset the moment either goes false.
    frames: u32,
}

impl EntryCover {
    /// Whether the cover has had enough frames to reach the glass; true when no cover is up, since
    /// a consumer would otherwise wait forever (a capture that boots straight in world).
    pub(crate) fn presented(&self) -> bool {
        self.frames == 0 || self.frames >= COVER_PRESENT_FRAMES
    }

    /// A cover is up and not yet presented: a renderer draws nothing but the cover this frame.
    pub(crate) fn owes_a_present(&self) -> bool {
        self.frames > 0 && self.frames < COVER_PRESENT_FRAMES
    }

    /// A cover is up and the client is in world.
    pub(crate) fn covering(&self) -> bool {
        self.frames > 0
    }

    /// Covered frames so far.
    #[cfg(test)]
    pub(crate) fn frames(&self) -> u32 {
        self.frames
    }
}

impl EntryCover {
    /// One frame's count; the system and the tests both go through this.
    pub(crate) fn tick(&mut self, covered: bool) {
        self.frames = if covered {
            self.frames.saturating_add(1)
        } else {
            0
        };
    }
}

/// `First`: advance or reset the covered-frame count.
fn count_entry_cover(
    screen: Res<LoadingScreen>,
    state: Res<State<crate::client_state::ClientState>>,
    mut cover: ResMut<EntryCover>,
) {
    let in_world = *state.get() == crate::client_state::ClientState::InWorld;
    cover.tick(screen.covering() && in_world);
}

/// The `LoadingScreenID` → BLP path table; [`MapCatalogRes`] maps a map to its `LoadingScreenID`.
#[derive(Resource)]
struct LoadingScreenCatalogRes(LoadingScreenCatalog);

/// What a raise means for the tip of the day.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TipEdge {
    /// The glue→world entry: pick the next row and advance the cursor.
    Pick,
    /// Every other raise: this screen carries no tip.
    Clear,
}

/// Loading-screen state machine (event-raised, readiness-cleared).
#[derive(Resource, Default)]
pub(crate) struct LoadingScreen {
    active: bool,
    /// A raise from an entry edge holds until the destination snap lands, so the old location's
    /// residency cannot clear a screen raised for the new one (at world entry the login vista is
    /// resident well before `SMSG_LOGIN_VERIFY_WORLD`).
    awaiting_snap: bool,
    /// The map this screen's art resolves from, the reference's `[0x82f00c]`, set by every raise;
    /// `None` is its `-1`, no backdrop. Not `CurrentMap`: vmangos can relocate a character inside
    /// `Player::LoadFromDB` (`Player.cpp:15020`), under a live screen. Its writers: `0x4067e6` and
    /// `0x4072d3` store a map id, `0x406d0e`, `0x4073d4` and `0x407e59` store `-1`.
    map: Option<u32>,
    /// The reference's `[0x882e04]` texture handle, which latches the art: `0x406cf0` reads the map
    /// id only to reject `-1`, and `0x406cfc`/`0x406d01`/`0x406d03` skip the resolver `0x406e20`
    /// while it is set. Cleared by [`Self::dismiss`], never by a raise.
    art_resolved: bool,
    /// Plain black cover with no art or bar, for the world→glue cut; dropped once the state leaves
    /// `InWorld`.
    blackout: bool,
    /// Consecutive presentable frames while active ([`CLEAR_AFTER_READY_FRAMES`]).
    ready_frames: u32,
    /// Bar fill, 0..1, monotonic within a load: the raw residency ratio dips as the view moves.
    displayed: f32,
    /// Decoded backdrop art by BLP path.
    art_cache: HashMap<String, Handle<Image>>,
    /// Which tip the next raise carries, taken by [`crate::game_tip::drive_game_tip`].
    pub(crate) tip_edge: Option<TipEdge>,
    /// Capture only: the clear is skipped while set ([`Self::hold_for_capture`]).
    held: bool,

    /// `Time::elapsed_secs` at the last raise and at the last wait log line.
    active_since: f32,
    last_wait_log: f32,
}

impl LoadingScreen {
    /// Whether the opaque cover is up; the world camera renders under it, never behind the glue.
    pub(crate) fn covering(&self) -> bool {
        self.active
    }

    /// An active cover, for tests.
    #[cfg(test)]
    pub(crate) fn test_covering() -> Self {
        Self {
            active: true,
            ..Self::default()
        }
    }

    /// Take the pending raise's tip edge; read once, by [`crate::game_tip::drive_game_tip`].
    pub(crate) fn take_tip_edge(&mut self) -> Option<TipEdge> {
        self.tip_edge.take()
    }

    /// The capture instrument (`WOW_CAPTURE=loading-tip`): raise as the glue→world entry does,
    /// with art, bar and a `Pick` tip, and never clear. `map` latches the art as a raise's does.
    pub(crate) fn hold_for_capture(&mut self, map: Option<u32>) {
        self.active = true;
        self.blackout = false;
        self.awaiting_snap = false;
        self.map = map;
        self.held = true;
        self.tip_edge = Some(TipEdge::Pick);
    }

    /// Raise the screen for a fresh load, its art latched to `map` for its life (a caller with no
    /// announced destination passes the current map). Only an edge that reaches the reference's
    /// transition entries calls this, never a snap ([`Self::snap_landed`]).
    fn raise(&mut self, reason: &str, awaiting_snap: bool, map: Option<u32>, now: f32) {
        self.active = true;
        self.blackout = false;
        self.awaiting_snap = awaiting_snap;
        self.map = map;
        self.ready_frames = 0;
        self.displayed = 0.0; // restart the fill for the new load
        self.active_since = now;
        self.last_wait_log = now;
        info!("loading screen: up ({reason}, map {map:?})");
    }

    /// The destination snap (`SMSG_NEW_WORLD` or `SMSG_LOGIN_VERIFY_WORLD`) landed: it ends the
    /// raise's wait and nothing else, not the art, the tip or the bar. The reference's handlers
    /// (`0x401b00`, `0x401de0`) never touch the screen: the raise `0x406800` has three call sites,
    /// all inside `LoadingScreen.cpp`. `map` is `None` for a same-map teleport ack.
    fn snap_landed(&mut self, map: Option<u32>) {
        if let (Some(raised), Some(dest)) = (self.map, map) {
            if raised != dest {
                info!("loading screen: snap landed on map {dest} — raised for map {raised}, art holds");
            }
        }
        self.awaiting_snap = false;
    }

    /// A far snap: a screen already up is this snap's own, so the snap only ends its wait. Returns
    /// `true` when no screen is up and the caller should raise one, our backstop for a port with no
    /// announcing edge (the reference's blocking world load needs none).
    #[must_use]
    fn far_snap(&mut self, map: u32) -> bool {
        if !self.active {
            return true;
        }
        self.snap_landed(Some(map));
        false
    }

    /// The dismiss (the reference's `0x407e80`), the only place a tip clears: `[0x882e10]`'s
    /// writers are the `EnterWorld` setter `0x406630` and `0x407f2b` here.
    fn dismiss(&mut self) {
        self.active = false;
        self.blackout = false;
        self.tip_edge = Some(TipEdge::Clear);
        // `0x407ed7` clears `[0x882e04]`: the next screen resolves its own art.
        self.art_resolved = false;
    }
}

// UI entity markers.
#[derive(Component)]
struct LoadingRoot;
#[derive(Component)]
struct LoadingBackdrop;
/// The content area: the viewport `0x406a60` fits to the window ([`fit_rect`]). The bar's
/// fractions are relative to it.
#[derive(Component)]
pub(crate) struct LoadingArea;
#[derive(Component)]
struct LoadingBarFill;
#[derive(Component)]
struct LoadingBarBorder;
/// The tip-of-the-day `Text` root; its coloured runs are children, rebuilt when the tip changes.
#[derive(Component)]
pub(crate) struct LoadingTip;

pub(crate) struct LoadingScreenPlugin;

impl Plugin for LoadingScreenPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadingScreen>()
            .init_resource::<EntryCover>()
            // In `First`, ahead of every consumer in `PreUpdate` and `Update`.
            .add_systems(First, count_entry_cover)
            .add_systems(Startup, setup_loading_screen.after(AssetSet::Open))
            // In `WorldStage::Present`, after Input and Stream: a worldport snaps in Input and the
            // streamer refocuses in Stream, so the cover lands on the same frame, never a flash.
            .add_systems(Update, drive_loading_screen.in_set(WorldStage::Present))
            // After the drive, which sets the visibility the same nodes carry, and after the
            // window's mode is applied, so the size it reads is this frame's.
            .add_systems(
                Update,
                fit_loading_area
                    .in_set(WorldStage::Present)
                    .after(drive_loading_screen)
                    .after(crate::video::apply_window_mode)
                    .after(crate::video::apply_present_mode),
            );
        // The cover's input rule, registered here so the two cannot be registered apart.
        input::build(app);
    }
}

/// Startup: load the LoadingScreens catalog and the bar textures, and spawn the hidden UI tree.
fn setup_loading_screen(
    mut commands: Commands,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
) {
    // The black root, the bands of a pillarbox or letterbox, spawns unconditionally: `covering()` consumers act as covered while
    // the screen is active, so missing art must still leave a black cover.
    let root = commands
        .spawn((
            LoadingRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            // UI already paints after the 3D scene; this orders within UI.
            GlobalZIndex(1000),
            Visibility::Hidden,
        ))
        .id();

    let Some(mut assets) = world_assets else {
        return;
    };

    // Catalog (LoadingScreenID → BLP path).
    match load_loading_screens(&mut assets.chain.lock_recover()) {
        Ok(c) => {
            info!("LoadingScreens.dbc: {} screens catalogued", c.len());
            commands.insert_resource(LoadingScreenCatalogRes(c));
        }
        Err(e) => {
            error!("LoadingScreens.dbc unavailable — plain black cover, no art: {e:#}");
            return;
        }
    }

    // The two layers 1.12 draws, as sRGB clamp sprites, from the archives alone: the loading
    // screen's `TextureCreate` calls (`0x406891`) pass file flag 0.
    let mut tex = |path: &str| assets.sprite_texture_opened(path, LooseOpen::Archives, &mut images);
    let Some(border) = tex("Interface\\Glues\\LoadingBar\\Loading-BarBorder.blp") else {
        error!("loading bar textures missing — plain black cover, no art");
        return;
    };
    let fill = tex("Interface\\Glues\\LoadingBar\\Loading-BarFill.blp").unwrap_or_default();

    commands.entity(root).with_children(|root| {
        // The fitted rect, centred by the root; [`fit_loading_area`] sizes it each frame. The bar's
        // fractions are relative to it. Paint order is spawn order: backdrop, tip, fill, border.
        root.spawn((
            LoadingArea,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Relative,
                ..default()
            },
        ))
        .with_children(|area| {
            // Tinted black until the art resolves.
            area.spawn((
                LoadingBackdrop,
                ImageNode {
                    color: Color::BLACK,
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ));
            // The tip, between the backdrop and the bar as in the reference (`0x406e12`,
            // `0x406e18`), placed by `crate::game_tip` in window terms, as the reference does.
            area.spawn((LoadingTip, crate::game_tip::tip_bundle()));
            // The fill art is a horizontally uniform gradient, so scaling its width is a reveal.
            area.spawn((
                LoadingBarFill,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(FILL_LEFT * 100.0),
                    bottom: Val::Percent(FILL_BOTTOM * 100.0),
                    width: Val::Percent(0.0), // set each frame
                    height: Val::Percent(FILL_HEIGHT * 100.0),
                    ..default()
                },
                ImageNode {
                    image: fill,
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
            ));
            // Border frame, on top.
            area.spawn((
                LoadingBarBorder,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(BORDER_LEFT * 100.0),
                    bottom: Val::Percent(BORDER_BOTTOM * 100.0),
                    width: Val::Percent(BORDER_WIDTH * 100.0),
                    height: Val::Percent(BORDER_HEIGHT * 100.0),
                    ..default()
                },
                ImageNode {
                    image: border,
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
            ));
        });
    });
}

/// Per frame: the content area is the window's [`fit_rect`], so a resize during a load refits the
/// art, the bar and the tip, as the reference's per-frame paint does (`0x406a60`).
pub(crate) fn fit_loading_area(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut area: Query<&mut Node, With<LoadingArea>>,
    cvars: Option<Res<crate::cvars::Cvars>>,
) {
    let (Some(window), Ok(mut node)) = (windows.iter().next(), area.single_mut()) else {
        return;
    };
    let fit = fit_rect(
        Vec2::new(window.width(), window.height()),
        widescreen(cvars.as_deref()),
    );
    let (width, height) = (Val::Px(fit.width), Val::Px(fit.height));
    if node.width != width || node.height != height {
        node.width = width;
        node.height = height;
    }
}

/// Per frame: observe the lifecycle edges, run the raise and clear rules, resolve the backdrop and
/// set the bar.
#[allow(clippy::type_complexity)]
fn drive_loading_screen(
    mut screen: ResMut<LoadingScreen>,
    progress: Res<WorldLoadProgress>,
    maps: Option<Res<MapCatalogRes>>,
    screens: Option<Res<LoadingScreenCatalogRes>>,
    mut assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut root: Query<&mut Visibility, (With<LoadingRoot>, Without<LoadingBackdrop>)>,
    mut backdrop: Query<(&mut ImageNode, &mut Visibility), With<LoadingBackdrop>>,
    mut fill: Query<&mut Node, With<LoadingBarFill>>,
    mut bar_vis: Query<
        &mut Visibility,
        (
            Or<(With<LoadingBarFill>, With<LoadingBarBorder>)>,
            Without<LoadingRoot>,
            Without<LoadingBackdrop>,
        ),
    >,
    player: Option<Res<crate::player::Player>>,
    // Real time: `Time<Virtual>` clamps frames over 250 ms, and a load is made of those.
    time: Res<Time<Real>>,
    // The cover holds until the warm pass has compiled its pipelines.
    warm: Res<crate::pipe_warm::WarmPass>,
    // The world-entry UI load runs behind this cover; the reference's reveal always has the UI up.
    entry_ui_pending: Option<Res<crate::ui_script::PendingEntryUiLoad>>,
    edges: (
        Res<State<crate::client_state::ClientState>>,
        MessageReader<crate::net::EnteredWorldMessage>,
        MessageReader<crate::net::WorldportMessage>,
        MessageReader<crate::net::TeleportMessage>,
        MessageReader<crate::net::LoggedOutMessage>,
        MessageReader<crate::net::DisconnectedMessage>,
        MessageReader<crate::net::CharacterLoginFailedMessage>,
        Option<Res<crate::net::PendingTransfer>>,
        Option<Res<crate::glue_session::roster::Roster>>,
    ),
) {
    let (
        state,
        mut entered,
        mut worldports,
        mut teleports,
        mut logouts,
        mut lost,
        mut refusals,
        transfer,
        roster,
    ) = edges;
    let now = time.elapsed_secs();
    // The physics hold releases on the same residency signal, so the body's world is live first.
    let player_settling = player.as_ref().is_some_and(|p| p.settling);

    // --- The edges. ---
    // A glue entry (not the seamless in-world reconnect), raised on the frame the glue tears down
    // and held across the server's character load; the art comes from the roster's map.
    if entered.read().next().is_some() && *state.get() != crate::client_state::ClientState::InWorld
    {
        let map = roster.as_ref().and_then(|r| r.pending_map());
        screen.raise("world entry", true, map, now);
        // The tip rides this edge alone: the reference's setter `0x406630` has one caller, in
        // `CGlueMgr::EnterWorld`, which neither `SMSG_TRANSFER_PENDING` arm reaches.
        screen.tip_edge = Some(TipEdge::Pick);
    }
    // A portal (`SMSG_TRANSFER_PENDING`, no transport): cover now, before the server unloads us, as
    // the reference does. A transport crossing rides visibly until its worldport lands.
    if let Some(t) = transfer.as_ref().filter(|t| t.is_changed()) {
        match &t.0 {
            Some(info) if info.transport_entry.is_none() => {
                screen.raise("transfer pending", true, Some(info.map_id), now);
            }
            // Cleared with no snap in flight is `SMSG_TRANSFER_ABORTED` (a worldport lands below
            // this frame): stop waiting, and the resident old world clears the screen.
            None => screen.awaiting_snap = false,
            Some(_) => {}
        }
    }
    // A snap never re-raises a live screen ([`LoadingScreen::far_snap`]).
    for w in worldports.read() {
        if screen.far_snap(w.map_id) {
            screen.raise("worldport", false, Some(w.map_id), now);
        }
    }
    // A same-map teleport ends any awaited snap and raises nothing: the reference's
    // `MSG_MOVE_TELEPORT_ACK` handler (`0x603f90`) reaches none of the screen's raises.
    if teleports.read().next().is_some() {
        screen.snap_landed(None);
    }
    // Logout, a lost session or a refused login: `CharSelect` applies next frame, so cover the torn
    // down world with black (the reference's world→glue cut), and disarm a snap that will not come.
    let session_over = lost.read().any(|m| m.session_over);
    let refused = refusals.read().next().is_some();
    if logouts.read().next().is_some() || session_over || refused {
        screen.active = true;
        screen.blackout = true;
        screen.awaiting_snap = false;
        screen.ready_frames = 0;
        info!(
            "loading screen: blackout ({})",
            if session_over {
                "session lost"
            } else if refused {
                "character login refused"
            } else {
                "logout"
            }
        );
    }
    if screen.blackout && *state.get() != crate::client_state::ClientState::InWorld {
        // The glue is up (it renders above this root); the cover's job is done.
        screen.dismiss();
    }

    // --- Clear, sustained a few frames. ---
    if screen.active && !screen.held {
        if progress.is_ready()
            && progress.presentable()
            && !screen.awaiting_snap
            && !player_settling
            && warm.satisfied()
            && entry_ui_pending.is_none()
        {
            screen.ready_frames += 1;
            if screen.ready_frames >= CLEAR_AFTER_READY_FRAMES {
                screen.dismiss();
                info!(
                    "loading screen: cleared ({}/{} resident, {:.1}s)",
                    progress.ready,
                    progress.total,
                    now - screen.active_since
                );
            }
        } else {
            screen.ready_frames = 0;
            if now - screen.active_since > WAIT_LOG_AFTER
                && now - screen.last_wait_log >= WAIT_LOG_EVERY
            {
                screen.last_wait_log = now;
                info!(
                    "loading screen: waiting {:.1}s — {}/{} resident, {} placements pending, \
                     {} colliders pending, {} merges pending, awaiting_snap={}, settling={}, \
                     warm={}, ui_pending={}",
                    now - screen.active_since,
                    progress.ready,
                    progress.total,
                    progress.placements_pending,
                    progress.colliders_pending,
                    progress.merge_pending,
                    screen.awaiting_snap,
                    player_settling,
                    warm.satisfied(),
                    entry_ui_pending.is_some(),
                );
            }
        }
    }

    // --- Visibility: under blackout only the root's black shows. ---
    if let Ok(mut vis) = root.single_mut() {
        *vis = if screen.active {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    let content_vis = if screen.blackout {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    if let Ok((_, mut vis)) = backdrop.single_mut() {
        if *vis != content_vis {
            *vis = content_vis;
        }
    }
    for mut vis in &mut bar_vis {
        if *vis != content_vis {
            *vis = content_vis;
        }
    }
    if !screen.active || screen.blackout {
        return;
    }

    // --- Backdrop art, `0x406cf0`'s gate: the map id is read once per screen to resolve the
    // texture, so a raise onto a live screen repaints nothing. ---
    if !screen.art_resolved {
        // Black, never the last screen's texture (the reference holds a null handle, no quad).
        if let Ok((mut img, _)) = backdrop.single_mut() {
            if img.color != Color::BLACK {
                img.color = Color::BLACK;
            }
        }
        if let (Some(map_id), Some(maps), Some(screens), Some(assets)) =
            (screen.map, maps.as_ref(), screens.as_ref(), assets.as_mut())
        {
            let path = maps
                .0
                .loading_screen_id(map_id)
                .and_then(|id| screens.0.path(id))
                .map(str::to_string);
            let handle = path.and_then(|path| match screen.art_cache.get(&path) {
                Some(h) => Some(h.clone()),
                // `ResolveLoadingArt` opens the art with file flag 0 (`0x406eba`, `0x406ef1`).
                None => assets
                    .sprite_texture_opened(&path, LooseOpen::Archives, &mut images)
                    .inspect(|h| {
                        screen.art_cache.insert(path.clone(), h.clone());
                    }),
            });
            match (handle, backdrop.single_mut()) {
                (Some(handle), Ok((mut img, _))) => {
                    img.image = handle;
                    img.color = Color::WHITE; // untint to reveal the art
                    screen.art_resolved = true;
                    info!("loading screen: backdrop for map {map_id} resolved");
                }
                // No retry: `0x406d0e` writes `-1` into the map id and `0x406cf0` returns on it.
                _ => {
                    warn!("loading screen: no backdrop for map {map_id} — plain black this load");
                    screen.map = None;
                }
            }
        }
    }

    // --- Progress bar: zero until the snap lands (residency is still the old place's). ---
    if !screen.awaiting_snap {
        screen.displayed = screen.displayed.max(progress.fraction());
    }
    let frac = screen.displayed;
    if let Ok(mut node) = fill.single_mut() {
        node.width = Val::Percent(frac * FILL_MAX_WIDTH * 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_world::world_map::CurrentMap;

    fn fit(w: f32, h: f32) -> [f32; 4] {
        let r = fit_rect(Vec2::new(w, h), true);
        [r.x, r.y, r.width, r.height]
    }

    fn close(got: [f32; 4], want: [f32; 4]) {
        assert!(
            got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01),
            "{got:?} != {want:?}"
        );
    }

    /// `0x406a60`: `a > 1` is `1/a` of the width, `a < 1` is `a` of the height, each centred, and
    /// exactly 4:3 is the whole window.
    #[test]
    fn the_picture_is_fit_inside_the_window_in_both_directions() {
        // Wider than 4:3: pillarboxed, full height.
        close(fit(1920.0, 1080.0), [240.0, 0.0, 1440.0, 1080.0]);
        // Narrower: letterboxed, full width, 3/4 of it tall.
        close(fit(1280.0, 1024.0), [0.0, 32.0, 1280.0, 960.0]);
        close(fit(600.0, 1000.0), [0.0, 275.0, 600.0, 450.0]);
        // 4:3: the window.
        close(fit(1024.0, 768.0), [0.0, 0.0, 1024.0, 768.0]);
        close(fit(800.0, 600.0), [0.0, 0.0, 800.0, 600.0]);
    }

    /// `widescreen` off is a 4/3 aspect (`0x46ab40`, `0x4901e0`): `a` is exactly 1.0 and the picture
    /// fills the window, at any window shape.
    #[test]
    fn widescreen_off_fills_the_window() {
        for (w, h) in [(1920.0, 1080.0), (1280.0, 1024.0), (600.0, 1000.0)] {
            let r = fit_rect(Vec2::new(w, h), false);
            assert_eq!(scaled_aspect(Vec2::new(w, h), false), 1.0);
            close([r.x, r.y, r.width, r.height], [0.0, 0.0, w, h]);
        }
        assert!(
            widescreen(Some(&crate::cvars::Cvars::default())),
            "the default is on"
        );
        assert!(widescreen(None));
        assert!(!widescreen(Some(&crate::cvars::Cvars::with_value(
            "widescreen",
            "0"
        ))));
    }

    /// The system reads the CVar: a tall window, `widescreen` off, is not letterboxed.
    #[test]
    fn the_content_area_honours_widescreen_off() {
        let mut app = App::new();
        app.insert_resource(crate::cvars::Cvars::with_value("widescreen", "0"));
        app.world_mut().spawn((
            Window {
                resolution: (1280_u32, 1024_u32).into(),
                ..default()
            },
            bevy::window::PrimaryWindow,
        ));
        let area = app.world_mut().spawn((LoadingArea, Node::default())).id();
        app.add_systems(Update, fit_loading_area);
        app.update();
        let n = app.world().get::<Node>(area).unwrap();
        assert_eq!((n.width, n.height), (Val::Px(1280.0), Val::Px(1024.0)));
    }

    /// A window with no size has no aspect; it reads as 4:3 and fits to nothing, never a NaN.
    #[test]
    fn a_window_without_a_size_fits_to_nothing() {
        close(fit(0.0, 0.0), [0.0; 4]);
    }

    /// The system sizes the node the loading screen spawns from the primary window each frame, so
    /// a window resized tall during a load letterboxes it.
    #[test]
    fn the_content_area_follows_the_window() {
        let mut app = App::new();
        let window = app
            .world_mut()
            .spawn((
                Window {
                    resolution: (1280_u32, 1024_u32).into(),
                    ..default()
                },
                bevy::window::PrimaryWindow,
            ))
            .id();
        let area = app.world_mut().spawn((LoadingArea, Node::default())).id();
        app.add_systems(Update, fit_loading_area);
        let size = |app: &App| {
            let n = app.world().get::<Node>(area).unwrap();
            (n.width, n.height)
        };
        app.update();
        assert_eq!(size(&app), (Val::Px(1280.0), Val::Px(960.0)));
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set(1920.0, 1080.0);
        app.update();
        assert_eq!(size(&app), (Val::Px(1440.0), Val::Px(1080.0)));
    }

    #[test]
    fn the_cover_counts_frames_to_the_glass_and_resets_the_moment_it_drops() {
        let mut cover = EntryCover::default();
        assert!(
            cover.presented(),
            "no cover up: nothing is watching the previous present, so nothing waits"
        );
        assert!(!cover.owes_a_present(), "no cover owes no present");

        cover.tick(true);
        assert_eq!(cover.frames(), 1);
        assert!(
            !cover.presented(),
            "the cover's own render has not committed yet"
        );
        assert!(cover.owes_a_present());

        cover.tick(true);
        assert!(!cover.presented(), "one render committed, not two");

        cover.tick(true);
        assert!(
            cover.presented(),
            "at COVER_PRESENT_FRAMES the cover is provably on the glass"
        );
        assert!(!cover.owes_a_present());

        // A reveal drops the count to zero in one frame; the next raise earns its own.
        cover.tick(false);
        assert_eq!(cover.frames(), 0);
        assert!(cover.presented());
        cover.tick(true);
        assert!(!cover.presented(), "the next raise starts its own count");
    }
    /// vmangos can relocate a character out of an expired instance inside `Player::LoadFromDB`, so
    /// `SMSG_LOGIN_VERIFY_WORLD` names another map than the roster did.
    #[test]
    fn a_relocating_snap_cannot_move_the_art_the_screen_was_raised_with() {
        let mut screen = LoadingScreen::default();
        // The pick edge: art from the roster row, Shadowfang Keep, snap still to come.
        screen.raise("world entry", true, Some(33), 0.0);
        screen.tip_edge = Some(TipEdge::Pick);
        screen.displayed = 0.4;

        // `SMSG_LOGIN_VERIFY_WORLD`: the server seats us on map 0, at the instance's exit.
        assert!(
            !screen.far_snap(0),
            "a screen is already up — its own snap must not raise a second one"
        );
        assert_eq!(
            screen.map,
            Some(33),
            "the backdrop is the raise's and holds the whole way — the report"
        );
        assert!(
            !screen.awaiting_snap,
            "the snap the raise was waiting for has landed"
        );
        assert_eq!(
            screen.tip_edge,
            Some(TipEdge::Pick),
            "and the tip just picked survives it — a raise is the only thing that could \
             have cleared it, and no raise happened"
        );
        assert!(
            (screen.displayed - 0.4).abs() < f32::EPSILON,
            "nor does the bar restart mid-load"
        );
        assert!(screen.active, "the screen stays up across its own snap");
    }

    /// A cross-map port with no announcing edge still gets a cover: our backstop, which the
    /// reference's blocking world load does not need.
    #[test]
    fn a_snap_with_nothing_covering_it_asks_for_a_raise() {
        let mut screen = LoadingScreen::default();
        assert!(screen.far_snap(1), "nothing is covering this load");
        assert!(
            !screen.active,
            "far_snap answers the question; the caller does the raising, with its own reason"
        );
    }

    #[test]
    fn only_the_dismiss_clears_the_tip() {
        let mut screen = LoadingScreen::default();
        screen.raise("world entry", true, Some(0), 0.0);
        screen.tip_edge = Some(TipEdge::Pick);

        screen.raise("transfer pending", true, Some(389), 0.0);
        assert_eq!(
            screen.tip_edge,
            Some(TipEdge::Pick),
            "no raise clears a tip"
        );

        screen.dismiss();
        assert_eq!(screen.tip_edge, Some(TipEdge::Clear), "the dismiss does");
        assert!(!screen.active);
    }

    /// The reference's raises (`0x406640`/`0x4072c0`) store the map id with no guard.
    #[test]
    fn a_raise_onto_a_live_screen_repoints_the_map_but_never_the_picture() {
        let mut screen = LoadingScreen::default();
        screen.raise("world entry", true, Some(33), 0.0);
        screen.art_resolved = true; // the first plain-draw frame resolved Shadowfang Keep's art

        screen.raise("transfer pending", true, Some(389), 0.0);
        assert_eq!(screen.map, Some(389), "the map id is re-pointed, unguarded");
        assert!(
            screen.art_resolved,
            "…and the picture is not: the texture is still the one this screen resolved"
        );

        // The dismiss is the only thing that lets the next screen resolve its own.
        screen.dismiss();
        assert!(!screen.art_resolved);
    }

    /// `drive_loading_screen` alone, in world, standing on resident ground at `at` (wow coords).
    fn in_world(at: [f32; 3]) -> App {
        let mut player = crate::player::Player::default();
        player.active = true;
        player.pos = benilla_assets::coords::wow_to_bevy(at);
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(crate::client_state::ClientState::InWorld)
            .add_message::<crate::net::EnteredWorldMessage>()
            .add_message::<crate::net::WorldportMessage>()
            .add_message::<crate::net::TeleportMessage>()
            .add_message::<crate::net::LoggedOutMessage>()
            .add_message::<crate::net::DisconnectedMessage>()
            .add_message::<crate::net::CharacterLoginFailedMessage>()
            .init_resource::<LoadingScreen>()
            .init_resource::<crate::pipe_warm::WarmPass>()
            .init_resource::<Assets<Image>>()
            .insert_resource(CurrentMap(1))
            .insert_resource(benilla_world::terrain_stream::ViewFocus::body(at, true))
            .insert_resource(WorldLoadProgress {
                ready: 9,
                total: 9,
                focus_resident: true,
                scene_ready: true,
                ..Default::default()
            })
            .insert_resource(player)
            .add_systems(Update, drive_loading_screen);
        app.update();
        app
    }

    /// The snap's frame as the stream sees it: the body held over ground not yet loaded.
    fn land_unloaded(app: &mut App, at: [f32; 3]) {
        let pos = benilla_assets::coords::wow_to_bevy(at);
        let mut player = app.world_mut().resource_mut::<crate::player::Player>();
        player.pos = pos;
        player.settling = true;
        app.insert_resource(benilla_world::terrain_stream::ViewFocus::body(at, false));
        app.insert_resource(WorldLoadProgress {
            ready: 0,
            total: 9,
            ..Default::default()
        });
    }

    fn covering(app: &App) -> bool {
        app.world().resource::<LoadingScreen>().covering()
    }

    const GM_ISLAND: [f32; 3] = [16222.0, 16252.0, 12.5];
    const SOUTH_SEAS: [f32; 3] = [-11880.0, -4700.0, 2.0];
    const NORTHSHIRE: [f32; 3] = [-8949.95, -132.493, 83.5312];

    /// GM Island to the South Seas, both on Kalimdor.
    #[test]
    fn a_same_map_teleport_across_the_continent_shows_no_loading_screen() {
        let mut app = in_world(GM_ISLAND);
        assert!(!covering(&app), "standing on resident ground");

        land_unloaded(&mut app, SOUTH_SEAS);
        app.world_mut().write_message(crate::net::TeleportMessage {
            guid: 1,
            counter: 0,
            position: SOUTH_SEAS,
            orientation: 0.0,
        });
        app.update();
        assert!(!covering(&app), "the snap frame raises no screen");

        for _ in 0..30 {
            app.update();
            assert!(
                !covering(&app),
                "nor does the destination streaming in under the held body"
            );
        }
    }

    /// A live body outrunning the stream (GM speed ×100) stands on unloaded ground with no hold:
    /// the reference's tile pass waits for the read in the frame and calls no raise (`0x6985d0`),
    /// so the frame hitches and no screen comes up.
    #[test]
    fn outrunning_the_stream_raises_no_screen() {
        let mut app = in_world(GM_ISLAND);
        land_unloaded(&mut app, SOUTH_SEAS);
        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .settling = false;
        app.insert_resource(benilla_world::terrain_stream::ViewFocus::body(
            SOUTH_SEAS, true,
        ));
        for _ in 0..30 {
            app.update();
            assert!(
                !covering(&app),
                "the ground under a live body is not a load"
            );
        }
    }

    #[test]
    fn a_worldport_covers_its_load() {
        let mut app = in_world(GM_ISLAND);
        land_unloaded(&mut app, NORTHSHIRE);
        app.world_mut().write_message(crate::net::WorldportMessage {
            map_id: 0,
            position: NORTHSHIRE,
            orientation: 0.0,
            needs_ack: true,
            transport_entry: None,
        });
        app.update();
        assert!(covering(&app));
    }
}
