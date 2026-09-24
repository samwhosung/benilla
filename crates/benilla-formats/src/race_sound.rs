//! `ChrRaces.dbc` column 3: the sound the client plays on `SMSG_EXPLORATION_EXPERIENCE`, by the
//! player's race (`0x5e41d2`: row `[0xc0dee0][race]`, offset `0xc`, played at `0x458850`).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";

/// Race (the `UNIT_FIELD_BYTES_0` race byte) → exploration `SoundEntries` id; a 0 is absent.
pub struct ExplorationSoundCatalog {
    by_race: HashMap<u32, u32>,
}

impl ExplorationSoundCatalog {
    pub fn kit(&self, race: u32) -> Option<u32> {
        self.by_race.get(&race).copied()
    }

    pub fn len(&self) -> usize {
        self.by_race.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_race.is_empty()
    }
}

/// Load the race → exploration-sound map off the patch chain.
pub fn load_exploration_sound_catalog(chain: &mut Chain) -> Result<ExplorationSoundCatalog> {
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
    let mut by_race = HashMap::new();
    for r in rs.records() {
        let (Some(id), Some(kit)) = (u32_at(r, 0), u32_at(r, 3)) else {
            continue;
        };
        if kit != 0 {
            by_race.insert(id, kit);
        }
    }
    Ok(ExplorationSoundCatalog { by_race })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_chr_races_exploration_kits() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_exploration_sound_catalog(&mut chain).expect("load ChrRaces");
        for race in 1..=8u32 {
            let kit = cat.kit(race);
            eprintln!("race {race}: exploration kit {kit:?}");
            assert!(kit.is_some(), "race {race} has no exploration sound");
        }
        // The shipped kits run 4140-4147; Human and Orc guard the column position.
        assert_eq!(cat.kit(1), Some(4140));
        assert_eq!(cat.kit(2), Some(4141));
    }
}
