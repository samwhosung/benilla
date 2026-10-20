//! `Resistances.dbc`, the damage schools: which school's resistance is armor. `0x53f510` walks the
//! first seven rows in record order and keeps the position of the last whose `Flags` (`+0x4`) has
//! bit 0 (`0x53f598`-`0x53f59e`), which `0x53f830` answers; the studied-creature block reads that
//! slot of `UNIT_FIELD_RESISTANCES` as armor (`0x529ae6`) and skips it among the resistances
//! (`0x529b76`). The shipped file flags only row 0, Physical.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const RESISTANCES: &str = "DBFilesClient\\Resistances.dbc";
/// 12 fields: the id, `Flags`, the fizzle sound, eight name locales and their flags.
const FIELDS: usize = 12;
const FLAGS_FIELD: usize = 1;
/// The rows `0x53f510` walks (`0x53f534`-`0x53f539`), one per `UNIT_FIELD_RESISTANCES` slot.
const MAX_SCHOOLS: usize = 7;

fn schema() -> Schema {
    let mut s = Schema::new("Resistances");
    for i in 0..FIELDS {
        let ty = if (3..=10).contains(&i) {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// The armor school (`0x53f510`): `None` when no row is flagged, the reference's -1.
pub fn load_armor_school(chain: &mut Chain) -> Result<Option<u8>> {
    let bytes = chain
        .read_file(RESISTANCES)
        .with_context(|| format!("reading {RESISTANCES}"))?;
    armor_school_from(&bytes)
}

fn armor_school_from(bytes: &[u8]) -> Result<Option<u8>> {
    let rs = parse(bytes, schema(), "Resistances.dbc")?;
    Ok(rs
        .records()
        .iter()
        .take(MAX_SCHOOLS)
        .enumerate()
        .rfind(|(_, r)| u32_at(r, FLAGS_FIELD).unwrap_or(0) & 1 != 0)
        .map(|(i, _)| i as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthesize(flags: &[u32]) -> Vec<u8> {
        let mut dbc = b"WDBC".to_vec();
        for w in [flags.len() as u32, FIELDS as u32, FIELDS as u32 * 4, 1] {
            dbc.extend_from_slice(&w.to_le_bytes());
        }
        for (id, &f) in flags.iter().enumerate() {
            let mut row = [0u32; FIELDS];
            row[0] = id as u32;
            row[FLAGS_FIELD] = f;
            for v in row {
                dbc.extend_from_slice(&v.to_le_bytes());
            }
        }
        dbc.push(0);
        dbc
    }

    /// The last flagged row of the first seven, by position (`0x53f59e`), or none.
    #[test]
    fn the_armor_school_is_the_last_flagged_row() {
        let school = |flags: &[u32]| armor_school_from(&synthesize(flags)).expect("parse");
        assert_eq!(school(&[1, 0, 0, 0, 0, 0, 0]), Some(0));
        assert_eq!(school(&[1, 0, 3, 0, 0, 0, 0]), Some(2));
        assert_eq!(
            school(&[0, 0, 0, 0, 0, 0, 0, 1]),
            None,
            "an eighth row is not read"
        );
        assert_eq!(school(&[0, 2, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn the_installed_table_flags_physical() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        assert_eq!(load_armor_school(&mut chain).expect("load"), Some(0));
    }
}
