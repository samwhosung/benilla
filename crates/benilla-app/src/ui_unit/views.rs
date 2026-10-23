//! The unit views a listener reads, as the packets before it left them. In the reference each
//! packet's handler runs whole before the next (`0x537aa0` calls it at `0x537ae5`), its watchers
//! fire inside it (`0x465570` → `0x4655dc`), and `UnitHealth` reads the live object
//! (`0x5174d0`, `[obj+0x110]+0x40` at `0x517553`), so a listener sees the state its own packet
//! left, never a later packet's. The unit feed pushes the frame's last state, so here each update
//! packet logs what it moved ([`ViewLog`]); the views lane rewinds the stores the tokens name to
//! the frame's start and replays the log in stamp order among the other lanes, rebuilding the
//! tokens' views once per gap between events ([`UnitViews`]).
//!
//! The unit of "as of" is the packet: the update handler `0x4651a0` runs both write passes over
//! every block (`0x46520e`, `0x465222`) before its notify pass (`0x46522d`-`0x46528b`), so a
//! watcher of one block already reads a later block of the same packet.

use std::collections::{HashMap, HashSet, VecDeque};

use benilla_protocol::ObjectFields;
use benilla_ui::script::UnitState;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::lanes::{Stamp, Until};
use crate::names::NameCache;
use crate::net::{Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::target::{RelationWorld, Selection};

/// The four tokens [`super::feed_units`] pushes and this lane versions, in [`Tokens`]' order.
const TOKENS: [&str; 4] = ["player", "target", "targettarget", "npc"];

/// One value per token of [`TOKENS`].
pub(super) type Tokens<T> = [T; 4];

/// One change an update packet made to a store.
enum Change {
    /// The dwords a write moved, `(index, old, new)`, as [`crate::net::write_fields`] returns
    /// them.
    Moved(Vec<(u16, u32, u32)>),
    /// The store on the change's other side, `None` for no object: before it as logged, after it
    /// once the rewind has undone it. A create logs no object before; a destroy, the store it
    /// took.
    Swap(Option<ObjectStore>),
}

impl Change {
    /// Undo the change on `store`, the store as it left it.
    fn undo(&mut self, store: &mut Option<ObjectStore>) {
        match self {
            Self::Moved(moved) => put(store, moved.iter().map(|&(i, old, _)| (i, old))),
            Self::Swap(other) => std::mem::swap(store, other),
        }
    }

    /// Make the change again on `store`, the store as it found it.
    fn redo(&mut self, store: &mut Option<ObjectStore>) {
        match self {
            Self::Moved(moved) => put(store, moved.iter().map(|&(i, _, new)| (i, new))),
            Self::Swap(other) => std::mem::swap(store, other),
        }
    }
}

/// Write `pairs` over `store` as a values block would.
fn put(store: &mut Option<ObjectStore>, pairs: impl Iterator<Item = (u16, u32)>) {
    if let Some(store) = store {
        let pairs: Vec<(u16, u32)> = pairs.collect();
        store.0.merge(ObjectFields::from_pairs(&pairs));
    }
}

/// One packet's changes, stamped at its first, so it sorts after what earlier packets raised and
/// before what its own notify pass raises.
struct Entry {
    at: Stamp,
    changes: Vec<(u64, Change)>,
}

/// What the packets of the frame changed in the stores, one entry per packet, in their order.
/// The update handler writes it ([`crate::net`]'s object layer); the views lane takes it.
#[derive(Resource, Default)]
pub(crate) struct ViewLog {
    entries: Vec<Entry>,
    /// The last entry is the packet still being handled.
    open: bool,
    /// How many times the entries were taken or dropped: a frame-start snapshot holds for the
    /// entries it saw ([`FrameStart`]).
    epoch: u64,
}

impl ViewLog {
    fn change(&mut self, guid: u64, change: Change) {
        if !self.open {
            self.entries.push(Entry {
                at: crate::lanes::stamp(),
                changes: Vec::new(),
            });
            self.open = true;
        }
        if let Some(entry) = self.entries.last_mut() {
            entry.changes.push((guid, change));
        }
    }

    /// A write of `guid`'s store moved these dwords.
    pub(crate) fn moved(&mut self, guid: u64, moved: &[(u16, u32, u32)]) {
        if !moved.is_empty() {
            self.change(guid, Change::Moved(moved.to_vec()));
        }
    }

    /// `guid`'s store was replaced whole: created, brought back, written with no type to diff by,
    /// or destroyed. `before` is the store it had, `None` for no object.
    pub(crate) fn replaced(&mut self, guid: u64, before: Option<ObjectStore>) {
        self.change(guid, Change::Swap(before));
    }

    /// The packet is handled: the next change opens the next entry.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// The world left at once (a worldport, a disconnect, a logout): every unit the object manager
    /// holds is logged as its store stands, a destroy for each the teardown takes. For one it
    /// keeps, the entry holds the store unchanged, which undoing and replaying leaves as it is.
    pub(crate) fn torn_down<'a>(
        &mut self,
        index: &GuidIndex,
        store: impl Fn(Entity) -> Option<&'a ObjectStore>,
    ) {
        for (&guid, &e) in &index.0 {
            if !benilla_protocol::guid::is_item(guid) {
                self.replaced(guid, store(e).cloned());
            }
        }
        self.close();
    }

    /// The frame's entries, taken for the replay.
    fn take(&mut self) -> Vec<Entry> {
        self.close();
        self.epoch += 1;
        std::mem::take(&mut self.entries)
    }
}

/// Each store a token names as the frame began, with the entity it was read from, and how many
/// entries the log held then (`mark`), for the check that the rewind lands on it: a write the log
/// missed makes it land elsewhere. Taken ahead of the net stage, in debug builds.
#[derive(Resource, Default)]
pub(crate) struct FrameStart {
    epoch: u64,
    mark: usize,
    stores: HashMap<u64, (Option<Entity>, Option<ObjectStore>)>,
}

/// [`FrameStart`]'s snapshot, of the stores the lane's rewind starts from.
#[cfg(debug_assertions)]
pub(crate) fn mark_frame_start(
    log: Res<ViewLog>,
    mut start: ResMut<FrameStart>,
    selection: Option<Res<Selection>>,
    interact: Option<Res<crate::ui_session::InteractNpc>>,
    index: Option<Res<GuidIndex>>,
    all: Query<&ObjectStore>,
    me: Query<(Entity, &Guid), With<SelfPlayer>>,
) {
    let store = |e: Option<Entity>| e.and_then(|e| all.get(e).ok()).cloned();
    let mut stores = HashMap::new();
    if let Some((e, g)) = me.iter().next() {
        stores.insert(g.0, (Some(e), store(Some(e))));
    }
    if let Some((guid, entity)) = selection.as_deref().and_then(|s| Some((s.guid?, s.target))) {
        let target = store(entity);
        let hop = target
            .as_ref()
            .and_then(|s| s.0.unit_target())
            .filter(|&g| g != 0);
        stores.insert(guid, (entity, target));
        if let Some(hop) = hop {
            let e = index.as_deref().and_then(|i| i.0.get(&hop).copied());
            stores.entry(hop).or_insert_with(|| (e, store(e)));
        }
    }
    if let Some((guid, entity)) = interact.as_deref().and_then(|i| Some((i.1?, i.0))) {
        stores
            .entry(guid)
            .or_insert_with(|| (entity, store(entity)));
    }
    *start = FrameStart {
        epoch: log.epoch,
        mark: log.entries.len(),
        stores,
    };
}

/// The views lane: the stores the tokens name as of the drain's place, and what it has pushed.
#[derive(Resource, Default)]
pub(crate) struct UnitViews {
    /// The frame's entries not yet applied, in stamp order: only their changes to the stores
    /// rewound.
    pending: VecDeque<Entry>,
    /// Each rewound guid's store as of the drain's place, `None` for no object, with the entity
    /// its live store is read from.
    scratch: HashMap<u64, (Option<Entity>, Option<ObjectStore>)>,
    /// The drain has fired the lane's start.
    started: bool,
    /// What the VM holds for each token, as far as the lane knows: `None` unknown.
    shown: Option<Tokens<Option<Option<UnitState>>>>,
    /// The guids `"target"` and `"npc"` named as the drain began: a call during it that moves one
    /// seats that token itself, and the lane leaves it to the next frame's feed.
    named: (Option<u64>, Option<u64>),
    /// A drain ended with entries unapplied, so the VM may hold an older view than the feed's
    /// memo says: the next start pushes every token.
    stale: bool,
    /// The builds made, for the tests.
    #[cfg(test)]
    pub(crate) builds: usize,
    /// The stores checked against the live ones at the end of a drain, for the tests.
    #[cfg(test)]
    pub(crate) checked: usize,
    /// The views pushed, for the tests.
    #[cfg(test)]
    pub(crate) pushes: usize,
    /// The stores the rewind checked against the frame-start snapshot, for the tests.
    #[cfg(test)]
    pub(crate) starts_checked: usize,
}

impl UnitViews {
    /// `guid`'s store as of the drain's place: the rewound store while the drain replays it, else
    /// `live`.
    pub(crate) fn as_of<'a>(
        &'a self,
        guid: u64,
        live: Option<&'a ObjectStore>,
    ) -> Option<&'a ObjectStore> {
        match self.scratch.get(&guid) {
            Some((_, store)) => store.as_ref(),
            None => live,
        }
    }

    /// Apply every entry `until` admits.
    fn apply(&mut self, until: Until) {
        while self.pending.front().is_some_and(|e| until.admits(e.at)) {
            let Some(mut entry) = self.pending.pop_front() else {
                break;
            };
            for (guid, change) in &mut entry.changes {
                if let Some((_, store)) = self.scratch.get_mut(guid) {
                    change.redo(store);
                }
            }
        }
    }

    /// The drain's end for this lane: every store is back to the live one.
    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    fn finish<'a>(&mut self, live: impl Fn(Entity) -> Option<&'a ObjectStore>) {
        #[cfg(debug_assertions)]
        for (guid, (entity, store)) in &self.scratch {
            debug_assert!(
                same_reads(store.as_ref(), entity.and_then(&live)),
                "views: {guid:#x} replayed to a store the live one is not"
            );
            #[cfg(test)]
            {
                self.checked += 1;
            }
        }
        self.scratch.clear();
        self.pending.clear();
        self.started = false;
        self.shown = None;
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.scratch.clear();
        self.started = false;
        self.shown = None;
    }
}

/// Whether two stores read the same at every dword: a replayed write lands a cleared field as a
/// sent 0, which a create's descriptor reads as 0 too.
fn same_reads(a: Option<&ObjectStore>, b: Option<&ObjectStore>) -> bool {
    let (a, b) = match (a, b) {
        (None, None) => return true,
        (Some(a), Some(b)) => (&a.0, &b.0),
        _ => return false,
    };
    if a.created_as() != b.created_as() {
        return false;
    }
    let created = a.created_as().is_some();
    let read = |f: &ObjectFields| -> HashMap<u16, u32> {
        f.raw_fields()
            .filter(|&(_, v)| !created || v != 0)
            .collect()
    };
    read(a) == read(b)
}

/// The stores the getters read, through the views lane: a script call during the drain reads the
/// store as its listener's packet left it ([`UnitViews::as_of`]). Outside the drain it is the live
/// store.
#[derive(SystemParam)]
pub(crate) struct ViewStores<'w, 's> {
    live: Query<'w, 's, (&'static ObjectStore, Option<&'static Guid>)>,
    views: Option<Res<'w, UnitViews>>,
}

/// No store as of the drain's place: the entity holds none, or its object did not exist yet.
#[derive(Debug)]
pub(crate) struct NotHeld;

impl ViewStores<'_, '_> {
    /// `entity`'s store as of the drain's place.
    pub(crate) fn get(&self, entity: Entity) -> Result<&ObjectStore, NotHeld> {
        let (store, guid) = self.live.get(entity).map_err(|_| NotHeld)?;
        match (self.views.as_deref(), guid) {
            (Some(views), Some(guid)) => views.as_of(guid.0, Some(store)).ok_or(NotHeld),
            _ => Ok(store),
        }
    }
}

/// What the four tokens' snapshots read beside their stores.
pub(super) struct TokenSources<'a, 'w, 's> {
    pub(super) tables: &'a super::SnapshotTables<'w>,
    pub(super) names: &'a NameCache,
    pub(super) commands: &'a NetCommands,
    pub(super) group: &'a crate::ui_party::GroupState,
    pub(super) guild: &'a crate::ui_guild::GuildState,
    pub(super) world: &'a RelationWorld<'w, 's>,
}

/// The stores the four tokens read, each as of the place they are built for.
pub(super) struct TokenStores<'a> {
    /// Our own store and guid.
    pub(super) me: Option<(&'a ObjectStore, u64)>,
    /// The selection's.
    pub(super) target: Option<(&'a ObjectStore, u64)>,
    /// The interaction NPC's.
    pub(super) npc: Option<(&'a ObjectStore, u64)>,
    /// The object manager's lookup (`0x468460`), for the target's target and a tooltip's owner.
    pub(super) by_guid: &'a dyn Fn(u64) -> Option<&'a ObjectStore>,
}

/// The four tokens' snapshots off `stores`: `"player"` off our descriptor, `"target"` and
/// `"npc"` off theirs, and `"targettarget"` one hop off the target's `UNIT_FIELD_TARGET`, streamed
/// guids only, for the stock frame's poll (`TargetofTarget_OnUpdate`, `TargetFrame.lua:494-538`).
pub(super) fn token_views<'a>(
    src: &TokenSources<'a, '_, '_>,
    stores: &TokenStores<'a>,
) -> Tokens<Option<UnitState>> {
    let world = src.world;
    let player = stores.me.map(|(store, guid)| {
        let mut s = super::player_snapshot(
            store,
            guid,
            src.tables,
            src.names,
            src.commands,
            src.group,
            world.factions(),
            src.guild,
        );
        if let Some(me) = world.me() {
            s.is_pvp_ffa = super::pvp_free_for_all(&world.get(), &me);
        }
        s
    });
    let held = super::held::HeldUnits {
        names: src.names,
        commands: src.commands,
        group: src.group,
        self_store: stores.me.map(|(s, _)| s),
        unit_names: src.tables.unit_names(),
        types: src.tables.types(src.names),
        rel: world.get(),
        me: world.me(),
        tooltip: super::tooltip_facts::TooltipSources {
            names: src.names,
            catalog: src.tables.catalog(),
            object: stores.by_guid,
            study: src.tables.study(),
        },
        guild: src.guild,
    };
    let target = stores.target.map(|(store, guid)| held.state(store, guid));
    let tot = stores
        .target
        .and_then(|(store, _)| store.0.unit_target())
        .filter(|guid| *guid != 0)
        .and_then(|guid| Some(held.state((stores.by_guid)(guid)?, guid)));
    let npc = stores.npc.map(|(store, guid)| held.state(store, guid));
    [player, target, tot, npc]
}

/// The lane, its start, and the drop under the exact complement of the drain's gate.
pub(super) fn add_unit_views_lane(app: &mut App) {
    use crate::lanes::LaneApp;
    app.init_resource::<ViewLog>()
        .init_resource::<UnitViews>()
        .init_resource::<FrameStart>()
        .add_lane("unit views", peek, fire_unit_views, None)
        .add_drain_end(|world| {
            if let Some(mut views) = world.get_resource_mut::<UnitViews>() {
                views.end_drain();
            }
        })
        .add_systems(
            Update,
            drop_unit_views
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
    #[cfg(debug_assertions)]
    app.add_systems(
        Update,
        mark_frame_start.before(benilla_world::schedule::WorldStage::Net),
    );
}

/// The lane's head: its start while the frame logged anything, then its next entry.
fn peek(world: &World) -> Option<Stamp> {
    let views = world.get_resource::<UnitViews>()?;
    if !views.started {
        let logged = world
            .get_resource::<ViewLog>()
            .is_some_and(|log| !log.entries.is_empty());
        return (logged || views.stale).then(Stamp::origin);
    }
    views.pending.front().map(|e| e.at)
}

/// What the packets wrote while no in-game interface is up, taken unreplayed: there is no VM to
/// read it.
fn drop_unit_views(mut log: ResMut<ViewLog>, mut views: ResMut<UnitViews>) {
    log.take();
    views.clear();
}

/// The units the tokens name as the drain begins, each by guid with the entity its live store is
/// read from: `"player"`, `"target"` and `"npc"`.
///
/// Each getter resolves its token at the call (`UnitHealth` calls `0x515970` at `0x517511`), so
/// a listener for an earlier packet reads the unit the token named then. Not yet: `"target"` and
/// `"npc"` name the frame's last selection and interaction through the drain, and a token a call
/// moves during it is left to that call's own seat, until the selection and the interaction are
/// versioned too (#2217).
struct Seats {
    me: Option<(u64, Option<Entity>)>,
    target: Option<(u64, Option<Entity>)>,
    npc: Option<(u64, Option<Entity>)>,
}

/// One guid's store from `live` back to the frame's start, undoing `entries`' changes to it in
/// reverse, with each value its `UNIT_FIELD_TARGET` held on the way.
fn undo_frame(
    guid: u64,
    live: Option<ObjectStore>,
    entries: &mut [Entry],
    start: Option<&FrameStart>,
) -> (Option<ObjectStore>, Vec<u64>) {
    let hop = |s: &Option<ObjectStore>| s.as_ref().and_then(|s| s.0.unit_target());
    let mut store = live;
    let mut hops: Vec<u64> = hop(&store).into_iter().collect();
    // The store where the frame-start snapshot was taken, past the entries logged before it.
    let mark = start.map_or(0, |s| s.mark);
    let mut at_mark = None;
    for (i, entry) in entries.iter_mut().enumerate().rev() {
        if start.is_some() && i + 1 == mark {
            at_mark = Some(store.clone());
        }
        for (g, change) in entry.changes.iter_mut().rev() {
            if *g == guid {
                change.undo(&mut store);
                hops.extend(hop(&store));
            }
        }
    }
    if let Some((entity, snapshot)) = start.and_then(|s| s.stores.get(&guid)) {
        let at_mark = at_mark.unwrap_or_else(|| store.clone());
        debug_assert!(
            same_reads(at_mark.as_ref(), snapshot.as_ref()),
            "views: {guid:#x} ({entity:?}) rewound to a store the frame did not start from: a \
             write the log missed"
        );
    }
    (store, hops)
}

impl UnitViews {
    /// The drain's end, however it ended: one that stopped before this lane replayed all it took
    /// leaves no rewound store for a later read, and the VM unknown, so the next start pushes
    /// every token.
    fn end_drain(&mut self) {
        if self.started {
            self.stale = true;
            self.clear();
        }
    }

    /// The lane's start: the frame's entries, and the store of each guid a token names rewound
    /// to the frame's start, the target's target being each unit the target's
    /// `UNIT_FIELD_TARGET` named during the frame. Only a guid the frame changed is rewound.
    fn rewind(
        &mut self,
        mut entries: Vec<Entry>,
        seats: Seats,
        live: impl Fn(Entity) -> Option<ObjectStore>,
        by_guid: impl Fn(u64) -> Option<Entity>,
        start: Option<&FrameStart>,
    ) {
        // The snapshot is checked against for each guid read from the entity it was taken from.
        #[cfg(test)]
        let checked = std::cell::Cell::new(0);
        let start_for = |guid: u64, entity: Option<Entity>| {
            let start = start.filter(|s| s.stores.get(&guid).is_some_and(|(e, _)| *e == entity));
            #[cfg(test)]
            checked.set(checked.get() + usize::from(start.is_some()));
            start
        };
        let touched: HashSet<u64> = entries
            .iter()
            .flat_map(|e| e.changes.iter().map(|(g, _)| *g))
            .collect();
        let mut hops = Vec::new();
        let mut scratch = HashMap::new();
        if let Some((guid, entity)) = seats.target {
            let (store, seen) = undo_frame(
                guid,
                entity.and_then(&live),
                &mut entries,
                start_for(guid, entity),
            );
            hops = seen;
            if touched.contains(&guid) {
                scratch.insert(guid, (entity, store));
            }
        }
        let others = seats.me.into_iter().chain(seats.npc).chain(
            hops.into_iter()
                .filter(|&g| g != 0)
                .map(|g| (g, by_guid(g))),
        );
        for (guid, entity) in others {
            if scratch.contains_key(&guid) {
                continue;
            }
            // An untouched store is not rewound, but is checked against the snapshot all the same.
            let check = start_for(guid, entity);
            if touched.contains(&guid) || check.is_some() {
                let (store, _) = undo_frame(guid, entity.and_then(&live), &mut entries, check);
                if touched.contains(&guid) {
                    scratch.insert(guid, (entity, store));
                }
            }
        }
        self.pending = entries
            .into_iter()
            .filter_map(|mut entry| {
                entry.changes.retain(|(g, _)| scratch.contains_key(g));
                (!entry.changes.is_empty()).then_some(entry)
            })
            .collect();
        self.named = (seats.target.map(|(g, _)| g), seats.npc.map(|(g, _)| g));
        self.scratch = scratch;
        #[cfg(test)]
        {
            self.starts_checked += checked.get();
        }
    }
}

/// The lane's `fire_until` ([`crate::lanes`]). Its start takes the frame's log and rewinds
/// ([`UnitViews::rewind`]). Each firing applies every entry `until` admits, then pushes the
/// tokens' views as of there, one build for the whole gap. A start that admits every entry finds
/// nothing due before the feed's push, and builds nothing; the last entry leaves the feed's own
/// views, which it restores.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // the snapshots' inputs, as the unit feed reads them
fn fire_unit_views(
    In(until): In<Until>,
    mut vm: crate::script_calls::VmWith<(
        ResMut<'static, UnitViews>,
        ResMut<'static, super::UnitFeedState>,
        Res<'static, Selection>,
        RelationWorld<'static, 'static>,
    )>,
    mut log: ResMut<ViewLog>,
    tables: super::SnapshotTables,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    group: Res<crate::ui_party::GroupState>,
    guild: Res<crate::ui_guild::GuildState>,
    index: Option<Res<GuidIndex>>,
    interact: Option<Res<crate::ui_session::InteractNpc>>,
    all: Query<&ObjectStore>,
    self_q: Query<(Entity, &ObjectStore, &Guid), With<SelfPlayer>>,
    #[cfg_attr(not(debug_assertions), allow(unused_variables))] start: Option<Res<FrameStart>>,
) {
    vm.turn(|script, (mut views, mut feed, selection, world)| {
        let live = |e: Entity| all.get(e).ok().cloned();
        let by_guid = |guid: u64| index.as_ref().and_then(|i| i.0.get(&guid).copied());
        let npc_seat = interact.as_deref().and_then(|i| Some((i.1?, i.0)));
        let Some(script) = script else {
            log.take();
            views.clear();
            return;
        };
        let first = !views.started;
        if first {
            #[cfg(debug_assertions)]
            let start = start.as_deref().filter(|s| s.epoch == log.epoch);
            #[cfg(not(debug_assertions))]
            let start = None;
            let entries = log.take();
            let seats = Seats {
                me: self_q.iter().next().map(|(e, _, g)| (g.0, Some(e))),
                target: selection.guid.map(|g| (g, selection.target)),
                npc: npc_seat,
            };
            views.rewind(entries, seats, live, by_guid, start);
            views.started = true;
        }
        views.apply(until);
        let last = views.pending.is_empty();
        if first && last && !views.stale {
            views.finish(|e| all.get(e).ok());
            return;
        }
        let stale = std::mem::take(&mut views.stale);
        let memo = &feed.vm.get(script).last;
        let feed_views: Tokens<Option<UnitState>> = TOKENS.map(|t| memo.get(t).cloned());
        let mut shown = views.shown.take().unwrap_or_else(|| {
            if stale {
                Default::default()
            } else {
                feed_views.clone().map(Some)
            }
        });
        let now = if last {
            feed_views
        } else {
            #[cfg(test)]
            {
                views.builds += 1;
            }
            let src = TokenSources {
                tables: &tables,
                names: &names,
                commands: &commands,
                group: &group,
                guild: &guild,
                world: &world,
            };
            let views = &*views;
            let held = |guid: u64, entity: Option<Entity>| {
                let live = entity.and_then(|e| all.get(e).ok());
                Some((views.as_of(guid, live)?, guid))
            };
            let object = |guid: u64| held(guid, by_guid(guid)).map(|(s, _)| s);
            let stores = TokenStores {
                me: self_q
                    .iter()
                    .next()
                    .and_then(|(e, _, g)| held(g.0, Some(e))),
                target: selection
                    .target
                    .zip(selection.guid)
                    .and_then(|(e, g)| held(g, Some(e))),
                npc: interact
                    .as_deref()
                    .and_then(|i| Some((i.0?, i.1?)))
                    .and_then(|(e, g)| held(g, Some(e))),
                by_guid: &object,
            };
            token_views(&src, &stores)
        };
        // A call during the drain that moved the selection or the interaction seated its token
        // itself, which the next frame's feed takes up.
        let moved_target = selection.guid != views.named.0;
        let moved_npc = npc_seat.map(|(g, _)| g) != views.named.1;
        for (i, view) in now.into_iter().enumerate() {
            let frozen = match i {
                1 | 2 => moved_target,
                3 => moved_npc,
                _ => false,
            };
            // `"player"` is pushed only while its descriptor exists, as the feed pushes it.
            if frozen || (i == 0 && view.is_none()) {
                continue;
            }
            if shown[i].as_ref() != Some(&view) {
                #[cfg(test)]
                {
                    views.pushes += 1;
                }
                script.set_unit(TOKENS[i], view.clone());
                shown[i] = Some(view);
            }
        }
        views.shown = Some(shown);
        if last {
            views.finish(|e| all.get(e).ok());
        }
    });
}

#[cfg(test)]
impl UnitViews {
    /// Whether the lane holds a rewound store for `guid`.
    pub(crate) fn holds(&self, guid: u64) -> bool {
        self.scratch.contains_key(&guid)
    }
}
