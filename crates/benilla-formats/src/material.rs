//! `Material.dbc`: the armor foley a body makes as it moves, and the flags behind weapon-impact
//! sounds. A footfall is two sounds: `$FSD` (`0x623390`) plays the foley (`0x623610` for a unit,
//! `0x62fa30` for a player) before the terrain step and its gates, so a creature of footstep class
//! 0 still rustles; the foley is on the uncapped bus 0, the step on bus 9 (cap 6). `FoleySoundID`
//! (row `+0x8`, read at `0x4584e0`) plays at the feet plus 2.0 yd (`0x45851d`, `[0x801628]`) at
//! volume 1.0; only chain, plate and leather have one.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

/// `Material.dbc` keyed by id.
pub struct MaterialCatalog {
    foley: HashMap<u32, u32>,
    /// The `Flags` column: the metal or wood side of a weapon impact and a chest's armor slot.
    flags: HashMap<u32, u32>,
}

impl MaterialCatalog {
    /// The foley kit for a material id; `None` for no foley or no row, where the reference's
    /// negative and past-the-end misses (`0x4584e6`, `0x4584ea`) land too.
    pub fn foley_kit(&self, material: u32) -> Option<u32> {
        self.foley.get(&material).copied().filter(|&k| k != 0)
    }

    /// Whether a material is metal-bodied, `Flags & 0x1` (`0x5d9a50`, tested at `0x5d9a68`): the
    /// metal or wood half of `WeaponImpactSounds`, for the weapon's row and the parry slot
    /// (`0x457e80`, asked twice by `0x457dc0`). Leather, cloth, liquid and an absent material
    /// (id 0) are not metal, so this is not `material != WOOD`.
    pub fn is_metal(&self, material: u32) -> bool {
        self.flags.get(&material).is_some_and(|f| f & 0x1 != 0)
    }

    /// The `WeaponImpactSounds` target slot a player's chest material presents (`0x62fb70`):
    /// `Flags` bit 1 gives 2 (plate), else bit 2 gives 1 (chain), else 0 (flesh), where leather and
    /// cloth land. Creatures take theirs from `CreatureSoundData` through `{0, 8, 7, 9}`
    /// (`0x6238f0`).
    pub fn armor_impact_slot(&self, material: u32) -> u32 {
        let Some(&flags) = self.flags.get(&material) else {
            return 0;
        };
        if flags & 0x2 != 0 {
            2
        } else {
            (flags & 0x4) >> 2
        }
    }

    pub fn len(&self) -> usize {
        self.foley.len()
    }

    pub fn is_empty(&self) -> bool {
        self.foley.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("Material");
    for name in ["ID", "Flags", "FoleySoundID"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Read `Material.dbc` off the patch chain.
pub fn load_material_catalog(chain: &mut Chain) -> Result<MaterialCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\Material.dbc")
        .context("reading Material.dbc")?;
    let rs = parse(&bytes, schema(), "Material")?;
    let mut foley = HashMap::with_capacity(rs.records().len());
    let mut flags = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let Some(id) = u32_at(r, 0) {
            foley.insert(id, u32_at(r, 2).unwrap_or(0));
            flags.insert(id, u32_at(r, 1).unwrap_or(0));
        }
    }
    Ok(MaterialCatalog { foley, flags })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 5875 table, whose ids are the item query's materials (1 metal, 2 wood, 5 chain,
    /// 6 plate, 7 cloth, 8 leather).
    #[test]
    fn real_materials_decode() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_material_catalog(&mut chain).expect("load materials");
        assert_eq!(cat.len(), 8);

        assert_eq!(cat.foley_kit(5), Some(1005), "chain");
        assert_eq!(cat.foley_kit(6), Some(1004), "plate");
        assert_eq!(cat.foley_kit(8), Some(1003), "leather");

        // A robe must not borrow leather's rustle.
        assert_eq!(cat.foley_kit(7), None, "cloth");
        for quiet in [1, 2, 3, 4] {
            assert_eq!(cat.foley_kit(quiet), None, "material {quiet}");
        }

        // 0 is the wire's no-material id; 9 is past the table.
        assert_eq!(cat.foley_kit(0), None);
        assert_eq!(cat.foley_kit(9), None);
    }

    /// The `Flags` column on 5875; a missing material, as a creature without virtual-item info
    /// has, is not metal.
    #[test]
    fn the_flags_column_splits_metal_and_names_the_armor_slot() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_material_catalog(&mut chain).expect("load materials");

        for metal in [1, 4, 5, 6] {
            assert!(cat.is_metal(metal), "material {metal} is metal-bodied");
        }
        for soft in [2, 3, 7, 8] {
            assert!(!cat.is_metal(soft), "material {soft} is not metal");
        }
        assert!(!cat.is_metal(0), "an absent material is not metal");
        assert!(!cat.is_metal(99), "an unknown material is not metal");

        assert_eq!(cat.armor_impact_slot(6), 2, "plate");
        assert_eq!(cat.armor_impact_slot(5), 1, "chain");
        for flesh in [0, 1, 2, 3, 4, 7, 8, 99] {
            assert_eq!(cat.armor_impact_slot(flesh), 0, "material {flesh}");
        }
    }
}
