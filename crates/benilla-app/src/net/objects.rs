//! The bridge's own object layer (in the net handler table since 2327 — the last family out of
//! the drain's dispatch match, which went with it): the streamed world's creates, deltas, moves
//! and destroys, the movers' speeds and granted modes, the GameObject templates and anims, and
//! the item store's three kinds. Each packet is one handler over [`Scene`]; a handler's commands
//! are applied before the next packet's handler runs, which is why the three
//! intra-drain staging maps the match kept (0061's `pending`, 1478's `SpeedStage`, 1780's
//! `StagedModes`) are gone: a create inserts its store and speeds at spawn, and the next packet
//! reads the live component. Registered from [`super::NetPlugin`].
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

/// A GameObject plays its one-shot **Custom** animation (`SMSG_GAMEOBJECT_CUSTOM_ANIM`, decision
/// 1086) — bridged to the GO animation machine ([`crate::go_anim`]), which owns the reject
/// (`anim_id >= 4`), the id mapping (153..156) and the model-ownership gate. The load-bearing
/// sender: the fishing bobber's bite splash (`anim_id 0`).
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

/// An object plays its one-shot **Despawn** animation (`SMSG_GAMEOBJECT_DESPAWN_ANIM`, decision
/// 1404) — the other half of the one-shot channel `0x5f8c50`, AnimationData id 157.
///
/// This marks the entity **immediately**, with a component rather than a message, because vmangos
/// sends `SMSG_DESTROY_OBJECT` for the same object in the same server tick: the mark has to be on
/// the entity before [`object_destroyed`] runs, or the destroy frees it before anything can play.
/// Commands apply in the order they were queued, so an insert queued here is visible to the
/// closure the destroy queues below. The arm itself, and the model-ownership gate, are
/// [`crate::go_anim`]'s.
fn gameobject_despawn_anim(guid: u64, commands: &mut Commands, index: &GuidIndex) {
    debug!("net: gameobject {guid:#x} despawn anim");
    if let Some(&e) = index.0.get(&guid) {
        commands
            .entity(e)
            .insert(crate::go_anim::DespawnAnimAnnounced);
    }
}

/// Register the object layer's handlers — called from [`super::NetPlugin`].
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

/// The streamed world as one parameter: the entities and their components the packets write,
/// the caches they warm, the hooks other subsystems keep on the object lifecycle (the reclaim
/// latch's, the roster's), and the edges they publish.
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

/// The item store's kinds: a descriptor-only create, the template's display head
/// (`SMSG_ITEM_QUERY_SINGLE_RESPONSE`, answering our `CMSG_ITEM_QUERY_SINGLE` — the ask-once
/// cache, decisions 0068/0072; a server miss records `None` so the entry is never re-asked), the
/// item-lifetime countdown's only feed and the temporary-enchant countdown's
/// — both written into the item's own [`crate::items::Countdowns`] (decision
/// 2340), which every tooltip surface reads back.
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
        // `0x5e4f30`'s two arms share the item lookup (`0x468460`, `TYPEMASK_ITEM`) — here the
        // index plus the item's own cells — and part company on a miss.
        SessionEvent::ItemTime { item_guid, seconds } => {
            // **An item we do not hold drops the update — no queue, no retry** (`0x1EA`'s arm
            // returns on the miss). The login re-send (`Player::SendItemDurations`) goes out the
            // moment the player is added to the map, so an item whose create has not landed yet
            // shows no lifetime until the server's next update, as in the reference.
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
            // `0x1EB`'s miss arm: the record goes onto a resolving player's pending list
            // (`0x5ebd40`) — the packet's own player guid first, the active player second — and
            // only the active player's list is ever replayed (`0x5d8440` → `0x5ebde0`). vmangos
            // names the owner, which is us (`SendItemEnchantTimeUpdate(GetObjectGuid(), ..)`,
            // `Player.cpp:11807`/`:12001`), so both lookups land on the active player.
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

/// **An observed mover skipped time**. No pose moved — only that unit's clock
/// ran on — so this touches its relay chain and nothing else, which is the whole of what the
/// reference's handler does (`0x603b40` → `0x61ab90`: `[CMovement+0xac] += lag`). A guid we do
/// not hold is dropped, faithfully: the reference resolves under `TYPEMASK_UNIT` and returns on
/// a miss.
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

/// The scheduled-replay law: `unit_move` runs the mover's own replay chain
/// over this packet's wire stamp to get its client fire-time, then applies it now if due, else
/// queues it on the unit for `drain_pending_moves`.
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

/// Our corpse's own `CORPSE_FIELD_FLAGS` can flip to BONES under a live guid; the reclaim latch
/// is re-asked on that edge, as the reference's `FLAGS` mirror handler `0x5d6d60` does (1729).
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

/// The party hook runs FIRST and on the same edge the reference takes it: the deactivate
/// virtual reads the descriptor that is about to go.
fn on_object_destroyed(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectDestroyed(guid) = ev {
        crate::death::net::forget_corpse(guid, &mut sc.death_net);
        let store = sc.index.0.get(&guid).and_then(|e| sc.stores.get(*e).ok());
        crate::ui_party::net::member_deactivated(guid, &mut sc.group, store, &sc.net);
        object_destroyed(guid, &mut sc.commands, &mut sc.index);
    }
}

/// OUT_OF_RANGE and DESTROY take the same virtual in the reference — so the snapshot +
/// `CMSG_REQUEST_PARTY_MEMBER_STATS` fire here too, which is the edge report B334 is actually
/// about: a member walking over the hill.
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

/// The observer movement-mode family — the same modes, on a body somebody else
/// is driving. No ack, so the handler ends the packet.
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

/// An object entered range / was created (`SMSG_UPDATE_OBJECT` create block): spawn or refresh the
/// entity, warm the ask-once caches, and seed its descriptor store. A handler's commands are
/// applied before the next packet's handler runs, so the store and the speeds
/// are components at spawn and the next packet reads them live.
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
    // A transport's cycle anchor: the create block's `UPDATE_FLAG_TRANSPORT`
    // u32 + the local instant it landed. Re-creates re-anchor (the server re-sends the create at
    // map transitions and mid-course update frames precisely so clients can correct drift). The
    // transport tick owns it from here (`crate::transport`). Both ticking GO types (the only two
    // the client's per-frame tick `0x630970` runs): 15 (boats — vmangos sends its path-progress
    // clock) and 11 (elevators/lifts — the same flag, `GameObject.cpp:246`, a
    // `time-since-create % period` clock).
    let go_type = (kind == EntityKind::GameObject).then(|| fields.gameobject_type_id());
    let transport_anchor = matches!(go_type, Some(11 | 15))
        .then_some(transport_progress)
        .flatten()
        .map(|progress_ms| crate::transport::TransportAnchor {
            progress_ms,
            at: std::time::Instant::now(),
        });
    // The spawn's `GAMEOBJECT_ROTATION` quaternion — read once, wanted twice: it is this object's
    // placement (below) and, on a type-11 lift, the basis its keyframe offsets rotate through.
    let go_quat = (kind == EntityKind::GameObject)
        .then(|| fields.gameobject_rotation())
        .flatten();
    // A type-11 lift's arm seed (decision 0438 phase 3's second consumer): the keyframe path is
    // keyed by the template **entry**, the offsets rotate through the spawn's `GAMEOBJECT_ROTATION`
    // quat, and the base is the stationary spot the movement block carried — all already in this
    // create block, so the lift arm needs no template round-trip. Anchor-gated: a type-11 whose
    // create carried no progress u32 has no clock to tick and stays frozen.
    let elevator_seed = (matches!(go_type, Some(11)) && transport_anchor.is_some())
        .then(|| fields.object_entry())
        .flatten()
        .map(|entry| crate::transport::ElevatorSeed {
            entry,
            base_pos: position,
            yaw: orientation,
            quat: go_quat,
        });
    // Where this object is *pointed*. A mover carries one yaw; a GameObject is placed by its
    // `GAMEOBJECT_ROTATION` quaternion, which is the reference's own placement input and is a
    // strictly wider answer than the facing (`motion::gameobject_rotation`).
    //
    // **A mover's create pose is not a bare yaw.** The reference's create-block apply seeds both
    // halves of the body-pitch law from this very block — the flags word through `0x618c30`'s
    // `0x75a07dff` merge, the pitch through the pose commit `0x7c6420`'s unconditional
    // `fst [ecx+0x20]` — and it does so *before* `0x613e10` builds the model or registers the
    // per-frame render callback that reads them, so the tilt is in force on the unit's **first
    // drawn frame**. A player who swims into view
    // nose-down renders nose-down, not level: [`crate::creature_anim::swim_body_rotation`], the
    // same one law our own avatar and the relay extrapolator call. Without the block's flags +
    // pitch — which we parsed and threw away until now — every observed swimmer entered the world
    // flat, and an *idle* floater (who sends no packets at all) stayed flat for as long as they
    // floated.
    let placement = match kind {
        EntityKind::GameObject => gameobject_rotation(go_quat, orientation),
        _ => mover.map_or_else(
            || wire_yaw(orientation),
            |m| crate::creature_anim::swim_body_rotation(orientation, m.flags, m.pitch),
        ),
    };
    // A unit/player created already ON a transport (deck NPCs stream in this way): its LIVING
    // block's rider tail is its local pose — `compose_riders` re-anchors it through the boat's
    // live matrix each frame (decision 0438 phase 2). The block's world `position` is the
    // spawn-time compose, kept as the pre-arm fallback pose.
    let rider = matches!(kind, EntityKind::Unit | EntityKind::Player)
        .then_some(transport)
        .flatten()
        .map(|t| crate::transport::TransportRider {
            transport_guid: t.guid,
            local_pos: [t.pos.x, t.pos.y, t.pos.z],
            local_orientation: t.orientation,
        });
    // Warm the name cache the moment a unit streams in. **This is the reference's own timing,
    // not a convenience**: `0x60afb0` ResolveDisplayInfo registers the unit in
    // the creature-query cache at `0x60b157`, on the create path (`0x5fb880` ← the `UPDATETYPE`
    // driver's per-type table) and behind `0x60b134 cmp [OBJECT_FIELD_TYPE], 0x9` — so every
    // creature it streams is asked for, whether or not anything ever shows its name. And for a
    // creature the name IS that record's field 0 (`0x60934b`), so template and name are one
    // query; only players use the separate guid-keyed ask. A `CreatureCache.wdb` hit answers it
    // with no wire traffic at all there, which our session-lifetime cache approximates.
    if matches!(kind, EntityKind::Unit | EntityKind::Player) {
        let _ = names.resolve(guid, net_commands);
        // …and for a **pet**, that ask is not the template ask. `resolve` routes a `HIGHGUID_PET`
        // guid to the pet-NAME query, because a pet guid carries a pet number where a creature's
        // carries its entry — so a tamed unit would never get a template record at all, and
        // everything read off one (`type_flags`, rank, creature type) would silently degrade for
        // it. The reference has no such split: its cache key is the DESCRIPTOR's entry
        // (`[[unit+8]+0xc]`), which vmangos fills with the real `cinfo->entry` for pets too. Ask
        // by that key as well; the ask-once discipline makes it free for every non-pet, whose
        // descriptor entry and guid entry are the same number.
        if let Some(entry) = fields.object_entry().filter(|&e| e != 0) {
            let _ = names.resolve_creature(entry, guid, net_commands);
        }
    }
    // Warm the lock cache the moment a GameObject streams in, so a
    // right-click resolves use-vs-cast instantly — the same ask-once, ask-at-sight
    // discipline as the name cache. The lockId isn't in the create packet; only the query
    // carries it.
    if matches!(kind, EntityKind::GameObject) {
        go_templates.request(guid, net_commands);
        // Where and how big, the moment it streams in — the readout that answers "is this prop in
        // the wrong place, or the wrong size, or just drawn wrong" without a guess (
        // the duel flag read as huge and mislocated, and nothing in the client could say which).
        // `RUST_LOG=benilla_app::net::objects=debug`.
        debug!(
            "gameobject spawn: entry {:?} display {display_id:?} type {go_type:?} \
             pos [{:.2}, {:.2}, {:.2}] scale {scale}",
            fields.object_entry(),
            position[0],
            position[1],
            position[2],
        );
    }
    // The walk this unit is ALREADY on: its create block's live spline, joined at the
    // server's own progress along it. Traced before it is interpreted, so the `WOW_CREATE_SPLINE=off`
    // leg of the A/B still records what the wire offered.
    trace_create_spline(guid, spline.as_ref());
    let walk = spline.and_then(create_spline);
    if let Some(&e) = index.0.get(&guid) {
        // Re-create of a tracked guid: refresh identity + pose. A create is a fresh server snapshot, so
        // any in-flight extrapolation is stale too — clear it.
        commands.entity(e).insert(net).remove::<RemoteMotion>();
        // Speeds land straight on the entity (the drain's staging maps are gone):
        // a fresh create inserts them as a component at spawn, a re-create replaces them whole
        // (below). A `SMSG_FORCE_*_SPEED_CHANGE` riding the same tick as this create still lands
        // on top of it, because each handler's commands are applied before the next packet's
        // runs — 1478's law unchanged (`apply::seam_tests`).
        if let Some(s) = speeds {
            // A create is the server's newest snapshot of the mover: it replaces the set whole,
            // and lands before the next packet's handler reads it.
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
        // The snapshot's own path outranks whatever we were riding; a create *without* one says the
        // unit is standing still, so a stale path goes.
        match walk {
            Some(s) => {
                commands.entity(e).insert(s);
            }
            None => {
                commands.entity(e).remove::<Spline>();
            }
        }
        write_pose(commands, transforms, e, position, placement);
        // Overlay the fresh snapshot's descriptor fields onto the existing store — the reference's
        // in-place refresh of a live guid, which notifies its field watchers like any delta.
        merge_fields(commands, stores, edges, e, guid, fields);
    } else {
        // A transport spawns hidden: its create pose is the *stationary* spawn point (or worse,
        // the origin), not where the boat is in its cycle — the transport tick unhides it at the
        // first sampled pose.
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
        // Chain-only visibility (benilla_world::vis_chain): the net root renders nothing —
        // its model parts and joints are the children, and the cull authorities flip this
        // root's `Visibility` through `Mut` writes, which don't re-add the sweep row.
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
        // The seed itself is never merged, which is the reference's create-time notify-suppress.
        entity.insert(ObjectStore(fields));
        index.0.insert(guid, entity.id());
    }
}

/// An item or container entered our view (`SMSG_UPDATE_OBJECT` descriptor-only create): an
/// object like any other — an entity in the index carrying its store, with no
/// pose and no model. A re-create of a live guid overlays the snapshot through the values path,
/// which notifies the field watchers like any delta (the scene create's own rule).
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
        // The item arrived: replay the enchant times queued for it while it was not held
        // (`0x5ebde0`), each through the setter as of now.
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

/// An existing object moved to a new authoritative pose (an `SMSG_UPDATE_OBJECT` movement block) —
/// a one-off correction/relocation that supersedes any active path.
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
        // A movement block carries a yaw and nothing else — the only GameObjects that get one are
        // transports, whose pose the transport tick owns from the next frame.
        write_pose(commands, transforms, e, position, wire_yaw(orientation));
    }
}

/// A relayed player movement packet (`MSG_MOVE_*`): the mover's authoritative pose + live move
/// flags. The reference SCHEDULES a remote's apply (`0x618c30`): the mover's own
/// replay chain gives the packet a client fire-time
/// ([`crate::net::motion::RelayMove`] → `RelayChain::schedule`); an already-due move
/// applies now, a future one queues on the unit and fires in `drain_pending_moves` — the dead-reckon
/// covering the mover's own timeline in between, which is what kills the arrival-jitter snap.
/// `WOW_REMOTE_SNAP=1` restores raw apply-at-arrival for an A/B.
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
    // Addressed to US: the server writing our own pose, never an echo of ours (every one is
    // `SetAsServerSide`, `ctime = 0`). The reference APPLIES it — there is no mover-guid gate
    // anywhere on its inbound move path, and the local player resolves through the same object
    // lookup as anyone else (`0x603bb0`). What is ours is only
    // *where it goes*: our avatar's motion source is the controller, not [`RemoteMotion`], so the
    // pose crosses to `player::wire_in` instead of down this lane.
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
        // No entity for this guid — the packet changes nothing. Traced rather than dropped in
        // silence: a mover that keeps running while these pile up is a streaming bug, not a replay
        // one, and the two look identical from the outside.
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
        // The server is authoritative now, not any creature path — drop a stale spline.
        commands.entity(e).remove::<Spline>();
        if let Ok(mut rm) = remote_motion.get_mut(e) {
            // The chain reads the mover's state as it stands BEFORE this move applies — the
            // reference times the packet off the live `[esi+0x40]`/`[esi+0x150]`.
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
                // Fire-times are monotone per unit by construction (a chained fire never lands
                // before its predecessor), so the queue stays ordered.
                rm.pending.push_back(PendingMove { fire_ms, mv });
            }
        } else {
            // First move for this unit: apply immediately (the chain seeds on it — its own fire
            // is arrival — and paces from the next packet on) and place it this frame; the
            // component insert is deferred, so the extrapolator won't see it until next frame.
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

/// A descriptor delta (`SMSG_UPDATE_OBJECT` values block): merge into the object's store in place
/// — a unit's, a GameObject's, an item's, one path. An unknown guid — a `Values`
/// with no create seen — is dropped, as before.
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

/// The object ceased to exist (`SMSG_DESTROY_OBJECT` — corpse decay ahead of respawn, a despawn).
fn object_destroyed(guid: u64, commands: &mut Commands, index: &mut GuidIndex) {
    // **The object goes away by the same fade its stream-out takes** — `DespawnFade`, not a raw
    // despawn. The reference's object-manager destroy hands the object's *model*
    // to the `SWModelFadeout` scheduler on the way out: the base OnDeactivate `0x6145e0` (vtable
    // slot 1, which `0x464920` invokes on **both** DESTROY and OUT_OF_RANGE) unbinds the scene
    // handle and calls `0x672df0`, which keeps the detached model drawing and ramps its alpha to
    // zero. So "the object is freed instantly" and "the model fades" are both true, one hop
    // apart — and the paragraph that used to stand here read the first as the whole story,
    // because it went looking for a `FadeTo` on the OBJECT and found none.
    //
    // That is why a looted chest pops: the chest's model authors no `Despawn` sequence, so the
    // announced despawn animation produces nothing and there was nothing left but the pop. The
    // fade is not the animation; it is what every teardown does underneath it.
    //
    // The guid leaves the index either way — to the server it no longer exists, and a respawn
    // streams in as a fresh entity that fades in over the top. **The object ends here, not when
    // its model finishes fading** ([`tear_down`]): the entity sheds its [`Guid`] in this same
    // handler's command flush, so every consumer that means "a live object" stops seeing it at
    // once — and if it was the target, the ring's gone-object branch clears the selection and
    // sends `CMSG_SET_SELECTION 0` on its next pass, this frame, as the reference's OnDeactivate
    // does at the teardown itself (`0x5fbb60` → `0x493910`).
    //
    // …*unless the object is pinned* — `0x464920` on a still-pinned object only sets the
    // pending-destroy bit and returns, and the real free waits for the last pin to drop
    // (`0x468410`). The one pin benilla takes is the despawn animation
    // announced a moment earlier by `SMSG_GAMEOBJECT_DESPAWN_ANIM`, which is the whole of how an
    // object gets to play its own despawn after the server says it is gone; the
    // fade then follows the animation, where the deferred destroy — and so the teardown — runs
    // ([`crate::go_anim::release_despawn_pin`]). A pinned object is still an object until then.
    if let Some(e) = index.0.remove(&guid) {
        // An item has no model to hand the fadeout, so it goes now — what the
        // scheduler does with a modelless entity anyway, one frame later — and its countdown
        // cells go with it (2340).
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

/// Stream-out (out-of-range, the update-object `OutOfRange` block): the unit still exists, we just
/// left its range.
fn objects_removed(guids: Vec<u64>, commands: &mut Commands, index: &mut GuidIndex) {
    // Don't pop the entity, fade it out, then despawn (`apply_despawn_fade` drives the ramp; an
    // entity with no fadeable geometry pops straight out there). Director-verified look: on the
    // reference, distant mobs fade out, never blink out (0067's open question, settled by their
    // eyes). The mechanism behind it is the same one [`object_destroyed`] above now takes — the
    // OUT_OF_RANGE block and DESTROY reach `0x464920` alike, and its OnDeactivate hands the model
    // to the `SWModelFadeout` scheduler either way ([`DespawnFade`]). That the two
    // routes agree is not a convenience here; it is the reference's own shape — and so is the
    // object ending at the stream-out rather than when the fade does ([`tear_down`]): a unit that
    // walks out of range, vanishes or stealths stops being targetable, TAB-able and hoverable now.
    for g in guids {
        if let Some(e) = index.0.remove(&g) {
            commands.entity(e).queue(tear_down);
        }
    }
}

/// **An object's teardown** — the reference's `0x464920`, which both `SMSG_DESTROY_OBJECT` and
/// the OUT_OF_RANGE block reach: the OBJECT is freed on the spot, and only its detached MODEL
/// survives, handed to the `SWModelFadeout` scheduler to ramp out ([`DespawnFade`], decision
/// 2198). The OnDeactivate on the way (`0x5fbb60` → `0x493910`) clears the selection if it was
/// this unit and sends `CMSG_SET_SELECTION 0` — at the teardown, not two seconds later.
///
/// benilla keeps the model on the same entity rather than re-parenting it onto a fresh one, so
/// "the object is gone" is said by taking away the one component that makes an entity an object:
/// its [`Guid`], the server identity every object consumer keys on — the TAB scan, `/target`,
/// the mouseover pick, the name and nameplate walks, the minimap blips, the quest markers, the
/// chat bubbles, the selection ring's gone-object branch, the unit feed's "left the manager"
/// gate. What the model needs to finish drawing stays: the [`NetEntity`] the renderer reads, and
/// the [`ObjectStore`] the animation driver reads — a corpse that decays keeps lying dead while
/// it fades instead of reading as alive and standing up. The guid has already left the
/// [`GuidIndex`] (the caller's job), so a same-tick re-create of it (corpse → respawn) spawns a
/// fresh object beside the fading model, and nothing ever sees two objects with one guid.
///
/// Every teardown goes through here: both wire routes, and a pinned object's deferred destroy
/// ([`crate::go_anim::release_despawn_pin`]).
pub(crate) fn tear_down(mut ent: EntityWorldMut) {
    ent.remove::<Guid>();
    ent.insert(DespawnFade::default());
}

/// A creature path packet (`SMSG_MONSTER_MOVE`, or its deck twin `SMSG_MONSTER_MOVE_TRANSPORT`):
/// settle the unit's transport membership, apply the dictated facing snap, then attach or clear
/// the travel spline.
///
/// **`transport` changes the frame of everything else in the packet**. When it is
/// `Some`, `start`, every `path` point and an `Angle` facing are offsets in that transport's frame,
/// the unit becomes (or stays) a [`TransportRider`] on it, and the spline is sampled into the
/// rider's local pose for `transport::compose_riders` to carry out to the world. When it is `None`
/// on a unit we had riding, the unit has *left* the deck — vmangos drops it from the transport on
/// exactly this edge (`MoveSplineInit::Launch`, `spline/MoveSplineInit.cpp:156-159`).
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
        // The DESYNC readout: how far this packet is about to teleport the unit — the
        // gap between where we have been drawing it and where the server says the path begins. A
        // correctly-followed creature reads ~0; a frozen one reads the whole walk it slept through.
        trace_move_snap(
            guid,
            // Both sides of the readout have to be in ONE frame. A deck packet's `start` is a
            // transport-local offset, so it is compared against the rider's own local pose, not
            // against the composed world position (which would read as the whole boat's travel).
            match transport {
                Some(_) => riders.get(e).ok().map(|r| r.local_pos),
                None => transforms.get(e).ok().map(|t| bevy_to_wow(t.translation)),
            },
            start,
            stop,
            duration_ms,
        );
        // **Transport membership, settled before anything reads a pose.** A packet naming a
        // transport attaches the unit to it (and re-seeds its deck-local pose from the packet's
        // own `start`, so the stop form places a body too); a packet naming none detaches a unit
        // we had riding. The facing that goes with it is deck-local for an `Angle` — vmangos runs
        // `CalculatePassengerOffset(…, &args.facing.angle)` on it (`MoveSplineInit.cpp:88`) — but
        // a `Spot`/`Target` facing is **not** converted server-side, so those resolve to a world
        // bearing and are rebased here by the deck's own yaw.
        //
        // **Above the root gate deliberately.** The gate below refuses the *path*, which is what
        // the reference establishes; membership is not a path. A rooted body on a deck that was
        // left unattached would be abandoned in the sea as the boat sails out from under it, which
        // is a worse failure than carrying a pinned body along with the deck it is standing on.
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
                    // The unit's own WORLD position anchors the bearing (the wire's `start` is
                    // deck-local and would put the unit near the map origin).
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
        // Apply the dictated final facing (moveType 2/3/4) as a **snap** — faithful to the
        // client, which stores it straight into the unit's **raw** movement facing (`0x7c6f30`).
        // This is the *packet*-driven re-face — a scripted/emote/aggro `SetFacingTo` the
        // server actually sends. (The client-local re-faces — squaring up on a target, and
        // turning to face you while an interaction window is open — carry no packet at all and
        // are `motion::drive_display_facing`'s, decision 1467.) Because it is the raw facing
        // that moved, the display smoother's state goes with it: the unit re-seeds from this
        // pose instead of swinging back to the heading it was snapped off. When a real path
        // follows, `sample_splines` overwrites the rotation with the travel direction each
        // frame (faithful — the client's spline-follow snaps the mesh yaw to the path tangent,
        // `0x7c5490`). The receipt snap thus only sticks for a path-less move (a
        // `Stop`/in-place re-face); a moving unit ends on its last tangent.
        // The world-space facing snap — for a deck packet the rider's `local_orientation` above is
        // the one that counts, and `compose_riders` writes the rotation from it.
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
        // A spline move **un-nocks and drops the weapon-visual hold**, unconditionally:
        // `0x6018f0` (this packet's handler, via `0x603f00` registered on opcodes 0xDD/0x2AE) has
        // exactly one `ret`, and its shared tail runs `0x6020e8 call 0x60d040` (clear `0x400`) →
        // `0x6020ef call 0x60f530` (un-nock) → `RecomputeBaseAnim(-1)` on every path through it.
        // So an archer NPC yanked along a path, or a player charged/knocked back, loses the arrow —
        // the ranged sheath is untouched, exactly like the locomotion un-nock.
        commands.entity(e).remove::<(
            crate::creature_anim::NockLatch,
            crate::creature_anim::RangedHold,
        )>();
        // **A rooted unit cannot be splined**. `0x6187a0` — the *server
        // position/spline apply*, and the sole path from this packet's parse chain into
        // `CMovement`'s spline installer `0x7c6a50` — opens with `0x6187c2 test ah,0x10` and
        // returns on ROOT: a rooted unit cannot translate, cannot jump, cannot be splined.
        //
        // **Scoped to the path deliberately.** What is settled is that the *position/spline*
        // apply is refused; whether the rest of `0x6018f0` still runs for a rooted unit is not
        // known, and the one part of it we do know — the un-nock tail — runs on **every** path
        // through the handler (`0x6020ef`), so it stays above this gate. The facing snap is left
        // running for the same reason: refusing more than the reference establishes would be
        // inventing a behaviour, and refusing the path is what keeps the body still.
        //
        // On vmangos this is a race rather than the common case — `Unit::SetRooted(true)` calls
        // `StopMoving()` before the root goes out, so an already-walking creature is normally
        // stopped by its own stop-spline. The gate is what keeps a path the server had already
        // queued from moving a body it has since pinned.
        if rooted {
            return;
        }
        // The spline is authoritative now, not the relay stream — the mirror of `unit_move`'s
        // "drop a stale spline". A splined PLAYER (charge, taxi) otherwise keeps a stale
        // `RemoteMotion` whose old flags outrank the spline in the anim selector's `unify`
        // precedence (Stand frozen mid-flight); the relay re-seeds it on its next packet. Below the
        // root gate because it is the *install's* own consequence: with no path taken there is no
        // new authority to hand the pose to.
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
            // A moving path: sample_splines drives the transform along every waypoint.
            Some(spline) => {
                commands.entity(e).insert(spline).remove::<SplineStopped>();
            }
            // Stop/clear: freeze where the last sample left it (≈ the endpoint) — and keep the id,
            // which the server is waiting to hear back for a player-driven unit.
            None => {
                commands
                    .entity(e)
                    .remove::<Spline>()
                    .insert(SplineStopped(spline_id));
            }
        }
    }
}

/// This unit's granted modes as of now — the live component, else none. A grant lands before
/// the next packet's handler reads it. The reference's word lives as long as the
/// `CGUnit`, so "no component" and "all bits clear" are the same answer and both are
/// [`UnitMoveModes::default`].
fn modes_of(guid: u64, index: &GuidIndex, modes: &Query<&mut UnitMoveModes>) -> UnitMoveModes {
    index
        .0
        .get(&guid)
        .and_then(|&e| modes.get(e).ok().copied())
        .unwrap_or_default()
}

/// **A movement mode granted on a unit we do not control** — the `SMSG_SPLINE_MOVE_*` family.
/// No ack, and `guid` is whatever unit the server named: normally a creature, which
/// is exactly the body the ack'd `SMSG_FORCE_*` family structurally cannot address.
///
/// An unresolvable guid is **dropped**, faithfully: the reference's handler `0x603c80` resolves with
/// `ClntObjMgrObjectPtr(TYPEMASK_UNIT)` and returns without applying anything when it misses.
///
/// Our own guid is not special-cased. vmangos cannot send it for our mover (`Unit::SetRooted` and
/// its three siblings take the broadcast leg only when the unit is **not** moved by a player), and
/// the reference applies to whatever it finds — so we do too. It is inert on the avatar either way:
/// the animation selector's `unify` gives the controller's own `MovementState` precedence, and our
/// mover's modes are the handshake family's ([`crate::player::state::MoveModes`]).
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
        return; // no such unit — the reference drops it too
    };
    let mut word = modes_of(guid, index, modes);
    word.set(mode, apply);
    if let Ok(mut live) = modes.get_mut(e) {
        *live = word;
    } else {
        commands.entity(e).insert(word);
    }

    // **`SetRoot 0x7c7340`'s one-shot wipe**, on the one benilla body that has direction bits to
    // wipe — a relayed player's. The reference clears `0xffe07f00`'s complement from the flags word
    // at apply and calls `0x7c6290` StopFalling with it; with the direction bits gone the unit fails
    // the integration gate (`move_flags::INTEGRATED`) and is not stepped at all, which is *how* a
    // root stops a body rather than leaving it coasting on its last reported heading until the next
    // packet. Without this the dead-reckon keeps walking a rooted player forward for the ~500 ms to
    // its next heartbeat.
    //
    // A creature needs none of it: it has no direction bits (its motion is its [`Spline`]), and what
    // stops *it* is `monster_move`'s refusal above.
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

/// The ask-once GameObject template (`SMSG_GAMEOBJECT_QUERY_RESPONSE`): cache it and
/// resolve the lockId from the type-specific `data[]` slot — the interact routing reads it to choose
/// use-vs-cast; the hover tooltip reads the name (decision 0276's GO law).
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

/// Merge a descriptor delta into an object's live store. The `else` — in the index but with no
/// store — should not happen (a create inserts the store at spawn, and lands before the next
/// packet's handler), but seeds defensively rather than drop the delta.
///
/// Every merge reports its field edges ([`FieldChanged`]): a create and a values
/// delta for the same guid are two wire blocks, and the reference notifies on the second.
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

/// Write one speed-kind slot of a mover's live [`UnitSpeeds`]. `false` means there is nowhere to
/// write — an untracked guid, or a mover whose create carried no movement block — and the change
/// is reported unapplied rather than silently dropped.
///
/// The two speed sources used to disagree about *when* they land: a
/// create's whole set was inserted through `Commands`, deferred to the drain's sync point, while
/// a `SMSG_FORCE_*_SPEED_CHANGE` edited the live component at once — so the later packet lost
/// either way when vmangos put the two back to back (`HandleMoveWorldportAckOpcode`'s self create
/// and the mount strip three statements later). Under a handler per packet the create's insert
/// lands before the change's handler runs, so both write the component and the
/// wire's order is the order.
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

/// A forced speed change on a mover (aura/mount/GM `.modify speed`): stage the new value onto the
/// entity's speed set, and — when the mover is our own player — forward to the controller, which
/// answers the mandatory ack with its live pose (the TeleportMessage pattern). An unknown guid
/// still acks if it's ours-by-guid; a foreign mover (we never control others) is only applied,
/// never acked — acking a unit we don't control is the server's error path.
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
        // Our own mover having no speed set to write is B213's failure shape, and it was silent
        // for as long as it existed. It should be unreachable now that the create stages too —
        // so if it ever fires, the next report starts from a line instead of from a guess.
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

/// An observed unit's speed changed (the SPLINE_SET / MOVE_SET families — another player
/// mounting up, a hastened creature): stage it onto that unit's speed set, nothing to ack.
/// The MOVE_SET flavour's pose already arrived as its own UnitMove.
fn speed_changed(
    guid: u64,
    kind: SpeedKind,
    speed: f32,
    index: &GuidIndex,
    speeds: &mut Query<&mut UnitSpeeds>,
) {
    if !set_speed(guid, kind, speed, index, speeds) {
        // Ordinary: a unit's speed broadcast can outrun its create block. Nothing to ack and the
        // create that follows carries the same set, so this self-heals.
        debug!("net: {kind:?} speed change for untracked mover {guid:#x} — nothing to write");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **An item's countdowns are its own cells**, end to end through the real
    /// registration on the built client: `0x1EB` for an item not yet held waits on the active
    /// player and is replayed into the item's cells when its create lands, `0x1EA` for one not
    /// held is dropped for good, a refused slot writes nothing, and the destroy takes the cells
    /// with the object. Without an active player to queue on, the enchant update is dropped too.
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

        // With the active player resolving, it waits for the item — and the lifetime does not.
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

    /// The observer movement-mode family's own harness: one indexed unit, and a
    /// `World` small enough that the only thing that can move it is the code under test.
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

        /// Run one grant through the real arm body, then flush the `Commands` it queued.
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

        /// **The grant lands, and the revoke takes it back off** — the whole family in one shape,
        /// because every one of the twelve opcodes is this same set-or-clear of one bit.
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

        /// **`SET_RUN_MODE` clears the walk bit.** The pair is inverted against its opcode names
        /// (`0x617e80` → `SetRunMode 0x7c71c0`, whose argument is *run*), folded at the parse — so
        /// by the time it reaches here `apply` is the bit's direction and this is a plain round
        /// trip. Pinned anyway: the inversion is the family's one trap, and a regression that
        /// flipped it would put every running creature into a walk with nothing else visibly wrong.
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

        /// **A root stops a watched body dead, and that is the direction bits' doing.** `SetRoot
        /// 0x7c7340` sets the bit and then wipes `0xffe07f00`'s complement — the four direction
        /// bits among it — in one shot; with those gone the unit fails the client's integration
        /// gate (`0x616e20 test dword [esi+0x40],0x20ff`) and is not stepped at all. Without the
        /// wipe our dead-reckon walks a rooted player forward for the ~500 ms to its next
        /// heartbeat, which is the reported "rooted and still sliding".
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

        /// The unroot is **not** the wipe run backwards: `ClearRoot 0x7c7370` only clears the bit.
        /// A unit is put back in motion by the server's next pose or path, never by us inventing
        /// direction bits for it.
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

        /// **An unknown guid is dropped, not defaulted.** The reference's handler `0x603c80`
        /// resolves with `ClntObjMgrObjectPtr(TYPEMASK_UNIT)` and returns having applied nothing
        /// when it misses. Ours must not spawn or index anything to hold the grant: a mode
        /// broadcast can outrun its unit's create block, and the create carries the modes itself.
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

        /// **A rooted unit cannot be splined** — `0x6187a0`, the server position/spline apply,
        /// opens `0x6187c2 test ah,0x10` and returns. So a path the server queued before it pinned
        /// the body does not move it.
        ///
        /// The gate is scoped to the *path*, not to the packet: what the reference establishes is
        /// that the position/spline apply is refused, and the one other thing the handler is known
        /// to do (the un-nock tail, `0x6020ef`) runs on every path through it. The assertion below
        /// is therefore about the `Spline` and nothing else.
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

        /// **`SMSG_MONSTER_MOVE_TRANSPORT` attaches, and its plain twin detaches** (decision
        /// 1936). A packet naming a transport makes the unit a [`TransportRider`] on it, seeds the
        /// rider pose from the packet's own deck-local `start`, and hands the spline the deck so
        /// the sampler writes the local pose instead of the world transform. A packet naming none
        /// takes the unit off the deck, which is the edge vmangos itself drops the passenger on
        /// (`MoveSplineInit::Launch`, `spline/MoveSplineInit.cpp:156-159`).
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
            // The rider's world transform is `compose_riders`' to write, not this apply's — the
            // deck-local start must never have been mistaken for a world position.
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
