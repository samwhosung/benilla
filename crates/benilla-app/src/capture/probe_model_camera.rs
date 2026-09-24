//! `WOW_PROBE_MODEL_CAMERA=1` — the live probe for the `<Model>` widget's **perspective leg**
//! (decision 2027): a plain `CreateFrame("Model")` pointed at a camera-bearing file, framed by the
//! file's own camera through a camera of its own into its cell of the tile atlas.
//!
//! ## What it asks, and why it is a probe rather than a test
//!
//! The unit tests in `crate::ui_models` pin the leg's arithmetic — the client's diagonal-FOV matrix
//! (`0x5c3cc0`) against its worked numbers, and the three cancellations. What they cannot reach is
//! the half that only exists at run time: does the engine resolve the camera at all, does the
//! renderer find the record, does a camera get spawned, aimed, and pointed at a real cell of a
//! real atlas. So this probe drives the **whole live pipeline** from Lua and reads the result off
//! the entities the renderer actually built.
//!
//! The measurement is numeric, not visual (docs/METHOD.md's rule: a capture can confirm an existence
//! fact, it cannot measure one). Each leg projects three model-space probe points through the
//! camera and the root the renderer placed this frame, and compares the resulting NDC against the
//! baseline's:
//!
//! - `scale` — `SetModelScale(3)` must not move a pixel. The authored eye is carried through the
//!   model's root transform, so the camera scales with the model.
//! - `position` — `SetPosition(0.4, −0.3, 0.9)` must not move a pixel, for the same reason.
//! - `facing` — `SetFacing(1.0)` must not move a pixel **either**: the up vector `0x7ac640` builds
//!   is model-space `+Z` at `roll = 0`, which is the axis the facing turns about, so eye, target,
//!   geometry and up all turn together. The rule that only `SetFacing` shows is scoped to
//!   `<PlayerModel>`'s frozen camera (`0x7acf10`).
//! - `ortho` — the control that must NOT hold: `SetCamera(9)` is past the file's camera count, so
//!   the widget installs the NULL camera and falls to the orthographic leg, where a facing is a
//!   plain roll in the screen plane. A probe whose "identical" legs all passed because nothing was
//!   drawing would fail here.
//!
//! Three screenshots ride along — the baseline, the scaled/offset one and the turned one — as the
//! existence evidence and for the director's own eye. They are the whole window, so they are not
//! compared: the world behind the pane moves.
//!
//! ## The run recipe
//! ```text
//! WOW_USER=probe5 WOW_PASS=pprobe5 WOW_CHAR=Probefive WOW_UNATTENDED=1 WOW_NOSOUND=1 \
//!     WOW_PROBE_MODEL_CAMERA=1 cargo run -q -p benilla
//! ```
//! (the checkout's probe identity (`.probe-identity`, or WOW_USER/WOW_PASS/WOW_CHAR — the `probe` skill)). `WOW_TILE_TRACE=1`
//! alongside it prints the leg, the record, the eye and the matrix per pane per frame.

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::net::SelfPlayer;
use crate::ui_models::{TilePerspectiveCamera, TileRoot, UiModelTiles};

/// The file the probe frames. `Creature\Wolf\Wolf.m2` carries **two** cameras
/// (`benilla-extract m2cam`: index 0 type 0 at eye `(1.6927, 0.7238, 0.8837)` fov `0.95002`,
/// index 1 type 1 at eye `(5.5556, 0, 1.8056)` fov `0.67620`), so it exercises the raw-index
/// selection AND has an index past the count for the orthographic control.
const FILE: &str = r"Creature\\Wolf\\Wolf.mdx";
/// The pane's size in FrameXML units — the pet pane's own `318×224`, whose aspect is a worked
/// example of the projection `0x5c3cc0` (`θ = 0.287938 · fov`).
const PANE_W: f32 = 318.0;
const PANE_H: f32 = 224.0;
/// Frames to let a change settle before the numbers are read: the extract republishes the request
/// on the next conversion and the renderer aims the camera the frame after that.
const SETTLE_FRAMES: u32 = 8;
/// NDC agreement the cancellation legs must hold to. Generous against f32 round-off through two
/// 4×4 composes; a leg that failed to cancel misses by tens of percent, not by a thousandth.
const NDC_EPS: f32 = 1.0e-3;

/// The Lua that builds the pane. Anonymous is the corpus shape (pfUI's autocast shine), but the
/// probe needs a handle on it, so it is global.
const BUILD: &str = r#"
ProbeModelCam = CreateFrame("Model", "ProbeModelCam", UIParent)
ProbeModelCam:SetWidth(318)
ProbeModelCam:SetHeight(224)
ProbeModelCam:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
ProbeModelCam:SetModel("Creature\\Wolf\\Wolf.mdx")
ProbeModelCam:SetCamera(0)
ProbeModelCam:SetLight(1, 0, 0, -0.707, -0.707, 0.7, 1.0, 1.0, 1.0, 0.8, 1.0, 1.0, 0.8)
ProbeModelCam:Show()
"#;

pub(crate) struct ProbeModelCameraPlugin;

impl Plugin for ProbeModelCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ModelCameraProbe>()
            .add_systems(Update, model_camera_probe);
    }
}

/// One leg's reading: the NDC of the three probe points, and the label to report it under.
#[derive(Clone, Copy, Default)]
struct Reading([Vec2; 3]);

#[derive(Resource, Default)]
struct ModelCameraProbe {
    phase: Phase,
    baseline: Option<Reading>,
    passes: u32,
    fails: u32,
    skips: u32,
    exited: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Start,
    /// Waiting for the pane's tile to be packed and its camera aimed.
    Await {
        since: f64,
    },
    /// A change is in flight; read it in `at`.
    Settle {
        frames: u32,
        leg: Leg,
    },
    Done,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Leg {
    Baseline,
    Scale,
    Position,
    Facing,
    Ortho,
}

impl Leg {
    /// The Lua that puts the pane INTO this leg — run when the leg is entered, before its reading.
    fn setup(self) -> &'static str {
        match self {
            Leg::Baseline => "",
            Leg::Scale => "ProbeModelCam:SetModelScale(3)",
            Leg::Position => {
                "ProbeModelCam:SetModelScale(1) ProbeModelCam:SetPosition(0.4, -0.3, 0.9)"
            }
            Leg::Facing => "ProbeModelCam:SetPosition(0, 0, 0) ProbeModelCam:SetFacing(1.0)",
            // The control: index 9 is past Wolf's two cameras, so the widget installs the NULL
            // camera and the pane drops to the orthographic leg. Its perspective camera goes
            // inactive, which is what the reading detects.
            Leg::Ortho => "ProbeModelCam:SetCamera(9)",
        }
    }

    fn next(self) -> Option<Leg> {
        match self {
            Leg::Baseline => Some(Leg::Scale),
            Leg::Scale => Some(Leg::Position),
            Leg::Position => Some(Leg::Facing),
            Leg::Facing => Some(Leg::Ortho),
            Leg::Ortho => None,
        }
    }
}

/// The three model-space points the legs are compared on — a wolf-sized spread, in Bevy space
/// (the frame the tile root and the camera both work in).
fn probe_points() -> [Vec3; 3] {
    [
        benilla_assets::coords::wow_to_bevy([0.0, 0.0, 0.8]),
        benilla_assets::coords::wow_to_bevy([0.6, -0.3, 0.4]),
        benilla_assets::coords::wow_to_bevy([-0.4, 0.5, 1.1]),
    ]
}

/// Project the three points through whatever the renderer built this frame — the ACTIVE
/// perspective camera and the tile root under its layer. `None` when no perspective camera is
/// aimed (which is the orthographic leg, and the control's expected answer).
fn read_leg(
    cams: &Query<
        (&Camera, &GlobalTransform, &Projection, &RenderLayers),
        With<TilePerspectiveCamera>,
    >,
    roots: &Query<(&GlobalTransform, &RenderLayers), With<TileRoot>>,
) -> Option<Reading> {
    let (_, cam_tf, proj, layers) = cams.iter().find(|(c, _, _, _)| c.is_active)?;
    // The pane's root is the one on the camera's OWN layer — a perspective camera sees exactly one
    // tile by construction, and the interface has other tiles up (the minimap ping, the map arrow)
    // whose roots would otherwise be picked at random.
    let (root, _) = roots.iter().find(|(_, l)| *l == layers)?;
    let clip = proj.get_clip_from_view() * cam_tf.to_matrix().inverse() * root.to_matrix();
    let mut out = [Vec2::ZERO; 3];
    for (o, p) in out.iter_mut().zip(probe_points()) {
        let c = clip * p.extend(1.0);
        *o = Vec2::new(c.x / c.w, c.y / c.w);
    }
    Some(Reading(out))
}

fn shoot(commands: &mut Commands, name: &str) {
    // Every file we write goes through `local_state` — never into the WoW install, never a
    // platform config dir (docs/METHOD.md's one-folder rule).
    let Some(path) = crate::local_state::home().map(|d| d.join(format!("{name}.png"))) else {
        warn!("PROBE_MODEL_CAMERA: no local-state folder — skipping the {name} shot");
        return;
    };
    info!("PROBE_MODEL_CAMERA: shot -> {}", path.display());
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}

fn model_camera_probe(
    mut commands: Commands,
    time: ProbeClock,
    mut probe: ResMut<ModelCameraProbe>,
    script: Option<NonSendMut<UiScript>>,
    tiles: Res<UiModelTiles>,
    self_player: Query<(), With<SelfPlayer>>,
    cams: Query<
        (&Camera, &GlobalTransform, &Projection, &RenderLayers),
        With<TilePerspectiveCamera>,
    >,
    roots: Query<(&GlobalTransform, &RenderLayers), With<TileRoot>>,
    mut exit: MessageWriter<AppExit>,
) {
    if self_player.is_empty() {
        return; // not in-world yet
    }
    let Some(script) = script else { return };
    let now = time.elapsed_secs_f64();

    match probe.phase {
        Phase::Start => {
            info!("PROBE_MODEL_CAMERA: building a {PANE_W}x{PANE_H} <Model> on {FILE}, camera 0");
            if let Err(e) = script.run(BUILD) {
                error!("PROBE_MODEL_CAMERA: FAIL building the pane: {e}");
                probe.fails += 1;
                probe.phase = Phase::Done;
                return;
            }
            probe.phase = Phase::Await { since: now };
        }
        Phase::Await { since } => {
            if !tiles.cells.is_empty() && read_leg(&cams, &roots).is_some() {
                let cell = tiles.cells.values().next().copied();
                info!("PROBE_MODEL_CAMERA: tile up, cell {cell:?} — the perspective leg is live");
                probe.passes += 1;
                probe.phase = Phase::Settle {
                    frames: 0,
                    leg: Leg::Baseline,
                };
            } else if now - since > 30.0 {
                // The asset never landed, or the pane never reached the paint list. An
                // environmental miss, not a wrong answer.
                error!(
                    "PROBE_MODEL_CAMERA: SKIP — no tile after 30 s (cells={}, camera aimed={})",
                    tiles.cells.len(),
                    read_leg(&cams, &roots).is_some()
                );
                probe.skips += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Settle { frames, leg } => {
            if frames < SETTLE_FRAMES {
                probe.phase = Phase::Settle {
                    frames: frames + 1,
                    leg,
                };
                return;
            }
            let reading = read_leg(&cams, &roots);
            match (leg, reading) {
                (Leg::Baseline, Some(r)) => {
                    info!("PROBE_MODEL_CAMERA: baseline ndc {:?}", r.0);
                    probe.baseline = Some(r);
                    shoot(&mut commands, "probe-model-camera-a-baseline");
                }
                (Leg::Ortho, None) => {
                    info!(
                        "PROBE_MODEL_CAMERA: ortho PASS — SetCamera(9) is past the file's two \
                         cameras, so the NULL camera installed and the perspective camera parked"
                    );
                    probe.passes += 1;
                }
                (Leg::Ortho, Some(_)) => {
                    error!(
                        "PROBE_MODEL_CAMERA: ortho FAIL — an out-of-range SetCamera left the \
                         perspective leg live; the bounds check is not being applied"
                    );
                    probe.fails += 1;
                }
                (_, None) => {
                    error!("PROBE_MODEL_CAMERA: {leg:?} FAIL — the perspective camera went away");
                    probe.fails += 1;
                }
                (_, Some(r)) => {
                    let base = probe.baseline.unwrap_or_default();
                    let worst = base
                        .0
                        .iter()
                        .zip(r.0)
                        .map(|(a, b)| (a.x - b.x).abs().max((a.y - b.y).abs()))
                        .fold(0.0f32, f32::max);
                    if worst <= NDC_EPS {
                        info!(
                            "PROBE_MODEL_CAMERA: {leg:?} PASS — cancels, worst NDC delta {worst:.6}"
                        );
                        probe.passes += 1;
                    } else {
                        error!(
                            "PROBE_MODEL_CAMERA: {leg:?} FAIL — worst NDC delta {worst:.6} > \
                             {NDC_EPS}; base {:?} got {:?}",
                            base.0, r.0
                        );
                        probe.fails += 1;
                    }
                    if leg == Leg::Scale {
                        shoot(&mut commands, "probe-model-camera-b-scale-and-position");
                    }
                    if leg == Leg::Facing {
                        shoot(&mut commands, "probe-model-camera-c-facing");
                    }
                }
            }
            match leg.next() {
                Some(next) => {
                    if let Err(e) = script.run(next.setup()) {
                        error!("PROBE_MODEL_CAMERA: FAIL setting up {next:?}: {e}");
                        probe.fails += 1;
                        probe.phase = Phase::Done;
                        return;
                    }
                    probe.phase = Phase::Settle {
                        frames: 0,
                        leg: next,
                    };
                }
                None => probe.phase = Phase::Done,
            }
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            info!(
                "PROBE_MODEL_CAMERA: DONE pass={} fail={} skip={}",
                probe.passes, probe.fails, probe.skips
            );
            exit.write(AppExit::Success);
            // The polite `AppExit` plus a hard backstop, the harness's standard pair
            // (`ProbeExitPlugin::fire_probe_exit`): a net or winit teardown hang must not leave a
            // zombie client holding the probe account.
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_MODEL_CAMERA: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
