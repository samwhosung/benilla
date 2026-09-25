//! The macro **file format** — the reference client's own, byte-for-byte.
//!
//! The 1.12 client writes each macro with the format string **`MACRO %d "%s" %s`** (byte-read at
//! `0x44cb60`, inside the `UIMacros.cpp` string block) followed by the body's lines and a bare
//! `END`, into `macros-cache.txt`/`macros-local.txt` (`0x45de74`/`0x45de88`). A real file off the
//! director's own install:
//!
//! ```text
//! MACRO 1 "f" Ability_Ambush
//! .cheat fly on
//! .modify aspeed 10
//! END
//! ```
//!
//! benilla writes exactly that. Not nostalgia: it means **a player can drop their vanilla
//! `macros-cache.txt` into `benilla-config/macros/` and their macros are there** — and it keeps the file
//! hand-editable, which a serialized blob would not be. The icon column is the texture's *basename*
//! (`Ability_Ambush`); the client prepends `Interface\Icons\` (`0x44ca64`), which is why the stored
//! token survives an icon list changing under it where a chooser index would silently re-point.
//!
//! Two liberties, both taken because the reference's own writer can produce files its own reader
//! must survive: an index is trusted only as an *ordering key* (the lists are dense — see
//! `benilla_ui::script::macros`), and a name containing a `"` round-trips (the reference's `"%s"`
//! is unescaped, so we take the span between the FIRST and LAST quote on the line, which is what
//! its own writer would have produced).

use benilla_ui::script::{MacroView, MAX_MACROS};

/// The path prefix the client prepends to a stored icon token (`0x44ca64`).
pub(super) const ICON_PREFIX: &str = "Interface\\Icons\\";

/// Parse one scope's file into a dense macro list, longest-effort: a malformed record is skipped
/// with the rest kept, because losing every macro to one bad line is the worse failure. Records
/// beyond [`MAX_MACROS`] are dropped (the tab cannot show them).
pub(super) fn parse(text: &str) -> Vec<MacroView> {
    let mut out: Vec<(u32, MacroView)> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim_start().strip_prefix("MACRO ") else {
            continue; // stray text between records (or a leading blank) — skipped
        };
        let Some((index, name, icon)) = parse_header(rest) else {
            continue;
        };
        // Body: every line up to the bare END (or the next MACRO / EOF, so a file truncated
        // mid-record still yields the macro it was writing).
        let mut body: Vec<&str> = Vec::new();
        loop {
            match lines.peek() {
                None => break,
                Some(l) if l.trim() == "END" => {
                    lines.next();
                    break;
                }
                Some(l) if l.trim_start().starts_with("MACRO ") => break,
                Some(_) => body.push(lines.next().unwrap_or_default()),
            }
        }
        out.push((
            index,
            MacroView {
                name,
                texture: icon.map(icon_path),
                body: body.join("\n"),
                local_only: false,
            },
        ));
    }
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, m)| m).take(MAX_MACROS).collect()
}

/// `<index> "<name>" <icon>` — the tail of a `MACRO ` header line.
fn parse_header(rest: &str) -> Option<(u32, String, Option<String>)> {
    let (index, rest) = rest.split_once('"')?;
    let index: u32 = index.trim().parse().ok()?;
    // The name runs to the LAST quote on the line: the reference's `"%s"` is unescaped, so a name
    // holding a quote is only recoverable from the outside in.
    let close = rest.rfind('"')?;
    let name = rest[..close].to_string();
    let icon = rest[close + 1..].trim();
    Some((index, name, (!icon.is_empty()).then(|| icon.to_string())))
}

/// Serialize a dense macro list back into the reference's format.
pub(super) fn write(macros: &[MacroView]) -> String {
    let mut out = String::new();
    for (i, m) in macros.iter().enumerate() {
        let icon = m.texture.as_deref().map(icon_token).unwrap_or_default();
        out.push_str(&format!("MACRO {} \"{}\" {}\n", i + 1, m.name, icon));
        if !m.body.is_empty() {
            out.push_str(&m.body);
            out.push('\n');
        }
        out.push_str("END\n");
    }
    out
}

/// A stored icon token → the full texture path. A token that already carries a path separator is
/// taken whole (a hand-edited file naming art outside `Interface\Icons`).
fn icon_path(token: String) -> String {
    if token.contains('\\') || token.contains('/') {
        token
    } else {
        format!("{ICON_PREFIX}{token}")
    }
}

/// The inverse: a full texture path → the stored token (the basename, when it sits in the icons
/// folder; the whole path otherwise, so a hand-edited entry round-trips).
fn icon_token(path: &str) -> &str {
    path.strip_prefix(ICON_PREFIX).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The **real file** off the director's 1.12 install (`WTF/Account/WINUSER/macros-cache.txt`),
    /// verbatim — the one input that proves this reader against the reference's own writer. Note
    /// the out-of-order indices: the file is a dump, the index column is the ordering key.
    const REAL_112_FILE: &str = "MACRO 1 \"f\" Ability_Ambush\n\
         .cheat fly on\n\
         .modify aspeed 10\n\
         END\n\
         MACRO 3 \"ns\" Ability_BackStab\n\
         .cheat fly off\n\
         .modify aspeed 1\n\
         END\n\
         MACRO 2 \"w\" Ability_Creature_Cursed_05\n\
         .wchange 0 0\n\
         END\n";

    #[test]
    fn a_real_1_12_macros_cache_file_parses_in_index_order() {
        let macros = parse(REAL_112_FILE);
        assert_eq!(macros.len(), 3);
        // Sorted by the index COLUMN, not by file position (the real file is 1, 3, 2).
        let names: Vec<&str> = macros.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["f", "w", "ns"]);
        assert_eq!(
            macros[0].texture.as_deref(),
            Some("Interface\\Icons\\Ability_Ambush"),
            "the stored token is a basename; the client's own prefix completes it"
        );
        assert_eq!(macros[0].body, ".cheat fly on\n.modify aspeed 10");
        assert_eq!(macros[1].body, ".wchange 0 0");
    }

    #[test]
    fn write_then_parse_round_trips_and_matches_the_reference_shape() {
        let macros = parse(REAL_112_FILE);
        let text = write(&macros);
        // The reference's own line shape, with the list re-densified 1..n.
        assert!(text.starts_with("MACRO 1 \"f\" Ability_Ambush\n"));
        assert!(text.contains("\nMACRO 2 \"w\" Ability_Creature_Cursed_05\n"));
        assert!(text.ends_with("END\n"));
        assert_eq!(parse(&text), macros, "round trip");
    }

    #[test]
    fn an_empty_body_and_a_missing_icon_survive_the_round_trip() {
        let m = vec![MacroView {
            name: "bare".into(),
            texture: None,
            body: String::new(),
            local_only: false,
        }];
        let text = write(&m);
        assert_eq!(text, "MACRO 1 \"bare\" \nEND\n");
        assert_eq!(parse(&text), m);
    }

    /// Longest-effort parsing: a malformed record loses only itself, and a file truncated
    /// mid-record (a crash between the body and its `END`) still yields what was written.
    #[test]
    fn malformed_records_lose_only_themselves() {
        let text = "garbage line\n\
             MACRO notanumber \"x\" Icon\n\
             END\n\
             MACRO 2 \"good\" Ability_Ambush\n\
             /say hi\n\
             END\n\
             MACRO 3 \"truncated\" Ability_Ambush\n\
             /say bye\n";
        let macros = parse(text);
        assert_eq!(macros.len(), 2);
        assert_eq!(macros[0].name, "good");
        assert_eq!(macros[1].name, "truncated");
        assert_eq!(macros[1].body, "/say bye", "no END is still a macro");
    }

    /// A name holding a quote round-trips, because the reference's unescaped `"%s"` means the
    /// only recoverable reading is first-quote-to-last-quote.
    #[test]
    fn a_quoted_name_round_trips_the_way_the_reference_would_write_it() {
        let m = vec![MacroView {
            name: "say \"hi\"".into(),
            texture: Some("Interface\\Icons\\Ability_Ambush".into()),
            body: "/say hi".into(),
            local_only: false,
        }];
        assert_eq!(parse(&write(&m)), m);
    }

    /// A hand-edited entry naming art outside `Interface\Icons` keeps its whole path.
    #[test]
    fn a_full_path_icon_token_is_taken_whole() {
        let text = "MACRO 1 \"custom\" Interface\\Buttons\\UI-Panel-Button\nEND\n";
        let macros = parse(text);
        assert_eq!(
            macros[0].texture.as_deref(),
            Some("Interface\\Buttons\\UI-Panel-Button")
        );
        assert_eq!(write(&macros), text, "and writes back unchanged");
    }

    /// A file with more records than a tab can hold is truncated, never wrapped into the other
    /// tab (the two scopes are separate files).
    #[test]
    fn a_file_longer_than_the_tab_is_truncated() {
        let text: String = (1..=25)
            .map(|i| format!("MACRO {i} \"m{i}\" Ability_Ambush\nEND\n"))
            .collect();
        assert_eq!(parse(&text).len(), MAX_MACROS);
    }
}
