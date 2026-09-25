//! The `/who` **filter parser** — the string the player types into the wire fields the server
//! reads.
//!
//! This is engine work in the real client too: the FrameXML hands `SendWho` a raw string
//! (`WhoFrame_GetDefaultWhoCommand` builds `z-"Elwynn Forest" 55-63`), and nothing in Lua turns
//! that into `CMSG_WHO`'s level bounds, masks and **zone ids**. The id part is why the parse
//! belongs here rather than in `benilla-protocol`: `z-` names a zone, the wire wants an
//! `AreaTable.dbc` row id, so the parser needs the DBC projection the app already loads
//! ([`crate::area::AreaTableRes`]).
//!
//! The grammar, from the reference's own vocabulary (`GlobalStrings.lua`'s `WHO_TAG_*` and the
//! filters `WhoFrame_GetDefaultWhoCommand` emits):
//!
//! | token | field |
//! |---|---|
//! | `n-<name>` | name substring |
//! | `g-<guild>` | guild substring |
//! | `z-<zone>` | a zone id (name resolved through `AreaTable`) |
//! | `c-<class>` | a bit in the class mask |
//! | `r-<race>` | a bit in the race mask |
//! | `<lo>-<hi>` | level range |
//! | `<n>` | that exact level (`lo = hi = n`) |
//! | anything else | a search term the server matches against name, guild **and** zone |
//!
//! Values may be quoted (`z-"Elwynn Forest"`), which is how a two-word zone survives the split.
//! Everything is case-insensitive; unmatched class/race words fall through to search terms rather
//! than narrowing the mask to nothing, so a typo returns too much rather than nothing.
//!
//! **INTERIM**: the tag set and the fall-through are transcribed from the
//! reference's strings and its own emitted filters, not from that TU's disassembly — the one
//! place in this slice where the *client* end is inferred rather than byte-read. The dispatch
//! item is named in the record.

use benilla_formats::AreaTableCatalog;
use benilla_protocol::messages::WhoRequest;

use crate::ui_unit::{class_names, race_names};

/// The 1.12 class ids that exist (`class_names`' domain) — the mask universe. Class 6 (Death
/// Knight) and 10 are absent in 1.12, so a mask of "everything" must not claim them: the server
/// tests `classMask & (1 << classId)` against real ids only, but an honest all-classes mask is
/// the union of the ones that exist.
const CLASS_IDS: [u8; 9] = [1, 2, 3, 4, 5, 7, 8, 9, 11];
/// The 1.12 playable race ids (`race_names`' domain).
const RACE_IDS: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// Parse a `/who` filter string into the wire request.
///
/// `areas` resolves `z-` names; without it (no DBCs loaded, e.g. a unit test) a `z-` term
/// degrades to a plain search term, which the server still matches against the zone name — a
/// wider query, never a wrong one.
pub(crate) fn parse(filter: &str, areas: Option<&AreaTableCatalog>) -> WhoRequest {
    let mut request = WhoRequest::default();
    let mut class_mask = 0u32;
    let mut race_mask = 0u32;

    for token in tokenize(filter) {
        let (tag, value) = split_tag(&token);
        match tag {
            Some('n') => request.player_name = value.to_string(),
            Some('g') => request.guild_name = value.to_string(),
            Some('z') => match areas.and_then(|areas| zone_id(areas, value)) {
                Some(id) => request.zones.push(id),
                // Unresolvable zone name: keep it as a search term (the server matches zone
                // names too) rather than dropping the player's intent on the floor.
                None => request.search_terms.push(value.to_string()),
            },
            Some('c') => match mask_bit(value, &CLASS_IDS, class_names) {
                Some(bit) => class_mask |= bit,
                None => request.search_terms.push(value.to_string()),
            },
            Some('r') => match mask_bit(value, &RACE_IDS, race_names) {
                Some(bit) => race_mask |= bit,
                None => request.search_terms.push(value.to_string()),
            },
            _ => {
                if let Some((lo, hi)) = level_range(value) {
                    request.level_min = lo;
                    request.level_max = hi;
                } else if !value.is_empty() {
                    request.search_terms.push(value.to_string());
                }
            }
        }
    }

    // An empty mask means "no c-/r- term was given" — every class/race, the default.
    if class_mask != 0 {
        request.class_mask = class_mask;
    }
    if race_mask != 0 {
        request.race_mask = race_mask;
    }
    request
}

/// Split a filter string into tokens, honouring double quotes so `z-"Elwynn Forest"` survives as
/// one token. An unterminated quote runs to the end of the string (the forgiving read — the
/// player is mid-typing, not writing a config file).
fn tokenize(filter: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in filter.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Split `c-warrior` into `(Some('c'), "warrior")`. Only a single ASCII letter followed by `-`
/// counts, so a bare `1-10` level range and a hyphenated name are not mistaken for tags.
fn split_tag(token: &str) -> (Option<char>, &str) {
    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(tag), Some('-')) if tag.is_ascii_alphabetic() => {
            (Some(tag.to_ascii_lowercase()), &token[2..])
        }
        _ => (None, token),
    }
}

/// `1-10` → `(1, 10)`, `40` → `(40, 40)`; anything else is not a level term. Bounds are clamped
/// to the client's own 1..=100 window (its un-set max is 100 — see [`WhoRequest::level_max`]), so
/// a typo'd `1-9999` still asks a question the server answers.
fn level_range(value: &str) -> Option<(u32, u32)> {
    let clamp = |n: u32| n.clamp(1, 100);
    match value.split_once('-') {
        Some((lo, hi)) => {
            let (lo, hi) = (lo.parse::<u32>().ok()?, hi.parse::<u32>().ok()?);
            Some((clamp(lo), clamp(hi.max(lo))))
        }
        None => {
            let n = clamp(value.parse::<u32>().ok()?);
            Some((n, n))
        }
    }
}

/// Match a class/race word against the display names and return its `1 << id` bit.
fn mask_bit(
    value: &str,
    ids: &[u8],
    names: fn(u8) -> Option<(&'static str, &'static str)>,
) -> Option<u32> {
    ids.iter().copied().find_map(|id| {
        let (display, token) = names(id)?;
        // Both spellings answer: "night elf" (the display name) and "nightelf" (the file token).
        (display.eq_ignore_ascii_case(value) || token.eq_ignore_ascii_case(value))
            .then_some(1u32 << id)
    })
}

/// Resolve a zone name to its `AreaTable` id, case-insensitively (the catalog prefers a
/// top-level row over a same-named subzone). Exact names only — there is no fuzzy fallback,
/// because a wrong id is a query about the *wrong zone*; the caller degrades a miss to a search
/// term instead, which the server still matches against zone names.
fn zone_id(areas: &AreaTableCatalog, name: &str) -> Option<u32> {
    areas.id_for_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare `/who` asks the widest question the client ever asks.
    #[test]
    fn an_empty_filter_is_the_default_query() {
        assert_eq!(parse("", None), WhoRequest::default());
        assert_eq!(parse("   ", None), WhoRequest::default());
    }

    /// The tagged forms fill their own fields; the level range is untagged.
    #[test]
    fn tags_route_to_their_fields() {
        let q = parse("n-bob g-\"Legacy of Steel\" 10-20", None);
        assert_eq!(q.player_name, "bob");
        assert_eq!(q.guild_name, "Legacy of Steel");
        assert_eq!((q.level_min, q.level_max), (10, 20));
        assert!(q.search_terms.is_empty(), "tagged terms are not also terms");
    }

    /// A single number is an exact level, not a search term.
    #[test]
    fn a_bare_number_is_an_exact_level() {
        let q = parse("60", None);
        assert_eq!((q.level_min, q.level_max), (60, 60));
        assert!(q.search_terms.is_empty());
    }

    /// Class and race words become mask bits, by display name or file token, case-insensitively.
    #[test]
    fn class_and_race_terms_become_mask_bits() {
        let q = parse("c-Warrior r-\"night elf\"", None);
        assert_eq!(q.class_mask, 1 << 1, "warrior is class 1");
        assert_eq!(q.race_mask, 1 << 4, "night elf is race 4");

        let token_spelling = parse("r-nightelf", None);
        assert_eq!(token_spelling.race_mask, 1 << 4);

        // Two of a kind union rather than overwrite.
        let both = parse("c-mage c-warlock", None);
        assert_eq!(both.class_mask, (1 << 8) | (1 << 9));
    }

    /// An unrecognised class/race word must not narrow the mask to nothing — it falls through to
    /// a search term, so the query returns too much rather than a silent empty list.
    #[test]
    fn an_unknown_class_word_falls_through_to_a_search_term() {
        let q = parse("c-necromancer", None);
        assert_eq!(q.class_mask, u32::MAX, "still every class");
        assert_eq!(q.search_terms, vec!["necromancer".to_string()]);
    }

    /// Without an AreaTable a `z-` term degrades to a search term — the server matches zone names
    /// against those too, so the query widens instead of breaking.
    #[test]
    fn an_unresolvable_zone_degrades_to_a_search_term() {
        let q = parse("z-\"Elwynn Forest\"", None);
        assert!(q.zones.is_empty());
        assert_eq!(q.search_terms, vec!["Elwynn Forest".to_string()]);
    }

    /// Quotes hold multi-word values together; an unterminated quote runs to the end.
    #[test]
    fn quotes_hold_a_value_together() {
        assert_eq!(
            tokenize("z-\"Elwynn Forest\" 1-10"),
            vec!["z-Elwynn Forest".to_string(), "1-10".to_string()]
        );
        assert_eq!(tokenize("n-\"bo"), vec!["n-bo".to_string()]);
    }

    /// A hyphenated word is not a tag (only a single letter + `-` is), and a reversed range is
    /// read as ascending rather than as an empty window.
    #[test]
    fn hyphens_that_are_not_tags() {
        let q = parse("well-met", None);
        assert_eq!(q.search_terms, vec!["well-met".to_string()]);

        let reversed = parse("20-10", None);
        assert_eq!((reversed.level_min, reversed.level_max), (20, 20));
    }

    /// The level window is clamped to the client's own 1..=100.
    #[test]
    fn levels_clamp_to_the_clients_window() {
        let q = parse("0-9999", None);
        assert_eq!((q.level_min, q.level_max), (1, 100));
    }

    /// The zone name resolves against the **real** `AreaTable.dbc` — the fact the `z-` path
    /// rests on, and the one a synthetic catalog could not catch. Skips without client data.
    #[test]
    fn zone_names_resolve_against_real_area_data() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let areas = benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable.dbc");

        let q = parse("z-\"elwynn forest\" 1-10", Some(&areas));
        assert_eq!(q.zones.len(), 1, "one zone id, resolved case-insensitively");
        assert_eq!(
            areas.name(q.zones[0]),
            Some("Elwynn Forest"),
            "and it is the right row"
        );
        assert!(q.search_terms.is_empty(), "resolved, so not a search term");
    }
}
