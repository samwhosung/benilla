//! `DurabilityCosts.dbc` and `DurabilityQuality.dbc`, behind the repair price the reference
//! computes itself (`0x5da330`, wrapped by `0x4faf30`): points lost × the quality row
//! `2·Quality + 1` × the item level's weapon or armor column by subclass, rounded half away from
//! zero to at least 1, then × `f32(1 - discount)`, stored as f32 and rounded half to even. vmangos
//! keys the quality row `(Quality + 1)·2` (`Player.cpp:4996`), so its charge may differ from the
//! price shown.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{f32_at, parse, u32_at};

const DURABILITY_COSTS: &str = "DBFilesClient\\DurabilityCosts.dbc";
const DURABILITY_QUALITIES: &str = "DBFilesClient\\DurabilityQuality.dbc";

/// The two repair tables, id-keyed.
pub struct DurabilityTables {
    /// Item level → the 29 per-point cost columns (21 weapon, then 8 armor).
    costs: HashMap<u32, Vec<u32>>,
    /// Row id → the quality multiplier (the client keys this with `2·Quality + 1`).
    qualities: HashMap<u32, f32>,
}

impl DurabilityTables {
    /// The per-point cost for a class and subclass at `item_level`, `None` when unrepairable.
    fn column(&self, item_level: u32, class: u32, subclass: u32) -> Option<u32> {
        let row = self.costs.get(&item_level)?;
        let idx = match class {
            2 if subclass < 21 => subclass as usize,
            4 if subclass < 8 => 21 + subclass as usize,
            _ => return None,
        };
        row.get(idx).copied()
    }

    /// The displayed repair cost of one item in copper, `0x4faf30`, after `discount`, the price
    /// discount `0x612b80` returns (0 with no merchant open), kept in f64 as the reference's `st0`.
    pub fn repair_cost(
        &self,
        points_lost: u32,
        item_level: u32,
        quality: u32,
        class: u32,
        subclass: u32,
        discount: f64,
    ) -> u32 {
        if points_lost == 0 {
            return 0;
        }
        let Some(col) = self.column(item_level, class, subclass) else {
            return 0;
        };
        let Some(&mult) = self.qualities.get(&(2 * quality + 1)) else {
            return 0;
        };
        let base = f64::from(points_lost) * f64::from(mult) * f64::from(col);
        // Half away from zero, as the reference's ±0.5 then `_ftol`, then at least 1.
        let per = ((base + 0.5).floor() as i64).max(1);
        // `1 - discount` stored as an f32 (`0x4faf8f fsubr`, `0x4faf95 fstp dword`).
        let factor = (1.0 - discount) as f32;
        // The product stored as an f32 (`0x4fafaf`), then `fistp` rounds half to even (`0x4fafb5`).
        let product = (per as f64 * f64::from(factor)) as f32;
        f64::from(product).round_ties_even() as u32
    }
}

fn costs_schema() -> Schema {
    let mut s = Schema::new("DurabilityCosts");
    s.add_field(SchemaField::new("id", FieldType::UInt32));
    s.add_field(SchemaField::new_array("weapon", FieldType::UInt32, 21));
    s.add_field(SchemaField::new_array("armor", FieldType::UInt32, 8));
    s.set_key_field("id");
    s
}

fn qualities_schema() -> Schema {
    let mut s = Schema::new("DurabilityQuality");
    s.add_field(SchemaField::new("id", FieldType::UInt32));
    s.add_field(SchemaField::new("mult", FieldType::Float32));
    s.set_key_field("id");
    s
}

/// Load both durability tables off the patch chain.
pub fn load_durability_tables(chain: &mut Chain) -> Result<DurabilityTables> {
    let bytes = chain
        .read_file(DURABILITY_COSTS)
        .with_context(|| format!("reading {DURABILITY_COSTS}"))?;
    let rs = parse(&bytes, costs_schema(), "DurabilityCosts")?;
    let mut costs = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let row: Vec<u32> = (1..30).map(|i| u32_at(r, i).unwrap_or(0)).collect();
        costs.insert(id, row);
    }

    let bytes = chain
        .read_file(DURABILITY_QUALITIES)
        .with_context(|| format!("reading {DURABILITY_QUALITIES}"))?;
    let rs = parse(&bytes, qualities_schema(), "DurabilityQuality")?;
    let mut qualities = HashMap::new();
    for r in rs.records() {
        let (Some(id), Some(mult)) = (u32_at(r, 0), f32_at(r, 1)) else {
            continue;
        };
        qualities.insert(id, mult);
    }

    Ok(DurabilityTables { costs, qualities })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> DurabilityTables {
        let mut costs = HashMap::new();
        // Item level 23: weapon subclass 7 (sword) costs 4/point, armor subclass 3 (mail) 2/point.
        let mut row = vec![0u32; 29];
        row[7] = 4;
        row[21 + 3] = 2;
        costs.insert(23, row);
        let mut qualities = HashMap::new();
        qualities.insert(3, 0.6f32); // the client's row for quality 1 (2·1+1)
        DurabilityTables { costs, qualities }
    }

    #[test]
    fn per_item_cost_is_the_clients_arithmetic() {
        let t = tables();
        assert_eq!(t.repair_cost(10, 23, 1, 2, 7, 0.0), 24);
        // Mail gloves, 7 points: 7 · 0.6 · 2 = 8.4, rounded to 8.
        assert_eq!(t.repair_cost(7, 23, 1, 4, 3, 0.0), 8);
        // A base under 1 still charges 1: 1 · 0.1 · 2 = 0.2.
        let mut t2 = tables();
        t2.qualities.insert(3, 0.1);
        assert_eq!(t2.repair_cost(1, 23, 1, 4, 3, 0.0), 1, "min 1 copper");
        assert_eq!(t.repair_cost(0, 23, 1, 2, 7, 0.0), 0);
        assert_eq!(t.repair_cost(10, 23, 1, 15, 0, 0.0), 0, "not weapon/armor");
        assert_eq!(t.repair_cost(10, 99, 1, 2, 7, 0.0), 0, "no level row");
    }

    #[test]
    fn discount_rounds_to_nearest_even() {
        let mut t = tables();
        t.qualities.insert(3, 1.0);
        let mut row = vec![0u32; 29];
        row[7] = 1; // 1 copper/point so points == the pre-discount price
        t.costs.insert(23, row);
        // Half off: 20.5 rounds to 20 and 21.5 to 22; 21.0 is no tie.
        assert_eq!(t.repair_cost(41, 23, 1, 2, 7, 0.5), 20);
        assert_eq!(t.repair_cost(43, 23, 1, 2, 7, 0.5), 22);
        assert_eq!(t.repair_cost(42, 23, 1, 2, 7, 0.5), 21);
    }

    /// `0x4faf30` at the discounts `0x612b80` returns: the f32 multiplier and the f32 product make
    /// a near-tie an exact `.5`, which then rounds to even where the f64 shortcut rounds down.
    #[test]
    fn the_discounted_cost_stores_the_multiplier_and_the_product_as_f32() {
        let mut t = tables();
        t.qualities.insert(3, 1.0);
        let mut row = vec![0u32; 29];
        row[7] = 1; // 1 copper/point so points == the base price
        t.costs.insert(23, row);
        let cost = |base: u32, disc: f64| t.repair_cost(base, 23, 1, 2, 7, disc);
        let (d05, d10) = (f64::from(0.05f32), f64::from(0.1f32));
        // 0.1: f32(0.9) = 0.89999997615814208984375; × 15 = 13.49999964237213134765625, stored
        // 13.5, to even 14. The f64 shortcut `rint(15 × 0.8999999985)` gives 13.
        assert_eq!(cost(15, d10), 14);
        // 0.05: f32(0.95) = 0.949999988079071044921875; × 10 = 9.49999988079071044921875, stored
        // 9.5, to even 10. The f64 shortcut gives 9.
        assert_eq!(cost(10, d05), 10);
        // 0.15, the f64 sum: f32(0.85) = 0.85000002384185791015625; × 30 =
        // 25.5000007152557373046875, stored 25.5, to even 26. The f64 shortcut gives 25.
        assert_eq!(cost(30, d10 + d05), 26);
        // 0.15 at 786437: × f32(0.85) = 668471.46875012, stored 668471.5, to even 668472; the
        // taxi's f64 multiplier stores 668471.4375 there, so 668471.
        assert_eq!(cost(786_437, d10 + d05), 668_472);
        // No discount: the base itself.
        assert_eq!(cost(15, 0.0), 15);
    }
}
