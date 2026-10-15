//! `LFGDungeons.dbc`, the rows the looking-for-group lists are built from (`0x4e7c70`): a name, a
//! level range, a type (1 dungeon, 2 raid, 4 zone, 5 battleground; 3 is the quest list) and a side.
//! The ids are even and sparse; the client indexes the store by id.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, loc_str, parse, u32_at};
use crate::Chain;

const LFG_DUNGEONS: &str = "DBFilesClient\\LFGDungeons.dbc";

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfgDungeon {
    pub id: u32,
    /// The name, in the locale's column.
    pub name: String,
    /// `+0x28` and `+0x2c`: the player levels the row is offered to.
    pub min_level: u32,
    pub max_level: u32,
    /// `+0x30`: the list the row joins.
    pub kind: u32,
    /// `+0x34`: 0 Horde, 1 Alliance, negative either.
    pub side: i32,
}

fn schema() -> Schema {
    let mut s = Schema::new("LFGDungeons");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    for name in ["NameFlags", "MinLevel", "MaxLevel", "Type", "Faction"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// The rows in file order.
pub fn load_lfg_dungeons(chain: &mut Chain) -> Result<Vec<LfgDungeon>> {
    let bytes = chain
        .read_file(LFG_DUNGEONS)
        .with_context(|| format!("reading {LFG_DUNGEONS}"))?;
    let rs = parse(&bytes, schema(), "LFGDungeons")?;
    let locale = chain.locale();
    Ok(rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(LfgDungeon {
                id: u32_at(r, 0)?,
                name: loc_str(&rs, r, 1, locale).unwrap_or_default(),
                min_level: u32_at(r, 10)?,
                max_level: u32_at(r, 11)?,
                kind: u32_at(r, 12)?,
                side: i32_at(r, 13)?,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_table_is_sixty_seven_rows_in_type_runs() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_lfg_dungeons(&mut chain).expect("LFGDungeons.dbc");
        assert_eq!(rows.len(), 67);
        assert_eq!((rows[0].id, rows[0].name.as_str()), (1, "Wailing Caverns"));
        assert_eq!(
            (rows[0].min_level, rows[0].max_level, rows[0].kind),
            (15, 28, 1)
        );
        // Ragefire Chasm is Horde (0), Stormwind Stockades Alliance (1), Deadmines either.
        let side = |name: &str| rows.iter().find(|r| r.name == name).map(|r| r.side);
        assert_eq!(side("Ragefire Chasm"), Some(0));
        assert_eq!(side("Stormwind Stockades"), Some(1));
        assert_eq!(side("Deadmines"), Some(-1));
        // The reference groups by type on the assumption each type is one run (`0x4e7d19`).
        let mut seen = vec![];
        for r in &rows {
            if seen.last() != Some(&r.kind) {
                assert!(!seen.contains(&r.kind), "type {} twice", r.kind);
                seen.push(r.kind);
            }
        }
    }
}
