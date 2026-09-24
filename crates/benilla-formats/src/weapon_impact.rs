//! `WeaponImpactSounds.dbc`: the melee impact sounds, ten normal and ten critical kits per weapon
//! subclass and material (column 2; a metal weapon carries the metal parry kits), which the client
//! reads as `AUIMPACTSOUNDARRAY` (`0x587450`). Slots 7-9 are wood, stone and ethereal creatures,
//! `CreatureImpactType` 2, 1 and 3.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

/// The ten target slots, as the shipped kit names pin them.
pub mod impact_slot {
    pub const FLESH: usize = 0;
    pub const CHAIN: usize = 1;
    pub const PLATE: usize = 2;
    pub const SHIELD_METAL: usize = 3;
    pub const SHIELD_WOOD: usize = 4;
    pub const PARRY_METAL: usize = 5;
    pub const PARRY_WOOD: usize = 6;
    pub const WOOD: usize = 7;
    pub const STONE: usize = 8;
    pub const ETHEREAL: usize = 9;
}

/// One weapon row: the normal and critical impact kits by target slot.
pub struct WeaponImpactRow {
    pub impact: [u32; 10],
    pub crit: [u32; 10],
}

/// `(WeaponSubClassID, metal)` → the impact kits.
pub struct WeaponImpactCatalog {
    rows: HashMap<(u32, bool), WeaponImpactRow>,
}

impl WeaponImpactCatalog {
    /// The row for a subclass in the given material, else in the other, as some have only one;
    /// wands and thrown have none.
    pub fn get(&self, subclass: u32, metal: bool) -> Option<&WeaponImpactRow> {
        self.rows
            .get(&(subclass, metal))
            .or_else(|| self.rows.get(&(subclass, !metal)))
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("WeaponImpactSounds");
    for i in 0..23 {
        s.add_field(SchemaField::new(format!("f{i}"), FieldType::UInt32));
    }
    s
}

/// Read `WeaponImpactSounds.dbc` off the patch chain.
pub fn load_weapon_impact_catalog(chain: &mut Chain) -> Result<WeaponImpactCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\WeaponImpactSounds.dbc")
        .context("reading WeaponImpactSounds.dbc")?;
    let rs = parse(&bytes, schema(), "WeaponImpactSounds")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(subclass), Some(metal)) = (u32_at(r, 1), u32_at(r, 2)) else {
            continue;
        };
        let g = |i: usize| u32_at(r, i).unwrap_or(0);
        let mut impact = [0u32; 10];
        let mut crit = [0u32; 10];
        for s in 0..10 {
            impact[s] = g(3 + s);
            crit[s] = g(13 + s);
        }
        rows.insert((subclass, metal != 0), WeaponImpactRow { impact, crit });
    }
    Ok(WeaponImpactCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_weapon_impacts_decode() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_weapon_impact_catalog(&mut chain).expect("load weapon impacts");
        assert_eq!(cat.len(), 30);

        let axe = cat.get(0, true).expect("Axe1H metal");
        assert_eq!(axe.impact[impact_slot::FLESH], 171);
        assert_eq!(axe.crit[impact_slot::FLESH], 172);
        assert_eq!(axe.impact[impact_slot::STONE], 3206);
        assert_eq!(axe.impact[impact_slot::PARRY_METAL], 1002);

        let unarmed = cat.get(13, true).expect("Fist/unarmed");
        assert_ne!(unarmed.impact[impact_slot::FLESH], 0);

        // Bows (2) have only a non-metal row.
        assert!(cat.get(2, true).is_some());
    }
}
