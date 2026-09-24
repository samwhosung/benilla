//! The third-person camera rig: the two mouse-look modes (right-drag turns the character, left-drag
//! orbits the camera), the wheel-zoom glide, the collision-swept boom that seats the camera behind the
//! avatar's framing [`CameraPivot`], and the self-avatar zoom-in fade as the boom pulls into first
//! person. Split out of the controller — this owns the camera's pose and input session, not the
//! avatar/movement/networking [`super::control`] drives with it.

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

/// The reference's up-edge click predicate, in **camera degrees and milliseconds** — the whole
/// orbit-vs-select law (decision 1122). Verbatim, from `0x514ae0`, which returns
/// 1 = suppress / 0 = dispatch:
///
/// ```text
/// isClick = elapsed < 200ms
///        || (elapsed < 800ms && yaw_travel < 2.25° && pitch_travel < 2.0°)
/// ```
///
/// **The `ms` on those two numbers is now settled, where 1122 could only infer it**:
/// the clock both constants are compared against is `0x42c010` → `0x42b790`, whose counter is the
/// `KERNEL32!GetTickCount` import at `[0x7ff310]` and whose scale is stored as `1.0/freq × 1000.0`
/// — milliseconds in either counter mode. Nothing here changes; the units simply stopped being a
/// guess.
///
/// **A press under 200 world selects however far the mouse swept.** That arm is the entire bug report
/// (ledger B226): flick the cursor across a mob and click on arrival with the hand still moving, and
/// the reference targets it — the camera has been orbiting since the first motion sample and selects
/// anyway. The two mechanisms are independent and share only the button state.
///
/// **The orbit has no threshold at all** — it engages on the *down* edge (`0x51491f`, guarded only by
/// the 0→1 held transition) and the first motion sample already turns the camera. benilla deferred it
/// behind 4 px of travel and, worse, *destroyed* the pending click on crossing; there is no such
/// thing in the reference. The travel numbers below gate the **click**, never the orbit.
///
/// The literals in the binary are 8.0 event-units per axis against `Σ|0.8·Δx|` / `Σ|0.6·Δy|` of raw
/// device counts. Copying those would bind us to the reference's device scaling, which is itself
/// only INFERRED ("1 unit = 1 mouse count" is not established). So we copy the **angle** they mean,
/// which is exact and device-independent: 8.0 event-units is `Σ|Δx| = 10.0` raw ⇒
/// `cameraYawMoveSpeed·10/800` = **2.25°** of yaw at the shipped default 180; and `Σ|Δy| = 13.333`
/// raw ⇒ `cameraPitchMoveSpeed·13.333/600` = **2.0°** of pitch at the default 90.
///
/// Per axis, independently, and **accumulated absolute travel** — not net displacement from the
/// press point (`0x514400`: `fabs; fadd; fstp`, no subtraction and no stored origin). A shake that
/// returns to where it started still spends the budget.
const CLICK_HOLD_CEILING: f32 = 0.800;
const CLICK_FREE_WINDOW: f32 = 0.200;
const CLICK_YAW_TRAVEL: f32 = 2.25 * std::f32::consts::PI / 180.0;
const CLICK_PITCH_TRAVEL: f32 = 2.0 * std::f32::consts::PI / 180.0;

/// A primary button's press, while it is still undecided — the reference's world-input state
/// (`[0xbe1148]`): press instant `+0x14`, and the two independent motion accumulators zeroed at
/// `0x514910`/`0x514913`. Our accumulators are in **radians of camera rotation** rather than device
/// counts, for the reason in [`CLICK_HOLD_CEILING`]'s note.
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

    /// `0x514ae0`, minimised: the free window first, then the bounded-travel window.
    fn is_click(&self, now: f32) -> bool {
        let elapsed = now - self.at;
        elapsed < CLICK_FREE_WINDOW
            || (elapsed < CLICK_HOLD_CEILING
                && self.yaw_travel < CLICK_YAW_TRAVEL
                && self.pitch_travel < CLICK_PITCH_TRAVEL)
    }
}

/// Third-person orbit-distance limits (yards), from `FUN_005112d0` + the camera CVars:
/// max orbit = `cameraDistanceMax × cameraDistanceMaxFactor`,
/// **hard-capped at 50**; the low clamp is **0** — zoom-to-first-person (at distance 0 the eye sits at
/// the framing pivot, inside the head, and the avatar fades to invisible — see
/// [`benilla_world::model_fade::self_model_fade_alpha`]). The out-of-box max is **15** (`15 × 1`) —
/// the reference's, and since 1804 ours. This file shipped the factor fully raised (30 yd) from
/// the day it was written — a taste call ("a wider view") that nobody had weighed against the
/// client it imitates. The slider is still there and still reaches 30; it just is not where a
/// fresh install starts. Our starting zoom
/// is 15 — the reference's own shipped `cameraDistance` is 5.55 (`0x84f488`), a divergence
/// this file has always carried in its own words
/// ("pulled back a bit further than vanilla's own default for a wider view") and one 1804 leaves
/// alone: it is the camera's *initial state*, not a settings row, and it is the director's look.
pub(super) const CAM_DIST_MIN: f32 = 0.0;
/// The reference's `cameraDistanceMax` — the BASE the factor multiplies (registrar default 15).
/// Not exposed: 1.12's panel offers only the factor, so this stays the constant it is there.
pub(super) const CAM_DIST_BASE_MAX: f32 = 15.0;
/// `cameraDistanceMaxFactor`'s slider range — 1.12's own (MAX_FOLLOW_DIST: 1 … 2, step 0.1).
pub(crate) const CAM_DIST_FACTOR_RANGE: std::ops::RangeInclusive<f32> = 1.0..=2.0;
/// The orbit ceiling at the factor slider's **top** — the furthest any saved view or restored
/// camera pose may sit. Not the shipped default any more (1804): [`ZoomLimit::default`] is the
/// slider at rest, `1 × 15`. This is what a distance read back off disk is clamped to, which has
/// to admit the whole slider range rather than only its resting point.
pub(super) const CAM_DIST_MAX: f32 = CAM_DIST_BASE_MAX * 2.0;
pub(super) const CAM_DIST_DEFAULT: f32 = 15.0;

/// The max-orbit knob (decision 1140) — 1.12's `cameraDistanceMaxFactor` over the base above.
/// A fourth frozen constant made reachable: [`CAM_DIST_MAX`] was the only zoom ceiling there was.
///
/// **The default is the reference's 1.0** — `15 yd` (`cameraDistanceMax` defaults to `"15.0"`,
/// `0x84fbd0`, and `cameraDistanceMaxFactor` to `"1.0"`, `0x82e92c`). It was
/// 2.0 from 1140 until 1804, which is the whole reason that record exists: the raised factor was a
/// reasonable taste call on its day and it was never weighed as a *default*, so benilla shipped a
/// camera that started 15 yd further out than the client it imitates. Raising the slider re-clamps
/// nothing; lowering it re-clamps the live target on the next frame, so the view comes in rather
/// than waiting for the next wheel notch.
#[derive(Resource)]
pub(crate) struct ZoomLimit {
    /// Max orbit distance in yards — `CAM_DIST_BASE_MAX × factor`, hard-capped like the client's 50.
    pub(crate) max: f32,
}

impl Default for ZoomLimit {
    fn default() -> Self {
        // The slider at rest — `cameraDistanceMaxFactor` 1.0 × the 15 yd base, i.e. the reference's
        // own out-of-box ceiling. `CAM_DIST_MAX` is the slider's TOP, and belongs to the clamps.
        Self {
            max: CAM_DIST_BASE_MAX,
        }
    }
}

impl ZoomLimit {
    /// Set from the CVar's factor, clamped to the reference's slider range first.
    pub(crate) fn set_factor(&mut self, factor: f32) {
        let f = factor.clamp(*CAM_DIST_FACTOR_RANGE.start(), *CAM_DIST_FACTOR_RANGE.end());
        self.max = CAM_DIST_BASE_MAX * f;
    }

    /// The live factor — the inverse of [`Self::set_factor`], kept for the weld test (the
    /// registry holds the string itself since 2303).
    #[cfg(test)]
    pub(crate) fn factor(&self) -> f32 {
        self.max / CAM_DIST_BASE_MAX
    }
}
/// Yards the wheel moves the target per notch — `CameraZoomIn`/`CameraZoomOut`'s default `amount`
/// (VERIFIED 1.0 in `WoW.exe`).
const CAM_ZOOM_STEP: f32 = 1.0;
/// Camera zoom speed in **yards/second** — `cameraDistanceMoveSpeed` (VERIFIED default 8.33). Vanilla
/// glides the distance toward the wheel target at this *constant velocity* (linear, frame-delta-scaled
/// — `FUN_005112d0` in `WoW.exe`), **not** an exponential ease.
const CAM_MOVE_SPEED: f32 = 8.33;
/// Mouse-look sensitivity at the slider's neutral notch — radians of camera rotation per pixel of
/// mouse motion. [`LookConfig::sensitivity`] scales it; this is the ×1.0 case, and it is what the
/// client felt like before there was a slider at all (decision 1140).
const LOOK_SENSITIVITY: f32 = 0.003;
/// The `mousespeed` slider's range — 1.12's own (UIOptionsFrameSliders' MOUSE_SENSITIVITY row:
/// 0.5 … 1.5, step 0.05). A multiplier over [`LOOK_SENSITIVITY`], so the registered default 1.0
/// reproduces the shipped feel exactly.
pub(crate) const MOUSE_SPEED_RANGE: std::ops::RangeInclusive<f32> = 0.5..=1.5;

/// The camera rows' change callback (decision 2303) — the look, zoom and follow knobs.
pub(crate) fn on_cvar(
    ev: On<crate::cvars::CvarChanged>,
    mut look: ResMut<LookConfig>,
    mut zoom: ResMut<ZoomLimit>,
    mut follow: ResMut<FollowConfig>,
) {
    let v = ev.num();
    match ev.key().as_str() {
        "mouseinvertpitch" => look.invert_pitch = v != 0.0,
        // The 1.12 slider's own range; an off-grid hand-edit rides between stops, like the others.
        "mousespeed" => {
            look.sensitivity = v.clamp(*MOUSE_SPEED_RANGE.start(), *MOUSE_SPEED_RANGE.end());
        }
        // The reference's `0x50b330` validator REJECTS an out-of-range value rather than clamping
        // it: it prints `Value out of range (%f - %f)` and `CVar::Set` never stores, so the old
        // value stands. That is a different posture from every clamping row, and it is the
        // faithful one — a script writing 1e9 gets a refusal, not a silently pinned camera.
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
        // The three stops are 1 Smart / 2 Always / 3 Never; anything else reads as the registrar
        // default rather than as a dead camera (`FollowStyle::from_cvar`).
        "camerasmoothstyle" => follow.style = FollowStyle::from_cvar(v),
        // Its sibling selector — the one the reference swaps in for the externally-driven states.
        "camerasmoothtrackingstyle" => follow.tracking_style = FollowStyle::from_cvar(v),
        // The auto-follow rate, clamped to 1.12's own AUTO_FOLLOW_SPEED slider range.
        "camerayawsmoothspeed" => {
            follow.yaw_speed = v.clamp(*FOLLOW_SPEED_RANGE.start(), *FOLLOW_SPEED_RANGE.end());
        }
        _ => {}
    }
}

/// **The mouse-look rate law, and the one place benilla's units are not the reference's.**
///
/// The reference's own law (`0x50fee0`):
///
/// ```text
///   Δyaw_deg   = cameraYawMoveSpeed   × Δx / 800
///   Δpitch_deg = cameraPitchMoveSpeed × Δy / 600
/// ```
///
/// **800 × 600 is a screen, not a magic number** — the era's reference resolution. At the shipped
/// `180`/`90` the law reads: a drag across the full screen width is a half-turn, and a drag up the
/// full screen height is horizon-to-zenith. That is what makes the two divisors and the two
/// defaults one design rather than four constants.
///
/// **What does NOT transfer is the unit.** The reference has *no DirectInput import at all*: it
/// integrates `WM_MOUSEMOVE` (`0x42d31c`), so its `Δ` is a
/// **screen pixel after Windows pointer acceleration** — which is exactly why its `mousespeed`
/// slider works by calling `SPI_SETMOUSESPEED` on the OS rather than scaling anything in-engine.
/// benilla's `Δ` is `AccumulatedMouseMotion`, i.e. winit's `DeviceEvent::MouseMotion`: **raw,
/// unaccelerated device units**. `deg per accelerated pixel` and `deg per raw unit` are different
/// quantities, and the factor between them is a per-machine OS setting, not a fact about the
/// client — so transplanting `180`/`90` as absolute numbers would be a confident guess on a
/// load-bearing constant.
///
/// So we take the law's **shape** — per-axis, linear in the CVar — and anchor its **scale** to
/// [`LOOK_SENSITIVITY`], the rate this client has shipped and the director has been looking at
/// since 1140. The CVars therefore keep the reference's own defaults (1804: a setting's default is
/// the reference's) and the divergence lands here, in a named constant, where it can be read.
///
/// **The one live divergence this leaves**, stated rather than buried: the reference is
/// *anisotropic* at its defaults — `180/800 = 0.225` deg/px of yaw against `90/600 = 0.15` of
/// pitch, so its yaw turns 1.5× faster than its pitch. Ours is isotropic, because
/// [`LOOK_SENSITIVITY`] is one number. The **ratio** is unit-independent (both axes take the same
/// acceleration curve), so unlike the absolute scale it *is* transferable — it is simply a feel
/// change, and feel is the director's call, not a fidelity bug to fix quietly.
const LOOK_YAW_PER_SPEED: f32 = LOOK_SENSITIVITY / 180.0;
const LOOK_PITCH_PER_SPEED: f32 = LOOK_SENSITIVITY / 90.0;

/// The reference's validator range for all four `camera*MoveSpeed`/`SmoothSpeed` CVars
/// (`0x50c000` → `0x50b330`). It **rejects rather than clamps**: out of range prints
/// `"Value out of range (%f - %f)"` and `CVar::Set` skips the store, so the old value stands.
pub(crate) const CAMERA_SPEED_RANGE: std::ops::RangeInclusive<f32> = 0.1..=360.0;

/// The mouse-look player knobs (decision 0961): `mouseInvertPitch` is 1.12's own Interface
/// Options checkbox (UIOptionsFrame.lua index 1, CVar-backed), settable from the Options
/// window's Controls page through the CVar store (0954). Inverted, moving the mouse up pitches
/// the camera down — the delta.y term flips sign at the one apply site, both drag styles alike.
///
/// `sensitivity` is 1.12's `mousespeed` slider (1140), the same story one layer down: the rate was
/// a frozen constant with no way to reach it. It multiplies [`LOOK_SENSITIVITY`] at BOTH apply
/// sites — the rotation itself and the click-vs-drag travel budget — because the reference scales
/// the device delta once, upstream of everything that reads it, and a budget measured in unscaled
/// pixels would make a click's drag threshold drift as the slider moved.
#[derive(Resource, Clone, Copy)]
pub(crate) struct LookConfig {
    pub(crate) invert_pitch: bool,
    pub(crate) sensitivity: f32,
    /// `cameraYawMoveSpeed` — the MOUSE_LOOK_SPEED slider (90…270 by 10). See
    /// [`LOOK_YAW_PER_SPEED`] for why the number is the reference's and the scale is ours.
    pub(crate) yaw_speed: f32,
    /// `cameraPitchMoveSpeed` — no slider of its own; `UIOptionsFrame_Save` writes it as
    /// `cameraYawMoveSpeed / 2` beside the yaw one, which is exactly the reference's 180/90 pair.
    pub(crate) pitch_speed: f32,
}

impl Default for LookConfig {
    fn default() -> Self {
        Self {
            invert_pitch: false,
            sensitivity: 1.0,
            // The reference's registered defaults, and with them the shipped feel: at these two
            // values both axes land on `LOOK_SENSITIVITY` exactly.
            yaw_speed: 180.0,
            pitch_speed: 90.0,
        }
    }
}

impl LookConfig {
    /// Radians of camera rotation per pixel of mouse motion, this session. ONE function because
    /// both readers must agree: the look rotation itself and the click-vs-drag travel budget that
    /// decides whether a press was a click. Splitting them would let the slider move the drag
    /// threshold out from under the gesture (decision 1140).
    pub(super) fn yaw_rate(self) -> f32 {
        self.yaw_speed * LOOK_YAW_PER_SPEED * self.sensitivity
    }

    /// The pitch axis's rate — its own CVar, because the reference's law has its own divisor for
    /// it (600, the reference screen's height) and its own default (90).
    pub(super) fn pitch_rate(self) -> f32 {
        self.pitch_speed * LOOK_PITCH_PER_SPEED * self.sensitivity
    }
}
/// The auto-follow's angular rate — 1.12's `cameraYawSmoothSpeed`, registrar default **180 °/s**
/// (`WoW.exe` `[0xbe1070]`, read at `0x512d75`). It is not a slew rate but a *duration* divisor:
/// the transition below lasts `|Δyaw| / rate × factor`, so 180 °/s is the **average** rate at
/// factor 1, and the cosine profile peaks at π/2 × that. 1.12 exposes it as the AUTO_FOLLOW_SPEED
/// slider (`UIOptionsFrameSliders`, 90 … 270 by 10), disabled while the style is Never.
pub(crate) const FOLLOW_SPEED_DEFAULT: f32 = 180.0;
/// The 1.12 slider's own range for [`FollowConfig::yaw_speed`].
pub(crate) const FOLLOW_SPEED_RANGE: std::ops::RangeInclusive<f32> = 90.0..=270.0;
/// `cameraSmoothTimeMin`/`Max` — the transition's duration floor and ceiling, **VERIFIED
/// 0.1 s / 2.0 s** (`[0xbe105c]`/`[0xbe1038]`, clamped at `0x510f4d`). These are why the felt
/// rate is *not* 180 °/s at the ends: a 5° correction still takes 0.1 s, and a lazy `Track`
/// return (factor 10) is capped at 2 s however far it has to come.
const FOLLOW_TIME_MIN: f32 = 0.1;
const FOLLOW_TIME_MAX: f32 = 2.0;
/// The "already there / already arming this" epsilon — the reference's own `0.001`
/// (`[0x801360]`, `0x512ce4`/`0x512d41`). A float-equality guard, not a perceptible deadzone.
const FOLLOW_EPS: f32 = 1.0e-3;

/// **Camera Following Style** — 1.12's `cameraSmoothStyle` (decisions 1493/1502): does the camera
/// return to behind the character on its own?
///
/// benilla shipped the reference behaviour *removed* from the day the camera was written (the
/// orbit offset simply persisted, a director's call) — this is the setting that gives it back, at
/// the reference's own default: **Smart**.
///
/// **The enum is the ENGINE's, `0 = Never · 1 = Smart · 2 = Always`** — twice over (the
/// registration loop in `0x50b6f0` walks
/// `{"Never","Smart","Always"}` filling three blocks in order, and both consumers index by
/// `style × stride`). 1.12's own *dropdown* writes `1/2/3` instead (`UIOptionsFrameCameraDropDown`
/// — Smart 1, Always 2, Never **3**), and 3 is not a style at all: the validator accepts it
/// (`0x50b330(v, 0, 3)`) but the terrain-tilt consumer does not bound-check and indexes 360 bytes
/// past its table. So the reference's shipped UI writes an out-of-range value for Never, and a
/// client must clamp to 0..2. We do: our own dropdown writes the engine's numbers, and a stray
/// `3` is *read* as Never — what whoever wrote it meant.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum FollowStyle {
    /// Never auto-adjust: the orbit offset stays exactly where the hand left it. This is what
    /// benilla did unconditionally before 1493. It does **not** stop the rigid keyboard-turn
    /// carry — in the reference that is a different mechanism entirely (the camera yaw is stored
    /// *relative* to the followed unit's facing), and Never leaves it running.
    Never,
    /// Stay where placed, *except* while the character is being driven — the reference's own
    /// "(Recommended Mode)" and its registrar default.
    #[default]
    Smart,
    /// Always prefer being behind the character: every input edge arms a return, standing still
    /// included.
    Always,
}

impl FollowStyle {
    /// From the CVar's number. `3` is the shipped-UI's Never (see the type doc); anything else off
    /// the ladder reads as the registrar default rather than as a dead camera.
    pub(crate) fn from_cvar(v: f32) -> Self {
        match v as i32 {
            0 => Self::Never,
            2 => Self::Always,
            // The 1.12 dropdown's own Never. Out of range for the engine's tables — accepted here
            // because a config or addon carrying it means Never, not "surprise me".
            3 => Self::Never,
            _ => Self::Smart,
        }
    }

    /// The CVar string this style is — the inverse of [`Self::from_cvar`], kept for the
    /// round-trip test (the registry holds the string itself since 2303).
    #[cfg(test)]
    pub(crate) fn cvar(self) -> &'static str {
        match self {
            Self::Never => "0",
            Self::Smart => "1",
            Self::Always => "2",
        }
    }

    /// The `cameraSmooth<Style><State>{Delay,Factor}` row — family A at `[0xbe0e70]`, at its
    /// defaults. Factor `0` means *cancel*: the armed
    /// transition is dropped and the camera keeps the offset it has.
    ///
    /// Family B (`cameraSmoothViewData<Style>Yaw{Delay,Factor}`) multiplies in: its Yaw factor is
    /// `1.0` under Smart and Always and `0.0` under Never, and its delay is `0.0` at every style —
    /// so for the yaw channel the composition is the identity and the table below is the answer.
    fn row(self, state: FollowState) -> (f32, f32) {
        match self {
            // Every row is 0/0 — and family B's Never Yaw factor is 0.0 as well, twice over.
            Self::Never => (0.0, 0.0),
            Self::Smart => match state {
                // The two states that make Smart *smart*: nothing returns while you stand or stop.
                FollowState::Idle | FollowState::Stop => (0.0, 0.0),
                // Driven from outside (a taxi, a spline, a fear): a lazy return — 0.4 s of delay
                // and factor 10, which is 18 °/s of average rate before the 2 s cap bites.
                FollowState::Track | FollowState::Fear => (0.4, 10.0),
                FollowState::Move | FollowState::Strafe | FollowState::Turn => (0.0, 1.0),
            },
            // Always is factor 1.0 in every state, Idle and Stop included.
            Self::Always => (0.0, 1.0),
        }
    }
}

/// The seven arming states of 1.12's auto-return classifier (`0x510960`),
/// **in the reference's own priority order — highest first**. The
/// winner is the highest-priority bit set, and the states are read off the *camera's* input
/// command word, not off the character's velocity: right-mouse alone is a `Turn`, a turn key
/// under right-mouse is a `Strafe`, and both mouse buttons are a `Move`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FollowState {
    /// Driven by something that is not us and is not a spline — the reference's external-control
    /// flag `[cam+0x90] & 0x1000`.
    Fear,
    Turn,
    Strafe,
    Move,
    /// Externally-driven movement (taxi / server spline) — `[cam+0x90] & 0x100`.
    Track,
    /// This edge released a movement input (the reference's `stopping` argument). Carries the same
    /// `(0, 0)` / `(0, 1)` rows as [`FollowState::Idle`] at every shipped style, so the two are
    /// currently indistinguishable in behaviour — kept apart because the *matrix* keeps them apart.
    Stop,
    Idle,
}

/// The camera's input command word — 1.12's `[InputControl+0x4]` bit for bit,
/// because the state classifier and the *edge* that arms a
/// transition are both defined on it. benilla has no PitchUp/PitchDown bindings, so those two bits
/// are simply never set.
pub(super) mod follow_cmd {
    /// `TurnOrAction` — right mouse / mouselook.
    pub(in crate::player) const RIGHT_MOUSE: u32 = 0x1;
    /// `CameraOrSelectOrMove` — left mouse.
    pub(in crate::player) const LEFT_MOUSE: u32 = 0x2;
    pub(in crate::player) const FORWARD: u32 = 0x10;
    pub(in crate::player) const BACKWARD: u32 = 0x20;
    pub(in crate::player) const STRAFE_LEFT: u32 = 0x40;
    pub(in crate::player) const STRAFE_RIGHT: u32 = 0x80;
    pub(in crate::player) const TURN_LEFT: u32 = 0x100;
    pub(in crate::player) const TURN_RIGHT: u32 = 0x200;
    pub(in crate::player) const AUTORUN: u32 = 0x1000;
    /// The union of bits 20/21/23, which the reference folds into the camera's `Track` flag —
    /// externally-driven movement.
    pub(in crate::player) const TRACK: u32 = 0x100000;
    /// External control (the reference's own `[cam+0x90] & 0x1000`, not an InputControl bit —
    /// carried here so one word carries every edge the arming function reacts to).
    pub(in crate::player) const FEAR: u32 = 0x2000_0000;

    /// The bits `Move` reads: forward, backward, autorun.
    pub(in crate::player) const MOVE_BITS: u32 = FORWARD | BACKWARD | AUTORUN;
    pub(in crate::player) const STRAFE_BITS: u32 = STRAFE_LEFT | STRAFE_RIGHT;
    pub(in crate::player) const TURN_BITS: u32 = TURN_LEFT | TURN_RIGHT;
}

/// The knobs behind the auto-follow (decision 1502) — the style, the style the *externally-driven*
/// states use instead, and the rate. All three are 1.12 CVars with the reference's own defaults.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct FollowConfig {
    /// `cameraSmoothStyle`.
    pub(crate) style: FollowStyle,
    /// `cameraSmoothTrackingStyle` — the selector the reference swaps in when the state mask so
    /// much as *contains* `Track` or `Fear` (`0x510a51 test bl,0x44`, before the priority scan),
    /// indexing the very same matrices. Registrar default `"1"` = Smart, like its sibling.
    pub(crate) tracking_style: FollowStyle,
    /// `cameraYawSmoothSpeed`, °/s — see [`FOLLOW_SPEED_DEFAULT`].
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

/// What [`seat_camera`] needs to run the auto-follow this frame: the knobs, where "behind" is, and
/// the input word whose *edges* arm a return. Bundled because they are one question, and
/// `seat_camera` is already at its argument ceiling.
pub(super) struct FollowInput {
    pub(super) cfg: FollowConfig,
    /// The character's own facing (same yaw convention as [`FlyCam::yaw`] — a right-drag couples
    /// the two directly), so the camera's *offset* is `cam.yaw − face_yaw`.
    pub(super) face_yaw: f32,
    /// This frame's [`follow_cmd`] word.
    pub(super) command: u32,
}

impl FollowInput {
    /// The winning state — the reference's descending priority scan, highest bit first.
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

    /// Which style picks the row: the *tracking* style whenever the mask so much as contains
    /// `Track` or `Fear`, even when a higher-priority state supplies the row.
    fn style(&self) -> FollowStyle {
        if self.command & (follow_cmd::TRACK | follow_cmd::FEAR) != 0 {
            self.cfg.tracking_style
        } else {
            self.cfg.style
        }
    }
}

/// The armed yaw transition — the reference's descriptor `[+0x208 startMs, +0x20c dur,
/// +0x210 target, +0x214 start]`, in seconds and in *offset* space.
#[derive(Clone, Copy, Debug)]
struct FollowArm {
    /// The offset the transition started from, radians (camera yaw minus character facing).
    from: f32,
    /// Where it is going. `0.0` — directly behind — at the shipped defaults, because
    /// `cameraYawSmoothMin`/`Max` are both `0.0` and the reference *substitutes* the crossed bound
    /// for the saved view yaw whenever the live offset is outside the band.
    to: f32,
    /// Seconds the move takes, already clamped to `[FOLLOW_TIME_MIN, FOLLOW_TIME_MAX]`.
    dur: f32,
    /// Seconds of dead time before it starts (`Track`/`Fear` under Smart: 0.4).
    delay: f32,
    /// Seconds since it was armed.
    elapsed: f32,
    /// What it was armed with — the reference's re-arm memo (`[+0x218, +0x21c]`), so a repeated
    /// edge asking for the same transition is a no-op instead of restarting the swing.
    armed_with: (f32, f32),
}

/// The auto-follow's own state on the rig: the input word we last saw (edges are what arm a
/// return) and the transition in flight, if any.
#[derive(Default)]
pub(super) struct FollowRig {
    last_command: Option<u32>,
    arm: Option<FollowArm>,
}

impl FollowRig {
    /// Run the auto-follow for a frame and return the camera yaw it wants, if it wants one.
    ///
    /// The shape is the reference's, and the shape is the point (`0x510960`, `0x50f160`): a
    /// transition is **armed on an input edge**, from a snapshot taken at that instant,
    /// and then plays out unattended — it is *not* a per-frame chase of a moving target. That is
    /// why "drag the camera aside, then press W" swings you back over one smooth arc, while
    /// holding W changes nothing at all.
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
        // (`0x510850`'s `!([cam+0x90] & 1)`), and **entering** mouse-look cancels whatever was in
        // flight outright (`0x50fe30` zeroes the descriptors). So the return does not resume when
        // the button comes up — it begins at the next input edge after the release, which is
        // usually the release itself (the mouse bits are part of the word).
        if look_held {
            self.arm = None;
            return None;
        }
        // The EDGE: any movement/camera binding changing state re-evaluates the transition. The
        // reference re-evaluates on the binding call itself, not on a change of the classified
        // state, which is why releasing the drag while standing still is an Idle *arming* under
        // Always and an Idle *cancel* under Smart.
        if let Some(p) = previous.filter(|p| *p != word) {
            // `stopping` — the reference's second argument, set by the Stop half of a movement
            // binding. Our equivalent is the edge itself: a movement bit went away.
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
            // The reference's kernel `0x5b7bb0` — a **cosine** smoothstep, eased at both ends:
            // `a + (b − a)·(1 − cos(πs))/2`.
            let e = (1.0 - (std::f32::consts::PI * s).cos()) * 0.5;
            arm.from + (arm.to - arm.from) * e
        };
        Some(wrap_pi(input.face_yaw + offset))
    }

    /// What the transition is doing, for `WOW_CAM_DUMP` — `None` when nothing is armed, else
    /// `(elapsed, delay, duration)` in seconds. The auto-follow is the one camera behaviour with
    /// no headless retest (nothing synthesizes a mouse drag), so the trace line is how a run says
    /// what it armed and when.
    fn probe(&self) -> Option<(f32, f32, f32)> {
        self.arm.map(|a| (a.elapsed, a.delay, a.dur))
    }

    /// The arming half (`0x510960` → `0x512c70`): pick the row, then either cancel outright or
    /// snapshot a transition to the target offset.
    fn arm(&mut self, input: &FollowInput, cam_yaw: f32, stopping: bool) {
        let (delay, factor) = input.style().row(input.state(stopping));
        if factor == 0.0 {
            // Cancel: the target becomes the live yaw and the channel disarms — the camera simply
            // keeps the offset it has. This is Smart standing still, and it is all of Never.
            self.arm = None;
            return;
        }
        // The target: directly behind, at the shipped defaults (see [`FollowArm::to`]).
        let to = 0.0;
        let from = wrap_pi(cam_yaw - input.face_yaw);
        let gap = (to - from).abs();
        if gap < FOLLOW_EPS {
            return; // already there — the reference returns without arming
        }
        // The re-arm memo: an edge asking for the transition already in flight is a no-op, so a
        // second keypress mid-swing does not restart it from the current angle.
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

/// Camera pitch clamp (radians) — **±89.00°** (`WoW.exe` `0x8089d8`/`0x8089dc` =
/// 1.5533430576 rad; the pitch integrate `FUN_00510120`). A single uniform
/// clamp at every zoom level — the reference has **no** distinct first-person look-down limit.
pub(super) const CAM_PITCH_LIMIT: f32 = 89.0 * std::f32::consts::PI / 180.0;
/// How fast the camera glides back out to the player's chosen zoom once an obstruction clears (1/s).
/// Pull-*in* is instant (a wall must never sit between the camera and the character); only the
/// push-*out* eases — the vanilla feel of the camera snapping close past an obstacle and easing back.
const CAM_RETURN_RATE: f32 = 6.0;
/// The camera framing pivot — the point the boom looks at + seats behind, and the first-person eye at
/// zoom 0 — sits at `feet + H` where **H is model-derived** (not a fixed height):
/// `H = (attach17.z + 0.0972) × scale` from **M2 attachment id 17** (`WoW.exe` `0x50cbc0`) —
/// ~neck height on every character (1.90 human / 0.88 gnome), with a `0.9 × vertex-box`
/// fallback only for models lacking that attachment. Floored at [`CAM_PIVOT_FLOOR`]. The per-model
/// pre-scale height rides on [`CameraPivot`], stamped at attach; `control` multiplies the live scale and
/// floors. The collision sweep still starts from the *head* (not the pivot), so a jump in a low room
/// stops the camera under the ceiling — see `control`.
///
/// That height is one of the reference's **three** presets, rebuilt together by `0x50ca90` and
/// chosen between per frame by `0x50f880`. benilla builds two of the three: the standing height
/// above, and the **swim** preset `cam+0x124` — the same height less the model's
/// [`CameraPivot::swim_drop_local`] — selected on MOVEFLAG_SWIMMING. The zoomed-in/zoomed-out pair
/// (`cam+0x11c`/`cam+0x120`, split on `cam+0x198 < 1.8315`) is **not built**; see
/// [`model_pivot_height`].
///
/// Floor (yd) on the world pivot height — VERIFIED `5/6` (`0x50ca90`'s per-preset clamp, and
/// `0x50e570`'s corridor lower bound).
pub(super) const CAM_PIVOT_FLOOR: f32 = 5.0 / 6.0;
/// Ceiling (yd) on the world pivot height — VERIFIED `15.0` (`[0x8089c8]`, the upper arm of the same
/// per-preset clamp in `0x50ca90`). A giant's scale cannot walk the framing pivot off into the sky.
pub(super) const CAM_PIVOT_CEIL: f32 = 15.0;
/// Pivot height used before the avatar model has attached (so `CameraPivot` isn't on the entity yet):
/// a human's ~neck height, so the first frames of third-person don't ride high. Replaced by the exact
/// model-derived value the moment the body attaches — as a **snap**, not a glide ([`PivotGlide`]).
pub(super) const CAM_PIVOT_FALLBACK: f32 = 1.8;

/// One modeled unit's world framing-pivot height: its model-local [`CameraPivot`] × the given scale,
/// clamped to `[CAM_PIVOT_FLOOR, CAM_PIVOT_CEIL]` — the reference's per-preset clamp in `0x50ca90`.
///
/// **`swimming` selects the preset, per frame, the way `0x50f880` does**
/// (`0x50f89e test [[unit+0x118]+0x40],0x200000` — MOVEFLAG_SWIMMING on the *camera target's*
/// CMovement word): set, and the pivot drops by the model's own
/// [`benilla_formats::M2Bounds::swim_pivot_drop`] before the clamp, which is the reference's
/// `cam+0x124` preset. Clear, and it stays the standing height. The drop is a *model* constant
/// (`StandSeq.max.z − SwimSeq.max.z`, `0x50ccf6`), so a body that authors no Swim sequence carries
/// `0.0` and the two presets are the same number — correct for anything that cannot swim.
///
/// **What is deliberately not here:** the reference keeps a *third* preset. `0x50f880`'s non-swim
/// leg picks `cam+0x11c` (zoomed-in) or `cam+0x120` (zoomed-out) on `cam+0x198 < 1.8315`
/// (`[0x8089b0]`), and benilla builds neither — one standing height serves both zoom regimes. On a
/// scale-1 human the reference computes those two presets *equal* (both 1.9002692), so nothing yet
/// says what authors them apart; naming it beats stubbing a threshold we cannot justify (1203).
pub(super) fn model_pivot_height(pivot: &CameraPivot, scale: f32, swimming: bool) -> f32 {
    let local = if swimming {
        pivot.height_local - pivot.swim_drop_local
    } else {
        pivot.height_local
    };
    (local * scale).clamp(CAM_PIVOT_FLOOR, CAM_PIVOT_CEIL)
}

/// World head height above a modeled unit's feet — [`model_pivot_height`], or the neck-height
/// [`CAM_PIVOT_FALLBACK`] when the body has no model yet. The single definition shared by the things
/// that sit at the character's head: the framing-pivot *target*, the far-sight subject's pivot, and
/// the 3D-audio listener (the client's `SoundListenerAtCharacter=1` default, `0x457890`).
///
/// **Which `scale` to pass is a fidelity question with a verified answer**: the reference's pivot
/// preset multiplies the **raw**
/// `OBJECT_FIELD_SCALE_X` descriptor (vtable slot 7 = `0x469f10`, `fld [descriptors+0x10]`), *not*
/// the 2 s-eased render scale — the two are deliberately split in the binary (`0x4833d3` folds the
/// eased one in for a selection-ring consumer, and only there). So the camera passes
/// [`crate::net::NetEntity::scale`] and the pivot moves in **one** step per model event, which
/// [`PivotGlide`] then walks; multiplying by the eased scale instead would stack a second, slower
/// ease on top of the first and is what made a shapeshift snap *and* drift. The audio listener still
/// passes the rendered scale — it tracks the drawn body, and nothing verified says otherwise.
///
/// **Always the standing preset.** Its two consumers are a *head*, not the camera's framing pivot:
/// the 3D-audio listener sits at our own head, and the far-sight subject is a unit whose movement
/// flags we do not carry. The swim preset is the framing pivot's alone — [`model_pivot_height`]
/// with `swimming` — and the driven body's own target goes through
/// [`super::body_pose::pivot_target`], not here.
pub(crate) fn head_height(pivot: Option<&CameraPivot>, scale: f32) -> f32 {
    pivot.map_or(CAM_PIVOT_FALLBACK, |p| model_pivot_height(p, scale, false))
}

/// `cameraHeightSmoothSpeed` (yd/s) — the pivot channel's rate, VERIFIED registrar default `"1.2"`.
/// The duration of a move is `|Δh| / this` (`0x51276c`/`0x512777`), so it is an *average* rate: the
/// cosine profile peaks at `π/2 ×` it in the middle and is zero at both ends. There is no duration
/// clamp on this channel (unlike the yaw channel's `[0.1 s, 2.0 s]`).
const CAM_PIVOT_SMOOTH_SPEED: f32 = 1.2;
/// **The camera's pivot-height channel** — the height the framing pivot actually rides, chasing
/// the model-derived target with a cosine smoothstep instead of taking it raw.
///
/// The reference's live `cam+0xfc` chasing target `cam+0x1c8`: armed by `0x5126b0` → `0x512790`,
/// stepped by `0x50f160`'s `[0x50f36a, 0x50f417)` block — the **fourth instantiation** of the
/// channel template [`SmoothChannel`] holds, which is why nothing of the tween lives here any more.
/// What is left is the two things this instantiation does that its three siblings do not.
/// **This is why a druid shapeshift does not snap the reference's camera**, and it glides in *both*
/// directions: the solver's `max(target, live)` (`0x50e5a9`) is only the collision-corridor seed, and
/// the far chain clamps the result back down to the live value (`0x50e767`), so an unobstructed pivot
/// simply *is* `cam+0xfc` — rising or falling.
///
/// Two structural facts, both load-bearing:
/// - **The first arm of a camera's life snaps** (`0x5127d4`; the latch bit `0x80` is never cleared —
///   image-wide census). So logging in establishes the height instantly, and everything after it
///   glides. Nothing else re-snaps: a target-GUID change, a mount, a morph, `SetView` — all glide.
/// - **A model that has not resolved yet holds the channel**, it does not re-aim it (the reference
///   skips the whole camera update while the preset is stale, `0x50e907`). Ours is the `None` target:
///   during the frames a swapped-in model is loading, the pivot stays where it is and one glide runs
///   when the new height lands.
#[derive(Default)]
pub(super) struct PivotGlide {
    /// The channel itself — **linear**, not angular: its live value is yards, so the armer's `2π`
    /// rewrap must not run on it.
    channel: SmoothChannel,
    /// Has the channel ever been armed? The reference's latch bit `0x80` — false only until the
    /// first model-derived height arrives, which is therefore a snap.
    seeded: bool,
}

impl PivotGlide {
    /// Arm the channel with this frame's model-derived target (`None` while the subject has no
    /// model — hold), step whatever is in flight, and return the height to frame at.
    ///
    /// Called every frame, which is the reference's own cadence (`0x50f880` from the driver tail
    /// `0x50f011`): the armer's own two epsilon refusals turn a steady target into a no-op, so
    /// "arm per frame" and "arm on change" are the same thing except at the instant the target
    /// actually moves.
    pub(super) fn advance(&mut self, target: Option<f32>, dt: f32) -> f32 {
        if let Some(target) = target {
            if self.seeded {
                self.channel.arm(&Arm::at(target, CAM_PIVOT_SMOOTH_SPEED));
            } else {
                // The latch: the first height a camera ever sees is established, not travelled to.
                self.seeded = true;
                self.channel.snap(target);
            }
        }
        self.channel.advance(dt)
    }

    /// What the channel is doing, for `WOW_CAM_DUMP`: `(live, target)`. A pivot question is a
    /// *timing* question — "does it snap?" is answered by these two columns on a trace, never by
    /// watching a capture (method: timing is measured, never eyeballed).
    pub(super) fn probe(&self) -> (f32, f32) {
        self.channel.probe()
    }
}

/// Which mouse button is driving mouse-look, if any — the two vanilla look modes. While looking, the
/// OS cursor is hidden + locked in place (relative motion drives the camera); `cursor_stash` is the
/// position it's restored to on release so it reappears exactly where the user pressed. `pub(crate)`
/// so the [`crate::cursor`] subsystem can hide the cursor while looking (`is_looking`).
#[derive(Resource, Default)]
pub(crate) struct CameraControl {
    /// Current third-person orbit distance (yards) — eased toward `target_distance` each frame so the
    /// wheel zoom glides instead of snapping (like the real client).
    pub(super) distance: f32,
    /// Where the wheel set the orbit distance; `distance` chases this.
    pub(super) target_distance: f32,
    /// Effective length of the camera arm (from the head pivot out to the camera) after world
    /// collision. Pulled in instantly when geometry intrudes (so a wall never sits between the camera
    /// and the character), eased back out when it clears. Kept separate from the zoom `distance` so the
    /// player's chosen zoom is preserved while obstructed and restored once the view is open again.
    pub(super) collision_distance: f32,
    /// The button currently held for look, or `None`.
    pub(super) look: Option<LookButton>,
    /// **Is the player in mouse-look right now?** — the reference's `[cam+0x90] & 1`, set at
    /// `0x50fe41` and cleared at `0x50fddd`, identified at the bytes by the two mode strings
    /// `"Camera FREELOOK"` / `"Camera NORMAL"`. Written by [`run_look_session`] and read the same
    /// frame by [`seat_on_subject`], which is `cameraTerrainTilt`'s hand-off edge.
    pub(super) freelook: bool,
    /// **Which mouse buttons the world owns** this frame ([`WorldMouse`]) — the player side's one
    /// answer to "did the UI eat that press?", written by [`latch_world_mouse`] before anything
    /// reads a button. The look session, the camera's input command word and the both-button run
    /// all read it instead of `ButtonInput`.
    pub(super) world_mouse: WorldMouse,
    /// Logical cursor position captured when look began, to restore on release.
    pub(super) cursor_stash: Option<Vec2>,
    /// The self-avatar's render alpha for this frame, from the camera-to-pivot distance
    /// ([`benilla_world::model_fade::self_model_fade_alpha`]): `1.0` third-person (opaque), ramping to `0.0` as
    /// the camera zooms into the head (first-person). `control` computes it (it owns the pivot + camera
    /// pose); [`apply_self_model_fade`] applies it to the body parts. Starts opaque.
    pub(super) self_fade_alpha: f32,
    /// The auto-follow's own state (decision 1502): the input word we last saw, and the armed
    /// return in flight. It lives on the rig rather than beside the knob because it is *pose*, not
    /// setting — a transition survives the frame, not the session.
    pub(super) follow: FollowRig,
    /// The framing pivot's height channel — smoothed, not taken raw ([`PivotGlide`]). On the rig
    /// for the same reason `follow` is, and for one more: it belongs to the **camera**, not to the
    /// body, which is why it glides *through* a change of subject (a shapeshift, a far-sight
    /// switch) instead of being reset by one.
    pub(super) pivot: PivotGlide,
    /// `cameraPivot`'s pitch-bias channel ([`SmartPivot`], decision 2149) — pose, like the two
    /// above it.
    pub(super) smart_pivot: SmartPivot,
    /// `cameraTerrainTilt`'s ground-pitch channel and its 100 ms probe throttle ([`TerrainTilt`],
    /// decision 2149).
    pub(super) terrain_tilt: TerrainTilt,
    /// `cameraBobbing`'s session latch and eye offset ([`HeadBob`], decision 2149).
    pub(super) head_bob: HeadBob,
    /// **Did the collision sweep actually clip the camera on the frame just seated?** The
    /// reference's `[cam+0x90] & 0x30000`, which nothing but the solver `0x50e570` writes and the
    /// driver ORs in per frame — and which [`SmartPivot`]'s gate reads as its sixth conjunct, so
    /// that an unobstructed camera never pivots. Written by [`seat_camera`]; read a frame later by
    /// [`run_look_session`], exactly as the reference's input handler reads the flags the last
    /// driver pass left (`0x50fee0`'s sole caller `0x514446` precedes the mover lookup).
    pub(super) clipped: bool,
}

impl CameraControl {
    /// Park the orbit distance at `d` — **both** the live value and the wheel target.
    ///
    /// Both, or the wheel glide eases `distance` back toward the old target every frame and a
    /// parked shot drifts through the whole zoom while the burst is running.
    ///
    /// The scripted camera park (`capture::probe_cam`, decision 0653) is the only caller. It gets a
    /// named method rather than `pub(crate)` fields because an instrument reaching into gameplay is
    /// the allowed direction but not a licence to open gameplay's internals to the whole crate
    /// (decision 1174) — this is the entire surface the probe needs.
    pub(crate) fn park_distance(&mut self, d: f32) {
        self.distance = d;
        self.target_distance = d;
    }

    /// True while a mouse-look drag is active (right- or left-button). The cursor is hidden then.
    pub(crate) fn is_looking(&self) -> bool {
        self.look.is_some()
    }

    /// The self-avatar's render alpha this frame (`1.0` third-person → `0.0` first-person). The
    /// blob shadow multiplies it in for the self unit — the reference's shadow diffuse rides the
    /// same model fade slot the body does (`[model+0x180]`).
    pub(crate) fn self_fade(&self) -> f32 {
        self.self_fade_alpha
    }
}

/// The active mouse-look mode. `Right` turns the character (movement follows the camera heading);
/// `Left` orbits the camera around a stationary character (vanilla left-drag look).
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

/// **The mouse buttons the world owns**, latched at the press — where the player side asks
/// about a primary *press*, instead of `ButtonInput<MouseButton>` (ledger B364). The raw buttons
/// stay readable for the hold and release of a gesture already claimed, which is
/// [`run_look_session`]'s business and no one else's.
///
/// 1.12 never reads the mouse for any of this. It reads two *bindings* — `TurnOrAction` (right)
/// and `CameraOrSelectOrMove` (left) — and a press a UI frame captured never dispatches them: it
/// sets neither of `[InputControl+0x4]`'s mouse bits, engages no look session, and classifies as
/// no [`FollowState`]. That is the observable rule, and it is why a right-click in a bag has never
/// turned anyone. Only the look session applied it here; the command word and the both-button run
/// read the raw buttons, so a right-click on a Who-list row was a `Turn`, whose Smart row is
/// `(0.0, 1.0)` — an immediate return that ran to completion and swung the camera round to behind
/// the character (B364). The same raw read had both primaries over a bag running the avatar
/// forward, through [`super::state::forward_axis`]'s both-button term.
///
/// **Latched, not re-tested each frame**, because the reference latches: the down that reached the
/// world sets the bit, and that button's *up* clears it. A level test would hand the button back
/// to the UI mid-gesture the moment a frame appeared under the (stationary, locked) cursor — an
/// edge on the command word from nothing the player did.
#[derive(Default)]
pub(super) struct WorldMouse {
    /// Held by the world right now, indexed by [`LookButton`].
    held: [bool; 2],
    /// Took its DOWN edge from the world this frame, same index.
    down: [bool; 2],
}

impl WorldMouse {
    /// Is the world holding this button?
    pub(super) fn held(&self, b: LookButton) -> bool {
        self.held[b as usize]
    }

    /// Did the world take this button's DOWN edge this frame?
    pub(super) fn down(&self, b: LookButton) -> bool {
        self.down[b as usize]
    }

    /// Both primaries in the world's hand — vanilla's both-button run, minus the presses the UI ate.
    pub(super) fn both(&self) -> bool {
        self.held(LookButton::Right) && self.held(LookButton::Left)
    }

    /// Latch this frame. `world_press` says whether a DOWN edge *now* belongs to the world; the
    /// held bits then ride to their own release, whatever the cursor is over by then — including
    /// the release a cover synthesises by emptying the button planes, which is what keeps a
    /// loading screen from stranding a latched bit.
    fn update(&mut self, buttons: &ButtonInput<MouseButton>, world_press: bool) {
        for b in [LookButton::Right, LookButton::Left] {
            let i = b as usize;
            self.down[i] = world_press && buttons.just_pressed(b.button());
            self.held[i] = (self.held[i] || self.down[i]) && buttons.pressed(b.button());
        }
    }
}

/// Decide, once per frame, which mouse buttons the world owns — [`CameraControl::world_mouse`].
///
/// **Its own system**, ahead of every reader, rather than a call inside [`super::control`]: the
/// readers are not all in the controller. The look session and the camera's command word are, but
/// `/follow`'s both-button cancel ([`super::follow::steer_follow`]) is a system that runs *before*
/// it — reading a latch the controller wrote would put that cancel a frame behind, and a frame
/// late on an edge-driven cancel is the wrong frame entirely.
///
/// A press belongs to the world when it lands in the viewport off the UI — or whenever a look
/// session already owns the (hidden, locked) cursor, because the second button of a chord joins a
/// gesture the world already has.
pub(super) fn latch_world_mouse(
    buttons: Res<ButtonInput<MouseButton>>,
    // **The raw flag — a press that lands on a V-plate is the PLATE's, not the camera's** (decision
    // 2233, reversing 2159's exception).
    //
    // 2159 read it the other way and excepted plates here, on an inference from the other end:
    // entering freelook disables plate mouse input (`0x60f830`, from `0x483e80`), "a toggle that
    // would have nothing to do if a press on a plate could not reach freelook". The reference
    // refutes that: `0x60f830` has plenty to do for a press that starts on
    // the **world** and then drags the pointer across a plate mid-turn. The press itself never gets
    // there. `0x7662c0` delivers a mouse-down to exactly ONE frame — `[root+0x80]` else
    // `[root+0x7c]` — sets the capture at `0x7663e9`, calls `[vt+0x68]`, and returns 0, stopping
    // the bus walk; and `CBindings::ExecuteBinding 0x4b7990` has exactly six call sites image-wide,
    // every one inside a `CGWorldFrame` vtable handler. So with a plate under the cursor the
    // binding that would start mouselook is never reached, and the release goes to the plate's own
    // click slot (`0x7792d0` → `0x7cb910` → `0x4949f0`, mask 1 select / mask 4 interact — the same
    // two terminals the world right-click's object leg reaches).
    //
    // What it costs is real and is the reference's own cost: **you cannot swing the camera by
    // dragging off a nameplate.** Plates are small dead patches for turning, exactly as in 1.12.
    // The one exception is the plate's own `+0x3c` veto (`0x7cba30`) — while a ground-targeted
    // spell is armed the plates refuse the hit test, the WorldFrame wins the press, and the gesture
    // *does* enter mouselook. That is built where it belongs, in the hit test itself
    // ([`benilla_ui::script::UiScript::set_nameplate_hit_test_veto`]), so it arrives here for free
    // as `PointerOverUi` simply being false over a vetoing plate.
    //
    // `PointerOverUiPanel` keeps its other reader — the wheel still zooms with the cursor on a
    // plate, which is a different law (the wheel walks PAST a frame that merely takes the mouse).
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

#[derive(Component)]
// `pub(crate)` on the TYPE only — the scripted camera park has to name it in a query. Its fields
// stay `pub(super)`; [`FlyCam::park`] is the whole surface an instrument gets (decision 1174).
pub(crate) struct FlyCam {
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) speed: f32,
}

impl FlyCam {
    /// Point the rig at an absolute world yaw/pitch — the scripted camera park's one lever
    /// (`capture::probe_cam`, decision 0653). From here on this is the identical path a mouse-drag
    /// takes.
    pub(crate) fn park(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = pitch;
    }
}

/// The per-model camera-pivot height in **model-local yards, pre-scale** — `attach17.z + 0.0972` (M2
/// attachment id 17) for a character, else `0.9 × vertex-box Z-extent`; the reference's camera-target
/// height (`0x50cbc0`). Stamped on every modeled unit at attach
/// ([`crate::entities`]); `control` reads it off the [`Embodied`], multiplies that body's live scale,
/// and floors at [`CAM_PIVOT_FLOOR`] to get the world pivot the third-person camera looks at (and the
/// first-person eye). `0.0` for a bounds-less display (→ floor).
#[derive(Component, Clone, Copy)]
pub(crate) struct CameraPivot {
    pub height_local: f32,
    /// How far that height drops while this body **swims**, model-local and pre-scale
    /// ([`benilla_formats::M2Bounds::swim_pivot_drop`] — `StandSeq.max.z − SwimSeq.max.z`,
    /// `0x50ccf6`). The reference builds the swim framing-pivot preset `cam+0x124` by subtracting
    /// exactly this from the standing one before the shared clamp, and `0x50f880` picks it whenever
    /// the camera target carries MOVEFLAG_SWIMMING. `0.0` for a model with no Swim sequence (every
    /// non-character model) and for a bounds-less display — the two presets then coincide.
    pub swim_drop_local: f32,
}

/// Mouse-look session state machine — start/stop/hand-off between the two look buttons, cursor
/// grab/stash/restore, and the two click tests that emit [`WorldClick`]/[`WorldRightClick`]. Also
/// applies this frame's accumulated motion as look rotation while a button is held (right-drag syncs
/// the character facing too). Called once per frame from [`super::control`]; `both_buttons` is
/// vanilla's both-button run (steers like a right-drag without its own click test).
///
/// **Orbit and select are independent** (decision 1122): every primary press engages its look
/// session immediately and *also* arms a click test, and the release decides the click on
/// [`PressGesture::is_click`] alone. There is no "promotion" and nothing cancels the click for
/// having moved — the pending click used to be destroyed the moment the cursor crossed a 4 px
/// threshold, which is why a drag could never select (ledger B226).
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
    // A left press this frame the UI already consumed as a cursor-payload world drop (0216 §3) —
    // the left click test must yield to it exactly as it yields to a UI hover, so dropping a held
    // item never also selects.
    click_consumed: bool,
    world_click: &mut MessageWriter<WorldClick>,
    world_right_click: &mut MessageWriter<WorldRightClick>,
    world_right_press: &mut MessageWriter<WorldRightPress>,
    left_click: &mut Option<PressGesture>,
    right_click: &mut Option<PressGesture>,
    look_cfg: LookConfig,
    // The camera-option knobs + this frame's gate facts (decision 2149). `cameraPivot`'s routing
    // lives in the look session because the reference's does: `0x50fee0` IS the mouse-motion
    // handler, and the whole decision is "does THIS motion event go into the pitch or the bias".
    dynamics: &DynamicsInput,
    // Seconds on the app clock — the press predicate's two time gates are measured against it.
    now: f32,
) {
    // The right button's DOWN edge, before any click-vs-drag classification — the reference's
    // WorldFrame OnMouseDown fires at the press whether it becomes a click or a turn. Whether the
    // press was the world's at all is [`latch_world_mouse`]'s single answer, shared with the
    // camera's command word: the viewport off the UI, or a right join into a left-orbit, whose
    // session already owns the cursor. Ground-targeting's cancel reads this edge (decision 0792).
    if rig.world_mouse.down(LookButton::Right) {
        world_right_press.write(WorldRightPress);
    }
    // A chord — both primaries down — is a both-button run, never a select. The reference kills the
    // pending click on the *second* press and refuses to arm a new one while another primary is held
    // (`0x514ac1`, `0x51481a`), so neither release of a chord can dispatch. Cancel both tests.
    // The world's pair, not the device's: what those two sites test is the *binding* state, so a
    // primary a UI frame is holding has never been half of a chord (ledger B364).
    if rig.world_mouse.both() {
        *left_click = None;
        *right_click = None;
    }
    // Both tests just ride their session, accumulating the camera rotation the press has asked for;
    // the release decides. The travel is charged from the *input* delta, before the pitch clamp —
    // the reference accumulates raw device motion, so a drag pinned at the pitch limit still spends
    // its budget.
    let (yaw_rate, pitch_rate) = (look_cfg.yaw_rate(), look_cfg.pitch_rate());
    let dyaw = (mouse_motion.delta.x * yaw_rate).abs();
    let dpitch = (mouse_motion.delta.y * pitch_rate).abs();
    for test in [&mut *left_click, &mut *right_click].into_iter().flatten() {
        test.yaw_travel += dyaw;
        test.pitch_travel += dpitch;
    }

    // Mouse-look start/stop + cursor grab. **Both** buttons engage their look session on the DOWN
    // edge — the reference has no deferral and no engage threshold (`0x51491f`), and the first
    // motion sample already turns the camera. The click test is a separate rider decided at the
    // release, which is exactly what lets one gesture orbit *and* select. Either button hides +
    // locks the cursor in place (so it can't drift out of the window while we turn) and restores it
    // where it was on release. A press that begins over the debug panel is egui's, not ours.
    if let Some(active) = rig.look {
        if !buttons.pressed(active.button()) {
            // The session's button went up: settle its click test against the reference predicate.
            // A handoff release (the other button still held) never fires — the chord already
            // cancelled both tests above.
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
            // The latched button went up. If the *other* look button is still held **by the
            // world** (both-button run → single-button), hand the look session off to it rather
            // than ending it — vanilla keeps turning/orbiting seamlessly on the remaining button,
            // cursor staying hidden throughout. A primary the UI is holding is not a candidate:
            // its binding never fired, so the reference has nothing to hand off to (B364).
            let other = match active {
                LookButton::Right => LookButton::Left,
                LookButton::Left => LookButton::Right,
            };
            if rig.world_mouse.held(other) {
                rig.look = Some(other);
            } else {
                rig.look = None;
                cursor_opts.grab_mode = CursorGrabMode::None;
                // Show the cursor again (cross-platform; on macOS hiding is the cursor subsystem's job).
                cursor_opts.visible = true;
                if let Some(pos) = rig.cursor_stash.take() {
                    window.set_cursor_position(Some(pos));
                }
            }
        }
    } else {
        // A press over the egui dev UI (the overlaid debug panel, the perf pill), over a
        // mouse-enabled player frame, or outside the world viewport is not ours — the whole content
        // of [`WorldMouse`], and what keeps a slider-drag from grabbing the cursor into mouse-look.
        // Right-drag turn. Arms its context-click test too; not armed when left is already down (a
        // chord is never a click).
        if rig.world_mouse.down(LookButton::Right) {
            rig.look = Some(LookButton::Right);
            rig.cursor_stash = window.cursor_position();
            cursor_opts.grab_mode = CursorGrabMode::Locked;
            cursor_opts.visible = false;
            *right_click =
                (!rig.world_mouse.held(LookButton::Left)).then(|| PressGesture::new(now));
        } else if rig.world_mouse.down(LookButton::Left) && !inspect_enabled {
            // Left-drag orbit — engaged on the press, exactly like right, because the reference
            // engages on the press (`0x51491f`). The select is not deferred behind it; it rides
            // along and settles at the release. While the inspector is armed left belongs to it
            // (its own copy-on-click handler), so neither the orbit nor the test starts.
            rig.look = Some(LookButton::Left);
            rig.cursor_stash = window.cursor_position();
            cursor_opts.grab_mode = CursorGrabMode::Locked;
            cursor_opts.visible = false;
            // A press the UI already consumed as a cursor-payload world drop (0216 §3) still orbits
            // — the reference's orbit is unconditional on the down edge — but must not also select.
            *right_click = None;
            *left_click = (!click_consumed && !rig.world_mouse.held(LookButton::Right))
                .then(|| PressGesture::new(now));
        }
    }

    // Apply this frame's accumulated motion as look rotation while a button is held. Right-drag also
    // turns the character (its facing tracks the camera yaw); left-drag leaves the character facing.
    if let Some(active) = rig.look {
        let delta = mouse_motion.delta;
        let d_yaw = -delta.x * yaw_rate;
        cam.yaw += d_yaw;
        // `mouseInvertPitch` flips only the pitch axis (the 1.12 checkbox's whole meaning).
        let dy = if look_cfg.invert_pitch {
            -delta.y
        } else {
            delta.y
        };
        // **The pivot's fork** (`0x50fee0`, decision 2149): a pinned camera looking level-or-up,
        // dragged mostly vertically, spends this delta on the view's pitch BIAS and leaves the
        // arm alone. `None` is the reference's pure-pivot frame — the integrator does not run.
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
    // **The freelook latch.** Right-held is the reference's mouse-look; a left-drag is an orbit and
    // is not freelook. A both-button run steers exactly as a right-drag does, which is the same
    // "the world holds the right button" the `face_yaw` sync above tests — so it counts, and the
    // two tests stay written the same way on purpose.
    rig.freelook = rig.look == Some(LookButton::Right) || (rig.look.is_some() && both_buttons);
}

/// Wheel-zoom: the CAMERAZOOMIN/OUT bindings set a new target orbit distance, and the actual
/// distance glides toward it at a constant `cameraDistanceMoveSpeed` (vanilla's linear,
/// frame-delta-scaled glide — not an ease). Runs every frame regardless of active/detached state,
/// mirroring the reference camera. `scroll` is this frame's net zoom-in amount (wheel notches in
/// line-equivalents — the binding dispatch normalizes trackpad pixels — or the 1.12 key step of
/// 1.0 per press; positive = closer), so a rebound zoom key feels exactly like a wheel notch.
pub(super) fn apply_zoom_scroll(scroll: f32, dt: f32, rig: &mut CameraControl, max: f32) {
    if scroll != 0.0 {
        rig.target_distance =
            (rig.target_distance - scroll * CAM_ZOOM_STEP).clamp(CAM_DIST_MIN, max);
    }
    // Re-clamp every frame, not just on a notch: lowering the Max Camera Distance slider has to
    // pull a camera already sitting past the new ceiling back in, and the glide below then eases
    // it there at the same yd/s a wheel notch would.
    rig.target_distance = rig.target_distance.min(max);
    // Glide the actual distance toward the wheel target at a constant `cameraDistanceMoveSpeed` yd/s,
    // stopping exactly there — the verified vanilla behavior (linear, frame-delta-scaled; not an ease).
    let max_step = CAM_MOVE_SPEED * dt;
    rig.distance += (rig.target_distance - rig.distance).clamp(-max_step, max_step);
}

/// Seat the camera on **whatever the rig is orbiting this frame** — our own body, or a far-sight
/// subject (B151: Mind Vision, Sentry Totem, and Mind Control's camera half in B211) while
/// `PLAYER_FARSIGHT` names one. Three substitutions and then [`seat_camera`]: the orbit centre,
/// the collision sweep's origin, and the pivot height's target.
///
/// It exists because the controller seats the camera from **two** places — the ordinary driving
/// path, and the stand-down path where a spline, a possession or a reseat window owns the body —
/// and far sight outlives all of those (Sentry Totem carries no interrupt flags at all, so you can
/// board a taxi with your view still on the totem). Two copies of the substitution is how one of
/// them silently stops honouring it.
///
/// `feet`/`head` are the caller's, because the head offset is the avatar capsule's and those
/// constants are a movement concern; `body_pivot` is the target height read off the driven body
/// this frame, used only when nothing else is being watched.
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
    // The sweep origin moves with the subject too; rooting it at our own head would cast the boom
    // across the world and jam it on the first wall in between
    // ([`super::view_subject::RemoteView::sweep_origin`]).
    let (orbit_pos, sweep_from) = match view.remote {
        Some(v) => (v.feet, v.sweep_origin()),
        None => (feet, head),
    };
    // The framing height is the **channel's**, not this frame's target: it eases there over
    // `|Δh| / 1.2` s with a cosine profile, so a shapeshift, a mount, a growth aura or a far-sight
    // switch move the camera smoothly instead of teleporting it ([`PivotGlide`], `0x50f160`). A
    // far-sight subject supplies the target the same way the body
    // does — one channel, whatever it is looking at.
    let live_pivot = rig
        .pivot
        .advance(view.remote.map(|v| v.pivot_height).or(body_pivot), dt);
    // **`cameraWaterCollision`'s corridor** — the half 2149 and 2170 each shipped without, and the
    // reason the option is atomic. [`super::camera_water`] carries the block, the byte sites and
    // the why; this is only the wiring.
    //
    // Classified against the channel's **target** (`[cam+0x1c8]`), never its live value — the
    // polarity is easy to invert, and byte-exact. Banding against a continuously-eased quantity
    // chatters on its own, independently of the corridor.
    //
    // `headroom` is `1.0`: the reference scales the reach by the hit fraction of a vertical
    // head-room probe at `0x50e6bd` that this client has never had. Absent and named, not stubbed
    // (1203) — a low ceiling over a swimmer keeps the full reach, which is a pre-existing gap in
    // the pivot rather than something the corridor introduces.
    let (_, pivot_target) = rig.pivot.probe();
    let (band, depth) = super::camera_water::classify(
        // Our own body's cached surface, and only when the camera is watching our own body:
        // `0x511ad0` reads the camera TARGET's liquid object and far sight carries none.
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
    // **And the corridor's floor lifts the SWEEP ORIGIN, which is the half that actually stops the
    // pin.** The reference builds the boom's start from the clamped `*heightOut` itself
    // (`0x50e786`), so in the surface band it sweeps from `surface + 2/9`; this client roots the
    // boom at the capsule's top hemisphere centre instead, a pre-existing divergence that is
    // harmless on land and decisive here. A surface-swimming human male's head sits
    // `surface + 0.171`, and the camera probe has a `0.15` radius — so the probe starts **21 mm**
    // clear of the water it is now allowed to hit, and a fraction of a degree of downward pitch
    // collapses the arm from 15 yd to nothing.
    //
    // Measured, not reasoned: the end-to-end pitch sweep in `benilla_world::collision`
    // (`a_surface_swimmers_arm_moves_smoothly_through_the_whole_pitch_range`) put that step at
    // **6.65 yd** with the corridor applied to the framing pivot alone. Lifting the origin to the
    // corridor floor takes it to a fraction of a yard. `max`, so this can only ever raise the
    // origin: dry land and the submerge band are untouched, and nothing here can push the sweep
    // start down into geometry.
    let sweep_from = Vec3::new(
        sweep_from.x,
        sweep_from.y.max(orbit_pos.y + corridor.floor),
        sweep_from.z,
    );
    // **`cameraTerrainTilt`'s probe and channel.** The probe looks at the ground AHEAD of the
    // subject, not under it: a horizontal ray along its facing from `feet + 5/3`, its hit pulled
    // back `5/18`, then a `64/9` drop — so the slope is the rise of the ground you are walking
    // ONTO over the run to it. Rooted at the subject, which is what makes far sight tilt to the
    // totem's hill rather than to ours.
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
        // `L = √(Δx² + Δy²)` is a length, so a hit closer than the backoff runs the probe
        // *behind* the subject and still divides by a positive run — the reference's own
        // arithmetic, not a guard added here.
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
    // **The mouse-look hand-off** (`0x50d500` push / `0x50d520` pop).
    // Run after the channel has stepped, so an edge hands off the value this frame is about to
    // compose. The reference fires it from the input handler instead; the two differ by at most one
    // frame of channel motion, which at `cameraGroundSmoothSpeed` is a fortieth of a degree.
    let handed = rig.terrain_tilt.hand_off(rig.freelook);
    if handed != 0.0 {
        cam.pitch = (cam.pitch + handed).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    }

    // **`cameraBobbing`'s latch and kernel.** The session is armed off the input-command word, not
    // off this gate, so it runs whatever the CVar says and only the OUTPUT is gated — which is what
    // makes turning the CVar on mid-run start the bob at the phase the session has reached.
    // `rig.distance` and not `collision_distance`: the reference's first conjunct is on the zoom
    // (`[cam+0xec]`), so a camera squeezed against a wall is not thereby in first person.
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

/// Seat the third-person camera: orient it, orbit it behind the avatar's torso with a collision
/// sweep from the head to the ideal seat (snap-in instantly, ease back out), write the resulting
/// transform, and compute the self-avatar zoom-in fade from the realized camera-to-pivot distance.
/// A **keyboard** turn (or the drunk veer, which rides `turn_delta` the same way — decision 1018)
/// carries the camera rigidly — the character's own turns only: a transport
/// deck turning under the rider is frame motion and is applied to `cam.yaw` at the ride block in
/// [`super::control`], bypassing this function's look-session gate (routing it here was the
/// right-drag drift bug — the gate ate the deck's share while a drag was held). A left-drag orbit
/// offset is then reeled back in by the **auto-follow**, on the player's `cameraSmoothStyle`
/// setting ([`FollowStyle`], decision 1493) — or kept forever, on Never.
/// `head`/`player_pos` are precomputed by [`super::control`] (which owns the avatar capsule
/// constants); `cam_pivot_height` is the world pivot height it derived from [`CameraPivot`] this
/// frame.
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
    // A keyboard turn carries the camera RIGIDLY (char and camera rotate as one — the reference
    // look, director's call closing 0050's open "camera follow on turn"): an eased chase of a
    // continuously-turning facing lags by ω/rate, which read as the char angled on screen while
    // run-turning and a release-snap landing off-camera. A drag (`rig.look` held) owns the camera
    // — no INPUT-turn carry against the user's hand. (A transport deck's turn is not an input and
    // never arrives here — the ride block applies it to `cam.yaw` directly, drag or no drag.)
    //
    // **The auto-follow** (1.12's `cameraSmoothStyle`, decisions 1493/1502) rides the same gate
    // for the same reason — a held drag owns the camera, hand on it. It is NOT a per-frame chase:
    // an input edge arms a cosine-smoothstep return to directly-behind and that transition then
    // plays out unattended ([`FollowRig::advance`]). It writes an absolute yaw because our camera
    // stores one; the reference stores the *offset* and re-adds the facing at render time
    // (`0x50f7f2`), which is the same picture and a different mechanism (and the reason Never
    // must not, and here does not, touch the rigid carry above).
    let look_held = rig.look.is_some();
    if !look_held {
        cam.yaw += turn_delta;
    }
    if let Some(yaw) = rig.follow.advance(follow, cam.yaw, dt, look_held) {
        cam.yaw = yaw;
    }
    // Orient the camera, then orbit it behind the avatar's torso. The framing **pivot** is
    // `feet + cam_pivot_height` (model-derived, ~neck height — [`CameraPivot`]); the camera looks at
    // it and, at zoom 0, sits *on* it (first-person eye inside the head). Camera collision is a single
    // sweep of the probe sphere from the player's *head* (the capsule's top hemisphere centre) out to
    // the ideal camera seat (`pivot - fwd·zoom`). The camera rides along that sweep, stopping at the
    // first surface (held off it by the probe radius). Rooting the arm at the head is what makes it
    // robust: body collision keeps the head inside the room — even mid-jump it can't pass the ceiling
    // — so the swept camera can never end up on the far side of a wall or ceiling. That is why a jump
    // in a low room no longer pushes it through the roof: the sweep just stops under the ceiling
    // instead of overshooting (the old min-distance floor used to force the camera *past* a too-close
    // hit — gone; collision wins outright). `cast_move` ignores origin penetration, so a head grazing
    // a surface still casts outward.
    // **The seat is built from the UNBIASED pitch and the view from the biased one** — the whole
    // of `cameraPivot` is that ordering (decision 2149). The reference computes the eye at
    // `0x50edcc → 0x50de00` and stores it, and only *then* rotates the camera basis `[cam+0x14]`
    // by `[cam+0x104]` at `0x50ee32`; so the arm never swings and the look direction does. The
    // bias is zero at every default until a pinned camera is dragged, so `arm_rotation` and
    // `rotation` are the same quaternion on almost every frame.
    let bias = rig.smart_pivot.bias();
    // **The ground tilt IS part of the arm's pitch**, unlike the bias: `0x50f710` composes
    // `[cam+0xf4] + [cam+0x108]` and clamps the SUM to ±89° before the basis is built, and the eye
    // is then seated from that basis. So a followed terrain moves the camera; a smart pivot does
    // not.
    let arm_pitch = (cam.pitch + rig.terrain_tilt.pitch()).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    let arm_rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, arm_pitch, 0.0);
    // **The composite is deliberately NOT re-clamped to ±89°.** The reference's clamp at
    // `0x50f710` binds `[cam+0xf4] + [cam+0x108]` — the pitch channel plus the ground tilt — and
    // the bias is composed **after** it, at `0x50ee58`, so the rendered pitch is not bounded by it.
    // The bound that does apply to the bias is the one-sided
    // one on its own accumulate ([`SmartPivot::route_pitch`]), and the ±89° on the body hand-off
    // (`0x5103e0`), which is where `mover_pitch` takes it.
    let rotation = if bias == 0.0 {
        arm_rotation
    } else {
        Quat::from_euler(EulerRot::YXZ, cam.yaw, arm_pitch + bias, 0.0)
    };
    // `Transform::forward()` is exactly `rotation * -Z` (no renormalize), computed here from the
    // local so the write below can be gated.
    let cam_fwd = arm_rotation * Vec3::NEG_Z;
    let pivot = player_pos + Vec3::Y * cam_pivot_height;
    let seat = pivot - cam_fwd * rig.distance;
    let boom = seat - head;
    let boom_len = boom.length().max(1.0e-3);
    // The camera collides with the WMO *camera/LOS* faces (keeps DETAIL overhangs like forge pipes,
    // drops NOCAMCOLLIDE) + terrain/doodads/GameObjects — its own audience, not the walking mesh.
    //
    // **And the waterline, under `cameraWaterCollision`** — registered `"1"`, so this is on out of
    // the box. `0x50e5ec` ORs the `0xf0000` ADT-liquid nibble into the word all three of
    // `0x50e570`'s queries carry, and it reaches `0x69cc13` through four direct calls, where it
    // gates a Möller–Trumbore intersection (`0x7c2c40`, hit distance written at `0x7c2e5f`) over
    // the chunk's four MCLQ slots. Two-sided, and with no near floor — which is precisely why the
    // pivot corridor above is not optional: nothing in the trace itself stops a boom that starts
    // on the water plane, so the *origin* is what has to be lifted clear.
    //
    // **And that water leg is a RAY, not this probe sphere** (decision 2185): `0x7c2c40` is a
    // ray/triangle test and `0x672170` carries no radius, so the `2/9` yd the corridor lifts the
    // origin by is a clearance budgeted for a point. The probe is `0.3` — it does not fit, and a
    // level boom behind a surface swimmer was coming back pinned at zero. The split lives in
    // [`benilla_world::collision::WorldCollision::cast_camera`], which owns the probe now; the
    // sphere still sweeps the solid world, which is what it was always for.
    let hit = collide.cast_camera(head, boom, dynamics.options.water_collision);
    // The solver's own clip verdict (`0x50e570`'s `0x30000` return, OR'd into `[cam+0x90]` by the
    // driver) — [`SmartPivot`]'s sixth conjunct, and the reason an unobstructed camera never
    // pivots. Written here because here is the only place that knows.
    rig.clipped = hit.is_some();
    let open = hit.unwrap_or(boom_len);
    // Snap in instantly when geometry intrudes (a wall must never sit between camera and character);
    // ease back out to the open arm length once it clears — the vanilla snap-close-then-glide-back.
    rig.collision_distance = if open < rig.collision_distance {
        open
    } else {
        let t = 1.0 - (-CAM_RETURN_RATE * dt).exp();
        rig.collision_distance + (open - rig.collision_distance) * t
    };
    let frac = (rig.collision_distance / boom_len).clamp(0.0, 1.0);
    let seated = head + boom * frac;
    // **The head bob is a pure world-space translation of the eye**, added last, into the same
    // slot [`crate::camera_shake`] writes — which is exactly what the reference does (`0x50eb0f`
    // folds the bob into the shake's accumulator and `0x50de00` applies the pair once). Zero on
    // every frame nothing is bobbing, so the no-op write gate below still holds a parked camera
    // bit-stable.
    let translation = seated + rig.head_bob.offset();
    // The no-op write gate (decision 1362 — 1355's clamp lesson, at the camera): a parked
    // camera's pose is bit-stable once the collision ease settles, but writing it anyway marked
    // the camera's transform changed every frame — which re-ran its propagation and told every
    // camera-watching gate in the app that the view moved when it hadn't. Bit equality, not an
    // epsilon: a real sub-epsilon drift must still land.
    {
        let t = cam_t.bypass_change_detection();
        if t.rotation != rotation || t.translation != translation {
            t.rotation = rotation;
            t.translation = translation;
            cam_t.set_changed();
        }
    }
    // No waterline handling here — deliberately. The reference NEVER moves the eye for liquid
    // (zero liquid-height queries in the camera TU `[0x7ac640, 0x7ae010)`); the no-straddle
    // experience is the *submersion probe's* — the frame flips
    // submerged the moment the lowest near-plane corner reaches the surface
    // (`liquid::detect_submersion`, the corner-min probe), and with the near plane at the
    // `nearclip` CVar's registered 0.1 the whole crossing band is a few inches tall (2163 — it
    // said "the reference's 1/9" until the per-frame re-stamp at `0x511bd4` was read). 0905's eye
    // snap — the local compensation for the old 1.0-yd near plane — is removed with its cause
    // (its record is superseded; see the 0905-successor decision).
    // `WOW_CAM_DUMP=frame`: the REALIZED pose, per frame, bit-exact — not the pose that was asked for.
    //
    // Every scripted probe sets `yaw`/`pitch`/`distance` and we then reason as though the camera is
    // therefore where we put it. It is not: `collision_distance` is an exponentially-eased chase of a
    // per-frame collision CAST, so a grazing hit that alternates gives an arm that snaps in and eases
    // back out, and the camera keeps moving for as long as that lasts — with the scripted pose
    // perfectly constant the whole time. B38's "the camera is static by construction, so nothing
    // camera-derived can be the cause" (0671) rests entirely on that being untrue, and it was never
    // measured. `open` is printed beside the eased arm so a hit/miss alternation in the CAST is
    // visible even on a frame where the ease has not yet moved the camera far enough to see.
    if cam_dump_enabled() {
        // `follow=` is the auto-follow's own reading (1502): the offset the return is animating,
        // the state the input word classifies to, and — once armed — how far through the
        // transition this frame is. `off` moving while `arm` reads `-` means something other than
        // the follow moved the camera.
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

    // The pivot bias's own per-frame half (`0x50ed77` → `0x5107f0`): with the gate no longer true
    // the bias eases back to zero at `cameraTargetSmoothSpeed`; with it true an ease in flight is
    // cancelled where it stands. Runs after the sweep, so `rig.clipped` is this frame's.
    rig.smart_pivot.advance(
        cam.pitch,
        &dynamics.subject,
        rig.clipped,
        dynamics.tracking_style,
        &dynamics.options,
        dt,
    );

    // Fade the avatar as the camera nears its pivot (zoom-in / a wall pulling the boom in): opaque
    // in third-person, ramping to invisible in first-person. Keyed off the *realized* camera→pivot
    // distance (collision-pulled), so backing into a wall also thins you — the faithful behavior.
    // Off the SEATED eye, not the bobbed one: the fade is a statement about how far the boom was
    // pulled in, and a 5 cm wobble is not that.
    //
    // The LIVE `nearclip`, not a constant (2163): the fade is defined as finishing where the near
    // plane starts cutting, so a player who moves that plane has to move this with it.
    rig.self_fade_alpha = self_model_fade_alpha(
        (seated - pivot).length(),
        dynamics.nearclip,
        SELF_FADE_WINDOW,
    );
}

/// Apply the self-avatar zoom-in fade ([`CameraControl::self_fade_alpha`], computed in [`control`]) to
/// the player's own body parts **and every attach-model descendant** (held items, helm, shoulders —
/// [`crate::entities::BoneAttach`] rides them several levels down through the joint hierarchy), so you
/// go translucent then invisible — weapon and armor included — as the camera zooms into the head. Drives
/// the same per-instance render-alpha channel as [`benilla_world::model_fade::apply_render_fade`] — the `MeshTag`
/// alpha field on the blend-twin material — and hard-hides via [`Visibility`] at α 0 (true
/// first-person; cheaper + cleaner than a ≈0-alpha head sitting on the camera).
///
/// Runs **after** the interior classifier + the appear/despawn fades so its override wins the frame; it
/// overrides while fading (`α < 1`) and, on the frame the fade ends, **releases** the channel back
/// (decision 0213): the classifier skips settled parts and rewrites only on a classification change, so
/// without an explicit hand-back a fade episode that ends in a jump past 1 (a hitch frame closing the
/// camera ease in one step, a pivot jump) left the avatar latched on the blend twin at its last low alpha
/// — stuck translucent until the player happened to cross a room boundary. At steady `α ≥ 1` it does
/// nothing, leaving the classifier the sole steady-state author. Parts mid appear/despawn fade
/// (`RenderFade`/`PendingAppearFade`) are left to that fade — it's brief, owns the channel, and performs
/// its own release on completion.
///
/// Walks the **full** descendant tree from the avatar root rather than just its direct children: body
/// submeshes are direct children of the root, but a held item / helm / shoulder is a child of a joint
/// entity (itself a descendant of the root, at varying depth) — an earlier direct-`Children`-only version
/// silently skipped every attach model. The self-player entity is singular, so a per-frame tree walk over
/// its handful of joints + submeshes is nil cost.
///
/// **The tree is not the whole model.** An M2's BILLBOARD batches can't be tree children — their mesh is
/// centred on the bone pivot and their transform belongs to the billboard system, so every one of them is
/// a world ROOT entity that merely *follows* an anchor inside the tree (decision 0153). The descendant
/// walk therefore cannot see them, and the night-elf eye glow — two additive `…EYEGLOW.BLP` billboard
/// quads at head height — went on burning in mid-air after the body it belongs to had gone (reported
/// first-hand; ledger B71). Cards are picked up here by testing their follow-anchor against the walked
/// set, and folded into the same α — the idiom [`crate::blob_shadow`] already uses for the other
/// world-root follower of the self avatar ("the self first-person fade rides the same model-fade slot in
/// the reference"). One multiply covers both halves: it feathers with the body, and at α 0 the additive
/// compose (`wow_model.wgsl`: `out_rgb *= faded_alpha`) takes the card to black, which for an ADD blend
/// is gone. That deliberately avoids `Visibility`, which the card's own hidden-owner mirror authors every
/// frame in a different system.
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
            // Disjointness for the card query below (both want `&mut MeshTag`). A card carries
            // `FadeMaterials` too since 0836, so this now genuinely diverts them — into the loop
            // at the end, which applies the same law without touching `Visibility` (a card's own
            // hidden-owner mirror authors that in a different system).
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
    // The water-plane axis, composed into every pick below (`far_resolved`) like every other
    // owner of the handle channel — the feather and the classifier converge, never re-swap.
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
    // Our own live aura translucency (stealth, invisibility, ghost — `crate::aura_visual`) is a
    // FACTOR of this fade, not a rival author: this system runs last on the self body and writes the
    // alpha field verbatim, so a zoom-in while stealthed must carry the aura's term or the feather
    // would silently re-opaque the character to 1.0 × the camera ramp. Folding it in here also makes
    // the release edge honest — it releases at the *product*, so a fade ending while still stealthed
    // hands the material back only if the body is genuinely opaque again.
    let feather = rig.self_fade_alpha * crate::aura_visual::root_alpha(aura);
    // The walked set doubles as "which anchors belong to this model" for the card pass — built only
    // while fading, so the steady state (the early return above) never pays for it.
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
        // The card's steady author is `entities::apply_unit_mat_alpha`, ordered before this system,
        // which writes the batch's per-sequence factor every frame — so composing from `current`
        // (not from the tag we'd read back) keeps that animation alive under the fade, and the
        // release frame's `α = 1` write lands exactly on the value it would have had.
        let authored = anim.map_or(1.0, |a| a.current);
        let bits = benilla_world::mesh_tag::with_alpha(tag.0, authored * alpha);
        if tag.0 != bits {
            tag.0 = bits;
        }
        // …and the blend twin while feathering, exactly as a mesh part does. The alpha alone is
        // enough for an ADDITIVE card (`wow_model.wgsl` folds it into the colour, so α 0 is black
        // is gone), but an OPAQUE one — a pauldron's camera-facing trim, a chain link — ignores it
        // entirely and stayed solid in first person until the card carried a twin to swap to
        // (decision 0836). No `Visibility` here: that channel belongs to the card's hidden-owner
        // mirror in another system.
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

/// Depth-first helper for [`apply_self_model_fade`]: apply the fade (or, at `α ≥ 1`, the release) to
/// `entity` if it's a fadeable part, then recurse into its children regardless (a joint or an attach-model
/// root carries no `FadeMaterials` itself but must still be descended through to reach the mesh leaves
/// under it). Every entity visited — parts, joints, attach roots, billboard anchors alike — is recorded
/// in `walked`, which the caller uses to recognise the world-root billboard cards that follow this model.
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
            // The release edge (runs once, on the frame the fade ends — decision 0213): un-hide,
            // restore the alpha field this system owns, and hand the material back to the part's
            // law. The alpha restore is unconditional: the classifier's payload writes carry the
            // tag's alpha through since 0755 (that is what lets a part re-lane mid-fade), so
            // leaning on its re-author to *also* re-opaque the avatar — as this used to — would
            // leave it stuck translucent, the exact 0213 bug.
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
            // A classifier-lit part is still enqueued so the next run re-asserts its full payload
            // (probe slot / fog bit) over whatever this feather episode wrote — 0734's queue.
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
            // Feathering: ride the blend twin with the alpha packed into the tag's alpha field
            // (the cutout ignores α; `with_alpha` preserves the ground-shade byte so a shadowed
            // avatar doesn't flash lit while zooming).
            let bits = benilla_world::mesh_tag::with_alpha(tag.0, alpha);
            if tag.0 != bits {
                tag.0 = bits;
            }
            // A bake-classified part feathers on the PROBE-lit blend twin — the room light
            // rides the fade (the tag re-lane keeps the slot alongside the alpha, 0355); the
            // exterior twin at shade byte 0 read as full outdoor intensity deep indoors
            // (director-caught, 2026-07-13). Shared with the appear/despawn ramp since 0755, so
            // the two can never disagree about which twin a law wants.
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

/// True if the OS pointer is over the world camera's render area. The world camera now fills the window
/// (the debug panel overlays rather than insetting), so this is really just "is the cursor inside the
/// window?"; the panel itself is excluded by `PointerOverUi` at the call site. Kept viewport-aware in
/// case anything insets the camera again.
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

/// Free-fly (pre-connect or `F`-detached): aim from the look angles, move the camera directly —
/// WASD in the camera basis, Space/C up/down, Ctrl 5× boost. The avatar stays frozen where it was;
/// [`super::control`] parks the mover before calling so the wire never extrapolates a phantom walk.
pub(super) fn fly_free(
    dt: f32,
    keys: &ButtonInput<KeyCode>,
    typing: bool,
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    cam_t: &mut Transform,
) {
    let keys_pressed = |k: KeyCode| !typing && keys.pressed(k);
    // Detached / pre-connect: keep the avatar fully opaque (you flew off to look at it — no fade).
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

/// `$WOW_CAM_DUMP` — the per-frame camera/turn dump (this file's seat and the controller's turn
/// line share it). One read for the process: both sites sit on the every-frame path.
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

    /// Step a [`PivotGlide`] at 60 Hz for `secs`, holding the target, and return the heights it
    /// passed through (one per frame).
    fn glide_run(g: &mut PivotGlide, target: Option<f32>, secs: f32) -> Vec<f32> {
        let dt = 1.0 / 60.0;
        (0..(secs / dt).round() as usize)
            .map(|_| g.advance(target, dt))
            .collect()
    }

    /// **The report** (the director, on the reference vs ours): shifting form on the real client
    /// *glides* the camera to the new body's height; ours snapped there and then drifted. The
    /// channel's whole job is that this is one smooth move, in **both** directions — the solver's
    /// `max(target, live)` is only a collision seed, and the far chain clamps back to the live
    /// value (`0x50e767`), so nothing about a *rising* target arrives early.
    #[test]
    fn a_shapeshift_glides_the_pivot_both_ways_and_never_snaps() {
        // Tauren → cat: the heights measured off a live probe run.
        let (tauren, cat) = (2.4659_f32, 1.0552_f32);
        let mut g = PivotGlide::default();
        // The first height a camera ever sees is established, not travelled to (`0x5127d4`).
        assert_eq!(
            g.advance(Some(tauren), 1.0 / 60.0),
            tauren,
            "the first arm snaps"
        );

        for (from, to) in [(tauren, cat), (cat, tauren)] {
            let expected = (to - from).abs() / CAM_PIVOT_SMOOTH_SPEED;
            let frames = glide_run(&mut g, Some(to), expected * 2.0);
            // It arrives, and only at the end.
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
            // No frame jumps: the biggest single step is the cosine's own midpoint rate, which
            // for these Δ is far under half the total. A snap would put the whole Δ in one frame.
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

    /// A model that has not resolved yet **holds** the channel — the reference skips the camera
    /// update outright while the preset is stale (`0x50e907`), which is what makes a display swap
    /// read as a pause and then one glide. Aiming at a placeholder in the meantime would send the
    /// camera on a round trip.
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

    /// The per-frame re-arm has to be a **no-op** in steady state (the setter's 0.001 epsilon,
    /// `0x5126b0`). Restarting the move from its own midpoint every frame would stretch it
    /// asymptotically and it would never arrive — the classic re-arm bug this epsilon prevents.
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

    /// The pivot target is the model height × the **raw** scale, clamped to the reference's own
    /// `[5/6, 15]` band (`0x50ca90`) — a giant's aura cannot walk the framing pivot into the sky,
    /// and a shrink cannot bury it in the floor.
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

    /// **The swim preset** — the reference's `cam+0x124`, selected by `0x50f880` on
    /// MOVEFLAG_SWIMMING and built at `0x50ccf6` as the standing height less
    /// `StandSeq.max.z − SwimSeq.max.z`. The numbers are the shipped Human Male's, measured off
    /// the binary: standing 1.9002692, swimming 1.5120120.
    #[test]
    fn swimming_takes_the_lower_pivot_preset() {
        let human = CameraPivot {
            height_local: 1.9002692,
            swim_drop_local: 0.3882572,
        };
        assert!((model_pivot_height(&human, 1.0, false) - 1.9002692).abs() < 1e-5);
        assert!((model_pivot_height(&human, 1.0, true) - 1.512_012).abs() < 1e-5);
        // The preset multiplies the scale, then clamps — the swim leg shares the band with the
        // standing one (`0x50ca90` clamps all three presets together).
        assert!(
            (model_pivot_height(&human, 2.0, true) - 2.0 * 1.512_012).abs() < 1e-5,
            "the swim preset scales like its sibling"
        );
        assert_eq!(model_pivot_height(&human, 0.01, true), CAM_PIVOT_FLOOR);
        assert_eq!(model_pivot_height(&human, 100.0, true), CAM_PIVOT_CEIL);
    }

    /// A model with **no Swim sequence** — every non-character model, and the reference's own
    /// both-sequences-present guard (`0x711960` on ids 0 and 0x2a). Its drop is `0.0`, so the swim
    /// preset degenerates to the standing one and a body that cannot swim never dips.
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

    /// A press that has travelled `yaw`/`pitch` **degrees** of camera rotation.
    fn press(yaw_deg: f32, pitch_deg: f32) -> PressGesture {
        PressGesture {
            at: 0.0,
            yaw_travel: yaw_deg.to_radians(),
            pitch_travel: pitch_deg.to_radians(),
        }
    }

    /// The report this whole change exists for (ledger B226, decision 1122): **a fast click
    /// selects however far the mouse swept.** Under 200 world the reference asks nothing about
    /// motion at all (`0x514ae0`'s first arm, `0x514b24`) — which is the gesture people actually
    /// make, flicking the cursor at a mob and clicking on arrival with the hand still moving.
    /// benilla used to destroy the pending click after 4 px of travel, so this case never fired.
    #[test]
    fn a_fast_click_selects_however_far_the_mouse_swept() {
        // A whole screen's worth of sweep — orders of magnitude past the travel gate.
        let swept = press(90.0, 45.0);
        assert!(
            swept.is_click(0.199),
            "under 200 world, travel is not consulted"
        );
        // And the camera is expected to have orbited through all of it: the two are independent,
        // which is the half that makes the reference's gesture possible at all.
    }

    /// The `mousespeed` slider is a MULTIPLIER over the shipped per-pixel rate (decision 1140), and
    /// the neutral notch has to reproduce the old constant exactly — the whole point of registering
    /// the default at 1.0 is that nobody's feel changes until they move the slider. Both the look
    /// rotation and the click-vs-drag travel budget read this one function, so the drag threshold
    /// scales with the pointer instead of drifting away from it.
    #[test]
    fn the_sensitivity_slider_is_a_multiplier_over_the_shipped_rate() {
        // **The shipped feel is unchanged by the two move-speed CVars landing**: at the
        // reference's own defaults BOTH axes come out at exactly the rate this client has always
        // used. That is the whole point of anchoring the scale to `LOOK_SENSITIVITY` rather than
        // transplanting the reference's deg-per-accelerated-pixel onto our raw device delta.
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

    /// **Each move-speed CVar scales its own axis, linearly, and only its own** — the reference's
    /// law shape (`Δyaw_deg = value × Δx / 800`, `Δpitch_deg = value × Δy / 600`).
    ///
    /// The slider's own stops are the check: `UIOptionsFrameSliders`' MOUSE_LOOK_SPEED row runs
    /// 90…270, so its ends are half and one-and-a-half times the shipped rate.
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

        // The MOUSE_LOOK_SPEED slider's two ends. Approximate, because the two sides multiply
        // the same three factors in a different order and f32 is not associative — the property
        // under test is the ratio, not the last bit.
        let near = |a: f32, b: f32| (a - b).abs() < 1e-9;
        for (speed, factor) in [(90.0_f32, 0.5_f32), (270.0, 1.5)] {
            let at = LookConfig {
                yaw_speed: speed,
                ..Default::default()
            };
            assert!(near(at.yaw_rate(), LOOK_SENSITIVITY * factor));
        }

        // `mousespeed` still multiplies on top of both — the 1140 property, unchanged.
        let both = LookConfig {
            yaw_speed: 270.0,
            sensitivity: 1.5,
            ..Default::default()
        };
        assert!(near(both.yaw_rate(), LOOK_SENSITIVITY * 1.5 * 1.5));
    }

    /// **The auto-follow** (decisions 1493/1502) — 1.12's `cameraSmoothStyle`, the setting benilla
    /// spent its whole life behaving as "Never". The properties that a re-derivation gets wrong,
    /// and that the reference's mechanism (`0x510960`, `0x50f160`) turns on:
    /// it is armed by an input **edge** and then plays out unattended (not a per-frame chase of a
    /// moving target), the profile is a **cosine** smoothstep, the duration is `|Δ| / rate ×
    /// factor` **clamped to [0.1 s, 2.0 s]**, and Smart's `Idle`/`Stop` rows are a *cancel* — which
    /// is what "stays where you put it, except while you're moving" actually is.
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

        // ── Smart: standing still with the camera dragged aside, nothing happens; the moment the
        // W edge lands, one armed transition brings it home over |Δ|/180°/s = 0.5 s.
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
        // And holding W changes nothing further — the transition is spent, not a standing chase.
        let still_home = run(&mut rig, c, follow_cmd::FORWARD, home + 0.4, 1.0);
        assert_eq!(
            still_home,
            home + 0.4,
            "a HELD key re-arms nothing: only edges arm"
        );

        // ── Smart: releasing W (an edge into Stop) cancels rather than arming, so a camera nudged
        // while stopping stays nudged.
        let mut rig = FollowRig::default();
        let held = run(&mut rig, c, follow_cmd::FORWARD, 0.0, 0.1);
        let released = run(&mut rig, c, 0, held + OFFSET, 0.5);
        assert_eq!(released, held + OFFSET, "Stop is a cancel under Smart");

        // ── Always arms on that very same Idle edge — the one row where the two styles differ.
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

        // ── The duration floor: a 5° correction is 0.028 s of travel at 180 °/s, and the
        // reference's 0.1 s minimum overrides it — so it is NOT finished after 0.05 s.
        let mut rig = FollowRig::default();
        let small = 5.0_f32.to_radians();
        let _ = run(&mut rig, c, 0, small, DT);
        let mid = run(&mut rig, c, follow_cmd::FORWARD, small, 0.05);
        assert!(
            mid.abs() > 1.0e-4,
            "the 0.1 s floor is doing the work: {mid}"
        );
        assert!(run(&mut rig, c, follow_cmd::FORWARD, mid, 0.06).abs() < 1.0e-4);

        // ── Track (a taxi, a spline): Smart takes it lazily — 0.4 s of dead time first, then a
        // factor-10 return the 2 s ceiling caps.
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

        // ── …and entering the drag **cancels** what was in flight (`0x50fe30` zeroes the
        // descriptors), so the return does not simply resume when the button comes up. Grab the
        // camera mid-swing and it stays where the hand left it until the next input edge.
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
        // …and the release's own edge (the mouse bit leaving the word) is what starts a new one.
        assert!(
            run(&mut rig, c, follow_cmd::FORWARD, parked, 1.0).abs() < 1.0e-4,
            "the release edge arms a fresh return"
        );
    }

    /// Right-clicking a row of the Who list must not swing the camera round to
    /// behind the character — over ~180°, on a body that never turned. The click itself was the
    /// UI's (1816's no-fall-through hit test opened the dropdown); only the camera's command word
    /// saw it, because the word's two mouse bits were built from `ButtonInput` with no UI term.
    /// `RIGHT_MOUSE` alone is a [`FollowState::Turn`], and Smart's Turn row is `(0.0, 1.0)` — no
    /// delay, full factor — so the return armed on the press edge and ran to completion.
    ///
    /// The decode is pinned end to end, from the latch to the row the classifier picks, because
    /// each half was individually right: the classifier is the reference's (the test below), and
    /// the look session's own gate was already there. What was missing was the word reading the
    /// same gate.
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

        // The report: the press landed on a UI row.
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

    /// The latch's own law: a button is claimed at the DOWN edge and held to its own release.
    ///
    /// Level-testing "is the cursor over UI *right now*" instead would drop the bit mid-drag the
    /// moment a frame appeared under the (stationary, locked) cursor — a phantom edge on the
    /// command word, which is exactly what arms a return. And the second button of a chord has to
    /// join: the reference's both-button run is both bindings held, and the second press lands on
    /// a cursor the first one already hid.
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

        // …and the release is what ends it — including the synthetic one a cover produces by
        // emptying the button planes, which is a release of both without a `just_released`.
        buttons = ButtonInput::<MouseButton>::default();
        rig.world_mouse.update(&buttons, false);
        assert!(!rig.world_mouse.both());
        assert!(!rig.world_mouse.held(LookButton::Right));
        assert!(!rig.world_mouse.held(LookButton::Left));
    }

    /// The state classifier's three vanilla input rules (`0x510960`) —
    /// the ones a client that keys off character *velocity* cannot reproduce, because they are
    /// read off the camera's own command word: right-mouse alone is a `Turn`, a turn key held
    /// **under** right-mouse is a `Strafe`, and both mouse buttons together are a `Move`. Plus the
    /// priority order, which is what decides the row when several are true at once.
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
        // …and the tracking style is the selector the moment Track or Fear is merely PRESENT,
        // even when a higher-priority state supplies the row.
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

    /// The enum is the ENGINE's (0/1/2), not the 1/2/3 the reference's own dropdown writes — and
    /// that stray `3` still has to mean Never, because that is what whoever wrote it meant.
    #[test]
    fn the_follow_style_enum_is_the_engines_and_tolerates_the_dropdowns_stray() {
        assert_eq!(FollowStyle::from_cvar(0.0), FollowStyle::Never);
        assert_eq!(FollowStyle::from_cvar(1.0), FollowStyle::Smart);
        assert_eq!(FollowStyle::from_cvar(2.0), FollowStyle::Always);
        assert_eq!(FollowStyle::from_cvar(3.0), FollowStyle::Never);
        // Off the ladder entirely: the registrar default, not a dead camera.
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

    /// The second arm: between 200 and 800 world the travel gate applies, per axis and independently
    /// (`0x514ae0`'s `both accums < 8.0` arm). The thresholds are the reference's 2.25° of yaw and
    /// 2.0° of pitch — see [`CLICK_HOLD_CEILING`] for why we hold the angle, not the raw literal.
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

    /// The 800 world ceiling (`0x514aeb lea eax,[edx-0x320]`) is absolute — a long hold is never a
    /// click, however still the hand was. This is the arm that keeps a deliberate camera orbit from
    /// re-targeting whatever it started on, and it is the reason the fix could not simply be
    /// "always select on release".
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

    /// The self-avatar's zoom-to-first-person fade reaches its BILLBOARD cards — the night-elf eye
    /// glow (ledger B71: two additive quads left burning in mid-air after the body was hidden).
    /// A card is a world ROOT following an anchor inside the model (decision 0153), so the fade's
    /// descendant walk can only claim it through that anchor — and must claim ONLY its own: every
    /// brazier and lamppost in the zone is a card too, and dimming those with the player's zoom
    /// would be a far worse bug than the one being fixed.
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
        // The water-plane twin map the feather composes with (empty — no water in a fixture).
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

        // Mid-feather: the glow rides the body's alpha down.
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

        // Back out to third person — the release frame hands the authored value back.
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
