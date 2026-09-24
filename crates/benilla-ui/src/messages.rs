//! The client's **message catalog** — the one table that decides where a UI message is shown.
//!
//! Every user-visible refusal, notice and confirmation the engine raises goes through a single
//! function, `CGGameUI::DisplayError` (`0x496720`), which takes a **message id** and nothing else.
//! The id indexes the registry at `0xb4b498`, 465 records of 20 bytes:
//!
//! | offset | field | what it is |
//! |---|---|---|
//! | `+0x00` | [`MessageRecord::key`] | the `GlobalStrings.lua` key, resolved by `FrameScript_GetText` |
//! | `+0x04` | [`MessageRecord::kind`] | **where it goes** — the four-way jump at `0x496888` ([`MsgKind`]) |
//! | `+0x08` | [`MessageRecord::sound`] | a sound cue name, or nothing |
//! | `+0x0c` | [`MessageRecord::type_tag`] | `0x44` = play the cue; anything else is an error-speech id |
//! | `+0x10` | [`MessageRecord::chat_type`] | the chat type handed to `0x49a870` — read on [`MsgKind::Chat`] only |
//!
//! **Why this table is in the tree at all.** benilla names ~160 of these keys across a dozen
//! modules, and until now every one of them arrived by hand-tracing a `push <id>; call 0x496720`
//! in the binary and reading the record by eye — the key *and* the surface, carried separately to
//! the call site, with nothing checking they agreed. That is how [`super::script`]'s petition lines
//! got their split wrong on the first try (`ui_petition::lines`' own module doc records it), and
//! it is a tax on every future message. The registry is a **fact about the client**, of exactly the
//! kind this project already carries as code — opcode numbers, struct offsets, DBC schemas — so it
//! belongs here as a table, once, instead of being re-derived per call site. Nothing localized
//! ships with it: the *keys* are `GlobalStrings.lua` identifiers (interface code, decision 1602)
//! and the text itself is still read at runtime out of the player's own install.
//!
//! It is **generated**, never hand-edited — `scripts/gen-message-catalog.py` from the vendored
//! `reference/1.12-message-catalog.tsv`, extracted by decoding the CRT static initializer
//! `[0x484ca0, 0x488408)` that fills the table.
//!
//! **What the sweep settles that guessing could not:** the split does not follow the key names.
//! `ERR_PETITION_SIGNED_S` is a chat line and `ERR_PETITION_ALREADY_SIGNED` is a red toast;
//! `ERR_FISH_ESCAPED` is yellow and `ERR_INV_FULL` is red. 170 rows are [`MsgKind::Chat`], 28 are
//! [`MsgKind::Info`], 267 are [`MsgKind::Error`] — and **none is the console arm**, which the
//! dispatcher has and this build's table never reaches (`0x63cb50`'s console lines are called
//! directly, not through a record), so [`MsgKind`] models three.

mod catalog;

pub use catalog::CATALOG;

/// Where a message is shown — the record's `+0x04`, as the four-way jump at `0x496888` reads it.
///
/// The reference's fourth arm (`3` → the debug console `0x63cd00`) has **no tenant in 5875's
/// table**: no row carries it, so modelling it would be modelling something that cannot happen.
/// The generator refuses a kind it does not know, so a build whose table did use it would fail
/// loudly here rather than silently pick a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    /// `0` → the chat window, through the shared composer `0x49a870` with the record's own
    /// [`MessageRecord::chat_type`].
    Chat,
    /// `1` → `AddErrorMessage(text, 0)`, which fires `UI_INFO_MESSAGE` (`0xe1`) — the **yellow**
    /// `UIErrorsFrame` line.
    Info,
    /// `2` → `AddErrorMessage(text, 1)`, which fires `UI_ERROR_MESSAGE` (`0xe0`) — the **red**
    /// `UIErrorsFrame` line. The commonest by a wide margin, which is exactly why guessing it
    /// usually works and is not good enough.
    Error,
}

/// One row of `0xb4b498`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageRecord {
    /// The message id — the argument `DisplayError` is called with, and this row's index.
    pub id: u16,
    /// The `GlobalStrings.lua` key whose text is displayed. An absent or empty string shows
    /// nothing at all: the reference's own data-suppression face (`0x4967bd`/`0x4967c5`).
    pub key: &'static str,
    /// Where it is shown.
    pub kind: MsgKind,
    /// The sound cue played with it, or `None` for the 435 silent rows. Read at the display, by
    /// `benilla_app::sound::message` (decision 1815).
    pub sound: Option<&'static str>,
    /// `+0x0c`. `0x44` means "play [`Self::sound`]"; the other 56 rows carry an error-speech line
    /// id instead, spoken in the player's own race and gender voice (decision 1815).
    pub type_tag: u8,
    /// The chat type handed to the composer, meaningful for [`MsgKind::Chat`] rows only. `10`
    /// (`CHAT_MSG_SYSTEM`) for all but three — the skill-up trio `ERR_PROFICIENCY_GAINED_S`,
    /// `ERR_SKILL_GAINED_S` and `ERR_SKILL_UP_SI` speak on `23` (`CHAT_MSG_SKILL`).
    pub chat_type: u8,
}

/// The record for a message id, or `None` past the table's end — the reference's own bound, the
/// `cmp id,0x1d1; jge` at `0x49672c`.
pub fn by_id(id: u16) -> Option<&'static MessageRecord> {
    CATALOG.get(usize::from(id))
}

/// The record for a `GlobalStrings` key. Keys are unique across the table (the generator refuses a
/// duplicate), so this is well defined.
///
/// A linear scan, deliberately: this is reached once per *displayed message* — a toast, a system
/// line — which is a handful per second at the very worst, and a sorted side-table would be one
/// more generated thing to keep true for no measurable gain.
pub fn by_key(key: &str) -> Option<&'static MessageRecord> {
    CATALOG.iter().find(|r| r.key == key)
}

/// Where a message goes, by key — the one question nearly every caller has.
///
/// A key with no row falls back to [`MsgKind::Error`]. That case cannot arise in the reference (a
/// message is an *index*, so an unknown key is not expressible), so it only ever means benilla
/// named a key the client does not have; the red line is both the commonest kind and the most
/// visible place for such a mistake to show itself. `benilla-app`'s
/// `every_error_key_in_the_source_is_a_catalog_row` test is what keeps it unreachable.
pub fn kind_of(key: &str) -> MsgKind {
    by_key(key).map_or(MsgKind::Error, |r| r.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is complete and dense, so `CATALOG[id]` is the reference's own addressing.
    #[test]
    fn ids_are_dense_and_the_table_is_whole() {
        assert_eq!(CATALOG.len(), 465);
        for (i, r) in CATALOG.iter().enumerate() {
            assert_eq!(usize::from(r.id), i, "row {i} carries id {}", r.id);
        }
        assert!(by_id(465).is_none(), "the reference bounds at 0x1d1");
    }

    /// Keys are unique — [`by_key`] would otherwise be answering an ambiguous question.
    #[test]
    fn keys_are_unique() {
        let mut keys: Vec<&str> = CATALOG.iter().map(|r| r.key).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n);
    }

    /// **The split does not follow the key names.** Four rows that a re-implementation reading
    /// English would sort differently than the binary does — the petition pair that was actually
    /// got wrong once, and the fishing verdict that sits one id away from two red neighbours.
    #[test]
    fn the_surface_is_not_guessable_from_the_key() {
        assert_eq!(kind_of("ERR_PETITION_SIGNED_S"), MsgKind::Chat);
        assert_eq!(kind_of("ERR_PETITION_ALREADY_SIGNED"), MsgKind::Error);
        assert_eq!(kind_of("ERR_FISH_ESCAPED"), MsgKind::Info);
        assert_eq!(kind_of("ERR_INV_FULL"), MsgKind::Error);
    }

    /// The three rows that speak on a chat type other than `CHAT_MSG_SYSTEM`.
    #[test]
    fn the_skill_up_rows_speak_on_chat_msg_skill() {
        for key in [
            "ERR_PROFICIENCY_GAINED_S",
            "ERR_SKILL_GAINED_S",
            "ERR_SKILL_UP_SI",
        ] {
            let r = by_key(key).expect(key);
            assert_eq!(r.kind, MsgKind::Chat);
            assert_eq!(r.chat_type, 23, "{key}");
        }
        assert_eq!(by_key("ERR_QUEST_ALREADY_ON").unwrap().chat_type, 10);
    }

    /// A key the client does not have is not expressible as a message id; the fallback exists so
    /// that a benilla typo is loud rather than silent.
    #[test]
    fn an_unknown_key_falls_back_to_the_red_line() {
        assert!(by_key("ERR_NOT_A_REAL_MESSAGE").is_none());
        assert_eq!(kind_of("ERR_NOT_A_REAL_MESSAGE"), MsgKind::Error);
    }
}
