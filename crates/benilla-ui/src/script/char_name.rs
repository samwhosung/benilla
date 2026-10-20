//! The client's name validator `0x6c96c0` and its two wrappers: the character name check
//! `0x5abd50` (`0x6c9ae0` read as a `CHAR_NAME_*` status, `+0x45`), which the rename's send
//! (`0x46b36f`) and the create screen (`0x4711e2`, `0x4719ab`) call, and the guild name check
//! `0x6c9b70`, which the two charter verbs call through `0x4f5160`.
//!
//! The validator takes the client's locale, a list of extra characters (`edx`) and a flag per word
//! list; it answers a code with the name's unit count and the charset it chose, which each wrapper
//! caps. `0x6c9ae0` passes no extra characters (`0x6c9af5 xor edx,edx`) and all three lists
//! (`0x6c9ae6`-`0x6c9aea`); `0x6c9b70` passes a space alone (`0x6c9b88`) and the two profanity
//! lists (`0x6c9b76`-`0x6c9b7a`). The validator takes two charsets from `0x6c9a70`
//! ([`NameCharsets`]), and the first unit in either decides which one the whole name is held to.
//!
//! The checks, in the order the bytes make them, which decides the code of a name failing two:
//!
//! 1. A null name is `NO_NAME` (`0x6c96d6`).
//! 2. The UTF-8 is converted to UTF-16 into 0x400 units, the NUL included; a name that does not fit
//!    is `FAILURE` (`0x65b350` → `0x6c96fe` → `0x6c9a5f`).
//! 3. An empty name is `NO_NAME` (`0x6c9750` → `0x6c98a4`, the count still 0).
//! 4. Then one walk over the units, left to right (`0x6c9756`-`0x6c9897`), so the first offending
//!    unit decides. Until a unit has picked the name's charset, each is tried against the primary,
//!    then the secondary (`0x6c9764`-`0x6c97e3`), and the one that takes it is the name's
//!    (`0x6c9778`, `0x6c97eb`). A unit in the name's charset, from the third unit on, ends
//!    `THREE_CONSECUTIVE` when the last three are equal (`0x6c977a`-`0x6c97d2` → `0x6c98d4`):
//!    under the C library's `tolower` (`0x40f92b`, ASCII only) for charset 1 (`0x6c9788`), exactly
//!    for any other (`0x6c9874`-`0x6c9886`). Any other unit must be an extra character, else it is
//!    `INVALID_CHARACTER` (`0x6c9803` → `0x6c9827`). An apostrophe first is `INVALID_APOSTROPHE`
//!    and a second one `MULTIPLE_APOSTROPHES` (`0x6c9833`-`0x6c984f`); a space first is
//!    `INVALID_SPACE` and one right after a space `CONSECUTIVE_SPACES` (`0x6c9851`-`0x6c9872`).
//! 5. One unit is `TOO_SHORT` (`0x6c989f` → `0x6c98ac`); from two on, an apostrophe last is
//!    `INVALID_APOSTROPHE` and a space last `INVALID_SPACE` (`0x6c98e0`-`0x6c98f9`).
//! 6. The lists the flags turn on, NamesProfanity, ChatProfanity, NamesReserved (`0x6c9909`,
//!    `0x6c9977`, `0x6c99e1`): any match is `RESERVED` (`0x6c9a51`). `MIXED_LANGUAGES` and
//!    `PROFANE` are never produced.
//! 7. The wrapper's cap, checked last: `0x6c9ae0` refuses over the cap of the name's charset as
//!    `TOO_LONG` (`0x6c9b04`-`0x6c9b27`), 12 units for charset 1, else 8, and charset 3 at most 6;
//!    `0x6c9b70` over 24 units whatever the charset (`0x6c9b9e`).

mod hangul;

/// The three lists the validator can search, each row a PCRE compiled caseless (the shipped
/// `NamesProfanity.dbc`, `ChatProfanity.dbc` and `NamesReserved.dbc`), matched unanchored against
/// the name as typed, in UTF-8 (`0x71fc70` with `edx` the name, `0x6c9925`). The validator takes a
/// flag per list (`0x6c9909`, `0x6c9977`, `0x6c99e1`), so each is asked on its own.
pub trait NameLists {
    /// Whether a row of `NamesProfanity` (the list at `0xce8948`) matches `name`.
    fn names_profanity(&self, name: &str) -> bool;
    /// Whether a row of `ChatProfanity` (`0xce893c`) matches `name`.
    fn chat_profanity(&self, name: &str) -> bool;
    /// Whether a row of `NamesReserved` (`0xce8930`) matches `name`.
    fn names_reserved(&self, name: &str) -> bool;
}

/// No lists, for a VM whose host seated none: nothing is reserved.
pub struct NoNameLists;

impl NameLists for NoNameLists {
    fn names_profanity(&self, _: &str) -> bool {
        false
    }

    fn chat_profanity(&self, _: &str) -> bool {
        false
    }

    fn names_reserved(&self, _: &str) -> bool {
        false
    }
}

/// `CHAR_NAME_SUCCESS`, `0x45 + 0xd`, the one status the callers act on (`0x46b374`).
pub const CHAR_NAME_SUCCESS: u8 = 0x52;

/// The validator's codes (`0x6c96c0`'s returns, `ret 0x18`), before the `+0x45`.
pub(crate) const NO_NAME: u8 = 0;
pub(crate) const TOO_SHORT: u8 = 1;
const TOO_LONG: u8 = 2;
const INVALID_CHARACTER: u8 = 3;
/// `MIXED_LANGUAGES` (4) and `PROFANE` (5) are codes the callers map, which the validator never
/// returns.
pub(crate) const MIXED_LANGUAGES: u8 = 4;
pub(crate) const PROFANE: u8 = 5;
pub(crate) const RESERVED: u8 = 6;
const INVALID_APOSTROPHE: u8 = 7;
const MULTIPLE_APOSTROPHES: u8 = 8;
const THREE_CONSECUTIVE: u8 = 9;
pub(crate) const INVALID_SPACE: u8 = 0xa;
pub(crate) const CONSECUTIVE_SPACES: u8 = 0xb;
const FAILURE: u8 = 0xc;
pub(crate) const SUCCESS: u8 = 0xd;

/// The conversion buffer, in UTF-16 units with the NUL (`0x6c96eb push 0x400`).
const UNITS: usize = 0x400;

/// Charset 1's longest name (`0x6c9b07 mov ecx,0xc`); the others' is [`OTHER_MAX_UNITS`].
const LATIN_MAX_UNITS: usize = 12;
/// Any other charset's longest name (`0x6c9b0e mov ecx,0x8`).
const OTHER_MAX_UNITS: usize = 8;
/// Charset 3's, CJK ideographs, is cut to this (`0x6c9b13`-`0x6c9b1d`).
const CJK_MAX_UNITS: usize = 6;
/// A guild name's longest, whatever its charset (`0x6c9b9e cmp [ebp-0x8],0x18`).
const GUILD_MAX_UNITS: usize = 24;

/// The apostrophe and the space, the two extra characters the validator rules on.
const APOSTROPHE: u16 = 0x27;
const SPACE: u16 = 0x20;

/// Which lists a wrapper turns on, the validator's last three arguments in its order.
#[derive(Clone, Copy)]
struct Searched {
    names_profanity: bool,
    chat_profanity: bool,
    names_reserved: bool,
}

/// What `0x6c96c0` answers: its code, and through its two pointers the units it walked and the
/// charset it chose, which the wrappers' caps read (`[ebp-0x8]` and `[ebp-0x4]` of `0x6c9ae0`).
struct Verdict {
    code: u8,
    units: usize,
    charset: u8,
}

/// The charsets a name is checked against: the ids the validator's dispatch (`0x6c9dd0`) takes.
const LATIN_1: u8 = 1;
const HANGUL: u8 = 2;
const CJK: u8 = 3;
const ASCII_LETTERS: u8 = 4;

/// The two charsets `0x6c9a70` returns, the primary and the secondary; a name's first unit picks
/// whichever takes it, primary first, and every later unit must be of that one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameCharsets {
    primary: u8,
    secondary: u8,
}

impl NameCharsets {
    /// What `Wow.ini`'s `Region` and the locale index give. A `Region` of 1 to 5 (signed,
    /// `0x6c9a78`-`0x6c9a80`; `0x402b7f` hands it over at start-up, `0x6c91b8` stores it) reads the
    /// pairs at `0x86c9e4`: 1 (1, 1), 2 (2, 4), 3 (1, 1), 4 (3, 1), 5 (3, 1). Any other reads the
    /// locale's: the table at `0x86ca0c` (1, 2, 1, 1, 3, 3, 1, then 0, which reads as index 0)
    /// for the primary, and 4 for koKR's secondary, 1 for every other (`0x6c9aa1`-`0x6c9acb`).
    pub fn for_client(region: i32, locale: usize) -> Self {
        const REGIONS: [(u8, u8); 5] = [(1, 1), (2, 4), (1, 1), (3, 1), (3, 1)];
        const LOCALES: [u8; 8] = [1, 2, 1, 1, 3, 3, 1, 0];
        const KO_KR: usize = 1;
        if let 1..=5 = region {
            let (primary, secondary) = REGIONS[(region - 1) as usize];
            return Self { primary, secondary };
        }
        // An index past the table or whose entry is 0 reads row 0.
        let index = match LOCALES.get(locale) {
            Some(&entry) if entry != 0 => locale,
            _ => 0,
        };
        Self {
            primary: LOCALES[index],
            secondary: if index == KO_KR {
                ASCII_LETTERS
            } else {
                LATIN_1
            },
        }
    }
}

impl Default for NameCharsets {
    /// enUS's, and any locale that reads the Latin charset twice.
    fn default() -> Self {
        Self::for_client(0, 0)
    }
}

/// `0x5abd50`: the status of `name` (`CHAR_NAME_NO_NAME` 0x45 to `CHAR_NAME_SUCCESS` 0x52). `None`
/// is a null name.
pub fn check_char_name(name: Option<&str>, lists: &dyn NameLists, charsets: NameCharsets) -> u8 {
    // `0x6c9ae0`: no extra characters, every list, the charset's cap.
    let all = Searched {
        names_profanity: true,
        chat_profanity: true,
        names_reserved: true,
    };
    let v = validate(name, &[], lists, all, charsets);
    let cap = match v.charset {
        LATIN_1 => LATIN_MAX_UNITS,
        CJK => CJK_MAX_UNITS,
        _ => OTHER_MAX_UNITS,
    };
    0x45 + if v.code == SUCCESS && v.units > cap {
        TOO_LONG
    } else {
        v.code
    }
}

/// `0x6c9b70`: the validator's code for guild name `name`, [`SUCCESS`] for one it accepts.
pub(crate) fn check_guild_name(name: &str, lists: &dyn NameLists, charsets: NameCharsets) -> u8 {
    let profanity = Searched {
        names_profanity: true,
        chat_profanity: true,
        names_reserved: false,
    };
    let v = validate(Some(name), &[SPACE], lists, profanity, charsets);
    if v.code == SUCCESS && v.units > GUILD_MAX_UNITS {
        TOO_LONG
    } else {
        v.code
    }
}

/// `0x6c96c0` over `name`, taking the units of `extras` outside the charset and searching the
/// lists `searched` turns on.
fn validate(
    name: Option<&str>,
    extras: &[u16],
    lists: &dyn NameLists,
    searched: Searched,
    charsets: NameCharsets,
) -> Verdict {
    let mut v = Verdict {
        code: NO_NAME,
        units: 0,
        charset: 0,
    };
    let Some(name) = name else {
        return v;
    };
    let units: Vec<u16> = name.encode_utf16().collect();
    if units.len() >= UNITS {
        v.code = FAILURE;
        return v;
    }
    let fold = |u: u16| -> u16 {
        if (0x41..=0x5a).contains(&u) {
            u + 0x20
        } else {
            u
        }
    };
    // The last apostrophe's and the last space's index (`[ebp+0xc]`, `[ebp-0xc]`, -1 before).
    let mut apostrophe: Option<usize> = None;
    let mut space: Option<usize> = None;
    for (i, &unit) in units.iter().enumerate() {
        v.units = i + 1;
        if v.charset == 0 {
            v.charset = [charsets.primary, charsets.secondary]
                .into_iter()
                .find(|&set| in_charset(set, unit))
                .unwrap_or(0);
        }
        if v.charset != 0 && in_charset(v.charset, unit) {
            if i >= 2 {
                let (a, b) = (units[i - 2], units[i - 1]);
                let same = if v.charset == LATIN_1 {
                    fold(a) == fold(b) && fold(a) == fold(unit)
                } else {
                    a == b && a == unit
                };
                if same {
                    v.code = THREE_CONSECUTIVE;
                    return v;
                }
            }
            continue;
        }
        if !extras.contains(&unit) {
            v.code = INVALID_CHARACTER;
            return v;
        }
        let refused = match unit {
            APOSTROPHE if i == 0 => Some(INVALID_APOSTROPHE),
            APOSTROPHE if apostrophe.is_some() => Some(MULTIPLE_APOSTROPHES),
            APOSTROPHE => {
                apostrophe = Some(i);
                None
            }
            SPACE if i == 0 => Some(INVALID_SPACE),
            SPACE if space == Some(i - 1) => Some(CONSECUTIVE_SPACES),
            SPACE => {
                space = Some(i);
                None
            }
            _ => None,
        };
        if let Some(code) = refused {
            v.code = code;
            return v;
        }
    }
    let last = units.len().checked_sub(1);
    v.code = if units.len() < 2 {
        // 0 or 1 by the count (`0x6c98ac setne`).
        if units.is_empty() {
            NO_NAME
        } else {
            TOO_SHORT
        }
    } else if apostrophe == last {
        INVALID_APOSTROPHE
    } else if space == last {
        INVALID_SPACE
    } else if (searched.names_profanity && lists.names_profanity(name))
        || (searched.chat_profanity && lists.chat_profanity(name))
        || (searched.names_reserved && lists.names_reserved(name))
    {
        RESERVED
    } else {
        SUCCESS
    };
    v
}

/// Whether `unit` is in charset `set` (`0x6c9dd0`); an id it does not know, 0 included, takes none.
fn in_charset(set: u8, unit: u16) -> bool {
    match set {
        LATIN_1 => in_latin_1(unit),
        HANGUL => hangul::is_listed(unit),
        CJK => in_cjk(unit),
        ASCII_LETTERS => matches!(unit, 0x41..=0x5a | 0x61..=0x7a),
        _ => false,
    }
}

/// Charset 1 (`0x6c9c60`): A-Z, a-z, and `0xC0`-`0xFF` but `0xD7`, `0xDE` and `0xF7`.
fn in_latin_1(u: u16) -> bool {
    matches!(u, 0x41..=0x5a | 0x61..=0x7a | 0xc0..=0xff) && !matches!(u, 0xd7 | 0xde | 0xf7)
}

/// Charset 3 (`0x6c9d80`, through `0x6c9dc0`): the CJK ideographs, U+4E00-U+9FFF, the extension A
/// block U+3400-U+4DFF and the compatibility ideographs U+F900-U+FAFF.
fn in_cjk(u: u16) -> bool {
    matches!(u, 0x3400..=0x4dff | 0x4e00..=0x9fff | 0xf900..=0xfaff)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list that reserves any name holding one of its words, ASCII-folded.
    #[derive(Clone)]
    struct Words(&'static [&'static str]);

    impl NameLists for Words {
        fn names_profanity(&self, _: &str) -> bool {
            false
        }

        fn chat_profanity(&self, _: &str) -> bool {
            false
        }

        fn names_reserved(&self, name: &str) -> bool {
            let name = name.to_ascii_lowercase();
            self.0.iter().any(|w| name.contains(w))
        }
    }

    fn check(name: &str) -> u8 {
        check_char_name(
            Some(name),
            &Words(&["blizz"]),
            NameCharsets::for_client(0, 0),
        )
    }

    /// The status of `name` on a client of this `Region` and locale index.
    fn check_on(region: i32, locale: usize, name: &str) -> u8 {
        check_char_name(
            Some(name),
            &Words(&["blizz"]),
            NameCharsets::for_client(region, locale),
        )
    }

    /// One name per code the check produces.
    #[test]
    fn each_code_the_check_produces() {
        assert_eq!(
            check_char_name(None, &NoNameLists, NameCharsets::default()),
            0x45,
            "NO_NAME, a null name"
        );
        assert_eq!(check(""), 0x45, "NO_NAME");
        assert_eq!(check("A"), 0x46, "TOO_SHORT");
        assert_eq!(check("Abcdefghijklm"), 0x47, "TOO_LONG, 13 units");
        assert_eq!(check("Abc1"), 0x48, "INVALID_CHARACTER");
        assert_eq!(check("Ab cd"), 0x48, "a space is outside the charset");
        assert_eq!(check("Ab'cd"), 0x48, "an apostrophe is outside the charset");
        assert_eq!(check("Blizzbob"), 0x4b, "RESERVED");
        assert_eq!(check("Abbbc"), 0x4e, "THREE_CONSECUTIVE");
        assert_eq!(check(&"a".repeat(0x400)), 0x51, "FAILURE, 1024 units");
        assert_eq!(check("Ab"), 0x52, "SUCCESS");
        assert_eq!(check("Abcdefghijkl"), 0x52, "12 units is the cap");
    }

    /// The charset's edges, its Latin-1 half included, and its three holes.
    #[test]
    fn charset_1_is_the_letters_and_latin_1_less_three() {
        for ok in ["Zoë", "Ægir", "Øyvind", "Ðaß", "Ab\u{ff}", "Ab\u{c0}"] {
            assert_eq!(check(ok), 0x52, "{ok}");
        }
        for bad in [
            "Ab\u{d7}",
            "Ab\u{de}",
            "Ab\u{f7}",
            "Ab\u{bf}",
            "Ab\u{100}",
            "Ab_",
            "Abé1",
        ] {
            assert_eq!(check(bad), 0x48, "{bad:?}");
        }
    }

    /// Three in a row folds ASCII case (`0x40f92b`), and only ASCII: `Éé` is two units apart.
    #[test]
    fn three_in_a_row_folds_ascii_only() {
        assert_eq!(check("AaAb"), 0x4e);
        assert_eq!(check("Bbb"), 0x4e);
        assert_eq!(check("ÉéÉb"), 0x52);
        assert_eq!(check("ÉÉÉb"), 0x4e);
        assert_eq!(check("Aabb"), 0x52, "two pairs are not three");
    }

    /// The order, where two rules fail one name: the walk decides by position between an invalid
    /// unit and a triple, ahead of the short test, the lists and the length.
    #[test]
    fn the_order_of_the_checks() {
        // The walk, by position.
        assert_eq!(check("Aaa1"), 0x4e, "the triple ends before the digit");
        assert_eq!(check("A1aaa"), 0x48, "the digit comes before the triple");
        // The walk before the short test: one bad unit is INVALID, not TOO_SHORT.
        assert_eq!(check("1"), 0x48);
        // The walk before the lists, the lists before the length.
        assert_eq!(check("Blizz1"), 0x48);
        assert_eq!(check("Blizzz"), 0x4e);
        assert_eq!(
            check("Blizzardbobby"),
            0x4b,
            "13 units and reserved is RESERVED"
        );
        // The length last of all: a 13-unit name with a digit is INVALID.
        assert_eq!(check("Abcdefghijkl1"), 0x48);
        assert_eq!(check("Abcdefghijkkk"), 0x4e);
        // The conversion first: a name too long to convert fails whatever it holds.
        assert_eq!(check(&"1".repeat(0x400)), 0x51);
        assert_eq!(
            check(&"1".repeat(0x3ff)),
            0x48,
            "1023 units and the NUL fit"
        );
        // A surrogate pair is two units and outside the charset.
        assert_eq!(check("Ab\u{1f600}"), 0x48);
    }

    /// `0x6c9a70` over the bytes' tables: a `Region` of 1 to 5 picks its pair, any other value
    /// reads the locale's, and the Latin pair (1, 1) is every locale but koKR, zhCN and zhTW.
    #[test]
    fn the_pair_is_the_regions_else_the_locales() {
        let pair = |region, locale| {
            let c = NameCharsets::for_client(region, locale);
            (c.primary, c.secondary)
        };
        for (region, want) in [
            (1, (1, 1)),
            (2, (2, 4)),
            (3, (1, 1)),
            (4, (3, 1)),
            (5, (3, 1)),
        ] {
            for locale in 0..8 {
                assert_eq!(
                    pair(region, locale),
                    want,
                    "Region {region}, locale {locale}"
                );
            }
        }
        // locale index -> (primary, secondary): enUS, koKR, frFR, deDE, zhCN, zhTW, esES, xxYY.
        let by_locale = [
            (1, 1),
            (2, 4),
            (1, 1),
            (1, 1),
            (3, 1),
            (3, 1),
            (1, 1),
            (1, 1),
        ];
        for (locale, want) in by_locale.into_iter().enumerate() {
            for region in [0, 6, -1, 1000] {
                assert_eq!(
                    pair(region, locale),
                    want,
                    "Region {region}, locale {locale}"
                );
            }
        }
        // An index past the table reads row 0, as the client's does.
        assert_eq!(pair(0, 9), (1, 1));
        assert_eq!(NameCharsets::default(), NameCharsets::for_client(0, 0));
        // The extremes of the signed word are no region, not an overflow.
        assert_eq!(pair(i32::MIN, 1), (2, 4));
        assert_eq!(pair(i32::MAX, 4), (3, 1));
    }

    /// koKR: a Hangul name passes, and the same bytes fail on every Latin locale. The first
    /// unit picks the charset: a name opening in Latin letters is held to ASCII letters.
    #[test]
    fn a_hangul_name_passes_on_korean_and_fails_on_a_latin_locale() {
        for name in ["한글", "가나다라", "김철수"] {
            assert_eq!(check_on(0, 1, name), 0x52, "{name} on koKR");
            for locale in [0, 2, 3, 6] {
                assert_eq!(check_on(0, locale, name), 0x48, "{name} on locale {locale}");
            }
        }
        // The first unit opens the secondary (ASCII letters), so the accents of charset 1 and
        // Hangul after it are outside.
        assert_eq!(check_on(0, 1, "Abc"), 0x52);
        assert_eq!(check_on(0, 1, "Zoë"), 0x48);
        assert_eq!(check_on(0, 1, "A한"), 0x48);
        // Opening in Hangul, a Latin letter after it is outside the name's charset.
        assert_eq!(check_on(0, 1, "한a"), 0x48);
        // Only the 2,350 listed syllables: 갂 (U+AC02) is in the block but not the list.
        assert_eq!(check_on(0, 1, "\u{ac02}\u{ac00}"), 0x48);
        assert_eq!(check_on(0, 1, "\u{ac00}\u{ac01}"), 0x52);
        // The decomposed jamo are not the syllables.
        assert_eq!(check_on(0, 1, "\u{1112}\u{1161}"), 0x48);
    }

    /// zhCN and zhTW: ideographs pass, six at most, and Latin letters after the first
    /// ideograph are outside; a name opening in ASCII letters falls to the secondary, charset 1.
    #[test]
    fn a_chinese_name_passes_on_chinese_locales_up_to_six() {
        for locale in [4, 5] {
            assert_eq!(check_on(0, locale, "李小龙"), 0x52, "locale {locale}");
            assert_eq!(check_on(0, locale, "歐陽修"), 0x52);
            assert_eq!(check_on(0, locale, "一二三四五六"), 0x52, "six is the cap");
            assert_eq!(
                check_on(0, locale, "一二三四五六七"),
                0x47,
                "seven is TOO_LONG"
            );
            assert_eq!(check_on(0, locale, "李a"), 0x48);
            assert_eq!(
                check_on(0, locale, "Zoë"),
                0x52,
                "the secondary is charset 1"
            );
            assert_eq!(
                check_on(0, locale, "Abcdefghijkl"),
                0x52,
                "charset 1 keeps 12"
            );
            assert_eq!(check_on(0, locale, "한글"), 0x48);
        }
        // Extension A and the compatibility block are ideographs too.
        assert_eq!(check_on(0, 4, "\u{3400}\u{f900}"), 0x52);
        // The enUS client has none of this.
        assert_eq!(check_on(0, 0, "李小龙"), 0x48);
    }

    /// A `Region` overrides the locale: Region 2 is koKR's pair on an enUS install, Region 4 and 5
    /// zhCN's, and Region 1 or 3 Latin on a koKR install.
    #[test]
    fn the_region_overrides_the_locale() {
        assert_eq!(check_on(2, 0, "한글"), 0x52);
        assert_eq!(check_on(4, 0, "李小龙"), 0x52);
        assert_eq!(check_on(5, 0, "李小龙"), 0x52);
        for region in [1, 3] {
            assert_eq!(check_on(region, 1, "한글"), 0x48, "Region {region}");
            assert_eq!(check_on(region, 1, "Zoë"), 0x52, "Region {region}");
        }
        assert_eq!(
            check_on(6, 1, "한글"),
            0x52,
            "Region 6 is none: the locale's"
        );
    }

    /// The cap follows the chosen charset: 12 for charset 1, 8 for 2 and 4, 6 for 3.
    #[test]
    fn the_cap_follows_the_chosen_charset() {
        let hangul = |n: usize| "가".repeat(n);
        // The same syllable repeated trips the triple test, so vary it over listed syllables.
        let varied =
            |n: usize| ["가", "나", "다", "라", "마", "바", "사", "아", "자", "차"][..n].concat();
        assert_eq!(check_on(0, 1, &varied(8)), 0x52, "eight Hangul");
        assert_eq!(check_on(0, 1, &varied(9)), 0x47, "nine Hangul");
        // koKR's secondary, ASCII letters: also 8.
        assert_eq!(check_on(0, 1, "Abcdefgh"), 0x52);
        assert_eq!(
            check_on(0, 1, "Abcdefghi"),
            0x47,
            "charset 4 caps at 8, not 12"
        );
        // Latin on enUS stays 12.
        assert_eq!(check_on(0, 0, "Abcdefghijkl"), 0x52);
        assert_eq!(check_on(0, 0, "Abcdefghijklm"), 0x47);
        // The cap is last: three of one syllable is the triple, whatever the length.
        assert_eq!(check_on(0, 1, &hangul(3)), 0x4e);
    }

    /// The triple test folds ASCII case for charset 1 alone; every other charset compares exactly.
    #[test]
    fn the_triple_test_is_exact_outside_charset_1() {
        // Charset 1 on enUS: Aaa is three of one letter.
        assert_eq!(check_on(0, 0, "Aaab"), 0x4e);
        // koKR's secondary is charset 4, whose triple is exact: Aaa is not three alike.
        assert_eq!(check_on(0, 1, "Aaab"), 0x52);
        assert_eq!(check_on(0, 1, "aaab"), 0x4e);
        // Hangul and ideographs likewise.
        assert_eq!(check_on(0, 1, "가가가나"), 0x4e);
        assert_eq!(check_on(0, 1, "가가나가"), 0x52);
        assert_eq!(check_on(0, 4, "一一一二"), 0x4e);
        assert_eq!(check_on(0, 4, "一一二一"), 0x52);
    }

    /// The first offending unit decides across charsets as within one: a bad second unit is
    /// INVALID before the triple or the cap, and the lists come before the cap.
    #[test]
    fn the_order_holds_across_charsets() {
        assert_eq!(check_on(0, 1, "한1글"), 0x48);
        assert_eq!(check_on(0, 1, "가"), 0x46, "one syllable is TOO_SHORT");
        assert_eq!(check_on(0, 1, "1"), 0x48);
        assert_eq!(check_on(0, 1, "가나다라마바사아자1"), 0x48);
    }

    /// The extra characters' own rules (`0x6c9833`-`0x6c9872`, `0x6c98e0`-`0x6c98f9`), with both
    /// the apostrophe and the space passed: neither may open or close the name, an apostrophe
    /// comes once, a space never twice in a row, and any other unit outside the charset is
    /// invalid. The walk decides by position, before the short test and the lists.
    #[test]
    fn the_extra_characters_rules() {
        let code = |name: &str| {
            let all = Searched {
                names_profanity: true,
                chat_profanity: true,
                names_reserved: true,
            };
            validate(
                Some(name),
                &[APOSTROPHE, SPACE],
                &Words(&["blizz"]),
                all,
                NameCharsets::default(),
            )
            .code
        };
        for (name, want) in [
            ("'Ab", INVALID_APOSTROPHE),
            ("Ab'", INVALID_APOSTROPHE),
            ("A'b'c", MULTIPLE_APOSTROPHES),
            ("A''b", MULTIPLE_APOSTROPHES),
            ("A'b", SUCCESS),
            (" Ab", INVALID_SPACE),
            ("Ab ", INVALID_SPACE),
            ("A  b", CONSECUTIVE_SPACES),
            ("A b c", SUCCESS),
            ("A-b", INVALID_CHARACTER),
            ("'", INVALID_APOSTROPHE),
            (" ", INVALID_SPACE),
            ("Blizz b", RESERVED),
            ("A1  b", INVALID_CHARACTER),
            ("Aaa b", THREE_CONSECUTIVE),
        ] {
            assert_eq!(code(name), want, "{name:?}");
        }
    }

    /// `0x6c9b70`: a space allowed, the two profanity lists searched and the reserved one not,
    /// and 24 units whatever the charset.
    #[test]
    fn the_guild_check_caps_at_24_and_skips_the_reserved_list() {
        let guild =
            |name: &str| check_guild_name(name, &Words(&["blizz"]), NameCharsets::default());
        assert_eq!(
            guild("Blizzard Fans"),
            SUCCESS,
            "Words fills NamesReserved alone"
        );
        assert_eq!(guild("Abcdefghij Abcdefghij Ab"), SUCCESS, "24 units");
        assert_eq!(guild("Abcdefghij Abcdefghij Abc"), TOO_LONG, "25 units");
        assert_eq!(guild("Ab cd"), SUCCESS);
        assert_eq!(check("Ab cd"), 0x48, "the character check takes no space");
    }
}
