//! CreatureType.dbc — the 13-row creature-class table (Beast, Humanoid, Critter, Totem, …).
//!
//! The one consumer today: the TAB-target scan's critter rejection (the scorer `0x494200` looks
//! the candidate's creature type up in the cached table `[0xc0de2c]` and rejects when
//! `row[+0x28] & 1` — **flag bit 0**).
//! In the shipped 1.12 data only **Critter (8)** carries the bit (the "critter/totem/non-combat
//! pet" gloss is later-era; see the real-chain test). A unit's creature type itself comes off
//! the wire (`SMSG_CREATURE_QUERY_RESPONSE`), cached with its name.
//!
//! Record layout (1.12, 11 × u32-slot fields): id@0, name(8 locales + flags = 9 slots)@1..9,
//! flags@10.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const CREATURE_TYPE: &str = "DBFilesClient\\CreatureType.dbc";

/// creature-type id → the row's flags dword. Query through [`CreatureTypeFlags::no_tab_target`].
#[derive(Debug, Default, Clone)]
pub struct CreatureTypeFlags(HashMap<u32, u32>);

impl CreatureTypeFlags {
    /// Whether this creature type is excluded from TAB/nearest-enemy targeting — the client's
    /// `flags & 1` test. In the shipped **1.12** data exactly ONE row carries the bit:
    /// **Critter (8)** (real-chain verified below — Totem 11 does NOT; the "can't tab totems"
    /// lore is a later-era flag change, not 5875 data). An unknown or missing type is targetable
    /// (the client's out-of-range index skips the check).
    pub fn no_tab_target(&self, creature_type: u32) -> bool {
        self.0.get(&creature_type).is_some_and(|f| f & 1 != 0)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn creature_type_schema() -> Schema {
    let mut s = Schema::new("CreatureType");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    s
}

/// Load CreatureType.dbc from the patch chain.
pub fn load_creature_type_flags(chain: &mut Chain) -> Result<CreatureTypeFlags> {
    let bytes = chain
        .read_file(CREATURE_TYPE)
        .with_context(|| format!("reading {CREATURE_TYPE}"))?;
    let rs = parse(&bytes, creature_type_schema(), "CreatureType")?;
    let mut flags = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let Some(id) = u32_at(r, 0) {
            flags.insert(id, u32_at(r, 10).unwrap_or(0));
        }
    }
    Ok(CreatureTypeFlags(flags))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped 1.12 data, read through the real chain when the install is present: **11**
    /// rows (ids 1–11), and exactly one carries flag bit 0 — **Critter (8)**. Totem (11) does
    /// NOT (verified against the real file; the critter/totem gloss from later expansions doesn't
    /// hold for 5875 data). Beast (1) and Humanoid (7) are targetable.
    #[test]
    fn shipped_flags_mark_only_critter() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let flags = load_creature_type_flags(&mut chain).expect("load CreatureType.dbc");
        assert_eq!(flags.len(), 11, "1.12 ships 11 creature types");
        assert!(flags.no_tab_target(8), "Critter (8) must be un-TAB-able");
        for id in [1, 7, 11] {
            assert!(!flags.no_tab_target(id), "type {id} must be targetable");
        }
        assert!(!flags.no_tab_target(999), "unknown type is targetable");
    }
}
