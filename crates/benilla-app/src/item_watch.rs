//! The item watchers: the reference's handlers that run when one of the active player's items,
//! or a seat that holds one, changes. The object packets write one [`ItemNotice`] per handler run,
//! in wire order, and the systems that care read the stream instead of diffing snapshots: the
//! action bar recounts (`0x4e5c70`) and the trade re-signals its slot (`0x4bfb10`, `0x4bfb60`).
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

use crate::items::Items;
use crate::query_cache::Ask;

/// One run of a reference handler on the active player's items, in the order the packets ran
/// them. Entries are `OBJECT_FIELD_ENTRY` values; an item the handler could not find has none.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
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

/// The equipment slots with a visible-item watcher: mask `0x7c3fd` of `0x5e2510`, slots 0 and 2-9
/// and 14-18; the neck, rings and trinkets have none.
const VISIBLE_WATCHED: u32 = 0x7c3fd;

/// The bags whose seats `0x4f90d0` watches: the cache `0x4f91a0` keeps of seats 19-22 and 63-68.
fn our_bags(me: &ObjectFields) -> impl Iterator<Item = u64> + '_ {
    (19..23u8)
        .filter_map(|i| me.player_inv_slot(i))
        .chain((0..6u8).filter_map(|i| me.player_bank_bag_slot(i)))
        .filter(|&g| g != 0)
}

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
    pub(crate) fn values(
        &self,
        guid: u64,
        kind: ObjectType,
        fields: &ObjectFields,
        block: &crate::field_watch::Notified,
    ) -> Vec<ItemNotice> {
        if block.is_quiet() {
            return Vec::new();
        }
        let old_of = |index: u16, now: u32| block.edge(index).map_or(now, |(old, _)| old);
        let moved = |range: std::ops::Range<u16>| block.runs(range);
        // `(first index of the watched range, notice)`, sorted at the end.
        let mut out: Vec<(u16, ItemNotice)> = Vec::new();
        match kind {
            ObjectType::Item | ObjectType::Container if self.ours(fields) => {
                let entry = fields.object_entry().unwrap_or(0);
                if moved(FIELD_OBJECT_ENTRY..FIELD_OBJECT_ENTRY + 1) {
                    let old = old_of(FIELD_OBJECT_ENTRY, entry);
                    out.push((
                        FIELD_OBJECT_ENTRY,
                        ItemNotice::Entry {
                            guid,
                            old,
                            new: entry,
                            new_cached: self.items.template(entry, 0, Ask::Read).is_some(),
                        },
                    ));
                }
                if moved(FIELD_ITEM_STACK_COUNT..FIELD_ITEM_STACK_COUNT + 1) {
                    out.push((
                        FIELD_ITEM_STACK_COUNT,
                        ItemNotice::StackCount { guid, entry },
                    ));
                }
                if moved(FIELD_ITEM_SPELL_CHARGES..FIELD_ITEM_SPELL_CHARGES + 5) {
                    out.push((
                        FIELD_ITEM_SPELL_CHARGES,
                        ItemNotice::SpellCharges { guid, entry },
                    ));
                }
                // `0x5d9a90`: a record missing reads flags 0 and passes; the owner must be the
                // active player, found live (`0x5d9acd`, `0x5d9ae8`).
                let charter = self
                    .items
                    .template(entry, 0, Ask::Read)
                    .is_some_and(|t| t.flags & ITEM_FLAG_CHARTER != 0);
                if moved(FIELD_ITEM_ENCHANTMENT..FIELD_ITEM_ENCHANTMENT + 21)
                    && !charter
                    && self.in_world()
                {
                    out.push((
                        FIELD_ITEM_ENCHANTMENT,
                        ItemNotice::Enchantment { guid, entry },
                    ));
                }
            }
            _ => {}
        }
        // A bag's own seats, watched while it sits in one of our bag seats (`0x4f932a`).
        if kind == ObjectType::Container {
            let bagged = self
                .me
                .and_then(|me| (self.lookup)(me))
                .is_some_and(|me| our_bags(me).any(|g| g == guid));
            if bagged {
                let count = fields.container_num_slots().unwrap_or(0);
                for j in 0..36u8 {
                    let lo = FIELD_CONTAINER_SLOT_1 + 2 * u16::from(j);
                    if !moved(lo..lo + 2) {
                        continue;
                    }
                    // A seat past the bag's count reads empty (`0x4f9106`).
                    let now = if u32::from(j) < count {
                        fields.container_slot(j).unwrap_or(0)
                    } else {
                        0
                    };
                    let was = guid_of(old_of(lo, now as u32), old_of(lo + 1, (now >> 32) as u32));
                    if was != now {
                        out.push((
                            lo,
                            ItemNotice::Seat {
                                old: self.entry_of(was),
                                new: self.entry_of(now),
                            },
                        ));
                    }
                }
            }
        }
        if kind == ObjectType::Player && self.me == Some(guid) {
            for i in 0..19u8 {
                if VISIBLE_WATCHED & (1 << i) == 0 {
                    continue;
                }
                let base = FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 12 * u16::from(i);
                if !moved(base..base + 12) {
                    continue;
                }
                let new = fields.player_visible_item_entry(i).unwrap_or(0);
                out.push((
                    base,
                    ItemNotice::Visible {
                        old: old_of(base + 2, new),
                        new,
                    },
                ));
            }
            for seat in 0..113u8 {
                let Some(watcher) = seat_watcher(seat) else {
                    continue;
                };
                let lo = FIELD_PLAYER_INV_SLOT_HEAD + 2 * u16::from(seat);
                if !moved(lo..lo + 2) {
                    continue;
                }
                let now = seat_guid(fields, seat);
                match watcher {
                    // `0x4f8ec0`: no equality test; a container's items, each found live.
                    SeatWatcher::Bag => {
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
                        out.push((lo, ItemNotice::Bag { entries }));
                    }
                    SeatWatcher::Item => {
                        let was =
                            guid_of(old_of(lo, now as u32), old_of(lo + 1, (now >> 32) as u32));
                        if was != now {
                            out.push((
                                lo,
                                ItemNotice::Seat {
                                    old: self.entry_of(was),
                                    new: self.entry_of(now),
                                },
                            ));
                        }
                    }
                }
            }
        }
        out.sort_by_key(|&(at, _)| at);
        out.into_iter().map(|(_, n)| n).collect()
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

    /// A record answer for `entry` that the cache did not hold. Every item of ours with that
    /// entry still waiting on its create's request finishes its set-up (`0x5d8390` into
    /// `0x5d8440`), in guid order, while in the world; then each [`ItemNotice::Entry`] miss
    /// waiting on it (`entry_waiters`, in request order) runs its callback `0x5d96b0`, ungated.
    /// `held` lists the live objects.
    pub(crate) fn record_landed<'b>(
        &self,
        entry: u32,
        held: impl IntoIterator<Item = (u64, &'b ObjectFields)>,
        entry_waiters: &[u64],
    ) -> Vec<ItemNotice> {
        let mut guids: Vec<u64> = if self.in_world() {
            held.into_iter()
                .filter(|(g, f)| {
                    self.ours(f) && f.object_entry() == Some(entry) && !entry_waiters.contains(g)
                })
                .map(|(g, _)| g)
                .collect()
        } else {
            Vec::new()
        };
        guids.sort_unstable();
        guids
            .into_iter()
            .map(|guid| ItemNotice::Record { guid, entry })
            .chain(
                entry_waiters
                    .iter()
                    .map(|&guid| ItemNotice::EntryRecord { guid, entry }),
            )
            .collect()
    }
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
    }

    impl World {
        fn new() -> Self {
            let mut objects = HashMap::new();
            objects.insert(ME, player(&[]));
            World {
                objects,
                items: Items::default(),
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
            self.watch().values(guid, kind, &fields, &block)
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
        assert!(w
            .watch()
            .record_landed(POTION_ID, [(POTION, &potion)], &[])
            .is_empty());
        w.objects.insert(ME, player(&[]));
        let other = item(POTION_ID, ME);
        assert_eq!(
            w.watch()
                .record_landed(POTION_ID, [(ARROWS, &other), (POTION, &potion)], &[]),
            [
                ItemNotice::Record {
                    guid: POTION,
                    entry: POTION_ID
                },
                ItemNotice::Record {
                    guid: ARROWS,
                    entry: POTION_ID
                }
            ],
            "in guid order"
        );
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
        let potion = item(POTION_ID, ME);
        let changed = item(POTION_ID, ME);
        let both = || [(POTION, &potion), (ARROWS, &changed)];
        assert_eq!(
            w.watch().record_landed(POTION_ID, both(), &[ARROWS]),
            [
                ItemNotice::Record {
                    guid: POTION,
                    entry: POTION_ID
                },
                ItemNotice::EntryRecord {
                    guid: ARROWS,
                    entry: POTION_ID
                }
            ]
        );
        w.objects.remove(&ME);
        assert_eq!(
            w.watch().record_landed(POTION_ID, both(), &[ARROWS]),
            [ItemNotice::EntryRecord {
                guid: ARROWS,
                entry: POTION_ID
            }],
            "out of the world only the callback runs"
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
}
