//! `ChrRaces.dbc`, the per-race table, narrowed to the two strings the unit getters push. The
//! reference loads it into a race-indexed record table (`0xc0dee0`, max id `0xc0dee4`).
//!
//! `UnitRace` (`0x5182e0`) takes the race byte's row and pushes two strings: the display name from
//! the `Name_Lang` block (`+0x44`, field 17, eight locale columns, read at `[0xc0e080]`,
//! `0x518301`), then the race token from field 15 (`+0x3c`, `0x51830c`), `"NightElf"` or
//! `"Scourge"`, the file string. A race with no row pushes two nils. The token is a key, never
//! display text, and is not localized.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{loc_str, parse, str_at, u32_at};
use crate::{Chain, Locale};

const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";

/// The shipped copy's column count, which `benilla-dbc` enforces as the reference loader does.
const CHR_RACES_FIELDS: usize = 29;

/// Field 15, the file string (`+0x3c`).
const FILE_FIELD: usize = 0x3c / 4;

/// Field 16, `CinematicSequenceID` (`+0x40`).
const INTRO_FIELD: usize = 0x40 / 4;

/// Field 17, the first of the eight `Name_Lang` columns (`+0x44`).
const NAME_FIELD: usize = 0x44 / 4;

/// One race's row: its name at the locale and its token.
#[derive(Debug, Clone, Default)]
struct Row {
    name: String,
    file: String,
    /// `CinematicSequenceID` (`+0x40`).
    intro: u32,
}

/// `ChrRaces.dbc`'s read columns by race id.
#[derive(Debug, Default, Clone)]
pub struct ChrRaces(HashMap<u32, Row>);

impl ChrRaces {
    /// A table of `(id, name, file)` rows.
    pub fn from_rows<'a>(rows: impl IntoIterator<Item = (u32, &'a str, &'a str)>) -> Self {
        Self(
            rows.into_iter()
                .map(|(id, name, file)| {
                    let (name, file) = (name.to_string(), file.to_string());
                    let intro = 0;
                    (id, Row { name, file, intro })
                })
                .collect(),
        )
    }

    /// `UnitRace`'s two returns for a race byte: the name and the token. A row pushes whatever its
    /// strings are, an empty one included; a race with no row has none (the two nils, `0x518324`).
    pub fn names(&self, race: u32) -> Option<(&str, &str)> {
        self.0
            .get(&race)
            .map(|r| (r.name.as_str(), r.file.as_str()))
    }

    /// The race row's `CinematicSequenceID` (`+0x40`), the intro cinematic the first-login camera
    /// looks up (`0x46c2f8`); a race with no row has none.
    pub fn intro_sequence(&self, race: u32) -> Option<u32> {
        self.0.get(&race).map(|r| r.intro)
    }

    /// A table for tests of what reads the intro column: `race` holds `sequence`.
    #[cfg(test)]
    pub(crate) fn with_intro_sequence(mut self, race: u32, sequence: u32) -> Self {
        self.0.entry(race).or_default().intro = sequence;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("ChrRaces");
    for i in 0..CHR_RACES_FIELDS {
        let ty = if i == FILE_FIELD
            || (NAME_FIELD..NAME_FIELD + 8).contains(&i)
            || (26..=28).contains(&i)
        {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load `ChrRaces.dbc`'s read columns from the patch chain, names at the chain's locale.
pub fn load_chr_races(chain: &mut Chain) -> Result<ChrRaces> {
    let bytes = chain
        .read_file(CHR_RACES)
        .with_context(|| format!("reading {CHR_RACES}"))?;
    table_from_bytes(&bytes, chain.locale())
}

/// The table a `ChrRaces.dbc` holds, its names read at `locale`.
fn table_from_bytes(bytes: &[u8], locale: Locale) -> Result<ChrRaces> {
    let rs = parse(bytes, schema(), "ChrRaces.dbc")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.insert(
            id,
            Row {
                name: loc_str(&rs, r, NAME_FIELD, locale).unwrap_or_default(),
                file: str_at(&rs, r, FILE_FIELD).unwrap_or_default(),
                intro: u32_at(r, INTRO_FIELD).unwrap_or(0),
            },
        );
    }
    Ok(ChrRaces(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two 29-field rows: race 4 with a name per locale, race 9 with its seventh column blank.
    fn synthetic() -> Vec<u8> {
        let names = [
            "Night Elf",
            "나이트 엘프",
            "Elfe de la nuit",
            "Nachtelf",
            "暗夜精灵",
            "夜精靈",
            "Elfo de la noche",
            "xx",
        ];
        let mut strings = vec![0u8];
        let put = |s: &str, strings: &mut Vec<u8>| {
            let at = strings.len() as u32;
            strings.extend_from_slice(s.as_bytes());
            strings.push(0);
            at
        };
        let mut rows: Vec<Vec<u32>> = Vec::new();
        let mut row = vec![0u32; CHR_RACES_FIELDS];
        row[0] = 4;
        row[INTRO_FIELD] = 61;
        row[FILE_FIELD] = put("NightElf", &mut strings);
        for (i, n) in names.iter().enumerate() {
            row[NAME_FIELD + i] = put(n, &mut strings);
        }
        rows.push(row);
        let mut row = vec![0u32; CHR_RACES_FIELDS];
        row[0] = 9;
        row[FILE_FIELD] = put("Goblin", &mut strings);
        for i in 0..7 {
            row[NAME_FIELD + i] = put("Goblin", &mut strings);
        }
        rows.push(row);
        let mut bytes = b"WDBC".to_vec();
        for w in [
            2,
            CHR_RACES_FIELDS as u32,
            4 * CHR_RACES_FIELDS as u32,
            strings.len() as u32,
        ] {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        for f in rows.iter().flatten() {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        bytes.extend_from_slice(&strings);
        bytes
    }

    /// Each locale reads its own name column (`row + 0x44 + 4 * [0xc0e080]`) and the token is the
    /// same string in all of them; a blank column is an empty name on a row that exists, and a
    /// race with no row has nothing.
    #[test]
    fn each_locale_reads_its_own_name_and_the_token_stays() {
        let bytes = synthetic();
        let want = [
            "Night Elf",
            "나이트 엘프",
            "Elfe de la nuit",
            "Nachtelf",
            "暗夜精灵",
            "夜精靈",
            "Elfo de la noche",
            "xx",
        ];
        for (index, name) in want.iter().enumerate() {
            let locale = Locale::from_index(index).unwrap();
            let table = table_from_bytes(&bytes, locale).unwrap();
            assert_eq!(
                table.names(4),
                Some((*name, "NightElf")),
                "{}",
                locale.name()
            );
        }
        let blank = table_from_bytes(&bytes, Locale::from_index(7).unwrap()).unwrap();
        assert_eq!(blank.names(9), Some(("", "Goblin")));
        assert_eq!(blank.names(3), None);
        assert_eq!(blank.names(0), None);
    }

    /// `CinematicSequenceID` is the row's field 16 (`+0x40`, read at `0x46c2f8`): a row whose column
    /// is 0 holds 0, and a race with no row holds nothing.
    #[test]
    fn a_race_row_holds_its_intro_sequence() {
        let table = table_from_bytes(&synthetic(), Locale::from_index(0).unwrap()).unwrap();
        assert_eq!(table.intro_sequence(4), Some(61));
        assert_eq!(table.intro_sequence(9), Some(0));
        assert_eq!(table.intro_sequence(3), None);
    }

    /// The shipped table at the install's locale: the eight playable races and the Goblin row.
    #[test]
    fn the_shipped_names_and_tokens() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let t = load_chr_races(&mut chain).expect("load ChrRaces.dbc");
        if chain.locale().index() == 0 {
            for (race, name, file) in [
                (1, "Human", "Human"),
                (2, "Orc", "Orc"),
                (3, "Dwarf", "Dwarf"),
                (4, "Night Elf", "NightElf"),
                (5, "Undead", "Scourge"),
                (6, "Tauren", "Tauren"),
                (7, "Gnome", "Gnome"),
                (8, "Troll", "Troll"),
            ] {
                assert_eq!(t.names(race), Some((name, file)), "race {race}");
            }
        }
        assert_eq!(t.names(0), None);
        assert!(t
            .names(1)
            .is_some_and(|(n, f)| !n.is_empty() && !f.is_empty()));
    }
}
