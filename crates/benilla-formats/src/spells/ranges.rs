//! `SpellRange.dbc`, by `rangeIndex` ([`crate::spells::SpellDisplay::range_index`]), and
//! `GetMinMaxRange` (`0x6e3480`): a spell's cast range in yards.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};

/// One `SpellRange.dbc` row, in yards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpellRange {
    pub min: f32,
    pub max: f32,
    /// Bit 0: the melee range family (combat-reach based, not the authored min/max).
    pub flags: u32,
}

impl SpellRange {
    /// Flags bit 0: `GetMinMaxRange` takes the reach sum instead of the row's pair.
    pub fn is_melee(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// The pad the melee arm adds to the two reaches, `[0x80b058]`; `CanLootNow` (`0x5ec110`) pads the
/// loot reach by the same value.
pub const COMBAT_REACH_ADD: f32 = f32::from_bits(0x3faa_aaab);
/// The melee arm's floor, not a cap, `[0x80a1e8]`: a lower reach sum is raised to it (`0x6e35bf`).
pub const MELEE_RANGE_FLOOR: f32 = 5.0;
/// The on-next-swing short-circuit's max, `[0x8118d4]` (`0x6e3504`).
pub const ON_NEXT_SWING_RANGE: f32 = 100.0;

/// `GetMinMaxRange` (`0x6e3480`): a spell's `{min, max}` cast range, summed in `f64` because the
/// client keeps the reach sums on the x87 stack and stores `f32` only at the end. On-next-swing
/// spells (`Attributes & 0x404`, `0x6e34fb`) short-circuit to `(0, 100)`. The melee arm sums the
/// target's reach, else the caster's own a second time, and reads the auto-attack target itself
/// (`attack_target_guid`, `0x6e356a`), so a caller with no explicit target passes that unit's
/// reach. Given a target, the ranged arm pads the max, and the min only when nonzero (the
/// `fcomp`-vs-0.0 guard), so a min-0 spell never refuses `TOO_CLOSE`. `None`: no row, or the self
/// row (id 1, `{0, 0}`).
///
/// Not applied: the PvP `max += 2.6667` (`0x6e3648`, gated by `0x5fc350`), and for a player the
/// `Attributes & 2` scale (`0x6e36aa`), `max *= RangedModRange · 0.01` of the ranged-slot item,
/// which a relic (range mod 0, InventoryType 28, admitted by `0x809200`) would zero, though no
/// spell a relic class learns carries the bit.
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

/// `SpellRange.dbc`, by row id ([`crate::spells::SpellDisplay::range_index`]).
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

/// Load `SpellRange.dbc` off the patch chain.
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
        // Two 1.5-reach units (4.333) floor at 5.0,
        let d = spell(2, 0);
        assert_eq!(
            min_max_range(&d, Some(&melee), 1.5, Some(1.5)),
            Some((0.0, MELEE_RANGE_FLOOR))
        );
        // a 4.0 pair (9.333) clears it.
        let (_, max) = min_max_range(&d, Some(&melee), 4.0, Some(4.0)).unwrap();
        assert!((max - 9.3333).abs() < 1e-3);
        // With no target the caster's reach counts twice, as in the client: 9.333 alone too.
        let (_, max) = min_max_range(&d, Some(&melee), 4.0, None).unwrap();
        assert!((max - 9.3333).abs() < 1e-3);

        // Charge's 8-25 row pads both bounds by the bare reach sum; the 1.3333 is melee-only.
        let charge_row = SpellRange {
            min: 8.0,
            max: 25.0,
            flags: 0,
        };
        let (min, max) = min_max_range(&d, Some(&charge_row), 1.5, Some(1.5)).unwrap();
        assert!((min - (8.0 + 3.0)).abs() < 1e-3);
        assert!((max - (25.0 + 3.0)).abs() < 1e-3);

        // Fireball's 0-35 row pads the max only: the fcomp-vs-0.0 guard keeps the min at zero.
        let fireball_row = SpellRange {
            min: 0.0,
            max: 35.0,
            flags: 0,
        };
        let (min, max) = min_max_range(&d, Some(&fireball_row), 1.5, Some(1.5)).unwrap();
        assert_eq!(min, 0.0);
        assert!((max - 38.0).abs() < 1e-3);

        // No target, as the tooltip calls it (`target = NULL`, `0x52e9c2`): the raw bounds.
        assert_eq!(
            min_max_range(&d, Some(&charge_row), 1.5, None),
            Some((8.0, 25.0))
        );

        // The on-next-swing attribute short-circuits to 100 without reading the row.
        assert_eq!(
            min_max_range(&spell(1, 0x400), None, 1.5, None),
            Some((0.0, ON_NEXT_SWING_RANGE))
        );

        let self_row = SpellRange {
            min: 0.0,
            max: 0.0,
            flags: 0,
        };
        assert_eq!(min_max_range(&d, Some(&self_row), 1.5, None), None);
    }

    /// Row 2 is the melee family, 114 Auto Shot's 8-35, 95 Charge's 8-25.
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

        // Row 4 (Shadow Bolt, Frostbolt, wand Shoot) reads a true 0.0 min, the guard's input.
        let nuke = ranges.get(4).expect("row 4");
        assert_eq!((nuke.min, nuke.max), (0.0, 30.0));
        assert!(!nuke.is_melee());
    }
}
