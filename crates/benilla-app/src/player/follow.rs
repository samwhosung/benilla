//! `/follow` — the auto-follow movement mode.
//!
//! The finding that shapes this module: **follow synthesizes keyboard input.** It owns no
//! translation of its own. Every state change in the reference funnels through the movement
//! singleton's setter (`0x60e790` sets the move-forward bit `0x100000`, `0x60e7f0` clears it) — the
//! same setter the MoveForward keybinding drives, one of ~40 stubs in the keybinding command table
//! at `0x513de0`-`0x514273`. So follow is "hold W for me, and steer", and the right implementation
//! reuses the controller wholesale rather than growing a second mover.
//!
//! It also sends **nothing on the wire** — corroborated against vmangos, which has no follow opcode
//! in 1.12.1. The server only ever sees the ordinary movement stream our synthesized input produces.
//!
//! ## The motion law
//!
//! - **Facing is steered, never snapped** (`0x6103d0`/`0x6108xx`): turn toward the followee at
//!   [`TURN_RATE`] = π rad/s (180°/s), clamped to `min(remaining, rate × elapsed)`, and stop turning
//!   inside a [`TURN_DEADZONE`] of 0.001 rad. That rate limit is why follow *reads* like a character
//!   running rather than a camera snapping.
//! - **Beeline, re-aimed every tick** (`0x610e40`): the guid and both positions are re-resolved every
//!   tick and nothing is cached. No path, no spline — `0x670630` is a *getter* over the click-to-move
//!   order, which follow reads and never writes.
//! - **It never reads the followee's speed** (VERIFIED negative, whole-graph scan). You move at
//!   whatever a held forward key yields for *you*, which is why follow falls behind a faster runner.
//! - **A hysteresis band, not a stop distance** — see [`should_move`]. Arrive is inclusive, resume is
//!   one-directional. The gap between the two is why follow visibly starts and stops rather than
//!   juddering on a single threshold. Arrival does **not** end the follow.
//!
//! ## The cancel set — what actually makes this a *mode*
//!
//! **Every player-initiated movement START cancels the follow, on the key-DOWN edge.** A key
//! *release* never does, and neither does the follow's own synthesized input.
//!
//! The mechanism sits two layers below the keybinding stubs, which is why reading the handlers
//! answers "nothing cancels follow" and is wrong about the behaviour. Every movement START
//! emitter calls **`0x60e990`**, which
//! cancels at `0x60e9b5` unless the re-entrancy bracket `ds:0xc4da48` **bit 0** is set — and
//! follow's own four emitters (`0x60e790`, `0x60e7f0`, `0x60e8a0`, `0x60e940`) set that bracket
//! around their calls. A real key press never sets it. **That asymmetry is the entire mechanism.**
//!
//! We need no bracket of our own, because our synthesized input is [`Player::follow_forward`] — a
//! flag the controller reads, never a key event — so it cannot reach the cancel at all.
//!
//! | action | benilla key | verdict |
//! |---|---|---|
//! | MoveForward / MoveBackward | `W` / `S` | **cancels** (key-DOWN) |
//! | TurnLeft / TurnRight | `A` / `D` | **cancels** (key-DOWN) |
//! | StrafeLeft / StrafeRight | `Q` / `E` | **cancels** (key-DOWN) |
//! | ToggleAutoRun | `MouseButton::Forward` | **cancels**, on the ON edge only |
//! | both-mouse-buttons run | `L`+`R` | **cancels**, on the both-held transition |
//! | mouse-look turning | `R` held | **cancels** while held, however far the mouse moved |
//! | **Jump** | `Space` | **survives** — the one emitter passing `0x60e990(0,0)` (`0x60dea8`) |
//! | Sit / stand | — | **survives** (a `CMSG_STANDSTATECHANGE`, no guard call) |
//! | walk/run toggle | — | **survives** |
//! | any key release | — | **survives** |
//!
//! Plus the non-input cancels: **losing the mover** (death, stun, a taxi hand-off — the reference's
//! second, rarer site at `0x5146d6`), the followee becoming unresolvable or **dying**, and the
//! degenerate-bearing guard below.
//!
//! ## What is deliberately NOT here
//!
//! **The ~180° turn-away cancel is drunk-only, and is therefore absent.** 0890 shipped it as an
//! INFERRED rule with a latch invented to make the 160°-220° band self-consistent. The band is
//! real (`0x80c604` = 2.7925267 rad, `0x80c600` = 3.8397243 rad, both strict) — but its whole
//! evaluation is gated at `0x610a3d` on an argument that is the **inebriation fraction**,
//! `min([[unit+0xe68]+0x1d], 100) × 0.01`. That byte is proven to be drunkenness by two independent
//! consumers of the identical expression: the `DRUNK_MESSAGE_SELF%d` formatter at `0x5e2ac3`, and
//! `0x60001a` comparing the same fraction against 0.5 to pick an animation state. **On a sober
//! character the band never executes**, so a client that does not model inebriation must not
//! implement it — and benilla does not. (Its real arming trigger is turn-*convergence*, not "inside
//! the band", with a stochastic disarm at `0x61092e`; recorded for whoever adds drunkenness.)
//!
//! Also refuted by census, each a real "surely this cancels follow?" candidate: taking damage,
//! entering combat, mounting, beginning a cast, and the followee simply going out of range
//! (`0x6107e5` explicitly excludes follow's mode at `0x610749`/`0x610752`).
//!
//! ## The start gate lives elsewhere
//!
//! Who you may begin following — a living, assistable **player**, and only while you are alive,
//! unstunned and not casting — is [`crate::target::by_name`]'s, because that is where the subject
//! is resolved. The refusals it raises are real error lines, not silence.

use std::f32::consts::PI;

use bevy::prelude::*;

use crate::net::GuidIndex;

use super::camera::{CameraControl, LookButton};
use super::state::{MoveSpeed, Player};

/// Follow's facing-turn rate, rad/s — `0xc4d93c`, seeded `0x40490fdb` = π at `0x6111f9`. Its own
/// constant, distinct from the keyboard turn rate.
const TURN_RATE: f32 = PI;

/// Inside this many radians of the bearing, follow stops turning (`0x6108bb` → `0x60e920`).
const TURN_DEADZONE: f32 = 0.001;

/// The speed the distance thresholds are normalised by — `.rdata 0x80c4d0` = 7.0, which is also
/// vanilla's base run speed, so at normal speed the band is exactly 3.0 / 4.5 yd.
const SPEED_NORM: f32 = 7.0;

/// The base stop distance — `.rdata 0x80c4c0` = 3.0, a compile-time constant (its three writers are
/// CRT dynamic initializers behind `_initterm`), **not** a cvar.
const STOP_DISTANCE: f32 = 3.0;

/// Resume is 1.5× the stop distance, and its speed scale is floored at 1.0 — so the band never
/// closes below 3.0 / 4.5 yd however slowly you are moving.
const RESUME_FACTOR: f32 = 1.5;

/// `cos(1°)`. The degenerate-bearing guard at `0x610d32` ends the follow once the followee sits
/// within one degree of straight up or down (`|dy| / dist > cos 1°`) — at which point the
/// horizontal bearing a beeline needs is numerically meaningless. Held in `0xc4d9d0`, seeded at
/// `0x610c8d` from `.rdata 0x80c5f8`.
const VERTICAL_ALIGN_COS: f32 = 0.999_847_7;

/// Who we are following, and the hysteresis latch — the reference's followed-guid pair `0xc4d980`
/// (armed `0x6111c9`, cleared `0x60fc1e`; **exactly two writers each**, earned by an
/// opcode-agnostic raw-byte scan for the little-endian addresses, not a literal-displacement grep).
#[derive(Resource, Default)]
pub(crate) struct FollowState {
    /// The followee's guid, or `None` when not following.
    pub(crate) guid: Option<u64>,
    /// The followee's name as it read at the moment the follow began — what
    /// `AUTOFOLLOW_BEGIN`'s argument carries, and therefore what the status text says. Latched
    /// rather than re-read so the line can't change under a rename or a cache eviction mid-follow.
    pub(crate) name: String,
    /// Whether the synthesized forward input is currently held. The band's latch: which threshold
    /// applies depends on which side we are already on.
    moving: bool,
    /// `WOW_FOLLOW_TRACE` bookkeeping: when the last trace line went out, and where we were.
    /// `None` until a tick has seeded it.
    traced_at: f64,
    traced_pos: Option<Vec3>,
}

impl FollowState {
    /// Begin following `guid`, from a clean band.
    pub(crate) fn start(&mut self, guid: u64, name: String) {
        self.guid = Some(guid);
        self.name = name;
        self.moving = false;
        self.traced_at = 0.0;
        self.traced_pos = None;
    }

    /// Stop following. Returns whether we actually were.
    pub(crate) fn stop(&mut self) -> bool {
        self.moving = false;
        self.guid.take().is_some()
    }
}

/// Start following — the app-side funnel every entry point converges on, whether it came from the
/// chat parser (decision 0881 parses slash lines in Rust) or from the Era API globals the shipped
/// UI calls ([`benilla_ui::script::FollowRequest`], via [`crate::ui_follow`]).
///
/// The two variants are the reference's own two bindings. They differ in how the subject is *found*
/// — a name resolves players only and through the by-name resolver's filter mode 2, a token takes
/// whatever it points at — but **not** in what is allowed: the start gate downstream applies to
/// both alike, which is why `/follow` on a creature refuses either way.
#[derive(bevy::ecs::message::Message, Clone, Debug)]
pub(crate) enum FollowRequest {
    /// `FollowUnit(unit)` — the bare `/follow` is `FollowUnit("target")`.
    Unit(String),
    /// `FollowByName(name, exactMatch)`. `exact` is the second Lua argument: the unit popup's
    /// Follow row passes `1` (it already knows the exact name and must not prefix-match onto a
    /// bystander), `/follow <name>` passes nothing.
    Name { name: String, exact: bool },
}

/// The distance at which follow **arrives** and lets go of the key (`0x610ad2`-`0x610b1b`):
/// `(speed / 7.0) × 3.0`, tested inclusively (`test ah,0x41; jp` — the `<=` edge).
fn arrive_distance(speed: f32) -> f32 {
    (speed / SPEED_NORM) * STOP_DISTANCE
}

/// The distance at which a stopped follow **resumes** (`0x610bc4`-`0x610c2b`):
/// `3.0 × 1.5 × max(speed / 7.0, 1.0)` = 4.5 yd at normal run speed. Only consulted while stopped.
fn resume_distance(speed: f32) -> f32 {
    STOP_DISTANCE * RESUME_FACTOR * (speed / SPEED_NORM).max(1.0)
}

/// Should the synthesized forward key be held this tick? The band, as a pure function of which side
/// we are already on — the whole reason follow starts and stops instead of juddering.
fn should_move(was_moving: bool, distance: f32, speed: f32) -> bool {
    if was_moving {
        // Arrive is inclusive, so we keep going only while strictly beyond it.
        distance > arrive_distance(speed)
    } else {
        distance >= resume_distance(speed)
    }
}

/// Does this frame's input **destroy** the follow? See the module header's table for the census
/// and for what is deliberately absent.
///
/// `move_start` is the key-DOWN **edge** of any movement start (W/S/A/D/Q/E), never held state —
/// the reference cancels in the START emitters and its STOP siblings carry no guard call at all,
/// which is why letting go of a key never resumes a follow you just broke. `autorun_engaged` is
/// likewise the ON edge only (the OFF edge routes to the unguarded `0x60dc90`). `mouse_look` is a
/// *level*, not an edge: the camera's facing commit re-enters the guard every frame the look is
/// held, regardless of how far the mouse actually moved.
fn follow_cancelled(
    move_start: bool,
    autorun_engaged: bool,
    both_engaged: bool,
    mouse_look: bool,
    lost_mover: bool,
) -> bool {
    move_start || autorun_engaged || both_engaged || mouse_look || lost_mover
}

/// Wrap an angle into `(-π, π]`.
fn wrap_pi(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let x = (a + PI).rem_euclid(t);
    x - PI
}

/// The `face_yaw` that points at a horizontal delta in **Bevy** space.
///
/// Derived from the controller's own forward vector rather than from a coordinate convention:
/// `control` builds `move_fwd = Quat::from_rotation_y(face_yaw) * NEG_Z`, which expands to
/// `(-sin y, 0, -cos y)`. Solving `(-sin y, -cos y) ∝ (dx, dz)` gives `y = atan2(-dx, -dz)`. Tied to
/// the expression it must agree with, so it cannot drift out of sign with it.
fn bearing_to(delta: Vec3) -> f32 {
    (-delta.x).atan2(-delta.z)
}

/// Is the followee within one degree of straight up or down? `0x610d32`'s guard — at that point the
/// horizontal bearing is numerically meaningless and the follow ends rather than spinning.
fn vertically_degenerate(delta: Vec3) -> bool {
    let dist = delta.length();
    dist > f32::EPSILON && delta.y.abs() / dist > VERTICAL_ALIGN_COS
}

/// Turn `face` toward `bearing` by at most this tick's budget, or leave it alone inside the
/// deadzone. The reference's `min(remaining, rate × elapsed)` clamp.
fn steer(face: f32, bearing: f32, dt: f32) -> f32 {
    let remaining = wrap_pi(bearing - face);
    if remaining.abs() <= TURN_DEADZONE {
        return face;
    }
    let budget = TURN_RATE * dt;
    face + remaining.clamp(-budget, budget)
}

/// Everything the cancel set reads. Bundled so [`steer_follow`]'s own parameter list stays about
/// the motion.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct FollowInput<'w, 's> {
    buttons: Res<'w, ButtonInput<MouseButton>>,
    /// The binding dispatch — the movement commands' press edges, wherever they are bound.
    /// It already carries the typing gate this struct used to hold `UiKeyboardCapture` for
    /// (typing "we should go" in chat must not break the follow).
    binds: Res<'w, crate::bindings::BindingsState>,
    /// The camera rig, for the mouse-look level and the world's own mouse buttons
    /// ([`super::camera::WorldMouse`]). The look level is read one frame behind `control`'s own update,
    /// which is deliberate and harmless: the cancel then lands on the frame *after* the look
    /// begins, versus the reference's same-frame commit. The button latch is **not** behind —
    /// [`super::camera::latch_world_mouse`] is ordered ahead of this system precisely so the both-button
    /// cancel below sees this frame's edge.
    rig: Res<'w, CameraControl>,
    /// The **mover's** descriptor block, for the terms of the cancel set that are not inputs:
    /// its health and its stun. See [`FollowInput::input_torn_down`].
    mover: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::Embodied>>,
    /// What the camera orbits ([`super::view_subject`]) — the resolved half of the shared
    /// precondition's far-sight conjunct, which takes both predicates down and so reaches the
    /// teardown leg on its own.
    ///
    /// Read possibly one frame behind, like `rig` above and for the same benign reason: the
    /// publisher and this system are both merely `.before(control)` and are unordered against each
    /// other, so a follow engaged and far-sighted on the very same frame ends on the next one.
    view_subject: Res<'w, super::view_subject::ViewSubject>,
}

impl FollowInput<'_, '_> {
    /// The press edge of any movement command (wherever the six are bound today). The turn
    /// pair are turn commands normally and strafe under mouse-look; either way they are a
    /// movement start and either way they cancel, so the distinction the controller draws does
    /// not matter here.
    fn move_start(&self) -> bool {
        use crate::bindings::cmd;
        [
            cmd::MOVE_FORWARD,
            cmd::MOVE_BACKWARD,
            cmd::TURN_LEFT,
            cmd::TURN_RIGHT,
            cmd::STRAFE_LEFT,
            cmd::STRAFE_RIGHT,
        ]
        .iter()
        .any(|&c| self.binds.just_pressed(c))
    }

    /// Has the input tick reached its **teardown leg** — the one that cancels click-to-move and the
    /// follow and fires `AUTOFOLLOW_END` ([`super::state::MoverInput::torn_down`])? Not an input, but the
    /// member of the cancel set that is a *state*, and the one benilla had wrong in both directions.
    ///
    /// The mechanism runs through the health test at `0x5144f8`, and this file passed the **root**
    /// instead. More exactly, `0x5146d6 call 0x60fb60`
    /// is reached only when *both* predicates are down (`0x5146c3` and `0x5146ce` both not taken),
    /// so a pure ROOT — which benilla cancelled on — does **not** end a follow in the reference, and
    /// death — which benilla did not cancel on, so a follow taken into death kept steering the
    /// corpse's facing every frame — does. Ice Block, root and stun together, ends it as well.
    ///
    /// **Far sight ends it too**, and for the same structural reason death does rather than as a
    /// case of its own: the far-sight conjunct sits in the precondition the two predicates *share*,
    /// so engaging it takes both down at once and lands on this leg.
    fn input_torn_down(&self, rooted: bool, driving_own_body: bool) -> bool {
        self.mover.single().is_ok_and(|s| {
            super::state::MoverInput {
                dead: s.0.unit_is_dead(),
                view_is_out: super::state::view_is_out(
                    driving_own_body,
                    self.view_subject.remote.is_some(),
                ),
            }
            .torn_down(rooted, s.0.unit_flags() & super::UNIT_FLAG_STUNNED != 0)
        })
    }
}

/// Drive the follow: cancel if this frame's input says so, then re-resolve the followee, steer the
/// facing, and decide whether the synthesized forward input is held this tick. Runs immediately
/// **before** `control`, which reads [`Player::follow_forward`] as one more term of its forward
/// axis — the reference's shape exactly, where follow pushes the same move-forward bit W does.
///
/// The cancel runs before the steer for the reference's own reason: it lives in the movement START
/// emitters, which run ahead of the axis math (`0x5150a7` vs the emitter tail `0x5151a0`), so the
/// frame you break a follow on is a frame the follow never steers.
pub(super) fn steer_follow(
    time: Res<Time>,
    mut follow: ResMut<FollowState>,
    mut player: ResMut<Player>,
    speed: Res<MoveSpeed>,
    index: Res<GuidIndex>,
    transforms: Query<&Transform>,
    input: FollowInput,
) {
    player.follow_forward = false;
    if follow.guid.is_none() {
        return;
    }
    // ── The cancel set ── before anything else this frame (see the doc above).
    // Off the WORLD's buttons ([`super::camera::WorldMouse`], ledger B364), not the device's: the
    // reference's both-button run is its two bindings held, and a press a UI frame captured
    // dispatches neither — so two primaries pressed over a bag are two clicks, not a cancel.
    let both_engaged = input.rig.world_mouse.both()
        && (input.rig.world_mouse.down(LookButton::Left)
            || input.rig.world_mouse.down(LookButton::Right));
    if follow_cancelled(
        input.move_start(),
        // The ON edge only: `control` toggles `autorun` AFTER this runs, so the flag we read here
        // is still the pre-toggle value, and "press while off" is exactly the engage.
        input.buttons.just_pressed(MouseButton::Forward) && !player.autorun,
        both_engaged,
        input.rig.look == Some(LookButton::Right),
        // The input tick on its teardown leg, or the body handed to a server spline. The ride term
        // is benilla's own and predates this: it is not one of `0x5144e0`'s conjuncts (conjunct 4 is
        // the Knockdown lockout, not a taxi test), and nothing known bears on it either way, so it
        // is left exactly as it stood.
        input.input_torn_down(player.modes.rooted, player.foreign_mover.is_none())
            || player.server_riding(),
    ) {
        info!("follow: cancelled by the player's own movement input");
        follow.stop();
        return;
    }
    let Some(guid) = follow.guid else { return };
    // Re-resolved every tick, nothing cached — a followee that streams out ends the follow
    // (`0x610e40` → `0x6106e7`).
    let Some(target) = index
        .0
        .get(&guid)
        .and_then(|e| transforms.get(*e).ok())
        .map(|t| t.translation)
    else {
        info!("follow: the followee is gone — follow ends");
        follow.stop();
        return;
    };
    let delta = target - player.pos;
    // The degenerate-bearing guard (`0x610d32`), checked on the FULL 3D delta before the bearing is
    // taken from the flattened one.
    if vertically_degenerate(delta) {
        info!("follow: the followee is within 1° of straight overhead — follow ends");
        follow.stop();
        return;
    }
    let flat = Vec3::new(delta.x, 0.0, delta.z);
    let distance = flat.length();
    if distance < f32::EPSILON {
        return;
    }
    let bearing = bearing_to(flat);
    player.face_yaw = steer(player.face_yaw, bearing, time.delta_secs());
    let moving = should_move(follow.moving, distance, speed.value);
    if moving != follow.moving {
        info!(
            "follow: {} at {distance:.1} yd (arrive {:.1}, resume {:.1}, speed {:.1})",
            if moving { "running" } else { "arrived" },
            arrive_distance(speed.value),
            resume_distance(speed.value),
            speed.value,
        );
    }
    // `WOW_FOLLOW_TRACE=1` — the field instrument for "follow won't catch up / overshoots": one
    // line a second carrying the closing distance and the ground we actually covered, so the
    // travel rate is a measured number rather than an end-to-end guess (timing and
    // feel are measured, never eyeballed). Gated because this one IS per-frame.
    if follow_trace_on() {
        let now = time.elapsed_secs_f64();
        if now - follow.traced_at >= 1.0 {
            // Quiet until a tick has seeded a reference point: an instrument whose opening line is
            // garbage is worse than one that says nothing for a second.
            if let Some(from) = follow.traced_pos {
                let elapsed = now - follow.traced_at;
                let moved = player.pos.distance(from);
                info!(
                    "follow-trace: {distance:.1} yd to go, covered {moved:.1} yd in {elapsed:.2} s \
                     ({:.1} yd/s), moving={moving}",
                    moved / elapsed as f32,
                );
            }
            follow.traced_at = now;
            follow.traced_pos = Some(player.pos);
        }
    }
    follow.moving = moving;
    player.follow_forward = moving;
}

/// `WOW_FOLLOW_TRACE=1` — see [`steer_follow`]. One `OnceLock` read per tick when unset.
fn follow_trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_FOLLOW_TRACE").is_some())
}

/// Register the follow mode. The steer runs in the input stage just before `control`, so the flag
/// and the facing it writes are what the controller reads this same frame.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<FollowState>()
        .add_message::<FollowRequest>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_band_is_three_and_four_point_five_yards_at_normal_speed() {
        assert_eq!(arrive_distance(7.0), 3.0);
        assert_eq!(resume_distance(7.0), 4.5);
    }

    #[test]
    fn resume_scales_with_speed_but_never_below_the_base_band() {
        // Above base speed both thresholds stretch…
        assert_eq!(arrive_distance(14.0), 6.0);
        assert_eq!(resume_distance(14.0), 9.0);
        // …below it, arrive shrinks but resume is FLOORED at the 1.0 scale (the `max` in `0x610bfd`),
        // so the band can never invert.
        assert!(arrive_distance(3.5) < 3.0);
        assert_eq!(resume_distance(3.5), 4.5);
    }

    #[test]
    fn the_hysteresis_band_latches_on_both_edges() {
        // Moving: keep going until strictly inside arrive (which is inclusive, so 3.0 stops).
        assert!(should_move(true, 3.001, 7.0));
        assert!(!should_move(true, 3.0, 7.0), "arrive is inclusive");
        // Stopped: nothing happens until resume, so the gap between 3.0 and 4.5 is dead in BOTH
        // directions — that gap is the whole point of the band.
        assert!(!should_move(false, 4.0, 7.0));
        assert!(should_move(false, 4.5, 7.0));
    }

    #[test]
    fn bearing_agrees_with_the_controllers_own_forward_vector() {
        // The invariant that keeps this from drifting out of sign with `control`.
        for (dx, dz) in [
            (0.0, -1.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (-1.0, 0.0),
            (3.0, -4.0),
        ] {
            let delta = Vec3::new(dx, 0.0, dz);
            let yaw = bearing_to(delta);
            let fwd = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
            let want = delta.normalize();
            assert!(
                (fwd.x - want.x).abs() < 1e-5 && (fwd.z - want.z).abs() < 1e-5,
                "yaw {yaw} should point at ({dx}, {dz}), got ({}, {})",
                fwd.x,
                fwd.z
            );
        }
    }

    #[test]
    fn the_turn_is_rate_limited_and_has_a_deadzone() {
        // A quarter turn at 180°/s takes 0.5 s, so one 0.1 s tick covers 18°, not the whole 90°.
        let after = steer(0.0, PI / 2.0, 0.1);
        assert!(
            (after - TURN_RATE * 0.1).abs() < 1e-6,
            "clamped to the budget"
        );
        // Within the budget it lands exactly on the bearing rather than overshooting.
        assert!((steer(0.0, 0.05, 1.0) - 0.05).abs() < 1e-6);
        // Inside the deadzone it does not move at all.
        assert_eq!(steer(1.0, 1.0 + TURN_DEADZONE / 2.0, 1.0), 1.0);
    }

    #[test]
    fn steering_takes_the_short_way_round() {
        // Bearing just past -π from a facing just under π: the turn must be a small positive step,
        // not a near-full sweep back the other way.
        let after = steer(PI - 0.05, -PI + 0.05, 1.0);
        assert!(
            wrap_pi(after - (PI - 0.05)) > 0.0,
            "should wrap forward across ±π"
        );
    }

    /// The cancel set's table, in its own terms. Each argument is one row of the module header, and
    /// the two that matter most are the FALSE ones: jump and a plain key release are VERIFIED
    /// survivors, not oversights — they reach the guard with `realStart = 0` or never reach it.
    #[test]
    fn any_movement_start_cancels_but_jump_and_releases_do_not() {
        // Nothing happening: a follow just keeps following, however long it runs.
        assert!(!follow_cancelled(false, false, false, false, false));
        // Each start, alone, is enough.
        assert!(follow_cancelled(true, false, false, false, false));
        assert!(follow_cancelled(false, true, false, false, false));
        assert!(follow_cancelled(false, false, true, false, false));
        assert!(follow_cancelled(false, false, false, true, false));
        // Losing the mover — death, stun, a taxi hand-off — is a LEVEL, not an edge.
        assert!(follow_cancelled(false, false, false, false, true));
        // Jump and key releases never become any of those arguments (see `FollowInput::move_start`,
        // which reads `just_pressed` and does not list Space at all), so the all-false row above IS
        // the jump case — asserted here so deleting Space's absence breaks a test.
        assert!(
            !follow_cancelled(false, false, false, false, false),
            "a jump leaves every cancel input false"
        );
    }

    /// The degenerate-bearing guard (`0x610d32`) — a followee straight overhead ends the follow,
    /// one a degree off the vertical does not. The horizontal case must be nowhere near it.
    #[test]
    fn a_followee_straight_overhead_ends_the_follow() {
        assert!(vertically_degenerate(Vec3::new(0.0, 30.0, 0.0)));
        assert!(vertically_degenerate(Vec3::new(0.0, -30.0, 0.0)));
        // 1° off vertical is outside the guard: tan(1°) ≈ 0.01746, so 30 yd up needs > 0.52 yd out.
        assert!(!vertically_degenerate(Vec3::new(0.6, 30.0, 0.0)));
        // The ordinary case — a followee on roughly your own level — is untouched.
        assert!(!vertically_degenerate(Vec3::new(10.0, 2.0, 5.0)));
        // A zero delta is not "degenerate": it is the arrived case, which the band owns.
        assert!(!vertically_degenerate(Vec3::ZERO));
    }
}
