//! The five camera views (`SetView`, `SaveView`, `ResetView`, `NextView`, `PrevView`) and
//! `FlipCameraYaw`: the engine half of [`benilla_ui::script::CameraViewRequest`].
//!
//! The reference's rows are `FIRST_PERSON` and `THIRD_PERSON_A`..`D` (names at `0x84f848`); Lua
//! `SetView(n)` takes row `n - 1`. Pitch is in degrees down-positive, the opposite of
//! [`FlyCam::pitch`], converted only by [`super::camera_saved::pitch_from_file`]. Row 0's distance
//! `0.0` is real first person ([`CAM_DIST_MIN`] is `0.0`).
//!
//! The views persist as the reference's archived CVars (registered at `0x50b9b0`). [`CameraViews`]
//! is seeded from them once and writes every change back, so a live `SetCVar` does not reach the
//! slots; the reference also loads them only at camera construction (`0x50fb80`). The live view is
//! remembered, not applied at startup: [`super::camera_saved`]'s pose lands over it, as the
//! reference's saved pose lands over `cameraView`'s row at UI load.
//!
//! Deviation: `ResetView` writes the CVars back to their defaults, because the CVar store is our
//! persistence; the reference (`0x50fae0`) touches no CVar, so its reset reverts at next launch.
//!
//! `SetView` glides the distance and snaps pitch and yaw: the reference arms three glide channels
//! (`0x513189`, `0x5131a3`, `0x5131b8`), the rig has only the distance one.

use bevy::prelude::*;

use benilla_ui::script::{CameraViewRequest, UiScript, CAMERA_VIEW_COUNT};
use benilla_world::view::WorldCamera;

use crate::creature_anim::wrap_pi;

use super::camera::{CameraControl, FlyCam, CAM_DIST_MAX, CAM_DIST_MIN, CAM_PITCH_LIMIT};
use super::camera_saved::{pitch_from_file, pitch_to_file};
use super::Player;

/// The reference's five views, through the UI crate's constant so the Lua range check agrees.
const VIEW_COUNT: usize = CAMERA_VIEW_COUNT as usize;

/// The live view's CVar (`0x84fdc8`), holding the internal index as `"%d"` (`0x512ef1`).
pub(crate) const CVAR_ACTIVE_VIEW: &str = "cameraView";

/// The fifteen saved-view CVars, `[view][field]` (distance, pitch, yaw), as `0x50b9b0` composes
/// them; [`crate::cvars::REGISTERED`] must hold the same names (tested).
pub(crate) const VIEW_CVARS: [[&str; 3]; VIEW_COUNT] = [
    ["cameraDistance", "cameraPitch", "cameraYaw"],
    ["cameraDistanceA", "cameraPitchA", "cameraYawA"],
    ["cameraDistanceB", "cameraPitchB", "cameraYawB"],
    ["cameraDistanceC", "cameraPitchC", "cameraYawC"],
    ["cameraDistanceD", "cameraPitchD", "cameraYawD"],
];

/// The reference's default strings (`0x84f488`), kept as text: [`crate::cvars`]'s file is a diff
/// against the registered default string, so a reset strips the key only by writing this text.
pub(crate) const VIEW_DEFAULTS: [[&str; 3]; VIEW_COUNT] = [
    ["0.0", "0.0", "0.0"],
    ["5.55", "10.0", "0.0"],
    ["5.55", "20.0", "0.0"],
    ["13.88", "30.0", "0.0"],
    ["13.88", "10.0", "0.0"],
];

/// [`CVAR_ACTIVE_VIEW`]'s default (`[0x84f484]`): internal index 1, `THIRD_PERSON_A`.
pub(crate) const ACTIVE_VIEW_DEFAULT: &str = "1";

/// One view's pose: yards, radians with [`FlyCam::pitch`]'s up-positive sign, and `yaw` as an
/// offset from the subject's facing, which the reference re-adds every frame (`0x50f7f2`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ViewPose {
    pub(crate) distance: f32,
    pub(crate) pitch: f32,
    pub(crate) yaw: f32,
}

impl ViewPose {
    /// From the CVar units (yards, degrees down-positive, degrees), clamped to the rig's zoom range
    /// and ±89° pitch (reference validators: `[0, 50]` at `0x50b310`, ±89° at `0x50b380`). Yaw is
    /// wrapped, as the reference validates `[0, 360]` (`0x50b3c0`) but `SaveView` writes an offset.
    fn from_reference(distance: f32, pitch_deg: f32, yaw_deg: f32) -> Self {
        Self {
            distance: distance.clamp(CAM_DIST_MIN, CAM_DIST_MAX),
            pitch: pitch_from_file(pitch_deg),
            yaw: wrap_pi(yaw_deg.to_radians()),
        }
    }

    /// The three CVar strings, in the reference's `"%f"` (`0x50f9f3`, format `0x835160`).
    fn to_reference(self) -> [String; 3] {
        [
            format!("{:.6}", self.distance),
            format!("{:.6}", pitch_to_file(self.pitch)),
            format!("{:.6}", self.yaw.to_degrees()),
        ]
    }

    fn shipped(view: usize) -> Self {
        let row = VIEW_DEFAULTS[view];
        let parse = |s: &str| {
            s.parse::<f32>()
                .expect("the reference's default strings are floats")
        };
        Self::from_reference(parse(row[0]), parse(row[1]), parse(row[2]))
    }
}

/// The five views and the live one: the reference's `[cam+0xb0..+0xdc]` and `[cam+0xac]`.
#[derive(Resource, Debug, PartialEq)]
pub(crate) struct CameraViews {
    views: [ViewPose; VIEW_COUNT],
    /// The live view, persisted as [`CVAR_ACTIVE_VIEW`].
    current: usize,
}

impl Default for CameraViews {
    fn default() -> Self {
        Self {
            views: std::array::from_fn(ViewPose::shipped),
            current: ACTIVE_VIEW_DEFAULT
                .parse::<usize>()
                .expect("the registrar default is an index"),
        }
    }
}

impl CameraViews {
    fn pose(&self, view: usize) -> ViewPose {
        self.views[view]
    }

    /// `SetView`, and whether the index moved: the gate on writing the CVar (`0x512ee6`).
    fn set(&mut self, view: usize) -> (ViewPose, bool) {
        let moved = self.current != view;
        self.current = view;
        (self.views[view], moved)
    }

    /// `SaveView`: the reference stores the camera's target fields, not the live ones (`0x50fa30`).
    fn save(&mut self, view: usize, pose: ViewPose) {
        self.views[view] = pose;
    }

    /// `ResetView`; `true` when `view` is live, which the reference re-applies (`0x50fb5b`).
    fn reset(&mut self, view: usize) -> bool {
        self.views[view] = ViewPose::shipped(view);
        self.current == view
    }

    /// `NextView`/`PrevView`, with no wrap at either end (`0x50faa0`, `0x50fac0`).
    fn step(&self, forward: bool) -> Option<usize> {
        if forward {
            (self.current + 1 < VIEW_COUNT).then(|| self.current + 1)
        } else {
            self.current.checked_sub(1)
        }
    }
}

/// Startup: seed the views and the live index from the CVar registry, which exists from `CvarLoad`
/// on while the VM's mirror does not yet; an absent key is the shipped default.
fn load_saved_views(cvars: Res<crate::cvars::Cvars>, mut views: ResMut<CameraViews>) {
    for (view, slot) in views.views.iter_mut().enumerate() {
        // Each field falls back on its own: a bad value costs that number, not the view.
        let field = |i: usize| {
            cvars
                .get(VIEW_CVARS[view][i])
                .and_then(|s| s.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        let shipped_row = VIEW_DEFAULTS[view];
        let raw = |i: usize| {
            field(i).unwrap_or_else(|| {
                shipped_row[i]
                    .parse()
                    .expect("the reference's default strings are floats")
            })
        };
        *slot = ViewPose::from_reference(raw(0), raw(1), raw(2));
    }
    if let Some(v) = cvars
        .get(CVAR_ACTIVE_VIEW)
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|v| *v < VIEW_COUNT)
    {
        views.current = v;
    }
}

/// Everything one drained request may write.
#[derive(bevy::ecs::system::SystemParam)]
struct ViewTargets<'w, 's> {
    views: ResMut<'w, CameraViews>,
    rig: ResMut<'w, CameraControl>,
    cam: Query<'w, 's, &'static mut FlyCam, With<WorldCamera>>,
    player: Res<'w, Player>,
}

/// Drain the Lua queue into the rig, before [`super::control`] seats this frame's camera.
fn drain_view_requests(
    script: Option<NonSendMut<UiScript>>,
    mut cvars: ResMut<crate::cvars::Cvars>,
    mut targets: ViewTargets,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_camera_view_requests();
    if requests.is_empty() {
        return;
    }
    let Ok(mut cam) = targets.cam.single_mut() else {
        return; // no camera: a silent drop, as the reference's `0x4818f0` miss
    };
    let face_yaw = targets.player.facing();
    // Written to the CVars after the batch: the slots a save or reset moved, and the live index.
    let mut write_slots: Vec<usize> = Vec::new();
    let mut write_active = false;

    for request in requests {
        match request {
            CameraViewRequest::Set(n) => {
                write_active |= apply(
                    &mut targets.views,
                    &mut targets.rig,
                    &mut cam,
                    face_yaw,
                    usize::from(n),
                );
            }
            CameraViewRequest::Save(n) => {
                let view = usize::from(n);
                targets.views.save(
                    view,
                    ViewPose {
                        // The target zoom, not the collision-pulled arm (`cam+0x198`).
                        distance: targets.rig.target_distance,
                        pitch: cam.pitch,
                        yaw: wrap_pi(cam.yaw - face_yaw),
                    },
                );
                write_slots.push(view);
            }
            CameraViewRequest::Reset(n) => {
                let view = usize::from(n);
                if targets.views.reset(view) {
                    apply(
                        &mut targets.views,
                        &mut targets.rig,
                        &mut cam,
                        face_yaw,
                        view,
                    );
                }
                write_slots.push(view);
            }
            CameraViewRequest::Next | CameraViewRequest::Prev => {
                let forward = request == CameraViewRequest::Next;
                if let Some(view) = targets.views.step(forward) {
                    write_active |= apply(
                        &mut targets.views,
                        &mut targets.rig,
                        &mut cam,
                        face_yaw,
                        view,
                    );
                }
            }
            CameraViewRequest::FlipYaw(degrees) => {
                // `cam[+0x100] += arg * pi/180` (`0x50b6a0`). The reference keeps the flip in its
                // own yaw field, added outside the follow channel (`0x50f7de`), so it survives
                // re-centring; the rig has one yaw, so auto-follow reels the flip back in on Smart
                // and Always.
                cam.yaw = wrap_pi(cam.yaw + degrees.to_radians());
            }
        }
    }

    // Mirror into the registry so the change reaches `config.toml`.
    for view in write_slots {
        // A reset writes the default string verbatim, so the diff-shaped file drops the key.
        let reset_to_default = targets.views.pose(view) == ViewPose::shipped(view);
        let rendered = targets.views.pose(view).to_reference();
        for field in 0..3 {
            let value = if reset_to_default {
                VIEW_DEFAULTS[view][field]
            } else {
                rendered[field].as_str()
            };
            cvars.set(VIEW_CVARS[view][field], value);
        }
    }
    if write_active {
        cvars.set(CVAR_ACTIVE_VIEW, &targets.views.current.to_string());
    }
}

/// Seat one view; the distance glides through `target_distance`. `true` when the live index moved.
fn apply(
    views: &mut CameraViews,
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    face_yaw: f32,
    view: usize,
) -> bool {
    let (pose, moved) = views.set(view);
    rig.target_distance = pose.distance;
    cam.pitch = pose.pitch.clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    cam.yaw = wrap_pi(face_yaw + pose.yaw);
    moved
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<CameraViews>()
        // After the config load, so the persisted views are in before anything selects one.
        .add_systems(Startup, load_saved_views.after(crate::cvars::CvarLoad))
        .add_systems(
            Update,
            drain_view_requests
                // After the Lua tick that queues them, before the control pass seats the pose.
                .after(crate::ui_script::UiInput)
                .before(super::control)
                // Capture parks the camera itself; a queued view must not move it.
                .run_if(not(resource_exists::<crate::run_mode::CaptureMode>)),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_shipped_view_lands_its_recorded_pose() {
        // (Lua arg, distance yd, pitch in degrees down-positive), from `0x84f488`.
        let table = [
            (1, 0.0_f32, 0.0_f32),
            (2, 5.55, 10.0),
            (3, 5.55, 20.0),
            (4, 13.88, 30.0),
            (5, 13.88, 10.0),
        ];
        let views = CameraViews::default();
        for (lua_arg, distance, pitch_deg) in table {
            let pose = views.pose(lua_arg - 1);
            assert_eq!(pose.distance, distance, "SetView({lua_arg}) distance");
            assert!(
                (pose.pitch - pitch_from_file(pitch_deg)).abs() < 1e-6,
                "SetView({lua_arg}) pitch: {} vs {}",
                pose.pitch,
                pitch_from_file(pitch_deg)
            );
            assert_eq!(
                pose.yaw, 0.0,
                "SetView({lua_arg}) yaw is behind the character"
            );
            assert!(
                pose.pitch <= 0.0,
                "a positive saved pitch looks DOWN for us"
            );
        }
    }

    #[test]
    fn view_one_is_real_first_person_not_a_near_stop() {
        assert_eq!(CAM_DIST_MIN, 0.0);
        assert_eq!(CameraViews::default().pose(0).distance, 0.0);
    }

    #[test]
    fn the_shipped_view_is_third_person_a() {
        assert_eq!(CameraViews::default().current, 1);
        assert_eq!(CameraViews::default().pose(1).distance, 5.55);
    }

    #[test]
    fn a_saved_view_round_trips_and_reset_restores_the_default() {
        let mut views = CameraViews::default();
        let saved = ViewPose {
            distance: 12.25,
            pitch: -0.42,
            yaw: 0.75,
        };
        views.save(2, saved);
        views.save(
            3,
            ViewPose {
                distance: 1.0,
                pitch: 0.1,
                yaw: -2.0,
            },
        );
        assert_eq!(views.pose(2), saved);

        let [d, p, y] = saved.to_reference();
        let reloaded =
            ViewPose::from_reference(d.parse().unwrap(), p.parse().unwrap(), y.parse().unwrap());
        assert!((reloaded.distance - saved.distance).abs() < 1e-4);
        assert!((reloaded.pitch - saved.pitch).abs() < 1e-4);
        assert!((reloaded.yaw - saved.yaw).abs() < 1e-4);

        assert!(!views.reset(2), "view 2 was not the live one");
        assert_eq!(views.pose(2), ViewPose::shipped(2));
        views.set(3);
        assert!(views.reset(3));
    }

    /// The engine takes `1..=5` (`0x50b5d2`, `0x50b626`, `0x50b666`); the stock
    /// `SAVEVIEW`/`RESETVIEW` bindings start at 2.
    #[test]
    fn view_one_is_saveable_and_resettable() {
        let mut views = CameraViews::default();
        let pose = ViewPose {
            distance: 3.0,
            pitch: -0.2,
            yaw: 0.0,
        };
        views.save(0, pose);
        assert_eq!(views.pose(0), pose);
        views.reset(0);
        assert_eq!(views.pose(0), ViewPose::shipped(0));
        assert_eq!(views.pose(0).distance, 0.0);
    }

    #[test]
    fn next_and_prev_stop_at_the_ends() {
        let mut views = CameraViews::default();
        views.set(0);
        assert_eq!(views.step(false), None, "PrevView at view 1 is a no-op");
        for expected in 1..VIEW_COUNT {
            let next = views.step(true).expect("a view above this one");
            assert_eq!(next, expected);
            views.set(next);
        }
        assert_eq!(views.step(true), None, "NextView at view 5 is a no-op");
        for expected in (0..VIEW_COUNT - 1).rev() {
            let prev = views.step(false).expect("a view below this one");
            assert_eq!(prev, expected);
            views.set(prev);
        }
        assert_eq!(views.step(false), None);
    }

    /// 180 is the stock binding's argument (`Bindings.xml:796`).
    #[test]
    fn flipping_the_yaw_twice_returns_the_original() {
        let flip = |yaw: f32, degrees: f32| wrap_pi(yaw + degrees.to_radians());
        for start in [0.0_f32, 1.3, -2.9, std::f32::consts::PI] {
            let once = flip(start, 180.0);
            assert!(
                (wrap_pi(once - start).abs() - std::f32::consts::PI).abs() < 1e-5,
                "one flip is half a turn"
            );
            let twice = flip(once, 180.0);
            assert!(
                wrap_pi(twice - start).abs() < 1e-5,
                "two flips return: {twice} vs {start}"
            );
        }
    }

    #[test]
    fn a_hand_edited_view_cannot_land_an_illegal_pose() {
        let wild = ViewPose::from_reference(999.0, 400.0, 720.0 + 90.0);
        assert_eq!(wild.distance, CAM_DIST_MAX);
        assert_eq!(wild.pitch, -CAM_PITCH_LIMIT);
        assert!((wild.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(
            ViewPose::from_reference(-5.0, -400.0, 0.0).distance,
            CAM_DIST_MIN
        );
        assert_eq!(
            ViewPose::from_reference(-5.0, -400.0, 0.0).pitch,
            CAM_PITCH_LIMIT
        );
    }

    #[test]
    fn the_view_cvars_are_registered_with_the_references_defaults() {
        let registered = |name: &str| {
            crate::cvars::REGISTERED
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(name))
                .map(|r| r.default)
        };
        for view in 0..VIEW_COUNT {
            for field in 0..3 {
                let name = VIEW_CVARS[view][field];
                assert_eq!(
                    registered(name),
                    Some(VIEW_DEFAULTS[view][field]),
                    "{name} is not registered with its shipped default"
                );
            }
        }
        assert_eq!(registered(CVAR_ACTIVE_VIEW), Some(ACTIVE_VIEW_DEFAULT));
    }
}
