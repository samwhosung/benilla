//! `WeaponSwingSounds2.dbc`: the whoosh of a connecting melee swing. The reference loads it
//! (`0x45cb00`) into a six-slot cache (`0xb06bd4`) at `critical + swingType * 2`, and the play site
//! (`0x457f8d`) indexes it the same way; this catalog is that cache. `swingType` is the weapon's
//! `ItemSubClass.WeaponSwingSize`: 0 light, 1 medium, 2 heavy.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

/// The reference's `swingType` bound: the play site (`0x457f63`) and the loader both stop at 3.
const SWING_TYPES: usize = 3;

/// The six-slot swing-kit cache (`0xb06bd4`), indexed `critical + swingType*2`.
pub struct WeaponSwingCatalog {
    cache: [u32; SWING_TYPES * 2],
}

impl WeaponSwingCatalog {
    /// The kit for a swing; the reference plays nothing for `swingType >= 3`, never a light swing.
    pub fn kit(&self, swing_type: u32, critical: bool) -> Option<u32> {
        let slot = usize::try_from(swing_type).ok()?;
        if slot >= SWING_TYPES {
            return None;
        }
        let kit = self.cache[usize::from(critical) + slot * 2];
        (kit != 0).then_some(kit)
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("WeaponSwingSounds2");
    for name in ["ID", "SwingType", "Critical", "SoundEntriesID"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Read `WeaponSwingSounds2.dbc` off the patch chain into the reference's cache shape.
pub fn load_weapon_swing_catalog(chain: &mut Chain) -> Result<WeaponSwingCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\WeaponSwingSounds2.dbc")
        .context("reading WeaponSwingSounds2.dbc")?;
    let rs = parse(&bytes, schema(), "WeaponSwingSounds2")?;
    let mut cache = [0u32; SWING_TYPES * 2];
    for r in rs.records() {
        let g = |i: usize| u32_at(r, i).unwrap_or(0);
        let (swing_type, critical) = (g(1), g(2));
        // The reference loader's bounds: a row outside them is dropped, not clamped.
        if swing_type as usize >= SWING_TYPES || critical >= 2 {
            continue;
        }
        cache[(critical != 0) as usize + swing_type as usize * 2] = g(3);
    }
    Ok(WeaponSwingCatalog { cache })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_weapon_swing_sounds_decode() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_weapon_swing_catalog(&mut chain).expect("load weapon swing sounds");

        assert_eq!(cat.kit(0, false), Some(233), "LightWeaponNormal");
        assert_eq!(cat.kit(0, true), Some(234), "LightWeaponCritical");
        assert_eq!(cat.kit(1, false), Some(235), "MediumWeaponNormal");
        assert_eq!(cat.kit(1, true), Some(236), "MediumWeaponCritical");
        assert_eq!(cat.kit(2, false), Some(237), "HeavyWeaponNormal");
        assert_eq!(cat.kit(2, true), Some(238), "HeavyWeaponCritical");
    }

    #[test]
    fn a_weight_past_the_ceiling_is_silence_not_a_fallback() {
        let cat = WeaponSwingCatalog {
            cache: [233, 234, 235, 236, 237, 238],
        };
        assert_eq!(cat.kit(3, false), None);
        assert_eq!(cat.kit(u32::MAX, true), None);
        // A slot the file never filled reads as absent, not as kit 0.
        let sparse = WeaponSwingCatalog {
            cache: [233, 0, 0, 0, 0, 0],
        };
        assert_eq!(sparse.kit(0, false), Some(233));
        assert_eq!(sparse.kit(0, true), None);
    }
}
