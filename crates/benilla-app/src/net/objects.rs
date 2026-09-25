//! The bridge's object layer: the streamed world's creates, deltas, moves and destroys, movers'
//! speeds and modes, GameObject templates and anims, and items. Each packet is one handler over
//! [`Scene`] whose commands apply before the next packet's, so a create's store and speeds are
//! live components by the time the next packet reads them.
use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::{
    guid, EntityKind, MonsterMoveFacing, MoveSpeeds, ObjectFields, SpeedKind, SplineMode,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use benilla_world::model_fade::DespawnFade;
use benilla_world::vis_chain::VisChainOnly;

use super::motion::{
    create_spline, gameobject_rotation, monster_move_spline, pose_transform, resolve_facing,
    trace_create_spline, trace_move_snap, wire_yaw, write_pose, SplineStopped, ROOT_APPLY_WIPE,
};
use super::{
    merge_store_fields, FieldChanged, Guid, GuidIndex, NetCommands, NetEntity, NetHandlerApp,
    ObjectStore, RemoteMotion, SelfGuid, SpeedChangeMessage, Spline, UnitMoveModes, UnitSpeeds,
};

/// `SMSG_GAMEOBJECT_CUSTOM_ANIM`, bridged to [`crate::go_anim`], which rejects `anim_id >= 4` and
/// maps the rest to AnimationData 153..156; the fishing bobber's splash is `anim_id` 0.
fn gameobject_custom_anim(
    guid: u64,
    anim_id: u32,
    plays: &mut MessageWriter<crate::go_anim::GoCustomAnim>,
) {
    debug!("net: gameobject {guid:#x} custom anim {anim_id}");
    plays.write(crate::go_anim::GoCustomAnim {
        go_guid: guid,
        anim_id,
    });
}

/// `SMSG_GAMEOBJECT_DESPAWN_ANIM`: AnimationData 157 on the one-shot channel (`0x5f8c50`).
/// vmangos sends `SMSG_DESTROY_OBJECT` in the same tick, so the mark is a component queued now,
/// ahead of the closure [`object_destroyed`] queues, not a message.
fn gameobject_despawn_anim(guid: u64, commands: &mut Commands, index: &GuidIndex) {
    debug!("net: gameobject {guid:#x} despawn anim");
    if let Some(&e) = index.0.get(&guid) {
        commands
            .entity(e)
            .insert(crate::go_anim::DespawnAnimAnnounced);
    }
}

/// Registers the object layer's handlers.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::ObjectCreate, on_object_create)
        .net_handler(K::ItemCreate, on_item)
        .net_handler(K::ItemTemplate, on_item)
        .net_handler(K::ItemTime, on_item)
        .net_handler(K::ItemEnchantTime, on_item)
        .net_handler(K::ObjectMove, on_object_move)
        .net_handler(K::MoveTimeSkipped, on_move_time_skipped)
        .net_handler(K::UnitMove, on_unit_move)
        .net_handler(K::ObjectValues, on_object_values)
        .net_handler(K::ObjectDestroyed, on_object_destroyed)
        .net_handler(K::ObjectsRemoved, on_objects_removed)
        .net_handler(K::MonsterMove, on_monster_move)
        .net_handler(K::GameObjectInfo, on_gameobject_info)
        .net_handler(K::GameObjectCustomAnim, on_gameobject_anim)
        .net_handler(K::GameObjectDespawnAnim, on_gameobject_anim)
        .net_handler(K::SplineMoveMode, on_spline_move_mode)
        .net_handler(K::ForceSpeedChange, on_speed)
        .net_handler(K::SpeedChanged, on_speed);
}

/// The streamed world as one parameter: the components, caches, lifecycle hooks and edges the
/// object packets touch.
#[derive(SystemParam)]
pub(crate) struct Scene<'w, 's> {
    commands: Commands<'w, 's>,
    net: Res<'w, NetCommands>,
    index: ResMut<'w, GuidIndex>,
    self_guid: Res<'w, SelfGuid>,
    real: Res<'w, Time<Real>>,
    transforms: Query<'w, 's, &'static mut Transform>,
    stores: Query<'w, 's, &'static mut ObjectStore>,
    countdowns: Query<'w, 's, &'static mut crate::items::Countdowns>,
    remote: Query<'w, 's, &'static mut RemoteMotion>,
    modes: Query<'w, 's, &'static mut UnitMoveModes>,
    riders: Query<'w, 's, &'static mut crate::transport::TransportRider>,
    speeds: Query<'w, 's, &'static mut UnitSpeeds>,
    field_changes: MessageWriter<'w, FieldChanged>,
    speed_changes: MessageWriter<'w, SpeedChangeMessage>,
    self_moves: MessageWriter<'w, super::SelfMoveMessage>,
    hard_landings: MessageWriter<'w, crate::creature_anim::HardLanding>,
    go_custom_anims: MessageWriter<'w, crate::go_anim::GoCustomAnim>,
    names: Res<'w, NameCache>,
    items: ResMut<'w, Items>,
    go_templates: ResMut<'w, GameObjectTemplates>,
    death_net: ResMut<'w, crate::death::DeathNet>,
    group: ResMut<'w, crate::ui_party::GroupState>,
}

fn on_object_create(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectCreate {
        guid,
        kind,
        display_id,
        position,
        orientation,
        scale,
        speeds,
        mover,
        transport_progress,
        transport,
        spline,
        fields,
    } = ev
    {
        crate::death::net::note_corpse(guid, kind, &fields, &sc.self_guid, &mut sc.death_net);
        object_create(
            guid,
            kind,
            display_id,
            position,
            orientation,
            scale,
            speeds,
            mover,
            transport_progress,
            transport,
            spline,
            fields,
            &mut sc.commands,
            &mut sc.index,
            &mut sc.transforms,
            &mut sc.stores,
            &mut sc.field_changes,
            &sc.names,
            &sc.go_templates,
            &sc.net,
        );
    }
}

/// Items: a descriptor-only create, `SMSG_ITEM_QUERY_SINGLE_RESPONSE` (a miss is cached as `None`
/// and never re-asked), and the lifetime and enchant countdowns, written into the item's own
/// [`crate::items::Countdowns`].
fn on_item(In(ev): In<SessionEvent>, mut sc: Scene) {
    match ev {
        SessionEvent::ItemCreate {
            guid,
            container,
            fields,
        } => item_create(
            guid,
            container,
            fields,
            &mut sc.commands,
            &mut sc.index,
            &mut sc.stores,
            &mut sc.field_changes,
            &mut sc.items,
        ),
        SessionEvent::ItemTemplate { entry, info } => {
            let info = info.map(|b| *b);
            debug!("net: item template {entry} → {info:?}");
            sc.items.insert_template(entry, info);
        }
        // `0x5e4f30`'s two arms share the item lookup (`0x468460`, `TYPEMASK_ITEM`) and differ on
        // a miss.
        SessionEvent::ItemTime { item_guid, seconds } => {
            // An unheld item drops the update, with no queue (`0x1EA`'s arm returns), so an item
            // created after the login re-send (`Player::SendItemDurations`) waits for the next.
            match sc.index.0.get(&item_guid).copied() {
                Some(e) if sc.countdowns.contains(e) => {
                    if let Ok(mut c) = sc.countdowns.get_mut(e) {
                        c.set_lifetime(seconds);
                    }
                    debug!("item duration: item {item_guid:#x} → {seconds}s");
                }
                _ => debug!("item duration: {item_guid:#x} names an item we do not hold — dropped"),
            }
        }
        SessionEvent::ItemEnchantTime {
            item_guid,
            slot,
            seconds,
        } => match sc.index.0.get(&item_guid).copied() {
            Some(e) if sc.countdowns.contains(e) => {
                if let Ok(mut c) = sc.countdowns.get_mut(e) {
                    if c.set_enchant(slot, seconds) {
                        debug!("enchant timer: item {item_guid:#x} slot {slot} → {seconds}s");
                    } else {
                        debug!("enchant timer: item {item_guid:#x} slot {slot} is past the seventh — refused");
                    }
                }
            }
            // `0x1EB`'s miss arm queues on a resolving player's list (`0x5ebd40`), and only the
            // active player's is replayed (`0x5d8440` → `0x5ebde0`). vmangos names the owner, us
            // (`Player.cpp:11883`, `:12077`), so it queues on the active player.
            _ if sc.self_guid.0.is_some_and(|g| sc.index.0.contains_key(&g)) => {
                sc.items.queue_enchant_time(item_guid, slot, seconds)
            }
            _ => debug!("enchant timer: {item_guid:#x} unheld and no player to queue on — dropped"),
        },
        _ => {}
    }
}

fn on_object_move(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectMove {
        guid,
        position,
        orientation,
    } = ev
    {
        object_move(
            guid,
            position,
            orientation,
            &mut sc.commands,
            &sc.index,
            &mut sc.transforms,
        );
    }
}

/// An observed mover skipped time: only its relay chain moves, as the reference's handler does
/// (`0x603b40` → `0x61ab90`, `[CMovement+0xac] += lag`); an unknown guid is dropped, as there.
fn on_move_time_skipped(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::MoveTimeSkipped { guid, lag_ms } = ev {
        if let Some(mut m) = sc
            .index
            .0
            .get(&guid)
            .and_then(|&e| sc.remote.get_mut(e).ok())
        {
            m.relay.skip_time(lag_ms);
        }
    }
}

fn on_unit_move(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::UnitMove {
        guid,
        position,
        orientation,
        flags,
        pitch,
        time,
        verb,
        fall_time,
        jump,
        transport,
    } = ev
    {
        let now_ms = sc.real.elapsed_secs_f64() * 1000.0;
        unit_move(
            guid,
            crate::net::motion::RelayMove {
                wire_ms: time,
                position,
                orientation,
                flags,
                pitch,
                fall_time,
                jump,
                transport,
                verb,
            },
            now_ms,
            &mut sc.commands,
            &sc.index,
            &sc.self_guid,
            &mut sc.remote,
            &mut sc.transforms,
            &mut sc.hard_landings,
            &mut sc.self_moves,
        );
    }
}

/// Our corpse's `CORPSE_FIELD_FLAGS` can flip to BONES under a live guid; the reclaim latch is
/// re-asked on that edge, as the reference's `FLAGS` handler does (`0x5d6d60`).
fn on_object_values(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectValues { guid, fields } = ev {
        crate::death::net::recheck_corpse(guid, &fields, &sc.self_guid, &mut sc.death_net);
        object_values(
            guid,
            fields,
            &mut sc.commands,
            &sc.index,
            &mut sc.stores,
            &mut sc.field_changes,
        );
    }
}

/// The party hook runs first: the reference's deactivate reads the descriptor about to go.
fn on_object_destroyed(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectDestroyed(guid) = ev {
        crate::death::net::forget_corpse(guid, &mut sc.death_net);
        let store = sc.index.0.get(&guid).and_then(|e| sc.stores.get(*e).ok());
        crate::ui_party::net::member_deactivated(guid, &mut sc.group, store, &sc.net);
        object_destroyed(guid, &mut sc.commands, &mut sc.index);
    }
}

/// OUT_OF_RANGE takes the same deactivate as DESTROY in the reference, so a member walking out of
/// range also gets the snapshot and `CMSG_REQUEST_PARTY_MEMBER_STATS`.
fn on_objects_removed(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectsRemoved(guids) = ev {
        for guid in &guids {
            let store = sc.index.0.get(guid).and_then(|e| sc.stores.get(*e).ok());
            crate::ui_party::net::member_deactivated(*guid, &mut sc.group, store, &sc.net);
        }
        objects_removed(guids, &mut sc.commands, &mut sc.index);
    }
}

fn on_monster_move(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::MonsterMove {
        guid,
        transport,
        start,
        spline_id,
        path,
        facing,
        stop,
        duration_ms,
        flying,
        run_mode,
    } = ev
    {
        let rooted = modes_of(guid, &sc.index, &sc.modes).rooted();
        monster_move(
            guid,
            transport,
            start,
            spline_id,
            path,
            facing,
            stop,
            duration_ms,
            flying,
            run_mode,
            rooted,
            &mut sc.commands,
            &sc.index,
            &mut sc.transforms,
            &mut sc.riders,
        );
    }
}

fn on_gameobject_info(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::GameObjectInfo {
        entry,
        type_id,
        display_id,
        name,
        data,
    } = ev
    {
        gameobject_info(
            entry,
            type_id,
            display_id,
            name,
            &data,
            &mut sc.go_templates,
        );
    }
}

fn on_gameobject_anim(In(ev): In<SessionEvent>, mut sc: Scene) {
    match ev {
        SessionEvent::GameObjectCustomAnim { guid, anim_id } => {
            gameobject_custom_anim(guid, anim_id, &mut sc.go_custom_anims)
        }
        SessionEvent::GameObjectDespawnAnim { guid } => {
            gameobject_despawn_anim(guid, &mut sc.commands, &sc.index)
        }
        _ => {}
    }
}

/// The observer movement-mode family: no ack.
fn on_spline_move_mode(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::SplineMoveMode { guid, mode, apply } = ev {
        spline_move_mode(
            guid,
            mode,
            apply,
            &mut sc.commands,
            &sc.index,
            &mut sc.modes,
            &mut sc.remote,
        );
    }
}

fn on_speed(In(ev): In<SessionEvent>, mut sc: Scene) {
    match ev {
        SessionEvent::ForceSpeedChange {
            guid,
            kind,
            counter,
            speed,
        } => force_speed_change(
            guid,
            kind,
            counter,
            speed,
            &sc.index,
            &mut sc.speeds,
            &sc.self_guid,
            &mut sc.speed_changes,
        ),
        SessionEvent::SpeedChanged { guid, kind, speed } => {
            speed_changed(guid, kind, speed, &sc.index, &mut sc.speeds)
        }
        _ => {}
    }
}

/// An `SMSG_UPDATE_OBJECT` create block: spawns or refreshes the entity, warms the ask-once
/// caches and seeds its descriptor store.
fn object_create(
    guid: u64,
    kind: EntityKind,
    display_id: Option<u32>,
    position: [f32; 3],
    orientation: f32,
    scale: f32,
    speeds: Option<MoveSpeeds>,
    mover: Option<benilla_protocol::MoverState>,
    transport_progress: Option<u32>,
    transport: Option<benilla_protocol::TransportPose>,
    spline: Option<benilla_protocol::CreateSpline>,
    fields: ObjectFields,
    commands: &mut Commands,
    index: &mut GuidIndex,
    transforms: &mut Query<&mut Transform>,
    stores: &mut Query<&mut ObjectStore>,
    edges: &mut MessageWriter<FieldChanged>,
    names: &NameCache,
    go_templates: &GameObjectTemplates,
    net_commands: &NetCommands,
) {
    let net = NetEntity {
        kind,
        display_id,
        scale,
    };
    // A transport's cycle anchor: the create block's `UPDATE_FLAG_TRANSPORT` u32 and when it
    // landed; a re-create re-anchors. The reference's per-frame tick (`0x630970`) runs types 15
    // (boats, a path-progress clock) and 11 (lifts, `time-since-create % period`,
    // `GameObject.cpp:246`).
    let go_type = (kind == EntityKind::GameObject).then(|| fields.gameobject_type_id());
    let transport_anchor = matches!(go_type, Some(11 | 15))
        .then_some(transport_progress)
        .flatten()
        .map(|progress_ms| crate::transport::TransportAnchor {
            progress_ms,
            at: std::time::Instant::now(),
        });
    // The placement, and on a type-11 lift the basis its keyframe offsets rotate through.
    let go_quat = (kind == EntityKind::GameObject)
        .then(|| fields.gameobject_rotation())
        .flatten();
    // A type-11 lift's seed: keyframes keyed by the entry, rotated by the quat, based at the
    // create position. One whose create carried no progress u32 has no clock and stays frozen.
    let elevator_seed = (matches!(go_type, Some(11)) && transport_anchor.is_some())
        .then(|| fields.object_entry())
        .flatten()
        .map(|entry| crate::transport::ElevatorSeed {
            entry,
            base_pos: position,
            yaw: orientation,
            quat: go_quat,
        });
    // A GameObject is placed by its quaternion. A mover's create block also seeds its flags
    // (`0x618c30`, merge mask `0x75a07dff`) and pitch (`0x7c6420`) before the model is built
    // (`0x613e10`), so a swimmer that streams in nose-down is drawn nose-down from the first frame.
    let placement = match kind {
        EntityKind::GameObject => gameobject_rotation(go_quat, orientation),
        _ => mover.map_or_else(
            || wire_yaw(orientation),
            |m| crate::creature_anim::swim_body_rotation(orientation, m.flags, m.pitch),
        ),
    };
    // A unit created on a transport: the living block's rider tail is its local pose, which
    // `compose_riders` carries through the boat each frame; the world `position` is the fallback.
    let rider = matches!(kind, EntityKind::Unit | EntityKind::Player)
        .then_some(transport)
        .flatten()
        .map(|t| crate::transport::TransportRider {
            transport_guid: t.guid,
            local_pos: [t.pos.x, t.pos.y, t.pos.z],
            local_orientation: t.orientation,
        });
    // Warm the name cache as a unit streams in, the reference's timing: its create path
    // (`0x5fb880`) queries every creature (`0x60afb0`, at `0x60b157`), whose name is the record's
    // field 0 (`0x60934b`). Its `CreatureCache.wdb` answers without the wire; ours lasts a session.
    if matches!(kind, EntityKind::Unit | EntityKind::Player) {
        let _ = names.resolve(guid, net_commands);
        // A pet guid carries a pet number, so `resolve` asks the pet-name query; the template is
        // asked by the descriptor's entry as well, the reference's cache key (`[[unit+8]+0xc]`).
        if let Some(entry) = fields.object_entry().filter(|&e| e != 0) {
            let _ = names.resolve_creature(entry, guid, net_commands);
        }
    }
    // Warm the template cache as a GameObject streams in: only the query carries its lockId,
    // which decides use-vs-cast on a right-click.
    if matches!(kind, EntityKind::GameObject) {
        go_templates.request(guid, net_commands);
        // Where and how big, under `RUST_LOG=benilla_app::net::objects=debug`.
        debug!(
            "gameobject spawn: entry {:?} display {display_id:?} type {go_type:?} \
             pos [{:.2}, {:.2}, {:.2}] scale {scale}",
            fields.object_entry(),
            position[0],
            position[1],
            position[2],
        );
    }
    // The walk this unit is already on, joined at the server's progress. Traced first, so a
    // `WOW_CREATE_SPLINE=off` run still records what the wire offered.
    trace_create_spline(guid, spline.as_ref());
    let walk = spline.and_then(create_spline);
    if let Some(&e) = index.0.get(&guid) {
        // A re-create is a fresh snapshot: refresh identity and pose, drop any extrapolation.
        commands.entity(e).insert(net).remove::<RemoteMotion>();
        // The set is replaced whole; a `SMSG_FORCE_*_SPEED_CHANGE` in the same tick lands on top,
        // since this insert applies before the next packet's handler (`apply::seam_tests`).
        if let Some(s) = speeds {
            commands.entity(e).insert(UnitSpeeds(s));
        }
        if let Some(anchor) = transport_anchor {
            commands.entity(e).insert(anchor);
        }
        if let Some(seed) = elevator_seed {
            commands.entity(e).insert(seed);
        }
        match rider {
            Some(r) => {
                commands.entity(e).insert(r);
            }
            None => {
                commands
                    .entity(e)
                    .remove::<crate::transport::TransportRider>();
            }
        }
        // A create without a path says the unit stands still, so a stale path goes.
        match walk {
            Some(s) => {
                commands.entity(e).insert(s);
            }
            None => {
                commands.entity(e).remove::<Spline>();
            }
        }
        write_pose(commands, transforms, e, position, placement);
        // The reference refreshes a live guid in place, notifying field watchers like any delta.
        merge_fields(commands, stores, edges, e, guid, fields);
    } else {
        // A transport spawns hidden: its create pose is the spawn point, not where it is in its
        // cycle; the transport tick unhides it at the first sampled pose.
        let visibility = if transport_anchor.is_some() {
            Visibility::Hidden
        } else {
            Visibility::default()
        };
        let mut entity = commands.spawn((
            Guid(guid),
            net,
            pose_transform(position, placement),
            visibility,
        ));
        // Chain-only visibility: the root renders nothing itself, and culling flips its
        // `Visibility` through `Mut` writes, which do not re-add the sweep row.
        entity.vis_chain_only();
        if let Some(s) = speeds {
            entity.insert(UnitSpeeds(s));
        }
        if let Some(anchor) = transport_anchor {
            entity.insert(anchor);
        }
        if let Some(seed) = elevator_seed {
            entity.insert(seed);
        }
        if let Some(r) = rider {
            entity.insert(r);
        }
        if let Some(s) = walk {
            entity.insert(s);
        }
        // The seed is not merged: the reference does not notify on create.
        entity.insert(ObjectStore(fields));
        index.0.insert(guid, entity.id());
    }
}

/// A descriptor-only item or container create: an indexed entity with a store and no pose or
/// model; a re-create merges like a values delta.
fn item_create(
    guid: u64,
    container: bool,
    fields: ObjectFields,
    commands: &mut Commands,
    index: &mut GuidIndex,
    stores: &mut Query<&mut ObjectStore>,
    edges: &mut MessageWriter<FieldChanged>,
    items: &mut Items,
) {
    debug!("net: item create {guid:#x} (container: {container})");
    if let Some(&e) = index.0.get(&guid) {
        merge_fields(commands, stores, edges, e, guid, fields);
    } else {
        let e = crate::items::spawn_item(commands, index, guid, fields, container);
        // Replay the enchant times queued while it was not held (`0x5ebde0`), as of now.
        let queued = items.take_enchant_times(guid);
        if !queued.is_empty() {
            let mut countdowns = crate::items::Countdowns::default();
            for (slot, seconds) in queued {
                if !countdowns.set_enchant(slot, seconds) {
                    debug!("enchant timer: queued slot {slot} for {guid:#x} is past the seventh — refused");
                }
            }
            commands.entity(e).insert(countdowns);
        }
    }
}

/// An `SMSG_UPDATE_OBJECT` movement block: a new pose that supersedes any active path.
fn object_move(
    guid: u64,
    position: [f32; 3],
    orientation: f32,
    commands: &mut Commands,
    index: &GuidIndex,
    transforms: &mut Query<&mut Transform>,
) {
    if let Some(&e) = index.0.get(&guid) {
        commands.entity(e).remove::<Spline>();
        // A yaw only: the GameObjects that get one are transports, which their tick then owns.
        write_pose(commands, transforms, e, position, wire_yaw(orientation));
    }
}

/// A relayed `MSG_MOVE_*`. The reference schedules a remote's apply (`0x618c30`): the mover's
/// replay chain gives the packet a fire-time, a due move applies now and a later one queues for
/// `drain_pending_moves`, with dead-reckoning in between. `WOW_REMOTE_SNAP=1` applies at arrival.
fn unit_move(
    guid: u64,
    mv: crate::net::motion::RelayMove,
    now_ms: f64,
    commands: &mut Commands,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    remote_motion: &mut Query<&mut RemoteMotion>,
    transforms: &mut Query<&mut Transform>,
    landings: &mut MessageWriter<crate::creature_anim::HardLanding>,
    self_moves: &mut MessageWriter<crate::net::SelfMoveMessage>,
) {
    use crate::net::motion::{apply_move, arrival_snap, trace_relay, PendingMove, RelayOutcome};
    // Our own guid: the server writing our pose (`SetAsServerSide`, `ctime = 0`), which the
    // reference applies with no mover gate (`0x603bb0`); it goes to `player::wire_in`.
    if self_guid.0 == Some(guid) {
        trace_relay(
            guid,
            &mv,
            &Default::default(),
            now_ms,
            0,
            RelayOutcome::SelfMover,
        );
        self_moves.write(crate::net::SelfMoveMessage {
            position: mv.position,
            orientation: mv.orientation,
            flags: mv.flags,
            pitch: mv.pitch,
            fall_time: mv.fall_time,
            jump: mv.jump,
            transport: mv.transport,
        });
        return;
    }
    let Some(&e) = index.0.get(&guid) else {
        // No entity: traced, since a mover running on while these pile up is a streaming bug.
        trace_relay(
            guid,
            &mv,
            &Default::default(),
            now_ms,
            0,
            RelayOutcome::Unknown,
        );
        return;
    };
    {
        // The server's pose supersedes a stale spline.
        commands.entity(e).remove::<Spline>();
        if let Ok(mut rm) = remote_motion.get_mut(e) {
            // The chain reads the state before this move applies, as the reference times the
            // packet off the live `[esi+0x40]`/`[esi+0x150]`.
            let (flags, queue_empty) = (rm.flags, rm.pending.is_empty());
            let fire_ms = rm.relay.schedule(mv.wire_ms, now_ms, flags, queue_empty);
            let at_arrival = arrival_snap() || rm.fires_at_arrival(fire_ms, now_ms);
            trace_relay(
                guid,
                &mv,
                &rm.relay,
                now_ms,
                rm.pending.len(),
                if at_arrival {
                    RelayOutcome::Now
                } else {
                    RelayOutcome::Queued
                },
            );
            if at_arrival {
                apply_move(e, &mv, &mut rm, now_ms, commands, landings);
            } else {
                // Fire-times are monotone per unit, so the queue stays ordered.
                rm.pending.push_back(PendingMove { fire_ms, mv });
            }
        } else {
            // The first move seeds the chain and applies at arrival; the insert is deferred, so
            // the extrapolator sees it next frame.
            let mut rm = RemoteMotion {
                wow_pos: mv.position,
                orientation: mv.orientation,
                flags: 0,
                pitch: 0.0,
                speed: 0.0,
                vertical_velocity: 0.0,
                jump_xy_vel: [0.0; 2],
                fall_start_z: None,
                pending: std::collections::VecDeque::new(),
                relay: Default::default(),
                last_apply_ms: now_ms,
                last_apply_pos: mv.position,
            };
            let fire_ms = rm.relay.schedule(mv.wire_ms, now_ms, 0, true);
            debug_assert_eq!(fire_ms, now_ms, "a seeding packet fires at arrival");
            trace_relay(guid, &mv, &rm.relay, now_ms, 0, RelayOutcome::Seed);
            apply_move(e, &mv, &mut rm, now_ms, commands, landings);
            write_pose(
                commands,
                transforms,
                e,
                mv.position,
                wire_yaw(mv.orientation),
            );
            commands.entity(e).insert(rm);
        }
    }
}

/// An `SMSG_UPDATE_OBJECT` values block, merged into any object's store; an unknown guid drops.
fn object_values(
    guid: u64,
    fields: ObjectFields,
    commands: &mut Commands,
    index: &GuidIndex,
    stores: &mut Query<&mut ObjectStore>,
    edges: &mut MessageWriter<FieldChanged>,
) {
    if let Some(&e) = index.0.get(&guid) {
        merge_fields(commands, stores, edges, e, guid, fields);
    }
}

/// `SMSG_DESTROY_OBJECT`: corpse decay ahead of respawn, or a despawn.
fn object_destroyed(guid: u64, commands: &mut Commands, index: &mut GuidIndex) {
    // The object is freed now and its model fades out, as the reference's OnDeactivate
    // (`0x6145e0`, called by `0x464920` on DESTROY and OUT_OF_RANGE alike) hands the model to the
    // `SWModelFadeout` scheduler (`0x672df0`). The guid leaves the index at once ([`tear_down`]).
    //
    // A pinned object only gets its pending-destroy bit and is freed when the last pin drops
    // (`0x464920`, `0x468410`). Our one pin is an announced `SMSG_GAMEOBJECT_DESPAWN_ANIM`, whose
    // end runs the deferred teardown ([`crate::go_anim::release_despawn_pin`]).
    if let Some(e) = index.0.remove(&guid) {
        // An item has no model to fade, so it goes now with its countdown cells.
        if guid::is_item(guid) {
            commands.entity(e).despawn();
            return;
        }
        commands.queue(move |world: &mut bevy::ecs::world::World| {
            if world
                .get::<crate::go_anim::DespawnAnimAnnounced>(e)
                .is_some()
            {
                if let Ok(mut ent) = world.get_entity_mut(e) {
                    ent.insert(crate::go_anim::PendingDestroy);
                }
            } else if let Ok(ent) = world.get_entity_mut(e) {
                tear_down(ent);
            }
        });
    }
}

/// The `OutOfRange` block: the object still exists, out of our range.
fn objects_removed(guids: Vec<u64>, commands: &mut Commands, index: &mut GuidIndex) {
    // Faded, not popped: OUT_OF_RANGE reaches `0x464920` like DESTROY, and the object ends now
    // while its model fades ([`tear_down`]).
    for g in guids {
        if let Some(e) = index.0.remove(&g) {
            commands.entity(e).queue(tear_down);
        }
    }
}

/// An object's teardown, the reference's `0x464920`: the object is freed and its detached model
/// fades out ([`DespawnFade`]); its OnDeactivate (`0x5fbb60` → `0x493910`) clears the selection
/// and sends `CMSG_SET_SELECTION 0` at once.
///
/// The model stays on the same entity, so removing its [`Guid`] is what ends the object for every
/// consumer; the [`NetEntity`] and [`ObjectStore`] stay so the model draws out (a decaying corpse
/// stays lying down). The caller has already removed the guid from the [`GuidIndex`], so a
/// same-tick re-create spawns a fresh object beside the fading model.
pub(crate) fn tear_down(mut ent: EntityWorldMut) {
    ent.remove::<Guid>();
    ent.insert(DespawnFade::default());
}

/// `SMSG_MONSTER_MOVE` or `SMSG_MONSTER_MOVE_TRANSPORT`: settles transport membership, snaps the
/// facing, then attaches or clears the spline. With a `transport`, `start`, the path and an
/// `Angle` facing are deck-local and the unit rides as a [`TransportRider`]; without one, a riding
/// unit leaves the deck, as vmangos drops it (`MoveSplineInit.cpp:156-159`).
fn monster_move(
    guid: u64,
    transport: Option<u64>,
    start: [f32; 3],
    spline_id: u32,
    path: Vec<[f32; 3]>,
    facing: MonsterMoveFacing,
    stop: bool,
    duration_ms: u32,
    flying: bool,
    run_mode: bool,
    rooted: bool,
    commands: &mut Commands,
    index: &GuidIndex,
    transforms: &mut Query<&mut Transform>,
    riders: &mut Query<&mut crate::transport::TransportRider>,
) {
    if let Some(&e) = index.0.get(&guid) {
        // The desync readout: the gap between where we draw the unit and where its path begins.
        trace_move_snap(
            guid,
            // Both sides in one frame: a deck packet's `start` is compared to the local pose.
            match transport {
                Some(_) => riders.get(e).ok().map(|r| r.local_pos),
                None => transforms.get(e).ok().map(|t| bevy_to_wow(t.translation)),
            },
            start,
            stop,
            duration_ms,
        );
        // Transport membership, settled before any pose is read, and above the root gate, which
        // refuses only the path. An `Angle` facing is deck-local (`MoveSplineInit.cpp:87`); a
        // `Spot` or `Target` is not converted server-side, so it is rebased by the deck's yaw.
        let deck_yaw = |t: u64| {
            index
                .0
                .get(&t)
                .and_then(|&te| transforms.get(te).ok())
                .map(|tf| tf.rotation.to_euler(EulerRot::YXZ).0)
        };
        if let Some(t) = transport {
            let local_facing = match facing {
                MonsterMoveFacing::None => None,
                MonsterMoveFacing::Angle(a) => Some(a),
                other => {
                    let target_pos = |g: u64| {
                        index
                            .0
                            .get(&g)
                            .and_then(|&te| transforms.get(te).ok())
                            .map(|tf| bevy_to_wow(tf.translation))
                    };
                    // The world position anchors the bearing; the wire's `start` is deck-local.
                    let world_pos = transforms.get(e).ok().map(|tf| bevy_to_wow(tf.translation));
                    world_pos
                        .and_then(|p| resolve_facing(other, p, target_pos))
                        .zip(deck_yaw(t))
                        .map(|(world, yaw)| world - yaw)
                }
            };
            match riders.get_mut(e) {
                Ok(mut rider) => {
                    rider.transport_guid = t;
                    rider.local_pos = start;
                    if let Some(o) = local_facing {
                        rider.local_orientation = o;
                    }
                }
                Err(_) => {
                    commands.entity(e).insert(crate::transport::TransportRider {
                        transport_guid: t,
                        local_pos: start,
                        local_orientation: local_facing.unwrap_or_default(),
                    });
                }
            }
        } else if riders.get(e).is_ok() {
            commands
                .entity(e)
                .remove::<crate::transport::TransportRider>();
        }
        // The final facing (moveType 2/3/4) snaps the raw facing (`0x7c6f30`) and resets the
        // display smoother. A following path overrides it with the tangent each frame (`0x7c5490`),
        // so it sticks only on a stop. A deck packet's facing is the rider's `local_orientation`.
        if transport.is_none() && !matches!(facing, MonsterMoveFacing::None) {
            let target_pos = |g: u64| {
                index
                    .0
                    .get(&g)
                    .and_then(|&te| transforms.get(te).ok())
                    .map(|t| bevy_to_wow(t.translation))
            };
            if let Some(orientation) = resolve_facing(facing, start, target_pos) {
                if let Ok(mut t) = transforms.get_mut(e) {
                    t.rotation = Quat::from_rotation_y(orientation);
                    commands
                        .entity(e)
                        .remove::<crate::net::motion::DisplayFacing>();
                }
            }
        }
        // Every spline move un-nocks and drops the weapon hold: the handler `0x6018f0` (opcodes
        // 0xDD, 0x2AE) always ends through `0x60d040` (clear `0x400`) and `0x60f530` (un-nock).
        commands.entity(e).remove::<(
            crate::creature_anim::NockLatch,
            crate::creature_anim::RangedHold,
        )>();
        // A rooted unit takes no path: `0x6187a0`, the only route into the spline installer
        // `0x7c6a50`, returns on ROOT (`0x6187c2`). Only the path is refused: the un-nock tail runs
        // on every path (`0x6020ef`), and the rest for a rooted unit is untraced.
        if rooted {
            return;
        }
        // The spline supersedes the relay stream: a splined player (charge, taxi) would otherwise
        // keep stale flags that outrank the spline in the anim selector.
        commands.entity(e).remove::<RemoteMotion>();
        match monster_move_spline(
            path,
            spline_id,
            stop,
            duration_ms,
            flying,
            run_mode,
            transport,
        ) {
            Some(spline) => {
                commands.entity(e).insert(spline).remove::<SplineStopped>();
            }
            // A stop keeps the id, which the server awaits back for a player-driven unit.
            None => {
                commands
                    .entity(e)
                    .remove::<Spline>()
                    .insert(SplineStopped(spline_id));
            }
        }
    }
}

/// This unit's granted modes; no component means all bits clear.
fn modes_of(guid: u64, index: &GuidIndex, modes: &Query<&mut UnitMoveModes>) -> UnitMoveModes {
    index
        .0
        .get(&guid)
        .and_then(|&e| modes.get(e).ok().copied())
        .unwrap_or_default()
}

/// `SMSG_SPLINE_MOVE_*`: a mode on a unit we do not control, with no ack. An unknown guid is
/// dropped, as the reference's handler (`0x603c80`) does; our own guid is not special-cased, as
/// vmangos never sends it for a player's mover.
fn spline_move_mode(
    guid: u64,
    mode: SplineMode,
    apply: bool,
    commands: &mut Commands,
    index: &GuidIndex,
    modes: &mut Query<&mut UnitMoveModes>,
    remote: &mut Query<&mut RemoteMotion>,
) {
    let Some(&e) = index.0.get(&guid) else {
        return; // the reference drops it too
    };
    let mut word = modes_of(guid, index, modes);
    word.set(mode, apply);
    if let Ok(mut live) = modes.get_mut(e) {
        *live = word;
    } else {
        commands.entity(e).insert(word);
    }

    // `SetRoot`'s one-shot wipe (`0x7c7340`): it clears `0xffe07f00`'s complement and stops the
    // fall (`0x7c6290`), so the unit fails the integration gate and a relayed player stops dead.
    // A creature has no direction bits; `monster_move` refuses its path instead.
    if mode == SplineMode::Root && apply {
        if let Ok(mut rm) = remote.get_mut(e) {
            use crate::creature_anim::move_flags;
            rm.flags &= ROOT_APPLY_WIPE & !(move_flags::FALLING | move_flags::FALLING_FAR);
            rm.speed = 0.0;
            rm.vertical_velocity = 0.0;
            rm.jump_xy_vel = [0.0; 2];
            rm.fall_start_z = None;
        }
    }
}

/// `SMSG_GAMEOBJECT_QUERY_RESPONSE`: cached, with the lockId resolved from its type's `data[]`
/// slot for use-vs-cast.
fn gameobject_info(
    entry: u32,
    type_id: u32,
    display_id: u32,
    name: String,
    data: &[i32; 24],
    go_templates: &mut GameObjectTemplates,
) {
    debug!("net: gameobject template {entry} type {type_id} display {display_id} {name:?}");
    go_templates.insert(entry, type_id, name, data);
}

/// Merges a descriptor delta into an object's store and reports its [`FieldChanged`] edges. An
/// indexed entity with no store should not happen, and is seeded from the delta.
fn merge_fields(
    commands: &mut Commands,
    stores: &mut Query<&mut ObjectStore>,
    edges: &mut MessageWriter<FieldChanged>,
    entity: Entity,
    guid: u64,
    delta: ObjectFields,
) {
    if let Ok(mut s) = stores.get_mut(entity) {
        merge_store_fields(&mut s.0, delta, entity, guid, |e| {
            edges.write(e);
        });
    } else {
        warn!(
            "net: values delta for {guid:#x} found no store on its entity — seeding from the delta"
        );
        commands.entity(entity).insert(ObjectStore(delta));
    }
}

/// Writes one slot of a mover's [`UnitSpeeds`]; `false` when there is none (an untracked guid,
/// or a create with no movement block). vmangos sends a self create and a speed change back to
/// back (`HandleMoveWorldportAckOpcode`), and wire order holds as each handler applies in turn.
fn set_speed(
    guid: u64,
    kind: SpeedKind,
    speed: f32,
    index: &GuidIndex,
    speeds: &mut Query<&mut UnitSpeeds>,
) -> bool {
    let Some(mut s) = index.0.get(&guid).and_then(|&e| speeds.get_mut(e).ok()) else {
        return false;
    };
    let s = &mut s.0;
    match kind {
        SpeedKind::Walk => s.walk = speed,
        SpeedKind::Run => s.run = speed,
        SpeedKind::RunBack => s.run_back = speed,
        SpeedKind::Swim => s.swim = speed,
        SpeedKind::SwimBack => s.swim_back = speed,
        SpeedKind::TurnRate => s.turn_rate = speed,
    }
    true
}

/// `SMSG_FORCE_*_SPEED_CHANGE`: applied to the mover's speeds, and for our own guid forwarded to
/// the controller, which owes the ack. A foreign mover is never acked, which the server treats as
/// an error.
fn force_speed_change(
    guid: u64,
    kind: SpeedKind,
    counter: u32,
    speed: f32,
    index: &GuidIndex,
    speeds: &mut Query<&mut UnitSpeeds>,
    self_guid: &SelfGuid,
    speed_changes: &mut MessageWriter<SpeedChangeMessage>,
) {
    let applied = set_speed(guid, kind, speed, index, speeds);
    if self_guid.0 == Some(guid) {
        info!("net: force {kind:?} speed change -> {speed} yd/s (counter {counter})");
        // Should be unreachable; the ack still goes, so it warns.
        if !applied {
            warn!(
                "net: force {kind:?} speed change for OUR mover {guid:#x} landed nowhere \
                 (no live speed set) — we still ack, so the server and we now disagree"
            );
        }
        speed_changes.write(SpeedChangeMessage {
            guid,
            kind,
            counter,
            speed,
        });
    } else {
        debug!("net: force speed change for foreign mover {guid:#x} — applied {applied}, no ack");
    }
}

/// The SPLINE_SET and MOVE_SET families: an observed unit's speed, no ack. MOVE_SET's pose
/// arrives as its own `UnitMove`.
fn speed_changed(
    guid: u64,
    kind: SpeedKind,
    speed: f32,
    index: &GuidIndex,
    speeds: &mut Query<&mut UnitSpeeds>,
) {
    if !set_speed(guid, kind, speed, index, speeds) {
        // A speed broadcast can outrun the create block, which carries the same set.
        debug!("net: {kind:?} speed change for untracked mover {guid:#x} — nothing to write");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x1EB` for an unheld item waits on the active player and replays at its create; `0x1EA`
    /// for one is dropped; the destroy takes the cells.
    #[test]
    fn item_countdowns_live_on_the_item_and_a_queued_enchant_waits_for_it() {
        use crate::items::Countdowns;
        const ME: u64 = 0x0000_0000_0000_0007;
        const SWORD: u64 = 0x4000_0000_0000_0042;
        const STONE: u64 = 0x4000_0000_0000_0043;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let create = |guid| SessionEvent::ItemCreate {
            guid,
            container: false,
            fields: ObjectFields::from_pairs(&[(3, 117)]),
        };
        let cells = |world: &World, guid: u64| -> Option<Countdowns> {
            let e = *world.resource::<GuidIndex>().0.get(&guid)?;
            world.get::<Countdowns>(e).cloned()
        };

        // No active player: the enchant update has nowhere to wait.
        super::super::handlers::dispatch(
            world,
            vec![
                SessionEvent::ItemEnchantTime {
                    item_guid: STONE,
                    slot: 1,
                    seconds: 60,
                },
                create(STONE),
            ],
        );
        assert_eq!(
            cells(world, STONE),
            Some(Countdowns::default()),
            "dropped, not queued"
        );

        // With the active player resolving, the enchant waits for the item; the lifetime does not.
        let me = world.spawn_empty().id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        super::super::handlers::dispatch(
            world,
            vec![
                SessionEvent::ItemEnchantTime {
                    item_guid: SWORD,
                    slot: 1,
                    seconds: 90,
                },
                SessionEvent::ItemTime {
                    item_guid: SWORD,
                    seconds: 600,
                },
            ],
        );
        assert_eq!(cells(world, SWORD), None, "nothing held yet");
        super::super::handlers::dispatch(world, vec![create(SWORD)]);
        let c = cells(world, SWORD).expect("the create spawned the item with its cells");
        assert!(
            c.enchant_remaining_ms(1)
                .is_some_and(|ms| (89_000..=90_000).contains(&ms)),
            "the queued enchant replayed as of the create"
        );
        assert_eq!(
            c.lifetime_remaining_ms(),
            None,
            "`0x1EA` for an unheld item has no queue"
        );

        // Held now: both arms write the cells directly; a slot past the array writes nothing.
        super::super::handlers::dispatch(
            world,
            vec![
                SessionEvent::ItemTime {
                    item_guid: SWORD,
                    seconds: 600,
                },
                SessionEvent::ItemEnchantTime {
                    item_guid: SWORD,
                    slot: 9,
                    seconds: 30,
                },
            ],
        );
        let c = cells(world, SWORD).unwrap();
        assert!(c.lifetime_remaining_ms().is_some());
        assert!(c.enchant_remaining_ms(1).is_some());

        // The destroy takes the object and its cells with it.
        let e = world.resource::<GuidIndex>().0[&SWORD];
        super::super::handlers::dispatch(world, vec![SessionEvent::ObjectDestroyed(SWORD)]);
        assert!(!world.resource::<GuidIndex>().0.contains_key(&SWORD));
        assert!(world.get_entity(e).is_err(), "the cells died with the item");
    }

    /// The observer movement-mode family, on a bare `World` with one indexed unit.
    mod spline_modes {
        use super::*;
        use benilla_protocol::SplineMode;
        use bevy::ecs::system::RunSystemOnce;

        const MOB: u64 = 0x0000_0000_0000_00F0;

        /// A world holding one indexed unit, optionally already dead-reckoning as a relayed player.
        fn world_with_unit(remote: Option<RemoteMotion>) -> (World, Entity) {
            let mut w = World::new();
            let mut e = w.spawn_empty();
            if let Some(rm) = remote {
                e.insert(rm);
            }
            let entity = e.id();
            let mut index = GuidIndex::default();
            index.0.insert(MOB, entity);
            w.insert_resource(index);
            (w, entity)
        }

        fn remote_walking_forward() -> RemoteMotion {
            RemoteMotion {
                wow_pos: [0.0; 3],
                pending: std::collections::VecDeque::new(),
                orientation: 0.0,
                flags: crate::creature_anim::move_flags::FORWARD,
                pitch: 0.0,
                speed: 7.0,
                vertical_velocity: 0.0,
                jump_xy_vel: [0.0; 2],
                fall_start_z: None,
                relay: Default::default(),
                last_apply_ms: 0.0,
                last_apply_pos: [0.0; 3],
            }
        }

        /// Runs one grant and flushes its commands.
        fn grant(w: &mut World, mode: SplineMode, apply: bool) {
            w.run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      mut modes: Query<&mut UnitMoveModes>,
                      mut remote: Query<&mut RemoteMotion>| {
                    spline_move_mode(
                        MOB,
                        mode,
                        apply,
                        &mut commands,
                        &index,
                        &mut modes,
                        &mut remote,
                    );
                },
            )
            .unwrap();
        }

        /// Every one of the twelve opcodes sets or clears one bit.
        #[test]
        fn a_grant_sets_the_bit_and_its_pair_clears_it() {
            use crate::creature_anim::move_flags;
            let (mut w, e) = world_with_unit(None);

            grant(&mut w, SplineMode::Hover, true);
            grant(&mut w, SplineMode::WaterWalk, true);
            let m = *w
                .entity(e)
                .get::<UnitMoveModes>()
                .expect("first grant inserts");
            assert_eq!(
                m.0,
                move_flags::HOVER | move_flags::WATER_WALKING,
                "two grants accumulate in one word — the reference has one CMovement+0x40"
            );

            grant(&mut w, SplineMode::Hover, false);
            let m = *w.entity(e).get::<UnitMoveModes>().unwrap();
            assert_eq!(
                m.0,
                move_flags::WATER_WALKING,
                "UNSET_HOVER takes back hover and nothing else"
            );
        }

        /// `SET_RUN_MODE` clears the walk bit: the pair is inverted against its names (`0x617e80`
        /// → `SetRunMode` `0x7c71c0`), folded at the parse.
        #[test]
        fn the_walk_bit_round_trips_with_the_run_opcodes_sense() {
            use crate::creature_anim::move_flags;
            let (mut w, e) = world_with_unit(None);
            grant(&mut w, SplineMode::WalkMode, true); // SMSG_SPLINE_MOVE_SET_WALK_MODE
            assert_eq!(
                w.entity(e).get::<UnitMoveModes>().unwrap().0,
                move_flags::WALK_MODE
            );
            grant(&mut w, SplineMode::WalkMode, false); // SMSG_SPLINE_MOVE_SET_RUN_MODE
            assert_eq!(w.entity(e).get::<UnitMoveModes>().unwrap().0, 0);
        }

        /// `SetRoot` (`0x7c7340`) wipes the direction bits, so the unit fails the integration gate
        /// (`0x616e20`, `test [esi+0x40], 0x20ff`) and is not stepped.
        #[test]
        fn a_root_wipes_the_direction_bits_the_dead_reckon_runs_on() {
            use crate::creature_anim::move_flags;
            let (mut w, e) = world_with_unit(Some(remote_walking_forward()));

            grant(&mut w, SplineMode::Root, true);

            let rm = w.entity(e).get::<RemoteMotion>().unwrap();
            assert_eq!(
                rm.flags & move_flags::INTEGRATED,
                0,
                "nothing the integration gate tests may survive the root — this is what stops it"
            );
            assert_eq!(rm.speed, 0.0, "and it is not still travelling at run speed");
            assert!(w.entity(e).get::<UnitMoveModes>().unwrap().rooted());
        }

        /// `ClearRoot` (`0x7c7370`) only clears the bit.
        #[test]
        fn an_unroot_clears_the_bit_and_invents_no_movement() {
            let (mut w, e) = world_with_unit(Some(remote_walking_forward()));
            grant(&mut w, SplineMode::Root, true);
            grant(&mut w, SplineMode::Root, false);
            assert!(!w.entity(e).get::<UnitMoveModes>().unwrap().rooted());
            assert_eq!(
                w.entity(e).get::<RemoteMotion>().unwrap().flags,
                0,
                "still no direction bits — the wipe is one-shot and the unroot does not undo it"
            );
        }

        /// As in the reference's handler (`0x603c80`); the create carries the modes itself.
        #[test]
        fn a_grant_for_a_unit_we_do_not_have_is_dropped() {
            let (mut w, e) = world_with_unit(None);
            w.run_system_once(
                |mut commands: Commands,
                 index: Res<GuidIndex>,
                 mut modes: Query<&mut UnitMoveModes>,
                 mut remote: Query<&mut RemoteMotion>| {
                    spline_move_mode(
                        0xDEAD_BEEF,
                        SplineMode::Root,
                        true,
                        &mut commands,
                        &index,
                        &mut modes,
                        &mut remote,
                    );
                },
            )
            .unwrap();
            assert!(w.entity(e).get::<UnitMoveModes>().is_none());
        }

        /// `0x6187a0` returns on ROOT (`0x6187c2`), so a queued path does not move a pinned body.
        #[test]
        fn a_rooted_unit_refuses_the_path_the_server_queued() {
            let mut w = World::new();
            let e = w
                .spawn((
                    Transform::default(),
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                ))
                .id();
            let mut index = GuidIndex::default();
            index.0.insert(MOB, e);
            w.insert_resource(index);

            let path = vec![[0.0, 0.0, 0.0], [30.0, 0.0, 0.0]];
            for (rooted, expect_spline) in [(true, false), (false, true)] {
                let path = path.clone();
                w.run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          mut transforms: Query<&mut Transform>,
                          mut riders: Query<&mut crate::transport::TransportRider>| {
                        monster_move(
                            MOB,
                            None,
                            [0.0, 0.0, 0.0],
                            7,
                            path.clone(),
                            MonsterMoveFacing::None,
                            false,
                            3000,
                            false,
                            true,
                            rooted,
                            &mut commands,
                            &index,
                            &mut transforms,
                            &mut riders,
                        );
                    },
                )
                .unwrap();
                assert_eq!(
                    w.entity(e).get::<Spline>().is_some(),
                    expect_spline,
                    "rooted={rooted}: a pinned body takes no path, an unpinned one does"
                );
            }
        }

        /// A plain path takes the unit off the deck, where vmangos drops the passenger
        /// (`MoveSplineInit.cpp:156-159`).
        #[test]
        fn a_transport_path_attaches_the_rider_and_a_plain_one_lets_it_go() {
            const BOAT: u64 = 0x2000_0000_0000_0007;
            let mut w = World::new();
            let e = w
                .spawn((
                    Transform::default(),
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                ))
                .id();
            let boat = w.spawn(Transform::default()).id();
            let mut index = GuidIndex::default();
            index.0.insert(MOB, e);
            index.0.insert(BOAT, boat);
            w.insert_resource(index);

            let deck_path = vec![[1.5, -2.5, 0.75], [4.0, -2.5, 0.75]];
            w.run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      mut transforms: Query<&mut Transform>,
                      mut riders: Query<&mut crate::transport::TransportRider>| {
                    monster_move(
                        MOB,
                        Some(BOAT),
                        [1.5, -2.5, 0.75],
                        7,
                        deck_path.clone(),
                        MonsterMoveFacing::Angle(1.25),
                        false,
                        3000,
                        false,
                        true,
                        false,
                        &mut commands,
                        &index,
                        &mut transforms,
                        &mut riders,
                    );
                },
            )
            .unwrap();
            let rider = w
                .entity(e)
                .get::<crate::transport::TransportRider>()
                .expect("a transport path attaches its rider");
            assert_eq!(rider.transport_guid, BOAT);
            assert_eq!(rider.local_pos, [1.5, -2.5, 0.75], "seeded deck-local");
            assert!(
                (rider.local_orientation - 1.25).abs() < 1e-6,
                "Angle is deck-local"
            );
            assert_eq!(
                w.entity(e).get::<Spline>().expect("a path").deck,
                Some(BOAT),
                "the spline rides the deck frame, not the world"
            );
            // The deck-local start is never written as a world position.
            assert_eq!(
                w.entity(e).get::<Transform>().unwrap().translation,
                Vec3::ZERO
            );

            let world_path = vec![[100.0, 200.0, 30.0], [110.0, 200.0, 30.0]];
            w.run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      mut transforms: Query<&mut Transform>,
                      mut riders: Query<&mut crate::transport::TransportRider>| {
                    monster_move(
                        MOB,
                        None,
                        [100.0, 200.0, 30.0],
                        8,
                        world_path.clone(),
                        MonsterMoveFacing::None,
                        false,
                        3000,
                        false,
                        true,
                        false,
                        &mut commands,
                        &index,
                        &mut transforms,
                        &mut riders,
                    );
                },
            )
            .unwrap();
            assert!(
                w.entity(e)
                    .get::<crate::transport::TransportRider>()
                    .is_none(),
                "a plain path takes the unit off the deck"
            );
            assert_eq!(w.entity(e).get::<Spline>().expect("a path").deck, None);
        }
    }
}
