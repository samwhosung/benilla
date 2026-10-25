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
//! watcher of one block already reads a later block of the same packet. A stats packet writes the
//! member's group record before it signals (`0x5e5110`: `0x5e54c4`, `0x5e54d8`, then `0x5e54fa`),
//! so the log holds the records too, and the views built over them: the group tokens, the pet,
//! the units a `target` chain ends on and every unit's aura list ([`families`]).

use std::collections::{HashMap, HashSet, VecDeque};

use benilla_protocol::messages::PartyMemberStatsInfo;
use benilla_protocol::ObjectFields;
use benilla_ui::script::{UiScript, UnitState};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::lanes::{Stamp, Until};
use crate::names::NameCache;
use crate::net::{Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::target::{RelationWorld, Selection};

mod families;

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
    /// The member's group record on the change's other side, `None` for none, as [`Self::Swap`]
    /// holds a store.
    Record(Option<PartyMemberStatsInfo>),
}

impl Change {
    /// Undo the change on `store`, the store as it left it.
    fn undo(&mut self, store: &mut Option<ObjectStore>) {
        match self {
            Self::Moved(moved) => put(store, moved.iter().map(|&(i, old, _)| (i, old))),
            Self::Swap(other) => std::mem::swap(store, other),
            Self::Record(_) => {}
        }
    }

    /// Make the change again on `store`, the store as it found it.
    fn redo(&mut self, store: &mut Option<ObjectStore>) {
        match self {
            Self::Moved(moved) => put(store, moved.iter().map(|&(i, _, new)| (i, new))),
            Self::Swap(other) => std::mem::swap(store, other),
            Self::Record(_) => {}
        }
    }
}

/// Swap `guid`'s record in `records` with `other`, the record on a [`Change::Record`]'s other
/// side: an undo or a redo, as [`Change::Swap`] is either.
fn swap_record(
    records: &mut HashMap<u64, PartyMemberStatsInfo>,
    guid: u64,
    other: &mut Option<PartyMemberStatsInfo>,
) {
    let here = records.remove(&guid);
    if let Some(record) = other.take() {
        records.insert(guid, record);
    }
    *other = here;
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
        self.open();
        if let Some(entry) = self.entries.last_mut() {
            entry.changes.push((guid, change));
        }
    }

    /// Open the packet's entry, stamped here, ahead of what its handler raises before its first
    /// write: a handler that signals after writes it logs later, as a group list signals after
    /// its records (`0x5e6c4a`, then `0x5e6c66`), opens it at its start.
    pub(crate) fn open(&mut self) {
        if !self.open {
            self.entries.push(Entry {
                at: crate::lanes::stamp(),
                changes: Vec::new(),
            });
            self.open = true;
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

    /// `guid`'s group record was written: `before` is the record it had, `None` for none.
    pub(crate) fn record(&mut self, guid: u64, before: Option<PartyMemberStatsInfo>) {
        self.change(guid, Change::Record(before));
    }

    /// The group records went from `before` to `now`: each record that differs is logged.
    pub(crate) fn records(
        &mut self,
        before: &HashMap<u64, PartyMemberStatsInfo>,
        now: &HashMap<u64, PartyMemberStatsInfo>,
    ) {
        for (&guid, record) in before {
            if now.get(&guid) != Some(record) {
                self.record(guid, Some(record.clone()));
            }
        }
        for &guid in now.keys() {
            if !before.contains_key(&guid) {
                self.record(guid, None);
            }
        }
    }

    /// The packet is handled: the next change opens the next entry.
    pub(crate) fn close(&mut self) {
        if self.open && self.entries.last().is_some_and(|e| e.changes.is_empty()) {
            self.entries.pop();
        }
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

/// Each store a token can name as the frame began, with the entity it was read from, the group
/// records, and how many entries the log held then (`mark`), for the check that the rewind lands
/// on them: a write the log missed makes it land elsewhere. Taken ahead of the frame's input
/// records and the net stage, in debug builds.
#[derive(Resource, Default)]
pub(crate) struct FrameStart {
    epoch: u64,
    mark: usize,
    stores: HashMap<u64, (Option<Entity>, Option<ObjectStore>)>,
    records: Option<HashMap<u64, PartyMemberStatsInfo>>,
}

/// [`FrameStart`]'s snapshot, of the stores and records the lane's rewind starts from: the four
/// tokens' units and every held unit the resolver's inputs reach.
#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)] // what the tokens name, each read as the frame begins
pub(crate) fn mark_frame_start(
    log: Res<ViewLog>,
    mut start: ResMut<FrameStart>,
    selection: Option<Res<Selection>>,
    interact: Option<Res<crate::ui_session::InteractNpc>>,
    index: Option<Res<GuidIndex>>,
    all: Query<&ObjectStore>,
    me: Query<(Entity, &Guid), With<SelfPlayer>>,
    group: Option<Res<crate::ui_party::GroupState>>,
    script: Option<NonSend<UiScript>>,
) {
    let store = |e: Option<Entity>| e.and_then(|e| all.get(e).ok()).cloned();
    let by_guid = |g: u64| index.as_deref().and_then(|i| i.0.get(&g).copied());
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
            let e = by_guid(hop);
            stores.entry(hop).or_insert_with(|| (e, store(e)));
        }
    }
    if let Some((guid, entity)) = interact.as_deref().and_then(|i| Some((i.1?, i.0))) {
        stores
            .entry(guid)
            .or_insert_with(|| (entity, store(entity)));
    }
    for guid in script.iter().flat_map(|s| s.held_unit_guids()) {
        let e = by_guid(guid);
        stores.entry(guid).or_insert_with(|| (e, store(e)));
    }
    *start = FrameStart {
        epoch: log.epoch,
        mark: log.entries.len(),
        stores,
        records: group.map(|g| g.stats.clone()),
    };
}

/// The views lane: the stores and records as of the drain's place, and what it has pushed.
#[derive(Resource, Default)]
pub(crate) struct UnitViews {
    /// The frame's entries not yet applied, in stamp order.
    pending: VecDeque<Entry>,
    /// Each guid's store as of the drain's place, `None` for no object, with the entity its live
    /// store is read from: every unit the frame changed.
    scratch: HashMap<u64, (Option<Entity>, Option<ObjectStore>)>,
    /// Every group record as of the drain's place, while the frame changed any.
    records: Option<HashMap<u64, PartyMemberStatsInfo>>,
    /// The guids whose store or record the frame changed, of those some view reads.
    touched: HashSet<u64>,
    /// The guids some view reads: every unit a token can name, its target's targets through the
    /// frame, the group's members and pets and our pet.
    readable: Readable,
    /// The drain has fired the lane's start.
    started: bool,
    /// The guids `"target"` and `"npc"` named as the drain began: a call during it that moves one
    /// seats that token itself, and the lane leaves it to the next frame's feed.
    named: (Option<u64>, Option<u64>),
    /// What the VM held of each view before the lane pushed its own, put back as the drain ends.
    saved: families::Saved,
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

/// `guid`'s store in `scratch` while the drain replays it, else `live`.
fn scratch_as_of<'a>(
    scratch: &'a HashMap<u64, (Option<Entity>, Option<ObjectStore>)>,
    guid: u64,
    live: Option<&'a ObjectStore>,
) -> Option<&'a ObjectStore> {
    match scratch.get(&guid) {
        Some((_, store)) => store.as_ref(),
        None => live,
    }
}

impl UnitViews {
    /// `guid`'s store as of the drain's place: the rewound store while the drain replays it, else
    /// `live`.
    pub(crate) fn as_of<'a>(
        &'a self,
        guid: u64,
        live: Option<&'a ObjectStore>,
    ) -> Option<&'a ObjectStore> {
        scratch_as_of(&self.scratch, guid, live)
    }

    /// Apply every entry `until` admits; the guids they changed.
    fn apply(&mut self, until: Until) -> HashSet<u64> {
        let mut applied = HashSet::new();
        while self.pending.front().is_some_and(|e| until.admits(e.at)) {
            let Some(mut entry) = self.pending.pop_front() else {
                break;
            };
            for (guid, change) in &mut entry.changes {
                if !benilla_protocol::guid::is_item(*guid) {
                    applied.insert(*guid);
                }
                if let Change::Record(other) = change {
                    if let Some(records) = self.records.as_mut() {
                        swap_record(records, *guid, other);
                    }
                } else if let Some((_, store)) = self.scratch.get_mut(guid) {
                    change.redo(store);
                }
            }
        }
        applied
    }

    /// The drain's end for this lane: every store and record is back to the live one.
    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    fn finish<'a>(
        &mut self,
        live: impl Fn(Entity) -> Option<&'a ObjectStore>,
        live_records: &HashMap<u64, PartyMemberStatsInfo>,
    ) {
        #[cfg(debug_assertions)]
        {
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
            if let Some(records) = &self.records {
                debug_assert!(
                    records == live_records,
                    "views: the group records replayed to a roster the live one is not"
                );
            }
        }
        self.clear();
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.scratch.clear();
        self.records = None;
        self.touched.clear();
        self.readable = Readable::default();
        self.started = false;
        self.saved = Default::default();
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

/// The objects an inventory walk reads as of the drain's place ([`ViewStores::item_objects`]).
pub(crate) struct AsOfObjects<'a> {
    by_guid: Box<dyn Fn(u64) -> Option<&'a ObjectStore> + 'a>,
}

impl crate::ui_items::ItemObjects for AsOfObjects<'_> {
    fn object(&self, guid: u64) -> Option<&ObjectFields> {
        (self.by_guid)(guid).map(|s| &s.0)
    }
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

    /// `guid`'s store as of the drain's place, `live` its store now, `None` for no object: one the
    /// drain rewinds reads as its packet left it, a destroyed one included.
    pub(crate) fn by_guid<'a>(
        &'a self,
        guid: u64,
        live: Option<&'a ObjectStore>,
    ) -> Option<&'a ObjectStore> {
        match self.views.as_deref() {
            Some(views) => views.as_of(guid, live),
            None => live,
        }
    }

    /// `guid`'s store as of the drain's place, `entity` the one the object manager holds it under
    /// now ([`Self::by_guid`]): a unit a later packet of the frame destroys still reads as its
    /// packet left it.
    pub(crate) fn at(&self, guid: u64, entity: Option<Entity>) -> Option<&ObjectStore> {
        let live = entity
            .and_then(|e| self.live.get(e).ok())
            .map(|(store, _)| store);
        self.by_guid(guid, live)
    }

    /// The group records as of the drain's place, `live` the roster's now: while the drain replays
    /// a frame that wrote any, as its packet left them.
    pub(crate) fn records<'a>(
        &'a self,
        live: &'a HashMap<u64, PartyMemberStatsInfo>,
    ) -> &'a HashMap<u64, PartyMemberStatsInfo> {
        self.views
            .as_deref()
            .and_then(|v| v.records.as_ref())
            .unwrap_or(live)
    }

    /// The item objects as of the drain's place, `objects` the live ones: an item the frame's
    /// later packets destroy still reads to an earlier listener's walk.
    pub(crate) fn item_objects<'a>(
        &'a self,
        objects: &'a crate::net::Objects<'_, '_>,
    ) -> AsOfObjects<'a> {
        AsOfObjects {
            by_guid: Box::new(move |guid| self.by_guid(guid, objects.store(guid))),
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

/// What a held unit's snapshot reads, off `stores`.
fn held_units<'a>(
    src: &TokenSources<'a, '_, '_>,
    stores: &TokenStores<'a>,
) -> super::held::HeldUnits<'a> {
    let world = src.world;
    super::held::HeldUnits {
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
    }
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
    let held = held_units(src, stores);
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
        .add_drain_end(end_drain)
        .add_systems(
            Update,
            drop_unit_views
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
    #[cfg(debug_assertions)]
    app.add_systems(
        Update,
        mark_frame_start
            .before(crate::ui_script::UiInput)
            .before(benilla_world::schedule::WorldStage::Net),
    );
}

/// The lane's head: its start while the frame logged anything, then its next entry.
fn peek(world: &World) -> Option<Stamp> {
    let views = world.get_resource::<UnitViews>()?;
    if !views.started {
        let logged = world
            .get_resource::<ViewLog>()
            .is_some_and(|log| !log.entries.is_empty());
        return logged.then(Stamp::origin);
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
/// read from: `"player"`, `"target"` and `"npc"`. Any other unit's store is read from the entity
/// the object manager holds it under.
///
/// Each getter resolves its token at the call (`UnitHealth` calls `0x515970` at `0x517511`), so
/// a listener for an earlier packet reads the unit the token named then. Not yet: the tokens name
/// the frame's last selection, interaction and roster through the drain, and a token a call
/// moves during it is left to that call's own seat, until those are versioned too (#2217).
struct Seats {
    me: Option<(u64, Option<Entity>)>,
    target: Option<(u64, Option<Entity>)>,
    npc: Option<(u64, Option<Entity>)>,
}

impl Seats {
    /// The entity a seat reads `guid` from, when a seat names it.
    fn entity_of(&self, guid: u64) -> Option<Option<Entity>> {
        [self.me, self.target, self.npc]
            .into_iter()
            .flatten()
            .find(|(g, _)| *g == guid)
            .map(|(_, e)| e)
    }
}

/// The guids some view reads as the drain begins: every unit the resolver's last inputs can name
/// (`0x515970`), the target chains' ends among them, the seats, our pet, and each group
/// member with every pet its descriptor or record names (`0x4e81d0`, `0x491960`).
fn readable_guids<'a>(
    script: &UiScript,
    seats: &Seats,
    group: &crate::ui_party::GroupState,
    pet: u64,
    live: &impl Fn(u64) -> Option<&'a ObjectStore>,
) -> Readable {
    let g = script.unit_guids_view();
    let mut grouped: HashSet<u64> = (g.party.into_iter().chain(g.party_pets))
        .chain(g.raid.iter().copied())
        .chain(g.raid_pets.iter().copied())
        .collect();
    grouped.remove(&0);
    let mut out: HashSet<u64> = [g.player, g.pet, g.target, g.mouseover, g.npc, pet]
        .into_iter()
        .chain(g.party)
        .chain(g.party_pets)
        .chain(g.raid.iter().copied())
        .chain(g.raid_pets.iter().copied())
        .chain(g.held.keys().copied())
        .chain(g.held.values().copied())
        .chain(
            [seats.me, seats.target, seats.npc]
                .into_iter()
                .flatten()
                .map(|(g, _)| g),
        )
        .collect();
    for m in &group.members {
        out.insert(m.guid);
        out.extend(group.stats.get(&m.guid).and_then(|r| r.pet_guid));
        out.extend(live(m.guid).and_then(|s| s.0.unit_pet_guid()));
    }
    out.remove(&0);
    Readable { all: out, grouped }
}

/// The guids some view reads ([`readable_guids`]), and of them those a group token names, whose
/// aura list a roster record stands in for (`0x519735`).
#[derive(Default)]
struct Readable {
    all: HashSet<u64>,
    grouped: HashSet<u64>,
}

/// The items a store's seats name that the getters walk at mask 0 (`0x622420`): the active
/// player's equipment and equipped bags (0-22), backpack and keyring, private fields only its store
/// carries, whose weapons the stat getters read (`UnitAttackSpeed` `0x518f19`, `HasWandEquipped`
/// `0x4c99c2`) and whose whole set the bar's worn and reagent walks read; and a container's slots.
fn carried(f: &ObjectFields) -> Vec<Option<u64>> {
    use benilla_protocol::messages::ObjectType;
    match f.created_as() {
        Some(ObjectType::Player) => (0..23)
            .map(|i| f.player_inv_slot(i))
            .chain((0..16).map(|i| f.player_pack_slot(i)))
            .chain((0..32).map(|i| f.player_keyring_slot(i)))
            .collect(),
        Some(ObjectType::Container) => (0..36).map(|i| f.container_slot(i)).collect(),
        _ => Vec::new(),
    }
}

/// One guid's store from `live` back to the frame's start, undoing `entries`' changes to it in
/// reverse, checked against the frame-start snapshot `start` when given; with every object it named
/// on the way, by the fields a token resolves through: `UNIT_FIELD_TARGET` (a `target` chain,
/// `0x515970`), `UNIT_FIELD_CHARM` and `UNIT_FIELD_SUMMON` (a pet token, `0x4e8204`-`0x4e821a`),
/// and the weapons the active player wears, whose items the stat getters read
/// (`UnitAttackSpeed` `0x518f19`, `HasWandEquipped` `0x4c99c2`).
fn undo_frame(
    guid: u64,
    live: Option<ObjectStore>,
    entries: &mut [Entry],
    start: Option<&FrameStart>,
) -> (Option<ObjectStore>, Vec<u64>) {
    let hop = |s: &Option<ObjectStore>| {
        s.as_ref()
            .map(|s| {
                let f = &s.0;
                [f.unit_target(), f.unit_charm(), f.unit_summon()]
                    .into_iter()
                    .chain(carried(f))
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten()
            .flatten()
    };
    let mut store = live;
    let mut hops: Vec<u64> = hop(&store).collect();
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

/// The group records from `live` back to the frame's start, undoing `entries`' record changes in
/// reverse, checked against the frame-start snapshot `start` when given.
fn undo_records(
    live: &HashMap<u64, PartyMemberStatsInfo>,
    entries: &mut [Entry],
    start: Option<&FrameStart>,
) -> HashMap<u64, PartyMemberStatsInfo> {
    let mut records = live.clone();
    let mark = start.map_or(0, |s| s.mark);
    let mut at_mark = None;
    for (i, entry) in entries.iter_mut().enumerate().rev() {
        if start.is_some() && i + 1 == mark {
            at_mark = Some(records.clone());
        }
        for (guid, change) in entry.changes.iter_mut().rev() {
            if let Change::Record(other) = change {
                swap_record(&mut records, *guid, other);
            }
        }
    }
    if let Some(snapshot) = start.and_then(|s| s.records.as_ref()) {
        debug_assert!(
            at_mark.as_ref().unwrap_or(&records) == snapshot,
            "views: the group records rewound to a roster the frame did not start from: a write \
             the log missed"
        );
    }
    records
}

/// The drain's end, however it ended: one that stopped before this lane replayed all it took
/// leaves no rewound store for a later read, and the VM back on the views it held.
fn end_drain(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    if !world.get_resource::<UnitViews>().is_some_and(|v| v.started) {
        return;
    }
    if let Err(e) = world.run_system_once(restore_after_stop) {
        error!("views: the stopped drain's views are not put back: {e}");
        if let Some(mut views) = world.get_resource_mut::<UnitViews>() {
            views.clear();
        }
    }
}

/// [`end_drain`]'s putting back.
fn restore_after_stop(
    mut vm: crate::script_calls::VmWith<(
        ResMut<'static, UnitViews>,
        Option<Res<'static, Selection>>,
    )>,
    interact: Option<Res<crate::ui_session::InteractNpc>>,
) {
    vm.turn(|script, (mut views, selection)| {
        if let Some(script) = script {
            let target = selection.as_deref().and_then(|s| s.guid);
            let npc = interact.as_deref().and_then(|i| i.1);
            let skip = views.frozen(target, npc);
            let views = &mut *views;
            let _pushes = views.saved.restore(script, &skip);
            #[cfg(test)]
            {
                views.pushes += _pushes;
            }
        }
        views.clear();
    });
}

impl UnitViews {
    /// The lane's start: the frame's entries, and the store of each unit and the record of each
    /// member the frame changed rewound to the frame's start.
    fn rewind(
        &mut self,
        mut entries: Vec<Entry>,
        seats: Seats,
        mut readable: Readable,
        live: impl Fn(Entity) -> Option<ObjectStore>,
        by_guid: impl Fn(u64) -> Option<Entity>,
        live_records: &HashMap<u64, PartyMemberStatsInfo>,
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
        let entity_of = |guid: u64| seats.entity_of(guid).unwrap_or_else(|| by_guid(guid));
        let mut touched = HashSet::new();
        let mut stores = Vec::new();
        let mut records = false;
        for (guid, change) in entries.iter().flat_map(|e| &e.changes) {
            if let Change::Record(_) = change {
                records = true;
            } else {
                stores.push(*guid);
            }
            touched.insert(*guid);
        }
        stores.sort_unstable();
        stores.dedup();
        let mut scratch = HashMap::new();
        // Only a unit some view reads is rewound: what the others' entries hold drops unreplayed.
        // A view reads too each unit a readable one named during the frame, by its target or its
        // pet (`"targettarget"`, `"partypet1"` before a swap), and a pet a member's earlier record
        // named; so each rewound store's names join the readable, and are rewound in turn.
        let mut queue: Vec<u64> = Vec::new();
        let name = |g: u64, readable: &mut Readable, queue: &mut Vec<u64>| {
            if g != 0 && readable.all.insert(g) && stores.binary_search(&g).is_ok() {
                queue.push(g);
            }
        };
        for (_, change) in entries.iter().flat_map(|e| &e.changes) {
            if let Change::Record(Some(record)) = change {
                name(record.pet_guid.unwrap_or(0), &mut readable, &mut queue);
            }
        }
        queue.extend(stores.iter().filter(|g| readable.all.contains(g)));
        while let Some(guid) = queue.pop() {
            if scratch.contains_key(&guid) {
                continue;
            }
            let entity = entity_of(guid);
            let (store, named) = undo_frame(
                guid,
                entity.and_then(&live),
                &mut entries,
                start_for(guid, entity),
            );
            for g in named {
                name(g, &mut readable, &mut queue);
            }
            scratch.insert(guid, (entity, store));
        }
        // An untouched store the snapshot holds is not rewound, but is checked all the same.
        for &guid in start.iter().flat_map(|s| s.stores.keys()) {
            if scratch.contains_key(&guid) {
                continue;
            }
            let entity = entity_of(guid);
            if let Some(check) = start_for(guid, entity) {
                undo_frame(guid, entity.and_then(&live), &mut entries, Some(check));
            }
        }
        // The records likewise, rewound only when the frame changed one, and checked all the same.
        let checked_records = start.is_some_and(|s| s.records.is_some());
        let rewound =
            (records || checked_records).then(|| undo_records(live_records, &mut entries, start));
        self.records = rewound.filter(|_| records);
        self.pending = entries
            .into_iter()
            .filter_map(|mut entry| {
                entry
                    .changes
                    .retain(|(g, c)| matches!(c, Change::Record(_)) || scratch.contains_key(g));
                (!entry.changes.is_empty()).then_some(entry)
            })
            .collect();
        // An item no view reads: it is rewound for the getters alone, and builds nothing.
        touched.retain(|&g| {
            !benilla_protocol::guid::is_item(g)
                && (scratch.contains_key(&g) || readable.all.contains(&g))
        });
        self.named = (seats.target.map(|(g, _)| g), seats.npc.map(|(g, _)| g));
        self.scratch = scratch;
        self.touched = touched;
        self.readable = readable;
        #[cfg(test)]
        {
            self.starts_checked += checked.get();
        }
    }

    /// The tokens a call during the drain seated itself, by moving the selection from the guid
    /// the drain began with to `target` or the interaction to `npc`: the lane leaves them.
    fn frozen(&self, target: Option<u64>, npc: Option<u64>) -> Vec<&'static str> {
        let mut frozen = Vec::new();
        if target != self.named.0 {
            frozen.extend(["target", "targettarget"]);
        }
        if npc != self.named.1 {
            frozen.push("npc");
        }
        frozen
    }
}

/// What the lane's firing reads beside the VM and the stores it rewinds.
#[derive(SystemParam)]
pub(crate) struct ViewSources<'w, 's> {
    tables: super::SnapshotTables<'w>,
    names: Res<'w, NameCache>,
    commands: Res<'w, NetCommands>,
    guild: Res<'w, crate::ui_guild::GuildState>,
    index: Option<Res<'w, GuidIndex>>,
    interact: Option<Res<'w, crate::ui_session::InteractNpc>>,
    all: Query<'w, 's, &'static ObjectStore>,
    self_q: Query<'w, 's, (Entity, &'static Guid), With<SelfPlayer>>,
    /// What the group tokens read beside the roster: the faction table and the area names.
    factions: Option<Res<'w, crate::target::Factions>>,
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    auras: crate::ui_aura::AuraViews<'w>,
    start: Option<Res<'w, FrameStart>>,
}

/// The lane's `fire_until` ([`crate::lanes`]). Its start takes the frame's log and rewinds
/// ([`UnitViews::rewind`]). Each firing applies every entry `until` admits, then pushes the views
/// as of there, one build for the whole gap: at the start every view the frame's changes reach,
/// then those the gap's do. A start that admits every entry finds nothing due before the feeds'
/// pushes, and builds nothing; the last entry puts back what the VM held, the feeds' own views.
#[allow(clippy::type_complexity)] // the snapshots' inputs, as the feeds read them
fn fire_unit_views(
    In(until): In<Until>,
    mut vm: crate::script_calls::VmWith<(
        ResMut<'static, UnitViews>,
        Res<'static, Selection>,
        RelationWorld<'static, 'static>,
        ResMut<'static, crate::ui_party::GroupState>,
        // Inside the set: the call host holds some of what the views read.
        ViewSources<'static, 'static>,
    )>,
    mut log: ResMut<ViewLog>,
) {
    vm.turn(|script, (mut views, selection, world, mut group, src)| {
        let ViewSources {
            tables,
            names,
            commands,
            guild,
            index,
            interact,
            all,
            self_q,
            factions,
            areas,
            auras,
            start,
        } = &src;
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
            let start = {
                let _ = start;
                None
            };
            let entries = log.take();
            let seats = Seats {
                me: self_q.iter().next().map(|(e, g)| (g.0, Some(e))),
                target: selection.guid.map(|g| (g, selection.target)),
                npc: npc_seat,
            };
            let live_store = |guid: u64| by_guid(guid).and_then(|e| all.get(e).ok());
            let me = self_q.iter().next().and_then(|(e, _)| all.get(e).ok());
            let pet = crate::ui_unit::pet_token_guid(me);
            let readable = readable_guids(script, &seats, &group, pet, &live_store);
            views.rewind(entries, seats, readable, live, by_guid, &group.stats, start);
            views.started = true;
        }
        let applied = views.apply(until);
        let last = views.pending.is_empty();
        let live_store = |e: Entity| all.get(e).ok();
        if first && last {
            views.finish(live_store, &group.stats);
            return;
        }
        let frozen = views.frozen(selection.guid, npc_seat.map(|(g, _)| g));
        if last {
            let views = &mut *views;
            let _pushes = views.saved.restore(script, &frozen);
            #[cfg(test)]
            {
                views.pushes += _pushes;
            }
            views.finish(live_store, &group.stats);
            return;
        }
        let dirty = if first {
            views.touched.clone()
        } else {
            applied
        };
        // A gap whose entries no view reads builds nothing.
        if dirty.is_empty() {
            return;
        }
        #[cfg(test)]
        {
            views.builds += 1;
        }
        // The records as of here stand in the roster while the views are built.
        let mut records = views.records.take();
        if let Some(records) = records.as_mut() {
            std::mem::swap(&mut group.bypass_change_detection().stats, records);
        }
        let UnitViews {
            scratch,
            saved,
            readable,
            ..
        } = &mut *views;
        let scratch = &*scratch;
        let object =
            |guid: u64| scratch_as_of(scratch, guid, by_guid(guid).and_then(|e| all.get(e).ok()));
        let held = |guid: u64, entity: Option<Entity>| {
            let live = entity.and_then(|e| all.get(e).ok());
            Some((scratch_as_of(scratch, guid, live)?, guid))
        };
        let me = self_q.iter().next().and_then(|(e, g)| held(g.0, Some(e)));
        let src = TokenSources {
            tables,
            names,
            commands,
            group: &group,
            guild,
            world: &world,
        };
        let stores = TokenStores {
            me,
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
        let mut pushes = 0;
        // The four tokens when one of their units moved: ours, the target, its target, the NPC.
        let hop = stores.target.and_then(|(s, _)| s.0.unit_target());
        let tokens_moved = [
            self_q.iter().next().map(|(_, g)| g.0),
            selection.guid,
            hop,
            npc_seat.map(|(g, _)| g),
        ]
        .into_iter()
        .flatten()
        .any(|g| dirty.contains(&g));
        if tokens_moved {
            for (i, view) in token_views(&src, &stores).into_iter().enumerate() {
                // `"player"` is pushed only while its descriptor exists, as the feed pushes it.
                if frozen.contains(&TOKENS[i]) || (i == 0 && view.is_none()) {
                    continue;
                }
                pushes += usize::from(saved.token(script, TOKENS[i], view));
            }
        }
        let held_units = held_units(&src, &stores);
        let rel = world.get();
        let ffa = |guid: u64| {
            object(guid).is_some_and(|store| super::pvp_free_for_all_of(&rel, guid, store))
        };
        let ctx = families::Families {
            dirty: &dirty,
            grouped: &readable.grouped,
            object: &object,
            me: me.map(|(s, g)| (g, s)),
            live_me: self_q
                .iter()
                .next()
                .and_then(|(e, g)| Some((g.0, all.get(e).ok()?))),
            group: &group,
            tables,
            names,
            commands,
            guild,
            factions: factions.as_deref(),
            areas: areas.as_deref().map(|a| &a.0),
            auras,
            world: &world,
            ffa: &ffa,
            held: &held_units,
        };
        pushes += families::push(script, saved, &ctx);
        if let Some(records) = records.as_mut() {
            std::mem::swap(&mut group.bypass_change_detection().stats, records);
        }
        views.records = records;
        #[cfg(test)]
        {
            views.pushes += pushes;
        }
        let _ = pushes;
    });
}

#[cfg(test)]
impl UnitViews {
    /// Whether the lane holds a rewound store for `guid`.
    pub(crate) fn holds(&self, guid: u64) -> bool {
        self.scratch.contains_key(&guid)
    }
}
