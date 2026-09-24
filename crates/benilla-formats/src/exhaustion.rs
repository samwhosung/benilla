//! `Exhaustion.dbc`: the rest states. `GetRestState` (`0x48d350`) indexes it by the
//! `PLAYER_BYTES_2` rest-state byte (`[0xc0dd78]`) and returns the id, the name from this file's
//! string block (not GlobalStrings) and the factor. `GetXPExhaustion` (`0x48d3f0`) scales the
//! rested pool by row 1's factor, 2.0 in the data. Rows 3-5 are beta tiers never sent.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

const EXHAUSTION: &str = "DBFilesClient\\Exhaustion.dbc";

/// One rest state, as `GetRestState` returns it.
pub struct ExhaustionRow {
    /// Also the wire's rest-state byte, which the client indexes by.
    pub id: u32,
    /// The state name, from the enUS slot.
    pub name: String,
    /// The XP multiplier.
    pub factor: f32,
}

fn exhaustion_schema() -> Schema {
    let mut s = Schema::new("Exhaustion");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Xp", FieldType::UInt32));
    s.add_field(SchemaField::new("Factor", FieldType::Float32));
    s.add_field(SchemaField::new("OutdoorHours", FieldType::Float32));
    s.add_field(SchemaField::new("InnHours", FieldType::Float32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.add_field(SchemaField::new("Threshold", FieldType::UInt32));
    s
}

/// Load Exhaustion.dbc from the patch chain.
pub fn load_exhaustion(chain: &mut Chain) -> Result<Vec<ExhaustionRow>> {
    let bytes = chain
        .read_file(EXHAUSTION)
        .with_context(|| format!("reading {EXHAUSTION}"))?;
    let rs = parse(&bytes, exhaustion_schema(), "Exhaustion")?;
    let mut rows = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(factor)) = (u32_at(r, 0), f32_at(r, 2)) else {
            continue;
        };
        let name = str_at(&rs, r, 5).unwrap_or_default();
        rows.push(ExhaustionRow { id, name, factor });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::load_exhaustion;

    #[test]
    fn the_shipped_rest_states_carry_the_rested_double() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_exhaustion(&mut chain).expect("Exhaustion.dbc");
        let by_id: std::collections::HashMap<u32, (&str, f32)> = rows
            .iter()
            .map(|r| (r.id, (r.name.as_str(), r.factor)))
            .collect();
        assert_eq!(by_id[&1], ("Rested", 2.0), "the ×2 is this row's data");
        assert_eq!(by_id[&2], ("Normal", 1.0));
        assert_eq!(by_id[&3], ("XXXTired", 1.0), "beta tier, never sent");
        assert_eq!(by_id[&4], ("XXXTired", 0.5));
        assert_eq!(by_id[&5], ("XXXExhausted", 0.25));
        assert_eq!(rows.len(), 5, "the whole shipped table");
    }
}
