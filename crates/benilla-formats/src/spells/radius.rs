//! `SpellRadius.dbc`: the yards behind the `$a` token, by `EffectRadiusIndex`
//! ([`crate::spells::SpellDisplay::effect_radius_index`]).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};

/// One `SpellRadius.dbc` row (yards; per-level scaling is 0 on every 5875 row).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpellRadius {
    pub radius: f32,
    pub per_level: f32,
    pub max: f32,
}

/// `SpellRadius.dbc`, by row id.
#[derive(Default)]
pub struct SpellRadiusCatalog {
    rows: HashMap<u32, SpellRadius>,
}

impl SpellRadiusCatalog {
    pub fn get(&self, index: u32) -> Option<&SpellRadius> {
        self.rows.get(&index)
    }

    /// Fixture constructor for tests; the live path is [`load_spell_radii`].
    pub fn from_rows(rows: HashMap<u32, SpellRadius>) -> Self {
        Self { rows }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Load `SpellRadius.dbc` off the patch chain.
pub fn load_spell_radii(chain: &mut Chain) -> Result<SpellRadiusCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\SpellRadius.dbc")
        .context("reading SpellRadius.dbc")?;
    let mut schema = Schema::new("SpellRadius");
    schema.add_field(SchemaField::new("ID", FieldType::UInt32));
    schema.add_field(SchemaField::new("Radius", FieldType::Float32));
    schema.add_field(SchemaField::new("RadiusPerLevel", FieldType::Float32));
    schema.add_field(SchemaField::new("RadiusMax", FieldType::Float32));
    let set = parse(&bytes, schema, "SpellRadius.dbc")?;
    let mut rows = HashMap::new();
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.insert(
            id,
            SpellRadius {
                radius: f32_at(r, 1).unwrap_or(0.0),
                per_level: f32_at(r, 2).unwrap_or(0.0),
                max: f32_at(r, 3).unwrap_or(0.0),
            },
        );
    }
    Ok(SpellRadiusCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Row 13 is Arcane Explosion's 10 yd.
    #[test]
    fn real_spell_radii_read_the_probed_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let radii = load_spell_radii(&mut chain).expect("load SpellRadius");
        assert_eq!(radii.get(13).map(|r| r.radius), Some(10.0));
        assert_eq!(radii.get(8).map(|r| r.radius), Some(5.0));
        assert_eq!(radii.get(10).map(|r| r.radius), Some(30.0));
    }
}
