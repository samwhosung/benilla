//! The `/who` builder `0x5aebb0` token by token, read back off the `CMSG_WHO` body it sends: the
//! copy and its appended space (`0x5aebf2`-`0x5aec1a`), the quotes (`0x5aec90`), the five
//! localized tags in their order (`0x5aecd5`-`0x5aeee8`), the substring searches (`0x64b540`), the
//! level ranges (`0x5aef7d`-`0x5aeff2`) and the stop at the fourth term (`0x5af02d`).

use benilla_formats::{AreaTableCatalog, AreaTableRow};

use super::*;
use crate::ui_unit::TestNames;

/// The enUS `WHO_TAG_NAME`, `_GUILD`, `_ZONE`, `_RACE` and `_CLASS`.
const EN_TAGS: [&str; 5] = ["n-", "g-", "z-", "r-", "c-"];

/// The `CMSG_WHO` body, read back field by field.
#[derive(Debug, PartialEq, Eq)]
struct Sent {
    min: u32,
    max: u32,
    name: Vec<u8>,
    guild: Vec<u8>,
    races: u32,
    classes: u32,
    zones: Vec<u32>,
    terms: Vec<Vec<u8>>,
}

impl Default for Sent {
    /// The builder's defaults: levels 0 (`0x5aebd7`) to 100 (`0x5aebda`), both masks all ones
    /// (`0x5aebbb`), no zone, no term.
    fn default() -> Self {
        Self {
            min: 0,
            max: 100,
            name: Vec::new(),
            guild: Vec::new(),
            races: !0,
            classes: !0,
            zones: Vec::new(),
            terms: Vec::new(),
        }
    }
}

fn read(body: &[u8]) -> Sent {
    let mut at = 0;
    let mut u32_ = || {
        let v = u32::from_le_bytes(body[at..at + 4].try_into().unwrap());
        at += 4;
        v
    };
    let min = u32_();
    let max = u32_();
    let cstr = |at: &mut usize| {
        let n = body[*at..].iter().position(|&b| b == 0).unwrap();
        let s = body[*at..*at + n].to_vec();
        *at += n + 1;
        s
    };
    let name = cstr(&mut at);
    let guild = cstr(&mut at);
    let word = |at: &mut usize| {
        let v = u32::from_le_bytes(body[*at..*at + 4].try_into().unwrap());
        *at += 4;
        v
    };
    let races = word(&mut at);
    let classes = word(&mut at);
    let zones = (0..word(&mut at)).map(|_| word(&mut at)).collect();
    let terms = (0..word(&mut at)).map(|_| cstr(&mut at)).collect();
    assert_eq!(at, body.len(), "nothing after the terms");
    Sent {
        min,
        max,
        name,
        guild,
        races,
        classes,
        zones,
        terms,
    }
}

/// What the builder sends for `filter` under `tags`, over `areas` and the race and class rows.
fn sent_with(
    filter: &[u8],
    tags: [&str; 5],
    areas: &AreaTableCatalog,
    names: crate::ui_unit::UnitNames<'_>,
) -> Sent {
    let tags = WhoTags::from_texts(tags);
    let request = parse(filter, &tags, Some(areas), names);
    read(&benilla_protocol::messages::who(&request))
}

fn top(name: &str) -> AreaTableRow {
    row(0, name)
}

fn row(zone_id: u32, name: &str) -> AreaTableRow {
    AreaTableRow {
        map_id: 0,
        zone_id,
        explore_flag: 0,
        flags: 0,
        faction_group_mask: 0,
        exploration_level: 0,
        name: name.to_string(),
    }
}

/// Elwynn with a subzone under it, and the two top-level Westfalls of 1.12's table (40 on
/// Azeroth, 206 for the Deadmines' map), in that record order.
fn areas() -> AreaTableCatalog {
    AreaTableCatalog::from_rows(vec![
        (12, top("Elwynn Forest")),
        (87, row(12, "Goldshire")),
        (40, top("Westfall")),
        (206, top("Westfall")),
        (1, top("Dun Morogh")),
    ])
}

fn sent(filter: &str) -> Sent {
    sent_with(
        filter.as_bytes(),
        EN_TAGS,
        &areas(),
        TestNames::english().names(),
    )
}

fn terms(words: &[&str]) -> Vec<Vec<u8>> {
    words.iter().map(|w| w.as_bytes().to_vec()).collect()
}

#[test]
fn an_empty_filter_sends_the_defaults() {
    assert_eq!(sent(""), Sent::default());
    assert_eq!(sent("   "), Sent::default());
    assert_eq!(sent("\t"), Sent::default());
}

#[test]
fn name_and_guild_take_the_text_after_their_tag() {
    let q = sent("n-bob g-\"Legacy of Steel\" 10-20");
    assert_eq!(q.name, b"bob");
    assert_eq!(q.guild, b"Legacy of Steel");
    assert_eq!((q.min, q.max), (10, 20));
    assert!(q.terms.is_empty());
    // Without case (`0x64a4c0` folds `A`-`Z`), and the last of a kind wins.
    assert_eq!(sent("N-bob n-al").name, b"al");
    // `SStrCopy` into 48 bytes (`0x5aed0a`, `0x5aed49`): 47 kept.
    let long = "x".repeat(60);
    assert_eq!(sent(&format!("n-{long}")).name, &long.as_bytes()[..47]);
    assert_eq!(sent(&format!("g-{long}")).guild, &long.as_bytes()[..47]);
}

/// Only the five localized tags are tags; any other token is kept whole as a term.
#[test]
fn a_token_with_no_tag_is_a_term_whole() {
    assert_eq!(sent("x-foo").terms, terms(&["x-foo"]));
    assert_eq!(sent("a-10").terms, terms(&["a-10"]));
    assert_eq!(sent("well-met").terms, terms(&["well-met"]));
    assert_eq!(sent("-").terms, terms(&["-"]));
    assert_eq!(sent("-x").terms, terms(&["-x"]));
}

/// The tags are `FrameScript_GetText`'s answers for the five keys, whatever the locale makes of
/// them; the enUS letters are then no tags.
#[test]
fn the_tags_are_the_localized_strings() {
    let tags = ["nom-", "guilde-", "zone-", "race-", "classe-"];
    let q = sent_with(
        b"nom-Bob n-Al ZONE-elwynn",
        tags,
        &areas(),
        TestNames::english().names(),
    );
    assert_eq!(q.name, b"Bob");
    assert_eq!(q.terms, terms(&["n-Al"]));
    assert_eq!(q.zones, [12]);
}

#[test]
fn levels_read_as_typed_with_no_clamp() {
    let levels = |f: &str| {
        let q = sent(f);
        assert!(q.terms.is_empty(), "{f}: a range, not a term");
        (q.min, q.max)
    };
    assert_eq!(levels("60"), (60, 60));
    assert_eq!(levels("10-20"), (10, 20));
    // `N-` is N..100 (`0x5aefc4`), `-M` the default minimum to M (`0x5aeff2`).
    assert_eq!(levels("55-"), (55, 100));
    assert_eq!(levels("-20"), (0, 20));
    assert_eq!(levels("10 -20"), (10, 20), "-M keeps an earlier minimum");
    // Sent as typed: reversed, zero and past 100.
    assert_eq!(levels("20-10"), (20, 10));
    assert_eq!(levels("0-9999"), (0, 9999));
    // A digit-start token is a range whatever follows its digits.
    assert_eq!(levels("10x"), (10, 10));
    assert_eq!(levels("10-x"), (10, 100));
}

/// `0x64b540` over every row's localized name (`ChrRaces +0x44`, `ChrClasses +0x14`): the first
/// tag of a kind clears its mask (`0x5aee59`, `0x5aef06`) and every row containing the text adds
/// its bit (`0x5aeea1`, `0x5aef50`).
#[test]
fn race_and_class_take_every_name_containing_the_text() {
    assert_eq!(
        sent("c-war").classes,
        (1 << 1) | (1 << 9),
        "Warrior and Warlock"
    );
    assert_eq!(sent("c-WARRIOR").classes, 1 << 1);
    assert_eq!(sent("r-\"night elf\"").races, 1 << 4);
    assert_eq!(sent("c-mage c-warlock").classes, (1 << 8) | (1 << 9));
    // The file token is no name, and a word that names nothing sends a mask that matches nobody.
    let q = sent("r-nightelf c-necromancer");
    assert_eq!((q.races, q.classes), (0, 0));
    assert!(q.terms.is_empty(), "the tagged word is consumed");
    // An empty text is in every name (`0x64b540` with an empty needle).
    assert_eq!(
        sent("c-").classes,
        [1, 2, 3, 4, 5, 7, 8, 9, 11]
            .iter()
            .fold(0, |m, id| m | 1 << id)
    );
    // The install's words: 마법사 is in both 마법사 and 흑마법사.
    let ko = TestNames::korean();
    let q = sent_with("c-마법사".as_bytes(), EN_TAGS, &areas(), ko.names());
    assert_eq!(q.classes, (1 << 8) | (1 << 9));
}

/// The zone walk (`0x5aed95`-`0x5aedff`): rows with no parent (`0x5aedc1`), in record order,
/// whose name contains the text, at most 10 (`0x5aedc8`); none sends zone 0 (`0x5aee08`).
#[test]
fn a_zone_takes_every_top_level_row_containing_the_text() {
    assert_eq!(
        sent("z-westfall").zones,
        [40, 206],
        "both rows, in table order"
    );
    assert_eq!(sent("z-forest").zones, [12]);
    assert_eq!(sent("z-gold").zones, [0], "a subzone is never searched");
    let q = sent("z-nowhere");
    assert_eq!(q.zones, [0]);
    assert!(q.terms.is_empty(), "the tagged word is consumed");
    // The list carries over from tag to tag.
    assert_eq!(sent("z-nowhere z-elwynn").zones, [0, 12]);
    assert_eq!(sent("z-elwynn z-nowhere").zones, [12]);

    let many = AreaTableCatalog::from_rows((1..=12).map(|id| (id, top("Zone"))).collect());
    let q = sent_with(
        b"z-zone z-zone",
        EN_TAGS,
        &many,
        TestNames::english().names(),
    );
    assert_eq!(q.zones, (1..=10).collect::<Vec<_>>());
}

#[test]
fn quotes_toggle_and_are_dropped() {
    assert_eq!(sent("a\"b c\"d").terms, terms(&["ab cd"]));
    assert_eq!(
        sent("\"\"").terms,
        terms(&[""]),
        "an empty quoted token is a term"
    );
    assert_eq!(sent("z-\"Elwynn Forest\" 1-10").zones, [12]);
}

/// The space the copy appends (`0x5aec12`) goes into a token still quoted, and the scan ends at the
/// NUL (`0x5af099`) without processing it.
#[test]
fn a_quote_open_at_the_end_drops_its_token() {
    assert_eq!(sent("a \"b c").terms, terms(&["a"]));
    assert_eq!(sent("g-\"Foo").guild, b"");
    assert_eq!(sent("n-\"bo").name, b"");
}

/// `0x5af02d`: the scan ends at the fourth term, so nothing after it is read.
#[test]
fn the_fourth_term_ends_the_scan() {
    assert_eq!(sent("a b c d e").terms, terms(&["a", "b", "c", "d"]));
    let q = sent("a b c d 60 n-bob");
    assert_eq!((q.min, q.max), (0, 100));
    assert_eq!(q.name, b"");
    let q = sent("60 n-bob a b c d");
    assert_eq!((q.min, q.max, q.name.as_slice()), (60, 60, &b"bob"[..]));
}

/// The copy keeps 127 bytes (`SStrCopy` with `0x80`, `0x5aebf2`), then the space.
#[test]
fn the_filter_is_cut_at_127_bytes() {
    let long = "a".repeat(130);
    assert_eq!(sent(&long).terms, vec![long.as_bytes()[..127].to_vec()]);
}

/// `sgetu8` (`0x41aab0`) and `iswspace` (`0x74006f`) of its code point: tab and the ideographic
/// space separate, the no-break space does not (the C table has no bit from `0x80` on), and a
/// lead byte takes the space that breaks it into the token.
#[test]
fn white_space_is_the_code_points() {
    assert_eq!(sent("a\tb").terms, terms(&["a", "b"]));
    assert_eq!(sent("a\u{3000}b").terms, terms(&["a", "b"]));
    assert_eq!(sent("a\u{a0}b").terms, terms(&["a\u{a0}b"]));
    let q = sent_with(b"\xc3 b", EN_TAGS, &areas(), TestNames::english().names());
    assert_eq!(q.terms, vec![b"\xc3 b".to_vec()]);
}

/// `_isdigit` (`0x73fc0f`) reads `0xf7`, `0xfb` and `0xfd` as digits from the bytes before its
/// table, so after a range's dash or a leading `-` they set the maximum to `SStrToInt`'s 0 for no
/// digits; `iswdigit` (`0x740050`) begins no range at a high byte. Each is a broken lead, so it
/// takes the byte after it into the token (`0x41aab0`).
#[test]
fn three_high_bytes_pass_the_second_digit_test() {
    let bytes = |f: &[u8]| sent_with(f, EN_TAGS, &areas(), TestNames::english().names());
    let q = bytes(b"10-\xf7a");
    assert_eq!((q.min, q.max), (10, 0));
    let q = bytes(b"-\xfba");
    assert_eq!((q.min, q.max, q.terms.len()), (0, 0, 0));
    assert_eq!(bytes(b"-\xfca").terms, vec![b"-\xfca".to_vec()]);
    assert_eq!(bytes(b"\xfda").terms, vec![b"\xfda".to_vec()]);
    // Alone, the lead takes the appended space and the token is still open at the NUL.
    assert_eq!(bytes(b"a \xfd").terms, vec![b"a".to_vec()]);
}
