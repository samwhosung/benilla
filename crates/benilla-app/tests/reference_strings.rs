//! No user-facing sentence is written in Rust when the reference ships one: the 1.12 client
//! resolves display text by key from `GlobalStrings.lua` or `GlueStrings.lua` and their
//! `Localize()` patches. A re-typed sentence is byte-identical to the shipped one, so this walk
//! finds it; an invented sentence matches nothing and is invisible here. A literal in a function
//! that also resolves a key is a fallback and is not flagged. Skips without client data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Per-file budgets of re-typed sentences, which may only go down; empty, so no file may carry
/// any. A one-word value is not checked: it collides with ordinary program text.
const ALLOWED: &[(&str, usize)] = &[];

/// Paths that are never player-facing: dev instruments, probes, capture harnesses and benches.
/// `resolve_bench` is a `#[cfg(test)]` module in its own file, which [`strip_test_modules`] cannot
/// see from its parent, so it is named here.
fn is_instrument(rel: &str) -> bool {
    [
        "/capture/",
        "/bin/",
        "debug_panel",
        "shape_gate",
        "resolve_bench",
    ]
    .iter()
    .any(|p| rel.contains(p))
}

/// Collapse a shipped value and a Rust literal onto one shape: every placeholder (`%s`, `%1$s`,
/// `{}`, `{name}`) becomes one marker, `\32` becomes a space, whitespace collapses, case drops.
fn normalize(s: &str) -> String {
    let s = s.replace("\\32", " ");
    let mut out = String::with_capacity(s.len());
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            '{' => {
                // a Rust format hole
                while i < b.len() && b[i] != '}' {
                    i += 1;
                }
                i += 1;
                out.push('\u{1}');
            }
            '%' => {
                let mut j = i + 1;
                while j < b.len()
                    && (b[j].is_ascii_digit() || b[j] == '$' || b[j] == '.' || b[j] == '-')
                {
                    j += 1;
                }
                if j < b.len() && matches!(b[j], 's' | 'd' | 'f' | 'c' | 'g') {
                    out.push('\u{1}');
                    i = j + 1;
                } else {
                    out.push('%');
                    i += 1;
                }
            }
            c => {
                out.push(c.to_ascii_lowercase());
                i += 1;
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse `KEY = "value";` out of a Lua string table.
fn lua_table(src: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in src.lines() {
        let Some((k, rest)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        if k.is_empty()
            || !k
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        {
            continue;
        }
        let rest = rest.trim();
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        if let Some(end) = rest.rfind("\";") {
            out.insert(k.to_string(), rest[..end].to_string());
        }
    }
    out
}

fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if !matches!(name, "target" | ".git" | "tests" | "benches" | "examples") {
                walk(&path, into);
            }
        } else if name.ends_with(".rs") && !name.contains("test") {
            into.push(path);
        }
    }
}

/// Calls that resolve a key; a literal in a function containing one is a fallback.
const RESOLVERS: &[&str] = &[
    "strings.get(",
    ".text(",
    "keyed_line",
    "globals().get::<String>",
    "GlueStrings",
    "by_key(",
    "UiError::key",
    "glue_strings",
];

/// [`RESOLVERS`], on an identifier boundary for the bare-word entries (`by_key(` must not match
/// `sort_by_key(`).
fn resolves(body: &str) -> bool {
    RESOLVERS.iter().any(|r| {
        let word = r.starts_with(|c: char| c.is_alphanumeric() || c == '_');
        body.match_indices(r).any(|(i, _)| {
            !word
                || !body[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
    })
}

#[test]
fn no_user_facing_sentence_is_written_in_rust_when_the_reference_ships_one() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let mut shipped: HashMap<String, Vec<String>> = HashMap::new();
    // The base tables and the locale patches: where `Localize()` redefines a key, its wording is
    // the one the player reads.
    for file in [
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\Localization.lua",
        "Interface\\GlueXML\\GlueStrings.lua",
        "Interface\\GlueXML\\GlueLocalization.lua",
    ] {
        let src = chain
            .read_file(file)
            .unwrap_or_else(|e| panic!("{file}: {e}"));
        let mut taken = 0usize;
        for (k, v) in lua_table(&String::from_utf8_lossy(&src)) {
            let n = normalize(&v);
            // A one-word or punctuation-only value ("Locked", "%s") is too weak a signal: it
            // collides with ordinary program text. Sentences are what this is after.
            if n.split(' ').count() >= 2 && n.chars().any(|c| c.is_ascii_lowercase()) {
                // Every key with this wording: several keys share one enUS sentence
                // (`CHAT_IGNORED`/`ERR_IGNORING_YOU_S`), and the call site decides which applies.
                shipped.entry(n).or_default().push(k);
                taken += 1;
            }
        }
        // Every source must contribute: the `Localize()` files wrap their assignments in a
        // function, a shape the base tables never have.
        assert!(taken > 0, "{file} contributed no sentences");
    }

    let mut sources = Vec::new();
    walk(Path::new("src"), &mut sources);
    walk(Path::new("../benilla-ui/src"), &mut sources);
    walk(Path::new("../benilla-formats/src"), &mut sources);

    let allowed: HashMap<&str, usize> = ALLOWED.iter().copied().collect();
    let mut found: HashMap<String, Vec<String>> = HashMap::new();

    for path in &sources {
        // Name every hit `<crate>/src/...`.
        let raw = path.to_string_lossy().to_string();
        let rel = match raw.strip_prefix("../") {
            Some(sibling) => sibling.to_string(),
            None => format!("benilla-app/{raw}"),
        };
        if is_instrument(&rel) {
            continue;
        }
        let src = strip_test_modules(&std::fs::read_to_string(path).expect("read source"));
        for (fname, body) in split_fns(&src) {
            if resolves(body) {
                continue; // a literal here is a fallback beside a real lookup
            }
            for lit in string_literals(body) {
                let n = normalize(&lit);
                if n.split(' ').count() < 2 {
                    continue;
                }
                if let Some(keys) = shipped.get(&n) {
                    let mut keys = keys.clone();
                    keys.sort_unstable();
                    found
                        .entry(rel.clone())
                        .or_default()
                        .push(format!("{} — {fname}: {lit:?}", keys.join(" | ")));
                }
            }
        }
    }

    // `BENILLA_REFSTRINGS_REPORT=1` with `--nocapture` prints every hit (key, function, literal),
    // biggest file first.
    if std::env::var_os("BENILLA_REFSTRINGS_REPORT").is_some() {
        let mut files: Vec<(&String, &Vec<String>)> = found.iter().collect();
        files.sort_by_key(|(f, h)| (std::cmp::Reverse(h.len()), (*f).clone()));
        let total: usize = files.iter().map(|(_, h)| h.len()).sum();
        println!("\n{total} re-typed literals over {} files", files.len());
        for (file, hits) in files {
            println!("\n{file} ({})", hits.len());
            for h in hits {
                println!("    {h}");
            }
        }
    }

    let mut over = Vec::new();
    for (file, hits) in &found {
        let budget = allowed.get(file.as_str()).copied().unwrap_or(0);
        if hits.len() > budget {
            over.push(format!(
                "\n  {file}: {} literals, ratchet allows {budget}\n{}",
                hits.len(),
                hits.iter()
                    .map(|h| format!("      {h}\n"))
                    .collect::<String>()
            ));
        }
    }
    // A stale row is an error too, or it would re-permit the drift it counted.
    for (file, budget) in ALLOWED {
        let actual = found.get(*file).map_or(0, |h| h.len());
        assert!(
            actual >= *budget,
            "\nRATCHET STALE: {file} now has {actual} re-typed literals but its row still allows \
             {budget}.\nLower it to {actual} (or delete the row at 0) — the count may only go down."
        );
    }
    assert!(
        over.is_empty(),
        "\nA user-facing sentence is written in Rust where the reference ships the string.\n\
         Resolve it by key instead — `keyed_line`/`keyed_line_s` where a `UiScript` is in hand, \
         `UiError::key` into a queue at the net bridge, `GlueStrings::text` on the glue screens.\n\
         Over the ratchet:{}",
        over.join("")
    );
}

/// Cut every `#[cfg(test)]` item out of a source file before scanning it, so test fixtures do not
/// count. An item ends at its brace or its semicolon, whichever comes first (`mod tests;`).
fn strip_test_modules(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        let end = match (after.find('{'), after.find(';')) {
            // A braced item: brace-match it away.
            (Some(open), None) => brace_match(after, open),
            (Some(open), Some(semi)) if open < semi => brace_match(after, open),
            // `mod tests;` or `use …;`: the item ends at its semicolon.
            (_, Some(semi)) => Some(semi + 1),
            (None, None) => None,
        };
        match end {
            Some(e) => rest = &after[e..],
            // A trailing attribute with nothing after it: there is nothing left to keep.
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The index just past the `}` matching the `{` at `open`, or `None` if it never closes.
fn brace_match(src: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a source file into `(fn name, body)` pairs, enough to ask whether the enclosing function
/// resolves a key.
fn split_fns(src: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut starts: Vec<(usize, &str)> = Vec::new();
    for (i, _) in src.match_indices("fn ") {
        // a top-levelish `fn`: preceded only by whitespace/visibility on its line
        let line_start = src[..i].rfind('\n').map_or(0, |n| n + 1);
        let prefix = &src[line_start..i];
        if !prefix.trim_start().is_empty()
            && !matches!(
                prefix.trim(),
                "pub" | "pub(crate)" | "pub(super)" | "const" | "async"
            )
            && !prefix.trim().starts_with("pub(")
        {
            continue;
        }
        let rest = &src[i + 3..];
        let name_end = rest
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        starts.push((i, &rest[..name_end]));
    }
    for (n, (i, name)) in starts.iter().enumerate() {
        let end = starts.get(n + 1).map_or(bytes.len(), |(j, _)| *j);
        out.push((*name, &src[*i..end]));
    }
    if out.is_empty() {
        out.push(("<file>", src));
    } else if let Some((first, _)) = starts.first() {
        out.push(("<consts>", &src[..*first])); // module-level `const … : &str = "…"`
    }
    out
}

/// Every double-quoted literal in a chunk, skipping comment lines and developer-facing macros.
fn string_literals(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        if [
            "debug!",
            "info!",
            "warn!",
            "error!",
            "trace!",
            "println!",
            "eprintln!",
            "panic!",
            "assert",
            "unreachable!",
            "todo!",
            ".expect(",
            "unwrap_or_else",
        ]
        .iter()
        .any(|m| line.contains(m))
        {
            continue;
        }
        let mut rest = line;
        while let Some(start) = rest.find('"') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('"') else { break };
            let lit = &after[..end];
            if lit.len() >= 6 && lit.contains(' ') && lit.chars().any(|c| c.is_ascii_lowercase()) {
                out.push(lit.to_string());
            }
            rest = &after[end + 1..];
        }
    }
    out
}
