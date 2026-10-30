//! One `SMSG_UPDATE_OBJECT` as the reference's handler runs it (`0x4651a0`): an out-of-range list
//! as the first block releases first; the write passes then create, write and bring back objects,
//! the first, behind the header byte, for top-nibble 6 and 8 guids, the second for the rest; the
//! notify pass reads every block again, in packet order, and runs the watches and the create
//! hooks; and each object the packet brought back gets its vtable slot 3 last
//! (`0x465290`-`0x4652ff`). The protocol hands each block over as its own event in the order the
//! passes write them, a first-pass block's notify place as [`SessionEvent::ObjectNotify`], and
//! closes the packet with [`SessionEvent::ObjectUpdateEnd`], so the write passes run block by
//! block, each block's commands applied before the next, and the notify pass runs at the end.
//!
//! The object manager keeps two tables (`0x464ff0`): the live one, here the [`GuidIndex`], and the
//! released one, [`Released`]. A release (`0x464920`) runs the object's deactivate, its vtable slot
//! 1, and moves it to the released table with its fields; a create, values, movement or
//! near-objects block for the guid brings it back (`0x464530`), and only `SMSG_DESTROY_OBJECT` or
//! a new object of its type frees it (`0x467521`, `0x466e55`). A brought-back object's model is
//! rebuilt (slot 2, `0x614640`); here it is a new entity.

use std::collections::{HashMap, HashSet, VecDeque};

use benilla_protocol::messages::ObjectType;
use benilla_protocol::{guid, EntityKind, MoveSpeeds, NotifyBlock, ObjectFields, SessionEvent};
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
use crate::ui_chat::combat::watch::XpAwarded;
use benilla_world::vis_chain::VisChainOnly;

/// Registers the update packet's block handlers, its end, and `SMSG_DESTROY_OBJECT`.
pub(super) fn register(app: &mut App) {
    use benilla_protocol::SessionEventKind as K;
    app.init_resource::<UpdatePass>()
        .init_resource::<Released>()
        .init_resource::<ObjectList>()
        .init_resource::<crate::target::OwedSelects>()
        .net_handler(K::ObjectCreate, on_object_create)
        .net_handler(K::ItemCreate, on_item_create)
        .net_handler(K::ObjectValues, on_object_values)
        .net_handler(K::ObjectMove, on_object_move)
        .net_handler(K::ObjectsRemoved, on_objects_removed)
        .net_handler(K::ObjectsNear, on_objects_near)
        .net_handler(K::ObjectNotify, on_object_notify)
        .net_handler(K::ObjectUpdateEnd, on_update_end)
        .net_handler(K::ObjectDestroyed, on_object_destroyed);
}

/// What one packet's write pass leaves for its notify pass; empty between packets.
#[derive(Resource, Default)]
pub(crate) struct UpdatePass {
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

/// The release-pending bit, `0x10` of `+0xe4` (`0x614f50`): a release (`0x464920`) of an object
/// its game object state player holds (`0x5f3b27`, the count `+0xe8`) only sets it, and the
/// object stays live, indexed and drawn, until the unhold releases it ([`release_unheld`]). A
/// block whose lookup finds it live clears it (`0x4645b2`).
#[derive(Component)]
pub(crate) struct ReleasePending;

/// The display row a unit's records were resolved from (`+0xb34`, `0x60afb0`), which a new
/// `UNIT_FIELD_DISPLAYID` is compared against (`0x60ae10`); `None` until its create hook resolves
/// them, or with no display table loaded. It lives as long as the object, across a release.
#[derive(Component, Clone, Copy, Default)]
pub(crate) struct HeldDisplay(Option<u32>);

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
        /// Bit 3 of the unit's `+0xc58`, which only the constructor clears (`0x5fae7a`): the
        /// object a block brings back is the one that left.
        xp_awarded: bool,
        /// Bit 0x400 of the same word ([`crate::net::ClientControlled`]).
        client_controlled: bool,
        /// A unit's resolved records, which its slot 3 compares (`0x5fbe28`).
        display: Option<HeldDisplay>,
        /// A transport's clock, which reactivation leaves (`0x464530` constructs nothing).
        anchor: Option<crate::transport::TransportAnchor>,
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
    /// the head of the list); the guid it freed.
    fn free_oldest(&mut self, type_id: u8) -> Option<u64> {
        let guid = self.reuse.get_mut(&type_id).and_then(VecDeque::pop_front)?;
        self.objects.remove(&guid);
        Some(guid)
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
            found_live(&mut sc, e);
            object_create(&mut sc, block, &fields, Landing::Live(e));
            match sc.stores.get_mut(e) {
                Ok(mut store) => write_logged(&mut sc.view_log, guid, &mut store, fields),
                Err(_) => {
                    sc.view_log.replaced(guid, None);
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
            let (pose, modes, (xp_awarded, controlled), display, kept_anchor) = match body {
                Body::World {
                    pose,
                    modes,
                    xp_awarded,
                    client_controlled,
                    display,
                    anchor,
                    ..
                } => (
                    Some(pose),
                    modes,
                    (xp_awarded, client_controlled),
                    display,
                    anchor,
                ),
                Body::Item { .. } => (None, None, (false, false), None, None),
            };
            // Only a unit takes the block's movement (`0x46638c`); any other type keeps its pose.
            let unit = matches!(kind, EntityKind::Unit | EntityKind::Player);
            let kept_pose = pose.filter(|_| !unit);
            let e = object_create(
                &mut sc,
                block,
                &fields,
                Landing::New {
                    store,
                    kept_pose,
                    kept_anchor,
                },
            );
            if let Some(modes) = modes {
                sc.commands.entity(e).insert(modes);
            }
            if xp_awarded {
                sc.commands.entity(e).insert(XpAwarded);
            }
            if controlled {
                sc.commands.entity(e).insert(crate::net::ClientControlled);
            }
            if let Some(display) = display {
                sc.commands.entity(e).insert(display);
            }
            sc.pass.reactivated.push(guid);
            sc.view_log.replaced(guid, None);
            moved
        }
        (None, None) => {
            allocate(&mut sc, guid, world_type(kind));
            sc.view_log.replaced(guid, None);
            let store = fields.clone();
            let e = object_create(
                &mut sc,
                block,
                &fields,
                Landing::New {
                    store,
                    kept_pose: None,
                    kept_anchor: None,
                },
            );
            if matches!(kind, EntityKind::Unit | EntityKind::Player) {
                sc.commands.entity(e).insert(HeldDisplay::default());
            }
            sc.pass.unshown.insert(guid);
            Vec::new()
        }
    };
    sc.pass.copies.insert(guid, moved);
    notify_in_place(
        &mut sc,
        Notify::Create {
            guid,
            kind: Created::World(kind),
        },
    );
}

/// A new object of `ty` frees the oldest released one of its type (`0x466e55`); our own player is
/// allocated apart and frees nothing (`0x466e0c`).
fn allocate(sc: &mut Scene, guid: u64, ty: ObjectType) {
    if sc.self_guid.0 != Some(guid) {
        if let Some(freed) = sc.released.free_oldest(type_id(ty)) {
            free_item(sc, freed);
        }
    }
}

/// An object freed here: an item's lock goes with it, at this place among the packet's runs
/// ([`crate::item_watch::Freed`]).
fn free_item(sc: &mut Scene, object: u64) {
    if guid::is_item(object) {
        sc.item_lane.push(crate::item_watch::Run::freeing(
            crate::item_watch::Freed::Items(vec![object]),
        ));
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
    debug!("net: item create {guid:#x} (container: {container})");
    let live = sc.index.0.get(&guid).copied();
    let kept = live.is_none().then(|| sc.released.take(guid)).flatten();
    let moved = match (live, kept) {
        (Some(e), _) => {
            found_live(&mut sc, e);
            match sc.stores.get_mut(e) {
                Ok(mut store) => write_logged(&mut sc.view_log, guid, &mut store, fields),
                Err(_) => Vec::new(),
            }
        }
        (
            None,
            Some(Kept {
                fields: mut store,
                body,
            }),
        ) => {
            let moved = write_fields(&mut store, fields);
            sc.view_log.replaced(guid, None);
            sc.view_log.countdowns(guid, None);
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
            sc.view_log.replaced(guid, None);
            sc.view_log.countdowns(guid, None);
            crate::items::spawn_item(&mut sc.commands, &mut sc.index, guid, fields, container);
            sc.pass.unshown.insert(guid);
            Vec::new()
        }
    };
    sc.pass.copies.insert(guid, moved);
    notify_in_place(
        &mut sc,
        Notify::Create {
            guid,
            kind: Created::Item,
        },
    );
}

/// A block's write into a held store ([`write_fields`]), logged for the views a listener reads
/// ([`crate::ui_unit::views::ViewLog`]): the dwords it moved, or the store whole when it had no
/// type to diff by. An item's too, whose stack, durability and slots the bag getters read as its
/// packet left them.
fn write_logged(
    log: &mut crate::ui_unit::views::ViewLog,
    guid: u64,
    store: &mut ObjectStore,
    delta: ObjectFields,
) -> Vec<(u16, u32, u32)> {
    let untyped = store.0.created_as().is_none().then(|| store.clone());
    let moved = write_fields(&mut store.0, delta);
    match untyped {
        Some(before) => log.replaced(guid, Some(before)),
        None => log.moved(guid, &moved),
    }
    moved
}

/// A held object leaving the live table: the store it takes, logged for any unit, as one a token
/// names now or named earlier in the frame (the target's target before it changed) reads it in
/// the frame's earlier listeners, and for any item, as a weapon the active player wore earlier in
/// the frame reads to the stat getters there, with the item's countdown cells.
fn log_destroy(sc: &mut Scene, guid: u64) {
    let Some(&e) = sc.index.0.get(&guid) else {
        return;
    };
    let before = sc.stores.get(e).ok().cloned();
    if before.is_some() {
        sc.view_log.replaced(guid, before);
    }
    if let Ok(cells) = sc.countdowns.get(e) {
        sc.view_log.countdowns(guid, Some(cells.clone()));
    }
}

/// A values block (`0x467160`): written into a held object, or one it brings back; a guid the
/// manager knows neither way is skipped (`0x465bb0`).
fn on_object_values(In(ev): In<SessionEvent>, mut sc: Scene) {
    let SessionEvent::ObjectValues { guid, fields } = ev else {
        return;
    };
    // Our corpse's `CORPSE_FIELD_FLAGS` can flip to BONES under a live guid; the reclaim latch is
    // re-asked on that edge, as the reference's `FLAGS` handler does (`0x5d6d60`).
    crate::death::net::recheck_corpse(guid, &fields, &sc.self_guid, &mut sc.death_net);
    let sent: Vec<u16> = fields.raw_fields().map(|(index, _)| index).collect();
    let moved = match sc.index.0.get(&guid).copied() {
        Some(e) => {
            found_live(&mut sc, e);
            match sc.stores.get_mut(e) {
                Ok(mut store) => Some(write_logged(&mut sc.view_log, guid, &mut store, fields)),
                Err(_) => None,
            }
        }
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
    notify_in_place(&mut sc, Notify::Values { guid, sent });
}

/// A written block's place in the notify pass, which reads every block in packet order
/// (`0x46522d`): a second-pass block's is where it was written; a first-pass block's comes as its
/// own [`SessionEvent::ObjectNotify`] at its place among the second pass's.
fn notify_in_place(sc: &mut Scene, block: Notify) {
    let guid = match block {
        Notify::Values { guid, .. } | Notify::Create { guid, .. } => guid,
    };
    if !guid::first_pass(guid) {
        sc.pass.blocks.push(block);
    }
}

/// A first-pass block's place in the notify pass ([`notify_in_place`]).
fn on_object_notify(In(ev): In<SessionEvent>, mut pass: ResMut<UpdatePass>) {
    let SessionEvent::ObjectNotify { guid, block } = ev else {
        return;
    };
    pass.blocks.push(match block {
        NotifyBlock::Values(sent) => Notify::Values { guid, sent },
        NotifyBlock::ItemCreate => Notify::Create {
            guid,
            kind: Created::Item,
        },
        NotifyBlock::Create(kind) => Notify::Create {
            guid,
            kind: Created::World(kind),
        },
    });
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
    // The unit this client moves drops its block before the lookup that would bring a released
    // one back (`0x4670a5`-`0x4670bb`, ahead of `0x4670c2`); a held one is `object_move`'s test.
    let Some(&e) = sc.index.0.get(&guid) else {
        if active_mover(&sc) == Some(guid) {
            return;
        }
        if let Some(Kept { fields, body }) = sc.released.take(guid) {
            let pose = pose_transform(position, wire_yaw(orientation));
            respawn(&mut sc, guid, body, fields, Some(pose));
        }
        return;
    };
    if active_mover(&sc) != Some(guid) {
        found_live(&mut sc, e);
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
    sc.player.as_ref()?;
    let lock = sc.mover_lock.as_deref().copied().unwrap_or_default();
    if lock.control_lost {
        return None;
    }
    lock.foreign_mover.or(sc.self_guid.0)
}

/// A near-objects block (`0x467230`): each listed released object but our own player comes back
/// (`0x467283`, `0x4644f0`).
fn on_objects_near(In(ev): In<SessionEvent>, mut sc: Scene) {
    let SessionEvent::ObjectsNear(guids) = ev else {
        return;
    };
    for guid in guids {
        if sc.self_guid.0 == Some(guid) {
            continue;
        }
        if let Some(&e) = sc.index.0.get(&guid) {
            found_live(&mut sc, e);
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
            sc.view_log.countdowns(guid, None);
        }
        Body::World {
            kind,
            display_id,
            scale,
            pose: kept_pose,
            speeds,
            modes,
            rider,
            xp_awarded,
            client_controlled,
            display,
            anchor,
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
            if xp_awarded {
                entity.insert(XpAwarded);
            }
            if client_controlled {
                entity.insert(crate::net::ClientControlled);
            }
            if let Some(display) = display {
                entity.insert(display);
            }
            if let Some(anchor) = anchor {
                entity.insert(anchor);
            }
            let e = entity.id();
            sc.index.0.insert(guid, e);
            crate::target::object_held(sc.selection.as_mut(), guid, e);
        }
    }
    sc.pass.reactivated.push(guid);
    sc.view_log.replaced(guid, None);
}

/// The out-of-range list, which the protocol hands over only as a packet's first block
/// (`0x4651e1`). It releases in two rounds: every guid outside the first pass's, our own player
/// excepted (`0x465f10`-`0x465f4f`), then the first pass's, with no such exception
/// (`0x465f75`-`0x465fae`).
fn on_objects_removed(In(ev): In<SessionEvent>, mut sc: Scene, script: Option<NonSend<UiScript>>) {
    let SessionEvent::ObjectsRemoved(guids) = ev else {
        return;
    };
    let (ahead, rest): (Vec<u64>, Vec<u64>) = guids.into_iter().partition(|&g| guid::first_pass(g));
    for guid in rest {
        if sc.self_guid.0 == Some(guid) {
            continue;
        }
        release_or_hold(&mut sc, guid, script.as_deref());
    }
    for guid in ahead {
        release_or_hold(&mut sc, guid, script.as_deref());
    }
}

/// The lookup of a create, values, movement or near-objects block that finds the object live
/// clears its release-pending bit (`0x464530` → `0x4645b2`).
fn found_live(sc: &mut Scene, e: Entity) {
    if sc.pending.contains(e) {
        sc.commands.entity(e).remove::<ReleasePending>();
    }
}

/// Whether a live object is held (`0x614f00`): its state player plays a holding substate, or
/// will once this frame's drive arms what the packets asked ([`GoAnim::will_hold`]).
///
/// [`GoAnim::will_hold`]: crate::go_anim::GoAnim::will_hold
fn held_by_play(sc: &Scene, e: Entity) -> bool {
    sc.go_anims
        .get(e)
        .is_ok_and(|go| go.will_hold(sc.go_models.get(e).ok()))
}

/// `0x464920`: a held object only takes its release-pending bit and stays live
/// (`0x464942`-`0x46494f`); any other runs its deactivate and leaves for the released table.
fn release_or_hold(sc: &mut Scene, guid: u64, script: Option<&UiScript>) {
    if let Some(&e) = sc.index.0.get(&guid) {
        if held_by_play(sc, e) {
            sc.commands.entity(e).insert(ReleasePending);
            return;
        }
    }
    deactivate(sc, guid, script);
    release(sc, guid);
}

/// `ClntObjMgrUnholdObject` (`0x468410`) past its decrement: an object no longer held whose
/// release is pending is released then (`0x46844a` → `0x464920`). It runs after the state
/// players' drive ([`crate::go_anim`]), which ends and replaces their plays, and hands the release
/// to [`unhold_releases`] at the chain's next sync point, so the schedule carries only this test.
pub(crate) fn release_unheld(
    pending: Query<Entity, With<ReleasePending>>,
    plays: Query<&crate::go_anim::GoAnim>,
    mut commands: Commands,
) {
    if pending
        .iter()
        .any(|e| !plays.get(e).is_ok_and(|go| go.holds()))
    {
        commands.run_system_cached(unhold_releases);
    }
}

/// The unhold's release ([`release_unheld`]): each pending object nothing holds runs its
/// deactivate, then leaves for the released table.
fn unhold_releases(mut sc: Scene, script: Option<NonSend<UiScript>>) {
    let due: Vec<(Entity, u64)> = sc
        .pending
        .iter()
        .filter(|&(e, _)| !sc.go_anims.get(e).is_ok_and(|go| go.holds()))
        .map(|(e, g)| (e, g.0))
        .collect();
    for (e, guid) in due {
        sc.commands.entity(e).remove::<ReleasePending>();
        if sc.index.0.get(&guid) == Some(&e) {
            deactivate(&mut sc, guid, script.as_deref());
            release(&mut sc, guid);
        }
    }
    sc.view_log.close();
}

/// `SMSG_DESTROY_OBJECT` (`0x4674a0`): the release, then the object freed at once, a released one
/// included (`0x467521`). The object is freed now and its model fades out, as the deactivate
/// (`0x6145e0`) hands the model to the `SWModelFadeout` scheduler (`0x672df0`); the guid leaves
/// the index at once ([`super::tear_down`]).
///
/// A held object's release only marks it pending (`0x464920`), so the destroy finds nothing
/// released to free: it stays live, and its unhold releases it into the released table
/// ([`release_unheld`]).
fn on_object_destroyed(In(ev): In<SessionEvent>, mut sc: Scene, script: Option<NonSend<UiScript>>) {
    let SessionEvent::ObjectDestroyed(guid) = ev else {
        return;
    };
    crate::death::net::forget_corpse(guid, &mut sc.death_net);
    // A released object ran its deactivate as it left, and is freed now.
    if sc.released.take(guid).is_some() {
        free_item(&mut sc, guid);
        return;
    }
    // A guid neither live nor released has nothing to release (`0x464931`, `0x46493a`).
    let Some(&e) = sc.index.0.get(&guid) else {
        return;
    };
    if held_by_play(&sc, e) {
        sc.commands.entity(e).insert(ReleasePending);
        return;
    }
    deactivate(&mut sc, guid, script.as_deref());
    log_destroy(&mut sc, guid);
    sc.view_log.close();
    let Some(e) = sc.index.0.remove(&guid) else {
        return;
    };
    // The unit's destructor zeroes the active mover it names (`0x5fb631`-`0x5fb655`).
    if let Some(mover) = sc.mover.as_deref_mut() {
        mover.forget(guid);
    }
    // An item has no model to fade, so it goes now with its countdown cells, its lock behind its
    // deactivate's run.
    if benilla_protocol::guid::is_item(guid) {
        sc.commands.entity(e).despawn();
        free_item(&mut sc, guid);
        return;
    }
    sc.commands.entity(e).queue(super::tear_down);
}

/// The deactivate a release runs (vtable slot 1, `0x46496f`), reading the descriptor about to go:
/// the item's (`0x5d8660`), its head ([`super::item_deactivate::head`]) then its interface tail,
/// and the party member's, which snapshots its record and sends `CMSG_REQUEST_PARTY_MEMBER_STATS`.
fn deactivate(sc: &mut Scene, guid: u64, script: Option<&UiScript>) {
    if benilla_protocol::guid::is_item(guid) {
        super::item_deactivate::head(sc, guid);
    }
    let gone = item_watch(&sc.index, &sc.stores, &sc.self_guid, &sc.items).destroyed_run(guid);
    sc.item_lane.push(gone.unwrap_or_default());
    let held = |g: u64| sc.index.0.get(&g).and_then(|e| sc.stores.get(*e).ok());
    let before = sc.group.stats.get(&guid).cloned();
    let local = |e: Entity| sc.transforms.get(e).ok().copied();
    let object = held(guid).map(|s| (s, sc.place.position(sc.index.0.get(&guid).copied(), local)));
    crate::ui_party::net::member_deactivated(
        guid,
        &mut sc.group,
        object,
        sc.place.zone(),
        held,
        &sc.names,
        script,
        &sc.net,
    );
    // The record the snapshot wrote, for the views a listener reads.
    if sc.group.stats.get(&guid) != before.as_ref() {
        sc.view_log.record(guid, before);
    }
    // The active player's release, past its roster legs: its watchers go (`0x5e9b50`), then the
    // three mirror timers stop (`0x5e9b55`); the leave-world sweep that follows (`0x5e9b5a`) has
    // no producer here.
    if sc.self_guid.0 == Some(guid) {
        crate::ui_mirror::stop_all(&mut sc.field_notices);
    }
    // `CGUnit_C::Release` (`0x5fbb60`, after the player's roster legs, `0x5e9b61`): the
    // selection's teardown (`0x5fbbe5`), unless a group row holds the guid (`0x5fbbcd`). Its run
    // waits for the packet's end, so in one out-of-range list its sends follow a later member's
    // `0x27f`.
    {
        let held = |g: u64| held(g).map(|s| &s.0);
        let me = sc.self_guid.0;
        let group = &sc.group;
        crate::target::unit_released(
            sc.selection.as_mut(),
            sc.owed_releases.as_deref_mut(),
            guid,
            || crate::target::raid_member_or_pet(group, guid, me, &held),
        );
    }
    // `0x5fbc26`-`0x5fbc3b`: the active mover's release stops its auto-move
    // (`ReleaseActiveMover 0x6006b0` → `0x6006c2`); the global goes with the unit ([`release`]).
    // The later `0x5fbcd7` finds no active mover left.
    stop_auto_move(
        sc.mover.as_deref(),
        sc.follow.as_deref_mut(),
        sc.approach.as_deref_mut(),
        sc.call_autorun.as_deref_mut(),
        &mut sc.field_notices,
        guid,
    );
    // The release's `PARTY_MEMBER_DISABLE` for a party slot's player (`0x5e9b78`).
    if held(guid).is_some_and(|s| s.0.created_as() == Some(ObjectType::Player)) {
        sc.group.member_object(guid, false);
    }
}

/// Whether the mover's input tick takes its teardown leg (`0x5146c1`-`0x5146ce`): it may neither
/// translate (`0x514560`) nor turn (`0x5145b0`), off its health, the far-sight latch as the packet
/// leaves it (`[mover+0x1c70] & 1`, `0x514540`, past the active-player test `0x514537`), the root
/// and the stun its new flags hold ([`crate::player::MoverInput`]).
fn tick_tears_down(sc: &Scene, fields: &ObjectFields) -> bool {
    let driving_own_body = sc
        .mover_lock
        .as_deref()
        .is_none_or(|l| l.foreign_mover.is_none());
    let input = crate::player::MoverInput {
        dead: fields.unit_is_dead(),
        view_is_out: crate::player::view_is_out(
            driving_own_body,
            sc.far_sight
                .as_deref()
                .is_some_and(crate::player::FarSight::on),
        ),
    };
    let rooted = sc
        .player
        .as_deref()
        .is_some_and(crate::player::Player::rooted);
    let stunned = fields.unit_flags() & crate::player::UNIT_FLAG_STUNNED != 0;
    input.torn_down(rooted, stunned)
}

/// `StopAutoMove` (`0x60fb60`) on `guid`, a no-op unless it is the active mover
/// ([`crate::player::AutoMoveStop`]): the scene's claim, auto-move and notice lane.
pub(super) fn stop_auto_move(
    mover: Option<&crate::net::MoverClaim>,
    follow: Option<&mut crate::player::FollowState>,
    approach: Option<&mut crate::player::Approach>,
    autorun: Option<&mut crate::player::CallAutorun>,
    lane: &mut crate::field_watch::NoticeLane,
    guid: u64,
) {
    if mover.is_some_and(|m| m.holds(guid)) {
        crate::player::end_auto_move(follow, approach, autorun, Some(lane));
    }
}

/// `StartFarSight` (`0x5ee270`) on our player `me`, whose `PLAYER_FARSIGHT` names `object`, held:
/// the update while far sight is off. A start (`0x5ee39c` on, past the unresolved return) stops
/// the auto-move: a unit we control takes the active mover (`0x5ee45d` → `0x5ee477`), whose switch
/// stops the outgoing one's (`0x600742`) and is a no-op when it already holds it (`0x6006fb`); the
/// switch itself is possession's control update's. Any other object runs the mover's tick
/// (`0x5ee4a2`), which the view's going out tears down.
fn start_far_sight(sc: &mut Scene, me: u64, object: u64) {
    let started = match sc.far_sight.as_deref() {
        Some(far_sight) => {
            let was_on = far_sight.on();
            let index = &sc.index;
            far_sight.start(Some(object), |g| index.0.contains_key(&g), &sc.net);
            !was_on
        }
        // A harness without the machine: the start's stop.
        None => true,
    };
    if !started {
        return;
    }
    let active = sc.mover.as_deref().and_then(crate::net::MoverClaim::get);
    // `IsClientControlled` (`0x5fa5f0`): our own control, or the unit's own bit.
    let controlled = if object == me {
        !sc.own_control_lost.as_deref().is_some_and(|l| l.0)
    } else {
        sc.client_controlled.contains(sc.index.0[&object])
    };
    let stops = if controlled {
        active.filter(|&m| m != object)
    } else {
        Some(me)
    };
    if let Some(mover) = stops {
        stop_auto_move(
            sc.mover.as_deref(),
            sc.follow.as_deref_mut(),
            sc.approach.as_deref_mut(),
            sc.call_autorun.as_deref_mut(),
            &mut sc.field_notices,
            mover,
        );
    }
}

/// The far-sight arrival of a new object: a unit's `PostInit` (`0x5fbb1a`) and a dynamic object's
/// `OnCreate` (`0x5d552d`) look up the active player (`0x468460`, as a player) and, when its
/// `PLAYER_FARSIGHT` (`[player+0xe68]+0x830`) names the new object, start far sight on it
/// ([`start_far_sight`]).
fn far_sight_arrives(sc: &mut Scene, guid: u64) {
    let Some(me) = sc.self_guid.0 else {
        return;
    };
    let field = sc
        .index
        .0
        .get(&me)
        .and_then(|&e| sc.stores.get(e).ok())
        .and_then(|s| s.0.player_farsight());
    if field == Some(guid) {
        start_far_sight(sc, me, guid);
    }
}

/// `0x464920` past the deactivate: the object leaves the live table for the released one, its
/// fields and what it stood as kept, and its model fades out ([`super::tear_down`]).
fn release(sc: &mut Scene, guid: u64) {
    log_destroy(sc, guid);
    let Some(e) = sc.index.0.remove(&guid) else {
        return;
    };
    // `CGUnit_C::Release` zeroes the active mover it names (`0x5fbc26`-`0x5fbc3b` → `0x6006b0`).
    if let Some(mover) = sc.mover.as_deref_mut() {
        mover.forget(guid);
    }
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
                    xp_awarded: sc.xp_awarded.contains(e),
                    client_controlled: sc.client_controlled.contains(e),
                    display: sc.held_displays.get(e).ok().copied(),
                    anchor: sc.anchors.get(e).ok().copied(),
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
fn on_update_end(
    In(ev): In<SessionEvent>,
    mut sc: Scene,
    script: Option<NonSend<UiScript>>,
    mut arrival: super::arrival::Arrival,
) {
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
            //
            // Deviation: a first-pass values block the clear header byte left unwritten re-fires,
            // for a live object, its last write's edges against the watch copy (`0x465570`,
            // `0x466686`); benilla compares against this packet's copies, which hold none for it,
            // so a stale edge never fires twice.
            Notify::Values { guid, sent } => {
                let moved = copies.get(&guid).unwrap_or(&none);
                let block = Notified::new(Sent::Fields(&sent), moved);
                notify(&mut sc, guid, block, !away.contains(&guid));
            }
            // An object not yet shown runs its type's create hook; one shown before runs every
            // field's watch instead (`0x465d79`, `0x465d8e`).
            //
            // Deviation: the create notify uses its lookup's result untested (`0x465c97`,
            // `0x465d79`). For a first-pass create the clear header byte left unwritten, an
            // object not live crashes the reference, and a live one runs the all-fields watch
            // (`0x465d8e`) against its stale watch copy, re-firing its last write's edges as the
            // values block above does; benilla's [`notify`] fires nothing for either.
            Notify::Create { guid, kind } => {
                if unshown.remove(&guid) {
                    create_hook(&mut sc, guid, kind, script.as_deref(), &mut arrival);
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
    mirror_guild(&mut sc);
    sc.view_log.close();
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
    // The `UNIT_FIELD_DISPLAYID` watcher (`0x604990`) and the health watcher's return to life
    // (`0x6046f0`: old health not above 0, new above, `0x6047ee`-`0x604835`) rebuild the display.
    if matches!(kind, ObjectType::Unit | ObjectType::Player)
        && block.edges().any(|(index, old, new)| {
            index == benilla_protocol::field::FIELD_UNIT_DISPLAYID
                || (index == benilla_protocol::field::FIELD_UNIT_HEALTH
                    && old as i32 <= 0
                    && new as i32 > 0)
        })
    {
        let fields = fields.clone();
        display_changed(sc, e, guid, &fields);
    }
    let Some(fields) = sc.stores.get(e).ok().map(|s| &s.0) else {
        return;
    };
    // The `GAMEOBJECT_STATE` watcher stores the state its state player arms from (`0x5f89e0`).
    if kind == ObjectType::GameObject {
        let state = block
            .edges()
            .find(|&(index, _, _)| index == benilla_protocol::field::FIELD_GAMEOBJECT_STATE);
        if let (Some((_, _, state)), Ok(mut go)) = (state, sc.go_anims.get_mut(e)) {
            go.set_wire_state(state);
        }
    }
    // `0x5ee990`'s helm and cloak arms re-dress the piece by its moved bit (`0x5eea45`-`0x5eea8e`),
    // and the visible-item watch a slot by the flag (`0x5dee30` → `0x5df0de`): the watch covers the
    // slot's whole 0x30 bytes (`0x5e2541`), so a creator or enchant move counts as the entry's.
    if kind == ObjectType::Player {
        if let Ok(mut pins) = sc.worn_pins.get_mut(e) {
            for (index, old, new) in block.edges() {
                if index == benilla_protocol::field::FIELD_PLAYER_FLAGS {
                    pins.flags_moved(old ^ new);
                }
                for slot in [0u8, 14] {
                    let first = benilla_protocol::field::FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR
                        + 12 * u16::from(slot);
                    if (first..first + 12).contains(&index) {
                        pins.visible_moved(slot);
                    }
                }
            }
        }
    }
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
        target: sc.selection.as_deref().and_then(|s| s.guid),
        // An entry of 0 has no key (`0x556aad`).
        creature_lookup: &|entry| {
            names
                .resolve_creature(entry, guid, Ask::Event(net))
                .is_some()
        },
        others: &others,
        pet_bar: sc.pet_bar.as_ref().map_or(0, |bar| bar.spells.pet_guid),
    };
    let mut placed = crate::field_watch::placed_values(&around, guid, kind, fields, &block);
    if !bridged {
        placed.retain(|(_, n)| !matches!(n, crate::field_watch::FieldNotice::Unit { .. }));
    }
    // The item watchers' runs, each with its unlocks, its bag events and the signals its handler
    // ends in, at their watches' places; a bag seat's rescans the bag cache.
    let runs: Vec<(crate::field_watch::Place, crate::item_watch::Run)> = item_watch(
        &sc.index,
        &sc.stores,
        &sc.self_guid,
        &sc.items,
    )
    .placed_values(guid, kind, fields, &block, &mut sc.bag_cache);
    let stamp = crate::field_watch::guild_timestamp(&block, kind, fields);
    // `OnCharmOrSummonChanged` (`0x5e2660`) on the active player or a party or raid member
    // (`0x5e266b`, `0x4918e0`): an old pet the object manager holds no unit of has its selection
    // torn down (`0x5e26f7`, `0x5e2719`).
    if me == Some(guid) || sc.group.party_or_raid_member(guid) {
        for old in crate::field_watch::pets_moved_off(&block, kind, fields) {
            let held = sc
                .index
                .0
                .get(&old)
                .and_then(|&e| sc.stores.get(e).ok())
                .is_some_and(|s| s.is_unit());
            if let (false, Some(owed)) = (held, sc.deselect.as_deref_mut()) {
                owed.deselect(old);
            }
        }
    }
    // An entry change that missed the cache asks and waits on the answer (`0x5d9652` asks with
    // `0x5d96b0`).
    for (_, run) in &runs {
        if let Some(crate::item_watch::ItemNotice::Entry {
            guid,
            new,
            new_cached: false,
            ..
        }) = run.notice
        {
            sc.items.template(new, guid, Ask::Event(&sc.net));
            sc.items.wait_for_entry(guid, new);
        }
    }
    // The aura watcher's one call (`0x604d00`), on a unit whose fields 47 to 100 moved.
    let mut aura_call = matches!(kind, ObjectType::Unit | ObjectType::Player)
        .then(|| crate::net::aura_watch_span(fields, block.edges()))
        .flatten()
        .map(|(old, new)| (old, new, fields.unit_health().unwrap_or(0)));
    // The edges, the runs and the notices in the watchers' one walk, each stamped where it is
    // written.
    for step in crate::field_watch::in_watch_order(&block, placed, runs) {
        match step {
            crate::field_watch::Step::Edge { index, old, new } => {
                let at = crate::lanes::stamp();
                sc.field_changes.write(FieldChanged {
                    at,
                    entity: e,
                    guid,
                    kind,
                    index,
                    old,
                    new,
                });
                // The watcher runs at its range's place, where its first edge is written.
                if crate::net::in_aura_watch(index) {
                    if let Some((old, new, health)) = aura_call.take() {
                        sc.aura_watch.write(crate::net::AuraFieldsChanged {
                            at,
                            entity: e,
                            guid,
                            old,
                            new,
                            health,
                        });
                    }
                }
            }
            crate::field_watch::Step::Notice(crate::field_watch::FieldNotice::TrainerPass) => {
                sc.trainer.owed()
            }
            // A watcher's stop of the mover's auto-move, its `AUTOFOLLOW_END` at this place.
            crate::field_watch::Step::Notice(crate::field_watch::FieldNotice::AutoMoveStop) => {
                stop_auto_move(
                    sc.mover.as_deref(),
                    sc.follow.as_deref_mut(),
                    sc.approach.as_deref_mut(),
                    sc.call_autorun.as_deref_mut(),
                    &mut sc.field_notices,
                    guid,
                );
            }
            // The stun's tick (`0x5ff501` → `0x514640`): its teardown leg (`0x5146d6`).
            crate::field_watch::Step::Notice(crate::field_watch::FieldNotice::InputTick) => {
                if sc
                    .stores
                    .get(e)
                    .is_ok_and(|store| tick_tears_down(sc, &store.0))
                {
                    stop_auto_move(
                        sc.mover.as_deref(),
                        sc.follow.as_deref_mut(),
                        sc.approach.as_deref_mut(),
                        sc.call_autorun.as_deref_mut(),
                        &mut sc.field_notices,
                        guid,
                    );
                }
            }
            // The far-sight field's change (`0x5de0d0`): on a held object the start
            // (`0x5de144` → `0x5ee270`, [`start_far_sight`]); else the end (`0x5de15d` →
            // `0x5ee590`).
            crate::field_watch::Step::Notice(crate::field_watch::FieldNotice::FarSight {
                object,
            }) => {
                let field = (object != 0).then_some(object);
                if field.is_some_and(|g| sc.index.0.contains_key(&g)) {
                    start_far_sight(sc, guid, object);
                } else if let Some(far_sight) = sc.far_sight.as_deref() {
                    let index = &sc.index;
                    far_sight.end(field, |g| index.0.contains_key(&g), &sc.net);
                }
            }
            // The ghost flag's clear (`0x5dea20`): `SetAreaSpiritHealer(0, 0)` runs at the packet,
            // a pending wave's `CMSG_CANCEL_AURA` going out here and its event on the lane ahead
            // of the `PLAYER_UNGHOST` that follows.
            crate::field_watch::Step::Notice(
                crate::field_watch::FieldNotice::AreaSpiritHealerClear,
            ) => {
                let Some(spirit) = sc.spirit.as_deref_mut() else {
                    continue;
                };
                if spirit.set_healer(None).cancel_aura {
                    let farsight = sc.stores.get(e).ok().and_then(|s| s.0.player_farsight());
                    let player = Some((e, farsight));
                    let held = |guid: u64| sc.index.0.contains_key(&guid);
                    let spell = crate::ui_dialog_verbs::AREA_SPIRIT_HEALER_AURA;
                    if sc.aura.cancel_at_packet(player, held, spell) {
                        sc.field_notices
                            .write(crate::field_watch::FieldNotice::Global {
                                id: crate::field_watch::id::AREA_SPIRIT_HEALER_OUT_OF_RANGE,
                            });
                    }
                }
            }
            // The quest-slot watcher's accept line (`0x5dde4f`-`0x5dde7d`), at its place.
            crate::field_watch::Step::Notice(crate::field_watch::FieldNotice::QuestAccepted {
                quest_id,
            }) => {
                if let (Some(quest_log), Some(lane), Some(lines)) = (
                    sc.quest_log.as_deref(),
                    sc.quest_giver.as_deref_mut(),
                    sc.quest_lines.as_deref_mut(),
                ) {
                    crate::ui_quest::lines::Raise {
                        quest_log,
                        items: &sc.items,
                        commands: &sc.net,
                        lane,
                        notices: &mut sc.field_notices,
                        lines,
                    }
                    .line(crate::ui_quest::QuestLine::Accepted(quest_id));
                }
            }
            crate::field_watch::Step::Notice(n) => sc.field_notices.write(n),
            crate::field_watch::Step::Item(run) => sc.item_lane.push(run),
        }
    }
    // The active player's coinage watcher `OnMoneyChanged` (`0x5ddf30`), registered on its own
    // object (`0x5dd8a0`): the trade offer's trim (`0x5ddfda`), at the packet, behind the
    // `PLAYER_MONEY` its notice stamped (`0x5ddf4b`); the trim signals nothing.
    if kind == ObjectType::Player
        && me == Some(guid)
        && block
            .edges()
            .any(|(index, _, _)| index == benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE)
    {
        let purse = sc.stores.get(e).ok().and_then(|s| s.0.player_money());
        if let (Some(trade), Some(purse)) = (sc.trade.as_mut(), purse) {
            crate::ui_trade::trim_offer_to_purse(trade, purse, &sc.net);
        }
    }
    if let Some(((guild_id, stamp), guilds)) = stamp.zip(sc.guilds.as_mut()) {
        guilds.stamp_changed(guild_id, stamp, &sc.net);
    }
}

/// Our guild id and rank as the packet left them in our descriptor (fields 191 and 192), which the
/// guild mirror keeps ([`crate::ui_guild::GuildState::mirror_self`]). Mirrored only while the avatar
/// is streamed: the frames between a logout's despawn and the teardown must not read as leaving
/// the guild.
fn mirror_guild(sc: &mut Scene) {
    let Some(guilds) = sc.guilds.as_mut() else {
        return;
    };
    let Some(store) = sc
        .self_guid
        .0
        .and_then(|guid| sc.index.0.get(&guid))
        .and_then(|&e| sc.stores.get(e).ok())
    else {
        return;
    };
    let (id, rank) = (store.0.player_guild_id(), store.0.player_guild_rank());
    // A write only on a move: a packet that leaves the guild alone marks it unchanged.
    if !guilds.mirrors(id, rank) {
        guilds.mirror_self(id, rank);
    }
}

/// The create hook of an object not yet shown (the table `0x465e78`), which alone asks the
/// object's record: an item's record set-up (`0x5d8400` → `0x5d8300`); a unit's `PostInit`
/// (`0x5fb880`), whose display resolve asks a creature's record (`0x60afb0`, at `0x60b15b`) and
/// which starts far sight late, ahead of its tail (`0x60c520`, `0x606fe0`, `0x607710`,
/// `0x5ff130`, `0x612a30`; [`far_sight_arrives`]); a player's (`0x5debe0`), the unit's
/// first (`0x5dec09`), then its name (`0x5dec12` → `0x609210`) and guild tag (`0x5dec19`) ahead of
/// another player's party record (`0x5dec5f`, the active player skipped at `0x5dec48`-`0x5dec5b`),
/// and for the active player its arrival last (`0x5dec7b`, [`super::arrival`]); a game object's
/// template (`0x5f7c40`, at `0x5f7d81`), whose lockId decides use-vs-cast; a dynamic object's
/// `OnCreate` (`0x5d5420`), a far-sight focus's arrival.
fn create_hook(
    sc: &mut Scene,
    guid: u64,
    kind: Created,
    script: Option<&UiScript>,
    arrival: &mut super::arrival::Arrival,
) {
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
        Created::Item => request_item_record(sc, guid, &fields),
        Created::World(EntityKind::Player) => {
            // Our own create runs the world-enter cascade (`0x5deb60` → `0x4908c0`), whose bag
            // watcher set-up zeroes and rescans the bag cache (`0x49098d` → `0x4f8cc0`).
            if sc.self_guid.0 == Some(guid) {
                let (index, stores) = (&sc.index, &sc.stores);
                let lookup = |g: u64| {
                    index
                        .0
                        .get(&g)
                        .and_then(|&e| stores.get(e).ok())
                        .map(|s| &s.0)
                };
                sc.bag_cache.seat(&fields, &lookup);
            }
            resolve_display_records(sc, guid, &fields);
            // The unit's `PostInit` (`0x5dec09` → `0x5fb880`): its far-sight arrival, ahead of
            // its tail (`0x60c520`, `0x606fe0`, `0x607710`, `0x5ff130`, `0x612a30`).
            far_sight_arrives(sc, guid);
            sc.names.lookup_fields(guid, &fields, &sc.net);
            sc.field_notices
                .write_batch(crate::field_watch::guild_tag(guid));
            // `0x5dec35` → `0x4e85d0` mode 1: a party slot's `PARTY_MEMBER_ENABLE`.
            sc.group.member_object(guid, true);
            if sc.self_guid.0 != Some(guid) {
                compare_party_record(sc, guid, &fields, script);
            } else {
                super::arrival::arrive(sc, arrival, guid);
            }
        }
        Created::World(EntityKind::Unit) => {
            resolve_display_records(sc, guid, &fields);
            sc.names.lookup_fields(guid, &fields, &sc.net);
            far_sight_arrives(sc, guid);
        }
        // `OnCreate` (`0x5d5420`): a far-sight focus (the bytes' low byte 2, `0x5d54a7`) that
        // the active player cast (`0x5d54b6`) is the far-sight arrival's.
        Created::World(EntityKind::DynamicObject) => {
            let focus = fields.dynamicobject_bytes().is_some_and(|b| b & 0xff == 2);
            let ours = fields
                .dynamicobject_caster()
                .is_some_and(|caster| sc.self_guid.0 == Some(caster));
            if focus && ours {
                far_sight_arrives(sc, guid);
            }
        }
        Created::World(EntityKind::GameObject) => {
            sc.go_templates.request_at_create(guid, &fields, &sc.net)
        }
        Created::World(_) => {}
    }
}

/// Vtable slot 3 of an object the packet brought back: a unit's (`0x5fbe00`) rebuilds its display
/// when its records changed while it was away (`0x5fbe28` → `0x60abe0`); a player's `PostUpdate`
/// (`0x5dec90`) runs the unit's, then compares its party record (`0x5decb7`), with no
/// active-player gate, as the active player is never released; a game object's (`0x5f7ed0`) asks
/// its template when it holds none (`+0x214`, `0x558560` at `0x5f7f31`); an item's and a bag's
/// (`0x5d8430` → `0x5d8300`) request their record as their create did, set up at once when
/// cached ([`request_item_record`]).
fn reactivated_end(sc: &mut Scene, guid: u64, script: Option<&UiScript>) {
    let Some(e) = sc.index.0.get(&guid).copied() else {
        return;
    };
    let Some(fields) = sc.stores.get(e).ok().map(|s| s.0.clone()) else {
        return;
    };
    match fields.created_as() {
        Some(ObjectType::Unit) => display_changed(sc, e, guid, &fields),
        Some(ObjectType::Player) => {
            display_changed(sc, e, guid, &fields);
            // `0x5deca1` → `0x4e85d0` mode 1.
            sc.group.member_object(guid, true);
            compare_party_record(sc, guid, &fields, script);
        }
        Some(ObjectType::GameObject) => sc.go_templates.request_at_create(guid, &fields, &sc.net),
        Some(ObjectType::Item | ObjectType::Container) => request_item_record(sc, guid, &fields),
        _ => {}
    }
}

/// `CGItem_C::RequestRecord` (`0x5d8300`), an item's create hook (`0x5d8400`) and its slot 3
/// (`EndReactivation 0x5d8430`): the record, set up at once when cached (`0x5d834a` →
/// `0x5d8440`), for our item in the world, and its signal for the owner of any item; a miss asks
/// for it with the callback `0x5d8390` (`0x5d833f` → `0x55ba30`), whoever's the item. The readers
/// of a held item only read it.
fn request_item_record(sc: &mut Scene, guid: u64, fields: &ObjectFields) {
    let run = item_watch(&sc.index, &sc.stores, &sc.self_guid, &sc.items).created_run(
        guid,
        fields,
        &sc.bag_cache,
    );
    if let Some(entry) = fields.object_entry() {
        if sc
            .items
            .template(entry, guid, Ask::Event(&sc.net))
            .is_none()
        {
            sc.items
                .wait(entry, crate::items::RecordCallback::SetUp(guid));
        }
    }
    if let Some(crate::item_watch::ItemNotice::Record { guid, .. }) = run.notice {
        super::replay_enchant_times(
            &mut sc.items,
            &mut sc.countdowns,
            &sc.index,
            &mut sc.view_log,
            guid,
        );
    }
    sc.item_lane.push(run);
}

/// `CGUnit_C::ResolveDisplayRecords` from the unit's `PostInit` (`0x5fb88b` → `0x60afb0`): the
/// records held off the display row ([`HeldDisplay`]); its creature ask is the create hook's
/// ([`NameCache::lookup_fields`]).
///
/// [`NameCache::lookup_fields`]: crate::names::NameCache::lookup_fields
fn resolve_display_records(sc: &mut Scene, guid: u64, fields: &ObjectFields) {
    let Some(e) = sc.index.0.get(&guid).copied() else {
        return;
    };
    let row = sc
        .creatures
        .as_deref()
        .and_then(|c| c.records_row(unit_display(fields)));
    if let Ok(mut held) = sc.held_displays.get_mut(e) {
        held.0 = row;
    }
}

/// `CGUnit_C::OnDisplayChanged` (`0x60abe0`), from the `UNIT_FIELD_DISPLAYID` watcher
/// (`0x6049b5`), the health watcher's return to life (`0x604835`) and slot 3 (`0x5fbe28`): when
/// `0x60ae10` sees a new record set the unit resolves its records again (`0x60ac21`), and a
/// creature asks its record (`0x60b15b`).
fn display_changed(sc: &mut Scene, e: Entity, guid: u64, fields: &ObjectFields) {
    let Some(creatures) = sc.creatures.as_deref() else {
        return;
    };
    let Ok(mut held) = sc.held_displays.get_mut(e) else {
        return;
    };
    let display = unit_display(fields);
    let Some(was) = held.0 else {
        return;
    };
    if !creatures.display_records_differ(display, was) {
        return;
    }
    held.0 = creatures.records_row(display);
    // The body rebuilt (`0x60abe0`) dresses both pieces by the flag (`0x5ed802`, `0x5ed81a`).
    if let Ok(mut pins) = sc.worn_pins.get_mut(e) {
        pins.rebuilt();
    }
    sc.names.ask_creature_record(guid, fields, &sc.net);
}

/// `UNIT_FIELD_DISPLAYID` as the resolve reads it, absent as 0 (vmangos omits zero fields).
fn unit_display(fields: &ObjectFields) -> u32 {
    fields.unit_displayid().map_or(0, |d| d as u32)
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
    let position = sc.place.position(sc.index.0.get(&guid).copied(), |e| {
        sc.transforms.get(e).ok().copied()
    });
    live.snapshot_descriptor(
        fields,
        pet.map(|p| &p.0),
        &pet_name,
        sc.place.zone(),
        position,
    );
    let notices = crate::ui_party::record_notices(guid, &old, &live);
    sc.field_notices.write_batch(notices);
}
