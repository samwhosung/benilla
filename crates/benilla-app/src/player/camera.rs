//! The third-person camera rig: the two mouse-look modes, the wheel-zoom glide, the
//! collision-swept boom on the framing pivot, and the self-avatar fade into first person.

use bevy::ecs::entity::EntityHashSet;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use super::camera_channel::{Arm, SmoothChannel};
use super::camera_dynamics::{DynamicsInput, HeadBob, SmartPivot, TerrainTilt};
use crate::creature_anim::wrap_pi;
use crate::net::Embodied;
use benilla_assets::materials::WowModelMaterial;
use benilla_world::interact::{WorldClick, WorldRightClick, WorldRightPress};
use benilla_world::model_fade::{
    self_model_fade_alpha, FadeMaterials, PendingAppearFade, RenderFade, SELF_FADE_WINDOW,
};

/// The reference's up-edge click predicate (`0x514ae0`): a click is a release within 200 ms, or
/// within 800 ms under 2.25° of yaw and 2.0° of pitch travel (ms from `GetTickCount`, `[0x7ff310]`,
/// via `0x42c010` → `0x42b790`). The travel gates only the click; the orbit engages on the down
/// edge (`0x51491f`). Travel sums absolute motion per axis (`0x514400`: `fabs; fadd; fstp`). The
/// literals are 8.0 event-units against `Σ|0.8·Δx|`, `Σ|0.6·Δy|` of mouse deltas of untraced device
/// scale; we hold the angles they mean at the default speeds (`180·10/800`, `90·13.333/600`).
const CLICK_HOLD_CEILING: f32 = 0.800;
const CLICK_FREE_WINDOW: f32 = 0.200;
const CLICK_YAW_TRAVEL: f32 = 2.25 * std::f32::consts::PI / 180.0;
const CLICK_PITCH_TRAVEL: f32 = 2.0 * std::f32::consts::PI / 180.0;

/// An undecided primary press: the reference's world-input state (`[0xbe1148]`), press time at
/// `+0x14`, accumulators zeroed at `0x514910`/`0x514913`; ours are radians of camera rotation.
#[derive(Clone, Copy)]
pub(super) struct PressGesture {
    /// Seconds on the app clock when the button went down.
    at: f32,
    /// Accumulated |Δyaw| and |Δpitch| this press has asked the camera for, radians.
    yaw_travel: f32,
    pitch_travel: f32,
}

impl PressGesture {
    fn new(now: f32) -> Self {
        Self {
            at: now,
            yaw_travel: 0.0,
            pitch_travel: 0.0,
        }
    }

    fn is_click(&self, now: f32) -> bool {
        let elapsed = now - self.at;
        elapsed < CLICK_FREE_WINDOW
            || (elapsed < CLICK_HOLD_CEILING
                && self.yaw_travel < CLICK_YAW_TRAVEL
                && self.pitch_travel < CLICK_PITCH_TRAVEL)
    }
}

/// The zoom floor, yd: at 0 the eye sits at the framing pivot, inside the head, and the avatar
/// fades out. The reference clamps the orbit to `cameraDistanceMax × cameraDistanceMaxFactor`,
/// capped at 50 (`0x5112d0`). Deviation: the starting zoom, [`CAM_DIST_DEFAULT`], is 15 yd where
/// the reference's `cameraDistance` is 5.55 (`0x84f488`), for a wider view.
pub(super) const CAM_DIST_MIN: f32 = 0.0;
/// The reference's `cameraDistanceMax` (default 15); 1.12's panel offers only the factor.
pub(super) const CAM_DIST_BASE_MAX: f32 = 15.0;
/// `cameraDistanceMaxFactor`'s slider, MAX_FOLLOW_DIST (1 to 2 by 0.1, `UIOptionsFrame.lua:90`).
pub(crate) const CAM_DIST_FACTOR_RANGE: std::ops::RangeInclusive<f32> = 1.0..=2.0;
/// The factor slider's top: the clamp for a distance read back off disk ([`ZoomLimit`] is live).
pub(super) const CAM_DIST_MAX: f32 = CAM_DIST_BASE_MAX * 2.0;
pub(super) const CAM_DIST_DEFAULT: f32 = 15.0;

/// The max orbit distance: 1.12's `cameraDistanceMaxFactor` over [`CAM_DIST_BASE_MAX`], 15 yd at
/// the reference's defaults (`cameraDistanceMax` "15.0" at `0x84fbd0`, `cameraDistanceMaxFactor`
/// "1.0" at `0x82e92c`). Lowering the factor pulls the live target in on the next frame.
#[derive(Resource)]
pub(crate) struct ZoomLimit {
    pub(crate) max: f32,
}

impl Default for ZoomLimit {
    fn default() -> Self {
        // Factor 1.0, the reference's default; `CAM_DIST_MAX` is the slider's top.
        Self {
            max: CAM_DIST_BASE_MAX,
        }
    }
}

impl ZoomLimit {
    pub(crate) fn set_factor(&mut self, factor: f32) {
        let f = factor.clamp(*CAM_DIST_FACTOR_RANGE.start(), *CAM_DIST_FACTOR_RANGE.end());
        self.max = CAM_DIST_BASE_MAX * f;
    }

    /// The live factor, the inverse of [`Self::set_factor`], for tests.
    #[cfg(test)]
    pub(crate) fn factor(&self) -> f32 {
        self.max / CAM_DIST_BASE_MAX
    }
}
/// Yards per wheel notch, the stock bindings' `CameraZoomIn(1.0)` (`Bindings.xml:707`).
const CAM_ZOOM_STEP: f32 = 1.0;
/// Zoom speed in yd/s, `cameraDistanceMoveSpeed`'s default: the reference glides the distance to
/// the wheel target at this constant speed (`0x5112d0`), not an ease.
const CAM_MOVE_SPEED: f32 = 8.33;
/// Radians of camera rotation per raw mouse unit at the default move speeds and `mousespeed` 1.0.
const LOOK_SENSITIVITY: f32 = 0.003;
/// `mousespeed`'s slider, MOUSE_SENSITIVITY (0.5 to 1.5 by 0.05, `UIOptionsFrame.lua:87`).
pub(crate) const MOUSE_SPEED_RANGE: std::ops::RangeInclusive<f32> = 0.5..=1.5;

/// The camera rows' change callback: the look, zoom and follow knobs.
pub(crate) fn on_cvar(
    ev: On<crate::cvars::CvarChanged>,
    mut look: ResMut<LookConfig>,
    mut zoom: ResMut<ZoomLimit>,
    mut follow: ResMut<FollowConfig>,
) {
    let v = ev.num();
    match ev.key().as_str() {
        "mouseinvertpitch" => look.invert_pitch = v != 0.0,
        "mousespeed" => {
            look.sensitivity = v.clamp(*MOUSE_SPEED_RANGE.start(), *MOUSE_SPEED_RANGE.end());
        }
        "camerayawmovespeed" | "camerapitchmovespeed" => {
            if !CAMERA_SPEED_RANGE.contains(&v) {
                warn!(
                    "cvar {}: value out of range ({} - {}) — ignored",
                    ev.name,
                    CAMERA_SPEED_RANGE.start(),
                    CAMERA_SPEED_RANGE.end()
                );
                return;
            }
            if ev.is("cameraYawMoveSpeed") {
                look.yaw_speed = v;
            } else {
                look.pitch_speed = v;
            }
        }
        "cameradistancemaxfactor" => zoom.set_factor(v),
        // The stock dropdown writes 1 Smart, 2 Always, 0 Never (`UIOptionsFrame.lua:518`).
        "camerasmoothstyle" => follow.style = FollowStyle::from_cvar(v),
        "camerasmoothtrackingstyle" => follow.tracking_style = FollowStyle::from_cvar(v),
        "camerayawsmoothspeed" => {
            follow.yaw_speed = v.clamp(*FOLLOW_SPEED_RANGE.start(), *FOLLOW_SPEED_RANGE.end());
        }
        _ => {}
    }
}

/// The mouse-look rate law. The reference's (`0x50fee0`) is `Δyaw° = cameraYawMoveSpeed × Δx / 800`
/// and `Δpitch° = cameraPitchMoveSpeed × Δy / 600` in screen pixels after Windows acceleration
/// (`WM_MOUSEMOVE`, `0x42d31c`; its `mousespeed` sets the OS pointer speed); ours are raw device
/// units. Deviation: the shape is kept, its scale anchored to [`LOOK_SENSITIVITY`] on both axes at
/// the default speeds, because the unit factor is a per-machine OS setting and this keeps the
/// shipped feel: both axes turn alike, where the reference's yaw turns 1.5× faster than its pitch.
const LOOK_YAW_PER_SPEED: f32 = LOOK_SENSITIVITY / 180.0;
const LOOK_PITCH_PER_SPEED: f32 = LOOK_SENSITIVITY / 90.0;

/// The reference's validator range for the `camera*MoveSpeed`/`SmoothSpeed` CVars (`0x50c000` →
/// `0x50b330`): an out-of-range value prints "Value out of range" and the old one stands.
pub(crate) const CAMERA_SPEED_RANGE: std::ops::RangeInclusive<f32> = 0.1..=360.0;

/// The mouse-look knobs. `invert_pitch` is 1.12's `mouseInvertPitch` checkbox
/// (`UIOptionsFrame.lua:4`); `sensitivity` is `mousespeed`, scaling both the rotation and the click
/// travel budget, as the reference's OS pointer speed scales the delta upstream of both.
#[derive(Resource, Clone, Copy)]
pub(crate) struct LookConfig {
    pub(crate) invert_pitch: bool,
    pub(crate) sensitivity: f32,
    /// `cameraYawMoveSpeed`, the MOUSE_LOOK_SPEED slider (90 to 270 by 10).
    pub(crate) yaw_speed: f32,
    /// `cameraPitchMoveSpeed`: no slider; `UIOptionsFrame_Save` writes it as half the yaw speed
    /// (`UIOptionsFrame.lua:356`).
    pub(crate) pitch_speed: f32,
}

impl Default for LookConfig {
    fn default() -> Self {
        Self {
            invert_pitch: false,
            sensitivity: 1.0,
            // The reference's defaults; at these both axes land exactly on `LOOK_SENSITIVITY`.
            yaw_speed: 180.0,
            pitch_speed: 90.0,
        }
    }
}

impl LookConfig {
    /// Radians of yaw per raw mouse unit, read by both the rotation and the click travel budget.
    pub(super) fn yaw_rate(self) -> f32 {
        self.yaw_speed * LOOK_YAW_PER_SPEED * self.sensitivity
    }

    /// Radians of pitch per raw mouse unit: its own CVar and the law's divisor 600.
    pub(super) fn pitch_rate(self) -> f32 {
        self.pitch_speed * LOOK_PITCH_PER_SPEED * self.sensitivity
    }
}
/// `cameraYawSmoothSpeed`, default 180°/s (`[0xbe1070]`, read at `0x512d75`): a duration divisor,
/// as a return lasts `|Δyaw| / rate × factor`. The AUTO_FOLLOW_SPEED slider, greyed under Never.
pub(crate) const FOLLOW_SPEED_DEFAULT: f32 = 180.0;
/// The 1.12 slider's own range for [`FollowConfig::yaw_speed`].
pub(crate) const FOLLOW_SPEED_RANGE: std::ops::RangeInclusive<f32> = 90.0..=270.0;
/// `cameraSmoothTimeMin`/`Max`: 0.1 s and 2.0 s (`[0xbe105c]`/`[0xbe1038]`, clamped at `0x510f4d`).
const FOLLOW_TIME_MIN: f32 = 0.1;
const FOLLOW_TIME_MAX: f32 = 2.0;
/// The reference's 0.001 already-there and same-arm epsilon (`[0x801360]`, `0x512ce4`/`0x512d41`).
const FOLLOW_EPS: f32 = 1.0e-3;

/// Camera Following Style, 1.12's `cameraSmoothStyle`: whether the camera returns behind the
/// character on its own. The engine's enum is 0 Never, 1 Smart, 2 Always (`0x50b6f0` registers
/// them in that order and the consumers index by `style × stride`), the values the stock dropdown
/// writes (`UIOptionsFrame.lua:518`). The validator also accepts 3 (`0x50b330(v, 0, 3)`), which
/// arms nothing (`0x510a89`) and reads past the terrain-tilt table (`0x50dbc0`); ours reads Never.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum FollowStyle {
    /// The offset stays put. The keyboard-turn carry is a separate mechanism (the reference's
    /// camera yaw is relative to the unit's facing) and keeps running.
    Never,
    /// Stays where placed except while the character is being driven; the reference's default.
    #[default]
    Smart,
    /// Every input edge arms a return, standing still included.
    Always,
}

impl FollowStyle {
    /// From the CVar: 3 reads as Never (see the type doc), anything else off the ladder as Smart.
    pub(crate) fn from_cvar(v: f32) -> Self {
        match v as i32 {
            0 => Self::Never,
            2 => Self::Always,
            3 => Self::Never,
            _ => Self::Smart,
        }
    }

    /// The CVar string, the inverse of [`Self::from_cvar`], for the round-trip test.
    #[cfg(test)]
    pub(crate) fn cvar(self) -> &'static str {
        match self {
            Self::Never => "0",
            Self::Smart => "1",
            Self::Always => "2",
        }
    }

    /// The `cameraSmooth<Style><State>{Delay,Factor}` row (family A, `[0xbe0e70]`) at its
    /// defaults; factor 0 cancels. Family B, `cameraSmoothViewData<Style>Yaw{Delay,Factor}`, is
    /// delay 0 and factor 1 (0 under Never), the identity on yaw.
    fn row(self, state: FollowState) -> (f32, f32) {
        match self {
            // Every row is 0/0, and family B's Never factor is 0 as well.
            Self::Never => (0.0, 0.0),
            Self::Smart => match state {
                // Nothing returns while standing or stopping.
                FollowState::Idle | FollowState::Stop => (0.0, 0.0),
                // Driven from outside (a taxi, a spline, a fear): 18°/s on average, 2 s at most.
                FollowState::Track | FollowState::Fear => (0.4, 10.0),
                FollowState::Move | FollowState::Strafe | FollowState::Turn => (0.0, 1.0),
            },
            Self::Always => (0.0, 1.0),
        }
    }
}

/// The seven arming states of 1.12's auto-return classifier (`0x510960`), highest priority first,
/// read off the camera's input command word, not the character's velocity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FollowState {
    /// External control that is not a spline: the reference's `[cam+0x90] & 0x1000`.
    Fear,
    Turn,
    Strafe,
    Move,
    /// Externally driven movement (a taxi, a server spline): `[cam+0x90] & 0x100`.
    Track,
    /// This edge released a movement input (the reference's `stopping` argument). Its rows equal
    /// `Idle`'s at every style; the reference's matrix keeps the two apart.
    Stop,
    Idle,
}

/// The camera's input command word, 1.12's `[InputControl+0x4]` bit for bit. PitchUp and
/// PitchDown are not built, so their bits are never set.
pub(super) mod follow_cmd {
    /// `TurnOrAction`: right mouse, mouselook.
    pub(in crate::player) const RIGHT_MOUSE: u32 = 0x1;
    /// `CameraOrSelectOrMove`: left mouse.
    pub(in crate::player) const LEFT_MOUSE: u32 = 0x2;
    pub(in crate::player) const FORWARD: u32 = 0x10;
    pub(in crate::player) const BACKWARD: u32 = 0x20;
    pub(in crate::player) const STRAFE_LEFT: u32 = 0x40;
    pub(in crate::player) const STRAFE_RIGHT: u32 = 0x80;
    pub(in crate::player) const TURN_LEFT: u32 = 0x100;
    pub(in crate::player) const TURN_RIGHT: u32 = 0x200;
    pub(in crate::player) const AUTORUN: u32 = 0x1000;
    /// The union of bits 20, 21 and 23, which the reference folds into the camera's `Track` flag.
    pub(in crate::player) const TRACK: u32 = 0x100000;
    /// External control, the reference's `[cam+0x90] & 0x1000`: not an InputControl bit, carried
    /// so one word holds every edge that arms.
    pub(in crate::player) const FEAR: u32 = 0x2000_0000;

    pub(in crate::player) const MOVE_BITS: u32 = FORWARD | BACKWARD | AUTORUN;
    pub(in crate::player) const STRAFE_BITS: u32 = STRAFE_LEFT | STRAFE_RIGHT;
    pub(in crate::player) const TURN_BITS: u32 = TURN_LEFT | TURN_RIGHT;
}

/// The auto-follow's knobs: three 1.12 CVars at the reference's defaults.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct FollowConfig {
    /// `cameraSmoothStyle`.
    pub(crate) style: FollowStyle,
    /// `cameraSmoothTrackingStyle`: the style the reference uses whenever the mask contains `Track`
    /// or `Fear`, even when another state wins (`0x510a51 test bl,0x44`); default Smart.
    pub(crate) tracking_style: FollowStyle,
    /// `cameraYawSmoothSpeed`, °/s.
    pub(crate) yaw_speed: f32,
}

impl Default for FollowConfig {
    fn default() -> Self {
        Self {
            style: FollowStyle::default(),
            tracking_style: FollowStyle::default(),
            yaw_speed: FOLLOW_SPEED_DEFAULT,
        }
    }
}

/// What [`seat_camera`] needs for the auto-follow: the knobs, the facing, and the input word.
pub(super) struct FollowInput {
    pub(super) cfg: FollowConfig,
    /// The character's facing ([`FlyCam::yaw`]'s convention); the offset is `cam.yaw − face_yaw`.
    pub(super) face_yaw: f32,
    /// This frame's [`follow_cmd`] word.
    pub(super) command: u32,
}

impl FollowInput {
    /// The winning state: the reference's priority scan, highest first.
    fn state(&self, stopping: bool) -> FollowState {
        let mf = self.command;
        let held = |bits: u32| mf & bits != 0;
        if held(follow_cmd::FEAR) {
            FollowState::Fear
        } else if held(follow_cmd::TURN_BITS) || held(follow_cmd::RIGHT_MOUSE) {
            FollowState::Turn
        } else if held(follow_cmd::STRAFE_BITS)
            || (held(follow_cmd::RIGHT_MOUSE) && held(follow_cmd::TURN_BITS))
        {
            FollowState::Strafe
        } else if held(follow_cmd::MOVE_BITS)
            || (held(follow_cmd::RIGHT_MOUSE) && held(follow_cmd::LEFT_MOUSE))
        {
            FollowState::Move
        } else if held(follow_cmd::TRACK) {
            FollowState::Track
        } else if stopping {
            FollowState::Stop
        } else {
            FollowState::Idle
        }
    }

    /// The tracking style whenever the word holds `Track` or `Fear`, even if another state wins.
    fn style(&self) -> FollowStyle {
        if self.command & (follow_cmd::TRACK | follow_cmd::FEAR) != 0 {
            self.cfg.tracking_style
        } else {
            self.cfg.style
        }
    }
}

/// The armed yaw transition: the reference's descriptor `[+0x208 startMs, +0x20c dur,
/// +0x210 target, +0x214 start]`, in seconds and offset space.
#[derive(Clone, Copy, Debug)]
struct FollowArm {
    /// The offset the transition started from, radians (camera yaw minus character facing).
    from: f32,
    /// The target offset: 0, directly behind, at the defaults, since `cameraYawSmoothMin`/`Max`
    /// are both 0 and the reference substitutes the crossed bound when the offset is outside them.
    to: f32,
    /// Seconds the move takes, already clamped to `[FOLLOW_TIME_MIN, FOLLOW_TIME_MAX]`.
    dur: f32,
    /// Seconds of dead time before it starts (`Track`/`Fear` under Smart: 0.4).
    delay: f32,
    elapsed: f32,
    /// The `(delay, factor)` it was armed with, the reference's re-arm memo (`[+0x218, +0x21c]`).
    armed_with: (f32, f32),
}

/// The auto-follow's state: the last input word (edges arm a return) and the transition in flight.
#[derive(Default)]
pub(super) struct FollowRig {
    last_command: Option<u32>,
    arm: Option<FollowArm>,
}

impl FollowRig {
    /// Run the auto-follow for a frame; returns the camera yaw it wants, if any. As in the
    /// reference (`0x510960`, `0x50f160`), a return is armed on an input edge from a snapshot and
    /// then plays out unattended, not a per-frame chase: holding W changes nothing.
    fn advance(
        &mut self,
        input: &FollowInput,
        cam_yaw: f32,
        dt: f32,
        look_held: bool,
    ) -> Option<f32> {
        let word = input.command;
        let previous = self.last_command.replace(word);
        // A held drag owns the camera: the yaw channel is frozen (`0x50f623`), arming is gated
        // (`0x510850`, `!([cam+0x90] & 1)`), and entering mouse-look cancels the transition in
        // flight (`0x50fe30`), so a return starts at the next edge, usually the release itself.
        if look_held {
            self.arm = None;
            return None;
        }
        // Any change of the word re-evaluates, as the reference does on every binding call.
        if let Some(p) = previous.filter(|p| *p != word) {
            // `stopping`, the reference's second argument: a movement bit went away.
            let stopping = (p & !word)
                & (follow_cmd::MOVE_BITS | follow_cmd::STRAFE_BITS | follow_cmd::TURN_BITS)
                != 0;
            self.arm(input, cam_yaw, stopping);
        }
        let arm = self.arm.as_mut()?;
        arm.elapsed += dt;
        let t = arm.elapsed - arm.delay;
        if t < 0.0 {
            return None; // still inside the delay window
        }
        let s = t / arm.dur;
        let offset = if s >= 1.0 {
            let to = arm.to;
            self.arm = None;
            to
        } else {
            // The reference's cosine smoothstep (`0x5b7bb0`): `a + (b − a)·(1 − cos(πs))/2`.
            let e = (1.0 - (std::f32::consts::PI * s).cos()) * 0.5;
            arm.from + (arm.to - arm.from) * e
        };
        Some(wrap_pi(input.face_yaw + offset))
    }

    /// `(elapsed, delay, duration)` of the armed transition in seconds, for `WOW_CAM_DUMP`.
    fn probe(&self) -> Option<(f32, f32, f32)> {
        self.arm.map(|a| (a.elapsed, a.delay, a.dur))
    }

    /// The arming half (`0x510960` → `0x512c70`): pick the row, then cancel or snapshot a return.
    fn arm(&mut self, input: &FollowInput, cam_yaw: f32, stopping: bool) {
        let (delay, factor) = input.style().row(input.state(stopping));
        if factor == 0.0 {
            // Factor 0 cancels, keeping the offset (Smart standing still, and all of Never).
            self.arm = None;
            return;
        }
        // Directly behind, at the defaults ([`FollowArm::to`]).
        let to = 0.0;
        let from = wrap_pi(cam_yaw - input.face_yaw);
        let gap = (to - from).abs();
        if gap < FOLLOW_EPS {
            return; // already there: the reference does not arm
        }
        // The re-arm memo: an edge asking for the transition in flight does not restart it.
        if self.arm.is_some_and(|a| {
            a.to == to
                && (a.armed_with.0 - delay).abs() < FOLLOW_EPS
                && (a.armed_with.1 - factor).abs() < FOLLOW_EPS
        }) {
            return;
        }
        let rate = input.cfg.yaw_speed.to_radians().max(FOLLOW_EPS);
        let dur = (gap / rate * factor).clamp(FOLLOW_TIME_MIN, FOLLOW_TIME_MAX);
        self.arm = Some(FollowArm {
            from,
            to,
            dur,
            delay,
            elapsed: 0.0,
            armed_with: (delay, factor),
        });
    }
}

/// Camera pitch clamp, ±89° (`0x8089d8`/`0x8089dc`, 1.5533430576 rad, applied in `0x510120`), the
/// same at every zoom: the reference has no separate first-person limit.
pub(super) const CAM_PITCH_LIMIT: f32 = 89.0 * std::f32::consts::PI / 180.0;
/// Rate (1/s) at which the arm eases back out once clear; pulling in is instant.
const CAM_RETURN_RATE: f32 = 6.0;
/// Floor on the world pivot height, 5/6 yd (`0x50ca90`'s clamp, `0x50e570`'s corridor bound).
pub(super) const CAM_PIVOT_FLOOR: f32 = 5.0 / 6.0;
/// Ceiling on the world framing-pivot height, 15 yd (`[0x8089c8]`, the same clamp in `0x50ca90`).
pub(super) const CAM_PIVOT_CEIL: f32 = 15.0;
/// Head height before a model has attached: about a human's neck height.
pub(super) const CAM_PIVOT_FALLBACK: f32 = 1.8;

/// A modeled unit's world framing-pivot height: [`CameraPivot`] × `scale`, clamped to
/// `[CAM_PIVOT_FLOOR, CAM_PIVOT_CEIL]` (`0x50ca90`). `swimming` picks the preset per frame as
/// `0x50f880` does (MOVEFLAG_SWIMMING, `0x50f89e`): the swim drop gives the reference's
/// `cam+0x124`. Its third preset is not built: `cam+0x11c`/`cam+0x120`, split on
/// `cam+0x198 < 1.8315` (`[0x8089b0]`), are both 1.9002692 on a scale-1 human, and what sets them
/// apart is untraced.
pub(super) fn model_pivot_height(pivot: &CameraPivot, scale: f32, swimming: bool) -> f32 {
    let local = if swimming {
        pivot.height_local - pivot.swim_drop_local
    } else {
        pivot.height_local
    };
    (local * scale).clamp(CAM_PIVOT_FLOOR, CAM_PIVOT_CEIL)
}

/// World head height above a unit's feet: the standing preset, or [`CAM_PIVOT_FALLBACK`] with no
/// model. For the 3D-audio listener (`SoundListenerAtCharacter=1`, `0x457890`) and a far-sight
/// subject, whose movement flags we do not carry. A camera pivot passes the raw
/// `OBJECT_FIELD_SCALE_X`, as the reference reads it (`0x469f10`), not the 2 s-eased render scale
/// only a selection ring uses (`0x4833d3`); the listener passes the rendered scale.
pub(crate) fn head_height(pivot: Option<&CameraPivot>, scale: f32) -> f32 {
    pivot.map_or(CAM_PIVOT_FALLBACK, |p| model_pivot_height(p, scale, false))
}

/// `cameraHeightSmoothSpeed`, default 1.2 yd/s: a move lasts `|Δh| / this`
/// (`0x51276c`/`0x512777`), an average rate, with no duration clamp.
const CAM_PIVOT_SMOOTH_SPEED: f32 = 1.2;
/// The framing pivot's height channel, a [`SmoothChannel`]: the reference's live `cam+0xfc`
/// chasing `cam+0x1c8`, armed by `0x5126b0` → `0x512790`, stepped in `0x50f160`
/// (`[0x50f36a, 0x50f417)`). It glides both ways: `max(target, live)` (`0x50e5a9`) only seeds the
/// collision corridor, and the far chain clamps back to the live value (`0x50e767`). Only the
/// first arm snaps (`0x5127d4`), and an unresolved model holds it (`0x50e907` skips the update).
#[derive(Default)]
pub(super) struct PivotGlide {
    /// Linear, not angular: the live value is yards, so the armer's `2π` rewrap must not run on it.
    channel: SmoothChannel,
    /// The reference's latch bit `0x80`, never cleared: false until the first height, which snaps.
    seeded: bool,
}

impl PivotGlide {
    /// Arm with this frame's target (`None` holds), step, and return the height. Called every
    /// frame, as `0x50f880` is from the driver tail `0x50f011`; the armer's epsilon makes a steady
    /// target a no-op.
    pub(super) fn advance(&mut self, target: Option<f32>, dt: f32) -> f32 {
        if let Some(target) = target {
            if self.seeded {
                self.channel.arm(&Arm::at(target, CAM_PIVOT_SMOOTH_SPEED));
            } else {
                self.seeded = true;
                self.channel.snap(target);
            }
        }
        self.channel.advance(dt)
    }

    /// `(live, target)`, for `WOW_CAM_DUMP`.
    pub(super) fn probe(&self) -> (f32, f32) {
        self.channel.probe()
    }
}

/// The camera rig's state: the zoom, the collided arm, the mouse-look session and the pose
/// channels. `pub(crate)` so [`crate::cursor`] can hide the cursor while looking.
#[derive(Resource, Default)]
pub(crate) struct CameraControl {
    /// Third-person orbit distance (yd), gliding toward `target_distance`.
    pub(super) distance: f32,
    pub(super) target_distance: f32,
    /// The arm's length from the head after world collision: pulled in instantly, eased back out,
    /// and kept apart from `distance` so the chosen zoom survives an obstruction.
    pub(super) collision_distance: f32,
    pub(super) look: Option<LookButton>,
    /// Mouse-look is on: the reference's `[cam+0x90] & 1` (set `0x50fe41`, cleared `0x50fddd`).
    /// Written by [`run_look_session`], read the same frame for `cameraTerrainTilt`'s hand-off.
    pub(super) freelook: bool,
    /// The mouse buttons the world owns this frame, latched by [`latch_world_mouse`].
    pub(super) world_mouse: WorldMouse,
    /// Logical cursor position captured when look began, to restore on release.
    pub(super) cursor_stash: Option<Vec2>,
    /// The self-avatar's alpha from the camera-to-pivot distance, 1 in third person to 0 in first
    /// (`self_model_fade_alpha`); applied by [`apply_self_model_fade`].
    pub(super) self_fade_alpha: f32,
    /// The auto-follow's state; on the rig because it is pose, not setting.
    pub(super) follow: FollowRig,
    /// The framing pivot's height channel ([`PivotGlide`]), on the camera rather than the body so
    /// it glides through a change of subject (a shapeshift, a far-sight switch).
    pub(super) pivot: PivotGlide,
    /// `cameraPivot`'s pitch-bias channel ([`SmartPivot`]).
    pub(super) smart_pivot: SmartPivot,
    /// `cameraTerrainTilt`'s ground-pitch channel and its 100 ms probe throttle ([`TerrainTilt`]).
    pub(super) terrain_tilt: TerrainTilt,
    /// `cameraBobbing`'s session latch and eye offset ([`HeadBob`]).
    pub(super) head_bob: HeadBob,
    /// The sweep clipped the camera on the frame just seated: the reference's
    /// `[cam+0x90] & 0x30000`, written only by the solver `0x50e570`, [`SmartPivot`]'s sixth
    /// conjunct; the look session reads it a frame later, as `0x50fee0` (from `0x514446`) does.
    pub(super) clipped: bool,
}

impl CameraControl {
    /// Park the orbit distance at `d`, both the live value and the wheel target, or the glide
    /// drifts back; for the scripted camera park (`capture::probe_cam`).
    pub(crate) fn park_distance(&mut self, d: f32) {
        self.distance = d;
        self.target_distance = d;
    }

    /// True while a mouse-look drag is active (right- or left-button). The cursor is hidden then.
    pub(crate) fn is_looking(&self) -> bool {
        self.look.is_some()
    }

    /// The self-avatar's render alpha this frame. The blob shadow multiplies it in, as the
    /// reference's shadow diffuse rides the body's model fade slot (`[model+0x180]`).
    pub(crate) fn self_fade(&self) -> f32 {
        self.self_fade_alpha
    }
}

/// The mouse-look mode: `Right` turns the character with the camera, `Left` orbits around it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum LookButton {
    Right,
    Left,
}

impl LookButton {
    fn button(self) -> MouseButton {
        match self {
            LookButton::Right => MouseButton::Right,
            LookButton::Left => MouseButton::Left,
        }
    }
}

/// The mouse buttons the world owns, latched at the press. 1.12 reads the `TurnOrAction` and
/// `CameraOrSelectOrMove` bindings, which a press a UI frame captured never dispatches: no mouse
/// bit in `[InputControl+0x4]`, no look session, no [`FollowState`]. The world's down sets the bit
/// and that button's up clears it, so a frame appearing under the locked cursor cannot steal it.
#[derive(Default)]
pub(super) struct WorldMouse {
    /// Held by the world right now, indexed by [`LookButton`].
    held: [bool; 2],
    /// Took its down edge from the world this frame, same index.
    down: [bool; 2],
}

impl WorldMouse {
    pub(super) fn held(&self, b: LookButton) -> bool {
        self.held[b as usize]
    }

    pub(super) fn down(&self, b: LookButton) -> bool {
        self.down[b as usize]
    }

    /// Both primaries in the world's hand: the both-button run.
    pub(super) fn both(&self) -> bool {
        self.held(LookButton::Right) && self.held(LookButton::Left)
    }

    /// Latch this frame; `world_press` says whether a down edge belongs to the world. A held bit
    /// rides to its release, including a cover's emptying of the button planes.
    fn update(&mut self, buttons: &ButtonInput<MouseButton>, world_press: bool) {
        for b in [LookButton::Right, LookButton::Left] {
            let i = b as usize;
            self.down[i] = world_press && buttons.just_pressed(b.button());
            self.held[i] = (self.held[i] || self.down[i]) && buttons.pressed(b.button());
        }
    }
}

/// Decide once per frame which mouse buttons the world owns: a press in the viewport off the UI,
/// or any press while a look session owns the cursor (a chord's second button). Its own system,
/// ahead of `/follow`'s both-button cancel ([`super::follow::steer_follow`]), which runs before
/// the controller and would otherwise read it a frame late.
pub(super) fn latch_world_mouse(
    buttons: Res<ButtonInput<MouseButton>>,
    // The raw flag: a press on a nameplate is the plate's, so dragging from a plate never turns the
    // camera, as in 1.12. `0x7662c0` hands a mouse-down to one frame (capture at `0x7663e9`), and
    // `CBindings::ExecuteBinding` (`0x4b7990`) runs only from `CGWorldFrame` handlers; the release
    // goes to the plate's click slot (`0x7792d0` → `0x7cb910` → `0x4949f0`). Entering freelook
    // disables plate input (`0x60f830`, from `0x483e80`) for a world drag across one. A plate's
    // ground-targeting veto (`+0x3c`, `0x7cba30`) arrives through `PointerOverUi`
    // (`UiScript::set_nameplate_hit_test_veto`). The wheel still zooms over a plate.
    pointer_over_ui: Res<crate::ui_script::PointerOverUi>,
    mut rig: ResMut<CameraControl>,
    cameras: Query<&Camera, With<FlyCam>>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    let over_ui = pointer_over_ui.0;
    let world_press = rig.look.is_some() || (cursor_in_viewport(&window, camera) && !over_ui);
    rig.world_mouse.update(&buttons, world_press);
}

/// The right button's down edge in the world, before the press is judged a click or a drag:
/// `CGWorldFrame::OnMouseDown` (`0x483c40`) runs its hook `0x492c20` ahead of the button's binding,
/// so ground targeting's cancel and the repair-mode reset read it before the look session.
pub(super) fn send_world_right_press(
    rig: Res<CameraControl>,
    mut world_right_press: MessageWriter<WorldRightPress>,
) {
    if rig.world_mouse.down(LookButton::Right) {
        world_right_press.write(WorldRightPress);
    }
}

#[derive(Component)]
// `pub(crate)` for the scripted camera park's query; `FlyCam::park` is its one lever.
pub(crate) struct FlyCam {
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) speed: f32,
}

impl FlyCam {
    /// Point the rig at an absolute world yaw and pitch, for `capture::probe_cam`.
    pub(crate) fn park(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = pitch;
    }
}

/// A model's framing-pivot height in model-local yards before scale, the reference's camera-target
/// height (`0x50cbc0`): `attach17.z + 0.0972` from M2 attachment 17 where the model has one, else
/// `0.9 ×` the vertex box's Z extent; `0.0` for a display with no bounds. Stamped on every modeled
/// unit at attach; [`model_pivot_height`] makes it the world pivot.
#[derive(Component, Clone, Copy)]
pub(crate) struct CameraPivot {
    pub height_local: f32,
    /// How far that height drops while swimming, model-local before scale
    /// ([`benilla_formats::M2Bounds::swim_pivot_drop`], `StandSeq.max.z − SwimSeq.max.z`,
    /// `0x50ccf6`); `0.0` with no Swim sequence, where the two presets coincide.
    pub swim_drop_local: f32,
}

/// The mouse-look session: start, stop and hand-off between the buttons, cursor grab and restore,
/// the look rotation, and the click tests behind [`WorldClick`]/[`WorldRightClick`]. Orbit and
/// select are independent: each primary press engages its session at once and arms a click test,
/// and the release decides on [`PressGesture::is_click`] alone.
pub(super) fn run_look_session(
    buttons: &ButtonInput<MouseButton>,
    mouse_motion: &AccumulatedMouseMotion,
    both_buttons: bool,
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    face_yaw: &mut f32,
    window: &mut Window,
    cursor_opts: &mut CursorOptions,
    inspect_enabled: bool,
    // A left press the UI consumed as a cursor-payload world drop, which must not also select.
    click_consumed: bool,
    world_click: &mut MessageWriter<WorldClick>,
    world_right_click: &mut MessageWriter<WorldRightClick>,
    left_click: &mut Option<PressGesture>,
    right_click: &mut Option<PressGesture>,
    look_cfg: LookConfig,
    // `cameraPivot`'s routing lives here because the reference's does: `0x50fee0`, the mouse-motion
    // handler, sends each motion event to the pitch or the bias.
    dynamics: &DynamicsInput,
    // Seconds on the app clock, for the press predicate's two time gates.
    now: f32,
) {
    // A chord is a both-button run, never a select: the reference kills the pending click and arms
    // none while another primary's binding is held (`0x514ac1`, `0x51481a`).
    if rig.world_mouse.both() {
        *left_click = None;
        *right_click = None;
    }
    // Both tests accumulate the rotation the press asked for, charged before the pitch clamp: the
    // reference accumulates raw motion, so a drag pinned at the pitch limit still spends budget.
    let (yaw_rate, pitch_rate) = (look_cfg.yaw_rate(), look_cfg.pitch_rate());
    let dyaw = (mouse_motion.delta.x * yaw_rate).abs();
    let dpitch = (mouse_motion.delta.y * pitch_rate).abs();
    for test in [&mut *left_click, &mut *right_click].into_iter().flatten() {
        test.yaw_travel += dyaw;
        test.pitch_travel += dpitch;
    }

    // Both buttons engage their session on the down edge, with no threshold (`0x51491f`); the click
    // test rides along to the release. Looking hides and locks the cursor until the release.
    if let Some(active) = rig.look {
        if !buttons.pressed(active.button()) {
            // The button went up: settle its click test (a chord already cancelled both).
            let test = match active {
                LookButton::Left => left_click.take(),
                LookButton::Right => right_click.take(),
            };
            if let Some(test) = test {
                if test.is_click(now) {
                    match active {
                        LookButton::Left => {
                            world_click.write(WorldClick);
                        }
                        LookButton::Right => {
                            world_right_click.write(WorldRightClick);
                        }
                    }
                }
            }
            // Hand off to the other button if the world holds it, as the reference keeps turning;
            // a button the UI holds never fired its binding.
            let other = match active {
                LookButton::Right => LookButton::Left,
                LookButton::Left => LookButton::Right,
            };
            if rig.world_mouse.held(other) {
                rig.look = Some(other);
            } else {
                rig.look = None;
                cursor_opts.grab_mode = CursorGrabMode::None;
                // Show the cursor again; on macOS hiding is the cursor subsystem's job.
                cursor_opts.visible = true;
                if let Some(pos) = rig.cursor_stash.take() {
                    window.set_cursor_position(Some(pos));
                }
            }
        }
    } else {
        // `WorldMouse` has already dropped a press over the UI, the dev overlay or outside the
        // viewport. Right-drag turns, and arms its click test unless left is held.
        if rig.world_mouse.down(LookButton::Right) {
            rig.look = Some(LookButton::Right);
            rig.cursor_stash = window.cursor_position();
            cursor_opts.grab_mode = CursorGrabMode::Locked;
            cursor_opts.visible = false;
            *right_click =
                (!rig.world_mouse.held(LookButton::Left)).then(|| PressGesture::new(now));
        } else if rig.world_mouse.down(LookButton::Left) && !inspect_enabled {
            // Left-drag orbits, engaged on the press like right (`0x51491f`); the select settles
            // at the release. While the inspector is armed, left belongs to it.
            rig.look = Some(LookButton::Left);
            rig.cursor_stash = window.cursor_position();
            cursor_opts.grab_mode = CursorGrabMode::Locked;
            cursor_opts.visible = false;
            // A cursor-payload world drop still orbits (`0x51491f`) but must not also select.
            *right_click = None;
            *left_click = (!click_consumed && !rig.world_mouse.held(LookButton::Right))
                .then(|| PressGesture::new(now));
        }
    }

    // Look rotation; a right-drag also turns the character.
    if let Some(active) = rig.look {
        let delta = mouse_motion.delta;
        let d_yaw = -delta.x * yaw_rate;
        cam.yaw += d_yaw;
        // `mouseInvertPitch` flips only the pitch axis.
        let dy = if look_cfg.invert_pitch {
            -delta.y
        } else {
            delta.y
        };
        // The pivot's fork (`0x50fee0`): a pinned camera looking level or up, dragged mostly
        // vertically, spends the delta on the pitch bias; `None` means the integrator does not run.
        let d_pitch = -dy * pitch_rate;
        if let Some(d_pitch) = rig.smart_pivot.route_pitch(
            d_pitch,
            d_yaw,
            cam.pitch,
            &dynamics.subject,
            rig.clipped,
            &dynamics.options,
        ) {
            cam.pitch = (cam.pitch + d_pitch).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
        }
        if active == LookButton::Right || both_buttons {
            *face_yaw = cam.yaw;
        }
    }
    // Freelook is right-held mouse-look or a both-button run, which steers like it; a left-drag
    // orbit is not. This test and the `face_yaw` sync above must stay alike.
    rig.freelook = rig.look == Some(LookButton::Right) || (rig.look.is_some() && both_buttons);
}

/// Wheel zoom, run every frame: `CAMERAZOOMIN`/`OUT` move the target, and the distance glides to it
/// at a constant `cameraDistanceMoveSpeed`, as the reference's does. `scroll` is this frame's net
/// zoom-in in notches (positive is closer).
pub(super) fn apply_zoom_scroll(scroll: f32, dt: f32, rig: &mut CameraControl, max: f32) {
    if scroll != 0.0 {
        rig.target_distance =
            (rig.target_distance - scroll * CAM_ZOOM_STEP).clamp(CAM_DIST_MIN, max);
    }
    // Re-clamp every frame, so lowering the max-distance slider pulls the camera in.
    rig.target_distance = rig.target_distance.min(max);
    let max_step = CAM_MOVE_SPEED * dt;
    rig.distance += (rig.target_distance - rig.distance).clamp(-max_step, max_step);
}

/// Seat the camera on whatever it orbits: our own body, or the far-sight subject `PLAYER_FARSIGHT`
/// names (Mind Vision, Sentry Totem, Mind Control), substituting the orbit centre, sweep origin and
/// pivot target before [`seat_camera`]. Both of the controller's seat paths call it, as far sight
/// outlives a spline or a possession (Sentry Totem has no interrupt flags).
pub(super) fn seat_on_subject(
    dt: f32,
    turn_delta: f32,
    feet: Vec3,
    head: Vec3,
    body_pivot: Option<f32>,
    view: &super::view_subject::ViewSubject,
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    cam_t: &mut Mut<Transform>,
    collide: &benilla_world::collision::WorldCollision<'_, '_>,
    follow: &FollowInput,
    dynamics: &DynamicsInput,
) {
    // The sweep origin moves with the subject: our own head would cast the boom across the world.
    let (orbit_pos, sweep_from) = match view.remote {
        Some(v) => (v.feet, v.sweep_origin()),
        None => (feet, head),
    };
    // The framing height is the channel's, easing to the target over `|Δh| / 1.2` s
    // (`0x50f160`); a far-sight subject feeds the same channel.
    let live_pivot = rig
        .pivot
        .advance(view.remote.map(|v| v.pivot_height).or(body_pivot), dt);
    // `cameraWaterCollision`'s pivot corridor (`super::camera_water`), classified against the
    // channel's target (`[cam+0x1c8]`), as the reference does. `headroom` is 1.0: the reference's
    // vertical head-room probe (`0x50e6bd`) is not built, so a low ceiling keeps the full reach.
    let (_, pivot_target) = rig.pivot.probe();
    let (band, depth) = super::camera_water::classify(
        // Our own body's cached surface, only while watching our own body: `0x511ad0` reads the
        // camera target's liquid object, and far sight carries none.
        view.remote
            .is_none()
            .then_some(dynamics.surface_y)
            .flatten(),
        orbit_pos.y,
        pivot_target,
    );
    let corridor = if dynamics.options.water_collision {
        super::camera_water::corridor(band, depth, live_pivot)
    } else {
        super::camera_water::corridor_off(live_pivot)
    };
    let orbit_pivot = super::camera_water::pivot_height(&corridor, pivot_target, live_pivot, 1.0);
    // The corridor floor also lifts the sweep origin, which keeps the arm off the water. The
    // reference starts the boom at the clamped pivot height (`0x50e786`), `surface + 2/9` in the
    // surface band; ours starts at the head, `surface + 0.171` on a surface-swimming human male.
    // `max` only raises it, so dry land and the submerge band are untouched.
    let sweep_from = Vec3::new(
        sweep_from.x,
        sweep_from.y.max(orbit_pos.y + corridor.floor),
        sweep_from.z,
    );
    // `cameraTerrainTilt`'s probe looks at the ground ahead of the subject: a level ray along its
    // facing from `feet + 5/3`, the hit pulled back `5/18`, then a `64/9` drop, so the slope is
    // that of the ground being walked onto, the subject's own under far sight.
    let ground_probe = || {
        let origin = orbit_pos + Vec3::Y * super::camera_dynamics::PROBE_LIFT;
        let fwd = Quat::from_rotation_y(dynamics.subject.facing) * Vec3::NEG_Z;
        let reach = Dir3::new(fwd)
            .ok()
            .and_then(|d| collide.ray_body(origin, d, super::camera_dynamics::PROBE_REACH))
            .map_or(super::camera_dynamics::PROBE_REACH, |h| {
                h.distance - super::camera_dynamics::PROBE_BACKOFF
            });
        let ahead = origin + fwd * reach;
        let ground_y = collide
            .ray_body(ahead, Dir3::NEG_Y, super::camera_dynamics::PROBE_DROP)
            .map_or(ahead.y - super::camera_dynamics::PROBE_DROP, |h| {
                ahead.y - h.distance
            });
        // A hit closer than the backoff runs the probe behind the subject, and `L = √(Δx² + Δy²)`
        // still divides by a positive run: the reference's own arithmetic.
        (ground_y - orbit_pos.y) / reach.abs().max(1.0e-3)
    };
    rig.terrain_tilt.advance(
        ground_probe,
        dynamics.options.terrain_tilt,
        &dynamics.subject,
        dynamics.smooth_style,
        &dynamics.options,
        dt,
    );
    // The mouse-look hand-off (`0x50d500` push, `0x50d520` pop), after the channel steps; the
    // reference fires it from the input handler, at most one frame of channel motion apart (a
    // fortieth of a degree at `cameraGroundSmoothSpeed`).
    let handed = rig.terrain_tilt.hand_off(rig.freelook);
    if handed != 0.0 {
        cam.pitch = (cam.pitch + handed).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    }

    // `cameraBobbing`: the session runs whatever the CVar says and only its output is gated. It
    // reads the zoom, not the collided arm, as the reference's first conjunct is `[cam+0xec]`: a
    // camera squeezed against a wall is not in first person.
    rig.head_bob
        .advance(rig.distance, &dynamics.subject, &dynamics.options, dt);

    seat_camera(
        dt,
        turn_delta,
        orbit_pos,
        sweep_from,
        orbit_pivot,
        rig,
        cam,
        cam_t,
        collide,
        follow,
        dynamics,
    );
}

/// Seat the third-person camera: orient it, sweep the arm from `head` toward the ideal seat
/// (snapping in, easing out), write the transform, and set the self-avatar fade. A keyboard turn
/// (and the drunk veer on `turn_delta`) carries the camera rigidly; a transport deck's turn is
/// applied in [`super::control`]'s ride block, outside the look-session gate here, which would
/// drop it during a drag. `player_pos`, `head` and `cam_pivot_height` are [`seat_on_subject`]'s
/// orbit centre, sweep origin and framing height.
pub(super) fn seat_camera(
    dt: f32,
    turn_delta: f32,
    player_pos: Vec3,
    head: Vec3,
    cam_pivot_height: f32,
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    cam_t: &mut Mut<Transform>,
    collide: &benilla_world::collision::WorldCollision<'_, '_>,
    follow: &FollowInput,
    dynamics: &DynamicsInput,
) {
    // A keyboard turn carries the camera rigidly, as the reference's facing-relative yaw does; a
    // held drag takes no carry. The auto-follow writes an absolute yaw, where the reference stores
    // the offset and re-adds the facing at render time (`0x50f7f2`): the same picture, so Never
    // leaves this carry running.
    let look_held = rig.look.is_some();
    if !look_held {
        cam.yaw += turn_delta;
    }
    if let Some(yaw) = rig.follow.advance(follow, cam.yaw, dt, look_held) {
        cam.yaw = yaw;
    }
    // The framing pivot is `feet + cam_pivot_height`; at zoom 0 the camera sits on it. One sweep
    // runs from the head (the capsule's top hemisphere centre) to `pivot - fwd·zoom`, and body
    // collision keeps the head inside the room, so the camera never ends up past a wall or ceiling.
    // The cast ignores origin penetration, so a head grazing a surface still casts outward.
    // The seat is built from the unbiased pitch and the view from the biased one: the reference
    // stores the eye (`0x50edcc` → `0x50de00`) before rotating the basis `[cam+0x14]` by the bias
    // `[cam+0x104]` (`0x50ee32`), so the arm never swings and the look does.
    let bias = rig.smart_pivot.bias();
    // The ground tilt is part of the arm's pitch, unlike the bias: `0x50f710` clamps
    // `[cam+0xf4] + [cam+0x108]` to ±89° before the basis is built.
    let arm_pitch = (cam.pitch + rig.terrain_tilt.pitch()).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    let arm_rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, arm_pitch, 0.0);
    // Not re-clamped: the bias is composed after that clamp (`0x50ee58`), bounded only by its own
    // one-sided accumulate (`SmartPivot::route_pitch`) and ±89° at the body hand-off (`0x5103e0`).
    let rotation = if bias == 0.0 {
        arm_rotation
    } else {
        Quat::from_euler(EulerRot::YXZ, cam.yaw, arm_pitch + bias, 0.0)
    };
    // `Transform::forward()` is exactly `rotation * -Z`; computed here so the write can be gated.
    let cam_fwd = arm_rotation * Vec3::NEG_Z;
    let pivot = player_pos + Vec3::Y * cam_pivot_height;
    let seat = pivot - cam_fwd * rig.distance;
    let boom = seat - head;
    let boom_len = boom.length().max(1.0e-3);
    // The camera collides with the WMO camera/LOS faces (DETAIL kept, NOCAMCOLLIDE dropped),
    // terrain, doodads and GameObjects. Under `cameraWaterCollision` (default on) it also hits the
    // waterline: `0x50e5ec` ORs the ADT-liquid nibble `0xf0000` into `0x50e570`'s query word,
    // reaching a two-sided Möller–Trumbore test over the chunk's MCLQ slots (`0x69cc13`,
    // `0x7c2c40`, hit at `0x7c2e5f`) with no near floor. That leg is a ray (`0x672170` has no
    // radius), so `cast_camera` traces water as a ray and solids as a sphere.
    let hit = collide.cast_camera(head, boom, dynamics.options.water_collision);
    // The solver's clip verdict (`0x50e570`'s `0x30000`, OR'd into `[cam+0x90]` by the driver):
    // [`SmartPivot`]'s sixth conjunct, so an unobstructed camera never pivots.
    rig.clipped = hit.is_some();
    let open = hit.unwrap_or(boom_len);
    // Snap in when geometry intrudes; ease back out once it clears.
    rig.collision_distance = if open < rig.collision_distance {
        open
    } else {
        let t = 1.0 - (-CAM_RETURN_RATE * dt).exp();
        rig.collision_distance + (open - rig.collision_distance) * t
    };
    let frac = (rig.collision_distance / boom_len).clamp(0.0, 1.0);
    let seated = head + boom * frac;
    // The head bob is a world-space translation of the eye, added last into the slot
    // [`crate::camera_shake`] writes, as the reference folds the bob into the shake's accumulator
    // (`0x50eb0f`) and applies the pair once (`0x50de00`). Zero when nothing bobs.
    let translation = seated + rig.head_bob.offset();
    // Written only on a change, so a settled camera does not mark its transform changed every
    // frame; bit equality, so a sub-epsilon drift still lands.
    {
        let t = cam_t.bypass_change_detection();
        if t.rotation != rotation || t.translation != translation {
            t.rotation = rotation;
            t.translation = translation;
            cam_t.set_changed();
        }
    }
    // The eye is never moved for liquid, as in the reference (no liquid-height query in
    // `[0x7ac640, 0x7ae010)`): the submersion probe flips the frame once the lowest near-plane
    // corner crosses the surface, a few inches at `nearclip`'s 0.1 (re-stamped at `0x511bd4`).
    // `WOW_CAM_DUMP`: the realized pose per frame, bit-exact; `open` beside the eased arm shows a
    // hit/miss alternation in the cast before the camera moves.
    if cam_dump_enabled() {
        // `follow`: the offset, classified state, input word and `elapsed/dur+delay` (`-1`
        // unarmed); `off` moving while unarmed means something else moved the camera.
        let (elapsed, delay, dur) = rig.follow.probe().unwrap_or((-1.0, -1.0, -1.0));
        eprintln!(
            "[cam] yaw {:.6} pitch {:.6} dist {:.6} open {:.6} coll {:.6} frac {:.6} \
             pos [{:.6},{:.6},{:.6}] bits [{:08x},{:08x},{:08x}] \
             follow off {:.6} state {:?} word {:06x} arm {:.3}/{:.3}+{:.3}",
            cam.yaw,
            cam.pitch,
            rig.distance,
            open,
            rig.collision_distance,
            frac,
            translation.x,
            translation.y,
            translation.z,
            translation.x.to_bits(),
            translation.y.to_bits(),
            translation.z.to_bits(),
            wrap_pi(cam.yaw - follow.face_yaw),
            follow.state(false),
            follow.command,
            elapsed,
            dur,
            delay,
        );
    }

    // The pivot bias's per-frame half (`0x50ed77` → `0x5107f0`), eased to zero at
    // `cameraTargetSmoothSpeed` once the gate is false; after the sweep, so `rig.clipped` is fresh.
    rig.smart_pivot.advance(
        cam.pitch,
        &dynamics.subject,
        rig.clipped,
        dynamics.tracking_style,
        &dynamics.options,
        dt,
    );

    // Fade by the realized camera-to-pivot distance from the seated eye, not the bobbed one, and
    // the live `nearclip`: the fade finishes where the near plane starts cutting.
    rig.self_fade_alpha = self_model_fade_alpha(
        (seated - pivot).length(),
        dynamics.nearclip,
        SELF_FADE_WINDOW,
    );
}

/// Apply the self-avatar fade ([`CameraControl::self_fade_alpha`]) to the body parts and every
/// attach model under a joint, through the blend twin's alpha; α 0 hides them. Runs after the
/// interior classifier and the appear/despawn fades, writes only while fading, and hands the
/// channel back on the frame the fade ends. Billboard cards (the night-elf eye glow) are world
/// roots following an anchor in the model: one whose anchor was walked takes the same α, with
/// `Visibility` left to the card's hidden-owner mirror.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(crate) fn apply_self_model_fade(
    rig: Res<CameraControl>,
    self_player: Query<(Entity, Option<&crate::aura_visual::AuraNodes>), With<Embodied>>,
    children_of: Query<&Children>,
    mut parts: Query<
        (
            &FadeMaterials,
            &mut MeshTag,
            &mut MeshMaterial3d<WowModelMaterial>,
            &mut Visibility,
            Option<&benilla_world::interior::InteriorLit>,
            Has<benilla_world::model_render::FarSideOfWater>,
        ),
        (
            Without<RenderFade>,
            Without<PendingAppearFade>,
            // Disjoint from the card query below (both take `&mut MeshTag`): cards carry
            // `FadeMaterials` too, and the card loop fades them without touching `Visibility`.
            Without<benilla_world::billboard::BillboardCard>,
        ),
    >,
    mut cards: Query<(
        &benilla_world::billboard::BillboardCard,
        &mut MeshTag,
        Option<&benilla_world::doodad_anim::MatAnim>,
        Option<&FadeMaterials>,
        Option<&mut MeshMaterial3d<WowModelMaterial>>,
        Option<&benilla_world::interior::InteriorLit>,
        Has<benilla_world::model_render::FarSideOfWater>,
    )>,
    // The water-plane twin, composed into every material pick below (`far_resolved`) as the
    // classifier does, so the two never swap against each other.
    far_twins: Res<benilla_world::model_render::FarSideTwins>,
    mut reauthor: ResMut<benilla_world::interior::InteriorReauthor>,
    mut was_fading: Local<bool>,
) {
    let fading = rig.self_fade_alpha < 1.0;
    if !fading && !*was_fading {
        // Steady opaque: nothing to author and nothing to release.
        return;
    }
    let Ok((root, aura)) = self_player.single() else {
        *was_fading = false;
        return;
    };
    // Our own aura translucency (stealth, invisibility, ghost) multiplies in, since this system
    // writes the self body's alpha last; the release fires only once the product is opaque.
    let feather = rig.self_fade_alpha * crate::aura_visual::root_alpha(aura);
    // The walked set names the anchors that belong to this model, for the card pass.
    let mut walked = EntityHashSet::default();
    apply_self_fade_to_descendants(
        root,
        feather,
        &children_of,
        &mut parts,
        &far_twins,
        &mut reauthor,
        &mut walked,
    );
    let alpha = feather.clamp(0.0, 1.0);
    for (card, mut tag, anim, fm, mat, lit, far_side) in &mut cards {
        if !card
            .follows()
            .is_some_and(|anchor| walked.contains(&anchor))
        {
            continue;
        }
        // From the card's authored factor (`MatAnim::current`, written first by
        // `entities::apply_unit_mat_alpha`), not the tag, so the animation survives the fade.
        let authored = anim.map_or(1.0, |a| a.current);
        let bits = benilla_world::mesh_tag::with_alpha(tag.0, authored * alpha);
        if tag.0 != bits {
            tag.0 = bits;
        }
        // The blend twin too: an additive card fades by alpha alone (`wow_model.wgsl`), an opaque
        // one only through the twin. `Visibility` is the card's hidden-owner mirror's.
        if let (Some(fm), Some(mut mat)) = (fm, mat) {
            let want = benilla_world::model_render::far_resolved(
                fm.material_for(lit, alpha < 1.0),
                far_side,
                &far_twins,
            );
            if mat.0 != *want {
                mat.0 = want.clone();
            }
        }
    }
    *was_fading = fading;
}

/// Apply the fade, or at `α ≥ 1` the release, to `entity` if it is a fadeable part, then recurse
/// through its children (joints and attach roots carry no `FadeMaterials`). Every visited entity
/// goes into `walked`, which names the anchors whose billboard cards follow this model.
#[allow(clippy::type_complexity)]
fn apply_self_fade_to_descendants(
    entity: Entity,
    alpha: f32,
    children_of: &Query<&Children>,
    parts: &mut Query<
        (
            &FadeMaterials,
            &mut MeshTag,
            &mut MeshMaterial3d<WowModelMaterial>,
            &mut Visibility,
            Option<&benilla_world::interior::InteriorLit>,
            Has<benilla_world::model_render::FarSideOfWater>,
        ),
        (
            Without<RenderFade>,
            Without<PendingAppearFade>,
            Without<benilla_world::billboard::BillboardCard>,
        ),
    >,
    far_twins: &benilla_world::model_render::FarSideTwins,
    reauthor: &mut benilla_world::interior::InteriorReauthor,
    walked: &mut EntityHashSet,
) {
    walked.insert(entity);
    if let Ok((fm, mut tag, mut mat, mut vis, lit, far_side)) = parts.get_mut(entity) {
        if alpha >= 1.0 {
            // The release edge: un-hide, restore the alpha and hand the material back. The alpha
            // restore is ours: the classifier's writes carry the tag's alpha through.
            if *vis != Visibility::Inherited {
                *vis = Visibility::Inherited;
            }
            let bits = benilla_world::mesh_tag::with_alpha(tag.0, 1.0);
            if tag.0 != bits {
                tag.0 = bits;
            }
            let want = benilla_world::model_render::far_resolved(
                fm.material_for(lit, false),
                far_side,
                far_twins,
            );
            if mat.0 != *want {
                mat.0 = want.clone();
            }
            // A classifier-lit part is queued so the next run re-asserts its full payload (probe
            // slot, fog bit) over this feather's writes.
            if lit.is_some() {
                reauthor.0.push(entity);
            }
        } else if alpha <= 0.0 {
            // First-person: hide outright. Leave tag/material to the classifier (not drawn anyway).
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
        } else {
            if *vis != Visibility::Inherited {
                *vis = Visibility::Inherited;
            }
            // Feathering: the blend twin, with the alpha in the tag's alpha field (`with_alpha`
            // keeps the ground-shade byte, so a shadowed avatar does not flash lit).
            let bits = benilla_world::mesh_tag::with_alpha(tag.0, alpha);
            if tag.0 != bits {
                tag.0 = bits;
            }
            // A bake-classified part feathers on the probe-lit blend twin, so the room light rides
            // the fade; `material_for` is shared with the appear/despawn ramp, so both pick alike.
            let want = benilla_world::model_render::far_resolved(
                fm.material_for(lit, true),
                far_side,
                far_twins,
            );
            if mat.0 != *want {
                mat.0 = want.clone();
            }
        }
    }
    if let Ok(children) = children_of.get(entity) {
        for &child in children {
            apply_self_fade_to_descendants(
                child,
                alpha,
                children_of,
                parts,
                far_twins,
                reauthor,
                walked,
            );
        }
    }
}

/// True if the OS pointer is over the world camera's viewport (the whole window when it has none).
fn cursor_in_viewport(window: &Window, camera: &Camera) -> bool {
    let Some(cursor) = window.physical_cursor_position() else {
        return false;
    };
    match &camera.viewport {
        Some(vp) => {
            let min = vp.physical_position.as_vec2();
            let max = min + vp.physical_size.as_vec2();
            cursor.x >= min.x && cursor.y >= min.y && cursor.x < max.x && cursor.y < max.y
        }
        None => true,
    }
}

/// Free-fly (pre-connect or detached): WASD in the camera basis, Space/C up and down, Ctrl 5×.
/// [`super::control`] parks the mover first, so the wire never extrapolates a phantom walk.
pub(super) fn fly_free(
    dt: f32,
    keys: &ButtonInput<KeyCode>,
    typing: bool,
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    cam_t: &mut Transform,
) {
    let keys_pressed = |k: KeyCode| !typing && keys.pressed(k);
    // Detached, the avatar stays opaque.
    rig.self_fade_alpha = 1.0;
    cam_t.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);
    let forward = *cam_t.forward();
    let right = *cam_t.right();
    let mut dir = Vec3::ZERO;
    if keys_pressed(KeyCode::KeyW) {
        dir += forward;
    }
    if keys_pressed(KeyCode::KeyS) {
        dir -= forward;
    }
    if keys_pressed(KeyCode::KeyD) {
        dir += right;
    }
    if keys_pressed(KeyCode::KeyA) {
        dir -= right;
    }
    if keys_pressed(KeyCode::Space) {
        dir += Vec3::Y;
    }
    if keys_pressed(KeyCode::KeyC) {
        dir -= Vec3::Y;
    }
    if dir != Vec3::ZERO {
        let boost = if keys_pressed(KeyCode::ControlLeft) {
            5.0
        } else {
            1.0
        };
        cam_t.translation += dir.normalize() * cam.speed * boost * dt;
    }
}

/// `WOW_CAM_DUMP`: the per-frame camera and turn dump, read once per process.
pub(crate) fn cam_dump_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_CAM_DUMP").is_some())
}

#[cfg(test)]
mod tests {
    use super::super::camera_channel::CHANNEL_EPS;
    use super::*;
    use benilla_assets::BillboardInfo;
    use benilla_formats::BillboardKind;

    use benilla_world::billboard::BillboardCard;
    use benilla_world::mesh_tag::alpha_bits;

    /// Step a [`PivotGlide`] at 60 Hz for `secs`, holding the target; returns each frame's height.
    fn glide_run(g: &mut PivotGlide, target: Option<f32>, secs: f32) -> Vec<f32> {
        let dt = 1.0 / 60.0;
        (0..(secs / dt).round() as usize)
            .map(|_| g.advance(target, dt))
            .collect()
    }

    /// A shapeshift glides the pivot both ways and never snaps: the far chain clamps back to the
    /// live value (`0x50e767`).
    #[test]
    fn a_shapeshift_glides_the_pivot_both_ways_and_never_snaps() {
        // Tauren → cat: the heights measured off a live probe run.
        let (tauren, cat) = (2.4659_f32, 1.0552_f32);
        let mut g = PivotGlide::default();
        assert_eq!(
            g.advance(Some(tauren), 1.0 / 60.0),
            tauren,
            "the first arm snaps"
        );

        for (from, to) in [(tauren, cat), (cat, tauren)] {
            let expected = (to - from).abs() / CAM_PIVOT_SMOOTH_SPEED;
            let frames = glide_run(&mut g, Some(to), expected * 2.0);
            assert!((frames.last().copied().unwrap() - to).abs() < CHANNEL_EPS);
            let arrived = frames
                .iter()
                .position(|h| (h - to).abs() < CHANNEL_EPS)
                .unwrap();
            let took = arrived as f32 / 60.0;
            assert!(
                (took - expected).abs() < 0.05,
                "|Δh| / 1.2 yd/s = {expected:.3} s, took {took:.3} s ({from} → {to})"
            );
            // No frame jumps: the biggest step is the cosine's midpoint rate, far under half of Δ.
            let biggest = frames
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0, f32::max);
            assert!(
                biggest < (to - from).abs() * 0.5,
                "the pivot must never teleport: biggest step {biggest} of Δ {}",
                (to - from).abs()
            );
        }
    }

    /// A model not yet resolved holds the channel (the reference skips the camera update while the
    /// preset is stale, `0x50e907`), so a display swap is a hold, then one glide.
    #[test]
    fn a_body_with_no_model_holds_the_pivot_instead_of_re_aiming_it() {
        let mut g = PivotGlide::default();
        g.advance(Some(2.4659), 1.0 / 60.0);
        let held = glide_run(&mut g, None, 0.5);
        assert!(
            held.iter().all(|h| *h == 2.4659),
            "no target ⇒ no motion (the model is still loading)"
        );
        // …and the glide that follows starts from where it held, not from a placeholder.
        let frames = glide_run(&mut g, Some(1.0552), 2.0);
        assert!(frames[0] < 2.4659 && frames[0] > 2.4);
    }

    /// Re-arming the same target every frame is a no-op (`0x5126b0`'s 0.001 epsilon).
    #[test]
    fn re_arming_the_same_target_every_frame_does_not_stretch_the_glide() {
        let mut g = PivotGlide::default();
        g.advance(Some(1.0), 1.0 / 60.0);
        let frames = glide_run(&mut g, Some(2.2), 2.0);
        assert!(
            (frames.last().copied().unwrap() - 2.2).abs() < CHANNEL_EPS,
            "a per-frame re-arm must still arrive"
        );
    }

    /// The pivot target is clamped to the reference's `[5/6, 15]` band (`0x50ca90`).
    #[test]
    fn the_pivot_target_is_clamped_to_the_references_band() {
        let p = CameraPivot {
            height_local: 2.0,
            swim_drop_local: 0.0,
        };
        assert_eq!(model_pivot_height(&p, 1.0, false), 2.0);
        assert_eq!(model_pivot_height(&p, 0.01, false), CAM_PIVOT_FLOOR);
        assert_eq!(model_pivot_height(&p, 100.0, false), CAM_PIVOT_CEIL);
    }

    /// The swim preset, `cam+0x124` (`0x50f880`, `0x50ccf6`): the standing height less
    /// `StandSeq.max.z − SwimSeq.max.z`. The Human Male figures are computed from its shipped model
    /// with the reference's formula: standing 1.9002692, swimming 1.5120120.
    #[test]
    fn swimming_takes_the_lower_pivot_preset() {
        let human = CameraPivot {
            height_local: 1.9002692,
            swim_drop_local: 0.3882572,
        };
        assert!((model_pivot_height(&human, 1.0, false) - 1.9002692).abs() < 1e-5);
        assert!((model_pivot_height(&human, 1.0, true) - 1.512_012).abs() < 1e-5);
        // The preset scales, then clamps: `0x50ca90` clamps all three presets alike.
        assert!(
            (model_pivot_height(&human, 2.0, true) - 2.0 * 1.512_012).abs() < 1e-5,
            "the swim preset scales like its sibling"
        );
        assert_eq!(model_pivot_height(&human, 0.01, true), CAM_PIVOT_FLOOR);
        assert_eq!(model_pivot_height(&human, 100.0, true), CAM_PIVOT_CEIL);
    }

    /// A model with no Swim sequence (the reference needs both ids 0 and 0x2a, `0x711960`) has a
    /// drop of 0.0, so its swim preset is the standing one.
    #[test]
    fn a_model_that_cannot_swim_keeps_the_standing_preset() {
        let chicken = CameraPivot {
            height_local: 1.2,
            swim_drop_local: 0.0,
        };
        assert_eq!(
            model_pivot_height(&chicken, 1.0, true),
            model_pivot_height(&chicken, 1.0, false),
        );
        assert_eq!(model_pivot_height(&chicken, 1.0, true), 1.2);
    }

    /// A press that has travelled `yaw`/`pitch` degrees of camera rotation.
    fn press(yaw_deg: f32, pitch_deg: f32) -> PressGesture {
        PressGesture {
            at: 0.0,
            yaw_travel: yaw_deg.to_radians(),
            pitch_travel: pitch_deg.to_radians(),
        }
    }

    /// Under 200 ms the reference ignores motion (`0x514ae0`'s first arm, `0x514b24`): flick the
    /// cursor onto a mob, click on arrival with the hand still moving, and it selects.
    #[test]
    fn a_fast_click_selects_however_far_the_mouse_swept() {
        // A whole screen's worth of sweep, far past the travel gate.
        let swept = press(90.0, 45.0);
        assert!(
            swept.is_click(0.199),
            "under 200 world, travel is not consulted"
        );
    }

    /// `mousespeed` multiplies the base rate, and its default 1.0 reproduces `LOOK_SENSITIVITY`.
    #[test]
    fn the_sensitivity_slider_is_a_multiplier_over_the_shipped_rate() {
        let d = LookConfig::default();
        assert_eq!(d.yaw_rate(), LOOK_SENSITIVITY);
        assert_eq!(d.pitch_rate(), LOOK_SENSITIVITY);

        let fast = LookConfig {
            sensitivity: 1.5,
            ..Default::default()
        };
        assert_eq!(fast.yaw_rate(), LOOK_SENSITIVITY * 1.5);
        assert_eq!(fast.pitch_rate(), LOOK_SENSITIVITY * 1.5);
        let slow = LookConfig {
            sensitivity: *MOUSE_SPEED_RANGE.start(),
            ..Default::default()
        };
        assert_eq!(slow.yaw_rate(), LOOK_SENSITIVITY * 0.5);
        assert_eq!(slow.pitch_rate(), LOOK_SENSITIVITY * 0.5);
    }

    /// Each move-speed CVar scales its own axis linearly, the reference's law shape
    /// (`Δyaw° = value × Δx / 800`, `Δpitch° = value × Δy / 600`).
    #[test]
    fn each_move_speed_cvar_scales_its_own_axis() {
        let doubled_yaw = LookConfig {
            yaw_speed: 360.0,
            ..Default::default()
        };
        assert_eq!(doubled_yaw.yaw_rate(), LOOK_SENSITIVITY * 2.0);
        assert_eq!(
            doubled_yaw.pitch_rate(),
            LOOK_SENSITIVITY,
            "the yaw CVar must not move the pitch axis"
        );

        let doubled_pitch = LookConfig {
            pitch_speed: 180.0,
            ..Default::default()
        };
        assert_eq!(doubled_pitch.pitch_rate(), LOOK_SENSITIVITY * 2.0);
        assert_eq!(doubled_pitch.yaw_rate(), LOOK_SENSITIVITY);

        // The MOUSE_LOOK_SPEED slider's ends (90, 270), compared approximately: the two sides
        // multiply in a different order, and f32 multiplication is not associative.
        let near = |a: f32, b: f32| (a - b).abs() < 1e-9;
        for (speed, factor) in [(90.0_f32, 0.5_f32), (270.0, 1.5)] {
            let at = LookConfig {
                yaw_speed: speed,
                ..Default::default()
            };
            assert!(near(at.yaw_rate(), LOOK_SENSITIVITY * factor));
        }

        let both = LookConfig {
            yaw_speed: 270.0,
            sensitivity: 1.5,
            ..Default::default()
        };
        assert!(near(both.yaw_rate(), LOOK_SENSITIVITY * 1.5 * 1.5));
    }

    /// The auto-follow (`0x510960`, `0x50f160`): edge-armed, then unattended, a cosine over
    /// `|Δ| / rate × factor` clamped to [0.1 s, 2.0 s]; Smart's `Idle` and `Stop` rows cancel.
    #[test]
    fn the_auto_follow_is_armed_by_an_input_edge_and_eases_home() {
        const DT: f32 = 1.0 / 120.0;
        // A quarter turn of orbit offset, left there by a drag.
        const OFFSET: f32 = std::f32::consts::FRAC_PI_2;

        fn cfg(style: FollowStyle) -> FollowConfig {
            FollowConfig {
                style,
                tracking_style: style,
                yaw_speed: FOLLOW_SPEED_DEFAULT,
            }
        }
        /// Run `secs` of frames at a fixed input word; returns the camera yaw it ends on.
        fn run(rig: &mut FollowRig, cfg: FollowConfig, word: u32, cam_yaw: f32, secs: f32) -> f32 {
            let mut yaw = cam_yaw;
            for _ in 0..((secs / DT).round() as i32).max(0) {
                let input = FollowInput {
                    cfg,
                    face_yaw: 0.0,
                    command: word,
                };
                if let Some(y) = rig.advance(&input, yaw, DT, false) {
                    yaw = y;
                }
            }
            yaw
        }

        // ── Smart: standing still nothing happens; the W edge returns it in |Δ|/180°/s = 0.5 s.
        let mut rig = FollowRig::default();
        let c = cfg(FollowStyle::Smart);
        let parked = run(&mut rig, c, 0, OFFSET, 1.0);
        assert_eq!(
            parked, OFFSET,
            "Smart standing still leaves the camera alone"
        );
        let half = run(&mut rig, c, follow_cmd::FORWARD, parked, 0.25);
        assert!(
            half < OFFSET * 0.75 && half > OFFSET * 0.25,
            "mid-swing, eased: {half}"
        );
        let home = run(&mut rig, c, follow_cmd::FORWARD, half, 0.3);
        assert!(home.abs() < 1.0e-4, "arrived behind the character: {home}");
        let still_home = run(&mut rig, c, follow_cmd::FORWARD, home + 0.4, 1.0);
        assert_eq!(
            still_home,
            home + 0.4,
            "a HELD key re-arms nothing: only edges arm"
        );

        // ── Smart: releasing W (an edge into Stop) cancels, so a nudge while stopping stays.
        let mut rig = FollowRig::default();
        let held = run(&mut rig, c, follow_cmd::FORWARD, 0.0, 0.1);
        let released = run(&mut rig, c, 0, held + OFFSET, 0.5);
        assert_eq!(released, held + OFFSET, "Stop is a cancel under Smart");

        // ── Always arms on that same Idle edge, the one row where the two styles differ.
        let mut rig = FollowRig::default();
        let a = cfg(FollowStyle::Always);
        let held = run(&mut rig, a, follow_cmd::FORWARD, 0.0, 0.1);
        let returned = run(&mut rig, a, 0, held + OFFSET, 1.0);
        assert!(
            returned.abs() < 1.0e-4,
            "Always returns even from a standstill: {returned}"
        );

        // ── Never is inert, edge or no edge.
        let mut rig = FollowRig::default();
        let n = cfg(FollowStyle::Never);
        let _ = run(&mut rig, n, 0, OFFSET, 0.1);
        assert_eq!(
            run(&mut rig, n, follow_cmd::FORWARD, OFFSET, 2.0),
            OFFSET,
            "Never never arms"
        );

        // ── The duration floor: 5° at 180°/s is 0.028 s, but the 0.1 s minimum holds.
        let mut rig = FollowRig::default();
        let small = 5.0_f32.to_radians();
        let _ = run(&mut rig, c, 0, small, DT);
        let mid = run(&mut rig, c, follow_cmd::FORWARD, small, 0.05);
        assert!(
            mid.abs() > 1.0e-4,
            "the 0.1 s floor is doing the work: {mid}"
        );
        assert!(run(&mut rig, c, follow_cmd::FORWARD, mid, 0.06).abs() < 1.0e-4);

        // ── Track: Smart waits 0.4 s, then a factor-10 return the 2 s ceiling caps.
        let mut rig = FollowRig::default();
        let _ = run(&mut rig, c, 0, OFFSET, DT);
        let delayed = run(&mut rig, c, follow_cmd::TRACK, OFFSET, 0.3);
        assert_eq!(delayed, OFFSET, "nothing moves inside the 0.4 s delay");
        let crawling = run(&mut rig, c, follow_cmd::TRACK, delayed, 0.6);
        assert!(
            crawling > OFFSET * 0.5,
            "a factor-10 return is a crawl, not a swing: {crawling}"
        );
        assert!(
            run(&mut rig, c, follow_cmd::TRACK, crawling, 2.0).abs() < 1.0e-4,
            "and it does arrive, inside the 2 s cap"
        );

        // ── A held drag freezes the channel outright: the hand owns the camera.
        let mut rig = FollowRig::default();
        let input = FollowInput {
            cfg: c,
            face_yaw: 0.0,
            command: follow_cmd::FORWARD,
        };
        assert!(rig.advance(&input, OFFSET, DT, true).is_none());
        assert!(rig.advance(&input, OFFSET, DT, true).is_none());

        // ── Entering the drag cancels the swing in flight (`0x50fe30`) until the next edge.
        let mut rig = FollowRig::default();
        let _ = run(&mut rig, c, 0, OFFSET, DT);
        let mid = run(&mut rig, c, follow_cmd::FORWARD, OFFSET, 0.1);
        assert!(mid < OFFSET && mid > 0.0, "mid-swing: {mid}");
        let dragging = FollowInput {
            cfg: c,
            face_yaw: 0.0,
            command: follow_cmd::FORWARD | follow_cmd::LEFT_MOUSE,
        };
        assert!(rig.advance(&dragging, mid, DT, true).is_none());
        // The word is unchanged from the drag frame's, so nothing re-arms on its own…
        let parked = {
            let mut yaw = mid;
            for _ in 0..120 {
                let input = FollowInput {
                    cfg: c,
                    face_yaw: 0.0,
                    command: follow_cmd::FORWARD | follow_cmd::LEFT_MOUSE,
                };
                if let Some(y) = rig.advance(&input, yaw, DT, false) {
                    yaw = y;
                }
            }
            yaw
        };
        assert_eq!(parked, mid, "the cancelled transition does not resume");
        assert!(
            run(&mut rig, c, follow_cmd::FORWARD, parked, 1.0).abs() < 1.0e-4,
            "the release edge arms a fresh return"
        );
    }

    /// A right-click the UI took (a Who-list row) sets no mouse bit in the camera's command word,
    /// so it cannot arm Smart's `Turn` row, an immediate return; pinned from the latch to the row.
    #[test]
    fn a_press_the_ui_ate_never_reaches_the_camera_command_word() {
        let word = |world_press: bool| {
            let mut rig = CameraControl::default();
            let mut buttons = ButtonInput::<MouseButton>::default();
            buttons.press(MouseButton::Right);
            rig.world_mouse.update(&buttons, world_press);
            super::super::input::look_input(
                &crate::bindings::BindingsState::default(),
                &super::super::Player::default(),
                &rig,
            )
            .follow_command
        };
        let classify = |command| {
            FollowInput {
                cfg: FollowConfig::default(),
                face_yaw: 0.0,
                command,
            }
            .state(false)
        };

        // The press landed on a UI row.
        assert_eq!(word(false), 0, "a captured press sets no mouse bit");
        assert_eq!(classify(word(false)), FollowState::Idle);
        assert_eq!(
            FollowStyle::Smart.row(FollowState::Idle),
            (0.0, 0.0),
            "and Idle is the row that arms nothing — the swing has no source"
        );

        // The control that must not change: the same press in the world still turns.
        assert_eq!(word(true), follow_cmd::RIGHT_MOUSE);
        assert_eq!(classify(word(true)), FollowState::Turn);
        assert_eq!(FollowStyle::Smart.row(FollowState::Turn), (0.0, 1.0));
    }

    /// A button is claimed at its down edge and held to its own release, so the UI arriving under
    /// the locked cursor cannot drop it; a chord's second button joins the held gesture.
    #[test]
    fn the_world_holds_a_button_from_its_press_to_its_release() {
        let mut rig = CameraControl::default();
        let mut buttons = ButtonInput::<MouseButton>::default();

        // Pressed over a UI row: never claimed, and no amount of later frames claims it.
        buttons.press(MouseButton::Right);
        rig.world_mouse.update(&buttons, false);
        assert!(!rig.world_mouse.held(LookButton::Right));
        buttons.clear();
        rig.world_mouse.update(&buttons, true);
        assert!(
            !rig.world_mouse.held(LookButton::Right),
            "a press the UI ate is never handed back mid-hold"
        );
        buttons.release(MouseButton::Right);
        buttons.clear();

        // Pressed in the world: claimed, and it survives the UI arriving under the locked cursor.
        buttons.press(MouseButton::Right);
        rig.world_mouse.update(&buttons, true);
        assert!(rig.world_mouse.down(LookButton::Right));
        buttons.clear();
        rig.world_mouse.update(&buttons, false);
        assert!(rig.world_mouse.held(LookButton::Right));
        assert!(
            !rig.world_mouse.down(LookButton::Right),
            "the edge is one frame"
        );

        // The chord's second button joins the gesture the world already holds…
        buttons.press(MouseButton::Left);
        rig.world_mouse.update(&buttons, true);
        assert!(
            rig.world_mouse.both(),
            "both primaries = the both-button run"
        );

        // …and the release ends it, including a cover's synthetic one (emptied button planes).
        buttons = ButtonInput::<MouseButton>::default();
        rig.world_mouse.update(&buttons, false);
        assert!(!rig.world_mouse.both());
        assert!(!rig.world_mouse.held(LookButton::Right));
        assert!(!rig.world_mouse.held(LookButton::Left));
    }

    /// A right mouse-down in the world ends repair mode (`0x492c68`), from the latch through the
    /// press edge to the reset; the same press on a UI frame, or one that drops a held payload
    /// (`0x492b50`), leaves it standing.
    #[test]
    fn a_right_press_in_the_world_ends_repair_mode_and_one_on_the_ui_does_not() {
        use benilla_ui::script::{MerchantState, UiScript};
        use bevy::ecs::system::RunSystemOnce;
        use bevy::math::DVec2;

        // One right press with repair mode armed; returns whether repair mode survived it.
        let press = |over_ui: bool, payload_held: bool| {
            let mut world = World::new();
            let mut buttons = ButtonInput::<MouseButton>::default();
            buttons.press(MouseButton::Right);
            world.insert_resource(buttons);
            world.insert_resource(crate::ui_script::PointerOverUi(over_ui));
            world.insert_resource(crate::ui_script::CursorPayloadHeld(payload_held));
            world.init_resource::<CameraControl>();
            world.init_resource::<Messages<WorldRightPress>>();
            world.spawn((
                Camera::default(),
                FlyCam {
                    yaw: 0.0,
                    pitch: 0.0,
                    speed: 0.0,
                },
            ));
            let mut window = Window::default();
            window.set_physical_cursor_position(Some(DVec2::new(100.0, 100.0)));
            world.spawn((window, PrimaryWindow));
            let mut script = UiScript::new().unwrap();
            script.set_merchant(Some(MerchantState {
                can_repair: true,
                ..MerchantState::default()
            }));
            script.run("ShowRepairCursor()").unwrap();
            assert!(script.repair_mode(), "the repair vendor arms repair mode");
            world.insert_non_send_resource(script);

            world.run_system_once(latch_world_mouse).unwrap();
            world.run_system_once(send_world_right_press).unwrap();
            world
                .run_system_once(crate::ui_merchant::end_repair_mode_on_right_press)
                .unwrap();
            world.non_send_resource::<UiScript>().repair_mode()
        };

        assert!(
            !press(false, false),
            "a right press in the world ends repair mode"
        );
        assert!(
            press(true, false),
            "a right press on a UI frame never reaches the world's hook"
        );
        assert!(
            press(false, true),
            "a press that drops a held payload is consumed before the hook"
        );
    }

    /// The classifier (`0x510960`) reads the camera's command word, not the character's velocity,
    /// and its priority order decides when several states hold.
    #[test]
    fn the_follow_state_reads_the_camera_input_word_not_the_character() {
        let state = |command: u32, stopping: bool| {
            FollowInput {
                cfg: FollowConfig::default(),
                face_yaw: 0.0,
                command,
            }
            .state(stopping)
        };
        assert_eq!(state(0, false), FollowState::Idle);
        assert_eq!(state(0, true), FollowState::Stop);
        assert_eq!(state(follow_cmd::RIGHT_MOUSE, false), FollowState::Turn);
        assert_eq!(
            state(follow_cmd::RIGHT_MOUSE | follow_cmd::TURN_LEFT, false),
            FollowState::Turn,
            "Turn outranks the Strafe its own condition also satisfies"
        );
        assert_eq!(state(follow_cmd::STRAFE_LEFT, false), FollowState::Strafe);
        assert_eq!(
            state(follow_cmd::RIGHT_MOUSE | follow_cmd::LEFT_MOUSE, false),
            FollowState::Turn,
            "both buttons satisfy Move, but Turn outranks it"
        );
        assert_eq!(state(follow_cmd::LEFT_MOUSE, false), FollowState::Idle);
        assert_eq!(state(follow_cmd::AUTORUN, false), FollowState::Move);
        assert_eq!(state(follow_cmd::TRACK, false), FollowState::Track);
        assert_eq!(
            state(follow_cmd::TRACK | follow_cmd::FORWARD, false),
            FollowState::Move,
            "Move outranks Track"
        );
        assert_eq!(
            state(follow_cmd::FEAR | follow_cmd::FORWARD, false),
            FollowState::Fear,
            "Fear outranks everything"
        );
        // …and the tracking style selects whenever Track or Fear is present.
        let mixed = FollowInput {
            cfg: FollowConfig {
                style: FollowStyle::Never,
                tracking_style: FollowStyle::Always,
                yaw_speed: FOLLOW_SPEED_DEFAULT,
            },
            face_yaw: 0.0,
            command: follow_cmd::TRACK | follow_cmd::FORWARD,
        };
        assert_eq!(mixed.style(), FollowStyle::Always);
        assert_eq!(mixed.state(false), FollowState::Move);
    }

    /// The engine's 0/1/2, which the stock dropdown writes; the validator's 3 reads as Never.
    #[test]
    fn the_follow_style_enum_is_the_engines_and_tolerates_the_dropdowns_stray() {
        assert_eq!(FollowStyle::from_cvar(0.0), FollowStyle::Never);
        assert_eq!(FollowStyle::from_cvar(1.0), FollowStyle::Smart);
        assert_eq!(FollowStyle::from_cvar(2.0), FollowStyle::Always);
        assert_eq!(FollowStyle::from_cvar(3.0), FollowStyle::Never);
        // Off the ladder: the default, Smart.
        assert_eq!(FollowStyle::from_cvar(-1.0), FollowStyle::Smart);
        assert_eq!(FollowStyle::from_cvar(9.0), FollowStyle::Smart);
        for style in [FollowStyle::Never, FollowStyle::Smart, FollowStyle::Always] {
            assert_eq!(
                FollowStyle::from_cvar(style.cvar().parse::<f32>().unwrap()),
                style,
                "the string round-trips"
            );
        }
        assert_eq!(FollowStyle::default(), FollowStyle::Smart);
    }

    /// Between 200 and 800 ms the travel gate applies per axis (`0x514ae0`'s `< 8.0` arm), at 2.25°
    /// of yaw and 2.0° of pitch ([`CLICK_HOLD_CEILING`]).
    #[test]
    fn between_the_windows_a_steady_hand_still_clicks_but_a_drag_does_not() {
        assert!(press(2.0, 1.5).is_click(0.5), "inside both travel gates");
        assert!(
            !press(2.3, 1.5).is_click(0.5),
            "yaw alone spends the budget"
        );
        assert!(!press(2.0, 2.1).is_click(0.5), "pitch alone spends it too");
        // Exactly at a threshold is a drag: the reference's compare is `< 8.0`, not `<=`.
        assert!(!press(2.25, 0.0).is_click(0.5), "the yaw gate is exclusive");
        assert!(
            !press(0.0, 2.0).is_click(0.5),
            "the pitch gate is exclusive"
        );
    }

    /// The 800 ms ceiling (`0x514aeb lea eax,[edx-0x320]`) is absolute: a long hold is never a
    /// click, so a deliberate orbit does not retarget what it started on.
    #[test]
    fn a_long_hold_is_never_a_click_however_still() {
        let motionless = press(0.0, 0.0);
        assert!(motionless.is_click(0.799), "just inside the ceiling");
        assert!(!motionless.is_click(0.8), "the ceiling is exclusive");
        assert!(
            !motionless.is_click(5.0),
            "a long motionless hold is a drag"
        );
    }

    /// The self fade reaches the avatar's billboard cards (the night-elf eye glow) through their
    /// anchors, and only its own: every brazier in the zone is a card too.
    #[test]
    fn self_fade_reaches_the_avatars_billboard_cards_and_no_others() {
        let info = BillboardInfo {
            bone: 0,
            pivot: Vec3::new(0.0, 2.14, 0.0), // the eye-glow bone, head height
            kind: BillboardKind::Spherical,
            scale_anim: None,
            seq_translations: vec![],
        };
        let mut app = App::new();
        app.init_resource::<CameraControl>();
        app.init_resource::<benilla_world::interior::InteriorReauthor>();
        // The water-plane twin map the feather composes with, empty in a fixture.
        app.init_resource::<benilla_world::model_render::FarSideTwins>();
        app.add_systems(Update, apply_self_model_fade);

        // The avatar: root -> joint (the eye-glow bone). Its card follows the joint.
        let avatar = app.world_mut().spawn(Embodied).id();
        let joint = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().entity_mut(avatar).add_child(joint);
        let eye_glow = app
            .world_mut()
            .spawn((
                BillboardCard::following_joint(&info, joint),
                MeshTag(alpha_bits(1.0)),
            ))
            .id();
        // A brazier across the square: same mechanism, another model entirely.
        let brazier_anchor = app.world_mut().spawn(Transform::default()).id();
        let brazier = app
            .world_mut()
            .spawn((
                BillboardCard::following(&info, brazier_anchor),
                MeshTag(alpha_bits(1.0)),
            ))
            .id();

        let tag_of = |app: &App, e: Entity| app.world().entity(e).get::<MeshTag>().unwrap().0;

        app.world_mut()
            .resource_mut::<CameraControl>()
            .self_fade_alpha = 0.5;
        app.update();
        assert_eq!(
            tag_of(&app, eye_glow),
            alpha_bits(0.5),
            "the avatar's card feathers with the body"
        );
        assert_eq!(
            tag_of(&app, brazier),
            alpha_bits(1.0),
            "another model's card is untouched by the player's zoom"
        );

        // First person: the additive compose (`out_rgb *= faded_alpha`) takes it to black.
        app.world_mut()
            .resource_mut::<CameraControl>()
            .self_fade_alpha = 0.0;
        app.update();
        assert_eq!(tag_of(&app, eye_glow), alpha_bits(0.0));

        app.world_mut()
            .resource_mut::<CameraControl>()
            .self_fade_alpha = 1.0;
        app.update();
        assert_eq!(
            tag_of(&app, eye_glow),
            alpha_bits(1.0),
            "the release edge restores the card, like the body parts"
        );
    }
}
