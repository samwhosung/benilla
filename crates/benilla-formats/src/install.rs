//! Where the WoW install is: the one resolver for the `Data` folder. The first candidate that
//! exists wins:
//!
//! 1. `$WOW_DATA`. Set and empty (`WOW_DATA=`) means no install and ends the search, so the
//!    no-install path runs on a machine that has one.
//! 2. `<project folder>/WoW/Data`, `dev` builds only, from this crate's `CARGO_MANIFEST_DIR`.
//! 3. `<exe dir>/Data`, then `<exe dir>/WoW/Data`, the release layouts.
//!
//! `dev` is a default feature here, so every dependent declares `default-features = false` and
//! re-exports it; otherwise feature unification puts the source-tree path in a player build.
//! `scripts/gates.sh`'s player build and `tests::a_player_build_carries_no_source_tree_path`
//! catch it.

use std::path::{Path, PathBuf};

/// The vanilla `Data` directory, or `None` when there is no install. Uncached, so a `$WOW_DATA`
/// change is seen.
pub fn wow_data() -> Option<PathBuf> {
    candidates().into_iter().find(|c| c.is_dir())
}

/// Every place [`wow_data`] looks, in order, whether or not it exists, for a "looked in" message.
pub fn candidates() -> Vec<PathBuf> {
    candidates_from(
        std::env::var_os("WOW_DATA").map(PathBuf::from),
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf)),
    )
}

/// [`candidates`] with the environment passed in: a test that set `$WOW_DATA` would change the
/// answer for every test running beside it. `tests/wow_data_env.rs` covers the real read.
fn candidates_from(override_dir: Option<PathBuf>, exe_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(4);

    // 1. The override; set and empty is no install at all, even in a dev build
    // (`WOW_DATA= cargo play` runs as a player without data).
    if let Some(over) = override_dir {
        if over.as_os_str().is_empty() {
            return Vec::new();
        }
        out.push(over);
    }

    // 2. The project folder, dev builds only: this crate's manifest dir, two levels up, is the
    // workspace root for every caller.
    #[cfg(feature = "dev")]
    if let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2) {
        out.push(root.join("WoW/Data"));
    }

    // 3. Beside the binary; a failed `current_exe` adds nothing.
    if let Some(dir) = exe_dir {
        out.push(dir.join("Data"));
        out.push(dir.join("WoW/Data"));
    }

    out
}

/// The install, or skip this test: `wow_data_or_skip!()` in a `-> ()` test,
/// `wow_data_or_skip!(None)` in a helper returning `Option`. The skip names every path tried, and
/// under `BENILLA_REQUIRE_DATA=1` it fails instead.
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

/// A data-gated test's skip, printed; a panic under `BENILLA_REQUIRE_DATA=1`, which the gate sets
/// where the data is, since libtest hides a passing test's stderr and a lost install reads green.
#[doc(hidden)]
pub fn skipped(what: &str, looked_in: &[PathBuf]) {
    skipped_under(
        std::env::var_os("BENILLA_REQUIRE_DATA").is_some_and(|v| !v.is_empty() && v != "0"),
        what,
        looked_in,
    );
    // `$BENILLA_SKIP_LOG`: libtest hides the line above, so the gate counts skips from this file.
    if let Some(log) = std::env::var_os("BENILLA_SKIP_LOG").filter(|p| !p.is_empty()) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            // One write per line: parallel tests append here, and a formatted write can interleave.
            let _ = f.write_all(format!("{what}\n").as_bytes());
        }
    }
}

/// [`skipped`] with the switch passed in, testable without the process environment.
fn skipped_under(required: bool, what: &str, looked_in: &[PathBuf]) {
    let msg = format!("skipping: {what} — looked in {looked_in:?}");
    assert!(
        !required,
        "{msg} — and BENILLA_REQUIRE_DATA is set: the gate resolved this data on this machine, \
         so a skip here is a broken resolver or a missing link, not a missing install"
    );
    eprintln!("{msg}");
}

/// The third-party addon corpus the UI engine's real-addon tests run against, or `None`:
/// `$BENILLA_ADDON_CORPUS`, then `<project folder>/wow-addons-vanilla` (`dev` only), a gitignored
/// link beside `WoW`. Never a folder beside the checkout, which depends on where the checkout sits.
pub fn addon_corpus() -> Option<PathBuf> {
    addon_corpus_candidates().into_iter().find(|c| c.is_dir())
}

/// Every place [`addon_corpus`] looks, in order, whether or not it exists, for the skip message.
pub fn addon_corpus_candidates() -> Vec<PathBuf> {
    addon_corpus_candidates_from(std::env::var_os("BENILLA_ADDON_CORPUS").map(PathBuf::from))
}

/// [`addon_corpus_candidates`] with the environment passed in, like [`candidates_from`].
fn addon_corpus_candidates_from(override_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(2);
    if let Some(over) = override_dir {
        out.push(over);
    }
    // The project folder, dev builds only, as the install's rung 2.
    #[cfg(feature = "dev")]
    if let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2) {
        out.push(root.join("wow-addons-vanilla"));
    }
    out
}

/// The addon corpus, or skip this test, spelled and refused as [`wow_data_or_skip`] is.
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

    /// No test here touches the process environment (see [`candidates_from`]).
    fn probe(over: Option<&str>, exe: Option<&str>) -> Vec<PathBuf> {
        candidates_from(over.map(PathBuf::from), exe.map(PathBuf::from))
    }

    #[test]
    fn the_ladder_is_override_then_project_folder_then_beside_the_binary() {
        let c = probe(Some("/opt/wow/Data"), Some("/games/benilla"));
        assert_eq!(c.first(), Some(&PathBuf::from("/opt/wow/Data")), "{c:?}");
        assert!(c.contains(&PathBuf::from("/games/benilla/Data")), "{c:?}");
        assert!(
            c.contains(&PathBuf::from("/games/benilla/WoW/Data")),
            "{c:?}"
        );

        // Either input may be absent without shifting the rest.
        assert_eq!(
            probe(None, Some("/games/benilla")).last(),
            Some(&PathBuf::from("/games/benilla/WoW/Data"))
        );
        assert!(!probe(Some("/opt/wow/Data"), None)
            .iter()
            .any(|p| p.starts_with("/games")));
    }

    #[test]
    fn a_candidate_that_does_not_exist_is_never_chosen() {
        let tmp = std::env::temp_dir().join(format!("benilla-wd-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("Data")).unwrap();
        let ghost = tmp.join("nope");

        // The winner depends on the build (dev puts the project folder first), so assert only
        // that it is not the ghost and exists.
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

    #[test]
    fn an_empty_override_means_there_is_no_install() {
        assert_eq!(
            probe(Some(""), Some("/games/benilla")),
            Vec::<PathBuf>::new(),
            "`WOW_DATA=` must leave the ladder with no rungs at all"
        );
    }

    /// A failure here means feature unification put the project-folder rung in a player build.
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

    #[cfg(not(feature = "dev"))]
    #[test]
    fn a_player_build_has_no_corpus_rung_of_its_own() {
        assert_eq!(addon_corpus_candidates_from(None), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_skip_is_a_skip_where_nothing_requires_the_data() {
        skipped_under(false, "no WoW install found", &[PathBuf::from("/nowhere")]);
    }

    #[test]
    #[should_panic(expected = "BENILLA_REQUIRE_DATA is set")]
    fn a_skip_is_refused_where_the_gate_says_the_data_is_present() {
        skipped_under(true, "no WoW install found", &[PathBuf::from("/nowhere")]);
    }
}
