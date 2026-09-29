//! `ChrRaces.dbc` column 9: the creature type a player's race reads as, the last stage of the
//! creature-type resolver (`0x605570`: race byte → row `[0xc0dee0][race]`, offset `0x24`, at
//! `0x6055ca`). The runtime uses a frozen copy, `spell::unit_type::race_creature_type`; this
//! loader backs the test that pins it.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";

/// Each `ChrRaces.dbc` row's creature type by race id; a race without a row is absent.
pub fn load_race_creature_types(chain: &mut Chain) -> Result<HashMap<u8, u32>> {
    let bytes = chain
        .read_file(CHR_RACES)
        .with_context(|| format!("reading {CHR_RACES}"))?;
    let mut schema = Schema::new("ChrRaces");
    for i in 0..29 {
        let ty = match i {
            15 | 26 | 27 | 28 => FieldType::String,
            _ => FieldType::UInt32,
        };
        schema.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    let rs = parse(&bytes, schema, "ChrRaces")?;
    Ok(rs
        .records()
        .iter()
        .filter_map(|r| Some((u8::try_from(u32_at(r, 0)?).ok()?, u32_at(r, 9)?)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nine rows, every one Humanoid (7), the rows `race_creature_type` freezes.
    #[test]
    fn every_shipped_race_reads_humanoid() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_race_creature_types(&mut chain).expect("load ChrRaces");
        assert_eq!(rows.len(), 9, "ChrRaces.dbc row count");
        for race in 1..=9u8 {
            assert_eq!(rows.get(&race), Some(&7), "race {race}");
        }
        for race in [0u8, 10, 255] {
            assert!(!rows.contains_key(&race), "race {race} has no row");
        }
    }
}
