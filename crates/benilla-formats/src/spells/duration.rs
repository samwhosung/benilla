//! `SpellDuration.dbc` — the base/per-level/max duration (ms) a spell's `DurationIndex` column
//! ([`crate::spells::SpellDisplay::duration_index`]) resolves against. Feeds the duration
//! formula — `Spell_C::GetDuration 0x6ea000`: it reads DurationIndex
//! `[SpellRec+0x78]`, resolves this table's recordsById at `[0xc0d828]`, and applies spell-mod
//! op `1` (SPELLMOD_DURATION). The row layout is settled independently (WoWDBDefs'
//! `1.0.0.3980`–`1.12.3.6141` layout `$id$ID<32> Duration<32> DurationPerLevel<32>
//! MaxDuration<32>`, covering build 5875 — cross-checked against the real extracted file below).
//!
//! **Row layout** — pinned on the extracted 5875 file (82 records × 4 fields, 16 B/record): `ID(0)`,
//! `Duration(1)` = the ms a flat 1.12 tooltip shows, `DurationPerLevel(2)`, `MaxDuration(3)`. All
//! three are **signed**: row 21 = `{-1, 0, -1}` — the client's "permanent, until cancelled"
//! sentinel (a stance/passive-style aura with no timer; [`SpellDuration::is_permanent`]) — and a
//! few rows (e.g. 427) carry a negative base with a nonzero per-level term for level-scaling
//! formulas this crate doesn't evaluate; the raw triple is carried faithfully. Row 30 =
//! `{1_800_000, 0, 1_800_000}` — Frost Armor (spell 168)'s real 30-minute duration, cross-checked
//! end-to-end against the local vmangos `spell_template` (`durationIndex` 30 for entry 168).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, u32_at};

/// One `SpellDuration.dbc` row (module doc's row law).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpellDuration {
    /// The duration a flat (level-independent) tooltip render shows, ms. `-1` = permanent
    /// ([`Self::is_permanent`]).
    pub base_ms: i32,
    /// Signed ms added per caster level above the spell's `BaseLevel`; `0` for most rows.
    pub per_level_ms: i32,
    /// The ceiling the level-scaled duration clamps to.
    pub max_ms: i32,
}

impl SpellDuration {
    /// The client's "permanent, no timer" sentinel (`base_ms == -1`) — a stance or passive-style
    /// aura that lasts until cancelled, not a countdown.
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
    /// Test-only seeding (the token engine's unit tests build tiny catalogs).
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

/// Load `SpellDuration.dbc` off the patch chain ([`SpellDuration`]'s row law).
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

    /// `SpellDuration.dbc` on the real data — the module doc's own probe rows. Skips without
    /// client data.
    #[test]
    fn real_spell_durations_read_the_probed_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let durations = load_spell_durations(&mut chain).expect("load SpellDuration");

        // Row 30: Frost Armor's real 30-minute duration (vmangos spell_template.durationIndex==30
        // for entry 168).
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

        // Row 427 is the reason [`SpellDuration::is_permanent`]'s one-field test is not obviously
        // safe: it carries a NEGATIVE base (`-600_000`) with a positive per-level term, and the
        // client's own permanence test is the two-field `Duration < 0 && DurationPerLevel <= 0`
        // (byte-verified at `0x4e456e`-`0x4e457a`, the buff cache's `untilCancelled` derivation —
        // see `benilla::ui_aura`). Row 427 must therefore read NOT permanent under both.
        let scaling = durations.get(427).expect("row 427");
        assert_eq!((scaling.base_ms, scaling.per_level_ms), (-600_000, 60_000));
        assert!(!scaling.is_permanent());
    }

    /// The shared helper against the **client's own** predicate, over every shipped row.
    ///
    /// `is_permanent()` tests one field (`base_ms == -1`); the binary tests two
    /// (`Duration < 0 && DurationPerLevel <= 0`, `0x4e456e`-`0x4e457a`). Those are different
    /// functions in general — a row like `{-5, 0}` would split them — so "they agree" is a fact
    /// about the shipped 5875 data, not a theorem, and it is exactly the kind of fact that a data
    /// change would silently invalidate. Assert it on the real file rather than assume it.
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
