//! `SoundWaterType.dbc` — the liquid-class → ambient-loop-kit map of the **above-water liquid
//! ambient-loop system** (`0x462a40`, benilla decision 0506):
//! the continuous ocean/river/lava/slime sound the client plays near liquid. The driver reads the
//! nearest wet cell's MCLQ low nibble as `class = nibble & 3`, `speed = nibble >> 2`, and resolves
//! the kit **data-driven** through this table (store `[0xc0d898]`; not hardcoded ids): 16-byte
//! records `ID, SoundType(class), FluidSpeed(4·speed), SoundEntriesID`.
//!
//! 5875 content: river 0/4/8 → 1111 RiverStill / 1112 RiverSlow / 1113 RiverFast; ocean → 1114
//! at every speed; magma 0→3072 LavaPoolLoop, 4/8→3052 LavaFlowLoop; slime → 3880 SlimeLoop.

use std::collections::HashMap;

use anyhow::Result;
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

/// The `(SoundType, FluidSpeed) → SoundEntries kit` map.
pub struct WaterSoundCatalog {
    by_key: HashMap<(u32, u32), u32>,
}

impl WaterSoundCatalog {
    /// The loop kit for an MCLQ/MLIQ cell low nibble (`class = nibble & 3`, `FluidSpeed =
    /// nibble & 0xc` — the nibble packs `class + 4·speed`). `None` for a nibble the table
    /// doesn't cover (incl. the `0xf` dry sentinel).
    pub fn kit_for_nibble(&self, nibble: u8) -> Option<u32> {
        let n = u32::from(nibble & 0xf);
        self.by_key.get(&(n & 3, n & 0xc)).copied()
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("SoundWaterType");
    for name in ["ID", "SoundType", "FluidSpeed", "SoundEntriesID"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Read `SoundWaterType.dbc` off the patch chain.
pub fn load_water_sound_catalog(chain: &mut Chain) -> Result<WaterSoundCatalog> {
    let bytes = chain.read_file("DBFilesClient\\SoundWaterType.dbc")?;
    let rs = parse(&bytes, schema(), "SoundWaterType")?;
    let mut by_key = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(class), Some(speed), Some(kit)) = (u32_at(r, 1), u32_at(r, 2), u32_at(r, 3))
        else {
            continue;
        };
        if kit != 0 {
            by_key.insert((class, speed), kit);
        }
    }
    Ok(WaterSoundCatalog { by_key })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 5875 table, end to end: every wet nibble resolves, and the mapping matches the
    /// reference's dispatch (`0x462a40`: river split by authored speed, ocean uniform, magma
    /// pool/flow).
    /// Skips without client data.
    #[test]
    fn real_table_resolves_every_wet_nibble() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_water_sound_catalog(&mut chain).expect("load SoundWaterType");
        assert_eq!(cat.len(), 12, "all 5875 rows load");
        // nibble = class + 4·speed
        assert_eq!(cat.kit_for_nibble(0), Some(1111), "RiverStill");
        assert_eq!(cat.kit_for_nibble(4), Some(1112), "RiverSlow");
        assert_eq!(cat.kit_for_nibble(8), Some(1113), "RiverFast");
        assert_eq!(cat.kit_for_nibble(1), Some(1114), "Ocean");
        assert_eq!(cat.kit_for_nibble(5), Some(1114), "Ocean at any speed");
        assert_eq!(cat.kit_for_nibble(2), Some(3072), "LavaPoolLoop");
        assert_eq!(cat.kit_for_nibble(6), Some(3052), "LavaFlowLoop");
        assert_eq!(cat.kit_for_nibble(3), Some(3880), "SlimeLoop");
        assert_eq!(cat.kit_for_nibble(7), Some(3880), "SlimeLoop fast");
        assert_eq!(
            cat.kit_for_nibble(0xf),
            None,
            "dry sentinel resolves nothing"
        );
    }
}
