//! The table's `Deviates` rows held to the reading `scripts/genmap.sh` makes of them. The
//! generator writes `docs/DEVIATIONS.md`'s settings table from `table.rs` as text (a `cargo run`
//! would make the generator slow), so a row built or written in a way that reading does not
//! handle must fail here, not go missing from the doc.

use std::path::{Path, PathBuf};

use super::table::{Reference, REGISTERED};

/// The arguments of the `n`th `deviates(…)` call in `src` (comments skipped, top-level commas
/// split), each trimmed; `None` when there is no such call.
fn call_args(src: &str, n: usize) -> Option<Vec<String>> {
    let mut at = 0;
    let mut seen = 0;
    while let Some(i) = src[at..].find("deviates(") {
        let start = at + i;
        at = start + "deviates(".len();
        let before = src[..start].trim_end();
        let line_start = src[..start].rfind('\n').map_or(0, |l| l + 1);
        if before.ends_with("fn")
            || src[line_start..start].trim_start().starts_with("//")
            || src[..start].ends_with(|c: char| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        if seen < n {
            seen += 1;
            continue;
        }
        let (mut depth, mut args, mut cur) = (1, Vec::new(), String::new());
        let mut chars = src[at..].chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '/' if chars.peek() == Some(&'/') => {
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                    continue;
                }
                '"' => {
                    cur.push(c);
                    while let Some(c) = chars.next() {
                        cur.push(c);
                        if c == '\\' {
                            cur.extend(chars.next());
                        } else if c == '"' {
                            break;
                        }
                    }
                    continue;
                }
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                ',' if depth == 1 => {
                    args.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(c);
        }
        args.push(cur);
        let mut args: Vec<String> = args.iter().map(|a| a.trim().to_string()).collect();
        if args.last().is_some_and(String::is_empty) {
            args.pop();
        }
        return Some(args);
    }
    None
}

/// A string literal's text, with the escapes the generator reads (`\"`, `\\`, a `\`-newline
/// continuation); `None` for anything else, an identifier path or a literal with other escapes.
fn literal(arg: &str) -> Option<String> {
    let inner = arg.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '\n' => while chars.next_if(|c| c.is_whitespace()).is_some() {},
            _ => return None,
        }
    }
    Some(out)
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// What a constant argument (`crate::realmlist::CVAR_REALMLIST`) names: the `&str` literal of its
/// `const NAME: &str = "…";` in this crate, as the generator greps it.
fn const_str(path: &str) -> Option<String> {
    let name = path.rsplit("::").next()?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let mut files = Vec::new();
    rs_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let needle = format!("const {name}: &str = ");
    files.iter().find_map(|f| {
        let text = std::fs::read_to_string(f).ok()?;
        let line = text.lines().find(|l| l.contains(&needle))?;
        let rest = line.split_once(&needle)?.1.trim_end().strip_suffix(';')?;
        literal(rest)
    })
}

/// Every `Deviates` row is made by `deviates(name, default, value, why)`, once in the table, with
/// four arguments that each read back, as a string literal or a `&str` constant, to the row's own
/// text. Its `Reference::Deviates` is built nowhere else.
#[test]
fn the_deviates_rows_read_back_as_the_generator_reads_them() {
    let src = include_str!("table.rs");
    assert_eq!(
        src.matches("Reference::Deviates {").count(),
        1,
        "a `Deviates` row is built in `deviates()` alone: `scripts/genmap.sh` reads only its calls",
    );
    let rows: Vec<_> = REGISTERED
        .iter()
        .filter_map(|r| match &r.reference {
            Reference::Deviates { value, why } => Some((r.name, r.default, *value, *why)),
            _ => None,
        })
        .collect();
    assert!(!rows.is_empty());
    assert!(
        call_args(src, rows.len()).is_none(),
        "more `deviates(…)` calls than `Deviates` rows",
    );
    for (n, (name, default, value, why)) in rows.into_iter().enumerate() {
        let args = call_args(src, n).unwrap_or_else(|| panic!("{name}: no `deviates(…)` call"));
        assert_eq!(
            args.len(),
            4,
            "{name}: `deviates(name, default, value, why)`"
        );
        for (arg, want, what) in [
            (&args[0], name, "name"),
            (&args[1], default, "default"),
            (&args[2], value, "value"),
            (&args[3], why, "why"),
        ] {
            let read = literal(arg).or_else(|| const_str(arg));
            assert_eq!(
                read.as_deref(),
                Some(want),
                "{name}: the {what} argument {arg:?} is not a string literal or a `&str` \
                 constant that `scripts/genmap.sh` reads; teach the generator or write it as one",
            );
        }
    }
}
