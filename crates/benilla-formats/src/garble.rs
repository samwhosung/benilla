//! The chat language scramble, the reference's `0x49b560`: the server sends foreign speech in
//! plaintext beside a language id, and the client garbles it.
//!
//! The line alternates separator runs and words. Each word is hashed once with case-folded
//! `SStrHash`; it is understood when `hash % 300 < skill`, and otherwise the same hash picks a
//! substitute of its byte length from `LanguageWords.dbc`, which takes the source's case letter
//! by letter. The result is a pure function of the word's bytes. The three call sites differ only
//! in [`Garble`]'s flags and cap: chat (`0x49aa7c`), the gossip greeting (`0x4e22ab`), item text
//! (`0x4e35b0`).

use crate::LanguageWords;

/// `SStrHash`'s 16-entry mix table (`0x80e4e0`), not Storm's 0x500-entry crypt table: a byte
/// mixes as `T[hi] - T[lo]`.
const HASH_TABLE: [u32; 16] = [
    0x486e26ee, 0xdcaa16b3, 0xe1918eef, 0x202dafdb, 0x341c7dc7, 0x1c365303, 0x40ef2d37, 0x65fd5e49,
    0xd6057177, 0x904ece93, 0x1c38024f, 0x98fd323b, 0xe3061ae7, 0xa39b0fa1, 0x9797f25f, 0xe4444563,
];

/// Full fluency (`0x49b599`): at or above it the line is copied untokenized.
pub const FLUENT_SKILL: u32 = 300;

/// The per-word test's modulus (`0x49b79c`), so a line keeps about `skill / 300` of its words.
const UNDERSTAND_MODULUS: u32 = 300;

/// The bytes of a word copied into the reference's scratch buffer and hashed (`0x49b768`).
const MAX_HASHED_BYTES: usize = 0x100;

/// The lookup length's clamp (`0x49b7c1`), apart from [`MAX_HASHED_BYTES`]: a 45-byte word hashes
/// all 45 bytes and is looked up at length 18.
const MAX_LENGTH_KEY: usize = 0x12;

/// The flags and destination cap `0x49b560` takes, one set per call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Garble {
    /// `keepAngle`: pass a `<…>` span through verbatim, brackets included.
    pub keep_angle: bool,
    /// `keepPunct`: copy separators instead of collapsing each run to one space; off for chat, so
    /// `"hello, world."` comes back with a trailing space.
    pub keep_punct: bool,
    /// `dstSize`, the whole destination buffer: output stops at `dst_size - 1` bytes.
    pub dst_size: usize,
}

impl Garble {
    /// Chat (`0x49aa7c`, in the display path `0x49a870`).
    pub const CHAT: Self = Self {
        keep_angle: false,
        keep_punct: false,
        dst_size: 0x800,
    };

    /// The NPC gossip greeting (`0x4e22ab`, then `GOSSIP_SHOW`).
    pub const GOSSIP: Self = Self {
        keep_angle: false,
        keep_punct: true,
        dst_size: 0x800,
    };

    /// Readable item text, letters and books (`0x4e3856`, then `ITEM_TEXT_READY`).
    pub const ITEM_TEXT: Self = Self {
        keep_angle: true,
        keep_punct: true,
        dst_size: 0x1f40,
    };
}

/// `SStrHash` (`0x64af90`), case-folded with a zero seed as `0x49b560` calls it. The fold is
/// ASCII-only (`a`-`z` up, `/` to `\`), and the walk stops at a NUL, both as the reference does.
fn sstr_hash_folded(bytes: &[u8]) -> u32 {
    let mut s1: u32 = 0x7FED_7FED;
    let mut s2: u32 = 0xEEEE_EEEE;
    for &raw in bytes {
        if raw == 0 {
            break;
        }
        let mut c = raw;
        if c.is_ascii_lowercase() {
            c -= 0x20;
        }
        if c == b'/' {
            c = b'\\';
        }
        let t = s1.wrapping_add(s2);
        s1 = t ^ HASH_TABLE[usize::from(c >> 4)].wrapping_sub(HASH_TABLE[usize::from(c & 0xF)]);
        s2 = s2
            .wrapping_mul(0x21)
            .wrapping_add(u32::from(c))
            .wrapping_add(3)
            .wrapping_add(s1);
    }
    if s1 != 0 {
        s1
    } else {
        1
    }
}

/// `0x41aab0`'s malformed-byte sentinel; above 0xff, so a malformed run stays inside its word.
const MALFORMED: u32 = 0x8000_0000;

/// The reference's UTF-8 decoder (`0x41aab0`), which accepts 1 to 6 byte forms; returns the
/// codepoint and the bytes consumed, one when malformed.
fn decode(bytes: &[u8]) -> (u32, usize) {
    let b0 = bytes[0];
    let (mut cp, n) = match b0 {
        0x00..=0x7f => return (u32::from(b0), 1),
        0xc0..=0xdf => (u32::from(b0 & 0x1f), 2),
        0xe0..=0xef => (u32::from(b0 & 0x0f), 3),
        0xf0..=0xf7 => (u32::from(b0 & 0x07), 4),
        0xf8..=0xfb => (u32::from(b0 & 0x03), 5),
        0xfc..=0xfd => (u32::from(b0 & 0x01), 6),
        _ => return (MALFORMED, 1),
    };
    if bytes.len() < n {
        return (MALFORMED, 1);
    }
    for &cont in &bytes[1..n] {
        if cont & 0xc0 != 0x80 {
            return (MALFORMED, 1);
        }
        cp = (cp << 6) | u32::from(cont & 0x3f);
    }
    (cp, n)
}

/// A Latin-1 letter as `0x6c9c60`'s table classifies one, which leaves out `0xDE` (thorn).
fn is_latin1_letter(cp: u32) -> bool {
    matches!(cp, 0x41..=0x5a | 0x61..=0x7a)
        || matches!(cp, 0xc0..=0xdd if cp != 0xd7)
        || cp == 0xdf
        || matches!(cp, 0xe0..=0xff if cp != 0xf7)
}

/// The word-character predicate (`0x49b940`): digits, the apostrophe and every codepoint above
/// Latin-1 are word characters.
fn is_word_char(cp: u32) -> bool {
    is_latin1_letter(cp) || (0x30..=0x39).contains(&cp) || cp == 0x27 || cp > 0xff
}

/// `src` as `language` sounds to a listener with `skill` in it; unchanged for language 0
/// (Universal) or full skill. The addon, chat-type and GM gates are the caller's.
pub fn garble(words: &LanguageWords, language: u32, skill: u32, src: &str, mode: Garble) -> String {
    let cap = mode.dst_size.saturating_sub(1);
    if language == 0 || skill >= FLUENT_SKILL {
        // `SStrCopy` at `0x49b5a1`: untokenized, so separator runs stay as they are.
        return truncate_utf8(src, cap).to_string();
    }
    let Some(pool) = words.pool(language) else {
        // Deviation: a language with no words keeps the line, where the reference drops every
        // word, because a missing pool is a broken install and blank chat would hide it.
        return truncate_utf8(src, cap).to_string();
    };

    let bytes = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // --- separator run ---------------------------------------------------------------
        // Chat collapses each separator run, leading and trailing included, to one space.
        let mut emitted_space = false;
        while i < bytes.len() {
            let (cp, n) = decode(&bytes[i..]);
            if is_word_char(cp) {
                break;
            }
            if mode.keep_angle && cp == 0x3c {
                // Copy the `<…>` span verbatim, opener and closer included, or to end of string.
                let start = i;
                i += n;
                while i < bytes.len() {
                    let (c2, n2) = decode(&bytes[i..]);
                    i += n2;
                    if c2 == 0x3e {
                        break;
                    }
                }
                push(&mut out, &bytes[start..i], cap);
                continue;
            }
            if mode.keep_punct {
                push(&mut out, &bytes[i..i + n], cap);
            } else if !emitted_space {
                push(&mut out, b" ", cap);
                emitted_space = true;
            }
            i += n;
        }
        if i >= bytes.len() {
            break;
        }

        // --- word ------------------------------------------------------------------------
        let word_start = i;
        while i < bytes.len() {
            let (cp, n) = decode(&bytes[i..]);
            if !is_word_char(cp) {
                break;
            }
            i += n;
        }
        let word = &bytes[word_start..i.min(word_start + MAX_HASHED_BYTES)];
        let hash = sstr_hash_folded(word);

        // The per-word gate: which words survive partial skill is fixed by their bytes.
        if hash % UNDERSTAND_MODULUS < skill {
            push(&mut out, word, cap);
            continue;
        }

        // A miss retries one length shorter; a miss at length 1 drops the word, space and all.
        let mut key = word.len().min(MAX_LENGTH_KEY);
        let substitute = loop {
            if let Some(s) = pool.nth_of_len(key, hash) {
                break Some(s);
            }
            if key <= 1 {
                break None;
            }
            key -= 1;
        };
        let Some(substitute) = substitute else {
            continue;
        };
        stamp_case(&mut out, word, substitute.as_bytes(), cap);
    }
    // Only the reference's own cuts, the hash clamp and the cap, can split a codepoint here.
    String::from_utf8_lossy(&out).into_owned()
}

/// [`garble`] on the chat path.
pub fn garble_chat(words: &LanguageWords, language: u32, skill: u32, src: &str) -> String {
    garble(words, language, skill, src, Garble::CHAT)
}

/// Append, honouring the destination cap (`dstSize - 1` usable bytes).
fn push(out: &mut Vec<u8>, bytes: &[u8], cap: usize) {
    let room = cap.saturating_sub(out.len());
    out.extend_from_slice(&bytes[..room.min(bytes.len())]);
}

/// `0x49b8b0`: each character takes the case of the source's at the same position (`hEllO` to
/// `kAzuM`), a non-letter forcing lowercase, for the shorter of the two words.
fn stamp_case(out: &mut Vec<u8>, source: &[u8], substitute: &[u8], cap: usize) {
    for (i, &s) in source.iter().enumerate() {
        if i >= substitute.len() || out.len() >= cap {
            break;
        }
        out.push(if s.is_ascii_uppercase() {
            substitute[i].to_ascii_uppercase()
        } else {
            substitute[i].to_ascii_lowercase()
        });
    }
}

/// The verbatim paths' `SStrCopy`, at most `cap` bytes. Deviation: it stops at a codepoint
/// boundary where the reference's byte copy can split one, because a `&str` must stay UTF-8.
fn truncate_utf8(s: &str, cap: usize) -> &str {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_language_words, Chain};

    /// `(language, skill, input, expected)`: the reference's `0x49b560` output, emulated over the
    /// shipped `LanguageWords.dbc`.
    const GOLDEN: &[(u32, u32, &str, &str)] = &[
        (1, 0, "hello", "kazum"),
        (1, 0, "h", "o"),
        (1, 0, "ab", "ha"),
        (1, 0, "antidisestablishment", "khaz'rogg'ahn"),
        (
            1,
            0,
            "pneumonoultramicroscopicsilicovolcanoconiosis",
            "khaz'rogg'ahn",
        ),
        (
            1,
            0,
            "the cat sat on the mat the cat",
            "mog ruk ogg gi mog gul mog ruk",
        ),
        (1, 0, "hello hello", "kazum kazum"),
        (1, 0, "hello, world.", "kazum magan "),
        (1, 0, "don't", "re'ka"),
        (1, 0, "abc123", "moguna"),
        (1, 0, "12345", "regas"),
        (1, 0, "hello    world", "kazum magan"),
        (1, 0, "  hello  ", " kazum "),
        (1, 0, "hello\tworld", "kazum magan"),
        (1, 0, "Hello", "Kazum"),
        (1, 0, "HELLO", "KAZUM"),
        (1, 0, "hEllO", "kAzuM"),
        (1, 0, "Hello There Friend", "Kazum No'ku Raznos"),
        (1, 0, "", ""),
        (1, 0, "!!!", " "),
        (1, 0, "a b c", "g g o"),
        (7, 0, "hello", "majis"),
        (2, 0, "hello", "talah"),
        (33, 0, "hello", "majis"),
        (14, 0, "hello", "atuad"),
        (1, 1, "hello", "kazum"),
        (1, 75, "hello", "kazum"),
        (1, 150, "hello", "hello"),
        (1, 299, "hello", "hello"),
        (1, 300, "hello", "hello"),
        (0, 0, "hello", "hello"),
        (1, 0, "Kek lol Rofl", "Ogg kek Ogar"),
        (1, 0, "Thrall sends his regards", "No'gor zugas kil gul'rok"),
        (7, 0, "For the Alliance!", "Nud ras Landowar "),
        (
            1,
            150,
            "the quick brown fox jumps over the lazy dog",
            "the quick nogah fox re'ka nogu the maka kil",
        ),
    ];

    #[test]
    fn the_reference_golden_vectors() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let words = load_language_words(&mut chain).expect("load");
        for &(lang, skill, input, want) in GOLDEN {
            let got = garble_chat(&words, lang, skill, input);
            assert_eq!(got, want, "lang={lang} skill={skill} input={input:?}");
        }
    }

    #[test]
    fn the_mapping_is_a_pure_function_of_the_words_bytes() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let words = load_language_words(&mut chain).expect("load");
        let first: Vec<String> = GOLDEN
            .iter()
            .map(|&(l, s, i, _)| garble_chat(&words, l, s, i))
            .collect();
        let again: Vec<String> = GOLDEN
            .iter()
            .map(|&(l, s, i, _)| garble_chat(&words, l, s, i))
            .collect();
        assert_eq!(first, again);
        // The same word keeps its substitute in another sentence.
        let alone = garble_chat(&words, 1, 0, "hello");
        assert_eq!(alone, "kazum", "the golden vector still anchors it");
        let in_sentence = garble_chat(&words, 1, 0, "well hello there");
        assert_eq!(in_sentence.split(' ').nth(1), Some(alone.as_str()));
    }

    #[test]
    fn the_hash_folds_case_and_is_seeded_the_storm_way() {
        assert_eq!(sstr_hash_folded(b"hello"), sstr_hash_folded(b"HELLO"));
        assert_eq!(sstr_hash_folded(b"hello"), sstr_hash_folded(b"hEllO"));
        assert_ne!(sstr_hash_folded(b"hello"), sstr_hash_folded(b"world"));
        // The empty string returns the untouched seed.
        assert_eq!(sstr_hash_folded(b""), 0x7FED_7FED);
    }

    #[test]
    fn digits_apostrophes_and_high_codepoints_are_word_characters() {
        assert!(is_word_char(u32::from(b'7')));
        assert!(is_word_char(0x27));
        assert!(is_word_char(0x4e00)); // CJK
        assert!(is_word_char(MALFORMED));
        assert!(!is_word_char(u32::from(b' ')));
        assert!(!is_word_char(u32::from(b'-')));
        assert!(!is_word_char(u32::from(b'.')));
    }

    #[test]
    fn the_gossip_and_item_text_flags_change_the_separators_only() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let words = load_language_words(&mut chain).expect("load");
        assert_eq!(garble_chat(&words, 1, 0, "hello, world."), "kazum magan ");
        assert_eq!(
            garble(&words, 1, 0, "hello, world.", Garble::GOSSIP),
            "kazum, magan."
        );
        assert_eq!(
            garble(&words, 1, 0, "hello <Name> world", Garble::ITEM_TEXT),
            "kazum <Name> magan"
        );
        // Without `keep_angle` the name garbles too.
        assert_ne!(
            garble(&words, 1, 0, "hello <Name> world", Garble::GOSSIP),
            "kazum <Name> magan"
        );
    }

    #[test]
    fn the_destination_cap_truncates() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let words = load_language_words(&mut chain).expect("load");
        let tiny = Garble {
            dst_size: 4,
            ..Garble::CHAT
        };
        assert_eq!(garble(&words, 1, 0, "hello hello hello", tiny), "kaz");
        // The verbatim paths honour it too.
        assert_eq!(garble(&words, 0, 0, "hello hello", tiny), "hel");
    }
}
