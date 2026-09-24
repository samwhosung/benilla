//! `Languages.dbc` joined with `ChrRaces.dbc` for `GetDefaultLanguage()`, and the
//! `LanguageWords.dbc` substitution pools.
//!
//! The binding (`0x49fcd0`) is a two-hop walk: the race's `ChrRaces` record (`[0xc0dee0]`) at
//! `+0x20`, field 8, names a language, whose `Languages` record (`[0xc0db48]`) holds the name at
//! `+4 + 4 * locale` (`[0xc0e080]`). Field 8 is the faction language, Common or Orcish, never the
//! racial one: Darnassian and Gutterspeak are extra languages `GetLanguageByIndex` lists.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const LANGUAGES: &str = "DBFilesClient\\Languages.dbc";
const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";
const LANGUAGE_WORDS: &str = "DBFilesClient\\LanguageWords.dbc";

/// Locale columns in a 5875 `Name_lang` block.
const LOCALES: usize = 8;

/// Race id to its base language's name, per locale column.
#[derive(Debug, Default, Clone)]
pub struct DefaultLanguages(HashMap<u32, [Option<String>; LOCALES]>);

impl DefaultLanguages {
    /// The race's default language name in `locale`'s column. `None` is the reference's answer at
    /// three of its four failure edges, which push no Lua value at all, not `nil`.
    pub fn name(&self, race: u32, locale: usize) -> Option<&str> {
        self.0.get(&race)?.get(locale)?.as_deref()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn languages_schema() -> Schema {
    let mut s = Schema::new("Languages");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..LOCALES {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s
}

/// `ChrRaces.dbc`, 29 fields in 5875, all read as dwords: only the count must match the header,
/// and only field 8 is used.
fn chr_races_schema() -> Schema {
    let mut s = Schema::new("ChrRaces");
    for i in 0..29 {
        s.add_field(SchemaField::new(format!("f{i}"), FieldType::UInt32));
    }
    s
}

/// `BaseLanguage`, which the binding reads at `record + 0x20`.
const CHR_RACES_BASE_LANGUAGE: usize = 8;

/// `Languages.dbc` in row order, the order `GetNumLanguages` and `GetLanguageByIndex` count in
/// (`0x49fb30`, `0x49fbe0`).
#[derive(Debug, Default, Clone)]
pub struct Languages(Vec<(u32, [Option<String>; LOCALES])>);

impl Languages {
    /// `(id, name)` per row for `locale`. A row with no name there is skipped, where the binding
    /// would push `""`.
    pub fn names(&self, locale: usize) -> impl Iterator<Item = (u32, &str)> + '_ {
        self.0
            .iter()
            .filter_map(move |(id, names)| Some((*id, names.get(locale)?.as_deref()?)))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A hand-built table for tests: `(id, enUS name)` rows.
    pub fn from_rows(rows: &[(u32, &str)]) -> Self {
        Self(
            rows.iter()
                .map(|(id, name)| {
                    let mut names: [Option<String>; LOCALES] = Default::default();
                    names[0] = Some((*name).to_string());
                    (*id, names)
                })
                .collect(),
        )
    }
}

pub fn load_languages(chain: &mut Chain) -> Result<Languages> {
    let bytes = chain
        .read_file(LANGUAGES)
        .with_context(|| format!("reading {LANGUAGES}"))?;
    let langs = parse(&bytes, languages_schema(), "Languages.dbc")?;
    let mut rows = Vec::with_capacity(langs.records().len());
    for r in langs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut names: [Option<String>; LOCALES] = Default::default();
        for (locale, slot) in names.iter_mut().enumerate() {
            *slot = str_at(&langs, r, 1 + locale);
        }
        rows.push((id, names));
    }
    Ok(Languages(rows))
}

/// Load and join both tables into the race-to-language-name map.
pub fn load_default_languages(chain: &mut Chain) -> Result<DefaultLanguages> {
    let lang_bytes = chain
        .read_file(LANGUAGES)
        .with_context(|| format!("reading {LANGUAGES}"))?;
    let langs = parse(&lang_bytes, languages_schema(), "Languages.dbc")?;
    let mut by_id: HashMap<u32, [Option<String>; LOCALES]> = HashMap::new();
    for r in langs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut names: [Option<String>; LOCALES] = Default::default();
        for (locale, slot) in names.iter_mut().enumerate() {
            *slot = str_at(&langs, r, 1 + locale);
        }
        by_id.insert(id, names);
    }

    let race_bytes = chain
        .read_file(CHR_RACES)
        .with_context(|| format!("reading {CHR_RACES}"))?;
    let races = parse(&race_bytes, chr_races_schema(), "ChrRaces.dbc")?;
    let mut out = HashMap::new();
    for r in races.records() {
        let (Some(race), Some(lang)) = (u32_at(r, 0), u32_at(r, CHR_RACES_BASE_LANGUAGE)) else {
            continue;
        };
        if let Some(names) = by_id.get(&lang) {
            out.insert(race, names.clone());
        }
    }
    Ok(DefaultLanguages(out))
}

/// `LanguageWords.dbc`, the fake words the client substitutes, word for word, when it renders
/// speech in a language the listener's character does not know. The wire carries plaintext
/// (vmangos `ChatHandler.cpp` never rewrites it): the garble is the client's, `0x49b560`, called
/// from the chat display chokepoint `0x49a870` at `0x49aa7c`.
#[derive(Debug, Default, Clone)]
pub struct LanguageWords(HashMap<u32, LanguagePool>);

/// One language's words in file order, indexed by length.
#[derive(Debug, Default, Clone)]
pub struct LanguagePool {
    words: Vec<String>,
    /// `by_len[n]`: indices of the words exactly `n` bytes long; slot 0 is empty. Bytes, as the
    /// reference's index build takes `strlen` (`0x4982c0`, `0x64a6f0`), and the source word it is
    /// matched against is arbitrary player text.
    by_len: Vec<Vec<u32>>,
}

impl LanguagePool {
    /// Every word in the pool, in file order.
    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// The words of exactly `len` bytes, in file order.
    pub fn of_len(&self, len: usize) -> impl Iterator<Item = &str> {
        self.by_len
            .get(len)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .map(|&i| self.words[i as usize].as_str())
    }

    /// How many words of exactly `len` bytes this language authored.
    pub fn count_of_len(&self, len: usize) -> usize {
        self.by_len.get(len).map_or(0, Vec::len)
    }

    /// The longest word this language authored, in bytes.
    pub fn max_len(&self) -> usize {
        self.by_len.len().saturating_sub(1)
    }

    /// The reference's substitute for a word of `len` bytes: `words[hash % count]` over that
    /// length's words in row order (`0x49b885`). `None` means no word that long, and the caller
    /// steps the length down and asks again.
    pub fn nth_of_len(&self, len: usize, hash: u32) -> Option<&String> {
        let bucket = self.by_len.get(len)?;
        let count = u32::try_from(bucket.len()).ok()?;
        if count == 0 {
            return None;
        }
        let idx = bucket[(hash % count) as usize];
        self.words.get(idx as usize)
    }
}

impl LanguageWords {
    /// A `Languages.dbc` id's pool. Universal (0) and `LANG_ADDON` (`-1`, tested at `0x49a89b`)
    /// have none and are never garbled.
    pub fn pool(&self, language: u32) -> Option<&LanguagePool> {
        self.0.get(&language)
    }

    /// How many languages have a pool.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn language_words_schema() -> Schema {
    let mut s = Schema::new("LanguageWords");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("LanguageID", FieldType::UInt32));
    s.add_field(SchemaField::new("Word", FieldType::String));
    s
}

/// Load the per-language substitution pools, indexed by byte length.
pub fn load_language_words(chain: &mut Chain) -> Result<LanguageWords> {
    let bytes = chain
        .read_file(LANGUAGE_WORDS)
        .with_context(|| format!("reading {LANGUAGE_WORDS}"))?;
    let table = parse(&bytes, language_words_schema(), "LanguageWords.dbc")?;
    let mut out: HashMap<u32, LanguagePool> = HashMap::new();
    for r in table.records() {
        let (Some(lang), Some(word)) = (u32_at(r, 1), str_at(&table, r, 2)) else {
            continue;
        };
        if word.is_empty() {
            continue;
        }
        let pool = out.entry(lang).or_default();
        let len = word.len();
        let idx = u32::try_from(pool.words.len()).unwrap_or(u32::MAX);
        pool.words.push(word);
        if pool.by_len.len() <= len {
            pool.by_len.resize(len + 1, Vec::new());
        }
        pool.by_len[len].push(idx);
    }
    Ok(LanguageWords(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_tables_join_to_two_faction_languages() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let langs = load_default_languages(&mut chain).expect("load");

        // All nine `ChrRaces` rows resolve: the eight playable races and the Goblin.
        assert_eq!(langs.len(), 9, "one entry per ChrRaces row");
        // Alliance speaks Common, Horde Orcish; the racial language would give Darnassian.
        for race in [1, 3, 4, 7] {
            assert_eq!(langs.name(race, 0), Some("Common"), "race {race}");
        }
        for race in [2, 5, 6, 8] {
            assert_eq!(langs.name(race, 0), Some("Orcish"), "race {race}");
        }
        // No such race: the reference's no-record edge.
        assert_eq!(langs.name(99, 0), None);
        // An enUS install fills only column 0; another is empty, not a panic.
        assert_eq!(langs.name(1, 5), None);
    }

    #[test]
    fn the_shipped_word_pools_cover_every_language_and_index_by_length() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let words = load_language_words(&mut chain).expect("load");

        // One pool per `Languages.dbc` row, so no spoken language lacks one.
        assert_eq!(words.len(), 13, "one pool per language");
        let langs = load_default_languages(&mut chain).expect("load languages");
        let _ = &langs; // the join test asserts this table
        for id in [1, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 33] {
            assert!(words.pool(id).is_some(), "language {id} has no word pool");
        }
        // Universal (0) has no pool: it is never garbled.
        assert!(words.pool(0).is_none());

        // Orcish (1) as shipped: 100 words, one to thirteen bytes, five of length one.
        let orcish = words.pool(1).expect("orcish pool");
        assert_eq!(orcish.words().len(), 100);
        assert_eq!(orcish.max_len(), 13);
        assert_eq!(orcish.count_of_len(1), 5);
        assert_eq!(
            orcish.of_len(1).collect::<Vec<_>>(),
            ["A", "N", "G", "O", "L"]
        );
        // Length zero and past the longest word are empty, not a panic.
        assert_eq!(orcish.count_of_len(0), 0);
        assert_eq!(orcish.count_of_len(99), 0);

        // Every pool holds at least 79 words, all of them in its length index.
        let mut total = 0;
        for id in [1, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 33] {
            let p = words.pool(id).expect("pool");
            assert!(p.words().len() >= 79, "language {id} pool is thin");
            let indexed: usize = (0..=p.max_len()).map(|n| p.count_of_len(n)).sum();
            assert_eq!(indexed, p.words().len(), "language {id} length index");
            total += p.words().len();
        }
        assert_eq!(total, 1481, "every LanguageWords row landed in a pool");
    }
}
