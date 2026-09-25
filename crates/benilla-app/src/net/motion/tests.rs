//! Tests for the motion kernels: the spline sampler, dead-reckoning, jump ballistics, the relay
//! chain, facing and gameobject placement.

use std::time::{Duration, Instant};

use benilla_protocol::{JumpInfo, MonsterMoveFacing, MoveSpeeds};
use bevy::prelude::Quat;

use benilla_protocol::RelayVerb;

use crate::creature_anim::move_flags;
use crate::player::{GRAVITY, TERMINAL_VELOCITY};

use super::facing::resolve_facing;
use super::relay::RelayChain;
use super::remote::{facing_lerp, fall_arc_step, jump_seed, reconcile_lerp};
use super::spline::monster_move_spline;
use super::{RemoteMotion, Spline};

fn speeds() -> MoveSpeeds {
    MoveSpeeds {
        walk: 2.5,
        run: 7.0,
        run_back: 4.5,
        swim: 4.0,
        swim_back: 0.0,
        turn_rate: std::f32::consts::PI,
    }
}

#[test]
fn remote_fall_arc_reports_height_only_on_the_landing_edge() {
    assert_eq!(fall_arc_step(false, true, None, 100.0), (Some(100.0), None));
    assert_eq!(
        fall_arc_step(true, true, Some(100.0), 80.0),
        (Some(100.0), None)
    );
    assert_eq!(
        fall_arc_step(true, false, Some(100.0), 70.0),
        (None, Some(30.0))
    );
    // Entered view mid-fall: no takeoff, no prediction.
    assert_eq!(fall_arc_step(true, false, None, 70.0), (None, None));
    assert_eq!(fall_arc_step(false, false, None, 70.0), (None, None));
}

fn motion(flags: u32, orientation: f32) -> RemoteMotion {
    RemoteMotion {
        wow_pos: [0.0, 0.0, 0.0],
        pending: std::collections::VecDeque::new(),
        orientation,
        flags,
        pitch: 0.0,
        speed: 0.0,
        vertical_velocity: 0.0,
        jump_xy_vel: [0.0, 0.0],
        fall_start_z: None,
        relay: Default::default(),
        last_apply_ms: 0.0,
        last_apply_pos: [0.0, 0.0, 0.0],
    }
}

/// The swim body-pitch render law (`0x60a110`) on a relayed swimmer, and the three cases that
/// render level.
#[test]
fn a_relayed_swimmer_renders_pitched_and_the_gates_render_level() {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::EulerRot;

    use crate::creature_anim::swim_body_rotation;

    // (yaw, pitch, roll).
    let decompose = |flags: u32, pitch: f32, yaw: f32| {
        swim_body_rotation(yaw, flags, pitch).to_euler(EulerRot::YXZ)
    };

    // A swimmer nose-down 0.6 rad on a 1.2 rad heading.
    let mut rm = motion(0, 0.0);
    let mv = super::relay::RelayMove {
        wire_ms: 0,
        position: [0.0; 3],
        orientation: 1.2,
        flags: move_flags::SWIMMING | move_flags::FORWARD,
        pitch: -0.6,
        fall_time: 0,
        jump: None,
        transport: None,
        verb: benilla_protocol::RelayVerb::Pose,
    };
    // Through `apply_move`, the seam the relay and the queue drain share.
    let mut world = bevy::prelude::World::new();
    world.init_resource::<bevy::ecs::message::Messages<crate::creature_anim::HardLanding>>();
    let e = world.spawn_empty().id();
    world
        .run_system_once(
            move |mut commands: bevy::prelude::Commands,
                  mut landings: bevy::prelude::MessageWriter<crate::creature_anim::HardLanding>| {
                let mut rm = motion(0, 0.0);
                super::remote::apply_move(e, &mv, &mut rm, 0.0, &mut commands, &mut landings);
                rm
            },
        )
        .map(|applied| rm = applied)
        .expect("the one-shot apply runs");
    assert_eq!(rm.pitch, -0.6, "the wire pitch reaches the component");
    assert_eq!(
        rm.flags,
        move_flags::SWIMMING | move_flags::FORWARD,
        "…and so do the flags its render gate reads"
    );

    let (yaw, pitch, roll) = decompose(rm.flags, rm.pitch, rm.orientation);
    assert!((yaw - 1.2).abs() < 1e-5, "the heading is untouched: {yaw}");
    assert!(
        (pitch + 0.6).abs() < 1e-5,
        "nose-down 0.6 rad renders: {pitch}"
    );
    assert!(roll.abs() < 1e-5, "a swimmer never banks: {roll}");

    let (_, up, _) = decompose(rm.flags, 0.6, 0.0);
    assert!((up - 0.6).abs() < 1e-5, "nose-up 0.6 rad renders: {up}");

    for (name, flags) in [
        ("an idle floater", move_flags::SWIMMING),
        (
            "a strafe-only swim",
            move_flags::SWIMMING | move_flags::STRAFE_LEFT,
        ),
        ("a walker on dry land", move_flags::FORWARD),
    ] {
        let (yaw, pitch, roll) = decompose(flags, 0.9, 1.2);
        assert!((yaw - 1.2).abs() < 1e-5, "{name} keeps its heading");
        assert_eq!(pitch, 0.0, "{name} renders level");
        assert_eq!(roll, 0.0, "{name} never banks");
    }
}

#[test]
fn swim_dead_reckon_folds_the_pitch_into_the_travel() {
    // `0x7c5880`: vertical sin(pitch) * swim speed, horizontal scaled by cos(pitch).
    let pitch = 0.5_f32;
    let mut rm = motion(move_flags::SWIMMING | move_flags::FORWARD, 0.0);
    rm.pitch = pitch;
    let (pos, _, vertical, speed) = rm.advance(speeds(), 1.0);
    // Facing +X, swim speed 4.0 for 1 s.
    assert!(
        (pos[0] - 4.0 * pitch.cos()).abs() < 1e-4,
        "horizontal shrinks by cos(pitch): {}",
        pos[0]
    );
    assert!(
        (pos[2] - 4.0 * pitch.sin()).abs() < 1e-4,
        "the dive/climb is sin(pitch)·speed: {}",
        pos[2]
    );
    assert_eq!(
        vertical, 0.0,
        "no ballistic vertical persists for a swimmer"
    );
    assert!((speed - 4.0).abs() < 1e-5, "anim rate reads the 3D speed");

    let level = motion(move_flags::SWIMMING | move_flags::FORWARD, 0.0);
    let (pos, ..) = level.advance(speeds(), 1.0);
    assert_eq!(pos[2], 0.0);
    let mut idle = motion(move_flags::SWIMMING, 0.0);
    idle.pitch = -1.0;
    let (pos, _, _, speed) = idle.advance(speeds(), 1.0);
    assert_eq!((pos[0], pos[2], speed), (0.0, 0.0, 0.0));
}

#[test]
fn resolve_facing_angle_spot_and_target() {
    let none = |_g: u64| None;
    assert_eq!(
        resolve_facing(MonsterMoveFacing::Angle(1.25), [0.0; 3], none),
        Some(1.25)
    );
    // WoW +X is north: orientation 0.
    assert_eq!(
        resolve_facing(MonsterMoveFacing::Spot([5.0, 0.0, 0.0]), [0.0; 3], none),
        Some(0.0)
    );
    // WoW +Y is west: orientation +pi/2.
    assert_eq!(
        resolve_facing(MonsterMoveFacing::Spot([0.0, 5.0, 0.0]), [0.0; 3], none),
        Some(std::f32::consts::FRAC_PI_2)
    );
    assert_eq!(
        resolve_facing(MonsterMoveFacing::Target(0x42), [1.0, 1.0, 0.0], |g| {
            (g == 0x42).then_some([1.0, 6.0, 0.0])
        }),
        Some(std::f32::consts::FRAC_PI_2)
    );
    assert_eq!(
        resolve_facing(MonsterMoveFacing::None, [0.0; 3], none),
        None
    );
    assert_eq!(
        resolve_facing(MonsterMoveFacing::Target(0x1), [0.0; 3], none),
        None
    );
    assert_eq!(
        resolve_facing(
            MonsterMoveFacing::Spot([0.0, 0.0, 9.0]),
            [0.0, 0.0, 0.0],
            none
        ),
        None,
        "a point directly above (no horizontal delta) is degenerate"
    );
}

#[test]
fn remote_motion_runs_forward_along_facing() {
    let (pos, o, _vz, speed) = motion(move_flags::FORWARD, 0.0).advance(speeds(), 1.0);
    assert!((pos[0] - 7.0).abs() < 1e-3, "forward advances +X: {pos:?}");
    assert!(pos[1].abs() < 1e-3, "no lateral drift: {pos:?}");
    assert_eq!(o, 0.0, "forward doesn't turn");
    assert_eq!(speed, 7.0, "uses run speed");
}

#[test]
fn remote_motion_backpedal_uses_run_back_speed() {
    let (pos, _o, _vz, speed) = motion(move_flags::BACKWARD, 0.0).advance(speeds(), 1.0);
    assert!(
        (pos[0] + 4.5).abs() < 1e-3,
        "backpedal advances −X by run_back: {pos:?}"
    );
    assert_eq!(speed, 4.5);
}

/// The walk arm (`0x7c4d11` to `0x7c4d4d`) comes before the run arm's backward min (`0x7c4d1d`),
/// so a walk is `min(walk, run)` in both directions.
#[test]
fn a_walking_remote_backpedals_at_walk_speed_not_run_back() {
    let s = speeds();
    let (pos, _o, _vz, speed) =
        motion(move_flags::FORWARD | move_flags::WALK_MODE, 0.0).advance(s, 1.0);
    assert!(
        (pos[0] - 2.5).abs() < 1e-3,
        "a walk advances +X by 2.5: {pos:?}"
    );
    assert_eq!(speed, 2.5);

    let (pos, _o, _vz, speed) =
        motion(move_flags::BACKWARD | move_flags::WALK_MODE, 0.0).advance(s, 1.0);
    assert!(
        (pos[0] + 2.5).abs() < 1e-3,
        "…and a WALKING backpedal is still 2.5, not run_back's 4.5: {pos:?}"
    );
    assert_eq!(speed, 2.5);
    // Without the walk bit, the run-back min.
    let (_pos, _o, _vz, speed) = motion(move_flags::BACKWARD, 0.0).advance(s, 1.0);
    assert_eq!(speed, 4.5);
    // Swimming pre-empts the walk bit.
    let (_pos, _o, _vz, speed) = motion(
        move_flags::FORWARD | move_flags::SWIMMING | move_flags::WALK_MODE,
        0.0,
    )
    .advance(s, 1.0);
    assert_eq!(speed, 4.0, "a walker who wades strokes at full swim speed");
}

#[test]
fn remote_motion_swim_backpedal_takes_min_of_the_swim_pair() {
    // `0x7c4c90`'s backward arms: `min(back, forward)` for both pairs.
    let mut s = speeds();
    s.swim_back = 2.5;
    let (pos, _o, _vz, speed) =
        motion(move_flags::SWIMMING | move_flags::BACKWARD, 0.0).advance(s, 1.0);
    assert!(
        (pos[0] + 2.5).abs() < 1e-3,
        "swim backpedal advances −X by swim_back: {pos:?}"
    );
    assert_eq!(speed, 2.5);
    s.swim_back = 9.0; // above forward swim (4.0)
    let (_pos, _o, _vz, speed) =
        motion(move_flags::SWIMMING | move_flags::BACKWARD, 0.0).advance(s, 1.0);
    assert_eq!(
        speed, 4.0,
        "swimBack above swim clamps to swim (the min law)"
    );
}

#[test]
fn remote_motion_strafe_left_moves_90deg_left() {
    // Facing +X (north), strafe-left is +Y (west), at run speed.
    let (pos, o, _vz, _s) = motion(move_flags::STRAFE_LEFT, 0.0).advance(speeds(), 1.0);
    assert!(
        (pos[1] - 7.0).abs() < 1e-3,
        "strafe-left advances +Y: {pos:?}"
    );
    assert!(pos[0].abs() < 1e-3, "no forward drift: {pos:?}");
    assert_eq!(o, 0.0, "strafe doesn't turn the facing");
}

#[test]
fn remote_motion_turn_in_place_rotates_facing_only() {
    let (pos, o, _vz, speed) = motion(move_flags::TURN_LEFT, 0.0).advance(speeds(), 0.5);
    assert!(
        (o - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
        "turn-left raises facing by turn_rate·dt: {o}"
    );
    assert_eq!(
        pos,
        [0.0, 0.0, 0.0],
        "no translation while turning in place"
    );
    assert_eq!(speed, 0.0);
}

#[test]
fn remote_motion_stationary_when_no_move_flags() {
    let (pos, o, _vz, speed) = motion(0, 1.0).advance(speeds(), 1.0);
    assert_eq!(pos, [0.0, 0.0, 0.0]);
    assert_eq!(o, 1.0);
    assert_eq!(speed, 0.0);
}

#[test]
fn jump_seed_derives_velocity_and_clamps() {
    // The wire zspeed is down-positive: the 1.12 client sends -7.955547 for a rising jump.
    let j = JumpInfo {
        zspeed: -7.955_547,
        cos_angle: 1.0,
        sin_angle: 0.0,
        xy_speed: 7.0,
    };
    let (vz, xy) = jump_seed(Some(j), 0, false);
    assert!(
        (vz - 7.955_547).abs() < 1e-3,
        "take-off up-speed = -zspeed (positive, rising): {vz}"
    );
    assert!(
        (xy[0] - 7.0).abs() < 1e-3 && xy[1].abs() < 1e-3,
        "horizontal +X: {xy:?}"
    );
    // 1 s in: -zspeed - g * t.
    let (vz1, _) = jump_seed(Some(j), 1000, false);
    assert!(
        (vz1 - (7.955_547 - GRAVITY)).abs() < 1e-3,
        "vertical decays by gravity: {vz1}"
    );
    let (vzt, _) = jump_seed(Some(j), 10_000, false);
    assert!(
        (vzt + TERMINAL_VELOCITY).abs() < 1e-3,
        "clamped to −terminal: {vzt}"
    );
    assert_eq!(jump_seed(None, 0, false), (0.0, [0.0, 0.0]));
}

#[test]
fn remote_motion_jump_is_a_parabola_not_flag_walking() {
    // FORWARD is set, but the horizontal is the frozen 7 yd/s launch, not run speed.
    let mut rm = motion(move_flags::FALLING | move_flags::FORWARD, 0.0);
    rm.vertical_velocity = 7.955_547;
    rm.jump_xy_vel = [7.0, 0.0];
    let (pos, o, vz, speed) = rm.advance(speeds(), 0.5);
    assert!(
        (pos[0] - 3.5).abs() < 1e-3,
        "horizontal coasts at the frozen 7 yd/s: {pos:?}"
    );
    assert!(pos[1].abs() < 1e-3, "no lateral drift: {pos:?}");
    // The closed form `v0 * t - g * t^2 / 2`, exact at any `dt`.
    let analytic = 7.955_547 * 0.5 - 0.5 * GRAVITY * 0.5 * 0.5;
    assert!(
        (pos[2] - analytic).abs() < 1e-3,
        "height follows the closed form {analytic}: {pos:?}"
    );
    assert!(
        (vz - (7.955_547 - GRAVITY * 0.5)).abs() < 1e-3,
        "vertical speed decays by gravity: {vz}"
    );
    assert_eq!(o, 0.0, "no in-air turn");
    assert!(
        (speed - 7.0).abs() < 1e-3,
        "anim speed is the frozen horizontal: {speed}"
    );
}

#[test]
fn spline_interpolates_constant_speed_and_faces_travel() {
    // Two 10 yd legs over 4 s: 2 s each at constant speed.
    let start = Instant::now();
    let s = Spline {
        deck: None,
        points: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]],
        start,
        duration: Duration::from_secs(4),
        id: 0,
        grounded: true,
        run_mode: true,
    };
    let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 0.05);

    let (p0, f0, pitch0) = s.sample(start);
    assert!(
        close(p0, [0.0, 0.0, 0.0]),
        "start at first point, got {p0:?}"
    );
    assert!(f0.unwrap().abs() < 1e-3, "faces +X, got {f0:?}");
    assert_eq!(pitch0, 0.0, "a level segment has no travel pitch");

    let (p1, _, _) = s.sample(start + Duration::from_secs(1));
    assert!(close(p1, [5.0, 0.0, 0.0]), "mid leg 1, got {p1:?}");

    let (p3, f3, _) = s.sample(start + Duration::from_secs(3));
    assert!(close(p3, [10.0, 5.0, 0.0]), "mid leg 2, got {p3:?}");
    assert!(
        (f3.unwrap() - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
        "faces +Y, got {f3:?}"
    );

    let (pe, _, _) = s.sample(start + Duration::from_secs(10));
    assert!(
        close(pe, [10.0, 10.0, 0.0]),
        "clamps to last point, got {pe:?}"
    );
}

#[test]
fn spline_travel_pitch_is_the_segment_climb_angle() {
    // A 45 degree climb reports pitch `asin(dir.z)` = pi/4, up positive.
    let start = Instant::now();
    let s = Spline {
        deck: None,
        points: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 10.0]],
        start,
        duration: Duration::from_secs(4),
        id: 0,
        grounded: true,
        run_mode: true,
    };
    let (_, f, pitch) = s.sample(start + Duration::from_secs(1));
    assert!(f.unwrap().abs() < 1e-3, "facing is the horizontal heading");
    assert!(
        (pitch - std::f32::consts::FRAC_PI_4).abs() < 1e-3,
        "climb pitch is +π/4, got {pitch}"
    );
}

#[test]
fn monster_move_carries_every_waypoint() {
    let path = vec![
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 10.0, 0.0],
        [10.0, 10.0, 5.0],
    ];
    let s = monster_move_spline(path.clone(), 42, false, 2000, false, true, None)
        .expect("a moving monster-move yields a spline");
    assert_eq!(
        s.points, path,
        "every waypoint survives, not just the endpoint"
    );
    assert_eq!(
        s.id, 42,
        "the spline id rides through (for the SPLINE_DONE ack)"
    );
    assert_eq!(s.duration, Duration::from_millis(2000));
    assert!(
        s.grounded,
        "a non-flying spline is a ground walk (terrain-clamped)"
    );
}

/// `MSG_MOVE_TIME_SKIPPED` is `[CMovement+0xac] += lag` (`0x603b40` to `0x61ab90`): the next
/// packet schedules as if nothing were skipped.
#[test]
fn a_reported_skip_keeps_the_relay_chain_level_with_the_sender() {
    let step = |chain: &mut RelayChain, wire_ms: u32, now_ms: f64| {
        chain.schedule(wire_ms, now_ms, 0, true)
    };
    // Two chains, one told about the sender's 300 ms skip.
    let (mut told, mut untold) = (RelayChain::default(), RelayChain::default());
    step(&mut told, 10_000, 0.0);
    step(&mut untold, 10_000, 0.0);

    told.skip_time(300);

    // 100 ms later on our clock, but the stamp carries the skip too: a wire step of 400.
    let a = step(&mut told, 10_400, 100.0);
    let b = step(&mut untold, 10_400, 100.0);
    assert!(
        a < b,
        "the informed chain schedules earlier: it already knew 300 of those 400 ms were skipped, \
         not travelled (informed {a}, uninformed {b})"
    );
    assert!(
        (b - a - 300.0).abs() < 1e-6,
        "and the difference is exactly the skip it was told about: {} vs 300",
        b - a
    );
}

#[test]
fn a_skip_before_the_first_packet_changes_nothing() {
    let mut seeded = RelayChain::default();
    let mut cold = RelayChain::default();
    cold.skip_time(5_000);
    assert_eq!(
        seeded.schedule(10_000, 0.0, 0, true),
        cold.schedule(10_000, 0.0, 0, true),
        "an unseeded chain has no clock to move"
    );
}

#[test]
fn monster_move_flying_spline_is_not_grounded() {
    let s = monster_move_spline(
        vec![[0.0, 0.0, 0.0], [10.0, 0.0, 50.0]],
        0,
        false,
        2000,
        true,
        true,
        None,
    )
    .expect("a flying monster-move still yields a spline");
    assert!(
        !s.grounded,
        "a flying spline keeps its own Z, never terrain-clamped"
    );
}

#[test]
fn monster_move_stop_clears_the_spline() {
    assert!(
        monster_move_spline(
            vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            0,
            true,
            2000,
            false,
            true,
            None
        )
        .is_none(),
        "a Stop move snaps and clears, never builds a path"
    );
}

#[test]
fn monster_move_zero_duration_clears_the_spline() {
    assert!(
        monster_move_spline(
            vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            0,
            false,
            0,
            false,
            true,
            None
        )
        .is_none(),
        "a zero-duration move would divide by ~0 when sampled; treat as stationary"
    );
}

#[test]
fn monster_move_without_a_travelable_path_clears_the_spline() {
    assert!(
        monster_move_spline(vec![[1.0, 2.0, 3.0]], 0, false, 2000, false, true, None).is_none(),
        "a single point is nowhere to travel — no spline"
    );
}

/// Any `0x20ff` bit makes the chain treat a mover as mid-motion.
const MOVING: u32 = move_flags::FORWARD;

/// `0x618c30`: stamps 500 ms apart arriving late and in a burst still fire 500 ms apart.
#[test]
fn relay_chain_replays_on_the_senders_cadence() {
    let mut chain = RelayChain::default();
    let script = [(1000, 0.0), (1500, 520.0), (2000, 1450.0), (2500, 1460.0)];
    let fires: Vec<f64> = script
        .iter()
        .map(|&(wire, now)| chain.schedule(wire, now, MOVING, true))
        .collect();
    assert_eq!(
        fires,
        vec![0.0, 500.0, 1000.0, 1500.0],
        "the burst is de-clumped onto the sender's cadence"
    );
}

/// The first packet fires at arrival, a stale stamp adds no step (`@0x618cb8`), and the `u32`
/// clock's wrap is a forward step.
#[test]
fn relay_chain_seeds_holds_stale_stamps_and_survives_the_clock_wrap() {
    let mut chain = RelayChain::default();
    assert_eq!(
        chain.schedule(7_000, 4_000.0, MOVING, true),
        4_000.0,
        "first packet: fire at arrival, whatever the server's clock reads"
    );
    // Re-sent and out-of-order stamps; the next step measures from the last that counted.
    assert_eq!(chain.schedule(7_000, 4_100.0, MOVING, true), 4_000.0);
    assert_eq!(chain.schedule(6_900, 4_200.0, MOVING, true), 4_000.0);
    assert_eq!(
        chain.schedule(7_300, 4_300.0, MOVING, true),
        4_300.0,
        "the next forward stamp steps 300 ms from the last one that counted"
    );
    // 150 is 251 ms after u32::MAX - 100.
    let mut chain = RelayChain::default();
    chain.schedule(u32::MAX - 100, 0.0, MOVING, true);
    assert_eq!(
        chain.schedule(150, 10.0, MOVING, true),
        251.0,
        "the u32 wrap is a forward step, not a 49-day jump backwards"
    );
}

/// What the reference's chain does with a server-authored self move, which benilla applies at
/// once: the first fires at arrival, a later early one is held to the cadence. The stamps are
/// server time (vmangos `MovementInfo.h:208`).
#[test]
fn the_chain_paces_a_server_authored_self_move_and_would_hold_an_early_one() {
    let mut chain = RelayChain::default();
    assert_eq!(
        chain.schedule(1_000, 0.0, 0, true),
        0.0,
        "the first server-authored move seeds the chain and fires at arrival"
    );
    // 30 s later, 140 ms late: fires at arrival.
    assert_eq!(chain.schedule(31_000, 30_140.0, 0, true), 30_140.0);
    // 30 s later again, 90 ms early: held 90 ms.
    assert_eq!(
        chain.schedule(61_000, 60_050.0, 0, true),
        60_140.0,
        "an early arrival is held to the sender's cadence — the pacing benilla's self arm skips"
    );
}

/// Only a standing mover with an empty queue re-bases (`@0x618ce4`, `@0x618cf3`), by the window's
/// worst lateness (`0x618b50`), and an absorbed spike is not charged again.
#[test]
fn relay_chain_rebases_the_buffer_only_when_idle_and_unqueued() {
    // A packet 200 ms late while moving.
    let mut chain = RelayChain::default();
    chain.schedule(0, 0.0, MOVING, true);
    assert_eq!(chain.schedule(500, 700.0, MOVING, true), 500.0);
    let mut moving = chain.clone();
    assert_eq!(
        moving.schedule(1000, 1000.0, MOVING, true),
        1000.0,
        "mid-motion: the 200 ms spike buys no buffer"
    );
    let mut idle = chain.clone();
    assert_eq!(
        idle.schedule(1000, 1000.0, 0, true),
        1200.0,
        "idle + empty: re-based by the window max (200 ms late)"
    );
    assert_eq!(
        idle.schedule(1500, 1500.0, 0, true),
        1700.0,
        "the absorbed spike is not re-charged: still 200 ms of lead"
    );
    let mut queued = chain.clone();
    assert_eq!(
        queued.schedule(1000, 1000.0, 0, false),
        1000.0,
        "a non-empty queue defers the re-base"
    );
}

/// The skew clamp (`@0x618d0d`, `@0x618d49`): a fire lands within 500 ms before and 1000 ms after
/// arrival.
#[test]
fn relay_chain_holds_the_offset_inside_the_reference_clamp() {
    // A 2 s wire step 100 ms after the last fire would be 1.9 s out.
    let mut chain = RelayChain::default();
    chain.schedule(0, 0.0, MOVING, true);
    assert_eq!(chain.schedule(2000, 100.0, MOVING, true), 1100.0);
    // A stalled sender 4 s after the last fire would be 4 s in the past.
    let mut chain = RelayChain::default();
    chain.schedule(0, 1000.0, MOVING, true);
    assert_eq!(chain.schedule(0, 5000.0, MOVING, true), 4500.0);
}

/// Fire-times never go backwards, which lets the queue be a plain FIFO.
#[test]
fn relay_chain_stays_monotone_and_bounded_under_scripted_jitter() {
    // (wire stamp, arrival, moving): 500 ms stamps, jittered arrivals.
    let script: [(u32, f64, bool); 14] = [
        (500, 100.0, true),    // first packet
        (1000, 600.0, true),   // steady
        (1500, 1100.0, true),  // steady
        (2000, 2400.0, true),  // a 1.3 s stall
        (2500, 2410.0, true),  // burst catch-up
        (3000, 2420.0, true),  // burst catch-up
        (3500, 2430.0, true),  // burst catch-up
        (4000, 4000.0, false), // stopped, queue drained: resync
        (4500, 4500.0, false), // idle heartbeats
        (5000, 5000.0, false),
        (5500, 5480.0, true), // moving again, slightly early
        (6000, 6050.0, true), // slightly late
        (6500, 6500.0, true),
        (7000, 7100.0, true),
    ];
    let mut chain = RelayChain::default();
    let mut prev_fire = f64::NEG_INFINITY;
    for (wire, now, moving) in script {
        let fire = chain.schedule(wire, now, if moving { MOVING } else { 0 }, true);
        assert!(
            fire >= prev_fire,
            "fire-times must never reorder: {fire} after {prev_fire}"
        );
        let lead = fire - now;
        assert!(
            (-500.0..=1000.0).contains(&lead),
            "lead {lead} outside the reference clamp at wire={wire}"
        );
        prev_fire = fire;
    }
}

/// The pre-fire reconcile (`0x619090`, `0x6191c0`): Z joins the arm test only while swimming.
#[test]
fn reconcile_lerp_lands_on_the_event_at_its_fire_time() {
    let target = [10.0, 0.0, 0.0];
    // Five 100 ms frames toward a fire 500 ms out.
    let mut pos = [0.0, 0.0, 0.0];
    for i in 1..=5 {
        let remaining_after = 0.5 - 0.1 * i as f32;
        pos = reconcile_lerp(pos, pos, target, false, 0.1, remaining_after);
    }
    assert!((pos[0] - 10.0).abs() < 1e-4, "landed on the event: {pos:?}");
    // Within the 0.0278 yd tolerance.
    let held = reconcile_lerp(
        [5.0, 5.0, 0.0],
        [10.0, 0.01, 0.0],
        [10.0, 0.0, 0.0],
        false,
        0.1,
        0.4,
    );
    assert_eq!(held, [5.0, 5.0, 0.0], "sub-tolerance miss arms nothing");
    let dry = reconcile_lerp(
        [0.0; 3],
        [10.0, 0.0, 1.0],
        [10.0, 0.0, 0.0],
        false,
        0.1,
        0.4,
    );
    assert_eq!(dry, [0.0; 3], "grounded: Z ignored by the arm test");
    let wet = reconcile_lerp([0.0; 3], [10.0, 0.0, 1.0], [10.0, 0.0, 0.0], true, 0.1, 0.4);
    assert_ne!(wet, [0.0; 3], "swimming: Z arms the correction");
}

/// The pre-fire facing interp (`0x618f80`, `0x7c4f30`) takes the short way round.
#[test]
fn facing_lerp_turns_the_short_way_and_lands_at_fire_time() {
    use std::f32::consts::TAU;
    // Five 100 ms frames toward a fire 500 ms out.
    let mut o = 0.0f32;
    for i in 1..=5 {
        let remaining_after = 0.5 - 0.1 * i as f32;
        o = facing_lerp(o, 1.5, 0.1, remaining_after);
    }
    assert!((o - 1.5).abs() < 1e-4, "landed on the event facing: {o}");
    // From 0.1 toward 6.2, about -0.083 the short way, through 0.
    let stepped = facing_lerp(0.1, 6.2, 0.1, 0.4);
    assert!(
        stepped < 0.1 && stepped > 6.2 - TAU,
        "short way around: {stepped}"
    );
    let held = facing_lerp(1.0, 1.0 + 1.0e-8, 0.1, 0.4);
    assert_eq!(held, 1.0, "dead-zone: negligible turn skipped");
}

/// The frame loop as [`crate::net`] chains it, arrival before drain on one clock; returns the
/// applied order by `fall_time` and the final flags.
fn replay_frames(script: &[(u32, f64, u32)]) -> (Vec<u32>, u32) {
    use super::relay::{PendingMove, RelayMove};
    let mut rm = motion(MOVING, 0.0);
    let mut applied = Vec::new();
    let apply = |rm: &mut RemoteMotion, mv: &RelayMove, applied: &mut Vec<u32>| {
        rm.flags = mv.flags; // the one part of `apply_move` the ordering needs
        applied.push(mv.fall_time);
    };
    for (id, &(wire_ms, arrival_ms, flags)) in script.iter().enumerate() {
        let now = arrival_ms;
        let mv = RelayMove {
            wire_ms,
            position: [0.0; 3],
            orientation: 0.0,
            flags,
            pitch: 0.0,
            fall_time: id as u32, // the packet's identity
            jump: None,
            transport: None,
            verb: benilla_protocol::RelayVerb::Pose,
        };
        let (live, empty) = (rm.flags, rm.pending.is_empty());
        let fire_ms = rm.relay.schedule(wire_ms, now, live, empty);
        if rm.fires_at_arrival(fire_ms, now) {
            apply(&mut rm, &mv, &mut applied);
        } else {
            rm.pending.push_back(PendingMove { fire_ms, mv });
        }
        while rm.pending.front().is_some_and(|p| p.fire_ms <= now) {
            let ev = rm.pending.pop_front().expect("front checked");
            apply(&mut rm, &ev.mv, &mut applied);
        }
    }
    (applied, rm.flags)
}

#[test]
fn a_due_arrival_never_jumps_the_queue() {
    // A 60 Hz burst builds a one-deep queue, then the Stop lands at 96 ms with its fire-time (64)
    // already past while the FORWARD packet 3 is still queued.
    let script: [(u32, f64, u32); 5] = [
        (1000, 0.0, MOVING),  // 0: seeds, fires at arrival
        (1016, 0.0, MOVING),  // 1: fire 16, queued
        (1032, 16.0, MOVING), // 2: fire 32, queued; the drain releases 1
        (1048, 32.0, MOVING), // 3: fire 48, queued; the drain releases 2
        (1064, 96.0, 0),      // 4: the Stop, fire 64, due; 3 still queued
    ];
    let (order, flags) = replay_frames(&script);
    assert_eq!(
        order,
        vec![0, 1, 2, 3, 4],
        "relayed moves apply in arrival order — a due arrival waits behind the queue it belongs after"
    );
    assert_eq!(
        flags, 0,
        "the mover ends STOPPED: the Stop is the last write, not the FORWARD packet queued in front \
         of it — a still player then sends nothing, so a stale FORWARD here runs off forever"
    );
}

// ── GameObject placement: the `GAMEOBJECT_ROTATION` quaternion ────────────────

/// The seven `nightelfsignpostpointer02` arms of the Ravenwind post (Feralas) from vmangos'
/// `gameobject` table, `(entry, position, orientation, rotation0..3)`; six are pure yaw, and
/// 152580 authors a 70 degree tilt in `rotation0/1`.
const RAVENWIND_POST: [(u32, [f32; 3], f32, [f32; 4]); 7] = [
    (
        152574,
        [-4446.32, 2055.25, 46.2946],
        -1.20428,
        [0.0, 0.0, -0.566406, 0.824126],
    ),
    (
        152575,
        [-4446.34, 2055.23, 45.5724],
        -1.20428,
        [0.0, 0.0, -0.566406, 0.824126],
    ),
    (
        152576,
        [-4446.4, 2055.31, 46.2764],
        0.401426,
        [0.0, 0.0, 0.199368, 0.979925],
    ),
    (
        152577,
        [-4446.41, 2055.24, 46.2863],
        1.97222,
        [0.0, 0.0, 0.833886, 0.551937],
    ),
    (
        152578,
        [-4446.41, 2055.24, 45.6197],
        1.97222,
        [0.0, 0.0, 0.833886, 0.551937],
    ),
    (
        152579,
        [-4446.38, 2055.25, 44.954],
        1.97222,
        [0.0, 0.0, 0.833886, 0.551937],
    ),
    (
        152580,
        [-4445.28, 2058.18, 44.9976],
        -0.767946,
        [0.468413, 0.332221, -0.472813, 0.668331],
    ),
];

/// The plank's mid-point in model space (WoW axes), the centre of the model's collision hull
/// (`benilla-extract m2coll`: x +-0.197, y 0.338 to 2.075, z 3.062 to 3.624).
const PLANK_MID_MODEL: [f32; 3] = [0.0, 1.206, 3.343];

/// The post's axis, where the six untilted arms spawn.
const POST_AXIS: [f32; 2] = [-4446.4, 2055.25];

/// The plank's mid-point through a spawn transform, in WoW world coordinates.
fn plank_in_world(position: [f32; 3], rotation: Quat) -> [f32; 3] {
    let t = super::pose_transform(position, rotation);
    benilla_assets::coords::bevy_to_wow(
        t.transform_point(benilla_assets::coords::wow_to_bevy(PLANK_MID_MODEL)),
    )
}

/// A pure-yaw quaternion is `rot_z(facing)` and places exactly like the facing; 54 747 of the
/// 56 632 spawn rows are this case.
#[test]
fn a_pure_yaw_gameobject_quaternion_places_exactly_like_the_facing() {
    for orientation in [0.0_f32, 0.401426, -0.767946, -1.20428, 1.97222, 3.0, -2.7] {
        let (s, c) = (orientation * 0.5).sin_cos();
        let by_quat = super::gameobject_rotation(Some([0.0, 0.0, s, c]), orientation);
        let by_yaw = super::wire_yaw(orientation);
        assert!(
            by_quat.dot(by_yaw).abs() > 0.99999,
            "orientation {orientation}: quat {by_quat:?} vs yaw {by_yaw:?}"
        );
    }
    // The six untilted spawn rows at the Ravenwind post.
    for (entry, position, orientation, quat) in RAVENWIND_POST.iter().take(6) {
        let by_quat = plank_in_world(
            *position,
            super::gameobject_rotation(Some(*quat), *orientation),
        );
        let by_yaw = plank_in_world(*position, super::wire_yaw(*orientation));
        let moved = (0..3)
            .map(|i| (by_quat[i] - by_yaw[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        assert!(moved < 1e-3, "entry {entry} moved {moved} yd");
    }
}

/// Entry 152580 spawns 3 yd off the post; its 70 degree tilt carries the plank back onto it, and
/// facing-only placement leaves it 4.3 yd away in mid-air.
#[test]
fn the_tilted_ravenwind_pointer_lands_on_its_post() {
    let (_, position, orientation, quat) = RAVENWIND_POST[6];
    let by_quat = plank_in_world(
        position,
        super::gameobject_rotation(Some(quat), orientation),
    );
    let by_yaw = plank_in_world(position, super::wire_yaw(orientation));

    // Hand-derived from the spawn row and the model's hull.
    for (got, want) in by_quat.iter().zip([-4444.139_f32, 2055.174, 46.512]) {
        assert!((got - want).abs() < 0.01, "{by_quat:?} vs the golden");
    }
    let radius = |p: [f32; 3]| (p[0] - POST_AXIS[0]).hypot(p[1] - POST_AXIS[1]);
    assert!(
        radius(by_quat) < 2.5,
        "on the post: r = {}",
        radius(by_quat)
    );
    assert!(radius(by_yaw) > 4.0, "off the post: r = {}", radius(by_yaw));
    let apart = (0..3)
        .map(|i| (by_quat[i] - by_yaw[i]).powi(2))
        .sum::<f32>()
        .sqrt();
    assert!(
        (apart - 4.29).abs() < 0.05,
        "the two placements are {apart} yd apart"
    );
}

/// A create block folds absent fields to zero, so an all-zero quaternion means none was sent.
#[test]
fn a_gameobject_without_a_usable_quaternion_falls_back_to_its_facing() {
    for quat in [None, Some([0.0; 4])] {
        let r = super::gameobject_rotation(quat, 1.97222);
        assert!(
            r.dot(super::wire_yaw(1.97222)).abs() > 0.99999,
            "{quat:?} → {r:?}"
        );
    }
}

/// A mover with no `0x20ff` bit is not integrated (`0x616e20`, `0x6166f5`): its pose is the
/// last packet's.
#[test]
fn a_flag_still_remote_is_left_where_the_wire_put_it() {
    use avian3d::prelude::{Collider, RigidBody};
    use bevy::prelude::*;
    use bevy::transform::TransformPlugin;

    let mut app = App::new();
    // `WorldCollision` needs the trace exclusions the world plugins would initialise.
    app.init_resource::<benilla_world::collision::MoverTraceExclusions>();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        bevy::asset::AssetPlugin::default(),
        bevy::scene::ScenePlugin,
        avian3d::prelude::PhysicsPlugins::new(bevy::app::PostUpdate),
    ));
    app.init_asset::<Mesh>()
        .init_resource::<benilla_world::collision::ColliderEpoch>();
    // An empty liquid world: no water anywhere.
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.finish();
    app.cleanup();
    app.insert_resource(crate::player::PlayerCapsule(Collider::capsule(
        crate::player::CAPSULE_RADIUS,
        crate::player::CAPSULE_HEIGHT - 2.0 * crate::player::CAPSULE_RADIUS,
    )));
    app.add_systems(Update, super::remote::extrapolate_remote_units);
    // A 10x10 up-wound floor at z = 0.
    let verts = vec![
        Vec3::new(-5.0, 0.0, -5.0),
        Vec3::new(5.0, 0.0, -5.0),
        Vec3::new(5.0, 0.0, 5.0),
        Vec3::new(-5.0, 0.0, 5.0),
    ];
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::trimesh(verts, vec![[0u32, 2, 1], [0, 3, 2]]),
        Transform::default(),
    ));
    let idle = app
        .world_mut()
        .spawn((
            Transform::default(),
            motion(0, 0.0),
            // Real speeds, so the FORWARD leg below translates.
            crate::net::UnitSpeeds(speeds()),
        ))
        .id();
    // Standing where no collider exists: an unattached building floor, or a tile still streaming.
    let mut over_nothing = motion(0, 0.0);
    over_nothing.wow_pos = [50.0, 50.0, 10.0];
    let stranded = app
        .world_mut()
        .spawn((Transform::default(), over_nothing))
        .id();
    // A hair above the floor, as a wire Z arrives: the sender's resting clearance.
    let seated = [0.0, 0.0, 0.01];
    app.world_mut()
        .entity_mut(idle)
        .get_mut::<RemoteMotion>()
        .unwrap()
        .wow_pos = seated;

    for _ in 0..8 {
        app.update();
    }
    let rm = app.world().entity(idle).get::<RemoteMotion>().unwrap();
    assert_eq!(
        rm.wow_pos, seated,
        "flag-still ⇒ not integrated: the wire's Z stands, hair and all"
    );
    let rm = app.world().entity(stranded).get::<RemoteMotion>().unwrap();
    assert_eq!(
        rm.wow_pos,
        [50.0, 50.0, 10.0],
        "no ground under a standing mover is OUR world being incomplete, not a drop to take"
    );
    // A direction flag brings the resolve back; the floor 0.01 below is inside its reach.
    app.world_mut()
        .entity_mut(idle)
        .get_mut::<RemoteMotion>()
        .unwrap()
        .flags = move_flags::FORWARD;
    app.update();
    let rm = app.world().entity(idle).get::<RemoteMotion>().unwrap();
    assert!(
        rm.wow_pos[2] < seated[2],
        "a mover carrying a direction bit is integrated and resolved against the world"
    );
}

// ── The observer leg of the movement-mode family ──────────────────────────────

/// The `RelayMove` the six observer opcodes decode to: the server's flags word, which carries
/// apply and unapply for five of them, plus the verb.
fn observed(flags: u32, position: [f32; 3], verb: RelayVerb) -> super::relay::RelayMove {
    super::relay::RelayMove {
        wire_ms: 1000,
        position,
        orientation: 0.0,
        flags,
        pitch: 0.0,
        fall_time: 0,
        jump: None,
        transport: None,
        verb,
    }
}

/// Runs one relayed move through `apply_move` from `before`.
fn apply_observed(before: RemoteMotion, mv: &super::relay::RelayMove) -> RemoteMotion {
    use bevy::ecs::system::RunSystemOnce;
    let mv = mv.clone();
    let mut world = bevy::prelude::World::new();
    world.init_resource::<bevy::ecs::message::Messages<crate::creature_anim::HardLanding>>();
    let e = world.spawn_empty().id();
    world
        .run_system_once(
            move |mut commands: bevy::prelude::Commands,
                  mut landings: bevy::prelude::MessageWriter<crate::creature_anim::HardLanding>| {
                let mut rm = before.clone();
                super::remote::apply_move(e, &mv, &mut rm, 0.0, &mut commands, &mut landings);
                rm
            },
        )
        .expect("the one-shot apply runs")
}

/// The rooted client wipes its direction bits (`0x7c7340`, `& 0xffe07f00`) and acks the wiped
/// word, which vmangos stores (`MovementHandler.cpp:1067`) and relays as `MSG_MOVE_ROOT`.
#[test]
fn an_observed_root_lands_the_wiped_word_and_stops_the_dead_reckon() {
    let walking = motion(move_flags::FORWARD, 0.0);
    assert_ne!(
        walking.flags & move_flags::INTEGRATED,
        0,
        "precondition: this mover is being stepped"
    );

    // What vmangos broadcasts: ROOT set, the direction bits gone.
    let rooted = apply_observed(
        walking,
        &observed(move_flags::ROOT, [0.0; 3], RelayVerb::Root(true)),
    );

    assert_eq!(
        rooted.flags & move_flags::INTEGRATED,
        0,
        "nothing the integration gate tests survives the root — this is what stops the slide"
    );
    assert_ne!(
        rooted.flags & move_flags::ROOT,
        0,
        "and the bit itself lands"
    );
}

/// Levitate grants hover, feather fall and water walk together, each on its own observer opcode
/// carrying the whole flags word, so the last to arrive holds all three bits.
#[test]
fn an_observed_levitate_lands_all_three_granted_bits() {
    let trio = move_flags::HOVER | move_flags::SAFE_FALL | move_flags::WATER_WALKING;
    let floating = apply_observed(motion(0, 0.0), &observed(trio, [0.0; 3], RelayVerb::Pose));
    assert_eq!(
        floating.flags, trio,
        "hover + feather fall + water walk, from one broadcast word"
    );

    // The unapply is the same word without the bits.
    let landed = apply_observed(floating, &observed(0, [0.0; 3], RelayVerb::Pose));
    assert_eq!(landed.flags, 0, "the revoke is the absence of the bits");
}

/// Tag `0x26`, skipped by `0x619030` and `0x619090`, is the teleport's: `push 0x26` occurs only at
/// `0x6186bd` and `0x618736`, reached only from the teleport arms (`0x602fb0` sends
/// `MSG_MOVE_TELEPORT_ACK` on its other branch).
#[test]
fn the_teleport_is_the_only_relay_the_reconcile_skips() {
    let dest = [20.0, 5.0, 3.0];
    let tp = observed(move_flags::FORWARD, dest, RelayVerb::Teleport);

    assert!(!tp.reconciles(), "a blink is never blended toward");
    for armed in [
        RelayVerb::Heartbeat,
        RelayVerb::Pose,
        RelayVerb::Root(true),
        RelayVerb::Root(false),
    ] {
        assert!(
            observed(move_flags::FORWARD, dest, armed).reconciles(),
            "{armed:?} arms both blends — tag 0x26 is the teleport's alone"
        );
    }

    let there = apply_observed(motion(move_flags::FORWARD, 0.0), &tp);
    assert_eq!(
        there.wow_pos, dest,
        "the teleport pose lands outright: excluded from the blend, never from the apply"
    );
}

/// After the merge the client runs `SetRoot` (`0x7c7340`) unconditionally, so the opcode roots
/// the mover even when the word disagrees; vmangos always sends a wiped word, so no live run
/// would catch this.
#[test]
fn the_root_opcode_wins_over_a_word_that_disagrees_with_it() {
    // The opcode says root, the word says running forward.
    let liar = observed(move_flags::FORWARD, [0.0; 3], RelayVerb::Root(true));
    let rooted = apply_observed(motion(move_flags::FORWARD, 0.0), &liar);
    assert_ne!(rooted.flags & move_flags::ROOT, 0, "the opcode set the bit");
    assert_eq!(
        rooted.flags & move_flags::INTEGRATED,
        0,
        "and the one-shot wipe took the direction bits with it — this is what stops the slide"
    );

    // `ClearRoot` (`0x7c7370`) is `and ~0x1000`.
    let freed = apply_observed(rooted, &observed(0, [0.0; 3], RelayVerb::Root(false)));
    assert_eq!(freed.flags, 0, "cleared, and still not moving");
}
