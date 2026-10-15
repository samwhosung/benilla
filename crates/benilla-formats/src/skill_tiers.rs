//! `SkillTiers.dbc`: per tier, sixteen step costs and sixteen step values. `BuySkillTier` prices
//! a skill step by the cost at the player's step in the tier its `SkillRaceClassInfo` row names
//! (`0x6de1c0`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const SKILL_TIERS: &str = "DBFilesClient\\SkillTiers.dbc";

/// The tiers by id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillTiers {
    costs: HashMap<u32, [u32; 16]>,
}

impl SkillTiers {
    /// Tiers from `(id, costs)` pairs, for tests and fixtures.
    pub fn from_costs(tiers: impl IntoIterator<Item = (u32, [u32; 16])>) -> Self {
        SkillTiers {
            costs: tiers.into_iter().collect(),
        }
    }

    /// `0x6de1c0(tier, step)`: the cost at `step` in the tier; 0 for tier 0, a tier with no row
    /// and a step of 16 or more.
    pub fn cost(&self, tier_id: u32, step: u32) -> u32 {
        if tier_id == 0 {
            return 0;
        }
        self.costs
            .get(&tier_id)
            .and_then(|c| c.get(step as usize))
            .copied()
            .unwrap_or(0)
    }

    /// Every tier, for the app to hand the VM.
    pub fn tiers(&self) -> impl Iterator<Item = (u32, [u32; 16])> + '_ {
        self.costs.iter().map(|(&id, &c)| (id, c))
    }

    pub fn len(&self) -> usize {
        self.costs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.costs.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("SkillTiers");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..32 {
        s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
    }
    s
}

/// Load `SkillTiers.dbc` from the patch chain.
pub fn load_skill_tiers(chain: &mut Chain) -> Result<SkillTiers> {
    let bytes = chain
        .read_file(SKILL_TIERS)
        .with_context(|| format!("reading {SKILL_TIERS}"))?;
    let rs = parse(&bytes, schema(), "SkillTiers")?;
    let mut costs = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut c = [0u32; 16];
        for (i, slot) in c.iter_mut().enumerate() {
            *slot = u32_at(r, 1 + i).unwrap_or(0);
        }
        costs.insert(id, c);
    }
    Ok(SkillTiers { costs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cost_is_zero_for_tier_zero_a_missing_tier_and_a_step_past_sixteen() {
        let t = SkillTiers::from_costs([(3, [5, 6, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9])]);
        assert_eq!(t.cost(3, 1), 6);
        assert_eq!(t.cost(3, 15), 9);
        assert_eq!(t.cost(3, 16), 0);
        assert_eq!(t.cost(0, 1), 0);
        assert_eq!(t.cost(4, 1), 0);
    }

    /// The shipped table has 26 tiers, and the tier ids the race-class rows name resolve in it.
    #[test]
    fn the_shipped_table_has_twenty_six_tiers() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let tiers = load_skill_tiers(&mut chain).expect("SkillTiers.dbc");
        assert_eq!(tiers.len(), 26);
    }
}
