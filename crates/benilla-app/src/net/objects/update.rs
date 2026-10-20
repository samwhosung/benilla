//! One `SMSG_UPDATE_OBJECT` as the reference's handler runs it (`0x4651a0`): an out-of-range list
//! as the first block releases first; the write pass then handles every block in order, creating,
//! writing and bringing back objects; the notify pass reads every block again and runs the watches
//! and the create hooks; and each object the packet brought back gets its vtable slot 3 last
//! (`0x465290`-`0x4652ff`). The protocol hands each block over as its own event and closes the
//! packet with [`SessionEvent::ObjectUpdateEnd`], so the write pass runs block by block, each
//! block's commands applied before the next, and the notify pass runs at the end.
//!
//! The object manager keeps two tables (`0x464ff0`): the live one, here the [`GuidIndex`], and the
//! released one, [`Released`]. A release (`0x464920`) runs the object's deactivate, its vtable slot
//! 1, and moves it to the released table with its fields; a create, values, movement or
//! near-objects block for the guid brings it back (`0x464530`), and only `SMSG_DESTROY_OBJECT` or
//! a new object of its type frees it (`0x467521`, `0x466e55`). A brought-back object's model is
//! rebuilt (slot 2, `0x614640`); here it is a new entity.

use std::collections::{HashMap, HashSet, VecDeque};

use benilla_protocol::messages::ObjectType;
use benilla_protocol::{EntityKind, MoveSpeeds, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::super::motion::{pose_transform, wire_yaw};
use super::super::{
    write_fields, FieldChanged, Guid, NetEntity, NetHandlerApp, ObjectStore, UnitMoveModes,
    UnitSpeeds,
};
use super::{item_watch, object_create, object_move, CreateBlock, Landing, Scene};
use crate::field_watch::{Notified, Sent};
use crate::query_cache::Ask;
use crate::transport::TransportRider;
use benilla_world::vis_chain::VisChainOnly;

/// Registers the update packet's block handlers, its end, and `SMSG_DESTROY_OBJECT`.
pub(super) fn register(app: &mut App) {
    use benilla_protocol::SessionEventKind as K;
    app.init_resource::<UpdatePass>()
        .init_resource::<Released>()
        .init_resource::<ObjectList>()
        .net_handler(K::ObjectCreate, on_object_create)
        .net_handler(K::ItemCreate, on_item_create)
        .net_handler(K::ObjectValues, on_object_values)
        .net_handler(K::ObjectMove, on_object_move)
        .net_handler(K::ObjectsRemoved, on_objects_removed)
        .net_handler(K::ObjectsNear, on_objects_near)
        .net_handler(K::ObjectUpdateEnd, on_update_end)
        .net_handler(K::ObjectDestroyed, on_object_destroyed);
}

/// What one packet's write pass leaves for its notify pass; empty between packets.
#[derive(Resource, Default)]
pub(crate) struct UpdatePass {
    /// A block of this packet has been handled: an out-of-range list releases only as the
    /// first (`0x4651e1`).
    started: bool,
    /// The blocks the notify pass reads again, in packet order.
    blocks: Vec<Notify>,
    /// Each object's last write in this packet: the dwords it moved, `(index, copy, now)`. A
    /// write of a held object first copies every watched range of it (`0x46664b`-`0x4666b6`),
    /// so only the last write's copy reaches the notify pass.
    copies: HashMap<u64, Vec<(u16, u32, u32)>>,
    /// The objects this packet allocated whose create hook has not run: their `+0xe4` bit `0x4`,
    /// which the constructor clears (`0x613a6d`) and only the hook sets (`0x613af0`), is still
    /// clear (`0x614e90`).
    unshown: HashSet<u64>,
    /// The objects this packet brought back, in order: the list at the manager's `+0xb0`, each
    /// linked at its tail (`0x4646ed`).
    reactivated: Vec<u64>,
}

/// A placed object's place on the manager's object list (`+0xa4`), which `0x468380` walks from the
/// head (`+0xac`): a create (`0x4662cf`-`0x4662dd`) and a reactivation (`0x4646c3`-`0x4646d4`)
/// link it at the tail, so a larger number joined later. Only the players' order is read; items
/// and game objects leave the list and rejoin it on their own (`0x468270`, `0x4682f0`).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Listed(u64);

/// The object list's tail: the next [`Listed`] to hand out.
#[derive(Resource, Default)]
pub(crate) struct ObjectList(u64);

impl ObjectList {
    /// Links one more object at the tail.
    pub(super) fn link(&mut self) -> Listed {
        self.0 += 1;
        Listed(self.0)
    }
}

/// A block the notify pass reads again.
enum Notify {
    /// A values block (`0x465330`) and the fields its mask sent, ascending.
    Values { guid: u64, sent: Vec<u16> },
    /// A create block (`0x465c50`): an item's or a placed object's, whose kind picks its hook
    /// (the table `0x465e78`).
    Create { guid: u64, kind: Created },
}

#[derive(Clone, Copy)]
enum Created {
    Item,
    World(EntityKind),
}

/// The released table (`+0x28` of the manager): objects out of range, kept with their fields
/// until a block brings them back, a destroy names them, or a new object of their type takes
/// their place.
#[derive(Resource, Default)]
pub(crate) struct Released {
    objects: HashMap<u64, Kept>,
    /// Each type's released guids, oldest first: the reuse lists from the manager's `+0x50`,
    /// which a release appends to (`0x464a83`-`0x464ac4`).
    reuse: HashMap<u8, VecDeque<u64>>,
}

/// A released object as it left.
struct Kept {
    fields: ObjectFields,
    body: Body,
}

enum Body {
    /// A placed object: its identity, pose, speeds, the movement modes that live as long as the
    /// unit (`0x7c4850`), and the deck it rides.
    World {
        kind: EntityKind,
        display_id: Option<u32>,
        scale: f32,
        pose: Transform,
        speeds: Option<MoveSpeeds>,
        modes: Option<UnitMoveModes>,
        rider: Option<TransportRider>,
    },
    /// An item or a container, with its countdown cells.
    Item {
        container: bool,
        countdowns: crate::items::Countdowns,
    },
}

impl Kept {
    fn type_id(&self) -> u8 {
        match self.body {
            Body::World { kind, .. } => type_id(world_type(kind)),
            Body::Item { container, .. } => type_id(item_type(container)),
        }
    }
}

impl Released {
    fn insert(&mut self, guid: u64, kept: Kept) {
        self.reuse
            .entry(kept.type_id())
            .or_default()
            .push_back(guid);
        self.objects.insert(guid, kept);
    }

    /// Takes `guid` out of the table and its type's reuse list (`0x464635`, `0x46464c`).
    fn take(&mut self, guid: u64) -> Option<Kept> {
        let kept = self.objects.remove(&guid)?;
        if let Some(list) = self.reuse.get_mut(&kept.type_id()) {
            list.retain(|&g| g != guid);
        }
        Some(kept)
    }

    /// A new object of type `type_id` frees the oldest released one of that type (`0x466e55`,
    /// the head of the list).
    fn free_oldest(&mut self, type_id: u8) {
        if let Some(guid) = self.reuse.get_mut(&type_id).and_then(VecDeque::pop_front) {
            self.objects.remove(&guid);
        }
    }

    /// Frees every released object, as leaving the world does (`0x467700`).
    pub(crate) fn clear(&mut self) {
        self.objects.clear();
        self.reuse.clear();
    }
}

/// The type id that picks a reuse list (`0x466e50`): 1 item to 7 corpse.
fn type_id(ty: ObjectType) -> u8 {
    match ty {
        ObjectType::Object => 0,
        ObjectType::Item => 1,
        ObjectType::Container => 2,
        ObjectType::Unit => 3,
        ObjectType::Player => 4,
        ObjectType::GameObject => 5,
        ObjectType::DynamicObject => 6,
        ObjectType::Corpse => 7,
    }
}

fn world_type(kind: EntityKind) -> ObjectType {
    match kind {
        EntityKind::Player => ObjectType::Player,
        EntityKind::Unit => ObjectType::Unit,
        EntityKind::GameObject => ObjectType::GameObject,
        EntityKind::DynamicObject => ObjectType::DynamicObject,
        EntityKind::Corpse => ObjectType::Corpse,
        EntityKind::Other => ObjectType::Object,
    }
}

fn item_type(container: bool) -> ObjectType {
    if container {
        ObjectType::Container
    } else {
        ObjectType::Item
    }
}

/// A placed object's create block (types 2 and 3, `0x4660a0`).
fn on_object_create(In(ev): In<SessionEvent>, mut sc: Scene) {
    let SessionEvent::ObjectCreate {
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
    else {
        return;
    };
    sc.pass.started = true;
    crate::death::net::note_corpse(guid, kind, &fields, &sc.self_guid, &mut sc.death_net);
    let block = CreateBlock {
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
    };
    // The lookup that brings a released object back (`0x46612d` → `0x464530`).
    let live = sc.index.0.get(&guid).copied();
    let kept = live.is_none().then(|| sc.released.take(guid)).flatten();
    let moved = match (live, kept) {
        (Some(e), _) => {
            object_create(&mut sc, block, &fields, Landing::Live(e));
            match sc.stores.get_mut(e) {
                Ok(mut store) => write_fields(&mut store.0, fields),
                Err(_) => {
                    sc.commands.entity(e).insert(ObjectStore(fields));
                    Vec::new()
                }
            }
        }
        (
            None,
            Some(Kept {
                fields: mut store,
                body,
            }),
        ) => {
            let moved = write_fields(&mut store, fields.clone());
            let (pose, modes) = match body {
                Body::World { pose, modes, .. } => (Some(pose), modes),
                Body::Item { .. } => (None, None),
            };
            // Only a unit takes the block's movement (`0x46638c`); any other type keeps its pose.
            let unit = matches!(kind, EntityKind::Unit | EntityKind::Player);
            let kept_pose = pose.filter(|_| !unit);
            let e = object_create(&mut sc, block, &fields, Landing::New { store, kept_pose });
            if let Some(modes) = modes {
                sc.commands.entity(e).insert(modes);
            }
            sc.pass.reactivated.push(guid);
            moved
        }
        (None, None) => {
            allocate(&mut sc, guid, world_type(kind));
            let store = fields.clone();
            object_create(
                &mut sc,
                block,
                &fields,
                Landing::New {
                    store,
                    kept_pose: None,
                },
            );
            sc.pass.unshown.insert(guid);
            Vec::new()
        }
    };
    sc.pass.copies.insert(guid, moved);
    sc.pass.blocks.push(Notify::Create {
        guid,
        kind: Created::World(kind),
    });
}

/// A new object of `ty` frees the oldest released one of its type (`0x466e55`); our own player is
/// allocated apart and frees nothing (`0x466e0c`).
fn allocate(sc: &mut Scene, guid: u64, ty: ObjectType) {
    if sc.self_guid.0 != Some(guid) {
        sc.released.free_oldest(type_id(ty));
    }
}

/// An item or container create block: an indexed entity with a store and no pose or model.
fn on_item_create(In(ev): In<SessionEvent>, mut sc: Scene) {
    let SessionEvent::ItemCreate {
        guid,
        container,
        fields,
    } = ev
    else {
        return;
    };
    sc.pass.started = true;
    debug!("net: item create {guid:#x} (container: {container})");
    let live = sc.index.0.get(&guid).copied();
    let kept = live.is_none().then(|| sc.released.take(guid)).flatten();
    let moved = match (live, kept) {
        (Some(e), _) => match sc.stores.get_mut(e) {
            Ok(mut store) => write_fields(&mut store.0, fields),
            Err(_) => Vec::new(),
        },
        (
            None,
            Some(Kept {
                fields: mut store,
                body,
            }),
        ) => {
            let moved = write_fields(&mut store, fields);
            let e =
                crate::items::spawn_item(&mut sc.commands, &mut sc.index, guid, store, container);
            if let Body::Item { countdowns, .. } = body {
                sc.commands.entity(e).insert(countdowns);
            }
            sc.pass.reactivated.push(guid);
            moved
        }
        (None, None) => {
            allocate(&mut sc, guid, item_type(container));
            let e =
                crate::items::spawn_item(&mut sc.commands, &mut sc.index, guid, fields, container);
            // Replay the enchant times queued while it was not held (`0x5ebde0`), as of now.
            let queued = sc.items.take_enchant_times(guid);
            if !queued.is_empty() {
                let mut countdowns = crate::items::Countdowns::default();
                for (slot, seconds) in queued {
                    if !countdowns.set_enchant(slot, seconds) {
                        debug!("enchant timer: queued slot {slot} for {guid:#x} is past the seventh — refused");
                    }
                }
                sc.commands.entity(e).insert(countdowns);
            }
            sc.pass.unshown.insert(guid);
            Vec::new()
        }
    };
    sc.pass.copies.insert(guid, moved);
    sc.pass.blocks.push(Notify::Create {
        guid,
        kind: Created::Item,
    });
}

/// A values block (`0x467160`): written into a held object, or one it brings back; a guid the
/// manager knows neither way is skipped (`0x465bb0`).
fn on_object_values(In(ev): In<SessionEvent>, mut sc: Scene) {
    let SessionEvent::ObjectValues { guid, fields } = ev else {
        return;
    };
    sc.pass.started = true;
    // Our corpse's `CORPSE_FIELD_FLAGS` can flip to BONES under a live guid; the reclaim latch is
    // re-asked on that edge, as the reference's `FLAGS` handler does (`0x5d6d60`).
    crate::death::net::recheck_corpse(guid, &fields, &sc.self_guid, &mut sc.death_net);
    let sent: Vec<u16> = fields.raw_fields().map(|(index, _)| index).collect();
    let moved = match sc.index.0.get(&guid).copied() {
        Some(e) => sc
            .stores
            .get_mut(e)
            .ok()
            .map(|mut store| write_fields(&mut store.0, fields)),
        None => sc.released.take(guid).map(
            |Kept {
                 fields: mut store,
                 body,
             }| {
                let moved = write_fields(&mut store, fields);
                respawn(&mut sc, guid, body, store, None);
                moved
            },
        ),
    };
    if let Some(moved) = moved {
        sc.pass.copies.insert(guid, moved);
    }
    sc.pass.blocks.push(Notify::Values { guid, sent });
}

/// A movement block (`0x467050`): a held object's new pose, or one it brings back placed there.
fn on_object_move(In(ev): In<SessionEvent>, mut sc: Scene) {
    let SessionEvent::ObjectMove {
        guid,
        position,
        orientation,
    } = ev
    else {
        return;
    };
    sc.pass.started = true;
    // The unit this client moves drops its block before the lookup that would bring a released
    // one back (`0x4670a5`-`0x4670bb`, ahead of `0x4670c2`); a held one is `object_move`'s test.
    if !sc.index.0.contains_key(&guid) {
        if active_mover(&sc) == Some(guid) {
            return;
        }
        if let Some(Kept { fields, body }) = sc.released.take(guid) {
            let pose = pose_transform(position, wire_yaw(orientation));
            respawn(&mut sc, guid, body, fields, Some(pose));
        }
        return;
    }
    object_move(
        guid,
        position,
        orientation,
        &mut sc.commands,
        &sc.index,
        &mut sc.transforms,
        &sc.movers,
    );
}

/// The active-mover global (`0xc4da98`) by guid, as [`crate::player`] seats it: the claimed
/// foreign mover, else our own, and none while control is lost (`0x5fa600` zeroes it).
fn active_mover(sc: &Scene) -> Option<u64> {
    let player = sc.player.as_ref().filter(|p| !p.control_lost)?;
    player.foreign_mover.or(sc.self_guid.0)
}

/// A near-objects block (`0x467230`): each listed released object but our own player comes back
/// (`0x467283`, `0x4644f0`).
fn on_objects_near(In(ev): In<SessionEvent>, mut sc: Scene) {
    let SessionEvent::ObjectsNear(guids) = ev else {
        return;
    };
    sc.pass.started = true;
    for guid in guids {
        if sc.self_guid.0 == Some(guid) || sc.index.0.contains_key(&guid) {
            continue;
        }
        if let Some(Kept { fields, body }) = sc.released.take(guid) {
            respawn(&mut sc, guid, body, fields, None);
        }
    }
}

/// A released object back in the live table off a values, movement or near-objects block: a new
/// entity with what it kept, `store` its fields now and `pose` the block's, if any.
fn respawn(sc: &mut Scene, guid: u64, body: Body, store: ObjectFields, pose: Option<Transform>) {
    match body {
        Body::Item {
            container,
            countdowns,
        } => {
            let e =
                crate::items::spawn_item(&mut sc.commands, &mut sc.index, guid, store, container);
            sc.commands.entity(e).insert(countdowns);
        }
        Body::World {
            kind,
            display_id,
            scale,
            pose: kept_pose,
            speeds,
            modes,
            rider,
        } => {
            let mut entity = sc.commands.spawn((
                Guid(guid),
                NetEntity {
                    kind,
                    display_id,
                    scale,
                },
                pose.unwrap_or(kept_pose),
                Visibility::default(),
                ObjectStore(store),
                sc.list.link(),
            ));
            entity.vis_chain_only();
            if let Some(s) = speeds {
                entity.insert(UnitSpeeds(s));
            }
            if let Some(m) = modes {
                entity.insert(m);
            }
            if let Some(r) = rider {
                entity.insert(r);
            }
            let e = entity.id();
            sc.index.0.insert(guid, e);
        }
    }
    sc.pass.reactivated.push(guid);
}

/// The out-of-range block. As a packet's first block it releases each listed object, our own
/// player excepted (`0x465f39`-`0x465f45`, the player's guid outside the second round's types at
/// `0x465f19`, `0x465f28`); anywhere else the notify pass skips it (`0x465fd0`) and nothing is
/// released.
fn on_objects_removed(In(ev): In<SessionEvent>, mut sc: Scene, script: Option<NonSend<UiScript>>) {
    let SessionEvent::ObjectsRemoved(guids) = ev else {
        return;
    };
    let first = !std::mem::replace(&mut sc.pass.started, true);
    if !first {
        return;
    }
    for guid in guids {
        if sc.self_guid.0 == Some(guid) {
            continue;
        }
        deactivate(&mut sc, guid, script.as_deref());
        release(&mut sc, guid);
    }
}

/// `SMSG_DESTROY_OBJECT` (`0x4674a0`): the release, then the object freed at once, a released one
/// included (`0x467521`). The object is freed now and its model fades out, as the deactivate
/// (`0x6145e0`) hands the model to the `SWModelFadeout` scheduler (`0x672df0`); the guid leaves
/// the index at once ([`super::tear_down`]).
///
/// A pinned object only gets its pending-destroy bit and is freed when the last pin drops
/// (`0x464920`, `0x468410`). Our one pin is an announced `SMSG_GAMEOBJECT_DESPAWN_ANIM`, whose
/// end runs the deferred teardown ([`crate::go_anim::release_despawn_pin`]).
fn on_object_destroyed(In(ev): In<SessionEvent>, mut sc: Scene, script: Option<NonSend<UiScript>>) {
    let SessionEvent::ObjectDestroyed(guid) = ev else {
        return;
    };
    crate::death::net::forget_corpse(guid, &mut sc.death_net);
    // A released object ran its deactivate as it left.
    if sc.released.take(guid).is_some() {
        return;
    }
    deactivate(&mut sc, guid, script.as_deref());
    let Some(e) = sc.index.0.remove(&guid) else {
        return;
    };
    // An item has no model to fade, so it goes now with its countdown cells.
    if benilla_protocol::guid::is_item(guid) {
        sc.commands.entity(e).despawn();
        return;
    }
    sc.commands.queue(move |world: &mut World| {
        if world
            .get::<crate::go_anim::DespawnAnimAnnounced>(e)
            .is_some()
        {
            if let Ok(mut ent) = world.get_entity_mut(e) {
                ent.insert(crate::go_anim::PendingDestroy);
            }
        } else if let Ok(ent) = world.get_entity_mut(e) {
            super::tear_down(ent);
        }
    });
}

/// The deactivate a release runs (vtable slot 1, `0x46496f`), reading the descriptor about to go:
/// the item's (`0x5d8660`) and the party member's, which snapshots its record and sends
/// `CMSG_REQUEST_PARTY_MEMBER_STATS`.
fn deactivate(sc: &mut Scene, guid: u64, script: Option<&UiScript>) {
    let gone = item_watch(&sc.index, &sc.stores, &sc.self_guid, &sc.items).destroyed(guid);
    sc.item_notices.write_batch(gone);
    let held = |g: u64| sc.index.0.get(&g).and_then(|e| sc.stores.get(*e).ok());
    crate::ui_party::net::member_deactivated(
        guid,
        &mut sc.group,
        held(guid),
        held,
        &sc.names,
        script,
        &sc.net,
    );
}

/// `0x464920` past the deactivate: the object leaves the live table for the released one, its
/// fields and what it stood as kept, and its model fades out ([`super::tear_down`]).
fn release(sc: &mut Scene, guid: u64) {
    let Some(e) = sc.index.0.remove(&guid) else {
        return;
    };
    let kept = sc
        .stores
        .get(e)
        .ok()
        .map(|store| store.0.clone())
        .and_then(|fields| {
            let body = if let Ok(item) = sc.item_objects.get(e) {
                Body::Item {
                    container: item.container,
                    countdowns: sc.countdowns.get(e).cloned().unwrap_or_default(),
                }
            } else {
                let net = sc.nets.get(e).ok()?;
                Body::World {
                    kind: net.kind,
                    display_id: net.display_id,
                    scale: net.scale,
                    pose: sc.transforms.get(e).copied().unwrap_or_default(),
                    speeds: sc.speeds.get(e).ok().map(|s| s.0),
                    modes: sc.modes.get(e).ok().copied(),
                    rider: sc.riders.get(e).ok().map(|r| TransportRider {
                        transport_guid: r.transport_guid,
                        local_pos: r.local_pos,
                        local_orientation: r.local_orientation,
                    }),
                }
            };
            Some(Kept { fields, body })
        });
    if let Some(kept) = kept {
        sc.released.insert(guid, kept);
    }
    // An item has no model to fade, so its entity goes now; the table holds what it was.
    if sc.item_objects.contains(e) {
        sc.commands.entity(e).despawn();
    } else {
        sc.commands.entity(e).queue(super::tear_down);
    }
}

/// The end of the packet: the notify pass over every block (`0x46522d`-`0x46528b`), then each
/// brought-back object's vtable slot 3 (`0x465290`-`0x4652ff`).
fn on_update_end(In(ev): In<SessionEvent>, mut sc: Scene, script: Option<NonSend<UiScript>>) {
    let SessionEvent::ObjectUpdateEnd = ev else {
        return;
    };
    // The packet's own pass, taken whole: nothing here publishes to the VM, which only names a
    // party member's pet ([`compare_party_record`]).
    let UpdatePass {
        blocks,
        copies,
        mut unshown,
        reactivated,
        ..
    } = std::mem::take(&mut *sc.pass);
    // A unit's release drops its token flags and with them its field bridge (`0x5fbc44` →
    // `0x60c4f0(-1)` → `0x51bdb0`), which only its slot 3 adds back (`0x5fbe36` → `0x60c520`),
    // after this pass: an object this packet brought back notifies with no bridge.
    let away: HashSet<u64> = reactivated.iter().copied().collect();
    let none = Vec::new();
    for block in blocks {
        match block {
            // Live objects only (`0x464890`, `0x465371`).
            Notify::Values { guid, sent } => {
                let moved = copies.get(&guid).unwrap_or(&none);
                let block = Notified::new(Sent::Fields(&sent), moved);
                notify(&mut sc, guid, block, !away.contains(&guid));
            }
            // An object not yet shown runs its type's create hook; one shown before runs every
            // field's watch instead (`0x465d79`, `0x465d8e`).
            Notify::Create { guid, kind } => {
                if unshown.remove(&guid) {
                    create_hook(&mut sc, guid, kind, script.as_deref());
                } else {
                    let moved = copies.get(&guid).unwrap_or(&none);
                    let block = Notified::new(Sent::All, moved);
                    notify(&mut sc, guid, block, !away.contains(&guid));
                }
            }
        }
    }
    for guid in reactivated {
        reactivated_end(&mut sc, guid, script.as_deref());
    }
}

/// One block's notify on a live object: its field edges, the field watches whose callbacks
/// signal interface events ([`crate::field_watch`]), the item watchers, and the guild timestamp's
/// watcher. `bridged` says whether the unit's field bridge is registered.
fn notify(sc: &mut Scene, guid: u64, block: Notified, bridged: bool) {
    let Some(e) = sc.index.0.get(&guid).copied() else {
        return;
    };
    let Some(fields) = sc.stores.get(e).ok().map(|s| &s.0) else {
        return;
    };
    let Some(kind) = fields.created_as() else {
        return;
    };
    sc.field_changes
        .write_batch(block.edges().map(|(index, old, new)| FieldChanged {
            entity: e,
            guid,
            kind,
            index,
            old,
            new,
        }));
    let me = sc.self_guid.0;
    let (names, index, stores, net) = (&sc.names, &sc.index, &sc.stores, &sc.net);
    let others = || {
        let units = index.0.iter().filter_map(|(&g, &e)| {
            let s = stores.get(e).ok()?;
            s.is_unit().then_some((g, &s.0))
        });
        me.map(|me| crate::field_watch::reaction_sweep(me, units))
            .unwrap_or_default()
    };
    let around = crate::field_watch::Around {
        me,
        // An entry of 0 has no key (`0x556aad`).
        creature_lookup: &|entry| {
            names
                .resolve_creature(entry, guid, Ask::Event(net))
                .is_some()
        },
        others: &others,
        pet_bar: sc.pet_bar.as_ref().map_or(0, |bar| bar.spells.pet_guid),
    };
    let mut fired = crate::field_watch::values(&around, guid, kind, fields, &block);
    if !bridged {
        fired.retain(|n| !matches!(n, crate::field_watch::FieldNotice::Unit { .. }));
    }
    let notices = item_watch(&sc.index, &sc.stores, &sc.self_guid, &sc.items)
        .values(guid, kind, fields, &block);
    let stamp = crate::field_watch::guild_timestamp(&block, kind, fields);
    // An entry change that missed the cache asks and waits on the answer (`0x5d9652` asks with
    // `0x5d96b0`).
    for n in &notices {
        if let crate::item_watch::ItemNotice::Entry {
            guid,
            new,
            new_cached: false,
            ..
        } = *n
        {
            sc.items.template(new, guid, Ask::Event(&sc.net));
            sc.items.wait_for_entry(guid, new);
        }
    }
    sc.item_notices.write_batch(notices);
    sc.field_notices.write_batch(fired);
    if let Some(((guild_id, stamp), guilds)) = stamp.zip(sc.guilds.as_mut()) {
        guilds.stamp_changed(guild_id, stamp, &sc.net);
    }
}

/// The create hook of an object not yet shown (the table `0x465e78`): an item's record set-up
/// (`0x5d8400` → `0x5d8300`); a player's `PostInit` (`0x5debe0`), its guild tag (`0x5dec19`)
/// ahead of another player's party record (`0x5dec5f`, the active player skipped at
/// `0x5dec48`-`0x5dec5b`).
fn create_hook(sc: &mut Scene, guid: u64, kind: Created, script: Option<&UiScript>) {
    let Some(fields) = sc
        .index
        .0
        .get(&guid)
        .and_then(|&e| sc.stores.get(e).ok())
        .map(|s| s.0.clone())
    else {
        return;
    };
    match kind {
        Created::Item => {
            // The record, set up at once when cached, for our item in the world.
            let hook =
                item_watch(&sc.index, &sc.stores, &sc.self_guid, &sc.items).created(guid, &fields);
            // The set-up asks for the record of every item, whoever's, with the callback
            // `0x5d8390` (`0x5d8300` into `0x55ba30`): the readers of a held item only read it.
            if let Some(entry) = fields.object_entry() {
                sc.items.template(entry, guid, Ask::Event(&sc.net));
            }
            sc.item_notices.write_batch(hook);
        }
        Created::World(EntityKind::Player) => {
            sc.field_notices
                .write_batch(crate::field_watch::guild_tag(guid));
            if sc.self_guid.0 != Some(guid) {
                compare_party_record(sc, guid, &fields, script);
            }
        }
        Created::World(_) => {}
    }
}

/// Vtable slot 3 of an object the packet brought back: a player's `PostUpdate` (`0x5dec90`)
/// compares its party record (`0x5decb7`), with no active-player gate, as the active player is
/// never released.
fn reactivated_end(sc: &mut Scene, guid: u64, script: Option<&UiScript>) {
    let Some(e) = sc.index.0.get(&guid).copied() else {
        return;
    };
    let player = sc
        .stores
        .get(e)
        .is_ok_and(|s| s.0.created_as() == Some(ObjectType::Player));
    if !player {
        return;
    }
    let Some(fields) = sc.stores.get(e).ok().map(|s| s.0.clone()) else {
        return;
    };
    compare_party_record(sc, guid, &fields, script);
}

/// `CGPlayer_C::ComparePartyRecord` (`0x5f0b80`): the stored party or raid record, an empty one
/// off the roster (`0x5f0bfe`), against a live one built off the descriptor and the held pet
/// (`0x5f0c0b`), the difference signalled through `0x5e5720` (`0x5f0c42`). Neither record is
/// written back.
fn compare_party_record(
    sc: &mut Scene,
    guid: u64,
    fields: &ObjectFields,
    script: Option<&UiScript>,
) {
    let old = sc.group.stats.get(&guid).cloned().unwrap_or_default();
    let pet_guid = fields.unit_pet_guid();
    let pet = pet_guid
        .and_then(|g| sc.index.0.get(&g))
        .and_then(|&e| sc.stores.get(e).ok());
    let pet_name = pet_guid
        .zip(pet)
        .and_then(|(g, p)| sc.names.resolve_unit(g, Some(p), Ask::Read));
    let pet_name = crate::ui_party::net::snapshot_pet_name(pet_name, script);
    let mut live = old.clone();
    live.snapshot_descriptor(fields, pet.map(|p| &p.0), &pet_name);
    let notices = crate::ui_party::record_notices(guid, &old, &live);
    sc.field_notices.write_batch(notices);
}
