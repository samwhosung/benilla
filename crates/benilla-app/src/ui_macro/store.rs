//! The macro file format, the reference's own: a `MACRO %d "%s" %s` header (`0x84cb60`), the body
//! lines and a bare `END`, in `macros-cache.txt` and `macros-local.txt` (`0x85de74`, `0x85de88`).
//! benilla ends lines with `\n` where the reference writes `\r\n`, and reads either, so a 1.12
//! `macros-cache.txt` drops into `benilla-config/macros/`. The index only orders the records, and
//! a name holding a `"` runs from the first quote to the last, since the reference's `"%s"` is
//! unescaped.

use benilla_ui::script::{MacroView, MAX_MACROS};

/// The path prefix the client prepends to a stored icon basename (`0x84ca64`).
pub(super) const ICON_PREFIX: &str = "Interface\\Icons\\";

/// Parse one scope's file into a dense macro list: a malformed record is skipped, and records past
/// [`MAX_MACROS`] are dropped.
pub(super) fn parse(text: &str) -> Vec<MacroView> {
    let mut out: Vec<(u32, MacroView)> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim_start().strip_prefix("MACRO ") else {
            continue; // stray text between records
        };
        let Some((index, name, icon)) = parse_header(rest) else {
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

/// The tail of a `MACRO ` header line: `<index> "<name>" <icon>`.
fn parse_header(rest: &str) -> Option<(u32, String, Option<String>)> {
    let (index, rest) = rest.split_once('"')?;
    let index: u32 = index.trim().parse().ok()?;
    // To the last quote on the line: the reference's `"%s"` is unescaped.
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

    /// A real 1.12 `macros-cache.txt`, transcribed with `\n` line ends; its indices are out of
    /// order.
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
        // Sorted by the index column, not by file position (the file is 1, 3, 2).
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
        // The reference's line shape, the list renumbered 1..n.
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

    /// A file truncated before its last `END` still yields that macro.
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

    /// Never wrapped into the other tab: the two scopes are separate files.
    #[test]
    fn a_file_longer_than_the_tab_is_truncated() {
        let text: String = (1..=25)
            .map(|i| format!("MACRO {i} \"m{i}\" Ability_Ambush\nEND\n"))
            .collect();
        assert_eq!(parse(&text).len(), MAX_MACROS);
    }
}
