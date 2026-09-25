//! Startup: spawn the camera and seed the avatar resources the [`super::control`] system then drives.
//! Split from the controller because it's a one-shot `Startup` system (a distinct schedule phase), not
//! the per-frame loop — the plugin remains the stable face that wires both.

use avian3d::prelude::*;
use bevy::camera::{CameraOutputMode, PerspectiveProjection, Projection};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::render::view::Hdr;

use benilla_assets::coords::wow_to_bevy;

use benilla_assets::{RenderConfig, WorldAssets};
use benilla_world::terrain_stream::SPAWN_XY;
use benilla_world::view::{WorldCamera, CAM_FAR, CAM_FOVY, NEARCLIP_DEFAULT};

use super::{
    CameraControl, FlyCam, MoveSpeed, Player, PlayerCapsule, CAM_DIST_DEFAULT, CAPSULE_HEIGHT,
    CAPSULE_RADIUS,
};

/// Default avatar speed in yards/second — the **VERIFIED** vanilla run speed (`MOVE_RUN` 7.0). Ctrl
/// sprints 2.5× (≈17.5) for getting around; `$WOW_MOVE_SPEED`
/// overrides. (Was 60.0 as a fly-around convenience before collision; the faithful default now that
/// the character controller is in.)
const DEFAULT_MOVE_SPEED: f32 = 7.0;

/// The world camera's `Camera` — output mode `Skip` (
/// [`benilla_world::final_pass`]): nothing writes this camera's target, and bevy's `upscaling`
/// blit — a pure copy of the finished frame into it — is skipped. The world's final pass, the
/// FFXGlow combine, is the first draw of the player-UI camera's main pass now
/// (`benilla_world::ffx_glow::FfxBackdrop`), reading this view's finished main texture straight
/// into the interface's byte buffer; the target the camera carries is a size-carrier only
/// ([`crate::world_backdrop`]).
fn world_camera_output() -> Camera {
    Camera {
        output_mode: CameraOutputMode::Skip,
        ..default()
    }
}

fn spawn_fallback_camera(commands: &mut Commands, msaa: Msaa) {
    commands.spawn((
        Camera3d::default(),
        WorldCamera,
        // The same level the real camera takes — this one used to name nothing at all, which
        // (`Camera` requires `Msaa`, defaulting to `Sample4`) meant a data-less free-fly quietly
        // ran four samples no matter what the player had set.
        msaa,
        Hdr,
        Tonemapping::None,
        benilla_world::ffx_glow::FfxGlow::WORLD,
        world_camera_output(),
        Transform::from_xyz(0.0, 50.0, 100.0).looking_at(Vec3::ZERO, Vec3::Y),
        FlyCam {
            yaw: 0.0,
            pitch: 0.0,
            speed: 40.0,
        },
    ));
}

/// Startup: insert the move speed + avatar state, and spawn the camera. With client data present the
/// camera sits above the spawn with the scene `AmbientLight`/`DistanceFog` (driven each frame by
/// `update_time_lighting`) + a radius-derived far plane; without it, a plain free-fly fallback camera.
pub(super) fn setup_player(
    mut commands: Commands,
    config: Option<Res<RenderConfig>>,
    world_assets: Option<Res<WorldAssets>>,
    // The pending `gxMultisample`, already resolved: this system is ordered after
    // [`crate::cvars::CvarLoad`], so `config.toml` has been folded in before the camera is born.
    msaa: Res<benilla_world::view::MsaaSetting>,
) {
    let env_speed = std::env::var("WOW_MOVE_SPEED")
        .ok()
        .and_then(|s| s.parse::<f32>().ok());
    commands.insert_resource(MoveSpeed {
        value: env_speed.unwrap_or(DEFAULT_MOVE_SPEED),
        env_override: env_speed.is_some(),
    });
    commands.insert_resource(Player::default());
    // The character capsule swept by avian's `MoveAndSlide` (length = cylinder segment between the
    // hemisphere centres, so total height is `length + 2·radius`).
    commands.insert_resource(PlayerCapsule(Collider::capsule(
        CAPSULE_RADIUS,
        CAPSULE_HEIGHT - 2.0 * CAPSULE_RADIUS,
    )));
    commands.insert_resource(CameraControl {
        distance: CAM_DIST_DEFAULT,
        target_distance: CAM_DIST_DEFAULT,
        collision_distance: CAM_DIST_DEFAULT,
        // Start opaque so the avatar never flashes invisible before `control`'s first fade computation.
        self_fade_alpha: 1.0,
        ..default()
    });

    // No client data → free-fly an empty scene.
    let (Some(_), Some(_)) = (config, world_assets) else {
        spawn_fallback_camera(&mut commands, msaa.level());
        return;
    };

    // Camera starts above the spawn (terrain streams in around it); `control` repositions it
    // third-person once we're in the world. The projection far is the horizon plane
    // (`view::CAM_FAR`); the detailed world ends at `farclip`, by the wall, not by this plane.
    let spawn = wow_to_bevy([SPAWN_XY.0, SPAWN_XY.1, 100.0]);
    let cam_far = CAM_FAR;
    let mut world_cam = commands.spawn((
        Camera3d::default(),
        // THE world camera (the portrait booths are further `Camera3d`s — every "where is the viewer"
        // consumer filters on this marker, never on bare `Camera3d`; see its doc).
        WorldCamera,
        // The player's `gxMultisample`, read ONCE here and never again — the
        // reference registers this CVar *latched* and its callback echoes "set pending gxRestart",
        // so a change is pending until the next launch. Which is also the only thing we could do:
        // swapping MSAA live leaves our post passes (glow/egui) MSAA-mismatched and freezes the
        // view. `$WOW_MSAA` still overrides it session-only, through the resource's `Default`.
        msaa.level(),
        Projection::from(PerspectiveProjection {
            far: cam_far,
            // The registered default, and only for frame zero: `view::stamp_near_clip` re-stamps
            // this from the live `nearclip` every frame, exactly as `0x511bc0` overwrites whatever
            // the reference's camera ctor left in `[cam+0x38]` (2163).
            near: NEARCLIP_DEFAULT,
            fov: CAM_FOVY,
            ..default()
        }),
        // HDR render target (linear `Rgba16Float`) — the prerequisite for Bevy's `Bloom`. `Hdr` is a
        // marker component (`Bloom` requires it; added explicitly for clarity). Option 1 keeps the
        // vanilla look: `Tonemapping::None` (no filmic curve — the shaders still light in clamped gamma
        // space and output the same values), so the HDR pipeline reproduces the LDR look while letting
        // Bevy's bloom replace the hand-rolled `glow.rs`. (Option 2, on a branch, swaps in a filmic
        // tonemapper + scene-referred lighting for a modern look.)
        Hdr,
        Tonemapping::None,
        // The faithful FFXGlow pass: the byte-pinned `scene + glow·blur²`.
        // This view runs its blur; its combine is the UI camera's own ground pass (2234).
        benilla_world::ffx_glow::FfxGlow::WORLD,
        world_camera_output(),
        Transform::from_translation(spawn + Vec3::new(0.0, 60.0, 60.0)).looking_at(spawn, Vec3::Y),
        // PHASE 0: no PBR ambient fill, no distance fog — pitch-black clean slate. The faithful scene
        // light is rebuilt in-shader from Light.dbc (terrain/model WGSL), not via Bevy PBR lights.
        FlyCam {
            yaw: 0.0,
            pitch: -0.5,
            speed: 100.0,
        },
    ));
    // `WOW_NO_INDIRECT=1` — opt the world camera out of indirect draws + GPU culling. An
    // EXPERIMENT knob (the full-picture sizing): wgpu 27 dropped MULTI_DRAW_INDIRECT from
    // bevy's gate, so 0.18 is the first release where Metal reaches `Culling` mode — silently.
    // This knob restores the pre-0.18 mode for an A/B. It must ride the SPAWN: the phase
    // cache latches the preprocessing mode the first time it sees the view.
    if std::env::var_os("WOW_NO_INDIRECT").is_some() {
        world_cam.insert(bevy::render::view::NoIndirectDrawing);
    }
    // Bevy's clustered-forward light assignment is OFF on the world camera by default (the
    // 1370 bracket surfaced the lane; the 3-round SW split then measured the skip at −0.28
    // cpu_ms): every world shader takes its point-light term off OUR storage buffer
    // (`lighting::global_light` — bevy's clusterable buffer is fragment-only in the view
    // layout and nothing of ours imports `apply_pbr_lighting`; the WDL far ring is unlit), so
    // the whole assign/extract/prepare cluster lane is dead work proportional to the resident
    // `PointLight` population (794 at the SW pin). `ClusterConfig::None` short-circuits
    // `assign_objects_to_clusters` per view and starves the light extract/prepare downstream;
    // the `Clusters` component stays, so the view bind group still builds. Scoped to THIS
    // camera: the booth/pane cameras keep bevy's default.
    // `WOW_CLUSTERS=1` restores bevy's upstream default (the A/B lever back).
    if std::env::var_os("WOW_CLUSTERS").is_none() {
        world_cam.insert(bevy::light::cluster::ClusterConfig::None);
    }
}

/// The world camera's demand gate: active in world or under the opaque loading
/// screen — never behind the glue screens, where the streamed world (25 tiles, tens of thousands
/// of entities, MSAA 4×) otherwise renders unseen behind an opaque fullscreen glue scene every
/// frame. The loading-screen case is load-bearing: that covered render is what compiles the
/// world's pipelines, so the first visible in-world frame doesn't hitch on shader builds.
/// Capture runs boot straight `InWorld` (`CharSelectPlugin::start`) — always active there.
///
/// This gate stays deliberately WIDER than [`benilla_world::schedule::world_is_live`], which decides
/// whether the world is *loaded* at all: the camera must also render while the
/// cover is up, which is exactly the window in which the world is streaming in.
///
/// **…but not on the frame the cover owes the glass.** 0962's rule, third consumer: on world
/// entry the state flips a frame after the raise, so the flip frame is the first whose render can
/// draw the loading screen — and this gate turned the world camera on for that exact frame, which
/// then rendered a three-thousand-entity world with every pipeline cold *before* it could present
/// the cover. Measured: 57 of that frame's 60 ms, and what stayed on the glass throughout was the
/// character-select screen, frozen (the director's report). Held off until
/// [`EntryCover::presented`], the camera loses three frames of warm-up out of the seconds it gets
/// and the cover reaches the glass on the frame it was drawn for. Nothing is *shown* by the
/// deferral — the loading root is an unconditional fullscreen black node, so a frame with no
/// world camera is the same picture the cover would have drawn over anyway.
pub(super) fn gate_world_camera(
    state: Res<State<crate::char_select::ClientState>>,
    loading: Res<crate::loading_screen::LoadingScreen>,
    cover: Res<crate::loading_screen::EntryCover>,
    mut cams: Query<&mut Camera, With<WorldCamera>>,
) {
    let active = (*state.get() == crate::char_select::ClientState::InWorld || loading.covering())
        && !cover.owes_a_present();
    for mut cam in &mut cams {
        if cam.is_active != active {
            cam.is_active = active;
        }
    }
}
