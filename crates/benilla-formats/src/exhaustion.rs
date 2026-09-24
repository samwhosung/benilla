//! Exhaustion.dbc — the rest-state table behind the client's rested-XP surface.
//!
//! The whole client contract: `GetRestState 0x48d350` indexes this table
//! **directly by the `PLAYER_BYTES_2` rest-state byte** (an ID→row-ptr array, `[0xc0dd78]`) and
//! returns `(row.ID, row.name[locale], row.factor)`; `GetXPExhaustion 0x48d3f0` multiplies the
//! rested pool by **row ID 1's factor, hard-coded** (2.0 in the shipped data — the "rested XP is
//! double" law is this one f32, not a client constant). The names come from this file's string
//! block, **not** GlobalStrings — which is why "Rested"/"Normal" localize with the install.
//!
//! Row layout (fieldCount 0xf / recordSize 0x3c, validated by the client loader `0x544da0`):
//! `ID@0`, `Xp@1`, `Factor@2` (f32), `OutdoorHours@3`, `InnHours@4` (both unread by the
//! bindings), `Name_Lang@5..12`, `NameFlags@13`, `Threshold@14`. Shipped 5875 rows: 1 "Rested"
//! 2.0 · 2 "Normal" 1.0 · 3/4 "XXXTired" 1.0/0.5 · 5 "XXXExhausted" 0.25 — rows 3..5 are the
//! never-shipped beta tiers FrameXML still carries dead branches for.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

const EXHAUSTION: &str = "DBFilesClient\\Exhaustion.dbc";

/// One Exhaustion.dbc row as the rest bindings consume it.
pub struct ExhaustionRow {
    /// The row ID — also exactly the wire's rest-state byte (the client indexes by it).
    pub id: u32,
    /// The localized state name (`GetRestState`'s second return; enUS slot of this install).
    pub name: String,
    /// The XP multiplier (`GetRestState`'s third return; row 1's value is `GetXPExhaustion`'s
    /// scale).
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

/// Load Exhaustion.dbc from the patch chain — the rows for `UiScript::set_exhaustion_rows`.
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

    /// The shipped 5875 table, read as data rather than assumed — five rows: the rested factor
    /// really is 2.0-as-data, and the beta tiers really ship with placeholder names. Skips
    /// without client data.
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
