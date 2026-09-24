//! `Languages.dbc` × `ChrRaces.dbc` — the one join behind `GetDefaultLanguage()`.
//!
//! The binding `0x49fcd0` (94 bytes) is a two-hop table walk and nothing else:
//!
//! ```text
//! player race byte  ->  [0xc0dee0][race] + 0x20   =  base language id
//!                   ->  [0xc0db48][id]  + 4 + 4·[0xc0e080]  =  the localized name
//! ```
//!
//! `+0x20` on a `ChrRaces` record is byte offset 32, i.e. **field 8** — which the shipped 5875
//! data confirms independently: field 8 reads **7 for Human/Dwarf/Night Elf/Gnome/Goblin and 1 for
//! Orc/Undead/Tauren/Troll**, and `Languages.dbc` rows 7 and 1 are `Common` and `Orcish`. So the
//! "default" language is the **faction** language, not the racial one — a Night Elf's default is
//! Common, an Undead's is Orcish, and Darnassian/Gutterspeak are *additional* languages the
//! `GetLanguageByIndex` family enumerates. (Field 9, its neighbour, is `CreatureType` = 7 Humanoid
//! for every playable race — already anchored elsewhere in this crate's consumers, which is what
//! makes the field-8 identification a cross-check rather than a count.)
//!
//! Verified against the real chain below: 13 language rows, 9 race rows, and the nine races
//! resolving to exactly two distinct names.
//!
//! **Locale.** `[0xc0e080]` is the client's locale slot and only column 0 (enUS) is populated in
//! this install — every other DBC catalog in this crate reads column 0 for the same reason. The
//! loader keeps the whole locale row so a localized build has somewhere to go; the app asks for
//! the slot it wants.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const LANGUAGES: &str = "DBFilesClient\\Languages.dbc";
const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";
const LANGUAGE_WORDS: &str = "DBFilesClient\\LanguageWords.dbc";

/// How many locale columns a `Name_lang` block carries in 5875 (`Languages.dbc` is
/// `ID + 8 names + NameFlags` = 10 fields).
const LOCALES: usize = 8;

/// race id → the localized name of that race's base language, one entry per locale column.
#[derive(Debug, Default, Clone)]
pub struct DefaultLanguages(HashMap<u32, [Option<String>; LOCALES]>);

impl DefaultLanguages {
    /// The race's default chat language in `locale`'s column, or `None` — which is the reference's
    /// own answer shape at three of its four failure edges (no such race row, a language id past
    /// the table, a null record), all of which push **zero Lua values** rather than `nil`.
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

/// `Languages.dbc` — `ID`, eight `Name_lang` strings, `NameFlags`.
fn languages_schema() -> Schema {
    let mut s = Schema::new("Languages");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..LOCALES {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s
}

/// `ChrRaces.dbc` — 29 fields in 5875. Only field 8 (`BaseLanguage`) is read here, so the rest are
/// declared as plain dwords: the schema's field *count* is what has to match the header, and a
/// string column read as a dword is just its unresolved offset.
fn chr_races_schema() -> Schema {
    let mut s = Schema::new("ChrRaces");
    for i in 0..29 {
        s.add_field(SchemaField::new(format!("f{i}"), FieldType::UInt32));
    }
    s
}

/// The field 8 the binding reads at `record + 0x20` (module doc).
const CHR_RACES_BASE_LANGUAGE: usize = 8;

/// `Languages.dbc` as the client walks it — **row order**, `(ID, Name_lang[8])`. Row order is the
/// order `GetNumLaguages`/`GetLanguageByIndex` count in (`0x49fb30`/`0x49fbe0` walk the store's
/// rows and filter), so this keeps it rather than keying by id.
#[derive(Debug, Default, Clone)]
pub struct Languages(Vec<(u32, [Option<String>; LOCALES])>);

impl Languages {
    /// `(id, name)` per row in row order, for `locale`; a row with no name in that locale is
    /// skipped (the binding pushes `Name_lang[locale]` verbatim and a NULL would be `""`).
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

/// Load and join both tables into the race → language-name map.
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

/// `LanguageWords.dbc` — the fake-word pool the client substitutes, word for word, when it renders
/// speech in a language the listener's character does not know.
///
/// **The wire carries plaintext.** `SMSG_MESSAGECHAT` ships the real sentence plus a language id
/// and the server never rewrites it (vmangos `ChatHandler.cpp`); turning it into gibberish is
/// entirely the client's job, which is why an unmodelled garble step renders opposite-faction
/// speech perfectly readable (B262). The reference's garble routine is `0x49b560`, called from the
/// chat display chokepoint `0x49a870` at `0x49aa7c` — the same function whose `cmp edi,-0x1` at
/// `0x49a89b` is the `LANG_ADDON` test.
///
/// **The shipped table**, read off this install's chain: **1481 rows x 3 fields, 12-byte records**
/// — `ID`, `LanguageID`, `Word` — over **13 languages** (ids 1, 2, 3, 6-14, 33: the same id set
/// `Languages.dbc` carries, minus none and plus none). Each language's pool is 79-128 words, and
/// the words are authored in **runs of ascending length**, one to seventeen characters: Orcish
/// opens `A N G O L` (one letter), then `Ha Ko No Mu Ag Ka Gi` (two), and so on. That authored
/// shape is what makes a length index the natural read of the pool — a substitution that ignored
/// length would have no use for a table built this way.
#[derive(Debug, Default, Clone)]
pub struct LanguageWords(HashMap<u32, LanguagePool>);

/// One language's substitution pool: the words in file order, plus an index from word length to
/// the rows of exactly that length.
///
/// The index is **shape, not policy**. Storing which words are one character long commits us to
/// nothing about *how* the reference picks among them (exact-length match, a clamp at the longest
/// authored word, buckets); every one of those rules reads this index. The rule itself is
/// `0x49b560`'s and is filled in from the reference, not guessed here.
#[derive(Debug, Default, Clone)]
pub struct LanguagePool {
    words: Vec<String>,
    /// `by_len[n]` = indices into `words` of every word whose length is exactly `n` **bytes**; slot
    /// 0 is always empty (the index build skips an empty word) and the vector runs to the longest
    /// word this language authored.
    ///
    /// **Bytes, because the reference's index build takes `strlen`** (`0x4982c0`, a plain byte
    /// `0x64a6f0`), and the length key it later matches is likewise a byte count. Every shipped word
    /// is ASCII so the two agree in this table — but the key is compared against a *source* word,
    /// which is arbitrary player text, and there the distinction is real.
    by_len: Vec<Vec<u32>>,
}

impl LanguagePool {
    /// Every word in the pool, in file order.
    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// The words of exactly `len` bytes, in file order — empty when the language authored none
    /// that long (and for `len` past its longest word).
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

    /// The longest word this language authored, in bytes (0 for an empty pool).
    pub fn max_len(&self) -> usize {
        self.by_len.len().saturating_sub(1)
    }

    /// The substitute the reference picks for a word of `len` bytes whose hash is `hash`:
    /// `node.words[hash % node.count]` (`0x49b885`), over the candidates **in DBC row order** —
    /// which is the order the index builder appends them in, so it is the order this pool holds.
    ///
    /// `None` means the language authored no word of that length, which is the caller's cue to step
    /// the length key down and ask again.
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
    /// The substitution pool for a `Languages.dbc` id, or `None` for an id the table has no rows
    /// for — which includes `0` (universal) and `-1` (`LANG_ADDON`), neither of which is ever
    /// garbled.
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

/// `LanguageWords.dbc` — `ID`, `LanguageID`, `Word`.
fn language_words_schema() -> Schema {
    let mut s = Schema::new("LanguageWords");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("LanguageID", FieldType::UInt32));
    s.add_field(SchemaField::new("Word", FieldType::String));
    s
}

/// Load the per-language substitution pools.
///
/// Word length is counted in **bytes**, which is what the reference's index build uses
/// (`0x4982c0` takes `strlen`); see [`LanguagePool::by_len`].
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

    /// The real chain, because the whole point of this module is a join between two shipped
    /// tables — a synthetic fixture would only test the plumbing.
    #[test]
    fn the_shipped_tables_join_to_two_faction_languages() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let langs = load_default_languages(&mut chain).expect("load");

        // All nine `ChrRaces` rows resolve (the eight playable ones plus Goblin).
        assert_eq!(langs.len(), 9, "one entry per ChrRaces row");
        // Alliance → Common, Horde → Orcish. This is the finding that makes the field-8
        // identification falsifiable: if 8 were the *racial* language, Night Elf would read
        // "Darnassian" and Undead "Gutterspeak".
        for race in [1, 3, 4, 7] {
            assert_eq!(langs.name(race, 0), Some("Common"), "race {race}");
        }
        for race in [2, 5, 6, 8] {
            assert_eq!(langs.name(race, 0), Some("Orcish"), "race {race}");
        }
        // A race the table has no row for is `None` — the reference's "no such record" edge.
        assert_eq!(langs.name(99, 0), None);
        // Only enUS is populated in this install; a higher locale column is empty, not a panic.
        assert_eq!(langs.name(1, 5), None);
    }

    /// The garble pool, against the real chain for the same reason as above — the shape of the
    /// shipped table *is* the finding, and a fixture would only test the parser.
    #[test]
    fn the_shipped_word_pools_cover_every_language_and_index_by_length() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let words = load_language_words(&mut chain).expect("load");

        // One pool per `Languages.dbc` row — the id sets match exactly, so no spoken language can
        // reach the garble step and find nothing to substitute.
        assert_eq!(words.len(), 13, "one pool per language");
        let langs = load_default_languages(&mut chain).expect("load languages");
        let _ = &langs; // the join above already asserts that table; here only the count matters.
        for id in [1, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 33] {
            assert!(words.pool(id).is_some(), "language {id} has no word pool");
        }
        // 0 (universal) and the addon sentinel are absent by construction, never garbled.
        assert!(words.pool(0).is_none());

        // Orcish (1), the shipped numbers: 100 words, the shortest one character, the longest
        // thirteen, and the one-character run is the five vowels-and-consonants `A N G O L`.
        let orcish = words.pool(1).expect("orcish pool");
        assert_eq!(orcish.words().len(), 100);
        assert_eq!(orcish.max_len(), 13);
        assert_eq!(orcish.count_of_len(1), 5);
        assert_eq!(
            orcish.of_len(1).collect::<Vec<_>>(),
            ["A", "N", "G", "O", "L"]
        );
        // Nothing is authored at length zero, and asking past the longest word is empty, not a
        // panic — the two edges any length rule will lean on.
        assert_eq!(orcish.count_of_len(0), 0);
        assert_eq!(orcish.count_of_len(99), 0);

        // Every pool is non-trivial and its length index accounts for every word it holds.
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
