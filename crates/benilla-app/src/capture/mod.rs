//! Deterministic capture mode: with `$WOW_CAPTURE=<scenario>` the app boots server-less, pins the
//! game clock and the camera to a named viewpoint, waits for the rendered image to stop changing,
//! writes one PNG of the primary window to `$WOW_CAPTURE_OUT` and exits. `scripts/visual.sh`
//! drives it and `benilla-visual` diffs the shots against baselines.
//!
//! Captures are rendered game art: they live under the gitignored `target/visual/`.
//!
//! Two mechanisms make a capture reproducible:
//! 1. The shutter waits for the image itself to stop changing ([`STABLE_FRAMES`], [`FrameWatch`]),
//!    compared byte for byte off the framebuffer, so streaming, pipeline warm-up and late loads
//!    are all waited out.
//! 2. The game clock is frozen ([`CAPTURE_FRAME_DT`], [`hold_clock`]) while the scene builds, then
//!    released for exactly [`age_frames`] fixed steps, so the shot's sim age is the same on any
//!    machine.
//!
//! Neither fixes the residual flake: a few scenarios land in one of two stable states, because
//! draw order among coplanar equal-depth batches follows spawn order, which follows thread-pool
//! load completion. It is a renderer defect the harness reports; waiting longer cannot gate it
//! away. `creature-indoor-front` is one: two runs of one build differ at MAE 4.518, confined to
//! the subject's brightness, so a 4.518 there is the flake, not the change under test.
//!
//! ## Running one capture by hand
//! Run through Cargo, never a bare `target/debug/benilla`, which may be stale:
//! ```text
//! WOW_CAPTURE=ui-unitframes \
//!     WOW_CAPTURE_OUT=/tmp/shot.png cargo run -q -p benilla
//! ```
//! `WOW_DATA` is needed only for an install the client does not find on its own
//! (`benilla_formats::wow_data`). A `ui-*` scenario opts the player UI in by declaring a `ui:`
//! fixture ([`scenarios::ui_opted_in`]); `WOW_CAPTURE_UI=1` paints the UI over a world scenario,
//! whose baselines are UI-free by default. `WOW_CAPTURE=list` prints the scenario names.

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
// The scripted aim, swim-pitch and camera-rig drivers order themselves before `player::control`:
// an instrument may name the gameplay system it drives, gameplay never names the instrument.
mod probe_cam;
mod probe_look;
mod probe_pitch;
pub(crate) mod waterfx;

pub(crate) use probe_cam::ProbeCamPlugin;
pub(crate) use probe_look::ProbeLookPlugin;
pub(crate) use probe_pitch::ProbePitchPlugin;

/// The screen a capture starts on, the dev arm of [`crate::run_mode::start_state`]: a glue capture
/// boots onto its screen, any other capture straight in-world, no capture the login screen.
///
/// One of three readers of `$WOW_CAPTURE`, with `run_mode` and `dev_state::deterministic_run`,
/// one per layer and sharing no symbol: keep them in step.
pub(crate) fn start_state() -> crate::char_select::ClientState {
    match glue_screen() {
        Some(GlueScreen::CharCreate) => crate::char_select::ClientState::CharCreate,
        Some(GlueScreen::Login) => crate::char_select::ClientState::Login,
        None if crate::run_mode::scenario_active() => crate::char_select::ClientState::InWorld,
        None => crate::char_select::ClientState::Login,
    }
}

/// The glue screen `$WOW_CAPTURE` names, if any; read before the plugins build, since it decides
/// the start screen.
fn glue_screen() -> Option<GlueScreen> {
    let name = std::env::var("WOW_CAPTURE").ok()?;
    GLUE_SCENARIOS
        .iter()
        .find(|g| g.name == name)
        .map(|g| g.screen)
}

/// The `fxview` effect viewer: spawn one effect or missile model through the game's own
/// `attach_effect_visuals`, let it run `WOW_FX_AGE` seconds and shoot it from a chosen angle. Not a
/// golden scenario: its output depends on the knobs.
///
/// ```text
/// WOW_CAPTURE=fxview WOW_FX_MODEL='Spells\DemonArmor_Impact_Head.mdx' \
///   WOW_FX_AGE=1.2 WOW_FX_AZ=60 WOW_FX_EL=15 WOW_CAPTURE_OUT=/tmp/fx.png cargo run -q -p benilla
/// ```
///
/// Three lanes: the effect pool (`WOW_FX_MODEL`), a unit (`WOW_FX_DISPLAY`, the component set a
/// streamed creature gets) and a GameObject (`WOW_FX_GO`, whose sequence `crate::go_anim`'s state
/// machine picks, `0x5f3cb0`). A missing `GAMEOBJECT_STATE` reads as 0, ACTIVE, which on a model
/// with no `Opened` sequence resolves elsewhere.
///
/// Knobs, defaults in parentheses: `WOW_FX_AGE` s after attach (1.0), `WOW_FX_AZ`/`WOW_FX_EL`
/// orbit degrees (0/10), `WOW_FX_DIST` yd (5), `WOW_FX_FLY` yd/s along the facing (0),
/// `WOW_FX_YAW` degrees (0), `WOW_FX_TURN` deg/s after spawn (0), `WOW_FX_GROUND=1` seats it on
/// the terrain for ground decals, `WOW_FX_HOLD=1` keeps it past one sequence pass,
/// `WOW_FX_MINUTE` the clock (720).
#[derive(Resource)]
pub(crate) struct FxViewRequest {
    pub(crate) model_path: String,
    /// `WOW_FX_DISPLAY`: spawn the subject as a unit with this `CreatureDisplayInfo` id, the A/B
    /// that says whether a defect is the model's or the unit path's; `WOW_FX_MODEL` is optional.
    pub(crate) display: Option<u32>,
    /// `WOW_FX_GO`: spawn the subject as a GameObject with this `GameObjectDisplayInfo` id, the
    /// lane a placed trap renders through.
    pub(crate) go: Option<u32>,
    /// `WOW_FX_GO_STATE`: the descriptor's `GAMEOBJECT_STATE`, default 1 READY, the state vmangos
    /// creates a spell-cast trap in (`SpellEffects.cpp:5655`); 0 shows what an omitted field reads.
    pub(crate) go_state: u32,
    /// `WOW_FX_GO_TYPE`: the `GAMEOBJECT_TYPE_ID`, default 6 TRAP; decides whether
    /// [`crate::go_anim::go_animates`] runs the state machine at all.
    pub(crate) go_type: u32,
    /// `WOW_FX_SCALE`: the unit lane's wire scale.
    pub(crate) scale: f32,
    pub(crate) age: f32,
    pub(crate) az_deg: f32,
    pub(crate) el_deg: f32,
    pub(crate) dist: f32,
    pub(crate) fly: f32,
    pub(crate) yaw_deg: f32,
    pub(crate) turn: f32,
    pub(crate) ground: bool,
    pub(crate) hold: bool,
    /// `WOW_FX_AT=x,y,z`: a raw WoW point in place of [`FXVIEW_POS`], `WOW_MAP` the map. Ground
    /// decals depend on the surface: footprints need `TerrainType.Flags & 1` (snow, sand).
    pub(crate) at: Option<[f32; 3]>,
    /// `WOW_FX_UP`: yards above the resolved seat, for a model authored below its anchor (Arcane
    /// Intellect's star cluster starts 1.6 yd under it) whose opening the terrain would swallow.
    pub(crate) up: f32,
}

/// The fixture's live state, written by `fxview::drive_fx_view` and `drive_capture`.
#[derive(Resource, Default)]
pub(crate) struct FxViewState {
    /// Set once the scene has settled; the fixture spawns only then, so its age is the one asked.
    pub(crate) armed: bool,
    pub(crate) root: Option<Entity>,
    /// `time.elapsed_secs()` when the visuals attached: the age clock's zero.
    pub(crate) attached_at: Option<f32>,
    /// The root was reaped after one sequence pass, as the game reaps a discrete kit;
    /// `WOW_FX_HOLD=1` disables the reap.
    pub(crate) expired: bool,
}

/// Where the fxview effect spawns: mid-air over the Northshire slope, inside the ground
/// scenario's streamed tiles.
pub(crate) const FXVIEW_POS: [f32; 3] = [-8960.0, -145.0, 90.0];

/// How far the `vista` fixture seats its eye above the given position (yd): a standing human's
/// camera pivot, so a `.go xyz` off the debug panel frames what a player there saw.
const VISTA_EYE_HEIGHT: f32 = 2.0;

/// Print the baseline scenario names, one per line, for `WOW_CAPTURE=list`; `scripts/visual.sh`
/// reads its sweep from this. On-demand fixtures are not listed.
pub(crate) fn print_scenario_names() {
    for s in SCENARIOS {
        println!("{}", s.name);
    }
}

/// Consecutive byte-identical framebuffer readbacks that mean the scene is built: anything still
/// arriving changes the image or does not affect the shot. `$WOW_CAPTURE_STABLE` overrides it.
const STABLE_FRAMES: u32 = 30;

fn stable_frames() -> u32 {
    std::env::var("WOW_CAPTURE_STABLE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(STABLE_FRAMES)
}

/// Cap on held frames waiting for [`STABLE_FRAMES`], so a scene that never settles (a live UI
/// animation) shoots anyway, with a warning that the shot is not reproducible.
const BUILD_CAP_FRAMES: u32 = 1800;

/// Wall-clock ceiling on a capture run, past which it exits with an error. The other bounds count
/// frames, and macOS can stop granting drawables to a window it is not compositing (about 1 s per
/// `-[CAMetalLayer nextDrawable]`), which stretches the frame caps to half an hour.
const DEADLINE_SECS: u64 = 300;

/// The effective ceiling: `$WOW_CAPTURE_DEADLINE=<secs>`, `0` to disable.
fn capture_deadline() -> Option<Duration> {
    let secs = std::env::var("WOW_CAPTURE_DEADLINE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEADLINE_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Steps of [`CAPTURE_FRAME_DT`] the sims run once the scene is built: the shot's sim age. 150 is
/// 2.5 s, past the 2 s spawn fade (`model_fade::APPEAR_FADE_SECS`) and a flame pool's particle
/// lifetime. `$WOW_CAPTURE_AGE` overrides it for a scene that fills slowly (snow wants ~660).
const AGE_FRAMES: u32 = 150;

fn age_frames() -> u32 {
    std::env::var("WOW_CAPTURE_AGE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(AGE_FRAMES)
}

/// The frozen clock's step, 1/60 s per capture frame, so everything integrated in seconds reaches
/// the shutter in the same state every run. On a live clock two runs of one build differ by up to
/// MAE 0.009, as much as a real render change.
const CAPTURE_FRAME_DT: Duration = Duration::from_nanos(16_666_667);

/// Cap on frames waiting for the screenshot save, so a capture never hangs.
const SAVE_TIMEOUT_FRAMES: u32 = 120;
/// A couple of grace frames after the save completes before exiting, so the file is flushed.
const EXIT_GRACE_FRAMES: u32 = 3;

/// Frames the perf probe discards after uncapping vsync (present-mode switch + pipeline settle).
const PROBE_WARMUP_FRAMES: u32 = 60;

/// Marks a stability readback, so [`Phase::Saving`]'s wait cannot mistake it for the shot.
#[derive(Component)]
struct StabilityShot;

/// The image-stability tracker. Compares whole bytes, not a hash: a `memcmp` is free next to the
/// readback and needs no collision argument.
#[derive(Resource, Default)]
struct FrameWatch {
    prev: Option<Vec<u8>>,
    stable: u32,
    /// A readback is outstanding; only one at a time, so `stable` counts distinct frames.
    in_flight: bool,
}

/// Observer for a stability readback: compare against the previous frame, count, and free the slot.
fn watch_frame(shot: On<ScreenshotCaptured>, mut watch: ResMut<FrameWatch>) {
    watch.in_flight = false;
    let Some(bytes) = shot.image.data.as_ref() else {
        return; // no payload (a zero-sized target): no evidence
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
    /// Clock held; waiting for the image to stop changing ([`STABLE_FRAMES`]).
    Building(u32),
    /// Clock released: running exactly [`age_frames`] steps of [`CAPTURE_FRAME_DT`].
    Aging(u32),
    /// Fixture viewers only: fixture armed, waiting for it to attach and reach its age.
    FxAging,
    /// Perf probe (`$WOW_FPS_PROBE`): vsync off, discarding warm-up frames.
    ProbeWarmup(u32),
    /// Perf probe: sampling frame times, then print and exit.
    Probing(u32),
    /// Screenshot requested; waiting for the async save's `Capturing` marker to appear and clear.
    Saving { frames: u32, seen: bool },
    /// Save done; a few grace frames, then `AppExit`.
    Done(u32),
}

#[derive(Resource)]
struct CaptureCtx {
    /// The in-world viewpoint; `None` for a glue-screen capture, which has no world.
    scenario: Option<Scenario>,
    /// The scenario's name, for the output path and the probe line.
    name: &'static str,
    out: String,
    phase: Phase,
    /// The UI fixture is seeded ([`seed_ui_fixture`]).
    ui_seeded: bool,
    /// `$WOW_FPS_PROBE`: sample this many frames, vsync off, and print frame-time stats and scene
    /// counts instead of screenshotting; 0 is a normal capture.
    probe_frames: u32,
    /// The frozen clock ([`CAPTURE_FRAME_DT`]) is in force. A screenshot run keeps it to the end; a
    /// perf probe builds and ages frozen like a capture, so it measures a scene of the same age on
    /// any machine, then drops it because its measurement is the real frame cost.
    frozen_clock: bool,
    /// `$WOW_RESIZE` is applied (once, at first settle).
    resized: bool,
    probe_samples: Vec<f32>,
    /// Process CPU seconds at the first sample, the baseline for the probe line's `cpu_ms`.
    probe_cpu_start: Option<f64>,
    /// Wall-clock start of the run ([`capture_deadline`]). A real [`Instant`]: under the frozen
    /// clock `Time<Real>` is itself manual and would only count frames.
    started: Instant,
    deadline: Option<Duration>,
    /// Frames [`drive_capture`] has run, the denominator of the deadline message's rate.
    frames: u32,
    /// The deadline already fired; `AppExit` is written and no phase advances again.
    bailed: bool,
}

/// `$WOW_RESIZE=WxH`: resize the window (logical px) once the image first settles, then settle
/// again before shooting.
fn resize_request() -> Option<(u32, u32)> {
    let v = std::env::var("WOW_RESIZE").ok()?;
    let (w, h) = v.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// The present mode a perf probe uncaps to, shared with the live probe.
///
/// `AutoNoVsync` by default: on macOS/Metal explicit `Immediate` rails near 16.6 ms and takes
/// 1.0-1.5 s drawable stalls, while `AutoNoVsync` uncaps when the window server grants it; when
/// it does not, `cpu_ms` on the probe line is the metric. `WOW_PROBE_UNCAP=immediate` selects
/// `Immediate`. `WOW_PROBE_UNCAP=vsync` keeps vsync for a sitting whose legs never rail; the vsync
/// wait costs process CPU unevenly, so read it only against a same-mode baseline.
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
        // The `waterfx` dummy moves before the foam emitter reads its motion.
        app.add_systems(
            Update,
            (waterfx::spawn, waterfx::drive)
                .chain()
                .before(benilla_world::water_fx::WaterFoamSet)
                .in_set(benilla_world::schedule::WorldStage::Present),
        );
        // The `fxview` driver creates the subject's display-cache entry, so it runs before the
        // frame's display build, after the net stage.
        app.add_systems(
            Update,
            fxview::drive_fx_view
                .in_set(crate::entities::EntityVisualsSet)
                .before(crate::entities::DisplayBuildSet)
                .after(benilla_world::schedule::WorldStage::Net),
        );
        let name = std::env::var("WOW_CAPTURE").unwrap_or_default();
        // A glue capture has no map, viewpoint or fixture; only the shutter is shared.
        let glue = GLUE_SCENARIOS.iter().find(|g| g.name == name).copied();
        if let Some(g) = glue {
            // The preview pick rides `WOW_CHARCREATE_PICK`, and a pick already in the environment
            // wins, so one scenario photographs every race's stage.
            if let Some((race, sex, class)) = g.pick {
                if std::env::var_os("WOW_CHARCREATE_PICK").is_none() {
                    std::env::set_var("WOW_CHARCREATE_PICK", format!("{race},{sex},{class}"));
                }
            }
        }
        // `fxview`, `waterfx`, `vista` and `name-close` are built from knobs, not the tables, and
        // are never in the golden sweep.
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
                    (Err(_), Some(_)) => String::new(), // the id lanes name their own model
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
                // The water-foam viewer: a wading unit over a synthetic lattice, or in real
                // liquid with `WOW_WFX_AT`, on a fixed orbit. Knobs: `WOW_WFX_MODE`
                // (ring|wake|turn), `_SPEED` yd/s, `_HEAD` deg, `_AGE` s, `_DEPTH` yd, and the
                // camera's `_AZ`/`_EL`/`_DIST`.
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
                // Rig centre, raw WoW coords: a synthetic surface over the Northshire ground by
                // default; `WOW_WFX_AT` puts it in the real liquid there, `z` its surface.
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
                    // `WOW_MAP` picks the map for a live rig.
                    map: None,
                    eye,
                    look: center,
                    minute: 720,
                    ui: None,
                }
            } else if name == "vista" {
                // The arbitrary-viewpoint instrument: any position, heading and clock, e.g.
                //
                //   WOW_CAPTURE=vista WOW_VISTA_AT=-5841.9,-3802.4,-59.7 WOW_VISTA_FACE=24 \
                //     WOW_VISTA_MIN=1052 WOW_FARCLIP=320 cargo run -q -p benilla
                //
                // `WOW_VISTA_AT` is the player's feet (the eye sits `VISTA_EYE_HEIGHT` above),
                // `WOW_VISTA_FACE` the debug panel's facing in degrees (0 = +X, counter-clockwise),
                // `WOW_VISTA_PITCH` degrees up (0), `WOW_VISTA_MIN` the game minute (720).
                // `WOW_FARCLIP` matches a player's far-clip slider.
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
                    // `WOW_MAP` picks the map (a `Map.dbc` id).
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
                // The magnified overhead-name instrument ([`scenarios::NAME_CLOSE_AT`]).
                let knob = |k: &str, d: f32| {
                    std::env::var(k)
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(d)
                };
                let at = scenarios::NAME_CLOSE_AT;
                // The camera orbits and aims at the name, `WOW_NAME_H` above the unit's feet.
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
        // Seed the map before `world_map::load_world_map` reads `WOW_MAP` at `Startup`, the one
        // place `CurrentMap` is set server-less; a `map: None` instrument leaves the knob alone.
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
        // The frozen clock for every run; a probe switches back to `Automatic` before its first
        // sample, or it would measure a constant 16.67 ms ([`CaptureCtx::frozen_clock`]).
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
            // Before `UnitFeed`: the seed stands in for wire data live play delivers on earlier
            // frames, so the same frame's feeds and the one-shot `MERCHANT_SHOW` paint must see it.
            .add_systems(Update, seed_ui_fixture.before(crate::ui_unit::UnitFeed))
            .add_systems(Last, drive_capture);
    }
}

/// Each frame, force the capture conditions: pinned time of day, no perf HUD, the fixed camera.
/// Runs in `WorldStage::Present`, after terrain streaming reads the camera.
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
    perf.visible = false;
    // A glue screen has no world, clock or camera to pin.
    let Some(scenario) = ctx.scenario else {
        return;
    };
    debug.lighting.follow_server_time = false;
    debug.lighting.manual_minute = scenario.minute;

    // `WOW_MM_PROBE=x,y,z` (raw WoW coords) places an active player there, so the interior
    // minimap renders server-less, with the camera above so the WMO streams in around it.
    let probe = std::env::var("WOW_MM_PROBE").ok().and_then(|s| {
        let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    });
    if let Some(p) = probe {
        player.pos = wow_to_bevy(p);
        player.active = true;
        player.detached = false;
        // Off to the side: straight down degenerates `looking_at` with Y up.
        let eye = wow_to_bevy([p[0] - 25.0, p[1] - 25.0, p[2] + 40.0]);
        for mut t in &mut cam {
            *t = Transform::from_translation(eye).looking_at(wow_to_bevy(p), Vec3::Y);
        }
        return;
    }

    let (eye, look) = match (&fx_req, &fx_state) {
        // fxview: orbit the fixture's live root (a missile moves), aimed one yard up, where the
        // effect models author their bodies.
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
/// Tiles spawn under a wall-clock budget, so without the hold an emitter would age from whichever
/// frame its tile arrived on; held, every emitter has run exactly [`age_frames`] steps at the
/// shutter. Streaming and the save are frame-driven and run on regardless.
fn hold_clock(mut clock: ResMut<Time<Virtual>>) {
    clock.pause();
}

/// The scene-population queries the `FPS_PROBE` line prints, bundled because `drive_capture` sits
/// at Bevy's 16-parameter ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ProbeCensus<'w, 's> {
    particles: Query<'w, 's, &'static benilla_world::particles::ParticleEmitter>,
    parts: Query<'w, 's, &'static ViewVisibility, With<benilla_world::model_render::ModelPart>>,
    entities: Query<'w, 's, ()>,
}

/// Drive the capture lifecycle: settle, age, screenshot or probe, exit.
fn drive_capture(
    mut ctx: ResMut<CaptureCtx>,
    mut watch: ResMut<FrameWatch>,
    // Excludes stability readbacks, or one in flight could satisfy the real shot's wait.
    capturing: Query<(), (With<Capturing>, Without<StabilityShot>)>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    // `ResMut` to re-anchor the clock at the probe's release (`Phase::Aging`).
    mut time: ResMut<Time<bevy::time::Real>>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    census: ProbeCensus,
    fx_req: Option<Res<FxViewRequest>>,
    wfx_req: Option<Res<waterfx::WaterFxView>>,
    mut fx_state: Option<ResMut<FxViewState>>,
    // The harness's one virtual-clock read, allowlisted in
    // `probes::probe_schedules_read_the_wall_clock`: the fixture's age is measured on the clock the
    // effect animates on. Everything schedule-shaped reads [`probes::ProbeClock`].
    game_time: Res<Time>,
    mut clock: ResMut<Time<Virtual>>,
    // Switched to `Automatic` at the probe's release (`Phase::Aging`).
    mut time_strategy: ResMut<TimeUpdateStrategy>,
    // `None` on a glue-screen capture, which builds no composite lane.
    backdrop: Option<Res<crate::world_backdrop::WorldBackdrop>>,
) {
    // The wall-clock ceiling covers every phase: a fixture may never attach, a readback may never
    // land ([`capture_deadline`]).
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
    let fixture_age = fx_req
        .as_ref()
        .map(|r| r.age)
        .or(wfx_req.as_ref().map(|r| r.age));
    ctx.phase = match ctx.phase {
        Phase::Building(n) => {
            // One readback at a time, requested only here, marked so `Saving` cannot mistake it.
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
            // Stability does not prove content: a lane that renders nothing is stable too. Color
            // channels only, since alpha is opaque even on a black frame.
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
                             scene rendered nothing"
                        );
                    }
                }
            }
            if watch.stable < stable_frames() && !capped {
                Phase::Building(n + 1)
            } else if let Some((rw, rh)) = resize_request().filter(|_| !ctx.resized) {
                // `$WOW_RESIZE`: resize after the UI was built and measured at the boot size, the
                // fullscreen-toggle flow a fresh boot at the target size cannot exercise.
                ctx.resized = true;
                if let Ok(mut w) = windows.single_mut() {
                    w.resolution.set(rw as f32, rh as f32);
                }
                watch.stable = 0;
                info!("capture: resized to {rw}x{rh}, re-settling");
                Phase::Building(0)
            } else if fixture_age.is_some() {
                if let Some(state) = fx_state.as_deref_mut() {
                    state.armed = true; // the fixture spawns now, its age clock clean
                }
                Phase::FxAging
            } else {
                // Clock released: the sims run exactly `age_frames()` steps, probe or capture.
                info!(
                    "capture: image settled after {} frames, aging {}",
                    n + 1,
                    age_frames()
                );
                Phase::Aging(0)
            }
        }
        Phase::FxAging => {
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
            // No restart on late arrivals: the image already stopped changing before release.
            if n + 1 < age_frames() {
                Phase::Aging(n + 1)
            } else if ctx.probe_frames > 0 {
                // Hand the clock back to real time before the first sample, or `Probing` would
                // read the manual [`CAPTURE_FRAME_DT`] and report a flawless 16.67 ms.
                ctx.frozen_clock = false;
                *time_strategy = TimeUpdateStrategy::Automatic;
                // Re-anchor `Time<Real>` to now: the manual strategy left `last_update` behind by
                // the frozen phase's wall time (~8 s), which the first automatic tick would bill
                // as one hitch and a quarter-second sim step. Called from `Last`, so the delta it
                // writes is overwritten by the next `time_system` before anything reads it.
                time.update_with_instant(Instant::now());
                // Uncap presentation to measure frame cost; `$WOW_PROBE_VSYNC=1` keeps vsync and
                // measures the present ceiling itself.
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
            // `==`, not `>=`: `AppExit` takes a frame or two to drain, and `>=` would print twice.
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
                // The world's render size, which need not match the window `px`: a supersample
                // would otherwise be priced as a native frame.
                let world_px = backdrop.map_or(String::new(), |b| {
                    let s = b.render_size();
                    format!(" world_px={}x{}", s.x, s.y)
                });
                // Submeshes, how many survived the cull, and the entity count.
                let (submeshes, drawn) = census.parts.iter().fold((0usize, 0usize), |(n, d), v| {
                    (n + 1, d + usize::from(v.get()))
                });
                let entity_count = census.entities.iter().len();
                // CPU per frame across every thread, the load-robust metric.
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
                // The present mode measured under, so a silently railed uncap is diagnosable.
                let present = windows
                    .single()
                    .map(|w| format!(" present={:?}", w.present_mode))
                    .unwrap_or_default();
                // One greppable line on stdout, clear of log filtering.
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
                // Exit on what is on disk: a save that timed out without a file is a failed
                // capture, so `scripts/visual.sh`'s `set -e` stops the sweep at it.
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
    // The clock runs only once the image has stopped changing ([`hold_clock`]): held while
    // building and while saving, running while aging.
    if ctx.frozen_clock {
        let held = match ctx.phase {
            Phase::Building(_) => true,
            // Pipelined rendering may serve the screenshot a frame or two after the request; a
            // running clock would move every particle pool by a step in that gap.
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
