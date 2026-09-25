//! The `/who` filter parser: the typed string into `CMSG_WHO`'s fields, which the reference's
//! `SendWho` builds in the engine (`0x5aebb0`). `z-` names a zone the wire wants as an
//! `AreaTable.dbc` id, so the parse needs [`crate::area::AreaTableRes`].
//!
//! `n-` name, `g-` guild, `z-` zone, `c-` class and `r-` race (the `WHO_TAG_*` strings, without
//! case); `lo-hi` or `n` a level range; anything else a search term the server matches against
//! name, guild and zone. Values may be quoted. Here a `z-`, `c-` or `r-` word must equal a name,
//! and one that matches nothing becomes a search term; the reference takes every name containing
//! the word, and sends zone 0 or an empty mask for one that matches nothing.

use benilla_formats::AreaTableCatalog;
use benilla_protocol::messages::WhoRequest;

use crate::ui_unit::{class_names, race_names};

/// The 1.12 class ids a `c-` word can name; 6 and 10 do not exist in 1.12.
const CLASS_IDS: [u8; 9] = [1, 2, 3, 4, 5, 7, 8, 9, 11];
/// The 1.12 playable race ids (`race_names`' domain).
const RACE_IDS: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// Parse a `/who` filter into the wire request. A `z-` name that `areas` cannot resolve becomes a
/// search term, which the server still matches against zone names.
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

    // No `c-` or `r-` term keeps the default all-ones mask.
    if class_mask != 0 {
        request.class_mask = class_mask;
    }
    if race_mask != 0 {
        request.race_mask = race_mask;
    }
    request
}

/// Split a filter on whitespace outside double quotes; an unterminated quote runs to the end.
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

/// Split `c-warrior` into `(Some('c'), "warrior")`. Only one ASCII letter and a `-` is a tag, so
/// `1-10` and a hyphenated word are not.
fn split_tag(token: &str) -> (Option<char>, &str) {
    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(tag), Some('-')) if tag.is_ascii_alphabetic() => {
            (Some(tag.to_ascii_lowercase()), &token[2..])
        }
        _ => (None, token),
    }
}

/// `1-10` is `(1, 10)` and `40` is `(40, 40)`, clamped to 1..=100 with the high bound raised to
/// the low. The reference (`0x5aef7d`) sends the numbers as typed, unclamped, and also reads `N-`
/// as N..100 and `-M` as 0..M.
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

/// A zone name's `AreaTable` id, exact but without case; the catalog prefers a top-level row over
/// a same-named subzone, and the reference skips subzones.
fn zone_id(areas: &AreaTableCatalog, name: &str) -> Option<u32> {
    areas.id_for_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_filter_is_the_default_query() {
        assert_eq!(parse("", None), WhoRequest::default());
        assert_eq!(parse("   ", None), WhoRequest::default());
    }

    #[test]
    fn tags_route_to_their_fields() {
        let q = parse("n-bob g-\"Legacy of Steel\" 10-20", None);
        assert_eq!(q.player_name, "bob");
        assert_eq!(q.guild_name, "Legacy of Steel");
        assert_eq!((q.level_min, q.level_max), (10, 20));
        assert!(q.search_terms.is_empty(), "tagged terms are not also terms");
    }

    #[test]
    fn a_bare_number_is_an_exact_level() {
        let q = parse("60", None);
        assert_eq!((q.level_min, q.level_max), (60, 60));
        assert!(q.search_terms.is_empty());
    }

    /// By display name or file token, without case.
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

    #[test]
    fn an_unknown_class_word_falls_through_to_a_search_term() {
        let q = parse("c-necromancer", None);
        assert_eq!(q.class_mask, u32::MAX, "still every class");
        assert_eq!(q.search_terms, vec!["necromancer".to_string()]);
    }

    #[test]
    fn an_unresolvable_zone_degrades_to_a_search_term() {
        let q = parse("z-\"Elwynn Forest\"", None);
        assert!(q.zones.is_empty());
        assert_eq!(q.search_terms, vec!["Elwynn Forest".to_string()]);
    }

    #[test]
    fn quotes_hold_a_value_together() {
        assert_eq!(
            tokenize("z-\"Elwynn Forest\" 1-10"),
            vec!["z-Elwynn Forest".to_string(), "1-10".to_string()]
        );
        assert_eq!(tokenize("n-\"bo"), vec!["n-bo".to_string()]);
    }

    /// A hyphenated word is a search term, and the reversed range `20-10` reads as level 20.
    #[test]
    fn hyphens_that_are_not_tags() {
        let q = parse("well-met", None);
        assert_eq!(q.search_terms, vec!["well-met".to_string()]);

        let reversed = parse("20-10", None);
        assert_eq!((reversed.level_min, reversed.level_max), (20, 20));
    }

    #[test]
    fn levels_clamp_to_the_clients_window() {
        let q = parse("0-9999", None);
        assert_eq!((q.level_min, q.level_max), (1, 100));
    }

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
