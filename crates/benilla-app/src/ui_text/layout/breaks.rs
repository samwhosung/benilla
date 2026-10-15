//! The line-break classifier of the reference's line kernel (`0x5c7780`): whether a line may break
//! between two characters. [`super::wrap`] asks it about each glyph pair.

/// The reference's `iswspace` (`0x74006f` → `0x7458e4`) of a code point's low 16 bits. Below
/// U+0100 it reads the CRT's ctype table: `0x09`-`0x0d` and `0x20` (U+00A0 is none). From U+0100 it
/// defers to Windows' `GetStringTypeW` CT_CTYPE1 (`0x745918` → `0x413ed3`), whose C1_SPACE code
/// points above are U+1680, U+2000-U+200A, U+2028, U+2029, U+202F, U+205F and U+3000.
pub(super) fn is_space(c: char) -> bool {
    matches!(
        c as u32 & 0xffff,
        0x09..=0x0d | 0x20 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000
    )
}

/// Whether a line may break between `prev` and `cur` (`0x5c7780(prev, cur)`), tested in the
/// reference's order. `None` is the kernel's 0, a glyph with nothing before it on the line:
/// - after `-`, `:` or `/`, whatever follows (`0x5c778e`, `0x5c779b`, `0x5c77a8`);
/// - never right after whitespace (`0x5c77c3`), always before it (`0x5c77d4`);
/// - never after an opening bracket or quote ([`is_opener`], from `0x5c77e8`) or before a closing
///   mark ([`is_closer`], from `0x5c7814`);
/// - before a character of the East Asian ranges, Hangul, CJK, compatibility and the half and
///   full width forms (`0x5c7974`-`0x5c79cc`);
/// - else not.
pub(super) fn may_break(prev: Option<char>, cur: char) -> bool {
    let Some(prev) = prev else {
        return false; // 0x5c7785
    };
    if matches!(prev, '-' | ':' | '/') {
        return true;
    }
    if is_space(prev) {
        return false;
    }
    if is_space(cur) {
        return true;
    }
    if is_opener(prev) || is_closer(cur) {
        return false;
    }
    matches!(
        cur as u32,
        0x1100..=0x11ff | 0x3000..=0xd7af | 0xf900..=0xfaff | 0xff00..=0xff9f | 0xffa0..=0xffdc
    )
}

/// No break after these (the tree at `0x5c77e8` and the table at `0x5c7acc`).
pub(super) fn is_opener(c: char) -> bool {
    matches!(
        c,
        '$' | '('
            | '['
            | '\\'
            | '{'
            | '\u{2018}'
            | '\u{201c}'
            | '\u{2035}'
            | '\u{3008}'
            | '\u{300a}'
            | '\u{300c}'
            | '\u{300e}'
            | '\u{3010}'
            | '\u{3014}'
            | '\u{301d}'
            | '\u{fe59}'
            | '\u{fe5b}'
            | '\u{fe5d}'
            | '\u{ff04}'
            | '\u{ff08}'
            | '\u{ff3b}'
            | '\u{ff5b}'
            | '\u{ffe1}'
            | '\u{ffe5}'
            | '\u{ffe6}'
    )
}

/// No break before these (the tree at `0x5c7814` and the tables at `0x5c7a2c`, `0x5c7bb4`,
/// `0x5c7bd8`, `0x5c7bf0` and `0x5c7ca8`).
pub(super) fn is_closer(c: char) -> bool {
    matches!(
        c,
        '!' | '%' | ')' | ',' | '.' | ':' | ';' | '?' | ']' | '}' | '\u{b0}' | '\u{b7}'
            | '\u{2013}' | '\u{2014}' | '\u{2019}' | '\u{201d}' | '\u{2022}' | '\u{2026}'
            | '\u{2027}' | '\u{2032}' | '\u{2033}' | '\u{2103}' | '\u{3001}' | '\u{3002}'
            | '\u{3009}' | '\u{300b}' | '\u{300d}' | '\u{300f}' | '\u{3011}' | '\u{3015}'
            | '\u{301e}' | '\u{30fc}' | '\u{fe30}' | '\u{fe50}'..='\u{fe52}'
            | '\u{fe54}'..='\u{fe57}' | '\u{fe5a}' | '\u{fe5c}' | '\u{fe5e}' | '\u{ff01}'
            | '\u{ff05}' | '\u{ff09}' | '\u{ff0c}' | '\u{ff0e}' | '\u{ff1a}' | '\u{ff1b}'
            | '\u{ff1f}' | '\u{ff3d}' | '\u{ff5d}' | '\u{ff70}' | '\u{ff9e}' | '\u{ff9f}'
            | '\u{ffe0}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sets are exactly the members the classifier's jump tables and trees hold (read off the
    /// exe), so a stray entry or a lost one fails here.
    #[test]
    fn the_sets_are_the_reference_sets() {
        let members = |f: fn(char) -> bool| -> Vec<u32> {
            (0..=0xffffu32)
                .filter(|&u| char::from_u32(u).is_some_and(f))
                .collect()
        };
        assert_eq!(
            members(is_opener),
            [
                0x24, 0x28, 0x5b, 0x5c, 0x7b, 0x2018, 0x201c, 0x2035, 0x3008, 0x300a, 0x300c,
                0x300e, 0x3010, 0x3014, 0x301d, 0xfe59, 0xfe5b, 0xfe5d, 0xff04, 0xff08, 0xff3b,
                0xff5b, 0xffe1, 0xffe5, 0xffe6
            ]
        );
        assert_eq!(
            members(is_closer),
            [
                0x21, 0x25, 0x29, 0x2c, 0x2e, 0x3a, 0x3b, 0x3f, 0x5d, 0x7d, 0xb0, 0xb7, 0x2013,
                0x2014, 0x2019, 0x201d, 0x2022, 0x2026, 0x2027, 0x2032, 0x2033, 0x2103, 0x3001,
                0x3002, 0x3009, 0x300b, 0x300d, 0x300f, 0x3011, 0x3015, 0x301e, 0x30fc, 0xfe30,
                0xfe50, 0xfe51, 0xfe52, 0xfe54, 0xfe55, 0xfe56, 0xfe57, 0xfe5a, 0xfe5c, 0xfe5e,
                0xff01, 0xff05, 0xff09, 0xff0c, 0xff0e, 0xff1a, 0xff1b, 0xff1f, 0xff3d, 0xff5d,
                0xff70, 0xff9e, 0xff9f, 0xffe0
            ]
        );
    }

    #[test]
    fn nothing_before_the_first_glyph_and_the_pair_rules_in_order() {
        assert!(!may_break(None, 'a'));
        assert!(!may_break(None, ' '));
        // After `-`, `:` and `/`, whatever follows, even a closer or a space.
        assert!(may_break(Some('-'), 'c'));
        assert!(may_break(Some(':'), '1'));
        assert!(may_break(Some('/'), ')'));
        assert!(may_break(Some('-'), ' '));
        // Never right after whitespace, always before it.
        assert!(!may_break(Some(' '), 'a'));
        assert!(!may_break(Some('\t'), '\u{4e00}'));
        assert!(may_break(Some('a'), ' '));
        assert!(
            may_break(Some('('), ' '),
            "the space test comes before the opener's"
        );
        // Inside a Latin word: no break.
        assert!(!may_break(Some('a'), 'b'));
        assert!(!may_break(Some('1'), '2'));
    }

    #[test]
    fn the_east_asian_ranges_break_before_themselves() {
        // Hangul syllables, CJK ideographs, kana, Hangul Jamo, compatibility ideographs, the
        // fullwidth forms and the halfwidth Hangul.
        for c in [
            '\u{ac00}', '\u{4e2d}', '\u{3042}', '\u{1100}', '\u{f900}', '\u{ff21}', '\u{ffa1}',
        ] {
            assert!(may_break(Some('a'), c), "{c:?}");
            assert!(may_break(Some('\u{4e2d}'), c), "{c:?}");
        }
        // The ranges end where the reference's compares do.
        for c in [
            '\u{10ff}', '\u{2fff}', '\u{d7b0}', '\u{f8ff}', '\u{fb00}', '\u{ffdd}',
        ] {
            assert!(!may_break(Some('a'), c), "{c:?}");
        }
    }

    #[test]
    fn an_opening_bracket_holds_what_follows_and_a_closing_mark_holds_back() {
        // After an opener (ASCII and CJK): no break before a CJK character.
        for open in ['(', '[', '{', '$', '\u{300c}', '\u{ff08}', '\u{2018}'] {
            assert!(!may_break(Some(open), '\u{4e2d}'), "{open:?}");
        }
        // Before a closer: no break, though the ranges would give one.
        for close in [
            '\u{3002}', '\u{ff0c}', '\u{3001}', '\u{300d}', '\u{30fc}', '\u{ff01}', '\u{ff9e}',
        ] {
            assert!(!may_break(Some('\u{4e2d}'), close), "{close:?}");
        }
        // The neighbours of a closer are not closers.
        assert!(may_break(Some('\u{4e2d}'), '\u{3003}'));
        assert!(may_break(Some('\u{4e2d}'), '\u{300a}'));
        // Latin closers are never breaks either.
        assert!(!may_break(Some('a'), ')'));
        assert!(!may_break(Some('a'), ','));
    }

    /// Whitespace is the ctype table's below U+0100 and Windows' C1_SPACE from there up, which
    /// takes the ideographic space.
    #[test]
    fn whitespace_is_the_ctype_table_then_c1_space() {
        for c in [' ', '\t', '\n', '\u{b}', '\u{c}', '\r'] {
            assert!(is_space(c), "{c:?}");
        }
        for c in [
            '\u{a0}', '\u{85}', 'a', '\u{200b}', '\u{feff}', '\u{180e}', '\u{3001}',
        ] {
            assert!(!is_space(c), "{c:?}");
        }
        let above: Vec<u32> = (0x100..=0xffffu32)
            .filter(|&u| is_space(char::from_u32(u).unwrap_or('a')))
            .collect();
        let mut want: Vec<u32> = vec![0x1680, 0x2028, 0x2029, 0x202f, 0x205f, 0x3000];
        want.extend(0x2000..=0x200a);
        want.sort_unstable();
        assert_eq!(above, want);
    }

    /// The ideographic space is whitespace to the classifier: a break before it, none after it, and
    /// before it even after an opening bracket (the space test comes first).
    #[test]
    fn the_ideographic_space_breaks_as_whitespace() {
        assert!(may_break(Some('\u{4e2d}'), '\u{3000}'));
        assert!(
            !may_break(Some('\u{3000}'), '\u{4e2d}'),
            "none after whitespace"
        );
        assert!(
            may_break(Some('\u{ff08}'), '\u{3000}'),
            "\u{ff08} is an opener"
        );
    }
}
