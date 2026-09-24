//! The self-spline ride: when a server-authored spline drives our own player — an `SMSG_MONSTER_MOVE`
//! addressed to our guid — input yields, we ride it out, and we acknowledge `CMSG_MOVE_SPLINE_DONE`
//! so the server hands control back. Warrior **Charge** is the first case (decision 0260); knockback,
//! taxi flight, and fear reuse the same path.
//!
//! The mechanic, pinned live against vmangos: Charge is neither a teleport nor a knockback — the
//! server moves the *caster* with the same `MoveSpline` machinery as any creature (a ground spline at
//! run×4-capped-24 yd/s, facing the target), broadcasts it to the caster too, and for a player mover
//! waits on `CMSG_MOVE_SPLINE_DONE` (the `MovementInfo` at the endpoint + the `splineId`) before it
//! stops treating us as spline-controlled.
//!
//! Division of labour each frame while riding:
//! - [`crate::net`]'s `sample_splines` (Net stage) advances the [`Spline`] into the entity
//!   `Transform` — the horizontal authority for the ride. **Its Z is not authoritative** and never
//!   was (decision 1927): the reference discards a grounded spline's vertical exactly as it does a
//!   creature's, and [`ride_z`] re-derives it from the ground under us.
//! - [`drive_self_ride`] (Input stage, *before* `control`) owns the *whole* pose while the ride
//!   lasts: it mirrors that transform into [`Player`] (`pos`/`face_yaw`/`model_yaw`), drives a
//!   forward-run animation via [`MovementState`], unwinds the strafe counter-twist ([`BodyTwist`]),
//!   and — the frame the spline ends — emits the ack and clears the ride so `control` resumes from
//!   the endpoint at rest.
//! - `control`'s ride guard carries the follow-camera onto the moving avatar and skips input,
//!   physics, and the outbound movement stream.

use benilla_assets::coords::bevy_to_wow;
use bevy::prelude::*;

use crate::creature_anim::{move_flags, BodyTwist, MovementState};
use crate::net::{ClientCommand, Embodied, NetCommands, Spline, SplineStopped};

use super::Player;

/// The walkable surface under a ride's pose, measured the way every other mover's ground is —
/// a one-sided down-ray through the same window `net::motion::spline`'s clamp uses, from the
/// server's own Z rather than from any answer of ours. `None` = nothing in reach.
fn ride_ground(world: &benilla_world::collision::WorldCollision, pos: Vec3) -> Option<f32> {
    const UP: f32 = 2.5;
    const DOWN: f32 = 4.0;
    let origin = Vec3::new(pos.x, pos.y + UP, pos.z);
    world
        .ray_body(origin, Dir3::NEG_Y, UP + DOWN)
        .map(|h| origin.y - h.distance)
}

/// **The Z the body we are attached to rides at** — the ground under the spline's XZ, not the
/// spline's own vertical (decision 1927).
///
/// The reference's integrate loop makes no distinction between the body it steers and any other
/// mover here: `0x616de0`'s path-select produces a displacement from *either* the physics path or
/// the spline path and hands **both** to `0x616cb0`, which zeroes the vertical component and lets
/// `0x634040`'s swept resolve read Z off the surface. Keeping the chord instead is what B357 fixed
/// for a Playerbot; this is the same wire Z, on the one body that had it left.
///
/// **The keeps are the reference's own fork**, `0x616cec`-`0x616d03`:
/// - a **flying spline** (`MI.flags & 0x200`) — the taxi owns its altitude, and its sampled Z is
///   the flight path;
/// - **`SWIMMING`** (`[CMovement+0x40] & 0x200000`, one of the two bits at `0x616cfa`) — in liquid
///   the wire Z *is* the depth, the same exemption the creature clamp takes;
/// - a probe **miss** — nothing walkable in reach, so the body stays where the server put it.
///
/// The other bit at `0x616cfa` (`0x800`) has no expression here: our ride carries no live
/// CMovement flag word of its own, and a knockback — the airborne case that would want it — is not
/// a spline at all on this build (`SMSG_MOVE_KNOCK_BACK` is a ballistic launch through
/// [`super::mover`], decision 1702), so it never reaches this function.
///
/// The arithmetic itself is [`crate::net::grounded_y`], shared with the creature clamp so the two
/// cannot drift; hover and water-walking come from **our** state, not the granted-mode word — the
/// avatar's modes live on [`super::state::MoveModes`], and the wire family the clamp reads is inert
/// on us.
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

/// Extract the Bevy Y-yaw of a facing quaternion. The net bridge and `sample_splines` both write
/// the self entity's rotation as `Quat::from_rotation_y(facing)` (a pure Y turn), and benilla's
/// Bevy yaw equals the WoW orientation (decision 0002), so this recovers both the controller's
/// `face_yaw` and the wire orientation. Shared with the take-control edge (`wire_in`), which adopts
/// the streamed spawn pose's facing the same way.
pub(super) fn yaw_of(rotation: Quat) -> f32 {
    rotation.to_euler(EulerRot::YXZ).0
}

/// Mirror an in-progress self-spline into [`Player`], and ack it when it ends. Runs in
/// [`benilla_world::schedule::WorldStage::Input`] just before `control`, so the pose it publishes is what the
/// camera seats on and the animation reads this frame.
#[allow(clippy::type_complexity)] // a Bevy query's component tuple
pub(super) fn drive_self_ride(
    net: Res<NetCommands>,
    mut commands: Commands,
    mut player: ResMut<Player>,
    // The world under the ride: the surface it stands on ([`ride_z`]) and the liquid a
    // water-walker stands on instead.
    world: benilla_world::collision::WorldCollision,
    points: benilla_world::world_point::WorldPoint,
    mut q: Query<
        (
            Entity,
            // **Written, not just read**: the ride's Z is re-derived here ([`ride_z`]) and the
            // entity transform is where it has to land. `control`'s ride guard returns before
            // `body_pose::drive`, so nothing downstream would carry a correction made only to
            // `Player` — the camera and the wire would move and the rendered body would not.
            &mut Transform,
            Option<&Spline>,
            Option<&SplineStopped>,
            Option<&mut MovementState>,
            Option<&mut BodyTwist>,
        ),
        With<Embodied>,
    >,
) {
    // Only while we hold control (post-login). A free-fly detach (`F`) abandons any ride rather than
    // yanking the parked camera — rare, and the spline still finalizes server-side.
    if !player.active || player.detached {
        player.server_riding = false;
        return;
    }
    let Ok((entity, mut transform, spline, stopped, motion, twist)) = q.single_mut() else {
        return;
    };
    // A teleport landed since last frame: the server relocated us, voiding any in-progress ride
    // (the taxi flight-end teleport beats our own spline end by ~latency — vmangos's spline-done
    // handler ignores acks while its teleport is pending, so the relocation IS the hand-back; no
    // `CMSG_MOVE_SPLINE_DONE` is owed, and the teleport ack + position report already went out).
    // Mirroring the still-present spline this frame would clobber the snap back to the stale
    // flight pose — the 4-yd hover whose settle probe then missed the ground for the full 6 s
    // timeout at every taxi landing (decision 0501).
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
                // **A spline re-authors the walk gait, and its RUNMODE bit is inverted** (decision
                // 1758). The reference's `SMSG_MONSTER_MOVE` commit
                // `0x7c6a50` does this unconditionally for every incoming spline: `0x7c6ac2 and
                // edi,0x100` takes the path's own `SPLINEFLAG_RUNMODE` and `0x7c6acb` hands it to
                // `CMovement::SetRunMode 0x7c71c0`, whose argument is *run* — so a path the server
                // did NOT mark as a run leaves the body in walk mode when the ride ends, and a path
                // it did mark clears any walk the player had toggled. The two `0x100`s are inverses.
                //
                // On the START edge only, matching the byte site: the commit runs once per spline,
                // not per sampled frame. It writes the same latch the keybind does, so the wire
                // announces it through the ordinary flag differ on the frame the ride hands back.
                player.walking = !spline.run_mode;
                super::move_trace::gait("spline", player.walking, false, player.modes.rooted, true);
            }
            let yaw = yaw_of(transform.rotation);
            let wire_y = transform.translation.y;
            // One probe, one meaning: the floor found from the SERVER's own pose feeds both the
            // law and the line that reports it. Measuring again from the corrected pose would be a
            // different question, and would report the correction as if it were the ground.
            let floor = ride_ground(&world, transform.translation);
            let y = ride_z(
                &player,
                &points,
                // **A deck path re-derives its Z exactly like any other** (decision 1936's
                // correction). The tempting exemption — "there is no terrain under a boat" — is
                // refuted: the transport guid appears in neither `0x616cb0`'s predicate nor
                // `0x634040`'s dispatch, so a grounded deck spline takes the same fork, and the
                // probe does not miss because it runs at the **composed world position** against
                // a class mask that admits GameObject meshes. This system is in `WorldStage::Input`
                // and `transport::compose_riders` in the stage before it, so the translation read
                // here is already composed — nothing else is needed for the deck case.
                spline.grounded,
                transform.translation,
                floor,
            );
            if y != wire_y {
                transform.translation.y = y;
            }
            // Traced with the answer in hand, not the question: a line logged before the correction
            // measures the wire and reads identically whether the law above runs or not.
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
            // A forward run — the charge reads as a fast run (the gait selector keys on the FORWARD
            // flag + speed). It also gives a sane baseline for the resume and for observers.
            player.move_flags = move_flags::FORWARD;
            if let Some(mut motion) = motion {
                motion.speed = spline.speed();
                motion.vertical_speed = 0.0;
                motion.flags = move_flags::FORWARD;
                motion.stand_state = 0;
            }
            // The ride is a forward run, and the display-facing law's moving-forward case (the
            // `flags & 0x2003` snap — decisions 0101/0103) puts the body ON the aim: no gap, no
            // counter-twist — the same one-frame unwind as releasing a strafe key while running.
            // `control`, the normal gap owner, is parked behind the ride guard, so without this
            // write a charge engaged mid-strafe rode the whole spline with the spine/head frozen
            // ±90° off the run (the director's strafe-engage report).
            if let Some(mut twist) = twist {
                twist.yaw_gap = 0.0;
            }
        }
        // The ride just ended: `sample_splines` wrote the endpoint transform this frame and then
        // dropped the `Spline`. Sync to that exact endpoint, ack the server — it holds us as
        // spline-pending until this arrives, then relocates us and broadcasts a stop to observers —
        // and clear the ride so `control` resumes its own stream from rest.
        None if player.server_riding => {
            // The endpoint is grounded by the same law as every frame before it: this pose is what
            // `CMSG_MOVE_SPLINE_DONE` reports and what `control` resumes from, and a ride that ends
            // a hair off our terrain is exactly what decision 0501's settle-probe hunt was about.
            // `grounded` is read off the path that just finished — the last spline seen this ride.
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
            // "Resumes from the endpoint at rest" (decision 0260) — including the velocities. The
            // mover re-derives them only when it reads grounded; a ride ending a hair above our
            // terrain (navmesh Z vs ours) would otherwise inherit the pre-ride momentum — e.g. a
            // strafe-engaged charge sliding sideways out of its landing.
            player.vel_y = 0.0;
            player.horiz_vel = Vec3::ZERO;
            // **Whose id?** The one the server is actually waiting on. A ride that ran to its own
            // end leaves the path's id as the newest, but a ride the server *cut short* was cut by
            // launching a fresh stop spline, and vmangos checks the ack against that newest id
            // (`HandleMoveSplineDone`) — so an interrupted flee or charge acked with the path's id
            // is silently rejected, and every movement packet after it is dropped (decision 1281).
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
        // A stop with no ride behind it — the server halting a body it was already holding still
        // (the fear that ends between flee paths, the possession's opening `StopMoving`). It arms
        // the same wait and owes the same answer.
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

    /// …parameterised by the spline's `SPLINEFLAG_RUNMODE` and the walk gait the player had
    /// toggled before the ride took over.
    fn ride_app_with(
        run_mode: bool,
        walking: bool,
    ) -> (App, Entity, crossbeam_channel::Receiver<ClientCommand>) {
        let mut app = App::new();
        // The ride reads the world under it (the `rid` probe, and the ground the Z law stands on),
        // so the harness has to be a world — an empty one, which answers "no ground in reach" and
        // leaves every assertion below about the ride's own bookkeeping, as they were.
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

    /// **The ride's Z law** (decision 1927) — the local half of B357, measured live at +0.47 yd on
    /// a 16-yd Elwynn charge before it landed, and 0.000 after.
    ///
    /// The world is one flat floor and the ride is seated a chord's height above it, which is the
    /// whole of the defect: a two-point server path holds its endpoints' Z across everything
    /// between them, so on any concave ground the body flies. The reference does not: `0x616de0`
    /// hands the spline path's displacement to the same `0x616cb0` the physics path uses, which
    /// zeroes the vertical and lets the swept resolve read Z off the surface.
    mod ground {
        use super::*;
        use avian3d::prelude::{Collider, RigidBody};

        const FLOOR_Y: f32 = 7.5;
        /// Where the server's chord holds the body — a body-height over the floor, inside the
        /// probe's own reach so what is measured is the law and not the window.
        const CHORD_Y: f32 = 9.4;

        /// A 20×20 up-wound floor at [`FLOOR_Y`] — the one-sided down-ray stands only on those.
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

        /// Seat the body on the chord, with the collider trees built. The seating `update` runs a
        /// ride frame too — harmlessly, from `Transform::default()`, where the probe cannot reach a
        /// floor 7.5 yd overhead — so each test below sets its own condition and then takes
        /// **exactly one** frame, which is the frame it asserts about.
        fn seat(app: &mut App, entity: Entity) {
            app.update();
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<Transform>()
                .unwrap()
                .translation
                .y = CHORD_Y;
        }

        /// The ride app with ground under it (or deliberately without) and the body on the chord.
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

        /// **The report's shape, on our own body.** The chord is discarded; the ground is the Z —
        /// and `Player::pos` carries the same answer, because the camera and the outbound position
        /// report read it, not the transform.
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

        /// **A flying path owns its own altitude** — the reference keeps the vertical when the
        /// spline carries FLYING (`0x616cec` reads the MI flag word). A taxi must not be walked
        /// into the hillside it passes over.
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

        /// **In liquid the wire Z IS the depth** — the same exemption the creature clamp takes, and
        /// the reference's own: `[CMovement+0x40] & 0x200000` SWIMMING keeps the vertical at
        /// `0x616cfa`. A feared swimmer must not be dragged to the lakebed.
        #[test]
        fn a_swimming_ride_keeps_its_depth() {
            let (mut app, entity) = world(true);
            app.world_mut().resource_mut::<Player>().swimming = true;
            app.update();
            assert_eq!(y_of(&app, entity), CHORD_Y, "swimming keeps the wire Z");
        }

        /// **A probe miss leaves the body where the server put it** — nothing walkable in reach is
        /// a genuinely airborne pose or ground that has not streamed in, and an unclamped body
        /// belongs at its seat. Same answer the creature clamp gives.
        #[test]
        fn a_ride_with_no_ground_in_reach_keeps_the_servers_pose() {
            let (mut app, entity) = world(false);
            app.update();
            assert_eq!(y_of(&app, entity), CHORD_Y, "no floor, no correction");
        }

        /// **The endpoint is grounded too, and it is the pose the ack carries.** The frame a ride
        /// ends has already lost its `Spline` (`sample_splines` drops a finished path), so the law
        /// reads `Player::ride_grounded` there — without it the last frame of every charge would
        /// hand the server the chord's Z and resume the controller from mid-air.
        #[test]
        fn the_endpoint_the_ack_reports_is_grounded() {
            let (mut app, entity, rx) = ride_app();
            floor(&mut app);
            seat(&mut app, entity);
            app.update(); // one riding frame — arms `server_riding` + `ride_grounded`
            app.world_mut().entity_mut(entity).remove::<Spline>();
            // Put it back on the chord, as the finished path's last sample leaves it.
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

    /// **A spline re-authors the walk gait, inverted from its own RUNMODE bit** (decision 1758).
    /// The reference does this in the `SMSG_MONSTER_MOVE` commit `0x7c6a50`, unconditionally for
    /// every incoming spline: `0x7c6ac2 and edi,0x100` takes `SPLINEFLAG_RUNMODE` and `0x7c6acb`
    /// hands it to `SetRunMode 0x7c71c0`, whose argument is *run*. The two `0x100`s are inverses,
    /// which is the whole reason this is a test and not an obvious line of code — reading it the
    /// natural way round gets the behaviour exactly backwards in both directions.
    #[test]
    fn a_spline_re_authors_the_walk_gait_from_its_inverted_runmode_bit() {
        // A path the server did NOT mark as a run leaves the body walking — even though the
        // player was running when it took over.
        let (mut app, _e, _rx) = ride_app_with(false, false);
        app.update();
        assert!(
            app.world().resource::<Player>().walking,
            "a spline without RUNMODE forces walk mode ON"
        );
        // …and a run path clears a walk the player had toggled.
        let (mut app, _e, _rx) = ride_app_with(true, true);
        app.update();
        assert!(
            !app.world().resource::<Player>().walking,
            "a spline WITH RUNMODE clears the toggled walk"
        );
        // The start edge only: the commit runs once per spline, not once per sampled frame, so a
        // toggle taken mid-ride survives the rest of it.
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
        // The ride owns the pose: `control` (the normal gap owner) is parked behind the ride
        // guard, so the ride itself must zero the strafe counter-twist — frozen at ±90°, the
        // avatar charged the whole spline with its spine/head twisted off the run direction.
        let (mut app, entity, _rx) = ride_app();
        app.update();
        assert!(app.world().resource::<Player>().server_riding);
        let gap = app.world().get::<BodyTwist>(entity).unwrap().yaw_gap;
        assert_eq!(
            gap, 0.0,
            "riding forward, the body is on the aim — no counter-twist"
        );
    }

    /// The landing teleport voids the ride (decision 0501): the server relocates us at ITS
    /// flight end, before our own spline finishes — the mirror must not clobber the snap, the
    /// spline drops, and no `CMSG_MOVE_SPLINE_DONE` goes out (vmangos ignores it mid-teleport).
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
        app.update(); // and the ride-end arm must NOT fire afterwards (server_riding cleared)
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
            "the controller resumes from the endpoint at rest (decision 0260) — \
             stale pre-ride momentum must not leak into the resume"
        );
        match rx.try_recv() {
            Ok(ClientCommand::MoveSplineDone { spline_id, .. }) => assert_eq!(spline_id, 77),
            Ok(_) => panic!("expected the MoveSplineDone ack, got another command"),
            Err(_) => panic!("expected the MoveSplineDone ack, got nothing"),
        }
    }
}
