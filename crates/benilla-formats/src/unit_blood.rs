//! `UnitBlood.dbc` and `UnitBloodLevels.dbc`, the melee blood-spurt tables.
//!
//! The chain (`0x624530`, `0x625010`): a melee hit with `HitInfo & 0x2`, nonzero damage and
//! victim state 1 or 4 resolves the victim's blood id ([`BloodCatalog::level_key`]), whose
//! `UnitBloodLevels` row names a `UnitBlood` row per violence level: the censorship table, where
//! red blood turns green at violence 1 and vanishes at 0. That row's first four fields are
//! `SpellVisualEffectName` ids in front-small, front-large, back-small, back-large order (read
//! from the effect names): front or back by `sign(victimForward · (attackerPos − victimPos))`,
//! large on the crushing bit `HitInfo & 0x2000`, not the crit bit `0x80`, the wound flinch's.
//!
//! Fields 5 to 9 are ground-splat textures 1.12.1 never reads: no instruction touches
//! `UnitBloodRecord + 0x14` to `+0x24` and the string `"BloodSplat"` is absent from the binary,
//! though the textures ship. [`BloodCatalog::splats`] exists to pin that, not to feed a renderer.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

const UNIT_BLOOD: &str = "DBFilesClient\\UnitBlood.dbc";
const UNIT_BLOOD_LEVELS: &str = "DBFilesClient\\UnitBloodLevels.dbc";

/// Both blood tables: `(blood id, violence, front, crushing)` to the spurt's
/// `SpellVisualEffectName` id.
pub struct BloodCatalog {
    /// `UnitBloodLevels` id to the `UnitBlood` id per violence level 0 to 2; 0 is no blood.
    levels: HashMap<u32, [u32; 3]>,
    /// The id of `UnitBloodLevels`' first file row, the records base the reference's third tier
    /// returns: 1 as shipped, and not `idIndex[0]`, which is null.
    default_level: Option<u32>,
    rows: HashMap<u32, BloodRow>,
}

struct BloodRow {
    /// `[FrontSmall, FrontLarge, BackSmall, BackLarge]` effect ids.
    effects: [u32; 4],
    /// The non-empty ground-splat textures in column order, extensionless; four in every shipped
    /// row.
    splats: Vec<String>,
}

impl BloodCatalog {
    /// The victim's `UnitBloodLevels` key by the reference's three tiers (`0x60afb0`, which stores
    /// the row at `[unit+0xb48]`): `CreatureDisplayInfo.BloodLevel`, else
    /// `CreatureModelData.BloodID`, else `UnitBloodLevels`' first file row, the red one. A tier
    /// misses on `< 0`, on `> maxId` or on an empty `idIndex` slot. `BloodID` -1 is a miss, not
    /// bloodless: 595 of the 10534 shipped displays reach the third tier, Quilboar, Crocolisks and
    /// Gnolls among them.
    pub fn level_key(&self, display_blood: i32, model_blood: i32) -> Option<u32> {
        let resolves = |v: i32| {
            u32::try_from(v)
                .ok()
                .filter(|k| self.levels.contains_key(k))
        };
        resolves(display_blood)
            .or_else(|| resolves(model_blood))
            .or(self.default_level)
    }

    /// The spurt for a victim's [`Self::level_key`] at a violence level (0 to 2, clamped); `None`
    /// where the level censors it, as every row does at violence 0, or the key names no row.
    pub fn effect_id(
        &self,
        blood_id: i32,
        violence: usize,
        front: bool,
        large: bool,
    ) -> Option<u32> {
        let row = self.row(blood_id, violence)?;
        let idx = match (front, large) {
            (true, false) => 0,
            (true, true) => 1,
            (false, false) => 2,
            (false, true) => 3,
        };
        Some(row.effects[idx]).filter(|&id| id != 0)
    }

    /// The ground-splat textures for a victim, through the same censored row as
    /// [`Self::effect_id`].
    pub fn splats(&self, blood_id: i32, violence: usize) -> &[String] {
        self.row(blood_id, violence).map_or(&[], |r| &r.splats)
    }

    /// The `UnitBlood` row through the censorship table (`GetUnitBloodRecord`, `0x60a390`). The
    /// non-positive guard is defensive: no resolve produces such a key.
    fn row(&self, blood_id: i32, violence: usize) -> Option<&BloodRow> {
        if blood_id <= 0 {
            return None;
        }
        let level = self.levels.get(&(blood_id as u32))?[violence.min(2)];
        if level == 0 {
            return None;
        }
        self.rows.get(&level)
    }

    /// Row counts, levels then rows, for diagnostics.
    pub fn len(&self) -> (usize, usize) {
        (self.levels.len(), self.rows.len())
    }
}

fn unit_blood_levels_schema() -> Schema {
    let mut s = Schema::new("UnitBloodLevels");
    for name in ["ID", "Violence0", "Violence1", "Violence2"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

fn unit_blood_schema() -> Schema {
    let mut s = Schema::new("UnitBlood");
    for name in ["ID", "FrontSmall", "FrontLarge", "BackSmall", "BackLarge"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for i in 0..5 {
        s.add_field(SchemaField::new(
            format!("GroundSplat{i}"),
            FieldType::String,
        ));
    }
    s
}

/// Load both blood tables off the patch chain.
pub fn load_blood_catalog(chain: &mut Chain) -> Result<BloodCatalog> {
    let (levels, default_level) = {
        let bytes = chain
            .read_file(UNIT_BLOOD_LEVELS)
            .with_context(|| format!("reading {UNIT_BLOOD_LEVELS}"))?;
        let rs = parse(&bytes, unit_blood_levels_schema(), "UnitBloodLevels")?;
        // File order matters here alone: the third tier returns the records base, row 0, so its
        // id is read before the rows go into the unordered map.
        let default_level = rs.records().first().and_then(|r| u32_at(r, 0));
        let mut m = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let Some(id) = u32_at(r, 0) {
                m.insert(
                    id,
                    [
                        u32_at(r, 1).unwrap_or(0),
                        u32_at(r, 2).unwrap_or(0),
                        u32_at(r, 3).unwrap_or(0),
                    ],
                );
            }
        }
        (m, default_level)
    };
    let rows = {
        let bytes = chain
            .read_file(UNIT_BLOOD)
            .with_context(|| format!("reading {UNIT_BLOOD}"))?;
        let rs = parse(&bytes, unit_blood_schema(), "UnitBlood")?;
        let mut m = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let Some(id) = u32_at(r, 0) {
                m.insert(
                    id,
                    BloodRow {
                        effects: [
                            u32_at(r, 1).unwrap_or(0),
                            u32_at(r, 2).unwrap_or(0),
                            u32_at(r, 3).unwrap_or(0),
                            u32_at(r, 4).unwrap_or(0),
                        ],
                        // `str_at` drops empty columns: a row keeps only the textures it names.
                        splats: (5..10).filter_map(|i| str_at(&rs, r, i)).collect(),
                    },
                );
            }
        }
        m
    };
    Ok(BloodCatalog {
        levels,
        default_level,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blood 1, the human male's: red at violence 2 (effect 109 is
    /// `Particles\BloodSpurts\BloodSpurt.mdl`), green at 1, nothing at 0.
    #[test]
    fn real_blood_tables_resolve_the_censorship_chain() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let blood = load_blood_catalog(&mut chain).expect("blood tables");

        // Violence 2: all four spurts of row 1, red.
        assert_eq!(
            blood.effect_id(1, 2, true, false),
            Some(109),
            "front small red"
        );
        assert_eq!(
            blood.effect_id(1, 2, true, true),
            Some(164),
            "front large red"
        );
        assert_eq!(
            blood.effect_id(1, 2, false, false),
            Some(534),
            "back small red"
        );
        assert_eq!(
            blood.effect_id(1, 2, false, true),
            Some(55),
            "back large red"
        );
        // Violence 1: censored to green, `UnitBlood` row 2.
        assert_eq!(
            blood.effect_id(1, 1, true, false),
            Some(183),
            "censored green"
        );
        // Violence 0 gives nothing, and so does a non-positive key.
        assert_eq!(blood.effect_id(1, 0, true, false), None);
        assert_eq!(blood.effect_id(-1, 2, true, false), None);
        assert_eq!(blood.effect_id(0, 2, true, false), None);
    }

    /// The three-tier resolve (`0x60afb0`): a `BloodLevel` of 0 and a `BloodID` of -1 both miss,
    /// and the third tier returns the red row, id 1.
    #[test]
    fn the_blood_row_resolve_falls_through_to_the_records_base() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let blood = load_blood_catalog(&mut chain).expect("blood tables");

        // Tier 1 wins when the display carries a real override; tier 2 when only the model does.
        assert_eq!(
            blood.level_key(3, 1),
            Some(3),
            "tier 1: the display override"
        );
        assert_eq!(
            blood.level_key(0, 2),
            Some(2),
            "tier 2: 0 is not a row of the table"
        );
        // Tier 3: `-1` is a miss, not a bloodless marker.
        assert_eq!(
            blood.level_key(0, -1),
            Some(1),
            "tier 3: the records base, id 1 (red)"
        );
        assert_eq!(
            blood.level_key(-1, -1),
            Some(1),
            "tier 3: both tiers negative"
        );
        assert_eq!(
            blood.level_key(99, 99),
            Some(1),
            "tier 3: both tiers out of range"
        );
        // A tier-3 unit spurts.
        assert_eq!(blood.effect_id(1, 2, true, false), Some(109));
    }

    /// Four ground splats per row, never five, coloured by the row the violence level selects.
    #[test]
    fn real_blood_tables_carry_four_ground_splats_per_row() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let blood = load_blood_catalog(&mut chain).expect("blood tables");

        assert_eq!(
            blood.splats(1, 2),
            [
                r"textures\BloodSplats\BloodSplatRed01",
                r"textures\BloodSplats\BloodSplatRed02",
                r"textures\BloodSplats\BloodSplatRed03",
                r"textures\BloodSplats\BloodSplatRed04",
            ],
            "red blood at max violence"
        );
        // Violence 1: green, row 2, as the spurt.
        assert!(blood.splats(1, 1).iter().all(|p| p.contains("Green")));
        assert_eq!(blood.splats(1, 1).len(), 4);
        // Row 3 is black; violence 0 and a non-positive key splat nothing.
        assert!(blood.splats(3, 2).iter().all(|p| p.contains("Black")));
        assert!(blood.splats(1, 0).is_empty());
        assert!(blood.splats(-1, 2).is_empty());
        assert!(blood.splats(0, 2).is_empty());
    }
}
