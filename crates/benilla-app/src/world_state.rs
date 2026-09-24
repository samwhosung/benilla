//! The world-state table — the server's key/value scoreboard, fed by `SMSG_INIT_WORLD_STATES` and
//! `SMSG_UPDATE_WORLD_STATE` ([`benilla_protocol::messages::world_state`] for both wires).
//!
//! Two readers:
//!
//! - the NPC-text expander's `$<n>w` / `$<n>e` tokens ([`crate::npc_text`]) — the reference
//!   renders a hit as `%d` and a **miss as `"0"`**, so every such token printed `"0"` until this
//!   landed: faithful while the table was empty, wrong the moment a zone sent real states;
//! - the **world map's landmark pass** ([`crate::ui_world_map`], decision 1586) — an `AreaPOI.dbc`
//!   row carrying a `WorldStateID` shows only while that state reads non-zero, which is what makes
//!   the Eastern Plaguelands tower icons and the capitals' "Under Attack" markers appear and
//!   change side. [`WorldStates::generation`] is the edge it rebuilds on.
//!
//! Shape is the reference's, not a convenience: it keeps an open hash keyed by the **raw wire
//! dword** (`[0xb71ec8]`, bucket `mask & key`, value at node `+0x18`, miss → `0` at `0x4c5867`).
//! Raw matters because `$<n>e` reads the table at the *negated* key — `$2077e` looks up
//! `0xFFFFF7E3`, which only resolves if nothing re-interpreted the id on the way in.
//!
//! **An init CLEARS the table first**, correcting the assumption this module shipped with.
//! `0x4c5650(ecx = map, edx = area)`, the function `SMSG_INIT_WORLD_STATES`' two leading dwords go
//! to before the pair loop runs, does four things in order: it drains every entry (the all-entries
//! list, then every bucket), stores the pair as the **display filter** `[0xb71e84]`/`[0xb71ea8]`,
//! rebuilds the world-state UI list against that filter, and only then does the handler apply the
//! packet's pairs. The bucket array itself survives; only entries are freed.
//!
//! That filter pair is [`WorldStates::scope`], and it is the reference's own notion of "where the
//! player is" for this UI — **the server's last init, never the avatar's position**. Its only two
//! writers image-wide are this clear and the logout reset.

use std::collections::HashMap;

use bevy::prelude::*;

/// The live world-state table. Keys and values are raw wire dwords; see the module doc.
#[derive(Resource, Default)]
pub(crate) struct WorldStates {
    values: HashMap<u32, u32>,
    generation: u64,
    scope: Option<(u32, u32)>,
}

impl WorldStates {
    /// Bumped on every [`Self::write`] — the "the table moved" edge a reader rebuilds on.
    ///
    /// This is the reference's own trigger shape, not a cache trick bolted on: the bulk
    /// world-state handler re-runs the world-map landmark builder from *inside itself*
    /// (`0x48fa0d` → `0x4a67a0`), so a landmark list is
    /// rebuilt **because a world state arrived**, never once a frame. Counted per `write` rather
    /// than per changed value for the same reason: the handler re-runs the builder once for the
    /// whole packet, not once per pair.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The reference's getter `0x4c5810`, including its miss leg: an absent key reads **0**, which
    /// is why an un-received world state renders `"0"` rather than blank or the literal token.
    ///
    /// Returned as `i32` because that is the width the one caller formats — the reference prints
    /// the dword through `"%d"`, so a value with the top bit set shows as negative.
    pub(crate) fn get(&self, key: u32) -> i32 {
        self.values.get(&key).copied().unwrap_or(0) as i32
    }

    /// Every `(key, value)` currently in the table, in no particular order — the instrument
    /// read, and only that. No reference function enumerates this hash: the client's own readers
    /// are the point getter above and the `WorldStateUI.dbc` walk, both of which ask for a key
    /// they already know. A battleground's whole score lives here as raw dwords nobody has a name
    /// for yet, so a probe that cannot dump it cannot say what arrived (`capture::ProbeBgPlugin`).
    pub(crate) fn pairs(&self) -> impl Iterator<Item = (u32, i32)> + '_ {
        self.values.iter().map(|(&k, &v)| (k, v as i32))
    }

    /// The `(map, area)` the last `SMSG_INIT_WORLD_STATES` scoped the table to — the reference's
    /// `[0xb71e84]`/`[0xb71ea8]`, and the filter its world-state UI list is built against. `None`
    /// before any init has arrived, which is the reference's `-1` (its rebuild trigger refuses to
    /// run until an init has been seen).
    pub(crate) fn scope(&self) -> Option<(u32, u32)> {
        self.scope
    }

    /// `SMSG_INIT_WORLD_STATES`' leading half — the reference's `0x4c5650`: **drop every entry**,
    /// then record the `(map, area)` filter. The caller applies the packet's pairs after; the
    /// order is the reference's, and it is what makes a zone change forget the previous zone's
    /// keys rather than leaving them readable.
    pub(crate) fn init_scope(&mut self, map: u32, area: u32) {
        self.values.clear();
        self.scope = Some((map, area));
        self.generation = self.generation.wrapping_add(1);
    }

    /// Apply a run of `(id, value)` writes — the reference's setter `0x4c5870`, which both opcodes
    /// funnel into. Init's trailing `(0, 0)` terminator is written like any other pair, exactly as
    /// the reference's count loop does; it is data, not a sentinel — and `Set(k, 0)` **stores**
    /// rather than deleting (`edx` is never tested across `[0x4c5870, 0x4c5a31)`), which is why a
    /// state that goes to zero stays a present key reading `0`. A repeated key overwrites in place
    /// (`0x4c58bb`); there is never a second node.
    pub(crate) fn write(&mut self, states: &[(u32, u32)]) {
        self.values.extend(states.iter().copied());
        self.generation = self.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The getter's miss leg and the `%d` width: an unknown key is 0 (not blank), a known one is
    /// its value, and the top bit reads through as a negative — the three things `$<n>w` depends on.
    #[test]
    fn a_miss_reads_zero_and_a_value_keeps_its_sign() {
        let mut table = WorldStates::default();
        assert_eq!(table.get(2077), 0, "empty table — the reference's `\"0\"`");
        table.write(&[(2077, 7), (2264, 0xFFFF_FFFF)]);
        assert_eq!(table.get(2077), 7);
        assert_eq!(table.get(2264), -1, "printed through `%d`");
        assert_eq!(table.get(9999), 0, "still a miss");
    }

    /// `$<n>e`'s negated key is a *different* entry, not the same one — the whole reason ids stay
    /// raw dwords. Writing id 2077 must not make `$2077e` (key `0xFFFFF7E3`) resolve.
    #[test]
    fn the_negated_key_is_a_separate_entry() {
        let mut table = WorldStates::default();
        table.write(&[(2077, 7)]);
        assert_eq!(table.get(2077u32.wrapping_neg()), 0);
        table.write(&[(2077u32.wrapping_neg(), 42)]);
        assert_eq!(table.get(2077u32.wrapping_neg()), 42);
        assert_eq!(table.get(2077), 7, "the positive key is untouched");
    }

    /// A later write to a live key replaces it (`SMSG_UPDATE_WORLD_STATE`'s whole job), and an
    /// init's terminator lands as the ordinary entry it is.
    #[test]
    fn a_write_replaces_and_the_terminator_is_just_data() {
        let mut table = WorldStates::default();
        table.write(&[(2264, 1), (0, 0)]);
        table.write(&[(2264, 5)]);
        assert_eq!(table.get(2264), 5);
        assert_eq!(table.get(0), 0);
    }
}
