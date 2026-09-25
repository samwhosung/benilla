//! The self-spline ride: a server spline driving our own player (an `SMSG_MONSTER_MOVE` naming our
//! guid) owns the pose until it ends, then we ack `CMSG_MOVE_SPLINE_DONE` so the server hands
//! control back. Charge, taxi flights and fear take this path; a charge is a ground spline at run
//! speed × 4, capped at 24 yd/s (`PointMovementGenerator.cpp:278-281`). `sample_splines` advances
//! the [`Spline`] into the entity `Transform`, and [`drive_self_ride`] owns the rest of the pose.

use benilla_assets::coords::bevy_to_wow;
use bevy::prelude::*;

use crate::creature_anim::{move_flags, BodyTwist, MovementState};
use crate::net::{ClientCommand, Embodied, NetCommands, Spline, SplineStopped};

use super::Player;

/// The walkable surface under a ride's pose: a one-sided down-ray from the server's own Z, through
/// the same window as `net::motion::spline`'s clamp.
fn ride_ground(world: &benilla_world::collision::WorldCollision, pos: Vec3) -> Option<f32> {
    const UP: f32 = 2.5;
    const DOWN: f32 = 4.0;
    let origin = Vec3::new(pos.x, pos.y + UP, pos.z);
    world
        .ray_body(origin, Dir3::NEG_Y, UP + DOWN)
        .map(|h| origin.y - h.distance)
}

/// The Z the ride stands at: the ground under the spline's XZ, not its vertical. The reference
/// treats every mover alike: `0x616de0` hands the spline's displacement to `0x616cb0`, which zeroes
/// the vertical for `0x634040`'s swept resolve. It keeps the vertical (`0x616cec`-`0x616d03`) for
/// a flying spline (`MI.flags & 0x200`) and a swimmer (`[CMovement+0x40] & 0x200000`); the other
/// bit it tests at `0x616cfa`, `0x800`, has no counterpart, as a knockback is not a spline here.
/// Hover and water walking come from our [`super::state::MoveModes`], not the wire's mode word.
fn ride_z(
    player: &Player,
    points: &benilla_world::world_point::WorldPoint,
    grounded: bool,
    pos: Vec3,
    floor: Option<f32>,
) -> f32 {
    if !grounded || player.swimming {
        return pos.y;
    }
    let wow = bevy_to_wow(pos);
    let water = super::mover::water_floor(
        player.modes.water_walking,
        player.swimming,
        player.mover_pitch,
        points
            .liquid_at(benilla_world::world_point::Subject::Player, wow)
            .map(|l| pos.y + (l.surface_z - wow[2])),
    );
    crate::net::grounded_y(pos.y, floor, water, player.modes.hover)
}

/// The yaw of a pure-Y facing rotation, as the net bridge and `sample_splines` write it; Bevy yaw
/// equals the WoW orientation, so this is both `face_yaw` and the wire orientation.
pub(super) fn yaw_of(rotation: Quat) -> f32 {
    rotation.to_euler(EulerRot::YXZ).0
}

/// Mirror an in-progress self-spline into [`Player`] and ack it when it ends; runs just before
/// `control`, so the camera and the animation read this frame's pose.
#[allow(clippy::type_complexity)] // a Bevy query's component tuple
pub(super) fn drive_self_ride(
    net: Res<NetCommands>,
    mut commands: Commands,
    mut player: ResMut<Player>,
    // The ground and the liquid under the ride, for `ride_z`.
    world: benilla_world::collision::WorldCollision,
    points: benilla_world::world_point::WorldPoint,
    mut q: Query<
        (
            Entity,
            // Written: the corrected Z must land on the entity, as `control`'s ride guard returns
            // before `body_pose::drive` would carry it.
            &mut Transform,
            Option<&Spline>,
            Option<&SplineStopped>,
            Option<&mut MovementState>,
            Option<&mut BodyTwist>,
        ),
        With<Embodied>,
    >,
) {
    // Only while we hold control; a free-fly detach abandons the ride (the server still ends it).
    if !player.active || player.detached {
        player.server_riding = false;
        return;
    }
    let Ok((entity, mut transform, spline, stopped, motion, twist)) = q.single_mut() else {
        return;
    };
    // A teleport since last frame (a taxi landing beats our spline's end) voids the ride and owes
    // no ack: vmangos ignores a spline-done while teleporting (`MovementHandler.cpp:819-820`), and
    // mirroring the stale spline would undo the snap.
    if std::mem::take(&mut player.ride_abort) {
        if spline.is_some() {
            commands.entity(entity).remove::<Spline>();
        }
        if stopped.is_some() {
            commands.entity(entity).remove::<SplineStopped>();
        }
        if player.server_riding {
            player.server_riding = false;
            player.move_flags = 0;
            player.airborne_since = None;
            player.vel_y = 0.0;
            player.horiz_vel = Vec3::ZERO;
        }
        return;
    }
    match spline {
        // Riding: the freshly-sampled transform is our pose this frame.
        Some(spline) => {
            if !player.server_riding {
                info!(
                    "charge/ride: server spline {} drives the avatar ({} pts, {:.0} yd/s over {} ms)",
                    spline.id,
                    spline.points.len(),
                    spline.speed(),
                    spline.duration.as_millis(),
                );
                // Each spline sets the walk gait from its `SPLINEFLAG_RUNMODE`, inverted: the
                // reference passes the bit to `SetRunMode` (`0x7c6ac2` → `0x7c71c0`), whose
                // argument is run. Once per spline, into the keybind's latch.
                player.walking = !spline.run_mode;
                super::move_trace::gait("spline", player.walking, false, player.modes.rooted, true);
            }
            let yaw = yaw_of(transform.rotation);
            let wire_y = transform.translation.y;
            // One probe from the server's pose feeds both the Z and its trace line.
            let floor = ride_ground(&world, transform.translation);
            let y = ride_z(
                &player,
                &points,
                // A deck path grounds like any other (the transport guid is in neither
                // `0x616cb0`'s predicate nor `0x634040`'s dispatch); `transport::compose_riders`
                // has already composed the world pose, and the probe admits GameObject meshes.
                spline.grounded,
                transform.translation,
                floor,
            );
            if y != wire_y {
                transform.translation.y = y;
            }
            // Traced after the correction, so the line shows it.
            super::move_trace::ride(
                spline.id,
                spline.grounded,
                transform.translation,
                wire_y,
                floor,
            );
            player.pos = transform.translation;
            player.face_yaw = yaw;
            player.model_yaw = yaw;
            player.server_riding = true;
            player.ride_spline_id = spline.id;
            player.ride_grounded = spline.grounded;
            // A forward run: the gait selector keys on FORWARD and speed.
            player.move_flags = move_flags::FORWARD;
            if let Some(mut motion) = motion {
                motion.speed = spline.speed();
                motion.vertical_speed = 0.0;
                motion.flags = move_flags::FORWARD;
                motion.stand_state = 0;
            }
            // Moving forward puts the body on the aim (the `flags & 0x2003` snap), so no
            // counter-twist; `control`, the gap's normal owner, is parked behind the ride guard.
            if let Some(mut twist) = twist {
                twist.yaw_gap = 0.0;
            }
        }
        // The ride ended: `sample_splines` wrote the endpoint and dropped the `Spline`. On the ack,
        // vmangos relocates us and broadcasts a stop (`MovementHandler.cpp:830-841`).
        None if player.server_riding => {
            // The endpoint grounds by the same law: it is what the ack reports and `control`
            // resumes from. `ride_grounded` is the last spline's, as the `Spline` is gone.
            let floor = ride_ground(&world, transform.translation);
            let y = ride_z(
                &player,
                &points,
                player.ride_grounded,
                transform.translation,
                floor,
            );
            if y != transform.translation.y {
                transform.translation.y = y;
            }
            player.pos = transform.translation;
            player.face_yaw = yaw_of(transform.rotation);
            player.model_yaw = player.face_yaw;
            player.server_riding = false;
            player.move_flags = 0;
            player.airborne_since = None;
            // At rest, velocities too: the mover re-derives them only when grounded, so a ride
            // ending a hair above our terrain would inherit the pre-ride momentum.
            player.vel_y = 0.0;
            player.horiz_vel = Vec3::ZERO;
            // The newest spline id: a cut-short ride ended in a fresh stop spline, and vmangos
            // ignores an ack for an older one (`MovementHandler.cpp:807`), then drops our movement.
            let spline_id = stopped.map_or(player.ride_spline_id, |s| s.0);
            if stopped.is_some() {
                commands.entity(entity).remove::<SplineStopped>();
            }
            let _ = net.0.send(ClientCommand::MoveSplineDone {
                flags: 0,
                pos: bevy_to_wow(player.pos),
                orientation: player.face_yaw,
                spline_id,
            });
        }
        // A stop with no ride behind it (a fear ending between flee paths, a possession's opening
        // `StopMoving`) arms the same wait and owes the same ack.
        None if stopped.is_some() => {
            let id = stopped.expect("checked above").0;
            commands.entity(entity).remove::<SplineStopped>();
            let _ = net.0.send(ClientCommand::MoveSplineDone {
                flags: 0,
                pos: bevy_to_wow(transform.translation),
                orientation: yaw_of(transform.rotation),
                spline_id: id,
            });
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;
    use std::time::{Duration, Instant};

    use super::*;

    /// A one-system app riding a 2-point self-spline, `Player` mid-strafe as at charge engage.
    fn ride_app() -> (App, Entity, crossbeam_channel::Receiver<ClientCommand>) {
        ride_app_with(true, false)
    }

    fn ride_app_with(
        run_mode: bool,
        walking: bool,
    ) -> (App, Entity, crossbeam_channel::Receiver<ClientCommand>) {
        let mut app = App::new();
        // An empty world, whose probes answer "no ground in reach".
        app.init_resource::<benilla_world::collision::MoverTraceExclusions>();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            avian3d::prelude::PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>();
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        app.finish();
        app.cleanup();
        app.add_systems(Update, drive_self_ride);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.insert_resource(Player {
            active: true,
            // Stale pre-ride momentum: a strafe was held when the spline took over.
            horiz_vel: Vec3::new(5.0, 0.0, 0.0),
            vel_y: -2.0,
            walking,
            ..Default::default()
        });
        // Mid-strafe counter-twist: the aim sits 90° off the rendered root.
        let mut twist = BodyTwist::new(None, None);
        twist.yaw_gap = -FRAC_PI_2;
        let entity = app
            .world_mut()
            .spawn((
                Transform::default(),
                Spline {
                    deck: None,
                    points: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
                    start: Instant::now(),
                    duration: Duration::from_secs(600), // far from ending during the test
                    id: 77,
                    grounded: true,
                    run_mode,
                },
                twist,
                Embodied,
            ))
            .id();
        (app, entity, rx)
    }

    /// The ride's Z on one flat floor, with the body seated a chord's height above it.
    mod ground {
        use super::*;
        use avian3d::prelude::{Collider, RigidBody};

        const FLOOR_Y: f32 = 7.5;
        /// A body-height over the floor, inside the probe's reach.
        const CHORD_Y: f32 = 9.4;

        /// A 20×20 up-wound floor at [`FLOOR_Y`]; the one-sided down-ray hits only up-wound faces.
        fn floor(app: &mut App) {
            let v = vec![
                Vec3::new(-10.0, FLOOR_Y, -10.0),
                Vec3::new(10.0, FLOOR_Y, -10.0),
                Vec3::new(10.0, FLOOR_Y, 10.0),
                Vec3::new(-10.0, FLOOR_Y, 10.0),
            ];
            app.world_mut().spawn((
                RigidBody::Static,
                Collider::trimesh(v, vec![[0u32, 2, 1], [0, 3, 2]]),
                Transform::default(),
            ));
        }

        /// Seat the body on the chord, colliders built; each test then takes exactly one frame.
        fn seat(app: &mut App, entity: Entity) {
            app.update();
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<Transform>()
                .unwrap()
                .translation
                .y = CHORD_Y;
        }

        fn world(with_floor: bool) -> (App, Entity) {
            let (mut app, entity, _rx) = ride_app();
            if with_floor {
                floor(&mut app);
            }
            seat(&mut app, entity);
            (app, entity)
        }

        fn y_of(app: &App, e: Entity) -> f32 {
            app.world().get::<Transform>(e).unwrap().translation.y
        }

        #[track_caller]
        fn assert_grounded(app: &App, e: Entity, why: &str) {
            let y = y_of(app, e);
            assert!(
                (y - FLOOR_Y).abs() < 1e-3,
                "{why}: expected the floor {FLOOR_Y}, got {y}"
            );
        }

        #[test]
        fn a_grounded_ride_stands_on_the_world_not_the_chord() {
            let (mut app, entity) = world(true);
            app.update();
            assert_grounded(&app, entity, "a grounded path's Z is the terrain's");
            assert!(
                (app.world().resource::<Player>().pos.y - FLOOR_Y).abs() < 1e-3,
                "the pose the camera and the wire read is the same one"
            );
        }

        /// The reference keeps a flying spline's vertical (`0x616cec`).
        #[test]
        fn a_flying_ride_keeps_its_altitude() {
            let (mut app, entity) = world(true);
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<Spline>()
                .unwrap()
                .grounded = false;
            app.update();
            assert_eq!(y_of(&app, entity), CHORD_Y, "a flight is not grounded");
        }

        /// SWIMMING keeps the vertical (`0x616cfa`), so a feared swimmer stays off the lakebed.
        #[test]
        fn a_swimming_ride_keeps_its_depth() {
            let (mut app, entity) = world(true);
            app.world_mut().resource_mut::<Player>().swimming = true;
            app.update();
            assert_eq!(y_of(&app, entity), CHORD_Y, "swimming keeps the wire Z");
        }

        #[test]
        fn a_ride_with_no_ground_in_reach_keeps_the_servers_pose() {
            let (mut app, entity) = world(false);
            app.update();
            assert_eq!(y_of(&app, entity), CHORD_Y, "no floor, no correction");
        }

        /// The end frame has no `Spline`, so the law reads `Player::ride_grounded`.
        #[test]
        fn the_endpoint_the_ack_reports_is_grounded() {
            let (mut app, entity, rx) = ride_app();
            floor(&mut app);
            seat(&mut app, entity);
            app.update(); // one riding frame: arms `server_riding` and `ride_grounded`
            app.world_mut().entity_mut(entity).remove::<Spline>();
            // Back on the chord, as the finished path's last sample leaves it.
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<Transform>()
                .unwrap()
                .translation
                .y = CHORD_Y;
            app.update();

            assert_grounded(&app, entity, "the ride's last pose");
            let ack = rx
                .try_iter()
                .find_map(|c| match c {
                    ClientCommand::MoveSplineDone { pos, .. } => Some(pos),
                    _ => None,
                })
                .expect("the ride acks when it ends");
            assert!(
                (ack[2] - FLOOR_Y).abs() < 1e-3,
                "the ack reports where we actually are, got {ack:?}"
            );
        }
    }

    /// `SPLINEFLAG_RUNMODE` and `MOVEFLAG_WALK_MODE`, both `0x100`, are inverses (`0x7c6a50`).
    #[test]
    fn a_spline_re_authors_the_walk_gait_from_its_inverted_runmode_bit() {
        let (mut app, _e, _rx) = ride_app_with(false, false);
        app.update();
        assert!(
            app.world().resource::<Player>().walking,
            "a spline without RUNMODE forces walk mode ON"
        );
        let (mut app, _e, _rx) = ride_app_with(true, true);
        app.update();
        assert!(
            !app.world().resource::<Player>().walking,
            "a spline WITH RUNMODE clears the toggled walk"
        );
        // Once per spline: a toggle taken mid-ride survives.
        let (mut app, _e, _rx) = ride_app_with(true, true);
        app.update();
        app.world_mut().resource_mut::<Player>().walking = true;
        app.update();
        assert!(
            app.world().resource::<Player>().walking,
            "re-authored on the START edge, not every frame"
        );
    }

    #[test]
    fn engaging_a_ride_mid_strafe_unwinds_the_counter_twist() {
        let (mut app, entity, _rx) = ride_app();
        app.update();
        assert!(app.world().resource::<Player>().server_riding);
        let gap = app.world().get::<BodyTwist>(entity).unwrap().yaw_gap;
        assert_eq!(
            gap, 0.0,
            "riding forward, the body is on the aim — no counter-twist"
        );
    }

    #[test]
    fn a_teleport_aborts_the_ride_without_an_ack() {
        let (mut app, entity, rx) = ride_app();
        app.update(); // riding
        app.world_mut().resource_mut::<Player>().ride_abort = true;
        app.update(); // the abort frame: no mirror, spline dropped
        let player = app.world().resource::<Player>();
        assert!(!player.server_riding);
        assert!(
            app.world().get::<Spline>(entity).is_none(),
            "the spline is dropped with the ride"
        );
        app.update(); // the ride-end arm must not fire after it
        assert!(
            rx.try_recv().is_err(),
            "no MoveSplineDone — the teleport relocation superseded the ride"
        );
    }

    #[test]
    fn ride_end_acks_the_spline_and_resumes_at_rest() {
        let (mut app, entity, rx) = ride_app();
        app.update(); // riding
        app.world_mut().entity_mut(entity).remove::<Spline>();
        app.update(); // the ride-end edge
        let player = app.world().resource::<Player>();
        assert!(!player.server_riding);
        assert_eq!(
            (player.vel_y, player.horiz_vel),
            (0.0, Vec3::ZERO),
            "the controller resumes from the endpoint at rest — \
             stale pre-ride momentum must not leak into the resume"
        );
        match rx.try_recv() {
            Ok(ClientCommand::MoveSplineDone { spline_id, .. }) => assert_eq!(spline_id, 77),
            Ok(_) => panic!("expected the MoveSplineDone ack, got another command"),
            Err(_) => panic!("expected the MoveSplineDone ack, got nothing"),
        }
    }
}
