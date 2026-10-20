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

/// Parse one scope's file into its records in file order, each with its id. A header with no id, or
/// a zero one, is no record (`0x4f072a`); the table that takes them skips what the scope cannot
/// hold and any id it has, and gives an id below its base the base (`MacroState::from_scopes`).
pub(super) fn parse(text: &str) -> Vec<MacroView> {
    let mut out: Vec<MacroView> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim_start().strip_prefix("MACRO ") else {
            continue; // stray text between records
        };
        let Some((id, name, icon)) = parse_header(rest) else {
            continue;
        };
        // The body runs to a bare END, or to the next MACRO or EOF, so a truncated file keeps its
        // last macro.
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
        out.push(MacroView {
            id,
            name,
            texture: icon.map(icon_path),
            body: body.join("\n"),
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

/// Serialize a scope's macros back into the reference's format, in the order given.
pub(super) fn write<'a>(macros: impl Iterator<Item = &'a MacroView>) -> String {
    let mut out = String::new();
    for m in macros {
        let icon = m.texture.as_deref().map(icon_token).unwrap_or_default();
        out.push_str(&format!("MACRO {} \"{}\" {}\n", m.id, m.name, icon));
        if !m.body.is_empty() {
            out.push_str(&m.body);
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
        assert_eq!(macros[0].body, ".cheat fly on\n.modify aspeed 10");
        assert_eq!(macros[2].body, ".wchange 0 0");
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

    /// A file truncated before its last `END` still yields that macro. A header with a zero or
    /// no id is no record (`0x4f0728`).
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
        assert_eq!(macros[1].body, "/say bye", "no END is still a macro");
    }

    #[test]
    fn a_quoted_name_round_trips_the_way_the_reference_would_write_it() {
        let m = vec![MacroView {
            id: 4,
            name: "say \"hi\"".into(),
            texture: Some("Interface\\Icons\\Ability_Ambush".into()),
            body: "/say hi".into(),
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
