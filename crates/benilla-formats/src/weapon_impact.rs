//! `WeaponImpactSounds.dbc` — the melee impact kit table.
//!
//! Layout — VERIFIED against build 5875 (header + full row dump with SoundEntries name joins,
//! 2026-07-03): **30 × 23 × 92 B**: `ID(0), WeaponSubClassID(1), Metal(2),
//! ImpactSound[10](3..12), CritImpactSound[10](13..22)`. Column 2 is the weapon's own material
//! variant (subclasses appear twice — e.g. `Mace1H_*` at 0 vs `Mace1HMetal_*` at 1 — and it
//! selects the parry family: metal weapons carry `*ParryMetal*` kits). The ten slots, pinned by
//! the kit names on rows 1–3 (`Axe1H_ArmorFlesh`, `Shield Metal Impact`,
//! `1hParryMetalHitMetal`, `Axe1H_HitStone`, `Ethereal_1H`…):
//!
//! | slot | target |
//! |---|---|
//! | 0 | flesh (`*_ArmorFlesh`) |
//! | 1 | chain armor |
//! | 2 | plate armor |
//! | 3 | shield, metal (block) |
//! | 4 | shield, wood (block) |
//! | 5 | parry vs a metal weapon |
//! | 6 | parry vs a wood weapon |
//! | 7 | wood creature (`CreatureImpactType` 2) |
//! | 8 | stone creature (`CreatureImpactType` 1) |
//! | 9 | ethereal creature (`CreatureImpactType` 3) |
//!
//! The runtime mirror is the client's `AUIMPACTSOUNDARRAY` (10 per-row slots, reader `0x587450`;
//! well-known kits like `Combat_Miss_1H` cached separately at `0x4575b0`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

/// The impact-slot indices (module docs — pinned by the 5875 kit names).
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

/// `(WeaponSubClassID, metal)` → the impact kits. The item-weapon subclass ids (0 Axe1H,
/// 1 Axe2H, … 7 Sword1H, 13 Fist/unarmed, 15 Dagger) all carry rows.
pub struct WeaponImpactCatalog {
    rows: HashMap<(u32, bool), WeaponImpactRow>,
}

impl WeaponImpactCatalog {
    /// The row for a weapon subclass, preferring the given material variant and falling back to
    /// the other (a few subclasses have only one). `None` when the subclass has no row at all
    /// (wands, thrown — no melee impact).
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

    /// The real 5875 table decodes to the byte-verified row 1 (Axe1H metal: flesh 171 / crit
    /// 172, stone 3206, parry-metal 1002) and the unarmed row; the material fallback resolves a
    /// single-variant subclass from either flag. Skips without client data.
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

        // Fallback: bows (2) only have a non-metal row; asking for metal still resolves.
        assert!(cat.get(2, true).is_some());
    }
}
