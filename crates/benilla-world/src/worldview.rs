//! `benilla-worldview`: the engine with no game attached, and the wall that keeps it that way. It
//! boots the engine plugins with a free-fly camera over Elwynn and no server, login, UI or player,
//! so a game concept wired back into the engine breaks it. A crate boundary alone cannot hold that
//! line: two Bevy systems couple through a runtime resource with no symbol crossing between them.

use bevy::camera::{PerspectiveProjection, Projection};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::render::view::Hdr;
use bevy::window::{CursorGrabMode, PrimaryWindow};

use benilla_assets::coords::wow_to_bevy;

use crate::boot;
use crate::build_id::BuildId;
use crate::terrain_stream::SPAWN_XY;
use crate::thread_qos;

/// Where the viewer opens, in WoW world coords: Northshire, the client's own boot anchor
/// ([`crate::terrain_stream::SPAWN_XY`]), so both binaries stream the same tiles.
const VIEW_START: (f32, f32) = SPAWN_XY;

/// Height above the spawn point the camera opens at, in yards.
const VIEW_START_HEIGHT: f32 = 60.0;

/// `WOW_WORLDVIEW_AT=<x>,<y>[,<z>]` opens the viewer elsewhere (raw WoW coords, `z` defaulting to
/// [`VIEW_OPEN_Z`]); unparseable or absent stays at [`VIEW_START`], which must remain the default.
fn view_start() -> [f32; 3] {
    let Some(v) = std::env::var("WOW_WORLDVIEW_AT").ok() else {
        return [VIEW_START.0, VIEW_START.1, VIEW_OPEN_Z];
    };
    let c: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    match c.len() {
        2 => [c[0], c[1], VIEW_OPEN_Z],
        3 => [c[0], c[1], c[2]],
        _ => {
            warn!("worldview: WOW_WORLDVIEW_AT wants `x,y[,z]` — staying at the default anchor");
            [VIEW_START.0, VIEW_START.1, VIEW_OPEN_Z]
        }
    }
}

/// The look-at height when `WOW_WORLDVIEW_AT` names no `z`, clear of the Northshire valley floor.
const VIEW_OPEN_Z: f32 = 100.0;

/// Near plane in yards: the `nearclip` CVar's default, since the viewer has no CVar table.
const NEAR: f32 = crate::view::NEARCLIP_DEFAULT;

/// Vertical FOV in radians (70°), wider than the client camera's [`crate::view::CAM_FOVY`].
const FOVY: f32 = 1.221_730_5;

/// Builds and runs the world viewer; `build` is the launcher shim's compile-time git stamp.
pub fn run(build: BuildId) -> AppExit {
    let mut app = App::new();
    app.insert_resource(build);

    // `WOW_WORLDVIEW_SURVEY=1` turns Bevy's panic on unvalidated system parameters into a
    // warning, so one run names every missing resource.
    if std::env::var("WOW_WORLDVIEW_SURVEY").as_deref() == Ok("1") {
        warn!("worldview: SURVEY mode — unmet dependencies are warnings, not panics");
        app.set_error_handler(bevy::ecs::error::warn);
    }

    // `WOW_WORLDVIEW_CHECK[=seconds]`, the survey as a gate: it records every distinct fault, runs
    // for a bounded time, prints the set and exits non-zero if any; `scripts/gates.sh` runs it.
    let check = check_seconds();
    if let Some(secs) = check {
        app.set_error_handler(record_fault);
        app.insert_resource(CheckDeadline(secs))
            .add_systems(Update, end_check);
    }

    // The `mpq://` source must be registered before `AssetPlugin` (in `DefaultPlugins`) builds.
    match benilla_formats::wow_data() {
        Some(data_dir) => {
            if let Err(e) = benilla_assets::register_mpq_source(&mut app, &data_dir) {
                eprintln!("benilla-assets: mpq:// source unavailable ({e:#})");
            }
        }
        None => eprintln!(
            "benilla-worldview: no WoW install found — looked in {:?}",
            benilla_formats::candidates()
        ),
    }

    let background = crate::bgwin::background_run();
    app.add_plugins(boot::tuned_default_plugins(Window {
        title: "benilla worldview".into(),
        resolution: std::env::var("WOW_WIN")
            .ok()
            .and_then(|v| {
                let (w, h) = v.split_once('x')?;
                Some(UVec2::new(w.parse().ok()?, h.parse().ok()?))
            })
            // Small for a run that reads no pixels, as in the client.
            .unwrap_or(if crate::bgwin::no_pixel_run() {
                UVec2::new(640, 360)
            } else {
                UVec2::new(1600, 900)
            })
            .into(),
        present_mode: if std::env::var("WOW_NOVSYNC").as_deref() == Ok("1") {
            bevy::window::PresentMode::AutoNoVsync
        } else {
            bevy::window::PresentMode::default()
        },
        // As in the client: an instrumented run opens unfocused, below normal windows.
        focused: !background,
        window_level: if background {
            bevy::window::WindowLevel::AlwaysOnBottom
        } else {
            bevy::window::WindowLevel::Normal
        },
        ..default()
    }))
    .add_plugins(thread_qos::ThreadQosPlugin)
    .add_plugins(crate::bgwin::BgWinPlugin)
    // macOS `Cmd+Q` is wired to `terminate:`, which leaves the event loop with no `AppExit`, and
    // `report_check` needs one for the exit code.
    .add_plugins(crate::mac_quit::MacQuitPlugin);

    // The cut line: the engine's whole plugin group, the same one the client adds.
    app.add_plugins(crate::world_plugins::WorldPlugins);
    stubs(&mut app);

    app.add_plugins(plugin);

    // After `AssetPlugin`: the loaders go into the live `AssetServer`, as in the client.
    benilla_assets::register_asset_loaders(&mut app);

    let exit = app.run();
    match check {
        Some(_) => report_check(exit),
        None => exit,
    }
}

/// `WOW_WORLDVIEW_CHECK` in seconds: unset is off, bare or unparseable is [`CHECK_SECS_DEFAULT`].
fn check_seconds() -> Option<f32> {
    let v = std::env::var("WOW_WORLDVIEW_CHECK").ok()?;
    Some(v.trim().parse().unwrap_or(CHECK_SECS_DEFAULT))
}

/// Long enough on a warm cache for every engine system to run and validate its parameters.
const CHECK_SECS_DEFAULT: f32 = 10.0;

/// Wall-clock seconds the check runs for.
#[derive(Resource)]
struct CheckDeadline(f32);

/// Every distinct fault the check saw, by the ECS construct that raised it. A `static` because
/// Bevy's error handler is a bare `fn` pointer and cannot capture.
static FAULTS: std::sync::Mutex<std::collections::BTreeSet<String>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// The check's error handler: records and warns, so one run names every fault.
fn record_fault(error: bevy::ecs::error::BevyError, ctx: bevy::ecs::error::ErrorContext) {
    let entry = format!("{} `{}`: {error}", ctx.kind(), ctx.name());
    warn!("worldview: {entry}");
    FAULTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(entry);
}

fn end_check(
    time: Res<Time<bevy::time::Real>>,
    deadline: Res<CheckDeadline>,
    mut exit: MessageWriter<AppExit>,
) {
    if time.elapsed_secs() >= deadline.0 {
        exit.write(AppExit::Success);
    }
}

/// Prints the check's verdict and makes it the process exit code.
fn report_check(exit: AppExit) -> AppExit {
    let faults = FAULTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if faults.is_empty() {
        println!("WORLDVIEW_CHECK ok — the engine booted and ran with no game attached");
        return exit;
    }
    for f in faults.iter() {
        println!("WORLDVIEW_CHECK fault {f}");
    }
    println!(
        "WORLDVIEW_CHECK {} fault(s) — a game concept is wired into the engine, or an engine \
         fact is parked on the game side.",
        faults.len()
    );
    // A run with no install (`WOW_DATA=`) faults for a different reason, so it says which.
    if benilla_formats::wow_data().is_none() {
        println!(
            "WORLDVIEW_CHECK ran with NO INSTALL: a fault here is a system taking a resource that \
             only exists when there is client data as a hard `Res`/`ResMut`. Take it as `Option` \
             and return, or insert it ahead of the no-data bail."
        );
    }
    AppExit::error()
}
/// What the engine still needs told: only whether there is a world. Coupling that never panics is
/// invisible to this binary: an ordering edge onto an unregistered system is dropped, an
/// `Option<Res<…>>` sees `None`, and a query on a component nobody spawns matches nothing.
fn stubs(app: &mut App) {
    // The viewer's world is always live; whatever composes the engine writes `WorldLive`.
    app.insert_resource(crate::schedule::WorldLive(true));
}

/// The viewer itself: the free-fly camera and its controller.
fn plugin(app: &mut App) {
    app.add_systems(
        Startup,
        spawn_view_camera.after(benilla_assets::AssetSet::Open),
    )
    .add_systems(Update, fly.in_set(crate::schedule::WorldStage::Input));

    // `WOW_WORLDVIEW_SHOT=<png>` (at `WOW_WORLDVIEW_SHOT_AT` seconds, default 20) writes one frame
    // and exits, with no subject gate: the client's live shot needs a player the engine lacks.
    if std::env::var("WOW_WORLDVIEW_SHOT").is_ok() {
        app.add_systems(Update, shoot_and_exit);
    }
}

/// Fires the `WOW_WORLDVIEW_SHOT` screenshot once and exits two seconds later: the write is
/// asynchronous, and an immediate exit loses the PNG.
fn shoot_and_exit(
    time: Res<Time>,
    mut commands: Commands,
    mut fired_at: Local<Option<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    let at = std::env::var("WOW_WORLDVIEW_SHOT_AT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.0);
    match *fired_at {
        None if time.elapsed_secs() >= at => {
            let path = std::env::var("WOW_WORLDVIEW_SHOT").unwrap_or_default();
            info!("worldview: writing {path}");
            commands
                .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
                .observe(bevy::render::view::screenshot::save_to_disk(path));
            *fired_at = Some(time.elapsed_secs());
        }
        Some(t) if time.elapsed_secs() >= t + 2.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

/// The viewer's minimal free-fly camera, not a twin of the client's `FlyCam`.
#[derive(Component)]
struct ViewCam {
    yaw: f32,
    pitch: f32,
    speed: f32,
}

fn spawn_view_camera(mut commands: Commands, msaa: Res<crate::view::MsaaSetting>) {
    let start = wow_to_bevy(view_start());
    let far = crate::view::CAM_FAR;
    commands.spawn((
        Camera3d::default(),
        crate::view::WorldCamera,
        msaa.level(),
        Projection::from(PerspectiveProjection {
            far,
            near: NEAR,
            fov: FOVY,
            ..default()
        }),
        Hdr,
        Tonemapping::None,
        crate::ffx_glow::FfxGlow::WORLD,
        Transform::from_translation(start + Vec3::new(0.0, VIEW_START_HEIGHT, VIEW_START_HEIGHT))
            .looking_at(start, Vec3::Y),
        ViewCam {
            yaw: 0.0,
            pitch: -0.5,
            speed: 100.0,
        },
    ));
}

/// WASD, Space and C fly, a right or left drag looks, Ctrl boosts, the wheel sets speed.
fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut window: Query<&mut bevy::window::CursorOptions, With<PrimaryWindow>>,
    mut cam: Query<(&mut Transform, &mut ViewCam)>,
) {
    let Ok((mut xf, mut cam)) = cam.single_mut() else {
        return;
    };

    let looking = buttons.pressed(MouseButton::Right) || buttons.pressed(MouseButton::Left);
    if let Ok(mut cursor) = window.single_mut() {
        let want = if looking {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        if cursor.grab_mode != want {
            cursor.grab_mode = want;
            cursor.visible = !looking;
        }
    }
    if looking {
        const LOOK: f32 = 0.003;
        cam.yaw -= motion.delta.x * LOOK;
        cam.pitch = (cam.pitch - motion.delta.y * LOOK).clamp(-1.54, 1.54);
    }
    xf.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);

    for ev in wheel.read() {
        cam.speed = (cam.speed * (1.0 + ev.y * 0.1)).clamp(1.0, 2000.0);
    }

    let mut dir = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        dir += *xf.forward();
    }
    if keys.pressed(KeyCode::KeyS) {
        dir += *xf.back();
    }
    if keys.pressed(KeyCode::KeyA) {
        dir += *xf.left();
    }
    if keys.pressed(KeyCode::KeyD) {
        dir += *xf.right();
    }
    if keys.pressed(KeyCode::Space) {
        dir += Vec3::Y;
    }
    if keys.pressed(KeyCode::KeyC) {
        dir -= Vec3::Y;
    }
    if dir != Vec3::ZERO {
        let boost = if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
            5.0
        } else {
            1.0
        };
        xf.translation += dir.normalize() * cam.speed * boost * time.delta_secs();
    }
}
