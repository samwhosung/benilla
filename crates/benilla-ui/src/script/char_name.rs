//! The client's character name check, `0x5abd50`: the validator `0x6c96c0` over the name, then the
//! length cap of `0x6c9ae0`, read as a `CHAR_NAME_*` status (`+0x45`). The rename's send
//! (`0x46b36f`) and the create screen (`0x4711e2`, `0x4719ab`) call it, so it is one function.
//!
//! `0x6c9ae0` runs the validator with no extra characters (`0x6c9af5 xor edx,edx`) and all three
//! list flags set (`0x6c9ae6`-`0x6c9aea`). The charsets come from `Wow.ini`'s `[WoW Config]`
//! `Region` when it is 1 to 5 (`0x4024ac` → `[0x882730]`, stored at `0x6c91b8` to `[0x86c9e0]`,
//! picked from `0x86c9dc`), else from the locale: for enUS (index 0, `0x8558a4`) both are 1
//! (`0x6c9a70`, table `0x86ca0c`). benilla builds the enUS case with no `Region`, so charset 1 is
//! the only one built, and its cap is 12 units (`0x6c9b07`).
//!
//! The checks, in the order the bytes make them, which decides the code of a name failing two:
//!
//! 1. A null name is `NO_NAME` (`0x6c96d6`).
//! 2. The UTF-8 is converted to UTF-16 into 0x400 units, the NUL included; a name that does not fit
//!    is `FAILURE` (`0x65b350` → `0x6c96fe` → `0x6c9a5f`).
//! 3. An empty name is `NO_NAME` (`0x6c9750` → `0x6c98a4`, the count still 0).
//! 4. Then one walk over the units, left to right (`0x6c9756`-`0x6c9897`), so the first offending
//!    unit decides: a unit outside the charset is `INVALID_CHARACTER` (`0x6c97ef` → `0x6c9803`;
//!    the extra list is empty, so space and apostrophe are outside too, `0x6c9827`); from the third
//!    unit on, the last three equal under the C library's `tolower` (`0x40f92b`, ASCII only) are
//!    `THREE_CONSECUTIVE` (`0x6c977a`-`0x6c97d2` → `0x6c98d4`).
//! 5. One unit is `TOO_SHORT` (`0x6c989f` → `0x6c98ac`).
//! 6. The lists, NamesProfanity, ChatProfanity, NamesReserved (`0x6c9909`, `0x6c9977`, `0x6c99e1`):
//!    any match is `RESERVED` (`0x6c9a51`). `MIXED_LANGUAGES` and `PROFANE` are never produced.
//! 7. Over 12 units is `TOO_LONG`, checked last (`0x6c9b22`).

/// The three lists the check searches, each row a PCRE compiled caseless (the shipped
/// `NamesProfanity.dbc`, `ChatProfanity.dbc` and `NamesReserved.dbc`), matched unanchored against
/// the name as typed, in UTF-8 (`0x71fc70` with `edx` the name, `0x6c9925`).
pub trait NameLists {
    /// Whether any row of any of the three lists matches `name`.
    fn matches(&self, name: &str) -> bool;
}

/// No lists, for a VM whose host seated none: nothing is reserved.
pub struct NoNameLists;

impl NameLists for NoNameLists {
    fn matches(&self, _: &str) -> bool {
        false
    }
}

/// `CHAR_NAME_SUCCESS`, `0x45 + 0xd`, the one status the callers act on (`0x46b374`).
pub const CHAR_NAME_SUCCESS: u8 = 0x52;

/// The validator's codes (`0x6c96c0`'s returns), before the `+0x45`.
const NO_NAME: u8 = 0;
const TOO_SHORT: u8 = 1;
const TOO_LONG: u8 = 2;
const INVALID_CHARACTER: u8 = 3;
const RESERVED: u8 = 6;
const THREE_CONSECUTIVE: u8 = 9;
const FAILURE: u8 = 0xc;
const SUCCESS: u8 = 0xd;

/// The conversion buffer, in UTF-16 units with the NUL (`0x6c96eb push 0x400`).
const UNITS: usize = 0x400;

/// Charset 1's longest name (`0x6c9b07 mov ecx,0xc`).
const MAX_UNITS: usize = 12;

/// `0x5abd50`: the status of `name` (`CHAR_NAME_NO_NAME` 0x45 to `CHAR_NAME_SUCCESS` 0x52). `None`
/// is a null name.
pub fn check_char_name(name: Option<&str>, lists: &dyn NameLists) -> u8 {
    0x45 + validate(name, lists)
}

fn validate(name: Option<&str>, lists: &dyn NameLists) -> u8 {
    let Some(name) = name else {
        return NO_NAME;
    };
    let units: Vec<u16> = name.encode_utf16().collect();
    if units.len() >= UNITS {
        return FAILURE;
    }
    if units.is_empty() {
        return NO_NAME;
    }
    let fold = |u: u16| -> u16 {
        if (0x41..=0x5a).contains(&u) {
            u + 0x20
        } else {
            u
        }
    };
    for (i, &unit) in units.iter().enumerate() {
        if !in_charset_1(unit) {
            return INVALID_CHARACTER;
        }
        if i >= 2 && fold(units[i - 2]) == fold(units[i - 1]) && fold(units[i - 2]) == fold(unit) {
            return THREE_CONSECUTIVE;
        }
    }
    if units.len() == 1 {
        return TOO_SHORT;
    }
    if lists.matches(name) {
        return RESERVED;
    }
    if units.len() > MAX_UNITS {
        return TOO_LONG;
    }
    SUCCESS
}

/// Charset 1 (`0x6c9c60`): A-Z, a-z, and `0xC0`-`0xFF` but `0xD7`, `0xDE` and `0xF7`.
fn in_charset_1(u: u16) -> bool {
    matches!(u, 0x41..=0x5a | 0x61..=0x7a | 0xc0..=0xff) && !matches!(u, 0xd7 | 0xde | 0xf7)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list that reserves any name holding one of its words, ASCII-folded.
    struct Words(&'static [&'static str]);

    impl NameLists for Words {
        fn matches(&self, name: &str) -> bool {
            let name = name.to_ascii_lowercase();
            self.0.iter().any(|w| name.contains(w))
        }
    }

    fn check(name: &str) -> u8 {
        check_char_name(Some(name), &Words(&["blizz"]))
    }

    /// One name per code the check produces.
    #[test]
    fn each_code_the_check_produces() {
        assert_eq!(
            check_char_name(None, &NoNameLists),
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
}
