//! The player's reputation store: the client's one array of 64 slots (`0xb73290`, 16 bytes each),
//! whose flag byte (`+4`) and wire standing (`+0xc`) every reader shares, and the forced-reaction
//! table beside it.
//!
//! The wire writes the slots, the reputation verbs write a slot's flag byte inside their call
//! (`0x4d5fc0` from `0x4d6074` and `0x4d612c`), and the reaction reads the byte live (`0x60640c`
//! through `0x4d61b0`). Every write goes through `&mut`, the wire's handlers' and the call host's,
//! so a write marks the resource changed in the frame it happens. The slots sit behind one shared
//! handle for one reader outside the world: the Lua API's relation functions, whose snapshot the
//! VM holds between pushes, hold a clone, which is the same slots, so a verb's write is in their
//! answer on the call's next line.
//!
//! Each message's writes are logged with their stamp, and the reputation lane rewinds the store to
//! the span's start and replays them packet by packet ([`log`]), so a listener's getters read
//! the standings its own packet left.

use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use std::sync::Arc;

use benilla_protocol::messages::FACTION_LIST_LEN;
use benilla_ui::script::ReputationListOp;
use bevy::prelude::*;

use crate::lanes::{Stamp, Until};
use log::{Next, ReputationLog, Saved, Write};

mod log;

/// One slot's flag byte and wire standing.
#[derive(Default)]
struct Slot {
    flags: AtomicU8,
    standing: AtomicI32,
}

/// The 64 slots.
struct Slots {
    cells: [Slot; FACTION_LIST_LEN],
}

impl Slots {
    /// Slot `slot`'s `(flags, standing)`, `(0, 0)` past the 64.
    fn read(&self, slot: usize) -> (u8, i32) {
        self.cells.get(slot).map_or((0, 0), |c| {
            (
                c.flags.load(Ordering::Relaxed),
                c.standing.load(Ordering::Relaxed),
            )
        })
    }
}

impl Default for Slots {
    fn default() -> Self {
        Self {
            cells: std::array::from_fn(|_| Slot::default()),
        }
    }
}

/// Our reputation state: the `(flags, standing)` per slot and the forced-reaction table.
///
/// A slot is a `Faction.dbc` `reputationIndex`, 0 to 63; one past them reads `(0, 0)`. The
/// standing excludes the DBC race/class base, which consumers add before ranking. Filled by
/// `SMSG_INITIALIZE_FACTIONS`, kept current by `SMSG_SET_FACTION_STANDING` (which also reveals and
/// sets the at-war bit by rank), `SMSG_SET_FACTION_VISIBLE` and `SMSG_SET_FACTION_ATWAR`, and by
/// the pane's verbs. Flag bit `0x08` marks a pane header.
///
/// `forced` is `SMSG_SET_FORCED_REACTIONS` (`0x4d59a0`): `(Faction.dbc id, reaction rank)` per
/// faction a `SPELL_AURA_FORCE_REACTION` aura forces, which answers with that rank whatever its
/// templates or our standing say. Both halves live on one resource so a change to either wakes the
/// unit feed and the questgiver-marker re-query that read reactions.
///
/// A clone shares the slots and the forced table and carries no log: the shared handle is there for
/// the Lua API's relation snapshot alone, which reads them live between pushes.
#[derive(Resource, Default, Clone)]
pub(crate) struct Reputations {
    slots: Arc<Slots>,
    /// The forced table, shared as the slots are: the relation snapshot's reads see the replay.
    forced: Arc<std::sync::RwLock<Vec<(u32, u32)>>>,
    /// The span's writes and the pane's list operations each message made, for the lane to replay
    /// ([`log`]).
    log: ReputationLog,
}

impl Reputations {
    /// A store holding only `slots`, from slot 0, with nothing forced.
    #[cfg(test)]
    pub(crate) fn from_slots(slots: Vec<(u8, i32)>) -> Self {
        let mut store = Self::default();
        store.set_slots(&slots);
        store
    }

    /// One empty store for the life of the process, for a test's `'static` relation inputs.
    #[cfg(test)]
    pub(crate) fn empty() -> &'static Self {
        static EMPTY: std::sync::LazyLock<Reputations> = std::sync::LazyLock::new(Default::default);
        &EMPTY
    }

    /// Slot `slot`'s flag byte and wire standing, `(0, 0)` past the 64.
    pub(crate) fn slot(&self, slot: usize) -> (u8, i32) {
        self.slots.read(slot)
    }

    /// Slot `slot`'s flag byte (`[slot * 16 + 0xb73294]`), 0 past the 64.
    pub(crate) fn flags(&self, slot: usize) -> u8 {
        self.slot(slot).0
    }

    /// Slot `slot`'s wire standing (`[slot * 16 + 0xb7329c]`), 0 past the 64.
    pub(crate) fn standing(&self, slot: usize) -> i32 {
        self.slot(slot).1
    }

    /// Every slot's `(flags, standing)`, slot 0 first.
    #[cfg(test)]
    pub(crate) fn slots(&self) -> impl Iterator<Item = (u8, i32)> + '_ {
        (0..FACTION_LIST_LEN).map(|i| self.slot(i))
    }

    /// `SMSG_INITIALIZE_FACTIONS`: the list whole, from slot 0; a slot it does not reach reads
    /// `(0, 0)`, and one past the 64 is dropped.
    pub(crate) fn set_slots(&mut self, slots: &[(u8, i32)]) {
        self.logged(Write::Slots(slots.to_vec()));
        self.put_slots(slots);
    }

    /// A wire message's store of slot `slot`'s wire standing (`0x4d6330`).
    pub(crate) fn set_standing(&mut self, slot: usize, standing: i32) {
        self.logged(Write::Standing(slot, standing));
        self.put_standing(slot, standing);
    }

    /// A wire message's store of slot `slot`'s flag byte (`0x4d5fc0`).
    pub(crate) fn set_flags(&mut self, slot: usize, flags: u8) {
        let held = self.flags(slot);
        self.logged(Write::Flags {
            slot,
            set: flags & !held,
            clear: held & !flags,
        });
        self.put_flags(slot, flags);
    }

    /// `SMSG_SET_FORCED_REACTIONS`: the table replaces the last whole (`0x4d59a0`).
    pub(crate) fn set_forced(&mut self, table: Vec<(u32, u32)>) {
        self.logged(Write::Forced(table.clone()));
        self.put_forced(table);
    }

    /// The teardown (`0x4d5360`): the forced table's count zeroed (`0x4d5419`) and all 64 slots
    /// with it (`0x4d541f`-`0x4d542b`).
    pub(crate) fn tear_down(&mut self) {
        self.logged(Write::TearDown);
        self.put_forced(Vec::new());
        self.put_slots(&[]);
    }

    /// A message's edit of the pane's list, applied by the lane at the message's place.
    pub(crate) fn list_op(&mut self, op: ReputationListOp) {
        self.log.listed(op);
    }

    fn put_forced(&self, table: Vec<(u32, u32)>) {
        *self.forced.write().unwrap_or_else(|e| e.into_inner()) = table;
    }

    /// The rank the server forces for `faction`, if it does (`0x4d6490`).
    pub(crate) fn forced_rank(&self, faction: u32) -> Option<u32> {
        let table = self.forced.read().unwrap_or_else(|e| e.into_inner());
        table
            .iter()
            .find(|&&(id, _)| id == faction)
            .map(|&(_, r)| r)
    }

    /// The forced table, copied.
    pub(crate) fn forced(&self) -> Vec<(u32, u32)> {
        self.forced
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn put_slots(&self, slots: &[(u8, i32)]) {
        for (i, cell) in self.slots.cells.iter().enumerate() {
            let (flags, standing) = slots.get(i).copied().unwrap_or((0, 0));
            cell.flags.store(flags, Ordering::Relaxed);
            cell.standing.store(standing, Ordering::Relaxed);
        }
    }

    fn put_standing(&self, slot: usize, standing: i32) {
        if let Some(cell) = self.slots.cells.get(slot) {
            cell.standing.store(standing, Ordering::Relaxed);
        }
    }

    fn put_flags(&self, slot: usize, flags: u8) {
        if let Some(cell) = self.slots.cells.get(slot) {
            cell.flags.store(flags, Ordering::Relaxed);
        }
    }

    /// Log `write`, made next, with the tables before it.
    fn logged(&mut self, write: Write) {
        let Self {
            slots, forced, log, ..
        } = self;
        log.wrote(
            || Saved {
                slots: (0..FACTION_LIST_LEN).map(|i| slots.read(i)).collect(),
                forced: forced.read().unwrap_or_else(|e| e.into_inner()).clone(),
            },
            write,
        );
    }

    /// The lane's head ([`log`]).
    pub(crate) fn lane_head(&self) -> Option<Stamp> {
        self.log.head()
    }

    /// Rewind the store to the log's start, once per drain: whether it did so now.
    pub(crate) fn rewind(&mut self) -> bool {
        let Some(start) = self.log.rewind() else {
            return false;
        };
        if let Some(start) = start {
            self.put_slots(&start.slots);
            self.put_forced(start.forced);
        }
        true
    }

    /// Replay the writes `until` admits, up to the next list operation it admits, which it hands
    /// back with its stamp.
    pub(crate) fn replay(&mut self, until: Until) -> Option<(Stamp, ReputationListOp)> {
        self.rewind();
        while let Some((at, next)) = self.log.next(until) {
            match next {
                Next::Write(write) => self.apply(write),
                Next::Op(op) => return Some((at, op)),
            }
        }
        None
    }

    /// The drain's end, however it ended: the store as the span left it.
    pub(crate) fn end_replay(&mut self) {
        for write in self.log.finish() {
            self.apply(write);
        }
    }

    /// The log dropped unreplayed, while no in-game interface is up to hear it.
    pub(crate) fn drop_log(&mut self) {
        self.log.drop_all();
    }

    /// The operations logged and not yet applied, taken, for the tests.
    #[cfg(test)]
    pub(crate) fn take_ops(&mut self) -> Vec<ReputationListOp> {
        self.log.take_ops()
    }

    fn apply(&mut self, write: Write) {
        match write {
            Write::Slots(slots) => self.put_slots(&slots),
            Write::Standing(slot, standing) => self.put_standing(slot, standing),
            Write::Flags { slot, set, clear } => {
                let held = self.flags(slot);
                self.put_flags(slot, held & !clear | set);
            }
            Write::Forced(table) => self.put_forced(table),
            Write::TearDown => {
                self.put_forced(Vec::new());
                self.put_slots(&[]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lane's replay: the store rewound to the span's start, each write applied at its place
    /// ahead of the operation stamped after it, and the span's last state once it ends.
    #[test]
    fn the_replay_walks_the_spans_writes_between_its_operations() {
        let mut store = Reputations::default();
        store.set_standing(3, 100);
        store.list_op(ReputationListOp::Rebuild);
        store.set_flags(3, 1);
        store.set_standing(3, 200);
        store.list_op(ReputationListOp::Edit {
            listed: vec![3],
            sort: true,
        });
        assert_eq!((store.standing(3), store.flags(3)), (200, 1));
        let until = Until::all();
        assert!(store.rewind());
        assert!(!store.rewind(), "once per drain");
        assert_eq!(
            (store.standing(3), store.flags(3)),
            (0, 0),
            "the span's start"
        );
        assert_eq!(
            store.replay(until).map(|(_, op)| op),
            Some(ReputationListOp::Rebuild)
        );
        assert_eq!(
            (store.standing(3), store.flags(3)),
            (100, 0),
            "the first message's"
        );
        assert_eq!(
            store.replay(until).map(|(_, op)| op),
            Some(ReputationListOp::Edit {
                listed: vec![3],
                sort: true
            })
        );
        assert_eq!((store.standing(3), store.flags(3)), (200, 1));
        assert!(store.replay(until).is_none());
        store.end_replay();
        assert!(store.lane_head().is_none());
    }

    /// A drain that stops early leaves the store as the span wrote it, and its operations for the
    /// next drain; a verb's write during the replay lands in place and is not logged.
    #[test]
    fn a_drain_that_stops_early_leaves_the_final_store_and_a_verbs_write_stands() {
        let mut store = Reputations::default();
        store.set_standing(3, 100);
        store.list_op(ReputationListOp::Rebuild);
        store.set_standing(3, 200);
        store.list_op(ReputationListOp::Edit {
            listed: vec![3],
            sort: true,
        });
        assert!(store.rewind());
        assert!(store.replay(Until::all()).is_some());
        // The first listener's `FactionToggleAtWar`.
        store.set_flags(4, 2);
        store.end_replay();
        assert_eq!(
            store.standing(3),
            200,
            "the writes it did not replay are made"
        );
        assert_eq!(store.flags(4), 2, "and the verb's stands");
        assert_eq!(
            store.take_ops(),
            [ReputationListOp::Edit {
                listed: vec![3],
                sort: true
            }]
        );
    }

    /// The forced table and the teardown are replayed with the slots.
    #[test]
    fn the_forced_table_and_the_teardown_replay_with_the_slots() {
        let mut store = Reputations::default();
        store.set_forced(vec![(72, 0)]);
        store.set_standing(1, 5);
        store.tear_down();
        store.set_forced(vec![(47, 7)]);
        assert!(store.rewind());
        assert!(store.forced().is_empty() && store.standing(1) == 0);
        store.list_op(ReputationListOp::Rebuild);
        // Replay stops at the operation only after the writes ahead of it: all four are stamped
        // before it, so they are applied by then.
        assert!(store.replay(Until::all()).is_some());
        assert_eq!((store.forced(), store.standing(1)), (vec![(47, 7)], 0));
        store.end_replay();
    }

    /// The relation snapshot's clone shares the forced table with the slots, so the replay reaches
    /// its reads (`0x4d6490`).
    #[test]
    fn a_clone_reads_the_forced_table_the_replay_stands_at() {
        let mut store = Reputations::default();
        store.set_forced(vec![(72, 0)]);
        store.set_forced(vec![(72, 7)]);
        let clone = store.clone();
        assert_eq!(clone.forced_rank(72), Some(7));
        assert!(store.rewind());
        assert_eq!(clone.forced_rank(72), None, "the span's start");
        store.replay(Until::all());
        assert_eq!(clone.forced_rank(72), Some(7));
        store.end_replay();
    }
}
