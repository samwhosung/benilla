//! The world-state table, the server's key/value scoreboard from `SMSG_INIT_WORLD_STATES` and
//! `SMSG_UPDATE_WORLD_STATE`. Read by the NPC-text `$<n>w`/`$<n>e` tokens ([`crate::npc_text`])
//! and by the world map's landmark pass ([`crate::ui_world_map`]), where an `AreaPOI.dbc` row with
//! a `WorldStateID` shows only while that state is nonzero.
//!
//! The reference keeps an open hash keyed by the raw wire dword (`[0xb71ec8]`, bucket
//! `mask & key`, value at node `+0x18`); keys stay raw because `$<n>e` reads the negated key
//! (`$2077e` looks up `0xFFFFF7E3`).
//!
//! An init first runs `0x4c5650(ecx = map, edx = area)`: it frees every entry, stores the pair as
//! the display filter `[0xb71e84]`/`[0xb71ea8]` ([`WorldStates::scope`]) and rebuilds the
//! world-state UI list, and only then are the packet's pairs applied. The filter is the server's
//! last init, never the avatar's position; only this clear and the logout reset write it.

use std::collections::HashMap;

use bevy::prelude::*;

/// The live world-state table, keyed and valued by raw wire dwords.
#[derive(Resource, Default)]
pub(crate) struct WorldStates {
    values: HashMap<u32, u32>,
    generation: u64,
    scope: Option<(u32, u32)>,
}

impl WorldStates {
    /// Bumped per write and per init, not per pair, the edge readers rebuild on: the reference's
    /// handler re-runs the world-map landmark builder once per packet (`0x48fa0d` to `0x4a67a0`).
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The reference's getter (`0x4c5810`): a missing key reads 0 (`0x4c5867`). Signed, as the
    /// reference prints it through `"%d"`.
    pub(crate) fn get(&self, key: u32) -> i32 {
        self.values.get(&key).copied().unwrap_or(0) as i32
    }

    /// Every pair, unordered, for instruments only (`capture::ProbeBgPlugin`); the reference
    /// never enumerates the table.
    pub(crate) fn pairs(&self) -> impl Iterator<Item = (u32, i32)> + '_ {
        self.values.iter().map(|(&k, &v)| (k, v as i32))
    }

    /// The `(map, area)` of the last init, `[0xb71e84]`/`[0xb71ea8]`; `None` before any init, the
    /// reference's `-1`.
    pub(crate) fn scope(&self) -> Option<(u32, u32)> {
        self.scope
    }

    /// An init's leading half (`0x4c5650`): drops every entry, then records the filter. The caller
    /// applies the pairs after.
    pub(crate) fn init_scope(&mut self, map: u32, area: u32) {
        self.values.clear();
        self.scope = Some((map, area));
        self.generation = self.generation.wrapping_add(1);
    }

    /// Writes `(id, value)` pairs, the reference's setter (`0x4c5870`) for both opcodes. Init's
    /// trailing `(0, 0)` is written as data; a zero value is stored, not deleted (`edx` is never
    /// tested in `[0x4c5870, 0x4c5a31)`); a repeated key overwrites in place (`0x4c58bb`).
    pub(crate) fn write(&mut self, states: &[(u32, u32)]) {
        self.values.extend(states.iter().copied());
        self.generation = self.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_miss_reads_zero_and_a_value_keeps_its_sign() {
        let mut table = WorldStates::default();
        assert_eq!(table.get(2077), 0, "empty table — the reference's `\"0\"`");
        table.write(&[(2077, 7), (2264, 0xFFFF_FFFF)]);
        assert_eq!(table.get(2077), 7);
        assert_eq!(table.get(2264), -1, "printed through `%d`");
        assert_eq!(table.get(9999), 0, "still a miss");
    }

    #[test]
    fn the_negated_key_is_a_separate_entry() {
        let mut table = WorldStates::default();
        table.write(&[(2077, 7)]);
        assert_eq!(table.get(2077u32.wrapping_neg()), 0);
        table.write(&[(2077u32.wrapping_neg(), 42)]);
        assert_eq!(table.get(2077u32.wrapping_neg()), 42);
        assert_eq!(table.get(2077), 7, "the positive key is untouched");
    }

    #[test]
    fn a_write_replaces_and_the_terminator_is_just_data() {
        let mut table = WorldStates::default();
        table.write(&[(2264, 1), (0, 0)]);
        table.write(&[(2264, 5)]);
        assert_eq!(table.get(2264), 5);
        assert_eq!(table.get(0), 0);
    }
}
