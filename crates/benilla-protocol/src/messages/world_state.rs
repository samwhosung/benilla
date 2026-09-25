//! World states, the server's key/value table (`SMSG_INIT_WORLD_STATES` `0x2C2`,
//! `SMSG_UPDATE_WORLD_STATE` `0x2C3`) that NPC-text `$<n>w`/`$<n>e` tokens and scoreboards read.
//!
//! On `0x2C2` the reference clears the table and takes map and zone as its UI filter (`0x4c5650`)
//! before the pairs run. vmangos counts a trailing `(0, 0)` pair (`Player.cpp:8295-8369`), which
//! is read as data. Ids and values stay raw dwords: `$<n>e` reads the table at the negated key, so
//! `$2077e` looks up `0xFFFFF7E3`.

use std::io;

use crate::wire::{read_u16_le, read_u32_le};

/// `SMSG_INIT_WORLD_STATES`: the whole table for a zone, sent on login and every zone change.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InitWorldStates {
    /// The map the states are scoped to; the reference keeps it at `[0xb71e84]`.
    pub map: u32,
    /// The zone vmangos fills in; the reference keeps it at `[0xb71ea8]` and matches it against
    /// `WorldStateUI.dbc`'s `AreaID`.
    pub zone: u32,
    /// `(id, value)` verbatim, including the trailing `(0, 0)` terminator.
    pub states: Vec<(u32, u32)>,
}

/// Read `SMSG_INIT_WORLD_STATES`: `u32 map, u32 zone, u16 count`, then `count` `(id, value)` pairs.
pub(in crate::messages) fn read_init_world_states(r: &mut &[u8]) -> io::Result<InitWorldStates> {
    let map = read_u32_le(r)?;
    let zone = read_u32_le(r)?;
    let count = read_u16_le(r)?;
    let mut states = Vec::new();
    for _ in 0..count {
        states.push((read_u32_le(r)?, read_u32_le(r)?));
    }
    Ok(InitWorldStates { map, zone, states })
}

/// Read `SMSG_UPDATE_WORLD_STATE`: one `(id, value)` dword pair.
pub(in crate::messages) fn read_update_world_state(r: &mut &[u8]) -> io::Result<(u32, u32)> {
    Ok((read_u32_le(r)?, read_u32_le(r)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_reads_every_pair_including_the_terminator() {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes()); // map 0 (Eastern Kingdoms)
        body.extend_from_slice(&1519u32.to_le_bytes()); // zone (Stormwind)
        body.extend_from_slice(&3u16.to_le_bytes()); // 2 real pairs + the terminator
        for (id, value) in [(2264u32, 7u32), (2263u32, 0u32), (0, 0)] {
            body.extend_from_slice(&id.to_le_bytes());
            body.extend_from_slice(&value.to_le_bytes());
        }
        let got = read_init_world_states(&mut body.as_slice()).unwrap();
        assert_eq!(got.map, 0);
        assert_eq!(got.zone, 1519);
        assert_eq!(got.states, vec![(2264, 7), (2263, 0), (0, 0)]);
    }

    #[test]
    fn init_truncated_run_errors() {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&u16::MAX.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        assert!(read_init_world_states(&mut body.as_slice()).is_err());
    }

    /// A top-bit value stays unsigned; only the `%d` render downstream gives it a sign.
    #[test]
    fn update_reads_a_raw_dword_pair() {
        let mut body = Vec::new();
        body.extend_from_slice(&2264u32.to_le_bytes());
        body.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert_eq!(
            read_update_world_state(&mut body.as_slice()).unwrap(),
            (2264, 0xFFFF_FFFF)
        );
    }
}
