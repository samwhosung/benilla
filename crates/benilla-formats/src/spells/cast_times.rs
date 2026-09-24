//! `SpellCastTimes.dbc`: the cast time a spell's `CastingTimeIndex`
//! ([`crate::spells::SpellDisplay::casting_time_index`]) resolves to in `Spell_C::GetCastTime`
//! (`0x6e3340`), before spell-mod op `0xa` (`SPELLMOD_CASTING_TIME`). Row 1, `{0, 0, 0}`, is the
//! instant sentinel every spell without a cast time points at.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, u32_at};

/// One `SpellCastTimes.dbc` row, in ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpellCastTime {
    /// The cast time a level-independent tooltip shows; 0 is instant.
    pub base_ms: u32,
    /// Added per caster level above the spell's `BaseLevel`; negative on a few rows (row 10).
    pub per_level_ms: i32,
    /// The floor the level-scaled cast time clamps to.
    pub minimum_ms: u32,
}

impl SpellCastTime {
    /// The level-scaled cast time (`0x6e3340`): `base + perLevel·(casterLevel − baseLevel)`,
    /// floored to the row minimum and to zero. `base_level` is the DBC's `baseLevel`
    /// ([`crate::spells::SpellDisplay::base_level`]), not `spellLevel`. Spell-mod op `0xa` is not
    /// applied.
    pub fn resolved_ms(&self, caster_level: u32, base_level: u32) -> u32 {
        let delta = i64::from(caster_level.saturating_sub(base_level));
        let scaled = i64::from(self.base_ms) + i64::from(self.per_level_ms) * delta;
        scaled.max(i64::from(self.minimum_ms)).max(0) as u32
    }
}

/// `SpellCastTimes.dbc`, by row id ([`crate::spells::SpellDisplay::casting_time_index`]).
#[derive(Default)]
pub struct SpellCastTimeCatalog {
    times: HashMap<u32, SpellCastTime>,
}

impl SpellCastTimeCatalog {
    pub fn get(&self, index: u32) -> Option<&SpellCastTime> {
        self.times.get(&index)
    }

    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }
}

const SPELL_CAST_TIMES: &str = "DBFilesClient\\SpellCastTimes.dbc";
const SPELL_CAST_TIMES_FIELDS: usize = 4;

/// Load `SpellCastTimes.dbc` off the patch chain.
pub fn load_spell_cast_times(chain: &mut Chain) -> Result<SpellCastTimeCatalog> {
    let bytes = chain
        .read_file(SPELL_CAST_TIMES)
        .context("reading SpellCastTimes.dbc")?;
    let mut schema = Schema::new("SpellCastTimes");
    for i in 0..SPELL_CAST_TIMES_FIELDS {
        match i {
            2 => schema.add_field(SchemaField::new("PerLevel", FieldType::Int32)),
            _ => schema.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32)),
        }
    }
    let set = parse(&bytes, schema, "SpellCastTimes.dbc")?;
    let mut times = HashMap::new();
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        times.insert(
            id,
            SpellCastTime {
                base_ms: u32_at(r, 1).unwrap_or(0),
                per_level_ms: i32_at(r, 2).unwrap_or(0),
                minimum_ms: u32_at(r, 3).unwrap_or(0),
            },
        );
    }
    Ok(SpellCastTimeCatalog { times })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_ms_scales_and_floors() {
        let instant = SpellCastTime {
            base_ms: 0,
            per_level_ms: 0,
            minimum_ms: 0,
        };
        assert_eq!(instant.resolved_ms(60, 0), 0);

        let fireball = SpellCastTime {
            base_ms: 1500,
            per_level_ms: 0,
            minimum_ms: 1500,
        };
        assert_eq!(fireball.resolved_ms(60, 1), 1500);

        // Row 10's shape: 100 ms less per level above the spell's, floored at 500.
        let scaling = SpellCastTime {
            base_ms: 1000,
            per_level_ms: -100,
            minimum_ms: 500,
        };
        assert_eq!(scaling.resolved_ms(10, 10), 1000);
        assert_eq!(scaling.resolved_ms(13, 10), 700);
        assert_eq!(scaling.resolved_ms(60, 10), 500, "the minimum floor");

        // A hypothetical floor-less shrink clamps at zero rather than going negative.
        let floorless = SpellCastTime {
            base_ms: 100,
            per_level_ms: -100,
            minimum_ms: 0,
        };
        assert_eq!(floorless.resolved_ms(60, 1), 0);
    }

    #[test]
    fn real_spell_cast_times_read_the_probed_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let times = load_spell_cast_times(&mut chain).expect("load SpellCastTimes");

        // Row 1: the instant sentinel.
        let instant = times.get(1).expect("row 1");
        assert_eq!(
            (instant.base_ms, instant.per_level_ms, instant.minimum_ms),
            (0, 0, 0)
        );

        // Row 16: Fireball rank 1, spell 133 (vmangos `spell_template.castingTimeIndex`).
        let fireball = times.get(16).expect("row 16");
        assert_eq!(fireball.base_ms, 1500);

        // Row 10: a level-scaling row with a negative per-level term.
        let scaling = times.get(10).expect("row 10");
        assert_eq!(
            (scaling.base_ms, scaling.per_level_ms, scaling.minimum_ms),
            (1000, -100, 500)
        );

        assert_eq!(times.len(), 52, "5875 ships 52 SpellCastTimes rows");
    }
}
