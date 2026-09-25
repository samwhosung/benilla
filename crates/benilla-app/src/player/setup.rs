//! Startup: spawns the world camera and seeds the avatar resources [`super::control`] drives.

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

/// The stock run speed, `MOVE_RUN` 7.0 yd/s, until the server's speeds stream in.
const DEFAULT_MOVE_SPEED: f32 = 7.0;

/// The world camera's `Camera`, output mode `Skip` ([`benilla_world::final_pass`]): no upscaling
/// blit, as the player-UI camera's first draw, the FFXGlow combine, reads this view's main texture.
/// The target only carries the size ([`crate::world_backdrop`]).
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
        // The player's level: `Camera` requires `Msaa`, which would default to `Sample4`.
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

/// Inserts the move speed and avatar state and spawns the world camera above the spawn, or a plain
/// free-fly camera without client data.
pub(super) fn setup_player(
    mut commands: Commands,
    config: Option<Res<RenderConfig>>,
    world_assets: Option<Res<WorldAssets>>,
    // `gxMultisample` with `config.toml` folded in: this runs after [`crate::cvars::CvarLoad`].
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
    // Avian's capsule length is the segment between the hemisphere centres.
    commands.insert_resource(PlayerCapsule(Collider::capsule(
        CAPSULE_RADIUS,
        CAPSULE_HEIGHT - 2.0 * CAPSULE_RADIUS,
    )));
    commands.insert_resource(CameraControl {
        distance: CAM_DIST_DEFAULT,
        target_distance: CAM_DIST_DEFAULT,
        collision_distance: CAM_DIST_DEFAULT,
        // Opaque until `control` first computes the fade.
        self_fade_alpha: 1.0,
        ..default()
    });

    // No client data: free-fly an empty scene.
    let (Some(_), Some(_)) = (config, world_assets) else {
        spawn_fallback_camera(&mut commands, msaa.level());
        return;
    };

    // Above the spawn until `control` seats it. The far plane is the horizon (`view::CAM_FAR`); the
    // detailed world ends at `farclip`, by the wall.
    let spawn = wow_to_bevy([SPAWN_XY.0, SPAWN_XY.1, 100.0]);
    let cam_far = CAM_FAR;
    let mut world_cam = commands.spawn((
        Camera3d::default(),
        // The portrait booths are `Camera3d`s too: viewer queries filter on this marker.
        WorldCamera,
        // `gxMultisample` is read once: the reference latches it, pending until the next launch,
        // and a live swap would mismatch our post passes. `$WOW_MSAA` overrides it for a session.
        msaa.level(),
        Projection::from(PerspectiveProjection {
            far: cam_far,
            // Frame zero only: `view::stamp_near_clip` re-stamps the live `nearclip` every frame,
            // as `0x511bc0` overwrites the reference camera's `[cam+0x38]`.
            near: NEARCLIP_DEFAULT,
            fov: CAM_FOVY,
            ..default()
        }),
        // A linear `Rgba16Float` target without tonemapping: the shaders light in clamped gamma
        // space, so the output matches an 8-bit target.
        Hdr,
        Tonemapping::None,
        // FFXGlow, `scene + glow·blur²`: the blur here, the combine in the UI camera's ground pass.
        benilla_world::ffx_glow::FfxGlow::WORLD,
        world_camera_output(),
        Transform::from_translation(spawn + Vec3::new(0.0, 60.0, 60.0)).looking_at(spawn, Vec3::Y),
        // No Bevy ambient or fog: the scene light is computed in-shader from Light.dbc.
        FlyCam {
            yaw: 0.0,
            pitch: -0.5,
            speed: 100.0,
        },
    ));
    // `WOW_NO_INDIRECT=1` opts out of indirect draws and GPU culling for an A/B; it rides the
    // spawn, as the phase cache latches the preprocessing mode on first sight of the view.
    if std::env::var_os("WOW_NO_INDIRECT").is_some() {
        world_cam.insert(bevy::render::view::NoIndirectDrawing);
    }
    // No clustered light assignment here: world shaders take point lights from our own buffer
    // (`lighting::global_light`). `Clusters` stays so the view bind group builds; other cameras
    // keep bevy's default, and `WOW_CLUSTERS=1` restores it on this one.
    if std::env::var_os("WOW_CLUSTERS").is_none() {
        world_cam.insert(bevy::light::cluster::ClusterConfig::None);
    }
}

/// Runs the world camera in world or under the opaque loading screen, never behind the glue
/// screens; the covered render compiles the world's pipelines before the first visible frame. It
/// is wider than `world_is_live`, as the world streams in under the cover, but waits until the
/// cover is presented, so the cover's first frame is not spent on a cold world render.
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
