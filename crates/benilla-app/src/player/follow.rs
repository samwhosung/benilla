//! `/follow`, the auto-follow movement mode. It synthesizes input through the setter the
//! MoveForward binding drives (`0x60e790` sets move-forward `0x100000`, `0x60e7f0` clears it), so
//! the controller moves the body and nothing goes on the wire (vmangos has no follow opcode). It
//! beelines with no path (`0x610e40`; `0x670630` only reads click-to-move), ignores the followee's
//! speed, and does not end on arrival.
//!
//! A player's movement start cancels it on the key-down edge: the start emitters call `0x60e990`,
//! which cancels at `0x60e9b5` unless bit 0 of `ds:0xc4da48` is set, and only follow's own emitters
//! (`0x60e790`, `0x60e7f0`, `0x60e8a0`, `0x60e940`) set it; ours is a flag,
//! [`Player::follow_forward`], so it never reaches the cancel. Jump survives (`0x60e990(0, 0)` at
//! `0x60dea8`), as do key releases, sitting, the walk toggle, damage, combat, mounting, casting and
//! the followee leaving range (`0x6107e5` skips follow's mode at `0x610749`, `0x610752`).
//!
//! The reference's turn-away cancel (160°-220°, `0x80c604`, `0x80c600`) runs only while drunk,
//! gated at `0x610a3d` on the drunk fraction, armed as the turn converges and disarmed at random
//! (`0x61092e`); it is not built. The start gate (who may be followed) is
//! [`crate::target::by_name`]'s.

use std::f32::consts::PI;

use bevy::prelude::*;

use crate::net::GuidIndex;

use super::camera::{CameraControl, LookButton};
use super::state::{MoveSpeed, Player};

/// Follow's turn rate in rad/s, its own (`0xc4d93c`, seeded at `0x6111f9`), not the keyboard's.
const TURN_RATE: f32 = PI;

/// Inside this many radians of the bearing, follow stops turning (`0x6108bb` → `0x60e920`).
const TURN_DEADZONE: f32 = 0.001;

/// The speed the thresholds scale by (`0x80c4d0`), the base run speed.
const SPEED_NORM: f32 = 7.0;

/// The base stop distance (`0x80c4c0`), a constant, not a CVar.
const STOP_DISTANCE: f32 = 3.0;

/// Resume sits at 1.5× the stop distance.
const RESUME_FACTOR: f32 = 1.5;

/// `cos 1°` (`0xc4d9d0`, from `0x80c5f8`), the vertical guard's threshold (`0x610d32`).
const VERTICAL_ALIGN_COS: f32 = 0.999_847_7;

/// Who we follow; the reference's guid `0xc4d980` is set at `0x6111c9`, cleared at `0x60fc1e`.
#[derive(Resource, Default)]
pub(crate) struct FollowState {
    pub(crate) guid: Option<u64>,
    /// The followee's name at the start, `AUTOFOLLOW_BEGIN`'s argument, latched for the status.
    pub(crate) name: String,
    /// The band's latch: the synthesized forward input is held.
    moving: bool,
    /// `WOW_FOLLOW_TRACE` bookkeeping: the last trace line's time and position.
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

    /// Stop following; `true` if we were.
    pub(crate) fn stop(&mut self) -> bool {
        self.moving = false;
        self.guid.take().is_some()
    }
}

/// A start-following request: the chat input's `/follow`, or the UI's `FollowUnit`/`FollowByName`.
/// A name finds players only; the start gate refuses a creature either way.
#[derive(bevy::ecs::message::Message, Clone, Debug)]
pub(crate) enum FollowRequest {
    /// `FollowUnit(unit)`; a bare `/follow` is `FollowUnit("target")`.
    Unit(String),
    /// `FollowByName(name, exactMatch)`: the unit popup passes `1`, `/follow <name>` nothing.
    Name { name: String, exact: bool },
}

/// Follow arrives and lets go of the key at this distance or closer (`0x610ad2`-`0x610b1b`).
fn arrive_distance(speed: f32) -> f32 {
    (speed / SPEED_NORM) * STOP_DISTANCE
}

/// A stopped follow resumes at this distance or beyond (`0x610bc4`-`0x610c2b`).
fn resume_distance(speed: f32) -> f32 {
    STOP_DISTANCE * RESUME_FACTOR * (speed / SPEED_NORM).max(1.0)
}

/// Whether the forward key is held this tick: a hysteresis band, so follow does not judder.
fn should_move(was_moving: bool, distance: f32, speed: f32) -> bool {
    if was_moving {
        // Arrive is inclusive, so we keep going only while strictly beyond it.
        distance > arrive_distance(speed)
    } else {
        distance >= resume_distance(speed)
    }
}

/// `move_start` and `autorun_engaged` are key-down edges (the stop emitters and autorun's off edge,
/// `0x60dc90`, have no guard call); `mouse_look` is a level, re-entering the guard every frame.
fn follow_cancelled(
    move_start: bool,
    autorun_engaged: bool,
    both_engaged: bool,
    mouse_look: bool,
    lost_mover: bool,
) -> bool {
    move_start || autorun_engaged || both_engaged || mouse_look || lost_mover
}

/// Wrap an angle into `[-π, π)`.
fn wrap_pi(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let x = (a + PI).rem_euclid(t);
    x - PI
}

/// The `face_yaw` pointing at a horizontal Bevy-space delta, matching `control`'s forward
/// `from_rotation_y(y) * NEG_Z = (-sin y, 0, -cos y)`: `y = atan2(-dx, -dz)`.
fn bearing_to(delta: Vec3) -> f32 {
    (-delta.x).atan2(-delta.z)
}

/// The followee is within 1° of straight up or down, where the bearing means nothing (`0x610d32`).
fn vertically_degenerate(delta: Vec3) -> bool {
    let dist = delta.length();
    dist > f32::EPSILON && delta.y.abs() / dist > VERTICAL_ALIGN_COS
}

/// Turn toward `bearing` by at most `rate × dt`, not inside the deadzone (`0x6103d0`).
fn steer(face: f32, bearing: f32, dt: f32) -> f32 {
    let remaining = wrap_pi(bearing - face);
    if remaining.abs() <= TURN_DEADZONE {
        return face;
    }
    let budget = TURN_RATE * dt;
    face + remaining.clamp(-budget, budget)
}

/// Everything the cancel set reads.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct FollowInput<'w, 's> {
    buttons: Res<'w, ButtonInput<MouseButton>>,
    /// The movement commands' press edges, wherever bound; typing in chat does not reach them.
    binds: Res<'w, crate::bindings::BindingsState>,
    /// The mouse-look level, read a frame behind `control`, so its cancel lands a frame after the
    /// reference's; the world's mouse buttons are current ([`super::camera::latch_world_mouse`]
    /// runs first).
    rig: Res<'w, CameraControl>,
    /// The mover's descriptor block, for the cancel terms that are state: health and stun.
    mover: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::Embodied>>,
    /// What the camera orbits, for the far-sight term; possibly a frame behind its publisher.
    view_subject: Res<'w, super::view_subject::ViewSubject>,
}

impl FollowInput<'_, '_> {
    /// The press edge of any of the six movement commands; turn and strafe both cancel.
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

    /// The input tick's teardown leg ([`super::state::MoverInput::torn_down`]), which the reference
    /// takes (`0x5146d6 call 0x60fb60`) only with both predicates down (`0x5146c3`, `0x5146ce`):
    /// death, far sight, or root and stun together end a follow, a root alone does not.
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

/// Cancel, re-resolve the followee, steer, and set [`Player::follow_forward`] for `control`. The
/// cancel comes first, as the reference's start emitters run ahead of the axis math (`0x5150a7` vs
/// the emitter tail `0x5151a0`).
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
    // ── The cancel set ──
    // The world's buttons, not the device's: the both-button run is two bindings held, and a
    // press a UI frame captured dispatches neither.
    let both_engaged = input.rig.world_mouse.both()
        && (input.rig.world_mouse.down(LookButton::Left)
            || input.rig.world_mouse.down(LookButton::Right));
    if follow_cancelled(
        input.move_start(),
        // The on edge: `control` toggles `autorun` after this, so the flag is the pre-toggle value.
        input.buttons.just_pressed(MouseButton::Forward) && !player.autorun,
        both_engaged,
        input.rig.look == Some(LookButton::Right),
        // The teardown leg, or the body handed to a server spline; the ride term is not one of the
        // reference's conjuncts (`0x5144e0`), and its reference side is untraced.
        input.input_torn_down(player.modes.rooted, player.foreign_mover.is_none())
            || player.server_riding(),
    ) {
        info!("follow: cancelled by the player's own movement input");
        follow.stop();
        return;
    }
    let Some(guid) = follow.guid else { return };
    // A followee that streams out ends the follow (`0x610e40` → `0x6106e7`).
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
    // On the full 3D delta, before the bearing flattens it.
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
    // `WOW_FOLLOW_TRACE=1`: once a second, the distance left and the ground covered.
    if follow_trace_on() {
        let now = time.elapsed_secs_f64();
        if now - follow.traced_at >= 1.0 {
            // Silent until a first tick seeds the start point.
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

/// Whether `WOW_FOLLOW_TRACE` is set, read once.
fn follow_trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_FOLLOW_TRACE").is_some())
}

/// Register the follow state and its request message.
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
        assert_eq!(arrive_distance(14.0), 6.0);
        assert_eq!(resume_distance(14.0), 9.0);
        // Below base speed, resume's scale is floored at 1.0 (`0x610bfd`).
        assert!(arrive_distance(3.5) < 3.0);
        assert_eq!(resume_distance(3.5), 4.5);
    }

    #[test]
    fn the_hysteresis_band_latches_on_both_edges() {
        assert!(should_move(true, 3.001, 7.0));
        assert!(!should_move(true, 3.0, 7.0), "arrive is inclusive");
        assert!(!should_move(false, 4.0, 7.0));
        assert!(should_move(false, 4.5, 7.0));
    }

    #[test]
    fn bearing_agrees_with_the_controllers_own_forward_vector() {
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
        // 180°/s for a 0.1 s tick is 18° of the 90°.
        let after = steer(0.0, PI / 2.0, 0.1);
        assert!(
            (after - TURN_RATE * 0.1).abs() < 1e-6,
            "clamped to the budget"
        );
        assert!((steer(0.0, 0.05, 1.0) - 0.05).abs() < 1e-6);
        assert_eq!(steer(1.0, 1.0 + TURN_DEADZONE / 2.0, 1.0), 1.0);
    }

    #[test]
    fn steering_takes_the_short_way_round() {
        let after = steer(PI - 0.05, -PI + 0.05, 1.0);
        assert!(
            wrap_pi(after - (PI - 0.05)) > 0.0,
            "should wrap forward across ±π"
        );
    }

    /// Jump and key releases survive: they reach the guard with `realStart = 0` or never reach it.
    #[test]
    fn any_movement_start_cancels_but_jump_and_releases_do_not() {
        assert!(!follow_cancelled(false, false, false, false, false));
        assert!(follow_cancelled(true, false, false, false, false));
        assert!(follow_cancelled(false, true, false, false, false));
        assert!(follow_cancelled(false, false, true, false, false));
        assert!(follow_cancelled(false, false, false, true, false));
        assert!(follow_cancelled(false, false, false, false, true));
        assert!(
            !follow_cancelled(false, false, false, false, false),
            "a jump leaves every cancel input false"
        );
    }

    #[test]
    fn a_followee_straight_overhead_ends_the_follow() {
        assert!(vertically_degenerate(Vec3::new(0.0, 30.0, 0.0)));
        assert!(vertically_degenerate(Vec3::new(0.0, -30.0, 0.0)));
        // tan(1°) ≈ 0.01746, so 30 yd up needs more than 0.52 yd out.
        assert!(!vertically_degenerate(Vec3::new(0.6, 30.0, 0.0)));
        assert!(!vertically_degenerate(Vec3::new(10.0, 2.0, 5.0)));
        assert!(!vertically_degenerate(Vec3::ZERO));
    }
}
