//! **Where the WoW install is** — THE ONE ANSWER.
//!
//! 0954 made every path benilla *writes* resolve through one module, on the grounds that a
//! hand-built path is a place the rule can be got wrong. Its *input* path never got the same
//! treatment: the string
//! `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WoW/Data")` was hand-copied to 215+
//! sites, alongside 11 bare `var("WOW_DATA")` reads and 271 copies of the same four-line "is it
//! there? if not, skip" guard whose message had drifted into five variants. Every one of those is
//! a place the release rule would have to be written again — and the first Windows build finds
//! every place someone forgot. This module is the rule, written once.
//!
//! **The rule, in the director's words:** *everything should always assume that the wow/data
//! folder is in the project folder or the folder the binary is run from.*
//!
//! It lives in `benilla-formats` because every consumer already depends on it — `benilla-app`,
//! `benilla-assets`, `benilla-world`, and the two detached probes — and because this crate already
//! owns [`crate::Chain`], the thing you open *with* the answer.
//!
//! ## Resolution order
//!
//! 1. **`$WOW_DATA`** — the explicit override, for a second install or a non-standard layout. Kept
//!    because the director's rule is about what the client *assumes*, not about removing the
//!    escape hatch; the sprawl was 11 reads of it, not the variable itself. **Set and empty**
//!    (`WOW_DATA=`) means *there is no install*: the ladder stops there and returns nothing, which
//!    is how a machine that has an install can still run the no-install path.
//! 2. **`<project folder>/WoW/Data`** — `#[cfg(feature = "dev")]` only. Computed from **this
//!    crate's** `CARGO_MANIFEST_DIR`, which lands on the same workspace root whichever crate is
//!    asking, and is what the repo-root `WoW` symlink points at. Gated because a shipped binary
//!    must not carry the build machine's source tree: that is the whole point of the record.
//! 3. **`<exe dir>/Data`, then `<exe dir>/WoW/Data`** — the release convention. Drop benilla into
//!    your WoW folder, or drop a `WoW/` folder beside benilla, and double-click. Both spellings
//!    are cheap to accept and a player will try both.
//!
//! Every candidate must **exist** to be chosen, so a stale `$WOW_DATA` falls through to a real
//! install rather than poisoning the run. [`wow_data`] returns `None` when none of them do — the
//! honest answer, and the one [`wow_data_or_skip`] turns into a uniform test skip.
//!
//! **The feature-unification trap** (called out in the record, and the reason
//! `scripts/gates.sh`'s `--no-default-features` line exists): `dev` is default-on here, so every
//! dependent must declare `default-features = false` and re-export it, or a player build pulls
//! candidate 2 back in through unification and the binary carries `/Users/…` after all. The gate
//! is what catches getting this wrong; [`tests::a_player_build_carries_no_source_tree_path`] is
//! what catches it here.

use std::path::{Path, PathBuf};

/// The vanilla `Data` directory, or `None` when there is no install to be found.
///
/// See the module header for the resolution order and why each step is there. Cheap enough to call
/// at a use site (three `is_dir` stats at worst) — there is deliberately no cache, because a
/// `OnceLock` would freeze the answer across a `$WOW_DATA` change.
pub fn wow_data() -> Option<PathBuf> {
    candidates().into_iter().find(|c| c.is_dir())
}

/// Every place [`wow_data`] looks, in order, whether or not it exists — the resolver's own
/// explanation of itself.
///
/// Public because "no install found" is a message a human has to act on: the skip in
/// [`wow_data_or_skip`] and any future first-run screen both want to say *where we looked*, not
/// just that we failed.
pub fn candidates() -> Vec<PathBuf> {
    candidates_from(
        std::env::var_os("WOW_DATA").map(PathBuf::from),
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf)),
    )
}

/// [`candidates`] with the two environment facts passed in, so the ladder can be tested without
/// touching the process environment.
///
/// That is not tidiness — it is the fix for a real flake. Once every test in the workspace resolves
/// its install through this module, a test that *sets* `$WOW_DATA` poisons every other test running
/// concurrently in the same process, and the failure moves around depending on scheduling. Before
/// 1175 each test baked its own `CARGO_MANIFEST_DIR` path and was immune. The env read now happens
/// in exactly one place that no test mutates; the wiring of that one read is covered out-of-process
/// by `tests/wow_data_env.rs`.
fn candidates_from(override_dir: Option<PathBuf>, exe_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(4);

    // 1 · the explicit override — and its EMPTY spelling. `WOW_DATA=` (set, no value) is the
    // answer *there is no install*: it returns no candidates at all, so [`wow_data`] is `None`
    // even in a dev build with the project folder sitting right there. It exists because the
    // no-install boot path — which every player who unzips benilla into the wrong folder takes —
    // was unreachable in any build we run on this machine, and rotted until it panicked on frame
    // one. `scripts/gates.sh` runs the enforcer under it on every commit; a
    // session can see what a player without data sees with `WOW_DATA= cargo play`.
    if let Some(over) = override_dir {
        if over.as_os_str().is_empty() {
            return Vec::new();
        }
        out.push(over);
    }

    // 2 · the project folder — dev builds only. `CARGO_MANIFEST_DIR` is THIS crate's, so it is the
    // same workspace root for every caller, including the two detached probes. Walked with
    // `ancestors` rather than joined with `../..` so the path prints readably in the "looked in"
    // message a human has to act on.
    #[cfg(feature = "dev")]
    if let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2) {
        out.push(root.join("WoW/Data"));
    }

    // 3 · beside the binary. `current_exe` fails only on exotic platforms and on a deleted exe;
    // there is nothing to fall back to, so an error is simply "no candidate here".
    if let Some(dir) = exe_dir {
        out.push(dir.join("Data"));
        out.push(dir.join("WoW/Data"));
    }

    out
}

/// The install, or **skip this test** — the one replacement for 271 hand-copied existence guards
/// and the five skip messages they had drifted into.
///
/// Expands to an expression, so it reads as an ordinary binding:
///
/// ```ignore
/// let data = wow_data_or_skip!();          // in a `-> ()` test
/// let data = wow_data_or_skip!(None);      // in a helper returning Option
/// ```
///
/// These tests read the **real** 1.12 install, which is gitignored and not on every machine
/// (`docs/METHOD.md`: never commit Blizzard assets), so "no install" has always meant *pass without
/// asserting* rather than *fail*. That is deliberately unchanged; what changes is that the message
/// now names every path that was tried, so a machine where the tests silently do nothing says why.
///
/// **Except where the gate says the data is here** — see [`skipped`]: under
/// `BENILLA_REQUIRE_DATA=1` a skip is a failure.
#[macro_export]
macro_rules! wow_data_or_skip {
    () => {
        $crate::wow_data_or_skip!(())
    };
    ($ret:expr) => {
        match $crate::wow_data() {
            Some(data) => data,
            None => {
                $crate::skipped("no WoW install found", &$crate::candidates());
                return $ret;
            }
        }
    };
}

/// A data-gated test's skip, said out loud — and **refused when the gate says the data is here.**
///
/// libtest swallows a passing test's stderr, and `scripts/gates.sh` keeps no log of a green run,
/// so a skip is invisible at exactly the place it matters: a resolver that has drifted reads as
/// green. That is how the thirty addon-corpus tests skipped at every land for three weeks after
/// the pool moved drives (2026-08-30 → 09-22) — the ladder looked beside the checkout, the slots
/// had moved, and nothing said so. So the gate sets `BENILLA_REQUIRE_DATA=1` on the machine that
/// has the data, and there a skip is what it actually is: a broken resolver, a missing link, a
/// ladder that looks in the wrong place. A machine without the data keeps the skip — third-party
/// content is not on every machine, and never in this repo.
#[doc(hidden)]
pub fn skipped(what: &str, looked_in: &[PathBuf]) {
    skipped_under(
        std::env::var_os("BENILLA_REQUIRE_DATA").is_some_and(|v| !v.is_empty() && v != "0"),
        what,
        looked_in,
    );
    // `$BENILLA_SKIP_LOG`: libtest swallows the line above, so a gate hands in a file and counts
    // the skips after the run — a clone without the data sees how hollow its green is.
    if let Some(log) = std::env::var_os("BENILLA_SKIP_LOG").filter(|p| !p.is_empty()) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            // One write per line: parallel test threads append to the same file, and a
            // formatted write is several syscalls that interleave.
            let _ = f.write_all(format!("{what}\n").as_bytes());
        }
    }
}

/// [`skipped`] with the switch passed in, so the refusal is testable without touching the
/// process environment (the same rule as [`candidates_from`]).
fn skipped_under(required: bool, what: &str, looked_in: &[PathBuf]) {
    let msg = format!("skipping: {what} — looked in {looked_in:?}");
    assert!(
        !required,
        "{msg} — and BENILLA_REQUIRE_DATA is set: the gate resolved this data on this machine, \
         so a skip here is a broken resolver or a missing link, not a missing install"
    );
    eprintln!("{msg}");
}

/// The vanilla addon corpus — the third-party addons the UI engine's real-addon tests run
/// against — or `None` when it is not on this machine. Third-party content, never in this repo;
/// the tests that need it skip through [`addon_corpus_or_skip`] the way the install's do.
///
/// Two rungs, the install's own shape: **`$BENILLA_ADDON_CORPUS`**, then
/// **`<project folder>/wow-addons-vanilla`** — `dev` only, the same `CARGO_MANIFEST_DIR` hop as
/// the install's rung 2. That folder is a gitignored symlink beside `WoW`, laid by whoever set the
/// checkout up; the corpus itself lives outside the tree.
///
/// **Why one rung and a link, not a walk up the tree.** Five test files carried their own copy of
/// this resolver, and every copy looked for a *sibling* of the checkout — the manifest's ancestors
/// two to four, joined with the folder name. That found the corpus from the primary and from the
/// old pool root beside it, and nothing once the pool moved to the external drive (2026-08-30):
/// from a worktree on another volume the three hops name three folders that do not exist.
/// Every land gates in a slot, so thirty tests — a ratchet among them — skipped at every land for
/// three weeks. A resolver that looks *outside* the checkout answers according to where the
/// checkout happens to sit; the install learned this in 1175, and this is the same rule for the
/// second piece of external data the tests read.
pub fn addon_corpus() -> Option<PathBuf> {
    addon_corpus_candidates().into_iter().find(|c| c.is_dir())
}

/// Every place [`addon_corpus`] looks, in order, whether or not it exists — for the skip message.
pub fn addon_corpus_candidates() -> Vec<PathBuf> {
    addon_corpus_candidates_from(std::env::var_os("BENILLA_ADDON_CORPUS").map(PathBuf::from))
}

/// [`addon_corpus_candidates`] with the environment fact passed in — testable without touching
/// the process environment, like [`candidates_from`].
fn addon_corpus_candidates_from(override_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(2);
    if let Some(over) = override_dir {
        out.push(over);
    }
    // The project folder — dev builds only, the install's rung 2 verbatim: a player build has no
    // business knowing where a test corpus was.
    #[cfg(feature = "dev")]
    if let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2) {
        out.push(root.join("wow-addons-vanilla"));
    }
    out
}

/// The addon corpus, or **skip this test** — [`wow_data_or_skip`]'s twin for the second piece of
/// external data the tests read, with the same two spellings (`addon_corpus_or_skip!()` in a
/// `-> ()` test, `addon_corpus_or_skip!(None)` in a helper returning `Option`) and the same
/// `BENILLA_REQUIRE_DATA` refusal.
#[macro_export]
macro_rules! addon_corpus_or_skip {
    () => {
        $crate::addon_corpus_or_skip!(())
    };
    ($ret:expr) => {
        match $crate::addon_corpus() {
            Some(root) => root,
            None => {
                $crate::skipped(
                    "no vanilla addon corpus (set $BENILLA_ADDON_CORPUS)",
                    &$crate::addon_corpus_candidates(),
                );
                return $ret;
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No test in here touches the process environment — see [`candidates_from`] for why.
    fn probe(over: Option<&str>, exe: Option<&str>) -> Vec<PathBuf> {
        candidates_from(over.map(PathBuf::from), exe.map(PathBuf::from))
    }

    /// The override leads, and beside-the-binary always follows in both spellings a player will
    /// try — the release convention, and the only candidates a shipped binary has.
    #[test]
    fn the_ladder_is_override_then_project_folder_then_beside_the_binary() {
        let c = probe(Some("/opt/wow/Data"), Some("/games/benilla"));
        assert_eq!(c.first(), Some(&PathBuf::from("/opt/wow/Data")), "{c:?}");
        assert!(c.contains(&PathBuf::from("/games/benilla/Data")), "{c:?}");
        assert!(
            c.contains(&PathBuf::from("/games/benilla/WoW/Data")),
            "{c:?}"
        );

        // Both halves are optional and their absence must not shift the rest.
        assert_eq!(
            probe(None, Some("/games/benilla")).last(),
            Some(&PathBuf::from("/games/benilla/WoW/Data"))
        );
        assert!(!probe(Some("/opt/wow/Data"), None)
            .iter()
            .any(|p| p.starts_with("/games")));
    }

    /// A candidate only wins if it EXISTS, so a stale `$WOW_DATA` falls through to a real install
    /// rather than poisoning the run. Exercised through [`wow_data`]'s own filter.
    #[test]
    fn a_candidate_that_does_not_exist_is_never_chosen() {
        let tmp = std::env::temp_dir().join(format!("benilla-wd-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("Data")).unwrap();
        let ghost = tmp.join("nope");

        // Which real candidate wins depends on the build (a dev build has the project folder
        // ahead of the exe dir), so the assertion is the property, not the winner: never the
        // ghost, always something that is actually there.
        let chosen = candidates_from(Some(ghost.clone()), Some(tmp.clone()))
            .into_iter()
            .find(|c| c.is_dir());
        assert_ne!(
            chosen,
            Some(ghost),
            "a $WOW_DATA that does not exist must never be chosen"
        );
        assert!(
            chosen.is_some_and(|c| c.is_dir()),
            "a stale override must fall through to a real install"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// `WOW_DATA=` — set and empty — is *there is no install*, and it outranks every other rung
    /// including the dev build's project folder. Without it the no-install path is unreachable on
    /// any machine that has an install, which is every machine this is developed on (1451).
    #[test]
    fn an_empty_override_means_there_is_no_install() {
        assert_eq!(
            probe(Some(""), Some("/games/benilla")),
            Vec::<PathBuf>::new(),
            "`WOW_DATA=` must leave the ladder with no rungs at all"
        );
    }

    /// **The falsifier for the whole record, as a unit test.** A player build — `dev` off — must
    /// not look anywhere inside the source tree that compiled it. If this fails, the project-folder
    /// candidate came back through feature unification and `strings <binary> | grep /Users` will
    /// find it too.
    #[cfg(not(feature = "dev"))]
    #[test]
    fn a_player_build_carries_no_source_tree_path() {
        let root = env!("CARGO_MANIFEST_DIR");
        for c in probe(None, Some("/games/benilla")) {
            assert!(
                !c.starts_with(root),
                "a player build looked inside the source tree at {}",
                c.display()
            );
        }
    }

    /// The dev twin: with the feature on, the project folder IS a candidate — otherwise every
    /// real-data test in the workspace silently turns into a skip and nobody notices.
    #[cfg(feature = "dev")]
    #[test]
    fn a_dev_build_looks_in_the_project_folder() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        let c = probe(None, Some("/games/benilla"));
        assert!(
            c.contains(&root.join("WoW/Data")),
            "the dev build lost its project-folder candidate ({}): {c:?}",
            root.display()
        );
    }

    /// The corpus ladder is the install's in shape: the override leads, and the project folder —
    /// the checkout itself, never a sibling of it — is the only other rung a dev build has.
    #[cfg(feature = "dev")]
    #[test]
    fn the_corpus_ladder_is_override_then_the_project_folder_and_nothing_outside_it() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        let c = addon_corpus_candidates_from(Some(PathBuf::from("/opt/corpus")));
        assert_eq!(
            c,
            vec![
                PathBuf::from("/opt/corpus"),
                root.join("wow-addons-vanilla")
            ],
            "{c:?}"
        );
        for c in addon_corpus_candidates_from(None) {
            assert!(
                c.starts_with(root),
                "a corpus candidate outside the checkout ({}) — that is the resolver whose answer \
                 moved when the pool did",
                c.display()
            );
        }
    }

    /// A player build's corpus ladder is the override alone — a shipped binary knows no test
    /// corpus, exactly as it knows no source tree.
    #[cfg(not(feature = "dev"))]
    #[test]
    fn a_player_build_has_no_corpus_rung_of_its_own() {
        assert_eq!(addon_corpus_candidates_from(None), Vec::<PathBuf>::new());
    }

    /// Unrequired, a skip is a line on stderr and the test goes on.
    #[test]
    fn a_skip_is_a_skip_where_nothing_requires_the_data() {
        skipped_under(false, "no WoW install found", &[PathBuf::from("/nowhere")]);
    }

    /// Required, the same skip is the failure it actually is.
    #[test]
    #[should_panic(expected = "BENILLA_REQUIRE_DATA is set")]
    fn a_skip_is_refused_where_the_gate_says_the_data_is_present() {
        skipped_under(true, "no WoW install found", &[PathBuf::from("/nowhere")]);
    }
}
