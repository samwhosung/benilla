//! The death thud a corpse makes as it lands, fired on the `$DTH` animation event (`0x6236e0`,
//! beside the same event's camera shake `0x625c30`). `DeathThudLookups.dbc` joins the creature's
//! size class (0 Small to 4 Colossal; the display's, else the model's) with the footsteps' terrain
//! sound to a land and a water `SoundEntries` kit, so every creature with a `$DTH` key thuds.
//! `TerrainTypeSounds.dbc` is read for its id domain, `1..=9`, which the reference sizes its baked
//! array by and bounds-checks against (`0x623771`).

use std::collections::{BTreeSet, HashMap};

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

/// `DeathThudLookups.dbc` joined with the `TerrainTypeSounds.dbc` id domain.
pub struct DeathThudCatalog {
    /// `(SizeClass, TerrainTypeSoundID)` → `(land kit, water kit)`.
    lookup: HashMap<(u32, u32), (u32, u32)>,
    /// Every `TerrainTypeSounds.dbc` id, including those no `DeathThudLookups` row names.
    terrain_sounds: BTreeSet<u32>,
}

impl DeathThudCatalog {
    /// The `(land, water)` `SoundEntries` kits for a size class landing on a terrain sound; a
    /// pair with no row is silent, as in the reference.
    pub fn resolve(&self, size_class: u32, terrain_sound: u32) -> Option<(u32, u32)> {
        self.lookup.get(&(size_class, terrain_sound)).copied()
    }

    /// The kit that plays, water in liquid and land otherwise. A `0` is silence, never the other
    /// column: the reference plays the dword it read, and `SoundEntries` has no row 0.
    pub fn kit(&self, size_class: u32, terrain_sound: u32, in_water: bool) -> Option<u32> {
        let (land, water) = self.resolve(size_class, terrain_sound)?;
        let kit = if in_water { water } else { land };
        (kit != 0).then_some(kit)
    }

    /// Every `TerrainTypeSounds` id, ascending.
    pub fn terrain_sounds(&self) -> impl Iterator<Item = u32> + '_ {
        self.terrain_sounds.iter().copied()
    }

    /// Every size class a row names, ascending: `0..=4` in 5875, so the reference's
    /// `sizeClass >= 5` gate (`0x623744`) never fires.
    pub fn size_classes(&self) -> BTreeSet<u32> {
        self.lookup.keys().map(|(sc, _)| *sc).collect()
    }

    pub fn len(&self) -> usize {
        self.lookup.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lookup.is_empty()
    }
}

fn n_u32_schema(name: &str, n: usize) -> Schema {
    let mut s = Schema::new(name);
    for i in 0..n {
        s.add_field(SchemaField::new(format!("f{i}"), FieldType::UInt32));
    }
    s
}

/// Read the two tables off the patch chain.
pub fn load_death_thud_catalog(chain: &mut Chain) -> Result<DeathThudCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\TerrainTypeSounds.dbc")
        .context("reading TerrainTypeSounds.dbc")?;
    let rs = parse(
        &bytes,
        n_u32_schema("TerrainTypeSounds", 1),
        "TerrainTypeSounds",
    )?;
    let terrain_sounds: BTreeSet<u32> = rs.records().iter().filter_map(|r| u32_at(r, 0)).collect();

    let bytes = chain
        .read_file("DBFilesClient\\DeathThudLookups.dbc")
        .context("reading DeathThudLookups.dbc")?;
    let rs = parse(
        &bytes,
        n_u32_schema("DeathThudLookups", 5),
        "DeathThudLookups",
    )?;
    let mut lookup = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(size_class), Some(ts)) = (u32_at(r, 1), u32_at(r, 2)) else {
            continue;
        };
        lookup.insert(
            (size_class, ts),
            (u32_at(r, 3).unwrap_or(0), u32_at(r, 4).unwrap_or(0)),
        );
    }

    Ok(DeathThudCatalog {
        lookup,
        terrain_sounds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 5875 tables; the spot rows pin the column order, as a swapped pair still counts 45.
    #[test]
    fn real_death_thud_chain_resolves() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_death_thud_catalog(&mut chain).expect("load death thud catalog");

        assert_eq!(cat.len(), 45, "all DeathThudLookups rows load");
        assert_eq!(
            cat.terrain_sounds().collect::<Vec<_>>(),
            (1..=9).collect::<Vec<_>>(),
            "TerrainTypeSounds is the bare 1..=9 enum"
        );
        assert_eq!(cat.size_classes(), (0..=4).collect(), "Small..Colossal");

        // Small on Dirt (`DeathThudSmallDirt`), Colossal on Grass (`DeathThudColossalGrass`).
        assert_eq!(cat.resolve(0, 1), Some((907, 1266)));
        assert_eq!(cat.resolve(4, 6), Some((928, 1269)));
        assert_eq!(cat.kit(4, 6, false), Some(928));
        assert_eq!(cat.kit(4, 6, true), Some(1269));

        // Metallic (2) borrows the Stone kits and has no water kit: silent in water.
        assert_eq!(
            cat.resolve(0, 2),
            Some((910, 0)),
            "Small Metallic → Stone kit"
        );
        assert_eq!(cat.kit(0, 2, true), None, "and nothing in water");
        assert_eq!(cat.kit(0, 2, false), Some(910));

        // Terrain sound 0 (`TerrainType` "None") and size class 5 have no rows.
        assert_eq!(cat.resolve(0, 0), None);
        assert_eq!(cat.resolve(5, 1), None);
    }

    /// The reference joins these tables by array index, so every terrain sound a `TerrainType` or
    /// `DeathThudLookups` row names must be a `TerrainTypeSounds` row, or `0`.
    #[test]
    fn the_terrain_axis_agrees_across_the_three_tables() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let thuds = load_death_thud_catalog(&mut chain).expect("load death thud catalog");
        let steps = crate::load_footstep_catalog(&mut chain).expect("load footstep catalog");
        let domain: BTreeSet<u32> = thuds.terrain_sounds().collect();

        for (_, terrain_sound) in thuds.lookup.keys() {
            assert!(
                domain.contains(terrain_sound),
                "DeathThudLookups names terrain sound {terrain_sound}, which TerrainTypeSounds lacks"
            );
        }
        // `TerrainType` 10, "None", is the unauthored default: SoundID 0, so a floor with no
        // material makes no thud.
        for terrain in 0..=10 {
            let sound = steps
                .sound_class_of(terrain)
                .expect("every TerrainType row");
            assert!(
                sound == 0 || domain.contains(&sound),
                "TerrainType {terrain} names terrain sound {sound}"
            );
        }
        assert_eq!(steps.sound_class_of(10), Some(0), r#"TerrainType "None""#);
    }
}
