//! **The reverse wall of decision 1177, measured** — how many files in `benilla-app` outside the
//! VM's own module name `UiScript`.
//!
//! 1177 (2026-08-10) drew the line: the feed layer becomes one module, the only legitimate caller
//! of the Lua VM, and everything else produces model values. Its instrument was 1160's — a wall
//! test in `tests/` whose number is ratcheted down by the work — and it named the number: **141
//! files**. The instrument was never built, and with nothing failing the count went the other
//! way: 164 files by 2026-09-16. This file is the instrument. It does not
//! decide whether the crate move happens — that stays the director's — it only makes the number
//! visible on every test run and refuses to let it grow unnoticed.
//!
//! "Outside the VM's module" means any `.rs` under `crates/benilla-app/src` whose path does not
//! start with `ui_script/`; 1177's "feed module" does not exist yet, and when it does the
//! exclusion moves there. A file counts if it contains the identifier `UiScript` at all — a type
//! in a parameter list, a `use`, a doc comment naming the seam: each is a place that knows the
//! FrameXML VM exists, which is the coupling 1177 measures.
//!
//! Same shape as `world_api_wall.rs`: `CEILING` is the standing count, `SLACK` keeps a single
//! closure from failing the gate while making it impossible to bank a whole stage of work without
//! writing the new number down. `WOW_UISCRIPT_DUMP=1` prints the files.
//!
//! **Four families, one gated**. The `ui_*` window feeds are 1177's feed layer
//! in a hundred modules (2265 §A3 makes them one); `capture/` probes read the stock binding on
//! purpose; test modules test. None of those is what 1177 meant, so they are reported, and only
//! the remaining gameplay files — the ones that can reach zero — are ratcheted.

use std::path::{Path, PathBuf};

/// Where a file that names the VM stands under 1177.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Family {
    /// Gameplay code that knows the FrameXML VM exists — the camera, targeting, the minimap,
    /// the sound layer, bindings, the glue screens. **1177's number**: the one this wall
    /// gates, and the one that can reach zero.
    Game,
    /// `ui_*` — a window's feed. 1177 makes the feeds THE legitimate caller; that they are a
    /// hundred modules and not one is 2265 §A3's refactor, not a leak. Reported, never gated.
    Feed,
    /// `capture/` — a live probe that asks the STOCK binding on purpose (2283, 2290): fed a
    /// model value it would prove nothing. Reported.
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

/// The standing count of **`Game`** files — gameplay code outside the feeds that names the VM.
///
/// **2026-09-16, 164** was the whole outside-`ui_script/` count the day the instrument was
/// built, against 1177's 141 five weeks earlier; it moved to 167 the same day (2279's
/// structural test, 2283's and 2290's probes — each legitimate, each written down here, and
/// two sessions colliding on this line instead of quietly passing each other). **Decision 2338
/// split the count**: of those 167, 96 were the window feeds, 18 the probes, 8 tests — none of
/// them what 1177 meant — and **45** were gameplay files that know the VM. The wall gates the
/// 45 and reports the rest, so the number here is one that can actually reach 1177's zero.
const CEILING: usize = 45;

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
         {CEILING}.\nA new file learned that the FrameXML VM exists. Decision 1177's line is that \
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
