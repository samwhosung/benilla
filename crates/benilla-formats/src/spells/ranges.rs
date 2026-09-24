//! `SpellRange.dbc` — the `GetMinMaxRange 0x6e3480` inputs a spell's `rangeIndex` column
//! ([`crate::spells::SpellDisplay::range_index`]) resolves against: a min/max yard pair per row,
//! plus a melee-family flag whose branch substitutes the combat-reach sum for the authored pair.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};

/// One `SpellRange.dbc` row — the `GetMinMaxRange 0x6e3480` inputs
/// (min f32 `+0x4`, max f32 `+0x8`, flags `+0xc` with **bit 0 = melee**, whose branch
/// substitutes the combat-reach sum floored at 5.0 for the authored pair). Pinned on the extracted
/// 5875 file (28 records × 22 fields): row 2 = {0, 5, flags 1} (melee), 114 = {8, 35} (Auto Shot),
/// 95 = {8, 25} (Charge), 35 = {0, 35} (Fireball).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpellRange {
    pub min: f32,
    pub max: f32,
    /// Bit 0: the melee range family (combat-reach based, not the authored min/max).
    pub flags: u32,
}

impl SpellRange {
    /// The melee-range family (flags bit 0) — `GetMinMaxRange`'s reach-sum branch.
    pub fn is_melee(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// The pad the melee arm adds to the two reaches — `[0x80b058]`, byte-exact `1.3333334`
/// (`0x3faa_aaab`). The same constant `CanLootNow 0x5ec110` uses inline for the loot/skin reach.
pub const COMBAT_REACH_ADD: f32 = f32::from_bits(0x3faa_aaab);
/// The melee arm's FLOOR — `[0x80a1e8]`. A reach sum *below* it is replaced by it (the "cap 5.0"
/// gloss is inverted; byte-checked at `0x6e35bf`).
pub const MELEE_RANGE_FLOOR: f32 = 5.0;
/// The on-next-swing short-circuit's max — `[0x8118d4]`, `0x6e3504`.
pub const ON_NEXT_SWING_RANGE: f32 = 100.0;

/// `GetMinMaxRange 0x6e3480` — a spell's `{min, max}` cast range, the ONE home for the reach
/// arithmetic every consumer shares (the action bar's out-of-range red, the spell tooltip's range
/// cell). Transcribed from `0x6e3480`, including its 53-bit intermediates: the binary keeps the
/// reach sums on the x87 stack and stores `f32` only at the end, so the additions are `f64` here.
///
/// - **on-next-swing** (`Attributes & 0x404`, `0x6e34fb`): `(0, 100)` — the self-cast
///   short-circuit, before the row is even read.
/// - **melee** (`SpellRange` flags bit 0): `min = 0`; `max = reach + caster_reach + 1.3333334`,
///   replaced by `5.0` when it does not exceed it.
/// - **ranged**: the authored row pair, padded by `caster_reach + reach` when a target resolves —
///   the max unconditionally, the min ONLY when the row's min is already nonzero (the
///   `fcomp`-vs-0.0 guard: a min-0 spell must never grow a min range, or point-blank casts refuse
///   TOO_CLOSE).
///
/// `reach` is the TARGET's when one resolves and the **caster's own** otherwise — the binary
/// doubles the caster's reach rather than assuming a default body (`target_reach.unwrap_or(
/// caster_reach)` in the transcription; benilla defaulted the missing side to a flat `1.5` until
/// this moved here, which diverged for every non-default reach). **A caller with no explicit
/// target still owes the auto-attack one**: the melee arm resolves `[caster+0xc48]`
/// (`attack_target_guid`) itself at `0x6e356a` and uses that unit's reach, which is why the spell
/// tooltip's melee cell moves while you are swinging at something big.
/// Passing it as `target_reach` is how a
/// caller models that; only with neither does the caster's own reach stand in twice.
///
/// Two decomp legs are deliberately UNMODELED, and **neither is target-gated the way an
/// earlier reading here claimed**: the PvP `max += 2.6667` bonus (`0x6e3648`, gated on both
/// units' `[unit+0x118]+0x40 & 0x200d` through the unidentified `0x5fc350`) is reachable from the
/// melee arm even with no target argument, because that arm sets the target register itself; and
/// the `Attributes & 2` leg (`0x6e36aa`) is not an "inspect" arm and touches the target nowhere —
/// it is `max *= RangedModRange(item template +0x118) · 0.01` for whatever sits in
/// EQUIPMENT_SLOT_RANGED, for a player caster.
///
/// **That second one is NOT the data no-op it was twice recorded as**. Scoped by
/// the client's own slot-17 mask (`0x809200` gives bit 17 to InventoryTypes 15/25/26/28 and no
/// others), `item_template.range_mod` is 100 on all 515 bows/guns/crossbows/wands/thrown — but
/// **0 on all 19 shipped RELICS**: every Druid idol, Paladin libram and Shaman totem
/// (InventoryType 28, ordinary player gear) carries a zero there, alongside nine NPC "Monster -"
/// wands and three oddments. The gate never tests the item's class, so a relic in the ranged slot
/// would collapse `max` to 0 and make the caller suppress the **whole** `SPELL_RANGE` cell.
///
/// It stays unmodelled anyway, and for the honest reason rather than the false one: whether a
/// stock client ever *builds* a bit-1 spell's range cell for a relic-wearing player is a question
/// about the builder's callers, not about this function — none of the 192 `Attributes & 2` rows
/// is a spell a relic class learns, so the only route is an addon's `SetHyperlink`. The caller
/// question is open; `ItemInfo::ranged_mod_range` is already on the wire here, so
/// modelling it is a small change the moment it is shown reachable.
///
/// `None` = no range to test: no row, or the self row (id 1, `{0, 0}`).
pub fn min_max_range(
    spell: &crate::spells::SpellDisplay,
    row: Option<&SpellRange>,
    caster_reach: f32,
    target_reach: Option<f32>,
) -> Option<(f32, f32)> {
    if spell.on_next_swing() {
        return Some((0.0, ON_NEXT_SWING_RANGE));
    }
    let row = row?;
    let reach = target_reach.unwrap_or(caster_reach);
    if row.is_melee() {
        let sum = f64::from(reach) + f64::from(caster_reach) + f64::from(COMBAT_REACH_ADD);
        let max = if sum > f64::from(MELEE_RANGE_FLOOR) {
            sum as f32
        } else {
            MELEE_RANGE_FLOOR
        };
        return Some((0.0, max));
    }
    if row.min == 0.0 && row.max == 0.0 {
        return None;
    }
    let Some(target_reach) = target_reach else {
        return Some((row.min, row.max));
    };
    let pad = f64::from(caster_reach) + f64::from(target_reach);
    let min = if row.min == 0.0 {
        0.0
    } else {
        (pad + f64::from(row.min)) as f32
    };
    Some((min, (f64::from(row.max) + pad) as f32))
}

/// `SpellRange.dbc`, by row id ([`SpellDisplay::range_index`]).
#[derive(Default)]
pub struct SpellRangeCatalog {
    ranges: HashMap<u32, SpellRange>,
}

impl SpellRangeCatalog {
    pub fn get(&self, index: u32) -> Option<&SpellRange> {
        self.ranges.get(&index)
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }
}

const SPELL_RANGE: &str = "DBFilesClient\\SpellRange.dbc";
const SPELL_RANGE_FIELDS: usize = 22;

/// Load `SpellRange.dbc` off the patch chain ([`SpellRange`]'s row law).
pub fn load_spell_ranges(chain: &mut Chain) -> Result<SpellRangeCatalog> {
    let bytes = chain
        .read_file(SPELL_RANGE)
        .context("reading SpellRange.dbc")?;
    let mut schema = Schema::new("SpellRange");
    for i in 0..SPELL_RANGE_FIELDS {
        match i {
            1 => schema.add_field(SchemaField::new("MinRange", FieldType::Float32)),
            2 => schema.add_field(SchemaField::new("MaxRange", FieldType::Float32)),
            _ => schema.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32)),
        }
    }
    let set = parse(&bytes, schema, "SpellRange.dbc")?;
    let mut ranges = HashMap::new();
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        ranges.insert(
            id,
            SpellRange {
                min: f32_at(r, 1).unwrap_or(0.0),
                max: f32_at(r, 2).unwrap_or(0.0),
                flags: u32_at(r, 3).unwrap_or(0),
            },
        );
    }
    Ok(SpellRangeCatalog { ranges })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `GetMinMaxRange 0x6e3480` transcription: melee reach floor, the no-target reach
    /// DOUBLING, the ranged reach pad on both bounds, the self-cast short-circuit, and the
    /// rangeless self row.
    #[test]
    fn min_max_range_follows_the_byte_law() {
        let spell = |range_index: u32, attributes: u32| crate::spells::SpellDisplay {
            range_index,
            attributes,
            ..Default::default()
        };
        let melee = SpellRange {
            min: 0.0,
            max: 5.0,
            flags: 1,
        };
        // Two naked-reach units (1.5 + 1.5 + 1.3333334 = 4.333) floor at 5.0…
        let d = spell(2, 0);
        assert_eq!(
            min_max_range(&d, Some(&melee), 1.5, Some(1.5)),
            Some((0.0, MELEE_RANGE_FLOOR))
        );
        // …a big pair (4 + 4 + 1.3333334) exceeds it.
        let (_, max) = min_max_range(&d, Some(&melee), 4.0, Some(4.0)).unwrap();
        assert!((max - 9.3333).abs() < 1e-3);
        // With NO target the caster's own reach stands in for the missing side — the binary
        // doubles it, so a 4.0-reach caster reads the same 9.333 alone as it does against an
        // equal-reach target. (A flat 1.5 fallback would have said 6.833.)
        let (_, max) = min_max_range(&d, Some(&melee), 4.0, None).unwrap();
        assert!((max - 9.3333).abs() < 1e-3);

        // Charge's 8–25 row pads both bounds by the BARE reach sum (no 1.3333 — melee-only)
        // against a unit target.
        let charge_row = SpellRange {
            min: 8.0,
            max: 25.0,
            flags: 0,
        };
        let (min, max) = min_max_range(&d, Some(&charge_row), 1.5, Some(1.5)).unwrap();
        assert!((min - (8.0 + 3.0)).abs() < 1e-3);
        assert!((max - (25.0 + 3.0)).abs() < 1e-3);

        // A min-0 row (Fireball's 0–35) pads the max only — the fcomp-vs-0.0 guard keeps the
        // min at zero, so a point-blank cast never reads a min range.
        let fireball_row = SpellRange {
            min: 0.0,
            max: 35.0,
            flags: 0,
        };
        let (min, max) = min_max_range(&d, Some(&fireball_row), 1.5, Some(1.5)).unwrap();
        assert_eq!(min, 0.0);
        assert!((max - 38.0).abs() < 1e-3);

        // No unit target: the row's raw bounds, unpadded — the shape the spell tooltip's own
        // call (`target = NULL` at `0x52e9c2`) always takes.
        assert_eq!(
            min_max_range(&d, Some(&charge_row), 1.5, None),
            Some((8.0, 25.0))
        );

        // The self-cast attribute short-circuits to a flat 100 without touching the row.
        assert_eq!(
            min_max_range(&spell(1, 0x400), None, 1.5, None),
            Some((0.0, ON_NEXT_SWING_RANGE))
        );

        // The self row (0, 0, no melee flag) resolves to no range at all.
        let self_row = SpellRange {
            min: 0.0,
            max: 0.0,
            flags: 0,
        };
        assert_eq!(min_max_range(&d, Some(&self_row), 1.5, None), None);
    }

    /// `SpellRange.dbc` on the real data — the byte law's own probe rows (`GetMinMaxRange
    /// 0x6e3480`): row 2 is the melee family (flags bit 0), 114 = Auto Shot's 8–35, 95 =
    /// Charge's 8–25. Skips without client data.
    #[test]
    fn real_spell_ranges_read_the_byte_laws_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let ranges = load_spell_ranges(&mut chain).expect("load SpellRange");

        let melee = ranges.get(2).expect("row 2");
        assert_eq!((melee.min, melee.max), (0.0, 5.0));
        assert!(melee.is_melee(), "row 2 carries the melee flag");

        let auto_shot = ranges.get(114).expect("row 114");
        assert_eq!((auto_shot.min, auto_shot.max), (8.0, 35.0));
        assert!(!auto_shot.is_melee());

        let charge = ranges.get(95).expect("row 95");
        assert_eq!((charge.min, charge.max), (8.0, 25.0));

        // A min-0 nuke row reads a true 0.0 min (row 4: Shadow Bolt, Frostbolt, wand Shoot) —
        // the fcomp-vs-0.0 guard's input, so the min/max field mapping can't silently shift
        // (a manufactured min range refused point-blank casts).
        let nuke = ranges.get(4).expect("row 4");
        assert_eq!((nuke.min, nuke.max), (0.0, 30.0));
        assert!(!nuke.is_melee());
    }
}
