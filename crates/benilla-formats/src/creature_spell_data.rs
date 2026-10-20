//! `CreatureSpellData.dbc`: a creature template's pet abilities, the four spells a tamed beast of
//! it brings, which the studied-creature block names (`0x529d56`-`0x529e59`). The template's
//! `pet_spell_list_id` keys it (the creature query's sixth dword). 9 fields in 36-byte rows: the
//! id, four spell ids at `+0x4`, and four availability words at `+0x14`, each 0 or 100 in 1.12.1.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const CREATURE_SPELL_DATA: &str = "DBFilesClient\\CreatureSpellData.dbc";
const FIELDS: usize = 9;
/// The first spell id and the first availability word, by column.
const SPELLS_FIELD: usize = 1;
const AVAILABILITY_FIELD: usize = 5;
/// The availability the block requires of an ability before it names it (`0x529dbc`).
const AVAILABLE: u32 = 100;

/// `CreatureSpellData.dbc` by row id.
#[derive(Debug, Default, Clone)]
pub struct CreatureSpellData(HashMap<u32, [(u32, u32); 4]>);

impl CreatureSpellData {
    /// The spells of row `id` the studied-creature block names: each of the four slots whose
    /// availability word is at least 100 (`0x529dbc`), in slot order, zero ids included, which
    /// the caller drops as having no `Spell.dbc` record (`0x529db4`). `None` without the row.
    pub fn available_spells(&self, id: u32) -> Option<impl Iterator<Item = u32> + '_> {
        let row = self.0.get(&id)?;
        Some(
            row.iter()
                .filter(|&&(_, availability)| availability >= AVAILABLE)
                .map(|&(spell, _)| spell),
        )
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("CreatureSpellData");
    for i in 0..FIELDS {
        s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
    }
    s
}

/// Load `CreatureSpellData.dbc` from the patch chain.
pub fn load_creature_spell_data(chain: &mut Chain) -> Result<CreatureSpellData> {
    let bytes = chain
        .read_file(CREATURE_SPELL_DATA)
        .with_context(|| format!("reading {CREATURE_SPELL_DATA}"))?;
    table_from(&bytes)
}

fn table_from(bytes: &[u8]) -> Result<CreatureSpellData> {
    let rs = parse(bytes, schema(), "CreatureSpellData.dbc")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let slot = |i: usize| {
            (
                u32_at(r, SPELLS_FIELD + i).unwrap_or(0),
                u32_at(r, AVAILABILITY_FIELD + i).unwrap_or(0),
            )
        };
        rows.insert(id, [slot(0), slot(1), slot(2), slot(3)]);
    }
    Ok(CreatureSpellData(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two rows of the shipped shape: one ability, and a second slot held back by its word.
    fn synthesize() -> Vec<u8> {
        let rows: &[[u32; FIELDS]] = &[
            [5796, 16827, 0, 0, 0, 100, 0, 0, 0],
            [9000, 17253, 24450, 0, 0, 100, 0, 0, 0],
        ];
        let mut dbc = b"WDBC".to_vec();
        for w in [rows.len() as u32, FIELDS as u32, FIELDS as u32 * 4, 1] {
            dbc.extend_from_slice(&w.to_le_bytes());
        }
        for r in rows {
            for v in r {
                dbc.extend_from_slice(&v.to_le_bytes());
            }
        }
        dbc.push(0);
        dbc
    }

    /// A slot counts only at availability 100 (`0x529dbc`); a missing row answers `None`.
    #[test]
    fn a_row_names_the_spells_its_availability_words_allow() {
        let t = table_from(&synthesize()).expect("parse");
        let spells = |id| t.available_spells(id).map(|s| s.collect::<Vec<_>>());
        assert_eq!(spells(5796), Some(vec![16827]));
        assert_eq!(spells(9000), Some(vec![17253]), "24450's word is 0");
        assert_eq!(spells(1), None);
    }

    /// The shipped file: 315 rows; the Ice Claw Bear's list 5796 (vmangos `creature_template`
    /// entry 1196) is Claw, 16827.
    #[test]
    fn the_installed_table_reads_a_bears_claw() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let t = load_creature_spell_data(&mut chain).expect("load CreatureSpellData.dbc");
        assert_eq!(t.len(), 315);
        assert_eq!(
            t.available_spells(5796).map(|s| s.collect::<Vec<_>>()),
            Some(vec![16827])
        );
    }
}
