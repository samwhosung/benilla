//! `SheatheSoundLookups.dbc` — the draw/stow sound table (the client's runtime
//! `AUSHEATHSOUNDHASH`, its entry alloc `0x45d3a0`).
//!
//! Layout — VERIFIED against build 5875 (header + full dump with SoundEntries name joins,
//! 2026-07-03): **33 × 7 × 28 B**: `ID(0), ItemClass(1), ItemSubclass(2), Material(3),
//! _col4(4), SheatheSound(5), UnsheatheSound(6)`. The data forms three families:
//! shields (class 4, subclasses 5/6, material 0 → kits 696/701 `SHEATHINGSHIELD*`), metal
//! weapons (class 2 × subclasses 0–14, material 1 → 698/700 `SHEATHINGMETALWEAPON*`) and wood
//! weapons (material 2 → 697/699 `SHEATHINGWOODWEAPON*`), plus one material-0 weapon default
//! row (→ wood). Column 4 is 0 on shield rows and 1 on weapon rows — meaning unrecorded,
//! unused here.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

/// One row's kit pair.
#[derive(Clone, Copy)]
pub struct SheatheSounds {
    /// Stowing the item (SoundEntries kit).
    pub sheathe: u32,
    /// Drawing it.
    pub unsheathe: u32,
}

/// `(ItemClass, ItemSubclass, Material)` → the kit pair.
pub struct SheatheSoundCatalog {
    rows: HashMap<(u32, u32, u32), SheatheSounds>,
}

impl SheatheSoundCatalog {
    /// The kit pair for an item. **`material` is the load-bearing argument** — the weapon rows
    /// agree within a material and differ across one, so the class/subclass only choose the
    /// *family* (weapon vs shield). Callers pass the item's real `Material` off the wire; it is
    /// never inferred from the subclass, which carries no material information.
    ///
    /// Falls back to any material carried for that `(class, subclass)` — this is what lands a
    /// shield, whose own rows hold material 0 as a don't-care, on the shield pair — and then to
    /// the class's subclass-0 row: the 5875 table only covers weapon subclasses 0–14, so daggers
    /// (15) and other stragglers ring like a generic weapon of their material (INTERIM — the
    /// client's hash-miss behavior is unrecorded).
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

    /// The real 5875 table: a metal 1H sword (2/7/1) draws with 700 and stows with 698; a
    /// shield (4/6) hits the shield pair; the material fallback resolves an unknown material.
    /// Skips without client data.
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

        // Daggers (15) have NO row in 5875 — they fall back to the class's subclass-0 metal
        // pair (and an unlisted material to the material chain).
        let dagger = cat.get(2, 15, 1).expect("dagger falls back to subclass 0");
        assert_eq!((dagger.sheathe, dagger.unsheathe), (698, 700));
        assert!(cat.get(2, 15, 7).is_some(), "weird material still resolves");
    }

    /// **The material is the whole key; the subclass is inert**. Every weapon row
    /// of a material carries the same kit pair, so a bow (subclass 2, sitting in the middle of the
    /// weapon range) rings by what it is *made of* — and the reference client's own `itemcache.wdb`
    /// puts every bow, crossbow and wand at material 2 (wood) while guns and thrown are metal.
    /// Ringing a bow on the metal pair — which a "staves and polearms are the wooden ones" guess
    /// does — is the director-reported wrong sound. Skips without client data.
    #[test]
    fn the_kit_pair_is_decided_by_material_alone() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_sheathe_sound_catalog(&mut chain).expect("load sheathe sounds");

        // A bow is wood, and wood rings 697/699 — never the metal 698/700 pair.
        let bow = cat.get(2, 2, 2).expect("wood bow");
        assert_eq!((bow.sheathe, bow.unsheathe), (697, 699));

        // The same subclass at the other material is the metal pair: subclass carries no
        // information here, which is exactly why it must not be used to infer the material.
        let metal = cat.get(2, 2, 1).expect("metal row for the same subclass");
        assert_eq!((metal.sheathe, metal.unsheathe), (698, 700));

        // …and that holds across every weapon subclass the table covers.
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

        // A shield's real material is whatever its armor row says (1 metal and 6 plate both occur
        // in the reference cache); its DBC rows carry material 0 as a don't-care, and the
        // fallback chain lands them on the shield pair regardless.
        for material in [0, 1, 6] {
            let shield = cat
                .get(4, 6, material)
                .expect("shield resolves at any material");
            assert_eq!((shield.sheathe, shield.unsheathe), (696, 701));
        }
    }
}
