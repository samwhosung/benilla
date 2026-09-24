//! **The chat language scramble** — the reference's `0x49b560`, reimplemented.
//!
//! When someone speaks a language your character does not know, the *server* sends the sentence in
//! plaintext with a language id beside it and never rewrites it; turning it into gibberish is
//! entirely the client's job (B262 — opposite-faction speech rendered perfectly readable because
//! this step did not exist). This module is that step.
//!
//! **The oracle is an emulated run of the binary's own bytes** (`0x49b560` over the player's own
//! `LanguageWords.dbc`). [`tests::the_reference_golden_vectors`] carries all 35 of its golden
//! vectors verbatim; they are the oracle this file is graded against, and this implementation
//! reproduces every vector byte for byte.
//!
//! The shape, in one paragraph. The line is walked as an alternating sequence of separator runs and
//! words. Each word is hashed once with `SStrHash` (case-folded, so `hello`/`Hello`/`HELLO` share a
//! hash and therefore a substitute); the *same* hash decides both whether that particular word is
//! understood (`hash % 300 < skill` — a **per-word, graduated** gate, not an all-or-nothing switch)
//! and, when it is not, which replacement is drawn from the `(language, byte length)` bucket of
//! `LanguageWords.dbc`. The source word's capitalisation is then stamped onto the replacement one
//! character at a time. Nothing is random and nothing is stateful: the mapping is a pure function
//! of the word's bytes, stable within a session and across sessions.
//!
//! **Three call sites in the reference, and the flags are what separate them** ([`Garble`]):
//! chat (`0x49aa7c`, both flags off), the NPC gossip greeting (`0x4e22ab`, `keep_punct`), and
//! readable item text (`0x4e35b0`, both). So the same language barrier covers gossip and books —
//! benilla only wires chat today, but the routine is the whole routine, not the chat slice of it.

use crate::LanguageWords;

/// `SStrHash`'s mix table — the **16-dword** table at `0x80e4e0`, not Storm's public 0x500-entry
/// crypt table. Both nibbles of a byte index these same sixteen entries and the mix is a
/// **subtraction**, `T[hi] - T[lo]`.
const HASH_TABLE: [u32; 16] = [
    0x486e26ee, 0xdcaa16b3, 0xe1918eef, 0x202dafdb, 0x341c7dc7, 0x1c365303, 0x40ef2d37, 0x65fd5e49,
    0xd6057177, 0x904ece93, 0x1c38024f, 0x98fd323b, 0xe3061ae7, 0xa39b0fa1, 0x9797f25f, 0xe4444563,
];

/// Full fluency (`0x49b599 cmp esi,0x12c`). At or above this the line is copied verbatim without
/// even being tokenized; below it every word runs the per-word gate.
pub const FLUENT_SKILL: u32 = 300;

/// The per-word understand test's modulus (`0x49b79c div ecx=0x12c`). It is the same 300 as
/// [`FLUENT_SKILL`] and that is not a coincidence — it makes the surviving fraction of a line
/// exactly `skill / 300`.
const UNDERSTAND_MODULUS: u32 = 300;

/// How many bytes of a word are copied into the reference's `0x110`-byte scratch buffer and
/// therefore hashed (`0x49b768`).
const MAX_HASHED_BYTES: usize = 0x100;

/// The **length key**'s clamp (`0x49b7c1`) — a separate quantity from [`MAX_HASHED_BYTES`], and a
/// re-implementation must keep them apart: a 45-byte word is hashed over all 45 bytes but looked up
/// as if it were 18 long. The shipped table's longest word is 17, so this never binds against
/// shipped content; it is the retry below that does the work for a long word.
const MAX_LENGTH_KEY: usize = 0x12;

/// The two flags `0x49b560` takes, plus the destination cap — i.e. which of the reference's three
/// call sites this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Garble {
    /// `keepAngle` — pass a `<…>` span through verbatim, opener and closer included. Set only on
    /// readable item text (`0x4e3856`).
    pub keep_angle: bool,
    /// `keepPunct` — copy each separator's own bytes instead of collapsing the whole run to one
    /// space. Set on gossip (`0x4e22ab`) and item text; **off on the chat path**, which is why
    /// `"hello, world."` comes back `"kazum magan "` with a trailing space.
    pub keep_punct: bool,
    /// `dstSize` — the reference's destination buffer, **total size not remaining**. Output is
    /// truncated to `dst_size - 1` bytes.
    pub dst_size: usize,
}

impl Garble {
    /// The chat path (`0x49aa7c` in the display chokepoint `0x49a870`): both flags off, `0x800`.
    ///
    /// The cap cannot bind on real chat — vmangos refuses a `CMSG_MESSAGECHAT` body past 255 bytes
    /// and no server-composed line approaches 2 KiB — but it is the reference's number and modelling
    /// it costs nothing, so a hostile or merely odd line truncates where the real client truncates
    /// instead of somewhere of our own invention.
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

    /// Readable item text — letters and books (`0x4e3856`, then `ITEM_TEXT_READY`).
    pub const ITEM_TEXT: Self = Self {
        keep_angle: true,
        keep_punct: true,
        dst_size: 0x1f40,
    };
}

/// `SStrHash` (`0x64af90`) as `0x49b560` calls it: **case-folded** (`caseSensitive = 0`) with a zero
/// seed.
///
/// The fold is ASCII-only — `a`-`z` lift by 0x20 and `/` becomes `\` — so a Latin-1 accented capital
/// and its lowercase form hash differently, exactly as they do in the reference. The walk stops at a
/// NUL because the reference's does; a wire chat string cannot contain one, but a caller composing
/// text locally could.
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

/// The sentinel `0x41aab0` returns for a malformed lead or continuation byte. It is `> 0xff`, so
/// [`is_word_char`] swallows a malformed run into the surrounding word rather than splitting on it —
/// a quirk worth preserving rather than "fixing", since it decides tokenization.
const MALFORMED: u32 = 0x8000_0000;

/// The reference's UTF-8 decoder `0x41aab0`, which predates RFC 3629 and so accepts the **1–6 byte**
/// forms. Returns `(codepoint, bytes consumed)`; consumes one byte on a malformed lead so the walk
/// always advances.
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

/// A Latin-1 letter as `0x6c9c60` classifies one.
///
/// Transcribed from the binary's table rather than from what Latin-1 "should" say:
/// `0xDE` (thorn) is **not** in the accepted set even though `0xC0..=0xDD`, `0xDF` and
/// `0xE0..=0xFF` around it are. It cannot affect an ASCII line, and guessing the table is a worse
/// error than transcribing it.
fn is_latin1_letter(cp: u32) -> bool {
    matches!(cp, 0x41..=0x5a | 0x61..=0x7a)
        || matches!(cp, 0xc0..=0xdd if cp != 0xd7)
        || cp == 0xdf
        || matches!(cp, 0xe0..=0xff if cp != 0xf7)
}

/// The word-character predicate `0x49b940`.
///
/// Three consequences that decide output and that a re-implementation guesses wrong: **digits are
/// word characters**, so `12345` is one token and gets substituted like a word; **an apostrophe
/// never splits a word**, so `don't` is one token; and **every codepoint above Latin-1 is
/// unconditionally a word character**, so CJK, Cyrillic and a malformed byte run are all swallowed
/// into words.
fn is_word_char(cp: u32) -> bool {
    is_latin1_letter(cp) || (0x30..=0x39).contains(&cp) || cp == 0x27 || cp > 0xff
}

/// Rewrite `src` as `language` sounds to a listener with `skill` in it.
///
/// Returns the text unchanged when `language` is 0 (Universal — `Languages.dbc` has no row 0, so the
/// reference checks this twice, in the caller *and* here) or when `skill >= 300`. Everything else
/// goes word by word.
///
/// The caller owns the gates this routine does not: the addon sentinel, the chat types that force
/// the language to 0, and the GM flag. See `benilla-app`'s chat feed.
pub fn garble(words: &LanguageWords, language: u32, skill: u32, src: &str, mode: Garble) -> String {
    let cap = mode.dst_size.saturating_sub(1);
    if language == 0 || skill >= FLUENT_SKILL {
        // `SStrCopy(dst, src, dstSize)` at `0x49b5a1` — the whole line, untokenized, so a
        // fluent listener does not even get separator runs collapsed.
        return truncate_utf8(src, cap).to_string();
    }
    let Some(pool) = words.pool(language) else {
        // A language with no rows cannot be garbled — the reference would find no node at any
        // length and drop every word, emitting nothing. We keep the line instead: a missing pool
        // is a broken install, and silently blanking chat is the worse failure.
        return truncate_utf8(src, cap).to_string();
    };

    let bytes = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // --- separator run ---------------------------------------------------------------
        // One space per *run* on the chat path, latched per outer iteration: a leading run, a
        // trailing run, `"    "`, `", "`, `"!!!"` and a tab all collapse to exactly one space.
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

        // The per-word gate. This is the correction that matters most: partial skill is **not**
        // "garbled like skill 0". A word survives exactly when `hash % 300 < skill`, so the
        // surviving fraction of a line is `skill / 300` and *which* words survive is fixed by their
        // bytes — at skill 150 a pangram comes back half-plain, the same half every time.
        if hash % UNDERSTAND_MODULUS < skill {
            push(&mut out, word, cap);
            continue;
        }

        // The length key steps *down* on a miss, and a length-1 miss drops the word entirely —
        // nothing is emitted for it, not even a space. Unreachable against shipped content (every
        // language has a word at every length from 1 up to its own maximum), reachable against a
        // trimmed table.
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
    // Every substitute is ASCII and every verbatim span is a whole-codepoint slice of `src`, so the
    // only way out of UTF-8 is the `0x100` hash clamp or the destination cap splitting a multi-byte
    // character. Both are the reference's own truncations; we keep them and repair the encoding
    // rather than moving the cut.
    String::from_utf8_lossy(&out).into_owned()
}

/// The chat path, which is the one benilla wires today.
pub fn garble_chat(words: &LanguageWords, language: u32, skill: u32, src: &str) -> String {
    garble(words, language, skill, src, Garble::CHAT)
}

/// Append, honouring the destination cap (`dstSize - 1` usable bytes).
fn push(out: &mut Vec<u8>, bytes: &[u8], cap: usize) {
    let room = cap.saturating_sub(out.len());
    out.extend_from_slice(&bytes[..room.min(bytes.len())]);
}

/// `0x49b8b0` — stamp the source word's capitalisation onto the substitute.
///
/// Two things a reader gets wrong if this is described as "capitalisation is preserved". The case is
/// decided **per character, from the source**, not first-letter-only and not from the substitute:
/// `hEllO` → `kAzuM`. And a non-letter in the source (a digit, an apostrophe) is not upper, so it
/// forces the substitute's character **lowercase**.
///
/// The emitted length is `min(len(source), len(substitute), budget)`. Those first two are usually
/// equal because the lookup is length-keyed — but not after a retry, which is exactly the long-word
/// case: a 20-byte word looked up at length 13 emits 13 bytes.
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

/// Truncate to at most `cap` bytes without splitting a codepoint — the verbatim paths' `SStrCopy`,
/// which is a byte copy in the reference and cannot produce invalid UTF-8 for us.
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

    /// **The oracle.** Every one of the 35 golden vectors produced by emulating the reference's own
    /// `0x49b560` over the shipped `LanguageWords.dbc` — not transcribed from a description of the
    /// output, but the exact bytes the binary wrote.
    ///
    /// `(language, skill, input, expected)`.
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

    /// No per-call state anywhere: the same call repeats identically, and so does the same word in
    /// a different sentence. (The reference's own oracle proves the second half across separately
    /// built emulator "sessions"; ours is a pure function, so within-process repetition is the
    /// strongest form the property can take here.)
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
        // The same word carries its substitute between sentences, not just within one — stated
        // against the standalone result rather than a literal, so this asserts the *property*
        // instead of re-recording whatever this file happens to produce.
        let alone = garble_chat(&words, 1, 0, "hello");
        assert_eq!(alone, "kazum", "the golden vector still anchors it");
        let in_sentence = garble_chat(&words, 1, 0, "well hello there");
        assert_eq!(in_sentence.split(' ').nth(1), Some(alone.as_str()));
    }

    /// `SStrHash` is case-folded, which is *why* `hello`/`Hello`/`HELLO` share a substitute — the
    /// case stamp only decides how it is printed.
    #[test]
    fn the_hash_folds_case_and_is_seeded_the_storm_way() {
        assert_eq!(sstr_hash_folded(b"hello"), sstr_hash_folded(b"HELLO"));
        assert_eq!(sstr_hash_folded(b"hello"), sstr_hash_folded(b"hEllO"));
        assert_ne!(sstr_hash_folded(b"hello"), sstr_hash_folded(b"world"));
        // The empty string never reaches the table, so it returns the untouched seed.
        assert_eq!(sstr_hash_folded(b""), 0x7FED_7FED);
    }

    /// The tokenizer's three surprises, stated as tests so they cannot be "cleaned up" later.
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

    /// The flags the other two call sites set. Chat collapses `", "` to one space; gossip keeps
    /// both bytes; item text additionally passes a `<…>` span straight through.
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
        // Without `keep_angle` the span is just separators and a word, so the name garbles too.
        assert_ne!(
            garble(&words, 1, 0, "hello <Name> world", Garble::GOSSIP),
            "kazum <Name> magan"
        );
    }

    /// The destination cap is the reference's, and it truncates rather than growing.
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
