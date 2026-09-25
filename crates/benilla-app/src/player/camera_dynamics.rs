//! **The 1.12 camera option toggles** — the four `UIOptionsFrame` checkboxes `FOLLOW_TERRAIN` /
//! `HEAD_BOB` / `SMART_PIVOT` / `WATER_COLLISION` and the mechanisms behind them.
//!
//! Each is a CVar the reference registers and this client ticked at nothing: the census in
//! `ui_script::options_tests` had all four on its unbacked list with a byte-level spec and no
//! feature.
//!
//! **The sign convention is the one thing to keep straight.** The reference's pitch is positive
//! **downward** (`0x511f40`: `row 0 = (cos y·cos p, sin y·cos p, −sin p)`, `0x50de00`: `eye =
//! pivot − dist·forward`, so `+89°` is the eye above the target looking down). benilla's
//! [`super::camera::FlyCam::pitch`] is a Bevy `EulerRot::YXZ` X-rotation with `forward = rot ·
//! −Z`, so **positive is upward** — the exact negation. Every reference predicate below is
//! therefore transcribed with its comparison flipped, and the flip is written out at each site
//! rather than hidden in a conversion, because a silently mirrored inequality is the failure mode
//! this file exists to avoid.

use bevy::prelude::*;

use super::camera_channel::{Arm, SmoothChannel, CHANNEL_EPS};

/// `cameraPivotDXMax`'s registered default — radians of **yaw** per motion event, above which a
/// drag is not "mostly vertical" any more (`0.05` ≈ 2.86°, `[0xbe0f30]`).
pub(crate) const PIVOT_DX_MAX_DEFAULT: f32 = 0.05;
/// `cameraPivotDYMin`'s registered default — radians of **pitch** below which a drag does not
/// engage the pivot at all (`0.0`, `[0xbe0cec]`: any vertical motion qualifies).
pub(crate) const PIVOT_DY_MIN_DEFAULT: f32 = 0.0;
/// `cameraTargetSmoothSpeed`'s registered default, deg/s — the rate the pitch bias eases back to
/// zero at once the pivot lets go (`90.0`, `[0xbe0fc8]`).
pub(crate) const TARGET_SMOOTH_SPEED_DEFAULT: f32 = 90.0;

/// `cameraGroundSmoothSpeed`'s registered default, deg/s — `[0xbe0fc0]`, `"7.5"`.
pub(crate) const GROUND_SMOOTH_SPEED_DEFAULT: f32 = 7.5;
/// `cameraTerrainTiltTimeMin`'s registered default, seconds — `[0xbe1050]`, `"3.0"`.
pub(crate) const TILT_TIME_MIN_DEFAULT: f32 = 3.0;
/// `cameraTerrainTiltTimeMax`'s registered default, seconds — `[0xbe1054]`, `"10.0"`.
pub(crate) const TILT_TIME_MAX_DEFAULT: f32 = 10.0;

/// `cameraBobbingLRAmplitude` / `cameraBobbingUDAmplitude`'s registered default — `[0xbe1064]` /
/// `[0xbe10c8]`, both `"2.0"`. Scaled by [`BOB_AMPLITUDE_SCALE`] to yards.
pub(crate) const BOB_AMPLITUDE_DEFAULT: f32 = 2.0;
/// `cameraBobbingFrequency`'s registered default, Hz at unit speed factor — `[0xbe0cf8]`, `"0.8"`.
pub(crate) const BOB_FREQUENCY_DEFAULT: f32 = 0.8;
/// `cameraBobbingSmoothSpeed`'s registered default — `[0xbe10bc]`, `"0.8"`. **Not a bob rate**: it
/// is the DECAY rate, and its one image-wide read is in the disarm (`0x51113a`).
pub(crate) const BOB_SMOOTH_SPEED_DEFAULT: f32 = 0.8;
/// What the two amplitude CVars are in — `[0x7ff9d0] = 1/36`. The columns are inches; yards out.
const BOB_AMPLITUDE_SCALE: f32 = 1.0 / 36.0;
/// The speed the bob's rate factor is measured against — `[0x808a08] = 7.2` yd/s.
const BOB_SPEED_DIVISOR: f32 = 7.2;
/// The rate factor's clamp — `[0x8089a0] = 0.5` (LOWER) and `[0x80899c] = 1.5` (upper). **Stored in
/// the order a reader would not assume**: the lower bound sits at the higher address, and it is the
/// substituting block at `0x5119be`, not the address order, that says which is which.
const BOB_SPEED_CLAMP: (f32, f32) = (0.5, 1.5);
/// Head bob's first conjunct — `[cam+0xec] <= [0x8089ac] = 1/6`, INCLUSIVE. It is a compare on the
/// ZOOM distance, so "first person" here is the wheel being all the way in, not the renderer's own
/// first-person switch (`dist − [cam+0x38] <= 1/360`).
pub(crate) const BOB_FIRST_PERSON_DISTANCE: f32 = 1.0 / 6.0;

/// The camera option knobs — the CVar-backed half of this module (each default is
/// the reference's registrar value, and [`crate::cvars`] carries the provenance per row).
#[derive(Resource, Clone, Copy, Debug)]
pub(crate) struct CameraOptions {
    /// `cameraPivot` — registered **"1"**, so this is ON out of the box and benilla was the
    /// divergence until it was built.
    pub(crate) pivot: bool,
    /// `cameraPivotDXMax` — see [`PIVOT_DX_MAX_DEFAULT`].
    pub(crate) pivot_dx_max: f32,
    /// `cameraPivotDYMin` — see [`PIVOT_DY_MIN_DEFAULT`].
    pub(crate) pivot_dy_min: f32,
    /// `cameraTargetSmoothSpeed` — see [`TARGET_SMOOTH_SPEED_DEFAULT`].
    pub(crate) target_smooth_speed: f32,
    /// `cameraTerrainTilt` — registered **"0"**, so Follow Terrain is OFF out of the box.
    pub(crate) terrain_tilt: bool,
    /// `cameraGroundSmoothSpeed`, deg/s — the ground channel's rate (`[0xbe0fc0]`, `"7.5"`).
    pub(crate) ground_smooth_speed: f32,
    /// `cameraTerrainTiltTimeMin`/`Max`, seconds — the ground channel's duration bound, each
    /// scaled by the row's `Factor` (`[0xbe1050]`/`[0xbe1054]`, `"3.0"`/`"10.0"`).
    pub(crate) tilt_time_min: f32,
    pub(crate) tilt_time_max: f32,
    /// `cameraBobbing` — registered **"0"**, so head bob is OFF out of the box.
    pub(crate) bobbing: bool,
    /// `cameraBobbingLRAmplitude` / `cameraBobbingUDAmplitude`, in the CVars' own units.
    pub(crate) bob_lr_amplitude: f32,
    pub(crate) bob_ud_amplitude: f32,
    /// `cameraBobbingFrequency`.
    pub(crate) bob_frequency: f32,
    /// `cameraBobbingSmoothSpeed` — the DECAY rate, see [`BOB_SMOOTH_SPEED_DEFAULT`].
    pub(crate) bob_smooth_speed: f32,
    /// **`cameraWaterCollision`** — registered **"1"** (`0x50bd63`, default string `0x82e748`), so
    /// this is ON out of the box.
    ///
    /// **It is one CVar with two consumers, and shipping either alone is a visible defect.** The
    /// read at `0x50e5ec` produces one register: its `0xf0000` nibble rides the trace mask to all
    /// three of `0x50e570`'s queries (the boom hits a bare waterline), and `0x50e629` tests the
    /// *same* register to admit the pivot floor/cap block ([`super::camera_water`]).
    ///
    /// This tree has shipped each half on its own and broken the camera both times — 2149 the
    /// corridor without the trace, 2170 the trace without the corridor (2173 §1: a surface
    /// swimmer's pivot sits 11 mm under the plane, so the boom straddles it every frame). The two
    /// are inseparable: the corridor's surface arm is what lifts the sweep origin to
    /// `surface + 2/9`, clear of the geometry the mask just switched on.
    pub(crate) water_collision: bool,
}

/// The camera options' change callback (2149, 2303). The numeric rows take the value
/// straight: the reference's own validator on them is `0x50b330`'s range REFUSAL, which is the
/// camera-speed rows' business ([`super::camera::on_cvar`]), not these.
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

/// The camera-dynamics inputs one frame hands the rig — the knobs, plus the facts about the
/// *followed unit* that the mechanisms' gates need and the camera does not hold.
#[derive(Clone, Copy, Debug)]
pub(super) struct DynamicsInput {
    pub(super) options: CameraOptions,
    /// `cameraSmoothStyle` **raw** — the terrain-tilt matrix indexes the un-remapped value
    /// (`0x50dbc0`), so this is the knob and not the auto-follow's own
    /// far-sight-adjusted copy.
    pub(super) smooth_style: super::camera::FollowStyle,
    /// `cameraSmoothTrackingStyle` — read by exactly one thing here, [`SmartPivot::advance`]'s
    /// third release conjunct (`0x511010`). Separate from `smooth_style` because the reference
    /// keeps two registrations (`[0xbe10c4]` vs `[0xbe1098]`) off the same default pointer.
    pub(super) tracking_style: super::camera::FollowStyle,
    pub(super) subject: SubjectState,
    /// The live `nearclip` ([`benilla_world::view::ViewDistance::nearclip`]) — the self-avatar
    /// fade's reference plane, carried on this bundle rather than as a fourteenth `seat_camera`
    /// argument. It rides here because it is a camera CVar reaching a camera kernel, which is what
    /// this struct is for (2163).
    pub(super) nearclip: f32,
    /// **The liquid surface over the driven body's feet** (Bevy-Y), as the last movement tick
    /// cached it — [`crate::player::state::Player::liquid_surface`]. Feeds
    /// [`super::camera_water::classify`], the half of `cameraWaterCollision` that is not the trace
    /// mask.
    ///
    /// Our own body's, so it is `None`-equivalent for a far-sight subject: `0x511ad0` reads the
    /// camera TARGET's liquid object, and `view_subject` carries no movement state for a watched
    /// unit — the same gap the swim framing preset has, recorded rather than papered over. A
    /// totem watched across a lake gets the dry-land corridor.
    pub(super) surface_y: Option<f32>,
}

/// What the four mechanisms' gates need to know about the **followed unit** — the conjuncts that
/// are facts about the body rather than about the camera or a CVar. The movement word is the one
/// the last update left, which is what the reference's own input handler reads (`0x50fee0`'s sole
/// caller `0x514446` precedes the mover lookup).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SubjectState {
    /// This frame's `MOVEMENTFLAGS`, verbatim — the three mechanisms that read it read *different*
    /// masks of the same word, so it is carried whole rather than pre-reduced to booleans.
    pub(super) move_flags: u32,
    /// `UNIT_FIELD_FLAGS & 0x100000`, the taxi-flight bit — the terrain-tilt classifier's
    /// highest-priority state (and head bob's ninth conjunct, when that lands).
    pub(super) taxi: bool,
    /// The camera's `Track` bit (`[cam+0x90] & 0x100`) — externally-driven movement.
    pub(super) track: bool,
    /// The camera's `Fear` bit (`[cam+0x90] & 0x1000`) — external control.
    pub(super) fear: bool,
    /// The subject's facing, radians in benilla's yaw convention — the direction terrain tilt
    /// probes along, and the frame head bob's lateral sway is written in (`[obj+0xc98]`).
    pub(super) facing: f32,
    /// `UNIT_FIELD_MOUNTDISPLAYID > 0` — head bob's eighth conjunct.
    pub(super) mounted: bool,
    /// The mover's current speed in yd/s — `GetCurrentSpeed 0x7c4c90`, which benilla already has
    /// as [`crate::net::current_speed`]. Head bob's rate factor is `clamp(speed / 7.2, [0.5,1.5])`.
    pub(super) speed: f32,
    /// The camera's input-command word ([`super::camera::follow_cmd`]) — head bob's ARM is an edge
    /// on this and nothing else (`0x511170`'s two callers).
    pub(super) command: u32,
    /// Is the spyglass scope up? `[cam+0x90] & 0x8`, the aura-76 (`SPELL_AURA_FAR_SIGHT`) FOV lock
    /// — head bob's fourth conjunct.
    pub(super) scoped: bool,
}

impl SubjectState {
    /// `MOVEMENTFLAGS & 0xf` — MOVE (`0x1|0x2`) plus STRAFE (`0x4|0x8`). **TURN (`0x10|0x20`) is
    /// deliberately not in the mask** (`0x5106b7`'s byte-lane `test byte ptr [eax+0x40],0xf`), so
    /// turning in place still passes `cameraPivot`'s fourth conjunct.
    pub(super) fn translating(&self) -> bool {
        self.move_flags & (mf::FORWARD | mf::BACKWARD | mf::STRAFE_LEFT | mf::STRAFE_RIGHT) != 0
    }
}

/// The movement-flag bits these gates read, by the names the wire uses
/// ([`crate::creature_anim::move_flags`] is the one definition; re-exported here so the predicates
/// below read as the reference's masks do).
use crate::creature_anim::move_flags as mf;

/// **`cameraPivot` — "smart pivot"**: the camera the collision solver has pinned against geometry
/// tilts its *view* instead of swinging its *arm*.
///
/// The whole feature is one extra pitch channel, `[cam+0x104]`, and where it is applied:
///
/// - **The gate `0x510690`** is `obj != 0 ∧ typemask bit 0x8 ∧ cameraPivot ∧ ¬translating ∧
///   [cam+0xf4] ≤ 0`, returning the solver's own clip flags `[cam+0x90] & 0x30000` — which nothing
///   but `0x50e570` writes (it builds them from its two sweep hits and the driver ORs the return
///   in at `0x50ed6a`). So **the verdict is nonzero only in a frame the camera was actually
///   clipped**, which is what makes this "smart": an unobstructed camera never pivots.
///   `[cam+0xf4] ≤ 0` is the eye at or below the target looking level-or-**up**.
/// - **The routing `0x50fee0`.** A true verdict *plus* a mostly-vertical drag
///   (`|dPitch| > cameraPivotDYMin ∧ |dYaw| < cameraPivotDXMax`) — or a bias already displaced
///   past `0.001` with the ease not armed — accumulates the pitch delta into `+0x104` instead of
///   calling the ordinary pitch integrator `0x510120`. The clamp on that accumulate is a
///   **one-sided floor** at `−89° − [cam+0xf4]`, applied only while `[cam+0xf4] < 0`; the
///   symmetric ±89° pair belongs to the integrator, not here.
/// - **The sink.** The driver computes the eye from the *unbiased* pitch
///   (`0x50edcc → 0x50de00`, stored to `[cam+0x08..0x10]`) and only **then** rotates the camera
///   basis at `[cam+0x14]` by the bias (`0x50ee32`–`0x50ee58`, `0x7be730` about `[0xbe0f20]` =
///   `(0,1,0)`, skipped entirely while `|bias| < 0.001`). That ordering *is* the feature: the seat
///   does not move and the look direction does. Corroborated by `0x5103e0`, the camera→body
///   hand-off, which clamps **`[cam+0x104] + [cam+0xf4]`** to ±89° before passing the pitch on —
///   i.e. the player's aim is the composite.
/// - **The release `0x5107f0`**, run per frame from the driver at `0x50ed77`:
///   `|bias| ≥ 0.001 ∧ 0x511010(cam) == 0 ∧ ¬gate` (the last negated) arms `+0x104` back to zero at
///   `cameraTargetSmoothSpeed`; a false verdict snapshots and disarms instead.
///
/// **The third release conjunct, `0x511010`**, decoded: `[cam+0x90] & 0x100` — the camera's TRACK
/// latch, the same bit that picks the tilt matrix's `Track` row, fed from `[inputState+4] &
/// 0xf00000` (externally-driven movement, which is benilla's `server_riding`) — and then, only if
/// that is set, `cameraSmoothTrackingStyle == Never` OR a move-type-**0** order in flight. A true
/// answer BLOCKS the ease home, holding the bias while a tracking swing runs.
///
/// benilla builds the first disjunct and cannot build the second: `[0xc4d888]` is the
/// click-to-move / auto-action move-type global, and this client has no click-to-move to put an
/// order in flight. That half is inert at every state benilla can reach, so it is named here
/// rather than stubbed — the day click-to-move exists, this is the line that needs the arm.
pub(super) struct SmartPivot {
    /// The pitch-bias channel `+0x104` — angular, so the armer's `2π` rewrap applies. Its armed
    /// bit is the reference's `[cam+0x90] & 0x8000000`, which is read as a routing conjunct.
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
    /// The bias the view is carrying this frame, radians, **benilla's sign** (positive = the view
    /// tilted further up). Added to the pitch for the camera's *rotation* and for the body pitch
    /// hand-off — never for the seat.
    pub(super) fn bias(&self) -> f32 {
        self.bias.live()
    }

    /// `0x50fee0`'s pitch routing, for one motion event.
    ///
    /// Takes this event's pitch and yaw deltas (radians, benilla sign) and the camera's live pitch,
    /// and answers **the pitch delta the ordinary integrator should apply** — `None` on the
    /// reference's *pure-pivot frame*, where neither the ease nor the integrator runs and the drag
    /// lives entirely in the bias.
    ///
    /// The three legs are the reference's, transcribed with every comparison flipped for benilla's
    /// upward-positive pitch (see the module doc):
    ///
    /// | reference | here |
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
        // `0x510690`, conjuncts 3-6. (1 and 2 — a non-null UNIT-or-PLAYER subject — are structural
        // here: the rig only ever follows one.)
        let verdict = cfg.pivot && !subject.translating() && pitch >= 0.0 && clipped;
        // The selector `ecx`: an already-displaced bias with the ease NOT armed keeps routing, or
        // a fresh verdict with a mostly-vertical drag starts it.
        let displaced = !self.bias.in_flight() && self.bias().abs() >= CHANNEL_EPS;
        let vertical_drag =
            verdict && d_pitch.abs() > cfg.pivot_dy_min && d_yaw.abs() < cfg.pivot_dx_max;
        if displaced || vertical_drag {
            let mut bias = self.bias() + d_pitch;
            // The one-sided clamp: only while the camera is looking up, and only on the side that
            // would take the composite past the pitch limit.
            if pitch > 0.0 {
                bias = bias.min(super::camera::CAM_PITCH_LIMIT - pitch);
            }
            self.bias.snap(bias);
            if bias >= 0.0 && pitch > 0.0 {
                // The pure-pivot frame (`0x5100c3`): the bias stands, the ease is disarmed, and
                // the ordinary pitch integrator does not run at all.
                return None;
            }
        }
        // The ordinary path (`0x51009a`): start the bias easing back to zero, then integrate the
        // pitch as usual. Reached with the increment above already standing when the selector
        // fired — the reference applies the delta to BOTH on this leg, and the ease then walks the
        // bias off while the pitch keeps it.
        self.release(cfg);
        Some(d_pitch)
    }

    /// The per-frame half (`0x50ed77` → `0x5107f0`): with the gate false and the bias displaced,
    /// ease it back to zero; otherwise hold it where it is. Then step whatever is in flight.
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
        // `0x511010`'s first disjunct — the TRACK latch up with tracking set to `Never`. A true
        // answer blocks the ease home, so it joins the gate on the holding side rather than
        // forming a third arm.
        let tracking_hold = subject.track && tracking_style == super::camera::FollowStyle::Never;
        let holding = verdict || tracking_hold;
        if !holding && self.bias().abs() >= CHANNEL_EPS {
            self.release(cfg);
        } else if holding {
            // The FALSE leg of `0x5107f0`'s caller (`0x50ed93`): snapshot and clear the armed bit,
            // i.e. a re-engaged gate cancels an ease in flight rather than fighting it.
            let live = self.bias();
            self.bias.snap(live);
        }
        self.bias.advance(dt);
    }

    /// `0x512a50(cam, target = 0, 0, 1.0f, now)` — arm the bias back to zero at
    /// `cameraTargetSmoothSpeed`. A no-op when it is already there (the armer's own epsilon).
    fn release(&mut self, cfg: &CameraOptions) {
        self.bias
            .arm(&Arm::at(0.0, cfg.target_smooth_speed.to_radians()));
    }
}

/// **`cameraTerrainTilt` — "Follow Terrain"**: the camera pitches with the ground the character is
/// walking onto, so cresting a hill does not put the view into the dirt.
///
/// Registered `"0"`, so this one is OFF out of the box — building it changes nothing until a player
/// ticks the box, which is exactly why it could sit unbuilt with a full byte-level spec.
///
/// Three pieces:
///
/// **The probe (`0x50d900`), which does not look under the character at all.** It samples the
/// ground **ahead**: a horizontal ray `10/3` yd along the unit's facing from `feet + 5/3` up, its
/// hit pulled back `5/18`; then a `64/9` drop straight down from wherever that landed. The `5/3`
/// is a probe LIFT, added going in and cancelled coming out, so the whole thing reduces to
/// `slope = (groundY_ahead − feet.y) / horizontal_run` — rise over run, ahead of you. A missed
/// horizontal ray means the full reach; a missed drop means `−64/9`, a slope of about `−1.78`,
/// which the staircase and the clamp turn into the full downward tilt: **walking off a cliff edge
/// pitches the camera all the way down**, which is the behaviour and not an accident. Throttled to
/// 100 ms, and on a throttled frame the previous value STANDS (`0x50d956 js` leaves `[cam+0xa0]`
/// alone) — only a failed gate zeroes it.
///
/// **The staircase (`0x808a40`), which is not a curve.** Ten `{key, angle}` records walked
/// *downward* from index 9, taking the first `|slope| >= key` — a nearest-below step with no
/// interpolation. The keys are `round(tan(5k°), 2)` and the angles `5k°`; but the ±20° clamp that
/// follows equals record 4 exactly, and the table has one consumer image-wide, so **records 5–9
/// are unreachable** and the shipped map is five steps: `0 / 5 / 10 / 15 / 20°` at
/// `|slope| >= 0 / 0.09 / 0.18 / 0.27 / 0.36`. Uphill is a NEGATIVE angle in the reference, i.e.
/// the camera looks up — which is `+` here (module doc).
///
/// **The arm (`0x50dbc0`)**, which is where `cameraSmoothStyle` gets a second job: a 10-state
/// movement classifier indexes a `style × state` matrix of `{Absorb, Delay, Factor}`, the probe's
/// angle is scaled by `Absorb`, and `Factor` decides what happens — negative is an explicit
/// **disable sentinel** (the channel holds where it is), otherwise the ground channel `+0x108`
/// arms toward `Absorb × angle`. The duration is clamped to `[3·Factor, 10·Factor]` seconds by
/// `cameraTerrainTiltTimeMin`/`Max`, and since `20° / 7.5°/s` is only 2.67 s **that floor always
/// binds**: every tilt takes at least three seconds. It is a lazy, deliberate lean, not a
/// suspension.
///
/// The sink is the third additive pitch: `0x50f810` folds `+0x108` into the view pitch, and unlike
/// the pivot bias it sits INSIDE the ±89° clamp.
pub(super) struct TerrainTilt {
    /// The ground channel `+0x108` — angular, rate `cameraGroundSmoothSpeed`.
    ground: SmoothChannel,
    /// The probe's own output `[cam+0xa0]`, held between throttle ticks.
    slope_pitch: f32,
    /// Seconds since the probe last ran — the 100 ms throttle (`0x50d94f`).
    since_probe: f32,
    /// **The mouse-look hand-off, `[cam+0xa8]`** — is the lean currently living inside the pitch
    /// rather than being composed onto it? The reference keeps a signed REFCOUNT here and gates
    /// the compose on `> 0` (`0x50f809 jg`); benilla has exactly one look session, so the count
    /// can only ever be 0 or 1 and a bool carries it without pretending otherwise.
    handed_off: bool,
}

impl Default for TerrainTilt {
    fn default() -> Self {
        Self {
            ground: SmoothChannel::angular(),
            slope_pitch: 0.0,
            // The probe runs on the very first frame rather than 100 ms into the session.
            since_probe: PROBE_THROTTLE,
            handed_off: false,
        }
    }
}

/// One row of the `cameraTerrainTilt<Style><State>` matrix — `{Absorb, Delay, Factor}`.
///
/// **A code table, not ninety CVar rows**, following exactly what 1502 did for its sibling family
/// `cameraSmooth<Style><State>{Delay,Factor}` ([`super::camera::FollowStyle::row`]): the reference
/// registers these through a loop nest that a per-name row here would only obscure, and the matrix
/// is a *law*, not a setting anybody tunes. The values are the full byte-read dump of its
/// default-string table `0x84f620`.
type TiltRow = (f32, f32, f32);

/// The ten movement states `0x50dbc0` classifies into, in the reference's own index order (the
/// `0x84f5e8` name table): `Fall, Fear, Idle, Jump, Move, Strafe, Swim, Taxi, Track, Turn`.
///
/// **`Jump` (3) is unreachable in the reference** and is unreachable here: `0x50dc28` and
/// `0x50dc35` test the same bit of the same reloaded dword, so the second `je` is unconditional and
/// the block that would emit state 3 is dead. Its row
/// carries the disable sentinel at every style anyway, so nothing observable rides on it — it is
/// kept in the enum because the *matrix* keeps it, and dropping it would silently re-index the
/// other nine.
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
    /// `0x50dbc0`'s classifier, as the priority ladder its branch order makes it.
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

    /// This state's `{Absorb, Delay, Factor}` at `style`.
    ///
    /// `Never` is every state disabled. `Always` differs from `Smart` in exactly one row — `Idle`,
    /// which `Smart` disables (so a standing camera holds its tilt rather than levelling) and
    /// `Always` drives. `Swim` and `Taxi` absorb **0.0** at both live styles, i.e. they aim the
    /// channel at LEVEL rather than at the ground.
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
    /// The signed pitch the ground is contributing this frame, radians, benilla's sign — added to
    /// the view pitch inside the ±89° clamp.
    pub(super) fn pitch(&self) -> f32 {
        // `0x50f809 test eax,eax / jg` — with the hand-off refcount up, the compose is SKIPPED,
        // because the lean is already inside the pitch that `0x50d500` pushed it into.
        if self.handed_off {
            0.0
        } else {
            self.ground.live()
        }
    }

    /// **The mouse-look hand-off** (`0x50d500` push / `0x50d520` pop) — returns the delta the
    /// pitch owes, which is `0x510120`'s argument and nothing else.
    ///
    /// It is a hand-off, not a suppression. On the freelook 0→1 edge the lean's *current* value is
    /// added to the pitch and the compose stops, so the view does not move: the lean simply becomes
    /// part of the angle the player is now dragging. The channel keeps tracking the terrain
    /// underneath throughout — `0x50d900`/`0x50dbc0` are not gated by `+0xa8` — and on the →0 edge
    /// the *then-current* value is subtracted back out and the compose resumes.
    ///
    /// So crossing a slope change mid-drag leaves a step of exactly the difference between the
    /// lean at press and at release. That is the reference's behaviour, not a defect of this
    /// transcription: the reference eases that step through the pitch channel at
    /// `cameraPitchSmoothSpeed`, and benilla's pitch is an immediate write (`0x510120`), so here
    /// it lands in one frame. Named, not smoothed over.
    pub(super) fn hand_off(&mut self, freelook: bool) -> f32 {
        if freelook == self.handed_off {
            return 0.0;
        }
        self.handed_off = freelook;
        // Always the RAW channel value: `pitch()` answers zero once the flag is up, and the pop
        // has to give back what the push took plus whatever the terrain moved in between.
        let live = self.ground.live();
        if freelook {
            live
        } else {
            -live
        }
    }

    /// The probe's staircase (`0x50db37`'s downward walk over `0x808a40`, then the ±20° clamp),
    /// given the ground's rise-over-run ahead of the subject. Uphill (`slope >= 0`) tilts the view
    /// UP here, which is the reference's negative.
    pub(super) fn slope_to_pitch(slope: f32) -> f32 {
        // The five reachable steps, biggest first — the walk takes the first key the slope clears.
        // The keys are `round(tan(5k°), 2)`, and each of these literals is bit-identical to the
        // f32 the table holds (`0.36` IS `0.36000001430511475`), so writing them short costs no
        // fidelity and keeps the derivation legible.
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

    /// Run the probe if the throttle allows, then arm and step the ground channel.
    ///
    /// `probe` answers the *rise over run* of the ground ahead — `None` when the CVar's gate is
    /// false, which is the one case that ZEROES the held value (`0x50d922`) rather than holding it.
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
            // `0x5105a0` false: the kernel's second early exit writes an integer zero into
            // `[cam+0xa0]` and returns, so the held value does NOT survive a switched-off CVar.
            self.slope_pitch = 0.0;
        } else if self.since_probe >= PROBE_THROTTLE {
            self.since_probe = 0.0;
            self.slope_pitch = Self::slope_to_pitch(probe());
        }
        let (absorb, delay, factor) = TiltState::of(subject).row(style);
        if factor < 0.0 {
            // The disable sentinel (`0x50dcd1`): the target becomes the live value and the channel
            // disarms — the camera keeps the lean it has. This is all of `Never`, and it is `Smart`
            // standing still.
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

/// The probe's re-run interval — `0x50d953 sub ecx,0x64` / `js`, i.e. 100 ms.
const PROBE_THROTTLE: f32 = 0.1;
/// The horizontal probe's reach, yd — `[0x808a34] = 10/3`.
pub(super) const PROBE_REACH: f32 = 10.0 / 3.0;
/// How far the horizontal hit is pulled back before the drop, yd — `[0x808ab8] = 5/18`.
pub(super) const PROBE_BACKOFF: f32 = 5.0 / 18.0;
/// The probe LIFT, yd — `[0x808a38] = 5/3`. Added to the feet going in and cancelled coming out,
/// so it never reaches the slope; it exists to start the horizontal ray above the ground.
pub(super) const PROBE_LIFT: f32 = 5.0 / 3.0;
/// The vertical drop's reach, yd — `[0x808ab4] = 64/9`.
pub(super) const PROBE_DROP: f32 = 64.0 / 9.0;

/// **`cameraBobbing` — head bob**: the eye's small figure-of-eight while you walk, in first person
/// only.
///
/// Registered `"0"`, so this ships OFF like Follow Terrain.
///
/// **The gate `0x5105e0` has ten conjuncts, and the first one is the surprise.** `[cam+0xec] <= 1/6`
/// is a compare on the **zoom distance**, inclusive — so head bob is *first person only*.
/// The rest: a non-null UNIT-or-PLAYER subject, not the
/// aura-76 FOV lock, `cameraBobbing`, not SWIMMING, not FALLING, `MOUNTDISPLAYID <= 0`, not on a
/// taxi — and, as its tenth, the **return mask itself**: `0x510675 and eax,0x200`, so the whole
/// predicate is false unless a bobbing *session* is armed. (`0x51063d`–`0x510653` is dead code — a
/// second `test ch,0x20` on a bit the previous `jne` already proved clear. Not transcribed.)
///
/// **What arms a session is the movement-COMMAND word, not the gate.** `0x511170(cam, enable)` is
/// an edge setter called from the input word's AddFlags/RemoveFlags (`0x5149d5`/`0x514cc1`), which
/// recompute from the NEW word:
///
/// ```text
/// armed = (f & (Fwd|Back|StrafeL|StrafeR|AutoRun|Track)) != 0
///      || (RightMouse && (LeftMouse || TurnL|TurnR))
/// ```
///
/// — the very word benilla already builds for the auto-follow ([`super::camera::follow_cmd`]). The
/// arm stamps the epoch, so **the phase restarts at every session**; a redundant re-arm writes
/// nothing. And the latch is **not** gated by `cameraBobbing`: a stock client runs it on every
/// start and stop and renders nothing, which is why turning the CVar on mid-run starts the bob at
/// the phase the session has already reached rather than from zero.
///
/// **The kernel `0x511920`.** `A = cameraBobbingFrequency × clamp(speed / 7.2, [0.5, 1.5])`,
/// `t` = seconds since the epoch; horizontal `a = ampH · sin(2πAt)` laid along the subject's
/// **left** axis (`φ + π/2`), vertical `ampV · sin(2π · 2A · t)` straight up — the horizontal
/// traces a line and the vertical runs at double rate, which is the figure of eight. Both
/// amplitudes are their CVar × 1/36. The rate factor's floor is `0.5`, so a session armed while
/// standing still still bobs, at half rate.
///
/// **It is a pure TRANSLATION of the eye** and it lands in the same accumulator the camera shake
/// uses, reaching the world position once; the look-at target is `eye + forward`, so it rides
/// along and the view never rotates.
///
/// **The decay `0x5106f0` → `0x50f160`.** `cameraBobbingSmoothSpeed` is *not* in the kernel at all:
/// its one image-wide read is in the disarm, where `|largest component| / speed` becomes the ramp
/// DURATION. A residual offset with the gate false eases to zero over that, cosine, terminating on
/// an exact zero — about 0.069 s at the shipped defaults.
///
/// **The named divergence:** the reference solves the camera collision against the *bobbed* trial
/// eye (`0x50ed47`), so the camera pushes off geometry it bobs into. benilla adds the offset after
/// the sweep, which is the same thing [`crate::camera_shake`] already does with the very same
/// accumulator — one divergence, in one place, rather than a new one here.
pub(super) struct HeadBob {
    /// Is a session armed? `[cam+0x90] & 0x200`, latched on the command word's edges.
    armed: bool,
    /// The command word the last edge was computed from.
    last_command: Option<u32>,
    /// Seconds since the last transition — `[cam+0x238]`, which serves the bob phase while armed
    /// and the decay's clock while not, the two never overlapping.
    since_transition: f32,
    /// The offset this frame, world axes.
    offset: Vec3,
    /// The disarm snapshot and the ramp's duration — `[cam+0x240..0x248]` and `[cam+0x23c]`.
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
            // The ctor's zero: a decay with nothing snapshotted lands on `s >= 1` immediately and
            // writes the exact zero, which is the reference's behaviour and not a division to
            // guard against.
            ramp_duration: 0.0,
        }
    }
}

impl HeadBob {
    /// The eye offset this frame, world axes — [`Vec3::ZERO`] whenever nothing is bobbing.
    pub(super) fn offset(&self) -> Vec3 {
        self.offset
    }

    /// The arm predicate, on the camera's own input-command word.
    fn arms(command: u32) -> bool {
        use super::camera::follow_cmd as c;
        const TRANSLATING: u32 =
            c::FORWARD | c::BACKWARD | c::STRAFE_LEFT | c::STRAFE_RIGHT | c::AUTORUN | c::TRACK;
        command & TRANSLATING != 0
            || (command & c::RIGHT_MOUSE != 0
                && command & (c::LEFT_MOUSE | c::TURN_LEFT | c::TURN_RIGHT) != 0)
    }

    /// One frame: run the latch off the command word, then either the kernel or the decay.
    ///
    /// `zoom` is the wheel's distance (`[cam+0xec]`), NOT the collision-pulled arm — a camera
    /// squeezed against a wall is not in first person.
    pub(super) fn advance(
        &mut self,
        zoom: f32,
        subject: &SubjectState,
        cfg: &CameraOptions,
        dt: f32,
    ) {
        self.since_transition += dt;
        // The latch (`0x511170`), edge-triggered on the command word and NOT on `cameraBobbing`.
        let want = Self::arms(subject.command);
        if self.last_command.replace(subject.command) != Some(subject.command) && want != self.armed
        {
            if want {
                self.armed = true;
            } else {
                // The disarm (`0x511110`) snapshots the offset and turns its largest component
                // into the ramp's duration.
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
            // `0x5106f0`'s displacement test is on the SIGNED SUM's magnitude, not on the vector's
            // length — a detail worth keeping, because the two disagree whenever the components
            // cancel.
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

    /// `0x5105e0`'s ten conjuncts, in its own order.
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

    /// `0x511920`, once the gate has passed.
    fn kernel(&self, subject: &SubjectState, cfg: &CameraOptions) -> Vec3 {
        let rate = cfg.bob_frequency
            * (subject.speed / BOB_SPEED_DIVISOR).clamp(BOB_SPEED_CLAMP.0, BOB_SPEED_CLAMP.1);
        let phase = std::f32::consts::TAU * rate * self.since_transition;
        let lateral = cfg.bob_lr_amplitude * BOB_AMPLITUDE_SCALE * phase.sin();
        // `φ + π/2` in the reference's Z-up frame is the subject's LEFT — here, the same rotation
        // about Bevy's +Y. The sway is a sine, so left and right differ only by half a period;
        // the axis is what matters, and it is the body's, written out in world axes.
        let left = Quat::from_rotation_y(subject.facing) * Vec3::NEG_X;
        left * lateral
            + Vec3::Y * (cfg.bob_ud_amplitude * BOB_AMPLITUDE_SCALE * (2.0 * phase).sin())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;
    /// A drag that is unambiguously "mostly vertical": well over `cameraPivotDYMin` on pitch and
    /// well under `cameraPivotDXMax` on yaw.
    const UP: f32 = 0.01;

    fn moving(translating: bool) -> SubjectState {
        SubjectState {
            move_flags: if translating { mf::FORWARD } else { 0 },
            ..SubjectState::default()
        }
    }

    /// **The gate's whole point** (`0x510690`, conjunct 6): the verdict is the collision solver's
    /// own clip flags, so a camera with nothing behind it never pivots however you drag it. Every
    /// other conjunct is checked in the same shape — one at a time, against a case that would
    /// otherwise pivot.
    #[test]
    fn only_a_clipped_camera_looking_level_or_up_and_not_translating_pivots() {
        let cfg = CameraOptions::default();
        // The reference case: clipped, pitched up, standing still, CVar on -> the bias takes it.
        let mut p = SmartPivot::default();
        assert_eq!(
            p.route_pitch(UP, 0.0, 0.2, &moving(false), true, &cfg),
            None
        );
        assert!(p.bias() > 0.0, "the drag went into the bias");

        // …and each single conjunct removed hands the delta back to the ordinary integrator.
        for (name, opts, pitch, subject, clipped) in [
            ("unclipped", cfg, 0.2, moving(false), false),
            ("translating", cfg, 0.2, moving(true), true),
            // Looking DOWN — the reference's `[cam+0xf4] <= 0`, mirrored (module doc).
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

    /// The drag-shape test (`0x50fff5`/`0x510004`): a mostly-HORIZONTAL drag is an ordinary orbit
    /// even with every gate conjunct true — smart pivot is for looking up, not for turning.
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
        // Exactly at the threshold is still refused — the compare is `|dYaw| < DXMax`.
        assert_eq!(
            p.route_pitch(UP, cfg.pivot_dx_max, 0.2, &moving(false), true, &cfg),
            Some(UP)
        );
        assert_eq!(p.bias(), 0.0);
        // And a hair under it pivots.
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

    /// The one-sided clamp (`0x510065`–`0x510079`): the pivot may not take the composite aim past
    /// the ±89° pitch limit, and the bound is on **that sum**, not on the bias alone.
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

    /// Dragging back DOWN unwinds the bias and, the moment it would carry the view below the
    /// camera's own pitch, hands the axis back to the ordinary integrator — the reference's
    /// `bias > 0 || f4 >= 0` fork at `0x510094`/`0x510045`, mirrored.
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
        // Down again, five times: the bias walks back to (about) zero and the axis is handed over.
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

    /// The release (`0x50ed77` → `0x5107f0`): once any gate conjunct drops, the bias eases back to
    /// zero at `cameraTargetSmoothSpeed` — not instantly, and not never.
    #[test]
    fn losing_the_gate_eases_the_bias_home_at_the_target_smooth_speed() {
        let cfg = CameraOptions::default();
        let mut p = SmartPivot::default();
        p.route_pitch(0.3, 0.0, 0.5, &moving(false), true, &cfg);
        let held = p.bias();
        assert!(held > 0.0);
        // The gate still true: the bias sits exactly where it is, indefinitely.
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
        // Step out of the wall: it eases home over |bias| / 90°/s.
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

    /// The staircase, not a curve (`0x808a40` walked downward with no interpolation) — and the
    /// **five** reachable steps, because the ±20° clamp equals record 4 and the table has one
    /// consumer image-wide, so records 5–9 can never show through.
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
            // Records 5-9 would read 25/30/35/40/45° — the clamp makes them all 20°.
            (0.47, 20.0),
            (100.0, 20.0),
        ] {
            assert!(
                (deg(slope) - want).abs() < 1e-4,
                "slope {slope} -> {} °, wanted {want}",
                deg(slope)
            );
            // Symmetric in magnitude, opposite in sign: downhill tilts the view down.
            assert!(
                (deg(-slope) + want).abs() < 1e-4,
                "slope -{slope} -> {} °, wanted -{want}",
                deg(-slope)
            );
        }
        // The sign convention, stated as an assertion rather than a comment: UPHILL looks UP,
        // which is positive here and negative in the reference.
        assert!(TerrainTilt::slope_to_pitch(0.5) > 0.0);
    }

    /// The `Factor < 0` disable sentinel holds the channel where it is — which at the shipped
    /// `Smart` style is what a standing camera does, and what all of `Never` does. Every other
    /// state drives, and `Swim`/`Taxi` drive it to LEVEL rather than to the ground.
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

    /// The classifier is a priority ladder, and the order is the reference's branch order — a taxi
    /// outranks swimming, swimming outranks falling, and turning in place is its own state rather
    /// than Idle.
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
        // …and the ladder's order where several are set at once.
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

    /// The whole probe→channel round trip: the throttle holds the probe at 100 ms, the gate's
    /// false leg ZEROES the held value (it does not hold it), and the duration floor binds so that
    /// even the full 20° takes its three seconds.
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
        // The probe runs on frame one, then not again for 100 ms.
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

        // The duration floor: 20° at 7.5 °/s is 2.67 s, and `cameraTerrainTiltTimeMin` is 3.
        let mut t = TerrainTilt::default();
        let target = TerrainTilt::slope_to_pitch(0.5);
        let mut took = None;
        for frame in 0..600 {
            let p = t.advance(|| 0.5, true, &moving, FollowStyle::Smart, &cfg, 1.0 / 60.0);
            // Exact equality, not an epsilon: the cosine's tail is flat, so `|p − target| < 0.001`
            // lands a fifth of a second early and would hide a wrong duration. The step writes the
            // target bit-exactly on the frame `s >= 1`, and that frame is the measurement.
            if took.is_none() && p == target {
                took = Some(frame as f32 / 60.0);
            }
        }
        let took = took.expect("the lean arrives");
        assert!(
            (took - cfg.tilt_time_min).abs() < 0.05,
            "the 3 s floor should bind, took {took:.2}s"
        );
        // And the floor is doing the work: |Δ| / rate alone would be 2.67 s.
        assert!(target.abs() / cfg.ground_smooth_speed.to_radians() < cfg.tilt_time_min);

        // The gate's false leg zeroes the held probe value rather than holding it (`0x50d922`).
        let mut zeroed = 0.0_f32;
        for _ in 0..600 {
            zeroed = t.advance(|| 0.5, false, &moving, FollowStyle::Smart, &cfg, 1.0 / 60.0);
        }
        assert!(zeroed.abs() < CHANNEL_EPS, "levels off, at {zeroed}");
    }

    /// The arm predicate, which is the reference's own word and not a re-derivation of "moving":
    /// autorun and externally-driven movement arm it with no key held, and right-mouse arms it only
    /// in a chord.
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

    /// The ten conjuncts, one removed at a time from a case that otherwise bobs — including the
    /// first-person one, which is the easiest to get wrong (it is the ZOOM distance, inclusive at
    /// 1/6).
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
            // A first frame to take the arming edge, then a quarter period to leave zero.
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

    /// The kernel's shape: horizontal along the body's own left axis at `A`, vertical at `2A`, both
    /// amplitudes scaled by 1/36 — and the rate factor's floor, which is why a session armed at a
    /// standstill still bobs at half rate rather than stopping.
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
            // 7.2 yd/s is exactly the divisor, so the rate factor is 1.0 and A = the frequency.
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
        // Five periods, so the counts are large enough that where the window happens to end
        // cannot move the ratio the assertion is about.
        for _ in 0..(5.0 * period / dt) as usize {
            b.advance(0.0, &subject, &cfg, dt);
            let o = b.offset();
            // Facing −Z, so the body's left is world −X and the sway has no Z at all.
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
        // **The vertical runs at double the horizontal** — the figure of eight. Counted over five
        // periods and compared with one crossing of slack, because where the window happens to end
        // can leave the faster axis one crossing short of the exact 2:1 and that is a property of
        // the ruler, not of the wave.
        assert!(
            (up_crossings - 2 * lat_crossings).abs() <= 1 && lat_crossings >= 9,
            "vertical {up_crossings} crossings against lateral {lat_crossings}"
        );

        // The rate floor: standing still (speed 0) still bobs, at half the unit rate.
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

    /// The decay: a session that ends eases the residual to zero over
    /// `|largest component| / cameraBobbingSmoothSpeed` and terminates on an EXACT zero.
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
        // A quarter period in, so the offset is near its peak when the key comes up.
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

    /// A gate that comes back while the ease is in flight **cancels** it where it stands
    /// (`0x50ed93`) rather than fighting it — so stepping back against the wall mid-return leaves
    /// the view tilted, it does not snap.
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

    /// **The sweep decision 2165 says every regime switch owes** — here across the staircase's own
    /// four keys, in both signs.
    ///
    /// Two claims, and the pair is the point. The staircase itself steps by exactly 5° and never
    /// more: that jump is the mechanism, and this is where its size is written down. What reaches
    /// the *camera* steps by a fortieth of that, because the staircase arms a channel instead of
    /// being rendered — which is precisely what the water corridor did not do.
    #[test]
    fn the_staircases_five_degree_jumps_reach_the_camera_as_a_smooth_lean() {
        use super::super::camera::FollowStyle;
        use super::super::camera_channel::assert_bounded_step;

        // The mechanism's own jump, bounded at itself.
        assert_bounded_step(
            (-0.6, 0.6),
            0.0005,
            5.0_f32.to_radians() + 1.0e-6,
            TerrainTilt::slope_to_pitch,
        );

        // And what a player actually sees, walking terrain that sweeps every key in 20 s.
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

    /// The same sweep across head bob's two edges — the arm and the disarm — which are the only
    /// places its offset can move discontinuously.
    ///
    /// The arm is continuous by construction (`since_transition` resets, so the kernel's phase
    /// starts at zero and both components with it); the disarm is the cosine ramp, whose peak rate
    /// is what the bound is set from. A regression that armed at an arbitrary phase would put a
    /// full amplitude into one frame and land here.
    #[test]
    fn arming_and_disarming_the_bob_never_steps_the_eye_by_a_visible_amount() {
        use super::super::camera::follow_cmd as c;
        use super::super::camera_channel::assert_bounded_step;

        let cfg = CameraOptions {
            bobbing: true,
            ..CameraOptions::default()
        };
        // A full amplitude in one frame would be this; the two bounds are fractions of it.
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
        // Arming, and three seconds of bobbing.
        assert_bounded_step((0.0, 3.9), DT, amplitude * 0.25, &mut step);
        // The disarm's own ramp, bounded at what `cameraBobbingSmoothSpeed` actually buys.
        assert_bounded_step((3.9, 6.0), DT, amplitude * 0.5, &mut step);
    }

    /// **The mouse-look hand-off** (`0x50d500` push / `0x50d520` pop).
    ///
    /// The lean moves *into* the pitch for the duration of a drag and comes back out on release,
    /// so the view does not move at either edge — and a slope change crossed mid-drag leaves
    /// behind exactly the difference between the lean at press and the lean at release, which is
    /// the reference's behaviour and the reason this is a hand-off rather than a suppression.
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

        // What the camera composes, across the press.
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
        // Idempotent while held — the reference's refcount does not double-push.
        assert_eq!(tilt.hand_off(true), 0.0);

        // Released over the same terrain: the pitch gives back exactly what it took.
        pitch += tilt.hand_off(false);
        assert!((pitch - 0.2).abs() < 1.0e-6, "the pop returns the push");
        assert!(
            (pitch + tilt.pitch() - composite).abs() < 1.0e-6,
            "and the release must not move the view either"
        );

        // Now cross a slope change mid-drag: press on the hill, flatten, release.
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

    /// **`0x511010`'s first disjunct**: with the TRACK latch up and `cameraSmoothTrackingStyle` at
    /// `Never`, the bias is HELD where it stands instead of easing home — the reference's way of
    /// not fighting a tracking swing in flight. Both of the other two legs release as before.
    #[test]
    fn a_never_tracking_swing_holds_the_bias_the_gate_would_have_released() {
        use super::super::camera::FollowStyle;
        let cfg = CameraOptions::default();
        // Arm a bias through the pure-pivot leg, then drop the gate and run half a second.
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
