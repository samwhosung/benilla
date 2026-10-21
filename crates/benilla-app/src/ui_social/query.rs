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

use crate::ui_unit::UnitNames;

/// The 1.12 class ids a `c-` word can name; 6 and 10 do not exist in 1.12.
const CLASS_IDS: [u8; 9] = [1, 2, 3, 4, 5, 7, 8, 9, 11];
/// The 1.12 playable race ids.
const RACE_IDS: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// Parse a `/who` filter into the wire request. A `z-` name that `areas` cannot resolve becomes a
/// search term, which the server still matches against zone names.
pub(crate) fn parse(
    filter: &str,
    areas: Option<&AreaTableCatalog>,
    names: UnitNames<'_>,
) -> WhoRequest {
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
            Some('c') => match mask_bit(value, &CLASS_IDS, |id| names.class(id)) {
                Some(bit) => class_mask |= bit,
                None => request.search_terms.push(value.to_string()),
            },
            Some('r') => match mask_bit(value, &RACE_IDS, |id| names.race(id)) {
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
fn mask_bit<'n>(
    value: &str,
    ids: &[u8],
    names: impl Fn(u8) -> Option<(&'n str, &'n str)>,
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
mod tests;
