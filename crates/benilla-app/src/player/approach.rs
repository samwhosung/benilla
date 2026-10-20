//! Click to Move's walks (`AutoInteract`, "Toggles auto-move to interact target"). With the option
//! on, a right-click on an NPC, a body to loot or skin, or a GameObject beyond the verb's reach
//! walks the player there and the verb runs on arrival; a right-click on the ground walks to the
//! point (mode 4), one on the sky walks that way until stopped (mode 8), and an attack on an enemy
//! out of melee walks to where it stood (mode 10). It is [`super::follow`]'s auto-move: one cell
//! holds either (mode `0xc4d888`, guid `0xc4d980`), so arming one ends the other (`0x611130` calls
//! the canceller `0x60fb60` first), the same input cancels both, and nothing goes on the wire but
//! the movement; an attack on an enemy already in melee only turns to face it (mode 2).

use std::f32::consts::{FRAC_PI_2, PI};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::EntityKind;

use crate::net::{Embodied, Guid, GuidIndex, NetEntity, ObjectStore, SelfPlayer, UnitSpeeds};

use super::follow::{self, FollowInput, FollowState};
use super::state::Player;

/// The verb an approach owes, by its mode in `0xc4d888`; the pending dispatcher `0x60fa20` runs it
/// at the stop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ApproachVerb {
    /// Mode 5: the NPC-service dispatch `0x5f0130`.
    Talk,
    /// Mode 6: the unit loot `0x5df2a0`.
    Loot,
    /// Mode 7: the GameObject use `0x5f86b0`.
    Use,
    /// Mode 9: the skin cast `0x5f05e0`.
    Skin,
}

/// What the start gate (`0x60fed0`) checks the target for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Subject {
    Unit { dead: bool },
    Corpse,
    GameObject,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Refused {
    Silent,
    /// `ERR_AUTOFOLLOW_TOO_FAR` (`0x126`, `0x61110c`).
    TooFar,
}

/// Talk stops at half the 5.5556 yd service reach (`0x60e680`).
pub(crate) const TALK_STOP: f32 = 5.555_555_3 * 0.5;
/// Use and Skin stop at this fraction of the verb's range (`0x804588`).
pub(crate) const RANGE_STOP_FRACTION: f32 = 0.9;
/// The ground walk stops half a yard short (`0x60e740`).
const GROUND_STOP: f32 = 0.5;
/// No approach starts or lasts at 80 yd or more: the rows of modes 5-7, 9 and 10 in `0x860a58`
/// (`0x60e5d1`); the ground and sky rows have none.
const LEASH_SQ: f32 = 6400.0;
/// Within this many yards on the ground the goal's height is ignored (`0x80c4c8`).
const FLAT_SNAP: f32 = 0.5;
/// The sky walk's swim pitch: straight up or down past this sine (`0x80c4d4`), level within the
/// next (`0x80c4d8`).
const SKY_PITCH_STEEP: f32 = 0.866_02;
const SKY_PITCH_LEVEL: f32 = 0.173_64;
/// `automoveturnspeednarrow` and `automoveturnspeedwide`, deg/s (`0x6039e9`, `0x6039ca`).
const TURN_NARROW: f32 = 800.0;
const TURN_WIDE: f32 = 1200.0;
/// Less ground than this in a frame while lined up and holding forward is stuck (`0x80c4dc`).
const STUCK: f32 = 1.0 / 360.0;

/// The approach in flight, and the verb an arrival owes.
#[derive(Resource, Default)]
pub(crate) struct Approach {
    /// `AutoInteract`, `CanAutoInteract`'s last term (`0x60f925`).
    pub(crate) enabled: bool,
    goal: Option<Goal>,
    /// The pending cells `0x60f940` fills (`0xc4da28`, `0xc4da78`, `0xc4d950`, `0xc4da20`).
    pub(crate) arrived: Option<(ApproachVerb, u64)>,
}

/// What a walk heads for, by its mode in `0xc4d888`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Toward {
    /// Modes 5-7 and 9: the object's live position.
    Object { verb: ApproachVerb, guid: u64 },
    /// Mode 4.
    Ground(Anchor),
    /// Mode 10.
    Melee(Anchor),
    /// Mode 8: a facing (`0xc4d9a8`), with no stop.
    Sky(f32),
    /// Mode 2: a facing, turned to and done.
    Face(f32),
}

/// A point fixed at the arm (`0xc4d890`), held in the frame of the transport the player stands on
/// (`0x7c4a00`) and carried back to the world each frame (`0x7c4930`).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Anchor {
    at: Vec3,
    frame: Option<Entity>,
}

impl Toward {
    fn is_talk(self) -> bool {
        matches!(
            self,
            Toward::Object {
                verb: ApproachVerb::Talk,
                ..
            }
        )
    }

    /// Talk's and the melee walk's arrive and snap radii never scale with speed (`0x610ae8`,
    /// `0x610ff1`).
    fn unscaled(self) -> bool {
        self.is_talk() || matches!(self, Toward::Melee(_))
    }

    fn leashed(self) -> bool {
        matches!(self, Toward::Object { .. } | Toward::Melee(_))
    }
}

struct Goal {
    toward: Toward,
    /// `0xc4d910`.
    stop: f32,
    /// Latched by the first turn (`0x61042c`).
    turn_rate: Option<f32>,
    /// Bit 1 of `0xc4da48`: the facing has lined up once.
    aligned: bool,
    /// Last frame's position (`0xc4da84`).
    last_pos: Vec3,
    held: bool,
    /// The swim pitch the sky arm sets (`0x6101ee`), applied on the walk's first frame.
    pitch: Option<f32>,
}

impl Approach {
    /// The reference's `0x6103a0` for this mode family: an approach is in flight.
    pub(crate) fn active(&self) -> bool {
        self.goal.is_some()
    }

    pub(crate) fn stop(&mut self) {
        self.goal = None;
    }

    #[cfg(test)]
    pub(crate) fn stop_distance(&self) -> Option<f32> {
        self.goal.as_ref().map(|g| g.stop)
    }

    #[cfg(test)]
    pub(crate) fn facing(&self) -> bool {
        self.goal
            .as_ref()
            .is_some_and(|g| matches!(g.toward, Toward::Face(_)))
    }
}

/// `CanAutoInteract` (`0x60f900`): alive, driving our own body, and the option on.
/// The canceller `0x60fb60(0, 1)` as the UI shutdown's `/reload` guard calls it (`0x490c1b`): a
/// follow or a walk in flight ends and the mover stops (`0x60fc05`-`0x60fc18`, the mode reset
/// `0x60fc30`). `true` when it ended a follow, mode 3, which owes `AUTOFOLLOW_END` (`0x60fb84`).
pub(crate) fn cancel_move_action_on_ui_shutdown(
    mut follow: ResMut<FollowState>,
    mut approach: ResMut<Approach>,
) -> bool {
    approach.stop();
    follow.stop()
}

pub(crate) fn can_auto_interact(enabled: bool, me: Option<&ObjectStore>, player: &Player) -> bool {
    enabled
        && me.is_some_and(|s| s.0.unit_health().unwrap_or(0) > 0)
        && player.foreign_mover.is_none()
        && !player.control_lost
}

/// The start: the gates (`0x60fed0`, `0x610300`, `CanAutoInteract` for the rest) and the arm
/// `0x611130`.
#[derive(SystemParam)]
pub(crate) struct AutoMove<'w, 's> {
    pub(crate) approach: ResMut<'w, Approach>,
    follow: ResMut<'w, FollowState>,
    player: Res<'w, Player>,
    me: Query<'w, 's, (&'static Guid, &'static ObjectStore), With<SelfPlayer>>,
    transports: Query<'w, 's, &'static Transform>,
}

impl AutoMove<'_, '_> {
    pub(crate) fn can_auto_interact(&self) -> bool {
        can_auto_interact(
            self.approach.enabled,
            self.me.single().ok().map(|(_, s)| s),
            &self.player,
        )
    }

    /// The interaction walks' gate (`0x60fed0`): the verb's own kind of target, never ourselves.
    pub(crate) fn start(
        &mut self,
        verb: ApproachVerb,
        guid: u64,
        subject: Subject,
        at: Vec3,
        stop: f32,
        stand: &mut MessageWriter<super::StandStateRequest>,
    ) -> Result<(), Refused> {
        let Ok((me, store)) = self.me.single() else {
            return Err(Refused::Silent);
        };
        let admitted = match (verb, subject) {
            (ApproachVerb::Talk, Subject::Unit { .. }) => true,
            (ApproachVerb::Loot | ApproachVerb::Skin, Subject::Corpse) => true,
            (ApproachVerb::Loot | ApproachVerb::Skin, Subject::Unit { dead }) => dead,
            (ApproachVerb::Use, Subject::GameObject) => true,
            _ => false,
        };
        if !admitted
            || guid == me.0
            || self.player.foreign_mover.is_some()
            || self.player.control_lost
            || store.0.unit_health().unwrap_or(0) == 0
        {
            return Err(Refused::Silent);
        }
        self.arm(Toward::Object { verb, guid }, at, stop, stand)
    }

    /// `0x6102b0`, the right-click on the ground (`0x5e0378`).
    pub(crate) fn walk_to_ground(
        &mut self,
        at: Vec3,
        stand: &mut MessageWriter<super::StandStateRequest>,
    ) -> bool {
        if !self.can_auto_interact() {
            return false;
        }
        let anchor = self.anchor(at);
        self.arm(Toward::Ground(anchor), at, GROUND_STOP, stand)
            .is_ok()
    }

    /// `0x610100`, the right-click on the sky (`0x492dca`), along the press's ray.
    pub(crate) fn walk_toward_sky(
        &mut self,
        ray: Vec3,
        stand: &mut MessageWriter<super::StandStateRequest>,
    ) -> bool {
        let dir = ray.normalize_or_zero();
        if dir == Vec3::ZERO || !self.can_auto_interact() {
            return false;
        }
        let facing = follow::bearing_to(Vec3::new(dir.x, 0.0, dir.z));
        let at = self.player.pos;
        if self.arm(Toward::Sky(facing), at, 0.0, stand).is_err() {
            return false;
        }
        if let Some(goal) = self.approach.goal.as_mut() {
            goal.pitch = self.player.swimming.then(|| sky_pitch(dir.y));
        }
        true
    }

    /// `0x60fcc0`'s walk, for an attack on an enemy out of melee: to where it stands, stopping
    /// `stop` short.
    /// Deviation: the reference re-projects the point's height onto the first walkable facet a
    /// zero-size box gather finds at the enemy (`0x60fe25`-`0x60fe51`); here it keeps the enemy's
    /// height. The snap radius is the stop, so the two differ only while swimming or on a steep facet.
    /// Deviation: the reference stores the world point unconverted and reads it back as deck-local
    /// (`0x60fe6c`, `0x610e95`), so on a transport it walks astray; here it rides the deck.
    pub(crate) fn walk_into_melee(
        &mut self,
        at: Vec3,
        stop: f32,
        stand: &mut MessageWriter<super::StandStateRequest>,
    ) -> Result<(), Refused> {
        if !self.can_auto_interact() {
            return Err(Refused::Silent);
        }
        let anchor = self.anchor(at);
        self.arm(Toward::Melee(anchor), at, stop, stand)
    }

    /// `0x6100a0`, for an attack on an enemy in melee: turn to face `at`.
    pub(crate) fn face(
        &mut self,
        at: Vec3,
        stand: &mut MessageWriter<super::StandStateRequest>,
    ) -> bool {
        if !self.can_auto_interact() {
            return false;
        }
        let delta = at - self.player.pos;
        let facing = follow::bearing_to(Vec3::new(delta.x, 0.0, delta.z));
        self.arm(Toward::Face(facing), at, 0.0, stand).is_ok()
    }

    /// Deviation: the reference converts back through the transport the player stands on each
    /// frame (`0x610e6b`), so stepping off mid-walk sends the point astray; here the deck is fixed
    /// at the arm.
    fn anchor(&self, at: Vec3) -> Anchor {
        let frame = self.player.ride.as_ref().map(|r| r.entity);
        match frame.and_then(|e| self.transports.get(e).ok().map(|tf| (e, tf))) {
            Some((e, tf)) => Anchor {
                at: tf.compute_affine().inverse().transform_point3(at),
                frame: Some(e),
            },
            None => Anchor { at, frame: None },
        }
    }

    /// `0x611130`: the canceller, then the leash on the world point `at`, then the goal.
    fn arm(
        &mut self,
        toward: Toward,
        at: Vec3,
        stop: f32,
        stand: &mut MessageWriter<super::StandStateRequest>,
    ) -> Result<(), Refused> {
        let standing = self
            .me
            .single()
            .is_ok_and(|(_, s)| s.0.unit_stand_state() == 0);
        self.follow.stop();
        self.approach.stop();
        if toward.leashed() && self.player.pos.distance_squared(at) >= LEASH_SQ {
            return Err(Refused::TooFar);
        }
        info!("approach: {toward:?}, stopping {stop:.2} yd short");
        self.approach.goal = Some(Goal {
            toward,
            stop,
            turn_rate: None,
            aligned: false,
            last_pos: self.player.pos,
            held: false,
            pitch: None,
        });
        if !standing {
            stand.write(super::StandStateRequest { state: 0 });
        }
        Ok(())
    }
}

/// The swim pitch the sky walk takes from its ray (`0x6101b9`-`0x61022c`).
fn sky_pitch(y: f32) -> f32 {
    if y.abs() > SKY_PITCH_STEEP {
        FRAC_PI_2.copysign(y)
    } else if y.abs() > SKY_PITCH_LEVEL {
        y.asin()
    } else {
        0.0
    }
}

fn speed_scale(speed: f32) -> f32 {
    (speed / follow::SPEED_NORM).max(1.0)
}

/// The goal distance, squared (`0x610e40`'s tail): on the ground alone once inside the snap
/// radius, unless swimming. The melee walk's snap radius is its stop (`0x610eaf`).
fn goal_distance_sq(toward: Toward, stop: f32, delta: Vec3, speed: f32, swimming: bool) -> f32 {
    let base = if matches!(toward, Toward::Melee(_)) {
        stop
    } else {
        FLAT_SNAP
    };
    let snap = if toward.unscaled() {
        base
    } else {
        base * speed_scale(speed)
    };
    let flat = delta.x * delta.x + delta.z * delta.z;
    if !swimming && flat <= snap * snap {
        flat
    } else {
        delta.length_squared()
    }
}

/// The arrive distance (`0x610add`-`0x610b07`): the stop distance scaled by the mover's current
/// speed over 7 (`0x7c4c90`, 0 at rest), never below 1×.
fn arrive_distance(toward: Toward, stop: f32, speed: f32) -> f32 {
    if toward.unscaled() {
        stop
    } else {
        stop * speed_scale(speed)
    }
}

/// The rate the first turn latches, rad/s: wide for a turn of more than 120° (`0x61043f`).
fn turn_rate(remaining: f32) -> f32 {
    let deg = if remaining.abs() > 2.0 * PI / 3.0 {
        TURN_WIDE
    } else {
        TURN_NARROW
    };
    deg.to_radians()
}

/// Steer toward the goal and hold forward; on arrival at an object face it and owe its verb. After
/// [`follow::steer_follow`], which clears [`Player::auto_forward`].
pub(super) fn steer_approach(
    time: Res<Time>,
    mut approach: ResMut<Approach>,
    mut player: ResMut<Player>,
    mover: Query<&UnitSpeeds, With<Embodied>>,
    index: Res<GuidIndex>,
    targets: Query<(&Transform, Option<&ObjectStore>, Option<&NetEntity>)>,
    input: FollowInput,
) {
    let Some(goal) = approach.goal.as_mut() else {
        return;
    };
    // `GetCurrentSpeed` on last frame's word, as the reference reads the mover's own.
    let speed = mover
        .single()
        .map_or(0.0, |s| crate::net::current_speed(&s.0, player.move_flags));
    if input.cancels(&player) {
        info!("approach: cancelled by the player's own movement input");
        approach.stop();
        return;
    }
    let toward = goal.toward;
    if let Some(pitch) = goal.pitch.take() {
        player.mover_pitch = pitch;
    }
    let target = match toward {
        Toward::Object { verb, guid } => {
            let Some((tf, store, kind)) = index.0.get(&guid).and_then(|e| targets.get(*e).ok())
            else {
                info!("approach: the target is gone");
                approach.stop();
                return;
            };
            // `0x610f13`-`0x610f40`: a unit on a taxi ends it, a dead one too unless the verb
            // wants a body.
            if kind.is_some_and(|k| matches!(k.kind, EntityKind::Unit | EntityKind::Player)) {
                let store = store.map(|s| &s.0);
                let taxi =
                    store.is_some_and(|s| s.unit_flags() & super::UNIT_FLAG_TAXI_FLIGHT != 0);
                let dead = store.is_none_or(|s| s.unit_health().unwrap_or(0) == 0);
                if taxi || (dead && !matches!(verb, ApproachVerb::Loot | ApproachVerb::Skin)) {
                    info!("approach: the target left reach (taxi {taxi}, dead {dead})");
                    approach.stop();
                    return;
                }
            }
            Some(tf.translation)
        }
        Toward::Ground(anchor) | Toward::Melee(anchor) => match anchor.frame {
            None => Some(anchor.at),
            Some(frame) => {
                let Ok((tf, ..)) = targets.get(frame) else {
                    info!("approach: the transport is gone");
                    approach.stop();
                    return;
                };
                Some(tf.transform_point(anchor.at))
            }
        },
        Toward::Sky(_) | Toward::Face(_) => None,
    };
    let delta = target.map(|t| t - player.pos);
    let d2 = delta.map(|d| goal_distance_sq(toward, goal.stop, d, speed, player.swimming));
    if toward.leashed() && d2.is_some_and(|d2| d2 >= LEASH_SQ) {
        info!("approach: beyond 80 yd");
        approach.stop();
        return;
    }
    // `0x61073c`: once lined up, a frame that ends farther off than the last aborts, but not Talk.
    if let Some(target) = target {
        if goal.aligned
            && !toward.is_talk()
            && goal.last_pos.distance_squared(target) < player.pos.distance_squared(target)
        {
            info!("approach: no longer closing");
            approach.stop();
            return;
        }
    }
    let bearing = match (toward, delta) {
        (Toward::Sky(facing) | Toward::Face(facing), _) => facing,
        (_, Some(d)) => follow::bearing_to(Vec3::new(d.x, 0.0, d.z)),
        (_, None) => player.face_yaw,
    };
    let remaining = follow::wrap_pi(bearing - player.face_yaw);
    let turning = remaining.abs() > follow::TURN_DEADZONE;
    if turning {
        let rate = *goal.turn_rate.get_or_insert_with(|| turn_rate(remaining));
        player.face_yaw = follow::steer(player.face_yaw, bearing, rate, time.delta_secs());
    } else if let Toward::Face(_) = toward {
        // `0x6108c3`: the facing walk ends once lined up, never holding forward.
        approach.goal = None;
        return;
    } else {
        goal.aligned = true;
    }
    if let Toward::Face(_) = toward {
        return;
    }
    if let (Some(d2), Some(delta)) = (d2, delta) {
        let arrive = arrive_distance(toward, goal.stop, speed);
        if d2 <= arrive * arrive {
            info!("approach: arrived at {:.2} yd", d2.sqrt());
            // The canceller faces the armed guid (`0x60fbe8`): the object, and none for the ground.
            // Deviation: the melee arm names the player's own guid (`0x60fe5f`), so the reference
            // turns to face itself, a fixed heading; here the melee walk does not turn.
            if let Toward::Object { verb, guid } = toward {
                if delta.x * delta.x + delta.z * delta.z > f32::EPSILON {
                    player.face_yaw = bearing;
                }
                approach.arrived = Some((verb, guid));
            }
            approach.goal = None;
            return;
        }
    }
    // Rooted, the reference lets go of forward (`0x610b4a`), which also spares the stuck test.
    let hold = !player.modes.rooted;
    if !turning && goal.held && player.pos.distance_squared(goal.last_pos) < STUCK * STUCK {
        info!("approach: stuck");
        approach.stop();
        return;
    }
    if delta.is_some_and(follow::vertically_degenerate) {
        info!("approach: the target is straight overhead");
        approach.stop();
        return;
    }
    goal.last_pos = player.pos;
    goal.held = hold;
    player.auto_forward = hold;
}

pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut approach: ResMut<Approach>) {
    if ev.is("AutoInteract") {
        approach.enabled = ev.flag();
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Approach>().add_observer(on_cvar);
}

#[cfg(test)]
mod tests {
    use super::super::state::{MoveSpeed, PlayerRide};
    use super::super::StandStateRequest;
    use super::*;
    use benilla_protocol::messages::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;

    const fn object(verb: ApproachVerb) -> Toward {
        Toward::Object { verb, guid: 0 }
    }

    const POINT: Anchor = Anchor {
        at: Vec3::ZERO,
        frame: None,
    };

    #[test]
    fn talk_stops_inside_the_service_reach_and_never_scales() {
        let talk = object(ApproachVerb::Talk);
        assert!((TALK_STOP - 2.777_777_7).abs() < 1e-5);
        assert_eq!(arrive_distance(talk, TALK_STOP, 14.0), TALK_STOP);
        let loot = object(ApproachVerb::Loot);
        assert_eq!(arrive_distance(loot, 5.0, 7.0), 5.0);
        assert_eq!(arrive_distance(loot, 5.0, 14.0), 10.0);
        let usage = object(ApproachVerb::Use);
        assert_eq!(arrive_distance(usage, 5.0, 3.5), 5.0);
        assert_eq!(arrive_distance(usage, 5.0, 0.0), 5.0, "at rest");
    }

    /// `0x610ae8`: the melee walk's stop is Talk's kind, never scaled; the ground's is.
    #[test]
    fn the_ground_stop_scales_with_speed_and_the_melee_stop_does_not() {
        assert_eq!(
            arrive_distance(Toward::Ground(POINT), GROUND_STOP, 7.0),
            0.5
        );
        assert_eq!(
            arrive_distance(Toward::Ground(POINT), GROUND_STOP, 14.0),
            1.0
        );
        assert_eq!(arrive_distance(Toward::Melee(POINT), 1.9, 14.0), 1.9);
    }

    #[test]
    fn the_goal_flattens_only_inside_the_snap_radius() {
        let usage = object(ApproachVerb::Use);
        let above = Vec3::new(0.3, 4.0, 0.3);
        assert!((goal_distance_sq(usage, 0.0, above, 7.0, false) - 0.18).abs() < 1e-5);
        assert!((goal_distance_sq(usage, 0.0, above, 7.0, true) - 16.18).abs() < 1e-4);
        let far = Vec3::new(3.0, 4.0, 0.0);
        assert_eq!(goal_distance_sq(usage, 0.0, far, 7.0, false), 25.0);
    }

    /// `0x610eaf`: the melee walk snaps to the ground within its stop, not half a yard, unscaled.
    #[test]
    fn the_melee_goal_flattens_within_its_stop() {
        let melee = Toward::Melee(POINT);
        let above = Vec3::new(1.5, 4.0, 0.0);
        assert!((goal_distance_sq(melee, 1.9, above, 14.0, false) - 2.25).abs() < 1e-5);
        let beyond = Vec3::new(2.0, 4.0, 0.0);
        assert!((goal_distance_sq(melee, 1.9, beyond, 14.0, false) - 20.0).abs() < 1e-4);
    }

    /// `0x6101b9`-`0x61022c`.
    #[test]
    fn the_sky_swim_pitch_clamps_steep_and_levels_shallow() {
        assert_eq!(sky_pitch(0.9), FRAC_PI_2);
        assert_eq!(sky_pitch(-0.9), -FRAC_PI_2);
        assert!((sky_pitch(0.5) - 0.5_f32.asin()).abs() < 1e-6);
        assert_eq!(sky_pitch(0.1), 0.0);
    }

    const ME: u64 = 0x1;
    const VENDOR: u64 = 0xF00D;
    const DT: f32 = 1.0 / 60.0;

    /// Our body at the origin facing -Z, alive (and seated when `stand_state` is set), a vendor at
    /// `at`, and both steers chained as the plugin runs them.
    fn world(at: Vec3, stand_state: u32) -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Approach>()
            .init_resource::<FollowState>()
            .init_resource::<Player>()
            .init_resource::<GuidIndex>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<crate::bindings::BindingsState>()
            .init_resource::<super::super::camera::CameraControl>()
            .init_resource::<super::super::view_subject::ViewSubject>()
            .insert_resource(MoveSpeed {
                value: 7.0,
                env_override: false,
            })
            .add_message::<super::super::StandStateRequest>()
            .add_systems(Update, (follow::steer_follow, steer_approach).chain());
        app.world_mut().resource_mut::<Approach>().enabled = true;
        app.world_mut().spawn((
            SelfPlayer,
            Guid(ME),
            ObjectStore(ObjectFields::from_pairs(&[
                (22, 100),
                (28, 100),
                (138, stand_state),
            ])),
        ));
        let vendor = app
            .world_mut()
            .spawn((Guid(VENDOR), Transform::from_translation(at)))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(VENDOR, vendor);
        app
    }

    fn start(app: &mut App, verb: ApproachVerb, subject: Subject, at: Vec3) -> Result<(), Refused> {
        app.world_mut()
            .run_system_once(
                move |mut auto: AutoMove, mut stand: MessageWriter<StandStateRequest>| {
                    auto.start(verb, VENDOR, subject, at, TALK_STOP, &mut stand)
                },
            )
            .unwrap()
    }

    /// One frame: the steers, then the controller's walk along the facing while forward is held.
    fn frame(app: &mut App, walks: bool) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(DT));
        app.update();
        let mut player = app.world_mut().resource_mut::<Player>();
        if walks && player.auto_forward {
            let forward = Quat::from_rotation_y(player.face_yaw) * Vec3::NEG_Z;
            player.pos += forward * 7.0 * DT;
        }
    }

    #[test]
    fn a_vendor_out_of_reach_is_walked_to_and_owed_the_talk() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 0);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                at
            ),
            Ok(())
        );
        frame(&mut app, true);
        assert!(
            app.world().resource::<Player>().auto_forward,
            "forward from the first frame"
        );
        let mut frames = 1;
        while app.world().resource::<Approach>().arrived.is_none() {
            assert!(frames < 300, "never arrived");
            frame(&mut app, true);
            frames += 1;
        }
        let player = app.world().resource::<Player>();
        let left = player.pos.distance(at);
        assert!(
            left <= TALK_STOP && left > TALK_STOP - 7.0 * DT - 1e-3,
            "stopped {left} yd off, not at the 2.78 yd stop"
        );
        assert!(!player.auto_forward, "forward let go at the stop");
        assert!(
            (follow::wrap_pi(player.face_yaw - follow::bearing_to(at - player.pos))).abs() < 1e-4,
            "facing the vendor"
        );
        let approach = app.world().resource::<Approach>();
        assert_eq!(approach.arrived, Some((ApproachVerb::Talk, VENDOR)));
        assert!(!approach.active());
    }

    #[test]
    fn the_arm_ends_a_follow_and_refuses_past_80_yards() {
        let near = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(near, 0);
        app.world_mut()
            .resource_mut::<FollowState>()
            .start(0xBEEF, "Friend".into());
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                near
            ),
            Ok(())
        );
        assert!(
            app.world().resource::<FollowState>().guid.is_none(),
            "one auto-move cell"
        );

        let far = Vec3::new(80.0, 0.0, 0.0);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                far
            ),
            Err(Refused::TooFar)
        );
        assert!(
            !app.world().resource::<Approach>().active(),
            "the arm cancelled the last walk"
        );
    }

    #[test]
    fn the_gate_admits_each_verb_its_own_kind_of_target() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 0);
        for (verb, subject, ok) in [
            (ApproachVerb::Talk, Subject::Unit { dead: false }, true),
            (ApproachVerb::Talk, Subject::GameObject, false),
            (ApproachVerb::Loot, Subject::Unit { dead: true }, true),
            (ApproachVerb::Loot, Subject::Unit { dead: false }, false),
            (ApproachVerb::Loot, Subject::Corpse, true),
            (ApproachVerb::Skin, Subject::Unit { dead: false }, false),
            (ApproachVerb::Use, Subject::GameObject, true),
            (ApproachVerb::Use, Subject::Unit { dead: false }, false),
        ] {
            let got = start(&mut app, verb, subject, at);
            assert_eq!(got.is_ok(), ok, "{verb:?} at {subject:?}: {got:?}");
        }
    }

    #[test]
    fn a_seated_player_stands_to_walk() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 1);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                at
            ),
            Ok(())
        );
        let asks: Vec<u8> = app
            .world_mut()
            .resource_mut::<Messages<super::super::StandStateRequest>>()
            .drain()
            .map(|m| m.state)
            .collect();
        assert_eq!(asks, vec![0]);
    }

    #[test]
    fn the_players_own_movement_cancels_the_walk() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 0);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                at
            ),
            Ok(())
        );
        frame(&mut app, true);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Forward);
        frame(&mut app, true);
        let approach = app.world().resource::<Approach>();
        assert!(!approach.active() && approach.arrived.is_none());
    }

    #[test]
    fn a_walk_that_cannot_move_gives_up_once_lined_up() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 0);
        assert_eq!(
            start(&mut app, ApproachVerb::Use, Subject::GameObject, at),
            Ok(())
        );
        frame(&mut app, false);
        frame(&mut app, false);
        assert!(
            app.world().resource::<Approach>().active(),
            "still turning, where standing still is not stuck"
        );
        for _ in 0..60 {
            frame(&mut app, false);
        }
        let approach = app.world().resource::<Approach>();
        assert!(!approach.active(), "stuck against a wall");
        assert!(approach.arrived.is_none(), "nothing owed");
    }

    #[test]
    fn the_first_turn_picks_the_wide_rate_past_120_degrees() {
        assert_eq!(turn_rate(0.5), 800f32.to_radians());
        assert_eq!(turn_rate(-2.0 * PI / 3.0), 800f32.to_radians());
        assert_eq!(turn_rate(2.2), 1200f32.to_radians());
        assert_eq!(turn_rate(-PI), 1200f32.to_radians());
    }

    fn run_until_idle(app: &mut App, frames: usize) -> usize {
        for n in 0..frames {
            if !app.world().resource::<Approach>().active() {
                return n;
            }
            frame(app, true);
        }
        frames
    }

    /// `0x6102b0`: the ground walk stops half a yard short at walking pace and owes nothing.
    #[test]
    fn a_ground_walk_stops_half_a_yard_short_and_owes_nothing() {
        let point = Vec3::new(0.0, 0.0, -10.0);
        let mut app = world(Vec3::new(40.0, 0.0, 0.0), 0);
        assert!(app
            .world_mut()
            .run_system_once(
                move |mut auto: AutoMove, mut stand: MessageWriter<StandStateRequest>| auto
                    .walk_to_ground(point, &mut stand)
            )
            .unwrap());
        assert!(run_until_idle(&mut app, 300) < 300, "never arrived");
        let left = app.world().resource::<Player>().pos.distance(point);
        assert!(left <= 0.5 && left > 0.5 - 7.0 * DT - 1e-3, "{left}");
        assert!(app.world().resource::<Approach>().arrived.is_none());
    }

    /// The ground row has no leash (`0x860a8c` = 0): a point past 80 yd still walks.
    #[test]
    fn a_ground_walk_has_no_leash() {
        let mut app = world(Vec3::new(14.0, 0.0, 0.0), 0);
        let far = Vec3::new(0.0, 0.0, -120.0);
        assert!(app
            .world_mut()
            .run_system_once(
                move |mut auto: AutoMove, mut stand: MessageWriter<StandStateRequest>| auto
                    .walk_to_ground(far, &mut stand)
            )
            .unwrap());
        for _ in 0..30 {
            frame(&mut app, true);
        }
        assert!(app.world().resource::<Approach>().active());
    }

    /// `0x60f900`: with the option off, neither world walk arms.
    #[test]
    fn the_world_walks_need_click_to_move() {
        let mut app = world(Vec3::new(14.0, 0.0, 0.0), 0);
        app.world_mut().resource_mut::<Approach>().enabled = false;
        let armed = app
            .world_mut()
            .run_system_once(
                |mut auto: AutoMove, mut stand: MessageWriter<StandStateRequest>| {
                    auto.walk_to_ground(Vec3::new(0.0, 0.0, -10.0), &mut stand)
                        || auto.walk_toward_sky(Vec3::new(1.0, 0.5, 0.0), &mut stand)
                },
            )
            .unwrap();
        assert!(!armed);
    }

    /// `0x610100`: the sky walk turns to the ray's heading and holds forward with no stop (row 8's
    /// `+9` flag, `0x610a93`).
    #[test]
    fn a_sky_walk_turns_to_the_ray_and_keeps_going() {
        let mut app = world(Vec3::new(14.0, 0.0, 0.0), 0);
        assert!(app
            .world_mut()
            .run_system_once(
                |mut auto: AutoMove, mut stand: MessageWriter<StandStateRequest>| auto
                    .walk_toward_sky(Vec3::new(1.0, 0.4, 0.0), &mut stand)
            )
            .unwrap());
        for _ in 0..600 {
            frame(&mut app, true);
        }
        let player = app.world().resource::<Player>();
        let want = follow::bearing_to(Vec3::X);
        assert!(follow::wrap_pi(player.face_yaw - want).abs() < 1e-3);
        assert!(player.auto_forward);
        assert!(player.pos.x > 60.0, "walked {}", player.pos.x);
        assert!(app.world().resource::<Approach>().active());
    }

    /// `0x7c4a00` and `0x7c4930`: a ground point on a deck rides the deck.
    #[test]
    fn a_ground_point_on_a_transport_rides_it() {
        let mut app = world(Vec3::new(40.0, 0.0, 0.0), 0);
        let boat = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().resource_mut::<Player>().ride = Some(PlayerRide {
            entity: boat,
            guid: 0xB0A7,
            local_pos: Vec3::ZERO,
            boat_yaw: 0.0,
        });
        let point = Vec3::new(0.0, 0.0, -10.0);
        assert!(app
            .world_mut()
            .run_system_once(
                move |mut auto: AutoMove, mut stand: MessageWriter<StandStateRequest>| auto
                    .walk_to_ground(point, &mut stand)
            )
            .unwrap());
        app.world_mut()
            .entity_mut(boat)
            .insert(Transform::from_xyz(-6.0, 0.0, 0.0));
        assert!(run_until_idle(&mut app, 300) < 300, "never arrived");
        let left = app
            .world()
            .resource::<Player>()
            .pos
            .distance(Vec3::new(-6.0, 0.0, -10.0));
        assert!(left <= 0.5, "{left}");
    }

    /// `0x60fcc0` → mode 10: the walk stops at its stop, unscaled, owes nothing and does not turn
    /// to face on arrival.
    #[test]
    fn a_melee_walk_stops_at_its_stop_and_owes_nothing() {
        let at = Vec3::new(0.0, 0.0, -10.0);
        let stop = (5.0_f32 - 1.333_333_3).sqrt();
        let mut app = world(Vec3::new(40.0, 0.0, 0.0), 0);
        assert_eq!(
            app.world_mut()
                .run_system_once(
                    move |mut auto: AutoMove, mut stand: MessageWriter<StandStateRequest>| auto
                        .walk_into_melee(at, stop, &mut stand)
                )
                .unwrap(),
            Ok(())
        );
        assert!(run_until_idle(&mut app, 300) < 300, "never arrived");
        let left = app.world().resource::<Player>().pos.distance(at);
        assert!(left <= stop && left > stop - 7.0 * DT - 1e-3, "{left}");
        assert!(app.world().resource::<Approach>().arrived.is_none());
    }

    /// `0x6100a0` → mode 2: ends a follow, turns to the enemy and is done, never walking.
    #[test]
    fn a_face_turns_to_the_enemy_ends_a_follow_and_never_walks() {
        let mut app = world(Vec3::new(14.0, 0.0, 0.0), 0);
        app.world_mut()
            .resource_mut::<FollowState>()
            .start(0xBEEF, "Friend".into());
        let enemy = Vec3::new(3.0, 0.0, 0.0);
        assert!(app
            .world_mut()
            .run_system_once(
                move |mut auto: AutoMove, mut stand: MessageWriter<StandStateRequest>| auto
                    .face(enemy, &mut stand)
            )
            .unwrap());
        assert!(app.world().resource::<FollowState>().guid.is_none());
        let mut walked = false;
        let frames = (0..120)
            .take_while(|_| {
                frame(&mut app, true);
                walked |= app.world().resource::<Player>().auto_forward;
                app.world().resource::<Approach>().active()
            })
            .count();
        assert!(frames < 119, "never lined up");
        assert!(!walked);
        let player = app.world().resource::<Player>();
        assert_eq!(player.pos, Vec3::ZERO);
        assert!(follow::wrap_pi(player.face_yaw - follow::bearing_to(enemy)).abs() < 1e-3);
    }

    /// `0x7c4930` on a turning deck: the point turns with it.
    #[test]
    fn a_ground_point_on_a_turning_transport_turns_with_it() {
        let mut app = world(Vec3::new(40.0, 0.0, 0.0), 0);
        let boat = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().resource_mut::<Player>().ride = Some(PlayerRide {
            entity: boat,
            guid: 0xB0A7,
            local_pos: Vec3::ZERO,
            boat_yaw: 0.0,
        });
        let point = Vec3::new(0.0, 0.0, -10.0);
        assert!(app
            .world_mut()
            .run_system_once(
                move |mut auto: AutoMove, mut stand: MessageWriter<StandStateRequest>| auto
                    .walk_to_ground(point, &mut stand)
            )
            .unwrap());
        app.world_mut()
            .entity_mut(boat)
            .insert(Transform::from_rotation(Quat::from_rotation_y(FRAC_PI_2)));
        assert!(run_until_idle(&mut app, 300) < 300, "never arrived");
        let left = app
            .world()
            .resource::<Player>()
            .pos
            .distance(Vec3::new(-10.0, 0.0, 0.0));
        assert!(left <= 0.5, "{left}");
    }

    /// `0x6101b9`: swimming, the sky walk pitches along its ray.
    #[test]
    fn a_swimming_sky_walk_pitches_along_the_ray() {
        let mut app = world(Vec3::new(14.0, 0.0, 0.0), 0);
        app.world_mut().resource_mut::<Player>().swimming = true;
        assert!(app
            .world_mut()
            .run_system_once(
                |mut auto: AutoMove, mut stand: MessageWriter<StandStateRequest>| auto
                    .walk_toward_sky(Vec3::new(1.0, 1.0, 0.0), &mut stand)
            )
            .unwrap());
        frame(&mut app, false);
        let pitch = app.world().resource::<Player>().mover_pitch;
        assert!(
            (pitch - std::f32::consts::FRAC_PI_4).abs() < 1e-5,
            "{pitch}"
        );
    }
}
