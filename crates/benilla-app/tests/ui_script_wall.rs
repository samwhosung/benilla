//! A ratchet on how many `benilla-app` files outside `ui_script/` name the Lua VM's `UiScript`:
//! only the feed layer should know the FrameXML VM exists, and everything else produces model
//! values.
//!
//! A file counts if it contains the identifier `UiScript` anywhere (a type, a `use`, a doc
//! comment). `CEILING` is the standing count of gameplay files and `SLACK` how far under it the
//! count may sit, as in `world_api_wall.rs`. `WOW_UISCRIPT_DUMP=1` prints the files.
//!
//! Window feeds (`ui_*`), `capture/` probes and test modules are reported; only gameplay files,
//! the ones that can reach zero, are gated.

use std::path::{Path, PathBuf};

/// What kind of file names the VM.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Family {
    /// Gameplay code that knows the FrameXML VM exists: the one family this wall gates.
    Game,
    /// `ui_*`, a window's feed: the legitimate caller. Reported, never gated.
    Feed,
    /// `capture/`, a live probe that asks the stock binding on purpose. Reported.
    Probe,
    /// A test module beside the code it tests. Reported.
    Test,
}

fn family(rel: &str) -> Family {
    let file = rel.rsplit('/').next().unwrap_or(rel);
    if rel.starts_with("capture/") {
        Family::Probe
    } else if file == "tests.rs" || file.ends_with("_tests.rs") || rel.contains("/tests/") {
        Family::Test
    } else if rel.starts_with("ui_") {
        Family::Feed
    } else {
        Family::Game
    }
}

/// The standing count of `Game` files; raise it only with the reason written down.
const CEILING: usize = 40;

/// How far under [`CEILING`] the count may sit before the test asks for the ceiling to follow it.
const SLACK: usize = 4;

#[test]
fn the_vm_is_named_by_no_more_files_than_the_ceiling_says() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<(Family, String)> = rs_files(&src)
        .into_iter()
        .filter_map(|p| {
            let rel = p
                .strip_prefix(&src)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            if rel.starts_with("ui_script/") || rel == "ui_script.rs" {
                return None;
            }
            let text = std::fs::read_to_string(&p).ok()?;
            names_ident(&text, "UiScript").then(|| (family(&rel), rel))
        })
        .collect();
    files.sort();
    let count = |f: Family| files.iter().filter(|(fam, _)| *fam == f).count();
    let n = count(Family::Game);
    eprintln!(
        "files outside ui_script/ naming UiScript: {} — {n} game (ceiling {CEILING}, slack \
         {SLACK}), {} feeds, {} probes, {} tests",
        files.len(),
        count(Family::Feed),
        count(Family::Probe),
        count(Family::Test),
    );
    if std::env::var("WOW_UISCRIPT_DUMP").is_ok() {
        for (fam, rel) in &files {
            eprintln!("  {fam:?}\t{rel}");
        }
    }
    assert!(
        n <= CEILING,
        "{n} gameplay files outside ui_script/ and the feeds name UiScript; the ceiling is \
         {CEILING}.\nA new file learned that the FrameXML VM exists. The line is that \
         only the feed layer may know: feed a model value instead, or — if this file genuinely is \
         feed code — it belongs under a `ui_*` module, and a probe under `capture/`. Raise CEILING \
         here only with the reason, the way world_api_wall.rs requires.\nWOW_UISCRIPT_DUMP=1 lists \
         the files by family."
    );
    assert!(
        n + SLACK >= CEILING,
        "only {n} gameplay files name UiScript and the ceiling still says {CEILING}. Lower CEILING \
         to {n} in this file so the next coupling has to earn its place — the ratchet only holds \
         if the number follows the work down."
    );
}

/// Does `text` contain `ident` as a whole identifier (not as part of a longer one)?
fn names_ident(text: &str, ident: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(i) = text[from..].find(ident) {
        let at = from + i;
        let end = at + ident.len();
        let before_ok = at == 0 || !text[..at].chars().next_back().is_some_and(is_ident);
        let after_ok = !text[end..].chars().next().is_some_and(is_ident);
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn the_identifier_match_is_whole_word() {
    assert!(names_ident("fn f(s: NonSendMut<UiScript>)", "UiScript"));
    assert!(names_ident("UiScript", "UiScript"));
    assert!(!names_ident("UiScriptPlugin only", "UiScript"));
    assert!(!names_ident("MyUiScript", "UiScript"));
    assert!(names_ident("MyUiScript and UiScript", "UiScript"));
}
