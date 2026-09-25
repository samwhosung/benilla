//! **No user-facing sentence is written in Rust when the reference ships one.**
//!
//! The real client never composes display text: every sentence is a key into `GlobalStrings.lua`
//! (in-game) or `GlueStrings.lua` (the login/character screens) — each with a `Localize()` patch
//! file laid over it, which is where a good many of the sentences the player actually reads come
//! from — resolved at runtime from the player's own install. Writing the English in
//! Rust throws away three things at once —
//! localization, and, for anything that goes through the message catalog, the *surface* the
//! message is shown on and the *voice line* it speaks with.
//!
//! **Why a tripwire and not a review rule.** Decision 2035 found a loot-refusal table that had
//! composed its own eight sentences, six of which said something 1.12 never says, under a comment
//! asserting they were quoted from the reference. The comment was the only check there had ever
//! been, and a comment cannot fail. What makes that class *findable* is that a re-typed string is
//! byte-identical to the shipped one — so a walk that normalises both sides and compares them finds
//! every instance mechanically, which is what this does.
//!
//! **What it cannot find, stated plainly.** Text that matches nothing is invisible here: an
//! *invented* sentence (2035's "Those pockets are already empty.", for a code 1.12 has no string
//! for) has no shipped counterpart to match against. This tripwire catches the re-typing class,
//! which is the large one; the invention class needs a reader who checks the reference.
//!
//! **A literal beside a key lookup is correct and is not flagged** — a fallback for an install
//! whose chain lacks the key is the reference's own `UNKNOWNOBJECT` shape. The walk only counts a
//! literal whose enclosing function resolves nothing.
//!
//! [`ALLOWED`] is a **ratchet**: it records what was already drifted when the tripwire was built,
//! per file, and the counts may only ever go **down**. Converting a file means lowering its number
//! (or deleting the row); a file not listed may carry none at all. Skips without client data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// **Empty, and that is the point.** This was decision 2045's ratchet — a per-file budget of
/// re-typed sentences that might only ever go down — and it went down to nothing.
///
/// It stays as an empty list rather than being deleted with the machinery, because the walk below
/// is still the guard: a row appearing here is a file that started re-typing again, and the
/// argument for the rule holds whether or not anything is currently breaking it.
///
/// **The number went UP once before it went down, and that is the part worth remembering.** Two
/// defects in the walk itself — a `RESOLVERS` entry that a `sort_by_key` call satisfied, and a
/// `#[cfg(test)]` stripper blind to the out-of-line `mod tests;` — had it reporting 160 where the
/// truth was 180, hiding 36 literals inside one function while double-counting a handful
/// elsewhere. A ratchet is only ever as honest as the walk under it: a number that may only
/// decrease is worthless if the number was never right.
///
/// **What this walk still cannot do**, unchanged from 2045 and worth keeping in view now that the
/// count is zero: it finds *re-typed* strings, because those are byte-identical to the shipped
/// ones. It cannot find *invented* ones — text matching nothing has nothing to match against —
/// and it cannot see a one-word value, which collides with ordinary program text. Three invented
/// slot words and a hand-typed `LockType` name were found in this arc by reading the reference,
/// not by this test.
///
/// **Regenerate against a clean tree, never mid-edit.** The walk is deterministic on the sources
/// plus the shipped tables (two consecutive runs agree exactly), but a baseline captured while
/// other files are half-converted records counts no later tree will reproduce.
const ALLOWED: &[(&str, usize)] = &[];

/// Paths that are never player-facing: dev instruments, probes, capture harnesses and benches.
/// A `.learn <spell>` GM command is not a UI string even when it collides with one.
///
/// **`resolve_bench` is the one entry that is not a *category* but a hole in the walk**, and it is
/// named here rather than papered over. The file is `#[cfg(test)] mod resolve_bench;` — test-only
/// code, exactly what [`strip_test_modules`] exists to drop — but that stripper works *within* a
/// file, and this module's body lives in another one whose name does not contain "test", so the
/// walk visits it as production source. Its three hits are Lua fixtures inside `#[ignore]`d
/// release-only benches (`GameTooltip:AddLine("Main Hand", …)` — a line whose only job is to have
/// a width). Skipping cfg(test)-only *files* properly would mean parsing the module tree; this is
/// the honest one-line stand-in until something needs the general answer.
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

/// Collapse a shipped value and a Rust literal onto the same shape: every placeholder — the
/// reference's `%s`/`%d`/`%1$s` and Rust's `{}`/`{name}` alike — becomes one marker, `\32` is the
/// escaped space it stands for, runs of whitespace collapse, and case is dropped.
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

/// Does this function resolve a key at all? If so, a literal inside it is a fallback.
///
/// Matched on an **identifier boundary** for the bare-word entries, which is not fussiness:
/// `by_key(` is a substring of `sort_by_key(`, and one `bonuses.sort_by_key(…)` was exempting the
/// whole 530-line `render_view` — the largest literal-bearing function in the workspace — from
/// this walk. A resolver list that can be satisfied by an unrelated method name is not a filter,
/// it is a blindfold. Entries that begin with punctuation (`.text(`) are already delimited.
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

/// [`RESOLVERS`], on an identifier boundary for the bare-word entries.
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
    // The base tables AND the locale patches laid over them: where `Localize()`
    // redefines a key, its wording is the one the player actually reads, so a set that stopped at
    // the base files would be grading against text this install never shows.
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
                // **Every** key with this wording, not the first one seen. A value maps to more
                // than one key far more often than you would guess — `CHAT_IGNORED` and
                // `ERR_IGNORING_YOU_S` are the same enUS sentence, and so are
                // `INVTYPE_SHIELD`/`SECONDARYHANDSLOT`, `CHAR_CREATE_NAME_IN_USE`/
                // `CHAR_NAME_RESERVED` — and naming one of them is how a reader converts a site
                // to the wrong key. Which one belongs at a given call site is the *reference's
                // code* to answer, never this table's; the report's job is to say that a choice
                // exists ("assert the identifier, not the sentence").
                shipped.entry(n).or_default().push(k);
                taken += 1;
            }
        }
        // A file that reads fine but parses to nothing would make this whole test pass vacuously
        // — and the two `Localize()` files wrap their assignments in a function, a shape the base
        // tables never have. Every source has to contribute or the walk is grading against less
        // than it claims.
        assert!(taken > 0, "{file} contributed no sentences");
    }

    let mut sources = Vec::new();
    walk(Path::new("src"), &mut sources);
    walk(Path::new("../benilla-ui/src"), &mut sources);
    walk(Path::new("../benilla-formats/src"), &mut sources);

    let allowed: HashMap<&str, usize> = ALLOWED.iter().copied().collect();
    let mut found: HashMap<String, Vec<String>> = HashMap::new();

    for path in &sources {
        // The walk starts at this crate's `src` and at the two sibling crates by relative path;
        // name every hit `<crate>/src/...` so the ratchet rows read the same wherever it ran.
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

    // A ratchet says only whether a file got worse; converting one needs the list. `cargo test -p
    // benilla-app --test reference_strings -- --nocapture` with `BENILLA_REFSTRINGS_REPORT=1`
    // prints every hit — key, function, literal — biggest file first, so a conversion starts from
    // what is actually there rather than from a count.
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
    // The ratchet only bites downward if a *stale* row is an error too — otherwise a converted
    // file's row lingers and silently re-permits the drift it was meant to retire.
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
         (decision 2035){}",
        over.join("")
    );
}

/// Cut every `#[cfg(test)]` item out of a source file before scanning it.
///
/// Without this the walk reads test fixtures as production text — `ui_action::cast_fail`'s unit
/// tests build a *fake* GlobalStrings map whose 48 entries are, by construction, byte-identical to
/// the shipped strings. Those are the test doing its job, not drift.
///
/// **The item's terminator is a brace OR a semicolon, whichever comes first**, and getting that
/// wrong was silently corrupting this walk in both directions. `#[cfg(test)] mod tests;` — the
/// out-of-line form, 28 files in this workspace — has no brace of its own, so a brace-only scan
/// either ran off the end (re-appending the prefix it had already emitted, double-counting every
/// literal above it: that is the whole of `unit/mod.rs`'s row of "2") or, when any later item had
/// a brace, matched THAT one and deleted every line between — hiding real drift with no trace.
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
            // `mod tests;` / `use …;` — the item ends at its semicolon and its body, if it has
            // one, is another file's problem.
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

/// Split a source file into `(fn name, body)` pairs — crude but enough to ask "does the enclosing
/// function resolve a key?", which is the only question here.
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
