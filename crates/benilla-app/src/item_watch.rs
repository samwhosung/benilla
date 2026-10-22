//! The item watchers: the reference's handlers that run when one of the active player's items,
//! or a seat that holds one, changes. The object packets ready one [`Run`] per handler run, its
//! [`ItemNotice`], the unlocks it opens with, the bag events and the unit signals it ends in, onto
//! the item lane ([`crate::item_lane`]), which fires each whole at its packet's place: the unlocks
//! (`0x495420`, `0x6221e0`), the action bar's recount (`0x4e5c70`), the trade's slot (`0x4bfb10`,
//! `0x4bfb60`), the quest log's ask for its item (`0x4df7b0`) and the bag events (`0x4f9370`,
//! `0x4f91a0`, `0x5ddd69`).
//!
//! A block's notify runs each watcher whose watched bytes now differ from the copy its object's last
//! write in the packet took, once, at the first field of its range the block visits (`0x465570`
//! sets the record's countdown to 1), so the notices of one block come out in field order. A first
//! create runs no watcher; a create of an object already shown runs them over every field
//! ([`crate::field_watch::Notified`]).
//! One notice belongs to no item: the landing of a record a trade getter asked for
//! ([`ItemNotice::TradeRecord`]), which signals `TRADE_UPDATE`.

use benilla_protocol::field::{
    FIELD_CONTAINER_SLOT_1, FIELD_ITEM_ENCHANTMENT, FIELD_ITEM_SPELL_CHARGES,
    FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD,
    FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR,
};
use benilla_protocol::messages::{ObjectType, ITEM_FLAG_CHARTER};
use benilla_protocol::ObjectFields;
use bevy::prelude::*;

use crate::field_watch::{id, FieldNotice, List, Place};
use crate::items::Items;
use crate::query_cache::Ask;

/// One run of a reference handler on the active player's items, in the order the packets ran
/// them. Entries are `OBJECT_FIELD_ENTRY` values; an item the handler could not find has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ItemNotice {
    /// `CGItem_C::Deactivate` (`0x5d8660`) on our item, from a destroy or the out-of-range list
    /// (`0x464920` runs it for both): the interface refreshes from `0x5d8786` on.
    Destroyed { guid: u64, entry: u32 },
    /// The record set up (`0x5d8440`) on our item while in the world (`[0xb4b424]`, `0x5d84ff`):
    /// at its create when the record is cached (`0x5d8300`), else when the answer lands
    /// (`0x5d8390`).
    Record { guid: u64, entry: u32 },
    /// The answer to an [`Self::Entry`] miss, its callback `0x5d96b0`: the new entry's slots
    /// refresh (`0x5d96bd`), then `0x4f9480` and `0x4c7180` with the guid. It has no world latch
    /// and finds no item: it runs for the request, whatever the item has become since.
    EntryRecord { guid: u64, entry: u32 },
    /// `ITEM_FIELD_STACK_COUNT`'s watcher `0x5d9400`.
    StackCount { guid: u64, entry: u32 },
    /// `ITEM_FIELD_SPELL_CHARGES`'s watcher `0x5d9540`.
    SpellCharges { guid: u64, entry: u32 },
    /// `OBJECT_FIELD_ENTRY`'s watcher `0x5d95e0`: the old entry's slots refresh at once
    /// (`0x5d963e`), the new one's when its record is cached (`0x5d9676`); a miss asks with the
    /// callback `0x5d96b0` and waits for the answer, an [`Self::EntryRecord`].
    Entry {
        guid: u64,
        old: u32,
        new: u32,
        new_cached: bool,
    },
    /// `ITEM_FIELD_ENCHANTMENT`'s watcher `0x5d95a0` into `0x5d9a90`, which passes over a charter
    /// (record flags `0x2000`, `0x5d9aa8`) and acts for the active player's item only.
    Enchantment { guid: u64, entry: u32 },
    /// A seat of the backpack or keyring (`0x4f8db0`) or of an equipped or bank bag (`0x4f90d0`)
    /// took another guid: the items that left and arrived, each when it is still an object.
    Seat { old: Option<u32>, new: Option<u32> },
    /// A bag seat, 19-22 or 63-68, took another guid (`0x4f8ec0`): the entries of the new bag's
    /// items, in slot order (`0x4f8f48`).
    Bag { entries: Vec<u32> },
    /// One of our visible items changed (`0x5dee30`, for the active player): the old and new
    /// `PLAYER_VISIBLE_ITEM_<n>_0`.
    Visible { old: u32, new: u32 },
    /// The record a trade getter asked for landed: its callback `0x4c0340` signals `TRADE_UPDATE`
    /// (`0x4c034f`), for a ready record only. Each getter call that missed registered one callback
    /// with no dedupe (`0x4c017f`, `0x4c0582`), so an answer writes one notice per call, after the
    /// notices of the handlers that registered earlier. It concerns no item of ours.
    TradeRecord { entry: u32 },
}

/// What a notice asks of the trade's own column (`0x4bfb10`, `0x4bfb60`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TradeTouch {
    /// The item is gone: its slot clears (`0x5d87a7`).
    Gone(u64),
    /// The item restacked or took an enchantment: its slot is placed again (`0x5d94f0`,
    /// `0x5d9b0c`).
    Changed(u64),
}

/// An unlock a run opens with, ahead of its recount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unlock {
    /// `UnlockItem` (`0x495420`) on one nonzero guid: its `ITEM_LOCK_CHANGED` fires whether or
    /// not the guid names an item we hold (`0x495450`).
    Item(u64),
    /// A stackable item's arrival or stack-count change (`0x5d8590`, `0x5d94af` into `0x6221e0`):
    /// every item of ours with its entry the walk reaches ([`kin`]), bar the trade, mail and
    /// auction attachments, unlocks, then one `ITEM_LOCK_CHANGED` (`0x5d859a`, `0x5d94b9`).
    Entry(Vec<u64>),
}

/// A bag event a run signals after its recount, at its handler's place in the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BagEvent {
    /// `RaiseBagUpdate` (`0x4f9370`): the action bar's usable refresh `0x4e5c00` (`0x4f9374`), then
    /// `BAG_UPDATE` with the container's number when it is the player (0) or a cached bag
    /// ([`BagCache::number`]), else nothing more (`0x4f93ae`).
    Raise(Option<i64>),
    /// `BAG_UPDATE` signalled directly: the keyring's -2 (`0x4f8e8a`) and the rescan's (`0x4f9343`).
    Update(i64),
    /// `BAG_CLOSED` (`0x4f92b5`), the rescan's alone.
    Closed(i64),
    /// `PLAYERBANKSLOTS_CHANGED` with no argument, the inventory watcher's (`0x5ddd69`).
    BankSlots,
    /// `BAG_OPEN` (`0x4f9434`, `0x4f9469`), `SMSG_OPEN_CONTAINER`'s ([`BagCache::opens`]).
    Open(i64),
}

/// One run of an item watcher's callback, readied at its packet: the notice our item's readers act
/// on, when the item is ours, the unlocks it opens with, the bag events it signals and the unit
/// signals it ends in, each in its order. A record set-up on another's item runs the owner's
/// signal alone (`0x5d8521` → `0x5d85ed`); the inventory watcher `0x5ddcf0` runs an unlock and the
/// bank's event with no notice, and `SMSG_OPEN_CONTAINER` its `BAG_OPEN`s.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Run {
    pub(crate) notice: Option<ItemNotice>,
    pub(crate) signals: Vec<FieldNotice>,
    pub(crate) unlocks: Vec<Unlock>,
    pub(crate) bags: Vec<BagEvent>,
}

impl Run {
    /// Whether the run does nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.notice.is_none()
            && self.signals.is_empty()
            && self.unlocks.is_empty()
            && self.bags.is_empty()
    }

    /// How many times the run calls `RaiseBagUpdate` (`0x4f9370`), each of which runs the action
    /// bar's usable refresh (`0x4e5c00`, `0x4f9374`) whether or not it signals.
    pub(crate) fn bag_raises(&self) -> usize {
        self.bags
            .iter()
            .filter(|e| matches!(e, BagEvent::Raise(_)))
            .count()
    }
}

/// The bag cache `0xbdd060`: the guids of bags 1 to 10 (the four equipped, then the six bank bags)
/// as the rescan `0x4f91a0` last stored them. The world-enter cascade's watcher set-up zeroes and
/// rescans it (`0x4f8cc0`, `0x4f8da6`); a bag seat's watcher rescans it (`0x4f8f66`). A bag in it
/// has its slots watched (`0x4f932a`, removed at `0x4f9299`) and a number its events carry.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BagCache(
    pub(crate) [u64; 10],
    /// Each bag's watched slots: its slot count when the rescan stored it, read off the bag
    /// object (`0x4f92e3`, `0x4f92fd`), and none for a bag the rescan did not find (`0x4f92ec`).
    pub(crate) [u8; 10],
);

impl BagCache {
    /// The guid in the seat of bag `n`, 1 to 10: seat `n + 18` for 1-4, `n + 58` for 5-10
    /// (`0x4f91f8`, `0x4f9204`).
    fn seat_of(player: &ObjectFields, n: u8) -> u64 {
        seat_guid(player, if n <= 4 { n + 18 } else { n + 58 })
    }

    /// `RaiseBagUpdate`'s number for `container` (`0x4f9370`): 0 for the active player
    /// (`0x4f938c`), the cache index plus 1 for a cached bag (`0x4f93b3`), else none.
    pub(crate) fn number(&self, me: Option<u64>, container: u64) -> Option<i64> {
        if me == Some(container) {
            return Some(0);
        }
        self.0
            .iter()
            .position(|&g| g == container)
            .map(|i| i as i64 + 1)
    }

    /// `0x4f9410`'s `BAG_OPEN` numbers for `container`: 0 for the active player alone
    /// (`0x4f9428`), else every cache index it matches, plus 1, the scan going on past a match
    /// (`0x4f9445`-`0x4f9475`).
    pub(crate) fn opens(&self, me: Option<u64>, container: u64) -> Vec<i64> {
        if me == Some(container) {
            return vec![0];
        }
        (1..=10)
            .zip(self.0)
            .filter(|&(_, g)| g == container)
            .map(|(n, _)| n)
            .collect()
    }

    /// How many of `bag`'s slots are watched: its count when the cache stored it.
    fn watched(&self, bag: u64) -> u8 {
        self.0
            .iter()
            .zip(self.1)
            .filter(|&(&g, _)| g == bag)
            .map(|(_, n)| n)
            .max()
            .unwrap_or(0)
    }

    /// The rescan `0x4f91a0` over the active player's bag seats: a bag whose guid differs from the
    /// cache fires `BAG_CLOSED` when the old one was not zero (`0x4f9245`, `0x4f92b5`), is stored
    /// (`0x4f92c7`) with its slots watched over the count `lookup` finds (`0x4f92fd`), and fires
    /// `BAG_UPDATE` when the new one is not zero (`0x4f92cc`, `0x4f9343`), bag by bag.
    fn rescan<'a>(
        &mut self,
        player: &ObjectFields,
        lookup: &dyn Fn(u64) -> Option<&'a ObjectFields>,
    ) -> Vec<BagEvent> {
        let mut out = Vec::new();
        for n in 1..=10u8 {
            let i = usize::from(n - 1);
            let now = Self::seat_of(player, n);
            if now == self.0[i] {
                continue;
            }
            if self.0[i] != 0 {
                out.push(BagEvent::Closed(i64::from(n)));
            }
            self.0[i] = now;
            self.1[i] = slot_count(now, lookup);
            if now != 0 {
                out.push(BagEvent::Update(i64::from(n)));
            }
        }
        out
    }

    /// The world-enter cascade's set-up of the cache (`0x4f8cc0`: zeroed, then rescanned at
    /// `0x4f8da6`), which leaves it holding the player's bags, each watched over its count; its
    /// `BAG_UPDATE`s fire with the interface's world-enter events.
    pub(crate) fn seat<'a>(
        &mut self,
        player: &ObjectFields,
        lookup: &dyn Fn(u64) -> Option<&'a ObjectFields>,
    ) {
        self.0 = std::array::from_fn(|i| Self::seat_of(player, i as u8 + 1));
        self.1 = self.0.map(|g| slot_count(g, lookup));
    }
}

/// The slot count the rescan registers a bag's watchers over: the container's first field, read
/// off the object `0x468460(guid, TYPEMASK_CONTAINER)` finds (`0x4f92d9`-`0x4f9300`), none when
/// it finds none or the count is not positive (`0x4f9304`).
fn slot_count<'a>(bag: u64, lookup: &dyn Fn(u64) -> Option<&'a ObjectFields>) -> u8 {
    if bag == 0 {
        return 0;
    }
    lookup(bag)
        .filter(|f| f.is_container())
        .and_then(|f| f.container_num_slots())
        .map_or(0, |n| n.min(36) as u8)
}

/// The scopes of the inventory walk `0x622420` the entry unlocks take (`0x6221e0`): the arrival's
/// flags 0, which the walk widens to equipment, bags, backpack and keyring (`0x622439`), and the
/// stack-count write's 8, which adds the bank and its bags. Neither reaches buyback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Walk {
    Arrival,
    Stack,
}

/// The items of `entry` the walk reaches from the player `store`, sorted. A bag reached is walked
/// inside (`0x62256d`). A broken item, durability 0 of a nonzero maximum on an unwrapped item, and
/// one flagged `0x10` are passed over (`0x6224f7`-`0x622513`).
pub(crate) fn kin<'a>(
    store: &ObjectFields,
    lookup: &dyn Fn(u64) -> Option<&'a ObjectFields>,
    walk: Walk,
    entry: u32,
) -> Vec<u64> {
    let in_scope = |i: u8| match i {
        0..=38 | 81..=112 => true,
        39..=68 => walk == Walk::Stack,
        _ => false,
    };
    let passed_over = |f: &ObjectFields| {
        let flags = f.item_flags().unwrap_or(0);
        let broken = flags & 0x8 == 0
            && f.item_max_durability().unwrap_or(0) > 0
            && f.item_durability().unwrap_or(0) == 0;
        broken || flags & 0x10 != 0
    };
    fn walk_item<'a>(
        guid: u64,
        lookup: &dyn Fn(u64) -> Option<&'a ObjectFields>,
        entry: u32,
        passed_over: &dyn Fn(&ObjectFields) -> bool,
        out: &mut Vec<u64>,
    ) {
        let Some(f) = lookup(guid).filter(|f| !passed_over(f)) else {
            return;
        };
        if f.object_entry() == Some(entry) {
            out.push(guid);
        }
        let n = f.container_num_slots().unwrap_or(0).min(36) as u8;
        for j in 0..n {
            if let Some(inner) = f.container_slot(j).filter(|&g| g != 0) {
                walk_item(inner, lookup, entry, passed_over, out);
            }
        }
    }
    let mut out = Vec::new();
    for seat in (0..113u8).filter(|&i| in_scope(i)) {
        let guid = seat_guid(store, seat);
        if guid != 0 {
            walk_item(guid, lookup, entry, &passed_over, &mut out);
        }
    }
    out.sort_unstable();
    out
}

impl ItemNotice {
    /// The entries the handler passes to the action bar's `0x4e5c70`, in call order.
    pub(crate) fn recounts(&self) -> Vec<u32> {
        match self {
            Self::Destroyed { entry, .. }
            | Self::Record { entry, .. }
            | Self::EntryRecord { entry, .. }
            | Self::StackCount { entry, .. }
            | Self::SpellCharges { entry, .. }
            | Self::Enchantment { entry, .. } => vec![*entry],
            Self::Entry {
                old,
                new,
                new_cached,
                ..
            } => {
                let mut out = vec![*old];
                if *new_cached {
                    out.push(*new);
                }
                out
            }
            Self::Seat { old, new } => old.iter().chain(new.iter()).copied().collect(),
            Self::Bag { entries } => entries.clone(),
            Self::Visible { old, new } => vec![*old, *new],
            Self::TradeRecord { .. } => Vec::new(),
        }
    }

    /// The trade's part, after the action bar's: each handler calls `0x4e5c70` before the trade
    /// scan (`0x5d8798` then `0x5d87a7`, `0x5d94e1` then `0x5d94f0`, `0x5d9afd` then
    /// `0x5d9b0c`).
    pub(crate) fn trade(&self) -> Option<TradeTouch> {
        match *self {
            Self::Destroyed { guid, .. } => Some(TradeTouch::Gone(guid)),
            Self::StackCount { guid, .. } | Self::Enchantment { guid, .. } => {
                Some(TradeTouch::Changed(guid))
            }
            _ => None,
        }
    }
}

/// The watched part of the player's array, in the reference's flat numbering (`(n - 0x4a8) / 8`):
/// the bag seats under `0x4f8ec0` and the backpack and keyring under `0x4f8db0`, as `0x4f8cc0`
/// registers them. Equipment (0-18), the bank (39-62) and buyback (69-80) have no watcher here.
fn seat_watcher(seat: u8) -> Option<SeatWatcher> {
    match seat {
        19..=22 | 63..=68 => Some(SeatWatcher::Bag),
        23..=38 | 81..=112 => Some(SeatWatcher::Item),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SeatWatcher {
    Bag,
    Item,
}

/// The seats the inventory watcher `0x5ddcf0` watches, as `0x5dd8a0` registers it: equipment and
/// bags (`0x4a8`-`0x558`), the bank and its bags (`0x5e0`-`0x6c8`) and the keyring
/// (`0x730`-`0x828`); not the backpack or buyback.
fn inventory_watched(seat: u8) -> bool {
    matches!(seat, 0..=22 | 39..=68 | 81..=112)
}

/// The equipment slots with a visible-item watcher: mask `0x7c3fd` of `0x5e2510`, slots 0 and 2-9
/// and 14-18; the neck, rings and trinkets have none.
const VISIBLE_WATCHED: u32 = 0x7c3fd;

/// What the watchers read besides the block: our guid, the world latch, the object lookup and the
/// record cache.
pub(crate) struct Watch<'a, F>
where
    F: Fn(u64) -> Option<&'a ObjectFields>,
{
    /// The active player's guid (`0x468550`), known from the login on.
    pub(crate) me: Option<u64>,
    /// `0x468460(guid, TYPEMASK_ITEM)` and kin: a live object's fields.
    pub(crate) lookup: F,
    pub(crate) items: &'a Items,
}

impl<'a, F> Watch<'a, F>
where
    F: Fn(u64) -> Option<&'a ObjectFields>,
{
    /// The world latch `[0xb4b424]`, set when our own player arrives (`0x4908ce`): our player is
    /// a live object.
    fn in_world(&self) -> bool {
        self.me.is_some_and(|me| (self.lookup)(me).is_some())
    }

    /// The item behind a guid and its entry, as `0x468460(guid, 2)` finds it.
    fn entry_of(&self, guid: u64) -> Option<u32> {
        if guid == 0 {
            return None;
        }
        (self.lookup)(guid)
            .filter(|f| f.is_item())
            .and_then(ObjectFields::object_entry)
    }

    /// Whether `fields` is an item the active player owns (`ITEM_FIELD_OWNER`, `[item+0x114]`).
    fn ours(&self, fields: &ObjectFields) -> bool {
        fields.is_item() && self.me.is_some() && fields.item_owner() == self.me
    }

    /// The notices of one block's notify on `guid` ([`crate::field_watch::Notified`]): `fields`
    /// is the store now.
    #[cfg(test)]
    pub(crate) fn values(
        &self,
        guid: u64,
        kind: ObjectType,
        fields: &ObjectFields,
        block: &crate::field_watch::Notified,
        bags: &mut BagCache,
    ) -> Vec<ItemNotice> {
        self.placed_values(guid, kind, fields, block, bags)
            .into_iter()
            .filter_map(|(_, r)| r.notice)
            .collect()
    }

    /// The runs of one block's notify on `guid` ([`crate::field_watch::Notified`]), `fields` the
    /// store now, each with its watch's place in the notify pass
    /// ([`crate::field_watch::in_watch_order`]): the item type watchers `0x5d9360` registers
    /// (`0x468070`) and the visible-item ones of `0x5e2510`, on the type list; the inventory
    /// watcher `0x5dd8a0` registers, then the seat watchers `0x4f8cc0` and `0x4f932a` register, on
    /// the player and on each cached bag (`0x467e70`), on the object's own, in that order at one
    /// seat. A bag seat's run rescans `bags` as it is built, in field order.
    pub(crate) fn placed_values(
        &self,
        guid: u64,
        kind: ObjectType,
        fields: &ObjectFields,
        block: &crate::field_watch::Notified,
        bags: &mut BagCache,
    ) -> Vec<(Place, Run)> {
        if block.is_quiet() {
            return Vec::new();
        }
        let old_of = |index: u16, now: u32| block.edge(index).map_or(now, |(old, _)| old);
        let place = |range: std::ops::Range<u16>, list: List| block.place(range, list);
        let mut out: Vec<(Place, Run)> = Vec::new();
        match kind {
            ObjectType::Item | ObjectType::Container if self.ours(fields) => {
                let entry = fields.object_entry().unwrap_or(0);
                let mut notices = Vec::new();
                if let Some(at) = place(FIELD_OBJECT_ENTRY..FIELD_OBJECT_ENTRY + 1, List::Type) {
                    let old = old_of(FIELD_OBJECT_ENTRY, entry);
                    notices.push((
                        at,
                        ItemNotice::Entry {
                            guid,
                            old,
                            new: entry,
                            new_cached: self.items.template(entry, 0, Ask::Read).is_some(),
                        },
                    ));
                }
                if let Some(at) = place(
                    FIELD_ITEM_STACK_COUNT..FIELD_ITEM_STACK_COUNT + 1,
                    List::Type,
                ) {
                    notices.push((at, ItemNotice::StackCount { guid, entry }));
                }
                if let Some(at) = place(
                    FIELD_ITEM_SPELL_CHARGES..FIELD_ITEM_SPELL_CHARGES + 5,
                    List::Type,
                ) {
                    notices.push((at, ItemNotice::SpellCharges { guid, entry }));
                }
                // `0x5d9a90`: a record missing reads flags 0 and passes; the owner must be the
                // active player, found live (`0x5d9acd`, `0x5d9ae8`).
                let charter = self
                    .items
                    .template(entry, 0, Ask::Read)
                    .is_some_and(|t| t.flags & ITEM_FLAG_CHARTER != 0);
                if let Some(at) = place(
                    FIELD_ITEM_ENCHANTMENT..FIELD_ITEM_ENCHANTMENT + 21,
                    List::Type,
                ) {
                    if !charter && self.in_world() {
                        notices.push((at, ItemNotice::Enchantment { guid, entry }));
                    }
                }
                out.extend(notices.into_iter().map(|(at, n)| (at, self.run(n, bags))));
            }
            _ => {}
        }
        // A bag's own seats, watched while it is in the bag cache (`0x4f932a`, `0x4f9299`).
        if kind == ObjectType::Container {
            let count = fields.container_num_slots().unwrap_or(0);
            for j in 0..bags.watched(guid) {
                let lo = FIELD_CONTAINER_SLOT_1 + 2 * u16::from(j);
                let Some(at) = place(lo..lo + 2, List::Object) else {
                    continue;
                };
                // A seat past the bag's count reads empty (`0x4f9106`).
                let now = if u32::from(j) < count {
                    fields.container_slot(j).unwrap_or(0)
                } else {
                    0
                };
                let was = guid_of(old_of(lo, now as u32), old_of(lo + 1, (now >> 32) as u32));
                // `0x4f90d0`: a changed seat unlocks its item (`0x4f912c`) and recounts; changed or
                // not, the bag's `BAG_UPDATE` (`0x4f918e`).
                let mut run = Run {
                    bags: vec![BagEvent::Raise(bags.number(self.me, guid))],
                    ..Run::default()
                };
                if was != now {
                    run.notice = Some(ItemNotice::Seat {
                        old: self.entry_of(was),
                        new: self.entry_of(now),
                    });
                    run.unlocks.extend(unlock_of(now));
                }
                out.push((at, run));
            }
        }
        if kind == ObjectType::Player && self.me == Some(guid) {
            for i in 0..19u8 {
                if VISIBLE_WATCHED & (1 << i) == 0 {
                    continue;
                }
                let base = FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 12 * u16::from(i);
                let Some(at) = place(base..base + 12, List::Type) else {
                    continue;
                };
                let new = fields.player_visible_item_entry(i).unwrap_or(0);
                let notice = ItemNotice::Visible {
                    old: old_of(base + 2, new),
                    new,
                };
                out.push((at, self.run(notice, bags)));
            }
            for seat in 0..113u8 {
                let lo = FIELD_PLAYER_INV_SLOT_HEAD + 2 * u16::from(seat);
                let Some(at) = place(lo..lo + 2, List::Object) else {
                    continue;
                };
                let now = seat_guid(fields, seat);
                let was = guid_of(old_of(lo, now as u32), old_of(lo + 1, (now >> 32) as u32));
                // `0x5ddcf0`, registered ahead of the seat watchers (`0x5dd8a0`): the item now in
                // a changed seat unlocks (`0x5ddd50`), and a bank seat signals
                // `PLAYERBANKSLOTS_CHANGED` (`0x5ddd55`-`0x5ddd6e`).
                if inventory_watched(seat) {
                    let mut run = Run::default();
                    if was != now {
                        run.unlocks.extend(unlock_of(now));
                    }
                    if (39..=68).contains(&seat) {
                        run.bags.push(BagEvent::BankSlots);
                    }
                    out.push((at, run));
                }
                match seat_watcher(seat) {
                    // `0x4f8ec0`: no equality test; a container's items, each found live, then the
                    // rescan (`0x4f8f66`).
                    Some(SeatWatcher::Bag) => {
                        let entries = (self.lookup)(now)
                            .filter(|f| f.is_container())
                            .map(|bag| {
                                let n = bag.container_num_slots().unwrap_or(0).min(36) as u8;
                                (0..n)
                                    .filter_map(|j| bag.container_slot(j))
                                    .filter_map(|g| self.entry_of(g))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let run = Run {
                            notice: Some(ItemNotice::Bag { entries }),
                            bags: bags.rescan(fields, &self.lookup),
                            ..Run::default()
                        };
                        out.push((at, run));
                    }
                    // `0x4f8db0`: a changed seat unlocks its item (`0x4f8e15`) and recounts;
                    // changed or not, the keyring's `BAG_UPDATE(-2)` (`0x4f8e8a`) or the
                    // backpack's through `0x4f9370` (`0x4f8ea6`).
                    Some(SeatWatcher::Item) => {
                        let bag = if seat >= 81 {
                            BagEvent::Update(-2)
                        } else {
                            BagEvent::Raise(bags.number(self.me, guid))
                        };
                        let mut run = Run {
                            bags: vec![bag],
                            ..Run::default()
                        };
                        if was != now {
                            run.notice = Some(ItemNotice::Seat {
                                old: self.entry_of(was),
                                new: self.entry_of(now),
                            });
                            run.unlocks.extend(unlock_of(now));
                        }
                        out.push((at, run));
                    }
                    None => {}
                }
            }
        }
        out.sort_by_key(|&(at, _)| at);
        out
    }

    /// The run of an item type watcher behind `notice`: the unlock its handler opens with, the
    /// `BAG_UPDATE` it raises ([`Self::item_bag`]) and the unit signals it ends in
    /// ([`Self::signals`]). The stack count's (`0x5d9400`) unlocks its entry or the item
    /// (`0x5d94af`, `0x5d94ca`) and raises (`0x5d94ff`); the entry's raises once its record is
    /// cached (`0x5d9685`), the enchantment's always (`0x5d9b1b`).
    pub(crate) fn run(&self, notice: ItemNotice, bags: &BagCache) -> Run {
        let mut run = Run {
            signals: self.signals(&notice),
            ..Run::default()
        };
        match notice {
            ItemNotice::StackCount { guid, entry } => {
                run.unlocks
                    .extend(self.entry_unlock(guid, entry, Walk::Stack));
                run.bags.extend(self.item_bag(guid, bags));
            }
            ItemNotice::Entry {
                guid,
                new_cached: true,
                ..
            }
            | ItemNotice::Enchantment { guid, .. } => run.bags.extend(self.item_bag(guid, bags)),
            _ => {}
        }
        run.notice = Some(notice);
        run
    }

    /// A first create's record set-up ([`Self::created`], [`Self::created_signal`]): the notice,
    /// unlock and `BAG_UPDATE` for our item ([`Self::set_up`]), then the owner's signal for any.
    pub(crate) fn created_run(&self, guid: u64, fields: &ObjectFields, bags: &BagCache) -> Run {
        let mut run = Run {
            notice: self.created(guid, fields),
            signals: self.created_signal(fields).into_iter().collect(),
            ..Run::default()
        };
        if let Some(ItemNotice::Record { entry, .. }) = run.notice {
            self.set_up(&mut run, guid, entry, bags);
        }
        run
    }

    /// A destroy's run ([`Self::destroyed`]), for our item.
    pub(crate) fn destroyed_run(&self, guid: u64) -> Option<Run> {
        self.destroyed(guid).map(|n| Run {
            signals: self.signals(&n),
            notice: Some(n),
            ..Run::default()
        })
    }

    /// A first create of an item: the record set up at once when cached (`0x5d8300` into
    /// `0x5d8440`), for our item while in the world.
    pub(crate) fn created(&self, guid: u64, fields: &ObjectFields) -> Option<ItemNotice> {
        let entry = fields.object_entry()?;
        (self.ours(fields) && self.in_world() && self.items.template(entry, 0, Ask::Read).is_some())
            .then_some(ItemNotice::Record { guid, entry })
    }

    /// A destroy or out-of-range release of `guid`, read before the object goes.
    pub(crate) fn destroyed(&self, guid: u64) -> Option<ItemNotice> {
        let fields = (self.lookup)(guid).filter(|f| self.ours(f))?;
        Some(ItemNotice::Destroyed {
            guid,
            entry: fields.object_entry().unwrap_or(0),
        })
    }

    /// The record set-up's own part on our item (`0x5d8530`-`0x5d85e8`): the arrival's unlock
    /// (`0x5d8590`, `0x5d85ab`) ahead of the recount, and the `BAG_UPDATE` behind the quest log's
    /// ask (`0x5d85e8`).
    fn set_up(&self, run: &mut Run, guid: u64, entry: u32, bags: &BagCache) {
        run.unlocks
            .extend(self.entry_unlock(guid, entry, Walk::Arrival));
        run.bags.extend(self.item_bag(guid, bags));
    }

    /// The unlock of a record set-up or a stack-count write: with the record cached and stacking
    /// past 1 (`[rec+0x60] > 1`, `0x5d857b`, `0x5d947e`), the walk over the entry's items when the
    /// active player is found (`0x5d857d`, `0x5d949c`), else none; otherwise `UnlockItem` on the
    /// item alone (`0x5d85ab`, `0x5d94ca`).
    fn entry_unlock(&self, guid: u64, entry: u32, walk: Walk) -> Option<Unlock> {
        let stacks = self
            .items
            .template(entry, 0, Ask::Read)
            .is_some_and(|t| t.stackable > 1);
        if !stacks {
            return unlock_of(guid);
        }
        let me = self.me.and_then(|g| (self.lookup)(g))?;
        Some(Unlock::Entry(kin(me, &self.lookup, walk, entry)))
    }

    /// `0x4f9480(guid)`: an item found (`0x4f949c`) whose owner is the active player (`0x4f94b8`)
    /// raises `BAG_UPDATE` for its `ITEM_FIELD_CONTAINED` (`0x4f94cf` → `0x4f9370`); else nothing.
    fn item_bag(&self, guid: u64, bags: &BagCache) -> Option<BagEvent> {
        let item = (self.lookup)(guid).filter(|f| self.ours(f))?;
        let contained = item.item_contained().unwrap_or(0);
        Some(BagEvent::Raise(bags.number(self.me, contained)))
    }
}

impl<'a, F> Watch<'a, F>
where
    F: Fn(u64) -> Option<&'a ObjectFields>,
{
    /// The unit signals a notice's handler ends in: `NotifyItemSlotChanged` ([`Self::slot_changed`])
    /// closes the destroy (`0x5d87c1`), the stack count's (`0x5d950e`), the entry's once its record
    /// is cached (`0x5d9694`) or answered (`0x5d96d5`), and the enchantment's (`0x5d9b2a`). The
    /// record set-up's signal is [`Self::created_signal`]'s and [`Self::set_up_run`]'s.
    pub(crate) fn signals(&self, notice: &ItemNotice) -> Vec<FieldNotice> {
        match *notice {
            ItemNotice::Destroyed { guid, .. }
            | ItemNotice::StackCount { guid, .. }
            | ItemNotice::EntryRecord { guid, .. }
            | ItemNotice::Enchantment { guid, .. }
            | ItemNotice::Entry {
                guid,
                new_cached: true,
                ..
            } => self.slot_changed(guid),
            _ => Vec::new(),
        }
    }

    /// `NotifyItemSlotChanged` (`0x4c7180`) on the item `guid`: its flat seat in the active
    /// player's array, else its bag's (`ITEM_FIELD_CONTAINED`, searched unless it is the player,
    /// `0x4c721c`). A seat below 23 signals `UNIT_INVENTORY_CHANGED` with `"player"` (`0x4c7265`,
    /// `0x4c72e0`) and ends; a bank seat, 39-68, signals `PLAYERBANKSLOTS_CHANGED` with `"player"`
    /// (`0x4c726a`-`0x4c728d`); then anything but the first signals `UNIT_INVENTORY_CHANGED` for a
    /// live item whose entry is the active player's `PLAYER_AMMO_ID` (`0x4c7298`, `0x4c72cd`).
    fn slot_changed(&self, guid: u64) -> Vec<FieldNotice> {
        let signal = FieldNotice::Player {
            id: id::UNIT_INVENTORY_CHANGED,
        };
        // No active player, nothing (`0x4c71ab`).
        let Some((me_guid, me)) = self.me.and_then(|g| Some((g, (self.lookup)(g)?))) else {
            return Vec::new();
        };
        let item = (self.lookup)(guid).filter(|f| f.is_item());
        let seat_of = |g: u64| (0..113u8).find(|&n| seat_guid(me, n) == g);
        let seat = seat_of(guid).or_else(|| {
            let bag = item?.item_contained().unwrap_or(0);
            (bag != me_guid).then(|| seat_of(bag)).flatten()
        });
        if seat.is_some_and(|n| n < 23) {
            return vec![signal];
        }
        let mut out = Vec::new();
        if seat.is_some_and(|n| (39..=68).contains(&n)) {
            out.push(FieldNotice::Player {
                id: id::PLAYERBANKSLOTS_CHANGED,
            });
        }
        let ammo = me.player_ammo_id().unwrap_or(0);
        let entry = item.map(|f| f.object_entry().unwrap_or(0));
        if entry == Some(ammo) {
            out.push(signal);
        }
        out
    }

    /// The record set-up's last act (`0x5d85ed`-`0x5d8606`), at a first create whose record is
    /// cached (`0x5d8300`): `UNIT_INVENTORY_CHANGED` for each token of the item's owner,
    /// whoever's the item, while in the world (`0x5d8504`).
    pub(crate) fn created_signal(&self, fields: &ObjectFields) -> Option<FieldNotice> {
        let entry = fields.object_entry()?;
        (self.in_world() && self.items.template(entry, 0, Ask::Read).is_some())
            .then(|| owner_signal(fields))
            .flatten()
    }

    /// The create set-up's callback `0x5d8390` on `guid`, as its record answers: it finds the item
    /// (mask 2) and finishes its set-up (`0x5d8440`) while in the world (`0x5d84ff`), one of ours
    /// as an [`ItemNotice::Record`] with its unlock and `BAG_UPDATE`, and anyone's with its owner's
    /// signal at the end (`0x5d85ed`-`0x5d8606`). An item gone since runs nothing.
    pub(crate) fn set_up_run(&self, guid: u64, entry: u32, bags: &BagCache) -> Run {
        let Some(f) = (self.lookup)(guid).filter(|f| f.is_item()) else {
            return Run::default();
        };
        if !self.in_world() {
            return Run::default();
        }
        let mut run = Run {
            notice: self.ours(f).then_some(ItemNotice::Record { guid, entry }),
            signals: owner_signal(f).into_iter().collect(),
            ..Run::default()
        };
        if run.notice.is_some() {
            self.set_up(&mut run, guid, entry, bags);
        }
        run
    }

    /// An entry change's callback `0x5d96b0` on `guid`, as its record answers, ungated: the new
    /// entry's slots recount (`0x5d96bd`), then its `BAG_UPDATE` (`0x5d96c9`) and its slot run
    /// (`0x5d96d5`).
    pub(crate) fn entry_run(&self, guid: u64, entry: u32, bags: &BagCache) -> Run {
        Run {
            notice: Some(ItemNotice::EntryRecord { guid, entry }),
            signals: self.slot_changed(guid),
            bags: self.item_bag(guid, bags).into_iter().collect(),
            ..Run::default()
        }
    }
}

/// `UnlockItem` on `guid`, which a zero guid skips (`0x49542d`).
fn unlock_of(guid: u64) -> Option<Unlock> {
    (guid != 0).then_some(Unlock::Item(guid))
}

/// `0x515e50(&owner, 0xba)` (`0x5d8606`): the event for each token naming `ITEM_FIELD_OWNER`.
fn owner_signal(fields: &ObjectFields) -> Option<FieldNotice> {
    let guid = fields.item_owner().filter(|&g| g != 0)?;
    Some(FieldNotice::Signal {
        guid,
        id: id::UNIT_INVENTORY_CHANGED,
    })
}

fn guid_of(lo: u32, hi: u32) -> u64 {
    u64::from(lo) | (u64::from(hi) << 32)
}

/// The guid in flat seat `n` of the player's array (`PLAYER_FIELD_INV_SLOT_HEAD + 2n`).
fn seat_guid(fields: &ObjectFields, seat: u8) -> u64 {
    match seat {
        0..=22 => fields.player_inv_slot(seat),
        23..=38 => fields.player_pack_slot(seat - 23),
        39..=62 => fields.player_bank_slot(seat - 39),
        63..=68 => fields.player_bank_bag_slot(seat - 63),
        69..=80 => fields.player_buyback_slot(seat - 69),
        81..=112 => fields.player_keyring_slot(seat - 81),
        _ => None,
    }
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const ME: u64 = 0x0000_0000_0000_0007;
    const OTHER: u64 = 0x0000_0000_0000_0008;
    const POTION: u64 = 0x4000_0000_0000_0101;
    const ARROWS: u64 = 0x4000_0000_0000_0102;
    const BAG: u64 = 0x4000_0000_0000_0103;
    const POTION_ID: u32 = 118;
    const ARROWS_ID: u32 = 2512;
    const BAG_ID: u32 = 4496;
    /// `ITEM_FIELD_OWNER`'s two dwords.
    const OWNER: u16 = 6;
    /// `OBJECT_FIELD_TYPE`.
    const TYPE: u16 = 2;

    fn item(entry: u32, owner: u64) -> ObjectFields {
        ObjectFields::from_pairs(&[
            (TYPE, 0x3),
            (FIELD_OBJECT_ENTRY, entry),
            (OWNER, owner as u32),
            (OWNER + 1, (owner >> 32) as u32),
            (FIELD_ITEM_STACK_COUNT, 1),
        ])
        .into_created(ObjectType::Item)
    }

    fn bag(entry: u32, contents: &[(u8, u64)]) -> ObjectFields {
        let mut pairs = vec![
            (TYPE, 0x7),
            (FIELD_OBJECT_ENTRY, entry),
            (OWNER, ME as u32),
            (OWNER + 1, 0),
            (48, 4),
        ];
        for &(j, g) in contents {
            let lo = FIELD_CONTAINER_SLOT_1 + 2 * u16::from(j);
            pairs.push((lo, g as u32));
            pairs.push((lo + 1, (g >> 32) as u32));
        }
        ObjectFields::from_pairs(&pairs).into_created(ObjectType::Container)
    }

    fn seat(seat: u8, guid: u64) -> [(u16, u32); 2] {
        let lo = FIELD_PLAYER_INV_SLOT_HEAD + 2 * u16::from(seat);
        [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)]
    }

    fn player(seats: &[(u8, u64)]) -> ObjectFields {
        let pairs: Vec<(u16, u32)> = seats.iter().flat_map(|&(s, g)| seat(s, g)).collect();
        ObjectFields::from_pairs(&pairs).into_created(ObjectType::Player)
    }

    /// A world of objects by guid and the record cache.
    struct World {
        objects: HashMap<u64, ObjectFields>,
        items: Items,
        bags: BagCache,
    }

    impl World {
        fn new() -> Self {
            let mut objects = HashMap::new();
            objects.insert(ME, player(&[]));
            World {
                objects,
                items: Items::default(),
                bags: BagCache::default(),
            }
        }

        fn watch<'a>(&'a self) -> Watch<'a, impl Fn(u64) -> Option<&'a ObjectFields> + 'a> {
            Watch {
                me: Some(ME),
                lookup: |g| self.objects.get(&g),
                items: &self.items,
            }
        }

        /// Merge `delta` into `guid`'s store and run the watchers on what moved, as a packet of
        /// this one values block does: the runs, in the notify pass's order.
        fn runs(&mut self, guid: u64, delta: &[(u16, u32)]) -> Vec<Run> {
            use crate::field_watch::{Notified, Sent};
            let delta = ObjectFields::from_pairs(delta);
            let sent: Vec<u16> = delta.raw_fields().map(|(i, _)| i).collect();
            let mut moved = Vec::new();
            let store = self.objects.get_mut(&guid).expect("a live object");
            let kind = store.created_as().expect("created");
            store.merge_diff(delta, |i, o, n| moved.push((i, o, n)));
            let fields = self.objects[&guid].clone();
            let block = Notified::new(Sent::Fields(&sent), &moved);
            let mut bags = self.bags;
            let out = self
                .watch()
                .placed_values(guid, kind, &fields, &block, &mut bags);
            self.bags = bags;
            out.into_iter().map(|(_, r)| r).collect()
        }

        /// Merge `delta` into `guid`'s store and run the watchers on what moved, as a packet of
        /// this one values block does.
        fn values(&mut self, guid: u64, delta: &[(u16, u32)]) -> Vec<ItemNotice> {
            use crate::field_watch::{Notified, Sent};
            let delta = ObjectFields::from_pairs(delta);
            let sent: Vec<u16> = delta.raw_fields().map(|(i, _)| i).collect();
            let mut moved = Vec::new();
            let store = self.objects.get_mut(&guid).expect("a live object");
            let kind = store.created_as().expect("created");
            store.merge_diff(delta, |i, o, n| moved.push((i, o, n)));
            let fields = self.objects[&guid].clone();
            let block = Notified::new(Sent::Fields(&sent), &moved);
            let mut bags = self.bags;
            let out = self.watch().values(guid, kind, &fields, &block, &mut bags);
            self.bags = bags;
            out
        }
    }

    #[test]
    fn an_item_of_ours_reports_each_watched_range_once_in_field_order() {
        let mut w = World::new();
        w.objects.insert(POTION, item(POTION_ID, ME));
        // Two enchant dwords and a stack count in one block: one notice each, entry first.
        let got = w.values(
            POTION,
            &[
                (FIELD_ITEM_ENCHANTMENT, 2564),
                (FIELD_ITEM_ENCHANTMENT + 1, 3600),
                (FIELD_ITEM_STACK_COUNT, 4),
                (FIELD_ITEM_SPELL_CHARGES + 2, 0xffff_ffff),
            ],
        );
        assert_eq!(
            got,
            [
                ItemNotice::StackCount {
                    guid: POTION,
                    entry: POTION_ID
                },
                ItemNotice::SpellCharges {
                    guid: POTION,
                    entry: POTION_ID
                },
                ItemNotice::Enchantment {
                    guid: POTION,
                    entry: POTION_ID
                },
            ]
        );
        // A resend of the same values changes nothing and runs nothing.
        assert!(w.values(POTION, &[(FIELD_ITEM_STACK_COUNT, 4)]).is_empty());
    }

    #[test]
    fn another_players_item_and_a_charter_run_no_watcher() {
        let mut w = World::new();
        w.objects.insert(POTION, item(POTION_ID, OTHER));
        assert!(w.values(POTION, &[(FIELD_ITEM_STACK_COUNT, 2)]).is_empty());
        // A charter's enchantment slot carries its petition id (`0x5d9aa8`).
        w.objects.insert(ARROWS, item(ARROWS_ID, ME));
        let mut charter = crate::items::test_template("Guild Charter");
        charter.flags = ITEM_FLAG_CHARTER;
        w.items.insert_template(ARROWS_ID, Some(charter));
        assert!(w.values(ARROWS, &[(FIELD_ITEM_ENCHANTMENT, 9)]).is_empty());
        assert_eq!(
            w.values(ARROWS, &[(FIELD_ITEM_STACK_COUNT, 5)]),
            [ItemNotice::StackCount {
                guid: ARROWS,
                entry: ARROWS_ID
            }],
            "the stack watcher has no charter test"
        );
    }

    #[test]
    fn an_entry_change_refreshes_the_old_entry_and_the_new_one_once_cached() {
        let mut w = World::new();
        w.objects.insert(POTION, item(POTION_ID, ME));
        let got = w.values(POTION, &[(FIELD_OBJECT_ENTRY, ARROWS_ID)]);
        assert_eq!(
            got,
            [ItemNotice::Entry {
                guid: POTION,
                old: POTION_ID,
                new: ARROWS_ID,
                new_cached: false
            }]
        );
        assert_eq!(
            got[0].recounts(),
            [POTION_ID],
            "the new waits for its record"
        );
        w.items
            .insert_template(POTION_ID, Some(crate::items::test_template("Potion")));
        let back = w.values(POTION, &[(FIELD_OBJECT_ENTRY, POTION_ID)]);
        assert_eq!(back[0].recounts(), [ARROWS_ID, POTION_ID]);
    }

    #[test]
    fn a_backpack_seat_reports_the_item_that_left_and_the_one_that_came() {
        let mut w = World::new();
        w.objects.insert(POTION, item(POTION_ID, ME));
        w.objects.insert(ARROWS, item(ARROWS_ID, ME));
        w.values(ME, &seat(23, POTION));
        let got = w.values(ME, &[seat(23, ARROWS), seat(81, POTION)].concat());
        assert_eq!(
            got,
            [
                ItemNotice::Seat {
                    old: Some(POTION_ID),
                    new: Some(ARROWS_ID)
                },
                ItemNotice::Seat {
                    old: None,
                    new: Some(POTION_ID)
                },
            ],
            "the backpack seat, then the keyring seat, in field order"
        );
        // A seat whose old item is no longer an object reports none for it.
        w.objects.remove(&ARROWS);
        assert_eq!(
            w.values(ME, &seat(23, 0)),
            [ItemNotice::Seat {
                old: None,
                new: None
            }]
        );
        // The equipment and bank seats have no watcher here.
        assert!(w
            .values(ME, &[seat(5, POTION), seat(40, POTION)].concat())
            .is_empty());
    }

    #[test]
    fn a_bag_seat_reports_the_new_bags_items_and_its_own_seats_once_equipped() {
        let mut w = World::new();
        w.objects.insert(POTION, item(POTION_ID, ME));
        w.objects.insert(ARROWS, item(ARROWS_ID, ME));
        w.objects
            .insert(BAG, bag(BAG_ID, &[(0, POTION), (2, ARROWS)]));
        let lo = FIELD_CONTAINER_SLOT_1;
        // Not in a bag seat yet: its seats are unwatched.
        assert!(w
            .values(
                BAG,
                &[(lo + 2, ARROWS as u32), (lo + 3, (ARROWS >> 32) as u32)]
            )
            .is_empty());
        let got = w.values(ME, &seat(19, BAG));
        assert_eq!(
            got,
            [ItemNotice::Bag {
                entries: vec![POTION_ID, ARROWS_ID, ARROWS_ID]
            }]
        );
        // Equipped: a seat inside it reports.
        let got = w.values(BAG, &[(lo, 0), (lo + 1, 0)]);
        assert_eq!(
            got,
            [ItemNotice::Seat {
                old: Some(POTION_ID),
                new: None
            }]
        );
    }

    #[test]
    fn a_visible_item_reports_its_old_and_new_entry_on_any_dword() {
        let mut w = World::new();
        let base = FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 12 * 15;
        w.values(ME, &[(base + 2, 2504)]);
        let got = w.values(ME, &[(base + 2, 2505), (base + 3, 7)]);
        assert_eq!(
            got,
            [ItemNotice::Visible {
                old: 2504,
                new: 2505
            }]
        );
        // An enchant alone still runs it, the entry unchanged.
        assert_eq!(
            w.values(ME, &[(base + 3, 8)]),
            [ItemNotice::Visible {
                old: 2505,
                new: 2505
            }]
        );
        // A ring (slot 10) has no watcher.
        let ring = FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 12 * 10;
        assert!(w.values(ME, &[(ring + 2, 1)]).is_empty());
        // Another player's visible items run nothing for us.
        w.objects.insert(OTHER, player(&[]));
        assert!(w.values(OTHER, &[(base + 2, 1)]).is_empty());
    }

    #[test]
    fn the_record_set_up_needs_the_world_and_a_cached_record() {
        let mut w = World::new();
        let potion = item(POTION_ID, ME);
        assert_eq!(w.watch().created(POTION, &potion), None, "not cached");
        w.items
            .insert_template(POTION_ID, Some(crate::items::test_template("Potion")));
        assert_eq!(
            w.watch().created(POTION, &potion),
            Some(ItemNotice::Record {
                guid: POTION,
                entry: POTION_ID
            })
        );
        assert_eq!(w.watch().created(ARROWS, &item(POTION_ID, OTHER)), None);
        // Before our player arrives, the latch is clear.
        w.objects.remove(&ME);
        assert_eq!(w.watch().created(POTION, &potion), None);
        w.objects.insert(POTION, potion);
        assert!(w
            .watch()
            .set_up_run(POTION, POTION_ID, &BagCache::default())
            .is_empty());
        w.objects.insert(ME, player(&[]));
        assert_eq!(
            w.watch()
                .set_up_run(POTION, POTION_ID, &BagCache::default())
                .notice,
            Some(ItemNotice::Record {
                guid: POTION,
                entry: POTION_ID
            }),
            "the answer's callback finds the item (`0x5d8390`)"
        );
        assert!(
            w.watch()
                .set_up_run(ARROWS, POTION_ID, &BagCache::default())
                .is_empty(),
            "an item gone since"
        );
    }

    /// `UNIT_INVENTORY_CHANGED` with `"player"`, as `0x4c72e0` signals it.
    const INVENTORY: FieldNotice = FieldNotice::Player {
        id: id::UNIT_INVENTORY_CHANGED,
    };

    /// An item of ours, `ITEM_FIELD_CONTAINED` the bag or player holding it.
    fn held_item(entry: u32, contained: u64) -> ObjectFields {
        let mut f = item(entry, ME);
        f.merge(ObjectFields::from_pairs(&[
            (8, contained as u32),
            (9, (contained >> 32) as u32),
        ]));
        f
    }

    /// `NotifyItemSlotChanged` (`0x4c7180`) finds the item's seat, else its bag's: a seat below
    /// 23 signals (`0x4c7265`); anything else signals only for the active player's ammunition
    /// (`0x4c72cd`). Each notice whose handler calls it runs it.
    #[test]
    fn the_slot_change_signals_a_worn_or_bagged_item_and_the_ammunition() {
        const HELM: u64 = 0x4000_0000_0000_0201;
        const BANKED: u64 = 0x4000_0000_0000_0202;
        const INNER: u64 = 0x4000_0000_0000_0203;
        let mut w = World::new();
        // Head (0), a bag in seat 19, the potion and the arrows in the backpack (23, 24), a
        // bank seat (39); `PLAYER_AMMO_ID` the arrows'.
        let mut me = player(&[
            (0, HELM),
            (19, BAG),
            (23, POTION),
            (24, ARROWS),
            (39, BANKED),
        ]);
        me.merge(ObjectFields::from_pairs(&[(1223, ARROWS_ID)]));
        w.objects.insert(ME, me);
        w.objects.insert(HELM, held_item(7366, ME));
        w.objects.insert(BAG, bag(BAG_ID, &[(0, INNER)]));
        w.objects.insert(INNER, held_item(POTION_ID, BAG));
        w.objects.insert(POTION, held_item(POTION_ID, ME));
        w.objects.insert(ARROWS, held_item(ARROWS_ID, ME));
        w.objects.insert(BANKED, held_item(POTION_ID, ME));
        let stack = |guid| ItemNotice::StackCount { guid, entry: 0 };
        {
            let watch = w.watch();
            assert_eq!(watch.signals(&stack(HELM)), [INVENTORY], "a worn seat");
            assert_eq!(
                watch.signals(&stack(INNER)),
                [INVENTORY],
                "the bag's seat, 19"
            );
            assert!(watch.signals(&stack(POTION)).is_empty(), "a backpack seat");
            assert_eq!(watch.signals(&stack(ARROWS)), [INVENTORY], "the ammunition");
            assert_eq!(
                watch.signals(&stack(BANKED)),
                [FieldNotice::Player {
                    id: id::PLAYERBANKSLOTS_CHANGED
                }],
                "the bank band's own event (`0x4c728d`)"
            );
            // The other handlers that call it (`0x5d87c1`, `0x5d9694`, `0x5d96d5`, `0x5d9b2a`);
            // an entry change whose record missed waits for its answer (`0x5d966e`).
            for notice in [
                ItemNotice::Destroyed {
                    guid: HELM,
                    entry: 7366,
                },
                ItemNotice::Entry {
                    guid: HELM,
                    old: 1,
                    new: 7366,
                    new_cached: true,
                },
                ItemNotice::EntryRecord {
                    guid: HELM,
                    entry: 7366,
                },
                ItemNotice::Enchantment {
                    guid: HELM,
                    entry: 7366,
                },
            ] {
                assert_eq!(watch.signals(&notice), [INVENTORY], "{notice:?}");
            }
            let missed = ItemNotice::Entry {
                guid: HELM,
                old: 1,
                new: 7366,
                new_cached: false,
            };
            assert!(watch.signals(&missed).is_empty());
            // The spell charges' handler `0x5d9540` and a seat's `0x4f8db0` never call it.
            assert!(watch
                .signals(&ItemNotice::SpellCharges {
                    guid: HELM,
                    entry: 7366
                })
                .is_empty());
        }
        // A worn seat signals with the item gone; the ammunition test needs it live (`0x4c7298`).
        w.objects.remove(&HELM);
        w.objects.remove(&ARROWS);
        assert_eq!(w.watch().signals(&stack(HELM)), [INVENTORY]);
        assert!(w.watch().signals(&stack(ARROWS)).is_empty());
        // No active player, nothing (`0x4c71ab`).
        w.objects.remove(&ME);
        assert!(w.watch().signals(&stack(INNER)).is_empty());
    }

    /// The record set-up's last act signals `UNIT_INVENTORY_CHANGED` for the tokens of the item's
    /// owner, whoever's the item, while in the world (`0x5d8504`, `0x5d85ed`-`0x5d8606`): at a
    /// create whose record is cached, and from the callback of each item an answer finishes.
    #[test]
    fn a_record_set_up_signals_its_owner_and_an_answer_its_waiters_after() {
        const HELM: u64 = 0x4000_0000_0000_0201;
        let mut w = World::new();
        let ours = held_item(POTION_ID, ME);
        let theirs = item(POTION_ID, OTHER);
        assert_eq!(w.watch().created_signal(&ours), None, "not cached");
        w.items
            .insert_template(POTION_ID, Some(crate::items::test_template("Potion")));
        let owner = |guid| FieldNotice::Signal {
            guid,
            id: id::UNIT_INVENTORY_CHANGED,
        };
        assert_eq!(w.watch().created_signal(&ours), Some(owner(ME)));
        assert_eq!(w.watch().created_signal(&theirs), Some(owner(OTHER)));
        w.objects.insert(ME, player(&[(0, HELM)]));
        w.objects.insert(ARROWS, theirs);
        w.objects.insert(POTION, ours.clone());
        assert_eq!(
            w.watch()
                .set_up_run(POTION, POTION_ID, &BagCache::default())
                .signals,
            [owner(ME)]
        );
        let run = w
            .watch()
            .set_up_run(ARROWS, POTION_ID, &BagCache::default());
        assert_eq!(run.notice, None, "another's item: its owner's signal alone");
        assert_eq!(run.signals, [owner(OTHER)]);
        w.objects.remove(&ME);
        assert_eq!(w.watch().created_signal(&ours), None, "out of the world");
        assert!(w
            .watch()
            .set_up_run(POTION, POTION_ID, &BagCache::default())
            .is_empty());
    }

    /// A trade getter's record callback (`0x4c0340`) touches neither the bar nor a trade slot.
    #[test]
    fn a_trade_record_notice_recounts_and_touches_nothing() {
        let notice = ItemNotice::TradeRecord { entry: POTION_ID };
        assert!(notice.recounts().is_empty());
        assert_eq!(notice.trade(), None);
    }

    #[test]
    fn a_destroy_reports_our_item_with_its_entry() {
        let mut w = World::new();
        w.objects.insert(POTION, item(POTION_ID, ME));
        w.objects.insert(ARROWS, item(ARROWS_ID, OTHER));
        let gone = w.watch().destroyed(POTION).unwrap();
        assert_eq!(gone.recounts(), [POTION_ID]);
        assert_eq!(gone.trade(), Some(TradeTouch::Gone(POTION)));
        assert_eq!(w.watch().destroyed(ARROWS), None);
        assert_eq!(w.watch().destroyed(BAG), None);
    }

    /// An entry change that missed the cache waits on the answer with its own callback
    /// (`0x5d96b0`): no world latch, no Record set-up for that item, the request's entry.
    #[test]
    fn an_entry_miss_runs_its_own_callback_when_the_answer_lands() {
        let mut w = World::new();
        w.objects.insert(ARROWS, item(POTION_ID, ME));
        w.objects.remove(&ME);
        assert_eq!(
            w.watch()
                .entry_run(ARROWS, POTION_ID, &BagCache::default())
                .notice,
            Some(ItemNotice::EntryRecord {
                guid: ARROWS,
                entry: POTION_ID
            }),
            "out of the world the callback still runs"
        );
        assert_eq!(
            ItemNotice::EntryRecord {
                guid: ARROWS,
                entry: POTION_ID
            }
            .recounts(),
            [POTION_ID]
        );
    }

    /// A run's unlocks and bag events alone, for the goldens below.
    fn bag_steps(runs: &[Run]) -> Vec<(Vec<Unlock>, Vec<BagEvent>)> {
        runs.iter()
            .map(|r| (r.unlocks.clone(), r.bags.clone()))
            .collect()
    }

    /// The inventory watcher `0x5ddcf0` runs ahead of the seat watchers at one seat (`0x5dd8a0`
    /// before `0x4f8cc0`): it unlocks the item now in the seat (`0x5ddd50`), none for an emptied
    /// one, and a bank seat or bank bag seat signals `PLAYERBANKSLOTS_CHANGED` (`0x5ddd69`). The
    /// keyring's seat watcher then unlocks again and signals `BAG_UPDATE(-2)` directly
    /// (`0x4f8e15`, `0x4f8e8a`), the backpack's through `0x4f9370` with 0 (`0x4f8ea6`).
    #[test]
    fn the_inventory_watcher_runs_ahead_of_the_seat_watchers() {
        let mut w = World::new();
        w.objects.insert(POTION, item(POTION_ID, ME));
        w.objects.insert(ARROWS, item(ARROWS_ID, ME));
        let runs = w.runs(
            ME,
            &[seat(23, POTION), seat(40, ARROWS), seat(81, POTION)].concat(),
        );
        assert_eq!(
            bag_steps(&runs),
            [
                (vec![Unlock::Item(POTION)], vec![BagEvent::Raise(Some(0))]),
                (vec![Unlock::Item(ARROWS)], vec![BagEvent::BankSlots]),
                (vec![Unlock::Item(POTION)], vec![]),
                (vec![Unlock::Item(POTION)], vec![BagEvent::Update(-2)]),
            ]
        );
        assert_eq!(runs[2].notice, None, "the inventory watcher has no notice");
        // Emptied: no unlock, the events still.
        let runs = w.runs(ME, &[seat(23, 0), seat(40, 0)].concat());
        assert_eq!(
            bag_steps(&runs),
            [
                (vec![], vec![BagEvent::Raise(Some(0))]),
                (vec![], vec![BagEvent::BankSlots]),
            ]
        );
        // A worn seat runs the inventory watcher alone, which signals nothing.
        let runs = w.runs(ME, &seat(5, POTION));
        assert_eq!(bag_steps(&runs), [(vec![Unlock::Item(POTION)], vec![])]);
    }

    /// The rescan `0x4f91a0` runs from the first bag seat's watcher of a block over all ten bags,
    /// bag by bag: `BAG_CLOSED` off a nonzero old guid, then `BAG_UPDATE` onto a nonzero new one
    /// (`0x4f92b5`, `0x4f9343`), so two bags swapped close and update both there and the second
    /// seat's rescan finds nothing.
    #[test]
    fn a_bag_seats_rescan_closes_and_updates_bag_by_bag() {
        const BAG2: u64 = 0x4000_0000_0000_0104;
        let mut w = World::new();
        w.objects.insert(BAG, bag(BAG_ID, &[]));
        w.objects.insert(BAG2, bag(BAG_ID, &[]));
        let rescans = |runs: &[Run]| -> Vec<Vec<BagEvent>> {
            runs.iter()
                .filter(|r| matches!(r.notice, Some(ItemNotice::Bag { .. })))
                .map(|r| r.bags.clone())
                .collect()
        };
        let runs = w.runs(ME, &[seat(19, BAG), seat(64, BAG2)].concat());
        assert_eq!(
            rescans(&runs),
            [vec![BagEvent::Update(1), BagEvent::Update(6)], vec![]]
        );
        assert_eq!(w.bags.0[0], BAG);
        assert_eq!(w.bags.0[5], BAG2);
        let runs = w.runs(ME, &[seat(19, BAG2), seat(64, BAG)].concat());
        assert_eq!(
            rescans(&runs),
            [
                vec![
                    BagEvent::Closed(1),
                    BagEvent::Update(1),
                    BagEvent::Closed(6),
                    BagEvent::Update(6)
                ],
                vec![]
            ]
        );
        let runs = w.runs(ME, &seat(19, 0));
        assert_eq!(rescans(&runs), [vec![BagEvent::Closed(1)]]);
    }

    /// A cached bag's slot watcher (`0x4f90d0`) unlocks the item now in the slot (`0x4f912c`) and
    /// raises the bag's number (`0x4f918e`); a bag the cache does not hold has no slot watchers
    /// (`0x4f932a`).
    #[test]
    fn a_cached_bags_slot_unlocks_its_arrival_and_raises_the_bags_number() {
        let mut w = World::new();
        w.objects.insert(POTION, item(POTION_ID, ME));
        w.objects.insert(BAG, bag(BAG_ID, &[]));
        let lo = FIELD_CONTAINER_SLOT_1 + 2;
        let fill = [(lo, POTION as u32), (lo + 1, (POTION >> 32) as u32)];
        assert!(w.runs(BAG, &fill).is_empty(), "not cached");
        w.objects
            .get_mut(&BAG)
            .unwrap()
            .merge(ObjectFields::from_pairs(&[(lo, 0), (lo + 1, 0)]));
        w.bags.0[2] = BAG;
        w.bags.1[2] = 4;
        let runs = w.runs(BAG, &fill);
        assert_eq!(
            bag_steps(&runs),
            [(vec![Unlock::Item(POTION)], vec![BagEvent::Raise(Some(3))])]
        );
    }

    /// A bag's slot watchers are registered over its slot count as the rescan stores it
    /// (`0x4f92fd`, `0x4f932a`): none for a bag the rescan does not find (`0x4f92ec`), and none
    /// past the count.
    #[test]
    fn a_bags_slot_watchers_cover_its_count_when_cached() {
        let mut w = World::new();
        w.objects.insert(ME, player(&[]));
        w.objects.insert(POTION, item(POTION_ID, ME));
        let fill = |j: u16| {
            let lo = FIELD_CONTAINER_SLOT_1 + 2 * j;
            [(lo, POTION as u32), (lo + 1, (POTION >> 32) as u32)]
        };
        let _ = w.runs(ME, &seat(19, BAG));
        w.objects.insert(BAG, bag(BAG_ID, &[]));
        assert!(
            w.runs(BAG, &fill(0)).is_empty(),
            "the seat came before the bag: no rescan found it"
        );
        let _ = w.runs(ME, &seat(19, 0));
        let _ = w.runs(ME, &seat(19, BAG));
        assert_eq!(w.runs(BAG, &fill(1)).len(), 1);
        assert!(w.runs(BAG, &fill(4)).is_empty(), "past its four slots");
    }

    /// The item type watchers' runs: the stack count unlocks a stacking entry's items through the
    /// walk (`0x5d94af`) and anything else alone (`0x5d94ca`), then raises the item's container
    /// (`0x5d94ff` → `0x4f9480`): the player's 0, a cached bag's number, or none past the cache.
    #[test]
    fn an_items_stack_count_unlocks_then_raises_its_container() {
        const INNER: u64 = 0x4000_0000_0000_0105;
        let mut w = World::new();
        w.objects
            .insert(ME, player(&[(23, POTION), (24, ARROWS), (19, BAG)]));
        w.objects.insert(POTION, held_item(POTION_ID, ME));
        w.objects.insert(ARROWS, held_item(ARROWS_ID, ME));
        w.objects.insert(BAG, bag(BAG_ID, &[(0, INNER)]));
        w.objects.insert(INNER, held_item(ARROWS_ID, BAG));
        w.items.insert_template(
            ARROWS_ID,
            Some(benilla_protocol::messages::ItemInfo {
                stackable: 200,
                ..crate::items::test_template("Arrow")
            }),
        );
        let steps =
            |w: &mut World, guid: u64| bag_steps(&w.runs(guid, &[(FIELD_ITEM_STACK_COUNT, 7)]));
        assert_eq!(
            steps(&mut w, POTION),
            [(vec![Unlock::Item(POTION)], vec![BagEvent::Raise(Some(0))])],
            "no record: the item alone (`0x5d947c`)"
        );
        assert_eq!(
            steps(&mut w, INNER),
            [(
                vec![Unlock::Entry(vec![ARROWS, INNER])],
                vec![BagEvent::Raise(None)]
            )],
            "the bag is not cached: the usable refresh alone"
        );
        w.bags.0[0] = BAG;
        let runs = w.runs(INNER, &[(FIELD_ITEM_STACK_COUNT, 8)]);
        assert_eq!(runs[0].bags, [BagEvent::Raise(Some(1))]);
    }

    /// A record set-up on our item opens with its arrival's unlock (`0x5d8590`, the walk's flags 0
    /// leaving out the bank) and raises its container behind the quest log's ask (`0x5d85e8`).
    #[test]
    fn a_record_set_up_unlocks_its_arrival_and_raises_its_container() {
        const BANKED: u64 = 0x4000_0000_0000_0106;
        let mut w = World::new();
        w.objects.insert(ME, player(&[(23, POTION), (39, BANKED)]));
        w.objects.insert(POTION, held_item(POTION_ID, ME));
        w.objects.insert(BANKED, held_item(POTION_ID, ME));
        w.items.insert_template(
            POTION_ID,
            Some(benilla_protocol::messages::ItemInfo {
                stackable: 20,
                ..crate::items::test_template("Potion")
            }),
        );
        let run = w
            .watch()
            .set_up_run(POTION, POTION_ID, &BagCache::default());
        assert_eq!(run.unlocks, [Unlock::Entry(vec![POTION])]);
        assert_eq!(run.bags, [BagEvent::Raise(Some(0))]);
        let created = w
            .watch()
            .created_run(BANKED, &w.objects[&BANKED], &BagCache::default());
        assert_eq!(
            created.bags,
            [BagEvent::Raise(Some(0))],
            "the bank's is the player"
        );
    }

    /// `NotifyItemSlotChanged`'s bank arm (`0x4c726a`-`0x4c728d`): an item in a bank bag, found by
    /// its bag's seat (`0x4c720b`-`0x4c725f`), and a bank bag itself signal
    /// `PLAYERBANKSLOTS_CHANGED` with `"player"`, then the ammunition test (`0x4c7295`).
    #[test]
    fn the_slot_change_signals_the_bank_slots_for_a_bank_bag_and_its_items() {
        const INNER: u64 = 0x4000_0000_0000_0107;
        let mut w = World::new();
        let mut me = player(&[(63, BAG)]);
        me.merge(ObjectFields::from_pairs(&[(1223, ARROWS_ID)]));
        w.objects.insert(ME, me);
        w.objects.insert(BAG, bag(BAG_ID, &[(0, INNER)]));
        w.objects.insert(INNER, held_item(ARROWS_ID, BAG));
        let bank = FieldNotice::Player {
            id: id::PLAYERBANKSLOTS_CHANGED,
        };
        let stack = |guid| ItemNotice::StackCount { guid, entry: 0 };
        assert_eq!(w.watch().signals(&stack(BAG)), [bank]);
        assert_eq!(
            w.watch().signals(&stack(INNER)),
            [bank, INVENTORY],
            "and the ammunition"
        );
    }

    /// `0x4f9370`'s and `0x4f9410`'s numbers: the player is 0, a cached bag its index plus 1, and
    /// a `BAG_OPEN` goes on past a match (`0x4f9445`-`0x4f9475`).
    #[test]
    fn the_bag_cache_numbers_the_player_and_its_bags() {
        let mut cache = BagCache::default();
        cache.0[1] = BAG;
        cache.0[7] = BAG;
        assert_eq!(cache.number(Some(ME), ME), Some(0));
        assert_eq!(cache.number(Some(ME), BAG), Some(2));
        assert_eq!(cache.number(Some(ME), POTION), None);
        assert_eq!(cache.opens(Some(ME), ME), [0]);
        assert_eq!(cache.opens(Some(ME), BAG), [2, 8]);
        assert!(cache.opens(Some(ME), POTION).is_empty());
        cache.seat(&player(&[(20, POTION), (68, BAG)]), &|_| None);
        assert_eq!(cache.0, [0, POTION, 0, 0, 0, 0, 0, 0, 0, BAG]);
    }

    /// The arrival's walk skips the bank and the stack-count write's reaches it (`0x622420`
    /// flags 0 and 8); both pass over a broken item and one flagged `0x10`, and walk a bag inside.
    #[test]
    fn the_entry_walks_scope_and_skip_as_the_reference_does() {
        const BROKEN: u64 = 0x4000_0000_0000_0d01;
        const FLAGGED: u64 = 0x4000_0000_0000_0d02;
        const BANKED: u64 = 0x4000_0000_0000_0d03;
        const WORN: u64 = 0x4000_0000_0000_0d04;
        const INNER: u64 = 0x4000_0000_0000_0d05;
        let mut w = World::new();
        let me = player(&[
            (23, POTION),
            (24, BROKEN),
            (25, FLAGGED),
            (39, BANKED),
            (0, WORN),
            (19, BAG),
        ]);
        w.objects.insert(ME, me.clone());
        w.objects.insert(BAG, bag(BAG_ID, &[(0, INNER)]));
        for guid in [POTION, BANKED, WORN, INNER] {
            w.objects.insert(guid, item(ARROWS_ID, ME));
        }
        let mut broken = item(ARROWS_ID, ME);
        broken.merge(ObjectFields::from_pairs(&[(47, 50), (46, 0)]));
        w.objects.insert(BROKEN, broken);
        let mut flagged = item(ARROWS_ID, ME);
        flagged.merge(ObjectFields::from_pairs(&[(21, 0x10)]));
        w.objects.insert(FLAGGED, flagged);
        let lookup = |g: u64| w.objects.get(&g);
        let mut arrival = vec![POTION, WORN, INNER];
        arrival.sort_unstable();
        assert_eq!(kin(&me, &lookup, Walk::Arrival, ARROWS_ID), arrival);
        let mut stack = vec![POTION, WORN, INNER, BANKED];
        stack.sort_unstable();
        assert_eq!(kin(&me, &lookup, Walk::Stack, ARROWS_ID), stack);
    }
}
