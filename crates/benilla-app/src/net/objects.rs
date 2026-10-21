//! The bridge's object layer: the streamed world's creates, deltas, moves and destroys, movers'
//! speeds and modes, GameObject templates and anims, and items. Each event is one handler over
//! [`Scene`] whose commands apply before the next event's, so a create's store and speeds are live
//! components by the time the next block reads them. An `SMSG_UPDATE_OBJECT`'s blocks and its
//! passes are [`update`]'s.
use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::{
    EntityKind, MonsterMoveFacing, MoveSpeeds, ObjectFields, SpeedKind, SplineMode,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use crate::query_cache::Ask;
use benilla_world::model_fade::DespawnFade;
use benilla_world::vis_chain::VisChainOnly;

use super::motion::{
    create_spline, gameobject_rotation, monster_move_spline, pose_transform, resolve_facing,
    trace_create_spline, trace_move_snap, wire_yaw, write_pose, SplineStopped, ROOT_APPLY_WIPE,
};
use super::{
    ActiveMover, FieldChanged, Guid, GuidIndex, NetCommands, NetEntity, NetHandlerApp, ObjectStore,
    RemoteMotion, SelfGuid, SpeedChangeMessage, Spline, UnitMoveModes, UnitSpeeds,
};

mod update;

pub(crate) use update::{release_unheld, Listed, ReleasePending, Released};

/// `SMSG_GAMEOBJECT_CUSTOM_ANIM` and `SMSG_GAMEOBJECT_DESPAWN_ANIM` (`0x5f8930`, `0x5f8990`): an
/// event on the object's one-shot channel (`0x5f8c50`), armed at the packet so the hold its play
/// takes is there for a release in the same tick; vmangos sends `SMSG_DESTROY_OBJECT` right after
/// the despawn. An object whose model has no state machine yet drops the event.
fn gameobject_anim_event(
    guid: u64,
    index: &GuidIndex,
    go_anims: &mut Query<&mut crate::go_anim::GoAnim>,
    arm: impl FnOnce(&mut crate::go_anim::GoAnim),
) {
    if let Some(mut go) = index.0.get(&guid).and_then(|&e| go_anims.get_mut(e).ok()) {
        arm(&mut go);
    }
}

/// Registers the object layer's handlers.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    update::register(app);
    app.net_handler(K::ItemTemplate, on_item)
        .net_handler(K::ItemTime, on_item)
        .net_handler(K::ItemEnchantTime, on_item)
        .net_handler(K::MoveTimeSkipped, on_move_time_skipped)
        .net_handler(K::UnitMove, on_unit_move)
        .net_handler(K::MonsterMove, on_monster_move)
        .net_handler(K::GameObjectInfo, on_gameobject_info)
        .net_handler(K::GameObjectUnknown, on_gameobject_unknown)
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
    movers: Query<'w, 's, (), With<ActiveMover>>,
    stores: Query<'w, 's, &'static mut ObjectStore>,
    countdowns: Query<'w, 's, &'static mut crate::items::Countdowns>,
    remote: Query<'w, 's, &'static mut RemoteMotion>,
    modes: Query<'w, 's, &'static mut UnitMoveModes>,
    xp_awarded: Query<'w, 's, (), With<crate::ui_chat::combat::watch::XpAwarded>>,
    riders: Query<'w, 's, &'static mut crate::transport::TransportRider>,
    speeds: Query<'w, 's, &'static mut UnitSpeeds>,
    field_changes: MessageWriter<'w, FieldChanged>,
    item_notices: MessageWriter<'w, crate::item_watch::ItemNotice>,
    field_notices: MessageWriter<'w, crate::field_watch::FieldNotice>,
    /// The guid-scoped selection teardowns (`0x493910(guid, 1)`) a watcher raises.
    deselect: MessageWriter<'w, crate::target::DeselectGuid>,
    speed_changes: MessageWriter<'w, SpeedChangeMessage>,
    self_moves: MessageWriter<'w, super::SelfMoveMessage>,
    hard_landings: MessageWriter<'w, crate::creature_anim::HardLanding>,
    names: Res<'w, NameCache>,
    items: ResMut<'w, Items>,
    go_templates: ResMut<'w, GameObjectTemplates>,
    death_net: ResMut<'w, crate::death::DeathNet>,
    group: ResMut<'w, crate::ui_party::GroupState>,
    pet_bar: Option<Res<'w, crate::ui_pet::PetBar>>,
    guilds: Option<ResMut<'w, crate::ui_guild::GuildState>>,
    pass: ResMut<'w, update::UpdatePass>,
    released: ResMut<'w, Released>,
    nets: Query<'w, 's, &'static NetEntity>,
    list: ResMut<'w, update::ObjectList>,
    player: Option<Res<'w, crate::player::Player>>,
    item_objects: Query<'w, 's, &'static crate::items::ItemObject>,
    go_anims: Query<'w, 's, &'static mut crate::go_anim::GoAnim>,
    go_models:
        Query<'w, 's, &'static benilla_assets::ModelAnimations, With<crate::go_anim::GoAnim>>,
    pending: Query<'w, 's, (Entity, &'static Guid), With<update::ReleasePending>>,
    creatures: Option<Res<'w, crate::entities::Creatures>>,
    held_displays: Query<'w, 's, &'static mut update::HeldDisplay>,
}

/// The item watchers' reads of the scene ([`crate::item_watch::Watch`]).
fn item_watch<'a, 'w, 's>(
    index: &'a GuidIndex,
    stores: &'a Query<'w, 's, &'static mut ObjectStore>,
    self_guid: &SelfGuid,
    items: &'a Items,
) -> crate::item_watch::Watch<'a, impl Fn(u64) -> Option<&'a ObjectFields> + use<'a, 'w, 's>> {
    crate::item_watch::Watch {
        me: self_guid.0,
        lookup: move |g| {
            index
                .0
                .get(&g)
                .and_then(|&e| stores.get(e).ok())
                .map(|s| &s.0)
        },
        items,
    }
}

/// Items past their create block: `SMSG_ITEM_QUERY_SINGLE_RESPONSE`, whose refusal the next
/// create or entry change of that entry asks again; and the lifetime and enchant countdowns,
/// written into the item's own [`crate::items::Countdowns`].
fn on_item(In(ev): In<SessionEvent>, mut sc: Scene) {
    match ev {
        SessionEvent::ItemTemplate { entry, info } => {
            let info = info.map(|b| *b);
            debug!("net: item template {entry} → {info:?}");
            // The answer finishes the set-up of every item of ours waiting on it (`0x5d8390`); a
            // negative answer finishes nothing (its found byte is clear, `0x5d83b8`).
            let answered = info.is_some();
            let landed = answered && sc.items.template(entry, 0, Ask::Read).is_none();
            sc.items.insert_template(entry, info);
            // A negative answer releases the waiters with nothing run (`0x5d96bb`).
            let waiters = sc.items.take_entry_waiters(entry);
            let trade_waiters = sc.items.take_trade_waiters(entry);
            let ammo_waiters = sc.items.take_ammo_waiters(entry);
            if landed {
                let (notices, signals) = {
                    let (index, stores) = (&sc.index, &sc.stores);
                    let held = || {
                        index
                            .0
                            .iter()
                            .filter_map(|(&g, &e)| stores.get(e).ok().map(|s| (g, &s.0)))
                    };
                    let watch = item_watch(index, stores, &sc.self_guid, &sc.items);
                    (
                        watch.record_landed(entry, held(), &waiters),
                        watch.landed_signals(entry, held(), &waiters),
                    )
                };
                for notice in &notices {
                    if let crate::item_watch::ItemNotice::Record { guid, .. } = *notice {
                        replay_enchant_times(&mut sc.items, &mut sc.countdowns, &sc.index, guid);
                    }
                }
                sc.item_notices.write_batch(notices);
                sc.field_notices.write_batch(signals);
            }
            // The trade getters' and the ammo getters' callbacks run for a ready record only
            // (`0x4c0343`, `0x4c84f6`).
            if answered {
                sc.item_notices.write_batch(std::iter::repeat_n(
                    crate::item_watch::ItemNotice::TradeRecord { entry },
                    trade_waiters,
                ));
                sc.field_notices.write_batch(std::iter::repeat_n(
                    crate::field_watch::FieldNotice::Player {
                        id: crate::field_watch::id::UNIT_INVENTORY_CHANGED,
                    },
                    ammo_waiters,
                ));
            }
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

/// `CGItem_C::ApplyRecord`'s replay (`0x5ebde0` at `0x5d85b7`): the enchant times queued on the
/// active player while the item was not held, set as of now. The record set-up reaches it only
/// for our item while in the world (`0x5d8504`, `0x5d8521`), as [`ItemNotice::Record`] runs.
///
/// [`ItemNotice::Record`]: crate::item_watch::ItemNotice::Record
fn replay_enchant_times(
    items: &mut Items,
    countdowns: &mut Query<&mut crate::items::Countdowns>,
    index: &GuidIndex,
    guid: u64,
) {
    let queued = items.take_enchant_times(guid);
    let Some(mut cells) = index.0.get(&guid).and_then(|&e| countdowns.get_mut(e).ok()) else {
        return;
    };
    for (slot, seconds) in queued {
        if !cells.set_enchant(slot, seconds) {
            debug!("enchant timer: queued slot {slot} for {guid:#x} is past the seventh — refused");
        }
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

/// A refused entry: the template cache frees it (`0x5588e0`'s refusal leg), so no zeroed template
/// stands in for the object.
fn on_gameobject_unknown(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::GameObjectUnknown { entry } = ev {
        sc.go_templates.refuse(entry);
    }
}

fn on_gameobject_anim(In(ev): In<SessionEvent>, mut sc: Scene) {
    match ev {
        SessionEvent::GameObjectCustomAnim { guid, anim_id } => {
            debug!("net: gameobject {guid:#x} custom anim {anim_id}");
            gameobject_anim_event(guid, &sc.index, &mut sc.go_anims, |go| go.custom(anim_id));
        }
        SessionEvent::GameObjectDespawnAnim { guid } => {
            debug!("net: gameobject {guid:#x} despawn anim");
            gameobject_anim_event(guid, &sc.index, &mut sc.go_anims, |go| go.despawn());
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

/// A placed object's create block past its fields: what it is, where, and how it moves.
pub(super) struct CreateBlock {
    pub(super) guid: u64,
    pub(super) kind: EntityKind,
    pub(super) display_id: Option<u32>,
    pub(super) position: [f32; 3],
    pub(super) orientation: f32,
    pub(super) scale: f32,
    pub(super) speeds: Option<MoveSpeeds>,
    pub(super) mover: Option<benilla_protocol::MoverState>,
    pub(super) transport_progress: Option<u32>,
    pub(super) transport: Option<benilla_protocol::TransportPose>,
    pub(super) spline: Option<benilla_protocol::CreateSpline>,
}

/// Where a create block's object lands.
pub(super) enum Landing {
    /// A held object, refreshed in place; the caller writes its store.
    Live(Entity),
    /// A new entity with `store`: an object allocated now, or a released one brought back, which
    /// keeps `kept_pose` when its type takes no position from the block.
    New {
        store: ObjectFields,
        kept_pose: Option<Transform>,
    },
}

/// An `SMSG_UPDATE_OBJECT` create block's placement: spawns or refreshes the entity and warms the
/// ask-once caches off the block's `fields`. Returns the entity.
fn object_create(
    sc: &mut Scene,
    block: CreateBlock,
    fields: &ObjectFields,
    landing: Landing,
) -> Entity {
    let CreateBlock {
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
    } = block;
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
    if matches!(kind, EntityKind::GameObject) {
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
    let commands = &mut sc.commands;
    match landing {
        Landing::Live(e) => {
            // A re-create is a fresh snapshot: refresh identity and a unit's pose, drop any
            // extrapolation.
            commands.entity(e).insert(net).remove::<RemoteMotion>();
            // The set is replaced whole; a `SMSG_FORCE_*_SPEED_CHANGE` in the same tick lands on
            // top, since this insert applies before the next packet's handler
            // (`apply::seam_tests`).
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
            // Only a unit takes the block's movement (type-mask bit 3, `0x46638c`); any other type
            // reads the position and drops it (`0x4664c3`..`0x46656b`), so it stays where it
            // stands.
            if matches!(kind, EntityKind::Unit | EntityKind::Player) {
                write_pose(commands, &mut sc.transforms, e, position, placement);
            }
            e
        }
        Landing::New { store, kept_pose } => {
            // A transport spawns hidden: its create pose is the spawn point, not where it is in
            // its cycle; the transport tick unhides it at the first sampled pose.
            let visibility = if transport_anchor.is_some() {
                Visibility::Hidden
            } else {
                Visibility::default()
            };
            let mut entity = commands.spawn((
                Guid(guid),
                net,
                kept_pose.unwrap_or_else(|| pose_transform(position, placement)),
                visibility,
                ObjectStore(store),
                sc.list.link(),
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
            let e = entity.id();
            sc.index.0.insert(guid, e);
            e
        }
    }
}

/// An `SMSG_UPDATE_OBJECT` movement block: a new pose that supersedes any active path. The unit
/// this client moves, our player or a possessed unit ([`ActiveMover`]), drops it: the reference
/// compares the block's guid with the active mover before it looks the object up (`0x4670a5`,
/// `0x4670ae`, `0x4670b5`), and returns on a match (`0x4670bb`).
pub(super) fn object_move(
    guid: u64,
    position: [f32; 3],
    orientation: f32,
    commands: &mut Commands,
    index: &GuidIndex,
    transforms: &mut Query<&mut Transform>,
    movers: &Query<(), With<ActiveMover>>,
) {
    if let Some(&e) = index.0.get(&guid) {
        if movers.contains(e) {
            return;
        }
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
                fall_start: None,
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

/// An object leaving the live table, released or destroyed (`0x464920`): its detached model fades
/// out ([`DespawnFade`]); its OnDeactivate (`0x5fbb60` → `0x493910`) ends the selection with
/// `CMSG_SET_SELECTION 0` as the net stage ends, unless a party or raid member or pet keeps it as
/// a guid (`0x5fbbcd`), which the selection's tracker settles.
///
/// The model stays on the same entity, so removing its [`Guid`] is what ends the object for every
/// consumer; the [`NetEntity`] and [`ObjectStore`] stay so the model draws out (a decaying corpse
/// stays lying down). The caller has already removed the guid from the [`GuidIndex`], so a block
/// that brings the object back spawns a fresh entity beside the fading model.
fn tear_down(mut ent: EntityWorldMut) {
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
        // Every spline move un-nocks and ends the precast here. The handler `0x6018f0` (opcodes
        // 0xDD, 0x2AE) ends through `0x60d040` with the unit's own `+0xc8c` (`0x6020e8`) and
        // `0x60f530` (un-nock) on every path but one: a stop within 3 yd of the stop point jumps
        // past both (`0x6019e4`). The end's loot release reads the flags the path just wrote: a
        // path's install starts the move (`0x7c6ad4`), a refused one stops it (`0x6187b1`).
        commands
            .entity(e)
            .remove::<crate::creature_anim::NockLatch>();
        crate::creature_anim::end_precast(
            commands,
            e,
            crate::creature_anim::PrecastTail::Moving(!rooted),
        );
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
            rm.fall_start = None;
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

/// One update packet's events, as the protocol decodes them: its blocks, then its end.
#[cfg(test)]
pub(crate) fn packet(blocks: impl IntoIterator<Item = SessionEvent>) -> Vec<SessionEvent> {
    blocks
        .into_iter()
        .chain([SessionEvent::ObjectUpdateEnd])
        .collect()
}

/// `events` as the wire brings them one packet each: every update block closed as its own packet,
/// any other event as it is.
#[cfg(test)]
pub(crate) fn wire(events: impl IntoIterator<Item = SessionEvent>) -> Vec<SessionEvent> {
    events
        .into_iter()
        .flat_map(|ev| {
            let block = matches!(
                ev,
                SessionEvent::ObjectCreate { .. }
                    | SessionEvent::ItemCreate { .. }
                    | SessionEvent::ObjectValues { .. }
                    | SessionEvent::ObjectMove { .. }
                    | SessionEvent::ObjectsRemoved(_)
                    | SessionEvent::ObjectsNear(_)
                    | SessionEvent::ObjectNotify { .. }
            );
            std::iter::once(ev).chain(block.then_some(SessionEvent::ObjectUpdateEnd))
        })
        .collect()
}

#[cfg(test)]
mod update_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::ClientCommand;
    use benilla_protocol::guid;

    /// The object packets run the item watchers in wire order: the create hook, a values block,
    /// a seat of ours, a late record, the destroy; another player's item and a resend run none.
    #[test]
    fn the_object_packets_write_the_item_watchers_notices_in_wire_order() {
        use crate::item_watch::ItemNotice;
        use benilla_protocol::field::{
            FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD,
        };
        use benilla_protocol::messages::ObjectType;
        const ME: u64 = 0x0000_0000_0000_0007;
        const POTIONS: u64 = 0x4000_0000_0000_0042;
        const SCROLL: u64 = 0x4000_0000_0000_0043;
        const THEIRS: u64 = 0x4000_0000_0000_0044;
        const POTION: u32 = 118;
        const SCROLL_ID: u32 = 955;
        // `OBJECT_FIELD_TYPE` item, the entry, `ITEM_FIELD_OWNER` and a stack of one.
        let create = |guid: u64, entry: u32, owner: u64| SessionEvent::ItemCreate {
            guid,
            container: false,
            fields: ObjectFields::from_pairs(&[
                (2, 0x3),
                (FIELD_OBJECT_ENTRY, entry),
                (6, owner as u32),
                (7, 0),
                (FIELD_ITEM_STACK_COUNT, 1),
            ])
            .into_created(ObjectType::Item),
        };
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let me = world
            .spawn((
                Guid(ME),
                ObjectStore(ObjectFields::default().into_created(ObjectType::Player)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world
            .resource_mut::<Items>()
            .insert_template(POTION, Some(crate::items::test_template("Potion")));
        let pack_1 = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                create(POTIONS, POTION, ME),
                create(SCROLL, SCROLL_ID, ME),
                create(THEIRS, POTION, 0x8),
                SessionEvent::ObjectValues {
                    guid: POTIONS,
                    fields: ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 3)]),
                },
                SessionEvent::ObjectValues {
                    guid: POTIONS,
                    fields: ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 3)]),
                },
                SessionEvent::ObjectValues {
                    guid: THEIRS,
                    fields: ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 3)]),
                },
                SessionEvent::ObjectValues {
                    guid: ME,
                    fields: ObjectFields::from_pairs(&[
                        (pack_1, POTIONS as u32),
                        (pack_1 + 1, (POTIONS >> 32) as u32),
                    ]),
                },
                SessionEvent::ItemTemplate {
                    entry: SCROLL_ID,
                    info: Some(Box::new(crate::items::test_template("Scroll"))),
                },
                SessionEvent::ObjectDestroyed(POTIONS),
                SessionEvent::ObjectDestroyed(THEIRS),
            ]),
        );
        let notices: Vec<ItemNotice> = world
            .resource_mut::<Messages<ItemNotice>>()
            .drain()
            .collect();
        assert_eq!(
            notices,
            [
                ItemNotice::Record {
                    guid: POTIONS,
                    entry: POTION
                },
                ItemNotice::StackCount {
                    guid: POTIONS,
                    entry: POTION
                },
                ItemNotice::Seat {
                    old: None,
                    new: Some(POTION)
                },
                ItemNotice::Record {
                    guid: SCROLL,
                    entry: SCROLL_ID
                },
                ItemNotice::Destroyed {
                    guid: POTIONS,
                    entry: POTION
                },
            ]
        );
    }

    /// `0x4c0340`: every trade getter call that missed the cache registered its own callback, and
    /// the record's answer runs each once, in call order, signalling for a ready record only
    /// (`0x4c0343`); a refusal frees them silent.
    #[test]
    fn an_item_answer_runs_each_waiting_trade_getters_callback_once() {
        use crate::item_watch::ItemNotice;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        {
            let items = world.resource::<Items>();
            for entry in [4306, 4306, 2592, 7777] {
                items.wait_for_trade_record(entry);
            }
        }
        let answer = |entry: u32, ok: bool| SessionEvent::ItemTemplate {
            entry,
            info: ok.then(|| Box::new(crate::items::test_template("Thing"))),
        };
        super::super::handlers::dispatch(
            world,
            super::wire(vec![answer(4306, true), answer(7777, false)]),
        );
        let notices: Vec<ItemNotice> = world
            .resource_mut::<Messages<ItemNotice>>()
            .drain()
            .collect();
        assert_eq!(
            notices,
            [
                ItemNotice::TradeRecord { entry: 4306 },
                ItemNotice::TradeRecord { entry: 4306 }
            ],
            "one per call that asked, none for the refusal"
        );
        // The callbacks are spent: a second answer, or the refused entry's later one, runs none.
        super::super::handlers::dispatch(
            world,
            super::wire(vec![answer(4306, true), answer(7777, true)]),
        );
        assert!(world
            .resource_mut::<Messages<ItemNotice>>()
            .drain()
            .next()
            .is_none());
        // The one still waiting runs on its own answer.
        super::super::handlers::dispatch(world, super::wire(vec![answer(2592, true)]));
        assert_eq!(
            world
                .resource_mut::<Messages<ItemNotice>>()
                .drain()
                .collect::<Vec<_>>(),
            [ItemNotice::TradeRecord { entry: 2592 }]
        );
    }

    /// The item paths' `UNIT_INVENTORY_CHANGED`, in wire order: a create's record set-up for its
    /// owner, whoever's (`0x5d85fe`); a worn item's restack (`0x5d950e` → `0x4c72e0`); the late
    /// record of an item of ours; the worn item's destroy (`0x5d87c1`). A backpack restack is none.
    #[test]
    fn the_object_packets_write_the_item_paths_inventory_signals_in_wire_order() {
        use crate::field_watch::FieldNotice;
        use benilla_protocol::field::{
            FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD,
        };
        use benilla_protocol::messages::ObjectType;
        const ME: u64 = 0x0000_0000_0000_0007;
        const HELM: u64 = 0x4000_0000_0000_0042;
        const POTIONS: u64 = 0x4000_0000_0000_0043;
        const THEIRS: u64 = 0x4000_0000_0000_0044;
        const HELM_ID: u32 = 7366;
        const POTION: u32 = 118;
        let create = |guid: u64, entry: u32, owner: u64| SessionEvent::ItemCreate {
            guid,
            container: false,
            fields: ObjectFields::from_pairs(&[
                (2, 0x3),
                (FIELD_OBJECT_ENTRY, entry),
                (6, owner as u32),
                (7, 0),
                (8, owner as u32),
                (9, 0),
                (FIELD_ITEM_STACK_COUNT, 1),
            ])
            .into_created(ObjectType::Item),
        };
        let seat = |n: u16, guid: u64| {
            let lo = FIELD_PLAYER_INV_SLOT_HEAD + 2 * n;
            [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)]
        };
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let pairs: Vec<(u16, u32)> = seat(0, HELM).into_iter().chain(seat(23, POTIONS)).collect();
        let me = world
            .spawn((
                Guid(ME),
                ObjectStore(ObjectFields::from_pairs(&pairs).into_created(ObjectType::Player)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world
            .resource_mut::<Items>()
            .insert_template(POTION, Some(crate::items::test_template("Potion")));
        let restack = |guid: u64| SessionEvent::ObjectValues {
            guid,
            fields: ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 2)]),
        };
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                create(POTIONS, POTION, ME),
                create(HELM, HELM_ID, ME),
                create(THEIRS, POTION, 0x8),
                restack(POTIONS),
                restack(HELM),
                SessionEvent::ItemTemplate {
                    entry: HELM_ID,
                    info: Some(Box::new(crate::items::test_template("Helm"))),
                },
                SessionEvent::ObjectDestroyed(HELM),
            ]),
        );
        let inventory: Vec<FieldNotice> = world
            .resource_mut::<Messages<FieldNotice>>()
            .drain()
            .filter(|n| match *n {
                FieldNotice::Unit { id, .. }
                | FieldNotice::Signal { id, .. }
                | FieldNotice::Player { id } => {
                    id == crate::field_watch::id::UNIT_INVENTORY_CHANGED
                }
                _ => false,
            })
            .collect();
        let owner = |guid| FieldNotice::Signal {
            guid,
            id: crate::field_watch::id::UNIT_INVENTORY_CHANGED,
        };
        let slot = FieldNotice::Player {
            id: crate::field_watch::id::UNIT_INVENTORY_CHANGED,
        };
        assert_eq!(
            inventory,
            [owner(ME), owner(0x8), slot, owner(ME), slot],
            "the potions' set-up, theirs, the helm's restack, its late record, its destroy"
        );
    }

    /// The ammunition getters' record callback `0x4c84f0`: each call that missed runs once on a
    /// ready answer, `UNIT_INVENTORY_CHANGED` with `"player"` (`0x4c8509`); a refusal frees them.
    #[test]
    fn an_item_answer_runs_each_waiting_ammo_getters_callback_once() {
        use crate::field_watch::FieldNotice;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        {
            let items = world.resource::<Items>();
            for entry in [2512, 2512, 3030] {
                items.wait_for_ammo_record(entry);
            }
        }
        let answer = |entry: u32, ok: bool| SessionEvent::ItemTemplate {
            entry,
            info: ok.then(|| Box::new(crate::items::test_template("Arrow"))),
        };
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                answer(2512, true),
                answer(3030, false),
                answer(3030, true),
            ]),
        );
        let notices: Vec<FieldNotice> = world
            .resource_mut::<Messages<FieldNotice>>()
            .drain()
            .collect();
        assert_eq!(
            notices,
            [FieldNotice::Player {
                id: crate::field_watch::id::UNIT_INVENTORY_CHANGED
            }; 2],
            "one per call that asked, none for the refusal or after it"
        );
    }

    /// The object packets run the field watches per block in wire order: our first create none
    /// (`0x465d80`), a values block each watch it moved once, a create of an object already shown
    /// each watch it moved over every field (`0x465d8e`); another unit only its bridge.
    #[test]
    fn the_object_packets_write_the_field_watches_notices_per_block() {
        use crate::field_watch::FieldNotice;
        use benilla_protocol::messages::ObjectType;
        const ME: u64 = 0x0000_0000_0000_0007;
        const MOB: u64 = 0xF130_0000_0000_0001;
        const XP: u16 = 716;
        const STAT0: u16 = 150;
        let create = |guid: u64, kind: EntityKind, pairs: &[(u16, u32)]| {
            let created = if kind == EntityKind::Player {
                ObjectType::Player
            } else {
                ObjectType::Unit
            };
            SessionEvent::ObjectCreate {
                guid,
                kind,
                display_id: None,
                position: [0.0; 3],
                orientation: 0.0,
                scale: 1.0,
                speeds: None,
                mover: None,
                transport_progress: None,
                transport: None,
                spline: None,
                fields: ObjectFields::from_pairs(pairs).into_created(created),
            }
        };
        let values = |guid: u64, pairs: &[(u16, u32)]| SessionEvent::ObjectValues {
            guid,
            fields: ObjectFields::from_pairs(pairs),
        };
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let drain = |world: &mut World| -> Vec<FieldNotice> {
            world
                .resource_mut::<Messages<FieldNotice>>()
                .drain()
                .collect()
        };
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                create(ME, EntityKind::Player, &[(STAT0, 20), (XP, 10)]),
                create(MOB, EntityKind::Unit, &[(STAT0, 5)]),
            ]),
        );
        assert_eq!(
            drain(world),
            crate::field_watch::guild_tag(ME),
            "a first create runs no notify pass, only a player's own `PostInit`"
        );
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                values(ME, &[(XP, 30)]),
                values(MOB, &[(STAT0, 6), (STAT0 + 1, 2)]),
                // Two blocks on one watch: two runs.
                values(ME, &[(XP, 40)]),
                values(ME, &[(STAT0, 21), (XP, 40)]),
                // A create of the shown player runs each watch it moved, over every field.
                create(ME, EntityKind::Player, &[(STAT0, 21), (XP, 50)]),
            ]),
        );
        assert_eq!(
            drain(world),
            [
                FieldNotice::Player { id: 189 },
                FieldNotice::Unit { guid: MOB, id: 144 },
                FieldNotice::Unit { guid: MOB, id: 145 },
                FieldNotice::Player { id: 189 },
                FieldNotice::Unit { guid: ME, id: 144 },
                FieldNotice::Player { id: 189 },
            ]
        );
    }

    /// A player's first create runs its `PostInit` (`0x5debe0`): its guild tag (`0x5dec19`), then,
    /// for another player, its party record against its descriptor, the difference signalled and
    /// nothing written back (`0x5dec5f`). A create of a live player is neither a first create nor
    /// a reactivation, so it runs neither; our own create compares nothing.
    #[test]
    fn another_players_create_signals_its_record_against_its_descriptor() {
        use crate::field_watch::FieldNotice;
        use benilla_protocol::messages::{ObjectType, PartyMemberStatsInfo};
        const ME: u64 = 0x07;
        const MATE: u64 = 0x2001;
        const HEALTH: u16 = 22;
        const MAXHEALTH: u16 = 28;
        const LEVEL: u16 = 34;
        let create = |guid: u64, pairs: &[(u16, u32)]| SessionEvent::ObjectCreate {
            guid,
            kind: EntityKind::Player,
            display_id: None,
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: None,
            transport: None,
            spline: None,
            fields: ObjectFields::from_pairs(pairs).into_created(ObjectType::Player),
        };
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let record = PartyMemberStatsInfo {
            status: Some(1),
            cur_hp: Some(100),
            max_hp: Some(100),
            power_type: Some(0),
            cur_power: Some(0),
            max_power: Some(0),
            level: Some(60),
            auras: Some(Vec::new()),
            auras_negative: Some(Vec::new()),
            ..Default::default()
        };
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .stats
            .insert(MATE, record.clone());
        let drain = |world: &mut World| -> Vec<FieldNotice> {
            world
                .resource_mut::<Messages<FieldNotice>>()
                .drain()
                .collect()
        };
        let mate = [(HEALTH, 100), (MAXHEALTH, 200), (LEVEL, 60)];
        super::super::handlers::dispatch(world, super::wire(vec![create(MATE, &mate)]));
        let signal = |id| FieldNotice::Signal { guid: MATE, id };
        assert_eq!(
            drain(world),
            [FieldNotice::Global { id: 376 }, signal(345), signal(22)],
            "the guild tag, then the maximum differs"
        );
        super::super::handlers::dispatch(world, super::wire(vec![create(MATE, &mate)]));
        assert_eq!(
            drain(world),
            [],
            "a live re-create: no hook, no `PostUpdate`"
        );
        assert_eq!(
            world.resource::<crate::ui_party::GroupState>().stats[&MATE],
            record
        );
        super::super::handlers::dispatch(world, super::wire(vec![create(ME, &mate)]));
        assert_eq!(
            drain(world),
            crate::field_watch::guild_tag(ME),
            "our own create: the guild tag alone"
        );
    }

    /// The object handlers over a headless client with our player live (an empty store created
    /// as a player), the potion's template cached; the items it lists created, our own.
    fn watch_world(items: &[(u64, u32, bool)]) -> App {
        use benilla_protocol::messages::ObjectType;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let me = world
            .spawn((
                Guid(WATCH_ME),
                ObjectStore(ObjectFields::default().into_created(ObjectType::Player)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(WATCH_ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(WATCH_ME);
        world
            .resource_mut::<Items>()
            .insert_template(118, Some(crate::items::test_template("Potion")));
        let creates = items
            .iter()
            .map(|&(guid, entry, container)| watched_item(guid, entry, container));
        super::super::handlers::dispatch(world, super::wire(creates));
        drain_notices(world);
        app
    }

    const WATCH_ME: u64 = 0x0000_0000_0000_0007;

    /// Our item's create block: type item (or container, 4 seats), the entry, ourselves as owner
    /// and a stack of one.
    fn watched_item(guid: u64, entry: u32, container: bool) -> SessionEvent {
        use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY};
        use benilla_protocol::messages::ObjectType;
        let mut pairs = vec![
            (2, if container { 0x7 } else { 0x3 }),
            (FIELD_OBJECT_ENTRY, entry),
            (6, WATCH_ME as u32),
            (7, 0),
            (FIELD_ITEM_STACK_COUNT, 1),
        ];
        if container {
            pairs.push((48, 4));
        }
        SessionEvent::ItemCreate {
            guid,
            container,
            fields: ObjectFields::from_pairs(&pairs).into_created(if container {
                ObjectType::Container
            } else {
                ObjectType::Item
            }),
        }
    }

    fn drain_notices(world: &mut World) -> Vec<crate::item_watch::ItemNotice> {
        world
            .resource_mut::<Messages<crate::item_watch::ItemNotice>>()
            .drain()
            .collect()
    }

    /// Every item's first create asks for its record with the callback `0x5d8390` (`0x5d8300`),
    /// and so does an entry change to an uncached entry (`0x5d9652`); a re-create and a zero
    /// entry ask nothing. A refused record is asked again by the next create or entry change.
    #[test]
    fn an_items_record_is_asked_at_its_create_and_entry_change_only() {
        use benilla_protocol::field::FIELD_OBJECT_ENTRY;
        const A: u64 = 0x4000_0000_0000_00a1;
        const B: u64 = 0x4000_0000_0000_00b2;
        let mut app = watch_world(&[]);
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        let asked = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<u32> {
            crate::query_cache::queries(rx)
                .into_iter()
                .filter_map(|c| match c {
                    ClientCommand::ItemQuery { entry, .. } => Some(entry),
                    _ => None,
                })
                .collect()
        };
        let dispatch = |app: &mut App, ev: SessionEvent| {
            super::super::handlers::dispatch(app.world_mut(), super::wire(vec![ev]));
        };
        dispatch(&mut app, watched_item(A, 955, false));
        assert_eq!(asked(&rx), [955], "the create asks");
        dispatch(
            &mut app,
            SessionEvent::ItemTemplate {
                entry: 955,
                info: None,
            },
        );
        dispatch(&mut app, watched_item(A, 955, false));
        assert!(asked(&rx).is_empty(), "a re-create runs no set-up");
        dispatch(&mut app, watched_item(B, 955, false));
        assert_eq!(asked(&rx), [955], "the next create asks again");
        dispatch(
            &mut app,
            SessionEvent::ObjectValues {
                guid: A,
                fields: ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, 956)]),
            },
        );
        assert_eq!(asked(&rx), [956], "an entry change to a miss asks");
        dispatch(&mut app, watched_item(0x4000_0000_0000_00c3, 0, false));
        assert!(asked(&rx).is_empty(), "entry 0 has no key (`0x55ba42`)");
    }

    /// An entry change to an uncached entry waits on the answer with its own callback
    /// (`0x5d96b0`), not the create's set-up; a negative answer runs nothing.
    #[test]
    fn an_entry_change_that_misses_the_cache_runs_its_callback_on_the_answer() {
        use crate::item_watch::ItemNotice;
        use benilla_protocol::field::FIELD_OBJECT_ENTRY;
        const POTIONS: u64 = 0x4000_0000_0000_0042;
        let mut app = watch_world(&[(POTIONS, 118, false)]);
        let world = app.world_mut();
        let to = |entry: u32| SessionEvent::ObjectValues {
            guid: POTIONS,
            fields: ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, entry)]),
        };
        let answer = |entry: u32, known: bool| SessionEvent::ItemTemplate {
            entry,
            info: known.then(|| Box::new(crate::items::test_template("Scroll"))),
        };
        super::super::handlers::dispatch(world, super::wire(vec![to(955), answer(955, true)]));
        assert_eq!(
            drain_notices(world),
            [
                ItemNotice::Entry {
                    guid: POTIONS,
                    old: 118,
                    new: 955,
                    new_cached: false
                },
                ItemNotice::EntryRecord {
                    guid: POTIONS,
                    entry: 955
                },
            ]
        );
        super::super::handlers::dispatch(world, super::wire(vec![to(956), answer(956, false)]));
        assert_eq!(
            drain_notices(world),
            [ItemNotice::Entry {
                guid: POTIONS,
                old: 955,
                new: 956,
                new_cached: false
            }],
            "a negative answer runs nothing"
        );
    }

    /// `0x5588e0`'s refusal leg: a game object entry the server refuses is no template, where a
    /// zeroed one stood in for it. It reads unknown, a hover does not ask it again, and the next
    /// spawn of the entry does.
    #[test]
    fn a_refused_game_object_entry_is_no_template() {
        const ENTRY: u32 = 1731;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let fields = crate::go_templates::test_fields(ENTRY);
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        world
            .resource::<GameObjectTemplates>()
            .request_at_create(0x11, &fields, &commands);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::GameObjectQuery { entry: ENTRY, .. })
        ));

        super::super::handlers::dispatch(
            world,
            super::wire(vec![SessionEvent::GameObjectUnknown { entry: ENTRY }]),
        );
        let templates = world.resource::<GameObjectTemplates>();
        assert!(
            templates.of(&fields).is_none(),
            "no zeroed template stands in"
        );
        assert!(templates.is_unknown(ENTRY));
        assert!(rx.try_recv().is_err(), "reading it asks nothing");
        // A hover reads the record every frame and never asks: the builder (`0x52aa20`) calls no
        // object cache lookup.
        for _ in 0..600 {
            assert!(templates.of(&fields).is_none());
        }
        assert!(
            rx.try_recv().is_err(),
            "a hovered refused object is not asked again"
        );

        templates.request_at_create(0x12, &fields, &commands);
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::GameObjectQuery {
                    entry: ENTRY,
                    guid: 0x12
                })
            ),
            "the next spawn asks at once"
        );
    }

    /// A refused creature with a unit of it in view: the next create block of a unit of that
    /// entry asks at once (`0x60b15b`), with no window between.
    #[test]
    fn a_refused_creature_is_asked_again_when_a_new_unit_of_it_is_created() {
        const ENTRY: u32 = 1234;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        let create = |guid: u64| SessionEvent::ObjectCreate {
            guid,
            kind: EntityKind::Unit,
            display_id: None,
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: None,
            transport: None,
            spline: None,
            fields: ObjectFields::from_pairs(&[(2, 9), (3, ENTRY)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        };
        let creature_asks = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::CreatureQuery { entry: ENTRY, .. }))
                .count()
        };
        let unit =
            |counter: u64| counter | (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48);
        let (a, b) = (unit(1), unit(2));

        super::super::handlers::dispatch(app.world_mut(), super::wire(vec![create(a)]));
        assert_eq!(creature_asks(&rx), 1, "the create asks");
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_creature(ENTRY, None);
        assert_eq!(creature_asks(&rx), 0, "the refusal itself asks nothing");

        super::super::handlers::dispatch(app.world_mut(), super::wire(vec![create(b)]));
        assert_eq!(creature_asks(&rx), 1, "a new unit of it asks again");
    }

    /// `0x604600`, the entry watcher, looks the new entry up with a callback (`0x60463f`): a
    /// values block that moves a unit's entry to one not cached asks for it.
    #[test]
    fn an_entry_change_asks_the_creature_cache_for_the_new_entry() {
        const OLD: u32 = 299;
        const NEW: u32 = 2_671;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        let mob = (u64::from(OLD) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        super::super::handlers::dispatch(
            app.world_mut(),
            super::wire(vec![SessionEvent::ObjectCreate {
                guid: mob,
                kind: EntityKind::Unit,
                display_id: None,
                position: [0.0; 3],
                orientation: 0.0,
                scale: 1.0,
                speeds: None,
                mover: None,
                transport_progress: None,
                transport: None,
                spline: None,
                fields: ObjectFields::from_pairs(&[(3, OLD)])
                    .into_created(benilla_protocol::messages::ObjectType::Unit),
            }]),
        );
        let _ = rx.try_iter().count();
        super::super::handlers::dispatch(
            app.world_mut(),
            super::wire(vec![SessionEvent::ObjectValues {
                guid: mob,
                fields: ObjectFields::from_pairs(&[(3, NEW)]),
            }]),
        );
        assert!(
            rx.try_iter().any(
                |c| matches!(c, ClientCommand::CreatureQuery { entry: NEW, guid } if guid == mob)
            ),
            "the new entry is asked for"
        );
    }

    /// A unit of `entry` and a game object of `go_entry`, created as their blocks bring them.
    fn record_creates(unit: u64, entry: u32, go: u64, go_entry: u32) -> Vec<SessionEvent> {
        use benilla_protocol::messages::ObjectType;
        let create = |guid, kind, fields: ObjectFields| SessionEvent::ObjectCreate {
            guid,
            kind,
            display_id: None,
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: None,
            transport: None,
            spline: None,
            fields,
        };
        vec![
            create(
                unit,
                EntityKind::Unit,
                ObjectFields::from_pairs(&[(2, 9), (3, entry)]).into_created(ObjectType::Unit),
            ),
            create(
                go,
                EntityKind::GameObject,
                ObjectFields::from_pairs(&[(3, go_entry)]).into_created(ObjectType::GameObject),
            ),
        ]
    }

    /// The creature and template asks the wire saw, as `(creature, game object)` counts.
    fn record_asks(rx: &crossbeam_channel::Receiver<ClientCommand>) -> (usize, usize) {
        rx.try_iter().fold((0, 0), |(c, g), cmd| match cmd {
            ClientCommand::CreatureQuery { .. } => (c + 1, g),
            ClientCommand::GameObjectQuery { .. } => (c, g + 1),
            _ => (c, g),
        })
    }

    /// The record asks belong to the create hooks (`0x5fb880` → `0x60afb0`, the ask at
    /// `0x60b15b`; `0x5f7c40`, `0x5f7d81`), which a create of an object already shown skips for
    /// the all-fields watch (`0x465d79`, `0x465d8e`): a refused entry is not asked again by it.
    #[test]
    fn a_re_create_of_a_shown_object_asks_no_record() {
        const ENTRY: u32 = 1234;
        const GO_ENTRY: u32 = 1000;
        const GO: u64 = 0xF110_0000_0000_0009;
        let mob = (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));

        let creates = record_creates(mob, ENTRY, GO, GO_ENTRY);
        super::super::handlers::dispatch(app.world_mut(), super::wire(creates.clone()));
        assert_eq!(record_asks(&rx), (1, 1), "the first creates' hooks ask");
        let world = app.world_mut();
        world
            .resource_mut::<NameCache>()
            .insert_creature(ENTRY, None);
        world.resource_mut::<GameObjectTemplates>().refuse(GO_ENTRY);

        super::super::handlers::dispatch(app.world_mut(), super::wire(creates));
        assert_eq!(record_asks(&rx), (0, 0), "shown: no hook, no ask");
    }

    /// A game object brought back asks its record when it has none (`0x5f7ed0`: `+0x214` clear,
    /// `0x558560` at `0x5f7f31`); a unit brought back with its display unchanged asks nothing, its
    /// slot 3's display rebuild finding the same records (`0x5fbe28` → `0x60abe0` → `0x60ae10`).
    #[test]
    fn a_released_game_object_brought_back_asks_its_missing_record() {
        const ENTRY: u32 = 1234;
        const GO_ENTRY: u32 = 1000;
        const GO: u64 = 0xF110_0000_0000_0009;
        let mob = (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));

        let creates = record_creates(mob, ENTRY, GO, GO_ENTRY);
        super::super::handlers::dispatch(app.world_mut(), super::wire(creates.clone()));
        let world = app.world_mut();
        world
            .resource_mut::<NameCache>()
            .insert_creature(ENTRY, None);
        world.resource_mut::<GameObjectTemplates>().refuse(GO_ENTRY);
        super::super::handlers::dispatch(
            app.world_mut(),
            super::wire(vec![SessionEvent::ObjectsRemoved(vec![mob, GO])]),
        );
        let _ = record_asks(&rx);

        super::super::handlers::dispatch(app.world_mut(), super::wire(creates));
        assert_eq!(record_asks(&rx), (0, 1), "the game object's slot 3 asks");
    }

    /// The display tables of the display tests: 10 and 12 share model 1, whose sound is no row,
    /// so they name the same records; 11 is model 2, with a sound row.
    fn display_tables() -> crate::entities::Creatures {
        crate::entities::Creatures::from_catalog(
            benilla_formats::CreatureCatalog::from_record_rows(
                &[(10, 1, 0, 0), (11, 2, 0, 0), (12, 1, 0, 0)],
                &[(1, 0), (2, 50)],
                &[50],
            ),
        )
    }

    /// A unit of `ENTRY` at `display`, alive, as `type_mask` (9 a creature, `0x19` a player).
    fn displayed(guid: u64, kind: EntityKind, type_mask: u32, display: u32) -> SessionEvent {
        use benilla_protocol::field::{FIELD_UNIT_DISPLAYID, FIELD_UNIT_HEALTH};
        use benilla_protocol::messages::ObjectType;
        let as_type = match kind {
            EntityKind::Player => ObjectType::Player,
            _ => ObjectType::Unit,
        };
        SessionEvent::ObjectCreate {
            guid,
            kind,
            display_id: Some(display),
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: None,
            transport: None,
            spline: None,
            fields: ObjectFields::from_pairs(&[
                (2, type_mask),
                (3, 1234),
                (FIELD_UNIT_HEALTH, 100),
                (FIELD_UNIT_DISPLAYID, display),
            ])
            .into_created(as_type),
        }
    }

    /// A creature asks its record again when its display names a new record set: the display
    /// watcher (`0x6049b5`) runs `0x60abe0`, whose `0x60ae10` finds the records changed, and the
    /// resolve asks (`0x60ac21` → `0x60b15b`). A display with the same records, or one with no
    /// row (`0x60ae36`), changes nothing; a player never asks (`0x60b134`).
    #[test]
    fn a_new_display_record_set_asks_the_creature_record_again() {
        use benilla_protocol::field::FIELD_UNIT_DISPLAYID;
        const ENTRY: u32 = 1234;
        let mob = (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        const PLAYER: u64 = 0x0000_0000_0000_0042;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        app.world_mut().insert_resource(display_tables());
        let display = |world: &mut World, guid: u64, id: u32| {
            super::super::handlers::dispatch(
                world,
                super::wire(vec![SessionEvent::ObjectValues {
                    guid,
                    fields: ObjectFields::from_pairs(&[(FIELD_UNIT_DISPLAYID, id)]),
                }]),
            );
        };

        let world = app.world_mut();
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                displayed(mob, EntityKind::Unit, 9, 10),
                displayed(PLAYER, EntityKind::Player, 0x19, 10),
            ]),
        );
        assert_eq!(record_asks(&rx), (1, 0), "the creature's create hook alone");
        // Refused: the next lookup with a callback asks at once.
        world
            .resource_mut::<NameCache>()
            .insert_creature(ENTRY, None);

        display(world, mob, 12);
        assert_eq!(record_asks(&rx), (0, 0), "12 names 10's records");
        display(world, mob, 999);
        assert_eq!(record_asks(&rx), (0, 0), "no row: no change");
        display(world, PLAYER, 11);
        assert_eq!(record_asks(&rx), (0, 0), "a player resolves, never asks");
        display(world, mob, 11);
        assert_eq!(record_asks(&rx), (1, 0), "a new model: the records changed");
        display(world, mob, 11);
        assert_eq!(record_asks(&rx), (0, 0), "no edge, no watcher");
    }

    /// The health watcher rebuilds the display of a unit come back to life (`0x6047ee` →
    /// `0x604835`), whose records the display watcher did not see change: here the display table
    /// was not up when it did.
    #[test]
    fn a_unit_back_to_life_rebuilds_its_display() {
        use benilla_protocol::field::{FIELD_UNIT_DISPLAYID, FIELD_UNIT_HEALTH};
        const ENTRY: u32 = 1234;
        let mob = (u64::from(ENTRY) << 24) | (u64::from(guid::HIGH_UNIT) << 48) | 1;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.world_mut().insert_resource(NetCommands(tx));
        app.world_mut().insert_resource(display_tables());
        let world = app.world_mut();
        let values = |world: &mut World, pairs: &[(u16, u32)]| {
            super::super::handlers::dispatch(
                world,
                super::wire(vec![SessionEvent::ObjectValues {
                    guid: mob,
                    fields: ObjectFields::from_pairs(pairs),
                }]),
            );
        };
        super::super::handlers::dispatch(
            world,
            super::wire(vec![displayed(mob, EntityKind::Unit, 9, 10)]),
        );
        world
            .resource_mut::<NameCache>()
            .insert_creature(ENTRY, None);
        let _ = record_asks(&rx);

        let tables = world
            .remove_resource::<crate::entities::Creatures>()
            .unwrap();
        values(world, &[(FIELD_UNIT_DISPLAYID, 11)]);
        world.insert_resource(tables);
        values(world, &[(FIELD_UNIT_HEALTH, 0)]);
        assert_eq!(record_asks(&rx), (0, 0), "dying rebuilds nothing");
        values(world, &[(FIELD_UNIT_HEALTH, 50)]);
        assert_eq!(record_asks(&rx), (1, 0), "coming back does");
    }

    /// A create block for a live item runs the watchers over what it moved, never the create's
    /// record set-up (`0x465d79`, the object already shown).
    #[test]
    fn a_re_create_of_a_live_item_runs_its_watchers() {
        use crate::item_watch::ItemNotice;
        use benilla_protocol::field::FIELD_ITEM_STACK_COUNT;
        use benilla_protocol::messages::ObjectType;
        const POTIONS: u64 = 0x4000_0000_0000_0042;
        let mut app = watch_world(&[(POTIONS, 118, false)]);
        let world = app.world_mut();
        let SessionEvent::ItemCreate { fields, .. } = watched_item(POTIONS, 118, false) else {
            unreachable!()
        };
        let mut fields = fields;
        fields.merge(ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 4)]));
        let again = SessionEvent::ItemCreate {
            guid: POTIONS,
            container: false,
            fields: fields.into_created(ObjectType::Item),
        };
        super::super::handlers::dispatch(world, super::wire(vec![again]));
        assert_eq!(
            drain_notices(world),
            [ItemNotice::StackCount {
                guid: POTIONS,
                entry: 118
            }]
        );
    }

    /// The out-of-range list releases our item through the same deactivate as a destroy
    /// (`0x464920`); our own player in the list is skipped.
    #[test]
    fn an_item_of_ours_out_of_range_is_destroyed_for_the_watchers() {
        use crate::item_watch::ItemNotice;
        const POTIONS: u64 = 0x4000_0000_0000_0042;
        let mut app = watch_world(&[(POTIONS, 118, false)]);
        let world = app.world_mut();
        super::super::handlers::dispatch(
            world,
            super::wire(vec![SessionEvent::ObjectsRemoved(vec![WATCH_ME, POTIONS])]),
        );
        assert_eq!(
            drain_notices(world),
            [ItemNotice::Destroyed {
                guid: POTIONS,
                entry: 118
            }]
        );
    }

    /// A bag seated in one of our bag seats has its own seats watched (`0x4f90d0`); the same bag
    /// before it is seated, or another in the backpack, has none.
    #[test]
    fn a_seated_bags_seats_run_their_watcher_through_the_handlers() {
        use crate::item_watch::ItemNotice;
        use benilla_protocol::field::{FIELD_CONTAINER_SLOT_1, FIELD_PLAYER_INV_SLOT_HEAD};
        const POTIONS: u64 = 0x4000_0000_0000_0042;
        const BAG: u64 = 0x4000_0000_0000_0050;
        let mut app = watch_world(&[(POTIONS, 118, false), (BAG, 4496, true)]);
        let world = app.world_mut();
        let seat = |j: u16, g: u64| SessionEvent::ObjectValues {
            guid: BAG,
            fields: ObjectFields::from_pairs(&[
                (FIELD_CONTAINER_SLOT_1 + 2 * j, g as u32),
                (FIELD_CONTAINER_SLOT_1 + 2 * j + 1, (g >> 32) as u32),
            ]),
        };
        let bag_seat = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 19;
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                seat(1, POTIONS),
                SessionEvent::ObjectValues {
                    guid: WATCH_ME,
                    fields: ObjectFields::from_pairs(&[
                        (bag_seat, BAG as u32),
                        (bag_seat + 1, (BAG >> 32) as u32),
                    ]),
                },
                seat(1, 0),
                seat(2, POTIONS),
            ]),
        );
        assert_eq!(
            drain_notices(world),
            [
                ItemNotice::Bag { entries: vec![118] },
                ItemNotice::Seat {
                    old: Some(118),
                    new: None
                },
                ItemNotice::Seat {
                    old: None,
                    new: Some(118)
                },
            ],
            "the unseated bag's change ran nothing; the seated one's two did"
        );
    }

    /// `0x1EB` for an unheld item waits on the active player, for the item's record to replay it
    /// ([`a_queued_enchant_time_replays_when_the_items_record_applies`]); `0x1EA` for one is
    /// dropped; the destroy takes the cells.
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
            super::wire(vec![
                SessionEvent::ItemEnchantTime {
                    item_guid: STONE,
                    slot: 1,
                    seconds: 60,
                },
                create(STONE),
            ]),
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
            super::wire(vec![
                SessionEvent::ItemEnchantTime {
                    item_guid: SWORD,
                    slot: 1,
                    seconds: 90,
                },
                SessionEvent::ItemTime {
                    item_guid: SWORD,
                    seconds: 600,
                },
            ]),
        );
        assert_eq!(cells(world, SWORD), None, "nothing held yet");
        super::super::handlers::dispatch(world, super::wire(vec![create(SWORD)]));
        let c = cells(world, SWORD).expect("the create spawned the item with its cells");
        assert_eq!(
            c.enchant_remaining_ms(1),
            None,
            "the enchant waits on the record, which no one has sent"
        );
        assert_eq!(
            c.lifetime_remaining_ms(),
            None,
            "`0x1EA` for an unheld item has no queue"
        );

        // Held now: both arms write the cells directly; a slot past the array writes nothing.
        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                SessionEvent::ItemTime {
                    item_guid: SWORD,
                    seconds: 600,
                },
                SessionEvent::ItemEnchantTime {
                    item_guid: SWORD,
                    slot: 1,
                    seconds: 30,
                },
                SessionEvent::ItemEnchantTime {
                    item_guid: SWORD,
                    slot: 9,
                    seconds: 30,
                },
            ]),
        );
        let c = cells(world, SWORD).unwrap();
        assert!(c.lifetime_remaining_ms().is_some());
        assert!(c.enchant_remaining_ms(1).is_some());

        // The destroy takes the object and its cells with it.
        let e = world.resource::<GuidIndex>().0[&SWORD];
        super::super::handlers::dispatch(
            world,
            super::wire(vec![SessionEvent::ObjectDestroyed(SWORD)]),
        );
        assert!(!world.resource::<GuidIndex>().0.contains_key(&SWORD));
        assert!(world.get_entity(e).is_err(), "the cells died with the item");
    }

    /// A queued `0x1EB` replays from the item's record application, `0x5d8440` (`0x5ebde0` at
    /// `0x5d85b7`): at the create hook when the record is cached, else when it lands (`0x5d8390`),
    /// and only for the active player's item while in the world (`0x5d8504`, `0x5d8521`).
    #[test]
    fn a_queued_enchant_time_replays_when_the_items_record_applies() {
        use crate::items::Countdowns;
        use benilla_protocol::messages::ObjectType;
        const ME: u64 = 0x0000_0000_0000_0007;
        const SWORD: u64 = 0x4000_0000_0000_0042;
        const THEIRS: u64 = 0x4000_0000_0000_0043;
        const MACE: u64 = 0x4000_0000_0000_0044;
        const ENTRY: u32 = 117;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let me = world
            .spawn((
                Guid(ME),
                ObjectStore(ObjectFields::default().into_created(ObjectType::Player)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let create = |guid: u64, owner: u64| SessionEvent::ItemCreate {
            guid,
            container: false,
            fields: ObjectFields::from_pairs(&[
                (2, 0x3),
                (3, ENTRY),
                (6, owner as u32),
                (7, (owner >> 32) as u32),
            ])
            .into_created(ObjectType::Item),
        };
        let queue = |guid: u64| SessionEvent::ItemEnchantTime {
            item_guid: guid,
            slot: 1,
            seconds: 90,
        };
        let replayed = |world: &World, guid: u64| -> bool {
            let e = world.resource::<GuidIndex>().0[&guid];
            world
                .get::<Countdowns>(e)
                .and_then(|c| c.enchant_remaining_ms(1))
                .is_some_and(|ms| (89_000..=90_000).contains(&ms))
        };

        super::super::handlers::dispatch(
            world,
            super::wire(vec![
                queue(SWORD),
                queue(THEIRS),
                create(SWORD, ME),
                create(THEIRS, 0x99),
            ]),
        );
        assert!(!replayed(world, SWORD), "no record yet: nothing applies");
        super::super::handlers::dispatch(
            world,
            vec![SessionEvent::ItemTemplate {
                entry: ENTRY,
                info: Some(Box::new(crate::items::test_template("Sword"))),
            }],
        );
        assert!(replayed(world, SWORD), "the record landed and applied");
        assert!(!replayed(world, THEIRS), "another's item applies no replay");

        super::super::handlers::dispatch(world, super::wire(vec![queue(MACE), create(MACE, ME)]));
        assert!(
            replayed(world, MACE),
            "a cached record applies at the create hook"
        );
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
                fall_start: None,
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

        /// `0x6020e8`: a spline move ends the unit's precast whatever it is, a rooted unit's too
        /// (the tail `0x6020db`-`0x6020ef` runs on every path). On our own player the end's loot
        /// release reads the flags the path just wrote: an installed path is moving
        /// (`0x7c6ad4`), a refused one has stopped (`0x6187b1`), whatever we last sent.
        #[test]
        fn a_spline_move_ends_the_precast() {
            for (rooted, ours) in [(false, false), (true, false), (false, true), (true, true)] {
                let mut w = World::new();
                w.init_resource::<crate::ui_loot::LootMoveStart>();
                w.init_resource::<crate::ui_loot::LootCloseAtRest>();
                let moving = crate::creature_anim::move_flags::FORWARD;
                w.insert_resource(crate::player::Player::with_move_flags(if rooted {
                    moving
                } else {
                    0
                }));
                let e = w
                    .spawn((
                        Transform::default(),
                        crate::creature_anim::Precast {
                            spell_id: 133,
                            busy: true,
                        },
                    ))
                    .id();
                if ours {
                    w.entity_mut(e).insert(crate::net::SelfPlayer);
                }
                let mut index = GuidIndex::default();
                index.0.insert(MOB, e);
                w.insert_resource(index);
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
                            vec![[0.0, 0.0, 0.0], [30.0, 0.0, 0.0]],
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
                assert!(
                    w.entity(e).get::<crate::creature_anim::Precast>().is_none(),
                    "rooted={rooted}: the precast ends"
                );
                assert_eq!(
                    (
                        w.resource::<crate::ui_loot::LootMoveStart>().0,
                        w.resource::<crate::ui_loot::LootCloseAtRest>().0,
                    ),
                    (ours && !rooted, ours && rooted),
                    "rooted={rooted}, ours={ours}: the loot release"
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
