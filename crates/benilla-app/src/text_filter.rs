//! The two 1.12 text filters: the `profanityFilter` masker (`0x4a1a60`) over `ChatProfanity.dbc`
//! and the `spamFilter` predicate (`0x4a1ca0`) over `SpamMessages.dbc`, both PCRE lists compiled
//! once at startup and never reloaded.
//!
//! The masker self-gates on `[0x843600]` (read at `0x4a1a66`), so no call site tests the CVar. Each
//! matched byte becomes the next of `!@#$%^&*` (`.rdata 0x806458`) through a process-global index
//! (`[0xb6e5f0]`) that is never reset, so the phase carries across matches and messages. Patterns
//! apply in list order, not text order, every occurrence, and the length never changes.
//!
//! Only the chat chokepoint is wired. The reference masks twelve more sites, not built here: the
//! mail subject, `GetInboxText`, `GetGuildInfo`, the two guild-record accessors, the roster MOTD
//! and info, `GetGuildRosterInfo`, `GuildControlGetRankName`, item text, `SendChatMessage` and the
//! guild-event MOTD; seven of them pass the `dl = 1` memo, which re-blits a cached string without
//! advancing the index.
//!
//! The predicate has one call site, in the chat chokepoint; it never edits the text, and a hit
//! drops the line silently.
//!
//! Two PCRE translations are the reference's own behaviour. `\<` and `\>` are the non-directional
//! `\b`: the client's patched escape table (`.rdata 0x812330`) gives all three the code -4.
//! CASELESS folds ASCII only (the fold table `0x873c40` is the identity over `0x80..=0xff`), so
//! the pattern literals and the subject are ASCII-lowercased and matched case-sensitively, which
//! keeps every offset. The shipped patterns use no backreferences or lookaround.

use bevy::prelude::*;
use regex::{Regex, RegexSet};

use benilla_formats::FilterPattern;

/// `.rdata 0x806458`: the eight mask bytes, in order.
const MASK_TOKENS: &[u8; 8] = b"!@#$%^&*";

/// One compiled list: the set for the cheap pass beside the expressions, in list order.
struct PatternList {
    set: RegexSet,
    each: Vec<Regex>,
}

impl PatternList {
    /// Compile a list, skipping and naming a row that fails, as `0x6c94cf` does.
    fn compile(rows: &[FilterPattern], list: &str) -> Self {
        let mut patterns = Vec::with_capacity(rows.len());
        let mut each = Vec::with_capacity(rows.len());
        for row in rows {
            let translated = translate(&row.pattern);
            match Regex::new(&translated) {
                Ok(re) => {
                    patterns.push(translated);
                    each.push(re);
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
}

/// Translate one shipped PCRE pattern to `regex`: `\<`/`\>` become `\b`, other escapes stay
/// verbatim (never lowercased: `\S` is not `\s`), and literal ASCII is lowercased.
fn translate(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c.to_ascii_lowercase());
            continue;
        }
        match chars.next() {
            // The patched escape table: both are `\b`.
            Some('<' | '>') => out.push_str("\\b"),
            Some(next) => {
                out.push('\\');
                out.push(next);
            }
            // A trailing backslash is a compile error in PCRE too.
            None => out.push('\\'),
        }
    }
    out
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
    profanity: PatternList,
    spam: PatternList,
    /// `[0xb6e5f0]`: zero-initialized, advanced mod 8, never reset.
    mask_index: usize,
}

impl Default for TextFilter {
    /// No lists, without client data: both filters pass everything.
    fn default() -> Self {
        TextFilter {
            profanity: PatternList::empty(),
            spam: PatternList::empty(),
            mask_index: 0,
        }
    }
}

impl TextFilter {
    pub(crate) fn new(profanity: &[FilterPattern], spam: &[FilterPattern]) -> Self {
        TextFilter {
            profanity: PatternList::compile(profanity, "chat profanity filter"),
            spam: PatternList::compile(spam, "chat spam filter"),
            mask_index: 0,
        }
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
        self.spam.set.is_match(&ascii_lower(text))
    }

    /// `0x4a1a60` with `dl = 0`: mask in place, return whether anything matched. `enabled` is
    /// `profanityFilter`, tested here as the reference does.
    pub(crate) fn mask(&mut self, enabled: bool, text: &mut String) -> bool {
        if !enabled || text.is_empty() || self.profanity.len() == 0 {
            return false;
        }
        self.mask_uncached(text)
    }

    /// The byte loop of `0x4a1b6a`-`0x4a1ba6`: one pass in list order over the mutating buffer, so
    /// an earlier pattern's mask can enable a later match but never the reverse.
    fn mask_uncached(&mut self, text: &mut String) -> bool {
        // The subject is ASCII-lowercased, byte-aligned with `text` and masked in step.
        let mut subject = ascii_lower(text);
        let mut candidates: Vec<usize> = self.profanity.set.matches(&subject).iter().collect();
        let mut any = false;
        let mut i = 0;
        while i < candidates.len() {
            let p = candidates[i];
            let mut wrote = false;
            let mut cursor = 0;
            while let Some(m) = self.profanity.each[p].find_at(&subject, cursor) {
                let (start, end) = (m.start(), m.end());
                any = true;
                if start == end {
                    // The reference spins on an empty match; this stops instead. No shipped row
                    // can match empty (tested below).
                    break;
                }
                // One token per byte of the span, as the reference's loop writes; a match is on
                // char boundaries, so both strings stay valid UTF-8 and aligned.
                let run: String = (start..end)
                    .map(|_| {
                        let token = MASK_TOKENS[self.mask_index] as char;
                        self.mask_index = (self.mask_index + 1) % MASK_TOKENS.len();
                        token
                    })
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
                    .set
                    .matches(&subject)
                    .iter()
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
            let mut copy = f;
            assert!(copy.mask(true, &mut hit), "{spelling} must match [4,8)");
            assert_eq!(hit, "you !@#$ here", "{spelling}");
            assert!(
                !copy.mask(true, &mut miss),
                "{spelling} must not match inside a word"
            );
            assert_eq!(miss, "atwatb");
        }
    }

    /// A transcript of the reference's masker: in place, length-preserving, caseless, the phase
    /// carrying across calls, and list order giving the later word the earlier mask characters.
    #[test]
    fn the_masker_reproduces_the_oracle_transcript() {
        let mut f = TextFilter::new(&list(&[r"\<fagg[aeiouy]t", r"\<twat\>", "shit"]), &[]);

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
            f.mask_index, 6,
            "the cycle carries forward, and is never reset"
        );
    }

    /// With `profanityFilter = 0` the text is untouched and the phase does not move.
    #[test]
    fn the_masker_self_gates_and_a_gated_call_does_not_move_the_phase() {
        let mut f = TextFilter::new(&list(&[r"\<twat\>"]), &[]);
        let mut line = "you are a twat".to_string();
        assert!(!f.mask(false, &mut line));
        assert_eq!(line, "you are a twat");
        assert_eq!(f.mask_index, 0);
    }

    /// A multi-byte match is masked byte for byte.
    #[test]
    fn a_non_ascii_match_is_masked_byte_for_byte() {
        let mut f = TextFilter::new(&list(&["傻B"]), &[]);
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
                re.as_str()
            );
        }
    }

    /// The transcript's sentences through the real lists.
    #[test]
    fn the_shipped_lists_reproduce_the_oracle_sentences() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let profanity = benilla_formats::load_chat_profanity(&mut chain).expect("profanity");
        let spam = benilla_formats::load_spam_messages(&mut chain).expect("spam");
        let mut f = TextFilter::new(&profanity, &spam);

        let mut line = "you are a twat and a faggot".to_string();
        assert!(f.mask(true, &mut line));
        assert_eq!(
            line, "you are a &*!@ and a !@#$%^",
            "list order beats text order on the real list too"
        );

        let mut clean = "hello friend, nice weather".to_string();
        assert!(!f.mask(true, &mut clean));
        assert_eq!(clean, "hello friend, nice weather");

        assert!(f.is_spam(true, "buy gold at www.itembay.ca"));
        assert!(!f.is_spam(true, "hello friend"));
    }
}

/// The chat chokepoint's view of the two filters, one [`SystemParam`] to keep `feed_chat` under
/// Bevy's sixteen-parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ChatTextFilter<'w> {
    filter: ResMut<'w, TextFilter>,
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
    pub(crate) fn mask_chat(&mut self, chat_type: u8, text: &mut String) {
        if !PROFANITY_MASKED_CHAT_TYPES.contains(&chat_type) {
            return;
        }
        self.filter.mask(self.switches.profanity, text);
    }
}

/// Load both lists off the patch chain once, as the reference's startup init does
/// (`0x402b7f → 0x6c91a0`). It must run after `AssetSet::Open`, or the filters are silently empty.
fn load_text_filter_lists(
    mut commands: Commands,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    let Some(assets) = assets else { return };
    let (profanity, spam) = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        (
            benilla_formats::load_chat_profanity(&mut chain),
            benilla_formats::load_spam_messages(&mut chain),
        )
    };
    // A missing list passes everything; name it and carry on.
    let profanity = profanity
        .inspect_err(|e| warn!("chat profanity filter unavailable: {e:#}"))
        .unwrap_or_default();
    let spam = spam
        .inspect_err(|e| warn!("chat spam filter unavailable: {e:#}"))
        .unwrap_or_default();
    let filter = TextFilter::new(&profanity, &spam);
    info!(
        "chat filters: {} profanity expressions, {} spam expressions",
        filter.profanity_len(),
        filter.spam_len()
    );
    commands.insert_resource(filter);
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
