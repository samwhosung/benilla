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

use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use std::sync::Arc;

use benilla_protocol::messages::FACTION_LIST_LEN;
use bevy::prelude::*;

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
/// A clone shares the slots and copies the rest: the shared handle is there for the Lua API's
/// relation snapshot alone, which reads the slots live between pushes.
#[derive(Resource, Default, Clone)]
pub(crate) struct Reputations {
    slots: Arc<Slots>,
    pub(crate) forced: Vec<(u32, u32)>,
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
        self.slots.cells.get(slot).map_or((0, 0), |c| {
            (
                c.flags.load(Ordering::Relaxed),
                c.standing.load(Ordering::Relaxed),
            )
        })
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
    pub(crate) fn slots(&self) -> impl Iterator<Item = (u8, i32)> + '_ {
        (0..FACTION_LIST_LEN).map(|i| self.slot(i))
    }

    /// `SMSG_INITIALIZE_FACTIONS`: the list whole, from slot 0; a slot it does not reach reads
    /// `(0, 0)`, and one past the 64 is dropped.
    pub(crate) fn set_slots(&mut self, slots: &[(u8, i32)]) {
        for (i, cell) in self.slots.cells.iter().enumerate() {
            let (flags, standing) = slots.get(i).copied().unwrap_or((0, 0));
            cell.flags.store(flags, Ordering::Relaxed);
            cell.standing.store(standing, Ordering::Relaxed);
        }
    }

    /// A wire message's store of slot `slot`'s wire standing (`0x4d6330`).
    pub(crate) fn set_standing(&mut self, slot: usize, standing: i32) {
        if let Some(cell) = self.slots.cells.get(slot) {
            cell.standing.store(standing, Ordering::Relaxed);
        }
    }

    /// A wire message's store of slot `slot`'s flag byte (`0x4d5fc0`).
    pub(crate) fn set_flags(&mut self, slot: usize, flags: u8) {
        if let Some(cell) = self.slots.cells.get(slot) {
            cell.flags.store(flags, Ordering::Relaxed);
        }
    }

    /// `SMSG_SET_FORCED_REACTIONS`: the table replaces the last whole (`0x4d59a0`).
    pub(crate) fn set_forced(&mut self, table: Vec<(u32, u32)>) {
        self.forced = table;
    }

    /// The teardown (`0x4d5360`): the forced table's count zeroed (`0x4d5419`) and all 64 slots
    /// with it (`0x4d541f`-`0x4d542b`).
    pub(crate) fn tear_down(&mut self) {
        self.forced.clear();
        self.set_slots(&[]);
    }
}
