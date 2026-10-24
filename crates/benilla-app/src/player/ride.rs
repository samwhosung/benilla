//! Riding a transport (boat, zeppelin, tram). [`carry`] recomposes the rider from the deck's pose
//! before any input integrates; [`update_attachment`] decides after the mover whether we are still
//! aboard, re-snapshots the deck-local pose and writes the `ride` trace tag.

use bevy::prelude::*;

use super::{wrap_pi, FlyCam, Player, PlayerRide, TransportQuery};

/// Recomposes the rider from the deck's this-frame pose.
pub(super) fn carry(
    player: &mut Player,
    cam: &mut FlyCam,
    transports: &TransportQuery,
    index: &crate::net::GuidIndex,
) {
    // The transport tick runs on the Net→Input edge, so the deck pose is this frame's; a
    // streamed-out boat detaches into a fall. The yaw delta goes to aim, body and camera alike, on
    // top of this frame's mouse-look, as the reference's camera rides the transport-local rig: a
    // deck turn must reach neither the body chase, whose steps latch the turn-in-place shuffle,
    // nor `seat_camera`'s look-session gate.
    if let Some(ride) = player.ride.as_ref() {
        match transports.get(ride.entity) {
            Ok((boat, ..)) => {
                let world = boat.translation + boat.rotation * ride.local_pos;
                let yaw_now = boat.rotation.to_euler(EulerRot::YXZ).0;
                let mut dyaw = yaw_now - ride.boat_yaw;
                // `to_euler` wraps to (−π, π]; a boat crossing that seam reads as a ±2π hop.
                dyaw = (dyaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                player.carry_to(world);
                player.face_yaw += dyaw;
                player.model_yaw = wrap_pi(player.model_yaw + dyaw);
                cam.yaw += dyaw;
                if let Some(r) = player.ride.as_mut() {
                    r.boat_yaw = yaw_now;
                    r.placed = true;
                }
            }
            // Seated from our create on a transport whose path is not built yet: held where the
            // seat put us until it ticks. Its object gone, deactivation has taken us off
            // (`0x5f4fb0`).
            Err(_) if !ride.placed && held(ride, index) => {}
            Err(_) => player.ride = None,
        }
    }
}

impl Player {
    /// Moves the rider to its deck-carried `world` feet, and the arc's launch height with it: a
    /// rider's fall start `+0x7c` lives in the transport's frame (`0x7c6620` maps it at board and
    /// leave), where the fall height (`0x7c60c0`) and FALLINGFAR (`0x633240`) measure it. A deck
    /// pose is yaw-only, so the launch height moves by the feet's own vertical carry.
    pub(super) fn carry_to(&mut self, world: Vec3) {
        self.fall_start_y += world.y - self.pos.y;
        self.pos = world;
    }
}

/// Whether the object of the transport the ride names is still held: one deactivated takes its
/// passengers off, the active mover starting a fall (`0x5f4fb0` → `0x617ac0`, `0x617100(0, 0)`).
fn held(ride: &PlayerRide, index: &crate::net::GuidIndex) -> bool {
    index.0.get(&ride.guid) == Some(&ride.entity)
}

/// Whether the rider's feet lie inside the transport it rides (`0x630ba0`): the point in the
/// transport's frame, as the reference keeps the mover (`0x6173d5` passes `[mover+0x10]`), against
/// its [`crate::transport::TransportVolume`]. A transport held but not yet armed has no world
/// entity, which answers inside (`0x5f55c8`); one whose model has not loaded holds no one
/// (`0x670a70`), nor one whose object is gone.
fn aboard(player: &Player, transports: &TransportQuery, index: &crate::net::GuidIndex) -> bool {
    let Some(ride) = player.ride.as_ref() else {
        return false;
    };
    match transports.get(ride.entity) {
        Err(_) => held(ride, index),
        Ok((boat, _, _, Some(volume))) => {
            let local = boat.compute_affine().inverse().transform_point3(player.pos);
            volume.contains(benilla_assets::coords::bevy_to_wow(local))
        }
        Ok(_) => false,
    }
}

/// Attaches or detaches against this frame's walkable support, then re-snapshots the deck-local
/// pose. `ground` is the mover's resolved support ([`super::mover::Outcome::ground`]). A support
/// that is no transport detaches only once the feet leave the ridden transport's volume
/// (`0x6173b0`).
pub(super) fn update_attachment(
    player: &mut Player,
    transports: &TransportQuery,
    index: &crate::net::GuidIndex,
    child_of: &Query<&ChildOf>,
    ground: Option<Entity>,
    grounded: bool,
    swimming: bool,
) {
    // Attach on a transport's hull or a deck prop's collider child, found up the parent chain;
    // detach on world geometry or in water. Airborne keeps the attachment, so a jump above the
    // deck is deck-frame ballistics.
    let owning_transport = |mut e: Entity| {
        for _ in 0..4 {
            if let Ok((t, g, ..)) = transports.get(e) {
                return Some((e, t, g));
            }
            e = child_of.get(e).ok()?.parent();
        }
        None
    };
    if swimming {
        if player.ride.take().is_some() {
            info!("transport: deboard (entered the water)");
        }
    } else if grounded {
        match ground.and_then(owning_transport) {
            Some((entity, _, guid)) => {
                if player.ride.as_ref().map(|r| r.entity) != Some(entity) {
                    info!("transport: board {:#x} (support is its deck)", guid.0);
                }
                player.ride = Some(PlayerRide {
                    entity,
                    guid: guid.0,
                    local_pos: Vec3::ZERO, // filled by the snapshot just below
                    boat_yaw: 0.0,
                    placed: true,
                });
            }
            // 0x6173b0 with no transport for the support: the ride holds while `0x630ba0` finds
            // the feet, in the transport's frame, inside it.
            None if aboard(player, transports, index) => {}
            None => {
                if player.ride.take().is_some() {
                    info!("transport: deboard (support is world geometry)");
                }
            }
        }
    }
    let feet = player.pos;
    // The `ride` trace (`WOW_MOVE_TRACE_TAGS=ride`): the deck-relative path, which no other
    // instrument records, a line a frame while attached.
    if benilla_assets::trace::enabled_for("ride") {
        let boat_pose = player
            .ride
            .as_ref()
            .and_then(|r| transports.get(r.entity).ok())
            .map(|(t, _, aabb, _)| {
                (
                    t.translation,
                    t.rotation.to_euler(EulerRot::YXZ).0,
                    aabb.copied(),
                )
            });
        if let (Some(ride), Some((bpos, byaw, baabb))) = (player.ride.as_ref(), boat_pose) {
            let local = Quat::from_euler(EulerRot::YXZ, byaw, 0.0, 0.0)
                .inverse()
                .mul_vec3(feet - bpos);
            // The deck's broad-phase box, which the mover probes' candidate query tests, trails the
            // deck by a frame: avian refreshes it in `FixedPostUpdate`, before `tick_transports`
            // moves the deck in `Update`. A positive `gap` hides the deck from the down-probe.
            let aabb_col = match baabb {
                Some(a) => format!(
                    " | aabbY[{:8.2},{:8.2}] gap{:+7.2}",
                    a.min.y,
                    a.max.y,
                    a.min.y - feet.y,
                ),
                None => " | aabbY[  absent]".to_string(),
            };
            benilla_assets::trace::line(
                "ride",
                &format!(
                    "on {:#x} deck({:8.2},{:7.2},{:8.2}) yaw{:+.3} | feet({:8.2},{:7.2},{:8.2}) \
                     local({:7.2},{:6.2},{:7.2}) | grounded={} support={} vy={:+6.2}{}",
                    ride.guid,
                    bpos.x,
                    bpos.y,
                    bpos.z,
                    byaw,
                    feet.x,
                    feet.y,
                    feet.z,
                    local.x,
                    local.y,
                    local.z,
                    grounded as u8,
                    match ground.and_then(owning_transport) {
                        Some(_) => "deck",
                        None if ground.is_some() => "world",
                        None => "NONE",
                    },
                    player.vel_y,
                    aabb_col,
                ),
            );
        } else if !swimming {
            // Not riding: a line only when standing on a transport, a missed attach.
            if let Some((_, _, guid)) = ground.and_then(owning_transport) {
                benilla_assets::trace::line(
                    "ride",
                    &format!(
                        "OFF but standing on {:#x} at ({:8.2},{:7.2},{:8.2}) grounded={}",
                        guid.0, feet.x, feet.y, feet.z, grounded as u8
                    ),
                );
            }
        }
    }
    if let Some(ride) = player.ride.as_mut() {
        if let Ok((boat, ..)) = transports.get(ride.entity) {
            ride.local_pos = boat.compute_affine().inverse().transform_point3(feet);
            ride.boat_yaw = boat.rotation.to_euler(EulerRot::YXZ).0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::mover::fall_step;
    use super::super::{GRAVITY, JUMP_SPEED, TERMINAL_VELOCITY};
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// A rider on a deck at the origin facing +x, its volume a box 10 yd long, 4 wide and 6
    /// high, standing at `feet` (WoW axes, the deck's frame) on world geometry: whether the ride
    /// holds.
    fn holds_on_world_ground(
        feet: [f32; 3],
        volume: Option<crate::transport::TransportVolume>,
    ) -> bool {
        let mut world = World::new();
        let mut deck = world.spawn((
            Transform::default(),
            crate::net::Guid(0xF120_0000_0000_0001),
            crate::transport::Transport::parked_for_test(),
        ));
        if let Some(v) = volume {
            deck.insert(v);
        }
        let deck = deck.id();
        let ground = world.spawn_empty().id();
        let mut player = Player {
            pos: benilla_assets::coords::wow_to_bevy(feet),
            ride: Some(PlayerRide {
                entity: deck,
                guid: 0xF120_0000_0000_0001,
                local_pos: Vec3::ZERO,
                boat_yaw: 0.0,
                placed: true,
            }),
            ..Default::default()
        };
        let index = crate::net::GuidIndex([(0xF120_0000_0000_0001, deck)].into_iter().collect());
        world
            .run_system_once(
                move |transports: TransportQuery, child_of: Query<&ChildOf>| {
                    update_attachment(
                        &mut player,
                        &transports,
                        &index,
                        &child_of,
                        Some(ground),
                        true,
                        false,
                    );
                    player.ride.is_some()
                },
            )
            .expect("runs")
    }

    /// `0x6173b0`: landing on anything that is no transport keeps the ride while `0x630ba0` finds
    /// the feet inside the transport, and ends it once they leave.
    #[test]
    fn a_landing_inside_the_transport_keeps_the_ride() {
        let hull = || {
            Some(crate::transport::TransportVolume::Box(
                [-5.0, -2.0, 0.0],
                [5.0, 2.0, 6.0],
            ))
        };
        assert!(
            holds_on_world_ground([3.0, 1.0, 0.5], hull()),
            "inside the hull"
        );
        assert!(
            !holds_on_world_ground([3.0, 4.0, 0.5], hull()),
            "over the side"
        );
        assert!(
            !holds_on_world_ground([0.0, 0.0, 0.5], None),
            "a hull whose model has not loaded holds no one"
        );
    }

    /// A ride seated from our create on a transport that has not armed, its object held or gone:
    /// whether it outlives a landing on world geometry, and then a carry.
    fn unarmed_seat(held: bool) -> (bool, bool) {
        const DECK: u64 = 0xF120_0000_0000_0001;
        let mut world = World::new();
        let deck = world
            .spawn((Transform::default(), crate::net::Guid(DECK)))
            .id();
        let ground = world.spawn_empty().id();
        let seated = || Player {
            ride: Some(PlayerRide {
                entity: deck,
                guid: DECK,
                local_pos: Vec3::ZERO,
                boat_yaw: 0.0,
                placed: false,
            }),
            ..Default::default()
        };
        let index = crate::net::GuidIndex(held.then_some((DECK, deck)).into_iter().collect());
        let (mut landed, mut carried) = (seated(), seated());
        world
            .run_system_once(
                move |transports: TransportQuery, child_of: Query<&ChildOf>| {
                    update_attachment(
                        &mut landed,
                        &transports,
                        &index,
                        &child_of,
                        Some(ground),
                        true,
                        false,
                    );
                    let mut cam = FlyCam {
                        yaw: 0.0,
                        pitch: 0.0,
                        speed: 0.0,
                    };
                    carry(&mut carried, &mut cam, &transports, &index);
                    (landed.ride.is_some(), carried.ride.is_some())
                },
            )
            .expect("runs")
    }

    /// `0x5f55c8`: a transport with no world entity yet, its record not in, answers inside, so a
    /// ride seated from our create holds until it arms. One whose object is gone ends the seat, as
    /// deactivation takes each passenger off and starts its fall (`0x5f4fb0` → `0x617ac0`,
    /// `0x617100(0, 0)`).
    #[test]
    fn a_seat_holds_for_its_transport_until_the_transport_is_gone() {
        assert_eq!(unarmed_seat(true), (true, true), "no world entity yet");
        assert_eq!(unarmed_seat(false), (false, false), "gone");
    }

    /// Undercity's lift car 20652 descends 96.5 yd in 3.33 s (`TransportAnimation.dbc`).
    const LIFT_DROP_SPEED: f32 = 28.95;

    /// A 60 fps jump on a descending deck, carried every frame as `carry` does: the launch height
    /// rides the deck, so the landing reads the deck-frame drop of a flat jump.
    #[test]
    fn a_jump_on_a_descending_lift_lands_with_no_fall_height() {
        let dt = 1.0 / 60.0;
        let mut deck_y = 100.0;
        let mut p = Player {
            pos: Vec3::new(0.0, deck_y, 0.0),
            launch_vz: JUMP_SPEED,
            ..Default::default()
        };
        let (mut height, mut vz) = (0.0_f32, JUMP_SPEED);
        let (mut latched, mut frames) = (false, 0);
        let descent = loop {
            if frames > 0 {
                deck_y -= LIFT_DROP_SPEED * dt;
                p.carry_to(Vec3::new(0.0, deck_y + height, 0.0));
            }
            let launch_y = p.pos.y;
            let (v1, mean) = fall_step(vz, dt, TERMINAL_VELOCITY);
            vz = v1;
            height += mean * dt;
            let airborne = height > 0.0;
            if !airborne {
                height = 0.0;
            }
            p.pos.y = deck_y + height;
            let edges = p.advance_airborne_arc(airborne, frames == 0, frames as f32 * dt, launch_y);
            latched |= p.fall_far;
            frames += 1;
            if edges.landed {
                break p.fall_start_y - p.pos.y;
            }
            assert!(frames < 120, "the arc lands");
        };
        let air_time = frames as f32 * dt;
        assert!(
            (air_time - 2.0 * JUMP_SPEED / GRAVITY).abs() < 2.0 * dt,
            "a full flat arc, {air_time} s"
        );
        assert!(
            descent.abs() < 0.01,
            "a flat jump drops nothing in the deck's frame, not the deck's {descent} yd"
        );
        assert!(descent < crate::creature_anim::HARD_LANDING_DESCENT);
        assert!(!latched, "nor does it latch FALLINGFAR");
    }
}
