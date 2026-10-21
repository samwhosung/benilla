//! The two 1.12 text filters: the `profanityFilter` masker (`0x4a1a60`) over `ChatProfanity.dbc`
//! and the `spamFilter` predicate (`0x4a1ca0`) over `SpamMessages.dbc`, both PCRE lists compiled
//! once at startup and never reloaded.
//!
//! The masker self-gates on `[0x843600]` (read at `0x4a1a66`), so no call site tests the CVar. Each
//! matched byte becomes the next of `!@#$%^&*` (`.rdata 0x806458`) through a process-global index
//! (`[0xb6e5f0]`) that is never reset, so the phase carries across matches and messages. Patterns
//! apply in list order, not text order, every occurrence, and the length never changes.
//!
//! The chat chokepoint, the mail list's subjects (`0x4ad5b2`) and `GetInboxText`'s letter
//! (`0x4af203`) are wired. The reference masks ten more sites, not built here: `GetGuildInfo`, the
//! two guild-record accessors, the roster MOTD and info, `GetGuildRosterInfo`,
//! `GuildControlGetRankName`, item text, `SendChatMessage` and the guild-event MOTD; seven of them
//! pass the `dl = 1` memo, which re-blits a cached string without advancing the index.
//!
//! The predicate has one call site, in the chat chokepoint; it never edits the text, and a hit
//! drops the line silently.
//!
//! The same startup init compiles `NamesProfanity.dbc` and `NamesReserved.dbc` for the character
//! name check (`0x6c96c0`), which searches them with `ChatProfanity`'s compiled list, ungated by
//! either CVar: [`NameFilter`], the glue VM's [`benilla_ui::script::NameLists`].
//!
//! Three PCRE translations are the reference's own behaviour. `\<` and `\>` are the non-directional
//! `\b`: the client's patched escape table (`.rdata 0x812330`) gives all three the code -4.
//! CASELESS folds ASCII only (the fold table `0x873c40` is the identity over `0x80..=0xff`), so
//! the pattern literals and the subject are ASCII-lowercased and matched case-sensitively, which
//! keeps every offset. The shipped patterns use no backreferences or lookaround.
//!
//! The word and space classes are the client's own tables, not Unicode's. `\b` and `\s` test a
//! character below `0x100` against `.data 0x873e80` (bit `0x10` word, `0x01` space) and treat every
//! code point from `0x100` up as neither (`0x724fc0`, `0x7252b8`, `0x72534c`; a `\S` matches one).
//! So the word class is [`WORD_CLASS`] and the space class [`SPACE_CLASS`].

use std::sync::{Arc, OnceLock};

use bevy::prelude::*;
use regex::{Regex, RegexSet};

use benilla_formats::FilterPattern;

/// `.rdata 0x806458`: the eight mask bytes, in order.
const MASK_TOKENS: &[u8; 8] = b"!@#$%^&*";

/// The body of the client's word class: the characters with bit `0x10` in the ctype table
/// (`.data 0x873e80`), which PCRE reads as `md->ctypes` (`0x724512`, the default tables `0x873b40`
/// plus `0x340`). ASCII digits, letters and `_`, and Latin-1 `0xC0..=0xFF` without `×` (`D7`),
/// `Þ` (`DE`) and `÷` (`F7`); `ª µ º` (`AA B5 BA`) are not in it. Nothing from `0x100` up is.
const WORD_CLASS: &str = r"0-9A-Z_a-z\x{C0}-\x{D6}\x{D8}-\x{DD}\x{DF}-\x{F6}\x{F8}-\x{FF}";

/// The body of the client's space class: bit `0x01` of the same table, `\t \n \f \r`, the space and
/// `NBSP` (`A0`). Not `\v`, not `NEL`, nothing from `0x100` up.
const SPACE_CLASS: &str = r"\t\n\x0C\r \x{A0}";

/// `\b` over [`WORD_CLASS`]: a word character on exactly one side, the ends of the subject being
/// non-word. `regex` has no look-around and its `\b` is Unicode's (or ASCII's), so this is for
/// `fancy_regex`.
fn boundary() -> String {
    format!("(?:(?<=[{WORD_CLASS}])(?![{WORD_CLASS}])|(?<![{WORD_CLASS}])(?=[{WORD_CLASS}]))")
}

/// One shipped pattern, translated: the `exact` expression, and the `relaxed` one with its word
/// boundaries dropped, which matches wherever `exact` does and cheaply, for the set.
struct Translated {
    exact: String,
    relaxed: String,
    /// `exact` holds a boundary, so only `fancy_regex` can run it.
    bounded: bool,
}

/// One row's matcher: `regex` where the pattern has no word boundary, `fancy_regex` where it has.
enum Matcher {
    Plain(Regex),
    /// Compiled on first use: a `fancy_regex` look-around costs milliseconds to build, and the
    /// set rules out nearly every row before one is asked. The pattern parsed at startup.
    Bounded {
        exact: String,
        re: OnceLock<Option<fancy_regex::Regex>>,
    },
}

impl Matcher {
    fn compile(t: &Translated) -> Result<Self, String> {
        if t.bounded {
            fancy_regex::Expr::parse_tree(&t.exact)
                .map(|_| Matcher::Bounded {
                    exact: t.exact.clone(),
                    re: OnceLock::new(),
                })
                .map_err(|e| e.to_string())
        } else {
            Regex::new(&t.exact)
                .map(Matcher::Plain)
                .map_err(|e| e.to_string())
        }
    }

    /// The first match at or after byte `at`, with the text before `at` still in view of a
    /// boundary, as `pcre_exec`'s start offset leaves it. A backtracking limit counts as no match,
    /// as an error from `pcre_exec` does.
    fn find_at(&self, subject: &str, at: usize) -> Option<(usize, usize)> {
        match self {
            Matcher::Plain(re) => re.find_at(subject, at).map(|m| (m.start(), m.end())),
            Matcher::Bounded { exact, re } => re
                .get_or_init(|| {
                    fancy_regex::Regex::new(exact)
                        .inspect_err(|e| warn!("text filter expression {exact:?}: {e}"))
                        .ok()
                })
                .as_ref()?
                .find_from_pos(subject, at)
                .ok()
                .flatten()
                .map(|m| (m.start(), m.end())),
        }
    }

    fn is_match(&self, subject: &str) -> bool {
        self.find_at(subject, 0).is_some()
    }
}

/// One compiled list: the set for the cheap pass beside the matchers, in list order.
struct PatternList {
    /// Over the `relaxed` expressions: a superset of the rows that match.
    set: RegexSet,
    each: Vec<Matcher>,
}

impl PatternList {
    /// Compile a list, skipping and naming a row that fails, as `0x6c94cf` does.
    fn compile(rows: &[FilterPattern], list: &str) -> Self {
        let mut patterns = Vec::with_capacity(rows.len());
        let mut each = Vec::with_capacity(rows.len());
        for row in rows {
            let translated = translate(&row.pattern);
            match Matcher::compile(&translated) {
                Ok(m) => {
                    patterns.push(translated.relaxed);
                    each.push(m);
                }
                Err(e) => warn!(
                    "skipping {list} expression {:?} (record ID {}): {e}",
                    row.pattern, row.id
                ),
            }
        }
        // Built from the strings that compiled, so it agrees with `each`.
        let set = RegexSet::new(&patterns).unwrap_or_else(|e| {
            error!("{list}: the pattern set would not build ({e}) — the filter is off");
            RegexSet::empty()
        });
        PatternList { set, each }
    }

    fn empty() -> Self {
        PatternList {
            set: RegexSet::empty(),
            each: Vec::new(),
        }
    }

    fn len(&self) -> usize {
        self.each.len()
    }

    /// The rows that may match `subject`, already folded by [`ascii_lower`], in list order: the
    /// set's candidates, which a bounded row still has to confirm.
    fn candidates(&self, subject: &str) -> Vec<usize> {
        self.set.matches(subject).iter().collect()
    }

    /// Whether any row matches `subject`, already folded by [`ascii_lower`].
    fn any(&self, subject: &str) -> bool {
        self.len() != 0
            && self
                .set
                .matches(subject)
                .iter()
                .any(|p| match &self.each[p] {
                    Matcher::Plain(_) => true,
                    bounded => bounded.is_match(subject),
                })
    }
}

/// Translate one shipped PCRE pattern to `regex`/`fancy_regex`: `\<`, `\>` and `\b` become a
/// boundary over the client's word class and `\s` its space class, other escapes stay verbatim
/// (never lowercased: `\S` is not `\s`), and literal ASCII is lowercased. A `\s` inside a bracket
/// class contributes its members; a boundary there is left as written (PCRE's `\b` is a backspace
/// in a class, and no shipped row has one).
fn translate(pattern: &str) -> Translated {
    let mut exact = String::with_capacity(pattern.len());
    let mut relaxed = String::with_capacity(pattern.len());
    let mut bounded = false;
    let mut in_class = false;
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            match c {
                '[' if !in_class => in_class = true,
                ']' => in_class = false,
                _ => {}
            }
            let c = c.to_ascii_lowercase();
            exact.push(c);
            relaxed.push(c);
            continue;
        }
        match chars.next() {
            // The patched escape table: all three are `\b`.
            Some('<' | '>' | 'b') if !in_class => {
                exact.push_str(&boundary());
                bounded = true;
            }
            Some('s') => {
                let class = if in_class {
                    SPACE_CLASS.to_owned()
                } else {
                    format!("[{SPACE_CLASS}]")
                };
                exact.push_str(&class);
                relaxed.push_str(&class);
            }
            Some(next) => {
                exact.push('\\');
                exact.push(next);
                relaxed.push('\\');
                relaxed.push(next);
            }
            // A trailing backslash is a compile error in PCRE too.
            None => {
                exact.push('\\');
                relaxed.push('\\');
            }
        }
    }
    Translated {
        exact,
        relaxed,
        bounded,
    }
}

/// The reference's ASCII-only `CASELESS` fold, byte-aligned with the original.
fn ascii_lower(text: &str) -> String {
    let mut s = text.to_owned();
    s.make_ascii_lowercase();
    s
}

/// The two switches mirrored off their CVars: the reference's `[0x843600]` (`profanityFilter`)
/// and `[0x843604]` (`spamFilter`), both registered `"1"` (`0x402e68`, `0x402e8e`).
#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct TextFilterSwitches {
    /// `profanityFilter`, the masker's self-gate.
    pub(crate) profanity: bool,
    /// `spamFilter`; its options row ("Disable Spam Filter") is inverted.
    pub(crate) spam: bool,
}

impl Default for TextFilterSwitches {
    fn default() -> Self {
        TextFilterSwitches {
            profanity: true,
            spam: true,
        }
    }
}

/// The 14 player-authored chat types the reference masks (`.rdata 0x8046d8`, bound at `0x49aabb`);
/// any other type, a monster's yell included, is never masked.
pub(crate) const PROFANITY_MASKED_CHAT_TYPES: &[u8] = &[
    0x00, // SAY
    0x01, // PARTY
    0x02, // RAID
    0x03, // GUILD
    0x04, // OFFICER
    0x05, // YELL
    0x06, // WHISPER
    0x07, // WHISPER_INFORM
    0x08, // EMOTE
    0x0e, // CHANNEL
    0x14, // AFK
    0x15, // DND
    0x58, // RAID_WARNING
    0x5c, // BATTLEGROUND
];

/// The two compiled lists and the rotating mask index.
#[derive(Resource)]
pub(crate) struct TextFilter {
    /// Shared with [`NameFilter`], as the reference's one compiled `ChatProfanity` triple
    /// (`0xce893c`) is read by the masker and the name check alike.
    profanity: Arc<PatternList>,
    spam: PatternList,
    /// `[0xb6e5f0]`: zero-initialized, advanced mod 8, never reset. Atomic, so every reader of the
    /// filter (the chat chokepoint, the mail calls) shares the one index without excluding the
    /// others.
    mask_index: std::sync::atomic::AtomicUsize,
}

impl Default for TextFilter {
    /// No lists, without client data: both filters pass everything.
    fn default() -> Self {
        TextFilter {
            profanity: Arc::new(PatternList::empty()),
            spam: PatternList::empty(),
            mask_index: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl TextFilter {
    pub(crate) fn new(profanity: &[FilterPattern], spam: &[FilterPattern]) -> Self {
        TextFilter {
            profanity: Arc::new(PatternList::compile(profanity, "chat profanity filter")),
            spam: PatternList::compile(spam, "chat spam filter"),
            mask_index: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The name check's three lists: `names` (`NamesProfanity`) and `reserved` (`NamesReserved`)
    /// compiled here, `ChatProfanity` this filter's own.
    pub(crate) fn name_filter(
        &self,
        names: &[FilterPattern],
        reserved: &[FilterPattern],
    ) -> NameFilter {
        NameFilter(Arc::new(NameLists {
            names: PatternList::compile(names, "profane names filter"),
            chat: Arc::clone(&self.profanity),
            reserved: PatternList::compile(reserved, "reserved names filter"),
        }))
    }

    pub(crate) fn profanity_len(&self) -> usize {
        self.profanity.len()
    }

    pub(crate) fn spam_len(&self) -> usize {
        self.spam.len()
    }

    /// `0x4a1ca0`: whether the text matches the spam list. The reference also scans a server-pushed
    /// list (SMSG `0x332`) that vmangos never sends. `enabled` is `spamFilter`, which the reference
    /// tests at the call site.
    pub(crate) fn is_spam(&self, enabled: bool, text: &str) -> bool {
        if !enabled || text.is_empty() || self.spam.len() == 0 {
            return false;
        }
        self.spam.any(&ascii_lower(text))
    }

    /// `0x4a1a60` with `dl = 0`: mask in place, return whether anything matched. `enabled` is
    /// `profanityFilter`, tested here as the reference does.
    pub(crate) fn mask(&self, enabled: bool, text: &mut String) -> bool {
        if !enabled || text.is_empty() || self.profanity.len() == 0 {
            return false;
        }
        self.mask_uncached(text)
    }

    /// The index's current token, and the index advanced past it.
    fn next_mask_index(&self) -> usize {
        use std::sync::atomic::Ordering::Relaxed;
        let at = self.mask_index.load(Relaxed);
        self.mask_index.store((at + 1) % MASK_TOKENS.len(), Relaxed);
        at
    }

    /// The byte loop of `0x4a1b6a`-`0x4a1ba6`: one pass in list order over the mutating buffer, so
    /// an earlier pattern's mask can enable a later match but never the reverse.
    fn mask_uncached(&self, text: &mut String) -> bool {
        // The subject is ASCII-lowercased, byte-aligned with `text` and masked in step.
        let mut subject = ascii_lower(text);
        let mut candidates = self.profanity.candidates(&subject);
        let mut any = false;
        let mut i = 0;
        while i < candidates.len() {
            let p = candidates[i];
            let mut wrote = false;
            let mut cursor = 0;
            while let Some((start, end)) = self.profanity.each[p].find_at(&subject, cursor) {
                any = true;
                if start == end {
                    // The reference spins on an empty match; this stops instead. No shipped row
                    // can match empty (tested below).
                    break;
                }
                // One token per byte of the span, as the reference's loop writes; a match is on
                // char boundaries, so both strings stay valid UTF-8 and aligned.
                let run: String = (start..end)
                    .map(|_| MASK_TOKENS[self.next_mask_index()] as char)
                    .collect();
                text.replace_range(start..end, &run);
                subject.replace_range(start..end, &run);
                wrote = true;
                cursor = end;
                if cursor >= subject.len() {
                    break;
                }
            }
            if wrote {
                // The buffer moved: re-derive the candidates past this pattern.
                candidates = self
                    .profanity
                    .candidates(&subject)
                    .into_iter()
                    .filter(|&c| c > p)
                    .collect();
                i = 0;
            } else {
                i += 1;
            }
        }
        any
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(patterns: &[&str]) -> Vec<FilterPattern> {
        patterns
            .iter()
            .enumerate()
            .map(|(i, p)| FilterPattern {
                id: i as u32 + 1,
                pattern: (*p).to_string(),
            })
            .collect()
    }

    /// `\<` and `\>` are non-directional word boundaries, as the reference's compiler treats them.
    #[test]
    fn the_angle_escapes_are_non_directional_word_boundaries() {
        for spelling in [r"\<twat\>", r"\>twat\<", r"\btwat\b"] {
            let f = TextFilter::new(&list(&[spelling]), &[]);
            let mut hit = "you twat here".to_string();
            let mut miss = "atwatb".to_string();
            let copy = f;
            assert!(copy.mask(true, &mut hit), "{spelling} must match [4,8)");
            assert_eq!(hit, "you !@#$ here", "{spelling}");
            assert!(
                !copy.mask(true, &mut miss),
                "{spelling} must not match inside a word"
            );
            assert_eq!(miss, "atwatb");
        }
    }

    /// The 124 characters with bit `0x10` in the client's ctype table (`.data 0x873e80`), as read
    /// from the bytes: ASCII digits, letters and `_`, and Latin-1 `C0..=FF` except `D7 DE F7`.
    fn client_word_chars() -> Vec<char> {
        let mut v: Vec<char> = ('0'..='9').chain('A'..='Z').chain('a'..='z').collect();
        v.push('_');
        v.extend(
            (0xC0u32..=0xFF)
                .filter(|c| ![0xD7, 0xDE, 0xF7].contains(c))
                .filter_map(char::from_u32),
        );
        v
    }

    fn masked(pattern: &str, text: &str) -> String {
        let f = TextFilter::new(&list(&[pattern]), &[]);
        let mut line = text.to_string();
        f.mask(true, &mut line);
        line
    }

    #[test]
    fn the_client_word_set_is_the_124_characters_of_its_table() {
        assert_eq!(client_word_chars().len(), 124);
    }

    /// A letter above `0xFF` is not a word character (`0x724fc0`, `0x725052`: a code point of
    /// `0x100` or more skips the table), so a word that touches Cyrillic, Greek or CJK text still
    /// has its boundary there, whichever side the text is on.
    #[test]
    fn a_word_next_to_letters_above_latin1_has_its_boundary() {
        for other in ["ы", "Ж", "α", "傻", "한", "😀"] {
            assert_eq!(
                masked(r"\<twat\>", &format!("{other}twat{other}")),
                format!("{other}!@#${other}"),
                "{other:?} around \\<twat\\>"
            );
            assert_eq!(
                masked(r"\<twat", &format!("{other}twatty")),
                format!("{other}!@#$ty"),
                "{other:?} before \\<twat"
            );
            assert_eq!(
                masked(r"twat\>", &format!("tw{other}twat{other}")),
                format!("tw{other}!@#${other}"),
                "{other:?} after twat\\>"
            );
        }
    }

    /// Each of the 256 Latin-1 characters beside a word is a word character exactly when the
    /// client's table says so: a boundary falls between `x` and the word only if `x` is not one.
    #[test]
    fn every_latin1_character_is_a_word_character_as_the_client_table_says() {
        let word = client_word_chars();
        for code in 0u32..=0xFF {
            let x = char::from_u32(code).unwrap();
            // A line feed or a control byte may not be in a chat line, but the pattern does not care.
            for (pattern, text, hit) in [
                (r"\<twat\>", format!("{x}twat{x}"), format!("{x}!@#${x}")),
                (r"\btwat\b", format!("{x}twat{x}"), format!("{x}!@#${x}")),
            ] {
                // The subject is folded, so an uppercase ASCII letter is the lowercase one.
                let is_word = word.contains(&x) || x.is_ascii_uppercase();
                let want = if is_word { text.clone() } else { hit };
                assert_eq!(
                    masked(pattern, &text),
                    want,
                    "U+{code:04X} beside {pattern}"
                );
            }
        }
    }

    /// The characters the table leaves out of its Latin-1 letters, `ª µ º × Þ ÷`, are not word
    /// characters, and `þ` and `ß`, which sit beside them, are.
    #[test]
    fn the_latin1_gaps_in_the_word_table() {
        for gap in ['ª', 'µ', 'º', '×', 'Þ', '÷'] {
            assert_eq!(
                masked(r"\<twat\>", &format!("{gap}twat")),
                format!("{gap}!@#$"),
                "{gap}"
            );
        }
        for letter in ['þ', 'ß', 'ÿ', 'À', 'Ý'] {
            assert_eq!(
                masked(r"\<twat\>", &format!("{letter}twat")),
                format!("{letter}twat"),
                "{letter}"
            );
        }
    }

    /// `\s` is the client's space class: `\t \n \f \r`, the space and `NBSP`, and nothing else,
    /// where `regex`'s also takes `\v`, `NEL` and the spaces from `U+1680` up (`0x7252b8`,
    /// `0x72534c`).
    #[test]
    fn the_space_escape_is_the_client_space_class() {
        let f = TextFilter::new(&[], &list(&[r"a\sb"]));
        for yes in ['\t', '\n', '\u{0c}', '\r', ' ', '\u{a0}'] {
            assert!(f.is_spam(true, &format!("a{yes}b")), "U+{:04X}", yes as u32);
        }
        for no in [
            '\u{0b}', '\u{85}', '\u{1680}', '\u{2003}', '\u{3000}', '\u{feff}', 'x',
        ] {
            assert!(!f.is_spam(true, &format!("a{no}b")), "U+{:04X}", no as u32);
        }
    }

    /// The cheap pass drops a boundary, so it may name a row the boundary then refuses: the
    /// answer is the boundary's.
    #[test]
    fn the_cheap_pass_does_not_decide_a_bounded_row() {
        let lists = NameLists {
            names: PatternList::compile(&list(&[r"\<thrall\>"]), "t"),
            chat: Arc::new(PatternList::empty()),
            reserved: PatternList::empty(),
        };
        use benilla_ui::script::NameLists as _;
        assert!(lists.names_profanity("Thrall"));
        assert!(!lists.names_profanity("Thralls"));
        assert!(lists.names_profanity("ыThrallы"));
    }

    /// A transcript of the reference's masker: in place, length-preserving, caseless, the phase
    /// carrying across calls, and list order giving the later word the earlier mask characters.
    #[test]
    fn the_masker_reproduces_the_oracle_transcript() {
        let f = TextFilter::new(&list(&[r"\<fagg[aeiouy]t", r"\<twat\>", "shit"]), &[]);

        let mut line = "you are a twat and a faggot".to_string();
        assert!(f.mask(true, &mut line));
        assert_eq!(line, "you are a &*!@ and a !@#$%^");

        let mut line = "oh shit that hurts".to_string();
        assert!(f.mask(true, &mut line));
        assert_eq!(line, "oh #$%^ that hurts");

        let mut line = "shit shit shit".to_string();
        assert!(f.mask(true, &mut line));
        assert_eq!(line, "&*!@ #$%^ &*!@");

        // Case-insensitive, and unanchored inside a word.
        let mut line = "BULLSHIT".to_string();
        assert!(f.mask(true, &mut line));
        assert_eq!(line, "BULL#$%^");

        let mut line = "hello friend, nice weather".to_string();
        assert!(!f.mask(true, &mut line));
        assert_eq!(line, "hello friend, nice weather");

        assert_eq!(
            f.mask_index.load(std::sync::atomic::Ordering::Relaxed),
            6,
            "the cycle carries forward, and is never reset"
        );
    }

    /// With `profanityFilter = 0` the text is untouched and the phase does not move.
    #[test]
    fn the_masker_self_gates_and_a_gated_call_does_not_move_the_phase() {
        let f = TextFilter::new(&list(&[r"\<twat\>"]), &[]);
        let mut line = "you are a twat".to_string();
        assert!(!f.mask(false, &mut line));
        assert_eq!(line, "you are a twat");
        assert_eq!(f.mask_index.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    /// A multi-byte match is masked byte for byte.
    #[test]
    fn a_non_ascii_match_is_masked_byte_for_byte() {
        let f = TextFilter::new(&list(&["傻B"]), &[]);
        let mut line = "说 傻B 了".to_string();
        let before = line.len();
        assert!(f.mask(true, &mut line));
        assert_eq!(line.len(), before, "length-preserving in BYTES");
        assert_eq!(line, "说 !@#$ 了", "3 bytes of 傻 + 1 of B = four tokens");
    }

    /// The predicate never edits, and it is off when the switch is.
    #[test]
    fn the_spam_predicate_is_a_boolean_over_the_untouched_text() {
        let spam = list(&[
            r"(w\s*w\s*w\s*\.)?\s*i\s*t\s*e\s*m\s*b\s*a\s*y\s*(\.\s*c\s*a)",
            r"(w\s*w\s*w\s*\.)?\s*g\s*m\s*w\s*o\s*r\s*k\s*e\s*r\s*(\.\s*c\s*o\s*m\s*)",
        ]);
        let f = TextFilter::new(&[], &spam);
        assert!(f.is_spam(true, "buy gold at www.itembay.ca cheap"));
        assert!(f.is_spam(true, "w w w . i t e m b a y . c a"));
        assert!(f.is_spam(true, "W W W . I T E M B A Y . C A"), "CASELESS");
        assert!(f.is_spam(true, "visit gmworker.com now"));
        assert!(!f.is_spam(true, "hello friend"));
        assert!(
            !f.is_spam(true, "you are a twat"),
            "the two lists are disjoint"
        );
        assert!(
            !f.is_spam(false, "buy gold at www.itembay.ca"),
            "the switch is off"
        );
    }

    /// Every shipped row compiles (the reference compiles all of them), and none matches the empty
    /// string, which `mask_uncached`'s zero-length break rests on.
    #[test]
    fn every_shipped_pattern_compiles_and_none_matches_empty() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let profanity = benilla_formats::load_chat_profanity(&mut chain).expect("profanity");
        let spam = benilla_formats::load_spam_messages(&mut chain).expect("spam");
        let f = TextFilter::new(&profanity, &spam);
        assert_eq!(
            f.profanity_len(),
            profanity.len(),
            "every profanity row compiled"
        );
        assert_eq!(f.spam_len(), spam.len(), "every spam row compiled");
        for (i, re) in f.profanity.each.iter().enumerate() {
            assert!(
                !re.is_match(""),
                "profanity row {i} matches the empty string: {}",
                profanity[i].pattern
            );
        }
    }

    /// The name check over the shipped lists: a word of each list reserves the name, by its row's
    /// own anchors, caseless; the three compile whole; and the check's other rules come first.
    #[test]
    fn the_shipped_name_lists_reserve_names() {
        use benilla_ui::script::check_char_name;
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let profanity = benilla_formats::load_chat_profanity(&mut chain).expect("profanity");
        let names = benilla_formats::load_names_profanity(&mut chain).expect("names");
        let reserved = benilla_formats::load_names_reserved(&mut chain).expect("reserved");
        let started = std::time::Instant::now();
        let filter = TextFilter::new(&profanity, &[]).name_filter(&names, &reserved);
        let built = started.elapsed();
        assert_eq!(
            filter.0.len(),
            (names.len(), profanity.len(), reserved.len()),
            "every row of the three compiled"
        );
        let check = |name: &str| check_char_name(Some(name), &*filter.0, Default::default());
        // NamesReserved's `\<thrall\>` and `blizzard`; NamesProfanity's `\<pussy`; ChatProfanity's
        // unanchored `shit`.
        for name in ["Thrall", "THRALL", "Blizzardbob", "Pussycat", "Bullshit"] {
            assert_eq!(check(name), 0x4b, "{name} is CHAR_NAME_RESERVED");
        }
        // `\<thrall\>` is a whole word: `Thralls` is free.
        for name in ["Thralls", "Probeone", "Zoë"] {
            assert_eq!(check(name), 0x52, "{name} is CHAR_NAME_SUCCESS");
        }
        // The walk before the lists, the lists before the length.
        assert_eq!(check("Thrall1"), 0x48);
        assert_eq!(
            check("Thrallthrall"),
            0x52,
            "no word boundary inside the name"
        );
        assert_eq!(check("Blizzardbobby"), 0x4b, "13 units, reserved first");
        eprintln!("the name lists compiled in {built:?}");
    }

    /// The transcript's sentences through the real lists.
    #[test]
    fn the_shipped_lists_reproduce_the_oracle_sentences() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let profanity = benilla_formats::load_chat_profanity(&mut chain).expect("profanity");
        let spam = benilla_formats::load_spam_messages(&mut chain).expect("spam");
        let f = TextFilter::new(&profanity, &spam);

        let mut line = "you are a twat and a faggot".to_string();
        assert!(f.mask(true, &mut line));
        assert_eq!(
            line, "you are a &*!@ and a !@#$%^",
            "list order beats text order on the real list too"
        );

        // Cyrillic is not a word character to the client, so `\<twat\>` bounds against it.
        let mut cyrillic = "ыtwatы".to_string();
        assert!(
            f.mask(true, &mut cyrillic),
            "twat between Cyrillic letters is masked"
        );
        assert!(!cyrillic.contains("twat"), "{cyrillic}");
        assert!(
            cyrillic.starts_with('ы') && cyrillic.ends_with('ы'),
            "{cyrillic}"
        );

        let mut clean = "hello friend, nice weather".to_string();
        assert!(!f.mask(true, &mut clean));
        assert_eq!(clean, "hello friend, nice weather");

        assert!(f.is_spam(true, "buy gold at www.itembay.ca"));
        assert!(!f.is_spam(true, "hello friend"));
    }
}

/// The lists the character name check searches (`0x6c9909`-`0x6c9a3f`), in its order: profane
/// names, chat profanity, reserved names.
pub(crate) struct NameLists {
    names: PatternList,
    chat: Arc<PatternList>,
    reserved: PatternList,
}

impl NameLists {
    fn len(&self) -> (usize, usize, usize) {
        (self.names.len(), self.chat.len(), self.reserved.len())
    }
}

/// Each a caseless, unanchored search of the name as typed (`0x71fc70`, one match per call): any
/// row of that list.
impl benilla_ui::script::NameLists for NameLists {
    fn names_profanity(&self, name: &str) -> bool {
        self.names.any(&ascii_lower(name))
    }

    fn chat_profanity(&self, name: &str) -> bool {
        self.chat.any(&ascii_lower(name))
    }

    fn names_reserved(&self, name: &str) -> bool {
        self.reserved.any(&ascii_lower(name))
    }
}

/// The name check's lists, once the startup has compiled them; none without client data, where
/// no name is reserved.
#[derive(Resource, Clone)]
pub(crate) struct NameFilter(pub(crate) Arc<NameLists>);

impl benilla_ui::script::NameLists for NameFilter {
    fn names_profanity(&self, name: &str) -> bool {
        benilla_ui::script::NameLists::names_profanity(&*self.0, name)
    }

    fn chat_profanity(&self, name: &str) -> bool {
        benilla_ui::script::NameLists::chat_profanity(&*self.0, name)
    }

    fn names_reserved(&self, name: &str) -> bool {
        benilla_ui::script::NameLists::names_reserved(&*self.0, name)
    }
}

/// The chat chokepoint's view of the two filters, one [`SystemParam`] to keep `feed_chat` under
/// Bevy's sixteen-parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ChatTextFilter<'w> {
    filter: Res<'w, TextFilter>,
    switches: Res<'w, TextFilterSwitches>,
}

impl ChatTextFilter<'_> {
    /// Arm 1 (`0x49aac9`-`0x49ab33`): whether to drop the line. In the reference's order: not
    /// `CHAT_MSG_FILTERED`, no `"GM"` tag, a non-GM viewer, `spamFilter` on, then the predicate.
    /// Every exit, a GM viewer's included, still reaches the mask arm (`0x49abb7`).
    pub(crate) fn should_drop(
        &self,
        chat_type: u8,
        chat_tag: u8,
        viewer_is_gm: bool,
        text: &str,
    ) -> bool {
        use benilla_protocol::messages::{chat_tag as tag, CHAT_MSG_FILTERED};
        if chat_type == CHAT_MSG_FILTERED || chat_tag == tag::GM || viewer_is_gm {
            return false;
        }
        self.filter.is_spam(self.switches.spam, text)
    }

    /// Arm 2: mask the line if its type is in the reference's table (`0x49aa98`-`0x49aac2`); the
    /// CVar is the masker's own.
    pub(crate) fn mask_chat(&self, chat_type: u8, text: &mut String) {
        if !PROFANITY_MASKED_CHAT_TYPES.contains(&chat_type) {
            return;
        }
        self.filter.mask(self.switches.profanity, text);
    }
}

/// Load the four lists off the patch chain once, as the reference's startup init does
/// (`0x402b7f → 0x6c91a0`). It must run after `AssetSet::Open`, or the filters are silently empty.
fn load_text_filter_lists(
    mut commands: Commands,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    let Some(assets) = assets else { return };
    let (profanity, spam, names, reserved) = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        (
            benilla_formats::load_chat_profanity(&mut chain),
            benilla_formats::load_spam_messages(&mut chain),
            benilla_formats::load_names_profanity(&mut chain),
            benilla_formats::load_names_reserved(&mut chain),
        )
    };
    // A missing list passes everything; name it and carry on.
    let profanity = profanity
        .inspect_err(|e| warn!("chat profanity filter unavailable: {e:#}"))
        .unwrap_or_default();
    let spam = spam
        .inspect_err(|e| warn!("chat spam filter unavailable: {e:#}"))
        .unwrap_or_default();
    let names = names
        .inspect_err(|e| warn!("profane names filter unavailable: {e:#}"))
        .unwrap_or_default();
    let reserved = reserved
        .inspect_err(|e| warn!("reserved names filter unavailable: {e:#}"))
        .unwrap_or_default();
    let filter = TextFilter::new(&profanity, &spam);
    let name_filter = filter.name_filter(&names, &reserved);
    let (n, _, r) = name_filter.0.len();
    info!(
        "chat filters: {} profanity expressions, {} spam expressions; name filters: {n} profane, \
         {r} reserved",
        filter.profanity_len(),
        filter.spam_len()
    );
    commands.insert_resource(filter);
    commands.insert_resource(name_filter);
}

pub(crate) struct TextFilterPlugin;

/// The two switches' change callback, as the reference's `0x403570`/`0x4035b0` mirror
/// `SStrToInt(newValue)` into a global.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut switches: ResMut<TextFilterSwitches>) {
    match ev.key().as_str() {
        "profanityfilter" => switches.profanity = ev.flag(),
        "spamfilter" => switches.spam = ev.flag(),
        _ => {}
    }
}

impl Plugin for TextFilterPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        app.init_resource::<TextFilterSwitches>()
            .init_resource::<TextFilter>()
            .add_systems(
                Startup,
                load_text_filter_lists.after(benilla_assets::AssetSet::Open),
            );
    }
}
