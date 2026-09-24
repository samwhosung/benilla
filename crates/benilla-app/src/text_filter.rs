//! The two 1.12 text filters — `profanityFilter`'s **masker** and `spamFilter`'s **predicate**
//! (decision 2077; an emulated run of the binary's own compile/exec over the shipped lists).
//!
//! Both are **PCRE over shipped DBCs**, not word lists: `ChatProfanity.dbc` (2289 rows) feeds the
//! masker `0x4a1a60`, `SpamMessages.dbc` (28 gold-seller URL patterns) feeds the predicate
//! `0x4a1ca0`. The reference compiles them once at startup and never reloads.
//!
//! ## The masker (`0x4a1a60`) — in place, length-preserving, phase-carrying
//!
//! It **self-gates**: `[0x843600]` (the `profanityFilter` mirror) is read at `0x4a1a66`, at the head
//! of the transform, which is why all thirteen of its call sites are CVar-gated without any of them
//! testing the CVar. That is reproduced here — [`TextFilter::mask`] takes the switch, and no call
//! site repeats it.
//!
//! Per matched span it overwrites **one byte per byte** from `.rdata 0x806458` = `!@#$%^&*`,
//! advancing a **process-global index `[0xb6e5f0]` that is never reset**. So the mask phase carries
//! across matches, across messages and across subsystems for the whole session — a client that
//! restarts the cycle per message diverges visibly on the second censored word. Patterns apply in
//! **list order, not text order**, every occurrence, and the string's length never changes (a
//! multi-byte match becomes that many ASCII punctuation bytes, exactly as the reference's byte loop
//! does).
//!
//! **Wired here: the chat chokepoint only** — the fourteen social chat types of `.rdata 0x8046d8`,
//! which is the masker's largest consumer and the one the options row is about. The reference has
//! twelve more call sites and they are named rather than quietly skipped: `SMSG_MAIL_LIST_RESULT`'s
//! subject, `GetInboxText`, `GetGuildInfo`, the two guild-record accessors, `SMSG_GUILD_ROSTER`'s
//! MOTD and info buffers, `GetGuildRosterInfo`, `GuildControlGetRankName`, item text before
//! `ITEM_TEXT_READY`, `SendChatMessage`, and `SMSG_GUILD_EVENT`'s MOTD. Each needs the mask applied
//! where that text enters our model rather than at the Lua getter (benilla publishes a model where
//! the reference re-filters a raw store), which is a different piece of plumbing per subsystem.
//!
//! **The `dl = 1` memo lands with them, not before.** Seven of those twelve pass it, and it is
//! observable: a cache hit re-blits the stored text so the mask index does *not* advance, which is
//! what stops a roster string re-read every repaint from shimmering. Masking once as the text
//! enters the model is that same behaviour by construction, so the memo is a thing those sites will
//! need only if they mask at the getter. Shipping the leg now would be a method nothing calls.
//!
//! ## The predicate (`0x4a1ca0`) — a boolean, never an edit
//!
//! One call site image-wide, inside the chat chokepoint. It never touches the subject text; its
//! out-buffer is written and discarded, and its memo cache is dead code in this build. A positive
//! result **drops the line silently** — nothing is shown in its place.
//!
//! ## PCRE → `regex`, exactly
//!
//! Two translations, and both are the reference's own behaviour rather than a convenience:
//!
//! - **`\<` and `\>` are word boundaries, not literals.** Blizzard *patched* PCRE's escape table
//!   (`.rdata 0x812330`) to give `\<`, `\>` and `\b` the identical code `−4`, so both are
//!   non-directional `\b`. Stock PCRE has no `<`/`>` entries and would make them literals, which
//!   would kill the entire ASCII half of `ChatProfanity`. The binary's own compiler, run on a
//!   reversal control, agrees: `\>twat\<` behaves exactly like `\<twat\>`.
//! - **CASELESS folds ASCII only** — the fold table `0x873c40` is the identity over `0x80..=0xff`.
//!   `regex`'s own `(?i)` folds Unicode, which would match `É` where the reference matches nothing,
//!   so instead both the pattern's literals and the subject are ASCII-lowercased and the match runs
//!   case-*sensitively*. ASCII lowercasing never touches a UTF-8 continuation byte, so every match
//!   offset maps back to the original string unchanged.
//!
//! The shipped patterns use only `. < > s` after a backslash and `? [ ] * + ^ $ .` as metacharacters
//! — no backreferences, no lookaround — so `regex` runs them as PCRE would.

use bevy::prelude::*;
use regex::{Regex, RegexSet};

use benilla_formats::FilterPattern;

/// `.rdata 0x806458` — the eight mask bytes, in order.
const MASK_TOKENS: &[u8; 8] = b"!@#$%^&*";

/// One compiled list: the set (for the cheap "which patterns can match at all" pass) beside the
/// individual expressions, in **list order** — which is the order the mask law walks them in.
struct PatternList {
    set: RegexSet,
    each: Vec<Regex>,
}

impl PatternList {
    /// Compile a list, **skipping a row that fails** exactly as `0x6c94cf`'s success-only increment
    /// leg does — so the live count is the number that compiled, not the row count. On the shipped
    /// data none fails; a row that does is named, because the reference names it too.
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
        // The set is built from the same strings that compiled one by one, so it cannot disagree
        // with `each` about what is in the list.
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

/// **Translate one shipped PCRE pattern** into the equivalent `regex` expression: `\<`/`\>` become
/// `\b`, every other escape is preserved verbatim, and literal ASCII is lowercased so the match can
/// run case-sensitively against an ASCII-lowercased subject (the module doc says why).
///
/// The lowercasing deliberately does **not** reach the character after a backslash: `\S` and `\s`
/// are different classes, and folding one into the other would silently change a pattern. The
/// shipped lists only ever use `\.`, `\<`, `\>` and `\s`, so nothing rides on it today — but a
/// locale archive is free to ship a pattern that does.
fn translate(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c.to_ascii_lowercase());
            continue;
        }
        match chars.next() {
            // The patched escape table: both are the non-directional `\b`.
            Some('<' | '>') => out.push_str("\\b"),
            Some(next) => {
                out.push('\\');
                out.push(next);
            }
            // A trailing backslash is a compile error in PCRE too; let `Regex::new` report it.
            None => out.push('\\'),
        }
    }
    out
}

/// ASCII-only lowercase — the reference's `CASELESS` fold, whose table is the identity over
/// `0x80..=0xff`. Byte-for-byte, so every offset in the result names the same position in the
/// original (a UTF-8 continuation byte is never touched: it is always `>= 0x80`).
fn ascii_lower(text: &str) -> String {
    let mut s = text.to_owned();
    // SAFETY-free equivalent of `make_ascii_lowercase` on the String's bytes: only ASCII letters
    // change, so UTF-8 validity is preserved by construction.
    s.make_ascii_lowercase();
    s
}

/// The two switches, mirrored off their CVars — the reference's `[0x843600]` (`profanityFilter`)
/// and `[0x843604]` (`spamFilter`), each written by that CVar's own change callback.
///
/// Both default **on**: `CVar::Register` at `0x402e68` and `0x402e8e` pushes the shared `"1"`
/// literal `0x82e748` for each, and the registrar runs the callback on the registered default, so a
/// fresh client boots with both mirrors set.
#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct TextFilterSwitches {
    /// `profanityFilter` — the masker's self-gate.
    pub(crate) profanity: bool,
    /// `spamFilter` — the chat chokepoint's spam arm. The options row for it is **inverted**
    /// ("Disable Spam Filter"), which is the row's business, not this flag's.
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

/// The 14 chat types the reference's profanity arm is eligible for — `.rdata 0x8046d8`, read as a
/// table of 14 dwords with the bound `0x38` at `0x49aabb`, i.e. **every player-authored social
/// channel**. A type outside this table is never masked, which is why a monster's yell and a
/// combat-log line reach the frame verbatim.
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

/// The two compiled lists plus the state the reference keeps beside them: the rotating mask index
/// and the masker's per-string memo.
#[derive(Resource)]
pub(crate) struct TextFilter {
    profanity: PatternList,
    spam: PatternList,
    /// `[0xb6e5f0]` — BSS, zero-init, advanced mod 8, **never reset**.
    mask_index: usize,
}

impl Default for TextFilter {
    /// An engine with no lists — what a machine with no client data gets. Both filters then pass
    /// everything through, which is the same shape as the reference booting with an empty list.
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

    /// `0x4a1ca0` — **does this text match the spam list?** A predicate, never an edit.
    ///
    /// The reference scans the shipped list then a server-pushed one (SMSG `0x332`) which starts
    /// empty and stays empty unless the server sends it; vmangos never does, so there is one list
    /// here. It does not exit early on a hit, but the only consumed output is the boolean, so a
    /// first-match-wins scan is observably identical.
    ///
    /// `enabled` is the caller's `spamFilter`; the reference tests it *outside* this function (the
    /// chokepoint's `esi`), unlike the masker's self-gate.
    pub(crate) fn is_spam(&self, enabled: bool, text: &str) -> bool {
        if !enabled || text.is_empty() || self.spam.len() == 0 {
            return false;
        }
        self.spam.set.is_match(&ascii_lower(text))
    }

    /// `0x4a1a60` with `dl = 0` — mask in place, return whether anything matched.
    ///
    /// **Self-gating, like the reference**: `enabled` is `profanityFilter`, tested here so no call
    /// site has to.
    pub(crate) fn mask(&mut self, enabled: bool, text: &mut String) -> bool {
        if !enabled || text.is_empty() || self.profanity.len() == 0 {
            return false;
        }
        self.mask_uncached(text)
    }

    /// The transform itself — the byte loop of `0x4a1b6a`–`0x4a1ba6` under `0x4a1a60`'s pattern
    /// walk.
    ///
    /// **Single pass, list order, over the mutating buffer.** The reference walks every pattern in
    /// order against the buffer it is masking, so a pattern that only becomes matchable because an
    /// *earlier* pattern masked something is caught, while one that would need a *later* pattern's
    /// mask is not — it has already been passed. That asymmetry is reproduced here rather than
    /// smoothed: the candidate set is re-derived after each pattern that actually wrote, and only
    /// entries past the current one are kept.
    fn mask_uncached(&mut self, text: &mut String) -> bool {
        // The matching subject: ASCII-lowercased, byte-aligned with `text`, and masked in step so
        // later patterns see what the reference's later patterns see.
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
                    // The reference latches "something matched" before the length test and then
                    // re-runs from an unadvanced cursor — i.e. an empty-matching pattern spins.
                    // No shipped row can match empty (the test below holds that), so this is the
                    // one place we decline to reproduce the reference and simply stop.
                    break;
                }
                // One token per BYTE of the span — the reference's `mov [esi],cl / inc esi / dec
                // eax` loop, so a three-byte character becomes three punctuation marks. The span is
                // on char boundaries (a regex match always is) and the run is the same byte length,
                // so both strings stay valid UTF-8 and stay byte-aligned with each other.
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
                // The buffer moved: whatever can still match now is what the reference's remaining
                // patterns would see. Keep only the ones it has not walked past.
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

    /// `\<` and `\>` are **word boundaries**, and non-directional — the reversal control run
    /// through the binary's own compiler, which could have failed: the reversed spelling behaves
    /// identically to the ordinary one and to `\b`, and none of the three matches inside a word.
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

    /// The mask is **in place, length-preserving, case-insensitive and unanchored where the pattern
    /// is** — and the phase **carries across calls**. This is an oracle transcript, run against
    /// the binary's own bytes, reproduced pattern for pattern.
    ///
    /// The list order is the oracle's: `\<fagg[aeiouy]t` before `\<twat\>`, which is what makes the
    /// *later* word in the sentence take the *earlier* mask characters. Had the client scanned in
    /// text order the assignment would come out the other way round — the control that could have
    /// failed.
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

    /// The switch is read **inside** the masker, which is why none of the thirteen call sites tests
    /// it: with `profanityFilter = 0` the text is untouched and the phase does not move.
    #[test]
    fn the_masker_self_gates_and_a_gated_call_does_not_move_the_phase() {
        let mut f = TextFilter::new(&list(&[r"\<twat\>"]), &[]);
        let mut line = "you are a twat".to_string();
        assert!(!f.mask(false, &mut line));
        assert_eq!(line, "you are a twat");
        assert_eq!(f.mask_index, 0);
    }

    /// A multi-byte match is masked **byte for byte**: the reference's loop writes one ASCII token
    /// per byte of the span, so a three-byte character becomes three punctuation marks and the
    /// string's byte length never changes.
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

    /// The shipped lists, compiled — every row, through the real chain. Two things are held: the
    /// translation compiles all 2317 rows (the reference's own oracle measured zero failures, so a
    /// skipped row here would be ours, not the data's), and **no shipped pattern can match the
    /// empty string**, which is the assumption `mask_uncached`'s zero-length break rests on.
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

    /// The shipped data end to end: the oracle's own sentences through the real lists.
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

/// The chat chokepoint's view of the two filters — the engine and its switches in one
/// [`SystemParam`], because a Bevy system takes at most sixteen parameters and `feed_chat` had
/// already reached fifteen (the `SpeakerEffects` precedent, same file).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ChatTextFilter<'w> {
    filter: ResMut<'w, TextFilter>,
    switches: Res<'w, TextFilterSwitches>,
}

impl ChatTextFilter<'_> {
    /// Arm 1 — should this line be **dropped**? (`0x49aac9`–`0x49ab33`.)
    ///
    /// The conjuncts, in the reference's own order: the type is not `CHAT_MSG_FILTERED` (`0x5b` —
    /// the type the filter itself produces, so re-filtering it would be circular), the line does
    /// not carry the `"GM"` chat tag, the viewer is not a GM, and `spamFilter` is on. Only then is
    /// the predicate run.
    ///
    /// **A GM viewer skips this arm and still reaches the mask** — all three of the spam arm's
    /// exits converge on `0x49abb7`, the profanity arm's entry, not on the epilogue `0x49afd7`.
    /// (Reading it as "bypassing both arms" is wrong: it would make `spamFilter = 0` disable
    /// profanity masking too.)
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

    /// Arm 2 — mask the line, if its type is one of the fourteen the reference's table names.
    ///
    /// Eligibility is the call site's (`[ebp-8]`, computed at `0x49aa98`–`0x49aac2`); the CVar is
    /// the masker's own (`0x4a1a66`), so it is not tested here.
    pub(crate) fn mask_chat(&mut self, chat_type: u8, text: &mut String) {
        if !PROFANITY_MASKED_CHAT_TYPES.contains(&chat_type) {
            return;
        }
        self.filter.mask(self.switches.profanity, text);
    }
}

/// Load both lists off the patch chain, once, and install the engine.
///
/// The reference does this in its linear startup init (`0x402b7f → 0x6c91a0`) with **no reload
/// path**, so a `Startup` system is the same shape. `.after(AssetSet::Open)` is load-bearing for
/// the reason every other DBC load in this tree carries it: without it the chain does not exist
/// yet, the `Option<Res<_>>` takes its `None` arm, and the filters are silently empty forever.
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
    // A missing list is a filter that passes everything, not a boot failure — the same posture the
    // reference has when a row will not compile: name it and carry on with what did.
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

/// The two filter switches' change callback (decision 2303): flags — the reference's own
/// callbacks (`0x403570`, `0x4035b0`) mirror `SStrToInt(newValue)` into a global the same way.
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
