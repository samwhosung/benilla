//! Transports, whose cycle the client computes. The server never streams a transport's position,
//! only an anchor: the movement block's `UPDATE_FLAG_TRANSPORT` u32, its path-progress ms at
//! create, re-sent on every re-create.
//!
//! - Type 15 (`MO_TRANSPORT`, boats and zeppelins): a timetable from `TaxiPathNode.dbc` and the
//!   template's `(taxiPathId, moveSpeed, accelRate)` ([`benilla_formats::TransportTimetable`]),
//!   sampled at `(anchor + elapsed) % period`.
//! - Type 11 (`TRANSPORT`, elevators and trams): the `TransportAnimation.dbc` keyframe path keyed
//!   by template entry, at `spawn + R(spawn_quat)·lerp(keyframes, (anchor + elapsed) % period)`
//!   ([`benilla_formats::elevator_sample`], the reference's `0x5f6280`; vmangos does the same in
//!   `ElevatorTransport::Update`, `Transport.cpp:396`).
//!
//! This module only moves the streamed GameObject; riding keys on [`Transport`], not the drive.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

// avian's own AABB and collider-tree refresh, run a second time each frame in the chain below.
use avian3d::collider_tree::update_moved_collider_aabbs as republish_moved_collider_aabbs;
use avian3d::prelude::{Collider, Position, RigidBody, Rotation};
use benilla_formats::{
    elevator_period_ms, elevator_sample, load_elevator_paths, load_taxi_path_nodes,
    ElevatorKeyframe, ElevatorPaths, TaxiPathNodes, TransportSample, TransportTimetable,
};
use bevy::prelude::*;

use crate::go_templates::GameObjectTemplates;
use crate::net::Guid;
use benilla_assets::{LockRecover, WorldAssets};
use benilla_world::ride_frame::RideFrame;
use benilla_world::vis_chain::VisChainOnly;
use benilla_world::world_map::CurrentMap;

pub(crate) struct TransportPlugin;

impl Plugin for TransportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Timetables>()
            // After `AssetSet::Open`, whose Commands insert `WorldAssets`; unordered, this finds
            // none and silently does nothing.
            .add_systems(
                Startup,
                setup_taxi_nodes.after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                // After the net drain and before input, so the deck is posed before the mover
                // reads the world. avian refreshes collider AABBs in `FixedPostUpdate`, before
                // `Update`, so without [`republish_moved_collider_aabbs`] the mover's casts see the
                // deck's previous-frame box, above its rider after one long frame. It writes no
                // tick resource, so a second run is idempotent; it sweeps every collider.
                (
                    arm_transports,
                    reseek_ridden_transport_at_worldport,
                    tick_transports,
                    watch_ridden_transport_map,
                    republish_moved_collider_aabbs::<Collider>,
                    compose_riders,
                    ground_deck_riders,
                )
                    .chain()
                    .after(benilla_world::schedule::WorldStage::Net)
                    .before(benilla_world::schedule::WorldStage::Input),
            )
            // The ride frame last, after `PlayerControlSet` decides what we stand on; its
            // consumers, the particle and ribbon sims, run in `PostUpdate`.
            .add_systems(
                Update,
                stamp_ride_frames.after(crate::player::PlayerControlSet),
            );
    }
}

/// The create-block anchor: the server's path-progress ms and the local instant it arrived,
/// inserted on every create that carries `UPDATE_FLAG_TRANSPORT`.
#[derive(Component)]
pub(crate) struct TransportAnchor {
    pub(crate) progress_ms: u32,
    pub(crate) at: Instant,
}

/// A pending type-11 lift, captured off the create block: its keyframe path is keyed by the
/// template entry, which `OBJECT_FIELD_ENTRY` carries, so no template query is needed.
#[derive(Component)]
pub(crate) struct ElevatorSeed {
    /// `gameobject_template` entry, `TransportAnimation.dbc`'s `TransportID` key.
    pub(crate) entry: u32,
    /// Spawn position (raw WoW coords), from the movement block: a transport create carries no
    /// `GAMEOBJECT_POS_*`.
    pub(crate) base_pos: [f32; 3],
    /// Spawn facing (WoW radians), the car's constant heading.
    pub(crate) yaw: f32,
    /// `GAMEOBJECT_ROTATION`, the spawn quaternion the keyframe offsets rotate through; `None`
    /// falls back to the pure-yaw quat, which is what spawn rows encode.
    pub(crate) quat: Option<[f32; 4]>,
}

/// A ticking transport, armed by [`arm_transports`].
#[derive(Component)]
pub(crate) struct Transport {
    drive: Drive,
    /// Last frame's `moving`, for the dock and depart log lines.
    was_moving: bool,
}

/// The two cycle evaluators; consumers key on [`Transport`], never on the drive.
enum Drive {
    /// Type 15: the client-computed taxi-path timetable.
    Taxi(Arc<TransportTimetable>),
    /// Type 11: the authored `TransportAnimation` keyframe path.
    Lift(Lift),
}

/// No map field: vmangos sends a player the map's whole transport set on entry
/// (`Map::SendInitTransports`, `Map.cpp:1719`), so a lift is always on the viewer's map. At login
/// `CurrentMap` is still the startup seed when lifts arm, so a stamped map would hide them.
struct Lift {
    frames: Arc<Vec<ElevatorKeyframe>>,
    period_ms: u32,
    base_pos: [f32; 3],
    quat: [f32; 4],
    /// Constant rendered heading (wire orientation, WoW radians).
    yaw: f32,
}

impl Transport {
    /// Which drive is underneath, for the instruments only (`WOW_LIFT_CENSUS`).
    pub(crate) fn drive_label(&self) -> &'static str {
        match &self.drive {
            Drive::Taxi(_) => "taxi",
            Drive::Lift(_) => "lift",
        }
    }

    pub(crate) fn period_ms(&self) -> u32 {
        match &self.drive {
            Drive::Taxi(t) => t.period_ms,
            Drive::Lift(l) => l.period_ms,
        }
    }

    /// The live cycle position for an anchor, shared by the tick and the rider math.
    pub(crate) fn cycle_ms(&self, anchor: &TransportAnchor) -> u32 {
        let progress = u64::from(anchor.progress_ms) + anchor.at.elapsed().as_millis() as u64;
        (progress % u64::from(self.period_ms().max(1))) as u32
    }

    /// Re-anchor the clock to the head of `map_id`'s leg unless the cycle is already on that map;
    /// returns the sample and the cycle instant it moved to, if it moved (the only sound test, as
    /// `progress_ms` is uptime-scale until a seek). `None` when no leg is on that map.
    pub(crate) fn reseek_to_map(
        &self,
        anchor: &mut TransportAnchor,
        map_id: u32,
    ) -> Option<(TransportSample, Option<u32>)> {
        let Drive::Taxi(t) = &self.drive else {
            return None;
        };
        let cycle = self.cycle_ms(anchor);
        // Already there: seeking would rewind to the leg's head and jerk the deck backwards.
        let here = t.sample(cycle);
        if here.map == map_id {
            return Some((here, None));
        }
        let target = t.first_cycle_on_map(cycle, map_id)?;
        anchor.progress_ms = target;
        anchor.at = Instant::now();
        Some((t.sample(target), Some(target)))
    }

    /// Whether any leg of the cycle lies on `map_id`, the cross-map worldport's spare predicate. A
    /// lift answers no: the server re-sends the destination's transports on arrival
    /// (`SendInitTransports`), so a lift despawns and streams back like any object.
    pub(crate) fn touches_map(&self, map_id: u32) -> bool {
        match &self.drive {
            Drive::Taxi(t) => t.touches_map(map_id),
            Drive::Lift(_) => false,
        }
    }

    /// The sample at `cycle_ms`. `here` is the viewer's map: a taxi ignores it (its timetable
    /// knows each leg's map), a lift reports it, as a type-11 exists only on the viewer's map.
    fn sample(&self, cycle_ms: u32, here: u32) -> TransportSample {
        match &self.drive {
            Drive::Taxi(t) => t.sample(cycle_ms),
            Drive::Lift(l) => {
                let (pos, moving) = elevator_sample(&l.frames, l.base_pos, l.quat, cycle_ms);
                TransportSample {
                    map: here,
                    pos,
                    heading: l.yaw,
                    moving,
                }
            }
        }
    }

    /// The live sample for an anchor, the pose the tick writes this frame; for instruments.
    pub(crate) fn sample_at(&self, anchor: &TransportAnchor, here: u32) -> TransportSample {
        self.sample(self.cycle_ms(anchor), here)
    }
}

/// `TaxiPathNode.dbc` with the per-path timetables built from it (`None` in `built` is a failed
/// build, not retried), and the `TransportAnimation.dbc` lift keyframes, both shared by path.
#[derive(Resource, Default)]
struct Timetables {
    nodes: Option<TaxiPathNodes>,
    built: HashMap<u32, Option<Arc<TransportTimetable>>>,
    elevators: Option<ElevatorPaths>,
    lifts: HashMap<u32, Arc<Vec<ElevatorKeyframe>>>,
}

fn setup_taxi_nodes(mut timetables: ResMut<Timetables>, world_assets: Option<Res<WorldAssets>>) {
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match load_taxi_path_nodes(&mut chain) {
        Ok(nodes) => {
            info!("taxi path nodes: {} paths", nodes.len());
            timetables.nodes = Some(nodes);
        }
        // Without taxi data boats stay at their create pose.
        Err(e) => warn!("taxi path nodes unavailable, transports stay put: {e:#}"),
    }
    match load_elevator_paths(&mut chain) {
        Ok(paths) => {
            info!("transport animations: {} lift paths", paths.len());
            timetables.elevators = Some(paths);
        }
        // Without keyframes cars stay at their spawn point.
        Err(e) => warn!("TransportAnimation unavailable, lifts stay put: {e:#}"),
    }
}

/// Arm anchored GameObjects: a boat once its template's taxi tuple lands, a lift
/// ([`ElevatorSeed`]) once the keyframe catalog is open. The lift arm reads no world state: at
/// login it runs a stage before `player::wire_in` writes `CurrentMap`.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn arm_transports(
    mut commands: Commands,
    mut timetables: ResMut<Timetables>,
    templates: Res<GameObjectTemplates>,
    // A re-created lift is already armed but carries a fresh seed, so it re-enters to consume it.
    pending: Query<
        (Entity, &Guid, Option<&ElevatorSeed>),
        (
            With<TransportAnchor>,
            Or<(Without<Transport>, With<ElevatorSeed>)>,
        ),
    >,
) {
    if pending.is_empty() {
        return;
    }
    for (entity, guid, seed) in &pending {
        // The lift arm.
        if let Some(seed) = seed {
            let Timetables {
                elevators, lifts, ..
            } = &mut *timetables;
            let Some(paths) = elevators.as_ref() else {
                continue; // no client data: stays frozen
            };
            let Some(frames) = paths.entry(seed.entry) else {
                // No authored path: park it at its spawn pose, as the reference does. The anchor
                // goes too, so it leaves this query, and it is unhidden (anchored creates spawn
                // `Hidden` until their first ticked pose).
                warn!(
                    "transport: no TransportAnimation path for entry {} (guid {:#x}) — parked",
                    seed.entry, guid.0
                );
                commands
                    .entity(entity)
                    .remove::<(ElevatorSeed, TransportAnchor)>()
                    .insert(Visibility::default())
                    // Inserting `Visibility` re-adds the sweep row a net root sheds at spawn.
                    .vis_chain_only();
                continue;
            };
            let frames = lifts
                .entry(seed.entry)
                .or_insert_with(|| Arc::new(frames.to_vec()))
                .clone();
            let period_ms = elevator_period_ms(&frames);
            // No rotation on the wire: the pure-yaw quat, the encoding 1.12 spawn rows use.
            let quat = seed.quat.unwrap_or_else(|| {
                let (s, c) = (seed.yaw * 0.5).sin_cos();
                [0.0, 0.0, s, c]
            });
            debug!(
                "transport: lift entry {} armed — period {period_ms} ms (guid {:#x})",
                seed.entry, guid.0
            );
            commands.entity(entity).remove::<ElevatorSeed>().insert((
                Transport {
                    drive: Drive::Lift(Lift {
                        frames,
                        period_ms,
                        base_pos: seed.base_pos,
                        quat,
                        yaw: seed.yaw,
                    }),
                    was_moving: false,
                },
                // Kinematic for the same reason as the boats below.
                RigidBody::Kinematic,
            ));
            continue;
        }

        // The boat arm.
        let Some(mo) = templates.get(guid.0).and_then(|t| t.mo_transport) else {
            continue; // template answer not in yet: next frame
        };
        let Timetables { nodes, built, .. } = &mut *timetables;
        let Some(nodes) = nodes.as_ref() else {
            continue; // no client data: stays frozen
        };
        let timetable = built
            .entry(mo.taxi_path_id)
            .or_insert_with(|| {
                let t = nodes.path(mo.taxi_path_id).and_then(|path| {
                    // The build pins its period to the reference's own bookkeeping (`0x5f4cc0`).
                    let t = TransportTimetable::build(path, mo.move_speed, mo.accel_rate)?;
                    info!(
                        "transport: path {} timetable built — period {} ms",
                        mo.taxi_path_id, t.period_ms
                    );
                    Some(t)
                });
                if t.is_none() {
                    warn!(
                        "transport: path {} timetable build FAILED (guid {:#x})",
                        mo.taxi_path_id, guid.0
                    );
                }
                t.map(Arc::new)
            })
            .clone();
        if let Some(timetable) = timetable {
            // Kinematic, not the GO default of Static: it moves every frame, and avian's static
            // tree is tuned for immobile geometry.
            commands.entity(entity).insert((
                Transport {
                    drive: Drive::Taxi(timetable),
                    was_moving: false,
                },
                RigidBody::Kinematic,
            ));
        }
    }
}

/// The per-frame tick, the reference's `(progress + anchor) % period` leg walk (`0x5f50a0`):
/// sample and write translation and rotation (model scale is baked in by the renderer). An
/// off-map sample, a boat on the other continent's leg, hides the model; a lift never trips it.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn tick_transports(
    current_map: Option<Res<CurrentMap>>,
    mut transports: Query<(
        &Guid,
        &mut Transport,
        &TransportAnchor,
        &mut Transform,
        &mut Visibility,
        Option<&mut Position>,
        Option<&mut Rotation>,
    )>,
) {
    let Some(current_map) = current_map else {
        return;
    };
    for (guid, mut transport, anchor, mut transform, mut visibility, position, rotation) in
        &mut transports
    {
        let cycle = transport.cycle_ms(anchor);
        let sample = transport.sample(cycle, current_map.0);
        if sample.moving != transport.was_moving {
            transport.was_moving = sample.moving;
            debug!(
                "transport {:#x}: {} at cycle {cycle} ms (period {}, map {})",
                guid.0,
                if sample.moving { "departed" } else { "docked" },
                transport.period_ms(),
                sample.map,
            );
        }
        // Write on change only: a docked deck samples the same pose every frame.
        if sample.map != current_map.0 {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        visibility.set_if_neq(Visibility::Inherited);
        write_deck_pose(&sample, &mut transform, position, rotation);
    }
}

/// Write a sampled pose into the transform and avian's `Position`/`Rotation`: avian syncs
/// `Transform` before `Update`, so without the mirror this frame's casts see last frame's deck.
/// Takes `&mut Mut<Transform>` so the change flag is set only on a real change; a plain
/// `&mut Transform` would re-dirty the deck's whole subtree every frame.
fn write_deck_pose(
    sample: &TransportSample,
    transform: &mut Mut<Transform>,
    position: Option<Mut<Position>>,
    rotation: Option<Mut<Rotation>>,
) {
    let translation = benilla_assets::coords::wow_to_bevy(sample.pos);
    let heading = Quat::from_rotation_y(sample.heading);
    // Read through `Deref` (no flag), write through `DerefMut` (flag), only on a change.
    if transform.translation != translation || transform.rotation != heading {
        transform.translation = translation;
        transform.rotation = heading;
    }
    if let Some(mut p) = position {
        if p.0 != transform.translation {
            p.0 = transform.translation;
        }
    }
    if let Some(mut r) = rotation {
        if r.0 != transform.rotation {
            r.0 = transform.rotation;
        }
    }
}

/// Put the ridden transport on the destination map in the worldport frame, so `wire_in` composes
/// the rider's pose through the destination continent's coordinates rather than off its grid.
///
/// The reference keeps nothing across `SMSG_NEW_WORLD`: `0x401bc0` destroys every object
/// (`0x467700` → `0x467800`), builds a manager on the destination map (`0x464ff0`) and rebuilds
/// rider and transport from the post-ack creates. We spare the ridden transport instead; the
/// server's create for it (`Map.cpp:1698-1702`, which is why `SendInitTransports` skips it at
/// `Map.cpp:1726`) re-anchors it a round trip later, and this fills that window.
///
/// It writes the pose itself because `CurrentMap` still names the source map this frame, so
/// [`tick_transports`] would hide the deck. Order: before it, and before `WorldStage::Input`.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn reseek_ridden_transport_at_worldport(
    mut worldports: MessageReader<crate::net::WorldportMessage>,
    player: Res<crate::player::Player>,
    mut transports: Query<(
        &Guid,
        &Transport,
        &mut TransportAnchor,
        &mut Transform,
        &mut Visibility,
        Option<&mut Position>,
        Option<&mut Rotation>,
    )>,
) {
    for w in worldports.read() {
        // Only a transport transfer while still riding, `wire_in`'s riding predicate.
        if w.transport_entry.is_none() {
            continue;
        }
        let Some(deck) = player.ride_entity() else {
            continue;
        };
        let Ok((guid, transport, mut anchor, mut transform, mut visibility, position, rotation)) =
            transports.get_mut(deck)
        else {
            continue;
        };
        let before = transport.cycle_ms(&anchor);
        let Some((sample, moved_to)) = transport.reseek_to_map(&mut anchor, w.map_id) else {
            warn!(
                "transport {:#x}: rode a transfer to map {} its own path never visits — cannot \
                 re-anchor; the rider's pose will be composed through a stale one",
                guid.0, w.map_id
            );
            continue;
        };
        // When the clock moved, log how far behind the server's our path clock was, measured
        // forward around the cycle: a crossing is often the cycle wrap (path 241's Kalimdor legs
        // come first), where a plain `after - before` goes negative.
        if let Some(after) = moved_to {
            let period = transport.period_ms().max(1);
            let behind =
                (u64::from(after) + u64::from(period) - u64::from(before)) % u64::from(period);
            info!(
                "transport {:#x}: re-anchored across the seam to map {} — our path clock was \
                 {behind} ms behind the server's (cycle {before} → {after} ms of {period} ms). \
                 Deck now at [{:.1}, {:.1}, {:.1}].",
                guid.0, w.map_id, sample.pos[0], sample.pos[1], sample.pos[2],
            );
        }
        visibility.set_if_neq(Visibility::Inherited);
        write_deck_pose(&sample, &mut transform, position, rotation);
    }
}

/// Frames the ridden transport's map may disagree with [`CurrentMap`] before it is logged; one
/// frame of it is normal at every crossing.
const DISAGREEMENT_GRACE_FRAMES: u32 = 3;

/// Tripwire: the transport we ride samples another map. Its frozen off-map pose is what
/// [`crate::player::wire_in`] composes our body through at a worldport, which lands it off the
/// destination's grid; it arises when our path clock and the server's disagree about the
/// crossing. One frame is normal: `CurrentMap` is written by a command insert, a frame after
/// [`reseek_ridden_transport_at_worldport`], so only a persisting state logs, once per entry.
fn watch_ridden_transport_map(
    current_map: Option<Res<CurrentMap>>,
    player: Res<crate::player::Player>,
    transports: Query<(&Guid, &Transport, &TransportAnchor)>,
    mut reported: Local<Option<u64>>,
    mut streak: Local<u32>,
) {
    let Some(current_map) = current_map else {
        return;
    };
    let Some((guid, transport, anchor)) = player
        .ride_entity()
        .and_then(|deck| transports.get(deck).ok())
    else {
        *reported = None;
        *streak = 0;
        return;
    };
    let cycle = transport.cycle_ms(anchor);
    let sample = transport.sample(cycle, current_map.0);
    if sample.map == current_map.0 {
        *reported = None;
        *streak = 0;
        return;
    }
    *streak += 1;
    if *streak < DISAGREEMENT_GRACE_FRAMES {
        return;
    }
    if reported.replace(guid.0) == Some(guid.0) {
        return;
    }
    warn!(
        "transport {:#x}: RIDDEN but its own timetable says map {} while we are on map {} — \
         cycle {cycle} ms of {} ms, sampled pose [{:.1}, {:.1}, {:.1}]. Its transform is frozen \
         at that off-map pose, so anything composed through it (our own body at a worldport) \
         lands in the wrong continent's coordinates.",
        guid.0,
        sample.map,
        current_map.0,
        transport.period_ms(),
        sample.pos[0],
        sample.pos[1],
        sample.pos[2],
    );
}

/// An observed mover's pose in a transport's frame, the `MOVEFLAG_ONTRANSPORT` tail of its
/// `LIVING` create block or `MSG_MOVE_*` relay. [`compose_riders`] carries it every frame; the
/// local pose changes only per packet, with no extrapolation in the deck frame.
#[derive(Component)]
pub(crate) struct TransportRider {
    pub(crate) transport_guid: u64,
    /// Position in the transport's frame, raw WoW coords.
    pub(crate) local_pos: [f32; 3],
    /// Facing in the transport's frame (WoW radians); world facing is `local + transport yaw`.
    pub(crate) local_orientation: f32,
}

/// Compose each observed rider's world pose through its transport's this-frame pose, after
/// [`tick_transports`], and refresh [`RemoteMotion`]'s WoW pose to match. A rider whose transport
/// is not streamed keeps its last world pose; the server despawns the pair together.
fn compose_riders(
    guid_index: Res<crate::net::GuidIndex>,
    boats: Query<&Transform, (With<Transport>, Without<TransportRider>)>,
    mut riders: Query<
        (
            &TransportRider,
            &mut Transform,
            Option<&mut crate::net::RemoteMotion>,
        ),
        Without<Transport>,
    >,
) {
    for (rider, mut transform, motion) in &mut riders {
        let Some(&boat_entity) = guid_index.0.get(&rider.transport_guid) else {
            continue;
        };
        let Ok(boat) = boats.get(boat_entity) else {
            continue;
        };
        let local = benilla_assets::coords::wow_to_bevy(rider.local_pos);
        let world = boat.translation + boat.rotation * local;
        let yaw = rider.local_orientation + boat.rotation.to_euler(EulerRot::YXZ).0;
        transform.translation = world;
        transform.rotation = Quat::from_rotation_y(yaw);
        if let Some(mut rm) = motion {
            rm.wow_pos = benilla_assets::coords::bevy_to_wow(world);
            rm.orientation = yaw;
        }
    }
}

/// Re-ground a rider on a grounded deck spline onto the deck. The reference treats a grounded
/// `SMSG_MONSTER_MOVE_TRANSPORT` spline as a plain one: the transport guid is in neither
/// `0x616cb0`'s predicate nor `0x634040`'s dispatch, `0x616d03` zeroes the Z delta, and the WALK
/// resolver `0x6367b0` re-derives Z from a world trace (`0x636e52`) at the composed position,
/// whose class mask from `0x6315f0` (`0x100111`) has the `0x100000` bit that admits GameObject
/// meshes, the deck among them.
///
/// It runs here, not in [`crate::net::ground_clamp_creatures`] (in `WorldStage::Net`), because
/// only after [`compose_riders`] and [`republish_moved_collider_aabbs`] are the rider's position
/// and the deck's box this frame's. The height goes to the `Transform`, not `local_pos`, which
/// [`compose_riders`] recomposes from every frame.
///
/// Not modelled: `0x616af0`'s anti-warp guard takes the wire Z verbatim, skipping the re-ground,
/// when the horizontal step exceeds 3 yd (`[0x80c5bc]` = 9.0, squared) or 60 yd/s, the common
/// first substep on a deck; ours re-grounds that frame, and both settle the same the next.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn ground_deck_riders(
    world: benilla_world::collision::WorldCollision,
    mut riders: Query<
        (
            &mut Transform,
            Option<&mut crate::net::RemoteMotion>,
            &crate::net::Spline,
        ),
        (With<TransportRider>, Without<Transport>),
    >,
) {
    /// The probe's reach above and below the seat, the creature clamp's
    /// (`GROUND_CLAMP_UP`/`_DOWN`): short of a deck one storey up.
    const UP: f32 = 2.5;
    const DOWN: f32 = 4.0;
    for (mut transform, motion, spline) in &mut riders {
        // Only a grounded deck path: a flying one (the `0x200` bit) owns its altitude, and a
        // world-frame spline is `ground_clamp_creatures`'.
        if spline.deck.is_none() || !spline.grounded {
            continue;
        }
        let origin = transform.translation + Vec3::Y * UP;
        let Some(hit) = world.ray_body(origin, Dir3::NEG_Y, UP + DOWN) else {
            continue; // nothing under the rider: keep the composed pose
        };
        let y = origin.y - hit.distance;
        if y == transform.translation.y {
            continue;
        }
        transform.translation.y = y;
        if let Some(mut rm) = motion {
            rm.wow_pos = benilla_assets::coords::bevy_to_wow(transform.translation);
        }
    }
}

/// Publish the ride frame onto every model standing on a transport, the reference's `SetMoveBase`
/// install (`0x617170`/`0x618970` → `0x718910` → `[CM2Model+0x17c]`), the frame a rider's
/// world-space effects are stored in ([`benilla_world::ride_frame`]). The body we steer takes the
/// mover's platform attach, an observed rider its [`TransportRider`] tail; attached models inherit
/// it through `ParentModel`, as the reference propagates `[model+0x17c]` to children every frame
/// (`0x7142c1` in the animate kernel `0x714260`).
fn stamp_ride_frames(
    mut commands: Commands,
    player: Res<crate::player::Player>,
    guid_index: Res<crate::net::GuidIndex>,
    body: Query<Entity, With<crate::net::Embodied>>,
    riders: Query<(Entity, &TransportRider)>,
    stamped: Query<(Entity, &RideFrame)>,
) {
    let mut want: HashMap<Entity, Entity> = HashMap::new();
    if let (Ok(me), Some(deck)) = (body.single(), player.ride_entity()) {
        want.insert(me, deck);
    }
    for (rider, on) in &riders {
        // A rider whose transport is not streamed in keeps no frame.
        if let Some(&deck) = guid_index.0.get(&on.transport_guid) {
            want.insert(rider, deck);
        }
    }
    // Reconcile rather than re-stamp, so an unchanged rider's component is never touched.
    for (model, current) in &stamped {
        match want.remove(&model) {
            Some(deck) if deck == current.0 => {}
            Some(deck) => {
                commands.entity(model).insert(RideFrame(deck));
            }
            None => {
                commands.entity(model).remove::<RideFrame>();
            }
        }
    }
    for (model, deck) in want {
        commands.entity(model).insert(RideFrame(deck));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::ElevatorKeyframe;

    /// The Ratchet–Booty Bay ferry's real timetable (taxi path 241): the builder can truncate a
    /// short synthetic cycle so the far continent's legs are never reached.
    fn ferry_241(chain: &mut benilla_formats::Chain) -> Transport {
        let cat = benilla_formats::load_taxi_path_nodes(chain).expect("TaxiPathNode.dbc");
        let nodes = cat.path(241).expect("taxi path 241 (Ratchet–Booty Bay)");
        let tt = TransportTimetable::build(nodes, 30.0, 1.0).expect("path 241 builds");
        Transport {
            drive: Drive::Taxi(Arc::new(tt)),
            was_moving: false,
        }
    }

    /// Path 241's Kalimdor legs are the first frames of the cycle, so crossing to Kalimdor is the
    /// cycle wrap, not an interior map change.
    #[test]
    fn the_seam_reanchor_lands_on_the_new_map_and_leaves_an_agreeing_clock_alone() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let transport = ferry_241(&mut chain);
        let period = transport.period_ms();

        // Where each continent's legs fall in the built timetable.
        let on = |map: u32| {
            (0..period)
                .step_by(500)
                .find(|&ms| transport.sample(ms, 0).map == map)
                .unwrap_or_else(|| panic!("path 241 has an instant on map {map}"))
        };
        let (on_azeroth, on_kalimdor) = (on(0), on(1));

        // On Azeroth's leg, told Kalimdor: the stored anchor moves onto Kalimdor.
        let mut anchor = TransportAnchor {
            progress_ms: on_azeroth,
            at: Instant::now(),
        };
        let (sample, moved_to) = transport
            .reseek_to_map(&mut anchor, 1)
            .expect("path 241 visits Kalimdor");
        assert_eq!(sample.map, 1, "the re-anchored sample must be on Kalimdor");
        assert!(
            moved_to.is_some(),
            "a clock on Azeroth's leg told it is on Kalimdor must report that it moved"
        );
        assert_eq!(
            transport.sample(anchor.progress_ms, 0).map,
            1,
            "the stored anchor must itself sit on Kalimdor, not just the returned sample"
        );

        // Already on Kalimdor: the clock is left alone.
        let mut anchor = TransportAnchor {
            progress_ms: on_kalimdor,
            at: Instant::now(),
        };
        let (sample, moved_to) = transport
            .reseek_to_map(&mut anchor, 1)
            .expect("path 241 visits Kalimdor");
        assert_eq!(sample.map, 1);
        assert_eq!(
            anchor.progress_ms, on_kalimdor,
            "an agreeing clock must not be touched"
        );
        assert_eq!(
            moved_to, None,
            "a no-op seek must report that it did not move the clock"
        );

        // A map the path never visits has no answer.
        let mut anchor = TransportAnchor {
            progress_ms: on_azeroth,
            at: Instant::now(),
        };
        assert!(transport.reseek_to_map(&mut anchor, 4242).is_none());
        assert_eq!(
            anchor.progress_ms, on_azeroth,
            "a failed seek must not move the clock"
        );
    }

    #[test]
    fn a_lift_has_no_seam_to_reanchor() {
        let lift = Transport {
            drive: Drive::Lift(parked_lift()),
            was_moving: false,
        };
        let mut anchor = TransportAnchor {
            progress_ms: 0,
            at: Instant::now(),
        };
        assert!(lift.reseek_to_map(&mut anchor, 1).is_none());
    }

    /// A two-frame pause path at `base_pos`.
    fn parked_lift() -> Lift {
        Lift {
            frames: Arc::new(vec![
                ElevatorKeyframe {
                    time_ms: 0,
                    pos: [0.0; 3],
                },
                ElevatorKeyframe {
                    time_ms: 1000,
                    pos: [0.0; 3],
                },
            ]),
            period_ms: 1000,
            base_pos: [10.0, 20.0, 30.0],
            quat: [0.0, 0.0, 0.0, 1.0],
            yaw: 0.5,
        }
    }

    /// A pause path samples the same pose forever, so after the arming write nothing changes.
    #[test]
    fn a_docked_lift_stops_dirtying_its_transform_and_visibility() {
        #[derive(Resource, Default)]
        struct Dirty {
            transforms: usize,
            visibilities: usize,
        }
        fn spy(
            t: Query<Entity, (Changed<Transform>, With<Transport>)>,
            v: Query<Entity, (Changed<Visibility>, With<Transport>)>,
            mut out: ResMut<Dirty>,
        ) {
            out.transforms = t.iter().count();
            out.visibilities = v.iter().count();
        }
        let mut app = App::new();
        app.insert_resource(CurrentMap(0))
            .init_resource::<Dirty>()
            .add_systems(Update, (tick_transports, spy).chain());
        app.world_mut().spawn((
            Guid(0xF110_0000_0000_0001),
            Transport {
                drive: Drive::Lift(parked_lift()),
                was_moving: false,
            },
            TransportAnchor {
                progress_ms: 0,
                at: Instant::now(),
            },
            Transform::default(),
            Visibility::default(),
        ));
        app.update(); // arming frame: the first pose write lands (and spawn reads as Changed)
        for _ in 0..3 {
            app.update();
        }
        let dirty = app.world().resource::<Dirty>();
        assert_eq!(
            (dirty.transforms, dirty.visibilities),
            (0, 0),
            "a docked lift must stop dirtying its transform and visibility"
        );
    }

    /// At login lifts arm before `CurrentMap` is written, so the off-map hide must never judge a
    /// lift against a map.
    #[test]
    fn a_lift_is_visible_on_whatever_map_the_viewer_is_on() {
        let mut app = App::new();
        // Alterac Valley, a map with no type-11 GameObject: the value must not matter.
        app.insert_resource(CurrentMap(30))
            .add_systems(Update, tick_transports);
        let lift = app
            .world_mut()
            .spawn((
                Guid(0xF110_0000_0000_0002),
                Transport {
                    drive: Drive::Lift(parked_lift()),
                    was_moving: false,
                },
                TransportAnchor {
                    progress_ms: 0,
                    at: Instant::now(),
                },
                Transform::default(),
                // Anchored transports spawn hidden until their first ticked pose.
                Visibility::Hidden,
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(lift),
            Some(&Visibility::Inherited),
            "the tick must unhide a lift on the viewer's map, not judge it against a stamped one"
        );
        // The pose is the base plus its zero offset, in Bevy axes.
        assert_eq!(
            app.world().get::<Transform>(lift).map(|t| t.translation),
            Some(benilla_assets::coords::wow_to_bevy([10.0, 20.0, 30.0])),
        );
    }
}
