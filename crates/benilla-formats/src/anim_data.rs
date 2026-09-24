//! `AnimationData.dbc`, the per-animation policy: 208 rows of `ID`, `Name`, `WeaponFlags`,
//! `BodyFlags`, two unidentified columns and `Fallback`. The reference's sheath reconcile
//! (`0x5fdf80`, on every `PlayAnimation`) reads `WeaponFlags` at row `+0x8` of the `0xc0e070` cache
//! for the requested id, not a model's substitute: `4` and `0x10` stow, `0x20` draws melee, and
//! 5875 sets no other bit. `Fallback` is walked only for ids past a model's baked
//! `PlayableAnimationLookup` (203 and up), which was computed from it.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

/// One animation's policy row.
#[derive(Clone, Copy)]
pub struct AnimEntry {
    /// The sheath-reconcile bits (col 2).
    pub weapon_flags: u32,
    /// The substitute animation id when a model lacks this clip (`0` = Stand).
    pub fallback: u16,
}

/// `AnimationData.dbc` id → its policy row.
pub struct AnimDataCatalog {
    rows: HashMap<u16, AnimEntry>,
    /// The Name column (col 1, `"Attack1H"`) for debug readouts, kept apart so [`AnimEntry`]
    /// stays `Copy`.
    names: HashMap<u16, String>,
}

impl AnimDataCatalog {
    /// A catalog of synthetic rows with no names, for tests without an `AnimationData.dbc`.
    pub fn from_rows(rows: impl IntoIterator<Item = (u16, AnimEntry)>) -> Self {
        Self {
            rows: rows.into_iter().collect(),
            names: HashMap::new(),
        }
    }

    /// The WeaponFlags for an animation id, `0` (no policy) for an unknown one.
    pub fn weapon_flags(&self, id: u16) -> u32 {
        self.rows.get(&id).map_or(0, |r| r.weapon_flags)
    }

    /// The fallback for a model lacking clip `id`; `None` for Stand (`0`), which the caller
    /// resolves itself.
    pub fn fallback(&self, id: u16) -> Option<u16> {
        self.rows
            .get(&id)
            .map(|r| r.fallback)
            .filter(|&f| f != 0 && f != id)
    }

    /// The row's Name (`"Run"`); a [`Self::from_rows`] catalog has none.
    pub fn name(&self, id: u16) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("AnimationData");
    for i in 0..7 {
        let ty = if i == 1 {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    s
}

/// Read `AnimationData.dbc` off the patch chain.
pub fn load_anim_data_catalog(chain: &mut Chain) -> Result<AnimDataCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\AnimationData.dbc")
        .context("reading AnimationData.dbc")?;
    let rs = parse(&bytes, schema(), "AnimationData")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    let mut names = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.insert(
            id as u16,
            AnimEntry {
                weapon_flags: u32_at(r, 2).unwrap_or(0),
                fallback: u32_at(r, 6).unwrap_or(0) as u16,
            },
        );
        if let Some(name) = str_at(&rs, r, 1).filter(|n| !n.is_empty()) {
            names.insert(id as u16, name);
        }
    }
    Ok(AnimDataCatalog { rows, names })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 5875 table: WeaponFlags within the reconcile's three bits, spot rows, fallback chains.
    #[test]
    fn real_animation_data_decodes() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_anim_data_catalog(&mut chain).expect("load AnimationData");
        assert_eq!(cat.len(), 208);

        for (&id, row) in &cat.rows {
            assert!(
                matches!(row.weapon_flags, 0 | 4 | 0x10 | 0x14 | 0x20),
                "anim {id}: unexpected WeaponFlags {:#x}",
                row.weapon_flags
            );
        }

        assert_eq!(cat.weapon_flags(42), 4, "Swim force-stows");
        assert_eq!(cat.weapon_flags(91), 4, "Mount force-stows");
        assert_eq!(cat.weapon_flags(102), 4, "SitChairLow force-stows");
        assert_eq!(cat.weapon_flags(60), 0x10, "EmoteTalk force-stows");
        assert_eq!(
            cat.weapon_flags(16),
            0x10,
            "AttackUnarmed needs empty hands"
        );
        assert_eq!(cat.weapon_flags(96), 0x10, "SitGroundDown force-stows");
        assert_eq!(cat.weapon_flags(17), 0x20, "Attack1H force-draws");
        assert_eq!(cat.weapon_flags(26), 0x20, "Ready1H force-draws");
        assert_eq!(cat.weapon_flags(133), 0x20, "FishingCast force-draws");
        assert_eq!(cat.weapon_flags(0), 0, "Stand carries no policy");

        // The ranged-handling trio the reconcile exempts.
        for id in [105, 106, 112] {
            assert_eq!(cat.weapon_flags(id), 0, "Load* anims carry no policy");
        }

        assert_eq!(cat.name(0), Some("Stand"));
        assert_eq!(cat.name(5), Some("Run"));
        assert_eq!(cat.name(17), Some("Attack1H"));
        assert_eq!(cat.name(255), None, "unknown id has no name");

        assert_eq!(cat.fallback(18), Some(17), "Attack2H → Attack1H");
        assert_eq!(cat.fallback(17), Some(16), "Attack1H → AttackUnarmed");
        assert_eq!(cat.fallback(143), Some(5), "Sprint → Run");
        assert_eq!(cat.fallback(187), Some(5), "JumpLandRun → Run");
        assert_eq!(cat.fallback(100), Some(99), "Sleep → SleepDown");
        assert_eq!(cat.fallback(4), None, "Walk falls back to Stand (0 → None)");
    }
}
