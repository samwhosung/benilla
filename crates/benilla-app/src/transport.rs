//! Transports, whose cycle the client computes. The server never streams a transport's position,
//! only an anchor: the movement block's `UPDATE_FLAG_TRANSPORT` u32, its path-progress ms at
//! create, re-sent on every re-create.
//!
//! - Type 15 (`MO_TRANSPORT`, boats and zeppelins): the reference's ship path from
//!   `TaxiPathNode.dbc` and the template's `(taxiPathId, moveSpeed, accelRate)`
//!   ([`benilla_formats::TransportTimetable`], `0x5f4cc0`), placed at `(anchor + elapsed) % period`
//!   (`0x5f50a0`).
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
    ElevatorKeyframe, ElevatorPaths, ShipAnim, TaxiPathNodes, TransportSample, TransportTimetable,
};
use bevy::prelude::*;

use crate::go_templates::GameObjectTemplates;
use crate::net::{Guid, ObjectStore};
use benilla_assets::{LockRecover, WorldAssets};
use benilla_world::doodad_anim::CarrierAnimation;
use benilla_world::ride_frame::RideFrame;
use benilla_world::vis_chain::VisChainOnly;
use benilla_world::world_map::CurrentMap;

pub(crate) struct TransportPlugin;

impl Plugin for TransportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Timetables>()
            .add_message::<TransportLegLeft>()
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
                    deck_ticks(),
                    republish_moved_collider_aabbs::<Collider>,
                    compose_riders,
                    ground_deck_riders,
                )
                    .chain()
                    .after(benilla_world::schedule::WorldStage::Net)
                    .before(benilla_world::schedule::WorldStage::Input)
                    // A script call after the packets applies inside its verb, against the decks
                    // as they stand this frame; the input records' calls read them as the last
                    // frame left them, as the reference's input runs ahead of its frame time.
                    .before(crate::vm_pass::VmPass::Resume),
            )
            // The ride frame last, after `PlayerControlSet` decides what we stand on; its
            // consumers, the particle and ribbon sims, run in `PostUpdate`.
            .add_systems(
                Update,
                stamp_ride_frames.after(crate::player::PlayerControlSet),
            );
    }
}

/// The systems that arm the transports and pose their decks for this frame, in order.
fn deck_ticks() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (
        arm_transports,
        attach_transport_volumes,
        tick_transports,
        watch_ridden_transport_map,
    )
        .chain()
}

/// A ship we ride left leg `leg` for another on the map we are on: the tick's
/// `0x407de0(entry, leg)` (`0x5f54c2`), which raises the loading screen with that leg's route.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(crate) struct TransportLegLeft {
    /// The ship's `OBJECT_FIELD_ENTRY` (`0x5f54bd`).
    pub(crate) entry: u32,
    pub(crate) leg: usize,
}

/// The create-block anchor: the server's path-progress ms and the local instant it arrived,
/// inserted on every create that carries `UPDATE_FLAG_TRANSPORT`.
#[derive(Component, Clone, Copy)]
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
    /// The ship's leg as its tick last stored it (`+0x58`).
    leg: Option<usize>,
    /// The animation it last played (`+0x54`, −1 until the first): a leg change raises the route
    /// screen only after one (`0x5f50b9`, `0x5f547a`).
    anim: Option<ShipAnim>,
}

impl Transport {
    fn new(drive: Drive) -> Self {
        Self {
            drive,
            was_moving: false,
            leg: None,
            anim: None,
        }
    }
}

/// A transport's answer to "is this point aboard" (handler slot 21, `0x5f55c0` and `0x5f6540`,
/// through `0x670a70`), for a point in its own frame, WoW axes: a map object (tag 0x8) by its
/// MCVP planes (`0x6a4990`), a doodad (tag 0x40) by its file's box at `+0xb4`, faces included and
/// a NaN inside. `0x6173b0` holds a rider aboard while it says yes.
#[derive(Component, Clone, Debug, PartialEq)]
pub(crate) enum TransportVolume {
    Planes(Arc<[[f32; 4]]>),
    Box([f32; 3], [f32; 3]),
}

impl TransportVolume {
    pub(crate) fn contains(&self, p: [f32; 3]) -> bool {
        match self {
            Self::Planes(planes) => benilla_formats::point_in_convex_volume(planes, p),
            // 0x670ae2-0x670cc8: the six planes on the box's faces, `d = −n·corner`, each distance
            // `((nx·px + nz·pz) + ny·py) + d` failing only above 0.
            Self::Box(min, max) => {
                let [px, py, pz] = p.map(f64::from);
                (0..6).all(|i| {
                    let mut n = [0.0f64; 3];
                    n[i / 2] = if i % 2 == 0 { 1.0 } else { -1.0 };
                    let corner = if i % 2 == 0 { max } else { min };
                    let d = -(n[0] * f64::from(corner[0])
                        + n[1] * f64::from(corner[1])
                        + n[2] * f64::from(corner[2]));
                    let dist = ((n[0] * px + n[2] * pz) + n[1] * py) + f64::from(d as f32);
                    dist <= 0.0 || dist.is_nan()
                })
            }
        }
    }
}

/// Give each armed transport its [`TransportVolume`] once its model has loaded.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn attach_transport_volumes(
    mut commands: Commands,
    gameobjects: Option<Res<crate::entities::GameObjects>>,
    wmos: Res<Assets<benilla_assets::WmoModel>>,
    m2s: Res<Assets<benilla_assets::M2Model>>,
    pending: Query<(Entity, &crate::net::NetEntity), (With<Transport>, Without<TransportVolume>)>,
) {
    let Some(gameobjects) = gameobjects else {
        return;
    };
    for (entity, net) in &pending {
        if let Some(volume) = net
            .display_id
            .and_then(|d| gameobjects.transport_volume(d, &wmos, &m2s))
        {
            commands.entity(entity).insert(volume);
        }
    }
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
                    leg: 0,
                    map: here,
                    pos,
                    heading: l.yaw,
                    // A lift has no speed curve: under way reads as cruising.
                    anim: if moving {
                        ShipAnim::Cruise
                    } else {
                        ShipAnim::Stand
                    },
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
        (Entity, &Guid, Option<&ElevatorSeed>, Option<&ObjectStore>),
        (
            With<TransportAnchor>,
            Or<(Without<Transport>, With<ElevatorSeed>)>,
        ),
    >,
) {
    if pending.is_empty() {
        return;
    }
    for (entity, guid, seed, store) in &pending {
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
                Transport::new(Drive::Lift(Lift {
                    frames,
                    period_ms,
                    base_pos: seed.base_pos,
                    quat,
                    yaw: seed.yaw,
                })),
                // Kinematic for the same reason as the boats below.
                RigidBody::Kinematic,
            ));
            continue;
        }

        // The boat arm.
        let Some(mo) = store
            .and_then(|s| templates.of(&s.0))
            .and_then(|t| t.mo_transport)
        else {
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
                Transport::new(Drive::Taxi(timetable)),
                RigidBody::Kinematic,
                // Nothing played until its model is ready.
                CarrierAnimation::default(),
            ));
        }
    }
}

/// The per-frame tick, the reference's `(progress + anchor) % period` leg walk (`0x5f50a0`):
/// sample and write translation and rotation (model scale is baked in by the renderer). A leg on
/// another map stores the leg and ends the tick (`0x5f5274`-`0x5f5279`), so a boat on the other
/// continent's leg keeps its last pose in view; a lift never trips it. A ship we ride changing
/// legs on our map, after its first animation, raises the route screen ([`TransportLegLeft`]).
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn tick_transports(
    current_map: Option<Res<CurrentMap>>,
    player: Option<Res<crate::player::Player>>,
    mut legs_left: MessageWriter<TransportLegLeft>,
    mut transports: Query<(
        Entity,
        &Guid,
        &mut Transport,
        Ref<TransportAnchor>,
        &mut Transform,
        &mut Visibility,
        Option<&mut Position>,
        Option<&mut Rotation>,
        (
            Has<TransportVolume>,
            Option<&ObjectStore>,
            Option<&mut CarrierAnimation>,
        ),
    )>,
) {
    let Some(current_map) = current_map else {
        return;
    };
    let riding = player.as_ref().and_then(|p| p.ride_entity());
    for (
        entity,
        guid,
        mut transport,
        anchor,
        mut transform,
        mut visibility,
        position,
        rotation,
        (loaded, store, carrier),
    ) in &mut transports
    {
        // A fresh clock is a new object's: the ship kept across a worldport, re-anchored by its
        // post-port create, ticks on as the one the reference builds then, with no leg and no
        // animation yet (`0x401bc0`).
        if anchor.is_added() {
            transport.leg = None;
            transport.anim = None;
        }
        let cycle = transport.cycle_ms(&anchor);
        let sample = transport.sample(cycle, current_map.0);
        if sample.moving() != transport.was_moving {
            transport.was_moving = sample.moving();
            debug!(
                "transport {:#x}: {} at cycle {cycle} ms (period {}, map {})",
                guid.0,
                if sample.moving() {
                    "departed"
                } else {
                    "docked"
                },
                transport.period_ms(),
                sample.map,
            );
        }
        // Anchored creates spawn hidden until their first tick, which shows them on any leg.
        visibility.set_if_neq(Visibility::Inherited);
        if sample.map != current_map.0 {
            transport.leg = Some(sample.leg);
            continue;
        }
        // Write on change only: a docked deck samples the same pose every frame.
        write_deck_pose(&sample, &mut transform, position, rotation);
        if !matches!(transport.drive, Drive::Taxi(_)) {
            continue;
        }
        // 0x5f50b9: whether no animation had played as the tick began.
        let first = transport.anim.is_none();
        // 0x5f52bd-0x5f5301: a loaded model plays the phase's animation when it changes, on its
        // doodads (`0x6709f0` → `0x6a24b0`).
        if loaded {
            if let Some(mut carrier) = carrier.filter(|_| transport.anim != Some(sample.anim)) {
                carrier.0 = Some(sample.anim.id() as u16);
            }
            transport.anim = Some(sample.anim);
        }
        // 0x5f5461-0x5f54c2: a new leg is stored, and with the camera's unit aboard and an
        // animation played, the tick calls `0x407de0(entry, old leg)`.
        let left = transport
            .leg
            .replace(sample.leg)
            .filter(|&l| l != sample.leg);
        if let Some(leg) = left.filter(|_| riding == Some(entity) && !first) {
            let entry = store.and_then(|s| s.0.object_entry()).unwrap_or(0);
            info!(
                "transport {:#x}: left leg {leg} for {} on map {}",
                guid.0, sample.leg, sample.map
            );
            legs_left.write(TransportLegLeft { entry, leg });
        }
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

/// Frames the ridden transport's map may disagree with [`CurrentMap`] before it is logged; one
/// frame of it is normal at every crossing.
const DISAGREEMENT_GRACE_FRAMES: u32 = 3;

/// Tripwire: the transport we ride samples another map. Its frozen off-map pose is what
/// [`crate::player::wire_in`] composes our body through at a worldport, which lands it off the
/// destination's grid; it arises when our path clock and the server's disagree about the
/// crossing, until our clock crosses too or the post-port create re-anchors it. One frame is
/// normal: `CurrentMap` is written by a command insert, a frame after the worldport, so only a
/// persisting state logs, once per entry.
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
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn compose_riders(
    guid_index: Res<crate::net::GuidIndex>,
    boats: Query<&Transform, (With<Transport>, Without<TransportRider>)>,
    // The body we steer is the mover's: its create's tail seated it (`player::wire_in`).
    mut riders: Query<
        (
            &TransportRider,
            &mut Transform,
            Option<&mut crate::net::RemoteMotion>,
        ),
        (Without<Transport>, Without<crate::net::Embodied>),
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
    riders: Query<(Entity, &TransportRider), Without<crate::net::Embodied>>,
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
impl Transport {
    /// A ship sailing two legs of five nodes, the first on `maps.0`, the second on `maps.1`, for
    /// other modules' tests.
    pub(crate) fn sailing_for_test(maps: (u32, u32)) -> Self {
        let node = |id: u32, map_id: u32, x: f32| benilla_formats::TaxiPathNode {
            id,
            path_id: 1,
            node_index: id,
            map_id,
            pos: [x, 0.0, 0.0],
            flags: u32::from(id == 4),
            delay: 0,
        };
        let nodes: Vec<_> = (0..10)
            .map(|i| node(i, if i < 5 { maps.0 } else { maps.1 }, i as f32 * 100.0))
            .collect();
        Self::new(Drive::Taxi(Arc::new(
            TransportTimetable::build(&nodes, 30, 1).expect("two legs build"),
        )))
    }

    /// A lift parked at the origin, for other modules' tests.
    pub(crate) fn parked_for_test() -> Self {
        Self::new(Drive::Lift(Lift {
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
            base_pos: [0.0; 3],
            quat: [0.0, 0.0, 0.0, 1.0],
            yaw: 0.0,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::ElevatorKeyframe;

    /// `0x670a70`'s doodad box: faces included, a NaN inside; a map object by its planes.
    #[test]
    fn a_transport_volume_holds_points_inside_it() {
        let lift = TransportVolume::Box([-1.0, -2.0, 0.0], [1.0, 2.0, 3.0]);
        assert!(lift.contains([0.0, 0.0, 1.5]));
        assert!(lift.contains([1.0, -2.0, 3.0]), "faces count");
        assert!(!lift.contains([0.0, 0.0, 3.01]));
        assert!(!lift.contains([-1.5, 0.0, 1.0]));
        // 0·NaN is NaN on every plane.
        assert!(lift.contains([f32::NAN, 100.0, 100.0]));
        let ship = TransportVolume::Planes(vec![[0.0, 0.0, 1.0, -10.0]].into());
        assert!(ship.contains([500.0, 0.0, 10.0]));
        assert!(!ship.contains([0.0, 0.0, 10.5]));
    }

    /// A ship path with no data: two legs of five nodes 100 yd apart along +x, the first on
    /// `maps.0` from x = 0, the second on `maps.1` from x = 1000, split after a flag 1 node when
    /// the maps are one.
    fn two_legs(maps: (u32, u32)) -> TransportTimetable {
        let node = |id: u32, map_id: u32, x: f32, flags: u32| benilla_formats::TaxiPathNode {
            id,
            path_id: 1,
            node_index: id,
            map_id,
            pos: [x, 0.0, 0.0],
            flags,
            delay: 0,
        };
        let nodes: Vec<_> = (0..5)
            .map(|i| node(i, maps.0, i as f32 * 100.0, u32::from(i == 4)))
            .chain((0..5).map(|i| node(5 + i, maps.1, 1000.0 + i as f32 * 100.0, 0)))
            .collect();
        TransportTimetable::build(&nodes, 30, 1).expect("two legs build")
    }

    fn taxi(t: TransportTimetable) -> Transport {
        Transport::new(Drive::Taxi(Arc::new(t)))
    }

    /// The deck systems alone, on `map`, with us riding `deck` when it is given.
    fn deck_app(map: u32) -> App {
        let mut app = App::new();
        app.insert_resource(CurrentMap(map))
            .init_resource::<Timetables>()
            .init_resource::<GameObjectTemplates>()
            .init_resource::<Assets<benilla_assets::WmoModel>>()
            .init_resource::<Assets<benilla_assets::M2Model>>()
            .init_resource::<crate::player::Player>()
            .add_message::<crate::net::WorldportMessage>()
            .add_message::<TransportLegLeft>()
            .add_systems(Update, deck_ticks());
        app
    }

    fn ride(app: &mut App, deck: Entity) {
        app.insert_resource(crate::player::Player::riding_for_test(
            deck,
            0xF120_0000_0000_0001,
        ));
    }

    /// The reference keeps no transport across `SMSG_NEW_WORLD` and has nothing that moves a
    /// clock: only the constructor writes the anchor (`0x5f7080`). A transfer we ride leaves the
    /// clock where it was, whatever map the worldport names.
    #[test]
    fn a_worldport_leaves_the_ridden_transports_clock_alone() {
        let mut app = deck_app(1);
        let at = Instant::now();
        let deck = app
            .world_mut()
            .spawn((
                Guid(0xF120_0000_0000_0001),
                taxi(two_legs((1, 0))),
                TransportAnchor {
                    progress_ms: 1000,
                    at,
                },
                Transform::default(),
                Visibility::Hidden,
            ))
            .id();
        ride(&mut app, deck);
        app.update();
        app.world_mut().write_message(crate::net::WorldportMessage {
            map_id: 0,
            position: [0.0; 3],
            orientation: 0.0,
            needs_ack: true,
            transport_entry: Some(176495),
        });
        app.update();
        let anchor = app.world().get::<TransportAnchor>(deck).unwrap();
        assert_eq!((anchor.progress_ms, anchor.at), (1000, at));
    }

    /// `0x5f5274`: a leg on another map ends the tick, so the ship keeps its last pose and stays
    /// in view; nothing hides it.
    #[test]
    fn a_ship_on_another_maps_leg_stays_where_it_was() {
        let mut app = deck_app(0);
        let pose = Transform::from_xyz(1.0, 2.0, 3.0);
        let deck = app
            .world_mut()
            .spawn((
                Guid(0xF120_0000_0000_0001),
                taxi(two_legs((1, 0))),
                TransportAnchor {
                    progress_ms: 1000,
                    at: Instant::now(),
                },
                pose,
                Visibility::Hidden,
            ))
            .id();
        app.update();
        assert_eq!(app.world().get::<Transform>(deck), Some(&pose), "not moved");
        assert_eq!(
            app.world().get::<Visibility>(deck),
            Some(&Visibility::Inherited),
            "in view"
        );
    }

    /// A ship of entry 176495 on two legs of one map, `riding` or not, its model loaded or not,
    /// ticked twice on leg 0 and then on leg 1: the leg-change calls it made.
    fn leg_changes(riding: bool, ready: bool) -> Vec<TransportLegLeft> {
        let mut app = deck_app(0);
        let timetable = two_legs((0, 0));
        let leg1 = (0..timetable.period_ms)
            .step_by(100)
            .find(|&t| timetable.sample(t).leg == 1)
            .expect("a second leg");
        let mut deck = app.world_mut().spawn((
            Guid(0xF120_0000_0000_0001),
            taxi(timetable),
            TransportAnchor {
                progress_ms: 1000,
                at: Instant::now(),
            },
            Transform::default(),
            Visibility::Hidden,
            // `OBJECT_FIELD_ENTRY`, field 3.
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(3, 176495)])),
        ));
        if ready {
            deck.insert(TransportVolume::Box([-1.0; 3], [1.0; 3]));
        }
        let deck = deck.id();
        if riding {
            ride(&mut app, deck);
        }
        app.update();
        app.update();
        app.world_mut()
            .get_mut::<TransportAnchor>(deck)
            .unwrap()
            .progress_ms = leg1 + 1000;
        app.update();
        app.world_mut()
            .resource_mut::<Messages<TransportLegLeft>>()
            .drain()
            .collect()
    }

    /// `0x5f52ec`-`0x5f5301`: a ship whose model is ready plays its speed curve's phase on its
    /// doodads (`0x6709f0` → `0x6a24b0`) when the phase changes; one still loading plays nothing.
    #[test]
    fn a_ready_ship_plays_its_phase_on_its_doodads() {
        // One leg of six nodes 100 yd apart with a 10 s stop at the fourth: too short a span to
        // reach cruise, the ship slows from the start into the stop, then stands.
        let node = |id: u32| benilla_formats::TaxiPathNode {
            id,
            path_id: 1,
            node_index: id,
            map_id: 0,
            pos: [id as f32 * 100.0, 0.0, 0.0],
            flags: if id == 3 { 2 } else { 0 },
            delay: if id == 3 { 10 } else { 0 },
        };
        let nodes: Vec<_> = (0..6).map(node).collect();
        let timetable = TransportTimetable::build(&nodes, 30, 1).expect("one leg builds");
        let first = timetable.sample(1000).anim;
        let next = (1000..timetable.period_ms)
            .step_by(100)
            .find(|&t| timetable.sample(t).anim != first)
            .expect("a second phase");
        assert_eq!(
            (first, timetable.sample(next).anim),
            (ShipAnim::Decelerate, ShipAnim::Stand)
        );
        let phases = |ready: bool| {
            let mut app = deck_app(0);
            let mut deck = app.world_mut().spawn((
                Guid(0xF120_0000_0000_0001),
                taxi(timetable.clone()),
                // As `arm_transports` arms a ship.
                CarrierAnimation::default(),
                TransportAnchor {
                    progress_ms: 1000,
                    at: Instant::now(),
                },
                Transform::default(),
                Visibility::Hidden,
            ));
            if ready {
                deck.insert(TransportVolume::Box([-1.0; 3], [1.0; 3]));
            }
            let deck = deck.id();
            let mut seen = Vec::new();
            for progress in [1000, next] {
                app.world_mut()
                    .get_mut::<TransportAnchor>(deck)
                    .unwrap()
                    .progress_ms = progress;
                app.update();
                seen.push(app.world().get::<CarrierAnimation>(deck).and_then(|a| a.0));
            }
            seen
        };
        assert_eq!(
            phases(true),
            [Some(0xa4), Some(0)],
            "slowing, then standing"
        );
        assert_eq!(phases(false), [None, None], "not ready");
    }

    /// `0x5f5461`-`0x5f54c2`: a leg change on the map we are on, with the camera's unit among
    /// the passengers and an animation already played (`+0x54` not −1), calls `0x407de0` with
    /// the ship's entry and the leg it left; anything else only stores the new leg.
    #[test]
    fn a_ridden_ship_changing_legs_on_our_map_raises_the_route() {
        assert_eq!(
            leg_changes(true, true),
            [TransportLegLeft {
                entry: 176495,
                leg: 0
            }]
        );
        assert_eq!(leg_changes(false, true), [], "not riding");
        assert_eq!(leg_changes(true, false), [], "no animation played yet");
    }

    /// The ship kept across a worldport waits, unanchored, for its create, and ticks from it as the
    /// new object the reference builds then (`0x401bc0`), with no leg and no animation: the leg it
    /// starts on raises nothing.
    #[test]
    fn a_ship_anchored_afresh_starts_its_legs_afresh() {
        let mut app = deck_app(1);
        let timetable = two_legs((1, 0));
        let leg1 = (0..timetable.period_ms)
            .step_by(100)
            .find(|&t| timetable.sample(t).leg == 1)
            .expect("a second leg");
        let deck = app
            .world_mut()
            .spawn((
                Guid(0xF120_0000_0000_0001),
                taxi(timetable),
                TransportAnchor {
                    progress_ms: 1000,
                    at: Instant::now(),
                },
                Transform::default(),
                Visibility::Hidden,
                TransportVolume::Box([-1.0; 3], [1.0; 3]),
            ))
            .id();
        ride(&mut app, deck);
        app.update();
        app.update();
        // The worldport keeps the deck and drops its clock; we land on map 0.
        app.world_mut().entity_mut(deck).remove::<TransportAnchor>();
        app.insert_resource(CurrentMap(0));
        app.update();
        // Its create, on map 0's leg.
        app.world_mut().entity_mut(deck).insert(TransportAnchor {
            progress_ms: leg1 + 1000,
            at: Instant::now(),
        });
        app.update();
        let left: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<TransportLegLeft>>()
            .drain()
            .collect();
        assert_eq!(left, []);
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
            .add_message::<crate::net::WorldportMessage>()
            .add_message::<TransportLegLeft>()
            .init_resource::<Dirty>()
            .add_systems(Update, (tick_transports, spy).chain());
        app.world_mut().spawn((
            Guid(0xF110_0000_0000_0001),
            Transport::new(Drive::Lift(parked_lift())),
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

    /// At login lifts arm before `CurrentMap` is written, so the off-map test must never judge a
    /// lift against a map.
    #[test]
    fn a_lift_is_visible_on_whatever_map_the_viewer_is_on() {
        let mut app = App::new();
        // Alterac Valley, a map with no type-11 GameObject: the value must not matter.
        app.insert_resource(CurrentMap(30))
            .add_message::<crate::net::WorldportMessage>()
            .add_message::<TransportLegLeft>()
            .add_systems(Update, tick_transports);
        let lift = app
            .world_mut()
            .spawn((
                Guid(0xF110_0000_0000_0002),
                Transport::new(Drive::Lift(parked_lift())),
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
