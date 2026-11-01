//! The five camera views (`SetView`, `SaveView`, `ResetView`, `NextView`, `PrevView`) and
//! `FlipCameraYaw`: the engine half of [`benilla_ui::script::CameraViewRequest`].
//!
//! The reference's rows are `FIRST_PERSON` and `THIRD_PERSON_A`..`D` (names at `0x84f848`); Lua
//! `SetView(n)` takes row `n - 1`. Pitch is in degrees down-positive, the opposite of
//! [`FlyCam::pitch`], converted only by [`super::camera_saved::pitch_from_file`]. Row 0's distance
//! `0.0` is real first person ([`CAM_DIST_MIN`] is `0.0`).
//!
//! The views persist as the reference's archived CVars (registered at `0x50b9b0`). [`CameraViews`]
//! is seeded from them at each new camera and writes every change back, so a live `SetCVar` reaches
//! the slots only at the next one, as the reference loads them only at camera construction
//! (`0x50fb80`). The live view is remembered, not applied at startup: [`super::camera_saved`]'s
//! pose lands over it, as the reference's saved pose lands over `cameraView`'s row at UI load.
//!
//! Deviation: `ResetView` writes the CVars back to their defaults, because the CVar store is our
//! persistence; the reference (`0x50fae0`) touches no CVar, so its reset reverts at next launch.
//!
//! A view is seated by `cameraViewBlendStyle` ([`set_view`]): style 1 runs the distance, pitch, yaw
//! and pivot bias channels over one shared time, style 2 writes the pose at once. While a scope is
//! held it refuses every request; only the scope's own start and end select one ([`scope_edge`]).

use bevy::prelude::*;

use benilla_ui::script::{CameraViewRequest, CAMERA_VIEW_COUNT};
use benilla_world::view::WorldCamera;

use crate::creature_anim::wrap_pi;

use super::camera::{bound_smooth_time, CameraControl, FlyCam, FollowConfig, CAM_PITCH_LIMIT};
use super::camera_channel::{Arm, Armed, SmoothChannel};
use super::camera_dynamics::CameraOptions;
use super::camera_saved::{pitch_from_file, pitch_to_file};
use super::camera_zoom::{CAM_DIST_MAX, CAM_DIST_MIN};
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

    /// The three CVar strings, in the reference's `"%f"` (`0x50f9f3`, format `0x835160`), each
    /// written through `CVar::Set` with its callback (`0x50fa09`): a negative yaw offset is refused
    /// by `0x50b3c0` and only the in-memory view keeps it.
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
    /// A scope's start or end moved the live view, and the frame's sync owes it `CVar::Set`
    /// (`0x512f17`).
    owed_active: bool,
}

impl Default for CameraViews {
    fn default() -> Self {
        Self {
            views: std::array::from_fn(ViewPose::shipped),
            current: ACTIVE_VIEW_DEFAULT
                .parse::<usize>()
                .expect("the registrar default is an index"),
            owed_active: false,
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

    /// Whether the live view is customised, `0x50fbc0` answering 0: one of its distance, pitch and
    /// yaw is 0.001 or more from the view's default string (`0x84f488`, the angles in radians by
    /// the flags `0x84f474`), or not comparable (`0x50fc07`). The reference's saved yaw is its
    /// target, which `AngleWrap` keeps in [0, 2π) (`0x5102c6`), so it is compared wrapped so.
    pub(crate) fn current_customised(&self) -> bool {
        let (live, shipped) = (self.views[self.current], ViewPose::shipped(self.current));
        [
            (live.distance, shipped.distance),
            (live.pitch, shipped.pitch),
            (live.yaw.rem_euclid(std::f32::consts::TAU), shipped.yaw),
        ]
        .into_iter()
        .any(|(a, b)| (a - b).abs() >= 0.001 || a.is_nan())
    }

    /// `NextView`/`PrevView`, with no wrap at either end (`0x50faa0`, `0x50fac0`).
    fn step(&self, forward: bool) -> Option<usize> {
        if forward {
            (self.current + 1 < VIEW_COUNT).then(|| self.current + 1)
        } else {
            self.current.checked_sub(1)
        }
    }

    /// What a new camera reads of the CVars: the fifteen slots (`0x50c3db` → `0x50fb80`) and the
    /// live index (`0x50c17a`), an absent key being the shipped default. An index whose write a
    /// scope's edge still owes is newer than its row, and stands.
    pub(super) fn reseed(&mut self, cvars: &crate::cvars::Cvars) {
        for (view, slot) in self.views.iter_mut().enumerate() {
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
        if self.owed_active {
            return;
        }
        if let Some(v) = cvars
            .get(CVAR_ACTIVE_VIEW)
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|v| *v < VIEW_COUNT)
        {
            self.current = v;
        }
    }

    /// Whether a scope's start or end owes [`CVAR_ACTIVE_VIEW`] its write.
    pub(crate) fn owes_active(&self) -> bool {
        self.owed_active
    }

    /// The owed write's value, and it is no longer owed.
    pub(crate) fn take_owed_active(&mut self) -> String {
        self.owed_active = false;
        self.current.to_string()
    }
}

/// Startup: seed the views and the live index from the CVar registry, which exists from `CvarLoad`
/// on while the VM's mirror does not yet; an absent key is the shipped default.
fn load_saved_views(cvars: Res<crate::cvars::Cvars>, mut views: ResMut<CameraViews>) {
    views.reseed(&cvars);
}

/// `0x512e90`'s style for a timed view.
const STYLE_TIMED: i32 = 1;
/// `0x512e90`'s style for an instant view.
const STYLE_INSTANT: i32 = 2;

/// What [`set_view`] reads of the CVars: the style its caller passes and a timed view's rates and
/// duration bound.
#[derive(Clone, Copy, Debug)]
struct ViewCvars {
    style: i32,
    /// `cameraDistanceSmoothSpeed`, yd/s (`0x5121be`).
    distance_speed: f32,
    /// `cameraPitchSmoothSpeed`, °/s (`0x512935`).
    pitch_speed: f32,
    /// `cameraYawSmoothSpeed`, °/s (`0x512d75`).
    yaw_speed: f32,
    /// `cameraTargetSmoothSpeed`, °/s (`0x512b55`).
    bias_speed: f32,
    /// `cameraSmoothTimeMin` and `cameraSmoothTimeMax`, seconds (`0x513255`).
    time_min: f32,
    time_max: f32,
}

impl ViewCvars {
    /// Off the knobs the rows' callback writes, each at its default in a harness without it.
    fn read(follow: Option<&FollowConfig>, options: Option<&CameraOptions>) -> Self {
        let follow = follow.copied().unwrap_or_default();
        Self {
            style: follow.view_blend_style,
            distance_speed: follow.distance_speed,
            pitch_speed: follow.pitch_speed,
            yaw_speed: follow.yaw_speed,
            bias_speed: options.copied().unwrap_or_default().target_smooth_speed,
            time_min: follow.time_min,
            time_max: follow.time_max,
        }
    }
}

/// A timed view's distance and pitch channels (`0x512120`, `0x512830`), in flight from its arm to
/// their landing. The yaw's is the follow's ([`super::camera::FollowRig`]) and the bias's is
/// [`super::camera_dynamics::SmartPivot`]'s.
pub(super) struct ViewBlend {
    distance: SmoothChannel,
    pitch: SmoothChannel,
}

impl Default for ViewBlend {
    fn default() -> Self {
        Self {
            distance: SmoothChannel::default(),
            pitch: SmoothChannel::angular(),
        }
    }
}

impl ViewBlend {
    /// One channel's arm from `live`, delay 0 and factor 1: the duration armed, `None` already
    /// there (`0x512184`).
    fn arm(channel: &mut SmoothChannel, live: f32, target: f32, rate: f32) -> Option<f32> {
        channel.snap(live);
        match channel.arm(&Arm::at(target, rate)) {
            Armed::Started => channel.duration(),
            Armed::Already | Armed::AtRest => None,
        }
    }

    /// The shared duration, on whichever of the two is armed (`0x51328d`, `0x513297`).
    fn set_duration(&mut self, duration: f32) {
        self.distance.set_duration(duration);
        self.pitch.set_duration(duration);
    }

    /// Both flights ended where they stand (`0x512f2e`-`0x512f99`).
    pub(super) fn stop(&mut self) {
        let live = self.distance.live();
        self.distance.snap(live);
        self.cancel_pitch();
    }

    /// The pitch's flight ended where it stands: freelook's entry and leave (`0x50fe90`,
    /// `0x50f978`).
    pub(super) fn cancel_pitch(&mut self) {
        let live = self.pitch.live();
        self.pitch.snap(live);
    }

    pub(super) fn distance_in_flight(&self) -> bool {
        self.distance.in_flight()
    }

    /// The distance channel's step toward `target`, the rig's (`0x50f2b8`): the live distance, or
    /// `None` with no flight.
    pub(super) fn step_distance(&mut self, target: f32, dt: f32) -> Option<f32> {
        if !self.distance.in_flight() {
            return None;
        }
        self.distance.retarget(target);
        Some(self.distance.advance(dt))
    }

    /// A keyboard axis's step under the pitch's flight, which moves the target alone (`0x51153a`,
    /// `0x5115d6`), held to ±89°; `false` with no flight.
    pub(super) fn move_pitch_target(&mut self, step: f32) -> bool {
        if !self.pitch.in_flight() {
            return false;
        }
        let (_, target) = self.pitch.probe();
        self.pitch
            .retarget((target + step).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT));
        true
    }

    /// The pitch `SaveView` stores, `[cam+0x1e0]`, given the live pitch: the live one with no
    /// flight, else the flight's target moved by what else moved the live one ([`step_pitch`]).
    pub(super) fn pitch_target(&self, live: f32) -> f32 {
        if !self.pitch.in_flight() {
            return live;
        }
        let (channel, target) = self.pitch.probe();
        (target + live - channel).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT)
    }

    /// A pitch flight, for a test of what ends it.
    #[cfg(test)]
    pub(super) fn arm_pitch_for_test(&mut self) {
        Self::arm(&mut self.pitch, 0.0, 1.0, 1.0);
    }

    #[cfg(test)]
    pub(super) fn pitch_in_flight(&self) -> bool {
        self.pitch.in_flight()
    }
}

/// The pitch channel's step in the camera's update (`0x50f576`): its motion, added to what the
/// mouse and the ground lean moved the pitch by, which shift the whole tween in the reference
/// (`0x510120`), held to ±89° (`0x50d720`).
pub(super) fn step_pitch(rig: &mut CameraControl, cam: &mut FlyCam, dt: f32) {
    let channel = &mut rig.view_blend.pitch;
    if !channel.in_flight() {
        return;
    }
    let before = channel.live();
    let after = channel.advance(dt);
    cam.pitch = (cam.pitch + after - before).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
}

/// The camera-view verbs inside their calls ([`benilla_ui::script::ScriptCall::CameraView`]):
/// `SetView` (`0x50b5b0` → `0x512e90`) seats the view and writes `cameraView` with `CVar::Set`
/// (`0x512f17`), `SaveView` (`0x50b600` → `0x50fa30` → `0x50f990`) writes the three fields
/// (`0x50fa09`), so the verb's next line reads them. Each absent in a harness without it.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ViewSteps<'w, 's> {
    views: Option<ResMut<'w, CameraViews>>,
    rig: Option<ResMut<'w, CameraControl>>,
    /// A timed view's rates and time bound, and its style ([`ViewCvars`]).
    follow: Option<Res<'w, FollowConfig>>,
    options: Option<Res<'w, CameraOptions>>,
    /// `cameraDistanceMoveSpeed`, the zoom's rate (`[0xbe0fbc]+0x24`, `0x50fc63`).
    zoom: Option<Res<'w, super::camera_zoom::ZoomLimit>>,
    cam: Query<'w, 's, &'static mut FlyCam, With<WorldCamera>>,
    player: Option<Res<'w, Player>>,
    /// Capture parks the camera itself; a view must not move it.
    capture: Option<Res<'w, crate::run_mode::CaptureMode>>,
}

impl ViewSteps<'_, '_> {
    /// `MoveView*Start`/`Stop` inside the call (`0x50b4b0` → `0x50fd40`, `0x50b4e0` → `0x50fd90`):
    /// the axis's bits on the rig, which the camera's update runs (`0x5112d0`).
    pub(crate) fn move_axis(&mut self, axis: benilla_ui::script::ViewAxis, start: bool) {
        if let Some(rig) = self.rig.as_deref_mut() {
            rig.view_axes.apply(&[(axis, start)]);
        }
    }

    /// `CameraZoomIn`/`Out(yards)` inside the call (`0x50b400` → `0x50fc60`, `0x50b450` →
    /// `0x50fcd0`): the distance axis queued for the yards' span on the rig.
    pub(crate) fn zoom(&mut self, out: bool, yards: f32) {
        let speed = self.zoom.as_deref().map_or_else(
            || super::camera_zoom::ZoomLimit::default().move_speed,
            |z| z.move_speed,
        );
        if let Some(rig) = self.rig.as_deref_mut() {
            rig.view_axes.zoom(out, yards, speed);
        }
    }

    /// Apply `request`, and the CVar writes it makes, `(name, value)` in order, for the registry
    /// to take inside the same call.
    pub(crate) fn apply(&mut self, request: CameraViewRequest) -> Vec<(&'static str, String)> {
        let mut writes = Vec::new();
        if self.capture.is_some() {
            return writes;
        }
        let cvars = ViewCvars::read(self.follow.as_deref(), self.options.as_deref());
        let (Some(views), Some(rig), Some(player)) = (
            self.views.as_deref_mut(),
            self.rig.as_deref_mut(),
            self.player.as_deref(),
        ) else {
            return writes;
        };
        let Ok(mut cam) = self.cam.single_mut() else {
            return writes; // no camera: a silent drop, as the reference's `0x4818f0` miss
        };
        let face_yaw = player.facing();
        let mut write_slot = None;
        let mut write_active = false;
        match request {
            CameraViewRequest::Set(n) => {
                write_active = set_view(views, rig, &mut cam, face_yaw, usize::from(n), &cvars);
            }
            CameraViewRequest::Save(n) => {
                let view = usize::from(n);
                views.save(
                    view,
                    ViewPose {
                        // The targets (`0x50fa33`-`0x50fa40`): the zoom, not the collision-pulled
                        // arm, and where a channel in flight is going.
                        distance: rig.target_distance,
                        pitch: rig.view_blend.pitch_target(cam.pitch),
                        yaw: wrap_pi(rig.follow.target_offset().unwrap_or(cam.yaw - face_yaw)),
                    },
                );
                write_slot = Some(view);
            }
            CameraViewRequest::Reset(n) => {
                let view = usize::from(n);
                if views.reset(view) {
                    write_active = set_view(views, rig, &mut cam, face_yaw, view, &cvars);
                }
                write_slot = Some(view);
            }
            CameraViewRequest::Next | CameraViewRequest::Prev => {
                let forward = request == CameraViewRequest::Next;
                if let Some(view) = views.step(forward) {
                    write_active = set_view(views, rig, &mut cam, face_yaw, view, &cvars);
                }
            }
            CameraViewRequest::FlipYaw(degrees) => {
                // `[cam+0x100] += arg * pi/180` (`0x50b6c9`-`0x50b6d8`), wrapped as at `0x50d779`.
                rig.flip_yaw = wrap_pi(rig.flip_yaw + degrees.to_radians());
            }
        }
        if write_active {
            writes.push((CVAR_ACTIVE_VIEW, views.current.to_string()));
        }
        if let Some(view) = write_slot {
            // A reset writes the default string verbatim, so the diff-shaped file drops the key.
            let reset_to_default = views.pose(view) == ViewPose::shipped(view);
            let rendered = views.pose(view).to_reference();
            for field in 0..3 {
                let value = if reset_to_default {
                    VIEW_DEFAULTS[view][field].to_string()
                } else {
                    rendered[field].clone()
                };
                writes.push((VIEW_CVARS[view][field], value));
            }
        }
        writes
    }
}

/// `0x512e90`: make `view` current and seat it by `cvars.style`. A request for the current view,
/// with nothing having moved the camera since the last, goes at once unless its style is 0
/// (`0x512eac`-`0x512ebf`). Every channel first stops where it stands, mode bit 6 clearing with
/// them (`0x512f22`-`0x512f99`). `true` when the live index moved, which writes `cameraView`.
fn set_view(
    views: &mut CameraViews,
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    face_yaw: f32,
    view: usize,
    cvars: &ViewCvars,
) -> bool {
    // Mode bit 3 returns ahead of every write below (`0x512ea3`).
    if rig.scoped {
        return false;
    }
    let at_once = views.current == view && !rig.view_moved && cvars.style != 0;
    let style = if at_once { STYLE_INSTANT } else { cvars.style };
    let (pose, moved) = views.set(view);
    rig.view_moved = false;
    rig.target_distance = rig.distance;
    rig.view_blend.stop();
    rig.follow.cancel();
    rig.smart_pivot.stop();
    let pitch = pose.pitch.clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    match style {
        STYLE_TIMED => {
            // One arm per channel, delay 0 and factor 1 (`0x513189`-`0x5131cc`), then every armed
            // one takes the longest duration, bounded by the time CVars (`0x5131d1`-`0x5132ab`).
            let distance = ViewBlend::arm(
                &mut rig.view_blend.distance,
                rig.distance,
                pose.distance,
                cvars.distance_speed,
            );
            if distance.is_some() {
                rig.target_distance = pose.distance;
            }
            let pitch = ViewBlend::arm(
                &mut rig.view_blend.pitch,
                cam.pitch,
                pitch,
                cvars.pitch_speed.to_radians(),
            );
            let offset = wrap_pi(cam.yaw - face_yaw);
            let yaw = rig.follow.arm_view(offset, pose.yaw, cvars.yaw_speed);
            let bias = rig.smart_pivot.arm_view(cvars.bias_speed);
            let longest = [distance, pitch, yaw, bias]
                .into_iter()
                .flatten()
                .fold(0.0, f32::max);
            let shared = bound_smooth_time(longest, cvars.time_min, cvars.time_max);
            rig.view_blend.set_duration(shared);
            rig.follow.set_duration(shared);
            rig.smart_pivot.set_duration(shared);
        }
        STYLE_INSTANT => {
            // The pose at once, live and target (`0x5130d8`-`0x513111`), with the arm on it.
            rig.distance = pose.distance;
            rig.target_distance = pose.distance;
            rig.collision_distance = pose.distance;
            cam.pitch = pitch;
            cam.yaw = wrap_pi(face_yaw + pose.yaw);
            rig.smart_pivot.home();
        }
        // Any other style seats nothing (`0x51301e`).
        _ => {}
    }
    moved
}

/// `0x50d320`, a scope's start or end on the camera: the start selects view 0 at once and then sets
/// mode bit 3 (`0x50d32d`-`0x50d33f`), the end clears the bit and then selects view 1 at once
/// (`0x50d368`-`0x50d378`). The controller that runs this holds no registry, so a moved index's
/// `cameraView` write waits for the frame's sync ([`CameraViews::take_owed_active`]).
pub(super) fn scope_edge(
    views: &mut CameraViews,
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    face_yaw: f32,
    held: bool,
) {
    let instant = ViewCvars {
        style: STYLE_INSTANT,
        ..ViewCvars::read(None, None)
    };
    let moved = if held {
        let moved = set_view(views, rig, cam, face_yaw, 0, &instant);
        rig.scoped = true;
        moved
    } else {
        rig.scoped = false;
        set_view(views, rig, cam, face_yaw, 1, &instant)
    };
    views.owed_active |= moved;
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<CameraViews>()
        // After the config load, so the persisted views are in before anything selects one.
        .add_systems(Startup, load_saved_views.after(crate::cvars::CvarLoad));
}

#[cfg(test)]
mod tests {
    use super::super::camera::{follow_cmd, seat_camera, FollowInput, FollowStyle, LookConfig};
    use super::super::camera_axes::tick;
    use super::super::camera_dynamics::{DynamicsInput, SubjectState};
    use super::super::camera_zoom::{glide_zoom, ZoomLimit};
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// `SetView` writes `cameraView` inside its call (`0x512f17`) and `SaveView` the view's three
    /// fields (`0x50fa09`), so the chunk's next line reads both: view 4's distance is the rig's
    /// target after the set, and the save stores it into view 3. Main drained the views after the
    /// chunk and read the defaults.
    #[test]
    fn a_view_verbs_cvar_writes_read_on_the_chunks_next_line() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
            bevy::ecs::error::ignore,
        ));
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        app.update();
        app.insert_non_send_resource(benilla_ui::script::UiScript::new().expect("a VM"));
        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        app.update();
        app.update();
        let cams = app
            .world_mut()
            .query_filtered::<(), (With<FlyCam>, With<WorldCamera>)>()
            .iter(app.world())
            .count();
        assert_eq!(cams, 1, "the world camera");
        let (active, distance): (String, String) =
            crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
                s.expect("a VM").eval(
                    "SetView(4) SaveView(3) return GetCVar('cameraView'), GetCVar('cameraDistanceB')",
                )
            })
            .expect("the chunk runs");
        assert_eq!(active, "3");
        assert_eq!(distance, "13.880000");
    }

    /// `MoveViewLeftStart()` sets its axis's bit on the camera inside the call (`0x50b540` →
    /// `0x50b4b0` → `0x50fd40`), so the call returns with the axis running and the camera's next
    /// update moves it (`0x5112d0`). Main left an edge for the frame's controller.
    #[test]
    fn a_move_view_call_starts_its_axis_inside_the_call() {
        use benilla_ui::script::ViewAxis;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        app.world_mut().init_resource::<CameraControl>();
        app.insert_non_send_resource(benilla_ui::script::UiScript::new().expect("a VM"));
        let left = |app: &mut App, chunk: &str| {
            crate::script_calls::with_hosted_vm(app.world_mut(), |s| s.expect("a VM").run(chunk))
                .expect("the chunk runs");
            app.world()
                .resource::<CameraControl>()
                .view_axes
                .active(ViewAxis::Left)
        };
        assert!(
            left(&mut app, "MoveViewLeftStart()"),
            "running as it returns"
        );
        assert!(
            app.world()
                .resource::<crate::bindings::BindingsState>()
                .view_calls()
                .is_empty(),
            "nothing waits for the frame"
        );
        // A zoom stops the opposite axis inside its call (`0x50fc83`): of `In` then `Out`, the
        // in axis returns stopping and the out axis running, its span queued.
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM").run("CameraZoomIn(1) CameraZoomOut(1)")
        })
        .expect("the chunk runs");
        let axes = &app.world().resource::<CameraControl>().view_axes;
        assert_eq!(
            (axes.stopping(ViewAxis::In), axes.active(ViewAxis::Out)),
            (true, true),
            "both set as the call returns"
        );
    }

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

    /// `0x50fbc0`: the live view is customised once one of its three values is 0.001 or more from
    /// its default string; another view's change does not count.
    #[test]
    fn the_live_view_is_customised_off_its_defaults() {
        let mut views = CameraViews::default();
        assert!(!views.current_customised());
        let shipped = views.pose(1);
        views.save(
            1,
            ViewPose {
                distance: shipped.distance + 0.0005,
                ..shipped
            },
        );
        assert!(!views.current_customised(), "inside the 0.001 band");
        views.save(
            1,
            ViewPose {
                yaw: 0.5,
                ..shipped
            },
        );
        assert!(views.current_customised());
        // The saved yaw wraps into [0, 2π) (`0x5102c6`): a hair left of the facing is near 2π.
        for (yaw, customised) in [(0.0005, false), (-0.0005, true)] {
            views.save(1, ViewPose { yaw, ..shipped });
            assert_eq!(views.current_customised(), customised, "yaw {yaw}");
        }
        views.set(2);
        assert!(!views.current_customised(), "only the live view counts");
        views.save(
            2,
            ViewPose {
                distance: f32::NAN,
                ..views.pose(2)
            },
        );
        assert!(
            views.current_customised(),
            "a NaN never compares (`0x50fc07`)"
        );
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

    /// The rig and camera seated on the shipped view 1, the starting view.
    fn on_view_one() -> (CameraViews, CameraControl, FlyCam) {
        let views = CameraViews::default();
        let pose = views.pose(1);
        let rig = CameraControl {
            distance: pose.distance,
            target_distance: pose.distance,
            collision_distance: pose.distance,
            ..Default::default()
        };
        let cam = FlyCam {
            yaw: 0.0,
            pitch: pose.pitch,
            speed: 0.0,
        };
        (views, rig, cam)
    }

    /// The defaults, with `style`.
    fn style(style: i32) -> ViewCvars {
        ViewCvars {
            style,
            ..ViewCvars::read(None, None)
        }
    }

    /// `secs` of the camera update's distance and pitch channels, at 60 Hz.
    fn run_channels(rig: &mut CameraControl, cam: &mut FlyCam, secs: f32) {
        const DT: f32 = 1.0 / 60.0;
        for _ in 0..(secs / DT).round() as usize {
            glide_zoom(DT, rig, CAM_DIST_MAX, 8.33);
            step_pitch(rig, cam, DT);
        }
    }

    /// Style 1 arms each channel and gives them the longest duration (`0x513189`-`0x5132ab`): from
    /// view 1 to view 3, 8.33 yd at 8.33 yd/s is 1 s against 20° at 45°/s, so the pitch shares the
    /// second, and both pass the midpoint together on the cosine (`0x5b7bb0`).
    #[test]
    fn a_timed_view_moves_its_channels_over_one_shared_time() {
        let (mut views, mut rig, mut cam) = on_view_one();
        let (from, to) = (views.pose(1), views.pose(3));
        let timed = style(STYLE_TIMED);
        assert!(set_view(&mut views, &mut rig, &mut cam, 0.0, 3, &timed));
        assert_eq!(rig.distance, from.distance, "nothing moves at the arm");
        assert_eq!(cam.pitch, from.pitch);
        assert_eq!(rig.target_distance, to.distance, "the target does");
        assert_eq!(
            rig.view_blend.distance.duration(),
            rig.view_blend.pitch.duration(),
            "one duration"
        );
        run_channels(&mut rig, &mut cam, 0.5);
        let midpoint = |a: f32, b: f32| (a + b) * 0.5;
        assert!(
            (rig.distance - midpoint(from.distance, to.distance)).abs() < 0.02,
            "{}",
            rig.distance
        );
        assert!(
            (cam.pitch - midpoint(from.pitch, to.pitch)).abs() < 1e-3,
            "{}",
            cam.pitch
        );
        run_channels(&mut rig, &mut cam, 0.6);
        assert_eq!(rig.distance, to.distance, "it lands");
        assert!((cam.pitch - to.pitch).abs() < 1e-4, "{}", cam.pitch);
        assert!(!rig.view_blend.distance_in_flight());
        assert!(!rig.view_blend.pitch_in_flight());
    }

    /// The yaw runs on the follow's channel, to the view's saved offset (`0x5131b8`).
    #[test]
    fn a_timed_view_arms_the_yaw_to_its_saved_offset() {
        let (mut views, mut rig, mut cam) = on_view_one();
        let to = views.pose(3);
        views.save(3, ViewPose { yaw: 1.0, ..to });
        set_view(&mut views, &mut rig, &mut cam, 0.0, 3, &style(STYLE_TIMED));
        assert_eq!(rig.follow.target_offset(), Some(1.0));
        assert_eq!(cam.yaw, 0.0, "nothing moves at the arm");
    }

    /// The shared time takes the time CVars' bound (`0x513255`-`0x513286`): 5.55 to 50 yd at
    /// 8.33 yd/s is 5.3 s.
    #[test]
    fn the_shared_time_is_bound_by_the_time_cvars() {
        let duration = |time_min, time_max| {
            let (mut views, mut rig, mut cam) = on_view_one();
            let far = ViewPose {
                distance: CAM_DIST_MAX,
                ..views.pose(3)
            };
            views.save(3, far);
            let cvars = ViewCvars {
                time_min,
                time_max,
                ..style(STYLE_TIMED)
            };
            set_view(&mut views, &mut rig, &mut cam, 0.0, 3, &cvars);
            rig.view_blend.distance.duration()
        };
        assert_eq!(duration(0.1, 2.0), Some(2.0));
        assert_eq!(duration(9.0, 30.0), Some(9.0));
        assert_eq!(duration(5.0, 1.0), Some(1.0), "an inverted pair");
    }

    /// Style 2 writes the pose, live and target, at once (`0x5130d8`-`0x513111`).
    #[test]
    fn style_two_writes_the_pose_at_once() {
        let (mut views, mut rig, mut cam) = on_view_one();
        let to = views.pose(3);
        let instant = style(STYLE_INSTANT);
        assert!(set_view(&mut views, &mut rig, &mut cam, 0.0, 3, &instant));
        assert_eq!(
            (rig.distance, rig.target_distance, rig.collision_distance),
            (to.distance, to.distance, to.distance)
        );
        assert_eq!(cam.pitch, to.pitch);
        assert!(!rig.view_blend.distance_in_flight());
        assert!(!rig.view_blend.pitch_in_flight());
    }

    /// A request for the current view goes at once while mode bit 6 is clear
    /// (`0x512eac`-`0x512ebf`), and is timed once the mouse or an axis has set it; the request
    /// clears the bit (`0x512f60`). The index did not move, so `cameraView` is not written
    /// (`0x512eef`).
    #[test]
    fn the_current_view_goes_at_once_until_the_camera_has_moved() {
        let (mut views, mut rig, mut cam) = on_view_one();
        let home = views.pose(1).distance;
        let timed = style(STYLE_TIMED);
        rig.distance = 9.0;
        rig.target_distance = 9.0;
        assert!(!set_view(&mut views, &mut rig, &mut cam, 0.0, 1, &timed));
        assert_eq!(rig.distance, home, "at once");
        rig.distance = 9.0;
        rig.target_distance = 9.0;
        rig.view_moved = true;
        assert!(!set_view(&mut views, &mut rig, &mut cam, 0.0, 1, &timed));
        assert_eq!(rig.distance, 9.0, "timed");
        assert!(rig.view_blend.distance_in_flight());
        assert!(!rig.view_moved);
    }

    /// Any other style makes the view current and seats nothing, each target on its live value
    /// (`0x512f2e`, `0x51301e`), and style 0 stays itself for the current view (`0x512ebd`).
    #[test]
    fn a_style_of_neither_seats_nothing() {
        let (mut views, mut rig, mut cam) = on_view_one();
        let (distance, pitch) = (rig.distance, cam.pitch);
        let (zero, other) = (style(0), style(3));
        rig.target_distance = 20.0;
        assert!(set_view(&mut views, &mut rig, &mut cam, 0.0, 3, &other));
        assert_eq!(views.current, 3);
        assert_eq!((rig.distance, rig.target_distance), (distance, distance));
        assert_eq!(cam.pitch, pitch);
        assert!(!set_view(&mut views, &mut rig, &mut cam, 0.0, 3, &zero));
        assert_eq!(rig.distance, distance);
        // Style 3 for the current view is not 0, so it goes at once.
        assert!(!set_view(&mut views, &mut rig, &mut cam, 0.0, 3, &other));
        assert_eq!(rig.distance, views.pose(3).distance);
    }

    /// While a channel runs an axis moves its target alone (`0x51140c`, `0x51152f`, `0x5115d6`),
    /// and the channel lands on where it was moved. A start tick runs nothing, so of three ticks
    /// two move: 1.666 yd and 4.5°.
    #[test]
    fn an_axis_under_a_timed_view_moves_the_target_alone() {
        use benilla_ui::script::ViewAxis;
        let (mut views, mut rig, mut cam) = on_view_one();
        let to = views.pose(3);
        views.save(3, ViewPose { yaw: 1.0, ..to });
        set_view(&mut views, &mut rig, &mut cam, 0.0, 3, &style(STYLE_TIMED));
        let live = (rig.distance, cam.pitch, cam.yaw);
        rig.view_axes.apply(&[
            (ViewAxis::Out, true),
            (ViewAxis::Down, true),
            (ViewAxis::Right, true),
        ]);
        for _ in 0..3 {
            tick(
                &mut rig,
                &mut cam,
                0.1,
                LookConfig::default(),
                8.33,
                CAM_DIST_MAX,
            );
        }
        assert_eq!((rig.distance, cam.pitch, cam.yaw), live);
        assert!((rig.target_distance - (to.distance + 1.666)).abs() < 1e-3);
        assert!(rig.follow.target_offset().is_some_and(|t| t > 1.0));
        run_channels(&mut rig, &mut cam, 1.1);
        assert_eq!(rig.distance, rig.target_distance);
        assert!(
            (cam.pitch - (to.pitch + 4.5f32.to_radians())).abs() < 1e-3,
            "{}",
            cam.pitch
        );
    }

    /// The mouse's and the ground lean's steps shift the whole tween (`0x510120`): one made under
    /// the flight stands when it lands.
    #[test]
    fn a_pitch_step_under_a_timed_view_stands_when_it_lands() {
        let (mut views, mut rig, mut cam) = on_view_one();
        let to = views.pose(3);
        set_view(&mut views, &mut rig, &mut cam, 0.0, 3, &style(STYLE_TIMED));
        run_channels(&mut rig, &mut cam, 0.25);
        cam.pitch += 0.1;
        assert!((rig.view_blend.pitch_target(cam.pitch) - (to.pitch + 0.1)).abs() < 1e-4);
        run_channels(&mut rig, &mut cam, 0.9);
        assert!((cam.pitch - (to.pitch + 0.1)).abs() < 1e-4, "{}", cam.pitch);
    }

    /// `SaveView` stores the targets, not the live values (`0x50fa33`-`0x50fa40`).
    #[test]
    fn a_view_saved_in_flight_stores_where_it_is_going() {
        let mut world = verb_world();
        let pose = ViewPose {
            distance: 13.88,
            pitch: -0.5,
            yaw: 1.0,
        };
        world.resource_mut::<CameraViews>().save(3, pose);
        verb(&mut world, CameraViewRequest::Set(3));
        verb(&mut world, CameraViewRequest::Save(2));
        let saved = world.resource::<CameraViews>().pose(2);
        assert_eq!(saved.distance, pose.distance);
        assert!((saved.pitch - pose.pitch).abs() < 1e-5, "{}", saved.pitch);
        assert!((saved.yaw - pose.yaw).abs() < 1e-5, "{}", saved.yaw);
    }

    /// The verbs read the style off the row: 2 seats the view inside the call.
    #[test]
    fn a_view_verb_reads_the_blend_style() {
        let mut world = verb_world();
        world.insert_resource(FollowConfig {
            view_blend_style: STYLE_INSTANT,
            ..FollowConfig::default()
        });
        verb(&mut world, CameraViewRequest::Set(3));
        let want = world.resource::<CameraViews>().pose(3).distance;
        assert_eq!(world.resource::<CameraControl>().distance, want);
    }

    /// A write to each row reaches the knob a view reads, through the rows' callback.
    #[test]
    fn the_view_cvars_reach_their_knobs() {
        let mut world = World::new();
        world.init_resource::<crate::cvars::Cvars>();
        world.init_resource::<LookConfig>();
        world.init_resource::<ZoomLimit>();
        world.init_resource::<FollowConfig>();
        world
            .run_system_once(|mut steps: crate::cvars::CvarSteps| {
                let cvars = steps.cvars().expect("the registry");
                cvars.set("cameraViewBlendStyle", "2");
                cvars.set("cameraSmoothTimeMin", "0.25");
                cvars.set("cameraSmoothTimeMax", "1.5");
                cvars.set("cameraDistanceSmoothSpeed", "4.0");
                cvars.set("cameraPitchSmoothSpeed", "30.0");
                steps.sync(None);
            })
            .expect("the sync runs");
        let knobs = *world.resource::<FollowConfig>();
        assert_eq!(knobs.view_blend_style, 2);
        assert_eq!((knobs.time_min, knobs.time_max), (0.25, 1.5));
        assert_eq!((knobs.distance_speed, knobs.pitch_speed), (4.0, 30.0));
    }

    /// A world with what the view verbs read: the camera, the rig and a body facing 0.
    fn verb_world() -> World {
        let mut world = World::new();
        world.init_resource::<CameraViews>();
        world.init_resource::<CameraControl>();
        world.insert_resource(Player::default());
        world.spawn((
            FlyCam {
                yaw: 0.0,
                pitch: 0.0,
                speed: 0.0,
            },
            WorldCamera,
        ));
        world
    }

    /// One view verb as the call host applies it, and the CVar writes it made.
    fn verb(world: &mut World, request: CameraViewRequest) -> Vec<(&'static str, String)> {
        world
            .run_system_once(move |mut steps: ViewSteps| steps.apply(request))
            .expect("the verb runs")
    }

    /// `FlipCameraYaw` adds its radians to `[cam+0x100]` (`0x50b6c9`-`0x50b6d8`) and to no other
    /// word: the offset stands, and a view leaves the term, as `0x512e90` never writes it. 180 is
    /// the stock binding's argument (`Bindings.xml:796`).
    #[test]
    fn a_flip_is_a_term_of_its_own() {
        let mut world = verb_world();
        let flip = |world: &World| world.resource::<CameraControl>().flip_yaw;
        assert!(verb(&mut world, CameraViewRequest::FlipYaw(180.0)).is_empty());
        assert!(
            (flip(&world).abs() - std::f32::consts::PI).abs() < 1e-5,
            "one flip is half a turn"
        );
        let yaw = world.query::<&FlyCam>().single(&world).unwrap().yaw;
        assert_eq!(yaw, 0.0, "the offset stands");
        verb(&mut world, CameraViewRequest::FlipYaw(180.0));
        assert!(flip(&world).abs() < 1e-5, "two flips return");
        verb(&mut world, CameraViewRequest::FlipYaw(90.0));
        verb(&mut world, CameraViewRequest::Set(3));
        assert!(
            (flip(&world) - std::f32::consts::FRAC_PI_2).abs() < 1e-5,
            "a view leaves it"
        );
    }

    /// The whole client's world, for [`seat`], with the view verbs' state and no other camera.
    fn seat_app() -> App {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let world = app.world_mut();
        world.init_resource::<CameraControl>();
        world.insert_resource(Player::default());
        world.spawn((
            Transform::default(),
            FlyCam {
                yaw: 0.0,
                pitch: 0.0,
                speed: 0.0,
            },
            WorldCamera,
        ));
        app
    }

    /// One [`seat_camera`] of the camera on a body at the origin facing 0, in an empty world, with
    /// the Always follow style and `word` as the input word: the transform it wrote.
    fn seat(world: &mut World, word: u32, dt: f32, scoped: bool) -> Transform {
        world
            .run_system_once(
                move |collide: benilla_world::collision::WorldCollision,
                      mut rig: ResMut<CameraControl>,
                      mut q: Query<(&mut Transform, &mut FlyCam)>| {
                    let (mut t, mut cam) = q.single_mut().expect("the camera");
                    let follow = FollowInput {
                        cfg: FollowConfig {
                            style: FollowStyle::Always,
                            tracking_style: FollowStyle::Always,
                            ..FollowConfig::default()
                        },
                        face_yaw: 0.0,
                        command: word,
                    };
                    let dynamics = DynamicsInput {
                        options: CameraOptions::default(),
                        smooth_style: FollowStyle::Always,
                        tracking_style: FollowStyle::Always,
                        subject: SubjectState {
                            scoped,
                            ..Default::default()
                        },
                        nearclip: 0.2,
                        surface_y: None,
                    };
                    seat_camera(
                        dt,
                        0.0,
                        Vec3::ZERO,
                        Vec3::Y,
                        1.8,
                        &mut rig,
                        &mut cam,
                        &mut t,
                        &collide,
                        &follow,
                        &dynamics,
                    );
                    *t
                },
            )
            .expect("the seat runs")
    }

    /// The view's yaw is the offset, the flip and the facing (`0x50f7de`-`0x50f7f2`), and a return
    /// arms the offset alone (`0x512c70`): the walk brings the offset home and the view stays
    /// flipped, looking back at the character.
    #[test]
    fn a_follow_return_keeps_the_flip_in_the_view() {
        const DT: f32 = 1.0 / 60.0;
        let mut app = seat_app();
        let world = app.world_mut();
        // An offset a drag left, then the stock binding's flip.
        world.query::<&mut FlyCam>().single_mut(world).unwrap().yaw = 1.0;
        verb(world, CameraViewRequest::FlipYaw(180.0));
        let mut seated = seat(world, 0, DT, false);
        for _ in 0..60 {
            seated = seat(world, follow_cmd::FORWARD, DT, false);
        }
        let yaw = world.query::<&FlyCam>().single(world).unwrap().yaw;
        assert!(yaw.abs() < 1e-3, "the offset came home: {yaw}");
        let forward = seated.rotation * Vec3::NEG_Z;
        assert!(
            (forward - Vec3::Z).length() < 1e-3,
            "the view still looks back: {forward}"
        );
    }

    /// A new camera reads the slots and the live index off their CVars (`0x50fb80`, `0x50c17a`),
    /// except an index whose write a scope's edge still owes.
    #[test]
    fn a_new_camera_reads_the_views_off_the_cvars() {
        let mut cvars = crate::cvars::Cvars::default();
        cvars.set("cameraDistanceA", "20");
        cvars.set(CVAR_ACTIVE_VIEW, "3");
        let mut views = CameraViews::default();
        views.reseed(&cvars);
        assert_eq!(views.pose(1).distance, 20.0);
        assert_eq!(views.current, 3);

        cvars.set(CVAR_ACTIVE_VIEW, "2");
        views.current = 0;
        views.owed_active = true;
        views.reseed(&cvars);
        assert_eq!(views.current, 0);
        assert_eq!(views.take_owed_active(), "0");
    }

    /// Mode bit 3 returns ahead of every write of `0x512e90` (`0x512ea3`), whatever the style.
    #[test]
    fn a_held_scope_refuses_a_view_before_it_writes() {
        for word in [STYLE_TIMED, STYLE_INSTANT] {
            let (mut views, mut rig, mut cam) = on_view_one();
            rig.scoped = true;
            rig.view_moved = true;
            let pitch = cam.pitch;
            let moved = set_view(&mut views, &mut rig, &mut cam, 0.5, 3, &style(word));
            assert!(!moved, "no index moved");
            assert_eq!(views.current, 1);
            assert_eq!(rig.distance, views.pose(1).distance);
            assert!(!rig.view_blend.distance_in_flight());
            assert!(rig.view_moved, "bit 6 clears below the return");
            assert_eq!((cam.yaw, cam.pitch), (0.0, pitch));
        }
    }

    /// The verbs that select a view reach that return, so `cameraView` stands. `SaveView` writes
    /// its slot with no view (`0x50fa09`), a reset of the live view its slot alone, and
    /// `FlipCameraYaw` never reaches `0x512e90`.
    #[test]
    fn a_held_scope_refuses_only_the_verbs_that_select_a_view() {
        let mut world = verb_world();
        world.resource_mut::<CameraControl>().scoped = true;
        for request in [
            CameraViewRequest::Set(3),
            CameraViewRequest::Next,
            CameraViewRequest::Prev,
        ] {
            assert!(verb(&mut world, request).is_empty());
            assert_eq!(world.resource::<CameraViews>().current, 1);
        }
        assert_eq!(verb(&mut world, CameraViewRequest::Save(2)).len(), 3);
        assert_eq!(verb(&mut world, CameraViewRequest::Reset(1)).len(), 3);
        assert_eq!(world.resource::<CameraControl>().distance, 0.0);
        verb(&mut world, CameraViewRequest::FlipYaw(90.0));
        assert_ne!(world.resource::<CameraControl>().flip_yaw, 0.0);
    }

    /// A scope's start selects view 0 at once and then sets the bit (`0x50d32d`-`0x50d33f`); its
    /// `cameraView` write is gated on the index moving (`0x512ee6`).
    #[test]
    fn a_scope_start_selects_view_zero_at_once() {
        let (mut views, mut rig, mut cam) = on_view_one();
        scope_edge(&mut views, &mut rig, &mut cam, 0.5, true);
        let pose = views.pose(0);
        assert_eq!(views.current, 0);
        assert!(rig.scoped);
        assert_eq!((rig.distance, rig.target_distance), (0.0, 0.0));
        assert_eq!(cam.pitch, pose.pitch);
        assert_eq!(cam.yaw, wrap_pi(0.5 + pose.yaw));
        assert!(views.owes_active());

        let (mut views, mut rig, mut cam) = on_view_one();
        views.set(0);
        scope_edge(&mut views, &mut rig, &mut cam, 0.5, true);
        assert!(!views.owes_active(), "already on view 0");
    }

    /// A scope's end clears the bit and then selects view 1 at once (`0x50d368`-`0x50d378`), over
    /// what the zoom did in between, which nothing gates (`0x50edb7` only holds the eye).
    #[test]
    fn a_scope_end_selects_view_one_at_once() {
        let (mut views, mut rig, mut cam) = on_view_one();
        scope_edge(&mut views, &mut rig, &mut cam, 0.5, true);
        assert_eq!(views.take_owed_active(), "0");
        rig.distance = 20.0;
        rig.target_distance = 20.0;
        scope_edge(&mut views, &mut rig, &mut cam, 0.5, false);
        let pose = views.pose(1);
        assert_eq!(views.current, 1);
        assert!(!rig.scoped);
        assert_eq!(rig.distance, pose.distance);
        assert_eq!(rig.target_distance, pose.distance);
        assert_eq!(rig.collision_distance, pose.distance);
        assert!(!rig.view_blend.distance_in_flight());
        assert_eq!(cam.pitch, pose.pitch);
        assert_eq!(cam.yaw, wrap_pi(0.5 + pose.yaw));
        assert_eq!(views.take_owed_active(), "1");
    }

    /// While the bit is set the eye takes distance 0 and the zoom stands (`0x50edb7`-`0x50edc0`).
    #[test]
    fn a_held_scope_seats_the_eye_on_the_pivot() {
        let mut app = seat_app();
        let world = app.world_mut();
        {
            let mut rig = world.resource_mut::<CameraControl>();
            rig.distance = 5.55;
            rig.target_distance = 5.55;
            rig.collision_distance = 5.55;
        }
        let eye = seat(world, 0, 1.0 / 60.0, true).translation;
        assert!((eye - Vec3::new(0.0, 1.8, 0.0)).length() < 1e-4, "{eye}");
        assert_eq!(world.resource::<CameraControl>().distance, 5.55);
    }

    /// The frame's sync makes the `CVar::Set` a scope's edge owes (`0x512f17`), once.
    #[test]
    fn the_sync_writes_the_view_a_scope_selected() {
        let mut world = World::new();
        world.init_resource::<crate::cvars::Cvars>();
        world.init_resource::<LookConfig>();
        world.init_resource::<ZoomLimit>();
        world.init_resource::<FollowConfig>();
        let (mut views, mut rig, mut cam) = on_view_one();
        scope_edge(&mut views, &mut rig, &mut cam, 0.0, true);
        world.insert_resource(views);
        world
            .run_system_once(|mut steps: crate::cvars::CvarSteps| steps.sync(None))
            .expect("the sync runs");
        let cvars = world.resource::<crate::cvars::Cvars>();
        assert_eq!(cvars.get(CVAR_ACTIVE_VIEW), Some("0"));
        assert!(!world.resource::<CameraViews>().owes_active());
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
