//! `EnvironmentalDamage.dbc`: the `SpellVisualKit` played on the victim of
//! `SMSG_ENVIRONMENTALDAMAGELOG` (read at `0x624fcc` in `0x624f30`), by the wire's `damage_type`:
//! 0 exhausted, 1 drowning, 2 fall, 3 lava, 4 slime, 5 fire.

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

const ENVIRONMENTAL_DAMAGE: &str = "DBFilesClient\\EnvironmentalDamage.dbc";

/// The client's 6-slot damage type → kit table (`[0xc4d8e4]`).
pub struct EnvironmentalDamageTable {
    kits: [u32; 6],
}

impl EnvironmentalDamageTable {
    /// The kit for a wire `damage_type`; `None` past type 5 or for an empty slot.
    pub fn kit_id(&self, damage_type: u8) -> Option<u32> {
        self.kits
            .get(usize::from(damage_type))
            .copied()
            .filter(|&k| k != 0)
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("EnvironmentalDamage");
    for name in ["ID", "DamageType", "VisualKit"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Load the table as the client's init `0x603900` fills it: zeroed, then
/// `slot[DamageType] = VisualKit` for each row whose type is below 6.
pub fn load_environmental_damage(chain: &mut Chain) -> Result<EnvironmentalDamageTable> {
    let bytes = chain
        .read_file(ENVIRONMENTAL_DAMAGE)
        .with_context(|| format!("reading {ENVIRONMENTAL_DAMAGE}"))?;
    table_from(&bytes)
}

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

    /// The shipped rows, plus a type-9 row that the range guard must skip.
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
