//! `/follow`, the auto-follow movement mode. It synthesizes input through the setter the
//! MoveForward binding drives (`0x60e790` sets move-forward `0x100000`, `0x60e7f0` clears it), so
//! the controller moves the body and nothing goes on the wire (vmangos has no follow opcode). It
//! beelines with no path (`0x610e40`; `0x670630` only reads click-to-move), ignores the followee's
//! speed, and does not end on arrival.
//!
//! A player's movement start cancels it on the key-down edge: the start emitters call `0x60e990`,
//! which cancels at `0x60e9b5` unless bit 0 of `ds:0xc4da48` is set, and only follow's own emitters
//! (`0x60e790`, `0x60e7f0`, `0x60e8a0`, `0x60e940`) set it; ours is a flag,
//! [`Player::auto_forward`], so it never reaches the cancel. Jump survives (`0x60e990(0, 0)` at
//! `0x60dea8`), as do key releases, sitting, the walk toggle, damage, combat, mounting, casting and
//! the followee leaving range (`0x6107e5` skips follow's mode at `0x610749`, `0x610752`).
//!
//! The reference's turn-away cancel (160°-220°, `0x80c604`, `0x80c600`) runs only while drunk,
//! gated at `0x610a3d` on the drunk fraction, armed as the turn converges and disarmed at random
//! (`0x61092e`); it is not built. The start gate (who may be followed) is
//! [`crate::target::by_name`]'s, and the start signals `AUTOFOLLOW_BEGIN` inside its call
//! (`0x610071`, [`crate::script_calls::ScriptCallHost`]).
//!
//! Every end goes through the canceller `0x60fb60`, which signals `AUTOFOLLOW_END` for a follow
//! first (`0x60fb76`-`0x60fb84`), where the end runs ([`end`]): the packet handlers' ends at their
//! packets ([`super::AutoMoveStop`]), the input's and the steer's at their place in the frame.
//!
//! Deviation: every end signals `END`, because the reference's three silent ends are a bug: two
//! writers reset the follow mode cell `0xc4d888` directly (`0x5fb646`, `0x60396f`), leaving a
//! stale guid at `0xc4d980`, and the canceller's early-out (`0x60fb70`) fires nothing.

use std::f32::consts::PI;

use bevy::prelude::*;

use crate::net::GuidIndex;

use super::state::{MoveSpeed, Player};

/// Follow's turn rate in rad/s, its own (`0xc4d93c`, seeded at `0x6111f9`), not the keyboard's.
const TURN_RATE: f32 = PI;

/// Inside this many radians of the bearing, follow stops turning (`0x6108bb` → `0x60e920`).
pub(super) const TURN_DEADZONE: f32 = 0.001;

/// The speed the thresholds scale by (`0x80c4d0`), the base run speed.
pub(super) const SPEED_NORM: f32 = 7.0;

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
    /// A packet's auto-move stop owes its forward release (`0x60e7f0`): it clears autorun
    /// (`0x514630`, [`super::press::CallAutorun::clear`]) behind its `AUTOFOLLOW_END`, and the
    /// frame's steer ([`steer_follow`]) applies the bit the calls left before the controller's step.
    release_owed: bool,
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

    /// Owe the forward release of an auto-move a packet stopped ([`super::end_auto_move`]).
    pub(crate) fn owe_release(&mut self) {
        self.release_owed = true;
    }

    /// Whether a stop owes the frame its release, for a test.
    #[cfg(test)]
    pub(crate) fn owes_release(&self) -> bool {
        self.release_owed
    }

    /// Stop following; `true` if we were.
    pub(crate) fn stop(&mut self) -> bool {
        self.moving = false;
        self.guid.take().is_some()
    }
}

/// The canceller's follow leg (`0x60fb60`): a follow in flight signals `AUTOFOLLOW_END` (`0x60fb84`)
/// on the notice lane at the caller's place, then ends (`0x60fc1e`); `true` if one ended. With no
/// lane, in a harness without the unit feed, it ends silently.
pub(crate) fn end(
    follow: &mut FollowState,
    lane: Option<&mut crate::field_watch::NoticeLane>,
) -> bool {
    if follow.guid.is_none() {
        return false;
    }
    if let Some(lane) = lane {
        lane.write(crate::field_watch::FieldNotice::Global {
            id: crate::field_watch::id::AUTOFOLLOW_END,
        });
    }
    follow.stop()
}

/// A start-following request: `FollowUnit` or `FollowByName`. A name finds players only; the start
/// gate refuses a creature either way.
#[derive(Clone, Debug)]
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
/// `0x60dc90`, have no guard call); `hand_off` is the mouse's hand-off to the body, whose facing
/// write reaches the guard (`0x5103e0` → `0x5152e0` → `0x60de30` → `0x60e990(1, 1)`) on each
/// motion that turns the body: behind the TurnOrAction bit (`0x5145fa`), not the look.
fn follow_cancelled(
    move_start: bool,
    autorun_engaged: bool,
    both_engaged: bool,
    hand_off: bool,
    lost_mover: bool,
) -> bool {
    move_start || autorun_engaged || both_engaged || hand_off || lost_mover
}

/// Wrap an angle into `[-π, π)`.
pub(super) fn wrap_pi(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let x = (a + PI).rem_euclid(t);
    x - PI
}

/// The `face_yaw` pointing at a horizontal Bevy-space delta, matching `control`'s forward
/// `from_rotation_y(y) * NEG_Z = (-sin y, 0, -cos y)`: `y = atan2(-dx, -dz)`.
pub(super) fn bearing_to(delta: Vec3) -> f32 {
    (-delta.x).atan2(-delta.z)
}

/// The followee is within 1° of straight up or down, where the bearing means nothing (`0x610d32`).
pub(super) fn vertically_degenerate(delta: Vec3) -> bool {
    let dist = delta.length();
    dist > f32::EPSILON && delta.y.abs() / dist > VERTICAL_ALIGN_COS
}

/// Turn toward `bearing` by at most `rate × dt`, not inside the deadzone (`0x6103d0`).
pub(super) fn steer(face: f32, bearing: f32, rate: f32, dt: f32) -> f32 {
    let remaining = wrap_pi(bearing - face);
    if remaining.abs() <= TURN_DEADZONE {
        return face;
    }
    let budget = rate * dt;
    face + remaining.clamp(-budget, budget)
}

/// Everything the cancel set reads.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct FollowInput<'w, 's> {
    buttons: Res<'w, ButtonInput<MouseButton>>,
    /// The movement commands' press edges, wherever bound; typing in chat does not reach them.
    binds: Res<'w, crate::bindings::BindingsState>,
    /// The mouse's hand-off to the body this frame ([`super::LookSample`]), written by the look
    /// session ahead of this; absent in a harness without the mouse, which hands nothing off.
    sample: Option<Res<'w, super::LookSample>>,
    /// The mover's descriptor block, for the cancel terms that are state: health and stun.
    mover: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::Embodied>>,
    /// The far-sight latch, the input tick's far-sight term (`0x514540`).
    far_sight: Res<'w, super::view_subject::FarSight>,
    /// Whether we drive our own body.
    lock: Res<'w, super::MoverLock>,
}

impl FollowInput<'_, '_> {
    /// The press edge of any of the six movement commands; turn and strafe both cancel. A call's
    /// own edge ran its start's stop inside it ([`super::press::TickLegs`]).
    fn move_start(&self) -> bool {
        use crate::bindings::Input;
        [
            Input::MoveForward,
            Input::MoveBackward,
            Input::TurnLeft,
            Input::TurnRight,
            Input::StrafeLeft,
            Input::StrafeRight,
        ]
        .iter()
        .any(|&c| self.binds.just_pressed(c) && !self.binds.settled(c))
    }

    /// The cancel set, the start emitters' `0x60e990` and the input tick's teardown, which end any
    /// auto-move.
    pub(super) fn cancels(&self, player: &Player) -> bool {
        // The world's buttons, not the device's: the both-button run is two bindings held, and a
        // press a UI frame captured dispatches neither.
        let both_engaged = super::input::both_engaged(&self.binds);
        follow_cancelled(
            self.move_start(),
            // The on edge only, as the start reaches the guard (`0x60dc60` → `0x60e990`) and the
            // stop does not (`0x60dc90`): the press toggled `autorun` first ([`super::press`]),
            // so the flag is the post-toggle value.
            self.buttons.just_pressed(MouseButton::Forward)
                && player.autorun
                && !self.binds.settled(crate::bindings::Input::ToggleAutoRun),
            both_engaged,
            self.sample.as_deref().is_some_and(|s| s.hands_off),
            // The teardown leg, or the body handed to a server spline; the ride term is not one of
            // the reference's conjuncts (`0x5144e0`), and its reference side is untraced.
            self.input_torn_down(player.modes.rooted, self.lock.foreign_mover.is_none())
                || player.server_riding(),
        )
    }

    /// The input tick's teardown leg ([`super::state::MoverInput::torn_down`]), which the reference
    /// takes (`0x5146d6 call 0x60fb60`) only with both predicates down (`0x5146c3`, `0x5146ce`):
    /// death, far sight, or root and stun together end a follow, a root alone does not.
    fn input_torn_down(&self, rooted: bool, driving_own_body: bool) -> bool {
        self.mover.single().is_ok_and(|s| {
            super::state::MoverInput {
                dead: s.0.unit_is_dead(),
                view_is_out: super::state::view_is_out(driving_own_body, self.far_sight.on()),
            }
            .torn_down(rooted, s.0.unit_flags() & super::UNIT_FLAG_STUNNED != 0)
        })
    }
}

/// Cancel, re-resolve the followee, steer, and set [`Player::auto_forward`] for `control`. The
/// cancel comes first, as the reference's start emitters run ahead of the axis math (`0x5150a7` vs
/// the emitter tail `0x5151a0`). Each end here is frame time's (`0x5146d6`, `0x6106b0`), after the
/// packets, and signals behind them ([`end`]).
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
pub(super) fn steer_follow(
    time: Res<Time>,
    mut follow: ResMut<FollowState>,
    mut player: ResMut<Player>,
    speed: Res<MoveSpeed>,
    index: Res<GuidIndex>,
    transforms: Query<&Transform>,
    input: FollowInput,
    mut lane: Option<ResMut<crate::field_watch::NoticeLane>>,
    called: Option<Res<super::press::CallAutorun>>,
) {
    player.auto_forward = false;
    release_owed(&mut follow, &mut player, called.as_deref());
    if follow.guid.is_none() {
        return;
    }
    if input.cancels(&player) {
        info!("follow: cancelled by the player's own movement input");
        end(&mut follow, lane.as_deref_mut());
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
        end(&mut follow, lane.as_deref_mut());
        return;
    };
    let delta = target - player.pos;
    // On the full 3D delta, before the bearing flattens it.
    if vertically_degenerate(delta) {
        info!("follow: the followee is within 1° of straight overhead — follow ends");
        end(&mut follow, lane.as_deref_mut());
        return;
    }
    let flat = Vec3::new(delta.x, 0.0, delta.z);
    let distance = flat.length();
    if distance < f32::EPSILON {
        return;
    }
    let bearing = bearing_to(flat);
    player.face_yaw = steer(player.face_yaw, bearing, TURN_RATE, time.delta_secs());
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
    player.auto_forward = moving;
}

/// The forward release a stop owed (`0x60e7f0`): autorun as the calls left it after the stop's
/// clear (`0x514630`), clear with no store for it (a harness). The synthesized forward is the
/// steer's own, rewritten each frame.
pub(super) fn release_owed(
    follow: &mut FollowState,
    player: &mut Player,
    called: Option<&super::press::CallAutorun>,
) {
    if std::mem::take(&mut follow.release_owed) {
        player.autorun = called.is_some_and(|c| c.bit(false));
    }
}

/// [`steer_follow`]'s release of what a packet's stop owed, for a test.
#[cfg(test)]
pub(crate) fn release_owed_for_test(world: &mut World) {
    world.resource_scope(|world, mut follow: Mut<FollowState>| {
        world.resource_scope(|world, mut player: Mut<Player>| {
            release_owed(
                &mut follow,
                &mut player,
                world.get_resource::<super::press::CallAutorun>(),
            );
        });
    });
}

/// Whether `WOW_FOLLOW_TRACE` is set, read once.
fn follow_trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_FOLLOW_TRACE").is_some())
}

/// Register the follow state.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<FollowState>();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Far sight on, its object gone: the render's update (`0x483054` → `0x5ee290`) finds the field
    /// naming no held object, puts the camera back on the player and returns with the bit still
    /// set (`0x5ee324`-`0x5ee360`), so the input tick's teardown leg still ends a follow
    /// (`0x514540`, `0x5146d6`), though the camera is home.
    #[test]
    fn a_lost_far_sight_object_still_ends_a_follow() {
        use super::super::view_subject::{publish_view_subject, FarSight, ViewSubject};
        use crate::net::{Embodied, NetCommands, ObjectStore, SelfPlayer};
        /// `PLAYER_FARSIGHT`, the guid's low word.
        const FARSIGHT: u16 = 712;
        const EYE: u64 = 0xF130_0000_0000_0043;
        const LEADER: u64 = 0x52;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<FollowState>()
            .init_resource::<Player>()
            .init_resource::<super::super::MoverLock>()
            .init_resource::<GuidIndex>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<crate::bindings::BindingsState>()
            .init_resource::<super::super::camera::CameraControl>()
            .init_resource::<ViewSubject>()
            .init_resource::<FarSight>()
            .insert_resource(NetCommands(tx))
            .insert_resource(MoveSpeed {
                value: 7.0,
                env_override: false,
            })
            .add_systems(Update, (publish_view_subject, steer_follow).chain());
        let world = app.world_mut();
        world.spawn((
            SelfPlayer,
            Embodied,
            Transform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (22, 100),
                (28, 100),
                (FARSIGHT, EYE as u32),
                (FARSIGHT + 1, (EYE >> 32) as u32),
            ])),
        ));
        let eye = world.spawn(Transform::from_xyz(30.0, 0.0, 0.0)).id();
        let leader = world.spawn(Transform::from_xyz(10.0, 0.0, 0.0)).id();
        let mut index = world.resource_mut::<GuidIndex>();
        index.0.insert(EYE, eye);
        index.0.insert(LEADER, leader);
        let world = app.world();
        let index = world.resource::<GuidIndex>();
        world.resource::<FarSight>().start(
            Some(EYE),
            |g| index.0.contains_key(&g),
            world.resource::<NetCommands>(),
        );
        app.world_mut().resource_mut::<GuidIndex>().0.remove(&EYE);
        app.update();
        let world = app.world();
        let far_sight = world.resource::<FarSight>();
        assert!(far_sight.on() && far_sight.camera().is_none());
        assert!(
            world.resource::<ViewSubject>().remote.is_none(),
            "the camera is home"
        );

        app.world_mut()
            .resource_mut::<FollowState>()
            .start(LEADER, "Leader".into());
        app.update();
        assert_eq!(
            app.world().resource::<FollowState>().guid,
            None,
            "the bit still tears the input down"
        );
    }

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
        let after = steer(0.0, PI / 2.0, TURN_RATE, 0.1);
        assert!(
            (after - TURN_RATE * 0.1).abs() < 1e-6,
            "clamped to the budget"
        );
        assert!((steer(0.0, 0.05, TURN_RATE, 1.0) - 0.05).abs() < 1e-6);
        assert_eq!(steer(1.0, 1.0 + TURN_DEADZONE / 2.0, TURN_RATE, 1.0), 1.0);
    }

    #[test]
    fn steering_takes_the_short_way_round() {
        let after = steer(PI - 0.05, -PI + 0.05, TURN_RATE, 1.0);
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
