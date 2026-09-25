//! The mechanisms behind the four `UIOptionsFrame` camera checkboxes (`UIOptionsFrame.lua:32-35`).
//!
//! The reference's pitch is positive downward (`0x511f40`, `0x50de00`), benilla's
//! [`super::camera::FlyCam::pitch`] positive upward, so every reference comparison is flipped.

use bevy::prelude::*;

use super::camera_channel::{Arm, SmoothChannel, CHANNEL_EPS};

/// `cameraPivotDXMax` default (`[0xbe0f30]`): the yaw per motion event a vertical drag stays under.
pub(crate) const PIVOT_DX_MAX_DEFAULT: f32 = 0.05;
/// `cameraPivotDYMin` default (`[0xbe0cec]`): the pitch per motion event a pivot drag exceeds.
pub(crate) const PIVOT_DY_MIN_DEFAULT: f32 = 0.0;
/// `cameraTargetSmoothSpeed` default, deg/s (`[0xbe0fc8]`): the rate the pivot bias eases home at.
pub(crate) const TARGET_SMOOTH_SPEED_DEFAULT: f32 = 90.0;

/// `cameraGroundSmoothSpeed` default, deg/s (`[0xbe0fc0]`).
pub(crate) const GROUND_SMOOTH_SPEED_DEFAULT: f32 = 7.5;
/// `cameraTerrainTiltTimeMin` default, seconds (`[0xbe1050]`).
pub(crate) const TILT_TIME_MIN_DEFAULT: f32 = 3.0;
/// `cameraTerrainTiltTimeMax` default, seconds (`[0xbe1054]`).
pub(crate) const TILT_TIME_MAX_DEFAULT: f32 = 10.0;

/// `cameraBobbingLRAmplitude`/`UDAmplitude` default, inches (`[0xbe1064]`, `[0xbe10c8]`).
pub(crate) const BOB_AMPLITUDE_DEFAULT: f32 = 2.0;
/// `cameraBobbingFrequency` default, Hz at a unit speed factor (`[0xbe0cf8]`).
pub(crate) const BOB_FREQUENCY_DEFAULT: f32 = 0.8;
/// `cameraBobbingSmoothSpeed` default (`[0xbe10bc]`): the decay rate, read only by the disarm.
pub(crate) const BOB_SMOOTH_SPEED_DEFAULT: f32 = 0.8;
/// Inches to yards for the amplitude CVars (`[0x7ff9d0]`).
const BOB_AMPLITUDE_SCALE: f32 = 1.0 / 36.0;
/// The speed the bob's rate factor divides by, yd/s (`[0x808a08]`).
const BOB_SPEED_DIVISOR: f32 = 7.2;
/// The rate factor's clamp, `[0x8089a0]` and `[0x80899c]`: the lower bound is the higher address.
const BOB_SPEED_CLAMP: (f32, f32) = (0.5, 1.5);
/// Head bob's inclusive first-person test, on the zoom distance (`[cam+0xec] <= [0x8089ac]`).
pub(crate) const BOB_FIRST_PERSON_DISTANCE: f32 = 1.0 / 6.0;

/// The camera option CVars; each default is the reference's registered value.
#[derive(Resource, Clone, Copy, Debug)]
pub(crate) struct CameraOptions {
    pub(crate) pivot: bool,
    pub(crate) pivot_dx_max: f32,
    pub(crate) pivot_dy_min: f32,
    pub(crate) target_smooth_speed: f32,
    pub(crate) terrain_tilt: bool,
    pub(crate) ground_smooth_speed: f32,
    /// `cameraTerrainTiltTimeMin`/`Max`, seconds, each scaled by the matrix row's `Factor`.
    pub(crate) tilt_time_min: f32,
    pub(crate) tilt_time_max: f32,
    pub(crate) bobbing: bool,
    pub(crate) bob_lr_amplitude: f32,
    pub(crate) bob_ud_amplitude: f32,
    pub(crate) bob_frequency: f32,
    pub(crate) bob_smooth_speed: f32,
    /// `cameraWaterCollision`: one read (`0x50e5ec`) adds liquid to the sweeps' trace mask and
    /// admits the pivot floor and cap block (`0x50e629`, [`super::camera_water`]). Neither works
    /// alone: that block lifts the sweep origin to `surface + 2/9`, clear of the solid waterline.
    pub(crate) water_collision: bool,
}

/// Applies a camera option CVar change. The numeric rows take any value, where the reference's
/// `0x50b330` refuses an out-of-range smooth speed, tilt time or bob frequency.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut opts: ResMut<CameraOptions>) {
    let v = ev.num();
    match ev.key().as_str() {
        "camerapivot" => opts.pivot = v != 0.0,
        "camerawatercollision" => opts.water_collision = v != 0.0,
        "camerapivotdxmax" => opts.pivot_dx_max = v,
        "camerapivotdymin" => opts.pivot_dy_min = v,
        "cameratargetsmoothspeed" => opts.target_smooth_speed = v,
        "cameraterraintilt" => opts.terrain_tilt = v != 0.0,
        "cameragroundsmoothspeed" => opts.ground_smooth_speed = v,
        "cameraterraintilttimemin" => opts.tilt_time_min = v,
        "cameraterraintilttimemax" => opts.tilt_time_max = v,
        "camerabobbing" => opts.bobbing = v != 0.0,
        "camerabobbinglramplitude" => opts.bob_lr_amplitude = v,
        "camerabobbingudamplitude" => opts.bob_ud_amplitude = v,
        "camerabobbingfrequency" => opts.bob_frequency = v,
        "camerabobbingsmoothspeed" => opts.bob_smooth_speed = v,
        _ => {}
    }
}

impl Default for CameraOptions {
    fn default() -> Self {
        Self {
            pivot: true,
            water_collision: true,
            pivot_dx_max: PIVOT_DX_MAX_DEFAULT,
            pivot_dy_min: PIVOT_DY_MIN_DEFAULT,
            target_smooth_speed: TARGET_SMOOTH_SPEED_DEFAULT,
            terrain_tilt: false,
            ground_smooth_speed: GROUND_SMOOTH_SPEED_DEFAULT,
            tilt_time_min: TILT_TIME_MIN_DEFAULT,
            tilt_time_max: TILT_TIME_MAX_DEFAULT,
            bobbing: false,
            bob_lr_amplitude: BOB_AMPLITUDE_DEFAULT,
            bob_ud_amplitude: BOB_AMPLITUDE_DEFAULT,
            bob_frequency: BOB_FREQUENCY_DEFAULT,
            bob_smooth_speed: BOB_SMOOTH_SPEED_DEFAULT,
        }
    }
}

/// One frame's camera-dynamics inputs: the options, and the followed unit's facts the gates read.
#[derive(Clone, Copy, Debug)]
pub(super) struct DynamicsInput {
    pub(super) options: CameraOptions,
    /// Raw `cameraSmoothStyle`, which the tilt matrix indexes (`0x50dbc0`), not the follow's copy.
    pub(super) smooth_style: super::camera::FollowStyle,
    /// `cameraSmoothTrackingStyle`, read only by the pivot release's third conjunct (`0x511010`).
    pub(super) tracking_style: super::camera::FollowStyle,
    pub(super) subject: SubjectState,
    pub(super) nearclip: f32,
    /// The liquid surface over our own feet (Bevy Y), cached by the last movement tick. Unused
    /// under far sight: the reference reads the watched unit's liquid (`0x511ad0`), absent here.
    pub(super) surface_y: Option<f32>,
}

/// The followed unit's side of the gates. The movement word is last frame's, as the reference's
/// input handler reads it (`0x50fee0`'s caller `0x514446` runs before the mover lookup).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SubjectState {
    /// `MOVEMENTFLAGS`, whole: the mechanisms read different masks of it.
    pub(super) move_flags: u32,
    /// `UNIT_FIELD_FLAGS & 0x100000`, taxi flight.
    pub(super) taxi: bool,
    /// The camera's `Track` bit (`[cam+0x90] & 0x100`): externally driven movement.
    pub(super) track: bool,
    /// The camera's `Fear` bit (`[cam+0x90] & 0x1000`): external control.
    pub(super) fear: bool,
    /// Facing, radians, in benilla's yaw convention (`[obj+0xc98]`).
    pub(super) facing: f32,
    /// `UNIT_FIELD_MOUNTDISPLAYID > 0`.
    pub(super) mounted: bool,
    /// Current speed, yd/s (`GetCurrentSpeed`, `0x7c4c90`).
    pub(super) speed: f32,
    /// The input-command word ([`super::camera::follow_cmd`]); its edges arm head bob (`0x511170`).
    pub(super) command: u32,
    /// The aura-76 FOV lock of the spyglass scope (`[cam+0x90] & 0x8`).
    pub(super) scoped: bool,
}

impl SubjectState {
    /// `MOVEMENTFLAGS & 0xf`, move and strafe: turning in place is not translating (`0x5106b7`).
    pub(super) fn translating(&self) -> bool {
        self.move_flags & (mf::FORWARD | mf::BACKWARD | mf::STRAFE_LEFT | mf::STRAFE_RIGHT) != 0
    }
}

use crate::creature_anim::move_flags as mf;

/// `cameraPivot`, smart pivot: a camera the collision solver pinned tilts its view, through a bias
/// channel (`[cam+0x104]`), instead of swinging its arm. The gate `0x510690` returns the solver's
/// clip flags (`[cam+0x90] & 0x30000`), so only a camera clipped this frame pivots. The eye is
/// seated from the unbiased pitch (`0x50ee32`-`0x50ee58`); the body's aim is the sum (`0x5103e0`).
///
/// The release's `0x511010` holds the bias while the Track latch is up and tracking is Never, or
/// a move-type-0 click-to-move order is in flight (`[0xc4d888]`); benilla has no click-to-move,
/// so only the first is built.
pub(super) struct SmartPivot {
    /// The bias channel `[cam+0x104]`; its armed bit is the reference's `[cam+0x90] & 0x8000000`.
    bias: SmoothChannel,
}

impl Default for SmartPivot {
    fn default() -> Self {
        Self {
            bias: SmoothChannel::angular(),
        }
    }
}

impl SmartPivot {
    /// The bias, radians, positive up: part of the view and the body's aim, never the seat's.
    pub(super) fn bias(&self) -> f32 {
        self.bias.live()
    }

    /// `0x50fee0`'s pitch routing: the integrator's delta, or `None` on a pure-pivot frame.
    ///
    /// | reference | here (sign flipped) |
    /// |---|---|
    /// | `[cam+0xf4] < 0` (looking up) | `pitch > 0` |
    /// | floor `bias ≥ −89° − f4` | ceiling `bias ≤ +89° − pitch` |
    /// | pure pivot: `bias ≤ 0 ∧ f4 < 0` | `bias ≥ 0 ∧ pitch > 0` |
    pub(super) fn route_pitch(
        &mut self,
        d_pitch: f32,
        d_yaw: f32,
        pitch: f32,
        subject: &SubjectState,
        clipped: bool,
        cfg: &CameraOptions,
    ) -> Option<f32> {
        // `0x510690`'s conjuncts 3-6; 1 and 2, a UNIT or PLAYER subject, always hold here.
        let verdict = cfg.pivot && !subject.translating() && pitch >= 0.0 && clipped;
        // A displaced bias with no ease armed keeps routing; a gated vertical drag starts it.
        let displaced = !self.bias.in_flight() && self.bias().abs() >= CHANNEL_EPS;
        let vertical_drag =
            verdict && d_pitch.abs() > cfg.pivot_dy_min && d_yaw.abs() < cfg.pivot_dx_max;
        if displaced || vertical_drag {
            let mut bias = self.bias() + d_pitch;
            // One-sided: only while looking up, and only toward the pitch limit.
            if pitch > 0.0 {
                bias = bias.min(super::camera::CAM_PITCH_LIMIT - pitch);
            }
            self.bias.snap(bias);
            if bias >= 0.0 && pitch > 0.0 {
                // The pure-pivot frame (`0x5100c3`): the integrator does not run.
                return None;
            }
        }
        // The ordinary path (`0x51009a`). After a routed delta it lands in both, as in the
        // reference, and the ease walks it off the bias.
        self.release(cfg);
        Some(d_pitch)
    }

    /// Per frame (`0x50ed77` → `0x5107f0`): a false gate eases a displaced bias home, a true one
    /// holds it.
    pub(super) fn advance(
        &mut self,
        pitch: f32,
        subject: &SubjectState,
        clipped: bool,
        tracking_style: super::camera::FollowStyle,
        cfg: &CameraOptions,
        dt: f32,
    ) {
        let verdict = cfg.pivot && !subject.translating() && pitch >= 0.0 && clipped;
        // `0x511010`'s first disjunct: tracking at Never holds the bias as the gate does.
        let tracking_hold = subject.track && tracking_style == super::camera::FollowStyle::Never;
        let holding = verdict || tracking_hold;
        if !holding && self.bias().abs() >= CHANNEL_EPS {
            self.release(cfg);
        } else if holding {
            // `0x50ed93`: a held gate snapshots the bias and disarms, cancelling an ease in flight.
            let live = self.bias();
            self.bias.snap(live);
        }
        self.bias.advance(dt);
    }

    /// Arms the bias home at `cameraTargetSmoothSpeed` (`0x512a50(cam, 0, 0, 1.0f, now)`).
    fn release(&mut self, cfg: &CameraOptions) {
        self.bias
            .arm(&Arm::at(0.0, cfg.target_smooth_speed.to_radians()));
    }
}

/// `cameraTerrainTilt`, follow terrain: the camera pitches toward the slope of the ground ahead.
/// The probe `0x50d900` reads its rise over run every 100 ms (a missed drop is the full downward
/// tilt); the arm `0x50dbc0` eases the ground channel to the stepped angle over
/// `[3, 10] × Factor` seconds, a floor that always binds (20° at 7.5°/s is 2.67 s), and the
/// channel adds to the view pitch inside the ±89° clamp (`0x50f810`).
pub(super) struct TerrainTilt {
    /// The ground channel `+0x108`, at `cameraGroundSmoothSpeed`.
    ground: SmoothChannel,
    /// The probe's output `[cam+0xa0]`, held between runs.
    slope_pitch: f32,
    /// Seconds since the probe ran, for the 100 ms throttle (`0x50d94f`).
    since_probe: f32,
    /// Mouse look holds the lean inside the pitch (the reference's refcount `[cam+0xa8]`).
    handed_off: bool,
}

impl Default for TerrainTilt {
    fn default() -> Self {
        Self {
            ground: SmoothChannel::angular(),
            slope_pitch: 0.0,
            // The probe runs on the first frame.
            since_probe: PROBE_THROTTLE,
            handed_off: false,
        }
    }
}

/// An `{Absorb, Delay, Factor}` row, from `0x84f620`. Deviation: a code table, not the reference's
/// ninety `cameraTerrainTilt<Style><State>` CVars, because the matrix is a law, not a setting.
type TiltRow = (f32, f32, f32);

/// The movement states `0x50dbc0` classifies into (names at `0x84f5e8`), less the reference's
/// `Jump`, which is dead: `0x50dc28` and `0x50dc35` test the same bit, so state 3 is never set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TiltState {
    Fall,
    Fear,
    Idle,
    Move,
    Strafe,
    Swim,
    Taxi,
    Track,
    Turn,
}

impl TiltState {
    /// `0x50dbc0`'s classifier, a priority ladder in its branch order.
    fn of(subject: &SubjectState) -> Self {
        if subject.taxi {
            Self::Taxi
        } else if subject.move_flags & mf::SWIMMING != 0 {
            Self::Swim
        } else if subject.move_flags & mf::FALLING != 0 {
            Self::Fall
        } else if subject.track {
            Self::Track
        } else if subject.fear {
            Self::Fear
        } else if subject.move_flags & (mf::FORWARD | mf::BACKWARD) != 0 {
            Self::Move
        } else if subject.move_flags & (mf::STRAFE_LEFT | mf::STRAFE_RIGHT) != 0 {
            Self::Strafe
        } else if subject.move_flags & (mf::TURN_LEFT | mf::TURN_RIGHT) != 0 {
            Self::Turn
        } else {
            Self::Idle
        }
    }

    /// This state's row at `style`; an `Absorb` of 0 aims the channel at level.
    fn row(self, style: super::camera::FollowStyle) -> TiltRow {
        use super::camera::FollowStyle;
        if style == FollowStyle::Never {
            return (0.0, 0.0, -1.0);
        }
        let always = style == FollowStyle::Always;
        match self {
            Self::Fall => (1.0, 0.0, 0.75),
            Self::Fear => (1.0, 0.0, 1.0),
            Self::Idle if always => (1.0, 0.0, 1.0),
            Self::Idle => (0.0, 0.0, -1.0),
            Self::Move | Self::Strafe | Self::Track | Self::Turn => (1.0, 0.0, 1.0),
            Self::Swim | Self::Taxi => (0.0, 0.0, 1.0),
        }
    }
}

impl TerrainTilt {
    /// The ground's pitch this frame, radians, positive up.
    pub(super) fn pitch(&self) -> f32 {
        // `0x50f809`: while handed off, the lean is already inside the pitch.
        if self.handed_off {
            0.0
        } else {
            self.ground.live()
        }
    }

    /// The mouse-look hand-off (`0x50d500` push, `0x50d520` pop): the delta the pitch owes. The
    /// lean moves into the pitch at press and the then-current lean comes out at release, both
    /// through `0x510120`, which writes the live pitch at once; a slope crossed mid-drag leaves the
    /// difference in the pitch.
    pub(super) fn hand_off(&mut self, freelook: bool) -> f32 {
        if freelook == self.handed_off {
            return 0.0;
        }
        self.handed_off = freelook;
        // The raw channel value: `pitch()` reads zero while handed off.
        let live = self.ground.live();
        if freelook {
            live
        } else {
            -live
        }
    }

    /// The staircase: `0x50db37` walks `0x808a40` down to the first key `|slope|` reaches, with no
    /// interpolation, then clamps to ±20°. Uphill looks up: positive here, negative there.
    pub(super) fn slope_to_pitch(slope: f32) -> f32 {
        // Keys `round(tan(5k°), 2)`; each literal is bit-identical to the table's f32.
        const STEPS: [(f32, f32); 5] = [
            (0.36, 20.0),
            (0.27, 15.0),
            (0.18, 10.0),
            (0.09, 5.0),
            (0.0, 0.0),
        ];
        let magnitude = STEPS
            .iter()
            .find(|(key, _)| slope.abs() >= *key)
            .map_or(0.0, |(_, deg)| *deg)
            .to_radians();
        if slope < 0.0 {
            -magnitude
        } else {
            magnitude
        }
    }

    /// Runs the probe (the ground ahead's rise over run) when the throttle allows, then arms and
    /// steps the ground channel.
    pub(super) fn advance(
        &mut self,
        probe: impl FnOnce() -> f32,
        gate: bool,
        subject: &SubjectState,
        style: super::camera::FollowStyle,
        cfg: &CameraOptions,
        dt: f32,
    ) -> f32 {
        self.since_probe += dt;
        if !gate {
            // A false gate (`0x5105a0`) zeroes the held value (`0x50d922`); the throttle keeps it.
            self.slope_pitch = 0.0;
        } else if self.since_probe >= PROBE_THROTTLE {
            self.since_probe = 0.0;
            self.slope_pitch = Self::slope_to_pitch(probe());
        }
        let (absorb, delay, factor) = TiltState::of(subject).row(style);
        if factor < 0.0 {
            // The disable sentinel (`0x50dcd1`): the channel disarms and keeps the lean it has.
            self.ground.snap(self.ground.live());
        } else {
            let rate = cfg.ground_smooth_speed.to_radians();
            self.ground.arm(&Arm {
                target: absorb * self.slope_pitch,
                delay,
                factor,
                rate,
                duration: Some((cfg.tilt_time_min * factor, cfg.tilt_time_max * factor)),
            });
        }
        self.ground.advance(dt)
    }
}

/// The probe's rerun interval, seconds (`0x50d953`).
const PROBE_THROTTLE: f32 = 0.1;
/// The horizontal ray's reach along the facing, yd (`[0x808a34]`).
pub(super) const PROBE_REACH: f32 = 10.0 / 3.0;
/// How far the horizontal hit is pulled back before the drop, yd (`[0x808ab8]`).
pub(super) const PROBE_BACKOFF: f32 = 5.0 / 18.0;
/// The height over the feet the horizontal ray starts at, yd (`[0x808a38]`); it cancels out.
pub(super) const PROBE_LIFT: f32 = 5.0 / 3.0;
/// The vertical drop's reach, yd (`[0x808ab4]`).
pub(super) const PROBE_DROP: f32 = 64.0 / 9.0;

/// `cameraBobbing`, head bob: a figure-of-eight translation of the eye while moving, first person
/// only. Edges of the input-command word arm a session (`0x511170`) and restart its phase; the
/// disarm eases the offset to an exact zero (`0x511110`, `0x50f160`).
///
/// Deviation: the reference sweeps the camera collision against the bobbed eye (`0x50ed47`);
/// benilla adds the offset after the sweep, because it shares camera shake's accumulator there.
pub(super) struct HeadBob {
    /// A session is armed (`[cam+0x90] & 0x200`).
    armed: bool,
    last_command: Option<u32>,
    /// Seconds since the last arm or disarm (`[cam+0x238]`): the phase, then the decay's clock.
    since_transition: f32,
    offset: Vec3,
    /// The disarm's snapshot (`[cam+0x240..0x248]`) and ramp duration (`[cam+0x23c]`).
    ramp_from: Vec3,
    ramp_duration: f32,
}

impl Default for HeadBob {
    fn default() -> Self {
        Self {
            armed: false,
            last_command: None,
            since_transition: 0.0,
            offset: Vec3::ZERO,
            ramp_from: Vec3::ZERO,
            // The ctor's zero: a decay with no snapshot lands on its exact zero at once.
            ramp_duration: 0.0,
        }
    }
}

impl HeadBob {
    /// The eye offset this frame, world axes; zero when nothing bobs.
    pub(super) fn offset(&self) -> Vec3 {
        self.offset
    }

    /// The arm predicate on the input-command word (`0x5149d5`, `0x514cc1`).
    fn arms(command: u32) -> bool {
        use super::camera::follow_cmd as c;
        const TRANSLATING: u32 =
            c::FORWARD | c::BACKWARD | c::STRAFE_LEFT | c::STRAFE_RIGHT | c::AUTORUN | c::TRACK;
        command & TRANSLATING != 0
            || (command & c::RIGHT_MOUSE != 0
                && command & (c::LEFT_MOUSE | c::TURN_LEFT | c::TURN_RIGHT) != 0)
    }

    /// One frame: the latch, then the kernel or the decay. `zoom` is the wheel's distance.
    pub(super) fn advance(
        &mut self,
        zoom: f32,
        subject: &SubjectState,
        cfg: &CameraOptions,
        dt: f32,
    ) {
        self.since_transition += dt;
        // The latch (`0x511170`): edges of the command word, ungated by `cameraBobbing`.
        let want = Self::arms(subject.command);
        if self.last_command.replace(subject.command) != Some(subject.command) && want != self.armed
        {
            if want {
                self.armed = true;
            } else {
                // The disarm (`0x511110`): the largest component sets the ramp's duration.
                self.armed = false;
                self.ramp_from = self.offset;
                let largest = self
                    .offset
                    .to_array()
                    .into_iter()
                    .fold(0.0_f32, |m, c| m.max(c.abs()));
                self.ramp_duration = largest / cfg.bob_smooth_speed.max(f32::EPSILON);
            }
            self.since_transition = 0.0;
        }
        if self.eligible(zoom, subject, cfg) {
            self.offset = self.kernel(subject, cfg);
        } else if (self.offset.x + self.offset.y + self.offset.z).abs() >= CHANNEL_EPS {
            // `0x5106f0` tests the signed sum's magnitude, not the length.
            let s = self.since_transition / self.ramp_duration.max(f32::EPSILON);
            self.offset = if s >= 1.0 {
                Vec3::ZERO
            } else {
                let e = (1.0 - (std::f32::consts::PI * s).cos()) * 0.5;
                self.ramp_from * (1.0 - e)
            };
        } else {
            self.offset = Vec3::ZERO;
        }
    }

    /// The gate `0x5105e0`, in its order; its UNIT-or-PLAYER conjuncts always hold here.
    fn eligible(&self, zoom: f32, subject: &SubjectState, cfg: &CameraOptions) -> bool {
        zoom <= BOB_FIRST_PERSON_DISTANCE
            && !subject.scoped
            && cfg.bobbing
            && subject.move_flags & mf::SWIMMING == 0
            && subject.move_flags & mf::FALLING == 0
            && !subject.mounted
            && !subject.taxi
            && self.armed
    }

    /// The kernel `0x511920`: lateral at rate `A` and vertical at `2A`, the figure of eight.
    fn kernel(&self, subject: &SubjectState, cfg: &CameraOptions) -> Vec3 {
        let rate = cfg.bob_frequency
            * (subject.speed / BOB_SPEED_DIVISOR).clamp(BOB_SPEED_CLAMP.0, BOB_SPEED_CLAMP.1);
        let phase = std::f32::consts::TAU * rate * self.since_transition;
        let lateral = cfg.bob_lr_amplitude * BOB_AMPLITUDE_SCALE * phase.sin();
        // The reference's `φ + π/2` is the subject's left: the same rotation about Bevy's +Y.
        let left = Quat::from_rotation_y(subject.facing) * Vec3::NEG_X;
        left * lateral
            + Vec3::Y * (cfg.bob_ud_amplitude * BOB_AMPLITUDE_SCALE * (2.0 * phase).sin())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;
    /// A clearly vertical drag: over `cameraPivotDYMin` on pitch, under `cameraPivotDXMax` on yaw.
    const UP: f32 = 0.01;

    fn moving(translating: bool) -> SubjectState {
        SubjectState {
            move_flags: if translating { mf::FORWARD } else { 0 },
            ..SubjectState::default()
        }
    }

    /// The gate `0x510690`: each conjunct removed alone from a case that pivots.
    #[test]
    fn only_a_clipped_camera_looking_level_or_up_and_not_translating_pivots() {
        let cfg = CameraOptions::default();
        let mut p = SmartPivot::default();
        assert_eq!(
            p.route_pitch(UP, 0.0, 0.2, &moving(false), true, &cfg),
            None
        );
        assert!(p.bias() > 0.0, "the drag went into the bias");

        for (name, opts, pitch, subject, clipped) in [
            ("unclipped", cfg, 0.2, moving(false), false),
            ("translating", cfg, 0.2, moving(true), true),
            // Looking down: the reference's `[cam+0xf4] <= 0`, mirrored.
            ("pitched down", cfg, -0.2, moving(false), true),
            (
                "cvar off",
                CameraOptions {
                    pivot: false,
                    ..cfg
                },
                0.2,
                moving(false),
                true,
            ),
        ] {
            let mut p = SmartPivot::default();
            assert_eq!(
                p.route_pitch(UP, 0.0, pitch, &subject, clipped, &opts),
                Some(UP),
                "{name}: the ordinary pitch integrator must take it"
            );
            assert_eq!(p.bias(), 0.0, "{name}: and nothing reached the bias");
        }
    }

    /// The drag-shape test (`0x50fff5`/`0x510004`).
    #[test]
    fn a_mostly_horizontal_drag_is_never_a_pivot() {
        let cfg = CameraOptions::default();
        let mut p = SmartPivot::default();
        let d_yaw = cfg.pivot_dx_max + 0.001;
        assert_eq!(
            p.route_pitch(UP, d_yaw, 0.2, &moving(false), true, &cfg),
            Some(UP)
        );
        assert_eq!(p.bias(), 0.0);
        // The compare is `|dYaw| < DXMax`, so the threshold itself is refused.
        assert_eq!(
            p.route_pitch(UP, cfg.pivot_dx_max, 0.2, &moving(false), true, &cfg),
            Some(UP)
        );
        assert_eq!(p.bias(), 0.0);
        assert_eq!(
            p.route_pitch(
                UP,
                cfg.pivot_dx_max - 0.001,
                0.2,
                &moving(false),
                true,
                &cfg
            ),
            None
        );
        assert!(p.bias() > 0.0);
    }

    /// The one-sided clamp (`0x510065`-`0x510079`) bounds the sum with the pitch, not the bias.
    #[test]
    fn the_bias_cannot_take_the_composite_past_the_pitch_limit() {
        let cfg = CameraOptions::default();
        let mut p = SmartPivot::default();
        let pitch = 0.5_f32;
        for _ in 0..400 {
            p.route_pitch(UP, 0.0, pitch, &moving(false), true, &cfg);
        }
        assert!(
            (p.bias() - (super::super::camera::CAM_PITCH_LIMIT - pitch)).abs() < 1e-5,
            "clamped to 89° − pitch, got {}",
            p.bias()
        );
    }

    /// The reference's `bias > 0 || f4 >= 0` fork (`0x510094`/`0x510045`), mirrored.
    #[test]
    fn dragging_back_down_returns_the_axis_to_the_ordinary_pitch() {
        let cfg = CameraOptions::default();
        let mut p = SmartPivot::default();
        for _ in 0..5 {
            assert_eq!(
                p.route_pitch(UP, 0.0, 0.2, &moving(false), true, &cfg),
                None
            );
        }
        let peak = p.bias();
        assert!(peak > 0.0);
        let mut handed_back = 0;
        for _ in 0..6 {
            if p.route_pitch(-UP, 0.0, 0.2, &moving(false), true, &cfg)
                .is_some()
            {
                handed_back += 1;
            }
        }
        assert!(handed_back > 0, "the ordinary integrator never got it back");
        assert!(p.bias() < peak, "and the bias unwound");
    }

    /// The release (`0x50ed77` → `0x5107f0`).
    #[test]
    fn losing_the_gate_eases_the_bias_home_at_the_target_smooth_speed() {
        let cfg = CameraOptions::default();
        let mut p = SmartPivot::default();
        p.route_pitch(0.3, 0.0, 0.5, &moving(false), true, &cfg);
        let held = p.bias();
        assert!(held > 0.0);
        for _ in 0..120 {
            p.advance(
                0.5,
                &moving(false),
                true,
                super::super::camera::FollowStyle::Smart,
                &cfg,
                DT,
            );
        }
        assert_eq!(p.bias(), held, "a held gate must not move the bias");
        let expected = held / cfg.target_smooth_speed.to_radians();
        let mut took = None;
        for frame in 0..600 {
            p.advance(
                0.5,
                &moving(false),
                false,
                super::super::camera::FollowStyle::Smart,
                &cfg,
                DT,
            );
            if took.is_none() && p.bias().abs() < CHANNEL_EPS {
                took = Some(frame as f32 * DT);
            }
        }
        let took = took.expect("the bias comes home");
        assert!(
            (took - expected).abs() < 0.05,
            "|bias| / cameraTargetSmoothSpeed = {expected:.3}s, took {took:.3}s"
        );
    }

    /// `0x808a40` walked down with no interpolation; the ±20° clamp hides records 5-9.
    #[test]
    fn the_terrain_staircase_is_five_signed_steps_and_nothing_between_them() {
        let deg = |slope: f32| TerrainTilt::slope_to_pitch(slope).to_degrees();
        for (slope, want) in [
            (0.0, 0.0),
            (0.089, 0.0),
            (0.09, 5.0),
            (0.17, 5.0),
            (0.18, 10.0),
            (0.26, 10.0),
            (0.27, 15.0),
            (0.35, 15.0),
            (0.36, 20.0),
            (1.0, 20.0),
            // Records 5-9 would read 25-45°; the clamp makes them 20°.
            (0.47, 20.0),
            (100.0, 20.0),
        ] {
            assert!(
                (deg(slope) - want).abs() < 1e-4,
                "slope {slope} -> {} °, wanted {want}",
                deg(slope)
            );
            assert!(
                (deg(-slope) + want).abs() < 1e-4,
                "slope -{slope} -> {} °, wanted -{want}",
                deg(-slope)
            );
        }
        // Uphill looks up: positive here, negative in the reference.
        assert!(TerrainTilt::slope_to_pitch(0.5) > 0.0);
    }

    #[test]
    fn the_tilt_matrix_disables_smart_idle_and_levels_swim_and_taxi() {
        use super::super::camera::FollowStyle;
        let held = |style, state: TiltState| state.row(style).2 < 0.0;
        for state in [
            TiltState::Fall,
            TiltState::Fear,
            TiltState::Idle,
            TiltState::Move,
            TiltState::Strafe,
            TiltState::Swim,
            TiltState::Taxi,
            TiltState::Track,
            TiltState::Turn,
        ] {
            assert!(held(FollowStyle::Never, state), "Never disables {state:?}");
        }
        assert!(
            held(FollowStyle::Smart, TiltState::Idle),
            "Smart holds Idle"
        );
        assert!(
            !held(FollowStyle::Always, TiltState::Idle),
            "Always is the one row that differs"
        );
        for state in [TiltState::Swim, TiltState::Taxi] {
            assert_eq!(
                state.row(FollowStyle::Smart).0,
                0.0,
                "{state:?} absorbs nothing — it aims the channel at level"
            );
        }
        assert_eq!(TiltState::Fall.row(FollowStyle::Smart).2, 0.75);
    }

    #[test]
    fn the_tilt_state_ladder_takes_the_highest_priority_flag_set() {
        let with = |f: fn(&mut SubjectState)| {
            let mut s = SubjectState::default();
            f(&mut s);
            TiltState::of(&s)
        };
        assert_eq!(with(|_| {}), TiltState::Idle);
        assert_eq!(with(|s| s.move_flags = mf::FORWARD), TiltState::Move);
        assert_eq!(with(|s| s.move_flags = mf::STRAFE_LEFT), TiltState::Strafe);
        assert_eq!(with(|s| s.move_flags = mf::TURN_RIGHT), TiltState::Turn);
        assert_eq!(with(|s| s.move_flags = mf::FALLING), TiltState::Fall);
        assert_eq!(with(|s| s.move_flags = mf::SWIMMING), TiltState::Swim);
        assert_eq!(with(|s| s.track = true), TiltState::Track);
        assert_eq!(with(|s| s.fear = true), TiltState::Fear);
        assert_eq!(with(|s| s.taxi = true), TiltState::Taxi);
        assert_eq!(
            with(|s| {
                s.taxi = true;
                s.move_flags = mf::SWIMMING | mf::FORWARD;
                s.track = true;
            }),
            TiltState::Taxi
        );
        assert_eq!(
            with(|s| s.move_flags = mf::SWIMMING | mf::FALLING | mf::FORWARD),
            TiltState::Swim
        );
        assert_eq!(
            with(|s| s.move_flags = mf::FALLING | mf::FORWARD),
            TiltState::Fall
        );
    }

    #[test]
    fn the_tilt_channel_is_throttled_gated_and_floored_at_three_seconds() {
        let cfg = CameraOptions {
            terrain_tilt: true,
            ..CameraOptions::default()
        };
        use super::super::camera::FollowStyle;
        let moving = SubjectState {
            move_flags: mf::FORWARD,
            ..SubjectState::default()
        };
        let mut t = TerrainTilt::default();
        let mut probes = 0;
        for _ in 0..6 {
            t.advance(
                || {
                    probes += 1;
                    0.5
                },
                true,
                &moving,
                FollowStyle::Smart,
                &cfg,
                1.0 / 60.0,
            );
        }
        assert_eq!(probes, 1, "100 ms is six frames at 60 Hz, so one probe");

        // 20° at 7.5°/s is 2.67 s, under the 3 s `cameraTerrainTiltTimeMin`.
        let mut t = TerrainTilt::default();
        let target = TerrainTilt::slope_to_pitch(0.5);
        let mut took = None;
        for frame in 0..600 {
            let p = t.advance(|| 0.5, true, &moving, FollowStyle::Smart, &cfg, 1.0 / 60.0);
            // Exact: an epsilon passes on the cosine's flat tail a fifth of a second early.
            if took.is_none() && p == target {
                took = Some(frame as f32 / 60.0);
            }
        }
        let took = took.expect("the lean arrives");
        assert!(
            (took - cfg.tilt_time_min).abs() < 0.05,
            "the 3 s floor should bind, took {took:.2}s"
        );
        assert!(target.abs() / cfg.ground_smooth_speed.to_radians() < cfg.tilt_time_min);

        // A false gate zeroes the held value (`0x50d922`).
        let mut zeroed = 0.0_f32;
        for _ in 0..600 {
            zeroed = t.advance(|| 0.5, false, &moving, FollowStyle::Smart, &cfg, 1.0 / 60.0);
        }
        assert!(zeroed.abs() < CHANNEL_EPS, "levels off, at {zeroed}");
    }

    #[test]
    fn head_bob_arms_on_the_movement_command_word_and_on_the_mouse_chord() {
        use super::super::camera::follow_cmd as c;
        for bits in [
            c::FORWARD,
            c::BACKWARD,
            c::STRAFE_LEFT,
            c::STRAFE_RIGHT,
            c::AUTORUN,
            c::TRACK,
            c::RIGHT_MOUSE | c::LEFT_MOUSE,
            c::RIGHT_MOUSE | c::TURN_LEFT,
            c::RIGHT_MOUSE | c::TURN_RIGHT,
        ] {
            assert!(HeadBob::arms(bits), "{bits:#x} should arm");
        }
        for bits in [
            0,
            c::RIGHT_MOUSE,
            c::LEFT_MOUSE,
            c::TURN_LEFT,
            c::TURN_RIGHT,
            c::LEFT_MOUSE | c::TURN_LEFT,
            c::FEAR,
        ] {
            assert!(!HeadBob::arms(bits), "{bits:#x} should not arm");
        }
    }

    #[test]
    fn head_bob_is_first_person_only_and_every_conjunct_can_stop_it() {
        use super::super::camera::follow_cmd as c;
        let cfg = CameraOptions {
            bobbing: true,
            ..CameraOptions::default()
        };
        let running = SubjectState {
            command: c::FORWARD,
            move_flags: mf::FORWARD,
            speed: 7.0,
            ..SubjectState::default()
        };
        let bobs = |zoom: f32, subject: &SubjectState, cfg: &CameraOptions| {
            let mut b = HeadBob::default();
            // One frame for the arming edge, then half a second to leave zero.
            b.advance(zoom, subject, cfg, 1.0 / 60.0);
            for _ in 0..30 {
                b.advance(zoom, subject, cfg, 1.0 / 60.0);
            }
            b.offset() != Vec3::ZERO
        };
        assert!(bobs(0.0, &running, &cfg), "first person, running");
        assert!(
            bobs(BOB_FIRST_PERSON_DISTANCE, &running, &cfg),
            "the 1/6 compare is INCLUSIVE"
        );
        assert!(
            !bobs(BOB_FIRST_PERSON_DISTANCE + 0.001, &running, &cfg),
            "a hair zoomed out is not first person"
        );
        assert!(!bobs(5.0, &running, &cfg), "third person never bobs");
        for (name, subject, cfg) in [
            (
                "cvar off",
                running,
                CameraOptions {
                    bobbing: false,
                    ..cfg
                },
            ),
            (
                "swimming",
                SubjectState {
                    move_flags: mf::FORWARD | mf::SWIMMING,
                    ..running
                },
                cfg,
            ),
            (
                "falling",
                SubjectState {
                    move_flags: mf::FORWARD | mf::FALLING,
                    ..running
                },
                cfg,
            ),
            (
                "mounted",
                SubjectState {
                    mounted: true,
                    ..running
                },
                cfg,
            ),
            (
                "on a taxi",
                SubjectState {
                    taxi: true,
                    ..running
                },
                cfg,
            ),
            (
                "scoped",
                SubjectState {
                    scoped: true,
                    ..running
                },
                cfg,
            ),
            (
                "no movement command",
                SubjectState {
                    command: 0,
                    ..running
                },
                cfg,
            ),
        ] {
            assert!(!bobs(0.0, &subject, &cfg), "{name} must not bob");
        }
    }

    #[test]
    fn the_bob_traces_a_figure_of_eight_at_the_scaled_amplitudes() {
        use super::super::camera::follow_cmd as c;
        let cfg = CameraOptions {
            bobbing: true,
            ..CameraOptions::default()
        };
        let subject = SubjectState {
            command: c::FORWARD,
            move_flags: mf::FORWARD,
            // The divisor itself: a rate factor of 1, so `A` is the frequency.
            speed: BOB_SPEED_DIVISOR,
            facing: 0.0,
            ..SubjectState::default()
        };
        let mut b = HeadBob::default();
        let dt = 1.0 / 600.0;
        b.advance(0.0, &subject, &cfg, dt);
        let (mut peak_lat, mut peak_up) = (0.0_f32, 0.0_f32);
        let (mut lat_crossings, mut up_crossings) = (0_i32, 0_i32);
        let (mut was, period) = (Vec3::ZERO, 1.0 / cfg.bob_frequency);
        for _ in 0..(5.0 * period / dt) as usize {
            b.advance(0.0, &subject, &cfg, dt);
            let o = b.offset();
            // Facing −Z, so the body's left is world −X and the sway has no Z.
            assert_eq!(o.z, 0.0, "the sway is lateral, never fore/aft");
            peak_lat = peak_lat.max(o.x.abs());
            peak_up = peak_up.max(o.y.abs());
            if (was.x < 0.0) != (o.x < 0.0) {
                lat_crossings += 1;
            }
            if (was.y < 0.0) != (o.y < 0.0) {
                up_crossings += 1;
            }
            was = o;
        }
        let amp = BOB_AMPLITUDE_DEFAULT * BOB_AMPLITUDE_SCALE;
        assert!((peak_lat - amp).abs() < 1e-3, "lateral peak {peak_lat}");
        assert!((peak_up - amp).abs() < 1e-3, "vertical peak {peak_up}");
        // Vertical at twice the lateral rate, with one crossing of slack for where the window ends.
        assert!(
            (up_crossings - 2 * lat_crossings).abs() <= 1 && lat_crossings >= 9,
            "vertical {up_crossings} crossings against lateral {lat_crossings}"
        );

        let still = SubjectState {
            speed: 0.0,
            ..subject
        };
        let mut slow = HeadBob::default();
        slow.advance(0.0, &still, &cfg, dt);
        for _ in 0..(period / dt) as usize {
            slow.advance(0.0, &still, &cfg, dt);
        }
        assert!(
            slow.offset() != Vec3::ZERO,
            "the 0.5 floor keeps it running"
        );
    }

    #[test]
    fn releasing_the_key_ramps_the_bob_to_an_exact_zero() {
        use super::super::camera::follow_cmd as c;
        let cfg = CameraOptions {
            bobbing: true,
            ..CameraOptions::default()
        };
        let running = SubjectState {
            command: c::FORWARD,
            move_flags: mf::FORWARD,
            speed: BOB_SPEED_DIVISOR,
            ..SubjectState::default()
        };
        let dt = 1.0 / 600.0;
        let mut b = HeadBob::default();
        b.advance(0.0, &running, &cfg, dt);
        // A quarter second in, the lateral sway near its peak when the key comes up.
        for _ in 0..150 {
            b.advance(0.0, &running, &cfg, dt);
        }
        let held = b.offset();
        assert!(held != Vec3::ZERO);
        let largest = held
            .to_array()
            .into_iter()
            .fold(0.0_f32, |m, c| m.max(c.abs()));
        let expected = largest / cfg.bob_smooth_speed;
        let idle = SubjectState {
            command: 0,
            ..running
        };
        let mut took = None;
        for frame in 0..2000 {
            b.advance(0.0, &idle, &cfg, dt);
            if took.is_none() && b.offset() == Vec3::ZERO {
                took = Some(frame as f32 * dt);
            }
        }
        let took = took.expect("the residual comes home");
        assert!(
            (took - expected).abs() < 0.02,
            "|largest| / cameraBobbingSmoothSpeed = {expected:.3}s, took {took:.3}s"
        );
        assert_eq!(b.offset(), Vec3::ZERO, "and terminates on an exact zero");
    }

    /// `0x50ed93`: a gate back mid-return leaves the view tilted, without a snap.
    #[test]
    fn re_entering_the_gate_cancels_the_return_where_it_stands() {
        let cfg = CameraOptions::default();
        let mut p = SmartPivot::default();
        p.route_pitch(0.3, 0.0, 0.5, &moving(false), true, &cfg);
        for _ in 0..6 {
            p.advance(
                0.5,
                &moving(false),
                false,
                super::super::camera::FollowStyle::Smart,
                &cfg,
                DT,
            );
        }
        let mid = p.bias();
        assert!(mid > CHANNEL_EPS && mid < 0.3, "mid-return, got {mid}");
        for _ in 0..60 {
            p.advance(
                0.5,
                &moving(false),
                true,
                super::super::camera::FollowStyle::Smart,
                &cfg,
                DT,
            );
        }
        assert_eq!(p.bias(), mid, "the return was cancelled, not completed");
    }

    /// The staircase jumps 5° at each key; the channel it arms reaches the camera smoothly.
    #[test]
    fn the_staircases_five_degree_jumps_reach_the_camera_as_a_smooth_lean() {
        use super::super::camera::FollowStyle;
        use super::super::camera_channel::assert_bounded_step;

        assert_bounded_step(
            (-0.6, 0.6),
            0.0005,
            5.0_f32.to_radians() + 1.0e-6,
            TerrainTilt::slope_to_pitch,
        );

        // Walking terrain that sweeps every key in 20 s.
        let cfg = CameraOptions {
            terrain_tilt: true,
            ..CameraOptions::default()
        };
        let moving = SubjectState {
            move_flags: mf::FORWARD,
            ..SubjectState::default()
        };
        let mut tilt = TerrainTilt::default();
        assert_bounded_step((0.0, 20.0), DT, 0.25_f32.to_radians(), |now| {
            let slope = -0.5 + now * 0.05;
            tilt.advance(|| slope, true, &moving, FollowStyle::Smart, &cfg, DT)
        });
    }

    /// The arm starts at phase zero and the disarm is a cosine ramp, so neither edge steps the eye.
    #[test]
    fn arming_and_disarming_the_bob_never_steps_the_eye_by_a_visible_amount() {
        use super::super::camera::follow_cmd as c;
        use super::super::camera_channel::assert_bounded_step;

        let cfg = CameraOptions {
            bobbing: true,
            ..CameraOptions::default()
        };
        // The bounds are fractions of a full amplitude in one frame.
        let amplitude = cfg.bob_ud_amplitude * BOB_AMPLITUDE_SCALE;
        let mut bob = HeadBob::default();
        let mut step = |now: f32| {
            let command = if (1.0..4.0).contains(&now) {
                c::FORWARD
            } else {
                0
            };
            bob.advance(
                0.0,
                &SubjectState {
                    command,
                    speed: 7.0,
                    ..SubjectState::default()
                },
                &cfg,
                DT,
            );
            bob.offset().length()
        };
        // Arming and three seconds of bobbing, then the disarm's ramp.
        assert_bounded_step((0.0, 3.9), DT, amplitude * 0.25, &mut step);
        assert_bounded_step((3.9, 6.0), DT, amplitude * 0.5, &mut step);
    }

    /// The hand-off (`0x50d500` push, `0x50d520` pop).
    #[test]
    fn mouse_look_hands_the_lean_into_the_pitch_and_takes_it_back() {
        use super::super::camera::FollowStyle;
        let cfg = CameraOptions {
            terrain_tilt: true,
            ..CameraOptions::default()
        };
        let moving = SubjectState {
            move_flags: mf::FORWARD,
            ..SubjectState::default()
        };
        let mut tilt = TerrainTilt::default();
        for _ in 0..900 {
            tilt.advance(|| 0.5, true, &moving, FollowStyle::Smart, &cfg, DT);
        }
        let lean = tilt.pitch();
        assert!(lean > 0.0, "uphill leans the view UP in benilla's sign");

        let mut pitch = 0.2_f32;
        let composite = pitch + tilt.pitch();
        pitch += tilt.hand_off(true);
        assert_eq!(
            tilt.pitch(),
            0.0,
            "the compose stops while the hand-off holds"
        );
        assert!(
            (pitch + tilt.pitch() - composite).abs() < 1.0e-6,
            "the press must not move the view"
        );
        // Held: the reference's refcount does not push twice.
        assert_eq!(tilt.hand_off(true), 0.0);

        pitch += tilt.hand_off(false);
        assert!((pitch - 0.2).abs() < 1.0e-6, "the pop returns the push");
        assert!(
            (pitch + tilt.pitch() - composite).abs() < 1.0e-6,
            "and the release must not move the view either"
        );

        // Press on the hill, flatten, release.
        pitch += tilt.hand_off(true);
        for _ in 0..900 {
            tilt.advance(|| 0.0, true, &moving, FollowStyle::Smart, &cfg, DT);
        }
        assert_eq!(
            tilt.ground.live(),
            0.0,
            "the channel keeps tracking under the hand-off"
        );
        pitch += tilt.hand_off(false);
        assert!(
            (pitch - (0.2 + lean)).abs() < 1.0e-6,
            "the step left behind is the lean at press minus the lean at release"
        );
    }

    /// `0x511010`'s first disjunct: the Track latch with `cameraSmoothTrackingStyle` at Never.
    #[test]
    fn a_never_tracking_swing_holds_the_bias_the_gate_would_have_released() {
        use super::super::camera::FollowStyle;
        let cfg = CameraOptions::default();
        // A bias from the pure-pivot leg, then half a second with the gate dropped.
        let run = |subject: &SubjectState, tracking: FollowStyle| {
            let mut p = SmartPivot::default();
            assert_eq!(p.route_pitch(UP, 0.0, 0.1, subject, true, &cfg), None);
            let armed = p.bias();
            for _ in 0..30 {
                p.advance(0.1, subject, false, tracking, &cfg, DT);
            }
            (armed, p.bias())
        };
        let tracked = SubjectState {
            track: true,
            ..SubjectState::default()
        };

        let (armed, after) = run(&tracked, FollowStyle::Never);
        assert_eq!(after, armed, "tracking + Never blocks the ease home");

        let (armed, after) = run(&tracked, FollowStyle::Smart);
        assert!(
            after.abs() < armed.abs(),
            "the block is the tracking STYLE's, not the latch's alone"
        );

        let (armed, after) = run(&SubjectState::default(), FollowStyle::Never);
        assert!(
            after.abs() < armed.abs(),
            "and not the style's alone either — the latch has to be up"
        );
    }
}
