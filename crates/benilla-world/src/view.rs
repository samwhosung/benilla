//! The view: the camera the scene renders through ([`WorldCamera`]), its optic ([`CAM_FOVY`]),
//! and how far the detailed world draws ([`ViewDistance`], the reference's `nearclip` and
//! `farclip` pair). `farclip` bounds the per-pixel wall, the CPU cull ([`within_farclip`]) and
//! the terrain residency window (`terrain_stream::window`), which derives its reach from it as the
//! reference does.

use bevy::prelude::*;

/// The viewer's own body, as the world needs it: the eye point for the WMO interior probe, a wading
/// body for the water foam, the commanded speed for the precipitation tilt. Defaulted, it is no
/// body at all, which is what the world viewer runs with.
#[derive(Resource, Clone, Copy)]
pub struct Viewer {
    /// The avatar's position in Bevy space; `None` with no live avatar or a detached eye.
    pub at: Option<Vec3>,
    /// Its last-streamed `MOVEMENTFLAGS` (the reference caches them at `unit+0x9e8`); 0, no body.
    pub move_flags: u32,
    /// The commanded planar speed in yd/s (`[[player+0x118]+0x84]`), zero with no direction key.
    pub planar_speed: f32,
    /// Its collision cylinder height in yards, the foam ring's radius input.
    pub height: f32,
    /// The first-person feather: the body's opacity, `1.0` normally, `0.0` at full zoom-in.
    pub self_fade: f32,
    /// Drunkenness, `0.0..=1.0`: `PLAYER_BYTES_3` byte 1, clamped at 100; the full-screen haze.
    pub drunk: f32,
    /// A ghost: the active `LightParams` slot is 4, the death profile, applied instantly
    /// (`0x6d4620` sets it, `0x6d2260` re-derives it every frame).
    pub ghost: bool,
    /// A loading cover is over the world. The appear ramp arms on its falling edge, since the
    /// reference's trigger is the player seeing the entity.
    pub world_covered: bool,
}

impl Default for Viewer {
    /// No body and nothing covering the world; `self_fade` 1.0, nothing feathered.
    fn default() -> Self {
        Self {
            at: None,
            move_flags: 0,
            planar_speed: 0.0,
            height: 0.0,
            self_fade: 1.0,
            drunk: 0.0,
            ghost: false,
            world_covered: false,
        }
    }
}

impl Viewer {
    /// The body is translating: the four direction bits (`& 0xf`), the test the reference's
    /// water-ripple driver runs (`0x5fa760`).
    pub(crate) fn translating(&self) -> bool {
        self.move_flags & 0xf != 0
    }

    /// The body is turning in place: the two keyboard turn bits (`& 0x30`). A strafe is
    /// [`Self::translating`]; a mouse-look body step sets no flag.
    pub(crate) fn turning(&self) -> bool {
        self.move_flags & 0x30 != 0
    }
}

/// View distance in yards. `farclip`, the `farclip` CVar, is the one view distance: the detailed
/// world's far wall and the reach of terrain residency; its default, 350, is the reference's
/// registered one.
/// `nearclip` shares the resource because `0x511bc0` stamps both onto the camera every frame, the
/// `nearclip` record's float to `[cam+0x38]` (`0x511bd4`) and the `farclip` one's to `[cam+0x3c]`.
#[derive(Resource, Clone, Copy)]
pub struct ViewDistance {
    pub farclip: f32,
    /// The camera near plane in yards, the `nearclip` CVar ([`stamp_near_clip`]).
    pub nearclip: f32,
}

/// The settable range of [`ViewDistance::farclip`]: the reference's `farclip` clamp,
/// `[0x81021c]` = 177 to `[0x80fed8]` = 777 in its validate callback `0x688d40`; shared by the
/// CVar apply, the options row and `$WOW_FARCLIP`.
pub const FARCLIP_RANGE: std::ops::RangeInclusive<f32> = 177.0..=777.0;

/// The settable range of [`ViewDistance::nearclip`]: the bounds in the reference's `nearclip`
/// change callback `0x688d90`, `[0x8029d0]` = 0.01 and `[0x808300]` = 0.33.
///
/// Deviation: the reference refuses a value outside them, echoing "NearClip must be in range
/// 0.01 - 0.33" (`0x869abc`); ours clamps, as every CVar consumer here clamps at its own edge.
pub const NEARCLIP_RANGE: std::ops::RangeInclusive<f32> = 0.01..=0.33;

/// The world camera's multisampling, the `gxMultisample` CVar: a sample count in [`MSAA_RANGE`],
/// 1 meaning none, as both reference backends read it (D3D9 `0x599899`, GL `0x59de32`). Its
/// default, 1, is the reference's: `CVar::Register` (`0x63a950`) takes it from the
/// `VideoHardware.dbc` row `DetectHardware` (`0x641260`) matched, and every fallback row holds 1.
///
/// Latched, as the reference registers it ("set pending gxRestart"): a change persists, but the
/// camera keeps its `Msaa` until the next launch (swapping it live would freeze our post passes).
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct MsaaSetting {
    /// The requested sample count; 1 is none.
    pub samples: u32,
}

/// The settable range of [`MsaaSetting::samples`]: the reference's `atoi`-then-clamp at
/// `0x63b250`.
pub const MSAA_RANGE: std::ops::RangeInclusive<u32> = 1..=16;

impl Default for MsaaSetting {
    /// `$WOW_MSAA` overrides the default for the session; `off`, `0` and `1` all mean none.
    fn default() -> Self {
        let samples = match std::env::var("WOW_MSAA").ok().as_deref() {
            Some("off") => Some(1),
            Some(v) => v.parse::<u32>().ok(),
            None => None,
        }
        .map_or(1, |v| v.clamp(*MSAA_RANGE.start(), *MSAA_RANGE.end()));
        Self { samples }
    }
}

impl MsaaSetting {
    /// The sample count as a Bevy [`Msaa`](bevy::render::view::Msaa) level: wgpu expresses only 1,
    /// 2, 4 and 8, so a request rounds down, the direction the reference's device-init retry loop
    /// steps (`0x63b380`, `-= 2` floored at 1).
    pub fn level(self) -> bevy::render::view::Msaa {
        use bevy::render::view::Msaa;
        match self.samples {
            0..=1 => Msaa::Off,
            2..=3 => Msaa::Sample2,
            4..=7 => Msaa::Sample4,
            _ => Msaa::Sample8,
        }
    }
}

/// The sample counts this GPU accepts in all three formats we multisample into (the
/// `Rgba16Float` target, `Depth32Float` depth, the swapchain), each a separate wgpu capability;
/// Bevy passes a count on unchecked, and wgpu fails validation on one the device lacks.
fn supported_sample_counts(adapter: &bevy::render::renderer::RenderAdapter) -> Vec<u32> {
    use bevy::image::BevyDefault as _;
    use bevy::render::render_resource::TextureFormat;

    let formats = [
        TextureFormat::Rgba16Float,
        TextureFormat::Depth32Float,
        TextureFormat::bevy_default(),
    ];
    MSAA_RANGE
        .clone()
        .filter(|&n| {
            formats.iter().all(|f| {
                adapter
                    .get_texture_format_features(*f)
                    .flags
                    .sample_count_supported(n)
            })
        })
        .collect()
}

/// The largest count in `supported` at or below `requested`, floored at 1, which no device refuses.
pub fn clamp_to_supported(requested: u32, supported: &[u32]) -> u32 {
    supported
        .iter()
        .copied()
        .filter(|&n| n <= requested)
        .max()
        .unwrap_or(1)
}

/// The multisample formats this device accepts, `(color_bits, depth_bits, samples)` ascending, for
/// the Video options dropdown, as the reference's triple list at `[0xb4b444]` (built by
/// `0x48c3e0`); empty with no adapter.
#[derive(Resource, Default, Clone, Debug)]
pub struct MsaaFormats {
    pub formats: Vec<(u32, u32, u32)>,
}

impl MsaaFormats {
    /// `requested`, stepped down to the nearest count this device offers, for the startup clamp
    /// and every CVar write; an empty list (no adapter) passes it through.
    pub fn clamp(&self, requested: u32) -> u32 {
        if self.formats.is_empty() {
            return requested;
        }
        let counts: Vec<u32> = self.formats.iter().map(|&(_, _, n)| n).collect();
        clamp_to_supported(requested, &counts)
    }
}

/// Colour and depth bits for the dropdown's `MULTISAMPLING_FORMAT_STRING`, from the formats we
/// render into; colour is the swapchain's, the mode the window presents in.
fn dropdown_bit_depths() -> (u32, u32) {
    use bevy::image::BevyDefault as _;
    use bevy::render::render_resource::TextureFormat;

    let bits = |f: TextureFormat| f.block_copy_size(None).unwrap_or(4) * 8;
    (
        bits(TextureFormat::bevy_default()),
        bits(TextureFormat::Depth32Float),
    )
}

/// Clamps [`MsaaSetting`] to this GPU and publishes [`MsaaFormats`], in `finish`, the first moment
/// `RenderAdapter` exists and still before any camera spawns; headless, it does nothing.
pub struct MsaaSupportPlugin;

impl Plugin for MsaaSupportPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        // Scoped so the adapter borrow ends before the resource is written.
        let supported = match app
            .world()
            .get_resource::<bevy::render::renderer::RenderAdapter>()
        {
            Some(adapter) => supported_sample_counts(adapter),
            None => return,
        };
        // Published whether or not anything clamps: it is the dropdown's whole menu.
        let (color_bits, depth_bits) = dropdown_bit_depths();
        app.insert_resource(MsaaFormats {
            formats: supported
                .iter()
                .map(|&s| (color_bits, depth_bits, s))
                .collect(),
        });
        let Some(requested) = app.world().get_resource::<MsaaSetting>().map(|m| m.samples) else {
            return;
        };
        // Through `MsaaFormats::clamp`, the same rule as the app's per-write `gxMultisample`
        // clamp (`video::on_cvar`).
        let granted = app.world().resource::<MsaaFormats>().clamp(requested);
        if granted == requested {
            debug!("msaa: {requested}x accepted (this GPU offers {supported:?})");
            return;
        }
        warn!(
            "msaa: this GPU does not offer {requested}x — using {granted}x (it offers {supported:?})"
        );
        app.world_mut().resource_mut::<MsaaSetting>().samples = granted;
    }
}

/// The world camera's projection far plane in yards. Deviation: the reference projects the
/// detailed world to `farclip` and the horizon to `horizonfarclip` (default 2112, floored at
/// `farclip + 528`); ours has one far plane well beyond `farclip`, because the WDL ring draws in
/// the same projection, and the detailed world ends at `farclip` by the wall.
pub const CAM_FAR: f32 = 3000.0;

impl ViewDistance {
    /// Set the near plane from a `nearclip` write, clamped to [`NEARCLIP_RANGE`].
    pub fn set_nearclip(&mut self, v: f32) {
        self.nearclip = v.clamp(*NEARCLIP_RANGE.start(), *NEARCLIP_RANGE.end());
    }
}

impl Default for ViewDistance {
    /// `$WOW_FARCLIP` (yd, clamped to [`FARCLIP_RANGE`]) overrides the 350 default, read once at
    /// startup, so a headless capture can reproduce a player's view distance.
    fn default() -> Self {
        let farclip = std::env::var("WOW_FARCLIP")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .map_or(350.0, |v| {
                v.clamp(*FARCLIP_RANGE.start(), *FARCLIP_RANGE.end())
            });
        Self {
            farclip,
            nearclip: NEARCLIP_DEFAULT,
        }
    }
}

/// Whether a world bounding sphere is inside the far-clip wall, the one CPU-side test of it: planar
/// depth along camera-forward to the sphere's nearest point, the coordinate the shaders' per-pixel
/// wall discards on (eye-Z past `fog_params.w`), so a straddling object dissolves instead of
/// popping. The frustum's far plane is [`CAM_FAR`], so it is no substitute: this test and the
/// shaders' discard are the whole `farclip` bound.
pub fn within_farclip(
    farclip: f32,
    cam_pos: Vec3,
    cam_fwd: Vec3,
    center: Vec3,
    radius: f32,
) -> bool {
    (center - cam_pos).dot(cam_fwd) - radius <= farclip
}

/// Marks the world camera, the one the scene renders through. Viewer consumers filter on this, not
/// on `Camera3d`: the portrait booths add off-screen `Camera3d`s.
#[derive(Component)]
pub struct WorldCamera;

/// The `$WOW_MSAA` knob as a Bevy `Msaa` level, 4x when unset. `8` falls back to 4x:
/// `Rgba16Float` is only guaranteed `[1, 4]` samples, and there is no adapter to ask here.
pub fn msaa_from_env() -> bevy::render::view::Msaa {
    use bevy::render::view::Msaa;
    match std::env::var("WOW_MSAA").ok().as_deref() {
        Some("off") | Some("0") | Some("1") => Msaa::Off,
        Some("2") => Msaa::Sample2,
        Some("8") => {
            warn!(
                "WOW_MSAA=8: Rgba16Float is only guaranteed [1, 4] samples and this adapter \
                 reports [1, 2, 4] — falling back to 4x rather than panicking the render thread"
            );
            Msaa::Sample4
        }
        _ => Msaa::Sample4,
    }
}

/// The `nearclip` CVar's registered default, 0.1 (the string at `0x84fb48`, registered at
/// `0x68867a`, record `[0xc7f348]`); the camera ctor's 1/9 (`0x50a6c0`) is overwritten from the
/// record in the first frame, before it renders. The small near keeps the waterline-crossing band
/// inches tall (`liquid::detect_submersion`), and the self-avatar fade completes over it
/// ([`crate::model_fade::self_model_fade_alpha`]), as in the reference, where both are
/// `[cam+0x38]`.
pub const NEARCLIP_DEFAULT: f32 = 0.1;

/// The reference's `0x511bc0` near stamp: the live `nearclip` onto the world camera's projection
/// every frame. The far plane is not stamped: ours is [`CAM_FAR`].
pub fn stamp_near_clip(
    view: Res<ViewDistance>,
    mut cam: Query<&mut Projection, With<WorldCamera>>,
) {
    for mut proj in &mut cam {
        // `&mut` marks the component changed even when the value is equal, so compare first.
        let Projection::Perspective(current) = proj.as_ref() else {
            continue;
        };
        if current.near == view.nearclip {
            continue;
        }
        if let Projection::Perspective(p) = &mut *proj {
            p.near = view.nearclip;
        }
    }
}
/// The vertical field of view in radians, shared by the projection and every consumer of the near
/// rectangle: 45°, Bevy's `PerspectiveProjection` default; the reference's follows the aspect,
/// 44.1° at 16:9.
pub const CAM_FOVY: f32 = std::f32::consts::FRAC_PI_4;

/// The world camera's pose made current within `Update`, for every viewer authority there: Bevy
/// propagates `GlobalTransform` in `PostUpdate`, a whole jump late after a teleport snap.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CameraPoseSet;

/// The root world cameras' two transforms: [`publish_camera_pose`] copies `Transform` into
/// `GlobalTransform` early ([`CameraPoseSet`]), `set_if_neq` so a still camera stays unchanged.
type RootCameraPose<'w, 's> =
    Query<'w, 's, (&'static Transform, &'static mut GlobalTransform), RootCamera>;
/// A world camera with no parent, for which Bevy's `PostUpdate` propagation is exactly that copy;
/// a parented camera keeps Bevy's value and its one-frame lag.
type RootCamera = (With<WorldCamera>, Without<ChildOf>);

pub(crate) fn publish_camera_pose(mut cam: RootCameraPose) {
    for (t, mut g) in &mut cam {
        g.set_if_neq(GlobalTransform::from(*t));
    }
}

/// The view lane's plugin: [`stamp_near_clip`] and [`publish_camera_pose`].
pub(crate) struct ViewPlugin;

impl Plugin for ViewPlugin {
    fn build(&self, app: &mut App) {
        plugin(app);
    }
}

pub(crate) fn plugin(app: &mut App) {
    // In `Update`, so this frame's `nearclip` write reaches this frame's projection, as `0x511bc0`
    // runs inside the world-frame driver; initialized here too for a harness with only this plugin.
    app.init_resource::<ViewDistance>()
        .add_systems(Update, stamp_near_clip);
    app.add_systems(
        Update,
        publish_camera_pose
            .in_set(CameraPoseSet)
            // After the controller writes this frame's `Transform`.
            .after(crate::schedule::WorldStage::Input),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// `0x511bc0`'s near stamp reaches the projection, and an unchanged one stays unmarked.
    #[test]
    fn the_near_plane_follows_the_cvar_and_settles() {
        let mut app = App::new();
        app.add_systems(Update, stamp_near_clip);
        let cam = app
            .world_mut()
            .spawn((
                WorldCamera,
                Projection::from(PerspectiveProjection {
                    near: NEARCLIP_DEFAULT,
                    ..default()
                }),
            ))
            .id();
        // A `nearclip 0.3` write, from a player or an addon's `ConsoleExec`.
        app.insert_resource(ViewDistance {
            farclip: 350.0,
            nearclip: 0.3,
        });
        app.update();
        let near = |app: &App| match app.world().get::<Projection>(cam).unwrap() {
            Projection::Perspective(p) => p.near,
            _ => unreachable!("spawned perspective"),
        };
        assert_eq!(near(&app), 0.3, "the stamp is the reference's, every frame");

        // A second frame with nothing moved must not mark the projection changed.
        let tick = app.world().read_change_tick();
        app.update();
        assert_eq!(near(&app), 0.3);
        let now = app.world().read_change_tick();
        assert!(
            !app.world()
                .entity(cam)
                .get_ref::<Projection>()
                .unwrap()
                .last_changed()
                .is_newer_than(tick, now),
            "an unchanged near plane must not dirty the projection"
        );
    }

    /// [`CameraPoseSet`]'s contract: a reader ordered after it sees this frame's teleport snap.
    #[test]
    fn a_reader_after_the_pose_set_sees_this_frames_camera() {
        #[derive(Resource, Default)]
        struct Seen(Vec3);

        fn snap_the_camera(mut cam: Query<&mut Transform, With<WorldCamera>>) {
            for mut t in &mut cam {
                t.translation = Vec3::new(500.0, 0.0, 0.0);
            }
        }
        fn read_the_pose(cam: Query<&GlobalTransform, With<WorldCamera>>, mut seen: ResMut<Seen>) {
            for g in &cam {
                seen.0 = g.translation();
            }
        }

        let mut app = App::new();
        app.init_resource::<Seen>();
        app.configure_sets(Update, crate::schedule::WorldStage::Input);
        app.add_systems(
            Update,
            snap_the_camera.in_set(crate::schedule::WorldStage::Input),
        );
        plugin(&mut app);
        app.add_systems(Update, read_the_pose.after(CameraPoseSet));
        app.world_mut().spawn((
            WorldCamera,
            Transform::default(),
            GlobalTransform::default(),
        ));

        app.update();
        assert_eq!(
            app.world().resource::<Seen>().0,
            Vec3::new(500.0, 0.0, 0.0),
            "an Update-stage viewer authority must see the snapped pose, not the frame-old one"
        );
    }

    /// A parented camera is left to Bevy: a child's local `Transform` is not a world pose.
    #[test]
    fn a_parented_camera_is_left_alone() {
        let mut app = App::new();
        plugin(&mut app);
        let parent = app.world_mut().spawn(Transform::default()).id();
        let child = app
            .world_mut()
            .spawn((
                WorldCamera,
                Transform::from_xyz(7.0, 0.0, 0.0),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut().entity_mut(parent).add_child(child);

        app.world_mut()
            .run_system_once(publish_camera_pose)
            .unwrap();
        assert_eq!(
            app.world()
                .entity(child)
                .get::<GlobalTransform>()
                .unwrap()
                .translation(),
            Vec3::ZERO,
            "the child's local transform is not a world pose"
        );
    }
    /// The wall is planar depth along camera-forward, measured to the sphere's nearest point.
    #[test]
    fn planar_depth_of_the_nearest_point() {
        let eye = Vec3::ZERO;
        let fwd = Vec3::NEG_Z; // Bevy's camera looks down −Z
        let at = |d: f32, r: f32| within_farclip(777.0, eye, fwd, Vec3::new(0.0, 0.0, -d), r);
        assert!(at(700.0, 0.0));
        assert!(at(777.0, 0.0)); // exactly at the wall still draws (the shader discards past it)
        assert!(!at(778.0, 0.0));
        // A big object straddling the wall stays in; the per-pixel wall dissolves its far half.
        assert!(at(800.0, 30.0));
        assert!(!at(900.0, 30.0));
    }

    /// A point 700 yd forward and 700 yd aside (radially ~990) is inside the planar wall.
    #[test]
    fn the_wall_is_a_plane_not_a_sphere() {
        let eye = Vec3::ZERO;
        let fwd = Vec3::NEG_Z;
        let off = Vec3::new(700.0, 0.0, -700.0);
        assert!(off.length() > 777.0, "radially outside");
        assert!(
            within_farclip(777.0, eye, fwd, off, 0.0),
            "but inside the planar wall — must match the shader, which discards on eye-Z"
        );
    }

    /// Behind the camera is inside the wall; the frustum's side planes reject it, never an `abs()`.
    #[test]
    fn behind_the_camera_is_not_this_tests_job() {
        let eye = Vec3::ZERO;
        assert!(within_farclip(
            777.0,
            eye,
            Vec3::NEG_Z,
            Vec3::new(0.0, 0.0, 5000.0),
            0.0
        ));
    }
}

#[cfg(test)]
mod msaa_tests {
    use super::*;
    use bevy::render::view::Msaa;

    /// The default is off, spelled 1 on the reference's scale.
    #[test]
    fn the_default_is_the_references_one_sample_which_means_none() {
        // The literal, not `MsaaSetting::default()`, which reads `$WOW_MSAA`.
        let literal = MsaaSetting { samples: 1 };
        assert_eq!(literal.level(), Msaa::Off);
        assert_eq!(*MSAA_RANGE.start(), 1);
        assert_eq!(*MSAA_RANGE.end(), 16);
    }

    /// A request steps down to the nearest offered count, as in the reference (`0x63b380`).
    #[test]
    fn an_unsupported_count_steps_down_to_the_nearest_the_device_offers() {
        // A player types 8 on a device that stops at 4x.
        let upto4 = [1, 2, 4];
        assert_eq!(clamp_to_supported(8, &upto4), 4);
        assert_eq!(clamp_to_supported(16, &upto4), 4);
        assert_eq!(clamp_to_supported(4, &upto4), 4);
        assert_eq!(clamp_to_supported(3, &upto4), 2);
        assert_eq!(clamp_to_supported(1, &upto4), 1);
    }

    #[test]
    fn a_supported_count_is_never_stepped_up_and_never_touched() {
        let all = [1, 2, 4, 8, 16];
        for n in all {
            assert_eq!(clamp_to_supported(n, &all), n, "{n}x should pass through");
        }
        // Between two offered levels it rounds down.
        assert_eq!(clamp_to_supported(6, &all), 4);
        assert_eq!(clamp_to_supported(15, &all), 8);
    }

    #[test]
    fn a_device_that_offers_nothing_still_boots_at_one_sample() {
        // An empty list means some format refused even 1x; the floor is still 1, not a panic.
        assert_eq!(clamp_to_supported(8, &[]), 1);
        assert_eq!(clamp_to_supported(1, &[]), 1);
        assert_eq!(MsaaSetting { samples: 1 }.level(), Msaa::Off);
    }

    /// [`MsaaFormats::clamp`]: a device list steps a request down, an empty list passes it through.
    #[test]
    fn the_device_list_steps_down_and_an_empty_one_has_no_opinion() {
        let apple = MsaaFormats {
            formats: vec![(32, 32, 1), (32, 32, 2), (32, 32, 4)],
        };
        assert_eq!(apple.clamp(4), 4, "a count it offers is untouched");
        assert_eq!(apple.clamp(3), 2, "down to the nearest offered, never up");
        assert_eq!(apple.clamp(8), 4, "unoffered 8x panics the render thread");
        assert_eq!(apple.clamp(16), 4);
        assert_eq!(apple.clamp(1), 1);

        let headless = MsaaFormats::default();
        assert!(headless.formats.is_empty());
        for n in [1, 2, 4, 8, 16] {
            assert_eq!(headless.clamp(n), n, "no adapter, no opinion");
        }
    }

    #[test]
    fn a_sample_count_rounds_down_never_up() {
        let level = |n| MsaaSetting { samples: n }.level();
        assert_eq!(level(1), Msaa::Off);
        assert_eq!(level(2), Msaa::Sample2);
        assert_eq!(level(3), Msaa::Sample2);
        assert_eq!(level(4), Msaa::Sample4);
        assert_eq!(level(7), Msaa::Sample4);
        assert_eq!(level(8), Msaa::Sample8);
        // The reference's top, 16, maps to wgpu's top level, 8.
        assert_eq!(level(16), Msaa::Sample8);
        // 0 cannot pass the clamp, but must not panic.
        assert_eq!(level(0), Msaa::Off);
    }
}
