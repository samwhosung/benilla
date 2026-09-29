//! `SpellCastTimes.dbc`: the cast time a spell's `CastingTimeIndex`
//! ([`crate::spells::SpellDisplay::casting_time_index`]) resolves to in `Spell_C::GetCastTime`
//! (`0x6e3340`), before spell-mod op `0xa` (`SPELLMOD_CASTING_TIME`). Row 1, `{0, 0, 0}`, is the
//! instant sentinel every spell without a cast time points at.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, u32_at};

/// One `SpellCastTimes.dbc` row, in ms. All three columns are signed: row 18, the hunter shots',
/// is `{-1000000, 0, -1000000}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpellCastTime {
    /// The cast time at the spell's `BaseLevel`; 0 is instant.
    pub base_ms: i32,
    /// Added per caster level above the spell's `BaseLevel`; negative on a few rows (row 10).
    pub per_level_ms: i32,
    /// The floor the level-scaled cast time is raised to.
    pub minimum_ms: i32,
}

impl SpellCastTime {
    /// The level-scaled cast time (`0x6e3395`-`0x6e33b4`): `base + perLevel·(casterLevel −
    /// baseLevel)`, the level delta signed, raised to the row minimum and nothing else, so row 18
    /// stays negative. `base_level` is the DBC's `baseLevel`
    /// ([`crate::spells::SpellDisplay::base_level`]), not `spellLevel`. Spell-mod op `0xa` and the
    /// clamp at zero are the caller's.
    pub fn resolved_ms(&self, caster_level: u32, base_level: u32) -> i32 {
        let delta = (caster_level as i32).wrapping_sub(base_level as i32);
        let scaled = self
            .base_ms
            .wrapping_add(self.per_level_ms.wrapping_mul(delta));
        scaled.max(self.minimum_ms)
    }
}

impl SpellCastTime {
    /// Whether the caster's level moves [`Self::resolved_ms`]: only through the per-level column.
    pub fn reads_caster_level(&self) -> bool {
        self.per_level_ms != 0
    }
}

/// `SpellCastTimes.dbc`, by row id ([`crate::spells::SpellDisplay::casting_time_index`]).
#[derive(Default)]
pub struct SpellCastTimeCatalog {
    times: HashMap<u32, SpellCastTime>,
}

impl SpellCastTimeCatalog {
    /// A catalog of the given rows, for callers' tests.
    pub fn from_rows(rows: impl IntoIterator<Item = (u32, SpellCastTime)>) -> Self {
        SpellCastTimeCatalog {
            times: rows.into_iter().collect(),
        }
    }

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
                base_ms: i32_at(r, 1).unwrap_or(0),
                per_level_ms: i32_at(r, 2).unwrap_or(0),
                minimum_ms: i32_at(r, 3).unwrap_or(0),
            },
        );
    }
    Ok(SpellCastTimeCatalog { times })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row reads the caster's level when some level changes its answer, which the per-level
    /// column decides for every row shape the data has.
    #[test]
    fn a_row_reads_the_caster_level_only_through_its_per_level_column() {
        for row in [
            SpellCastTime {
                base_ms: 0,
                per_level_ms: 0,
                minimum_ms: 0,
            },
            SpellCastTime {
                base_ms: 1500,
                per_level_ms: 0,
                minimum_ms: 1500,
            },
            SpellCastTime {
                base_ms: -1_000_000,
                per_level_ms: 0,
                minimum_ms: -1_000_000,
            },
            SpellCastTime {
                base_ms: 1000,
                per_level_ms: -100,
                minimum_ms: 500,
            },
            SpellCastTime {
                base_ms: 1000,
                per_level_ms: 50,
                minimum_ms: 0,
            },
        ] {
            for base_level in [0, 10] {
                let moves = (0..=60).any(|level| {
                    row.resolved_ms(level, base_level) != row.resolved_ms(level + 1, base_level)
                });
                assert_eq!(
                    row.reads_caster_level(),
                    moves,
                    "{row:?} at base {base_level}"
                );
            }
        }
    }

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

        // Below the spell's level the delta goes negative and the cast lengthens.
        assert_eq!(scaling.resolved_ms(8, 10), 1200);

        // Row 18: a negative base under a negative minimum stays negative.
        let shots = SpellCastTime {
            base_ms: -1_000_000,
            per_level_ms: 0,
            minimum_ms: -1_000_000,
        };
        assert_eq!(shots.resolved_ms(60, 1), -1_000_000);
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
