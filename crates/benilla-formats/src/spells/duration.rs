//! `SpellDuration.dbc`: the duration a spell's `DurationIndex`
//! ([`crate::spells::SpellDisplay::duration_index`]) resolves to in `Spell_C::GetSpellDuration`
//! (`0x6ea000`), before spell-mod op 1 (`SPELLMOD_DURATION`). All three columns are signed: a few
//! rows (427) pair a negative base with a per-level term this crate does not evaluate.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, u32_at};

/// One `SpellDuration.dbc` row, in ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpellDuration {
    /// The duration a level-independent tooltip shows; -1 is permanent.
    pub base_ms: i32,
    /// Added per caster level above the spell's `BaseLevel`; 0 for most rows.
    pub per_level_ms: i32,
    /// The ceiling the level-scaled duration clamps to.
    pub max_ms: i32,
}

impl SpellDuration {
    /// The permanent sentinel, `base_ms == -1`; the client tests `Duration < 0` and
    /// `DurationPerLevel <= 0` (`0x4e456e`-`0x4e457a`), which agrees on the shipped rows only.
    pub fn is_permanent(&self) -> bool {
        self.base_ms == -1
    }
}

/// `SpellDuration.dbc`, by row id ([`crate::spells::SpellDisplay::duration_index`]).
#[derive(Default)]
pub struct SpellDurationCatalog {
    durations: HashMap<u32, SpellDuration>,
}

impl SpellDurationCatalog {
    /// Test-only seeding for the token engine's tests.
    #[cfg(test)]
    pub(crate) fn insert_for_tests(&mut self, index: u32, base_ms: i32) {
        self.durations.insert(
            index,
            SpellDuration {
                base_ms,
                per_level_ms: 0,
                max_ms: base_ms,
            },
        );
    }

    pub fn get(&self, index: u32) -> Option<&SpellDuration> {
        self.durations.get(&index)
    }

    pub fn len(&self) -> usize {
        self.durations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.durations.is_empty()
    }
}

const SPELL_DURATION: &str = "DBFilesClient\\SpellDuration.dbc";
const SPELL_DURATION_FIELDS: usize = 4;

/// Load `SpellDuration.dbc` off the patch chain.
pub fn load_spell_durations(chain: &mut Chain) -> Result<SpellDurationCatalog> {
    let bytes = chain
        .read_file(SPELL_DURATION)
        .context("reading SpellDuration.dbc")?;
    let mut schema = Schema::new("SpellDuration");
    for i in 0..SPELL_DURATION_FIELDS {
        if i == 0 {
            schema.add_field(SchemaField::new("ID", FieldType::UInt32));
        } else {
            schema.add_field(SchemaField::new(format!("F{i}"), FieldType::Int32));
        }
    }
    let set = parse(&bytes, schema, "SpellDuration.dbc")?;
    let mut durations = HashMap::new();
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        durations.insert(
            id,
            SpellDuration {
                base_ms: i32_at(r, 1).unwrap_or(0),
                per_level_ms: i32_at(r, 2).unwrap_or(0),
                max_ms: i32_at(r, 3).unwrap_or(0),
            },
        );
    }
    Ok(SpellDurationCatalog { durations })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_spell_durations_read_the_probed_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let durations = load_spell_durations(&mut chain).expect("load SpellDuration");

        // Row 30: Frost Armor, spell 168 (vmangos `spell_template.durationIndex`).
        let frost_armor = durations.get(30).expect("row 30");
        assert_eq!(
            (
                frost_armor.base_ms,
                frost_armor.per_level_ms,
                frost_armor.max_ms
            ),
            (1_800_000, 0, 1_800_000)
        );
        assert!(!frost_armor.is_permanent());

        // Row 21: the permanent sentinel.
        let permanent = durations.get(21).expect("row 21");
        assert_eq!(permanent.base_ms, -1);
        assert!(permanent.is_permanent());

        // Row 1: a short, ordinary duration.
        let short = durations.get(1).expect("row 1");
        assert_eq!((short.base_ms, short.per_level_ms), (10_000, 0));

        assert_eq!(durations.len(), 82, "5875 ships 82 SpellDuration rows");

        // Row 427: a negative base with a positive per-level term, permanent under neither test.
        let scaling = durations.get(427).expect("row 427");
        assert_eq!((scaling.base_ms, scaling.per_level_ms), (-600_000, 60_000));
        assert!(!scaling.is_permanent());
    }

    #[test]
    fn is_permanent_matches_the_clients_two_field_test_on_every_shipped_row() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let durations = load_spell_durations(&mut chain).expect("load SpellDuration");

        let disagree: Vec<_> = durations
            .durations
            .iter()
            .filter(|(_, d)| d.is_permanent() != (d.base_ms < 0 && d.per_level_ms <= 0))
            .map(|(id, d)| (*id, d.base_ms, d.per_level_ms))
            .collect();
        assert!(
            disagree.is_empty(),
            "rows where base_ms == -1 disagrees with the client's \
             (Duration < 0 && DurationPerLevel <= 0): {disagree:?}"
        );
    }
}
