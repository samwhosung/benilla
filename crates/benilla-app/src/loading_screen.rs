//! Loading screen — the faithful full-screen world-load splash + progress bar shown on initial
//! world entry and on cross-map teleport (the load latency async streaming can't hide). Built on the
//! VERIFIED reference mechanism: per-map art via the
//! `Map.dbc` → `LoadingScreens.dbc` → BLP FK chain (resolved by `benilla-formats`), under the engine's
//! bar — which in 1.12 is exactly two layers, `Loading-BarBorder` + `Loading-BarFill`, at the
//! screen-fraction rects byte-verified from `LoadingScreen.cpp` (NOT the 5-texture stack the asset
//! names suggest; Background/Glow/Glass are never composited in vanilla).
//!
//! **Foundation, not a one-off.** The art lookup is the *same* mechanism for every map kind (open
//! world / instance / battleground) — only the BLP row differs — so this hosts all of them for free.
//! The two cases out of current scope (taxi/boat/zeppelin's moving flight-path icon; a richer
//! progress model) slot in as an extra overlay layer / a different progress source without restructure.
//!
//! **The lifecycle is event-raised, readiness-cleared (decision 0737)** — and the reference's is
//! too, which 0737 assumed it was not. This header claimed for months that the reference *blocks*
//! on its world load and so covers "by construction"; decision 1990 says
//! half of that is wrong. The reference runs ordinary frames with **one** blocking stretch inside
//! them (`SMSG_NEW_WORLD`'s `0x401b00` defers `0x401bc0` onto the deadline heap, which drains at
//! `0x420d0c` and runs to completion in that one iteration, `Sleep(1)` residency spins and all) —
//! but the screen is raised *frames earlier*, at `TRANSFER_PENDING`, and dismissed *frames later*
//! by a per-frame readiness poll. Which is this file's shape. What an async-streaming client has to
//! build explicitly is not the lifecycle after all: it is the **input suppression** the blocking
//! stretch does not provide either (see [`input`], decision 1990). The screen
//! rises at the **transition edges and only those** — the character pick's `Connected` edge (before
//! the glue tears down) and `SMSG_TRANSFER_PENDING` (the portal walk-in), which are precisely the
//! reference's two entries into `0x406640`'s tail — all observed *here*, from the
//! messages/resources the net bridge already publishes. The destination **snap** is not one of
//! them: `SMSG_NEW_WORLD` and `SMSG_LOGIN_VERIFY_WORLD` are two handlers there (`0x401b00` /
//! `0x401de0`) and neither can reach the raise `0x406800` or the screen's map global `[0x82f00c]`,
//! so a snap only ends the wait its raise armed and can never re-point a live screen
//! ([`LoadingScreen::far_snap`]). And what a live screen *shows* is latched harder still: the
//! resolved texture, not the map id ([`LoadingScreen::art_resolved`], decision 2087). A residency backstop catches any
//! path with no edge, **in world only** — it used to fire at boot too, and that is what warmed the
//! world's pipelines behind the glue (0540), which only worked while a world was being streamed
//! there at all. Since 0777 none is, and the warm-up rides the entry cover instead. It clears when
//! the destination is **scene-presentable** ([`WorldLoadProgress`],
//! published by `terrain_stream` + the collider queue): every wanted tile spawned, the focus
//! neighbourhood's placements up, colliders quiet, the snap no longer awaited. Never anything
//! about the *body* — feet-on-ground is not a load condition (the flying-teleport hang, 0737).

use bevy::prelude::*;
use std::collections::HashMap;

use benilla_formats::{load_loading_screens, LoadingScreenCatalog};

use benilla_assets::LockRecover;
use benilla_assets::MapCatalogRes;
use benilla_assets::{AssetSet, WorldAssets};
use benilla_world::schedule::WorldStage;
use benilla_world::terrain_stream::WorldLoadProgress;
use benilla_world::world_map::CurrentMap;

/// **The cover takes the input plane** — the loading screen's input half, in its own file because
/// it is a different mechanism (a source cut in `PreUpdate`) from the state machine below, and one
/// file should not have to explain both.
mod input;
pub(crate) use input::CoverInput;

// Bar layout — VERIFIED THREE ways (build 5875): the `WoW.exe` `LoadingScreen.cpp` bar descriptor
// table (@0x7ffd34, `FUN_00407150`) gives entry = {cx, cy, halfW, halfH} with rect = [cx ± halfW·0.5]
// × [cy ± halfH·0.5] and fill right edge = left + progress·halfW; Border {0.5,0.075,0.600,0.050}, Fill
// {0.5,0.075,0.525,0.025}. An apitrace of the live reference (WoW.12/WoW.17) confirmed these rects, and
// a reference screenshot measured the fill at left=0.245 (≈0.2375), ~9% from the BOTTOM. The bar sits
// at the BOTTOM — `cy=0.075` is measured from the bottom (the engine's GL ortho origin); the trace's
// "top" reading was the wined3d D3D→GL y-flip (a host representation, see memory
// `apitrace-is-crossover-translated`), refuted by the screenshot + the binary. ONLY Border + Fill are
// drawn in 1.12 (Background/Glow/Glass are never composited). Rects are viewport fractions; y is the
// distance from the BOTTOM edge (Bevy UI `bottom:`).
const BORDER_LEFT: f32 = 0.200; // 0.5 − 0.600·0.5
const BORDER_WIDTH: f32 = 0.600;
const BORDER_BOTTOM: f32 = 0.050; // 0.075 − 0.050·0.5
const BORDER_HEIGHT: f32 = 0.050;
const FILL_LEFT: f32 = 0.2375; // 0.5 − 0.525·0.5
const FILL_BOTTOM: f32 = 0.0625; // 0.075 − 0.025·0.5
const FILL_HEIGHT: f32 = 0.025;
const FILL_MAX_WIDTH: f32 = 0.525; // halfW; fill width = progress · FILL_MAX_WIDTH

/// The glue/loading screen is authored 4:3; on a wider window the reference fits it to height and
/// letterboxes (black bars L/R). Measured from a reference screenshot (content ≈ 1878×1385 ≈ 4:3 in a
/// 1999-wide window). The square BLP is stretched to this aspect (a mild widen).
const BACKDROP_ASPECT: f32 = 4.0 / 3.0;
/// Frames the world must read fully-resident before we clear the screen — debounces the post-teleport
/// frame where `loaded` is drained (`total > 0`, `ready` momentarily 0) so we don't flicker off/on.
const CLEAR_AFTER_READY_FRAMES: u32 = 3;
/// The wait instrument (decision 0737): once the screen has been up this long, say *which term*
/// still blocks the clear, every [`WAIT_LOG_EVERY`] seconds — so a "loading screen stuck" report is
/// a one-line diagnosis instead of a session. Ordinary loads finish under the threshold and log
/// nothing extra.
const WAIT_LOG_AFTER: f32 = 3.0;
const WAIT_LOG_EVERY: f32 = 2.0;
/// How far a same-map snap must move the body before its destination is treated as a **load** at
/// all (yards). Under this, a teleport cannot outrun the streamer — the ground and buildings you
/// land on are the ones you left — and the raise below would only be able to flash a cover over
/// content that happened to be arriving anyway. Sized to clear every *combat* relocation with
/// room to spare (charge/intercept 25 yd, blink 20 yd, the knockbacks under 30), because those
/// end in a server teleport too and a black screen mid-fight is the one thing this must never
/// do; the reported case — `.tele` across a city — is 458 yd.
const SNAP_LOAD_MIN_YD: f32 = 100.0;

/// Consecutive covered+in-world frames before the cover counts as **on the glass**.
///
/// Renders are serial, so at 3 the two intermediate frames' renders have committed their
/// presents. This was 0962's argument and its constant; it now lives beside the fact it defines
/// rather than being restated in each consumer (it had already been copied twice, and the third
/// consumer — the world camera — never got it at all).
const COVER_PRESENT_FRAMES: u32 = 3;

/// **Is the entry cover ON THE GLASS?** — the one fact 0962's rule is written in terms of, held
/// in one place because restating it is exactly how it gets forgotten.
///
/// The world-entry raise happens in `Update`; the state flips a frame later, so the FIRST
/// covered+in-world frame is also the first frame whose render can draw the cover. Anything
/// synchronous on that frame holds the *previous* present — the character-select screen, frozen
/// — for its whole duration. That is the director's report, three times now (0962, 1345, and the
/// world camera below), and each time it was one consumer nobody had counted:
///
/// - the **pipeline-warm menagerie** (0962) — its own `covered_frames`, now this;
/// - the **world-entry FrameXML load** (1345) — its own `covered_frames`, now this;
/// - the **world camera and the booth wake** — never deferred at all, and measured (this record's
///   round) as **57 of the flip frame's 60 ms**: the first render of a 3 000-entity world with
///   cold pipelines, plus fifteen booth cameras woken by the warm pass, all on the one frame that
///   owes the glass a loading screen.
///
/// Counted in `First` so a `PreUpdate` reader (the entry load, an exclusive system) and an
/// `Update` reader (the warm pass, the camera gate) see the same frame's answer.
#[derive(Resource, Default)]
pub(crate) struct EntryCover {
    /// Consecutive covered+in-world frames; reset the moment either goes false.
    frames: u32,
}

impl EntryCover {
    /// Has the cover had enough frames to reach the glass? **True whenever no cover is up** —
    /// there is then no glass to protect, and a consumer that waited would wait forever (the
    /// capture that boots straight in-world is exactly this case).
    pub(crate) fn presented(&self) -> bool {
        self.frames == 0 || self.frames >= COVER_PRESENT_FRAMES
    }

    /// Is a cover up and still owed its first present? The inverse of the arm above that a
    /// *renderer* wants: "do not draw anything but the cover this frame".
    pub(crate) fn owes_a_present(&self) -> bool {
        self.frames > 0 && self.frames < COVER_PRESENT_FRAMES
    }

    /// **Is a world cover up right now?** — `LoadingScreen::covering()` ∧ in world, which is
    /// the pair every cover consumer actually means, counted once here.
    pub(crate) fn covering(&self) -> bool {
        self.frames > 0
    }

    /// Covered frames so far — what the tests assert on.
    #[cfg(test)]
    pub(crate) fn frames(&self) -> u32 {
        self.frames
    }
}

impl EntryCover {
    /// One frame's worth of counting — the whole rule, so the test seam and the system cannot
    /// drift apart.
    pub(crate) fn tick(&mut self, covered: bool) {
        self.frames = if covered {
            self.frames.saturating_add(1)
        } else {
            0
        };
    }
}

/// `First`: advance (or reset) the covered-frame count. One writer, read by every consumer.
fn count_entry_cover(
    screen: Res<LoadingScreen>,
    state: Res<State<crate::char_select::ClientState>>,
    mut cover: ResMut<EntryCover>,
) {
    let in_world = *state.get() == crate::char_select::ClientState::InWorld;
    cover.tick(screen.covering() && in_world);
}

/// Bevy resource wrapper around the format-crate [`LoadingScreenCatalog`] (the `LoadingScreenID` → BLP
/// path table). Paired with [`MapCatalogRes`] (the `mapId` → `LoadingScreenID` FK) to resolve art.
#[derive(Resource)]
struct LoadingScreenCatalogRes(LoadingScreenCatalog);

/// What a raise means for the tip of the day (decision 2077).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TipEdge {
    /// The glue→world entry: pick the next row and advance the cursor.
    Pick,
    /// Every other raise: this screen carries no tip.
    Clear,
}

/// Loading-screen state machine (decision 0737: event-raised, readiness-cleared).
#[derive(Resource, Default)]
pub(crate) struct LoadingScreen {
    active: bool,
    /// A raise from an entry edge (the pick's `Connected`, `SMSG_TRANSFER_PENDING`) holds until the
    /// destination snap (worldport/teleport) actually lands — so the OLD location's readiness can
    /// never clear a screen raised for the NEW one. This is what closes the world-entry flash: the
    /// login-vista tiles are fully resident the moment the glue tears down, and without this hold
    /// that residency would clear the screen seconds before `SMSG_LOGIN_VERIFY_WORLD` arrives.
    awaiting_snap: bool,
    /// **The map to resolve this screen's art from** — the reference's `[0x82f00c]`, set by every
    /// raise: the roster's `Character.map` at the pick edge, the transfer's map at
    /// `SMSG_TRANSFER_PENDING`, the map we are on for a backstop raise. `None` is the reference's
    /// `-1`: *this screen draws no backdrop* — the boot default, and what a failed resolve leaves
    /// behind ([`Self::art_resolved`]).
    ///
    /// It is deliberately not `CurrentMap`, which moves under a live screen. Log out inside a
    /// dungeon whose instance is gone by the time you come back and vmangos relocates you *during*
    /// `Player::LoadFromDB` (`Player.cpp`, the `GetGoBackTrigger` arm) — no
    /// `SMSG_TRANSFER_PENDING`, no `SMSG_NEW_WORLD`, just a `SMSG_LOGIN_VERIFY_WORLD` naming a
    /// different map than the roster row the screen was raised from. Reading the art per frame
    /// from `CurrentMap` made the dungeon backdrop switch to the continent one mid-load; nothing
    /// on the wire can do that in the reference ([`Self::snap_landed`]).
    ///
    /// **This field is not the latch, though — [`Self::art_resolved`] is** (decision 2087). Five
    /// writers touch `[0x82f00c]` image-wide, all inside the `LoadingScreen.cpp` TU: `0x4067e6`
    /// and `0x4072d3` store a map id (the two transition entries), and `0x406d0e`/`0x4073d4`/
    /// `0x407e59` store `-1`. A raise landing on a screen that is already up re-points it
    /// *unconditionally* — and repaints nothing, because the texture is already resolved.
    map: Option<u32>,
    /// **Has this screen resolved its backdrop yet?** — the reference's `[0x882e04]`, the loaded
    /// texture handle, and the thing the art is actually latched by (decision 2087). `0x406cf0`
    /// reads the map id only to reject `-1`, then `0x406cfc`/`0x406d01`/`0x406d03` skip the
    /// resolver `0x406e20` outright whenever the handle is non-null; the handle's three writers
    /// image-wide are the resolver's two stores and `0x407ed7 = 0` inside the dismiss `0x407e80`.
    ///
    /// So the map id decides the picture on **exactly one frame per screen**, and nothing that
    /// happens afterwards can change it. Cleared by [`Self::dismiss`], never by a raise.
    art_resolved: bool,
    /// **Plain black cover, no art, no bar** — the logout transition (decision 0738). The logout
    /// teardown despawns the avatar and the streamed world in the same frame's Net stage, but
    /// `CharSelect` (and the glue screen with it) only applies at the NEXT frame's state
    /// transition — without this the dead world renders uncovered for that frame, and it is
    /// exactly the teardown-burst frame, so it lingers. The ref's world→glue swap shows black
    /// there too. Dropped the moment the state leaves `InWorld` (the glue owns the screen from
    /// then, at a higher z); any real raise replaces it with the full screen.
    blackout: bool,
    /// Consecutive presentable frames while active (see [`CLEAR_AFTER_READY_FRAMES`]).
    ready_frames: u32,
    /// Monotonic bar fill, 0..1 — only ever advances within a load (reset to 0 on each activation), so
    /// the bar reads as one continuous stream even though the raw residency ratio dips as `desired`
    /// shifts while the view moves. A real loading bar never goes backwards.
    displayed: f32,
    /// Decoded backdrop art by BLP path, so repeated teleports to a continent don't re-decode.
    art_cache: HashMap<String, Handle<Image>>,
    /// **Which tip the next raise should carry** — `Pick` on the glue→world entry, `Clear` on every
    /// other raise, taken by [`crate::game_tip::drive_game_tip`] on the same frame. A field rather
    /// than a message because it is a property OF the raise, and the raise is this struct's.
    pub(crate) tip_edge: Option<TipEdge>,
    /// **Capture only** — the clear below is skipped while this is set. A loading screen is a
    /// picture that is up for a second and then gone: nothing in the tree could photograph one,
    /// which is how the tip of the day shipped dead and stayed dead through a live smoke run
    /// (decision 2083). Written by [`Self::hold_for_capture`] and by nothing a player can reach.
    held: bool,

    /// `Time::elapsed_secs` at the last raise + at the last wait-instrument line (see
    /// [`WAIT_LOG_AFTER`]).
    active_since: f32,
    last_wait_log: f32,
    /// The avatar's position as of LAST frame — so a snap's own displacement is knowable here
    /// (the snap is applied in `Input`, a stage before this one). Read only by the snap raise
    /// below, which uses it to tell a relocation from a spell's little hop.
    last_pos: Option<Vec3>,
}

impl LoadingScreen {
    /// Whether the opaque loading backdrop currently covers the frame — the world camera renders
    /// under it (pipeline warm-up), never behind the glue screens (decision 0540).
    pub(crate) fn covering(&self) -> bool {
        self.active
    }

    /// An active cover, for tests that drive covered-frame accounting (the entry-load deferral)
    /// and the cursor's covered arm.
    #[cfg(test)]
    pub(crate) fn test_covering() -> Self {
        Self {
            active: true,
            ..Self::default()
        }
    }

    /// Take the raise's tip edge, if one is pending — read once, by
    /// [`crate::game_tip::drive_game_tip`].
    pub(crate) fn take_tip_edge(&mut self) -> Option<TipEdge> {
        self.tip_edge.take()
    }

    /// **The capture instrument** (`WOW_CAPTURE=loading-tip`): raise the screen the way the
    /// glue→world entry does — art, bar and a `Pick` edge for the tip — and then never let go, so
    /// the harness's settle window has something to hold still on. The one screen in the client
    /// whose whole existence is measured in the seconds before it disappears, made photographable.
    /// `map` is the destination the art latches to, exactly as a real raise's is ([`Self::map`]):
    /// the harness knows it from the scenario, and with `None` the shot would be a black box with a
    /// tip on it rather than the real `Map.dbc` → `LoadingScreens.dbc` → BLP chain.
    pub(crate) fn hold_for_capture(&mut self, map: Option<u32>) {
        self.active = true;
        self.blackout = false;
        self.awaiting_snap = false;
        self.map = map;
        self.held = true;
        self.tip_edge = Some(TipEdge::Pick);
    }

    /// Raise the screen for a fresh load. `awaiting_snap` marks a raise whose destination snap is
    /// still in flight; `map` is the destination this screen's art is LATCHED to for its whole life
    /// ([`Self::map`]) — every caller states what it knows, and a caller with no announced
    /// destination passes the map we are on rather than leaving the art to follow `CurrentMap`.
    ///
    /// **Only an edge that would reach the reference's own transition entries calls this.** The
    /// destination *snap* is not one of them ([`Self::snap_landed`]).
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

    /// **The destination snap landed** — `SMSG_NEW_WORLD` or `SMSG_LOGIN_VERIFY_WORLD`. It ends the
    /// wait a raise armed, and it does **nothing else**: not the art, not the tip, not the bar.
    ///
    /// The two are one [`crate::net::WorldportMessage`] here and two handlers in the reference
    /// (`0x401b00` and `0x401de0`), and *neither touches the screen*:
    /// the raise `0x406800` has three call sites and all three are inside the `LoadingScreen.cpp`
    /// TU, so nothing on the wire can re-raise or re-point a screen that is already up. Treating
    /// the snap as a fresh load is what swapped the backdrop mid-load ([`Self::map`]), restarted
    /// the bar, and — a round-trip into *every* world entry — wiped the tip of the day that
    /// decision 2077 had just picked.
    ///
    /// `map` is the destination the snap names — `None` for a same-map teleport ack, which cannot
    /// disagree with the screen by construction.
    fn snap_landed(&mut self, map: Option<u32>) {
        // Worth a line on its own: the destination the server actually seated us on is not the one
        // this screen was raised for, i.e. we were relocated between the roster and the login.
        if let (Some(raised), Some(dest)) = (self.map, map) {
            if raised != dest {
                info!("loading screen: snap landed on map {dest} — raised for map {raised}, art holds");
            }
        }
        self.awaiting_snap = false;
    }

    /// **The far snap's whole rule** — the one place it lives, so the test seam and the system
    /// cannot drift apart (the [`EntryCover::tick`] pattern, two structs up).
    ///
    /// A screen that is already up **is this snap's screen** — the entry raise waiting for
    /// `SMSG_LOGIN_VERIFY_WORLD`, the portal's waiting for `SMSG_NEW_WORLD` — so the snap ends its
    /// wait and nothing more. Returns `true` only when no screen is up at all, which is the
    /// caller's cue to raise one: our backstop for a server-initiated port that arrived with no
    /// announcing edge (the reference needs no such backstop — its own world load blocks).
    #[must_use]
    fn far_snap(&mut self, map: u32) -> bool {
        if !self.active {
            return true;
        }
        self.snap_landed(Some(map));
        false
    }

    /// **The dismiss** (the reference's `0x407e80`) — the one place the screen comes down, so the
    /// things a screen owns for its lifetime come down with it exactly once. The tip is one of
    /// those: `[0x882e10]`'s only writers image-wide are the `EnterWorld` setter `0x406630` and
    /// `0x407f2b` inside this dismiss, so a *raise* never clears a tip — only this does.
    fn dismiss(&mut self) {
        self.active = false;
        self.blackout = false;
        self.tip_edge = Some(TipEdge::Clear);
        // `0x407ed7 mov ds:0x882e04,edi` — the next screen resolves its own art, and only here.
        self.art_resolved = false;
    }
}

// UI entity markers.
#[derive(Component)]
struct LoadingRoot;
#[derive(Component)]
struct LoadingBackdrop;
#[derive(Component)]
struct LoadingBarFill;
#[derive(Component)]
struct LoadingBarBorder;
/// The tip-of-the-day text block (decision 2077) — its `Text` root; the coloured runs are its
/// children, rebuilt when the shown tip changes.
#[derive(Component)]
pub(crate) struct LoadingTip;

pub(crate) struct LoadingScreenPlugin;

impl Plugin for LoadingScreenPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadingScreen>()
            .init_resource::<EntryCover>()
            // `First`, ahead of every consumer in `PreUpdate` and `Update` — see [`EntryCover`].
            .add_systems(First, count_entry_cover)
            .add_systems(Startup, setup_loading_screen.after(AssetSet::Open))
            // In `WorldStage::Present` (after Input + Stream): we read residency for the SAME frame's
            // player position — a teleport snaps in Input → the streamer recomputes focus in Stream →
            // we cover it here. Visibility set now propagates in PostUpdate and renders this frame, so
            // the swap never flashes.
            .add_systems(Update, drive_loading_screen.in_set(WorldStage::Present));
        // …and the other half of what a cover *is*: while it is up, the client takes no input
        // (`input`). Wired here rather than as a plugin of its own so the cover and its input rule
        // can never be registered apart.
        input::build(app);
    }
}

/// Startup: load the LoadingScreens catalog + the bar texture stack off the shared chain, and spawn
/// the (initially hidden, then activated on the first `drive` frame) UI tree.
fn setup_loading_screen(
    mut commands: Commands,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Root: fullscreen black — this IS the pillarbox letterbox, and it spawns UNCONDITIONALLY.
    // Every `covering()` consumer (the world camera stays active to warm pipelines, the warm
    // pass, the settle hold, and now the audio hold) behaves as if covered whenever
    // `LoadingScreen::active` is true — so a missing catalog or bar texture must degrade to a
    // plain black cover, never to "no cover node at all" while the whole client pretends one is
    // up (the naked-world failure this used to be). Flex-centres the 4:3 content area.
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
            // Above the 3D scene; UI already paints after the main pass, this orders within UI.
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

    // The two layers 1.12 actually draws (sRGB clamp sprites — UI art, not tiling world art).
    let mut tex = |path: &str| assets.sprite_texture(path, &mut images);
    let Some(border) = tex("Interface\\Glues\\LoadingBar\\Loading-BarBorder.blp") else {
        error!("loading bar textures missing — plain black cover, no art");
        return;
    };
    let fill = tex("Interface\\Glues\\LoadingBar\\Loading-BarFill.blp").unwrap_or_default();
    // The spawned ImageNodes below own these handles (the root is never despawned, only hidden), so
    // the textures stay resident without a separate holder resource.

    commands.entity(root).with_children(|root| {
        // The whole loading screen renders in one 4:3 area fit to viewport height, centred, with
        // the root's black showing as pillarbox bars L/R (the reference's behaviour — the glue/load
        // screen is authored 4:3; on a wider window it letterboxes). Width = height·4/3 in vh.
        // The backdrop + bar are children, so their verified fractions are relative to THIS area.
        // Sibling paint order = spawn order: backdrop → fill → border (border on top).
        root.spawn(Node {
            width: Val::Vh(100.0 * BACKDROP_ASPECT),
            height: Val::Vh(100.0),
            position_type: PositionType::Relative,
            ..default()
        })
        .with_children(|area| {
            // Backdrop art: the square BLP stretched to fill the 4:3 area (mild widen — what the
            // reference does). Starts black (default-white texture tinted) until the art resolves.
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
            // The tip of the day (decision 2077), drawn between the background quad and the
            // progress bar — the reference's own order (`0x406e12`/`0x406e18`). The block is
            // positioned in PERCENT of this 4:3 area, which is exactly the space
            // `crate::game_tip`'s constants are in; only the font size and the shadow need pixels,
            // and those follow the window each frame. Empty and hidden until a glue->world raise
            // fills it.
            area.spawn((LoadingTip, crate::game_tip::tip_bundle()));
            // Fill — left-anchored; width = progress·FILL_MAX_WIDTH (set each frame). y from the
            // BOTTOM (verified by screenshot + binary). The fill art is a horizontally-uniform
            // gradient, so width-scaling reads as a left→right reveal.
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

/// Per-frame: observe the lifecycle edges, run the trigger/clear state machine, resolve backdrop
/// art, and push the progress fraction into the bar.
#[allow(clippy::type_complexity)]
fn drive_loading_screen(
    mut screen: ResMut<LoadingScreen>,
    // The streamer's two published facts, bundled into one param (Bevy's 16-element system-param
    // ceiling, the same squeeze `player::control` and `feed_ui_input` already pay): what is
    // resident, and whether that residency is even ABOUT the player's own ground this frame.
    stream: (
        Res<WorldLoadProgress>,
        Res<benilla_world::terrain_stream::ViewFocus>,
    ),
    current_map: Option<Res<CurrentMap>>,
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
    // **Real, not virtual** — this clock measures a LOAD, and `Time<Virtual>` clamps any frame
    // longer than 250 ms (`transport.rs` names the same constant for the same reason). A loading
    // screen is made of exactly those frames, so the default clock under-reports it: measured on
    // one smoke re-entry, a cover that was up for 2.24 s wall reported 1.2 s. An instrument whose
    // whole job is "how long was this up, and what was blocking it" cannot run on a clock that
    // discards the stalls.
    time: Res<Time<Real>>,
    // The 0837 warm pass: the cover holds until the menagerie's pipelines have compiled — the
    // point of the cover is that NOTHING first-sight-compiles after it lifts.
    warm: Res<crate::pipe_warm::WarmPass>,
    // The deferred world-entry UI load (0962's frame accounting, applied to 1051's burst):
    // armed at the entry edge, run behind this cover a few frames in. While it is still
    // pending the reveal would show a world with no interface — the reference's reveal always
    // has the UI up, because its UI load happens inside its blocking world load.
    entry_ui_pending: Option<Res<crate::ui_script::PendingEntryUiLoad>>,
    // The lifecycle edges (decisions 0737/0738), all observed from what the net bridge already
    // publishes — this module owns the whole state machine, nothing else is instrumented for it.
    edges: (
        Res<State<crate::char_select::ClientState>>,
        MessageReader<crate::net::EnteredWorldMessage>,
        MessageReader<crate::net::WorldportMessage>,
        MessageReader<crate::net::TeleportMessage>,
        MessageReader<crate::net::LoggedOutMessage>,
        MessageReader<crate::net::DisconnectedMessage>,
        MessageReader<crate::net::CharacterLoginFailedMessage>,
        Option<Res<crate::net::PendingTransfer>>,
        Option<Res<crate::char_select::Roster>>,
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
    let (progress, focus) = (&stream.0, &stream.1);
    let now = time.elapsed_secs();
    let map_id = current_map.as_ref().map(|m| m.0);
    // The physics hold releases on the same residency signal that clears this screen (decision
    // 0737 — never on ground contact), so waiting for it costs nothing and guarantees the safe
    // order: the body's world is live before the reveal, never a reveal of a body about to drop.
    let player_settling = player.as_ref().is_some_and(|p| p.settling);

    // --- The edges. ---
    // A real glue entry (not 0065's seamless in-world reconnect, which must stay seamless): the
    // same frame `enter_on_connected` flips the state that tears the glue down, this raise puts
    // the cover up — raise and teardown are atomic by construction. The destination snap
    // (`SMSG_LOGIN_VERIFY_WORLD`) is still a server character-load away; `awaiting_snap` holds the
    // screen across that gap. Art resolves at once from the roster's own `Character.map`.
    if entered.read().next().is_some() && *state.get() != crate::char_select::ClientState::InWorld {
        let map = roster.as_ref().and_then(|r| r.pending_map());
        screen.raise("world entry", true, map, now);
        // **The tip of the day rides THIS edge and no other** (decision 2077): the reference's
        // setter `0x406630` has exactly one caller, inside `CGlueMgr::EnterWorld`, and neither
        // `SMSG_TRANSFER_PENDING` arm reaches it — so a portal or worldport screen carries no tip.
        // The pick itself is `crate::game_tip`'s, one system over; this list is at Bevy's
        // sixteen-parameter ceiling and the tip needs three resources of its own.
        screen.tip_edge = Some(TipEdge::Pick);
    }
    // A portal walk-in (`SMSG_TRANSFER_PENDING`, no transport): the server is about to unload us
    // and the `SMSG_NEW_WORLD` snap follows after its own load — cover now, like the reference.
    // Transport crossings (0455) keep riding visibly until their worldport lands below.
    if let Some(t) = transfer.as_ref().filter(|t| t.is_changed()) {
        match &t.0 {
            Some(info) if info.transport_entry.is_none() => {
                screen.raise("transfer pending", true, Some(info.map_id), now);
            }
            // The latch cleared with no snap in flight = `SMSG_TRANSFER_ABORTED` (a worldport
            // clears it too, but that path also lands below this frame): stop awaiting, and the
            // still-resident old world clears the screen through the ordinary debounce.
            None => screen.awaiting_snap = false,
            Some(_) => {}
        }
    }
    // The snap ([`LoadingScreen::far_snap`]): it never re-raises a live screen, so the art that
    // screen was raised with holds even when the server seats us on a different map than the one
    // it announced. That is the reference exactly — `0x406800` is reachable from nothing outside
    // the loading-screen TU, so no packet can re-point a screen that is up.
    for w in worldports.read() {
        if screen.far_snap(w.map_id) {
            screen.raise("worldport", false, Some(w.map_id), now);
        }
    }
    // A same-map teleport just ends any awaited snap — whether it needs a screen at all is the
    // backstop's call below (a summon across the room shouldn't flash one).
    let teleported = teleports.read().next().is_some();
    if teleported {
        screen.snap_landed(None);
    }
    // Logout (decision 0738): the same frame's Net stage despawned the avatar and tore the world
    // down, but `CharSelect` only applies at the NEXT frame's state transition — cover the dead
    // world with plain black until the glue owns the screen. No art, no bar: the ref's
    // world→glue swap is a black cut, not a loading screen.
    // A dead session is the same world→glue cut (decision 1262), and it needs this arm more than
    // logout does: the entry race raises the cover on `EnteredWorldMessage` a few lines up and
    // arms `awaiting_snap` for a snap the dead socket will never send, so without the disarm here
    // the cover is what the player would have been left staring at.
    let session_over = lost.read().any(|m| m.session_over);
    // A **refused character login** is the same cut, and it needs the disarm as badly as a dead
    // session does: the entry edge above raised the cover with `awaiting_snap` armed for a snap
    // the server has just told us will never come, and nothing else in this function can end that
    // wait. Left alone it is a loading screen with no world behind it and no way off.
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
    if screen.blackout && *state.get() != crate::char_select::ClientState::InWorld {
        // The glue is up (it renders above this root); the cover's job is done.
        screen.dismiss();
    }

    // --- Backstop trigger: the ground under the view focus isn't resident and nothing raised us
    // (a far same-map `.tele`, or any path with no edge). Normal streaming keeps the focus tile
    // resident, so this never fires while walking.
    //
    // **In world only** (decision 0777). It used to fire at boot as well, and that was load-bearing
    // by accident: a raised screen keeps the world camera active (0540), which is what compiled the
    // world's pipelines behind the glue. But it only worked because the world was being streamed
    // behind the glue in the first place — with no world to load there, the same trigger would put
    // an invisible screen up forever against residency that never arrives. The warm-up is not lost,
    // it MOVED: the world now loads under the cover this same function raises on world entry, which
    // is where a warm-up belongs. ---
    //
    // **And only while the focus IS the body** ([`ViewFocus::follows_body`]). The trigger's whole
    // premise is "residency says the ground under the player is missing and no edge announced it,
    // so a load must be happening". While the eye is deliberately elsewhere — a cinematic fly-by,
    // a free-fly — residency describes the *camera's* tile, and reading it as a fact about the
    // player is reading someone else's ground (decision 1336's lesson, one consumer over).
    // Measured: every cinematic raised this cover the instant it started (`.debug play cinematic
    // 41` → "loading screen: up (focus not resident, map None)" in the same millisecond as the
    // shot), covering the opening seconds of the fly-by the deferred-start latch exists to protect
    // — and, once the body took the settle hold for the same detachment, covering ALL of it,
    // because this cover's clear waits on that hold and that hold waits on the focus coming home.
    if !screen.active
        && !progress.focus_resident
        && focus.follows_body()
        && *state.get() == crate::char_select::ClientState::InWorld
    {
        // Same map by construction — the destination this covers is the ground under the body.
        screen.raise("focus not resident", false, map_id, now);
    }

    // --- …and the same test at the SNAP, on everything the reveal actually needs. The backstop
    // above watches one term — the focus TERRAIN tile — and a teleport inside the streamer's keep
    // band (up to ~1600 yd, three tiles) lands on ground that is already resident, so it never
    // fires. The destination's *buildings* are a different question: their placements may still be
    // spawning, and since the retained pass (1429) a spawned building still has a bake between it
    // and the screen. That is how `.tele` across a city put the player down in a Stormwind with no
    // Stormwind in it, uncovered, for the frames it took to arrive.
    //
    // Only at a snap, and only against the same predicate that CLEARS the screen: a summon across
    // a room whose world is already there raises nothing (no flash), and ordinary walking — which
    // crosses tile lines with placements pending all day — is not a snap and cannot reach this. ---
    let body = player.as_ref().map(|p| p.pos);
    let relocated = match (teleported, screen.last_pos, body) {
        (true, Some(was), Some(now_pos)) => was.distance(now_pos) >= SNAP_LOAD_MIN_YD,
        // No previous position to compare (the entry frame): treat the snap as a relocation, the
        // conservative arm — a covered load is recoverable, a naked one is what this closes.
        (true, _, _) => true,
        _ => false,
    };
    screen.last_pos = body;
    if relocated
        && !screen.active
        && *state.get() == crate::char_select::ClientState::InWorld
        && !(progress.is_ready() && progress.presentable())
    {
        screen.raise("teleport, destination not presentable", false, map_id, now);
    }

    // --- Clear: the destination snap has landed, the scene is presentable (tiles + focus
    // placements + colliders — [`WorldLoadProgress::presentable`]), and the physics hold is done,
    // sustained a few frames. ---
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
            // The wait instrument (0737): a screen up past the threshold names the blocking term.
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

    // --- Visibility. The root is the cover; the backdrop art + bar hide under blackout (0738's
    // plain black cut — the root's black background IS the frame then). ---
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

    // --- Backdrop art — `0x406cf0`'s gate in shape (decision 2087): the map id is read ONCE per
    // screen, to resolve the texture; from then on the resolved texture is what draws, and only
    // the dismiss clears it. A raise landing on a live screen re-points [`LoadingScreen::map`] and
    // repaints nothing, which is why "the screen you were given is the screen you keep" survives
    // even a transfer arriving mid-load. Resolution walks the FK chain and caches by path. ---
    if !screen.art_resolved {
        // Until this screen's own art lands, the backdrop is the root's black — never the LAST
        // screen's texture, which is what a stale tint would leave on the glass (the reference
        // holds a null handle here and does not draw the quad at all).
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
                None => assets.sprite_texture(&path, &mut images).inspect(|h| {
                    screen.art_cache.insert(path.clone(), h.clone());
                }),
            });
            match (handle, backdrop.single_mut()) {
                (Some(handle), Ok((mut img, _))) => {
                    img.image = handle;
                    img.color = Color::WHITE; // reveal the art (was tinted black)
                    screen.art_resolved = true;
                    // Once per screen, and the only place the picture is decided — so "which
                    // backdrop did that load actually show" is a log line rather than a capture.
                    info!("loading screen: backdrop for map {map_id} resolved");
                }
                // The resolve failed — no `LoadingScreens` row, or the BLP is missing. This screen
                // shows no backdrop for the rest of its life and does not retry: `0x406d0e` writes
                // `-1` into the map id for exactly this, and `0x406cf0` early-returns on it after.
                _ => {
                    warn!("loading screen: no backdrop for map {map_id} — plain black this load");
                    screen.map = None;
                }
            }
        }
    }

    // --- Progress bar: monotonic fill, reveals left→right, width = progress·FILL_MAX_WIDTH.
    // While the snap is still in flight the residency being published is the OLD location's
    // (fully resident) — hold the bar at zero until the destination's own numbers exist. ---
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

    /// **The one fact three consumers now share** ([`EntryCover`]). Two of them each kept their
    /// own copy of this count (0962's menagerie, 1345's FrameXML load) and the third — the world
    /// camera, which renders the whole arriving world on the same frame — never had one at all,
    /// which is the freeze this round measured. The arithmetic is trivial; that it has exactly one
    /// home is the point.
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

        // A reveal (or leaving the world) drops it back to the uncovered arm in one frame — the
        // next raise must not inherit the last one's credit.
        cover.tick(false);
        assert_eq!(cover.frames(), 0);
        assert!(cover.presented());
        cover.tick(true);
        assert!(!cover.presented(), "the next raise starts its own count");
    }
    /// **The director's report, as an assertion.** Log out inside a dungeon, come back hours
    /// later, and the instance is gone: vmangos relocates the character to the dungeon's go-back
    /// trigger *inside* `Player::LoadFromDB`, before the world is entered — so no
    /// `SMSG_TRANSFER_PENDING` and no `SMSG_NEW_WORLD` ever announce it, and the first the client
    /// hears is `SMSG_LOGIN_VERIFY_WORLD` naming a map the roster row did not. The screen keeps
    /// the art it was raised with, for its whole life. The reference could not do otherwise:
    /// `[0x82f00c]` has no writer on either world-load handler's path (`0x401b00`/`0x401de0`), and
    /// the raise `0x406800` is reachable from nothing outside the `LoadingScreen.cpp` TU.
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
            "and the tip 2077 just picked survives it — a raise is the only thing that could \
             have cleared it, and no raise happened"
        );
        assert!(
            (screen.displayed - 0.4).abs() < f32::EPSILON,
            "nor does the bar restart mid-load"
        );
        assert!(screen.active, "the screen stays up across its own snap");
    }

    /// The other half of the same rule: a cross-map port that arrived with **no** announcing edge
    /// still gets a cover. This is ours, not the reference's — its own world load blocks, so it
    /// needs no backstop.
    #[test]
    fn a_snap_with_nothing_covering_it_asks_for_a_raise() {
        let mut screen = LoadingScreen::default();
        assert!(screen.far_snap(1), "nothing is covering this load");
        assert!(
            !screen.active,
            "far_snap answers the question; the caller does the raising, with its own reason"
        );
    }

    /// **The tip comes down with the screen, never with a raise** — `[0x882e10]`'s only writers
    /// image-wide are `EnterWorld`'s setter `0x406630` and `0x407f2b`, inside the dismiss
    /// `0x407e80`. Ours used to clear it on every non-entry raise, which meant the login snap wiped
    /// the tip a server round-trip into *every* world entry.
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

    /// **What a live screen SHOWS is latched by the resolved texture, not by the map id**
    /// (decision 2087, correcting 2081's reading). A raise onto a screen that is already up
    /// re-points `[0x82f00c]` unconditionally — the reference's `0x406640`/`0x4072c0` have no
    /// guard on that store — and repaints nothing, because `0x406cf0` skips the resolver whenever
    /// `[0x882e04]` is non-null. Only the dismiss `0x407ed7` clears it.
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
}
