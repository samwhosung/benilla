//! **EnvironmentalDamage.dbc** — the environmental-damage feedback table: damage type →
//! `SpellVisualKit` id, played on the victim when `SMSG_ENVIRONMENTALDAMAGELOG` arrives (the
//! fall-landing dust puff and its five siblings).
//!
//! The client shape: init `0x603900`
//! zeroes a **6-slot table** (`[0xc4d8e4]`) and, for each record with `field1 < 6`, stores
//! `slot[field1] = field2` — so field 1 is the `EnvironmentalDamageType` enum (0 exhausted ·
//! 1 drowning · 2 fall · 3 lava · 4 slime · 5 fire; the wire's `damage_type`) and field 2 the
//! kit id. Sole reader `0x624fcc` (in the 0x1FC consequence method `0x624f30`) bounds the kit id
//! against `SpellVisualKit.dbc` and plays it. Build 5875 data: 0→871, 1→870, 2→**1066** (the
//! "DustCloud Land" kit), 3→1064, 4→1065, 5→1067.

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

const ENVIRONMENTAL_DAMAGE: &str = "DBFilesClient\\EnvironmentalDamage.dbc";

/// The 6-slot damage-type → `SpellVisualKit` table (the client's `[0xc4d8e4]`).
pub struct EnvironmentalDamageTable {
    kits: [u32; 6],
}

impl EnvironmentalDamageTable {
    /// The `SpellVisualKit` id for a wire `damage_type` (0–5). `None` for an out-of-range type or
    /// an empty slot — the client's `field1 < 6` init guard and zero-init respectively.
    pub fn kit_id(&self, damage_type: u8) -> Option<u32> {
        self.kits
            .get(usize::from(damage_type))
            .copied()
            .filter(|&k| k != 0)
    }
}

/// EnvironmentalDamage.dbc — 3 fields in build 5875: ID, the damage-type enum, the kit id.
fn schema() -> Schema {
    let mut s = Schema::new("EnvironmentalDamage");
    for name in ["ID", "DamageType", "VisualKit"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Load the table from the patch chain, exactly as the client's init `0x603900` fills its 6-slot
/// store: zeroed slots, `slot[DamageType] = VisualKit` for each in-range record.
pub fn load_environmental_damage(chain: &mut Chain) -> Result<EnvironmentalDamageTable> {
    let bytes = chain
        .read_file(ENVIRONMENTAL_DAMAGE)
        .with_context(|| format!("reading {ENVIRONMENTAL_DAMAGE}"))?;
    table_from(&bytes)
}

/// The fill itself, split from the chain read so the golden test drives the identical path.
fn table_from(bytes: &[u8]) -> Result<EnvironmentalDamageTable> {
    let rs = parse(bytes, schema(), "EnvironmentalDamage")?;
    let mut kits = [0u32; 6];
    for r in rs.records() {
        if let (Some(ty), Some(kit)) = (u32_at(r, 1), u32_at(r, 2)) {
            if let Some(slot) = kits.get_mut(ty as usize) {
                *slot = kit;
            }
        }
    }
    Ok(EnvironmentalDamageTable { kits })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built WDBC with the real 5875 rows (verified via benilla-extract 2026-07-16) plus an
    /// out-of-range type-9 row that must hit the `field1 < 6` init guard and load nowhere.
    #[test]
    fn table_loads_by_damage_type_with_the_range_guard() {
        let rows: &[[u32; 3]] = &[
            [1, 0, 871],
            [2, 1, 870],
            [3, 2, 1066],
            [4, 3, 1064],
            [5, 4, 1065],
            [6, 5, 1067],
            [7, 9, 999], // out of range: the client's `cmp edx,6; jae` skip
        ];
        let mut dbc = Vec::new();
        dbc.extend_from_slice(b"WDBC");
        dbc.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        dbc.extend_from_slice(&3u32.to_le_bytes()); // fields
        dbc.extend_from_slice(&12u32.to_le_bytes()); // record size
        dbc.extend_from_slice(&1u32.to_le_bytes()); // string block
        for row in rows {
            for v in row {
                dbc.extend_from_slice(&v.to_le_bytes());
            }
        }
        dbc.push(0); // the string block

        let table = table_from(&dbc).unwrap();
        assert_eq!(table.kit_id(2), Some(1066), "fall → the DustCloud Land kit");
        assert_eq!(table.kit_id(0), Some(871));
        assert_eq!(table.kit_id(5), Some(1067));
        assert_eq!(table.kit_id(9), None, "out-of-range type");
    }
}
