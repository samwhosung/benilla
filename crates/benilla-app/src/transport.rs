//! Transports (decision 0438) — the client-computed cycle, two drives, one subsystem.
//!
//! The server never streams a transport's position. It sends one **anchor** — the movement
//! block's `UPDATE_FLAG_TRANSPORT` u32, the server's path-progress ms clock at create — and the
//! client computes everything else, per GameObject type:
//!
//! - **Type 15 (`MO_TRANSPORT`, boats/zeppelins):** the whole timetable (per-leg Catmull-Rom
//!   splines + stop-to-stop trapezoid times, [`benilla_formats::TransportTimetable`]) from
//!   `TaxiPathNode.dbc` and the template's `(taxiPathId, moveSpeed, accelRate)` tuple; the boat
//!   sits at `timetable((anchor + local_elapsed) % period)` every frame.
//! - **Type 11 (`TRANSPORT`, elevators/lifts/trams — decision 0438 phase 3's second consumer):**
//!   the authored keyframe path from `TransportAnimation.dbc` keyed by the **template entry**;
//!   the car sits at `spawn + R(spawn_quat)·lerp(keyframes, (anchor + local_elapsed) % period)`
//!   ([`benilla_formats::elevator_sample`] — the client's path evaluator `0x5f6280`; vmangos
//!   `ElevatorTransport::Update` is the same math server-side).
//!
//! Re-creates (map transitions, the server's mid-course "update frames") re-anchor the clock;
//! decision 0318's replace semantics make that free.
//!
//! The armed entity is the ordinary streamed GameObject — the display (WMO for boats, M2 for
//! elevator cars), its collider, and its `Transform` all pre-exist on the GO-entity path
//! (decision 0438 §2); this module only *moves* it. Riding hangs off the same component set for
//! both drives — the mover's platform frame keys on [`Transport`], not on the drive.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

// avian's own public AABB/tree republish system, registered by name below. Aliased because the
// chain's concern is *when* it runs, not whose it is — and because the upstream name says what it
// does to colliders, not why this app needs it a second time each frame.
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
            // `.after(AssetSet::Open)` is load-bearing: the chain resource lands via Commands in
            // that set, and an unordered Startup system races it and silently finds no
            // `WorldAssets` (bit us on the first live run — every downstream log went quiet).
            .add_systems(
                Startup,
                setup_taxi_nodes.after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                // After the net drain (fresh anchors/re-anchors applied), before the player's
                // input stage — the boat must be at its this-frame pose before the mover reads
                // the world (phase 2's platform carry rides exactly this edge).
                //
                // **[`republish_moved_collider_aabbs`] is the third link, and it is here because
                // of a general engine truth, not a lift one** (decision 1663): *a collider moved
                // in `Update` is invisible to `Update`'s own spatial queries until the next
                // physics step, by up to one frame of its own travel.* avian refreshes
                // `ColliderAabb`, `EnlargedAabb` and the `ColliderTrees` proxies in
                // `ColliderTreeSystems::UpdateAabbs`, which sits in `PhysicsSchedule` — i.e. in
                // `FixedPostUpdate`, **before** `Update` — while [`tick_transports`] writes the
                // deck's pose *in* `Update`. Every `Update`-side spatial query
                // (`SpatialQuery::aabb_intersections_with_aabb`, which is how the mover's
                // ground/step/slide casts enumerate candidates — `benilla_world::collision`) is
                // therefore pruned against the deck's **previous-frame** box.
                //
                // At 60 Hz that is centimetres and harmless. On one long frame — a backgrounded
                // window is at least one ~1 s frame (decisions 0713/0906) — a Thunder Bluff lift
                // descends 7.3 yd while its box stays put, the box ends up *above* the rider it
                // is carrying, the down-probe enumerates no deck at all, and the rider is
                // declared airborne on the very platform they are standing on. Two such frames in
                // a row and `Time<Virtual>`'s clamped 250 ms lands a full gravity step on the
                // second one: the body drops 1.2 yd through the deck and never re-earns it,
                // because the deck is now above the probe. That is the reported fall-through, and
                // 1663 records the measured chain.
                //
                // The system is avian's own, public, and read-only on `LastPhysicsTick` — it
                // writes no tick resource, so running it a second time in the frame is
                // idempotent, and the next physics step simply recomputes what has not moved
                // since. It sweeps *every* collider, not just transports; 1663 records the
                // measured per-frame cost of that sweep in a streamed outdoor scene.
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
            // …and the ride frame LAST, after the mover has decided what we are standing on this
            // frame ([`crate::player::Player::ride`] is written inside `PlayerControlSet`). Its
            // consumers — the particle and ribbon sims — run in `PostUpdate`, so the deferred
            // insert has landed by the time anything reads it.
            .add_systems(
                Update,
                stamp_ride_frames.after(crate::player::PlayerControlSet),
            );
    }
}

/// The create-block anchor: the server's path-progress ms + the local instant it arrived.
/// Inserted (and re-inserted, on every re-create) by the net apply's `object_create` arm for a
/// GameObject whose movement block carried `UPDATE_FLAG_TRANSPORT`.
#[derive(Component)]
pub(crate) struct TransportAnchor {
    pub(crate) progress_ms: u32,
    pub(crate) at: Instant,
}

/// A pending type-11 elevator/lift: everything its arm needs, captured off the create block by
/// the net apply (no template round-trip — the keyframe path is keyed by the template **entry**,
/// which the create's `OBJECT_FIELD_ENTRY` already carries). Replaced by [`Transport`] once the
/// keyframe catalog answers; a GO whose entry has no authored path stays seeded-and-frozen (the
/// same silence the reference affords it).
#[derive(Component)]
pub(crate) struct ElevatorSeed {
    /// `gameobject_template` entry — `TransportAnimation.dbc`'s `TransportID` key.
    pub(crate) entry: u32,
    /// The stationary spawn position (raw WoW coords) — the movement block's `HAS_POSITION`
    /// slot (a transport create never carries `GAMEOBJECT_POS_*`).
    pub(crate) base_pos: [f32; 3],
    /// The spawn facing (wire orientation, WoW radians) — the car's constant rendered heading.
    pub(crate) yaw: f32,
    /// `GAMEOBJECT_ROTATION` — the spawn's local-rotation quaternion the keyframe offsets
    /// rotate through. `None` on a wire that carried no component (falls back to a pure-yaw
    /// quat built from `yaw`, which is exactly what the live spawn rows encode anyway).
    pub(crate) quat: Option<[f32; 4]>,
}

/// A ticking transport. Armed by [`arm_transports`]: a boat once the ask-once template answer
/// (the `(taxiPathId, moveSpeed, accelRate)` tuple) lands; an elevator as soon as the keyframe
/// catalog is open (its create block already carried everything else).
#[derive(Component)]
pub(crate) struct Transport {
    drive: Drive,
    /// Last frame's `moving` — the dock/depart edge detector behind the trace log (the phase-1
    /// reconciliation instrument: a "the boat isn't where the server thinks" report is settled
    /// against these timestamps, never by eye — decision 0438 phasing).
    was_moving: bool,
}

/// The two cycle evaluators behind one component (decision 0438 §5: elevators are "the same
/// subsystem, second consumer" — the rider frame, the worldport spare, and the instruments all
/// key on [`Transport`] and never care which drive is underneath).
enum Drive {
    /// Type 15: the client-computed taxi-path timetable.
    Taxi(Arc<TransportTimetable>),
    /// Type 11: the authored `TransportAnimation` keyframe path.
    Lift(Lift),
}

/// **No map field, deliberately** (decision 1654). A lift's map is not a datum it owns: vmangos
/// hands a player the whole map's transport set on entry (`Map::SendInitTransports`) and takes it
/// back on the way out, so a type-11 is only ever resident for someone standing on its own map —
/// the car is always on the map *you* are on. 0611 stamped `CurrentMap` here at arm time, which in
/// the login frame is the startup seed (`arm_transports` runs a stage before `player::wire_in`
/// writes the real map), and the off-map hide below then hid every lift on Kalimdor for the whole
/// session — the director's Thunder Bluff report, and B168's Gnomeregan lift.
struct Lift {
    frames: Arc<Vec<ElevatorKeyframe>>,
    period_ms: u32,
    base_pos: [f32; 3],
    quat: [f32; 4],
    /// Constant rendered heading (wire orientation, WoW radians).
    yaw: f32,
}

impl Transport {
    /// Which drive is underneath — for the instruments only (`WOW_LIFT_CENSUS`); no consumer
    /// keys on it (decision 0438 §5: the drive is private by design).
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

    /// The transport's live cycle position for an anchor — the rider math needs the same
    /// number the tick used, so it lives here.
    pub(crate) fn cycle_ms(&self, anchor: &TransportAnchor) -> u32 {
        let progress = u64::from(anchor.progress_ms) + anchor.at.elapsed().as_millis() as u64;
        (progress % u64::from(self.period_ms().max(1))) as u32
    }

    /// Re-anchor this transport's clock so its cycle sits at the head of `map_id`'s leg, and
    /// return the sample there **plus the cycle instant it was moved to, if it moved at all**.
    /// `None` if the drive has no leg on that map (a lift never does).
    ///
    /// The anchor is stored as `progress_ms` + the instant it was taken, so seeking is simply
    /// "restamp both": the clock runs on from the new cycle position exactly as it would have.
    ///
    /// **The second element must be how the caller decides whether anything happened.** The stored
    /// `progress_ms` is the server's own uptime-scale path progress until a seek replaces it —
    /// unbounded, and nothing like a cycle position — so "did it change?" cannot be answered by
    /// comparing it against a `% period` cycle value. Doing exactly that made the healthy
    /// already-agreed crossing print `our path clock was 157334 ms behind` with a target of
    /// 35081741 ms, which is 9.7 hours: the server's uptime, not a position in a 350 s loop.
    pub(crate) fn reseek_to_map(
        &self,
        anchor: &mut TransportAnchor,
        map_id: u32,
    ) -> Option<(TransportSample, Option<u32>)> {
        let Drive::Taxi(t) = &self.drive else {
            return None;
        };
        let cycle = self.cycle_ms(anchor);
        // **Already there — leave the clock alone.** The healthy crossing is the common case, and
        // seeking anyway would rewind the cycle to the head of the frame we are mid-way through,
        // throwing away up to a leg of progress and jerking the deck backwards under the rider.
        // Only a clock that actually disagrees with the server gets corrected.
        let here = t.sample(cycle);
        if here.map == map_id {
            return Some((here, None));
        }
        let target = t.first_cycle_on_map(cycle, map_id)?;
        anchor.progress_ms = target;
        anchor.at = Instant::now();
        Some((t.sample(target), Some(target)))
    }

    /// Whether any leg of this transport's cycle lies on `map_id` — the cross-map worldport's
    /// spare predicate (decision 0455). **A lift always answers no** (1654): the spare exists so a
    /// *ridden boat* survives the seam mid-cycle, and a lift never spans one — the server re-sends
    /// the destination map's whole transport set on arrival (`SendInitTransports`), so a lift
    /// despawns and streams back like every other object.
    pub(crate) fn touches_map(&self, map_id: u32) -> bool {
        match &self.drive {
            Drive::Taxi(t) => t.touches_map(map_id),
            Drive::Lift(_) => false,
        }
    }

    /// The cycle sample — position/heading/map/motion at `cycle_ms`, both drives. `here` is the
    /// map the viewer is on: a **taxi** ignores it (its timetable knows which continent each leg
    /// lies on, which is the whole point of the off-map hide), and a **lift** reports it, because
    /// a type-11 exists only for players standing on its own map. That is the honest answer to
    /// "which map is this position expressed in", and it is what [`Lift`] refuses to store.
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

    /// The live sample for an anchor — the same pose the tick writes this frame. For
    /// instruments (the crossing probe reads dock state + the deck drop point off it).
    pub(crate) fn sample_at(&self, anchor: &TransportAnchor, here: u32) -> TransportSample {
        self.sample(self.cycle_ms(anchor), here)
    }
}

/// The transport path data: `TaxiPathNode.dbc` + the per-path timetables built from it, shared
/// across instances (the Menethil boat and its return-leg twin share one build; `None` in
/// `built` = a failed build, remembered so it isn't retried every frame) — and the
/// `TransportAnimation.dbc` keyframe catalog for the type-11 lifts, shared the same way (the
/// two Mesa cars of one entry share one `Arc`).
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
        // No taxi data ⇒ boats render frozen at their create pose — degraded, not broken.
        Err(e) => warn!("taxi path nodes unavailable, transports stay put: {e:#}"),
    }
    match load_elevator_paths(&mut chain) {
        Ok(paths) => {
            info!("transport animations: {} lift paths", paths.len());
            timetables.elevators = Some(paths);
        }
        // Same degradation as the boats: no keyframes ⇒ cars park at their spawn point.
        Err(e) => warn!("TransportAnimation unavailable, lifts stay put: {e:#}"),
    }
}

/// Arm anchored GameObjects: a **boat** (type 15) once its template answer (the taxi tuple)
/// lands — build (or reuse) the path's timetable; a **lift** (type 11, [`ElevatorSeed`]) as
/// soon as the keyframe catalog is open — its create block already carried everything else.
/// The query is a handful of entities at most (the map's transports), so polling is free; the
/// boat's ask-once template request was already made at create (`go_templates.request` in the
/// apply arm).
///
/// **The lift arm reads no world state at all** (1654): at login every type-11 on the map arms in
/// the same drain that carries `SMSG_LOGIN_VERIFY_WORLD`, one stage before `player::wire_in`
/// writes `CurrentMap` — so anything captured from the world *here* is captured stale.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn arm_transports(
    mut commands: Commands,
    mut timetables: ResMut<Timetables>,
    templates: Res<GameObjectTemplates>,
    // `Or<…, With<ElevatorSeed>>`: a re-created lift is already armed (`Transport` present) but
    // carries a fresh seed from the new create block — it re-enters here so the seed is consumed
    // (a fresh, identical arm) instead of lingering. Boats never carry a seed, so for them the
    // filter is exactly `Without<Transport>` as before.
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
        // — the lift arm (decision 0438 phase 3's second consumer) —
        if let Some(seed) = seed {
            let Timetables {
                elevators, lifts, ..
            } = &mut *timetables;
            let Some(paths) = elevators.as_ref() else {
                continue; // no client data — stays frozen
            };
            let Some(frames) = paths.entry(seed.entry) else {
                // No authored path for this entry: park it at its spawn pose for good (the
                // real client's behaviour for a pathless type-11). The anchor goes too, so
                // the entity leaves this query instead of re-polling as a mistaken boat — and
                // the spawn-hidden state lifts (anchored creates spawn `Hidden` until their
                // first ticked pose, which a parked car never gets).
                warn!(
                    "transport: no TransportAnimation path for entry {} (guid {:#x}) — parked",
                    seed.entry, guid.0
                );
                commands
                    .entity(entity)
                    .remove::<(ElevatorSeed, TransportAnchor)>()
                    .insert(Visibility::default())
                    // The one insert-based visibility flip: `Visibility`'s require chain just
                    // re-added the sweep row a net root sheds at spawn — strip it again
                    // (benilla_world::vis_chain).
                    .vis_chain_only();
                continue;
            };
            let frames = lifts
                .entry(seed.entry)
                .or_insert_with(|| Arc::new(frames.to_vec()))
                .clone();
            let period_ms = elevator_period_ms(&frames);
            // A wire that carried no rotation quat falls back to the pure-yaw quat of the
            // spawn facing — the identical encoding every live 1.12 spawn row uses.
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

        // — the boat arm —
        let Some(mo) = templates.get(guid.0).and_then(|t| t.mo_transport) else {
            continue; // template answer not in yet — next frame
        };
        let Timetables { nodes, built, .. } = &mut *timetables;
        let Some(nodes) = nodes.as_ref() else {
            continue; // no client data — stays frozen
        };
        let timetable = built
            .entry(mo.taxi_path_id)
            .or_insert_with(|| {
                let t = nodes.path(mo.taxi_path_id).and_then(|path| {
                    // The build self-pins its cycle length to the real client's own bookkeeping
                    // (`0x5f4cc0`, bit-exact against all nine server-sniff periods), so the
                    // period on the label IS the server's.
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
            // Kinematic, not the GO-attach default of Static: this body moves every frame, and
            // avian rebuilds the kinematic collider tree per step by design (the static tree is
            // incremental, tuned for immobile geometry). The mover's casts see it either way —
            // spatial queries traverse all trees — but the label matches the motion.
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

/// The per-frame transport tick — the client's own `(progress + anchor) % period` leg walk
/// (`0x5f50a0`, decision 0438): sample the timetable, write the pose. Translation +
/// rotation only — the renderer bakes model scale into the transform (`write_pose`'s law).
/// Off-map samples (the boat is sailing the other continent's leg) hide the model; the server
/// removes the GO around the same time, so this is belt-and-braces for the transition frames.
/// **A lift can never trip it** (decision 1654): its sample is expressed in the map passed in, so
/// the term is a boat's alone — which is what it always meant, and what 0611's stamped map broke.
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
        // Write-on-change only, on every component here (1473): a docked leg samples the
        // identical pose each frame, and the unconditional writes this replaces re-dirtied the
        // deck's whole subtree — transform propagation over every submesh plus a `Visibility`
        // re-propagation — for every docked elevator on the map, every frame.
        // Boats only, by construction — see this system's doc.
        if sample.map != current_map.0 {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        visibility.set_if_neq(Visibility::Inherited);
        write_deck_pose(&sample, &mut transform, position, rotation);
    }
}

/// Write one sampled transport pose into the render transform **and** avian's own components.
///
/// The avian mirror is not optional: its `Transform`→`Position` sync runs in the physics schedule
/// (before `Update`), so without it the mover's same-frame casts see the deck one frame behind —
/// ~0.5 yd astern at cruise. Since 1663 the chain republishes the moved collider's AABB inline
/// too, so a pose written here is fully visible to this frame's queries however far it jumped.
///
/// Write-on-change on every component (1473): a docked leg samples the identical pose each frame,
/// and unconditional writes re-dirty the deck's whole subtree — transform propagation over every
/// submesh plus a `Visibility` re-propagation — for every docked elevator on the map, every frame.
///
/// **Takes `&mut Mut<Transform>`, not `&mut Transform`, and that is the whole point of the
/// signature.** Reaching through `Mut` with `DerefMut` sets the change flag unconditionally, so a
/// helper that took a plain `&mut Transform` would re-dirty the deck's subtree on every call —
/// exactly the per-frame propagation 1473 removed, reintroduced by the act of factoring the write
/// out of its loop. The existing `a_docked_lift_stops_dirtying_its_transform_and_visibility` test
/// catches it; this comment is here so the next factoring does not have to rediscover why.
fn write_deck_pose(
    sample: &TransportSample,
    transform: &mut Mut<Transform>,
    position: Option<Mut<Position>>,
    rotation: Option<Mut<Rotation>>,
) {
    let translation = benilla_assets::coords::wow_to_bevy(sample.pos);
    let heading = Quat::from_rotation_y(sample.heading);
    // Read through `Deref` (no flag), write through `DerefMut` (flag) — only when it changed.
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

/// **Put the ridden transport on the destination map in the worldport frame itself** (decision
/// 2026), so the rider's world pose is not composed through the source continent's coordinates.
///
/// **This exists because of a divergence of ours, not a gap in the server.** The server does send
/// a fresh create for the boat we are standing on: `Map::SendInitSelf` builds it *first*
/// (`Map.cpp:1698-1702`), which is the only reason `SendInitTransports` then skips
/// `player->GetTransport()` (`Map.cpp:1719`) — a skip that reads like an omission and is not one.
/// That create re-anchors the clock exactly. But it lands a round trip *after* the worldport ack,
/// and decision 0455's spared transport is asked for a pose in the worldport frame itself, before
/// it. This fills that window; the server's create then supersedes it.
///
/// **The reference has no such window, because it keeps nothing.** `SMSG_NEW_WORLD` (`0x401bc0`)
/// destroys every object in the manager (`0x467700` → `0x467800`) and builds a fresh one already
/// set to the destination map (`0x464ff0`), then rebuilds both rider and transport from the
/// post-ack CREATE blocks; the worldport composes nothing at all, and streams around the raw wire
/// pose. Our spare — and therefore this — is the price of not doing that. Whether to adopt the
/// reference's purge-and-rebuild wholesale is the recorded follow-on (2026).
///
/// **Why this system writes the pose itself instead of leaving it to [`tick_transports`].**
/// `CurrentMap` is still the *source* map this frame — `player::wire_in` inserts the new one, and
/// a command insert lands at the next flush — so `tick_transports` would compare the freshly
/// sought sample against the old map, take its off-map arm, and hide the boat with its transform
/// unwritten. This system knows the destination map explicitly from the message, so it needs no
/// resource at all. From the next frame `tick_transports` agrees and takes over.
///
/// **Order matters and is enforced by the chain:** this runs before `tick_transports` (which must
/// not undo it) and both run `.before(WorldStage::Input)`, so `wire_in` composes the rider's world
/// pose through a transform that is already the destination continent's. Without that, the rider
/// is composed through the source continent's coordinates — off the destination's WDT grid, where
/// nothing streams, the loading cover cannot clear and there is no ground under the body.
#[allow(clippy::type_complexity)] // one Bevy system's full input set — as `tick_transports`
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
        // Only a transfer the server carried us through on a transport, and only while we are in
        // fact still on one — the same predicate `wire_in`'s riding branch uses.
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
        // Silent when we were already there (the healthy crossing — the two clocks agreed and the
        // seek was a no-op, which `moved_to == None` is the ONLY sound way to detect). Loud when it
        // moved, carrying **how far behind the server our own path clock was**: this is the only
        // measurement of client-vs-server transport drift the client can take.
        //
        // Measured **forward around the cycle**, because a crossing is very often the cycle wrap
        // (on taxi path 241 the Kalimdor legs are the *first* frames, so "cross to Kalimdor" IS
        // the wrap): a plain `after - before` reports a 144 ms nudge as −356 140 ms and reads as
        // the clock running half a cycle ahead, which is the opposite of what happened.
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

/// Frames the ridden transport's map may disagree with [`CurrentMap`] before it is a fault rather
/// than the one-frame seam artefact — see [`watch_ridden_transport_map`]. Three, matching the
/// loading screen's own `CLEAR_AFTER_READY_FRAMES` debounce: long enough to outlast any single
/// deferred resource write, short enough that a real stall is named while it is still on screen.
const DISAGREEMENT_GRACE_FRAMES: u32 = 3;

/// **Tripwire: the boat we are STANDING ON says it is on another map.**
///
/// [`tick_transports`] leaves an off-map transport's `Transform` unwritten (and hides it) — for a
/// boat sailing the other continent's leg that is right, and for a boat we are *riding* it is the
/// one state that must never happen. Its pose is what [`crate::player::wire_in`] composes our own
/// world position from at a cross-map worldport (decision 0455), so a frozen source-map pose puts
/// the body at *that continent's* coordinates on *this* map: off the destination's WDT grid, where
/// nothing streams, the loading cover cannot clear (`total == 0` never reads ready) and there is no
/// ground to stand on. That is the 2026-09-05 report — a minute of loading screen on the
/// Booty Bay ferry, then a fall through the world.
///
/// It arises from our own path clock disagreeing with the server's about *when* the crossing
/// happens — measured live at 32 ms and 144 ms on the Orgrimmar zeppelin, in the two directions.
/// One line per sustained entry into the state, with the numbers needed to size it.
///
/// **A single frame of it is normal and must not warn.** `CurrentMap` is written by
/// `player::wire_in` with a command insert, so it lands one frame after the worldport that
/// [`reseek_ridden_transport_at_worldport`] handled — for that one frame the freshly sought
/// transport legitimately reads the *new* map while the resource still reads the old, which is
/// the inverse of the pathology and fired on every healthy crossing before this grace existed.
/// Hence a consecutive-frame count: only a state that *persists* is the one worth a line.
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

/// An observed mover's pose in a transport's local frame — the `MOVEFLAG_ONTRANSPORT` tail of
/// its `LIVING` create block or `MSG_MOVE_*` relay (decision 0438 phase 2: "observed riders stop
/// discarding it"). [`compose_riders`] re-anchors the entity through the named transport's live
/// matrix every frame; the local pose itself only changes when the next packet lands (no
/// between-packet extrapolation in the deck frame yet — deck NPCs stand still, and a walking
/// rider corrects at its packet rate). Inserted/updated/removed by the net apply.
#[derive(Component)]
pub(crate) struct TransportRider {
    pub(crate) transport_guid: u64,
    /// Rider position in the transport's local frame, raw WoW coords (as on the wire).
    pub(crate) local_pos: [f32; 3],
    /// Rider facing in the transport's local frame (WoW radians) — world facing is
    /// `local + transport yaw` (the GetAbsoluteFacing law).
    pub(crate) local_orientation: f32,
}

/// Compose every observed rider's world pose through its transport's this-frame matrix — chained
/// after [`tick_transports`], so the deck carries its riders in the same frame it moves. Also
/// refreshes [`RemoteMotion`]'s canonical WoW pose so every downstream consumer (nameplates, the
/// animation selector, the landing predictor) sees the carried position; the extrapolator ran
/// earlier (WorldStage::Net) and this simply wins the frame. A rider whose boat isn't streamed
/// in (or is off-map) keeps its last relayed world pose — the server despawns the pair together.
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

/// **Re-ground a deck-splined rider onto the deck it is walking on** (decision 1936's correction).
///
/// The tempting reading — "a transport spline's Z is authoritative, because there is no terrain
/// under a boat" — is what benilla shipped for a few hours, and the reference refutes it. The
/// transport guid appears in **neither** `0x616cb0`'s predicate **nor** `0x634040`'s dispatch: a
/// grounded `SMSG_MONSTER_MOVE_TRANSPORT` spline takes the *same* fork as a plain one,
/// `0x616d03` zeroes its Z-delta and the WALK resolver `0x6367b0` re-derives Z off the world trace
/// (`0x636e52`: `pos.z -= hitDistance`). The reason that does not drop the unit into the sea is
/// that the query runs at the **composed world position**, and the class mask `0x6315f0` yields
/// `0x100111` whose `0x100000` bit unconditionally admits GameObject meshes — a boat's `.wmo`
/// being in the global map-object list precisely because it is a type-11/15 GameObject.
///
/// **So the deck's grounding is the ordinary grounding, moved one step later.** It cannot live in
/// [`crate::net::ground_clamp_creatures`] with every other unit's, because that system runs inside
/// `WorldStage::Net` — *before* [`compose_riders`] — where a deck rider's `Transform` is still
/// last frame's composed pose and its fresh position exists only as a transport-local offset. Here
/// the deck is at its this-frame pose, its collider AABB has just been republished
/// ([`republish_moved_collider_aabbs`], decision 1663 — without which the probe would be pruned
/// against the deck's previous-frame box) and the rider's world position is on the transform. One
/// probe, at the reference's own moment.
///
/// The corrected Y is written to the `Transform` and not back into
/// [`TransportRider::local_pos`], deliberately: [`compose_riders`] recomputes the world pose from
/// the local one every frame, so a write-back would be re-derived away next frame anyway, and the
/// correction is idempotent as it stands. [`crate::net::RemoteMotion`]'s canonical pose moves with
/// it, as it does in `compose_riders`.
///
/// **A named gap.** The reference has a *third* regime this does not model: `0x616af0`'s anti-warp
/// guard snaps `pos := base + sampledOffset` verbatim — wire Z included — and bypasses the
/// re-ground entirely whenever the horizontal step exceeds 3 yd (`[0x80c5bc]` = 9.0, squared) or
/// 60 yd/s. For a creature freshly placed on a deck that is the common *first* substep, so our
/// first frame re-grounds where the reference would take the wire Z. It settles to the same place
/// on the next frame either way.
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
    /// The probe's reach above and below the rider's seat — the creature clamp's own numbers
    /// (`GROUND_CLAMP_UP`/`_DOWN`), for the same reason: enough to clear a slightly-high server Z
    /// and to follow a step down, not enough to grab a deck one storey up.
    const UP: f32 = 2.5;
    const DOWN: f32 = 4.0;
    for (mut transform, motion, spline) in &mut riders {
        // Only a GROUNDED deck path. A flying one owns its altitude (the `0x200` bit is the same
        // one that decides the point encoding and the resolver arm), and a world-frame spline is
        // `ground_clamp_creatures`' business, not ours.
        if spline.deck.is_none() || !spline.grounded {
            continue;
        }
        let origin = transform.translation + Vec3::Y * UP;
        let Some(hit) = world.ray_body(origin, Dir3::NEG_Y, UP + DOWN) else {
            continue; // nothing under the rider — leave the composed pose alone
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

/// Publish the **ride frame** onto every model standing on a transport — the reference's
/// `SetMoveBase` install (`0x617170`/`0x618970` → `0x718910` → `[CM2Model+0x17c]`), which is the
/// frame a rider's world-space effects are *stored* in ([`benilla_world::ride_frame`]).
///
/// Two riders, one component. The **body we steer** takes the mover's own platform attach
/// ([`crate::player::Player::ride_entity`] — the deck collider that grounded us), and every
/// **observed** rider takes the wire's transport tail ([`TransportRider`]). Everything hung off
/// either — a held weapon, its enchant streamer, a spell kit — inherits it through the
/// `ParentModel` chain rather than being stamped itself, which is exactly how the reference
/// propagates `[model+0x17c]` to a model's children every frame (`0x7142c1` in the animate
/// kernel `0x714260`).
///
/// Why this is a component and not a field on `Transport`: the fact is *per rider*, it changes on
/// a step, and `benilla-world` — which owns the emitters that consume it — cannot see either
/// `Player` or `TransportRider`. Decision 1591 named this seam and left it unbuilt; the director's
/// weapon-trail report on the Thunder Bluff lift is what it costs.
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
        // A rider whose transport isn't streamed in keeps no frame: there is no pose to store
        // against, and the server despawns the pair together anyway (`compose_riders`' note).
        if let Some(&deck) = guid_index.0.get(&on.transport_guid) {
            want.insert(rider, deck);
        }
    }
    // Reconcile rather than re-stamp: an unchanged rider must not touch its component at all, so
    // change detection stays quiet on a deck full of passengers.
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

    /// The Ratchet–Booty Bay ferry's real timetable (taxi path 241) — the seam the 2026-09-05
    /// report rode. Real data on purpose: a hand-built path is not a substitute here, because the
    /// builder's map-change skip dance plus the client period pin can silently truncate a short
    /// synthetic cycle so the far continent's legs are never reachable, and a test written against
    /// that proves nothing about the code under test.
    fn ferry_241(chain: &mut benilla_formats::Chain) -> Transport {
        let cat = benilla_formats::load_taxi_path_nodes(chain).expect("TaxiPathNode.dbc");
        let nodes = cat.path(241).expect("taxi path 241 (Ratchet–Booty Bay)");
        let tt = TransportTimetable::build(nodes, 30.0, 1.0).expect("path 241 builds");
        Transport {
            drive: Drive::Taxi(Arc::new(tt)),
            was_moving: false,
        }
    }

    /// **The seam re-anchor moves the clock onto the destination map — and only when it has to**
    /// (decision 2026). Composing the rider through a transport still sampling the *source*
    /// continent is what put a body at Booty Bay's coordinates on Kalimdor, off the destination's
    /// tile grid, where nothing streams and the loading cover cannot clear.
    ///
    /// Path 241 is the case that matters and the reason this is not a synthetic test: its Kalimdor
    /// legs are the *first* frames of the cycle, so "cross to Kalimdor" is the **cycle wrap**, not
    /// an interior map change. A seek that only handled the interior case would leave exactly the
    /// reported ferry broken.
    #[test]
    fn the_seam_reanchor_lands_on_the_new_map_and_leaves_an_agreeing_clock_alone() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let transport = ferry_241(&mut chain);
        let period = transport.period_ms();

        // Ask the built timetable where each continent's legs actually fall rather than assuming.
        let on = |map: u32| {
            (0..period)
                .step_by(500)
                .find(|&ms| transport.sample(ms, 0).map == map)
                .unwrap_or_else(|| panic!("path 241 has an instant on map {map}"))
        };
        let (on_azeroth, on_kalimdor) = (on(0), on(1));

        // Standing on Azeroth's leg, told we are now on Kalimdor: the clock moves, and it moves
        // onto Kalimdor — the stored anchor, not merely the returned sample.
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

        // Already there and told so: the clock is left exactly as it was. Seeking anyway would
        // rewind to the head of the frame we are mid-way through and jerk the deck backwards
        // under the rider — the healthy crossing is the common case and must cost nothing.
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
        // **And it must SAY it did nothing.** The stored anchor is the server's uptime-scale path
        // progress until a seek replaces it, so a caller cannot infer "unchanged" by comparing it
        // against a cycle value — it reported a 9.7-hour raw progress as a 157 s clock skew until
        // this flag existed.
        assert_eq!(
            moved_to, None,
            "a no-op seek must report that it did not move the clock"
        );

        // A map the path never visits has no answer — the caller warns rather than inventing one.
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

    /// A **lift** is never re-anchored across a seam (1654): a type-11 exists only for players on
    /// its own map, it is despawned and re-created like any other object, and it has no taxi path
    /// to seek along at all.
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

    /// A two-frame pause path at `base_pos` — the same lift both tests below drive.
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

    /// A docked lift goes QUIET: `tick_transports` writes pose and visibility on change only
    /// (1473) — the unconditional writes it replaces re-dirtied every docked elevator's whole
    /// subtree (transform + `Visibility` propagation over all its submeshes), map-wide, every
    /// frame. A two-frame pause path samples the identical pose forever, so after the first
    /// arming write nothing may be marked changed.
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

    /// **A lift is never hidden by the off-map term, whatever `CurrentMap` says** (decision 1654).
    /// The off-map hide is the boat's: a ferry mid-cycle on the other continent's leg. 0611 gave
    /// the lift a `map` stamped from `CurrentMap` at arm time — and `arm_transports` runs a stage
    /// *before* `player::wire_in` writes that resource, so at login every type-11 on the map armed
    /// against the startup seed and the term hid all of them for the session (Thunder Bluff; B168).
    /// Driving the tick on a map the arm never saw is exactly that shape, and the car must show.
    #[test]
    fn a_lift_is_visible_on_whatever_map_the_viewer_is_on() {
        let mut app = App::new();
        // Alterac Valley — a real 1.12 map that carries no type-11 GO at all, so it is a map no
        // lift could ever have been stamped with. The point is that the value cannot matter.
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
                // As the net apply spawns an anchored transport: hidden until its first ticked
                // pose. If the tick declines to unhide it, the car is solid and invisible.
                Visibility::Hidden,
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(lift),
            Some(&Visibility::Inherited),
            "the tick must unhide a lift on the viewer's map, not judge it against a stamped one"
        );
        // And the pose it wrote is the car's own — base + its (zero) local offset, in Bevy axes.
        assert_eq!(
            app.world().get::<Transform>(lift).map(|t| t.translation),
            Some(benilla_assets::coords::wow_to_bevy([10.0, 20.0, 30.0])),
        );
    }
}
