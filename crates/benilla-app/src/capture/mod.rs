//! Deterministic capture mode — the machine half of the Phase-5 visual A/B harness (decision 0008).
//!
//! With `$WOW_CAPTURE=<scenario>` set, the app boots server-less (net disabled in `main`), pins the
//! game-clock and the camera to a named viewpoint, waits for the rendered image to stop changing,
//! writes one PNG of the primary window to `$WOW_CAPTURE_OUT`, and exits. The render rework (decision 0008) is the
//! single riskiest change in the architecture — it can regress the whole look at once — so it goes
//! behind this harness: baselines are captured on the *current* pipeline, then every rework step is
//! diffed against them by the `benilla-visual` tool, catching regressions by machine before the
//! director's eye. Driven by `scripts/visual.sh`.
//!
//! Captures contain Blizzard-derived imagery (rendered terrain/models), so they live under the
//! gitignored `target/visual/` and are never committed.
//!
//! ## What makes a capture reproducible — two mechanisms, and that is all
//! "Deterministic" is the whole claim: a golden diff is evidence only if two runs of one unchanged
//! build agree. Pinning the camera, the game clock and the clutter seed is not enough.
//!
//! 1. **The shutter waits for the IMAGE to stop changing** ([`STABLE_FRAMES`], [`FrameWatch`],
//!    decision 0815). The scene is built when the rendered frame stops moving — measured, by reading
//!    the framebuffer back and comparing bytes. This replaced four *proxies* for the same question
//!    (tile residency, `scene_ready`, outstanding placements, and a world-entity-count quiescence
//!    counter), each of which was partial by construction and each of which had been added after the
//!    previous one was caught missing something. Streaming, pipeline warm-up, late placements, late
//!    M2 loads and the loading-screen fade are all *visible in the frame* — or they do not affect the
//!    shot, in which case they were never the harness's business.
//! 2. **The game clock is frozen** ([`CAPTURE_FRAME_DT`], [`hold_clock`], decision 0723), because the
//!    sims integrate in *seconds* while the harness counts *frames*. Held while the scene is being
//!    built, then released for exactly [`age_frames`] fixed steps, so the shot's sim age is the same
//!    duration on any machine.
//!
//! **Neither one fixes the known residual flake, and no gate here can.** `scripts/visual.sh
//! selfcheck` still finds a small number of scenarios that land in one of exactly two states. The
//! cause is not timing: it is that our draw order among *coplanar, equal-depth* batches follows spawn
//! order, and spawn order varies because asset loads complete on a thread pool (0723's Open section,
//! reached again by 0810 and 0815). Both states are perfectly stable — so waiting longer, waiting for
//! quiescence, or waiting for pixel stability cannot help. It is a renderer defect that the harness
//! correctly *reports*; do not try to gate it away here. Read 0815 before touching this file's phases.
//!
//! **`creature-indoor-front` is one of them** (named 2026-09-01, while A/B-ing 1787's fog gate): two
//! runs of ONE unchanged build land in the two states at MAE 4.518, the whole difference confined to
//! the subject's own brightness with the room around it pixel-identical, and the settle counts differ
//! (235 vs 211 frames). If a change you are A/B-ing shows exactly 4.518 here, re-run the baseline
//! before believing it — the number is the flake's, not yours.
//!
//! ## Running one capture by hand
//! **Run through Cargo — never the built binary directly:**
//! ```text
//! WOW_CAPTURE=ui-unitframes \
//!     WOW_CAPTURE_OUT=/tmp/shot.png cargo run -q -p benilla
//! ```
//! (`WOW_DATA` is only needed for a non-standard install — the client finds one in the project
//! folder or beside the binary on its own, `benilla_formats::wow_data`, decision 1175.)
//!
//! `cargo run` rebuilds first; a bare `target/debug/benilla` can silently be **stale code** — the
//! classic way a capture "disproves" a fix that was never in the binary. Staleness is now the
//! *only* trap here, and this note is worth keeping for why. Before 0993, `assets/` resolved
//! through Cargo's runtime `CARGO_MANIFEST_DIR`, so a bare run loaded **no** WGSL at all and every
//! custom-shader layer — the entire player UI, sky, liquid, models — rendered blank; it was read
//! as a UI bug for hours once. 0993 patched that by baking an absolute source-tree path in at
//! compile time, which fixed the capture and left a binary that worked only on the machine that
//! built it. 1175 deleted the path instead: every shader is compiled into the binary and addressed
//! `embedded://<crate>/shaders/…`, so there is no asset root left to resolve wrongly.
//! A **`ui-*` scenario opts the player UI in on its own** — it declares a `ui:` fixture, which is
//! this table saying the window is the subject ([`scenarios::ui_opted_in`], and read its doc
//! for why that is a correctness fix and not a convenience). `WOW_CAPTURE_UI=1` remains, for what
//! it was always actually for: painting the UI over a **world** scenario's shot. World baselines
//! stay UI-free by default either way. `WOW_CAPTURE=list`
//! prints the scenario names. `scripts/visual.sh` wraps all of this.

use std::path::Path;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot, ScreenshotCaptured};
use bevy::time::TimeUpdateStrategy;

use benilla_assets::coords::wow_to_bevy;

use crate::perf::PerfHud;
use benilla_world::dev_state::DebugState;
use benilla_world::schedule::WorldStage;
use benilla_world::terrain_stream::WorldLoadProgress;
use benilla_world::view::WorldCamera;

// The UI fixture seeding (the synthetic window states), the scenario table, and the live-run
// probe instruments (`probes`) each live in their own file — the server-less harness (settle,
// screenshot, perf probe) is this one's concern.
mod depth_probe;
mod fixtures;
mod live_shot;
mod phase_probe;
mod pick_probe;
mod probe_auction;
mod probe_bank;
mod probe_bg;
mod probe_bg_queue;
mod probe_binder;
mod probe_book;
mod probe_castcancel;
mod probe_charcreate;
mod probe_charter;
mod probe_chest;
mod probe_clam;
mod probe_crossing;
pub(crate) mod probe_env;
mod probe_gm_ticket;
mod probe_goquest;
mod probe_guard_poi;
mod probe_mail;
mod probe_melee;
mod probe_model_camera;
mod probe_partner;
mod probe_rig;
mod probe_service;
mod probe_stone;
mod probe_taxi;
mod probe_vendor_swap;
mod probes;
mod scenarios;
use crate::run_mode::CaptureMode;
pub(crate) use depth_probe::DepthProbePlugin;
use fixtures::seed_ui_fixture;
pub(crate) use live_shot::LiveShotPlugin;
pub(crate) use phase_probe::PhaseProbePlugin;
pub(crate) use pick_probe::PickProbePlugin;
pub(crate) use probe_auction::ProbeAuctionPlugin;
pub(crate) use probe_bank::ProbeBankPlugin;
pub(crate) use probe_bg::ProbeBgPlugin;
pub(crate) use probe_bg_queue::ProbeBgQueuePlugin;
pub(crate) use probe_binder::ProbeBinderPlugin;
pub(crate) use probe_book::ProbeBookPlugin;
pub(crate) use probe_castcancel::ProbeCastCancelPlugin;
pub(crate) use probe_charcreate::ProbeCharCreatePlugin;
pub(crate) use probe_charter::ProbeCharterPlugin;
pub(crate) use probe_chest::ProbeChestPlugin;
pub(crate) use probe_clam::ProbeClamPlugin;
pub(crate) use probe_crossing::ProbeCrossingPlugin;
pub(crate) use probe_gm_ticket::ProbeGmTicketPlugin;
pub(crate) use probe_goquest::ProbeGoQuestPlugin;
pub(crate) use probe_guard_poi::ProbeGuardPoiPlugin;
pub(crate) use probe_mail::ProbeMailPlugin;
pub(crate) use probe_melee::ProbeMeleePlugin;
pub(crate) use probe_model_camera::ProbeModelCameraPlugin;
pub(crate) use probe_partner::ProbePartnerPlugin;
pub(crate) use probe_rig::ProbeRigPlugin;
pub(crate) use probe_service::ProbeServicePlugin;
pub(crate) use probe_stone::ProbeStonePlugin;
pub(crate) use probe_taxi::ProbeTaxiPlugin;
pub(crate) use probe_vendor_swap::ProbeVendorSwapPlugin;
pub(crate) use probes::{
    fx_draw_census_plugin, DressCensusPlugin, EntityCensusPlugin, GroundCensusPlugin,
    JitterMeterPlugin, LiftCensusPlugin, LiveFpsPlugin, NodeProbePlugin, ParticleCensusPlugin,
    ProbeChatPlugin, ProbeClock, ProbeDragPlugin, ProbeExitPlugin, ProbeFocusPlugin,
    ProbeHoverPlugin, ProbeKeyPlugin, ProbeLuaPlugin, ProbeResizePlugin, RevealAuditPlugin,
    SchedCensusPlugin, StallPlugin, TrailCensusPlugin, UnitVisualsPlugin,
};
pub(crate) use scenarios::ui_opted_in;
use scenarios::GlueScreen;
use scenarios::{Scenario, SubjectKind, UiFixture, GLUE_SCENARIOS, GROUND_EYE, SCENARIOS};

pub(crate) mod fxview;
// The three scripted probe drivers, which lived in `player/` until decision 1174: they turn the
// avatar's aim, aim its swim pitch, and park the camera rig, and all three order themselves BEFORE
// `player::control`. An instrument may name the gameplay system it runs against; gameplay may not
// name the instrument.
mod probe_cam;
mod probe_look;
mod probe_pitch;
pub(crate) mod waterfx;

pub(crate) use probe_cam::ProbeCamPlugin;
pub(crate) use probe_look::ProbeLookPlugin;
pub(crate) use probe_pitch::ProbePitchPlugin;

/// Which screen a capture starts the client on — the dev arm of [`crate::run_mode::start_state`],
/// which is what `main` actually calls. A glue capture boots onto the screen it photographs; any
/// other capture boots straight in-world (no net, no picker); with no capture at all this is the
/// ordinary login screen.
///
/// The **third** independent reader of `$WOW_CAPTURE`, and deliberately so: `run_mode` asks it for
/// the app and `dev_state::deterministic_run` asks it for the engine, because after 1160's split
/// each layer must be able to ask with nothing above it. One environment variable, three readers,
/// no shared symbol across either boundary. Keep them in step.
pub(crate) fn start_state() -> crate::char_select::ClientState {
    match glue_screen() {
        Some(GlueScreen::CharCreate) => crate::char_select::ClientState::CharCreate,
        Some(GlueScreen::Login) => crate::char_select::ClientState::Login,
        None if crate::run_mode::scenario_active() => crate::char_select::ClientState::InWorld,
        None => crate::char_select::ClientState::Login,
    }
}

/// Is `$WOW_CAPTURE` naming a **glue** screen? Consulted before the plugins build, because
/// the answer decides which screen the client starts on — a glue capture is the one kind of
/// capture that must NOT boot straight into the world.
fn glue_screen() -> Option<GlueScreen> {
    let name = std::env::var("WOW_CAPTURE").ok()?;
    GLUE_SCENARIOS
        .iter()
        .find(|g| g.name == name)
        .map(|g| g.screen)
}

/// The `fxview` fixture request — the **effect-viewer instrument**: spawn one effect/missile
/// model (full rig + emitters + ribbons + cards, the same `attach_effect_visuals` body the game
/// uses), let it run `age` seconds, and shoot it from a chosen angle. The agent's own eye on
/// spell visuals: every "this effect looks wrong from angle X" report becomes a reproducible
/// headless capture instead of a director round-trip. Not a golden scenario (deliberately
/// excluded from `print_scenario_names` — output depends on the model/age/angle knobs):
///
/// ```text
/// WOW_DATA=<Data> WOW_CAPTURE=fxview WOW_FX_MODEL='Spells\DemonArmor_Impact_Head.mdx' \
///   WOW_FX_AGE=1.2 WOW_FX_AZ=60 WOW_FX_EL=15 WOW_CAPTURE_OUT=/tmp/fx.png cargo run -q -p benilla
/// ```
///
/// **`WOW_FX_DISPLAY=<CreatureDisplayInfo id>` swaps the subject onto the UNIT path** — the same
/// component set a streamed creature gets, seated on the terrain, with everything that hangs off
/// being a unit (tag alpha, the distance-fade gate, anim LOD, the emitter's sequence host). Model
/// lane vs unit lane at one knob is what turns "does this creature look wrong?" into a headless
/// A/B instead of a director round-trip:
///
/// ```text
/// WOW_DATA=<Data> WOW_CAPTURE=fxview WOW_FX_DISPLAY=1132 WOW_FX_AGE=6 WOW_FX_EL=20 \
///   WOW_FX_DIST=12 WOW_CAPTURE_OUT=/tmp/vw.png cargo run -q -p benilla
/// ```
///
/// **`WOW_FX_GO=<GameObjectDisplayInfo id>` swaps the subject onto the GAMEOBJECT path** — the
/// third lane, and the one a placed trap/door/chest actually takes: the wire component set a
/// streamed GO gets, so `crate::go_anim`'s state machine (`0x5f3cb0`) — not the effect pool —
/// chooses the sequence. `WOW_FX_GO_STATE` (`GAMEOBJECT_STATE`, default 1 READY) and
/// `WOW_FX_GO_TYPE` (`GAMEOBJECT_TYPE_ID`, default 6 TRAP) select the substate;
/// `benilla-extract goanimscan` *predicts* what that resolves to, and this *shows* it. The lane
/// matters because an absent `GAMEOBJECT_STATE` reads as the wire default `0` = ACTIVE, which on a
/// model with no `Opened` sequence lands somewhere else entirely:
///
/// ```text
/// WOW_DATA=<Data> WOW_CAPTURE=fxview WOW_FX_GO=3073 WOW_FX_GO_STATE=1 WOW_FX_AGE=4 \
///   WOW_FX_DIST=3 WOW_CAPTURE_OUT=/tmp/go.png cargo run -q -p benilla
/// ```
///
/// Knobs: `WOW_FX_MODEL` (required, internal path), `WOW_FX_AGE` (seconds after attach, default
/// 1.0), `WOW_FX_AZ`/`WOW_FX_EL` (camera orbit degrees, default 0/10), `WOW_FX_DIST` (yards,
/// default 5), `WOW_FX_FLY` (yd/s along the model's facing — a missile only trails in motion;
/// default 0), `WOW_FX_YAW` (model facing, degrees, default 0), `WOW_FX_TURN` (deg/s the fixture
/// keeps turning after spawn — a host that changes heading mid-effect, which is how you see that
/// a world-mode cloud does NOT swing with it (decisions 1585/1591; the "heading-since-birth fan"
/// this knob was built for turned out not to exist); default 0), `WOW_FX_GROUND` (=1 seats the
/// fixture ON the terrain via a down-ray — required to see a ground-anchored effect's projected
/// surface decals, `crate::ground_fx`; default 0 = the mid-air point), `WOW_FX_HOLD` (=1 keeps
/// the fixture alive past one sequence pass — previewing a persistent HOLD kit's steady state;
/// default 0 = the game's discrete-instance reap at one pass, then the pool drains),
/// `WOW_FX_AT` (`x,y,z` — plant the fixture at a real world point instead of the Northshire
/// hillside, with `WOW_MAP` picking the map and `WOW_FX_MINUTE` the clock; the ground-decal lanes
/// need real terrain under the subject),
/// `WOW_FX_UP` (yards to raise the fixture above its resolved seat — for models authored below
/// their anchor, whose opening frames the terrain otherwise swallows; default 0).
#[derive(Resource)]
pub(crate) struct FxViewRequest {
    pub(crate) model_path: String,
    /// `WOW_FX_DISPLAY=<CreatureDisplayInfo id>` — spawn the subject as a real **unit** (the live
    /// `NetEntity` component set, the same path a streamed creature takes) instead of attaching it
    /// as an effect. The two lanes differ in everything that hangs off being a unit — material tag
    /// alpha, the distance-fade gate, anim LOD, the emitter's sequence host — so switching this one
    /// knob is the A/B that says whether a defect belongs to the model or to the unit path.
    /// `WOW_FX_MODEL` is then optional (the display id names the model).
    pub(crate) display: Option<u32>,
    /// `WOW_FX_GO=<GameObjectDisplayInfo id>` — spawn the subject as a real **GameObject**, so
    /// [`crate::go_anim`]'s state machine picks the sequence instead of the effect pool's
    /// "play clip 0". A placed trap renders through this lane and nothing else, so it is the only
    /// honest A/B for "does the trap look right".
    pub(crate) go: Option<u32>,
    /// `WOW_FX_GO_STATE` — the `GAMEOBJECT_STATE` the fixture's descriptor carries (default 1 =
    /// READY, what vmangos spawns a trap in). Set it to 0 to see what an omitted field renders as.
    pub(crate) go_state: u32,
    /// `WOW_FX_GO_TYPE` — the `GAMEOBJECT_TYPE_ID` (default 6 = TRAP). Decides whether
    /// [`crate::go_anim::go_animates`] puts the instance on the machine at all.
    pub(crate) go_type: u32,
    /// `WOW_FX_SCALE` — the unit lane's `NetEntity::scale` (the wire scale a creature carries).
    pub(crate) scale: f32,
    pub(crate) age: f32,
    pub(crate) az_deg: f32,
    pub(crate) el_deg: f32,
    pub(crate) dist: f32,
    pub(crate) fly: f32,
    pub(crate) yaw_deg: f32,
    /// See `WOW_FX_TURN` above (deg/s).
    pub(crate) turn: f32,
    /// See `WOW_FX_GROUND` above.
    pub(crate) ground: bool,
    /// See `WOW_FX_HOLD` above.
    pub(crate) hold: bool,
    /// `WOW_FX_AT=x,y,z` — plant the fixture at this raw WoW point instead of [`FXVIEW_POS`]
    /// (with `WOW_MAP` picking the map, like `vista` and `waterfx`). The ground-decal lanes —
    /// blob shadow, footprints, the selection ring — only exist *on* terrain, and which terrain
    /// decides whether they exist at all (footprints gate on the surface's `TerrainType.Flags & 1`,
    /// i.e. snow and sand) and how hard the coplanar tie is (a steep slope seen at a grazing
    /// angle is the whole difficulty). A fixture nailed to one Northshire hillside can photograph
    /// none of that.
    pub(crate) at: Option<[f32; 3]>,
    /// `WOW_FX_UP` — raise the fixture this many yards above its resolved seat (default 0).
    /// The escape hatch for models authored BELOW their anchor (Arcane Intellect's star cluster
    /// starts 1.6 yd under its attach point): at the default seat the terrain swallows the
    /// opening of the animation, and no camera angle can look through the ground.
    pub(crate) up: f32,
}

/// The fixture's live state, written by `fxview::drive_fx_view` and the phase
/// driver below.
#[derive(Resource, Default)]
pub(crate) struct FxViewState {
    /// Set by the capture driver once the scene has settled — the fixture spawns only then, so
    /// a one-shot effect's age at the shot is the REQUESTED age, not age + settle time.
    pub(crate) armed: bool,
    pub(crate) root: Option<Entity>,
    /// `time.elapsed_secs()` at the frame the visuals attached — the age clock's zero.
    pub(crate) attached_at: Option<f32>,
    /// The fixture ran its one sequence pass and the root was reaped (the game's discrete-kit
    /// completion callback, mirrored so captures past the span tell the truth: emitters drain,
    /// they don't pour). `WOW_FX_HOLD=1` disables the reap.
    pub(crate) expired: bool,
}

/// Where the fxview effect spawns: mid-air over the Northshire slope (raw WoW coords), inside
/// the ground scenario's streamed tiles, high enough that terrain never intersects the model.
pub(crate) const FXVIEW_POS: [f32; 3] = [-8960.0, -145.0, 90.0];

/// How far the `vista` fixture seats its eye above the position it is given (yd) — a standing human's
/// camera pivot, so pasting a `.go xyz` straight off the debug panel frames what the director saw
/// rather than a worm's-eye view from inside the ground.
const VISTA_EYE_HEIGHT: f32 = 2.0;

/// Print the BASELINE scenario names, one per line — the single source of truth
/// `scripts/visual.sh` reads so the driver never drifts from the code. Invoked by `main` for
/// `WOW_CAPTURE=list`. On-demand fixtures (the UI look-pass windows, sun/moon/sky, house-compass)
/// are deliberately absent: the blessed sweep is the director-chosen spot×time set only, so a
/// `visual.sh baseline` opens six windows on their screen and not thirty (decision 0632).
pub(crate) fn print_scenario_names() {
    for s in SCENARIOS {
        println!("{}", s.name);
    }
}

/// Consecutive byte-identical framebuffer readbacks that mean "the scene is built" (decision 0815).
///
/// This is the harness's ONE residency test, and it is a measurement rather than a proxy: whatever is
/// still arriving — a tile, a placement, an M2 that just finished loading, a pipeline wgpu has not
/// specialised yet (until it is warm Bevy draws only the clear colour, which is why a blind settle
/// could photograph a uniform fog-blue frame) — either changes the image, and resets this counter, or
/// does not affect the shot at all.
///
/// 30 frames (~0.5 s of held-clock frames) is the same confidence window the entity-count counter it
/// replaced used. Raise it with `$WOW_CAPTURE_STABLE` for a scene with a very slow tail; this is the
/// knob that used to be `WOW_CAPTURE_SETTLE`, and unlike that one it buys *evidence* rather than a
/// guess at a duration.
const STABLE_FRAMES: u32 = 30;

/// The effective stability window — see [`STABLE_FRAMES`].
fn stable_frames() -> u32 {
    std::env::var("WOW_CAPTURE_STABLE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(STABLE_FRAMES)
}

/// Hard cap on held frames spent waiting for [`STABLE_FRAMES`], so a scene whose image never settles
/// (a UI fixture with a live animation) shoots anyway rather than hanging — forfeiting the guarantee,
/// loudly. Deliberately generous: reaching it means the shot is not reproducible.
const BUILD_CAP_FRAMES: u32 = 1800;

/// Wall-clock ceiling on a whole capture run, deadline exceeded → `AppExit::error()`.
///
/// **Every other bound in this harness counts FRAMES, and that is only a bound while frames keep
/// arriving.** On 2026-08-26 macOS stopped granting drawables to our window — `-[CAMetalLayer
/// nextDrawable]` parked on its own ~1 s internal timeout, over and over, verified in the stall
/// sampler's own captures (758 of ~1000 samples in `semaphore_timedwait_trap` beneath
/// `CAMetalLayerPrivateNextDrawableLocked`, with `frame hitch: 1005..1019 ms` repeating in the log).
/// At that rate [`BUILD_CAP_FRAMES`] is not 30 seconds of patience, it is **thirty minutes**, and
/// every agent-driven capture that session read as a dead terminal with no error and no clue.
///
/// So the ceiling that matters is the one measured in the units the failure is measured in. It is
/// not a retry, a fallback or a heuristic: a capture either produces its image or fails, and this is
/// the line that makes the second one *happen* instead of hanging. The message it prints carries the
/// observed frame rate, because "47 frames in 300 s" is the diagnosis and a bare timeout is not.
///
/// The starvation itself is macOS's, not ours — the compositor stops recycling presented drawables
/// for a window it is not compositing, and nothing in-process can hand them back. We bound our own
/// instrument; we do not fight the window server. Decision 1637.
const DEADLINE_SECS: u64 = 300;

/// The effective wall-clock ceiling — `$WOW_CAPTURE_DEADLINE=<secs>`, `0` to disable. A healthy
/// capture is seconds (`ui-bag` 7 s, a settled world scenario ~10 s), so 300 s is ~30× headroom and
/// cannot fire on a run that is merely slow.
fn capture_deadline() -> Option<Duration> {
    let secs = std::env::var("WOW_CAPTURE_DEADLINE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEADLINE_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Steps of [`CAPTURE_FRAME_DT`] the sims run, clock released, once the scene is built and
/// quiescent — the shot's *sim age*. 150 = 2.5 s: past the 2 s spawn appear-fade
/// (`model_fade::APPEAR_FADE_SECS`) and long past a flame pool's particle lifetime, so the scene is
/// at steady state and not mid-transient.
///
/// Because it is counted in frozen frames it is a **duration**, identically on any machine — which
/// is the whole point. `$WOW_CAPTURE_AGE` overrides it for a scene that needs longer to fill (snow:
/// flakes sink at 2-6.5 yd/s from ~22 yd up, so the column wants ~11 s ≈ 660).
const AGE_FRAMES: u32 = 150;

/// The effective aging window — see [`AGE_FRAMES`].
fn age_frames() -> u32 {
    std::env::var("WOW_CAPTURE_AGE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(AGE_FRAMES)
}

/// The frozen capture clock's frame step: every capture frame advances the game clock by exactly
/// this, whatever the frame really cost. Anything integrated in *seconds* — particle pools, ribbon
/// trails, animation, weather, fades — therefore reaches the shutter in the same state every run.
///
/// Without it the harness is only **frame**-pinned: the build window counts frames while the sims
/// advance on `time.delta_secs()`, i.e. real wall-clock, so two runs spend different real time per
/// frame (streaming, pipeline warm-up, whatever else the machine is doing) and photograph the flames
/// in different places. Measured on this tree before the change — the golden six captured twice from
/// one **unchanged** build: MAE up to 0.009 with max pixel deltas of 180, the same band a real render
/// change lands in. That noise floor is why decision 0721 could not read its own flame pixels, and
/// why 0719 read signal out of some that were partly run-to-run churn.
///
/// 1/60 s, so every settle window keeps the duration its comment claims (150 frames ≈ 2.5 s) instead
/// of buying however much sim time the machine happened to grant.
const CAPTURE_FRAME_DT: Duration = Duration::from_nanos(16_666_667);

/// Hard cap on how long to wait for the screenshot save to land, so a capture never hangs the harness.
const SAVE_TIMEOUT_FRAMES: u32 = 120;
/// A couple of grace frames after the save completes before exiting, so the file is flushed.
const EXIT_GRACE_FRAMES: u32 = 3;

/// Frames the perf probe discards after uncapping vsync (present-mode switch + pipeline settle).
const PROBE_WARMUP_FRAMES: u32 = 60;

/// Marks a framebuffer readback taken to test scene stability, so the real shot's `Capturing` wait
/// (see [`Phase::Saving`]) cannot mistake one for the screenshot it is waiting on.
#[derive(Component)]
struct StabilityShot;

/// The image-stability tracker: the previous framebuffer, and how many consecutive readbacks have
/// matched it byte for byte (decision 0815). Bytes, not a hash — the buffer is tens of MB at most,
/// a `memcmp` is free next to the readback itself, and an exact compare needs no collision argument.
#[derive(Resource, Default)]
struct FrameWatch {
    /// The last framebuffer read back, or `None` before the first one lands.
    prev: Option<Vec<u8>>,
    /// Consecutive readbacks identical to their predecessor. Reset to 0 by any change.
    stable: u32,
    /// A readback is outstanding. Only ever one at a time, so `stable` counts *distinct* frames
    /// rather than however many requests the GPU happened to retire together.
    in_flight: bool,
}

/// Observer for a stability readback: compare against the previous frame, count, and free the slot.
fn watch_frame(shot: On<ScreenshotCaptured>, mut watch: ResMut<FrameWatch>) {
    watch.in_flight = false;
    let Some(bytes) = shot.image.data.as_ref() else {
        return; // no readback payload (a zero-sized or unavailable target) — treat as no evidence
    };
    watch.stable = if watch.prev.as_deref() == Some(bytes.as_slice()) {
        watch.stable + 1
    } else {
        0
    };
    watch.prev = Some(bytes.clone());
}

/// Capture lifecycle. Advances one step per frame in [`drive_capture`].
#[derive(Clone, Copy)]
enum Phase {
    /// Clock held; waiting for the rendered image to stop changing ([`STABLE_FRAMES`]). This one
    /// phase replaced `Streaming` + `Settling` and the four world-state proxies they waited on — see
    /// this module's header.
    Building(u32),
    /// Scene built, clock released: running exactly [`age_frames`] steps of
    /// [`CAPTURE_FRAME_DT`] so every second-driven thing is at a known, machine-independent age.
    Aging(u32),
    /// fxview only: scene settled, fixture armed; waiting for the effect to attach and run its
    /// requested age before the shot.
    FxAging,
    /// Perf-probe mode (`$WOW_FPS_PROBE`): vsync off, discarding warm-up frames.
    ProbeWarmup(u32),
    /// Perf-probe mode: sampling frame times until the target count, then print + exit.
    Probing(u32),
    /// Screenshot requested; waiting for the async save (`Capturing` marker) to appear and clear.
    Saving { frames: u32, seen: bool },
    /// Save done; a few grace frames, then `AppExit`.
    Done(u32),
}

#[derive(Resource)]
struct CaptureCtx {
    /// The in-world viewpoint — `None` for a glue-screen capture, which has no world, no camera
    /// and no map. Every reader of it sits on a world path; the shutter itself needs neither.
    scenario: Option<Scenario>,
    /// The scenario's name, whichever table it came from — for the output path and the probe line.
    name: &'static str,
    out: String,
    phase: Phase,
    /// UI fixture already seeded (once, at residency) — see [`seed_ui_fixture`].
    ui_seeded: bool,
    /// `$WOW_FPS_PROBE` — sample this many frames (vsync off) instead of screenshotting, then print
    /// frame-time stats + scene counts and exit. The repeatable perf instrument: same scenario, same
    /// settle, numbers instead of pixels. 0 = normal capture.
    probe_frames: u32,
    /// The frozen clock is still in force ([`CAPTURE_FRAME_DT`]). True for the whole of every run
    /// that ends in a screenshot; a perf probe **starts** frozen and drops this the moment the scene
    /// is built and aged, because its entire measurement *is* the real frame cost.
    ///
    /// It used to be `probe_frames == 0`, decided once at build — i.e. a probe ran its whole
    /// `Building` phase on a live clock. That silently broke when 0815 replaced the settle proxies
    /// with the image-stability gate: on a live clock the sky drifts and the flames burn, so the
    /// image *cannot* stop changing and `watch.stable` never leaves 0. Every probe since has spent
    /// the full [`BUILD_CAP_FRAMES`] — 1800 frames, ~30 s, and 1800 full-resolution readbacks of a
    /// stability test that could not pass — and then printed a warning about a shot it never takes.
    /// Worse for a *perf* instrument: the scene it measured was however old 1800 real frames made
    /// it, which is a different age on every machine. Frozen build → deterministic [`age_frames`] →
    /// release is the same three steps a capture takes, and it is what makes the probe repeatable.
    /// Decision 1637.
    frozen_clock: bool,
    /// `$WOW_RESIZE=WxH` already applied (once, at first settle) — see the `Building` arm.
    resized: bool,
    /// Probe samples (frame ms).
    probe_samples: Vec<f32>,
    /// Process CPU seconds at the first sampled frame — the window baseline for the probe line's
    /// `cpu_ms`/`cpu_pct` (the load-robust metric, decision 0711; the scenario probe lacked it).
    probe_cpu_start: Option<f64>,
    /// Wall-clock start of the run, and how long it may take — see [`capture_deadline`]. A real
    /// [`Instant`], never `Time<Real>`: under the frozen clock `Time<Real>` is itself manual
    /// ([`CAPTURE_FRAME_DT`] per frame), so a harness that timed itself by it would measure the
    /// very frame count it is trying not to trust.
    started: Instant,
    deadline: Option<Duration>,
    /// Frames [`drive_capture`] has run, all phases — the denominator of the rate the deadline
    /// message reports.
    frames: u32,
    /// The deadline already fired; `AppExit` is written and no phase advances again.
    bailed: bool,
}

/// `$WOW_RESIZE=WxH` — resize the window to this (logical px) once the image first settles,
/// then settle again before shooting. The mid-session resolution-change instrument (see the
/// `Building` arm). `None` when unset or malformed.
fn resize_request() -> Option<(u32, u32)> {
    let v = std::env::var("WOW_RESIZE").ok()?;
    let (w, h) = v.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// The present mode a perf probe uncaps to (also the live probe's — see `probes/live_fps.rs`).
///
/// `AutoNoVsync`, measured, not assumed: explicit `Immediate` on macOS/Metal is a trap — A/B'd
/// 2026-07-27 (overlook-noon, release, twice each), it *rails* near 16.6 ms AND takes 1.0–1.5 s
/// stalls (the `-[CAMetalLayer nextDrawable]` timeout — drawable starvation), while `AutoNoVsync`
/// genuinely uncaps when macOS grants it (p50 12.7 ms on the same scene). 0362's "AutoNoVsync
/// doesn't uncap" was the *power state* withholding the grant, not the mode — no present mode
/// escapes that; `cpu_ms` on the probe line is the rail-proof metric. `WOW_PROBE_UNCAP=immediate`
/// re-runs the losing arm when macOS/wgpu move.
///
/// `WOW_PROBE_UNCAP=vsync` does not uncap at all: the leg keeps the player's present mode and
/// rails at the display's rate on purpose. It was built because the WindowServer's grant is not
/// ours to schedule — one sitting on the M2 Air read 123.5, 60.0, 60.0 and 87.5 fps across four
/// otherwise identical uncapped legs — and 1442's "railed-60.0 pairs only" rule discards every
/// pair the grant split. **Measured the same day, it is not a neutral instrument:** two
/// interleaved rounds at the Goldshire pin, no crowd, read `cpu_ms` 11.80 / 11.55 uncapped
/// (both railed 60.0 by the WindowServer anyway) against 14.65 / 12.43 under this arm — the
/// vsync wait costs the process CPU, and unevenly. So the default stays uncapped and a pair is
/// still accepted only when both legs happened to rail; this arm is for a sitting whose legs
/// refuse to rail at all, with the tax read against a same-arm baseline.
pub(crate) fn probe_uncap_mode() -> bevy::window::PresentMode {
    match std::env::var("WOW_PROBE_UNCAP").as_deref() {
        Ok("immediate") => bevy::window::PresentMode::Immediate,
        Ok("vsync") => bevy::window::PresentMode::AutoVsync,
        _ => bevy::window::PresentMode::AutoNoVsync,
    }
}

pub(crate) struct CapturePlugin;

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        // The `waterfx` fixture drives its synthetic wading dummy before the foam emitter reads
        // the motion — registered here, against the engine's ordering handle, because the
        // instrument is the one that knows it is an instrument.
        app.add_systems(
            Update,
            (waterfx::spawn, waterfx::drive)
                .chain()
                .before(benilla_world::water_fx::WaterFoamSet)
                .in_set(benilla_world::schedule::WorldStage::Present),
        );
        // The `fxview` fixture's driver, same principle and the same shape (decision 1174): it
        // creates the subject's display-cache entry, so it runs before the frame's build, inside
        // the entity-visuals set and after the net stage — exactly the slot it held while it was
        // an element of `EntitiesPlugin`'s chain, now stated rather than positional.
        app.add_systems(
            Update,
            fxview::drive_fx_view
                .in_set(crate::entities::EntityVisualsSet)
                .before(crate::entities::DisplayBuildSet)
                .after(benilla_world::schedule::WorldStage::Net),
        );
        let name = std::env::var("WOW_CAPTURE").unwrap_or_default();
        // A **glue** capture short-circuits everything below: no map to seed, no viewpoint to
        // pin, no fixture to open. Only the shutter is shared, and the shutter wants no world.
        let glue = GLUE_SCENARIOS.iter().find(|g| g.name == name).copied();
        if let Some(g) = glue {
            // The preview pick goes through the existing `WOW_CHARCREATE_PICK` instrument rather
            // than a second path into `CreateSelection` — same reason the map is seeded by env
            // (decision 0743): one route into a fact, whoever is asking.
            // …and an explicit pick in the environment outranks the scenario's default — the
            // per-race lever, so one scenario photographs every `UI_*` stage.
            if let Some((race, sex, class)) = g.pick {
                if std::env::var_os("WOW_CHARCREATE_PICK").is_none() {
                    std::env::set_var("WOW_CHARCREATE_PICK", format!("{race},{sex},{class}"));
                }
            }
        }
        // The fxview instrument: a synthetic scenario (ground scene, noon) + the fixture
        // request from env. Not in SCENARIOS — `scripts/visual.sh`'s golden sweep must never
        // run it (its output depends on the model/age/angle knobs, not just the name).
        let scenario: Option<Scenario> = if glue.is_some() {
            None
        } else {
            Some(if name == "fxview" {
                let at = std::env::var("WOW_FX_AT").ok().and_then(|v| {
                    let c: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
                    (c.len() == 3).then(|| [c[0], c[1], c[2]])
                });
                let display: Option<u32> = std::env::var("WOW_FX_DISPLAY")
                    .ok()
                    .and_then(|v| v.trim().parse().ok());
                let go: Option<u32> = std::env::var("WOW_FX_GO")
                    .ok()
                    .and_then(|v| v.trim().parse().ok());
                let model_path = match (std::env::var("WOW_FX_MODEL"), display.or(go)) {
                    (Ok(p), _) => p,
                    (Err(_), Some(_)) => String::new(), // the id lanes name their model by display id
                    (Err(_), None) => {
                        eprintln!(
                            "WOW_CAPTURE=fxview needs WOW_FX_MODEL=<internal .mdx/.m2 path>, \
                         WOW_FX_DISPLAY=<CreatureDisplayInfo id> or \
                         WOW_FX_GO=<GameObjectDisplayInfo id>"
                        );
                        std::process::exit(2);
                    }
                };
                let knob = |k: &str, d: f32| {
                    std::env::var(k)
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(d)
                };
                app.insert_resource(FxViewRequest {
                    model_path,
                    display,
                    go,
                    go_state: knob("WOW_FX_GO_STATE", 1.0) as u32,
                    go_type: knob("WOW_FX_GO_TYPE", 6.0) as u32,
                    scale: knob("WOW_FX_SCALE", 1.0),
                    age: knob("WOW_FX_AGE", 1.0),
                    az_deg: knob("WOW_FX_AZ", 0.0),
                    el_deg: knob("WOW_FX_EL", 10.0),
                    dist: knob("WOW_FX_DIST", 5.0),
                    fly: knob("WOW_FX_FLY", 0.0),
                    yaw_deg: knob("WOW_FX_YAW", 0.0),
                    turn: knob("WOW_FX_TURN", 0.0),
                    ground: knob("WOW_FX_GROUND", 0.0) > 0.5,
                    hold: knob("WOW_FX_HOLD", 0.0) > 0.5,
                    up: knob("WOW_FX_UP", 0.0),
                    // `WOW_FX_AT=x,y,z` plants the fixture anywhere in the real world, with
                    // `WOW_MAP` picking the map — the same pair `vista` and `waterfx` take. The
                    // default hillside can photograph an effect but not a ground decal: which
                    // surface is under the subject decides whether footprints exist at all, and
                    // how steeply it falls away decides how hard the coplanar tie is.
                    at,
                })
                .init_resource::<FxViewState>();
                Scenario {
                    name: "fxview",
                    map: None,
                    eye: GROUND_EYE, // overridden per frame by the orbit in `pin_scene`
                    look: at.unwrap_or(FXVIEW_POS),
                    minute: knob("WOW_FX_MINUTE", 720.0) as u32,
                    ui: None,
                }
            } else if name == "waterfx" {
                // The water-foam viewer (see `capture::waterfx`): a wading unit over a synthetic
                // wet lattice — or, with WOW_WFX_AT=x,y,z, in the real streamed liquid at that pin
                // — framed by a fixed orbit around the rig centre. Knobs: WOW_WFX_MODE
                // (ring|wake|turn), WOW_WFX_SPEED (yd/s), WOW_WFX_HEAD (deg), WOW_WFX_AGE (s),
                // WOW_WFX_DEPTH (yd),
                // camera WOW_WFX_AZ/EL/DIST. Not a golden scenario.
                let knob = |k: &str, d: f32| {
                    std::env::var(k)
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(d)
                };
                let mode = match std::env::var("WOW_WFX_MODE").as_deref() {
                    Ok("wake") => waterfx::WfxMode::Wake,
                    Ok("turn") => waterfx::WfxMode::Turn,
                    _ => waterfx::WfxMode::Ring,
                };
                // Rig centre in raw WoW coords. Default: over the Northshire ground scene, the
                // synthetic surface a few yards above the terrain so the backdrop plane reads
                // clean. `WOW_WFX_AT=x,y,z` moves it anywhere and switches the rig to the REAL
                // streamed liquid there (`z` = that water's surface height) — a synthetic square
                // of water has no bank to clip against and no neighbour to be sorted against.
                let at = std::env::var("WOW_WFX_AT").ok().and_then(|v| {
                    let c: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
                    (c.len() == 3).then(|| [c[0], c[1], c[2]])
                });
                let live = at.is_some();
                let center = at.unwrap_or([-8961.0_f32, -145.0, 95.0]);
                let az = knob("WOW_WFX_AZ", 180.0).to_radians();
                let el = knob("WOW_WFX_EL", 35.0).to_radians();
                let dist = knob("WOW_WFX_DIST", 14.0);
                let eye = [
                    center[0] + dist * el.cos() * az.cos(),
                    center[1] + dist * el.cos() * az.sin(),
                    center[2] + dist * el.sin(),
                ];
                app.insert_resource(waterfx::WaterFxView {
                    mode,
                    speed: knob("WOW_WFX_SPEED", 4.0),
                    heading: knob("WOW_WFX_HEAD", 0.0).to_radians(),
                    age: knob("WOW_WFX_AGE", 1.5),
                    center,
                    depth: knob("WOW_WFX_DEPTH", 0.5),
                    live,
                })
                .init_resource::<FxViewState>();
                Scenario {
                    name: "waterfx",
                    // The synthetic lattice sits over the Northshire slope; a live rig
                    // (`WOW_WFX_AT`) goes wherever its pin is, so its map is a knob like `vista`'s.
                    map: None,
                    eye,
                    look: center,
                    minute: 720,
                    ui: None,
                }
            } else if name == "vista" {
                // The **arbitrary-viewpoint** instrument: stand anywhere on the map, face any heading,
                // at any clock — the world half of what `fxview` is for effects. A director report that
                // arrives as "look at this horizon, here" (position, facing and time are all on the debug
                // panel, and `copy .go xyz` puts the position on the clipboard) becomes a reproducible
                // headless capture instead of a round-trip. Pair it with `WOW_FARCLIP` to match their
                // slider — horizon and fog artifacts live and die by the far-clip wall. Not a golden
                // scenario (its output depends on the knobs, not the name).
                //
                //   WOW_CAPTURE=vista WOW_VISTA_AT=-5841.9,-3802.4,-59.7 WOW_VISTA_FACE=24 \
                //     WOW_VISTA_MIN=1052 WOW_FARCLIP=320 WOW_CAPTURE_OUT=/tmp/v.png cargo run -q -p benilla
                //
                // Knobs: `WOW_VISTA_AT` (required, raw WoW `x,y,z` — the PLAYER position; the eye seats
                // `VISTA_EYE_HEIGHT` above it), `WOW_VISTA_FACE` (heading in degrees — the panel's
                // "facing" in its `(24°)` form; 0 = +X, counter-clockwise), `WOW_VISTA_PITCH` (degrees,
                // + = up, default 0 = level), `WOW_VISTA_MIN` (game minute of day, default 720 = noon).
                let knob = |k: &str, d: f32| {
                    std::env::var(k)
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(d)
                };
                let Some(at) = std::env::var("WOW_VISTA_AT").ok().and_then(|v| {
                    let c: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
                    (c.len() == 3).then(|| [c[0], c[1], c[2]])
                }) else {
                    eprintln!("WOW_CAPTURE=vista needs WOW_VISTA_AT=<x,y,z> (raw WoW coords)");
                    std::process::exit(2);
                };
                let face = knob("WOW_VISTA_FACE", 0.0).to_radians();
                let pitch = knob("WOW_VISTA_PITCH", 0.0).to_radians();
                let eye = [at[0], at[1], at[2] + VISTA_EYE_HEIGHT];
                // A look point far enough out that the framing is the heading, not the distance.
                let d = 500.0_f32;
                Scenario {
                    name: "vista",
                    // The arbitrary-viewpoint instrument goes anywhere, so its map is a knob: a
                    // horizon report from Kalimdor is `WOW_MAP=1` (a `Map.dbc` id).
                    map: None,
                    eye,
                    look: [
                        eye[0] + d * pitch.cos() * face.cos(),
                        eye[1] + d * pitch.cos() * face.sin(),
                        eye[2] + d * pitch.sin(),
                    ],
                    minute: knob("WOW_VISTA_MIN", 720.0) as u32,
                    ui: None,
                }
            } else if name == "name-close" {
                // The magnified overhead-name instrument (see `scenarios::NAME_CLOSE_AT`): the
                // `name-water` wolf, orbited by knob and looked straight at, so the ONE variable
                // is how many device pixels a glyph texel is drawn into. Not a golden scenario.
                let knob = |k: &str, d: f32| {
                    std::env::var(k)
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(d)
                };
                let at = scenarios::NAME_CLOSE_AT;
                // The name hangs `h` above the unit's FEET (the posed PlayerName attachment plus
                // the block's own line of rise) — that point is what the camera orbits and aims at.
                let name_at = [at[0], at[1], at[2] + knob("WOW_NAME_H", 1.4)];
                let (dist, az, el) = (
                    knob("WOW_NAME_DIST", 4.0),
                    knob("WOW_NAME_AZ", 124.0).to_radians(),
                    knob("WOW_NAME_EL", 8.0).to_radians(),
                );
                Scenario {
                    name: "name-close",
                    map: Some(scenarios::MAP_AZEROTH),
                    eye: [
                        name_at[0] + dist * el.cos() * az.cos(),
                        name_at[1] + dist * el.cos() * az.sin(),
                        name_at[2] + dist * el.sin(),
                    ],
                    look: name_at,
                    minute: 720,
                    ui: Some(UiFixture::NameWater),
                }
            // By name, EITHER table: the blessed six or an on-demand fixture. Only the sweep is
            // narrowed — every old viewpoint is still capturable by name (decision 0632).
            } else if let Some(&s) = SCENARIOS
                .iter()
                .chain(scenarios::ON_DEMAND.iter())
                .find(|s| s.name == name)
            {
                s
            } else {
                let glue_known: Vec<_> = GLUE_SCENARIOS.iter().map(|g| g.name).collect();
                let known: Vec<_> = SCENARIOS
                    .iter()
                    .chain(scenarios::ON_DEMAND.iter())
                    .map(|s| s.name)
                    .collect();
                eprintln!(
                "WOW_CAPTURE={name:?} is not a known scenario; choose one of: {known:?}, {glue_known:?} (or fxview, waterfx, name-close)"
            );
                std::process::exit(2);
            })
        };
        // Seed the continent the scenario names, BEFORE `world_map::load_world_map` reads it at
        // `Startup` — that is the single place `CurrentMap` is set for a server-less run, and the
        // terrain/WDL streamers and per-map lighting all key off it. Raw WoW coords repeat on every
        // continent, so a scenario that could not say which map it meant would stream the wrong
        // world's tiles and photograph a void (Felwood's tile `33_24` exists in Azeroth, empty).
        // The arbitrary-viewpoint instruments (`vista`, `waterfx`, `fxview`) carry `map: None`
        // instead: their map IS the knob, so writing it back would be a round trip through a second
        // parser — and a second parser is how `WOW_MAP=Kalimdor` came to photograph Azeroth in
        // silence. `world_map` is the one reader (decision 0743). A glue screen has no map.
        if let Some(m) = scenario.as_ref().and_then(|s| s.map) {
            std::env::set_var("WOW_MAP", m.to_string());
        }
        let shot_name = scenario.map(|s| s.name).or(glue.map(|g| g.name)).unwrap();
        let out = std::env::var("WOW_CAPTURE_OUT")
            .unwrap_or_else(|_| format!("target/visual/{shot_name}.png"));
        if let Some(parent) = Path::new(&out).parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "capture: cannot create output dir {}: {e}",
                    parent.display()
                );
                std::process::exit(2);
            }
        }
        let probe_frames = std::env::var("WOW_FPS_PROBE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        // The frozen capture clock ([`CAPTURE_FRAME_DT`]) — now for every run, probe included, and
        // released at a phase boundary rather than never installed (see `CaptureCtx::frozen_clock`).
        // A probe still measures a live clock: `drive_capture` restores
        // `TimeUpdateStrategy::Automatic` on the way into `ProbeWarmup`, before a single frame is
        // sampled, because `time.delta_secs()` under a manual strategy would report a flawless
        // constant 16.67 ms for any scene, however slow.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(CAPTURE_FRAME_DT))
            .add_systems(Startup, hold_clock);
        app.insert_resource(CaptureMode)
            .init_resource::<FrameWatch>()
            .insert_resource(CaptureCtx {
                scenario,
                name: shot_name,
                out,
                phase: Phase::Building(0),
                ui_seeded: false,
                probe_frames,
                frozen_clock: true,
                resized: false,
                probe_samples: Vec::new(),
                probe_cpu_start: None,
                started: Instant::now(),
                deadline: capture_deadline(),
                frames: 0,
                bailed: false,
            })
            .add_systems(Update, pin_scene.in_set(WorldStage::Present))
            // Before the UnitFeed pass: the seed stands in for wire data that in live play
            // filled the app caches on EARLIER frames, so the same frame's feeds (item-template
            // / player-req pushes, then the merchant paint) must all see it. Unordered, the
            // one-shot MERCHANT_SHOW paint races feed_item_stats and the usable reds are
            // flaky in the capture.
            .add_systems(Update, seed_ui_fixture.before(crate::ui_unit::UnitFeed))
            .add_systems(Last, drive_capture);
    }
}

/// Each frame, force the deterministic capture conditions: pinned time-of-day, no perf HUD, and the
/// fixed camera pose. Runs in `WorldStage::Present` (after `control` is gated off and after terrain
/// streaming reads the camera), so the harness is the sole, stable author of the view.
fn pin_scene(
    ctx: Res<CaptureCtx>,
    mut debug: ResMut<DebugState>,
    mut perf: ResMut<PerfHud>,
    fx_req: Option<Res<FxViewRequest>>,
    fx_state: Option<Res<FxViewState>>,
    mut player: ResMut<crate::player::Player>,
    roots: Query<&Transform, Without<WorldCamera>>,
    mut cam: Query<&mut Transform, With<WorldCamera>>,
) {
    perf.visible = false; // hidden by default since 2099, but a session may have chorded it up —
                          // a capture is pristine and UI-free whatever the run did
                          // A glue screen has no world to light, no clock to pin and no camera to place. The shutter
                          // above needs none of that — it is watching the framebuffer.
    let Some(scenario) = ctx.scenario else {
        return;
    };
    debug.lighting.follow_server_time = false;
    debug.lighting.manual_minute = scenario.minute;

    // `WOW_MM_PROBE=x,y,z` (raw WoW coords) drops the player at that point and marks them active, so
    // the interior minimap (which keys off `player.active` + `player.pos`) renders in a headless
    // capture — the harness otherwise leaves the player inactive at spawn, so interiors never show.
    // The camera looks straight down from above so the WMO streams in around it. Interior-minimap
    // debugging instrument (decision 0203 arc), not a golden scenario.
    let probe = std::env::var("WOW_MM_PROBE").ok().and_then(|s| {
        let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    });
    if let Some(p) = probe {
        player.pos = wow_to_bevy(p);
        player.active = true;
        player.detached = false;
        // Above and off to the side (not straight down — that degenerates `looking_at` with Y up).
        let eye = wow_to_bevy([p[0] - 25.0, p[1] - 25.0, p[2] + 40.0]);
        for mut t in &mut cam {
            *t = Transform::from_translation(eye).looking_at(wow_to_bevy(p), Vec3::Y);
        }
        return;
    }

    let (eye, look) = match (&fx_req, &fx_state) {
        // fxview: orbit the fixture's LIVE root (a flying missile moves; the camera tracks it)
        // at the requested azimuth/elevation/distance, aimed one yard up — the effect models
        // author their bodies ~0.5–1.5 units above their root.
        (Some(req), Some(state)) => {
            let root_pos = state
                .root
                .and_then(|r| roots.get(r).ok())
                .map(|t| t.translation)
                .unwrap_or_else(|| wow_to_bevy(FXVIEW_POS));
            let look = root_pos + Vec3::Y;
            let orbit = Quat::from_rotation_y(req.az_deg.to_radians())
                * Quat::from_rotation_x(-req.el_deg.to_radians());
            (look + orbit * (Vec3::Z * req.dist), look)
        }
        _ => (wow_to_bevy(scenario.eye), wow_to_bevy(scenario.look)),
    };
    for mut t in &mut cam {
        *t = Transform::from_translation(eye).looking_at(look, Vec3::Y);
    }
}

/// Hold the game clock at zero until the image stops changing; [`drive_capture`] releases it.
///
/// The fixed frame step alone is not enough, because it only makes each frame the same *size* — a
/// torch flame still ages from the frame its tile happened to spawn on, and tiles spawn under a
/// wall-clock budget (`terrain_stream`'s `SPAWN_BUDGET`) against asset I/O that finishes on a
/// different frame every run. Held through the whole build, every emitter in the scene is zero-age
/// when the clock is released, so at the shutter each has run exactly [`age_frames`] steps no matter
/// when it appeared. Frame-driven work (streaming itself, the spawn budget, the screenshot save) is
/// on the real clock and runs on regardless — which is what lets the scene finish arriving while the
/// sims stand still.
fn hold_clock(mut clock: ResMut<Time<Virtual>>) {
    clock.pause();
}

/// The three scene-population queries the `FPS_PROBE` line prints — bundled because they are one
/// concern (how much world is resident, and how much of it survived the cull) and because
/// `drive_capture` sits against Bevy's 16-parameter ceiling, which the CVar host's old knob
/// bundle hit first (retired by 2303).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ProbeCensus<'w, 's> {
    particles: Query<'w, 's, &'static benilla_world::particles::ParticleEmitter>,
    parts: Query<'w, 's, &'static ViewVisibility, With<benilla_world::model_render::ModelPart>>,
    entities: Query<'w, 's, ()>,
}

/// Drive the capture lifecycle: wait for streaming, settle, screenshot, exit.
fn drive_capture(
    mut ctx: ResMut<CaptureCtx>,
    mut watch: ResMut<FrameWatch>,
    // Disjoint from the stability readbacks by the marker: without it, one of those in flight would
    // satisfy this wait and the harness could call the real shot done before it was written.
    capturing: Query<(), (With<Capturing>, Without<StabilityShot>)>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    // `ResMut` for one reason: re-anchoring the clock at the probe's release — see `Phase::Aging`.
    mut time: ResMut<Time<bevy::time::Real>>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    census: ProbeCensus,
    fx_req: Option<Res<FxViewRequest>>,
    wfx_req: Option<Res<waterfx::WaterFxView>>,
    mut fx_state: Option<ResMut<FxViewState>>,
    // **The harness's one deliberate virtual clock** — the allowlisted exception in
    // `probes::probe_schedules_read_the_wall_clock`. The fixture age below is an age *on the clock
    // the effect animates on* — the same clock this system freezes at save time, one field down —
    // not a schedule. Everything schedule-shaped in the harness reads [`probes::ProbeClock`]
    // instead; decision 0789 says why, and what learning it cost.
    game_time: Res<Time>,
    mut clock: ResMut<Time<Virtual>>,
    // Switched to `Automatic` at the probe's release point — see `Phase::Aging`.
    mut time_strategy: ResMut<TimeUpdateStrategy>,
    // `Option`: the glue-screen capture path builds no composite lane, so there is no backdrop to
    // ask how big the world was — and those scenarios have no world to price anyway.
    backdrop: Option<Res<crate::world_backdrop::WorldBackdrop>>,
) {
    // The wall-clock ceiling, checked before anything else and in every phase — `Building` is
    // where the drawable starvation was caught, but `FxAging` waits on a fixture that may never
    // attach and `Saving` on a readback that may never land, and one deadline covers all three
    // where three per-phase frame caps would not ([`capture_deadline`]).
    if ctx.bailed {
        return;
    }
    ctx.frames += 1;
    if let Some(limit) = ctx.deadline {
        let elapsed = ctx.started.elapsed();
        if elapsed > limit {
            ctx.bailed = true;
            error!(
                "capture: DEADLINE — {} ran {:.0}s without finishing ({} frames, {:.1} fps). \
                 The harness bounds itself in frames, so this is what a machine that stopped \
                 granting frames looks like; at 1 fps the frame caps are half an hour. No \
                 image written. ($WOW_CAPTURE_DEADLINE=<secs>, 0 disables.)",
                ctx.name,
                elapsed.as_secs_f32(),
                ctx.frames,
                ctx.frames as f32 / elapsed.as_secs_f32(),
            );
            exit.write(AppExit::error());
            return;
        }
    }
    // Both fixture viewers (fxview / waterfx) share the build→arm→age→shoot flow; only the
    // requested age differs.
    let fixture_age = fx_req
        .as_ref()
        .map(|r| r.age)
        .or(wfx_req.as_ref().map(|r| r.age));
    ctx.phase = match ctx.phase {
        Phase::Building(n) => {
            // One readback at a time, requested only here — so a leftover can never be mistaken for
            // the real shot (the `StabilityShot` marker keeps `Saving`'s wait disjoint too).
            if !watch.in_flight {
                watch.in_flight = true;
                commands
                    .spawn((Screenshot::primary_window(), StabilityShot))
                    .observe(watch_frame);
            }
            let capped = n + 1 >= BUILD_CAP_FRAMES;
            if capped {
                warn!(
                    "capture: the image never settled in {} frames ({} stable); the shot is not \
                     reproducible",
                    n + 1,
                    watch.stable,
                );
            }
            // The emptiness tripwire (1373): stability proves the image stopped changing, not
            // that it contains anything — a lane that renders nothing is perfectly stable, and
            // the 1371 black era sailed through this gate without a word. The bytes are already
            // in hand; color channels only, since alpha is opaque even on a black frame.
            if watch.stable >= stable_frames() || capped {
                if let Some(px) = watch.prev.as_deref() {
                    if px
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0)
                    {
                        error!(
                            "capture: settled image is EMPTY — every color channel is zero; the \
                             scene rendered nothing (1373)"
                        );
                    }
                }
            }
            if watch.stable < stable_frames() && !capped {
                Phase::Building(n + 1)
            } else if let Some((rw, rh)) = resize_request().filter(|_| !ctx.resized) {
                // `$WOW_RESIZE=WxH` (logical px): a mid-session resolution change, applied only
                // once the image has SETTLED at the boot size — the whole UI has been built and
                // its text measured under the boot scale before the window changes, which is
                // exactly the fullscreen-toggle flow a fresh boot at the target size can't
                // exercise (stale text-metric caches were invisible to every same-size capture).
                // The stability watch restarts and the shot photographs the post-resize frame.
                ctx.resized = true;
                if let Ok(mut w) = windows.single_mut() {
                    w.resolution.set(rw as f32, rh as f32);
                }
                watch.stable = 0;
                info!("capture: resized to {rw}x{rh}, re-settling");
                Phase::Building(0)
            } else if fixture_age.is_some() {
                if let Some(state) = fx_state.as_deref_mut() {
                    state.armed = true; // scene ready — the fixture spawns now, age clock clean
                }
                Phase::FxAging
            } else {
                // Clock released here: the sims now run exactly `age_frames()` fixed steps, so the
                // shot's sim age is the same on any machine (decision 0723) — and a probe takes the
                // same road, so the scene it measures is that same fixed age instead of "however
                // old 1800 real frames left it" (`CaptureCtx::frozen_clock`).
                info!(
                    "capture: image settled after {} frames, aging {}",
                    n + 1,
                    age_frames()
                );
                Phase::Aging(0)
            }
        }
        Phase::FxAging => {
            // The fixture was armed when settling completed; shoot once it has attached and run
            // its requested age on the game clock.
            let aged = match (fixture_age, &fx_state) {
                (Some(age), Some(state)) => state
                    .attached_at
                    .is_some_and(|t0| game_time.elapsed_secs() - t0 >= age),
                _ => true,
            };
            if aged {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(ctx.out.clone()));
                info!("capture: effect aged, writing {}", ctx.out);
                Phase::Saving {
                    frames: 0,
                    seen: false,
                }
            } else {
                Phase::FxAging
            }
        }
        Phase::Aging(n) => {
            // No churn-restart here. The old one re-held the clock and restarted this window if the
            // world entity count moved — and 0723 measured that it **never fired**, because the
            // count-quiescence gate it shared a threshold with had already passed. What replaced it
            // is upstream and stronger: the image itself stopped changing before the clock was
            // released, so a straggler that would have mattered was already waited out (0815).
            if n + 1 < age_frames() {
                Phase::Aging(n + 1)
            } else if ctx.probe_frames > 0 {
                // Scene built and aged to a known duration; hand the clock back to real time
                // BEFORE a single frame is sampled, or `Phase::Probing` would read the manual
                // [`CAPTURE_FRAME_DT`] as its measurement and report a flawless 16.67 ms.
                ctx.frozen_clock = false;
                *time_strategy = TimeUpdateStrategy::Automatic;
                // Re-anchor `Time<Real>` to NOW, or the first automatic frame is billed for the
                // entire frozen phase. `update_with_duration` sets `last_update = last_update +
                // dt` — a fictional instant that falls behind reality by exactly the wall-clock
                // time spent frozen (~8 s here). The next `Automatic` tick does
                // `Instant::now() - that`, which lands as one multi-second delta: `perf::stats`
                // reported it as a phantom `frame hitch: 1060 ms`, and `update_virtual_time`
                // clamped it to `max_delta` and stepped every sim a quarter-second at once — a
                // spawn burst whose cost then landed in the samples (measured: p95 46-52 ms
                // against 18 ms before).
                //
                // Called from `Last`, so the huge delta it writes is overwritten by the next
                // frame's `time_system` before anything reads it, and it never reaches
                // `Time<Virtual>` at all — `update_virtual_time` only runs inside `time_system`.
                // What survives is the anchor: `last_update = now`, so frame one of the
                // measurement is an ordinary frame.
                time.update_with_instant(Instant::now());
                // Uncap presentation so we measure true frame cost, not the vsync ceiling.
                // `$WOW_PROBE_VSYNC=1` keeps vsync ON instead — the probe then measures the PRESENT
                // ceiling itself (what fps the display sync actually grants this window), the
                // instrument for "what is the vsync cap right now".
                let keep_vsync = std::env::var("WOW_PROBE_VSYNC").as_deref() == Ok("1");
                if !keep_vsync {
                    if let Ok(mut w) = windows.single_mut() {
                        w.present_mode = probe_uncap_mode();
                    }
                }
                info!(
                    "probe: scene aged {} frames; vsync {}, warming {PROBE_WARMUP_FRAMES} frames",
                    age_frames(),
                    if keep_vsync { "KEPT ON" } else { "off" }
                );
                Phase::ProbeWarmup(0)
            } else {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(ctx.out.clone()));
                info!("capture: scene aged, writing {}", ctx.out);
                Phase::Saving {
                    frames: 0,
                    seen: false,
                }
            }
        }
        Phase::ProbeWarmup(n) => {
            if n + 1 >= PROBE_WARMUP_FRAMES {
                ctx.probe_cpu_start = crate::perf::process_cpu_secs();
                Phase::Probing(0)
            } else {
                Phase::ProbeWarmup(n + 1)
            }
        }
        Phase::Probing(n) => {
            let ms = time.delta_secs() * 1000.0;
            ctx.probe_samples.push(ms);
            // `==`, not `>=`: `AppExit` takes a frame or two to drain, and the re-entered finish
            // branch used to print a second (301-frame) line in the gap.
            if n + 1 == ctx.probe_frames {
                let mut v = ctx.probe_samples.clone();
                v.sort_by(f32::total_cmp);
                let at = |q: f32| v[(((v.len() - 1) as f32) * q).round() as usize];
                let mean = v.iter().sum::<f32>() / v.len() as f32;
                let (emitters, active, live) = census
                    .particles
                    .iter()
                    .fold((0usize, 0usize, 0usize), |(e, a, l), p| {
                        (e + 1, a + usize::from(p.live() > 0), l + p.live())
                    });
                let px = windows
                    .single()
                    .map(|w| (w.physical_width(), w.physical_height()))
                    .unwrap_or((0, 0));
                // The pixels the GPU was actually asked for. `px` is the WINDOW, and since 1639
                // the world need not match it: a probe line that reported only the window would
                // silently price a 4x supersample as if it were a native frame.
                let world_px = backdrop.map_or(String::new(), |b| {
                    let s = b.render_size();
                    format!(" world_px={}x{}", s.x, s.y)
                });
                // Scene population: model submeshes (the per-frame visibility walk's N), how many
                // survived the cull to render, and the whole-world entity count — the scale terms
                // behind every O(N) per-frame cost (the Stormwind fps hunt's instrument).
                let (submeshes, drawn) = census.parts.iter().fold((0usize, 0usize), |(n, d), v| {
                    (n + 1, d + usize::from(v.get()))
                });
                let entity_count = census.entities.iter().len();
                // CPU cost per frame across every thread — the load-robust half of the measurement
                // (`perf::process_cpu_secs`), same fields as the live probe's line.
                let cpu = match (ctx.probe_cpu_start, crate::perf::process_cpu_secs()) {
                    (Some(t0), Some(t1)) => {
                        let per_frame_ms = (t1 - t0) * 1000.0 / v.len() as f64;
                        format!(
                            " cpu_ms={per_frame_ms:.2} cpu_pct={:.0}",
                            per_frame_ms / mean as f64 * 100.0
                        )
                    }
                    _ => String::new(),
                };
                // The present mode the window actually measured under — an uncap that silently
                // rails (0362) is only diagnosable if the line says what was asked for.
                let present = windows
                    .single()
                    .map(|w| format!(" present={:?}", w.present_mode))
                    .unwrap_or_default();
                // Machine-greppable one-liner + a human block. stdout, not the log, so a script can
                // capture it without log-filter noise.
                println!(
                    "FPS_PROBE scenario={} frames={} mean_ms={mean:.2} p50_ms={:.2} p95_ms={:.2} p99_ms={:.2} max_ms={:.2} fps={:.1} emitters={emitters} active={active} particles={live} submeshes={submeshes} drawn={drawn} entities={entity_count} px={}x{}{world_px}{cpu}{present}",
                    ctx.name,
                    v.len(),
                    at(0.50),
                    at(0.95),
                    at(0.99),
                    v[v.len() - 1],
                    1000.0 / mean,
                    px.0,
                    px.1,
                );
                exit.write(AppExit::Success);
            }
            Phase::Probing(n + 1)
        }
        Phase::Saving { frames, seen } => {
            let busy = !capturing.is_empty();
            let seen = seen || busy;
            // The save spans a few frames; `Capturing` marks it in-flight. Done once it has appeared
            // and cleared — or on timeout, so a missed marker can't hang the harness.
            if (seen && !busy) || frames + 1 >= SAVE_TIMEOUT_FRAMES {
                Phase::Done(0)
            } else {
                Phase::Saving {
                    frames: frames + 1,
                    seen,
                }
            }
        }
        Phase::Done(n) => {
            if n + 1 >= EXIT_GRACE_FRAMES {
                // Exit on what is ON DISK, not on having reached this phase. The save is async and
                // the `Saving` phase gives up after `SAVE_TIMEOUT_FRAMES` so a missed `Capturing`
                // marker can't hang the harness — but "gave up" used to exit Success anyway, so a
                // capture that wrote nothing reported success and the sweep carried on around the
                // hole (2026-07-28: one `water-night` run left no PNG, and `selfcheck` passed on
                // the remaining eight). A missing file is a FAILED capture and says so, so
                // `scripts/visual.sh`'s `set -e` stops the sweep at it (decision 0743).
                if Path::new(&ctx.out).is_file() {
                    info!("capture: saved {}, exiting", ctx.out);
                    exit.write(AppExit::Success);
                } else {
                    error!(
                        "capture: FAILED — no file at {} after the save window; exiting nonzero",
                        ctx.out
                    );
                    exit.write(AppExit::error());
                }
                Phase::Done(n)
            } else {
                Phase::Done(n + 1)
            }
        }
    };
    // Release (or re-hold) the frozen clock with the phase: held while the scene is still being
    // built (see [`hold_clock`]), running from the moment it is quiescent. Re-held if a late tile
    // drops the scene back out of residency, so the restarted settle is the same settle.
    if ctx.frozen_clock {
        // The invariant, in one line: **the clock runs only once the image has stopped changing.**
        // Held while the scene is still being built, so no effect can ever age from the frame its
        // model happened to arrive on.
        let held = match ctx.phase {
            Phase::Building(_) => true,
            // Shutter open — hold the sims still. The screenshot is *requested* on one frame but
            // the render world may serve it a frame or two later (pipelined rendering), and with
            // the clock still running that is a one-step difference in every particle pool: the
            // static scene is identical and the flames are not. Frozen here, it no longer matters
            // which frame is grabbed.
            Phase::Saving { .. } | Phase::Done(_) => true,
            _ => false,
        };
        match (held, clock.is_paused()) {
            (false, true) => clock.unpause(),
            (true, false) => clock.pause(),
            _ => {}
        }
    }
}
