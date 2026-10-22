//! The macro file format, the reference's own: a `MACRO %d "%s" %s` header (`0x84cb60`), the body
//! lines and a bare `END`, in `macros-cache.txt` and `macros-local.txt` (`0x85de74`, `0x85de88`).
//! benilla ends lines with `\n` where the reference writes `\r\n`, and reads either, so a 1.12
//! `macros-cache.txt` drops into `benilla-config/macros/`. The number is the macro's id, which an
//! action slot holds. A save writes a scope in name order, the sorted table's (`0x4f0ad0`); a load
//! keeps the file's order as the list order the reference's sort and name lookups start from
//! (`MacroState`). A name holding a `"` runs from the first quote to the last, since the
//! reference's `"%s"` is unescaped.

use benilla_ui::script::{macro_slot_word, MacroView, CHARACTER_ID_BASE, MAX_MACROS};

/// The path prefix the client prepends to a stored icon basename (`0x84ca64`).
pub(super) const ICON_PREFIX: &str = "Interface\\Icons\\";

/// Parse one scope's file into its records in file order, each with its id, as the reader
/// `0x4f0600` walks it: the file splits into lines on CR and LF, blank ones skipped (`0x4f0687`).
/// With no record open, a line whose first space-separated token is `MACRO` without case opens one
/// (`0x4f06c1`, `0x4f06d7`); a header with no id, or a zero one, is no record (`0x4f072a`). Within
/// a record each line is body text followed by `\n` (`0x4f0a15`, `0x4f0a2b`), a `MACRO` line
/// included (`0x4f06a2`), until a line that is `END` without case (`0x4f09d3`), where the body is
/// stored (`0x4f09e4`): a record the file ends inside keeps an empty body. The table that takes
/// the records skips what the scope cannot hold and any id it has, and gives an id below its base
/// the base (`MacroState::from_scopes`).
pub(super) fn parse(text: &str) -> Vec<MacroView> {
    let mut out: Vec<MacroView> = Vec::new();
    // The open record, its index in `out`, and the body read so far.
    let mut open: Option<(usize, String)> = None;
    for line in text.split(['\r', '\n']).filter(|l| !l.is_empty()) {
        if let Some((at, body)) = &mut open {
            if benilla_ui::crt::str_cmp_i(line, "END") == 0 {
                out[*at].body = std::mem::take(body);
                open = None;
            } else {
                body.push_str(line);
                body.push('\n');
            }
            continue;
        }
        let header = line.trim_start_matches(' ');
        let (token, rest) = header.split_once(' ').unwrap_or((header, ""));
        if benilla_ui::crt::str_cmp_i(token, "MACRO") != 0 {
            continue; // stray text between records
        }
        let Some((id, name, icon)) = parse_header(rest) else {
            continue;
        };
        open = Some((out.len(), String::new()));
        out.push(MacroView {
            id,
            name,
            texture: icon.map(icon_path),
            body: String::new(),
            local_only: false,
        });
    }
    out
}

/// The tail of a `MACRO ` header line: `<id> "<name>" <icon>`.
fn parse_header(rest: &str) -> Option<(u32, String, Option<String>)> {
    let (id, rest) = rest.split_once('"')?;
    let id: u32 = id.trim().parse().ok().filter(|&id| id != 0)?;
    // To the last quote on the line: the reference's `"%s"` is unescaped.
    let close = rest.rfind('"')?;
    let name = rest[..close].to_string();
    let icon = rest[close + 1..].trim();
    Some((id, name, (!icon.is_empty()).then(|| icon.to_string())))
}

/// The bar words of the builds that numbered macros by position, in the character file's terms
/// (`(old word, new word)`, empty when the file is not such a build's).
///
/// Those builds wrote the character file with ids 1..n and read it back sorted by the id column,
/// keeping 18. A character macro's button held `0x40000000 | n`, n the macro's place in that order
/// plus 18, so 19 to 36: the account scope's ids as this build reads them, and the old place of the
/// character macro whose id is now its file id plus [`CHARACTER_ID_BASE`]. A file with an id below
/// the base is such a build's: this build writes the full id.
pub(super) fn legacy_character_words(records: &[MacroView]) -> Vec<(u32, u32)> {
    if records.iter().all(|m| m.id >= CHARACTER_ID_BASE) {
        return Vec::new();
    }
    let mut by_id: Vec<&MacroView> = records.iter().collect();
    by_id.sort_by_key(|m| m.id);
    by_id
        .into_iter()
        .take(MAX_MACROS)
        .enumerate()
        .map(|(place, m)| {
            let id = if m.id < CHARACTER_ID_BASE {
                m.id + CHARACTER_ID_BASE
            } else {
                m.id
            };
            let (kind, action) = macro_slot_word(id);
            let old = 0x4000_0000 | (MAX_MACROS + 1 + place) as u32;
            (old, (u32::from(kind) << 24) | action)
        })
        .collect()
}

/// Serialize a scope's macros back into the reference's format, in the order given: the header
/// (`0x4f0b61`), each body line the tokenizer splits on CR and LF with its own line end, so a
/// blank line writes nothing (`0x4f0b8c`-`0x4f0be5`), then `END` (`0x4f0bec`).
pub(super) fn write<'a>(macros: impl Iterator<Item = &'a MacroView>) -> String {
    let mut out = String::new();
    for m in macros {
        let icon = m.texture.as_deref().map(icon_token).unwrap_or_default();
        out.push_str(&format!("MACRO {} \"{}\" {}\n", m.id, m.name, icon));
        for line in m.body.split(['\r', '\n']).filter(|l| !l.is_empty()) {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("END\n");
    }
    out
}

/// A stored icon token to its texture path; a token with a path separator is taken whole.
fn icon_path(token: String) -> String {
    if token.contains('\\') || token.contains('/') {
        token
    } else {
        format!("{ICON_PREFIX}{token}")
    }
}

/// A texture path to its stored token: the basename in the icons folder, else the whole path.
fn icon_token(path: &str) -> &str {
    path.strip_prefix(ICON_PREFIX).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::{MacroState, MAX_MACROS};

    /// A real 1.12 `macros-cache.txt`, transcribed with `\n` line ends; its ids are out of order.
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
    fn a_real_1_12_macros_cache_file_parses_in_file_order_with_its_ids() {
        let macros = parse(REAL_112_FILE);
        assert_eq!(macros.len(), 3);
        // The file's order is the list order; the ids are the file's own.
        let seen: Vec<(u32, &str)> = macros.iter().map(|m| (m.id, m.name.as_str())).collect();
        assert_eq!(seen, [(1, "f"), (3, "ns"), (2, "w")]);
        assert_eq!(
            macros[0].texture.as_deref(),
            Some("Interface\\Icons\\Ability_Ambush"),
            "the stored token is a basename; the client's own prefix completes it"
        );
        assert_eq!(
            macros[0].body, ".cheat fly on\n.modify aspeed 10\n",
            "each line is followed by a newline, the last too (`0x4f0a2b`)"
        );
        assert_eq!(macros[2].body, ".wchange 0 0\n");
    }

    #[test]
    fn write_then_parse_round_trips_and_matches_the_reference_shape() {
        let macros = parse(REAL_112_FILE);
        let text = write(macros.iter());
        // The reference's line shape, each record under its own id.
        assert!(text.starts_with("MACRO 1 \"f\" Ability_Ambush\n"));
        assert!(text.contains("\nMACRO 3 \"ns\" Ability_BackStab\n"));
        assert!(text.contains("\nMACRO 2 \"w\" Ability_Creature_Cursed_05\n"));
        assert!(text.ends_with("END\n"));
        assert_eq!(parse(&text), macros, "round trip");
        assert_eq!(text, REAL_112_FILE, "the file as it was read");
    }

    #[test]
    fn an_empty_body_and_a_missing_icon_survive_the_round_trip() {
        let m = vec![MacroView {
            id: 1,
            name: "bare".into(),
            texture: None,
            body: String::new(),
            local_only: false,
        }];
        let text = write(m.iter());
        assert_eq!(text, "MACRO 1 \"bare\" \nEND\n");
        assert_eq!(parse(&text), m);
    }

    /// A file truncated before its last `END` still yields that macro, with an empty body: the
    /// body is stored at `END` alone (`0x4f09e4`). A header with a zero or no id is no record
    /// (`0x4f0728`).
    #[test]
    fn malformed_records_lose_only_themselves() {
        let text = "garbage line\n\
             MACRO notanumber \"x\" Icon\n\
             END\n\
             MACRO 0 \"zero\" Icon\n\
             END\n\
             MACRO 2 \"good\" Ability_Ambush\n\
             /say hi\n\
             END\n\
             MACRO 3 \"truncated\" Ability_Ambush\n\
             /say bye\n";
        let macros = parse(text);
        assert_eq!(macros.len(), 2);
        assert_eq!((macros[0].id, macros[0].name.as_str()), (2, "good"));
        assert_eq!((macros[1].id, macros[1].name.as_str()), (3, "truncated"));
        assert_eq!(macros[0].body, "/say hi\n");
        assert_eq!(macros[1].body, "", "no END is still a macro, with no body");
    }

    /// Within a record only a line that is `END` without case closes it (`0x4f09c8`, `SStrCmpI`
    /// on the whole line): a `MACRO` line, or an `END` with a space before it, is body text.
    /// Blank lines are no tokens of the reader's line split on CR and LF (`0x4f0687`), so they
    /// vanish.
    #[test]
    fn only_an_end_line_closes_a_record() {
        let text = "MACRO 1 \"a\" Ability_Ambush\r\n\
             /say one\r\n\
             \r\n\
             MACRO 2 \"b\" Ability_Ambush\r\n\
             \x20END\r\n\
             end\r\n\
             MACRO 3 \"c\" Ability_Ambush\n\
             \x20 /say three \n\
             End\n";
        let macros = parse(text);
        let seen: Vec<(u32, &str)> = macros.iter().map(|m| (m.id, m.body.as_str())).collect();
        assert_eq!(
            seen,
            [
                (1, "/say one\nMACRO 2 \"b\" Ability_Ambush\n END\n"),
                (3, "  /say three \n")
            ]
        );
    }

    /// The reader opens a record on a line whose first space-separated token is `MACRO` without
    /// case (`0x4f06c1`, `0x4f06d7`).
    #[test]
    fn a_header_is_read_without_case() {
        let macros = parse("  macro 7 \"m\" Ability_Ambush\n/say hi\nEND\n");
        assert_eq!(macros.len(), 1);
        assert_eq!((macros[0].id, macros[0].body.as_str()), (7, "/say hi\n"));
    }

    /// The writer tokenizes the body on CR and LF (`0x4f0b8c`) and writes each token with its own
    /// line end (`0x4f0bc5`, `0x4f0bdb`), so a blank line or a trailing newline writes no line.
    #[test]
    fn the_writer_writes_no_blank_line() {
        let m = vec![MacroView {
            id: 1,
            name: "a".into(),
            texture: None,
            body: "/say one\n\n/say two\r\n".into(),
            local_only: false,
        }];
        assert_eq!(write(m.iter()), "MACRO 1 \"a\" \n/say one\n/say two\nEND\n");
    }

    #[test]
    fn a_quoted_name_round_trips_the_way_the_reference_would_write_it() {
        let m = vec![MacroView {
            id: 4,
            name: "say \"hi\"".into(),
            texture: Some("Interface\\Icons\\Ability_Ambush".into()),
            body: "/say hi\n".into(),
            local_only: false,
        }];
        assert_eq!(parse(&write(m.iter())), m);
    }

    #[test]
    fn a_full_path_icon_token_is_taken_whole() {
        let text = "MACRO 1 \"custom\" Interface\\Buttons\\UI-Panel-Button\nEND\n";
        let macros = parse(text);
        assert_eq!(
            macros[0].texture.as_deref(),
            Some("Interface\\Buttons\\UI-Panel-Button")
        );
        assert_eq!(write(macros.iter()), text, "and writes back unchanged");
    }

    /// The table keeps 18 per scope (`0x4f06ee`), never wrapping into the other tab: the two scopes
    /// are separate files.
    #[test]
    fn a_file_longer_than_the_tab_is_truncated_by_the_table() {
        let text: String = (1..=25)
            .map(|i| format!("MACRO {i} \"m{i}\" Ability_Ambush\nEND\n"))
            .collect();
        let state = MacroState::from_scopes(parse(&text), Vec::new());
        assert_eq!(state.count(false), MAX_MACROS);
        assert_eq!(state.count(true), 0);
    }

    /// A character file written by an older build counts its ids from 1; the table adds the scope's
    /// base to them (`0x4f0737`), and the file then keeps the full id, as the reference's does.
    #[test]
    fn a_character_file_with_small_ids_is_rebased_and_saved_with_the_full_id() {
        let text = "MACRO 1 \"mine\" Ability_Ambush\n/say hi\nEND\n";
        let state = MacroState::from_scopes(Vec::new(), parse(text));
        let saved = write(state.saved(true).into_iter());
        assert!(
            saved.starts_with("MACRO 16777217 \"mine\" Ability_Ambush\n"),
            "{saved}"
        );
        let again = MacroState::from_scopes(Vec::new(), parse(&saved));
        assert_eq!(again, state, "the full id loads as itself");
    }
}
