//! The macro file format, the reference's own: a `MACRO %d "%s" %s` header (`0x84cb60`), the body
//! lines and a bare `END`, in `macros-cache.txt` and `macros-local.txt` (`0x85de74`, `0x85de88`).
//! benilla ends lines with `\n` where the reference writes `\r\n`, and reads either, so a 1.12
//! `macros-cache.txt` drops into `benilla-config/macros/`. The number is the macro's id, which an
//! action slot holds. A save writes a scope in name order, the sorted table's (`0x4f0ad0`); a load
//! keeps the file's order as the list order the reference's sort and name lookups start from
//! (`MacroState`). The writer's `"%s"` is unescaped and the reader's name token ends at the next
//! quote, so a name holding a `"` does not survive the round trip.

use benilla_ui::script::{
    macro_slot_word, s_str_to_int, s_str_tokenize, MacroView, CHARACTER_ID_BASE, MAX_MACROS,
};

/// The reader's and the writer's line buffers, and the reader's header token buffers: 0x100 bytes
/// (`0x4f0677`, `0x4f0b93`, `0x4f06b3`).
const LINE_BYTES: usize = 0x100;

/// The header's name and icon delimiters (`0x84cb48`), with quotes on.
const NAME_DELIMS: &[u8] = b" \"\r\n";

/// A token as its buffer kept it. Deviation: the reference's 255-byte cut (`0x4f0677`,
/// `0x4f0b93`) can end inside a character and keep its stray bytes; a `String` must stay UTF-8,
/// so the cut backs off to the character's start.
fn token_text(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).unwrap_or_default()
        }
    }
}

/// The lines of `text` as the tokenizer splits them on CR and LF into a [`LINE_BYTES`] buffer:
/// blank lines are no tokens, and each keeps its first 255 bytes.
fn lines(text: &str) -> impl Iterator<Item = String> + '_ {
    let mut cursor = text.as_bytes();
    std::iter::from_fn(move || {
        let line = s_str_tokenize(&mut cursor, LINE_BYTES, b"\r\n");
        (!line.is_empty()).then(|| token_text(line))
    })
}

/// The path prefix the client prepends to a stored icon basename (`0x84ca64`).
pub(super) const ICON_PREFIX: &str = "Interface\\Icons\\";

/// Parse one scope's file into its records in file order, each with its id, as the reader
/// `0x4f0600` walks it: the file splits into [`lines`] (`0x4f0687`). With no record open, a line
/// whose first space-separated token is `MACRO` without case is a header (`0x4f06c1`,
/// `0x4f06d7`), which opens nothing in a scope that holds 18 (`0x4f06f8`): its id is `SStrToInt`
/// of the next space-separated token, and a zero one, or one the scope's file opened already, is
/// no record (`0x4f072a`, `0x4f078b`); then the name and the icon are the next two tokens on
/// `` "\r\n`` with quotes on (`0x4f07bb`, `0x4f0986`), so a name runs to the next quote. A header
/// with an empty name is read but opens no record (`0x4f07c8`), the table skipping it once its
/// id has counted. Within a record each line is body text followed by `\n` (`0x4f0a15`,
/// `0x4f0a2b`), a `MACRO` line included (`0x4f06a2`), until a line that is `END` without case
/// (`0x4f09d3`), where the body is stored (`0x4f09e4`): a record the file ends inside keeps an
/// empty body. The table that takes the records skips what the scope cannot hold and any id it
/// has, and gives an id below its base the base (`MacroState::from_scopes`).
pub(super) fn parse(text: &str, per_character: bool) -> Vec<MacroView> {
    let base = if per_character { CHARACTER_ID_BASE } else { 0 };
    let mut out: Vec<MacroView> = Vec::new();
    // The ids of the records opened so far, as the table holds them: past the base
    // (`0x4f0737`, a signed compare).
    let mut held: std::collections::HashSet<u32> = std::collections::HashSet::new();
    // The open record, its index in `out`, and the body read so far.
    let mut open: Option<(usize, String)> = None;
    for line in lines(text) {
        if let Some((at, body)) = &mut open {
            if benilla_ui::crt::str_cmp_i(&line, "END") == 0 {
                out[*at].body = std::mem::take(body);
                open = None;
            } else {
                body.push_str(&line);
                body.push('\n');
            }
            continue;
        }
        let mut cursor = line.as_bytes();
        let mut token = |delims: &[u8]| token_text(s_str_tokenize(&mut cursor, LINE_BYTES, delims));
        if benilla_ui::crt::str_cmp_i(&token(b" "), "MACRO") != 0 {
            continue; // stray text between records
        }
        // A full scope opens no record (`0x4f06f8`).
        if held.len() >= MAX_MACROS {
            continue;
        }
        let id = s_str_to_int(&token(b" ")) as u32;
        if id == 0 {
            continue;
        }
        // An id the table holds opens no record (`0x4f078b`).
        let as_held = if (id as i32) < base as i32 {
            id.wrapping_add(base)
        } else {
            id
        };
        if held.contains(&as_held) {
            continue;
        }
        let name = token(NAME_DELIMS);
        let icon = token(NAME_DELIMS);
        if !name.is_empty() {
            held.insert(as_held);
            open = Some((out.len(), String::new()));
        }
        out.push(MacroView {
            id,
            name,
            texture: (!icon.is_empty()).then(|| icon_path(icon)),
            body: String::new(),
            local_only: false,
        });
    }
    out
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
/// (`0x4f0b61`), each of the body's [`lines`] with its own line end, so a blank line writes nothing
/// (`0x4f0b8c`-`0x4f0be5`), then `END` (`0x4f0bec`).
pub(super) fn write<'a>(macros: impl Iterator<Item = &'a MacroView>) -> String {
    let mut out = String::new();
    for m in macros {
        let icon = m.texture.as_deref().map(icon_token).unwrap_or_default();
        // `%d`: the id signed.
        out.push_str(&format!("MACRO {} \"{}\" {}\n", m.id as i32, m.name, icon));
        for line in lines(&m.body) {
            out.push_str(&line);
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
        let macros = parse(REAL_112_FILE, false);
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
        let macros = parse(REAL_112_FILE, false);
        let text = write(macros.iter());
        // The reference's line shape, each record under its own id.
        assert!(text.starts_with("MACRO 1 \"f\" Ability_Ambush\n"));
        assert!(text.contains("\nMACRO 3 \"ns\" Ability_BackStab\n"));
        assert!(text.contains("\nMACRO 2 \"w\" Ability_Creature_Cursed_05\n"));
        assert!(text.ends_with("END\n"));
        assert_eq!(parse(&text, false), macros, "round trip");
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
        assert_eq!(parse(&text, false), m);
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
        let macros = parse(text, false);
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
        let macros = parse(text, false);
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
        let macros = parse("  macro 7 \"m\" Ability_Ambush\n/say hi\nEND\n", false);
        assert_eq!(macros.len(), 1);
        assert_eq!((macros[0].id, macros[0].body.as_str()), (7, "/say hi\n"));
    }

    /// The writer tokenizes the body on CR and LF (`0x4f0b8c`) and writes each token with its own
    /// line end (`0x4f0bc5`, `0x4f0bdb`), so a blank line or a trailing newline writes no line.
    #[test]
    fn the_writer_writes_no_blank_line() {
        let m = [MacroView {
            id: 1,
            name: "a".into(),
            texture: None,
            body: "/say one\n\n/say two\r\n".into(),
            local_only: false,
        }];
        assert_eq!(write(m.iter()), "MACRO 1 \"a\" \n/say one\n/say two\nEND\n");
    }

    /// The name token is the tokenizer's with `` "\r\n`` as delimiters and quotes on (`0x4f07bb`,
    /// `0x84cb48`): a leading `"` opens a span the next `"` closes, so a name runs to the next
    /// quote, and the icon token (`0x4f0986`) starts after it. A name holding a quote does not
    /// survive the round trip; an empty name is no record (`0x4f07c8`).
    #[test]
    fn a_name_runs_to_the_next_quote() {
        let m = [MacroView {
            id: 4,
            name: "say \"hi\"".into(),
            texture: Some("Interface\\Icons\\Ability_Ambush".into()),
            body: "/say hi\n".into(),
            local_only: false,
        }];
        let text = write(m.iter());
        assert_eq!(
            text,
            "MACRO 4 \"say \"hi\"\" Ability_Ambush\n/say hi\nEND\n"
        );
        let back = parse(&text, false);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "say ");
        assert_eq!(back[0].texture.as_deref(), Some("Interface\\Icons\\hi"));
        let empty = parse("MACRO 2 \"\" Ability_Ambush\n/say x\nEND\n", false);
        assert_eq!(
            empty.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            [""],
            "read, for the table to skip after it has counted its id"
        );
        let state = MacroState::from_scopes(empty, Vec::new());
        assert_eq!(state.count(false), 0);
    }

    /// The id is `SStrToInt` of its space-separated token (`0x4f0721`): leading digits, a sign, and
    /// a negative id below the scope's base gets the base added (`0x4f0737`, a signed compare).
    #[test]
    fn the_id_is_read_as_sstrtoint_reads_it() {
        let macros = parse(
            "MACRO 12x \"a\" Ability_Ambush\nEND\nMACRO -5 \"b\" Ability_Ambush\nEND\n",
            true,
        );
        let ids: Vec<u32> = macros.iter().map(|m| m.id).collect();
        assert_eq!(ids, [12, (-5i32) as u32]);
        let state = MacroState::from_scopes(Vec::new(), macros);
        let ids: Vec<u32> = state.saved(true).iter().map(|m| m.id).collect();
        assert_eq!(ids, [CHARACTER_ID_BASE + 12, CHARACTER_ID_BASE - 5]);
    }

    /// A header whose id the table holds opens no record (`0x4f0789` into `0x4f0a30`), so the
    /// reader looks for a header on the next line: a `MACRO` line in that body is one. Before
    /// that, a full scope opens none either (`0x4f06f8`).
    #[test]
    fn a_header_with_an_id_already_read_opens_no_record() {
        let text = "MACRO 1 \"a\" Ability_Ambush\n/say a\nEND\n\
             MACRO 1 \"b\" Ability_Ambush\n\
             MACRO 2 \"c\" Ability_Ambush\n/say c\nEND\n";
        let seen: Vec<(u32, String, String)> = parse(text, false)
            .into_iter()
            .map(|m| (m.id, m.name, m.body))
            .collect();
        assert_eq!(
            seen,
            [
                (1, "a".to_string(), "/say a\n".to_string()),
                (2, "c".to_string(), "/say c\n".to_string())
            ]
        );
        // A character file's small id is the same record as its full one (`0x4f0737` first).
        let text = format!(
            "MACRO {} \"a\" Ability_Ambush\nEND\nMACRO 1 \"b\" Ability_Ambush\n\
             MACRO 2 \"c\" Ability_Ambush\nEND\n",
            CHARACTER_ID_BASE + 1
        );
        let names: Vec<String> = parse(&text, true).into_iter().map(|m| m.name).collect();
        assert_eq!(names, ["a", "c"]);
        let full: String = (1..=19)
            .map(|i| format!("MACRO {i} \"m{i}\" Ability_Ambush\nEND\n"))
            .chain([
                "MACRO 30 \"late\" Ability_Ambush\nMACRO 31 \"x\" Ability_Ambush\nEND\n".into(),
            ])
            .collect();
        let ids: Vec<u32> = parse(&full, false).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, (1..=18).collect::<Vec<u32>>(), "nothing past the 18th");
    }

    /// The header is formatted with `%d` (`0x84cb60`, pushed at `0x4f0b61`): an id is written
    /// signed.
    #[test]
    fn the_writer_writes_the_id_signed() {
        let m = [MacroView {
            id: (-5i32) as u32,
            name: "n".into(),
            texture: None,
            body: String::new(),
            local_only: false,
        }];
        assert_eq!(write(m.iter()), "MACRO -5 \"n\" \nEND\n");
    }

    /// The reader's line buffer and the writer's are 0x100 bytes (`0x4f0677`, `0x4f0b93`), so each
    /// keeps a line's first 255 bytes; a cut inside a character backs off to its start.
    #[test]
    fn a_line_keeps_its_first_255_bytes() {
        let long = "a".repeat(300);
        let wide = "\u{e9}".repeat(200);
        let text = format!("MACRO 1 \"m\" Ability_Ambush\n{long}\n{wide}\nEND\n");
        let body = &parse(&text, false)[0].body;
        assert_eq!(
            body,
            &format!("{}\n{}\n", "a".repeat(255), "\u{e9}".repeat(127))
        );
        let m = [MacroView {
            id: 1,
            name: "m".into(),
            texture: None,
            body: format!("{long}\n{wide}"),
            local_only: false,
        }];
        assert_eq!(
            write(m.iter()),
            format!(
                "MACRO 1 \"m\" \n{}\n{}\nEND\n",
                "a".repeat(255),
                "\u{e9}".repeat(127)
            )
        );
    }

    #[test]
    fn a_full_path_icon_token_is_taken_whole() {
        let text = "MACRO 1 \"custom\" Interface\\Buttons\\UI-Panel-Button\nEND\n";
        let macros = parse(text, false);
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
        let state = MacroState::from_scopes(parse(&text, false), Vec::new());
        assert_eq!(state.count(false), MAX_MACROS);
        assert_eq!(state.count(true), 0);
    }

    /// A character file written by an older build counts its ids from 1; the table adds the scope's
    /// base to them (`0x4f0737`), and the file then keeps the full id, as the reference's does.
    #[test]
    fn a_character_file_with_small_ids_is_rebased_and_saved_with_the_full_id() {
        let text = "MACRO 1 \"mine\" Ability_Ambush\n/say hi\nEND\n";
        let state = MacroState::from_scopes(Vec::new(), parse(text, true));
        let saved = write(state.saved(true).into_iter());
        assert!(
            saved.starts_with("MACRO 16777217 \"mine\" Ability_Ambush\n"),
            "{saved}"
        );
        let again = MacroState::from_scopes(Vec::new(), parse(&saved, true));
        assert_eq!(again, state, "the full id loads as itself");
    }
}
