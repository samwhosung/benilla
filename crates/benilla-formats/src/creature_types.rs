//! `CreatureType.dbc`: each creature type's name (columns 1-8, one per locale) and flags word
//! (column 10). `UnitCreatureType` (`0x51a2bc`) and the unit tooltip's level line (`0x52a2e5`)
//! resolve a type id (`0x605570`) to its row in the cached table (`[0xc0de2c]`, bounded by
//! `[0xc0de30]`) and read the string at `row + 4 + 4 * [0xc0e080]` (`0x51a2dc`, `0x52a4bd`): the
//! install's own language. The TAB-target scorer (`0x494200`) rejects a unit whose type row has
//! `row[+0x28] & 1`. A unit's type comes from `SMSG_CREATURE_QUERY_RESPONSE`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{loc_str, parse, u32_at};
use crate::{Chain, Locale};

const CREATURE_TYPE: &str = "DBFilesClient\\CreatureType.dbc";

/// A creature type's row: its name in the locale's column and its flags word.
#[derive(Debug, Default, Clone)]
struct Row {
    name: String,
    flags: u32,
}

/// Creature type id → the row's name and flags word.
#[derive(Debug, Default, Clone)]
pub struct CreatureTypeTable(HashMap<u32, Row>);

impl CreatureTypeTable {
    /// A table of `(id, name, flags)` rows.
    pub fn from_rows<'a>(rows: impl IntoIterator<Item = (u32, &'a str, u32)>) -> Self {
        Self(
            rows.into_iter()
                .map(|(id, name, flags)| {
                    let name = name.to_string();
                    (id, Row { name, flags })
                })
                .collect(),
        )
    }

    /// Whether TAB targeting skips this type (`flags & 1`): in 1.12 data only Critter (8), not
    /// Totem (11). An unknown type is targetable, as the client skips an out-of-range index.
    pub fn no_tab_target(&self, creature_type: u32) -> bool {
        self.0.get(&creature_type).is_some_and(|r| r.flags & 1 != 0)
    }

    /// The type's name in the table's locale column, `None` for an id with no row. The reference
    /// pushes whatever string the column holds, so a row whose column is blank answers `""`.
    pub fn name(&self, creature_type: u32) -> Option<&str> {
        self.0.get(&creature_type).map(|r| r.name.as_str())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn creature_type_schema() -> Schema {
    let mut s = Schema::new("CreatureType");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    s
}

/// Load CreatureType.dbc from the patch chain, its names at the chain's locale.
pub fn load_creature_type_table(chain: &mut Chain) -> Result<CreatureTypeTable> {
    let bytes = chain
        .read_file(CREATURE_TYPE)
        .with_context(|| format!("reading {CREATURE_TYPE}"))?;
    table_from_bytes(&bytes, chain.locale())
}

/// The table a `CreatureType.dbc` holds, its names read at `locale`.
fn table_from_bytes(bytes: &[u8], locale: Locale) -> Result<CreatureTypeTable> {
    let rs = parse(bytes, creature_type_schema(), "CreatureType")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let Some(id) = u32_at(r, 0) {
            let name = loc_str(&rs, r, 1, locale).unwrap_or_default();
            let flags = u32_at(r, 10).unwrap_or(0);
            rows.insert(id, Row { name, flags });
        }
    }
    Ok(CreatureTypeTable(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two WDBC rows of the shipped shape: id, eight names, the name mask and the flags word.
    /// Type 1's names differ per locale; type 8's seventh column is blank.
    fn synthetic() -> Vec<u8> {
        let names = [
            "Beast", "야수", "Bête", "Wildtier", "野兽", "野獸", "Bestia", "xx",
        ];
        let mut strings = vec![0u8];
        let offset = |s: &str, strings: &mut Vec<u8>| {
            let at = strings.len() as u32;
            strings.extend_from_slice(s.as_bytes());
            strings.push(0);
            at
        };
        let mut rows: Vec<Vec<u32>> = Vec::new();
        let mut row = vec![1u32];
        row.extend(names.iter().map(|n| offset(n, &mut strings)));
        row.extend([0x3f_007e, 0]);
        rows.push(row);
        let mut row = vec![8u32];
        row.extend((0..8).map(|i| {
            if i == 7 {
                0
            } else {
                offset("Critter", &mut strings)
            }
        }));
        row.extend([0x3f_007e, 1]);
        rows.push(row);
        let mut bytes = b"WDBC".to_vec();
        for w in [2, 11, 44, strings.len() as u32] {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        for f in rows.iter().flatten() {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        bytes.extend_from_slice(&strings);
        bytes
    }

    /// Each locale reads its own name column (`row + 4 + 4 * [0xc0e080]`), and a blank column
    /// answers an empty word, not a missing row.
    #[test]
    fn each_locale_reads_its_own_name_column() {
        let bytes = synthetic();
        let want = [
            "Beast", "야수", "Bête", "Wildtier", "野兽", "野獸", "Bestia", "xx",
        ];
        for (index, name) in want.iter().enumerate() {
            let locale = Locale::from_index(index).unwrap();
            let table = table_from_bytes(&bytes, locale).unwrap();
            assert_eq!(table.name(1), Some(*name), "{}", locale.name());
            assert!(!table.no_tab_target(1));
            assert!(table.no_tab_target(8), "flags are the same in every locale");
        }
        let seventh = Locale::from_index(7).unwrap();
        let table = table_from_bytes(&bytes, seventh).unwrap();
        assert_eq!(
            table.name(8),
            Some(""),
            "a blank column is a row with no word"
        );
        assert_eq!(table.name(2), None, "no row, no word");
        // enGB names no table entry, so it reads enUS's column.
        let gb = Locale::from_language_country(b"en", b"GB");
        assert_eq!(table_from_bytes(&bytes, gb).unwrap().name(1), Some("Beast"));
    }

    #[test]
    fn shipped_flags_mark_only_critter() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let table = load_creature_type_table(&mut chain).expect("load CreatureType.dbc");
        assert_eq!(table.len(), 11, "1.12 ships 11 creature types");
        assert!(table.no_tab_target(8), "Critter (8) must be un-TAB-able");
        for id in [1, 7, 11] {
            assert!(!table.no_tab_target(id), "type {id} must be targetable");
        }
        assert!(!table.no_tab_target(999), "unknown type is targetable");
    }

    /// The install's own column names the eleven types, and the chain's locale is the column
    /// read: another locale's, empty in a single-language install, reads blank words.
    #[test]
    fn the_shipped_names_follow_the_chains_locale() {
        let data = crate::wow_data_or_skip!();
        let mut own = crate::open_chain(&data).expect("open chain");
        let index = own.locale().index();
        let table = load_creature_type_table(&mut own).expect("load CreatureType.dbc");
        for id in (1..=9).chain([11]) {
            assert!(
                table.name(id).is_some_and(|n| !n.is_empty()),
                "type {id} has a name in {}",
                own.locale().name()
            );
        }
        let other = Locale::from_index((index + 1) % 8).unwrap();
        let mut other_chain = crate::open_chain(&data)
            .expect("open chain")
            .with_locale(other);
        let blank = load_creature_type_table(&mut other_chain).expect("load CreatureType.dbc");
        assert_eq!(blank.len(), table.len());
        assert_eq!(
            blank.name(1),
            Some(""),
            "{} is not this install's",
            other.name()
        );
    }
}
