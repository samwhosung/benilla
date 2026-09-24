//! `ChatProfanity.dbc` + `SpamMessages.dbc` — the two shipped pattern lists behind 1.12's
//! `profanityFilter` and `spamFilter` (decision 2077).
//!
//! **They are regular expressions, not word lists.** The reference compiles every row with PCRE
//! (`0x71fba0` → `pcre_compile 0x720250`, options `0x2801` = CASELESS | UTF8 | NO_UTF8_CHECK) at
//! startup — one linear pass, `0x402b7f → 0x6c91a0`, no reload path — and a row that fails to
//! compile is **skipped** rather than stored, so the live count is the number that compiled. On the
//! shipped data none fails.
//!
//! This module only *reads* the rows. Translating a pattern into something a modern engine runs, and
//! the mask/predicate laws over them, are the client's (`benilla_app::text_filter`).
//!
//! **The archive matters, and it is the easy thing to get wrong.** Both files must resolve through
//! the ordinary priority walk, not out of `dbc.MPQ`: `ChatProfanity` is **2289** rows in `patch.MPQ`
//! against 1512 in `dbc.MPQ`, and `dbc.MPQ` carries no `SpamMessages` at all. Neither filename holds
//! a locale (`"DBFilesClient\ChatProfanity.dbc"` `0x858348`, `"DBFilesClient\SpamMessages.dbc"`
//! `0x859a84`, both literal, neither with a `%s`), so a locale build overrides them one layer down,
//! at the archive — which is exactly what [`crate::Chain`] already does.
//!
//! Layout: 2 × u32 — `{ID(0), Pattern(1)}`, and the pattern is a *plain* string column, not the
//! eight-slot localized shape (the list is multi-language in one table: of ChatProfanity's 2289
//! rows 850 are ASCII, the rest carry high bytes — Korean, Chinese, French).
//!
//! The two lists are **disjoint in practice**: the spam list is 28 gold-seller URL patterns, each
//! letter separated by `\s*` to defeat spacing.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const CHAT_PROFANITY: &str = "DBFilesClient\\ChatProfanity.dbc";
const SPAM_MESSAGES: &str = "DBFilesClient\\SpamMessages.dbc";

/// One row: the DBC id (for diagnostics — a failed compile names it, as the reference's own
/// `"…filter expression: \"%s\" (record ID %d)"` does) and its raw PCRE pattern.
#[derive(Clone, Debug)]
pub struct FilterPattern {
    pub id: u32,
    pub pattern: String,
}

fn schema(name: &str) -> Schema {
    let mut s = Schema::new(name);
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Pattern", FieldType::String));
    s.set_key_field("ID");
    s
}

fn load(chain: &mut Chain, path: &str, name: &str) -> Result<Vec<FilterPattern>> {
    let bytes = chain
        .read_file(path)
        .with_context(|| format!("reading {name}.dbc"))?;
    let rs = parse(&bytes, schema(name), name)?;
    let mut out = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        // **File order is the list order**, and the mask law depends on it: patterns apply in list
        // order, not text order, so the row a sentence's *later* word matches can take the earlier
        // mask characters. Never sort.
        let (Some(id), Some(pattern)) = (u32_at(r, 0), str_at(&rs, r, 1)) else {
            continue;
        };
        if pattern.is_empty() {
            continue;
        }
        out.push(FilterPattern { id, pattern });
    }
    Ok(out)
}

/// `ChatProfanity.dbc` — the masker's list, in file order.
pub fn load_chat_profanity(chain: &mut Chain) -> Result<Vec<FilterPattern>> {
    load(chain, CHAT_PROFANITY, "ChatProfanity")
}

/// `SpamMessages.dbc` — the spam predicate's shipped list, in file order.
///
/// The reference scans this list **then** a second, server-pushed one (SMSG `0x332`, handler
/// `0x49e6e0`), which starts empty and stays empty unless the server sends it. vmangos never does.
pub fn load_spam_messages(chain: &mut Chain) -> Result<Vec<FilterPattern>> {
    load(chain, SPAM_MESSAGES, "SpamMessages")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped lists, measured on the reference client — the row counts its own PCRE
    /// compiled over, and the proof that the priority walk picked the right archive (the
    /// `dbc.MPQ` copies are 1512 rows and *absent* respectively). Skips without client data.
    #[test]
    fn the_lists_come_off_the_priority_walk_at_their_patched_sizes() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");

        let profanity = load_chat_profanity(&mut chain).expect("load ChatProfanity");
        assert_eq!(
            profanity.len(),
            2289,
            "2289 is patch.MPQ's copy; 1512 would mean we read dbc.MPQ"
        );
        assert!(
            profanity.iter().any(|p| p.pattern == r"\<twat\>"),
            "the ASCII half uses the \\< \\> word-boundary anchors"
        );
        assert!(
            profanity.iter().any(|p| !p.pattern.is_ascii()),
            "the list is multi-language in one table"
        );

        let spam = load_spam_messages(&mut chain).expect("load SpamMessages");
        assert_eq!(spam.len(), 28, "dbc.MPQ has no SpamMessages at all");
        assert!(
            spam.iter().all(|p| p.pattern.contains(r"\s*")),
            "every shipped spam row spaces its letters to defeat spacing"
        );
    }
}
