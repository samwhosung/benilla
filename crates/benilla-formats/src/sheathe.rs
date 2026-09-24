//! `SheatheSoundLookups.dbc`, the draw and stow sounds (the reference's `AUSHEATHSOUNDHASH`, entry
//! alloc `0x45d3a0`): `ID`, `ItemClass`, `ItemSubclass`, `Material`, an unidentified column (0 on
//! shields, 1 on weapons), `SheatheSound`, `UnsheatheSound`. Three families: shields (class 4,
//! material 0), metal weapons (class 2, material 1) and wood weapons (material 2), plus a
//! material-0 weapon row that rings wood.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

/// One row's kit pair.
#[derive(Clone, Copy)]
pub struct SheatheSounds {
    /// The `SoundEntries` kit for stowing the item.
    pub sheathe: u32,
    /// Drawing it.
    pub unsheathe: u32,
}

/// `(ItemClass, ItemSubclass, Material)` → the kit pair.
pub struct SheatheSoundCatalog {
    rows: HashMap<(u32, u32, u32), SheatheSounds>,
}

impl SheatheSoundCatalog {
    /// The kit pair for an item, decided by its real `Material` off the wire (never one guessed
    /// from the subclass), the class choosing only weapon or shield. A missing material falls back
    /// to any row of the `(class, subclass)`, landing a shield on its material-0 rows, then to the
    /// class's subclass 0, as the table covers weapon subclasses 0-14: a dagger (15) rings as a
    /// weapon of its material. The reference's behaviour on a miss is untraced.
    pub fn get(&self, class: u32, subclass: u32, material: u32) -> Option<SheatheSounds> {
        [subclass, 0]
            .iter()
            .find_map(|&sc| {
                [material, 1, 2, 0]
                    .iter()
                    .find_map(|&m| self.rows.get(&(class, sc, m)))
            })
            .copied()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("SheatheSoundLookups");
    for i in 0..7 {
        s.add_field(SchemaField::new(format!("f{i}"), FieldType::UInt32));
    }
    s
}

/// Read `SheatheSoundLookups.dbc` off the patch chain.
pub fn load_sheathe_sound_catalog(chain: &mut Chain) -> Result<SheatheSoundCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\SheatheSoundLookups.dbc")
        .context("reading SheatheSoundLookups.dbc")?;
    let rs = parse(&bytes, schema(), "SheatheSoundLookups")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let g = |i: usize| u32_at(r, i).unwrap_or(0);
        rows.insert(
            (g(1), g(2), g(3)),
            SheatheSounds {
                sheathe: g(5),
                unsheathe: g(6),
            },
        );
    }
    Ok(SheatheSoundCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 5875 table: a metal sword (2/7/1), a wood staff, a shield (4/6) and the fallbacks.
    #[test]
    fn real_sheathe_sounds_decode() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_sheathe_sound_catalog(&mut chain).expect("load sheathe sounds");
        assert_eq!(cat.len(), 33);

        let sword = cat.get(2, 7, 1).expect("metal 1H sword");
        assert_eq!((sword.sheathe, sword.unsheathe), (698, 700));

        let staff_wood = cat.get(2, 10, 2).expect("wood staff");
        assert_eq!((staff_wood.sheathe, staff_wood.unsheathe), (697, 699));

        let shield = cat.get(4, 6, 0).expect("shield");
        assert_eq!((shield.sheathe, shield.unsheathe), (696, 701));

        // Daggers (15) have no row and fall back to subclass 0.
        let dagger = cat.get(2, 15, 1).expect("dagger falls back to subclass 0");
        assert_eq!((dagger.sheathe, dagger.unsheathe), (698, 700));
        assert!(cat.get(2, 15, 7).is_some(), "weird material still resolves");
    }

    /// Every weapon row of a material carries the same pair, so a bow rings by what it is made of:
    /// the reference's item cache has bows, crossbows and wands at material 2 (wood), guns and
    /// thrown at metal.
    #[test]
    fn the_kit_pair_is_decided_by_material_alone() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_sheathe_sound_catalog(&mut chain).expect("load sheathe sounds");

        let bow = cat.get(2, 2, 2).expect("wood bow");
        assert_eq!((bow.sheathe, bow.unsheathe), (697, 699));

        let metal = cat.get(2, 2, 1).expect("metal row for the same subclass");
        assert_eq!((metal.sheathe, metal.unsheathe), (698, 700));

        for subclass in 0..=14 {
            assert_eq!(
                cat.get(2, subclass, 2).map(|p| (p.sheathe, p.unsheathe)),
                Some((697, 699)),
                "subclass {subclass} at material 2 must ring wood"
            );
            assert_eq!(
                cat.get(2, subclass, 1).map(|p| (p.sheathe, p.unsheathe)),
                Some((698, 700)),
                "subclass {subclass} at material 1 must ring metal"
            );
        }

        // Shields come as metal (1) or plate (6) in the reference's item cache.
        for material in [0, 1, 6] {
            let shield = cat
                .get(4, 6, material)
                .expect("shield resolves at any material");
            assert_eq!((shield.sheathe, shield.unsheathe), (696, 701));
        }
    }
}
