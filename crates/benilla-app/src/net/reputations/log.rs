//! The reputation store as each packet left it. In the reference each reputation message's handler
//! writes the slots, edits the pane's list and signals `UPDATE_FACTION` inside itself (`0x4d56f6`,
//! `0x4d5699`, `0x4d5984`), and the getters read the slots at their calls (`0x4d65ac`, `0x4d65ea`), so a
//! listener reads its own packet's standings. Here the lanes fire once a packet's handlers are
//! done, over the span of packets since they last fired, so each write is logged with its stamp,
//! as is each list operation the message made ([`ReputationListOp`]); the lane (`ui_reputation`)
//! rewinds the store to the span's start and replays the log in stamp order among the other lanes,
//! applying each list operation at its place. A write made during the drain, a verb's flag write,
//! lands at its own place and is not logged.

use std::collections::VecDeque;

use benilla_ui::script::ReputationListOp;

use crate::lanes::{Stamp, Until};

/// The store's two tables, for the rewind.
pub(super) struct Saved {
    pub(super) slots: Vec<(u8, i32)>,
    pub(super) forced: Vec<(u32, u32)>,
}

/// One write to the store, replayable.
#[derive(Clone, Debug)]
pub(super) enum Write {
    /// `SMSG_INITIALIZE_FACTIONS`: the list whole.
    Slots(Vec<(u8, i32)>),
    /// A slot's wire standing (`0x4d6330`).
    Standing(usize, i32),
    /// The bits a store of a slot's flag byte (`0x4d5fc0`) moved, applied to the byte as the
    /// replay finds it: a verb's write in a listener stays through a later message's.
    Flags { slot: usize, set: u8, clear: u8 },
    /// `SMSG_SET_FORCED_REACTIONS`: the table whole (`0x4d59a0`).
    Forced(Vec<(u32, u32)>),
    /// The teardown (`0x4d5360`): the forced table and every slot zeroed.
    TearDown,
}

/// One logged item.
enum Entry {
    Write(Write),
    Op(ReputationListOp),
}

/// The store's log of the span's writes and list operations.
#[derive(Default)]
pub(super) struct ReputationLog {
    /// The tables before the log's first write.
    start: Option<Saved>,
    /// The writes and operations not yet replayed, in stamp order.
    entries: VecDeque<(Stamp, Entry)>,
    /// The drain has rewound the store and replays the log: a write lands in place, unlogged.
    replaying: bool,
}

/// A clone is a store's copy for a reader outside the drain: it carries no log.
impl Clone for ReputationLog {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl ReputationLog {
    /// Insert `entry` at `at` behind everything stamped before it: one pushed while an item fires
    /// is stamped under that item and goes ahead of the later ones.
    fn push(&mut self, at: Stamp, entry: Entry) {
        let i = self.entries.partition_point(|(s, _)| *s < at);
        self.entries.insert(i, (at, entry));
    }

    /// Log `write` unless the drain is replaying; `saved` reads the tables before it, taken once.
    pub(super) fn wrote(&mut self, saved: impl FnOnce() -> Saved, write: Write) {
        if self.replaying {
            return;
        }
        if self.start.is_none() {
            self.start = Some(saved());
        }
        self.push(crate::lanes::stamp(), Entry::Write(write));
    }

    /// Log `op`, a message's edit of the pane's list, at its place.
    pub(super) fn listed(&mut self, op: ReputationListOp) {
        self.push(crate::lanes::stamp(), Entry::Op(op));
    }

    /// The lane's head: the rewind, at the origin, while the log holds what the drain has not
    /// begun to replay; else the next entry.
    pub(super) fn head(&self) -> Option<Stamp> {
        if self.replaying {
            self.entries.front().map(|(s, _)| *s)
        } else {
            (!self.entries.is_empty()).then(Stamp::origin)
        }
    }

    /// Begin the drain's replay, once: whether it began now, with the tables to rewind to.
    pub(super) fn rewind(&mut self) -> Option<Option<Saved>> {
        if self.replaying {
            return None;
        }
        self.replaying = true;
        Some(self.start.take())
    }

    /// The next entry `until` admits: a write, which the caller applies, or an operation, which
    /// it hands back with its stamp.
    pub(super) fn next(&mut self, until: Until) -> Option<(Stamp, Next)> {
        if !self.entries.front().is_some_and(|(s, _)| until.admits(*s)) {
            return None;
        }
        let (at, entry) = self.entries.pop_front()?;
        Some((
            at,
            match entry {
                Entry::Write(w) => Next::Write(w),
                Entry::Op(op) => Next::Op(op),
            },
        ))
    }

    /// The drain's end, however it ended: the writes it did not replay, for the caller to apply,
    /// so the store stands as the span left it; its operations wait for the next drain, as every
    /// lane's unfired items do, and the next write opens a new log.
    pub(super) fn finish(&mut self) -> Vec<Write> {
        if !self.replaying {
            return Vec::new();
        }
        let mut unplayed = Vec::new();
        self.entries.retain(|(_, entry)| match entry {
            Entry::Write(w) => {
                unplayed.push(w.clone());
                false
            }
            Entry::Op(_) => true,
        });
        self.replaying = false;
        self.start = None;
        unplayed
    }

    /// Everything logged, dropped unreplayed: no in-game interface is up to hear it.
    pub(super) fn drop_all(&mut self) {
        self.entries.clear();
        self.start = None;
        self.replaying = false;
    }

    /// The operations logged and not yet applied, taken in order.
    #[cfg(test)]
    pub(super) fn take_ops(&mut self) -> Vec<ReputationListOp> {
        let mut ops = Vec::new();
        self.entries.retain(|(_, e)| match e {
            Entry::Op(op) => {
                ops.push(op.clone());
                false
            }
            Entry::Write(_) => true,
        });
        ops
    }
}

/// What [`ReputationLog::next`] hands back.
pub(super) enum Next {
    Write(Write),
    Op(ReputationListOp),
}
