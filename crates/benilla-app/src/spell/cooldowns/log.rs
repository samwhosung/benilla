//! Each list as each packet left it, the player's and the pet's. In the reference each cooldown
//! packet's handler writes its list and signals inside itself (`0x6e9460` adds its records, then
//! `0x4b31b0` at `0x6e95b0`, and for the pet's list `0x4bce90` at `0x6e95b9`), and the getters
//! query the list inside their calls (`0x6e2ea0` → `0x6e13e0`, the pet's through `0x4be03c`), so
//! a listener reads its own packet's records. Here the lanes fire once a packet's handlers are
//! done, over the span of packets since they last fired, and a handler's signals among its
//! writes, so each write is logged with its stamp, as are the flushes' signals ([`Signal`]); each
//! list's lane (`ui_action::cooldown_lane`) rewinds it to the span's start and replays its log in
//! stamp order among the other lanes, firing each signal at its place. A write made during the
//! drain, a listener's cast arming its global cooldown, lands at its own place and is not logged;
//! its signal is, stamped under the item firing, and fires straight after it.

use std::collections::VecDeque;
use std::time::Instant;

use super::Record;
use crate::lanes::{Stamp, Until};

/// One write to a list's records, replayable: each carries the time its handler read.
#[derive(Clone, Debug)]
pub(super) enum Write {
    /// `AddCooldown 0x6e12c0` past its early-out: the record appended, nothing swept.
    Add(Record),
    /// The expiry sweep `0x6e1910` at the time ([`expired`]).
    Sweep(Instant),
    /// `ClearGlobalCooldown 0x6e1630` on a match: every record of the spell has its GCD category
    /// and time zeroed, then the sweep at the time.
    ClearGcd(u32, Instant),
    /// `0x6e1790` with no `remove`: every held record of the spell starts at the time.
    Start(u32, Instant),
    /// `0x6e1790` with `remove`: every record of the spell goes.
    Remove(u32),
    /// `0x6e1880`: every record goes.
    Clear,
}

/// Whether the sweep `0x6e1910` frees a record at `now`: unheld (`0x6e1946`), with its recovery and
/// its category recovery over or of no time (`0x6e195d`, `0x6e1973`). It never reads the global
/// cooldown, so a record with only a global cooldown goes at the first sweep.
pub(super) fn expired(r: &Record, now: Instant) -> bool {
    let over = |t: &super::Timer| t.duration.is_zero() || t.remaining(now).is_zero();
    !r.on_hold && over(&r.recovery) && over(&r.category_recovery)
}

impl Write {
    /// Make the write on `records`.
    pub(super) fn apply(&self, records: &mut Vec<Record>) {
        match self {
            Self::Add(record) => records.push(record.clone()),
            Self::Sweep(now) => records.retain(|r| !expired(r, *now)),
            Self::ClearGcd(spell, now) => {
                for r in records.iter_mut().filter(|r| r.spell_id == *spell) {
                    r.gcd_category = 0;
                    r.gcd = std::time::Duration::ZERO;
                }
                records.retain(|r| !expired(r, *now));
            }
            Self::Start(spell, now) => {
                for r in records
                    .iter_mut()
                    .filter(|r| r.spell_id == *spell && r.on_hold)
                {
                    r.recovery.start = *now;
                    r.category_recovery.start = *now;
                    r.on_hold = false;
                }
            }
            Self::Remove(spell) => records.retain(|r| r.spell_id != *spell),
            Self::Clear => records.clear(),
        }
    }
}

/// What a list's flush site signals, each one routine of the reference's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    /// `0x4b31b0`: `ACTIONBAR_UPDATE_COOLDOWN`, then `SPELL_UPDATE_COOLDOWN`.
    Pair,
    /// `0x4b31c0`, the usable refresh: `ACTIONBAR_UPDATE_USABLE` on a change, each reactive
    /// spell's `SPELL_ACTIVE`, then `SPELL_UPDATE_USABLE`.
    Usable,
    /// `0x4f93d0`: `BAG_UPDATE_COOLDOWN`.
    Bags,
    /// `0x4bce90`: `PET_BAR_UPDATE_COOLDOWN`, the pet's list's tail in place of the bags.
    Pet,
}

/// One logged item.
#[derive(Debug)]
enum Entry {
    Write(Write),
    Signal(Signal),
}

/// A list's log of the span's writes and signals ([`super::Cooldowns`]), the player's and the
/// pet's alike.
#[derive(Debug, Default)]
pub(crate) struct CooldownLog {
    /// The records before the log's first write.
    start: Option<Vec<Record>>,
    /// The writes and signals not yet replayed, in stamp order.
    entries: VecDeque<(Stamp, Entry)>,
    /// The drain has rewound the list and replays the log: a write lands in place, unlogged.
    replaying: bool,
}

impl CooldownLog {
    /// Insert `entry` at `at` behind everything stamped before it: one pushed while an item fires
    /// is stamped under that item and goes ahead of the later ones.
    fn push(&mut self, at: Stamp, entry: Entry) {
        let i = self.entries.partition_point(|(s, _)| *s < at);
        self.entries.insert(i, (at, entry));
    }

    /// Log `write`, made on `records`, unless the drain is replaying.
    pub(super) fn wrote(&mut self, records: &[Record], write: &Write) {
        if self.replaying {
            return;
        }
        if self.start.is_none() {
            self.start = Some(records.to_vec());
        }
        self.push(crate::lanes::stamp(), Entry::Write(write.clone()));
    }

    /// Log `signal`, the replay's included: a listener's flush fires after its cause.
    pub(super) fn signalled(&mut self, signal: Signal) {
        self.push(crate::lanes::stamp(), Entry::Signal(signal));
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

    /// Whether the log holds a signal: its replay raises an event.
    pub(super) fn raises(&self) -> bool {
        self.entries
            .iter()
            .any(|(_, entry)| matches!(entry, Entry::Signal(_)))
    }

    /// Rewind `records` to the log's start, once per drain: whether it did so now.
    pub(super) fn rewind(&mut self, records: &mut Vec<Record>) -> bool {
        if self.replaying {
            return false;
        }
        self.replaying = true;
        if let Some(start) = self.start.take() {
            *records = start;
        }
        true
    }

    /// Replay onto `records` the writes `until` admits, up to the next signal it admits, which it
    /// hands back with its stamp.
    pub(super) fn next(
        &mut self,
        records: &mut Vec<Record>,
        until: Until,
    ) -> Option<(Stamp, Signal)> {
        while self.entries.front().is_some_and(|(s, _)| until.admits(*s)) {
            let Some((at, entry)) = self.entries.pop_front() else {
                break;
            };
            match entry {
                Entry::Write(w) => w.apply(records),
                Entry::Signal(s) => return Some((at, s)),
            }
        }
        None
    }

    /// The drain's end, however it ended: what it did not replay is written onto `records`, so the
    /// list stands as the span left it, and its signals wait for the next drain, as every lane's
    /// unfired items do; the next write opens a new log.
    pub(super) fn finish(&mut self, records: &mut Vec<Record>) {
        if !self.replaying {
            return;
        }
        self.entries.retain(|(_, entry)| match entry {
            Entry::Write(w) => {
                w.apply(records);
                false
            }
            Entry::Signal(_) => true,
        });
        self.replaying = false;
        self.start = None;
    }

    /// Everything logged, dropped unreplayed: no in-game interface is up to hear it.
    pub(super) fn drop_all(&mut self) {
        self.entries.clear();
        self.start = None;
        self.replaying = false;
    }

    /// The signals logged and not yet fired, in order.
    #[cfg(test)]
    pub(super) fn signals(&self) -> Vec<Signal> {
        self.entries
            .iter()
            .filter_map(|(_, e)| match e {
                Entry::Signal(s) => Some(*s),
                Entry::Write(_) => None,
            })
            .collect()
    }
}
